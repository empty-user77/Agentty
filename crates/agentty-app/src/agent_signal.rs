//! Local socket that agent hooks report to (`<pane id>\t<kind>\t<json payload>` per line).
//!
//! The socket is a Unix domain socket in the user's private temp folder (`0600`): nothing reaches it
//! over a network, and no other account can open it. Within the account it is closed as well: a
//! connection is only heard when the process on the other end descends from the shell of a pane
//! Agentty started — the kernel names that process (`LOCAL_PEERPID` on macOS, `SO_PEERCRED` on
//! Linux), so it cannot be claimed — and it may only speak for that pane. A plugin, another app or a
//! script outside a pane cannot report a pane's status, post notifications or drive the in-app
//! browser.
//!
//! Windows has no Unix sockets in the standard library; there it is a loopback port, and each pane
//! gets a token of its own in its environment (`$AGENTTY_SOCKET_TOKEN`). A connection must open with
//! a pane's token and then speaks for that pane only — the same rule, with the token standing in
//! for the process tree (children of a pane inherit it, nothing else has it).
//!
//! A second Agentty launch (Windows / Linux single instance, see `instance.rs`) may hand over links
//! and folders (`open`) and nothing else: same user on Unix, the launch token on Windows.

use crate::ipc::Stream;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use gpui::Global;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::Mutex;
use std::time::Duration;

/// Shell process of every live pane → pane id.
static PANES: Mutex<Option<HashMap<u32, u64>>> = Mutex::new(None);

pub fn register_pane(shell_pid: u32, pane_id: u64) {
    if let Ok(mut panes) = PANES.lock() {
        panes.get_or_insert_with(HashMap::new).insert(shell_pid, pane_id);
    }
}

pub fn unregister_pane(shell_pid: u32) {
    if let Ok(mut panes) = PANES.lock() {
        if let Some(panes) = panes.as_mut() {
            panes.remove(&shell_pid);
        }
    }
}

/// The pane whose shell is `pid` or one of its ancestors (Unix; Windows panes use tokens).
#[cfg_attr(windows, allow(dead_code))]
fn pane_of(pid: u32, parent_of: impl Fn(u32) -> Option<u32>) -> Option<u64> {
    let panes = PANES.lock().ok()?;
    let panes = panes.as_ref()?;
    let mut pid = pid;
    // Deeper than any real process tree; ends a loop if pids were ever to form one.
    for _ in 0..64 {
        if let Some(pane) = panes.get(&pid) {
            return Some(*pane);
        }
        pid = parent_of(pid).filter(|parent| *parent > 1 && *parent != pid)?;
    }
    None
}

/// Per-pane connection tokens (Windows) → pane id.
static PANE_TOKENS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// A fresh token for pane `pane_id`'s environment (Windows); forgotten with [`unregister_pane_token`].
#[cfg_attr(unix, allow(dead_code))]
pub fn register_pane_token(pane_id: u64) -> String {
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    if let Ok(mut tokens) = PANE_TOKENS.lock() {
        tokens.get_or_insert_with(HashMap::new).insert(token.clone(), pane_id);
    }
    token
}

#[cfg_attr(unix, allow(dead_code))]
pub fn unregister_pane_token(token: &str) {
    if let Ok(mut tokens) = PANE_TOKENS.lock() {
        if let Some(tokens) = tokens.as_mut() {
            tokens.remove(token);
        }
    }
}

#[cfg_attr(unix, allow(dead_code))]
fn pane_of_token(token: &str) -> Option<u64> {
    let tokens = PANE_TOKENS.lock().ok()?;
    let tokens = tokens.as_ref()?;
    // Compared in constant time against every entry, so timing says nothing about a guess.
    tokens.iter().filter(|(known, _)| crate::ipc::constant_time_eq(known.as_bytes(), token.as_bytes())).map(|(_, pane)| *pane).next()
}

/// Who is on the other end of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    /// A process of this pane: its signals, notifications and browser requests are heard.
    Pane(u64),
    /// A second Agentty launch of this user: may only hand over links and folders (`open`).
    Launcher,
    Nobody,
}

impl Caller {
    fn pane(self) -> Option<u64> {
        match self {
            Caller::Pane(pane) => Some(pane),
            _ => None,
        }
    }
}

/// Who a connection may speak for.
#[cfg(unix)]
fn authenticate(stream: &mut Stream, launcher_token: Option<&str>) -> Caller {
    let _ = launcher_token;
    if !stream.peer_is_this_user() {
        return Caller::Nobody;
    }
    match stream.peer_pid().and_then(|pid| pane_of(pid, crate::procinfo::parent_pid)) {
        Some(pane) => Caller::Pane(pane),
        None => Caller::Launcher,
    }
}

/// Windows: the connection's first line is `auth\t<token>` — a pane's token, or the launch token.
#[cfg(not(unix))]
fn authenticate(stream: &mut Stream, launcher_token: Option<&str>) -> Caller {
    let Some(token) = stream.read_token() else { return Caller::Nobody };
    if let Some(pane) = pane_of_token(&token) {
        return Caller::Pane(pane);
    }
    match launcher_token {
        Some(expected) if crate::ipc::constant_time_eq(expected.as_bytes(), token.as_bytes()) => Caller::Launcher,
        _ => Caller::Nobody,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    /// A prompt was submitted or a tool is about to run.
    Working,
    /// The agent finished its turn.
    Stop,
    /// The agent needs the user (permission prompt, idle input request, …).
    Notification,
    /// A message sent with `agentty notify`.
    Notify,
    /// Account rate-limit usage (percent) reported by the statusline wrapper.
    Usage,
    /// A tool is waiting for the user's approval (Claude Code `PermissionRequest`).
    Permission,
    /// A subagent started / finished (Claude Code `SubagentStart` / `SubagentStop`).
    SubagentStart,
    SubagentStop,
    /// The agent session ended (the CLI is exiting).
    SessionEnd,
}

/// A message received on the Agentty socket.
#[derive(Debug, Clone)]
pub enum SocketMessage {
    Signal(AgentSignal),
    /// `browser\t{"pane":…,"command":…,"args":[…]}` from `agentty browser`; answered on `reply`.
    Browser(BrowserRequest),
    /// `worktree\t{"cwd":…,"label":…}` from `agentty worktree-for` (an agent typed into a shell);
    /// answered on `reply`.
    Worktree(WorktreeRequest),
    /// `debug\t<command>\t<argument>`; only accepted when `AGENTTY_DEBUG=1`.
    Debug(String, String),
    /// `open\t["agentty://…", "/folder", …]` from a second launch (Windows / Linux single instance).
    Open(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct AgentSignal {
    pub pane_id: u64,
    pub kind: SignalKind,
    pub message: Option<String>,
    /// Extra hook fields (tool, notification type, subagent), when the payload had them.
    pub detail: SignalDetail,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SignalDetail {
    /// `permission_prompt`, `idle_prompt`, `elicitation_dialog`, …
    pub notification_type: Option<String>,
    pub tool: Option<String>,
    /// Short description of what the tool does (command, file, task description).
    pub target: Option<String>,
    /// Subagent the event belongs to: (agent id, agent type).
    pub subagent: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct BrowserRequest {
    pub pane: Option<u64>,
    pub command: String,
    pub args: Vec<String>,
    /// One JSON line: `{"ok":true,"result":…}` or `{"ok":false,"error":"…"}`.
    pub reply: std::sync::mpsc::Sender<String>,
}

/// An agent about to start in a shell pane asks whether it should get a working tree of its own.
#[derive(Debug, Clone)]
pub struct WorktreeRequest {
    /// The pane the connection belongs to.
    pub pane: u64,
    pub cwd: std::path::PathBuf,
    /// `claude` or `codex`: folder name and branch suffix of the tree.
    pub label: String,
    /// One JSON line, as [`browser_reply`] makes it: the tree's path, or `null` to stay put.
    pub reply: std::sync::mpsc::Sender<String>,
}

pub fn browser_reply(result: Result<String, String>) -> String {
    match result {
        Ok(json) => {
            let value: serde_json::Value = serde_json::from_str(&json).unwrap_or(serde_json::Value::String(json));
            serde_json::json!({ "ok": true, "result": value }).to_string()
        }
        Err(error) => serde_json::json!({ "ok": false, "error": error }).to_string(),
    }
}

pub struct SignalSocket {
    /// `$AGENTTY_SOCKET` for panes (a socket path, or `127.0.0.1:<port>` on Windows).
    pub address: String,
    /// Windows: the token of a second Agentty launch (single instance), which may only send `open`.
    pub token: Option<String>,
}

impl Global for SignalSocket {}

impl Drop for SignalSocket {
    fn drop(&mut self) {
        crate::ipc::Listener::cleanup(&self.address);
    }
}

pub fn parse_line(line: &str) -> Option<AgentSignal> {
    let mut parts = line.splitn(3, '\t');
    let pane_id = parts.next()?.trim().parse().ok()?;
    let kind = match parts.next()?.trim() {
        "working" => SignalKind::Working,
        "stop" => SignalKind::Stop,
        "notification" => SignalKind::Notification,
        "notify" => SignalKind::Notify,
        "usage" => SignalKind::Usage,
        "permission" => SignalKind::Permission,
        "subagent_start" => SignalKind::SubagentStart,
        "subagent_stop" => SignalKind::SubagentStop,
        "session_end" => SignalKind::SessionEnd,
        _ => return None,
    };
    let raw = parts.next().unwrap_or("").trim();
    let plain = |message: Option<String>| AgentSignal { pane_id, kind, message, detail: SignalDetail::default() };
    if kind == SignalKind::Usage {
        let percent: f64 = raw.parse().ok().filter(|p: &f64| p.is_finite())?;
        return Some(plain(Some(format!("{}", percent.clamp(0., 100.)))));
    }
    if kind == SignalKind::Notify {
        let message: String = raw.chars().take(300).collect();
        return Some(plain((!message.is_empty()).then_some(message)));
    }
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(raw) else { return Some(plain(None)) };
    let first_line = |text: &str| text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(160).collect::<String>();
    let text = |key: &str| payload[key].as_str().map(str::to_string).filter(|s| !s.is_empty());
    let message = ["message", "last-assistant-message", "last_assistant_message"].iter().find_map(|k| payload[*k].as_str()).map(first_line);
    let input = &payload["tool_input"];
    let target = ["description", "command", "file_path", "pattern", "url", "prompt"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .map(first_line)
        .filter(|s| !s.is_empty());
    let subagent = text("agent_id").map(|id| (id, text("agent_type").unwrap_or_default()));
    let detail = SignalDetail { notification_type: text("notification_type"), tool: text("tool_name"), target, subagent };
    Some(AgentSignal { pane_id, kind, message, detail })
}

/// Binds the socket and starts the accept thread.
pub fn start() -> anyhow::Result<(SignalSocket, UnboundedReceiver<SocketMessage>)> {
    start_with(crate::ipc::Listener::bind()?)
}

fn start_with(listener: crate::ipc::Listener) -> anyhow::Result<(SignalSocket, UnboundedReceiver<SocketMessage>)> {
    let socket = SignalSocket { address: listener.address.clone(), token: listener.token.clone() };
    let launcher_token = listener.token.clone();
    let (tx, rx) = unbounded();
    let debug = crate::debug::enabled();

    std::thread::Builder::new().name("agentty-signals".into()).spawn(move || {
        for mut stream in listener.incoming() {
            // Looked at right away on Unix: a hook's `nc` is gone a moment after it has written its
            // line. (Windows reads the token line in the connection's own thread, so a slow client
            // can't hold up the others.)
            let tx = tx.clone();
            let launcher_token = launcher_token.clone();
            #[cfg(unix)]
            let caller = authenticate(&mut stream, None);
            // The debug driver (`AGENTTY_DEBUG=1`, development only) connects from outside a pane.
            #[cfg(unix)]
            if caller == Caller::Nobody && !debug {
                continue;
            }
            // One thread per connection: a browser request waits for its answer, and signals from
            // other panes must not wait with it.
            let _ = std::thread::Builder::new().name("agentty-signal".into()).spawn(move || {
                #[cfg(not(unix))]
                let caller = authenticate(&mut stream, launcher_token.as_deref());
                #[cfg(unix)]
                let _ = &launcher_token;
                if caller == Caller::Nobody && !debug {
                    return;
                }
                serve(stream, caller, debug, tx)
            });
        }
    })?;
    Ok((socket, rx))
}

/// Reads one connection. `caller`: who it may speak for. Only a pane is heard for signals and
/// browser requests; a second Agentty launch only for `open` (Windows / Linux); anyone else only
/// with the debug driver on, and then only its `debug` lines.
fn serve(stream: Stream, caller: Caller, debug: bool, tx: UnboundedSender<SocketMessage>) {
    let pane = caller.pane();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut writer = stream.try_clone().ok();
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        if let Some(json) = line.strip_prefix("browser\t").filter(|_| pane.is_some()) {
            // Request / response: the CLI waits for one line back.
            let request: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
            let (reply, answer) = std::sync::mpsc::channel();
            let message = SocketMessage::Browser(BrowserRequest {
                // The pane the connection belongs to, whatever the request says.
                pane,
                command: request["command"].as_str().unwrap_or_default().to_string(),
                args: request["args"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
                reply,
            });
            if tx.unbounded_send(message).is_err() {
                return;
            }
            let response = answer.recv_timeout(Duration::from_secs(120)).unwrap_or_else(|_| browser_reply(Err("timed out".into())));
            if let Some(writer) = writer.as_mut() {
                use std::io::Write;
                let _ = writeln!(writer, "{response}");
            }
            continue;
        }
        if let (Some(json), Some(pane)) = (line.strip_prefix("worktree\t"), pane) {
            let request: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
            let cwd = std::path::PathBuf::from(request["cwd"].as_str().unwrap_or_default());
            let label = request["label"].as_str().unwrap_or_default();
            let response = if cwd.is_absolute() && is_agent_label(label) {
                let (reply, answer) = std::sync::mpsc::channel();
                let message = SocketMessage::Worktree(WorktreeRequest { pane, cwd, label: label.to_string(), reply });
                if tx.unbounded_send(message).is_err() {
                    return;
                }
                // Creating a tree is a `git worktree add`: seconds at most.
                answer.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|_| browser_reply(Err("timed out".into())))
            } else {
                browser_reply(Err("bad request".into()))
            };
            if let Some(writer) = writer.as_mut() {
                use std::io::Write;
                let _ = writeln!(writer, "{response}");
            }
            continue;
        }
        // Single-instance hand-over (Windows / Linux only; macOS gets open events).
        if let Some(json) = line.strip_prefix("open\t").filter(|_| !cfg!(target_os = "macos") && caller != Caller::Nobody) {
            let arguments: Vec<String> = serde_json::from_str(json).unwrap_or_default();
            if tx.unbounded_send(SocketMessage::Open(arguments.into_iter().take(32).collect())).is_err() {
                return;
            }
            continue;
        }
        let message = match line.strip_prefix("debug\t") {
            Some(rest) if debug => {
                let (command, argument) = rest.split_once('\t').unwrap_or((rest, ""));
                Some(SocketMessage::Debug(command.to_string(), argument.to_string()))
            }
            Some(_) => None,
            // A pane reports for itself only.
            None => parse_line(&line).filter(|signal| pane == Some(signal.pane_id)).map(SocketMessage::Signal),
        };
        if let Some(message) = message {
            if tx.unbounded_send(message).is_err() {
                return;
            }
        }
    }
}

/// The agents a shell pane asks a working tree for (`label` of [`WorktreeRequest`]).
pub fn is_agent_label(label: &str) -> bool {
    matches!(label, "claude" | "codex")
}

/// `agentty worktree-for <claude|codex>`: run by the shell wrappers right before an agent starts
/// in a pane. Prints the working tree to start in when another agent already works in this folder
/// (Agentty created it), nothing otherwise. Always exits 0 and stays quiet on errors, so the agent
/// starts either way.
pub fn worktree_for(args: &[String]) -> i32 {
    use std::io::Write;
    let (Some(label), Ok(socket), Ok(cwd)) = (args.first(), std::env::var("AGENTTY_SOCKET"), std::env::current_dir()) else {
        return 0;
    };
    if !is_agent_label(label) {
        return 0;
    }
    let Ok(mut stream) = crate::ipc::connect(&socket) else { return 0 };
    let request = serde_json::json!({ "cwd": cwd, "label": label });
    if writeln!(stream, "worktree\t{request}").is_err() {
        return 0;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(40)));
    let mut line = String::new();
    let _ = BufReader::new(stream).read_line(&mut line);
    let reply: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
    if let Some(path) = reply["result"]["path"].as_str() {
        if let Some(message) = reply["result"]["message"].as_str() {
            eprintln!("{message}");
        }
        println!("{path}");
    }
    0
}

/// `agentty notify <message>`: posts a notification for the pane this command runs in.
pub fn send_notify(message: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let socket = std::env::var("AGENTTY_SOCKET").map_err(|_| anyhow::anyhow!("not running inside a Agentty terminal"))?;
    let pane = std::env::var("AGENTTY_PANE_ID").map_err(|_| anyhow::anyhow!("not running inside a Agentty terminal"))?;
    let text: String = message.chars().filter(|c| *c != '\n' && *c != '\t').collect();
    let mut stream = crate::ipc::connect(&socket)?;
    writeln!(stream, "{pane}\tnotify\t{text}")?;
    linger(stream);
    Ok(())
}

/// Stays connected until Agentty has read the line: it checks who is on the other end when it
/// accepts the connection, and a process that has already exited is nobody.
pub fn linger(mut stream: Stream) {
    use std::io::Read;
    stream.shutdown_write();
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.read(&mut [0u8; 1]);
}

/// `agentty signal <kind> [payload]`: forwards one agent hook event to the pane's Agentty. The
/// payload is the argument (Codex `notify`) or stdin (Claude Code hooks), newlines removed.
/// Used on platforms without `nc -U`; always exits 0 so a closed Agentty never breaks the agent.
pub fn forward_signal(args: &[String]) -> i32 {
    use std::io::{Read, Write};
    let (Some(kind), Ok(socket), Ok(pane)) = (args.first(), std::env::var("AGENTTY_SOCKET"), std::env::var("AGENTTY_PANE_ID")) else {
        return 0;
    };
    let payload = match args.get(1) {
        Some(argument) => argument.clone(),
        None => {
            let mut input = Vec::new();
            let _ = std::io::stdin().take(1024 * 1024).read_to_end(&mut input);
            String::from_utf8_lossy(&input).into_owned()
        }
    };
    let payload: String = payload.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    if let Ok(mut stream) = crate::ipc::connect(&socket) {
        let _ = writeln!(stream, "{pane}\t{kind}\t{payload}");
        linger(stream);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests run in one process, in parallel: each gets a socket of its own.
    #[cfg(unix)]
    fn start_at(name: &str) -> (SignalSocket, UnboundedReceiver<SocketMessage>) {
        let path = std::env::temp_dir().join(format!("agentty-test-{}-{name}.sock", std::process::id()));
        start_with(crate::ipc::Listener::bind_path(path).unwrap()).unwrap()
    }

    #[test]
    fn parses_claude_notification() {
        let s = parse_line("7\tnotification\t{\"message\":\"Claude is waiting for your input\"}").unwrap();
        assert_eq!(s.pane_id, 7);
        assert_eq!(s.kind, SignalKind::Notification);
        assert_eq!(s.message.as_deref(), Some("Claude is waiting for your input"));
    }

    #[test]
    fn parses_hook_details() {
        let s = parse_line(&format!("2\tpermission\t{}", r#"{"tool_name":"Bash","tool_input":{"command":"rm -rf target\nls"}}"#)).unwrap();
        assert_eq!(s.kind, SignalKind::Permission);
        assert_eq!((s.detail.tool.as_deref(), s.detail.target.as_deref()), (Some("Bash"), Some("rm -rf target")));
        let s = parse_line(&format!(
            "2\tnotification\t{}",
            r#"{"message":"Claude is waiting for your input","notification_type":"idle_prompt"}"#
        ))
        .unwrap();
        assert_eq!(s.detail.notification_type.as_deref(), Some("idle_prompt"));
        let s = parse_line(&format!(
            "2\tsubagent_stop\t{}",
            r#"{"agent_id":"a1","agent_type":"Explore","last_assistant_message":"\nFound 3 call sites.\nDetails follow."}"#
        ))
        .unwrap();
        assert_eq!(s.detail.subagent, Some(("a1".into(), "Explore".into())));
        assert_eq!(s.message.as_deref(), Some("Found 3 call sites."));
        let s =
            parse_line(&format!("2\tworking\t{}", r#"{"tool_name":"Agent","tool_input":{"description":"Find call sites","prompt":"…"}}"#))
                .unwrap();
        assert_eq!((s.detail.tool.as_deref(), s.detail.target.as_deref()), (Some("Agent"), Some("Find call sites")));
    }

    #[test]
    fn parses_usage_percent() {
        let s = parse_line("3\tusage\t36.5").unwrap();
        assert_eq!((s.kind, s.message.as_deref()), (SignalKind::Usage, Some("36.5")));
        assert!(parse_line("3\tusage\tlots").is_none());
    }

    #[test]
    fn parses_notify_text() {
        let s = parse_line("5\tnotify\tBuild finished: 0 errors").unwrap();
        assert_eq!(s.kind, SignalKind::Notify);
        assert_eq!(s.message.as_deref(), Some("Build finished: 0 errors"));
    }

    #[test]
    fn parses_bare_signal_and_rejects_garbage() {
        let s = parse_line("3\tworking\t").unwrap();
        assert_eq!(s.kind, SignalKind::Working);
        assert!(s.message.is_none());
        assert!(parse_line("x\tstop\t").is_none());
        assert!(parse_line("1\tunknown\t").is_none());
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn a_connection_speaks_only_for_the_pane_it_runs_in() {
        use std::io::Write;
        let (socket, mut rx) = start_at("pane");
        // This test process stands in for a pane's shell: what connects from it is pane 42.
        register_pane(std::process::id(), 42);
        let mut client = crate::ipc::connect(&socket.address).unwrap();
        // A line for another pane is dropped, the pane's own is heard.
        client.write_all(b"43\tstop\t{}\n42\tstop\t{}\n").unwrap();
        linger(client);
        let message = futures::executor::block_on(futures::StreamExt::next(&mut rx)).unwrap();
        assert!(matches!(message, SocketMessage::Signal(s) if s.pane_id == 42));

        // Once the pane is gone, nothing from this process is heard any more.
        unregister_pane(std::process::id());
        let mut stranger = crate::ipc::connect(&socket.address).unwrap();
        stranger.write_all(b"42\tstop\t{}\n").unwrap();
        linger(stranger);
        assert!(rx.try_recv().is_err(), "a process outside every pane was heard");
    }

    /// The real path of a hook: the pane's shell starts `sh`, which pipes into `nc` — a grandchild
    /// that Agentty has to trace back to the pane through the kernel's process table.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_hook_is_traced_back_to_its_pane() {
        let (socket, mut rx) = start_at("hook");
        let script = "sleep 0.3; printf '9\\tstop\\t{}\\n' | nc -U -w 1 \"$0\"";
        let mut shell = std::process::Command::new("sh").args(["-c", script]).arg(&socket.address).spawn().unwrap();
        register_pane(shell.id(), 9);
        let message = futures::executor::block_on(futures::StreamExt::next(&mut rx)).unwrap();
        assert!(matches!(message, SocketMessage::Signal(s) if s.pane_id == 9));
        let _ = shell.wait();
        unregister_pane(shell.id());

        // The same command from a shell that is not a pane is not heard.
        let mut outsider = std::process::Command::new("sh").args(["-c", script]).arg(&socket.address).spawn().unwrap();
        let _ = outsider.wait();
        std::thread::sleep(Duration::from_millis(200));
        assert!(rx.try_recv().is_err(), "a shell outside every pane was heard");
    }

    #[test]
    fn the_pane_is_found_through_the_process_tree() {
        // shell 900001 (pane 7) → claude 900002 → hook `sh` 900003 → `nc` 900004
        register_pane(900_001, 7);
        let parent_of = |pid: u32| match pid {
            900_004 => Some(900_003),
            900_003 => Some(900_002),
            900_002 => Some(900_001),
            900_001 => Some(1),
            // A plugin (900010) is a child of Agentty (900009), not of any pane.
            900_010 => Some(900_009),
            900_009 => Some(1),
            _ => None,
        };
        assert_eq!(pane_of(900_004, parent_of), Some(7));
        assert_eq!(pane_of(900_001, parent_of), Some(7));
        assert_eq!(pane_of(900_010, parent_of), None);
        // A process that has exited has no parent to follow.
        assert_eq!(pane_of(900_099, parent_of), None);
        unregister_pane(900_001);
        assert_eq!(pane_of(900_004, parent_of), None);
    }
}
