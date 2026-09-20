//! Write an Agentty plugin in Rust and ship it as one `.wasm` file that runs on macOS, Windows
//! and Linux.
//!
//! The module Agentty loads reaches nothing but the three functions Agentty hands it, so a plugin
//! cannot read files, open sockets, look at environment variables or start processes. Everything
//! it wants — a panel, a notification, an HTTP request, a prompt — it asks Agentty for, and
//! Agentty checks the permissions in `agentty-plugin.json` first.
//!
//! ```ignore
//! use agentty_plugin::{export_plugin, ui, Host, Plugin, UiEvent};
//!
//! #[derive(Default)]
//! struct Hello {
//!     clicks: u32,
//! }
//!
//! impl Plugin for Hello {
//!     fn panel_open(&mut self, host: &Host) {
//!         host.set_panel(ui::column(vec![
//!             ui::text(format!("Clicked {} times", self.clicks)),
//!             ui::button("go", "Click me"),
//!         ]));
//!     }
//!
//!     fn ui_event(&mut self, host: &Host, event: UiEvent) {
//!         if event.element == "go" {
//!             self.clicks += 1;
//!             self.panel_open(host);
//!         }
//!     }
//! }
//!
//! export_plugin!(Hello);
//! ```

use serde::Deserialize;
use serde_json::{json, Value};
use std::cell::RefCell;

pub mod ui;

pub use serde_json;

// Functions Agentty gives the module. There are no others: this list is the whole of what a
// plugin can reach outside its own memory.
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "agentty")]
extern "C" {
    fn send(ptr: i32, len: i32);
    fn log(ptr: i32, len: i32);
    fn now_ms() -> i64;
}

// Building for the host (tests, `cargo check`) instead of wasm: the imports do nothing.
#[cfg(not(target_arch = "wasm32"))]
mod host_stubs {
    pub unsafe fn send(_ptr: i32, _len: i32) {}
    pub unsafe fn log(_ptr: i32, _len: i32) {}
    pub unsafe fn now_ms() -> i64 {
        0
    }
}
#[cfg(not(target_arch = "wasm32"))]
use host_stubs::{log, now_ms, send};

/// What a plugin answers. Every method has a default, so a plugin implements only what it uses.
pub trait Plugin: 'static {
    /// Agentty started the plugin. `info` holds the plugin's id, folder, version and the
    /// language the user reads.
    fn init(&mut self, host: &Host, info: &Value) {
        let _ = (host, info);
    }
    /// A command from the palette or a pane-bar button.
    fn command(&mut self, host: &Host, command: &str) {
        let _ = (host, command);
    }
    /// The panel became visible: draw it with [`Host::set_panel`].
    fn panel_open(&mut self, host: &Host) {
        let _ = host;
    }
    fn panel_close(&mut self, host: &Host) {
        let _ = host;
    }
    /// Someone used the panel.
    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        let _ = (host, event);
    }
    /// An `agentty://plugin/<id>/<path>` link was opened. Treat what it carries as text from a
    /// stranger: any web page can open one.
    fn link(&mut self, host: &Host, path: &str, query: &Value) {
        let _ = (host, path, query);
    }
    /// An answer to something the plugin asked Agentty for (`Host::fetch`, `Host::call`).
    fn answer(&mut self, host: &Host, id: u64, result: Result<Value, String>) {
        let _ = (host, id, result);
    }
    /// Where the user is now (focused pane, its folder and status), as far as the plugin's
    /// permissions allow.
    fn context(&mut self, host: &Host, context: &Value) {
        let _ = (host, context);
    }
    /// Agentty is closing the plugin.
    fn shutdown(&mut self, host: &Host) {
        let _ = host;
    }
}

/// Agentty, as the plugin can ask things of it.
pub struct Host {
    next_id: RefCell<u64>,
    context: RefCell<Value>,
}

impl Host {
    fn new() -> Self {
        Self { next_id: RefCell::new(1), context: RefCell::new(Value::Null) }
    }

    /// Milliseconds since the Unix epoch.
    pub fn now_ms(&self) -> i64 {
        // SAFETY: an import Agentty defines; it takes and returns plain numbers.
        unsafe { now_ms() }
    }

    /// A line for this plugin's log in the Plugins page.
    pub fn log(&self, line: impl AsRef<str>) {
        let line = line.as_ref();
        // SAFETY: the pointer and length describe `line`, which outlives the call.
        unsafe { log(line.as_ptr() as i32, line.len() as i32) }
    }

    /// The last context Agentty sent (workspace, pane, language).
    pub fn context(&self) -> Value {
        self.context.borrow().clone()
    }

    /// Replaces the panel with this UI tree.
    pub fn set_panel(&self, tree: Value) {
        self.notify("ui/setPanel", json!({ "tree": tree }));
    }

    /// Brings the panel on screen.
    pub fn show_panel(&self) {
        self.notify("ui/showPanel", json!({}));
    }

    /// A short message in the corner of the window. `kind` is `info`, `success`, `warning` or
    /// `error`.
    pub fn notify_user(&self, kind: &str, message: impl Into<String>) {
        self.notify("ui/notify", json!({ "kind": kind, "message": message.into() }));
    }

    /// Up to 8 characters next to the plugin's icon.
    pub fn set_badge(&self, text: impl Into<String>) {
        self.notify("ui/setBadge", json!({ "text": text.into() }));
    }

    /// Opens a page in the user's browser.
    pub fn open_url(&self, url: impl Into<String>) {
        self.notify("host/openUrl", json!({ "url": url.into() }));
    }

    /// An HTTP request, which needs the `net.request` permission. The answer arrives in
    /// [`Plugin::answer`] under the id this returns.
    pub fn fetch(&self, request: FetchRequest) -> u64 {
        self.call("net/fetch", serde_json::to_value(request).unwrap_or(Value::Null))
    }

    /// Sends a prompt to an agent (needs `prompt.inject`). Without a `target` the user picks
    /// where it goes.
    pub fn prompt(&self, text: impl Into<String>, target: Option<&str>) -> u64 {
        let mut params = json!({ "text": text.into() });
        if let Some(target) = target {
            params["target"] = json!(target);
        }
        self.call("prompt/inject", params)
    }

    /// Any method of the protocol, answered in [`Plugin::answer`].
    pub fn call(&self, method: &str, params: Value) -> u64 {
        let mut next = self.next_id.borrow_mut();
        let id = *next;
        *next += 1;
        drop(next);
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    /// Any method of the protocol, with no answer.
    pub fn notify(&self, method: &str, params: Value) {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn write(&self, message: &Value) {
        let text = message.to_string();
        // SAFETY: the pointer and length describe `text`, which outlives the call.
        unsafe { send(text.as_ptr() as i32, text.len() as i32) }
    }

    fn reply(&self, id: &Value, result: Value) {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }
}

/// Something the user did in the panel.
#[derive(Debug, Clone, Deserialize)]
pub struct UiEvent {
    /// The `id` of the element.
    #[serde(default)]
    pub element: String,
    /// `click`, `change`, `submit`, `select` or `action`.
    #[serde(default)]
    pub event: String,
    /// What was typed or picked.
    #[serde(default)]
    pub value: Option<Value>,
    /// The list item, when the event came from a list.
    #[serde(default)]
    pub item: Option<String>,
    /// The row button, when one was pressed.
    #[serde(default)]
    pub action: Option<String>,
}

impl UiEvent {
    /// The typed or picked value as text.
    pub fn text(&self) -> String {
        match &self.value {
            Some(Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        }
    }

    pub fn is_on(&self) -> bool {
        self.value.as_ref().and_then(Value::as_bool).unwrap_or(false)
    }
}

/// An HTTP request for [`Host::fetch`].
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl FetchRequest {
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self { url: url.into(), method: method.into(), ..Default::default() }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }
}

/// The plugin, its host and the buffer Agentty writes messages into. A module has one thread and
/// one instance, so a plain thread-local holds all three.
pub struct Runner {
    plugin: Box<dyn Plugin>,
    host: Host,
}

thread_local! {
    static RUNNER: RefCell<Option<Runner>> = const { RefCell::new(None) };
    static INCOMING: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Installs the plugin. [`export_plugin!`] calls this from the module's exports.
pub fn set_plugin(plugin: impl Plugin) {
    RUNNER.with(|slot| {
        *slot.borrow_mut() = Some(Runner { plugin: Box::new(plugin), host: Host::new() });
    });
}

/// The buffer Agentty writes the next message into. Exported by [`export_plugin!`].
pub fn alloc(len: i32) -> i32 {
    INCOMING.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        buffer.clear();
        buffer.resize(len.max(0) as usize, 0);
        buffer.as_ptr() as i32
    })
}

/// Handles the message in that buffer. Exported by [`export_plugin!`].
pub fn on_message(ptr: i32, len: i32) {
    let text = INCOMING.with(|buffer| {
        let buffer = buffer.borrow();
        // Agentty writes into the buffer `alloc` returned; anything else is not a message.
        if buffer.as_ptr() as i32 != ptr || buffer.len() != len.max(0) as usize {
            return String::new();
        }
        String::from_utf8_lossy(&buffer).into_owned()
    });
    if text.is_empty() {
        return;
    }
    RUNNER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(runner) = slot.as_mut() else { return };
        runner.handle(&text);
    });
}

impl Runner {
    fn handle(&mut self, text: &str) {
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            self.host.log(format!("could not read a message: {text}"));
            return;
        };
        let id = message.get("id").cloned().filter(|id| !id.is_null());
        let method = message.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if method.is_empty() {
            // An answer to something the plugin asked for.
            let (Some(id), Some(number)) = (id.clone(), id.as_ref().and_then(Value::as_u64)) else { return };
            let _ = id;
            let result = match message.get("error") {
                Some(error) => Err(error.get("message").and_then(Value::as_str).unwrap_or("error").to_string()),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            self.plugin.answer(&self.host, number, result);
            return;
        }
        if let Some(context) = params.get("context").filter(|context| !context.is_null()) {
            *self.host.context.borrow_mut() = context.clone();
        }
        match method.as_str() {
            "initialize" => {
                // Answered first: Agentty waits for it before anything else counts as started.
                if let Some(id) = &id {
                    self.host.reply(id, json!({}));
                }
                let info = params.get("plugin").cloned().unwrap_or(Value::Null);
                self.plugin.init(&self.host, &info);
            }
            "command/execute" => {
                let command = params.get("command").and_then(Value::as_str).unwrap_or("").to_string();
                self.plugin.command(&self.host, &command);
            }
            "panel/open" => self.plugin.panel_open(&self.host),
            "panel/close" => self.plugin.panel_close(&self.host),
            "ui/event" => {
                if let Ok(event) = serde_json::from_value::<UiEvent>(params.clone()) {
                    self.plugin.ui_event(&self.host, event);
                }
            }
            "url/open" => {
                let path = params.get("path").and_then(Value::as_str).unwrap_or("").to_string();
                let query = params.get("query").cloned().unwrap_or(Value::Null);
                self.plugin.link(&self.host, &path, &query);
            }
            "context/changed" => {
                let context = self.host.context();
                self.plugin.context(&self.host, &context);
            }
            "shutdown" => self.plugin.shutdown(&self.host),
            other => self.host.log(format!("ignored {other}")),
        }
        if let Some(id) = id.filter(|_| method != "initialize") {
            self.host.reply(&id, Value::Null);
        }
    }
}

/// Exports the two functions Agentty calls, and installs `$plugin` (which must implement
/// [`Plugin`] and [`Default`]) the first time one of them runs.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        #[no_mangle]
        pub extern "C" fn agentty_alloc(len: i32) -> i32 {
            $crate::ready::<$plugin>();
            $crate::alloc(len)
        }

        #[no_mangle]
        pub extern "C" fn agentty_on_message(ptr: i32, len: i32) {
            $crate::ready::<$plugin>();
            $crate::on_message(ptr, len);
        }
    };
}

/// Installs the plugin once. Called by [`export_plugin!`]; there is no `start` section to do it
/// in, and a module is set up the first time Agentty speaks to it.
pub fn ready<P: Plugin + Default>() {
    RUNNER.with(|slot| {
        if slot.borrow().is_none() {
            *slot.borrow_mut() = Some(Runner { plugin: Box::new(P::default()), host: Host::new() });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn seen() -> Vec<String> {
        SEEN.with(|seen| std::mem::take(&mut *seen.borrow_mut()))
    }

    #[derive(Default)]
    struct Recorder;

    impl Recorder {
        fn note(line: String) {
            SEEN.with(|seen| seen.borrow_mut().push(line));
        }
    }

    impl Plugin for Recorder {
        fn init(&mut self, _: &Host, info: &Value) {
            Self::note(format!("init {}", info.get("id").and_then(Value::as_str).unwrap_or("?")));
        }
        fn command(&mut self, _: &Host, command: &str) {
            Self::note(format!("command {command}"));
        }
        fn ui_event(&mut self, _: &Host, event: UiEvent) {
            Self::note(format!("event {} {} {}", event.element, event.event, event.text()));
        }
        fn answer(&mut self, _: &Host, id: u64, result: Result<Value, String>) {
            Self::note(format!("answer {id} {}", if result.is_ok() { "ok" } else { "failed" }));
        }
        fn link(&mut self, _: &Host, path: &str, _: &Value) {
            Self::note(format!("link {path}"));
        }
    }

    fn runner() -> Runner {
        Runner { plugin: Box::new(Recorder), host: Host::new() }
    }

    #[test]
    fn routes_messages_to_the_plugin() {
        let mut runner = runner();
        let _ = seen();
        runner.handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"plugin":{"id":"hello"},"context":{"language":"ko"}}}"#);
        runner.handle(r#"{"jsonrpc":"2.0","method":"command/execute","params":{"command":"hello.say"}}"#);
        runner.handle(r#"{"jsonrpc":"2.0","method":"ui/event","params":{"element":"go","event":"change","value":"typed"}}"#);
        runner.handle(r#"{"jsonrpc":"2.0","method":"url/open","params":{"path":"open","query":{}}}"#);
        runner.handle(r#"{"jsonrpc":"2.0","id":7,"result":{"status":200}}"#);
        runner.handle(r#"{"jsonrpc":"2.0","id":8,"error":{"code":-32602,"message":"no"}}"#);
        assert_eq!(
            seen(),
            ["init hello", "command hello.say", "event go change typed", "link open", "answer 7 ok", "answer 8 failed"]
        );
        // The context that came with `initialize` is kept for the plugin to read.
        assert_eq!(runner.host.context()["language"], "ko");
    }

    #[test]
    fn a_message_that_is_not_json_is_logged_rather_than_a_panic() {
        let mut runner = runner();
        let _ = seen();
        runner.handle("{ not json");
        runner.handle("");
        runner.handle("[]");
        assert!(seen().is_empty());
    }

    #[test]
    fn ids_count_up_per_call() {
        let host = Host::new();
        assert_eq!(host.call("net/fetch", Value::Null), 1);
        assert_eq!(host.call("net/fetch", Value::Null), 2);
        assert_eq!(host.fetch(FetchRequest::new("GET", "https://example.com")), 3);
    }

    #[test]
    fn events_read_their_value() {
        let event: UiEvent = serde_json::from_value(json!({ "element": "a", "event": "change", "value": true })).unwrap();
        assert!(event.is_on());
        let event: UiEvent = serde_json::from_value(json!({ "element": "a", "event": "submit", "value": "x" })).unwrap();
        assert_eq!(event.text(), "x");
        assert!(!event.is_on());
    }

    #[test]
    fn a_fetch_request_is_shaped_like_the_protocol() {
        let request = FetchRequest::new("POST", "https://example.com/api").header("Accept", "application/json").body("{}");
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["method"], "POST");
        assert_eq!(value["headers"]["Accept"], "application/json");
        assert_eq!(value["body"], "{}");
        assert!(value.get("timeoutMs").is_none(), "what is not set is left out");
    }
}
