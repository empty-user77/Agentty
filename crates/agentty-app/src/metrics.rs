//! Queues usage events and sends them to GA4 through `agentty_bridge::metrics` once a minute.

use gpui::App;
use serde_json::Value;
use std::sync::Mutex;

static QUEUE: Mutex<Vec<Value>> = Mutex::new(Vec::new());

fn os_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(crate::platform::os_version)
}

/// Queues `event` (dropped unless analytics is built in and not turned off by `DO_NOT_TRACK`).
pub fn track(_cx: &App, event: &'static str, props: Value) {
    if !agentty_bridge::metrics::enabled() {
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
        if !events.is_empty() {
            cx.background_executor().spawn(async move { agentty_bridge::metrics::send(events) }).await;
        }
    })
    .detach();
}
