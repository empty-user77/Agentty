//! Plugins as MCP tools: what `agentty mcp-plugins` lists and calls on behalf of an agent in a
//! pane. A plugin is reachable only when it says so — the `mcp.tools` permission and the tools
//! it declares — and the user has not turned its MCP connection off on its page. Anything else
//! is not offered and not called, whatever the agent asks for.

use super::{ensure_started, host, host_mut, plugin, request, touch, Manifest, RunState};
use agentty_bridge::plugins::store::InstalledPlugin;
use gpui::App;
use serde_json::{json, Value};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// How long a plugin has to answer a tool call.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Tool calls one plugin may have waiting for an answer.
const MAX_PENDING: usize = 8;
/// Largest arguments passed on to a plugin (as JSON).
const MAX_ARGUMENT_BYTES: usize = 256 * 1024;

/// A tool call waiting for the plugin's answer; `reply` gets the socket's answer line.
pub struct PendingCall {
    plugin: String,
    reply: Sender<String>,
}

fn answer(result: Result<Value, String>) -> String {
    match result {
        Ok(value) => json!({ "ok": true, "result": value }).to_string(),
        Err(error) => json!({ "ok": false, "error": error }).to_string(),
    }
}

/// Whether the user left `id`'s MCP connection on (the default for a plugin that offers tools).
pub fn allowed_by_user(id: &str, cx: &App) -> bool {
    !crate::settings::settings(cx).plugin_mcp_off.contains(id)
}

/// The plugin, if an agent may reach it through MCP right now: installed, enabled, offering tools
/// under `mcp.tools`, and not turned off by the user.
fn reachable<'a>(id: &str, cx: &'a App) -> Result<(&'a InstalledPlugin, &'a Manifest), String> {
    let found = plugin(cx, id).filter(|p| p.manifest.as_ref().is_some_and(Manifest::supports_mcp));
    let Some(found) = found else { return Err(format!("no plugin \"{id}\" offers tools to agents (see plugins_list)")) };
    let name = found.name().to_string();
    if !found.active() {
        return Err(format!("the plugin {name} is disabled in Agentty"));
    }
    if !allowed_by_user(id, cx) {
        return Err(format!("the user turned off the MCP connection of {name} (Agentty → Plugins → {name})"));
    }
    Ok((found, found.manifest.as_ref().expect("checked above")))
}

/// Every plugin an agent may call now, with its tools.
pub fn list(cx: &App) -> Value {
    let plugins: Vec<Value> = host(cx)
        .installed
        .iter()
        .filter_map(|p| reachable(&p.id, cx).ok())
        .map(|(plugin, manifest)| {
            let state = match super::runtime(cx, &plugin.id).map(|r| &r.state) {
                Some(RunState::Running | RunState::Starting) => "running",
                Some(RunState::NeedsConsent) => "waiting for the user to allow it",
                Some(RunState::Failed(_)) => "failed (starts again when called)",
                _ => "stopped (starts when called)",
            };
            let tools: Vec<Value> = manifest
                .contributes
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.schema(),
                        "readOnly": tool.read_only,
                    })
                })
                .collect();
            json!({
                "plugin": plugin.id,
                "name": plugin.name(),
                "description": manifest.description,
                "state": state,
                "tools": tools,
            })
        })
        .collect();
    json!({ "plugins": plugins })
}

/// Sends `tools/call` from the agent in `pane` to `plugin_id` (started first if it is not running) and answers on `reply`
/// once the plugin does, or with an error right away when it may not be called.
pub fn call(pane: u64, plugin_id: &str, tool: &str, arguments: Value, reply: Sender<String>, cx: &mut App) {
    let refuse = |error: String| {
        let _ = reply.send(answer(Err(error)));
    };
    let (name, declared) = match reachable(plugin_id, cx) {
        Ok((plugin, manifest)) => (plugin.name().to_string(), manifest.tool(tool).is_some()),
        Err(error) => return refuse(error),
    };
    if !declared {
        return refuse(format!("{name} has no tool \"{tool}\" (see plugins_list)"));
    }
    let arguments = match arguments {
        Value::Null => json!({}),
        Value::Object(_) => arguments,
        _ => return refuse("arguments must be an object".into()),
    };
    if arguments.to_string().len() > MAX_ARGUMENT_BYTES {
        return refuse(format!("arguments are larger than {MAX_ARGUMENT_BYTES} bytes"));
    }
    if host(cx).tool_calls.values().filter(|call| call.plugin == plugin_id).count() >= MAX_PENDING {
        return refuse(format!("{name} is still busy with {MAX_PENDING} earlier calls"));
    }
    // Starting asks the user first when the plugin has not been allowed yet; that answer does not
    // come in time for this call.
    if !ensure_started(plugin_id, &super::default_context(cx), cx) {
        let waiting = super::runtime(cx, plugin_id).is_some_and(|r| r.state == RunState::NeedsConsent);
        return refuse(if waiting {
            format!("{name} has not been allowed to run yet: the user is asked in Agentty; try again after they allow it")
        } else {
            format!("{name} could not be started")
        });
    }
    let context = super::default_context(cx);
    let state = host_mut(cx);
    let request_id = state.next_request;
    state.next_request += 1;
    let Some(process) = state.runtimes.get_mut(plugin_id).and_then(|r| {
        r.log(format!("tool {tool} called by the agent in pane {pane}"));
        r.process.as_ref()
    }) else {
        return refuse(format!("{name} is not running"));
    };
    process.send(request(request_id, "tools/call", json!({ "name": tool, "arguments": arguments, "context": context })));
    state.tool_calls.insert(request_id, PendingCall { plugin: plugin_id.to_string(), reply });
    touch(cx);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(CALL_TIMEOUT).await;
        let _ = cx.update(|cx| {
            if let Some(call) = host_mut(cx).tool_calls.remove(&request_id) {
                super::log(&call.plugin, format!("tool {request_id} got no answer in {}s", CALL_TIMEOUT.as_secs()), cx);
                let _ = call.reply.send(answer(Err(format!("the plugin did not answer in {}s", CALL_TIMEOUT.as_secs()))));
            }
        });
    })
    .detach();
}

/// Delivers a plugin's answer to a tool call. Returns whether `request_id` was one (an answer to
/// something else Agentty asked, `initialize`, is not).
pub(super) fn answered(plugin_id: &str, request_id: &Value, result: Result<Value, String>, cx: &mut App) -> bool {
    let Some(request_id) = request_id.as_u64() else { return false };
    // Only the plugin that was asked may answer: another one guessing the number would otherwise
    // put its words in the first one's mouth.
    if !host(cx).tool_calls.get(&request_id).is_some_and(|call| call.plugin == plugin_id) {
        return false;
    }
    if let Some(call) = host_mut(cx).tool_calls.remove(&request_id) {
        let _ = call.reply.send(answer(result));
    }
    true
}

/// Fails the calls still waiting on `plugin_id`: it stopped and will not answer them.
pub(super) fn abandon(plugin_id: &str, cx: &mut App) {
    let host = host_mut(cx);
    let gone: Vec<u64> = host.tool_calls.iter().filter(|(_, call)| call.plugin == plugin_id).map(|(id, _)| *id).collect();
    for id in gone {
        if let Some(call) = host.tool_calls.remove(&id) {
            let _ = call.reply.send(answer(Err("the plugin stopped before it answered".into())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_read_like_the_other_socket_replies() {
        let ok: Value = serde_json::from_str(&answer(Ok(json!({ "a": 1 })))).unwrap();
        assert_eq!(ok, json!({ "ok": true, "result": { "a": 1 } }));
        let err: Value = serde_json::from_str(&answer(Err("nope".into()))).unwrap();
        assert_eq!(err, json!({ "ok": false, "error": "nope" }));
    }
}
