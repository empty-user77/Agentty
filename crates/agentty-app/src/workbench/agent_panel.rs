//! Collaboration view for one agent pane: its subagents (what each is doing, with its log) and
//! its session links (who it shares context with, and controls for them).

use super::flow::{EdgeStatus, LinkMode};
use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use agentty_bridge::claude::SubagentFile;
use agentty_bridge::subagent_trace::TraceReader;
use gpui::{div, prelude::*, px, AnyElement, Bounds, ClickEvent, Context, EntityId, MouseButton, Pixels, Point, SharedString, Size};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// The link chip picks a terminal; the panel behind it is one modifier away.
/// What ⌘-click does on the link chip, which is the other of the two things it can do.
const LINKS_PANEL_HINT: &str = "⌘ click → links";
const LINKS_PICK_HINT: &str = "⌘ click → connect another";
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Agents,
    Links,
    Schedules,
}

pub struct AgentPanel {
    pub pane: EntityId,
    pub tab: PanelTab,
    /// Sessions ticked in the "connect" list, to link them all at once.
    pub selected_peers: Vec<u64>,
    pub subagents: Option<Vec<SubagentFile>>,
    /// Each subagent's transcript, followed while the panel is open (by subagent id).
    pub traces: Arc<Mutex<HashMap<String, TraceReader>>>,
    pub selected: Option<String>,
    /// Steps of the selected subagent opened to show their output.
    pub expanded: HashSet<usize>,
    pub scroll: gpui::ScrollHandle,
}

impl AgentPanel {
    fn new(pane: EntityId, tab: PanelTab) -> Self {
        Self {
            pane,
            tab,
            selected_peers: Vec::new(),
            subagents: None,
            traces: Arc::default(),
            selected: None,
            expanded: HashSet::new(),
            scroll: gpui::ScrollHandle::new(),
        }
    }
}

/// The subagents view in a window of its own, to keep anywhere on the screen. What it shows lives
/// in the workbench (`popped_out`), which draws it.
pub struct SubagentWindow {
    workbench: gpui::WeakEntity<Workbench>,
}

impl Render for SubagentWindow {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.workbench.upgrade().and_then(|workbench| workbench.update(cx, |this, cx| this.render_popped_out(cx)));
        if body.is_none() {
            // Its workbench window is gone.
            cx.defer_in(window, |_, window, _| window.remove_window());
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(Chrome::OVERLAY))
            .text_color(hex(Chrome::FOREGROUND))
            .font_family(".SystemUIFont")
            .children(body)
    }
}

/// Where the subagents window sits and how big it is: moved by its title bar, resized by its
/// corner, and kept the same while the app runs.
pub struct AgentWindow {
    /// None until it is first moved: it opens under its chip.
    pub origin: Option<Point<Pixels>>,
    pub size: Size<Pixels>,
    /// Where it was drawn last, for a move or resize to start from.
    placed: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The pointer and the window when a move or resize started.
    drag: Option<(WindowDrag, Point<Pixels>, Bounds<Pixels>)>,
    /// Subagents drawn as an office (people at desks) instead of the timeline.
    pub office: bool,
    /// The office's animation is running (one loop at a time).
    pub ticking: bool,
    /// The app window's size at the last frame: the office opens as large as fits in it.
    pub viewport: Size<Pixels>,
    /// The user resized this window: their size wins over the office's.
    resized: bool,
    /// How tall the office drew last (its whole building): the window is no taller than that.
    pub office_height: Rc<Cell<f32>>,
}

impl Default for AgentWindow {
    fn default() -> Self {
        Self {
            origin: None,
            size: gpui::size(px(980.), px(640.)),
            placed: Rc::default(),
            drag: None,
            office: true,
            ticking: false,
            viewport: Size::default(),
            resized: false,
            office_height: Rc::default(),
        }
    }
}

const MIN_WINDOW: (f32, f32) = (560., 320.);

#[derive(Clone, Copy, PartialEq)]
pub enum WindowDrag {
    Move,
    Resize,
}

impl Render for WindowDrag {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
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
        // With the subagents in a window of their own, that window shows this pane's.
        if tab == PanelTab::Agents && self.popout_window.is_some() {
            if !self.popped_out.as_ref().is_some_and(|p| p.pane == id) {
                self.popped_out = Some(AgentPanel::new(id, tab));
                self.follow_subagents(id, cx);
            }
            if let Some(handle) = self.popout_window {
                handle.update(cx, |_, window, _| window.activate_window()).ok();
            }
            return cx.notify();
        }
        self.agent_panel = Some(AgentPanel::new(id, tab));
        if tab == PanelTab::Agents {
            self.follow_subagents(id, cx);
            if self.agent_window.office {
                self.animate_office(cx);
            }
        }
        cx.notify();
    }

    /// The subagents view on screen: in its own window, or in the panel.
    pub(super) fn subagents_shown(&mut self) -> Option<&mut AgentPanel> {
        match self.popped_out.as_mut() {
            Some(panel) => Some(panel),
            None => self.agent_panel.as_mut().filter(|p| p.tab == PanelTab::Agents),
        }
    }

    /// Refreshes a pane's subagents while their view is open: they come and go during a turn,
    /// and each transcript grows. Stops once that view is closed (or replaced).
    fn follow_subagents(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(mine) = self.subagents_shown().map(|p| p.traces.clone()) else { return };
        let ours = move |panel: &&mut AgentPanel| panel.pane == id && Arc::ptr_eq(&panel.traces, &mine);
        cx.spawn(async move |this, cx| loop {
            let Ok(Some((session_id, since, traces))) = this.update(cx, |this, cx| {
                let panel = this.subagents_shown().filter(&ours)?;
                let traces = panel.traces.clone();
                let pane = this.all_panes().into_iter().find(|p| p.entity_id() == id)?;
                Some((pane.read(cx).session_id_live.clone(), pane.read(cx).agent_since_ms, traces))
            }) else {
                break;
            };
            let (agents, changed) = cx
                .background_spawn(async move {
                    let agents = session_id.as_deref().map(agentty_bridge::claude::subagent_files).unwrap_or_default();
                    let mut changed = Vec::new();
                    for agent in &agents {
                        let known = traces.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&agent.id);
                        if known {
                            // Only what was appended since the last round.
                            let mut map = traces.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(reader) = map.get_mut(&agent.id) {
                                if reader.poll().unwrap_or(false) {
                                    changed.push(agent.id.clone());
                                }
                            }
                        } else {
                            // A whole transcript is read outside the lock, so the panel keeps drawing.
                            let mut reader = TraceReader::new(&agent.path);
                            reader.poll().ok();
                            traces.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.id.clone(), reader);
                            changed.push(agent.id.clone());
                        }
                    }
                    // Those the agent process took with it when it ended (the app was closed, say).
                    for reader in traces.lock().unwrap_or_else(|e| e.into_inner()).values_mut() {
                        reader.mark_stopped_before(since);
                    }
                    (agents, changed)
                })
                .await;
            let alive = this.update(cx, |this, cx| {
                // The office opens a log when someone is clicked; the timeline starts with one open.
                let office = this.agent_window.office;
                let Some(panel) = this.subagents_shown().filter(&ours) else { return false };
                if panel.selected.is_none() && !office {
                    // Start with the running one (or the latest).
                    let traces = panel.traces.lock().unwrap_or_else(|e| e.into_inner());
                    let running = agents.iter().rev().find(|a| traces.get(&a.id).is_some_and(|r| !r.trace.ended()));
                    let selected = running.or(agents.last()).map(|a| a.id.clone());
                    drop(traces);
                    panel.selected = selected;
                    panel.scroll.scroll_to_bottom();
                } else if panel.selected.as_ref().is_some_and(|s| changed.contains(s)) {
                    // Follow the newest step, unless the reader scrolled up to look at an earlier one.
                    let (offset, max) = (panel.scroll.offset().y, panel.scroll.max_offset().height);
                    if -offset >= max - px(48.) {
                        panel.scroll.scroll_to_bottom();
                    }
                }
                panel.subagents = Some(agents);
                // Also when nothing changed: running times count up.
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

    /// (all, running) subagents for a pane's chip. A session whose transcripts are followed (an
    /// orchestration's) counts as its strip and page do; another from its hooks and its files.
    pub(super) fn subagent_chip_counts(&self, pane: &Pane, cx: &gpui::App) -> (usize, usize) {
        let view = pane.read(cx);
        if let Some(session) = view.current_session().filter(|s| self.watches_session(s)) {
            let team = self.team_of(&session, Some(pane), cx);
            return (team.all, team.running);
        }
        let (hook_total, hook_running) = (view.subagents.len(), view.subagents.iter().filter(|r| r.running()).count());
        let (file_total, file_active) = view.subagent_files;
        (hook_total.max(file_total), hook_running.max(file_active))
    }

    /// `[⛓ 2] [👥 1/3]` chips for a pane header: session links and subagents, when there are any.
    pub(super) fn render_collab_chips(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = pane.read(cx);
        let pane_id = view.pane_id;
        let links = self.flow.edges().iter().filter(|e| e.from == pane_id || e.to == pane_id).count();
        let live = self.flow.edges().iter().any(|e| (e.from == pane_id || e.to == pane_id) && e.live);
        let (total, running) = self.subagent_chip_counts(pane, cx);
        let now = chrono::Utc::now().timestamp_millis();
        let schedules = view.schedules.iter().filter(|s| s.next_run(now).is_some()).count();
        // The link chip is also the way to *make* a link, so it shows whenever there is another agent.
        let connectable = view.is_agent() && view.is_running() && !self.connectable_panes(pane_id, cx).is_empty();
        // A side chat forks a Claude Code session, so it needs one to fork.
        let side_chat = view.agent_kind() == Some(crate::launch::PaneKind::Claude)
            && view.is_running()
            && view.session_id_live.as_ref().or(view.spec.session_id.as_ref()).is_some();
        if links == 0 && total == 0 && schedules == 0 && !connectable && !side_chat {
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
                .when(active, |d| d.bg(hex_alpha(color, 0.22)))
                .hover(|s| s.bg(hex_alpha(color, 0.22)))
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
                // Its tooltip would sit over the panel it opened.
                .when(open != Some(PanelTab::Links), |d| {
                    d.tooltip(if links > 0 {
                        crate::ui::Tooltip::text(t(cx, "collab.links"), Some(LINKS_PICK_HINT))
                    } else {
                        crate::ui::Tooltip::text(t(cx, "collab.pick"), Some(LINKS_PANEL_HINT))
                    })
                })
                // With nothing linked yet the chip has one job — make the first link — so it goes
                // straight to picking a terminal. Once there are links it opens the panel that
                // manages them, which carries "connect another" itself. ⌘-click is the other one.
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if (links > 0) != crate::keymap::link_modifier(&event.modifiers()) {
                        this.toggle_agent_panel(&target, PanelTab::Links, cx);
                    } else {
                        this.start_connect_pick(pane_id, cx);
                    }
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
                .when(open != Some(PanelTab::Agents), |d| d.tooltip(crate::ui::Tooltip::text(t(cx, "collab.agents"), None)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.toggle_agent_panel(&target, PanelTab::Agents, cx);
                })),
            );
        }
        if side_chat {
            let target = pane.clone();
            row = row.child(
                chip(SharedString::from(format!("collab-side-chat-{pane_id}")), "git-fork", String::new(), Chrome::MUTED, false)
                    .tooltip(crate::ui::Tooltip::text(t(cx, "side_chat.open"), Some(t(cx, "side_chat.hint"))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        // A session that has said nothing yet has nothing to copy.
                        let view = target.read(cx);
                        let session = view.session_id_live.clone().or_else(|| view.spec.session_id.clone());
                        if !session.is_some_and(|id| agentty_bridge::claude::exists(&id)) {
                            return this.show_toast(t(cx, "side_chat.empty"), cx);
                        }
                        // What it does is said first; the copy is made on "Open".
                        this.side_chat_confirm = Some(target.clone());
                        window.focus(&this.focus_handle);
                        cx.notify();
                    })),
            );
        }
        // Only while the session has something scheduled: the jobs live in its process.
        if schedules > 0 {
            let target = pane.clone();
            row = row.child(
                chip(
                    SharedString::from(format!("collab-schedules-{pane_id}")),
                    "clock",
                    schedules.to_string(),
                    Chrome::PURPLE,
                    open == Some(PanelTab::Schedules),
                )
                .when(open != Some(PanelTab::Schedules), |d| d.tooltip(crate::ui::Tooltip::text(t(cx, "schedules.title"), None)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.toggle_agent_panel(&target, PanelTab::Schedules, cx);
                })),
            );
        }
        // The schedules list goes with its chip once nothing is scheduled any more.
        let panel = self
            .agent_panel
            .as_ref()
            .filter(|p| p.pane == pane.entity_id() && !(p.tab == PanelTab::Schedules && schedules == 0))
            .map(|panel| self.render_agent_panel(pane, panel, cx));
        Some(div().relative().child(row).children(panel).into_any_element())
    }

    /// "Open a side chat?": what it is, before the session is copied.
    pub(super) fn render_side_chat_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pane = self.side_chat_confirm.clone()?;
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                .when(!primary && crate::theme::is_light(), crate::ui::outlined)
                .text_color(hex(if primary { Chrome::ON_ACCENT } else { Chrome::BRIGHT }))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        let point = |text: &'static str| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child("•"))
                .child(div().flex_1().min_w_0().child(text))
        };
        Some(
            div()
                .id("side-chat-confirm-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .child(
                    div()
                        .w(px(420.))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .child(
                            div().flex().items_center().gap_2().child(icon("git-fork", 16., hex(Chrome::BRIGHT))).child(
                                div()
                                    .t_title()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(t(cx, "side_chat.confirm_title")),
                            ),
                        )
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "side_chat.confirm_body")))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(point(t(cx, "side_chat.point_original")))
                                .child(point(t(cx, "side_chat.point_context")))
                                .child(point(t(cx, "side_chat.point_files"))),
                        )
                        .child(
                            div()
                                .pt_1()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(button("side-chat-cancel", t(cx, "confirm.cancel"), false).on_click(cx.listener(
                                    |this, _: &ClickEvent, _, cx| {
                                        this.side_chat_confirm = None;
                                        cx.notify();
                                    },
                                )))
                                .child(button("side-chat-open", t(cx, "side_chat.confirm_open"), true).on_click(cx.listener(
                                    move |this, _: &ClickEvent, window, cx| {
                                        this.side_chat_confirm = None;
                                        this.open_side_chat(&pane, window, cx);
                                    },
                                ))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// A side chat beside `pane`: a fork of its session, which starts with the whole conversation
    /// so far and goes its own way, while the original carries on with its work untouched.
    pub(super) fn open_side_chat(&mut self, pane: &Pane, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let view = pane.read(cx);
        let Some(session) = view.session_id_live.clone().or_else(|| view.spec.session_id.clone()) else { return };
        let cwd = view.current_dir();
        let mut spec =
            crate::launch::LaunchSpec::resume(agentty_bridge::model::Agent::Claude, session.clone(), t(cx, "side_chat.title").into(), cwd);
        spec.start = crate::launch::Start::Fork(session);
        spec.session_id = None;
        spec.model = view.spec.model.clone();
        // Beside the pane it came from, whichever pane was in front.
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
        self.split_with(spec, super::panes::Axis::Horizontal, window, cx);
    }

    fn render_agent_panel(&self, pane: &Pane, panel: &AgentPanel, cx: &mut Context<Self>) -> AnyElement {
        let body = match panel.tab {
            PanelTab::Agents => self.render_subagents(pane, panel, cx),
            PanelTab::Links => self.render_links(pane, cx),
            PanelTab::Schedules => self.render_schedules(pane, cx),
        };
        if panel.tab == PanelTab::Agents && panel.subagents.as_ref().is_some_and(|a| !a.is_empty()) {
            return self.render_agent_window(body, cx);
        }
        let popover = crate::ui::popover()
            .id("agent-panel")
            .w(px(match panel.tab {
                // The hook-reported runs are a short list.
                PanelTab::Agents | PanelTab::Schedules => 380.,
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
        super::layout::bar_popover(crate::ui::fade_in("agent-panel-fade", popover), 3, cx).into_any_element()
    }

    /// The subagents' timeline and logs, as a window over the terminals: it stays open while you
    /// work beside it, moves by its title bar and resizes by its corner.
    fn render_agent_window(&self, body: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let window = &self.agent_window;
        let placed = window.placed.clone();
        let corner = div()
            .id("agent-window-resize")
            .absolute()
            .right_0()
            .bottom_0()
            .size(px(14.))
            .cursor(gpui::CursorStyle::ResizeUpLeftDownRight)
            .child(div().absolute().right(px(3.)).bottom(px(3.)).size(px(6.)).border_r_2().border_b_2().border_color(hex(Chrome::MUTED)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, _| this.start_window_drag(WindowDrag::Resize, event.position)),
            )
            .on_drag(WindowDrag::Resize, |drag, _, _, cx| cx.new(|_| *drag));
        // The office shows the whole building where it can: nearly all of the app window, unless
        // the user sized this window themselves.
        let viewport = (f32::from(window.viewport.width), f32::from(window.viewport.height));
        let fit = window.office && !window.resized && viewport.0 > 0.;
        let size = if fit {
            // Its title bar and borders on top of the building, once the building was measured.
            let content = window.office_height.get();
            let tall = if content > 0. { content + 46. } else { viewport.1 - 110. };
            gpui::size(px((viewport.0 - 48.).clamp(MIN_WINDOW.0, 1360.)), px(tall.min(viewport.1 - 110.).max(MIN_WINDOW.1)))
        } else {
            window.size
        };
        let origin = window.origin.or_else(|| fit.then(|| gpui::point(px((viewport.0 - f32::from(size.width)) / 2.), px(64.))));
        let popover = crate::ui::popover()
            .id("agent-panel")
            .relative()
            .w(size.width)
            .h(size.height)
            .on_drag_move(cx.listener(|this, event: &gpui::DragMoveEvent<WindowDrag>, _, cx| this.drag_window(event.event.position, cx)))
            .child(gpui::canvas(|_, _, _| {}, move |bounds, _, _, _| placed.set(Some(bounds))).absolute().size_full())
            .child(div().flex_1().min_h_0().child(body))
            .child(corner);
        match origin {
            Some(origin) => div()
                .child(gpui::deferred(gpui::anchored().position(origin).snap_to_window_with_margin(px(8.)).child(popover)).with_priority(3))
                .into_any_element(),
            None => super::layout::bar_popover(crate::ui::fade_in("agent-panel-fade", popover), 3, cx).into_any_element(),
        }
    }

    /// The subagents window's title bar: drag it to move the window; × closes it.
    pub(super) fn agent_window_bar(&self, bar: gpui::Div, cx: &mut Context<Self>) -> AnyElement {
        if self.popped_out.is_some() {
            // A window of its own: the system title bar moves it and closes it.
            return bar.into_any_element();
        }
        bar.id("agent-window-bar")
            .cursor_grab()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, _| this.start_window_drag(WindowDrag::Move, event.position)),
            )
            .on_drag(WindowDrag::Move, |drag, _, _, cx| cx.new(|_| *drag))
            .child(div().flex_1())
            .child(
                div()
                    .id("agent-window-pop-out")
                    .p_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex_alpha(0xffffff, 0.08)))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "collab.pop_out"), None))
                    .child(icon("external-link", IconSize::INLINE, hex(Chrome::MUTED)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.pop_out_subagents(cx))),
            )
            .child(
                div()
                    .id("agent-window-close")
                    .p_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex_alpha(0xffffff, 0.08)))
                    .child(icon("x", IconSize::INLINE, hex(Chrome::MUTED)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.agent_panel = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// Moves the subagents view out of the Agentty window into a window of its own.
    pub(super) fn pop_out_subagents(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.agent_panel.take_if(|p| p.tab == PanelTab::Agents) else { return };
        self.popped_out = Some(panel);
        cx.notify();
        if let Some(handle) = self.popout_window {
            handle.update(cx, |_, window, _| window.activate_window()).ok();
            return;
        }
        let workbench = cx.entity();
        let weak = workbench.downgrade();
        let bounds = gpui::Bounds::centered(None, self.agent_window.size, cx);
        let options = gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions { title: Some(t(cx, "collab.agents").into()), ..Default::default() }),
            window_min_size: Some(gpui::size(px(MIN_WINDOW.0), px(MIN_WINDOW.1))),
            ..Default::default()
        };
        // Opened after this update: the window draws its first frame from the workbench at once.
        cx.defer(move |cx| {
            let opened = cx.open_window(options, |_, cx| {
                cx.new(|cx| {
                    // Redrawn whenever the workbench is: the steps arrive there.
                    cx.observe(&workbench, |_, _, cx| cx.notify()).detach();
                    // Closed by its own button: the workbench forgets it (later, it may be mid-update).
                    let workbench = weak.clone();
                    cx.on_release(move |_, cx| {
                        cx.defer(move |cx| {
                            workbench
                                .update(cx, |this, cx| {
                                    this.popped_out = None;
                                    this.popout_window = None;
                                    cx.notify();
                                })
                                .ok();
                        })
                    })
                    .detach();
                    SubagentWindow { workbench: weak }
                })
            });
            let back = workbench.downgrade();
            back.update(cx, |this, cx| {
                match opened {
                    Ok(handle) => this.popout_window = Some(handle),
                    Err(_) => this.agent_panel = this.popped_out.take(),
                }
                cx.notify();
            })
            .ok();
        });
    }

    pub(super) fn close_popped_out(&mut self, cx: &mut Context<Self>) {
        self.popped_out = None;
        if let Some(handle) = self.popout_window.take() {
            handle.update(cx, |_, window, _| window.remove_window()).ok();
        }
    }

    fn render_popped_out(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.popped_out.as_ref()?;
        Some(match &panel.subagents {
            None => crate::ui::loading_row(t(cx, "collab.loading")).into_any_element(),
            Some(agents) if agents.is_empty() => crate::ui::hint(t(cx, "collab.no_agents")).into_any_element(),
            Some(agents) => self.render_subagent_traces(panel, agents, cx),
        })
    }

    fn start_window_drag(&mut self, mode: WindowDrag, pointer: Point<Pixels>) {
        let window = &mut self.agent_window;
        if let Some(bounds) = window.placed.get() {
            window.drag = Some((mode, pointer, bounds));
        }
    }

    fn drag_window(&mut self, pointer: Point<Pixels>, cx: &mut Context<Self>) {
        let window = &mut self.agent_window;
        let Some((mode, from, start)) = window.drag else { return };
        let moved = pointer - from;
        match mode {
            WindowDrag::Move => window.origin = Some(start.origin + moved),
            WindowDrag::Resize => {
                window.resized = true;
                window.origin = Some(start.origin);
                window.size =
                    gpui::size((start.size.width + moved.x).max(px(MIN_WINDOW.0)), (start.size.height + moved.y).max(px(MIN_WINDOW.1)));
            }
        }
        cx.notify();
    }

    fn render_subagents(&self, pane: &Pane, panel: &AgentPanel, cx: &mut Context<Self>) -> AnyElement {
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
            // The order the timeline uses: running first, then the one that started last.
            let mut order: Vec<usize> = (0..runs.len()).collect();
            order.sort_by_key(|&i| (!runs[i].running(), std::cmp::Reverse(runs[i].started)));
            for (index, run) in order.into_iter().map(|i| (i, &runs[i])) {
                let seconds =
                    run.finished.filter(|_| !run.running()).unwrap_or_else(std::time::Instant::now).duration_since(run.started).as_secs();
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
                                .child(if !run.running() {
                                    icon("circle-check", IconSize::INLINE, hex(Chrome::SUCCESS)).into_any_element()
                                } else {
                                    crate::ui::spinner(IconSize::INLINE, hex(Chrome::ORANGE)).into_any_element()
                                })
                                .child(div().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(run.kind.clone()))
                                .child(div().flex_1())
                                .child(div().text_color(hex(Chrome::MUTED)).child(super::layout::format_elapsed(seconds)))
                                .children(run.running().then(|| {
                                    let stopping = pane.read(cx).stopping_subagents.contains(&run.id);
                                    self.stop_subagent_button(pane, &run.id, stopping, cx)
                                })),
                        )
                        .children(run.task.clone().map(|task| div().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(task)))
                        // Finished: what it answered; running: what it is doing now.
                        .children(match (&run.result, &run.last_tool) {
                            (Some(result), _) if !run.running() => Some(
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
        self.render_subagent_traces(panel, agents, cx)
    }

    /// What the session has scheduled, soonest first: when each runs next and what it runs.
    /// Nothing to act on here: a job is cancelled from its session.
    fn render_schedules(&self, pane: &Pane, cx: &mut Context<Self>) -> AnyElement {
        use crate::agent_schedule::ScheduleKind;
        let now = chrono::Utc::now().timestamp_millis();
        let mut schedules: Vec<_> = pane.read(cx).schedules.iter().filter_map(|s| Some((s.next_run(now)?, s.clone()))).collect();
        if schedules.is_empty() {
            return crate::ui::hint(t(cx, "schedules.none")).into_any_element();
        }
        schedules.sort_by_key(|(next, _)| *next);
        let clock = |ms: i64| {
            use chrono::TimeZone;
            // To the nearest minute, as Claude Code itself tells the time.
            chrono::Local.timestamp_millis_opt((ms + 30_000) / 60_000 * 60_000).single().map(|at| {
                let today = chrono::Local::now().date_naive() == at.date_naive();
                if today {
                    at.format("%H:%M").to_string()
                } else {
                    at.format("%m-%d %H:%M").to_string()
                }
            })
        };
        let mut list = div().flex().flex_col().gap_0p5().p_1().w_full();
        for (index, (next, schedule)) in schedules.into_iter().enumerate() {
            let wait = ((next - now) / 1000).max(0) as u64;
            let when = if next <= now { t(cx, "schedules.due").to_string() } else { tf(cx, "schedules.in", &[("time", &until(wait))]) };
            let (kind, rule) = match &schedule.kind {
                ScheduleKind::Cron { cron, recurring: true } => (t(cx, "schedules.recurring"), Some(cron.clone())),
                ScheduleKind::Cron { cron, recurring: false } => (t(cx, "schedules.once"), Some(cron.clone())),
                ScheduleKind::Wakeup => (t(cx, "schedules.loop"), None),
            };
            let expires = schedule.expires_ms().and_then(clock).map(|at| tf(cx, "schedules.expires", &[("time", &at)]));
            list = list.child(
                div()
                    .id(("schedule", index))
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
                            .child(icon(
                                if schedule.kind == ScheduleKind::Wakeup { "rotate-cw" } else { "clock" },
                                IconSize::INLINE,
                                hex(Chrome::PURPLE),
                            ))
                            .child(div().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(kind))
                            .children(rule.map(|rule| div().font_family("monospace").text_color(hex(Chrome::MUTED)).child(rule)))
                            .child(div().flex_1())
                            .child(div().text_color(hex(Chrome::PURPLE)).child(when))
                            .children(clock(next).map(|at| div().text_color(hex(Chrome::MUTED)).child(at)))
                            // Asked to cancel: the session does it on its next turn.
                            .child(if schedule.cancelling {
                                div()
                                    .size(px(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(crate::ui::spinner(IconSize::INLINE, hex(Chrome::MUTED)))
                                    .into_any_element()
                            } else {
                                let (target, wanted) = (pane.clone(), schedule.clone());
                                crate::ui::icon_only_sized(
                                    ("schedule-cancel", index),
                                    "trash-2",
                                    20.,
                                    12.,
                                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.cancel_schedule(&target, &wanted, cx);
                                    }),
                                )
                                .tooltip(crate::ui::Tooltip::text(t(cx, "schedules.cancel"), None))
                                .into_any_element()
                            }),
                    )
                    .children(
                        (!schedule.prompt.is_empty())
                            .then(|| div().t_small().line_clamp(2).text_color(hex(Chrome::FOREGROUND)).child(schedule.prompt.clone())),
                    )
                    .children(expires.map(|text| div().t_caption().text_color(hex(Chrome::MUTED)).child(text))),
            );
        }
        list.into_any_element()
    }

    /// Stops one running subagent at its next tool call; the session and its other subagents go on.
    pub(super) fn stop_subagent(&mut self, pane: &Pane, agent_id: &str, cx: &mut Context<Self>) {
        let agent_id = agent_id.to_string();
        pane.update(cx, |view, _| {
            crate::subagent_stop::request(view.pane_id, &agent_id);
            view.stopping_subagents.insert(agent_id);
        });
        cx.notify();
    }

    /// The stop button of a running subagent, or what it shows once stopped and not yet ended.
    pub(super) fn stop_subagent_button(&self, pane: &Pane, agent_id: &str, stopping: bool, cx: &mut Context<Self>) -> AnyElement {
        if stopping {
            return div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .t_caption()
                .text_color(hex(Chrome::MUTED))
                .child(crate::ui::spinner(11., hex(Chrome::MUTED)))
                .child(t(cx, "collab.stopping"))
                .into_any_element();
        }
        let (target, id) = (pane.clone(), agent_id.to_string());
        crate::ui::icon_only_sized(
            SharedString::from(format!("subagent-stop-{agent_id}")),
            "circle-x",
            20.,
            12.,
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.stop_subagent(&target, &id, cx);
            }),
        )
        .tooltip(crate::ui::Tooltip::text(t(cx, "collab.stop"), None))
        .into_any_element()
    }

    /// Asks the session to cancel `schedule`: the job lives in its process, so only it can. A
    /// message typed while it waits for an answer would answer that prompt instead.
    fn cancel_schedule(&mut self, pane: &Pane, schedule: &crate::agent_schedule::AgentSchedule, cx: &mut Context<Self>) {
        if pane.read(cx).status.needs_user() {
            return self.show_toast(t(cx, "schedules.cancel_waiting"), cx);
        }
        let prompt = crate::agent_schedule::cancel_prompt(schedule);
        pane.update(cx, |view, cx| {
            if let Some(found) = view.schedules.iter_mut().find(|s| s.id == schedule.id && s.kind == schedule.kind) {
                found.cancelling = true;
            }
            view.submit_prompt(prompt, cx);
        });
        cx.notify();
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
                        self.workspace_label(&self.workspaces[w], cx),
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
                    .when(crate::theme::is_light(), crate::ui::outlined)
                    .text_color(hex(Chrome::FOREGROUND))
                    .hover(crate::ui::hover_fill)
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
                                    .font_weight(crate::theme::EMPHASIS)
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
                            .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(color)))
                            .child(div().min_w_0().text_color(hex(color)).truncate().child(status))
                            .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child("·"))
                            .child(div().min_w_0().text_color(hex(peer_color)).truncate().child(peer_status))
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
                        div().flex_1().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(
                            if edges.is_empty() { t(cx, "collab.connect_first") } else { t(cx, "collab.connect_more") }.to_uppercase(),
                        ),
                    )
                    // Tab names say little when there are many of them: let the user click the pane.
                    // A button, not a line of text: with links already made this is the way back
                    // into picking another terminal, and it has to be found at a glance.
                    .when(!candidates.is_empty(), |d| {
                        d.child(
                            div()
                                .id("connect-pick")
                                .flex()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .t_small()
                                .cursor_pointer()
                                .border_1()
                                .border_color(hex_alpha(Chrome::BLUE, 0.6))
                                .text_color(hex(Chrome::BLUE))
                                .hover(|s| s.bg(hex_alpha(Chrome::BLUE, 0.18)).text_color(hex(Chrome::BRIGHT)))
                                .child(icon("square-terminal", IconSize::INLINE, hex(Chrome::BLUE)))
                                .child(t(cx, "collab.pick"))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.start_connect_pick(pane_id, cx))),
                        )
                    }),
            );
        if candidates.is_empty() {
            // Links live in one window: say so rather than leave an empty list with no reason.
            let elsewhere = crate::workbenches(cx)
                .into_iter()
                .filter_map(|handle| handle.read(cx).ok())
                .filter(|other| other.slot != self.slot)
                .map(|other| other.agent_pane_count(cx))
                .sum::<usize>();
            let text = if elsewhere > 0 { t(cx, "collab.other_window") } else { t(cx, "collab.no_targets") };
            connect = connect.child(crate::ui::hint(text));
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
                        self.workspace_label(&self.workspaces[w], cx),
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
                    .when(!(primary) && crate::theme::is_light(), crate::ui::outlined)
                    .text_color(hex(if primary { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                    .hover(crate::ui::hover_fill)
                    .child(label)
            };
            let ticked = self.agent_panel.as_ref().is_some_and(|p| p.selected_peers.contains(&peer_id));
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
                    // Tick several, connect them in one go: one session working with three others
                    // should not be four trips through this list.
                    .child(
                        div()
                            .id(SharedString::from(format!("connect-tick-{index}")))
                            .size(px(14.))
                            .flex_shrink_0()
                            .rounded_sm()
                            .border_1()
                            .border_color(hex(if ticked { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                            .bg(if ticked { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(ticked, |d| d.child(icon("check", 10., hex(Chrome::BRIGHT))))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Some(panel) = this.agent_panel.as_mut() {
                                    match panel.selected_peers.iter().position(|id| *id == peer_id) {
                                        Some(at) => panel.selected_peers.remove(at),
                                        None => {
                                            panel.selected_peers.push(peer_id);
                                            peer_id
                                        }
                                    };
                                }
                                cx.notify();
                            })),
                    )
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

        let ticked: Vec<u64> = self.agent_panel.as_ref().map(|p| p.selected_peers.clone()).unwrap_or_default();
        let ticked: Vec<u64> = ticked.into_iter().filter(|id| candidates.iter().any(|p| p.read(cx).pane_id == *id)).collect();
        if !ticked.is_empty() {
            let (share, live) = (ticked.clone(), ticked.clone());
            connect = connect.child(
                div()
                    .px_2()
                    .pt_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                        cx,
                        "collab.selected",
                        &[("n", &ticked.len().to_string())],
                    )))
                    .child(
                        div()
                            .id("connect-selected-share")
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .t_small()
                            .cursor_pointer()
                            .bg(hex(0x2d2d30))
                            .when(crate::theme::is_light(), crate::ui::outlined)
                            .text_color(hex(Chrome::FOREGROUND))
                            .hover(crate::ui::hover_fill)
                            .child(t(cx, "collab.share_context"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.connect_many(pane_id, &share, false, cx))),
                    )
                    .child(
                        div()
                            .id("connect-selected-live")
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .t_small()
                            .cursor_pointer()
                            .bg(hex_alpha(Chrome::ACCENT, 0.9))
                            .text_color(hex(Chrome::ON_ACCENT))
                            .hover(crate::ui::hover_fill)
                            .child(t(cx, "collab.make_live"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.connect_many(pane_id, &live, true, cx))),
                    ),
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
                    .child(div().flex_1().t_body().font_weight(crate::theme::EMPHASIS).child(t(cx, "collab.links")))
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

/// How long until a schedule runs, to the minute: `45s`, `12m`, `3h 05m`, `2d 4h`.
fn until(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds.div_ceil(60)),
        3600..=86_399 => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
        _ => format!("{}d {}h", seconds / 86_400, (seconds % 86_400) / 3600),
    }
}
