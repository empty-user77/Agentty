//! Chat view's running text, as a desktop chat app has it: dragged over to select, across
//! paragraphs, list items and code lines (a double click takes a word), copied with ⌘C, and with
//! its web addresses opening on a click.
//!
//! Every block of text has a place in the conversation: its entry, then its order in the entry
//! ([`begin_item`] starts an entry's count). A selection runs from one place to another; what lies
//! between is drawn with a background over each block's own highlights.

use crate::theme::{hex_alpha, Chrome};
use crate::workbench::Workbench;
use gpui::{div, prelude::*, AnyElement, App, HighlightStyle, MouseButton, StyledText, Window};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ops::Range;

/// A block's place: its chat (the pane), its entry in the conversation, then its order in the entry.
type Place = (u64, usize, usize);

/// A point in the text: a block and a byte in it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Point {
    place: Place,
    at: usize,
}

struct Selection {
    anchor: Point,
    head: Point,
    /// The mouse button is still down: moves over text extend it.
    dragging: bool,
}

thread_local! {
    static SELECTION: RefCell<Option<Selection>> = const { RefCell::new(None) };
    /// The text of each block as last drawn, for copying what the selection covers.
    static BLOCKS: RefCell<BTreeMap<Place, String>> = const { RefCell::new(BTreeMap::new()) };
    /// The entry being drawn and the next block's order in it.
    static NEXT: Cell<Place> = const { Cell::new((0, 0, 0)) };
    /// A press on text started a selection during this event (the chat clears one otherwise).
    static PRESSED_TEXT: Cell<bool> = const { Cell::new(false) };
    /// A drag (or a double click) just left something selected: the chat shows its bar.
    static JUST_SELECTED: Cell<bool> = const { Cell::new(false) };
}

/// Whether a selection was just made (once).
pub(in crate::workbench) fn take_just_selected() -> bool {
    JUST_SELECTED.with(|j| j.replace(false)) && selected_text().is_some()
}

/// Drops the selection.
pub(in crate::workbench) fn clear() {
    SELECTION.with(|s| s.borrow_mut().take());
}

/// The conversation entry the selection starts in, when it is in chat `chat`.
pub(in crate::workbench) fn selected_item(chat: u64) -> Option<usize> {
    SELECTION.with(|s| s.borrow().as_ref().map(|s| ordered(s.anchor, s.head).0.place).filter(|place| place.0 == chat).map(|place| place.1))
}

/// The text of entry `item` of chat `chat` as drawn, a block per line.
pub(in crate::workbench) fn item_text(chat: u64, item: usize) -> String {
    BLOCKS.with(|blocks| {
        blocks.borrow().range((chat, item, 0)..(chat, item + 1, 0)).map(|(_, text)| text.clone()).collect::<Vec<_>>().join("\n")
    })
}

/// The blocks drawn from here on belong to entry `item` of chat `chat` (its pane). What the entry
/// held when last drawn goes: a step closed since then draws fewer blocks.
pub(in crate::workbench) fn begin_item(chat: u64, item: usize) {
    NEXT.with(|next| next.set((chat, item, 0)));
    BLOCKS.with(|blocks| {
        let mut blocks = blocks.borrow_mut();
        let stale: Vec<Place> = blocks.range((chat, item, 0)..(chat, item + 1, 0)).map(|(place, _)| *place).collect();
        for place in stale {
            blocks.remove(&place);
        }
    });
}

/// `at`, within `text` and moved back to the start of the character it falls in.
fn char_floor(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Forgets a chat's text (its pane closed, or it went back to its terminal).
pub(in crate::workbench) fn drop_chat(chat: u64) {
    BLOCKS.with(|blocks| blocks.borrow_mut().retain(|(id, _, _), _| *id != chat));
    SELECTION.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_some_and(|s| s.anchor.place.0 == chat) {
            *s = None;
        }
    });
}

/// The selected text, the blocks it covers one per line.
pub(in crate::workbench) fn selected_text() -> Option<String> {
    let (from, to) = SELECTION.with(|s| s.borrow().as_ref().map(|s| ordered(s.anchor, s.head)))?;
    if from == to {
        return None;
    }
    let text = BLOCKS.with(|blocks| {
        blocks
            .borrow()
            .range(from.place..=to.place)
            .map(|(place, text)| {
                let start = if *place == from.place { from.at.min(text.len()) } else { 0 };
                let end = if *place == to.place { to.at.min(text.len()) } else { text.len() };
                text.get(start..end.max(start)).unwrap_or_default().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    (!text.trim().is_empty()).then_some(text)
}

/// A press anywhere in the chat, after the text's own: true when it was on text (which started a
/// selection); otherwise the selection is dropped.
pub(in crate::workbench) fn take_press() -> bool {
    if PRESSED_TEXT.with(|p| p.replace(false)) {
        return true;
    }
    SELECTION.with(|s| s.borrow_mut().take());
    false
}

/// `text` with its `runs` (sorted, not overlapping), selectable, its web addresses as links.
pub(in crate::workbench) fn selectable(text: String, runs: Vec<(Range<usize>, HighlightStyle)>) -> AnyElement {
    let place = NEXT.with(|next| {
        let place = next.get();
        next.set((place.0, place.1, place.2 + 1));
        place
    });
    BLOCKS.with(|blocks| {
        let mut blocks = blocks.borrow_mut();
        // A long conversation: keep the blocks of this chat's entries around this one only.
        if blocks.len() > 4000 {
            blocks.retain(|(chat, item, _), _| *chat == place.0 && item.abs_diff(place.1) < 40);
        }
        blocks.insert(place, text.clone());
    });
    let links = find_links(&text);
    let mut runs = runs;
    // Addresses not already styled (inside `code`, say) read as links.
    for link in &links {
        if !runs.iter().any(|(range, _)| range.start < link.end && link.start < range.end) {
            runs.push((link.clone(), HighlightStyle { color: Some(crate::theme::hex(Chrome::BLUE)), ..Default::default() }));
        }
    }
    runs.sort_by_key(|(range, _)| range.start);
    let selected = SELECTION.with(|s| {
        let s = s.borrow();
        let (from, to) = ordered(s.as_ref()?.anchor, s.as_ref()?.head);
        if place < from.place || place > to.place {
            return None;
        }
        let start = if place == from.place { from.at } else { 0 };
        let end = if place == to.place { to.at } else { text.len() };
        // Offsets kept from an earlier text at this place (a new session's) may fall inside a
        // character of this one.
        let (start, end) = (char_floor(&text, start), char_floor(&text, end));
        (start < end).then_some(start..end)
    });
    if let Some(range) = selected {
        runs = with_background(runs, range, hex_alpha(Chrome::BLUE, 0.35));
    }
    let styled = StyledText::new(text.clone()).with_highlights(runs);
    let layout = styled.layout().clone();
    let (press_layout, move_layout, up_layout) = (layout.clone(), layout.clone(), layout);
    let (press_text, move_text, up_text) = (text.clone(), text.clone(), text);
    let up_links = links;
    div()
        .id(gpui::SharedString::from(format!("chat-text-{}-{}-{}", place.0, place.1, place.2)))
        .cursor(gpui::CursorStyle::IBeam)
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            let at = index_at(&press_layout, &press_text, event.position);
            let (anchor, head) = if event.click_count == 2 { word_at(&press_text, at) } else { (at, at) };
            SELECTION.with(|s| {
                *s.borrow_mut() = Some(Selection {
                    anchor: Point { place, at: anchor },
                    head: Point { place, at: head },
                    dragging: event.click_count < 2,
                })
            });
            PRESSED_TEXT.with(|p| p.set(true));
            window.refresh();
        })
        // Over any block while the button is down: the selection reaches here.
        .on_mouse_move(move |event, window, _| {
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let moved = SELECTION.with(|s| {
                let mut s = s.borrow_mut();
                // Only within the chat the selection started in.
                let Some(s) = s.as_mut().filter(|s| s.dragging && s.anchor.place.0 == place.0) else { return false };
                let head = Point { place, at: index_at(&move_layout, &move_text, event.position) };
                let changed = s.head != head;
                s.head = head;
                changed
            });
            if moved {
                window.refresh();
            }
        })
        .on_mouse_up(MouseButton::Left, move |event, window, cx| {
            let clicked = SELECTION.with(|s| {
                let mut s = s.borrow_mut();
                let Some(s) = s.as_mut() else { return false };
                // Only the end of a drag (or a double click's word) brings up the bar.
                let was_dragging = std::mem::replace(&mut s.dragging, false);
                if s.anchor != s.head && (was_dragging || event.click_count == 2) {
                    JUST_SELECTED.with(|j| j.set(true));
                }
                s.anchor == s.head
            });
            // A click (no drag) on an address opens it.
            if clicked {
                let at = index_at(&up_layout, &up_text, event.position);
                if let Some(link) = up_links.iter().find(|link| link.contains(&at)) {
                    open_link(up_text[link.clone()].to_string(), window, cx);
                }
            }
        })
        .child(styled)
        .into_any_element()
}

fn open_link(url: String, window: &mut Window, cx: &mut App) {
    if let Some(Some(workbench)) = window.root::<Workbench>() {
        workbench.update(cx, |workbench, cx| workbench.open_link(url, cx));
    }
}

fn ordered(a: Point, b: Point) -> (Point, Point) {
    (a.min(b), a.max(b))
}

/// The text index under `position`, on a character boundary.
fn index_at(layout: &gpui::TextLayout, text: &str, position: gpui::Point<gpui::Pixels>) -> usize {
    let mut at = match layout.index_for_position(position) {
        Ok(at) | Err(at) => at.min(text.len()),
    };
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The word around `at` (letters, digits and `_`), for a double click.
fn word_at(text: &str, at: usize) -> (usize, usize) {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..at].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
    let end = text[at..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| at + i);
    (start, end)
}

/// The `http://` and `https://` addresses in `text`, without the punctuation that ends a sentence.
fn find_links(text: &str) -> Vec<Range<usize>> {
    let mut links = Vec::new();
    let mut from = 0;
    while let Some(found) = ["https://", "http://"].iter().filter_map(|scheme| text[from..].find(scheme)).min() {
        let start = from + found;
        let mut end =
            text[start..].find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`')).map_or(text.len(), |i| start + i);
        while end > start && text[..end].ends_with(['.', ',', ')', ';', ':', '!', '?', '\'']) {
            end -= 1;
        }
        if end > start + "https://".len() - 1 {
            links.push(start..end);
        }
        from = end.max(start + 1);
    }
    links
}

/// `runs` (sorted, not overlapping) with `background` over `selected`: runs split where the
/// selection starts and ends, and the unstyled stretches in it given a run of their own.
fn with_background(
    runs: Vec<(Range<usize>, HighlightStyle)>,
    selected: Range<usize>,
    background: gpui::Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts = vec![selected.start, selected.end];
    for (range, _) in &runs {
        cuts.push(range.start);
        cuts.push(range.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let piece = pair[0]..pair[1];
        if piece.is_empty() {
            continue;
        }
        let style = runs.iter().find(|(range, _)| range.start <= piece.start && piece.end <= range.end).map(|(_, style)| *style);
        let inside = selected.start <= piece.start && piece.end <= selected.end;
        match (style, inside) {
            (Some(style), true) => out.push((piece, HighlightStyle { background_color: Some(background), ..style })),
            (Some(style), false) => out.push((piece, style)),
            (None, true) => out.push((piece, HighlightStyle { background_color: Some(background), ..Default::default() })),
            (None, false) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_stop_at_sentence_ends() {
        let text = "see https://agentty.run/docs, or (http://localhost:3000).";
        let links: Vec<&str> = find_links(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(links, vec!["https://agentty.run/docs", "http://localhost:3000"]);
        assert!(find_links("no address here").is_empty());
    }

    #[test]
    fn a_double_click_takes_the_word() {
        let text = "run cargo_test now";
        assert_eq!(word_at(text, 6), (4, 14));
        assert_eq!(&text[4..14], "cargo_test");
    }

    #[test]
    fn the_selection_splits_runs() {
        let bold = HighlightStyle { font_weight: Some(gpui::FontWeight::BOLD), ..Default::default() };
        let out = with_background(vec![(2..6, bold)], 4..9, gpui::black());
        let ranges: Vec<Range<usize>> = out.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, vec![2..4, 4..6, 6..9]);
        assert!(out[0].1.background_color.is_none());
        assert!(out[1].1.background_color.is_some() && out[1].1.font_weight.is_some());
        assert!(out[2].1.background_color.is_some());
    }
}
