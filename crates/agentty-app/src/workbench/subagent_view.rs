//! What a pane's subagents are doing, step by step: a timeline of all of them side by side (when
//! each ran which kind of tool, where it failed), a list with each one's current step, and for the
//! selected one its plan, the files it changed and every tool call with its outcome.

use super::agent_panel::AgentPanel;
use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{compact_number, icon, now_ms, IconSize, TypeScale};
use agentty_bridge::claude::SubagentFile;
use agentty_bridge::subagent_trace::{DiffKind, PlanStatus, Step, StepStatus, ToolKind, Trace};
use gpui::{div, prelude::*, px, relative, AnyElement, ClickEvent, Context, SharedString};

/// Columns of the timeline: each row is drawn as this many cells, each colored by what ran in it.
const TIMELINE_CELLS: usize = 200;
/// Steps of the selected subagent drawn at most (the latest ones).
const MAX_STEPS: usize = 300;
const MONO: &str = "JetBrains Mono";
/// Width of the timeline's row labels.
const LABEL_WIDTH: f32 = 200.;
/// `expanded` key of the task text.
const TASK_KEY: usize = usize::MAX;

fn kind_color(kind: ToolKind) -> u32 {
    match kind {
        ToolKind::Read => Chrome::BLUE,
        ToolKind::Edit => Chrome::GREEN,
        ToolKind::Run => Chrome::ORANGE,
        ToolKind::Web => Chrome::PURPLE,
        ToolKind::Agent => Chrome::PINK,
        ToolKind::Plan | ToolKind::Other => Chrome::MUTED,
    }
}

fn kind_icon(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "search",
        ToolKind::Edit => "pencil",
        ToolKind::Run => "square-terminal",
        ToolKind::Web => "globe",
        ToolKind::Agent => "bot",
        ToolKind::Plan => "list-todo",
        ToolKind::Other => "wrench",
    }
}

fn kind_label(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "collab.kind.read",
        ToolKind::Edit => "collab.kind.edit",
        ToolKind::Run => "collab.kind.run",
        ToolKind::Web => "collab.kind.web",
        ToolKind::Agent => "collab.kind.agent",
        ToolKind::Plan => "collab.kind.plan",
        ToolKind::Other => "collab.kind.other",
    }
}

fn duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        super::layout::format_elapsed(ms / 1_000)
    }
}

/// (start, end) of a run; a running one ends now.
fn span(trace: &Trace, now: u64) -> Option<(u64, u64)> {
    let start = trace.started_ms?;
    let end = if trace.finished { trace.last_ms.unwrap_or(start) } else { now.max(trace.last_ms.unwrap_or(start)) };
    Some((start, end.max(start + 1)))
}

/// Indices of `keys` — (still running, when it last did something in ms) — running ones first,
/// then the most recently active: the timeline and the list show the subagents in this order, so
/// what is happening now is at the top of both.
fn recent_first(keys: &[(bool, u64)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|&i| (!keys[i].0, std::cmp::Reverse(keys[i].1)));
    order
}

fn status_icon(status: StepStatus, finished: bool) -> AnyElement {
    match status {
        StepStatus::Running if !finished => crate::ui::spinner(11., hex(Chrome::ORANGE)).into_any_element(),
        StepStatus::Running => icon("circle-dot", 11., hex(Chrome::MUTED)).into_any_element(),
        StepStatus::Ok => icon("check", 11., hex(Chrome::SUCCESS)).into_any_element(),
        StepStatus::Error => icon("circle-x", 11., hex(Chrome::ERROR)).into_any_element(),
    }
}

impl Workbench {
    pub(super) fn render_subagent_traces(&self, panel: &AgentPanel, agents: &[SubagentFile], cx: &mut Context<Self>) -> AnyElement {
        let now = now_ms();
        let traces = panel.traces.lock().unwrap_or_else(|e| e.into_inner());
        let trace_of = |id: &str| traces.get(id).map(|r| &r.trace);
        // Finished ones the user sent away stay out of both views.
        let dismissed = self
            .all_panes()
            .into_iter()
            .find(|p| p.entity_id() == panel.pane)
            .map(|p| p.read(cx).dismissed_subagents.clone())
            .unwrap_or_default();
        let agents: Vec<SubagentFile> = agents.iter().filter(|a| !dismissed.contains(&a.id)).cloned().collect();
        let agents = agents.as_slice();
        // Claude Code's `SubagentStop` can come before the transcript shows the end.
        let ended: std::collections::HashSet<String> = self
            .all_panes()
            .into_iter()
            .find(|p| p.entity_id() == panel.pane)
            .map(|p| p.read(cx).subagents.iter().filter(|r| !r.running()).map(|r| r.id.clone()).collect())
            .unwrap_or_default();
        let running = agents
            .iter()
            .filter(|a| trace_of(&a.id).is_some() && !super::agent_speech::finished(trace_of(&a.id), ended.contains(&a.id), now))
            .count();
        // Running ones by when they started, not by their last step: rows that swap places with
        // every step would move a stop button out from under the pointer.
        let keys: Vec<(bool, u64)> = agents
            .iter()
            .map(|a| {
                trace_of(&a.id).map_or((false, 0), |t| {
                    let at = if t.finished { t.last_ms.or(t.started_ms) } else { t.started_ms };
                    (!t.finished, at.unwrap_or(0))
                })
            })
            .collect();
        let agents: Vec<&SubagentFile> = recent_first(&keys).into_iter().map(|i| &agents[i]).collect();
        let agents = agents.as_slice();

        let header = div()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .child(icon("bot", IconSize::INLINE, hex(Chrome::FOREGROUND)))
            .child(div().t_body().font_weight(crate::theme::EMPHASIS).child(t(cx, "collab.agents")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                cx,
                "collab.agents_count",
                &[("running", &running.to_string()), ("total", &agents.len().to_string())],
            )))
            .child(div().flex_1())
            .child(self.view_switch(cx));
        let header = self.agent_window_bar(header, cx);

        let list = self.render_trace_list(panel, agents, &trace_of, now, cx);
        let detail = match agents.iter().find(|a| panel.selected.as_deref() == Some(a.id.as_str())) {
            Some(agent) => match trace_of(&agent.id) {
                Some(trace) => self.render_trace_detail(panel, agent, trace, now, cx),
                None => crate::ui::loading_row(t(cx, "viewer.loading")).into_any_element(),
            },
            None => crate::ui::hint(t(cx, "collab.pick_agent")).into_any_element(),
        };
        if self.agent_window.office {
            let office = self.render_office(panel, agents, &trace_of, now, cx);
            let open = agents.iter().any(|a| panel.selected.as_deref() == Some(a.id.as_str()));
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(header)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(div().flex_1().min_w_0().h_full().child(office))
                        // Whoever was clicked: their log beside the office.
                        .when(open, |d| {
                            d.child(div().w(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
                                .child(div().w(px(420.)).flex_shrink_0().h_full().child(detail))
                        }),
                )
                .into_any_element();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_shrink_0().child(self.render_gantt(panel, agents, &trace_of, now, cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .border_t_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .child(list)
                    .child(div().w(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
                    .child(div().flex_1().min_w_0().h_full().child(detail)),
            )
            .into_any_element()
    }

    /// One bar per subagent on a shared time axis, colored by the kind of tool running at each moment.
    fn render_gantt<'a>(
        &self,
        panel: &AgentPanel,
        agents: &[&SubagentFile],
        trace_of: &impl Fn(&str) -> Option<&'a Trace>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spans: Vec<Option<(u64, u64)>> = agents.iter().map(|a| trace_of(&a.id).and_then(|t| span(t, now))).collect();
        let (Some(from), Some(to)) = (spans.iter().flatten().map(|s| s.0).min(), spans.iter().flatten().map(|s| s.1).max()) else {
            return div().into_any_element();
        };
        let total = (to - from).max(1) as f64;
        let at = |ms: u64| ((ms.saturating_sub(from)) as f64 / total).clamp(0.0, 1.0) as f32;
        let mut rows = div().id("subagent-gantt").flex().flex_col().gap_1().max_h(px(150.)).overflow_y_scroll();
        for (index, agent) in agents.iter().enumerate() {
            let Some(trace) = trace_of(&agent.id) else { continue };
            let Some((start, end)) = spans[index] else { continue };
            let selected = panel.selected.as_deref() == Some(agent.id.as_str());
            // Which kind ran in each cell; a failure wins its cell so it is never lost in a long run.
            let mut cells: Vec<Option<(ToolKind, bool)>> = vec![None; TIMELINE_CELLS];
            for step in trace.tool_steps() {
                let Some(kind) = step.kind() else { continue };
                let step_end = step.end_ms.unwrap_or(if trace.finished { step.start_ms } else { now });
                let first = (at(step.start_ms) * TIMELINE_CELLS as f32) as usize;
                let last = ((at(step_end) * TIMELINE_CELLS as f32).ceil() as usize).max(first + 1);
                for cell in cells.iter_mut().take(last.min(TIMELINE_CELLS)).skip(first.min(TIMELINE_CELLS - 1)) {
                    let failed = step.status == StepStatus::Error;
                    if cell.is_none_or(|(_, f)| !f) {
                        *cell = Some((kind, failed));
                    }
                }
            }
            let mut track = div().relative().flex_1().h(px(12.)).child(
                div()
                    .absolute()
                    .top(px(4.))
                    .h(px(4.))
                    .rounded_full()
                    .left(relative(at(start)))
                    .w(relative((at(end) - at(start)).max(0.004)))
                    .bg(hex_alpha(Chrome::MUTED, 0.25)),
            );
            let mut cell = 0;
            while cell < TIMELINE_CELLS {
                let Some(value) = cells[cell] else {
                    cell += 1;
                    continue;
                };
                let run = cells[cell..].iter().take_while(|c| **c == Some(value)).count();
                let (kind, failed) = value;
                track = track.child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .h(px(10.))
                        .rounded_sm()
                        .left(relative(cell as f32 / TIMELINE_CELLS as f32))
                        .w(relative(run as f32 / TIMELINE_CELLS as f32))
                        .bg(hex(if failed { Chrome::ERROR } else { kind_color(kind) })),
                );
                cell += run;
            }
            if !trace.finished {
                track = track.child(div().absolute().top_0().h(px(12.)).w(px(2.)).left(relative(at(end))).bg(hex(Chrome::ORANGE)));
            }
            let id = agent.id.clone();
            rows = rows.child(
                div()
                    .id(("subagent-gantt-row", index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
                    .when(!selected, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
                    .tooltip(crate::ui::Tooltip::text(format!("{} · {}", agent.agent_type, agent.description), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_subagent(&id, cx)))
                    .child(
                        div()
                            .w(px(LABEL_WIDTH))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .t_caption()
                            .child(if trace.finished {
                                icon("circle-check", 11., hex(Chrome::SUCCESS)).into_any_element()
                            } else {
                                crate::ui::spinner(11., hex(Chrome::ORANGE)).into_any_element()
                            })
                            // Many runs share a type (general-purpose): what each was for tells them apart.
                            .child(div().truncate().text_color(hex(Chrome::BRIGHT)).child(if agent.description.is_empty() {
                                agent.agent_type.clone()
                            } else {
                                agent.description.clone()
                            })),
                    )
                    .child(track)
                    .child(
                        div()
                            .w(px(52.))
                            .flex_shrink_0()
                            .text_right()
                            .t_caption()
                            .text_color(hex(Chrome::MUTED))
                            .child(duration(end - start)),
                    ),
            );
        }
        let ticks = div()
            .flex()
            .gap_2()
            .px_1()
            .child(div().w(px(LABEL_WIDTH)).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_between()
                    .t_caption()
                    .text_color(hex(Chrome::MUTED))
                    .children((0..=4).map(|i| div().child(super::layout::format_elapsed((to - from) * i / 4 / 1_000)))),
            )
            .child(div().w(px(52.)).flex_shrink_0());
        let legend_kinds = [ToolKind::Read, ToolKind::Edit, ToolKind::Run, ToolKind::Web, ToolKind::Agent, ToolKind::Other];
        let swatch = |color: u32, label: &'static str| {
            div().flex().items_center().gap_1().child(div().size(px(8.)).rounded_sm().bg(hex(color))).child(label)
        };
        let legend = div()
            .flex()
            .flex_wrap()
            .gap_3()
            .px_1()
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .children(legend_kinds.iter().map(|k| swatch(kind_color(*k), t(cx, kind_label(*k)))))
            .child(swatch(Chrome::ERROR, t(cx, "collab.kind.error")));
        div()
            .px_2()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div().px_1().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(t(cx, "collab.timeline")),
            )
            .child(rows)
            .child(ticks)
            .child(legend)
            .into_any_element()
    }

    fn render_trace_list<'a>(
        &self,
        panel: &AgentPanel,
        agents: &[&SubagentFile],
        trace_of: &impl Fn(&str) -> Option<&'a Trace>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div().id("subagent-list").w(px(270.)).flex_shrink_0().h_full().overflow_y_scroll().flex().flex_col().gap_0p5().p_1();
        let pane = self.all_panes().into_iter().find(|p| p.entity_id() == panel.pane);
        let stopping = pane.as_ref().map(|p| p.read(cx).stopping_subagents.clone()).unwrap_or_default();
        for (index, agent) in agents.iter().enumerate() {
            let trace = trace_of(&agent.id);
            let finished = trace.is_some_and(|t| t.finished);
            let selected = panel.selected.as_deref() == Some(agent.id.as_str());
            let id = agent.id.clone();
            let elapsed = trace.and_then(|t| span(t, now)).map(|(s, e)| duration(e - s)).unwrap_or_default();
            // Running: the call it waits on; done: how much it did.
            let now_line = trace.map(|trace| match trace.current() {
                Some(step) => {
                    let kind = step.kind().unwrap_or(ToolKind::Other);
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .t_caption()
                        .text_color(hex(Chrome::FOREGROUND))
                        .child(icon(kind_icon(kind), 11., hex(kind_color(kind))))
                        .child(div().flex_shrink_0().font_weight(crate::theme::EMPHASIS).child(step.tool.clone().unwrap_or_default()))
                        .child(div().truncate().text_color(hex(Chrome::MUTED)).child(step.target.clone()))
                        .child(div().flex_1())
                        .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(duration(now.saturating_sub(step.start_ms))))
                        .into_any_element()
                }
                None if !trace.finished => div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .t_caption()
                    .text_color(hex(Chrome::MUTED))
                    .child(icon("brain", 11., hex(Chrome::MUTED)))
                    .child(t(cx, "collab.thinking"))
                    .into_any_element(),
                None => div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .t_caption()
                    .text_color(hex(Chrome::MUTED))
                    .child(tf(cx, "collab.step_count", &[("n", &trace.tool_steps().count().to_string())]))
                    .when(!trace.files.is_empty(), |d| d.child(tf(cx, "collab.file_count", &[("n", &trace.files.len().to_string())])))
                    .when(trace.errors > 0, |d| {
                        d.child(div().text_color(hex(Chrome::ERROR)).child(tf(cx, "collab.errors", &[("n", &trace.errors.to_string())])))
                    })
                    .into_any_element(),
            });
            let plan = trace.filter(|t| !t.plan.is_empty()).map(|t| {
                let (done, all) = t.plan_progress();
                progress_bar(done, all, Chrome::SUCCESS)
            });
            list = list.child(
                div()
                    .id(("subagent", index))
                    .p_2()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
                    .when(!selected, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_subagent(&id, cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .t_small()
                            .child(if finished || trace.is_none() {
                                icon("circle-check", IconSize::INLINE, hex(if finished { Chrome::SUCCESS } else { Chrome::MUTED }))
                                    .into_any_element()
                            } else {
                                crate::ui::spinner(IconSize::INLINE, hex(Chrome::ORANGE)).into_any_element()
                            })
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(agent.agent_type.clone()),
                            )
                            .children(agent.model.clone().map(|m| div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(m)))
                            .child(div().flex_1())
                            .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(elapsed))
                            .children(
                                pane.clone()
                                    .filter(|_| trace.is_some() && !finished)
                                    .map(|pane| self.stop_subagent_button(&pane, &agent.id, stopping.contains(&agent.id), cx)),
                            ),
                    )
                    .child(div().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(agent.description.clone()))
                    .children(now_line)
                    .children(plan),
            );
        }
        list.into_any_element()
    }

    fn render_trace_detail(&self, panel: &AgentPanel, agent: &SubagentFile, trace: &Trace, now: u64, cx: &mut Context<Self>) -> AnyElement {
        let mut column = div().flex().flex_col().gap_2().p_3();

        // What it is, how far it got, what it cost.
        let tokens = trace.tokens();
        let elapsed = span(trace, now).map(|(s, e)| duration(e - s)).unwrap_or_default();
        let meta = [
            Some(agent.agent_type.clone()),
            agent.model.clone(),
            Some(elapsed),
            Some(tf(cx, "collab.step_count", &[("n", &trace.tool_steps().count().to_string())])),
            (trace.context() > 0).then(|| tf(cx, "collab.context", &[("n", &compact_number(trace.context()))])),
            (tokens.output > 0).then(|| tf(cx, "collab.output_tokens", &[("n", &compact_number(tokens.output))])),
        ];
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(agent.description.clone()))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x_2()
                        .t_caption()
                        .text_color(hex(Chrome::MUTED))
                        .children(meta.into_iter().flatten().map(|m| div().child(m)))
                        .when(trace.errors > 0, |d| {
                            d.child(div().px_1().rounded_sm().bg(hex_alpha(Chrome::ERROR, 0.18)).text_color(hex(Chrome::ERROR)).child(tf(
                                cx,
                                "collab.errors",
                                &[("n", &trace.errors.to_string())],
                            )))
                        }),
                ),
        );

        // Tool use at a glance.
        let counts = trace.tool_counts();
        if !counts.is_empty() {
            column = column.child(div().flex().flex_wrap().gap_1().children(counts.into_iter().map(|(tool, n)| {
                let kind = ToolKind::of(&tool);
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .rounded_sm()
                    .bg(hex_alpha(kind_color(kind), 0.14))
                    .t_caption()
                    .text_color(hex(Chrome::FOREGROUND))
                    .child(icon(kind_icon(kind), 10., hex(kind_color(kind))))
                    .child(format!("{tool} {n}"))
            })));
        }

        // What it was asked.
        if let Some(task) = &trace.task {
            let open = panel.expanded.contains(&TASK_KEY);
            column = column.child(
                section(t(cx, "collab.task")).child(
                    div()
                        .id("subagent-task")
                        .p_2()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(hex_alpha(Chrome::ACCENT, 0.10))
                        .t_small()
                        .text_color(hex(Chrome::FOREGROUND))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_step(TASK_KEY, cx)))
                        .child(super::agent_speech::markdown(task, if open { usize::MAX } else { 3 })),
                ),
            );
        }

        // Its plan.
        if !trace.plan.is_empty() {
            let (done, all) = trace.plan_progress();
            let mut plan = section(tf(cx, "collab.plan", &[("done", &done.to_string()), ("total", &all.to_string())])).child(progress_bar(
                done,
                all,
                Chrome::SUCCESS,
            ));
            for item in &trace.plan {
                let (glyph, color, text_color) = match item.status {
                    PlanStatus::Done => ("circle-check", Chrome::SUCCESS, Chrome::MUTED),
                    PlanStatus::Active => ("circle-dot", Chrome::ORANGE, Chrome::BRIGHT),
                    PlanStatus::Pending => ("square", Chrome::MUTED, Chrome::FOREGROUND),
                };
                plan = plan.child(
                    div()
                        .flex()
                        .items_start()
                        .gap_1p5()
                        .t_small()
                        .text_color(hex(text_color))
                        .child(div().pt(px(2.)).child(icon(glyph, 12., hex(color))))
                        .child(div().min_w_0().child(item.text.clone())),
                );
            }
            column = column.child(plan);
        }

        // What it changed.
        if !trace.files.is_empty() {
            let mut files = section(tf(cx, "collab.files_changed", &[("n", &trace.files.len().to_string())]));
            for file in trace.files.iter().take(10) {
                files = files.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .t_caption()
                        .child(icon(if file.created { "file-plus" } else { "file-text" }, 11., hex(Chrome::MUTED)))
                        .child(div().truncate().font_family(MONO).text_color(hex(Chrome::FOREGROUND)).child(file.path.clone()))
                        .child(div().flex_1())
                        .child(div().flex_shrink_0().text_color(hex(Chrome::SUCCESS)).child(format!("+{}", file.added)))
                        .child(div().flex_shrink_0().text_color(hex(Chrome::ERROR)).child(format!("−{}", file.removed))),
                );
            }
            if trace.files.len() > 10 {
                files = files.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "collab.more_files",
                    &[("n", &(trace.files.len() - 10).to_string())],
                )));
            }
            column = column.child(files);
        }

        // Every step.
        let mut steps = section(tf(cx, "collab.steps", &[("n", &trace.steps.len().to_string())]));
        let skip = trace.steps.len().saturating_sub(MAX_STEPS);
        if skip > 0 {
            steps = steps.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                cx,
                "collab.earlier_steps",
                &[("n", &skip.to_string())],
            )));
        }
        let last = trace.steps.len().saturating_sub(1);
        for (index, step) in trace.steps.iter().enumerate().skip(skip) {
            let final_answer = trace.finished && index == last && step.tool.is_none();
            steps = steps.child(self.render_step(index, step, trace.finished, final_answer, panel.expanded.contains(&index), now, cx));
        }
        if !trace.finished && trace.current().is_none() {
            steps = steps.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_1()
                    .t_caption()
                    .text_color(hex(Chrome::MUTED))
                    .child(crate::ui::spinner(11., hex(Chrome::MUTED)))
                    .child(t(cx, "collab.thinking")),
            );
        }
        column = column.child(steps);

        div().id("subagent-log").size_full().overflow_y_scroll().track_scroll(&panel.scroll).child(column).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_step(
        &self,
        index: usize,
        step: &Step,
        finished: bool,
        final_answer: bool,
        open: bool,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toggle = cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_step(index, cx));
        let Some(tool) = step.tool.clone() else {
            // Something it said; its report at the end shows more before it is opened.
            return div()
                .id(("subagent-step", index))
                .flex()
                .items_start()
                .gap_1p5()
                .p_1p5()
                .rounded_md()
                .cursor_pointer()
                .when(final_answer, |d| d.bg(hex_alpha(Chrome::SUCCESS, 0.10)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .on_click(toggle)
                .child(div().pt(px(2.)).child(icon("message-square", 11., hex(if final_answer { Chrome::SUCCESS } else { Chrome::MUTED }))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .t_small()
                        .text_color(hex(if final_answer { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                        .when(final_answer, |d| {
                            d.child(
                                div()
                                    .t_caption()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::SUCCESS))
                                    .child(t(cx, "collab.result")),
                            )
                        })
                        // Closed, the report shows its first lines and a passing remark its first one.
                        .child(super::agent_speech::markdown(
                            &step.target,
                            if open {
                                usize::MAX
                            } else if final_answer {
                                8
                            } else {
                                1
                            },
                        )),
                )
                .into_any_element();
        };
        let kind = ToolKind::of(&tool);
        let failed = step.status == StepStatus::Error;
        let took = match step.end_ms {
            Some(end) => duration(end.saturating_sub(step.start_ms)),
            None if !finished => duration(now.saturating_sub(step.start_ms)),
            None => String::new(),
        };
        let has_body = !step.output.is_empty() || !step.diff.is_empty();
        let row = div()
            .flex()
            .items_center()
            .gap_1p5()
            .t_caption()
            .child(status_icon(step.status, finished))
            .child(icon(kind_icon(kind), 11., hex(kind_color(kind))))
            // What it did, in words; the call itself on the line below.
            .child(
                div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::BRIGHT)).child(super::agent_speech::narrate(step, cx)),
            )
            .when(step.added + step.removed > 0, |d| {
                d.child(div().flex_shrink_0().text_color(hex(Chrome::SUCCESS)).child(format!("+{}", step.added)))
                    .child(div().flex_shrink_0().text_color(hex(Chrome::ERROR)).child(format!("−{}", step.removed)))
            })
            .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(took))
            .when(has_body, |d| d.child(icon(if open { "chevron-down" } else { "chevron-right" }, 11., hex(Chrome::MUTED))));
        let body = open.then(|| {
            let mut body = div()
                .ml(px(30.))
                .p_2()
                .rounded_md()
                .bg(hex_alpha(Chrome::MUTED, 0.10))
                .flex()
                .flex_col()
                .t_caption()
                .font_family(MONO)
                .text_color(hex(Chrome::FOREGROUND));
            if !step.diff.is_empty() {
                body = body.children(step.diff.iter().map(|line| {
                    let (sign, color, bg) = match line.kind {
                        DiffKind::Added => ("+", Chrome::SUCCESS, Some(hex_alpha(Chrome::SUCCESS, 0.10))),
                        DiffKind::Removed => ("−", Chrome::ERROR, Some(hex_alpha(Chrome::ERROR, 0.10))),
                        DiffKind::Context => (" ", Chrome::MUTED, None),
                    };
                    div()
                        .flex()
                        .gap_1()
                        .when_some(bg, |d, bg| d.bg(bg))
                        .child(div().flex_shrink_0().text_color(hex(color)).child(sign))
                        .child(div().min_w_0().child(line.text.clone()))
                }));
            } else {
                if failed && step.output_more > 0 {
                    body = body.child(more_lines(step.output_more, cx));
                }
                body = body.children(step.output.iter().map(|l| {
                    div().text_color(hex(if failed { Chrome::ERROR } else { Chrome::FOREGROUND })).child(if l.is_empty() {
                        " ".to_string()
                    } else {
                        l.clone()
                    })
                }));
                if !failed && step.output_more > 0 {
                    body = body.child(more_lines(step.output_more, cx));
                }
            }
            body
        });
        div()
            .id(("subagent-step", index))
            .flex()
            .flex_col()
            .gap_1()
            .px_1p5()
            .py_1()
            .rounded_md()
            .when(failed, |d| d.bg(hex_alpha(Chrome::ERROR, 0.08)))
            .when(has_body, |d| d.cursor_pointer().hover(|s| s.bg(hex(Chrome::HOVER))).on_click(toggle))
            .child(row)
            .child(div().ml(px(30.)).t_caption().truncate().font_family(MONO).text_color(hex(Chrome::MUTED)).child(match &step.detail {
                // A Bash call's description is already the line above.
                Some(detail) if !matches!(tool.as_str(), "Bash" | "PowerShell") => format!("{tool} · {} · {detail}", step.target),
                _ => format!("{tool} · {}", step.target),
            }))
            // A failure shows its last line without opening it.
            .when(failed && !open, |d| {
                d.children(
                    step.output.last().map(|l| {
                        div().ml(px(30.)).t_caption().truncate().font_family(MONO).text_color(hex(Chrome::ERROR)).child(l.clone())
                    }),
                )
            })
            .children(body)
            .into_any_element()
    }

    /// Office or timeline: two ways to look at the same subagents.
    fn view_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let office = self.agent_window.office;
        let option = |id: &'static str, label: &'static str, glyph: &'static str, on: bool, value: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .t_caption()
                .when(on, |d| d.bg(hex(Chrome::SELECTED)).text_color(hex(Chrome::BRIGHT)))
                .when(!on, |d| d.text_color(hex(Chrome::MUTED)).hover(|s| s.bg(hex(Chrome::HOVER))))
                .child(icon(glyph, 11., hex(if on { Chrome::BRIGHT } else { Chrome::MUTED })))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.agent_window.office = value;
                    if value {
                        this.animate_office(cx);
                    }
                    cx.notify();
                }))
        };
        div()
            .flex()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded_md()
            .bg(hex_alpha(Chrome::MUTED, 0.12))
            .child(option("view-office", t(cx, "office.view"), "users", office, true, cx))
            .child(option("view-timeline", t(cx, "office.timeline"), "list", !office, false, cx))
            .into_any_element()
    }

    pub(super) fn select_subagent(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(panel) = self.subagents_shown() {
            if panel.selected.as_deref() != Some(id) {
                panel.selected = Some(id.to_string());
                panel.expanded.clear();
                panel.scroll = gpui::ScrollHandle::new();
                panel.scroll.scroll_to_bottom();
            }
        }
        cx.notify();
    }

    fn toggle_step(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(panel) = self.subagents_shown() {
            if !panel.expanded.remove(&index) {
                panel.expanded.insert(index);
            }
        }
        cx.notify();
    }
}

fn section(title: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(title.into()))
}

fn more_lines(n: usize, cx: &gpui::App) -> gpui::Div {
    div().text_color(hex(Chrome::MUTED)).child(tf(cx, "collab.more_lines", &[("n", &n.to_string())]))
}

fn progress_bar(done: usize, all: usize, color: u32) -> gpui::Div {
    let fraction = if all == 0 { 0.0 } else { done as f32 / all as f32 };
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .h(px(4.))
                .rounded_full()
                .bg(hex_alpha(Chrome::MUTED, 0.25))
                .child(div().h_full().rounded_full().w(relative(fraction)).bg(hex(color))),
        )
        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(format!("{done}/{all}")))
}

#[cfg(test)]
mod tests {
    use super::recent_first;

    #[test]
    fn running_first_then_the_most_recent() {
        // (running, last activity)
        let keys = [(false, 100), (true, 50), (false, 300), (true, 200)];
        assert_eq!(recent_first(&keys), [3, 1, 2, 0]);
    }
}
