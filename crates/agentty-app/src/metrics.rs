//! Queues usage events and sends them to GA4 through `agentty_bridge::metrics` once a minute,
//! with an `app_running` event every five minutes while the app is open.

use gpui::App;
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static QUEUE: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// The computer's model and maker (`platform::hardware_model`).
static MODEL: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

fn os_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(crate::platform::os_version)
}

/// The platform, its version, the computer's model and the country of the region setting; the
/// language Agentty is shown in and the size of the main display are read at each upload.
fn device(cx: &App) -> agentty_bridge::metrics::Device {
    // Read off the UI thread when uploads start (`sysctl`, `reg`): empty until then.
    let (model, brand) = MODEL.get().cloned().unwrap_or_default();
    let screen_resolution = cx
        .primary_display()
        .map(|display| {
            let size = display.bounds().size;
            format!("{}x{}", f32::from(size.width).round() as u32, f32::from(size.height).round() as u32)
        })
        .unwrap_or_default();
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
        model,
        brand,
        screen_resolution,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
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

/// The launch: `app_launched`, where this copy came from (`campaign_details`, GA's traffic source)
/// and, the first time on this install, `app_installed`.
pub fn launched(cx: &App, windows: usize) {
    if !allowed(cx) {
        return;
    }
    // Made here, so the first launch is known before the first upload.
    agentty_bridge::metrics::install_id();
    track(cx, "campaign_details", serde_json::json!({ "source": crate::platform::install_source(), "medium": "app" }));
    if agentty_bridge::metrics::first_launch() {
        track(cx, "app_installed", serde_json::json!({}));
    }
    track(cx, "app_launched", serde_json::json!({ "windows": windows }));
}

/// The screen in front (`home`, `terminals`, `flow`, …) as a page view.
pub fn screen(cx: &App, name: &'static str) {
    if !allowed(cx) {
        return;
    }
    if let Some(event) = agentty_bridge::metrics::page_view(name, env!("CARGO_PKG_VERSION"), os_version()) {
        if let Ok(mut queue) = QUEUE.lock() {
            queue.push(event);
        }
    }
}

/// How often `app_running` says the app is open. Each one carries the time since the previous,
/// so GA's engagement time stays exact whatever this is; it only sets how many requests it takes.
const RUNNING_EVERY: Duration = Duration::from_secs(5 * 60);

/// Sends queued events every minute, plus `app_running` every five minutes while the app is open.
/// Both ride one timer: no extra wake-ups, and at most one request a minute.
pub fn start_uploads(cx: &mut App) {
    if !agentty_bridge::metrics::enabled() {
        return;
    }
    let _ = std::thread::Builder::new().name("metrics-model".into()).spawn(|| MODEL.get_or_init(crate::platform::hardware_model).clone());
    cx.spawn(async move |cx| {
        let mut last_running = Instant::now();
        loop {
            cx.background_executor().timer(Duration::from_secs(60)).await;
            let mut events = QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
            // Consent can be withdrawn between two uploads: whatever was queued before is dropped.
            let Some(device) = cx.update(|cx| allowed(cx).then(|| device(cx))).ok().flatten() else {
                // Time without consent is never reported later.
                last_running = Instant::now();
                continue;
            };
            let open = last_running.elapsed();
            if open >= RUNNING_EVERY {
                // A computer that slept past a tick wasn't in use: count one interval at most.
                let open = open.min(RUNNING_EVERY + Duration::from_secs(60));
                events.push(agentty_bridge::metrics::app_running(open, env!("CARGO_PKG_VERSION"), os_version()));
                last_running = Instant::now();
            }
            if !events.is_empty() {
                cx.background_executor().spawn(async move { agentty_bridge::metrics::send(events, &device) }).await;
            }
        }
    })
    .detach();
}
