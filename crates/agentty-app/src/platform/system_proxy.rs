//! Pointing the machine's own proxy settings at Agentty's capture proxy, so traffic from other
//! apps — an external browser, anything using the system configuration — is listed too.
//!
//! macOS only, through `networksetup`. What each network service had before is written to a file
//! first and put back when capture stops, when Agentty quits, and on the next launch if Agentty
//! was killed while it was on: the user must never be left with a proxy pointing at a dead port.
//! Changing these settings needs administrator rights, so the call can fail — it is reported, not
//! retried, and nothing is left half-applied.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What one network service had set, to put back exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceProxy {
    pub service: String,
    pub web_enabled: bool,
    pub web_server: String,
    pub web_port: u16,
    pub secure_enabled: bool,
    pub secure_server: String,
    pub secure_port: u16,
}

/// The settings replaced while the machine is captured.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Previous {
    pub services: Vec<ServiceProxy>,
}

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

fn state_file() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("system-proxy-before.json")
}

#[cfg(target_os = "macos")]
fn networksetup(args: &[&str]) -> std::io::Result<String> {
    let output = agentty_bridge::process::command("/usr/sbin/networksetup").args(args).stdin(std::process::Stdio::null()).output()?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if message.is_empty() { String::from_utf8_lossy(&output.stdout).trim().to_string() } else { message };
        return Err(std::io::Error::other(if message.is_empty() { "networksetup failed".into() } else { message }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(not(target_os = "macos"))]
fn networksetup(_: &[&str]) -> std::io::Result<String> {
    Err(std::io::Error::other("only macOS can be captured this way"))
}

/// Network services that can carry a proxy, disabled ones (`*`) left out.
pub fn services() -> Vec<String> {
    let Ok(text) = networksetup(&["-listallnetworkservices"]) else { return Vec::new() };
    text.lines().skip(1).filter(|line| !line.starts_with('*') && !line.trim().is_empty()).map(|line| line.trim().to_string()).collect()
}

/// Reads `Enabled` / `Server` / `Port` out of a `-getwebproxy` answer.
fn parse_proxy(text: &str) -> (bool, String, u16) {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
        })
    };
    let enabled = field("Enabled").is_some_and(|v| v.eq_ignore_ascii_case("yes"));
    let server = field("Server").unwrap_or_default();
    let port = field("Port").and_then(|v| v.parse().ok()).unwrap_or(0);
    (enabled, server, port)
}

fn read_service(service: &str) -> ServiceProxy {
    let web = networksetup(&["-getwebproxy", service]).map(|t| parse_proxy(&t)).unwrap_or((false, String::new(), 0));
    let secure = networksetup(&["-getsecurewebproxy", service]).map(|t| parse_proxy(&t)).unwrap_or((false, String::new(), 0));
    ServiceProxy {
        service: service.to_string(),
        web_enabled: web.0,
        web_server: web.1,
        web_port: web.2,
        secure_enabled: secure.0,
        secure_server: secure.1,
        secure_port: secure.2,
    }
}

/// Whether the machine is pointed at `port` on the loopback interface right now.
pub fn points_at(port: u16) -> bool {
    services().iter().any(|service| {
        let current = read_service(service);
        current.secure_enabled && current.secure_port == port && is_loopback(&current.secure_server)
    })
}

fn is_loopback(server: &str) -> bool {
    matches!(server, "127.0.0.1" | "localhost" | "::1")
}

/// Sends the machine's HTTP and HTTPS traffic to `port` on the loopback interface. The settings
/// replaced are written to disk first, so they can be put back even after a crash.
pub fn enable(port: u16) -> std::io::Result<Previous> {
    let services = services();
    if services.is_empty() {
        return Err(std::io::Error::other("no network service to capture"));
    }
    // Already pointed here (a second click, or a restore that did not finish): keep the settings
    // that were saved the first time rather than recording Agentty's own as "what was there".
    if points_at(port) {
        if let Some(previous) = saved_previous() {
            return Ok(previous);
        }
    }
    let previous = Previous { services: services.iter().map(|s| read_service(s)).collect() };
    save_previous(&previous);
    let port = port.to_string();
    for service in &services {
        networksetup(&["-setwebproxy", service, "127.0.0.1", &port])?;
        networksetup(&["-setsecurewebproxy", service, "127.0.0.1", &port])?;
    }
    Ok(previous)
}

/// Puts back what [`enable`] replaced.
pub fn restore(previous: &Previous) -> std::io::Result<()> {
    let mut failure = None;
    for entry in &previous.services {
        let restore_one = |kind: &str, enabled: bool, server: &str, port: u16| -> std::io::Result<()> {
            let set = if kind == "web" { "-setwebproxy" } else { "-setsecurewebproxy" };
            let state = if kind == "web" { "-setwebproxystate" } else { "-setsecurewebproxystate" };
            if enabled && !server.is_empty() {
                networksetup(&[set, &entry.service, server, &port.to_string()])?;
            } else {
                networksetup(&[state, &entry.service, "off"])?;
            }
            Ok(())
        };
        if let Err(err) = restore_one("web", entry.web_enabled, &entry.web_server, entry.web_port) {
            failure.get_or_insert(err);
        }
        if let Err(err) = restore_one("secure", entry.secure_enabled, &entry.secure_server, entry.secure_port) {
            failure.get_or_insert(err);
        }
    }
    let _ = std::fs::remove_file(state_file());
    match failure {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn save_previous(previous: &Previous) {
    let path = state_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(previous) {
        let _ = std::fs::write(path, json);
    }
}

pub fn saved_previous() -> Option<Previous> {
    serde_json::from_slice(&std::fs::read(state_file()).ok()?).ok()
}

/// Called at startup: Agentty was killed while the machine was captured, so put the settings back
/// before anything else tries to use the network.
pub fn restore_after_crash() {
    let Some(previous) = saved_previous() else { return };
    if let Err(err) = restore(&previous) {
        eprintln!("agentty: could not put the system proxy settings back: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_networksetup_prints() {
        let text = "Enabled: Yes\nServer: 127.0.0.1\nPort: 51234\nAuthenticated Proxy Enabled: 0\n";
        assert_eq!(parse_proxy(text), (true, "127.0.0.1".to_string(), 51234));
        let off = "Enabled: No\nServer: \nPort: 0\nAuthenticated Proxy Enabled: 0\n";
        assert_eq!(parse_proxy(off), (false, String::new(), 0));
    }

    #[test]
    fn loopback_is_recognised() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("localhost"));
        assert!(!is_loopback("proxy.corp.example"));
    }
}
