//! The type or signature of the name under the pointer, from the language server, after the
//! pointer rests on it a moment (no key held, as in VS Code). Only asked of a server that is
//! running; a problem's message takes the place when the name has one.

use super::buffer::Pos;
use super::lsp;
use super::symbols;
use super::{CodeEditor, Content};
use crate::theme::{hex, Chrome};
use gpui::{div, prelude::*, px, AnyElement, Context};
use std::path::PathBuf;
use std::time::Duration;

/// The pointer rests this long on a name before its type is asked for.
const DELAY: Duration = Duration::from_millis(600);
/// A window stays this long after the pointer left what it is about (time to reach it).
const GRACE: Duration = Duration::from_millis(350);

pub(super) struct TypeHover {
    path: PathBuf,
    start: Pos,
    end: Pos,
    /// The server's text, once it answered (empty: nothing to show).
    text: Option<String>,
}

impl CodeEditor {
    /// The pointer over the text (`None`: off it, a key held, or busy elsewhere).
    pub(super) fn hover_type(&mut self, pos: Option<Pos>, cx: &mut Context<Self>) {
        let pos = pos.filter(|_| crate::settings::settings(cx).editor_hover_types);
        let target = pos.and_then(|pos| {
            let doc = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && !d.text_hidden())?;
            if lsp::status(&doc.project, doc.language, &doc.path) != "ready" {
                return None;
            }
            let line = doc.buffer.line(pos.line);
            let (start, end) = symbols::name_at(line, pos.col, doc.language).filter(|(s, e)| e > s)?;
            let name = &line[start..end];
            if !name.chars().any(char::is_alphabetic) || symbols::is_keyword(name) {
                return None;
            }
            Some((doc.path.clone(), Pos::new(pos.line, start), Pos::new(pos.line, end)))
        });
        let same = |h: &TypeHover| target.as_ref().is_some_and(|(path, start, end)| h.path == *path && h.start == *start && h.end == *end);
        if self.type_hover.as_ref().is_some_and(same) {
            return;
        }
        // A type that shows stays a moment: the pointer may be on its way to it.
        if self.type_hover.as_ref().is_some_and(|h| h.text.as_ref().is_some_and(|t| !t.is_empty())) {
            return self.leave_hovers(cx);
        }
        self.type_hover = target.clone().map(|(path, start, end)| TypeHover { path, start, end, text: None });
        self.popup_over = false;
        let Some((path, start, end)) = target else { return };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DELAY).await;
            let still = |this: &CodeEditor| this.type_hover.as_ref().is_some_and(|h| h.path == path && h.start == start && h.end == end);
            let asked = this
                .update(cx, |this, _| {
                    let doc = this.docs.iter().find(|d| d.path == path).filter(|_| still(this))?;
                    Some((doc.project.clone(), doc.language, lsp::Snapshot::of(&doc.buffer)))
                })
                .ok()
                .flatten();
            let Some((project, language, text)) = asked else { return };
            let file = path.clone();
            let found = cx.background_spawn(async move { lsp::hover(&project, language, &file, &text, start) }).await;
            let _ = this.update(cx, |this, cx| {
                if still(this) {
                    if let Some(hover) = this.type_hover.as_mut() {
                        hover.text = Some(found.unwrap_or_default());
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_type_hover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let busy = self.picker.is_some() || self.confirm.is_some() || self.preview.covered || self.link.is_some();
        let covered = self.fixes.is_some() || self.rename.is_some() || self.completion.is_some() || self.signature.is_some();
        if busy || covered || self.problem_hover.as_ref().is_some_and(|(_, shown)| *shown) {
            return None;
        }
        let hover = self.type_hover.as_ref()?;
        let text = hover.text.as_ref().filter(|t| !t.trim().is_empty())?;
        self.docs.get(self.active).filter(|d| d.path == hover.path)?;
        let lines = super::navigate::note_lines(text);
        // Above the name, as the problem messages are: the code under it stays readable.
        let layout = self.layout.as_ref()?;
        let top = layout.point_at(hover.start)?;
        let height = 10. + 18. * lines.len() as f32;
        let (corner, at) = if f32::from(top.y - layout.corners().0.y) > height {
            (gpui::Corner::BottomLeft, gpui::point(top.x, top.y - px(4.)))
        } else {
            (gpui::Corner::TopLeft, layout.point_below(hover.start)?)
        };
        let panel = div()
            .id("editor-type-hover")
            .max_w(px(600.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .occlude()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.over_popup(*hovered, cx)))
            .font_family(crate::settings::terminal_font(cx))
            .text_size(px(12.))
            .text_color(hex(Chrome::FOREGROUND))
            .children(lines.into_iter().enumerate().map(|(i, line)| {
                // The signature first, the documentation after it quieter.
                div().truncate().when(i > 0, |d| d.text_color(hex(Chrome::MUTED))).child(line)
            }));
        Some(
            gpui::deferred(gpui::anchored().anchor(corner).position(at).snap_to_window_with_margin(self.popup_margin()).child(panel))
                .with_priority(2)
                .into_any_element(),
        )
    }

    pub(super) fn debug_type_hover(&self) -> serde_json::Value {
        match &self.type_hover {
            Some(h) => serde_json::json!({ "at": [h.start.line, h.start.col], "text": h.text }),
            None => serde_json::Value::Null,
        }
    }

    /// The pointer left what a shown window is about: the window closes a moment later, unless
    /// the pointer is back on it or on the window by then. Whatever is under the pointer then
    /// is hovered as usual.
    pub(super) fn leave_hovers(&mut self, cx: &mut Context<Self>) {
        self.hover_leaves += 1;
        let leave = self.hover_leaves;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GRACE).await;
            let _ = this.update(cx, |this, cx| {
                if this.hover_leaves != leave || this.popup_over {
                    return;
                }
                let pos = this.pointer.and_then(|p| this.text_position(p));
                if this.problem_hover.as_ref().is_some_and(|(p, shown)| *shown && this.problem_at(pos).as_ref() != Some(p)) {
                    this.problem_hover = None;
                    cx.notify();
                }
                let on_name = |h: &TypeHover| pos.is_some_and(|p| p.line == h.start.line && p.col >= h.start.col && p.col <= h.end.col);
                if this.type_hover.as_ref().is_some_and(|h| h.text.is_some() && !on_name(h)) {
                    this.type_hover = None;
                    cx.notify();
                }
                this.hover_problem(pos, cx);
                this.hover_type(pos, cx);
            });
        })
        .detach();
    }

    /// The pointer went onto a problem's or a type's window, or off it.
    pub(super) fn over_popup(&mut self, over: bool, cx: &mut Context<Self>) {
        self.popup_over = over;
        if !over {
            self.pointer = None;
            self.leave_hovers(cx);
        }
    }
}
