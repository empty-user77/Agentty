//! Session Flow: agent panes as nodes; dragging a connection from one to another shares the
//! source session's conversation with the target (a handoff document is written and the target
//! agent is asked to read it).

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use crate::ui::{hint, kind_color, tilde};
use agentty_bridge::claude::PeerSession;
use agentty_bridge::model::Agent;
use gpui::{
    canvas, div, point, prelude::*, px, size, Bounds, ClickEvent, Context, FontWeight, MouseButton, MouseDownEvent, PathBuilder, Pixels,
    Point, SharedString, Window,
};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Prompt that ends a direct link.
fn stop_messaging(name: &str) -> String {
    format!("The link with the Claude Code session \"{name}\" is closed in Agentty: stop sending it messages.")
}

const NODE_WIDTH: f32 = 260.;
/// Distinct colors so sessions from the same workspace are recognizable at a glance.
const WORKSPACE_COLORS: [u32; 8] = [0x4daafc, 0xb180d7, 0x89d185, 0xe8a33d, 0xf14c4c, 0x4ec9b0, 0xd7ba7d, 0xc586c0];
const NODE_HEIGHT: f32 = 128.;
/// Node width plus the gap that holds edge labels.
const COLUMN_STEP: f32 = 470.;

/// Example cards of the onboarding tour: no session behind them, ids no pane ever gets. They exist
/// only while the tour teaches Session Flow, so there is always something to drag there.
pub(super) const DEMO_SOURCE: u64 = u64::MAX - 1;
pub(super) const DEMO_TARGET: u64 = u64::MAX - 2;

fn is_demo(id: u64) -> bool {
    id == DEMO_SOURCE || id == DEMO_TARGET
}

#[derive(Clone, PartialEq)]
pub enum EdgeStatus {
    Sharing,
    Shared(usize),
    Failed(String),
}

/// How two sessions are linked.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    /// The source conversation is written to a document the target agent is asked to read.
    /// Used whenever the two sides can't talk by themselves (Codex, agents started by hand).
    Context,
    /// Both sides are Claude Code sessions registered on this machine, so they message each other
    /// directly with Claude Code's own session messaging; Agentty only introduces them.
    Direct,
}

#[derive(Clone)]
pub struct FlowEdge {
    pub from: u64,
    pub to: u64,
    pub status: EdgeStatus,
    /// Live link: every time the source agent finishes a turn, the new turns are forwarded.
    pub live: bool,
    /// Source turns already delivered through this edge.
    pub sent: usize,
    pub mode: LinkMode,
}

impl FlowEdge {
    fn new(from: u64, to: u64) -> Self {
        Self { from, to, status: EdgeStatus::Sharing, live: false, sent: 0, mode: LinkMode::Context }
    }

    fn direct(from: u64, to: u64) -> Self {
        Self { from, to, status: EdgeStatus::Shared(0), live: false, sent: 0, mode: LinkMode::Direct }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShareMode {
    /// Whole conversation (new connection or manual resync).
    Full,
    /// Only turns added since the last delivery (live links).
    Update,
    /// Advance the cursor without sending (the new turns answer context we injected).
    Skip,
}

enum FlowDrag {
    Move { pane: u64, grab: Point<Pixels> },
    Connect { from: u64 },
}

#[derive(Default)]
pub struct FlowState {
    positions: HashMap<u64, Point<Pixels>>,
    edges: Vec<FlowEdge>,
    drag: Option<FlowDrag>,
    pointer: Point<Pixels>,
    canvas: Rc<Cell<Bounds<Pixels>>>,
    /// Panes whose next finished turn is a reply to context we injected; not forwarded, so two
    /// live links in opposite directions can't ping-pong forever.
    replying: HashSet<u64>,
    /// Prompts waiting for a busy target to finish its current turn.
    pending: HashMap<u64, Vec<String>>,
}

impl FlowState {
    pub fn edges(&self) -> &[FlowEdge] {
        &self.edges
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn drag_to(&mut self, position: Point<Pixels>) {
        self.pointer = position;
        if let Some(FlowDrag::Move { pane, grab }) = &self.drag {
            let origin = self.canvas.get().origin;
            let x = (position.x - origin.x - grab.x).max(px(0.));
            let y = (position.y - origin.y - grab.y).max(px(0.));
            self.positions.insert(*pane, point(x, y));
        }
    }

    /// Prompts queued for a busy pane.
    pub fn pending_for(&self, pane_id: u64) -> usize {
        self.pending.get(&pane_id).map_or(0, |p| p.len())
    }

    pub fn forget(&mut self, pane_id: u64) {
        self.positions.remove(&pane_id);
        self.edges.retain(|e| e.from != pane_id && e.to != pane_id);
        self.replying.remove(&pane_id);
        self.pending.remove(&pane_id);
    }

    fn position(&mut self, pane_id: u64, index: usize) -> Point<Pixels> {
        // As many columns as fit the canvas, with room for edge labels between nodes.
        let width = f32::from(self.canvas.get().size.width);
        // Before the first paint the canvas has no size yet; two columns fit any window.
        let columns = if width < 1. { 2 } else { (((width - 40.) / COLUMN_STEP).floor() as usize).clamp(1, 4) };
        *self.positions.entry(pane_id).or_insert_with(|| {
            let column = (index % columns) as f32;
            let row = (index / columns) as f32;
            point(px(40. + column * COLUMN_STEP), px(60. + row * 220.))
        })
    }

    fn node_at(&self, position: Point<Pixels>) -> Option<u64> {
        let origin = self.canvas.get().origin;
        self.positions.iter().find_map(|(id, p)| {
            let bounds = Bounds::new(point(origin.x + p.x, origin.y + p.y), size(px(NODE_WIDTH), px(NODE_HEIGHT)));
            bounds.contains(&position).then_some(*id)
        })
    }
}

impl Workbench {
    fn agent_panes(&self, cx: &gpui::App) -> Vec<Pane> {
        self.all_panes().into_iter().filter(|p| p.read(cx).is_agent() && p.read(cx).is_running()).collect()
    }

    pub(super) fn finish_flow_drag(&mut self, cx: &mut Context<Self>) {
        let drag = self.flow.drag.take();
        if let Some(FlowDrag::Connect { from }) = drag {
            if let Some(to) = self.flow.node_at(self.flow.pointer).filter(|to| *to != from) {
                // The tour's example cards: a line between them, nothing sent anywhere. An example
                // card and a real session are never linked.
                if is_demo(from) || is_demo(to) {
                    if is_demo(from) && is_demo(to) {
                        self.flow_demo_link(from, to, cx);
                    }
                    return cx.notify();
                }
                if !self.flow.edges.iter().any(|e| e.from == from && e.to == to) {
                    self.flow.edges.push(FlowEdge::new(from, to));
                }
                // `share` leaves a direct link alone: those sessions message each other already.
                self.share(from, to, cx);
            }
        }
        cx.notify();
    }

    /// Links the tour's two example cards (what a drag between them does, or "Do this step for me").
    pub(super) fn flow_demo_link(&mut self, from: u64, to: u64, cx: &mut Context<Self>) {
        if !self.flow.edges.iter().any(|e| e.from == from && e.to == to) {
            self.flow.edges.push(FlowEdge { status: EdgeStatus::Shared(0), ..FlowEdge::new(from, to) });
        }
        self.onboarding_event(super::onboarding::TourEvent::DemoLinked, cx);
        cx.notify();
    }

    pub(super) fn debug_connect(&mut self, from: u64, to: u64, cx: &mut Context<Self>) {
        if !self.flow.edges.iter().any(|e| e.from == from && e.to == to) {
            self.flow.edges.push(FlowEdge::new(from, to));
        }
        self.share(from, to, cx);
    }

    /// Debug: `live` makes the edge live (both directions when `both`).
    pub(super) fn debug_live(&mut self, from: u64, to: u64, both: bool, cx: &mut Context<Self>) {
        self.set_live(from, to, true, cx);
        if both {
            self.connect_back(from, to, cx);
        }
    }

    /// Connects two agent panes from outside the Session Flow page (the link button of a pane).
    /// Two Claude Code sessions are introduced to each other and talk directly; everything else
    /// gets the conversation as a document (optionally kept up to date).
    pub(super) fn connect_panes(&mut self, from: u64, to: u64, live: bool, cx: &mut Context<Self>) {
        if from == to {
            return;
        }
        match self.direct_peers(from, to, cx) {
            Some((source, target)) => self.connect_directly(from, to, source, target, cx),
            None => {
                if !self.flow.edges.iter().any(|e| e.from == from && e.to == to) {
                    self.flow.edges.push(FlowEdge::new(from, to));
                }
                self.share(from, to, cx);
                if live {
                    self.set_live(from, to, true, cx);
                }
            }
        }
        cx.notify();
    }

    /// The registered Claude Code sessions of both panes, when both can message each other.
    pub(super) fn direct_peers(&self, from: u64, to: u64, cx: &gpui::App) -> Option<(PeerSession, PeerSession)> {
        let peer = |pane_id: u64| self.peer_session_of(pane_id, cx);
        let (source, target) = (peer(from)?, peer(to)?);
        (source.session_id != target.session_id).then_some((source, target))
    }

    /// How another Claude Code session can address the session in this pane.
    pub(super) fn peer_session_of(&self, pane_id: u64, cx: &gpui::App) -> Option<PeerSession> {
        let panes = self.all_panes();
        let pane = panes.iter().find(|p| p.read(cx).pane_id == pane_id)?;
        let view = pane.read(cx);
        if view.agent_kind() != Some(crate::launch::PaneKind::Claude) || !view.is_running() {
            return None;
        }
        // A pane Agentty launched knows its session id; for an agent started by hand the id is
        // guessed from the newest transcript in its folder, which is another session's when two of
        // them run in the same folder — so a guess that another pane owns for sure is dropped.
        let launched = view.spec.kind == crate::launch::PaneKind::Claude;
        let id = match (launched, &view.spec.session_id) {
            (true, Some(id)) => id.clone(),
            _ => {
                let guess = view.session_id_live.clone()?;
                let owned_elsewhere = panes.iter().filter(|p| p.read(cx).pane_id != pane_id).any(|p| {
                    let other = p.read(cx);
                    other.spec.kind == crate::launch::PaneKind::Claude && other.spec.session_id.as_deref() == Some(guess.as_str())
                });
                if owned_elsewhere {
                    return None;
                }
                guess
            }
        };
        agentty_bridge::claude::peer_session(&id).filter(|p| p.interactive)
    }

    /// Introduces two Claude Code sessions so they keep talking by themselves.
    fn connect_directly(&mut self, from: u64, to: u64, source: PeerSession, target: PeerSession, cx: &mut Context<Self>) {
        match self.flow.edges.iter_mut().find(|e| e.from == from && e.to == to) {
            Some(edge) => {
                edge.mode = LinkMode::Direct;
                edge.status = EdgeStatus::Shared(0);
            }
            None => self.flow.edges.push(FlowEdge::direct(from, to)),
        }
        self.deliver(
            from,
            format!(
                "You are now working with another Claude Code session on this machine, named \"{}\" (in {}).                  Use ListAgents to confirm it is there, then SendMessage to introduce what you are working on and                  what you need from it. Keep collaborating with it directly: its replies arrive as messages.                  Agentty linked you; nothing else was shared with it yet.",
                target.name,
                target.cwd.display()
            ),
            cx,
        );
        self.deliver(
            to,
            format!(
                "The Claude Code session \"{}\" (in {}) was linked with you in Agentty and will message you shortly.                  When it does, work with it and answer with SendMessage.",
                source.name,
                source.cwd.display()
            ),
            cx,
        );
        self.set_status(tf(cx, "flow.direct_connected", &[("name", &target.name)]), cx);
    }

    /// Agent panes that `pane_id` is not connected to yet, in display order.
    pub(super) fn connectable_panes(&self, pane_id: u64, cx: &gpui::App) -> Vec<Pane> {
        self.agent_panes(cx)
            .into_iter()
            .filter(|p| {
                let other = p.read(cx).pane_id;
                other != pane_id
                    && !self.flow.edges().iter().any(|e| (e.from, e.to) == (pane_id, other) || (e.from, e.to) == (other, pane_id))
            })
            .collect()
    }

    pub(super) fn flow_set_live(&mut self, from: u64, to: u64, live: bool, cx: &mut Context<Self>) {
        self.set_live(from, to, live, cx);
    }

    pub(super) fn flow_resync(&mut self, from: u64, to: u64, cx: &mut Context<Self>) {
        self.share(from, to, cx);
    }

    pub(super) fn flow_disconnect(&mut self, from: u64, to: u64, cx: &mut Context<Self>) {
        let direct = self.flow.edges.iter().any(|e| e.from == from && e.to == to && e.mode == LinkMode::Direct);
        self.flow.edges.retain(|e| !(e.from == from && e.to == to));
        // Sessions that talk by themselves have to be told the link is over.
        if direct {
            let (source, target) = (self.peer_session_of(from, cx), self.peer_session_of(to, cx));
            if let Some(target) = &target {
                self.deliver(from, stop_messaging(&target.name), cx);
            }
            if let Some(source) = &source {
                self.deliver(to, stop_messaging(&source.name), cx);
            }
        }
        cx.notify();
    }

    /// A pane is going away: sessions it was messaging directly are told the link is over before
    /// its links are dropped (closing a tab must not leave the other session talking to nobody).
    pub(super) fn flow_forget_pane(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let mut peers: Vec<u64> = self
            .flow
            .edges()
            .iter()
            .filter(|e| e.mode == LinkMode::Direct)
            .filter_map(|e| match (e.from, e.to) {
                (from, to) if from == pane_id => Some(to),
                (from, to) if to == pane_id => Some(from),
                _ => None,
            })
            .collect();
        peers.sort_unstable();
        peers.dedup();
        if let Some(closing) = self.peer_session_of(pane_id, cx).filter(|_| !peers.is_empty()) {
            for peer in peers {
                self.deliver(peer, stop_messaging(&closing.name), cx);
            }
        }
        self.flow.forget(pane_id);
    }

    /// Disconnects every link of a pane (both directions).
    pub(super) fn flow_disconnect_all(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let pairs: Vec<(u64, u64)> =
            self.flow.edges.iter().filter(|e| e.from == pane_id || e.to == pane_id).map(|e| (e.from, e.to)).collect();
        for (from, to) in pairs {
            self.flow_disconnect(from, to, cx);
        }
    }

    fn set_live(&mut self, from: u64, to: u64, live: bool, cx: &mut Context<Self>) {
        if let Some(edge) = self.flow.edges.iter_mut().find(|e| e.from == from && e.to == to) {
            // Sessions that message each other need no forwarding.
            edge.live = live && edge.mode == LinkMode::Context;
        }
        cx.notify();
    }

    /// Adds the reverse edge as a live link, making a two-way tunnel.
    fn connect_back(&mut self, from: u64, to: u64, cx: &mut Context<Self>) {
        if let Some(edge) = self.flow.edges.iter_mut().find(|e| e.from == to && e.to == from) {
            edge.live = true;
            return cx.notify();
        }
        let mut edge = FlowEdge::new(to, from);
        edge.live = true;
        self.flow.edges.push(edge);
        if let Some(forward) = self.flow.edges.iter_mut().find(|e| e.from == from && e.to == to) {
            forward.live = true;
        }
        self.share(to, from, cx);
    }

    /// Called when an agent finishes a turn: forwards it over live links and delivers prompts
    /// that were waiting for this agent to become idle.
    pub(super) fn flow_agent_finished(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let replying = self.flow.replying.remove(&pane_id);
        let targets: Vec<u64> = self.flow.edges.iter().filter(|e| e.from == pane_id && e.live).map(|e| e.to).collect();
        for to in targets {
            self.share_with(pane_id, to, if replying { ShareMode::Skip } else { ShareMode::Update }, cx);
        }
        if let Some(prompts) = self.flow.pending.remove(&pane_id) {
            if let Some(prompt) = prompts.into_iter().reduce(|a, b| format!("{a}\n\n{b}")) {
                self.deliver(pane_id, prompt, cx);
            }
        }
    }

    /// Submits a prompt to a pane now, or queues it until the agent finishes its current turn.
    fn deliver(&mut self, pane_id: u64, prompt: String, cx: &mut Context<Self>) {
        let Some(target) = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane_id) else { return };
        if target.read(cx).status.in_turn() {
            self.flow.pending.entry(pane_id).or_default().push(prompt);
            return;
        }
        self.flow.replying.insert(pane_id);
        target.update(cx, |view, cx| view.submit_prompt(prompt, cx));
    }

    fn set_edge_status(&mut self, from: u64, to: u64, status: EdgeStatus) {
        if let Some(edge) = self.flow.edges.iter_mut().find(|e| e.from == from && e.to == to) {
            edge.status = status;
        }
    }

    /// Writes the source conversation to a handoff file and submits a prompt to the target.
    /// Hands the target session the source's conversation. Sessions on a direct link talk to each
    /// other themselves, so there is nothing to hand over.
    fn share(&mut self, from: u64, to: u64, cx: &mut Context<Self>) {
        if self.flow.edges.iter().any(|e| e.from == from && e.to == to && e.mode == LinkMode::Direct) {
            return;
        }
        self.share_with(from, to, ShareMode::Full, cx);
    }

    fn share_with(&mut self, from: u64, to: u64, mode: ShareMode, cx: &mut Context<Self>) {
        let panes = self.all_panes();
        let find = |id: u64| panes.iter().find(|p| p.read(cx).pane_id == id).cloned();
        let (Some(source), Some(target)) = (find(from), find(to)) else { return };
        let (Some(from_agent), Some(to_agent)) = (source.read(cx).display_kind().agent(), target.read(cx).display_kind().agent()) else {
            return;
        };
        let since = match mode {
            ShareMode::Full => 0,
            _ => self.flow.edges.iter().find(|e| e.from == from && e.to == to).map_or(0, |e| e.sent),
        };
        if mode != ShareMode::Skip {
            self.set_edge_status(from, to, EdgeStatus::Sharing);
        }

        let view = source.read(cx);
        // Launched agents know their session; agents started by hand in a shell are looked up.
        let known_id = if view.spec.kind == crate::launch::PaneKind::Shell { None } else { view.spec.session_id.clone() };
        let cwd = view.display_cwd();
        let launched = view.launched_at_ms;
        let title = view.display_title();
        let no_transcript = t(cx, "flow.no_transcript").to_string();
        let task = cx.background_spawn(async move {
            let id = match from_agent {
                Agent::Claude => known_id.or_else(|| agentty_bridge::claude::find_recent(&cwd, launched)),
                Agent::Codex => known_id.or_else(|| agentty_bridge::codex::find_recent(&cwd, launched)),
                _ => None,
            }
            .ok_or_else(|| anyhow::anyhow!(no_transcript.clone()))?;
            let mut loaded = agentty_bridge::load(from_agent, &id).map_err(|_| anyhow::anyhow!(no_transcript.clone()))?;
            // Stop hooks can fire just before the final reply reaches the transcript; live updates
            // wait briefly for it so the forwarded turn includes the answer.
            for _ in 0..12 {
                let answered = loaded.1.last().is_some_and(|t| t.role == agentty_bridge::model::Role::Assistant);
                if mode == ShareMode::Full || answered {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(400));
                if let Ok(reloaded) = agentty_bridge::load(from_agent, &id) {
                    loaded = reloaded;
                }
            }
            let (session_cwd, turns) = loaded;
            anyhow::ensure!(!turns.is_empty(), no_transcript);
            let total = turns.len();
            let handoff = match mode {
                ShareMode::Full => Some(agentty_bridge::handoff::create_share(from_agent, &id, &title, to_agent, session_cwd, &turns)?),
                ShareMode::Update if since < total => {
                    Some(agentty_bridge::handoff::create_update(from_agent, &id, &title, to_agent, session_cwd, &turns[since..])?)
                }
                _ => None,
            };
            anyhow::Ok((handoff, total))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((handoff, total)) => {
                        if let Some(edge) = this.flow.edges.iter_mut().find(|e| e.from == from && e.to == to) {
                            edge.sent = total;
                        }
                        if let Some(handoff) = handoff {
                            this.deliver(to, handoff.prompt.clone(), cx);
                            this.set_edge_status(from, to, EdgeStatus::Shared(total));
                            let name = target.read(cx).display_title();
                            this.set_status(tf(cx, "flow.connected", &[("name", &name)]), cx);
                        }
                    }
                    Err(err) if mode == ShareMode::Full => this.set_edge_status(from, to, EdgeStatus::Failed(format!("{err:#}"))),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_flow(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panes = self.agent_panes(cx);
        let mut alive: Vec<u64> = panes.iter().map(|p| p.read(cx).pane_id).collect();
        // The onboarding tour brings two example cards; they go when it moves on.
        let demo = self.tour_shows_flow_demo();
        if demo {
            alive.extend([DEMO_SOURCE, DEMO_TARGET]);
        }
        self.flow.positions.retain(|id, _| alive.contains(id));
        self.flow.edges.retain(|e| alive.contains(&e.from) && alive.contains(&e.to));

        let mut nodes = Vec::new();
        for (index, pane) in panes.iter().enumerate() {
            let id = pane.read(cx).pane_id;
            let position = self.flow.position(id, index);
            nodes.push((pane.clone(), id, position));
        }
        let demo_nodes: Vec<(u64, Point<Pixels>)> = if demo {
            [DEMO_SOURCE, DEMO_TARGET].iter().enumerate().map(|(k, id)| (*id, self.flow.position(*id, panes.len() + k))).collect()
        } else {
            Vec::new()
        };
        let position_of = |id: u64| {
            nodes
                .iter()
                .find(|(_, n, _)| *n == id)
                .map(|(_, _, p)| *p)
                .or_else(|| demo_nodes.iter().find(|(n, _)| *n == id).map(|(_, p)| *p))
        };

        // Edge geometry relative to the canvas: from the source's right handle to the target's left side.
        let mut curves: Vec<(Point<Pixels>, Point<Pixels>, u32)> = Vec::new();
        let mut labels = Vec::new();
        for edge in &self.flow.edges {
            let (Some(a), Some(b)) = (position_of(edge.from), position_of(edge.to)) else { continue };
            let start = point(a.x + px(NODE_WIDTH), a.y + px(NODE_HEIGHT / 2.));
            let end = point(b.x, b.y + px(NODE_HEIGHT / 2.));
            let color = match edge.status {
                EdgeStatus::Sharing => Chrome::ORANGE,
                EdgeStatus::Shared(_) => Chrome::ATTENTION,
                EdgeStatus::Failed(_) => Chrome::ERROR,
            };
            curves.push((start, end, color));
            // A two-way pair shares one line; keep the two labels apart.
            let reverse = self.flow.edges.iter().any(|e| e.from == edge.to && e.to == edge.from);
            let shift = if reverse { px(if edge.from < edge.to { -30. } else { 30. }) } else { px(0.) };
            labels.push((edge.clone(), point((start.x + end.x) / 2., (start.y + end.y) / 2. + shift)));
        }
        let pending = match &self.flow.drag {
            Some(FlowDrag::Connect { from }) => position_of(*from).map(|a| {
                let origin = self.flow.canvas.get().origin;
                (
                    point(a.x + px(NODE_WIDTH), a.y + px(NODE_HEIGHT / 2.)),
                    point(self.flow.pointer.x - origin.x, self.flow.pointer.y - origin.y),
                )
            }),
            _ => None,
        };

        let canvas_bounds = self.flow.canvas.clone();
        let mut area = div().id("flow-canvas").relative().flex_1().min_h_0().overflow_hidden().child(
            canvas(
                move |bounds, _, _| canvas_bounds.set(bounds),
                move |bounds, _, window, _| {
                    let o = bounds.origin;
                    let mut draw = |start: Point<Pixels>, end: Point<Pixels>, color: u32| {
                        let (s, e) = (point(o.x + start.x, o.y + start.y), point(o.x + end.x, o.y + end.y));
                        let bend = px(((f32::from(e.x) - f32::from(s.x)).abs() / 2.).max(60.));
                        let mut path = PathBuilder::stroke(px(2.));
                        path.move_to(s);
                        path.cubic_bezier_to(e, point(s.x + bend, s.y), point(e.x - bend, e.y));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, hex(color));
                        }
                        let mut arrow = PathBuilder::fill();
                        arrow.move_to(e);
                        arrow.line_to(point(e.x - px(9.), e.y - px(5.)));
                        arrow.line_to(point(e.x - px(9.), e.y + px(5.)));
                        arrow.close();
                        if let Ok(arrow) = arrow.build() {
                            window.paint_path(arrow, hex(color));
                        }
                    };
                    for (start, end, color) in &curves {
                        draw(*start, *end, *color);
                    }
                    if let Some((start, end)) = pending {
                        draw(start, end, Chrome::BLUE);
                    }
                },
            )
            .absolute()
            .size_full(),
        );

        for (pane, id, position) in nodes {
            area = area.child(self.render_flow_node(&pane, id, position, cx));
        }
        for (id, position) in demo_nodes {
            area = area.child(self.render_flow_demo_node(id, position, cx));
        }
        for (edge, mid) in labels {
            area = area.child(if is_demo(edge.from) {
                // No controls on an example link: there is nothing to keep live, resend or stop.
                div()
                    .absolute()
                    .left(mid.x - px(90.))
                    .top(mid.y - px(34.))
                    .w(px(180.))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(hex(Chrome::OVERLAY))
                            .border_1()
                            .border_color(hex(Chrome::ATTENTION))
                            .t_caption()
                            .text_color(hex(Chrome::BRIGHT))
                            .child(t(cx, "flow.demo_linked")),
                    )
                    .into_any_element()
            } else {
                self.render_edge_label(edge, mid, cx).into_any_element()
            });
        }
        if panes.is_empty() && !demo {
            area = area.child(div().absolute().top(px(40.)).left(px(40.)).child(hint(t(cx, "flow.empty"))));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(Chrome::EDITOR))
            .child(
                div()
                    .px_5()
                    .py_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(div().t_large().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.flow")))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "flow.hint"))),
            )
            .child(area)
    }

    fn render_flow_node(&self, pane: &Pane, id: u64, position: Point<Pixels>, cx: &mut Context<Self>) -> impl IntoElement {
        let view = pane.read(cx);
        let (status, status_color) = super::status_label(view, cx);
        let connecting = matches!(self.flow.drag, Some(FlowDrag::Connect { from }) if from != id);
        let hovered_target = connecting && self.flow.node_at(self.flow.pointer) == Some(id);
        // Where the session lives: workspace (with a color shared by its sessions) and tab.
        let location = self.locate(pane).map(|(w, tab)| {
            let ws = &self.workspaces[w];
            let tab_count = ws.tabs.len();
            let panes = ws.tabs[tab].root.leaves().len();
            let label = if panes > 1 {
                tf(
                    cx,
                    "flow.location_split",
                    &[("workspace", &self.workspace_title(ws, cx)), ("tab", &(tab + 1).to_string()), ("tabs", &tab_count.to_string())],
                )
            } else {
                tf(
                    cx,
                    "flow.location",
                    &[("workspace", &self.workspace_title(ws, cx)), ("tab", &(tab + 1).to_string()), ("tabs", &tab_count.to_string())],
                )
            };
            (WORKSPACE_COLORS[w % WORKSPACE_COLORS.len()], label)
        });
        let jump = pane.clone();
        let body_jump = pane.clone();
        let accent = kind_color(view.display_kind());

        div()
            .id(("flow-node", id as usize))
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(px(NODE_WIDTH))
            .h(px(NODE_HEIGHT))
            .rounded_lg()
            .bg(hex(Chrome::OVERLAY))
            .border_2()
            .border_color(if hovered_target {
                hex(Chrome::BLUE)
            } else if view.attention {
                hex(Chrome::ATTENTION)
            } else {
                hex(Chrome::OVERLAY_BORDER)
            })
            .shadow_md()
            .child(
                div()
                    .id(("flow-node-header", id as usize))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_t_lg()
                    .bg(hex_alpha(accent, 0.18))
                    .cursor_grab()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let origin = this.flow.canvas.get().origin;
                            let current = this.flow.positions.get(&id).copied().unwrap_or(position);
                            let grab = point(event.position.x - origin.x - current.x, event.position.y - origin.y - current.y);
                            this.flow.drag = Some(FlowDrag::Move { pane: id, grab });
                            this.flow.pointer = event.position;
                        }),
                    )
                    .child(crate::brand::avatar(view.tool_id(), 18.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .t_body()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(view.display_title()),
                    )
                    .child(crate::ui::icon_only(
                        ("flow-open", id as usize),
                        "arrow-up-right",
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            if let Some((w, tab)) = this.locate(&jump) {
                                this.activate_workspace(w, window, cx);
                                this.workspaces[w].active_tab = tab;
                                this.workspaces[w].tabs[tab].active = jump.clone();
                                this.focus_pane(&jump, window, cx);
                            }
                        }),
                    )),
            )
            .child(
                // Clicking the body opens the session (dragging happens on the header and handle).
                div()
                    .id(("flow-node-body", id as usize))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if let Some((w, tab)) = this.locate(&body_jump) {
                            this.activate_workspace(w, window, cx);
                            this.workspaces[w].active_tab = tab;
                            this.workspaces[w].tabs[tab].active = body_jump.clone();
                            this.focus_pane(&body_jump, window, cx);
                        }
                    }))
                    .px_3()
                    .pt_1()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .t_small()
                    .child(div().truncate().text_color(hex(status_color)).child(status))
                    .when_some(location, |d, (color, label)| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .min_w_0()
                                .child(div().flex_shrink_0().size(px(8.)).rounded_sm().bg(hex(color)))
                                .child(div().truncate().text_color(hex(Chrome::FOREGROUND)).child(label)),
                        )
                    })
                    .child(div().truncate().text_color(hex(Chrome::MUTED)).child(tilde(&view.display_cwd()))),
            )
            // Output handle: drag from here onto another node to connect.
            .child(
                div()
                    .id(("flow-handle", id as usize))
                    .absolute()
                    .right(px(-8.))
                    .top(px(NODE_HEIGHT / 2. - 8.))
                    .size(px(16.))
                    .rounded_full()
                    .bg(hex(accent))
                    .border_2()
                    .border_color(hex(Chrome::EDITOR))
                    .cursor_crosshair()
                    .hover(|s| s.bg(hex(Chrome::BLUE)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.flow.drag = Some(FlowDrag::Connect { from: id });
                            this.flow.pointer = event.position;
                            cx.notify();
                        }),
                    ),
            )
            // Input side marker.
            .child(
                div().absolute().left(px(-5.)).top(px(NODE_HEIGHT / 2. - 5.)).size(px(10.)).rounded_full().bg(hex(Chrome::OVERLAY_BORDER)),
            )
    }

    /// An example card of the onboarding tour: looks like a session, can be moved and linked to the
    /// other example card, and has nothing behind it.
    fn render_flow_demo_node(&self, id: u64, position: Point<Pixels>, cx: &mut Context<Self>) -> impl IntoElement {
        let (kind, title) =
            if id == DEMO_SOURCE { (crate::launch::PaneKind::Claude, "Claude Code") } else { (crate::launch::PaneKind::Codex, "Codex") };
        let accent = kind_color(kind);
        let connecting = matches!(self.flow.drag, Some(FlowDrag::Connect { from }) if from != id);
        let hovered_target = connecting && self.flow.node_at(self.flow.pointer) == Some(id);
        // The tour points at the handle of the first card until the two are linked.
        let ring = id == DEMO_SOURCE && self.tour_target() == Some("flow-demo-handle");
        div()
            .id(("flow-demo-node", (u64::MAX - id) as usize))
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(px(NODE_WIDTH))
            .h(px(NODE_HEIGHT))
            .rounded_lg()
            .bg(hex(Chrome::OVERLAY))
            .border_2()
            .border_dashed()
            .border_color(if hovered_target { hex(Chrome::BLUE) } else { hex_alpha(Chrome::WARNING, 0.7) })
            .shadow_md()
            .child(
                div()
                    .id(("flow-demo-header", (u64::MAX - id) as usize))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_t_lg()
                    .bg(hex_alpha(accent, 0.18))
                    .cursor_grab()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let origin = this.flow.canvas.get().origin;
                            let current = this.flow.positions.get(&id).copied().unwrap_or(position);
                            let grab = point(event.position.x - origin.x - current.x, event.position.y - origin.y - current.y);
                            this.flow.drag = Some(FlowDrag::Move { pane: id, grab });
                            this.flow.pointer = event.position;
                        }),
                    )
                    .child(crate::brand::avatar(crate::brand::kind_id(kind), 18.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .t_body()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_1p5()
                            .rounded_sm()
                            .bg(hex_alpha(Chrome::WARNING, 0.25))
                            .t_caption()
                            .text_color(hex(Chrome::WARNING))
                            .child(t(cx, "flow.demo_tag")),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pt_1()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .t_small()
                    .child(
                        div()
                            .text_color(hex(Chrome::FOREGROUND))
                            .child(t(cx, if id == DEMO_SOURCE { "flow.demo_source" } else { "flow.demo_target" })),
                    )
                    .child(div().text_color(hex(Chrome::MUTED)).child(t(cx, "flow.demo_note"))),
            )
            .child(
                div()
                    .id(("flow-demo-handle", (u64::MAX - id) as usize))
                    .absolute()
                    .right(px(-8.))
                    .top(px(NODE_HEIGHT / 2. - 8.))
                    .size(px(16.))
                    .rounded_full()
                    .bg(hex(accent))
                    .border_2()
                    .border_color(hex(Chrome::EDITOR))
                    .cursor_crosshair()
                    .hover(|s| s.bg(hex(Chrome::BLUE)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.flow.drag = Some(FlowDrag::Connect { from: id });
                            this.flow.pointer = event.position;
                            cx.notify();
                        }),
                    ),
            )
            .when(ring, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(-16.))
                        .top(px(NODE_HEIGHT / 2. - 16.))
                        .size(px(32.))
                        .child(crate::ui::pulse_ring("flow-demo-handle", true)),
                )
            })
            .child(
                div().absolute().left(px(-5.)).top(px(NODE_HEIGHT / 2. - 5.)).size(px(10.)).rounded_full().bg(hex(Chrome::OVERLAY_BORDER)),
            )
    }

    fn render_edge_label(&self, edge: FlowEdge, mid: Point<Pixels>, cx: &mut Context<Self>) -> impl IntoElement {
        let (from, to) = (edge.from, edge.to);
        let (text, color) = match &edge.status {
            _ if edge.mode == LinkMode::Direct => (t(cx, "collab.direct").to_string(), Chrome::GREEN),
            EdgeStatus::Sharing => (t(cx, "flow.sharing").to_string(), Chrome::ORANGE),
            EdgeStatus::Shared(n) => (tf(cx, "flow.shared_short", &[("n", &n.to_string())]), Chrome::ATTENTION),
            EdgeStatus::Failed(message) => (format!("{} · {message}", t(cx, "flow.failed")), Chrome::ERROR),
        };
        let key = SharedString::from(format!("edge-{from}-{to}"));
        let live = edge.live;
        let two_way = self.flow.edges.iter().any(|e| e.from == to && e.to == from);
        div().id(key).absolute().left(mid.x - px(120.)).top(mid.y - px(14.)).w(px(240.)).flex().justify_center().child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(color))
                .t_small()
                .child(div().max_w(px(90.)).truncate().text_color(hex(color)).child(text))
                .child(
                    div()
                        .id(SharedString::from(format!("edge-live-{from}-{to}")))
                        .px_1p5()
                        .rounded_full()
                        .cursor_pointer()
                        .border_1()
                        .border_color(hex(if live { Chrome::SUCCESS } else { Chrome::OVERLAY_BORDER }))
                        .bg(if live { hex_alpha(Chrome::SUCCESS, 0.15) } else { hex_alpha(0, 0.) })
                        .text_color(hex(if live { Chrome::SUCCESS } else { Chrome::MUTED }))
                        .child(t(cx, "flow.live"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_live(from, to, !live, cx))),
                )
                .when(!two_way, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("edge-back-{from}-{to}")))
                            .px_1p5()
                            .rounded_full()
                            .cursor_pointer()
                            .border_1()
                            .border_color(hex(Chrome::OVERLAY_BORDER))
                            .text_color(hex(Chrome::MUTED))
                            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                            .child(t(cx, "flow.two_way"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.connect_back(from, to, cx))),
                    )
                })
                .child(crate::ui::icon_only(
                    SharedString::from(format!("edge-resync-{from}-{to}")),
                    "refresh-cw",
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.share(from, to, cx)),
                ))
                .child(crate::ui::icon_only(
                    SharedString::from(format!("edge-remove-{from}-{to}")),
                    "x",
                    // Through `flow_disconnect`, so a direct link also tells both sessions to stop
                    // messaging each other — dropping the edge alone would leave them talking with
                    // nothing left in Agentty to stop them.
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.flow_disconnect(from, to, cx)),
                )),
        )
    }
}
