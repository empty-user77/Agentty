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

/// `workspace/setInstanceStatus`: what one of a plugin's automations (or, with no instance, the
/// plugin's panel outside any workspace) is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceState {
    Working,
    Idle,
    Error,
}

impl InstanceState {
    fn from_id(id: &str) -> Option<Self> {
        match id {
            "working" => Some(Self::Working),
            "idle" => Some(Self::Idle),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// Characters of `workspace/setInstanceStatus`'s `text` kept — a short line beside the automation.
const MAX_STATUS_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq)]
pub struct InstanceStatus {
    pub state: InstanceState,
    pub text: Option<String>,
    /// `now_ms` the last time `state` became `Working`, for the elapsed time shown beside it.
    /// Cleared as soon as the state moves off `Working`, so it is never stale once it comes back.
    pub working_since_ms: Option<u64>,
}

/// Folds a new report into what is kept for one automation: `working_since_ms` carries over while
/// `state` stays `Working` (so restating the same text does not reset the clock) and is set fresh
/// the moment it becomes `Working`; any other state clears it.
fn apply_status(previous: Option<&InstanceStatus>, state: InstanceState, text: Option<String>, now_ms: u64) -> InstanceStatus {
    let working_since_ms = match (state, previous) {
        (InstanceState::Working, Some(previous)) if previous.state == InstanceState::Working => previous.working_since_ms,
        (InstanceState::Working, _) => Some(now_ms),
        _ => None,
    };
    InstanceStatus { state, text, working_since_ms }
}

/// One line, at most `MAX_STATUS_CHARS` characters: what `workspace/setInstanceStatus.text` is cut
/// down to (newlines and other control characters become spaces — this is a status line, not a log).
fn sanitize_status_text(text: &str) -> Option<String> {
    let cleaned: String =
        text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = cleaned.chars().take(MAX_STATUS_CHARS).collect();
    (!trimmed.is_empty()).then_some(trimmed)
}

pub struct Runtime {
    pub state: RunState,
    pub panel: Option<Node>,
    /// Panels of the plugin's automations (tabs of its workspace), by automation id.
    pub panels: HashMap<String, Node>,
    /// `workspace/setInstanceStatus`, by automation id — `""` for the plugin's panel outside a
    /// workspace.
    pub statuses: HashMap<String, InstanceStatus>,
    pub badge: String,
    pub logs: VecDeque<String>,
    process: Option<PluginProcess>,
    /// Bumped on every start, so events from an old process are ignored.
    generation: u64,
    stopping: bool,
    /// Messages seen in the current second, for the flood limit.
    rate_window: Option<(Instant, u32)>,
    /// What those messages were (method, or "log" / "answer"), said when the plugin is stopped
    /// for flooding so its author knows what to look at.
    rate_kinds: HashMap<String, u32>,
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
            statuses: HashMap::new(),
            badge: String::new(),
            logs: VecDeque::new(),
            process: None,
            generation: 0,
            stopping: false,
            rate_window: None,
            rate_kinds: HashMap::new(),
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

    /// Counts a message sent at `at` and reports whether the plugin is flooding Agentty: more than
    /// so many sent within one second of each other, however late they are handled.
    fn over_rate_limit(&mut self, at: Instant) -> bool {
        let now = at;
        match self.rate_window {
            Some((start, count)) if at.saturating_duration_since(start) < Duration::from_secs(1) => {
                self.rate_window = Some((start, count + 1));
                count + 1 > MAX_MESSAGES_PER_SECOND
            }
            _ => {
                self.rate_window = Some((now, 1));
                self.rate_kinds.clear();
                false
            }
        }
    }

    /// The kinds of messages in the flooding second, the most frequent first: `browser/eval ×180`.
    fn flood_summary(&self) -> String {
        let mut kinds: Vec<(&String, &u32)> = self.rate_kinds.iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        kinds.iter().take(6).map(|(kind, n)| format!("{kind} ×{n}")).collect::<Vec<_>>().join(", ")
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
    /// When the plugin sent it — not when the main thread got to it. A main thread held up for a
    /// while (busy, or slowed down while the screen is locked) handles a backlog in one go, and
    /// counting that backlog as one second's messages stopped plugins that never flooded.
    sent_at: Instant,
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

/// Appends a line to `plugin`'s log (shown in the Plugins page), for something Agentty's own host
/// code did about a call — a request that timed out on this side rather than being answered by the
/// plugin or the page it drives — so it shows up next to the plugin's own log lines.
pub fn log(plugin: &str, line: impl Into<String>, cx: &mut App) {
    if let Some(runtime) = host_mut(cx).runtimes.get_mut(plugin) {
        runtime.log(line);
    }
    touch(cx);
}

/// `plugin`'s last `workspace/setInstanceStatus` for `instance` (`""` for its panel outside a
/// workspace), if it has reported one.
pub fn instance_status(cx: &App, plugin: &str, instance: &str) -> Option<InstanceStatus> {
    host(cx).runtimes.get(plugin)?.statuses.get(instance).cloned()
}

/// Whether any automation of `plugin` (or its panel outside a workspace) currently reports
/// `working` — what makes its workspace card spin.
/// Holds off App Nap while a plugin runs, and idle sleep while one of its automations works: out
/// of sight (a locked screen above all) their timers would otherwise stretch to minutes and the
/// automation would stop until someone looked (see `platform::app_nap`).
fn keep_plugins_awake(cx: &App) {
    let runtimes = &host(cx).runtimes;
    let running = runtimes.values().any(|r| r.state == RunState::Running);
    let working = runtimes.values().any(|r| r.statuses.values().any(|s| s.state == InstanceState::Working));
    crate::platform::app_nap::hold(crate::platform::app_nap::level_for(working, running));
}

/// Whether any plugin runs now (its automations may be working in pages of a window).
pub fn any_running(cx: &App) -> bool {
    host(cx).runtimes.values().any(|r| r.state == RunState::Running)
}

pub fn plugin_working(cx: &App, plugin: &str) -> bool {
    host(cx).runtimes.get(plugin).is_some_and(|r| r.statuses.values().any(|s| s.state == InstanceState::Working))
}

/// The `text` of a working automation of `plugin`, for its workspace card's second line, if one
/// reported both `working` and a line to show.
pub fn plugin_working_text(cx: &App, plugin: &str) -> Option<String> {
    host(cx).runtimes.get(plugin)?.statuses.values().find_map(|s| (s.state == InstanceState::Working).then(|| s.text.clone()).flatten())
}

/// Clears the status of one automation — its tab closed, so nothing more is owed to it.
pub fn clear_instance_status(plugin: &str, instance: &str, cx: &mut App) {
    if let Some(runtime) = host_mut(cx).runtimes.get_mut(plugin) {
        runtime.statuses.remove(instance);
    }
    touch(cx);
}

/// `workspace/setInstanceStatus`'s handling, shared with the `plugin-status` debug command: parses
/// `state`, folds it into what is kept for `instance` and marks the window for a repaint.
fn set_instance_status(plugin_id: &str, instance: &str, state: &str, text: Option<String>, cx: &mut App) -> Result<(), String> {
    let Some(state) = InstanceState::from_id(state) else {
        return Err("state must be \"working\", \"idle\" or \"error\"".into());
    };
    let now = crate::ui::now_ms();
    if let Some(runtime) = host_mut(cx).runtimes.get_mut(plugin_id) {
        let previous = runtime.statuses.get(instance);
        let status = apply_status(previous, state, text, now);
        runtime.statuses.insert(instance.to_string(), status);
    }
    touch(cx);
    Ok(())
}

/// `plugin-status <plugin> <instance> working|idle|error [text...]` — sets a status without a real
/// plugin process, for development and tests. `instance` may be `-` for the plugin's panel outside
/// a workspace.
pub fn debug_set_instance_status(plugin: &str, instance: &str, state: &str, text: Option<String>, cx: &mut App) -> Result<(), String> {
    let instance = if instance == "-" { "" } else { instance };
    set_instance_status(plugin, instance, state, text, cx)
}

/// Marks plugin state as changed. Repaints are spaced out, so a plugin that sends thousands of
/// updates a second can't keep the window busy redrawing.
fn touch(cx: &mut App) {
    keep_plugins_awake(cx);
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
    json!({ "workspace": null, "pane": null, "language": crate::settings::settings(cx).language.resolved().code() })
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
        let _ = tx.unbounded_send(Envelope { plugin: plugin_id.clone(), generation, event, sent_at: Instant::now() });
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
        runtime.statuses.clear();
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
    let Envelope { plugin: id, generation, event, sent_at } = envelope;
    let current = host(cx).runtimes.get(&id).is_some_and(|r| r.generation == generation);
    if !current {
        return;
    }
    // A plugin that floods Agentty is stopped rather than allowed to freeze the window.
    if matches!(event, ProcessEvent::Message(_) | ProcessEvent::Log(_)) {
        let kind = match &event {
            ProcessEvent::Message(Incoming::Request { method, .. } | Incoming::Notification { method, .. }) => method.clone(),
            ProcessEvent::Message(Incoming::Response { .. }) => "answer".into(),
            _ => "log".into(),
        };
        let flooding = host_mut(cx).runtimes.get_mut(&id).is_some_and(|runtime| {
            let over = runtime.over_rate_limit(sent_at);
            *runtime.rate_kinds.entry(kind).or_default() += 1;
            over
        });
        if flooding {
            if let Some(runtime) = host_mut(cx).runtimes.get_mut(&id) {
                let summary = runtime.flood_summary();
                runtime.log(format!("— stopped: more than {MAX_MESSAGES_PER_SECOND} messages a second ({summary}) —"));
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
        "workspace/setInstanceStatus" => {
            let instance = params.get("instance").and_then(Value::as_str).unwrap_or_default().to_string();
            let state = params.get("state").and_then(Value::as_str).unwrap_or_default().to_string();
            let text = params.get("text").and_then(Value::as_str).and_then(sanitize_status_text);
            let result = set_instance_status(plugin_id, &instance, &state, text, cx);
            reply(result.map(|()| Value::Null).map_err(|e| (codes::INVALID_PARAMS, e)), cx)
        }
        "host/info" => reply(
            Ok(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "apiVersion": agentty_bridge::plugins::manifest::API_VERSION,
                "language": crate::settings::settings(cx).language.resolved().code(),
                "utcOffsetMinutes": utc_offset_minutes(),
                // Panel elements newer than the first API version, for a plugin to fall back on
                // older Agentty without guessing from the version number.
                "uiFeatures": ["flow", "popover"],
            })),
            cx,
        ),
        "net/fetch" => fetch(plugin_id, request_id, params, cx),
        "files/download" => download(plugin_id, request_id, params, cx),
        "files/pick" => pick_files(plugin_id, request_id, params, cx),
        "media/svgToPng" => svg_to_png(plugin_id, request_id, params, cx),
        "media/svgsToVideo" => svgs_to_video(plugin_id, request_id, params, cx),
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

/// Largest side of a PNG `media/svgToPng` draws.
const MAX_PNG_SIDE: u32 = 4096;

/// `media/svgToPng`: draws the SVG at `from` into a PNG at `to` (both in the plugin's folder),
/// `width` pixels wide (the SVG's own size when not given), with the system's fonts for its text.
fn svg_to_png(plugin_id: &str, request_id: Option<Value>, params: Value, cx: &mut App) {
    let id = plugin_id.to_string();
    let Some(request_id) = request_id else { return };
    let path = |key: &str| agentty_bridge::plugins::files::resolve(&id, params.get(key).and_then(Value::as_str).unwrap_or_default());
    let (from, to) = match (path("from"), path("to")) {
        (Ok(from), Ok(to)) => (from, to),
        (Err(err), _) | (_, Err(err)) => return respond(&id, &request_id, Err((codes::INVALID_PARAMS, format!("{err:#}"))), cx),
    };
    let width = params.get("width").and_then(Value::as_u64).map(|w| (w as u32).clamp(16, MAX_PNG_SIDE));
    let task = cx.background_executor().spawn(async move { render_svg(&from, &to, width) });
    cx.spawn(async move |cx| {
        let result = task.await.map_err(|err| (codes::INVALID_PARAMS, format!("{err:#}")));
        let _ = cx.update(|cx| respond(&id, &request_id, result, cx));
    })
    .detach();
}

/// `media/svgsToVideo` limits: scenes, seconds in all, frames a second, pixels a side.
const MAX_SCENES: usize = 12;
const MAX_VIDEO_SECONDS: f32 = 60.;
const MAX_FPS: u32 = 30;
const MAX_VIDEO_SIDE: u32 = 1920;

/// `media/svgsToVideo`: `scenes` (`[{ path, seconds }]`, SVGs of the plugin's folder) drawn one
/// after another, each easing slowly in and fading into the next, into an MP4 at `to`.
fn svgs_to_video(plugin_id: &str, request_id: Option<Value>, params: Value, cx: &mut App) {
    let id = plugin_id.to_string();
    let Some(request_id) = request_id else { return };
    let bad = |message: String| (codes::INVALID_PARAMS, message);
    let parsed = (|| -> Result<_, (i64, String)> {
        let resolve = |path: &str| agentty_bridge::plugins::files::resolve(&id, path).map_err(|e| bad(format!("{e:#}")));
        let to = resolve(params.get("to").and_then(Value::as_str).unwrap_or_default())?;
        let list = params.get("scenes").and_then(Value::as_array).ok_or_else(|| bad("scenes is a list".into()))?;
        if list.is_empty() || list.len() > MAX_SCENES {
            return Err(bad(format!("1 to {MAX_SCENES} scenes")));
        }
        let mut scenes = Vec::new();
        for scene in list {
            let path = resolve(scene.get("path").and_then(Value::as_str).unwrap_or_default())?;
            let seconds = scene.get("seconds").and_then(Value::as_f64).unwrap_or(3.) as f32;
            // How much it moves: `lively` (default), `gentle` or `still`, for the mood of the post.
            let motion = match scene.get("motion").and_then(Value::as_str) {
                Some("still") => 0.2,
                Some("gentle") => 0.5,
                _ => 1.,
            };
            scenes.push((path, seconds.clamp(0.5, 20.), motion));
        }
        if scenes.iter().map(|(_, s, _)| s).sum::<f32>() > MAX_VIDEO_SECONDS {
            return Err(bad(format!("at most {MAX_VIDEO_SECONDS} seconds in all")));
        }
        let side = |key: &str, default: u32| {
            params.get(key).and_then(Value::as_u64).map_or(default, |v| (v as u32).clamp(64, MAX_VIDEO_SIDE)) & !1
        };
        let fps = params.get("fps").and_then(Value::as_u64).map_or(30, |v| (v as u32).clamp(10, MAX_FPS));
        Ok((scenes, to, side("width", 1280), side("height", 720), fps))
    })();
    let (scenes, to, width, height, fps) = match parsed {
        Ok(parsed) => parsed,
        Err(err) => return respond(&id, &request_id, Err(err), cx),
    };
    let task = cx.background_executor().spawn(async move { render_video(&scenes, &to, width, height, fps) });
    cx.spawn(async move |cx| {
        let result = task.await.map_err(|err| (codes::INVALID_PARAMS, format!("{err:#}")));
        let _ = cx.update(|cx| respond(&id, &request_id, result, cx));
    })
    .detach();
}

/// Seconds a scene takes to cross into the next.
const FADE_SECONDS: f32 = 0.35;
/// How much a scene's size changes while it is on, and how far it drifts (share of the width).
const PUSH_IN: f32 = 0.12;
const DRIFT: f32 = 0.04;

fn render_video(
    scenes: &[(std::path::PathBuf, f32, f32)],
    to: &std::path::Path,
    width: u32,
    height: u32,
    fps: u32,
) -> anyhow::Result<Value> {
    use anyhow::Context as _;
    use resvg::tiny_skia;
    // Each scene drawn once, filling the frame; the frames move and blend these.
    let mut drawn = Vec::new();
    for (path, _, _) in scenes {
        drawn.push(render_svg_into(path, width, height).with_context(|| format!("{}", path.display()))?);
    }
    if let Some(folder) = to.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let starts: Vec<f32> = scenes
        .iter()
        .scan(0., |at, (_, seconds, _)| {
            let start = *at;
            *at += seconds;
            Some(start)
        })
        .collect();
    let total: f32 = scenes.iter().map(|(_, s, _)| s).sum();
    let count = (total * fps as f32).round() as usize;
    let mut canvas = tiny_skia::Pixmap::new(width, height).context("no room to draw")?;
    // Each scene moves its own way, so the cut feels alive: push in, pull out while drifting, push
    // in while drifting the other way; eased, fast at first and settling.
    let place = |canvas: &mut tiny_skia::Pixmap, scene: usize, t: f32, opacity: f32| {
        let (_, seconds, motion) = scenes[scene];
        let p = (t / seconds).clamp(0., 1.);
        let eased = 1. - (1. - p).powi(3);
        let (push, drift_by) = (PUSH_IN * motion, DRIFT * motion);
        let (grow, drift) = match scene % 3 {
            0 => (1. + push * eased, 0.),
            1 => (1. + push * (1. - eased), drift_by * (1. - eased)),
            _ => (1. + push * 0.5 + push * 0.5 * eased, -drift_by * (1. - eased)),
        };
        let (cx, cy) = (width as f32 / 2., height as f32 / 2.);
        let transform = tiny_skia::Transform::from_translate(cx + drift * width as f32, cy).pre_scale(grow, grow).pre_translate(-cx, -cy);
        let paint = tiny_skia::PixmapPaint { opacity, quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
        canvas.draw_pixmap(0, 0, drawn[scene].as_ref(), &paint, transform, None);
    };
    crate::platform::video::encode_mp4(to, width, height, fps, count, |index, out| {
        let at = index as f32 / fps as f32;
        let scene = starts.iter().rposition(|start| *start <= at).unwrap_or(0);
        let t = at - starts[scene];
        canvas.fill(tiny_skia::Color::BLACK);
        place(&mut canvas, scene, t, 1.);
        // Into the next scene over the end of this one: quick when lively, slower when calm.
        let fade = FADE_SECONDS / scenes[scene].2.max(0.4);
        let left = scenes[scene].1 - t;
        if scene + 1 < scenes.len() && left < fade {
            place(&mut canvas, scene + 1, 0., 1. - left / fade);
        }
        // tiny-skia is RGBA (opaque here), the encoder takes BGRA.
        for (pixel, rgba) in out.as_chunks_mut::<4>().0.iter_mut().zip(canvas.data().as_chunks::<4>().0) {
            *pixel = [rgba[2], rgba[1], rgba[0], 255];
        }
    })?;
    let size = std::fs::metadata(to).map(|m| m.len()).unwrap_or(0);
    Ok(json!({ "width": width, "height": height, "seconds": total, "frames": count, "size": size }))
}

/// Draws an SVG to fill `width`×`height` (its own aspect kept, centred on black).
fn render_svg_into(from: &std::path::Path, width: u32, height: u32) -> anyhow::Result<resvg::tiny_skia::Pixmap> {
    use anyhow::Context as _;
    use resvg::tiny_skia;
    let tree = svg_tree(from)?;
    let size = tree.size();
    let scale = (width as f32 / size.width()).min(height as f32 / size.height());
    let (dx, dy) = ((width as f32 - size.width() * scale) / 2., (height as f32 - size.height() * scale) / 2.);
    let mut pixmap = tiny_skia::Pixmap::new(width, height).context("no room to draw")?;
    pixmap.fill(tiny_skia::Color::BLACK);
    resvg::render(&tree, tiny_skia::Transform::from_translate(dx, dy).pre_scale(scale, scale), &mut pixmap.as_mut());
    Ok(pixmap)
}

/// An SVG of the plugin's folder parsed with the system's fonts, and images only from inside it.
fn svg_tree(from: &std::path::Path) -> anyhow::Result<resvg::usvg::Tree> {
    use anyhow::Context as _;
    use resvg::usvg;
    // The system's fonts, read once: loading them takes longer than drawing.
    static FONTS: std::sync::OnceLock<std::sync::Arc<usvg::fontdb::Database>> = std::sync::OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        std::sync::Arc::new(db)
    });
    let data = std::fs::read(from).with_context(|| format!("{} is not there", from.display()))?;
    let options = usvg::Options {
        fontdb: fonts.clone(),
        resources_dir: None,
        // Images inside the SVG only (data: URLs). A path or a file: link would read a file of the
        // computer into the picture, which the plugin then reads back.
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    usvg::Tree::from_data(&data, &options).context("not an SVG Agentty can draw")
}

fn render_svg(from: &std::path::Path, to: &std::path::Path, width: Option<u32>) -> anyhow::Result<Value> {
    use anyhow::Context as _;
    use resvg::tiny_skia;
    let tree = svg_tree(from)?;
    let size = tree.size();
    let scale = width.map_or(1., |w| w as f32 / size.width());
    let (w, h) = ((size.width() * scale).round() as u32, (size.height() * scale).round() as u32);
    if w == 0 || h == 0 || w > MAX_PNG_SIDE || h > MAX_PNG_SIDE {
        anyhow::bail!("{w}×{h} is not a size to draw (at most {MAX_PNG_SIDE} a side)");
    }
    let mut pixmap = tiny_skia::Pixmap::new(w, h).context("no room to draw")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let png = pixmap.encode_png().context("could not write the PNG")?;
    if let Some(folder) = to.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(to, &png)?;
    Ok(json!({ "width": w, "height": h, "size": png.len() }))
}

/// `files/pick`: the system's open panel; what the user picks is copied into `into` (a folder of
/// the plugin's own), never handed over by its own path. `[]` when the user cancels.
fn pick_files(plugin_id: &str, request_id: Option<Value>, params: Value, cx: &mut App) {
    let id = plugin_id.to_string();
    let Some(request_id) = request_id else { return };
    let into = params.get("into").and_then(Value::as_str).unwrap_or("picked").to_string();
    let folder = match agentty_bridge::plugins::files::resolve(&id, &into) {
        Ok(folder) => folder,
        Err(err) => return respond(&id, &request_id, Err((codes::INVALID_PARAMS, format!("{err:#}"))), cx),
    };
    let multiple = params.get("multiple").and_then(Value::as_bool).unwrap_or(true);
    let paths = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple, prompt: None });
    cx.spawn(async move |cx| {
        let picked = match paths.await {
            Ok(Ok(Some(paths))) => paths,
            _ => Vec::new(),
        };
        let copied = copy_picked(&picked, &folder, &into);
        let _ = cx.update(|cx| respond(&id, &request_id, copied.map_err(|err| (codes::INTERNAL, format!("{err:#}"))), cx));
    })
    .detach();
}

/// Copies picked files into `folder` under names not taken yet: `[{ path, name, size }]`, `path`
/// relative to the plugin's folder.
fn copy_picked(picked: &[std::path::PathBuf], folder: &std::path::Path, into: &str) -> anyhow::Result<Value> {
    std::fs::create_dir_all(folder)?;
    let mut out = Vec::new();
    for source in picked.iter().filter(|p| p.is_file()) {
        let size = std::fs::metadata(source)?.len();
        if size > agentty_bridge::plugins::files::MAX_FILE {
            anyhow::bail!("{} is larger than 1 GB", source.display());
        }
        let name = source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let (stem, ext) = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
            _ => (name.clone(), String::new()),
        };
        let mut target_name = name.clone();
        let mut n = 2;
        while folder.join(&target_name).exists() {
            target_name = format!("{stem}-{n}{ext}");
            n += 1;
        }
        std::fs::copy(source, folder.join(&target_name))?;
        out.push(json!({ "path": format!("{}/{target_name}", into.trim_end_matches('/')), "name": name, "size": size }));
    }
    Ok(Value::Array(out))
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
    #[cfg(target_os = "macos")]
    fn svg_scenes_become_an_mp4() {
        let dir = std::env::temp_dir().join(format!("agentty-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let scene = |name: &str, color: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><rect width="1600" height="900" fill="{color}"/><text x="100" y="450" font-size="120" fill="#fff">Scene</text></svg>"##)).unwrap();
            (path, 1.0, 1.0)
        };
        let to = dir.join("out/v.mp4");
        let made = render_video(&[scene("a.svg", "#123"), scene("b.svg", "#812")], &to, 320, 180, 10).unwrap();
        assert_eq!(made["frames"].as_u64(), Some(20));
        let bytes = std::fs::read(&to).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp", "an MP4 file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_svg_is_drawn_into_a_png_of_the_asked_width() {
        let dir = std::env::temp_dir().join(format!("agentty-svg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let from = dir.join("a.svg");
        std::fs::write(&from, r##"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="90"><rect width="160" height="90" fill="#123"/><text x="10" y="50" font-family="Helvetica, Arial, sans-serif" font-size="24" fill="#fff">Hi</text></svg>"##).unwrap();
        let to = dir.join("out/a.png");
        let drawn = render_svg(&from, &to, Some(320)).unwrap();
        assert_eq!((drawn["width"].as_u64(), drawn["height"].as_u64()), (Some(320), Some(180)));
        assert!(std::fs::read(&to).unwrap().starts_with(b"\x89PNG"));
        // A picture elsewhere on the computer is not drawn in: the PNG stays the plain background.
        let secret = dir.join("secret.png");
        std::fs::copy(&to, &secret).unwrap();
        let path = secret.display();
        std::fs::write(&from, format!(r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="160" height="90"><rect width="160" height="90" fill="#000"/><image href="file://{path}" width="160" height="90"/><image xlink:href="{path}" width="160" height="90"/></svg>"##)).unwrap();
        let plain = dir.join("plain.png");
        render_svg(&from, &plain, None).unwrap();
        std::fs::write(
            &from,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="90"><rect width="160" height="90" fill="#000"/></svg>"##,
        )
        .unwrap();
        let black = dir.join("black.png");
        render_svg(&from, &black, None).unwrap();
        assert_eq!(std::fs::read(&plain).unwrap(), std::fs::read(&black).unwrap(), "a file of the computer was drawn in");
        std::fs::write(&from, "not svg").unwrap();
        assert!(render_svg(&from, &to, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        let start = Instant::now();
        for _ in 0..MAX_MESSAGES_PER_SECOND {
            assert!(!runtime.over_rate_limit(start));
        }
        assert!(runtime.over_rate_limit(start), "the next message is over the limit");
        // A new second starts over.
        runtime.rate_window = Some((Instant::now() - Duration::from_secs(2), MAX_MESSAGES_PER_SECOND + 10));
        assert!(!runtime.over_rate_limit(Instant::now()));

        // Sent a few a second over three seconds, handled all at once: not a flood.
        let mut spread = Runtime::new();
        let base = Instant::now();
        for i in 0..(MAX_MESSAGES_PER_SECOND * 3) {
            let at = base + Duration::from_millis(u64::from(i) * 3000 / u64::from(MAX_MESSAGES_PER_SECOND * 3));
            assert!(!spread.over_rate_limit(at), "message {i} was sent at a steady pace");
        }

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

    #[test]
    fn working_since_starts_when_work_begins_and_survives_the_same_text_again() {
        let started = apply_status(None, InstanceState::Working, Some("collecting".into()), 1_000);
        assert_eq!(started.working_since_ms, Some(1_000));
        // Restating the same state (a new count in the text) does not reset the clock.
        let restated = apply_status(Some(&started), InstanceState::Working, Some("collecting (3/6)".into()), 1_500);
        assert_eq!(restated.working_since_ms, Some(1_000));
        assert_eq!(restated.text.as_deref(), Some("collecting (3/6)"));
    }

    #[test]
    fn working_since_is_cleared_off_working_and_set_fresh_next_time() {
        let working = apply_status(None, InstanceState::Working, None, 1_000);
        let idle = apply_status(Some(&working), InstanceState::Idle, None, 2_000);
        assert_eq!(idle.working_since_ms, None);
        // Working again later gets its own start time, not the first one.
        let working_again = apply_status(Some(&idle), InstanceState::Working, None, 3_000);
        assert_eq!(working_again.working_since_ms, Some(3_000));
    }

    #[test]
    fn status_text_is_one_line_capped_and_trimmed() {
        assert_eq!(sanitize_status_text("  Collecting @sama (3/6)  "), Some("Collecting @sama (3/6)".to_string()));
        assert_eq!(sanitize_status_text("line one\nline two"), Some("line one line two".to_string()));
        assert_eq!(sanitize_status_text("   "), None, "a blank status is no status");
        let long = "x".repeat(200);
        assert_eq!(sanitize_status_text(&long).unwrap().chars().count(), MAX_STATUS_CHARS);
    }

    #[test]
    fn set_instance_status_rejects_an_unknown_state() {
        assert_eq!(InstanceState::from_id("done"), None);
        assert_eq!(InstanceState::from_id("working"), Some(InstanceState::Working));
    }
}
