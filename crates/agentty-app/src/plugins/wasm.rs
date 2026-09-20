//! WebAssembly plugins: a compiled program (Rust, or anything else that targets wasm) run inside
//! Agentty instead of as a process of the user's.
//!
//! What makes this safe is not a list of things the module is forbidden to do — it is that a
//! module can only call the functions handed to it. This host defines three
//! (`send`, `log`, `now_ms`), and nothing else exists: no file, no socket, no environment
//! variable, no process, no clock beyond a millisecond counter. Anything a plugin wants from
//! Agentty travels as the same JSON-RPC message a process plugin sends over stdout, so the
//! permissions in `agentty-plugin.json`, the rate limits and the link guard apply unchanged.
//!
//! The interpreter is used deliberately: it compiles nothing at runtime, so Agentty keeps its
//! hardened runtime with no JIT entitlement.
//!
//! ## ABI
//!
//! The module exports:
//!
//! | Export | Meaning |
//! |---|---|
//! | `memory` | the module's linear memory (the standard Rust/C export) |
//! | `agentty_alloc(len: i32) -> i32` | a buffer of `len` bytes for Agentty to write a message into |
//! | `agentty_on_message(ptr: i32, len: i32)` | one UTF-8 JSON message from Agentty |
//!
//! and imports, from the module named `agentty`:
//!
//! | Import | Meaning |
//! |---|---|
//! | `send(ptr: i32, len: i32)` | one UTF-8 JSON message to Agentty |
//! | `log(ptr: i32, len: i32)` | one line for the plugin's log |
//! | `now_ms() -> i64` | milliseconds since the Unix epoch |

use super::process::ProcessEvent;
use agentty_bridge::plugins::store::InstalledPlugin;
use agentty_bridge::plugins::Incoming;
use std::sync::mpsc;
use std::sync::Arc;
use wasmi::{Caller, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TrapCode, TypedFunc};

/// Largest module Agentty loads.
const MAX_MODULE_BYTES: u64 = 64 * 1024 * 1024;
/// Largest linear memory a plugin may grow to.
const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_TABLE_ELEMENTS: usize = 10_000;
/// Longest message in either direction (the line protocol's limit).
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Work one message may cost before the plugin is stopped. Roughly a second of CPU: enough for
/// any panel a plugin draws, and an end to a loop that never returns.
const FUEL_PER_MESSAGE: u64 = 200_000_000;
/// Messages a plugin may send while handling one. A plugin that goes past this is answering a
/// single event with a flood, and is trapped here rather than queued onto the main thread.
const MAX_MESSAGES_PER_DISPATCH: u32 = 256;

/// What the host functions can reach while the plugin runs.
struct HostState {
    events: Arc<dyn Fn(ProcessEvent) + Send + Sync>,
    memory: Option<Memory>,
    limits: StoreLimits,
    /// Messages sent while handling the current message.
    sent: u32,
}

impl HostState {
    /// Reads a UTF-8 string the guest points at, refusing what is out of bounds or too long.
    fn read(memory: &Memory, caller: &Caller<'_, Self>, ptr: i32, len: i32) -> Result<String, wasmi::Error> {
        let (ptr, len) = (ptr as usize, len as usize);
        if len > MAX_MESSAGE_BYTES {
            return Err(fail(format!("a plugin sent a message larger than {MAX_MESSAGE_BYTES} bytes")));
        }
        let data = memory.data(caller);
        let bytes = data.get(ptr..ptr.saturating_add(len)).ok_or_else(|| fail("out of bounds"))?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// An error a host function returns; it ends the guest call as a trap.
#[derive(Debug)]
struct Failure(String);

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failure {}

impl wasmi::errors::HostError for Failure {}

/// A trap carrying a message the plugin's log can show.
fn fail(message: impl Into<String>) -> wasmi::Error {
    wasmi::Error::host(Failure(message.into()))
}

/// A running WebAssembly plugin: the interpreter owns one thread, and messages reach it over a
/// channel. Dropping the sender ends the thread, which is how the plugin is stopped.
pub struct WasmPlugin {
    sender: mpsc::Sender<Command>,
}

enum Command {
    Message(String),
    Shutdown,
}

impl WasmPlugin {
    pub fn start(plugin: &InstalledPlugin, events: impl Fn(ProcessEvent) + Send + Sync + 'static) -> Self {
        let (sender, commands) = mpsc::channel();
        let events: Arc<dyn Fn(ProcessEvent) + Send + Sync> = Arc::new(events);
        let entry = plugin.manifest.as_ref().and_then(|m| m.entry(&plugin.dir).ok());
        let id = plugin.id.clone();
        let thread = std::thread::Builder::new().name(format!("plugin-wasm-{id}")).spawn(move || {
            let Some(entry) = entry else {
                return events(ProcessEvent::Failed("the plugin has no entry point".into()));
            };
            match Runner::load(&entry, events.clone()) {
                Ok(mut runner) => {
                    events(ProcessEvent::Started);
                    runner.run(commands);
                    // Nothing is left running: the interpreter stops with the thread.
                    events(ProcessEvent::Exited(Some(0)));
                }
                Err(err) => events(ProcessEvent::Failed(err)),
            }
        });
        if let Err(err) = thread {
            eprintln!("agentty: could not start wasm plugin thread: {err}");
        }
        Self { sender }
    }

    pub fn send(&self, line: String) {
        let _ = self.sender.send(Command::Message(line));
    }

    /// Asks the plugin to shut down; the thread ends once it has.
    pub fn stop(&self) {
        let _ = self.sender.send(Command::Shutdown);
    }
}

struct Runner {
    store: Store<HostState>,
    alloc: TypedFunc<i32, i32>,
    on_message: TypedFunc<(i32, i32), ()>,
    events: Arc<dyn Fn(ProcessEvent) + Send + Sync>,
}

impl Runner {
    fn load(entry: &std::path::Path, events: Arc<dyn Fn(ProcessEvent) + Send + Sync>) -> Result<Self, String> {
        let size = std::fs::metadata(entry).map_err(|e| format!("could not read the module: {e}"))?.len();
        if size > MAX_MODULE_BYTES {
            return Err(format!("the module is larger than {} MB", MAX_MODULE_BYTES / 1024 / 1024));
        }
        let bytes = std::fs::read(entry).map_err(|e| format!("could not read the module: {e}"))?;
        let mut config = wasmi::Config::default();
        // Fuel is what ends a plugin that never returns; without it a loop in the guest would hold
        // its thread for good.
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        // Validating: a module that is not well-formed is refused before anything of it runs.
        let module = Module::new(&engine, &bytes[..]).map_err(|e| format!("not a valid WebAssembly module: {e}"))?;
        let limits = StoreLimitsBuilder::new().memory_size(MAX_MEMORY_BYTES).table_elements(MAX_TABLE_ELEMENTS).instances(1).build();
        let state = HostState { events: events.clone(), memory: None, limits, sent: 0 };
        let mut store = Store::new(&engine, state);
        store.limiter(|state| &mut state.limits);
        let mut linker: Linker<HostState> = Linker::new(&engine);
        Self::define_host(&mut linker)?;
        // Whatever the module runs while it is set up is on the same budget as a message.
        store.set_fuel(FUEL_PER_MESSAGE).map_err(|e| e.to_string())?;
        let instance = linker
            .instantiate_and_start(&mut store, &module)
            // "unknown import" lands here: a module that wants anything but the three functions
            // above does not load at all.
            .map_err(|e| format!("the module could not be loaded: {e}"))?;
        let memory = instance.get_memory(&store, "memory").ok_or("the module exports no memory")?;
        store.data_mut().memory = Some(memory);
        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "agentty_alloc")
            .map_err(|_| "the module does not export agentty_alloc(len) -> ptr".to_string())?;
        let on_message = instance
            .get_typed_func::<(i32, i32), ()>(&store, "agentty_on_message")
            .map_err(|_| "the module does not export agentty_on_message(ptr, len)".to_string())?;
        Ok(Self { store, alloc, on_message, events })
    }

    /// The whole of what a plugin can call. Everything else it might want goes through `send` as
    /// a protocol message, where the permissions are checked.
    fn define_host(linker: &mut Linker<HostState>) -> Result<(), String> {
        let wrap = |result: Result<&mut Linker<HostState>, wasmi::errors::LinkerError>| result.map(|_| ()).map_err(|e| e.to_string());
        wrap(linker.func_wrap("agentty", "send", |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            let Some(memory) = caller.data().memory else { return Err(fail("no memory")) };
            let text = HostState::read(&memory, &caller, ptr, len)?;
            let state = caller.data_mut();
            state.sent += 1;
            if state.sent > MAX_MESSAGES_PER_DISPATCH {
                return Err(fail(format!("sent more than {MAX_MESSAGES_PER_DISPATCH} messages for one event")));
            }
            let events = state.events.clone();
            match Incoming::parse(&text) {
                Some(message) => events(ProcessEvent::Message(message)),
                None if text.trim().is_empty() => {}
                None => events(ProcessEvent::Log(format!("not a protocol message: {}", text.trim_end()))),
            }
            Ok(())
        }))?;
        wrap(linker.func_wrap("agentty", "log", |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            let Some(memory) = caller.data().memory else { return Err(fail("no memory")) };
            let text = HostState::read(&memory, &caller, ptr, len)?;
            let events = caller.data_mut().events.clone();
            events(ProcessEvent::Log(text.trim_end().to_string()));
            Ok(())
        }))?;
        wrap(linker.func_wrap("agentty", "now_ms", || {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
        }))?;
        Ok(())
    }

    /// Handles messages until the channel closes or the plugin traps.
    fn run(&mut self, commands: mpsc::Receiver<Command>) {
        for command in commands {
            let line = match command {
                Command::Message(line) => line,
                Command::Shutdown => agentty_bridge::plugins::notification("shutdown", serde_json::json!({})),
            };
            let shutdown = matches!(Incoming::parse(&line), Some(Incoming::Notification { ref method, .. }) if method == "shutdown");
            if let Err(err) = self.dispatch(&line) {
                (self.events)(ProcessEvent::Failed(err));
                return;
            }
            if shutdown {
                return;
            }
        }
    }

    fn dispatch(&mut self, line: &str) -> Result<(), String> {
        let bytes = line.as_bytes();
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("Agentty tried to send a message larger than the protocol allows".into());
        }
        // A fresh budget per message: what a plugin spent answering the last one is not held
        // against this one.
        self.store.set_fuel(FUEL_PER_MESSAGE).map_err(|e| e.to_string())?;
        self.store.data_mut().sent = 0;
        let len = bytes.len() as i32;
        let ptr = self.alloc.call(&mut self.store, len).map_err(|e| trap_message(&e, "agentty_alloc"))?;
        let Some(memory) = self.store.data().memory else { return Err("the module lost its memory".into()) };
        memory.write(&mut self.store, ptr as usize, bytes).map_err(|_| "agentty_alloc returned a buffer outside memory".to_string())?;
        self.on_message.call(&mut self.store, (ptr, len)).map_err(|e| trap_message(&e, "agentty_on_message"))
    }
}

/// A trap, in words the plugin's log can show. Running out of fuel is the common one and says so.
fn trap_message(error: &wasmi::Error, call: &str) -> String {
    if matches!(error.as_trap_code(), Some(TrapCode::OutOfFuel)) {
        return format!("{call} did not finish in time and the plugin was stopped");
    }
    format!("{call} failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the smallest module that speaks the ABI, in raw bytes, and runs a message through
    /// it. It echoes whatever it is given back to the host as `send`, so one round trip proves the
    /// exports, the imports, the memory writes and the parsing all line up.
    fn echo_module() -> Vec<u8> {
        // (module
        //   (import "agentty" "send" (func $send (param i32 i32)))
        //   (memory (export "memory") 1)
        //   (func (export "agentty_alloc") (param i32) (result i32) i32.const 64)
        //   (func (export "agentty_on_message") (param i32 i32)
        //     local.get 0 local.get 1 call $send))
        // Assembled by hand: the text format is not compiled into Agentty.
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        // Types: 0 = (i32,i32)->(), 1 = (i32)->(i32)
        wasm.extend([0x01, 0x0b, 0x02, 0x60, 0x02, 0x7f, 0x7f, 0x00, 0x60, 0x01, 0x7f, 0x01, 0x7f]);
        // Import "agentty" "send" : type 0
        wasm.extend([0x02, 0x10, 0x01, 0x07]);
        wasm.extend(b"agentty");
        wasm.extend([0x04]);
        wasm.extend(b"send");
        wasm.extend([0x00, 0x00]);
        // Functions: alloc (type 1), on_message (type 0)
        wasm.extend([0x03, 0x03, 0x02, 0x01, 0x00]);
        // Memory: one page
        wasm.extend([0x05, 0x03, 0x01, 0x00, 0x01]);
        // Exports: memory, agentty_alloc (func 1), agentty_on_message (func 2)
        wasm.extend([0x07, 0x2f, 0x03, 0x06]);
        wasm.extend(b"memory");
        wasm.extend([0x02, 0x00, 0x0d]);
        wasm.extend(b"agentty_alloc");
        wasm.extend([0x00, 0x01, 0x12]);
        wasm.extend(b"agentty_on_message");
        wasm.extend([0x00, 0x02]);
        // Code
        wasm.extend([0x0a, 0x10, 0x02]);
        wasm.extend([0x05, 0x00, 0x41, 0xc0, 0x00, 0x0b]); // alloc: i32.const 64
        wasm.extend([0x08, 0x00, 0x20, 0x00, 0x20, 0x01, 0x10, 0x00, 0x0b]); // on_message: send(p0, p1)
        wasm
    }

    fn run_echo(message: &str) -> Vec<ProcessEvent> {
        let dir = std::env::temp_dir().join(format!("agentty-wasm-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("echo.wasm");
        std::fs::write(&path, echo_module()).unwrap();
        let collected = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = collected.clone();
        let mut runner = Runner::load(&path, Arc::new(move |event| sink.lock().unwrap().push(event))).expect("the module loads");
        let result = runner.dispatch(message);
        let _ = std::fs::remove_dir_all(&dir);
        result.expect("the message is handled");
        let events = std::mem::take(&mut *collected.lock().unwrap());
        events
    }

    #[test]
    fn a_module_receives_a_message_and_answers() {
        let events = run_echo(r#"{"jsonrpc":"2.0","method":"ui/showPanel","params":{}}"#);
        let sent: Vec<&Incoming> = events
            .iter()
            .filter_map(|event| match event {
                ProcessEvent::Message(message) => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(sent.len(), 1, "{events:?}");
        assert!(matches!(sent[0], Incoming::Notification { method, .. } if method == "ui/showPanel"));
    }

    #[test]
    fn a_module_that_wants_more_than_the_three_host_functions_does_not_load() {
        // The same module, but importing "wasi_snapshot_preview1" "fd_write" instead of send.
        let mut wasm = echo_module();
        let at = wasm.windows(7).position(|w| w == b"agentty").expect("the import name is in the module");
        wasm.splice(at..at + 7, *b"nothere");
        let dir = std::env::temp_dir().join(format!("agentty-wasm-deny-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deny.wasm");
        std::fs::write(&path, &wasm).unwrap();
        let error = Runner::load(&path, Arc::new(|_| {})).err().expect("an unknown import is refused");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(error.contains("could not be loaded"), "{error}");
    }

    #[test]
    fn rubbish_is_not_a_module() {
        let dir = std::env::temp_dir().join(format!("agentty-wasm-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.wasm");
        std::fs::write(&path, b"not a module at all").unwrap();
        let error = Runner::load(&path, Arc::new(|_| {})).err().expect("rubbish is refused");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(error.contains("not a valid WebAssembly module"), "{error}");
    }
}
