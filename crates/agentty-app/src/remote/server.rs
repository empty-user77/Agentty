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
use super::http::{self, percent_encode, HttpError, Request, Response};
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
/// Images attached at once: each holds its body (up to `VOICE_MAX_BODY`) until it is written.
const MAX_ATTACHING: usize = 4;
/// Images being saved now (see [`MAX_ATTACHING`]).
static ATTACHING: AtomicUsize = AtomicUsize::new(0);
/// Image and file bodies being received now: taken from the head, before the body is read.
static RECEIVING: AtomicUsize = AtomicUsize::new(0);
/// How long an attached image is kept: long enough for the agent to read it, then cleared away.
const ATTACHMENT_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Refused uploads drained at once; past this the connection just closes, so refusals can't
/// hold every connection the server has.
const MAX_LINGERS: usize = 4;
/// Refused uploads being drained now (see [`MAX_LINGERS`]).
static LINGERING: AtomicUsize = AtomicUsize::new(0);
/// Longest text one input may carry (a prompt pasted from a phone).
pub const MAX_INPUT_CHARS: usize = 16_000;
const LOG_LEN: usize = 50;

const INDEX_HTML: &str = include_str!("../../assets/remote/index.html");
const APP_JS: &str = include_str!("../../assets/remote/app.js");
const APP_CSS: &str = include_str!("../../assets/remote/app.css");
const ICON_SVG: &str = include_str!("../../assets/remote/icon.svg");
const MANIFEST: &str = include_str!("../../assets/remote/manifest.webmanifest");
/// The file editor's Markdown renderer and sanitizer, for the viewer: loaded by the page only when
/// a Markdown file is opened.
const MARKDOWN_JS: &str = include_str!("../../assets/markdown-preview/markdown-it.min.js");
const PURIFY_JS: &str = include_str!("../../assets/markdown-preview/purify.min.js");

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
    /// The folder the page may open workspaces in (`folders::root`); `None`: it may not.
    pub folders: Option<std::path::PathBuf>,
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
    /// A prompt: pasted, then Enter. `images` (attached on the page) are pasted first, alone, as
    /// dropping them on the terminal would, so an agent CLI takes them as images.
    Prompt { pane: u64, text: String, images: Vec<std::path::PathBuf> },
    /// A message written in a chat, for its lead (`pane`): sent as the app's own chat sends it,
    /// with the lead's instructions in front the first time.
    Chat { pane: u64, text: String },
    /// The page shows this terminal at this size: the terminal takes it while watched.
    Resize { pane: u64, cols: u16, rows: u16 },
    /// A swipe on a terminal whose program scrolls itself (Claude Code's full-screen UI, less):
    /// `lines` up (positive) or down, as the mouse wheel would.
    Scroll { pane: u64, lines: i32 },
    /// No page shows this terminal any more: back to the pane's own size.
    Release { pane: u64 },
    /// A sleeping workspace brought back, its saved tabs started again.
    Wake { workspace: u64 },
    /// A new tab in a workspace, with a shell or an agent, where the workspace is.
    NewTab { workspace: u64, kind: PaneKind },
    /// A new workspace in a folder under the remote folder (checked), with a shell or an agent,
    /// listed under `group` (made if there is none) when one is given, and named `title` when one is given.
    OpenFolder { folder: std::path::PathBuf, kind: PaneKind, group: Option<String>, title: Option<String> },
    /// A local dev server a terminal runs (`port`, checked against the overview) shown on the
    /// tailnet through Agentty's proxy (`devproxy`).
    OpenPort { port: u16 },
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
            | Command::Scroll { pane, .. }
            | Command::Release { pane } => Some(*pane),
            Command::Wake { .. } | Command::NewTab { .. } | Command::OpenFolder { .. } | Command::OpenPort { .. } => None,
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
    #[serde(default)]
    workspace: u64,
    action: String,
    #[serde(default)]
    tool: String,
    /// For `open` and `mkdir`: folder names under the remote folder, joined with `/`.
    #[serde(default)]
    path: String,
    /// For `mkdir`: the new folder's name.
    #[serde(default)]
    name: String,
    /// For `open`: the group the new workspace is listed under.
    #[serde(default)]
    group: Option<String>,
    /// For `open`: the name the new workspace is shown with, instead of the tool's.
    #[serde(default)]
    title: Option<String>,
}

const MAX_GROUP_NAME: usize = 40;
const MAX_TITLE_NAME: usize = 60;

/// Files the page may send into a workspace's folder, by extension (lowercase).
const WORKSPACE_FILE_TYPES: &[&str] = &[
    "pdf", "png", "jpg", "jpeg", "gif", "webp", "heic", "txt", "md", "csv", "json", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "rtf",
    "zip",
];
/// Where those files go, inside the workspace's folder.
const WORKSPACE_FILE_DIR: &str = ".agentty-remote";
/// In characters: 70 Hangul syllables are 210 bytes, under the 255-byte name limit with the extension.
const MAX_WORKSPACE_FILE_NAME: usize = 70;

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
    #[serde(default)]
    lines: i32,
    /// Paths `/api/attach` returned, for a prompt or a chat message.
    #[serde(default)]
    images: Vec<String>,
}

/// The sizes a page may give a terminal: a phone held upright up to a wide desktop window.
const COLS: std::ops::RangeInclusive<u16> = 20..=400;
const ROWS: std::ops::RangeInclusive<u16> = 5..=200;
/// Lines one scroll input may move, and what waits for the app to pass on: a fast swipe, not a
/// flood of wheel events.
const SCROLL_LINES: i32 = 60;

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

    pub fn config(&self) -> Config {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Checks 2 and 3 for a request to a dev server's proxy (`devproxy`), which the page's own
    /// checks cover too: `login` is who Tailscale says sent it (`None`: no proxy added one), and
    /// the session cookie must be one of that login's. The page's cookie reaches the dev server's
    /// address as well: cookies are kept per machine name, not per port.
    pub fn admits(&self, login: Option<&str>, cookie_header: Option<&str>) -> bool {
        self.dev_admits(login, cookie_header, false)
    }

    /// `admits` for a connection already open, checked again every few seconds: it doesn't count
    /// as use, so a live-reload socket left open can't keep the session from going idle.
    pub fn still_admits(&self, login: Option<&str>, cookie_header: Option<&str>) -> bool {
        self.dev_admits(login, cookie_header, true)
    }

    fn dev_admits(&self, login: Option<&str>, cookie_header: Option<&str>, recheck: bool) -> bool {
        let config = self.config();
        if self.paused.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
            return false;
        }
        let login = match (&config.owner, login) {
            (Some(owner), Some(login)) if login.eq_ignore_ascii_case(owner) => login.to_string(),
            (None, _) => "local".to_string(),
            _ => return false,
        };
        let name = config.cookie_name();
        let token = cookie_header
            .into_iter()
            .flat_map(|h| h.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
            .unwrap_or("");
        let mut auth = self.auth.lock().unwrap_or_else(|e| e.into_inner());
        if recheck {
            auth.sessions.check_idle(token, &login, SystemTime::now())
        } else {
            auth.sessions.check(token, &login, SystemTime::now())
        }
    }

    /// Whether a terminal the page may see runs a server on `port` now (the overview's ports).
    pub fn dev_port_allowed(&self, port: u16) -> bool {
        self.view.lock().unwrap_or_else(|e| e.into_inner()).workspaces.iter().any(|w| w.ports.iter().any(|p| p.port == port))
    }

    /// The name of the page's session cookie, which a dev server must neither see nor replace.
    pub fn cookie_name(&self) -> &'static str {
        self.config().cookie_name()
    }

    /// A signed-in session for `login`, as a sign-in would make (tests of other modules).
    #[cfg(test)]
    pub fn session_for_tests(&self, login: &str) -> String {
        self.auth.lock().unwrap_or_else(|e| e.into_inner()).sessions.create(login, "test", SystemTime::now())
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
        let upload = std::cell::Cell::new(false);
        // Held from the head until the connection ends, so at most `MAX_ATTACHING` image or file
        // bodies (up to `FILE_MAX_BODY` each) are in memory at once, not one per connection.
        let receiving = std::cell::RefCell::new(None);
        let request = match http::read_request(&mut stream, deadline, |head| {
            upload.set(http::is_upload(&head.path));
            if config.secret.as_deref().is_some_and(|secret| strip_secret(&head.path, secret).is_none()) {
                return Err(404);
            }
            let (host, login) = self.admitted(head, &config, true).map_err(|refused| refused.status)?;
            if http::is_upload(&head.path) {
                self.upload_admitted(head, &config, &host, &login)?;
                if !http::is_voice(&head.path) {
                    *receiving.borrow_mut() = Some(Slot::take(&RECEIVING, MAX_ATTACHING).ok_or(429u16)?);
                }
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
                // Only an upload (voice clip, image) sends a body big enough to be still arriving
                // (the page acts on its 401 / 429); a few at a time, so refusals can't take every
                // connection.
                if upload.get() && matches!(error, HttpError::Refused(_) | HttpError::BodyTooLarge) {
                    if let Some(_slot) = Slot::take(&LINGERING, MAX_LINGERS) {
                        linger(&mut stream);
                    }
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

    /// An upload's head (voice clip or image), checked in full before its body (up to
    /// `VOICE_MAX_BODY`) is read: a POST from this page (check 4), a session of this login
    /// (check 3), and for a voice clip a transcription slot free, for an image a saving slot.
    /// `route` checks it all again once the body is in.
    fn upload_admitted(&self, request: &Request, config: &Config, host: &str, login: &str) -> Result<(), u16> {
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
        let busy = if http::is_voice(&request.path) {
            self.transcribing.load(Ordering::SeqCst) >= MAX_TRANSCRIPTIONS
        } else {
            ATTACHING.load(Ordering::SeqCst) >= MAX_ATTACHING
        };
        if busy {
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
            (true, "/md/markdown-it.js" | "/md/purify.js") => {
                let script = if path == "/md/markdown-it.js" { MARKDOWN_JS } else { PURIFY_JS };
                return Routed::Response(
                    Response::fixed(200, "text/javascript; charset=utf-8", script.as_bytes())
                        .with("Cache-Control", "private, max-age=604800"),
                );
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
        // A file is answered to HEAD as to GET (its body left out when sent): the viewer asks it of
        // a picture before showing it, to know it is there. Nothing else streams or changes on HEAD.
        match (if method == "HEAD" && path == "/api/file" { "GET" } else { method }, path) {
            ("POST", "/api/logout") => {
                self.auth.lock().unwrap_or_else(|e| e.into_inner()).sessions.remove(&token);
                self.record("remote.log.signed_out", &login, request.header("user-agent").unwrap_or(""));
                Routed::Response(Response::json(200, &json!({ "ok": true })).with("Set-Cookie", expired_cookie(&config)))
            }
            ("GET", "/api/folders") => Routed::Response(self.folders(request, &config)),
            ("GET", "/api/file") => Routed::Response(self.file(request)),
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
            ("POST", "/api/attach") => Routed::Response(self.attach(request)),
            ("POST", "/api/workspace/file") => Routed::Response(self.workspace_file(request)),
            ("POST", "/api/workspace") => Routed::Response(self.workspace_action(request)),
            ("POST", "/api/devserver") => Routed::Response(self.open_dev_server(request)),
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

    /// A file named in a terminal's output, for the page's viewer (see `files`): `pane` is the
    /// terminal it was named in, `path` as written there. A picture or a PDF goes out as itself;
    /// text as plain text, cut at `files::MAX_TEXT` (`X-Agentty-Cut: 1`). Opened on its own (not
    /// in the viewer), the answer is sandboxed: an SVG's script or a page's never runs.
    fn file(&self, request: &Request) -> Response {
        use super::files::{self, FileError, Kind};
        let refused = |status: u16, error: &str| Response::json(status, &json!({ "error": error }));
        let (Some(pane), Some(path)) = (request.query_param("pane").and_then(|p| p.parse::<u64>().ok()), request.query_param("path"))
        else {
            return refused(400, "bad request");
        };
        let found = {
            let view = self.view.lock().unwrap_or_else(|e| e.into_inner());
            view.sessions.iter().find(|s| s.pane == pane).map(|s| (s.dir.clone(), s.roots.clone()))
        };
        let Some((dir, roots)) = found else { return refused(404, "no such session") };
        let (real, kind) = match files::resolve(&path, &dir, &roots, &agentty_bridge::fsutil::home()) {
            Ok(found) => found,
            Err(FileError::Refused) => return refused(403, "refused"),
            Err(FileError::NotFound) => return refused(404, "not found"),
            Err(FileError::Unsupported) => return refused(415, "unsupported"),
            Err(FileError::TooLarge) => return refused(413, "too large"),
        };
        let Ok((bytes, cut)) = files::read(&real, kind) else { return refused(404, "not found") };
        let name = real.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let shown_as = match kind {
            Kind::Image(_) => "image",
            Kind::Pdf => "pdf",
            Kind::Text { markdown: true } => "markdown",
            Kind::Text { markdown: false } => "text",
        };
        let response = Response::new(200, kind.media_type(), bytes)
            .with("Content-Disposition", format!("inline; filename*=UTF-8''{}", percent_encode(&name)))
            .with("X-Agentty-Kind", shown_as)
            .with("X-Agentty-Cut", if cut { "1" } else { "0" });
        // A PDF viewer is a plugin a sandbox turns off; it runs no script of the file's own anyway.
        if kind == Kind::Pdf {
            response
        } else {
            response.with("Content-Security-Policy", "sandbox; default-src 'none'; img-src 'self'; style-src 'unsafe-inline'")
        }
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
        // Images attached on the page go with a prompt or a chat message, and only ones it saved.
        let Some(images) = attached_images(&attachments_dir(), &body.images) else {
            return Response::json(400, &json!({ "error": "bad request" }));
        };
        let has_images = !images.is_empty();
        let command = match body.kind.as_str() {
            "key" if allowed_key(&body.key) && !has_images => {
                Command::Key { pane: body.pane, key: body.key, ctrl: body.ctrl, alt: body.alt, shift: body.shift }
            }
            "text" if !body.text.is_empty() && !has_images => Command::Text { pane: body.pane, text: body.text },
            "prompt" if !body.text.trim().is_empty() || has_images => Command::Prompt { pane: body.pane, text: body.text, images },
            // The lead gets the paths in its message: a chat message is not pasted as typed.
            "chat" if !body.text.trim().is_empty() || has_images => {
                let paths = images.iter().map(|p| crate::launch::shell_quote(&p.display().to_string()));
                let text = paths.chain(Some(body.text).filter(|t| !t.trim().is_empty())).collect::<Vec<_>>().join(" ");
                Command::Chat { pane: body.pane, text }
            }
            "resize" if COLS.contains(&body.cols) && ROWS.contains(&body.rows) => {
                Command::Resize { pane: body.pane, cols: body.cols, rows: body.rows }
            }
            // `unsigned_abs`: `i32::MIN.abs()` would wrap to a negative number and pass this check.
            "scroll" if body.lines != 0 && body.lines.unsigned_abs() <= SCROLL_LINES as u32 => {
                Command::Scroll { pane: body.pane, lines: body.lines }
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
        // Scrolling right after scrolling the same terminal joins it, up to a swipe's worth.
        if let Command::Scroll { pane, lines } = command {
            if let Some(Command::Scroll { pane: p, lines: waiting }) = inbox.last_mut() {
                if *p == pane {
                    *waiting = (*waiting + lines).clamp(-SCROLL_LINES, SCROLL_LINES);
                    return Response::json(200, &json!({ "ok": true }));
                }
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

    /// Saves an image attached on the page (raw bytes in the body) where the agent can read it, and
    /// returns its path, which the page sends back with its prompt (`images`): an agent CLI takes
    /// a pasted image path as the image. Only PNG, JPEG, GIF and WebP, told
    /// by their bytes, not by what the page says; the file is private (`0600`) and removed after
    /// `ATTACHMENT_LIFETIME`.
    fn attach(&self, request: &Request) -> Response {
        let Some(pane) = request.query_param("pane").and_then(|t| t.parse::<u64>().ok()) else {
            return Response::json(400, &json!({ "error": "bad request" }));
        };
        if !self.is_session(pane) {
            return Response::json(404, &json!({ "error": "no such session" }));
        }
        let Some(_slot) = Slot::take(&ATTACHING, MAX_ATTACHING) else {
            return Response::json(429, &json!({ "error": "busy" }));
        };
        let Some(extension) = image_extension(&request.body) else {
            return Response::json(415, &json!({ "error": "not an image" }));
        };
        let dir = attachments_dir();
        remove_old_attachments(&dir, SystemTime::now());
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let millis = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let path = dir.join(format!("image-{millis}-{}.{extension}", COUNT.fetch_add(1, Ordering::SeqCst)));
        if agentty_bridge::fsutil::write_private(&path, &request.body).is_err() {
            return Response::json(500, &json!({ "error": "not saved" }));
        }
        Response::json(200, &json!({ "path": path.display().to_string() }))
    }

    /// Wakes a sleeping workspace, or opens a tab in one. Only workspaces the page is shown (the
    /// user's own, not left out of remote access) can be named.
    /// The subfolders of a folder under the remote folder (`?path=`), for picking one to open.
    fn folders(&self, request: &Request, config: &Config) -> Response {
        let Some(root) = &config.folders else { return Response::json(404, &json!({ "error": "not found" })) };
        let path = request.query_param("path").unwrap_or_default();
        match super::folders::list(root, &path) {
            Ok(folders) => Response::json(200, &json!({ "root": super::folders::ROOT_NAME, "path": path, "folders": folders })),
            Err(err) if matches!(err.kind(), std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound) => {
                Response::json(404, &json!({ "error": "no such folder" }))
            }
            Err(_) => Response::json(500, &json!({ "error": "couldn't read the folder" })),
        }
    }

    /// Takes one of the minute's new tabs (a new workspace counts as one); false when they are
    /// all taken.
    fn take_new_tab(&self) -> bool {
        let mut opened = self.new_tabs.lock().unwrap_or_else(|e| e.into_inner());
        while opened.front().is_some_and(|at| at.elapsed() >= NEW_TAB_WINDOW) {
            opened.pop_front();
        }
        if opened.len() >= MAX_NEW_TABS {
            return false;
        }
        opened.push_back(Instant::now());
        true
    }

    fn workspace_action(&self, request: &Request) -> Response {
        let Ok(body) = serde_json::from_slice::<WorkspaceBody>(&request.body) else {
            return Response::json(400, &json!({ "error": "bad request" }));
        };
        let kind = |tool: &str| match tool {
            "shell" => Some(PaneKind::Shell),
            "claude" => Some(PaneKind::Claude),
            "codex" => Some(PaneKind::Codex),
            _ => None,
        };
        // Folders under the remote folder: a new one made, or one opened as a workspace.
        if matches!(body.action.as_str(), "open" | "mkdir") {
            let config = self.config.read().unwrap_or_else(|e| e.into_inner()).clone();
            let Some(root) = &config.folders else { return Response::json(404, &json!({ "error": "not found" })) };
            let kind = kind(&body.tool);
            let plain = |group: &str| {
                (1..=MAX_GROUP_NAME).contains(&group.chars().count()) && group.trim() == group && !group.chars().any(char::is_control)
            };
            if body.group.as_deref().is_some_and(|group| !plain(group) || body.action != "open") {
                return Response::json(400, &json!({ "error": "bad request" }));
            }
            let title_ok = |title: &str| {
                (1..=MAX_TITLE_NAME).contains(&title.chars().count()) && title.trim() == title && !title.chars().any(char::is_control)
            };
            if body.title.as_deref().is_some_and(|title| !title_ok(title) || body.action != "open") {
                return Response::json(400, &json!({ "error": "bad request" }));
            }
            let group = body.group.clone();
            let title = body.title.clone();
            if body.action == "open" && kind.is_none() || body.action == "mkdir" && !body.tool.is_empty() {
                return Response::json(400, &json!({ "error": "bad request" }));
            }
            // Checked first, then counted as a new tab: a page in a loop makes no more folders than
            // it opens tabs, and a mistyped name costs none.
            let folder = super::folders::resolve(root, &body.path).and_then(|folder| {
                if kind.is_none() && !super::folders::valid_name(&body.name) {
                    return Err(std::io::ErrorKind::InvalidInput.into());
                }
                if !self.take_new_tab() {
                    return Err(std::io::ErrorKind::QuotaExceeded.into());
                }
                match kind {
                    Some(_) => Ok(folder),
                    None => super::folders::create(root, &body.path, &body.name),
                }
            });
            let folder = match folder {
                Ok(folder) => folder,
                Err(err) if err.kind() == std::io::ErrorKind::QuotaExceeded => return Response::json(429, &json!({ "error": "busy" })),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Response::json(409, &json!({ "error": "exists" }));
                }
                Err(err) if matches!(err.kind(), std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound) => {
                    return Response::json(400, &json!({ "error": "bad folder" }));
                }
                Err(_) => return Response::json(500, &json!({ "error": "couldn't make the folder" })),
            };
            return match kind {
                Some(kind) => self.queue(Command::OpenFolder { folder, kind, group, title }),
                None => Response::json(200, &json!({ "ok": true })),
            };
        }
        let known = self.view.lock().unwrap_or_else(|e| e.into_inner()).workspaces.iter().any(|w| w.id == body.workspace);
        if !known {
            return Response::json(404, &json!({ "error": "no such workspace" }));
        }
        let command = match (body.action.as_str(), body.tool.as_str()) {
            ("wake", "") => Command::Wake { workspace: body.workspace },
            ("new", tool) => {
                let Some(kind) = kind(tool) else { return Response::json(400, &json!({ "error": "bad request" })) };
                if !self.take_new_tab() {
                    return Response::json(429, &json!({ "error": "busy" }));
                }
                Command::NewTab { workspace: body.workspace, kind }
            }
            _ => return Response::json(400, &json!({ "error": "bad request" })),
        };
        self.queue(command)
    }

    /// Saves a file sent from the phone (raw bytes in the body) into a workspace's folder, under
    /// `.agentty-remote/`, and returns its path relative to that folder, which the page pastes
    /// into a prompt. Only the kinds of `WORKSPACE_FILE_TYPES`, by the name's extension, under the
    /// file name alone (a name with a separator or `..` is refused); never over a file already there.
    fn workspace_file(&self, request: &Request) -> Response {
        let bad = || Response::json(400, &json!({ "error": "bad request" }));
        let (Some(id), Some(name)) = (request.query_param("workspace").and_then(|t| t.parse::<u64>().ok()), request.query_param("name"))
        else {
            return bad();
        };
        let folder = {
            let view = self.view.lock().unwrap_or_else(|e| e.into_inner());
            let Some(workspace) = view.workspaces.iter().find(|w| w.id == id) else {
                return Response::json(404, &json!({ "error": "no such workspace" }));
            };
            // The folder it was opened on, not wherever its terminal has `cd`'d to since.
            workspace.root.clone()
        };
        let Some(name) = workspace_file_name(&name) else { return bad() };
        let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
        if !extension.is_some_and(|ext| WORKSPACE_FILE_TYPES.contains(&ext.as_str())) {
            return Response::json(415, &json!({ "error": "unsupported type" }));
        }
        if request.body.is_empty() {
            return bad();
        }
        if request.body.len() > http::FILE_MAX_BODY {
            return Response::json(413, &json!({ "error": "too large" }));
        }
        let Some(_slot) = Slot::take(&ATTACHING, MAX_ATTACHING) else {
            return Response::json(429, &json!({ "error": "busy" }));
        };
        match save_workspace_file(&folder, &name, &request.body) {
            Ok(path) => Response::json(200, &json!({ "path": path })),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Response::json(404, &json!({ "error": "no such workspace" })),
            Err(_) => Response::json(500, &json!({ "error": "not saved" })),
        }
    }

    /// `{"port": 3000}`: a dev server a terminal runs, to be shown on the tailnet. Only a port the
    /// overview lists (a server started in a terminal the page may see); the app answers through
    /// the overview, where the port gets its address once it is served.
    fn open_dev_server(&self, request: &Request) -> Response {
        #[derive(Deserialize)]
        struct Body {
            port: u16,
        }
        let Ok(body) = serde_json::from_slice::<Body>(&request.body) else {
            return Response::json(400, &json!({ "error": "bad request" }));
        };
        if !self.dev_port_allowed(body.port) {
            return Response::json(404, &json!({ "error": "no such server" }));
        }
        self.queue(Command::OpenPort { port: body.port })
    }

    /// Hands a command to the app's next round.
    fn queue(&self, command: Command) -> Response {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        if inbox.len() >= 256 {
            return Response::json(429, &json!({ "error": "busy" }));
        }
        // Asked twice before the app's round (a double tap): once is enough.
        if matches!(command, Command::Wake { .. } | Command::OpenPort { .. }) && inbox.contains(&command) {
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
        let mut rows = super::snapshot::Sent::default();
        let mut last_write = Instant::now();
        loop {
            if self.stop.load(Ordering::SeqCst)
                || self.paused.load(Ordering::SeqCst)
                || started.elapsed() > STREAM_LIFETIME
                || stream.peer_gone()
            {
                break;
            }
            if !self.auth.lock().unwrap_or_else(|e| e.into_inner()).sessions.check_idle(token, login, SystemTime::now()) {
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
pub(super) fn strip_secret(path: &str, secret: &str) -> Option<String> {
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

/// `name` as a file name to save under: a plain name only (no separators, no `..`), with what a
/// file system would trip on made `_`, and no hidden name. `None` when it is not one.
fn workspace_file_name(name: &str) -> Option<String> {
    if name.is_empty() || name.contains(['/', '\\', '\0']) || name.contains("..") {
        return None;
    }
    // The returned path is pasted into a terminal prompt, so only letters, digits, spaces and `._-`
    // stay: `$(…)`, backticks, `;`, quotes and the like would be shell syntax there.
    let clean: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-') { c } else { '_' }).collect();
    let clean = clean.trim();
    if clean.starts_with('.') || clean.starts_with('-') {
        return None;
    }
    let Some((stem, extension)) = clean.rsplit_once('.') else {
        let stem: String = clean.chars().take(MAX_WORKSPACE_FILE_NAME).collect();
        return (!stem.is_empty()).then_some(stem);
    };
    let stem: String = stem.trim().chars().take(MAX_WORKSPACE_FILE_NAME).collect();
    let extension: String = extension.chars().take(16).collect();
    (!stem.is_empty() && !extension.is_empty()).then(|| format!("{stem}.{extension}"))
}

/// Writes `bytes` as `name` (or `name-2`, … when taken) in `.agentty-remote/` of the workspace's
/// folder and returns the path from that folder.
fn save_workspace_file(folder: &std::path::Path, name: &str, bytes: &[u8]) -> std::io::Result<String> {
    use std::io::Write;
    if !folder.is_absolute() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    let folder = folder.canonicalize()?;
    if !folder.is_dir() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    let dir = folder.join(WORKSPACE_FILE_DIR);
    // A link named `.agentty-remote` (say, in a cloned repository) must not send the upload elsewhere.
    if std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "a link where the upload folder goes"));
    }
    std::fs::create_dir_all(&dir)?;
    if !dir.canonicalize()?.starts_with(&folder) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the upload folder is outside the workspace"));
    }
    // Uploads are the person's own documents: keep them out of commits (`git add -A` in the project).
    let ignore = dir.join(".gitignore");
    if std::fs::symlink_metadata(&ignore).is_err() {
        let _ = std::fs::write(&ignore, "*\n");
    }
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    for n in 1..=1000 {
        let file = if n == 1 { name.to_string() } else { format!("{stem}-{n}.{extension}") };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(dir.join(&file)) {
            Ok(mut out) => {
                // A half-written file is removed, so a failed upload leaves nothing behind.
                if let Err(err) = out.write_all(bytes) {
                    drop(out);
                    let _ = std::fs::remove_file(dir.join(&file));
                    return Err(err);
                }
                return Ok(format!("{WORKSPACE_FILE_DIR}/{file}"));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Err(std::io::ErrorKind::AlreadyExists.into())
}

/// Where images attached on the page are kept, in Agentty's own (private) data folder.
fn attachments_dir() -> std::path::PathBuf {
    agentty_bridge::fsutil::data_dir().join("remote-attachments")
}

/// Images one prompt may carry.
const MAX_IMAGES: usize = 10;

/// The images a prompt names, when every one is a file `/api/attach` saved in `dir` (by the name
/// it gives, no other folder or file); `None` otherwise.
fn attached_images(dir: &std::path::Path, paths: &[String]) -> Option<Vec<std::path::PathBuf>> {
    if paths.len() > MAX_IMAGES {
        return None;
    }
    paths
        .iter()
        .map(|path| {
            let path = std::path::Path::new(path);
            let name = path.file_name()?.to_str()?;
            let ours = name.starts_with("image-") && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
            (ours && path.parent() == Some(dir) && path.is_file()).then(|| path.to_path_buf())
        })
        .collect()
}

/// The file extension of an image an agent CLI reads, from its first bytes; `None` for anything else.
fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Removes attached images older than `ATTACHMENT_LIFETIME` (files only, never a folder).
fn remove_old_attachments(dir: &std::path::Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let old = meta.modified().ok().and_then(|at| now.duration_since(at).ok()).is_some_and(|age| age > ATTACHMENT_LIFETIME);
        if meta.is_file() && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// One of a bounded number of places, given back when dropped.
struct Slot(&'static AtomicUsize);

impl Slot {
    fn take(taken: &'static AtomicUsize, max: usize) -> Option<Self> {
        taken.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < max).then_some(n + 1)).ok().map(|_| Self(taken))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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
    // The Agentty app names itself in its user agent, so its devices show as the app, not a browser.
    let browser = if agent.contains("AgenttyRemote/") { "Agentty Remote" } else { browser };
    format!("{browser} · {system}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{Ipv4Addr, TcpStream};

    const HOST: &str = "mac.example.ts.net:8743";
    const OWNER: &str = "me@example.com";

    #[test]
    fn only_so_many_refusals_linger_at_once() {
        static TAKEN: AtomicUsize = AtomicUsize::new(0);
        let held: Vec<_> = (0..3).map(|_| Slot::take(&TAKEN, 3).expect("a free slot")).collect();
        assert!(Slot::take(&TAKEN, 3).is_none(), "every slot is taken");
        drop(held);
        assert!(Slot::take(&TAKEN, 3).is_some(), "given back when done");
    }

    fn hub(password: &str) -> Arc<Hub> {
        let config = Config { owner: Some(OWNER.into()), hosts: vec![HOST.into()], https: true, secret: None, folders: None };
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
            dir: std::path::PathBuf::new(),
            roots: Vec::new(),
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
        let config = Config { owner: Some(OWNER.into()), hosts: vec![HOST.into()], https: true, secret: None, folders: None };
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
        let config =
            Config { owner: Some(OWNER.into()), hosts: vec![HOST.into()], https: true, secret: Some("s3cr3t".into()), folders: None };
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
    fn an_attached_image_is_checked_before_and_after_its_body() {
        let hub = hub("secret-password");
        hub.publish(Vec::new(), vec![session(7)], Vec::new());
        let cookie = sign_in(&hub, "secret-password");
        let head_only = |req: Req| req.raw().replace("Content-Length: 2\r\n\r\n{}", "Content-Length: 4000000\r\n\r\n");
        let attach = |pane: u64| Req::post(if pane == 7 { "/api/attach?pane=7" } else { "/api/attach?pane=8" }, json!({}));
        assert_eq!(send(&hub, &head_only(attach(7))).0, 401, "no session");
        assert_eq!(send(&hub, &head_only(Req { cookie: Some(cookie.clone()), marker: false, ..attach(7) })).0, 403, "not from the page");
        // Signed in, the body is read: a terminal that is not a session, then bytes that are not an
        // image (whatever the page calls them), are refused.
        assert_eq!(send(&hub, &Req { cookie: Some(cookie.clone()), ..attach(8) }.raw()).0, 404);
        assert_eq!(send(&hub, &Req { cookie: Some(cookie), ..attach(7) }.raw()).0, 415);
        hub.shutdown();
    }

    #[test]
    fn images_are_told_by_their_bytes_and_cleared_when_old() {
        assert_eq!(image_extension(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(image_extension(&[0xff, 0xd8, 0xff, 0xe0]), Some("jpg"));
        assert_eq!(image_extension(b"GIF89a.."), Some("gif"));
        assert_eq!(image_extension(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(image_extension(b"<svg onload=x>"), None);
        assert_eq!(image_extension(b"RIFF\0\0\0\0WAVE"), None);
        let dir = std::env::temp_dir().join(format!("agentty-attach-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("image.png"), b"png").unwrap();
        remove_old_attachments(&dir, SystemTime::now());
        assert!(dir.join("image.png").exists(), "a new one stays");
        remove_old_attachments(&dir, SystemTime::now() + ATTACHMENT_LIFETIME + Duration::from_secs(60));
        assert!(!dir.join("image.png").exists(), "an old one goes");
        assert!(dir.join("folder").exists(), "folders are left alone");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_prompt_names_only_images_the_page_attached() {
        let dir = std::env::temp_dir().join(format!("agentty-attached-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("image-1-0.png"), b"png").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let path = |name: &str| dir.join(name).display().to_string();
        assert_eq!(attached_images(&dir, &[]), Some(Vec::new()));
        assert_eq!(attached_images(&dir, &[path("image-1-0.png")]), Some(vec![dir.join("image-1-0.png")]));
        assert_eq!(attached_images(&dir, &[path("notes.txt")]), None, "not one it saved");
        assert_eq!(attached_images(&dir, &[path("image-9-9.png")]), None, "not there");
        assert_eq!(attached_images(&dir, &[path("../image-1-0.png")]), None, "another folder");
        assert_eq!(attached_images(&dir, &["/etc/passwd".into()]), None);
        assert_eq!(attached_images(&dir, &vec![path("image-1-0.png"); MAX_IMAGES + 1]), None, "too many");
        std::fs::remove_dir_all(dir).ok();
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
            root: std::path::PathBuf::new(),
            sleeping: true,
            active_tab: 0,
            tabs: Vec::new(),
            ports: Vec::new(),
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
    fn opens_workspaces_in_the_remote_folder_only() {
        let hub = hub("secret-password");
        let cookie = sign_in(&hub, "secret-password");
        let get = |path: &str| send(&hub, &Req { cookie: Some(cookie.clone()), ..Req::get(path) }.raw());
        let act = |body: serde_json::Value| send(&hub, &Req { cookie: Some(cookie.clone()), ..Req::post("/api/workspace", body) }.raw()).0;
        assert_eq!(get("/api/folders").0, 404, "no remote folder: nothing to list");
        assert_eq!(act(json!({ "action": "open", "path": "", "tool": "shell" })), 404);
        assert_eq!(act(json!({ "action": "mkdir", "path": "", "name": "app" })), 404);

        let base = std::env::temp_dir().join(format!("agentty-remote-api-{}-{}", std::process::id(), crate::ui::now_ms()));
        let root = base.join("AgenttyRemote");
        let config = hub.config.read().unwrap().clone();
        hub.set_config(Config { folders: Some(root.clone()), ..config });
        let (status, _, body) = get("/api/folders");
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"folders\":[]"), "made on first use: {body}");
        assert_eq!(act(json!({ "action": "mkdir", "path": "", "name": "app" })), 200);
        assert_eq!(act(json!({ "action": "mkdir", "path": "", "name": "app" })), 409, "already there");
        assert_eq!(act(json!({ "action": "mkdir", "path": "app", "name": "web" })), 200);
        assert_eq!(act(json!({ "action": "mkdir", "path": "", "name": "x", "tool": "shell" })), 400, "making a folder opens nothing");
        assert_eq!(act(json!({ "action": "open", "path": "app", "tool": "claude" })), 200);
        assert_eq!(act(json!({ "action": "open", "path": "app/web", "tool": "codex", "group": "Phone", "title": "phone-test" })), 200);
        for title in [json!(""), json!("t".repeat(MAX_TITLE_NAME + 1)), json!("a\nb"), json!(" a"), json!(3)] {
            assert_eq!(act(json!({ "action": "open", "path": "app", "tool": "shell", "title": title })), 400, "{title}");
        }
        assert_eq!(act(json!({ "action": "mkdir", "path": "", "name": "z", "title": "x" })), 400, "a title is for opening");
        for group in [json!(""), json!("g".repeat(MAX_GROUP_NAME + 1)), json!("a\nb"), json!(" a"), json!(3)] {
            assert_eq!(act(json!({ "action": "open", "path": "app", "tool": "shell", "group": group })), 400, "{group}");
        }
        assert_eq!(act(json!({ "action": "mkdir", "path": "", "name": "y", "group": "Phone" })), 400, "a group is for opening");
        assert!(get("/api/folders").2.contains("\"folders\":[\"app\"]"));
        assert!(get("/api/folders?path=app").2.contains("\"folders\":[\"web\"]"));
        for path in ["..", "../..", "/etc", "app/..", ".ssh", "missing"] {
            assert_eq!(act(json!({ "action": "open", "path": path, "tool": "shell" })), 400, "{path}");
            assert_eq!(get(&format!("/api/folders?path={path}")).0, 404, "{path}");
        }
        assert_eq!(act(json!({ "action": "mkdir", "path": "", "name": "../out" })), 400);
        assert_eq!(act(json!({ "action": "open", "path": "app", "tool": "rm -rf" })), 400, "only the known tools");
        assert_eq!(act(json!({ "action": "open", "path": "app" })), 400, "opened with a known tool only");
        let made = root.canonicalize().unwrap().join("app");
        assert_eq!(
            hub.take_commands(),
            vec![
                Command::OpenFolder { folder: made.clone(), kind: PaneKind::Claude, group: None, title: None },
                Command::OpenFolder {
                    folder: made.join("web"),
                    kind: PaneKind::Codex,
                    group: Some("Phone".into()),
                    title: Some("phone-test".into()),
                }
            ]
        );
        let anonymous = send(&hub, &Req::get("/api/folders").raw()).0;
        assert_eq!(anonymous, 401, "needs a session");
        hub.shutdown();
        let _ = std::fs::remove_dir_all(base);
    }

    /// A file sent into a workspace's folder: only for a shown workspace, a known type, a plain name
    /// and a size within the limit.
    #[test]
    fn saves_files_in_the_workspace_folder() {
        let hub = hub("secret-password");
        let base = std::env::temp_dir().join(format!("agentty-remote-file-{}-{}", std::process::id(), crate::ui::now_ms()));
        std::fs::create_dir_all(&base).unwrap();
        let mut shown = workspace(3);
        shown.root = base.clone();
        // Its terminal has moved elsewhere: the file still goes to the workspace's own folder.
        shown.folder = std::env::temp_dir().display().to_string();
        hub.publish(vec![shown], Vec::new(), Vec::new());
        let cookie = sign_in(&hub, "secret-password");
        let upload = |path: &str, body: &str| {
            let req = Req { cookie: Some(cookie.clone()), body: body.to_string(), ..Req::post(path, json!({})) };
            send(&hub, &req.raw())
        };
        let (status, _, body) = upload("/api/workspace/file?workspace=3&name=notes.txt", "hello");
        assert_eq!((status, body.as_str()), (200, "{\"path\":\".agentty-remote/notes.txt\"}"));
        assert_eq!(std::fs::read_to_string(base.join(".agentty-remote/notes.txt")).unwrap(), "hello");
        let (_, _, again) = upload("/api/workspace/file?workspace=3&name=notes.txt", "second");
        assert_eq!(again, "{\"path\":\".agentty-remote/notes-2.txt\"}", "never over a file already there");
        assert_eq!(std::fs::read_to_string(base.join(".agentty-remote/notes.txt")).unwrap(), "hello");
        let (status, _, body) = upload("/api/workspace/file?workspace=3&name=Trip%20Photo.PNG", "x");
        assert_eq!((status, body.as_str()), (200, "{\"path\":\".agentty-remote/Trip Photo.PNG\"}"), "the extension is matched in any case");

        for name in ["", "../notes.txt", "..%2Fnotes.txt", "a/b.txt", "a%5Cb.txt", "notes..txt", ".txt", "%00.txt"] {
            assert_eq!(upload(&format!("/api/workspace/file?workspace=3&name={name}"), "x").0, 400, "{name:?}");
        }
        assert_eq!(upload("/api/workspace/file?workspace=3", "x").0, 400, "no name");
        assert_eq!(upload("/api/workspace/file?name=notes.txt", "x").0, 400, "no workspace");
        assert_eq!(upload("/api/workspace/file?workspace=3&name=notes.txt", "").0, 400, "no bytes");
        assert_eq!(upload("/api/workspace/file?workspace=4&name=notes.txt", "x").0, 404, "a workspace the page is not shown");
        for name in ["run.sh", "app.exe", "notes", "notes.txt.sh", "page.html"] {
            assert_eq!(upload(&format!("/api/workspace/file?workspace=3&name={name}"), "x").0, 415, "{name}");
        }
        for name in
            ["a.pdf", "a.jpg", "a.jpeg", "a.gif", "a.webp", "a.heic", "a.md", "a.csv", "a.json", "a.doc", "a.docx", "a.xls", "a.xlsx"]
        {
            assert_eq!(upload(&format!("/api/workspace/file?workspace=3&name={name}"), "x").0, 200, "{name}");
        }
        for name in ["a.ppt", "a.pptx", "a.rtf", "a.zip"] {
            assert_eq!(upload(&format!("/api/workspace/file?workspace=3&name={name}"), "x").0, 200, "{name}");
        }
        // Too large is told from the head, before the body is read.
        let head = |len: usize| {
            let req = Req { cookie: Some(cookie.clone()), ..Req::post("/api/workspace/file?workspace=3&name=big.zip", json!({})) };
            req.raw().replace("Content-Length: 2\r\n\r\n{}", &format!("Content-Length: {len}\r\n\r\n"))
        };
        assert_eq!(send(&hub, &head(http::FILE_MAX_BODY + 1)).0, 413);
        let head_only = |req: Req| req.raw().replace("Content-Length: 2\r\n\r\n{}", "Content-Length: 4000000\r\n\r\n");
        let file = || Req::post("/api/workspace/file?workspace=3&name=a.pdf", json!({}));
        assert_eq!(send(&hub, &head_only(file())).0, 401, "no session");
        assert_eq!(send(&hub, &head_only(Req { cookie: Some(cookie.clone()), marker: false, ..file() })).0, 403, "not from the page");
        let slots: Vec<_> = (0..MAX_ATTACHING).map(|_| Slot::take(&ATTACHING, MAX_ATTACHING).unwrap()).collect();
        assert_eq!(send(&hub, &head_only(Req { cookie: Some(cookie.clone()), ..file() })).0, 429, "as many as image attachments");
        drop(slots);
        // Bodies still arriving count too, from the head on: they are refused before being read.
        let receiving: Vec<_> = (0..MAX_ATTACHING).map(|_| Slot::take(&RECEIVING, MAX_ATTACHING).unwrap()).collect();
        assert_eq!(send(&hub, &head_only(Req { cookie: Some(cookie.clone()), ..file() })).0, 429, "as many bodies at once");
        drop(receiving);
        hub.shutdown();
        let _ = std::fs::remove_dir_all(base);
    }

    /// A file a terminal's output names: only for a signed-in page, only from that terminal's
    /// folders, as its own kind, and sandboxed when opened on its own.
    #[test]
    fn files_named_in_a_terminal_open_for_a_signed_in_page() {
        let base = std::env::temp_dir().join(format!("agentty-remote-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("project/shots")).unwrap();
        std::fs::create_dir_all(base.join("other")).unwrap();
        std::fs::write(base.join("project/shots/a b.png"), b"PNG").unwrap();
        std::fs::write(base.join("project/notes.md"), "# Notes").unwrap();
        std::fs::write(base.join("other/secret.txt"), "not yours").unwrap();
        let project = base.canonicalize().unwrap().join("project");
        let hub = hub("secret-password");
        let mine = SessionInfo { dir: project.clone(), roots: vec![project.clone()], ..session(7) };
        let other = SessionInfo { dir: base.join("other"), roots: vec![base.join("other")], ..session(8) };
        hub.publish(Vec::new(), vec![mine, other], Vec::new());
        let file = |pane: u64, path: &str| format!("/api/file?pane={pane}&path={}", percent_encode(path));
        assert_eq!(send(&hub, &Req::get(&file(7, "notes.md")).raw()).0, 401, "needs a session");
        let cookie = sign_in(&hub, "secret-password");
        let get = |path: String| send(&hub, &Req { cookie: Some(cookie.clone()), ..Req::get(&path) }.raw());
        let (status, head, body) = get(file(7, "notes.md"));
        assert_eq!(status, 200);
        assert!(head.contains("Content-Type: text/plain; charset=utf-8") && head.contains("X-Agentty-Kind: markdown"), "{head}");
        assert!(head.contains("sandbox"), "{head}");
        assert_eq!(body, "# Notes");
        let (status, head, _) = get(file(7, "shots/a b.png"));
        assert_eq!(status, 200);
        assert!(head.contains("Content-Type: image/png") && head.contains("filename*=UTF-8''a%20b.png"), "{head}");
        assert_eq!(get(file(7, "../other/secret.txt")).0, 404, "another terminal's folder");
        assert_eq!(get(file(9, "notes.md")).0, 404, "no such terminal");
        assert_eq!(get("/api/file?pane=7".into()).0, 400);
        let (status, head, body) =
            send(&hub, &Req { method: "HEAD", cookie: Some(cookie.clone()), ..Req::get(&file(7, "shots/a b.png")) }.raw());
        assert_eq!((status, body.as_str()), (200, ""), "HEAD: the answer without the file");
        assert!(head.contains("X-Agentty-Kind: image"), "{head}");
        hub.shutdown();
        let _ = std::fs::remove_dir_all(base);
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
        assert_eq!(input(json!({ "pane": 7, "kind": "scroll", "lines": 3 })), 200);
        assert_eq!(input(json!({ "pane": 7, "kind": "scroll", "lines": 50 })), 200);
        assert_eq!(input(json!({ "pane": 7, "kind": "scroll", "lines": 20 })), 200);
        assert_eq!(input(json!({ "pane": 7, "kind": "scroll", "lines": 0 })), 400, "no scrolling");
        assert_eq!(input(json!({ "pane": 7, "kind": "scroll", "lines": -500 })), 400, "too far at once");
        assert_eq!(
            hub.take_commands(),
            vec![
                Command::Prompt { pane: 7, text: "run the tests".into(), images: Vec::new() },
                Command::Chat { pane: 7, text: "add a login page".into() },
                Command::Key { pane: 7, key: "c".into(), ctrl: true, alt: false, shift: false },
                Command::Resize { pane: 7, cols: 50, rows: 30 },
                Command::Scroll { pane: 7, lines: SCROLL_LINES }
            ],
            "the newer size replaced the one still waiting, and scrolls in a row joined up to a swipe's worth"
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
        let config = Config { owner: Some(OWNER.into()), hosts: vec![HOST.into()], https: true, secret: None, folders: None };
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
            history: 0,
            program_scrolls: false,
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
        assert_eq!(device_name("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AgenttyRemote/1.0"), "Agentty Remote · iPhone");
    }

    #[test]
    fn upload_names_carry_no_shell_syntax() {
        assert_eq!(workspace_file_name("$(curl x.sh|sh).pdf").as_deref(), Some("__curl x.sh_sh_.pdf"));
        assert_eq!(workspace_file_name("a`id`;b&c'd\".txt").as_deref(), Some("a_id__b_c_d_.txt"));
        assert_eq!(workspace_file_name("기획 메모-1.md").as_deref(), Some("기획 메모-1.md"));
        assert_eq!(workspace_file_name("-rf.txt"), None);
        assert_eq!(workspace_file_name(".env"), None);
        assert_eq!(workspace_file_name("../x.txt"), None);
        let long = format!("{}.pdf", "가".repeat(200));
        assert!(workspace_file_name(&long).is_some_and(|n| n.len() <= 255));
    }
}
