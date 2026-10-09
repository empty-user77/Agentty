//! TODO: tabs (and split panes) set aside to pick up later. A TODO tab leaves the tab strip but
//! keeps its terminals running; each workspace has its own list, behind the button next to the
//! start page tab, and clicking an entry brings the tab back where it was.

use super::{Pane, PaneNode, Tab, Workbench, Workspace};
use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, menu_item, popover, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, MouseButton, Pixels, Point, SharedString, Window};

impl Workspace {
    /// Indices of the tabs shown in the tab strip.
    pub(super) fn visible_tabs(&self) -> Vec<usize> {
        (0..self.tabs.len()).filter(|i| !self.tabs[*i].todo).collect()
    }

    /// Indices of the tabs set aside in TODO.
    pub(super) fn todo_tabs(&self) -> Vec<usize> {
        (0..self.tabs.len()).filter(|i| self.tabs[*i].todo).collect()
    }

    /// Moves the active tab off a TODO tab onto the nearest one still in the strip: the next one,
    /// else the one before. With every tab in TODO it stays put (the TODO list is shown instead).
    pub(super) fn settle_active_tab(&mut self) {
        let todo: Vec<bool> = self.tabs.iter().map(|tab| tab.todo).collect();
        self.active_tab = settled_tab(&todo, self.active_tab);
    }
}

/// Where the active tab goes when tab `active` is in TODO: the next tab still shown, else the
/// last one before it, else nowhere (every tab is in TODO).
fn settled_tab(todo: &[bool], active: usize) -> usize {
    if todo.get(active).is_some_and(|parked| !parked) {
        return active;
    }
    let mut visible = (0..todo.len()).filter(|i| !todo[*i]);
    visible.clone().find(|i| *i > active).or_else(|| visible.next_back()).unwrap_or(active)
}

impl Workbench {
    /// ⌃Tab / ⌃⇧Tab: the next or previous tab in the strip, TODO tabs left out.
    pub(super) fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get(self.active_workspace) else { return };
        let visible = ws.visible_tabs();
        if visible.is_empty() {
            return;
        }
        let n = visible.len();
        let next = match visible.iter().position(|i| *i == ws.active_tab) {
            Some(at) if forward => visible[(at + 1) % n],
            Some(at) => visible[(at + n - 1) % n],
            None => visible[0],
        };
        self.activate_tab(next, window, cx);
    }

    /// ⌘1…⌘9: the `nth` tab as the strip shows it.
    pub(super) fn activate_visible_tab(&mut self, nth: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.workspaces.get(self.active_workspace).and_then(|ws| ws.visible_tabs().get(nth).copied()) else {
            return;
        };
        self.activate_tab(index, window, cx);
    }

    /// Sets tab `index` of the active workspace aside in TODO.
    pub(super) fn park_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_menu = None;
        let Some(ws) = self.workspaces.get_mut(self.active_workspace) else { return };
        let Some(tab) = ws.tabs.get_mut(index) else { return };
        tab.todo = true;
        ws.settle_active_tab();
        self.after_park(window, cx);
    }

    /// Sets one pane of a split aside in TODO, as a tab of its own; a tab's only pane takes the
    /// whole tab with it.
    pub(super) fn park_pane(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.pane_menu = None;
        let Some((w, t)) = self.locate(pane) else { return };
        let ws = &mut self.workspaces[w];
        if ws.tabs[t].root.leaves().len() < 2 {
            ws.tabs[t].todo = true;
            ws.settle_active_tab();
            return self.after_park(window, cx);
        }
        // Split off before the tab leaves the list, so a failure can never drop it and its terminals.
        let Some(root) = ws.tabs[t].root.clone().remove(pane) else { return };
        let tab = &mut ws.tabs[t];
        if tab.active == *pane {
            tab.active = root.leaves()[0].clone();
        }
        tab.root = root;
        ws.tabs.push(Tab { root: PaneNode::Leaf(pane.clone()), active: pane.clone(), instance: None, todo: true });
        if self.zoomed.as_ref() == Some(pane) {
            self.zoomed = None;
        }
        self.after_park(window, cx);
    }

    fn after_park(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.persist(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Brings TODO tab `index` of the active workspace back into the strip and in front.
    pub(super) fn unpark_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.todo_menu = None;
        self.activate_tab(index, window, cx);
        self.persist(cx);
    }

    /// The TODO button beside the start page tab, while the active workspace has TODO tabs.
    pub(super) fn render_todo_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.page.is_some() {
            return None;
        }
        let count = self.workspaces.get(self.active_workspace)?.todo_tabs().len();
        if count == 0 {
            return None;
        }
        let open = self.todo_menu.is_some();
        Some(
            div()
                .id("page-tab-todo")
                .h_full()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1p5()
                .px_3()
                .border_t_1()
                .border_r_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(if open { Chrome::EDITOR } else { Chrome::TAB_INACTIVE }))
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::EDITOR)))
                .tooltip(crate::ui::Tooltip::text(t(cx, "todo.tooltip"), None))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                    if this.just_dismissed("todo") {
                        return;
                    }
                    let position = event.position();
                    this.todo_menu = match this.todo_menu {
                        Some(_) => None,
                        None => Some(gpui::point(position.x - px(12.), position.y + px(20.))),
                    };
                    cx.notify();
                }))
                .child(icon("list-todo", IconSize::INLINE, hex(if open { Chrome::BRIGHT } else { Chrome::FOREGROUND })))
                .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(count.to_string()))
                .into_any_element(),
        )
    }

    /// One TODO tab as a row: its title, what it is doing, and a close button.
    fn render_todo_row(&self, ws: &Workspace, index: usize, prefix: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let tab = &ws.tabs[index];
        let view = tab.active.read(cx);
        let leaves = tab.root.leaves();
        let attention = leaves.iter().any(|p| p.read(cx).attention);
        let working = leaves.iter().any(|p| p.read(cx).status.in_turn());
        let title = match (&ws.plugin, &tab.instance) {
            (Some(_), Some(instance)) => instance.title.clone().unwrap_or_else(|| view.display_title()),
            _ => view.display_title(),
        };
        let folder = crate::ui::tilde(&view.display_cwd());
        div()
            .id(SharedString::from(format!("{prefix}-{index}")))
            .group("todo-row")
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.unpark_tab(index, window, cx)))
            .child(div().w(px(12.)).flex().justify_center().map(|d| {
                if working {
                    d.child(crate::ui::dot_spinner((prefix, index), 12., hex_alpha(Chrome::BRIGHT, 0.9)))
                } else if attention {
                    d.child(div().size(px(7.)).rounded_full().bg(hex(Chrome::ATTENTION)))
                } else {
                    d.child(div().size(px(6.)).rounded_full().bg(hex(Chrome::MUTED)))
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(title.clone()))
                    .when(folder != title, |d| d.child(div().t_small().text_color(hex(Chrome::MUTED)).truncate().child(folder))),
            )
            .when(leaves.len() > 1, |d| d.child(div().t_small().text_color(hex(Chrome::MUTED)).child(format!("⊞{}", leaves.len()))))
            .child(
                div()
                    .id(SharedString::from(format!("{prefix}-close-{index}")))
                    .size(px(20.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .invisible()
                    .group_hover("todo-row", |s| s.visible())
                    .hover(|s| s.bg(hex_alpha(0xffffff, 0.1)))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "todo.close"), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.todo_menu = None;
                        this.request_close_tabs(&[index], window, cx);
                    }))
                    .child(icon("x", IconSize::INLINE, hex(Chrome::FOREGROUND))),
            )
            .into_any_element()
    }

    pub(super) fn render_todo_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let position: Point<Pixels> = self.todo_menu?;
        let ws = self.workspaces.get(self.active_workspace)?;
        let todo = ws.todo_tabs();
        if todo.is_empty() {
            return None;
        }
        let mut list = popover()
            .w(px(320.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.todo_menu = None;
                this.note_dismissed("todo");
                cx.notify();
            }))
            .child(div().px_3().pt_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "todo.heading")))
            .child(div().px_3().pb_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "todo.hint")));
        for index in todo {
            list = list.child(self.render_todo_row(ws, index, "todo-menu-row", cx));
        }
        Some(gpui::deferred(gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(list)).with_priority(3))
    }

    /// The terminal area while every tab of the workspace is in TODO.
    pub(super) fn render_all_parked(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div().w(px(420.)).max_w_full().flex().flex_col().gap_0p5();
        if let Some(ws) = self.workspaces.get(self.active_workspace) {
            for index in ws.todo_tabs() {
                list = list.child(self.render_todo_row(ws, index, "todo-page-row", cx));
            }
        }
        div()
            .id("todo-all-parked")
            .size_full()
            .bg(hex(Chrome::PANEL))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .px_4()
            .child(icon("list-todo", 28., hex(Chrome::MUTED)))
            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(t(cx, "todo.all_parked")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "todo.all_parked_hint")))
            .child(list)
            .into_any_element()
    }

    pub(super) fn open_pane_menu(&mut self, pane: Pane, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.pane_menu = Some((pane, position));
        cx.notify();
    }

    /// Right-click menu on a split pane's header.
    pub(super) fn render_pane_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (pane, position) = self.pane_menu.clone()?;
        let list = popover()
            .w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.pane_menu = None;
                cx.notify();
            }))
            .child(menu_item(
                "pane-menu-todo",
                t(cx, "todo.park"),
                cx.listener(move |this, _: &ClickEvent, window, cx| this.park_pane(&pane, window, cx)),
            ));
        Some(gpui::deferred(gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(list)).with_priority(3))
    }
}

#[cfg(test)]
mod tests {
    use super::settled_tab;

    #[test]
    fn active_tab_moves_to_the_nearest_shown_tab() {
        assert_eq!(settled_tab(&[false, true, false], 1), 2);
        assert_eq!(settled_tab(&[false, false, true], 2), 1);
        assert_eq!(settled_tab(&[false, true], 0), 0);
        // Every tab in TODO: it stays, and the TODO list is shown instead.
        assert_eq!(settled_tab(&[true, true], 0), 0);
    }
}
