//! Session Flow: agent panes as nodes; dragging a connection from one to another shares the
//! source session's conversation with the target (a handoff document is written and the target
//! agent is asked to read it).

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{hint, kind_color, tilde};
use crate::ui::{Tooltip, Type, TypeScale};
use agentty_bridge::claude::PeerSession;
use agentty_bridge::model::Agent;
use gpui::{
    canvas, div, point, prelude::*, px, size, Bounds, ClickEvent, Context, MouseButton, MouseDownEvent, PathBuilder, Pixels, Point,
    SharedString, Window,
};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Prompt that ends a direct link.
fn stop_messaging(name: &str) -> String {
    format!("The link with the Claude Code session \"{name}\" is closed in Agentty: stop sending it messages.")
}

/// The first of `candidates` (the session a pane is in now, then the one it was launched with)
/// that is registered for messaging and not another pane's.
fn registered_peer(
    candidates: &[Option<String>],
    owned_elsewhere: impl Fn(&str) -> bool,
    registered: impl Fn(&str) -> Option<PeerSession>,
) -> Option<PeerSession> {
    candidates.iter().flatten().filter(|id| !owned_elsewhere(id)).find_map(|id| registered(id))
}

/// What the session a direct link starts from is told: who it works with, that work for that
/// session's project is handed over rather than done here, and how to reach it. Without the plain
/// instruction to delegate, a session told only "introduce yourself" went on doing it all itself.
fn lead_intro(partner: &PeerSession) -> String {
    format!(
        "Agentty linked you with another Claude Code session on this machine: \"{name}\", working in {cwd}.\n\
         - Reach it only with SendMessage, addressed to \"{name}\" exactly (ListAgents shows it if you are unsure).\n\
         - Work that belongs in {cwd} is that session's: hand it over with a clear task — what to do, what you \
         need back, and how to check it — instead of doing it yourself, then carry on with your own part.\n\
         - Its answers arrive as messages. When you need its result, wait for that message rather than \
         redoing the work.\n\
         - If SendMessage can't find \"{name}\", say so here instead of quietly doing its work.\n\
         Start by telling it what you are working on.",
        name = partner.name,
        cwd = partner.cwd.display(),
    )
}

/// What the other side of a direct link is told: who will message it, and to answer that way.
fn partner_intro(lead: &PeerSession) -> String {
    format!(
        "Agentty linked you with the Claude Code session \"{name}\" (working in {cwd}). It will send you \
         tasks as messages. Do each one in this session's project, then report back to it with SendMessage, \
         addressed to \"{name}\" exactly — what you did, what you found, and anything it has to decide. Ask it \
         the same way when something is unclear. Don't wait for anything else: until it writes, carry on as \
         before.",
        name = lead.name,
        cwd = lead.cwd.display(),
    )
}

const NODE_WIDTH: f32 = 260.;
/// Distinct colors so sessions from the same workspace are recognizable at a glance.
const WORKSPACE_COLORS: [u32; 8] = [0x4daafc, 0xb180d7, 0x89d185, 0xe8a33d, 0xf14c4c, 0x4ec9b0, 0xd7ba7d, 0xc586c0];
const NODE_HEIGHT: f32 = 128.;
/// Node width plus the gap that holds edge labels.
const COLUMN_STEP: f32 = 470.;
/// How far the chart zooms out and in, and the step of its buttons (as the other diagrams).
const ZOOM_RANGE: (f32, f32) = (0.3, 2.);
const ZOOM_STEP: f32 = 1.2;
/// Room kept around the nodes when zooming to fit.
const FIT_MARGIN: f32 = 40.;
/// Below this zoom a card drops its open button: the card itself opens the session too.
const SMALL_CARD_ZOOM: f32 = 0.7;
/// How long two Claude Code sessions are waited for to register (a session just started or still
/// resuming) before a link between them settles for a one-way document.
const DIRECT_WAIT_STEP: std::time::Duration = std::time::Duration::from_secs(1);
const DIRECT_WAIT_TRIES: u32 = 10;

fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1)
    } else {
        1.
    }
}

/// How the chart is looked at: its zoom, and how far it is moved inside the canvas (screen
/// pixels). Node positions stay in the chart's own pixels whatever the view.
#[derive(Clone, Copy)]
struct FlowView {
    zoom: f32,
    pan: Point<Pixels>,
}

impl Default for FlowView {
    fn default() -> Self {
        Self { zoom: 1., pan: point(px(0.), px(0.)) }
    }
}

/// The cell `preferred_index` would land on, in the row-major grid the chart lays nodes out on —
/// skipping every cell `existing` already occupies, so two nodes never draw on top of each other
/// no matter what order they were placed in.
fn grid_slot(existing: &[Point<Pixels>], preferred_index: usize, columns: usize) -> Point<Pixels> {
    let columns = columns.max(1);
    let cell = |index: usize| {
        let column = (index % columns) as f32;
        let row = (index / columns) as f32;
        point(px(40. + column * COLUMN_STEP), px(60. + row * 220.))
    };
    let mut index = preferred_index;
    while existing.contains(&cell(index)) {
        index += 1;
    }
    cell(index)
}

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
    /// `grab` is where the card was taken, in the chart's own pixels from its corner.
    Move {
        pane: u64,
        grab: Point<Pixels>,
    },
    Connect {
        from: u64,
    },
    /// The empty canvas dragged: where the pointer went down and the pan then.
    Pan {
        from: Point<Pixels>,
        pan: Point<Pixels>,
    },
}

#[derive(Default)]
pub struct FlowState {
    positions: HashMap<u64, Point<Pixels>>,
    edges: Vec<FlowEdge>,
    drag: Option<FlowDrag>,
    pointer: Point<Pixels>,
    canvas: Rc<Cell<Bounds<Pixels>>>,
    view: FlowView,
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
        match &self.drag {
            Some(FlowDrag::Move { pane, grab }) => {
                let at = self.to_chart(position);
                let (pane, x, y) = (*pane, (at.x - grab.x).max(px(0.)), (at.y - grab.y).max(px(0.)));
                self.positions.insert(pane, point(x, y));
            }
            Some(FlowDrag::Pan { from, pan }) => self.view.pan = *pan + (position - *from),
            _ => {}
        }
    }

    /// A chart point, relative to the canvas, as it is drawn now.
    fn to_canvas(&self, at: Point<Pixels>) -> Point<Pixels> {
        let z = self.view.zoom;
        point(self.view.pan.x + at.x * z, self.view.pan.y + at.y * z)
    }

    /// The chart point under a window position.
    fn to_chart(&self, position: Point<Pixels>) -> Point<Pixels> {
        let (origin, z) = (self.canvas.get().origin, self.view.zoom);
        point((position.x - origin.x - self.view.pan.x) / z, (position.y - origin.y - self.view.pan.y) / z)
    }

    /// Zooms to `zoom`, keeping the chart point under `anchor` (a window position) where it is.
    fn zoom_at(&mut self, zoom: f32, anchor: Point<Pixels>) {
        let at = self.to_chart(anchor);
        let origin = self.canvas.get().origin;
        self.view.zoom = clamp_zoom(zoom);
        let z = self.view.zoom;
        self.view.pan = point(anchor.x - origin.x - at.x * z, anchor.y - origin.y - at.y * z);
    }

    /// Zooms around the middle of the canvas (the zoom buttons).
    fn zoom_middle(&mut self, zoom: f32) {
        let bounds = self.canvas.get();
        self.zoom_at(zoom, bounds.center());
    }

    /// Every card in view (never above 100%), in the middle of the canvas.
    fn fit(&mut self) {
        let size = self.canvas.get().size;
        let (vw, vh) = (f32::from(size.width), f32::from(size.height));
        let mut out = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in self.positions.values() {
            let (x, y) = (f32::from(p.x), f32::from(p.y));
            out = (out.0.min(x), out.1.min(y), out.2.max(x + NODE_WIDTH), out.3.max(y + NODE_HEIGHT));
        }
        if out.0 > out.2 || vw <= 0. || vh <= 0. {
            self.view = FlowView::default();
            return;
        }
        let (w, h) = (out.2 - out.0, out.3 - out.1);
        let z = clamp_zoom(((vw - FIT_MARGIN) / w).min((vh - FIT_MARGIN) / h).min(1.));
        self.view.zoom = z;
        self.view.pan = point(px(vw / 2. - (out.0 + w / 2.) * z), px(vh / 2. - (out.1 + h / 2.) * z));
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
        if let Some(position) = self.positions.get(&pane_id) {
            return *position;
        }
        // As many columns as fit the canvas, with room for edge labels between nodes.
        let width = f32::from(self.canvas.get().size.width);
        // Before the first paint the canvas has no size yet; two columns fit any window.
        let columns = if width < 1. { 2 } else { (((width - 40.) / COLUMN_STEP).floor() as usize).clamp(1, 4) };
        // A pane that drops out of the chart for one frame — a session reconnecting flips
        // `is_running` false and back — loses its slot here and is treated as new when it
        // returns. Its old neighbour never moved, so placing the returning node by list order
        // alone can pick the exact cell that neighbour already sits in: two cards stacked with
        // nothing to tell them apart. Skipping cells already taken is what keeps every node
        // visible no matter how the list churns around it.
        let existing: Vec<Point<Pixels>> = self.positions.values().copied().collect();
        let position = grid_slot(&existing, index, columns);
        self.positions.insert(pane_id, position);
        position
    }

    fn node_at(&self, position: Point<Pixels>) -> Option<u64> {
        let at = self.to_chart(position);
        self.positions.iter().find_map(|(id, p)| {
            let bounds = Bounds::new(*p, size(px(NODE_WIDTH), px(NODE_HEIGHT)));
            bounds.contains(&at).then_some(*id)
        })
    }
}

/// Workspaces a session link holds together, for the sidebar list. A link made from one
/// workspace to another gathers the second under the first; links inside one workspace change
/// nothing, since those panes already share a card.
#[derive(Default)]
pub(super) struct LinkedWorkspaces {
    /// Workspace the link started from → the workspaces that joined it, in the order they did.
    pub members: HashMap<usize, Vec<usize>>,
    /// Every workspace listed under another one, so the list never shows it twice.
    pub followers: HashSet<usize>,
}

/// Gathers linked workspaces, given the links as pairs of workspace indices in the order they
/// were made. A pair of the same workspace (two panes of one card) is left alone.
fn cluster_links(pairs: impl Iterator<Item = (usize, usize)>) -> LinkedWorkspaces {
    let mut root_of: HashMap<usize, usize> = HashMap::new();
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for (from, to) in pairs {
        if from == to {
            continue;
        }
        // The cluster hangs off the workspace the first of its links started from.
        let root = root_of.get(&from).copied().unwrap_or(from);
        // A link back to the root — the "two way" button on an edge, or any ring of links — has
        // nothing to move: taking the root's own list apart here would leave its members listed
        // under no root at all, and the sidebar would stop showing them.
        if to == root {
            continue;
        }
        // Whatever already hangs off the joining workspace comes along with it.
        let moving: Vec<usize> = std::iter::once(to).chain(members.remove(&to).unwrap_or_default()).collect();
        for index in moving {
            if index == root {
                continue;
            }
            match root_of.insert(index, root) {
                Some(previous) if previous == root => continue,
                // It hung off another workspace before; it cannot be in two places at once.
                Some(previous) => {
                    if let Some(list) = members.get_mut(&previous) {
                        list.retain(|i| *i != index);
                    }
                }
                None => {}
            }
            members.entry(root).or_default().push(index);
        }
    }
    LinkedWorkspaces { followers: root_of.keys().copied().collect(), members }
}

impl Workbench {
    /// Reads the links as clusters of workspaces. Nothing is stored: unlinking makes the clusters
    /// disappear on the next frame and the list goes back to its plain order.
    pub(super) fn linked_workspaces(&self, cx: &gpui::App) -> LinkedWorkspaces {
        if self.flow.edges().is_empty() {
            return LinkedWorkspaces::default();
        }
        let mut of_pane: HashMap<u64, usize> = HashMap::new();
        for (index, ws) in self.workspaces.iter().enumerate() {
            for pane in ws.tabs.iter().flat_map(|t| t.root.leaves()) {
                of_pane.insert(pane.read(cx).pane_id, index);
            }
        }
        let pairs = self.flow.edges().iter().filter_map(|edge| Some((*of_pane.get(&edge.from)?, *of_pane.get(&edge.to)?)));
        cluster_links(pairs)
    }

    /// How many agent sessions run in this window (the link panel says when the others are elsewhere).
    pub fn agent_pane_count(&self, cx: &gpui::App) -> usize {
        self.agent_panes(cx).len()
    }

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
                // The same link as the pane's link button makes: two Claude Code sessions are
                // introduced and message each other; anything else gets the conversation as a
                // document. (A drag used to make only the document, so a session linked here
                // never learned it had someone to hand work to.)
                if !self.flow.edges.iter().any(|e| e.from == from && e.to == to) {
                    self.connect_panes(from, to, false, cx);
                } else {
                    // Drawn again over an existing link: bring the other side up to date.
                    self.share(from, to, cx);
                }
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

    /// Links the first panes of workspaces `a` and `b` without sending anything (debug), to see
    /// how linked workspaces are listed.
    pub(super) fn debug_link_workspaces(&mut self, a: usize, b: usize, cx: &mut Context<Self>) {
        let first = |i: usize| self.workspaces.get(i).and_then(|w| w.tabs.first()).map(|t| t.active.read(cx).pane_id);
        if let (Some(from), Some(to)) = (first(a), first(b)) {
            if !self.flow.edges.iter().any(|e| e.from == from && e.to == to) {
                self.flow.edges.push(FlowEdge::direct(from, to));
            }
        }
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
        if let Some((source, target)) = self.direct_peers(from, to, cx) {
            self.connect_directly(from, to, source, target, cx);
            return cx.notify();
        }
        if !(self.runs_claude(from, cx) && self.runs_claude(to, cx)) {
            self.connect_by_context(from, to, live, cx);
            return cx.notify();
        }
        // Two Claude Code sessions not registered yet (just started, or a tab still resuming) can
        // message each other in a moment: wait for them before settling for a one-way document,
        // which would leave the first session working on its own.
        if !self.flow.edges.iter().any(|e| e.from == from && e.to == to) {
            self.flow.edges.push(FlowEdge::new(from, to));
        }
        cx.spawn(async move |this, cx| {
            for _ in 0..DIRECT_WAIT_TRIES {
                cx.background_executor().timer(DIRECT_WAIT_STEP).await;
                let Ok(state) = this.update(cx, |this, cx| {
                    // Disconnected meanwhile: nothing more to do.
                    let wanted = this.flow.edges.iter().any(|e| e.from == from && e.to == to);
                    (wanted, this.direct_peers(from, to, cx))
                }) else {
                    return;
                };
                match state {
                    (false, _) => return,
                    (true, Some((source, target))) => {
                        let _ = this.update(cx, |this, cx| {
                            this.connect_directly(from, to, source, target, cx);
                            cx.notify();
                        });
                        return;
                    }
                    (true, None) => {}
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.flow.edges.iter().any(|e| e.from == from && e.to == to) {
                    this.connect_by_context(from, to, live, cx);
                    this.set_status(t(cx, "flow.one_way"), cx);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Whether the pane runs Claude Code now.
    fn runs_claude(&self, pane_id: u64, cx: &gpui::App) -> bool {
        self.all_panes().iter().any(|p| {
            let view = p.read(cx);
            view.pane_id == pane_id && view.is_running() && view.agent_kind() == Some(crate::launch::PaneKind::Claude)
        })
    }

    /// The link for sessions that can't message each other: the source's conversation handed to
    /// the target as a document (kept up to date with `live`).
    fn connect_by_context(&mut self, from: u64, to: u64, live: bool, cx: &mut Context<Self>) {
        if !self.flow.edges.iter().any(|e| e.from == from && e.to == to) {
            self.flow.edges.push(FlowEdge::new(from, to));
        }
        self.share(from, to, cx);
        if live {
            self.set_live(from, to, true, cx);
        }
    }

    /// Links one session to several at once (the ticked sessions in the panel): one agent working
    /// with a front end, an admin front end and a mobile app is one action, not three.
    pub(super) fn connect_many(&mut self, from: u64, targets: &[u64], live: bool, cx: &mut Context<Self>) {
        for to in targets {
            self.connect_panes(from, *to, live, cx);
        }
        if let Some(panel) = self.agent_panel.as_mut() {
            panel.selected_peers.clear();
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
        // The session the pane's Claude Code is in now comes first: the one its process registered,
        // which follows `/clear` and resumes. Both give the process a new session id, so the id a
        // pane was launched with (every restored tab is a resume) is often no longer registered,
        // and asking for it alone dropped the link to a one-way document without a word. The id it
        // was launched with is the fallback, for a Claude Code too old to register by process.
        let launched = (view.spec.kind == crate::launch::PaneKind::Claude).then(|| view.spec.session_id.clone()).flatten();
        let others: Vec<&Pane> = panes.iter().filter(|p| p.read(cx).pane_id != pane_id).collect();
        // A session another pane is in is that pane's (two agents in one folder share a project).
        let owned_elsewhere = |id: &str| others.iter().any(|p| p.read(cx).session_id_live.as_deref() == Some(id));
        registered_peer(&[view.session_id_live.clone(), launched], owned_elsewhere, |id| {
            agentty_bridge::claude::peer_session(id).filter(|p| p.interactive)
        })
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
        self.deliver(from, lead_intro(&target), cx);
        self.deliver(to, partner_intro(&source), cx);
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
        // Busy, or showing a permission prompt or a question: typed now, the message would answer
        // that prompt instead of reaching the agent. It waits for the turn to end.
        let status = &target.read(cx).status;
        if status.in_turn() || status.needs_user() {
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
            let start = self.flow.to_canvas(point(a.x + px(NODE_WIDTH), a.y + px(NODE_HEIGHT / 2.)));
            let end = self.flow.to_canvas(point(b.x, b.y + px(NODE_HEIGHT / 2.)));
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
                    self.flow.to_canvas(point(a.x + px(NODE_WIDTH), a.y + px(NODE_HEIGHT / 2.))),
                    point(self.flow.pointer.x - origin.x, self.flow.pointer.y - origin.y),
                )
            }),
            _ => None,
        };

        let canvas_bounds = self.flow.canvas.clone();
        let zoom = self.flow.view.zoom;
        let entity = cx.entity().downgrade();
        let mut area = div()
            .id("flow-canvas")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            // Dragging the empty canvas moves the view (a card's header moves the card).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.flow.drag = Some(FlowDrag::Pan { from: event.position, pan: this.flow.view.pan });
                    this.flow.pointer = event.position;
                    cx.notify();
                }),
            )
            .when(matches!(self.flow.drag, Some(FlowDrag::Pan { .. })), |d| d.cursor(gpui::CursorStyle::ClosedHand))
            .child(
                canvas(
                    move |bounds, _, _| canvas_bounds.set(bounds),
                    move |bounds, _, window, _| {
                        // The wheel moves the view; with ⌘ or Ctrl it zooms around the pointer.
                        let entity = entity.clone();
                        window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble || !bounds.contains(&event.position) {
                                return;
                            }
                            let delta = event.delta.pixel_delta(px(16.));
                            let zooming = event.modifiers.platform || event.modifiers.control;
                            let _ = entity.update(cx, |this, cx| {
                                if zooming {
                                    let dy = f32::from(delta.y);
                                    if dy != 0. {
                                        let zoom = this.flow.view.zoom * (1. + dy * 0.004);
                                        this.flow.zoom_at(zoom, event.position);
                                    }
                                } else {
                                    this.flow.view.pan += delta;
                                }
                                cx.notify();
                            });
                            cx.stop_propagation();
                        });
                        let o = bounds.origin;
                        let mut draw = |start: Point<Pixels>, end: Point<Pixels>, color: u32| {
                            let (s, e) = (point(o.x + start.x, o.y + start.y), point(o.x + end.x, o.y + end.y));
                            let bend = px(((f32::from(e.x) - f32::from(s.x)).abs() / 2.).max(60. * zoom));
                            let mut path = PathBuilder::stroke(px((2. * zoom).max(1.)));
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
            area = area.child(div().absolute().top(px(40.)).left(px(40.)).right(px(40.)).child(hint(t(cx, "flow.empty"))));
        } else {
            area = area.child(self.render_flow_zoom(cx));
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
                    .child(
                        div()
                            .flex_shrink_0()
                            .t_body()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(t(cx, "orch.page_title")),
                    )
                    .child(self.render_session_tabs(cx))
                    // The map's own hint, only where the map is.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .t_small()
                            .text_color(hex(Chrome::MUTED))
                            .when(self.session_board.tab == super::session_board::SessionTab::Links, |d| d.child(t(cx, "flow.hint"))),
                    )
                    .child(self.render_new_orchestrator_button(cx)),
            )
            .child(match self.session_board.tab {
                super::session_board::SessionTab::Workflow => {
                    div().flex_1().min_h_0().min_w_0().overflow_hidden().child(self.render_workflow_view(cx)).into_any_element()
                }
                super::session_board::SessionTab::Links => area.into_any_element(),
            })
    }

    /// − 100% + and fit, at the bottom right corner (as the other diagrams have).
    fn render_flow_zoom(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let set = |zoom: fn(&FlowState) -> Option<f32>| {
            move |this: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                match zoom(&this.flow) {
                    Some(zoom) => this.flow.zoom_middle(zoom),
                    None => this.flow.fit(),
                }
                cx.notify();
            }
        };
        let button =
            |id: &'static str, glyph: &'static str, tip: &'static str, zoom: fn(&FlowState) -> Option<f32>, cx: &mut Context<Self>| {
                crate::ui::icon_only(id, glyph, cx.listener(set(zoom))).tooltip(Tooltip::text(SharedString::from(t(cx, tip)), None))
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
            // The bar is no place to start moving the view from.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(button("flow-zoom-out", "minus", "plugins.graph.zoom_out", |f| Some(f.view.zoom / ZOOM_STEP), cx))
            .child(
                div()
                    .id("flow-zoom-reset")
                    .w(px(44.))
                    .text_center()
                    .t_caption()
                    .font_family(super::plugin_ui::MONO)
                    .text_color(hex(Chrome::FOREGROUND))
                    .cursor_pointer()
                    .tooltip(Tooltip::text(SharedString::from(t(cx, "plugins.graph.zoom_reset")), None))
                    .on_click(cx.listener(set(|_| Some(1.))))
                    .child(format!("{}%", (self.flow.view.zoom * 100.).round())),
            )
            .child(button("flow-zoom-in", "plus", "plugins.graph.zoom_in", |f| Some(f.view.zoom * ZOOM_STEP), cx))
            .child(button("flow-zoom-fit", "maximize-2", "flow.zoom_fit", |_| None, cx))
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
        // The card follows the chart's zoom: where it is drawn, and every size inside it.
        let (at, z) = (self.flow.to_canvas(position), self.flow.view.zoom);
        let s = |v: f32| px(v * z);

        div()
            .id(("flow-node", id as usize))
            .absolute()
            .left(at.x)
            .top(at.y)
            .w(s(NODE_WIDTH))
            .h(s(NODE_HEIGHT))
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
                    .gap(s(8.))
                    .px(s(12.))
                    .py(s(8.))
                    .rounded_t_lg()
                    .bg(hex_alpha(accent, 0.18))
                    .cursor_grab()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let current = this.flow.positions.get(&id).copied().unwrap_or(position);
                            let at = this.flow.to_chart(event.position);
                            let grab = point(at.x - current.x, at.y - current.y);
                            this.flow.drag = Some(FlowDrag::Move { pane: id, grab });
                            this.flow.pointer = event.position;
                        }),
                    )
                    .child(crate::brand::avatar(view.tool_id(), 18. * z))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(s(Type::BODY))
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(view.display_title()),
                    )
                    .when(z >= SMALL_CARD_ZOOM, |d| {
                        d.child(crate::ui::icon_only(
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
                        ))
                    }),
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
                    .px(s(12.))
                    .pt(s(4.))
                    .flex()
                    .flex_col()
                    .gap(s(2.))
                    .text_size(s(Type::SMALL))
                    .child(div().truncate().text_color(hex(status_color)).child(status))
                    .when_some(location, |d, (color, label)| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(s(6.))
                                .min_w_0()
                                .child(div().flex_shrink_0().size(s(8.)).rounded_sm().bg(hex(color)))
                                .child(div().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(label)),
                        )
                    })
                    .child(div().truncate().text_color(hex(Chrome::MUTED)).child(tilde(&view.display_cwd()))),
            )
            // Output handle: drag from here onto another node to connect.
            .child(
                div()
                    .id(("flow-handle", id as usize))
                    .absolute()
                    .right(s(-8.))
                    .top(s(NODE_HEIGHT / 2. - 8.))
                    .size(s(16.))
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
            .child(div().absolute().left(s(-5.)).top(s(NODE_HEIGHT / 2. - 5.)).size(s(10.)).rounded_full().bg(hex(Chrome::OVERLAY_BORDER)))
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
        let (at, z) = (self.flow.to_canvas(position), self.flow.view.zoom);
        let s = |v: f32| px(v * z);
        div()
            .id(("flow-demo-node", (u64::MAX - id) as usize))
            .absolute()
            .left(at.x)
            .top(at.y)
            .w(s(NODE_WIDTH))
            .h(s(NODE_HEIGHT))
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
                    .gap(s(8.))
                    .px(s(12.))
                    .py(s(8.))
                    .rounded_t_lg()
                    .bg(hex_alpha(accent, 0.18))
                    .cursor_grab()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let current = this.flow.positions.get(&id).copied().unwrap_or(position);
                            let at = this.flow.to_chart(event.position);
                            let grab = point(at.x - current.x, at.y - current.y);
                            this.flow.drag = Some(FlowDrag::Move { pane: id, grab });
                            this.flow.pointer = event.position;
                        }),
                    )
                    .child(crate::brand::avatar(crate::brand::kind_id(kind), 18. * z))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(s(Type::BODY))
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_1p5()
                            .rounded_sm()
                            .bg(hex_alpha(Chrome::WARNING, 0.25))
                            .text_size(s(Type::CAPTION))
                            .text_color(hex(Chrome::WARNING))
                            .child(t(cx, "flow.demo_tag")),
                    ),
            )
            .child(
                div()
                    .px(s(12.))
                    .pt(s(4.))
                    .flex()
                    .flex_col()
                    .gap(s(2.))
                    .text_size(s(Type::SMALL))
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
                    .right(s(-8.))
                    .top(s(NODE_HEIGHT / 2. - 8.))
                    .size(s(16.))
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
                        .right(s(-16.))
                        .top(s(NODE_HEIGHT / 2. - 16.))
                        .size(px(32.))
                        .child(crate::ui::pulse_ring("flow-demo-handle", true)),
                )
            })
            .child(div().absolute().left(s(-5.)).top(s(NODE_HEIGHT / 2. - 5.)).size(s(10.)).rounded_full().bg(hex(Chrome::OVERLAY_BORDER)))
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

#[cfg(test)]
mod tests {
    use super::{
        cluster_links, grid_slot, lead_intro, partner_intro, registered_peer, FlowState, FlowView, PeerSession, NODE_HEIGHT, NODE_WIDTH,
        ZOOM_RANGE,
    };
    use gpui::{point, px, size, Bounds};

    fn peer(name: &str, session: &str) -> PeerSession {
        PeerSession {
            name: name.into(),
            session_id: session.into(),
            pid: 1,
            cwd: "/work/web".into(),
            status: "idle".into(),
            interactive: true,
        }
    }

    /// A resumed or `/clear`ed tab is in a new session: that one is what is registered, and the
    /// one it was launched with is not looked for first (it used to be, and the link fell back to
    /// a one-way document).
    #[test]
    fn the_session_a_pane_is_in_now_is_the_one_linked() {
        let registry = |id: &str| (id == "now").then(|| peer("web", "now"));
        let candidates = [Some("now".to_string()), Some("launched".to_string())];
        assert_eq!(registered_peer(&candidates, |_| false, registry).map(|p| p.session_id), Some("now".into()));
        // Not known yet: the launched id is still tried.
        let launched_only = |id: &str| (id == "launched").then(|| peer("web", "launched"));
        assert_eq!(
            registered_peer(&[None, Some("launched".into())], |_| false, launched_only).map(|p| p.session_id),
            Some("launched".into())
        );
        // Another pane's session is never this pane's.
        assert!(registered_peer(&candidates, |id| id == "now", registry).is_none());
    }

    /// The first session is told plainly to hand work over and how; the other to report back.
    #[test]
    fn introductions_say_who_to_message_and_to_delegate() {
        let lead = lead_intro(&peer("agentty-web", "a"));
        assert!(lead.contains("SendMessage") && lead.contains("\"agentty-web\""));
        assert!(lead.contains("instead of doing it yourself"));
        let partner = partner_intro(&peer("agentty-be", "b"));
        assert!(partner.contains("report back") && partner.contains("\"agentty-be\""));
    }

    /// A chart shown in a canvas at (100, 50), 800 × 600 on the window.
    fn chart() -> FlowState {
        let flow = FlowState::default();
        flow.canvas.set(Bounds::new(point(px(100.), px(50.)), size(px(800.), px(600.))));
        flow
    }

    /// Zooming with the wheel keeps the point under the pointer where it is, at any zoom.
    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let mut flow = chart();
        flow.view = FlowView { zoom: 1.3, pan: point(px(-40.), px(25.)) };
        let pointer = point(px(420.), px(300.));
        let before = flow.to_chart(pointer);
        flow.zoom_at(0.6, pointer);
        let after = flow.to_chart(pointer);
        assert!((f32::from(before.x) - f32::from(after.x)).abs() < 0.01);
        assert!((f32::from(before.y) - f32::from(after.y)).abs() < 0.01);
        // And chart → canvas → window lands on the pointer again.
        let back = flow.to_canvas(after);
        assert!((f32::from(back.x) + 100. - 420.).abs() < 0.01 && (f32::from(back.y) + 50. - 300.).abs() < 0.01);
    }

    #[test]
    fn zoom_stays_in_range() {
        let mut flow = chart();
        flow.zoom_middle(50.);
        assert_eq!(flow.view.zoom, ZOOM_RANGE.1);
        flow.zoom_middle(0.001);
        assert_eq!(flow.view.zoom, ZOOM_RANGE.0);
        flow.zoom_middle(f32::NAN);
        assert_eq!(flow.view.zoom, 1.);
    }

    /// The reported problem: sessions below the canvas could not be reached. Fit brings every card
    /// into the canvas, never above 100%.
    #[test]
    fn fit_shows_every_card() {
        let mut flow = chart();
        for i in 0..12u64 {
            flow.positions.insert(i, grid_slot(&flow.positions.values().copied().collect::<Vec<_>>(), i as usize, 2));
        }
        flow.fit();
        assert!(flow.view.zoom <= 1. && flow.view.zoom >= ZOOM_RANGE.0);
        for p in flow.positions.values() {
            let (a, b) = (flow.to_canvas(*p), flow.to_canvas(point(p.x + px(NODE_WIDTH), p.y + px(NODE_HEIGHT))));
            assert!(f32::from(a.x) >= 0. && f32::from(a.y) >= 0., "{a:?}");
            assert!(f32::from(b.x) <= 800. && f32::from(b.y) <= 600., "{b:?}");
        }
        // A chart that already fits stays at 100%.
        let mut small = chart();
        small.positions.insert(1, point(px(40.), px(60.)));
        small.fit();
        assert_eq!(small.view.zoom, 1.);
    }

    /// Dropping a link and dragging a card find the card under the pointer in a moved, zoomed view.
    #[test]
    fn cards_are_found_where_they_are_drawn() {
        let mut flow = chart();
        flow.positions.insert(7, point(px(500.), px(700.)));
        flow.view = FlowView { zoom: 0.5, pan: point(px(10.), px(-20.)) };
        // Drawn at pan + p·z = (260, 330) in the canvas: (360, 380) on the window.
        assert_eq!(flow.node_at(point(px(365.), px(385.))), Some(7));
        assert_eq!(flow.node_at(point(px(355.), px(385.))), None);
        // The card's far corner is drawn at half its size.
        assert_eq!(flow.node_at(point(px(360. + NODE_WIDTH * 0.5 - 1.), px(380. + NODE_HEIGHT * 0.5 - 1.))), Some(7));
        assert_eq!(flow.node_at(point(px(360. + NODE_WIDTH * 0.5 + 1.), px(385.))), None);
    }

    /// Links between workspaces gather them under the one the first link started from; two panes
    /// of the same workspace are not a cluster.
    #[test]
    fn links_gather_workspaces_under_their_source() {
        let clusters = cluster_links([(0, 2), (0, 3), (5, 5)].into_iter());
        assert_eq!(clusters.members.get(&0), Some(&vec![2, 3]));
        assert!(clusters.followers.contains(&2) && clusters.followers.contains(&3));
        assert!(!clusters.followers.contains(&0) && !clusters.followers.contains(&5));
    }

    /// A workspace that already carries followers brings them along, and never ends up listed
    /// under two workspaces at once.
    #[test]
    fn a_joining_cluster_moves_whole() {
        let clusters = cluster_links([(1, 2), (2, 3), (0, 1)].into_iter());
        assert_eq!(clusters.members.get(&0), Some(&vec![1, 2, 3]));
        assert_eq!(clusters.members.get(&1), None);
        assert_eq!(clusters.followers.len(), 3);
    }

    /// A link back to the workspace a cluster already hangs off — the "two way" button on an edge
    /// — must not take the cluster apart: its members would be listed nowhere at all.
    #[test]
    fn a_link_back_to_the_root_keeps_the_cluster() {
        let both_ways = cluster_links([(0, 1), (1, 0)].into_iter());
        assert_eq!(both_ways.members.get(&0), Some(&vec![1]));
        assert_eq!(both_ways.followers.len(), 1);
        // The same closing a longer ring.
        let ring = cluster_links([(0, 1), (1, 2), (2, 0)].into_iter());
        assert_eq!(ring.members.get(&0), Some(&vec![1, 2]));
        assert_eq!(ring.followers.len(), 2);
        // Whatever the shape, every follower is listed under exactly one root.
        for clusters in [both_ways, ring] {
            let listed: Vec<usize> = clusters.members.values().flatten().copied().collect();
            assert_eq!(listed.len(), clusters.followers.len());
            assert!(listed.iter().all(|i| clusters.followers.contains(i)));
        }
    }

    #[test]
    fn nothing_is_gathered_without_links() {
        let clusters = cluster_links(std::iter::empty());
        assert!(clusters.members.is_empty() && clusters.followers.is_empty());
    }

    /// A fresh chart: the preferred cell is free, so a node lands exactly where its place in the
    /// list says.
    #[test]
    fn an_empty_chart_uses_the_preferred_cell() {
        assert_eq!(grid_slot(&[], 0, 2), point(px(40.), px(60.)));
        assert_eq!(grid_slot(&[], 1, 2), point(px(510.), px(60.)));
        assert_eq!(grid_slot(&[], 2, 2), point(px(40.), px(280.)));
    }

    /// The exact bug reported live: a pane drops out of the chart for one frame (a session
    /// reconnecting) and loses its slot; when it comes back it is placed by list order alone,
    /// which can be the cell its still-live neighbour never left. Two cards land on top of each
    /// other with nothing to tell them apart. `grid_slot` must never hand out a cell `existing`
    /// already holds, however it got there.
    #[test]
    fn a_returning_node_never_lands_on_a_cell_already_taken() {
        let neighbour = grid_slot(&[], 0, 2);
        // The returning node's own list position also says cell 0 — the same cell a naive
        // `index % columns` would have reused.
        let returning = grid_slot(&[neighbour], 0, 2);
        assert_ne!(returning, neighbour, "two nodes must never share a cell");
        // And it takes the very next free one rather than jumping further than it has to.
        assert_eq!(returning, grid_slot(&[], 1, 2));
    }

    /// Every cell already in use is skipped, not just the first one: three panes already seated
    /// leaves the fourth no choice but the fourth cell.
    #[test]
    fn every_taken_cell_is_skipped_in_order() {
        let taken: Vec<_> = (0..3).map(|i| grid_slot(&[], i, 2)).collect();
        assert_eq!(grid_slot(&taken, 0, 2), grid_slot(&[], 3, 2));
    }

    /// A wide canvas (more columns) and a narrow one (as few as one) both still avoid every
    /// occupied cell — the collision check does not assume any particular column count.
    #[test]
    fn works_at_every_column_count() {
        for columns in 1..=4 {
            let taken: Vec<_> = (0..columns).map(|i| grid_slot(&[], i, columns)).collect();
            let next = grid_slot(&taken, 0, columns);
            assert!(!taken.contains(&next), "columns={columns} let a node land on a taken cell");
        }
    }
}
