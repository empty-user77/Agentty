//! The remote page's server: listens where only Tailscale's `serve` proxy reaches it (a private
//! Unix socket, or `127.0.0.1` where that can't be used; see `conn`), never the network. Every
//! request passes, in order:
//!
//! 0. **The secret path**: `serve` is pointed at `http://127.0.0.1:<port>/<secret>` and puts that
//!    secret in front of every path it forwards; a request without it did not come through
//!    Tailscale (something on this Mac connected directly) and is refused before anything else.
//!    Only Tailscale's configuration holds the secret, which only administrators can read.
//! 1. **Host**: the name it was sent to must be this machine's tailnet name (or, for a developer
//!    build, `127.0.0.1`), so a web page elsewhere can't reach it by rebinding DNS.
//! 2. **Tailscale identity**: `serve` adds the sender's login (`Tailscale-User-Login`) and drops any
//!    the sender wrote itself; only the login that owns this machine is let in. Tagged devices and
//!    other people on a shared tailnet get nothing.
//! 3. **The web password**, then the session cookie it opens (`auth`).
//! 4. For anything that changes something: same-origin only (`Origin`, `Sec-Fetch-Site`), a JSON
//!    body and the page's own header, which a form or a cross-site request can't send.
//!
//! The UI thread and this server share only the `Hub`: the app copies sessions and screens in and
//! takes the page's input out, so the server never touches a terminal itself.

use super::auth::{self, Lockout, Sessions};
use super::conn::{Conn, Endpoint, Listener};
use super::http::{self, HttpError, Request, Response};
use super::snapshot::{screen_update, PluginInfo, Screen, SessionInfo, WorkspaceInfo};
use crate::launch::PaneKind;
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// Open connections at once; past this a request is answered 503.
const MAX_CONNECTIONS: usize = 32;
/// Live streams at once, in all and per session: one page holds one, so a few devices fit, and
/// streams can never take every connection.
const MAX_STREAMS: usize = 16;
const MAX_STREAMS_PER_SESSION: usize = 4;
/// A whole response is written within this, however slowly the other end reads.
const RESPONSE_DEADLINE: Duration = Duration::from_secs(20);
/// A live stream ends after this and the page reconnects, which checks its session again.
const STREAM_LIFETIME: Duration = Duration::from_secs(30 * 60);
const KEEPALIVE: Duration = Duration::from_secs(15);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// The whole request, head and body, arrives within this or the connection closes.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
/// Voice clips transcribed at once: each keeps a CPU busy for seconds, so more wait their turn
/// (answered 429) rather than slowing every one down.
const MAX_TRANSCRIPTIONS: usize = 2;
/// How long a refused request's unread body is drained so its answer arrives (`linger`).
const LINGER: Duration = Duration::from_secs(3);
/// Longest text one input may carry (a prompt pasted from a phone).
pub const MAX_INPUT_CHARS: usize = 16_000;
const LOG_LEN: usize = 50;

const INDEX_HTML: &str = include_str!("../../assets/remote/index.html");
const APP_JS: &str = include_str!("../../assets/remote/app.js");
const APP_CSS: &str = include_str!("../../assets/remote/app.css");
const ICON_SVG: &str = include_str!("../../assets/remote/icon.svg");
const MANIFEST: &str = include_str!("../../assets/remote/manifest.webmanifest");

/// Who and where requests may come from.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The Tailscale login allowed in. `None` only in a developer build's local test mode, where no
    /// proxy adds one.
    pub owner: Option<String>,
    /// Accepted `Host` values (`machine.tailnet.ts.net:8743`).
    pub hosts: Vec<String>,
    /// Served over HTTPS (through `tailscale serve`): the cookie is `Secure` and `__Host-`.
    pub https: bool,
    /// The path prefix `serve` adds to everything it forwards (check 0). `None` on the private
    /// Unix socket, which only Tailscale opens, and in the local test mode.
    pub secret: Option<String>,
}

impl Config {
    fn origin_for(&self, host: &str) -> String {
        format!("{}://{host}", if self.https { "https" } else { "http" })
    }

    fn cookie_name(&self) -> &'static str {
        if self.https {
            "__Host-agentty"
        } else {
            "agentty_dev"
        }
    }
}

/// Something the page asked the app to do in a terminal.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// A key with modifiers, by its GPUI name (`enter`, `up`, `c` with `ctrl`).
    Key { pane: u64, key: String, ctrl: bool, alt: bool, shift: bool },
    /// Typed text, as typed.
    Text { pane: u64, text: String },
    /// A prompt: pasted, then Enter.
    Prompt { pane: u64, text: String },
    /// A message written in a chat, for its lead (`pane`): sent as the app's own chat sends it,
    /// with the lead's instructions in front the first time.
    Chat { pane: u64, text: String },
    /// The page shows this terminal at this size: the terminal takes it while watched.
    Resize { pane: u64, cols: u16, rows: u16 },
    /// No page shows this terminal any more: back to the pane's own size.
    Release { pane: u64 },
    /// A sleeping workspace brought back, its saved tabs started again.
    Wake { workspace: u64 },
    /// A new tab in a workspace, with a shell or an agent, where the workspace is.
    NewTab { workspace: u64, kind: PaneKind },
}

impl Command {
    /// The terminal it is for; `None` for what is done to a whole workspace.
    pub fn pane(&self) -> Option<u64> {
        match self {
            Command::Key { pane, .. }
            | Command::Text { pane, .. }
            | Command::Prompt { pane, .. }
            | Command::Chat { pane, .. }
            | Command::Resize { pane, .. }
            | Command::Release { pane } => Some(*pane),
            Command::Wake { .. } | Command::NewTab { .. } => None,
        }
    }
}

/// An entry of the access log on the settings page.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub at_ms: u64,
    /// i18n key: `remote.log.signed_in`, `remote.log.wrong_password`, `remote.log.locked`,
    /// `remote.log.signed_out`, `remote.log.refused_login`.
    pub what: &'static str,
    pub login: String,
    pub device: String,
}

/// Told to the user on the desktop as it happens.
#[derive(Debug, Clone, PartialEq)]
pub enum Alert {
    SignedIn { device: String },
    Locked { minutes: u64 },
}

#[derive(Default)]
struct AuthState {
    password: Option<String>,
    /// Bumped by a new password and by "Sign out everywhere": a sign-in that was being checked
    /// across one of those does not open a session.
    generation: u64,
    sessions: Sessions,
    lockout: Lockout,
}

#[derive(Default)]
struct View {
    version: u64,
    workspaces: Vec<WorkspaceInfo>,
    sessions: Vec<SessionInfo>,
    sessions_version: u64,
    plugins: Vec<PluginInfo>,
    screens: HashMap<u64, (u64, Screen)>,
}

pub struct Hub {
    /// Where it listens: what `tailscale serve` is pointed at.
    pub endpoint: Endpoint,
    config: RwLock<Config>,
    auth: Mutex<AuthState>,
    /// One password check at a time: each costs ~0.3 s of CPU on purpose.
    verifying: Mutex<()>,
    view: Mutex<View>,
    changed: Condvar,
    inbox: Mutex<Vec<Command>>,
    watchers: Mutex<HashMap<u64, usize>>,
    open: AtomicUsize,
    stop: AtomicBool,
    /// Holds its port but lets nobody in (`pause`).
    paused: AtomicBool,
    log: Mutex<VecDeque<LogEntry>>,
    alerts: Mutex<Vec<Alert>>,
    /// Wakes the app the moment input arrives, instead of at its next round.
    waker: Mutex<Option<futures::channel::mpsc::UnboundedSender<()>>>,
    /// A wake is on its way: more input before the app's round needs no second one.
    wake_pending: AtomicBool,
    /// Tailnet devices looked up for the sign-in page: address → (when, what it is).
    devices_seen: Mutex<HashMap<String, (Instant, Option<super::tailscale::Device>)>>,
    /// When `tailscale whois` last ran for a new address: at most one a second.
    last_whois: Mutex<Option<Instant>>,
    /// Live streams per session token.
    streams: Mutex<HashMap<String, usize>>,
    /// When the page last opened tabs, for `MAX_NEW_TABS`.
    new_tabs: Mutex<VecDeque<Instant>>,
    /// Voice-to-text settings, or `None` when voice is off (no model set up). Read on every
    /// `/api/voice`.
    voice: RwLock<Option<VoiceConfig>>,
    /// Voice clips being transcribed now, at most `MAX_TRANSCRIPTIONS`.
    transcribing: AtomicUsize,
}

/// What `POST /api/voice` transcribes with: a model (already installed) and an optional language
/// hint. Set from the app's settings; `None` in the hub means voice is off.
#[derive(Clone)]
pub struct VoiceConfig {
    pub model: &'static agentty_bridge::voice::Model,
    pub language: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBody {
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceBody {
    workspace: u64,
    action: String,
    #[serde(default)]
    tool: String,
}

/// Tabs the page may open in a minute: a few by hand, never a loop starting hundreds of shells.
const MAX_NEW_TABS: usize = 10;
const NEW_TAB_WINDOW: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputBody {
    pane: u64,
    kind: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    alt: bool,
    #[serde(default)]
    shift: bool,
    #[serde(default)]
    text: String,
    #[serde(default)]
    cols: u16,
    #[serde(default)]
    rows: u16,
}

/// The sizes a page may give a terminal: a phone held upright up to a wide desktop window.
const COLS: std::ops::RangeInclusive<u16> = 20..=400;
const ROWS: std::ops::RangeInclusive<u16> = 5..=200;

/// Key names the page may send: what `terminal::keys` turns into bytes, and single characters.
fn allowed_key(key: &str) -> bool {
    const NAMED: &[&str] = &[
        "enter",
        "tab",
        "escape",
        "backspace",
        "delete",
        "up",
        "down",
        "left",
        "right",
        "home",
        "end",
        "pageup",
        "pagedown",
        "space",
        "insert",
        "f1",
        "f2",
        "f3",
        "f4",
        "f5",
        "f6",
        "f7",
        "f8",
        "f9",
        "f10",
        "f11",
        "f12",
    ];
    NAMED.contains(&key) || (key.chars().count() == 1 && key.chars().all(|c| c.is_ascii_graphic()))
}

impl Hub {
    /// Starts serving on `listener`.
    pub fn start(config: Config, password: Option<String>, listener: Listener) -> std::io::Result<Arc<Hub>> {
        let endpoint = listener.endpoint()?;
        let hub = Arc::new(Hub {
            endpoint,
            config: RwLock::new(config),
            auth: Mutex::new(AuthState { password, ..Default::default() }),
            verifying: Mutex::new(()),
            view: Mutex::new(View::default()),
            changed: Condvar::new(),
            inbox: Mutex::new(Vec::new()),
            watchers: Mutex::new(HashMap::new()),
            open: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            log: Mutex::new(VecDeque::new()),
            alerts: Mutex::new(Vec::new()),
            waker: Mutex::new(None),
            wake_pending: AtomicBool::new(false),
            devices_seen: Mutex::new(HashMap::new()),
            last_whois: Mutex::new(None),
            streams: Mutex::new(HashMap::new()),
            new_tabs: Mutex::new(VecDeque::new()),
            voice: RwLock::new(None),
            transcribing: AtomicUsize::new(0),
        });
        let accept = hub.clone();
        std::thread::Builder::new().name("agentty-remote".into()).spawn(move || accept.accept_loop(listener))?;
        Ok(hub)
    }

    fn accept_loop(self: Arc<Self>, listener: Listener) {
        while !self.stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok(Some(mut conn)) => {
                    if self.paused.load(Ordering::SeqCst) {
                        continue;
                    }
                    if self.open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        self.open.fetch_sub(1, Ordering::SeqCst);
                        let _ = conn.set_nonblocking(false);
                        let busy = Response::new(503, "text/plain", "busy");
                        let _ = conn.write_by(http::response_head(&busy).as_bytes(), Instant::now() + Duration::from_secs(1));
                        continue;
                    }
                    // Counted back down however the connection ends, a panic included (a slot
                    // that leaked would never come back), or with the closure if no thread starts.
                    let slot = OpenSlot(self.clone());
                    let _ = std::thread::Builder::new().name("agentty-remote-conn".into()).spawn(move || {
                        slot.0.serve_connection(conn);
                        drop(slot);
                    });
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(40)),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
        // The listener, and a Unix socket's file, go with this thread.
    }

    /// Stops serving but keeps the listener, closing every connection at once: while a Tailscale
    /// entry may still point at this port (Tailscale disconnected can't take it down), nothing
    /// else on the Mac can take the port over and receive what the entry forwards.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        self.auth.lock().unwrap_or_else(|e| e.into_inner()).sessions.clear();
        self.changed.notify_all();
    }

    /// Stops serving: the listener closes and every live stream ends.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.auth.lock().unwrap_or_else(|e| e.into_inner()).sessions.clear();
        self.changed.notify_all();
    }

    pub fn set_config(&self, config: Config) {
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = config;
    }

    /// A new password (or none): every session ends.
    /// Set which model (and optional language) `/api/voice` uses, or `None` to turn voice off.
    pub fn set_voice(&self, config: Option<VoiceConfig>) {
        *self.voice.write().unwrap_or_else(|e| e.into_inner()) = config;
    }

    /// Whether the page should offer the microphone: voice is configured and its model is installed.
    fn voice_available(&self) -> bool {
        self.voice.read().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|c| agentty_bridge::voice::model_present(c.model))
    }

    pub fn set_password(&self, password: Option<String>) {
        let mut auth = self.auth.lock().unwrap_or_else(|e| e.into_inner());
        auth.password = password;
        auth.generation += 1;
        auth.sessions.clear();
        auth.lockout.succeed();
        drop(auth);
        self.changed.notify_all();
    }

    /// How long sign-in stays locked after wrong passwords, if it is.
    pub fn locked_for(&self) -> Option<Duration> {
        self.auth.lock().unwrap_or_else(|e| e.into_inner()).lockout.remaining(SystemTime::now())
    }

    /// Lifts the lock from the Mac: someone may have locked the owner out on purpose, and the
    /// owner, at the Mac, can undo that.
    pub fn unlock(&self) {
        self.auth.lock().unwrap_or_else(|e| e.into_inner()).lockout.succeed();
    }

    pub fn sign_out_everywhere(&self) {
        let mut auth = self.auth.lock().unwrap_or_else(|e| e.into_inner());
        auth.generation += 1;
        auth.sessions.clear();
        drop(auth);
        self.changed.notify_all();
    }

    /// Signed-in devices: (device, last used).
    pub fn devices(&self) -> Vec<(String, Duration)> {
        let now = SystemTime::now();
        let mut auth = self.auth.lock().unwrap_or_else(|e| e.into_inner());
        auth.sessions.list(now).into_iter().map(|s| (s.device, now.duration_since(s.last_seen).unwrap_or_default())).collect()
    }

    pub fn log(&self) -> Vec<LogEntry> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
    }

    pub fn take_alerts(&self) -> Vec<Alert> {
        std::mem::take(&mut *self.alerts.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Where to signal that input arrived (the app's round loop).
    pub fn set_waker(&self, waker: futures::channel::mpsc::UnboundedSender<()>) {
        *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(waker);
    }

    pub fn take_commands(&self) -> Vec<Command> {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        self.wake_pending.store(false, Ordering::SeqCst);
        std::mem::take(&mut *inbox)
    }

    /// Panes some page is looking at: only their screens are copied.
    pub fn watched(&self) -> Vec<u64> {
        self.watchers.lock().unwrap_or_else(|e| e.into_inner()).keys().copied().filter(|pane| *pane != 0).collect()
    }

    /// Open streams, for the settings page.
    pub fn viewers(&self) -> usize {
        self.watchers.lock().unwrap_or_else(|e| e.into_inner()).get(&0).copied().unwrap_or(0)
    }

    /// The workspaces (sidebar order, with tabs and panes) and every terminal in them.
    pub fn publish(&self, workspaces: Vec<WorkspaceInfo>, sessions: Vec<SessionInfo>, plugins: Vec<PluginInfo>) {
        let mut view = self.view.lock().unwrap_or_else(|e| e.into_inner());
        if view.sessions != sessions || view.workspaces != workspaces || view.plugins != plugins {
            view.workspaces = workspaces;
            view.sessions = sessions;
            view.plugins = plugins;
            view.version += 1;
            view.sessions_version = view.version;
            let watched: Vec<u64> = view.sessions.iter().map(|s| s.pane).collect();
            view.screens.retain(|pane, _| watched.contains(pane));
            drop(view);
            self.changed.notify_all();
        }
    }

    pub fn publish_screen(&self, pane: u64, screen: Screen) {
        let mut view = self.view.lock().unwrap_or_else(|e| e.into_inner());
        if view.screens.get(&pane).is_some_and(|(_, s)| *s == screen) {
            return;
        }
        view.version += 1;
        let version = view.version;
        view.screens.insert(pane, (version, screen));
        drop(view);
        self.changed.notify_all();
    }

    fn record(&self, what: &'static str, login: &str, device: &str) {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let now = crate::ui::now_ms();
        let (login, device) = (clean_text(login), clean_text(device));
        // The same refusal again and again (a device of someone else's polling the page) counts
        // once a minute, so it can't push the owner's own entries out of the log.
        if what == "remote.log.refused_login"
            && log.iter().take(LOG_LEN).any(|e| e.what == what && e.login == login && now.saturating_sub(e.at_ms) < 60_000)
        {
            return;
        }
        log.push_front(LogEntry { at_ms: now, what, login, device });
        log.truncate(LOG_LEN);
    }

    fn serve_connection(&self, mut stream: Conn) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let config = self.config.read().unwrap_or_else(|e| e.into_inner()).clone();
        let deadline = Instant::now() + REQUEST_DEADLINE;
        let request = match http::read_request(&mut stream, deadline, |head| {
            if config.secret.as_deref().is_some_and(|secret| strip_secret(&head.path, secret).is_none()) {
                return Err(404);
            }
            let (host, login) = self.admitted(head, &config, true).map_err(|refused| refused.status)?;
            if http::is_voice(&head.path) {
                self.voice_admitted(head, &config, &host, &login)?;
            }
            Ok(())
        }) {
            Ok(request) => request,
            Err(HttpError::Gone) => return,
            Err(error) => {
                let status = match error {
                    HttpError::HeadTooLarge => 431,
                    HttpError::BodyTooLarge => 413,
                    HttpError::Unsupported => 501,
                    HttpError::Refused(status) => status,
                    _ => 400,
                };
                send(&mut stream, &Response::new(status, "text/plain", http::reason(status)), false);
                if matches!(error, HttpError::Refused(_) | HttpError::BodyTooLarge) {
                    linger(&mut stream);
                }
                return;
            }
        };
        let mut request = request;
        if let Some(path) = config.secret.as_deref().and_then(|secret| strip_secret(&request.path, secret)) {
            request.path = path;
        }
        let head_only = request.method == "HEAD";
        match self.route(&request) {
            Routed::Response(response) => send(&mut stream, &response, head_only),
            Routed::Events { token, login, pane } => {
                // A slot, or 429: one page (or a page reconnecting in a loop) can't take them all.
                let Some(_slot) = self.stream_slot(&token) else {
                    send(&mut stream, &Response::json(429, &json!({ "error": "too many streams" })), false);
                    return;
                };
                self.stream_events(stream, &token, &login, pane);
            }
        }
    }

    /// Reserves a live stream for `token`, given back when the guard drops.
    fn stream_slot(&self, token: &str) -> Option<StreamSlot<'_>> {
        let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        let total: usize = streams.values().sum();
        let mine = streams.get(token).copied().unwrap_or(0);
        if total >= MAX_STREAMS || mine >= MAX_STREAMS_PER_SESSION {
            return None;
        }
        streams.insert(token.to_string(), mine + 1);
        Some(StreamSlot { hub: self, token: token.to_string() })
    }

    /// Checks 1 and 2, from the head alone: the name it was sent to, and who Tailscale says sent
    /// it. Returns (host, login) or the answer to a refused request.
    fn admitted(&self, request: &Request, config: &Config, log: bool) -> Result<(String, String), Response> {
        let Some(host) = request.header("host").filter(|h| config.hosts.iter().any(|ok| ok.eq_ignore_ascii_case(h))) else {
            return Err(Response::new(421, "text/plain", "Misdirected Request"));
        };
        let login = match &config.owner {
            Some(owner) => match request.header("tailscale-user-login") {
                Some(login) if login.eq_ignore_ascii_case(owner) => login.to_string(),
                other => {
                    if log {
                        self.record("remote.log.refused_login", other.unwrap_or("-"), request.header("user-agent").unwrap_or(""));
                    }
                    return Err(Response::new(403, "text/plain; charset=utf-8", "This Tailscale account may not use this Agentty."));
                }
            },
            None => "local".to_string(),
        };
        Ok((host.to_string(), login))
    }

    /// The voice upload's head, checked in full before its body (up to `VOICE_MAX_BODY`) is read:
    /// a POST from this page (check 4), a session of this login (check 3), and a transcription
    /// slot free. `route` checks it all again once the body is in.
    fn voice_admitted(&self, request: &Request, config: &Config, host: &str, login: &str) -> Result<(), u16> {
        if request.method != "POST" {
            return Err(405);
        }
        same_origin_post(request, &config.origin_for(host))?;
        if cross_site(request) {
            return Err(403);
        }
        let token = request.cookie(config.cookie_name()).unwrap_or("");
        if !self.auth.lock().unwrap_or_else(|e| e.into_inner()).sessions.check(token, login, SystemTime::now()) {
            return Err(401);
        }
        if self.transcribing.load(Ordering::SeqCst) >= MAX_TRANSCRIPTIONS {
            return Err(429);
        }
        Ok(())
    }

    /// Reserves one of `MAX_TRANSCRIPTIONS`, given back when the guard drops.
    fn transcribe_slot(&self) -> Option<TranscribeSlot<'_>> {
        self.transcribing
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < MAX_TRANSCRIPTIONS).then_some(n + 1))
            .ok()
            .map(|_| TranscribeSlot(self))
    }

    fn route(&self, request: &Request) -> Routed {
        let config = self.config.read().unwrap_or_else(|e| e.into_inner()).clone();
        // 1 and 2 already passed on the head; checked again against the current settings.
        let (host, login) = match self.admitted(request, &config, false) {
            Ok(found) => found,
            Err(response) => return Routed::Response(response),
        };
        let host = host.as_str();
        let path = request.path.as_str();
        let method = request.method.as_str();
        let get = method == "GET" || method == "HEAD";
        match (get, path) {
            (true, "/") => return Routed::Response(Response::fixed(200, "text/html; charset=utf-8", INDEX_HTML.as_bytes())),
            (true, "/app.js") => return Routed::Response(Response::fixed(200, "text/javascript; charset=utf-8", APP_JS.as_bytes())),
            (true, "/app.css") => return Routed::Response(Response::fixed(200, "text/css; charset=utf-8", APP_CSS.as_bytes())),
            (true, "/icon.svg") => return Routed::Response(Response::fixed(200, "image/svg+xml", ICON_SVG.as_bytes())),
            (true, "/manifest.webmanifest") => {
                return Routed::Response(Response::fixed(200, "application/manifest+json", MANIFEST.as_bytes()));
            }
            // The app's own terminal fonts, so the page draws text as the app does; the Nerd Font
            // symbols only load when a prompt uses them (`unicode-range` in app.css). They never
            // change within a version, so the browser may keep them.
            (true, "/fonts/mono.ttf" | "/fonts/mono-bold.ttf" | "/fonts/symbols.ttf") => {
                let font = match path {
                    "/fonts/mono.ttf" => crate::FONTS[0],
                    "/fonts/mono-bold.ttf" => crate::FONTS[1],
                    _ => crate::FONTS[4],
                };
                return Routed::Response(Response::fixed(200, "font/ttf", font).with("Cache-Control", "private, max-age=604800"));
            }
            _ => {}
        }
        if !path.starts_with("/api/") {
            return Routed::Response(Response::new(404, "text/plain", "Not Found"));
        }
        // 4. Changes come only from this page.
        if method == "POST" {
            if let Err(status) = same_origin_post(request, &config.origin_for(host)) {
                return Routed::Response(Response::json(status, &json!({ "error": "refused" })));
            }
        } else if !get {
            return Routed::Response(Response::new(405, "text/plain", "Method Not Allowed"));
        }
        if cross_site(request) {
            return Routed::Response(Response::json(403, &json!({ "error": "refused" })));
        }
        let token = request.cookie(config.cookie_name()).unwrap_or("").to_string();
        let now = SystemTime::now();
        match (method, path) {
            ("GET", "/api/me") => {
                let mut auth = self.auth.lock().unwrap_or_else(|e| e.into_inner());
                let signed_in = auth.sessions.check(&token, &login, now);
                let locked = auth.lockout.remaining(now).map(|d| d.as_secs() + 1);
                let ready = auth.password.is_some();
                drop(auth);
                // Shown on the sign-in page, never kept: which device this is (its name and system,
                // as Tailscale knows it) and which machine it is about to reach. Not its address or
                // account: this answer comes before the password.
                let device = if config.owner.is_some() { self.device_of(request) } else { None };
                let device = device.map(|d| json!({ "name": d.name, "os": d.os }));
                return Routed::Response(Response::json(
                    200,
                    &json!({ "signedIn": signed_in, "lockedFor": locked, "ready": ready, "device": device, "target": host }),
                ));
            }
            ("POST", "/api/login") => return Routed::Response(self.login(request, &login, &config, now)),
            _ => {}
        }
        // 3. Everything past here needs a session of this login.
        if !self.auth.lock().unwrap_or_else(|e| e.into_inner()).sessions.check(&token, &login, now) {
            return Routed::Response(Response::json(401, &json!({ "error": "signed out" })));
        }
        match (method, path) {
            ("POST", "/api/logout") => {
                self.auth.lock().unwrap_or_else(|e| e.into_inner()).sessions.remove(&token);
                self.record("remote.log.signed_out", &login, request.header("user-agent").unwrap_or(""));
                Routed::Response(Response::json(200, &json!({ "ok": true })).with("Set-Cookie", expired_cookie(&config)))
            }
            ("GET", "/api/sessions") => {
                let voice = self.voice_available();
                let view = self.view.lock().unwrap_or_else(|e| e.into_inner());
                Routed::Response(Response::json(200, &overview(&view, voice)))
            }
            ("GET", "/api/events") => {
                let pane = match request.query_param("pane") {
                    None => None,
                    Some(text) => match text.parse::<u64>() {
                        Ok(pane) if self.is_session(pane) => Some(pane),
                        _ => return Routed::Response(Response::json(404, &json!({ "error": "no such session" }))),
                    },
                };
                Routed::Events { token, login, pane }
            }
            ("POST", "/api/input") => Routed::Response(self.input(request)),
            ("POST", "/api/voice") => Routed::Response(self.voice(request)),
            ("POST", "/api/workspace") => Routed::Response(self.workspace_action(request)),
            _ => Routed::Response(Response::json(404, &json!({ "error": "not found" }))),
        }
    }

    /// The device a request came from: `tailscale serve` adds its tailnet address to
    /// `X-Forwarded-For` (after any the sender wrote, so the last entry is the one it vouches
    /// for), and `tailscale whois` says what that device is. Remembered for a few minutes so a
    /// page reloading does not run the tool each time; a new address is looked up at most once a
    /// second.
    fn device_of(&self, request: &Request) -> Option<super::tailscale::Device> {
        let ip = request.header("x-forwarded-for")?.rsplit(',').next()?.trim().to_string();
        ip.parse::<std::net::IpAddr>().ok()?;
        let cache = self.devices_seen.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, device)) = cache.get(&ip) {
            if at.elapsed() < Duration::from_secs(300) {
                return device.clone();
            }
        }
        drop(cache);
        {
            let mut last = self.last_whois.lock().unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
                return None;
            }
            *last = Some(Instant::now());
        }
        let device = super::tailscale::whois(&ip);
        let mut cache = self.devices_seen.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(ip, (Instant::now(), device.clone()));
        device
    }

    fn is_session(&self, pane: u64) -> bool {
        self.view.lock().unwrap_or_else(|e| e.into_inner()).sessions.iter().any(|s| s.pane == pane)
    }

    fn login(&self, request: &Request, login: &str, config: &Config, now: SystemTime) -> Response {
        let device = device_name(request.header("user-agent").unwrap_or(""));
        let Ok(body) = serde_json::from_slice::<LoginBody>(&request.body) else {
            return Response::json(400, &json!({ "error": "bad request" }));
        };
        let _one_at_a_time = self.verifying.lock().unwrap_or_else(|e| e.into_inner());
        let (stored, generation) = {
            let auth = self.auth.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(left) = auth.lockout.remaining(now) {
                return Response::json(429, &json!({ "error": "locked", "lockedFor": left.as_secs() + 1 }));
            }
            (auth.password.clone(), auth.generation)
        };
        let Some(stored) = stored else {
            return Response::json(409, &json!({ "error": "no password" }));
        };
        // Checked without holding the session table, which the live streams use.
        let right = auth::verify_password(&stored, &body.password);
        let mut auth = self.auth.lock().unwrap_or_else(|e| e.into_inner());
        if right && auth.generation != generation {
            return Response::json(401, &json!({ "error": "signed out" }));
        }
        if right {
            auth.lockout.succeed();
            let token = auth.sessions.create(login, &device, SystemTime::now());
            drop(auth);
            self.record("remote.log.signed_in", login, &device);
            self.alerts.lock().unwrap_or_else(|e| e.into_inner()).push(Alert::SignedIn { device });
            let cookie = format!(
                "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
                config.cookie_name(),
                auth::SESSION_MAX.as_secs(),
                if config.https { "; Secure" } else { "" }
            );
            return Response::json(200, &json!({ "ok": true })).with("Set-Cookie", cookie);
        }
        let locked = auth.lockout.fail(SystemTime::now());
        drop(auth);
        if cfg!(debug_assertions) {
            let shape = auth::shape(&body.password);
            eprintln!("remote: sign-in refused, password {shape}"); // audit: ok — length and kind only, never the value
        }
        self.record("remote.log.wrong_password", login, &device);
        match locked {
            Some(lock) => {
                self.record("remote.log.locked", login, &device);
                self.alerts.lock().unwrap_or_else(|e| e.into_inner()).push(Alert::Locked { minutes: lock.as_secs().div_ceil(60) });
                Response::json(429, &json!({ "error": "locked", "lockedFor": lock.as_secs() }))
            }
            None => Response::json(401, &json!({ "error": "wrong password" })),
        }
    }

    fn input(&self, request: &Request) -> Response {
        let Ok(body) = serde_json::from_slice::<InputBody>(&request.body) else {
            return Response::json(400, &json!({ "error": "bad request" }));
        };
        if !self.is_session(body.pane) {
            return Response::json(404, &json!({ "error": "no such session" }));
        }
        if body.text.chars().count() > MAX_INPUT_CHARS {
            return Response::json(413, &json!({ "error": "too long" }));
        }
        let command = match body.kind.as_str() {
            "key" if allowed_key(&body.key) => {
                Command::Key { pane: body.pane, key: body.key, ctrl: body.ctrl, alt: body.alt, shift: body.shift }
            }
            "text" if !body.text.is_empty() => Command::Text { pane: body.pane, text: body.text },
            "prompt" if !body.text.trim().is_empty() => Command::Prompt { pane: body.pane, text: body.text },
            "chat" if !body.text.trim().is_empty() => Command::Chat { pane: body.pane, text: body.text },
            "resize" if COLS.contains(&body.cols) && ROWS.contains(&body.rows) => {
                Command::Resize { pane: body.pane, cols: body.cols, rows: body.rows }
            }
            _ => return Response::json(400, &json!({ "error": "bad request" })),
        };
        // Held while queueing: a size is only taken while a page shows that terminal, and the
        // `Release` its last page sends on leaving can't come before it (see `watch`).
        let watchers = self.watchers.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(command, Command::Resize { .. }) && !watchers.contains_key(&body.pane) {
            return Response::json(409, &json!({ "error": "not shown" }));
        }
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        // A newer size for the same terminal replaces the one still waiting.
        if let Command::Resize { pane, .. } = command {
            if let Some(waiting) = inbox.iter_mut().find(|c| matches!(c, Command::Resize { pane: p, .. } if *p == pane)) {
                *waiting = command;
                return Response::json(200, &json!({ "ok": true }));
            }
        }
        // The app drains this several times a second; a flood waits its turn rather than grows.
        if inbox.len() >= 256 {
            return Response::json(429, &json!({ "error": "busy" }));
        }
        inbox.push(command);
        drop(inbox);
        drop(watchers);
        self.wake();
        Response::json(200, &json!({ "ok": true }))
    }

    /// Transcribes an uploaded voice clip (16 kHz mono WAV in the body) on the Mac and returns the
    /// text for the page to place in its prompt box. The audio is never stored or forwarded; it is
    /// only transcribed in memory. The clip is tied to a real session, so a stranger with a cookie
    /// still cannot aim it at a terminal that is not the user's.
    fn voice(&self, request: &Request) -> Response {
        let Some(pane) = request.query_param("pane").and_then(|t| t.parse::<u64>().ok()) else {
            return Response::json(400, &json!({ "error": "bad request" }));
        };
        if !self.is_session(pane) {
            return Response::json(404, &json!({ "error": "no such session" }));
        }
        let Some(config) = self.voice.read().unwrap_or_else(|e| e.into_inner()).clone() else {
            return Response::json(503, &json!({ "error": "voice off" }));
        };
        if !agentty_bridge::voice::model_present(config.model) {
            return Response::json(503, &json!({ "error": "no model" }));
        }
        let Some(_slot) = self.transcribe_slot() else {
            return Response::json(429, &json!({ "error": "busy" }));
        };
        // The configured language, else whisper detects it. The page's UI language is no hint: a
        // Korean speaker may well use an English browser.
        let lang = config.language.clone();
        let samples = match agentty_bridge::voice::wav_to_samples(&request.body) {
            Ok(samples) if !samples.is_empty() => samples,
            Ok(_) => return Response::json(422, &json!({ "error": "no audio" })),
            Err(_) => return Response::json(400, &json!({ "error": "bad audio" })),
        };
        match agentty_bridge::voice::transcribe(config.model, &samples, lang.as_deref()) {
            Ok(text) => Response::json(200, &json!({ "text": text })),
            Err(_) => Response::json(500, &json!({ "error": "transcribe failed" })),
        }
    }

    /// Wakes a sleeping workspace, or opens a tab in one. Only workspaces the page is shown (the
    /// user's own, not left out of remote access) can be named.
    fn workspace_action(&self, request: &Request) -> Response {
        let Ok(body) = serde_json::from_slice::<WorkspaceBody>(&request.body) else {
            return Response::json(400, &json!({ "error": "bad request" }));
        };
        let known = self.view.lock().unwrap_or_else(|e| e.into_inner()).workspaces.iter().any(|w| w.id == body.workspace);
        if !known {
            return Response::json(404, &json!({ "error": "no such workspace" }));
        }
        let command = match (body.action.as_str(), body.tool.as_str()) {
            ("wake", "") => Command::Wake { workspace: body.workspace },
            ("new", tool) => {
                let kind = match tool {
                    "shell" => PaneKind::Shell,
                    "claude" => PaneKind::Claude,
                    "codex" => PaneKind::Codex,
                    _ => return Response::json(400, &json!({ "error": "bad request" })),
                };
                let mut opened = self.new_tabs.lock().unwrap_or_else(|e| e.into_inner());
                while opened.front().is_some_and(|at| at.elapsed() >= NEW_TAB_WINDOW) {
                    opened.pop_front();
                }
                if opened.len() >= MAX_NEW_TABS {
                    return Response::json(429, &json!({ "error": "busy" }));
                }
                opened.push_back(Instant::now());
                Command::NewTab { workspace: body.workspace, kind }
            }
            _ => return Response::json(400, &json!({ "error": "bad request" })),
        };
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        if inbox.len() >= 256 {
            return Response::json(429, &json!({ "error": "busy" }));
        }
        // Asked twice before the app's round (a double tap): once is enough.
        if matches!(command, Command::Wake { .. }) && inbox.contains(&command) {
            return Response::json(200, &json!({ "ok": true }));
        }
        inbox.push(command);
        drop(inbox);
        self.wake();
        Response::json(200, &json!({ "ok": true }))
    }

    /// Signals the app's round loop, once until it takes the commands.
    fn wake(&self) {
        if self.wake_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(waker) = self.waker.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = waker.unbounded_send(());
        }
    }

    fn watch(&self, pane: u64, delta: isize) {
        let mut watchers = self.watchers.lock().unwrap_or_else(|e| e.into_inner());
        let count = watchers.entry(pane).or_insert(0);
        *count = count.saturating_add_signed(delta);
        if *count == 0 {
            watchers.remove(&pane);
            drop(watchers);
            if pane != 0 {
                // Its last screen is not kept: the next page to open it would first see how it
                // looked when this one left, at this page's size.
                self.view.lock().unwrap_or_else(|e| e.into_inner()).screens.remove(&pane);
                // The last page showing it left: the terminal goes back to its pane's size. This
                // one goes in even when the inbox is full, or the size would stick; once per
                // terminal, so pages opening and closing fast can't pile them up.
                let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
                if !inbox.contains(&Command::Release { pane }) {
                    inbox.push(Command::Release { pane });
                }
                drop(inbox);
                self.wake();
            }
        }
    }

    fn stream_events(&self, mut stream: Conn, token: &str, login: &str, pane: Option<u64>) {
        // Key 0 counts open pages; a pane id counts the pages showing that terminal. Counted
        // before the page hears back, so the size it sends as soon as the stream opens is taken.
        self.watch(0, 1);
        if let Some(pane) = pane {
            self.watch(pane, 1);
        }
        let _counted = Watching { hub: self, pane };
        self.stream_to(&mut stream, token, login, pane);
    }

    fn stream_to(&self, stream: &mut Conn, token: &str, login: &str, pane: Option<u64>) {
        let mut head = Vec::new();
        let _ = http::write_event_stream_head(&mut head);
        if stream.write_by(&head, Instant::now() + IO_TIMEOUT).is_err() {
            return;
        }
        // Every event is written within IO_TIMEOUT, however slowly the page reads.
        let event = |stream: &mut Conn, name: &str, data: &str| {
            let mut bytes = Vec::new();
            let _ = http::write_event(&mut bytes, name, data);
            stream.write_by(&bytes, Instant::now() + IO_TIMEOUT)
        };
        let started = Instant::now();
        let mut sent_sessions = 0u64;
        let mut sent_screen = 0u64;
        let mut rows: Vec<u64> = Vec::new();
        let mut last_write = Instant::now();
        loop {
            if self.stop.load(Ordering::SeqCst)
                || self.paused.load(Ordering::SeqCst)
                || started.elapsed() > STREAM_LIFETIME
                || stream.peer_gone()
            {
                break;
            }
            if !self.auth.lock().unwrap_or_else(|e| e.into_inner()).sessions.check(token, login, SystemTime::now()) {
                let _ = event(stream, "signedout", "{}");
                break;
            }
            let voice = self.voice_available();
            let (seen, sessions, screen) = {
                let view = self.view.lock().unwrap_or_else(|e| e.into_inner());
                let seen = view.version;
                let sessions = (view.sessions_version > sent_sessions || sent_sessions == 0).then(|| {
                    sent_sessions = view.sessions_version.max(1);
                    overview(&view, voice).to_string()
                });
                let screen = pane.and_then(|pane| view.screens.get(&pane)).filter(|(v, _)| *v > sent_screen).map(|(v, s)| {
                    sent_screen = *v;
                    s.clone()
                });
                (seen, sessions, screen)
            };
            let mut wrote = false;
            if let Some(sessions) = sessions {
                if event(stream, "sessions", &sessions).is_err() {
                    break;
                }
                wrote = true;
            }
            if let (Some(pane), Some(screen)) = (pane, screen) {
                if let Some(update) = screen_update(pane, &screen, &mut rows) {
                    if event(stream, "screen", &update.to_string()).is_err() {
                        break;
                    }
                    wrote = true;
                }
            }
            if wrote {
                last_write = Instant::now();
            } else if last_write.elapsed() >= KEEPALIVE {
                if stream.write_by(b": ping\n\n", Instant::now() + IO_TIMEOUT).is_err() {
                    break;
                }
                last_write = Instant::now();
            }
            // Sleep until something changes (or a second passes, to notice a page that left,
            // check the session and keep the connection alive).
            let view = self.view.lock().unwrap_or_else(|e| e.into_inner());
            if view.version == seen && !self.stop.load(Ordering::SeqCst) {
                let _ = self.changed.wait_timeout(view, Duration::from_secs(1));
            }
        }
    }
}

/// What the page lists: workspaces, terminals and plugins.
fn overview(view: &View, voice: bool) -> serde_json::Value {
    json!({ "workspaces": view.workspaces, "sessions": view.sessions, "plugins": view.plugins, "voice": voice })
}

/// `path` without the secret prefix (`/<secret>/app.js` → `/app.js`), or `None` when it does not
/// start with it. Compared in constant time.
fn strip_secret(path: &str, secret: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    let (head, tail) = rest.split_at_checked(secret.len())?;
    let same = head.len() == secret.len() && head.bytes().zip(secret.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
    match tail {
        _ if !same => None,
        "" => Some("/".to_string()),
        _ if tail.starts_with('/') => Some(tail.to_string()),
        _ => None,
    }
}

/// After answering a request whose body was not read: closing with that body still arriving
/// would reset the connection, and the browser would lose the answer (a 401 or 429 the page acts
/// on) along with it. So the writing side is closed and what still comes is read and dropped, for
/// a short, bounded while.
fn linger(stream: &mut Conn) {
    if stream.shutdown_write().is_err() {
        return;
    }
    let deadline = Instant::now() + LINGER;
    let mut chunk = [0u8; 16 * 1024];
    let mut dropped = 0;
    while dropped <= http::VOICE_MAX_BODY {
        let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else { return };
        if stream.set_read_timeout(Some(left)).is_err() {
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => dropped += n,
        }
    }
}

/// Writes a whole response within RESPONSE_DEADLINE.
fn send(stream: &mut Conn, response: &Response, head_only: bool) {
    let deadline = Instant::now() + RESPONSE_DEADLINE;
    if stream.write_by(http::response_head(response).as_bytes(), deadline).is_ok() && !head_only {
        let _ = stream.write_by(&response.body, deadline);
    }
}

/// A page watching (and a terminal shown), counted in `Hub::watchers` until dropped.
struct Watching<'a> {
    hub: &'a Hub,
    pane: Option<u64>,
}

impl Drop for Watching<'_> {
    fn drop(&mut self) {
        if let Some(pane) = self.pane {
            self.hub.watch(pane, -1);
        }
        self.hub.watch(0, -1);
    }
}

/// One open connection, counted in `Hub::open` until dropped.
struct OpenSlot(Arc<Hub>);

impl Drop for OpenSlot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One transcription running, counted in `Hub::transcribing` until dropped.
struct TranscribeSlot<'a>(&'a Hub);

impl Drop for TranscribeSlot<'_> {
    fn drop(&mut self) {
        self.0.transcribing.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One live stream of a session, counted in `Hub::streams` until dropped.
struct StreamSlot<'a> {
    hub: &'a Hub,
    token: String,
}

impl Drop for StreamSlot<'_> {
    fn drop(&mut self) {
        let mut streams = self.hub.streams.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = streams.get_mut(&self.token) {
            *count -= 1;
            if *count == 0 {
                streams.remove(&self.token);
            }
        }
    }
}

enum Routed {
    Response(Response),
    Events { token: String, login: String, pane: Option<u64> },
}

/// Sent from another site, by the browser's word (`Sec-Fetch-Site`); a request without the
/// header is judged by the other checks.
fn cross_site(request: &Request) -> bool {
    request.header("sec-fetch-site").is_some_and(|site| site != "same-origin" && site != "none")
}

/// A POST that only this page can make: sent from its own origin, as JSON, with its header.
fn same_origin_post(request: &Request, origin: &str) -> Result<(), u16> {
    if request.header("origin") != Some(origin) {
        return Err(403);
    }
    if request.header("x-agentty") != Some("1") {
        return Err(403);
    }
    if !request.header("content-type").is_some_and(|t| t.starts_with("application/json")) {
        return Err(415);
    }
    Ok(())
}

fn expired_cookie(config: &Config) -> String {
    format!("{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}", config.cookie_name(), if config.https { "; Secure" } else { "" })
}

/// Text from a request, made safe to show: no control characters, no invisible formatting (a
/// right-to-left override would make a login read as another), and short.
fn clean_text(text: &str) -> String {
    let invisible = |c: char| matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}');
    text.chars().filter(|c| !c.is_control() && !invisible(*c)).take(80).collect()
}

/// "Safari on iPhone"-style name from a user agent, for the device list.
fn device_name(agent: &str) -> String {
    let browser = if agent.contains("Edg/") {
        "Edge"
    } else if agent.contains("Firefox/") || agent.contains("FxiOS") {
        "Firefox"
    } else if agent.contains("Chrome/") || agent.contains("CriOS") {
        "Chrome"
    } else if agent.contains("Safari/") {
        "Safari"
    } else {
        "Browser"
    };
    let system = if agent.contains("iPhone") {
        "iPhone"
    } else if agent.contains("iPad") {
        "iPad"
    } else if agent.contains("Android") {
        "Android"
    } else if agent.contains("Mac OS X") || agent.contains("Macintosh") {
        "Mac"
    } else if agent.contains("Windows") {
        "Windows"
    } else if agent.contains("Linux") {
        "Linux"
    } else {
        "?"
    };
    format!("{browser} · {system}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{Ipv4Addr, TcpStream};

    const HOST: &str = "mac.example.ts.net:8743";
    const OWNER: &str = "me@example.com";

    fn hub(password: &str) -> Arc<Hub> {
        let config = Config { owner: Some(OWNER.into()), hosts: vec![HOST.into()], https: true, secret: None };
        Hub::start(config, Some(auth::hash_for_tests(password)), Listener::tcp().unwrap()).unwrap()
    }

    fn port(hub: &Hub) -> u16 {
        hub.endpoint.tcp_port().expect("tests serve on TCP")
    }

    fn session(pane: u64) -> SessionInfo {
        SessionInfo {
            pane,
            workspace: "Agentty".into(),
            workspace_id: 1,
            tab: 0,
            title: "Fix".into(),
            tool: "claude".into(),
            status: "Working".into(),
            color: "#e8912d".into(),
            needs_user: false,
            working: true,
            unread: false,
            elapsed: Some(3),
            last_activity_ms: 0,
            asks: None,
        }
    }

    /// Sends raw bytes and returns (status, head, body).
    fn send(hub: &Hub, raw: &str) -> (u16, String, String) {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port(hub))).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = stream.read_to_string(&mut out);
        let status = out.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
        let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
        (status, head.to_string(), body.to_string())
    }

    struct Req<'a> {
        method: &'a str,
        path: &'a str,
        host: &'a str,
        login: Option<&'a str>,
        origin: Option<&'a str>,
        marker: bool,
        json: bool,
        cookie: Option<String>,
        body: String,
    }

    impl<'a> Req<'a> {
        fn get(path: &'a str) -> Self {
            Req {
                method: "GET",
                path,
                host: HOST,
                login: Some(OWNER),
                origin: None,
                marker: false,
                json: false,
                cookie: None,
                body: String::new(),
            }
        }
        fn post(path: &'a str, body: serde_json::Value) -> Self {
            Req {
                method: "POST",
                path,
                host: HOST,
                login: Some(OWNER),
                origin: Some("https://mac.example.ts.net:8743"),
                marker: true,
                json: true,
                cookie: None,
                body: body.to_string(),
            }
        }
        fn raw(&self) -> String {
            let mut out = format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Mozilla/5.0 (iPhone) Safari/604.1\r\n",
                self.method, self.path, self.host
            );
            if let Some(login) = self.login {
                out.push_str(&format!("Tailscale-User-Login: {login}\r\n"));
            }
            if let Some(origin) = self.origin {
                out.push_str(&format!("Origin: {origin}\r\n"));
            }
            if self.marker {
                out.push_str("X-Agentty: 1\r\n");
            }
            if self.json {
                out.push_str("Content-Type: application/json\r\n");
            }
            if let Some(cookie) = &self.cookie {
                out.push_str(&format!("Cookie: {cookie}\r\n"));
            }
            out.push_str(&format!("Content-Length: {}\r\n\r\n{}", self.body.len(), self.body));
            out
        }
    }

    fn sign_in(hub: &Hub, password: &str) -> String {
        let (status, head, body) = send(hub, &Req::post("/api/login", json!({ "password": password })).raw());
        assert_eq!(status, 200, "{body}");
        let cookie = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().to_string();
        assert!(
            cookie.contains("HttpOnly")
                && cookie.contains("SameSite=Strict")
                && cookie.contains("Secure")
                && cookie.starts_with("__Host-agentty=")
        );
        cookie.split(';').next().unwrap().to_string()
    }

    #[test]
    fn wrong_host_or_login_gets_nothing() {
        let hub = hub("secret-password");
        assert_eq!(send(&hub, &Req { host: "evil.example:8743", ..Req::get("/") }.raw()).0, 421, "DNS rebinding");
        assert_eq!(send(&hub, &Req { login: None, ..Req::get("/") }.raw()).0, 403, "no Tailscale identity");
        assert_eq!(send(&hub, &Req { login: Some("other@example.com"), ..Req::get("/") }.raw()).0, 403, "someone else on the tailnet");
        let (status, head, body) = send(&hub, &Req::get("/").raw());
        assert_eq!(status, 200);
        assert!(body.contains("<html") && head.contains("Content-Security-Policy"));
        assert!(hub.log().iter().any(|e| e.what == "remote.log.refused_login" && e.login == "other@example.com"));
        hub.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn serves_on_a_private_socket() {
        let base = std::env::temp_dir().join(format!("agentty-hub-test-{}", std::process::id()));
        let path = base.join("remote").join("web.sock");
        let config = Config { owner: Some(OWNER.into()), hosts: vec![HOST.into()], https: true, secret: None };
        let hub = Hub::start(config, Some(auth::hash_for_tests("secret-password")), Listener::unix(&path).unwrap()).unwrap();
        let mut stream = std::os::unix::net::UnixStream::connect(&path).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream.write_all(Req::get("/").raw().as_bytes()).unwrap();
        let mut out = String::new();
        let _ = stream.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 200"), "{out:.80}");
        // Every check still applies on the socket: the identity, then a session.
        let mut stream = std::os::unix::net::UnixStream::connect(&path).unwrap();
        stream.write_all(Req { login: Some("other@example.com"), ..Req::get("/") }.raw().as_bytes()).unwrap();
        let mut out = String::new();
        let _ = stream.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 403"), "{out:.80}");
        hub.shutdown();
        for _ in 0..50 {
            if !path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!path.exists(), "the socket goes with the server");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn only_requests_with_the_secret_path_get_in() {
        let config = Config { owner: Some(OWNER.into()), hosts: vec![HOST.into()], https: true, secret: Some("s3cr3t".into()) };
        let hub = Hub::start(config, Some(auth::hash_for_tests("secret-password")), Listener::tcp().unwrap()).unwrap();
        // What Tailscale forwards: the secret in front of the page's own path.
        let (status, _, body) = send(&hub, &Req::get("/s3cr3t/").raw());
        assert_eq!(status, 200);
        assert!(body.contains("<html"));
        assert_eq!(send(&hub, &Req::get("/s3cr3t").raw()).0, 200);
        assert_eq!(send(&hub, &Req::get("/s3cr3t/app.js").raw()).0, 200);
        // Straight to the port, with every header Tailscale would add forged: refused.
        assert_eq!(send(&hub, &Req::get("/").raw()).0, 404);
        assert_eq!(send(&hub, &Req::get("/app.js").raw()).0, 404);
        assert_eq!(send(&hub, &Req::post("/api/login", json!({ "password": "secret-password" })).raw()).0, 404);
        assert_eq!(send(&hub, &Req::get("/s3cr3tX/").raw()).0, 404, "a longer name is not the secret");
        assert_eq!(send(&hub, &Req::get("/s3cr3/").raw()).0, 404);
        hub.shutdown();
        assert_eq!(strip_secret("/abc/api/me", "abc").as_deref(), Some("/api/me"));
        assert_eq!(strip_secret("/abc", "abc").as_deref(), Some("/"));
        assert_eq!(strip_secret("/abd/api", "abc"), None);
        assert_eq!(strip_secret("/ab", "abc"), None);
        assert_eq!(strip_secret("/한글/x", "abc"), None, "no panic off a character boundary");
    }

    #[test]
    fn a_paused_server_keeps_its_port_and_lets_nobody_in() {
        let hub = hub("secret-password");
        let cookie = sign_in(&hub, "secret-password");
        hub.pause();
        // The port is still this server's: nothing else could bind it.
        assert!(std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port(&hub))).is_err());
        assert_eq!(send(&hub, &Req::get("/").raw()).0, 0, "closed without an answer");
        assert_eq!(send(&hub, &Req { cookie: Some(cookie), ..Req::get("/api/sessions") }.raw()).0, 0);
        hub.shutdown();
    }

    #[test]
    fn a_session_gets_a_few_streams_not_all() {
        let hub = hub("secret-password");
        let cookie = sign_in(&hub, "secret-password");
        let token = cookie.split_once('=').unwrap().1.to_string();
        let slots: Vec<_> = (0..MAX_STREAMS_PER_SESSION).map(|_| hub.stream_slot(&token).unwrap()).collect();
        assert!(hub.stream_slot(&token).is_none(), "one session's limit");
        assert!(hub.stream_slot("another-session").is_some(), "others still get theirs");
        drop(slots);
        assert!(hub.stream_slot(&token).is_some(), "given back when they end");
        hub.shutdown();
    }

    #[test]
    fn a_voice_upload_is_checked_before_its_body_is_read() {
        let hub = hub("secret-password");
        hub.publish(Vec::new(), vec![session(7)], Vec::new());
        let cookie = sign_in(&hub, "secret-password");
        // A head announcing a large clip whose body never comes: each refusal is answered at once
        // instead of waiting for (or reading) the body.
        let head_only = |req: Req| req.raw().replace("Content-Length: 2\r\n\r\n{}", "Content-Length: 4000000\r\n\r\n");
        let voice = || Req::post("/api/voice?pane=7", json!({}));
        let started = Instant::now();
        assert_eq!(send(&hub, &head_only(voice())).0, 401, "no session");
        let forged = format!("__Host-agentty={}", auth::new_token());
        assert_eq!(send(&hub, &head_only(Req { cookie: Some(forged), ..voice() })).0, 401, "a made-up token");
        assert_eq!(send(&hub, &head_only(Req { cookie: Some(cookie.clone()), marker: false, ..voice() })).0, 403, "not from the page");
        assert_eq!(send(&hub, &head_only(Req { cookie: Some(cookie.clone()), origin: None, ..voice() })).0, 403);
        assert_eq!(send(&hub, &head_only(Req { cookie: Some(cookie.clone()), method: "GET", ..voice() })).0, 405);
        // Every transcription slot taken: the page is told to wait, before it sends its clip.
        let slots: Vec<_> = (0..MAX_TRANSCRIPTIONS).map(|_| hub.transcribe_slot().unwrap()).collect();
        assert!(hub.transcribe_slot().is_none());
        assert_eq!(send(&hub, &head_only(Req { cookie: Some(cookie.clone()), ..voice() })).0, 429);
        drop(slots);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        // Signed in with a slot free, the body is read and the request reaches the route (voice is
        // off in tests).
        let (status, _, body) = send(&hub, &Req { cookie: Some(cookie), ..voice() }.raw());
        assert_eq!(status, 503, "{body}");
        assert!(hub.transcribe_slot().is_some(), "slots are given back");
        hub.shutdown();
    }

    #[test]
    fn a_refused_upload_still_gets_its_answer() {
        // The page sends its whole clip at once; refused before the body is read, the answer must
        // reach it rather than be lost to a reset connection.
        let hub = hub("secret-password");
        let body = vec![b'a'; 2 * 1024 * 1024];
        let head = Req::post("/api/voice?pane=7", json!({}))
            .raw()
            .replace("Content-Length: 2\r\n\r\n{}", &format!("Content-Length: {}\r\n\r\n", body.len()));
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port(&hub))).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let sender = std::thread::spawn(move || {
            let _ = writer.write_all(head.as_bytes());
            let _ = writer.write_all(&body);
        });
        // Like a browser, the answer is read once the upload is done.
        sender.join().unwrap();
        let mut status = String::new();
        let _ = BufReader::new(&stream).read_line(&mut status);
        assert!(status.starts_with("HTTP/1.1 401"), "{status:?}");
        hub.shutdown();
    }

    #[test]
    fn everything_needs_a_session() {
        let hub = hub("secret-password");
        hub.publish(Vec::new(), vec![session(7)], Vec::new());
        for path in ["/api/sessions", "/api/events", "/api/events?pane=7"] {
            assert_eq!(send(&hub, &Req::get(path).raw()).0, 401, "{path}");
        }
        assert_eq!(send(&hub, &Req::post("/api/input", json!({ "pane": 7, "kind": "text", "text": "ls" })).raw()).0, 401);
        let forged = format!("__Host-agentty={}", auth::new_token());
        assert_eq!(send(&hub, &Req { cookie: Some(forged), ..Req::get("/api/sessions") }.raw()).0, 401, "a made-up token");
        assert!(hub.take_commands().is_empty());
        hub.shutdown();
    }

    fn workspace(id: u64) -> WorkspaceInfo {
        WorkspaceInfo {
            id,
            name: "repo".into(),
            group: None,
            group_color: None,
            color: None,
            branch: None,
            folder: "~/repo".into(),
            sleeping: true,
            active_tab: 0,
            tabs: Vec::new(),
        }
    }

    #[test]
    fn wakes_and_opens_tabs_in_listed_workspaces_only() {
        let hub = hub("secret-password");
        let plugin = PluginInfo {
            id: "x".into(),
            name: "X".into(),
            version: "1.0.0".into(),
            state: "running",
            error: None,
            automations: Vec::new(),
        };
        hub.publish(vec![workspace(3)], Vec::new(), vec![plugin]);
        let cookie = sign_in(&hub, "secret-password");
        let (_, _, body) = send(&hub, &Req { cookie: Some(cookie.clone()), ..Req::get("/api/sessions") }.raw());
        assert!(body.contains("\"plugins\":[{"), "plugins are listed: {body}");
        let act = |body: serde_json::Value| send(&hub, &Req { cookie: Some(cookie.clone()), ..Req::post("/api/workspace", body) }.raw()).0;
        assert_eq!(act(json!({ "workspace": 3, "action": "wake" })), 200);
        assert_eq!(act(json!({ "workspace": 3, "action": "wake" })), 200, "a double tap");
        assert_eq!(act(json!({ "workspace": 3, "action": "new", "tool": "claude" })), 200);
        assert_eq!(act(json!({ "workspace": 4, "action": "wake" })), 404, "a workspace the page is not shown");
        assert_eq!(act(json!({ "workspace": 3, "action": "new", "tool": "bash -c x" })), 400, "only the known tools");
        assert_eq!(act(json!({ "workspace": 3, "action": "remove" })), 400);
        assert_eq!(act(json!({ "workspace": 3, "action": "wake", "tool": "shell" })), 400);
        assert_eq!(act(json!({ "workspace": 3, "action": "wake", "cwd": "/" })), 400, "unknown fields refused");
        assert_eq!(
            hub.take_commands(),
            vec![Command::Wake { workspace: 3 }, Command::NewTab { workspace: 3, kind: PaneKind::Claude }],
            "the second wake was not queued again"
        );
        for _ in 1..MAX_NEW_TABS {
            assert_eq!(act(json!({ "workspace": 3, "action": "new", "tool": "shell" })), 200);
        }
        assert_eq!(act(json!({ "workspace": 3, "action": "new", "tool": "shell" })), 429, "a few tabs a minute, not a flood");
        let anonymous = send(&hub, &Req::post("/api/workspace", json!({ "workspace": 3, "action": "wake" })).raw()).0;
        assert_eq!(anonymous, 401, "needs a session");
        hub.shutdown();
    }

    #[test]
    fn signs_in_and_works_then_signs_out() {
        let hub = hub("secret-password");
        hub.publish(Vec::new(), vec![session(7)], Vec::new());
        let cookie = sign_in(&hub, "secret-password");
        let (status, _, body) = send(&hub, &Req { cookie: Some(cookie.clone()), ..Req::get("/api/sessions") }.raw());
        assert_eq!(status, 200);
        assert!(body.contains("\"pane\":7"));
        let input = |body: serde_json::Value| send(&hub, &Req { cookie: Some(cookie.clone()), ..Req::post("/api/input", body) }.raw()).0;
        assert_eq!(input(json!({ "pane": 7, "kind": "prompt", "text": "run the tests" })), 200);
        assert_eq!(input(json!({ "pane": 7, "kind": "chat", "text": "add a login page" })), 200);
        assert_eq!(input(json!({ "pane": 7, "kind": "chat", "text": "  " })), 400, "an empty chat message");
        assert_eq!(input(json!({ "pane": 8, "kind": "chat", "text": "x" })), 404, "a chat message for a pane not on the list");
        assert_eq!(input(json!({ "pane": 7, "kind": "key", "key": "c", "ctrl": true })), 200);
        assert_eq!(input(json!({ "pane": 8, "kind": "text", "text": "x" })), 404, "a pane that is not on the list");
        assert_eq!(input(json!({ "pane": 7, "kind": "key", "key": "cmd-q" })), 400, "unknown key");
        assert_eq!(input(json!({ "pane": 7, "kind": "exec", "text": "x" })), 400);
        assert_eq!(input(json!({ "pane": 7, "kind": "text", "text": "x", "extra": 1 })), 400, "unknown fields refused");
        assert_eq!(input(json!({ "pane": 7, "kind": "text", "text": "x".repeat(MAX_INPUT_CHARS + 1) })), 413);
        assert_eq!(input(json!({ "pane": 7, "kind": "resize", "cols": 48, "rows": 30 })), 409, "no page shows it: the size would stick");
        hub.watch(7, 1);
        assert_eq!(input(json!({ "pane": 7, "kind": "resize", "cols": 48, "rows": 30 })), 200);
        assert_eq!(input(json!({ "pane": 7, "kind": "resize", "cols": 50, "rows": 30 })), 200);
        assert_eq!(input(json!({ "pane": 7, "kind": "resize", "cols": 5, "rows": 30 })), 400, "too narrow");
        assert_eq!(input(json!({ "pane": 7, "kind": "resize", "cols": 48, "rows": 900 })), 400, "too tall");
        assert_eq!(input(json!({ "pane": 7, "kind": "resize" })), 400, "no size");
        assert_eq!(
            hub.take_commands(),
            vec![
                Command::Prompt { pane: 7, text: "run the tests".into() },
                Command::Chat { pane: 7, text: "add a login page".into() },
                Command::Key { pane: 7, key: "c".into(), ctrl: true, alt: false, shift: false },
                Command::Resize { pane: 7, cols: 50, rows: 30 }
            ],
            "the newer size replaced the one still waiting"
        );
        hub.watch(7, 1);
        hub.watch(7, -1);
        assert!(hub.take_commands().is_empty(), "another page still shows it");
        hub.watch(7, -1);
        assert_eq!(hub.take_commands(), vec![Command::Release { pane: 7 }], "the last page left");
        for _ in 0..5 {
            hub.watch(7, 1);
            hub.watch(7, -1);
        }
        assert_eq!(hub.take_commands(), vec![Command::Release { pane: 7 }], "pages coming and going pile up one release, not five");
        hub.watch(0, 1);
        hub.watch(0, -1);
        assert!(hub.take_commands().is_empty(), "the page count is not a terminal");
        assert_eq!(send(&hub, &Req { cookie: Some(cookie.clone()), ..Req::post("/api/logout", json!({})) }.raw()).0, 200);
        assert_eq!(send(&hub, &Req { cookie: Some(cookie), ..Req::get("/api/sessions") }.raw()).0, 401, "gone after sign out");
        hub.shutdown();
    }

    #[test]
    fn a_session_is_bound_to_its_tailscale_login() {
        let config = Config { owner: Some(OWNER.into()), hosts: vec![HOST.into()], https: true, secret: None };
        let hub = Hub::start(config.clone(), Some(auth::hash_for_tests("secret-password")), Listener::tcp().unwrap()).unwrap();
        let cookie = sign_in(&hub, "secret-password");
        // The owner changes (another account signs in to Tailscale on this Mac).
        hub.set_config(Config { owner: Some("new@example.com".into()), ..config });
        let req = Req { cookie: Some(cookie), login: Some("new@example.com"), ..Req::get("/api/sessions") };
        assert_eq!(send(&hub, &req.raw()).0, 401);
        hub.shutdown();
    }

    #[test]
    fn cross_site_requests_are_refused() {
        let hub = hub("secret-password");
        hub.publish(Vec::new(), vec![session(7)], Vec::new());
        let cookie = sign_in(&hub, "secret-password");
        let body = json!({ "pane": 7, "kind": "text", "text": "rm -rf ~" });
        let base = || Req { cookie: Some(cookie.clone()), ..Req::post("/api/input", body.clone()) };
        assert_eq!(send(&hub, &Req { origin: None, ..base() }.raw()).0, 403, "no Origin");
        assert_eq!(send(&hub, &Req { origin: Some("https://evil.example"), ..base() }.raw()).0, 403, "another site");
        assert_eq!(send(&hub, &Req { origin: Some("http://mac.example.ts.net:8743"), ..base() }.raw()).0, 403, "plain http origin");
        assert_eq!(send(&hub, &Req { marker: false, ..base() }.raw()).0, 403, "a form can't add the header");
        assert_eq!(send(&hub, &Req { json: false, ..base() }.raw()).0, 415, "form-encoded");
        let mut raw = base().raw();
        raw = raw.replace("X-Agentty: 1\r\n", "X-Agentty: 1\r\nSec-Fetch-Site: cross-site\r\n");
        assert_eq!(send(&hub, &raw).0, 403);
        assert_eq!(send(&hub, &Req { method: "PUT", ..base() }.raw()).0, 400);
        assert!(hub.take_commands().is_empty(), "nothing got through");
        hub.shutdown();
    }

    #[test]
    fn wrong_passwords_lock_and_alert() {
        let hub = hub("secret-password");
        for _ in 0..4 {
            assert_eq!(send(&hub, &Req::post("/api/login", json!({ "password": "guess" })).raw()).0, 401);
        }
        let (status, _, body) = send(&hub, &Req::post("/api/login", json!({ "password": "guess" })).raw());
        assert_eq!(status, 429);
        assert!(body.contains("lockedFor"));
        // Locked: even the right password waits.
        assert_eq!(send(&hub, &Req::post("/api/login", json!({ "password": "secret-password" })).raw()).0, 429);
        assert_eq!(hub.take_alerts(), vec![Alert::Locked { minutes: 1 }]);
        assert!(hub.log().iter().any(|e| e.what == "remote.log.locked"));
        assert!(hub.log().iter().all(|e| !e.device.contains("guess") && !e.login.contains("guess")), "the log never holds a password");
        let (_, _, me) = send(&hub, &Req::get("/api/me").raw());
        assert!(me.contains("\"lockedFor\":"), "{me}");
        hub.shutdown();
    }

    #[test]
    fn a_new_password_or_sign_out_everywhere_ends_every_session() {
        let hub = hub("secret-password");
        let cookie = sign_in(&hub, "secret-password");
        hub.sign_out_everywhere();
        assert_eq!(send(&hub, &Req { cookie: Some(cookie), ..Req::get("/api/sessions") }.raw()).0, 401);
        let cookie = sign_in(&hub, "secret-password");
        hub.set_password(Some(auth::hash_for_tests("another-password")));
        assert_eq!(send(&hub, &Req { cookie: Some(cookie), ..Req::get("/api/sessions") }.raw()).0, 401);
        assert_eq!(send(&hub, &Req::post("/api/login", json!({ "password": "secret-password" })).raw()).0, 401);
        sign_in(&hub, "another-password");
        hub.set_password(None);
        assert_eq!(send(&hub, &Req::post("/api/login", json!({ "password": "another-password" })).raw()).0, 409);
        hub.shutdown();
    }

    #[test]
    fn malformed_requests_are_answered_not_served() {
        let hub = hub("secret-password");
        assert_eq!(send(&hub, "GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n").0, 400);
        assert_eq!(send(&hub, "POST /api/login HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").0, 501);
        assert_eq!(
            send(
                &hub,
                &format!("POST /api/login HTTP/1.1\r\nHost: {HOST}\r\nTailscale-User-Login: {OWNER}\r\nContent-Length: 999999\r\n\r\n")
            )
            .0,
            413
        );
        // From anyone else, the head alone is refused: the body is never waited for.
        assert_eq!(send(&hub, &format!("POST /api/login HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 50\r\n\r\n")).0, 403);
        assert_eq!(send(&hub, &Req::post("/api/login", json!({ "password": 5 })).raw()).0, 400);
        assert_eq!(send(&hub, &Req::get("/../../etc/passwd").raw()).0, 404);
        assert_eq!(send(&hub, &Req::get("/api/nothing").raw()).0, 401, "unknown API paths need a session first");
        hub.shutdown();
    }

    #[test]
    fn streams_sessions_and_the_watched_screen() {
        let hub = hub("secret-password");
        hub.publish(Vec::new(), vec![session(7)], Vec::new());
        let cookie = sign_in(&hub, "secret-password");
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port(&hub))).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream.write_all(Req { cookie: Some(cookie), ..Req::get("/api/events?pane=7") }.raw().as_bytes()).unwrap();
        let mut reader = BufReader::new(stream);
        let mut next_event = || {
            let mut name = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    return (name, String::new());
                }
                if let Some(event) = line.strip_prefix("event: ") {
                    name = event.trim().to_string();
                } else if let Some(data) = line.strip_prefix("data: ") {
                    return (name, data.trim().to_string());
                }
            }
        };
        let (name, data) = next_event();
        assert_eq!(name, "sessions");
        assert!(data.contains("\"pane\":7"));
        // Wait until the app would see this page watching pane 7.
        for _ in 0..50 {
            if hub.watched().contains(&7) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(hub.watched().contains(&7));
        let screen = Screen {
            cols: 4,
            rows: 1,
            cursor: None,
            fg: 0xffffff,
            bg: 0,
            lines: vec![vec![super::super::snapshot::Run { t: "hey".into(), f: None, b: None, s: 0 }]],
        };
        hub.publish_screen(7, screen);
        let (name, data) = next_event();
        assert_eq!(name, "screen");
        assert!(data.contains("\"hey\"") && data.contains("\"full\":true"));
        hub.sign_out_everywhere();
        let (name, _) = next_event();
        assert_eq!(name, "signedout", "a stream ends with its session");
        hub.shutdown();
    }

    #[test]
    fn events_for_a_pane_not_on_the_list_are_refused() {
        let hub = hub("secret-password");
        hub.publish(Vec::new(), vec![session(7)], Vec::new());
        let cookie = sign_in(&hub, "secret-password");
        assert_eq!(send(&hub, &Req { cookie: Some(cookie.clone()), ..Req::get("/api/events?pane=99") }.raw()).0, 404);
        assert_eq!(send(&hub, &Req { cookie: Some(cookie), ..Req::get("/api/events?pane=abc") }.raw()).0, 404);
        assert!(!hub.watched().contains(&99));
        hub.shutdown();
    }

    #[test]
    fn the_log_folds_repeats_and_drops_invisible_characters() {
        let hub = hub("secret-password");
        for _ in 0..60 {
            send(&hub, &Req { login: Some("other@example.com"), ..Req::get("/") }.raw());
        }
        let refused = hub.log().iter().filter(|e| e.what == "remote.log.refused_login").count();
        assert_eq!(refused, 1, "a stranger polling the page can't push the owner's entries out");
        assert_eq!(clean_text("me\u{202E}moc.elpmaxe@\u{200B}x\u{7}"), "memoc.elpmaxe@x");
        hub.shutdown();
    }

    #[test]
    fn device_names() {
        assert_eq!(
            device_name("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit Version/18.0 Mobile Safari/604.1"),
            "Safari · iPhone"
        );
        assert_eq!(device_name("Mozilla/5.0 (Linux; Android 14) Chrome/130.0 Mobile Safari/537.36"), "Chrome · Android");
        assert_eq!(device_name(""), "Browser · ?");
    }
}
