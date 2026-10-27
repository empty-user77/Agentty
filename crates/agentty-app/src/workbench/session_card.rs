//! What a workspace card says about the agents working in it, below its name: the state they are
//! in (the same words and colours as a pane's header), how long the agent has run and what its
//! session has used, how far along its to-do list is, and its subagents with what each is doing.
//! A click on the to-dos opens the list; a click on a subagent opens the subagents window.

use super::agent_panel::PanelTab;
use super::{Pane, Workbench, Workspace};
use crate::i18n::tf;
use crate::session_plan::{PlanItem, PlanStatus};
use crate::terminal::AgentStatus;
use crate::theme::{hex_alpha, Chrome};
use crate::ui::{icon, TypeScale};
use gpui::{div, prelude::*, px, relative, AnyElement, ClickEvent, Context, Hsla, Pixels, SharedString};
use std::time::Instant;

/// Running subagents listed on a card at most; the rest are counted.
const SUBAGENT_ROWS: usize = 4;
/// To-do items listed when the list is open.
const PLAN_ROWS: usize = 12;

/// How much a pane's state asks for the user: the card shows the most pressing one.
fn urgency(status: &AgentStatus) -> u8 {
    match status {
        AgentStatus::Permission(_) | AgentStatus::Question(_) => 5,
        AgentStatus::Working => 4,
        AgentStatus::Thinking => 3,
        AgentStatus::Interrupted => 2,
        AgentStatus::Finished(_) => 1,
        AgentStatus::Idle => 0,
    }
}

/// "45s", "12m", "1h 05m".
fn span(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// "812", "45.2k", "1.3M".
fn tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.),
        _ => format!("{:.1}M", n as f64 / 1_000_000.),
    }
}

impl Workbench {
    /// The card's agent lines, or nothing for a workspace without a running agent.
    pub(super) fn render_session_lines(
        &self,
        ws: &Workspace,
        ink: Option<u32>,
        indent: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = ws.id;
        let agents: Vec<Pane> = ws
            .tabs
            .iter()
            .flat_map(|t| t.root.leaves())
            .filter(|p| {
                let view = p.read(cx);
                view.is_agent() && view.is_running()
            })
            .collect();
        let lead = agents.iter().max_by_key(|p| urgency(&p.read(cx).status))?.clone();
        // On a card the user coloured, everything is cut from the card's own ink.
        let tone = |color: u32, alpha: f32| -> Hsla {
            match ink {
                Some(ink) => hex_alpha(ink, alpha.max(0.75)),
                None => hex_alpha(color, alpha),
            }
        };
        let muted = tone(Chrome::MUTED, 1.);
        let mut lines = div().pl(indent).pt_0p5().flex().flex_col().gap_0p5().t_caption();

        // State · how long · what it used.
        // An orchestrator's card says how far the orchestration got, as its strip and page do.
        let (label, color) = match self.record_of_pane(&lead, cx).cloned() {
            Some(record) => {
                let team = self.team_of(&record.session, Some(&lead), cx);
                let (_, color) = self.stage_words(record.stage, team, cx);
                (crate::i18n::t(cx, super::orchestration::stage_key(record.stage)).to_string(), color)
            }
            None => {
                let view = lead.read(cx);
                super::status_label_sized(view, true, cx)
            }
        };
        let now_ms = crate::ui::now_ms();
        let since = agents.iter().filter_map(|p| p.read(cx).agent_since_ms).min();
        let used: (u64, Option<f64>) = agents
            .iter()
            .filter_map(|p| p.read(cx).session_id_live.clone())
            .filter_map(|session| self.session_usage.get(&session).copied())
            .fold((0, None), |(tokens, cost), total| {
                (
                    tokens + total.tokens,
                    match (cost, total.cost) {
                        (None, None) => None,
                        (a, b) => Some(a.unwrap_or(0.) + b.unwrap_or(0.)),
                    },
                )
            });
        let mut facts = vec![label];
        if agents.len() > 1 {
            facts.push(tf(cx, "card.agents", &[("n", &agents.len().to_string())]));
        }
        if let Some(since) = since {
            facts.push(span(now_ms.saturating_sub(since) / 1000));
        }
        if used.0 > 0 {
            facts.push(match used.1 {
                Some(cost) if cost >= 0.01 => format!("{} · ${cost:.2}", tf(cx, "card.tokens", &[("n", &tokens(used.0))])),
                _ => tf(cx, "card.tokens", &[("n", &tokens(used.0))]),
            });
        }
        lines = lines.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .min_w_0()
                .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(tone(color, 1.)))
                .child(div().truncate().text_color(muted).child(facts.join(" · "))),
        );

        // The to-do list: progress, the item in hand, and the whole list when opened.
        let plan: Vec<PlanItem> = agents.iter().map(|p| p.read(cx).plan.clone()).find(|plan| !plan.is_empty()).unwrap_or_default();
        if !plan.is_empty() {
            let (done, all) = crate::session_plan::progress(&plan);
            let open = self.card_plans_open.contains(&id);
            let current =
                plan.iter().find(|p| p.status == PlanStatus::Active).or_else(|| plan.iter().find(|p| p.status == PlanStatus::Pending));
            lines = lines.child(
                div()
                    .id(("card-plan", id as usize))
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if !this.card_plans_open.remove(&id) {
                            this.card_plans_open.insert(id);
                        }
                        cx.notify();
                    }))
                    .child(icon("list-todo", 10., muted))
                    .child(div().flex_shrink_0().text_color(muted).child(tf(
                        cx,
                        "card.plan",
                        &[("done", &done.to_string()), ("all", &all.to_string())],
                    )))
                    .child(
                        div().flex_shrink_0().w(px(36.)).h(px(3.)).rounded_full().bg(tone(Chrome::MUTED, 0.3)).child(
                            div().h_full().rounded_full().w(relative(done as f32 / all.max(1) as f32)).bg(tone(Chrome::SUCCESS, 1.)),
                        ),
                    )
                    .children(current.filter(|_| !open).map(|item| div().min_w_0().truncate().text_color(muted).child(item.text.clone()))),
            );
            if open {
                for item in plan.iter().take(PLAN_ROWS) {
                    let (glyph, color) = match item.status {
                        PlanStatus::Done => ("circle-check", Chrome::SUCCESS),
                        PlanStatus::Active => ("circle-dot", Chrome::ORANGE),
                        PlanStatus::Pending => ("circle", Chrome::MUTED),
                    };
                    lines = lines.child(
                        div().pl(px(4.)).flex().items_center().gap_1().min_w_0().child(icon(glyph, 10., tone(color, 1.))).child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(muted)
                                .when(item.status == PlanStatus::Done, |d| d.line_through())
                                .child(item.text.clone()),
                        ),
                    );
                }
                if plan.len() > PLAN_ROWS {
                    lines = lines.child(div().pl(px(4.)).text_color(muted).child(tf(
                        cx,
                        "card.more",
                        &[("n", &(plan.len() - PLAN_ROWS).to_string())],
                    )));
                }
            }
        }

        // Subagents: the running ones with what each is doing, the finished ones counted — from
        // their transcripts when those are followed (as the chip and the subagents window count),
        // else as the hooks reported them.
        let mut running: Vec<(Pane, String, Option<String>, u64)> = Vec::new();
        let mut finished = 0;
        let now_ms = crate::ui::now_ms();
        for pane in &agents {
            match self.subagent_rows(pane, cx) {
                Some((rows, over)) => {
                    finished += over;
                    running.extend(rows.into_iter().map(|(name, doing, started)| (pane.clone(), name, doing, started)));
                }
                None => {
                    for run in &pane.read(cx).subagents {
                        if run.running() {
                            let doing = run.last_tool.as_ref().map(|(tool, target)| crate::terminal::tool_label(tool, target.as_deref()));
                            let name = run.task.clone().unwrap_or_else(|| run.kind.clone());
                            let started = now_ms.saturating_sub(Instant::now().duration_since(run.started).as_millis() as u64);
                            running.push((pane.clone(), name, doing, started));
                        } else {
                            finished += 1;
                        }
                    }
                }
            }
        }
        running.sort_by_key(|(_, _, _, started)| *started);
        for (index, (pane, name, doing, started)) in running.iter().take(SUBAGENT_ROWS).enumerate() {
            let (name, doing) = (name.clone(), doing.clone());
            let target = pane.clone();
            lines = lines.child(
                div()
                    .id(SharedString::from(format!("card-subagent-{id}-{index}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_subagents_of(&target, window, cx);
                    }))
                    .child(div().flex_shrink_0().text_color(muted).child("└"))
                    .child(crate::ui::dot_spinner(
                        SharedString::from(format!("card-subagent-spin-{id}-{index}")),
                        10.,
                        tone(Chrome::ORANGE, 1.),
                    ))
                    .child(div().flex_shrink_0().max_w(px(110.)).truncate().text_color(tone(Chrome::FOREGROUND, 1.)).child(name))
                    .children(doing.map(|doing| div().min_w_0().truncate().text_color(muted).child(doing)))
                    .child(div().flex_1())
                    .child(div().flex_shrink_0().text_color(muted).child(span(now_ms.saturating_sub(*started) / 1000))),
            );
        }
        let more = running.len().saturating_sub(SUBAGENT_ROWS);
        if more > 0 || finished > 0 {
            let mut parts = Vec::new();
            if more > 0 {
                parts.push(tf(cx, "card.subagents_more", &[("n", &more.to_string())]));
            }
            if finished > 0 {
                parts.push(tf(cx, "card.subagents_done", &[("n", &finished.to_string())]));
            }
            let target = lead.clone();
            lines = lines.child(
                div()
                    .id(("card-subagents", id as usize))
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .text_color(muted)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_subagents_of(&target, window, cx);
                    }))
                    .child(icon("bot", 10., muted))
                    .child(parts.join(" · ")),
            );
        }
        Some(lines.into_any_element())
    }

    /// Brings `pane` to the front (its workspace and tab) and opens its subagents window.
    fn open_subagents_of(&mut self, pane: &Pane, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if let Some((workspace, tab)) = self.locate(pane) {
            if workspace != self.active_workspace {
                self.activate_workspace(workspace, window, cx);
            }
            if let Some(ws) = self.workspaces.get_mut(workspace) {
                ws.active_tab = tab;
                if let Some(tab) = ws.tabs.get_mut(tab) {
                    tab.active = pane.clone();
                }
            }
        }
        if self.agent_panel.as_ref().is_none_or(|p| p.pane != pane.entity_id() || p.tab != PanelTab::Agents) {
            self.toggle_agent_panel(pane, PanelTab::Agents, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_numbers() {
        assert_eq!(span(45), "45s");
        assert_eq!(span(725), "12m");
        assert_eq!(span(3_900), "1h 05m");
        assert_eq!(tokens(812), "812");
        assert_eq!(tokens(45_210), "45.2k");
        assert_eq!(tokens(1_300_000), "1.3M");
    }
}
