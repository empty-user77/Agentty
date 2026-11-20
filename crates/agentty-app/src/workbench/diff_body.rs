//! The lines of a diff, as both places that show one draw them (a changed file in the editor's
//! place, AgentGit's page): dragged over to select, copied with ⌘C, and a bar where the selection
//! ends offering what the file editor's does, *Ask AI* and *Copy*.
//!
//! The gutters and the `+` / `-` column are not text: a selection only ever holds the code. What an
//! instruction names depends on what was selected. Lines that are in the file now are named by
//! their numbers, as the editor does; a removed line is not in the file to read, and a commit's
//! lines may not be there either, so those go along with the prompt.

use super::chat::selectable::word_at;
use super::Workbench;
use crate::editor::agent::{ask_prompt, AskOnly, ASK};
use crate::i18n::t;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use agentty_bridge::git::{DiffLine, LineKind};
use gpui::{
    div, prelude::*, px, AnyElement, Bounds, ClickEvent, ClipboardItem, Context, Entity, FocusHandle, HighlightStyle, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, StyledText, Subscription, Window,
};
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

/// A row's height in the list.
const ROW: f32 = 20.;
/// The text's size: what a press is measured against has to be what is drawn.
const TEXT: f32 = 12.;
/// The column between the line numbers and the code.
const SIGN: f32 = 16.;
/// The floating scrollbar's strip at the right edge: a press there is for the bar.
const SCROLLBAR: f32 = 14.;
/// How far a drag past the list's edge counts as all the way (the scroll is at its fastest there).
const SCROLL_EDGE: f32 = 40.;
/// Rows a tick scrolls at its fastest.
const SCROLL_ROWS: f32 = 1.5;
const SCROLL_TICK: Duration = Duration::from_millis(30);
/// How close to the code's left or right edge a drag has to come for the lines to move sideways,
/// and how far they move a tick at their fastest.
const SIDE_EDGE: f32 = 28.;
const SIDE_STEP: f32 = 28.;
/// The most diff lines a prompt carries: the rest is named, not pasted.
const PROMPT_ROWS: usize = 200;

gpui::actions!(diff_body, [DiffCopy, DiffSelectAll, DiffAsk, DiffClear]);

/// A diff's key context.
pub(crate) const CONTEXT: &str = "DiffBody";

/// A place in the text as drawn: a row and a byte in it (tabs drawn as four spaces).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Place {
    row: usize,
    at: usize,
}

/// An instruction being typed. It holds what it is about: the diff may be read again meanwhile.
struct AskBox {
    input: Entity<TextInput>,
    rows: Vec<DiffLine>,
    at: Point<Pixels>,
    agent: Option<String>,
    _subscription: Subscription,
}

/// What a diff shows, and what is selected in it.
pub(crate) struct DiffBody {
    lines: Rc<Vec<DiffLine>>,
    /// The file, from the root of its repository.
    rel: String,
    /// The commit these lines are from; `None` for what is uncommitted.
    commit: Option<String>,
    focus: FocusHandle,
    scroll: gpui::UniformListScrollHandle,
    /// How far the code is moved to the left, for lines longer than the view is wide: the strip
    /// under the list scrolls it, and the bar at the bottom is its bar.
    side: gpui::ScrollHandle,
    /// The widest line as drawn, for the lines and font it was measured with.
    widest: Option<(usize, SharedString, f32)>,
    /// Where the list is on screen, as last painted.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Where the code starts in a row, as last drawn.
    text_left: f32,
    font: SharedString,
    anchor: Option<Place>,
    head: Option<Place>,
    dragging: bool,
    /// Where the pointer last was in a drag, and how far a tick scrolls (rows, up is negative).
    pointer: Point<Pixels>,
    scroll_by: f32,
    /// How far a tick moves the lines sideways (pixels, left is negative).
    side_by: f32,
    scrolling: Option<gpui::Task<()>>,
    /// The pointer's x when the selection was made: the bar comes up there.
    bar_x: Pixels,
    ask: Option<AskBox>,
}

fn drawn(text: &str) -> String {
    text.replace('\t', "    ")
}

/// `at`, a byte in the drawn text, as a byte in `text` (inside a tab's spaces is the tab).
fn source_at(text: &str, at: usize) -> usize {
    let mut shown = 0;
    for (index, c) in text.char_indices() {
        let width = if c == '\t' { 4 } else { c.len_utf8() };
        if shown + width > at {
            return index;
        }
        shown += width;
    }
    text.len()
}

fn floor_char(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn is_code(line: &DiffLine) -> bool {
    matches!(line.kind, LineKind::Context | LineKind::Added | LineKind::Removed)
}

/// What a selection copies, a line per row. A hunk's header between lines of code is not part of
/// them; on its own it is what was selected.
fn selected_text(lines: &[DiffLine], from: Place, to: Place) -> String {
    let last = to.row.min(lines.len().saturating_sub(1));
    let Some(rows) = lines.get(from.row..=last) else { return String::new() };
    let code = rows.iter().any(is_code);
    rows.iter()
        .enumerate()
        .filter(|(_, line)| !code || is_code(line))
        .map(|(index, line)| {
            let row = from.row + index;
            let start = if row == from.row { source_at(&line.text, from.at) } else { 0 };
            let end = if row == to.row { source_at(&line.text, to.at) } else { line.text.len() };
            line.text.get(start..end.max(start)).unwrap_or_default().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The lines of code an instruction is about. A selection ending at the very start of a row does
/// not take that row.
fn selected_rows(lines: &[DiffLine], from: Place, to: Place) -> Vec<DiffLine> {
    let last = if to.at == 0 && to.row > from.row { to.row - 1 } else { to.row };
    lines.get(from.row..=last.min(lines.len().saturating_sub(1))).unwrap_or_default().iter().filter(|line| is_code(line)).cloned().collect()
}

/// The numbers in the file now of `rows` (first, last), from 1.
fn new_range(rows: &[DiffLine]) -> Option<(u32, u32)> {
    let numbers = || rows.iter().filter_map(|line| line.new);
    Some((numbers().min()?, numbers().max()?))
}

/// `rows` as the lines of a diff, without running on forever.
fn excerpt(rows: &[DiffLine]) -> String {
    let mut text: Vec<String> = rows
        .iter()
        .take(PROMPT_ROWS)
        .map(|line| {
            let sign = match line.kind {
                LineKind::Added => '+',
                LineKind::Removed => '-',
                _ => ' ',
            };
            format!("{sign}{}", line.text)
        })
        .collect();
    if rows.len() > PROMPT_ROWS {
        text.push(format!("... ({} more lines)", rows.len() - PROMPT_ROWS));
    }
    text.join("\n")
}

/// What is sent to the agent, in English so any agent reads it the same way (the instruction itself
/// stays in the user's words).
fn diff_prompt(rel: &str, rows: &[DiffLine], commit: Option<&str>, instruction: &str, only_answer: bool) -> String {
    let instruction = instruction.trim();
    let removed = rows.iter().any(|line| line.kind == LineKind::Removed);
    let new = new_range(rows);
    let mut prompt = match (commit, removed, new) {
        (Some(commit), _, _) => format!("In {rel}, about this part of the diff of commit {commit}: {instruction}"),
        (None, false, Some((first, last))) => ask_prompt(rel, (first as usize - 1, last as usize - 1), instruction),
        (None, _, Some((first, last))) => {
            let lines = if first == last { format!("line {first}") } else { format!("lines {first}-{last}") };
            format!("In {rel}, about this part of its uncommitted diff ({lines} of the file now): {instruction}")
        }
        (None, _, None) => format!("In {rel}, about these lines its uncommitted diff removes: {instruction}"),
    };
    if only_answer {
        prompt.push_str(" (Just answer; do not change any files.)");
    }
    if commit.is_some() || removed || new.is_none() {
        prompt.push_str(&format!("\n\n````diff\n{}\n````", excerpt(rows)));
    }
    prompt
}

/// The rows' place, for the box's heading: the file's numbers, or the old ones of removed lines.
fn rows_label(rows: &[DiffLine]) -> String {
    let span = |first: u32, last: u32| if first == last { first.to_string() } else { format!("{first}–{last}") };
    if let Some((first, last)) = new_range(rows) {
        return format!("L{}", span(first, last));
    }
    let old = || rows.iter().filter_map(|line| line.old);
    match (old().min(), old().max()) {
        (Some(first), Some(last)) => format!("−L{}", span(first, last)),
        _ => String::new(),
    }
}

/// One row of a diff, with what is selected of its text and whether the selection runs on past it.
fn diff_row(line: &DiffLine, gutter: f32, font: SharedString, selected: Option<Range<usize>>, past_end: bool, side: f32) -> AnyElement {
    let (bg, fg, sign) = match line.kind {
        LineKind::Added => (Some(hex_alpha(0x2ea043, 0.16)), Chrome::FOREGROUND, "+"),
        LineKind::Removed => (Some(hex_alpha(0xf85149, 0.16)), Chrome::FOREGROUND, "-"),
        LineKind::Hunk => (Some(hex_alpha(0x388bfd, 0.12)), Chrome::MUTED, ""),
        LineKind::Meta => (None, Chrome::MUTED, ""),
        LineKind::Context => (None, Chrome::FOREGROUND, " "),
    };
    let number = |n: Option<u32>| {
        div().w(px(gutter)).flex_shrink_0().pr_2().flex().justify_end().text_color(hex(0x6e7681)).children(n.map(|n| n.to_string()))
    };
    let color = hex_alpha(Chrome::BLUE, 0.35);
    let highlight = HighlightStyle { background_color: Some(color), ..Default::default() };
    let text = StyledText::new(drawn(&line.text)).with_highlights(selected.into_iter().map(|range| (range, highlight)));
    div()
        .h(px(ROW))
        .w_full()
        .flex()
        .items_center()
        .font_family(font)
        .text_size(px(TEXT))
        .whitespace_nowrap()
        .overflow_hidden()
        .when_some(bg, |d, bg| d.bg(bg))
        .child(number(line.old))
        .child(number(line.new))
        .child(div().w(px(SIGN)).flex_shrink_0().text_color(hex(Chrome::MUTED)).child(sign))
        .child(
            // The code alone moves sideways: the numbers and the sign stay where they are.
            div().flex_1().min_w_0().h_full().flex().overflow_hidden().cursor(gpui::CursorStyle::IBeam).text_color(hex(fg)).child(
                div()
                    .flex_shrink_0()
                    .h_full()
                    .ml(px(-side))
                    .flex()
                    .items_center()
                    .child(text)
                    // The end of the line is selected too, which an empty line otherwise doesn't show.
                    .when(past_end, |d| d.child(div().flex_shrink_0().w(px(6.)).h_full().bg(color))),
            ),
        )
        .into_any_element()
}

impl DiffBody {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            lines: Rc::default(),
            rel: String::new(),
            commit: None,
            focus: cx.focus_handle(),
            scroll: gpui::UniformListScrollHandle::new(),
            side: gpui::ScrollHandle::new(),
            widest: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            text_left: 0.,
            font: SharedString::default(),
            anchor: None,
            head: None,
            dragging: false,
            pointer: Point::default(),
            scroll_by: 0.,
            side_by: 0.,
            scrolling: None,
            bar_x: px(0.),
            ask: None,
        }
    }

    /// Shows `lines` of `rel`. The same file read again keeps its place, and its selection while
    /// the lines are what they were.
    pub(crate) fn set_lines(&mut self, lines: Rc<Vec<DiffLine>>, rel: String, commit: Option<String>, cx: &mut Context<Self>) {
        if Rc::ptr_eq(&self.lines, &lines) && self.rel == rel && self.commit == commit {
            return;
        }
        let same_file = self.rel == rel && self.commit == commit;
        if !same_file {
            self.scroll = gpui::UniformListScrollHandle::new();
            self.side = gpui::ScrollHandle::new();
            self.ask = None;
        }
        if !same_file || *self.lines != *lines {
            self.anchor = None;
            self.head = None;
            self.dragging = false;
        }
        self.lines = lines;
        self.rel = rel;
        self.commit = commit;
        cx.notify();
    }

    /// The selection in order, when it holds anything.
    fn range(&self) -> Option<(Place, Place)> {
        let (anchor, head) = (self.anchor?, self.head?);
        (anchor != head).then_some((anchor.min(head), anchor.max(head)))
    }

    fn base(&self) -> gpui::ScrollHandle {
        self.scroll.0.borrow().base_handle.clone()
    }

    /// How far the code is moved to the left now.
    fn side_offset(&self) -> f32 {
        -f32::from(self.side.offset().x)
    }

    /// Moves the code sideways by `by` pixels (right is positive), as far as its widest line goes.
    /// Whether it moved.
    fn scroll_side(&mut self, by: f32) -> bool {
        let offset = self.side.offset();
        let x = (f32::from(offset.x) - by).clamp(-f32::from(self.side.max_offset().x), 0.);
        if px(x) == offset.x {
            return false;
        }
        self.side.set_offset(gpui::point(px(x), offset.y));
        true
    }

    /// The width of the widest line as drawn, measured once for these lines in this font.
    fn widest_line(&mut self, window: &mut Window) -> f32 {
        let key = Rc::as_ptr(&self.lines) as usize;
        if let Some((_, _, width)) = self.widest.as_ref().filter(|(lines, font, _)| *lines == key && *font == self.font) {
            return *width;
        }
        // The longest few by characters and by bytes: one of them is the widest (wide characters
        // take more room than their count says, and more bytes).
        let mut by_chars: Vec<&DiffLine> = self.lines.iter().collect();
        by_chars.sort_by_key(|line| std::cmp::Reverse(line.text.chars().count()));
        let mut by_bytes: Vec<&DiffLine> = self.lines.iter().collect();
        by_bytes.sort_by_key(|line| std::cmp::Reverse(line.text.len()));
        let mut style = window.text_style();
        style.font_family = self.font.clone();
        let mut width = 0f32;
        for line in by_chars.into_iter().take(4).chain(by_bytes.into_iter().take(4)) {
            let text = drawn(&line.text);
            if text.is_empty() {
                continue;
            }
            let run = style.to_run(text.len());
            width = width.max(f32::from(window.text_system().shape_line(text.into(), px(TEXT), &[run], None).width));
        }
        self.widest = Some((key, self.font.clone(), width));
        width
    }

    /// The place under `position`. Past the list's ends is its first or last place, so a drag can
    /// leave the list and still reach them.
    fn place_at(&self, position: Point<Pixels>, window: &mut Window) -> Option<Place> {
        let bounds = self.bounds.get();
        let y = f32::from(position.y - bounds.origin.y - self.base().offset().y);
        if self.lines.is_empty() {
            return None;
        }
        if y < 0. {
            return Some(Place { row: 0, at: 0 });
        }
        let row = (y / ROW) as usize;
        if row >= self.lines.len() {
            let row = self.lines.len() - 1;
            return Some(Place { row, at: drawn(&self.lines[row].text).len() });
        }
        let text = drawn(&self.lines[row].text);
        let x = f32::from(position.x - bounds.origin.x) - self.text_left + self.side_offset();
        if x <= 0. || text.is_empty() {
            return Some(Place { row, at: 0 });
        }
        let mut style = window.text_style();
        style.font_family = self.font.clone();
        let run = style.to_run(text.len());
        let shaped = window.text_system().shape_line(text.clone().into(), px(TEXT), &[run], None);
        Some(Place { row, at: floor_char(&text, shaped.closest_index_for_x(px(x))) })
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.ask = None;
        let bounds = self.bounds.get();
        if f32::from(event.position.x) >= f32::from(bounds.origin.x + bounds.size.width) - SCROLLBAR {
            return cx.notify();
        }
        let Some(place) = self.place_at(event.position, window) else { return };
        let text = drawn(&self.lines[place.row].text);
        self.bar_x = event.position.x;
        match event.click_count {
            2 => {
                let (start, end) = word_at(&text, place.at);
                self.anchor = Some(Place { row: place.row, at: start });
                self.head = Some(Place { row: place.row, at: end });
            }
            3.. => {
                self.anchor = Some(Place { row: place.row, at: 0 });
                self.head = Some(Place { row: place.row, at: text.len() });
            }
            _ => {
                // ⇧ moves the end of what is selected.
                if !(event.modifiers.shift && self.anchor.is_some()) {
                    self.anchor = Some(place);
                }
                self.head = Some(place);
                self.dragging = true;
            }
        }
        cx.notify();
    }

    fn drag(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        // The button came up where nothing told us.
        if event.pressed_button != Some(MouseButton::Left) {
            return self.end_drag(event.position, cx);
        }
        self.pointer = event.position;
        self.move_head(window, cx);
        let bounds = self.bounds.get();
        let (y, top, bottom) = (f32::from(event.position.y), f32::from(bounds.origin.y), f32::from(bounds.origin.y + bounds.size.height));
        self.scroll_by = if y < top {
            -((top - y) / SCROLL_EDGE).clamp(0.2, 1.) * SCROLL_ROWS
        } else if y > bottom {
            ((y - bottom) / SCROLL_EDGE).clamp(0.2, 1.) * SCROLL_ROWS
        } else {
            0.
        };
        // Sideways too: near the code's right edge the lines move left under the pointer, near
        // its left edge back, so a drag reaches the end of a line longer than the view.
        let (x, left, right) = (
            f32::from(event.position.x),
            f32::from(bounds.origin.x) + self.text_left,
            f32::from(bounds.origin.x + bounds.size.width) - SCROLLBAR,
        );
        self.side_by = if x > right - SIDE_EDGE {
            ((x - (right - SIDE_EDGE)) / SIDE_EDGE).clamp(0.2, 1.) * SIDE_STEP
        } else if x < left + SIDE_EDGE {
            -(((left + SIDE_EDGE) - x) / SIDE_EDGE).clamp(0.2, 1.) * SIDE_STEP
        } else {
            0.
        };
        if self.scroll_by == 0. && self.side_by == 0. {
            self.scrolling = None;
        } else if self.scrolling.is_none() {
            let handle = window.window_handle();
            // Moves stop arriving once the pointer holds still past the edge.
            self.scrolling = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(SCROLL_TICK).await;
                let running = handle.update(cx, |_, window, cx| this.update(cx, |this, cx| this.scroll_tick(window, cx)).unwrap_or(false));
                if !running.unwrap_or(false) {
                    break;
                }
            }));
        }
    }

    fn scroll_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.dragging || (self.scroll_by == 0. && self.side_by == 0.) {
            return false;
        }
        let base = self.base();
        let offset = base.offset();
        let y = (f32::from(offset.y) - self.scroll_by * ROW).clamp(-f32::from(base.max_offset().y), 0.);
        let moved_down = px(y) != offset.y;
        base.set_offset(gpui::point(offset.x, px(y)));
        let moved_aside = self.side_by != 0. && self.scroll_side(self.side_by);
        // At the end both ways: nothing to draw until the pointer moves again (the ticks go on
        // while it is held at the edge, as before, and end with the drag).
        if moved_down || moved_aside {
            self.move_head(window, cx);
            cx.notify();
        }
        true
    }

    fn move_head(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let place = self.place_at(self.pointer, window);
        if place.is_some() && self.head != place {
            self.head = place;
            cx.notify();
        }
    }

    fn end_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.dragging = false;
        self.scroll_by = 0.;
        self.side_by = 0.;
        self.scrolling = None;
        self.bar_x = position.x;
        // Linux: what the pointer selected is what a middle click pastes.
        if let Some((from, to)) = self.range() {
            crate::platform::primary::set(selected_text(&self.lines, from, to), cx);
        }
        cx.notify();
    }

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((from, to)) = self.range() else { return };
        let text = selected_text(&self.lines, from, to);
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if let Some(Some(workbench)) = window.root::<Workbench>() {
            workbench.update(cx, |workbench, cx| workbench.note_copied(cx));
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(last) = self.lines.len().checked_sub(1) {
            self.anchor = Some(Place { row: 0, at: 0 });
            self.head = Some(Place { row: last, at: drawn(&self.lines[last].text).len() });
            let bounds = self.bounds.get();
            self.bar_x = bounds.origin.x + px(self.text_left);
            cx.notify();
        }
    }

    /// Where the bar or box goes: under the selection's last row, when that is in sight.
    fn point_below(&self, row: usize) -> Option<Point<Pixels>> {
        let bounds = self.bounds.get();
        let y = bounds.origin.y + self.base().offset().y + px((row + 1) as f32 * ROW);
        let (top, bottom) = (bounds.origin.y, bounds.origin.y + bounds.size.height);
        (y >= top && y <= bottom).then_some(gpui::point(self.bar_x, y))
    }

    fn open_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((from, to)) = self.range() else { return };
        let rows = selected_rows(&self.lines, from, to);
        if rows.is_empty() {
            return;
        }
        // Everything was selected and its end is out of sight: the box stays in the list.
        let bounds = self.bounds.get();
        let at = self.point_below(to.row).unwrap_or_else(|| gpui::point(self.bar_x, bounds.origin.y + px(ROW)));
        let agent = window.root::<Workbench>().flatten().and_then(|workbench| workbench.read(cx).linked_label(cx));
        let input = cx.new(|cx| TextInput::localized("", "editor.ask_placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed => this.send_ask(false, window, cx),
            TextInputEvent::Cancelled => this.close_ask(window, cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.ask = Some(AskBox { input, rows, at, agent, _subscription: subscription });
        cx.notify();
    }

    fn close_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ask.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Sends the instruction; `only_answer` asks without letting the agent change files (⌥↩).
    fn send_ask(&mut self, only_answer: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ask) = self.ask.as_ref() else { return };
        let instruction = ask.input.read(cx).text().trim().to_string();
        if instruction.is_empty() {
            return;
        }
        let prompt = diff_prompt(&self.rel, &ask.rows, self.commit.as_deref(), &instruction, only_answer);
        let Some(Some(workbench)) = window.root::<Workbench>() else { return };
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "diff_ask_agent" }));
        // Nowhere to send it and nowhere to pick from: what was typed stays in the box.
        if workbench.update(cx, |workbench, cx| workbench.ask_from_diff(prompt, window, cx)) {
            // As in the editor: it was sent, so the bar has nothing more to offer about it.
            self.anchor = None;
            self.head = None;
            self.close_ask(window, cx);
        }
    }

    fn render_ask(&self, ask: &AskBox, cx: &mut Context<Self>) -> AnyElement {
        let panel = div()
            .id("diff-ask")
            .key_context(ASK)
            .on_action(cx.listener(|this, _: &AskOnly, window, cx| this.send_ask(true, window, cx)))
            .w(px(460.))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_lg()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex_alpha(Chrome::ACCENT, 0.7))
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .t_caption()
                    .text_color(hex(Chrome::MUTED))
                    .child(icon("sparkles", IconSize::INLINE, hex(Chrome::ACCENT)))
                    .child(rows_label(&ask.rows))
                    .child(div().flex_1())
                    .child(match &ask.agent {
                        Some(label) => crate::i18n::tf(cx, "editor.ask_to", &[("agent", label)]),
                        None => t(cx, "editor.ask_no_session").to_string(),
                    }),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(hex(Chrome::EDITOR))
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .t_body()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(ask.input.clone()),
            )
            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "editor.ask_keys")));
        gpui::deferred(gpui::anchored().position(ask.at).snap_to_window_with_margin(px(8.)).child(panel))
            .with_priority(2)
            .into_any_element()
    }

    /// The bar under a selection: what the file editor's offers that a diff has a use for.
    fn render_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.dragging {
            return None;
        }
        let (from, to) = self.range()?;
        let at = self.point_below(to.row)?;
        let can_ask = !selected_rows(&self.lines, from, to).is_empty();
        let button = |id: &'static str, glyph: &'static str, label: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .t_small()
                .text_color(hex(Chrome::BRIGHT))
                .child(icon(glyph, IconSize::INLINE, hex(Chrome::FOREGROUND)))
                .child(label)
        };
        let bar = div()
            .id("diff-selection-bar")
            .flex()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded_lg()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(can_ask, |d| {
                d.child(
                    button("diff-sel-ask", "sparkles", t(cx, "editor.ask_ai").to_string())
                        .text_color(hex(Chrome::ACCENT))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_ask(window, cx))),
                )
            })
            .child(
                button("diff-sel-copy", "copy", t(cx, "editor.copy").to_string())
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.copy(window, cx))),
            );
        Some(
            gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(px(8.)).child(bar)).with_priority(2).into_any_element(),
        )
    }

    /// For the debug driver: what is selected, as a copy would take it, and the prompt it would make.
    pub(crate) fn debug_state(&self) -> serde_json::Value {
        let range = self.range();
        serde_json::json!({
            "rows": self.lines.len(),
            "selected": range.map(|(from, to)| selected_text(&self.lines, from, to)),
            "from": range.map(|(from, _)| from.row),
            "to": range.map(|(_, to)| to.row),
            "dragging": self.dragging,
            "asking": self.ask.is_some(),
            "scroll": f32::from(self.base().offset().y),
            "side": self.side_offset(),
            "side_max": f32::from(self.side.max_offset().x),
            "prompt": range.map(|(from, to)| {
                diff_prompt(&self.rel, &selected_rows(&self.lines, from, to), self.commit.as_deref(), "INSTRUCTION", false)
            }),
        })
    }
}

impl Render for DiffBody {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.lines.clone();
        let font: SharedString = crate::settings::terminal_font(cx).into();
        self.font = font.clone();
        let gutter = rows.iter().filter_map(|l| l.old.max(l.new)).max().unwrap_or(0).to_string().len().max(3) as f32 * 8. + 12.;
        self.text_left = gutter * 2. + SIGN;
        let widest = self.widest_line(window);
        let side = self.side_offset();
        let side_handle = self.side.clone();
        let text_left = self.text_left;
        let selection = self.range();
        let handle = self.scroll.clone();
        let base = self.base();
        let bounds = self.bounds.clone();
        let dragging = self.dragging;
        let this = cx.entity().downgrade();
        div()
            .relative()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .group(crate::ui::SCROLL_GROUP)
            .on_action(cx.listener(|this, _: &DiffCopy, window, cx| this.copy(window, cx)))
            .on_action(cx.listener(|this, _: &DiffSelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &DiffAsk, window, cx| this.open_ask(window, cx)))
            .on_action(cx.listener(|this, _: &DiffClear, _, cx| {
                this.anchor = None;
                this.head = None;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    use crate::context_menu::Item;
                    window.focus(&this.focus, cx);
                    let focus = this.focus.clone();
                    let item = |label: &'static str, action: fn() -> Box<dyn gpui::Action>| {
                        let focus = focus.clone();
                        Item::new(crate::i18n::t(cx, label), move |window, cx| {
                            window.focus(&focus, cx);
                            window.dispatch_action(action(), cx);
                        })
                    };
                    let items = vec![
                        item("input.copy", || Box::new(DiffCopy)).enabled(this.range().is_some()),
                        item("input.select_all", || Box::new(DiffSelectAll)),
                    ];
                    crate::context_menu::open(event.position, items, window, cx);
                    cx.stop_propagation();
                }),
            )
            // A quick click comes up before the drag's own listeners below are painted.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    if this.dragging {
                        this.end_drag(event.position, cx);
                    }
                }),
            )
            .child(
                gpui::uniform_list("diff-lines", rows.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let line = &rows[i];
                            let text = selection.filter(|(from, to)| from.row <= i && i <= to.row).map(|(from, to)| {
                                let length = drawn(&line.text).len();
                                let start = if i == from.row { from.at.min(length) } else { 0 };
                                let end = if i == to.row { to.at.min(length) } else { length };
                                (start..end, i < to.row)
                            });
                            let (selected, past_end) = match text {
                                Some((range, past_end)) => ((!range.is_empty()).then_some(range), past_end),
                                None => (None, false),
                            };
                            diff_row(line, gutter, font.clone(), selected, past_end, side)
                        })
                        .collect::<Vec<_>>()
                })
                .track_scroll(&handle)
                .size_full(),
            )
            .child(
                gpui::canvas(
                    |_, _, _| {},
                    move |area, _, window, _| {
                        bounds.set(area);
                        // A sideways scroll (a trackpad's, or the wheel with Shift) moves the code. It is
                        // taken before the list sees it: the list would scroll up or down by it.
                        let scrolled = this.clone();
                        window.on_mouse_event(move |event: &gpui::ScrollEvent, phase, _, cx| {
                            let delta = event.delta.pixel_delta(px(ROW));
                            if phase.capture() && area.contains(&event.position) && delta.x.abs() > delta.y.abs() {
                                let _ = scrolled.update(cx, |this, cx| {
                                    if this.scroll_side(-f32::from(delta.x)) {
                                        cx.notify();
                                    }
                                });
                                cx.stop_propagation();
                            }
                        });
                        if !dragging {
                            return;
                        }
                        // A drag goes on over anything, and past the window's edge.
                        let moved = this.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase.bubble() {
                                let _ = moved.update(cx, |this, cx| this.drag(event, window, cx));
                            }
                        });
                        let released = this.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase.bubble() && event.button == MouseButton::Left {
                                let _ = released.update(cx, |this, cx| this.end_drag(event.position, cx));
                            }
                        });
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(crate::ui::scrollbar(base))
            // What the sideways scroll is measured in: a strip as wide as the code's column, holding
            // something as wide as the widest line. Its bar is drawn over the list's bottom edge.
            .child(
                div()
                    .id("diff-side")
                    .absolute()
                    .left(px(text_left))
                    .right(px(SCROLLBAR))
                    .bottom_0()
                    .h(px(1.))
                    .overflow_x_scroll()
                    .track_scroll(&side_handle)
                    .child(div().flex_shrink_0().h(px(1.)).w(px(widest + 24.))),
            )
            .child(crate::ui::scrollbar_h(side_handle))
            .children(self.ask.as_ref().map(|ask| self.render_ask(ask, cx)).or_else(|| self.render_bar(cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine { kind, old, new, text: text.to_string() }
    }

    fn sample() -> Vec<DiffLine> {
        vec![
            line(LineKind::Hunk, None, None, "@@ -1,3 +1,3 @@"),
            line(LineKind::Context, Some(1), Some(1), "keep one"),
            line(LineKind::Removed, Some(2), None, "gone"),
            line(LineKind::Added, None, Some(2), "\tcame"),
            line(LineKind::Hunk, None, None, "@@ -9,1 +9,1 @@"),
            line(LineKind::Context, Some(9), Some(9), "keep nine"),
        ]
    }

    fn at(row: usize, at: usize) -> Place {
        Place { row, at }
    }

    #[test]
    fn a_copy_holds_the_code_and_no_hunk_header() {
        let lines = sample();
        assert_eq!(selected_text(&lines, at(1, 5), at(2, 2)), "one\ngo");
        assert_eq!(selected_text(&lines, at(3, 0), at(5, 4)), "\tcame\nkeep", "the header between them is not part of it");
        assert_eq!(selected_text(&lines, at(0, 0), at(5, 9)), "keep one\ngone\n\tcame\nkeep nine");
        assert_eq!(selected_text(&lines, at(0, 3), at(0, 5)), "-1", "a header on its own is what was selected");
    }

    #[test]
    fn a_tab_copies_as_the_tab_it_is() {
        assert_eq!(source_at("\tcame", 0), 0);
        assert_eq!(source_at("\tcame", 2), 0, "inside the tab's spaces");
        assert_eq!(source_at("\tcame", 4), 1);
        assert_eq!(source_at("\tcame", 8), 5);
        assert_eq!(source_at("a\tb", 5), 2);
        assert_eq!(source_at("한글", 3), 3);
        assert_eq!(source_at("", 4), 0);
        assert_eq!(selected_text(&sample(), at(3, 4), at(3, 6)), "ca");
    }

    #[test]
    fn a_selection_past_the_lines_is_safe() {
        let lines = sample();
        assert_eq!(selected_text(&lines, at(5, 400), at(9, 900)), "");
        assert!(selected_rows(&lines, at(40, 0), at(41, 0)).is_empty());
        assert_eq!(selected_text(&[], at(0, 0), at(0, 1)), "");
    }

    #[test]
    fn a_selection_ending_at_a_rows_start_leaves_that_row_out() {
        let lines = sample();
        assert_eq!(selected_rows(&lines, at(1, 0), at(2, 0)).len(), 1);
        assert_eq!(selected_rows(&lines, at(1, 0), at(2, 1)).len(), 2);
        assert!(selected_rows(&lines, at(0, 0), at(0, 4)).is_empty(), "a header has no lines to ask about");
    }

    #[test]
    fn lines_in_the_file_are_named_as_the_editor_names_them() {
        let lines = sample();
        let rows = selected_rows(&lines, at(5, 0), at(5, 3));
        assert_eq!(diff_prompt("a.rs", &rows, None, " why? ", false), "In a.rs, line 9: why?");
        let rows = vec![line(LineKind::Added, None, Some(457), "x"), line(LineKind::Added, None, Some(467), "y")];
        assert_eq!(diff_prompt("a.rs", &rows, None, "fix", false), "In a.rs, lines 457-467: fix");
        assert_eq!(diff_prompt("a.rs", &rows, None, "fix", true), "In a.rs, lines 457-467: fix (Just answer; do not change any files.)");
    }

    #[test]
    fn removed_lines_travel_with_the_prompt() {
        let lines = sample();
        let mixed = selected_rows(&lines, at(1, 0), at(3, 2));
        assert_eq!(
            diff_prompt("a.rs", &mixed, None, "why?", false),
            "In a.rs, about this part of its uncommitted diff (lines 1-2 of the file now): why?\n\n````diff\n keep one\n-gone\n+\tcame\n````"
        );
        let gone = selected_rows(&lines, at(2, 0), at(2, 4));
        assert_eq!(
            diff_prompt("a.rs", &gone, None, "why?", true),
            "In a.rs, about these lines its uncommitted diff removes: why? (Just answer; do not change any files.)\n\n````diff\n-gone\n````"
        );
    }

    #[test]
    fn a_commits_lines_are_not_named_as_the_files() {
        let rows = vec![line(LineKind::Added, None, Some(3), "x")];
        assert_eq!(
            diff_prompt("a.rs", &rows, Some("abc1234"), "why?", false),
            "In a.rs, about this part of the diff of commit abc1234: why?\n\n````diff\n+x\n````"
        );
    }

    #[test]
    fn a_long_selection_stops_short_in_the_prompt() {
        let rows: Vec<DiffLine> = (1..=250).map(|n| line(LineKind::Removed, Some(n), None, "x")).collect();
        let prompt = diff_prompt("a.rs", &rows, None, "why?", false);
        assert_eq!(prompt.matches("\n-x").count(), PROMPT_ROWS);
        assert!(prompt.contains("... (50 more lines)"));
    }

    #[test]
    fn the_heading_names_the_lines() {
        let lines = sample();
        assert_eq!(rows_label(&selected_rows(&lines, at(5, 0), at(5, 1))), "L9");
        assert_eq!(rows_label(&selected_rows(&lines, at(1, 0), at(3, 2))), "L1–2");
        assert_eq!(rows_label(&selected_rows(&lines, at(2, 0), at(2, 2))), "−L2");
    }
}
