//! AgentGit: the Git page, modeled on GitHub Desktop: repository / branch / fetch toolbar, a Changes tab (file
//! list with checkboxes, diff, commit box) and a History tab (commits, files, diff).
//! All git work runs on background threads through `agentty_bridge::git`.

use crate::i18n::{t, tf};
use crate::settings::settings;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, now_ms, popover, relative_time, tilde, IconSize, TypeScale};
use agentty_bridge::git::{self, Branch, Commit, CommitFile, DiffLine, FileChange, LineKind, RepoStatus};
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, Div, Entity, EventEmitter, FontWeight, SharedString, Subscription, Window,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

const SIDEBAR_WIDTH: f32 = 300.;
const DIFF_ROW: f32 = 20.;
const FILE_ROW: f32 = 28.;
const COMMIT_ROW: f32 = 50.;
const MAX_DIFF_LINES: usize = 20_000;
const HISTORY_LIMIT: usize = 300;

gpui::actions!(git, [CommitChanges, RemoteAction, Fetch, Refresh]);

pub enum GitEvent {
    /// Open a terminal tab in the repository.
    OpenTerminal(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Changes,
    History,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Repo,
    Branch,
}

#[derive(Default)]
struct Snapshot {
    status: RepoStatus,
    branches: Vec<Branch>,
    commits: Vec<Commit>,
    last_fetch: Option<i64>,
    has_remote: bool,
    web_url: Option<String>,
}

fn load_snapshot(repo: &std::path::Path) -> Result<Snapshot, String> {
    let status = git::status(repo).map_err(|e| e.to_string())?;
    Ok(Snapshot {
        status,
        branches: git::branches(repo).unwrap_or_default(),
        commits: git::log(repo, HISTORY_LIMIT).unwrap_or_default(),
        last_fetch: git::last_fetch(repo),
        has_remote: git::has_remote(repo),
        web_url: git::remote_web_url(repo),
    })
}

pub struct GitView {
    focus_handle: gpui::FocusHandle,
    cwds: Vec<PathBuf>,
    repos: Vec<PathBuf>,
    repo: Option<PathBuf>,
    snapshot: Snapshot,
    loaded: bool,
    loading: bool,
    refreshed: Option<Instant>,
    visible: bool,
    tab: Tab,
    menu: Option<Menu>,
    /// Paths the user unchecked (everything else is included in the commit, like GitHub Desktop).
    excluded: HashSet<String>,
    selected_file: Option<String>,
    confirm_discard: Option<String>,
    diff: Rc<Vec<DiffLine>>,
    selected_commit: Option<String>,
    commit_files: Vec<CommitFile>,
    commit_file: Option<String>,
    merge_mode: bool,
    creating_branch: bool,
    busy: Option<&'static str>,
    message: Option<(String, bool)>,
    filter: Entity<TextInput>,
    summary: Entity<TextInput>,
    description: Entity<TextInput>,
    branch_picker: Option<(Entity<crate::branch_picker::BranchPicker>, Subscription)>,
    /// Workspace the repository list was built for (switching workspaces re-picks the repository).
    workspace: Option<u64>,
    files_scroll: gpui::UniformListScrollHandle,
    commits_scroll: gpui::UniformListScrollHandle,
    diff_scroll: gpui::UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GitEvent> for GitView {}

impl GitView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |key: &'static str, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| TextInput::localized("", key, window, cx));
        let filter = input("git.filter", window, cx);
        let summary = input("git.summary", window, cx);
        let description = input("git.description", window, cx);
        let redraw = |_: &mut Self, _: Entity<TextInput>, event: &TextInputEvent, cx: &mut Context<Self>| {
            if matches!(event, TextInputEvent::Changed) {
                cx.notify();
            }
        };
        let subscriptions = vec![
            cx.subscribe(&filter, redraw),
            cx.subscribe(&summary, |this, _, event: &TextInputEvent, cx| match event {
                TextInputEvent::Changed => cx.notify(),
                TextInputEvent::Confirmed => this.commit(cx),
                _ => {}
            }),
            cx.subscribe(&description, |this, _, event: &TextInputEvent, cx| {
                if matches!(event, TextInputEvent::Confirmed) {
                    this.commit(cx);
                }
            }),
        ];
        // Poll while the page is on screen so edits made in terminals show up.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let Ok(()) = this.update(cx, |this, cx| {
                if this.visible {
                    this.refresh(cx);
                }
            }) else {
                break;
            };
        })
        .detach();
        Self {
            focus_handle: cx.focus_handle(),
            cwds: Vec::new(),
            repos: Vec::new(),
            repo: None,
            snapshot: Snapshot::default(),
            loaded: false,
            loading: false,
            refreshed: None,
            visible: false,
            tab: Tab::Changes,
            menu: None,
            excluded: HashSet::new(),
            selected_file: None,
            confirm_discard: None,
            diff: Rc::default(),
            selected_commit: None,
            commit_files: Vec::new(),
            commit_file: None,
            merge_mode: false,
            creating_branch: false,
            busy: None,
            message: None,
            filter,
            summary,
            description,
            branch_picker: None,
            workspace: None,
            files_scroll: gpui::UniformListScrollHandle::new(),
            commits_scroll: gpui::UniformListScrollHandle::new(),
            diff_scroll: gpui::UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Debug harness hook (`AGENTTY_DEBUG=1`): `tab:history`, `commit:<summary>`, `checkout:<name>`,
    /// `branch:<name>`, `menu:branch`, `exclude:<path>`; empty argument just dumps.
    pub fn debug(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> String {
        match command.split_once(':') {
            Some(("tab", "history")) => self.set_tab(Tab::History, cx),
            Some(("tab", _)) => self.set_tab(Tab::Changes, cx),
            Some(("commit", summary)) => {
                self.summary.update(cx, |i, cx| i.set_text(summary.to_string(), cx));
                self.commit(cx);
            }
            Some(("checkout", name)) => {
                let name = name.to_string();
                self.run("git.switching", cx, move |repo| git::checkout(repo, &name, false));
            }
            Some(("branch", name)) => {
                let name = name.to_string();
                self.run("git.creating", cx, move |repo| git::create_branch(repo, &name));
            }
            Some(("repo", index)) => {
                let repo = index.parse::<usize>().ok().and_then(|i| self.repos.get(i).cloned());
                self.select_repo(repo, cx);
            }
            Some(("remote", _)) => self.remote_action(cx),
            Some(("merge", name)) => {
                let name = name.to_string();
                self.run("git.merging", cx, move |repo| git::merge(repo, &name));
            }
            Some(("discard", path)) => {
                if let Some(file) = self.file(path).cloned() {
                    self.run("git.discarding", cx, move |repo| git::discard(repo, &file));
                }
            }
            Some(("menu", "branch")) => self.toggle_menu(Menu::Branch, window, cx),
            Some(("menu", _)) => self.toggle_menu(Menu::Repo, window, cx),
            Some(("exclude", path)) => {
                self.excluded.insert(path.to_string());
            }
            _ => {}
        }
        cx.notify();
        let s = &self.snapshot.status;
        format!(
            "repo={:?} repos={} branch={:?} upstream={:?} +{} -{} files={:?} excluded={:?} selected={:?} diff_lines={} commits={} selected_commit={:?} commit_files={} branches={:?} busy={:?} message={:?}",
            self.repo,
            self.repos.len(),
            s.branch,
            s.upstream,
            s.ahead,
            s.behind,
            s.files.iter().map(|f| format!("{}{}", f.kind, f.path)).collect::<Vec<_>>(),
            self.excluded,
            self.selected_file,
            self.diff.len(),
            self.snapshot.commits.len(),
            self.selected_commit.as_ref().map(|c| &c[..7]),
            self.commit_files.len(),
            self.snapshot.branches.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            self.busy,
            self.message,
        )
    }

    /// Pauses background polling while another page or the terminals are shown.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Called whenever the page is shown with the cwds of open panes (active pane first).
    pub fn set_cwds(&mut self, workspace: Option<u64>, cwds: Vec<PathBuf>, cx: &mut Context<Self>) {
        // Newly shown or another workspace selected: follow the active pane's repository, like
        // opening GitHub Desktop from a folder.
        let switched = workspace != self.workspace;
        self.workspace = workspace;
        let follow_active = !self.visible || switched;
        self.visible = true;
        if follow_active || cwds != self.cwds {
            self.cwds = cwds.clone();
            let task = cx.background_spawn(async move {
                let mut roots: Vec<PathBuf> = Vec::new();
                let mut active = None;
                for (index, cwd) in cwds.iter().enumerate() {
                    if let Some(root) = git::repo_root(cwd) {
                        if index == 0 {
                            active = Some(root.clone());
                        }
                        if !roots.contains(&root) {
                            roots.push(root);
                        }
                    }
                }
                (roots, active)
            });
            cx.spawn(async move |this, cx| {
                let (roots, active) = task.await;
                let _ = this.update(cx, |this, cx| {
                    let keep = this.repo.as_ref().is_some_and(|r| roots.contains(r));
                    this.repos = roots;
                    if follow_active && active.is_some() {
                        this.select_repo(active, cx);
                    } else if !keep || follow_active {
                        let first = this.repos.first().cloned();
                        this.select_repo(first, cx);
                    }
                    // Several repositories open in this workspace: let the user pick.
                    if switched && this.repos.len() > 1 {
                        this.menu = Some(Menu::Repo);
                    }
                    this.loaded = true;
                    cx.notify();
                });
            })
            .detach();
        } else if self.refreshed.is_none_or(|r| r.elapsed() > Duration::from_secs(2)) {
            self.refresh(cx);
        }
    }

    fn select_repo(&mut self, repo: Option<PathBuf>, cx: &mut Context<Self>) {
        if repo != self.repo {
            self.repo = repo;
            self.snapshot = Snapshot::default();
            self.excluded.clear();
            self.selected_file = None;
            self.selected_commit = None;
            self.commit_files.clear();
            self.commit_file = None;
            self.diff = Rc::default();
            self.message = None;
        }
        self.menu = None;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else { return };
        if self.loading {
            return;
        }
        self.loading = true;
        let task = cx.background_spawn(async move { load_snapshot(&repo) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.refreshed = Some(Instant::now());
                match result {
                    Ok(snapshot) => this.apply(snapshot, cx),
                    Err(err) => this.message = Some((err, true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        let paths: HashSet<String> = snapshot.status.files.iter().map(|f| f.path.clone()).collect();
        self.excluded.retain(|p| paths.contains(p));
        let changed = snapshot.status.files != self.snapshot.status.files;
        self.snapshot = snapshot;
        // The periodic refresh only touches the working-tree diff while it is on screen; in History
        // it would replace the commit diff being read.
        if self.selected_file.as_deref().is_none_or(|p| !paths.contains(p)) {
            self.selected_file = self.snapshot.status.files.first().map(|f| f.path.clone());
            if self.tab == Tab::Changes {
                self.load_working_diff(cx);
            }
        } else if changed && self.tab == Tab::Changes {
            self.load_working_diff(cx);
        }
        if self.tab == Tab::History && self.selected_commit.is_none() {
            if let Some(first) = self.snapshot.commits.first().map(|c| c.sha.clone()) {
                self.select_commit(first, cx);
            }
        }
    }

    fn load_working_diff(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(file)) = (self.repo.clone(), self.selected_file.as_ref().and_then(|p| self.file(p)).cloned()) else {
            self.diff = Rc::default();
            return;
        };
        let task = cx.background_spawn(async move { git::working_diff(&repo, &file).map(|d| (file.path, d)) });
        self.finish_diff(task, cx);
    }

    fn finish_diff(&mut self, task: gpui::Task<anyhow::Result<(String, Vec<DiffLine>)>>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((path, mut lines)) => {
                        let current = match this.tab {
                            Tab::Changes => this.selected_file.as_deref(),
                            Tab::History => this.commit_file.as_deref(),
                        };
                        if current != Some(path.as_str()) {
                            return;
                        }
                        lines.truncate(MAX_DIFF_LINES);
                        this.diff = Rc::new(lines);
                    }
                    Err(err) => {
                        this.diff = Rc::new(vec![DiffLine { kind: LineKind::Meta, old: None, new: None, text: err.to_string() }]);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn file(&self, path: &str) -> Option<&FileChange> {
        self.snapshot.status.files.iter().find(|f| f.path == path)
    }

    fn select_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.selected_file = Some(path);
        self.confirm_discard = None;
        self.diff_scroll = gpui::UniformListScrollHandle::new();
        self.load_working_diff(cx);
        cx.notify();
    }

    fn select_commit(&mut self, sha: String, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else { return };
        self.selected_commit = Some(sha.clone());
        self.commit_files.clear();
        self.commit_file = None;
        self.diff = Rc::default();
        let task = cx.background_spawn(async move { git::commit_files(&repo, &sha) });
        cx.spawn(async move |this, cx| {
            let files = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                let first = files.first().map(|f| f.path.clone());
                this.commit_files = files;
                if let Some(first) = first {
                    this.select_commit_file(first, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select_commit_file(&mut self, path: String, cx: &mut Context<Self>) {
        let (Some(repo), Some(sha)) = (self.repo.clone(), self.selected_commit.clone()) else { return };
        self.commit_file = Some(path.clone());
        self.diff_scroll = gpui::UniformListScrollHandle::new();
        let task = cx.background_spawn(async move { git::commit_diff(&repo, &sha, &path).map(|d| (path, d)) });
        self.finish_diff(task, cx);
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.diff = Rc::default();
        match tab {
            Tab::Changes => self.load_working_diff(cx),
            Tab::History => {
                if let Some(sha) = self.selected_commit.clone().or_else(|| self.snapshot.commits.first().map(|c| c.sha.clone())) {
                    self.select_commit(sha, cx);
                }
            }
        }
        cx.notify();
    }

    /// Runs a mutating git operation in the background, then refreshes.
    fn run(
        &mut self,
        label: &'static str,
        cx: &mut Context<Self>,
        op: impl FnOnce(&std::path::Path) -> anyhow::Result<()> + Send + 'static,
    ) {
        self.run_reporting(label, cx, |repo| op(repo).map(|()| None))
    }

    /// Like [`Self::run`], but the operation can report what it did ("3 commits · 12 files").
    fn run_reporting(
        &mut self,
        label: &'static str,
        cx: &mut Context<Self>,
        op: impl FnOnce(&std::path::Path) -> anyhow::Result<Option<String>> + Send + 'static,
    ) {
        let Some(repo) = self.repo.clone() else { return };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label);
        self.message = None;
        let task = cx.background_spawn(async move { op(&repo) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(Some(report)) => this.message = Some((report, false)),
                    Ok(None) => {}
                    Err(err) => this.message = Some((agentty_bridge::git::failure_reason(&err.to_string()), true)),
                }
                this.selected_commit = None;
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn included_paths(&self) -> Vec<String> {
        self.snapshot
            .status
            .files
            .iter()
            .filter(|f| !self.excluded.contains(&f.path))
            .flat_map(|f| std::iter::once(f.path.clone()).chain(f.original.clone()))
            .collect()
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let summary = self.summary.read(cx).text().trim().to_string();
        let description = self.description.read(cx).text().to_string();
        let paths = self.included_paths();
        if summary.is_empty() || paths.is_empty() || self.busy.is_some() {
            return;
        }
        let Some(repo) = self.repo.clone() else { return };
        self.busy = Some("git.committing");
        self.message = None;
        let task = cx.background_spawn(async move { git::commit(&repo, &summary, &description, &paths) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(sha) => {
                        let text = tf(cx, "git.committed", &[("sha", &sha)]);
                        this.message = Some((text.clone(), false));
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_secs(4)).await;
                            let _ = this.update(cx, |this, cx| {
                                if this.message.as_ref().is_some_and(|(m, _)| *m == text) {
                                    this.message = None;
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                        this.summary.update(cx, |i, cx| i.set_text("", cx));
                        this.description.update(cx, |i, cx| i.set_text("", cx));
                        this.excluded.clear();
                    }
                    Err(err) => this.message = Some((err.to_string(), true)),
                }
                this.selected_commit = None;
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// `git pull --ff-only`, reporting how many commits and files arrived.
    fn pull(&mut self, cx: &mut Context<Self>) {
        let language = crate::settings::settings(cx).language;
        self.run_reporting("git.pulling", cx, move |repo| {
            let pulled = git::pull(repo)?;
            if pulled.files() == 0 {
                return Ok(Some(crate::i18n::tr(language, "branch.pulled").to_string()));
            }
            let mut text = crate::i18n::tr(language, "branch.pulled_changes").to_string();
            for (name, value) in [
                ("commits", pulled.commits),
                ("files", pulled.files()),
                ("added", pulled.added),
                ("modified", pulled.modified),
                ("deleted", pulled.deleted),
            ] {
                text = text.replace(&format!("{{{name}}}"), &value.to_string());
            }
            Ok(Some(text))
        });
    }

    /// Primary remote action: publish, pull, push or fetch depending on the branch state.
    fn remote_action(&mut self, cx: &mut Context<Self>) {
        let status = &self.snapshot.status;
        let Some(branch) = status.branch.clone() else { return };
        if status.upstream.is_none() {
            self.run("git.publishing", cx, move |repo| git::push(repo, &branch, false));
        } else if status.behind > 0 {
            self.pull(cx);
        } else if status.ahead > 0 {
            self.run("git.pushing", cx, move |repo| git::push(repo, &branch, true));
        } else {
            self.run("git.fetching", cx, git::fetch);
        }
    }

    fn toggle_menu(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(menu) { None } else { Some(menu) };
        self.merge_mode = false;
        self.creating_branch = false;
        self.branch_picker = None;
        if let (Some(Menu::Branch), Some(repo)) = (self.menu, self.repo.clone()) {
            let current = self.snapshot.status.branch.clone();
            let picker = cx.new(|cx| crate::branch_picker::BranchPicker::new(repo, current, window, cx));
            let subscription = cx.subscribe(&picker, |this, _, event: &crate::branch_picker::BranchPickerEvent, cx| {
                use crate::branch_picker::BranchPickerEvent;
                let current = this.snapshot.status.branch.clone();
                match event {
                    BranchPickerEvent::Pick { name, remote } => {
                        let (name, remote) = (name.clone(), *remote);
                        this.menu = None;
                        if this.merge_mode {
                            if current.as_ref() != Some(&name) {
                                this.run("git.merging", cx, move |repo| git::merge(repo, &name));
                            }
                        } else if remote || current.as_ref() != Some(&name) {
                            this.run("git.switching", cx, move |repo| git::checkout(repo, &name, remote));
                        }
                    }
                    BranchPickerEvent::Create(name) => {
                        let name = name.clone();
                        this.menu = None;
                        this.run("git.creating", cx, move |repo| git::create_branch(repo, &name));
                    }
                    BranchPickerEvent::Dismiss => this.menu = None,
                }
                cx.notify();
            });
            self.branch_picker = Some((picker, subscription));
        }
        cx.notify();
    }
}

fn mono(cx: &gpui::App) -> SharedString {
    settings(cx).font_family.clone().into()
}

fn kind_badge(kind: char) -> Div {
    let (letter, color) = match kind {
        'A' | '?' => ("+", Chrome::SUCCESS),
        'D' => ("−", Chrome::ERROR),
        'R' | 'C' => ("→", Chrome::BLUE),
        'U' => ("!", Chrome::ERROR),
        _ => ("•", Chrome::ORANGE),
    };
    div()
        .flex_shrink_0()
        .size(px(14.))
        .rounded_sm()
        .border_1()
        .border_color(hex(color))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD)
        .text_color(hex(color))
        .child(letter)
}

fn checkbox(checked: bool, mixed: bool) -> Div {
    let on = checked || mixed;
    div()
        .flex_shrink_0()
        .size(px(14.))
        .rounded_sm()
        .border_1()
        .border_color(hex(if on { Chrome::ACCENT } else { 0x6b6b6b }))
        .bg(if on { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
        .flex()
        .items_center()
        .justify_center()
        .when(checked, |d| d.child(icon("check", 12., hex(Chrome::BRIGHT))))
        .when(mixed && !checked, |d| d.child(div().w(px(8.)).h(px(2.)).bg(hex(Chrome::BRIGHT))))
}

fn split_path(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (format!("{dir}/"), name.to_string()),
        None => (String::new(), path.to_string()),
    }
}

fn path_label(path: &str) -> Div {
    let (dir, name) = split_path(path);
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .overflow_hidden()
        .whitespace_nowrap()
        .child(div().flex_shrink().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(dir))
        .child(div().flex_shrink_0().text_color(hex(Chrome::FOREGROUND)).child(name))
}

fn field(input: &Entity<TextInput>) -> Div {
    div()
        .w_full()
        .px_2()
        .py_1p5()
        .rounded_md()
        .border_1()
        .border_color(hex(Chrome::BORDER))
        .bg(hex(0x1a1a1a))
        .t_body()
        .text_color(hex(Chrome::BRIGHT))
        .child(input.clone())
}

impl GitView {
    fn render_toolbar(&self, cx: &mut Context<Self>) -> Div {
        let status = &self.snapshot.status;
        let repo_name =
            self.repo.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "—".into());
        let branch = status.branch.clone().unwrap_or_else(|| t(cx, "git.detached").into());
        let (remote_icon, remote_title, remote_count) = if status.branch.is_none() || !self.snapshot.has_remote {
            ("refresh-cw", t(cx, "git.fetch"), None)
        } else if status.upstream.is_none() {
            ("arrow-up", t(cx, "git.publish"), None)
        } else if status.behind > 0 {
            ("arrow-down", t(cx, "git.pull"), Some(status.behind))
        } else if status.ahead > 0 {
            ("arrow-up", t(cx, "git.push"), Some(status.ahead))
        } else {
            ("refresh-cw", t(cx, "git.fetch"), None)
        };
        let remote_sub = match (self.busy, self.snapshot.last_fetch) {
            (Some(label), _) => t(cx, label).to_string(),
            _ if !self.snapshot.has_remote => t(cx, "git.no_remote").to_string(),
            (None, Some(at)) => tf(cx, "git.last_fetched", &[("time", &relative_time(now_ms(), at as u64 * 1000))]),
            (None, None) => t(cx, "git.never_fetched").to_string(),
        };
        let has_remote = self.snapshot.has_remote && self.repo.is_some();

        let segment = |id: &'static str, icon_name: &'static str, title: String, value: String, open: bool, chevron: bool| {
            div()
                .id(id)
                .h_full()
                .w(px(SIDEBAR_WIDTH))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .border_r_1()
                .border_color(hex(Chrome::BORDER))
                .cursor_pointer()
                .when(open, |d| d.bg(hex(Chrome::SELECTED)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(icon(icon_name, IconSize::BUTTON, hex(Chrome::MUTED)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(title))
                        .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).truncate().child(value)),
                )
                .when(chevron, |d| d.child(icon("chevron-down", IconSize::INLINE, hex(Chrome::MUTED))))
        };

        div()
            .h(px(52.))
            .flex_shrink_0()
            .flex()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .child(
                segment("git-repo", "folder", t(cx, "git.current_repo").into(), repo_name, self.menu == Some(Menu::Repo), true)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_menu(Menu::Repo, window, cx))),
            )
            .child(
                segment("git-branch", "git-branch", t(cx, "git.current_branch").into(), branch, self.menu == Some(Menu::Branch), true)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_menu(Menu::Branch, window, cx))),
            )
            .child(
                segment("git-remote", remote_icon, remote_title.into(), remote_sub, false, false)
                    .when(!has_remote, |d| d.opacity(0.5))
                    .when_some(remote_count, |d, n| {
                        d.child(
                            div()
                                .px_1p5()
                                .rounded_full()
                                .bg(hex(Chrome::SELECTED))
                                .t_caption()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(n.to_string()),
                        )
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if this.snapshot.has_remote {
                            this.remote_action(cx);
                        }
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .when_some(self.snapshot.web_url.clone(), |d, url| {
                        d.child(
                            crate::ui::icon_only("git-open-remote", "arrow-up-right", move |_, _, cx| cx.open_url(&url))
                                .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.open_remote"), None)),
                        )
                    })
                    .when_some(self.repo.clone(), |d, repo| {
                        d.child(
                            crate::ui::icon_only(
                                "git-open-terminal",
                                "terminal",
                                cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(GitEvent::OpenTerminal(repo.clone()))),
                            )
                            .tooltip(crate::ui::Tooltip::text(t(cx, "new.terminal"), None)),
                        )
                    })
                    .child(if self.loading || self.busy.is_some() {
                        div()
                            .size(px(crate::ui::ICON_BUTTON))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(crate::ui::spinner(IconSize::BUTTON, hex(Chrome::FOREGROUND)))
                            .into_any_element()
                    } else {
                        crate::ui::icon_only(
                            "git-refresh",
                            "refresh-cw",
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.message = None;
                                this.refresh(cx);
                            }),
                        )
                        .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.refresh"), Some("⌘R")))
                        .into_any_element()
                    }),
            )
    }

    fn render_repo_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut menu = popover().w(px(SIDEBAR_WIDTH + 60.)).max_h(px(420.)).id("git-repo-menu").overflow_y_scroll();
        if self.repos.is_empty() {
            menu = menu.child(crate::ui::hint(t(cx, "git.no_repo")));
        } else if self.repos.len() > 1 {
            menu = menu.child(crate::ui::hint(tf(cx, "git.pick_repo", &[("n", &self.repos.len().to_string())])));
        }
        for (index, repo) in self.repos.iter().enumerate() {
            let selected = self.repo.as_ref() == Some(repo);
            let target = repo.clone();
            menu = menu.child(
                div()
                    .id(("git-repo-item", index))
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(icon("folder", IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .t_body()
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(repo.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                            )
                            .child(div().t_small().truncate().text_color(hex(Chrome::MUTED)).child(tilde(repo))),
                    )
                    .when(selected, |d| d.child(icon("check", IconSize::INLINE, hex(Chrome::ACCENT))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_repo(Some(target.clone()), cx))),
            );
        }
        div()
            .absolute()
            .top(px(52.))
            .left_0()
            .child(gpui::deferred(crate::ui::fade_in("git-repo-menu-fade", menu)).with_priority(3))
            .into_any_element()
    }

    fn render_branch_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.snapshot.status.branch.clone().unwrap_or_default();
        let Some((picker, _)) = &self.branch_picker else { return div().into_any_element() };
        let tabs =
            div().flex().border_b_1().border_color(hex(Chrome::BORDER)).child(
                div().px_3().py_2().t_body().text_color(hex(Chrome::BRIGHT)).border_b_2().border_color(hex(Chrome::ACCENT)).child(
                    if self.merge_mode { tf(cx, "git.merge_into", &[("branch", &current)]) } else { t(cx, "git.branches").to_string() },
                ),
            );
        let footer = div().p_2().border_t_1().border_color(hex(Chrome::BORDER)).child(
            div()
                .id("git-merge-toggle")
                .w_full()
                .py_1p5()
                .rounded_md()
                .flex()
                .justify_center()
                .t_body()
                .cursor_pointer()
                .bg(hex(if self.merge_mode { Chrome::ACCENT } else { 0x2d2d30 }))
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.bg(hex(Chrome::ACCENT)))
                .child(if self.merge_mode {
                    t(cx, "git.cancel_merge").to_string()
                } else {
                    tf(cx, "git.choose_merge", &[("branch", &current)])
                })
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.merge_mode = !this.merge_mode;
                    cx.notify();
                })),
        );
        let menu = popover()
            .p_0()
            .w(px(SIDEBAR_WIDTH + 120.))
            .child(tabs)
            .child(div().p_1().child(picker.clone()))
            .when(!current.is_empty(), |d| d.child(footer));
        div()
            .absolute()
            .top(px(52.))
            .left(px(SIDEBAR_WIDTH))
            .child(gpui::deferred(crate::ui::fade_in("git-branch-menu-fade", menu)).with_priority(3))
            .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> Div {
        let tab_button = |id: &'static str, label: String, tab: Tab, cx: &mut Context<Self>| {
            let active = self.tab == tab;
            div()
                .id(id)
                .flex_1()
                .py_2()
                .flex()
                .justify_center()
                .gap_1p5()
                .t_body()
                .cursor_pointer()
                .border_b_2()
                .border_color(if active { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::MUTED }))
                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_tab(tab, cx)))
        };
        let count = self.snapshot.status.files.len();
        let changes_label = if count > 0 { format!("{}  {count}", t(cx, "git.changes")) } else { t(cx, "git.changes").into() };
        let body = match self.tab {
            Tab::Changes => self.render_changes_list(cx).into_any_element(),
            Tab::History => self.render_history_list(cx).into_any_element(),
        };
        div()
            .w(px(SIDEBAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(hex(Chrome::SIDE_BAR))
            .border_r_1()
            .border_color(hex(Chrome::BORDER))
            .child(
                div()
                    .flex()
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(tab_button("git-tab-changes", changes_label, Tab::Changes, cx))
                    .child(tab_button("git-tab-history", t(cx, "git.history").into(), Tab::History, cx)),
            )
            .child(body)
    }

    fn visible_files(&self, cx: &gpui::App) -> Vec<usize> {
        let query = self.filter.read(cx).text().to_lowercase();
        self.snapshot
            .status
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| query.is_empty() || f.path.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect()
    }

    fn render_changes_list(&self, cx: &mut Context<Self>) -> Div {
        let files = &self.snapshot.status.files;
        let included = files.iter().filter(|f| !self.excluded.contains(&f.path)).count();
        let visible = self.visible_files(cx);
        let all = included == files.len() && !files.is_empty();
        let header = div()
            .id("git-select-all")
            .px_3()
            .py_1p5()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .t_small()
            .text_color(hex(Chrome::FOREGROUND))
            .cursor_pointer()
            .child(checkbox(all, included > 0 && !all))
            .child(tf(cx, "git.changed_files", &[("n", &files.len().to_string())]))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if all {
                    this.excluded = this.snapshot.status.files.iter().map(|f| f.path.clone()).collect();
                } else {
                    this.excluded.clear();
                }
                cx.notify();
            }));

        let list: AnyElement = if files.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(icon("check", IconSize::ACTIVITY, hex(Chrome::MUTED)))
                .child(div().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "git.no_changes")))
                .into_any_element()
        } else {
            let handle = self.files_scroll.clone();
            let base = handle.0.borrow().base_handle.clone();
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    gpui::uniform_list(
                        "git-files",
                        visible.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            let visible = this.visible_files(cx);
                            range.filter_map(|i| visible.get(i).copied()).map(|index| this.render_file_row(index, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(handle)
                    .size_full(),
                )
                .child(crate::ui::scrollbar(base))
                .into_any_element()
        };

        let branch = self.snapshot.status.branch.clone().unwrap_or_else(|| "HEAD".into());
        let can_commit = included > 0 && !self.summary.read(cx).text().trim().is_empty() && self.busy.is_none();
        let commit_box = div()
            .flex_shrink_0()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_t_1()
            .border_color(hex(Chrome::BORDER))
            .child(field(&self.summary))
            .child(field(&self.description).min_h(px(56.)))
            .child(
                div()
                    .id("git-commit")
                    .w_full()
                    .py_1p5()
                    .rounded_md()
                    .flex()
                    .justify_center()
                    .t_body()
                    .font_weight(crate::theme::EMPHASIS)
                    .bg(hex(Chrome::ACCENT))
                    .text_color(hex(Chrome::BRIGHT))
                    .when(!can_commit, |d| d.opacity(0.45))
                    .when(can_commit, |d| d.cursor_pointer().hover(|s| s.bg(hex(0x1a8ae6))))
                    .child(match self.busy {
                        Some("git.committing") => t(cx, "git.committing").to_string(),
                        _ => tf(cx, "git.commit_to", &[("branch", &branch)]),
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.commit(cx))),
            );

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().p_2().border_b_1().border_color(hex(Chrome::BORDER)).child(field(&self.filter)))
            .child(header)
            .child(list)
            .child(commit_box)
    }

    fn render_file_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let file = &self.snapshot.status.files[index];
        let selected = self.selected_file.as_ref() == Some(&file.path);
        let included = !self.excluded.contains(&file.path);
        let confirming = self.confirm_discard.as_ref() == Some(&file.path);
        let (path, toggle_path, discard_path) = (file.path.clone(), file.path.clone(), file.path.clone());
        div()
            .id(("git-file", index))
            .group("git-file")
            .w_full()
            .h(px(FILE_ROW))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .t_body()
            .cursor_pointer()
            .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
            .when(!selected, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_file(path.clone(), cx)))
            .child(div().id(("git-file-check", index)).child(checkbox(included, false)).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    if !this.excluded.remove(&toggle_path) {
                        this.excluded.insert(toggle_path.clone());
                    }
                    cx.notify();
                },
            )))
            .child(path_label(&file.path))
            .child(
                div()
                    .id(("git-file-discard", index))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .when(!confirming, |d| d.invisible().group_hover("git-file", |s| s.visible()))
                    .when(confirming, |d| {
                        d.px_1().bg(hex(Chrome::ERROR)).t_caption().text_color(hex(Chrome::BRIGHT)).child(t(cx, "git.discard_confirm"))
                    })
                    .when(!confirming, |d| d.child(icon("undo-2", IconSize::INLINE, hex(Chrome::MUTED))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if this.confirm_discard.as_ref() == Some(&discard_path) {
                            this.confirm_discard = None;
                            if let Some(file) = this.file(&discard_path).cloned() {
                                this.run("git.discarding", cx, move |repo| git::discard(repo, &file));
                            }
                        } else {
                            this.confirm_discard = Some(discard_path.clone());
                        }
                        cx.notify();
                    })),
            )
            .child(kind_badge(file.kind))
            .into_any_element()
    }

    fn render_history_list(&self, cx: &mut Context<Self>) -> Div {
        if self.snapshot.commits.is_empty() {
            return div().flex_1().child(crate::ui::hint(t(cx, "git.no_commits")));
        }
        let handle = self.commits_scroll.clone();
        let base = handle.0.borrow().base_handle.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                gpui::uniform_list(
                    "git-commits",
                    self.snapshot.commits.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|i| this.render_commit_row(i, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(handle)
                .size_full(),
            )
            .child(crate::ui::scrollbar(base))
    }

    fn render_commit_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let commit = &self.snapshot.commits[index];
        let selected = self.selected_commit.as_ref() == Some(&commit.sha);
        let sha = commit.sha.clone();
        div()
            .id(("git-commit-row", index))
            .w_full()
            .h(px(COMMIT_ROW))
            .px_3()
            .flex()
            .flex_col()
            .justify_center()
            .gap_0p5()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .cursor_pointer()
            .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
            .when(!selected, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
            .child(div().t_body().truncate().text_color(hex(Chrome::BRIGHT)).child(commit.summary.clone()))
            .child(div().t_small().truncate().text_color(hex(Chrome::MUTED)).child(format!(
                "{} · {}",
                commit.author,
                relative_time(now_ms(), commit.time as u64 * 1000)
            )))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_commit(sha.clone(), cx)))
            .into_any_element()
    }

    fn render_diff(&self, header: Option<AnyElement>, cx: &mut Context<Self>) -> Div {
        let lines = self.diff.clone();
        let body: AnyElement = if lines.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .t_body()
                .text_color(hex(Chrome::MUTED))
                .child(t(cx, "git.select_file"))
                .into_any_element()
        } else {
            let handle = self.diff_scroll.clone();
            let base = handle.0.borrow().base_handle.clone();
            let font = mono(cx);
            let gutter = lines.iter().filter_map(|l| l.old.max(l.new)).max().unwrap_or(0).to_string().len().max(3) as f32 * 8. + 12.;
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    gpui::uniform_list("git-diff", lines.len(), move |range, _, _| {
                        range.map(|i| diff_row(&lines[i], gutter, font.clone())).collect::<Vec<_>>()
                    })
                    .track_scroll(handle)
                    .size_full(),
                )
                .child(crate::ui::scrollbar(base))
                .into_any_element()
        };
        div().flex_1().min_w_0().h_full().flex().flex_col().bg(hex(Chrome::EDITOR)).children(header).child(body)
    }

    fn render_file_header(&self, path: &str, extra: Option<String>) -> AnyElement {
        div()
            .h(px(34.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .t_body()
            .child(path_label(path))
            .when_some(extra, |d, extra| d.child(div().t_small().text_color(hex(Chrome::MUTED)).child(extra)))
            .into_any_element()
    }

    fn render_history_detail(&self, cx: &mut Context<Self>) -> Div {
        let Some(commit) = self.selected_commit.as_ref().and_then(|sha| self.snapshot.commits.iter().find(|c| &c.sha == sha)) else {
            return div().flex_1().h_full().bg(hex(Chrome::EDITOR)).child(crate::ui::hint(t(cx, "git.select_commit")));
        };
        let sha = commit.sha.clone();
        let header = div()
            .flex_shrink_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(commit.summary.clone()))
            .when(!commit.body.is_empty(), |d| d.child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(commit.body.clone())))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .t_small()
                    .text_color(hex(Chrome::MUTED))
                    .child(commit.author.clone())
                    .child(
                        div()
                            .id("git-copy-sha")
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                            .child(icon("git-commit-horizontal", IconSize::INLINE, hex(Chrome::MUTED)))
                            .child(commit.short.clone())
                            .on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(sha.clone()))),
                    )
                    .child(relative_time(now_ms(), commit.time as u64 * 1000))
                    .child(tf(cx, "git.changed_files", &[("n", &self.commit_files.len().to_string())])),
            );

        let mut files = div().id("git-commit-files").flex().flex_col().overflow_y_scroll();
        for (index, file) in self.commit_files.iter().enumerate() {
            let selected = self.commit_file.as_ref() == Some(&file.path);
            let path = file.path.clone();
            files = files.child(
                div()
                    .id(("git-commit-file", index))
                    .h(px(FILE_ROW))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .t_body()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
                    .when(!selected, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
                    .child(path_label(&file.path))
                    .child(kind_badge(file.kind))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_commit_file(path.clone(), cx))),
            );
        }
        let diff_header = self.commit_file.as_ref().map(|path| {
            let stats =
                self.commit_files.iter().find(|f| &f.path == path).and_then(|f| Some(format!("+{} −{}", f.additions?, f.deletions?)));
            self.render_file_header(path, stats)
        });
        div().flex_1().min_w_0().h_full().flex().flex_col().child(header).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(
                    div()
                        .w(px(260.))
                        .flex_shrink_0()
                        .h_full()
                        .border_r_1()
                        .border_color(hex(Chrome::BORDER))
                        .bg(hex(Chrome::SIDE_BAR))
                        .child(files),
                )
                .child(self.render_diff(diff_header, cx)),
        )
    }
}

fn diff_row(line: &DiffLine, gutter: f32, font: SharedString) -> AnyElement {
    let (bg, fg, sign) = match line.kind {
        LineKind::Added => (Some(hex_alpha(0x2ea043, 0.16)), Chrome::FOREGROUND, "+"),
        LineKind::Removed => (Some(hex_alpha(0xf85149, 0.16)), Chrome::FOREGROUND, "-"),
        LineKind::Hunk => (Some(hex_alpha(0x388bfd, 0.12)), Chrome::MUTED, ""),
        LineKind::Meta => (None, Chrome::MUTED, ""),
        LineKind::Context => (None, Chrome::FOREGROUND, " "),
    };
    let number = |n: Option<u32>| {
        div().w(px(gutter)).flex_shrink_0().pr_2().flex().justify_end().text_color(hex(0x6e7681)).children(n.map(|n| n.to_string()))
    };
    div()
        .h(px(DIFF_ROW))
        .w_full()
        .flex()
        .items_center()
        .font_family(font)
        .text_size(px(12.))
        .whitespace_nowrap()
        .overflow_hidden()
        .when_some(bg, |d, bg| d.bg(bg))
        .child(number(line.old))
        .child(number(line.new))
        .child(div().w(px(16.)).flex_shrink_0().text_color(hex(Chrome::MUTED)).child(sign))
        .child(div().flex_1().min_w_0().text_color(hex(fg)).child(line.text.replace('\t', "    ")))
        .into_any_element()
}

impl Render for GitView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.repo.is_none() {
            let message = if self.loaded { t(cx, "git.no_repo") } else { t(cx, "git.searching") };
            return div()
                .size_full()
                .bg(hex(Chrome::EDITOR))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(icon("git-branch", 40., hex(Chrome::MUTED)))
                .child(div().t_large().text_color(hex(Chrome::BRIGHT)).child(message))
                .when(self.loaded, |d| d.child(div().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "git.no_repo_hint"))))
                .into_any_element();
        }

        let content = match self.tab {
            Tab::Changes => {
                let header = self.selected_file.as_ref().and_then(|p| self.file(p)).map(|file| {
                    let label = match &file.original {
                        Some(original) => format!("{original} → {}", file.path),
                        None => file.path.clone(),
                    };
                    self.render_file_header(&label, None)
                });
                self.render_diff(header, cx)
            }
            Tab::History => self.render_history_detail(cx),
        };

        let banner = self.message.clone().map(|(text, error)| {
            div()
                .flex_shrink_0()
                .px_3()
                .py_1p5()
                .flex()
                .items_center()
                .gap_2()
                .bg(hex_alpha(if error { Chrome::ERROR } else { Chrome::SUCCESS }, 0.15))
                .border_b_1()
                .border_color(hex_alpha(if error { Chrome::ERROR } else { Chrome::SUCCESS }, 0.5))
                .t_body()
                .text_color(hex(Chrome::BRIGHT))
                .child(div().flex_1().min_w_0().child(text))
                .child(crate::ui::icon_only(
                    "git-dismiss",
                    "x",
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.message = None;
                        cx.notify();
                    }),
                ))
        });

        div()
            .id("git-view")
            .key_context("GitView")
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // Clicks elsewhere give the page keyboard focus (for its shortcuts), but never take it
                    // away from a text field that just got it.
                    if !window.focused(cx).is_some_and(|f| f != this.focus_handle) {
                        window.focus(&this.focus_handle);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &CommitChanges, _, cx| this.commit(cx)))
            .on_action(cx.listener(|this, _: &RemoteAction, _, cx| {
                if this.snapshot.has_remote {
                    this.remote_action(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Fetch, _, cx| {
                if this.snapshot.has_remote {
                    this.run("git.fetching", cx, git::fetch);
                }
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(Chrome::EDITOR))
            .child(self.render_toolbar(cx))
            .children(banner)
            .child(div().flex_1().min_h_0().flex().child(self.render_sidebar(cx)).child(content))
            .when(self.menu.is_some(), |d| {
                d.child(div().id("git-menu-backdrop").absolute().top(px(52.)).left_0().size_full().on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        this.menu = None;
                        cx.notify();
                    },
                )))
            })
            .when(self.menu == Some(Menu::Repo), |d| d.child(self.render_repo_menu(cx)))
            .when(self.menu == Some(Menu::Branch), |d| d.child(self.render_branch_menu(cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_paths_for_labels() {
        assert_eq!(split_path("src/ui/view.rs"), ("src/ui/".to_string(), "view.rs".to_string()));
        assert_eq!(split_path("README.md"), (String::new(), "README.md".to_string()));
    }
}
