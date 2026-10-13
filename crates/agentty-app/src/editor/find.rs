//! Find in file (⌘F): a small bar over the editor that finds a query in the file in front, ignoring
//! case, highlights every match (`element.rs` paints them) and selects one at a time; Enter / ↓ /
//! ⌘G next, ⇧Enter / ↑ / ⇧⌘G previous, Esc closes. The bar sits outside the editor's key context,
//! so typing in it never reaches the file (⌘Z undoes the query, not an edit).

use super::buffer::{Buffer, Pos};
use super::{CodeEditor, Content};
use crate::i18n::t;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::ui::{icon_only, TypeScale};
use gpui::{actions, div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Subscription, Window};

actions!(code_editor, [FindNext, FindPrevious]);

/// The bar's own key context: steps work while typing in it, and the editor's keys don't.
const CONTEXT: &str = "EditorFind";

/// Matches counted and highlighted at most: past this a query is too common to step through, and
/// a huge file stays responsive while typing.
pub const MAX_MATCHES: usize = 10_000;

/// Where one match starts and ends, on one line.
pub type Match = (Pos, Pos);

pub fn bind_keys(cx: &mut gpui::App) {
    use crate::key;
    // ⇧⌘G opens Git everywhere else: only the bar takes it over.
    cx.bind_keys([
        key("cmd-g", FindNext, Some(CONTEXT)),
        key("cmd-shift-g", FindPrevious, Some(CONTEXT)),
        key("cmd-g", FindNext, Some(super::CONTEXT)),
    ]);
}

pub struct FindBar {
    pub(super) input: Entity<TextInput>,
    query: String,
    pub matches: Vec<(Pos, Pos)>,
    /// What the matches were found in: the file in front and its text's revision.
    searched: Option<(usize, u64)>,
    _subscription: Subscription,
}

impl CodeEditor {
    /// Opens the bar (or goes back to it), starting from the selected text when one line of it is
    /// selected.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if !matches!(doc.content, Content::Text) || doc.previewing() {
            return;
        }
        let selected = doc.buffer.selected_text();
        let seed = (!selected.is_empty() && !selected.contains('\n')).then_some(selected);
        if let Some(bar) = self.find.as_ref() {
            let input = bar.input.clone();
            input.update(cx, |input, cx| {
                if let Some(seed) = &seed {
                    input.set_text(seed.clone(), cx);
                }
                input.focus_and_select_all(window, cx);
            });
            self.find_query_changed(cx);
            return;
        }
        let input = cx.new(|cx| TextInput::localized(seed.unwrap_or_default(), "find.placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Changed => this.find_query_changed(cx),
            TextInputEvent::Confirmed | TextInputEvent::Down => this.find_step(true, cx),
            TextInputEvent::Up => this.find_step(false, cx),
            TextInputEvent::Cancelled => this.close_find(window, cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.find = Some(FindBar { input, query: String::new(), matches: Vec::new(), searched: None, _subscription: subscription });
        self.find_query_changed(cx);
    }

    /// Closes the bar; the match that was selected stays selected, in the editor that has focus again.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            window.focus(&self.focus_handle);
            cx.notify();
        }
    }

    /// Finds again when the query, the file in front or its text changed since the last search.
    pub(super) fn refresh_find(&mut self) {
        let Some(bar) = self.find.as_mut() else { return };
        let Some(doc) = self.docs.get(self.active) else {
            bar.matches.clear();
            bar.searched = None;
            return;
        };
        let key = (self.active, doc.buffer.revision());
        if bar.searched != Some(key) {
            bar.matches = find_all(&doc.buffer, &bar.query);
            bar.searched = Some(key);
        }
    }

    /// The matches to paint, and which one is selected: the one the selection covers exactly.
    pub(super) fn find_matches(&self) -> Option<(&[Match], Option<usize>)> {
        let bar = self.find.as_ref()?;
        let doc = self.docs.get(self.active)?;
        let selected = doc.buffer.selection.range();
        let current = bar.matches.binary_search_by(|m| m.0.cmp(&selected.0)).ok().filter(|&i| bar.matches[i] == selected);
        Some((&bar.matches, current))
    }

    /// Typing in the bar: the match at or after where the selection starts, so the one being
    /// typed stays selected as the query grows.
    pub(super) fn find_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(bar) = self.find.as_mut() else { return };
        bar.query = bar.input.read(cx).text().to_string();
        bar.searched = None;
        self.refresh_find();
        let Some(doc) = self.docs.get(self.active) else { return };
        let at = doc.buffer.selection.range().0;
        let target = self.find.as_ref().and_then(|bar| next_from(&bar.matches, at, true));
        self.select_match(target, cx);
    }

    pub(super) fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.refresh_find();
        let Some((matches, current)) = self.find_matches() else { return };
        let Some(doc) = self.docs.get(self.active) else { return };
        let at = if forward { doc.buffer.selection.range().1 } else { doc.buffer.selection.range().0 };
        let target = step(matches, current, at, forward);
        self.select_match(target, cx);
    }

    fn select_match(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        let Some((start, end)) = index.and_then(|i| self.find.as_ref()?.matches.get(i).copied()) else {
            cx.notify();
            return;
        };
        self.motion(cx, |buffer| {
            buffer.move_to(start, false);
            buffer.move_to(end, true);
        });
    }

    pub(super) fn render_find_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.refresh_find();
        let (matches, current) = self.find_matches()?;
        let query_empty = self.find.as_ref()?.query.is_empty();
        let count = match (matches.len(), current) {
            _ if query_empty => String::new(),
            (0, _) => t(cx, "find.none").to_string(),
            (total, Some(index)) => format!("{}/{total}{}", index + 1, if total >= MAX_MATCHES { "+" } else { "" }),
            (total, None) => format!("{total}{}", if total >= MAX_MATCHES { "+" } else { "" }),
        };
        let input = self.find.as_ref()?.input.clone();
        Some(
            div()
                .id("editor-find")
                .key_context(CONTEXT)
                .absolute()
                .top(px(41.))
                .right(px(18.))
                .w(px(340.))
                .max_w(gpui::relative(0.9))
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .gap_1()
                .rounded_md()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .shadow_lg()
                .occlude()
                .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find_step(true, cx)))
                .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.find_step(false, cx)))
                // ⇧Enter is the field's own (a new line in a multi-line field): taken before it.
                .capture_action(cx.listener(|this, _: &crate::text_input::NewLine, _, cx| {
                    this.find_step(false, cx);
                    cx.stop_propagation();
                }))
                .child(div().flex_1().min_w_0().t_small().text_color(hex(Chrome::BRIGHT)).child(input))
                .child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(count))
                .child(
                    icon_only("editor-find-prev", "chevron-up", cx.listener(|this, _: &ClickEvent, _, cx| this.find_step(false, cx)))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.find_prev"), Some("⇧↩"))),
                )
                .child(
                    icon_only("editor-find-next", "chevron-down", cx.listener(|this, _: &ClickEvent, _, cx| this.find_step(true, cx)))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.find_next"), Some("↩"))),
                )
                .child(
                    icon_only("editor-find-close", "x", cx.listener(|this, _: &ClickEvent, window, cx| this.close_find(window, cx)))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "confirm.close"), Some("esc"))),
                )
                .into_any_element(),
        )
    }
}

/// Where `query` appears in the buffer, in document order, as (start, end) on one line each.
/// Case is ignored; matches don't overlap. An empty query, or one spanning lines, finds nothing.
pub fn find_all(buffer: &Buffer, query: &str) -> Vec<(Pos, Pos)> {
    if query.is_empty() || query.contains('\n') {
        return Vec::new();
    }
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut found = Vec::new();
    for index in 0..buffer.line_count() {
        for (start, end) in find_in_line(buffer.line(index), &needle) {
            found.push((Pos::new(index, start), Pos::new(index, end)));
            if found.len() >= MAX_MATCHES {
                return found;
            }
        }
    }
    found
}

/// Byte ranges of `needle` (already lowercased) in `line`, compared a lowercased character at a
/// time so a match's ends are always character boundaries of the line itself.
fn find_in_line(line: &str, needle: &[char]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let starts: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
    let mut from = 0;
    while from < starts.len() {
        let start = starts[from];
        let mut wanted = needle.iter();
        let mut end = None;
        for (offset, c) in line[start..].char_indices() {
            let mut lower = c.to_lowercase();
            if !lower.all(|l| wanted.next() == Some(&l)) {
                break;
            }
            if wanted.as_slice().is_empty() {
                end = Some(start + offset + c.len_utf8());
                break;
            }
        }
        match end {
            Some(end) => {
                found.push((start, end));
                from = starts.partition_point(|&s| s < end);
            }
            None => from += 1,
        }
    }
    found
}

/// The match to go to from `at`: the first one starting at or after it going forward, the last
/// one ending at or before it going back, wrapping around the file.
pub fn next_from(matches: &[(Pos, Pos)], at: Pos, forward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    Some(if forward {
        let index = matches.partition_point(|(start, _)| *start < at);
        if index == matches.len() {
            0
        } else {
            index
        }
    } else {
        let index = matches.partition_point(|(_, end)| *end <= at);
        if index == 0 {
            matches.len() - 1
        } else {
            index - 1
        }
    })
}

/// One step from match `current` (wrapping), or the nearest to `at` when none is current.
pub fn step(matches: &[(Pos, Pos)], current: Option<usize>, at: Pos, forward: bool) -> Option<usize> {
    let count = matches.len();
    match current {
        _ if count == 0 => None,
        Some(index) if index < count => Some(if forward { (index + 1) % count } else { (index + count - 1) % count }),
        _ => next_from(matches, at, forward),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(text: &str, query: &str) -> Vec<((usize, usize), (usize, usize))> {
        find_all(&Buffer::new(text), query).into_iter().map(|(s, e)| ((s.line, s.col), (e.line, e.col))).collect()
    }

    #[test]
    fn finds_every_match_ignoring_case() {
        assert_eq!(ranges("Foo foo\nbar FOO", "foo"), vec![((0, 0), (0, 3)), ((0, 4), (0, 7)), ((1, 4), (1, 7))]);
        assert!(ranges("foo", "").is_empty());
        assert!(ranges("foo\nbar", "o\nb").is_empty(), "a query over two lines finds nothing");
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(ranges("aaaa", "aa"), vec![((0, 0), (0, 2)), ((0, 2), (0, 4))]);
    }

    #[test]
    fn multibyte_text_gives_byte_ranges_on_character_boundaries() {
        // Each Hangul syllable is three bytes.
        assert_eq!(ranges("안녕 세상, 세상아", "세상"), vec![((0, 7), (0, 13)), ((0, 15), (0, 21))]);
        assert_eq!(ranges("ÉCOLE école", "école"), vec![((0, 0), (0, 6)), ((0, 7), (0, 13))]);
    }

    #[test]
    fn stepping_wraps_around() {
        let matches = find_all(&Buffer::new("x\nfoo\nx foo\nfoo"), "foo");
        assert_eq!(matches.len(), 3);
        assert_eq!(step(&matches, Some(2), Pos::default(), true), Some(0));
        assert_eq!(step(&matches, Some(0), Pos::default(), false), Some(2));
        assert_eq!(step(&matches, Some(1), Pos::default(), true), Some(2));
        assert_eq!(step(&[], Some(1), Pos::default(), true), None);
    }

    #[test]
    fn starts_from_the_cursor() {
        let matches = find_all(&Buffer::new("foo\nfoo\nfoo"), "foo");
        assert_eq!(next_from(&matches, Pos::new(1, 1), true), Some(2));
        assert_eq!(next_from(&matches, Pos::new(1, 0), true), Some(1));
        assert_eq!(next_from(&matches, Pos::new(2, 2), true), Some(0), "past the last one: back to the top");
        assert_eq!(next_from(&matches, Pos::new(1, 1), false), Some(0));
        assert_eq!(next_from(&matches, Pos::new(0, 0), false), Some(2), "before the first one: from the bottom");
    }

    #[test]
    fn matches_follow_an_edit() {
        let mut buffer = Buffer::new("one two\ntwo");
        assert_eq!(find_all(&buffer, "two").len(), 2);
        buffer.select_all();
        buffer.insert("two two two");
        assert_eq!(find_all(&buffer, "two").len(), 3);
    }
}
