//! Completion as you type: the language server's list (with the import a name needs, added when
//! it is chosen), or, without a server, the words of the file. It opens on a name being typed or
//! after `.` (`::`, `->`), and with **⌃Space**; ↑↓ choose, ↩ or ⇥ put it in, Esc closes it.
//! Typing on filters the list; a server that said its list was partial is asked again.

use super::buffer::Pos;
use super::lsp;
use super::symbols;
use super::{CodeEditor, Content};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use gpui::{actions, div, prelude::*, px, AnyElement, ClickEvent, Context};
use std::path::PathBuf;
use std::time::Duration;

actions!(code_editor, [TriggerCompletion, Dismiss]);

pub fn bind_keys(cx: &mut gpui::App) {
    use crate::key;
    let c = Some(super::CONTEXT);
    cx.bind_keys([key("ctrl-space", TriggerCompletion, c), key("escape", Dismiss, c)]);
}

/// Rows the list shows at once.
const ROWS: usize = 10;
/// Rows kept after filtering.
const MAX_SHOWN: usize = 200;
/// A word of the file is offered once this much of it is typed.
const WORD_MIN: usize = 2;
/// How long the server gets to answer.
const WAIT: Duration = Duration::from_millis(1500);
/// How long a chosen item may take to bring its import.
const RESOLVE_WAIT: Duration = Duration::from_millis(800);
/// Lines of the file read for its words at most.
const WORD_LINES: usize = 20_000;

#[derive(Clone)]
enum Source {
    Server(Box<lsp::CompletionItem>),
    Word,
}

#[derive(Clone)]
struct Entry {
    label: String,
    detail: Option<String>,
    tag: &'static str,
    filter: String,
    sort: String,
    preferred: bool,
    source: Source,
}

pub(super) struct Completion {
    path: PathBuf,
    /// Where the word being completed starts.
    start: Pos,
    entries: Vec<Entry>,
    /// The entries that match what is typed, best first.
    shown: Vec<usize>,
    selected: usize,
    /// The server's list was whole: typing only filters it.
    complete: bool,
    /// The request whose answer is wanted.
    asking: u64,
}

fn tag(kind: Option<u64>) -> &'static str {
    match kind {
        Some(2) => "method",
        Some(3) => "fn",
        Some(4) => "new",
        Some(5) => "field",
        Some(6) => "var",
        Some(7) => "class",
        Some(8) => "iface",
        Some(9) => "mod",
        Some(10) => "prop",
        Some(13) => "enum",
        Some(14) => "kw",
        Some(15) => "snip",
        Some(20) => "member",
        Some(21) => "const",
        Some(22) => "struct",
        Some(25) => "type",
        _ => "",
    }
}

/// Whether `query` appears in `name` in order (any case) from its first letter, and how well: 0
/// a prefix, 1 a prefix in other case, 2 scattered.
fn matches(name: &str, query: &str) -> Option<u8> {
    if query.is_empty() || name.starts_with(query) {
        return Some(0);
    }
    let (name_l, query_l) = (name.to_lowercase(), query.to_lowercase());
    if name_l.starts_with(&query_l) {
        return Some(1);
    }
    // Scattered, but from the first letter on (`gtl` → `getTotal`, not `AuthenticatorAttestation`
    // for `tot`).
    if name_l.chars().next() != query_l.chars().next() {
        return None;
    }
    let mut rest = name_l.chars();
    query_l.chars().all(|q| rest.any(|c| c == q)).then_some(2)
}

/// The words of `text` (names of at least [`WORD_MIN`] + 1 characters), each once.
fn words(text: &str, language: super::language::Language) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in text.lines().take(WORD_LINES) {
        for word in line.split(|c: char| !symbols::is_name_char(c, language)) {
            let word = word.trim_start_matches(|c: char| c.is_ascii_digit());
            if word.chars().count() > WORD_MIN && seen.insert(word) {
                out.push(word.to_string());
            }
        }
    }
    out
}

/// The byte offset of `pos` in `text` (lines joined with `\n`).
fn offset_of(text: &str, pos: Pos) -> usize {
    let start: usize = text.split('\n').take(pos.line).map(|l| l.len() + 1).sum();
    (start + pos.col).min(text.len())
}

impl CodeEditor {
    /// After text was typed at the cursor: open, filter or close the list.
    pub(super) fn completion_typed(&mut self, typed: &str, cx: &mut Context<Self>) {
        // Prose and plain text are written, not completed (⌃Space still asks).
        let code = |d: &&super::Document| matches!(d.content, Content::Text) && d.editable() && d.language.is_code();
        let Some(doc) = self.docs.get(self.active).filter(code) else { return };
        // Off in Settings: only ⌃Space opens the list (an open one still filters as typed).
        if !crate::settings::settings(cx).editor_complete_while_typing && self.completion.is_none() {
            return;
        }
        let cursor = doc.buffer.cursor();
        let line = doc.buffer.line(cursor.line);
        let mut chars = typed.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            self.completion = None;
            return;
        };
        if symbols::is_name_char(c, doc.language) {
            if self.completion.is_some() {
                return self.completion_filter(true, cx);
            }
            let start = symbols::name_at(line, cursor.col, doc.language).map_or(cursor.col, |(s, _)| s);
            if line[start..cursor.col].chars().next().is_some_and(|c| !c.is_ascii_digit()) {
                self.request_completion(None, cx);
            }
            return;
        }
        let before = line[..cursor.col].chars().rev().nth(1);
        let trigger = match c {
            '.' => true,
            ':' => before == Some(':'),
            '>' => before == Some('-'),
            _ => false,
        };
        self.completion = None;
        if trigger && lsp::id(doc.language).is_some() {
            self.request_completion(Some(c), cx);
        }
        cx.notify();
    }

    /// ⌃Space.
    pub(super) fn trigger_completion(&mut self, cx: &mut Context<Self>) {
        self.completion = None;
        self.request_completion(None, cx);
    }

    /// Backspace: the list follows the shorter word, and closes once it is gone.
    pub(super) fn completion_erased(&mut self, cx: &mut Context<Self>) {
        if self.completion.is_some() {
            self.completion_filter(false, cx);
        }
    }

    fn request_completion(&mut self, trigger: Option<char>, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && d.editable()) else { return };
        let cursor = doc.buffer.cursor();
        let line = doc.buffer.line(cursor.line);
        let start = symbols::name_at(line, cursor.col, doc.language).map_or(cursor.col, |(s, _)| s.min(cursor.col));
        self.completion_seq += 1;
        let asking = self.completion_seq;
        let (project, language, path, text) = (doc.project.clone(), doc.language, doc.path.clone(), lsp::Snapshot::of(&doc.buffer));
        let server = lsp::id(language).is_some() && lsp::status(&project, language, &path) == "ready";
        match self.completion.as_mut() {
            Some(open) => open.asking = asking,
            None => {
                self.completion = Some(Completion {
                    path: path.clone(),
                    start: Pos::new(cursor.line, start),
                    entries: Vec::new(),
                    shown: Vec::new(),
                    selected: 0,
                    complete: true,
                    asking,
                })
            }
        }
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let listed = server
                        .then(|| lsp::completions(&project, language, &path, &text, cursor.line, cursor.col, trigger, WAIT))
                        .flatten();
                    match listed {
                        Some((items, complete)) => (
                            items
                                .into_iter()
                                .map(|item| Entry {
                                    label: item.label.clone(),
                                    detail: item.detail.clone(),
                                    tag: tag(item.kind),
                                    filter: item.filter.clone(),
                                    sort: item.sort.clone(),
                                    preferred: item.preferred,
                                    source: Source::Server(Box::new(item)),
                                })
                                .collect::<Vec<_>>(),
                            complete,
                        ),
                        // No server (or none answering): the words of the file, never after `.`.
                        None if trigger.is_none() => (
                            words(&text, language)
                                .into_iter()
                                .map(|word| Entry {
                                    label: word.clone(),
                                    detail: None,
                                    tag: "abc",
                                    filter: word.clone(),
                                    sort: word,
                                    preferred: false,
                                    source: Source::Word,
                                })
                                .collect(),
                            true,
                        ),
                        None => (Vec::new(), true),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(open) = this.completion.as_mut().filter(|c| c.asking == asking) else { return };
                (open.entries, open.complete) = found;
                this.completion_filter(false, cx);
            });
        })
        .detach();
    }

    /// Filters the list by the word typed so far (`typed`: a character was just added, so a
    /// partial list is asked for again).
    fn completion_filter(&mut self, typed: bool, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let Some(open) = self.completion.as_mut() else { return };
        let cursor = doc.buffer.cursor();
        if open.path != doc.path || cursor.line != open.start.line || cursor.col < open.start.col {
            self.completion = None;
            return cx.notify();
        }
        // The word since where the list opened (gone when the text changed under it: undo, a
        // line moved).
        let Some(query) = doc.buffer.line(cursor.line).get(open.start.col..cursor.col).map(str::to_string) else {
            self.completion = None;
            return cx.notify();
        };
        if typed && !open.complete {
            return self.request_completion(None, cx);
        }
        let words_only = open.entries.iter().all(|e| matches!(e.source, Source::Word));
        let mut scored: Vec<(u8, usize)> = open
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !words_only || (query.chars().count() >= WORD_MIN && e.label != query))
            .filter_map(|(i, e)| matches(&e.filter, &query).map(|score| (score, i)))
            .collect();
        let entries = &open.entries;
        scored.sort_by(|a, b| {
            let (x, y) = (&entries[a.1], &entries[b.1]);
            (a.0, !x.preferred, &x.sort, &x.label).cmp(&(b.0, !y.preferred, &y.sort, &y.label))
        });
        open.shown = scored.into_iter().take(MAX_SHOWN).map(|(_, i)| i).collect();
        open.selected = 0;
        let waiting = open.entries.is_empty();
        if open.shown.is_empty() && !waiting {
            self.completion = None;
        }
        cx.notify();
    }

    /// ↑ / ↓ while the list shows (true: the key was for the list).
    pub(super) fn completion_step(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(open) = self.completion.as_mut().filter(|c| !c.shown.is_empty()) else { return false };
        let n = open.shown.len();
        open.selected = if down { (open.selected + 1) % n } else { (open.selected + n - 1) % n };
        cx.notify();
        true
    }

    /// ↩ / ⇥ while the list shows: puts the chosen entry in (true: the key was for the list).
    pub(super) fn completion_accept(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(open) = self.completion.take() else { return false };
        let Some(entry) = open.shown.get(open.selected).and_then(|&i| open.entries.get(i)).cloned() else {
            cx.notify();
            return false;
        };
        // A keyword or word already typed in full (`return`↩): the key does what it does without a
        // list. A name may still bring its import, so it is put in.
        let typed = self.docs.get(self.active).map(|d| {
            let cursor = d.buffer.cursor();
            d.buffer.line(cursor.line).get(open.start.col..cursor.col).unwrap_or("").to_string()
        });
        let plain = match &entry.source {
            Source::Word => true,
            Source::Server(item) => item.kind == Some(14),
        };
        if plain && typed.as_deref() == Some(entry.label.as_str()) {
            cx.notify();
            return false;
        }
        self.accept_entry(entry, open.start, cx);
        true
    }

    fn accept_entry(&mut self, entry: Entry, start: Pos, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let (project, language, path) = (doc.project.clone(), doc.language, doc.path.clone());
        match entry.source {
            Source::Word => {
                let cursor = doc.buffer.cursor();
                let edit = lsp::byte_edit(&path, (start.line, start.col), (cursor.line, cursor.col), &entry.label);
                self.put_completion(&path, &edit, cx);
            }
            Source::Server(item) => {
                // Its import comes when asked for; the name goes in once it is here.
                let file = path.clone();
                cx.spawn(async move |this, cx| {
                    let item =
                        cx.background_spawn(async move { lsp::resolve_completion(&project, language, &file, &item, RESOLVE_WAIT) }).await;
                    let _ = this.update(cx, |this, cx| {
                        let Some(doc) = this.docs.iter().find(|d| d.path == path) else { return };
                        let cursor = doc.buffer.cursor();
                        let text = doc.buffer.text();
                        let edit = lsp::completion_edit(&path, &text, &item, ((start.line, start.col), (cursor.line, cursor.col)));
                        this.put_completion(&path, &edit, cx);
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Makes `edit` in the open file `path` as one undo step, the cursor after the name put in.
    fn put_completion(&mut self, path: &std::path::Path, edit: &lsp::FileEdit, cx: &mut Context<Self>) {
        let Some(index) = self.docs.iter().position(|d| d.path == path) else { return };
        let doc = &self.docs[index];
        let text = doc.buffer.text();
        let (new, mark) = lsp::edited_at(&text, edit, offset_of(&text, doc.buffer.cursor()));
        if self.replace_text(index, &new, cx) {
            let before = &new[..mark];
            let at = Pos::new(before.matches('\n').count(), mark - before.rfind('\n').map_or(0, |i| i + 1));
            let doc = &mut self.docs[index];
            let at = doc.buffer.clamp(at);
            doc.buffer.move_to(at, false);
            doc.autoscroll = true;
        }
        cx.notify();
    }

    /// Esc: the list and the windows over the text close.
    pub(super) fn dismiss(&mut self, cx: &mut Context<Self>) {
        let shown = self.completion.is_some() || self.git_peek.is_some() || self.signature.is_some();
        if !shown {
            if let Some(doc) = self.docs.get_mut(self.active) {
                doc.buffer.single_cursor();
            }
        }
        self.completion = None;
        self.git_peek = None;
        self.signature = None;
        self.problem_hover = None;
        self.type_hover = None;
        cx.notify();
    }

    pub(super) fn render_completion(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.completion.as_ref().filter(|c| !c.shown.is_empty())?;
        self.docs.get(self.active).filter(|d| d.path == open.path)?;
        let at = self.layout.as_ref()?.point_below(open.start)?;
        let first = open.selected.saturating_sub(ROWS - 1).min(open.shown.len().saturating_sub(ROWS));
        let font = crate::settings::terminal_font(cx);
        let rows = open.shown.iter().enumerate().skip(first).take(ROWS).map(|(row, &index)| {
            let entry = &open.entries[index];
            let selected = row == open.selected;
            div()
                .id(("editor-completion", row))
                .mx_1()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .when(selected, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.35)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(div().w(px(42.)).flex_shrink_0().truncate().t_caption().text_color(hex(Chrome::BLUE)).child(entry.tag))
                .child(
                    div()
                        .flex_shrink_0()
                        .max_w(gpui::relative(0.55))
                        .truncate()
                        .font_family(font.clone())
                        .text_color(hex(if selected { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                        .child(entry.label.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_right()
                        .t_caption()
                        .text_color(hex(Chrome::MUTED))
                        .children(entry.detail.clone()),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(open) = this.completion.as_mut() {
                        open.selected = row;
                    }
                    this.completion_accept(cx);
                }))
        });
        let list = div()
            .id("editor-completion-list")
            .w(px(460.))
            .py_1()
            .rounded_md()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .occlude()
            .t_small()
            .children(rows);
        Some(
            gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(px(8.)).child(list))
                .with_priority(3)
                .into_any_element(),
        )
    }

    pub(super) fn debug_completion(&self) -> serde_json::Value {
        match &self.completion {
            Some(open) => serde_json::json!({
                "start": [open.start.line, open.start.col],
                "selected": open.selected,
                "complete": open.complete,
                "shown": open.shown.iter().take(8).map(|&i| {
                    let e = &open.entries[i];
                    format!("{} {}{}", e.tag, e.label, e.detail.as_ref().map(|d| format!(" — {d}")).unwrap_or_default())
                }).collect::<Vec<_>>(),
            }),
            None => serde_json::Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::language::Language;

    #[test]
    fn matches_prefixes_first_then_in_order() {
        assert_eq!(matches("total", "tot"), Some(0));
        assert_eq!(matches("Total", "tot"), Some(1));
        assert_eq!(matches("getTotal", "gtl"), Some(2));
        assert_eq!(matches("AuthenticatorAttestationResponse", "tot"), None);
        assert_eq!(matches("total", "xyz"), None);
        assert_eq!(matches("anything", ""), Some(0));
    }

    #[test]
    fn prose_is_not_code() {
        assert!(Language::TypeScript.is_code() && Language::Css.is_code());
        assert!(!Language::Markdown.is_code() && !Language::Other.is_code());
    }

    #[test]
    fn words_of_the_file_once() {
        let found = words("let total = 1;\nlet totals = total + 2 + x1;\n", Language::JavaScript);
        assert_eq!(found, ["let", "total", "totals"]);
        assert_eq!(offset_of("ab\ncd", Pos::new(1, 1)), 4);
    }
}
