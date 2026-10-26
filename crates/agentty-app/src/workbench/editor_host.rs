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
        self.open_in_editor_at(path, project, None, window, cx);
    }

    /// A file the system asked Agentty to open (Finder, Agentty as its default app): shown in the
    /// editor, in the files panel's folder when it is in it, else in its own folder.
    pub fn open_file_from_system(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let project = self.project_of_file(path);
        self.open_in_editor(path, &project, window, cx);
        window.activate_window();
    }

    /// The folder a file opened from outside belongs to: the files panel's, when it holds it.
    fn project_of_file(&self, path: &Path) -> std::path::PathBuf {
        self.files_panel
            .as_ref()
            .map(|p| p.root().to_path_buf())
            .filter(|root| path.starts_with(root))
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_default()
    }

    /// [`Self::open_in_editor`] with the cursor at `line:column` (from 1).
    fn open_in_editor_at(
        &mut self,
        path: &Path,
        project: &Path,
        place: Option<(usize, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "file_editor" }));
        let editor = self.editor_entity(window, cx);
        editor.update(cx, |editor, cx| editor.open_at(path, project, place, window, cx));
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

    /// ⌘P anywhere: the go-to-file box over the editor, for the project of the file in front or of
    /// the active terminal (its repository, else its folder).
    pub(super) fn go_to_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_visible(cx) && self.file_diff.is_none() {
            if let Some(editor) = self.editor.clone() {
                if !editor.read(cx).is_empty() {
                    return editor.update(cx, |editor, cx| editor.open_picker("", None, window, cx));
                }
            }
        }
        let project = self.files_panel.as_ref().map(|p| p.root().to_path_buf()).or_else(|| {
            let pane = self.active_pane()?;
            let view = pane.read(cx);
            let cwd = view.live_cwd.clone().unwrap_or_else(|| view.spec.cwd.clone());
            Some(agentty_bridge::git::repo_root(&cwd).unwrap_or(cwd))
        });
        let Some(project) = project.filter(|p| p.is_dir() && Some(p.as_path()) != dirs::home_dir().as_deref()) else {
            return self.show_toast(t(cx, "nav.no_project"), cx);
        };
        let editor = self.editor_entity(window, cx);
        editor.update(cx, |editor, cx| editor.open_picker("", Some(project), window, cx));
        self.file_diff = None;
        self.page = None;
        self.session_viewer = None;
        self.launcher_open = false;
        self.editor_shown = true;
        cx.notify();
    }

    /// Opens the file a terminal ⌘-click asked for (the event carrying it has no window).
    pub(super) fn open_pending_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, place)) = self.pending_editor_open.take() else { return };
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
        self.open_in_editor_at(&path, &project, place, window, cx);
    }

    fn editor_entity(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<CodeEditor> {
        if let Some(editor) = &self.editor {
            return editor.clone();
        }
        let editor = cx.new(CodeEditor::new);
        let subscription = cx.subscribe_in(&editor, window, |this, _, event: &EditorEvent, window, cx| match event {
            EditorEvent::TabsChanged => cx.notify(),
            EditorEvent::OpenUrl(url) => this.open_link(url.clone(), cx),
            EditorEvent::OpenFile(path, project) => this.open_in_editor(path, project, window, cx),
            EditorEvent::ShowInBrowser(path) => this.show_file_in_browser(path, cx),
            EditorEvent::AskAgent(prompt) => this.ask_linked_session(prompt, window, cx),
            EditorEvent::ToggleSession => this.toggle_editor_session(cx),
            EditorEvent::InstallLanguageServer(language) => this.install_language_server(language, window, cx),
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
            && (self.file_diff.is_some() || self.editor.as_ref().is_some_and(|e| !e.read(cx).is_empty() || e.read(cx).picker_open()))
    }

    /// The markdown preview is a native view over the editor: out of sight whenever the editor is
    /// (another tab, a page, a diff) or something of the workbench is drawn over it.
    pub(super) fn place_markdown_preview(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else { return };
        let covered = !self.editor_visible(cx) || self.file_diff.is_some() || self.overlay_open();
        editor.update(cx, |editor, _| editor.set_preview_covered(covered));
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

    /// Right click on a file's tab: close it or the others, its path, its folder, another editor.
    pub(super) fn render_file_tab_menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        use super::workspace_menu::{row, rule};
        let (index, position) = self.file_tab_menu?;
        let editor = self.editor.clone()?;
        let path = editor.read(cx).tabs().get(index)?.path.clone();
        let count = editor.read(cx).tabs().len();
        let done = |this: &mut Self, cx: &mut Context<Self>| {
            this.file_tab_menu = None;
            cx.notify();
        };
        let (copy, reveal) = (path.clone(), path.clone());
        let list = crate::ui::popover()
            .w(px(240.))
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| done(this, cx)))
            .child(row("file-tab-menu-close", "x", t(cx, "editor.close_file"), false).on_click(cx.listener(
                move |this, _: &ClickEvent, window, cx| {
                    done(this, cx);
                    this.close_file_tab(index, window, cx);
                },
            )))
            .when(count > 1, |d| {
                let editor = editor.clone();
                d.child(row("file-tab-menu-others", "minus", t(cx, "editor.close_other_files"), false).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        done(this, cx);
                        editor.update(cx, |editor, cx| editor.close_others(index, cx));
                    },
                )))
            })
            .child(rule())
            .child(row("file-tab-menu-copy", "copy", t(cx, "files.menu.copy_path"), false).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    done(this, cx);
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.to_string_lossy().into_owned()));
                },
            )))
            .child(row("file-tab-menu-reveal", "folder", t(cx, "files.reveal"), false).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    done(this, cx);
                    crate::platform::reveal(&reveal);
                },
            )))
            .child(row("file-tab-menu-external", "code", t(cx, "editor.open_external"), false).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    done(this, cx);
                    editor.update(cx, |editor, cx| editor.open_tab_externally(index, cx));
                },
            )));
        let list = crate::ui::fit_to_window(list, "file-tab-menu", window);
        Some(gpui::deferred(gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(list)).with_priority(1))
    }

    /// Tabs of the open files, after the terminal tabs.
    pub(super) fn render_file_tabs(&self, run: &mut super::chrome::TabRun, cx: &mut Context<Self>) -> Vec<Stateful<gpui::Div>> {
        let Some(editor) = self.editor.as_ref() else { return Vec::new() };
        let shown = self.editor_visible(cx);
        let tabs = editor.read(cx).tabs();
        tabs.into_iter()
            .enumerate()
            .map(|(index, tab)| {
                let active = shown && tab.active;
                let tooltip = crate::ui::tilde(&tab.path);
                self.tab_card(div().id(("file-tab", index)), SharedString::from(format!("file-tab-{index}")), active, run, cx)
                    .group("file-tab")
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .pl_3()
                    .pr_1()
                    .max_w(px(220.))
                    .cursor_pointer()
                    .t_body()
                    // A file with errors (warnings) has its name in their color, as in VS Code.
                    .text_color(match tab.problem {
                        Some(color) => hex(color),
                        None if active => hex(Chrome::BRIGHT),
                        None => hex(Chrome::MUTED),
                    })
                    // Not over the tab's own menu.
                    .when(self.file_tab_menu.is_none(), |d| d.tooltip(crate::ui::Tooltip::text(tooltip, None)))
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_file_tab(index, window, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.file_tab_menu = Some((index, event.position));
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.show_file_tab(index, window, cx)))
                    .child(icon("file-text", IconSize::INLINE, hex(if active { Chrome::BLUE } else { Chrome::MUTED })))
                    // An agent's changes here wait for review.
                    .children(tab.review.map(|color| div().size(px(6.)).rounded_full().flex_shrink_0().bg(hex(color))))
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
                            .hover(|s| s.bg(hex_alpha(Chrome::BRIGHT, 0.1)))
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
            let project = self.project_of_file(&path);
            return self.open_in_editor(&path, &project, window, cx);
        }
        // As a terminal ⌘-click on `path:line:column` would (the text must name an existing file).
        if command == "open-at" {
            let text: Vec<char> = argument.chars().collect();
            if let Some((path, place)) = crate::terminal::path_and_place_at(&text, 0, Path::new("/")) {
                self.pending_editor_open = Some((path, place));
                return self.open_pending_file(window, cx);
            }
            return;
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
