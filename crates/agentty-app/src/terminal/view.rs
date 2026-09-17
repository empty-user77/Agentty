//! Terminal pane: owns a [`Backend`] and paints its grid with GPUI (Metal) whenever it changes.

use super::backend::{Backend, GridSize, SpawnOptions};
use super::keys;
use crate::agent_signal::{SignalKind, SignalSocket};
use crate::launch::{next_pane_id, LaunchSpec, PaneKind};
use crate::settings::{settings, terminal_theme, CursorShapeSetting, SYMBOLS_FONT};
use crate::theme::{hex, Chrome, TerminalTheme};
use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Direction;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Rgb};
use futures::StreamExt;
use gpui::{
    actions, div, fill, font, outline, point, prelude::*, px, relative, size, App, BorderStyle, Bounds, ClipboardItem, Context,
    CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, FontStyle, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle, Style, Task, TextRun, UTF16Selection,
    UnderlineStyle, Window,
};
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

actions!(terminal, [Copy, Paste, Clear, SelectAll]);

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);
const SPAWN_FALLBACK_DELAY: Duration = Duration::from_millis(400);
const PROBE_INTERVAL: Duration = Duration::from_secs(1);

thread_local! {
    static LAST_GRID: std::cell::Cell<GridSize> = const { std::cell::Cell::new(GridSize { columns: 120, lines: 36, cell_width: 8, cell_height: 16 }) };
}

fn last_grid_size() -> GridSize {
    LAST_GRID.with(|g| g.get())
}

pub enum TerminalEvent {
    TitleChanged,
    Exited,
    /// Agent state or attention flag changed.
    StatusChanged,
    /// The user clicked or typed in this pane.
    Activated,
    /// Something happened the user should know about (agent finished, needs input, bell, `agentty notify`).
    Notified {
        kind: NoticeKind,
        message: Option<String>,
    },
    /// Clicked a link (URL in the text or an OSC 8 hyperlink).
    OpenLink(String),
    /// Clicked a file or folder path printed in the terminal: show it in Finder.
    RevealPath(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Finished,
    /// A tool call waits for the user's approval.
    Permission,
    /// The agent asked the user something.
    Question,
    Message,
    Bell,
}

/// What an agent in this pane is doing, from its hooks and what its screen shows.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatus {
    Idle,
    Working,
    Finished(Option<String>),
    /// Waiting for a tool approval (the text names the tool / command).
    Permission(Option<String>),
    /// Waiting for an answer to a question.
    Question(Option<String>),
    /// The user stopped the turn (Esc).
    Interrupted,
}

impl AgentStatus {
    pub fn needs_user(&self) -> bool {
        matches!(self, AgentStatus::Permission(_) | AgentStatus::Question(_))
    }
}

/// A subagent run by the pane's agent (Claude Code `SubagentStart` / `SubagentStop`).
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentRun {
    pub id: String,
    pub kind: String,
    pub started: Instant,
    pub finished: Option<Instant>,
    /// Most recent tool call inside the subagent: (tool, target).
    pub last_tool: Option<(String, Option<String>)>,
}

/// What an agent's TUI shows right now, read from the bottom of the screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScreenState {
    /// The "esc to interrupt" hint of a running turn.
    pub busy: bool,
    /// An approval prompt, with its question line.
    pub permission: Option<String>,
    /// A selection prompt (question) waiting for an answer.
    pub question: bool,
    /// The turn was interrupted ("Interrupted · What should Claude do instead?").
    pub interrupted: bool,
}

/// Classifies the visible bottom lines of Claude Code / Codex.
pub fn classify_screen(lines: &[String]) -> ScreenState {
    let mut state = ScreenState::default();
    // Only rows with content count: full-screen TUIs leave empty rows around the input box.
    let lines: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let tail = &lines[lines.len().saturating_sub(18)..];
    // "Interrupted" sits right above the input box; older ones scroll up with the conversation.
    let recent = &lines[lines.len().saturating_sub(8)..];
    state.interrupted = recent.iter().any(|l| l.contains("What should Claude do instead") || l.contains("Conversation interrupted"));
    let has_yes_option = tail.iter().any(|l| {
        let t = l.trim_start().trim_start_matches(['❯', '›', '>', ' ']);
        t.starts_with("1. Yes") || t.starts_with("1.Yes")
    });
    for line in tail {
        let lower = line.to_lowercase();
        if lower.contains("esc to interrupt") && (lower.contains('·') || lower.contains('•') || lower.contains('(')) {
            state.busy = true;
        }
        if has_yes_option && state.permission.is_none() {
            let trimmed = line.trim();
            let asks = [
                "Do you want to proceed",
                "Do you want to make this edit",
                "Do you want to create",
                "Do you want to allow",
                "Would you like to run the following command",
                "Would you like to make the following edits",
                "Would you like to grant these permissions",
                "Would you like to send input",
            ];
            if asks.iter().any(|a| trimmed.contains(a)) {
                state.permission = Some(trimmed.chars().take(120).collect());
            }
        }
        if lower.contains("enter to select") && (lower.contains("esc to cancel") || lower.contains("to navigate")) {
            state.question = true;
        }
    }
    if state.permission.is_some() {
        state.question = false;
    }
    state
}

/// Cell metrics and origin captured during prepaint, used for mouse and IME hit testing.
#[derive(Clone, Copy)]
struct Layout {
    /// Top-left of the element (the text origin adds padding).
    bounds_origin: Point<Pixels>,
    bounds_height: Pixels,
    origin: Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    display_offset: usize,
    cursor: (usize, usize),
}

pub struct TerminalView {
    pub pane_id: u64,
    pub spec: LaunchSpec,
    pub title: String,
    pub status: AgentStatus,
    /// Set when the agent finishes or asks for input; cleared when the user clicks the pane.
    pub attention: bool,
    pub launched_at_ms: u64,
    backend: Option<Backend>,
    spawned: bool,
    /// Input sent before the process started; flushed right after spawning.
    pending_input: Vec<Vec<u8>>,
    pub error: Option<String>,
    focus_handle: FocusHandle,
    marked_text: Option<String>,
    /// Text committed by the input method but not echoed yet: (cursor cell when committed, its width
    /// in cells). The next composition is drawn after it instead of on top of it.
    commit_anchor: Option<((usize, usize), usize)>,
    layout: Option<Layout>,
    scroll_remainder: f32,
    selecting: bool,
    /// Find-in-terminal matches (all of scrollback) and the one selected.
    pub search: Option<(std::rc::Rc<Vec<Match>>, usize)>,
    /// A mouse press was reported to the program; its drag and release go there too.
    mouse_reporting: bool,
    last_mouse_cell: Option<(usize, usize)>,
    /// Cell of a plain mouse press, to open a link on release without a drag.
    press_cell: Option<GridPoint>,
    /// Link under the pointer: (grid line, start column, end column).
    hover_link: Option<(i32, usize, usize)>,
    /// What the underlined link opens, for the hint shown next to it.
    hover_target: Option<LinkTarget>,
    cursor_visible: bool,
    /// Agent detected in the foreground (also when started by hand in a shell pane).
    pub live_agent: Option<PaneKind>,
    /// Any known agent CLI in the foreground, by brand id (`claude`, `gemini`, `agy`, …).
    pub live_tool: Option<&'static str>,
    /// When Esc was last pressed while the agent worked (interrupt detection).
    esc_at: Option<Instant>,
    /// Consecutive probes without the busy hint while marked as working.
    quiet_ticks: u8,
    /// Subagents of the current session, newest last.
    pub subagents: Vec<SubagentRun>,
    /// Session id of the agent in the foreground (launched with it, or found from its transcript).
    pub session_id_live: Option<String>,
    /// Subagent transcripts of that session: (total, written in the last few seconds).
    pub subagent_files: (usize, usize),
    /// Latest tool call of the main agent: (tool, target).
    pub last_tool: Option<(String, Option<String>)>,
    /// The user typed, pasted or dropped something here (closing it may lose work).
    pub user_typed: bool,
    /// When anything last happened in this pane (output from an agent turn, input, status).
    pub last_activity_ms: u64,
    pub live_cwd: Option<PathBuf>,
    pub git_branch: Option<String>,
    /// Uncommitted changes in the pane's repository.
    pub git_dirty: bool,
    /// Commits not pushed yet (`None` without an upstream).
    pub git_ahead: Option<u32>,
    git_probe: Option<Task<()>>,
    pub working_since: Option<Instant>,
    /// Model, context and plan usage of the agent's session (from its transcript).
    pub stats: Option<agentty_bridge::SessionStats>,
    /// Rate-limit usage reported live by the Claude statusline wrapper.
    pub live_usage: Option<f64>,
    probe_ticks: u32,
    /// A launched agent pane whose agent process was seen and has since exited to the shell.
    agent_exited: bool,
    agent_seen: bool,
    model_probe: Option<Task<()>>,
    /// Whether the pane had keyboard focus in the last painted frame.
    focused: bool,
    _events: Option<Task<()>>,
    _blink: Task<()>,
    _probe: Task<()>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    pub fn new(spec: LaunchSpec, cx: &mut Context<Self>) -> Self {
        let pane_id = next_pane_id();
        let blink = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
            let alive = this.update(cx, |view, cx| {
                if settings(cx).cursor_blink {
                    view.cursor_visible = !view.cursor_visible;
                    cx.notify();
                } else if !view.cursor_visible {
                    view.cursor_visible = true;
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        });

        let probe = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(PROBE_INTERVAL).await;
            if this.update(cx, |view, cx| view.probe(cx)).is_err() {
                break;
            }
        });

        // If the pane is never painted (hidden tab, display asleep), start it anyway with the
        // size of the most recently laid-out pane.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SPAWN_FALLBACK_DELAY).await;
            let _ = this.update(cx, |view, cx| {
                if !view.spawned {
                    view.spawn(last_grid_size(), cx);
                }
            });
        })
        .detach();

        Self {
            pane_id,
            title: spec.title.clone(),
            status: AgentStatus::Idle,
            attention: false,
            launched_at_ms: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
            spec,
            backend: None,
            spawned: false,
            pending_input: Vec::new(),
            error: None,
            focus_handle: cx.focus_handle(),
            marked_text: None,
            commit_anchor: None,
            layout: None,
            scroll_remainder: 0.,
            selecting: false,
            search: None,
            mouse_reporting: false,
            last_mouse_cell: None,
            press_cell: None,
            hover_link: None,
            hover_target: None,
            cursor_visible: true,
            live_agent: None,
            live_tool: None,
            esc_at: None,
            quiet_ticks: 0,
            subagents: Vec::new(),
            last_tool: None,
            session_id_live: None,
            subagent_files: (0, 0),
            last_activity_ms: crate::ui::now_ms(),
            user_typed: false,
            live_cwd: None,
            git_branch: None,
            git_dirty: false,
            git_ahead: None,
            git_probe: None,
            working_since: None,
            stats: None,
            live_usage: None,
            probe_ticks: 0,
            agent_exited: false,
            agent_seen: false,
            model_probe: None,
            focused: false,
            _events: None,
            _blink: blink,
            _probe: probe,
        }
    }

    /// Starts the process once the pane knows its real size, so shells never see an initial
    /// resize (zsh would otherwise leave a stray `%` line behind).
    fn spawn(&mut self, grid: GridSize, cx: &mut Context<Self>) {
        self.spawned = true;
        let socket = cx.try_global::<SignalSocket>().map(|s| s.path.clone());
        let options =
            SpawnOptions { spec: &self.spec, pane_id: self.pane_id, signal_socket: socket.as_deref(), scrollback: settings(cx).scrollback };
        match Backend::spawn(options, grid) {
            Ok((backend, mut rx)) => {
                for bytes in self.pending_input.drain(..) {
                    backend.write(bytes);
                }
                self.backend = Some(backend);
                self._events = Some(cx.spawn(async move |this, cx| {
                    while let Some(event) = rx.next().await {
                        // Coalesce bursts of output into a single repaint.
                        let mut batch = vec![event];
                        while let Ok(event) = rx.try_recv() {
                            batch.push(event);
                        }
                        if this.update(cx, |view, cx| view.handle_events(batch, cx)).is_err() {
                            break;
                        }
                    }
                }));
            }
            Err(err) => self.error = Some(format!("{err:#}")),
        }
        cx.notify();
    }

    /// Samples what is running in the foreground, where, and on which branch. Cheap syscalls only.
    fn probe(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = &self.backend else { return };
        let live_tool = crate::procinfo::foreground_tool(backend.tty_fd, backend.child_pid);
        let live_agent = match live_tool {
            Some("claude") => Some(PaneKind::Claude),
            Some("codex") => Some(PaneKind::Codex),
            _ => None,
        };
        let foreground = crate::procinfo::foreground_pid(backend.tty_fd).unwrap_or(backend.child_pid);
        let cwd = crate::procinfo::cwd_of(foreground).or_else(|| crate::procinfo::cwd_of(backend.child_pid));
        let mut changed = live_agent != self.live_agent || live_tool != self.live_tool;
        if self.spec.kind != PaneKind::Shell {
            if live_agent.is_some() {
                self.agent_seen = true;
                self.agent_exited = false;
            } else if (self.agent_seen || self.probe_ticks > 20) && !self.agent_exited {
                // `claude` / `codex` quit and the pane fell back to its shell.
                self.agent_exited = true;
                changed = true;
            }
        }
        if live_agent.is_none() && (self.live_agent.is_some() || (changed && self.agent_exited)) {
            self.forget_agent_state();
        }
        self.live_agent = live_agent;
        self.live_tool = live_tool;
        if cwd.is_some() && cwd != self.live_cwd {
            self.git_branch = cwd.as_deref().and_then(crate::procinfo::git_branch);
            self.live_cwd = cwd;
            changed = true;
        }
        if self.probe_ticks.is_multiple_of(3) {
            self.probe_git(cx);
        }
        self.probe_ticks = self.probe_ticks.wrapping_add(1);
        if self.probe_ticks % 3 == 1 {
            self.probe_model(cx);
        }
        if live_agent.is_some() && self.update_from_screen(cx) {
            changed = true;
        }
        // Keep the elapsed timer moving while an agent works.
        if changed || self.working_since.is_some() {
            if changed {
                cx.emit(TerminalEvent::StatusChanged);
            }
            cx.notify();
        }
    }

    /// Branch, uncommitted changes and unpushed commits, from `git status` in the background.
    fn probe_git(&mut self, cx: &mut Context<Self>) {
        if self.git_probe.is_some() {
            return;
        }
        let cwd = self.display_cwd();
        if crate::procinfo::git_branch(&cwd).is_none() {
            if self.git_branch.is_some() || self.git_dirty {
                self.git_branch = None;
                self.git_dirty = false;
                self.git_ahead = None;
                cx.notify();
            }
            return;
        }
        let task = cx.background_spawn(async move { agentty_bridge::git::status(&cwd).ok() });
        self.git_probe = Some(cx.spawn(async move |this, cx| {
            let status = task.await;
            let _ = this.update(cx, |view, cx| {
                view.git_probe = None;
                let Some(status) = status else { return };
                let branch = status.branch.clone().or_else(|| crate::procinfo::git_branch(&view.display_cwd()));
                let dirty = !status.files.is_empty();
                let ahead = status.upstream.as_ref().map(|_| status.ahead);
                if (branch.as_ref(), dirty, ahead) != (view.git_branch.as_ref(), view.git_dirty, view.git_ahead) {
                    view.git_branch = branch;
                    view.git_dirty = dirty;
                    view.git_ahead = ahead;
                    cx.emit(TerminalEvent::StatusChanged);
                    cx.notify();
                }
            });
        }));
    }

    /// Drops everything learned about an agent that has left the foreground.
    fn forget_agent_state(&mut self) {
        self.status = AgentStatus::Idle;
        self.working_since = None;
        self.stats = None;
        self.live_usage = None;
        self.quiet_ticks = 0;
        self.last_tool = None;
        self.subagents.clear();
    }

    /// The last `rows` lines of the live screen (ignores scrollback position).
    pub fn screen_lines(&self, rows: usize) -> Vec<String> {
        let Some(backend) = &self.backend else { return Vec::new() };
        let term = backend.term.lock();
        let grid = term.grid();
        let lines = grid.screen_lines();
        let columns = grid.columns();
        (lines.saturating_sub(rows)..lines)
            .map(|row| {
                let line = &grid[Line(row as i32)];
                (0..columns).map(|c| line[Column(c)].c).collect::<String>().trim_end().to_string()
            })
            .collect()
    }

    /// Corrects the hook-reported status with what the agent's screen shows. Hooks don't fire
    /// for Esc interrupts, Codex approvals or missed events; the screen always tells.
    fn update_from_screen(&mut self, cx: &mut Context<Self>) -> bool {
        let screen = classify_screen(&self.screen_lines(80));
        let before = self.status.clone();
        if let Some(prompt) = screen.permission.clone() {
            self.quiet_ticks = 0;
            if !matches!(self.status, AgentStatus::Permission(_)) {
                let label = self.last_tool.as_ref().map(|(tool, target)| tool_label(tool, target.as_deref())).or(Some(prompt));
                self.enter_waiting(AgentStatus::Permission(label.clone()), NoticeKind::Permission, label, cx);
            }
        } else if screen.busy {
            self.quiet_ticks = 0;
            if self.status != AgentStatus::Working {
                self.status = AgentStatus::Working;
                self.working_since.get_or_insert_with(Instant::now);
            }
        } else if screen.question && matches!(self.status, AgentStatus::Working | AgentStatus::Question(_)) {
            self.quiet_ticks = 0;
            if !matches!(self.status, AgentStatus::Question(_)) {
                self.enter_waiting(AgentStatus::Question(None), NoticeKind::Question, None, cx);
            }
        } else if matches!(self.status, AgentStatus::Working | AgentStatus::Permission(_) | AgentStatus::Question(_)) {
            self.quiet_ticks = self.quiet_ticks.saturating_add(1);
            let esc_recent = self.esc_at.is_some_and(|at| at.elapsed() < Duration::from_secs(20));
            // Esc early in a turn rewinds it without an "Interrupted" line; the transcript still says so.
            let logged = self.quiet_ticks >= 2
                && matches!((self.agent_kind().and_then(PaneKind::agent), self.session_id_live.as_deref()), (Some(agent), Some(id)) if agentty_bridge::last_turn_interrupted(agent, id));
            if screen.interrupted || logged || (esc_recent && self.quiet_ticks >= 2) {
                self.status = AgentStatus::Interrupted;
                self.working_since = None;
                self.esc_at = None;
            } else if self.quiet_ticks >= 4 {
                // No busy hint and no prompt for a few seconds: the turn is over without a hook.
                self.working_since = None;
                if self.spec.kind == PaneKind::Shell && self.status == AgentStatus::Working {
                    // Started by hand, so no hooks will say so: report the finished turn here.
                    self.status = AgentStatus::Finished(None);
                    self.attention = true;
                    cx.emit(TerminalEvent::Notified { kind: NoticeKind::Finished, message: None });
                } else {
                    self.status = AgentStatus::Idle;
                }
            }
        }
        if self.status != AgentStatus::Working {
            self.working_since = None;
        }
        self.status != before
    }

    fn enter_waiting(&mut self, status: AgentStatus, notice: NoticeKind, message: Option<String>, cx: &mut Context<Self>) {
        self.status = status;
        self.working_since = None;
        self.attention = true;
        self.last_activity_ms = crate::ui::now_ms();
        cx.emit(TerminalEvent::Notified { kind: notice, message });
    }

    /// Reads the model from the session transcript in the background (every few seconds).
    fn probe_model(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self.agent_kind().and_then(PaneKind::agent) else {
            self.stats = None;
            return;
        };
        if self.model_probe.is_some() {
            return;
        }
        let known = if self.spec.kind == PaneKind::Claude { self.spec.session_id.clone() } else { None };
        let cwd = self.display_cwd();
        let since = self.launched_at_ms;
        let task = cx.background_spawn(async move {
            let id = known.or_else(|| match agent {
                agentty_bridge::model::Agent::Claude => agentty_bridge::claude::find_recent(&cwd, since),
                agentty_bridge::model::Agent::Codex => agentty_bridge::codex::find_recent(&cwd, since),
                _ => None,
            })?;
            let subagents =
                if agent == agentty_bridge::model::Agent::Claude { agentty_bridge::claude::subagent_activity(&id) } else { (0, 0) };
            Some((id.clone(), agentty_bridge::session_stats(agent, &id), subagents))
        });
        self.model_probe = Some(cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |view, cx| {
                view.model_probe = None;
                let Some((id, stats, subagents)) = found else { return };
                let mut changed = view.session_id_live.as_ref() != Some(&id) || view.subagent_files != subagents;
                view.session_id_live = Some(id);
                view.subagent_files = subagents;
                if stats.is_some() && stats != view.stats {
                    view.stats = stats;
                    changed = true;
                }
                if changed {
                    cx.emit(TerminalEvent::StatusChanged);
                    cx.notify();
                }
            });
        }));
    }

    /// Which agent this pane is showing, if any.
    pub fn agent_kind(&self) -> Option<PaneKind> {
        match self.spec.kind {
            PaneKind::Shell => self.live_agent,
            _ if self.agent_exited => self.live_agent,
            kind => Some(kind),
        }
    }

    /// Kind used for icons and labels: the detected agent, else what the pane was launched as.
    pub fn display_kind(&self) -> PaneKind {
        self.agent_kind().unwrap_or(self.spec.kind)
    }

    /// Brand id of what the pane shows: the agent in the foreground, else a CLI it was launched
    /// for (e.g. Gemini started from the launcher), else `shell`.
    pub fn tool_id(&self) -> &'static str {
        if let Some(kind) = self.agent_kind() {
            return crate::brand::kind_id(kind);
        }
        if let Some(tool) = self.live_tool {
            return tool;
        }
        "shell"
    }

    pub fn display_cwd(&self) -> PathBuf {
        self.live_cwd.clone().unwrap_or_else(|| self.spec.cwd.clone())
    }

    /// Running, or about to start on first paint.
    pub fn is_running(&self) -> bool {
        self.backend.is_some() || (!self.spawned && self.error.is_none())
    }

    /// Whether closing the pane could interrupt or lose something (it was used, or an agent works).
    pub fn has_activity(&self) -> bool {
        self.user_typed || matches!(self.status, AgentStatus::Working) || self.status.needs_user()
    }

    pub fn is_agent(&self) -> bool {
        self.agent_kind().is_some()
    }

    /// Title for tabs and lists: shell titles like `user@host:~/code` are shortened to the path.
    pub fn display_title(&self) -> String {
        let title = self.title.trim();
        let title = match title.split_once(':') {
            Some((prefix, rest)) if prefix.contains('@') && !prefix.contains(' ') && !rest.is_empty() => rest.trim(),
            _ => title,
        };
        if title.is_empty() {
            self.spec.title.clone()
        } else if title == "~" {
            // A lone "~" (shell title in the home folder) says little; show where that is.
            crate::launch::home_dir().display().to_string()
        } else {
            title.to_string()
        }
    }

    /// Directory the shell is in right now (follows `cd`), falling back to the launch directory.
    pub fn current_dir(&self) -> PathBuf {
        self.backend
            .as_ref()
            .and_then(|b| crate::procinfo::cwd_of(b.child_pid))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| self.spec.cwd.clone())
    }

    /// Visible text with non-ASCII characters shown as code points (debug builds only).
    pub fn debug_screen_text(&self) -> String {
        let Some(backend) = &self.backend else { return String::new() };
        let term = backend.term.lock();
        let mut out = String::new();
        for cell in term.renderable_content().display_iter {
            let c = cell.c;
            if cell.point.column.0 == 0 {
                out.push('\n');
            }
            if c.is_ascii() {
                out.push(c)
            } else {
                out.push_str(&format!("<U+{:04X}>", c as u32))
            }
        }
        out.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
    }

    /// Usage meter value: the transcript's rate limits (Codex) or the live statusline report (Claude).
    pub fn usage_percent(&self) -> Option<f64> {
        self.stats.as_ref().and_then(|s| s.rate_limit_percent).or(self.live_usage)
    }

    pub fn apply_signal(&mut self, signal: crate::agent_signal::AgentSignal, cx: &mut Context<Self>) {
        let crate::agent_signal::AgentSignal { kind, message, detail, .. } = signal;
        if kind == SignalKind::Usage {
            let usage = message.and_then(|m| m.parse().ok());
            if usage != self.live_usage {
                self.live_usage = usage;
                cx.emit(TerminalEvent::StatusChanged);
                cx.notify();
            }
            return;
        }
        self.last_activity_ms = crate::ui::now_ms();
        let mut notice = None;
        match kind {
            SignalKind::Working => {
                if let Some((id, _)) = &detail.subagent {
                    // Tool calls inside a subagent keep the main turn working.
                    if let (Some(run), Some(tool)) = (self.subagents.iter_mut().find(|r| &r.id == id), detail.tool.clone()) {
                        run.last_tool = Some((tool, detail.target.clone()));
                    }
                } else if let Some(tool) = detail.tool.clone() {
                    self.last_tool = Some((tool, detail.target.clone()));
                }
                self.status = AgentStatus::Working;
                self.working_since.get_or_insert_with(Instant::now);
                self.quiet_ticks = 0;
            }
            SignalKind::Stop => {
                self.status = AgentStatus::Finished(message.clone());
                self.attention = true;
                notice = Some(NoticeKind::Finished);
            }
            SignalKind::Permission => {
                let label = detail.tool.as_deref().map(|tool| tool_label(tool, detail.target.as_deref()));
                self.status = AgentStatus::Permission(label.clone());
                self.attention = true;
                return self.finish_signal(Some(NoticeKind::Permission), label, cx);
            }
            SignalKind::Notification => match detail.notification_type.as_deref() {
                // Sent a while after a finished turn; the finish was already reported.
                Some("idle_prompt") | Some("auth_success") | Some("agent_completed") => return,
                Some("permission_prompt") | Some("worker_permission_prompt") => {
                    if self.status.needs_user() {
                        return;
                    }
                    let label = self.last_tool.as_ref().map(|(t, target)| tool_label(t, target.as_deref())).or(message.clone());
                    self.status = AgentStatus::Permission(label.clone());
                    self.attention = true;
                    return self.finish_signal(Some(NoticeKind::Permission), label, cx);
                }
                Some(_) => {
                    self.status = AgentStatus::Question(message.clone());
                    self.attention = true;
                    notice = Some(NoticeKind::Question);
                }
                // Older versions don't say why; the message does.
                None => {
                    let waiting_idle = message.as_deref().is_some_and(|m| m.contains("waiting for your input"));
                    if waiting_idle && !matches!(self.status, AgentStatus::Working) {
                        return;
                    }
                    let permission = message.as_deref().is_some_and(|m| m.contains("permission"));
                    self.status =
                        if permission { AgentStatus::Permission(message.clone()) } else { AgentStatus::Question(message.clone()) };
                    self.attention = true;
                    notice = Some(if permission { NoticeKind::Permission } else { NoticeKind::Question });
                }
            },
            SignalKind::Notify => {
                self.attention = true;
                notice = Some(NoticeKind::Message);
            }
            SignalKind::SubagentStart => {
                if let Some((id, agent_type)) = detail.subagent {
                    self.subagents.retain(|r| r.id != id);
                    self.subagents.push(SubagentRun { id, kind: agent_type, started: Instant::now(), finished: None, last_tool: None });
                    // Keep the list bounded for long sessions.
                    if self.subagents.len() > 40 {
                        self.subagents.remove(0);
                    }
                }
                self.status = AgentStatus::Working;
                self.working_since.get_or_insert_with(Instant::now);
            }
            SignalKind::SubagentStop => {
                if let Some((id, agent_type)) = detail.subagent {
                    match self.subagents.iter_mut().find(|r| r.id == id) {
                        Some(run) => run.finished = Some(Instant::now()),
                        None => self.subagents.push(SubagentRun {
                            id,
                            kind: agent_type,
                            started: Instant::now(),
                            finished: Some(Instant::now()),
                            last_tool: None,
                        }),
                    }
                }
            }
            SignalKind::SessionEnd => {
                // The CLI is exiting: show the shell state right away instead of on the next probe.
                if self.spec.kind != PaneKind::Shell {
                    self.agent_exited = true;
                }
                self.live_agent = None;
                self.live_tool = None;
                self.forget_agent_state();
            }
            SignalKind::Usage => {}
        }
        if self.status != AgentStatus::Working {
            self.working_since = None;
        }
        self.finish_signal(notice, message, cx);
    }

    fn finish_signal(&mut self, notice: Option<NoticeKind>, message: Option<String>, cx: &mut Context<Self>) {
        if self.status != AgentStatus::Working {
            self.working_since = None;
        }
        if let Some(kind) = notice {
            cx.emit(TerminalEvent::Notified { kind, message });
        }
        cx.emit(TerminalEvent::StatusChanged);
        cx.notify();
    }

    /// Marks the pane as seen (like reading a notification).
    pub fn acknowledge(&mut self, cx: &mut Context<Self>) {
        if self.attention {
            self.attention = false;
            cx.emit(TerminalEvent::StatusChanged);
            cx.notify();
        }
    }

    /// Types text into the program as a paste, without submitting.
    /// Case-insensitive plain-text search over the whole buffer; jumps to the newest match.
    pub fn set_search(&mut self, query: &str, cx: &mut Context<Self>) {
        const MAX_MATCHES: usize = 5_000;
        if query.is_empty() {
            self.search = None;
            return cx.notify();
        }
        let Some(backend) = &self.backend else { return };
        let escaped: String = query.chars().flat_map(|c| if "\\.+*?()|[]{}^$#&-~".contains(c) { vec!['\\', c] } else { vec![c] }).collect();
        let Ok(mut regex) = RegexSearch::new(&format!("(?i){escaped}")) else { return };
        let matches: Vec<Match> = {
            let term = backend.term.lock();
            let grid = term.grid();
            let start = GridPoint::new(grid.topmost_line(), Column(0));
            let end = GridPoint::new(grid.bottommost_line(), grid.last_column());
            RegexIter::new(start, end, Direction::Right, &term, &mut regex).take(MAX_MATCHES).collect()
        };
        let current = matches.len().saturating_sub(1);
        self.search = Some((std::rc::Rc::new(matches), current));
        self.reveal_search_match();
        cx.notify();
    }

    /// Moves to the next (`forward`) or previous match.
    pub fn step_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some((matches, current)) = self.search.as_mut().filter(|(m, _)| !m.is_empty()) {
            let len = matches.len();
            *current = if forward { (*current + 1) % len } else { (*current + len - 1) % len };
        }
        self.reveal_search_match();
        cx.notify();
    }

    /// (current index, count) for the find bar.
    /// Process id of the pane's shell (root of its process tree).
    pub fn shell_pid(&self) -> Option<u32> {
        self.backend.as_ref().map(|b| b.child_pid)
    }

    pub fn search_position(&self) -> Option<(usize, usize)> {
        self.search.as_ref().map(|(m, c)| (if m.is_empty() { 0 } else { c + 1 }, m.len()))
    }

    fn reveal_search_match(&mut self) {
        let (Some((matches, current)), Some(backend)) = (&self.search, &self.backend) else { return };
        if let Some(found) = matches.get(*current) {
            backend.term.lock().scroll_to_point(*found.start());
        }
    }

    /// Pastes dropped files as shell-quoted paths.
    pub fn drop_paths(&mut self, paths: &[std::path::PathBuf]) {
        let text = paths.iter().map(|p| crate::launch::shell_quote(&p.display().to_string())).collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            self.insert_text(&format!("{text} "));
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        let bytes =
            if self.mode().contains(TermMode::BRACKETED_PASTE) { format!("\x1b[200~{text}\x1b[201~") } else { text.replace('\n', " ") };
        self.write_user_input(bytes.into_bytes());
    }

    /// Submits text to the program as if pasted and entered by the user.
    pub fn submit_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        let payload = if bracketed { format!("\x1b[200~{text}\x1b[201~") } else { text.replace('\n', " ") };
        self.write_user_input(payload.into_bytes());
        if self.agent_kind() == Some(PaneKind::Codex) {
            self.status = AgentStatus::Working;
            self.working_since = Some(Instant::now());
            cx.emit(TerminalEvent::StatusChanged);
        }
        // Give the TUI a moment to process the paste before pressing Enter.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(200)).await;
            let _ = this.update(cx, |view, _| view.write(b"\r".to_vec()));
        })
        .detach();
    }

    fn handle_events(&mut self, events: Vec<TermEvent>, cx: &mut Context<Self>) {
        for event in events {
            match event {
                TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {}
                TermEvent::Title(title) => {
                    self.title = title;
                    cx.emit(TerminalEvent::TitleChanged);
                }
                TermEvent::ResetTitle => {
                    self.title = self.spec.title.clone();
                    cx.emit(TerminalEvent::TitleChanged);
                }
                TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
                TermEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                // Programs may not read the clipboard via OSC 52: any output (a `cat`-ed file, a remote
                // host) could otherwise exfiltrate it. Writes are still allowed.
                TermEvent::ClipboardLoad(..) => {}
                TermEvent::ColorRequest(index, format) => {
                    let theme = terminal_theme(cx).clone();
                    let rgb = self
                        .backend
                        .as_ref()
                        .map(|b| b.term.lock().colors()[index].unwrap_or_else(|| to_rgb(default_color(&theme, index))));
                    if let Some(rgb) = rgb {
                        self.write(format(rgb).into_bytes());
                    }
                }
                TermEvent::TextAreaSizeRequest(format) => {
                    if let Some(size) = self.backend.as_ref().map(|b| b.size()) {
                        let window_size = alacritty_terminal::event::WindowSize {
                            num_lines: size.lines as u16,
                            num_cols: size.columns as u16,
                            cell_width: size.cell_width,
                            cell_height: size.cell_height,
                        };
                        self.write(format(window_size).into_bytes());
                    }
                }
                TermEvent::Bell => {
                    if !self.focused {
                        self.attention = true;
                        cx.emit(TerminalEvent::StatusChanged);
                        cx.emit(TerminalEvent::Notified { kind: NoticeKind::Bell, message: None });
                    }
                }
                TermEvent::ChildExit(_) | TermEvent::Exit => {
                    self.backend = None;
                    cx.emit(TerminalEvent::Exited);
                }
            }
        }
        cx.notify();
    }

    pub fn write(&mut self, bytes: Vec<u8>) {
        match &self.backend {
            Some(backend) => backend.write(bytes),
            None if !self.spawned => self.pending_input.push(bytes),
            None => {}
        }
    }

    /// Input typed by the user: snaps the viewport back to the prompt and drops any selection.
    fn write_user_input(&mut self, bytes: Vec<u8>) {
        self.user_typed = true;
        self.last_activity_ms = crate::ui::now_ms();
        if let Some(backend) = &self.backend {
            let mut term = backend.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.cursor_visible = true;
        self.write(bytes);
    }

    pub fn mode(&self) -> TermMode {
        self.backend.as_ref().map(|b| *b.term.lock().mode()).unwrap_or_else(TermMode::empty)
    }

    fn activate(&mut self, cx: &mut Context<Self>) {
        self.acknowledge(cx);
        cx.emit(TerminalEvent::Activated);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.activate(cx);
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" && matches!(self.status, AgentStatus::Working | AgentStatus::Permission(_) | AgentStatus::Question(_))
        {
            self.esc_at = Some(Instant::now());
        }
        if keystroke.key == "enter" && self.agent_kind() == Some(PaneKind::Codex) && !keystroke.modifiers.shift {
            // Codex has no "prompt submitted" hook; Enter is the best signal (the screen check
            // corrects it within seconds if nothing started).
            self.status = AgentStatus::Working;
            self.working_since = Some(Instant::now());
            self.quiet_ticks = 0;
            cx.emit(TerminalEvent::StatusChanged);
        }
        if let Some(bytes) = keys::to_escape(keystroke, self.mode(), settings(cx).option_as_meta) {
            self.write_user_input(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.backend.as_ref().and_then(|b| b.term.lock().selection_to_string());
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let bytes = if self.mode().contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.write_user_input(bytes);
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        self.write_user_input(vec![0x0c]);
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(backend) = &self.backend {
            let mut term = backend.term.lock();
            let top = term.grid().topmost_line();
            let bottom = term.grid().bottommost_line();
            let last_col = term.grid().last_column();
            let mut selection = Selection::new(SelectionType::Simple, GridPoint::new(top, Column(0)), Side::Left);
            selection.update(GridPoint::new(bottom, last_col), Side::Right);
            term.selection = Some(selection);
        }
        cx.notify();
    }

    fn grid_point(&self, position: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let layout = self.layout?;
        let size = self.backend.as_ref()?.size();
        let x = ((position.x - layout.origin.x) / layout.cell_width).max(0.);
        let y = ((position.y - layout.origin.y) / layout.line_height).max(0.);
        let column = (x as usize).min(size.columns.saturating_sub(1));
        let row = (y as usize).min(size.lines.saturating_sub(1));
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        let line = Line(row as i32 - layout.display_offset as i32);
        Some((GridPoint::new(line, Column(column)), side))
    }

    /// Viewport cell (0-based column, row) under `position`, for mouse reporting.
    fn viewport_cell(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let layout = self.layout?;
        let size = self.backend.as_ref()?.size();
        let x = ((position.x - layout.origin.x) / layout.cell_width).max(0.) as usize;
        let y = ((position.y - layout.origin.y) / layout.line_height).max(0.) as usize;
        Some((x.min(size.columns.saturating_sub(1)), y.min(size.lines.saturating_sub(1))))
    }

    /// Sends an xterm mouse event (SGR when the program asked for it, legacy X10 encoding otherwise).
    fn report_mouse(&mut self, button: u8, cell: (usize, usize), pressed: bool) {
        let mode = self.mode();
        let bytes = mouse_report(button, cell, pressed, mode.contains(TermMode::SGR_MOUSE));
        if let Some(bytes) = bytes {
            self.write(bytes);
        }
    }

    /// Programs like Claude Code's full-screen UI, vim or htop take the mouse; Shift bypasses it
    /// so text can still be selected.
    fn mouse_captured(&self, shift: bool) -> bool {
        !shift && self.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// The link under a grid point: an OSC 8 hyperlink, a URL or an existing file path in the line.
    fn link_at(&self, point: GridPoint) -> Option<LinkTarget> {
        let backend = self.backend.as_ref()?;
        let text: Vec<char> = {
            let term = backend.term.lock();
            let grid = term.grid();
            if point.line < grid.topmost_line() || point.line > grid.bottommost_line() {
                return None;
            }
            let row = &grid[point.line];
            if let Some(link) = row[point.column].hyperlink() {
                return Some(LinkTarget::Url(link.uri().to_string()));
            }
            (0..grid.columns()).map(|c| row[Column(c)].c).collect()
        };
        if let Some(url) = url_at(&text, point.column.0) {
            return Some(LinkTarget::Url(url));
        }
        path_at(&text, point.column.0, &self.display_cwd()).map(LinkTarget::Path)
    }

    fn open_target(&mut self, target: LinkTarget, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Url(url) => cx.emit(TerminalEvent::OpenLink(url)),
            LinkTarget::Path(path) => cx.emit(TerminalEvent::RevealPath(path)),
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.activate(cx);
        if event.modifiers.platform {
            if let Some(target) = self.grid_point(event.position).and_then(|(point, _)| self.link_at(point)) {
                self.open_target(target, cx);
                return;
            }
        }
        self.press_cell = self.grid_point(event.position).map(|(point, _)| point).filter(|_| event.click_count == 1);
        if self.mouse_captured(event.modifiers.shift) {
            if let Some(cell) = self.viewport_cell(event.position) {
                self.mouse_reporting = true;
                self.report_mouse(0, cell, true);
            }
            return;
        }
        let Some((point, side)) = self.grid_point(event.position) else { return };
        let Some(backend) = &self.backend else { return };
        let ty = match event.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        backend.term.lock().selection = Some(Selection::new(ty, point, side));
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button.is_none() {
            self.update_hover(event.position, event.modifiers.platform, cx);
        }
        if self.mouse_reporting && event.pressed_button == Some(MouseButton::Left) {
            let mode = self.mode();
            if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
                if let Some(cell) = self.viewport_cell(event.position).filter(|c| Some(*c) != self.last_mouse_cell) {
                    self.last_mouse_cell = Some(cell);
                    self.report_mouse(32, cell, true);
                }
            }
            return;
        }
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some((point, side)) = self.grid_point(event.position) else { return };
        if let Some(backend) = &self.backend {
            if let Some(selection) = backend.term.lock().selection.as_mut() {
                selection.update(point, side);
            }
        }
        cx.notify();
    }

    /// With ⌘ held, underlines the link under the pointer and shows where a click goes (also over
    /// full-screen apps that take the mouse, like Claude Code).
    fn update_hover(&mut self, position: Point<Pixels>, command: bool, cx: &mut Context<Self>) {
        let found = command.then(|| self.grid_point(position)).flatten().and_then(|(point, _)| {
            let target = self.link_at(point)?;
            let range = match &target {
                LinkTarget::Url(_) => self.url_range(point).or_else(|| self.word_range(point)),
                LinkTarget::Path(_) => self.word_range(point),
            }?;
            Some((range, target))
        });
        let (range, target) = match found {
            Some((range, target)) => (Some(range), Some(target)),
            None => (None, None),
        };
        if range != self.hover_link || target != self.hover_target {
            self.hover_link = range;
            self.hover_target = target;
            cx.notify();
        }
    }

    pub fn debug_hover(&self) -> (Option<(i32, usize, usize)>, Option<LinkTarget>) {
        (self.hover_link, self.hover_target.clone())
    }

    fn on_modifiers_changed(&mut self, event: &gpui::ModifiersChangedEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.update_hover(window.mouse_position(), event.modifiers.platform, cx);
    }

    /// Column range of the URL covering `point`.
    fn url_range(&self, point: GridPoint) -> Option<(i32, usize, usize)> {
        let backend = self.backend.as_ref()?;
        let term = backend.term.lock();
        let grid = term.grid();
        let text: Vec<char> = (0..grid.columns()).map(|c| grid[point.line][Column(c)].c).collect();
        url_ranges(&text)
            .into_iter()
            .find(|(start, end)| (*start..*end).contains(&point.column.0))
            .map(|(start, end)| (point.line.0, start, end))
    }

    /// Line and column range of the non-blank word at `point` (for the link underline).
    fn word_range(&self, point: GridPoint) -> Option<(i32, usize, usize)> {
        let backend = self.backend.as_ref()?;
        let term = backend.term.lock();
        let grid = term.grid();
        let row = &grid[point.line];
        let breaks = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | '<' | '>' | '|');
        let mut start = point.column.0;
        while start > 0 && !breaks(row[Column(start - 1)].c) {
            start -= 1;
        }
        let mut end = point.column.0;
        while end < grid.columns() && !breaks(row[Column(end)].c) {
            end += 1;
        }
        Some((point.line.0, start, end))
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.mouse_reporting) {
            self.last_mouse_cell = None;
            if let Some(cell) = self.viewport_cell(event.position) {
                self.report_mouse(0, cell, false);
            }
            return;
        }
        self.selecting = false;
        let mut empty_selection = true;
        if let Some(backend) = &self.backend {
            let mut term = backend.term.lock();
            if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                term.selection = None;
            }
            empty_selection = term.selection.is_none();
        }
        // A plain click (no drag) on a link opens it too.
        let released = self.grid_point(event.position).map(|(point, _)| point);
        if let (Some(pressed), true) = (self.press_cell.take(), empty_selection) {
            if released == Some(pressed) {
                if let Some(target) = self.link_at(pressed) {
                    self.open_target(target, cx);
                }
            }
        }
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout else { return };
        let delta = event.delta.pixel_delta(layout.line_height).y;
        self.scroll_remainder += delta / layout.line_height;
        let lines = self.scroll_remainder.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;

        let mode = self.mode();
        if self.mouse_captured(event.modifiers.shift) {
            // Wheel as mouse buttons 4/5 (codes 64/65), one report per line.
            if let Some(cell) = self.viewport_cell(event.position) {
                let button = if lines > 0 { 64 } else { 65 };
                for _ in 0..lines.unsigned_abs().min(20) {
                    self.report_mouse(button, cell, true);
                }
            }
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            // Full-screen apps without mouse reporting (less, vim) scroll with arrow keys.
            let arrow: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
            self.write(arrow.repeat(lines.unsigned_abs() as usize));
        } else if let Some(backend) = &self.backend {
            backend.term.lock().scroll_display(Scroll::Delta(lines));
        }
        cx.notify();
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = hex(terminal_theme(cx).background);
        let container = div().id("terminal").key_context("Terminal").track_focus(&self.focus_handle).size_full().bg(background);

        if let Some(error) = &self.error {
            return container.p_4().text_color(hex(Chrome::ERROR)).child(format!("Failed to start terminal: {error}"));
        }

        container
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::select_all))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
                this.drop_paths(paths.paths());
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.hover_link.take().is_some() {
                    this.hover_target = None;
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .relative()
            .child(TerminalElement { view: cx.entity(), focus: self.focus_handle.clone() })
            .children(self.render_link_hint(cx))
    }
}

impl TerminalView {
    /// "⌘-click to open" label under the link the pointer is on.
    fn render_link_hint(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (layout, (line, start, _), target) = (self.layout?, self.hover_link?, self.hover_target.as_ref()?);
        let row = line + layout.display_offset as i32;
        let x = layout.origin.x - layout.bounds_origin.x + layout.cell_width * start as f32;
        let below = layout.origin.y - layout.bounds_origin.y + layout.line_height * (row + 1) as f32 + px(2.);
        // Above the link when it sits on the last rows.
        let y = if below + px(24.) > layout.bounds_height { below - layout.line_height - px(26.) } else { below };
        let label = match target {
            LinkTarget::Url(_) => crate::i18n::t(cx, "terminal.open_link"),
            LinkTarget::Path(path) if path.is_dir() => crate::i18n::t(cx, "terminal.open_folder"),
            LinkTarget::Path(_) => crate::i18n::t(cx, "terminal.reveal_file"),
        };
        Some(
            div()
                .absolute()
                .left(x.max(px(4.)))
                .top(y.max(px(0.)))
                .px_2()
                .py(px(3.))
                .rounded_md()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .shadow_md()
                .text_size(px(crate::ui::Type::SMALL))
                .font_family(".SystemUIFont")
                .text_color(hex(Chrome::BRIGHT))
                .child(label),
        )
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text.as_ref().map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        if let Some(layout) = self.layout {
            let width: usize = text.chars().map(cell_width).sum();
            let previous = self.commit_anchor.filter(|(at, _)| *at == layout.cursor).map_or(0, |(_, w)| w);
            self.commit_anchor = Some((layout.cursor, previous + width));
        }
        self.write_user_input(text.as_bytes().to_vec());
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = (!new_text.is_empty()).then(|| new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let layout = self.layout?;
        let (col, row) = layout.cursor;
        Some(Bounds::new(
            point(layout.origin.x + layout.cell_width * col as f32, layout.origin.y + layout.line_height * row as f32),
            size(layout.cell_width, layout.line_height),
        ))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------------------------

struct TerminalElement {
    view: Entity<TerminalView>,
    focus: FocusHandle,
}

struct Frame {
    hitbox: Hitbox,
    backgrounds: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
    cursor_text: Option<(Point<Pixels>, ShapedLine)>,
    marked: Option<(PaintQuad, Point<Pixels>, ShapedLine)>,
    line_height: Pixels,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    symbol: bool,
}

/// Nerd Font / Powerline glyphs live in Private Use Areas; they are drawn with the bundled
/// symbols font so prompts like Powerlevel10k and Starship render instead of showing boxes.
fn is_symbol_glyph(ch: char) -> bool {
    matches!(ch as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd | 0x23fb..=0x23fe | 0x2b58)
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

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
        let mut style = Style::default();
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
    ) -> Frame {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let prefs = settings(cx).clone();
        let theme = terminal_theme(cx).clone();
        let font_size = px(prefs.font_size);
        let base_font = font(prefs.font_family.clone());
        let symbol_font = font(SYMBOLS_FONT);
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&base_font);
        // Cell metrics like Ghostty: advance width and the font's natural height (ascent + descent),
        // snapped to device pixels so the grid stays crisp and Powerline glyphs meet exactly.
        let scale = window.scale_factor();
        let snap = |value: Pixels| px((f32::from(value) * scale).round() / scale);
        let cell_width = snap(text_system.advance(font_id, font_size, 'm').map(|s| s.width).unwrap_or(px(8.)));
        let natural = text_system.ascent(font_id, font_size) + text_system.descent(font_id, font_size).abs();
        let line_height = snap(natural * prefs.line_height).max(px(1.));
        let padding = px(prefs.padding);
        let origin = point(bounds.origin.x + padding, bounds.origin.y + padding / 2.);

        let grid = GridSize {
            columns: (((bounds.size.width - padding * 2.) / cell_width).floor() as usize).max(2),
            lines: (((bounds.size.height - padding) / line_height).floor() as usize).max(1),
            cell_width: f32::from(cell_width) as u16,
            cell_height: f32::from(line_height) as u16,
        };

        LAST_GRID.with(|g| g.set(grid));
        let focused = self.focus.is_focused(window);
        let mut frame =
            Frame { hitbox, backgrounds: Vec::new(), lines: Vec::new(), cursor: None, cursor_text: None, marked: None, line_height };

        let view = self.view.clone();
        let (marked_text, cursor_visible, search, hover_link, commit_anchor) = {
            let v = view.read(cx);
            (v.marked_text.clone(), v.cursor_visible || !focused, v.search.clone(), v.hover_link, v.commit_anchor)
        };
        let Some(term_handle) = view.update(cx, |view, cx| {
            if !view.spawned {
                view.spawn(grid, cx);
            }
            view.backend.as_mut().map(|backend| {
                backend.resize(grid);
                backend.term.clone()
            })
        }) else {
            return frame;
        };

        let term = term_handle.lock();
        // URLs on screen are drawn blue (and underlined), like links.
        let link_color = hex(Chrome::BLUE);
        let url_cells: std::collections::HashSet<(i32, usize)> = {
            let grid = term.grid();
            let offset = grid.display_offset() as i32;
            let mut cells = std::collections::HashSet::new();
            for row in 0..grid.screen_lines() as i32 {
                let line = Line(row - offset);
                let text: Vec<char> = (0..grid.columns()).map(|c| grid[line][Column(c)].c).collect();
                if !text.windows(3).any(|w| w == [':', '/', '/']) {
                    continue;
                }
                for (start, end) in url_ranges(&text) {
                    cells.extend((start..end).map(|c| (line.0, c)));
                }
            }
            cells
        };
        let content = term.renderable_content();
        let colors = content.colors;
        let display_offset = content.display_offset;
        let selection = content.selection;
        let default_bg = hex(theme.background);

        let make_run = |len: usize, style: CellStyle| TextRun {
            len,
            font: if style.symbol {
                symbol_font.clone()
            } else {
                gpui::Font {
                    weight: if style.bold { FontWeight::BOLD } else { FontWeight::NORMAL },
                    style: if style.italic { FontStyle::Italic } else { FontStyle::Normal },
                    ..base_font.clone()
                }
            },
            color: style.fg,
            background_color: None,
            underline: style.underline.then(|| UnderlineStyle { color: Some(style.fg), thickness: px(1.), wavy: false }),
            strikethrough: style.strike.then(|| StrikethroughStyle { color: Some(style.fg), thickness: px(1.) }),
        };

        // Pending text segment: (row, start column, text, style).
        let mut segment: Option<(usize, usize, String, CellStyle)> = None;
        let mut segment_end = 0;
        let flush = |segment: &mut Option<(usize, usize, String, CellStyle)>, frame: &mut Frame, cells: usize| {
            if let Some((row, col, text, style)) = segment.take() {
                if text.trim().is_empty() && !style.underline && !style.strike {
                    return;
                }
                let run = make_run(text.len(), style);
                // Wide glyphs (CJK) are drawn alone and centered in their two cells.
                let force = (cells == 1).then_some(cell_width);
                let shaped = text_system.shape_line(SharedString::from(text), font_size, &[run], force);
                let slack = if cells > 1 { ((cell_width * cells as f32) - shaped.width).max(px(0.)) / 2. } else { px(0.) };
                let pos = point(origin.x + cell_width * col as f32 + slack, origin.y + line_height * row as f32);
                frame.lines.push((pos, shaped));
            }
        };
        // Pending background span: (row, start column, end column exclusive, color).
        let mut bg_span: Option<(usize, usize, usize, Hsla)> = None;
        let push_bg = |span: Option<(usize, usize, usize, Hsla)>, frame: &mut Frame| {
            if let Some((row, start, end, color)) = span {
                let bounds = Bounds::new(
                    point(origin.x + cell_width * start as f32, origin.y + line_height * row as f32),
                    size(cell_width * (end - start) as f32, line_height),
                );
                frame.backgrounds.push(fill(bounds, color));
            }
        };

        let cursor_point = content.cursor.point;
        let cursor_shape = match (content.cursor.shape, prefs.cursor_shape) {
            // Programs that request a specific shape win; otherwise use the user's preference.
            (CursorShape::Block, CursorShapeSetting::Beam) => CursorShape::Beam,
            (CursorShape::Block, CursorShapeSetting::Underline) => CursorShape::Underline,
            (shape, _) => shape,
        };
        for indexed in content.display_iter {
            let row = (indexed.point.line.0 + display_offset as i32) as usize;
            let col = indexed.point.column.0;
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let width = if wide { 2 } else { 1 };

            let mut fg = resolve_color(cell.fg, colors, &theme, cell.flags.contains(Flags::BOLD));
            let mut bg = resolve_color(cell.bg, colors, &theme, false);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.flags.contains(Flags::DIM) {
                fg.a *= 0.66;
            }
            if let Some((matches, current)) = &search {
                if let Some(index) = matches.iter().position(|m| m.contains(&indexed.point)) {
                    bg = if index == *current { gpui::rgba(0xe8a33dff).into() } else { gpui::rgba(0xe8a33d59).into() };
                    if index == *current {
                        fg = hex(0x1b1b1b);
                    }
                }
            }
            if selection.is_some_and(|s| s.contains(indexed.point)) {
                bg = hex(theme.selection);
            }

            let bg_visible = bg != default_bg;
            match &mut bg_span {
                Some((r, _, end, color)) if *r == row && *end == col && *color == bg && bg_visible => *end += width,
                _ => {
                    push_bg(bg_span.take(), &mut frame);
                    if bg_visible {
                        bg_span = Some((row, col, col + width, bg));
                    }
                }
            }

            let mut ch = cell.c;
            if cell.flags.contains(Flags::HIDDEN) || ch == '\0' || ch == '\t' {
                ch = ' ';
            }
            let symbol = is_symbol_glyph(ch);
            let in_url = url_cells.contains(&(indexed.point.line.0, col));
            let hovered = hover_link.is_some_and(|(line, start, end)| line == indexed.point.line.0 && (start..end).contains(&col));
            if in_url && !selection.is_some_and(|s| s.contains(indexed.point)) {
                fg = link_color;
            }
            let style = CellStyle {
                fg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES) || hovered,
                strike: cell.flags.contains(Flags::STRIKEOUT),
                symbol,
            };
            let isolated = wide || symbol;
            let continues = matches!(&segment, Some((r, _, _, s)) if !isolated && *r == row && *s == style && segment_end == col);
            segment_end = col + width;
            if continues {
                if let Some((_, _, text, _)) = segment.as_mut() {
                    text.push(ch);
                    cell.zerowidth().into_iter().flatten().for_each(|z| text.push(*z));
                }
            } else {
                flush(&mut segment, &mut frame, 1);
                let mut text = ch.to_string();
                cell.zerowidth().into_iter().flatten().for_each(|z| text.push(*z));
                segment = Some((row, col, text, style));
                if isolated {
                    flush(&mut segment, &mut frame, width);
                }
            }

            // While composing (IME), the underlined composition stands in for the cursor.
            if indexed.point == cursor_point && cursor_shape != CursorShape::Hidden && cursor_visible && marked_text.is_none() {
                let cursor_bounds = Bounds::new(
                    point(origin.x + cell_width * col as f32, origin.y + line_height * row as f32),
                    size(cell_width * width as f32, line_height),
                );
                let color = hex(theme.cursor);
                frame.cursor = Some(match (focused, cursor_shape) {
                    (false, _) | (true, CursorShape::HollowBlock) => outline(cursor_bounds, color, BorderStyle::Solid),
                    (true, CursorShape::Beam) => fill(Bounds::new(cursor_bounds.origin, size(px(2.), line_height)), color),
                    (true, CursorShape::Underline) => fill(
                        Bounds::new(point(cursor_bounds.origin.x, cursor_bounds.bottom() - px(2.)), size(cursor_bounds.size.width, px(2.))),
                        color,
                    ),
                    (true, _) => {
                        if ch != ' ' {
                            let run = make_run(ch.len_utf8(), CellStyle { fg: default_bg, ..style });
                            let shaped = text_system.shape_line(ch.to_string().into(), font_size, &[run], None);
                            frame.cursor_text = Some((cursor_bounds.origin, shaped));
                        }
                        fill(cursor_bounds, color)
                    }
                });
            }
        }
        flush(&mut segment, &mut frame, 1);
        push_bg(bg_span.take(), &mut frame);

        let cursor_row = (cursor_point.line.0 + display_offset as i32).max(0) as usize;
        let cursor_col = cursor_point.column.0;
        // Until the program echoes committed text, the cursor hasn't moved: skip past it.
        let pending = commit_anchor.filter(|(at, _)| *at == (cursor_col, cursor_row)).map(|(_, w)| w);
        if let Some(text) = marked_text {
            let fg = hex(theme.foreground);
            let run = TextRun {
                len: text.len(),
                font: base_font.clone(),
                color: fg,
                background_color: None,
                underline: Some(UnderlineStyle { color: Some(fg), thickness: px(1.), wavy: false }),
                strikethrough: None,
            };
            let shaped = text_system.shape_line(text.into(), font_size, &[run], None);
            let pos = point(origin.x + cell_width * (cursor_col + pending.unwrap_or(0)) as f32, origin.y + line_height * cursor_row as f32);
            // Plain background (hides the cells underneath) with just an underline, like other terminals.
            let backdrop = fill(Bounds::new(pos, size(shaped.width, line_height)), hex(theme.background));
            frame.marked = Some((backdrop, pos, shaped));
        }
        drop(term);

        view.update(cx, |view, _| {
            view.layout = Some(Layout {
                bounds_origin: bounds.origin,
                bounds_height: bounds.size.height,
                origin,
                cell_width,
                line_height,
                display_offset,
                cursor: (cursor_col, cursor_row),
            });
            view.focused = focused;
            if pending.is_none() {
                view.commit_anchor = None;
            }
        });
        frame
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Frame,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(&self.focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        let over_link = self.view.read(cx).hover_link.is_some();
        window.set_cursor_style(if over_link { CursorStyle::PointingHand } else { CursorStyle::IBeam }, &frame.hitbox);

        window.paint_layer(bounds, |window| {
            for quad in frame.backgrounds.drain(..) {
                window.paint_quad(quad);
            }
            for (origin, line) in &frame.lines {
                let _ = line.paint(*origin, frame.line_height, window, cx);
            }
            if let Some(cursor) = frame.cursor.take() {
                window.paint_quad(cursor);
            }
            if let Some((origin, line)) = &frame.cursor_text {
                let _ = line.paint(*origin, frame.line_height, window, cx);
            }
            if let Some((backdrop, origin, line)) = frame.marked.take() {
                window.paint_quad(backdrop);
                let _ = line.paint(origin, frame.line_height, window, cx);
            }
        });
    }
}

/// Cells a character takes: 2 for East Asian wide characters (Hangul, CJK, full-width forms).
fn cell_width(c: char) -> usize {
    match c as u32 {
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

fn to_rgb(value: u32) -> Rgb {
    Rgb { r: (value >> 16) as u8, g: (value >> 8) as u8, b: value as u8 }
}

fn rgb_to_hsla(rgb: Rgb) -> Hsla {
    hex(((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32)
}

fn default_color(theme: &TerminalTheme, index: usize) -> u32 {
    match index {
        0..=255 => theme.indexed(index as u8),
        i if i == NamedColor::Background as usize => theme.background,
        i if i == NamedColor::Cursor as usize => theme.cursor,
        _ => theme.foreground,
    }
}

fn resolve_color(color: AnsiColor, colors: &Colors, theme: &TerminalTheme, bold: bool) -> Hsla {
    match color {
        AnsiColor::Spec(rgb) => rgb_to_hsla(rgb),
        AnsiColor::Indexed(index) => {
            let index = if bold && index < 8 { index + 8 } else { index } as usize;
            colors[index].map(rgb_to_hsla).unwrap_or_else(|| hex(theme.indexed(index as u8)))
        }
        AnsiColor::Named(named) => {
            let mut index = named as usize;
            if bold && index < 8 {
                index += 8;
            }
            if let Some(rgb) = colors[index] {
                return rgb_to_hsla(rgb);
            }
            let dim_start = NamedColor::DimBlack as usize;
            let dimmed = |value: u32| {
                let mut c = hex(value);
                c.a = 0.66;
                c
            };
            match index {
                0..=15 => hex(theme.ansi[index]),
                i if (dim_start..dim_start + 8).contains(&i) => dimmed(theme.ansi[i - dim_start]),
                i if i == NamedColor::DimForeground as usize => dimmed(theme.foreground),
                _ => hex(default_color(theme, index)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_symbol_glyph;

    #[test]
    fn shortens_shell_titles() {
        let shorten = |title: &str| {
            let title = title.trim();
            match title.split_once(':') {
                Some((prefix, rest)) if prefix.contains('@') && !prefix.contains(' ') && !rest.is_empty() => rest.trim().to_string(),
                _ => title.to_string(),
            }
        };
        assert_eq!(shorten("ray@mac:~/code"), "~/code");
        assert_eq!(shorten("✳ Claude Code"), "✳ Claude Code");
        assert_eq!(shorten("vim: file.rs"), "vim: file.rs");
    }

    #[test]
    fn wide_characters_take_two_cells() {
        assert_eq!(super::cell_width('한'), 2);
        assert_eq!(super::cell_width('a'), 1);
        assert_eq!(super::cell_width('中'), 2);
    }

    #[test]
    fn detects_powerline_and_nerd_glyphs() {
        assert!(is_symbol_glyph('\u{e0b0}')); // powerline arrow
        assert!(is_symbol_glyph('\u{f418}')); // git branch
        assert!(!is_symbol_glyph('a'));
        assert!(!is_symbol_glyph('한'));
        assert!(!is_symbol_glyph('─')); // box drawing stays in the text font
    }
}

/// "Bash · cargo test" for status labels.
pub fn tool_label(tool: &str, target: Option<&str>) -> String {
    match target.filter(|t| !t.is_empty()) {
        Some(target) => format!("{tool} · {}", target.chars().take(80).collect::<String>()),
        None => tool.to_string(),
    }
}

#[cfg(test)]
mod screen_tests {
    use super::classify_screen;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn detects_busy_and_prompts() {
        let claude = lines("> fix the bug\n\n✽ Pondering… (12s · ↑ 1.2k tokens · esc to interrupt)\n\n─────\n>");
        assert!(classify_screen(&claude).busy);
        let codex = lines("• Working (5s • esc to interrupt)\n› ");
        assert!(classify_screen(&codex).busy);
        let permission = lines(
            " Bash command\n   rm -rf target\n Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently (esc)",
        );
        let state = classify_screen(&permission);
        assert_eq!(state.permission.as_deref(), Some("Do you want to proceed?"));
        assert!(!state.busy);
        let codex_approval = lines("Would you like to run the following command?\n  $ cargo test\n› 1. Yes, proceed (y)\n  2. No (esc)");
        assert!(classify_screen(&codex_approval).permission.is_some());
        let question = lines(" Which approach?\n ❯ 1. Fast\n   2. Safe\n Enter to select · ↑/↓ to navigate · Esc to cancel");
        let state = classify_screen(&question);
        assert!(state.question && state.permission.is_none());
        let interrupted = lines("  ⎿  Interrupted · What should Claude do instead?\n────\n❯\n────\n  auto mode on\n\n\n\n\n");
        assert!(classify_screen(&interrupted).interrupted);
        let old = lines(
            "  ⎿  Interrupted · What should Claude do instead?\n❯ next\n⏺ one\n two\n three\n four\n five\n six\n seven\n────\n❯\n────",
        );
        assert!(!classify_screen(&old).interrupted);
    }

    #[test]
    fn quoted_prompts_are_not_permission_prompts() {
        // Conversation text mentioning the phrase, without an option list.
        let chat = lines("⏺ The dialog says \"Do you want to proceed?\" and I added esc to interrupt handling.\n>");
        let state = classify_screen(&chat);
        assert!(state.permission.is_none() && !state.busy);
    }
}

/// xterm mouse report bytes. `button`: 0 left, 32 = motion with left held, 64/65 wheel up/down.
fn mouse_report(button: u8, (column, row): (usize, usize), pressed: bool, sgr: bool) -> Option<Vec<u8>> {
    if sgr {
        let suffix = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{button};{};{}{suffix}", column + 1, row + 1).into_bytes());
    }
    // X10 encoding can't express positions past 223 or which button was released.
    let code = if pressed { button } else { 3 };
    let (x, y) = (column + 1 + 32, row + 1 + 32);
    (x <= 255 && y <= 255).then(|| vec![0x1b, b'[', b'M', 32 + code, x as u8, y as u8])
}

#[cfg(test)]
mod mouse_tests {
    use super::mouse_report;

    #[test]
    fn encodes_sgr_and_legacy_reports() {
        assert_eq!(mouse_report(64, (4, 9), true, true).unwrap(), b"\x1b[<64;5;10M");
        assert_eq!(mouse_report(0, (0, 0), false, true).unwrap(), b"\x1b[<0;1;1m");
        assert_eq!(mouse_report(65, (0, 0), true, false).unwrap(), vec![0x1b, b'[', b'M', 32 + 65, 33, 33]);
        assert!(mouse_report(0, (300, 0), true, false).is_none());
    }
}

/// Something clickable in terminal text.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    Url(String),
    Path(PathBuf),
}

/// Column ranges (start inclusive, end exclusive) of URLs in a line, for highlighting.
pub fn url_ranges(line: &[char]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut column = 0;
    while column < line.len() {
        let rest: String = line[column..].iter().collect();
        let Some(offset) = ["https://", "http://", "file://"].iter().filter_map(|s| rest.find(s)).min() else { break };
        let start = column + rest[..offset].chars().count();
        match url_at(line, start) {
            Some(url) => {
                let end = start + url.chars().count();
                ranges.push((start, end));
                column = end.max(start + 1);
            }
            None => column = start + 1,
        }
    }
    ranges
}

/// A path-like word covering `column`: `/abs/file`, `~/dir`, `./x`, `src/main.rs:12:4`. It is
/// resolved against `cwd` and must exist.
pub fn path_at(line: &[char], column: usize, cwd: &std::path::Path) -> Option<PathBuf> {
    let is_break = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '<' | '>' | '|' | ',');
    if column >= line.len() || is_break(line[column]) {
        return None;
    }
    let mut start = column;
    while start > 0 && !is_break(line[start - 1]) {
        start -= 1;
    }
    let mut end = column;
    while end < line.len() && !is_break(line[end]) {
        end += 1;
    }
    let word: String = line[start..end].iter().collect();
    let word = word.trim_end_matches(['.', ':', ';', '!', '?']);
    if word.contains("://") {
        return None;
    }
    // Drop `:line[:col]` suffixes from compiler / grep output.
    let mut candidate = word;
    for _ in 0..2 {
        if let Some((head, tail)) = candidate.rsplit_once(':') {
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
                candidate = head;
            }
        }
    }
    if candidate.is_empty() || !(candidate.contains('/') || candidate.contains('.')) || candidate.chars().all(|c| c == '.' || c == '/') {
        return None;
    }
    let expanded = match candidate.strip_prefix("~/") {
        Some(rest) => crate::launch::home_dir().join(rest),
        None if candidate == "~" => crate::launch::home_dir(),
        None if candidate.starts_with('/') => PathBuf::from(candidate),
        None => cwd.join(candidate),
    };
    expanded.exists().then_some(expanded)
}

/// A URL (http, https, file) covering `column` in a line of text.
pub fn url_at(line: &[char], column: usize) -> Option<String> {
    let text: String = line.iter().collect();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let char_col = column.min(chars.len().saturating_sub(1));
    for scheme in ["https://", "http://", "file://"] {
        let mut search = 0;
        while let Some(found) = text[search..].find(scheme) {
            let start_byte = search + found;
            let start = chars.iter().position(|(b, _)| *b == start_byte)?;
            let mut end = start;
            while end < chars.len() && !chars[end].1.is_whitespace() && !matches!(chars[end].1, '"' | '\'' | '<' | '>' | '`' | '|') {
                end += 1;
            }
            // Trailing punctuation usually ends a sentence rather than the URL.
            while end > start && matches!(chars[end - 1].1, '.' | ',' | ';' | ':' | ')' | ']' | '}' | '!' | '?') {
                end -= 1;
            }
            if (start..end).contains(&char_col) && end > start + scheme.len() {
                return Some(chars[start..end].iter().map(|(_, c)| c).collect());
            }
            search = start_byte + scheme.len();
        }
    }
    None
}

#[cfg(test)]
mod link_tests {
    use super::{path_at, url_at, url_ranges};

    #[test]
    fn finds_url_ranges_and_paths() {
        let line: Vec<char> = "see https://a.dev/x and http://localhost:3000 now".chars().collect();
        assert_eq!(url_ranges(&line), vec![(4, 19), (24, 45)]);
        let dir = std::env::temp_dir().join(format!("agentty-paths-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        let out: Vec<char> = "error: src/main.rs:12:4 mismatched".chars().collect();
        assert_eq!(path_at(&out, 10, &dir), Some(dir.join("src/main.rs")));
        assert_eq!(path_at(&out, 2, &dir), None);
        let missing: Vec<char> = "nope/nothing.txt".chars().collect();
        assert_eq!(path_at(&missing, 3, &dir), None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn finds_url_under_column() {
        let line: Vec<char> = "  ➜  Local:   http://localhost:5173/, see (https://a.dev/x).".chars().collect();
        assert_eq!(url_at(&line, 20).as_deref(), Some("http://localhost:5173/"));
        assert_eq!(url_at(&line, 50).as_deref(), Some("https://a.dev/x"));
        assert_eq!(url_at(&line, 2), None);
    }
}
