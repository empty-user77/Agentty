//! Tailscale, through its own command-line tool: whether it runs, this machine's tailnet name, the
//! login that owns it, and `tailscale serve` for Agentty's port only.
//!
//! - Funnel (the public internet) is never turned on, and a port with Funnel on is refused;
//!   anything unreadable about it counts as on.
//! - A port where something else is already served is left alone.
//! - `serve` on and off run one at a time, and a marker file lets the next launch take down an
//!   entry a crash left behind.

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

/// Runs the tool, or gives up (and kills it) after `timeout`.
fn run(args: &[&str], timeout: Duration) -> Option<Output> {
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

pub fn status() -> Status {
    if binary().is_none() {
        return Status::default();
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

/// Marks the port Agentty serves, so a launch after a crash can take a leftover entry down.
fn marker() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("remote-serve.json")
}

/// The port a previous run left served, if it did not get to turn it off.
pub fn leftover_port() -> Option<u16> {
    let text = std::fs::read_to_string(marker()).ok()?;
    serde_json::from_str::<Value>(&text).ok()?["port"].as_u64().and_then(|p| u16::try_from(p).ok())
}

/// Shares `127.0.0.1:<local_port>` on the tailnet at `https://<machine>:<https_port>`. Returns the
/// number of this "on", for `serve_off_if`.
pub fn serve_on(https_port: u16, local_port: u16) -> Result<u64, ServeError> {
    let mut epoch = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    // A leftover of Agentty's own on this port is Agentty's to replace; anything else is not.
    if leftover_port() == Some(https_port) {
        off(https_port);
    }
    let config = run(&["serve", "status", "--json"], Duration::from_secs(8)).map(|o| o.out);
    match config.as_deref().map(|json| port_in_use(json, https_port)) {
        Some(Ok(false)) => {}
        Some(Ok(true)) => return Err(ServeError::PortTaken),
        // Unreadable: better refuse than replace what someone set up.
        _ => return Err(ServeError::Failed("couldn't read the tailscale serve configuration".into())),
    }
    let https = format!("--https={https_port}");
    let target = format!("http://127.0.0.1:{local_port}");
    // `serve` waits for the tailnet admin when Serve is off; it prints where to turn it on first.
    let Some(output) = run(&["serve", "--bg", "--yes", &https, &target], Duration::from_secs(20)) else {
        return Err(ServeError::Failed("tailscale did not run".into()));
    };
    if let Some(url) = enable_url(&output.all) {
        off(https_port);
        return Err(ServeError::NeedsEnabling(url));
    }
    if !output.ok {
        off(https_port);
        let line = output.all.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("tailscale serve failed");
        return Err(ServeError::Failed(line.chars().take(200).collect()));
    }
    let _ = std::fs::write(marker(), serde_json::json!({ "port": https_port }).to_string());
    if funnel_on(https_port) {
        off(https_port);
        return Err(ServeError::Funnel);
    }
    *epoch += 1;
    Ok(*epoch)
}

fn off(https_port: u16) {
    let https = format!("--https={https_port}");
    let _ = run(&["serve", "--yes", &https, "off"], Duration::from_secs(10));
    if leftover_port() == Some(https_port) {
        let _ = std::fs::remove_file(marker());
    }
}

/// Stops sharing Agentty's port (only ever one Agentty set up itself, see `serve_on`).
pub fn serve_off(https_port: u16) {
    let _serial = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    off(https_port);
}

/// `serve_off`, unless another "on" came after `epoch`.
pub fn serve_off_if(https_port: u16, epoch: u64) {
    let current = SERVE.lock().unwrap_or_else(|e| e.into_inner());
    if *current == epoch {
        off(https_port);
    }
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

/// Whether the serve configuration already uses `port` (empty output: nothing is served).
pub fn port_in_use(json: &str, port: u16) -> Result<bool, ()> {
    if json.trim().is_empty() {
        return Ok(false);
    }
    let value = serde_json::from_str::<Value>(json).map_err(|_| ())?;
    let suffix = format!(":{port}");
    let tcp = value["TCP"].as_object().is_some_and(|ports| ports.contains_key(&port.to_string()));
    let web = value["Web"].as_object().is_some_and(|hosts| hosts.keys().any(|host| host.ends_with(&suffix)));
    Ok(tcp || web)
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
    }
}
