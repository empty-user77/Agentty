//! The session page's workflow view, laid out like omo's: the threads on the left (every agent
//! session running in Agentty, and the parallel runs agents started with `agentty tasks`); for the
//! one picked, what was asked (`$ …`), the orchestrator's state, and its subagents as a graph of
//! waves — those started together form a wave — each node saying whether it is done, how long it
//! took, what kind of agent on which model, and its last word; under it a feed of their latest
//! steps. The connection map the page always had is one click away in its header.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{hint, icon, Type, TypeScale};
use agentty_bridge::claude::SubagentFile;
use agentty_bridge::subagent_trace::{Step, TraceReader};
use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, ClickEvent, Context, Entity, PathBuilder, Pixels, Point, SharedString, Subscription,
    Window,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MONO: &str = "JetBrains Mono";
/// The graph's own colours, after omo's: cyan for what runs, green for what is done.
const RUNNING: u32 = 0x22d3ee;
const DONE: u32 = 0x34d399;
const NODE_W: f32 = 196.;
const NODE_H: f32 = 100.;
const COL_GAP: f32 = 64.;
const ROW_GAP: f32 = 14.;
/// Room above the nodes for a wave's title.
const WAVE_HEAD: f32 = 30.;
/// How far the chart zooms out and in, and one step of the buttons (as on the connection map).
const ZOOM_RANGE: (f32, f32) = (0.3, 2.);
const ZOOM_STEP: f32 = 1.2;
/// Subagents started within this long of the one before them are one wave.
const WAVE_GAP_MS: u64 = 8_000;
/// Claude Code reports background subagents stopped while they still work: such a report counts
/// once the transcript has been quiet this long.
const QUIET_MS: u64 = 20_000;
const CUT: u32 = super::orchestration::CUT;

/// Which view of the session page is on screen.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionTab {
    #[default]
    Workflow,
    Links,
}

/// The thread picked on the left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Thread {
    /// An orchestration, by its record's id.
    Orchestration(u64),
    /// An agent session, by its pane.
    Session(u64),
    /// A parallel run, by its id.
    Run(u64),
}

/// One `agentty tasks` request the user let start.
pub struct TaskRun {
    id: u64,
    pub started_ms: u64,
    /// The pane of the agent that asked.
    pub caller: u64,
    /// Where the agent that asked works (its workspace), as it was called then.
    pub caller_name: String,
    pub tasks: Vec<RunTask>,
}

pub struct RunTask {
    pub title: String,
    pub branch: Option<String>,
    pub folder: PathBuf,
    /// Its pane, while it is open.
    pub pane: u64,
}

/// The picked session's subagent transcripts, read as they grow.
#[derive(Default)]
struct Followed {
    files: Vec<SubagentFile>,
    traces: Arc<Mutex<HashMap<String, TraceReader>>>,
}

#[derive(Default)]
pub struct SessionBoard {
    pub tab: SessionTab,
    pub runs: Vec<TaskRun>,
    next_run: u64,
    pub selected: Option<Thread>,
    /// Subagent transcripts by session: the picked one's and every orchestration's.
    watched: HashMap<String, Followed>,
    /// Orchestrations, as kept on disk (read on first use).
    pub records: Vec<super::orchestration::Record>,
    pub records_loaded: bool,
    /// The page is being kept up to date (one loop at a time).
    ticking: bool,
    /// The message box under the picked session.
    message: Option<(u64, Entity<TextInput>, Subscription)>,
    /// The chart's view: zoom (0 until set: 100%), where it is moved to, a drag moving it, the
    /// canvas it was last drawn on, and the chart's own size (for Fit).
    zoom: f32,
    pub pan: Point<Pixels>,
    pan_drag: Option<(Point<Pixels>, Point<Pixels>)>,
    canvas: std::rc::Rc<std::cell::Cell<gpui::Bounds<Pixels>>>,
    chart_size: std::cell::Cell<(f32, f32)>,
    /// The thread the view was set for: another one starts at 100%, unmoved.
    view_of: Option<Thread>,
    /// A line on screen is running: the page is redrawn often enough for it to move smoothly.
    animating: std::cell::Cell<bool>,
}

impl SessionBoard {
    /// The orchestrations to show: none while orchestration is switched off in the settings.
    pub fn shown(&self, on: bool) -> &[super::orchestration::Record] {
        if on {
            &self.records
        } else {
            &[]
        }
    }

    fn zoom(&self) -> f32 {
        if self.zoom > 0. {
            self.zoom
        } else {
            1.
        }
    }

    /// Zooms to `zoom`, keeping the chart point under `anchor` (a window position) where it is.
    fn zoom_at(&mut self, zoom: f32, anchor: Point<Pixels>) {
        let origin = self.canvas.get().origin;
        let old = self.zoom();
        let at = point((anchor.x - origin.x - self.pan.x) / old, (anchor.y - origin.y - self.pan.y) / old);
        let zoom = if zoom.is_finite() { zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1) } else { 1. };
        self.zoom = zoom;
        self.pan = point(anchor.x - origin.x - at.x * zoom, anchor.y - origin.y - at.y * zoom);
    }

    /// The whole chart in view (never above 100%), from its top left.
    fn fit(&mut self) {
        let size = self.canvas.get().size;
        let (w, h) = self.chart_size.get();
        let (vw, vh) = (f32::from(size.width), f32::from(size.height));
        if w <= 0. || h <= 0. || vw <= 0. || vh <= 0. {
            return;
        }
        self.zoom = ((vw - 16.) / w).min((vh - 16.) / h).clamp(ZOOM_RANGE.0, 1.);
        self.pan = point(px(8.), px(8.));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum NodeState {
    Done,
    Running,
    Pending,
    /// Cut off: its process ended before it did.
    Stopped,
}

/// A node of the graph, as drawn.
struct Node {
    name: String,
    state: NodeState,
    seconds: u64,
    role: String,
    model: Option<String>,
    line: Option<String>,
    open: Open,
}

/// One line of the feed: when, who, on which model, what it did, and where that agent is.
type FeedLine = (u64, String, Option<String>, String, NodeState);

/// What a click on a node opens.
#[derive(Clone)]
enum Open {
    /// Nothing: its terminal is closed.
    Nothing,
    /// The subagents window of a pane, at one subagent.
    Subagent(Pane, String),
    Pane(Pane),
}

struct Wave {
    title: String,
    nodes: Vec<Node>,
}

/// "45s", "12m", "1h 05m".
fn span(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// The first line of some text, at most `max` characters.
fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_start_matches(['#', '*', '-', '>', ' ']).replace("**", "").replace('`', "");
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max - 1).collect::<String>())
    } else {
        line
    }
}

/// A step in words: a Bash call's own description, or the call.
fn step_words(step: &Step) -> String {
    match (&step.tool, &step.detail) {
        (Some(tool), Some(why)) if matches!(tool.as_str(), "Bash" | "PowerShell") => first_line(why, 80),
        (Some(tool), _) => crate::terminal::tool_label(tool, Some(step.target.as_str())),
        (None, _) => first_line(&step.target, 80),
    }
}

/// Groups starts (ms, in order) into waves: a new one when the gap to the previous is long.
fn waves_of(starts: &[u64]) -> Vec<usize> {
    let mut wave = 0;
    let mut out = Vec::with_capacity(starts.len());
    for (i, at) in starts.iter().enumerate() {
        if i > 0 && at.saturating_sub(starts[i - 1]) > WAVE_GAP_MS {
            wave += 1;
        }
        out.push(wave);
    }
    out
}

/// Edges between two neighbouring waves: every pair while that stays readable, else each node to
/// the one level with it.
fn edges_between(left: usize, right: usize) -> Vec<(usize, usize)> {
    if left == 0 || right == 0 {
        return Vec::new();
    }
    if left * right <= 12 {
        (0..left).flat_map(|a| (0..right).map(move |b| (a, b))).collect()
    } else {
        (0..right).map(|b| (b.min(left - 1), b)).collect()
    }
}

/// Where a subagent is: done when its transcript says so (a final answer, or a report handed back
/// to its caller), or when Claude Code reported it stopped and its transcript has been quiet since
/// (Claude Code also reports background agents stopped while they still work, so the report alone
/// is not enough); cut off when the process it ran in has gone; else running.
fn sub_state(trace: Option<&agentty_bridge::subagent_trace::Trace>, reported_stopped: bool, now_ms: u64) -> NodeState {
    let Some(trace) = trace else { return if reported_stopped { NodeState::Done } else { NodeState::Running } };
    let quiet = trace.last_ms.is_none_or(|at| now_ms.saturating_sub(at) > QUIET_MS);
    if trace.finished || (reported_stopped && quiet) {
        NodeState::Done
    } else if trace.stopped {
        NodeState::Stopped
    } else {
        NodeState::Running
    }
}

/// The orchestrator's node for an orchestration's stage.
fn stage_node(stage: super::orchestration::Stage) -> NodeState {
    use super::orchestration::Stage;
    match stage {
        Stage::Planning | Stage::Running => NodeState::Running,
        Stage::Interrupted | Stage::Stopped => NodeState::Stopped,
        Stage::Ready | Stage::Done => NodeState::Done,
    }
}

fn state_color(state: NodeState) -> u32 {
    match state {
        NodeState::Done => DONE,
        NodeState::Running => RUNNING,
        NodeState::Pending => Chrome::MUTED,
        NodeState::Stopped => CUT,
    }
}

impl Workbench {
    /// The page's header switch: the workflow view, or the connection map.
    pub(super) fn render_session_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.session_board.tab;
        let tab = |id: &'static str, value: SessionTab, glyph: &'static str, label: &'static str, cx: &mut Context<Self>| {
            let on = current == value;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1p5()
                .px_2p5()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .when(on, |d| d.bg(hex(Chrome::SELECTED)).text_color(hex(Chrome::BRIGHT)))
                .when(!on, |d| d.text_color(hex(Chrome::MUTED)).hover(|s| s.bg(hex(Chrome::HOVER))))
                .child(icon(glyph, 12., hex(if on { Chrome::BRIGHT } else { Chrome::MUTED })))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.session_board.tab = value;
                    cx.notify();
                }))
        };
        div()
            .flex()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded_lg()
            .bg(hex_alpha(Chrome::MUTED, 0.12))
            .child(tab("session-tab-workflow", SessionTab::Workflow, "list-todo", t(cx, "sessions.tab_workflow"), cx))
            .child(tab("session-tab-links", SessionTab::Links, "link", t(cx, "sessions.tab_links"), cx))
            .into_any_element()
    }

    /// "New orchestrator", at the right of the page's header.
    pub(super) fn render_new_orchestrator_button(&self, cx: &mut Context<Self>) -> AnyElement {
        if !crate::settings::settings(cx).orchestration {
            return div().into_any_element();
        }
        div()
            .id("orch-new")
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .bg(hex(Chrome::ACCENT))
            .text_color(hex(Chrome::ON_ACCENT))
            .t_small()
            .hover(|s| s.opacity(0.85))
            .child(icon("plus", 12., hex(Chrome::ON_ACCENT)))
            .child(t(cx, "orch.new"))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.open_orchestrator_dialog(super::orchestrator::Place::NewWorkspace { group: None }, window, cx)
            }))
            .into_any_element()
    }

    /// Orchestrations and sessions on the left, the picked one's workflow on the right.
    pub(super) fn render_workflow_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.keep_orchestrations(cx);
        let on = crate::settings::settings(cx).orchestration;
        let sessions: Vec<Pane> = self
            .all_panes()
            .into_iter()
            .filter(|p| p.read(cx).is_agent() && p.read(cx).is_running() && self.record_of_pane(p, cx).is_none())
            .collect();
        let still_there = match self.session_board.selected {
            Some(Thread::Orchestration(id)) => self.session_board.shown(on).iter().any(|r| r.id == id),
            Some(Thread::Session(id)) => sessions.iter().any(|p| p.read(cx).pane_id == id),
            Some(Thread::Run(id)) => self.session_board.runs.iter().any(|r| r.id == id),
            None => false,
        };
        if !still_there {
            // The latest orchestration that is not over, else the latest one, else the busiest session.
            use super::orchestration::Stage;
            let open = self.session_board.shown(on).iter().rev().find(|r| !matches!(r.stage, Stage::Done | Stage::Stopped));
            let busiest = sessions.iter().max_by_key(|p| {
                let view = p.read(cx);
                (view.subagents.iter().filter(|r| r.finished.is_none()).count(), view.status.in_turn(), view.subagents.len())
            });
            self.session_board.selected = open
                .or(self.session_board.shown(on).last())
                .map(|r| Thread::Orchestration(r.id))
                .or_else(|| busiest.map(|p| Thread::Session(p.read(cx).pane_id)));
        }
        self.follow_session_board(cx);
        if self.session_board.view_of != self.session_board.selected {
            let board = &mut self.session_board;
            (board.view_of, board.zoom, board.pan) = (board.selected, 1., point(px(0.), px(0.)));
        }

        let threads = self.render_threads(&sessions, cx);
        let body = match self.session_board.selected {
            Some(Thread::Orchestration(id)) => {
                let record = self.session_board.shown(on).iter().find(|r| r.id == id).cloned();
                match record.as_ref().and_then(|r| self.pane_of_record(r, cx)) {
                    Some(pane) => self.render_session_workflow(&pane, cx),
                    None => record.map_or_else(|| div().into_any_element(), |r| self.render_closed_orchestration(&r, cx)),
                }
            }
            Some(Thread::Session(id)) => match sessions.iter().find(|p| p.read(cx).pane_id == id) {
                Some(pane) => self.render_session_workflow(pane, cx),
                None => div().into_any_element(),
            },
            Some(Thread::Run(id)) => self.render_run_workflow(id, cx),
            None => div()
                .p_6()
                .flex()
                .flex_col()
                .items_start()
                .gap_3()
                .child(hint(t(cx, "sessions.no_agents")))
                .child(self.render_new_orchestrator_button(cx))
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .font_family(MONO)
            .child(threads)
            .child(div().w(px(1.)).h_full().bg(hex(Chrome::BORDER)))
            .child(div().flex_1().min_w_0().h_full().overflow_hidden().child(body))
            .into_any_element()
    }

    /// The list on the left: orchestrations (with Delete), other agent sessions, parallel runs —
    /// each with a dot in its state's colour.
    fn render_threads(&self, sessions: &[Pane], cx: &mut Context<Self>) -> AnyElement {
        let selected = self.session_board.selected;
        let on = crate::settings::settings(cx).orchestration;
        let row = |id: SharedString, label: String, sub: Option<String>, color: u32, on: bool| {
            div()
                .id(id)
                .group("thread-row")
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .when(on, |d| d.bg(hex(Chrome::SELECTED)))
                .when(!on, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
                .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(color)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div().truncate().t_small().text_color(hex(if on { Chrome::BRIGHT } else { Chrome::FOREGROUND })).child(label),
                        )
                        .children(sub.map(|s| div().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(s))),
                )
        };
        let heading =
            |key: &'static str, cx: &gpui::App| div().px_2().pt_2().pb_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, key));
        let mut list =
            div().id("session-threads").w(px(230.)).flex_shrink_0().h_full().overflow_y_scroll().p_2().flex().flex_col().gap_0p5();

        if on {
            list = list.child(heading("orch.list", cx));
        }
        if on && self.session_board.shown(on).is_empty() {
            list = list.child(div().px_2().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "orch.list_empty")));
        }
        for record in self.session_board.shown(on).iter().rev() {
            let id = record.id;
            let pane = self.pane_of_record(record, cx);
            let team = self.team_of(&record.session, pane.as_ref(), cx);
            let (_, color) = self.stage_words(record.stage, team, cx);
            let stage = t(cx, super::orchestration::stage_key(record.stage));
            let sub = if team.all > 0 { format!("{stage} · {}/{}", team.done, team.all) } else { stage.to_string() };
            list = list.child(
                row(
                    SharedString::from(format!("thread-orch-{id}")),
                    format!("◆ {}", first_line(&record.goal, 60)),
                    Some(sub),
                    color,
                    selected == Some(Thread::Orchestration(id)),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("thread-orch-delete-{id}")))
                        .flex_shrink_0()
                        .p_0p5()
                        .rounded_sm()
                        .invisible()
                        .group_hover("thread-row", |s| s.visible())
                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "orch.delete"), None))
                        .child(icon("trash-2", 12., hex(Chrome::MUTED)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.confirm_delete_orchestration(id, cx);
                        })),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.session_board.selected = Some(Thread::Orchestration(id));
                    cx.notify();
                })),
            );
        }

        list = list.child(heading("sessions.threads", cx));
        if sessions.is_empty() {
            list = list.child(div().px_2().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "sessions.no_agents_short")));
        }
        for pane in sessions {
            let view = pane.read(cx);
            let id = view.pane_id;
            let (_, color) = super::status_label_sized(view, true, cx);
            let running = view.subagents.iter().filter(|r| r.finished.is_none()).count();
            let place = self.locate(pane).map(|(w, _)| self.workspace_label(&self.workspaces[w], cx));
            let sub = match (place, running) {
                (Some(place), 0) => Some(place),
                (Some(place), n) => Some(format!("{place} · {}", tf(cx, "sessions.subagents_running", &[("n", &n.to_string())]))),
                (None, _) => None,
            };
            list = list.child(
                row(
                    SharedString::from(format!("thread-session-{id}")),
                    view.display_title(),
                    sub,
                    color,
                    selected == Some(Thread::Session(id)),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.session_board.selected = Some(Thread::Session(id));
                    cx.notify();
                })),
            );
        }
        if !self.session_board.runs.is_empty() {
            list = list.child(heading("sessions.runs", cx));
            for run in self.session_board.runs.iter().rev() {
                let id = run.id;
                let (done, all) = self.run_progress(run, cx);
                let color = if done == all { DONE } else { RUNNING };
                list = list.child(
                    row(
                        SharedString::from(format!("thread-run-{id}")),
                        tf(cx, "sessions.run_title", &[("n", &all.to_string())]),
                        Some(format!("{} · {done}/{all}", run.caller_name)),
                        color,
                        selected == Some(Thread::Run(id)),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.session_board.selected = Some(Thread::Run(id));
                        cx.notify();
                    })),
                );
            }
        }
        list.into_any_element()
    }

    /// An orchestration whose terminal is closed: its goal, its stage, Resume (or Open) and Delete.
    fn render_closed_orchestration(&self, record: &super::orchestration::Record, cx: &mut Context<Self>) -> AnyElement {
        use super::orchestration::Stage;
        let id = record.id;
        let team = self.team_of(&record.session, None, cx);
        let (said, color) = self.stage_words(record.stage, team, cx);
        let (nodes, feed) = self.team_nodes(Some(&record.session), None, crate::ui::now_ms(), cx);
        let last = nodes.iter().map(|(at, n)| at + n.seconds * 1000).max().unwrap_or(record.started_ms);
        let orchestrator = Node {
            name: "orchestrator".into(),
            state: stage_node(record.stage),
            seconds: last.saturating_sub(record.started_ms) / 1000,
            role: "Claude Code".into(),
            model: None,
            line: Some(t(cx, super::orchestration::stage_key(record.stage)).to_string()),
            open: Open::Nothing,
        };
        let waves = self.waves_after(orchestrator, nodes, cx);
        let buttons = if matches!(record.stage, Stage::Interrupted | Stage::Stopped) {
            self.stage_buttons(id, record.stage, "closed", cx)
        } else {
            // Over (or never started): its session opens again, to read it or ask more.
            vec![self
                .board_action(
                    SharedString::from(format!("closed-reopen-{id}")),
                    "square-terminal",
                    t(cx, "orch.reopen"),
                    Chrome::FOREGROUND,
                    cx,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.reopen_orchestration(id, window, cx)))
                .into_any_element()]
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(format!("$ {}", first_line(&record.goal, 120))))
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).truncate().child(record.folder.display().to_string())),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .t_small()
                    .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(color)))
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(said))
                    .children(buttons)
                    .child(
                        self.board_action(
                            SharedString::from(format!("closed-delete-{id}")),
                            "trash-2",
                            t(cx, "orch.delete"),
                            Chrome::MUTED,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.confirm_delete_orchestration(id, cx))),
                    ),
            )
            .child(div().px_5().pb_1().child(hint(t(cx, "orch.closed_hint"))))
            .child(self.render_waves(waves, cx))
            .child(self.render_feed(feed))
            .into_any_element()
    }

    /// A team's subagents as nodes (with when each started), and the feed of their latest steps:
    /// from the transcripts read for `session`, and — while its pane is open — the hooks' word for
    /// one whose transcript has not shown up yet.
    fn team_nodes(&self, session: Option<&str>, pane: Option<&Pane>, now_ms: u64, cx: &gpui::App) -> (Vec<(u64, Node)>, Vec<FeedLine>) {
        let none = Followed::default();
        let followed = session.and_then(|s| self.session_board.watched.get(s)).unwrap_or(&none);
        let traces = followed.traces.lock().unwrap_or_else(|e| e.into_inner());
        let mut files: Vec<&SubagentFile> = followed.files.iter().collect();
        files.sort_by_key(|f| traces.get(&f.id).and_then(|r| r.trace.started_ms).unwrap_or(u64::MAX));
        let hooks: &[crate::terminal::SubagentRun] = pane.map_or(&[], |p| p.read(cx).subagents.as_slice());
        let reported_stopped = |id: &str| hooks.iter().any(|r| r.id == id && r.finished.is_some());
        let open = |id: &str| pane.map_or(Open::Nothing, |p| Open::Subagent(p.clone(), id.to_string()));
        let mut nodes: Vec<(u64, Node)> = Vec::new();
        for file in &files {
            let trace = traces.get(&file.id).map(|r| &r.trace);
            let started = trace.and_then(|t| t.started_ms).unwrap_or(now_ms);
            let state = sub_state(trace, reported_stopped(&file.id), now_ms);
            let done = state != NodeState::Running;
            let end = if done { trace.and_then(|t| t.last_ms).unwrap_or(now_ms) } else { now_ms };
            let line = trace.and_then(|t| {
                if done {
                    // Its report, or what it handed back.
                    t.final_answer().or_else(|| t.steps.iter().rev().find(|s| s.tool.is_some())).map(|s| first_line(&step_words(s), 60))
                } else {
                    t.current().or_else(|| t.steps.iter().rev().find(|s| s.tool.is_some())).map(step_words)
                }
            });
            nodes.push((
                started,
                Node {
                    name: if file.description.is_empty() { file.agent_type.clone() } else { file.description.clone() },
                    state,
                    seconds: end.saturating_sub(started) / 1000,
                    role: file.agent_type.clone(),
                    model: file.model.clone(),
                    line,
                    open: open(&file.id),
                },
            ));
        }
        for run in hooks {
            if files.iter().any(|f| f.id == run.id) {
                continue;
            }
            nodes.push((
                now_ms.saturating_sub(run.started.elapsed().as_millis() as u64),
                Node {
                    name: run.task.clone().unwrap_or_else(|| run.kind.clone()),
                    state: if run.finished.is_some() { NodeState::Done } else { NodeState::Running },
                    seconds: run.finished.unwrap_or_else(Instant::now).duration_since(run.started).as_secs(),
                    role: run.kind.clone(),
                    model: None,
                    line: run.last_tool.as_ref().map(|(tool, target)| crate::terminal::tool_label(tool, target.as_deref())),
                    open: open(&run.id),
                },
            ));
        }
        nodes.sort_by_key(|(at, _)| *at);

        let mut feed: Vec<FeedLine> = Vec::new();
        for file in &files {
            let Some(reader) = traces.get(&file.id) else { continue };
            let state = sub_state(Some(&reader.trace), reported_stopped(&file.id), now_ms);
            for step in reader.trace.steps.iter().rev().take(3) {
                let name = if file.description.is_empty() { file.agent_type.clone() } else { file.description.clone() };
                feed.push((step.start_ms, name, file.model.clone(), step_words(step), state));
            }
        }
        feed.sort_by_key(|(at, ..)| *at);
        let feed = feed.into_iter().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect();
        (nodes, feed)
    }

    /// The orchestrator's column, then a column per wave of `nodes`, each titled with how far it got.
    fn waves_after(&self, orchestrator: Node, nodes: Vec<(u64, Node)>, cx: &gpui::App) -> Vec<Wave> {
        let starts: Vec<u64> = nodes.iter().map(|(at, _)| *at).collect();
        let wave_of = waves_of(&starts);
        let mut waves = vec![Wave { title: t(cx, "sessions.orchestrator").to_string(), nodes: vec![orchestrator] }];
        for ((_, node), wave) in nodes.into_iter().zip(wave_of) {
            if waves.len() <= wave + 1 {
                waves.push(Wave { title: String::new(), nodes: Vec::new() });
            }
            waves[wave + 1].nodes.push(node);
        }
        for (index, wave) in waves.iter_mut().enumerate().skip(1) {
            let done = wave.nodes.iter().filter(|n| n.state == NodeState::Done).count();
            let running = wave.nodes.iter().filter(|n| n.state == NodeState::Running).count();
            let (n, all) = (index.to_string(), wave.nodes.len().to_string());
            wave.title = if running > 0 {
                tf(cx, "sessions.wave_running", &[("n", &n), ("done", &done.to_string()), ("all", &all), ("running", &running.to_string())])
            } else {
                tf(cx, "sessions.wave", &[("n", &n), ("done", &done.to_string()), ("all", &all)])
            };
        }
        waves
    }

    /// A session: what was asked, the orchestrator's state, its subagents as waves, the feed.
    fn render_session_workflow(&self, pane: &Pane, cx: &mut Context<Self>) -> AnyElement {
        use crate::terminal::AgentStatus;
        let view = pane.read(cx);
        let pane_id = view.pane_id;
        let now_ms = crate::ui::now_ms();
        let record = self.record_of_pane(pane, cx).cloned();
        let session = view.current_session();
        let (nodes, feed) = self.team_nodes(session.as_deref(), Some(pane), now_ms, cx);

        // The orchestrator first, then the waves.
        use super::orchestration::Stage;
        let (state_label, state_color_main) = super::status_label_sized(view, true, cx);
        let main_running = view.status.in_turn();
        let waiting_on_user = view.status.needs_user();
        // Resting while its agents work is still running the team.
        let team_running = nodes.iter().any(|(_, n)| n.state == NodeState::Running);
        let orchestrator = Node {
            name: "orchestrator".into(),
            state: match record.as_ref() {
                Some(r) => stage_node(r.stage),
                None if main_running || team_running => NodeState::Running,
                None => NodeState::Done,
            },
            seconds: view.agent_since_ms.map_or(0, |since| now_ms.saturating_sub(since) / 1000),
            role: match view.agent_kind() {
                Some(crate::launch::PaneKind::Codex) => "Codex".into(),
                _ => "Claude Code".into(),
            },
            model: view.stats.as_ref().and_then(|s| s.model.clone()),
            line: match &view.status {
                AgentStatus::Working | AgentStatus::Thinking => {
                    view.last_tool.as_ref().map(|(tool, target)| crate::terminal::tool_label(tool, target.as_deref()))
                }
                _ => Some(
                    record.as_ref().map_or_else(|| state_label.clone(), |r| t(cx, super::orchestration::stage_key(r.stage)).to_string()),
                ),
            },
            open: Open::Pane(pane.clone()),
        };
        let waves = self.waves_after(orchestrator, nodes, cx);
        let subagents: Vec<&Node> = waves.iter().skip(1).flat_map(|w| &w.nodes).collect();
        let (done, all) = (subagents.iter().filter(|n| n.state == NodeState::Done).count(), subagents.len());
        let models = {
            let mut models: Vec<&str> = waves.iter().flat_map(|w| &w.nodes).filter_map(|n| n.model.as_deref()).collect();
            models.sort_unstable();
            models.dedup();
            models.len()
        };

        let asked = record
            .as_ref()
            .map(|r| first_line(&r.goal, 120))
            .or_else(|| view.last_prompt.clone().map(|p| first_line(p.strip_prefix("Goal: ").unwrap_or(&p), 120)));
        // An orchestration says its stage in the words of its pane's strip; another session, its state.
        let (said, said_color) = match &record {
            Some(r) if r.stage == Stage::Ready => (t(cx, "orch.plan_ready").to_string(), RUNNING),
            Some(r) => {
                let team = self.team_of(&r.session, Some(pane), cx);
                self.stage_words(r.stage, team, cx)
            }
            None => (format!("orchestrator · {state_label}"), state_color_main),
        };
        let title = view.display_title();
        let (open_pane, message_pane, stop_pane) = (pane.clone(), pane.clone(), pane.clone());
        let top = div()
            .flex_shrink_0()
            .px_5()
            .pt_4()
            .pb_3()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(format!("$ {}", asked.unwrap_or_else(|| title.clone()))))
            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(cx, "sessions.workflow_of", &[("title", &title)])));
        let status = div()
            .flex_shrink_0()
            .px_5()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .t_small()
            .flex_wrap()
            .gap_y_1()
            .child(
                div()
                    .flex_1()
                    .min_w(px(240.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(said_color)))
                    .child(div().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(said))
                    .child(div().min_w_0().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                        cx,
                        "sessions.summary",
                        &[
                            ("waves", &(waves.len() - 1).to_string()),
                            ("done", &done.to_string()),
                            ("all", &all.to_string()),
                            ("models", &models.to_string()),
                        ],
                    ))),
            )
            .when_some(record.as_ref().filter(|r| r.stage != Stage::Ready), |d, r| {
                d.children(self.stage_buttons(r.id, r.stage, "page", cx))
            })
            .child(
                self.board_action(
                    SharedString::from(format!("board-open-{pane_id}")),
                    "square-terminal",
                    t(cx, "sessions.open"),
                    Chrome::FOREGROUND,
                    cx,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.go_to_pane(&open_pane, window, cx))),
            )
            .child(
                self.board_action(
                    SharedString::from(format!("board-message-{pane_id}")),
                    "message-square",
                    t(cx, "sessions.message"),
                    Chrome::FOREGROUND,
                    cx,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.toggle_session_message(&message_pane, window, cx))),
            )
            .when_some(record.as_ref().map(|r| r.id), |d, id| {
                d.child(
                    self.board_action(SharedString::from(format!("board-delete-{id}")), "trash-2", t(cx, "orch.delete"), Chrome::MUTED, cx)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.confirm_delete_orchestration(id, cx))),
                )
            })
            .when(main_running && record.is_none(), |d| {
                d.child(
                    self.board_action(
                        SharedString::from(format!("board-stop-{pane_id}")),
                        "circle-x",
                        t(cx, "sessions.interrupt"),
                        Chrome::ERROR,
                        cx,
                    )
                    .tooltip(crate::ui::Tooltip::text(t(cx, "sessions.interrupt_hint"), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        // Esc: what stops a turn in the agent's own window.
                        stop_pane.update(cx, |view, _| view.write(b"\x1b".to_vec()));
                        this.show_toast(t(cx, "sessions.interrupted"), cx);
                    })),
                )
            });
        let message = self.session_board.message.as_ref().filter(|(id, _, _)| *id == pane_id).map(|(_, input, _)| {
            div().px_5().pb_2().flex().items_center().gap_2().child(icon("message-square", 12., hex(Chrome::MUTED))).child(
                div().flex_1().min_w_0().px_2().py_1().rounded_md().border_1().border_color(hex(Chrome::BORDER)).child(input.clone()),
            )
        });
        // An orchestrator that planned and waits for the go: say so, and let it start from here.
        let _ = waiting_on_user;
        let plan_bar = record.as_ref().filter(|r| r.stage == Stage::Ready).map(|r| self.render_plan_bar(r.id, pane, cx));
        div()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(top)
            .child(status)
            .children(plan_bar)
            .children(message)
            .child(self.render_waves(waves, cx))
            .child(self.render_feed(feed))
            .into_any_element()
    }

    /// A parallel run: its tasks as one wave, each in its pane's state.
    fn render_run_workflow(&self, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let Some(run) = self.session_board.runs.iter().find(|r| r.id == id) else { return div().into_any_element() };
        let panes = self.all_panes();
        let mut nodes = Vec::new();
        let mut feed = Vec::new();
        for task in &run.tasks {
            let pane = panes.iter().find(|p| p.read(cx).pane_id == task.pane).cloned();
            let (state, line, seconds) = match &pane {
                Some(pane) => {
                    let view = pane.read(cx);
                    let running = view.status.in_turn();
                    let line = view.last_tool.as_ref().map(|(tool, target)| crate::terminal::tool_label(tool, target.as_deref()));
                    if let Some(line) = &line {
                        feed.push((0, task.title.clone(), None, line.clone(), if running { NodeState::Running } else { NodeState::Done }));
                    }
                    let since = view.agent_since_ms.unwrap_or(run.started_ms);
                    (if running { NodeState::Running } else { NodeState::Done }, line, crate::ui::now_ms().saturating_sub(since) / 1000)
                }
                None => (NodeState::Pending, Some(t(cx, "sessions.closed").to_string()), 0),
            };
            nodes.push(Node {
                name: task.title.clone(),
                state,
                seconds,
                role: task.branch.clone().or_else(|| task.folder.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default(),
                model: None,
                line,
                open: match pane {
                    Some(pane) => Open::Pane(pane),
                    None => continue,
                },
            });
        }
        let (done, all) = self.run_progress(run, cx);
        let clock = {
            use chrono::TimeZone;
            chrono::Local.timestamp_millis_opt(run.started_ms as i64).single().map(|at| at.format("%H:%M").to_string()).unwrap_or_default()
        };
        let running = nodes.iter().filter(|n| n.state == NodeState::Running).count();
        let wave = Wave {
            title: if running > 0 {
                tf(
                    cx,
                    "sessions.wave_running",
                    &[("n", "1"), ("done", &done.to_string()), ("all", &all.to_string()), ("running", &running.to_string())],
                )
            } else {
                tf(cx, "sessions.wave", &[("n", "1"), ("done", &done.to_string()), ("all", &all.to_string())])
            },
            nodes,
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(
                        div()
                            .t_body()
                            .text_color(hex(Chrome::BRIGHT))
                            .child(format!("$ agentty tasks · {}", tf(cx, "sessions.run_title", &[("n", &all.to_string())]))),
                    )
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                        cx,
                        "sessions.run_from",
                        &[("name", &run.caller_name), ("time", &clock)],
                    ))),
            )
            .child(div().px_5().py_2().t_small().text_color(hex(Chrome::FOREGROUND)).child(tf(
                cx,
                "sessions.run_progress",
                &[("done", &done.to_string()), ("all", &all.to_string())],
            )))
            .child(self.render_waves(vec![wave], cx))
            .child(self.render_feed(feed))
            .into_any_element()
    }

    /// The waves side by side, nodes stacked in each, lines from each wave to the next — on a
    /// chart moved and zoomed like the connection map's: drag the empty space or use the wheel to
    /// move it, ⌘/Ctrl + wheel or the buttons at the bottom right to zoom.
    fn render_waves(&self, waves: Vec<Wave>, cx: &mut Context<Self>) -> AnyElement {
        let board = &self.session_board;
        let (z, pan) = (board.zoom(), board.pan);
        let s = move |v: f32| px(v * z);
        // Where the dashes and the travelling dot are now: smooth, from the clock.
        let now = crate::ui::now_ms();
        let phase = (now % 700) as f32 / 700.;
        let travel = (now % 1600) as f32 / 1600.;
        let tallest = waves.iter().map(|w| w.nodes.len()).max().unwrap_or(1).max(1);
        board.chart_size.set((24. + waves.len() as f32 * (NODE_W + COL_GAP), WAVE_HEAD + 16. + tallest as f32 * (NODE_H + ROW_GAP)));
        // A chart point, on the canvas.
        let at = move |col: usize, row: usize| {
            point(
                pan.x + px((24. + col as f32 * (NODE_W + COL_GAP)) * z),
                pan.y + px((WAVE_HEAD + 8. + row as f32 * (NODE_H + ROW_GAP)) * z),
            )
        };
        // Lines: from the right of a node to the left of one in the next wave, in the target's state.
        let mut lines: Vec<(Point<Pixels>, Point<Pixels>, NodeState)> = Vec::new();
        for col in 0..waves.len().saturating_sub(1) {
            for (a, b) in edges_between(waves[col].nodes.len(), waves[col + 1].nodes.len()) {
                let (from, to) = (at(col, a), at(col + 1, b));
                lines.push((
                    point(from.x + s(NODE_W), from.y + s(NODE_H / 2.)),
                    point(to.x, to.y + s(NODE_H / 2.)),
                    waves[col + 1].nodes[b].state,
                ));
            }
        }
        board.animating.set(lines.iter().any(|(_, _, state)| *state == NodeState::Running));
        let canvas_bounds = board.canvas.clone();
        let entity = cx.entity().downgrade();
        let mut area = div()
            .id("session-waves")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            // Dragging the empty space moves the chart.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.session_board.pan_drag = Some((event.position, this.session_board.pan));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if let Some((from, pan)) = this.session_board.pan_drag {
                    if event.pressed_button != Some(gpui::MouseButton::Left) {
                        this.session_board.pan_drag = None;
                    } else {
                        this.session_board.pan = pan + (event.position - from);
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| this.end_board_pan(cx)))
            .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| this.end_board_pan(cx)))
            .when(board.pan_drag.is_some(), |d| d.cursor(gpui::CursorStyle::ClosedHand))
            .child(
                canvas(
                    move |bounds, _, _| canvas_bounds.set(bounds),
                    move |bounds, _, window, _| {
                        // The wheel moves the chart (both ways); with ⌘ or Ctrl it zooms around the pointer.
                        let entity = entity.clone();
                        window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble || !bounds.contains(&event.position) {
                                return;
                            }
                            let delta = event.delta.pixel_delta(px(16.));
                            let zooming = event.modifiers.platform || event.modifiers.control;
                            let _ = entity.update(cx, |this, cx| {
                                let board = &mut this.session_board;
                                if zooming {
                                    let dy = f32::from(delta.y);
                                    if dy != 0. {
                                        board.zoom_at(board.zoom() * (1. + dy * 0.004), event.position);
                                    }
                                } else if event.modifiers.shift && delta.x == px(0.) {
                                    // Shift turns a plain wheel sideways.
                                    board.pan.x += delta.y;
                                } else {
                                    board.pan += delta;
                                }
                                cx.notify();
                            });
                            cx.stop_propagation();
                        });
                        let o = bounds.origin;
                        for (start, end, state) in &lines {
                            let (s, e) = (point(o.x + start.x, o.y + start.y), point(o.x + end.x, o.y + end.y));
                            let bend = px(COL_GAP / 2. * z);
                            let color = state_color(*state);
                            let alpha = if *state == NodeState::Pending { 0.35 } else { 0.8 };
                            let width = px((1.5 * z).max(1.));
                            if *state == NodeState::Running {
                                // Dashes that move along the line while the work runs.
                                let curve = |t: f32| {
                                    let u = 1. - t;
                                    let x = u * u * u * f32::from(s.x)
                                        + 3. * u * u * t * f32::from(s.x + bend)
                                        + 3. * u * t * t * f32::from(e.x - bend)
                                        + t * t * t * f32::from(e.x);
                                    let y = u * u * u * f32::from(s.y)
                                        + 3. * u * u * t * f32::from(s.y)
                                        + 3. * u * t * t * f32::from(e.y)
                                        + t * t * t * f32::from(e.y);
                                    point(px(x), px(y))
                                };
                                let steps = 18;
                                for i in 0..steps {
                                    let from = (i as f32 + phase) / steps as f32;
                                    let to = (from + 0.55 / steps as f32).min(1.);
                                    if from >= 1. {
                                        continue;
                                    }
                                    let mut dash = PathBuilder::stroke(width);
                                    dash.move_to(curve(from));
                                    dash.line_to(curve(to));
                                    if let Ok(path) = dash.build() {
                                        window.paint_path(path, hex_alpha(color, alpha));
                                    }
                                }
                                // A dot on its way along the line, as work flows to the next wave.
                                let at = curve(travel);
                                let r = px(2.5 * z.max(0.6));
                                window.paint_quad(
                                    gpui::fill(gpui::Bounds::new(point(at.x - r, at.y - r), gpui::size(r * 2., r * 2.)), hex(color))
                                        .corner_radii(r),
                                );
                            } else {
                                let mut path = PathBuilder::stroke(width);
                                path.move_to(s);
                                path.cubic_bezier_to(e, point(s.x + bend, s.y), point(e.x - bend, e.y));
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, hex_alpha(color, alpha));
                                }
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        for (col, wave) in waves.into_iter().enumerate() {
            let head = at(col, 0);
            let active = wave.nodes.iter().any(|n| n.state == NodeState::Running);
            area = area.child(
                div()
                    .absolute()
                    .left(head.x)
                    .top(pan.y + s(4.))
                    .w(s(NODE_W))
                    .pb(s(4.))
                    .text_size(s(Type::CAPTION))
                    .truncate()
                    .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::MUTED }))
                    // The wave at work is underlined, as omo marks it.
                    .when(active, |d| d.border_b_2().border_color(hex(RUNNING)))
                    .child(wave.title),
            );
            for (row, node) in wave.nodes.into_iter().enumerate() {
                area = area.child(self.render_wave_node(node, at(col, row), z, col, row, cx));
            }
        }
        area.child(self.render_board_zoom(cx)).into_any_element()
    }

    fn end_board_pan(&mut self, cx: &mut Context<Self>) {
        if self.session_board.pan_drag.take().is_some() {
            cx.notify();
        }
    }

    /// − 100% + and fit, at the bottom right — as on the connection map.
    fn render_board_zoom(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let set = |zoom: fn(&SessionBoard) -> Option<f32>| {
            move |this: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                match zoom(&this.session_board) {
                    Some(zoom) => {
                        let middle = this.session_board.canvas.get().center();
                        this.session_board.zoom_at(zoom, middle);
                    }
                    None => this.session_board.fit(),
                }
                cx.notify();
            }
        };
        let button =
            |id: &'static str, glyph: &'static str, tip: &'static str, zoom: fn(&SessionBoard) -> Option<f32>, cx: &mut Context<Self>| {
                crate::ui::icon_only(id, glyph, cx.listener(set(zoom)))
                    .tooltip(crate::ui::Tooltip::text(SharedString::from(t(cx, tip)), None))
            };
        div()
            .absolute()
            .bottom(px(10.))
            .right(px(10.))
            .px_1()
            .py_0p5()
            .flex()
            .items_center()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex_alpha(super::plugin_ui::RAISED, 0.94))
            .shadow_lg()
            .occlude()
            // The bar is no place to start moving the chart from.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(button("board-zoom-out", "minus", "plugins.graph.zoom_out", |b| Some(b.zoom() / ZOOM_STEP), cx))
            .child(
                div()
                    .id("board-zoom-reset")
                    .w(px(44.))
                    .text_center()
                    .t_caption()
                    .text_color(hex(Chrome::FOREGROUND))
                    .cursor_pointer()
                    .tooltip(crate::ui::Tooltip::text(SharedString::from(t(cx, "plugins.graph.zoom_reset")), None))
                    .on_click(cx.listener(set(|_| Some(1.))))
                    .child(format!("{}%", (self.session_board.zoom() * 100.).round())),
            )
            .child(button("board-zoom-in", "plus", "plugins.graph.zoom_in", |b| Some(b.zoom() * ZOOM_STEP), cx))
            .child(button("board-zoom-fit", "maximize-2", "orch.zoom_fit", |_| None, cx))
    }

    fn render_wave_node(&self, node: Node, at: Point<Pixels>, z: f32, col: usize, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let s = |v: f32| px(v * z);
        let color = state_color(node.state);
        let (state_word, dim) = match node.state {
            NodeState::Done => (t(cx, "sessions.node_done"), false),
            NodeState::Running => (t(cx, "sessions.node_running"), false),
            NodeState::Pending => (t(cx, "sessions.node_pending"), true),
            NodeState::Stopped => (t(cx, "sessions.node_stopped"), true),
        };
        let open = node.open.clone();
        div()
            .id(SharedString::from(format!("wave-node-{col}-{row}")))
            .absolute()
            .left(at.x)
            .top(at.y)
            .w(s(NODE_W))
            .h(s(NODE_H))
            .px(s(10.))
            .py(s(8.))
            .flex()
            .flex_col()
            .gap(s(2.))
            .overflow_hidden()
            .rounded(s(6.))
            .border_1()
            .border_color(hex_alpha(color, if dim { 0.3 } else { 0.55 }))
            .bg(hex(Chrome::OVERLAY))
            .when(dim, |d| d.opacity(0.6))
            .cursor_pointer()
            .hover(|style| style.border_color(hex(color)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| match &open {
                Open::Nothing => {}
                Open::Pane(pane) => this.go_to_pane(pane, window, cx),
                Open::Subagent(pane, id) => this.open_subagent_of(pane, id, window, cx),
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(s(6.))
                    .min_w_0()
                    .child(div().flex_shrink_0().size(s(6.)).rounded_full().bg(hex(color)))
                    .child(
                        div()
                            .truncate()
                            .text_size(s(Type::SMALL))
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(node.name),
                    ),
            )
            .child(div().text_size(s(Type::CAPTION)).text_color(hex(color)).child(format!("{state_word} · {}", span(node.seconds))))
            .child(
                // One line: a long model name is cut, not wrapped onto the line below.
                div()
                    .flex()
                    .gap(s(4.))
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(s(Type::CAPTION))
                    .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(node.role))
                    .children(node.model.map(|m| div().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(format!("· {m}")))),
            )
            .children(node.line.map(|line| div().truncate().text_size(s(Type::CAPTION)).text_color(hex(Chrome::MUTED)).child(line)))
            .into_any_element()
    }

    /// Keeps every orchestration's stage up to date and its subagents' transcripts read, whatever
    /// is on screen: the strip on its pane and the page tell the same thing.
    pub(super) fn keep_orchestrations(&mut self, cx: &mut Context<Self>) {
        let on = crate::settings::settings(cx).orchestration;
        if !on {
            return;
        }
        self.sync_orchestrations(cx);
        if !self.session_board.shown(on).is_empty() {
            self.follow_session_board(cx);
        }
    }

    /// Whether `session`'s subagent transcripts are being read (so its team counts are exact).
    pub(super) fn watches_session(&self, session: &str) -> bool {
        self.session_board.watched.get(session).is_some_and(|f| !f.files.is_empty())
    }

    /// A session's team: its subagents from their transcripts (the hooks' word for one whose
    /// transcript has not shown up yet).
    pub(super) fn team_of(&self, session: &str, pane: Option<&Pane>, cx: &gpui::App) -> super::orchestration::Team {
        let mut team = super::orchestration::Team::default();
        let now = crate::ui::now_ms();
        let reported = |id: &str| pane.is_some_and(|p| p.read(cx).subagents.iter().any(|r| r.id == id && r.finished.is_some()));
        let mut seen = std::collections::HashSet::new();
        if let Some(followed) = self.session_board.watched.get(session) {
            let traces = followed.traces.lock().unwrap_or_else(|e| e.into_inner());
            for file in &followed.files {
                seen.insert(file.id.clone());
                match sub_state(traces.get(&file.id).map(|r| &r.trace), reported(&file.id), now) {
                    NodeState::Done => team.done += 1,
                    NodeState::Stopped => team.cut_off += 1,
                    NodeState::Running | NodeState::Pending => team.running += 1,
                }
            }
        }
        if let Some(pane) = pane {
            let view = pane.read(cx);
            // Only this process's: the hooks of an earlier one are gone with it.
            let alive = view.live_agent.is_some();
            for run in view.subagents.iter().filter(|r| !seen.contains(&r.id)) {
                if run.finished.is_some() {
                    team.done += 1;
                } else if alive {
                    team.running += 1;
                } else {
                    team.cut_off += 1;
                }
            }
        }
        team.all = team.running + team.done + team.cut_off;
        team
    }

    /// "The plan is ready": Start, View plan (its pane), Ask for changes (the message box).
    fn render_plan_bar(&self, id: u64, pane: &Pane, cx: &mut Context<Self>) -> AnyElement {
        let pane_id = pane.read(cx).pane_id;
        let (view, change) = (pane.clone(), pane.clone());
        let button = |id: String, label: &'static str, primary: bool| {
            div()
                .id(SharedString::from(id))
                .flex_shrink_0()
                .px_2p5()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .when(primary, |d| d.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::ON_ACCENT)))
                .when(!primary, |d| d.border_1().border_color(hex(Chrome::BORDER)).text_color(hex(Chrome::FOREGROUND)))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        div()
            .mx_5()
            .mb_2()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .rounded_md()
            .border_1()
            .border_color(hex_alpha(RUNNING, 0.6))
            .bg(hex_alpha(RUNNING, 0.08))
            .child(icon("list-todo", 14., hex(RUNNING)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "orch.plan_ready")),
                    )
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "orch.plan_ready_hint"))),
            )
            .child(
                button(format!("plan-view-{pane_id}"), t(cx, "orch.plan_view"), false)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.go_to_pane(&view, window, cx))),
            )
            .child(
                button(format!("plan-change-{pane_id}"), t(cx, "orch.plan_change"), false)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.toggle_session_message(&change, window, cx))),
            )
            .child(
                button(format!("plan-start-{pane_id}"), t(cx, "orch.plan_start"), true)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.start_orchestration(id, cx))),
            )
            .into_any_element()
    }

    /// The latest steps, oldest first: "● name · model — what it did".
    fn render_feed(&self, feed: Vec<FeedLine>) -> AnyElement {
        if feed.is_empty() {
            return div().into_any_element();
        }
        let mut lines = div().flex_shrink_0().px_5().py_2().flex().flex_col().gap_0p5().border_t_1().border_color(hex(Chrome::BORDER));
        for (_, name, model, words, finished) in feed {
            lines = lines.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .t_caption()
                    .child(div().flex_shrink_0().size(px(5.)).rounded_full().bg(hex(state_color(finished))))
                    .child(div().flex_shrink_0().max_w(px(200.)).truncate().text_color(hex(Chrome::FOREGROUND)).child(name))
                    .children(model.map(|m| div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(format!("· {m}"))))
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(format!("— {words}"))),
            );
        }
        lines.into_any_element()
    }

    fn board_action(
        &self,
        id: SharedString,
        glyph: &'static str,
        label: &'static str,
        tone: u32,
        _: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_1p5()
            .py_0p5()
            .rounded_sm()
            .cursor_pointer()
            .t_caption()
            .text_color(hex(tone))
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .child(icon(glyph, 11., hex(tone)))
            .child(label)
    }

    /// Opens a pane's subagents window at one subagent.
    fn open_subagent_of(&mut self, pane: &Pane, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_pane(pane, window, cx);
        if self.agent_panel.as_ref().is_none_or(|p| p.pane != pane.entity_id() || p.tab != super::agent_panel::PanelTab::Agents) {
            self.toggle_agent_panel(pane, super::agent_panel::PanelTab::Agents, cx);
        }
        self.select_subagent(id, cx);
    }

    /// (finished, all) tasks of a run: a task whose pane closed, or whose agent is between turns, is done.
    fn run_progress(&self, run: &TaskRun, cx: &gpui::App) -> (usize, usize) {
        let panes = self.all_panes();
        let done = run
            .tasks
            .iter()
            .filter(|task| panes.iter().find(|p| p.read(cx).pane_id == task.pane).is_none_or(|p| !p.read(cx).status.in_turn()))
            .count();
        (done, run.tasks.len())
    }

    /// Reads subagent transcripts as they grow — the picked session's while the page shows it, and
    /// every orchestration's while it has a pane — and marks those whose process has gone as cut
    /// off. One loop at a time; it ends when there is nothing left to read.
    fn follow_session_board(&mut self, cx: &mut Context<Self>) {
        if self.session_board.ticking {
            return;
        }
        self.session_board.ticking = true;
        cx.spawn(async move |this, cx| loop {
            let Ok(Some(work)) = this.update(cx, |this, cx| {
                let wanted = this.watched_sessions(cx);
                if wanted.is_empty() {
                    this.session_board.ticking = false;
                    return None;
                }
                this.session_board.watched.retain(|session, _| wanted.iter().any(|(w, _)| w == session));
                let work: Vec<_> = wanted
                    .into_iter()
                    .map(|(session, since)| {
                        let traces = this.session_board.watched.entry(session.clone()).or_default().traces.clone();
                        (session, since, traces)
                    })
                    .collect();
                Some(work)
            }) else {
                break;
            };
            let read = cx
                .background_spawn(async move {
                    work.into_iter()
                        .map(|(session, since, traces)| {
                            let files = agentty_bridge::claude::subagent_files(&session);
                            let mut map = traces.lock().unwrap_or_else(|e| e.into_inner());
                            for file in &files {
                                let reader = map.entry(file.id.clone()).or_insert_with(|| TraceReader::new(&file.path));
                                reader.poll().ok();
                                reader.mark_stopped_before(since);
                            }
                            (session, files)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let alive = this.update(cx, |this, cx| {
                for (session, files) in read {
                    if let Some(followed) = this.session_board.watched.get_mut(&session) {
                        followed.files = files;
                    }
                }
                this.sync_orchestrations(cx);
                cx.notify();
            });
            if alive.is_err() {
                break;
            }
            // Between reads, frames for the running lines (about 30 a second), only while there are any.
            for _ in 0..36 {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                let moving = this.update(cx, |this, cx| {
                    let moving = this.session_board.animating.get() && this.page == Some(super::Page::Flow);
                    if moving {
                        cx.notify();
                    }
                    moving
                });
                if moving.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// The sessions to read now: (session, when the agent process that would run its subagents
    /// started — `None` when none runs).
    fn watched_sessions(&self, cx: &gpui::App) -> Vec<(String, Option<u64>)> {
        let on = crate::settings::settings(cx).orchestration;
        let since_of = |pane: &Pane| {
            let view = pane.read(cx);
            view.live_agent.is_some().then_some(view.agent_since_ms).flatten()
        };
        let on_page = self.page == Some(super::Page::Flow) && self.session_board.tab == SessionTab::Workflow;
        let selected = self.session_board.selected;
        let mut wanted: Vec<(String, Option<u64>)> = Vec::new();
        if let (true, Some(Thread::Session(id))) = (on_page, selected) {
            if let Some(pane) = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == id) {
                if let Some(session) = pane.read(cx).current_session() {
                    wanted.push((session, since_of(&pane)));
                }
            }
        }
        for record in self.session_board.shown(on) {
            let pane = self.pane_of_record(record, cx);
            // One whose terminal is closed is read while it is shown.
            if pane.is_none() && !(on_page && selected == Some(Thread::Orchestration(record.id))) {
                continue;
            }
            if !wanted.iter().any(|(s, _)| *s == record.session) {
                wanted.push((record.session.clone(), pane.as_ref().and_then(since_of)));
            }
        }
        wanted
    }

    /// Opens (or closes) the message box of a session. Enter sends the text to it as if typed
    /// there; a session waiting for an answer would take it as that answer, so it is not sent then.
    fn toggle_session_message(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        let pane_id = pane.read(cx).pane_id;
        if self.session_board.message.as_ref().is_some_and(|(id, _, _)| *id == pane_id) {
            self.session_board.message = None;
            return cx.notify();
        }
        let input = cx.new(|cx| TextInput::new("", t(cx, "sessions.message_placeholder"), window, cx));
        let target = pane.clone();
        let subscription = cx.subscribe(&input, move |this, input, event: &TextInputEvent, cx| match event {
            TextInputEvent::Confirmed => {
                let text = input.read(cx).text().trim().to_string();
                if text.is_empty() {
                    return;
                }
                if target.read(cx).status.needs_user() {
                    return this.show_toast(t(cx, "sessions.message_waiting"), cx);
                }
                target.update(cx, |view, cx| view.submit_prompt(text, cx));
                this.session_board.message = None;
                this.show_toast(t(cx, "sessions.message_sent"), cx);
            }
            TextInputEvent::Cancelled => {
                this.session_board.message = None;
                cx.notify();
            }
            _ => {}
        });
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.session_board.message = Some((pane_id, input, subscription));
        cx.notify();
    }

    /// A parallel run was let start: kept for the workflow view, its tasks added as each opens.
    pub(super) fn record_task_run(&mut self, caller: &Pane, cx: &mut Context<Self>) -> u64 {
        let caller_name = self
            .locate(caller)
            .map(|(w, _)| self.workspace_label(&self.workspaces[w], cx))
            .unwrap_or_else(|| caller.read(cx).display_title());
        self.session_board.next_run += 1;
        let id = self.session_board.next_run;
        let caller = caller.read(cx).pane_id;
        self.session_board.runs.push(TaskRun { id, started_ms: crate::ui::now_ms(), caller, caller_name, tasks: Vec::new() });
        // Old runs go, so the list stays a list of what is going on.
        if self.session_board.runs.len() > 20 {
            self.session_board.runs.remove(0);
        }
        id
    }

    pub(super) fn record_run_task(&mut self, run: u64, task: RunTask, cx: &mut Context<Self>) {
        if let Some(run) = self.session_board.runs.iter_mut().find(|r| r.id == run) {
            run.tasks.push(task);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagents_started_together_are_one_wave() {
        assert_eq!(waves_of(&[0, 1_000, 2_500, 30_000, 31_000, 90_000]), [0, 0, 0, 1, 1, 2]);
        assert!(waves_of(&[]).is_empty());
    }

    #[test]
    fn lines_between_waves_stay_readable() {
        assert_eq!(edges_between(1, 3), [(0, 0), (0, 1), (0, 2)]);
        assert_eq!(edges_between(2, 2).len(), 4);
        // Many on both sides: each to the one level with it.
        assert_eq!(edges_between(4, 8), [(0, 0), (1, 1), (2, 2), (3, 3), (3, 4), (3, 5), (3, 6), (3, 7)]);
        assert!(edges_between(0, 3).is_empty());
    }

    #[test]
    fn short_words() {
        assert_eq!(span(45), "45s");
        assert_eq!(span(3_900), "1h 05m");
        assert_eq!(first_line("\n**Done.** All 449 entries", 60), "Done. All 449 entries");
    }
}
