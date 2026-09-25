//! One Agentty per user on Windows and Linux. macOS routes a second launch, `agentty://` links
//! and "Open With" to the running app itself; elsewhere each of those starts a new process,
//! which would fight the first one over settings and workspace files. So the running Agentty
//! records how to reach its socket (`<data dir>/instance.json`, private to the user), and a new
//! launch hands its links and folders over and exits.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Instance {
    pid: u32,
    address: String,
    #[serde(default)]
    token: Option<String>,
}

fn path() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("instance.json")
}

/// Sends `arguments` to a running Agentty; `true` when one took them (this process should exit).
pub fn forward(arguments: &[String]) -> bool {
    let Some(instance) = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<Instance>(&b).ok()) else { return false };
    if instance.pid == std::process::id() {
        return false;
    }
    let Ok(mut stream) = crate::ipc::connect_to(&instance.address, instance.token.as_deref()) else { return false };
    let message = serde_json::to_string(arguments).unwrap_or_else(|_| "[]".into());
    writeln!(stream, "open\t{message}").is_ok()
}

/// Records this process as the running instance (after its socket is listening).
pub fn register(socket: &crate::agent_signal::SignalSocket) {
    let instance = Instance { pid: std::process::id(), address: socket.address.clone(), token: socket.token.clone() };
    let Ok(json) = serde_json::to_vec(&instance) else { return };
    // Holds the socket token, which on Windows is all a connection has to show. `0600` on Unix; on
    // Windows the file inherits the data folder's permissions, which only the user (and the system
    // and administrators) can read.
    let _ = agentty_bridge::fsutil::write_private(&path(), &json);
}

/// Forgets this process as the running instance (at quit).
pub fn unregister() {
    let current = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<Instance>(&b).ok());
    if current.is_some_and(|i| i.pid == std::process::id()) {
        let _ = std::fs::remove_file(path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_file_roundtrip() {
        let instance = Instance { pid: 7, address: "127.0.0.1:1".into(), token: Some("placeholder".into()) };
        let json = serde_json::to_vec(&instance).unwrap();
        assert_eq!(serde_json::from_slice::<Instance>(&json).unwrap(), instance);
        let unix: Instance = serde_json::from_str(r#"{"pid":1,"address":"/tmp/agentty-1.sock"}"#).unwrap();
        assert_eq!(unix.token, None);
    }
}
