//! Plugin host: starts plugins on demand, keeps what they show (panel, badge, log) and routes
//! their calls to the window they concern. Plugins are shared by every Agentty window.

pub mod process;
pub mod wasm;

use agentty_bridge::plugins::manifest::Manifest;
use agentty_bridge::plugins::store::{self, InstalledPlugin};
use agentty_bridge::plugins::ui::Node;
use agentty_bridge::plugins::{codes, notification, request, required_permission, response, Incoming, PromptTarget};
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use gpui::{App, Global};
use process::{PluginProcess, ProcessEvent};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const LOG_LINES: usize = 400;
/// Longest log line kept, and the total the log may take.
const LOG_LINE_CHARS: usize = 2_000;
const LOG_BYTES: usize = 256 * 1024;
/// A plugin sending more messages than this in a second is stopped: it would freeze the window.
const MAX_MESSAGES_PER_SECOND: u32 = 240;
/// Windows are refreshed at most this often, however many messages arrive.
const REFRESH_INTERVAL: Duration = Duration::from_millis(50);
/// One notification per plugin per this long; the rest are dropped.
const NOTIFY_INTERVAL: Duration = Duration::from_millis(700);
/// `net/fetch` calls one plugin may have in flight. A request holds a background thread until it
/// answers or times out, so a plugin cannot open as many as it likes.
const MAX_CONCURRENT_FETCHES: u32 = 4;

#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    Stopped,
    Starting,
    Running,
    Failed(String),
}

pub struct Runtime {
    pub state: RunState,
    pub panel: Option<Node>,
    pub badge: String,
    pub logs: VecDeque<String>,
    process: Option<PluginProcess>,
    /// Bumped on every start, so events from an old process are ignored.
    generation: u64,
    stopping: bool,
    /// A link reached this plugin, so what it asks for next may be the link author's wish rather
    /// than the user's. Set for the rest of the process's life — see [`Runtime::link_guarded`].
    link_tainted: bool,
    /// Messages seen in the current second, for the flood limit.
    rate_window: Option<(Instant, u32)>,
    /// When this plugin last showed a notification.
    notified_at: Option<Instant>,
    /// Bytes currently kept in `logs`.
    log_bytes: usize,
    /// `net/fetch` calls this plugin has in flight.
    fetches: u32,
}

impl Runtime {
    fn new() -> Self {
        Self {
            state: RunState::Stopped,
            panel: None,
            badge: String::new(),
            logs: VecDeque::new(),
            process: None,
            generation: 0,
            stopping: false,
            link_tainted: false,
            rate_window: None,
            notified_at: None,
            log_bytes: 0,
            fetches: 0,
        }
    }

    fn log(&mut self, line: impl Into<String>) {
        let mut line: String = line.into();
        if let Some((index, _)) = line.char_indices().nth(LOG_LINE_CHARS) {
            line.truncate(index);
            line.push('…');
        }
        self.log_bytes += line.len();
        self.logs.push_back(line);
        while self.logs.len() > LOG_LINES || (self.log_bytes > LOG_BYTES && self.logs.len() > 1) {
            self.log_bytes -= self.logs.pop_front().map_or(0, |line| line.len());
        }
    }

    /// A link reached this plugin, so it may be acting for whoever wrote the link — any website can
    /// open one. While this holds, the plugin cannot type into a terminal and its prompts have to
    /// go through the "Send to…" dialog.
    ///
    /// Nothing lifts it while the process runs: a click in the panel the link opened is not consent
    /// to type into a terminal, and neither is waiting, which a plugin can simply do (`setTimeout`)
    /// before acting on the text the link gave it. Restarting the plugin clears it.
    fn link_guarded(&self) -> bool {
        self.link_tainted
    }

    /// Counts a message and reports whether the plugin is flooding Agentty.
    fn over_rate_limit(&mut self) -> bool {
        let now = Instant::now();
        match self.rate_window {
            Some((start, count)) if start.elapsed() < Duration::from_secs(1) => {
                self.rate_window = Some((start, count + 1));
                count + 1 > MAX_MESSAGES_PER_SECOND
            }
            _ => {
                self.rate_window = Some((now, 1));
                false
            }
        }
    }

    /// Whether a notification may be shown now (the rest are dropped).
    fn may_notify(&mut self) -> bool {
        let allowed = self.notified_at.is_none_or(|at| at.elapsed() >= NOTIFY_INTERVAL);
        if allowed {
            self.notified_at = Some(Instant::now());
        }
        allowed
    }
}

pub struct Envelope {
    plugin: String,
    generation: u64,
    event: ProcessEvent,
}

pub struct PluginHost {
    pub installed: Vec<InstalledPlugin>,
    runtimes: HashMap<String, Runtime>,
    tx: UnboundedSender<Envelope>,
    next_request: u64,
    /// Bumped whenever something visible changes.
    pub revision: u64,
    /// Windows are refreshed on a timer while plugins keep talking.
    refreshed_at: Option<Instant>,
    refresh_queued: bool,
}

impl Global for PluginHost {}

/// Creates the host and returns the stream of plugin events for the app loop.
pub fn init(cx: &mut App) -> UnboundedReceiver<Envelope> {
    let (tx, rx) = unbounded();
    cx.set_global(PluginHost {
        installed: store::installed(),
        runtimes: HashMap::new(),
        tx,
        next_request: 1,
        revision: 0,
        refreshed_at: None,
        refresh_queued: false,
    });
    // Built-in plugins are updated in place when Agentty ships a newer version.
    let outdated: Vec<String> = host(cx).installed.iter().filter(|p| store::builtin_update_available(p)).map(|p| p.id.clone()).collect();
    for id in outdated {
        if let Err(err) = store::install_builtin(&id) {
            eprintln!("agentty: could not update plugin {id}: {err:#}");
        }
    }
    reload(cx);
    cx.on_app_quit(|cx| {
        for runtime in host_mut(cx).runtimes.values_mut() {
            if let Some(process) = runtime.process.take() {
                process.kill();
            }
        }
        async {}
    })
    .detach();
    rx
}

pub fn host(cx: &App) -> &PluginHost {
    cx.global::<PluginHost>()
}

fn host_mut(cx: &mut App) -> &mut PluginHost {
    cx.global_mut::<PluginHost>()
}

/// Re-reads installed plugins and starts the enabled `onStartup` ones.
pub fn reload(cx: &mut App) {
    let installed = store::installed();
    let state = host_mut(cx);
    // Stop what was removed or disabled.
    let gone: Vec<String> = state.runtimes.keys().filter(|id| !installed.iter().any(|p| &p.id == *id && p.active())).cloned().collect();
    state.installed = installed;
    for id in gone {
        stop(&id, cx);
        host_mut(cx).runtimes.remove(&id);
    }
    let startup: Vec<String> = host(cx)
        .installed
        .iter()
        .filter(|p| p.active() && p.manifest.as_ref().is_some_and(Manifest::starts_with_agentty))
        .map(|p| p.id.clone())
        .collect();
    for id in startup {
        ensure_started(&id, &default_context(cx), cx);
    }
    touch(cx);
}

pub fn plugin<'a>(cx: &'a App, id: &str) -> Option<&'a InstalledPlugin> {
    host(cx).installed.iter().find(|p| p.id == id)
}

/// Enabled, loadable plugins.
pub fn active(cx: &App) -> impl Iterator<Item = (&InstalledPlugin, &Manifest)> {
    host(cx).installed.iter().filter(|p| p.enabled).filter_map(|p| Some((p, p.manifest.as_ref()?)))
}

pub fn runtime<'a>(cx: &'a App, id: &str) -> Option<&'a Runtime> {
    host(cx).runtimes.get(id)
}

/// Marks plugin state as changed. Repaints are spaced out, so a plugin that sends thousands of
/// updates a second can't keep the window busy redrawing.
fn touch(cx: &mut App) {
    let host = host_mut(cx);
    host.revision += 1;
    let due = host.refreshed_at.is_none_or(|at| at.elapsed() >= REFRESH_INTERVAL);
    if due {
        host.refreshed_at = Some(Instant::now());
        cx.refresh_windows();
        return;
    }
    if host.refresh_queued {
        return;
    }
    host.refresh_queued = true;
    cx.spawn(async move |cx| {
        cx.background_executor().timer(REFRESH_INTERVAL).await;
        let _ = cx.update(|cx| {
            let host = host_mut(cx);
            host.refresh_queued = false;
            host.refreshed_at = Some(Instant::now());
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Context for calls not tied to a window (language only).
pub fn default_context(cx: &App) -> Value {
    json!({ "workspace": null, "pane": null, "language": crate::settings::settings(cx).language.code() })
}

/// Starts the plugin unless it runs. `context` goes into `initialize`; it is passed in because
/// callers are often inside a window update, where the window can't be read.
fn ensure_started(id: &str, context: &Value, cx: &mut App) -> bool {
    let Some(plugin) = plugin(cx, id).filter(|p| p.active()).cloned() else { return false };
    let running = host(cx).runtimes.get(id).is_some_and(|r| r.process.is_some());
    if running {
        return true;
    }
    let language = crate::settings::settings(cx).language.resolved().code().to_string();
    let host = host_mut(cx);
    let tx = host.tx.clone();
    let runtime = host.runtimes.entry(id.to_string()).or_insert_with(Runtime::new);
    runtime.generation += 1;
    runtime.stopping = false;
    runtime.link_tainted = false;
    runtime.state = RunState::Starting;
    runtime.log(format!("— starting {} {} —", plugin.name(), plugin.manifest.as_ref().map_or("", |m| m.version.as_str())));
    let generation = runtime.generation;
    let plugin_id = id.to_string();
    let process = PluginProcess::start(&plugin, &language, move |event| {
        let _ = tx.unbounded_send(Envelope { plugin: plugin_id.clone(), generation, event });
    });
    let manifest = plugin.manifest.as_ref();
    let initialize = json!({
        "apiVersion": agentty_bridge::plugins::manifest::API_VERSION,
        "agentty": { "version": env!("CARGO_PKG_VERSION") },
        "plugin": {
            "id": plugin.id,
            "name": plugin.name(),
            "version": manifest.map(|m| m.version.clone()),
            "dir": plugin.dir,
            "dataDir": store::plugin_data_dir(&plugin.id),
        },
        "language": language,
        "context": context,
    });
    let request_id = host.next_request;
    host.next_request += 1;
    process.send(request(request_id, "initialize", initialize));
    if let Some(runtime) = host.runtimes.get_mut(id) {
        runtime.process = Some(process);
    }
    touch(cx);
    true
}

/// Sends a notification to a plugin, starting it first if needed (`params.context` seeds `initialize`).
pub fn notify_plugin(id: &str, method: &str, params: Value, cx: &mut App) -> bool {
    let context = params.get("context").cloned().unwrap_or_else(|| default_context(cx));
    if !ensure_started(id, &context, cx) {
        return false;
    }
    if let Some(process) = host(cx).runtimes.get(id).and_then(|r| r.process.as_ref()) {
        process.send(notification(method, params));
    }
    true
}

/// Delivers an `agentty://plugin/<id>/…` link.
pub fn open_link(
    id: &str,
    path: &str,
    query: &std::collections::BTreeMap<String, String>,
    url: &str,
    context: Value,
    cx: &mut App,
) -> bool {
    if !ensure_started(id, &context, cx) {
        return false;
    }
    if let Some(runtime) = host_mut(cx).runtimes.get_mut(id) {
        runtime.link_tainted = true;
        runtime.log(format!("link: {path}"));
    }
    notify_plugin(id, "url/open", json!({ "path": path, "query": query, "url": url, "context": context }), cx)
}

/// Plugins with a process running right now.
pub fn running_plugins(cx: &App) -> Vec<String> {
    host(cx).runtimes.iter().filter(|(_, r)| r.process.is_some()).map(|(id, _)| id.clone()).collect()
}

/// Sends to a plugin only if it is already running (never starts one).
pub fn send_if_running(id: &str, method: &str, params: Value, cx: &App) {
    if let Some(process) = host(cx).runtimes.get(id).and_then(|r| r.process.as_ref()) {
        process.send(notification(method, params));
    }
}

pub fn stop(id: &str, cx: &mut App) {
    if let Some(runtime) = host_mut(cx).runtimes.get_mut(id) {
        if let Some(process) = runtime.process.take() {
            runtime.stopping = true;
            process.stop();
        }
        // Whatever the old process still sends while it shuts down is ignored.
        runtime.generation += 1;
        runtime.state = RunState::Stopped;
        runtime.panel = None;
        runtime.badge.clear();
    }
    touch(cx);
}

pub fn restart(id: &str, cx: &mut App) {
    stop(id, cx);
    ensure_started(id, &default_context(cx), cx);
}

/// Answers a plugin's call.
pub fn respond(id: &str, request_id: &Value, result: Result<Value, (i64, String)>, cx: &App) {
    if let Some(process) = host(cx).runtimes.get(id).and_then(|r| r.process.as_ref()) {
        process.send(response(request_id, result));
    }
}

/// Handles one event from a plugin process (on the app's main thread).
pub fn handle(envelope: Envelope, cx: &mut App) {
    let Envelope { plugin: id, generation, event } = envelope;
    let current = host(cx).runtimes.get(&id).is_some_and(|r| r.generation == generation);
    if !current {
        return;
    }
    // A plugin that floods Agentty is stopped rather than allowed to freeze the window.
    if matches!(event, ProcessEvent::Message(_) | ProcessEvent::Log(_)) {
        let flooding = host_mut(cx).runtimes.get_mut(&id).is_some_and(Runtime::over_rate_limit);
        if flooding {
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
                runtime.log(format!("— stopped: more than {MAX_MESSAGES_PER_SECOND} messages a second —"));
            }
            stop(&id, cx);
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
                runtime.state = RunState::Failed(format!("sent more than {MAX_MESSAGES_PER_SECOND} messages a second"));
            }
            touch(cx);
            return;
        }
    }
    match event {
        ProcessEvent::Started => {
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
                runtime.state = RunState::Running;
            }
            touch(cx);
        }
        ProcessEvent::Failed(error) => {
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
                runtime.log(format!("error: {error}"));
                runtime.state = RunState::Failed(error);
                // The process may still be alive and writing (a flood the reader gave up on):
                // let it go through `stop`, which asks, then signals, rather than dropping it.
                if let Some(process) = runtime.process.take() {
                    runtime.stopping = true;
                    runtime.generation += 1;
                    process.stop();
                }
            }
            touch(cx);
        }
        ProcessEvent::Log(line) => {
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
                runtime.log(line);
            }
            touch(cx);
        }
        ProcessEvent::Exited(code) => {
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
                runtime.process = None;
                let status = code.map_or_else(|| "was terminated".to_string(), |c| format!("exited with code {c}"));
                runtime.log(format!("— {status} —"));
                runtime.state = if runtime.stopping || code == Some(0) { RunState::Stopped } else { RunState::Failed(status) };
                runtime.stopping = false;
            }
            touch(cx);
        }
        ProcessEvent::Message(Incoming::Response { result: Err(error), .. }) => {
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
                runtime.log(format!("error from plugin: {error}"));
            }
        }
        ProcessEvent::Message(Incoming::Response { .. }) => {}
        ProcessEvent::Message(Incoming::Request { id: request_id, method, params }) => call(&id, Some(request_id), &method, params, cx),
        ProcessEvent::Message(Incoming::Notification { method, params }) => call(&id, None, &method, params, cx),
    }
}

fn call(plugin_id: &str, request_id: Option<Value>, method: &str, mut params: Value, cx: &mut App) {
    let reply = |result: Result<Value, (i64, String)>, cx: &App| {
        if let Some(request_id) = &request_id {
            respond(plugin_id, request_id, result, cx);
        }
    };
    let Some(manifest) = plugin(cx, plugin_id).and_then(|p| p.manifest.clone()) else { return };
    let permission = match required_permission(method) {
        Some(permission) => permission,
        None => return reply(Err((codes::METHOD_NOT_FOUND, format!("unknown method {method}"))), cx),
    };
    if let Some(permission) = permission.filter(|p| !manifest.has_permission(p)) {
        return reply(
            Err((codes::PERMISSION_DENIED, format!("{method} needs the \"{permission}\" permission in agentty-plugin.json"))),
            cx,
        );
    }
    let guarded = host(cx).runtimes.get(plugin_id).is_some_and(Runtime::link_guarded);
    match method {
        "ui/setPanel" => {
            let tree = Node::from_value(params.get("tree").cloned().unwrap_or(Value::Null));
            let result = match tree {
                Ok(tree) => {
                    if let Some(runtime) = host_mut(cx).runtimes.get_mut(plugin_id) {
                        runtime.panel = Some(tree);
                    }
                    touch(cx);
                    Ok(Value::Null)
                }
                Err(error) => Err((codes::INVALID_PARAMS, error)),
            };
            reply(result, cx)
        }
        "ui/notify" if !host_mut(cx).runtimes.get_mut(plugin_id).is_some_and(Runtime::may_notify) => {
            // Dropped: notifications are limited so a plugin can't bury the window in toasts.
            reply(Ok(Value::Null), cx)
        }
        "ui/setBadge" => {
            let text: String = params.get("text").and_then(Value::as_str).unwrap_or_default().chars().take(8).collect();
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(plugin_id) {
                runtime.badge = text;
            }
            touch(cx);
            reply(Ok(Value::Null), cx)
        }
        "host/info" => reply(
            Ok(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "apiVersion": agentty_bridge::plugins::manifest::API_VERSION,
                "language": crate::settings::settings(cx).language.resolved().code(),
            })),
            cx,
        ),
        "net/fetch" => fetch(plugin_id, request_id, params, cx),
        "host/openUrl" => {
            let url = params.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
            if url.starts_with("https://") || url.starts_with("http://") {
                cx.open_url(&url);
                reply(Ok(Value::Null), cx)
            } else {
                reply(Err((codes::INVALID_PARAMS, "only http(s) URLs can be opened".into())), cx)
            }
        }
        "host/revealPath" => {
            let path = std::path::PathBuf::from(params.get("path").and_then(Value::as_str).unwrap_or_default());
            if path.is_absolute() && path.exists() {
                crate::platform::reveal(&path);
                reply(Ok(Value::Null), cx)
            } else {
                // Same answer whether the path is missing or not absolute: a plugin has no
                // business mapping the disk one probe at a time.
                reply(Err((codes::INVALID_PARAMS, "that path cannot be revealed".into())), cx)
            }
        }
        "terminal/send" if guarded => {
            reply(Err((codes::PERMISSION_DENIED, "a plugin that a link reached may not type into terminals; restart it first".into())), cx)
        }
        _ => {
            if method == "prompt/inject" && guarded {
                // Whatever a link asks for, the user picks where it goes and presses Enter.
                params["target"] = serde_json::to_value(PromptTarget::Ask).unwrap_or_default();
                params["submit"] = Value::Bool(false);
            }
            let unanswered = request_id.clone();
            let request = PluginCall {
                plugin: plugin_id.to_string(),
                plugin_name: manifest.name.clone(),
                request_id,
                method: method.to_string(),
                params,
            };
            let delivered = crate::with_active_workbench(cx, |workbench, window, cx| workbench.plugin_call(request, window, cx));
            if let (false, Some(request_id)) = (delivered, unanswered) {
                respond(plugin_id, &request_id, Err((codes::UNAVAILABLE, "no Agentty window is open".into())), cx);
            }
        }
    }
}

/// `net/fetch`: the plugin's own HTTP request, made on a background thread and answered when it
/// comes back. Agentty adds nothing to it — no cookie, no stored credential, no header of its own
/// beyond what the HTTP client must set — so a plugin reaches exactly what it was given.
fn fetch(plugin_id: &str, request_id: Option<Value>, params: Value, cx: &mut App) {
    let id = plugin_id.to_string();
    let Some(request_id) = request_id else {
        // Nothing to answer: a request nobody waits for is not worth a network call.
        if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
            runtime.log("net/fetch needs a request id to be answered");
        }
        return;
    };
    let request: agentty_bridge::plugins::net::FetchRequest = match serde_json::from_value(params) {
        Ok(request) => request,
        Err(err) => return respond(&id, &request_id, Err((codes::INVALID_PARAMS, format!("invalid request: {err}"))), cx),
    };
    let generation = host(cx).runtimes.get(&id).map_or(0, |runtime| runtime.generation);
    // The log is the user's view of what a plugin reached; a token in the URL stays out of it.
    let line = format!("{} {}", request.method.to_uppercase(), agentty_bridge::extensions::redact_url(&request.url));
    let accepted = match host_mut(cx).runtimes.get_mut(&id) {
        Some(runtime) if runtime.fetches < MAX_CONCURRENT_FETCHES => {
            runtime.fetches += 1;
            runtime.log(line);
            true
        }
        Some(_) => false,
        None => return,
    };
    if !accepted {
        let message = format!("more than {MAX_CONCURRENT_FETCHES} requests at once");
        return respond(&id, &request_id, Err((codes::UNAVAILABLE, message)), cx);
    }
    let task = cx.background_executor().spawn(async move { agentty_bridge::plugins::net::fetch(&request) });
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = cx.update(|cx| {
            let answer = {
                let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) else { return };
                runtime.fetches = runtime.fetches.saturating_sub(1);
                // The plugin was restarted while this was in the air: the answer belongs to a
                // plugin that is gone, and its request id means nothing to the one running now.
                if runtime.generation != generation {
                    return;
                }
                match result {
                    Ok(response) => {
                        runtime.log(format!("  → {} ({} bytes, {} ms)", response.status, response.bytes, response.duration_ms));
                        serde_json::to_value(response).map_err(|err| (codes::INTERNAL, err.to_string()))
                    }
                    Err(err) => {
                        let message = format!("{err:#}");
                        runtime.log(format!("  → failed: {message}"));
                        Err((codes::INVALID_PARAMS, message))
                    }
                }
            };
            touch(cx);
            respond(&id, &request_id, answer, cx);
        });
    })
    .detach();
}

/// A plugin call that needs a window (prompts, terminals, sessions, notifications).
pub struct PluginCall {
    pub plugin: String,
    pub plugin_name: String,
    pub request_id: Option<Value>,
    pub method: String,
    pub params: Value,
}

impl PluginCall {
    pub fn reply(&self, result: Result<Value, (i64, String)>, cx: &App) {
        if let Some(request_id) = &self.request_id {
            respond(&self.plugin, request_id, result, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_are_capped_by_size_as_well_as_count() {
        let mut runtime = Runtime::new();
        for _ in 0..10 {
            runtime.log("x".repeat(LOG_LINE_CHARS * 2));
        }
        assert!(runtime.logs.iter().all(|line| line.chars().count() <= LOG_LINE_CHARS + 1), "lines are cut");
        for _ in 0..LOG_LINES * 2 {
            runtime.log("y".repeat(LOG_LINE_CHARS));
        }
        assert!(runtime.logs.len() <= LOG_LINES);
        assert!(runtime.log_bytes <= LOG_BYTES + LOG_LINE_CHARS, "{} bytes kept", runtime.log_bytes);
        assert_eq!(runtime.log_bytes, runtime.logs.iter().map(String::len).sum::<usize>());
    }

    #[test]
    fn floods_are_caught_and_notifications_spaced_out() {
        let mut runtime = Runtime::new();
        for _ in 0..MAX_MESSAGES_PER_SECOND {
            assert!(!runtime.over_rate_limit());
        }
        assert!(runtime.over_rate_limit(), "the next message is over the limit");
        // A new second starts over.
        runtime.rate_window = Some((Instant::now() - Duration::from_secs(2), MAX_MESSAGES_PER_SECOND + 10));
        assert!(!runtime.over_rate_limit());

        assert!(runtime.may_notify());
        assert!(!runtime.may_notify(), "the second notification is dropped");
        runtime.notified_at = Some(Instant::now() - NOTIFY_INTERVAL);
        assert!(runtime.may_notify());
    }

    #[test]
    fn the_link_guard_holds_through_clicks_and_waiting() {
        let mut runtime = Runtime::new();
        assert!(!runtime.link_guarded());
        runtime.link_tainted = true;
        // Neither a click in the panel the link opened nor simply waiting is consent to type into
        // a terminal; a plugin can wait as easily as the user can click.
        assert!(runtime.link_guarded(), "a link arrived and nothing since then lifts the guard");
    }
}
