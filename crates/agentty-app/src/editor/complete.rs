//! Completion as you type: the language server's list (with the import a name needs, added when
//! it is chosen), or, without a server, the words of the file. It opens on a name being typed or
//! after `.` (`::`, `->`), and with **⌃Space**; ↑↓ choose, ↩ or ⇥ put it in, Esc closes it.
//! Typing on filters the list; a server that said its list was partial is asked again. A chosen
//! snippet (`onSearch={…}`, `setName(name)`) selects its first placeholder, and ⇥ goes on to the
//! next one and then to its end, as in VS Code. In HTML and JSX an Emmet abbreviation before
//! the cursor (`ul>li*3`) comes first ([`super::emmet`]).

use super::buffer::{Buffer, Pos, Selection};
use super::emmet;
use super::language::Language;
use super::lsp;
use super::project_index;
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
/// Names of the project offered without a server at most.
const PROJECT_NAMES: usize = 5_000;
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
    Emmet(Box<emmet::Expansion>),
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

/// A snippet put in by a completion with tab stops still ahead. Stops are kept counted from the
/// end of the text (lines from the last one, bytes from the end of their line), which typing at an
/// earlier stop leaves where they were. ⇥ outside it, Esc or a click ends it.
pub(super) struct Snippet {
    path: PathBuf,
    /// Where it starts.
    start: Pos,
    /// The stops still ahead: lines below, bytes from the end of the line, length.
    ahead: Vec<(usize, usize, usize)>,
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
        Some(12) => "value",
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

/// Where a snippet's stop is now (kept as lines below it and bytes from its line's end).
fn stop_at(buffer: &Buffer, (below, from_end, _): (usize, usize, usize)) -> Option<Pos> {
    let line = buffer.line_count().checked_sub(1 + below)?;
    let col = buffer.line(line).len().checked_sub(from_end)?;
    buffer.line(line).is_char_boundary(col).then_some(Pos::new(line, col))
}

/// A hash of the lines above `line`: an import found for them still fits while it is the same.
fn text_above(text: &str, line: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.split('\n').take(line).for_each(|l| l.hash(&mut hasher));
    hasher.finish()
}

/// A snippet's lines after its first take the indentation of the line it goes on, and its tabs
/// the file's indentation step, as in VS Code; its stops move along.
fn indent_snippet(insert: &str, stops: &[(usize, usize)], indent: &str, unit: &str) -> Option<(String, Vec<(usize, usize)>)> {
    if stops.is_empty() || !insert.contains(['\n', '\t']) {
        return None;
    }
    let mut out = String::with_capacity(insert.len() + indent.len() * 4);
    // Where each byte of the snippet went.
    let mut moved = Vec::with_capacity(insert.len() + 1);
    for c in insert.chars() {
        moved.extend(std::iter::repeat_n(out.len(), c.len_utf8()));
        match c {
            '\n' => {
                out.push('\n');
                out.push_str(indent);
            }
            '\t' => out.push_str(unit),
            c => out.push(c),
        }
    }
    moved.push(out.len());
    let stops = stops.iter().map(|&(from, to)| (moved[from.min(insert.len())], moved[to.min(insert.len())])).collect();
    Some((out, stops))
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

/// Whether a completion's label and detail (`findById(long id) : Optional<Product>`,
/// `fn(&self, id: u64)`, `func(id int64) error`) show parameters to fill in (a method's own
/// `self` does not count).
fn takes_arguments(signature: &str) -> bool {
    let Some(open) = signature.find('(') else { return false };
    let inside = signature[open + 1..].split(')').next().unwrap_or("");
    let rest = inside.trim().trim_start_matches("&mut self").trim_start_matches("&self").trim_start_matches("self");
    !rest.trim_start_matches(',').trim().is_empty()
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
            if line[start..cursor.col].chars().next().is_some_and(|c| !c.is_ascii_digit()) || self.emmet_entry().is_some() {
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
        if !trigger && self.emmet_entry().is_some() {
            return self.request_completion(None, cx);
        }
        // After `.` only a server knows the members (the file's words would be any word).
        if trigger && lsp::id(doc.language).is_some() && lsp::status(&doc.project, doc.language, &doc.path) == "ready" {
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
        // An abbreviation needs no server: it is offered at once.
        if self.emmet_entry().is_some() {
            self.completion_filter(false, cx);
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
                        // No server (or none answering): the names the project defines and the
                        // words of the file, never after `.`.
                        None if trigger.is_none() => {
                            let mut seen = std::collections::HashSet::new();
                            let defined = project_index::names(&project, language, PROJECT_NAMES)
                                .into_iter()
                                .filter(|(name, _)| name.chars().count() > WORD_MIN)
                                .map(|(name, kind)| (name, kind.tag()));
                            let entries = defined
                                .chain(words(&text, language).into_iter().map(|word| (word, "abc")))
                                .filter(|(name, _)| seen.insert(name.clone()))
                                .map(|(word, tag)| Entry {
                                    label: word.clone(),
                                    detail: None,
                                    tag,
                                    filter: word.clone(),
                                    sort: word,
                                    preferred: false,
                                    source: Source::Word,
                                })
                                .collect();
                            (entries, true)
                        }
                        None => (Vec::new(), true),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let emmet = this.emmet_entry();
                let Some(open) = this.completion.as_mut().filter(|c| c.asking == asking) else { return };
                (open.entries, open.complete) = found;
                open.entries.extend(emmet);
                // Nothing to offer: no list left open that typing would only filter.
                if open.entries.is_empty() {
                    this.completion = None;
                    return cx.notify();
                }
                this.completion_filter(false, cx);
            });
        })
        .detach();
    }

    /// Filters the list by the word typed so far (`typed`: a character was just added, so a
    /// partial list is asked for again).
    fn completion_filter(&mut self, typed: bool, cx: &mut Context<Self>) {
        let emmet = self.emmet_entry();
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
        // The abbreviation as it is now, first.
        let is_emmet = |e: &Entry| matches!(e.source, Source::Emmet(_));
        open.entries.retain(|e| !is_emmet(e));
        open.entries.extend(emmet);
        let words_only = open.entries.iter().filter(|e| !is_emmet(e)).all(|e| matches!(e.source, Source::Word));
        let mut scored: Vec<(u8, usize)> = open
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| is_emmet(e) || !words_only || (query.chars().count() >= WORD_MIN && e.label != query))
            .filter_map(|(i, e)| if is_emmet(e) { Some((0, i)) } else { matches(&e.filter, &query).map(|score| (score, i)) })
            .collect();
        let entries = &open.entries;
        // A server's snippet (rust-analyzer's `arc` for `s.ar`) after the names it matches as well.
        let snippet = |e: &Entry| matches!(&e.source, Source::Server(item) if item.kind == Some(15));
        scored.sort_by(|a, b| {
            let (x, y) = (&entries[a.1], &entries[b.1]);
            (!is_emmet(x), a.0, !x.preferred, snippet(x), &x.sort, &x.label).cmp(&(
                !is_emmet(y),
                b.0,
                !y.preferred,
                snippet(y),
                &y.sort,
                &y.label,
            ))
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
            Source::Emmet(_) => false,
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
            Source::Emmet(expansion) => {
                let cursor = doc.buffer.cursor();
                let line = doc.buffer.line(cursor.line);
                let indent = &line[..line.len() - line.trim_start().len()];
                let (text, stops) = indent_snippet(&expansion.text, &expansion.stops, indent, doc.buffer.indent_unit())
                    .unwrap_or((expansion.text.clone(), expansion.stops.clone()));
                let edit = lsp::byte_edit(&path, (cursor.line, expansion.start), (cursor.line, cursor.col), &text);
                self.put_completion(&path, &edit, cx);
                self.start_snippet_text(&path, &text, &stops, cx);
            }
            Source::Server(item) => {
                // The name goes in now, so what is typed next follows it; the import it needs
                // comes when the server has resolved it, if the text above is still as it was.
                let cursor = doc.buffer.cursor();
                let mut item = *item;
                let line = doc.buffer.line(cursor.line);
                if let Some((insert, stops)) =
                    indent_snippet(&item.insert, &item.stops, &line[..line.len() - line.trim_start().len()], doc.buffer.indent_unit())
                {
                    (item.insert, item.stops) = (insert, stops);
                }
                let text = doc.buffer.text();
                let edit = lsp::completion_edit(&path, &text, &item, ((start.line, start.col), (cursor.line, cursor.col)));
                let above = text_above(&text, start.line);
                self.put_completion(&path, &edit, cx);
                self.start_snippet(&path, &item, cx);
                self.call_parens(&path, language, &item, cx);
                if item.suggests_more() {
                    self.request_completion(None, cx);
                }
                if lsp::completion_imports(&path, &item, start.line).is_some() {
                    return cx.notify();
                }
                let file = path.clone();
                cx.spawn(async move |this, cx| {
                    let item =
                        cx.background_spawn(async move { lsp::resolve_completion(&project, language, &file, &item, RESOLVE_WAIT) }).await;
                    let _ = this.update(cx, |this, cx| {
                        let Some(doc) = this.docs.iter().find(|d| d.path == path) else { return };
                        if text_above(&doc.buffer.text(), start.line) != above {
                            return;
                        }
                        if let Some(imports) = lsp::completion_imports(&path, &item, start.line) {
                            this.put_imports(&path, &imports, cx);
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Just after `item` went in (the cursor after its text): to its first tab stop, the others
    /// kept for ⇥. Plain text (a CSS value inside `display: …;`) leaves the snippet it went into.
    fn start_snippet(&mut self, path: &std::path::Path, item: &lsp::CompletionItem, cx: &mut Context<Self>) {
        self.start_snippet_text(path, &item.insert, &item.stops, cx);
        // Into a call's parentheses: what goes there.
        if !item.stops.is_empty() && item.insert.contains('(') {
            self.signature_typed("(", cx);
        }
    }

    /// [`Self::start_snippet`] for `insert` just put in, with its `stops`.
    fn start_snippet_text(&mut self, path: &std::path::Path, insert: &str, stops: &[(usize, usize)], cx: &mut Context<Self>) {
        if stops.is_empty() {
            return;
        }
        self.snippet = None;
        let Some(doc) = self.docs.iter_mut().find(|d| d.path == path) else { return };
        if doc.buffer.has_extra_cursors() {
            return;
        }
        let end = doc.buffer.cursor();
        let buffer = &doc.buffer;
        // Where a byte of the text put in is now: the rest of its line is that text too.
        let at = |offset: usize| {
            let tail = &insert[offset.min(insert.len())..];
            let below = tail.matches('\n').count();
            let line = end.line.saturating_sub(below);
            let rest = tail.split('\n').next().unwrap_or("").len();
            let col = if below == 0 { end.col.saturating_sub(rest) } else { buffer.line(line).len().saturating_sub(rest) };
            buffer.clamp(Pos::new(line, col))
        };
        // A stop inside another's placeholder (`${1:a${2:b}}`) goes once that text is typed over:
        // only the outer one is visited.
        let inside = |stop: &(usize, usize)| stops.iter().any(|o| o != stop && o.0 < o.1 && o.0 <= stop.0 && stop.1 <= o.1);
        let last = stops.len() - 1;
        let stops: Vec<(Pos, usize)> = stops
            .iter()
            .enumerate()
            .filter(|(i, stop)| *i == last || !inside(stop))
            .map(|(_, &(from, to))| {
                let (from, to) = (at(from), at(to));
                (from, if to.line == from.line { to.col.saturating_sub(from.col) } else { 0 })
            })
            .collect();
        let start = at(0);
        let count = buffer.line_count();
        let ahead =
            stops[1..].iter().map(|&(pos, len)| (count - 1 - pos.line, buffer.line(pos.line).len() - pos.col, len)).collect::<Vec<_>>();
        let (first, len) = stops[0];
        doc.buffer.move_to(first, false);
        if len > 0 {
            doc.buffer.move_to(Pos::new(first.line, first.col + len), true);
        }
        if !ahead.is_empty() {
            self.snippet = Some(Snippet { path: path.to_path_buf(), start, ahead });
        }
        cx.notify();
    }

    /// An Emmet abbreviation ending at the cursor, in HTML or in a JSX file's markup.
    fn emmet_entry(&self) -> Option<Entry> {
        let doc = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && d.editable())?;
        let jsx = match doc.language {
            Language::Html => false,
            Language::Tsx | Language::Jsx => true,
            _ => return None,
        };
        if !doc.buffer.selection.is_empty() || doc.buffer.has_extra_cursors() || (jsx && !doc.buffer.selection_is_jsx()) {
            return None;
        }
        let cursor = doc.buffer.cursor();
        if !jsx {
            let line = doc.buffer.line(cursor.line);
            let above = (cursor.line.saturating_sub(2000)..cursor.line).rev().map(|i| doc.buffer.line(i));
            if emmet::in_script_or_style(std::iter::once(&line[..cursor.col]).chain(above)) {
                return None;
            }
        }
        let expansion = emmet::expand_at(doc.buffer.line(cursor.line), cursor.col, jsx)?;
        Some(Entry {
            label: expansion.abbreviation.clone(),
            detail: Some("Emmet".into()),
            tag: "emmet",
            filter: String::new(),
            sort: String::new(),
            preferred: true,
            source: Source::Emmet(Box::new(expansion)),
        })
    }

    /// ⇥ in a snippet: to its next stop (true), or nothing to do with one.
    pub(super) fn snippet_next(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(mut snippet) = self.snippet.take() else { return false };
        let Some(doc) = self.docs.get_mut(self.active).filter(|d| d.path == snippet.path) else { return false };
        let end = snippet.ahead.last().and_then(|&stop| stop_at(&doc.buffer, stop));
        let (below, from_end, len) = snippet.ahead.remove(0);
        let cursor = doc.buffer.cursor();
        let next = stop_at(&doc.buffer, (below, from_end, len));
        // Gone, or the cursor went out of the snippet: ⇥ indents as usual.
        let inside = |pos: Pos| end.is_some_and(|end| snippet.start <= pos && pos <= end);
        let Some(next) = next.filter(|next| inside(cursor) && inside(*next)) else { return false };
        doc.buffer.move_to(next, false);
        let to = doc.buffer.clamp(Pos::new(next.line, next.col + len));
        if to > next {
            doc.buffer.move_to(to, true);
        }
        doc.autoscroll = true;
        if !snippet.ahead.is_empty() {
            self.snippet = Some(snippet);
        }
        // The call's hint follows to the argument now chosen (or closes past it).
        self.signature_typed("", cx);
        cx.notify();
        true
    }

    /// The cursor moved: a snippet it left is done.
    pub(super) fn snippet_follow(&mut self) {
        let Some(snippet) = self.snippet.as_ref() else { return };
        let left = self.docs.get(self.active).filter(|d| d.path == snippet.path).is_none_or(|doc| {
            let cursor = doc.buffer.cursor();
            let end = snippet.ahead.last().and_then(|&stop| stop_at(&doc.buffer, stop));
            !end.is_some_and(|end| snippet.start <= cursor && cursor <= end)
        });
        if left {
            self.snippet = None;
        }
    }

    /// A method or function chosen in Java, Kotlin, Rust or Go comes with its parentheses (as VS
    /// Code puts them there), the cursor between them when it takes arguments; TypeScript, Python
    /// and the others leave the name alone, as a function is often passed rather than called.
    fn call_parens(&mut self, path: &std::path::Path, language: Language, item: &lsp::CompletionItem, cx: &mut Context<Self>) {
        let callable = matches!(item.kind, Some(2..=4));
        let adds = matches!(language, Language::Java | Language::Kotlin | Language::Rust | Language::Go);
        let Some(doc) = self.docs.iter_mut().find(|d| d.path == path) else { return };
        let cursor = doc.buffer.cursor();
        let after = doc.buffer.line(cursor.line)[cursor.col..].chars().next();
        if !callable || !adds || item.insert.contains('(') || matches!(after, Some('(' | '<')) {
            return;
        }
        let arguments = takes_arguments(&format!("{} {}", item.label, item.detail.as_deref().unwrap_or("")));
        doc.buffer.insert("()");
        if arguments {
            doc.buffer.left(false, false);
        }
        doc.sync_highlights();
        if arguments {
            self.signature_typed("(", cx);
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

    /// Lines added above the cursor (an import): the selection (a placeholder) and the snippet
    /// move down with their text.
    fn put_imports(&mut self, path: &std::path::Path, edit: &lsp::FileEdit, cx: &mut Context<Self>) {
        let Some(index) = self.docs.iter().position(|d| d.path == path) else { return };
        let (text, selection, count) = {
            let buffer = &self.docs[index].buffer;
            (buffer.text(), buffer.selection, buffer.line_count())
        };
        let (new, _) = lsp::edited_at(&text, edit, 0);
        if !self.replace_text(index, &new, cx) {
            return;
        }
        let doc = &mut self.docs[index];
        let grown = doc.buffer.line_count() as isize - count as isize;
        let shift = |pos: Pos| doc.buffer.clamp(Pos::new((pos.line as isize + grown).max(0) as usize, pos.col));
        let moved = Selection { anchor: shift(selection.anchor), head: shift(selection.head) };
        doc.buffer.move_to(moved.anchor, false);
        doc.buffer.move_to(moved.head, true);
        if let Some(snippet) = self.snippet.as_mut().filter(|s| s.path == path) {
            snippet.start.line = (snippet.start.line as isize + grown).max(0) as usize;
        }
        cx.notify();
    }

    /// Esc: the list and the windows over the text close; with none open, the snippet's stops and
    /// the other cursors go.
    pub(super) fn dismiss(&mut self, cx: &mut Context<Self>) {
        let shown = self.completion.is_some() || self.git_peek.is_some() || self.signature.is_some();
        if !shown {
            self.snippet = None;
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
                "order": open.shown.iter().take(8).map(|&i| {
                    let e = &open.entries[i];
                    format!("{}{} {}", if e.preferred { "*" } else { "" }, e.sort, e.filter)
                }).collect::<Vec<_>>(),
            }),
            None => serde_json::Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_takes_arguments_when_its_signature_shows_them() {
        assert!(takes_arguments("findById(long id) : Optional<Product>"));
        assert!(!takes_arguments("findAll() : List<Product>"));
        assert!(!takes_arguments("len fn(&self) -> usize"));
        assert!(takes_arguments("push fn(&mut self, value: T)"));
        assert!(takes_arguments("Save func(ctx context.Context) error"));
        assert!(!takes_arguments("count"));
    }

    #[test]
    fn a_snippets_lines_take_the_indentation_of_its_line() {
        // jdtls's `foreach` on a line indented by 8, in a file indented by 4.
        let insert = "for (T e : list) {\n\t\n}";
        let (text, stops) = indent_snippet(insert, &[(5, 6), (20, 20)], "        ", "    ").unwrap();
        assert_eq!(text, "for (T e : list) {\n            \n        }");
        assert_eq!((&text[stops[0].0..stops[0].1], stops[1]), ("T", (31, 31)), "the body's stop after the new indentation");
        assert_eq!(indent_snippet("f(x)", &[(2, 3), (4, 4)], "    ", "  "), None, "one line: as it is");
    }

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
