//! Right-click menu on a tab: close this / other / all tabs, set it aside in TODO, move the tab to
//! another workspace, and the recently closed tabs to reopen.

use super::workspace_menu::{row, rule, HoverListener};
use super::{Tab, Workbench, Workspace};
use crate::i18n::t;
use crate::theme::{hex, Chrome};
use crate::ui::{icon, popover, IconSize, TypeScale};
use gpui::{div, prelude::*, px, ClickEvent, Context, Pixels, Point, SharedString, Window};

const MENU_WIDTH: f32 = 240.;
const FLYOUT_WIDTH: f32 = 240.;

pub struct TabMenu {
    pub index: usize,
    pub position: Point<Pixels>,
    /// "Move to Workspace ›" unfolded: the height in the window of the row it hangs off. Drawn
    /// apart from the menu (anchored beside it), since the menu scrolls and would clip it.
    flyout_at: Option<Pixels>,
}

impl Workbench {
    pub(super) fn open_tab_menu(&mut self, index: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.tab_menu = Some(TabMenu { index, position, flyout_at: None });
        cx.notify();
    }

    /// Unfolds the move flyout at height `y`, or folds it away with `None`.
    fn tab_flyout(&mut self, y: Option<Pixels>, cx: &mut Context<Self>) {
        if let Some(menu) = self.tab_menu.as_mut().filter(|m| m.flyout_at.is_some() != y.is_some()) {
            menu.flyout_at = y;
            cx.notify();
        }
    }

    fn close_tabs_where(&mut self, keep: impl Fn(usize) -> bool, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_menu = None;
        let Some(ws) = self.workspaces.get(self.active_workspace) else { return };
        // TODO tabs are kept for later: "close others / all" means the tabs in the strip.
        let indices: Vec<usize> = ws.visible_tabs().into_iter().filter(|i| !keep(*i)).collect();
        self.request_close_tabs(&indices, window, cx);
        cx.notify();
    }

    /// Moves tab `index` of the active workspace into workspace `target` (a new workspace when `None`).
    pub(super) fn move_tab(&mut self, index: usize, target: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_menu = None;
        let source = self.active_workspace;
        if self.workspaces.get(source).is_none_or(|ws| index >= ws.tabs.len()) {
            return;
        }
        let tab: Tab = {
            let ws = &mut self.workspaces[source];
            let tab = ws.tabs.remove(index);
            if ws.active_tab >= ws.tabs.len() {
                ws.active_tab = ws.tabs.len().saturating_sub(1);
            } else if index < ws.active_tab {
                ws.active_tab -= 1;
            }
            ws.settle_active_tab();
            tab
        };
        let source_id = self.workspaces[source].id;
        // A tab taken out of a workspace hidden from remote access stays hidden in a new one.
        let source_hidden = self.workspaces[source].remote_hidden;
        if self.workspaces[source].tabs.is_empty() && self.workspaces[source].dormant.is_none() && self.workspaces[source].parked.is_empty()
        {
            self.workspaces.remove(source);
        }
        let target_index = match target.and_then(|id| self.workspaces.iter().position(|w| w.id == id)) {
            Some(i) => {
                if let Some(snapshot) = self.workspaces[i].dormant.take() {
                    self.revive(i, snapshot, cx);
                }
                i
            }
            None => {
                let id = self.next_id();
                let cwd = tab.active.read(cx).display_cwd();
                let group = self.workspaces.iter().find(|w| w.id == source_id).and_then(|w| w.group);
                self.workspaces.push(Workspace {
                    id,
                    name: None,
                    group,
                    cwd,
                    tabs: Vec::new(),
                    active_tab: 0,
                    dormant: None,
                    asleep_on_close: false,
                    closed_tabs: Vec::new(),
                    color: None,
                    plugin: None,
                    remote_hidden: source_hidden,
                    marks: Default::default(),
                    parked: Vec::new(),
                });
                self.workspaces.len() - 1
            }
        };
        let ws = &mut self.workspaces[target_index];
        ws.tabs.push(tab);
        ws.active_tab = ws.tabs.len() - 1;
        self.active_workspace = target_index;
        self.page = None;
        self.persist(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Hovering a plain row of the tab menu folds the move flyout away.
    fn fold_tab_flyout(cx: &mut Context<Self>) -> HoverListener {
        Box::new(cx.listener(|this, hovered: &bool, _, cx| {
            if *hovered {
                this.tab_flyout(None, cx);
            }
        }))
    }

    pub(super) fn render_tab_menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.tab_menu.as_ref()?;
        let (index, position, flyout_at) = (menu.index, menu.position, menu.flyout_at);
        let active = self.workspaces.get(self.active_workspace)?;
        let count = active.visible_tabs().len();
        let has_targets = self.workspaces.len() > 1;
        // The fixed actions first, then where the tab can go, then the history.
        let list = popover()
            .w(px(MENU_WIDTH))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.tab_menu = None;
                cx.notify();
            }))
            .child(
                row("tab-menu-close", "x", t(cx, "tab.close"), false)
                    .on_hover(Self::fold_tab_flyout(cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.close_tabs_where(|i| i != index, window, cx))),
            )
            .when(count > 1, |d| {
                d.child(
                    row("tab-menu-close-others", "minus", t(cx, "tab.close_others"), false)
                        .on_hover(Self::fold_tab_flyout(cx))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.close_tabs_where(|i| i == index, window, cx))),
                )
            })
            .child(
                row("tab-menu-close-all", "circle-x", t(cx, "tab.close_all"), false)
                    .on_hover(Self::fold_tab_flyout(cx))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_tabs_where(|_| false, window, cx))),
            )
            .child(rule())
            .child(
                row("tab-menu-todo", "list-todo", t(cx, "todo.park"), false)
                    .on_hover(Self::fold_tab_flyout(cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.park_tab(index, window, cx))),
            )
            .child(rule())
            // Move to Workspace ›: the workspaces, grouped as the sidebar groups them, in a flyout.
            .child(
                row("tab-menu-move", "arrow-right", t(cx, "tab.move_to_menu"), false)
                    .when(flyout_at.is_some(), |d| d.bg(hex(Chrome::HOVER)))
                    .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| this.tab_flyout(Some(event.position.y), cx)))
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| this.tab_flyout(Some(event.position().y), cx)))
                    .child(icon("chevron-right", IconSize::INLINE, hex(Chrome::MUTED))),
            )
            .child(div().id("tab-menu-history").on_hover(Self::fold_tab_flyout(cx)).children(self.render_reopen_tabs(cx)));
        let list = crate::ui::fit_to_window(list, "tab-menu", window);
        let flyout = flyout_at.map(|y| {
            let at = gpui::point(position.x + px(MENU_WIDTH - 6.), y - px(18.));
            gpui::anchored().position(at).snap_to_window_with_margin(px(8.)).child(self.render_tab_move_flyout(
                index,
                has_targets,
                window,
                cx,
            ))
        });
        Some(
            gpui::deferred(
                div().child(gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(list)).children(flyout),
            )
            .with_priority(3),
        )
    }

    /// Where the tab can go: a new workspace, then every other workspace under its group's name,
    /// the ungrouped ones first, as the sidebar lists them. Rows act as the button goes down: the
    /// menu beside it closes on that same press (the flyout lies outside it).
    fn render_tab_move_flyout(
        &self,
        index: usize,
        has_targets: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let active_id = self.workspaces.get(self.active_workspace).map(|w| w.id);
        let target = |id: u64, ws: &Workspace, cx: &mut Context<Self>| {
            crate::ui::menu_item_with_dot(
                SharedString::from(format!("tab-menu-move-{id}")),
                super::accent_color(ws.color),
                self.workspace_label(ws, cx),
                |_, _, _| {},
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.move_tab(index, Some(id), window, cx);
                }),
            )
        };
        let heading = |name: String, color: Option<u32>| {
            div()
                .px_2()
                .pt_2()
                .pb_0p5()
                .flex()
                .items_center()
                .gap_1p5()
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .child(icon("folder", 12., hex(color.unwrap_or(Chrome::MUTED))))
                .child(div().min_w_0().truncate().child(name))
        };
        let mut flyout =
            popover().w(px(FLYOUT_WIDTH)).child(row("tab-menu-move-new", "plus", t(cx, "new.workspace"), false).on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.move_tab(index, None, window, cx);
                }),
            ));
        if has_targets {
            flyout = flyout.child(rule());
        }
        let others = |group: Option<u64>| self.workspaces.iter().filter(move |w| Some(w.id) != active_id && w.group == group);
        let ungrouped: Vec<&Workspace> = others(None).collect();
        if !ungrouped.is_empty() && !self.groups.is_empty() {
            flyout = flyout.child(heading(t(cx, "ungrouped").to_string(), None));
        }
        for ws in ungrouped {
            flyout = flyout.child(target(ws.id, ws, cx));
        }
        for group in &self.groups {
            let members: Vec<&Workspace> = others(Some(group.id)).collect();
            if members.is_empty() {
                continue;
            }
            flyout = flyout.child(heading(group.name.clone(), super::accent_color(group.color)));
            for ws in members {
                flyout = flyout.child(div().pl_2().child(target(ws.id, ws, cx)));
            }
        }
        crate::ui::fit_to_window(flyout, "tab-menu-flyout", window)
    }
}
