//! Quick fixes: what the language server offers for a problem or a selection ("Add import from
//! ./bad", "Organize imports", refactorings). **⌘.** opens the list at the cursor (VS Code's key),
//! a light bulb in the gutter says the cursor's line has some, and a problem's message lists its
//! fixes. A fix changes the open files as one step (⌘Z takes it back) and files that are not
//! open on disk.

use super::buffer::{EditKind, Pos};
use super::lsp::{self, Action};
use super::{CodeEditor, Content};
use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, TypeScale};
use gpui::{actions, div, prelude::*, px, AnyElement, ClickEvent, Context, FocusHandle, Window};
use std::path::{Path, PathBuf};

actions!(code_editor, [QuickFix, OrganizeImports]);

pub fn bind_keys(cx: &mut gpui::App) {
    use crate::key;
    let c = Some(super::CONTEXT);
    // VS Code's keys.
    cx.bind_keys([key("cmd-.", QuickFix, c), key("shift-alt-o", OrganizeImports, c)]);
}

/// Actions a list shows at most.
const MAX_ACTIONS: usize = 20;

/// The list ⌘. opens.
pub(super) struct FixMenu {
    path: PathBuf,
    at: Pos,
    /// `None` while the server is asked.
    actions: Option<Vec<Action>>,
    /// The problem at the cursor, which the last row hands to the linked agent.
    problem: Option<super::lsp::Problem>,
    /// The revision of the text the actions were offered for.
    revision: u64,
    selected: usize,
    focus: FocusHandle,
}

/// Whether the cursor's line has fixes: (file, line, text revision) → found.
pub(super) type Bulb = (PathBuf, usize, u64, bool);

/// The same file, also where a server writes its path in other case (macOS, Windows).
fn same_file(a: &Path, b: &Path) -> bool {
    if cfg!(any(target_os = "macos", windows)) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Whether `path` is a file of `project`, links resolved (a link out of the project is not).
fn inside_project(project: &Path, path: &Path) -> bool {
    match (std::fs::canonicalize(project), std::fs::canonicalize(path)) {
        (Ok(project), Ok(path)) => path.starts_with(project) && path.is_file(),
        _ => false,
    }
}

/// The place in `text` at byte `offset`.
fn pos_of(text: &str, offset: usize) -> Pos {
    let before = &text[..offset];
    let line = before.matches('\n').count();
    Pos::new(line, offset - before.rfind('\n').map_or(0, |i| i + 1))
}

/// The one range that turns `old` into `new`: (start, end in `old`, the text for it), on
/// character boundaries.
fn difference<'a>(old: &str, new: &'a str) -> (usize, usize, &'a str) {
    let mut prefix = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let room = old.len().min(new.len()) - prefix;
    let mut suffix = old.bytes().rev().zip(new.bytes().rev()).take(room).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    (prefix, old.len() - suffix, &new[prefix..new.len() - suffix])
}

impl CodeEditor {
    /// ⌘.: the fixes and refactorings for the selection, or for the problem at the cursor.
    pub(super) fn open_quick_fix(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && d.editable()) else { return };
        if lsp::id(doc.language).is_none() || doc.project.as_os_str().is_empty() {
            return self.flash(t(cx, "editor.no_fixes").to_string(), cx);
        }
        let cursor = doc.buffer.cursor();
        let problem = doc.checks.problems.iter().find(|p| p.start <= cursor && cursor <= p.end).cloned();
        let (start, end) = match doc.buffer.selection.range() {
            (start, end) if start != end => (start, end),
            _ => problem.as_ref().map_or((cursor, cursor), |p| (p.start, p.end)),
        };
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.problem_hover = None;
        let has_problem = problem.is_some();
        let revision = doc.buffer.revision();
        let spelling = problem.clone();
        self.fixes = Some(FixMenu { path: doc.path.clone(), at: start, actions: None, problem, revision, selected: 0, focus });
        let (project, language, path, text) = (doc.project.clone(), doc.language, doc.path.clone(), lsp::Snapshot::of(&doc.buffer));
        let asked = path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (found, spelled) = cx
                .background_spawn(async move {
                    let found = lsp::actions(&project, language, &path, &text, start, end);
                    let spelled = spelling.and_then(|p| super::spell::suggestions(&p, &text.text, &project, language));
                    (found, spelled)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                // The names the editor finds spelled close come after the server's fixes.
                let found = match (found, spelled) {
                    (found, None) => found,
                    (found, spelled) => {
                        let mut found = found.unwrap_or_default();
                        let own = super::spell::actions(&asked, spelled, &found, cx);
                        let first_other = found.iter().position(|a| !a.fixes).unwrap_or(found.len());
                        found.splice(first_other..first_other, own);
                        Some(found)
                    }
                };
                let Some(menu) = this.fixes.as_mut().filter(|m| m.path == asked && m.actions.is_none()) else { return };
                match found.filter(|f| !f.is_empty() || has_problem) {
                    Some(mut found) => {
                        found.truncate(MAX_ACTIONS);
                        menu.actions = Some(found);
                    }
                    None => {
                        this.close_quick_fix(window, cx);
                        this.flash(t(cx, "editor.no_fixes").to_string(), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn close_quick_fix(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fixes.take().is_some() {
            window.focus(&self.focus_handle);
            cx.notify();
        }
    }

    /// Runs `action` of the file in front: its changes go into the open files (one undo step
    /// each) and onto disk for the others.
    /// `since` is the revision of the text the server offered the action for.
    pub(super) fn apply_fix(&mut self, action: Action, since: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.close_quick_fix(window, cx);
        self.problem_hover = None;
        let Some(doc) = self.docs.get(self.active) else { return };
        let (project, language, path) = (doc.project.clone(), doc.language, doc.path.clone());
        let file = path.clone();
        cx.spawn(async move |this, cx| {
            let edits = cx.background_spawn(async move { lsp::apply(&project, language, &path, &action) }).await;
            let Some(edits) = edits else {
                let _ = this.update(cx, |this, cx| this.flash(t(cx, "editor.fix_failed").to_string(), cx));
                return;
            };
            if CodeEditor::apply_edits(&this, edits, (file, since), cx).await == Some(false) {
                let _ = this.update(cx, |this, cx| this.flash(t(cx, "editor.fix_failed").to_string(), cx));
            }
        })
        .detach();
    }

    /// ⇧⌥O: the language server sorts the file's imports and takes out the unused ones.
    pub(super) fn organize_imports(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && d.editable()) else { return };
        if lsp::id(doc.language).is_none() || lsp::status(&doc.project, doc.language, &doc.path) != "ready" {
            return self.flash(t(cx, "editor.organize_needs_server").to_string(), cx);
        }
        let (project, language, path, text) = (doc.project.clone(), doc.language, doc.path.clone(), lsp::Snapshot::of(&doc.buffer));
        let since = text.revision;
        cx.spawn_in(window, async move |this, cx| {
            let action = cx.background_spawn(async move { lsp::organize_imports(&project, language, &path, &text) }).await;
            let _ = this.update_in(cx, |this, window, cx| match action {
                Some(action) => this.apply_fix(action, since, window, cx),
                None => this.flash(t(cx, "editor.organize_none").to_string(), cx),
            });
        })
        .detach();
    }

    /// Makes `edits` (a fix's, a rename's): into the open files, one undo step each, and on disk
    /// for the others. `since` is the file the server was asked about and the revision of its text
    /// then: when it changed meanwhile the server's places no longer hold, nothing is changed and
    /// a note says so (`None`). `Some(false)` when some file could not be changed.
    pub(super) async fn apply_edits(
        this: &gpui::WeakEntity<Self>,
        edits: Vec<lsp::FileEdit>,
        since: (PathBuf, u64),
        cx: &mut gpui::AsyncApp,
    ) -> Option<bool> {
        let changed = this
            .update(cx, |this, cx| {
                let changed = this.docs.iter().any(|d| d.path == since.0 && d.buffer.revision() != since.1);
                if changed {
                    this.flash(t(cx, "editor.changed_meanwhile").to_string(), cx);
                }
                changed
            })
            .unwrap_or(true);
        if changed {
            return None;
        }
        let (open, project): (Vec<PathBuf>, Option<PathBuf>) = this
            .update(cx, |this, _| {
                (this.docs.iter().map(|d| d.path.clone()).collect(), this.docs.get(this.active).map(|d| d.project.clone()))
            })
            .unwrap_or_default();
        let (here, elsewhere): (Vec<_>, Vec<_>) = edits.into_iter().partition(|e| open.iter().any(|p| same_file(p, &e.path)));
        // Files that are not open: changed on disk, off the UI thread — only inside the project
        // (a server pointing anywhere else is not followed).
        let written = cx
            .background_spawn(async move {
                elsewhere.iter().all(|edit| {
                    let inside = project.as_deref().is_some_and(|project| inside_project(project, &edit.path));
                    inside
                        && std::fs::read_to_string(&edit.path)
                            .is_ok_and(|text| std::fs::write(&edit.path, lsp::edited(&text, edit)).is_ok())
                })
            })
            .await;
        this.update(cx, |this, cx| {
            let mut ok = written;
            for edit in &here {
                let Some(index) = this.docs.iter().position(|d| same_file(&d.path, &edit.path)) else { continue };
                let new = lsp::edited(&this.docs[index].buffer.text(), edit);
                ok &= this.replace_text(index, &new, cx);
            }
            cx.notify();
            ok
        })
        .ok()
    }

    /// Makes the text of open file `index` `new` as one undo step, the cursor staying on the code
    /// it was on. False when the file cannot be edited.
    pub(super) fn replace_text(&mut self, index: usize, new: &str, cx: &mut Context<Self>) -> bool {
        let Some(doc) = self.docs.get_mut(index) else { return false };
        if !doc.editable() {
            return false;
        }
        let old = doc.buffer.text();
        let (from, to, text) = difference(&old, new);
        if from == to && text.is_empty() {
            return true;
        }
        let (start, end) = (pos_of(&old, from), pos_of(&old, to));
        let was_dirty = doc.is_dirty();
        let cursor = doc.buffer.cursor();
        doc.buffer.seal();
        let after = doc.buffer.replace(start, end, text, EditKind::Other);
        doc.buffer.seal();
        let moved = if cursor <= start {
            cursor
        } else if cursor >= end && cursor.line == end.line {
            Pos::new(after.line, after.col + cursor.col - end.col)
        } else if cursor >= end {
            Pos::new((cursor.line + after.line).saturating_sub(end.line), cursor.col)
        } else {
            // Inside the changed text (a rename around it): where it was, as near as there is.
            cursor
        };
        let moved = doc.buffer.clamp(moved);
        doc.buffer.move_to(moved, false);
        doc.sync_highlights();
        if doc.is_dirty() != was_dirty {
            self.tabs_changed(cx);
        }
        true
    }

    /// Asks in the background whether the cursor's line, when it has a problem, has fixes: the
    /// light bulb shows only then.
    pub(super) fn bulb_tick(&mut self, cx: &mut Context<Self>) {
        if self.bulb_asking || self.fixes.is_some() {
            return;
        }
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && d.editable()) else { return };
        let line = doc.buffer.cursor().line;
        let revision = doc.buffer.revision();
        // Asked once typing pauses, not on every key.
        if !doc.checks.paused(revision) {
            return;
        }
        let Some(problem) = doc.checks.problems.iter().find(|p| p.start.line <= line && line <= p.end.line) else {
            return;
        };
        if self.bulb.as_ref().is_some_and(|(path, l, r, _)| *path == doc.path && *l == line && *r == revision) {
            return;
        }
        self.bulb_asking = true;
        let (start, end) = (problem.start, problem.end);
        let spelling = problem.clone();
        let (project, language, path, text) = (doc.project.clone(), doc.language, doc.path.clone(), lsp::Snapshot::of(&doc.buffer));
        cx.spawn(async move |this, cx| {
            let asked = path.clone();
            let (found, spelled) = cx
                .background_spawn(async move {
                    let found = lsp::actions(&project, language, &path, &text, start, end);
                    (found, super::spell::suggestions(&spelling, &text.text, &project, language).is_some())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.bulb_asking = false;
                let has = spelled || found.is_some_and(|f| f.iter().any(|a| a.fixes));
                this.bulb = Some((asked, line, revision, has));
                cx.notify();
            });
        })
        .detach();
    }

    /// The light bulb at the start of the cursor's line, while it has fixes.
    pub(super) fn render_bulb(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.fixes.is_some() || self.picker.is_some() || self.preview.covered {
            return None;
        }
        let doc = self.docs.get(self.active)?;
        let cursor = doc.buffer.cursor();
        let (path, line, revision, true) = self.bulb.as_ref()? else { return None };
        if *path != doc.path || *line != cursor.line || *revision != doc.buffer.revision() {
            return None;
        }
        let layout = self.layout.as_ref()?;
        let at = layout.gutter_point(*line)?;
        let size = layout.line_height;
        Some(
            gpui::deferred(
                gpui::anchored().position(at).child(
                    div()
                        .id("editor-bulb")
                        .size(px(size))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .child(icon("lightbulb", 13., hex(Chrome::WARNING)))
                        .tooltip(crate::ui::Tooltip::text(super::with_shortcut(t(cx, "editor.quick_fix").to_string(), "⌘."), None))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_quick_fix(window, cx))),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    pub(super) fn render_quick_fix(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.fixes.as_ref()?;
        self.docs.get(self.active).filter(|d| d.path == menu.path)?;
        let at = self.layout.as_ref()?.point_below(menu.at)?;
        let mut list = div()
            .id("editor-quick-fix")
            .track_focus(&menu.focus)
            .key_context("EditorQuickFix")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let Some(menu) = this.fixes.as_mut() else { return };
                let count = menu.actions.as_ref().map_or(0, Vec::len) + usize::from(menu.problem.is_some() && menu.actions.is_some());
                match event.keystroke.key.as_str() {
                    "up" if count > 0 => menu.selected = (menu.selected + count - 1) % count,
                    "down" if count > 0 => menu.selected = (menu.selected + 1) % count,
                    "enter" => {
                        let listed = menu.actions.as_ref().map_or(0, Vec::len);
                        let since = menu.revision;
                        if let Some(action) = menu.actions.as_ref().and_then(|a| a.get(menu.selected)).cloned() {
                            this.apply_fix(action, since, window, cx);
                        } else if let Some(problem) = menu.problem.clone().filter(|_| menu.selected == listed) {
                            this.close_quick_fix(window, cx);
                            this.fix_with_ai(&problem, cx);
                        }
                    }
                    "escape" => this.close_quick_fix(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .min_w(px(260.))
            .max_w(px(520.))
            .py_1()
            .rounded_md()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .occlude()
            .t_small();
        match &menu.actions {
            None => {
                list = list.child(div().px_2().py_1().text_color(hex(Chrome::MUTED)).child(t(cx, "editor.finding_fixes")));
            }
            Some(actions) => {
                let since = menu.revision;
                for (i, action) in actions.iter().enumerate() {
                    let glyph = if action.fixes { "lightbulb" } else { "wrench" };
                    let chosen = action.clone();
                    list = list.child(
                        div()
                            .id(("editor-fix", i))
                            .mx_1()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .text_color(hex(Chrome::FOREGROUND))
                            .when(i == menu.selected, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.35)).text_color(hex(Chrome::BRIGHT)))
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .child(icon(glyph, 12., hex(if action.fixes { Chrome::WARNING } else { Chrome::MUTED })))
                            .child(div().min_w_0().truncate().child(action.title.clone()))
                            .on_click(
                                cx.listener(move |this, _: &ClickEvent, window, cx| this.apply_fix(chosen.clone(), since, window, cx)),
                            ),
                    );
                }
                if let Some(problem) = menu.problem.clone() {
                    let row = actions.len();
                    list = list.child(
                        div()
                            .id("editor-fix-ai")
                            .mx_1()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .text_color(hex(Chrome::FOREGROUND))
                            .when(row == menu.selected, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.35)).text_color(hex(Chrome::BRIGHT)))
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .child(icon("sparkles", 12., hex(Chrome::ACCENT)))
                            .child(div().min_w_0().truncate().child(t(cx, "editor.fix_with_ai")))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.close_quick_fix(window, cx);
                                this.fix_with_ai(&problem, cx);
                            })),
                    );
                }
            }
        }
        Some(
            gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(self.popup_margin()).child(list))
                .with_priority(3)
                .into_any_element(),
        )
    }

    pub(super) fn debug_quick_fix(&self) -> serde_json::Value {
        serde_json::json!({
            "menu": self.fixes.as_ref().map(|m| serde_json::json!({
                "actions": m.actions.as_ref().map(|a| a.iter().map(|a| a.title.clone()).collect::<Vec<_>>()),
                "selected": m.selected,
            })),
            "bulb": self.bulb.as_ref().map(|(_, line, _, has)| serde_json::json!({ "line": line, "has": has })),
        })
    }

    /// Debug driver: applies the menu's row `index`.
    pub(super) fn debug_pick_fix(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((action, since)) = self.fixes.as_ref().and_then(|m| Some((m.actions.as_ref()?.get(index).cloned()?, m.revision))) {
            self.apply_fix(action, since, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_range_turns_the_old_text_into_the_new() {
        assert_eq!(
            difference("const a = total();\n", "import { total } from \"./bad\";\n\nconst a = total();\n"),
            (0, 0, "import { total } from \"./bad\";\n\n")
        );
        assert_eq!(difference("abc", "abc"), (3, 3, ""));
        assert_eq!(difference("a totl b", "a total b"), (5, 5, "a"));
        // Never inside a character.
        let (from, to, text) = difference("é1", "è1");
        assert_eq!((from, to, text), (0, 2, "è"));
        assert_eq!(pos_of("ab\ncd", 4), Pos::new(1, 1));
    }

    #[test]
    fn edits_on_disk_stay_in_the_project() {
        let root = std::env::temp_dir().join(format!("agentty-fix-{}", std::process::id()));
        let project = root.join("p");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/a.ts"), "").unwrap();
        std::fs::write(root.join("outside.ts"), "").unwrap();
        assert!(inside_project(&project, &project.join("src/a.ts")));
        assert!(!inside_project(&project, &root.join("outside.ts")));
        assert!(!inside_project(&project, &project.join("src/../../outside.ts")));
        assert!(!inside_project(&project, &project.join("src/missing.ts")));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("outside.ts"), project.join("link.ts")).unwrap();
            assert!(!inside_project(&project, &project.join("link.ts")));
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
