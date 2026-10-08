//! `agentty mcp-plugins`: the tools Agentty's plugins offer, as MCP tools for Claude Code and
//! Codex started in Agentty. Only plugins that declare `mcp.tools` and whose MCP connection the
//! user left on are listed or called; Agentty checks that on every call, not this process.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};

const PROTOCOL_VERSION: &str = "2025-06-18";
/// Most text of one answer handed to the agent.
const MAX_TEXT: usize = 100_000;

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "plugins_list",
            "description": "Agentty plugins that offer tools to agents, with each tool's name, description, input schema and whether it only reads. Call this first to find the plugin and tool for a question about something a plugin manages, then call plugin_call.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "plugin_call",
            "description": "Call one tool of an Agentty plugin (see plugins_list) and return its answer. The plugin starts if it is not running.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "plugin": { "type": "string", "description": "plugin id from plugins_list" },
                    "tool": { "type": "string", "description": "tool name from plugins_list" },
                    "arguments": { "type": "object", "description": "arguments matching the tool's inputSchema" },
                },
                "required": ["plugin", "tool"],
            },
        }),
    ]
}

/// One request to Agentty's socket, as the pane this agent runs in; the answer line.
fn request(args: &Value) -> Result<Value, String> {
    let socket = std::env::var("AGENTTY_SOCKET").map_err(|_| "Agentty is not reachable ($AGENTTY_SOCKET is not set)".to_string())?;
    let line = (|| -> std::io::Result<String> {
        let mut stream = crate::ipc::connect(&socket)?;
        writeln!(stream, "plugins\t{args}")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(line)
    })()
    .map_err(|err| format!("Agentty is not reachable: {err}"))?;
    let reply: Value = serde_json::from_str(line.trim()).unwrap_or_default();
    if reply["ok"].as_bool() != Some(true) {
        return Err(reply["error"].as_str().unwrap_or("no response from Agentty").to_string());
    }
    Ok(reply["result"].clone())
}

fn clip(mut text: String) -> String {
    if let Some((index, _)) = text.char_indices().nth(MAX_TEXT) {
        text.truncate(index);
        text.push_str("\n… (cut)");
    }
    text
}

fn text_result(text: String, error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": clip(text) }], "isError": error })
}

/// A plugin's answer as MCP content: MCP-shaped content of text and images passes through,
/// a string is text, anything else is its JSON.
fn content_of(result: Value) -> Value {
    if let Some(items) = result.get("content").and_then(Value::as_array) {
        let content: Vec<Value> = items
            .iter()
            .filter_map(|item| match item["type"].as_str() {
                Some("text") => Some(json!({ "type": "text", "text": clip(item["text"].as_str().unwrap_or_default().to_string()) })),
                Some("image") if item["data"].is_string() && item["mimeType"].is_string() => {
                    Some(json!({ "type": "image", "data": item["data"], "mimeType": item["mimeType"] }))
                }
                _ => None,
            })
            .collect();
        return json!({ "content": content, "isError": result["isError"].as_bool().unwrap_or(false) });
    }
    match result {
        Value::String(text) => text_result(text, false),
        Value::Null => text_result("done".into(), false),
        other => text_result(serde_json::to_string_pretty(&other).unwrap_or_default(), false),
    }
}

fn call(name: &str, args: &Value) -> Value {
    let outcome = match name {
        "plugins_list" => request(&json!({ "action": "list" }))
            .map(|result| text_result(serde_json::to_string_pretty(&result).unwrap_or_default(), false)),
        "plugin_call" => request(&json!({
            "action": "call",
            "plugin": args["plugin"],
            "tool": args["tool"],
            "arguments": args.get("arguments").cloned().unwrap_or(json!({})),
        }))
        .map(content_of),
        other => Err(format!("unknown tool {other}")),
    };
    outcome.unwrap_or_else(|error| text_result(error, true))
}

/// What the agent is told when it connects: the plugins it can reach right now, so it knows when
/// to use them without listing first.
fn instructions() -> String {
    let mut text = String::from(
        "Tools of the plugins running in Agentty, the terminal app this agent runs in. When the user asks about something a plugin manages, use plugins_list to find the tool and plugin_call to run it.",
    );
    if let Ok(result) = request(&json!({ "action": "list" })) {
        let plugins: Vec<String> = result["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let tools: Vec<&str> = p["tools"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str()).collect();
                format!("{} ({}): {}", p["name"].as_str().unwrap_or_default(), p["plugin"].as_str().unwrap_or_default(), tools.join(", "))
            })
            .collect();
        if !plugins.is_empty() {
            text.push_str(" Plugins now: ");
            text.push_str(&plugins.join("; "));
            text.push('.');
        }
    }
    text
}

fn handle(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let result = match message["method"].as_str().unwrap_or("") {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "agentty-plugins", "version": env!("CARGO_PKG_VERSION") },
            "instructions": instructions(),
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
    fn lists_its_two_tools() {
        let list = handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["plugins_list", "plugin_call"]);
        assert!(handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());
        assert_eq!(call("nothing", &json!({}))["isError"], json!(true));
    }

    #[test]
    fn a_plugins_answer_becomes_mcp_content() {
        assert_eq!(content_of(json!("hello"))["content"][0]["text"], "hello");
        assert_eq!(content_of(json!(null))["content"][0]["text"], "done");
        assert!(content_of(json!({ "a": 1 }))["content"][0]["text"].as_str().unwrap().contains("\"a\": 1"));
        // MCP-shaped content passes through, minus what is not text or an image.
        let shaped = content_of(json!({
            "content": [{ "type": "text", "text": "hi" }, { "type": "resource", "uri": "file:///etc/passwd" }],
            "isError": true,
        }));
        assert_eq!(shaped["content"].as_array().unwrap().len(), 1);
        assert_eq!(shaped["isError"], json!(true));
        // Long answers are cut.
        let long = content_of(json!("a".repeat(MAX_TEXT + 10)));
        assert!(long["content"][0]["text"].as_str().unwrap().ends_with("(cut)"));
    }
}
