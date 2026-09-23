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

/// What GA shows as the device and the country. Measurement Protocol requests carry none of it on
/// their own (GA takes it from its web tag, which Agentty doesn't run), so it is sent with each
/// request: the platform, its version, the language Agentty is shown in and the country of the
/// system's region setting — never an IP address or a location.
#[derive(Debug, Clone, Default)]
pub struct Device {
    /// `macOS`, `Windows` or `Linux`.
    pub operating_system: String,
    pub operating_system_version: String,
    /// ISO 639-1, e.g. `ko`.
    pub language: String,
    /// ISO 3166-1 alpha-2 from the region setting, e.g. `KR`.
    pub country: Option<String>,
}

/// A short identifier GA may receive: letters, digits, `.`, `-`, `_` and spaces, at most 40.
fn short(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && text.len() <= 40 && text.chars().all(|c| c.is_ascii_alphanumeric() || " -_.".contains(c))).then_some(text)
}

/// The request body for one batch.
fn request(client_id: &str, events: &[Value], device: &Device) -> Value {
    let mut body = json!({ "client_id": client_id, "events": events });
    let mut info = serde_json::Map::new();
    info.insert("category".into(), json!("desktop"));
    for (key, value) in [
        ("operating_system", &device.operating_system),
        ("operating_system_version", &device.operating_system_version),
        ("language", &device.language),
    ] {
        if let Some(value) = short(value) {
            info.insert(key.into(), json!(value));
        }
    }
    body["device"] = Value::Object(info);
    if let Some(country) = device.country.as_deref().filter(|c| c.len() == 2 && c.chars().all(|c| c.is_ascii_uppercase())) {
        body["user_location"] = json!({ "country_id": country });
    }
    body
}

/// Sends events (at most 25 per request, GA's limit). Failures are ignored.
pub fn send(events: Vec<Value>, device: &Device) {
    let Some((measurement_id, secret)) = credentials() else { return };
    if events.is_empty() || do_not_track() {
        return;
    }
    let url = format!("https://www.google-analytics.com/mp/collect?measurement_id={measurement_id}&api_secret={secret}");
    let agent = crate::http::agent_builder().timeout(std::time::Duration::from_secs(10)).build();
    let client_id = install_id();
    for batch in events.chunks(25) {
        let _ = agent.post(&url).send_json(request(&client_id, batch, device));
    }
}

/// The country of a locale name: `ko-KR`, `ko_KR.UTF-8`, `zh-Hans-CN` → `KR`, `KR`, `CN`. A
/// language alone (`en`) or a numeric region (`es-419`) has none.
pub fn locale_country(locale: &str) -> Option<String> {
    let locale = locale.split(['.', '@']).next().unwrap_or("");
    locale
        .split(['-', '_'])
        .skip(1)
        .find(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_alphabetic()))
        .map(|part| part.to_ascii_uppercase())
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
    fn requests_carry_the_device_and_country_only_as_short_identifiers() {
        let device = Device {
            operating_system: "macOS".into(),
            operating_system_version: "15.1".into(),
            language: "ko".into(),
            country: Some("KR".into()),
        };
        let body = request("abc", &[json!({ "name": "app_launched" })], &device);
        assert_eq!(body["client_id"], "abc");
        assert_eq!(
            body["device"],
            json!({ "category": "desktop", "operating_system": "macOS", "operating_system_version": "15.1", "language": "ko" })
        );
        assert_eq!(body["user_location"], json!({ "country_id": "KR" }));
        // Anything that isn't a short identifier stays out.
        let odd = Device {
            operating_system: "Windows".into(),
            operating_system_version: "/home/me/secret\n".into(),
            language: String::new(),
            country: Some("Korea".into()),
        };
        let body = request("abc", &[], &odd);
        assert_eq!(body["device"], json!({ "category": "desktop", "operating_system": "Windows" }));
        assert!(body.get("user_location").is_none());
    }

    #[test]
    fn countries_from_locale_names() {
        assert_eq!(locale_country("ko-KR").as_deref(), Some("KR"));
        assert_eq!(locale_country("ko_KR.UTF-8").as_deref(), Some("KR"));
        assert_eq!(locale_country("zh-Hans-CN").as_deref(), Some("CN"));
        assert_eq!(locale_country("en_US@calendar=gregorian").as_deref(), Some("US"));
        assert_eq!(locale_country("en"), None);
        assert_eq!(locale_country("es-419"), None);
        assert_eq!(locale_country(""), None);
    }

    #[test]
    fn do_not_track_values() {
        std::env::set_var("DO_NOT_TRACK", "1");
        assert!(do_not_track());
        std::env::remove_var("DO_NOT_TRACK");
        assert!(!do_not_track());
    }
}
