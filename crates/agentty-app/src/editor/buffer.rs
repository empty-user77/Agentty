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
}

pub struct Buffer {
    lines: Vec<String>,
    pub selection: Selection,
    /// Display column kept while moving up and down through shorter lines.
    goal_column: Option<usize>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    next_id: u64,
    /// Id of the edit the text was saved at (0: as loaded).
    saved_at: u64,
    /// First line changed since [`Buffer::take_changed_from`] was last asked.
    changed_from: Option<usize>,
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
            changed_from: None,
            revision: 0,
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

    /// The first line whose text changed since the last call (for the highlighter's cache).
    pub fn take_changed_from(&mut self) -> Option<usize> {
        self.changed_from.take()
    }

    /// Goes up with every change to the text.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn cursor(&self) -> Pos {
        self.selection.head
    }

    pub fn selected_text(&self) -> String {
        let (start, end) = self.selection.range();
        self.slice(start, end)
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
        self.lines.splice(start.line..=end.line, new_lines);
        self.changed_from = Some(self.changed_from.map_or(start.line, |line| line.min(start.line)));
        self.revision += 1;
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
        let (start, end) = self.selection.range();
        let kind = if text.contains('\n') || text.chars().count() > 1 { EditKind::Other } else { EditKind::Typing };
        self.replace(start, end, text, kind);
    }

    /// Pastes (never joined with typing).
    pub fn paste(&mut self, text: &str) {
        let (start, end) = self.selection.range();
        self.replace(start, end, text, EditKind::Other);
    }

    pub fn delete_selection(&mut self) -> bool {
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
        let cursor = self.cursor();
        let end = self.next(cursor, false);
        if end != cursor {
            self.replace(cursor, end, "", EditKind::Deleting);
            self.selection = Selection::cursor(cursor);
        }
    }

    /// Deletes to the start of the word before the cursor (⌥⌫).
    pub fn delete_word_back(&mut self) {
        if self.delete_selection() {
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

    /// Replaces the whole text (formatting, reloading from disk) as one undo step, keeping the
    /// cursor on the same line and column where it still exists.
    pub fn set_text(&mut self, text: &str) {
        let text = normalize_newlines(text);
        if text == self.text() {
            return;
        }
        let selection = self.selection;
        let end = self.end();
        self.replace(Pos::default(), end, &text, EditKind::Other);
        self.selection = Selection { anchor: self.clamp(selection.anchor), head: self.clamp(selection.head) };
        if let Some(edit) = self.undo.last_mut() {
            edit.after = self.selection;
            edit.sealed = true;
        }
        self.changed_from = Some(0);
        self.revision += 1;
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
                let end = self.splice(start, end, &text);
                self.selection = Selection::cursor(end);
                self.finish(composition, end);
            }
            Some(composition) => {
                let marked_end = composition.marked.1;
                self.finish(composition, marked_end);
                let (start, end) = (self.clamp(start), self.clamp(end));
                self.replace(start, end, text, if text.chars().count() == 1 { EditKind::Typing } else { EditKind::Other });
            }
            None => {
                self.replace(start, end, text, if text.chars().count() == 1 && text != "\n" { EditKind::Typing } else { EditKind::Other });
            }
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
        });
    }

    pub fn undo(&mut self) -> bool {
        self.unmark();
        let Some(edit) = self.undo.pop() else { return false };
        let end = end_of(edit.start, &edit.inserted);
        self.splice(edit.start, end, &edit.removed);
        self.selection = edit.before;
        self.goal_column = None;
        self.redo.push(edit);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.unmark();
        let Some(mut edit) = self.redo.pop() else { return false };
        let end = end_of(edit.start, &edit.removed);
        self.splice(edit.start, end, &edit.inserted);
        self.selection = edit.after;
        self.goal_column = None;
        edit.sealed = true;
        self.undo.push(edit);
        true
    }

    // -- moving -----------------------------------------------------------------------------

    /// Moves the cursor to `pos`, extending the selection when `select`.
    pub fn move_to(&mut self, pos: Pos, select: bool) {
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
