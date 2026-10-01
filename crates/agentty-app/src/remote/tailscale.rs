//! Tailscale, through its own command-line tool: whether it runs, this machine's tailnet name, the
//! login that owns it, and `tailscale serve` for Agentty's port only.
//!
//! - Funnel (the public internet) is never turned on, and a port with Funnel on is refused;
//!   anything unreadable about it counts as on.
//! - A port where something else is already served is left alone, and only an entry pointing at
//!   this Agentty's own server is ever taken down (a test copy with its own data folder never
//!   touches the installed app's).
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
}

/// Where the tool is: on PATH, or inside the app that installs it.
pub fn binary() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/usr/local/bin/tailscale", "/opt/homebrew/bin/tailscale", "/Applications/Tailscale.app/Contents/MacOS/Tailscale"]
    } else if cfg!(windows) {
        &["C:\\Program Files\\Tailscale\\tailscale.exe"]
    } else {
        &["/usr/bin/tailscale", "/usr/local/bin/tailscale", "/usr/sbin/tailscale"]
    };
    candidates.iter().map(PathBuf::from).find(|p| p.is_file()).or_else(|| agentty_bridge::process::which("tailscale"))
}

/// What the tool printed: `out` is stdout alone (JSON, when asked for), `all` both streams.
struct Output {
    ok: bool,
    out: String,
    all: String,
}

/// Runs the tool, or gives up (and kills it) after `timeout`. Never while the Mac's Tailscale VPN
/// is off: on macOS the tool brings the VPN back up just by asking it something, and a VPN the
/// user turned off must stay off.
fn run(args: &[&str], timeout: Duration) -> Option<Output> {
    if vpn_off() {
        return None;
    }
    let mut child = Command::new(binary()?).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + timeout;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    Some(Output { ok, all: format!("{out}{err}"), out })
}

/// Whether the Mac's Tailscale VPN (the app's network extension) is off, as macOS itself tells:
/// asking the system does not wake it, asking Tailscale does. `false` where Tailscale runs
/// without one (its open-source daemon) or on other systems.
pub fn vpn_off() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let Ok(output) = Command::new("/usr/sbin/scutil").args(["--nc", "list"]).stdin(Stdio::null()).output() else { return false };
    vpn_off_in(&String::from_utf8_lossy(&output.stdout))
}

/// `scutil --nc list`'s answer: a Tailscale VPN listed, and not connected.
pub fn vpn_off_in(list: &str) -> bool {
    let mut lines = list.lines().filter(|line| line.contains("io.tailscale.ipn")).peekable();
    lines.peek().is_some() && !lines.any(|line| line.contains("(Connected)"))
}

pub fn status() -> Status {
    if binary().is_none() {
        return Status::default();
    }
    if vpn_off() {
        return Status { installed: true, ..Default::default() };
    }
    match run(&["status", "--json"], Duration::from_secs(8)) {
        Some(output) => parse_status(&output.out),
        None => Status { installed: true, ..Default::default() },
    }
}

pub fn parse_status(json: &str) -> Status {
    let mut status = Status { installed: true, ..Default::default() };
    let Ok(value) = serde_json::from_str::<Value>(json) else { return status };
    status.running = value["BackendState"] == "Running";
    let me = &value["Self"];
    status.dns_name = me["DNSName"].as_str().map(|n| n.trim_end_matches('.').to_string()).filter(|n| !n.is_empty());
    let user_id = me["UserID"].as_u64().map(|id| id.to_string());
    status.login = user_id.and_then(|id| value["User"][&id]["LoginName"].as_str().map(str::to_string)).filter(|l| !l.is_empty());
    status.https = value["CertDomains"].as_array().is_some_and(|d| !d.is_empty());
    status
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
pub fn serve_on(https_port: u16, target: &str) -> Result<u64, ServeError> {
    let mut epoch = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    // A leftover of this Agentty's own (on any port) is its to take down; anything else is not.
    if let Some((port, old)) = leftover() {
        off_if_ours(port, &old);
    }
    match serve_config().map(|json| port_in_use(&json, https_port)) {
        Some(Ok(false)) => {}
        Some(Ok(true)) => return Err(ServeError::PortTaken),
        // Unreadable: better refuse than replace what someone set up.
        _ => return Err(ServeError::Failed("couldn't read the tailscale serve configuration".into())),
    }
    // Recorded before `serve` runs: a crash right after must still leave a trace to clean up.
    write_marker(https_port, target);
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
        return Err(ServeError::Failed(line.chars().take(200).collect()));
    }
    if funnel_on(https_port) {
        off_if_ours(https_port, target);
        return Err(ServeError::Funnel);
    }
    *epoch += 1;
    Ok(*epoch)
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

/// Whether two serve targets name the same place (`serve` may write one back with a slash).
fn same_target(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Asks the page's own address, through Tailscale, from this Mac: whether `serve` really reaches
/// the server (a sandboxed Tailscale can't open the Unix socket, and says so only with a 502).
/// `None` when there is no `curl` to ask with.
pub fn reachable(url: &str) -> Option<bool> {
    let curl = if cfg!(target_os = "macos") { "/usr/bin/curl" } else { "curl" };
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = Command::new(curl)
        .args(["--silent", "--output", null, "--write-out", "%{http_code}", "--max-time", "8", "--noproxy", "*", url])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).trim() == "200")
}

/// Whether Funnel (the public internet) is on for `port`. Anything unreadable counts as on.
pub fn funnel_on(port: u16) -> bool {
    match run(&["serve", "status", "--json"], Duration::from_secs(8)) {
        Some(output) => funnel_in(&output.out, port),
        None => true,
    }
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
                https: true
            }
        );
        let stopped = parse_status(r#"{ "BackendState": "Stopped", "Self": null, "User": null, "CertDomains": null }"#);
        assert!(!stopped.running && stopped.dns_name.is_none() && stopped.login.is_none() && !stopped.https);
        assert_eq!(parse_status("not json"), Status { installed: true, ..Default::default() });
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
