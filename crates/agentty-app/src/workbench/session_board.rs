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
use crate::ui::{hint, icon, TypeScale};
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
/// Subagents started within this long of the one before them are one wave.
const WAVE_GAP_MS: u64 = 8_000;
/// Claude Code reports background subagents stopped while they still work: such a report counts
/// once the transcript has been quiet this long.
const QUIET_MS: u64 = 20_000;

/// Which view of the session page is on screen.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionTab {
    #[default]
    Workflow,
    Links,
}

/// The thread picked on the left.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Thread {
    /// An agent session, by its pane.
    Session(u64),
    /// A parallel run, by its id.
    Run(u64),
}

/// One `agentty tasks` request the user let start.
pub struct TaskRun {
    id: u64,
    pub started_ms: u64,
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
    session: Option<String>,
    files: Vec<SubagentFile>,
    traces: Arc<Mutex<HashMap<String, TraceReader>>>,
}

#[derive(Default)]
pub struct SessionBoard {
    pub tab: SessionTab,
    pub runs: Vec<TaskRun>,
    next_run: u64,
    selected: Option<Thread>,
    followed: Followed,
    /// The page is being kept up to date (one loop at a time).
    ticking: bool,
    /// The message box under the picked session.
    message: Option<(u64, Entity<TextInput>, Subscription)>,
    /// A line on screen is running: the page is redrawn often enough for it to move smoothly.
    animating: std::cell::Cell<bool>,
}

#[derive(Clone, Copy, PartialEq)]
enum NodeState {
    Done,
    Running,
    Pending,
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

/// What a click on a node opens.
#[derive(Clone)]
enum Open {
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

fn state_color(state: NodeState) -> u32 {
    match state {
        NodeState::Done => DONE,
        NodeState::Running => RUNNING,
        NodeState::Pending => Chrome::MUTED,
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

    /// Threads on the left, the picked one's workflow on the right.
    pub(super) fn render_workflow_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let sessions: Vec<Pane> = self.all_panes().into_iter().filter(|p| p.read(cx).is_agent() && p.read(cx).is_running()).collect();
        // The picked thread, or the busiest session.
        let still_there = match self.session_board.selected {
            Some(Thread::Session(id)) => sessions.iter().any(|p| p.read(cx).pane_id == id),
            Some(Thread::Run(id)) => self.session_board.runs.iter().any(|r| r.id == id),
            None => false,
        };
        if !still_there {
            let busiest = sessions.iter().max_by_key(|p| {
                let view = p.read(cx);
                (view.subagents.iter().filter(|r| r.finished.is_none()).count(), view.status.in_turn(), view.subagents.len())
            });
            self.session_board.selected = busiest.map(|p| Thread::Session(p.read(cx).pane_id));
        }
        self.follow_session_board(cx);

        let threads = self.render_threads(&sessions, cx);
        let body = match self.session_board.selected {
            Some(Thread::Session(id)) => match sessions.iter().find(|p| p.read(cx).pane_id == id) {
                Some(pane) => self.render_session_workflow(pane, cx),
                None => div().into_any_element(),
            },
            Some(Thread::Run(id)) => self.render_run_workflow(id, cx),
            None => div().p_6().child(hint(t(cx, "sessions.no_agents"))).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .font_family(MONO)
            .child(threads)
            .child(div().w(px(1.)).h_full().bg(hex(Chrome::BORDER)))
            .child(div().flex_1().min_w_0().h_full().child(body))
            .into_any_element()
    }

    /// "스레드": sessions, then parallel runs, each with a dot in its state's colour.
    fn render_threads(&self, sessions: &[Pane], cx: &mut Context<Self>) -> AnyElement {
        let selected = self.session_board.selected;
        let row = |id: SharedString, label: String, sub: Option<String>, color: u32, on: bool| {
            div()
                .id(id)
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
        let mut list = div()
            .id("session-threads")
            .w(px(230.))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .p_2()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(div().px_2().pb_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "sessions.threads")));
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
            list = list.child(div().px_2().pt_3().pb_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "sessions.runs")));
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

    /// A session: what was asked, the orchestrator's state, its subagents as waves, the feed.
    fn render_session_workflow(&self, pane: &Pane, cx: &mut Context<Self>) -> AnyElement {
        use crate::terminal::AgentStatus;
        let view = pane.read(cx);
        let pane_id = view.pane_id;
        let now_ms = crate::ui::now_ms();
        let traces = self.session_board.followed.traces.lock().unwrap_or_else(|e| e.into_inner());
        // Subagents from their transcripts, in the order they started; the hooks' word where a
        // transcript has not shown up yet.
        let mut files: Vec<&SubagentFile> = self.session_board.followed.files.iter().collect();
        files.sort_by_key(|f| traces.get(&f.id).and_then(|r| r.trace.started_ms).unwrap_or(u64::MAX));
        let reported_stopped = |id: &str| view.subagents.iter().any(|r| r.id == id && r.finished.is_some());
        let mut nodes: Vec<(u64, Node)> = Vec::new();
        for file in &files {
            let trace = traces.get(&file.id).map(|r| &r.trace);
            let started = trace.and_then(|t| t.started_ms).unwrap_or(now_ms);
            let quiet = trace.and_then(|t| t.last_ms).is_none_or(|at| now_ms.saturating_sub(at) > QUIET_MS);
            let done = trace.is_some_and(|t| t.finished) || (reported_stopped(&file.id) && quiet);
            let end = if done { trace.and_then(|t| t.last_ms).unwrap_or(now_ms) } else { now_ms };
            let line = trace.and_then(|t| {
                if done {
                    t.final_answer().map(|a| first_line(&a.target, 60))
                } else {
                    t.current().or_else(|| t.steps.iter().rev().find(|s| s.tool.is_some())).map(step_words)
                }
            });
            nodes.push((
                started,
                Node {
                    name: if file.description.is_empty() { file.agent_type.clone() } else { file.description.clone() },
                    state: if done { NodeState::Done } else { NodeState::Running },
                    seconds: end.saturating_sub(started) / 1000,
                    role: file.agent_type.clone(),
                    model: file.model.clone(),
                    line,
                    open: Open::Subagent(pane.clone(), file.id.clone()),
                },
            ));
        }
        // Subagents the hooks reported before their transcript appeared.
        for run in &view.subagents {
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
                    open: Open::Subagent(pane.clone(), run.id.clone()),
                },
            ));
        }
        nodes.sort_by_key(|(at, _)| *at);
        let starts: Vec<u64> = nodes.iter().map(|(at, _)| *at).collect();
        let wave_of = waves_of(&starts);

        // The orchestrator first, then the waves.
        let (state_label, state_color_main) = super::status_label_sized(view, true, cx);
        let main_running = view.status.in_turn();
        let orchestrator = Node {
            name: "orchestrator".into(),
            state: if main_running { NodeState::Running } else { NodeState::Done },
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
                _ => Some(state_label.clone()),
            },
            open: Open::Pane(pane.clone()),
        };
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
            wave.title = if running > 0 {
                tf(
                    cx,
                    "sessions.wave_running",
                    &[
                        ("n", &index.to_string()),
                        ("done", &done.to_string()),
                        ("all", &wave.nodes.len().to_string()),
                        ("running", &running.to_string()),
                    ],
                )
            } else {
                tf(cx, "sessions.wave", &[("n", &index.to_string()), ("done", &done.to_string()), ("all", &wave.nodes.len().to_string())])
            };
        }
        let subagents: Vec<&Node> = waves.iter().skip(1).flat_map(|w| &w.nodes).collect();
        let (done, all) = (subagents.iter().filter(|n| n.state == NodeState::Done).count(), subagents.len());
        let models = {
            let mut models: Vec<&str> = waves.iter().flat_map(|w| &w.nodes).filter_map(|n| n.model.as_deref()).collect();
            models.sort_unstable();
            models.dedup();
            models.len()
        };

        // The feed: the latest steps of its subagents.
        let mut feed: Vec<(u64, String, Option<String>, String, bool)> = Vec::new();
        for file in &files {
            let Some(reader) = traces.get(&file.id) else { continue };
            for step in reader.trace.steps.iter().rev().take(3) {
                let name = if file.description.is_empty() { file.agent_type.clone() } else { file.description.clone() };
                feed.push((step.start_ms, name, file.model.clone(), step_words(step), reader.trace.finished));
            }
        }
        feed.sort_by_key(|(at, ..)| *at);
        let feed: Vec<_> = feed.into_iter().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect();
        drop(traces);

        let asked = view.last_prompt.clone().map(|p| first_line(&p, 120));
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
            .child(div().text_color(hex(Chrome::FOREGROUND)).child(format!("orchestrator · {state_label}")))
            .child(div().size(px(6.)).rounded_full().bg(hex(state_color_main)))
            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                cx,
                "sessions.summary",
                &[
                    ("waves", &(waves.len() - 1).to_string()),
                    ("done", &done.to_string()),
                    ("all", &all.to_string()),
                    ("models", &models.to_string()),
                ],
            )))
            .child(div().flex_1())
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
            .when(main_running, |d| {
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
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(top)
            .child(status)
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
                        feed.push((0, task.title.clone(), None, line.clone(), !running));
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

    /// The waves side by side, nodes stacked in each, lines from each wave to the next.
    fn render_waves(&self, waves: Vec<Wave>, cx: &mut Context<Self>) -> AnyElement {
        // Where the dashes and the travelling dot are now: smooth, from the clock.
        let now = crate::ui::now_ms();
        let phase = (now % 700) as f32 / 700.;
        let travel = (now % 1600) as f32 / 1600.;
        let tallest = waves.iter().map(|w| w.nodes.len()).max().unwrap_or(1).max(1);
        let width = 24. + waves.len() as f32 * (NODE_W + COL_GAP);
        let height = WAVE_HEAD + 16. + tallest as f32 * (NODE_H + ROW_GAP);
        let at =
            |col: usize, row: usize| point(px(24. + col as f32 * (NODE_W + COL_GAP)), px(WAVE_HEAD + 8. + row as f32 * (NODE_H + ROW_GAP)));
        // Lines: from the right of a node to the left of one in the next wave, in the target's state.
        let mut lines: Vec<(Point<Pixels>, Point<Pixels>, NodeState)> = Vec::new();
        for col in 0..waves.len().saturating_sub(1) {
            for (a, b) in edges_between(waves[col].nodes.len(), waves[col + 1].nodes.len()) {
                let (from, to) = (at(col, a), at(col + 1, b));
                lines.push((
                    point(from.x + px(NODE_W), from.y + px(NODE_H / 2.)),
                    point(to.x, to.y + px(NODE_H / 2.)),
                    waves[col + 1].nodes[b].state,
                ));
            }
        }
        self.session_board.animating.set(lines.iter().any(|(_, _, state)| *state == NodeState::Running));
        let mut graph = div().relative().w(px(width)).h(px(height)).child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let o = bounds.origin;
                    for (start, end, state) in &lines {
                        let (s, e) = (point(o.x + start.x, o.y + start.y), point(o.x + end.x, o.y + end.y));
                        let bend = px(COL_GAP / 2.);
                        let color = state_color(*state);
                        let alpha = if *state == NodeState::Pending { 0.35 } else { 0.8 };
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
                                let mut dash = PathBuilder::stroke(px(1.5));
                                dash.move_to(curve(from));
                                dash.line_to(curve(to));
                                if let Ok(path) = dash.build() {
                                    window.paint_path(path, hex_alpha(color, alpha));
                                }
                            }
                            // A dot on its way along the line, as work flows to the next wave.
                            let at = curve(travel);
                            window.paint_quad(
                                gpui::fill(
                                    gpui::Bounds::new(point(at.x - px(2.5), at.y - px(2.5)), gpui::size(px(5.), px(5.))),
                                    hex(color),
                                )
                                .corner_radii(px(2.5)),
                            );
                        } else {
                            let mut path = PathBuilder::stroke(px(1.5));
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
            graph = graph.child(
                div()
                    .absolute()
                    .left(head.x)
                    .top(px(4.))
                    .w(px(NODE_W))
                    .pb_1()
                    .t_caption()
                    .truncate()
                    .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::MUTED }))
                    // The wave at work is underlined, as omo marks it.
                    .when(active, |d| d.border_b_2().border_color(hex(RUNNING)))
                    .child(wave.title),
            );
            for (row, node) in wave.nodes.into_iter().enumerate() {
                graph = graph.child(self.render_wave_node(node, at(col, row), col, row, cx));
            }
        }
        div().id("session-waves").flex_1().min_h_0().overflow_scroll().p_2().child(graph).into_any_element()
    }

    fn render_wave_node(&self, node: Node, at: Point<Pixels>, col: usize, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let color = state_color(node.state);
        let (state_word, dim) = match node.state {
            NodeState::Done => (t(cx, "sessions.node_done"), false),
            NodeState::Running => (t(cx, "sessions.node_running"), false),
            NodeState::Pending => (t(cx, "sessions.node_pending"), true),
        };
        let open = node.open.clone();
        div()
            .id(SharedString::from(format!("wave-node-{col}-{row}")))
            .absolute()
            .left(at.x)
            .top(at.y)
            .w(px(NODE_W))
            .h(px(NODE_H))
            .px_2p5()
            .py_2()
            .flex()
            .flex_col()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(hex_alpha(color, if dim { 0.3 } else { 0.55 }))
            .bg(hex(Chrome::OVERLAY))
            .when(dim, |d| d.opacity(0.6))
            .cursor_pointer()
            .hover(|s| s.border_color(hex(color)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| match &open {
                Open::Pane(pane) => this.go_to_pane(pane, window, cx),
                Open::Subagent(pane, id) => this.open_subagent_of(pane, id, window, cx),
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(color)))
                    .child(div().truncate().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(node.name)),
            )
            .child(div().t_caption().text_color(hex(color)).child(format!("{state_word} · {}", span(node.seconds))))
            .child(
                // One line: a long model name is cut, not wrapped onto the line below.
                div()
                    .flex()
                    .gap_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .t_caption()
                    .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(node.role))
                    .children(node.model.map(|m| div().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(format!("· {m}")))),
            )
            .children(node.line.map(|line| div().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(line)))
            .into_any_element()
    }

    /// The latest steps, oldest first: "● name · model — what it did".
    fn render_feed(&self, feed: Vec<(u64, String, Option<String>, String, bool)>) -> AnyElement {
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
                    .child(div().flex_shrink_0().size(px(5.)).rounded_full().bg(hex(if finished { DONE } else { RUNNING })))
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

    /// Reads the picked session's subagent transcripts as they grow, while the view is on screen.
    fn follow_session_board(&mut self, cx: &mut Context<Self>) {
        if self.session_board.ticking {
            return;
        }
        self.session_board.ticking = true;
        cx.spawn(async move |this, cx| loop {
            let Ok(Some((session, traces))) = this.update(cx, |this, cx| {
                let shown = this.page == Some(super::Page::Flow) && this.session_board.tab == SessionTab::Workflow;
                if !shown {
                    this.session_board.ticking = false;
                    return None;
                }
                let session = match this.session_board.selected {
                    Some(Thread::Session(id)) => {
                        this.all_panes().into_iter().find(|p| p.read(cx).pane_id == id).and_then(|p| p.read(cx).session_id_live.clone())
                    }
                    _ => None,
                };
                if this.session_board.followed.session != session {
                    this.session_board.followed = Followed { session: session.clone(), ..Followed::default() };
                }
                Some((session, this.session_board.followed.traces.clone()))
            }) else {
                break;
            };
            let files = cx
                .background_spawn(async move {
                    let Some(session) = session else { return Vec::new() };
                    let files = agentty_bridge::claude::subagent_files(&session);
                    for file in &files {
                        let mut map = traces.lock().unwrap_or_else(|e| e.into_inner());
                        let reader = map.entry(file.id.clone()).or_insert_with(|| TraceReader::new(&file.path));
                        reader.poll().ok();
                    }
                    files
                })
                .await;
            let alive = this.update(cx, |this, cx| {
                this.session_board.followed.files = files;
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
        self.session_board.runs.push(TaskRun { id, started_ms: crate::ui::now_ms(), caller_name, tasks: Vec::new() });
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
