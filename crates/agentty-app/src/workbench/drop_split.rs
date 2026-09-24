//! Moving panes and tabs by dragging: a tab dropped into a split joins it, a split pane dragged by
//! its header moves within (or between) tabs. Also the "pick a pane" mode of session links.

use super::chrome::DraggedTab;
use super::panes::Axis;
use super::{Pane, Tab, Workbench};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, TypeScale};
use gpui::{div, prelude::*, px, relative, AnyElement, ClickEvent, Context, SharedString, Window};

/// A split pane dragged by its header, for rearranging a tab's layout.
#[derive(Clone)]
pub struct DraggedPane {
    pub pane_id: u64,
    pub title: SharedString,
}

/// The tab being dragged right now, as (workspace id, tab index). GPUI only says that *some* drag
/// is active, and dragging a workspace row or a group over the terminals must not offer splits.
static TAB_DRAG: std::sync::Mutex<Option<(u64, usize)>> = std::sync::Mutex::new(None);
/// The pane being dragged by its header.
static PANE_DRAG: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

pub(super) fn note_pane_drag(pane_id: u64) {
    if let Ok(mut dragged) = PANE_DRAG.lock() {
        *dragged = Some(pane_id);
    }
}

pub(super) fn dragged_pane() -> Option<u64> {
    PANE_DRAG.lock().ok().and_then(|dragged| *dragged)
}

pub(super) fn note_tab_drag(workspace: u64, index: usize) {
    if let Ok(mut dragged) = TAB_DRAG.lock() {
        *dragged = Some((workspace, index));
    }
}

/// Every drag ends on mouse-up, wherever it was released.
pub(super) fn end_tab_drag() {
    if let Ok(mut dragged) = TAB_DRAG.lock() {
        *dragged = None;
    }
    if let Ok(mut dragged) = PANE_DRAG.lock() {
        *dragged = None;
    }
}

pub(super) fn dragged_tab() -> Option<(u64, usize)> {
    TAB_DRAG.lock().ok().and_then(|dragged| *dragged)
}

/// Which side of a pane a moved tab lands on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DropSide {
    Left,
    Right,
    Top,
    Bottom,
}

impl DropSide {
    fn axis(self) -> Axis {
        match self {
            DropSide::Left | DropSide::Right => Axis::Horizontal,
            DropSide::Top | DropSide::Bottom => Axis::Vertical,
        }
    }

    /// Whether the dropped tab goes before the pane it was dropped on.
    fn before(self) -> bool {
        matches!(self, DropSide::Left | DropSide::Top)
    }
}

impl Workbench {
    /// Moves a tab out of its workspace and into the tab holding `target`, as a split beside it.
    pub(super) fn move_tab_into_pane(
        &mut self,
        workspace: u64,
        index: usize,
        target: &Pane,
        side: DropSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.workspaces.iter().position(|w| w.id == workspace) else { return };
        let Some((target_workspace, target_tab)) = self.locate(target) else { return };
        // Dropping a tab on one of its own panes would move it into itself.
        if source == target_workspace && index == target_tab {
            return;
        }
        let Some(tab) = self.take_tab(source, index) else { return };
        // Removing the tab may have dropped its workspace, so find the target again.
        let Some((target_workspace, target_tab)) = self.locate(target) else {
            // The target went away while we held the tab: put it back as its own tab.
            self.restore_tab(tab, window, cx);
            return;
        };
        let moved = tab.active.clone();
        let ws = &mut self.workspaces[target_workspace];
        if !ws.tabs[target_tab].root.attach(target, tab.root, side.axis(), side.before()) {
            self.restore_tab(Tab { root: super::panes::PaneNode::Leaf(moved.clone()), active: moved, instance: None }, window, cx);
            return;
        }
        ws.active_tab = target_tab;
        ws.tabs[target_tab].active = moved.clone();
        self.activate_workspace(target_workspace, window, cx);
        self.mark_active(&moved, cx);
        self.focus_pane(&moved, window, cx);
        self.persist(cx);
        cx.notify();
    }

    /// Removes tab `index` from workspace `source`, keeping the workspace usable (or dropping it).
    fn take_tab(&mut self, source: usize, index: usize) -> Option<Tab> {
        let ws = self.workspaces.get_mut(source)?;
        if index >= ws.tabs.len() {
            return None;
        }
        let tab = ws.tabs.remove(index);
        if ws.active_tab >= ws.tabs.len() {
            ws.active_tab = ws.tabs.len().saturating_sub(1);
        } else if index < ws.active_tab {
            ws.active_tab -= 1;
        }
        if ws.tabs.is_empty() && ws.dormant.is_none() {
            self.workspaces.remove(source);
            if self.active_workspace >= self.workspaces.len() {
                self.active_workspace = self.workspaces.len().saturating_sub(1);
            } else if source < self.active_workspace {
                self.active_workspace -= 1;
            }
        }
        Some(tab)
    }

    /// Puts a tab back when the move could not be finished.
    fn restore_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.wake_for_new_tab(self.active_workspace, cx);
        match self.workspaces.get_mut(self.active_workspace) {
            Some(ws) => {
                ws.tabs.push(tab);
                ws.active_tab = ws.tabs.len() - 1;
            }
            None => {
                let cwd = tab.active.read(cx).display_cwd();
                let id = self.next_id();
                self.workspaces.push(super::Workspace {
                    id,
                    name: None,
                    group: None,
                    cwd,
                    tabs: vec![tab],
                    active_tab: 0,
                    dormant: None,
                    asleep_on_close: false,
                    closed_tabs: Vec::new(),
                    color: None,
                    plugin: None,
                });
                self.active_workspace = self.workspaces.len() - 1;
            }
        }
        self.focus_active(window, cx);
        self.persist(cx);
        cx.notify();
    }

    /// Shows a tab while a dragged tab hovers it, so its splits become the drop target.
    pub(super) fn preview_tab_during_drag(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let already = self.workspaces.get(self.active_workspace).is_some_and(|ws| ws.active_tab == index);
        if already || dragged_tab().is_none() {
            return;
        }
        let Some(ws) = self.workspaces.get_mut(self.active_workspace) else { return };
        if index >= ws.tabs.len() {
            return;
        }
        ws.active_tab = index;
        self.page = None;
        // Focus stays with the drag; only what is shown changes.
        let _ = window;
        cx.notify();
    }

    /// Whether `pane` can take what is being dragged: a tab from elsewhere, or another pane.
    pub(super) fn drop_target(&self, pane: &Pane, cx: &gpui::App) -> bool {
        if let Some(dragged) = dragged_pane() {
            // A pane can't be dropped onto itself, and a lone pane has nowhere else to go.
            return dragged != pane.read(cx).pane_id && self.all_panes().len() > 1;
        }
        match dragged_tab() {
            Some((workspace, index)) => {
                self.workspaces.get(self.active_workspace).is_some_and(|ws| ws.id != workspace || ws.active_tab != index)
            }
            None => false,
        }
    }

    /// Moves a split pane next to another one (the same tab, or another tab's layout).
    pub(super) fn move_pane_beside(&mut self, pane_id: u64, target: &Pane, side: DropSide, window: &mut Window, cx: &mut Context<Self>) {
        let Some(moved) = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane_id) else { return };
        if moved == *target {
            return;
        }
        let Some((source_workspace, source_tab)) = self.locate(&moved) else { return };
        {
            let ws = &mut self.workspaces[source_workspace];
            let tab = &mut ws.tabs[source_tab];
            // `remove` consumes the tree; the placeholder is dropped right after.
            let root = std::mem::replace(&mut tab.root, super::panes::PaneNode::Leaf(moved.clone()));
            match root.remove(&moved) {
                Some(root) => {
                    let active = if tab.active == moved { root.leaves()[0].clone() } else { tab.active.clone() };
                    tab.root = root;
                    tab.active = active;
                }
                // It was the tab's only pane: the tab (and an empty workspace) goes away with it.
                None => {
                    ws.tabs.remove(source_tab);
                    if ws.active_tab >= ws.tabs.len() {
                        ws.active_tab = ws.tabs.len().saturating_sub(1);
                    } else if source_tab < ws.active_tab {
                        ws.active_tab -= 1;
                    }
                    if ws.tabs.is_empty() && ws.dormant.is_none() {
                        self.workspaces.remove(source_workspace);
                        if self.active_workspace >= self.workspaces.len() {
                            self.active_workspace = self.workspaces.len().saturating_sub(1);
                        } else if source_workspace < self.active_workspace {
                            self.active_workspace -= 1;
                        }
                    }
                }
            }
        }
        // Indices may have shifted while the pane was taken out.
        let Some((target_workspace, target_tab)) = self.locate(target) else { return };
        let ws = &mut self.workspaces[target_workspace];
        if !ws.tabs[target_tab].root.attach(target, super::panes::PaneNode::Leaf(moved.clone()), side.axis(), side.before()) {
            return;
        }
        ws.active_tab = target_tab;
        ws.tabs[target_tab].active = moved.clone();
        self.activate_workspace(target_workspace, window, cx);
        self.mark_active(&moved, cx);
        self.focus_pane(&moved, window, cx);
        self.persist(cx);
        cx.notify();
    }

    /// Drop targets over a pane: where the dragged tab lands in this tab's layout.
    pub(super) fn render_tab_drop_zones(&self, pane: &Pane, cx: &mut Context<Self>) -> AnyElement {
        let pane_id = pane.read(cx).pane_id;
        let zone = |side: DropSide, target: &Pane, cx: &mut Context<Self>| {
            let target = target.clone();
            let (label, glyph) = match side {
                DropSide::Left => ("split.left", "columns-2"),
                DropSide::Right => ("split.right", "columns-2"),
                DropSide::Top => ("split.up", "rows-2"),
                DropSide::Bottom => ("split.down", "rows-2"),
            };
            div()
                .id(SharedString::from(format!("drop-{pane_id}-{label}")))
                .absolute()
                .flex()
                .items_center()
                .justify_center()
                .gap_1()
                .border_2()
                .border_color(hex_alpha(0, 0.))
                .t_small()
                .text_color(hex(Chrome::BRIGHT))
                .drag_over::<DraggedTab>(|style, _, _, _| style.bg(hex_alpha(Chrome::ACCENT, 0.35)).border_color(hex(Chrome::ACCENT)))
                .drag_over::<DraggedPane>(|style, _, _, _| style.bg(hex_alpha(Chrome::ACCENT, 0.35)).border_color(hex(Chrome::ACCENT)))
                .on_drop(cx.listener({
                    let target = target.clone();
                    move |this, dragged: &DraggedTab, window, cx| {
                        this.move_tab_into_pane(dragged.workspace, dragged.index, &target, side, window, cx);
                    }
                }))
                .on_drop(cx.listener(move |this, dragged: &DraggedPane, window, cx| {
                    this.move_pane_beside(dragged.pane_id, &target, side, window, cx);
                }))
                .child(icon(glyph, crate::ui::IconSize::BUTTON, hex(Chrome::BRIGHT)))
                .child(t(cx, label).to_string())
        };
        div()
            .absolute()
            .inset_0()
            // Dimmed so the drop labels are readable over whatever the terminal shows.
            .bg(hex_alpha(0x000000, 0.35))
            .child(zone(DropSide::Left, pane, cx).left_0().top_0().bottom_0().w(relative(0.28)))
            .child(zone(DropSide::Right, pane, cx).right_0().top_0().bottom_0().w(relative(0.28)))
            .child(zone(DropSide::Top, pane, cx).top_0().left(relative(0.28)).right(relative(0.28)).h(relative(0.5)))
            .child(zone(DropSide::Bottom, pane, cx).bottom_0().left(relative(0.28)).right(relative(0.28)).h(relative(0.5)))
            .into_any_element()
    }

    /// Merges a tab into another tab: its panes join that tab's split layout.
    pub(super) fn move_tab_into_tab(&mut self, dragged: &DraggedTab, target_tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get(self.active_workspace) else { return };
        if ws.id == dragged.workspace && target_tab == dragged.index {
            return;
        }
        let Some(target) = ws.tabs.get(target_tab).map(|tab| tab.active.clone()) else { return };
        self.move_tab_into_pane(dragged.workspace, dragged.index, &target, DropSide::Right, window, cx);
    }

    // -- picking a pane on screen ------------------------------------------------------------

    /// Starts "pick the pane to connect": every visible pane becomes a target.
    pub(super) fn start_connect_pick(&mut self, source: u64, cx: &mut Context<Self>) {
        self.agent_panel = None;
        self.connect_pick = Some(source);
        cx.notify();
    }

    pub(super) fn cancel_connect_pick(&mut self, cx: &mut Context<Self>) {
        if self.connect_pick.take().is_some() {
            cx.notify();
        }
    }

    /// The overlay a pane shows while the user is picking one to connect.
    pub(super) fn render_connect_pick(&self, pane: &Pane, cx: &mut Context<Self>) -> Option<AnyElement> {
        let source = self.connect_pick?;
        let view = pane.read(cx);
        let pane_id = view.pane_id;
        if pane_id == source {
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .border_2()
                    .border_color(hex(Chrome::ACCENT))
                    .flex()
                    .items_start()
                    .justify_center()
                    .child(
                        div()
                            .mt_4()
                            .px_2p5()
                            .py_1()
                            .rounded_md()
                            .bg(hex(Chrome::OVERLAY))
                            .border_1()
                            .border_color(hex(Chrome::ACCENT))
                            .shadow_lg()
                            .t_small()
                            .text_color(hex(Chrome::BRIGHT))
                            .child(t(cx, "collab.pick_source")),
                    )
                    .into_any_element(),
            );
        }
        let connectable =
            view.is_agent() && view.is_running() && self.connectable_panes(source, cx).iter().any(|p| p.read(cx).pane_id == pane_id);
        let title = view.display_title();
        let tool = view.tool_id();
        let (background, border) = if connectable {
            (hex_alpha(Chrome::ACCENT, 0.22), hex(Chrome::ACCENT))
        } else {
            (hex_alpha(0x000000, 0.45), hex_alpha(0, 0.))
        };
        Some(
            div()
                .id(SharedString::from(format!("connect-pick-{pane_id}")))
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(background)
                .border_2()
                .border_color(border)
                .when(connectable, |d| d.cursor_pointer())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if connectable {
                        this.connect_panes(source, pane_id, false, cx);
                    }
                    this.cancel_connect_pick(cx);
                }))
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(if connectable { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(crate::brand::avatar(tool, 16.))
                        .child(
                            div()
                                .t_body()
                                .font_weight(crate::theme::EMPHASIS)
                                .text_color(hex(if connectable { Chrome::BRIGHT } else { Chrome::MUTED }))
                                .child(if connectable {
                                    tf(cx, "collab.pick_this", &[("name", &title)])
                                } else {
                                    t(cx, "collab.pick_no").to_string()
                                }),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Bar shown while picking, with a way out.
    pub(super) fn render_connect_pick_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.connect_pick?;
        Some(
            div()
                .absolute()
                // Below the tab strip, so tabs stay clickable while picking a pane in another tab.
                .top(px(super::chrome::TITLE_BAR_HEIGHT + 43.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("connect-pick-bar")
                        .occlude()
                        .px_3()
                        .py_1p5()
                        .rounded_lg()
                        .flex()
                        .items_center()
                        .gap_3()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::ACCENT))
                        .shadow_lg()
                        .t_small()
                        .text_color(hex(Chrome::BRIGHT))
                        .child(icon("link", crate::ui::IconSize::INLINE, hex(Chrome::ACCENT)))
                        .child(t(cx, "collab.pick_hint"))
                        .child(
                            div()
                                .id("connect-pick-cancel")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .cursor_pointer()
                                .bg(hex(0x2d2d30))
                                .text_color(hex(Chrome::FOREGROUND))
                                .hover(|s| s.bg(hex(Chrome::HOVER)))
                                .child(t(cx, "confirm.cancel"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.cancel_connect_pick(cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}
