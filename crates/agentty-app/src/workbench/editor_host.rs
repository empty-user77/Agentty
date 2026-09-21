//! The file editor inside the workbench: files open from the files panel into it, it takes the
//! place of the terminals while shown, and each open file has a tab after the terminal tabs.
//! Terminals keep running behind it; picking a terminal tab brings them back, and the files stay
//! open (unsaved changes included) until their tabs are closed.

use super::Workbench;
use crate::editor::{AfterDiscard, CodeEditor, EditorEvent};
use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, MouseButton, SharedString, Stateful, Window};
use std::path::{Path, PathBuf};

impl Workbench {
    /// Opens `path` (in the folder `project`) in the editor and shows it.
    pub(super) fn open_in_editor(&mut self, path: &Path, project: &Path, window: &mut Window, cx: &mut Context<Self>) {
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "file_editor" }));
        let editor = self.editor_entity(window, cx);
        editor.update(cx, |editor, cx| editor.open(path, project, window, cx));
        self.file_diff = None;
        self.page = None;
        self.session_viewer = None;
        self.launcher_open = false;
        self.editor_shown = true;
        if let Some(panel) = self.files_panel.as_mut() {
            panel.select(path);
        }
        cx.notify();
    }

    /// Opens the file a terminal ⌘-click asked for (the event carrying it has no window).
    pub(super) fn open_pending_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.pending_editor_open.take() else { return };
        if !path.is_file() {
            return;
        }
        let project = self
            .files_panel
            .as_ref()
            .map(|p| p.root().to_path_buf())
            .filter(|root| path.starts_with(root))
            .or_else(|| agentty_bridge::git::repo_root(path.parent().unwrap_or(&path)))
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        self.open_in_editor(&path, &project, window, cx);
    }

    fn editor_entity(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<CodeEditor> {
        if let Some(editor) = &self.editor {
            return editor.clone();
        }
        let editor = cx.new(CodeEditor::new);
        let subscription = cx.subscribe_in(&editor, window, |this, _, event: &EditorEvent, window, cx| match event {
            EditorEvent::TabsChanged => cx.notify(),
            EditorEvent::Empty => {
                this.editor_shown = false;
                this.focus_active(window, cx);
                cx.notify();
            }
            EditorEvent::Proceed(AfterDiscard::Quit) => {
                this.discard_confirmed = true;
                // Other windows may still have unsaved files of their own to ask about.
                cx.defer(crate::request_quit);
            }
            // Answers given in other windows for this quit no longer count.
            EditorEvent::StayOpen => cx.defer(crate::forget_discard_answers),
            EditorEvent::Proceed(AfterDiscard::CloseWindow) => {
                this.discard_confirmed = true;
                this.remember_closed_window(cx);
                cx.defer(crate::set_app_menus);
                window.remove_window();
            }
        });
        self.editor = Some(editor.clone());
        self.editor_subscription = Some(subscription);
        editor
    }

    /// Whether the editor is what the main area shows right now. A diff counts: it takes the same
    /// place, and it is what picking a changed file opens.
    pub(super) fn editor_visible(&self, cx: &gpui::App) -> bool {
        self.editor_shown
            && self.page.is_none()
            && self.session_viewer.is_none()
            && (self.file_diff.is_some() || self.editor.as_ref().is_some_and(|e| !e.read(cx).is_empty()))
    }

    /// Back to the terminals (the files stay open in their tabs).
    pub(super) fn hide_editor(&mut self) {
        self.editor_shown = false;
    }

    pub(super) fn render_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.editor_visible(cx) {
            return None;
        }
        if let Some(diff) = self.render_file_diff(cx) {
            return Some(diff);
        }
        self.editor.clone().map(|editor| editor.into_any_element())
    }

    fn show_file_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else { return };
        editor.update(cx, |editor, cx| editor.activate(index, window, cx));
        if let (Some(path), Some(panel)) = (editor.read(cx).active_path().map(Path::to_path_buf), self.files_panel.as_mut()) {
            panel.select(&path);
        }
        self.page = None;
        self.session_viewer = None;
        self.editor_shown = true;
        cx.notify();
    }

    fn close_file_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else { return };
        // A dialog about unsaved changes shows in the editor, so it comes to the front first.
        if editor.read(cx).tabs().get(index).is_some_and(|tab| tab.dirty) {
            self.show_file_tab(index, window, cx);
        }
        editor.update(cx, |editor, cx| editor.request_close(index, cx));
    }

    /// ⌘W / ⇧⌘W while the editor is shown close its file, not the terminal behind it.
    pub(super) fn close_active_file(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.editor_visible(cx) {
            return false;
        }
        if let Some(editor) = self.editor.clone() {
            editor.update(cx, |editor, cx| {
                let active = editor.tabs().iter().position(|t| t.active).unwrap_or(0);
                editor.request_close(active, cx);
            });
        }
        true
    }

    /// Shows the editor with a note when it has unsaved files (before an update relaunches the
    /// app); false when there are none.
    pub(super) fn show_unsaved_files(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.editor.as_ref().is_some_and(|e| e.read(cx).dirty_count() > 0) {
            return false;
        }
        self.page = None;
        self.session_viewer = None;
        self.editor_shown = true;
        self.updates.popup = false;
        self.show_toast(t(cx, "editor.update_unsaved"), cx);
        cx.notify();
        true
    }

    pub fn forget_discard_answer(&mut self) {
        self.discard_confirmed = false;
    }

    /// Before quitting or closing the window: asks about unsaved files in the editor. True when
    /// the question is on screen (the caller waits for the answer).
    pub fn ask_about_unsaved_files(&mut self, then: AfterDiscard, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.discard_confirmed {
            return false;
        }
        let Some(editor) = self.editor.clone() else { return false };
        let asking = editor.update(cx, |editor, cx| editor.ask_discard(then, cx));
        if asking {
            self.page = None;
            self.session_viewer = None;
            self.editor_shown = true;
            editor.read(cx).focus(window);
            cx.notify();
        }
        asking
    }

    /// Tabs of the open files, after the terminal tabs.
    pub(super) fn render_file_tabs(&self, cx: &mut Context<Self>) -> Vec<Stateful<gpui::Div>> {
        let Some(editor) = self.editor.as_ref() else { return Vec::new() };
        let shown = self.editor_visible(cx);
        let tabs = editor.read(cx).tabs();
        tabs.into_iter()
            .enumerate()
            .map(|(index, tab)| {
                let active = shown && tab.active;
                let tooltip = crate::ui::tilde(&tab.path);
                div()
                    .id(("file-tab", index))
                    .group("file-tab")
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .pl_3()
                    .pr_1()
                    .max_w(px(220.))
                    .flex_shrink_0()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(hex(Chrome::BORDER))
                    .bg(if active { hex(Chrome::EDITOR) } else { hex(Chrome::TAB_INACTIVE) })
                    .when(active, |d| d.border_t_1().border_color(hex(Chrome::ACCENT)))
                    .t_body()
                    .text_color(if active { hex(Chrome::BRIGHT) } else { hex(Chrome::MUTED) })
                    .tooltip(crate::ui::Tooltip::text(tooltip, None))
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_file_tab(index, window, cx)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.show_file_tab(index, window, cx)))
                    .child(icon("file-text", IconSize::INLINE, hex(if active { Chrome::BLUE } else { Chrome::MUTED })))
                    .child(div().truncate().child(SharedString::from(tab.name)))
                    .child(
                        div()
                            .id(("file-tab-close", index))
                            .size(px(20.))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .hover(|s| s.bg(hex_alpha(0xffffff, 0.1)))
                            .tooltip(crate::ui::Tooltip::text(
                                crate::editor::with_shortcut(t(cx, "editor.close_file").to_string(), "⌘W"),
                                None,
                            ))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.close_file_tab(index, window, cx);
                            }))
                            // Unsaved: a dot that turns into the close button under the pointer.
                            .when(tab.dirty, |d| {
                                d.child(
                                    div()
                                        .size(px(8.))
                                        .rounded_full()
                                        .bg(hex(Chrome::FOREGROUND))
                                        .group_hover("file-tab", |s| s.invisible()),
                                )
                                .child(div().absolute().invisible().group_hover("file-tab", |s| s.visible()).child(icon(
                                    "x",
                                    IconSize::INLINE,
                                    hex(Chrome::FOREGROUND),
                                )))
                                .relative()
                            })
                            .when(!tab.dirty, |d| {
                                d.child(icon("x", IconSize::INLINE, hex(Chrome::FOREGROUND)))
                                    .when(!active, |d| d.invisible().group_hover("file-tab", |s| s.visible()))
                            }),
                    )
            })
            .collect()
    }

    /// For the debug driver: `edit <path>` opens a file, `editor <command> <argument>` drives it.
    pub(super) fn debug_editor(&mut self, command: &str, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        if command == "edit" {
            let path = PathBuf::from(argument);
            let project = self
                .files_panel
                .as_ref()
                .map(|p| p.root().to_path_buf())
                .filter(|root| path.starts_with(root))
                .or_else(|| path.parent().map(Path::to_path_buf))
                .unwrap_or_default();
            return self.open_in_editor(&path, &project, window, cx);
        }
        if let Some(editor) = self.editor.clone() {
            editor.update(cx, |editor, cx| editor.debug(command, argument, window, cx));
        }
    }

    pub(super) fn editor_debug_state(&self, cx: &gpui::App) -> serde_json::Value {
        match &self.editor {
            Some(editor) => {
                let mut state = editor.read(cx).debug_state();
                state["shown"] = serde_json::json!(self.editor_visible(cx));
                state
            }
            None => serde_json::Value::Null,
        }
    }
}
