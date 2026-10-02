//! Remote access: the user's own browser, on another device in their Tailscale network, sees the
//! workspaces and their terminals, gets told when an agent finishes or asks, and types into them.
//!
//! The pieces:
//! - `server`: the web server and every check a request passes;
//! - `conn`: where it listens (a Unix socket only Tailscale reaches, or `127.0.0.1`);
//! - `auth`: the web password, sessions and lockout;
//! - `tailscale`: the machine's tailnet name and owner, and `tailscale serve` for one port;
//! - this module: starts and stops all of it with the setting, and every few hundred milliseconds
//!   copies sessions and watched screens into the server and carries the page's input into the
//!   terminals, on the UI thread, the way typing would.
//!
//! It is off until the user sets a password and turns it on, and it runs only while Tailscale
//! does: signed out, disconnected or quit, and the server and `serve` go down within seconds,
//! coming back by themselves once Tailscale is connected again. A debug build started with
//! `AGENTTY_DEBUG=1 AGENTTY_REMOTE_DEV=1` serves on `http://127.0.0.1:<port>` without Tailscale, for
//! testing; a release build has no such mode (`dev_mode`).

pub mod auth;
pub mod conn;
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
/// How often Tailscale is asked whether it still runs (and, while off for that, whether it is
/// back).
const TAILSCALE_CHECK: Duration = Duration::from_secs(3);
/// How often the tailnet port is checked for Funnel while on.
const FUNNEL_CHECK: Duration = Duration::from_secs(15);

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
    /// Servers stopped but still holding their ports (`Hub::pause`) until the Tailscale entry
    /// that pointed at them is surely gone: a new start replaced it, or the marker went.
    parked: Vec<Arc<Hub>>,
    pub phase: Phase,
    pub tailscale: Option<tailscale::Status>,
    /// The tailnet port `serve` was turned on for and its target, to turn off again.
    served: Option<(u16, String)>,
    keep_awake: Option<std::process::Child>,
    /// Whether a password is saved; `None` until looked up.
    pub password_set: Option<bool>,
    /// The password last saved here (hashed), and how many saves there have been: a start that
    /// read the Keychain before a save finished takes this one instead.
    saved_password: Option<String>,
    password_saves: u64,
    /// Bumped by every start and stop, so a start that finishes late does not undo a stop.
    generation: u64,
    /// The Tailscale login and machine name the running server was started for: if either
    /// changes (another account signs in on this Mac), it starts over for the new one.
    watched_tailscale: (Option<String>, Option<String>),
    /// When Tailscale was last asked, and whether an answer is still awaited.
    last_check: Option<std::time::Instant>,
    checking: bool,
    /// When Funnel was last checked for, while on.
    last_funnel_check: Option<std::time::Instant>,
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
    // it down first (`serve_on`); otherwise it goes now.
    if !(settings(cx).remote.feature && settings(cx).remote.enabled) {
        cx.background_executor().spawn(async { tailscale::clean_leftover() }).detach();
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
        let served = remote.served.take();
        if let Some(hub) = remote.hub.take() {
            hub.shutdown();
        }
        if let Some(mut child) = remote.keep_awake.take() {
            let _ = child.kill();
        }
        for old in remote.parked.drain(..) {
            old.shutdown();
        }
        // On a thread of its own: quitting waits for this future on the main thread, and the
        // Tailscale tool can take seconds. What doesn't finish before the app exits is still in
        // the marker, for the next launch to take down.
        let (done, finished) = futures::channel::oneshot::channel::<()>();
        let _ = std::thread::Builder::new().name("agentty-remote-quit".into()).spawn(move || {
            match served {
                Some((port, target)) => tailscale::serve_off(port, &target),
                // Quit while still starting: whatever that start recorded goes too.
                None => tailscale::clean_leftover(),
            }
            let _ = done.send(());
        });
        async move {
            let _ = finished.await;
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

/// Shows why it is not on, unless a newer start or stop came since `generation`.
fn fail(problem: Problem, generation: u64, cx: &mut App) {
    let remote = cx.global_mut::<Remote>();
    if remote.generation != generation {
        return;
    }
    remote.phase = Phase::Failed(problem);
    cx.refresh_windows();
}

/// Where the server listens: a Unix socket only this user's folder holds, which only Tailscale
/// (running as root) opens; `127.0.0.1` on Windows, in the local test mode, or with `tcp`.
fn listener(dev: bool, tcp: bool) -> std::io::Result<conn::Listener> {
    #[cfg(unix)]
    if !dev && !tcp {
        return conn::Listener::unix(&conn::socket_path(&agentty_bridge::fsutil::data_dir()));
    }
    let _ = (dev, tcp);
    conn::Listener::tcp()
}

pub fn start(cx: &mut App) {
    if !(settings(cx).remote.feature && settings(cx).remote.enabled) {
        return stop(cx);
    }
    stop_serving(cx);
    let remote = cx.global_mut::<Remote>();
    remote.generation += 1;
    let generation = remote.generation;
    remote.phase = Phase::Starting;
    let saves = remote.password_saves;
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
            // Signed out, disconnected or quit: no page without Tailscale.
            let password = password.ok_or(Problem::NoPassword)?;
            if dev {
                return Ok((Config { owner: None, hosts: Vec::new(), https: false, secret: None }, password));
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
            Ok((Config { owner: Some(login), hosts, https: true, secret: None }, password))
        })();
        let _ = cx.update(|cx| cx.global_mut::<Remote>().tailscale = Some(status.clone()));
        let (config, password) = match checked {
            Ok(found) => found,
            Err(problem) => {
                let _ = cx.update(|cx| {
                    // Saved while this start read the Keychain: start again with it.
                    if problem == Problem::NoPassword && cx.global::<Remote>().password_saves != saves {
                        return start(cx);
                    }
                    fail(problem, generation, cx)
                });
                return;
            }
        };
        // A server that may still have a serve entry pointing at it keeps its port (closed to
        // everyone) until that entry is surely gone; see `stop_serving`.
        let park = |hub: Arc<Hub>, cx: &mut AsyncApp| {
            hub.pause();
            let _ = cx.update(|cx| cx.global_mut::<Remote>().parked.push(hub));
        };
        let (hub, url, served) = if dev {
            let hub = match listener(true, false).and_then(|l| Hub::start(config.clone(), Some(password), l)) {
                Ok(hub) => hub,
                Err(err) => {
                    let _ = cx.update(|cx| fail(Problem::Other(err.to_string()), generation, cx));
                    return;
                }
            };
            let Some(local_port) = hub.endpoint.tcp_port() else { return };
            hub.set_config(Config { hosts: vec![format!("127.0.0.1:{local_port}"), format!("localhost:{local_port}")], ..config });
            (hub, format!("http://127.0.0.1:{local_port}/"), None)
        } else {
            let host = status.dns_name.clone().unwrap_or_default();
            let url = if https_port == 443 { format!("https://{host}/") } else { format!("https://{host}:{https_port}/") };
            // The private socket first; where Tailscale can't open it (its sandboxed App Store
            // build), `127.0.0.1`, where every check still holds.
            let mut started = None;
            let mut last_problem = Problem::Other("tailscale serve failed".into());
            for tcp in if cfg!(unix) { vec![false, true] } else { vec![true] } {
                // On a port, Tailscale is told a secret path to put in front of everything it
                // forwards: what connects to the port directly can't know it (check 0).
                let secret = tcp.then(auth::new_token);
                let config = Config { secret: secret.clone(), ..config.clone() };
                let hub = match listener(false, tcp).and_then(|l| Hub::start(config, Some(password.clone()), l)) {
                    Ok(hub) => hub,
                    Err(err) => {
                        last_problem = Problem::Other(err.to_string());
                        continue;
                    }
                };
                let target = match &secret {
                    Some(secret) => format!("{}/{secret}", hub.endpoint.serve_target()),
                    None => hub.endpoint.serve_target(),
                };
                let serve_target = target.clone();
                let serve_host = host.clone();
                match cx.background_executor().spawn(async move { tailscale::serve_on(https_port, &serve_host, &serve_target) }).await {
                    Ok(epoch) => {
                        let probe = url.clone();
                        // The socket only counts once it is seen to work; a port is taken on
                        // trust where there is nothing to ask with.
                        let reached = cx.background_executor().spawn(async move { tailscale::reachable(&probe) }).await;
                        if reached.unwrap_or(tcp) {
                            started = Some((hub, (https_port, target, epoch)));
                            break;
                        }
                        let off = target.clone();
                        cx.background_executor().spawn(async move { tailscale::serve_off_if(https_port, &off, epoch) }).await;
                        park(hub, cx);
                        last_problem = Problem::Other("Tailscale can't reach Agentty's page".into());
                    }
                    // A Tailscale that refuses the socket outright: try the port.
                    Err(tailscale::ServeError::Failed(text)) if !tcp => {
                        park(hub, cx);
                        last_problem = Problem::Other(text);
                    }
                    Err(error) => {
                        park(hub, cx);
                        let problem = match error {
                            tailscale::ServeError::NeedsEnabling(url) => Problem::NeedsEnabling(url),
                            tailscale::ServeError::Funnel => Problem::Funnel,
                            tailscale::ServeError::PortTaken => Problem::PortTaken(https_port),
                            tailscale::ServeError::Failed(text) => Problem::Other(text),
                        };
                        let _ = cx.update(|cx| fail(problem, generation, cx));
                        return;
                    }
                }
                if !current(cx) {
                    return;
                }
            }
            let Some((hub, served)) = started else {
                let _ = cx.update(|cx| fail(last_problem, generation, cx));
                return;
            };
            (hub, url, Some(served))
        };
        let _ = cx.update(|cx| {
            let remote = cx.global_mut::<Remote>();
            if remote.generation != generation {
                // Turned off while it was starting: take down this start's serve only, not one a
                // newer start set up meanwhile, and hold the port until that is done.
                hub.pause();
                remote.parked.push(hub);
                if let Some((port, target, epoch)) = served {
                    std::thread::spawn(move || tailscale::serve_off_if(port, &target, epoch));
                }
                return;
            }
            if let Some(waker) = &remote.waker {
                hub.set_waker(waker.clone());
            }
            // Saved while this start was under way: the server got the one before.
            if remote.password_saves != saves {
                if let Some(hash) = remote.saved_password.clone() {
                    hub.set_password(Some(hash));
                }
            }
            remote.hub = Some(hub);
            remote.served = served.map(|(port, target, _)| (port, target));
            // The entry now points here: the ports the old servers held are no one's to take.
            for old in remote.parked.drain(..) {
                old.shutdown();
            }
            remote.last_funnel_check = Some(std::time::Instant::now());
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

/// Stops the server and `serve`, leaving the setting as it is. The server keeps its port, closed
/// to everyone, until the Tailscale entry pointing at it is gone (`watch_tailscale`): taken down
/// now if Tailscale answers, or replaced by the next start.
fn stop_serving(cx: &mut App) {
    let remote = cx.global_mut::<Remote>();
    remote.generation += 1;
    if let Some(hub) = remote.hub.take() {
        hub.pause();
        remote.parked.push(hub);
    }
    if let Some(mut child) = remote.keep_awake.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Some((port, target)) = remote.served.take() {
        cx.background_executor().spawn(async move { tailscale::serve_off(port, &target) }).detach();
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
    if cfg!(debug_assertions) {
        eprintln!("remote: saving password {}", auth::shape(&password)); // audit: ok — length and kind only, never the value
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
                remote.saved_password = Some(hash.clone());
                remote.password_saves += 1;
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

/// Every few seconds while on, and while off because Tailscale was:
/// - Tailscale stopped, disconnected or signed out: the server and `serve` go down at once;
/// - it is back: remote access starts again (it was never turned off);
/// - the Mac's Tailscale login or name changed: the server starts over for the new ones (every
///   session ends; the old login is let in no more);
/// - Funnel turned on for the port (by hand, or another tool): everything goes down.
fn watch_tailscale(cx: &mut App) {
    if dev_mode() {
        return;
    }
    let enabled = settings(cx).remote.feature && settings(cx).remote.enabled;
    let remote = cx.global_mut::<Remote>();
    let waiting = enabled && matches!(remote.phase, Phase::Failed(Problem::NotRunning | Problem::NotInstalled | Problem::NoHttps));
    let parked = !remote.parked.is_empty();
    let served_now = remote.served.is_some();
    let attempts = tailscale::serve_attempts();
    if (remote.served.is_none() && !waiting && !parked)
        || remote.checking
        || remote.last_check.is_some_and(|at| at.elapsed() < TAILSCALE_CHECK)
    {
        return;
    }
    remote.last_check = Some(std::time::Instant::now());
    remote.checking = true;
    let funnel_port =
        remote.served.as_ref().filter(|_| remote.last_funnel_check.is_none_or(|at| at.elapsed() >= FUNNEL_CHECK)).map(|(port, _)| *port);
    if funnel_port.is_some() {
        remote.last_funnel_check = Some(std::time::Instant::now());
    }
    let generation = remote.generation;
    let expected = remote.watched_tailscale.clone();
    cx.spawn(async move |cx| {
        let serving = served_now;
        let (status, funnel, entry_gone) = cx
            .background_executor()
            .spawn(async move {
                let status = tailscale::status();
                // `None` when it couldn't be read this time: a slow answer is not Funnel.
                let funnel = funnel_port.and_then(tailscale::funnel_state);
                // Stopped servers hold their ports until the entry that pointed at them is gone.
                let entry_gone = parked && !serving && {
                    if status.running {
                        tailscale::clean_leftover_unless_newer(attempts);
                    }
                    !tailscale::has_leftover()
                };
                (status, funnel, entry_gone)
            })
            .await;
        let _ = cx.update(|cx| {
            let remote = cx.global_mut::<Remote>();
            remote.checking = false;
            if entry_gone {
                for old in remote.parked.drain(..) {
                    old.shutdown();
                }
            }
            if remote.generation != generation {
                return;
            }
            let connected = status.running && status.login.is_some() && status.dns_name.is_some();
            if waiting {
                if connected && status.https {
                    start(cx);
                }
                return;
            }
            // Only a server that is up is watched past here: one turned off (and only parked)
            // must never be turned on, or shown an error, by this check.
            let enabled = settings(cx).remote.feature && settings(cx).remote.enabled;
            if !enabled || !serving {
                return;
            }
            if !connected {
                stop_serving(cx);
                cx.global_mut::<Remote>().phase = Phase::Failed(Problem::NotRunning);
                cx.global_mut::<Remote>().tailscale = Some(status);
                cx.refresh_windows();
            } else if funnel == Some(true) {
                stop_serving(cx);
                cx.global_mut::<Remote>().phase = Phase::Failed(Problem::Funnel);
                cx.refresh_windows();
            } else if (status.login.clone(), status.dns_name.clone()) != expected {
                start(cx);
            }
        });
    })
    .detach();
}

/// Characters of a plugin's failure kept for the page.
const MAX_PLUGIN_ERROR_CHARS: usize = 300;

/// Every installed plugin and how it runs, with its automations (`automations`: plugin, id,
/// title, asleep) and what each last reported.
fn plugin_overview(automations: &[(String, String, String, bool)], cx: &App) -> Vec<snapshot::PluginInfo> {
    use crate::plugins::{InstanceState, RunState};
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let mut plugins = Vec::new();
    for plugin in &crate::plugins::host(cx).installed {
        let runtime = crate::plugins::runtime(cx, &plugin.id);
        let (state, error) = match runtime.map(|r| &r.state) {
            _ if !plugin.enabled => ("off", None),
            _ if plugin.manifest.is_none() => ("failed", plugin.error.clone()),
            Some(RunState::Running) => ("running", None),
            Some(RunState::Starting) => ("starting", None),
            Some(RunState::Failed(why)) => ("failed", Some(why.clone())),
            Some(RunState::NeedsConsent) => ("consent", None),
            Some(RunState::Stopped) | None => ("stopped", None),
        };
        // Its panel outside any workspace reports under `""`: the plugin's own line, untitled.
        let own = runtime.filter(|r| r.statuses.contains_key("")).map(|_| (plugin.id.clone(), String::new(), String::new(), false));
        let automations = own
            .iter()
            .chain(automations.iter().filter(|(owner, ..)| *owner == plugin.id))
            .map(|(_, id, title, sleeping)| {
                let status = runtime.and_then(|r| r.statuses.get(id));
                snapshot::AutomationInfo {
                    title: title.clone(),
                    state: match status.map(|s| s.state) {
                        Some(InstanceState::Working) => "working",
                        Some(InstanceState::Idle) => "idle",
                        Some(InstanceState::Error) => "error",
                        None => "",
                    },
                    text: status.and_then(|s| s.text.clone()),
                    elapsed: status.and_then(|s| s.working_since_ms).map(|since| now_ms.saturating_sub(since) / 1000),
                    sleeping: *sleeping,
                }
            })
            .collect();
        plugins.push(snapshot::PluginInfo {
            id: plugin.id.clone(),
            name: plugin.name().to_string(),
            version: plugin.manifest.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
            state,
            error: error.map(|e| e.chars().take(MAX_PLUGIN_ERROR_CHARS).collect()),
            automations,
        });
    }
    plugins
}

/// One round: the page's input into the terminals, then sessions and watched screens out.
fn tick(cx: &mut App) {
    watch_tailscale(cx);
    let Some(hub) = hub(cx) else { return };
    let windows = crate::workbenches(cx);
    let commands = hub.take_commands();
    if !commands.is_empty() {
        cx.global_mut::<Remote>().last_input = Some(std::time::Instant::now());
    }
    let watched_now = hub.watched();
    for command in commands {
        // Its last page left and came back before this round: it is shown again, at the size
        // that page sends.
        if let server::Command::Release { pane } = command {
            if watched_now.contains(&pane) {
                continue;
            }
        }
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
    let (mut workspaces, mut sessions, mut automations) = (Vec::new(), Vec::new(), Vec::new());
    for window in &windows {
        let _ = window.update(cx, |wb, _, cx| {
            if overview {
                let (w, s) = wb.remote_overview(cx);
                workspaces.extend(w);
                sessions.extend(s);
                automations.extend(wb.remote_automations(cx));
            }
            for pane in &watched {
                if let Some(screen) = wb.remote_screen(*pane, cx) {
                    hub.publish_screen(*pane, screen);
                }
            }
        });
    }
    if overview {
        hub.publish(workspaces, sessions, plugin_overview(&automations, cx));
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
