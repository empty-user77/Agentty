//! The text of an open file with its cursor, selection and undo history. Plain data, no UI: the
//! editor view turns keys into calls here, and the tests drive it directly.
//!
//! Text is kept as lines without their `\n` (line endings are restored on save, see `file.rs`).
//! A position is a line and a byte offset into that line, always on a character boundary.

use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

/// Columns a tab advances to (for display and for moving up and down).
pub const TAB_WIDTH: usize = 4;
/// Edits kept for undo; the oldest go first.
const UNDO_LIMIT: usize = 1000;
/// Typing within this long of the last keystroke is undone together.
const TYPING_GROUP: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    /// Byte offset into the line.
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// `anchor` stays where the selection started; `head` is the cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
}

impl Selection {
    pub fn cursor(pos: Pos) -> Self {
        Self { anchor: pos, head: pos }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// (start, end) in document order.
    pub fn range(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// How an edit came about, which decides whether it joins the previous one in the undo history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Typed characters (joined while typing continues).
    Typing,
    /// Backspace / delete of single characters (joined the same way).
    Deleting,
    /// Anything else: paste, cut, indent, format, reload — each undone on its own.
    Other,
}

#[derive(Clone, Debug)]
struct Edit {
    id: u64,
    start: Pos,
    removed: String,
    inserted: String,
    before: Selection,
    after: Selection,
    kind: EditKind,
    at: Instant,
    /// Closed to joining (saved since, or the cursor moved away).
    sealed: bool,
    /// Typed or deleted at several cursors at once (such edits are undone a run at a time).
    multi: bool,
    /// Undone together with the edit before it (typing on at several cursors).
    chain: bool,
}

pub struct Buffer {
    lines: Vec<String>,
    pub selection: Selection,
    /// More cursors besides `selection` (⌘D, ⌥-click), in no particular order. Typing, deleting,
    /// Enter, Tab and pasting happen at every one; moving moves every one; anything else first
    /// leaves the main cursor alone.
    pub extra: Vec<Selection>,
    /// A change for every cursor is being made (its edit must not drop the other cursors).
    multi: bool,
    /// Display column kept while moving up and down through shorter lines.
    goal_column: Option<usize>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    next_id: u64,
    /// Id of the edit the text was saved at (0: as loaded).
    saved_at: u64,
    /// The changes to the text since [`Buffer::take_changes`] was last asked, in order (`None`:
    /// too many to follow, take everything as changed).
    changes: Option<Vec<Splice>>,
    /// Counts every change to the text, for views that redraw from it (the markdown preview).
    revision: u64,
    /// One indentation step: a tab or some spaces, guessed from the file.
    indent: String,
    /// Text an input method is still composing (Hangul, kana, pinyin…).
    composition: Option<Composition>,
}

/// An input method's text in progress. It is shown in the document but only becomes one undo
/// step when committed, from the text as it was before composing began.
#[derive(Clone, Debug)]
struct Composition {
    /// Where the composed text starts, and what it replaced.
    start: Pos,
    removed: String,
    before: Selection,
    /// The marked (still changing) part.
    marked: (Pos, Pos),
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let indent = detect_indent(&lines);
        Self {
            lines,
            selection: Selection::default(),
            goal_column: None,
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
            saved_at: 0,
            changes: Some(Vec::new()),
            extra: Vec::new(),
            multi: false,
            revision: next_revision(),
            indent,
            composition: None,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn line(&self, index: usize) -> &str {
        self.lines.get(index).map_or("", String::as_str)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn indent_unit(&self) -> &str {
        &self.indent
    }

    /// Differs from the saved text (text an input method is still composing counts).
    pub fn is_dirty(&self) -> bool {
        self.state() != self.saved_at || self.composition.is_some()
    }

    /// The text as it is now counts as saved.
    pub fn mark_saved(&mut self) {
        self.saved_at = self.state();
        self.seal();
    }

    fn state(&self) -> u64 {
        self.undo.last().map_or(0, |edit| edit.id)
    }

    /// The next edit starts an undo step of its own.
    pub fn seal(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.sealed = true;
        }
    }

    /// The changes to the text since the last call, in order (for the views that keep something
    /// per line: syntax colors). `None`: too many to follow, everything may have changed.
    pub fn take_changes(&mut self) -> Option<Vec<Splice>> {
        self.changes.replace(Vec::new())
    }

    fn note_change(&mut self, splice: Splice) {
        if let Some(changes) = self.changes.as_mut() {
            if changes.len() < MAX_CHANGES {
                changes.push(splice);
            } else {
                self.changes = None;
            }
        }
    }

    /// Goes up with every change to the text.
    /// The text's revision: it changes with every edit, and is never the same for two texts of the
    /// app (every buffer counts on from one counter), so a language server handed a text by its
    /// revision never takes an older text for a newer one.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn cursor(&self) -> Pos {
        self.selection.head
    }

    /// The selected text; with several cursors, each one's in order, a line each.
    pub fn selected_text(&self) -> String {
        if !self.extra.is_empty() {
            let mut all = self.selections();
            all.sort_by_key(|s| s.range().0);
            return all.iter().map(|s| self.slice(s.range().0, s.range().1)).collect::<Vec<_>>().join("\n");
        }
        let (start, end) = self.selection.range();
        self.slice(start, end)
    }

    /// Every cursor, the main one first.
    pub fn selections(&self) -> Vec<Selection> {
        std::iter::once(self.selection).chain(self.extra.iter().copied()).collect()
    }

    /// Makes `f`'s change (what replaces which range) at every cursor as one edit, each cursor
    /// after its new text. False, doing nothing, with one cursor only.
    /// `typing`: one character typed or deleted at each (a run of them is undone at once).
    fn edit_all(&mut self, typing: bool, f: impl Fn(&Self, Selection) -> (Pos, Pos, String)) -> bool {
        if self.extra.is_empty() {
            return false;
        }
        self.unmark();
        let main = self.selection;
        let mut changes: Vec<(bool, Pos, Pos, String)> = self
            .selections()
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                let (start, end, text) = f(self, s);
                (i == 0, start.min(end), start.max(end), text)
            })
            .collect();
        changes.sort_by_key(|c| (c.1, c.2));
        // Ranges that came to overlap (two cursors deleting into each other): the first wins.
        let mut kept: Vec<(bool, Pos, Pos, String)> = Vec::with_capacity(changes.len());
        for change in changes {
            match kept.last_mut() {
                Some(last) if change.1 < last.2 || (change.1 == last.2 && change.1 == change.2 && last.1 == last.2) => {
                    last.0 |= change.0;
                    last.2 = last.2.max(change.2);
                }
                _ => kept.push(change),
            }
        }
        let first = kept[0].1;
        let mut text = String::new();
        let mut at = first;
        let mut from = first;
        let mut after = Vec::with_capacity(kept.len());
        for (is_main, start, end, piece) in &kept {
            let gap = self.slice(from, *start);
            at = end_of(at, &gap);
            text.push_str(&gap);
            at = end_of(at, piece);
            text.push_str(piece);
            after.push((*is_main, at));
            from = *end;
        }
        self.multi = true;
        self.replace(first, from, &text, EditKind::Other);
        self.multi = false;
        let main_at = after.iter().find(|(m, _)| *m).map_or(after[0].1, |(_, p)| *p);
        self.selection = Selection::cursor(main_at);
        self.extra = after.iter().filter(|(m, _)| !*m).map(|(_, p)| Selection::cursor(*p)).collect();
        let joins = typing
            && self.undo.len() >= 2
            && self.undo[self.undo.len() - 2].multi
            && !self.undo[self.undo.len() - 2].sealed
            && self.undo[self.undo.len() - 2].at.elapsed() < TYPING_GROUP;
        if let Some(edit) = self.undo.last_mut() {
            edit.before = main;
            edit.after = self.selection;
            edit.multi = typing;
            edit.chain = joins;
        }
        true
    }

    /// Runs a move for every cursor (one cursor: just for it); cursors that meet become one.
    pub fn each(&mut self, f: impl Fn(&mut Self)) {
        if self.extra.is_empty() {
            return f(self);
        }
        let extra = std::mem::take(&mut self.extra);
        f(self);
        let (main, goal) = (self.selection, self.goal_column);
        let mut moved = Vec::with_capacity(extra.len());
        for selection in extra {
            self.selection = selection;
            self.goal_column = None;
            f(self);
            moved.push(self.selection);
        }
        self.selection = main;
        self.goal_column = goal;
        self.extra = moved;
        self.merge_cursors();
    }

    /// Cursors on the same place, or selections that overlap, become one.
    fn merge_cursors(&mut self) {
        let main = self.selection;
        let mut all: Vec<(bool, Selection)> = std::iter::once((true, main)).chain(self.extra.iter().map(|s| (false, *s))).collect();
        all.sort_by_key(|(_, s)| s.range());
        let mut merged: Vec<(bool, Selection)> = Vec::with_capacity(all.len());
        for (is_main, s) in all {
            if let Some((last_main, last)) = merged.last_mut() {
                let (ls, le) = last.range();
                let (s0, s1) = s.range();
                if s0 < le || (s0 == le && (s.is_empty() || last.is_empty())) || (s0 == ls && s1 == le) {
                    let end = le.max(s1);
                    *last =
                        if last.anchor <= last.head { Selection { anchor: ls, head: end } } else { Selection { anchor: end, head: ls } };
                    if is_main {
                        *last_main = true;
                    }
                    continue;
                }
            }
            merged.push((is_main, s));
        }
        self.selection = merged.iter().find(|(m, _)| *m).map_or(main, |(_, s)| *s);
        self.extra = merged.into_iter().filter(|(m, _)| !*m).map(|(_, s)| s).collect();
    }

    /// One cursor again (Esc).
    pub fn single_cursor(&mut self) -> bool {
        let had = !self.extra.is_empty();
        self.extra.clear();
        had
    }

    /// ⌥-click: one more cursor at `pos` (or, on a cursor already there, that one goes).
    pub fn toggle_cursor(&mut self, pos: Pos) {
        let pos = self.clamp(pos);
        if let Some(i) = self.extra.iter().position(|s| s.is_empty() && s.head == pos) {
            self.extra.remove(i);
            return;
        }
        if self.selection.is_empty() && self.selection.head == pos {
            if let Some(next) = self.extra.pop() {
                self.selection = next;
            }
            return;
        }
        self.extra.push(self.selection);
        self.selection = Selection::cursor(pos);
        self.goal_column = None;
        self.seal();
        self.merge_cursors();
    }

    /// ⌘D: the word at the cursor, then again the next place with the selected text gets a
    /// cursor of its own (round the end of the file). False when there is no other place.
    pub fn add_next_occurrence(&mut self) -> bool {
        if self.selection.is_empty() {
            let cursor = self.cursor();
            let line = self.line(cursor.line);
            let word = |c: char| c.is_alphanumeric() || c == '_';
            let start = line[..cursor.col].char_indices().rev().take_while(|(_, c)| word(*c)).last().map_or(cursor.col, |(i, _)| i);
            let end = cursor.col
                + line[cursor.col..].char_indices().take_while(|(_, c)| word(*c)).map(|(i, c)| i + c.len_utf8()).last().unwrap_or(0);
            if start == end {
                return false;
            }
            self.selection = Selection { anchor: Pos::new(cursor.line, start), head: Pos::new(cursor.line, end) };
            self.seal();
            return true;
        }
        let (start, end) = self.selection.range();
        let needle = self.slice(start, end);
        let text = self.text();
        let taken: Vec<usize> = self.selections().iter().map(|s| self.offset_of(s.range().0)).collect();
        let after = self.offset_of(end);
        let found = text[after..]
            .match_indices(needle.as_str())
            .map(|(i, _)| after + i)
            .chain(text[..after].match_indices(needle.as_str()).map(|(i, _)| i))
            .find(|i| !taken.contains(i));
        let Some(at) = found else { return false };
        let (from, to) = (self.pos_of(at), self.pos_of(at + needle.len()));
        self.extra.push(self.selection);
        self.selection = Selection { anchor: from, head: to };
        self.goal_column = None;
        self.seal();
        true
    }

    /// ⇧⌘L: a cursor on every place with the selected text (the word at the cursor first), the
    /// first [`MAX_CURSORS`] of them.
    pub fn select_all_occurrences(&mut self) {
        if self.selection.is_empty() && !self.add_next_occurrence() {
            return;
        }
        let (start, end) = self.selection.range();
        let needle = self.slice(start, end);
        if needle.is_empty() {
            return;
        }
        let main = self.offset_of(start);
        let text = self.text();
        let mut cursors = Vec::new();
        // Positions found in order: offsets turn into lines as the scan goes, not from the top each time.
        let (mut line, mut line_start) = (0usize, 0usize);
        for (at, _) in text.match_indices(needle.as_str()).take(MAX_CURSORS) {
            while line + 1 < self.lines.len() && line_start + self.lines[line].len() < at {
                line_start += self.lines[line].len() + 1;
                line += 1;
            }
            let from = Pos::new(line, at - line_start);
            let to = end_of(from, &needle);
            if at != main {
                cursors.push(Selection { anchor: from, head: to });
            }
        }
        self.extra = cursors;
        self.goal_column = None;
        self.seal();
    }

    /// Byte offset of `pos` in the whole text.
    fn offset_of(&self, pos: Pos) -> usize {
        self.lines[..pos.line].iter().map(|l| l.len() + 1).sum::<usize>() + pos.col
    }

    /// The position at byte offset `offset` of the whole text.
    fn pos_of(&self, mut offset: usize) -> Pos {
        for (i, line) in self.lines.iter().enumerate() {
            if offset <= line.len() {
                return Pos::new(i, offset);
            }
            offset -= line.len() + 1;
        }
        self.end()
    }

    pub fn slice(&self, start: Pos, end: Pos) -> String {
        if start.line == end.line {
            return self.line(start.line)[start.col..end.col].to_string();
        }
        let mut out = self.line(start.line)[start.col..].to_string();
        for line in start.line + 1..end.line {
            out.push('\n');
            out.push_str(self.line(line));
        }
        out.push('\n');
        out.push_str(&self.line(end.line)[..end.col]);
        out
    }

    /// `pos` moved onto the text: an existing line, a character boundary.
    pub fn clamp(&self, pos: Pos) -> Pos {
        let line = pos.line.min(self.lines.len() - 1);
        let text = self.line(line);
        let mut col = pos.col.min(text.len());
        while !text.is_char_boundary(col) {
            col -= 1;
        }
        Pos { line, col }
    }

    pub fn end(&self) -> Pos {
        let last = self.lines.len() - 1;
        Pos::new(last, self.lines[last].len())
    }

    // -- offsets ----------------------------------------------------------------------------

    /// UTF-16 offset of `pos` in the whole text (what input methods count in).
    pub fn utf16_offset(&self, pos: Pos) -> usize {
        let before: usize = self.lines[..pos.line].iter().map(|l| utf16_len(l) + 1).sum();
        before + utf16_len(&self.line(pos.line)[..pos.col])
    }

    pub fn pos_at_utf16(&self, mut offset: usize) -> Pos {
        for (index, line) in self.lines.iter().enumerate() {
            let len = utf16_len(line);
            if offset <= len {
                let mut units = 0;
                for (byte, ch) in line.char_indices() {
                    if units >= offset {
                        return Pos::new(index, byte);
                    }
                    units += ch.len_utf16();
                }
                return Pos::new(index, line.len());
            }
            offset -= len + 1;
        }
        self.end()
    }

    // -- editing ----------------------------------------------------------------------------

    /// Replaces `start..end` with `text`; the cursor lands after it. The one place text changes.
    pub fn replace(&mut self, start: Pos, end: Pos, text: &str, kind: EditKind) -> Pos {
        if self.composition.is_some() {
            self.unmark();
        }
        if !self.multi {
            self.extra.clear();
        }
        let text = normalize_newlines(text);
        let before = self.selection;
        let removed = self.slice(start, end);
        let after_pos = self.splice(start, end, &text);
        self.selection = Selection::cursor(after_pos);
        self.goal_column = None;
        self.redo.clear();
        self.record(Edit {
            id: 0,
            start,
            removed,
            inserted: text.into_owned(),
            before,
            after: self.selection,
            kind,
            at: Instant::now(),
            sealed: false,
            multi: false,
            chain: false,
        });
        after_pos
    }

    fn splice(&mut self, start: Pos, end: Pos, text: &str) -> Pos {
        let tail = self.lines[end.line][end.col..].to_string();
        let head = self.lines[start.line][..start.col].to_string();
        let mut new_lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let last = new_lines.len() - 1;
        let end_pos = Pos::new(start.line + last, if last == 0 { head.len() } else { 0 } + new_lines[last].len());
        new_lines[0].insert_str(0, &head);
        new_lines[last].push_str(&tail);
        let delta = new_lines.len() as isize - (end.line - start.line + 1) as isize;
        self.lines.splice(start.line..=end.line, new_lines);
        self.note_change(Splice { from: start.line, to: end_pos.line, delta });
        self.revision = next_revision();
        end_pos
    }

    fn record(&mut self, mut edit: Edit) {
        if let Some(last) = self.undo.last_mut().filter(|last| joins(last, &edit)) {
            if edit.kind == EditKind::Typing {
                last.inserted.push_str(&edit.inserted);
            } else if edit.start < last.start {
                // Backspace: the new character sits before the ones already removed.
                last.removed.insert_str(0, &edit.removed);
                last.start = edit.start;
            } else {
                // Forward delete: the text after moved up into place.
                last.removed.push_str(&edit.removed);
            }
            last.after = edit.after;
            last.at = edit.at;
            // The text differs from the one saved at this id now.
            if last.id == self.saved_at {
                self.next_id += 1;
                last.id = self.next_id;
            }
            return;
        }
        self.next_id += 1;
        edit.id = self.next_id;
        self.undo.push(edit);
        if self.undo.len() > UNDO_LIMIT {
            let dropped = self.undo.remove(0);
            // The saved text can't be reached by undoing any more: it never counts as clean again.
            if self.saved_at == 0 || self.saved_at == dropped.id {
                self.saved_at = u64::MAX;
            }
        }
    }

    /// Types `text` over the selection.
    pub fn insert(&mut self, text: &str) {
        if self.edit_all(text.chars().count() == 1 && text != "\n", |_, s| {
            let (start, end) = s.range();
            (start, end, text.to_string())
        }) {
            return;
        }
        let (start, end) = self.selection.range();
        let kind = if text.contains('\n') || text.chars().count() > 1 { EditKind::Other } else { EditKind::Typing };
        self.replace(start, end, text, kind);
    }

    /// Pastes (never joined with typing).
    pub fn paste(&mut self, text: &str) {
        // As many lines as cursors: a line for each (what copying them gave), else all for each.
        let count = self.extra.len() + 1;
        let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
        if count > 1 {
            let mut order: Vec<Selection> = self.selections();
            order.sort_by_key(|s| s.range().0);
            let piece = |s: Selection| match order.iter().position(|o| *o == s) {
                Some(i) if lines.len() == count => lines[i].to_string(),
                _ => text.to_string(),
            };
            if self.edit_all(false, |_, s| {
                let (start, end) = s.range();
                (start, end, piece(s))
            }) {
                return;
            }
        }
        let (start, end) = self.selection.range();
        self.replace(start, end, text, EditKind::Other);
    }

    pub fn delete_selection(&mut self) -> bool {
        if !self.extra.is_empty() && self.selections().iter().any(|s| !s.is_empty()) {
            return self.edit_all(false, |_, s| {
                let (start, end) = s.range();
                (start, end, String::new())
            });
        }
        let (start, end) = self.selection.range();
        if start == end {
            return false;
        }
        self.replace(start, end, "", EditKind::Other);
        true
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.edit_all(true, |b, s| (b.previous(s.head, false), s.head, String::new())) {
            return;
        }
        let cursor = self.cursor();
        let line = self.line(cursor.line);
        // In the indentation, one step back to the previous indentation level.
        let leading = &line[..cursor.col];
        let start = if !leading.is_empty() && leading.chars().all(|c| c == ' ') && self.indent != "\t" {
            let width = self.indent.len().max(1);
            let target = (cursor.col - 1) / width * width;
            Pos::new(cursor.line, target)
        } else {
            self.previous(cursor, false)
        };
        if start != cursor {
            self.replace(start, cursor, "", EditKind::Deleting);
        }
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.edit_all(true, |b, s| (s.head, b.next(s.head, false), String::new())) {
            return;
        }
        let cursor = self.cursor();
        let end = self.next(cursor, false);
        if end != cursor {
            self.replace(cursor, end, "", EditKind::Deleting);
            self.selection = Selection::cursor(cursor);
        }
    }

    /// Deletes to the end of the word after the cursor (⌥⌦).
    pub fn delete_word_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.edit_all(false, |b, s| (s.head, b.next(s.head, true), String::new())) {
            return;
        }
        let cursor = self.cursor();
        let end = self.next(cursor, true);
        if end != cursor {
            self.replace(cursor, end, "", EditKind::Other);
            self.selection = Selection::cursor(cursor);
        }
    }

    /// Deletes back to where the line's text starts, then to the start of the line, then the line
    /// break before it (⌘⌫).
    pub fn delete_to_line_start(&mut self) {
        if self.delete_selection() {
            return;
        }
        let cursor = self.cursor();
        let line = self.line(cursor.line);
        let first = line.len() - line.trim_start_matches([' ', '\t']).len();
        let start = match cursor.col {
            0 if cursor.line > 0 => Pos::new(cursor.line - 1, self.line(cursor.line - 1).len()),
            0 => return,
            col if col > first => Pos::new(cursor.line, first),
            _ => Pos::new(cursor.line, 0),
        };
        self.replace(start, cursor, "", EditKind::Other);
    }

    /// Selects the cursor's line, break included; again, the next line too (⌘L).
    pub fn select_line(&mut self) {
        let (start, end) = self.selection.range();
        // Whole lines selected already end at the start of the next one: that one comes next.
        let to = end.line + 1;
        let from = start.line;
        let head = if to < self.line_count() { Pos::new(to, 0) } else { Pos::new(to - 1, self.line(to - 1).len()) };
        self.selection = Selection { anchor: Pos::new(from, 0), head };
        self.goal_column = None;
        self.seal();
    }

    /// ⌘]: one step of indentation more on every line of the selection (empty lines stay empty).
    pub fn indent_lines(&mut self) {
        let (start, end) = self.selection.range();
        let unit = self.indent.clone();
        self.map_lines(start.line, end.line, |line| if line.is_empty() { line.to_string() } else { format!("{unit}{line}") });
    }

    /// A new line above the cursor's, indented like it, the cursor on it (⇧⌘↩).
    pub fn insert_line_above(&mut self) {
        let line = self.cursor().line;
        let indent: String = self.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        self.replace(Pos::new(line, 0), Pos::new(line, 0), &format!("{indent}\n"), EditKind::Other);
        self.selection = Selection::cursor(Pos::new(line, indent.len()));
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
        }
    }

    /// A new line under the cursor's, indented like it, the cursor on it (⌘↩).
    pub fn insert_line_below(&mut self) {
        let line = self.cursor().line;
        self.move_to(Pos::new(line, self.line(line).len()), false);
        self.newline();
    }

    /// Typing a closing bracket on a line that is only indentation so far takes one step of
    /// indentation away (the bracket lines up with the line that opened it).
    fn outdent_closing(&mut self) {
        let cursor = self.cursor();
        let before = &self.line(cursor.line)[..cursor.col];
        if before.is_empty() || !before.chars().all(|c| c == ' ' || c == '\t') {
            return;
        }
        let remove = if before.ends_with('\t') { 1 } else { before.len().min(self.indent.len().max(1)) };
        let start = Pos::new(cursor.line, cursor.col - remove);
        self.replace(start, cursor, "", EditKind::Typing);
    }

    /// Deletes to the start of the word before the cursor (⌥⌫).
    pub fn delete_word_back(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.edit_all(false, |b, s| (b.previous(s.head, true), s.head, String::new())) {
            return;
        }
        let cursor = self.cursor();
        let start = self.previous(cursor, true);
        if start != cursor {
            self.replace(start, cursor, "", EditKind::Other);
        }
    }

    /// Enter: a new line indented like this one, one level deeper after an opening bracket.
    pub fn newline(&mut self) {
        if self.edit_all(false, |b, s| {
            let (start, end) = s.range();
            let indent: String = b.line(start.line).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            (start, end, format!("\n{indent}"))
        }) {
            return;
        }
        let (start, end) = self.selection.range();
        let line = self.line(start.line);
        let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let before = line[..start.col].trim_end();
        let after = self.line(end.line)[end.col..].trim_start();
        let opens = before.ends_with(['{', '[', '(']) || (before.ends_with(':') && self.indent != "\t" && !before.contains("//"));
        let opens = opens && !before.ends_with("::");
        let closes = after.starts_with(['}', ']', ')']);
        if opens && closes {
            let inner = format!("\n{indent}{}", self.indent);
            let text = format!("{inner}\n{indent}");
            self.replace(start, end, &text, EditKind::Other);
            let line = start.line + 1;
            self.selection = Selection::cursor(Pos::new(line, self.line(line).len()));
            if let Some(edit) = self.undo.last_mut() {
                edit.after = self.selection;
            }
        } else if opens {
            let text = format!("\n{indent}{}", self.indent);
            self.replace(start, end, &text, EditKind::Other);
        } else {
            self.replace(start, end, &format!("\n{indent}"), EditKind::Other);
        }
    }

    /// Tab: indents the selected lines, or inserts one indentation step at the cursor.
    pub fn indent(&mut self) {
        let unit = self.indent.clone();
        if self.selections().iter().all(|s| s.range().0.line == s.range().1.line)
            && self.edit_all(false, |_, s| {
                let (start, end) = s.range();
                (start, end, unit.clone())
            })
        {
            return;
        }
        let (start, end) = self.selection.range();
        if start.line == end.line {
            let unit = if self.indent == "\t" {
                "\t".to_string()
            } else {
                // Spaces up to the next step.
                let width = self.indent.len().max(1);
                let column = display_column(self.line(start.line), start.col);
                " ".repeat(width - column % width)
            };
            self.replace(start, end, &unit, EditKind::Other);
            return;
        }
        let unit = self.indent.clone();
        self.map_lines(start.line, end.line, |line| if line.is_empty() { line.to_string() } else { format!("{unit}{line}") });
    }

    /// ⇧Tab: removes one indentation step from the selected lines.
    pub fn outdent(&mut self) {
        let (start, end) = self.selection.range();
        let width = self.indent.len().max(1);
        self.map_lines(start.line, end.line, |line| {
            if let Some(rest) = line.strip_prefix('\t') {
                return rest.to_string();
            }
            let spaces = line.chars().take_while(|c| *c == ' ').count().min(width);
            line[spaces..].to_string()
        });
    }

    /// Rewrites whole lines `first..=last` as one undo step, keeping the selection on them.
    fn map_lines(&mut self, first: usize, last: usize, f: impl Fn(&str) -> String) {
        let old: Vec<String> = self.lines[first..=last].to_vec();
        let new: Vec<String> = old.iter().map(|l| f(l)).collect();
        if old == new {
            return;
        }
        let selection = self.selection;
        let shift = |pos: Pos| {
            if pos.line < first || pos.line > last {
                return pos;
            }
            let index = pos.line - first;
            let delta = new[index].len() as isize - old[index].len() as isize;
            Pos::new(pos.line, (pos.col as isize + delta).clamp(0, new[index].len() as isize) as usize)
        };
        let end = Pos::new(last, self.lines[last].len());
        self.replace(Pos::new(first, 0), end, &new.join("\n"), EditKind::Other);
        self.selection = Selection { anchor: self.clamp(shift(selection.anchor)), head: self.clamp(shift(selection.head)) };
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
        }
    }

    // -- whole lines (⌘/, ⌥↑↓, ⇧⌥↑↓, ⇧⌘K) ----------------------------------------------------

    /// The lines the selection covers: a selection ending at the start of a line leaves that
    /// line out (as editors do).
    fn selected_lines(&self) -> (usize, usize) {
        let (start, end) = self.selection.range();
        let last = if end.line > start.line && end.col == 0 { end.line - 1 } else { end.line };
        (start.line, last)
    }

    /// ⌘/: comments the selected lines out with `open` (and `close` for block comments), or back
    /// in when they all are. The marks go at the shallowest indentation, so the code stays
    /// aligned; blank lines are left alone.
    pub fn toggle_comment(&mut self, open: &str, close: &str) {
        let (first, last) = self.selected_lines();
        let lines = &self.lines[first..=last];
        let filled: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        if filled.is_empty() {
            return;
        }
        let commented = |line: &str| {
            let body = line.trim();
            body.starts_with(open) && (close.is_empty() || body.ends_with(close))
        };
        if filled.iter().all(|l| commented(l)) {
            let (open, close) = (open.to_string(), close.to_string());
            self.map_lines(first, last, |line| {
                if line.trim().is_empty() {
                    return line.to_string();
                }
                let indent = line.len() - line.trim_start().len();
                let Some(mut body) = line.get(indent + open.len()..) else { return line.to_string() };
                body = body.strip_prefix(' ').unwrap_or(body);
                if !close.is_empty() {
                    body = body.trim_end();
                    body = body.strip_suffix(close.as_str()).unwrap_or(body);
                    body = body.strip_suffix(' ').unwrap_or(body);
                }
                format!("{}{body}", &line[..indent])
            });
        } else {
            let indent = |l: &str| l.len() - l.trim_start_matches([' ', '\t']).len();
            let column = filled.iter().map(|l| indent(l)).min().unwrap_or(0);
            let (open, close) = (open.to_string(), close.to_string());
            self.map_lines(first, last, |line| {
                if line.trim().is_empty() {
                    return line.to_string();
                }
                let tail = if close.is_empty() { String::new() } else { format!(" {close}") };
                format!("{}{open} {}{tail}", &line[..column], &line[column..])
            });
        }
    }

    /// ⌥↑ / ⌥↓: the selected lines swap places with the line above or below, selection and all.
    pub fn move_lines(&mut self, down: bool) {
        let (first, last) = self.selected_lines();
        if (down && last + 1 >= self.lines.len()) || (!down && first == 0) {
            return;
        }
        let (from, to) = if down { (first, last + 1) } else { (first - 1, last) };
        let mut block: Vec<String> = self.lines[from..=to].to_vec();
        if down {
            block.rotate_right(1);
        } else {
            block.rotate_left(1);
        }
        let selection = self.selection;
        let end = Pos::new(to, self.lines[to].len());
        self.replace(Pos::new(from, 0), end, &block.join("\n"), EditKind::Other);
        let shift = |pos: Pos| Pos::new(if down { pos.line + 1 } else { pos.line - 1 }, pos.col);
        self.selection = Selection { anchor: self.clamp(shift(selection.anchor)), head: self.clamp(shift(selection.head)) };
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
        }
    }

    /// ⇧⌥↓ / ⇧⌥↑: a copy of the selected lines below them (the selection moves onto the copy)
    /// or above them (it stays on the lines it was on, now the lower ones).
    pub fn duplicate_lines(&mut self, down: bool) {
        let (first, last) = self.selected_lines();
        let block = self.lines[first..=last].join("\n");
        let selection = self.selection;
        let end = Pos::new(last, self.lines[last].len());
        self.replace(end, end, &format!("\n{block}"), EditKind::Other);
        let count = last - first + 1;
        let shift = |pos: Pos| if down { Pos::new(pos.line + count, pos.col) } else { pos };
        self.selection = Selection { anchor: self.clamp(shift(selection.anchor)), head: self.clamp(shift(selection.head)) };
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
        }
    }

    /// ⇧⌘K: the selected lines go, the cursor staying in its column on the line that follows.
    pub fn delete_lines(&mut self) {
        let (first, last) = self.selected_lines();
        let column = self.cursor().col;
        let (start, end) = if last + 1 < self.lines.len() {
            (Pos::new(first, 0), Pos::new(last + 1, 0))
        } else if first > 0 {
            (Pos::new(first - 1, self.lines[first - 1].len()), Pos::new(last, self.lines[last].len()))
        } else {
            (Pos::new(0, 0), Pos::new(last, self.lines[last].len()))
        };
        self.replace(start, end, "", EditKind::Other);
        let line = first.min(self.lines.len() - 1);
        let at = self.clamp(Pos::new(line, column.min(self.lines[line].len())));
        self.selection = Selection::cursor(at);
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
        }
    }

    // -- brackets and quotes typed in pairs ----------------------------------------------------

    /// Typing `c` at the cursor with its pair: an opening bracket or quote gets its closing one
    /// (around the selection when there is one), a closing one typed over the same character
    /// steps past it. False when `c` is typed as it is (`quotes`: whether quotes pair here).
    pub fn type_paired(&mut self, c: char, quotes: bool) -> bool {
        // Several cursors: the character as typed at each.
        if !self.extra.is_empty() {
            return false;
        }
        let close = match c {
            '(' => ')',
            '[' => ']',
            '{' => '}',
            '"' | '`' if quotes => c,
            '\'' if quotes => c,
            ')' | ']' | '}' => {
                if self.step_over(c) {
                    return true;
                }
                if self.selection.is_empty() {
                    self.outdent_closing();
                }
                return false;
            }
            _ => return false,
        };
        let (start, end) = self.selection.range();
        if start != end {
            // Around the selection (on one line: across lines it would rather replace it).
            if start.line != end.line {
                return false;
            }
            let inner = self.slice(start, end);
            self.replace(start, end, &format!("{c}{inner}{close}"), EditKind::Other);
            self.selection =
                Selection { anchor: Pos::new(start.line, start.col + c.len_utf8()), head: Pos::new(end.line, end.col + c.len_utf8()) };
            return true;
        }
        let line = self.line(start.line);
        let next = line[start.col..].chars().next();
        let prev = line[..start.col].chars().next_back();
        if c == close {
            if next == Some(c) {
                return self.step_over(c);
            }
            // A quote right after a letter is an apostrophe (`don't`), not a string.
            if prev.is_some_and(|p| p.is_alphanumeric() || p == c) {
                return false;
            }
        }
        // Only where nothing would end up inside by surprise: before a blank, a closer or the end.
        if !next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ',' | ';' | ':')) {
            return false;
        }
        self.replace(start, start, &format!("{c}{close}"), EditKind::Typing);
        self.selection = Selection::cursor(Pos::new(start.line, start.col + c.len_utf8()));
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
        }
        true
    }

    /// The cursor steps over the `c` right after it instead of typing another.
    fn step_over(&mut self, c: char) -> bool {
        let cursor = self.cursor();
        if !self.selection.is_empty() || !self.line(cursor.line)[cursor.col..].starts_with(c) {
            return false;
        }
        self.selection = Selection::cursor(Pos::new(cursor.line, cursor.col + c.len_utf8()));
        true
    }

    /// Backspace between an empty pair (`(|)`, `"|"`): both go.
    pub fn backspace_pair(&mut self) -> bool {
        if !self.extra.is_empty() {
            return false;
        }
        let cursor = self.cursor();
        if !self.selection.is_empty() || cursor.col == 0 {
            return false;
        }
        let line = self.line(cursor.line);
        let (Some(before), Some(after)) = (line[..cursor.col].chars().next_back(), line[cursor.col..].chars().next()) else {
            return false;
        };
        if !matches!((before, after), ('(', ')') | ('[', ']') | ('{', '}') | ('"', '"') | ('\'', '\'') | ('`', '`')) {
            return false;
        }
        let start = Pos::new(cursor.line, cursor.col - 1);
        self.replace(start, Pos::new(cursor.line, cursor.col + 1), "", EditKind::Deleting);
        true
    }

    /// The bracket matching the one just before or at the cursor: (that one, its match). Looks
    /// `limit` characters at most.
    pub fn matching_bracket(&self, limit: usize) -> Option<(Pos, Pos)> {
        let cursor = self.cursor();
        let line = self.line(cursor.line);
        let at = |col: usize| line.get(col..).and_then(|s| s.chars().next());
        let before = line[..cursor.col].char_indices().next_back();
        let here = [before, at(cursor.col).map(|c| (cursor.col, c))]
            .into_iter()
            .flatten()
            .find(|(_, c)| matches!(c, '(' | ')' | '[' | ']' | '{' | '}'))?;
        let (col, c) = here;
        let (open, close, forward) = match c {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            _ => ('{', '}', false),
        };
        let mut depth = 0usize;
        let mut seen = 0usize;
        if forward {
            for row in cursor.line..self.lines.len() {
                let text = self.line(row);
                let from = if row == cursor.line { col } else { 0 };
                for (i, ch) in text[from..].char_indices() {
                    seen += 1;
                    if seen > limit {
                        return None;
                    }
                    if ch == open {
                        depth += 1;
                    } else if ch == close {
                        depth -= 1;
                        if depth == 0 {
                            return Some((Pos::new(cursor.line, col), Pos::new(row, from + i)));
                        }
                    }
                }
            }
        } else {
            for row in (0..=cursor.line).rev() {
                let text = self.line(row);
                let to = if row == cursor.line { col + 1 } else { text.len() };
                for (i, ch) in text[..to].char_indices().rev() {
                    seen += 1;
                    if seen > limit {
                        return None;
                    }
                    if ch == close {
                        depth += 1;
                    } else if ch == open {
                        depth -= 1;
                        if depth == 0 {
                            return Some((Pos::new(cursor.line, col), Pos::new(row, i)));
                        }
                    }
                }
            }
        }
        None
    }

    /// Replaces the whole text (formatting, reloading from disk) as one undo step, keeping the
    /// cursor on the same line and column where it still exists.
    pub fn set_text(&mut self, text: &str) {
        let text = normalize_newlines(text);
        if text == self.text() {
            return;
        }
        let selection = self.selection;
        let end = self.end();
        // The lines that really changed (a file an agent edited keeps most of its lines): the
        // whole-text splice below is noted as just those.
        let changed = splice_between(&self.lines, &text);
        self.replace(Pos::default(), end, &text, EditKind::Other);
        if let Some(changes) = self.changes.as_mut() {
            changes.pop();
            changes.push(changed);
        }
        self.selection = Selection { anchor: self.clamp(selection.anchor), head: self.clamp(selection.head) };
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
            edit.sealed = true;
        }
        self.revision = next_revision();
    }

    // -- input methods ----------------------------------------------------------------------

    /// The text an input method is still composing.
    pub fn marked(&self) -> Option<(Pos, Pos)> {
        self.composition.as_ref().map(|c| c.marked)
    }

    /// Shows `text` in place of `start..end` as composing text, with the cursor at `selected`
    /// (byte range within `text`). Nothing reaches the undo history until it is committed.
    pub fn compose(&mut self, start: Pos, end: Pos, text: &str, selected: Option<std::ops::Range<usize>>) {
        if self.composition.as_ref().is_some_and(|c| c.marked != (start, end)) {
            self.unmark();
        }
        let text = normalize_newlines(text);
        let composition = match self.composition.take() {
            Some(composition) => composition,
            None => Composition { start, removed: self.slice(start, end), before: self.selection, marked: (start, end) },
        };
        let marked_end = self.splice(start, end, &text);
        self.shift_extra(end, marked_end);
        let cursor = |offset: usize| end_of(start, &text[..offset.min(text.len())]);
        self.selection =
            match selected.filter(|r| text.is_char_boundary(r.start.min(text.len())) && text.is_char_boundary(r.end.min(text.len()))) {
                Some(range) => Selection { anchor: cursor(range.start), head: cursor(range.end) },
                None => Selection::cursor(marked_end),
            };
        self.goal_column = None;
        self.redo.clear();
        self.composition = (!text.is_empty()).then_some(Composition { marked: (start, marked_end), ..composition.clone() });
        if text.is_empty() {
            // Composing was cancelled: whatever was replaced is gone, as an ordinary edit.
            self.finish(composition, start);
        }
    }

    /// Replaces `start..end` with the final `text`. When that is the composing text, the whole
    /// composition becomes one edit.
    pub fn commit(&mut self, start: Pos, end: Pos, text: &str) {
        match self.composition.take() {
            Some(composition) if composition.marked == (start, end) => {
                let text = normalize_newlines(text);
                let new_end = self.splice(start, end, &text);
                self.shift_extra(end, new_end);
                self.selection = Selection::cursor(new_end);
                let typed = self.slice(composition.start, new_end);
                self.finish(composition, new_end);
                // Composed at the main cursor only: the others get the finished text now.
                self.mirror(&typed);
            }
            Some(composition) => {
                let marked_end = composition.marked.1;
                self.finish(composition, marked_end);
                let (start, end) = (self.clamp(start), self.clamp(end));
                self.replace(start, end, text, if text.chars().count() == 1 { EditKind::Typing } else { EditKind::Other });
            }
            None if !self.extra.is_empty() && self.selection.range() == (start, end) => self.insert(text),
            None => {
                self.replace(start, end, text, if text.chars().count() == 1 && text != "\n" { EditKind::Typing } else { EditKind::Other });
            }
        }
    }

    /// The other cursors after `old_end` on its line follow text there that now ends at `new_end`
    /// (composing text at the main cursor grew or shrank).
    fn shift_extra(&mut self, old_end: Pos, new_end: Pos) {
        let shift =
            |p: Pos| if p.line == old_end.line && p >= old_end { Pos::new(new_end.line, p.col - old_end.col + new_end.col) } else { p };
        for s in &mut self.extra {
            *s = Selection { anchor: shift(s.anchor), head: shift(s.head) };
        }
    }

    /// Types `text` at every cursor but the main one (where it already is).
    fn mirror(&mut self, text: &str) {
        let main = self.selection;
        if text.is_empty() {
            return;
        }
        if !self.edit_all(true, |_, s| {
            if s == main {
                (s.head, s.head, String::new())
            } else {
                let (start, end) = s.range();
                (start, end, text.to_string())
            }
        }) {
            return;
        }
        // The main cursor's composed text and this are one keystroke, and they go on a run of
        // typing at every cursor.
        let n = self.undo.len();
        if n >= 2 {
            let joins = n >= 3 && {
                let before = &self.undo[n - 3];
                before.multi && !before.sealed && before.at.elapsed() < TYPING_GROUP
            };
            self.undo[n - 2].multi = true;
            self.undo[n - 2].chain = joins;
            self.undo[n - 1].chain = true;
        }
    }

    /// Keeps the composing text as it is (the input method gave up on it).
    pub fn unmark(&mut self) {
        if let Some(composition) = self.composition.take() {
            let end = composition.marked.1;
            self.finish(composition, end);
        }
    }

    /// Records a finished composition (the text from its start to `end`) as one edit.
    fn finish(&mut self, composition: Composition, end: Pos) {
        let inserted = self.slice(composition.start, end);
        if inserted.is_empty() && composition.removed.is_empty() {
            return;
        }
        let kind = if composition.removed.is_empty() && inserted.chars().count() == 1 { EditKind::Typing } else { EditKind::Other };
        self.record(Edit {
            id: 0,
            start: composition.start,
            removed: composition.removed,
            inserted,
            before: composition.before,
            after: self.selection,
            kind,
            at: Instant::now(),
            sealed: false,
            multi: false,
            chain: false,
        });
    }

    pub fn undo(&mut self) -> bool {
        self.unmark();
        let Some(edit) = self.undo.pop() else { return false };
        self.extra.clear();
        let end = end_of(edit.start, &edit.inserted);
        self.splice(edit.start, end, &edit.removed);
        self.selection = edit.before;
        self.goal_column = None;
        let chain = edit.chain;
        self.redo.push(edit);
        if chain {
            self.undo();
        }
        true
    }

    pub fn redo(&mut self) -> bool {
        self.unmark();
        let Some(mut edit) = self.redo.pop() else { return false };
        self.extra.clear();
        let end = end_of(edit.start, &edit.removed);
        self.splice(edit.start, end, &edit.inserted);
        self.selection = edit.after;
        self.goal_column = None;
        edit.sealed = true;
        self.undo.push(edit);
        if self.redo.last().is_some_and(|next| next.chain) {
            self.redo();
        }
        true
    }

    // -- moving -----------------------------------------------------------------------------

    /// Moves the cursor to `pos`, extending the selection when `select`.
    pub fn move_to(&mut self, pos: Pos, select: bool) {
        self.extra.clear();
        let pos = self.clamp(pos);
        self.selection = if select { Selection { anchor: self.selection.anchor, head: pos } } else { Selection::cursor(pos) };
        self.goal_column = None;
        self.seal();
    }

    pub fn select_all(&mut self) {
        self.selection = Selection { anchor: Pos::default(), head: self.end() };
        self.seal();
    }

    /// ← (by character, or by word with `word`). Without `select`, a selection collapses to its start.
    pub fn left(&mut self, select: bool, word: bool) {
        if !select && !self.selection.is_empty() {
            let (start, _) = self.selection.range();
            return self.move_to(start, false);
        }
        let pos = self.previous(self.cursor(), word);
        self.move_to(pos, select);
    }

    pub fn right(&mut self, select: bool, word: bool) {
        if !select && !self.selection.is_empty() {
            let (_, end) = self.selection.range();
            return self.move_to(end, false);
        }
        let pos = self.next(self.cursor(), word);
        self.move_to(pos, select);
    }

    /// ↑ / ↓ by `lines` (negative: up), keeping the display column.
    pub fn vertical(&mut self, lines: isize, select: bool) {
        let cursor = self.cursor();
        let goal = self.goal_column.unwrap_or_else(|| display_column(self.line(cursor.line), cursor.col));
        let target = cursor.line as isize + lines;
        let pos = if target < 0 {
            Pos::default()
        } else if target as usize >= self.lines.len() {
            self.end()
        } else {
            let line = target as usize;
            Pos::new(line, column_to_byte(self.line(line), goal))
        };
        self.move_to(pos, select);
        self.goal_column = Some(goal);
    }

    /// Home: to the first non-blank character, or to the line start when already there.
    pub fn home(&mut self, select: bool) {
        let cursor = self.cursor();
        let line = self.line(cursor.line);
        let first = line.len() - line.trim_start().len();
        let col = if cursor.col == first { 0 } else { first };
        self.move_to(Pos::new(cursor.line, col), select);
    }

    pub fn line_end(&mut self, select: bool) {
        let line = self.cursor().line;
        self.move_to(Pos::new(line, self.line(line).len()), select);
    }

    pub fn doc_start(&mut self, select: bool) {
        self.move_to(Pos::default(), select);
    }

    pub fn doc_end(&mut self, select: bool) {
        self.move_to(self.end(), select);
    }

    /// The word around `pos` (a double click): a run of word characters, blanks or punctuation.
    pub fn word_at(&self, pos: Pos) -> (Pos, Pos) {
        let line = self.line(pos.line);
        let Some(class) = line[pos.col..].chars().next().or_else(|| line[..pos.col].chars().next_back()).map(char_class) else {
            return (pos, pos);
        };
        let (mut start, mut end) = (pos.col, pos.col);
        while let Some(c) = line[..start].chars().next_back().filter(|c| char_class(*c) == class) {
            start -= c.len_utf8();
        }
        while let Some(c) = line[end..].chars().next().filter(|c| char_class(*c) == class) {
            end += c.len_utf8();
        }
        (Pos::new(pos.line, start), Pos::new(pos.line, end))
    }

    /// Position before `pos`: one grapheme back (across a line break), or to a word start.
    fn previous(&self, pos: Pos, word: bool) -> Pos {
        if pos.col == 0 {
            return if pos.line == 0 { pos } else { Pos::new(pos.line - 1, self.line(pos.line - 1).len()) };
        }
        let line = self.line(pos.line);
        if !word {
            let col = line[..pos.col].grapheme_indices(true).next_back().map_or(0, |(i, _)| i);
            return Pos::new(pos.line, col);
        }
        let before = &line[..pos.col];
        let trimmed = before.trim_end();
        let mut col = trimmed.len();
        let class = trimmed.chars().next_back().map(char_class);
        while let Some(c) = trimmed[..col].chars().next_back().filter(|c| Some(char_class(*c)) == class) {
            col -= c.len_utf8();
        }
        Pos::new(pos.line, col)
    }

    fn next(&self, pos: Pos, word: bool) -> Pos {
        let line = self.line(pos.line);
        if pos.col >= line.len() {
            return if pos.line + 1 >= self.lines.len() { pos } else { Pos::new(pos.line + 1, 0) };
        }
        if !word {
            let col = line[pos.col..].grapheme_indices(true).nth(1).map_or(line.len(), |(i, _)| pos.col + i);
            return Pos::new(pos.line, col);
        }
        let rest = &line[pos.col..];
        let skipped = rest.len() - rest.trim_start().len();
        let mut col = pos.col + skipped;
        let class = line[col..].chars().next().map(char_class);
        while let Some(c) = line[col..].chars().next().filter(|c| Some(char_class(*c)) == class) {
            col += c.len_utf8();
        }
        Pos::new(pos.line, col)
    }
}

/// Cursors ⇧⌘L makes at most.
const MAX_CURSORS: usize = 1_000;

/// Changes kept for [`Buffer::take_changes`] at most (a replace-all of thousands is everything).
const MAX_CHANGES: usize = 256;

/// One change to the text, by lines: the lines `from..=to` (as they are after it) replace the
/// ones that were there, and the lines after them moved by `delta`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splice {
    pub from: usize,
    pub to: usize,
    pub delta: isize,
}

impl Splice {
    /// Where `line` of the text before the change is after it: moved with the lines around it,
    /// or (a line the change rewrote) the nearest line the change left.
    pub fn line_after(&self, line: usize) -> usize {
        let old_to = (self.to as isize - self.delta).max(self.from as isize) as usize;
        if line < self.from {
            line
        } else if line > old_to {
            (line as isize + self.delta) as usize
        } else {
            line.min(self.to)
        }
    }
}

/// The change from `old` (lines) to `new`: the lines between their common start and end.
fn splice_between(old: &[String], new: &str) -> Splice {
    let new: Vec<&str> = new.split('\n').collect();
    let shorter = old.len().min(new.len());
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a.as_str() == **b).count().min(shorter);
    let suffix = old.iter().rev().zip(new.iter().rev()).take_while(|(a, b)| a.as_str() == **b).count().min(shorter - prefix);
    let delta = new.len() as isize - old.len() as isize;
    // At least the lines added are in the change (`to >= from + delta`).
    let to = (new.len() - suffix).saturating_sub(1).max(prefix + delta.max(0) as usize).min(new.len() - 1);
    Splice { from: prefix.min(new.len() - 1), to, delta }
}

/// The next text revision of the app (see [`Buffer::revision`]).
fn next_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Words, punctuation and blanks: moving by word stops where one kind turns into another.
fn char_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

fn joins(last: &Edit, next: &Edit) -> bool {
    if last.sealed || last.kind != next.kind || next.at.duration_since(last.at) > TYPING_GROUP {
        return false;
    }
    match next.kind {
        EditKind::Typing => {
            last.removed.is_empty()
                && next.removed.is_empty()
                && next.start == end_of(last.start, &last.inserted)
                && !next.inserted.contains('\n')
                // A new word starts a new step, like other editors.
                && !(next.inserted.starts_with(char::is_whitespace) && !last.inserted.ends_with(char::is_whitespace))
        }
        EditKind::Deleting => {
            last.inserted.is_empty()
                && next.inserted.is_empty()
                && (end_of(next.start, &next.removed) == last.start || next.start == last.start)
        }
        EditKind::Other => false,
    }
}

/// Where `text` ends when it is inserted at `start`.
fn end_of(start: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        Some(index) => Pos::new(start.line + text.matches('\n').count(), text.len() - index - 1),
        None => Pos::new(start.line, start.col + text.len()),
    }
}

fn normalize_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n").into()
    } else {
        text.into()
    }
}

pub fn utf16_len(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.chars().map(char::len_utf16).sum()
    }
}

/// Display column of byte `col` in `line`: tabs advance to the next tab stop, wide (CJK)
/// characters take two columns.
pub fn display_column(line: &str, col: usize) -> usize {
    line[..col.min(line.len())].chars().fold(0, |column, c| column + char_columns(c, column))
}

fn char_columns(c: char, column: usize) -> usize {
    match c {
        '\t' => TAB_WIDTH - column % TAB_WIDTH,
        c if is_wide(c) => 2,
        _ => 1,
    }
}

/// East Asian wide characters (Hangul, CJK, full-width forms): the common ranges.
fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD)
}

/// Byte offset in `line` closest to display `column` (never past the line end).
pub fn column_to_byte(line: &str, column: usize) -> usize {
    let mut current = 0;
    for (byte, c) in line.char_indices() {
        let width = char_columns(c, current);
        if current + width > column {
            // Past the middle of a wide character or tab: land after it.
            return if column - current > width / 2 { byte + c.len_utf8() } else { byte };
        }
        current += width;
    }
    line.len()
}

/// The indentation step a file uses: a tab when most indented lines start with one, else the
/// smallest step between space indentations (2 or 4, 4 when nothing says otherwise).
fn detect_indent(lines: &[String]) -> String {
    let (mut tabs, mut spaced) = (0, 0);
    let mut steps = [0usize; 9];
    let mut previous = 0usize;
    for line in lines.iter().take(5000) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let spaces = line.len() - line.trim_start_matches(' ').len();
        if spaces > 0 {
            spaced += 1;
        }
        let step = spaces.abs_diff(previous);
        if (2..=8).contains(&step) {
            steps[step] += 1;
        }
        previous = spaces;
    }
    if tabs > spaced {
        return "\t".into();
    }
    let best = (2..=8).max_by_key(|step| (steps[*step], usize::from(*step == 4))).filter(|step| steps[*step] > 0).unwrap_or(4);
    " ".repeat(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(buffer: &mut Buffer, line: usize, col: usize) {
        buffer.selection = Selection::cursor(Pos::new(line, col));
    }

    #[test]
    fn comments_lines_in_and_out() {
        let mut b = Buffer::new("fn a() {\n    one();\n\n    two();\n}");
        b.selection = Selection { anchor: Pos::new(1, 2), head: Pos::new(3, 2) };
        b.toggle_comment("//", "");
        assert_eq!(b.text(), "fn a() {\n    // one();\n\n    // two();\n}");
        b.toggle_comment("//", "");
        assert_eq!(b.text(), "fn a() {\n    one();\n\n    two();\n}");
        // Mixed lines are commented, at the shallowest indentation.
        let mut b = Buffer::new("  a\n    // b");
        b.selection = Selection { anchor: Pos::new(0, 0), head: Pos::new(1, 3) };
        b.toggle_comment("#", "");
        assert_eq!(b.text(), "  # a\n  #   // b");
        let mut html = Buffer::new("<p>x</p>");
        html.toggle_comment("<!--", "-->");
        assert_eq!(html.text(), "<!-- <p>x</p> -->");
        html.toggle_comment("<!--", "-->");
        assert_eq!(html.text(), "<p>x</p>");
        // A selection ending at a line's start leaves that line alone; one undo takes it back.
        let mut b = Buffer::new("a\nb\nc");
        b.selection = Selection { anchor: Pos::new(0, 0), head: Pos::new(1, 0) };
        b.toggle_comment("//", "");
        assert_eq!(b.text(), "// a\nb\nc");
        b.undo();
        assert_eq!(b.text(), "a\nb\nc");
    }

    #[test]
    fn comments_lines_indented_with_other_spaces() {
        let mut b = Buffer::new(" x\n\u{a0}y\n\u{3000}z");
        b.selection = Selection { anchor: Pos::new(0, 0), head: Pos::new(2, 1) };
        b.toggle_comment("//", "");
        // A no-break space is text, not indentation: the marks go at the start of every line.
        assert_eq!(b.text(), "//  x\n// \u{a0}y\n// \u{3000}z");
        b.toggle_comment("//", "");
        assert_eq!(b.text(), " x\n\u{a0}y\n\u{3000}z");
    }

    #[test]
    fn next_occurrence_gets_a_cursor_and_typing_goes_to_every_one() {
        let mut b = Buffer::new("let count = 1;\ncount += 1;\nprint(count, recount);");
        at(&mut b, 0, 6);
        assert!(b.add_next_occurrence());
        assert_eq!(b.selected_text(), "count");
        assert!(b.add_next_occurrence());
        assert!(b.add_next_occurrence());
        assert!(b.add_next_occurrence(), "inside 'recount' too, as VS Code without whole-word");
        assert!(!b.add_next_occurrence());
        assert_eq!(b.extra.len(), 3);
        b.insert("n");
        assert_eq!(b.text(), "let n = 1;\nn += 1;\nprint(n, ren);");
        b.insert("um");
        assert_eq!(b.text(), "let num = 1;\nnum += 1;\nprint(num, renum);");
        b.backspace();
        assert_eq!(b.text(), "let nu = 1;\nnu += 1;\nprint(nu, renu);");
        // One undo step per change, and back to one cursor.
        assert!(b.undo());
        assert_eq!(b.text(), "let num = 1;\nnum += 1;\nprint(num, renum);");
        assert!(b.extra.is_empty());
    }

    #[test]
    fn every_match_gets_a_cursor_at_once() {
        let mut b = Buffer::new("ab x ab\nab\n\nab ab");
        at(&mut b, 2, 0);
        b.selection = Selection { anchor: Pos::new(1, 0), head: Pos::new(1, 2) };
        b.select_all_occurrences();
        let mut all: Vec<(Pos, Pos)> = b.selections().iter().map(|s| s.range()).collect();
        all.sort();
        let want = [(0, 0), (0, 5), (1, 0), (3, 0), (3, 3)].map(|(l, c)| (Pos::new(l, c), Pos::new(l, c + 2)));
        assert_eq!(all, want);
        assert_eq!(b.selection.range(), (Pos::new(1, 0), Pos::new(1, 2)), "the main cursor stays where it was");
        // A file full of matches: no more than the limit.
        let mut b = Buffer::new(&"a ".repeat(5_000));
        at(&mut b, 0, 0);
        b.select_all_occurrences();
        assert_eq!(b.extra.len() + 1, MAX_CURSORS);
    }

    #[test]
    fn every_cursor_moves_and_cursors_that_meet_become_one() {
        let mut b = Buffer::new("ab\ncd\nef");
        at(&mut b, 0, 1);
        b.toggle_cursor(Pos::new(1, 1));
        b.toggle_cursor(Pos::new(2, 1));
        b.each(|b| b.line_end(false));
        assert_eq!(b.selections().iter().map(|s| s.head).collect::<Vec<_>>(), [Pos::new(2, 2), Pos::new(0, 2), Pos::new(1, 2)]);
        b.insert(";");
        assert_eq!(b.text(), "ab;\ncd;\nef;");
        b.newline();
        assert_eq!(b.text(), "ab;\n\ncd;\n\nef;\n");
        b.each(|b| b.doc_start(false));
        assert!(b.extra.is_empty(), "all at the start: one cursor");
        // Pasting as many lines as cursors: one each.
        let mut b = Buffer::new("x\ny");
        at(&mut b, 0, 1);
        b.toggle_cursor(Pos::new(1, 1));
        b.paste("1\n2");
        assert_eq!(b.text(), "x1\ny2");
        // Any other edit leaves the main cursor alone.
        b.toggle_comment("//", "");
        assert!(b.extra.is_empty());
    }

    #[test]
    fn composed_text_reaches_every_cursor() {
        let mut b = Buffer::new("a\nb");
        at(&mut b, 0, 1);
        b.toggle_cursor(Pos::new(1, 1));
        // An input method composes at the main cursor (line 1), then commits.
        b.compose(Pos::new(1, 1), Pos::new(1, 1), "ㅎ", None);
        b.compose(Pos::new(1, 1), Pos::new(1, 4), "하", None);
        b.commit(Pos::new(1, 1), Pos::new(1, 4), "한");
        assert_eq!(b.text(), "a한\nb한");
        assert_eq!(b.extra.len(), 1);
        // Plain typing through the input path too.
        let (s, e) = b.selection.range();
        b.commit(s, e, "!");
        assert_eq!(b.text(), "a한!\nb한!");
        // Undone step by step back to the start.
        while b.undo() {}
        assert_eq!(b.text(), "a\nb");
    }

    #[test]
    fn undo_after_several_cursors_gives_the_text_back() {
        let original = "items x\nitems y\nitems z";
        let mut b = Buffer::new(original);
        at(&mut b, 0, 2);
        for _ in 0..3 {
            b.add_next_occurrence();
        }
        b.insert("list");
        b.backspace();
        let at_main = b.cursor();
        b.compose(at_main, at_main, "ㅎ", None);
        b.compose(at_main, Pos::new(at_main.line, at_main.col + 3), "하", None);
        b.commit(at_main, Pos::new(at_main.line, at_main.col + 3), "한");
        assert_eq!(b.text(), "lis한 x\nlis한 y\nlis한 z");
        // Typed at every cursor in a run: one undo step back to before the run.
        assert!(b.undo());
        assert_eq!(b.text(), "list x\nlist y\nlist z");
        assert!(b.undo());
        assert_eq!(b.text(), original);
        assert!(b.redo());
        assert_eq!(b.text(), "list x\nlist y\nlist z");
    }

    #[test]
    fn bracket_keys_indent_and_a_line_goes_above() {
        let mut b = Buffer::new("a\n\nb");
        b.selection = Selection { anchor: Pos::new(0, 0), head: Pos::new(2, 1) };
        b.indent_lines();
        assert_eq!(b.text(), "    a\n\n    b");
        b.outdent();
        assert_eq!(b.text(), "a\n\nb");
        let mut b = Buffer::new("  x");
        at(&mut b, 0, 3);
        b.insert_line_above();
        assert_eq!(b.text(), "  \n  x");
        assert_eq!(b.cursor(), Pos::new(0, 2));
    }

    #[test]
    fn line_keys_select_insert_and_delete() {
        let mut b = Buffer::new("fn a() {\n    let x = 1;\n}");
        let at = |b: &mut Buffer, line, col| b.selection = Selection { anchor: Pos::new(line, col), head: Pos::new(line, col) };
        at(&mut b, 1, 6);
        b.select_line();
        assert_eq!(b.selection.range(), (Pos::new(1, 0), Pos::new(2, 0)));
        b.select_line();
        assert_eq!(b.selection.range(), (Pos::new(1, 0), Pos::new(2, 1)), "the last line has no break");
        // ⌘↩ from the middle of a line: a line under it, indented alike.
        at(&mut b, 1, 6);
        b.insert_line_below();
        assert_eq!(b.text(), "fn a() {\n    let x = 1;\n    \n}");
        assert_eq!(b.cursor(), Pos::new(2, 4));
        // A closing bracket typed on the indentation goes one step left.
        assert!(!b.type_paired('}', true));
        b.insert("}");
        assert_eq!(b.line(2), "}");
        // ⌘⌫: to the text's start, to the line's start, then the break before.
        let mut b = Buffer::new("a\n    foo bar");
        at(&mut b, 1, 11);
        b.delete_to_line_start();
        assert_eq!(b.text(), "a\n    ");
        b.delete_to_line_start();
        assert_eq!(b.text(), "a\n");
        b.delete_to_line_start();
        assert_eq!(b.text(), "a");
        // ⌥⌦: the word after the cursor.
        let mut b = Buffer::new("foo bar baz");
        at(&mut b, 0, 3);
        b.delete_word_forward();
        assert_eq!(b.text(), "foo baz");
    }

    #[test]
    fn notes_the_lines_each_change_touched() {
        let mut b = Buffer::new("a\nb\nc\nd");
        b.selection = Selection { anchor: Pos::new(1, 1), head: Pos::new(1, 1) };
        b.insert("\nx");
        assert_eq!(b.take_changes(), Some(vec![Splice { from: 1, to: 2, delta: 1 }]));
        b.selection = Selection { anchor: Pos::new(1, 1), head: Pos::new(3, 0) };
        b.insert("");
        assert_eq!(b.take_changes(), Some(vec![Splice { from: 1, to: 1, delta: -2 }]));
        // A whole new text keeps the lines that stayed out of the change.
        b.set_text("a\nB\nB2\nd");
        assert_eq!(b.text(), "a\nB\nB2\nd");
        assert_eq!(b.take_changes(), Some(vec![Splice { from: 1, to: 2, delta: 1 }]));
        b.set_text("a\nd");
        assert_eq!(b.take_changes(), Some(vec![Splice { from: 1, to: 1, delta: -2 }]));
        b.set_text("a\nnew\nd");
        assert_eq!(b.take_changes(), Some(vec![Splice { from: 1, to: 2, delta: 1 }]));
        assert_eq!(b.take_changes(), Some(vec![]));
        let enter = Splice { from: 1, to: 2, delta: 1 };
        assert_eq!([0, 1, 2, 5].map(|l| enter.line_after(l)), [0, 1, 3, 6]);
        let joined = Splice { from: 1, to: 1, delta: -2 };
        assert_eq!([0, 1, 2, 3, 4].map(|l| joined.line_after(l)), [0, 1, 1, 1, 2]);
    }

    #[test]
    fn revisions_never_repeat_across_buffers() {
        let (mut a, b) = (Buffer::new("a"), Buffer::new("b"));
        let before = b.revision();
        a.insert("x");
        assert!(a.revision() > before && b.revision() != a.revision());
    }

    #[test]
    fn moves_duplicates_and_deletes_lines() {
        let mut b = Buffer::new("one\ntwo\nthree");
        at(&mut b, 0, 1);
        b.move_lines(true);
        assert_eq!((b.text().as_str(), b.cursor()), ("two\none\nthree", Pos::new(1, 1)));
        b.move_lines(true);
        b.move_lines(true);
        assert_eq!(b.text(), "two\nthree\none", "the last line stays last");
        b.move_lines(false);
        assert_eq!((b.text().as_str(), b.cursor()), ("two\none\nthree", Pos::new(1, 1)));
        b.duplicate_lines(true);
        assert_eq!((b.text().as_str(), b.cursor()), ("two\none\none\nthree", Pos::new(2, 1)));
        b.duplicate_lines(false);
        assert_eq!((b.text().as_str(), b.cursor()), ("two\none\none\none\nthree", Pos::new(2, 1)));
        b.delete_lines();
        assert_eq!((b.text().as_str(), b.cursor()), ("two\none\none\nthree", Pos::new(2, 1)));
        at(&mut b, 3, 2);
        b.delete_lines();
        assert_eq!((b.text().as_str(), b.cursor()), ("two\none\none", Pos::new(2, 2)));
        let mut single = Buffer::new("only");
        single.delete_lines();
        assert_eq!(single.text(), "");
    }

    #[test]
    fn brackets_and_quotes_come_in_pairs() {
        let mut b = Buffer::new("f");
        at(&mut b, 0, 1);
        assert!(b.type_paired('(', true));
        assert_eq!((b.text().as_str(), b.cursor()), ("f()", Pos::new(0, 2)));
        assert!(b.type_paired('"', true));
        assert_eq!(b.text(), "f(\"\")");
        assert!(b.type_paired('"', true), "typed over the closing quote");
        assert_eq!((b.text().as_str(), b.cursor()), ("f(\"\")", Pos::new(0, 4)));
        assert!(b.type_paired(')', true));
        assert_eq!(b.cursor(), Pos::new(0, 5));
        // Before a word nothing is added; an apostrophe stays one.
        let mut w = Buffer::new("word");
        at(&mut w, 0, 0);
        assert!(!w.type_paired('(', true));
        let mut d = Buffer::new("don");
        at(&mut d, 0, 3);
        assert!(!d.type_paired('\'', true));
        assert!(!d.type_paired('\'', false));
        // Around a selection.
        let mut s = Buffer::new("a total b");
        s.selection = Selection { anchor: Pos::new(0, 2), head: Pos::new(0, 7) };
        assert!(s.type_paired('[', true));
        assert_eq!(s.text(), "a [total] b");
        // Backspace in an empty pair takes both.
        let mut p = Buffer::new("x()");
        at(&mut p, 0, 2);
        assert!(p.backspace_pair());
        assert_eq!(p.text(), "x");
    }

    #[test]
    fn finds_the_matching_bracket() {
        let mut b = Buffer::new("fn a() {\n    if (x[0]) { y(); }\n}");
        at(&mut b, 0, 8);
        assert_eq!(b.matching_bracket(10_000), Some((Pos::new(0, 7), Pos::new(2, 0))));
        at(&mut b, 1, 13);
        assert_eq!(b.matching_bracket(10_000), Some((Pos::new(1, 12), Pos::new(1, 7))));
        at(&mut b, 1, 1);
        assert_eq!(b.matching_bracket(10_000), None);
        at(&mut b, 0, 8);
        assert_eq!(b.matching_bracket(5), None, "gives up past the limit");
    }

    fn typed(buffer: &mut Buffer, text: &str) {
        for c in text.chars() {
            if c == '\n' {
                buffer.newline();
            } else {
                buffer.insert(&c.to_string());
            }
        }
    }

    #[test]
    fn text_round_trips_through_lines() {
        for text in ["", "a", "a\nb", "a\nb\n", "\n\n", "한글\n"] {
            let buffer = Buffer::new(text);
            assert_eq!(buffer.text(), text);
        }
    }

    #[test]
    fn inserting_and_deleting() {
        let mut buffer = Buffer::new("hello\nworld");
        buffer.move_to(Pos::new(0, 5), false);
        buffer.insert("!");
        assert_eq!(buffer.text(), "hello!\nworld");
        buffer.paste(" there\nbig");
        assert_eq!(buffer.text(), "hello! there\nbig\nworld");
        assert_eq!(buffer.cursor(), Pos::new(1, 3));
        buffer.backspace();
        assert_eq!(buffer.text(), "hello! there\nbi\nworld");
        // Backspace at a line start joins the lines.
        buffer.move_to(Pos::new(2, 0), false);
        buffer.backspace();
        assert_eq!(buffer.text(), "hello! there\nbiworld");
        assert_eq!(buffer.cursor(), Pos::new(1, 2));
        buffer.delete_forward();
        assert_eq!(buffer.text(), "hello! there\nbiorld");
        // A selection is replaced by what is typed.
        buffer.move_to(Pos::new(0, 0), false);
        buffer.move_to(Pos::new(1, 2), true);
        assert_eq!(buffer.selected_text(), "hello! there\nbi");
        buffer.insert("W");
        assert_eq!(buffer.text(), "World");
    }

    #[test]
    fn deleting_goes_by_whole_characters() {
        let mut buffer = Buffer::new("가나🇰🇷");
        buffer.doc_end(false);
        buffer.backspace();
        assert_eq!(buffer.text(), "가나");
        buffer.backspace();
        assert_eq!(buffer.text(), "가");
        buffer.move_to(Pos::new(0, 0), false);
        buffer.delete_forward();
        assert_eq!(buffer.text(), "");
    }

    #[test]
    fn typing_is_undone_a_word_at_a_time() {
        let mut buffer = Buffer::new("");
        typed(&mut buffer, "let x");
        assert_eq!(buffer.text(), "let x");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "let");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "");
        assert!(!buffer.undo());
        assert!(buffer.redo());
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "let x");
        assert!(!buffer.redo());
    }

    #[test]
    fn backspaces_are_undone_together() {
        let mut buffer = Buffer::new("abcdef");
        buffer.doc_end(false);
        buffer.backspace();
        buffer.backspace();
        buffer.backspace();
        assert_eq!(buffer.text(), "abc");
        buffer.undo();
        assert_eq!(buffer.text(), "abcdef");
        assert_eq!(buffer.cursor(), Pos::new(0, 6));
        // Forward deletes too.
        buffer.move_to(Pos::new(0, 1), false);
        buffer.delete_forward();
        buffer.delete_forward();
        assert_eq!(buffer.text(), "adef");
        buffer.undo();
        assert_eq!(buffer.text(), "abcdef");
    }

    #[test]
    fn a_new_edit_drops_what_could_be_redone() {
        let mut buffer = Buffer::new("a");
        buffer.doc_end(false);
        buffer.paste("b");
        buffer.undo();
        buffer.paste("c");
        assert!(!buffer.redo());
        assert_eq!(buffer.text(), "ac");
    }

    #[test]
    fn dirty_follows_undo_back_to_the_saved_text() {
        let mut buffer = Buffer::new("x");
        assert!(!buffer.is_dirty());
        buffer.doc_end(false);
        typed(&mut buffer, "y");
        assert!(buffer.is_dirty());
        buffer.undo();
        assert!(!buffer.is_dirty());
        buffer.redo();
        buffer.mark_saved();
        assert!(!buffer.is_dirty());
        // Typing on after a save is a change even though it would join the last step otherwise.
        typed(&mut buffer, "z");
        assert!(buffer.is_dirty());
        buffer.undo();
        assert!(!buffer.is_dirty());
        assert_eq!(buffer.text(), "xy");
    }

    #[test]
    fn enter_keeps_the_indentation_and_opens_blocks() {
        let mut buffer = Buffer::new("fn main() {}");
        buffer.move_to(Pos::new(0, 11), false);
        buffer.newline();
        assert_eq!(buffer.text(), "fn main() {\n    \n}");
        assert_eq!(buffer.cursor(), Pos::new(1, 4));
        typed(&mut buffer, "let a = 1;\n");
        assert_eq!(buffer.text(), "fn main() {\n    let a = 1;\n    \n}");
    }

    #[test]
    fn tab_indents_lines_and_shift_tab_outdents() {
        let mut buffer = Buffer::new("a\n  b\nc");
        buffer.move_to(Pos::new(0, 0), false);
        buffer.move_to(Pos::new(2, 1), true);
        buffer.indent();
        assert_eq!(buffer.text(), "  a\n    b\n  c");
        buffer.outdent();
        assert_eq!(buffer.text(), "a\n  b\nc");
        buffer.outdent();
        assert_eq!(buffer.text(), "a\nb\nc");
        // One undo step each.
        buffer.undo();
        assert_eq!(buffer.text(), "a\n  b\nc");
        // At a cursor: spaces to the next step.
        let mut buffer = Buffer::new("ab");
        buffer.move_to(Pos::new(0, 1), false);
        buffer.indent();
        assert_eq!(buffer.text(), "a   b");
    }

    #[test]
    fn indentation_is_guessed_from_the_file() {
        assert_eq!(Buffer::new("a\n  b\n    c\n  d").indent_unit(), "  ");
        assert_eq!(Buffer::new("a\n\tb\n\t\tc").indent_unit(), "\t");
        assert_eq!(Buffer::new("a\n    b\n        c").indent_unit(), "    ");
        assert_eq!(Buffer::new("plain").indent_unit(), "    ");
    }

    #[test]
    fn moving_by_words_lines_and_home() {
        let mut buffer = Buffer::new("  let value = 10;\nx");
        buffer.move_to(Pos::new(0, 0), false);
        buffer.right(false, true);
        assert_eq!(buffer.cursor(), Pos::new(0, 5));
        buffer.right(false, true);
        assert_eq!(buffer.cursor(), Pos::new(0, 11));
        buffer.left(false, true);
        assert_eq!(buffer.cursor(), Pos::new(0, 6));
        buffer.home(false);
        assert_eq!(buffer.cursor(), Pos::new(0, 2));
        buffer.home(false);
        assert_eq!(buffer.cursor(), Pos::new(0, 0));
        // Down to a shorter line and back up keeps the column.
        buffer.move_to(Pos::new(0, 12), false);
        buffer.vertical(1, false);
        assert_eq!(buffer.cursor(), Pos::new(1, 1));
        buffer.vertical(-1, false);
        assert_eq!(buffer.cursor(), Pos::new(0, 12));
        // Past the ends.
        buffer.vertical(-5, false);
        assert_eq!(buffer.cursor(), Pos::new(0, 0));
        buffer.vertical(5, true);
        assert_eq!(buffer.cursor(), buffer.end());
        assert_eq!(buffer.selected_text(), "  let value = 10;\nx");
    }

    #[test]
    fn utf16_offsets_for_input_methods() {
        let buffer = Buffer::new("ab\n한글😀x");
        assert_eq!(buffer.utf16_offset(Pos::new(1, 0)), 3);
        assert_eq!(buffer.utf16_offset(Pos::new(1, 6)), 5);
        assert_eq!(buffer.utf16_offset(Pos::new(1, 10)), 7);
        assert_eq!(buffer.pos_at_utf16(7), Pos::new(1, 10));
        assert_eq!(buffer.pos_at_utf16(4), Pos::new(1, 3));
        assert_eq!(buffer.pos_at_utf16(100), buffer.end());
    }

    #[test]
    fn replacing_everything_keeps_the_cursor_line() {
        let mut buffer = Buffer::new("a  =  1\nb=2\nc=3");
        buffer.move_to(Pos::new(2, 1), false);
        buffer.set_text("a = 1\nb = 2\nc = 3\n");
        assert_eq!(buffer.cursor(), Pos::new(2, 1));
        buffer.undo();
        assert_eq!(buffer.text(), "a  =  1\nb=2\nc=3");
    }

    #[test]
    fn windows_line_breaks_in_pasted_text() {
        let mut buffer = Buffer::new("");
        buffer.paste("a\r\nb\rc");
        assert_eq!(buffer.text(), "a\nb\nc");
        assert_eq!(buffer.line_count(), 3);
    }

    #[test]
    fn display_columns_count_tabs_and_wide_characters() {
        assert_eq!(display_column("\tx", 1), 4);
        assert_eq!(display_column("ab\tx", 3), 4);
        assert_eq!(display_column("한a", 3), 2);
        assert_eq!(column_to_byte("한글", 2), 3);
        assert_eq!(column_to_byte("\tx", 4), 1);
        assert_eq!(column_to_byte("ab", 9), 2);
    }

    #[test]
    fn composing_hangul_is_one_undo_step() {
        let mut buffer = Buffer::new("a");
        buffer.doc_end(false);
        // ㅎ → 하 → 한, then committed.
        let at = buffer.cursor();
        buffer.compose(at, at, "ㅎ", None);
        assert_eq!(buffer.marked(), Some((Pos::new(0, 1), Pos::new(0, 4))));
        buffer.compose(Pos::new(0, 1), Pos::new(0, 4), "하", None);
        buffer.compose(Pos::new(0, 1), Pos::new(0, 4), "한", None);
        assert_eq!(buffer.text(), "a한");
        buffer.commit(Pos::new(0, 1), Pos::new(0, 4), "한");
        assert_eq!(buffer.marked(), None);
        assert_eq!(buffer.cursor(), Pos::new(0, 4));
        // The next syllable starts composing after it.
        let at = buffer.cursor();
        buffer.compose(at, at, "글", None);
        buffer.unmark();
        assert_eq!(buffer.text(), "a한글");
        assert!(buffer.is_dirty());
        buffer.undo();
        assert_eq!(buffer.text(), "a");
        assert!(!buffer.is_dirty());
        buffer.redo();
        assert_eq!(buffer.text(), "a한글");
    }

    #[test]
    fn composing_over_a_selection_restores_it_on_undo() {
        let mut buffer = Buffer::new("hello");
        buffer.select_all();
        let (start, end) = buffer.selection.range();
        buffer.compose(start, end, "ㅇ", None);
        assert_eq!(buffer.text(), "ㅇ");
        let (start, end) = buffer.marked().unwrap();
        buffer.commit(start, end, "안");
        assert_eq!(buffer.text(), "안");
        buffer.undo();
        assert_eq!(buffer.text(), "hello");
        assert_eq!(buffer.selected_text(), "hello");
    }

    #[test]
    fn a_long_history_never_undoes_back_to_clean() {
        let mut buffer = Buffer::new("");
        for i in 0..UNDO_LIMIT + 5 {
            buffer.paste(&i.to_string());
        }
        while buffer.undo() {}
        assert_ne!(buffer.text(), "");
        assert!(buffer.is_dirty(), "the loaded text is no longer reachable");
        let at = buffer.cursor();
        let mut composing = Buffer::new("a");
        composing.compose(at.min(composing.end()), at.min(composing.end()), "ㅎ", None);
        assert!(composing.is_dirty());
    }

    #[test]
    fn double_click_selects_a_word() {
        let buffer = Buffer::new("foo_bar(baz)");
        assert_eq!(buffer.word_at(Pos::new(0, 2)), (Pos::new(0, 0), Pos::new(0, 7)));
        assert_eq!(buffer.word_at(Pos::new(0, 7)), (Pos::new(0, 7), Pos::new(0, 8)));
        assert_eq!(buffer.word_at(Pos::new(0, 12)), (Pos::new(0, 11), Pos::new(0, 12)));
    }
}
