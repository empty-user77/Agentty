//! Renders a tab's split tree: pane headers, attention borders and draggable dividers.

use super::panes::{zoom_sizes, Axis, PaneNode};
use super::{status_label, Pane, Tab, Workbench};
use crate::hud::HudItem;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use crate::ui::{icon, IconSize};
use gpui::{
    canvas, div, prelude::*, px, relative, AnyElement, ClickEvent, Context, CursorStyle, MouseButton, MouseDownEvent, Pixels, Point,
    SharedString,
};

const DIVIDER: f32 = 4.;
const PANE_HEADER_HEIGHT: f32 = 26.;

#[derive(Clone)]
pub struct SplitDrag {
    path: Vec<usize>,
    index: usize,
    axis: Axis,
    start: Point<Pixels>,
    start_sizes: Vec<f32>,
}

impl Workbench {
    pub(super) fn render_tab(&self, tab: &Tab, cx: &mut Context<Self>) -> AnyElement {
        self.split_bounds.borrow_mut().clear();
        let split = matches!(tab.root, PaneNode::Split { .. });
        div().size_full().bg(hex(Chrome::PANEL)).child(self.render_node(&tab.root, Vec::new(), split, &tab.active, cx)).into_any_element()
    }

    fn render_node(&self, node: &PaneNode<Pane>, path: Vec<usize>, split: bool, active: &Pane, cx: &mut Context<Self>) -> AnyElement {
        match node {
            PaneNode::Leaf(pane) => self.render_pane(pane, split, pane == active, cx),
            PaneNode::Split { axis, children, sizes } => {
                let axis = *axis;
                let bounds = self.split_bounds.clone();
                let key = path.clone();
                let mut container = div().relative().size_full().flex().when(axis == Axis::Vertical, |d| d.flex_col()).child(
                    canvas(
                        move |b, _, _| {
                            bounds.borrow_mut().insert(key, b);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                );
                // Focus view: the child holding the zoomed pane takes most of the room for a while.
                let zoom_child = self.zoomed.as_ref().and_then(|z| children.iter().position(|c| c.contains(z)));
                for (index, (child, size)) in children.iter().zip(sizes).enumerate() {
                    if index > 0 {
                        container = container.child(self.render_divider(&path, index - 1, axis, cx));
                    }
                    let mut child_path = path.clone();
                    child_path.push(index);
                    let size = match zoom_child {
                        Some(zoomed) => zoom_sizes(children.len(), zoomed)[index],
                        None => *size,
                    };
                    container = container.child(
                        div()
                            .flex_basis(relative(0.))
                            .min_w_0()
                            .min_h_0()
                            .map(|mut d| {
                                d.style().flex_grow = Some(size);
                                d
                            })
                            .child(self.render_node(child, child_path, split, active, cx)),
                    );
                }
                container.into_any_element()
            }
        }
    }

    fn render_divider(&self, path: &[usize], index: usize, axis: Axis, cx: &mut Context<Self>) -> impl IntoElement {
        let id = SharedString::from(format!("divider-{path:?}-{index}"));
        let dragging = matches!(&self.split_drag, Some(d) if d.path == path && d.index == index);
        let path = path.to_vec();
        div()
            .id(id)
            .flex_shrink_0()
            .when(axis == Axis::Horizontal, |d| d.w(px(DIVIDER)).h_full().cursor(CursorStyle::ResizeLeftRight))
            .when(axis == Axis::Vertical, |d| d.h(px(DIVIDER)).w_full().cursor(CursorStyle::ResizeUpDown))
            .bg(if dragging { hex(Chrome::ACCENT) } else { hex(Chrome::BORDER) })
            .hover(|s| s.bg(hex_alpha(Chrome::ACCENT, 0.7)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    // Focus view draws its own proportions over the saved sizes, so a divider
                    // dragged while it is on would not move: leave it, keeping what is on screen.
                    this.leave_zoom_keeping_layout(cx);
                    let Some(start_sizes) = this.active_tab_mut().and_then(|tab| tab.root.sizes_at(&path)) else { return };
                    this.split_drag = Some(SplitDrag { path: path.clone(), index, axis, start: event.position, start_sizes });
                    cx.notify();
                }),
            )
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        let ws = self.workspaces.get_mut(self.active_workspace)?;
        ws.tabs.get_mut(ws.active_tab)
    }

    /// Turns focus view off, making its proportions the tab's real split sizes.
    fn leave_zoom_keeping_layout(&mut self, cx: &mut Context<Self>) {
        let Some(zoomed) = self.zoomed.take() else { return };
        for tab in self.workspaces.iter_mut().flat_map(|ws| ws.tabs.iter_mut()) {
            if tab.root.contains(&zoomed) {
                tab.root.apply_zoom(&zoomed);
            }
        }
        cx.notify();
    }

    pub(super) fn drag_split(&mut self, drag: SplitDrag, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(bounds) = self.split_bounds.borrow().get(&drag.path).copied() else { return };
        let (delta, length) = match drag.axis {
            Axis::Horizontal => (position.x - drag.start.x, bounds.size.width),
            Axis::Vertical => (position.y - drag.start.y, bounds.size.height),
        };
        if f32::from(length) <= 0. {
            return;
        }
        let fraction = f32::from(delta) / f32::from(length);
        let Some(ws) = self.workspaces.get_mut(self.active_workspace) else { return };
        let Some(tab) = ws.tabs.get_mut(ws.active_tab) else { return };
        tab.root.resize(&drag.path, drag.index, &drag.start_sizes, fraction);
        cx.notify();
    }

    fn render_pane(&self, pane: &Pane, split: bool, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let mut chip = pane.read(cx).git_branch.clone().filter(|_| split).map(|b| self.branch_chip(pane, b, cx));
        let mut tree_chip = split.then(|| self.worktree_chip(pane, cx)).flatten();
        let mut collab = split.then(|| self.render_collab_chips(pane, cx)).flatten();
        let mut port_chips = split.then(|| self.port_chips(pane, cx)).flatten();
        let hud = crate::hud::normalized(&crate::settings::settings(cx).hud);
        let width = self.pane_bounds.borrow().get(&pane.entity_id()).map(|b| f32::from(b.size.width)).unwrap_or(f32::MAX);
        let mut context_meter = {
            let shown = split && pane.read(cx).is_agent() && crate::settings::settings(cx).agent_bar;
            let percent = pane.read(cx).stats.as_ref().and_then(|s| s.context_percent());
            // A narrow pane keeps the figure and gives up the word and the bar, so the model's full
            // name and the pane's own buttons still fit.
            shown.then(|| self.context_meter(pane, percent, width < COMPACT_METER_WIDTH, cx))
        };
        let view = pane.read(cx);
        let prefs_bar = crate::settings::settings(cx).agent_bar;
        let bars_below = bars_below(cx);
        let attention = view.attention;
        let border = if attention {
            hex(Chrome::ATTENTION)
        } else if split && active {
            hex_alpha(Chrome::ACCENT, 0.6)
        } else {
            hex_alpha(0, 0.)
        };
        let (status, status_color) = status_label(view, cx);
        let pane_for_focus = pane.clone();

        // Split pane header: which tool, where (project folder + path), its live status and branch.
        let mut header = split.then(|| {
            let (zoom, close) = (pane.clone(), pane.clone());
            let zoomed = self.zoomed.as_ref() == Some(pane);
            let cwd = view.display_cwd();
            let folder = cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| crate::ui::tilde(&cwd));
            let agent_info = view.is_agent() && prefs_bar;
            div()
                .id(("pane-header", pane.entity_id().as_u64() as usize))
                .h(px(PANE_HEADER_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .pl_2()
                .pr_0p5()
                .overflow_hidden()
                .bg(if active { hex(Chrome::SELECTED) } else { hex(Chrome::TAB_INACTIVE) })
                .t_small()
                .text_color(if active { hex(Chrome::BRIGHT) } else { hex(Chrome::MUTED) })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.mark_active(&pane_for_focus, cx);
                    this.focus_pane(&pane_for_focus, window, cx);
                    pane_for_focus.update(cx, |v, cx| v.acknowledge(cx));
                }))
                // The icon is the grip: dragging it moves the pane, while the buttons further
                // right keep their clicks (they stop propagation of their own).
                .child(
                    div()
                        .id(("pane-grip", pane.entity_id().as_u64() as usize))
                        .cursor(gpui::CursorStyle::OpenHand)
                        .tooltip(crate::ui::Tooltip::text(crate::i18n::t(cx, "pane.move_or_zoom_hint"), None))
                        // Double-clicking the grip enlarges the pane and puts it back, like the
                        // status further along the bar: the two things a bar is grabbed by.
                        .on_click({
                            let target = pane.clone();
                            cx.listener(move |this, event: &ClickEvent, window, cx| {
                                if event.click_count() < 2 {
                                    return;
                                }
                                cx.stop_propagation();
                                this.toggle_zoom_from_bar(&target, window, cx);
                            })
                        })
                        .on_drag(
                            super::drop_split::DraggedPane { pane_id: view.pane_id, title: view.display_title().into() },
                            |dragged, _, _, cx| {
                                super::drop_split::note_pane_drag(dragged.pane_id);
                                cx.new(|_| super::chrome::DragPreview { title: dragged.title.clone() })
                            },
                        )
                        .child(crate::brand::avatar(view.tool_id(), 16.)),
                )
                // The items of the status bar, in the user's order (Settings → Appearance), in a row of
                // their own: where the pane is too narrow for all of them, they give way at its end,
                // never the pane's own buttons after it.
                .child({
                    let mut d = div().flex().items_center().gap_2().flex_1().min_w_0().overflow_hidden();
                    for entry in hud.iter().filter(|e| e.visible) {
                        d = match entry.item {
                            // The model's full name comes first on the bar: never cut, whatever else
                            // has to give. Short of room, the window shrinks to "(1M)" instead.
                            HudItem::Model => {
                                d.when_some(pane_model_label(view, self.installed.as_ref()).filter(|_| agent_info), |d, model| {
                                    d.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_color(hex(Chrome::BRIGHT))
                                            .child(model.text(width < FULL_MODEL_LABEL_WIDTH)),
                                    )
                                })
                            }
                            HudItem::Context => d.children(context_meter.take()),
                            HudItem::Usage => d.when_some(
                                view.usage_percent().filter(|p| *p >= USAGE_SHOWN_AT && agent_info && width >= 700.),
                                |d, percent| d.child(meter("Usage", percent)),
                            ),
                            // Double-clicking the status enlarges this pane and puts it back:
                            // the header's focus-view button without aiming at a 22 pt icon.
                            HudItem::Status => d.when(agent_info, |d| {
                                let target = pane.clone();
                                d.child(
                                    div()
                                        .id(("pane-status", pane.entity_id().as_u64() as usize))
                                        .flex_shrink()
                                        .min_w(px(40.))
                                        .max_w(px(220.))
                                        .truncate()
                                        .text_color(hex(status_color))
                                        .tooltip(crate::ui::Tooltip::text(crate::i18n::t(cx, "pane.zoom_hint"), None))
                                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                            if event.click_count() < 2 {
                                                return;
                                            }
                                            cx.stop_propagation();
                                            this.toggle_zoom_from_bar(&target, window, cx);
                                        }))
                                        .child(status.clone()),
                                )
                            }),
                            HudItem::Elapsed => d.when_some(
                                view.working_since.filter(|_| agent_info && width >= 560.).map(|t| format_elapsed(t.elapsed().as_secs())),
                                |d, elapsed| d.child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(elapsed)),
                            ),
                            HudItem::Links => d.children(collab.take()),
                            HudItem::Spacer => d.child(div().flex_1()),
                            HudItem::Ports => d.children(port_chips.take()),
                            HudItem::Worktree => d.children(tree_chip.take()),
                            HudItem::Branch => d.children(chip.take()),
                            // Where this pane is: project folder, with the path (cut in the middle) when
                            // there is room; the whole path on hover, ⌘-click shows it in the file manager.
                            HudItem::Folder => d.child(
                                div()
                                    .id(("pane-folder", pane.entity_id().as_u64() as usize))
                                    .tooltip(crate::ui::Tooltip::text(crate::ui::tilde(&cwd), Some(REVEAL_HINT)))
                                    .on_click({
                                        let cwd = cwd.clone();
                                        move |event: &ClickEvent, _, cx| {
                                            if crate::keymap::link_modifier(&event.modifiers()) {
                                                cx.stop_propagation();
                                                crate::platform::reveal(&cwd);
                                            }
                                        }
                                    })
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .min_w_0()
                                    .flex_shrink()
                                    // In a narrow pane the folder name gives way (…) instead of running into the branch.
                                    .overflow_hidden()
                                    .child(beside_text(icon("folder", 12., hex(Chrome::MUTED))))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_shrink()
                                            .max_w(px(160.))
                                            .truncate()
                                            .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                                            .child(folder.clone()),
                                    )
                                    .when(width >= 620., |d| {
                                        let room = ((width - 560.) / 7.5).clamp(14., 48.) as usize;
                                        d.child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_color(hex(Chrome::MUTED))
                                                .child(crate::ui::middle_ellipsis(&crate::ui::tilde(&cwd), room)),
                                        )
                                    }),
                            ),
                        };
                    }
                    d
                })
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        // Always there, in every pane: enlarging or closing a pane should not need a hover first.
                        .child(
                            small_icon_button(
                                ("pane-zoom", pane.entity_id().as_u64() as usize),
                                if zoomed { "minimize-2" } else { "maximize-2" },
                                cx.listener(move |this, _: &ClickEvent, window, cx| this.toggle_zoom(&zoom, window, cx)),
                            )
                            .tooltip(crate::ui::Tooltip::text(
                                crate::i18n::t(cx, if zoomed { "tooltip.unzoom" } else { "tooltip.zoom" }),
                                Some("⇧⌘↩"),
                            ))
                            .when(zoomed, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.35))),
                        )
                        .child(
                            small_icon_button(
                                ("pane-close", pane.entity_id().as_u64() as usize),
                                "x",
                                cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.request_close_pane(&close, window, cx);
                                }),
                            )
                            .tooltip(crate::ui::Tooltip::text(crate::i18n::t(cx, "close.pane"), Some("⌘W"))),
                        ),
                )
        });

        // Dropping a tab here merges it as a split; picking turns panes into link targets.
        let drop_zones = (cx.has_active_drag() && self.drop_target(pane, cx)).then(|| self.render_tab_drop_zones(pane, cx));
        let connect_pick = self.render_connect_pick(pane, cx);
        let resume_hint = self.render_resume_hint(pane, cx);
        let find_bar = self.render_find_bar(pane, cx);
        // Single agent panes get a slim live status bar; split panes carry the same info in their header.
        let mut agent_bar = (!split && prefs_bar).then(|| self.render_agent_bar(pane, width, cx)).flatten();
        let widths = self.pane_bounds.clone();
        let id = pane.entity_id();

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .border_2()
            .border_color(border)
            .child(
                canvas(
                    move |bounds, _, _| {
                        widths.borrow_mut().insert(id, bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            // The pane's bar: above the terminal, or under it (Settings → Appearance).
            .when(!bars_below, |d| d.children(header.take()).children(agent_bar.take()))
            .children(resume_hint)
            // Cached: the grid is only re-laid out when this terminal notifies (output, cursor,
            // focus), not when unrelated parts of the window re-render (e.g. sidebar scrolling).
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(gpui::AnyView::from(pane.clone()).cached(gpui::StyleRefinement::default().size_full()))
                    .children(find_bar)
                    .children(drop_zones)
                    .children(connect_pick),
            )
            .children(header)
            .children(agent_bar)
            .into_any_element()
    }

    /// Focus view from a pane's bar: double-clicking its status, or the icon it is dragged by.
    /// A tab holding a single pane already fills the tab, so there the double click is left alone
    /// rather than marking a pane zoomed that nothing would show as zoomed until the tab is split.
    pub(super) fn toggle_zoom_from_bar(&mut self, pane: &Pane, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let split = self
            .locate(pane)
            .and_then(|(w, t)| self.workspaces[w].tabs.get(t))
            .is_some_and(|tab| matches!(tab.root, PaneNode::Split { .. }));
        if !split {
            return;
        }
        self.toggle_zoom(pane, window, cx);
    }

    /// Focus view for a split pane: it takes most of the tab until toggled again.
    pub(super) fn toggle_zoom(&mut self, pane: &Pane, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.zoomed = if self.zoomed.as_ref() == Some(pane) { None } else { Some(pane.clone()) };
        self.mark_active(pane, cx);
        self.focus_pane(pane, window, cx);
        cx.notify();
    }
}

/// Below this pane width the window is written "(1M)" rather than "(1M context)".
const FULL_MODEL_LABEL_WIDTH: f32 = 700.;
/// Below this pane width the context meter is its figure alone.
const COMPACT_METER_WIDTH: f32 = 520.;

/// A model as the bar shows it: the full name, and the context window when it is known.
struct ModelLabel {
    name: String,
    window: Option<u64>,
}

impl ModelLabel {
    /// "Opus 5.5 (1M context)", or "Opus 5.5 (1M)" where the bar is short of room.
    fn text(&self, compact: bool) -> String {
        match self.window {
            Some(window) if compact => format!("{} ({})", self.name, agentty_bridge::short_tokens(window)),
            Some(window) => format!("{} ({} context)", self.name, agentty_bridge::short_tokens(window)),
            None => self.name.clone(),
        }
    }
}

/// The model a pane runs, from the most direct source there is: what the session wrote down (its
/// answers, a `/model`), else what its welcome banner says, else what the agent is set up to use.
fn pane_model_label(view: &crate::terminal::TerminalView, installed: Option<&crate::agents::Installed>) -> Option<ModelLabel> {
    view.stats
        .as_ref()
        .and_then(model_label)
        .or_else(|| view.banner_model.as_ref().map(|(name, window)| ModelLabel { name: name.clone(), window: Some(*window) }))
        .or_else(|| configured_model_label(installed, view.display_kind()))
}

fn model_label(stats: &agentty_bridge::SessionStats) -> Option<ModelLabel> {
    let name = agentty_bridge::pretty_model(stats.model.as_ref()?);
    Some(ModelLabel { name, window: (stats.context_window > 0).then_some(stats.context_window) })
}

/// The same label for a model nothing has been said to yet.
///
/// A pane's model is read from what the agent has written, so a tab that has just opened has none:
/// the bar used to fall back to the agent's name until the first answer arrived. The model the
/// agent is configured to use is known before that, from its own settings, and that is what it will
/// answer with.
fn configured_model_label(installed: Option<&crate::agents::Installed>, kind: crate::launch::PaneKind) -> Option<ModelLabel> {
    let installed = installed?;
    let model = match kind {
        crate::launch::PaneKind::Claude => installed.claude_models.first().map(|(value, _)| value.clone())?,
        crate::launch::PaneKind::Codex => installed.codex_models.first().cloned()?,
        crate::launch::PaneKind::Shell => return None,
    };
    let bare = model.trim_end_matches("[1m]");
    let name = agentty_bridge::pretty_model(bare);
    // `[1m]` is Claude Code's own way of asking for the long window.
    let window = if model.ends_with("[1m]") { 1_000_000 } else { agentty_bridge::claude_context_window(bare) };
    Some(ModelLabel { name, window: (kind == crate::launch::PaneKind::Claude).then_some(window) })
}

pub(super) fn format_elapsed(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// Branch switcher open under a pane's branch chip.
pub struct BranchMenu {
    pub pane: gpui::EntityId,
    pub repo: std::path::PathBuf,
    pub picker: gpui::Entity<crate::branch_picker::BranchPicker>,
    /// Upstream and ahead/behind counts, loaded when the menu opens.
    status: Option<agentty_bridge::git::RepoStatus>,
    /// A pull or push is running.
    sync: Option<SyncKind>,
    /// Result of the last pull/push: (succeeded, message).
    sync_result: Option<(bool, String)>,
    _subscription: gpui::Subscription,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SyncKind {
    Pull,
    Push,
}

impl Workbench {
    /// `:3000` chips for the local servers this pane started; a click opens them.
    fn port_chips(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ports = self.ports_of(std::iter::once(pane.read(cx).pane_id));
        if ports.is_empty() {
            return None;
        }
        let mut row = div().flex().flex_shrink_0().items_center().gap_1();
        for port in ports.into_iter().take(3) {
            row = row.child(
                div()
                    .id(SharedString::from(format!("hud-port-{}-{port}", pane.entity_id().as_u64())))
                    .px_1p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(hex_alpha(Chrome::GREEN, 0.18))
                    .text_color(hex(Chrome::GREEN))
                    .hover(|s| s.bg(hex_alpha(Chrome::GREEN, 0.3)))
                    .child(format!(":{port}"))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_link(format!("http://localhost:{port}"), cx);
                    })),
            );
        }
        Some(row.into_any_element())
    }

    /// Says that the pane works in a linked git worktree (a session's own copy of the project), and
    /// which; a click shows that tree in the Files panel.
    fn worktree_chip(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = pane.read(cx);
        let name = view.worktree.clone()?;
        let root = agentty_bridge::worktree::tree_root(&view.display_cwd())?;
        Some(
            div()
                .id(("worktree-chip", pane.entity_id().as_u64() as usize))
                .flex()
                .flex_shrink()
                .min_w(px(24.))
                .items_center()
                .gap_1()
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .bg(hex_alpha(Chrome::PURPLE, 0.18))
                .text_color(hex(Chrome::PURPLE))
                .hover(|s| s.bg(hex_alpha(Chrome::PURPLE, 0.3)))
                .tooltip(crate::ui::Tooltip::text(tf(cx, "worktree.chip", &[("name", &name)]), None))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_files_panel(Some(root.clone()), cx);
                }))
                .child(beside_text(icon("git-fork", 11., hex(Chrome::PURPLE))))
                .child(div().min_w_0().max_w(px(140.)).truncate().child(name))
                .into_any_element(),
        )
    }

    /// Clickable branch name; opens a dropdown of local branches for switching.
    fn branch_chip(&self, pane: &Pane, branch: String, cx: &mut Context<Self>) -> AnyElement {
        let id = pane.entity_id();
        let open = self.branch_menu.as_ref().filter(|m| m.pane == id);
        let target = pane.clone();
        let (dirty, ahead) = (pane.read(cx).git_dirty, pane.read(cx).git_ahead);
        let full_name = branch.clone();
        // The whole chip says what the markers say: uncommitted changes first (they come before a
        // push), then commits waiting to be pushed; a clean, pushed branch stays quiet.
        let tint = if dirty {
            Chrome::WARNING
        } else if ahead.is_some_and(|n| n > 0) {
            Chrome::BLUE
        } else {
            Chrome::MUTED
        };
        let sync_bar = open.map(|menu| self.render_branch_sync(menu, &full_name, cx));
        div()
            .relative()
            .flex_shrink_0()
            .child(
                div()
                    .id(("pane-branch", id.as_u64() as usize))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    // No cap: a branch name is what says where the work is going, and
                    // "agentty/claude-0921-2…" says nothing. The folder gives way instead.
                    .flex_shrink_0()
                    .cursor_pointer()
                    .text_color(hex(tint))
                    .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                    .when(open.is_some(), |d| d.bg(hex(Chrome::HOVER)))
                    .child(beside_text(icon("git-branch", IconSize::INLINE, hex(tint))))
                    .child(div().flex_shrink_0().child(branch))
                    // `*` uncommitted changes, `↑N` commits not pushed yet.
                    .when(dirty, |d| d.child(div().flex_shrink_0().text_color(hex(Chrome::WARNING)).child("*")))
                    .when_some(ahead.filter(|n| *n > 0), |d, n| {
                        d.child(div().flex_shrink_0().text_color(hex(Chrome::BLUE)).child(format!("↑{n}")))
                    })
                    .child(icon("chevron-down", 12., hex(Chrome::MUTED)))
                    // Long names are cut off in the chip; the tooltip shows the whole name.
                    .when(open.is_none(), |d| d.tooltip(crate::ui::Tooltip::text(full_name.clone(), Some("⌘ click → web"))))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        // ⌘-click (Ctrl-click on Windows / Linux): the branch on GitHub (or wherever
                        // `origin` lives); a plain click: the menu.
                        if crate::keymap::link_modifier(&event.modifiers()) {
                            this.open_branch_on_web(&target, cx);
                        } else {
                            this.toggle_branch_menu(&target, window, cx);
                        }
                    })),
            )
            .when_some(open, |d, menu| {
                let popover = crate::ui::popover()
                    .w(px(320.))
                    .on_mouse_down_out(cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                        if this.branch_menu.take().is_some() {
                            this.branch_menu_closed = Some((id, std::time::Instant::now()));
                            cx.notify();
                        }
                    }))
                    .children(sync_bar)
                    .child(menu.picker.clone());
                // Anchored right under the chip (over it when the bar sits under the terminal), left edges aligned.
                d.child(bar_popover(crate::ui::fade_in("branch-menu-fade", popover), 2, cx))
            })
            .into_any_element()
    }

    /// Opens the pane's branch in the default browser: `origin`'s web page for it, or the repository's
    /// page while the branch has not been pushed (its page would be a 404). Always the external
    /// browser, whatever links are set to open in: repository hosts need the user's own login.
    fn open_branch_on_web(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        let (repo, branch, pushed) = {
            let view = pane.read(cx);
            (view.display_cwd(), view.git_branch.clone(), view.git_ahead.is_some())
        };
        let Some(branch) = branch else { return };
        let task = cx.background_spawn(async move { agentty_bridge::git::remote_web_url(&repo) });
        cx.spawn(async move |this, cx| {
            let remote = task.await;
            let _ = this.update(cx, |this, cx| match remote {
                None => this.set_status(t(cx, "branch.no_remote").to_string(), cx),
                Some(repo_url) if pushed => cx.open_url(&agentty_bridge::git::branch_web_url(&repo_url, &branch)),
                Some(repo_url) => {
                    this.set_status(tf(cx, "branch.not_pushed", &[("branch", &branch)]), cx);
                    cx.open_url(&repo_url);
                }
            });
        })
        .detach();
    }

    pub(super) fn toggle_branch_menu(&mut self, pane: &Pane, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let id = pane.entity_id();
        // The outside-click handler already closed it on mouse down; don't reopen on the same click.
        if self.branch_menu_closed.take().is_some_and(|(pane, at)| pane == id && at.elapsed().as_millis() < 400) {
            return;
        }
        if self.branch_menu.take().is_some_and(|m| m.pane == id) {
            return cx.notify();
        }
        let cwd = pane.read(cx).display_cwd();
        let Some(repo) = agentty_bridge::git::repo_root(&cwd) else { return };
        let current = pane.read(cx).git_branch.clone();
        let picker = cx.new(|cx| crate::branch_picker::BranchPicker::new(repo.clone(), current, window, cx));
        let subscription = cx.subscribe(&picker, move |this, _, event: &crate::branch_picker::BranchPickerEvent, cx| {
            use crate::branch_picker::BranchPickerEvent;
            let Some(repo) = this.branch_menu.as_ref().map(|m| m.repo.clone()) else { return };
            match event {
                BranchPickerEvent::Pick { name, remote } => this.switch_branch(repo, name.clone(), *remote, cx),
                BranchPickerEvent::Create(name) => this.create_branch(repo, name.clone(), cx),
                BranchPickerEvent::Dismiss => {
                    this.branch_menu = None;
                    cx.notify();
                }
            }
        });
        self.branch_menu =
            Some(BranchMenu { pane: id, repo, picker, status: None, sync: None, sync_result: None, _subscription: subscription });
        self.load_branch_status(cx);
        cx.notify();
    }

    fn load_branch_status(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.branch_menu.as_ref().map(|m| m.repo.clone()) else { return };
        let task = cx.background_spawn(async move { agentty_bridge::git::status(&repo).ok() });
        cx.spawn(async move |this, cx| {
            let status = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(menu) = this.branch_menu.as_mut() {
                    menu.status = status;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// `git pull --ff-only` / `git push` for the branch menu's repository (commits stay on the Git page).
    fn sync_branch(&mut self, kind: SyncKind, cx: &mut Context<Self>) {
        let Some(menu) = self.branch_menu.as_mut() else { return };
        if menu.sync.is_some() {
            return;
        }
        menu.sync = Some(kind);
        menu.sync_result = None;
        let repo = menu.repo.clone();
        let (branch, has_upstream) = menu.status.as_ref().map(|s| (s.branch.clone(), s.upstream.is_some())).unwrap_or((None, false));
        cx.notify();
        let task = cx.background_spawn(async move {
            match kind {
                SyncKind::Pull => agentty_bridge::git::pull(&repo).map(Some),
                SyncKind::Push => match branch {
                    Some(branch) => agentty_bridge::git::push(&repo, &branch, has_upstream).map(|()| None),
                    None => Err(anyhow::anyhow!("detached HEAD")),
                },
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let message = match (&result, kind) {
                    // "3 commits · 12 files (+4 ~7 -1)": say what arrived, so "done" is checkable.
                    (Ok(Some(pulled)), _) if pulled.files() > 0 => tf(
                        cx,
                        "branch.pulled_changes",
                        &[
                            ("commits", &pulled.commits.to_string()),
                            ("files", &pulled.files().to_string()),
                            ("added", &pulled.added.to_string()),
                            ("modified", &pulled.modified.to_string()),
                            ("deleted", &pulled.deleted.to_string()),
                        ],
                    ),
                    (Ok(_), SyncKind::Pull) => t(cx, "branch.pulled").to_string(),
                    (Ok(_), SyncKind::Push) => t(cx, "branch.pushed").to_string(),
                    (Err(err), _) => agentty_bridge::git::failure_reason(&err.to_string()),
                };
                let repo = this.branch_menu.as_ref().map(|m| m.repo.clone());
                if let Some(menu) = this.branch_menu.as_mut() {
                    menu.sync = None;
                    menu.sync_result = Some((result.is_ok(), message.clone()));
                } else {
                    this.set_status(message, cx);
                }
                // Panes in this repository show the new ahead/behind state right away.
                if let Some(repo) = repo {
                    for pane in this.all_panes() {
                        if agentty_bridge::git::repo_root(&pane.read(cx).display_cwd()).as_deref() == Some(repo.as_path()) {
                            pane.update(cx, |view, cx| view.probe_git(cx));
                        }
                    }
                }
                this.load_branch_status(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Top of the branch menu: the full branch name with copy, and pull / push.
    fn render_branch_sync(&self, menu: &BranchMenu, branch: &str, cx: &mut Context<Self>) -> AnyElement {
        let status = menu.status.as_ref();
        let (ahead, behind) = status.map(|s| (s.ahead, s.behind)).unwrap_or((0, 0));
        let has_upstream = status.is_some_and(|s| s.upstream.is_some());
        let copy_name = branch.to_string();
        let button = |id: &'static str, glyph: &'static str, label: String, running: bool, enabled: bool| {
            div()
                .id(id)
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap_1()
                .py_1()
                .rounded_md()
                .t_small()
                .bg(hex(0x2d2d30))
                .text_color(hex(if enabled { Chrome::BRIGHT } else { Chrome::MUTED }))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(hex(Chrome::ACCENT))))
                .child(if running {
                    crate::ui::spinner(IconSize::INLINE, hex(Chrome::BRIGHT)).into_any_element()
                } else {
                    icon(glyph, IconSize::INLINE, hex(if enabled { Chrome::BRIGHT } else { Chrome::MUTED })).into_any_element()
                })
                .child(label)
        };
        let busy = menu.sync.is_some();
        let pull_label = if behind > 0 { format!("{} ↓{behind}", t(cx, "branch.pull")) } else { t(cx, "branch.pull").to_string() };
        let push_label = if ahead > 0 { format!("{} ↑{ahead}", t(cx, "branch.push")) } else { t(cx, "branch.push").to_string() };
        // Pushing needs something to push, or a branch that is not on the remote yet.
        let can_push = status.is_some() && (ahead > 0 || !has_upstream) && !busy;
        let can_pull = has_upstream && !busy;
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .px_2()
            .pt_2()
            .pb_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(icon("git-branch", IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .t_small()
                            .font_family(crate::settings::BUNDLED_FONT)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(branch.to_string()),
                    )
                    .child(
                        crate::ui::icon_only(
                            "branch-copy",
                            "copy",
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_name.clone()));
                                this.set_status(t(cx, "branch.copied"), cx);
                            }),
                        )
                        .tooltip(crate::ui::Tooltip::text(t(cx, "branch.copy"), None)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        button("branch-pull", "arrow-down", pull_label, menu.sync == Some(SyncKind::Pull), can_pull)
                            .when(can_pull, |d| d.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.sync_branch(SyncKind::Pull, cx))))
                            .tooltip(crate::ui::Tooltip::text(t(cx, "branch.pull_hint"), None)),
                    )
                    .child(
                        button("branch-push", "arrow-up", push_label, menu.sync == Some(SyncKind::Push), can_push)
                            .when(can_push, |d| d.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.sync_branch(SyncKind::Push, cx))))
                            .tooltip(crate::ui::Tooltip::text(
                                if has_upstream { t(cx, "branch.push_hint") } else { t(cx, "branch.publish_hint") },
                                None,
                            )),
                    ),
            )
            .children(
                menu.sync_result.clone().map(|(ok, message)| {
                    div().t_caption().text_color(hex(if ok { Chrome::SUCCESS } else { Chrome::ERROR })).child(message)
                }),
            )
            .child(div().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .into_any_element()
    }

    pub(super) fn create_branch(&mut self, repo: std::path::PathBuf, name: String, cx: &mut Context<Self>) {
        self.branch_menu = None;
        cx.notify();
        let task = cx.background_spawn(async move { agentty_bridge::git::create_branch(&repo, &name).map(|_| (repo, name)) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok((repo, branch)) => this.show_branch(&repo, branch, cx),
                Err(err) => this.set_status(format!("git: {err}"), cx),
            });
        })
        .detach();
    }

    /// Every pane in `repo` shows `branch` right away.
    fn show_branch(&mut self, repo: &std::path::Path, branch: String, cx: &mut Context<Self>) {
        for pane in self.all_panes() {
            if agentty_bridge::git::repo_root(&pane.read(cx).display_cwd()).as_deref() == Some(repo) {
                pane.update(cx, |v, cx| {
                    v.git_branch = Some(branch.clone());
                    cx.notify();
                });
            }
        }
        cx.notify();
    }

    pub(super) fn switch_branch(&mut self, repo: std::path::PathBuf, branch: String, remote: bool, cx: &mut Context<Self>) {
        let Some(menu) = self.branch_menu.take() else { return };
        cx.notify();
        let current = self.all_panes().into_iter().find(|p| p.entity_id() == menu.pane).and_then(|p| p.read(cx).git_branch.clone());
        if !remote && current.as_ref() == Some(&branch) {
            return;
        }
        let task = cx.background_spawn(async move {
            agentty_bridge::git::checkout(&repo, &branch, remote)?;
            Ok::<_, anyhow::Error>(crate::branch_picker::branch_label(&branch, remote).to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(branch) => this.show_branch(&menu.repo, branch, cx),
                    Err(err) => this.set_status(format!("git: {err}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Workbench {
    /// Always shows the model, context and branch; usage, elapsed time and the folder fold away
    /// as the pane narrows.
    fn render_agent_bar(&self, pane: &Pane, width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mut chip = pane.read(cx).git_branch.clone().map(|b| self.branch_chip(pane, b, cx));
        let mut tree_chip = self.worktree_chip(pane, cx);
        let mut collab = self.render_collab_chips(pane, cx);
        let mut port_chips = self.port_chips(pane, cx);
        let hud = crate::hud::normalized(&crate::settings::settings(cx).hud);
        let percent = pane.read(cx).stats.as_ref().and_then(|s| s.context_percent());
        let mut context_meter = Some(self.context_meter(pane, percent, false, cx));
        let view = pane.read(cx);
        let kind = view.agent_kind()?;
        let (status, status_color) = status_label(view, cx);
        let name = crate::brand::brand(crate::brand::kind_id(kind)).name;
        let working = view.working_since.is_some();
        Some(
            div()
                .h(px(28.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .overflow_hidden()
                .bg(hex(Chrome::TAB_INACTIVE))
                .map(|d| if bars_below(cx) { d.border_t_1() } else { d.border_b_1() })
                .border_color(hex(Chrome::BORDER))
                .t_small()
                // The items of the status bar, in the user's order (Settings → Appearance).
                .map(|mut d| {
                    for entry in hud.iter().filter(|e| e.visible) {
                        d = match entry.item {
                            // The model, without the logo: the tab strip and the card already say
                            // which agent this is, and the bar needs the room.
                            HudItem::Model => d.child(
                                div().flex_shrink_0().text_color(hex(Chrome::BRIGHT)).child(
                                    pane_model_label(view, self.installed.as_ref())
                                        .map(|model| model.text(false))
                                        .unwrap_or_else(|| name.to_string()),
                                ),
                            ),
                            HudItem::Context => d.children(context_meter.take()),
                            HudItem::Usage => d
                                .when_some(view.usage_percent().filter(|p| *p >= USAGE_SHOWN_AT && width >= 700.), |d, percent| {
                                    d.child(meter("Usage", percent))
                                }),
                            // Same double-click as the split header's status. This bar belongs to
                            // a pane that already fills its tab, so it only has something to do
                            // once the tab is split.
                            HudItem::Status => d.child(
                                div()
                                    .id(("agent-bar-status", pane.entity_id().as_u64() as usize))
                                    .flex()
                                    .flex_shrink()
                                    .min_w(px(24.))
                                    .items_center()
                                    .gap_1()
                                    .px_1p5()
                                    .rounded_sm()
                                    .bg(hex_alpha(status_color, 0.15))
                                    .on_click({
                                        let target = pane.clone();
                                        cx.listener(move |this, event: &ClickEvent, window, cx| {
                                            if event.click_count() < 2 {
                                                return;
                                            }
                                            cx.stop_propagation();
                                            this.toggle_zoom_from_bar(&target, window, cx);
                                        })
                                    })
                                    .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(status_color)))
                                    .child(div().min_w_0().max_w(px(320.)).truncate().text_color(hex(status_color)).child(status.clone())),
                            ),
                            HudItem::Elapsed => d.when_some(
                                view.working_since.map(|t| format_elapsed(t.elapsed().as_secs())).filter(|_| working && width >= 560.),
                                |d, elapsed| d.child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(elapsed)),
                            ),
                            HudItem::Links => d.children(collab.take()),
                            HudItem::Spacer => d.child(div().flex_1()),
                            HudItem::Ports => d.children(port_chips.take()),
                            HudItem::Worktree => d.children(tree_chip.take()),
                            HudItem::Branch => d.children(chip.take()),
                            // The folder is in the app's status bar (bottom left); this bar has
                            // the model, the branch and the status to fit.
                            HudItem::Folder => d,
                        };
                    }
                    d
                })
                .into_any_element(),
        )
    }
}

/// Whether panes show their bar under the terminal instead of above it.
pub(super) fn bars_below(cx: &gpui::App) -> bool {
    crate::settings::settings(cx).agent_bar_position == crate::hud::HudPosition::Bottom
}

/// A popover that belongs to a chip of a pane's bar: it opens away from the bar, under the chip
/// when the bar is above the terminal and over the chip when the bar is under it.
pub(super) fn bar_popover(popover: impl IntoElement, priority: usize, cx: &gpui::App) -> gpui::Div {
    let below = bars_below(cx);
    let anchored = gpui::anchored()
        .anchor(if below { gpui::Corner::BottomLeft } else { gpui::Corner::TopLeft })
        .snap_to_window_with_margin(px(8.))
        .child(div().map(|d| if below { d.mb_1() } else { d.mt_1() }).child(popover));
    div()
        .absolute()
        .left_0()
        .map(|d| if below { d.bottom_full() } else { d.top_full() })
        .child(gpui::deferred(anchored).with_priority(priority))
}

/// Tooltip hint of a folder chip: ⌘-click (Ctrl-click on Windows / Linux) shows the folder.
pub(super) const REVEAL_HINT: &str = if cfg!(target_os = "macos") { "⌘ click → Finder" } else { "⌘ click → folder" };

/// An icon that sits next to small text in a status row. The row centers both boxes, but the letters
/// sit low in their line box (measured: the middle of the text is ~1.5 pt under the middle of the
/// row), so an icon centered by the box reads as floating above its label. Lowered to the letters.
fn beside_text(icon: gpui::Svg) -> gpui::Svg {
    icon.relative().top(px(1.5))
}

fn small_icon_button(
    id: impl Into<gpui::ElementId>,
    name: &'static str,
    on_click: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .hover(|s| s.bg(hex(Chrome::HOVER)))
        .on_click(on_click)
        .child(icon(name, IconSize::INLINE, hex(Chrome::FOREGROUND)))
}

/// Label + slim progress bar + percentage, colored by how full it is.
impl Workbench {
    /// Context meter for an agent pane's bar. Past [`super::COMPACT_OFFER_AT`] it also offers the
    /// agent's own compaction command, typed in rather than sent: compacting is the user's call.
    /// The Context meter. Clicking it opens what makes up that number — memory files, skills, the
    /// files in context — anchored right under the figure it explains, rather than from an icon
    /// somewhere else in the window. A nearly full context also offers to compact it, and the
    /// panel it opens carries that button whatever the number says.
    fn context_meter(&self, pane: &Pane, percent: Option<f64>, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let view = pane.read(cx);
        let agent = view.agent_kind().and_then(crate::launch::PaneKind::agent);
        // A tab that has just opened has read nothing yet, and nothing is exactly 0%. Showing a
        // dash until the first answer made a new tab look like it was missing something.
        let percent = percent.or_else(|| agent.is_some().then_some(0.));
        let command =
            view.agent_kind().and_then(|kind| kind.compact_command()).filter(|_| percent.is_some_and(|p| p >= super::COMPACT_OFFER_AT));
        let open = self.status_menu == Some(super::status_menus::StatusMenu::Context) && self.menu_pane_is(pane);
        let mut chip = optional_meter("Context", percent, compact).id(("context-meter", pane.entity_id().as_u64() as usize)).relative();
        if let Some(command) = command {
            let target = pane.clone();
            chip = chip.child(
                div()
                    .id(("context-compact", pane.entity_id().as_u64() as usize))
                    .flex_shrink_0()
                    .p_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(hex(Chrome::ORANGE))
                    .hover(|s| s.bg(hex_alpha(Chrome::ORANGE, 0.16)))
                    .tooltip(crate::ui::Tooltip::text(crate::i18n::tf(cx, "context.compact_hint", &[("command", command)]), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.focus_pane(&target, window, cx);
                        target.update(cx, |view, _| view.insert_text(command));
                    }))
                    .child(icon("package", 11., hex(Chrome::ORANGE))),
            );
        }
        let Some(agent) = agent else { return chip.into_any_element() };
        // Room around the figure, so the hover and open states read as a chip rather than a box
        // squeezed against the text — the same padding the status bar's own chips use.
        chip.px_1p5()
            .py_0p5()
            .rounded_md()
            .cursor_pointer()
            .when(open, |d| d.bg(hex(Chrome::SELECTED)))
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .on_click({
                let pane = pane.clone();
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle_pane_menu(super::status_menus::StatusMenu::Context, pane.clone(), cx)
                })
            })
            .when(open, |d| d.child(bar_popover(self.render_context_menu(pane, agent, cx), 6, cx)))
            .into_any_element()
    }
}

/// Plan usage only earns a place in the bar once it is worth watching; under this it is left to
/// the menu bar popover and the Usage page.
const USAGE_SHOWN_AT: f64 = 50.0;

fn meter(label: &'static str, percent: f64) -> gpui::Div {
    optional_meter(label, Some(percent), false)
}

/// The same meter for a figure that is not in yet: it keeps its place and stays clickable, so what
/// it opens does not come and go with the first reading.
/// `compact`: the figure alone, without the word and the bar.
fn optional_meter(label: &'static str, percent: Option<f64>, compact: bool) -> gpui::Div {
    let color = match percent {
        Some(p) if p >= 90.0 => Chrome::ERROR,
        Some(p) if p >= 70.0 => Chrome::ORANGE,
        Some(_) => Chrome::SUCCESS,
        None => Chrome::MUTED,
    };
    let fraction = (percent.unwrap_or(0.) / 100.0).clamp(0.0, 1.0) as f32;
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .flex_shrink_0()
        .when(!compact, |d| d.child(div().text_color(hex(Chrome::MUTED)).child(label)))
        .when(!compact, |d| {
            d.child(
                div()
                    .w(px(56.))
                    .h(px(5.))
                    .rounded_full()
                    .bg(hex(0x3a3a3a))
                    .when_some(percent, |d, _| d.child(div().h_full().rounded_full().bg(hex(color)).w(gpui::relative(fraction.max(0.02))))),
            )
        })
        .child(div().text_color(hex(color)).child(match percent {
            Some(p) => format!("{p:.0}%"),
            None => "—".to_string(),
        }))
}

#[cfg(test)]
mod tests {
    use super::format_elapsed;

    #[test]
    fn elapsed_formatting() {
        assert_eq!(format_elapsed(7), "7s");
        assert_eq!(format_elapsed(125), "2m 05s");
        assert_eq!(format_elapsed(3720), "1h 02m");
    }
}
