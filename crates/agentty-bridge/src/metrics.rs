//! Usage analytics through Google Analytics 4 (Measurement Protocol).
//!
//! Events come from a fixed list with short identifier properties (which agent CLI, which
//! feature) — never paths, commands, prompts, output, repository or branch names. The GA
//! measurement ID and API secret are compiled in from the build environment
//! (`AGENTTY_GA_MEASUREMENT_ID`, `AGENTTY_GA_API_SECRET`); builds without them send nothing.
//! `DO_NOT_TRACK=1` turns it off. See docs/metrics.md.

use crate::fsutil;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Every event Agentty may send; anything else is dropped.
pub const EVENTS: &[&str] = &["app_launched", "pane_opened", "agent_turn_finished", "feature_used", "window_opened"];

/// Property keys allowed per event.
fn allowed(event: &str, key: &str) -> bool {
    matches!(
        (event, key),
        ("app_launched", "windows") | ("pane_opened", "tool") | ("agent_turn_finished", "tool") | ("feature_used", "feature")
    )
}

/// (measurement id, api secret) from the build, when both were set.
pub fn credentials() -> Option<(&'static str, &'static str)> {
    let id = option_env!("AGENTTY_GA_MEASUREMENT_ID").filter(|v| !v.is_empty())?;
    let secret = option_env!("AGENTTY_GA_API_SECRET").filter(|v| !v.is_empty())?;
    Some((id, secret))
}

/// `DO_NOT_TRACK` (https://donottrack.sh) turns analytics off.
pub fn do_not_track() -> bool {
    std::env::var("DO_NOT_TRACK").is_ok_and(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
}

pub fn enabled() -> bool {
    credentials().is_some() && !do_not_track()
}

fn id_path() -> PathBuf {
    fsutil::data_dir().join("install_id")
}

/// Random per-install id (GA `client_id`), created on first use.
pub fn install_id() -> String {
    if let Ok(id) = std::fs::read_to_string(id_path()) {
        if !id.trim().is_empty() {
            return id.trim().to_string();
        }
    }
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut file, &mut bytes);
    }
    let id = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let _ = std::fs::create_dir_all(fsutil::data_dir());
    let _ = std::fs::write(id_path(), &id);
    id
}

/// A GA4 event, or `None` if the event isn't allowed. Properties are filtered to the allow-list
/// and to short identifiers.
pub fn event(name: &str, props: &Value, app_version: &str, os_version: &str) -> Option<Value> {
    if !EVENTS.contains(&name) {
        return None;
    }
    let mut params = serde_json::Map::new();
    for (key, value) in props.as_object().into_iter().flatten() {
        let ok = allowed(name, key)
            && match value {
                Value::Number(_) | Value::Bool(_) => true,
                Value::String(text) => text.len() <= 32 && text.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
                _ => false,
            };
        if ok {
            params.insert(key.clone(), value.clone());
        }
    }
    params.insert("app_version".into(), json!(app_version));
    params.insert("os_version".into(), json!(os_version));
    // GA shows events without engagement time as inactive users.
    params.insert("engagement_time_msec".into(), json!(1));
    Some(json!({ "name": name, "params": params }))
}

/// Sends events (at most 25 per request, GA's limit). Failures are ignored.
pub fn send(events: Vec<Value>) {
    let Some((measurement_id, secret)) = credentials() else { return };
    if events.is_empty() || do_not_track() {
        return;
    }
    let url = format!("https://www.google-analytics.com/mp/collect?measurement_id={measurement_id}&api_secret={secret}");
    let agent = crate::http::agent_builder().timeout(std::time::Duration::from_secs(10)).build();
    let client_id = install_id();
    for batch in events.chunks(25) {
        let _ = agent.post(&url).send_json(json!({ "client_id": client_id, "events": batch }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_allowed_events_and_values() {
        let event_value = event("pane_opened", &json!({ "tool": "claude", "cwd": "/Users/me/secret" }), "0.2.0", "15.1").unwrap();
        assert_eq!(event_value["params"]["tool"], "claude");
        assert!(event_value["params"].get("cwd").is_none());
        assert!(event("keystroke", &json!({}), "0.2.0", "15.1").is_none());
        let event_value = event("feature_used", &json!({ "feature": "rm -rf /" }), "0.2.0", "15.1").unwrap();
        assert!(event_value["params"].get("feature").is_none());
    }

    #[test]
    fn do_not_track_values() {
        std::env::set_var("DO_NOT_TRACK", "1");
        assert!(do_not_track());
        std::env::remove_var("DO_NOT_TRACK");
        assert!(!do_not_track());
    }
}
