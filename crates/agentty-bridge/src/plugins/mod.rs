//! Agentty plugins: separate processes that talk JSON-RPC 2.0 (one JSON object per line) over
//! stdio, add commands, pane-bar buttons and a UI panel, and can read sessions and send prompts.
//!
//! The protocol is documented in `docs/plugins/protocol.md`; the Node.js SDK lives in `sdk/node`.

pub mod files;
pub mod link;
pub mod manifest;
pub mod market;
pub mod net;
pub mod sites;
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
    // A folder of the plugin's own to keep files in (`plugin-data/<id>/files`), and nothing
    // outside it. `files/download` also needs `net.request`, checked where it is handled.
    ("files/write", Some("files")),
    ("files/read", Some("files")),
    ("files/list", Some("files")),
    ("files/stat", Some("files")),
    ("files/remove", Some("files")),
    ("files/rename", Some("files")),
    ("files/copy", Some("files")),
    ("files/download", Some("files")),
    ("files/path", Some("files")),
    ("files/reveal", Some("files")),
    // The user picks files in the system's open panel; copies land in the plugin's folder.
    ("files/pick", Some("files")),
    // An SVG of the plugin's folder drawn into a PNG beside it (an image an agent wrote as code).
    ("media/svgToPng", Some("files")),
    // SVG scenes of the plugin's folder made into a short MP4 beside them.
    ("media/svgsToVideo", Some("files")),
    // An HTML animation of the plugin's folder recorded into an MP4 (a page of its own, no network).
    ("media/htmlToVideo", Some("files")),
    ("prompt/inject", Some("prompt.inject")),
    // Only a terminal `prompt/inject` opened for the plugin: what it opened, it may close.
    ("terminal/close", Some("prompt.inject")),
    // Which agents `prompt/inject` can start here, so a plugin offers only those.
    ("agent/list", Some("prompt.inject")),
    ("terminal/send", Some("terminal.write")),
    ("session/get", Some("session.read")),
    ("workspace/list", Some("workspace.read")),
    // The in-app browser, signed in as the user: only on the sites the manifest names, and never
    // its cookies.
    // The plugin's own workspace: its automations (tabs) and what they are called.
    ("workspace/instances", None),
    ("workspace/setInstanceTitle", None),
    ("workspace/setInstanceStatus", None),
    ("workspace/closeInstance", None),
    ("browser/sites", Some("browser.control")),
    ("browser/open", Some("browser.control")),
    ("browser/navigate", Some("browser.control")),
    ("browser/eval", Some("browser.control")),
    ("browser/wait", Some("browser.control")),
    ("browser/info", Some("browser.control")),
    ("browser/show", Some("browser.control")),
    ("browser/hide", Some("browser.control")),
    ("browser/close", Some("browser.control")),
    ("browser/signIn", Some("browser.control")),
    ("browser/profiles", Some("browser.control")),
    ("browser/removeProfile", Some("browser.control")),
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
    /// A new tab in the plugin's own workspace (`mode: "workspace"`), made if it has none yet: one
    /// tab per job, so a plugin can run several side by side.
    Own,
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
    /// With `target: "own"`: the automation (a tab of the plugin's workspace) the job belongs to;
    /// it opens beside that tab's terminals instead of as a tab of its own.
    #[serde(default)]
    pub instance: Option<String>,
    /// `claude`, `codex` or `shell` for new sessions (default: Claude Code).
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Press Enter after typing it (new agent sessions always start with it).
    #[serde(default = "default_submit")]
    pub submit: bool,
    /// `"files"`: a new agent that only reads and writes files in `cwd` — no shell, no web, no MCP
    /// tools. For text that is not the user's (web pages, posts, mail) handed to an agent.
    #[serde(default)]
    pub tools: Option<String>,
    /// The model the new agent runs on (`claude --model`, `codex -m`); one `agent/list` named.
    #[serde(default)]
    pub model: Option<String>,
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

impl PromptRequest {
    /// Whether the agent it starts gets files only (`tools: "files"`).
    pub fn restricted(&self) -> bool {
        self.tools.as_deref() == Some("files")
    }

    /// The model asked for, when it reads like a model name (letters, digits, `.-_:/[]`, at most
    /// 80 characters): it becomes an argument of the agent's command line.
    pub fn model(&self) -> Option<String> {
        self.model.clone().filter(|m| {
            !m.is_empty() && m.len() <= 80 && !m.starts_with('-') && m.chars().all(|c| c.is_ascii_alphanumeric() || ".-_:/[]".contains(c))
        })
    }
}

impl Default for PromptRequest {
    fn default() -> Self {
        Self {
            text: String::new(),
            title: None,
            target: PromptTarget::Ask,
            pane_id: None,
            workspace_id: None,
            instance: None,
            agent: None,
            cwd: None,
            submit: true,
            tools: None,
            model: None,
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

    #[test]
    fn a_model_is_taken_only_when_it_reads_like_one() {
        let with = |model: &str| PromptRequest { model: Some(model.into()), ..PromptRequest::default() }.model();
        assert_eq!(with("claude-opus-5-5[1m]").as_deref(), Some("claude-opus-5-5[1m]"));
        assert_eq!(with("gpt-6-astra").as_deref(), Some("gpt-6-astra"));
        assert_eq!(with("--dangerously-skip-permissions"), None);
        assert_eq!(with("opus; rm -rf ~"), None);
        assert_eq!(with(""), None);
    }
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

#[cfg(test)]
mod robustness {
    //! Everything a plugin, a marketplace or a link can put in front of Agentty, in shapes nobody
    //! meant. None of it may panic: a plugin that can crash the app it runs inside is worse than
    //! one that does nothing.

    use serde_json::{json, Value};

    /// Values of every shape, nested, for any field that takes one.
    fn awkward_values() -> Vec<Value> {
        let mut values = vec![
            Value::Null,
            json!(true),
            json!(0),
            json!(-1),
            json!(u64::MAX),
            json!(i64::MIN),
            json!(1.5e308),
            json!(""),
            json!("   "),
            json!("\0\u{7f}\u{feff}"),
            json!("../../etc/passwd"),
            json!("\u{202e}gnp.exe"),
            json!("😀".repeat(200)),
            json!("a".repeat(100_000)),
            json!([]),
            json!({}),
            json!([[[[[[[[[[1]]]]]]]]]]),
            json!({ "a": { "b": { "c": { "d": {} } } } }),
        ];
        // A deep-ish array, well inside what serde_json will parse.
        let mut nested = json!(1);
        for _ in 0..64 {
            nested = json!([nested]);
        }
        values.push(nested);
        values
    }

    /// Each field of `shape` replaced, in turn, by each awkward value — plus the whole thing.
    fn variants(shape: &Value) -> Vec<Value> {
        let mut out = awkward_values();
        if let Some(object) = shape.as_object() {
            for key in object.keys() {
                for value in awkward_values() {
                    let mut copy = shape.clone();
                    copy[key] = value;
                    out.push(copy);
                }
                // The field missing altogether.
                let mut copy = shape.clone();
                copy.as_object_mut().expect("an object").remove(key);
                out.push(copy);
            }
        }
        out
    }

    #[test]
    fn a_manifest_of_any_shape_is_read_or_refused_and_never_panics() {
        let shape = json!({
            "id": "plugin", "name": "Plugin", "version": "1.0.0", "main": "main.mjs",
            "runtime": "node", "apiVersion": 1, "permissions": ["net.request"],
            "contributes": { "commands": [{ "id": "plugin.go", "title": "Go" }] },
        });
        for value in variants(&shape) {
            let _ = super::manifest::Manifest::parse(value.to_string().as_bytes());
        }
    }

    #[test]
    fn a_marketplace_entry_of_any_shape_is_read_or_refused_and_never_panics() {
        let shape = json!({
            "id": "plugin", "name": "Plugin", "version": "1.0.0", "description": "A plugin.",
            "publisher": "Someone", "license": "MIT", "apiVersion": 1,
            "source": "https://github.com/someone/plugin",
            "module": {
                "url": "https://raw.githubusercontent.com/someone/plugin/main/p.wasm",
                "sha256": "0".repeat(64), "size": 1024,
            },
        });
        for value in variants(&shape) {
            let _ = super::market::Entry::checked(value);
        }
        // And a whole list of them, including one that is not a list at all.
        for value in awkward_values() {
            let _ = super::market::parse(json!({ "apiVersion": 1, "plugins": value }).to_string().as_bytes());
            let _ = super::market::parse(value.to_string().as_bytes());
        }
    }

    #[test]
    fn a_ui_tree_of_any_shape_is_read_or_refused_and_never_panics() {
        let shape = json!({
            "type": "column",
            "children": [
                { "type": "text", "text": "hello", "style": "title" },
                { "type": "button", "id": "go", "label": "Go" },
                { "type": "input", "id": "url", "value": "", "rows": 4 },
                { "type": "list", "id": "saved", "items": [{ "id": "a", "title": "A" }] },
            ],
        });
        for value in variants(&shape) {
            if let Ok(node) = super::ui::Node::from_value(value) {
                let mut fields = Vec::new();
                node.inputs(&mut fields);
            }
        }
    }

    #[test]
    fn a_link_of_any_shape_is_read_or_refused_and_never_panics() {
        let links = [
            "agentty://plugin",
            "agentty://plugin/",
            "agentty://plugin//////",
            "agentty://plugin/../../etc",
            "agentty://plugin/a%2e%2e%2fb/c?x=%00",
            "agentty://prompt",
            "agentty://prompt?text=",
            "agentty://prompt?file=/etc/passwd",
            "agentty://prompt?file=relative.txt",
            "agentty://prompt?text=hi&cwd=notabsolute",
            "agentty://prompt?text=hi&agent=../../bin/sh",
            "agentty://plugins/",
            "agentty://plugins/NOT-AN-ID",
            "agentty://",
            "agentty:",
            "agentty://unknown/thing",
            "https://example.com",
            "",
            "not a url at all",
            "agentty://prompt?text=%F0%9F%98%80",
        ];
        for raw in links {
            let _ = super::link::parse(raw);
        }
        // A link as long as anything that could arrive.
        let _ = super::link::parse(&format!("agentty://prompt?text={}", "a".repeat(200_000)));
    }

    #[test]
    fn a_message_of_any_shape_from_a_plugin_is_read_or_refused_and_never_panics() {
        let shape = json!({ "jsonrpc": "2.0", "id": 1, "method": "ui/setPanel", "params": { "tree": {} } });
        for value in variants(&shape) {
            let _ = super::Incoming::parse(&value.to_string());
        }
        for line in ["", "   ", "{", "null", "[]", "\"a string\"", &"{".repeat(200), "\u{0}", "\n\n"] {
            let _ = super::Incoming::parse(line);
        }
    }

    #[test]
    fn a_prompt_request_of_any_shape_is_read_or_refused_and_never_panics() {
        let shape = json!({
            "text": "hello", "title": "T", "target": "newTab", "paneId": 1,
            "workspaceId": 2, "agent": "claude", "cwd": "/tmp", "submit": true,
        });
        for value in variants(&shape) {
            let _ = serde_json::from_value::<super::PromptRequest>(value);
        }
    }

    #[test]
    fn a_fetch_request_of_any_shape_is_read_or_refused_and_never_panics() {
        let shape = json!({
            "url": "https://example.com", "method": "GET", "headers": { "A": "b" },
            "body": "x", "timeoutMs": 1000, "proxy": "http://127.0.0.1:1",
        });
        for value in variants(&shape) {
            if let Ok(request) = serde_json::from_value::<super::net::FetchRequest>(value) {
                // Checked, never sent: this must refuse or accept, not fall over.
                let _ = super::net::check_for_test(&request);
            }
        }
    }
}
