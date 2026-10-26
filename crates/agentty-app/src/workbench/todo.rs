//! TODO: tabs (and split panes) set aside to pick up later. A TODO tab leaves the tab strip and,
//! like a sleeping workspace, stops: its terminals and agents end, and only a snapshot is kept
//! (its splits, folders and agent sessions). Each workspace has its own list, behind the button
//! next to the start page tab; clicking an entry starts the tab again, its agents continuing
//! their sessions.

use super::persist::{NodeSnapshot, TabSnapshot};
use super::{Pane, Workbench, Workspace};
use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, menu_item, popover, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, MouseButton, Pixels, Point, SharedString, Window};

impl Workspace {
    /// Indices of the tabs shown in the tab strip: all of them, TODO tabs being kept apart.
    pub(super) fn visible_tabs(&self) -> Vec<usize> {
        (0..self.tabs.len()).collect()
    }

    /// Keeps the active tab on a tab that exists after one left the strip.
    pub(super) fn settle_active_tab(&mut self) {
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
    }
}

/// What a TODO entry shows: the title of the pane that was in front, its folder, and how many
/// panes the tab had.
fn parked_summary(snapshot: &TabSnapshot) -> (String, String, usize) {
    let panes = super::snapshot_panes(&snapshot.layout);
    let front = panes.get(snapshot.active_pane).or(panes.first());
    let title = snapshot.instance.as_ref().and_then(|i| i.title.clone()).or_else(|| front.map(|p| p.title.clone())).unwrap_or_default();
    let folder = front.map(|p| crate::ui::tilde(&p.cwd)).unwrap_or_default();
    (title, folder, panes.len())
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

    /// Sets tab `index` of the active workspace aside in TODO: its snapshot is kept and its panes
    /// close, ending what ran in them.
    pub(super) fn park_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_menu = None;
        let w = self.active_workspace;
        let Some(tab) = self.workspaces.get(w).and_then(|ws| ws.tabs.get(index)) else { return };
        let snapshot = self.snapshot_tab(tab, cx);
        let ws = &mut self.workspaces[w];
        let tab = ws.tabs.remove(index);
        if index < ws.active_tab {
            ws.active_tab -= 1;
        }
        ws.settle_active_tab();
        ws.parked.insert(0, snapshot);
        self.end_parked_panes(tab.root.leaves(), window, cx);
    }

    /// Sets one pane of a split aside in TODO, as a tab of its own; a tab's only pane takes the
    /// whole tab with it.
    pub(super) fn park_pane(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.pane_menu = None;
        let Some((w, t)) = self.locate(pane) else { return };
        if self.workspaces[w].tabs[t].root.leaves().len() < 2 {
            self.active_workspace = w;
            return self.park_tab(t, window, cx);
        }
        let snapshot = TabSnapshot {
            instance: None,
            layout: NodeSnapshot::Pane(Self::snapshot_pane(pane, cx)),
            active_pane: 0,
            zoomed_pane: None,
            chat: false,
            chat_roles: None,
            todo: false,
        };
        self.workspaces[w].parked.insert(0, snapshot);
        // Out of its split the usual way: the split closes up around where it was.
        self.remove_pane(pane, cx);
        self.end_parked_panes(Vec::new(), window, cx);
    }

    /// Closes the panes of a tab just set aside (already out of its workspace, so closing them
    /// neither records a closed tab nor folds the workspace), then saves.
    fn end_parked_panes(&mut self, panes: Vec<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        for pane in panes {
            self.remove_pane(&pane, cx);
        }
        self.persist(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Starts TODO entry `index` of the active workspace again as a tab, in front.
    pub(super) fn unpark_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.todo_menu = None;
        let w = self.active_workspace;
        if self.workspaces.get(w).is_none_or(|ws| index >= ws.parked.len()) {
            return;
        }
        let snapshot = self.workspaces[w].parked.remove(index);
        self.bring_back_tab(w, snapshot, window, cx);
    }

    /// Drops TODO entry `index` of the active workspace. Nothing of it was running; it goes to the
    /// recently closed tabs, so it can still be reopened from there.
    fn drop_parked(&mut self, index: usize, cx: &mut Context<Self>) {
        self.todo_menu = None;
        let Some(ws) = self.workspaces.get_mut(self.active_workspace).filter(|ws| index < ws.parked.len()) else { return };
        let snapshot = ws.parked.remove(index);
        ws.closed_tabs.insert(0, snapshot);
        ws.closed_tabs.truncate(super::CLOSED_TAB_HISTORY);
        self.persist(cx);
        cx.notify();
    }

    /// The TODO button beside the start page tab, while the active workspace has TODO tabs.
    pub(super) fn render_todo_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.page.is_some() {
            return None;
        }
        let count = self.workspaces.get(self.active_workspace)?.parked.len();
        if count == 0 {
            return None;
        }
        let open = self.todo_menu.is_some();
        Some(
            div()
                .id("page-tab-todo")
                .h(px(super::chrome::TAB_CARD_HEIGHT))
                // Not a card: on the strip's middle line, with the tabs' labels.
                .my_auto()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1p5()
                .px_3()
                .rounded_md()
                .when(open, |d| d.bg(hex_alpha(Chrome::BRIGHT, 0.08)))
                .cursor_pointer()
                .hover(|s| s.bg(hex_alpha(Chrome::BRIGHT, 0.06)))
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

    /// One TODO entry as a row: its title and folder, a moon (nothing of it is running), and a
    /// close button.
    fn render_todo_row(&self, ws: &Workspace, index: usize, prefix: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let (title, folder, panes) = parked_summary(&ws.parked[index]);
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
            .child(div().w(px(12.)).flex().justify_center().child(icon("moon", 12., hex(Chrome::MUTED))))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(title.clone()))
                    .when(folder != title, |d| d.child(div().t_small().text_color(hex(Chrome::MUTED)).truncate().child(folder))),
            )
            .when(panes > 1, |d| d.child(div().t_small().text_color(hex(Chrome::MUTED)).child(format!("⊞{panes}"))))
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
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.drop_parked(index, cx);
                    }))
                    .child(icon("x", IconSize::INLINE, hex(Chrome::FOREGROUND))),
            )
            .into_any_element()
    }

    pub(super) fn render_todo_menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let position: Point<Pixels> = self.todo_menu?;
        let ws = self.workspaces.get(self.active_workspace)?;
        let todo = 0..ws.parked.len();
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
        let list = crate::ui::fit_to_window(list, "todo-menu", window);
        Some(gpui::deferred(gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(list)).with_priority(3))
    }

    /// The terminal area while every tab of the workspace is in TODO.
    pub(super) fn render_all_parked(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div().w(px(420.)).max_w_full().flex().flex_col().gap_0p5();
        if let Some(ws) = self.workspaces.get(self.active_workspace) {
            for index in 0..ws.parked.len() {
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
    use super::parked_summary;
    use crate::launch::PaneKind;
    use crate::workbench::persist::{NodeSnapshot, PaneSnapshot, TabSnapshot};

    fn pane(title: &str, cwd: &str) -> NodeSnapshot {
        NodeSnapshot::Pane(PaneSnapshot { kind: PaneKind::Shell, cwd: cwd.into(), title: title.into(), session_id: None, tool: None })
    }

    #[test]
    fn an_entry_names_the_pane_that_was_in_front() {
        let layout = NodeSnapshot::Split {
            axis: crate::workbench::panes::Axis::Horizontal,
            sizes: vec![0.5, 0.5],
            children: vec![pane("server", "/w/api"), pane("claude", "/w/app")],
        };
        let tab = TabSnapshot { instance: None, layout, active_pane: 1, zoomed_pane: None, chat: false, chat_roles: None, todo: false };
        let (title, folder, panes) = parked_summary(&tab);
        assert_eq!((title.as_str(), panes), ("claude", 2));
        assert!(folder.ends_with("app"), "{folder}");
    }
}
