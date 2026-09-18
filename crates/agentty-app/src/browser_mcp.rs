//! `agentty mcp-browser`: the in-app browser as MCP tools, so Claude Code and Codex started in
//! Agentty can open, read, click through and screenshot web pages without extra setup.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL_VERSION: &str = "2025-06-18";

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({ "name": name, "description": description, "inputSchema": { "type": "object", "properties": properties, "required": required } })
}

fn tools() -> Vec<Value> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let n = |d: &str| json!({ "type": "number", "description": d });
    vec![
        tool(
            "browser_open",
            "Show Agentty's in-app browser next to the terminal, optionally loading a URL (localhost:3000, https://…).",
            json!({ "url": s("URL to load") }),
            &[],
        ),
        tool("browser_navigate", "Load a URL in the in-app browser.", json!({ "url": s("URL, host:port or search words") }), &["url"]),
        tool("browser_status", "Current URL, title and whether the page is loading.", json!({}), &[]),
        tool("browser_wait_load", "Wait until the page finished loading.", json!({ "timeout_ms": n("default 15000") }), &[]),
        tool(
            "browser_go",
            "Browser history: back, forward or reload.",
            json!({ "action": { "type": "string", "enum": ["back", "forward", "reload"] } }),
            &["action"],
        ),
        tool(
            "browser_text",
            "Visible text of the page, or of the element matching a CSS selector.",
            json!({ "selector": s("CSS selector (optional)") }),
            &[],
        ),
        tool(
            "browser_html",
            "HTML of the page or of an element (truncated at 200k chars).",
            json!({ "selector": s("CSS selector (optional)") }),
            &[],
        ),
        tool(
            "browser_elements",
            "Interactive elements (links, buttons, inputs) with CSS selectors to use with browser_click / browser_type.",
            json!({}),
            &[],
        ),
        tool("browser_click", "Click the element matching a CSS selector.", json!({ "selector": s("CSS selector") }), &["selector"]),
        tool(
            "browser_type",
            "Set the value of an input / textarea / contenteditable and fire input and change events.",
            json!({ "selector": s("CSS selector"), "text": s("text to enter") }),
            &["selector", "text"],
        ),
        tool(
            "browser_press",
            "Dispatch a key press (Enter, Tab, Escape, …) on the focused element; Enter submits its form.",
            json!({ "key": s("key name") }),
            &["key"],
        ),
        tool(
            "browser_wait",
            "Wait for an element to appear.",
            json!({ "selector": s("CSS selector"), "timeout_ms": n("default 10000") }),
            &["selector"],
        ),
        tool(
            "browser_eval",
            "Run JavaScript in the page (await allowed) and return the result as JSON.",
            json!({ "code": s("expression or statements with return") }),
            &["code"],
        ),
        tool("browser_console", "Console output and uncaught errors of the page.", json!({ "clear": { "type": "boolean" } }), &[]),
        tool("browser_screenshot", "Screenshot of what the page shows (returned as an image).", json!({}), &[]),
        tool("browser_close", "Close the in-app browser.", json!({}), &[]),
    ]
}

/// (command, args) for the socket, from a tool call.
fn command_for(name: &str, args: &Value) -> Option<(String, Vec<String>)> {
    let text = |key: &str| args[key].as_str().map(str::to_string);
    let number = |key: &str| args[key].as_f64().map(|n| (n as u64).to_string());
    let list = |items: Vec<Option<String>>| items.into_iter().flatten().collect::<Vec<_>>();
    Some(match name {
        "browser_open" => ("open".into(), list(vec![text("url")])),
        "browser_navigate" => ("navigate".into(), list(vec![text("url")])),
        "browser_status" => ("status".into(), vec![]),
        "browser_wait_load" => ("wait-load".into(), list(vec![number("timeout_ms")])),
        "browser_go" => (text("action")?, vec![]),
        "browser_text" => ("text".into(), list(vec![text("selector")])),
        "browser_html" => ("html".into(), list(vec![text("selector")])),
        "browser_elements" => ("elements".into(), vec![]),
        "browser_click" => ("click".into(), list(vec![text("selector")])),
        "browser_type" => ("type".into(), list(vec![text("selector"), text("text")])),
        "browser_press" => ("press".into(), list(vec![text("key")])),
        "browser_wait" => ("wait".into(), list(vec![text("selector"), number("timeout_ms")])),
        "browser_eval" => ("eval".into(), list(vec![text("code")])),
        "browser_console" => ("console".into(), if args["clear"].as_bool() == Some(true) { vec!["clear".into()] } else { vec![] }),
        "browser_screenshot" => {
            let name = format!(
                "agentty-browser-{}.png",
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
            );
            ("screenshot".into(), vec![std::env::temp_dir().join(name).display().to_string()])
        }
        "browser_close" => ("close".into(), vec![]),
        _ => return None,
    })
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn call(name: &str, args: &Value) -> Value {
    let error = |text: String| json!({ "content": [{ "type": "text", "text": text }], "isError": true });
    let Some((command, arguments)) = command_for(name, args) else { return error(format!("unknown tool {name}")) };
    let Ok(socket) = std::env::var("AGENTTY_SOCKET") else { return error("Agentty is not reachable ($AGENTTY_SOCKET is not set)".into()) };
    let line = match crate::browser_cli::request_line(&command, &arguments, &socket) {
        Ok(line) => line,
        Err(err) => return error(format!("Agentty is not reachable: {err}")),
    };
    let reply: Value = serde_json::from_str(line.trim()).unwrap_or_default();
    if reply["ok"].as_bool() != Some(true) {
        return error(reply["error"].as_str().unwrap_or("no response from Agentty").to_string());
    }
    let result = &reply["result"];
    if command == "screenshot" {
        if let Some(bytes) = result.as_str().and_then(|path| std::fs::read(path).ok()) {
            return json!({ "content": [
                { "type": "image", "data": base64(&bytes), "mimeType": "image/png" },
                { "type": "text", "text": format!("Saved to {}", result.as_str().unwrap_or_default()) }
            ] });
        }
    }
    let text = match result {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
}

fn handle(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let result = match message["method"].as_str().unwrap_or("") {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "agentty-browser", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Agentty's in-app browser, shown next to the terminal. Use it to open and test web pages you build (e.g. a local dev server): navigate, read text or elements, click, type, check the console and take screenshots."
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => call(message["params"]["name"].as_str().unwrap_or(""), &message["params"]["arguments"]),
        other => {
            return Some(
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("method not found: {other}") } }),
            )
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

pub fn serve() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&message),
            Err(_) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } })),
        };
        if let Some(reply) = reply {
            if writeln!(stdout, "{reply}").and_then(|_| stdout.flush()).is_err() {
                return 1;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_tools_and_maps_calls() {
        let list = handle(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
        assert!(list["result"]["tools"].as_array().unwrap().len() >= 15);
        assert_eq!(
            command_for("browser_type", &json!({"selector":"#q","text":"hi"})),
            Some(("type".into(), vec!["#q".into(), "hi".into()]))
        );
        assert_eq!(command_for("browser_go", &json!({"action":"back"})), Some(("back".into(), vec![])));
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
