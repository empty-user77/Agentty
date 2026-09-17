//! Opt-in, anonymous usage metrics (off by default).
//!
//! What is recorded: event names from a fixed list, small enum-like properties (which agent CLI,
//! which feature), app version, macOS version and a random install id. Never paths, commands,
//! prompts, output, repository or branch names. Events go to a local JSONL file the user can read
//! (`~/.agentty/metrics/`); they are uploaded only when an endpoint is configured
//! (`metrics.endpoint` in settings or `AGENTTY_METRICS_ENDPOINT`). `DO_NOT_TRACK=1` disables
//! everything, whatever the setting says. See docs/metrics.md.

use crate::fsutil;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

pub const SCHEMA_VERSION: u32 = 1;

/// Every event Agentty may record; anything else is dropped.
pub const EVENTS: &[&str] = &["app_launched", "pane_opened", "agent_turn_finished", "feature_used", "window_opened"];

/// Property keys allowed per event, with a whitelist of values where free text could leak.
fn allowed(event: &str, key: &str) -> bool {
    matches!(
        (event, key),
        ("app_launched", "windows" | "workspaces")
            | ("pane_opened", "tool")
            | ("agent_turn_finished", "tool" | "duration_bucket")
            | ("feature_used", "feature")
    )
}

/// `DO_NOT_TRACK` wins over the setting (https://donottrack.sh).
pub fn do_not_track() -> bool {
    std::env::var("DO_NOT_TRACK").is_ok_and(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
}

fn dir() -> PathBuf {
    fsutil::data_dir().join("metrics")
}

/// Random id created the first time metrics are enabled; "reset" deletes it.
pub fn install_id() -> String {
    let path = dir().join("install_id");
    if let Ok(id) = std::fs::read_to_string(&path) {
        if !id.trim().is_empty() {
            return id.trim().to_string();
        }
    }
    let id = random_id();
    let _ = std::fs::create_dir_all(dir());
    let _ = std::fs::write(&path, &id);
    id
}

fn random_id() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut file, &mut bytes);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn reset_install_id() {
    let _ = std::fs::remove_file(dir().join("install_id"));
}

/// Builds the event record, or `None` if the event or a property isn't allowed.
pub fn record_for(event: &str, props: &Value, app_version: &str, os_version: &str, install_id: &str, now_ms: u64) -> Option<Value> {
    if !EVENTS.contains(&event) {
        return None;
    }
    let mut clean = serde_json::Map::new();
    for (key, value) in props.as_object().into_iter().flatten() {
        let ok = allowed(event, key)
            && match value {
                Value::Number(_) | Value::Bool(_) => true,
                // Short identifiers only (e.g. "claude", "cosgit", "10-60s").
                Value::String(text) => text.len() <= 32 && text.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
                _ => false,
            };
        if ok {
            clean.insert(key.clone(), value.clone());
        }
    }
    Some(json!({
        "v": SCHEMA_VERSION,
        "ts": now_ms,
        "install_id": install_id,
        "app_version": app_version,
        "os": os_version,
        "event": event,
        "props": clean,
    }))
}

/// Appends an event to this month's local log.
pub fn append(record: &Value) {
    let _ = std::fs::create_dir_all(dir());
    let month = chrono::Local::now().format("%Y-%m");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir().join(format!("events-{month}.jsonl"))) {
        let _ = writeln!(file, "{record}");
    }
}

pub fn local_dir() -> PathBuf {
    dir()
}

/// Uploads events not sent yet (tracked by line count in `sent.json`) to `endpoint` as one JSON
/// array. Failures are silent; they are retried next time.
pub fn flush(endpoint: &str) {
    let Ok(url) = url::Url::parse(endpoint) else { return };
    if url.scheme() != "https" && !matches!(url.host_str(), Some("localhost" | "127.0.0.1")) {
        return;
    }
    let sent_path = dir().join("sent.json");
    let mut sent: serde_json::Map<String, Value> =
        std::fs::read(&sent_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(dir()) else { return };
    for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "jsonl")) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        let done = sent.get(&name).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        if done >= lines.len() {
            continue;
        }
        let batch: Vec<Value> = lines[done..].iter().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let ok = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .post(url.as_str())
            .send_json(Value::Array(batch))
            .is_ok();
        if ok {
            sent.insert(name, json!(lines.len()));
        }
    }
    let _ = std::fs::write(sent_path, Value::Object(sent).to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_allowed_events_and_values() {
        let record = record_for("pane_opened", &json!({ "tool": "claude", "cwd": "/Users/me/secret" }), "0.2.0", "15.1", "abc", 1).unwrap();
        assert_eq!(record["props"], json!({ "tool": "claude" }));
        assert!(record_for("keystroke", &json!({}), "0.2.0", "15.1", "abc", 1).is_none());
        let record = record_for("feature_used", &json!({ "feature": "rm -rf /" }), "0.2.0", "15.1", "abc", 1).unwrap();
        assert_eq!(record["props"], json!({}));
    }

    #[test]
    fn do_not_track_values() {
        std::env::set_var("DO_NOT_TRACK", "1");
        assert!(do_not_track());
        std::env::remove_var("DO_NOT_TRACK");
        assert!(!do_not_track());
    }
}
