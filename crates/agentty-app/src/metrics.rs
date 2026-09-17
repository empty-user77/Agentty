//! Records opt-in usage metrics through `agentty_bridge::metrics` (nothing happens unless enabled).

use crate::settings::settings;
use gpui::App;
use serde_json::Value;

fn os_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    })
}

pub fn enabled(cx: &App) -> bool {
    settings(cx).metrics.enabled && !agentty_bridge::metrics::do_not_track()
}

/// Records `event` with `props` (filtered against the allow-list) in the background.
pub fn track(cx: &App, event: &'static str, props: Value) {
    if !enabled(cx) {
        return;
    }
    cx.background_executor()
        .spawn(async move {
            let id = agentty_bridge::metrics::install_id();
            let now = crate::ui::now_ms();
            if let Some(record) = agentty_bridge::metrics::record_for(event, &props, env!("CARGO_PKG_VERSION"), os_version(), &id, now) {
                agentty_bridge::metrics::append(&record);
            }
        })
        .detach();
}

/// Uploads pending events every 15 minutes when an endpoint is configured.
pub fn start_uploads(cx: &mut App) {
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(std::time::Duration::from_secs(60)).await;
        let endpoint = cx.update(|cx| {
            let endpoint = std::env::var("AGENTTY_METRICS_ENDPOINT").ok().unwrap_or_else(|| settings(cx).metrics.endpoint.clone());
            (enabled(cx) && !endpoint.trim().is_empty()).then_some(endpoint)
        });
        let Ok(endpoint) = endpoint else { break };
        if let Some(endpoint) = endpoint {
            cx.background_executor().spawn(async move { agentty_bridge::metrics::flush(endpoint.trim()) }).await;
        }
        cx.background_executor().timer(std::time::Duration::from_secs(15 * 60)).await;
    })
    .detach();
}
