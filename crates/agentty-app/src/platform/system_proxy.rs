//! What is left of "capture this Mac" now that capture is the Proxy Capture plugin: putting the
//! machine's proxy settings back when an older Agentty was killed while it had pointed them at its
//! own capture proxy. That Agentty wrote what each network service had to a file first; until that
//! file is gone the machine may be pointed at a dead port, with no network at all.
//!
//! macOS only, through `networksetup`. The plugin keeps its own record and puts back its own.

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

/// The settings replaced while the machine was captured.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Previous {
    pub services: Vec<ServiceProxy>,
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
    Err(std::io::Error::other("only macOS was captured this way"))
}

/// Puts back what was replaced.
fn restore(previous: &Previous) -> std::io::Result<()> {
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
    match failure {
        // The record stays on disk: the machine is still pointed somewhere it should not be, and
        // the next launch is the only chance left to put it back.
        Some(err) => Err(err),
        None => {
            let _ = std::fs::remove_file(state_file());
            Ok(())
        }
    }
}

fn saved_previous() -> Option<Previous> {
    serde_json::from_slice(&std::fs::read(state_file()).ok()?).ok()
}

/// Called at startup: an older Agentty was killed while the machine was captured, so put the
/// settings back before anything else tries to use the network.
pub fn restore_after_crash() {
    let Some(previous) = saved_previous() else { return };
    if let Err(err) = restore(&previous) {
        eprintln!("agentty: could not put the system proxy settings back: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed restore keeps the record: it is the only way the next launch can put the settings
    /// back, and deleting it stranded the machine on a dead port.
    #[test]
    fn a_failed_restore_keeps_what_it_could_not_put_back() {
        let previous = Previous {
            services: vec![ServiceProxy {
                service: "No Such Service".into(),
                web_enabled: false,
                web_server: String::new(),
                web_port: 0,
                secure_enabled: false,
                secure_server: String::new(),
                secure_port: 0,
            }],
        };
        let path = state_file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, serde_json::to_vec(&previous).unwrap()).unwrap();
        // `networksetup` cannot know this service, so the restore fails on every platform.
        assert!(restore(&previous).is_err());
        assert!(saved_previous().is_some(), "the record is still there to try again");
        let _ = std::fs::remove_file(path);
    }
}
