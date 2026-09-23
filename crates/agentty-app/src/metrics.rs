//! Queues usage events and sends them to GA4 through `agentty_bridge::metrics` once a minute.

use gpui::App;
use serde_json::Value;
use std::sync::Mutex;

static QUEUE: Mutex<Vec<Value>> = Mutex::new(Vec::new());

fn os_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(crate::platform::os_version)
}

/// The platform, its version and the country of the region setting; the language is the one
/// Agentty is shown in, read at each upload.
fn device(cx: &App) -> agentty_bridge::metrics::Device {
    static SYSTEM: std::sync::OnceLock<(String, String, Option<String>)> = std::sync::OnceLock::new();
    let (os, version, country) = SYSTEM
        .get_or_init(|| {
            let os = if cfg!(target_os = "macos") {
                "macOS"
            } else if cfg!(windows) {
                "Windows"
            } else {
                "Linux"
            };
            // `Windows 10.0.26100` → `10.0.26100`; macOS answers the number alone, Linux its
            // distribution (`Ubuntu 24.04`).
            let version = os_version().strip_prefix("Windows ").unwrap_or(os_version()).to_string();
            (os.to_string(), version, agentty_bridge::metrics::locale_country(&crate::platform::region_locale()))
        })
        .clone();
    agentty_bridge::metrics::Device {
        operating_system: os,
        operating_system_version: version,
        language: crate::settings::settings(cx).language.code().to_string(),
        country,
    }
}

/// Whether usage events may be collected right now: built in, not turned off by `DO_NOT_TRACK`,
/// and consented to in Settings.
fn allowed(cx: &App) -> bool {
    agentty_bridge::metrics::enabled() && crate::settings::settings(cx).analytics
}

/// Queues `event` (dropped unless analytics is built in, consented to, and not turned off by
/// `DO_NOT_TRACK`).
pub fn track(cx: &App, event: &'static str, props: Value) {
    if !allowed(cx) {
        return;
    }
    if let Some(event) = agentty_bridge::metrics::event(event, &props, env!("CARGO_PKG_VERSION"), os_version()) {
        if let Ok(mut queue) = QUEUE.lock() {
            queue.push(event);
        }
    }
}

/// Sends queued events every minute.
pub fn start_uploads(cx: &mut App) {
    if !agentty_bridge::metrics::enabled() {
        return;
    }
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(std::time::Duration::from_secs(60)).await;
        let events = QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        // Consent can be withdrawn between two uploads: whatever was queued before is dropped.
        let Some(device) = cx.update(|cx| allowed(cx).then(|| device(cx))).ok().flatten() else {
            continue;
        };
        if !events.is_empty() {
            cx.background_executor().spawn(async move { agentty_bridge::metrics::send(events, &device) }).await;
        }
    })
    .detach();
}
