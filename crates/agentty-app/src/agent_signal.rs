//! Local socket that agent hooks report to (`<pane id>\t<kind>\t<json payload>` per line).

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use gpui::Global;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::Duration;

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
    /// `debug\t<command>\t<argument>`; only accepted when `AGENTTY_DEBUG=1`.
    Debug(String, String),
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
    pub path: PathBuf,
}

impl Global for SignalSocket {}

impl Drop for SignalSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
    // The per-user temp dir is private (0700) and short enough for SUN_LEN, unlike deep data dirs.
    let path = std::env::temp_dir().join(format!("agentty-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    let (tx, rx) = unbounded();
    let debug = crate::debug::enabled();

    std::thread::Builder::new().name("agentty-signals".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut writer = stream.try_clone().ok();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if let Some(json) = line.strip_prefix("browser\t") {
                    // Request / response: the CLI waits for one line back.
                    let request: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
                    let (reply, answer) = std::sync::mpsc::channel();
                    let message = SocketMessage::Browser(BrowserRequest {
                        pane: request["pane"].as_u64(),
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
                let message = match line.strip_prefix("debug\t") {
                    Some(rest) if debug => {
                        let (command, argument) = rest.split_once('\t').unwrap_or((rest, ""));
                        Some(SocketMessage::Debug(command.to_string(), argument.to_string()))
                    }
                    Some(_) => None,
                    None => parse_line(&line).map(SocketMessage::Signal),
                };
                if let Some(message) = message {
                    if tx.unbounded_send(message).is_err() {
                        return;
                    }
                }
            }
        }
    })?;
    Ok((SignalSocket { path }, rx))
}

/// `agentty notify <message>`: posts a notification for the pane this command runs in.
pub fn send_notify(message: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let socket = std::env::var("AGENTTY_SOCKET").map_err(|_| anyhow::anyhow!("not running inside a Agentty terminal"))?;
    let pane = std::env::var("AGENTTY_PANE_ID").map_err(|_| anyhow::anyhow!("not running inside a Agentty terminal"))?;
    let text: String = message.chars().filter(|c| *c != '\n' && *c != '\t').collect();
    let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
    writeln!(stream, "{pane}\tnotify\t{text}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let s = parse_line(&format!("2\tsubagent_stop\t{}", r#"{"agent_id":"a1","agent_type":"Explore"}"#)).unwrap();
        assert_eq!(s.detail.subagent, Some(("a1".into(), "Explore".into())));
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
    fn socket_roundtrip() {
        use std::io::Write;
        let (socket, mut rx) = start().unwrap();
        let mut client = std::os::unix::net::UnixStream::connect(&socket.path).unwrap();
        client.write_all(b"42\tstop\t{}\n").unwrap();
        drop(client);
        let message = futures::executor::block_on(futures::StreamExt::next(&mut rx)).unwrap();
        assert!(matches!(message, SocketMessage::Signal(s) if s.pane_id == 42));
    }
}
