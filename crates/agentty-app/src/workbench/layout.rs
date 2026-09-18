//! Renders a tab's split tree: pane headers, attention borders and draggable dividers.

use super::panes::{zoom_sizes, Axis, PaneNode};
use super::{status_label, Pane, Tab, Workbench};
use crate::i18n::t;
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
        let chip = pane.read(cx).git_branch.clone().filter(|_| split).map(|b| self.branch_chip(pane, b, cx));
        let collab = split.then(|| self.render_collab_chips(pane, cx)).flatten();
        let plugin_buttons = split.then(|| self.render_plugin_pane_buttons(pane, cx)).flatten();
        let view = pane.read(cx);
        let prefs_bar = crate::settings::settings(cx).agent_bar;
        let width = self.pane_bounds.borrow().get(&pane.entity_id()).map(|b| f32::from(b.size.width)).unwrap_or(f32::MAX);
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
        let header = split.then(|| {
            let (zoom, close) = (pane.clone(), pane.clone());
            let zoomed = self.zoomed.as_ref() == Some(pane);
            let group = SharedString::from(format!("pane-{}", pane.entity_id().as_u64()));
            let cwd = view.display_cwd();
            let folder = cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| crate::ui::tilde(&cwd));
            let agent_info = view.is_agent() && prefs_bar;
            div()
                .id(("pane-header", pane.entity_id().as_u64() as usize))
                .group(group.clone())
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
                        .tooltip(crate::ui::Tooltip::text(crate::i18n::t(cx, "pane.move_hint"), None))
                        .on_drag(
                            super::drop_split::DraggedPane { pane_id: view.pane_id, title: view.display_title().into() },
                            |dragged, _, _, cx| {
                                super::drop_split::note_pane_drag(dragged.pane_id);
                                cx.new(|_| super::chrome::DragPreview { title: dragged.title.clone() })
                            },
                        )
                        .child(crate::brand::avatar(view.tool_id(), 16.)),
                )
                .when_some(view.stats.as_ref().filter(|_| agent_info).and_then(model_label), |d, model| {
                    d.child(div().flex_shrink().min_w(px(40.)).truncate().text_color(hex(Chrome::BRIGHT)).child(model))
                })
                .when_some(view.stats.as_ref().filter(|_| agent_info).and_then(|s| s.context_percent()), |d, percent| {
                    d.child(meter("Context", percent))
                })
                .when(agent_info, |d| {
                    d.child(div().flex_shrink().min_w(px(40.)).max_w(px(220.)).truncate().text_color(hex(status_color)).child(status))
                })
                .when_some(
                    view.working_since.filter(|_| agent_info && width >= 560.).map(|t| format_elapsed(t.elapsed().as_secs())),
                    |d, elapsed| d.child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(elapsed)),
                )
                .children(collab)
                .child(div().flex_1())
                .children(plugin_buttons)
                // Where this pane is: project folder, with the full path when there is room.
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .min_w_0()
                        .flex_shrink()
                        .child(icon("folder", 12., hex(Chrome::MUTED)))
                        .child(
                            div()
                                .flex_shrink_0()
                                .max_w(px(160.))
                                .truncate()
                                .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                                .child(folder),
                        )
                        .when(width >= 620., |d| {
                            d.child(div().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(crate::ui::tilde(&cwd)))
                        }),
                )
                .children(chip)
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .when(!active && !zoomed, |d| d.invisible().group_hover(group, |s| s.visible()))
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
        let agent_bar = (!split && prefs_bar).then(|| self.render_agent_bar(pane, width, cx)).flatten();
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
            .children(header)
            .children(agent_bar)
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
            .into_any_element()
    }

    /// Focus view for a split pane: it takes most of the tab until toggled again.
    pub(super) fn toggle_zoom(&mut self, pane: &Pane, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.zoomed = if self.zoomed.as_ref() == Some(pane) { None } else { Some(pane.clone()) };
        self.mark_active(pane, cx);
        self.focus_pane(pane, window, cx);
        cx.notify();
    }
}

/// "Opus 5 (1M context)".
fn model_label(stats: &agentty_bridge::SessionStats) -> Option<String> {
    let model = agentty_bridge::pretty_model(stats.model.as_ref()?);
    Some(if stats.context_window > 0 { format!("{model} ({} context)", agentty_bridge::short_tokens(stats.context_window)) } else { model })
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
    /// Clickable branch name; opens a dropdown of local branches for switching.
    fn branch_chip(&self, pane: &Pane, branch: String, cx: &mut Context<Self>) -> AnyElement {
        let id = pane.entity_id();
        let open = self.branch_menu.as_ref().filter(|m| m.pane == id);
        let target = pane.clone();
        let (dirty, ahead) = (pane.read(cx).git_dirty, pane.read(cx).git_ahead);
        let full_name = branch.clone();
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
                    .max_w(px(180.))
                    .cursor_pointer()
                    .text_color(hex(Chrome::MUTED))
                    .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                    .when(open.is_some(), |d| d.bg(hex(Chrome::HOVER)))
                    .child(icon("git-branch", IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(div().truncate().child(branch))
                    // `*` uncommitted changes, `↑N` commits not pushed yet.
                    .when(dirty, |d| d.child(div().flex_shrink_0().text_color(hex(Chrome::WARNING)).child("*")))
                    .when_some(ahead.filter(|n| *n > 0), |d, n| {
                        d.child(div().flex_shrink_0().text_color(hex(Chrome::BLUE)).child(format!("↑{n}")))
                    })
                    .child(icon("chevron-down", 12., hex(Chrome::MUTED)))
                    // Long names are cut off in the chip; the tooltip shows the whole name.
                    .when(open.is_none(), |d| d.tooltip(crate::ui::Tooltip::text(full_name.clone(), None)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.toggle_branch_menu(&target, window, cx);
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
                // Anchored right under the chip, left edges aligned.
                d.child(
                    div().absolute().top_full().left_0().child(
                        gpui::deferred(
                            gpui::anchored()
                                .snap_to_window_with_margin(px(8.))
                                .child(div().mt_1().child(crate::ui::fade_in("branch-menu-fade", popover))),
                        )
                        .with_priority(2),
                    ),
                )
            })
            .into_any_element()
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
                SyncKind::Pull => agentty_bridge::git::pull(&repo),
                SyncKind::Push => match branch {
                    Some(branch) => agentty_bridge::git::push(&repo, &branch, has_upstream),
                    None => Err(anyhow::anyhow!("detached HEAD")),
                },
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let message = match (&result, kind) {
                    (Ok(()), SyncKind::Pull) => t(cx, "branch.pulled").to_string(),
                    (Ok(()), SyncKind::Push) => t(cx, "branch.pushed").to_string(),
                    (Err(err), _) => err.to_string().lines().last().unwrap_or_default().to_string(),
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
        let chip = pane.read(cx).git_branch.clone().map(|b| self.branch_chip(pane, b, cx));
        let collab = self.render_collab_chips(pane, cx);
        let plugin_buttons = self.render_plugin_pane_buttons(pane, cx);
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
                .border_b_1()
                .border_color(hex(Chrome::BORDER))
                .t_small()
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1p5()
                        .child(crate::brand::avatar(crate::brand::kind_id(kind), 16.))
                        // "[Opus 5 (1M context)]": the model says more than the agent name.
                        .child(
                            div()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(view.stats.as_ref().and_then(model_label).unwrap_or_else(|| name.to_string())),
                        ),
                )
                .when_some(view.stats.as_ref().and_then(|s| s.context_percent()), |d, percent| d.child(meter("Context", percent)))
                .when_some(view.usage_percent().filter(|_| width >= 700.), |d, percent| {
                    d.child(div().w(px(1.)).h(px(12.)).bg(hex(Chrome::BORDER))).child(meter("Usage", percent))
                })
                .child(
                    div()
                        .flex()
                        .flex_shrink()
                        .min_w(px(24.))
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded_sm()
                        .bg(hex_alpha(status_color, 0.15))
                        .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(status_color)))
                        .child(div().min_w_0().max_w(px(320.)).truncate().text_color(hex(status_color)).child(status)),
                )
                .when_some(
                    view.working_since.map(|t| format_elapsed(t.elapsed().as_secs())).filter(|_| working && width >= 560.),
                    |d, elapsed| d.child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(elapsed)),
                )
                .children(collab)
                .child(div().flex_1())
                .children(plugin_buttons)
                .children(chip)
                .when(width >= 820., |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .flex_shrink()
                            .text_color(hex(Chrome::MUTED))
                            .child(icon("folder", IconSize::INLINE, hex(Chrome::MUTED)))
                            .child(div().truncate().child(crate::ui::tilde(&view.display_cwd()))),
                    )
                })
                .into_any_element(),
        )
    }
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
fn meter(label: &'static str, percent: f64) -> gpui::Div {
    let color = match percent {
        p if p >= 90.0 => Chrome::ERROR,
        p if p >= 70.0 => Chrome::ORANGE,
        _ => Chrome::SUCCESS,
    };
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .flex_shrink_0()
        .child(div().text_color(hex(Chrome::MUTED)).child(label))
        .child(
            div()
                .w(px(56.))
                .h(px(5.))
                .rounded_full()
                .bg(hex(0x3a3a3a))
                .child(div().h_full().rounded_full().bg(hex(color)).w(gpui::relative(fraction.max(0.02)))),
        )
        .child(div().text_color(hex(color)).child(format!("{percent:.0}%")))
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
