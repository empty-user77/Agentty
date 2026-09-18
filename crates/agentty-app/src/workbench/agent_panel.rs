//! Collaboration view for one agent pane: its subagents (what each is doing, with its log) and
//! its session links (who it shares context with, and controls for them).

use super::flow::{EdgeStatus, LinkMode};
use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, now_ms, relative_time, IconSize, TypeScale};
use agentty_bridge::claude::Subagent;
use agentty_bridge::model::{Role, Turn};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, EntityId, FontWeight, SharedString};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Agents,
    Links,
}

pub struct AgentPanel {
    pub pane: EntityId,
    pub tab: PanelTab,
    pub subagents: Option<Vec<Subagent>>,
    pub selected: Option<String>,
    pub log: Option<Vec<Turn>>,
    pub scroll: gpui::ScrollHandle,
}

impl Workbench {
    pub(super) fn toggle_agent_panel(&mut self, pane: &Pane, tab: PanelTab, cx: &mut Context<Self>) {
        let id = pane.entity_id();
        if self.just_dismissed("agent-panel") {
            return;
        }
        if self.agent_panel.as_ref().is_some_and(|p| p.pane == id && p.tab == tab) {
            self.agent_panel = None;
            return cx.notify();
        }
        self.agent_panel =
            Some(AgentPanel { pane: id, tab, subagents: None, selected: None, log: None, scroll: gpui::ScrollHandle::new() });
        if tab == PanelTab::Agents {
            // Refresh while open: subagents come and go during a turn.
            cx.spawn(async move |this, cx| loop {
                let Ok(Some(session)) = this.read_with(cx, |this, cx| {
                    let panel = this.agent_panel.as_ref().filter(|p| p.pane == id && p.tab == PanelTab::Agents)?;
                    let pane = this.all_panes().into_iter().find(|p| p.entity_id() == panel.pane)?;
                    Some((pane.read(cx).session_id_live.clone(), panel.selected.clone()))
                }) else {
                    break;
                };
                let (session_id, selected) = session;
                let loaded = cx
                    .background_spawn(async move {
                        let agents = session_id.as_deref().map(agentty_bridge::claude::subagents).unwrap_or_default();
                        let log = selected
                            .and_then(|sel| agents.iter().find(|a| a.id == sel).map(|a| a.path.clone()))
                            .and_then(|path| agentty_bridge::claude::subagent_transcript(&path).ok());
                        (agents, log)
                    })
                    .await;
                let alive = this.update(cx, |this, cx| {
                    let Some(panel) = this.agent_panel.as_mut().filter(|p| p.pane == id) else { return false };
                    let (agents, log) = loaded;
                    if panel.selected.is_none() {
                        // Start with the running one (or the latest).
                        panel.selected = agents.iter().rev().find(|a| !a.finished).or(agents.last()).map(|a| a.id.clone());
                    }
                    panel.subagents = Some(agents);
                    if log.is_some() {
                        panel.log = log;
                    }
                    cx.notify();
                    true
                });
                if !alive.unwrap_or(false) {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            })
            .detach();
        }
        cx.notify();
    }

    /// `[⛓ 2] [👥 1/3]` chips for a pane header: session links and subagents, when there are any.
    pub(super) fn render_collab_chips(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = pane.read(cx);
        let pane_id = view.pane_id;
        let links = self.flow.edges().iter().filter(|e| e.from == pane_id || e.to == pane_id).count();
        let live = self.flow.edges().iter().any(|e| (e.from == pane_id || e.to == pane_id) && e.live);
        let (hook_total, hook_running) = (view.subagents.len(), view.subagents.iter().filter(|r| r.finished.is_none()).count());
        let (file_total, file_active) = view.subagent_files;
        let (total, running) = (hook_total.max(file_total), hook_running.max(file_active));
        // The link chip is also the way to *make* a link, so it shows whenever there is another agent.
        let connectable = view.is_agent() && view.is_running() && !self.connectable_panes(pane_id, cx).is_empty();
        if links == 0 && total == 0 && !connectable {
            return None;
        }
        let open = self.agent_panel.as_ref().filter(|p| p.pane == pane.entity_id()).map(|p| p.tab);
        let chip = |id: SharedString, glyph: &'static str, label: String, color: u32, active: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .text_color(hex(color))
                .bg(if active { hex_alpha(color, 0.25) } else { hex_alpha(color, 0.12) })
                .hover(|s| s.bg(hex_alpha(color, 0.3)))
                .child(icon(glyph, 12., hex(color)))
                .child(label)
        };
        let mut row = div().flex().items_center().gap_1();
        if links > 0 || connectable {
            let target = pane.clone();
            row = row.child(
                chip(
                    SharedString::from(format!("collab-links-{pane_id}")),
                    "link",
                    if links > 0 { links.to_string() } else { String::new() },
                    if live {
                        Chrome::ATTENTION
                    } else if links > 0 {
                        Chrome::BLUE
                    } else {
                        Chrome::MUTED
                    },
                    open == Some(PanelTab::Links),
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, if links > 0 { "collab.links" } else { "collab.connect" }), None))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.toggle_agent_panel(&target, PanelTab::Links, cx);
                })),
            );
        }
        if total > 0 {
            let target = pane.clone();
            let label = if running > 0 { format!("{running}/{total}") } else { total.to_string() };
            row = row.child(
                chip(
                    SharedString::from(format!("collab-agents-{pane_id}")),
                    "bot",
                    label,
                    if running > 0 { Chrome::ORANGE } else { Chrome::MUTED },
                    open == Some(PanelTab::Agents),
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, "collab.agents"), None))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.toggle_agent_panel(&target, PanelTab::Agents, cx);
                })),
            );
        }
        let panel = self.agent_panel.as_ref().filter(|p| p.pane == pane.entity_id()).map(|panel| self.render_agent_panel(pane, panel, cx));
        Some(div().relative().child(row).children(panel).into_any_element())
    }

    fn render_agent_panel(&self, pane: &Pane, panel: &AgentPanel, cx: &mut Context<Self>) -> AnyElement {
        let body = match panel.tab {
            PanelTab::Agents => self.render_subagents(pane, panel, cx),
            PanelTab::Links => self.render_links(pane, cx),
        };
        let popover = crate::ui::popover()
            .id("agent-panel")
            .w(px(match panel.tab {
                // List + log side by side only when there are transcripts; the hook-reported runs are a short list.
                PanelTab::Agents if panel.subagents.as_ref().is_some_and(|a| !a.is_empty()) => 760.,
                PanelTab::Agents => 380.,
                PanelTab::Links => 520.,
            }))
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.agent_panel.take().is_some() {
                    this.note_dismissed("agent-panel");
                }
                cx.notify();
            }))
            .child(body);
        div()
            .absolute()
            .top_full()
            .left_0()
            .child(
                gpui::deferred(
                    gpui::anchored()
                        .snap_to_window_with_margin(px(8.))
                        .child(div().mt_1().child(crate::ui::fade_in("agent-panel-fade", popover))),
                )
                .with_priority(3),
            )
            .into_any_element()
    }

    fn render_subagents(&self, pane: &Pane, panel: &AgentPanel, cx: &mut Context<Self>) -> AnyElement {
        let now = now_ms();
        let Some(agents) = &panel.subagents else {
            return crate::ui::loading_row(t(cx, "collab.loading")).into_any_element();
        };
        if agents.is_empty() {
            // No transcripts (yet): list what the hooks reported, so the count on the chip matches.
            let runs = pane.read(cx).subagents.clone();
            if runs.is_empty() {
                return crate::ui::hint(t(cx, "collab.no_agents")).into_any_element();
            }
            let mut list = div().flex().flex_col().gap_0p5().p_1().w_full();
            for (index, run) in runs.iter().enumerate().rev() {
                let seconds = run.finished.unwrap_or_else(std::time::Instant::now).duration_since(run.started).as_secs();
                list = list.child(
                    div()
                        .id(("subagent-run", index))
                        .p_2()
                        .rounded_md()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .t_small()
                                .child(if run.finished.is_some() {
                                    icon("circle-check", IconSize::INLINE, hex(Chrome::SUCCESS)).into_any_element()
                                } else {
                                    crate::ui::spinner(IconSize::INLINE, hex(Chrome::ORANGE)).into_any_element()
                                })
                                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(run.kind.clone()))
                                .child(div().flex_1())
                                .child(div().text_color(hex(Chrome::MUTED)).child(super::layout::format_elapsed(seconds))),
                        )
                        .children(run.task.clone().map(|task| div().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(task)))
                        // Finished: what it answered; running: what it is doing now.
                        .children(match (&run.result, &run.last_tool) {
                            (Some(result), _) if run.finished.is_some() => Some(
                                div()
                                    .flex()
                                    .items_start()
                                    .gap_1()
                                    .t_caption()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(icon("message-square", 11., hex(Chrome::MUTED)))
                                    .child(div().min_w_0().line_clamp(2).child(result.clone())),
                            ),
                            (_, Some((tool, target))) => Some(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .t_caption()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(icon("square-terminal", 11., hex(Chrome::MUTED)))
                                    .child(div().truncate().child(crate::terminal::tool_label(tool, target.as_deref()))),
                            ),
                            _ => None,
                        }),
                );
            }
            return div()
                .flex()
                .flex_col()
                .child(div().px_3().pt_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "collab.no_log")))
                .child(list)
                .into_any_element();
        }
        let running = agents.iter().filter(|a| !a.finished).count();
        let mut list =
            div().id("subagent-list").w(px(300.)).flex_shrink_0().h(px(420.)).overflow_y_scroll().flex().flex_col().gap_0p5().p_1();
        for (index, agent) in agents.iter().enumerate() {
            let selected = panel.selected.as_ref() == Some(&agent.id);
            let id = agent.id.clone();
            let status_color = if agent.finished { Chrome::SUCCESS } else { Chrome::ORANGE };
            list = list.child(
                div()
                    .id(("subagent", index))
                    .p_2()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
                    .when(!selected, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(panel) = this.agent_panel.as_mut() {
                            panel.selected = Some(id.clone());
                            panel.log = None;
                            panel.scroll = gpui::ScrollHandle::new();
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .t_small()
                            .child(if agent.finished {
                                icon("circle-check", IconSize::INLINE, hex(status_color)).into_any_element()
                            } else {
                                crate::ui::spinner(IconSize::INLINE, hex(status_color)).into_any_element()
                            })
                            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(agent.agent_type.clone()))
                            .children(agent.model.clone().map(|m| div().text_color(hex(Chrome::MUTED)).child(m)))
                            .child(div().flex_1())
                            .child(div().text_color(hex(Chrome::MUTED)).child(relative_time(now, agent.updated_ms))),
                    )
                    .child(div().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(agent.description.clone()))
                    .children(agent.last_tool.clone().map(|tool| {
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .t_caption()
                            .text_color(hex(Chrome::MUTED))
                            .child(icon("square-terminal", 11., hex(Chrome::MUTED)))
                            .child(div().truncate().child(tool))
                    })),
            );
        }
        let log: AnyElement = match &panel.log {
            None if panel.selected.is_some() => crate::ui::loading_row(t(cx, "viewer.loading")).into_any_element(),
            None => crate::ui::hint(t(cx, "collab.pick_agent")).into_any_element(),
            Some(turns) => {
                let mut column = div().flex().flex_col().gap_2().p_3();
                for turn in turns.iter().rev().take(60).collect::<Vec<_>>().into_iter().rev() {
                    let user = turn.role == Role::User;
                    column = column.child(
                        div()
                            .p_2()
                            .rounded_md()
                            .when(user, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.12)))
                            .t_small()
                            .text_color(hex(if user { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                            .child(div().t_caption().text_color(hex(Chrome::MUTED)).pb_0p5().child(if user {
                                t(cx, "collab.task")
                            } else {
                                t(cx, "collab.reply")
                            }))
                            .children(
                                turn.text.lines().take(40).map(|l| div().child(if l.is_empty() { " ".to_string() } else { l.to_string() })),
                            ),
                    );
                }
                div().id("subagent-log").size_full().overflow_y_scroll().track_scroll(&panel.scroll).child(column).into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .child(icon("bot", IconSize::INLINE, hex(Chrome::FOREGROUND)))
                    .child(div().t_body().font_weight(FontWeight::SEMIBOLD).child(t(cx, "collab.agents")))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                        cx,
                        "collab.agents_count",
                        &[("running", &running.to_string()), ("total", &agents.len().to_string())],
                    ))),
            )
            .child(
                div()
                    .flex()
                    .child(list)
                    .child(div().w(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
                    .child(div().flex_1().min_w_0().h(px(420.)).child(log)),
            )
            .into_any_element()
    }

    fn render_links(&self, pane: &Pane, cx: &mut Context<Self>) -> AnyElement {
        let pane_id = pane.read(cx).pane_id;
        let panes = self.all_panes();
        let edges: Vec<_> = self.flow.edges().iter().filter(|e| e.from == pane_id || e.to == pane_id).cloned().collect();
        let mut list = div().flex().flex_col().gap_1().p_1();
        for (index, edge) in edges.iter().enumerate() {
            let outgoing = edge.from == pane_id;
            let peer_id = if outgoing { edge.to } else { edge.from };
            let Some(peer) = panes.iter().find(|p| p.read(cx).pane_id == peer_id).cloned() else { continue };
            let view = peer.read(cx);
            let (peer_status, peer_color) = super::status_label(view, cx);
            let location = self
                .locate(&peer)
                .map(|(w, tab)| {
                    format!(
                        "{} › {}",
                        self.workspace_title(&self.workspaces[w], cx),
                        tf(cx, "collab.tab", &[("n", &(tab + 1).to_string())])
                    )
                })
                .unwrap_or_default();
            let (from, to, live) = (edge.from, edge.to, edge.live);
            let direct = edge.mode == LinkMode::Direct;
            let (status, color) = match &edge.status {
                // A direct link forwards nothing: the sessions message each other.
                _ if direct => (t(cx, "collab.direct_active").to_string(), Chrome::GREEN),
                EdgeStatus::Sharing => (t(cx, "flow.sharing").to_string(), Chrome::ORANGE),
                EdgeStatus::Shared(n) => (tf(cx, "flow.shared", &[("n", &n.to_string())]), Chrome::SUCCESS),
                EdgeStatus::Failed(error) => (format!("{} · {error}", t(cx, "flow.failed")), Chrome::ERROR),
            };
            let pending = self.flow.pending_for(peer_id);
            let button = |id: SharedString, label: String| {
                div()
                    .id(id)
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .t_small()
                    .cursor_pointer()
                    .bg(hex(0x2d2d30))
                    .text_color(hex(Chrome::FOREGROUND))
                    .hover(|s| s.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
                    .child(label)
            };
            list = list.child(
                div()
                    .p_2()
                    .rounded_md()
                    .bg(hex(0x252526))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .t_small()
                            .child(div().text_color(hex(Chrome::MUTED)).child(if outgoing {
                                t(cx, "collab.sends_to")
                            } else {
                                t(cx, "collab.receives_from")
                            }))
                            .child(crate::brand::avatar(view.tool_id(), 16.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .truncate()
                                    .child(view.display_title()),
                            )
                            .child(div().text_color(hex(Chrome::MUTED)).truncate().child(location))
                            .child(div().flex_1())
                            .when(live, |d| {
                                d.child(
                                    div()
                                        .px_1()
                                        .rounded_sm()
                                        .bg(hex_alpha(Chrome::ATTENTION, 0.2))
                                        .text_color(hex(Chrome::ATTENTION))
                                        .child(t(cx, "flow.live")),
                                )
                            })
                            .when(direct, |d| {
                                d.child(
                                    div()
                                        .px_1()
                                        .rounded_sm()
                                        .bg(hex_alpha(Chrome::GREEN, 0.2))
                                        .text_color(hex(Chrome::GREEN))
                                        .child(t(cx, "collab.direct")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .t_small()
                            .child(div().size(px(6.)).rounded_full().bg(hex(color)))
                            .child(div().text_color(hex(color)).truncate().child(status))
                            .child(div().text_color(hex(Chrome::MUTED)).child("·"))
                            .child(div().text_color(hex(peer_color)).truncate().child(peer_status))
                            .when(pending > 0, |d| {
                                d.child(div().text_color(hex(Chrome::ORANGE)).child(tf(
                                    cx,
                                    "collab.pending",
                                    &[("n", &pending.to_string())],
                                )))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .when(!direct, |d| {
                                d.child(
                                    button(
                                        SharedString::from(format!("link-live-{index}")),
                                        if live { t(cx, "collab.stop_live") } else { t(cx, "collab.make_live") }.to_string(),
                                    )
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.flow_set_live(from, to, !live, cx))),
                                )
                                .child(
                                    button(SharedString::from(format!("link-resync-{index}")), t(cx, "flow.resync").to_string())
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.flow_resync(from, to, cx))),
                                )
                            })
                            .child(button(SharedString::from(format!("link-jump-{index}")), t(cx, "collab.go_to").to_string()).on_click(
                                cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.agent_panel = None;
                                    this.jump_to_pane_id(peer_id, window, cx);
                                }),
                            ))
                            .child(
                                button(SharedString::from(format!("link-remove-{index}")), t(cx, "flow.remove").to_string())
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.flow_disconnect(from, to, cx))),
                            ),
                    ),
            );
        }
        // Connecting from here, instead of going to the Session Flow page.
        let candidates = self.connectable_panes(pane_id, cx);
        let mut connect = div().flex().flex_col().gap_1().px_1().pb_1();
        connect =
            connect.child(
                div()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_1().t_caption().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::MUTED)).child(
                            if edges.is_empty() { t(cx, "collab.connect_first") } else { t(cx, "collab.connect_more") }.to_uppercase(),
                        ),
                    )
                    // Tab names say little when there are many of them: let the user click the pane.
                    .when(!candidates.is_empty(), |d| {
                        d.child(
                            div()
                                .id("connect-pick")
                                .flex()
                                .items_center()
                                .gap_1()
                                .t_small()
                                .text_color(hex(Chrome::BLUE))
                                .cursor_pointer()
                                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                                .child(icon("square-terminal", IconSize::INLINE, hex(Chrome::BLUE)))
                                .child(t(cx, "collab.pick"))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.start_connect_pick(pane_id, cx))),
                        )
                    }),
            );
        if candidates.is_empty() {
            connect = connect.child(crate::ui::hint(t(cx, "collab.no_targets")));
        }
        for (index, peer) in candidates.iter().enumerate() {
            let view = peer.read(cx);
            let peer_id = view.pane_id;
            // Two Claude Code sessions can message each other; anything else gets the conversation.
            let direct = self.direct_peers(pane_id, peer_id, cx).map(|(_, target)| target);
            let location = self
                .locate(peer)
                .map(|(w, tab)| {
                    format!(
                        "{} › {}",
                        self.workspace_title(&self.workspaces[w], cx),
                        tf(cx, "collab.tab", &[("n", &(tab + 1).to_string())])
                    )
                })
                .unwrap_or_default();
            let action = |id: SharedString, label: String, primary: bool| {
                div()
                    .id(id)
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .t_small()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .bg(if primary { hex_alpha(Chrome::ACCENT, 0.9) } else { hex(0x2d2d30) })
                    .text_color(hex(if primary { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                    .hover(|s| s.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
                    .child(label)
            };
            connect = connect.child(
                div()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .t_small()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(crate::brand::avatar(view.tool_id(), 16.))
                    .child(div().min_w_0().truncate().text_color(hex(Chrome::BRIGHT)).child(view.display_title()))
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(location))
                    .when_some(direct.clone(), |d, target| {
                        d.child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::GREEN)).child(target.name.clone())).child(
                            action(SharedString::from(format!("connect-direct-{index}")), t(cx, "collab.direct_connect").to_string(), true)
                                .tooltip(crate::ui::Tooltip::text(t(cx, "collab.direct_hint"), None))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.connect_panes(pane_id, peer_id, false, cx))),
                        )
                    })
                    .when(direct.is_none(), |d| {
                        d.child(
                            action(SharedString::from(format!("connect-share-{index}")), t(cx, "collab.share_context").to_string(), false)
                                .tooltip(crate::ui::Tooltip::text(t(cx, "collab.share_context_hint"), None))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.connect_panes(pane_id, peer_id, false, cx))),
                        )
                        .child(
                            action(SharedString::from(format!("connect-live-{index}")), t(cx, "collab.make_live").to_string(), true)
                                .tooltip(crate::ui::Tooltip::text(t(cx, "collab.make_live_hint"), None))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.connect_panes(pane_id, peer_id, true, cx))),
                        )
                    }),
            );
        }

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .child(icon("link", IconSize::INLINE, hex(Chrome::FOREGROUND)))
                    .child(div().flex_1().t_body().font_weight(FontWeight::SEMIBOLD).child(t(cx, "collab.links")))
                    .when(!edges.is_empty(), |d| {
                        d.child(
                            div()
                                .id("links-disconnect-all")
                                .t_small()
                                .text_color(hex(Chrome::MUTED))
                                .cursor_pointer()
                                .hover(|s| s.text_color(hex(Chrome::ERROR)))
                                .child(t(cx, "collab.disconnect_all"))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.flow_disconnect_all(pane_id, cx))),
                        )
                    })
                    .child(
                        div()
                            .id("links-open-flow")
                            .t_small()
                            .text_color(hex(Chrome::BLUE))
                            .cursor_pointer()
                            .child(t(cx, "collab.open_flow"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.agent_panel = None;
                                this.page = Some(super::Page::Flow);
                                cx.notify();
                            })),
                    ),
            )
            .child(list)
            .child(connect)
            .into_any_element()
    }
}
