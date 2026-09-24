//! Single-line, IME-aware text field (adapted from GPUI's input example).

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};

actions!(
    text_input,
    [Backspace, Delete, Left, Right, SelectLeft, SelectRight, SelectAll, Home, End, Paste, Cut, Copy, Confirm, Cancel, MoveUp, MoveDown]
);

const CONTEXT: &str = "TextInput";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        crate::key("backspace", Backspace, Some(CONTEXT)),
        crate::key("delete", Delete, Some(CONTEXT)),
        crate::key("left", Left, Some(CONTEXT)),
        crate::key("right", Right, Some(CONTEXT)),
        crate::key("shift-left", SelectLeft, Some(CONTEXT)),
        crate::key("shift-right", SelectRight, Some(CONTEXT)),
        crate::key("cmd-a", SelectAll, Some(CONTEXT)),
        crate::key("cmd-v", Paste, Some(CONTEXT)),
        crate::key("cmd-c", Copy, Some(CONTEXT)),
        crate::key("cmd-x", Cut, Some(CONTEXT)),
        crate::key("home", Home, Some(CONTEXT)),
        crate::key("end", End, Some(CONTEXT)),
        crate::key("enter", Confirm, Some(CONTEXT)),
        crate::key("escape", Cancel, Some(CONTEXT)),
        crate::key("up", MoveUp, Some(CONTEXT)),
        crate::key("down", MoveDown, Some(CONTEXT)),
    ]);
}

pub enum TextInputEvent {
    Changed,
    Confirmed,
    Cancelled,
    Blurred,
    Up,
    Down,
    /// Multi-line text pasted into a field made with [`TextInput::keep_pasted_lines`]; the field
    /// itself is left unchanged.
    PastedLines(String),
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    /// i18n key of the placeholder, looked up at paint so it follows language changes.
    placeholder_key: Option<&'static str>,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// Secret entry: shows bullets and refuses copy/cut.
    masked: bool,
    /// Multi-line pastes are handed to the owner (`PastedLines`) instead of being joined.
    keep_pasted_lines: bool,
    /// Rows shown when the field holds many lines; 0 is the usual one-line field.
    rows: usize,
    /// First line drawn, when the text is taller than `rows`.
    scroll_line: usize,
    /// Each visible line of the last paint: where it starts in the content, and its layout.
    last_lines: Vec<(usize, ShapedLine)>,
    /// Right-click menu, at the position it was opened.
    menu_at: Option<Point<Pixels>>,
    _blur: Option<gpui::Subscription>,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInput {
    pub fn new(
        content: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let blur = cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(TextInputEvent::Blurred));
        let content: SharedString = content.into();
        let len = content.len();
        Self {
            focus_handle,
            content,
            placeholder: placeholder.into(),
            placeholder_key: None,
            selected_range: 0..len,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            masked: false,
            keep_pasted_lines: false,
            rows: 0,
            scroll_line: 0,
            last_lines: Vec::new(),
            menu_at: None,
            _blur: Some(blur),
        }
    }

    /// Like [`TextInput::new`] with a translated placeholder (`key` in `i18n.rs`).
    pub fn localized(content: impl Into<SharedString>, key: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut input = Self::new(content, crate::i18n::t(cx, key), window, cx);
        input.placeholder_key = Some(key);
        input
    }

    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    pub fn keep_pasted_lines(mut self) -> Self {
        self.keep_pasted_lines = true;
        self
    }

    /// A field of `rows` lines: Enter adds a line, Up and Down move through them, and a paste
    /// keeps its line breaks. One row or none is the usual single-line field.
    pub fn multiline(mut self, rows: usize) -> Self {
        self.rows = rows;
        self
    }

    /// Whether this field holds more than one line.
    pub fn is_multiline(&self) -> bool {
        self.rows > 1
    }

    fn line_starts(&self) -> Vec<usize> {
        line_starts(&self.content)
    }

    /// The line an offset is on, and where that line starts.
    fn line_at(&self, offset: usize) -> (usize, usize) {
        line_at(&self.content, offset)
    }

    /// The end of a line (before its newline).
    fn line_end(&self, line: usize) -> usize {
        line_end(&self.content, line)
    }

    /// The offset a line above or below the cursor, keeping the same place across the line.
    fn offset_line_away(&self, offset: usize, down: bool) -> usize {
        let starts = self.line_starts();
        let (line, start) = self.line_at(offset);
        let target = match down {
            true if line + 1 < starts.len() => line + 1,
            false if line > 0 => line - 1,
            // Already at the top or the bottom: to the start or the end of the text, as a
            // one-line field would.
            _ => return if down { self.content.len() } else { 0 },
        };
        let target_start = starts[target];
        let target_end = self.line_end(target);
        // The same x on the other line when it has been laid out; the same column otherwise.
        let x = self.last_lines.iter().find(|(from, _)| *from == start).map(|(_, layout)| layout.x_for_index(offset - start));
        match (x, self.last_lines.iter().find(|(from, _)| *from == target_start)) {
            (Some(x), Some((_, layout))) => target_start + layout.closest_index_for_x(x).min(target_end - target_start),
            _ => (target_start + (offset - start)).min(target_end),
        }
    }

    fn move_line(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let offset = self.offset_line_away(self.cursor_offset(), down);
        if select {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Gives this field the keyboard and selects what is in it (⌘L in the browser's address bar).
    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.selected_range = 0..self.content.len();
        cx.notify();
    }

    /// Changes the hint shown while the field is empty (a plugin may reword it).
    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) {
        let placeholder = placeholder.into();
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.placeholder_key = None;
            cx.notify();
        }
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text: SharedString = text.into();
        // One line holds no line break: the text system refuses to lay one out (and a plugin can
        // set any value it likes), so breaks become spaces rather than taking the window down.
        self.content = if !self.is_multiline() && text.contains(['\n', '\r']) {
            text.replace("\r\n", " ").replace(['\n', '\r'], " ").into()
        } else {
            text
        };
        self.selected_range = self.content.len()..self.content.len();
        self.marked_range = None;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        // In a field of many lines, Home is the start of the line, as everywhere else.
        let offset = if self.is_multiline() { self.line_at(self.cursor_offset()).1 } else { 0 };
        self.move_to(offset, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.is_multiline() { self.line_end(self.line_at(self.cursor_offset()).0) } else { self.content.len() };
        self.move_to(offset, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // A field of many lines takes the lines; a one-line field joins them, unless its
            // owner asked for them (`keep_pasted_lines`).
            if self.is_multiline() {
                self.replace_text_in_range(None, &text, window, cx);
                return;
            }
            if self.keep_pasted_lines && text.trim().contains('\n') {
                cx.emit(TextInputEvent::PastedLines(text.to_string()));
                return;
            }
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.is_multiline() {
            return self.index_for_mouse_in_lines(position);
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else { return 0 };
        if self.content.is_empty() || position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let index = line.closest_index_for_x(position.x - bounds.left());
        if self.masked {
            // The layout holds bullets (3 bytes each); map back to a content byte offset.
            let chars = index / '•'.len_utf8();
            return self.content.char_indices().nth(chars).map(|(i, _)| i).unwrap_or(self.content.len());
        }
        index
    }

    /// The offset under the mouse in a field of many lines: the row it is over, then the place
    /// across that row.
    fn index_for_mouse_in_lines(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), false) = (self.last_bounds.as_ref(), self.last_lines.is_empty()) else { return self.cursor_offset() };
        let height = (bounds.bottom() - bounds.top()) / self.last_lines.len() as f32;
        let row = ((position.y - bounds.top()) / height).floor().max(0.) as usize;
        let row = row.min(self.last_lines.len() - 1);
        let (start, layout) = &self.last_lines[row];
        let end = self.line_end(self.line_at(*start).0);
        (start + layout.closest_index_for_x(position.x - bounds.left())).min(end)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).rev().find_map(|(idx, _)| (idx < offset).then_some(idx)).unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).find_map(|(idx, _)| (idx > offset).then_some(idx)).unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        if self.masked {
            return None; // never hand secrets to IME or accessibility clients
        }
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected_range), reversed: self.selection_reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let range =
            range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked_range.clone()).unwrap_or(self.selected_range.clone());
        self.content = (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range =
            range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked_range.clone()).unwrap_or(self.selected_range.clone());
        self.content = (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.marked_range = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // A field of many lines: the range is on one of the drawn lines, and the candidate
        // window belongs under that line rather than under the whole field.
        if self.is_multiline() {
            let range = self.range_from_utf16(&range_utf16);
            let height = (bounds.bottom() - bounds.top()) / self.last_lines.len().max(1) as f32;
            let (row, (start, layout)) = self.last_lines.iter().enumerate().rev().find(|(_, (start, _))| *start <= range.start)?;
            let top = bounds.top() + height * row as f32;
            let from = layout.x_for_index(range.start.saturating_sub(*start));
            let to = layout.x_for_index(range.end.saturating_sub(*start));
            return Some(Bounds::from_corners(point(bounds.left() + from, top), point(bounds.left() + to, top + height)));
        }
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(bounds.left() + last_layout.x_for_index(range.start), bounds.top()),
            point(bounds.left() + last_layout.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

/// Where each line of `content` starts.
fn line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(content.match_indices('\n').map(|(index, _)| index + 1));
    starts
}

/// The line an offset is on, and where that line starts.
fn line_at(content: &str, offset: usize) -> (usize, usize) {
    let starts = line_starts(content);
    let line = starts.iter().rposition(|start| *start <= offset).unwrap_or(0);
    (line, starts[line])
}

/// The end of a line, before its newline.
fn line_end(content: &str, line: usize) -> usize {
    let starts = line_starts(content);
    match starts.get(line + 1) {
        Some(next) => next - 1,
        None => content.len(),
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    /// A field of many lines: each visible line with the offset it starts at.
    lines: Vec<(usize, ShapedLine)>,
    cursor: Option<PaintQuad>,
    /// One quad per line a selection covers.
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let rows = self.input.read(cx).rows.max(1);
        style.size.height = (window.line_height() * rows as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        if self.input.read(cx).is_multiline() {
            return self.prepaint_lines(bounds, window, cx);
        }
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();
        let masked = input.masked && !content.is_empty();
        let (display_text, text_color) = if content.is_empty() {
            let placeholder = input.placeholder_key.map_or_else(|| input.placeholder.clone(), |key| crate::i18n::t(cx, key).into());
            (placeholder, hex_alpha(Chrome::FOREGROUND, 0.4))
        } else if masked {
            (SharedString::from("•".repeat(content.chars().count())), style.color)
        } else {
            (content.clone(), style.color)
        };
        // Byte offsets in the content map to bullet offsets (each bullet is 3 bytes).
        let to_display = |offset: usize| if masked { content[..offset].chars().count() * '•'.len_utf8() } else { offset };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match input.marked_range.as_ref().filter(|_| !masked) {
            Some(marked) => vec![
                TextRun { len: marked.start, ..run.clone() },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false }),
                    ..run.clone()
                },
                TextRun { len: display_text.len() - marked.end, ..run },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            None => vec![run],
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        // `set_text` keeps line breaks out of a one-line field; a break that got in any other way
        // is drawn as a space (same length, so the offsets still line up) instead of panicking.
        let display_text: SharedString =
            if display_text.contains(['\n', '\r']) { display_text.replace(['\n', '\r'], " ").into() } else { display_text };
        let line = window.text_system().shape_line(display_text, font_size, &runs, None);
        let cursor_pos = line.x_for_index(to_display(cursor));
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(point(bounds.left() + cursor_pos, bounds.top()), size(px(1.5), bounds.bottom() - bounds.top())),
                    hex(Chrome::BLUE),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(to_display(selected_range.start)), bounds.top()),
                        point(bounds.left() + line.x_for_index(to_display(selected_range.end)), bounds.bottom()),
                    ),
                    hex_alpha(Chrome::ACCENT, 0.45),
                )),
                None,
            )
        };
        PrepaintState { line: Some(line), lines: Vec::new(), cursor, selections: selection.into_iter().collect() }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }
        let line_height = window.line_height();
        let focused = focus_handle.is_focused(window);
        if !prepaint.lines.is_empty() {
            let lines = std::mem::take(&mut prepaint.lines);
            for (index, (_, line)) in lines.iter().enumerate() {
                let origin = point(bounds.origin.x, bounds.origin.y + line_height * index as f32);
                let _ = line.paint(origin, line_height, window, cx);
            }
            if focused {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
            self.input.update(cx, |input, _| {
                input.last_lines = lines;
                input.last_bounds = Some(bounds);
            });
            return;
        }
        let Some(line) = prepaint.line.take() else { return };
        let _ = line.paint(bounds.origin, line_height, window, cx);
        if focused {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl TextElement {
    /// The same for a field of many lines: every visible line is shaped, the cursor is placed on
    /// its own line, and a selection becomes one quad per line it covers.
    fn prepaint_lines(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) -> PrepaintState {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let (content, selected_range, cursor, rows, placeholder) = {
            let input = self.input.read(cx);
            let placeholder = input
                .content
                .is_empty()
                .then(|| input.placeholder_key.map_or_else(|| input.placeholder.clone(), |key| crate::i18n::t(cx, key).into()));
            (input.content.clone(), input.selected_range.clone(), input.cursor_offset(), input.rows.max(1), placeholder)
        };
        // Keep the cursor's line in view, and never scroll past text that has since shrunk.
        let cursor_line = content[..cursor].matches('\n').count();
        let total = content.split('\n').count();
        let scroll = self.input.update(cx, |input, _| {
            input.scroll_line = input.scroll_line.min(total.saturating_sub(1));
            if cursor_line < input.scroll_line {
                input.scroll_line = cursor_line;
            } else if cursor_line >= input.scroll_line + rows {
                input.scroll_line = cursor_line + 1 - rows;
            }
            input.scroll_line
        });

        let text = placeholder.clone().unwrap_or_else(|| content.clone());
        let color = if placeholder.is_some() { hex_alpha(Chrome::FOREGROUND, 0.4) } else { style.color };
        let mut lines = Vec::new();
        let mut selections = Vec::new();
        let mut cursor_quad = None;
        let mut offset = 0;
        for (index, line_text) in text.split('\n').enumerate() {
            let start = offset;
            offset += line_text.len() + 1;
            if index < scroll || index >= scroll + rows {
                continue;
            }
            let run =
                TextRun { len: line_text.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
            let runs = if run.len > 0 { vec![run] } else { Vec::new() };
            let shaped = window.text_system().shape_line(SharedString::from(line_text.to_string()), font_size, &runs, None);
            let top = bounds.top() + line_height * (index - scroll) as f32;
            let end = start + line_text.len();
            if placeholder.is_none() {
                if selected_range.is_empty() {
                    if (start..=end).contains(&cursor) {
                        let x = bounds.left() + shaped.x_for_index(cursor - start);
                        cursor_quad = Some(fill(Bounds::new(point(x, top), size(px(1.5), line_height)), hex(Chrome::BLUE)));
                    }
                } else if selected_range.start <= end && selected_range.end >= start {
                    let from = shaped.x_for_index(selected_range.start.saturating_sub(start).min(line_text.len()));
                    let to = match selected_range.end > end {
                        // A line inside the selection is covered to its end, and a little past it
                        // so the line break is visible.
                        true => shaped.width + px(4.),
                        false => shaped.x_for_index(selected_range.end - start),
                    };
                    selections.push(fill(
                        Bounds::from_corners(point(bounds.left() + from, top), point(bounds.left() + to, top + line_height)),
                        hex_alpha(Chrome::ACCENT, 0.45),
                    ));
                }
            }
            lines.push((start, shaped));
        }
        PrepaintState { line: None, lines, cursor: cursor_quad, selections }
    }
}

impl TextInput {
    /// The right-click menu. macOS's own palette supplies emoji and symbols; the rest are this
    /// field's own commands, so they work here exactly as the keyboard shortcuts do.
    fn render_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let at = self.menu_at?;
        let has_selection = !self.selected_range.is_empty() && !self.masked;
        let item = |id: &'static str, label: String, enabled: bool, action: MenuAction, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_sm()
                .text_color(crate::theme::hex(if enabled { crate::theme::Chrome::BRIGHT } else { crate::theme::Chrome::MUTED }))
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(crate::theme::hex(crate::theme::Chrome::ACCENT)))
                        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, window, cx| this.run_menu(action, window, cx)))
                })
                .child(label)
        };
        Some(
            div().absolute().left(px(f32::from(at.x - self.last_bounds.map(|b| b.origin.x).unwrap_or(px(0.))))).top(px(18.)).child(
                gpui::deferred(
                    crate::ui::popover()
                        .id("text-input-menu")
                        .w(px(220.))
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu_at = None;
                            cx.notify();
                        }))
                        .child(item("ti-emoji", t(cx, "input.emoji").to_string(), true, MenuAction::Emoji, cx))
                        .child(div().my_1().h(px(1.)).bg(crate::theme::hex(crate::theme::Chrome::OVERLAY_BORDER)))
                        .child(item("ti-cut", t(cx, "input.cut").to_string(), has_selection, MenuAction::Cut, cx))
                        .child(item("ti-copy", t(cx, "input.copy").to_string(), has_selection, MenuAction::Copy, cx))
                        .child(item("ti-paste", t(cx, "input.paste").to_string(), true, MenuAction::Paste, cx))
                        .child(item(
                            "ti-select-all",
                            t(cx, "input.select_all").to_string(),
                            !self.content.is_empty(),
                            MenuAction::SelectAll,
                            cx,
                        )),
                )
                .with_priority(4),
            ),
        )
    }

    fn run_menu(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_at = None;
        match action {
            MenuAction::Emoji => crate::native::show_character_palette(),
            MenuAction::Cut => self.cut(&Cut, window, cx),
            MenuAction::Copy => self.copy(&Copy, window, cx),
            MenuAction::Paste => self.paste(&Paste, window, cx),
            MenuAction::SelectAll => self.select_all(&SelectAll, window, cx),
        }
        window.focus(&self.focus_handle);
        cx.notify();
    }
}

/// What an item of the right-click menu does.
#[derive(Clone, Copy)]
enum MenuAction {
    Emoji,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            // In a field of many lines Enter adds one, and Up and Down walk through them; in a
            // one-line field they mean what they always did to whoever owns it.
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                if this.is_multiline() {
                    this.replace_text_in_range(None, "\n", window, cx);
                } else {
                    cx.emit(TextInputEvent::Confirmed);
                }
            }))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(TextInputEvent::Cancelled)))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.is_multiline() {
                    this.move_line(false, false, cx);
                } else {
                    cx.emit(TextInputEvent::Up);
                }
            }))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.is_multiline() {
                    this.move_line(true, false, cx);
                } else {
                    cx.emit(TextInputEvent::Down);
                }
            }))
            // The right-click menu is placed against this element.
            .relative()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            // Right-click: emoji and symbols, and the edit commands, the way a text field should.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    this.menu_at = Some(event.position);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            // Grows inside flex containers too, where a percentage width alone can resolve to zero.
            .flex_1()
            .min_w_0()
            .w_full()
            .line_height(px(20.))
            .child(TextElement { input: cx.entity() })
            .children(self.render_menu(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "one\ntwo\n\nfour";

    #[test]
    fn lines_are_found_by_their_breaks() {
        assert_eq!(line_starts(TEXT), [0, 4, 8, 9]);
        assert_eq!(line_starts(""), [0]);
        // A trailing newline leaves an empty last line, as an editor shows it.
        assert_eq!(line_starts("a\n"), [0, 2]);
    }

    #[test]
    fn an_offset_knows_its_line() {
        assert_eq!(line_at(TEXT, 0), (0, 0));
        assert_eq!(line_at(TEXT, 3), (0, 0));
        // The offset after a newline is on the next line.
        assert_eq!(line_at(TEXT, 4), (1, 4));
        assert_eq!(line_at(TEXT, 8), (2, 8));
        assert_eq!(line_at(TEXT, TEXT.len()), (3, 9));
        assert_eq!(line_at("", 0), (0, 0));
    }

    #[test]
    fn a_line_ends_before_its_newline() {
        assert_eq!(line_end(TEXT, 0), 3);
        assert_eq!(line_end(TEXT, 1), 7);
        // An empty line ends where it starts.
        assert_eq!(line_end(TEXT, 2), 8);
        assert_eq!(line_end(TEXT, 3), TEXT.len());
        // Past the last line: the end of the text, not a panic.
        assert_eq!(line_end(TEXT, 9), TEXT.len());
    }
}
