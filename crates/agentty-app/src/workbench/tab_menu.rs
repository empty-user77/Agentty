//! Right-click menu on a tab: close this / other / all tabs, move the tab to another workspace.

use super::{Tab, Workbench, Workspace};
use crate::i18n::t;
use crate::theme::{hex, Chrome};
use crate::ui::{menu_item, popover, TypeScale};
use gpui::{div, prelude::*, px, ClickEvent, Context, Pixels, Point, SharedString, Window};

pub struct TabMenu {
    pub index: usize,
    pub position: Point<Pixels>,
}

impl Workbench {
    pub(super) fn open_tab_menu(&mut self, index: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.tab_menu = Some(TabMenu { index, position });
        cx.notify();
    }

    fn close_tabs_where(&mut self, keep: impl Fn(usize) -> bool, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_menu = None;
        let Some(ws) = self.workspaces.get(self.active_workspace) else { return };
        let indices: Vec<usize> = (0..ws.tabs.len()).filter(|i| !keep(*i)).collect();
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
            tab
        };
        let source_id = self.workspaces[source].id;
        if self.workspaces[source].tabs.is_empty() && self.workspaces[source].dormant.is_none() {
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
                    closed_tabs: Vec::new(),
                    color: None,
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

    pub(super) fn render_tab_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.tab_menu.as_ref()?;
        let (index, position) = (menu.index, menu.position);
        let active = self.workspaces.get(self.active_workspace)?;
        let count = active.tabs.len();
        let mut list = popover()
            .w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.tab_menu = None;
                cx.notify();
            }))
            .children(self.render_reopen_tabs(cx))
            .child(menu_item(
                "tab-menu-close",
                t(cx, "tab.close"),
                cx.listener(move |this, _: &ClickEvent, window, cx| this.close_tabs_where(|i| i != index, window, cx)),
            ))
            .when(count > 1, |d| {
                d.child(menu_item(
                    "tab-menu-close-others",
                    t(cx, "tab.close_others"),
                    cx.listener(move |this, _: &ClickEvent, window, cx| this.close_tabs_where(|i| i == index, window, cx)),
                ))
            })
            .child(menu_item(
                "tab-menu-close-all",
                t(cx, "tab.close_all"),
                cx.listener(|this, _: &ClickEvent, window, cx| this.close_tabs_where(|_| false, window, cx)),
            ))
            .child(div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(div().px_3().pt_1().pb_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "tab.move_to")));
        for ws in self.workspaces.iter().filter(|w| w.id != active.id) {
            let id = ws.id;
            // The workspace's own colour, when it has one, so the list reads like the sidebar.
            list = list.child(crate::ui::menu_item_with_dot(
                SharedString::from(format!("tab-menu-move-{id}")),
                super::accent_color(ws.color),
                self.workspace_title(ws, cx),
                cx.listener(move |this, _: &ClickEvent, window, cx| this.move_tab(index, Some(id), window, cx)),
            ));
        }
        list = list.child(menu_item(
            "tab-menu-move-new",
            format!("+ {}", t(cx, "new.workspace")),
            cx.listener(move |this, _: &ClickEvent, window, cx| this.move_tab(index, None, window, cx)),
        ));
        Some(gpui::deferred(gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(list)).with_priority(3))
    }
}
