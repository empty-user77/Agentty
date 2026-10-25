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
/// Edits kept at most (one step at many cursors is many edits).
const UNDO_EDITS: usize = 50_000;
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
        let indent = detect_indent(lines.iter().map(String::as_str)).unwrap_or_else(|| "    ".into());
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

    /// The indentation step to use where the text shows none of its own.
    pub fn set_indent_unit(&mut self, unit: String) {
        self.indent = unit;
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
        // Where each cursor ends up once every change is made.
        let mut after = Vec::with_capacity(kept.len());
        let (mut at, mut from) = (kept[0].1, kept[0].1);
        for (is_main, start, end, piece) in &kept {
            at = if start.line == from.line {
                Pos::new(at.line, at.col + start.col - from.col)
            } else {
                Pos::new(at.line + start.line - from.line, start.col)
            };
            at = end_of(at, piece);
            after.push((*is_main, at));
            from = *end;
        }
        let joins = typing && self.undo.last().is_some_and(|last| last.multi && !last.sealed && last.at.elapsed() < TYPING_GROUP);
        // One edit per cursor, the last first so the others keep their places: the text between
        // cursors far apart is neither copied nor counted as changed (its folds stay). They are
        // chained, so one undo takes back the change at every cursor.
        self.multi = true;
        let made = self.undo.len();
        for (start, end, piece) in
            kept.iter().rev().map(|(_, start, end, piece)| (start, end, piece)).filter(|(s, e, p)| s != e || !p.is_empty())
        {
            let first = self.undo.len() == made;
            self.replace(*start, *end, piece, EditKind::Other);
            if let Some(edit) = self.undo.last_mut() {
                edit.multi = typing;
                edit.chain = if first { joins } else { true };
            }
        }
        self.multi = false;
        let main_at = after.iter().find(|(m, _)| *m).map_or(after[0].1, |(_, p)| *p);
        self.selection = Selection::cursor(main_at);
        self.extra = after.iter().filter(|(m, _)| !*m).map(|(_, p)| Selection::cursor(*p)).collect();
        let selection = self.selection;
        let n = self.undo.len();
        for edit in &mut self.undo[made.min(n)..] {
            edit.before = main;
            edit.after = selection;
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

    pub fn has_extra_cursors(&self) -> bool {
        !self.extra.is_empty()
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

    /// ⌥⌘↑ / ⌥⌘↓: one more cursor on the line above the highest cursor (below the lowest), at the
    /// display column the main one keeps; the new one leads. False at the first (last) line.
    pub fn add_cursor_vertical(&mut self, up: bool) -> bool {
        let all = self.selections();
        let edge = if up { all.iter().map(|s| s.head).min() } else { all.iter().map(|s| s.head).max() }.unwrap_or_default();
        let line = if up { edge.line.checked_sub(1) } else { Some(edge.line + 1).filter(|l| *l < self.lines.len()) };
        let Some(line) = line else { return false };
        let main = self.cursor();
        let goal = self.goal_column.unwrap_or_else(|| display_column(self.line(main.line), main.col));
        self.extra.push(self.selection);
        self.selection = Selection::cursor(Pos::new(line, column_to_byte(self.line(line), goal)));
        self.seal();
        self.merge_cursors();
        self.goal_column = Some(goal);
        true
    }

    /// ⇧⌘\: to the bracket matching the one at the cursor, or else to the closing bracket of the
    /// pair the cursor is in. Looks `limit` characters at most. False when there is none.
    pub fn go_to_bracket(&mut self, limit: usize) -> bool {
        if let Some((_, there)) = self.matching_bracket(limit) {
            self.move_to(there, false);
            return true;
        }
        // The opening bracket of the pair around the cursor, then its match.
        let cursor = self.cursor();
        let mut depth = 0usize;
        let mut seen = 0usize;
        for row in (0..=cursor.line).rev() {
            let text = self.line(row);
            let to = if row == cursor.line { cursor.col } else { text.len() };
            for (i, ch) in text[..to].char_indices().rev() {
                seen += 1;
                if seen > limit {
                    return false;
                }
                match ch {
                    ')' | ']' | '}' => depth += 1,
                    '(' | '[' | '{' if depth > 0 => depth -= 1,
                    '(' | '[' | '{' => {
                        self.move_to(Pos::new(row, i), false);
                        return match self.matching_bracket(limit) {
                            Some((_, there)) => {
                                self.move_to(there, false);
                                true
                            }
                            None => {
                                self.move_to(cursor, false);
                                false
                            }
                        };
                    }
                    _ => {}
                }
            }
        }
        false
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
        // The oldest steps go, each with the edits chained to it.
        while self.undo.len() > UNDO_LIMIT && (self.undo.len() > UNDO_EDITS || self.undo.iter().filter(|e| !e.chain).count() > UNDO_LIMIT) {
            let dropped = self.undo.remove(0);
            let mut ids = vec![dropped.id];
            while self.undo.first().is_some_and(|next| next.chain) {
                ids.push(self.undo.remove(0).id);
            }
            // The saved text can't be reached by undoing any more: it never counts as clean again.
            if self.saved_at == 0 || ids.contains(&self.saved_at) {
                self.saved_at = u64::MAX;
            }
            if self.undo.is_empty() {
                break;
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

    /// After the `>` of an opening tag just typed (`<section className="cart">`), its closing tag
    /// goes after the cursor, as editors do for markup and JSX. Not after a self-closing tag, a
    /// generic (`useState<T>`), an arrow (`=>`) or a comparison, nor for the void elements of HTML
    /// (`html`: `<br>`, `<img>`, …). True when it was put in.
    pub fn close_tag(&mut self, html: bool) -> bool {
        if !self.extra.is_empty() || !self.selection.is_empty() {
            return false;
        }
        let cursor = self.cursor();
        let line = self.line(cursor.line);
        if !line[..cursor.col].ends_with('>') {
            return false;
        }
        // The opening tag may start a few lines up (attributes on lines of their own).
        let first = cursor.line.saturating_sub(TAG_LINES);
        let mut text = String::new();
        for i in first..cursor.line {
            text.push_str(self.line(i));
            text.push('\n');
        }
        text.push_str(&line[..cursor.col]);
        let Some(name) = opened_tag(&text) else { return false };
        const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];
        if html && VOID.contains(&name.to_ascii_lowercase().as_str()) {
            return false;
        }
        let closing = format!("</{name}>");
        if line[cursor.col..].starts_with(&closing) {
            return false;
        }
        self.replace(cursor, cursor, &closing, EditKind::Other);
        self.selection = Selection::cursor(cursor);
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
        }
        true
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
        // `<div>|</div>`: the tags open and close like brackets.
        let tags = before.ends_with('>') && !before.ends_with("/>") && after.starts_with("</");
        let (opens, closes) = (opens || tags, after.starts_with(['}', ']', ')']) || tags);
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

    /// The selected lines are markup of a `.tsx` / `.jsx` file, where a comment is `{/* … */}`
    /// (`//` would show as text): the first line with text starts a tag or is such a comment, or
    /// is text or `{…}` below a line that ends a tag, with only such lines between.
    pub fn selection_is_jsx(&self) -> bool {
        const CODE: &[&str] = &["const ", "let ", "var ", "return", "if ", "if(", "for ", "while ", "function ", "import ", "export "];
        // Text between tags: not a line of code by its start or its end.
        let text = |l: &str| {
            !l.starts_with([')', '}', ']', ';'])
                && !l.starts_with("//")
                && !CODE.iter().any(|word| l.starts_with(word))
                && !l.ends_with([';', '{', '(', '[', ',', '='])
        };
        let (first, last) = self.selected_lines();
        let Some(line) = (first..=last).map(|i| self.line(i).trim()).find(|l| !l.is_empty()) else { return false };
        if line.starts_with("{/*") || (line.starts_with('<') && !line.starts_with("<<")) {
            return true;
        }
        if !text(line) {
            return false;
        }
        for above in (first.saturating_sub(200)..first).rev().map(|i| self.line(i).trim()).filter(|l| !l.is_empty()) {
            // After a tag, or the `(` markup starts in (`return (`).
            if (above.ends_with('>') && !above.ends_with("=>")) || above.ends_with("return (") || above.ends_with("=> (") {
                return true;
            }
            if !text(above) || above.starts_with('<') {
                return false;
            }
        }
        false
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

    /// `text`, a formatting of this one, in its place: the cursor stays by the same character
    /// (counted without the spaces and line breaks a formatter moves), one cursor only.
    pub fn set_formatted(&mut self, text: &str) {
        let solid = |line: &str, col: usize| line[..col.min(line.len())].chars().filter(|c| !c.is_whitespace()).count();
        let count = |pos: Pos| (0..pos.line).map(|i| solid(self.line(i), usize::MAX)).sum::<usize>() + solid(self.line(pos.line), pos.col);
        // A cursor in a line's indentation goes before that line's first character, not after the
        // last one of the line above; one on a blank line stays after as many blank lines.
        let at = |pos: Pos| {
            let line_start = solid(self.line(pos.line), pos.col) == 0;
            let blanks = if line_start { (0..pos.line).rev().take_while(|&i| solid(self.line(i), usize::MAX) == 0).count() } else { 0 };
            (count(pos), line_start, blanks)
        };
        let (anchor, head) = (at(self.selection.anchor), at(self.selection.head));
        self.extra.clear();
        self.set_text(text);
        let place = |(mut left, line_start, blanks): (usize, bool, usize)| {
            if line_start {
                // The line after the one with the `left`-th character, past `blanks` blank lines.
                let mut row = 0;
                if left > 0 {
                    row = self.lines.len() - 1;
                    let mut seen = 0;
                    for (i, line) in self.lines.iter().enumerate() {
                        seen += solid(line, usize::MAX);
                        if seen >= left {
                            row = (i + 1).min(self.lines.len() - 1);
                            break;
                        }
                    }
                }
                let mut skipped = 0;
                while skipped < blanks && row + 1 < self.lines.len() && solid(&self.lines[row], usize::MAX) == 0 {
                    row += 1;
                    skipped += 1;
                }
                let line = &self.lines[row];
                return Pos::new(row, line.len() - line.trim_start().len());
            }
            for (row, line) in self.lines.iter().enumerate() {
                let here = solid(line, usize::MAX);
                if left > here {
                    left -= here;
                    continue;
                }
                // The place after the `left`-th character of the line that is not a space.
                let col = if left == 0 {
                    line.len() - line.trim_start().len()
                } else {
                    line.char_indices().filter(|(_, c)| !c.is_whitespace()).nth(left - 1).map_or(line.len(), |(i, c)| i + c.len_utf8())
                };
                return Pos::new(row, col);
            }
            self.end()
        };
        self.selection = Selection { anchor: place(anchor), head: place(head) };
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
        }
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
        let composed = self.undo.len();
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
        if composed >= 1 && composed < self.undo.len() {
            let joins = composed >= 2 && {
                let before = &self.undo[composed - 2];
                before.multi && !before.sealed && before.at.elapsed() < TYPING_GROUP
            };
            self.undo[composed - 1].multi = true;
            self.undo[composed - 1].chain = joins;
            self.undo[composed].chain = true;
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

    /// ↑ / ↓ by `rows` of the view (negative: up), keeping the display column; the `hidden` lines
    /// (folded away) take none.
    pub fn vertical_in(&mut self, rows: isize, select: bool, hidden: &[(usize, usize)]) {
        let cursor = self.cursor();
        let goal = self.goal_column.unwrap_or_else(|| display_column(self.line(cursor.line), cursor.col));
        let target = super::fold::row_of(hidden, cursor.line) as isize + rows;
        let shown = self.lines.len() - hidden.iter().map(|(from, to)| to - from + 1).sum::<usize>();
        let pos = if target < 0 {
            Pos::default()
        } else if target as usize >= shown {
            let line = super::fold::line_of(hidden, shown.saturating_sub(1));
            Pos::new(line, self.line(line).len())
        } else {
            let line = super::fold::line_of(hidden, target as usize);
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

/// Lines above the cursor looked through for the start of a tag being closed.
const TAG_LINES: usize = 12;

/// The name of the tag that `text` (up to a `>` just typed) ends opening, if it does:
/// `<a href="x">` → `a`, `<List onPick={(p) => pick(p)}>` → `List`. `None` for `<br/>`, `</a>`,
/// `useState<T>`, `a => b`, `x > y` and the like.
fn opened_tag(text: &str) -> Option<String> {
    let body = text.strip_suffix('>')?;
    if body.ends_with('/') || body.ends_with('=') || body.ends_with('-') {
        return None;
    }
    // From each `<` (the last first), the tag must run to the end at depth 0 of braces and
    // outside quotes, with no other `>` of its own.
    for (start, _) in body.match_indices('<').collect::<Vec<_>>().into_iter().rev() {
        let before = body[..start].chars().next_back();
        // `useState<T>`, `Array<string>`: a generic, not a tag.
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') {
            continue;
        }
        let rest = &body[start + 1..];
        let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '.' | ':' | '-' | '_')).collect();
        if name.is_empty() || !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        let attributes = &rest[name.len()..];
        if !attributes.is_empty() && !attributes.starts_with(char::is_whitespace) {
            return None;
        }
        let (mut depth, mut quote) = (0i32, None::<char>);
        for c in attributes.chars() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'' | '`') => quote = Some(c),
                (None, '{') => depth += 1,
                (None, '}') => depth -= 1,
                (None, '>' | '<') if depth == 0 => return None,
                _ => {}
            }
        }
        return (depth == 0 && quote.is_none()).then_some(name);
    }
    None
}

/// Cursors ⇧⌘L makes at most.
const MAX_CURSORS: usize = 1_000;

/// Changes kept for [`Buffer::take_changes`] at most (a replace-all of thousands is everything).
const MAX_CHANGES: usize = 4096;

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

/// The indentation step a text uses: a tab when most indented lines start with one, else the
/// smallest step between space indentations. `None` when no line is indented.
pub fn detect_indent<'a>(lines: impl Iterator<Item = &'a str>) -> Option<String> {
    let (mut tabs, mut spaced) = (0, 0);
    let mut steps = [0usize; 9];
    let mut previous = 0usize;
    for line in lines.take(5000) {
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
        return Some("\t".into());
    }
    let best = (2..=8).max_by_key(|step| (steps[*step], usize::from(*step == 4))).filter(|step| steps[*step] > 0)?;
    Some(" ".repeat(best))
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
    fn markup_in_jsx_is_commented_as_jsx() {
        let mut b =
            Buffer::new("  const a = 1;\n  return (\n    <label>\n      <input />\n      In stock\n      {count} left\n    </label>\n  );");
        let on = |b: &mut Buffer, line: usize| {
            b.move_to(Pos::new(line, 0), false);
            b.selection_is_jsx()
        };
        assert!(!on(&mut b, 0) && !on(&mut b, 1), "code");
        assert!(on(&mut b, 2) && on(&mut b, 3) && on(&mut b, 6), "tags");
        assert!(on(&mut b, 4) && on(&mut b, 5), "text and an expression between tags");
        assert!(!on(&mut b, 7), "the code after it");
        let mut code = Buffer::new("function f() {\n  const a = 1\n  a\n}");
        assert!(!on(&mut code, 2), "code without semicolons is not text");
        b.move_to(Pos::new(3, 0), false);
        b.toggle_comment("{/*", "*/}");
        assert_eq!(b.line(3), "      {/* <input /> */}");
        assert!(b.selection_is_jsx());
        b.toggle_comment("{/*", "*/}");
        assert_eq!(b.line(3), "      <input />");
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
    fn up_and_down_step_over_folded_lines() {
        let mut b = Buffer::new("a {\n  b\n  c\n}\nd");
        at(&mut b, 0, 1);
        b.vertical_in(1, false, &[(1, 2)]);
        assert_eq!(b.cursor(), Pos::new(3, 1));
        b.vertical_in(-1, false, &[(1, 2)]);
        assert_eq!(b.cursor(), Pos::new(0, 1));
        b.vertical_in(10, false, &[(1, 2)]);
        assert_eq!(b.cursor(), Pos::new(4, 1));
        // Folded to the end of the file: ↓ on its first line stays on it.
        at(&mut b, 3, 0);
        b.vertical_in(1, false, &[(4, 4)]);
        assert_eq!(b.cursor(), Pos::new(3, 1));
    }

    #[test]
    fn formatting_keeps_the_cursor_by_its_character() {
        let mut b = Buffer::new("class A {\n            int  x = 1;\n}");
        at(&mut b, 1, 18);
        assert_eq!(&b.line(1)[18..], " = 1;");
        b.set_formatted("class A {\n    int x = 1;\n}");
        assert_eq!(b.cursor(), Pos::new(1, 9), "still after the `x`");
        // At a line start: before its first character.
        let mut b = Buffer::new("a\n      b");
        at(&mut b, 1, 6);
        b.set_formatted("a\n  b");
        assert_eq!(b.cursor(), Pos::new(1, 2));
        b.undo();
        assert_eq!(b.text(), "a\n      b");
        // On a blank line, and after one: where it was.
        let mut b = Buffer::new("a  \n\nb");
        at(&mut b, 1, 0);
        b.set_formatted("a\n\nb");
        assert_eq!(b.cursor(), Pos::new(1, 0));
        at(&mut b, 2, 0);
        b.set_formatted("a\n\nb\n");
        assert_eq!(b.cursor(), Pos::new(2, 0));
        let mut b = Buffer::new("\nx");
        b.set_formatted("\nx");
        assert_eq!(b.cursor(), Pos::new(0, 0), "a blank first line");
    }

    #[test]
    fn far_apart_cursors_change_only_their_places() {
        let middle = "line\n".repeat(5000);
        let mut b = Buffer::new(&format!("a\n{middle}b"));
        at(&mut b, 0, 1);
        b.toggle_cursor(Pos::new(5001, 1));
        b.take_changes();
        b.insert("!");
        b.insert("?");
        assert!(b.undo.iter().all(|e| e.removed.is_empty() && e.inserted.len() == 1), "no copy of the lines between");
        assert_eq!(b.take_changes().map(|c| c.iter().all(|s| s.from == s.to && s.delta == 0)), Some(true));
        assert_eq!((b.line(0), b.line(5001)), ("a!?", "b!?"));
        assert_eq!(b.selections().len(), 2);
        // One undo takes back the typing at both.
        b.undo();
        assert_eq!((b.line(0), b.line(5001)), ("a", "b"));
        b.redo();
        assert_eq!((b.line(0), b.line(5001)), ("a!?", "b!?"));
    }

    #[test]
    fn cursors_go_on_the_lines_above_and_below() {
        let mut b = Buffer::new("let a = 1;\nb\nlet c = 3;\nlet d = 4;");
        at(&mut b, 2, 6);
        assert!(b.add_cursor_vertical(false));
        // A short line on the way keeps the column for the next one.
        assert!(b.add_cursor_vertical(true) && b.add_cursor_vertical(true));
        let mut heads: Vec<Pos> = b.selections().iter().map(|s| s.head).collect();
        heads.sort();
        assert_eq!(heads, [Pos::new(0, 6), Pos::new(1, 1), Pos::new(2, 6), Pos::new(3, 6)]);
        assert!(!b.add_cursor_vertical(true), "nothing above the first line");
        b.insert("x");
        assert_eq!(b.text(), "let a x= 1;\nbx\nlet c x= 3;\nlet d x= 4;");
    }

    #[test]
    fn goes_to_the_matching_or_enclosing_bracket() {
        let mut b = Buffer::new("fn f() {\n    call(a, [b]);\n}");
        at(&mut b, 0, 7);
        assert!(b.go_to_bracket(10_000));
        assert_eq!(b.cursor(), Pos::new(2, 0));
        assert!(b.go_to_bracket(10_000));
        assert_eq!(b.cursor(), Pos::new(0, 7), "and back");
        // Inside `call(…)`, next to no bracket: to its closing parenthesis.
        at(&mut b, 1, 10);
        assert!(b.go_to_bracket(10_000));
        assert_eq!(b.cursor(), Pos::new(1, 15));
        at(&mut b, 1, 12);
        assert!(b.go_to_bracket(10_000));
        assert_eq!(b.cursor(), Pos::new(1, 14));
        let mut b = Buffer::new("plain");
        at(&mut b, 0, 2);
        assert!(!b.go_to_bracket(10_000));
        assert_eq!(b.cursor(), Pos::new(0, 2));
    }

    #[test]
    fn an_opening_tag_gets_its_closing_tag() {
        assert_eq!(opened_tag("      <section className=\"cart\">").as_deref(), Some("section"));
        assert_eq!(opened_tag("<ProductList onPick={(p) => setCart([...cart, p])}>").as_deref(), Some("ProductList"));
        assert_eq!(opened_tag("<Foo.Bar>").as_deref(), Some("Foo.Bar"));
        assert_eq!(opened_tag("<div\n  className=\"a\"\n>").as_deref(), Some("div"));
        for text in ["<br/>", "</div>", "useState<Product[]>", "const f = (a) =>", "if (a > b) {x >", "a < b >", "x->", "<a href=\"x>"] {
            assert_eq!(opened_tag(text), None, "{text}");
        }
        let mut b = Buffer::new("  <ul>");
        at(&mut b, 0, 6);
        assert!(b.close_tag(false));
        assert_eq!((b.text().as_str(), b.cursor()), ("  <ul></ul>", Pos::new(0, 6)));
        // Enter between the tags: the cursor on a line of its own, one step (two spaces here) in.
        b.newline();
        assert_eq!(b.text(), "  <ul>\n    \n  </ul>");
        let mut b = Buffer::new("<br>");
        at(&mut b, 0, 4);
        assert!(!b.close_tag(true), "a void element of HTML");
        assert!(b.close_tag(false), "JSX closes every tag");
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
        buffer.vertical_in(1, false, &[]);
        assert_eq!(buffer.cursor(), Pos::new(1, 1));
        buffer.vertical_in(-1, false, &[]);
        assert_eq!(buffer.cursor(), Pos::new(0, 12));
        // Past the ends.
        buffer.vertical_in(-5, false, &[]);
        assert_eq!(buffer.cursor(), Pos::new(0, 0));
        buffer.vertical_in(5, true, &[]);
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
