//! The workspace card's menu (right click, or the ⋯ on the card) and what it does: open the
//! workspace's folder in an editor, copy its path or its worktree / branch name, pin it, mark it
//! unread, file it in a group, list it under a parent, put it to sleep, delete its worktree.
//!
//! "The workspace's folder" is where its selected pane works now, else where it was left (a
//! sleeping workspace has no pane to ask).

use super::chrome::ColorTarget;
use super::confirm::{CloseConfirm, CloseTarget};
use super::{RenameTarget, Workbench, Workspace};
use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, popover, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, ElementId, Entity, MouseButton, SharedString, Subscription, Window};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const MENU_WIDTH: f32 = 236.;
const FLYOUT_WIDTH: f32 = 210.;
const ROW: f32 = 28.;
const RULE: f32 = 9.;

/// What a menu row runs when the pointer comes onto it or leaves it.
type HoverListener = Box<dyn Fn(&bool, &mut Window, &mut gpui::App)>;

/// The flyout unfolded beside the workspace menu.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Submenu {
    OpenIn,
    Group,
}

/// "Set Parent Worktree…": which workspace gets a parent, and the search over the candidates.
pub struct ParentPicker {
    workspace: u64,
    search: Entity<TextInput>,
    /// Row under the keyboard, counting "No parent" when it is shown.
    selected: usize,
    _subscription: Subscription,
}

/// What "Copy … Name" copies: a linked worktree's folder name, else the branch.
enum TreeName {
    Worktree(String),
    Branch(String),
}

/// `members` (indices into the workspace list, in list order) with each one's depth: a workspace
/// whose parent is among them comes right after it, one level further in. A parent that is not
/// among them (closed, or in another group) leaves the workspace at the top level, and a loop of
/// parents never hides anyone.
pub(super) fn nest(members: &[usize], parent_of: impl Fn(usize) -> Option<usize>) -> Vec<(usize, usize)> {
    let inside: HashSet<usize> = members.iter().copied().collect();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    for &m in members {
        if let Some(p) = parent_of(m).filter(|p| inside.contains(p) && *p != m) {
            children.entry(p).or_default().push(m);
        }
    }
    let mut out = Vec::with_capacity(members.len());
    let mut seen = HashSet::new();
    fn visit(m: usize, depth: usize, children: &HashMap<usize, Vec<usize>>, seen: &mut HashSet<usize>, out: &mut Vec<(usize, usize)>) {
        if !seen.insert(m) {
            return;
        }
        out.push((m, depth));
        for &c in children.get(&m).into_iter().flatten() {
            visit(c, depth + 1, children, seen, out);
        }
    }
    let roots: Vec<usize> = members.iter().copied().filter(|m| parent_of(*m).is_none_or(|p| !inside.contains(&p) || p == *m)).collect();
    for m in roots {
        visit(m, 0, &children, &mut seen, &mut out);
    }
    // Only a loop is left: it starts where the list has it.
    for &m in members {
        visit(m, 0, &children, &mut seen, &mut out);
    }
    out
}

/// A menu row: icon, label, and an action on click. `danger` rows are drawn in red.
fn row(id: impl Into<ElementId>, glyph: &'static str, label: impl Into<SharedString>, danger: bool) -> gpui::Stateful<gpui::Div> {
    let ink = if danger { Chrome::ERROR } else { Chrome::FOREGROUND };
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .flex()
        .items_center()
        .gap_2()
        .t_body()
        .cursor_pointer()
        .text_color(hex(ink))
        .hover(crate::ui::hover_fill)
        .child(icon(glyph, IconSize::INLINE, hex(if danger { Chrome::ERROR } else { Chrome::MUTED })))
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
}

fn rule() -> gpui::Div {
    div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER))
}

impl Workbench {
    /// Opens the menu of workspace `id`, asked for at height `at` in the window.
    pub(super) fn open_workspace_menu(&mut self, id: u64, at: f32) {
        self.workspace_menu_at = at;
        self.workspace_menu = Some(id);
        self.workspace_submenu = None;
        // Installed since the last time is offered now; a lookup is a few folder checks.
        self.open_in_apps = crate::open_in::installed();
    }

    /// The workspace's folder: where its selected pane works, else where it was left.
    fn menu_folder(&self, id: u64, cx: &gpui::App) -> Option<PathBuf> {
        self.workspaces.iter().find(|w| w.id == id).map(|ws| self.workspace_folder(ws, cx))
    }

    fn tree_name(&self, ws: &Workspace, cx: &gpui::App) -> Option<TreeName> {
        let folder = self.workspace_folder(ws, cx);
        let root = agentty_bridge::worktree::tree_root(&folder)?;
        if agentty_bridge::worktree::is_linked(&root) {
            return root.file_name().map(|n| TreeName::Worktree(n.to_string_lossy().to_string()));
        }
        ws.tabs
            .get(ws.active_tab)
            .and_then(|t| t.active.read(cx).git_branch.clone())
            .or_else(|| self.folder_branch(&folder).map(str::to_string))
            .or_else(|| ws.dormant.as_ref().and_then(|d| d.branch.clone()))
            .map(TreeName::Branch)
    }

    /// The linked worktree workspace `id` works in, when nothing outside it works there too:
    /// what "Delete Worktree" removes.
    fn deletable_tree(&self, id: u64, cx: &gpui::App) -> Option<PathBuf> {
        let ws = self.workspaces.iter().find(|w| w.id == id)?;
        let tree = agentty_bridge::worktree::tree_root(&self.workspace_folder(ws, cx))?;
        if !agentty_bridge::worktree::is_linked(&tree) {
            return None;
        }
        let own: Vec<super::Pane> = ws.tabs.iter().flat_map(|t| t.root.leaves()).collect();
        let used_elsewhere = self
            .all_panes()
            .iter()
            .filter(|p| !own.contains(p))
            .any(|p| agentty_bridge::worktree::tree_root(&p.read(cx).display_cwd()).as_deref() == Some(tree.as_path()));
        (!used_elsewhere).then_some(tree)
    }

    fn copy_for_workspace(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
        self.set_status(tf(cx, "workspace.copied", &[("text", &text)]), cx);
        self.workspace_menu = None;
        cx.notify();
    }

    fn open_workspace_in(&mut self, id: u64, app: crate::open_in::OpenApp, cx: &mut Context<Self>) {
        self.workspace_menu = None;
        if let Some(folder) = self.menu_folder(id, cx) {
            if let Err(err) = app.open(&folder) {
                self.show_toast_for(tf(cx, "workspace.open_failed", &[("app", app.name), ("error", &err.to_string())]), 6000, cx);
            }
        }
        cx.notify();
    }

    /// Pinned goes to the top of the list (and so of its group); unpinning leaves it where it is.
    fn toggle_pinned(&mut self, id: u64, cx: &mut Context<Self>) {
        self.workspace_menu = None;
        let active_id = self.workspaces.get(self.active_workspace).map(|w| w.id);
        let Some(from) = self.workspaces.iter().position(|w| w.id == id) else { return };
        let mut ws = self.workspaces.remove(from);
        ws.marks.pinned = !ws.marks.pinned;
        self.workspaces.insert(if ws.marks.pinned { 0 } else { from }, ws);
        self.restore_active(active_id);
        self.persist(cx);
        cx.notify();
    }

    fn toggle_unread(&mut self, id: u64, cx: &mut Context<Self>) {
        self.workspace_menu = None;
        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == id) {
            ws.marks.unread = !ws.marks.unread;
        }
        self.persist(cx);
        cx.notify();
    }

    /// Ends every process of the workspace and keeps it as it was, like after a restart: opening
    /// it again starts its tabs and continues their agent sessions. The workspace in front hands
    /// over to the nearest one still awake, or to the start page.
    pub(super) fn sleep_workspace(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace_menu = None;
        let Some(index) = self.workspaces.iter().position(|w| w.id == id) else { return };
        if self.workspaces[index].tabs.is_empty() {
            return;
        }
        let snapshot = self.live_snapshot(&self.workspaces[index], cx);
        let name = self.workspace_label(&self.workspaces[index], cx);
        let ws = &mut self.workspaces[index];
        let tabs = std::mem::take(&mut ws.tabs);
        ws.active_tab = 0;
        ws.dormant = Some(snapshot);
        ws.asleep_on_close = false;
        // The tabs are already out of the workspace: closing their panes only lets the
        // processes go, it neither records closed tabs nor folds the workspace a second time.
        for pane in tabs.iter().flat_map(|t| t.root.leaves()) {
            self.remove_pane(&pane, cx);
        }
        if index == self.active_workspace {
            let awake = (0..self.workspaces.len())
                .filter(|&i| i != index && !self.workspaces[i].tabs.is_empty())
                .min_by_key(|&i| i.abs_diff(index));
            match awake {
                Some(next) => self.activate_workspace(next, window, cx),
                None => self.open_welcome(window, cx),
            }
        }
        self.set_status(tf(cx, "workspace.slept", &[("name", &name)]), cx);
        self.persist(cx);
        cx.notify();
    }

    /// "Delete Worktree": the close dialog for the workspace, with its worktree ticked to go too.
    fn request_delete_tree(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace_menu = None;
        let Some(tree) = self.deletable_tree(id, cx) else { return };
        self.close_confirm = Some(CloseConfirm {
            target: CloseTarget::Workspace(id),
            dont_ask: false,
            removes_workspace: false,
            trees: vec![tree],
            remove_trees: true,
            synced: self.workspace_synced(id),
            delete_sync: false,
            deleting_tree: true,
        });
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Workspace `id` and every workspace listed under it, at any depth.
    fn with_descendants(&self, id: u64) -> HashSet<u64> {
        let mut found = HashSet::from([id]);
        loop {
            let before = found.len();
            for ws in &self.workspaces {
                if ws.marks.parent.is_some_and(|p| found.contains(&p)) {
                    found.insert(ws.id);
                }
            }
            if found.len() == before {
                return found;
            }
        }
    }

    /// The order a list of workspaces is drawn in, parents before the workspaces under them.
    pub(super) fn nested_order(&self, members: &[usize]) -> Vec<(usize, usize)> {
        nest(members, |i| {
            let parent = self.workspaces[i].marks.parent?;
            self.workspaces.iter().position(|w| w.id == parent)
        })
    }

    /// Lists `id` under `parent` (or on its own with `None`). It joins the parent's group, so the
    /// two are drawn together; a parent that is `id` itself or one of its own children is refused.
    fn set_parent(&mut self, id: u64, parent: Option<u64>, cx: &mut Context<Self>) {
        if parent.is_some_and(|p| self.with_descendants(id).contains(&p)) {
            return;
        }
        let group = parent.and_then(|p| self.workspaces.iter().find(|w| w.id == p)).map(|w| w.group);
        let moving = self.with_descendants(id);
        for ws in self.workspaces.iter_mut() {
            if ws.id == id {
                ws.marks.parent = parent;
            }
            if let Some(group) = group.filter(|_| moving.contains(&ws.id)) {
                ws.group = group;
            }
        }
        self.persist(cx);
        cx.notify();
    }

    /// Moving a workspace to a group takes the workspaces listed under it along.
    pub(super) fn group_with_children(&mut self, id: u64, group: Option<u64>) {
        let moving = self.with_descendants(id);
        for ws in self.workspaces.iter_mut().filter(|w| moving.contains(&w.id)) {
            ws.group = group;
        }
    }

    fn open_parent_picker(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace_menu = None;
        let search = cx.new(|cx| TextInput::localized("", "parent.search", window, cx));
        let subscription = cx.subscribe_in(&search, window, |this, _, event: &TextInputEvent, window, cx| {
            let rows = this.parent_rows(cx);
            let count = rows.len();
            let Some(picker) = this.parent_picker.as_mut() else { return };
            match event {
                TextInputEvent::Changed => picker.selected = 0,
                TextInputEvent::Up => picker.selected = picker.selected.saturating_sub(1),
                TextInputEvent::Down => picker.selected = (picker.selected + 1).min(count.saturating_sub(1)),
                TextInputEvent::Confirmed => {
                    let chosen = rows.get(picker.selected).copied();
                    if let Some(parent) = chosen {
                        let id = picker.workspace;
                        this.close_parent_picker(window, cx);
                        this.set_parent(id, parent, cx);
                    }
                    return;
                }
                TextInputEvent::Cancelled => return this.close_parent_picker(window, cx),
                _ => return,
            }
            cx.notify();
        });
        window.focus(&gpui::Focusable::focus_handle(search.read(cx), cx));
        self.parent_picker = Some(ParentPicker { workspace: id, search, selected: 0, _subscription: subscription });
        cx.notify();
    }

    fn close_parent_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.parent_picker = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// The picker's rows: `None` ("No parent") first when the workspace has one, then every
    /// workspace that may be its parent and matches the search.
    fn parent_rows(&self, cx: &gpui::App) -> Vec<Option<u64>> {
        let Some(picker) = &self.parent_picker else { return Vec::new() };
        let query = picker.search.read(cx).text().trim().to_lowercase();
        let excluded = self.with_descendants(picker.workspace);
        let current = self.workspaces.iter().find(|w| w.id == picker.workspace).and_then(|w| w.marks.parent);
        let mut rows: Vec<Option<u64>> = Vec::new();
        if current.is_some() && query.is_empty() {
            rows.push(None);
        }
        rows.extend(
            self.workspaces
                .iter()
                .filter(|ws| !excluded.contains(&ws.id) && ws.plugin.is_none())
                .filter(|ws| {
                    query.is_empty() || {
                        let (_, project, branch) = self.parent_row_text(ws, cx);
                        [self.workspace_label(ws, cx), project, branch.unwrap_or_default()]
                            .iter()
                            .any(|s| s.to_lowercase().contains(&query))
                    }
                })
                .map(|ws| Some(ws.id)),
        );
        rows
    }

    /// Name, project (the repository's main folder) and branch of a picker row.
    fn parent_row_text(&self, ws: &Workspace, cx: &gpui::App) -> (String, String, Option<String>) {
        let folder = self.workspace_folder(ws, cx);
        let project = agentty_bridge::worktree::main_tree(&folder)
            .or_else(|| agentty_bridge::worktree::tree_root(&folder))
            .unwrap_or(folder)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let branch = match self.tree_name(ws, cx) {
            Some(TreeName::Branch(b)) => Some(b),
            Some(TreeName::Worktree(_)) => ws.tabs.get(ws.active_tab).and_then(|t| t.active.read(cx).git_branch.clone()),
            None => None,
        };
        (self.workspace_label(ws, cx), project, branch)
    }

    pub(super) fn render_parent_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.parent_picker.as_ref()?;
        let target = self.workspaces.iter().find(|w| w.id == picker.workspace)?;
        let current = target.marks.parent;
        let rows = self.parent_rows(cx);
        let mut list = div().id("parent-picker-list").flex().flex_col().gap_px().p_1().max_h(px(360.)).overflow_y_scroll();
        if rows.is_empty() {
            list = list.child(crate::ui::hint(t(cx, "parent.empty")));
        }
        for (i, row_parent) in rows.iter().copied().enumerate() {
            let selected = i == picker.selected;
            let id = picker.workspace;
            let mut line = div()
                .id(("parent-row", i))
                .px_2()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .flex()
                .items_start()
                .gap_2()
                .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
                .when(!selected, |d| d.hover(crate::ui::hover_fill))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.close_parent_picker(window, cx);
                    this.set_parent(id, row_parent, cx);
                }));
            match row_parent.and_then(|p| self.workspaces.iter().find(|w| w.id == p)) {
                None => {
                    line = line
                        .items_center()
                        .child(icon("circle-x", IconSize::INLINE, hex(Chrome::MUTED)))
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "parent.none")));
                }
                Some(ws) => {
                    let (name, project, branch) = self.parent_row_text(ws, cx);
                    let awake = !ws.tabs.is_empty();
                    line = line
                        .child(div().mt(px(6.)).size(px(8.)).flex_shrink_0().rounded_full().bg(hex(if awake {
                            Chrome::GREEN
                        } else {
                            Chrome::MUTED
                        })))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(div().min_w_0().truncate().t_body().text_color(hex(Chrome::BRIGHT)).child(name))
                                        .when(current == Some(ws.id), |d| d.child(icon("check", IconSize::INLINE, hex(Chrome::ACCENT)))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .t_caption()
                                        .text_color(hex(Chrome::MUTED))
                                        .when(!project.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .flex_shrink_0()
                                                    .max_w(px(150.))
                                                    .truncate()
                                                    .px_1p5()
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(hex(Chrome::BORDER))
                                                    .text_color(hex(Chrome::FOREGROUND))
                                                    .child(project),
                                            )
                                        })
                                        .children(branch.map(|b| {
                                            div()
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .child(icon("git-branch", 10., hex(Chrome::MUTED)))
                                                .child(div().min_w_0().truncate().child(b))
                                        })),
                                ),
                        );
                }
            }
            list = list.child(line);
        }
        let header = div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .border_b_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .child(t(cx, "parent.title"))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(crate::theme::EMPHASIS)
                    .text_color(hex(Chrome::BRIGHT))
                    .child(self.workspace_label(target, cx)),
            );
        let search = div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .child(icon("search", IconSize::INLINE, hex(Chrome::MUTED)))
            .child(div().flex_1().min_w_0().child(picker.search.clone()));
        Some(
            div()
                .id("parent-picker-overlay")
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                // Only as tall as its rows: without this the box stretches to the window's foot.
                .items_start()
                .pt(px(96.))
                .bg(hex_alpha(0x000000, 0.35))
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_parent_picker(window, cx)))
                .child(
                    popover()
                        .id("parent-picker")
                        .w(px(440.))
                        .max_w_full()
                        .h_auto()
                        .p_0()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(header)
                        .child(search)
                        .child(list),
                )
                .into_any_element(),
        )
    }

    /// Roughly how tall the workspace menu comes out, to decide which way it opens: its rows,
    /// rules and colour block, and the palette when that is unfolded.
    pub(super) fn workspace_menu_height(&self, id: u64, cx: &gpui::App) -> f32 {
        const COLOUR_BLOCK: f32 = 52.;
        const PALETTE: f32 = 116.;
        let Some(ws) = self.workspaces.iter().find(|w| w.id == id) else { return 0. };
        let palette = if matches!(self.color_palette_open, Some(ColorTarget::Workspace(_))) { PALETTE } else { 0. };
        let optional = [
            self.tree_name(ws, cx).is_some(),
            !self.groups.is_empty(),
            ws.group.is_some(),
            crate::settings::settings(cx).remote.feature,
            !ws.tabs.is_empty(),
            self.deletable_tree(id, cx).is_some(),
        ]
        .iter()
        .filter(|on| **on)
        .count() as f32;
        // header + rename, open in + copy path, pin + unread, new group, parent, delete, plus the
        // rows that come and go, and four rules, inside the popover's padding.
        12. + 24. + ROW + COLOUR_BLOCK + palette + ROW * (2. + 2. + 1. + 1. + 1. + optional) + RULE * 4.
    }

    /// Hovering a menu row unfolds its flyout, or folds away the one that is open.
    fn hover_flyout(flyout: Option<Submenu>, cx: &mut Context<Self>) -> HoverListener {
        Box::new(cx.listener(move |this: &mut Self, hovered: &bool, _, cx| {
            if *hovered && this.workspace_submenu != flyout {
                this.workspace_submenu = flyout;
                cx.notify();
            }
        }))
    }

    pub(super) fn render_workspace_menu(&self, id: u64, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspaces.iter().find(|w| w.id == id);
        let current_group = ws.and_then(|w| w.group);
        let marks = ws.map(|w| w.marks.clone()).unwrap_or_default();
        let awake = ws.is_some_and(|w| !w.tabs.is_empty());
        let tree_name = ws.and_then(|w| self.tree_name(w, cx));
        let deletable = self.deletable_tree(id, cx).is_some();
        let submenu = self.workspace_submenu;
        let hover = Self::hover_flyout;
        let chevron = || icon("chevron-right", IconSize::INLINE, hex(Chrome::MUTED));

        let mut menu =
            popover()
                .id("workspace-menu")
                .w(px(MENU_WIDTH))
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.workspace_menu.take().is_some() {
                        this.note_dismissed("workspace-menu");
                    }
                    cx.notify();
                }))
                .child(div().px_2().pt_1().pb_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "workspace.menu.title")))
                .child(row("wm-rename", "pencil", t(cx, "rename"), false).on_hover(hover(None, cx)).on_click(
                    cx.listener(move |this, _: &ClickEvent, window, cx| this.start_rename(RenameTarget::Workspace(id), window, cx)),
                ))
                .child(div().id("wm-color").on_hover(hover(None, cx)).child(self.render_color_row(
                    "wm",
                    ColorTarget::Workspace(id),
                    ws.and_then(|w| w.color),
                    cx,
                )))
                .child(rule());

        // Open in ›
        let open_in = div()
            .relative()
            .child(
                row("wm-open-in", "folder-open", t(cx, "workspace.open_in"), false)
                    .when(submenu == Some(Submenu::OpenIn), |d| d.bg(hex(Chrome::HOVER)))
                    .on_hover(hover(Some(Submenu::OpenIn), cx))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.workspace_submenu = Some(Submenu::OpenIn);
                        cx.notify();
                    }))
                    .child(chevron()),
            )
            .when(submenu == Some(Submenu::OpenIn), |d| {
                let mut flyout = popover().w(px(FLYOUT_WIDTH));
                for (i, app) in self.open_in_apps.iter().enumerate() {
                    let app = app.clone();
                    let glyph = if app.id == "files" { "folder" } else { "code" };
                    // Acted on as the button goes down: the main menu closes on that same press
                    // (the flyout lies outside its bounds), so a click would never arrive.
                    flyout = flyout.child(row(("wm-open-app", i), glyph, app.name, false).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_workspace_in(id, app.clone(), cx);
                        }),
                    ));
                }
                d.child(div().absolute().top(px(-4.)).left(px(MENU_WIDTH - 8.)).child(flyout))
            });
        menu =
            menu.child(open_in).child(row("wm-copy-path", "copy", t(cx, "workspace.copy_path"), false).on_hover(hover(None, cx)).on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(folder) = this.menu_folder(id, cx) {
                        this.copy_for_workspace(folder.display().to_string(), cx);
                    }
                }),
            ));
        if let Some(name) = tree_name {
            let (key, text) = match name {
                TreeName::Worktree(name) => ("workspace.copy_worktree", name),
                TreeName::Branch(name) => ("workspace.copy_branch", name),
            };
            menu = menu.child(
                row("wm-copy-name", "copy", t(cx, key), false)
                    .on_hover(hover(None, cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.copy_for_workspace(text.clone(), cx))),
            );
        }
        menu = menu
            .child(rule())
            .child(
                row("wm-pin", "pin", t(cx, if marks.pinned { "workspace.unpin" } else { "workspace.pin" }), false)
                    .on_hover(hover(None, cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_pinned(id, cx))),
            )
            .child(
                row(
                    "wm-unread",
                    if marks.unread { "bell" } else { "bell-dot" },
                    t(cx, if marks.unread { "workspace.mark_read" } else { "workspace.mark_unread" }),
                    false,
                )
                .on_hover(hover(None, cx))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_unread(id, cx))),
            )
            .child(rule())
            .child(row("wm-new-group", "folder-plus", t(cx, "new.group"), false).on_hover(hover(None, cx)).on_click(cx.listener(
                move |this, _: &ClickEvent, window, cx| {
                    let gid = this.create_group(window, cx);
                    this.move_to_group(id, Some(gid), cx);
                },
            )));

        // Move to group ›
        if !self.groups.is_empty() {
            let mut moves = div().relative().child(
                row("wm-move-group", "folder", t(cx, "move.to.group"), false)
                    .when(submenu == Some(Submenu::Group), |d| d.bg(hex(Chrome::HOVER)))
                    .on_hover(hover(Some(Submenu::Group), cx))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.workspace_submenu = Some(Submenu::Group);
                        cx.notify();
                    }))
                    .child(chevron()),
            );
            if submenu == Some(Submenu::Group) {
                let mut flyout = popover().w(px(FLYOUT_WIDTH));
                for group in &self.groups {
                    let gid = group.id;
                    let here = current_group == Some(gid);
                    flyout = flyout.child(
                        crate::ui::menu_item_with_dot(
                            SharedString::from(format!("wm-group-{gid}")),
                            super::accent_color(group.color),
                            if here { format!("✓ {}", group.name) } else { group.name.clone() },
                            |_, _, _| {},
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.move_to_group(id, Some(gid), cx);
                            }),
                        ),
                    );
                }
                moves = moves.child(div().absolute().top(px(-4.)).left(px(MENU_WIDTH - 8.)).child(flyout));
            }
            menu = menu.child(moves);
        }
        if current_group.is_some() {
            menu = menu.child(
                row("wm-ungroup", "circle-x", t(cx, "remove.from.group"), false)
                    .on_hover(hover(None, cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.move_to_group(id, None, cx))),
            );
        }
        menu = menu.child(rule()).child(
            row("wm-parent", "list-tree", t(cx, "workspace.set_parent"), false)
                .on_hover(hover(None, cx))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_parent_picker(id, window, cx))),
        );
        if crate::settings::settings(cx).remote.feature {
            let hidden = ws.is_some_and(|w| w.remote_hidden);
            let label = t(cx, "remote.hide_workspace");
            menu = menu.child(
                row("wm-remote-hidden", "eye-off", if hidden { format!("✓ {label}") } else { label.to_string() }, false)
                    .on_hover(hover(None, cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_remote_hidden(id, cx))),
            );
        }
        if awake {
            menu = menu.child(
                row("wm-sleep", "moon", t(cx, "workspace.sleep"), false)
                    .on_hover(hover(None, cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.sleep_workspace(id, window, cx))),
            );
        }
        if deletable {
            menu = menu.child(
                row("wm-delete-tree", "trash-2", t(cx, "workspace.delete_tree"), true)
                    .on_hover(hover(None, cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.request_delete_tree(id, window, cx))),
            );
        }
        menu.child(row("wm-close", "trash-2", t(cx, "close.workspace"), true).on_hover(hover(None, cx)).on_click(cx.listener(
            move |this, _: &ClickEvent, window, cx| {
                this.workspace_menu = None;
                this.request_close(CloseTarget::Workspace(id), window, cx)
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::nest;

    fn parents(list: &[Option<usize>]) -> impl Fn(usize) -> Option<usize> + '_ {
        move |i| list[i]
    }

    #[test]
    fn children_follow_their_parent_one_level_in() {
        // 0, 1 under 0, 2, 3 under 1.
        let p = [None, Some(0), None, Some(1)];
        assert_eq!(nest(&[0, 1, 2, 3], parents(&p)), [(0, 0), (1, 1), (3, 2), (2, 0)]);
    }

    #[test]
    fn a_parent_elsewhere_leaves_the_workspace_at_the_top() {
        // 1's parent (0) is in another group: not among the members.
        let p = [None, Some(0), None];
        assert_eq!(nest(&[1, 2], parents(&p)), [(1, 0), (2, 0)]);
    }

    #[test]
    fn a_loop_of_parents_hides_no_one() {
        let p = [Some(1), Some(0), None];
        let order = nest(&[0, 1, 2], parents(&p));
        assert_eq!(order.len(), 3);
        assert!(order.contains(&(2, 0)));
    }
}
