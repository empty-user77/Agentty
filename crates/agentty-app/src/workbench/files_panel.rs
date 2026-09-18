//! Files panel docked at the right edge: the folder structure of the project the active pane works
//! in, what changed there, and — for a git repository — its working trees: which one each session
//! works in, and what each holds. Sessions of the same project get a working tree of their own
//! (`worktrees.rs`), so this is where that is made visible.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, icon_only_sized, tilde, IconSize, Tooltip, TypeScale};
use agentty_bridge::git::FileChange;
use agentty_bridge::worktree::Worktree;
use gpui::{div, prelude::*, px, AnyElement, AppContext, ClickEvent, Context, FontWeight, SharedString};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MIN_WIDTH: f32 = 220.;
const ROW_HEIGHT: f32 = 22.;
const TREE_ROW_HEIGHT: f32 = 40.;
/// Entries shown per folder; a folder with more says how many were left out.
const MAX_ENTRIES: usize = 800;
/// Entries read from one folder before giving up on the rest (a cache folder can hold millions).
const MAX_READ: usize = 20_000;
const REFRESH_EVERY: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilesTab {
    Files,
    Changes,
}

#[derive(Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
}

/// A working tree of the repository with what is going on in it.
#[derive(Clone, PartialEq, Eq)]
struct TreeInfo {
    tree: Worktree,
    /// Files with uncommitted changes.
    changes: usize,
    /// Commits the project's own tree does not have.
    ahead: u32,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Snapshot {
    root: PathBuf,
    /// Children of the root and of every expanded folder; `usize`: entries left out.
    listings: HashMap<PathBuf, (Vec<Entry>, usize)>,
    changes: Vec<FileChange>,
    /// Absolute path → change kind, and the folders that contain a change.
    changed: HashMap<PathBuf, char>,
    changed_dirs: HashSet<PathBuf>,
    branch: Option<String>,
    trees: Vec<TreeInfo>,
}

pub(super) struct FilesPanel {
    /// Working tree picked in the panel; `None` follows the active pane.
    pinned: Option<PathBuf>,
    snapshot: Snapshot,
    expanded: HashSet<PathBuf>,
    tab: FilesTab,
    trees_folded: bool,
    selected: Option<PathBuf>,
    /// Tree whose removal waits for a second click, and an error from the last removal.
    confirm_remove: Option<PathBuf>,
    error: Option<String>,
    scroll: gpui::UniformListScrollHandle,
    generation: u64,
    loading: bool,
    /// Active pane and its folder when the panel last looked, to follow tab switches and `cd`.
    seen: Option<(gpui::EntityId, PathBuf)>,
}

impl FilesPanel {
    /// For the debug driver's `probe`.
    pub(super) fn debug_state(&self) -> serde_json::Value {
        serde_json::json!({
            "root": self.snapshot.root,
            "pinned": self.pinned,
            "changes": self.snapshot.changes.len(),
            "trees": self.snapshot.trees.iter().map(|t| serde_json::json!({ "path": t.tree.path, "branch": t.tree.branch, "managed": t.tree.managed, "changes": t.changes, "ahead": t.ahead })).collect::<Vec<_>>(),
            "error": self.error,
        })
    }
}

enum Row {
    Dir { path: PathBuf, name: String, depth: usize, open: bool, changed: bool },
    File { path: PathBuf, name: String, depth: usize, kind: Option<char> },
    More { depth: usize, count: usize },
}

fn read_folder(dir: &Path) -> (Vec<Entry>, usize) {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .take(MAX_READ)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            // Git's own folder is not part of the project; Finder's bookkeeping says nothing.
            if name == ".git" || name == ".DS_Store" {
                return None;
            }
            let dir = entry.file_type().map(|t| t.is_dir() || (t.is_symlink() && entry.path().is_dir())).unwrap_or(false);
            Some(Entry { name, path: entry.path(), dir })
        })
        .collect();
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    let left_out = entries.len().saturating_sub(MAX_ENTRIES);
    entries.truncate(MAX_ENTRIES);
    (entries, left_out)
}

fn change_color(kind: char) -> u32 {
    match kind {
        'A' | '?' => Chrome::SUCCESS,
        'D' => Chrome::ERROR,
        'U' => Chrome::ORANGE,
        _ => Chrome::WARNING,
    }
}

/// Everything the panel shows about `root`, read off the UI thread.
fn load(root: PathBuf, expanded: Vec<PathBuf>) -> Snapshot {
    let mut snapshot = Snapshot { root: root.clone(), ..Default::default() };
    snapshot.listings.insert(root.clone(), read_folder(&root));
    for dir in expanded.into_iter().filter(|dir| dir.starts_with(&root) && dir.is_dir()) {
        snapshot.listings.insert(dir.clone(), read_folder(&dir));
    }
    if agentty_bridge::worktree::tree_root(&root).as_deref() != Some(root.as_path()) {
        return snapshot; // a plain folder: nothing git can say about it
    }
    if let Ok(status) = agentty_bridge::git::status(&root) {
        snapshot.branch = status.branch.clone();
        for file in &status.files {
            let path = root.join(&file.path);
            for dir in path.ancestors().skip(1).take_while(|dir| dir.starts_with(&root)) {
                snapshot.changed_dirs.insert(dir.to_path_buf());
            }
            snapshot.changed.insert(path, file.kind);
        }
        snapshot.changes = status.files;
    }
    let trees = agentty_bridge::worktree::list(&root).unwrap_or_default();
    let main_head = trees.iter().find(|t| t.main).map(|t| t.head.clone()).unwrap_or_default();
    snapshot.trees = trees
        .into_iter()
        .filter(|tree| !tree.prunable)
        .take(24)
        .map(|tree| {
            let changes = if tree.path == root {
                snapshot.changes.len()
            } else {
                agentty_bridge::git::status(&tree.path).map(|s| s.files.len()).unwrap_or(0)
            };
            let ahead = if tree.main { 0 } else { agentty_bridge::worktree::commits_ahead(&tree.path, &main_head) };
            TreeInfo { tree, changes, ahead }
        })
        .collect();
    snapshot
}

impl Workbench {
    pub(super) fn toggle_files_panel(&mut self, cx: &mut Context<Self>) {
        if self.files_panel.take().is_some() {
            return cx.notify();
        }
        self.open_files_panel(None, cx);
    }

    /// Opens the panel, on the working tree `pinned` when given (else on the active pane's).
    pub(super) fn open_files_panel(&mut self, pinned: Option<PathBuf>, cx: &mut Context<Self>) {
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "files_panel" }));
        self.page = None;
        match self.files_panel.as_mut() {
            Some(panel) => panel.pinned = pinned,
            None => {
                self.files_panel = Some(FilesPanel {
                    pinned,
                    snapshot: Snapshot::default(),
                    expanded: HashSet::new(),
                    tab: FilesTab::Files,
                    trees_folded: false,
                    selected: None,
                    confirm_remove: None,
                    error: None,
                    scroll: gpui::UniformListScrollHandle::new(),
                    generation: 0,
                    loading: false,
                    seen: None,
                });
                // Refreshes while it is open; ends with the panel.
                cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(REFRESH_EVERY).await;
                    match this.update(cx, |this, cx| {
                        let open = this.files_panel.is_some();
                        if open && this.page.is_none() {
                            this.refresh_files_panel(cx);
                        }
                        open
                    }) {
                        Ok(true) => {}
                        _ => break,
                    }
                })
                .detach();
            }
        }
        self.refresh_files_panel(cx);
        cx.notify();
    }

    /// Follows the active pane: another tab or a `cd` shows that project right away. Called on render.
    pub(super) fn prepare_files_panel(&mut self, cx: &mut Context<Self>) {
        if self.files_panel.is_none() || self.page.is_some() {
            return;
        }
        let seen = self.active_pane().map(|pane| (pane.entity_id(), pane.read(cx).display_cwd()));
        let panel = self.files_panel.as_mut().expect("checked above");
        if panel.seen != seen {
            panel.seen = seen;
            self.refresh_files_panel(cx);
        }
    }

    /// The working tree (or plain folder) the active pane works in.
    fn active_tree(&self, cx: &gpui::App) -> Option<PathBuf> {
        let cwd = self
            .active_pane()
            .map(|p| p.read(cx).display_cwd())
            .or_else(|| self.workspaces.get(self.active_workspace).map(|w| w.cwd.clone()))?;
        Some(agentty_bridge::worktree::tree_root(&cwd).unwrap_or(cwd))
    }

    pub(super) fn refresh_files_panel(&mut self, cx: &mut Context<Self>) {
        let active = self.active_tree(cx);
        let Some(panel) = self.files_panel.as_mut() else { return };
        if panel.loading {
            return;
        }
        // A pinned tree is kept only while it belongs to the project the active pane is in.
        if let (Some(pinned), Some(active)) = (&panel.pinned, &active) {
            let related = panel.snapshot.trees.is_empty() || panel.snapshot.trees.iter().any(|t| &t.tree.path == active);
            if pinned == active || !related || !pinned.is_dir() {
                panel.pinned = None;
            }
        }
        let Some(root) = panel.pinned.clone().or(active) else { return };
        if root != panel.snapshot.root {
            panel.selected = None;
            panel.confirm_remove = None;
        }
        panel.loading = true;
        panel.generation += 1;
        let generation = panel.generation;
        let expanded: Vec<PathBuf> = panel.expanded.iter().cloned().collect();
        cx.spawn(async move |this, cx| {
            let snapshot = cx.background_spawn(async move { load(root, expanded) }).await;
            let _ = this.update(cx, |this, cx| {
                let Some(panel) = this.files_panel.as_mut().filter(|p| p.generation == generation) else { return };
                panel.loading = false;
                if panel.snapshot != snapshot {
                    panel.snapshot = snapshot;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn files_rows(panel: &FilesPanel) -> Vec<Row> {
        fn walk(panel: &FilesPanel, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
            let Some((entries, left_out)) = panel.snapshot.listings.get(dir) else { return };
            for entry in entries {
                if entry.dir {
                    let open = panel.expanded.contains(&entry.path);
                    rows.push(Row::Dir {
                        path: entry.path.clone(),
                        name: entry.name.clone(),
                        depth,
                        open,
                        changed: panel.snapshot.changed_dirs.contains(&entry.path),
                    });
                    if open && depth < 32 {
                        walk(panel, &entry.path, depth + 1, rows);
                    }
                } else {
                    let kind = panel.snapshot.changed.get(&entry.path).copied();
                    rows.push(Row::File { path: entry.path.clone(), name: entry.name.clone(), depth, kind });
                }
            }
            if *left_out > 0 {
                rows.push(Row::More { depth, count: *left_out });
            }
        }
        let mut rows = Vec::new();
        walk(panel, &panel.snapshot.root.clone(), 0, &mut rows);
        rows
    }

    fn toggle_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(panel) = self.files_panel.as_mut() else { return };
        if !panel.expanded.remove(&path) {
            // One folder reads fast enough to open under the click.
            panel.snapshot.listings.insert(path.clone(), read_folder(&path));
            panel.expanded.insert(path);
            self.onboarding_event(super::onboarding::TourEvent::FolderOpened, cx);
        }
        cx.notify();
    }

    /// Opens the first folder of the list (the onboarding tour's "do it for me").
    pub(super) fn open_first_folder(&mut self, cx: &mut Context<Self>) {
        let first = self.files_panel.as_ref().and_then(|panel| {
            let (entries, _) = panel.snapshot.listings.get(&panel.snapshot.root)?;
            entries.iter().find(|entry| entry.dir && !panel.expanded.contains(&entry.path)).map(|entry| entry.path.clone())
        });
        match first {
            Some(path) => self.toggle_folder(path, cx),
            // Nothing to open here (no folders, or the list is still loading): the step is done anyway.
            None => self.onboarding_event(super::onboarding::TourEvent::FolderOpened, cx),
        }
    }

    /// Types the path into the active pane (shell-quoted, never submitted), like a dropped file.
    fn insert_path(&mut self, path: &Path, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(pane) = self.active_pane() else { return };
        let base = pane.read(cx).display_cwd();
        let shown = path.strip_prefix(&base).map(Path::to_path_buf).unwrap_or_else(|_| path.to_path_buf());
        pane.update(cx, |view, _| view.drop_paths(&[dash_safe(shown)]));
        self.focus_pane(&pane, window, cx);
    }

    fn remove_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(panel) = self.files_panel.as_mut() else { return };
        if panel.confirm_remove.as_ref() != Some(&path) {
            panel.confirm_remove = Some(path);
            panel.error = None;
            return cx.notify();
        }
        panel.confirm_remove = None;
        // A session still working in it would lose its folder.
        if self.panes_in_tree(&path, cx).into_iter().next().is_some() {
            let text = t(cx, "files.tree_in_use").to_string();
            if let Some(panel) = self.files_panel.as_mut() {
                panel.error = Some(text);
            }
            return cx.notify();
        }
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx.background_spawn(async move { agentty_bridge::worktree::remove(&target, false) }).await;
            let _ = this.update(cx, |this, cx| {
                // Git's own words for the common case are a path and a hint at `--force`: say what to do.
                let message = result.as_ref().err().map(|err| {
                    let text = format!("{err:#}");
                    if text.contains("modified or untracked") {
                        t(cx, "files.tree_dirty").to_string()
                    } else {
                        text
                    }
                });
                if let Some(panel) = this.files_panel.as_mut() {
                    panel.error = message;
                    if result.is_ok() && panel.pinned.as_ref() == Some(&path) {
                        panel.pinned = None;
                    }
                }
                this.refresh_files_panel(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Panes whose folder is inside the working tree `root`.
    fn panes_in_tree(&self, root: &Path, cx: &gpui::App) -> Vec<Pane> {
        self.all_panes()
            .into_iter()
            .filter(|pane| agentty_bridge::worktree::tree_root(&pane.read(cx).display_cwd()).as_deref() == Some(root))
            .collect()
    }

    pub(super) fn render_files_splitter(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.files_panel.as_ref()?;
        Some(
            div()
                .id("files-resize")
                .group("files-resize")
                .w(px(5.))
                .flex_shrink_0()
                .h_full()
                .flex()
                .justify_center()
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .child(
                    div()
                        .w(px(1.))
                        .h_full()
                        .bg(hex(Chrome::BORDER))
                        .group_hover("files-resize", |s| s.w(px(5.)).bg(hex(Chrome::ACCENT)))
                        .when(self.files_resizing, |d| d.w(px(5.)).bg(hex(Chrome::ACCENT))),
                )
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.files_resizing = true;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_files_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.files_panel.as_ref()?;
        let snapshot = &panel.snapshot;
        let width = crate::settings::settings(cx).files_panel_width.max(MIN_WIDTH);
        let active_tree = self.active_tree(cx);
        let project = snapshot.trees.iter().find(|t| t.tree.main).map(|t| t.tree.path.clone()).unwrap_or_else(|| snapshot.root.clone());
        let project_name = project.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| tilde(&project));

        let header = div()
            .h(px(36.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .child(icon("folder-open", IconSize::BUTTON, hex(Chrome::BRIGHT)))
            .child(
                div()
                    .id("files-project")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .t_body()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hex(Chrome::BRIGHT))
                    .tooltip(Tooltip::text(tilde(&project), None))
                    .child(project_name),
            )
            .child(
                crate::ui::icon_only(
                    "files-refresh",
                    "refresh-cw",
                    cx.listener(|this, _: &ClickEvent, _, cx| this.refresh_files_panel(cx)),
                )
                .tooltip(Tooltip::text(t(cx, "usage.refresh"), None)),
            )
            .child(
                crate::ui::icon_only("files-reveal", "external-link", {
                    let root = snapshot.root.clone();
                    move |_: &ClickEvent, _, _| crate::platform::open_folder(&root)
                })
                .tooltip(Tooltip::text(t(cx, "files.reveal"), None)),
            )
            .child(crate::ui::icon_only(
                "files-close",
                "x",
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.files_panel = None;
                    cx.notify();
                }),
            ));

        let trees = (!snapshot.trees.is_empty()).then(|| self.render_worktrees(panel, active_tree.as_deref(), cx));

        let tab = |id: &'static str, label: String, which: FilesTab, cx: &mut Context<Self>| {
            let active = panel.tab == which;
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .t_small()
                .cursor_pointer()
                .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::MUTED }))
                .when(active, |d| d.bg(hex(Chrome::SELECTED)))
                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(panel) = this.files_panel.as_mut() {
                        panel.tab = which;
                    }
                    cx.notify();
                }))
        };
        let changes_label = if snapshot.changes.is_empty() {
            t(cx, "files.changes").to_string()
        } else {
            format!("{} {}", t(cx, "files.changes"), snapshot.changes.len())
        };
        // Which tree the list below belongs to, so a second copy of the same files is never mistaken
        // for the project's own.
        let shown = snapshot.trees.iter().find(|t| t.tree.path == snapshot.root);
        let tabs = div()
            .flex_shrink_0()
            .px_2()
            .py_1()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(tab("files-tab-files", t(cx, "files.files").to_string(), FilesTab::Files, cx))
            .child(tab("files-tab-changes", changes_label, FilesTab::Changes, cx))
            .child(div().flex_1())
            .when_some(shown.filter(|t| !t.tree.main), |d, info| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded_sm()
                        .bg(hex_alpha(Chrome::PURPLE, 0.18))
                        .t_caption()
                        .text_color(hex(Chrome::PURPLE))
                        .child(icon("git-fork", 11., hex(Chrome::PURPLE)))
                        .child(div().max_w(px(130.)).truncate().child(info.tree.name())),
                )
            });

        let body: AnyElement = match panel.tab {
            FilesTab::Files => self.render_file_rows(panel, cx),
            FilesTab::Changes => self.render_change_rows(panel, cx),
        };

        Some(
            div()
                .w(px(width))
                .flex_shrink_0()
                .h_full()
                .flex()
                .flex_col()
                .bg(hex(Chrome::PANEL))
                .child(header)
                .children(trees)
                .child(tabs)
                .when_some(panel.error.clone(), |d, error| {
                    d.child(div().flex_shrink_0().px_3().py_1p5().t_small().text_color(hex(Chrome::ERROR)).child(error))
                })
                .child(body)
                .into_any_element(),
        )
    }

    /// The working trees of the project, as a small graph hanging off the project's own tree.
    fn render_worktrees(&self, panel: &FilesPanel, active_tree: Option<&Path>, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = &panel.snapshot;
        let folded = panel.trees_folded;
        let linked = snapshot.trees.iter().filter(|t| !t.tree.main).count();
        let title = div()
            .id("files-trees-title")
            .px_2()
            .h(px(26.))
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .t_caption()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(hex(Chrome::MUTED))
            .child(icon(if folded { "chevron-right" } else { "chevron-down" }, 12., hex(Chrome::MUTED)))
            .child(t(cx, "files.worktrees").to_uppercase())
            .child(div().flex_1())
            .child(div().font_weight(FontWeight::NORMAL).child(tf(cx, "files.worktree_count", &[("n", &linked.to_string())])))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(panel) = this.files_panel.as_mut() {
                    panel.trees_folded = !panel.trees_folded;
                }
                cx.notify();
            }));
        let mut list = div().flex().flex_col().pb_1();
        let count = snapshot.trees.len();
        for (index, info) in snapshot.trees.iter().enumerate().filter(|_| !folded) {
            let tree = &info.tree;
            let viewing = tree.path == snapshot.root;
            let here = active_tree == Some(tree.path.as_path());
            let last = index + 1 == count;
            let sessions = self.panes_in_tree(&tree.path, cx);
            let path = tree.path.clone();
            let color = if tree.main { Chrome::BLUE } else { Chrome::PURPLE };
            let branch = tree.branch.clone().unwrap_or_else(|| t(cx, "files.detached").to_string());
            let confirm = panel.confirm_remove.as_ref() == Some(&tree.path);
            // Rail of the graph: the project's tree is the trunk, linked trees branch off it.
            let rail = div().w(px(18.)).h(px(TREE_ROW_HEIGHT)).flex_shrink_0().relative().when(!tree.main, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(8.))
                        .top_0()
                        .w(px(1.))
                        .h(px(if last { TREE_ROW_HEIGHT / 2. } else { TREE_ROW_HEIGHT }))
                        .bg(hex(Chrome::MUTED)),
                )
                .child(div().absolute().left(px(8.)).top(px(TREE_ROW_HEIGHT / 2.)).w(px(10.)).h(px(1.)).bg(hex(Chrome::MUTED)))
            });
            let mut avatars = div().flex().items_center().gap_0p5().flex_shrink_0();
            for pane in sessions.iter().take(4) {
                let view = pane.read(cx);
                let (_, status_color) = super::status_label(view, cx);
                avatars = avatars.child(div().relative().child(crate::brand::avatar(view.tool_id(), 14.)).when(view.is_agent(), |d| {
                    d.child(div().absolute().right(px(-1.)).bottom(px(-1.)).size(px(5.)).rounded_full().bg(hex(status_color)))
                }));
            }
            if sessions.len() > 4 {
                avatars = avatars.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(format!("+{}", sessions.len() - 4)));
            }
            let remove_path = tree.path.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("files-tree-{index}")))
                    .group("files-tree")
                    .h(px(TREE_ROW_HEIGHT))
                    .mx_1()
                    .pr_1()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(if viewing { hex(color) } else { hex_alpha(0, 0.) })
                    .when(viewing, |d| d.bg(hex_alpha(color, 0.12)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .tooltip(Tooltip::text(tilde(&tree.path), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(panel) = this.files_panel.as_mut() {
                            panel.pinned = Some(path.clone());
                            panel.error = None;
                        }
                        this.refresh_files_panel(cx);
                        cx.notify();
                    }))
                    .child(rail)
                    .child(icon(if tree.main { "folder" } else { "git-fork" }, 13., hex(color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .t_small()
                                            .text_color(hex(if viewing || here { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                                            .child(branch),
                                    )
                                    .when(here, |d| {
                                        d.child(
                                            div()
                                                .flex_shrink_0()
                                                .px_1()
                                                .rounded_sm()
                                                .bg(hex_alpha(Chrome::ACCENT, 0.35))
                                                .t_caption()
                                                .text_color(hex(Chrome::BRIGHT))
                                                .child(t(cx, "files.here")),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .t_caption()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(if tree.main { t(cx, "files.main_tree").to_string() } else { tree.name() })
                                    .when(info.changes > 0, |d| {
                                        d.child(div().text_color(hex(Chrome::WARNING)).child(tf(
                                            cx,
                                            "files.n_changed",
                                            &[("n", &info.changes.to_string())],
                                        )))
                                    })
                                    .when(info.ahead > 0, |d| {
                                        d.child(div().text_color(hex(Chrome::BLUE)).child(format!("↑{}", info.ahead)))
                                    })
                                    .when(info.changes == 0 && info.ahead == 0 && !tree.main, |d| d.child(t(cx, "files.clean"))),
                            ),
                    )
                    .child(avatars)
                    // Only trees Agentty made, and only when no session works in them.
                    .when(tree.managed && sessions.is_empty(), |d| {
                        d.child(
                            icon_only_sized(
                                SharedString::from(format!("files-tree-remove-{index}")),
                                if confirm { "check" } else { "trash-2" },
                                20.,
                                IconSize::INLINE,
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.remove_worktree(remove_path.clone(), cx);
                                }),
                            )
                            .when(confirm, |d| d.bg(hex_alpha(Chrome::ERROR, 0.35)))
                            .when(!confirm, |d| d.invisible().group_hover("files-tree", |s| s.visible()))
                            .tooltip(Tooltip::text(t(cx, if confirm { "files.remove_confirm" } else { "files.remove_tree" }), None)),
                        )
                    }),
            );
        }
        div().flex_shrink_0().border_b_1().border_color(hex(Chrome::BORDER)).child(title).child(list).into_any_element()
    }

    fn render_file_rows(&self, panel: &FilesPanel, cx: &mut Context<Self>) -> AnyElement {
        let count = Self::files_rows(panel).len();
        if count == 0 {
            let text = if panel.loading { t(cx, "usage.loading") } else { t(cx, "files.empty") };
            return div().flex_1().p_3().t_small().text_color(hex(Chrome::MUTED)).child(text).into_any_element();
        }
        let handle = panel.scroll.clone();
        let base = handle.0.borrow().base_handle.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                gpui::uniform_list(
                    "files-rows",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        let Some(panel) = this.files_panel.as_ref() else { return Vec::new() };
                        let rows = Self::files_rows(panel);
                        let selected = panel.selected.clone();
                        range
                            .filter_map(|i| rows.get(i).map(|row| this.render_file_row(i, row, selected.as_deref(), cx)))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(handle)
                .size_full(),
            )
            .child(crate::ui::scrollbar(base))
            .into_any_element()
    }

    fn render_file_row(&self, index: usize, row: &Row, selected: Option<&Path>, cx: &mut Context<Self>) -> AnyElement {
        let indent = |depth: usize| px(8. + depth as f32 * 12.);
        match row {
            Row::Dir { path, name, depth, open, changed } => {
                let target = path.clone();
                div()
                    .id(("files-row", index))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .pl(indent(*depth))
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .t_small()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_folder(target.clone(), cx)))
                    .child(icon(if *open { "chevron-down" } else { "chevron-right" }, 12., hex(Chrome::MUTED)))
                    .child(icon(if *open { "folder-open" } else { "folder" }, 13., hex(Chrome::BLUE)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(hex(if *changed { Chrome::WARNING } else { Chrome::FOREGROUND }))
                            .child(name.clone()),
                    )
                    .when(*changed, |d| d.child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(Chrome::WARNING))))
                    .into_any_element()
            }
            Row::File { path, name, depth, kind } => {
                let is_selected = selected == Some(path.as_path());
                let (select, insert, reveal) = (path.clone(), path.clone(), path.clone());
                let color = kind.map(change_color);
                div()
                    .id(("files-row", index))
                    .group("files-row")
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .pl(indent(*depth) + px(13.))
                    .pr_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .t_small()
                    .when(is_selected, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if let Some(panel) = this.files_panel.as_mut() {
                            panel.selected = Some(select.clone());
                        }
                        // A double click hands the path to the terminal, like dropping the file on it.
                        if event.click_count() >= 2 {
                            this.insert_path(&select, window, cx);
                        }
                        cx.notify();
                    }))
                    .child(icon("file", 12., hex(color.unwrap_or(Chrome::MUTED))))
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(color.unwrap_or(Chrome::FOREGROUND))).child(name.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .invisible()
                            .group_hover("files-row", |s| s.visible())
                            .child(
                                icon_only_sized(
                                    ("files-insert", index),
                                    "file-input",
                                    18.,
                                    12.,
                                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                                        cx.stop_propagation();
                                        this.insert_path(&insert, window, cx);
                                    }),
                                )
                                .tooltip(Tooltip::text(t(cx, "files.insert_path"), None)),
                            )
                            .child(
                                icon_only_sized(("files-show", index), "external-link", 18., 12., move |_: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    crate::platform::reveal(&reveal);
                                })
                                .tooltip(Tooltip::text(t(cx, "files.reveal"), None)),
                            ),
                    )
                    .when_some(*kind, |d, kind| {
                        d.child(div().flex_shrink_0().w(px(12.)).t_caption().text_color(hex(change_color(kind))).child(kind_label(kind)))
                    })
                    .into_any_element()
            }
            Row::More { depth, count } => div()
                .h(px(ROW_HEIGHT))
                .pl(indent(*depth) + px(13.))
                .flex()
                .items_center()
                .t_caption()
                .text_color(hex(Chrome::MUTED))
                .child(tf(cx, "files.more", &[("n", &count.to_string())]))
                .into_any_element(),
        }
    }

    fn render_change_rows(&self, panel: &FilesPanel, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = &panel.snapshot;
        if snapshot.changes.is_empty() {
            let text = if snapshot.trees.is_empty() { t(cx, "files.not_a_repo") } else { t(cx, "files.no_changes") };
            return div().flex_1().p_3().t_small().text_color(hex(Chrome::MUTED)).child(text).into_any_element();
        }
        let mut list = div().id("files-changes").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().py_1();
        for (index, file) in snapshot.changes.iter().take(500).enumerate() {
            let path = snapshot.root.join(&file.path);
            let (name, folder) = match file.path.rsplit_once('/') {
                Some((folder, name)) => (name.to_string(), folder.to_string()),
                None => (file.path.clone(), String::new()),
            };
            let (insert, reveal) = (path.clone(), path.clone());
            list = list.child(
                div()
                    .id(("files-change", index))
                    .group("files-change")
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .t_small()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(
                        div().flex_shrink_0().w(px(12.)).t_caption().text_color(hex(change_color(file.kind))).child(kind_label(file.kind)),
                    )
                    .child(div().flex_shrink_0().max_w(px(200.)).truncate().text_color(hex(change_color(file.kind))).child(name))
                    .child(div().flex_1().min_w_0().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(folder))
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .invisible()
                            .group_hover("files-change", |s| s.visible())
                            .child(
                                icon_only_sized(
                                    ("files-change-insert", index),
                                    "file-input",
                                    18.,
                                    12.,
                                    cx.listener(move |this, _: &ClickEvent, window, cx| this.insert_path(&insert, window, cx)),
                                )
                                .tooltip(Tooltip::text(t(cx, "files.insert_path"), None)),
                            )
                            .child(
                                icon_only_sized(("files-change-show", index), "external-link", 18., 12., move |_: &ClickEvent, _, _| {
                                    crate::platform::reveal(&reveal)
                                })
                                .tooltip(Tooltip::text(t(cx, "files.reveal"), None)),
                            ),
                    ),
            );
        }
        // The diffs themselves are on the Git page.
        list.child(div().px_2().pt_2().child(crate::ui::action_button(
            "files-open-git",
            t(cx, "files.open_git"),
            cx.listener(|this, _: &ClickEvent, _, cx| this.open_page(super::Page::Git, cx)),
        )))
        .into_any_element()
    }
}

/// `-rf` → `./-rf`: a file whose name starts with a dash would otherwise be read as an option by the
/// command it is typed after.
fn dash_safe(path: PathBuf) -> PathBuf {
    if path.is_relative() && path.to_string_lossy().starts_with('-') {
        Path::new(".").join(path)
    } else {
        path
    }
}

fn kind_label(kind: char) -> String {
    match kind {
        '?' => "U".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_looks_like_an_option_is_typed_as_a_path() {
        assert_eq!(dash_safe(PathBuf::from("-rf")), PathBuf::from("./-rf"));
        assert_eq!(dash_safe(PathBuf::from("src/-x")), PathBuf::from("src/-x"));
        assert_eq!(dash_safe(PathBuf::from("/abs/-x")), PathBuf::from("/abs/-x"));
    }

    #[test]
    fn folders_first_and_gits_folder_left_out() {
        let dir = std::env::temp_dir().join(format!("agentty-files-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for folder in [".git", "src", "Assets"] {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
        }
        for file in ["b.txt", "A.txt", ".DS_Store", ".env.example"] {
            std::fs::write(dir.join(file), "x").unwrap();
        }
        let (entries, left_out) = read_folder(&dir);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Assets", "src", ".env.example", "A.txt", "b.txt"]);
        assert_eq!(left_out, 0);
        std::fs::remove_dir_all(dir).ok();
    }
}
