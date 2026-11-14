//! The terminal's input handler: gpui's handler for a view, except that keys reach the terminal
//! before the input method.
//!
//! With an input method active (Korean, Japanese, Chinese), gpui hands a key that types a
//! character to the input method first when the focused handler asks for that, and its handler
//! for a view (`ElementInputHandler`) asks whenever the view takes text. A terminal reads keys as
//! keys: Option+B is Meta-B (`ESC b`) for the program, not the `b` the input method would insert
//! for it. So this answers no, as Zed's terminal does: the key goes to `on_key_down` first, and to
//! the input method only when that leaves it (a plain character, which is then composed as usual).

use gpui::{App, Bounds, ClipboardItem, InputHandler, Pixels, Point, TextInputConfiguration, UTF16Selection, Window};
use std::ops::Range;

pub struct KeysFirst<H>(pub H);

impl<H: InputHandler> InputHandler for KeysFirst<H> {
    fn prefers_ime_for_printable_keys(&mut self, _: &mut Window, _: &mut App) -> bool {
        false
    }

    fn selected_text_range(&mut self, ignore_disabled_input: bool, window: &mut Window, cx: &mut App) -> Option<UTF16Selection> {
        self.0.selected_text_range(ignore_disabled_input, window, cx)
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.0.marked_text_range(window, cx)
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.0.text_for_range(range_utf16, adjusted_range, window, cx)
    }

    fn replace_text_in_range(&mut self, replacement_range: Option<Range<usize>>, text: &str, window: &mut Window, cx: &mut App) {
        self.0.replace_text_in_range(replacement_range, text, window, cx)
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.replace_and_mark_text_in_range(range_utf16, new_text, new_selected_range, window, cx)
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.0.unmark_text(window, cx)
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut App) {
        self.0.paste(item, window, cx)
    }

    fn bounds_for_range(&mut self, range_utf16: Range<usize>, window: &mut Window, cx: &mut App) -> Option<Bounds<Pixels>> {
        self.0.bounds_for_range(range_utf16, window, cx)
    }

    fn selection_bounds_for_range(&mut self, range_utf16: Range<usize>, window: &mut Window, cx: &mut App) -> Vec<Bounds<Pixels>> {
        self.0.selection_bounds_for_range(range_utf16, window, cx)
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, window: &mut Window, cx: &mut App) -> Option<usize> {
        self.0.character_index_for_point(point, window, cx)
    }

    fn set_selected_text_range(&mut self, range_utf16: Range<usize>, window: &mut Window, cx: &mut App) {
        self.0.set_selected_text_range(range_utf16, window, cx)
    }

    fn element_bounds(&mut self, window: &mut Window, cx: &mut App) -> Option<Bounds<Pixels>> {
        self.0.element_bounds(window, cx)
    }

    fn text_length_utf16(&mut self, window: &mut Window, cx: &mut App) -> Option<usize> {
        self.0.text_length_utf16(window, cx)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        self.0.apple_press_and_hold_enabled()
    }

    fn accepts_text_input(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.0.accepts_text_input(window, cx)
    }

    fn text_input_editable_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.0.text_input_editable_range(window, cx)
    }

    fn text_input_configuration(&mut self, window: &mut Window, cx: &mut App) -> TextInputConfiguration {
        self.0.text_input_configuration(window, cx)
    }
}
