//! The grid a CSV, TSV or spreadsheet file shows instead of its text: rows and columns as in a
//! spreadsheet, a filter, sorting by a column, cells edited in place, and copy and paste that
//! spreadsheets understand. A delimited file's edits are made to its text (only the cells that
//! changed), so saving, undo and an agent's writes to the file work as they do for any text;
//! a spreadsheet is shown, not edited. Sorting and filtering only change what is shown.

use super::buffer::{EditKind, Pos};
use super::table::{self, Edit, Table};
use super::{CodeEditor, Content, Document};
use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, popover, IconSize, Tooltip, TypeScale};
use gpui::{
    actions, div, prelude::*, px, App, ClickEvent, ClipboardItem, Context, Entity, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, ScrollHandle, ScrollStrategy, Subscription, UniformListScrollHandle, Window,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

actions!(
    table_grid,
    [
        GridUp,
        GridDown,
        GridLeft,
        GridRight,
        GridSelectUp,
        GridSelectDown,
        GridSelectLeft,
        GridSelectRight,
        GridPageUp,
        GridPageDown,
        GridTop,
        GridBottom,
        GridRowStart,
        GridRowEnd,
        GridEdit,
        GridNext,
        GridPrevious,
        GridClear,
        GridCopy,
        GridCut,
        GridPaste,
        GridSelectAll,
        GridFilter,
        GridEscape,
    ]
);

const CONTEXT: &str = "TableGrid";
const ROW_HEIGHT: f32 = 26.;
const NUMBER_WIDTH: f32 = 52.;
const COLUMN_MIN: f32 = 56.;
const COLUMN_MAX: f32 = 360.;
const COLUMN_DRAG_MAX: f32 = 1200.;
const CHAR_WIDTH: f32 = 7.4;
/// Rows measured for the columns' widths: enough to see what a column holds, cheap on a big file.
const MEASURED_ROWS: usize = 300;
/// Rows a page key moves.
const PAGE_ROWS: isize = 20;

pub fn bind_keys(cx: &mut App) {
    use crate::key;
    let c = Some(CONTEXT);
    cx.bind_keys([
        key("up", GridUp, c),
        key("down", GridDown, c),
        key("left", GridLeft, c),
        key("right", GridRight, c),
        key("shift-up", GridSelectUp, c),
        key("shift-down", GridSelectDown, c),
        key("shift-left", GridSelectLeft, c),
        key("shift-right", GridSelectRight, c),
        key("pageup", GridPageUp, c),
        key("pagedown", GridPageDown, c),
        key("cmd-up", GridTop, c),
        key("cmd-down", GridBottom, c),
        key("home", GridRowStart, c),
        key("end", GridRowEnd, c),
        key("cmd-left", GridRowStart, c),
        key("cmd-right", GridRowEnd, c),
        key("enter", GridEdit, c),
        key("f2", GridEdit, c),
        key("tab", GridNext, c),
        key("shift-tab", GridPrevious, c),
        key("backspace", GridClear, c),
        key("delete", GridClear, c),
        key("cmd-c", GridCopy, c),
        key("cmd-x", GridCut, c),
        key("cmd-v", GridPaste, c),
        key("cmd-a", GridSelectAll, c),
        key("cmd-f", GridFilter, c),
        key("escape", GridEscape, c),
    ]);
}

/// A cell of the grid as shown: (row among the shown rows, column).
type At = (usize, usize);

/// What the grid of one open file remembers.
pub(crate) struct Grid {
    /// The grid shows instead of the text (a delimited file; a spreadsheet always shows it).
    pub on: bool,
    /// The delimiter the user picked; `None` is the one found in the file.
    delimiter: Option<char>,
    detected: std::cell::Cell<Option<char>>,
    /// The first row names the columns.
    header: bool,
    /// The spreadsheet's sheet shown.
    sheet: usize,
    cursor: At,
    anchor: At,
    /// Shown sorted by a column (index, ascending).
    sort: Option<(usize, bool)>,
    filter: String,
    /// Widths the user dragged columns to.
    widths: HashMap<usize, f32>,
    dragging: bool,
    table: RefCell<Option<(u64, char, Arc<Table>)>>,
    shown: RefCell<Option<(ShownKey, Arc<Vec<usize>>)>>,
    measured: RefCell<Option<Measured>>,
    scroll: UniformListScrollHandle,
    hscroll: ScrollHandle,
}

/// Columns' widths measured for a table (its address, whether its first row is the header).
type Measured = (usize, bool, Rc<Vec<f32>>);

#[derive(Clone, PartialEq)]
struct ShownKey {
    table: usize,
    header: bool,
    sort: Option<(usize, bool)>,
    filter: String,
}

impl Grid {
    pub fn new(on: bool) -> Self {
        Self {
            on,
            delimiter: None,
            detected: Default::default(),
            header: true,
            sheet: 0,
            cursor: (0, 0),
            anchor: (0, 0),
            sort: None,
            filter: String::new(),
            widths: HashMap::new(),
            dragging: false,
            table: RefCell::new(None),
            shown: RefCell::new(None),
            measured: RefCell::new(None),
            scroll: UniformListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
        }
    }

    /// The selected block: (first row, first column), (last row, last column).
    fn range(&self) -> (At, At) {
        let (a, b) = (self.anchor, self.cursor);
        ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)))
    }

    /// The table's first row names its columns: asked for, and the row holds something (an empty
    /// sheet starts with its first row as data).
    fn names(&self, table: &Table) -> bool {
        self.header && table.rows.first().is_some_and(|r| r.cells.iter().any(|c| !c.value.trim().is_empty()))
    }

    fn selected(&self, at: At) -> bool {
        let ((r0, c0), (r1, c1)) = self.range();
        (r0..=r1).contains(&at.0) && (c0..=c1).contains(&at.1)
    }
}

/// A cell being typed into.
pub(crate) struct CellEdit {
    /// The file it is in (another may be in front by the time it is written).
    path: std::path::PathBuf,
    /// The table's row (not the shown row: sorting does not move what is being edited).
    row: usize,
    col: usize,
    input: Entity<TextInput>,
    _events: Subscription,
}

/// Typing on the grid, as the system hands it over (input methods included): it starts editing the
/// cursor's cell with what was typed, and goes on to that cell's field while it is being edited, so a
/// Korean or Japanese composition begun on the grid carries on in the cell.
pub(crate) struct GridIme {
    editor: gpui::WeakEntity<CodeEditor>,
}

impl GridIme {
    /// The field of the cell being edited.
    fn field(&self, cx: &App) -> Option<Entity<TextInput>> {
        self.editor.upgrade()?.read(cx).cell_edit.as_ref().map(|e| e.input.clone())
    }

    /// The cell's field, the cell being opened first when none is.
    fn open_field(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<TextInput>> {
        if let Some(field) = self.field(cx) {
            return Some(field);
        }
        let editor = self.editor.upgrade()?;
        editor.update(cx, |editor, cx| editor.begin_cell(Some(String::new()), window, cx));
        self.field(cx)
    }
}

impl gpui::EntityInputHandler for GridIme {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted: &mut Option<std::ops::Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        match self.field(cx) {
            Some(field) => field.update(cx, |field, cx| field.text_for_range(range, adjusted, window, cx)),
            None => {
                adjusted.replace(0..0);
                Some(String::new())
            }
        }
    }

    fn selected_text_range(&mut self, ignore_disabled: bool, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::UTF16Selection> {
        match self.field(cx) {
            Some(field) => field.update(cx, |field, cx| field.selected_text_range(ignore_disabled, window, cx)),
            None => Some(gpui::UTF16Selection { range: 0..0, reversed: false }),
        }
    }

    fn marked_text_range(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        self.field(cx)?.update(cx, |field, cx| field.marked_text_range(window, cx))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.field(cx) {
            field.update(cx, |field, cx| field.unmark_text(window, cx));
        }
    }

    fn replace_text_in_range(&mut self, range: Option<std::ops::Range<usize>>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        // A control character (Enter, Tab) is a key the grid's own bindings handle, not text.
        if self.field(cx).is_none() && text.chars().any(char::is_control) {
            return;
        }
        if let Some(field) = self.open_field(window, cx) {
            field.update(cx, |field, cx| field.replace_text_in_range(range, text, window, cx));
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        selected: Option<std::ops::Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.open_field(window, cx) {
            field.update(cx, |field, cx| field.replace_and_mark_text_in_range(range, text, selected, window, cx));
        }
    }

    fn bounds_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        bounds: gpui::Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<Pixels>> {
        self.field(cx)?.update(cx, |field, cx| field.bounds_for_range(range, bounds, window, cx))
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        self.field(cx)?.update(cx, |field, cx| field.character_index_for_point(point, window, cx))
    }
}

/// The right-click menu of the grid, where it was opened.
pub(crate) struct GridMenu {
    at: Point<Pixels>,
    /// Opened on a column's name (column tools only).
    header: bool,
}

impl Document {
    /// A delimited file, which can be shown as a grid.
    pub(super) fn can_grid(&self) -> bool {
        matches!(self.content, Content::Text) && table::is_delimited(&self.path)
    }

    pub(super) fn showing_grid(&self) -> bool {
        (self.grid.on && self.can_grid()) || matches!(self.content, Content::Sheets(_))
    }

    /// The table the grid shows, read again only when the text changed.
    fn grid_table(&self) -> Arc<Table> {
        if let Content::Sheets(workbook) = &self.content {
            let sheets = &workbook.sheets;
            return sheets.get(self.grid.sheet).or(sheets.first()).map(|(_, t)| t.clone()).unwrap_or_default();
        }
        let revision = self.buffer.revision();
        let delimiter = self.grid.delimiter.unwrap_or_else(|| self.detected_delimiter());
        if let Some((_, _, table)) = self.grid.table.borrow().as_ref().filter(|(r, d, _)| *r == revision && *d == delimiter) {
            return table.clone();
        }
        let table = Arc::new(table::parse(&self.buffer.text(), delimiter));
        *self.grid.table.borrow_mut() = Some((revision, delimiter, table.clone()));
        table
    }

    fn detected_delimiter(&self) -> char {
        if let Some(found) = self.grid.detected.get() {
            return found;
        }
        let sample: Vec<&str> = (0..self.buffer.line_count().min(20)).map(|i| self.buffer.line(i)).collect();
        let found = table::detect_delimiter(&self.path, &sample.join("\n"));
        self.grid.detected.set(Some(found));
        found
    }

    /// The table's rows the grid shows, in the order it shows them (the header row left out).
    fn grid_shown(&self, table: &Arc<Table>) -> Arc<Vec<usize>> {
        let grid = &self.grid;
        let key = ShownKey { table: Arc::as_ptr(table) as usize, header: grid.names(table), sort: grid.sort, filter: grid.filter.clone() };
        if let Some((_, rows)) = grid.shown.borrow().as_ref().filter(|(k, _)| *k == key) {
            return rows.clone();
        }
        let first = usize::from(grid.names(table));
        let needle = grid.filter.trim().to_lowercase();
        let mut rows: Vec<usize> = (first..table.rows.len())
            .filter(|&r| needle.is_empty() || table.rows[r].cells.iter().any(|c| c.value.to_lowercase().contains(&needle)))
            .collect();
        if let Some((col, ascending)) = grid.sort {
            rows.sort_by(|&a, &b| {
                let order = table::compare(table.value(a, col), table.value(b, col));
                if ascending {
                    order
                } else {
                    order.reverse()
                }
            });
        }
        let rows = Arc::new(rows);
        *grid.shown.borrow_mut() = Some((key, rows.clone()));
        rows
    }

    /// Each column's width: what it holds, unless the user dragged it.
    fn grid_widths(&self, table: &Arc<Table>) -> Rc<Vec<f32>> {
        let ptr = Arc::as_ptr(table) as usize;
        let measured = self
            .grid
            .measured
            .borrow()
            .as_ref()
            .filter(|(p, h, _)| *p == ptr && *h == self.grid.names(table))
            .map(|(_, _, widths)| widths.clone());
        let measured = measured.unwrap_or_else(|| {
            // One empty column past the last, as a spreadsheet goes on to the right: typing there
            // adds a column.
            let columns = if self.editable() { table.width + 1 } else { table.width.max(1) };
            let widths: Vec<f32> = (0..columns)
                .map(|col| {
                    let name = if self.grid.names(table) { 0 } else { column_name(col).len() };
                    let longest =
                        table.rows.iter().take(MEASURED_ROWS).map(|r| text_width(r.cells.get(col).map_or("", |c| &c.value))).max();
                    ((longest.unwrap_or(0).max(name) as f32) * CHAR_WIDTH + 24.).clamp(COLUMN_MIN, COLUMN_MAX)
                })
                .collect();
            let widths = Rc::new(widths);
            *self.grid.measured.borrow_mut() = Some((ptr, self.grid.names(table), widths.clone()));
            widths
        });
        if self.grid.widths.is_empty() {
            return measured;
        }
        Rc::new(measured.iter().enumerate().map(|(i, w)| self.grid.widths.get(&i).copied().unwrap_or(*w)).collect())
    }
}

/// Columns a value takes on screen: wide (CJK, Hangul) characters count twice. Its first line only.
fn text_width(value: &str) -> usize {
    value.lines().next().unwrap_or("").chars().take(80).map(|c| if (c as u32) >= 0x1100 { 2 } else { 1 }).sum()
}

/// A spreadsheet's name for a column: A, B, … Z, AA, AB, …
pub fn column_name(mut col: usize) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (col % 26) as u8);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

/// What a cell shows on its one line: the first line of the value, marked when there is more.
fn one_line(value: &str) -> String {
    match value.split_once('\n') {
        Some((first, _)) => format!("{first} …"),
        None => value.to_string(),
    }
}

fn delimiter_name(delimiter: char) -> &'static str {
    match delimiter {
        ';' => "table.delimiter.semicolon",
        '\t' => "table.delimiter.tab",
        '|' => "table.delimiter.pipe",
        _ => "table.delimiter.comma",
    }
}

impl CodeEditor {
    fn grid_doc(&self) -> Option<&Document> {
        self.docs.get(self.active).filter(|d| d.showing_grid())
    }

    fn grid_doc_mut(&mut self) -> Option<&mut Document> {
        self.docs.get_mut(self.active).filter(|d| d.showing_grid())
    }

    /// The table, the rows shown and the selection's cells in the table: (table row, column).
    fn grid_state(&self) -> Option<(Arc<Table>, Arc<Vec<usize>>)> {
        let doc = self.grid_doc()?;
        let table = doc.grid_table();
        let shown = doc.grid_shown(&table);
        Some((table, shown))
    }

    /// Shows the file as a grid or as its text.
    pub(super) fn toggle_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_cell(cx);
        let Some(doc) = self.docs.get_mut(self.active).filter(|d| d.can_grid()) else { return };
        doc.grid.on = !doc.grid.on;
        if doc.grid.on {
            window.focus(&self.grid_focus);
        } else {
            window.focus(&self.focus_handle);
        }
        cx.notify();
    }

    /// Opens the filter (⌘F while the grid shows).
    pub(super) fn focus_grid_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.grid_filter_input(window, cx);
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
    }

    fn grid_filter_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some((input, _)) = self.grid_filter.as_ref() {
            return input.clone();
        }
        let input = cx.new(|cx| TextInput::localized("", "table.filter", window, cx));
        let events = cx.subscribe_in(&input, window, |this, input, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Changed => {
                let text = input.read(cx).text().to_string();
                if let Some(doc) = this.grid_doc_mut() {
                    doc.grid.filter = text;
                    doc.grid.cursor = (0, doc.grid.cursor.1);
                    doc.grid.anchor = doc.grid.cursor;
                    doc.grid.scroll.scroll_to_item(0, ScrollStrategy::Top);
                }
                cx.notify();
            }
            TextInputEvent::Confirmed | TextInputEvent::Cancelled | TextInputEvent::Down => window.focus(&this.grid_focus),
            _ => {}
        });
        self.grid_filter = Some((input.clone(), events));
        input
    }

    fn grid_move(&mut self, rows: isize, cols: isize, select: bool, cx: &mut Context<Self>) {
        let Some((table, shown)) = self.grid_state() else { return };
        let columns = self.grid_doc().map_or(table.width, |d| d.grid_widths(&table).len());
        let (last_row, last_col) = (shown.len().saturating_sub(1), columns.saturating_sub(1));
        let Some(doc) = self.grid_doc_mut() else { return };
        let (row, col) = doc.grid.cursor;
        let to = ((row as isize + rows).clamp(0, last_row as isize) as usize, (col as isize + cols).clamp(0, last_col as isize) as usize);
        self.grid_go(to, select, cx);
    }

    /// Puts the cursor on `to` (shown row, column), keeping the selection's start when `select`.
    fn grid_go(&mut self, to: At, select: bool, cx: &mut Context<Self>) {
        let Some((table, _)) = self.grid_state() else { return };
        let Some(doc) = self.grid_doc_mut() else { return };
        doc.grid.cursor = to;
        if !select {
            doc.grid.anchor = to;
        }
        doc.grid.scroll.scroll_to_item(to.0, ScrollStrategy::Top);
        // Sideways too: the cell's column comes into view.
        let widths = doc.grid_widths(&table);
        let left: f32 = NUMBER_WIDTH + widths.iter().take(to.1).sum::<f32>();
        let right = left + widths.get(to.1).copied().unwrap_or(COLUMN_MIN);
        let view = f32::from(doc.grid.hscroll.bounds().size.width);
        let mut offset = doc.grid.hscroll.offset();
        let scrolled = -f32::from(offset.x);
        if view > 0. {
            if left - NUMBER_WIDTH < scrolled {
                offset.x = px(-(left - NUMBER_WIDTH).max(0.));
            } else if right > scrolled + view {
                offset.x = px(-(right - view));
            }
            doc.grid.hscroll.set_offset(offset);
        }
        cx.notify();
    }

    /// Starts typing into the cursor's cell: with its value, or replacing it with `typed`.
    fn begin_cell(&mut self, typed: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((table, shown)) = self.grid_state() else { return };
        let Some(doc) = self.grid_doc().filter(|d| d.editable()) else { return };
        let (view_row, col) = doc.grid.cursor;
        // An empty table (or one with only its header) gets its first row when typed into.
        let row = shown.get(view_row).copied().unwrap_or(table.rows.len());
        let value = typed.clone().unwrap_or_else(|| table.value(row, col).to_string());
        let multiline = value.contains('\n');
        let input = cx.new(|cx| {
            let input = TextInput::new(value.clone(), "", window, cx);
            if multiline {
                input.composer(4)
            } else {
                input
            }
        });
        input.update(cx, |input, cx| {
            window.focus(&input.focus_handle(cx));
            input.select_from(value.len(), cx);
        });
        let events = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed | TextInputEvent::Down => this.finish_cell((1, 0), window, cx),
            TextInputEvent::Up => this.finish_cell((-1, 0), window, cx),
            TextInputEvent::Next => this.finish_cell((0, 1), window, cx),
            TextInputEvent::Previous => this.finish_cell((0, -1), window, cx),
            TextInputEvent::Cancelled => {
                this.cell_edit = None;
                window.focus(&this.grid_focus);
                cx.notify();
            }
            // Not when the window only went to the background: the cell stays open for its return.
            TextInputEvent::Blurred if window.is_window_active() => this.commit_cell(cx),
            _ => {}
        });
        let path = doc.path.clone();
        self.cell_edit = Some(CellEdit { path, row, col, input, _events: events });
        cx.notify();
    }

    fn finish_cell(&mut self, step: (isize, isize), window: &mut Window, cx: &mut Context<Self>) {
        self.commit_cell(cx);
        window.focus(&self.grid_focus);
        self.grid_move(step.0, step.1, false, cx);
    }

    /// Writes what was typed into the cell being edited (nothing when it did not change).
    pub(super) fn commit_cell(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.cell_edit.take() else { return };
        let value = edit.input.read(cx).text().to_string();
        let Some(index) = self.docs.iter().position(|d| d.path == edit.path) else { return };
        // Written into its own file, whichever is in front now.
        let front = std::mem::replace(&mut self.active, index);
        if let Some((table, _)) = self.grid_state() {
            if edit.row >= table.rows.len() || table.value(edit.row, edit.col) != value {
                self.grid_set(&table, vec![(edit.row, edit.col, value)], cx);
            }
        }
        self.active = front;
        cx.notify();
    }

    /// Sets cells (table row, column, value): in a spreadsheet's sheet, or in the file's text.
    fn grid_set(&mut self, table: &Table, changes: Vec<(usize, usize, String)>, cx: &mut Context<Self>) {
        let Some(doc) = self.grid_doc_mut().filter(|d| d.editable()) else { return };
        let sheet = doc.grid.sheet;
        if let Content::Sheets(workbook) = &mut doc.content {
            workbook.set(sheet, &changes);
            self.tabs_changed(cx);
            cx.notify();
            return;
        }
        let edits = table.set_cells(&changes);
        self.grid_apply(table, edits, cx);
    }

    /// Makes `edits` to the file's text: one undo step, whatever they touch.
    fn grid_apply(&mut self, table: &Table, edits: Vec<Edit>, cx: &mut Context<Self>) {
        if edits.is_empty() || !self.grid_doc().is_some_and(Document::editable) {
            return;
        }
        let whole = (edits.len() > 1).then(|| table.apply(&edits));
        self.edit(cx, |b| {
            b.seal();
            match (&whole, edits.first()) {
                (Some(text), _) => {
                    let end = b.end();
                    b.replace(Pos::new(0, 0), end, text, EditKind::Other);
                }
                (None, Some(edit)) => {
                    b.replace(edit.start, edit.end, &edit.text, EditKind::Other);
                }
                (None, None) => {}
            }
            b.seal();
        });
    }

    /// The selected cells as (table row, column) and their shown positions, row by row.
    fn grid_selection(&self) -> Vec<Vec<(usize, usize)>> {
        let (Some(doc), Some((_, shown))) = (self.grid_doc(), self.grid_state()) else { return Vec::new() };
        let ((r0, c0), (r1, c1)) = doc.grid.range();
        (r0..=r1).filter_map(|r| shown.get(r).map(|&row| (c0..=c1).map(|c| (row, c)).collect())).collect()
    }

    fn grid_copy(&mut self, cx: &mut Context<Self>) {
        let Some((table, _)) = self.grid_state() else { return };
        let rows: Vec<Vec<String>> =
            self.grid_selection().iter().map(|row| row.iter().map(|&(r, c)| table.value(r, c).to_string()).collect()).collect();
        // One cell copies as its value, so it pastes as itself anywhere.
        let text = match rows.as_slice() {
            [row] if row.len() == 1 => row[0].clone(),
            _ => table::to_clipboard(&rows),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn grid_clear(&mut self, cx: &mut Context<Self>) {
        let Some((table, _)) = self.grid_state() else { return };
        let changes: Vec<(usize, usize, String)> = self
            .grid_selection()
            .into_iter()
            .flatten()
            .filter(|&(r, c)| !table.value(r, c).is_empty())
            .map(|(r, c)| (r, c, String::new()))
            .collect();
        self.grid_set(&table, changes, cx);
    }

    /// Pastes cells from the clipboard at the cursor; one value pasted over a selection fills it.
    fn grid_paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        let Some((table, shown)) = self.grid_state() else { return };
        let Some(doc) = self.grid_doc() else { return };
        let block = table::from_clipboard(&text);
        let mut changes = Vec::new();
        if let [row] = block.as_slice() {
            if let [value] = row.as_slice() {
                for (r, c) in self.grid_selection().into_iter().flatten() {
                    changes.push((r, c, value.clone()));
                }
            }
        }
        if changes.is_empty() {
            let (top, left) = doc.grid.range().0;
            // Rows past the shown ones are added after the table's last row.
            let mut extra = table.rows.len();
            for (i, values) in block.iter().enumerate() {
                let row = match shown.get(top + i) {
                    Some(&row) => row,
                    None => {
                        extra += 1;
                        extra - 1
                    }
                };
                for (j, value) in values.iter().enumerate() {
                    changes.push((row, left + j, value.clone()));
                }
            }
            if let Some(doc) = self.grid_doc_mut() {
                let rows = block.len().max(1) - 1;
                let cols = block.iter().map(Vec::len).max().unwrap_or(1).max(1) - 1;
                doc.grid.anchor = (top, left);
                doc.grid.cursor = (top + rows, left + cols);
            }
        }
        self.grid_set(&table, changes, cx);
    }

    fn grid_insert_row(&mut self, below: bool, cx: &mut Context<Self>) {
        let Some((table, shown)) = self.grid_state() else { return };
        let Some(doc) = self.grid_doc() else { return };
        let ((r0, _), (r1, _)) = doc.grid.range();
        let row =
            if below { shown.get(r1).map_or(table.rows.len(), |r| r + 1) } else { shown.get(r0).copied().unwrap_or(table.rows.len()) };
        let edit = table.insert_row(row);
        self.grid_apply(&table, vec![edit], cx);
    }

    fn grid_delete_rows(&mut self, cx: &mut Context<Self>) {
        let Some((table, _)) = self.grid_state() else { return };
        let rows: Vec<usize> = self.grid_selection().iter().filter_map(|row| row.first().map(|&(r, _)| r)).collect();
        let edits = table.delete_rows(&rows);
        self.grid_apply(&table, edits, cx);
        let shown_len = self.grid_state().map_or(0, |(_, shown)| shown.len());
        if let Some(doc) = self.grid_doc_mut() {
            let row = doc.grid.range().0 .0.min(shown_len.saturating_sub(1));
            doc.grid.cursor = (row, doc.grid.cursor.1);
            doc.grid.anchor = doc.grid.cursor;
        }
    }

    fn grid_insert_column(&mut self, right: bool, cx: &mut Context<Self>) {
        let Some((table, _)) = self.grid_state() else { return };
        let Some(doc) = self.grid_doc() else { return };
        let ((_, c0), (_, c1)) = doc.grid.range();
        let edits = table.insert_column(if right { c1 + 1 } else { c0 });
        self.grid_apply(&table, edits, cx);
    }

    fn grid_delete_column(&mut self, cx: &mut Context<Self>) {
        let Some((table, _)) = self.grid_state() else { return };
        let Some(doc) = self.grid_doc() else { return };
        let col = doc.grid.cursor.1;
        let edits = table.delete_column(col);
        self.grid_apply(&table, edits, cx);
        if let Some(doc) = self.grid_doc_mut() {
            doc.grid.sort = doc.grid.sort.filter(|(c, _)| *c != col);
            doc.grid.widths.clear();
        }
    }

    fn grid_sort(&mut self, col: usize, ascending: Option<bool>, cx: &mut Context<Self>) {
        let Some(doc) = self.grid_doc_mut() else { return };
        doc.grid.sort = ascending.map(|a| (col, a));
        doc.grid.cursor = (0, col);
        doc.grid.anchor = doc.grid.cursor;
        cx.notify();
    }

    fn grid_mouse_down(&mut self, at: At, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_cell(cx);
        self.grid_menu = None;
        window.focus(&self.grid_focus);
        let Some(doc) = self.grid_doc_mut() else { return };
        if event.button == MouseButton::Right {
            if !doc.grid.selected(at) {
                doc.grid.cursor = at;
                doc.grid.anchor = at;
            }
            self.grid_menu = Some(GridMenu { at: event.position, header: false });
            cx.notify();
            return;
        }
        doc.grid.cursor = at;
        if !event.modifiers.shift {
            doc.grid.anchor = at;
        }
        doc.grid.dragging = true;
        if event.click_count == 2 {
            doc.grid.dragging = false;
            self.begin_cell(None, window, cx);
        }
        cx.notify();
    }

    fn grid_mouse_move(&mut self, at: At, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(doc) = self.grid_doc_mut() else { return };
        if !doc.grid.dragging || event.pressed_button != Some(MouseButton::Left) || doc.grid.cursor == at {
            return;
        }
        doc.grid.cursor = at;
        cx.notify();
    }

    fn grid_mouse_up(&mut self, cx: &mut Context<Self>) {
        if let Some(doc) = self.grid_doc_mut() {
            doc.grid.dragging = false;
        }
        let _ = cx;
    }

    /// The whole row (its number clicked) or the whole column (its name clicked) selected.
    fn grid_select_line(&mut self, row: Option<usize>, col: Option<usize>, extend: bool, cx: &mut Context<Self>) {
        let Some((table, shown)) = self.grid_state() else { return };
        let Some(doc) = self.grid_doc_mut() else { return };
        // The table's own columns: the empty one past them is not part of a row.
        let (last_row, last_col) = (shown.len().saturating_sub(1), table.width.saturating_sub(1));
        match (row, col) {
            (Some(r), _) => {
                let from = if extend { doc.grid.anchor.0 } else { r };
                doc.grid.anchor = (from, 0);
                doc.grid.cursor = (r, last_col);
            }
            (_, Some(c)) => {
                let from = if extend { doc.grid.anchor.1 } else { c };
                doc.grid.anchor = (0, from);
                doc.grid.cursor = (last_row, c);
            }
            _ => {}
        }
        cx.notify();
    }

    // -- rendering --------------------------------------------------------------------------

    /// The toolbar's switch between the grid and the text (for delimited files).
    pub(super) fn render_grid_switch(&self, doc: &Document, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !doc.can_grid() {
            return None;
        }
        let segment = |id: &'static str, glyph: &'static str, active: bool, tip: &'static str| {
            div()
                .id(id)
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .when(active, |d| d.bg(hex(Chrome::SELECTED)))
                .when(!active, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
                .child(icon(glyph, IconSize::INLINE, hex(if active { Chrome::BRIGHT } else { Chrome::MUTED })))
                .tooltip(Tooltip::text(t(cx, tip), None))
                .when(!active, |d| d.on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_grid(window, cx))))
        };
        let on = doc.grid.on;
        Some(
            div()
                .flex_shrink_0()
                .p(px(1.))
                .flex()
                .gap(px(1.))
                .rounded_md()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .child(segment("editor-grid-text", "code", !on, "table.show_text"))
                .child(segment("editor-grid-table", "table", on, "table.show_table")),
        )
    }

    pub(super) fn render_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let filter = self.grid_filter_input(window, cx);
        let editor = cx.weak_entity();
        let ime = self.grid_ime.get_or_insert_with(|| cx.new(|_| GridIme { editor })).clone();
        let focus = self.grid_focus.clone();
        // The field follows the file in front: each file keeps its own filter.
        if let Some(doc) = self.grid_doc() {
            if filter.read(cx).text() != doc.grid.filter {
                let text = doc.grid.filter.clone();
                filter.update(cx, |input, cx| input.set_text(text, cx));
            }
        }
        // The editor itself was given the keyboard (its tab chosen), or nothing has it (the field
        // that had it went with another file): the grid takes it.
        if self.focus_handle.is_focused(window) || window.focused(cx).is_none() {
            window.focus(&self.grid_focus);
        }
        let Some(doc) = self.grid_doc() else { return div().into_any_element() };
        let table = doc.grid_table();
        let shown = doc.grid_shown(&table);
        let widths = doc.grid_widths(&table);
        let editable = doc.editable();
        let total = NUMBER_WIDTH + widths.iter().sum::<f32>();
        let columns = self.grid_columns(&widths, doc);
        let scroll = doc.grid.scroll.clone();
        let base = scroll.0.borrow().base_handle.clone();
        let hscroll = doc.grid.hscroll.clone();
        let count = shown.len() + usize::from(editable);
        let sheets = match &doc.content {
            Content::Sheets(workbook) if workbook.sheets.len() > 1 => {
                Some((workbook.sheets.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(), doc.grid.sheet))
            }
            _ => None,
        };
        let top = div()
            .h(px(36.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(
                div()
                    .w(px(280.))
                    .h(px(26.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .bg(hex(Chrome::PANEL))
                    .child(icon("search", 12., hex(Chrome::MUTED)))
                    .child(div().flex_1().min_w_0().t_small().child(filter)),
            )
            .when(!doc.grid.filter.trim().is_empty(), |d| {
                d.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(cx, "table.matches", &[("n", &shown.len().to_string())])))
            });
        let header = self.render_grid_header(&table, &widths, &columns, total, cx);
        let rows = gpui::uniform_list(
            "table-rows",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| this.render_grid_rows(range, cx)),
        )
        .track_scroll(scroll)
        .flex_1()
        .min_h_0();
        let grid = div()
            .id("table-grid")
            .key_context(CONTEXT)
            .track_focus(&self.grid_focus)
            .flex_1()
            .min_h_0()
            .relative()
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.grid_mouse_up(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.grid_mouse_up(cx)))
            .on_action(cx.listener(|this, _: &GridUp, _, cx| this.grid_move(-1, 0, false, cx)))
            .on_action(cx.listener(|this, _: &GridDown, _, cx| this.grid_move(1, 0, false, cx)))
            .on_action(cx.listener(|this, _: &GridLeft, _, cx| this.grid_move(0, -1, false, cx)))
            .on_action(cx.listener(|this, _: &GridRight, _, cx| this.grid_move(0, 1, false, cx)))
            .on_action(cx.listener(|this, _: &GridSelectUp, _, cx| this.grid_move(-1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &GridSelectDown, _, cx| this.grid_move(1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &GridSelectLeft, _, cx| this.grid_move(0, -1, true, cx)))
            .on_action(cx.listener(|this, _: &GridSelectRight, _, cx| this.grid_move(0, 1, true, cx)))
            .on_action(cx.listener(|this, _: &GridPageUp, _, cx| this.grid_move(-PAGE_ROWS, 0, false, cx)))
            .on_action(cx.listener(|this, _: &GridPageDown, _, cx| this.grid_move(PAGE_ROWS, 0, false, cx)))
            .on_action(cx.listener(|this, _: &GridTop, _, cx| this.grid_move(isize::MIN / 2, 0, false, cx)))
            .on_action(cx.listener(|this, _: &GridBottom, _, cx| this.grid_move(isize::MAX / 2, 0, false, cx)))
            .on_action(cx.listener(|this, _: &GridRowStart, _, cx| this.grid_move(0, isize::MIN / 2, false, cx)))
            .on_action(cx.listener(|this, _: &GridRowEnd, _, cx| this.grid_move(0, isize::MAX / 2, false, cx)))
            .on_action(cx.listener(|this, _: &GridNext, _, cx| this.grid_move(0, 1, false, cx)))
            .on_action(cx.listener(|this, _: &GridPrevious, _, cx| this.grid_move(0, -1, false, cx)))
            .on_action(cx.listener(|this, _: &GridEdit, window, cx| this.begin_cell(None, window, cx)))
            .on_action(cx.listener(|this, _: &GridClear, _, cx| this.grid_clear(cx)))
            .on_action(cx.listener(|this, _: &GridCopy, _, cx| this.grid_copy(cx)))
            .on_action(cx.listener(|this, _: &GridCut, _, cx| {
                this.grid_copy(cx);
                this.grid_clear(cx);
            }))
            .on_action(cx.listener(|this, _: &GridPaste, _, cx| this.grid_paste(cx)))
            .on_action(cx.listener(|this, _: &GridSelectAll, _, cx| {
                this.grid_go((0, 0), false, cx);
                this.grid_move(isize::MAX / 2, isize::MAX / 2, true, cx);
            }))
            .on_action(cx.listener(|this, _: &GridFilter, window, cx| this.focus_grid_filter(window, cx)))
            .on_action(cx.listener(|this, _: &GridEscape, _, cx| {
                this.grid_menu = None;
                if let Some(doc) = this.grid_doc_mut() {
                    doc.grid.anchor = doc.grid.cursor;
                }
                cx.notify();
            }))
            .child(
                div()
                    .id("table-scroll")
                    .size_full()
                    .overflow_x_scroll()
                    .track_scroll(&hscroll)
                    .child(div().h_full().w(px(total)).flex().flex_col().child(header).child(rows)),
            )
            .group(crate::ui::SCROLL_GROUP)
            .child(crate::ui::scrollbar(base))
            .child(crate::ui::scrollbar_h(hscroll))
            // What is typed on a cell (an input method's composition too) reaches the grid here.
            .child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| window.handle_input(&focus, gpui::ElementInputHandler::new(bounds, ime), cx),
                )
                .absolute()
                .size_full(),
            );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(top)
            .child(grid)
            .children(sheets.map(|(names, active)| self.render_sheet_tabs(names, active, cx)))
            .children(self.render_grid_menu(cx))
            .into_any_element()
    }

    /// The columns in view (left of the first and right of the last only take room).
    fn grid_columns(&self, widths: &[f32], doc: &Document) -> std::ops::Range<usize> {
        let view = f32::from(doc.grid.hscroll.bounds().size.width);
        let scrolled = -f32::from(doc.grid.hscroll.offset().x);
        if view <= 0. {
            return 0..widths.len().min(40);
        }
        let mut x = NUMBER_WIDTH;
        let (mut first, mut last) = (widths.len(), widths.len());
        for (i, w) in widths.iter().enumerate() {
            if first == widths.len() && x + w >= scrolled {
                first = i;
            }
            if x > scrolled + view {
                last = i;
                break;
            }
            x += w;
        }
        first.min(widths.len())..last
    }

    fn render_grid_header(
        &self,
        table: &Arc<Table>,
        widths: &Rc<Vec<f32>>,
        columns: &std::ops::Range<usize>,
        total: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(doc) = self.grid_doc() else { return div() };
        let before: f32 = widths[..columns.start].iter().sum();
        let after: f32 = widths[columns.end..].iter().sum();
        let (sort, header, cursor_col) = (doc.grid.sort, doc.grid.names(table), doc.grid.cursor.1);
        let ((_, c0), (_, c1)) = doc.grid.range();
        div()
            .h(px(ROW_HEIGHT + 2.))
            .w(px(total))
            .flex_shrink_0()
            .flex()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .child(
                div()
                    .w(px(NUMBER_WIDTH))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .px_2()
                    .t_caption()
                    .text_color(hex(Chrome::MUTED))
                    .child("#"),
            )
            .child(div().w(px(before)).flex_shrink_0())
            .children(columns.clone().map(|col| {
                let width = widths[col];
                let name = if header { table.value(0, col).to_string() } else { column_name(col) };
                let sorted = sort.filter(|(c, _)| *c == col).map(|(_, a)| a);
                let picked = (c0..=c1).contains(&col);
                div()
                    .id(("table-col", col))
                    .relative()
                    .w(px(width))
                    .h_full()
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_r_1()
                    .border_color(hex_alpha(Chrome::BORDER, 0.7))
                    .cursor_pointer()
                    .when(picked, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.12)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .group("table-head")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.commit_cell(cx);
                            this.grid_menu = None;
                            window.focus(&this.grid_focus);
                            this.grid_select_line(None, Some(col), event.modifiers.shift, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.commit_cell(cx);
                            window.focus(&this.grid_focus);
                            if let Some(doc) = this.grid_doc_mut() {
                                if !(doc.grid.range().0 .1..=doc.grid.range().1 .1).contains(&col) {
                                    doc.grid.cursor = (doc.grid.cursor.0, col);
                                    doc.grid.anchor = doc.grid.cursor;
                                }
                            }
                            this.grid_menu = Some(GridMenu { at: event.position, header: true });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .t_small()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(if col == cursor_col { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                            .child(one_line(&name)),
                    )
                    // Sorted by this column: an arrow that turns the order around, then takes it off.
                    .child(
                        div()
                            .id(("table-sort", col))
                            .flex_shrink_0()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .when(sorted.is_none(), |d| d.invisible().group_hover("table-head", |s| s.visible()))
                            .hover(|s| s.bg(hex(Chrome::SELECTED)))
                            .child(icon(
                                if sorted == Some(false) { "arrow-down" } else { "arrow-up" },
                                11.,
                                hex(if sorted.is_some() { Chrome::ACCENT } else { Chrome::MUTED }),
                            ))
                            .tooltip(Tooltip::text(t(cx, "table.sort"), None))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let next = match sorted {
                                    None => Some(true),
                                    Some(true) => Some(false),
                                    Some(false) => None,
                                };
                                this.grid_sort(col, next, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("table-resize", col))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(-3.))
                            .w(px(6.))
                            .cursor(gpui::CursorStyle::ResizeLeftRight)
                            .hover(crate::ui::hover_fill)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_drag(ColumnDrag, |_, _, _, cx| cx.new(|_| ColumnDrag))
                            .on_drag_move(cx.listener(move |this, event: &gpui::DragMoveEvent<ColumnDrag>, _, cx| {
                                let delta = f32::from(event.event.position.x) - f32::from(event.bounds.origin.x);
                                if let Some(doc) = this.grid_doc_mut() {
                                    let current = doc.grid.widths.get(&col).copied().unwrap_or(width);
                                    doc.grid.widths.insert(col, (current + delta).clamp(COLUMN_MIN, COLUMN_DRAG_MAX));
                                    cx.notify();
                                }
                            })),
                    )
            }))
            .child(div().w(px(after)).flex_shrink_0())
    }

    fn render_grid_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let Some(doc) = self.grid_doc() else { return Vec::new() };
        let table = doc.grid_table();
        let shown = doc.grid_shown(&table);
        let widths = doc.grid_widths(&table);
        let columns = self.grid_columns(&widths, doc);
        let before: f32 = widths[..columns.start].iter().sum();
        let after: f32 = widths[columns.end..].iter().sum();
        let total = NUMBER_WIDTH + widths.iter().sum::<f32>();
        let grid = &doc.grid;
        let (cursor, focused) = (grid.cursor, true);
        let editing = self.cell_edit.as_ref().map(|e| (e.row, e.col, e.input.clone()));
        range
            .map(|view_row| {
                // A row typed into before it exists (an empty sheet's first) shows where it will be.
                let new_row = editing.as_ref().map(|(r, _, _)| *r).filter(|r| *r >= table.rows.len());
                let Some(row) = shown.get(view_row).copied().or(new_row.filter(|_| view_row == shown.len())) else {
                    // The last line adds a row, as a spreadsheet's empty row below the data does.
                    return div()
                        .id(("table-add", view_row))
                        .h(px(ROW_HEIGHT))
                        .w(px(total))
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_3()
                        .t_small()
                        .text_color(hex(Chrome::MUTED))
                        .cursor_pointer()
                        .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                        .child(icon("plus", 12., hex(Chrome::MUTED)))
                        .child(t(cx, "table.add_row"))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            let Some((table, shown)) = this.grid_state() else { return };
                            match this.grid_doc_mut().map(|d| (d.grid.sheet, &mut d.content)) {
                                Some((sheet, Content::Sheets(workbook))) => workbook.add_row(sheet),
                                _ => this.grid_apply(&table, vec![table.insert_row(table.rows.len())], cx),
                            }
                            this.grid_go((shown.len(), 0), false, cx);
                        }))
                        .into_any_element();
                };
                let number = (row + usize::from(!grid.names(&table))).to_string();
                div()
                    .id(("table-row", view_row))
                    .h(px(ROW_HEIGHT))
                    .w(px(total))
                    .flex()
                    .border_b_1()
                    .border_color(hex_alpha(Chrome::BORDER, 0.5))
                    .child(
                        div()
                            .id("n")
                            .w(px(NUMBER_WIDTH))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .px_2()
                            .t_caption()
                            .text_color(hex(if cursor.0 == view_row { Chrome::BRIGHT } else { Chrome::MUTED }))
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    this.commit_cell(cx);
                                    this.grid_menu = None;
                                    window.focus(&this.grid_focus);
                                    this.grid_select_line(Some(view_row), None, event.modifiers.shift, cx);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    this.grid_mouse_down((view_row, 0), event, window, cx);
                                    this.grid_select_line(Some(view_row), None, false, cx);
                                }),
                            )
                            .child(number),
                    )
                    .child(div().w(px(before)).flex_shrink_0())
                    .children(columns.clone().map(|col| {
                        let at = (view_row, col);
                        let value = table.value(row, col);
                        let input = editing.as_ref().filter(|(r, c, _)| *r == row && *c == col).map(|(_, _, input)| input.clone());
                        let is_cursor = focused && at == cursor;
                        div()
                            .id(("c", col))
                            .relative()
                            .w(px(widths[col]))
                            .h_full()
                            .flex_shrink_0()
                            .px_2()
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(hex_alpha(Chrome::BORDER, 0.5))
                            .when(grid.selected(at), |d| d.bg(hex_alpha(Chrome::ACCENT, 0.16)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| this.grid_mouse_down(at, event, window, cx)),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| this.grid_mouse_down(at, event, window, cx)),
                            )
                            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| this.grid_mouse_move(at, event, cx)))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .t_small()
                                    .font_family("monospace")
                                    .text_color(hex(Chrome::FOREGROUND))
                                    .child(one_line(value)),
                            )
                            .when(is_cursor, |d| d.child(div().absolute().inset_0().border_2().border_color(hex(Chrome::ACCENT))))
                            .when_some(input, |d, input| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        // A definite size: the field inside measures its text against it.
                                        .w(px(widths[col]))
                                        .min_h_full()
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .bg(hex(Chrome::EDITOR))
                                        .border_2()
                                        .border_color(hex(Chrome::ACCENT))
                                        .t_small()
                                        .font_family("monospace")
                                        .child(div().flex_1().min_w_0().child(input)),
                                )
                            })
                    }))
                    .child(div().w(px(after)).flex_shrink_0())
                    .into_any_element()
            })
            .collect()
    }

    fn render_sheet_tabs(&self, names: Vec<String>, active: usize, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("table-sheets")
            .h(px(30.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .overflow_x_scroll()
            .border_t_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::PANEL))
            .children(names.into_iter().enumerate().map(|(i, name)| {
                div()
                    .id(("table-sheet", i))
                    .flex_shrink_0()
                    .h(px(22.))
                    .px_2p5()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .t_small()
                    .cursor_pointer()
                    .text_color(hex(if i == active { Chrome::BRIGHT } else { Chrome::MUTED }))
                    .when(i == active, |d| d.bg(hex(Chrome::SELECTED)))
                    .when(i != active, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
                    .child(name)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(doc) = this.grid_doc_mut() {
                            doc.grid.sheet = i;
                            doc.grid.cursor = (0, 0);
                            doc.grid.anchor = (0, 0);
                            doc.grid.sort = None;
                            doc.grid.widths.clear();
                            doc.grid.scroll.scroll_to_item(0, ScrollStrategy::Top);
                            doc.grid.hscroll.set_offset(Point::default());
                        }
                        cx.notify();
                    }))
            }))
    }

    fn render_grid_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.grid_menu.as_ref()?;
        let doc = self.grid_doc()?;
        let editable = doc.editable();
        // Rows and columns move in text; a spreadsheet's formulas and ranges would point astray.
        let reshape = editable && matches!(doc.content, Content::Text);
        let ((r0, c0), (r1, _)) = doc.grid.range();
        let rows = r1 - r0 + 1;
        let col = doc.grid.cursor.1;
        let sorted = doc.grid.sort.filter(|(c, _)| *c == col).map(|(_, a)| a);
        let separator = || div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER));
        let item = |id: &'static str, label: String, f: fn(&mut CodeEditor, &mut Context<CodeEditor>)| {
            crate::ui::menu_item(
                id,
                label,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.grid_menu = None;
                    f(this, cx);
                    cx.notify();
                }),
            )
        };
        let mut list = popover().w(px(220.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.grid_menu = None;
            cx.notify();
        }));
        if !menu.header {
            list = list.child(item("table-copy", t(cx, "table.copy").to_string(), |this, cx| this.grid_copy(cx)));
            if editable {
                list = list.child(item("table-paste", t(cx, "table.paste").to_string(), |this, cx| this.grid_paste(cx))).child(separator());
            }
            if reshape {
                list = list
                    .child(item("table-row-above", t(cx, "table.insert_row_above").to_string(), |this, cx| this.grid_insert_row(false, cx)))
                    .child(item("table-row-below", t(cx, "table.insert_row_below").to_string(), |this, cx| this.grid_insert_row(true, cx)))
                    .child(item(
                        "table-row-delete",
                        if rows > 1 {
                            tf(cx, "table.delete_rows", &[("n", &rows.to_string())])
                        } else {
                            t(cx, "table.delete_row").to_string()
                        },
                        |this, cx| this.grid_delete_rows(cx),
                    ))
                    .child(separator());
            }
        }
        if reshape {
            list = list
                .child(item("table-col-left", t(cx, "table.insert_column_left").to_string(), |this, cx| this.grid_insert_column(false, cx)))
                .child(item("table-col-right", t(cx, "table.insert_column_right").to_string(), |this, cx| {
                    this.grid_insert_column(true, cx)
                }))
                .child(item("table-col-delete", t(cx, "table.delete_column").to_string(), |this, cx| this.grid_delete_column(cx)))
                .child(separator());
        }
        let _ = c0;
        list = list
            .when(sorted != Some(true), |l| {
                l.child(item("table-sort-up", t(cx, "table.sort_ascending").to_string(), |this, cx| {
                    let col = this.grid_doc().map_or(0, |d| d.grid.cursor.1);
                    this.grid_sort(col, Some(true), cx);
                }))
            })
            .when(sorted != Some(false), |l| {
                l.child(item("table-sort-down", t(cx, "table.sort_descending").to_string(), |this, cx| {
                    let col = this.grid_doc().map_or(0, |d| d.grid.cursor.1);
                    this.grid_sort(col, Some(false), cx);
                }))
            })
            .when(doc.grid.sort.is_some(), |l| {
                l.child(item("table-sort-off", t(cx, "table.sort_off").to_string(), |this, cx| this.grid_sort(0, None, cx)))
            });
        Some(gpui::deferred(gpui::anchored().position(menu.at).snap_to_window_with_margin(px(8.)).child(list)).with_priority(2))
    }

    /// The status bar's items while the grid shows: the cell, the table's size, the first row as
    /// names and the delimiter.
    pub(super) fn render_grid_status(&self, doc: &Document, cx: &mut Context<Self>) -> gpui::Div {
        let table = doc.grid_table();
        let shown = doc.grid_shown(&table);
        let grid = &doc.grid;
        let ((r0, c0), (r1, c1)) = grid.range();
        let names = grid.names(&table);
        let row_number = |view_row: usize| shown.get(view_row).map_or(view_row + 1, |r| r + usize::from(!names));
        let cell = |at: At| format!("{}{}", column_name(at.1), row_number(at.0));
        let place = if (r0, c0) == (r1, c1) { cell((r0, c0)) } else { format!("{}:{}", cell((r0, c0)), cell((r1, c1))) };
        let data_rows = table.rows.len() - usize::from(names);
        let item = |text: String| div().flex_shrink_0().child(text);
        let chip = |id: &'static str, text: String| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                .child(text)
        };
        let delimiter = grid.delimiter.unwrap_or_else(|| doc.detected_delimiter());
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_4()
            .child(item(place))
            .child(item(tf(cx, "table.size", &[("rows", &data_rows.to_string()), ("columns", &table.width.to_string())])))
            .child(div().flex_1())
            .child(
                chip("table-header", t(cx, "table.header").to_string())
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(icon(if grid.header { "check" } else { "square" }, 10., hex(Chrome::MUTED)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Some(doc) = this.grid_doc_mut() {
                            doc.grid.header = !doc.grid.header;
                            doc.grid.cursor = (0, 0);
                            doc.grid.anchor = (0, 0);
                            doc.grid.sort = None;
                        }
                        cx.notify();
                    })),
            )
            .when(matches!(doc.content, Content::Text), |d| {
                d.child(chip("table-delimiter", tf(cx, "table.delimiter", &[("name", t(cx, delimiter_name(delimiter)))])).on_click(
                    cx.listener(|this, event: &ClickEvent, _, cx| {
                        this.delimiter_menu = Some(event.position());
                        cx.notify();
                    }),
                ))
            })
            .children(self.render_delimiter_menu(doc, cx))
    }

    fn render_delimiter_menu(&self, doc: &Document, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let at = self.delimiter_menu?;
        let picked = doc.grid.delimiter;
        let detected = doc.detected_delimiter();
        let auto = tf(cx, "table.delimiter.auto", &[("name", t(cx, delimiter_name(detected)))]);
        let mut list = popover().w(px(200.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.delimiter_menu = None;
            cx.notify();
        }));
        let choices =
            std::iter::once((None, auto)).chain(table::DELIMITERS.iter().map(|d| (Some(*d), t(cx, delimiter_name(*d)).to_string())));
        for (i, (choice, label)) in choices.enumerate() {
            list = list.child(
                crate::ui::menu_item(
                    ("table-delimiter-choice", i),
                    label,
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.delimiter_menu = None;
                        if let Some(doc) = this.grid_doc_mut() {
                            doc.grid.delimiter = choice;
                            doc.grid.widths.clear();
                            doc.grid.cursor = (0, 0);
                            doc.grid.anchor = (0, 0);
                        }
                        cx.notify();
                    }),
                )
                .when(choice == picked, |d| d.text_color(hex(Chrome::BRIGHT)).bg(hex(Chrome::SELECTED))),
            );
        }
        Some(
            gpui::deferred(gpui::anchored().position(at).anchor(gpui::Corner::BottomLeft).snap_to_window_with_margin(px(8.)).child(list))
                .with_priority(2),
        )
    }
}

/// A column's edge being dragged.
struct ColumnDrag;

impl Render for ColumnDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_names_count_like_a_spreadsheet() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(27), "AB");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
    }

    #[test]
    fn an_empty_first_row_is_data_not_names() {
        let grid = Grid::new(true);
        assert!(!grid.names(&Table::from_values(vec![vec![String::new(), " ".into()], vec!["x".into()]])));
        assert!(!grid.names(&Table::from_values(Vec::new())));
        assert!(grid.names(&Table::from_values(vec![vec![String::new(), "name".into()]])));
        let mut plain = Grid::new(true);
        plain.header = false;
        assert!(!plain.names(&Table::from_values(vec![vec!["name".into()]])));
    }

    #[test]
    fn wide_characters_count_twice() {
        assert_eq!(text_width("abc"), 3);
        assert_eq!(text_width("접속 안 됨"), 10);
        assert_eq!(text_width("first\nsecond line"), 5);
    }
}
