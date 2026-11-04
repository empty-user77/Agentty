//! Tailscale, through its own command-line tool: whether it runs, this machine's tailnet name, the
//! login that owns it, and `tailscale serve` for Agentty's port only.
//!
//! - Funnel (the public internet) is never turned on, and a port with Funnel on is refused;
//!   anything unreadable about it counts as on.
//! - A port where something else is already served is left alone, and only an entry pointing at
//!   this Agentty's own server is ever taken down (a test copy with its own data folder never
//!   touches the installed app's), or an Agentty entry nothing answers behind any more.
//! - `serve` on and off run one at a time. A marker file, written before `serve` runs, lets the
//!   next launch (or quit) take down an entry a crash left behind.

use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub installed: bool,
    /// Connected to the tailnet.
    pub running: bool,
    /// `machine.tailnet.ts.net`, without the trailing dot.
    pub dns_name: Option<String>,
    /// The login that owns this machine (`me@example.com`).
    pub login: Option<String>,
    /// The tailnet can issue HTTPS certificates (MagicDNS and HTTPS on).
    pub https: bool,
    /// Why it can't be used yet, when it is installed.
    pub trouble: Option<Trouble>,
    /// What Tailscale calls its own state (`Running`, `NeedsLogin`, ...), for a problem report.
    pub state: Option<String>,
    /// Which of the tools found was asked: `app`, `homebrew` or `other`, never a path.
    pub tool: Option<&'static str>,
    pub version: Option<String>,
}

/// Why an installed Tailscale can't be used yet: each has its own way out.
#[derive(Debug, Clone, PartialEq)]
pub enum Trouble {
    /// The Mac's Tailscale VPN is off, or hasn't come up yet.
    VpnOff,
    /// Signed out, with Tailscale's own sign-in address when it gave one.
    NeedsLogin(Option<String>),
    /// Signed in; the tailnet's admin has to approve this device.
    NeedsApproval,
    /// On its way up.
    Starting,
    /// Switched off in Tailscale (`tailscale down`).
    Stopped,
    /// The tool can't reach Tailscale's background service: (what it said).
    NoService(String),
    /// The tool failed some other way: (what it said).
    Tool(String),
    /// Connected, without a name or an account for this machine.
    NoIdentity,
}

/// The tools a machine may have, in the order they are tried. A Mac can hold more than one (a
/// Homebrew `tailscale` beside the app, which talks to a daemon that isn't running), and the one
/// that belongs to a VPN macOS lists comes first.
fn candidate_paths(vpn_listed: bool) -> Vec<&'static str> {
    if cfg!(target_os = "macos") {
        const APP: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";
        let others = ["/usr/local/bin/tailscale", "/opt/homebrew/bin/tailscale"];
        return if vpn_listed {
            std::iter::once(APP).chain(others).collect()
        } else {
            others.into_iter().chain(std::iter::once(APP)).collect()
        };
    }
    if cfg!(windows) {
        return vec!["C:\\Program Files\\Tailscale\\tailscale.exe"];
    }
    vec!["/usr/bin/tailscale", "/usr/local/bin/tailscale", "/usr/sbin/tailscale", "/snap/bin/tailscale"]
}

/// The tool that last answered with something readable, used from then on: `cert` and `serve` must
/// reach the same Tailscale `status` did.
static TOOL: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// For the debug driver: a test's own tools, and what macOS says about the VPN.
#[cfg(debug_assertions)]
pub mod test_override {
    use std::path::PathBuf;
    use std::sync::Mutex;
    pub static TOOLS: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);
    pub static VPN: Mutex<Option<String>> = Mutex::new(None);
}

/// Every tool there is, in the order to try them.
fn candidates() -> Vec<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(tools) = test_override::TOOLS.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return tools;
    }
    let mut tools: Vec<PathBuf> = candidate_paths(vpn_listed()).into_iter().map(PathBuf::from).filter(|path| path.is_file()).collect();
    // The app dragged into the user's own Applications folder, or anywhere else Spotlight knows of.
    if cfg!(target_os = "macos") {
        let own = dirs::home_dir().map(|home| home.join("Applications/Tailscale.app"));
        tools.extend(own.filter(|app| signed_by_tailscale(app)).map(|app| app.join(APP_TOOL)));
        if tools.is_empty() {
            tools.extend(apps_spotlight_knows());
        }
    }
    // Windows installed on another drive.
    if cfg!(windows) {
        let other = std::env::var_os("ProgramFiles").map(|folder| PathBuf::from(folder).join("Tailscale").join("tailscale.exe"));
        tools.extend(other.filter(|path| path.is_file() && !tools.contains(path)));
    }
    if let Some(found) = agentty_bridge::process::which("tailscale") {
        if !tools.contains(&found) {
            tools.push(found);
        }
    }
    tools
}

/// The Tailscale apps Spotlight has seen on this Mac, wherever they are: the App Store's build and
/// the one from Tailscale's site.
fn apps_spotlight_knows() -> Vec<PathBuf> {
    let query = "kMDItemCFBundleIdentifier == 'io.tailscale.ipn.macos' || kMDItemCFBundleIdentifier == 'io.tailscale.ipn.macsys'";
    let Ok(output) = Command::new("/usr/bin/mdfind").arg(query).stdin(Stdio::null()).output() else { return Vec::new() };
    tools_in_apps(&String::from_utf8_lossy(&output.stdout))
}

/// The tool inside the app.
const APP_TOOL: &str = "Contents/MacOS/Tailscale";

/// The tool in each app `mdfind` listed that really is Tailscale's.
fn tools_in_apps(listed: &str) -> Vec<PathBuf> {
    listed
        .lines()
        .filter(|line| line.ends_with(".app"))
        .map(PathBuf::from)
        .filter(|app| signed_by_tailscale(app))
        .map(|app| app.join(APP_TOOL))
        .filter(|path| path.is_file())
        .collect()
}

/// Whether macOS confirms `app` is signed by Tailscale's developer account. An app found in a place
/// anything can write to gets run without the user choosing it, and a bundle id proves nothing: any
/// download can claim one.
fn signed_by_tailscale(app: &std::path::Path) -> bool {
    const REQUIREMENT: &str = "-R=anchor apple generic and certificate leaf[subject.OU] = \"W5364U7YZB\"";
    app.is_dir()
        && Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", REQUIREMENT])
            .arg(app)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
}

/// Where the tool is: the one that answered last, else the first there is.
pub fn binary() -> Option<PathBuf> {
    let known = TOOL.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|path| path.is_file());
    known.or_else(|| candidates().into_iter().next())
}

/// A tool's kind, for a problem report: a path would hold the user's name.
fn tool_kind(path: &std::path::Path) -> &'static str {
    let text = path.to_string_lossy();
    if text.contains("Tailscale.app") || text.contains("Program Files") {
        "app"
    } else if text.contains("homebrew") {
        "homebrew"
    } else {
        "other"
    }
}

/// How long a tool's output may take to end once the tool has.
const PIPES_CLOSE: Duration = Duration::from_secs(1);

/// What the tool printed: `out` is stdout alone (JSON, when asked for), `all` both streams.
struct Output {
    ok: bool,
    timed_out: bool,
    out: String,
    all: String,
}

/// Runs the tool, or gives up (and kills it) after `timeout`. Never while the Mac's Tailscale VPN
/// is off: on macOS the tool brings the VPN back up just by asking it something, and a VPN the
/// user turned off must stay off.
fn run(args: &[&str], timeout: Duration) -> Option<Output> {
    run_with(args, timeout, true)
}

/// `run`, with stdout thrown away unread when `keep_out` is false (`out` is then empty).
fn run_with(args: &[&str], timeout: Duration, keep_out: bool) -> Option<Output> {
    if vpn_off() {
        return None;
    }
    run_tool(&binary()?, args, timeout, keep_out)
}

fn run_tool(tool: &std::path::Path, args: &[&str], timeout: Duration, keep_out: bool) -> Option<Output> {
    let stdout = if keep_out { Stdio::piped() } else { Stdio::null() };
    // Without a console window: on Windows one would flash up on every status check.
    let mut child =
        agentty_bridge::process::command(tool).args(args).stdin(Stdio::null()).stdout(stdout).stderr(Stdio::piped()).spawn().ok()?;
    let stdout = child.stdout.take();
    let mut stderr = child.stderr.take()?;
    let (out_done, out) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stdout) = stdout {
            let _ = stdout.read_to_string(&mut text);
        }
        let _ = out_done.send(text);
    });
    let (err_done, err) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        let _ = err_done.send(text);
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    // `/usr/local/bin/tailscale` is a shell script: killing it leaves the real tool running, holding
    // these pipes. What it said by now is all there will be, or a stuck tool would stop every check.
    let (out, err) = (out.recv_timeout(PIPES_CLOSE).unwrap_or_default(), err.recv_timeout(PIPES_CLOSE).unwrap_or_default());
    Some(Output { ok, timed_out, all: format!("{out}{err}"), out })
}

/// Whether the Mac's Tailscale VPN (the app's network extension) is off, as macOS itself tells:
/// asking the system does not wake it, asking Tailscale does. `false` where Tailscale runs
/// without one (its open-source daemon) or on other systems.
pub fn vpn_off() -> bool {
    vpn_list().is_some_and(|list| vpn_off_in(&list))
}

/// Whether macOS has a Tailscale VPN at all: the app has been set up here.
fn vpn_listed() -> bool {
    vpn_list().is_some_and(|list| list.contains("io.tailscale.ipn"))
}

fn vpn_list() -> Option<String> {
    #[cfg(debug_assertions)]
    if let Some(list) = test_override::VPN.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(list);
    }
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = Command::new("/usr/sbin/scutil").args(["--nc", "list"]).stdin(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `scutil --nc list`'s answer: a Tailscale VPN listed, and not connected.
pub fn vpn_off_in(list: &str) -> bool {
    let mut lines = list.lines().filter(|line| line.contains("io.tailscale.ipn")).peekable();
    lines.peek().is_some() && !lines.any(|line| line.contains("(Connected)"))
}

pub fn status() -> Status {
    let tools = candidates();
    let Some(first) = tools.first() else {
        // No tool to be found, but macOS has its VPN: it is installed, whatever is in the way.
        return if vpn_listed() {
            let trouble = if vpn_off() { Trouble::VpnOff } else { Trouble::Tool("the tailscale tool wasn't found".into()) };
            Status { installed: true, trouble: Some(trouble), ..Default::default() }
        } else {
            Status::default()
        };
    };
    if vpn_off() {
        return Status { installed: true, trouble: Some(Trouble::VpnOff), tool: Some(tool_kind(first)), ..Default::default() };
    }
    // The first one connected, else the first that answered at all, else what the first one said.
    let mut answered: Option<(Status, &PathBuf)> = None;
    let mut failure = None;
    for (index, tool) in tools.iter().enumerate() {
        let wait = Duration::from_secs(if index == 0 { 8 } else { 4 });
        let Some(output) = run_tool(tool, &["status", "--json"], wait, true) else {
            failure.get_or_insert_with(|| Trouble::Tool("tailscale did not run".into()));
            continue;
        };
        let mut status = parse_status(&output.out);
        status.tool = Some(tool_kind(tool));
        if matches!(status.trouble, Some(Trouble::Tool(_))) {
            failure.get_or_insert_with(|| failed(&output));
        } else if status.running {
            answered = Some((status, tool));
            break;
        } else if answered.is_none() {
            answered = Some((status, tool));
        }
    }
    match answered {
        Some((status, tool)) => {
            *TOOL.lock().unwrap_or_else(|e| e.into_inner()) = Some(tool.clone());
            status
        }
        None => Status { installed: true, trouble: failure, tool: Some(tool_kind(first)), ..Default::default() },
    }
}

/// What a tool that gave no status said, without secret paths or the user's home.
fn failed(output: &Output) -> Trouble {
    if output.timed_out {
        return Trouble::Tool("timed out".into());
    }
    let line = output.all.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("no answer");
    let mut said = redact(line);
    if let Some(home) = std::env::var_os("HOME").and_then(|home| home.into_string().ok()).filter(|home| !home.is_empty()) {
        said = said.replace(&home, "~");
    }
    let said: String = said.chars().take(200).collect();
    let lower = said.to_lowercase();
    let unreachable = ["tailscaled", "is tailscale running", "connection refused", "failed to connect", "service"];
    if unreachable.iter().any(|word| lower.contains(word)) {
        Trouble::NoService(said)
    } else {
        Trouble::Tool(said)
    }
}

/// A status, from the first `{` on: a tool out of step with its service warns before the JSON.
pub fn parse_status(json: &str) -> Status {
    let mut status = Status { installed: true, ..Default::default() };
    let Some(Ok(value)) = json.find('{').map(|at| serde_json::from_str::<Value>(&json[at..])) else {
        status.trouble = Some(Trouble::Tool(String::new()));
        return status;
    };
    let state = value["BackendState"].as_str().unwrap_or_default();
    status.state = Some(state.to_string()).filter(|state| !state.is_empty());
    status.version = value["Version"].as_str().map(str::to_string);
    status.running = state == "Running";
    let me = &value["Self"];
    status.dns_name = me["DNSName"].as_str().map(|n| n.trim_end_matches('.').to_string()).filter(|n| !n.is_empty());
    let user_id = me["UserID"].as_u64().map(|id| id.to_string());
    status.login = user_id.and_then(|id| value["User"][&id]["LoginName"].as_str().map(str::to_string)).filter(|l| !l.is_empty());
    status.https = value["CertDomains"].as_array().is_some_and(|d| !d.is_empty());
    status.trouble = match state {
        "Running" if status.login.is_some() && status.dns_name.is_some() => None,
        "Running" => Some(Trouble::NoIdentity),
        "NeedsLogin" => Some(Trouble::NeedsLogin(value["AuthURL"].as_str().and_then(sign_in_url))),
        "NeedsMachineAuth" => Some(Trouble::NeedsApproval),
        "Stopped" => Some(Trouble::Stopped),
        _ => Some(Trouble::Starting),
    };
    status
}

/// A sign-in address, if it is Tailscale's own: nothing else is ever opened from what the tool says.
fn sign_in_url(url: &str) -> Option<String> {
    url.starts_with("https://login.tailscale.com/").then(|| url.to_string())
}

/// Brings the Tailscale app to the front, where there is one to open.
pub fn open_app() {
    if cfg!(target_os = "macos") {
        let _ = Command::new("/usr/bin/open").args(["-a", "Tailscale"]).stdin(Stdio::null()).spawn();
    } else if cfg!(windows) {
        let _ = agentty_bridge::process::command("C:\\Program Files\\Tailscale\\tailscale-ipn.exe").spawn();
    }
}

/// What was found, to paste into a question or hand to an agent. No account, machine or tailnet
/// name, and no path: what the tool said has been cleaned already (`failed`).
pub fn report(status: &Status, app_version: &str) -> String {
    let trouble = match &status.trouble {
        Some(Trouble::VpnOff) => "the macOS Tailscale VPN is not connected".to_string(),
        Some(Trouble::NeedsLogin(_)) => "signed out".to_string(),
        Some(Trouble::NeedsApproval) => "waiting for the tailnet admin to approve this device".to_string(),
        Some(Trouble::Starting) => "starting".to_string(),
        Some(Trouble::Stopped) => "switched off in Tailscale".to_string(),
        Some(Trouble::NoService(said)) => format!("the tool can't reach the background service: {said}"),
        Some(Trouble::Tool(said)) => format!("the tool failed: {said}"),
        Some(Trouble::NoIdentity) => "connected, but no machine name or account".to_string(),
        None if !status.installed => "not installed".to_string(),
        None => "none".to_string(),
    };
    [
        format!("Agentty: {app_version}"),
        format!("System: {}", std::env::consts::OS),
        format!("Installed: {}", status.installed),
        format!("Tool: {}", status.tool.unwrap_or("none")),
        format!("Tailscale version: {}", status.version.as_deref().unwrap_or("unknown")),
        format!("State: {}", status.state.as_deref().unwrap_or("unknown")),
        format!("Connected: {}", status.running),
        format!("HTTPS certificates: {}", status.https),
        format!("Problem: {trouble}"),
    ]
    .join("\n")
}

/// Has Tailscale get this machine's HTTPS certificate for `domain` now, rather than on the first
/// request through `serve` (which then takes longer than a start waits for). Only Tailscale keeps
/// it, and renews it: what the tool prints, the private key included, goes nowhere.
pub fn issue_certificate(domain: &str) -> Result<(), String> {
    let output = run_with(&["cert", "--cert-file", "-", "--key-file", "-", domain], Duration::from_secs(150), false)
        .ok_or_else(|| "tailscale did not run".to_string())?;
    if output.ok {
        return Ok(());
    }
    let line = output.all.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("tailscale cert failed");
    Err(redact(line).chars().take(200).collect())
}

/// A device on the tailnet, as `tailscale whois` describes it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Device {
    pub ip: String,
    /// Its machine name (`iphone`).
    pub name: String,
    /// Its operating system (`iOS`), when it tells.
    pub os: String,
    pub login: String,
}

/// Who a tailnet address belongs to. Only an address that parses as one ever reaches the tool.
pub fn whois(ip: &str) -> Option<Device> {
    let addr: std::net::IpAddr = ip.trim().parse().ok()?;
    let output = run(&["whois", "--json", &addr.to_string()], Duration::from_secs(5))?;
    parse_whois(&output.out, &addr.to_string())
}

pub fn parse_whois(json: &str, ip: &str) -> Option<Device> {
    let value: Value = serde_json::from_str(json).ok()?;
    let node = &value["Node"];
    let name = node["ComputedName"].as_str().or_else(|| node["Hostinfo"]["Hostname"].as_str()).unwrap_or("").to_string();
    let os = node["Hostinfo"]["OS"].as_str().unwrap_or("").to_string();
    let login = value["UserProfile"]["LoginName"].as_str().unwrap_or("").to_string();
    (!name.is_empty() || !login.is_empty()).then(|| Device { ip: ip.to_string(), name, os, login })
}

/// Why `serve` could not be turned on.
#[derive(Debug, Clone, PartialEq)]
pub enum ServeError {
    /// Serve is off for the tailnet; the admin turns it on at this address (Tailscale's own page).
    NeedsEnabling(String),
    /// Funnel is on for this port: Agentty will not share the page on the internet.
    Funnel,
    /// Something else is already served on this port: Agentty leaves it alone.
    PortTaken,
    Failed(String),
}

/// The `https://login.tailscale.com/…` address in the tool's output, if any.
fn enable_url(text: &str) -> Option<String> {
    text.split_whitespace().find(|w| w.starts_with("https://login.tailscale.com/")).map(|w| w.trim_end_matches(['.', ',', ')']).to_string())
}

/// `serve` on and off one at a time, each "on" numbered: a late "off" from a start that was
/// overtaken must not take down the newer one (`serve_off_if`).
static SERVE: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
/// How many times `serve_on` began: a cleanup decided before one must not take down its entry.
static ATTEMPTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The number of `serve_on` calls begun so far, for `clean_leftover_unless_newer`.
pub fn serve_attempts() -> u64 {
    ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst)
}

/// `clean_leftover`, unless a `serve_on` began since `attempts` was read: the marker then names
/// that newer entry, which is live and not a leftover.
pub fn clean_leftover_unless_newer(attempts: u64) {
    let _serial = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    if serve_attempts() == attempts {
        clean_leftover_locked();
    }
}

/// Marks what Agentty serves (tailnet port and target), so a launch or quit after a crash can
/// take a leftover entry down, and only its own.
fn marker() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("remote-serve.json")
}

/// What a previous run of this data folder served and did not get to turn off: (port, target).
fn leftover() -> Option<(u16, String)> {
    let text = std::fs::read_to_string(marker()).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    let port = value["port"].as_u64().and_then(|p| u16::try_from(p).ok())?;
    Some((port, value["target"].as_str()?.to_string()))
}

/// Only this user may read it (`0600`): the target holds the secret path Tailscale is told, which
/// must not reach anyone else on the Mac.
fn write_marker(https_port: u16, target: &str) {
    let json = serde_json::json!({ "port": https_port, "target": target }).to_string();
    let _ = agentty_bridge::fsutil::write_private(&marker(), json.as_bytes());
}

/// The serve configuration, if it can be read (`""` when nothing is served).
fn serve_config() -> Option<String> {
    run(&["serve", "status", "--json"], Duration::from_secs(8)).filter(|o| o.ok).map(|o| o.out)
}

/// Whether an entry this data folder served may still be up (its marker is there).
pub fn has_leftover() -> bool {
    leftover().is_some()
}

/// Takes down a leftover of this data folder's, if the entry still points at what it recorded.
/// A marker whose entry is gone (or now points elsewhere) is forgotten, but only while Tailscale
/// surely runs: stopped, it shows no configuration at all.
pub fn clean_leftover() {
    let _serial = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    clean_leftover_locked();
}

fn clean_leftover_locked() {
    if let Some((port, target)) = leftover() {
        off_if_ours(port, &target);
        if leftover().is_some() && status().running {
            if let Some(json) = serve_config() {
                if !served_target(&json, port).is_some_and(|served| same_target(&served, &target)) {
                    let _ = std::fs::remove_file(marker());
                }
            }
        }
    }
}

/// Shares `target` on the tailnet at `https://<machine>:<https_port>`. Returns the number of
/// this "on", for `serve_off_if`.
pub fn serve_on(https_port: u16, host: &str, target: &str) -> Result<u64, ServeError> {
    let mut epoch = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    // A leftover of this Agentty's own (on any port) is its to take down; anything else is not.
    if let Some((port, old)) = leftover() {
        off_if_ours(port, &old);
    }
    // One whose marker was lost (overwritten by a start Tailscale wasn't ready for) is still known
    // by its shape, once nothing serves behind it any more.
    if let Some(stale) = serve_config().and_then(|json| served_target(&json, https_port)).filter(|t| dead_agentty_target(t)) {
        off_if_ours(https_port, &stale);
    }
    match serve_config().map(|json| port_in_use(&json, https_port)) {
        Some(Ok(false)) => {}
        Some(Ok(true)) => return Err(ServeError::PortTaken),
        // Unreadable: better refuse than replace what someone set up.
        _ => return Err(ServeError::Failed("couldn't read the tailscale serve configuration".into())),
    }
    // Recorded before `serve` runs: a crash right after must still leave a trace to clean up.
    write_marker(https_port, target);
    // Through Tailscale's local API where it answers (the macOS app): the target, secret path and
    // all, then never appears on a command line, which every account on the Mac can list. Where
    // it refuses (Serve not allowed on the tailnet, most likely) the tool below says why.
    if let Some(Ok(())) = local_api::set_serve(host, https_port, target) {
        if funnel_on(https_port) {
            off_if_ours(https_port, target);
            return Err(ServeError::Funnel);
        }
        *epoch += 1;
        return Ok(*epoch);
    }
    let https = format!("--https={https_port}");
    // `serve` waits for the tailnet admin when Serve is off; it prints where to turn it on first.
    let Some(output) = run(&["serve", "--bg", "--yes", &https, target], Duration::from_secs(20)) else {
        off_if_ours(https_port, target);
        return Err(ServeError::Failed("tailscale did not run".into()));
    };
    if let Some(url) = enable_url(&output.all) {
        off_if_ours(https_port, target);
        return Err(ServeError::NeedsEnabling(url));
    }
    if !output.ok {
        off_if_ours(https_port, target);
        let line = output.all.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("tailscale serve failed");
        return Err(ServeError::Failed(redact(line).chars().take(200).collect()));
    }
    if funnel_on(https_port) {
        off_if_ours(https_port, target);
        return Err(ServeError::Funnel);
    }
    *epoch += 1;
    Ok(*epoch)
}

/// `config` (a serve configuration) with `https://<host>:<port>` sent to `target`, as `tailscale
/// serve --bg --https=<port> <target>` would set it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // used by the macOS local API only
pub fn with_entry(mut config: Value, host: &str, port: u16, target: &str) -> Value {
    if !config.is_object() {
        config = serde_json::json!({});
    }
    config["TCP"][port.to_string()] = serde_json::json!({ "HTTPS": true });
    config["Web"][format!("{host}:{port}")] = serde_json::json!({ "Handlers": { "/": { "Proxy": target } } });
    config
}

/// Tailscale's local API, reached the way its own command-line tool reaches it on macOS: the port
/// and the token in `/Library/Tailscale` (readable by administrators only).
#[cfg(target_os = "macos")]
mod local_api {
    use super::{Answer, Value};
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;

    fn endpoint() -> Option<(u16, String)> {
        let port: u16 = std::fs::read_link("/Library/Tailscale/ipnport").ok()?.to_str()?.parse().ok()?;
        let token = std::fs::read_to_string(format!("/Library/Tailscale/sameuserproof-{port}")).ok()?.trim().to_string();
        (!token.is_empty()).then_some((port, token))
    }

    /// One request: (status, headers with lowercase names, body).
    fn request(method: &str, path: &str, extra: &[(&str, &str)], body: &[u8]) -> Option<Answer> {
        use base64::Engine as _;
        let (port, token) = endpoint()?;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
        stream.set_write_timeout(Some(Duration::from_secs(10))).ok()?;
        let auth = base64::engine::general_purpose::STANDARD.encode(format!(":{token}"));
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: local-tailscaled.sock\r\nAuthorization: Basic {auth}\r\nSec-Tailscale: localapi\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in extra {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).ok()?;
        stream.write_all(body).ok()?;
        let mut raw = Vec::new();
        stream.take(1 << 20).read_to_end(&mut raw).ok()?;
        super::parse_response(&raw)
    }

    /// `Some(Ok)` once set, `Some(Err(why))` when refused, `None` with no local API to ask.
    pub fn set_serve(host: &str, port: u16, target: &str) -> Option<Result<(), String>> {
        let (status, headers, body) = request("GET", "/localapi/v0/serve-config", &[], b"")?;
        if status != 200 {
            return None;
        }
        let config: Value = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).ok()? };
        let config = super::with_entry(config, host, port, target).to_string();
        let etag = headers.iter().find(|(name, _)| name == "etag").map(|(_, value)| value.clone());
        let mut extra = vec![("Content-Type", "application/json")];
        // Set only over the configuration just read: a change made in between is not lost.
        if let Some(etag) = &etag {
            extra.push(("If-Match", etag.as_str()));
        }
        let (status, _, answer) = request("POST", "/localapi/v0/serve-config", &extra, config.as_bytes())?;
        if status == 200 {
            Some(Ok(()))
        } else {
            Some(Err(super::redact(String::from_utf8_lossy(&answer).trim()).chars().take(200).collect()))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod local_api {
    pub fn set_serve(_: &str, _: u16, _: &str) -> Option<Result<(), String>> {
        None
    }
}

/// An HTTP answer: status, headers (lowercase names) and body.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // used by the macOS local API only
pub type Answer = (u16, Vec<(String, String)>, Vec<u8>);

/// An HTTP/1.1 response: (status, headers with lowercase names, body), chunked bodies joined.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // used by the macOS local API only
pub fn parse_response(raw: &[u8]) -> Option<Answer> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let mut lines = head.split("\r\n");
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let mut body = raw[split + 4..].to_vec();
    if headers.iter().any(|(name, value)| name == "transfer-encoding" && value.eq_ignore_ascii_case("chunked")) {
        let mut joined = Vec::new();
        let mut rest = body.as_slice();
        loop {
            let end = rest.windows(2).position(|w| w == b"\r\n")?;
            let size = usize::from_str_radix(std::str::from_utf8(&rest[..end]).ok()?.split(';').next()?.trim(), 16).ok()?;
            rest = &rest[end + 2..];
            if size == 0 {
                break;
            }
            joined.extend_from_slice(rest.get(..size)?);
            rest = rest.get(size + 2..)?;
        }
        body = joined;
    }
    Some((status, headers, body))
}

/// Whether the entry on `https_port` still points at `target`; `None` when the configuration
/// couldn't be read (a slow answer is not a lost entry).
pub fn still_served(https_port: u16, target: &str) -> Option<bool> {
    let json = serve_config()?;
    Some(served_target(&json, https_port).is_some_and(|served| same_target(&served, target)))
}

/// Takes the entry on `https_port` down if it points at `target`, and then forgets the marker.
/// While Tailscale is stopped it shows no configuration (and keeps it for when it is back), so
/// the marker stays until an "off" really ran: the next start or quit tries again.
fn off_if_ours(https_port: u16, target: &str) {
    let Some(json) = serve_config() else { return };
    if !served_target(&json, https_port).is_some_and(|served| same_target(&served, target)) {
        return;
    }
    let https = format!("--https={https_port}");
    let done = run(&["serve", "--yes", &https, "off"], Duration::from_secs(10)).is_some_and(|o| o.ok);
    if done && leftover().is_some_and(|(port, old)| port == https_port && old == target) {
        let _ = std::fs::remove_file(marker());
    }
}

/// Stops sharing what this Agentty served on `https_port`.
pub fn serve_off(https_port: u16, target: &str) {
    let _serial = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    off_if_ours(https_port, target);
}

/// `serve_off`, unless another "on" came after `epoch`.
pub fn serve_off_if(https_port: u16, target: &str, epoch: u64) {
    let current = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    if *current == epoch {
        off_if_ours(https_port, target);
    }
}

// ---- Local dev servers on the tailnet: one entry per server, apart from the page's own ----
//
// A dev server shown on the page gets its own entry, `https://<machine>:<port>` sent to an Agentty
// proxy that checks every request as the page does (`devproxy`). These entries have a marker of
// their own: the page's entry and its leftover handling never see them, and they never touch it.

/// Marks the dev-server entries this Agentty serves: [{port, target}], so a crash leaves a trace
/// to take down at the next launch.
fn ports_marker() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("remote-serve-ports.json")
}

fn port_entries() -> Vec<(u16, String)> {
    let Ok(text) = std::fs::read_to_string(ports_marker()) else { return Vec::new() };
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    entries.iter().filter_map(|e| Some((u16::try_from(e["port"].as_u64()?).ok()?, e["target"].as_str()?.to_string()))).collect()
}

/// Private (`0600`), like the page's marker: the targets hold secret paths.
fn write_port_entries(entries: &[(u16, String)]) {
    if entries.is_empty() {
        let _ = std::fs::remove_file(ports_marker());
        return;
    }
    let json: Vec<Value> = entries.iter().map(|(port, target)| serde_json::json!({ "port": port, "target": target })).collect();
    let _ = agentty_bridge::fsutil::write_private(&ports_marker(), Value::Array(json).to_string().as_bytes());
}

/// Shares a dev server's proxy (`target`) at `https://<host>:<https_port>`. Refuses a port the
/// configuration already uses (the page's own, or anything set up by hand).
pub fn port_on(https_port: u16, host: &str, target: &str) -> Result<(), ServeError> {
    let _serial = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    match serve_config().map(|json| port_in_use(&json, https_port)) {
        Some(Ok(false)) => {}
        Some(Ok(true)) => return Err(ServeError::PortTaken),
        _ => return Err(ServeError::Failed("couldn't read the tailscale serve configuration".into())),
    }
    // Recorded first: a crash right after must still leave a trace to clean up.
    let mut entries = port_entries();
    entries.retain(|(port, _)| *port != https_port);
    entries.push((https_port, target.to_string()));
    write_port_entries(&entries);
    let undo = |why: ServeError| {
        port_off_locked(https_port, target);
        Err(why)
    };
    if let Some(Ok(())) = local_api::set_serve(host, https_port, target) {
        if funnel_on(https_port) {
            return undo(ServeError::Funnel);
        }
        return Ok(());
    }
    let https = format!("--https={https_port}");
    let Some(output) = run(&["serve", "--bg", "--yes", &https, target], Duration::from_secs(20)) else {
        return undo(ServeError::Failed("tailscale did not run".into()));
    };
    if let Some(url) = enable_url(&output.all) {
        return undo(ServeError::NeedsEnabling(url));
    }
    if !output.ok {
        let line = output.all.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("tailscale serve failed");
        return undo(ServeError::Failed(redact(line).chars().take(200).collect()));
    }
    if funnel_on(https_port) {
        return undo(ServeError::Funnel);
    }
    Ok(())
}

/// Takes a dev server's entry down, if it still points at `target`.
pub fn port_off(https_port: u16, target: &str) {
    let _serial = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    port_off_locked(https_port, target);
}

fn port_off_locked(https_port: u16, target: &str) {
    let Some(json) = serve_config() else { return };
    if served_target(&json, https_port).is_some_and(|served| same_target(&served, target)) {
        let https = format!("--https={https_port}");
        if !run(&["serve", "--yes", &https, "off"], Duration::from_secs(10)).is_some_and(|o| o.ok) {
            // Still up: the marker keeps it for the next try.
            return;
        }
    }
    let mut entries = port_entries();
    entries.retain(|(port, old)| !(*port == https_port && old == target));
    write_port_entries(&entries);
}

/// Takes down every dev-server entry a previous run left (a crash, or a quit Tailscale didn't
/// answer in time). Only while Tailscale runs: stopped, it shows no configuration to check.
pub fn clean_port_leftovers() {
    if !status().running {
        return;
    }
    let _serial = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    for (port, target) in port_entries() {
        port_off_locked(port, &target);
    }
}

/// Where the entry on `port` sends requests to (`http://127.0.0.1:1234`, `unix:/path`).
pub fn served_target(json: &str, port: u16) -> Option<String> {
    let value = serde_json::from_str::<Value>(json).ok()?;
    let suffix = format!(":{port}");
    let mut configs = vec![&value];
    if let Some(sessions) = value["Foreground"].as_object() {
        configs.extend(sessions.values());
    }
    configs.iter().find_map(|config| {
        config["Web"]
            .as_object()?
            .iter()
            .filter(|(host, _)| host.ends_with(&suffix))
            .find_map(|(_, web)| web["Handlers"].as_object()?.get("/")?["Proxy"].as_str().map(str::to_string))
    })
}

/// The local port of a target shaped like Agentty's own on a port: `http://127.0.0.1:<port>/`
/// and a secret path of 64 hex digits (`auth::new_token`).
pub fn agentty_tcp_port(target: &str) -> Option<u16> {
    let (port, secret) = target.strip_prefix("http://127.0.0.1:")?.trim_end_matches('/').split_once('/')?;
    let port: u16 = port.parse().ok()?;
    (port != 0 && secret.len() == 64 && secret.bytes().all(|b| b.is_ascii_hexdigit())).then_some(port)
}

/// Whether `target` is an Agentty server that is gone: its shape on a port, or this data folder's
/// socket, with nothing listening there. A running Agentty (a stopped one keeps its port until
/// its entry is gone) always answers, so only a dead entry is ever taken for a leftover.
fn dead_agentty_target(target: &str) -> bool {
    if let Some(port) = agentty_tcp_port(target) {
        let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
        return std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_err();
    }
    #[cfg(unix)]
    if let Some(path) = target.strip_prefix("unix:") {
        let ours = super::conn::socket_path(&agentty_bridge::fsutil::data_dir());
        return std::path::Path::new(path) == ours && std::os::unix::net::UnixStream::connect(path).is_err();
    }
    false
}

/// Whether two serve targets name the same place (`serve` may write one back with a slash).
fn same_target(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Asks the page's own address, through Tailscale, from this Mac: whether `serve` really reaches
/// the server (a sandboxed Tailscale can't open the Unix socket, and says so only with a 502).
/// `None` when there is no `curl` to ask with.
/// A first start can be slow (Tailscale fetches the certificate on the first request), so no
/// answer at all is asked again a few times; a gateway error is the answer.
pub fn reachable(url: &str) -> Option<bool> {
    let curl = if cfg!(target_os = "macos") { "/usr/bin/curl" } else { "curl" };
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    for attempt in 0..3 {
        let output = agentty_bridge::process::command(curl)
            .args(["--silent", "--output", null, "--write-out", "%{http_code}", "--max-time", "8", "--noproxy", "*", url])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        match String::from_utf8_lossy(&output.stdout).trim() {
            "200" => return Some(true),
            "000" if attempt < 2 => std::thread::sleep(Duration::from_secs(2)),
            _ => return Some(false),
        }
    }
    Some(false)
}

/// Tailscale's message with any secret path taken out: it may repeat the target it was given.
fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_hexdigit() {
            run.push(c);
            continue;
        }
        out.push_str(if run.len() >= 32 { "…" } else { &run });
        run.clear();
        out.push(c);
    }
    out.pop();
    out
}

/// Whether Funnel (the public internet) is on for `port`. Anything unreadable counts as on.
pub fn funnel_on(port: u16) -> bool {
    funnel_state(port).unwrap_or(true)
}

/// Whether Funnel is on for `port`, or `None` when the configuration couldn't be read.
pub fn funnel_state(port: u16) -> Option<bool> {
    let output = run(&["serve", "status", "--json"], Duration::from_secs(8)).filter(|o| o.ok)?;
    Some(funnel_in(&output.out, port))
}

pub fn funnel_in(json: &str, port: u16) -> bool {
    if json.trim().is_empty() {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(json) else { return true };
    let suffix = format!(":{port}");
    value["AllowFunnel"]
        .as_object()
        .is_some_and(|hosts| hosts.iter().any(|(host, on)| host.ends_with(&suffix) && on.as_bool() == Some(true)))
}

/// Whether the serve configuration already uses `port` (empty output: nothing is served),
/// a `serve` running in the foreground of some terminal included.
pub fn port_in_use(json: &str, port: u16) -> Result<bool, ()> {
    if json.trim().is_empty() {
        return Ok(false);
    }
    let value = serde_json::from_str::<Value>(json).map_err(|_| ())?;
    let suffix = format!(":{port}");
    let uses = |config: &Value| {
        config["TCP"].as_object().is_some_and(|ports| ports.contains_key(&port.to_string()))
            || config["Web"].as_object().is_some_and(|hosts| hosts.keys().any(|host| host.ends_with(&suffix)))
    };
    let foreground = value["Foreground"].as_object().is_some_and(|sessions| sessions.values().any(uses));
    Ok(uses(&value) || foreground)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_status() {
        let json = r#"{
            "BackendState": "Running",
            "Self": { "DNSName": "mac.tail1234.ts.net.", "UserID": 42 },
            "User": { "42": { "LoginName": "me@example.com" }, "7": { "LoginName": "other@example.com" } },
            "CertDomains": ["mac.tail1234.ts.net"]
        }"#;
        assert_eq!(
            parse_status(json),
            Status {
                installed: true,
                running: true,
                dns_name: Some("mac.tail1234.ts.net".into()),
                login: Some("me@example.com".into()),
                https: true,
                state: Some("Running".into()),
                ..Default::default()
            }
        );
        let stopped = parse_status(r#"{ "BackendState": "Stopped", "Self": null, "User": null, "CertDomains": null }"#);
        assert!(!stopped.running && stopped.dns_name.is_none() && stopped.login.is_none() && !stopped.https);
        assert_eq!(parse_status("not json").trouble, Some(Trouble::Tool(String::new())));
    }

    #[test]
    fn each_state_has_its_own_trouble() {
        let state = |s: &str| parse_status(&format!(r#"{{ "BackendState": "{s}" }}"#)).trouble;
        assert_eq!(state("NeedsLogin"), Some(Trouble::NeedsLogin(None)));
        assert_eq!(state("NeedsMachineAuth"), Some(Trouble::NeedsApproval));
        assert_eq!(state("Stopped"), Some(Trouble::Stopped));
        assert_eq!(state("Starting"), Some(Trouble::Starting));
        assert_eq!(state("NoState"), Some(Trouble::Starting));
        assert_eq!(state("Running"), Some(Trouble::NoIdentity), "connected, with no name or account");
    }

    #[test]
    fn only_tailscales_own_sign_in_address_is_kept() {
        let login = |url: &str| parse_status(&format!(r#"{{ "BackendState": "NeedsLogin", "AuthURL": "{url}" }}"#)).trouble;
        assert_eq!(login("https://login.tailscale.com/a/abc"), Some(Trouble::NeedsLogin(Some("https://login.tailscale.com/a/abc".into()))));
        assert_eq!(login("https://login.tailscale.com.evil.example/a"), Some(Trouble::NeedsLogin(None)));
        assert_eq!(login("file:///etc/passwd"), Some(Trouble::NeedsLogin(None)));
    }

    #[test]
    fn a_warning_before_the_status_does_not_hide_it() {
        let text = "Warning: client version \"1.2\" != tailscaled server version \"1.3\"\n{ \"BackendState\": \"Stopped\" }";
        assert_eq!(parse_status(text).trouble, Some(Trouble::Stopped));
    }

    #[test]
    fn says_why_a_tool_gave_no_status() {
        let output = |all: &str| Output { ok: false, timed_out: false, out: String::new(), all: all.to_string() };
        assert!(matches!(failed(&output("failed to connect to local tailscaled; it doesn't appear to be running")), Trouble::NoService(_)));
        assert!(matches!(failed(&output("The Tailscale CLI failed to start: Failed to load preferences.")), Trouble::Tool(_)));
        assert_eq!(failed(&Output { timed_out: true, ..output("") }), Trouble::Tool("timed out".into()));
        let secret = "ab".repeat(32);
        assert!(!format!("{:?}", failed(&output(&format!("bad http://127.0.0.1:1/{secret}")))).contains(&secret));
    }

    #[test]
    fn the_report_names_no_account_or_machine() {
        let status = parse_status(
            r#"{ "BackendState": "Running", "Version": "1.104.4", "Self": { "DNSName": "mac.tail1234.ts.net.", "UserID": 42 },
                 "User": { "42": { "LoginName": "me@example.com" } }, "CertDomains": ["mac.tail1234.ts.net"] }"#,
        );
        let report = report(&status, "0.3.11");
        assert!(report.contains("Tailscale version: 1.104.4") && report.contains("State: Running"));
        assert!(!report.contains("example.com") && !report.contains("tail1234"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_app_that_only_claims_to_be_tailscale_is_not_run() {
        let folder = std::env::temp_dir().join(format!("agentty-fake-tailscale-{}", std::process::id()));
        let app = folder.join("Tailscale.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join(APP_TOOL), "#!/bin/sh\n").unwrap();
        assert!(tools_in_apps(&format!("{}\n", app.display())).is_empty());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_app_comes_first_where_macos_lists_its_vpn() {
        if cfg!(target_os = "macos") {
            assert!(candidate_paths(true)[0].contains("Tailscale.app"));
            assert!(!candidate_paths(false)[0].contains("Tailscale.app"));
        }
        assert!(tools_in_apps("/nowhere/Tailscale.app\n/nowhere/Tailscale.app/Contents\n").is_empty(), "only a tool that is there");
        assert_eq!(tool_kind(std::path::Path::new("/opt/homebrew/bin/tailscale")), "homebrew");
        assert_eq!(tool_kind(std::path::Path::new("/Applications/Tailscale.app/Contents/MacOS/Tailscale")), "app");
    }

    #[test]
    fn adds_an_entry_as_serve_would() {
        let existing = serde_json::json!({ "TCP": { "443": { "HTTPS": true } },
            "Web": { "mac.ts.net:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:3000" } } } },
            "AllowFunnel": { "mac.ts.net:443": true } });
        let config = with_entry(existing, "mac.ts.net", 8743, "http://127.0.0.1:5000/s3cr3t");
        let json = config.to_string();
        assert_eq!(served_target(&json, 8743).as_deref(), Some("http://127.0.0.1:5000/s3cr3t"));
        assert_eq!(served_target(&json, 443).as_deref(), Some("http://127.0.0.1:3000"), "what was there stays");
        assert!(funnel_in(&json, 443) && !funnel_in(&json, 8743), "and its Funnel setting with it");
        assert_eq!(port_in_use(&json, 8743), Ok(true));
        let fresh = with_entry(Value::Null, "mac.ts.net", 8743, "unix:/x/web.sock").to_string();
        assert_eq!(served_target(&fresh, 8743).as_deref(), Some("unix:/x/web.sock"));
    }

    #[test]
    fn reads_local_api_answers() {
        let plain = b"HTTP/1.1 200 OK\r\nEtag: \"abc\"\r\nContent-Length: 2\r\n\r\n{}";
        let (status, headers, body) = parse_response(plain).unwrap();
        assert_eq!((status, body.as_slice()), (200, b"{}".as_slice()));
        assert!(headers.contains(&("etag".to_string(), "\"abc\"".to_string())));
        let chunked = b"HTTP/1.1 412 Precondition Failed\r\nTransfer-Encoding: chunked\r\n\r\n4\r\netag\r\n9\r\n mismatch\r\n0\r\n\r\n";
        let (status, _, body) = parse_response(chunked).unwrap();
        assert_eq!((status, body.as_slice()), (412, b"etag mismatch".as_slice()));
        assert!(parse_response(b"garbage").is_none());
    }

    #[test]
    fn takes_the_secret_out_of_messages() {
        // A made-up secret path, built here rather than written out: 64 hex digits.
        let secret: String = (0..64u32).filter_map(|i| char::from_digit(i % 16, 16)).collect();
        assert_eq!(redact(&format!("error: proxy http://127.0.0.1:5000/{secret} refused")), "error: proxy http://127.0.0.1:5000/… refused");
        assert_eq!(redact("port 8743 is in use"), "port 8743 is in use");
    }

    #[test]
    fn reads_the_vpn_state_from_macos() {
        let off = r#"* (Disconnected)   2A5E8A92 VPN (io.tailscale.ipn.macsys) "Tailscale"   [VPN:io.tailscale.ipn.macsys]"#;
        let on = r#"* (Connected)      2A5E8A92 VPN (io.tailscale.ipn.macsys) "Tailscale"   [VPN:io.tailscale.ipn.macsys]"#;
        let other = r#"* (Disconnected)   7B1C VPN (com.example.vpn) "Work"   [VPN:com.example.vpn]"#;
        assert!(vpn_off_in(off));
        assert!(!vpn_off_in(on));
        assert!(!vpn_off_in(&format!("{other}\n{on}")), "another VPN off says nothing about Tailscale");
        assert!(!vpn_off_in(other), "no Tailscale VPN at all (its open-source daemon): ask the tool");
        assert!(vpn_off_in(&off.replace("Disconnected", "Connecting")), "not up yet");
    }

    #[test]
    fn reads_whois() {
        let json = r#"{ "Node": { "ComputedName": "iphone", "Hostinfo": { "OS": "iOS", "Hostname": "iPhone" } },
                        "UserProfile": { "LoginName": "me@example.com" } }"#;
        assert_eq!(
            parse_whois(json, "100.64.0.9"),
            Some(Device { ip: "100.64.0.9".into(), name: "iphone".into(), os: "iOS".into(), login: "me@example.com".into() })
        );
        assert_eq!(parse_whois("{}", "100.64.0.9"), None);
        assert_eq!(whois("100.64.0.9; rm -rf /"), None, "only an address reaches the tool");
    }

    #[test]
    fn finds_the_enable_link() {
        let text = "Serve is not enabled on your tailnet.\nTo enable, visit:\n\n         https://login.tailscale.com/f/serve?node=abc123\n";
        assert_eq!(enable_url(text).as_deref(), Some("https://login.tailscale.com/f/serve?node=abc123"));
        assert_eq!(enable_url("Available within your tailnet: https://mac.ts.net:8743/"), None);
        assert_eq!(enable_url("see https://login.tailscale.com.evil.example/x"), None);
    }

    #[test]
    fn spots_funnel_on_the_port() {
        let json = r#"{ "TCP": {}, "AllowFunnel": { "mac.ts.net:8743": true, "mac.ts.net:443": false } }"#;
        assert!(funnel_in(json, 8743));
        assert!(!funnel_in(json, 443));
        assert!(!funnel_in(r#"{ "TCP": {} }"#, 8743));
        assert!(funnel_in("Warning: client version is old\n{", 8743), "unreadable counts as on");
        assert!(!funnel_in("", 8743), "no configuration at all");
    }

    #[test]
    fn spots_a_port_already_served() {
        let json = r#"{ "TCP": { "443": { "HTTPS": true } }, "Web": { "mac.ts.net:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:3000" } } } } }"#;
        assert_eq!(port_in_use(json, 443), Ok(true));
        assert_eq!(port_in_use(json, 8743), Ok(false));
        assert_eq!(port_in_use("", 8743), Ok(false));
        assert_eq!(port_in_use("{ not json", 8743), Err(()));
        let foreground = r#"{ "Foreground": { "abc": { "TCP": { "8743": { "HTTPS": true } } } } }"#;
        assert_eq!(port_in_use(foreground, 8743), Ok(true), "a serve running in a terminal");
    }

    #[test]
    fn knows_agentty_targets_by_shape() {
        let secret = "ab".repeat(32);
        assert_eq!(agentty_tcp_port(&format!("http://127.0.0.1:58960/{secret}")), Some(58960));
        assert_eq!(agentty_tcp_port(&format!("http://127.0.0.1:58960/{secret}/")), Some(58960));
        assert_eq!(agentty_tcp_port("http://127.0.0.1:3000"), None, "someone's dev server");
        assert_eq!(agentty_tcp_port("http://127.0.0.1:3000/api"), None);
        assert_eq!(agentty_tcp_port(&format!("http://127.0.0.1:3000/{}", "a".repeat(63))), None);
        assert_eq!(agentty_tcp_port(&format!("http://192.168.0.2:3000/{secret}")), None, "not this Mac");
        assert_eq!(agentty_tcp_port(&format!("http://127.0.0.1:0/{secret}")), None);
    }

    #[test]
    fn reads_what_a_port_is_served_to() {
        let json = r#"{ "TCP": { "8743": { "HTTPS": true } },
            "Web": { "mac.ts.net:8743": { "Handlers": { "/": { "Proxy": "unix:/Users/me/.agentty/remote/web.sock" } } },
                     "mac.ts.net:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:3000" } } } } }"#;
        assert_eq!(served_target(json, 8743).as_deref(), Some("unix:/Users/me/.agentty/remote/web.sock"));
        assert_eq!(served_target(json, 443).as_deref(), Some("http://127.0.0.1:3000"));
        assert_eq!(served_target(json, 9000), None);
        assert!(same_target("http://127.0.0.1:3000/", "http://127.0.0.1:3000"));
        assert!(!same_target("unix:/a/web.sock", "unix:/b/web.sock"), "another data folder's server is not ours");
    }
}
