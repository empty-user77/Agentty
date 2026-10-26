//! A Word document shown as a page: headings, paragraphs with their bold and italic runs, lists
//! and tables. A click on a paragraph edits it in place; Enter starts the next paragraph and
//! Shift+Enter a line break, as in a word processor. ⌘S writes it back into the `.docx`.

use super::docx::{self, Block, Kind};
use super::{CodeEditor, Content};
use crate::i18n::t;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, popover, TypeScale};
use gpui::{
    div, prelude::*, px, ClickEvent, Context, Entity, FontStyle, FontWeight, HighlightStyle, MouseButton, MouseDownEvent, Pixels, Point,
    ScrollHandle, StrikethroughStyle, Subscription, UnderlineStyle, Window,
};
use std::path::PathBuf;

/// The page's width, as a printed page reads.
const PAGE_WIDTH: f32 = 760.;
/// Characters (wide ones counting twice) a line of the page holds, roughly: how tall the field
/// for an edited paragraph starts.
const LINE_CHARS: usize = 90;

/// A Word document open in the editor.
pub struct DocFile {
    pub doc: docx::Document,
    /// It can be written back (in the project, writable on disk).
    pub writable: bool,
    pub scroll: ScrollHandle,
}

impl DocFile {
    pub fn new(doc: docx::Document, writable: bool) -> Self {
        Self { doc, writable, scroll: ScrollHandle::new() }
    }
}

/// A paragraph being typed into.
pub(crate) struct ParagraphEdit {
    path: PathBuf,
    index: usize,
    input: Entity<TextInput>,
    _events: Subscription,
}

/// The right-click menu of a paragraph.
pub(crate) struct ParagraphMenu {
    at: Point<Pixels>,
    index: usize,
}

impl CodeEditor {
    fn doc_file_mut(&mut self) -> Option<&mut DocFile> {
        match self.docs.get_mut(self.active).map(|d| &mut d.content) {
            Some(Content::Docx(file)) => Some(file),
            _ => None,
        }
    }

    /// Starts typing into paragraph `index`.
    fn begin_paragraph(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_paragraph(cx);
        let Some(doc) = self.docs.get(self.active).filter(|d| d.editable()) else { return };
        let Content::Docx(file) = &doc.content else { return };
        let Some(Block::Paragraph(p)) = file.doc.items.get(index).map(|i| &i.block) else { return };
        if p.locked {
            return;
        }
        let text = p.text();
        let path = doc.path.clone();
        let wide: usize = text
            .split('\n')
            .map(|line| line.chars().map(|c| if (c as u32) >= 0x1100 { 2 } else { 1 }).sum::<usize>() / LINE_CHARS + 1)
            .sum();
        // Two rows at least: one row is a one-line field, which does not wrap.
        let rows = (wide + 1).clamp(2, 24);
        let input = cx.new(|cx| TextInput::new(text.clone(), "", window, cx).composer(rows));
        input.update(cx, |input, cx| {
            window.focus(&gpui::Focusable::focus_handle(input, cx));
            input.select_from(text.len(), cx);
        });
        let events = cx.subscribe_in(&input, window, |this, field, event: &TextInputEvent, window, cx| match event {
            // Enter: the next paragraph, as a word processor starts one.
            TextInputEvent::Confirmed => {
                let index = this.paragraph_edit.as_ref().map(|e| e.index);
                this.commit_paragraph(cx);
                if let (Some(index), Some(file)) = (index, this.doc_file_mut()) {
                    let at = file.doc.insert(index + 1, index);
                    this.tabs_changed(cx);
                    this.begin_paragraph(at, window, cx);
                }
            }
            TextInputEvent::Cancelled => {
                this.paragraph_edit = None;
                window.focus(&this.focus_handle);
                cx.notify();
            }
            // Only this field's own edit: Enter has already moved on to the next paragraph's. The
            // window going to the background takes the keyboard too: the edit stays open for when
            // it comes back.
            TextInputEvent::Blurred if window.is_window_active() && this.paragraph_edit.as_ref().is_some_and(|e| e.input == *field) => {
                this.commit_paragraph(cx)
            }
            _ => {}
        });
        self.paragraph_edit = Some(ParagraphEdit { path, index, input, _events: events });
        cx.notify();
    }

    /// Writes what was typed into its paragraph.
    pub(super) fn commit_paragraph(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.paragraph_edit.take() else { return };
        let text = edit.input.read(cx).text().to_string();
        let doc = self.docs.iter_mut().find(|d| d.path == edit.path);
        if let Some(Content::Docx(file)) = doc.map(|d| &mut d.content) {
            if file.doc.set_text(edit.index, &text) {
                self.tabs_changed(cx);
            }
        }
        cx.notify();
    }

    /// ⌘Z / ⇧⌘Z in a spreadsheet or a Word document: their own steps (they have no text). False
    /// for other files.
    pub(super) fn document_step(&mut self, back: bool, cx: &mut Context<Self>) -> bool {
        self.cell_edit = None;
        self.paragraph_edit = None;
        let stepped = match self.docs.get_mut(self.active).map(|d| &mut d.content) {
            Some(Content::Sheets(workbook)) => {
                if back {
                    workbook.undo()
                } else {
                    workbook.redo()
                }
            }
            Some(Content::Docx(file)) => {
                if back {
                    file.doc.undo()
                } else {
                    file.doc.redo()
                }
            }
            _ => return false,
        };
        if stepped {
            self.tabs_changed(cx);
        }
        cx.notify();
        true
    }

    fn paragraph_action(&mut self, index: usize, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.paragraph_menu = None;
        self.commit_paragraph(cx);
        let Some(file) = self.doc_file_mut() else { return };
        let at = match action {
            MenuAction::Above => Some(file.doc.insert(index, index)),
            MenuAction::Below => Some(file.doc.insert(index + 1, index)),
            MenuAction::Delete => {
                file.doc.remove(index);
                None
            }
        };
        self.tabs_changed(cx);
        if let Some(at) = at {
            self.begin_paragraph(at, window, cx);
        }
        cx.notify();
    }

    pub(super) fn render_docx(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(doc) = self.docs.get(self.active) else { return div().into_any_element() };
        let Content::Docx(file) = &doc.content else { return div().into_any_element() };
        let editable = doc.editable();
        let editing = self.paragraph_edit.as_ref().filter(|e| e.path == doc.path).map(|e| (e.index, e.input.clone()));
        let blocks: Vec<gpui::AnyElement> = file
            .doc
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match &item.block {
                Block::Paragraph(p) => {
                    let input = editing.as_ref().filter(|(at, _)| *at == i).map(|(_, input)| input.clone());
                    Some(self.render_paragraph(i, p, editable && !p.locked, input, cx).into_any_element())
                }
                Block::Table(rows) => Some(render_table(i, rows, editable, cx).into_any_element()),
                Block::Other => None,
            })
            .collect();
        let page = div()
            // A definite width (the page's, or the view's when narrower): the fields inside
            // measure their lines against it.
            .w(px(PAGE_WIDTH))
            .max_w_full()
            .mx_auto()
            .px_10()
            .pt_8()
            .pb_4()
            .flex()
            .flex_col()
            .gap_1()
            .text_color(hex(Chrome::FOREGROUND))
            .children(blocks)
            .when(editable, |d| {
                d.child(
                    div()
                        .id("doc-add")
                        .mt_2()
                        .px_2()
                        .py_1()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .rounded_md()
                        .t_small()
                        .text_color(hex(Chrome::MUTED))
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                        .child(icon("plus", 12., hex(Chrome::MUTED)))
                        .child(t(cx, "doc.add_paragraph"))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.commit_paragraph(cx);
                            let Some(file) = this.doc_file_mut() else { return };
                            let at = file.doc.insert_after(None);
                            this.tabs_changed(cx);
                            this.begin_paragraph(at, window, cx);
                        })),
                )
            });
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(div().id("doc-page").size_full().overflow_y_scroll().track_scroll(&file.scroll).child(page))
            .group(crate::ui::SCROLL_GROUP)
            .child(crate::ui::scrollbar(file.scroll.clone()))
            .children(self.render_paragraph_menu(cx))
            .into_any_element()
    }

    fn render_paragraph(
        &self,
        index: usize,
        p: &docx::Paragraph,
        editable: bool,
        input: Option<Entity<TextInput>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (size, weight) = match p.kind {
            Kind::Title => (28., FontWeight::BOLD),
            Kind::Heading(1) => (22., FontWeight::BOLD),
            Kind::Heading(2) => (18., FontWeight::BOLD),
            Kind::Heading(_) => (15., FontWeight::SEMIBOLD),
            Kind::Normal | Kind::Quote => (14., FontWeight::NORMAL),
        };
        let heading = !matches!(p.kind, Kind::Normal | Kind::Quote);
        let text = p.text();
        // Runs as one text with their formats laid over it.
        let mut highlights = Vec::new();
        let mut at = 0;
        for run in &p.runs {
            let end = at + run.text.len();
            if run.bold || run.italic || run.underline || run.strike {
                highlights.push((
                    at..end,
                    HighlightStyle {
                        font_weight: run.bold.then_some(FontWeight::BOLD),
                        font_style: run.italic.then_some(FontStyle::Italic),
                        underline: run.underline.then(|| UnderlineStyle { thickness: px(1.), ..Default::default() }),
                        strikethrough: run.strike.then(|| StrikethroughStyle { thickness: px(1.), ..Default::default() }),
                        ..Default::default()
                    },
                ));
            }
            at = end;
        }
        let body = match input {
            // A column stretches the field to the page's width (it has none of its own).
            Some(input) => div().w_full().flex().flex_col().child(input).into_any_element(),
            // An empty paragraph keeps a line's height, as it does on paper.
            None if text.is_empty() => div().child("\u{00a0}").into_any_element(),
            None => gpui::StyledText::new(text).with_highlights(highlights).into_any_element(),
        };
        let indent = p.list.map_or(0., |level| 18. * (level as f32 + 1.));
        div()
            .id(("doc-p", index))
            .relative()
            .w_full()
            .px_2()
            .py(px(if heading { 6. } else { 2. }))
            .when(heading, |d| d.mt_2())
            .pl(px(8. + indent))
            .rounded_sm()
            .text_size(px(size))
            .line_height(px(size * 1.55))
            .font_weight(weight)
            .when(heading, |d| d.text_color(hex(Chrome::BRIGHT)))
            .when(p.kind == Kind::Quote, |d| d.border_l_2().border_color(hex(Chrome::BORDER)).text_color(hex(Chrome::MUTED)).italic())
            .when_some(p.list, |d, level| {
                d.child(div().absolute().left(px(indent - 6.)).top(px(2.)).text_color(hex(Chrome::MUTED)).child(if level % 2 == 0 {
                    "•"
                } else {
                    "◦"
                }))
            })
            .when(editable, |d| {
                d.cursor(gpui::CursorStyle::IBeam)
                    .hover(|s| s.bg(hex_alpha(Chrome::BRIGHT, 0.04)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        let path = this.docs.get(this.active).map(|d| d.path.clone());
                        if this.paragraph_edit.as_ref().is_some_and(|e| e.index == index && Some(&e.path) == path.as_ref()) {
                            return;
                        }
                        this.paragraph_menu = None;
                        this.begin_paragraph(index, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.commit_paragraph(cx);
                            this.paragraph_menu = Some(ParagraphMenu { at: event.position, index });
                            cx.notify();
                        }),
                    )
            })
            .child(body)
    }

    fn render_paragraph_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.paragraph_menu.as_ref()?;
        let index = menu.index;
        let item = |id: &'static str, label: &'static str, action: MenuAction| {
            crate::ui::menu_item(
                id,
                t(cx, label),
                cx.listener(move |this, _: &ClickEvent, window, cx| this.paragraph_action(index, action, window, cx)),
            )
        };
        let list = popover()
            .w(px(200.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.paragraph_menu = None;
                cx.notify();
            }))
            .child(item("doc-above", "doc.insert_above", MenuAction::Above))
            .child(item("doc-below", "doc.insert_below", MenuAction::Below))
            .child(div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(item("doc-delete", "doc.delete", MenuAction::Delete));
        Some(gpui::deferred(gpui::anchored().position(menu.at).snap_to_window_with_margin(px(8.)).child(list)).with_priority(2))
    }
}

#[derive(Clone, Copy)]
enum MenuAction {
    Above,
    Below,
    Delete,
}

fn render_table(index: usize, rows: &[Vec<String>], editable: bool, cx: &mut Context<CodeEditor>) -> impl IntoElement {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    div()
        .id(("doc-table", index))
        .my_2()
        .flex()
        .flex_col()
        .border_1()
        .border_color(hex(Chrome::BORDER))
        .rounded_sm()
        .t_small()
        .when(editable, |d| {
            d.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.commit_paragraph(cx);
                    this.paragraph_menu = Some(ParagraphMenu { at: event.position, index });
                    cx.notify();
                }),
            )
        })
        .children(rows.iter().enumerate().map(|(r, row)| {
            div()
                .flex()
                .when(r > 0, |d| d.border_t_1().border_color(hex(Chrome::BORDER)))
                .when(r == 0, |d| d.bg(hex(Chrome::PANEL)))
                .children((0..width).map(|c| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_1()
                        .when(c > 0, |d| d.border_l_1().border_color(hex(Chrome::BORDER)))
                        .child(row.get(c).cloned().unwrap_or_default())
                }))
        }))
}
