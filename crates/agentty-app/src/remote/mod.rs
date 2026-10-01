//! Remote access: the user's own browser, on another device in their Tailscale network, sees the
//! workspaces and their terminals, gets told when an agent finishes or asks, and types into them.
//!
//! The pieces:
//! - `server`: the web server on `127.0.0.1` and every check a request passes;
//! - `auth`: the web password, sessions and lockout;
//! - `tailscale`: the machine's tailnet name and owner, and `tailscale serve` for one port;
//! - this module: starts and stops all of it with the setting, and every few hundred milliseconds
//!   copies sessions and watched screens into the server and carries the page's input into the
//!   terminals, on the UI thread, the way typing would.
//!
//! It is off until the user sets a password and turns it on. A debug build started with
//! `AGENTTY_DEBUG=1 AGENTTY_REMOTE_DEV=1` serves on `http://127.0.0.1:<port>` without Tailscale, for
//! testing; a release build has no such mode (`dev_mode`).

pub mod auth;
pub mod http;
pub mod server;
pub mod snapshot;
pub mod tailscale;

use crate::settings::{settings, update_settings};
use futures::StreamExt;
use gpui::{App, AsyncApp, Global};
use server::{Alert, Config, Hub};
use std::sync::Arc;
use std::time::Duration;

const KEYCHAIN_SERVICE: &str = "run.agentty.remote";
const KEYCHAIN_ACCOUNT: &str = "password";
const TICK: Duration = Duration::from_millis(200);
const FAST_TICK: Duration = Duration::from_millis(30);
const FAST_FOR: Duration = Duration::from_millis(1500);

/// Why remote access is not on.
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    NotInstalled,
    NotRunning,
    /// MagicDNS / HTTPS certificates are off for the tailnet.
    NoHttps,
    NoPassword,
    /// Serve is off for the tailnet: the owner turns it on at this Tailscale address.
    NeedsEnabling(String),
    Funnel,
    /// Something else is already served on this tailnet port.
    PortTaken(u16),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Phase {
    #[default]
    Off,
    Starting,
    On {
        url: String,
    },
    Failed(Problem),
}

#[derive(Default)]
pub struct Remote {
    hub: Option<Arc<Hub>>,
    pub phase: Phase,
    pub tailscale: Option<tailscale::Status>,
    /// The tailnet port `serve` was turned on for, to turn off again.
    served_port: Option<u16>,
    keep_awake: Option<std::process::Child>,
    /// Whether a password is saved; `None` until looked up.
    pub password_set: Option<bool>,
    /// Bumped by every start and stop, so a start that finishes late does not undo a stop.
    generation: u64,
    /// The Tailscale login and machine name the running server was started for: if either
    /// changes (another account signs in on this Mac), it starts over for the new one.
    watched_tailscale: (Option<String>, Option<String>),
    /// When Tailscale was last asked, while on.
    last_check: Option<std::time::Instant>,
    /// Signals the round loop; the server holds a copy to wake it on input.
    waker: Option<futures::channel::mpsc::UnboundedSender<()>>,
    /// When the page last sent input: rounds run fast for a moment after.
    last_input: Option<std::time::Instant>,
    /// When the dashboard was last redrawn for its live counters.
    last_dashboard: Option<std::time::Instant>,
    /// When the workspace list was last built: at most every TICK, even in fast rounds.
    last_overview: Option<std::time::Instant>,
}

impl Global for Remote {}

/// The local test mode: only in a debug build, and only with both variables set. A release never
/// serves without Tailscale's identity check, whatever its environment says.
fn dev_mode() -> bool {
    cfg!(debug_assertions)
        && std::env::var("AGENTTY_DEBUG").as_deref() == Ok("1")
        && std::env::var("AGENTTY_REMOTE_DEV").as_deref() == Ok("1")
}

fn keychain_service() -> String {
    agentty_bridge::connectors::scoped_service(KEYCHAIN_SERVICE, std::env::var_os("AGENTTY_DATA_DIR").as_deref())
}

fn load_password() -> Option<String> {
    agentty_bridge::secret_store::load(&keychain_service(), KEYCHAIN_ACCOUNT).ok().filter(|h| h.starts_with("pbkdf2-"))
}

pub fn init(cx: &mut App) {
    cx.set_global(Remote::default());
    // A previous run that crashed (or was killed) never turned its serve off. Starting again takes
    // it over (`serve_on`); otherwise it goes now, before anything else can take its local port.
    if !(settings(cx).remote.feature && settings(cx).remote.enabled) {
        if let Some(port) = tailscale::leftover_port() {
            cx.background_executor().spawn(async move { tailscale::serve_off(port) }).detach();
        }
    }
    cx.spawn(async move |cx| {
        let saved = cx.background_executor().spawn(async { load_password().is_some() }).await;
        let _ = cx.update(|cx| {
            cx.global_mut::<Remote>().password_set = Some(saved);
            if settings(cx).remote.feature && settings(cx).remote.enabled {
                start(cx);
            }
        });
        // A round every TICK, and at once when the page sends input; for a moment after input,
        // rounds come every FAST_TICK so what was typed shows on the page as soon as it echoes.
        let (waker, mut woken) = futures::channel::mpsc::unbounded::<()>();
        let _ = cx.update(|cx| cx.global_mut::<Remote>().waker = Some(waker));
        loop {
            let busy = cx.update(|cx| cx.global::<Remote>().last_input.is_some_and(|at| at.elapsed() < FAST_FOR)).unwrap_or(false);
            let timer = cx.background_executor().timer(if busy { FAST_TICK } else { TICK });
            futures::select_biased! {
                _ = woken.next() => {}
                _ = futures::FutureExt::fuse(timer) => {}
            }
            if cx.update(tick).is_err() {
                break;
            }
        }
    })
    .detach();
    cx.on_app_quit(|cx| {
        let remote = cx.global_mut::<Remote>();
        let port = remote.served_port.take();
        if let Some(hub) = remote.hub.take() {
            hub.shutdown();
        }
        if let Some(mut child) = remote.keep_awake.take() {
            let _ = child.kill();
        }
        async move {
            if let Some(port) = port {
                tailscale::serve_off(port);
            }
        }
    })
    .detach();
}

/// Turns remote access on or off and remembers it.
pub fn set_enabled(on: bool, cx: &mut App) {
    update_settings(cx, move |s| s.remote.enabled = on);
    if on && settings(cx).remote.feature {
        start(cx);
    } else {
        stop(cx);
    }
}

/// The feature itself on or off (Settings → General): off hides its activity-bar item and page
/// and turns remote access off; on brings the item back (remote access stays off until turned on).
pub fn set_feature(on: bool, cx: &mut App) {
    update_settings(cx, move |s| s.remote.feature = on);
    if !on {
        set_enabled(false, cx);
        // After the current update: this is called from the page's own button, while that
        // workbench is still being updated, and updating it again from inside would panic.
        cx.defer(|cx| {
            for window in crate::workbenches(cx) {
                let _ = window.update(cx, |wb, _, cx| wb.close_remote_page(cx));
            }
        });
    }
    cx.refresh_windows();
}

pub fn refresh_tailscale(cx: &mut App) {
    cx.spawn(async move |cx| {
        let status = cx.background_executor().spawn(async { tailscale::status() }).await;
        let _ = cx.update(|cx| {
            cx.global_mut::<Remote>().tailscale = Some(status);
            cx.refresh_windows();
        });
    })
    .detach();
}

fn fail(problem: Problem, cx: &mut App) {
    cx.global_mut::<Remote>().phase = Phase::Failed(problem);
    cx.refresh_windows();
}

pub fn start(cx: &mut App) {
    stop_serving(cx);
    let remote = cx.global_mut::<Remote>();
    remote.generation += 1;
    let generation = remote.generation;
    remote.phase = Phase::Starting;
    let https_port = settings(cx).remote.https_port;
    let keep_awake = settings(cx).remote.keep_awake;
    cx.refresh_windows();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let dev = dev_mode();
        let (status, password) = cx
            .background_executor()
            .spawn(async move { (if dev { tailscale::Status::default() } else { tailscale::status() }, load_password()) })
            .await;
        let current = |cx: &mut AsyncApp| cx.update(|cx| cx.global::<Remote>().generation == generation).unwrap_or(false);
        if !current(cx) {
            return;
        }
        let checked = (|| {
            let password = password.ok_or(Problem::NoPassword)?;
            if dev {
                return Ok((Config { owner: None, hosts: Vec::new(), https: false }, password));
            }
            if !status.installed {
                return Err(Problem::NotInstalled);
            }
            let (true, Some(dns), Some(login)) = (status.running, status.dns_name.clone(), status.login.clone()) else {
                return Err(Problem::NotRunning);
            };
            if !status.https {
                return Err(Problem::NoHttps);
            }
            let mut hosts = vec![format!("{dns}:{https_port}")];
            if https_port == 443 {
                hosts.push(dns);
            }
            Ok((Config { owner: Some(login), hosts, https: true }, password))
        })();
        let _ = cx.update(|cx| cx.global_mut::<Remote>().tailscale = Some(status.clone()));
        let (config, password) = match checked {
            Ok(found) => found,
            Err(problem) => {
                let _ = cx.update(|cx| fail(problem, cx));
                return;
            }
        };
        let hub = match Hub::start(config.clone(), Some(password)) {
            Ok(hub) => hub,
            Err(err) => {
                let _ = cx.update(|cx| fail(Problem::Other(err.to_string()), cx));
                return;
            }
        };
        let local_port = hub.port;
        let (url, served) = if dev {
            hub.set_config(Config { hosts: vec![format!("127.0.0.1:{local_port}"), format!("localhost:{local_port}")], ..config });
            (format!("http://127.0.0.1:{local_port}/"), None)
        } else {
            let epoch = match cx.background_executor().spawn(async move { tailscale::serve_on(https_port, local_port) }).await {
                Ok(epoch) => epoch,
                Err(error) => {
                    hub.shutdown();
                    let problem = match error {
                        tailscale::ServeError::NeedsEnabling(url) => Problem::NeedsEnabling(url),
                        tailscale::ServeError::Funnel => Problem::Funnel,
                        tailscale::ServeError::PortTaken => Problem::PortTaken(https_port),
                        tailscale::ServeError::Failed(text) => Problem::Other(text),
                    };
                    let _ = cx.update(|cx| fail(problem, cx));
                    return;
                }
            };
            let host = status.dns_name.clone().unwrap_or_default();
            let url = if https_port == 443 { format!("https://{host}/") } else { format!("https://{host}:{https_port}/") };
            (url, Some((https_port, epoch)))
        };
        let _ = cx.update(|cx| {
            let remote = cx.global_mut::<Remote>();
            if remote.generation != generation {
                // Turned off while it was starting: take down this start's serve only, not one a
                // newer start set up meanwhile.
                hub.shutdown();
                if let Some((port, epoch)) = served {
                    std::thread::spawn(move || tailscale::serve_off_if(port, epoch));
                }
                return;
            }
            if let Some(waker) = &remote.waker {
                hub.set_waker(waker.clone());
            }
            remote.hub = Some(hub);
            remote.served_port = served.map(|(port, _)| port);
            remote.watched_tailscale = (status.login.clone(), status.dns_name.clone());
            if keep_awake && cfg!(target_os = "macos") {
                // `-i`: no idle sleep while Agentty runs (`-w`); the display may still sleep.
                remote.keep_awake =
                    std::process::Command::new("/usr/bin/caffeinate").args(["-i", "-w", &std::process::id().to_string()]).spawn().ok();
            }
            remote.phase = Phase::On { url };
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Stops the server and `serve`, leaving the setting as it is.
fn stop_serving(cx: &mut App) {
    let remote = cx.global_mut::<Remote>();
    remote.generation += 1;
    if let Some(hub) = remote.hub.take() {
        hub.shutdown();
    }
    if let Some(mut child) = remote.keep_awake.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Some(port) = remote.served_port.take() {
        cx.background_executor().spawn(async move { tailscale::serve_off(port) }).detach();
    }
    // Terminals sized for a page go back to their panes. Deferred: this often runs from a
    // button on the workbench, inside an update of it.
    cx.defer(|cx| {
        for window in crate::workbenches(cx) {
            let _ = window.update(cx, |wb, _, cx| wb.remote_release_all(cx));
        }
    });
}

pub fn stop(cx: &mut App) {
    stop_serving(cx);
    cx.global_mut::<Remote>().phase = Phase::Off;
    cx.refresh_windows();
}

/// Saves a new web password (hashed) and ends every session. `done` gets an error to show.
pub fn set_password(password: String, cx: &mut App, done: impl FnOnce(Result<(), String>, &mut App) + 'static) {
    if let Some(problem) = auth::password_problem(&password) {
        // Answered after the current update, like the success below: the caller is usually a
        // button's handler, inside an update of the workbench that `done` updates again.
        let message = crate::i18n::t(cx, problem).to_string();
        return cx.defer(move |cx| done(Err(message), cx));
    }
    cx.spawn(async move |cx| {
        let saved = cx
            .background_executor()
            .spawn(async move {
                let hash = auth::hash_password(&password);
                agentty_bridge::secret_store::store(&keychain_service(), KEYCHAIN_ACCOUNT, &hash).map(|_| hash)
            })
            .await;
        let _ = cx.update(|cx| match saved {
            Ok(hash) => {
                let remote = cx.global_mut::<Remote>();
                remote.password_set = Some(true);
                if let Some(hub) = &remote.hub {
                    hub.set_password(Some(hash));
                }
                if matches!(remote.phase, Phase::Failed(Problem::NoPassword)) && settings(cx).remote.enabled {
                    start(cx);
                }
                done(Ok(()), cx);
                cx.refresh_windows();
            }
            Err(err) => done(Err(format!("{err:#}")), cx),
        });
    })
    .detach();
}

/// Forgets the password: remote access turns off, since it can't be used without one.
pub fn remove_password(cx: &mut App) {
    cx.background_executor().spawn(async { agentty_bridge::secret_store::delete(&keychain_service(), KEYCHAIN_ACCOUNT) }).detach();
    cx.global_mut::<Remote>().password_set = Some(false);
    set_enabled(false, cx);
}

pub fn sign_out_everywhere(cx: &mut App) {
    if let Some(hub) = &cx.global::<Remote>().hub {
        hub.sign_out_everywhere();
    }
    cx.refresh_windows();
}

/// The server, while it runs (for the settings page's devices and log).
pub fn hub(cx: &App) -> Option<Arc<Hub>> {
    cx.try_global::<Remote>().and_then(|r| r.hub.clone())
}

/// Once a minute while on: if the Mac's Tailscale login or name changed, the server starts over
/// for the new ones (every session ends; the old login is let in no more).
fn watch_tailscale(cx: &mut App) {
    let remote = cx.global_mut::<Remote>();
    if remote.served_port.is_none() || remote.last_check.is_some_and(|at| at.elapsed() < Duration::from_secs(60)) {
        return;
    }
    remote.last_check = Some(std::time::Instant::now());
    let generation = remote.generation;
    let expected = remote.watched_tailscale.clone();
    cx.spawn(async move |cx| {
        let status = cx.background_executor().spawn(async { tailscale::status() }).await;
        // Not connected right now: nobody reaches the page anyway, so nothing to change.
        if !status.running {
            return;
        }
        let _ = cx.update(|cx| {
            if cx.global::<Remote>().generation == generation && (status.login.clone(), status.dns_name.clone()) != expected {
                start(cx);
            }
        });
    })
    .detach();
}

/// One round: the page's input into the terminals, then sessions and watched screens out.
fn tick(cx: &mut App) {
    let Some(hub) = hub(cx) else { return };
    watch_tailscale(cx);
    let windows = crate::workbenches(cx);
    let commands = hub.take_commands();
    if !commands.is_empty() {
        cx.global_mut::<Remote>().last_input = Some(std::time::Instant::now());
    }
    for command in commands {
        for window in &windows {
            let handled = window.update(cx, |wb, _, cx| wb.remote_apply(&command, cx)).unwrap_or(false);
            if handled {
                break;
            }
        }
    }
    // The screens being watched every round; the whole workspace list at most every TICK (fast
    // rounds are for echoing what was typed, not for re-reading every pane's state).
    let remote = cx.global_mut::<Remote>();
    let overview = remote.last_overview.is_none_or(|at| at.elapsed() >= TICK);
    if overview {
        remote.last_overview = Some(std::time::Instant::now());
    }
    let watched = hub.watched();
    let (mut workspaces, mut sessions) = (Vec::new(), Vec::new());
    for window in &windows {
        let _ = window.update(cx, |wb, _, cx| {
            if overview {
                let (w, s) = wb.remote_overview(cx);
                workspaces.extend(w);
                sessions.extend(s);
            }
            for pane in &watched {
                if let Some(screen) = wb.remote_screen(*pane, cx) {
                    hub.publish_screen(*pane, screen);
                }
            }
        });
    }
    if overview {
        hub.publish(workspaces, sessions);
    }
    // The dashboard's counters and log follow along, about once a second, while it is open.
    let remote = cx.global_mut::<Remote>();
    if remote.last_dashboard.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
        remote.last_dashboard = Some(std::time::Instant::now());
        for window in &windows {
            let _ = window.update(cx, |wb, _, cx| wb.refresh_remote_page(cx));
        }
    }
    for alert in hub.take_alerts() {
        let (title, body) = match alert {
            Alert::SignedIn { device } => (crate::i18n::t(cx, "remote.alert_signed_in").to_string(), device),
            Alert::Locked { minutes } => (
                crate::i18n::t(cx, "remote.alert_locked_title").to_string(),
                crate::i18n::tf(cx, "remote.alert_locked", &[("minutes", &minutes.to_string())]),
            ),
        };
        crate::notifications::show(0, &title, &body);
    }
}
