//! The mark on a pane's bar when its agent runs on another login than the window's: the login's
//! initial in a small ring, in the blue of the account icon's badge for a window on another
//! account, and a line of that blue under the bar. A click opens what that login has: its plan, the 5-hour and weekly
//! limits with when they reset, and what it used over the last days.

use super::account_usage::Limit;
use super::{Pane, Workbench};
use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{popover, TypeScale};
use agentty_bridge::accounts::AccountStore;
use agentty_bridge::agent_auth::AuthAgent;
use agentty_bridge::model::Agent;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, MouseButton};

const MENU_KEY: &str = "login-mark";
const BAR_WIDTH: f32 = 96.;

impl Workbench {
    /// The agent of `pane` and the login it runs on, when that login is not this window's.
    pub(super) fn foreign_login_of(&self, pane: &Pane, cx: &gpui::App) -> Option<(AuthAgent, Option<String>)> {
        let view = pane.read(cx);
        let agent = match view.agent_kind()? {
            crate::launch::PaneKind::Claude => AuthAgent::Claude,
            crate::launch::PaneKind::Codex => AuthAgent::Codex,
            crate::launch::PaneKind::Shell => return None,
        };
        let login = view.spec.accounts.get(agent);
        (login != self.accounts.get(agent)).then(|| (agent, login.map(str::to_string)))
    }

    /// Whether `pane` has a bar over it that carries the mark: a split pane's header, or the agent
    /// bar (Settings → Appearance).
    pub(super) fn pane_bar_shown(&self, pane: &Pane, cx: &gpui::App) -> bool {
        let split = self
            .locate(pane)
            .and_then(|(w, t)| self.workspaces[w].tabs.get(t))
            .is_some_and(|tab| matches!(tab.root, super::PaneNode::Split { .. }));
        split || crate::settings::settings(cx).agent_bar
    }

    /// The mark, and its menu while open.
    pub(super) fn foreign_login_chip(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.login_chip_at(pane, false, cx)
    }

    /// The same mark in front of a chat's footer (`in_chat`), its menu opening upwards.
    pub(super) fn login_chip_at(&self, pane: &Pane, in_chat: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (agent, login) = self.foreign_login_of(pane, cx)?;
        let name = self.login_name(agent, login.as_deref(), cx);
        let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let id = pane.entity_id();
        let open = self.login_mark_open == Some((id, in_chat));
        let mark = div()
            .id((if in_chat { "chat-account" } else { "pane-account" }, id.as_u64() as usize))
            .relative()
            .flex_shrink_0()
            .size(px(16.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .border_1()
            .border_color(hex_alpha(Chrome::BLUE, if open { 1. } else { 0.7 }))
            .bg(hex_alpha(Chrome::BLUE, if open { 0.32 } else { 0.16 }))
            .hover(|s| s.bg(hex_alpha(Chrome::BLUE, 0.3)))
            .text_color(hex(Chrome::BLUE))
            .text_size(px(9.))
            .font_weight(crate::theme::EMPHASIS)
            .when(!open, |d| d.tooltip(crate::ui::Tooltip::text(name.clone(), None)))
            // The bar under it marks the pane active on a click; the mark only opens its menu.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                if this.just_dismissed(MENU_KEY) {
                    return;
                }
                this.login_mark_open = if this.login_mark_open == Some((id, in_chat)) { None } else { Some((id, in_chat)) };
                if this.login_mark_open.is_some() {
                    // Fresh plan limits and spend for the menu.
                    this.refresh_account_rows(cx);
                    this.refresh_account_usage(cx);
                }
                cx.notify();
            }))
            .child(initial);
        let menu = open.then(|| {
            let menu = self.render_login_menu(agent, login.as_deref(), name, cx);
            if in_chat {
                super::layout::popover_above(menu, 3)
            } else {
                super::layout::bar_popover(menu, 3, cx)
            }
        });
        Some(mark.children(menu).into_any_element())
    }

    fn render_login_menu(&self, agent: AuthAgent, login: Option<&str>, name: String, cx: &mut Context<Self>) -> impl IntoElement {
        let row = self.account_rows.iter().find(|row| row.agent == agent && row.id.as_deref() == login);
        let now = crate::ui::now_ms();
        let (agent_name, model_agent) = match agent {
            AuthAgent::Claude => ("Claude Code", Agent::Claude),
            AuthAgent::Codex => ("Codex", Agent::Codex),
        };
        let plan = row.and_then(|row| row.account.plan_name());
        let subtitle = [Some(agent_name.to_string()), plan].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let warning = row.and_then(|row| row.login_warning(now, cx));
        let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();

        // The 5-hour and weekly windows, as the status line last reported them.
        let limits: Vec<Limit> = row
            .and_then(|row| row.limits.as_ref())
            .map(|limits| {
                [("tray.session", limits.session), ("tray.weekly", limits.weekly)]
                    .into_iter()
                    .filter_map(|(label, window)| window.map(|w| Limit { label, used_percent: w.used_percent, resets_at: w.resets_at }))
                    .collect()
            })
            .unwrap_or_default();
        // Spend over the last days: the usage page's figure for this login.
        let usage_name = login.map(|id| {
            AccountStore::load()
                .of(agent)
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.email.clone().or_else(|| a.org.clone()))
                .unwrap_or_else(|| id.to_string())
        });
        let spend = self.account_usage.iter().find(|u| u.agent == model_agent && u.account == usage_name).map(|u| u.cost_label());

        let meter = |limit: &Limit| {
            let percent = limit.used_percent.clamp(0., 100.);
            let color = if percent >= 90. {
                Chrome::ERROR
            } else if percent >= 70. {
                Chrome::WARNING
            } else {
                Chrome::ACCENT
            };
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(44.)).flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, limit.label)))
                        .child(
                            div()
                                .w(px(BAR_WIDTH))
                                .h(px(5.))
                                .flex_shrink_0()
                                .rounded_full()
                                .bg(hex_alpha(Chrome::BRIGHT, 0.1))
                                .child(div().h_full().rounded_full().w(px(BAR_WIDTH * percent as f32 / 100.)).bg(hex(color))),
                        )
                        .child(div().flex_1().text_right().t_small().text_color(hex(Chrome::BRIGHT)).child(format!("{percent:.0}%"))),
                )
                .child(div().pl(px(52.)).t_caption().text_color(hex(Chrome::MUTED)).child(limit.resets_in(cx)))
        };

        let mut body = div().flex().flex_col().gap_2p5();
        match warning {
            Some(warning) => body = body.child(div().t_small().text_color(hex(Chrome::WARNING)).child(warning)),
            None if limits.is_empty() => {
                body = body.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "account.no_limits")))
            }
            None => {
                for limit in &limits {
                    body = body.child(meter(limit));
                }
            }
        }
        if let Some(spend) = spend {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .t_small()
                    .child(div().text_color(hex(Chrome::MUTED)).child(t(cx, "tray.usage_title")))
                    .child(div().text_color(hex(Chrome::BRIGHT)).child(spend)),
            );
        }

        popover()
            .id("login-mark-menu")
            .w(px(280.))
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                this.login_mark_open = None;
                this.note_dismissed(MENU_KEY);
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(30.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(hex_alpha(Chrome::BLUE, 0.2))
                            .border_1()
                            .border_color(hex_alpha(Chrome::BLUE, 0.6))
                            .text_color(hex(Chrome::BLUE))
                            .t_body()
                            .font_weight(crate::theme::EMPHASIS)
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(name))
                            .child(div().t_caption().text_color(hex(Chrome::MUTED)).truncate().child(subtitle)),
                    ),
            )
            .child(div().h(px(1.)).bg(hex(Chrome::BORDER)))
            .child(body)
            .child(div().t_caption().text_color(hex(Chrome::BLUE)).child(t(cx, "account.other_than_window")))
    }
}
