//! "Close this?" confirmation for panes, tabs and workspaces that were used. Untouched ones
//! (a new terminal or agent nobody typed into) close right away.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::settings::{settings, update_settings};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use gpui::{div, prelude::*, px, ClickEvent, Context, FontWeight, Window};

#[derive(Clone)]
pub enum CloseTarget {
    Pane(Pane),
    /// Tabs of the active workspace, by their panes (indices shift while closing).
    Tabs(Vec<Pane>),
    Workspace(u64),
}

pub struct CloseConfirm {
    pub target: CloseTarget,
    pub dont_ask: bool,
}

impl Workbench {
    fn target_panes(&self, target: &CloseTarget) -> Vec<Pane> {
        match target {
            CloseTarget::Pane(pane) => vec![pane.clone()],
            CloseTarget::Tabs(panes) => panes.clone(),
            CloseTarget::Workspace(id) => {
                self.workspaces.iter().filter(|w| w.id == *id).flat_map(|w| w.tabs.iter().flat_map(|t| t.root.leaves())).collect()
            }
        }
    }

    /// Closes right away when nothing would be lost or the user opted out; asks otherwise.
    pub(super) fn request_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        let used = self.target_panes(&target).iter().any(|p| p.read(cx).has_activity());
        if !used || !settings(cx).confirm_close {
            return self.perform_close(target, window, cx);
        }
        self.close_confirm = Some(CloseConfirm { target, dont_ask: false });
        cx.notify();
    }

    pub(super) fn request_close_pane(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(CloseTarget::Pane(pane.clone()), window, cx);
    }

    pub(super) fn request_close_tabs(&mut self, indices: &[usize], window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get(self.active_workspace) else { return };
        let panes = indices.iter().filter_map(|i| ws.tabs.get(*i)).flat_map(|t| t.root.leaves()).collect();
        self.request_close(CloseTarget::Tabs(panes), window, cx);
    }

    fn perform_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            CloseTarget::Workspace(id) => self.close_workspace(id, window, cx),
            other => {
                for pane in self.target_panes(&other) {
                    self.remove_pane(&pane, cx);
                }
                self.focus_active(window, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn render_close_confirm(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let confirm = self.close_confirm.as_ref()?;
        let (title, body) = match &confirm.target {
            CloseTarget::Pane(_) => (t(cx, "confirm.close_pane"), t(cx, "confirm.close_pane_body").to_string()),
            CloseTarget::Tabs(panes) => (t(cx, "confirm.close_tab"), tf(cx, "confirm.close_tab_body", &[("n", &panes.len().to_string())])),
            CloseTarget::Workspace(id) => (
                t(cx, "confirm.close_workspace"),
                tf(
                    cx,
                    "confirm.close_workspace_body",
                    &[("name", &self.workspaces.iter().find(|w| w.id == *id).map(|w| self.workspace_title(w, cx)).unwrap_or_default())],
                ),
            ),
        };
        let dont_ask = confirm.dont_ask;
        let button = |id: &'static str, label: &str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ERROR) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(label.to_string())
        };
        Some(
            div()
                .id("close-confirm-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .child(
                    div()
                        .w(px(360.))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .child(div().t_title().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(title))
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(body))
                        .child(
                            div()
                                .id("close-confirm-dont-ask")
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .t_small()
                                .text_color(hex(Chrome::FOREGROUND))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Some(confirm) = this.close_confirm.as_mut() {
                                        confirm.dont_ask = !confirm.dont_ask;
                                    }
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .size(px(14.))
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(hex(if dont_ask { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                                        .bg(if dont_ask { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(dont_ask, |d| d.child(crate::ui::icon("check", 11., hex(Chrome::BRIGHT)))),
                                )
                                .child(t(cx, "confirm.dont_ask")),
                        )
                        .child(
                            div()
                                .pt_1()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(button("close-confirm-cancel", t(cx, "confirm.cancel"), false).on_click(cx.listener(
                                    |this, _: &ClickEvent, window, cx| {
                                        this.close_confirm = None;
                                        this.focus_active(window, cx);
                                        cx.notify();
                                    },
                                )))
                                .child(button("close-confirm-ok", t(cx, "confirm.close"), true).on_click(cx.listener(
                                    |this, _: &ClickEvent, window, cx| {
                                        let Some(confirm) = this.close_confirm.take() else { return };
                                        if confirm.dont_ask {
                                            update_settings(cx, |s| s.confirm_close = false);
                                        }
                                        this.perform_close(confirm.target, window, cx);
                                    },
                                ))),
                        ),
                ),
        )
    }
}
