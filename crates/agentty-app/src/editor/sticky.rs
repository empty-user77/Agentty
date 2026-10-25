//! Sticky scroll, as in VS Code: the lines that open the class and function the top of the view
//! is in stay pinned above it while their body scrolls by, up to three, outermost first. A click
//! goes to that line. Settings → Project → File editor turns it off.

use super::buffer::{Pos, TAB_WIDTH};
use super::symbols;
use super::{CodeEditor, Content};
use crate::theme::{hex, hex_alpha, Chrome};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context};

/// Lines pinned at most.
const MAX_ROWS: usize = 3;

/// The definitions line `first` (the top of the view) sits in whose own first line is above it,
/// outermost first, the innermost [`MAX_ROWS`] of them.
fn pinned<'a>(outline: &'a [symbols::Symbol], line_at: impl Fn(usize) -> &'a str, first: usize) -> Vec<&'a symbols::Symbol> {
    let above: Vec<_> = symbols::enclosing(outline, line_at, first).into_iter().filter(|s| s.line < first).collect();
    let skip = above.len().saturating_sub(MAX_ROWS);
    above.into_iter().skip(skip).collect()
}

impl CodeEditor {
    /// The definitions the top of the view sits in whose first line has scrolled away, outermost
    /// first: (line, its text).
    pub(super) fn sticky_lines(&self) -> Vec<(usize, usize, String)> {
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && !d.previewing()) else {
            return Vec::new();
        };
        let Some((first, _)) = self.layout.as_ref().and_then(|l| l.shown_lines()) else { return Vec::new() };
        if first == 0 {
            return Vec::new();
        }
        let outline = doc.outline();
        pinned(&outline, |i| doc.buffer.line(i), first)
            .into_iter()
            .map(|s| (s.line, s.col, doc.buffer.line(s.line).trim_end().replace('\t', &" ".repeat(TAB_WIDTH))))
            .collect()
    }

    pub(super) fn render_sticky(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !crate::settings::settings(cx).editor_sticky_scroll || self.picker.is_some() || self.preview.covered {
            return None;
        }
        let rows = self.sticky_lines();
        if rows.is_empty() {
            return None;
        }
        let layout = self.layout.as_ref()?;
        let (at, width, gutter, height) = (layout.top_left(), layout.width(), layout.gutter(), layout.line_height);
        let font = crate::settings::terminal_font(cx);
        let size = crate::settings::settings(cx).font_size;
        let panel = div()
            .id("editor-sticky")
            .w(px(width - 12.))
            .bg(hex(Chrome::EDITOR))
            .border_b_1()
            .border_color(hex_alpha(0x000000, 0.45))
            .shadow_md()
            .occlude()
            .font_family(font)
            .text_size(px(size))
            .children(rows.into_iter().enumerate().map(|(i, (line, col, text))| {
                div()
                    .id(("editor-sticky-row", i))
                    .h(px(height))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(
                        div()
                            .w(px(gutter - super::element::NUMBER_END))
                            .flex_shrink_0()
                            .text_right()
                            .text_color(hex(0x6e7681))
                            .child((line + 1).to_string()),
                    )
                    .child(div().w(px(super::element::NUMBER_END + 8.)).flex_shrink_0())
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(text))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if let Some(path) = this.docs.get(this.active).map(|d| d.path.clone()) {
                            this.go_to(&path, Pos::new(line, col), 0, window, cx);
                        }
                    }))
            }));
        Some(gpui::deferred(gpui::anchored().position(at).child(panel)).with_priority(1).into_any_element())
    }

    pub(super) fn debug_sticky(&self) -> serde_json::Value {
        serde_json::json!(self
            .sticky_lines()
            .into_iter()
            .map(|(line, _, text)| format!("{}: {}", line + 1, text.trim()))
            .collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::language::Language;

    #[test]
    fn pins_the_definitions_the_top_of_the_view_is_in() {
        let mut lines = vec![
            "export class Ledger {".to_string(),
            "  rows = [];".into(),
            "".into(),
            "  total(): number {".into(),
            "    let sum = 0;".into(),
        ];
        lines.extend((0..50).map(|i| format!("    sum += {i};")));
        lines.extend([
            "    return sum;".to_string(),
            "  }".into(),
            "".into(),
            "  other(): void {".into(),
            "    x();".into(),
            "  }".into(),
            "}".into(),
        ]);
        let text = lines.join("\n");
        let outline = symbols::symbols(Language::TypeScript, &text);
        let line_at = |i: usize| lines[i].as_str();
        let names = |first: usize| pinned(&outline, line_at, first).into_iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        assert!(names(0).is_empty(), "nothing above the top");
        assert_eq!(names(30), ["Ledger", "total"]);
        // The next method's first line at the top: only the class above it.
        assert_eq!(names(58), ["Ledger"]);
        // The method's own first line at the top: only the class above it.
        assert_eq!(names(3), ["Ledger"]);
    }
}
