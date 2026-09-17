//! Warns when the service behind a running agent (Claude Code, Codex, Amp) is degraded, with a
//! link to its status page. Checked every five minutes, only for agents that are running.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::launch::PaneKind;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use agentty_bridge::service_status::{self, ServiceStatus};
use gpui::{div, prelude::*, ClickEvent, Context};
use std::time::Duration;

const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

impl Workbench {
    /// Providers whose agent is running in some pane.
    fn running_providers(&self, cx: &gpui::App) -> Vec<&'static str> {
        let mut ids = Vec::new();
        for pane in self.all_panes() {
            let view = pane.read(cx);
            if !view.is_running() {
                continue;
            }
            let id = match view.agent_kind() {
                Some(PaneKind::Claude) => Some("claude"),
                Some(PaneKind::Codex) => Some("codex"),
                _ if view.spec.title == "Amp" => Some("amp"),
                _ => None,
            };
            if let Some(id) = id.filter(|id| !ids.contains(id)) {
                ids.push(id);
            }
        }
        ids
    }

    pub(super) fn start_service_status_checks(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            let Ok(providers) = this.read_with(cx, |this, cx| this.running_providers(cx)) else { break };
            if !providers.is_empty() {
                let results: Vec<ServiceStatus> = cx
                    .background_spawn(async move {
                        providers
                            .iter()
                            .filter_map(|id| service_status::provider(id))
                            .filter_map(|p| service_status::fetch(p).ok())
                            .collect()
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        for status in results {
                            this.service_status.insert(status.provider, status);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
            cx.background_executor().timer(CHECK_INTERVAL).await;
        })
        .detach();
    }

    /// Banner under the tab strip for degraded services of running agents.
    pub(super) fn render_service_banner(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let running = self.running_providers(cx);
        let status = running
            .iter()
            .filter_map(|id| self.service_status.get(id))
            .find(|s| s.degraded && !self.status_dismissed.contains(&format!("{}:{}", s.provider, s.detail)))?
            .clone();
        let provider = service_status::provider(status.provider)?;
        let key = format!("{}:{}", status.provider, status.detail);
        let page = provider.page_url;
        let message = if status.detail.is_empty() {
            tf(cx, "service.degraded", &[("name", provider.name)])
        } else {
            tf(cx, "service.degraded_detail", &[("name", provider.name), ("detail", &status.detail)])
        };
        Some(
            div()
                .flex_shrink_0()
                .px_3()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .bg(hex_alpha(Chrome::WARNING, 0.14))
                .border_b_1()
                .border_color(hex_alpha(Chrome::WARNING, 0.45))
                .t_small()
                .text_color(hex(Chrome::BRIGHT))
                .child(icon("bell-dot", IconSize::INLINE, hex(Chrome::WARNING)))
                .child(div().flex_1().min_w_0().truncate().child(message))
                .child(
                    div()
                        .id("service-status-page")
                        .px_2()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(hex(Chrome::BLUE))
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .child(t(cx, "service.open_status"))
                        .on_click(move |_, _, cx| cx.open_url(page)),
                )
                .child(crate::ui::icon_only(
                    "service-status-dismiss",
                    "x",
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.status_dismissed.insert(key.clone());
                        cx.notify();
                    }),
                )),
        )
    }
}
