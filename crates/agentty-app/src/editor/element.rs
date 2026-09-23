//! Paints the text of the active file: line numbers, colored lines, selection, cursor and a
//! scrollbar. Only the lines on screen are shaped; the geometry of the last paint is kept on the
//! editor for mouse hits and for placing input-method popups.

use super::buffer::{column_to_byte, Buffer, Pos, TAB_WIDTH};
use super::highlight::{Span, FOREGROUND};
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

/// Text runs for a display line: colors from the highlighter, the input method's text underlined.
fn runs_for(display: &DisplayLine, spans: Option<&[Span]>, marked: Option<(usize, usize)>, font: &gpui::Font) -> Vec<TextRun> {
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
    let Some((start, end)) = marked.filter(|(s, e)| e > s) else { return runs };
    // Split the runs at the marked range and underline it.
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut offset = 0;
    for run in runs {
        let (run_start, run_end) = (offset, offset + run.len);
        offset = run_end;
        for (piece_start, piece_end, underlined) in [
            (run_start, start.clamp(run_start, run_end), false),
            (start.clamp(run_start, run_end), end.clamp(run_start, run_end), true),
            (end.clamp(run_start, run_end), run_end, false),
        ] {
            if piece_end > piece_start {
                out.push(TextRun {
                    len: piece_end - piece_start,
                    underline: underlined.then(|| UnderlineStyle { color: Some(run.color), thickness: px(1.), wavy: false }),
                    ..run.clone()
                });
            }
        }
    }
    out
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
        editor.update(cx, |editor, _| {
            let active = editor.active;
            let Some(doc) = editor.docs.get_mut(active) else {
                return Prepainted {
                    text_bounds: bounds,
                    lines: Vec::new(),
                    numbers: Vec::new(),
                    backgrounds: Vec::new(),
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

            // Keep the cursor in view (after typing, moving, dragging past the edge).
            let margin = (line_height * 2.).min(height / 3.);
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
            let mut laid = Vec::with_capacity(last - first);
            for index in first..last {
                let display = DisplayLine::new(doc.buffer.line(index));
                let marked_here = marked
                    .filter(|(s, e)| s.line == index && e.line == index)
                    .map(|(s, e)| (display.to_display(s.col), display.to_display(e.col)));
                let runs = runs_for(&display, doc.highlights.spans(index), marked_here, &font);
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
            if selection.is_empty() {
                backgrounds.push(fill(
                    Bounds::new(point(bounds.left(), px(row_top(cursor.line))), size(bounds.size.width, px(line_height))),
                    hex_alpha(0xffffff, 0.04),
                ));
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
            let cursor_quad = cursor_x.map(|x| {
                fill(
                    Bounds::new(point(px(text_left - scroll.x + x), px(row_top(cursor.line) + 2.)), size(px(2.), px(line_height - 4.))),
                    hex(0xaeafad),
                )
            });

            let numbers = laid
                .iter()
                .map(|line| {
                    let label = (line.index + 1).to_string();
                    let color = if line.index == cursor.line { 0xc6c6c6 } else { 0x6e7681 };
                    let shaped = shape(label.clone(), &[plain(&label, color)]);
                    let x = f32::from(bounds.left()) + gutter_width - 14. - f32::from(shaped.width);
                    (point(px(x), px(row_top(line.index))), shaped)
                })
                .collect();

            let thumb = (max_scroll_y > 0. && line_count as f32 * line_height > height).then(|| {
                let content = max_scroll_y + height;
                let thumb_height = (height * height / content).max(24.).min(height);
                let top = f32::from(bounds.top()) + scroll.y / max_scroll_y * (height - thumb_height);
                Bounds::new(point(bounds.right() - px(SCROLLBAR_WIDTH), px(top)), size(px(SCROLLBAR_WIDTH - 2.), px(thumb_height)))
            });

            let lines = laid.iter().map(|l| (origin(l.index), l.shaped.clone())).collect();
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
        let runs = runs_for(&line, Some(&spans), Some((4, 5)), &font);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 5);
        assert_eq!(runs.iter().filter(|r| r.underline.is_some()).map(|r| r.len).sum::<usize>(), 1);
        assert_eq!(runs_for(&DisplayLine::new(""), None, None, &font).len(), 0);
    }
}
