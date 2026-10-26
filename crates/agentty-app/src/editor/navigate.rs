//! Finding your way around the code without another IDE: go to a file (⌘P), to a symbol of the
//! file (`@` in that box, ⇧⌘O) or of the project (`#`, ⌥⌘O), to a line (`:`, ⌃G), to text in any
//! file of the project (`%`, ⇧⌘F), to where the
//! name under the cursor is defined (F12, ⌘-click — an import's path opens that file) and to
//! every place it is used (⇧F12, ⌥F7), with back and forward through the places visited (⌃-, ⌃⇧-).
//!
//! Definitions and usages come from `project_index` (names, not types): when a name is defined in
//! several places the list shows them all, best guess first.

use super::buffer::Pos;
use super::language::Language;
use super::lsp;
use super::project_index::{self, Location, Usage};
use super::symbols;
use super::{CodeEditor, Content, EditorEvent};
use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use gpui::{
    actions, div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Focusable, ScrollHandle, SharedString, Subscription, Window,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

actions!(
    code_editor,
    [
        GoToFile,
        GoToSymbol,
        GoToFileSymbol,
        GoToLine,
        FindInFiles,
        GoToDefinition,
        GoToImplementation,
        FindUsages,
        NavigateBack,
        NavigateForward,
        PickerNext,
        PickerPrevious
    ]
);

/// The picker's own key context (outside the editor's, like the find bar).
const PICKER: &str = "EditorPicker";
/// Rows a list shows at most.
const MAX_ROWS: usize = 200;
/// Places remembered for going back.
const MAX_HISTORY: usize = 100;
/// Places a text search lists at most.
const MAX_TEXT_MATCHES: usize = 1_000;
/// A text search waits this long for the next key before reading the project.
const TEXT_PAUSE: std::time::Duration = std::time::Duration::from_millis(120);
/// How long a short note under the cursor stays.
const FLASH: std::time::Duration = std::time::Duration::from_millis(2600);

pub fn bind_keys(cx: &mut gpui::App) {
    use crate::key;
    let c = Some(super::CONTEXT);
    cx.bind_keys([
        key("cmd-p", GoToFile, c),
        key("cmd-shift-o", GoToFileSymbol, c),
        key("cmd-alt-o", GoToSymbol, c),
        key("ctrl-g", GoToLine, c),
        // VS Code's and IntelliJ's: in the editor it wins over the app's Flow page.
        key("cmd-shift-f", FindInFiles, c),
        // Find in the file. The app's ⌘F becomes Ctrl+Shift+F off the Mac (Ctrl+F stays with the
        // terminals), which the line above takes in the editor: an editor takes Ctrl+F itself.
        key("cmd-f", crate::workbench::FindInTerminal, c),
        key("f12", GoToDefinition, c),
        key("cmd-f12", GoToImplementation, c),
        key("shift-f12", FindUsages, c),
        key("alt-f7", FindUsages, c),
        key("ctrl--", NavigateBack, c),
        key("ctrl-shift--", NavigateForward, c),
        key("cmd-alt-left", NavigateBack, c),
        key("cmd-alt-right", NavigateForward, c),
        key("ctrl-n", PickerNext, Some(PICKER)),
        key("ctrl-p", PickerPrevious, Some(PICKER)),
    ]);
}

/// What a row leads to.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A place in a file (opened if need be); `len` bytes from there are selected.
    At { path: PathBuf, pos: Pos, len: usize },
    /// A line of the file in front.
    Line(usize),
}

#[derive(Clone, Debug)]
pub struct Row {
    /// Small tag in front (`class`, `fn`, …).
    pub tag: Option<&'static str>,
    pub label: SharedString,
    /// Where it is (path and line), right of the label.
    pub detail: SharedString,
    pub target: Target,
    /// What the query matched in the label (bytes), marked.
    pub mark: Option<(usize, usize)>,
}

/// Lists filled from the query as it is typed, by its first character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// No prefix: files by name.
    Files,
    /// `@`: symbols of the file in front.
    FileSymbols,
    /// `#`: symbols of the whole project.
    Symbols,
    /// `:`: a line number.
    Line,
    /// `%`: text in the project's files.
    Text,
    /// A list the editor found (definitions of a name, its usages); the query filters it.
    Found,
}

impl Mode {
    fn of(query: &str) -> (Self, &str) {
        match query.chars().next() {
            Some('@') => (Self::FileSymbols, &query[1..]),
            Some('#') => (Self::Symbols, &query[1..]),
            Some(':') => (Self::Line, &query[1..]),
            Some('%') => (Self::Text, &query[1..]),
            _ => (Self::Files, query),
        }
    }
}

pub struct Picker {
    input: Entity<TextInput>,
    project: PathBuf,
    /// Set for a found list: its title and rows (filtered by the query).
    found: Option<(String, Vec<Row>)>,
    rows: Vec<Row>,
    selected: usize,
    /// The project's files, once listed.
    files: Option<Arc<Vec<String>>>,
    /// Counts symbol and text searches, so only the latest one's answer is shown.
    generation: u64,
    loading: bool,
    /// Set to call off the text search that runs.
    cancel: Option<Arc<AtomicBool>>,
    /// What the last text search found: places, files, and whether it stopped at the most.
    found_text: Option<(usize, usize, bool)>,
    scroll: ScrollHandle,
    /// The code around the selected row's place: (its file and line, the lines from `first`).
    preview: Option<Preview>,
    _subscription: Subscription,
}

impl Picker {
    fn call_off_search(&self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Lines of a file around a place, shown beside a list of places.
#[derive(Clone)]
pub struct Preview {
    path: PathBuf,
    line: usize,
    first: usize,
    lines: Vec<String>,
    /// The language server's word on the name (its type or signature), shown above the code.
    note: Option<String>,
}

/// Lines shown above and below the place in a preview.
const PREVIEW_ABOVE: usize = 4;
const PREVIEW_BELOW: usize = 10;

fn preview_of(path: &Path, line: usize, all: impl Iterator<Item = String>) -> Preview {
    let first = line.saturating_sub(PREVIEW_ABOVE);
    let lines = all.skip(first).take(PREVIEW_ABOVE + PREVIEW_BELOW + 1).map(|l| l.chars().take(200).collect()).collect();
    Preview { path: path.to_path_buf(), line, first, lines, note: None }
}

/// Places visited, for back and forward.
#[derive(Default)]
pub struct History {
    back: Vec<(PathBuf, Pos)>,
    forward: Vec<(PathBuf, Pos)>,
}

impl CodeEditor {
    pub(super) fn project_root(&self) -> Option<PathBuf> {
        self.docs
            .get(self.active)
            .map(|d| d.project.clone())
            .or_else(|| self.picker.as_ref().map(|p| p.project.clone()))
            .or_else(|| self.picked_project.clone())
    }

    /// Opens the go-to box with `prefix` typed (`""` files, `@` file symbols, `#` project symbols,
    /// `:` line). `project` is for an editor with nothing open yet.
    pub fn open_picker(&mut self, prefix: &str, project: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = project.or_else(|| self.project_root()) else { return };
        self.find = None;
        let input = cx.new(|cx| TextInput::localized(prefix.to_string(), "nav.placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Changed => this.picker_query_changed(cx),
            TextInputEvent::Confirmed => this.picker_confirm(None, window, cx),
            TextInputEvent::Down => this.picker_step(1, cx),
            TextInputEvent::Up => this.picker_step(-1, cx),
            TextInputEvent::Cancelled => this.close_picker(window, cx),
            _ => {}
        });
        input.update(cx, |input, cx| {
            input.set_text(prefix.to_string(), cx);
            window.focus(&input.focus_handle(cx));
        });
        self.picker = Some(Picker {
            input,
            project,
            found: None,
            rows: Vec::new(),
            selected: 0,
            files: None,
            generation: 0,
            loading: false,
            cancel: None,
            found_text: None,
            scroll: ScrollHandle::new(),
            preview: None,
            _subscription: subscription,
        });
        self.picker_query_changed(cx);
        cx.notify();
    }

    /// ⇧⌘F: the go-to box searching the project's text, for the selected text or else the name
    /// at the cursor.
    pub fn find_in_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text)).and_then(|doc| {
            let selected = doc.buffer.selected_text();
            if !selected.is_empty() {
                return (!selected.contains('\n')).then_some(selected);
            }
            let cursor = doc.buffer.cursor();
            let line = doc.buffer.line(cursor.line);
            symbols::name_at(line, cursor.col, doc.language).filter(|(s, e)| s < e).map(|(s, e)| line[s..e].to_string())
        });
        self.open_picker(&format!("%{}", seed.unwrap_or_default()), None, window, cx);
        // Typing replaces the seed, and stays a text search.
        if let Some(picker) = self.picker.as_ref() {
            picker.input.update(cx, |input, cx| input.select_from(1, cx));
        }
    }

    /// Shows a list the editor found (definitions, usages) to pick from.
    pub(super) fn open_found(&mut self, title: String, rows: Vec<Row>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker("", None, window, cx);
        if let Some(picker) = self.picker.as_mut() {
            picker.rows = rows.clone();
            picker.found = Some((title, rows));
            let placeholder = t(cx, "nav.filter");
            picker.input.update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        }
        cx.notify();
    }

    /// Debug driver: picks row `index` of the go-to box.
    pub(super) fn debug_pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_confirm(Some(index), window, cx);
    }

    /// Debug driver: the rows of the go-to box.
    pub(super) fn debug_picker(&self) -> serde_json::Value {
        match self.picker.as_ref() {
            Some(picker) => serde_json::json!({
                "loading": picker.loading,
                "title": picker.found.as_ref().map(|(t, _)| t.clone()),
                "rows": picker.rows.iter().take(30).map(|r| format!("{} {} {}", r.tag.unwrap_or(""), r.label, r.detail)).collect::<Vec<_>>(),
                "preview": picker.preview.as_ref().map(|p| (p.line, p.lines.get(p.line - p.first).cloned())),
            }),
            None => serde_json::Value::Null,
        }
    }

    pub fn picker_open(&self) -> bool {
        self.picker.is_some()
    }

    pub fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.take() {
            picker.call_off_search();
            window.focus(&self.focus_handle);
            if self.docs.is_empty() {
                cx.emit(EditorEvent::Empty);
            }
            cx.notify();
        }
    }

    fn picker_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_mut() else { return };
        if picker.rows.is_empty() {
            return;
        }
        let count = picker.rows.len() as isize;
        picker.selected = ((picker.selected as isize + by).rem_euclid(count)) as usize;
        picker.scroll.scroll_to_item(picker.selected);
        cx.notify();
    }

    fn picker_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_mut() else { return };
        let query = picker.input.read(cx).text().to_string();
        picker.selected = 0;
        picker.scroll.scroll_to_item(0);
        picker.call_off_search();
        picker.found_text = None;
        if let Some((_, rows)) = &picker.found {
            picker.loading = false;
            let query = query.trim();
            picker.rows = rows
                .iter()
                .filter(|row| query.is_empty() || symbols::fuzzy_score(&format!("{} {}", row.detail, row.label), query).is_some())
                .cloned()
                .collect();
            return cx.notify();
        }
        let (mode, rest) = Mode::of(&query);
        let rest = rest.trim().to_string();
        match mode {
            Mode::Files => match picker.files.clone() {
                Some(files) => {
                    let recent: Vec<String> = self
                        .recent
                        .iter()
                        .filter(|p| Some(p.as_path()) != self.docs.get(self.active).map(|d| d.path.as_path()))
                        .filter_map(|p| p.strip_prefix(&picker.project).ok().map(|r| r.to_string_lossy().replace('\\', "/")))
                        .collect();
                    picker.rows = file_rows(&picker.project, &files, &rest, &recent);
                    picker.loading = false;
                }
                None => {
                    picker.loading = true;
                    let project = picker.project.clone();
                    cx.spawn(async move |this, cx| {
                        let root = project.clone();
                        let files = cx.background_spawn(async move { project_index::files(&root) }).await;
                        let _ = this.update(cx, |this, cx| {
                            if let Some(picker) = this.picker.as_mut().filter(|p| p.project == project) {
                                picker.files = Some(files);
                                this.picker_query_changed(cx);
                            }
                        });
                    })
                    .detach();
                }
            },
            Mode::FileSymbols => {
                let rows = self.file_symbol_rows(&rest);
                if let Some(picker) = self.picker.as_mut() {
                    picker.rows = rows;
                }
            }
            Mode::Line => {
                let row = rest.split([':', ',']).next().and_then(|n| n.trim().parse::<usize>().ok()).filter(|&n| n > 0);
                let lines = self.docs.get(self.active).map_or(0, |d| d.buffer.line_count());
                picker.rows = row
                    .map(|n| {
                        let n = n.min(lines.max(1));
                        vec![Row {
                            tag: None,
                            label: tf(cx, "nav.go_to_line", &[("line", &n.to_string())]).into(),
                            detail: SharedString::default(),
                            target: Target::Line(n - 1),
                            mark: None,
                        }]
                    })
                    .unwrap_or_default();
            }
            Mode::Symbols => {
                picker.generation += 1;
                let generation = picker.generation;
                if rest.is_empty() {
                    picker.rows.clear();
                    picker.loading = false;
                    return cx.notify();
                }
                picker.loading = true;
                let project = picker.project.clone();
                cx.spawn(async move |this, cx| {
                    let root = project.clone();
                    let found = cx.background_spawn(async move { project_index::search_symbols(&root, &rest, MAX_ROWS) }).await;
                    let _ = this.update(cx, |this, cx| {
                        let Some(picker) = this.picker.as_mut().filter(|p| p.generation == generation) else { return };
                        picker.rows = found.iter().map(location_row).collect();
                        picker.loading = false;
                        cx.notify();
                    });
                })
                .detach();
            }
            Mode::Text => {
                picker.generation += 1;
                let generation = picker.generation;
                if rest.chars().count() < 2 {
                    picker.rows.clear();
                    picker.loading = false;
                    return cx.notify();
                }
                picker.loading = true;
                let cancel = Arc::new(AtomicBool::new(false));
                picker.cancel = Some(cancel.clone());
                let project = picker.project.clone();
                // Unsaved text is what the user sees: it is searched instead of the file.
                let open: Vec<(PathBuf, String)> = self
                    .docs
                    .iter()
                    .filter(|d| d.is_dirty() && matches!(d.content, Content::Text) && d.path.starts_with(&project))
                    .map(|d| (d.path.clone(), d.buffer.text()))
                    .collect();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(TEXT_PAUSE).await;
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let found = cx
                        .background_spawn(async move { project_index::search_text(&project, &rest, &open, MAX_TEXT_MATCHES, &cancel) })
                        .await;
                    let Some(found) = found else { return };
                    let _ = this.update(cx, |this, cx| {
                        let Some(picker) = this.picker.as_mut().filter(|p| p.generation == generation) else { return };
                        let mut files: Vec<&Path> = found.iter().map(|m| m.usage.path.as_path()).collect();
                        files.dedup();
                        picker.found_text = Some((found.len(), files.len(), found.len() >= MAX_TEXT_MATCHES));
                        picker.rows = found.iter().take(MAX_ROWS).map(text_row).collect();
                        picker.loading = false;
                        cx.notify();
                    });
                })
                .detach();
            }
            Mode::Found => {}
        }
        cx.notify();
    }

    fn file_symbol_rows(&self, query: &str) -> Vec<Row> {
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text)) else { return Vec::new() };
        let text = doc.buffer.text();
        let mut found: Vec<(i64, symbols::Symbol)> = symbols::symbols(doc.language, &text)
            .into_iter()
            .filter_map(|s| symbols::fuzzy_score(&s.name, query).map(|score| (score, s)))
            .collect();
        // In file order while nothing is typed: an outline.
        if !query.is_empty() {
            found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.line.cmp(&b.1.line)));
        }
        found
            .into_iter()
            .take(MAX_ROWS)
            .map(|(_, s)| Row {
                tag: Some(s.kind.tag()),
                label: s.name.clone().into(),
                detail: format!(":{}", s.line + 1).into(),
                target: Target::At { path: doc.path.clone(), pos: Pos::new(s.line, s.col), len: s.name.len() },
                mark: None,
            })
            .collect()
    }

    fn picker_confirm(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_ref() else { return };
        let Some(row) = picker.rows.get(index.unwrap_or(picker.selected)).cloned() else { return };
        picker.call_off_search();
        self.picked_project = Some(picker.project.clone());
        self.picker = None;
        window.focus(&self.focus_handle);
        match row.target {
            Target::At { path, pos, len } => self.go_to(&path, pos, len, window, cx),
            Target::Line(line) => {
                if let Some(path) = self.docs.get(self.active).map(|d| d.path.clone()) {
                    self.go_to(&path, Pos::new(line, 0), 0, window, cx);
                }
            }
        }
        cx.notify();
    }

    // -- going places -------------------------------------------------------------------------

    fn here(&self) -> Option<(PathBuf, Pos)> {
        let doc = self.docs.get(self.active)?;
        Some((doc.path.clone(), doc.buffer.cursor()))
    }

    /// Shows `path` with the cursor at `pos` (and `len` bytes from there selected), remembering
    /// where the cursor was for going back.
    pub fn go_to(&mut self, path: &Path, pos: Pos, len: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(here) = self.here() {
            if here != (path.to_path_buf(), pos) {
                self.history.back.push(here);
                if self.history.back.len() > MAX_HISTORY {
                    self.history.back.remove(0);
                }
                self.history.forward.clear();
            }
        }
        self.show_place(path, pos, len, window, cx);
    }

    fn show_place(&mut self, path: &Path, pos: Pos, len: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.docs.iter().position(|d| d.path == path) {
            Some(index) => {
                if index != self.active {
                    self.activate(index, window, cx);
                }
                self.place_cursor(pos, len, cx);
                window.focus(&self.focus_handle);
            }
            None => {
                let project = self.project_root().unwrap_or_default();
                self.pending_goto = Some((path.to_path_buf(), pos, len, None));
                self.pending_column = None;
                cx.emit(EditorEvent::OpenFile(path.to_path_buf(), project));
            }
        }
    }

    fn place_cursor(&mut self, pos: Pos, len: usize, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        doc.buffer.unmark();
        let start = doc.buffer.clamp(pos);
        doc.buffer.move_to(start, false);
        if len > 0 {
            let end = doc.buffer.clamp(Pos::new(start.line, start.col + len));
            doc.buffer.move_to(end, true);
        }
        doc.center = true;
        doc.autoscroll = true;
        cx.notify();
    }

    /// A file opened for `go_to` is there now: its place is shown.
    pub(super) fn apply_pending_goto(&mut self, cx: &mut Context<Self>) {
        let Some((path, pos, len, anchor)) = self.pending_goto.take() else { return };
        let column = self.pending_column.take();
        let Some(doc) = self.docs.get(self.active).filter(|d| d.path == path) else { return };
        // A place known by a line of its text (an agent's edit in a file that was not open).
        let pos = anchor.and_then(|anchor| super::agent::find_anchor(&doc.buffer, &anchor)).unwrap_or(pos);
        // A column counted in characters (a terminal's `file:line:column`): its byte offset.
        let pos = match column {
            Some(chars) if pos.line < doc.buffer.line_count() => {
                let line = doc.buffer.line(pos.line);
                Pos::new(pos.line, line.char_indices().nth(chars).map_or(line.len(), |(i, _)| i))
            }
            _ => pos,
        };
        self.place_cursor(pos, len, cx);
    }

    /// Shows a file at the line holding `anchor` (or at `pos` when it is not found).
    pub(super) fn go_to_anchor(&mut self, path: &Path, pos: Pos, anchor: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.docs.iter().any(|d| d.path == path) {
            return self.go_to(path, pos, 0, window, cx);
        }
        self.go_to(path, pos, 0, window, cx);
        if let Some(pending) = self.pending_goto.as_mut() {
            pending.3 = anchor;
        }
    }

    pub fn navigate(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.here();
        let (from, to) =
            if back { (&mut self.history.back, &mut self.history.forward) } else { (&mut self.history.forward, &mut self.history.back) };
        let Some((path, pos)) = from.pop() else { return };
        to.extend(here);
        // A file closed since opens again.
        self.show_place(&path, pos, 0, window, cx);
    }

    /// The name under the cursor (or the one-line selection) of the file in front.
    fn name_here(&self) -> Option<(String, Language, Pos)> {
        let doc = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text))?;
        let selected = doc.buffer.selected_text();
        if !selected.is_empty() && !selected.contains('\n') && selected.trim() == selected {
            return Some((selected, doc.language, doc.buffer.selection.range().0));
        }
        let cursor = doc.buffer.cursor();
        let line = doc.buffer.line(cursor.line);
        let (start, end) = symbols::name_at(line, cursor.col, doc.language)?;
        Some((line[start..end].to_string(), doc.language, Pos::new(cursor.line, start)))
    }

    /// F12: where the name under the cursor is defined. On a definition itself, its usages instead
    /// (as IntelliJ does); on an import's path, that file. The language server answers when one
    /// runs for the file; the project's name index otherwise, or when it found nothing.
    pub fn go_to_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text)) else { return };
        let cursor = doc.buffer.cursor();
        let line = doc.buffer.line(cursor.line).to_string();
        let dir = doc.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let (project, language, path) = (doc.project.clone(), doc.language, doc.path.clone());
        if let Some(file) = symbols::linked_file(&line, cursor.col, language, &dir, &project) {
            return self.go_to(&file, Pos::default(), 0, window, cx);
        }
        if !symbols::supported(language) {
            return;
        }
        let Some((name, _, at)) = self.name_here() else { return };
        // An `impl` names a type, it does not define it.
        let local: Vec<symbols::Symbol> = symbols::symbols(language, &self.docs[self.active].buffer.text())
            .into_iter()
            .filter(|s| s.kind != symbols::Kind::Impl)
            .collect();
        if local.iter().any(|s| s.name == name && s.line == at.line && s.col == at.col) {
            return self.find_usages(window, cx);
        }
        let rel = relative(&project, &path);
        let handle = window.window_handle();
        let wanted = name.clone();
        let text = lsp::Snapshot::of(&self.docs[self.active].buffer);
        cx.spawn(async move |this, cx| {
            let (root, file) = (project.clone(), path.clone());
            let (exact, found) = cx
                .background_spawn(async move {
                    match lsp::definitions(&root, language, &file, &text, at) {
                        Some(found) => (true, found),
                        None => (false, project_index::definitions(&root, &wanted, &rel)),
                    }
                })
                .await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if exact {
                        // The server points back at the name itself: it is the definition.
                        if let [only] = found.as_slice() {
                            if only.path == path && only.line == at.line && only.col == at.col {
                                return this.find_usages(window, cx);
                            }
                        }
                        return this.show_definitions(&name, found, window, cx);
                    }
                    // This file as it is now (unsaved edits included) rather than as on disk.
                    let mut found: Vec<Location> = found.into_iter().filter(|l| l.path != path).collect();
                    let here = local.iter().filter(|s| s.name == name).map(|s| Location {
                        path: path.clone(),
                        rel: relative(&project, &path),
                        line: s.line,
                        col: s.col,
                        name: s.name.clone(),
                        kind: s.kind,
                    });
                    found.splice(0..0, here);
                    this.show_definitions(&name, found, window, cx);
                });
            });
        })
        .detach();
    }

    /// ⌘F12: where the name under the cursor is implemented (the classes implementing an
    /// interface, the methods overriding one). The language server knows; without one, nothing.
    pub fn go_to_implementation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name, language, at)) = self.name_here() else { return };
        let Some(doc) = self.docs.get(self.active) else { return };
        if lsp::id(language).is_none() || lsp::status(&doc.project, language, &doc.path) != "ready" {
            return self.flash(t(cx, "nav.implementation_needs_server").to_string(), cx);
        }
        let (project, path, text) = (doc.project.clone(), doc.path.clone(), lsp::Snapshot::of(&doc.buffer));
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { lsp::implementations(&project, language, &path, &text, at) }).await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match found {
                    Some(found) if found.len() > 1 => {
                        let title = tf(cx, "nav.implementations", &[("name", &name), ("n", &found.len().to_string())]);
                        let rows = found.iter().map(location_row).collect();
                        this.open_found(title, rows, window, cx);
                    }
                    Some(found) => this.show_definitions(&name, found, window, cx),
                    None => this.flash(tf(cx, "nav.no_implementation", &[("name", &name)]), cx),
                });
            });
        })
        .detach();
    }

    fn show_definitions(&mut self, name: &str, found: Vec<Location>, window: &mut Window, cx: &mut Context<Self>) {
        match found.as_slice() {
            [] => self.flash(tf(cx, "nav.no_definition", &[("name", name)]), cx),
            [only] => {
                let (path, pos, len) = (only.path.clone(), Pos::new(only.line, only.col), only.name.len());
                self.go_to(&path, pos, len, window, cx);
            }
            many => {
                let title = tf(cx, "nav.definitions", &[("name", name), ("n", &many.len().to_string())]);
                let rows = many.iter().map(location_row).collect();
                self.open_found(title, rows, window, cx);
            }
        }
    }

    /// ⇧F12: every place the name under the cursor is used in the project (the language server's
    /// answer when one runs for the file, the name index's otherwise).
    pub fn find_usages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name, language, at)) = self.name_here() else { return };
        let Some(project) = self.project_root() else { return };
        if !symbols::supported(language) {
            return;
        }
        let Some(doc) = self.docs.get(self.active) else { return };
        let (path, text) = (doc.path.clone(), lsp::Snapshot::of(&doc.buffer));
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let (root, wanted) = (project.clone(), name.clone());
            let found = cx
                .background_spawn(async move {
                    lsp::usages(&root, language, &path, &text, at).unwrap_or_else(|| project_index::usages(&root, &wanted, language))
                })
                .await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if found.is_empty() {
                        return this.flash(tf(cx, "nav.no_usages", &[("name", &name)]), cx);
                    }
                    let more = if found.len() >= project_index::MAX_USAGES { "+" } else { "" };
                    let title = tf(cx, "nav.usages", &[("name", &name), ("n", &format!("{}{more}", found.len()))]);
                    let rows = found.iter().map(|u| usage_row(u, name.len())).collect();
                    this.open_found(title, rows, window, cx);
                });
            });
        })
        .detach();
    }

    /// A short note under the cursor that goes away by itself (nothing to close).
    pub(super) fn flash(&mut self, text: String, cx: &mut Context<Self>) {
        self.flash = Some((text, std::time::Instant::now()));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLASH).await;
            let _ = this.update(cx, |this, cx| {
                if this.flash.as_ref().is_some_and(|(_, at)| at.elapsed() >= FLASH) {
                    this.flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_flash(&self) -> Option<AnyElement> {
        let (text, _) = self.flash.as_ref()?;
        let doc = self.docs.get(self.active)?;
        let at = self.layout.as_ref()?.point_below(doc.buffer.cursor())?;
        let note = div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_md()
            .t_small()
            .text_color(hex(Chrome::FOREGROUND))
            .child(text.clone());
        Some(
            gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(self.popup_margin()).child(note))
                .with_priority(2)
                .into_any_element(),
        )
    }

    /// Where the cursor is in the file's structure (`Store › load`), each part a way back to its
    /// definition.
    pub(super) fn render_breadcrumb(&self, doc: &super::Document, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !matches!(doc.content, Content::Text) || doc.previewing() {
            return Vec::new();
        }
        let outline = doc.outline();
        let cursor = doc.buffer.cursor().line;
        let chain = symbols::enclosing(&outline, |i| doc.buffer.line(i), cursor);
        let skip = chain.len().saturating_sub(3);
        chain
            .into_iter()
            .skip(skip)
            .enumerate()
            .map(|(index, symbol)| {
                let (pos, len) = (Pos::new(symbol.line, symbol.col), symbol.name.len());
                div()
                    .id(("editor-crumb", index))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(hex(Chrome::MUTED))
                    .child("›")
                    .child(
                        div()
                            .max_w(px(180.))
                            .truncate()
                            .cursor_pointer()
                            .text_color(hex(Chrome::FOREGROUND))
                            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                            .child(symbol.name.clone()),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if let Some(path) = this.docs.get(this.active).map(|d| d.path.clone()) {
                            this.go_to(&path, pos, len, window, cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// ⌘ held over a name: underlined as a link while the pointer is on it.
    pub(super) fn hover_link(&mut self, pos: Option<Pos>, cx: &mut Context<Self>) {
        let link = pos.and_then(|pos| {
            let doc = self.docs.get(self.active)?;
            if !symbols::supported(doc.language) {
                return None;
            }
            let line = doc.buffer.line(pos.line);
            let (start, end) = symbols::name_at(line, pos.col, doc.language)
                .or_else(|| quoted_range(line, pos.col).filter(|(s, e)| line[*s..*e].contains(['/', '.'])))?;
            Some((Pos::new(pos.line, start), Pos::new(pos.line, end)))
        });
        if link != self.link {
            self.link = link;
            self.peek_link(cx);
            cx.notify();
        }
    }

    /// ⌘ over a name: where it is defined shows in a small window under it (as VS Code does), so
    /// a glance answers what a click would.
    fn peek_link(&mut self, cx: &mut Context<Self>) {
        let Some((start, end)) = self.link else {
            self.link_peek = None;
            return;
        };
        let Some(doc) = self.docs.get(self.active) else { return };
        let name = doc.buffer.line(start.line)[start.col..end.col].to_string();
        if self.link_peek.as_ref().is_some_and(|(n, _)| *n == name) {
            return;
        }
        self.link_peek = Some((name.clone(), None));
        if !name.chars().any(|c| c.is_alphabetic()) || symbols::is_keyword(&name) {
            return;
        }
        let (project, path, language) = (doc.project.clone(), doc.path.clone(), doc.language);
        let local = doc.outline().iter().find(|s| s.name == name && s.line != start.line).map(|s| s.line);
        let text = lsp::Snapshot::of(&doc.buffer);
        cx.spawn(async move |this, cx| {
            let wanted = name.clone();
            let found = cx
                .background_spawn(async move {
                    // The language server's answer, with its type or signature, when one runs.
                    if let Some(place) = lsp::definitions(&project, language, &path, &text, start).and_then(|f| f.into_iter().next()) {
                        let source = if place.path == path { Some(text.text.clone()) } else { std::fs::read_to_string(&place.path).ok() };
                        if let Some(source) = source {
                            let mut preview = preview_of(&place.path, place.line, source.lines().map(str::to_string));
                            preview.note = lsp::hover(&project, language, &path, &text, start);
                            return Some(preview);
                        }
                    }
                    // This file first (as it is now), then the project's best guess.
                    if let Some(line) = local {
                        return Some(preview_of(&path, line, text.lines().map(str::to_string)));
                    }
                    let rel = relative(&project, &path);
                    let place = project_index::definitions(&project, &wanted, &rel).into_iter().next()?;
                    let text = std::fs::read_to_string(&place.path).ok()?;
                    Some(preview_of(&place.path, place.line, text.lines().map(str::to_string)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(peek) = this.link_peek.as_mut().filter(|(n, _)| *n == name) {
                    peek.1 = found;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn debug_peek(&self) -> serde_json::Value {
        match &self.link_peek {
            Some((name, Some(p))) => serde_json::json!({ "name": name, "path": p.path, "line": p.line, "note": p.note }),
            Some((name, None)) => serde_json::json!({ "name": name }),
            None => serde_json::Value::Null,
        }
    }

    /// The window under a ⌘-hovered name, with the code where it is defined.
    pub(super) fn render_link_peek(&self, cx: &gpui::App) -> Option<AnyElement> {
        if self.picker.is_some() || self.confirm.is_some() || self.preview.covered {
            return None;
        }
        let (start, _) = self.link?;
        let (_, Some(preview)) = self.link_peek.as_ref()? else { return None };
        let at = self.layout.as_ref()?.point_below(start)?;
        let project = self.docs.get(self.active).map(|d| d.project.clone()).unwrap_or_default();
        let where_ = format!("{}:{}", relative(&project, &preview.path), preview.line + 1);
        let lines = preview.lines.iter().enumerate().skip((preview.line - preview.first).saturating_sub(1)).take(9);
        let panel = div()
            .id("editor-link-peek")
            .w(px(560.))
            .rounded_md()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .occlude()
            .child(
                div().px_2().py_1().border_b_1().border_color(hex(Chrome::BORDER)).t_caption().text_color(hex(Chrome::MUTED)).child(where_),
            )
            .when_some(preview.note.as_ref().map(|n| note_lines(n)), |d, note| {
                d.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(hex(Chrome::BORDER))
                        .font_family(crate::settings::terminal_font(cx))
                        .text_size(px(12.))
                        .text_color(hex(Chrome::FOREGROUND))
                        .children(note.into_iter().map(|line| div().truncate().child(line))),
                )
            })
            .child(div().py_1().font_family(crate::settings::terminal_font(cx)).text_size(px(12.)).children(lines.map(|(i, text)| {
                let number = preview.first + i;
                div()
                    .flex()
                    .gap_2()
                    .px_2()
                    .when(number == preview.line, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.22)))
                    .child(div().w(px(30.)).flex_shrink_0().text_color(hex(Chrome::MUTED)).child((number + 1).to_string()))
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(text.clone()))
            })));
        Some(
            gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(self.popup_margin()).child(panel))
                .with_priority(2)
                .into_any_element(),
        )
    }

    /// Keeps the preview on the selected row's place: from the open file, or read from disk.
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_ref() else { return };
        let wanted = match picker.rows.get(picker.selected).map(|r| &r.target) {
            Some(Target::At { path, pos, .. })
                if picker.found.is_some() || matches!(Mode::of(picker.input.read(cx).text()).0, Mode::Symbols | Mode::Text) =>
            {
                Some((path.clone(), pos.line))
            }
            _ => None,
        };
        let Some((path, line)) = wanted else {
            if let Some(picker) = self.picker.as_mut() {
                picker.preview = None;
            }
            return;
        };
        if picker.preview.as_ref().is_some_and(|p| p.path == path && p.line == line) {
            return;
        }
        if let Some(doc) = self.docs.iter().find(|d| d.path == path) {
            let preview = preview_of(&path, line, (0..doc.buffer.line_count()).map(|i| doc.buffer.line(i).to_string()));
            if let Some(picker) = self.picker.as_mut() {
                picker.preview = Some(preview);
            }
            return;
        }
        // Read once per place: mark it as asked for, filled in when read.
        if let Some(picker) = self.picker.as_mut() {
            picker.preview = Some(Preview { path: path.clone(), line, first: line, lines: Vec::new(), note: None });
        }
        cx.spawn(async move |this, cx| {
            let read = path.clone();
            let text = cx.background_spawn(async move { std::fs::read_to_string(&read).unwrap_or_default() }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(picker) = this.picker.as_mut().filter(|p| p.preview.as_ref().is_some_and(|p| p.path == path && p.line == line))
                {
                    picker.preview = Some(preview_of(&path, line, text.lines().map(str::to_string)));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_picker(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.refresh_preview(cx);
        let font = crate::settings::terminal_font(cx);
        let picker = self.picker.as_ref()?;
        let query = picker.input.read(cx).text().to_string();
        let (mode, _) = if picker.found.is_some() { (Mode::Found, "") } else { Mode::of(&query) };
        let hint = match mode {
            Mode::Files => t(cx, "nav.hint_files"),
            Mode::FileSymbols => t(cx, "nav.hint_file_symbols"),
            Mode::Symbols => t(cx, "nav.hint_symbols"),
            Mode::Line => t(cx, "nav.hint_line"),
            Mode::Text => t(cx, "nav.hint_text"),
            Mode::Found => "",
        };
        let hint: SharedString = match picker.found_text {
            Some((places, files, more)) if mode == Mode::Text && places > 0 => {
                let places = format!("{places}{}", if more { "+" } else { "" });
                tf(cx, "nav.text_found", &[("n", &places), ("files", &files.to_string())]).into()
            }
            _ => hint.into(),
        };
        let empty = if picker.loading {
            t(cx, "nav.loading")
        } else if mode == Mode::Symbols && query.len() <= 1 {
            t(cx, "nav.type_symbol")
        } else if mode == Mode::Text && query.chars().count() <= 2 {
            t(cx, "nav.type_text")
        } else {
            t(cx, "find.none")
        };
        let mut list =
            div().id("editor-picker-list").flex().flex_col().max_h(px(380.)).overflow_y_scroll().track_scroll(&picker.scroll).p_1();
        if picker.rows.is_empty() {
            list = list.child(div().px_3().py_2().t_small().text_color(hex(Chrome::MUTED)).child(empty));
        }
        for (index, row) in picker.rows.iter().enumerate() {
            let selected = index == picker.selected;
            list = list.child(
                div()
                    .id(("editor-picker-row", index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if selected { hex_alpha(Chrome::ACCENT, 0.55) } else { hex_alpha(0, 0.) })
                    .hover(|s| s.bg(hex(if selected { Chrome::ACCENT } else { Chrome::HOVER })))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.picker_confirm(Some(index), window, cx)))
                    .children(row.tag.map(|tag| {
                        // The problems list's tags in their colors.
                        let color = match tag {
                            "error" => Chrome::ERROR,
                            "warning" => Chrome::WARNING,
                            _ => Chrome::BLUE,
                        };
                        div().w(px(54.)).flex_shrink_0().truncate().t_caption().text_color(hex(color)).child(tag)
                    }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(gpui::relative(0.6))
                            .truncate()
                            .t_body()
                            .text_color(hex(Chrome::BRIGHT))
                            .child(marked(row)),
                    )
                    .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::MUTED)).child(row.detail.clone())),
            );
        }
        let title = picker.found.as_ref().map(|(title, _)| title.clone());
        let input = picker.input.clone();
        let preview = picker.preview.clone().filter(|p| !p.lines.is_empty()).map(|p| {
            div()
                .id("editor-picker-preview")
                .w(gpui::relative(0.5))
                .flex_shrink_0()
                .max_h(px(380.))
                .overflow_hidden()
                .border_l_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(Chrome::EDITOR))
                .py_1()
                .font_family(font)
                .text_size(px(12.))
                .children(p.lines.iter().enumerate().map(|(i, text)| {
                    let number = p.first + i;
                    div()
                        .flex()
                        .gap_2()
                        .px_2()
                        .when(number == p.line, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.25)))
                        .child(div().w(px(34.)).flex_shrink_0().text_color(hex(Chrome::MUTED)).child((number + 1).to_string()))
                        .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(text.clone()))
                }))
        });
        let wide = preview.is_some();
        Some(
            div()
                .id("editor-picker-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(44.))
                .occlude()
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_picker(window, cx)))
                .child(
                    div()
                        .id("editor-picker")
                        .key_context(PICKER)
                        .w(px(if wide { 980. } else { 640. }))
                        .max_w(gpui::relative(0.94))
                        .flex()
                        .flex_col()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .rounded_lg()
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .on_action(cx.listener(|this, _: &PickerNext, _, cx| this.picker_step(1, cx)))
                        .on_action(cx.listener(|this, _: &PickerPrevious, _, cx| this.picker_step(-1, cx)))
                        .children(title.map(|title| {
                            div()
                                .px_3()
                                .pt_2()
                                .t_small()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(hex(Chrome::FOREGROUND))
                                .child(title)
                        }))
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(hex(Chrome::BORDER))
                                .t_body()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(input),
                        )
                        .child(div().flex().child(div().flex_1().min_w_0().child(list)).children(preview))
                        .when(!hint.is_empty(), |d| {
                            d.child(
                                div()
                                    .px_3()
                                    .py_1p5()
                                    .border_t_1()
                                    .border_color(hex(Chrome::BORDER))
                                    .t_caption()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(hint),
                            )
                        }),
                )
                .into_any_element(),
        )
    }
}

fn relative(project: &Path, path: &Path) -> String {
    path.strip_prefix(project).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Files matching `query`, best first; while nothing is typed, the files opened lately (`recent`,
/// newest first) and then the rest in order. Among equal matches a recent file wins.
fn file_rows(project: &Path, files: &[String], query: &str, recent: &[String]) -> Vec<Row> {
    let boost = |rel: &String| recent.iter().position(|r| r == rel).map_or(0, |i| 40 - i.min(20) as i64);
    let mut scored: Vec<(i64, &String)> =
        files.iter().filter_map(|rel| symbols::fuzzy_score(rel, query).map(|s| (s + boost(rel), rel))).collect();
    if query.is_empty() {
        scored.sort_by_key(|(_, rel)| recent.iter().position(|r| r == *rel).unwrap_or(usize::MAX));
    } else {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    }
    scored
        .into_iter()
        .take(MAX_ROWS)
        .map(|(_, rel)| {
            let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
            Row {
                tag: None,
                label: name.to_string().into(),
                detail: dir.to_string().into(),
                target: Target::At { path: project.join(rel), pos: Pos::default(), len: 0 },
                mark: None,
            }
        })
        .collect()
}

/// The first lines of a language server's hover text: the signature and the start of its
/// documentation.
pub(super) fn note_lines(note: &str) -> Vec<String> {
    note.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).take(4).map(|l| l.chars().take(160).collect()).collect()
}

/// A row's label, with what the query matched in it marked.
fn marked(row: &Row) -> gpui::StyledText {
    let text = gpui::StyledText::new(row.label.clone());
    match row.mark.filter(|&(start, end)| start < end && end <= row.label.len()) {
        Some((start, end)) => text.with_highlights([(
            start..end,
            gpui::HighlightStyle {
                background_color: Some(hex_alpha(Chrome::WARNING, 0.3)),
                font_weight: Some(crate::theme::EMPHASIS),
                ..Default::default()
            },
        )]),
        None => text,
    }
}

fn text_row(found: &project_index::TextMatch) -> Row {
    let usage = &found.usage;
    Row {
        tag: None,
        label: usage.text.clone().into(),
        detail: format!("{}:{}", usage.rel, usage.line + 1).into(),
        target: Target::At { path: usage.path.clone(), pos: Pos::new(usage.line, usage.col), len: found.len },
        mark: found.mark,
    }
}

fn location_row(location: &Location) -> Row {
    Row {
        tag: Some(location.kind.tag()),
        label: location.name.clone().into(),
        detail: format!("{}:{}", location.rel, location.line + 1).into(),
        target: Target::At { path: location.path.clone(), pos: Pos::new(location.line, location.col), len: location.name.len() },
        mark: None,
    }
}

fn usage_row(usage: &Usage, len: usize) -> Row {
    Row {
        tag: None,
        label: usage.text.clone().into(),
        detail: format!("{}:{}", usage.rel, usage.line + 1).into(),
        target: Target::At { path: usage.path.clone(), pos: Pos::new(usage.line, usage.col), len },
        mark: None,
    }
}

/// The quoted text around byte `col` (without the quotes).
fn quoted_range(line: &str, col: usize) -> Option<(usize, usize)> {
    let col = col.min(line.len());
    let open = line[..col].rfind(['"', '\''])?;
    let quote = line.as_bytes()[open];
    let close = line[open + 1..].find(quote as char)? + open + 1;
    (close >= col && close > open + 1).then_some((open + 1, close))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_prefix_picks_the_list() {
        assert_eq!(Mode::of("main"), (Mode::Files, "main"));
        assert_eq!(Mode::of("@load"), (Mode::FileSymbols, "load"));
        assert_eq!(Mode::of("#User"), (Mode::Symbols, "User"));
        assert_eq!(Mode::of(":42"), (Mode::Line, "42"));
        assert_eq!(Mode::of("%/api/products"), (Mode::Text, "/api/products"));
    }

    #[test]
    fn files_are_ranked_by_name() {
        let files = vec!["src/app.ts".to_string(), "src/model/user.ts".to_string(), "test/user_test.ts".to_string()];
        let rows = file_rows(Path::new("/p"), &files, "user", &[]);
        assert_eq!(rows.iter().map(|r| r.label.to_string()).collect::<Vec<_>>(), vec!["user.ts", "user_test.ts"]);
        let names = |rows: Vec<Row>| rows.iter().map(|r| r.label.to_string()).collect::<Vec<_>>();
        let recent = vec!["test/user_test.ts".to_string()];
        assert_eq!(names(file_rows(Path::new("/p"), &files, "", &recent)), vec!["user_test.ts", "app.ts", "user.ts"], "recent first");
        assert_eq!(names(file_rows(Path::new("/p"), &files, "user", &recent))[0], "user_test.ts", "a recent file wins a close match");
    }

    #[test]
    fn previews_show_the_lines_around_a_place() {
        let lines = (0..30).map(|i| format!("line {i}"));
        let preview = preview_of(Path::new("/p/a.rs"), 2, lines);
        assert_eq!((preview.first, preview.lines.len()), (0, PREVIEW_ABOVE + PREVIEW_BELOW + 1));
        let preview = preview_of(Path::new("/p/a.rs"), 20, (0..30).map(|i| format!("line {i}")));
        assert_eq!(preview.lines[preview.line - preview.first], "line 20");
    }

    #[test]
    fn quoted_paths_under_the_pointer() {
        let line = "import x from './lib/api';";
        assert_eq!(quoted_range(line, 18).map(|(s, e)| &line[s..e]), Some("./lib/api"));
        assert_eq!(quoted_range(line, 3), None);
    }
}
