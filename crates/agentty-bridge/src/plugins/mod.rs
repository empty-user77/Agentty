//! Agentty plugins: separate processes that talk JSON-RPC 2.0 (one JSON object per line) over
//! stdio, add commands, pane-bar buttons and a UI panel, and can read sessions and send prompts.
//!
//! The protocol is documented in `docs/plugins/protocol.md`; the Node.js SDK lives in `sdk/node`.

pub mod link;
pub mod manifest;
pub mod market;
pub mod net;
pub mod storage;
pub mod store;
pub mod ui;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Methods a plugin may call on Agentty, with the permission each needs (`None`: always allowed).
pub const HOST_METHODS: &[(&str, Option<&str>)] = &[
    ("ui/setPanel", None),
    ("ui/showPanel", None),
    ("ui/notify", None),
    ("ui/setBadge", None),
    ("host/info", None),
    ("host/openUrl", None),
    // A module has no clock and no loop of its own: this is how it waits.
    ("host/timer", None),
    // Puts text on the clipboard: what a plugin's "copy this" button does.
    ("host/copy", None),
    // Reveals a file in Finder, and tells the plugin whether a path exists: that is the user's
    // folders, so it needs the same permission as reading them.
    ("host/revealPath", Some("workspace.read")),
    ("context/get", None),
    // The plugin's own folder under `plugin-data`, and nothing else: no permission to ask for.
    ("storage/get", None),
    ("storage/set", None),
    ("storage/keys", None),
    // The plugin's own HTTP requests. Nothing of Agentty's travels with them: no cookies, no
    // stored credentials, only what the plugin puts in the request.
    ("net/fetch", Some("net.request")),
    ("prompt/inject", Some("prompt.inject")),
    ("terminal/send", Some("terminal.write")),
    ("session/get", Some("session.read")),
    ("workspace/list", Some("workspace.read")),
];

/// `None` for unknown methods; `Some(None)` when no permission is needed.
pub fn required_permission(method: &str) -> Option<Option<&'static str>> {
    HOST_METHODS.iter().find(|(name, _)| *name == method).map(|(_, permission)| *permission)
}

/// JSON-RPC error codes used by Agentty.
pub mod codes {
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const PERMISSION_DENIED: i64 = -32001;
    pub const UNAVAILABLE: i64 = -32002;
}

/// One line received from a plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// Plugin → Agentty call; answer with the same id.
    Request { id: Value, method: String, params: Value },
    /// Plugin → Agentty notification (no answer).
    Notification { method: String, params: Value },
    /// Answer to a call Agentty made.
    Response { id: Value, result: Result<Value, String> },
}

impl Incoming {
    pub fn parse(line: &str) -> Option<Incoming> {
        let value: Value = serde_json::from_str(line.trim()).ok()?;
        let object = value.as_object()?;
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        match (object.get("id").filter(|id| !id.is_null()).cloned(), object.get("method").and_then(Value::as_str)) {
            (Some(id), Some(method)) => Some(Incoming::Request { id, method: method.to_string(), params }),
            (None, Some(method)) => Some(Incoming::Notification { method: method.to_string(), params }),
            (Some(id), None) => {
                let result = match object.get("error") {
                    Some(error) => Err(error.get("message").and_then(Value::as_str).unwrap_or("error").to_string()),
                    None => Ok(object.get("result").cloned().unwrap_or(Value::Null)),
                };
                Some(Incoming::Response { id, result })
            }
            (None, None) => None,
        }
    }
}

pub fn response(id: &Value, result: Result<Value, (i64, String)>) -> String {
    match result {
        Ok(result) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        Err((code, message)) => {
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
        }
    }
}

pub fn notification(method: &str, params: Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

pub fn request(id: u64, method: &str, params: Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// Where a prompt goes (`prompt/inject`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptTarget {
    /// Show the "Send to…" dialog: new workspace, new tab or an open workspace.
    #[default]
    Ask,
    /// The focused pane.
    Active,
    NewWorkspace,
    NewTab,
    /// A new pane split off the focused one, in the current tab.
    Split,
    /// `paneId`.
    Pane,
    /// An idle agent in `workspaceId`, or a new agent tab there.
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub text: String,
    /// Workspace / tab name for new sessions, and the dialog's heading.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub target: PromptTarget,
    #[serde(default)]
    pub pane_id: Option<u64>,
    #[serde(default)]
    pub workspace_id: Option<u64>,
    /// `claude`, `codex` or `shell` for new sessions (default: Claude Code).
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Press Enter after typing it (new agent sessions always start with it).
    #[serde(default = "default_submit")]
    pub submit: bool,
    /// Who asked, shown in the dialog (a plugin name or an app).
    #[serde(default)]
    pub source: Option<String>,
    /// The plugin that asked, by id. Set by the host beside `source`, never read from the wire:
    /// a plugin must not be able to name another one. It is what tells Agentty whose pane the
    /// dialog's answer became, so a plugin hears about a session the user placed by hand.
    #[serde(skip)]
    pub plugin: Option<String>,
}

fn default_submit() -> bool {
    true
}

impl Default for PromptRequest {
    fn default() -> Self {
        Self {
            text: String::new(),
            title: None,
            target: PromptTarget::Ask,
            pane_id: None,
            workspace_id: None,
            agent: None,
            cwd: None,
            submit: true,
            source: None,
            plugin: None,
        }
    }
}

/// Prompts longer than this are written to a file the agent is asked to read.
pub const INLINE_PROMPT_LIMIT: usize = 60_000;

/// Moves a long prompt into `~/.agentty/prompts/` (0600) and returns a short prompt pointing to it.
pub fn spill_long_prompt(text: &str, title: Option<&str>) -> std::io::Result<String> {
    if text.len() <= INLINE_PROMPT_LIMIT {
        return Ok(text.to_string());
    }
    let dir = crate::fsutil::data_dir().join("prompts");
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = dir.join(format!("prompt-{stamp}.md"));
    std::fs::write(&path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    let heading = title.map(|t| format!("{t}\n\n")).unwrap_or_default();
    Ok(format!("{heading}The full request is in {} — read that file first, then continue from it.", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_messages() {
        assert_eq!(
            Incoming::parse(r#"{"jsonrpc":"2.0","id":1,"method":"ui/notify","params":{"message":"hi"}}"#),
            Some(Incoming::Request { id: json!(1), method: "ui/notify".into(), params: json!({ "message": "hi" }) })
        );
        assert_eq!(
            Incoming::parse(r#"{"jsonrpc":"2.0","method":"ui/setPanel","params":{}}"#),
            Some(Incoming::Notification { method: "ui/setPanel".into(), params: json!({}) })
        );
        assert_eq!(
            Incoming::parse(r#"{"jsonrpc":"2.0","id":"a","error":{"code":1,"message":"nope"}}"#),
            Some(Incoming::Response { id: json!("a"), result: Err("nope".into()) })
        );
        assert_eq!(Incoming::parse("not json"), None);
        assert_eq!(Incoming::parse("[1,2]"), None);
    }

    #[test]
    fn permissions_per_method() {
        assert_eq!(required_permission("ui/setPanel"), Some(None));
        // Anything that reaches the user's files or terminals needs a declared permission.
        assert_eq!(required_permission("host/revealPath"), Some(Some("workspace.read")));
        assert_eq!(required_permission("session/get"), Some(Some("session.read")));
        assert!(required_permission("fs/deleteEverything").is_none());
        for (_, permission) in HOST_METHODS {
            if let Some(permission) = permission {
                assert!(manifest::PERMISSIONS.iter().any(|(name, _)| name == permission));
            }
        }
    }

    #[test]
    fn prompt_request_defaults() {
        let request: PromptRequest = serde_json::from_value(json!({ "text": "hi" })).unwrap();
        assert_eq!(request.target, PromptTarget::Ask);
        assert!(request.submit);
        let request: PromptRequest = serde_json::from_value(json!({ "text": "hi", "target": "newWorkspace", "submit": false })).unwrap();
        assert_eq!(request.target, PromptTarget::NewWorkspace);
        assert!(!request.submit);
        assert_eq!(spill_long_prompt("short", None).unwrap(), "short");
    }
}
