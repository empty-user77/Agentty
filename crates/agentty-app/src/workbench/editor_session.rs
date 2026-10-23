//! The file editor and a terminal session side by side: the session the editor is linked to is
//! shown at its right (resizable by its edge), the editor's *Ask AI* sends instructions to it, and
//! its edits show in the editor as they happen (see `editor/agent.rs`).
//!
//! Which session: the one picked in the column's list, else the active pane when an agent runs
//! there, else an agent working in the folder of the file in front.

use super::{Pane, Workbench};
use crate::agent_signal::AgentSignal;
use crate::editor::agent::{short_id, LinkedAgent};
use crate::i18n::t;
use crate::terminal::AgentStatus;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, MouseButton, SharedString, Window};
use std::path::Path;

pub const DEFAULT_WIDTH: f32 = 440.;
/// Narrowest the session column gets, and what the editor keeps at least.
const MIN_WIDTH: f32 = 300.;
const MIN_EDITOR: f32 = 520.;

/// What the editor shows for a session: `Claude-3f9a`, its brand and color, and whether it works.
pub fn describe(pane: &Pane, cx: &gpui::App) -> LinkedAgent {
    let view = pane.read(cx);
    let brand = crate::brand::brand(view.tool_id());
    let short_name = brand.name.split_whitespace().next().unwrap_or(brand.name);
    let label = format!("{short_name}-{}", short_id(view.current_session().as_deref(), view.pane_id));
    let working = matches!(view.status, AgentStatus::Working | AgentStatus::Thinking);
    let waiting = matches!(view.status, AgentStatus::Permission(_) | AgentStatus::Question(_));
    // A grey brand (Codex, Cursor) would not stand out on the code.
    let grey = brand.color & 0xf0f0f0 == 0xe0e0e0 || brand.color == 0xd4d4d4;
    let color = match brand.id {
        "codex" => Chrome::CODEX,
        _ if grey => Chrome::PURPLE,
        _ => brand.color,
    };
    LinkedAgent { pane_id: view.pane_id, label, brand: brand.id, color, working, waiting }
}

fn is_session(pane: &Pane, cx: &gpui::App) -> bool {
    pane.read(cx).tool_id() != "shell"
}

impl Workbench {
    /// Agent sessions to link the editor to: this workspace's first, then the others'.
    fn agent_sessions(&self, cx: &gpui::App) -> Vec<Pane> {
        let mut here: Vec<Pane> = Vec::new();
        let mut others = Vec::new();
        for (index, ws) in self.workspaces.iter().enumerate() {
            for pane in ws.tabs.iter().flat_map(|tab| tab.root.leaves()) {
                if is_session(&pane, cx) {
                    if index == self.active_workspace {
                        here.push(pane);
                    } else {
                        others.push(pane);
                    }
                }
            }
        }
        here.extend(others);
        here
    }

    /// The session the editor talks to.
    pub(super) fn linked_session(&self, cx: &gpui::App) -> Option<Pane> {
        let sessions = self.agent_sessions(cx);
        if let Some(id) = self.editor_link {
            if let Some(pane) = sessions.iter().find(|p| p.read(cx).pane_id == id) {
                return Some(pane.clone());
            }
        }
        if let Some(active) = self.active_pane().filter(|p| is_session(p, cx)) {
            return Some(active);
        }
        let project = self.editor.as_ref().and_then(|e| e.read(cx).active_project().map(Path::to_path_buf));
        sessions
            .iter()
            .find(|p| {
                let view = p.read(cx);
                let cwd = view.live_cwd.clone().unwrap_or_else(|| view.spec.cwd.clone());
                project.as_ref().is_some_and(|project| cwd.starts_with(project) || project.starts_with(&cwd))
            })
            .cloned()
    }

    /// Tells the editor which session it is linked to (each frame the editor is shown; only a
    /// change repaints it).
    pub(super) fn sync_editor_link(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else { return };
        let linked = self.linked_session(cx).map(|pane| describe(&pane, cx));
        editor.update(cx, |editor, cx| editor.set_linked(linked, cx));
    }

    /// The editor's *Ask AI*: the instruction goes to the linked session as a prompt, and the
    /// session comes into view beside the editor.
    pub(super) fn ask_linked_session(&mut self, prompt: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.linked_session(cx) else {
            self.editor_session_open = true;
            self.editor_session_picking = true;
            self.show_toast(t(cx, "editor.ask_pick_session"), cx);
            return cx.notify();
        };
        pane.update(cx, |view, cx| view.submit_prompt(prompt.to_string(), cx));
        self.editor_session_open = true;
        // The keyboard stays in the editor: the user reads the code while the agent works.
        if let Some(editor) = self.editor.clone() {
            editor.read(cx).focus(window);
        }
        cx.notify();
    }

    pub(super) fn toggle_editor_session(&mut self, cx: &mut Context<Self>) {
        let waiting = self.linked_session(cx).is_some_and(|pane| describe(&pane, cx).waiting);
        if waiting {
            // It asks something: the click is to see it, never to hide it.
            self.editor_session_open = true;
            self.editor_session_picking = false;
        } else if self.linked_session(cx).is_none() {
            self.editor_session_open = true;
            self.editor_session_picking = true;
        } else {
            self.editor_session_open = !self.editor_session_open;
            self.editor_session_picking = false;
        }
        cx.notify();
    }

    /// An agent's tool reports a file it changes: the editor marks the spot (before) and reads
    /// the file again (after).
    pub(super) fn editor_signal(&mut self, signal: &AgentSignal, cx: &mut Context<Self>) {
        let (Some(edit), Some(editor)) = (signal.detail.edit.as_ref(), self.editor.clone()) else { return };
        let Some(pane) = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == signal.pane_id) else { return };
        let agent = describe(&pane, cx);
        match signal.detail.event.as_deref() {
            Some("PreToolUse") => editor.update(cx, |editor, cx| editor.agent_will_edit(&agent, &edit.path, edit.anchor.as_deref(), cx)),
            Some("PostToolUse") => editor.update(cx, |editor, cx| editor.agent_did_edit(&agent, &edit.path, cx)),
            _ => {}
        }
    }

    /// While the column's edge is dragged: its width from the pointer (the column's right edge is
    /// where the editor's area ends).
    pub(super) fn drag_editor_session(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some((start_x, start_width)) = self.editor_session_drag else { return };
        let most = (self.editor_area_width - MIN_EDITOR).max(MIN_WIDTH);
        let width = (start_width + start_x - pointer_x).clamp(MIN_WIDTH, most);
        gpui::BorrowAppContext::update_global::<crate::settings::SettingsStore, _>(cx, |store, _| {
            store.settings.editor_session_width = width
        });
        cx.notify();
    }

    /// The editor, with the linked session at its right when that is open.
    pub(super) fn render_editor_with_session(&mut self, editor: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        if !self.editor_session_open {
            return editor;
        }
        let pane = self.linked_session(cx).filter(|_| !self.editor_session_picking);
        let total = self.editor_area_width;
        let width = crate::settings::settings(cx).editor_session_width.clamp(MIN_WIDTH, (total - MIN_EDITOR).max(MIN_WIDTH));
        let dragging = self.editor_session_drag.is_some();
        let splitter = div()
            .id("editor-session-resize")
            .group("editor-session-resize")
            .w(px(5.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .justify_center()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .bg(hex(Chrome::BORDER))
                    .group_hover("editor-session-resize", |s| s.w(px(5.)).bg(hex(Chrome::ACCENT)))
                    .when(dragging, |d| d.w(px(5.)).bg(hex(Chrome::ACCENT))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.editor_session_drag = Some((f32::from(event.position.x), width));
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        let header = self.render_session_header(pane.as_ref(), cx);
        let body = match pane {
            Some(pane) => self.render_pane(&pane, false, true, cx),
            None => self.render_session_list(cx),
        };
        let this = cx.entity().downgrade();
        div()
            .id("editor-with-session")
            .size_full()
            .relative()
            .flex()
            // The whole area's width, for the drag's limits.
            .child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, _, cx| {
                        let _ = this.update(cx, |this, _| this.editor_area_width = f32::from(bounds.size.width));
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(div().flex_1().min_w_0().h_full().child(editor))
            .child(splitter)
            .child(
                div()
                    .w(px(width))
                    .flex_shrink_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(hex(Chrome::PANEL))
                    .child(header)
                    .child(div().flex_1().min_h_0().child(body)),
            )
            .into_any_element()
    }

    fn render_session_header(&self, pane: Option<&Pane>, cx: &mut Context<Self>) -> impl IntoElement {
        let row = div()
            .h(px(34.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::EDITOR))
            .t_small();
        let row = match pane {
            Some(pane) => {
                let agent = describe(pane, cx);
                let status = if agent.working { t(cx, "editor.session_working") } else { t(cx, "editor.session_idle") };
                row.child(crate::brand::avatar_working(agent.brand, 16., agent.working, agent.pane_id, agent.color))
                    .child(div().text_color(hex(Chrome::BRIGHT)).font_weight(gpui::FontWeight::MEDIUM).child(agent.label.clone()))
                    .child(div().t_caption().text_color(hex(if agent.working { agent.color } else { Chrome::MUTED })).child(status))
                    .child(div().flex_1())
                    .child(
                        crate::ui::icon_only(
                            "editor-session-switch",
                            "users",
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.editor_session_picking = true;
                                cx.notify();
                            }),
                        )
                        .tooltip(crate::ui::Tooltip::text(t(cx, "editor.session_switch"), None)),
                    )
            }
            None => row
                .child(icon("link", IconSize::INLINE, hex(Chrome::MUTED)))
                .child(div().flex_1().text_color(hex(Chrome::BRIGHT)).child(t(cx, "editor.session_pick"))),
        };
        row.child(
            crate::ui::icon_only(
                "editor-session-close",
                "x",
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.editor_session_open = false;
                    this.editor_session_picking = false;
                    cx.notify();
                }),
            )
            .tooltip(crate::ui::Tooltip::text(t(cx, "confirm.close"), None)),
        )
    }

    /// The sessions to pick from (and a way to start one in the file's project).
    fn render_session_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let sessions = self.agent_sessions(cx);
        let current = self.linked_session(cx).map(|p| p.read(cx).pane_id);
        let mut list = div().id("editor-session-list").flex().flex_col().gap_0p5().p_2().overflow_y_scroll().size_full();
        if sessions.is_empty() {
            list = list.child(div().p_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "editor.session_none")));
        }
        for (index, pane) in sessions.into_iter().enumerate() {
            let agent = describe(&pane, cx);
            let view = pane.read(cx);
            let title = view.title.clone();
            let cwd = view.live_cwd.clone().unwrap_or_else(|| view.spec.cwd.clone());
            let id = agent.pane_id;
            let selected = current == Some(id);
            list = list.child(
                div()
                    .id(("editor-session-row", index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.25)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.editor_link = Some(id);
                        this.editor_session_picking = false;
                        this.sync_editor_link(cx);
                        cx.notify();
                    }))
                    .child(crate::brand::avatar_working(agent.brand, 18., agent.working, id, agent.color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .t_small()
                                    .text_color(hex(Chrome::BRIGHT))
                                    .truncate()
                                    .child(SharedString::from(format!("{} · {title}", agent.label))),
                            )
                            .child(div().t_caption().text_color(hex(Chrome::MUTED)).truncate().child(crate::ui::tilde(&cwd))),
                    ),
            );
        }
        list.into_any_element()
    }
}
