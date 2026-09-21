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

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Gives this field the keyboard and selects what is in it (⌘L in the browser's address bar).
    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.selected_range = 0..self.content.len();
        cx.notify();
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
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
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
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

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
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
        style.size.height = window.line_height().into();
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
        PrepaintState { line: Some(line), cursor, selection }
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
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let Some(line) = prepaint.line.take() else { return };
        let _ = line.paint(bounds.origin, window.line_height(), window, cx);
        if focus_handle.is_focused(window) {
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
            .on_action(cx.listener(|_, _: &Confirm, _, cx| cx.emit(TextInputEvent::Confirmed)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(TextInputEvent::Cancelled)))
            .on_action(cx.listener(|_, _: &MoveUp, _, cx| cx.emit(TextInputEvent::Up)))
            .on_action(cx.listener(|_, _: &MoveDown, _, cx| cx.emit(TextInputEvent::Down)))
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
