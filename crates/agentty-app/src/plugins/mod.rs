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
/// Characters a plugin may put on the clipboard at once.
const MAX_COPY_CHARS: usize = 100_000;
/// Characters of `ui/setBadge` kept — what fits beside a plugin's icon.
const MAX_BADGE_CHARS: usize = 8;
/// One notification per plugin per this long; the rest are dropped.
const NOTIFY_INTERVAL: Duration = Duration::from_millis(700);
/// The shortest gap between two URLs one plugin may open in the browser.
const OPEN_URL_INTERVAL: Duration = Duration::from_millis(700);
/// `net/fetch` calls one plugin may have in flight. A request holds a background thread until it
/// answers or times out, so a plugin cannot open as many as it likes.
const MAX_CONCURRENT_FETCHES: u32 = 4;
/// `host/timer` waits one plugin may have running, and how long one may be.
const MAX_TIMERS: u32 = 8;
const MIN_TIMER: Duration = Duration::from_millis(100);
const MAX_TIMER: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    Stopped,
    Starting,
    Running,
    Failed(String),
    /// Not started: the user has not allowed what it asks for yet (a new plugin, or an update
    /// that asks for more).
    NeedsConsent,
}

pub struct Runtime {
    pub state: RunState,
    pub panel: Option<Node>,
    /// Panels of the plugin's automations (tabs of its workspace), by automation id.
    pub panels: HashMap<String, Node>,
    pub badge: String,
    pub logs: VecDeque<String>,
    process: Option<PluginProcess>,
    /// Bumped on every start, so events from an old process are ignored.
    generation: u64,
    stopping: bool,
    /// Messages seen in the current second, for the flood limit.
    rate_window: Option<(Instant, u32)>,
    /// When this plugin last showed a notification.
    notified_at: Option<Instant>,
    /// When this plugin last had a URL opened in the browser.
    opened_url_at: Option<Instant>,
    /// Bytes currently kept in `logs`.
    log_bytes: usize,
    /// `net/fetch` calls this plugin has in flight.
    fetches: u32,
    /// `host/timer` waits this plugin is in.
    timers: u32,
}

impl Runtime {
    fn new() -> Self {
        Self {
            state: RunState::Stopped,
            panel: None,
            panels: HashMap::new(),
            badge: String::new(),
            logs: VecDeque::new(),
            process: None,
            generation: 0,
            stopping: false,
            rate_window: None,
            notified_at: None,
            opened_url_at: None,
            log_bytes: 0,
            fetches: 0,
            timers: 0,
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

    /// The process this runtime had is going, and whatever it still sends belongs to a plugin
    /// that no longer runs. The requests it left in the air go with it: they run to their
    /// timeout, but they are not part of what the next one is allowed to have outstanding —
    /// otherwise a plugin restarted mid-request could make none of its own for a minute.
    fn abandon(&mut self) {
        self.generation += 1;
        self.fetches = 0;
        self.timers = 0;
    }

    /// Whether a URL may be opened now. `host/openUrl` needs no permission — a plugin's "read
    /// this in your browser" button — and the flood limit only ends a plugin after 240 messages,
    /// which is 240 browser tabs. One at a time is all a button ever needs.
    fn may_open_url(&mut self) -> bool {
        let allowed = self.opened_url_at.is_none_or(|at| at.elapsed() >= OPEN_URL_INTERVAL);
        if allowed {
            self.opened_url_at = Some(Instant::now());
        }
        allowed
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
    /// Plugins a link has reached. Held here rather than on the runtime, so that a plugin cannot
    /// shed the guard by letting its process end: it would come back untainted, read the link's
    /// text out of its own storage — which needs no permission — and type it into a terminal.
    /// Only the user restarting it from the Plugins page clears this.
    link_tainted: std::collections::HashSet<String>,
    tx: UnboundedSender<Envelope>,
    next_request: u64,
    /// Bumped whenever something visible changes.
    pub revision: u64,
    /// Windows are refreshed on a timer while plugins keep talking.
    refreshed_at: Option<Instant>,
    refresh_queued: bool,
    /// Plugins waiting for the user to allow what they ask for, in the order they wanted to start.
    pub consent_pending: Vec<String>,
    /// Plugins the user did not allow, this run: not asked about again until the user asks.
    pub consent_refused: std::collections::HashSet<String>,
}

impl Global for PluginHost {}

/// Creates the host and returns the stream of plugin events for the app loop.
pub fn init(cx: &mut App) -> UnboundedReceiver<Envelope> {
    let (tx, rx) = unbounded();
    cx.set_global(PluginHost {
        installed: store::installed(),
        link_tainted: std::collections::HashSet::new(),
        runtimes: HashMap::new(),
        tx,
        next_request: 1,
        revision: 0,
        refreshed_at: None,
        refresh_queued: false,
        consent_pending: Vec::new(),
        consent_refused: std::collections::HashSet::new(),
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

/// Whether `plugin` asks for something the user has not allowed yet. Plugins that come with
/// Agentty are Agentty's own; any other one is asked about before its first start, and again
/// when an update asks for more.
pub fn needs_consent(plugin: &InstalledPlugin, cx: &App) -> bool {
    if plugin.source == store::Source::Builtin {
        return false;
    }
    let Some(manifest) = plugin.manifest.as_ref() else { return false };
    let granted = crate::settings::settings(cx).plugin_permission_grants.get(&plugin.id).cloned().unwrap_or_default();
    manifest.permissions.iter().any(|p| !granted.contains(p))
}

/// The next plugin waiting for the user's answer, if nobody is asking about one yet.
pub fn next_consent(cx: &App) -> Option<String> {
    host(cx).consent_pending.first().cloned()
}

/// The user's answer about `id`'s permissions. Allowed: remembered (its browser sites too, which
/// the same dialog named) and the plugin starts. Not allowed: it stays stopped until the user
/// asks again from its page or panel.
pub fn answer_consent(id: &str, allowed: bool, cx: &mut App) {
    host_mut(cx).consent_pending.retain(|p| p != id);
    let Some(plugin) = plugin(cx, id).cloned() else { return };
    if !allowed {
        host_mut(cx).consent_refused.insert(id.to_string());
        touch(cx);
        return;
    }
    let permissions = plugin.manifest.as_ref().map(|m| m.permissions.clone()).unwrap_or_default();
    let domains: Vec<String> = plugin
        .manifest
        .as_ref()
        .and_then(|m| m.browser.as_ref())
        .map(|b| b.sites.iter().flat_map(|s| s.domains().map(str::to_string)).collect())
        .unwrap_or_default();
    let owner = id.to_string();
    crate::settings::update_settings(cx, move |settings| {
        settings.plugin_permission_grants.insert(owner.clone(), permissions);
        if !domains.is_empty() {
            settings.plugin_browser_grants.insert(owner, domains);
        }
    });
    // Only a plugin still waiting for the answer leaves that state: the same answer can come twice
    // (the dialog of another window), and a plugin already running keeps running as it is.
    if let Some(runtime) = host_mut(cx).runtimes.get_mut(id).filter(|r| matches!(r.state, RunState::NeedsConsent)) {
        runtime.state = RunState::Stopped;
    }
    let starts = plugin.manifest.as_ref().is_some_and(Manifest::starts_with_agentty);
    if starts {
        ensure_started(id, &default_context(cx), cx);
    }
    touch(cx);
}

/// Asks again about a plugin the user did not allow (its page's or panel's button).
pub fn ask_consent_again(id: &str, cx: &mut App) {
    let host = host_mut(cx);
    host.consent_refused.remove(id);
    if !host.consent_pending.iter().any(|p| p == id) {
        host.consent_pending.push(id.to_string());
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

/// The user's time zone right now, as minutes east of UTC.
fn utc_offset_minutes() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}

/// Every start of any plugin gets the next number (see `Runtime::generation`).
static NEXT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Starts the plugin unless it runs. `context` goes into `initialize`; it is passed in because
/// callers are often inside a window update, where the window can't be read.
fn ensure_started(id: &str, context: &Value, cx: &mut App) -> bool {
    let Some(plugin) = plugin(cx, id).filter(|p| p.active()).cloned() else { return false };
    let running = host(cx).runtimes.get(id).is_some_and(|r| r.process.is_some());
    if running {
        return true;
    }
    // Nothing of a plugin runs before the user has seen and allowed what it asks for.
    if needs_consent(&plugin, cx) {
        let host = host_mut(cx);
        host.runtimes.entry(id.to_string()).or_insert_with(Runtime::new).state = RunState::NeedsConsent;
        if !host.consent_refused.contains(id) && !host.consent_pending.iter().any(|p| p == id) {
            host.consent_pending.push(id.to_string());
        }
        touch(cx);
        return false;
    }
    let language = crate::settings::settings(cx).language.resolved().code().to_string();
    let host = host_mut(cx);
    let tx = host.tx.clone();
    let runtime = host.runtimes.entry(id.to_string()).or_insert_with(Runtime::new);
    // Unique across the app, not per runtime: a runtime is made again when the plugin list is
    // reloaded, and a count that started over would hand the new run what the old one was given.
    runtime.generation = NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    runtime.stopping = false;
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
        // A module has no clock zone of its own: the user's, for dates as the user reads them.
        "utcOffsetMinutes": utc_offset_minutes(),
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
    let host = host_mut(cx);
    host.link_tainted.insert(id.to_string());
    if let Some(runtime) = host.runtimes.get_mut(id) {
        runtime.log(format!("link: {path}"));
    }
    notify_plugin(id, "url/open", json!({ "path": path, "query": query, "url": url, "context": context }), cx)
}

/// Plugins with a process running right now.
pub fn running_plugins(cx: &App) -> Vec<String> {
    host(cx).runtimes.iter().filter(|(_, r)| r.process.is_some()).map(|(id, _)| id.clone()).collect()
}

/// The run of a plugin that is running now (it changes on every start), so what was given to one
/// run — a browser page — is not handed to the next.
pub fn generation(id: &str, cx: &App) -> Option<u64> {
    host(cx).runtimes.get(id).filter(|r| r.process.is_some()).map(|r| r.generation)
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
        runtime.abandon();
        runtime.state = RunState::Stopped;
        runtime.panel = None;
        runtime.panels.clear();
        runtime.badge.clear();
    }
    touch(cx);
}

/// Whether a link has reached this plugin, so what it asks for next may be the link author's wish
/// rather than the user's — any website can open one. While this holds, the plugin cannot type
/// into a terminal and its prompts go through the "Send to…" dialog.
///
/// Nothing lifts it but the user restarting the plugin. Not a click in the panel the link opened,
/// which is not consent to type into a terminal; not waiting, which a plugin can do as easily as
/// a user can click; and not the plugin's own process ending, which it can arrange — it would
/// otherwise come back untainted, read the link's text back out of its own storage (which needs
/// no permission) and carry on.
pub fn link_guarded(id: &str, cx: &App) -> bool {
    host(cx).link_tainted.contains(id)
}

/// The user asking for the plugin to start again — the one thing that clears a link's guard.
pub fn restart(id: &str, cx: &mut App) {
    stop(id, cx);
    host_mut(cx).link_tainted.remove(id);
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
                    runtime.abandon();
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
    let guarded = link_guarded(plugin_id, cx);
    match method {
        "ui/setPanel" => {
            let tree = Node::from_value(params.get("tree").cloned().unwrap_or(Value::Null));
            let result = match tree {
                Ok(tree) => {
                    let instance = params.get("instance").and_then(Value::as_str).map(str::to_string);
                    if let Some(runtime) = host_mut(cx).runtimes.get_mut(plugin_id) {
                        match instance {
                            // One panel per automation; a plugin cannot pile up more than it could run.
                            Some(instance) if runtime.panels.len() < 64 || runtime.panels.contains_key(&instance) => {
                                runtime.panels.insert(instance, tree);
                            }
                            Some(_) => {}
                            None => runtime.panel = Some(tree),
                        }
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
            let text: String = params.get("text").and_then(Value::as_str).unwrap_or_default().chars().take(MAX_BADGE_CHARS).collect();
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
                "utcOffsetMinutes": utc_offset_minutes(),
            })),
            cx,
        ),
        "net/fetch" => fetch(plugin_id, request_id, params, cx),
        "files/download" => download(plugin_id, request_id, params, cx),
        method if method.starts_with("files/") => {
            let result = files_call(plugin_id, method, &params, cx);
            reply(result, cx)
        }
        "host/timer" => timer(plugin_id, request_id, params, cx),
        // The plugin's own folder: what it keeps between runs. A wasm plugin has no files of its
        // own, so without this it forgets everything each time it starts.
        "storage/get" | "storage/set" | "storage/keys" => {
            use agentty_bridge::plugins::storage;
            let key = params.get("key").and_then(Value::as_str).unwrap_or_default().to_string();
            let result = match method {
                "storage/get" => storage::get(plugin_id, &key).map(|value| json!({ "key": key, "value": value })),
                "storage/set" => storage::set(plugin_id, &key, params.get("value").cloned().unwrap_or(Value::Null)).map(|()| Value::Null),
                _ => storage::keys(plugin_id).map(|keys| Value::Array(keys.into_iter().map(Value::String).collect())),
            };
            // A key that is not a key is the plugin's mistake; a folder that cannot be read is
            // the machine's, and a plugin that cannot tell them apart retries the wrong one.
            let code = match method {
                "storage/keys" => codes::INTERNAL,
                _ if storage::valid_key(&key) => codes::INTERNAL,
                _ => codes::INVALID_PARAMS,
            };
            reply(result.map_err(|err| (code, format!("{err:#}"))), cx)
        }
        "host/copy" => {
            let text = params.get("text").and_then(Value::as_str).unwrap_or_default();
            if text.is_empty() {
                return reply(Err((codes::INVALID_PARAMS, "nothing to copy".into())), cx);
            }
            // Bounded: the clipboard is the user's, and a plugin should not be able to fill it.
            let text: String = text.chars().take(MAX_COPY_CHARS).collect();
            // Writing to the clipboard is quiet by nature — what replaced what the user had
            // copied is at least in the plugin's log.
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(plugin_id) {
                runtime.log(format!("copied {} characters to the clipboard", text.chars().count()));
            }
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            reply(Ok(Value::Null), cx)
        }
        "host/openUrl" => {
            let url = params.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return reply(Err((codes::INVALID_PARAMS, "only http(s) URLs can be opened".into())), cx);
            }
            if !host_mut(cx).runtimes.get_mut(plugin_id).is_some_and(Runtime::may_open_url) {
                // Refused rather than dropped: a plugin that opens a URL on a click has one click
                // to answer for, and a plugin looping sees that it is being held back.
                if let Some(runtime) = host_mut(cx).runtimes.get_mut(plugin_id) {
                    runtime.log(format!("openUrl held back: {}", agentty_bridge::extensions::redact_url(&url)));
                }
                return reply(Err((codes::UNAVAILABLE, "one URL at a time".into())), cx);
            }
            cx.open_url(&url);
            reply(Ok(Value::Null), cx)
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
        // The browser is signed in as the user: a link's author must not get to drive it.
        browser if browser.starts_with("browser/") && guarded => {
            reply(Err((codes::PERMISSION_DENIED, "a plugin that a link reached may not use the browser; restart it first".into())), cx)
        }
        browser if browser.starts_with("browser/") && !crate::platform::HAS_WEBVIEW => {
            reply(Err((codes::UNAVAILABLE, "the in-app browser is not available on this platform yet".into())), cx)
        }
        browser if browser.starts_with("browser/") => {
            let unanswered = request_id.clone();
            let tab = params.get("tabId").and_then(Value::as_u64);
            let request = PluginCall {
                plugin: plugin_id.to_string(),
                plugin_name: manifest.name.clone(),
                request_id,
                method: method.to_string(),
                params,
            };
            // A page lives in the window it was opened in: calls about it go there.
            let delivered = crate::with_workbench_for_browser(cx, tab, |workbench, window, cx| workbench.plugin_call(request, window, cx));
            if let (false, Some(request_id)) = (delivered, unanswered) {
                respond(plugin_id, &request_id, Err((codes::UNAVAILABLE, "no Agentty window is open".into())), cx);
            }
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

/// `host/timer`: a request answered once the time has passed. A module runs only while it is
/// handling a message, so this is the whole of how it waits — and answering it is all the plugin
/// gets, which is why it is not a way to run in the background.
fn timer(plugin_id: &str, request_id: Option<Value>, params: Value, cx: &mut App) {
    let id = plugin_id.to_string();
    let Some(request_id) = request_id else {
        if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
            runtime.log("host/timer needs a request id to be answered");
        }
        return;
    };
    let wait = Duration::from_millis(params.get("ms").and_then(Value::as_u64).unwrap_or(0)).clamp(MIN_TIMER, MAX_TIMER);
    let generation = host(cx).runtimes.get(&id).map_or(0, |runtime| runtime.generation);
    let accepted = match host_mut(cx).runtimes.get_mut(&id) {
        Some(runtime) if runtime.timers < MAX_TIMERS => {
            runtime.timers += 1;
            true
        }
        Some(_) => false,
        None => return,
    };
    if !accepted {
        return respond(&id, &request_id, Err((codes::UNAVAILABLE, format!("more than {MAX_TIMERS} waits at once"))), cx);
    }
    cx.spawn(async move |cx| {
        let started = Instant::now();
        cx.background_executor().timer(wait).await;
        let _ = cx.update(|cx| {
            let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) else { return };
            // The plugin was restarted while it waited: this answer belongs to the one that is
            // gone, and its place in the count went with it — taking one off now would be taking
            // it off a wait the new plugin is in. A wait may be an hour long, so a plugin that
            // inherited eight of them could not wait at all for that hour.
            if runtime.generation != generation {
                return;
            }
            runtime.timers = runtime.timers.saturating_sub(1);
            respond(&id, &request_id, Ok(json!({ "elapsedMs": started.elapsed().as_millis() as u64 })), cx);
        });
    })
    .detach();
}

/// `net/fetch`: the plugin's own HTTP request, made on a background thread and answered when it
/// comes back. Agentty adds nothing to it — no cookie, no stored credential, no header of its own
/// beyond what the HTTP client must set — so a plugin reaches exactly what it was given.
/// `files/*` but `download`: the plugin's own folder, answered right away.
fn files_call(plugin_id: &str, method: &str, params: &Value, _cx: &mut App) -> Result<Value, (i64, String)> {
    use agentty_bridge::plugins::files;
    use base64::Engine as _;
    let path = params.get("path").and_then(Value::as_str).unwrap_or_default();
    let bad = |err: anyhow::Error| (codes::INVALID_PARAMS, format!("{err:#}"));
    match method {
        "files/write" => {
            let bytes = match (params.get("text").and_then(Value::as_str), params.get("base64").and_then(Value::as_str)) {
                (Some(text), None) => text.as_bytes().to_vec(),
                (None, Some(data)) => base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| (codes::INVALID_PARAMS, "base64 is not base64".to_string()))?,
                _ => return Err((codes::INVALID_PARAMS, "give either text or base64".into())),
            };
            let append = params.get("append").and_then(Value::as_bool).unwrap_or(false);
            files::write(plugin_id, path, &bytes, append).map(|size| json!({ "size": size })).map_err(bad)
        }
        "files/read" => {
            let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let length = params.get("length").and_then(Value::as_u64).unwrap_or(files::MAX_READ as u64) as usize;
            let (bytes, size) = files::read(plugin_id, path, offset, length).map_err(bad)?;
            let end = offset + bytes.len() as u64 >= size;
            if params.get("as").and_then(Value::as_str) == Some("base64") {
                Ok(json!({ "base64": base64::engine::general_purpose::STANDARD.encode(&bytes), "size": size, "eof": end }))
            } else {
                let text =
                    String::from_utf8(bytes).map_err(|_| (codes::INVALID_PARAMS, format!("{path} is not text; read it as base64")))?;
                Ok(json!({ "text": text, "size": size, "eof": end }))
            }
        }
        "files/list" => files::list(plugin_id, path).map(|entries| json!(entries)).map_err(bad),
        "files/stat" => files::stat(plugin_id, path).map(|entry| json!(entry)).map_err(bad),
        "files/remove" => files::remove(plugin_id, path).map(|removed| json!({ "removed": removed })).map_err(bad),
        "files/rename" => {
            let (from, to) = (
                params.get("from").and_then(Value::as_str).unwrap_or_default(),
                params.get("to").and_then(Value::as_str).unwrap_or_default(),
            );
            files::rename(plugin_id, from, to).map(|()| Value::Null).map_err(bad)
        }
        "files/copy" => {
            let (from, to) = (
                params.get("from").and_then(Value::as_str).unwrap_or_default(),
                params.get("to").and_then(Value::as_str).unwrap_or_default(),
            );
            files::copy(plugin_id, from, to).map(|size| json!({ "size": size })).map_err(bad)
        }
        // Where a file is on disk: for an agent the plugin starts to work there (`prompt/inject`
        // with `cwd`), or to show the user.
        "files/path" => files::resolve(plugin_id, path).map(|p| json!({ "path": p })).map_err(bad),
        "files/reveal" => {
            let target = files::resolve(plugin_id, path).map_err(bad)?;
            if !target.exists() {
                return Err((codes::INVALID_PARAMS, format!("{path} is not there")));
            }
            crate::platform::reveal(&target);
            Ok(Value::Null)
        }
        other => Err((codes::METHOD_NOT_FOUND, format!("unknown method {other}"))),
    }
}

/// `files/download`: a GET streamed into a file of the plugin's folder. Needs `net.request` too:
/// it reaches the network the same way `net/fetch` does.
fn download(plugin_id: &str, request_id: Option<Value>, params: Value, cx: &mut App) {
    let id = plugin_id.to_string();
    let Some(request_id) = request_id else { return };
    let allowed =
        crate::plugins::plugin(cx, &id).and_then(|p| p.manifest.as_ref()).is_some_and(|m| m.permissions.iter().any(|p| p == "net.request"));
    if !allowed {
        return respond(&id, &request_id, Err((codes::PERMISSION_DENIED, "files/download needs net.request as well".into())), cx);
    }
    let path = params.get("path").and_then(Value::as_str).unwrap_or_default().to_string();
    let target = match agentty_bridge::plugins::files::resolve(&id, &path) {
        Ok(target) if !target.is_dir() => target,
        Ok(_) => return respond(&id, &request_id, Err((codes::INVALID_PARAMS, format!("{path} is a folder"))), cx),
        Err(err) => return respond(&id, &request_id, Err((codes::INVALID_PARAMS, format!("{err:#}"))), cx),
    };
    let max = params
        .get("maxBytes")
        .and_then(Value::as_u64)
        .unwrap_or(agentty_bridge::plugins::files::MAX_FILE)
        .min(agentty_bridge::plugins::files::MAX_FILE);
    let request: agentty_bridge::plugins::net::FetchRequest = match serde_json::from_value(params) {
        Ok(request) => request,
        Err(err) => return respond(&id, &request_id, Err((codes::INVALID_PARAMS, format!("invalid request: {err}"))), cx),
    };
    let generation = host(cx).runtimes.get(&id).map_or(0, |runtime| runtime.generation);
    let line = format!("GET {} → {path}", agentty_bridge::extensions::redact_url(&request.url));
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
        return respond(&id, &request_id, Err((codes::UNAVAILABLE, format!("more than {MAX_CONCURRENT_FETCHES} requests at once"))), cx);
    }
    let task = cx.background_executor().spawn(async move { agentty_bridge::plugins::net::download(&request, &target, max) });
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = cx.update(|cx| {
            {
                let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) else { return };
                if runtime.generation != generation {
                    return;
                }
                runtime.fetches = runtime.fetches.saturating_sub(1);
            }
            let answer = result.map(|done| json!(done)).map_err(|err| (codes::UNAVAILABLE, format!("{err:#}")));
            respond(&id, &request_id, answer, cx);
        });
    })
    .detach();
}

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
                // The plugin was restarted while this was in the air: the answer belongs to a
                // plugin that is gone, and its request id means nothing to the one running now.
                // Its place in the count went with it — `stop` cleared the count, and taking one
                // off now would be taking it off the requests the new one has in the air.
                if runtime.generation != generation {
                    return;
                }
                runtime.fetches = runtime.fetches.saturating_sub(1);
                match result {
                    Ok(response) => {
                        runtime.log(format!("  → {} ({} bytes, {} ms)", response.status, response.bytes, response.duration_ms));
                        serde_json::to_value(response).map_err(|err| (codes::INTERNAL, err.to_string()))
                    }
                    Err(err) => {
                        let message = format!("{err:#}");
                        // The message may quote the URL, and a URL may carry a token.
                        runtime.log(format!("  → failed: {}", agentty_bridge::extensions::mask_words(&message)));
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
#[derive(Clone)]
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
    fn one_url_at_a_time() {
        let mut runtime = Runtime::new();
        assert!(runtime.may_open_url());
        assert!(!runtime.may_open_url(), "a plugin cannot open a second URL straight away");
        runtime.opened_url_at = Some(Instant::now() - OPEN_URL_INTERVAL);
        assert!(runtime.may_open_url());
    }

    #[test]
    fn a_restart_does_not_inherit_the_requests_of_the_plugin_before_it() {
        let mut runtime = Runtime::new();
        runtime.fetches = MAX_CONCURRENT_FETCHES;
        runtime.timers = MAX_TIMERS;
        let before = runtime.generation;
        runtime.abandon();
        assert!(runtime.generation > before, "answers meant for the old plugin are told apart");
        assert_eq!(runtime.fetches, 0, "the new plugin starts with nothing in the air");
        assert_eq!(runtime.timers, 0, "and in no wait it did not ask for");
    }

    #[test]
    fn the_link_guard_survives_the_plugin_that_earned_it() {
        // The guard is a set on the host, not a flag on the process, and that is the whole point:
        // a plugin can end its own process. It would come back with a clean flag, read the link's
        // text out of its own storage — which needs no permission — and type it into a terminal.
        let mut tainted: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert!(!tainted.contains("plugin"));
        tainted.insert("plugin".into());

        // What used to clear it: the process ending and starting again.
        let mut runtime = Runtime::new();
        runtime.abandon();
        assert!(tainted.contains("plugin"), "a restart the plugin arranged is not consent");

        // Neither a click in the panel the link opened nor simply waiting is consent either; a
        // plugin can wait as easily as a user can click, so the guard carries no clock at all.
        runtime.notified_at = Some(Instant::now() - Duration::from_secs(3600));
        runtime.opened_url_at = Some(Instant::now() - Duration::from_secs(3600));
        assert!(tainted.contains("plugin"), "an hour of doing other things is not consent");

        // Only the user pressing Restart, which is the one place that removes it.
        tainted.remove("plugin");
        assert!(!tainted.contains("plugin"));
    }
}
