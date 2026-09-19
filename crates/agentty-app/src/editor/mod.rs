//! File editor: files opened from the files panel, shown in place of the terminals with a tab each
//! in the tab strip. Syntax colors, editing with undo, saving, formatting with the installed
//! formatter and a button that hands the file to VS Code (or the system's editor).
//!
//! Files are only read and written here — never run (see `file.rs`, `format.rs`, `external.rs`).

pub mod buffer;
mod element;
pub mod external;
pub mod file;
pub mod format;
pub mod highlight;
pub mod language;

use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, Tooltip, TypeScale};
use buffer::{Buffer, Pos, Selection};
use file::{Opened, ReadOnly, Stamp};
use gpui::{
    actions, div, prelude::*, px, App, ClickEvent, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, FontWeight, Pixels, Point,
    Window,
};
use highlight::Highlights;
use language::{Formatter, Language};
use std::path::{Path, PathBuf};
use std::time::Duration;

actions!(
    code_editor,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
        Backspace,
        Delete,
        DeleteWordBack,
        Newline,
        Indent,
        Outdent,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Save,
        Format,
        CloseFile,
    ]
);

pub const CONTEXT: &str = "CodeEditor";
/// How often open files are checked for changes made by others (agents, git, other editors).
const DISK_CHECK: Duration = Duration::from_secs(2);

pub fn bind_keys(cx: &mut App) {
    use crate::key;
    let c = Some(CONTEXT);
    cx.bind_keys([
        key("left", MoveLeft, c),
        key("right", MoveRight, c),
        key("up", MoveUp, c),
        key("down", MoveDown, c),
        key("shift-left", SelectLeft, c),
        key("shift-right", SelectRight, c),
        key("shift-up", SelectUp, c),
        key("shift-down", SelectDown, c),
        key("home", LineStart, c),
        key("end", LineEnd, c),
        key("shift-home", SelectLineStart, c),
        key("shift-end", SelectLineEnd, c),
        key("pageup", PageUp, c),
        key("pagedown", PageDown, c),
        key("shift-pageup", SelectPageUp, c),
        key("shift-pagedown", SelectPageDown, c),
        key("backspace", Backspace, c),
        key("shift-backspace", Backspace, c),
        key("delete", Delete, c),
        key("enter", Newline, c),
        key("shift-enter", Newline, c),
        key("tab", Indent, c),
        key("shift-tab", Outdent, c),
        key("cmd-a", SelectAll, c),
        key("cmd-c", Copy, c),
        key("cmd-x", Cut, c),
        key("cmd-v", Paste, c),
        key("cmd-z", Undo, c),
        key("cmd-shift-z", Redo, c),
        key("cmd-s", Save, c),
        key("cmd-w", CloseFile, c),
        key("shift-alt-f", Format, c),
    ]);
    // Moving by word and to the ends of lines and of the file: macOS and PC conventions differ.
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            key("alt-left", WordLeft, c),
            key("alt-right", WordRight, c),
            key("alt-shift-left", SelectWordLeft, c),
            key("alt-shift-right", SelectWordRight, c),
            key("cmd-left", LineStart, c),
            key("cmd-right", LineEnd, c),
            key("cmd-shift-left", SelectLineStart, c),
            key("cmd-shift-right", SelectLineEnd, c),
            key("cmd-up", DocStart, c),
            key("cmd-down", DocEnd, c),
            key("cmd-shift-up", SelectDocStart, c),
            key("cmd-shift-down", SelectDocEnd, c),
            key("alt-backspace", DeleteWordBack, c),
            key("ctrl-a", LineStart, c),
            key("ctrl-e", LineEnd, c),
        ]);
    } else {
        cx.bind_keys([
            gpui::KeyBinding::new("ctrl-left", WordLeft, c),
            gpui::KeyBinding::new("ctrl-right", WordRight, c),
            gpui::KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
            gpui::KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
            gpui::KeyBinding::new("ctrl-home", DocStart, c),
            gpui::KeyBinding::new("ctrl-end", DocEnd, c),
            gpui::KeyBinding::new("ctrl-shift-home", SelectDocStart, c),
            gpui::KeyBinding::new("ctrl-shift-end", SelectDocEnd, c),
            gpui::KeyBinding::new("ctrl-backspace", DeleteWordBack, c),
            gpui::KeyBinding::new("ctrl-y", Redo, c),
        ]);
    }
}

pub enum EditorEvent {
    /// Files were opened or closed, or one was changed or saved: the tab strip shows it.
    TabsChanged,
    /// The last file was closed.
    Empty,
    /// The user chose to leave unsaved changes behind (or saved them): go on with `then`.
    Proceed(AfterDiscard),
    /// The user kept the app or window open after all.
    StayOpen,
}

/// What waits for "discard unsaved changes?".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterDiscard {
    Quit,
    CloseWindow,
}

/// A file tab as the tab strip shows it.
pub struct TabInfo {
    pub name: String,
    pub path: PathBuf,
    pub dirty: bool,
    pub active: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, PartialEq, Eq)]
enum NoticeAction {
    Reload,
    KeepMine,
}

#[derive(Clone)]
struct Notice {
    text: String,
    tone: Tone,
    /// A command to show in a copyable box (how to install a formatter).
    code: Option<String>,
    actions: Vec<NoticeAction>,
}

enum Content {
    Text,
    Binary,
    TooLarge(u64),
    Failed(String),
}

pub(crate) struct Document {
    /// Path as opened (shown, and handed to other editors).
    path: PathBuf,
    /// Where the bytes are written (the file a link points at).
    target: PathBuf,
    project: PathBuf,
    content: Content,
    buffer: Buffer,
    language: Language,
    highlights: Highlights,
    format: file::Format,
    read_only: Option<ReadOnly>,
    /// The user allowed editing a file outside the project.
    allow_outside: bool,
    stamp: Option<Stamp>,
    /// Scroll offset (x, y) in pixels.
    scroll: Point<f32>,
    /// Bring the cursor into view on the next paint.
    autoscroll: bool,
    notice: Option<Notice>,
    formatting: bool,
    /// Changed on disk while it had unsaved changes here (asked about in a notice).
    disk_changed: bool,
    /// Checking the disk is under way.
    checking: bool,
}

impl Document {
    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| self.path.display().to_string())
    }

    fn editable(&self) -> bool {
        matches!(self.content, Content::Text)
            && (self.read_only.is_none() || (self.allow_outside && self.read_only == Some(ReadOnly::OutsideProject)))
    }

    fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    /// Keeps the highlighter's cache in step with edits.
    fn sync_highlights(&mut self) {
        if let Some(line) = self.buffer.take_changed_from() {
            self.highlights.invalidate_from(line);
        }
    }

    /// Takes what is on disk now: text and everything read with it (read-only reason, where it
    /// really is, binary or too large now).
    fn reload(&mut self, opened: Opened) {
        match opened {
            Opened::Text(file) => {
                self.content = Content::Text;
                self.buffer.set_text(&file.text);
                self.buffer.mark_saved();
                self.sync_highlights();
                self.format = file.format;
                self.stamp = file.stamp;
                self.target = file.target;
                self.read_only = file.read_only;
            }
            Opened::Binary => self.content = Content::Binary,
            Opened::TooLarge(size) => self.content = Content::TooLarge(size),
            Opened::Failed(error) => self.content = Content::Failed(error),
        }
        self.disk_changed = false;
    }

    fn set_notice(&mut self, text: impl Into<String>, tone: Tone) {
        self.notice = Some(Notice { text: text.into(), tone, code: None, actions: Vec::new() });
    }
}

/// Where a mouse drag selects by: characters, words or lines (from the click count).
#[derive(Clone, Copy)]
enum DragUnit {
    Char,
    Word,
    Line,
}

enum Confirm {
    /// Close a file with unsaved changes (its path).
    CloseDirty(PathBuf),
    /// Save over a file that changed on disk since it was opened.
    Overwrite(PathBuf),
    /// Unsaved files and the app or window is closing.
    Discard(AfterDiscard),
}

pub struct CodeEditor {
    focus_handle: FocusHandle,
    docs: Vec<Document>,
    active: usize,
    confirm: Option<Confirm>,
    /// Geometry of the last paint, for mouse hits and input-method popups.
    layout: Option<element::Layout>,
    drag: Option<(DragUnit, Pos, Pos)>,
    /// Dragging the scrollbar: (mouse y at start, scroll y at start).
    scrollbar_drag: Option<(f32, f32)>,
    /// What the tab strip last showed (names and unsaved marks), to tell it only about changes.
    shown_tabs: Vec<(PathBuf, bool)>,
}

impl EventEmitter<EditorEvent> for CodeEditor {}

impl Focusable for CodeEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CodeEditor {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // The grammars take a moment to load: off the UI thread, then color what is open.
        if highlight::syntaxes().is_none() {
            let load = cx.background_spawn(async { highlight::load() });
            cx.spawn(async move |this, cx| {
                load.await;
                let _ = this.update(cx, |this, cx| {
                    for doc in &mut this.docs {
                        doc.highlights = highlights_for(doc.language, &doc.path, doc.buffer.line(0));
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(DISK_CHECK).await;
            if this.update(cx, |this, cx| this.check_disk(cx)).is_err() {
                break;
            }
        })
        .detach();
        Self {
            focus_handle: cx.focus_handle(),
            docs: Vec::new(),
            active: 0,
            confirm: None,
            layout: None,
            drag: None,
            scrollbar_drag: None,
            shown_tabs: Vec::new(),
        }
    }

    // -- files ------------------------------------------------------------------------------

    /// Opens `path` (from the project `project`), or shows it when it is already open.
    pub fn open(&mut self, path: &Path, project: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.docs.iter().position(|d| d.path == path) {
            self.active = index;
        } else {
            let doc = load_document(path, project);
            self.docs.push(doc);
            self.active = self.docs.len() - 1;
        }
        window.focus(&self.focus_handle);
        self.tabs_changed(cx);
        cx.notify();
    }

    pub fn tabs(&self) -> Vec<TabInfo> {
        self.docs
            .iter()
            .enumerate()
            .map(|(index, doc)| TabInfo { name: doc.name(), path: doc.path.clone(), dirty: doc.is_dirty(), active: index == self.active })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.docs.get(self.active).map(|d| d.path.as_path())
    }

    /// Files with unsaved changes.
    pub fn dirty_count(&self) -> usize {
        self.docs.iter().filter(|d| d.is_dirty()).count()
    }

    pub fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.docs.len() {
            if let Some(doc) = self.docs.get_mut(self.active) {
                doc.buffer.unmark();
            }
            self.active = index;
            window.focus(&self.focus_handle);
            self.tabs_changed(cx);
            cx.notify();
        }
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    /// Closes file `index`, asking first when it has unsaved changes.
    pub fn request_close(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(index) else { return };
        if doc.is_dirty() {
            self.active = index;
            self.confirm = Some(Confirm::CloseDirty(doc.path.clone()));
            self.tabs_changed(cx);
            return cx.notify();
        }
        self.close(index, cx);
    }

    fn close(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.docs.len() {
            return;
        }
        self.docs.remove(index);
        if self.active > index || self.active >= self.docs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.layout = None;
        self.tabs_changed(cx);
        if self.docs.is_empty() {
            cx.emit(EditorEvent::Empty);
        }
        cx.notify();
    }

    /// Asks what to do with unsaved files before `then` (quit, close the window). Returns false
    /// when there is nothing unsaved and `then` can go ahead right away.
    pub fn ask_discard(&mut self, then: AfterDiscard, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.docs.iter().position(|d| d.is_dirty()) else { return false };
        // A question already on screen (save this file? overwrite?) is answered first.
        if self.confirm.is_some() {
            return true;
        }
        self.active = index;
        self.confirm = Some(Confirm::Discard(then));
        cx.notify();
        true
    }

    fn tabs_changed(&mut self, cx: &mut Context<Self>) {
        let tabs: Vec<(PathBuf, bool)> = self.docs.iter().map(|d| (d.path.clone(), d.is_dirty())).collect();
        if tabs != self.shown_tabs {
            self.shown_tabs = tabs;
        }
        // The active tab changes too, so the strip is always told.
        cx.emit(EditorEvent::TabsChanged);
    }

    fn doc(&mut self) -> Option<&mut Document> {
        self.docs.get_mut(self.active)
    }

    /// Runs an edit on the active file (when it can be edited), then keeps colors, scroll and
    /// the tab strip up to date.
    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer)) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if !doc.editable() {
            return;
        }
        let was_dirty = doc.is_dirty();
        f(&mut doc.buffer);
        doc.sync_highlights();
        doc.autoscroll = true;
        if doc.is_dirty() != was_dirty {
            self.tabs_changed(cx);
        }
        cx.notify();
    }

    /// Moves the cursor (works in read-only files too).
    fn motion(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer)) {
        let Some(doc) = self.doc() else { return };
        doc.buffer.unmark();
        f(&mut doc.buffer);
        doc.sync_highlights();
        doc.autoscroll = true;
        cx.notify();
    }

    fn page_lines(&self) -> isize {
        self.layout.as_ref().map_or(20, |l| l.visible_rows.saturating_sub(2).max(1)) as isize
    }

    // -- saving -----------------------------------------------------------------------------

    fn save(&mut self, force: bool, cx: &mut Context<Self>) -> bool {
        let Some(doc) = self.docs.get_mut(self.active) else { return false };
        if !doc.editable() {
            return false;
        }
        doc.buffer.unmark();
        // Where the file really is now: a folder on the way may have become a link since it opened.
        let (target, outside) = file::resolve(&doc.path, &doc.project);
        if outside && !doc.allow_outside {
            doc.read_only = Some(ReadOnly::OutsideProject);
            doc.target = target;
            cx.notify();
            return false;
        }
        doc.target = target;
        if !force && doc.stamp.is_some() && Stamp::of(&doc.target) != doc.stamp {
            self.confirm = Some(Confirm::Overwrite(doc.path.clone()));
            cx.notify();
            return false;
        }
        let bytes = doc.format.encode(&doc.buffer.text());
        match file::save_atomic(&doc.target, &bytes) {
            Ok(()) => {
                doc.buffer.mark_saved();
                doc.stamp = Stamp::of(&doc.target);
                doc.disk_changed = false;
                doc.notice = None;
                self.tabs_changed(cx);
                cx.notify();
                true
            }
            Err(err) => {
                let text = tf(cx, "editor.save_failed", &[("error", &err.to_string())]);
                if let Some(doc) = self.doc() {
                    doc.set_notice(text, Tone::Error);
                }
                cx.notify();
                false
            }
        }
    }

    /// Saves every file with unsaved changes; false when one of them could not be saved.
    fn save_all(&mut self, cx: &mut Context<Self>) -> bool {
        let active = self.active;
        for index in 0..self.docs.len() {
            if self.docs[index].is_dirty() {
                self.active = index;
                // Files changed on disk ask first (the question shows and the rest waits).
                if !self.save(false, cx) {
                    return false;
                }
            }
        }
        self.active = active;
        true
    }

    // -- formatting -------------------------------------------------------------------------

    fn format(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if !doc.editable() || doc.formatting {
            return;
        }
        doc.buffer.unmark();
        let Some(formatter) = doc.language.formatter() else {
            let name = if doc.language.name().is_empty() { doc.name() } else { doc.language.name().to_string() };
            let text = tf(cx, "editor.no_formatter", &[("language", &name)]);
            if let Some(doc) = self.doc() {
                doc.set_notice(text, Tone::Info);
            }
            return cx.notify();
        };
        doc.formatting = true;
        doc.notice = None;
        let (path, project, text) = (doc.path.clone(), doc.project.clone(), doc.buffer.text());
        let task = cx.background_spawn(async move {
            let result = format::format(formatter, &path, &project, &text);
            (path, text, result)
        });
        cx.spawn(async move |this, cx| {
            let (path, before, result) = task.await;
            let _ = this.update(cx, |this, cx| this.formatted(&path, &before, formatter, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn formatted(
        &mut self,
        path: &Path,
        before: &str,
        formatter: Formatter,
        result: Result<String, format::FormatError>,
        cx: &mut Context<Self>,
    ) {
        let program = formatter.program();
        let messages = (
            tf(cx, "editor.formatted", &[("tool", program)]),
            tf(cx, "editor.already_formatted", &[("tool", program)]),
            t(cx, "editor.changed_while_formatting").to_string(),
        );
        let Some(index) = self.docs.iter().position(|d| d.path == path) else { return };
        let notice = match &result {
            Err(format::FormatError::Missing { program, hint }) => Some(Notice {
                text: tf(cx, "editor.formatter_missing", &[("tool", program)]),
                tone: Tone::Warning,
                code: Some(hint.to_string()),
                actions: Vec::new(),
            }),
            Err(format::FormatError::ProjectCode(config)) => Some(Notice {
                text: tf(cx, "editor.formatter_project_code", &[("file", &config.file_name().unwrap_or_default().to_string_lossy())]),
                tone: Tone::Warning,
                code: None,
                actions: Vec::new(),
            }),
            Err(format::FormatError::Failed(message)) => Some(Notice {
                text: tf(cx, "editor.formatter_failed", &[("tool", program)]),
                tone: Tone::Error,
                code: (!message.is_empty()).then(|| message.clone()),
                actions: Vec::new(),
            }),
            Err(format::FormatError::TimedOut) => Some(Notice {
                text: tf(cx, "editor.formatter_timeout", &[("tool", program)]),
                tone: Tone::Error,
                code: None,
                actions: Vec::new(),
            }),
            Ok(_) => None,
        };
        let doc = &mut self.docs[index];
        doc.formatting = false;
        match result {
            Ok(formatted) => {
                if doc.buffer.text() != before {
                    doc.set_notice(messages.2, Tone::Warning);
                } else if formatted == before {
                    doc.set_notice(messages.1, Tone::Success);
                } else {
                    doc.buffer.set_text(&formatted);
                    doc.sync_highlights();
                    doc.autoscroll = true;
                    doc.set_notice(messages.0, Tone::Success);
                }
            }
            Err(_) => doc.notice = notice,
        }
        self.tabs_changed(cx);
        cx.notify();
    }

    // -- other editors ----------------------------------------------------------------------

    fn open_externally(&mut self, cx: &mut Context<Self>) {
        let preference = crate::settings::settings(cx).external_editor;
        let Some(doc) = self.docs.get(self.active) else { return };
        let cursor = doc.buffer.cursor();
        let column = buffer::display_column(doc.buffer.line(cursor.line), cursor.col) + 1;
        let result = external::open(&doc.path, cursor.line + 1, column, preference, &doc.project);
        let message = match result {
            Ok(()) => return,
            Err(external::ExternalError::NotInstalled(app)) => tf(cx, "editor.external_missing", &[("app", app)]),
            Err(external::ExternalError::Failed(error)) => tf(cx, "editor.external_failed", &[("error", &error)]),
        };
        if let Some(doc) = self.doc() {
            doc.set_notice(message, Tone::Warning);
        }
        cx.notify();
    }

    // -- changes on disk --------------------------------------------------------------------

    /// Picks up changes others made to open files: reloads files without unsaved changes here,
    /// and asks about the others.
    fn check_disk(&mut self, cx: &mut Context<Self>) {
        for index in 0..self.docs.len() {
            let doc = &mut self.docs[index];
            if doc.checking || doc.formatting || !matches!(doc.content, Content::Text) {
                continue;
            }
            doc.checking = true;
            let (target, path, project) = (doc.target.clone(), doc.path.clone(), doc.project.clone());
            let known = doc.stamp;
            let opened_path = path.clone();
            let task = cx.background_spawn(async move {
                let now = Stamp::of(&target);
                if now == known {
                    return None;
                }
                Some((now, now.map(|_| file::open(&opened_path, &project))))
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    let Some(doc) = this.docs.iter_mut().find(|d| d.path == path) else { return };
                    doc.checking = false;
                    let Some((stamp, opened)) = result else { return };
                    if doc.stamp != known {
                        return; // saved here in the meantime
                    }
                    let deleted = t(cx, "editor.deleted_on_disk").to_string();
                    let changed = t(cx, "editor.changed_on_disk").to_string();
                    match opened {
                        None => {
                            doc.stamp = None;
                            doc.set_notice(deleted, Tone::Warning);
                        }
                        Some(opened) if !doc.is_dirty() && !doc.disk_changed => {
                            doc.reload(opened);
                            doc.stamp = stamp;
                        }
                        Some(_) => {
                            if !doc.disk_changed {
                                doc.disk_changed = true;
                                doc.notice = Some(Notice {
                                    text: changed,
                                    tone: Tone::Warning,
                                    code: None,
                                    actions: vec![NoticeAction::Reload, NoticeAction::KeepMine],
                                });
                            }
                        }
                    }
                    this.tabs_changed(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn notice_action(&mut self, action: NoticeAction, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        match action {
            NoticeAction::Reload => doc.reload(file::open(&doc.path, &doc.project)),
            // Theirs is overwritten on the next save.
            NoticeAction::KeepMine => doc.stamp = Stamp::of(&doc.target),
        }
        doc.disk_changed = false;
        doc.notice = None;
        self.tabs_changed(cx);
        cx.notify();
    }

    // -- mouse ------------------------------------------------------------------------------

    fn mouse_down(&mut self, event: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        let Some(layout) = self.layout.as_ref() else { return };
        if layout.on_scrollbar(event.position) {
            let scroll = self.docs.get(self.active).map_or(0., |d| d.scroll.y);
            self.scrollbar_drag = Some((f32::from(event.position.y), scroll));
            return;
        }
        let Some(pos) = self.docs.get(self.active).map(|doc| layout.position_for(event.position, &doc.buffer)) else { return };
        let in_gutter = layout.in_gutter(event.position);
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        doc.buffer.unmark();
        let unit = match (in_gutter, event.click_count) {
            (true, _) | (_, 3..) => DragUnit::Line,
            (_, 2) => DragUnit::Word,
            _ => DragUnit::Char,
        };
        let (start, end) = unit_range(&doc.buffer, unit, pos);
        if event.modifiers.shift {
            doc.buffer.move_to(if pos < doc.buffer.selection.anchor { start } else { end }, true);
            let anchor = doc.buffer.selection.anchor;
            self.drag = Some((unit, anchor, anchor));
        } else {
            doc.buffer.move_to(start, false);
            doc.buffer.move_to(end, true);
            self.drag = Some((unit, start, end));
        }
        doc.autoscroll = true;
        cx.notify();
    }

    fn mouse_move(&mut self, event: &gpui::MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(gpui::MouseButton::Left) {
            self.drag = None;
            self.scrollbar_drag = None;
            return;
        }
        let Some(layout) = self.layout.as_ref() else { return };
        if let Some((start_y, start_scroll)) = self.scrollbar_drag {
            let scroll = layout.scroll_for_thumb_drag(start_scroll, f32::from(event.position.y) - start_y);
            if let Some(doc) = self.doc() {
                doc.scroll.y = scroll;
            }
            return cx.notify();
        }
        let Some((unit, origin_start, origin_end)) = self.drag else { return };
        let Some(pos) = self.docs.get(self.active).map(|doc| layout.position_for(event.position, &doc.buffer)) else { return };
        let Some(doc) = self.doc() else { return };
        let (start, end) = unit_range(&doc.buffer, unit, pos);
        if pos < origin_start {
            doc.buffer.selection = Selection { anchor: origin_end, head: start };
        } else {
            doc.buffer.selection = Selection { anchor: origin_start, head: end.max(origin_end) };
        }
        doc.autoscroll = true;
        cx.notify();
    }

    fn mouse_up(&mut self, _: &gpui::MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag = None;
        self.scrollbar_drag = None;
    }

    fn scroll_wheel(&mut self, event: &gpui::ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.as_ref() else { return };
        let delta = event.delta.pixel_delta(px(layout.line_height));
        let (max_x, max_y) = (layout.max_scroll_x, layout.max_scroll_y);
        let Some(doc) = self.doc() else { return };
        doc.scroll.y = (doc.scroll.y - f32::from(delta.y)).clamp(0., max_y);
        doc.scroll.x = (doc.scroll.x - f32::from(delta.x)).clamp(0., max_x);
        doc.autoscroll = false;
        cx.notify();
    }

    // -- rendering --------------------------------------------------------------------------

    fn render_toolbar(&self, doc: &Document, cx: &mut Context<Self>) -> impl IntoElement {
        let relative = doc.path.strip_prefix(&doc.project).map(Path::to_path_buf).unwrap_or_else(|_| doc.path.clone());
        let folder = relative.parent().map(|p| p.display().to_string()).filter(|p| !p.is_empty());
        let can_format = doc.editable() && !doc.formatting;
        let button = |id: &'static str, glyph: &'static str, label: String, enabled: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .t_small()
                .text_color(hex(if enabled { Chrome::FOREGROUND } else { Chrome::MUTED }))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT))))
                .child(icon(glyph, IconSize::INLINE, hex(if enabled { Chrome::FOREGROUND } else { Chrome::MUTED })))
                .child(label)
        };
        div()
            .h(px(34.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::EDITOR))
            .child(icon("file-text", IconSize::INLINE, hex(Chrome::MUTED)))
            .child(
                div()
                    .id("editor-path")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .t_small()
                    .overflow_hidden()
                    .tooltip(Tooltip::text(crate::ui::tilde(&doc.path), None))
                    .when_some(folder, |d, folder| {
                        d.child(div().flex_shrink().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(format!("{folder}/")))
                    })
                    .child(div().flex_shrink_0().text_color(hex(Chrome::BRIGHT)).font_weight(FontWeight::MEDIUM).child(doc.name()))
                    .when(doc.is_dirty(), |d| d.child(div().flex_shrink_0().size(px(7.)).rounded_full().bg(hex(Chrome::BRIGHT))))
                    .when(!doc.editable() && matches!(doc.content, Content::Text), |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .ml_1()
                                .px_1p5()
                                .rounded_sm()
                                .flex()
                                .items_center()
                                .gap_1()
                                .bg(hex_alpha(Chrome::WARNING, 0.18))
                                .t_caption()
                                .text_color(hex(Chrome::WARNING))
                                .child(icon("lock", 10., hex(Chrome::WARNING)))
                                .child(t(cx, "editor.read_only")),
                        )
                    }),
            )
            .when(doc.language.formatter().is_some() && matches!(doc.content, Content::Text), |d| {
                d.child(
                    button(
                        "editor-format",
                        "wand-sparkles",
                        if doc.formatting { t(cx, "editor.formatting").to_string() } else { t(cx, "editor.format").to_string() },
                        can_format,
                    )
                    .tooltip(Tooltip::text(
                        with_shortcut(
                            tf(cx, "editor.format_with", &[("tool", doc.language.formatter().map_or("", Formatter::program))]),
                            "⇧⌥F",
                        ),
                        None,
                    ))
                    .when(can_format, |d| d.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.format(cx)))),
                )
            })
            .when(matches!(doc.content, Content::Text), |d| {
                let enabled = doc.editable() && doc.is_dirty();
                d.child(
                    button("editor-save", "save", t(cx, "editor.save").to_string(), enabled)
                        .tooltip(Tooltip::text(with_shortcut(t(cx, "editor.save").to_string(), "⌘S"), None))
                        .when(enabled, |d| {
                            d.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.save(false, cx);
                            }))
                        }),
                )
            })
            .child(
                crate::ui::icon_only("editor-external", "code", cx.listener(|this, _: &ClickEvent, _, cx| this.open_externally(cx)))
                    .tooltip(Tooltip::text(t(cx, "editor.open_external"), None)),
            )
    }

    fn render_notice(&self, doc: &Document, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let read_only_note = match (&doc.content, doc.read_only) {
            (Content::Text, Some(reason)) if !doc.editable() => Some(reason),
            _ => None,
        };
        if doc.notice.is_none() && read_only_note.is_none() {
            return None;
        }
        let (text, tone, code, actions, allow) = match &doc.notice {
            Some(notice) => (notice.text.clone(), notice.tone, notice.code.clone(), notice.actions.clone(), false),
            None => {
                let reason = read_only_note.expect("checked above");
                let key = match reason {
                    ReadOnly::Large => "editor.read_only_large",
                    ReadOnly::LongLines => "editor.read_only_long_lines",
                    ReadOnly::NotUtf8 => "editor.read_only_encoding",
                    ReadOnly::OutsideProject => "editor.read_only_outside",
                    ReadOnly::NoPermission => "editor.read_only_permission",
                };
                let text = if reason == ReadOnly::OutsideProject {
                    tf(cx, key, &[("path", &crate::ui::tilde(&doc.target))])
                } else {
                    t(cx, key).to_string()
                };
                (text, Tone::Warning, None, Vec::new(), reason == ReadOnly::OutsideProject)
            }
        };
        let color = match tone {
            Tone::Info => Chrome::BLUE,
            Tone::Success => Chrome::SUCCESS,
            Tone::Warning => Chrome::WARNING,
            Tone::Error => Chrome::ERROR,
        };
        let glyph = match tone {
            Tone::Success => "circle-check",
            Tone::Error => "circle-x",
            Tone::Warning => "shield-alert",
            Tone::Info => "info",
        };
        let dismissible = doc.notice.is_some() && actions.is_empty();
        let action_button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .t_small()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        let mut row = div()
            .flex_shrink_0()
            .px_3()
            .py_1p5()
            .flex()
            .items_start()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex_alpha(color, 0.10))
            .child(div().pt(px(2.)).child(icon(glyph, IconSize::INLINE, hex(color))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(text))
                    .when_some(code, |d, code| {
                        d.child(
                            div()
                                .id("editor-notice-code")
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(hex(Chrome::PANEL))
                                        .font_family(crate::settings::settings(cx).font_family.clone())
                                        .t_small()
                                        .text_color(hex(Chrome::BRIGHT))
                                        .child(code.clone()),
                                )
                                .child(
                                    crate::ui::icon_only_sized(
                                        "editor-notice-copy",
                                        "copy",
                                        20.,
                                        IconSize::INLINE,
                                        move |_: &ClickEvent, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(code.clone())),
                                    )
                                    .tooltip(Tooltip::text(t(cx, "editor.copy"), None)),
                                ),
                        )
                    }),
            );
        for action in actions {
            let (id, key, primary) = match action {
                NoticeAction::Reload => ("editor-notice-reload", "editor.reload", true),
                NoticeAction::KeepMine => ("editor-notice-keep", "editor.keep_mine", false),
            };
            row = row.child(
                action_button(id, t(cx, key).to_string(), primary)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.notice_action(action.clone(), cx))),
            );
        }
        if allow {
            row = row.child(action_button("editor-allow-outside", t(cx, "editor.allow_edit").to_string(), false).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    if let Some(doc) = this.doc() {
                        doc.allow_outside = true;
                    }
                    cx.notify();
                },
            )));
        }
        if dismissible {
            row = row.child(crate::ui::icon_only_sized(
                "editor-notice-close",
                "x",
                20.,
                IconSize::INLINE,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(doc) = this.doc() {
                        doc.notice = None;
                    }
                    cx.notify();
                }),
            ));
        }
        Some(row)
    }

    fn render_placeholder(&self, doc: &Document, cx: &mut Context<Self>) -> impl IntoElement {
        let text = match &doc.content {
            Content::Binary => t(cx, "editor.binary").to_string(),
            Content::TooLarge(size) => tf(cx, "editor.too_large", &[("size", &format!("{:.1} MB", *size as f64 / 1_048_576.))]),
            Content::Failed(error) => tf(cx, "editor.open_failed", &[("error", error)]),
            Content::Text => String::new(),
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(icon("file", 32., hex(Chrome::MUTED)))
            .child(div().max_w(px(420.)).text_center().t_body().text_color(hex(Chrome::FOREGROUND)).child(text))
            .child(crate::ui::action_button(
                "editor-placeholder-external",
                t(cx, "editor.open_external"),
                cx.listener(|this, _: &ClickEvent, _, cx| this.open_externally(cx)),
            ))
    }

    fn render_status(&self, doc: &Document, cx: &mut Context<Self>) -> impl IntoElement {
        let cursor = doc.buffer.cursor();
        let column = buffer::display_column(doc.buffer.line(cursor.line), cursor.col) + 1;
        let selected = if doc.buffer.selection.is_empty() { 0 } else { doc.buffer.selected_text().chars().count() };
        let indent = match doc.buffer.indent_unit() {
            "\t" => t(cx, "editor.tabs").to_string(),
            unit => tf(cx, "editor.spaces", &[("n", &unit.len().to_string())]),
        };
        let language = match (doc.language.name(), doc.highlights.grammar_name()) {
            ("", Some(grammar)) => grammar.to_string(),
            ("", None) => t(cx, "editor.plain_text").to_string(),
            (name, _) => name.to_string(),
        };
        let item = |text: String| div().flex_shrink_0().child(text);
        div()
            .h(px(24.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_4()
            .border_t_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::EDITOR))
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .when(matches!(doc.content, Content::Text), |d| {
                d.child(item(tf(cx, "editor.position", &[("line", &(cursor.line + 1).to_string()), ("column", &column.to_string())])))
            })
            .when(selected > 0, |d| d.child(item(tf(cx, "editor.selected", &[("n", &selected.to_string())]))))
            .child(div().flex_1())
            .when(matches!(doc.content, Content::Text), |d| {
                d.child(item(indent))
                    .child(item(if doc.format.bom { "UTF-8 BOM".into() } else { "UTF-8".into() }))
                    .child(item(if doc.format.crlf { "CRLF".into() } else { "LF".into() }))
                    .child(item(language))
            })
    }

    fn render_confirm(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let confirm = self.confirm.as_ref()?;
        let name = |path: &Path| path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (title, body, buttons): (String, String, Vec<(&'static str, String, u8)>) = match confirm {
            Confirm::CloseDirty(path) => (
                tf(cx, "editor.close_dirty_title", &[("name", &name(path))]),
                t(cx, "editor.close_dirty_body").to_string(),
                vec![
                    ("editor-confirm-save", t(cx, "editor.save").to_string(), 0),
                    ("editor-confirm-discard", t(cx, "editor.dont_save").to_string(), 1),
                    ("editor-confirm-cancel", t(cx, "editor.cancel").to_string(), 2),
                ],
            ),
            Confirm::Overwrite(path) => (
                tf(cx, "editor.overwrite_title", &[("name", &name(path))]),
                t(cx, "editor.overwrite_body").to_string(),
                vec![
                    ("editor-confirm-overwrite", t(cx, "editor.overwrite").to_string(), 3),
                    ("editor-confirm-reload", t(cx, "editor.reload").to_string(), 4),
                    ("editor-confirm-cancel", t(cx, "editor.cancel").to_string(), 2),
                ],
            ),
            Confirm::Discard(_) => (
                tf(cx, "editor.discard_title", &[("n", &self.dirty_count().to_string())]),
                t(cx, "editor.discard_body").to_string(),
                vec![
                    ("editor-confirm-save-all", t(cx, "editor.save_all").to_string(), 5),
                    ("editor-confirm-discard", t(cx, "editor.dont_save").to_string(), 6),
                    ("editor-confirm-cancel", t(cx, "editor.cancel").to_string(), 2),
                ],
            ),
        };
        let mut row = div().flex().justify_end().gap_2();
        for (index, (id, label, choice)) in buttons.into_iter().enumerate() {
            row = row.child(
                div()
                    .id(id)
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .t_body()
                    .cursor_pointer()
                    .bg(if index == 0 { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                    .text_color(hex(Chrome::BRIGHT))
                    .hover(|s| s.opacity(0.85))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.answer(choice, window, cx))),
            );
        }
        Some(
            div()
                .id("editor-confirm")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .child(
                    div()
                        .w(px(400.))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .child(div().t_title().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(title))
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(body))
                        .child(row),
                ),
        )
    }

    /// A button of the confirmation dialog: 0 save, 1 don't save, 2 cancel, 3 overwrite,
    /// 4 reload, 5 save all, 6 discard all.
    fn answer(&mut self, choice: u8, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm.take() else { return };
        match (confirm, choice) {
            (Confirm::CloseDirty(path), 0) => {
                if let Some(index) = self.docs.iter().position(|d| d.path == path) {
                    self.active = index;
                    if self.save(false, cx) {
                        self.close(index, cx);
                    }
                }
            }
            (Confirm::CloseDirty(path), 1) => {
                if let Some(index) = self.docs.iter().position(|d| d.path == path) {
                    self.close(index, cx);
                }
            }
            (Confirm::Overwrite(_), 3) => {
                self.save(true, cx);
            }
            (Confirm::Overwrite(_), 4) => self.notice_action(NoticeAction::Reload, cx),
            (Confirm::Discard(then), 5) => {
                if self.save_all(cx) {
                    cx.emit(EditorEvent::Proceed(then));
                } else {
                    cx.emit(EditorEvent::StayOpen);
                }
            }
            (Confirm::Discard(then), 6) => cx.emit(EditorEvent::Proceed(then)),
            (Confirm::Discard(_), _) => cx.emit(EditorEvent::StayOpen),
            _ => {}
        }
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// For the debug driver's `probe`.
    pub fn debug_state(&self) -> serde_json::Value {
        serde_json::json!({
            "files": self.docs.iter().map(|d| serde_json::json!({
                "path": d.path,
                "dirty": d.is_dirty(),
                "language": d.language.name(),
                "grammar": d.highlights.grammar_name(),
                "readOnly": d.read_only.map(|r| format!("{r:?}")),
                "editable": d.editable(),
                "lines": d.buffer.line_count(),
                "cursor": [d.buffer.cursor().line, d.buffer.cursor().col],
                "notice": d.notice.as_ref().map(|n| n.text.clone()),
            })).collect::<Vec<_>>(),
            "active": self.active,
            "confirm": self.confirm.is_some(),
        })
    }

    /// Debug driver: types text, runs an action by name, or saves (`AGENTTY_DEBUG` only).
    pub fn debug(&mut self, command: &str, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            // Typed key by key, `\n` as Enter (auto-indent and all).
            "type" => {
                let text = argument.replace("\\n", "\n");
                self.edit(cx, |b| {
                    for c in text.chars() {
                        if c == '\n' {
                            b.newline();
                        } else {
                            b.insert(&c.to_string());
                        }
                    }
                });
            }
            "goto" => {
                let mut parts = argument.split(':').map(|p| p.parse::<usize>().unwrap_or(1));
                let (line, column) = (parts.next().unwrap_or(1), parts.next().unwrap_or(1));
                self.motion(cx, |b| b.move_to(Pos::new(line.saturating_sub(1), column.saturating_sub(1)), false));
            }
            "save" => {
                self.save(false, cx);
            }
            "format" => self.format(cx),
            "undo" => self.edit(cx, |b| {
                b.undo();
            }),
            "close" => self.request_close(self.active, cx),
            // What an input method sends: `mark:ㅎ`, `commit:한`, `unmark`.
            "ime" => {
                use gpui::EntityInputHandler;
                let (kind, text) = argument.split_once(':').unwrap_or((argument, ""));
                match kind {
                    "commit" => self.replace_text_in_range(None, text, window, cx),
                    "unmark" => self.unmark_text(window, cx),
                    _ => self.replace_and_mark_text_in_range(None, text, None, window, cx),
                }
            }
            "external" => self.open_externally(cx),
            "answer" => self.answer(argument.parse().unwrap_or(2), window, cx),
            _ => {}
        }
    }
}

/// The range a click selects: the character position, the word or the whole line.
fn unit_range(buffer: &Buffer, unit: DragUnit, pos: Pos) -> (Pos, Pos) {
    match unit {
        DragUnit::Char => (pos, pos),
        DragUnit::Word => buffer.word_at(pos),
        DragUnit::Line => {
            let end = if pos.line + 1 < buffer.line_count() {
                Pos::new(pos.line + 1, 0)
            } else {
                Pos::new(pos.line, buffer.line(pos.line).len())
            };
            (Pos::new(pos.line, 0), end)
        }
    }
}

fn highlights_for(language: Language, path: &Path, first_line: &str) -> Highlights {
    let grammar = highlight::syntaxes().and_then(|set| highlight::grammar_for(set, language, path, first_line));
    Highlights::new(grammar)
}

fn load_document(path: &Path, project: &Path) -> Document {
    let language = Language::detect(path);
    let mut doc = Document {
        path: path.to_path_buf(),
        target: path.to_path_buf(),
        project: project.to_path_buf(),
        content: Content::Text,
        buffer: Buffer::new(""),
        language,
        highlights: Highlights::new(None),
        format: file::Format::default(),
        read_only: None,
        allow_outside: false,
        stamp: None,
        scroll: Point::default(),
        autoscroll: false,
        notice: None,
        formatting: false,
        disk_changed: false,
        checking: false,
    };
    match file::open(path, project) {
        Opened::Text(file) => {
            doc.buffer = Buffer::new(&file.text);
            // Big files stay uncolored: coloring them would cost more than it helps.
            if file.read_only != Some(ReadOnly::Large) {
                doc.highlights = highlights_for(language, path, doc.buffer.line(0));
            }
            doc.target = file.target;
            doc.format = file.format;
            doc.read_only = file.read_only;
            doc.stamp = file.stamp;
        }
        Opened::Binary => doc.content = Content::Binary,
        Opened::TooLarge(size) => doc.content = Content::TooLarge(size),
        Opened::Failed(error) => doc.content = Content::Failed(error),
    }
    doc
}

impl gpui::EntityInputHandler for CodeEditor {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let doc = self.docs.get(self.active)?;
        let (start, end) = (doc.buffer.pos_at_utf16(range.start), doc.buffer.pos_at_utf16(range.end));
        adjusted.replace(doc.buffer.utf16_offset(start)..doc.buffer.utf16_offset(end));
        Some(doc.buffer.slice(start, end))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<gpui::UTF16Selection> {
        let doc = self.docs.get(self.active)?;
        let selection = doc.buffer.selection;
        let (start, end) = selection.range();
        Some(gpui::UTF16Selection {
            range: doc.buffer.utf16_offset(start)..doc.buffer.utf16_offset(end),
            reversed: selection.head < selection.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        let doc = self.docs.get(self.active)?;
        let (start, end) = doc.buffer.marked()?;
        Some(doc.buffer.utf16_offset(start)..doc.buffer.utf16_offset(end))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |buffer| buffer.unmark());
    }

    fn replace_text_in_range(&mut self, range: Option<std::ops::Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |buffer| {
            let (start, end) = match range {
                Some(range) => (buffer.pos_at_utf16(range.start), buffer.pos_at_utf16(range.end)),
                None => buffer.marked().unwrap_or_else(|| buffer.selection.range()),
            };
            buffer.commit(start, end, text);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        selected: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The selection inside the new text comes in UTF-16 units.
        let to_bytes = |units: usize| {
            let mut count = 0;
            for (byte, c) in text.char_indices() {
                if count >= units {
                    return byte;
                }
                count += c.len_utf16();
            }
            text.len()
        };
        let selected = selected.map(|r| to_bytes(r.start)..to_bytes(r.end));
        self.edit(cx, |buffer| {
            let (start, end) = match range {
                Some(range) => (buffer.pos_at_utf16(range.start), buffer.pos_at_utf16(range.end)),
                None => buffer.marked().unwrap_or_else(|| buffer.selection.range()),
            };
            buffer.compose(start, end, text, selected);
        });
    }

    fn bounds_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        _: gpui::Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<gpui::Bounds<Pixels>> {
        let doc = self.docs.get(self.active)?;
        let layout = self.layout.as_ref()?;
        let (start, end) = (doc.buffer.pos_at_utf16(range.start), doc.buffer.pos_at_utf16(range.end));
        layout.bounds_for(start, end, &doc.buffer)
    }

    fn character_index_for_point(&mut self, point: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let doc = self.docs.get(self.active)?;
        let layout = self.layout.as_ref()?;
        Some(doc.buffer.utf16_offset(layout.position_for(point, &doc.buffer)))
    }
}

impl Render for CodeEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(doc) = self.docs.get(self.active) else {
            return div().id("code-editor").size_full().bg(hex(Chrome::EDITOR));
        };
        let body = if matches!(doc.content, Content::Text) {
            div()
                .id("editor-text")
                .flex_1()
                .min_h_0()
                .relative()
                .cursor(gpui::CursorStyle::IBeam)
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(Self::mouse_down))
                .on_mouse_move(cx.listener(Self::mouse_move))
                .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::mouse_up))
                .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(Self::mouse_up))
                .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                .child(element::EditorElement::new(cx.entity()))
                .into_any_element()
        } else {
            self.render_placeholder(doc, cx).into_any_element()
        };
        div()
            .id("code-editor")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(hex(Chrome::EDITOR))
            .on_action(cx.listener(|this, _: &MoveLeft, _, cx| this.motion(cx, |b| b.left(false, false))))
            .on_action(cx.listener(|this, _: &MoveRight, _, cx| this.motion(cx, |b| b.right(false, false))))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.motion(cx, |b| b.vertical(-1, false))))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.motion(cx, |b| b.vertical(1, false))))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.motion(cx, |b| b.left(true, false))))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.motion(cx, |b| b.right(true, false))))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.motion(cx, |b| b.vertical(-1, true))))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.motion(cx, |b| b.vertical(1, true))))
            .on_action(cx.listener(|this, _: &WordLeft, _, cx| this.motion(cx, |b| b.left(false, true))))
            .on_action(cx.listener(|this, _: &WordRight, _, cx| this.motion(cx, |b| b.right(false, true))))
            .on_action(cx.listener(|this, _: &SelectWordLeft, _, cx| this.motion(cx, |b| b.left(true, true))))
            .on_action(cx.listener(|this, _: &SelectWordRight, _, cx| this.motion(cx, |b| b.right(true, true))))
            .on_action(cx.listener(|this, _: &LineStart, _, cx| this.motion(cx, |b| b.home(false))))
            .on_action(cx.listener(|this, _: &LineEnd, _, cx| this.motion(cx, |b| b.line_end(false))))
            .on_action(cx.listener(|this, _: &SelectLineStart, _, cx| this.motion(cx, |b| b.home(true))))
            .on_action(cx.listener(|this, _: &SelectLineEnd, _, cx| this.motion(cx, |b| b.line_end(true))))
            .on_action(cx.listener(|this, _: &DocStart, _, cx| this.motion(cx, |b| b.doc_start(false))))
            .on_action(cx.listener(|this, _: &DocEnd, _, cx| this.motion(cx, |b| b.doc_end(false))))
            .on_action(cx.listener(|this, _: &SelectDocStart, _, cx| this.motion(cx, |b| b.doc_start(true))))
            .on_action(cx.listener(|this, _: &SelectDocEnd, _, cx| this.motion(cx, |b| b.doc_end(true))))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                let lines = this.page_lines();
                this.motion(cx, |b| b.vertical(-lines, false))
            }))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                let lines = this.page_lines();
                this.motion(cx, |b| b.vertical(lines, false))
            }))
            .on_action(cx.listener(|this, _: &SelectPageUp, _, cx| {
                let lines = this.page_lines();
                this.motion(cx, |b| b.vertical(-lines, true))
            }))
            .on_action(cx.listener(|this, _: &SelectPageDown, _, cx| {
                let lines = this.page_lines();
                this.motion(cx, |b| b.vertical(lines, true))
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.motion(cx, |b| b.select_all())))
            .on_action(cx.listener(|this, _: &Backspace, _, cx| this.edit(cx, |b| b.backspace())))
            .on_action(cx.listener(|this, _: &Delete, _, cx| this.edit(cx, |b| b.delete_forward())))
            .on_action(cx.listener(|this, _: &DeleteWordBack, _, cx| this.edit(cx, |b| b.delete_word_back())))
            .on_action(cx.listener(|this, _: &Newline, _, cx| this.edit(cx, |b| b.newline())))
            .on_action(cx.listener(|this, _: &Indent, _, cx| this.edit(cx, |b| b.indent())))
            .on_action(cx.listener(|this, _: &Outdent, _, cx| this.edit(cx, |b| b.outdent())))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.edit(cx, |b| {
                    b.undo();
                })
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.edit(cx, |b| {
                    b.redo();
                })
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                if let Some(doc) = this.docs.get(this.active) {
                    cx.write_to_clipboard(ClipboardItem::new_string(copied_text(&doc.buffer)));
                }
            }))
            .on_action(cx.listener(|this, _: &Cut, _, cx| {
                let Some(doc) = this.docs.get(this.active).filter(|d| d.editable()) else { return };
                cx.write_to_clipboard(ClipboardItem::new_string(copied_text(&doc.buffer)));
                this.edit(cx, |b| {
                    if b.selection.is_empty() {
                        // Nothing selected: the whole line, like other editors.
                        let (start, end) = unit_range(b, DragUnit::Line, b.cursor());
                        b.replace(start, end, "", buffer::EditKind::Other);
                    } else {
                        b.delete_selection();
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    this.edit(cx, |b| b.paste(&text));
                }
            }))
            .on_action(cx.listener(|this, _: &Save, _, cx| {
                this.save(false, cx);
            }))
            .on_action(cx.listener(|this, _: &Format, _, cx| this.format(cx)))
            .on_action(cx.listener(|this, _: &CloseFile, _, cx| this.request_close(this.active, cx)))
            .child(self.render_toolbar(doc, cx))
            .children(self.render_notice(doc, cx))
            .child(body)
            .child(self.render_status(doc, cx))
            .children(self.render_confirm(cx))
    }
}

/// A tooltip with the editor's shortcut (a text field's: ⌘S is Ctrl+S on Windows and Linux).
pub fn with_shortcut(text: String, keys: &str) -> String {
    format!("{text}   {}", crate::keymap::display_in_text(keys))
}

/// What copy takes: the selection, or the whole line (with its line break) when nothing is selected.
fn copied_text(buffer: &Buffer) -> String {
    if buffer.selection.is_empty() {
        format!("{}\n", buffer.line(buffer.cursor().line))
    } else {
        buffer.selected_text()
    }
}
