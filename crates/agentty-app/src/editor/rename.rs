//! Renaming a name everywhere it is used (**F2**, VS Code's key): a small box over the name takes
//! the new one, and the language server says every place to change, in every file. Without a
//! server the name is renamed in this file only, where it is written as a whole name, and the
//! note says so.

use super::buffer::Pos;
use super::lsp;
use super::symbols;
use super::{CodeEditor, Content};
use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use gpui::{actions, div, prelude::*, px, AnyElement, Context, Entity, Subscription, Window};
use std::path::PathBuf;

actions!(code_editor, [Rename]);

pub fn bind_keys(cx: &mut gpui::App) {
    use crate::key;
    cx.bind_keys([key("f2", Rename, Some(super::CONTEXT))]);
}

/// Occurrences renamed in one file at most without a server.
const MAX_LOCAL: usize = 5_000;

pub(super) struct RenameBox {
    input: Entity<TextInput>,
    path: PathBuf,
    /// The name as it is, and where (the place the server is asked about).
    old: String,
    at: Pos,
    _subscription: Subscription,
}

/// `text` with every whole-name `old` made `new` (`None` when there is none).
fn renamed_locally(text: &str, old: &str, new: &str, language: super::language::Language) -> Option<(String, usize)> {
    let found = symbols::occurrences(text, old, language, MAX_LOCAL);
    if found.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    for &(line, col) in found.iter().rev() {
        lines[line].replace_range(col..col + old.len(), new);
    }
    Some((lines.join("\n"), found.len()))
}

impl CodeEditor {
    pub(super) fn open_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && d.editable() && !d.previewing()) else {
            return;
        };
        let cursor = doc.buffer.cursor();
        let line = doc.buffer.line(cursor.line);
        let Some((start, end)) = symbols::name_at(line, cursor.col, doc.language).filter(|(s, e)| e > s) else {
            return self.flash(t(cx, "editor.rename_nothing").to_string(), cx);
        };
        let old = line[start..end].to_string();
        let input = cx.new(|cx| TextInput::new(old.clone(), "", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed => this.confirm_rename(window, cx),
            TextInputEvent::Cancelled => this.close_rename(window, cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.problem_hover = None;
        self.rename = Some(RenameBox { input, path: doc.path.clone(), old, at: Pos::new(cursor.line, start), _subscription: subscription });
        cx.notify();
    }

    pub(super) fn close_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.take().is_some() {
            window.focus(&self.focus_handle);
            cx.notify();
        }
    }

    pub(super) fn confirm_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.as_ref() else { return };
        let new = rename.input.read(cx).text().trim().to_string();
        let (old, at, path) = (rename.old.clone(), rename.at, rename.path.clone());
        self.close_rename(window, cx);
        if new.is_empty() || new == old {
            return;
        }
        let Some(doc) = self.docs.iter().find(|d| d.path == path) else { return };
        let (project, language, text) = (doc.project.clone(), doc.language, lsp::Snapshot::of(&doc.buffer));
        let since = (path.clone(), text.revision);
        let ask = (new.clone(), path.clone());
        cx.spawn(async move |this, cx| {
            let (wanted, file) = ask;
            let answer = cx.background_spawn(async move { lsp::rename(&project, language, &file, &text, at, &wanted) }).await;
            match answer {
                Some(Ok(edits)) if !edits.is_empty() => {
                    let files = edits.len();
                    let places: usize = edits.iter().map(lsp::edit_count).sum();
                    let Some(ok) = CodeEditor::apply_edits(&this, edits, since, cx).await else { return };
                    let _ = this.update(cx, |this, cx| {
                        let note = if ok {
                            tf(cx, "editor.renamed", &[("n", &places.to_string()), ("files", &files.to_string())])
                        } else {
                            t(cx, "editor.fix_failed").to_string()
                        };
                        this.flash(note, cx);
                    });
                }
                // The server would not: its reason, or a plain "cannot rename this".
                Some(refused) => {
                    let _ = this.update(cx, |this, cx| {
                        let reason = match refused {
                            Err(reason) if !reason.is_empty() => reason,
                            _ => t(cx, "editor.rename_refused").to_string(),
                        };
                        this.flash(reason, cx);
                    });
                }
                None => {
                    // No server: this file only.
                    let _ = this.update(cx, |this, cx| {
                        let Some(index) = this.docs.iter().position(|d| d.path == path) else { return };
                        let text = this.docs[index].buffer.text();
                        let note = match renamed_locally(&text, &old, &new, this.docs[index].language) {
                            Some((renamed, n)) if this.replace_text(index, &renamed, cx) => {
                                tf(cx, "editor.renamed_here", &[("n", &n.to_string())])
                            }
                            _ => t(cx, "editor.rename_refused").to_string(),
                        };
                        this.flash(note, cx);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(super) fn render_rename(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rename = self.rename.as_ref()?;
        self.docs.get(self.active).filter(|d| d.path == rename.path)?;
        let at = self.layout.as_ref()?.point_below(rename.at)?;
        let panel = div()
            .id("editor-rename")
            .w(px(300.))
            .flex()
            .flex_col()
            .gap_1()
            .p_1p5()
            .rounded_md()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex_alpha(Chrome::ACCENT, 0.7))
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(hex(Chrome::EDITOR))
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .t_body()
                    .font_family(crate::settings::terminal_font(cx))
                    .text_color(hex(Chrome::BRIGHT))
                    .child(rename.input.clone()),
            )
            .child(div().px_0p5().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "editor.rename_keys")));
        Some(
            gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(self.popup_margin()).child(panel))
                .with_priority(3)
                .into_any_element(),
        )
    }

    /// Debug driver: F2 and a new name, confirmed.
    pub(super) fn debug_rename(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_rename(window, cx);
        if let Some(input) = self.rename.as_ref().map(|r| r.input.clone()) {
            input.update(cx, |input, cx| input.set_text(name.to_string(), cx));
            self.confirm_rename(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::language::Language;

    #[test]
    fn renames_whole_names_only() {
        let (text, n) = renamed_locally("let total = 1;\nconsole.log(total, totals);\n", "total", "sum", Language::JavaScript).unwrap();
        assert_eq!(text, "let sum = 1;\nconsole.log(sum, totals);\n");
        assert_eq!(n, 2);
        assert!(renamed_locally("let a = 1;", "total", "sum", Language::JavaScript).is_none());
    }
}
