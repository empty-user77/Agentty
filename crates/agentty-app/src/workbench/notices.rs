//! Notification center: agent completions, input requests, bells and `agentty notify` messages,
//! with a panel, "jump to latest unread" and macOS notifications while Agentty is in the background.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::settings::settings;
use crate::terminal::NoticeKind;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use crate::ui::{action_button, hint, now_ms, popover, relative_time};
use gpui::{div, prelude::*, px, ClickEvent, Context, FontWeight, Window};

pub(super) const MAX_NOTICES: usize = 100;

#[derive(Clone)]
pub struct Notice {
    pub pane_id: u64,
    pub kind: NoticeKind,
    pub text: String,
    pub source: String,
    pub at_ms: u64,
    pub read: bool,
    /// A message to a chat service (Slack, …) failed: shown in red, and a click opens the
    /// notification settings, where the reason and the fix are.
    pub chat_failed: bool,
}

impl Workbench {
    pub(super) fn record_notice(&mut self, pane: &Pane, kind: NoticeKind, message: Option<String>, cx: &mut Context<Self>) {
        let view = pane.read(cx);
        let source = match view.display_kind().agent() {
            Some(agent) => agent.display_name().to_string(),
            None => crate::ui::tilde(&view.display_cwd()),
        };
        let text = message.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| {
            match kind {
                NoticeKind::Finished => t(cx, "status.finished"),
                NoticeKind::Permission => t(cx, "status.permission"),
                NoticeKind::Question => t(cx, "status.question"),
                NoticeKind::Message => t(cx, "notice.message"),
                NoticeKind::Bell => t(cx, "notice.bell"),
            }
            .to_string()
        });
        let workspace = self.locate(pane).map(|(w, _)| self.workspace_label(&self.workspaces[w], cx)).unwrap_or_default();
        let workspace_id = self.locate(pane).map(|(w, _)| self.workspaces[w].id);
        if kind == NoticeKind::Finished {
            crate::metrics::track(cx, "agent_turn_finished", serde_json::json!({ "tool": view.tool_id() }));
        }
        let pane_id = view.pane_id;
        self.notices.insert(
            0,
            Notice {
                pane_id,
                kind,
                text: text.clone(),
                source: view_title_for_bubble(&source, &workspace),
                at_ms: now_ms(),
                read: false,
                chat_failed: false,
            },
        );
        self.notices.truncate(MAX_NOTICES);

        let prefs = settings(cx);
        let sort_finished_to_top = prefs.sort_finished_to_top;
        let background = !self.in_front() || self.is_mini();
        // The user is looking at this very pane: no need to tell them elsewhere.
        let in_view = self.pane_in_view(pane_id, cx);
        let asks = matches!(kind, NoticeKind::Permission | NoticeKind::Question);
        let answer_request = asks && prefs.notify_answer_requests && !in_view;
        // A permission request that can be answered without the terminal: its notification and
        // mini bubble carry Allow / Deny, and say in full what would be allowed.
        let view = pane.read(cx);
        let ask = (kind == NoticeKind::Permission && view.answerable_request()).then_some(view.ask_serial);
        let request = ask.and_then(|_| view.ask_request.clone()).map(|(tool, request)| match request {
            Some(request) => format!("{tool}\n{request}"),
            None => tool,
        });
        if prefs.system_notifications && kind != NoticeKind::Bell && (background || prefs.notify_when_focused || answer_request) {
            let title = view_title_for_bubble(&source, &workspace);
            match ask {
                Some(ask) => crate::notifications::show_request(
                    pane_id,
                    ask,
                    &title,
                    request.as_deref().unwrap_or(&text),
                    t(cx, "notice.allow"),
                    t(cx, "notice.deny"),
                ),
                None => crate::notifications::show(pane_id, &title, &text),
            }
        }
        self.send_chat_notice(pane_id, kind, &source, &workspace, &text, cx);
        if kind != NoticeKind::Bell {
            if let Some(mini) = self.mini.as_ref().map(|m| m.view.clone()) {
                let title = view_title_for_bubble(&source, &workspace);
                mini.update(cx, |m, cx| m.push_bubble(pane_id, title, text.clone(), kind, ask, cx));
            }
        }
        // The workspace with the latest result moves to the top of its group — opt-in: a fixed
        // order is what most people expect from a sidebar they arranged themselves.
        if let (NoticeKind::Finished, Some(id)) = (kind, workspace_id) {
            if sort_finished_to_top {
                self.move_workspace_to_top(id, cx);
            }
        }
        cx.notify();
    }

    /// Whether this window is in front: it was the active one when it last drew, and Agentty is the
    /// active app now. A window that is hidden, covered or behind a locked screen draws nothing, so
    /// what it saw last can be old; the app's own state is asked at the moment.
    fn in_front(&self) -> bool {
        self.window_active && crate::native::app_is_active()
    }

    /// Whether the user is looking at this very pane: the window is in front, no page covers the
    /// terminals, and the pane is one of the tab's split panes — not only its focused one, since
    /// every pane of the tab on screen is equally in view.
    pub(super) fn pane_in_view(&self, pane_id: u64, cx: &gpui::App) -> bool {
        if !self.in_front() || self.is_mini() || self.page.is_some() {
            return false;
        }
        let Some(ws) = self.workspaces.get(self.active_workspace) else { return false };
        let Some(tab) = ws.tabs.get(ws.active_tab) else { return false };
        tab.root.leaves().iter().any(|pane| pane.read(cx).pane_id == pane_id)
    }

    pub(super) fn mark_pane_read(&mut self, pane_id: u64) -> bool {
        let mut changed = false;
        for notice in self.notices.iter_mut().filter(|n| n.pane_id == pane_id && !n.read) {
            notice.read = true;
            changed = true;
        }
        changed
    }

    pub(super) fn unread_count(&self) -> usize {
        self.notices.iter().filter(|n| !n.read).count()
    }

    pub(super) fn jump_to_pane_id(&mut self, pane_id: u64, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(pane) = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane_id) else { return false };
        let Some((w, tab)) = self.locate(&pane) else { return false };
        self.activate_workspace(w, window, cx);
        let ws = &mut self.workspaces[w];
        ws.active_tab = tab;
        ws.tabs[tab].active = pane.clone();
        self.focus_pane(&pane, window, cx);
        pane.update(cx, |view, cx| view.acknowledge(cx));
        self.mark_pane_read(pane_id);
        self.notices_open = false;
        cx.notify();
        true
    }

    /// A notification's (or the mini window's) Allow / Deny for permission request `ask` of
    /// `pane_id`. Typed into the terminal only while that request still waits there; otherwise
    /// (answered meanwhile, another request since, the prompt gone) the pane is brought up instead,
    /// so the click is never lost and never lands on something else.
    pub(super) fn answer_request(&mut self, pane_id: u64, ask: u64, allow: bool, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane_id);
        let answered = pane.is_some_and(|pane| pane.update(cx, |view, _| view.answer_request(ask, allow)));
        if answered {
            self.mark_pane_read(pane_id);
            crate::notifications::withdraw(pane_id);
            if let Some(mini) = self.mini.as_ref().map(|m| m.view.clone()) {
                mini.update(cx, |m, cx| m.drop_bubbles_of(pane_id, cx));
            }
            cx.notify();
            return;
        }
        if self.mini.is_some() {
            self.exit_mini(Some(pane_id), window, cx);
        } else {
            window.activate_window();
            cx.activate(true);
            self.jump_to_pane_id(pane_id, window, cx);
        }
    }

    pub(super) fn jump_to_unread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unread: Vec<u64> = self.notices.iter().filter(|n| !n.read).map(|n| n.pane_id).collect();
        for pane_id in unread {
            if self.jump_to_pane_id(pane_id, window, cx) {
                return;
            }
            // The pane is gone; its notices can't be acted on.
            self.mark_pane_read(pane_id);
        }
        self.set_status(t(cx, "notice.none_unread"), cx);
    }

    /// Notification bell at the far right of the status bar, with the unread count.
    pub(super) fn render_notices_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let unread = self.unread_count();
        // Gold when something is waiting: the blue sat too close to the chrome to notice. The count
        // is a badge on the bell's corner, so the button keeps the size of its neighbours.
        let color = if unread > 0 { Chrome::FAVORITE } else { Chrome::MUTED };
        div()
            .id("status-notices")
            // The same box as the other header icons, so the row reads as one set.
            .size(px(crate::ui::ICON_BUTTON))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .flex_shrink_0()
            .cursor_pointer()
            .text_color(hex(color))
            .when(self.notices_open, |d| d.bg(hex(Chrome::SELECTED)))
            .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
            .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.notices"), Some("⇧⌘U")))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if this.just_dismissed("notices") {
                    return;
                }
                this.notices_open = !this.notices_open;
                this.launcher_open = false;
                cx.notify();
            }))
            .child(crate::ui::icon("bell", crate::ui::IconSize::BUTTON, hex(color)))
            .when(unread > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .right(px(0.))
                        .min_w(px(13.))
                        .h(px(13.))
                        .px(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(hex(Chrome::FAVORITE))
                        .text_color(hex(Chrome::ACTIVITY_BAR))
                        .text_size(px(9.))
                        .font_weight(crate::theme::EMPHASIS)
                        .child(if unread > 9 { "9+".to_string() } else { unread.to_string() }),
                )
            })
    }

    pub(super) fn render_notices(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = now_ms();
        let panes = self.all_panes();
        let tool_of = |pane_id: u64, cx: &gpui::App| panes.iter().find(|p| p.read(cx).pane_id == pane_id).map(|p| p.read(cx).tool_id());
        let mut list = div().id("notice-list").flex().flex_col().max_h(px(420.)).overflow_y_scroll();
        if self.notices.is_empty() {
            list = list.child(hint(t(cx, "notice.empty")));
        }
        for (index, notice) in self.notices.iter().take(40).enumerate() {
            let pane_id = notice.pane_id;
            let tool = tool_of(pane_id, cx).filter(|_| !notice.chat_failed);
            let (chat_failed, at_ms) = (notice.chat_failed, notice.at_ms);
            let color = match notice.kind {
                _ if chat_failed => Chrome::ERROR,
                NoticeKind::Finished => Chrome::SUCCESS,
                NoticeKind::Permission | NoticeKind::Question => Chrome::ATTENTION,
                NoticeKind::Message => Chrome::PURPLE,
                NoticeKind::Bell => Chrome::WARNING,
            };
            list = list.child(
                div()
                    .id(("notice", index))
                    .flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if chat_failed {
                            // Found again by what it is: a notice arriving meanwhile shifts the rows.
                            if let Some(notice) =
                                this.notices.iter_mut().find(|n| n.chat_failed && n.pane_id == pane_id && n.at_ms == at_ms)
                            {
                                notice.read = true;
                            }
                            this.notices_open = false;
                            this.page = Some(super::Page::Settings);
                            this.settings_section = super::settings_page::SettingsSection::Integrations;
                            return cx.notify();
                        }
                        if !this.jump_to_pane_id(pane_id, window, cx) {
                            this.mark_pane_read(pane_id);
                            cx.notify();
                        }
                    }))
                    .child(div().pt_1p5().child(div().size(px(7.)).rounded_full().bg(if notice.read {
                        hex_alpha(color, 0.3)
                    } else {
                        hex(color)
                    })))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .t_small()
                                    .children(tool.map(|tool| crate::brand::avatar(tool, 16.)))
                                    .child(div().flex_1().truncate().text_color(hex(Chrome::MUTED)).child(notice.source.clone()))
                                    .child(div().text_color(hex(Chrome::MUTED)).child(relative_time(now, notice.at_ms))),
                            )
                            .child(
                                div()
                                    .t_body()
                                    .truncate()
                                    .text_color(if notice.read { hex(Chrome::MUTED) } else { hex(Chrome::BRIGHT) })
                                    .when(!notice.read, |d| d.font_weight(FontWeight::MEDIUM))
                                    .child(notice.text.clone()),
                            ),
                    ),
            );
        }

        popover()
            .id("notices")
            .w(px(360.))
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if std::mem::take(&mut this.notices_open) {
                    this.note_dismissed("notices");
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .child(div().t_body().font_weight(crate::theme::EMPHASIS).child(tf(
                        cx,
                        "notice.title",
                        &[("n", &self.unread_count().to_string())],
                    )))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(action_button(
                                "notice-read-all",
                                t(cx, "notice.read_all"),
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.notices.iter_mut().for_each(|n| n.read = true);
                                    cx.notify();
                                }),
                            ))
                            .child(action_button(
                                "notice-clear",
                                t(cx, "notice.clear"),
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.notices.clear();
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .child(list)
            .child(div().px_3().py_1p5().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "notice.shortcut")))
    }
}

/// An agent waiting for the user, somewhere off screen.
struct Waiting {
    pane_id: u64,
    permission: bool,
    /// "Claude Code · workspace".
    who: String,
    /// What it asks, when it says.
    what: Option<String>,
}

impl Workbench {
    /// Agents that asked the user something (a question, a permission) and wait, where the user
    /// is not looking — newest request first.
    fn waiting_for_user(&self, cx: &gpui::App) -> Vec<Waiting> {
        let mut waiting: Vec<(u64, Waiting)> = self
            .all_panes()
            .into_iter()
            .filter_map(|pane| {
                let view = pane.read(cx);
                if !view.status.needs_user() || !view.attention || self.pane_in_view(view.pane_id, cx) {
                    return None;
                }
                let (permission, what) = match &view.status {
                    crate::terminal::AgentStatus::Permission(what) => (true, what.clone()),
                    crate::terminal::AgentStatus::Question(what) => (false, what.clone()),
                    _ => return None,
                };
                let source = match view.display_kind().agent() {
                    Some(agent) => agent.display_name().to_string(),
                    None => crate::ui::tilde(&view.display_cwd()),
                };
                let workspace = self.locate(&pane).map(|(w, _)| self.workspace_label(&self.workspaces[w], cx)).unwrap_or_default();
                let at = self.notices.iter().find(|n| n.pane_id == view.pane_id).map_or(0, |n| n.at_ms);
                Some((at, Waiting { pane_id: view.pane_id, permission, who: view_title_for_bubble(&source, &workspace), what }))
            })
            .collect();
        waiting.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        waiting.into_iter().map(|(_, w)| w).collect()
    }

    /// Stays on screen while an agent waits for the user out of sight — until it is answered or
    /// looked at — so a question never sits unnoticed in a tab in the back. A click goes there.
    pub(super) fn render_waiting_banner(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let waiting = self.waiting_for_user(cx);
        let first = waiting.first()?;
        let pane_id = first.pane_id;
        let (icon, label) = if first.permission {
            ("shield-alert", t(cx, "status.permission"))
        } else {
            ("message-circle-question", t(cx, "status.question"))
        };
        let more = waiting.len() - 1;
        let right = 16. + self.overlay_right_inset(cx);
        Some(
            div().absolute().bottom(px(super::chrome::STATUS_BAR_HEIGHT + 12.)).right(px(right)).child(
                div()
                    .id("waiting-banner")
                    .occlude()
                    .w(px(340.))
                    .flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(hex(Chrome::OVERLAY))
                    .border_1()
                    .border_color(hex(Chrome::FAVORITE))
                    .shadow_lg()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.jump_to_pane_id(pane_id, window, cx);
                    }))
                    .child(div().pt_0p5().child(crate::ui::icon(icon, crate::ui::IconSize::INLINE, hex(Chrome::FAVORITE))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .flex()
                                    .gap_1p5()
                                    .t_small()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(div().font_weight(FontWeight::MEDIUM).text_color(hex(Chrome::FAVORITE)).child(label))
                                    .child(div().flex_1().truncate().child(first.who.clone())),
                            )
                            .children(
                                first.what.clone().map(|what| div().t_body().text_color(hex(Chrome::BRIGHT)).line_clamp(2).child(what)),
                            )
                            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(if more > 0 {
                                tf(cx, "waiting.go_more", &[("n", &more.to_string())])
                            } else {
                                t(cx, "waiting.go").to_string()
                            })),
                    ),
            ),
        )
    }
}

/// Workspace first, named as its sidebar card names it, then the agent: every notice reads the
/// same way as the sidebar.
pub(super) fn view_title_for_bubble(source: &str, workspace: &str) -> String {
    if workspace.is_empty() {
        source.to_string()
    } else {
        format!("{workspace} · {source}")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn notices_name_the_workspace_first() {
        assert_eq!(super::view_title_for_bubble("Claude Code", "CosmicaDesktop"), "CosmicaDesktop · Claude Code");
        assert_eq!(super::view_title_for_bubble("Claude Code", ""), "Claude Code");
    }
}
