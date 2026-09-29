//! Branch picker shared by the pane branch chip and AgentGit: search box, local / remote sections,
//! lazy paging while scrolling, a remote search (`git ls-remote`) for branches never fetched, and
//! "New branch". Every place that switches branches uses this, so they behave the same. A right click
//! on a branch opens its actions: rename, delete (on the server too, for a remote branch), a new
//! branch from it, copy its name.

use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, now_ms, relative_time, IconSize, TypeScale};
use agentty_bridge::git::{self, Branch};
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window,
};
use std::path::PathBuf;
use std::time::Duration;

const PAGE: usize = 60;
const ROW_HEIGHT: f32 = 28.;
const LIST_HEIGHT: f32 = 300.;

pub enum BranchPickerEvent {
    /// Switch to (or, in merge mode, merge) this branch; `remote` = a remote-tracking name.
    Pick {
        name: String,
        remote: bool,
    },
    Create(String),
    /// The picker changed the repository itself (a branch renamed, deleted or created from another):
    /// what shows branches reloads.
    Changed,
    Dismiss,
}

/// What the right-clicked branch's actions are waiting on.
enum Pending {
    Rename(gpui::Entity<TextInput>),
    NewFrom(gpui::Entity<TextInput>),
    /// `force`: git refused the plain delete (commits that aren't merged); asked again.
    /// `with_remote`: the branch's counterpart on the server goes too (ticked in the question).
    Delete {
        force: bool,
        with_remote: bool,
    },
    DeleteRemote,
}

/// The actions of one branch: a menu where it was right-clicked.
struct Actions {
    name: String,
    remote: bool,
    current: bool,
    position: gpui::Point<gpui::Pixels>,
    /// For a local branch: the same branch on a remote (`origin/feature`), when there is one.
    counterpart: Option<String>,
    pending: Option<Pending>,
    busy: bool,
    error: Option<String>,
}

#[derive(Clone)]
enum Row {
    Section(&'static str),
    Branch { name: String, remote: bool, current: bool, time: Option<i64> },
    More,
    Searching,
}

pub struct BranchPicker {
    repo: PathBuf,
    current: Option<String>,
    /// `None` while loading.
    branches: Option<Vec<Branch>>,
    /// Remote branches found by name on the servers (not fetched yet).
    remote_hits: Vec<String>,
    searching: bool,
    search_generation: u64,
    limit: usize,
    query: gpui::Entity<TextInput>,
    new_branch: Option<gpui::Entity<TextInput>>,
    actions: Option<Actions>,
    scroll: gpui::UniformListScrollHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<BranchPickerEvent> for BranchPicker {}

impl Focusable for BranchPicker {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BranchPicker {
    pub fn new(repo: PathBuf, current: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextInput::localized("", "git.search_branches", window, cx));
        let subscription = cx.subscribe(&query, |this, _, event: &TextInputEvent, cx| match event {
            TextInputEvent::Changed => this.query_changed(cx),
            TextInputEvent::Confirmed => {
                // Enter picks the first match.
                if let Some(Row::Branch { name, remote, .. }) = this.rows(cx).into_iter().find(|r| matches!(r, Row::Branch { .. })) {
                    cx.emit(BranchPickerEvent::Pick { name, remote });
                }
            }
            TextInputEvent::Cancelled => cx.emit(BranchPickerEvent::Dismiss),
            _ => {}
        });
        window.focus(&query.focus_handle(cx));
        let mut picker = Self {
            repo,
            current,
            branches: None,
            remote_hits: Vec::new(),
            searching: false,
            search_generation: 0,
            limit: PAGE,
            query,
            new_branch: None,
            actions: None,
            scroll: gpui::UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _subscriptions: vec![subscription],
        };
        picker.load(cx);
        picker
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        let task = cx.background_spawn(async move { git::branches(&repo).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let branches = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.current.is_none() {
                    this.current = branches.iter().find(|b| b.current).map(|b| b.name.clone());
                }
                this.branches = Some(branches);
                cx.notify();
            });
        })
        .detach();
    }

    fn query_text(&self, cx: &gpui::App) -> String {
        self.query.read(cx).text().trim().to_lowercase()
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.limit = PAGE;
        self.remote_hits.clear();
        self.search_generation += 1;
        let generation = self.search_generation;
        let query = self.query_text(cx);
        self.searching = query.chars().count() >= 2;
        cx.notify();
        if !self.searching {
            return;
        }
        let repo = self.repo.clone();
        cx.spawn(async move |this, cx| {
            // Debounced: only the last query of a burst of typing reaches the servers.
            cx.background_executor().timer(Duration::from_millis(450)).await;
            if this.read_with(cx, |this, _| this.search_generation != generation).unwrap_or(true) {
                return;
            }
            let found = cx.background_spawn(async move { git::search_remote_branches(&repo, &query).unwrap_or_default() }).await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation == generation {
                    this.remote_hits = found;
                    this.searching = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn rows(&self, cx: &gpui::App) -> Vec<Row> {
        let Some(branches) = &self.branches else { return Vec::new() };
        let query = self.query_text(cx);
        let matches = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
        let locals: Vec<&Branch> = branches.iter().filter(|b| !b.remote && matches(&b.name)).collect();
        let local_names: std::collections::HashSet<&str> = branches.iter().filter(|b| !b.remote).map(|b| b.name.as_str()).collect();
        // Remote branches that already have a local branch are the same thing twice.
        let short = |name: &str| name.split_once('/').map(|(_, n)| n.to_string()).unwrap_or_else(|| name.to_string());
        let remotes: Vec<&Branch> =
            branches.iter().filter(|b| b.remote && matches(&b.name) && !local_names.contains(short(&b.name).as_str())).collect();
        let mut rows = Vec::new();
        let mut shown = 0;
        let push = |row: Row, rows: &mut Vec<Row>, shown: &mut usize| {
            if *shown < self.limit {
                rows.push(row);
                *shown += 1;
            }
        };
        if !locals.is_empty() {
            rows.push(Row::Section("git.local_branches"));
            for b in &locals {
                push(
                    Row::Branch {
                        name: b.name.clone(),
                        remote: false,
                        current: self.current.as_ref() == Some(&b.name),
                        time: Some(b.time),
                    },
                    &mut rows,
                    &mut shown,
                );
            }
        }
        let known: std::collections::HashSet<String> = branches.iter().map(|b| b.name.clone()).collect();
        let extra: Vec<&String> =
            self.remote_hits.iter().filter(|name| !known.contains(*name) && !local_names.contains(short(name).as_str())).collect();
        if !remotes.is_empty() || !extra.is_empty() {
            rows.push(Row::Section("git.remote_branches"));
            for b in &remotes {
                push(Row::Branch { name: b.name.clone(), remote: true, current: false, time: Some(b.time) }, &mut rows, &mut shown);
            }
            for name in extra {
                push(Row::Branch { name: name.clone(), remote: true, current: false, time: None }, &mut rows, &mut shown);
            }
        }
        if locals.len() + remotes.len() > self.limit {
            rows.push(Row::More);
        }
        if self.searching {
            rows.push(Row::Searching);
        }
        rows
    }

    fn render_row(&mut self, index: usize, row: Row, cx: &mut Context<Self>) -> AnyElement {
        match row {
            Row::Section(label) => div()
                .id(("branch-section", index))
                .h(px(ROW_HEIGHT))
                .px_2()
                .flex()
                .items_end()
                .pb_1()
                .t_caption()
                .text_color(hex(Chrome::MUTED))
                .child(t(cx, label))
                .into_any_element(),
            Row::More => {
                // Reaching the end of the page loads the next one.
                let this = cx.entity().downgrade();
                cx.defer(move |cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.limit += PAGE;
                        cx.notify();
                    });
                });
                crate::ui::loading_row(t(cx, "git.loading_branches")).h(px(ROW_HEIGHT)).into_any_element()
            }
            Row::Searching => crate::ui::loading_row(t(cx, "git.searching_remote")).h(px(ROW_HEIGHT)).into_any_element(),
            Row::Branch { name, remote, current, time } => {
                let label = name.clone();
                div()
                    .id(("branch-row", index))
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .t_body()
                    .cursor_pointer()
                    .text_color(hex(Chrome::FOREGROUND))
                    .hover(|s| s.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
                    .child(
                        div()
                            .w(px(IconSize::INLINE))
                            .flex_shrink_0()
                            .when(current, |d| d.child(icon("check", IconSize::INLINE, hex(Chrome::BRIGHT)))),
                    )
                    .child(icon(if remote { "globe" } else { "git-branch" }, 12., hex(Chrome::MUTED)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(div().flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(
                        time.map(|t| relative_time(now_ms(), t as u64 * 1000)).unwrap_or_else(|| t(cx, "git.not_fetched").to_string()),
                    ))
                    // A right click opens the branch's menu (gpui's click is the left button only).
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener({
                            let name = name.clone();
                            move |this, event: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                // A change still running keeps its menu: its answer belongs there.
                                if this.actions.as_ref().is_some_and(|a| a.busy) {
                                    return;
                                }
                                let counterpart = if remote { None } else { this.counterpart_of(&name) };
                                this.actions = Some(Actions {
                                    name: name.clone(),
                                    remote,
                                    current,
                                    position: event.position,
                                    counterpart,
                                    pending: None,
                                    busy: false,
                                    error: None,
                                });
                                cx.notify();
                            }
                        }),
                    )
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        cx.emit(BranchPickerEvent::Pick { name: name.clone(), remote });
                    }))
                    .into_any_element()
            }
        }
    }

    /// Whether the branch menu is open: the menu the picker sits in must not close when it is clicked,
    /// though it may lie outside that menu's bounds.
    pub fn has_popup(&self) -> bool {
        self.actions.is_some()
    }

    /// The remote branch of the same name as the local `name` (`origin` first), from the list loaded.
    fn counterpart_of(&self, name: &str) -> Option<String> {
        let remotes: Vec<&str> = self
            .branches
            .iter()
            .flatten()
            .filter(|b| b.remote && b.name.split_once('/').is_some_and(|(_, short)| short == name))
            .map(|b| b.name.as_str())
            .collect();
        remotes.iter().find(|r| r.starts_with("origin/")).or(remotes.first()).map(|r| r.to_string())
    }

    /// Asks for a name (`rename`: the branch's new name, else a new branch from it), in place.
    fn ask_name(&mut self, rename: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(actions) = self.actions.as_mut() else { return };
        let start = if rename { actions.name.clone() } else { String::new() };
        let input = cx.new(|cx| TextInput::localized(start, if rename { "git.rename_to" } else { "git.new_branch_name" }, window, cx));
        cx.subscribe(&input, |this, input, event: &TextInputEvent, cx| match event {
            TextInputEvent::Confirmed => {
                let name = input.read(cx).text().trim().to_string();
                if !name.is_empty() {
                    this.confirm_pending(Some(name), cx);
                }
            }
            TextInputEvent::Cancelled => {
                if let Some(actions) = this.actions.as_mut() {
                    actions.pending = None;
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();
        window.focus(&input.focus_handle(cx));
        actions.error = None;
        actions.pending = Some(if rename { Pending::Rename(input) } else { Pending::NewFrom(input) });
        cx.notify();
    }

    /// Runs what the actions panel waits on (`name`: the name typed for a rename or new branch).
    fn confirm_pending(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        let Some(actions) = self.actions.as_mut().filter(|a| !a.busy) else { return };
        let Some(pending) = actions.pending.as_ref() else { return };
        let (repo, branch, remote) = (self.repo.clone(), actions.name.clone(), actions.remote);
        // `Ok(Some(error))`: the local branch went, its copy on the server did not.
        type Op = Box<dyn FnOnce() -> anyhow::Result<Option<String>> + Send>;
        let op: Op = match (pending, name) {
            // The same name again: nothing to do, the menu just closes.
            (Pending::Rename(_), Some(to)) if to == branch => {
                self.actions = None;
                return cx.notify();
            }
            (Pending::Rename(_), Some(to)) => Box::new(move || git::rename_branch(&repo, &branch, &to).map(|()| None)),
            (Pending::NewFrom(_), Some(to)) => Box::new(move || git::create_branch_from(&repo, &to, &branch).map(|()| None)),
            (Pending::Delete { force, with_remote }, _) => {
                let force = *force;
                // The server's copy only goes once the local branch did (an unmerged one stays whole).
                let remote_copy = actions.counterpart.clone().filter(|_| *with_remote);
                Box::new(move || {
                    git::delete_branch(&repo, &branch, force)?;
                    Ok(remote_copy.and_then(|remote_copy| git::delete_remote_branch(&repo, &remote_copy).err().map(|err| err.to_string())))
                })
            }
            (Pending::DeleteRemote, _) if remote => Box::new(move || git::delete_remote_branch(&repo, &branch).map(|()| None)),
            _ => return,
        };
        let deleting = matches!(pending, Pending::Delete { force: false, .. });
        let (acted_on, counterpart) = (actions.name.clone(), actions.counterpart.clone());
        let with_remote = matches!(pending, Pending::Delete { with_remote: true, .. });
        actions.busy = true;
        actions.error = None;
        cx.notify();
        let task = cx.background_spawn(async move { op() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                // The menu is still about the branch this ran for (a new one can't open meanwhile).
                if this.actions.as_ref().is_some_and(|a| a.name != acted_on) {
                    return;
                }
                match result {
                    Ok(None) => {
                        this.actions = None;
                        this.current = None;
                        this.load(cx);
                        cx.emit(BranchPickerEvent::Changed);
                    }
                    // The local branch went; its copy on the server did not: the list says so, and
                    // the menu offers that deletion again (only that one).
                    Ok(Some(error)) => {
                        this.current = None;
                        this.load(cx);
                        cx.emit(BranchPickerEvent::Changed);
                        if let (Some(actions), Some(counterpart)) = (this.actions.as_mut(), counterpart) {
                            actions.name = counterpart;
                            actions.remote = true;
                            actions.counterpart = None;
                            actions.busy = false;
                            actions.pending = Some(Pending::DeleteRemote);
                            actions.error = Some(git::failure_reason(&error));
                        }
                    }
                    Err(err) => {
                        let error = err.to_string();
                        let Some(actions) = this.actions.as_mut() else { return };
                        actions.busy = false;
                        // Not merged: say so and ask for the forced delete.
                        if deleting && error.contains("not fully merged") {
                            actions.pending = Some(Pending::Delete { force: true, with_remote });
                            actions.error = Some(t(cx, "git.branch_unmerged").to_string());
                        } else if let Some(tree) = git::checked_out_elsewhere(&error) {
                            actions.pending = None;
                            actions.error = Some(tf(cx, "git.branch_in_tree", &[("folder", &crate::ui::tilde(&tree))]));
                        } else if error == "invalid branch name" {
                            actions.error = Some(t(cx, "git.invalid_branch_name").to_string());
                        } else {
                            actions.error = Some(git::failure_reason(&error));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The right-clicked branch's menu, where it was clicked: its actions, then — for a rename, a new
    /// branch or a delete — the question in the same menu.
    /// The menu the picker sits in draws it beside itself, in a deferred layer above its own: inside
    /// that menu, its border and footer would be drawn over it (and gpui can't defer inside a
    /// deferred layer).
    pub fn render_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let actions = self.actions.as_ref()?;
        let separator = || div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER));
        let danger = |item: gpui::Stateful<gpui::Div>| item.text_color(hex(Chrome::ERROR));
        let mut menu = crate::ui::popover()
            .w(px(260.))
            .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                if this.actions.as_ref().is_some_and(|a| !a.busy) {
                    this.actions = None;
                    cx.notify();
                }
            }))
            .child(div().px_3().pt_1().pb_1().t_small().text_color(hex(Chrome::MUTED)).truncate().child(actions.name.clone()));
        match &actions.pending {
            Some(Pending::Rename(input) | Pending::NewFrom(input)) => {
                let hint = match actions.pending {
                    Some(Pending::Rename(_)) => tf(cx, "git.rename_branch_hint", &[("branch", &actions.name)]),
                    _ => tf(cx, "git.new_branch_from", &[("branch", &actions.name)]),
                };
                menu = menu
                    .child(div().px_3().pb_1().t_small().text_color(hex(Chrome::FOREGROUND)).child(hint))
                    .child(div().px_2().pb_1().child(field(input.clone(), true)));
            }
            Some(Pending::Delete { force, with_remote }) => {
                let key = if *force { "git.delete_force_ask" } else { "git.delete_branch_ask" };
                menu = menu.child(div().px_3().pb_1().t_small().text_color(hex(Chrome::FOREGROUND)).child(tf(
                    cx,
                    key,
                    &[("branch", &actions.name)],
                )));
                // Pushed: its copy on the server can go in the same step (off unless ticked).
                if let Some(counterpart) = &actions.counterpart {
                    let ticked = *with_remote;
                    menu = menu.child(
                        div()
                            .id("branch-delete-remote-tick")
                            .mx_1()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap_2()
                            .t_small()
                            .cursor_pointer()
                            .text_color(hex(Chrome::FOREGROUND))
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            // The tick box the other dialogs of the app draw.
                            .child(
                                div()
                                    .size(px(14.))
                                    .flex_shrink_0()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(hex(if ticked { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                                    .bg(if ticked { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(ticked, |d| d.child(icon("check", 11., hex(Chrome::BRIGHT)))),
                            )
                            .child(div().min_w_0().child(tf(cx, "git.delete_with_remote", &[("remote", counterpart)])))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                if let Some(Pending::Delete { with_remote, .. }) = this.actions.as_mut().and_then(|a| a.pending.as_mut()) {
                                    *with_remote = !*with_remote;
                                }
                                cx.notify();
                            })),
                    );
                }
                let confirm = if *force { "git.delete_force" } else { "git.delete_branch" };
                menu = menu.child(separator()).child(danger(crate::ui::menu_item(
                    "branch-action-confirm",
                    t(cx, confirm),
                    cx.listener(|this, _: &ClickEvent, _, cx| this.confirm_pending(None, cx)),
                )));
            }
            Some(Pending::DeleteRemote) => {
                menu = menu
                    .child(div().px_3().pb_1().t_small().text_color(hex(Chrome::FOREGROUND)).child(tf(
                        cx,
                        "git.delete_remote_ask",
                        &[("branch", &actions.name)],
                    )))
                    .child(separator())
                    .child(danger(crate::ui::menu_item(
                        "branch-action-confirm",
                        t(cx, "git.delete_remote"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.confirm_pending(None, cx)),
                    )));
            }
            None => {
                let (name, remote, current) = (actions.name.clone(), actions.remote, actions.current);
                if !current {
                    let pick = name.clone();
                    menu = menu.child(crate::ui::menu_item(
                        "branch-action-switch",
                        t(cx, "git.switch_to"),
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.actions = None;
                            cx.emit(BranchPickerEvent::Pick { name: pick.clone(), remote });
                        }),
                    ));
                }
                if !remote {
                    menu = menu.child(crate::ui::menu_item(
                        "branch-action-rename",
                        t(cx, "git.rename_branch"),
                        cx.listener(|this, _: &ClickEvent, window, cx| this.ask_name(true, window, cx)),
                    ));
                }
                menu = menu
                    .child(crate::ui::menu_item(
                        "branch-action-new",
                        t(cx, "git.new_branch_here"),
                        cx.listener(|this, _: &ClickEvent, window, cx| this.ask_name(false, window, cx)),
                    ))
                    .child(crate::ui::menu_item(
                        "branch-action-copy",
                        t(cx, "git.copy_branch_name"),
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(branch_label(&name, remote).to_string()));
                            this.actions = None;
                            cx.notify();
                        }),
                    ));
                if !current {
                    let (id, label) = if remote {
                        ("branch-action-delete-remote", "git.delete_remote")
                    } else {
                        ("branch-action-delete", "git.delete_branch")
                    };
                    menu = menu.child(separator()).child(danger(crate::ui::menu_item(
                        id,
                        t(cx, label),
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if let Some(actions) = this.actions.as_mut() {
                                actions.error = None;
                                actions.pending =
                                    Some(if remote { Pending::DeleteRemote } else { Pending::Delete { force: false, with_remote: false } });
                            }
                            cx.notify();
                        }),
                    )));
                }
            }
        }
        if actions.busy {
            menu = menu.child(crate::ui::loading_row(t(cx, "git.working")));
        }
        if let Some(error) = &actions.error {
            menu = menu.child(div().px_3().py_1().t_small().text_color(hex(Chrome::ERROR)).child(error.clone()));
        }
        // Where the click was, over the menu the picker sits in.
        Some(
            gpui::deferred(gpui::anchored().position(actions.position).snap_to_window_with_margin(px(8.)).child(menu))
                .with_priority(5)
                .into_any_element(),
        )
    }

    fn open_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::localized(self.query.read(cx).text().to_string(), "git.new_branch_name", window, cx));
        cx.subscribe(&input, |this, input, event: &TextInputEvent, cx| match event {
            TextInputEvent::Confirmed => {
                let name = input.read(cx).text().trim().to_string();
                if !name.is_empty() {
                    cx.emit(BranchPickerEvent::Create(name));
                }
            }
            TextInputEvent::Cancelled => {
                this.new_branch = None;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        window.focus(&input.focus_handle(cx));
        self.new_branch = Some(input);
        cx.notify();
    }
}

fn field(input: gpui::Entity<TextInput>, accent: bool) -> gpui::Div {
    let focus = input.clone();
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(hex(if accent { Chrome::ACCENT } else { Chrome::BORDER }))
        .bg(hex(0x1a1a1a))
        .t_body()
        .text_color(hex(Chrome::BRIGHT))
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| window.focus(&focus.focus_handle(cx)))
        .child(input)
}

impl Render for BranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let count = rows.len();
        let body: AnyElement = match &self.branches {
            None => crate::ui::loading_row(t(cx, "git.loading_branches")).into_any_element(),
            Some(_) if count == 0 => {
                crate::ui::hint(if self.query_text(cx).is_empty() { t(cx, "git.no_branches") } else { t(cx, "git.no_match") })
                    .into_any_element()
            }
            Some(_) => {
                let height = (count as f32 * ROW_HEIGHT).min(LIST_HEIGHT);
                div()
                    .relative()
                    .h(px(height))
                    .child(
                        gpui::uniform_list(
                            "branch-list",
                            count,
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                let rows = this.rows(cx);
                                range
                                    .filter_map(|i| rows.get(i).cloned().map(|row| (i, row)))
                                    .map(|(i, row)| this.render_row(i, row, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(self.scroll.clone())
                        .size_full(),
                    )
                    .into_any_element()
            }
        };
        let footer: AnyElement = match &self.new_branch {
            Some(input) => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().min_w_0().truncate().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "git.new_branch_from",
                    &[("branch", self.current.as_deref().unwrap_or("HEAD"))],
                )))
                .child(field(input.clone(), true))
                .into_any_element(),
            None => div()
                .id("branch-picker-new")
                .h(px(ROW_HEIGHT))
                .px_2()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .t_body()
                .cursor_pointer()
                .text_color(hex(Chrome::FOREGROUND))
                .hover(|s| s.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
                .child(icon("plus", IconSize::INLINE, hex(Chrome::MUTED)))
                .child(t(cx, "git.new_branch"))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_new_branch(window, cx);
                }))
                .into_any_element(),
        };
        div()
            .id("branch-picker")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_1()
            .child(div().px_1().pt_1().child(field(self.query.clone(), false).child(icon(
                "search",
                IconSize::INLINE,
                hex_alpha(Chrome::FOREGROUND, 0.5),
            ))))
            .child(div().px_1().child(body))
            .child(div().mx_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(div().px_1().pb_1().child(footer))
    }
}

pub fn branch_label(name: &str, remote: bool) -> SharedString {
    if remote {
        name.split_once('/').map(|(_, n)| n.to_string()).unwrap_or_else(|| name.to_string()).into()
    } else {
        name.to_string().into()
    }
}
