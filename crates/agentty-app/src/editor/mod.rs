//! File editor: files opened from the files panel, shown in place of the terminals with a tab each
//! in the tab strip. Syntax colors, editing with undo, saving, formatting with the installed
//! formatter and a button that hands the file to VS Code (or the system's editor).
//!
//! Files are only read and written here — never run (see `file.rs`, `format.rs`, `external.rs`).

pub mod agent;
pub mod buffer;
mod complete;
mod element;
mod emmet;
pub mod external;
pub mod file;
mod find;
mod fold;
pub mod format;
mod gitmarks;
pub mod highlight;
mod hover;
mod indent;
pub mod language;
mod lsp;
pub mod navigate;
mod preview;
mod problems;
pub mod project_index;
mod quickfix;
mod rename;
pub mod review;
mod signature;
mod spell;
mod sticky;
pub mod symbols;

use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, Tooltip, TypeScale};
use buffer::{Buffer, Pos, Selection};
use file::{Opened, ReadOnly, Stamp};
use futures::StreamExt;
use gpui::{
    actions, div, prelude::*, px, App, ClickEvent, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, FontWeight, Pixels, Point,
    Window,
};
use highlight::Highlights;
use language::{Formatter, Language};
use preview::{PageReply, Preview};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
        TogglePreview,
        TogglePreviewBeside,
        AskAgent,
        ToggleComment,
        MoveLinesUp,
        MoveLinesDown,
        DuplicateLinesUp,
        DuplicateLinesDown,
        DeleteLines,
        DeleteWordForward,
        DeleteToLineStart,
        SelectLine,
        InsertLineBelow,
        InsertLineAbove,
        IndentLines,
        OutdentLines,
        SelectNextOccurrence,
        SelectAllOccurrences,
        AddCursorAbove,
        AddCursorBelow,
        GoToBracket,
        Fold,
        Unfold,
        FoldAll,
        UnfoldAll,
    ]
);

pub const CONTEXT: &str = "CodeEditor";
/// Files remembered in the order they were in front, at most.
const RECENT_TABS: usize = 64;
/// How often open files are checked for changes made by others (agents, git, other editors).
const DISK_CHECK: Duration = Duration::from_secs(2);
/// How often the markdown preview catches up with edits and looks for clicked links.
const PREVIEW_TICK: Duration = Duration::from_millis(150);

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
        // VS Code's "Open Preview".
        key("cmd-shift-v", TogglePreview, c),
        // VS Code's "Open Preview to the Side" is ⌘K V, and ⌘K is the agent's here.
        key("cmd-alt-v", TogglePreviewBeside, c),
        // Cursor's and Kiro's "edit with AI".
        key("cmd-k", AskAgent, c),
        // VS Code's line keys.
        key("cmd-/", ToggleComment, c),
        key("alt-up", MoveLinesUp, c),
        key("alt-down", MoveLinesDown, c),
        key("shift-alt-up", DuplicateLinesUp, c),
        key("shift-alt-down", DuplicateLinesDown, c),
        key("cmd-shift-k", DeleteLines, c),
        key("cmd-l", SelectLine, c),
        key("cmd-enter", InsertLineBelow, c),
        // As in VS Code: in the editor these win over the app's split, pane and zoom keys.
        key("cmd-shift-enter", InsertLineAbove, c),
        key("cmd-]", IndentLines, c),
        key("cmd-[", OutdentLines, c),
        key("cmd-d", SelectNextOccurrence, c),
        key("cmd-shift-l", SelectAllOccurrences, c),
        key("cmd-alt-up", AddCursorAbove, c),
        key("cmd-alt-down", AddCursorBelow, c),
        key("cmd-shift-\\", GoToBracket, c),
        key("cmd-alt-[", Fold, c),
        key("cmd-alt-]", Unfold, c),
        // VS Code's are ⌘K ⌘0 and ⌘K ⌘J, and ⌘K is the agent's here: ⇧ with the keys above.
        key("cmd-alt-shift-[", FoldAll, c),
        key("cmd-alt-shift-]", UnfoldAll, c),
    ]);
    find::bind_keys(cx);
    navigate::bind_keys(cx);
    agent::bind_keys(cx);
    review::bind_keys(cx);
    problems::bind_keys(cx);
    quickfix::bind_keys(cx);
    rename::bind_keys(cx);
    complete::bind_keys(cx);
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
            key("alt-delete", DeleteWordForward, c),
            key("cmd-backspace", DeleteToLineStart, c),
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
            gpui::KeyBinding::new("ctrl-delete", DeleteWordForward, c),
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
    /// A web link clicked in the markdown preview.
    OpenUrl(String),
    /// A file linked from the markdown preview: open it (the path, and the project it belongs to).
    OpenFile(PathBuf, PathBuf),
    /// An HTML file to see as a page, in the in-app browser.
    ShowInBrowser(PathBuf),
    /// An instruction about some lines, for the linked session (the prompt as it is sent).
    AskAgent(String),
    /// Show or hide the linked session beside the editor (or pick one when none is linked).
    ToggleSession,
    /// Install the language server of an LSP language id (the status bar's "Install …").
    InstallLanguageServer(&'static str),
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
    /// The color of the agent whose changes in this file wait for review.
    pub review: Option<u32>,
    /// The color of the worst error or warning its language server reports in it.
    pub problem: Option<u32>,
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
    /// A picture, shown fitted to the editor (never larger than it is).
    Image(std::sync::Arc<gpui::Image>),
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
    /// Saved once the formatting under way is done (format on save): quietly, whatever it gives.
    save_after_format: bool,
    /// Changed on disk while it had unsaved changes here (asked about in a notice).
    disk_changed: bool,
    /// Checking the disk is under way.
    checking: bool,
    /// Markdown shown rendered instead of as text.
    preview: bool,
    /// Markdown shown rendered beside the text.
    beside: bool,
    /// Bring the cursor to the upper third of the view on the next paint (after a jump).
    center: bool,
    /// Scroll this line into view on the next paint, the cursor staying where it is.
    scroll_to_line: Option<usize>,
    /// The text as it is on disk (when read or saved last), what unsaved edits are made against:
    /// an agent's write to the file merges with them when they touch different lines.
    base: String,
    /// The file's symbols as of a text revision (the breadcrumb reads them on every paint).
    outline: std::cell::RefCell<Option<(u64, std::sync::Arc<Vec<symbols::Symbol>>)>>,
    /// What its language server reports in it.
    checks: problems::Checks,
    /// Its lines against the last commit.
    git: gitmarks::GitMarks,
    /// Regions folded away.
    folds: fold::Folds,
}

impl Document {
    /// The selection in UTF-16 units of the whole text, as the input method counts.
    fn selection_utf16(&self) -> std::ops::Range<usize> {
        let (start, end) = self.buffer.selection.range();
        self.buffer.utf16_offset(start)..self.buffer.utf16_offset(end)
    }

    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| self.path.display().to_string())
    }

    /// The file's symbols, read again only when its text changed.
    fn outline(&self) -> std::sync::Arc<Vec<symbols::Symbol>> {
        let revision = self.buffer.revision();
        if let Some((_, list)) = self.outline.borrow().as_ref().filter(|(at, _)| *at == revision) {
            return list.clone();
        }
        let list = std::sync::Arc::new(if symbols::supported(self.language) {
            symbols::symbols(self.language, &self.buffer.text())
        } else {
            Vec::new()
        });
        *self.outline.borrow_mut() = Some((revision, list.clone()));
        list
    }

    fn editable(&self) -> bool {
        matches!(self.content, Content::Text)
            && (self.read_only.is_none() || (self.allow_outside && self.read_only == Some(ReadOnly::OutsideProject)))
    }

    fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    /// Markdown text, which the preview can show rendered (on platforms with a web view).
    fn can_preview(&self) -> bool {
        self.language == Language::Markdown && matches!(self.content, Content::Text) && crate::platform::HAS_WEBVIEW
    }

    /// The text is out of sight: the page shows instead.
    fn previewing(&self) -> bool {
        self.preview && self.can_preview()
    }

    /// The page shows beside the text.
    fn previewing_beside(&self) -> bool {
        self.beside && !self.preview && self.can_preview()
    }

    /// Keeps what is kept by line in step with edits: syntax colors, and the problems and change
    /// marks (worked out again once typing pauses) move with their lines meanwhile instead of
    /// marking whatever line came to be there.
    fn sync_highlights(&mut self) {
        let Some(changes) = self.buffer.take_changes() else {
            self.highlights.invalidate_from(0);
            self.folds.keep_within(self.buffer.line_count());
            return;
        };
        if !changes.is_empty() {
            self.checks.edited_at = Some(Instant::now());
        }
        for splice in changes {
            self.highlights.apply(splice);
            self.folds.apply(splice);
            if splice.delta != 0 {
                for problem in &mut self.checks.problems {
                    problem.start.line = splice.line_after(problem.start.line);
                    problem.end.line = splice.line_after(problem.end.line);
                }
                for change in &mut self.git.marks {
                    change.line = splice.line_after(change.line);
                }
            }
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
                self.base = self.buffer.text();
                self.sync_highlights();
                self.format = file.format;
                self.stamp = file.stamp;
                self.target = file.target;
                self.read_only = file.read_only;
            }
            Opened::Image(image) => self.content = Content::Image(image),
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
    /// The markdown preview's web view (one, for whichever file is in front).
    preview: Preview,
    /// A markdown file a link in the preview asked for: it opens as a preview too, as in VS Code.
    preview_next: Option<PathBuf>,
    /// Where the preview's page answers arrive.
    preview_replies: futures::channel::mpsc::UnboundedSender<PageReply>,
    /// Blinking of the caret while the editor has focus.
    blink: crate::caret_blink::CaretBlink,
    /// The find bar (⌘F), while open.
    find: Option<find::FindBar>,
    /// The go-to box (⌘P and the lists of definitions and usages), while open.
    picker: Option<navigate::Picker>,
    /// The project the last go-to box searched: a file picked in it while nothing was open yet
    /// belongs to it (its language server, change marks and names come from there).
    picked_project: Option<PathBuf>,
    history: navigate::History,
    /// A place to show once its file is open: (path, position, bytes to select).
    pending_goto: Option<(PathBuf, Pos, usize, Option<String>)>,
    /// The column (in characters) the next `pending_goto` lands on, for places given that way.
    pending_column: Option<usize>,
    /// A short note under the cursor ("no definition found"), and since when.
    flash: Option<(String, std::time::Instant)>,
    /// Files shown lately, newest first (⌘P lists them first).
    recent: Vec<PathBuf>,
    /// Where the pointer was last over the text (⌘ pressed without moving still finds the link).
    pointer: Option<Point<Pixels>>,
    /// The pointer is over the gutter (where the fold arrows show).
    gutter_hover: bool,
    /// Agents' changes waiting to be kept or undone.
    changes: Vec<review::AgentChange>,
    next_change: u64,
    /// When the user last typed here: following an agent waits until they pause.
    last_typed: Option<std::time::Instant>,
    /// The name under the pointer while ⌘ is held, underlined as a link.
    link: Option<(Pos, Pos)>,
    /// Where that name is defined, once found: (the name, the code there).
    link_peek: Option<(String, Option<navigate::Preview>)>,
    /// The session the editor is linked to.
    linked: Option<agent::LinkedAgent>,
    /// Agents at work in open files.
    presence: Vec<agent::Presence>,
    /// Edits agents reported, waiting for the change to show on disk: (file, agent, when).
    expected_edits: Vec<(PathBuf, agent::LinkedAgent, std::time::Instant)>,
    presence_ticking: bool,
    /// Show the file and place the agent changes, as it changes them.
    follow_agent: bool,
    /// Changes read from disk that no agent claimed yet: (file, its text before, when).
    unclaimed: Vec<(PathBuf, String, std::time::Instant)>,
    /// Agents that announced edits here (their names and colors).
    agents_seen: Vec<agent::LinkedAgent>,
    /// A file opened to follow an agent: it does not take the keyboard.
    quiet_open: Option<PathBuf>,
    /// The instruction box for the selected lines, while open.
    ask: Option<agent::AskBox>,
    /// The last instruction sent, until the agent changes those lines.
    asked: Option<agent::Asked>,
    /// The change under review is shown as it was and as it is.
    review_diff: bool,
    /// The servers' reports as of this [`lsp::generation`].
    problems_generation: u64,
    /// The file that was in front at the last problems tick.
    problems_front: Option<PathBuf>,
    /// The servers turned off in Settings, as last handed to `agentty_bridge::lsp`.
    lsp_disabled: Option<Vec<String>>,
    lsp_trusted: Option<Vec<String>>,
    /// The files shown in front, the last first: closing the one in front goes back to the one
    /// before it, as VS Code does.
    recent_tabs: Vec<PathBuf>,
    /// The problem under the pointer, and whether its message shows yet.
    problem_hover: Option<(lsp::Problem, bool)>,
    /// The fixes of that problem, once the server said.
    hover_fixes: Option<(lsp::Problem, u64, Vec<lsp::Action>)>,
    /// The ⌘. list, while open.
    fixes: Option<quickfix::FixMenu>,
    /// Whether the cursor's line has fixes (the light bulb), and whether that is being asked.
    bulb: Option<quickfix::Bulb>,
    bulb_asking: bool,
    /// The F2 box, while open.
    rename: Option<rename::RenameBox>,
    /// The type of the name under the pointer.
    type_hover: Option<hover::TypeHover>,
    /// The pointer is on the problem's or the type's window (it stays open there).
    popup_over: bool,
    /// Counts the pointer leaving what a window is about: a window closes a moment later only if
    /// the pointer did not come back or go onto the window meanwhile.
    hover_leaves: u64,
    /// The change since the last commit whose window shows: (file, its first line).
    git_peek: Option<(PathBuf, usize)>,
    /// The completion list, while open, and the number of the last request for one.
    completion: Option<complete::Completion>,
    snippet: Option<complete::Snippet>,
    completion_seq: u64,
    /// The signature of the call being typed, and the number of the last request for one.
    signature: Option<signature::SignatureHint>,
    signature_seq: u64,
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
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(problems::TICK).await;
            if this
                .update(cx, |this, cx| {
                    this.problems_tick(cx);
                    this.bulb_tick(cx);
                    this.git_tick(cx);
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(PREVIEW_TICK).await;
            if this.update(cx, |this, cx| this.preview_tick(cx)).is_err() {
                break;
            }
        })
        .detach();
        let (preview_replies, mut replies) = futures::channel::mpsc::unbounded();
        cx.spawn(async move |this, cx| {
            while let Some(reply) = replies.next().await {
                if this.update(cx, |this, cx| this.preview_reply(reply, cx)).is_err() {
                    break;
                }
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
            preview: Preview::new(),
            blink: Default::default(),
            preview_next: None,
            preview_replies,
            find: None,
            picker: None,
            picked_project: None,
            history: Default::default(),
            pending_goto: None,
            pending_column: None,
            flash: None,
            recent: Vec::new(),
            pointer: None,
            gutter_hover: false,
            changes: Vec::new(),
            next_change: 0,
            last_typed: None,
            link: None,
            link_peek: None,
            linked: None,
            presence: Vec::new(),
            expected_edits: Vec::new(),
            presence_ticking: false,
            follow_agent: false,
            unclaimed: Vec::new(),
            agents_seen: Vec::new(),
            quiet_open: None,
            ask: None,
            asked: None,
            review_diff: false,
            problems_generation: 0,
            problems_front: None,
            lsp_disabled: None,
            lsp_trusted: None,
            recent_tabs: Vec::new(),
            problem_hover: None,
            hover_fixes: None,
            fixes: None,
            bulb: None,
            bulb_asking: false,
            rename: None,
            type_hover: None,
            popup_over: false,
            hover_leaves: 0,
            git_peek: None,
            completion: None,
            snippet: None,
            completion_seq: 0,
            signature: None,
            signature_seq: 0,
        }
    }

    // -- files ------------------------------------------------------------------------------

    /// Opens `path` (from the project `project`), or shows it when it is already open. The file is
    /// read on a background thread: a click in the files panel must not stop the window — the file
    /// can be on a network volume, an undownloaded cloud placeholder, or simply large.
    pub fn open(&mut self, path: &Path, project: &Path, window: &mut Window, cx: &mut Context<Self>) {
        // Opened to follow an agent: shown, but the keyboard stays where the user is typing.
        let quiet = self.quiet_open.as_deref() == Some(path);
        if let Some(index) = self.docs.iter().position(|d| d.path == path) {
            self.quiet_open = None;
            self.active = index;
            if self.preview_next.take().as_deref() == Some(path) {
                self.docs[index].preview = true;
            }
            if !quiet {
                window.focus(&self.focus_handle);
            }
            self.tabs_changed(cx);
            self.apply_pending_goto(cx);
            return cx.notify();
        }
        let (path, project) = (path.to_path_buf(), project.to_path_buf());
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let (read, of) = (path.clone(), project.clone());
            let opened = cx.background_spawn(async move { file::open(&read, &of) }).await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    // Asked for twice while it was read: show the one that is already open.
                    match this.docs.iter().position(|d| d.path == path) {
                        Some(index) => this.active = index,
                        None => {
                            this.docs.push(document(&path, &project, opened));
                            this.active = this.docs.len() - 1;
                            // What the servers reported already (a build file's error) is read
                            // for it at the next tick.
                            this.problems_generation = this.problems_generation.wrapping_sub(1);
                        }
                    }
                    if this.preview_next.take().as_deref() == Some(path.as_path()) {
                        this.docs[this.active].preview = true;
                    }
                    if this.quiet_open.take().as_deref() != Some(path.as_path()) {
                        window.focus(&this.focus_handle);
                    }
                    this.resolve_presence(&path);
                    this.tabs_changed(cx);
                    this.apply_pending_goto(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// [`Self::open`] with the cursor at `line:column` (from 1, the column counted in characters)
    /// once the file is there: a place clicked in a terminal (`src/app.ts:42:7`).
    pub fn open_at(&mut self, path: &Path, project: &Path, place: Option<(usize, usize)>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((line, column)) = place {
            self.pending_column = Some(column.saturating_sub(1));
            self.pending_goto = Some((path.to_path_buf(), Pos::new(line.saturating_sub(1), 0), 0, None));
        } else {
            self.pending_column = None;
        }
        self.open(path, project, window, cx);
    }

    /// Replaces the text of the document being edited. For a host that drives the editor as an
    /// input field (the SQL query box); a file editor never has its text set from outside.
    pub fn set_active_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let end = doc.buffer.end();
        doc.buffer.replace(buffer::Pos::default(), end, text, buffer::EditKind::Other);
        doc.autoscroll = true;
        doc.sync_highlights();
        cx.notify();
    }

    /// Text of the document being edited, for a host that uses the editor as an input (the SQL
    /// query box) rather than as a file editor.
    pub fn active_text(&self) -> Option<String> {
        self.docs.get(self.active).map(|doc| doc.buffer.text())
    }

    pub fn tabs(&self) -> Vec<TabInfo> {
        self.docs
            .iter()
            .enumerate()
            .map(|(index, doc)| TabInfo {
                name: doc.name(),
                path: doc.path.clone(),
                dirty: doc.is_dirty(),
                active: index == self.active,
                review: self.review_color(&doc.path),
                problem: doc.checks.problems.iter().map(|p| p.severity).filter(|s| *s <= lsp::Severity::Warning).min().map(problems::color),
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// The project of the file in front.
    pub fn active_project(&self) -> Option<&Path> {
        self.docs.get(self.active).map(|d| d.project.as_path())
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
            // The instruction box belongs to lines of the file it was opened on.
            self.ask = None;
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

    /// Closes every file but tab `keep`'s; an unsaved one is asked about (the first, then the
    /// others stay open).
    pub fn close_others(&mut self, keep: usize, cx: &mut Context<Self>) {
        let Some(kept) = self.docs.get(keep).map(|d| d.path.clone()) else { return };
        let mut index = self.docs.len();
        while index > 0 {
            index -= 1;
            if self.docs[index].path != kept && !self.docs[index].is_dirty() {
                self.close(index, cx);
            }
        }
        match self.docs.iter().position(|d| d.path != kept) {
            Some(dirty) => self.request_close(dirty, cx),
            None => {
                self.active = 0;
                self.tabs_changed(cx);
                cx.notify();
            }
        }
    }

    fn close(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.docs.len() {
            return;
        }
        let was_active = index == self.active;
        let closed = self.docs.remove(index);
        let (project, language, path) = (closed.project.clone(), closed.language, closed.path.clone());
        cx.background_spawn(async move { lsp::close(&project, language, &path) }).detach();
        self.changes.retain(|c| c.path != closed.path);
        self.presence.retain(|p| p.path != closed.path || p.pending);
        self.recent_tabs.retain(|p| *p != closed.path);
        if self.active > index || self.active >= self.docs.len() {
            self.active = self.active.saturating_sub(1);
        }
        // The file in front closed: the one shown before it comes back.
        if was_active {
            let before = self.recent_tabs.iter().find_map(|p| self.docs.iter().position(|d| d.path == *p));
            if let Some(before) = before {
                self.active = before;
            }
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
        // The language server for the file in front starts now, to have read the project by
        // the time it is asked something.
        if let Some(doc) = self.docs.get(self.active) {
            lsp::warm(&doc.project, doc.language, &doc.path);
        }
        if let Some(path) = self.docs.get(self.active).map(|d| d.path.clone()) {
            if self.recent.first() != Some(&path) {
                self.recent.retain(|p| *p != path);
                self.recent.insert(0, path);
                self.recent.truncate(50);
            }
        }
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
        // The text is out of sight while the preview shows: keys must not change it unseen.
        if !doc.editable() || doc.previewing() {
            return;
        }
        let was_dirty = doc.is_dirty();
        f(&mut doc.buffer);
        doc.sync_highlights();
        self.last_typed = Some(std::time::Instant::now());
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        doc.autoscroll = true;
        if doc.is_dirty() != was_dirty {
            self.tabs_changed(cx);
        }
        cx.notify();
    }

    /// ⌘D (the next place with the selected text) or ⇧⌘L (every place) gets a cursor.
    fn add_cursors(&mut self, all: bool, cx: &mut Context<Self>) {
        self.completion = None;
        self.signature = None;
        let Some(doc) = self.doc() else { return };
        doc.buffer.unmark();
        if all {
            doc.buffer.select_all_occurrences();
        } else {
            doc.buffer.add_next_occurrence();
        }
        doc.autoscroll = true;
        cx.notify();
    }

    /// ↑ / ↓ (or a page) at every cursor, over the folded lines.
    fn vertical(&mut self, rows: isize, select: bool, cx: &mut Context<Self>) {
        let hidden = self.docs.get(self.active).map(|d| d.folds.hidden().to_vec()).unwrap_or_default();
        self.motion(cx, move |b| b.vertical_in(rows, select, &hidden));
    }

    /// ⌥⌘[: folds the innermost region the cursor is in (one not folded yet); ⌥⌘]: unfolds it.
    fn fold_at_cursor(&mut self, fold: bool, cx: &mut Context<Self>) {
        let Some(doc) = self.doc() else { return };
        let cursor = doc.buffer.cursor();
        if !fold {
            if doc.folds.unfold_around(cursor.line) {
                cx.notify();
            }
            return;
        }
        let count = doc.buffer.line_count();
        let folded = doc.folds.ranges().to_vec();
        let found = fold::enclosing(|i| doc.buffer.line(i), cursor.line, count, |region| folded.contains(&region));
        let Some((start, end)) = found else { return };
        doc.folds.fold(start, end);
        // The cursor goes onto the line left in view.
        doc.buffer.single_cursor();
        if cursor.line != start {
            doc.buffer.move_to(doc.buffer.clamp(Pos::new(start, cursor.col)), false);
        }
        doc.autoscroll = true;
        cx.notify();
    }

    /// ⇧⌥⌘[: folds every region of the file, nested ones too; ⇧⌥⌘]: unfolds them all.
    fn fold_all(&mut self, fold: bool, cx: &mut Context<Self>) {
        let Some(doc) = self.doc() else { return };
        if !fold {
            if !doc.folds.is_empty() {
                doc.folds.unfold_all();
                doc.autoscroll = true;
                cx.notify();
            }
            return;
        }
        let regions = fold::all_regions(|i| doc.buffer.line(i), doc.buffer.line_count());
        if regions.is_empty() {
            return;
        }
        doc.folds.fold_all(regions);
        // The cursor goes onto the line left in view of the region it was in.
        let cursor = doc.buffer.cursor();
        if let Some(&(from, _)) = doc.folds.hidden().iter().find(|&&(from, to)| from <= cursor.line && cursor.line <= to) {
            doc.buffer.single_cursor();
            let line = from - 1;
            doc.buffer.move_to(Pos::new(line, doc.buffer.line(line).len()), false);
        } else if doc.buffer.selections().len() > 1 {
            doc.buffer.single_cursor();
        }
        doc.autoscroll = true;
        cx.notify();
    }

    /// A click on a fold arrow: the region starting at `line` folds or unfolds. False when none
    /// starts there.
    fn toggle_fold(&mut self, line: usize, cx: &mut Context<Self>) -> bool {
        let Some(doc) = self.doc() else { return false };
        if !doc.folds.unfold(line) {
            let Some(end) = fold::region_end(&|i| doc.buffer.line(i), line, doc.buffer.line_count()) else { return false };
            doc.folds.fold(line, end);
            // Cursors in the lines folded away go to its first line.
            let hidden = |pos: Pos| pos.line > line && pos.line <= end;
            if doc.buffer.selections().iter().any(|s| hidden(s.head) || hidden(s.anchor)) {
                doc.buffer.single_cursor();
                doc.buffer.move_to(Pos::new(line, doc.buffer.line(line).len()), false);
            }
        }
        cx.notify();
        true
    }

    fn add_cursor_vertical(&mut self, up: bool, cx: &mut Context<Self>) {
        self.completion = None;
        self.signature = None;
        let Some(doc) = self.doc() else { return };
        doc.buffer.unmark();
        doc.buffer.add_cursor_vertical(up);
        doc.autoscroll = true;
        cx.notify();
    }

    /// `>` typed in markup or JSX: the tag it ends gets its closing tag.
    fn close_typed_tag(&mut self, cx: &mut Context<Self>) {
        let Some(language) = self.docs.get(self.active).map(|d| d.language) else { return };
        let html = matches!(language, Language::Html);
        if html || matches!(language, Language::Tsx | Language::Jsx) {
            self.edit(cx, |b| {
                b.close_tag(html);
            });
        }
    }

    /// An edit that is not typing (moving lines): the completion list and the parameter hint,
    /// placed by the text as it was, close first.
    fn dismiss_and_edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer)) {
        self.completion = None;
        self.signature = None;
        self.edit(cx, f);
    }

    /// Moves the cursor (works in read-only files too).
    fn motion(&mut self, cx: &mut Context<Self>, f: impl Fn(&mut Buffer)) {
        self.completion = None;
        self.signature = None;
        let Some(doc) = self.doc() else { return };
        doc.buffer.unmark();
        doc.buffer.each(f);
        doc.sync_highlights();
        doc.autoscroll = true;
        self.snippet_follow();
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
                doc.base = doc.buffer.text();
                doc.stamp = Stamp::of(&doc.target);
                doc.disk_changed = false;
                doc.notice = None;
                let (project, path) = (doc.project.clone(), doc.path.clone());
                cx.background_spawn(async move { lsp::saved(&project, &path) }).detach();
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

    /// ⌘S: with format on save on, formats the file first (when its language has a formatter),
    /// then saves it.
    fn save_formatted(&mut self, cx: &mut Context<Self>) {
        let format = crate::settings::settings(cx).editor_format_on_save;
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if format && doc.editable() && doc.language.formatter().is_some() && matches!(doc.content, Content::Text) {
            if !doc.formatting {
                doc.save_after_format = true;
                self.format(cx);
            }
            return;
        }
        self.save(false, cx);
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
        let (path, project, language) = (doc.path.clone(), doc.project.clone(), doc.language);
        let snapshot = lsp::Snapshot::of(&doc.buffer);
        let indent = doc.buffer.indent_unit().to_string();
        let task = cx.background_spawn(async move {
            let text = snapshot.text.clone();
            let result = format::format(formatter, &path, &project, &text);
            // The formatter is not installed: the language server formats instead, when one runs.
            if let Err(format::FormatError::Missing { .. }) = &result {
                if let (Some(formatted), Some(server)) =
                    (lsp::format(&project, language, &path, &snapshot, &indent), lsp::server_name(language))
                {
                    return (path, text, server.to_string(), Ok(formatted));
                }
            }
            (path, text, formatter.program().to_string(), result)
        });
        cx.spawn(async move |this, cx| {
            let (path, before, tool, result) = task.await;
            let _ = this.update(cx, |this, cx| this.formatted(&path, &before, &tool, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn formatted(&mut self, path: &Path, before: &str, program: &str, result: Result<String, format::FormatError>, cx: &mut Context<Self>) {
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
        // Format on save: the formatting goes in when it can, no word about it, and the file is saved.
        if std::mem::take(&mut doc.save_after_format) {
            if let Some(formatted) = result.as_ref().ok().filter(|f| doc.buffer.text() == before && f.as_str() != before) {
                doc.buffer.set_formatted(formatted);
                doc.sync_highlights();
            }
            let active = std::mem::replace(&mut self.active, index);
            self.save(false, cx);
            self.active = active;
            return cx.notify();
        }
        match result {
            Ok(formatted) => {
                if doc.buffer.text() != before {
                    doc.set_notice(messages.2, Tone::Warning);
                } else if formatted == before {
                    doc.set_notice(messages.1, Tone::Success);
                } else {
                    doc.buffer.set_formatted(&formatted);
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
        self.open_tab_externally(self.active, cx);
    }

    /// The file of tab `index` in VS Code, Cursor or the system's editor (the file tab's menu).
    pub fn open_tab_externally(&mut self, index: usize, cx: &mut Context<Self>) {
        let preference = crate::settings::settings(cx).external_editor;
        let Some(doc) = self.docs.get(index) else { return };
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
                            let (before, selection) = (doc.buffer.text(), doc.buffer.selection);
                            doc.reload(opened);
                            doc.stamp = stamp;
                            let path = doc.path.clone();
                            this.note_disk_change(&path, &before, selection, cx);
                        }
                        // Unsaved edits here and a write there (an agent's): both kept when they
                        // touch different lines, as one step ⌘Z can take back.
                        Some(Opened::Text(file))
                            if !doc.disk_changed
                                && review::merge(&doc.base, &doc.buffer.text(), &Buffer::new(&file.text).text()).is_some() =>
                        {
                            let theirs = Buffer::new(&file.text).text();
                            let merged = review::merge(&doc.base, &doc.buffer.text(), &theirs).unwrap_or_default();
                            let (before, selection) = (doc.buffer.text(), doc.buffer.selection);
                            doc.buffer.set_text(&merged);
                            doc.sync_highlights();
                            doc.base = theirs;
                            doc.stamp = stamp;
                            let path = doc.path.clone();
                            this.note_disk_change(&path, &before, selection, cx);
                            let note = t(cx, "editor.merged").to_string();
                            this.flash(note, cx);
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
        if action == NoticeAction::Reload {
            // Read off the UI thread, like opening: the file is on whatever volume it is on.
            let (path, project) = (doc.path.clone(), doc.project.clone());
            return cx
                .spawn(async move |this, cx| {
                    let (read, of) = (path.clone(), project.clone());
                    let opened = cx.background_spawn(async move { file::open(&read, &of) }).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(doc) = this.docs.iter_mut().find(|d| d.path == path) {
                            doc.reload(opened);
                            doc.disk_changed = false;
                            doc.notice = None;
                        }
                        this.tabs_changed(cx);
                        cx.notify();
                    });
                })
                .detach();
        }
        // Theirs is overwritten on the next save.
        doc.stamp = Stamp::of(&doc.target);
        doc.disk_changed = false;
        doc.notice = None;
        self.tabs_changed(cx);
        cx.notify();
    }

    // -- mouse ------------------------------------------------------------------------------

    fn mouse_down(&mut self, event: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.close_quick_fix(window, cx);
        self.completion = None;
        self.snippet = None;
        self.signature = None;
        if self.docs.get(self.active).is_some_and(Document::previewing_beside) {
            crate::webview::focus_gpui_view(window);
        }
        window.focus(&self.focus_handle);
        let Some(layout) = self.layout.as_ref() else { return };
        if layout.on_scrollbar(event.position) {
            let scroll = self.docs.get(self.active).map_or(0., |d| d.scroll.y);
            self.scrollbar_drag = Some((f32::from(event.position.y), scroll));
            return;
        }
        let Some(pos) = self.docs.get(self.active).map(|doc| layout.position_for(event.position, &doc.buffer)) else { return };
        let mark = layout.fold_mark_at(event.position);
        if layout.on_fold_arrow(event.position) && !layout.below_text(event.position) && self.toggle_fold(pos.line, cx) {
            return;
        }
        if let Some(line) = mark {
            self.toggle_fold(line, cx);
            return;
        }
        let Some(layout) = self.layout.as_ref() else { return };
        // A click on a change since the last commit shows it (with Revert).
        if layout.on_change_bar(event.position) && self.toggle_git_peek(pos.line, cx) {
            return;
        }
        let Some(layout) = self.layout.as_ref() else { return };
        let in_gutter = layout.in_gutter(event.position);
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        doc.buffer.unmark();
        let unit = match (in_gutter, event.click_count) {
            (true, _) | (_, 3..) => DragUnit::Line,
            (_, 2) => DragUnit::Word,
            _ => DragUnit::Char,
        };
        // ⌘-click: to the definition of what was clicked (or the file an import names).
        // ⌥-click: one more cursor there.
        if event.modifiers.alt && !event.modifiers.shift && !event.modifiers.secondary() && !in_gutter && event.click_count == 1 {
            doc.buffer.unmark();
            doc.buffer.toggle_cursor(pos);
            self.link = None;
            self.drag = None;
            return cx.notify();
        }
        if event.modifiers.secondary() && !event.modifiers.shift && !in_gutter && event.click_count == 1 {
            doc.buffer.move_to(pos, false);
            self.link = None;
            self.ask = None;
            return self.go_to_definition(window, cx);
        }
        self.ask = None;
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let (start, end) = unit_range(&doc.buffer, unit, pos, doc.folds.hidden());
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
            self.pointer = Some(event.position);
            let in_gutter = self.layout.as_ref().is_some_and(|l| l.in_gutter(event.position));
            if in_gutter != self.gutter_hover {
                self.gutter_hover = in_gutter;
                cx.notify();
            }
            let over = self.text_position(event.position);
            let hovered = over.filter(|_| event.modifiers.secondary());
            let plain = over.filter(|_| !event.modifiers.secondary() && !event.modifiers.alt && !event.modifiers.shift);
            self.hover_problem(plain, cx);
            self.hover_type(plain, cx);
            return self.hover_link(hovered, cx);
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
        let (start, end) = unit_range(&doc.buffer, unit, pos, doc.folds.hidden());
        if pos < origin_start {
            doc.buffer.selection = Selection { anchor: origin_end, head: start };
        } else {
            doc.buffer.selection = Selection { anchor: origin_start, head: end.max(origin_end) };
        }
        doc.autoscroll = true;
        cx.notify();
    }

    fn mouse_up(&mut self, _: &gpui::MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        // The selection bar shows once the drag is over.
        if self.drag.take().is_some() {
            cx.notify();
        }
        self.scrollbar_drag = None;
    }

    /// The text position under a point in the text (not in the gutter or on the scrollbar).
    fn text_position(&self, position: Point<Pixels>) -> Option<Pos> {
        let layout = self.layout.as_ref()?;
        if layout.in_gutter(position) || layout.on_scrollbar(position) {
            return None;
        }
        let doc = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text))?;
        Some(layout.position_for(position, &doc.buffer))
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

    // -- markdown preview -------------------------------------------------------------------

    /// Shows the file in front rendered, or back as text (markdown files only).
    fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(self.active).filter(|d| d.can_preview()) else { return };
        doc.buffer.unmark();
        doc.preview = !doc.preview;
        doc.beside = false;
        if doc.preview {
            crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "markdown_preview" }));
        } else {
            doc.autoscroll = true;
            self.preview.hide();
            // The page may have had the keyboard: the text takes it back.
            crate::webview::focus_gpui_view(window);
            window.focus(&self.focus_handle);
        }
        cx.notify();
    }

    /// Shows the file in front rendered beside its text, or no longer (markdown files only).
    fn toggle_preview_beside(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(self.active).filter(|d| d.can_preview()) else { return };
        doc.beside = !doc.previewing_beside();
        if doc.beside {
            crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "markdown_preview_beside" }));
        } else {
            self.preview.hide();
        }
        if std::mem::take(&mut doc.preview) {
            doc.autoscroll = true;
        }
        self.preview.synced = None;
        // The page may have had the keyboard: the text takes it back.
        crate::webview::focus_gpui_view(window);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Something of the editor's own covers the page: it stays out of sight meanwhile (a native
    /// view is drawn over everything). The go-to box is wider than the text beside the page.
    fn preview_hidden(&self) -> bool {
        self.preview.covered
            || self.confirm.is_some()
            || (self.picker.is_some() && self.docs.get(self.active).is_some_and(Document::previewing_beside))
    }

    /// Where the editor's pop-ups may go: inside the window, and left of the page beside the text.
    pub(super) fn popup_margin(&self) -> gpui::Edges<Pixels> {
        let (right, _) = self.preview.side.get();
        gpui::Edges { top: px(8.), bottom: px(8.), left: px(8.), right: px(8. + right) }
    }

    /// How wide the page beside the text is (0 without one): what lies over the text keeps left of it.
    pub(super) fn beside_width(&self) -> f32 {
        self.preview.side.get().1
    }

    /// The workbench says whether something of its own is drawn over the editor, or the editor is
    /// not on screen at all: the preview's native view would sit on top of it (or linger there).
    pub fn set_preview_covered(&mut self, covered: bool) {
        if covered {
            self.preview.hide();
        }
        self.preview.covered = covered;
    }

    /// Brings the page up to date with the text, and picks up links clicked in it.
    fn preview_tick(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active).filter(|d| d.previewing() || d.previewing_beside()) else { return };
        if self.preview_hidden() {
            return;
        }
        let view = self.preview.view.clone();
        let view = view.borrow();
        let Some(view) = view.as_ref() else { return };
        if view.is_loading() {
            return;
        }
        // ⌘W in the page closes the file, as it does from the text.
        let close = crate::webview::take_key_commands(&[view.id()]).iter().any(|(_, key)| *key == crate::webview::BrowserKey::CloseTab);
        let wanted = (doc.path.clone(), doc.buffer.revision());
        if self.preview.shown.as_ref() != Some(&wanted) && !self.preview.rendering {
            self.preview.rendering = true;
            let replies = self.preview_replies.clone();
            let (path, revision) = wanted.clone();
            view.call_async(
                "return window.agenttyRender ? window.agenttyRender(text) : null;",
                &[("text", &doc.buffer.text())],
                Box::new(move |result| {
                    let reply = match result {
                        Ok(json) if json != "null" => {
                            PageReply::Rendered { path, revision, images: serde_json::from_str(&json).unwrap_or_default() }
                        }
                        _ => PageReply::NotRendered,
                    };
                    let _ = replies.unbounded_send(reply);
                }),
            );
        }
        // Beside the text, the page follows the text's scrolling: the first line in sight is the
        // first in sight there.
        if doc.previewing_beside() && self.preview.shown.as_ref() == Some(&wanted) {
            if let Some(layout) = self.layout.as_ref().filter(|l| l.line_height > 0.) {
                let rows = doc.scroll.y / layout.line_height;
                let line = doc.folds.line_of(rows.floor() as usize) as f32 + rows.fract();
                let at = (wanted.0.clone(), wanted.1, (line * 100.).round() as i64);
                if self.preview.synced.as_ref() != Some(&at) {
                    self.preview.synced = Some(at);
                    view.call_async(
                        "return window.agenttyScrollTo ? window.agenttyScrollTo(line) : null;",
                        &[("line", &format!("{line:.2}"))],
                        Box::new(|_| {}),
                    );
                }
            }
        }
        if !self.preview.asking {
            self.preview.asking = true;
            let replies = self.preview_replies.clone();
            view.call_async(
                "return window.agenttyTakeLinks ? window.agenttyTakeLinks() : '[]';",
                &[],
                Box::new(move |result| {
                    let links = result.ok().and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default();
                    let _ = replies.unbounded_send(PageReply::Links(links));
                }),
            );
        }
        if close {
            let active = self.active;
            self.request_close(active, cx);
        }
    }

    fn preview_reply(&mut self, reply: PageReply, cx: &mut Context<Self>) {
        match reply {
            PageReply::NotRendered => self.preview.rendering = false,
            PageReply::Rendered { path, revision, images } => {
                self.preview.rendering = false;
                self.preview.shown = Some((path.clone(), revision));
                let Some(doc) = self.docs.iter().find(|d| d.path == path) else { return };
                if images.is_empty() {
                    return;
                }
                // Read off the UI thread, then handed to the page.
                let project = doc.project.clone();
                let read = cx.background_spawn(async move {
                    let found: serde_json::Map<String, serde_json::Value> = images
                        .into_iter()
                        .filter_map(|src| {
                            let file = preview::local_target(&src, &path, &project)?;
                            let url = preview::image_data_url(&file, &path, &project)?;
                            Some((src, serde_json::Value::String(url)))
                        })
                        .collect();
                    serde_json::Value::Object(found).to_string()
                });
                let view = self.preview.view.clone();
                cx.spawn(async move |_, _| {
                    let json = read.await;
                    if let Some(view) = view.borrow().as_ref() {
                        view.call_async("return window.agenttyImages(json);", &[("json", &json)], Box::new(|_| {}));
                    }
                })
                .detach();
            }
            PageReply::Links(links) => {
                self.preview.asking = false;
                let Some(doc) = self.docs.get(self.active) else { return };
                for link in links {
                    if preview::has_scheme(&link) {
                        let scheme = link.split(':').next().unwrap_or_default().to_ascii_lowercase();
                        if matches!(scheme.as_str(), "http" | "https" | "mailto") {
                            cx.emit(EditorEvent::OpenUrl(link));
                        }
                    } else if let Some(file) = preview::local_target(&link, &doc.path, &doc.project).filter(|f| f.is_file()) {
                        if Language::detect(&file) == Language::Markdown {
                            self.preview_next = Some(file.clone());
                        }
                        cx.emit(EditorEvent::OpenFile(file, doc.project.clone()));
                    }
                }
            }
        }
    }

    // -- rendering --------------------------------------------------------------------------

    fn render_toolbar(&self, doc: &Document, cx: &mut Context<Self>) -> impl IntoElement {
        let relative = doc.path.strip_prefix(&doc.project).map(Path::to_path_buf).unwrap_or_else(|_| doc.path.clone());
        let folder = relative.parent().map(|p| p.display().to_string()).filter(|p| !p.is_empty());
        let can_format = doc.editable() && !doc.formatting;
        // A narrow editor (the session beside it, a small window) keeps the buttons' icons only:
        // the file's name and where the cursor is matter more. Their tooltips say the rest.
        let narrow = self.layout.as_ref().is_some_and(|l| l.width() + self.beside_width() < 760.);
        let button = move |id: &'static str, glyph: &'static str, label: String, enabled: bool| {
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
                .when(!narrow, |d| d.child(label))
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
                    .children(self.render_breadcrumb(doc, cx))
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
            .when(doc.can_preview(), |d| {
                // Rendered as VS Code's preview renders it, or back to the text.
                let (glyph, label) = if doc.preview { ("pencil", t(cx, "editor.edit_text")) } else { ("eye", t(cx, "editor.preview")) };
                d.child(
                    button("editor-preview", glyph, label.to_string(), true)
                        .when(doc.preview, |d| d.bg(hex(Chrome::SELECTED)))
                        .tooltip(Tooltip::text(
                            with_shortcut(
                                t(cx, if doc.preview { "editor.edit_text_tip" } else { "editor.preview_tip" }).to_string(),
                                "⇧⌘V",
                            ),
                            None,
                        ))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_preview(window, cx))),
                )
                .child(
                    div()
                        .id("editor-preview-beside")
                        .flex_shrink_0()
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .when(doc.previewing_beside(), |d| d.bg(hex(Chrome::SELECTED)))
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .child(icon("columns-2", IconSize::INLINE, hex(Chrome::FOREGROUND)))
                        .tooltip(Tooltip::text(with_shortcut(t(cx, "editor.preview_beside_tip").to_string(), "⌥⌘V"), None))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_preview_beside(window, cx))),
                )
            })
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
                crate::ui::icon_only(
                    "editor-goto",
                    "search",
                    cx.listener(|this, _: &ClickEvent, window, cx| this.open_picker("", None, window, cx)),
                )
                .tooltip(Tooltip::text(with_shortcut(t(cx, "nav.go_to_file").to_string(), "⌘P"), None)),
            )
            .children(self.render_changes_button(cx))
            .children(self.render_agents_at_work(cx))
            .child(self.render_session_chip(cx))
            // An HTML file is read as a page, not as its markup: the browser beside the terminals shows it.
            .when(doc.language == Language::Html, |d| {
                let path = doc.path.clone();
                d.child(
                    crate::ui::icon_only(
                        "editor-browser",
                        "globe",
                        cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(EditorEvent::ShowInBrowser(path.clone()))),
                    )
                    .tooltip(Tooltip::text(t(cx, "editor.open_browser"), None)),
                )
            })
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
                .when(!(primary) && crate::theme::is_light(), crate::ui::outlined)
                .text_color(hex(if primary { Chrome::ON_ACCENT } else { Chrome::BRIGHT }))
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
            Content::Text | Content::Image(_) => String::new(),
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

    /// The language server of the file in front, in the status bar: its name while it runs, and
    /// "Install …" while it is not installed or could not start (looked for again every so often,
    /// so one installed meanwhile takes over by itself).
    fn render_language_server(&self, doc: &Document, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let id = lsp::id(doc.language)?;
        let name = agentty_bridge::lsp::spec_for(id)?.program();
        if doc.project.as_os_str().is_empty() || !matches!(doc.content, Content::Text) {
            return None;
        }
        let shown = |text: String, tip: String| {
            div().id("editor-lsp").flex_shrink_0().child(text).tooltip(Tooltip::text(tip, None)).into_any_element()
        };
        // A build not trusted yet: jdtls reads the files' text only, kotlin-lsp waits; and the
        // way to read it.
        if let Some(root) = lsp::build_unread(&doc.project, doc.language, &doc.path) {
            let tip = if doc.language == Language::Kotlin { "lsp.read_build_kotlin_tip" } else { "lsp.read_build_tip" };
            return Some(
                div()
                    .id("editor-lsp")
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().opacity(0.55).child(name))
                    .child(
                        div()
                            .id("editor-lsp-read-build")
                            .cursor_pointer()
                            .text_color(hex(Chrome::ATTENTION))
                            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                            .child(t(cx, "lsp.read_build"))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                let root = root.to_string_lossy().into_owned();
                                crate::settings::update_settings(cx, move |s| {
                                    if !s.lsp_trusted_builds.contains(&root) {
                                        s.lsp_trusted_builds.push(root);
                                    }
                                });
                                cx.notify();
                            })),
                    )
                    .tooltip(Tooltip::text(t(cx, tip), None))
                    .into_any_element(),
            );
        }
        Some(match lsp::status(&doc.project, doc.language, &doc.path) {
            "ready" => shown(name.to_string(), t(cx, "lsp.ready_tip").to_string()),
            "starting" | "indexing" => shown(format!("{name} …"), t(cx, "lsp.indexing_tip").to_string()),
            "off" => div()
                .id("editor-lsp")
                .flex_shrink_0()
                .opacity(0.55)
                .child(name)
                .tooltip(Tooltip::text(t(cx, "lsp.off_tip"), None))
                .into_any_element(),
            "skipped" => div()
                .id("editor-lsp")
                .flex_shrink_0()
                .opacity(0.55)
                .child(name)
                .tooltip(Tooltip::text(t(cx, "lsp.skipped_tip"), None))
                .into_any_element(),
            _ => {
                lsp::warm(&doc.project, doc.language, &doc.path);
                div()
                    .id("editor-lsp")
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(hex(Chrome::ATTENTION))
                    .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                    .child(icon("download", 11., hex(Chrome::ATTENTION)))
                    .child(tf(cx, "lsp.install", &[("name", name)]))
                    .tooltip(Tooltip::text(t(cx, "lsp.install_tip"), None))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(EditorEvent::InstallLanguageServer(id))))
                    .into_any_element()
            }
        })
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
        // A narrow pane cuts the bar short at its edge instead of painting over the pane beside it.
        div()
            .h(px(24.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_4()
            .overflow_hidden()
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
                    .children(self.render_problem_counts(cx))
                    .children(self.render_language_server(doc, cx))
                    .child(div().min_w_0().truncate().child(language))
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
        // Buttons that don't fit the dialog side by side (long translations) move to a second line, whole.
        let mut row = div().flex().flex_wrap().justify_end().gap_2();
        for (index, (id, label, choice)) in buttons.into_iter().enumerate() {
            row = row.child(
                div()
                    .id(id)
                    .flex_shrink_0()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .t_body()
                    .cursor_pointer()
                    .bg(if index == 0 { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                    .when(!(index == 0) && crate::theme::is_light(), crate::ui::outlined)
                    .text_color(hex(if index == 0 { Chrome::ON_ACCENT } else { Chrome::BRIGHT }))
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
                        .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(title))
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
                "preview": d.previewing(),
                "beside": d.previewing_beside(),
            })).collect::<Vec<_>>(),
            "tabProblems": self.tabs().iter().map(|t| t.problem.map(|c| format!("{c:06x}"))).collect::<Vec<_>>(),
            "active": self.active,
            "confirm": self.confirm.is_some(),
            "find": self.find_matches().map(|(matches, current)| serde_json::json!({ "matches": matches.len(), "current": current })),
            "picker": self.debug_picker(),
            "linked": self.linked.as_ref().map(|l| l.label.clone()),
            "presence": self.presence.iter().map(|p| serde_json::json!({ "label": p.label, "path": p.path, "at": [p.at.line, p.at.col], "lines": p.lines, "pending": p.pending })).collect::<Vec<_>>(),
            "ask": self.ask.is_some(),
            "link": self.link.map(|(s, e)| [s.line, s.col, e.col]),
            "peek": self.debug_peek(),
            "lsp": self.docs.get(self.active).map(|d| lsp::status(&d.project, d.language, &d.path)),
            "problems": self.debug_problems(),
            "quickFix": self.debug_quick_fix(),
            "typeHover": self.debug_type_hover(),
            "completion": self.debug_completion(),
            "signature": self.debug_signature(),
            "gitMarks": self.debug_git_marks(),
            "sticky": self.debug_sticky(),
            "cursorLine": self.docs.get(self.active).map(|d| d.buffer.line(d.buffer.cursor().line).to_string()),
            "selected": self.docs.get(self.active).map(|d| d.buffer.selected_text()),
            "scrollY": self.docs.get(self.active).map(|d| d.scroll.y),
            "folds": self.docs.get(self.active).map(|d| d.folds.ranges().iter().map(|(s, e)| format!("{}-{}", s + 1, e + 1)).collect::<Vec<_>>()),
            "snippet": self.snippet.is_some(),
            "buildUnread": self.docs.get(self.active).and_then(|d| lsp::build_unread(&d.project, d.language, &d.path)).is_some(),
            "shownLines": self.layout.as_ref().and_then(|l| l.shown_lines()).map(|(a, b)| format!("{}-{}", a + 1, b + 1)),
            "buildError": self.docs.get(self.active).and_then(|d| d.checks.build.as_ref()).map(|e| format!("{}:{} {}", e.path.display(), e.line, e.message)),
            "head": self.docs.get(self.active).map(|d| (0..d.buffer.line_count().min(3)).map(|i| d.buffer.line(i).to_string()).collect::<Vec<_>>()),
            "changes": self.debug_changes(),
            "allChanges": self.changes.iter().map(|c| format!("{}:{}", c.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), c.hunk.start)).collect::<Vec<_>>(),
            "unclaimed": self.unclaimed.len(),
            "preview": {
                "view": self.preview.view.borrow().is_some(),
                "covered": self.preview.covered,
                "shownRevision": self.preview.shown.as_ref().map(|(_, r)| *r),
                "besideWidth": self.preview.side.get().1,
                "syncedLine": self.preview.synced.as_ref().map(|(_, _, line)| *line as f64 / 100.),
            },
        })
    }

    /// Debug driver: types text, runs an action by name, or saves (`AGENTTY_DEBUG` only).
    pub fn debug(&mut self, command: &str, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            // Typed key by key, `\n` as Enter (auto-indent and all).
            "type" => {
                let text = argument.replace("\\n", "\n");
                let (brackets, quotes) =
                    self.docs.get(self.active).map_or((false, false), |d| (d.language.pairs_brackets(), d.language.pairs_quotes()));
                for c in text.chars() {
                    self.edit(cx, |b| {
                        if c == '\n' {
                            b.newline()
                        } else if !(brackets && b.type_paired(c, quotes)) {
                            b.insert(&c.to_string())
                        }
                    });
                    if c == '>' {
                        self.close_typed_tag(cx);
                    }
                    if c != '\n' {
                        self.completion_typed(&c.to_string(), cx);
                        self.signature_typed(&c.to_string(), cx);
                    }
                }
            }
            // The find bar opened with a query, as typed into it.
            "find" => {
                self.open_find(window, cx);
                if let Some(input) = self.find.as_ref().map(|bar| bar.input.clone()) {
                    input.update(cx, |input, cx| input.set_text(argument.to_string(), cx));
                    self.find_query_changed(cx);
                }
            }
            "find-next" => self.find_step(argument != "back", cx),
            "replace" => self.debug_replace(argument, false, window, cx),
            // "Fix with AI" on the problem at the cursor.
            "fix-all-ai" => self.fix_all_with_ai(cx),
            // A click on the change mark of `line` (from 1), and Revert.
            "git-peek" => {
                self.toggle_git_peek(argument.parse::<usize>().unwrap_or(1).saturating_sub(1), cx);
            }
            "git-revert" => self.revert_git_change(cx),
            "organize-imports" => self.organize_imports(window, cx),
            "fix-ai" => {
                let cursor = self.docs.get(self.active).map(|d| d.buffer.cursor());
                let problem = cursor
                    .and_then(|at| self.docs.get(self.active)?.checks.problems.iter().find(|p| p.start <= at && at <= p.end).cloned());
                if let Some(problem) = problem {
                    self.fix_with_ai(&problem, cx);
                }
            }
            // A click on the fold arrow of a line (1-based).
            "snippet" => self.debug_snippet(argument, cx),
            "fold-arrow" => {
                if let Ok(line) = argument.trim().parse::<usize>() {
                    self.toggle_fold(line.saturating_sub(1), cx);
                }
            }
            // The build error row's button.
            "open-build-error" => {
                let found = self.docs.get(self.active).and_then(|d| Some((d.checks.build.clone()?, d.project.clone())));
                if let Some((error, project)) = found {
                    self.open_at(&error.path, &project, Some((error.line + 1, 1)), window, cx);
                }
            }
            "replace-all" => self.debug_replace(argument, true, window, cx),
            "find-option" => self.debug_find_option(argument, cx),
            "goto" => {
                let mut parts = argument.split(':').map(|p| p.parse::<usize>().unwrap_or(1));
                let (line, column) = (parts.next().unwrap_or(1), parts.next().unwrap_or(1));
                self.motion(cx, |b| b.move_to(Pos::new(line.saturating_sub(1), column.saturating_sub(1)), false));
            }
            "save" => {
                self.save(false, cx);
            }
            "format" => self.format(cx),
            "preview" => self.toggle_preview(window, cx),
            "preview-beside" => self.toggle_preview_beside(window, cx),
            "fold-all" => self.fold_all(argument != "off", cx),
            // The HTML file's browser button.
            "browser" => {
                if let Some(doc) = self.docs.get(self.active).filter(|doc| doc.language == Language::Html) {
                    cx.emit(EditorEvent::ShowInBrowser(doc.path.clone()));
                }
            }
            "undo" => self.edit(cx, |b| {
                b.undo();
            }),
            "close" => self.request_close(self.active, cx),
            // Code navigation: the go-to box with a query, a row of it, definition, usages, back.
            "goto-box" => {
                self.open_picker(argument, None, window, cx);
            }
            "pick" => self.debug_pick(argument.parse().unwrap_or(0), window, cx),
            "close-box" => self.close_picker(window, cx),
            // As if ⌘ were held over line:column.
            "hover" => {
                let mut parts = argument.split(':').map(|p| p.parse::<usize>().unwrap_or(1).saturating_sub(1));
                let pos = Pos::new(parts.next().unwrap_or(0), parts.next().unwrap_or(0));
                self.hover_link(Some(pos), cx);
            }
            // F8 / ⇧F8, and the pointer resting on line:column (its problem's message).
            "next-problem" => self.step_problem(argument != "back", cx),
            "quick-fix" => self.open_quick_fix(window, cx),
            "pick-fix" => self.debug_pick_fix(argument.parse().unwrap_or(0), window, cx),
            "rename" => self.debug_rename(argument, window, cx),
            "rename-box" => self.open_rename(window, cx),
            "complete" => self.trigger_completion(cx),
            "accept" => {
                self.completion_accept(cx);
            }
            "down" => {
                self.completion_step(true, cx);
            }
            "problems" => self.show_problems(window, cx),
            "rest" => {
                let mut parts = argument.split(':').map(|p| p.parse::<usize>().unwrap_or(1).saturating_sub(1));
                let pos = Pos::new(parts.next().unwrap_or(0), parts.next().unwrap_or(0));
                self.hover_type(Some(pos), cx);
            }
            "point" => {
                let mut parts = argument.split(':').map(|p| p.parse::<usize>().unwrap_or(1).saturating_sub(1));
                let pos = Pos::new(parts.next().unwrap_or(0), parts.next().unwrap_or(0));
                self.hover_problem(Some(pos), cx);
            }
            "definition" => self.go_to_definition(window, cx),
            "implementation" => self.go_to_implementation(window, cx),
            "usages" => self.find_usages(window, cx),
            "back" => self.navigate(true, window, cx),
            "forward" => self.navigate(false, window, cx),
            // `select 3:1-5:10`, lines and columns from 1.
            "select" => {
                let parse = |p: &str| {
                    let mut parts = p.split(':').map(|n| n.trim().parse::<usize>().unwrap_or(1).saturating_sub(1));
                    Pos::new(parts.next().unwrap_or(0), parts.next().unwrap_or(0))
                };
                let (from, to) = argument.split_once('-').unwrap_or((argument, argument));
                let (from, to) = (parse(from), parse(to));
                self.motion(cx, |b| {
                    b.move_to(from, false);
                    b.move_to(to, true);
                });
            }
            "ask" => {
                self.open_ask(window, cx);
                if let Some(input) = self.ask.as_ref().map(|a| a.input.clone()) {
                    input.update(cx, |input, cx| input.set_text(argument.to_string(), cx));
                }
                if !argument.is_empty() {
                    self.debug_send_ask(window, cx);
                }
            }
            "follow" => self.toggle_follow(cx),
            "session" => cx.emit(EditorEvent::ToggleSession),
            // A window out of sight is not painted, so the workbench never says the editor is on
            // screen: tests of what depends on it say so here.
            "on-screen" => self.preview.covered = false,
            // Review: `undo <n>` / `keep <n>` the n-th change of the file in front (from 0), `next`.
            "review" => {
                let (action, index) = argument.split_once(' ').unwrap_or((argument, "0"));
                let id = self.changes_here().get(index.parse::<usize>().unwrap_or(0)).map(|(c, _)| c.id);
                match (action, id) {
                    ("undo", Some(id)) => self.undo_change(id, cx),
                    ("keep", Some(id)) => self.keep_change(id, cx),
                    ("keep-all", _) => self.keep_all_changes(cx),
                    ("next", _) => self.review_step(true, cx),
                    ("diff", _) => self.review_diff = !self.review_diff,
                    ("all", _) => self.show_all_changes(window, cx),
                    _ => {}
                }
            }
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
/// What a click (or a drag) at `pos` selects. A folded line taken whole takes the lines folded
/// under it (`hidden`) too, as in VS Code: the selection never ends inside a fold.
fn unit_range(buffer: &Buffer, unit: DragUnit, pos: Pos, hidden: &[(usize, usize)]) -> (Pos, Pos) {
    match unit {
        DragUnit::Char => (pos, pos),
        DragUnit::Word => buffer.word_at(pos),
        DragUnit::Line => {
            let last = hidden.iter().find(|(from, _)| *from == pos.line + 1).map_or(pos.line, |(_, to)| *to);
            let end = if last + 1 < buffer.line_count() { Pos::new(last + 1, 0) } else { Pos::new(last, buffer.line(last).len()) };
            (Pos::new(pos.line, 0), end)
        }
    }
}

fn highlights_for(language: Language, path: &Path, first_line: &str) -> Highlights {
    let grammar = highlight::syntaxes().and_then(|set| highlight::grammar_for(set, language, path, first_line));
    Highlights::new(grammar)
}

/// The document for a file already read from disk ([`file::open`] runs off the UI thread).
fn document(path: &Path, project: &Path, opened: Opened) -> Document {
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
        save_after_format: false,
        disk_changed: false,
        checking: false,
        preview: false,
        beside: false,
        center: false,
        scroll_to_line: None,
        base: String::new(),
        outline: Default::default(),
        checks: Default::default(),
        git: Default::default(),
        folds: Default::default(),
    };
    match opened {
        Opened::Text(file) => {
            doc.buffer = Buffer::new(&file.text);
            if let Some(unit) = file.indent {
                doc.buffer.set_indent_unit(unit);
            }
            doc.base = doc.buffer.text();
            // Big files stay uncolored: coloring them would cost more than it helps.
            if file.read_only != Some(ReadOnly::Large) {
                doc.highlights = highlights_for(language, path, doc.buffer.line(0));
            }
            doc.target = file.target;
            doc.format = file.format;
            doc.read_only = file.read_only;
            doc.stamp = file.stamp;
        }
        Opened::Image(image) => doc.content = Content::Image(image),
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
        // A bracket or quote typed over the selection or at the cursor comes with its pair.
        if let Some(c) = typed_char(text) {
            let doc = self.docs.get(self.active);
            let plain = doc.is_some_and(|d| d.buffer.marked().is_none() && range.as_ref().is_none_or(|r| d.selection_utf16() == *r));
            let (brackets, quotes) = doc.map_or((false, false), |d| (d.language.pairs_brackets(), d.language.pairs_quotes()));
            if plain && brackets {
                let mut paired = false;
                self.edit(cx, |b| paired = b.type_paired(c, quotes));
                if paired {
                    self.completion_typed(text, cx);
                    self.signature_typed(text, cx);
                    return;
                }
            }
        }
        self.edit(cx, |buffer| {
            let (start, end) = match range {
                Some(range) => (buffer.pos_at_utf16(range.start), buffer.pos_at_utf16(range.end)),
                None => buffer.marked().unwrap_or_else(|| buffer.selection.range()),
            };
            buffer.commit(start, end, text);
        });
        if text == ">" {
            self.close_typed_tag(cx);
        }
        if self.docs.get(self.active).is_some_and(|d| d.buffer.marked().is_none()) {
            self.completion_typed(text, cx);
            self.signature_typed(text, cx);
        }
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

impl CodeEditor {
    /// The place the page's native view is laid over once it is known, and where it sits for
    /// the pop-ups (`side`, beside the text). Something drawn over it (a question: unsaved
    /// changes, overwrite?) keeps the page out of sight meanwhile.
    fn render_page(&self, id: &'static str, side: Option<std::rc::Rc<std::cell::Cell<(f32, f32)>>>) -> gpui::Stateful<gpui::Div> {
        let covered = self.preview_hidden();
        if covered {
            self.preview.hide();
        }
        let view = self.preview.view.clone();
        div().id(id).bg(hex(Chrome::EDITOR)).child(
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    if let Some(side) = side.as_ref() {
                        let right = f32::from(window.viewport_size().width - bounds.left());
                        side.set((right.max(0.), f32::from(bounds.size.width)));
                    }
                    if !covered {
                        if let Some(view) = view.borrow_mut().as_mut() {
                            view.set_frame(bounds);
                        }
                    }
                },
            )
            .size_full(),
        )
    }
}

impl Render for CodeEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(path) = self.docs.get(self.active).map(|d| d.path.clone()) {
            if self.recent_tabs.first() != Some(&path) {
                self.recent_tabs.retain(|p| *p != path);
                self.recent_tabs.insert(0, path);
                self.recent_tabs.truncate(RECENT_TABS);
            }
        }
        let previewing = self.docs.get(self.active).is_some_and(Document::previewing);
        let beside = self.docs.get(self.active).is_some_and(Document::previewing_beside);
        if !beside {
            self.preview.side.set((0., 0.));
        }
        if !previewing && !beside {
            self.preview.hide();
        } else if self.preview.view.borrow().is_none() {
            let view = crate::webview::WebView::new(window, &preview::view_settings());
            if let Some(view) = view.as_ref() {
                view.load_html(&preview::page_html());
            }
            *self.preview.view.borrow_mut() = view;
            self.preview.shown = None;
            self.preview.rendering = false;
            self.preview.asking = false;
        }
        let find_bar = self.render_find_bar(cx);
        let picker = self.render_picker(cx);
        let selection_tools = self.render_selection_tools(cx);
        let flash = self.render_flash();
        let review_bar = self.render_review_bar(cx);
        let change_diff = self.render_change_diff(cx);
        let link_peek = self.render_link_peek(cx);
        let problem_hover = self.render_problem_hover(cx);
        let quick_fix = self.render_quick_fix(cx);
        let bulb = self.render_bulb(cx);
        let rename_box = self.render_rename(cx);
        let type_hover = self.render_type_hover(cx);
        let completion = self.render_completion(cx);
        let signature = self.render_signature(cx);
        let git_peek = self.render_git_peek(cx);
        let sticky = self.render_sticky(cx);
        let presence_labels =
            if self.docs.get(self.active).is_some_and(|d| !d.previewing()) { self.render_presence_labels(cx) } else { Vec::new() };
        let Some(doc) = self.docs.get(self.active) else {
            // Nothing open yet: only the go-to box (⌘P from the terminals).
            return div()
                .id("code-editor-root")
                .size_full()
                .relative()
                .child(div().id("code-editor").size_full().bg(hex(Chrome::EDITOR)))
                .children(picker);
        };
        let body = if previewing {
            self.render_page("editor-preview", None).flex_1().min_h_0().into_any_element()
        } else if matches!(doc.content, Content::Text) {
            let text = div()
                .id("editor-text")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .relative()
                .cursor(if self.link.is_some() { gpui::CursorStyle::PointingHand } else { gpui::CursorStyle::IBeam })
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(Self::mouse_down))
                .on_mouse_move(cx.listener(Self::mouse_move))
                .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::mouse_up))
                .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(Self::mouse_up))
                .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                .child(element::EditorElement::new(cx.entity()));
            if beside {
                // Half and half, as VS Code opens it.
                let side = self.preview.side.clone();
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(text)
                    .child(div().w(px(1.)).flex_shrink_0().bg(hex(Chrome::BORDER)))
                    .child(self.render_page("editor-preview-beside", Some(side)).flex_1().min_w_0())
                    .into_any_element()
            } else {
                text.into_any_element()
            }
        } else if let Content::Image(image) = &doc.content {
            // Scaled down to fit, never up: a small icon stays sharp at its own size.
            div()
                .id("editor-image")
                .flex_1()
                .min_h_0()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .child(gpui::img(image.clone()).size_full().object_fit(gpui::ObjectFit::ScaleDown))
                .into_any_element()
        } else {
            self.render_placeholder(doc, cx).into_any_element()
        };
        let editor = div()
            .id("code-editor")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            // The pointer left the editor (or went onto a window over it): what it rested on goes
            // too, unless it is on that window a moment later.
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.pointer = None;
                    if std::mem::take(&mut this.gutter_hover) {
                        cx.notify();
                    }
                    this.leave_hovers(cx);
                }
            }))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(hex(Chrome::EDITOR))
            .on_action(cx.listener(|this, _: &MoveLeft, _, cx| this.motion(cx, |b| b.left(false, false))))
            .on_action(cx.listener(|this, _: &MoveRight, _, cx| this.motion(cx, |b| b.right(false, false))))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if !this.completion_step(false, cx) {
                    this.vertical(-1, false, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if !this.completion_step(true, cx) {
                    this.vertical(1, false, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.motion(cx, |b| b.left(true, false))))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.motion(cx, |b| b.right(true, false))))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.vertical(-1, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.vertical(1, true, cx)))
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
                this.vertical(-lines, false, cx)
            }))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                let lines = this.page_lines();
                this.vertical(lines, false, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPageUp, _, cx| {
                let lines = this.page_lines();
                this.vertical(-lines, true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPageDown, _, cx| {
                let lines = this.page_lines();
                this.vertical(lines, true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.motion(cx, |b| b.select_all())))
            .on_action(cx.listener(|this, _: &Backspace, _, cx| {
                let pairs = this.docs.get(this.active).is_some_and(|d| d.language.pairs_brackets());
                this.edit(cx, |b| {
                    if !(pairs && b.backspace_pair()) {
                        b.backspace()
                    }
                });
                this.completion_erased(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleComment, _, cx| {
                let Some(doc) = this.docs.get(this.active) else { return };
                let jsx = matches!(doc.language, Language::Tsx | Language::Jsx) && doc.buffer.selection_is_jsx();
                if let Some((open, close)) = if jsx { Some(("{/*", "*/}")) } else { doc.language.comment() } {
                    this.edit(cx, |b| b.toggle_comment(open, close));
                }
            }))
            .on_action(cx.listener(|this, _: &MoveLinesUp, _, cx| this.dismiss_and_edit(cx, |b| b.move_lines(false))))
            .on_action(cx.listener(|this, _: &MoveLinesDown, _, cx| this.dismiss_and_edit(cx, |b| b.move_lines(true))))
            .on_action(cx.listener(|this, _: &DuplicateLinesUp, _, cx| this.dismiss_and_edit(cx, |b| b.duplicate_lines(false))))
            .on_action(cx.listener(|this, _: &DuplicateLinesDown, _, cx| this.dismiss_and_edit(cx, |b| b.duplicate_lines(true))))
            .on_action(cx.listener(|this, _: &DeleteLines, _, cx| this.dismiss_and_edit(cx, |b| b.delete_lines())))
            .on_action(cx.listener(|this, _: &Delete, _, cx| this.edit(cx, |b| b.delete_forward())))
            .on_action(cx.listener(|this, _: &DeleteWordBack, _, cx| this.dismiss_and_edit(cx, |b| b.delete_word_back())))
            .on_action(cx.listener(|this, _: &DeleteWordForward, _, cx| this.dismiss_and_edit(cx, |b| b.delete_word_forward())))
            .on_action(cx.listener(|this, _: &DeleteToLineStart, _, cx| this.dismiss_and_edit(cx, |b| b.delete_to_line_start())))
            .on_action(cx.listener(|this, _: &SelectLine, _, cx| this.motion(cx, |b| b.select_line())))
            .on_action(cx.listener(|this, _: &InsertLineBelow, _, cx| this.dismiss_and_edit(cx, |b| b.insert_line_below())))
            .on_action(cx.listener(|this, _: &InsertLineAbove, _, cx| this.dismiss_and_edit(cx, |b| b.insert_line_above())))
            .on_action(cx.listener(|this, _: &IndentLines, _, cx| this.dismiss_and_edit(cx, |b| b.indent_lines())))
            .on_action(cx.listener(|this, _: &OutdentLines, _, cx| this.dismiss_and_edit(cx, |b| b.outdent())))
            .on_action(cx.listener(|this, _: &SelectNextOccurrence, _, cx| this.add_cursors(false, cx)))
            .on_action(cx.listener(|this, _: &SelectAllOccurrences, _, cx| this.add_cursors(true, cx)))
            .on_action(cx.listener(|this, _: &Fold, _, cx| this.fold_at_cursor(true, cx)))
            .on_action(cx.listener(|this, _: &Unfold, _, cx| this.fold_at_cursor(false, cx)))
            .on_action(cx.listener(|this, _: &FoldAll, _, cx| this.fold_all(true, cx)))
            .on_action(cx.listener(|this, _: &UnfoldAll, _, cx| this.fold_all(false, cx)))
            .on_action(cx.listener(|this, _: &AddCursorAbove, _, cx| this.add_cursor_vertical(true, cx)))
            .on_action(cx.listener(|this, _: &AddCursorBelow, _, cx| this.add_cursor_vertical(false, cx)))
            .on_action(cx.listener(|this, _: &GoToBracket, _, cx| {
                this.motion(cx, |b| {
                    b.go_to_bracket(element::BRACKET_SCAN);
                })
            }))
            .on_action(cx.listener(|this, _: &Newline, _, cx| {
                if !this.completion_accept(cx) {
                    this.edit(cx, |b| b.newline())
                }
            }))
            .on_action(cx.listener(|this, _: &Indent, _, cx| {
                if !this.completion_accept(cx) && !this.snippet_next(cx) {
                    this.edit(cx, |b| b.indent())
                }
            }))
            .on_action(cx.listener(|this, _: &complete::TriggerCompletion, _, cx| this.trigger_completion(cx)))
            .on_action(cx.listener(|this, _: &complete::Dismiss, _, cx| this.dismiss(cx)))
            .on_action(cx.listener(|this, _: &Outdent, _, cx| this.edit(cx, |b| b.outdent())))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.completion = None;
                this.snippet = None;
                this.signature = None;
                this.edit(cx, |b| {
                    b.undo();
                })
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.completion = None;
                this.signature = None;
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
                        let (start, end) = unit_range(b, DragUnit::Line, b.cursor(), &[]);
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
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save_formatted(cx)))
            .on_action(cx.listener(|this, _: &Format, _, cx| this.format(cx)))
            .on_action(cx.listener(|this, _: &CloseFile, _, cx| this.request_close(this.active, cx)))
            .on_action(cx.listener(|this, _: &TogglePreview, window, cx| this.toggle_preview(window, cx)))
            .on_action(cx.listener(|this, _: &TogglePreviewBeside, window, cx| this.toggle_preview_beside(window, cx)))
            .on_action(cx.listener(|this, _: &navigate::GoToFile, window, cx| this.open_picker("", None, window, cx)))
            .on_action(cx.listener(|this, _: &navigate::GoToFileSymbol, window, cx| this.open_picker("@", None, window, cx)))
            .on_action(cx.listener(|this, _: &navigate::GoToSymbol, window, cx| this.open_picker("#", None, window, cx)))
            .on_action(cx.listener(|this, _: &navigate::GoToLine, window, cx| this.open_picker(":", None, window, cx)))
            .on_action(cx.listener(|this, _: &navigate::GoToDefinition, window, cx| this.go_to_definition(window, cx)))
            .on_action(cx.listener(|this, _: &navigate::GoToImplementation, window, cx| this.go_to_implementation(window, cx)))
            .on_action(cx.listener(|this, _: &problems::NextProblem, _, cx| this.step_problem(true, cx)))
            .on_action(cx.listener(|this, _: &quickfix::QuickFix, window, cx| this.open_quick_fix(window, cx)))
            .on_action(cx.listener(|this, _: &quickfix::OrganizeImports, window, cx| this.organize_imports(window, cx)))
            .on_action(cx.listener(|this, _: &rename::Rename, window, cx| this.open_rename(window, cx)))
            .on_action(cx.listener(|this, _: &find::FindReplace, window, cx| this.open_replace(window, cx)))
            .on_action(cx.listener(|this, _: &problems::PreviousProblem, _, cx| this.step_problem(false, cx)))
            .on_action(cx.listener(|this, _: &navigate::FindUsages, window, cx| this.find_usages(window, cx)))
            .on_action(cx.listener(|this, _: &navigate::NavigateBack, window, cx| this.navigate(true, window, cx)))
            .on_action(cx.listener(|this, _: &navigate::NavigateForward, window, cx| this.navigate(false, window, cx)))
            .on_action(cx.listener(|this, _: &AskAgent, window, cx| this.open_ask(window, cx)))
            .on_action(cx.listener(|this, _: &review::NextChange, _, cx| this.review_step(true, cx)))
            .on_action(cx.listener(|this, _: &review::PreviousChange, _, cx| this.review_step(false, cx)))
            .on_modifiers_changed(cx.listener(|this, event: &gpui::ModifiersChangedEvent, _, cx| {
                let hovered = event.modifiers.secondary().then(|| this.pointer.and_then(|p| this.text_position(p))).flatten();
                this.hover_link(hovered, cx);
            }))
            .child(self.render_toolbar(doc, cx))
            .children(self.render_notice(doc, cx))
            .children(self.render_build_error(doc, cx))
            .child(body)
            .child(self.render_status(doc, cx))
            .children(self.render_confirm(cx));
        // The find bar beside the editor's key context, not in it: what is typed there stays there.
        div()
            .id("code-editor-root")
            .size_full()
            .relative()
            .child(editor)
            .children(find_bar)
            .children(picker)
            .children(selection_tools)
            .children(presence_labels)
            .children(review_bar)
            .children(change_diff)
            .children(link_peek)
            .children(problem_hover)
            .children(bulb)
            .children(type_hover)
            .children(quick_fix)
            .children(rename_box)
            .children(sticky)
            .children(signature)
            .children(git_peek)
            .children(completion)
            .children(flash)
    }
}

/// The one character `text` is, when it is one.
fn typed_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
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
