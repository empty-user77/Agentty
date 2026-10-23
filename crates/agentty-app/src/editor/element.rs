//! Paints the text of the active file: line numbers, colored lines, selection, cursor, the
//! problems a language server reported (wavy lines, ticks on the scrollbar) and a scrollbar. Only the lines on screen are shaped; the geometry of the last paint is kept on the
//! editor for mouse hits and for placing input-method popups.

use super::buffer::{column_to_byte, Buffer, Pos, TAB_WIDTH};
use super::highlight::{Span, FOREGROUND};
use super::lsp::{Problem, Severity};
use super::CodeEditor;
use crate::theme::{hex, hex_alpha, Chrome};
use gpui::{
    fill, point, px, relative, size, App, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity, FontStyle, FontWeight,
    GlobalElementId, LayoutId, PaintQuad, Pixels, Point, Position, ShapedLine, SharedString, Style, TextRun, UnderlineStyle, Window,
};

/// Bytes of one line shaped at most (longer lines make the file read-only anyway).
const SHAPE_LIMIT: usize = super::file::LONG_LINE;
/// Time spent highlighting per frame; the rest follows on the next frames.
const HIGHLIGHT_BUDGET: std::time::Duration = std::time::Duration::from_millis(8);
const SCROLLBAR_WIDTH: f32 = 10.;
/// Characters of a problem's message written at the end of its line at most.
const INLINE_PROBLEM_CHARS: usize = 160;
/// Characters looked through for the bracket matching the one at the cursor.
const BRACKET_SCAN: usize = 50_000;
const TEXT_PADDING: f32 = 8.;

/// A line as shown: tabs expanded to spaces, with the way back to buffer offsets.
pub struct DisplayLine {
    pub text: String,
    /// (buffer byte, display byte) at every character start and at the end, when they differ.
    stops: Option<Vec<(usize, usize)>>,
}

impl DisplayLine {
    pub fn new(line: &str) -> Self {
        let mut end = line.len().min(SHAPE_LIMIT);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        let line = &line[..end];
        if !line.contains('\t') {
            return Self { text: line.to_string(), stops: None };
        }
        let mut text = String::with_capacity(line.len() + 16);
        let mut stops = Vec::with_capacity(line.len() + 1);
        let mut column = 0;
        for (byte, c) in line.char_indices() {
            stops.push((byte, text.len()));
            if c == '\t' {
                let width = TAB_WIDTH - column % TAB_WIDTH;
                text.extend(std::iter::repeat_n(' ', width));
                column += width;
            } else {
                text.push(c);
                column += 1;
            }
        }
        stops.push((line.len(), text.len()));
        Self { text, stops: Some(stops) }
    }

    pub fn to_display(&self, col: usize) -> usize {
        match &self.stops {
            None => col.min(self.text.len()),
            Some(stops) => match stops.binary_search_by_key(&col, |(b, _)| *b) {
                Ok(index) => stops[index].1,
                Err(index) => stops.get(index.saturating_sub(1)).map_or(0, |s| s.1),
            },
        }
    }

    pub fn to_buffer(&self, display: usize) -> usize {
        match &self.stops {
            None => display.min(self.text.len()),
            Some(stops) => {
                // Inside an expanded tab: the nearer side of it.
                let index = stops.partition_point(|(_, d)| *d <= display).saturating_sub(1);
                let (byte, start) = stops[index];
                match stops.get(index + 1) {
                    Some((next_byte, next)) if display > start && display - start > (next - start) / 2 => *next_byte,
                    _ => byte,
                }
            }
        }
    }
}

struct LaidLine {
    index: usize,
    display: DisplayLine,
    shaped: ShapedLine,
}

/// Geometry of the last paint.
pub struct Layout {
    bounds: Bounds<Pixels>,
    text_left: f32,
    pub line_height: f32,
    char_width: f32,
    scroll: Point<f32>,
    lines: Vec<LaidLine>,
    pub visible_rows: usize,
    pub max_scroll_x: f32,
    pub max_scroll_y: f32,
    thumb: Option<Bounds<Pixels>>,
}

impl Layout {
    fn laid(&self, line: usize) -> Option<&LaidLine> {
        let first = self.lines.first()?.index;
        self.lines.get(line.checked_sub(first)?)
    }

    pub fn in_gutter(&self, position: Point<Pixels>) -> bool {
        f32::from(position.x) < self.text_left - TEXT_PADDING / 2.
    }

    pub fn on_scrollbar(&self, position: Point<Pixels>) -> bool {
        self.thumb.is_some() && f32::from(position.x) >= f32::from(self.bounds.right()) - SCROLLBAR_WIDTH - 2.
    }

    /// Scroll offset after the scrollbar's thumb was dragged `dy` from where it was at `start`.
    pub fn scroll_for_thumb_drag(&self, start: f32, dy: f32) -> f32 {
        let Some(thumb) = self.thumb else { return start };
        let track = f32::from(self.bounds.size.height) - f32::from(thumb.size.height);
        if track <= 0. {
            return start;
        }
        (start + dy * self.max_scroll_y / track).clamp(0., self.max_scroll_y)
    }

    /// The text position under a point (above or below the text: its first or last line).
    pub fn position_for(&self, position: Point<Pixels>, buffer: &Buffer) -> Pos {
        let y = f32::from(position.y) - f32::from(self.bounds.top()) + self.scroll.y;
        let row = (y / self.line_height).floor().max(0.) as usize;
        let row = row.min(buffer.line_count() - 1);
        let x = (f32::from(position.x) - self.text_left + self.scroll.x).max(0.);
        let col = match self.laid(row) {
            Some(line) => line.display.to_buffer(line.shaped.closest_index_for_x(px(x))),
            None => column_to_byte(buffer.line(row), (x / self.char_width).round() as usize),
        };
        buffer.clamp(Pos::new(row, col))
    }

    /// The window point just under the text at `pos` (where a bar about it goes), while that line
    /// is on screen.
    pub fn point_below(&self, pos: Pos) -> Option<Point<Pixels>> {
        let top = f32::from(self.bounds.top()) + pos.line as f32 * self.line_height - self.scroll.y;
        if top < f32::from(self.bounds.top()) - self.line_height || top > f32::from(self.bounds.bottom()) - self.line_height {
            return None;
        }
        let x = match self.laid(pos.line) {
            Some(line) => self.text_left - self.scroll.x + f32::from(line.shaped.x_for_index(line.display.to_display(pos.col))),
            None => self.text_left,
        };
        let x = x.clamp(f32::from(self.bounds.left()), f32::from(self.bounds.right()) - 40.);
        Some(point(px(x), px(top + self.line_height + 4.)))
    }

    /// Width of the text area (gutter included).
    pub fn width(&self) -> f32 {
        f32::from(self.bounds.size.width)
    }

    /// The first and last lines shaped in the last paint (the ones on screen).
    pub fn shown_lines(&self) -> Option<(usize, usize)> {
        Some((self.lines.first()?.index, self.lines.last()?.index.saturating_sub(1).max(self.lines.first()?.index)))
    }

    /// Window points just inside the text's top right and bottom right corners.
    pub fn corners(&self) -> (Point<Pixels>, Point<Pixels>) {
        let right = self.bounds.right() - px(SCROLLBAR_WIDTH + 6.);
        (point(right, self.bounds.top() + px(6.)), point(right, self.bounds.bottom() - px(6.)))
    }

    /// The window point at the left edge of the gutter on `line`, while that line is on screen.
    pub fn gutter_point(&self, line: usize) -> Option<Point<Pixels>> {
        let at = self.point_below(Pos::new(line, 0))?;
        Some(point(self.bounds.left() + px(2.), at.y - px(self.line_height + 4.)))
    }

    /// The window point at the top left of the text at `pos`, while that line is on screen.
    pub fn point_at(&self, pos: Pos) -> Option<Point<Pixels>> {
        self.point_below(pos).map(|p| point(p.x, p.y - px(self.line_height + 4.)))
    }

    /// Where `start..end` is on screen (for the input method's candidate window).
    pub fn bounds_for(&self, start: Pos, end: Pos, _: &Buffer) -> Option<Bounds<Pixels>> {
        let line = self.laid(start.line)?;
        let x = |col: usize| self.text_left - self.scroll.x + f32::from(line.shaped.x_for_index(line.display.to_display(col)));
        let top = f32::from(self.bounds.top()) + start.line as f32 * self.line_height - self.scroll.y;
        let right = if end.line == start.line { x(end.col) } else { x(start.col) + self.char_width };
        Some(Bounds::from_corners(point(px(x(start.col)), px(top)), point(px(right.max(x(start.col) + 1.)), px(top + self.line_height))))
    }
}

pub struct EditorElement {
    editor: Entity<CodeEditor>,
}

impl EditorElement {
    pub fn new(editor: Entity<CodeEditor>) -> Self {
        Self { editor }
    }
}

pub struct Prepainted {
    text_bounds: Bounds<Pixels>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    numbers: Vec<(Point<Pixels>, ShapedLine)>,
    backgrounds: Vec<PaintQuad>,
    /// Marks in the gutter (agents' changes, a sent instruction's lines), over its fill.
    gutter_marks: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    gutter: PaintQuad,
    thumb: Option<PaintQuad>,
    line_height: Pixels,
}

impl gpui::IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

/// Text runs for a display line: colors from the highlighter, and `marks` (display ranges)
/// underlined — the input method's text, problems a server reported. A later mark wins where
/// marks overlap.
fn runs_for(display: &DisplayLine, spans: Option<&[Span]>, marks: &[(usize, usize, UnderlineStyle)], font: &gpui::Font) -> Vec<TextRun> {
    let run = |len: usize, color: u32, bold: bool, italic: bool| TextRun {
        len,
        font: gpui::Font {
            weight: if bold { FontWeight::BOLD } else { FontWeight::NORMAL },
            style: if italic { FontStyle::Italic } else { FontStyle::Normal },
            ..font.clone()
        },
        color: hex(color),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let total = display.text.len();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut shown = 0;
    let mut byte = 0;
    for span in spans.unwrap_or_default() {
        let end = display.to_display(byte + span.len).min(total);
        if end > shown {
            runs.push(run(end - shown, span.color, span.bold, span.italic));
            shown = end;
        }
        byte += span.len;
    }
    if shown < total {
        runs.push(run(total - shown, FOREGROUND, false, false));
    }
    let marks: Vec<&(usize, usize, UnderlineStyle)> = marks.iter().filter(|(s, e, _)| e > s).collect();
    if marks.is_empty() {
        return runs;
    }
    // Split the runs where a mark starts or ends, and underline the pieces under one.
    let mut out = Vec::with_capacity(runs.len() + marks.len() * 2);
    let mut offset = 0;
    for run in runs {
        let (run_start, run_end) = (offset, offset + run.len);
        offset = run_end;
        let mut cuts: Vec<usize> = marks.iter().flat_map(|(s, e, _)| [*s, *e]).filter(|c| *c > run_start && *c < run_end).collect();
        cuts.extend([run_start, run_end]);
        cuts.sort_unstable();
        cuts.dedup();
        for piece in cuts.windows(2) {
            let underline = marks.iter().rev().find(|(s, e, _)| *s <= piece[0] && piece[1] <= *e).map(|(_, _, style)| *style);
            out.push(TextRun { len: piece[1] - piece[0], underline, ..run.clone() });
        }
    }
    out
}

/// The problem written at the end of line `index`: the worst error or warning starting there.
fn inline_problem<'a>(problems: &[&'a Problem], index: usize) -> Option<&'a Problem> {
    problems.iter().filter(|p| p.start.line == index && p.severity <= Severity::Warning).min_by_key(|p| p.severity).copied()
}

/// The display ranges of `line` (its text `text`) that `problems` cover, as wavy lines in their
/// colors, the worst last. A problem without width marks the character it starts at (or the one
/// before, at the end of the line).
fn problem_marks(index: usize, text: &str, display: &DisplayLine, problems: &[&Problem]) -> Vec<(usize, usize, UnderlineStyle)> {
    let mut here: Vec<&&Problem> = problems.iter().filter(|p| p.start.line <= index && index <= p.end.line).collect();
    here.sort_by_key(|p| std::cmp::Reverse(p.severity));
    here.into_iter()
        .filter_map(|p| {
            let mut from = if p.start.line == index { p.start.col.min(text.len()) } else { 0 };
            let mut to = if p.end.line == index { p.end.col.min(text.len()) } else { text.len() };
            if to <= from {
                match text[from..].chars().next() {
                    Some(c) => to = from + c.len_utf8(),
                    None => from = text[..from].char_indices().next_back().map(|(i, _)| i)?,
                }
            }
            let style = UnderlineStyle { color: Some(hex(super::problems::color(p.severity))), thickness: px(1.), wavy: true };
            Some((display.to_display(from), display.to_display(to), style))
        })
        .collect()
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

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
        let mut style = Style { position: Position::Absolute, ..Style::default() };
        style.inset.top = px(0.).into();
        style.inset.left = px(0.).into();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
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
    ) -> Prepainted {
        let (family, points) = (crate::settings::terminal_font(cx), crate::settings::settings(cx).font_size);
        let font = gpui::font(family);
        let font_size = px(points);
        let line_height = (points * 1.5).round();
        let editor = self.editor.clone();
        editor.update(cx, |editor, cx| {
            let active = editor.active;
            // An inactive window has no blinking caret (like native fields); activation repaints.
            let focused = editor.focus_handle.is_focused(window) && window.is_window_active();
            let Some(doc) = editor.docs.get_mut(active) else {
                // No document: no caret, and no timer redrawing nothing.
                editor.blink.stop();
                return Prepainted {
                    text_bounds: bounds,
                    lines: Vec::new(),
                    numbers: Vec::new(),
                    backgrounds: Vec::new(),
                    gutter_marks: Vec::new(),
                    cursor: None,
                    gutter: fill(bounds, hex(Chrome::EDITOR)),
                    thumb: None,
                    line_height: px(line_height),
                };
            };
            doc.sync_highlights();
            let text_system = window.text_system().clone();
            let shape = |text: String, runs: &[TextRun]| text_system.shape_line(SharedString::from(text), font_size, runs, None);
            let plain = |text: &str, color: u32| TextRun {
                len: text.len(),
                font: font.clone(),
                color: hex(color),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let char_width = f32::from(shape("0000000000".into(), &[plain("0000000000", FOREGROUND)]).width) / 10.;
            let line_count = doc.buffer.line_count();
            let digits = line_count.to_string().len().max(3);
            let gutter_width = (digits as f32 * char_width + 28.).round();
            let height = f32::from(bounds.size.height);
            let text_left = f32::from(bounds.left()) + gutter_width + TEXT_PADDING;
            let text_width = (f32::from(bounds.size.width) - gutter_width - TEXT_PADDING - SCROLLBAR_WIDTH).max(char_width);
            let max_scroll_y = ((line_count as f32 - 1.) * line_height).max(0.);
            let cursor = doc.buffer.cursor();
            let caret_key = (active, cursor.line, cursor.col, doc.buffer.revision());

            // Keep the cursor in view (after typing, moving, dragging past the edge).
            let margin = (line_height * 2.).min(height / 3.);
            // After a jump the place reads best in the upper third, with what leads to it above.
            if doc.center {
                doc.center = false;
                let top = cursor.line as f32 * line_height;
                if top < doc.scroll.y + margin || top + line_height > doc.scroll.y + height - margin {
                    doc.scroll.y = top - height / 3.;
                }
                doc.autoscroll = false;
            }
            if let Some(line) = doc.scroll_to_line.take() {
                let top = line as f32 * line_height;
                if top < doc.scroll.y + margin || top + line_height > doc.scroll.y + height - margin {
                    doc.scroll.y = top - height / 3.;
                }
            }
            if doc.autoscroll {
                let top = cursor.line as f32 * line_height;
                if top < doc.scroll.y + margin {
                    doc.scroll.y = top - margin;
                } else if top + line_height > doc.scroll.y + height - margin {
                    doc.scroll.y = top + line_height - height + margin;
                }
            }
            doc.scroll.y = doc.scroll.y.clamp(0., max_scroll_y);

            let first = (doc.scroll.y / line_height).floor() as usize;
            let rows = (height / line_height).ceil() as usize + 1;
            let last = (first + rows).min(line_count);
            let buffer = &doc.buffer;
            if !doc.highlights.advance(last, std::time::Instant::now() + HIGHLIGHT_BUDGET, |i| buffer.line(i)) {
                window.request_animation_frame();
            }

            let marked = doc.buffer.marked();
            let problems = &doc.checks.problems;
            let in_view: Vec<&Problem> = problems.iter().filter(|p| p.end.line >= first && p.start.line < last).collect();
            let mut laid = Vec::with_capacity(last - first);
            for index in first..last {
                let display = DisplayLine::new(doc.buffer.line(index));
                let mut marks = problem_marks(index, doc.buffer.line(index), &display, &in_view);
                marks.extend(marked.filter(|(s, e)| s.line == index && e.line == index).map(|(s, e)| {
                    (display.to_display(s.col), display.to_display(e.col), UnderlineStyle { color: None, thickness: px(1.), wavy: false })
                }));
                let runs = runs_for(&display, doc.highlights.spans(index), &marks, &font);
                let shaped = shape(display.text.clone(), &runs);
                laid.push(LaidLine { index, display, shaped });
            }

            // Horizontally too, once the cursor's line is shaped.
            let cursor_x =
                laid.iter().find(|l| l.index == cursor.line).map(|l| f32::from(l.shaped.x_for_index(l.display.to_display(cursor.col))));
            if let (true, Some(x)) = (doc.autoscroll, cursor_x) {
                if x < doc.scroll.x {
                    doc.scroll.x = (x - char_width * 4.).max(0.);
                } else if x > doc.scroll.x + text_width - char_width * 2. {
                    doc.scroll.x = x - text_width + char_width * 4.;
                }
            }
            let widest = laid.iter().map(|l| f32::from(l.shaped.width)).fold(0., f32::max);
            let max_scroll_x = (widest - text_width + char_width * 4.).max(0.).max(if doc.autoscroll { doc.scroll.x } else { 0. });
            doc.scroll.x = doc.scroll.x.clamp(0., max_scroll_x);
            doc.autoscroll = false;
            let scroll = doc.scroll;

            let row_top = |index: usize| f32::from(bounds.top()) + index as f32 * line_height - scroll.y;
            let origin = |index: usize| point(px(text_left - scroll.x), px(row_top(index)));
            let mut backgrounds = Vec::new();
            let selection = doc.buffer.selection;
            let (sel_start, sel_end) = selection.range();
            // Find matches on the lines in view, under the selection; the selected one stronger.
            if let Some(bar) = editor.find.as_ref() {
                let from = bar.matches.partition_point(|(start, _)| start.line < first);
                for &(start, end) in bar.matches[from..].iter().take_while(|(start, _)| start.line < last) {
                    let Some(line) = laid.iter().find(|l| l.index == start.line) else { continue };
                    let x0 = f32::from(line.shaped.x_for_index(line.display.to_display(start.col)));
                    let x1 = f32::from(line.shaped.x_for_index(line.display.to_display(end.col)));
                    let current = (start, end) == (sel_start, sel_end);
                    backgrounds.push(fill(
                        Bounds::new(
                            point(px(text_left - scroll.x + x0), px(row_top(start.line))),
                            size(px((x1 - x0).max(2.)), px(line_height)),
                        ),
                        hex_alpha(0xe2a03f, if current { 0.55 } else { 0.25 }),
                    ));
                }
            }
            // The name under the cursor, everywhere it is written on screen (as IDEs mark it).
            if editor.find.is_none() && super::symbols::supported(doc.language) {
                let (start, end) = (sel_start, sel_end);
                let line = doc.buffer.line(cursor.line);
                let name = if start == end {
                    super::symbols::name_at(line, cursor.col, doc.language).map(|(s, e)| line[s..e].to_string())
                } else if start.line == end.line {
                    Some(line[start.col.min(line.len())..end.col.min(line.len())].to_string())
                        .filter(|n| n.chars().all(|c| super::symbols::is_name_char(c, doc.language)))
                } else {
                    None
                };
                if let Some(name) = name.filter(|n| n.chars().count() >= 2 && !super::symbols::is_keyword(n)) {
                    for laid_line in &laid {
                        let text = doc.buffer.line(laid_line.index);
                        for (_, col) in super::symbols::occurrences(text, &name, doc.language, 64) {
                            let x0 = f32::from(laid_line.shaped.x_for_index(laid_line.display.to_display(col)));
                            let x1 = f32::from(laid_line.shaped.x_for_index(laid_line.display.to_display(col + name.len())));
                            backgrounds.push(fill(
                                Bounds::new(
                                    point(px(text_left - scroll.x + x0), px(row_top(laid_line.index))),
                                    size(px(x1 - x0), px(line_height)),
                                ),
                                hex_alpha(0xffffff, 0.09),
                            ));
                        }
                    }
                }
            }
            // Lines an agent just changed, fading out; its caret where it works.
            let mut agent_carets = Vec::new();
            for presence in editor.presence.iter().filter(|p| p.path == doc.path) {
                let glow = presence.glow();
                for &(from, to) in presence.parts.iter().filter(|_| glow > 0.) {
                    for line in from.max(first)..=to.min(last.saturating_sub(1)) {
                        backgrounds.push(fill(
                            Bounds::new(point(bounds.left(), px(row_top(line))), size(bounds.size.width, px(line_height))),
                            hex_alpha(presence.color, 0.22 * glow),
                        ));
                    }
                }
                if presence.label.is_empty() || presence.at.line < first || presence.at.line >= last {
                    continue;
                }
                let Some(line) = laid.iter().find(|l| l.index == presence.at.line) else { continue };
                let x = f32::from(
                    line.shaped.x_for_index(line.display.to_display(presence.at.col.min(doc.buffer.line(presence.at.line).len()))),
                );
                let pulse = if presence.pending { 0.55 + 0.45 * (presence.since.elapsed().as_secs_f32() * 4.).sin().abs() } else { 1. };
                agent_carets.push(fill(
                    Bounds::new(point(px(text_left - scroll.x + x), px(row_top(presence.at.line))), size(px(2.), px(line_height))),
                    hex_alpha(presence.color, pulse),
                ));
            }
            backgrounds.extend(agent_carets);
            let mut gutter_marks = Vec::new();
            // Lines an instruction was sent about, until the agent answers with a change.
            if let Some(asked) = editor.asked.as_ref().filter(|a| a.path == doc.path) {
                let (from, to) = (asked.lines.0.max(first), asked.lines.1.min(last.saturating_sub(1)));
                if from <= to {
                    gutter_marks.push(fill(
                        Bounds::new(
                            point(px(f32::from(bounds.left()) + 2.), px(row_top(from))),
                            size(px(2.), px((to - from + 1) as f32 * line_height)),
                        ),
                        hex_alpha(asked.color, 0.8),
                    ));
                    for line in from..=to {
                        backgrounds.push(fill(
                            Bounds::new(point(px(text_left - 4.), px(row_top(line))), size(bounds.size.width, px(line_height))),
                            hex_alpha(asked.color, 0.06),
                        ));
                    }
                }
            }
            // Agents' changes waiting for review: a bar in the gutter, in the agent's color.
            let changes = editor
                .changes
                .iter()
                .filter(|c| c.path == doc.path)
                .filter_map(|c| super::review::locate(&doc.buffer, &c.hunk).map(|at| (c, at)));
            for (change, at) in changes {
                let rows = change.hunk.new.len();
                let (top, height) = if rows == 0 {
                    (row_top(at) - 2., 4.) // lines taken away: a notch where they were
                } else {
                    (row_top(at), rows as f32 * line_height)
                };
                if at + rows.max(1) < first || at > last {
                    continue;
                }
                gutter_marks.push(fill(
                    Bounds::new(point(px(f32::from(bounds.left()) + gutter_width - 5.), px(top)), size(px(3.), px(height))),
                    hex_alpha(change.color, 0.9),
                ));
            }
            // ⌘ over a name: underlined, as a link.
            if let Some((start, end)) = editor.link.filter(|(s, _)| s.line >= first && s.line < last) {
                if let Some(line) = laid.iter().find(|l| l.index == start.line) {
                    let x0 = f32::from(line.shaped.x_for_index(line.display.to_display(start.col)));
                    let x1 = f32::from(line.shaped.x_for_index(line.display.to_display(end.col)));
                    backgrounds.push(fill(
                        Bounds::new(
                            point(px(text_left - scroll.x + x0), px(row_top(start.line) + line_height - 3.)),
                            size(px(x1 - x0), px(1.)),
                        ),
                        hex(Chrome::BLUE),
                    ));
                }
            }
            if selection.is_empty() {
                backgrounds.push(fill(
                    Bounds::new(point(bounds.left(), px(row_top(cursor.line))), size(bounds.size.width, px(line_height))),
                    hex_alpha(0xffffff, 0.04),
                ));
                // The bracket at the cursor and its match, boxed.
                if let Some((here, there)) = doc.buffer.matching_bracket(BRACKET_SCAN) {
                    for pos in [here, there] {
                        let Some(line) = laid.iter().find(|l| l.index == pos.line) else { continue };
                        let x0 = f32::from(line.shaped.x_for_index(line.display.to_display(pos.col)));
                        let x1 = f32::from(line.shaped.x_for_index(line.display.to_display(pos.col + 1)));
                        backgrounds.push(
                            fill(
                                Bounds::new(
                                    point(px(text_left - scroll.x + x0), px(row_top(pos.line) + 1.)),
                                    size(px((x1 - x0).max(2.)), px(line_height - 2.)),
                                ),
                                hex_alpha(0xffffff, 0.10),
                            )
                            .border_widths(px(1.))
                            .border_color(hex_alpha(0xffffff, 0.35)),
                        );
                    }
                }
            }
            for line in &laid {
                if selection.is_empty() || line.index < sel_start.line || line.index > sel_end.line {
                    continue;
                }
                let from = if line.index == sel_start.line { sel_start.col } else { 0 };
                let to = if line.index == sel_end.line { sel_end.col } else { doc.buffer.line(line.index).len() };
                let x0 = f32::from(line.shaped.x_for_index(line.display.to_display(from)));
                let mut x1 = f32::from(line.shaped.x_for_index(line.display.to_display(to)));
                if line.index != sel_end.line {
                    x1 += char_width * 0.6; // the line break is selected too
                }
                if x1 > x0 {
                    backgrounds.push(fill(
                        Bounds::new(point(px(text_left - scroll.x + x0), px(row_top(line.index))), size(px(x1 - x0), px(line_height))),
                        hex_alpha(Chrome::ACCENT, 0.40),
                    ));
                }
            }
            let caret = editor.blink.update(focused, caret_key, cx, |e: &mut CodeEditor| &mut e.blink);
            let cursor_quad = cursor_x.filter(|_| caret).map(|x| {
                fill(
                    Bounds::new(point(px(text_left - scroll.x + x), px(row_top(cursor.line) + 2.)), size(px(2.), px(line_height - 4.))),
                    hex(0xaeafad),
                )
            });

            // A line with a problem has its number in the color of the worst one there.
            let worst = |line: usize| in_view.iter().filter(|p| p.start.line <= line && line <= p.end.line).map(|p| p.severity).min();
            let numbers = laid
                .iter()
                .map(|line| {
                    let label = (line.index + 1).to_string();
                    let color = match worst(line.index) {
                        Some(severity @ (Severity::Error | Severity::Warning)) => super::problems::color(severity),
                        _ if line.index == cursor.line => 0xc6c6c6,
                        _ => 0x6e7681,
                    };
                    let shaped = shape(label.clone(), &[plain(&label, color)]);
                    let x = f32::from(bounds.left()) + gutter_width - 14. - f32::from(shaped.width);
                    (point(px(x), px(row_top(line.index))), shaped)
                })
                .collect();

            // Errors and warnings anywhere in the file: ticks on the scrollbar's track.
            let mut ticked = std::collections::HashSet::new();
            for problem in problems.iter().filter(|p| p.severity <= Severity::Warning) {
                if line_count > 1 && ticked.insert((problem.start.line, problem.severity)) {
                    let y = f32::from(bounds.top()) + problem.start.line as f32 / line_count as f32 * (height - 3.);
                    gutter_marks.push(fill(
                        Bounds::new(point(bounds.right() - px(SCROLLBAR_WIDTH - 1.), px(y)), size(px(SCROLLBAR_WIDTH - 3.), px(3.))),
                        hex_alpha(super::problems::color(problem.severity), 0.9),
                    ));
                }
            }
            let thumb = (max_scroll_y > 0. && line_count as f32 * line_height > height).then(|| {
                let content = max_scroll_y + height;
                let thumb_height = (height * height / content).max(24.).min(height);
                let top = f32::from(bounds.top()) + scroll.y / max_scroll_y * (height - thumb_height);
                Bounds::new(point(bounds.right() - px(SCROLLBAR_WIDTH), px(top)), size(px(SCROLLBAR_WIDTH - 2.), px(thumb_height)))
            });

            let mut lines: Vec<(Point<Pixels>, ShapedLine)> = laid.iter().map(|l| (origin(l.index), l.shaped.clone())).collect();
            // Each line's worst error or warning, written after its text (Error Lens).
            if crate::settings::settings(cx).editor_inline_problems {
                for line in &laid {
                    let Some(problem) = inline_problem(&in_view, line.index) else { continue };
                    let text: String = problem.message.lines().next().unwrap_or("").chars().take(INLINE_PROBLEM_CHARS).collect();
                    let text = format!("■ {text}");
                    let run = TextRun {
                        len: text.len(),
                        font: gpui::Font { style: FontStyle::Italic, ..font.clone() },
                        color: hex_alpha(super::problems::color(problem.severity), 0.75),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let shaped = shape(text, &[run]);
                    let at = point(px(text_left - scroll.x + f32::from(line.shaped.width) + char_width * 3.), px(row_top(line.index)));
                    lines.push((at, shaped));
                }
            }
            editor.layout = Some(Layout {
                bounds,
                text_left,
                line_height,
                char_width,
                scroll,
                lines: laid,
                visible_rows: (height / line_height).floor() as usize,
                max_scroll_x,
                max_scroll_y,
                thumb,
            });
            let text_bounds = Bounds::from_corners(point(px(text_left - TEXT_PADDING / 2.), bounds.top()), bounds.bottom_right());
            Prepainted {
                text_bounds,
                lines,
                numbers,
                backgrounds,
                gutter_marks,
                cursor: cursor_quad,
                gutter: fill(Bounds::new(bounds.origin, size(px(gutter_width), bounds.size.height)), hex(Chrome::EDITOR)),
                thumb: thumb.map(|t| fill(t, hex_alpha(0xffffff, 0.14)).corner_radii(px(3.))),
                line_height: px(line_height),
            }
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepainted: &mut Prepainted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.editor.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(prepainted.gutter.clone());
            for quad in prepainted.gutter_marks.drain(..) {
                window.paint_quad(quad);
            }
            for (origin, number) in &prepainted.numbers {
                let _ = number.paint(*origin, prepainted.line_height, window, cx);
            }
            window.with_content_mask(Some(ContentMask { bounds: prepainted.text_bounds }), |window| {
                for quad in prepainted.backgrounds.drain(..) {
                    window.paint_quad(quad);
                }
                for (origin, line) in &prepainted.lines {
                    let _ = line.paint(*origin, prepainted.line_height, window, cx);
                }
                if focus.is_focused(window) {
                    if let Some(cursor) = prepainted.cursor.take() {
                        window.paint_quad(cursor);
                    }
                }
            });
            if let Some(thumb) = prepainted.thumb.take() {
                window.paint_quad(thumb);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_are_shown_as_spaces_and_mapped_back() {
        let line = DisplayLine::new("\tx\ty");
        assert_eq!(line.text, "    x   y");
        assert_eq!(line.to_display(0), 0);
        assert_eq!(line.to_display(1), 4);
        assert_eq!(line.to_display(2), 5);
        assert_eq!(line.to_display(4), 9);
        assert_eq!(line.to_buffer(4), 1);
        assert_eq!(line.to_buffer(1), 0);
        assert_eq!(line.to_buffer(3), 1);
        assert_eq!(line.to_buffer(9), 4);
        let plain = DisplayLine::new("abc");
        assert_eq!((plain.to_display(2), plain.to_buffer(2)), (2, 2));
    }

    #[test]
    fn runs_cover_the_line_and_underline_composing_text() {
        let font = gpui::font("JetBrains Mono");
        let line = DisplayLine::new("let x");
        let spans = [Span { len: 3, color: 0x569cd6, bold: false, italic: false }];
        let plain = UnderlineStyle { color: None, thickness: px(1.), wavy: false };
        let runs = runs_for(&line, Some(&spans), &[(4, 5, plain)], &font);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 5);
        assert_eq!(runs.iter().filter(|r| r.underline.is_some()).map(|r| r.len).sum::<usize>(), 1);
        assert_eq!(runs_for(&DisplayLine::new(""), None, &[], &font).len(), 0);
    }

    #[test]
    fn the_worst_problem_starting_on_a_line_is_written_after_it() {
        let problem = |line: usize, severity| Problem {
            start: Pos::new(line, 0),
            end: Pos::new(line, 1),
            severity,
            message: String::new(),
            origin: None,
        };
        let (warning, error, note) = (problem(2, Severity::Warning), problem(2, Severity::Error), problem(3, Severity::Information));
        let all = [&warning, &error, &note];
        assert_eq!(inline_problem(&all, 2).map(|p| p.severity), Some(Severity::Error));
        assert!(inline_problem(&all, 3).is_none(), "notes stay under the line only");
        assert!(inline_problem(&all, 1).is_none());
    }

    #[test]
    fn problems_are_wavy_and_the_worst_wins() {
        let font = gpui::font("JetBrains Mono");
        let text = "let total = 1;";
        let line = DisplayLine::new(text);
        let problem = |start: usize, end: usize, severity| Problem {
            start: Pos::new(0, start),
            end: Pos::new(0, end),
            severity,
            message: String::new(),
            origin: None,
        };
        let (warning, error, empty) = (problem(4, 9, Severity::Warning), problem(6, 12, Severity::Error), problem(14, 14, Severity::Error));
        let marks = problem_marks(0, text, &line, &[&warning, &error]);
        // The error is applied last, so it wins where the two overlap.
        assert_eq!(marks.iter().map(|m| (m.0, m.1)).collect::<Vec<_>>(), [(4, 9), (6, 12)]);
        assert!(marks.iter().all(|m| m.2.wavy));
        let runs = runs_for(&line, None, &marks, &font);
        // The underline color of each byte.
        let colors: Vec<Option<gpui::Hsla>> =
            runs.iter().flat_map(|r| std::iter::repeat_n(r.underline.and_then(|u| u.color), r.len)).collect();
        let (yellow, red) = (Some(hex(Chrome::WARNING)), Some(hex(Chrome::ERROR)));
        assert_eq!(colors, [None, None, None, None, yellow, yellow, red, red, red, red, red, red, None, None]);
        // At the end of the line: the last character.
        assert_eq!(problem_marks(0, text, &line, &[&empty]).iter().map(|m| (m.0, m.1)).collect::<Vec<_>>(), [(13, 14)]);
    }
}
