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

const MAX_NOTICES: usize = 100;

#[derive(Clone)]
pub struct Notice {
    pub pane_id: u64,
    pub kind: NoticeKind,
    pub text: String,
    pub source: String,
    pub at_ms: u64,
    pub read: bool,
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
        let workspace = self.locate(pane).map(|(w, _)| self.workspace_title(&self.workspaces[w], cx)).unwrap_or_default();
        let workspace_id = self.locate(pane).map(|(w, _)| self.workspaces[w].id);
        if kind == NoticeKind::Finished {
            crate::metrics::track(cx, "agent_turn_finished", serde_json::json!({ "tool": view.tool_id() }));
        }
        let pane_id = view.pane_id;
        self.notices.insert(0, Notice { pane_id, kind, text: text.clone(), source: source.clone(), at_ms: now_ms(), read: false });
        self.notices.truncate(MAX_NOTICES);

        let prefs = settings(cx);
        let background = !self.window_active || self.is_mini();
        // The user is looking at this very pane: no need to tell them elsewhere.
        let in_view = self.pane_in_view(pane_id, cx);
        let asks = matches!(kind, NoticeKind::Permission | NoticeKind::Question);
        let answer_request = asks && prefs.notify_answer_requests && !in_view;
        if prefs.system_notifications && kind != NoticeKind::Bell && (background || prefs.notify_when_focused || answer_request) {
            crate::notifications::show(pane_id, &format!("{source} · {workspace}"), &text);
        }
        self.send_chat_notice(pane_id, kind, &source, &workspace, &text, cx);
        if kind != NoticeKind::Bell {
            if let Some(mini) = self.mini.as_ref().map(|m| m.view.clone()) {
                let title = view_title_for_bubble(&source, &workspace);
                mini.update(cx, |m, cx| m.push_bubble(pane_id, title, text.clone(), kind, cx));
            }
        }
        // The workspace with the latest result moves to the top of the list.
        if let (NoticeKind::Finished, Some(id)) = (kind, workspace_id) {
            self.move_workspace_to_top(id, cx);
        }
        cx.notify();
    }

    /// Whether the user is looking at this very pane: the window is in front, no page covers the
    /// terminals, and the pane is one of the tab's split panes — not only its focused one, since
    /// every pane of the tab on screen is equally in view.
    pub(super) fn pane_in_view(&self, pane_id: u64, cx: &gpui::App) -> bool {
        if !self.window_active || self.is_mini() || self.page.is_some() {
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
        // Gold when something is waiting: the blue sat too close to the chrome to notice, and a
        // tinted plate behind it does what a heavier glyph would (Lucide strokes are fixed).
        let color = if unread > 0 { Chrome::FAVORITE } else { Chrome::MUTED };
        div()
            .id("status-notices")
            // The same box as the other header icons, so the row reads as one set.
            .size(px(crate::ui::ICON_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .gap_0p5()
            .rounded_md()
            .flex_shrink_0()
            .cursor_pointer()
            .text_color(hex(color))
            .when(unread > 0, |d| d.bg(hex_alpha(Chrome::FAVORITE, 0.18)))
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
            .child(crate::ui::icon(if unread > 0 { "bell-dot" } else { "bell" }, crate::ui::IconSize::BUTTON, hex(color)))
            .when(unread > 0, |d| d.child(div().t_caption().font_weight(crate::theme::EMPHASIS).child(unread.to_string())))
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
            let tool = tool_of(pane_id, cx);
            let color = match notice.kind {
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

fn view_title_for_bubble(source: &str, workspace: &str) -> String {
    if workspace.is_empty() {
        source.to_string()
    } else {
        format!("{source} · {workspace}")
    }
}
