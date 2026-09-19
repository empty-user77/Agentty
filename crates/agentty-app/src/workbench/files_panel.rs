//! Files panel docked at the right edge: the folder structure of the project the active pane works
//! in, what changed there, and — for a git repository — its working trees: which one each session
//! works in, and what each holds. Sessions of the same project get a working tree of their own
//! (`worktrees.rs`), so this is where that is made visible.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, icon_only_sized, menu_item, popover, tilde, IconSize, Tooltip, TypeScale};
use agentty_bridge::git::FileChange;
use agentty_bridge::worktree::Worktree;
use gpui::{div, prelude::*, px, AnyElement, AppContext, ClickEvent, ClipboardItem, Context, FontWeight, Pixels, Point, SharedString};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MIN_WIDTH: f32 = 220.;
/// Width the terminals keep when the browser and this panel are both docked at the right.
const MIN_TERMINALS: f32 = 440.;
const MIN_BROWSER: f32 = 320.;
/// The working-tree list: one row at least, and by default no more than six and a half (the half row
/// says "there is more"); the handle under it sets any height in between.
const MIN_TREES_HEIGHT: f32 = TREE_ROW_HEIGHT;
const DEFAULT_TREES_HEIGHT: f32 = TREE_ROW_HEIGHT * 6.5;

/// Height of the working-tree list: what the user dragged it to (`chosen`, 0 = never), never more
/// than its rows need and never less than one row.
pub(super) fn trees_height(rows: usize, chosen: f32) -> f32 {
    let content = rows as f32 * TREE_ROW_HEIGHT + 4.;
    let wanted = if chosen > 0. { chosen } else { DEFAULT_TREES_HEIGHT };
    wanted.min(content).max(MIN_TREES_HEIGHT.min(content))
}

/// Widths the browser and the files panel are shown at. Each has the width the user gave it — until
/// together they would squeeze the terminals: then the browser gives way first (down to its
/// minimum), then this panel. `room` is what is left of the window beside the side bars and the
/// plugin panel.
pub(super) fn docked_widths(room: f32, browser: Option<f32>, files: Option<f32>) -> (f32, f32) {
    let (mut browser_width, mut files_width) = (browser.unwrap_or(0.), files.map_or(0., |w| w.max(MIN_WIDTH)));
    let over = browser_width + files_width + MIN_TERMINALS - room;
    if over > 0. && browser.is_some() && files.is_some() {
        let give = over.min((browser_width - MIN_BROWSER).max(0.));
        browser_width -= give;
        files_width -= (over - give).min((files_width - MIN_WIDTH).max(0.));
    }
    (browser_width, files_width)
}
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

/// Right-click menu on a working tree of the list.
pub(super) struct TreeMenu {
    tree: Worktree,
    position: Point<Pixels>,
    /// Removal waiting for a second click: `Some(true)` with its branch.
    confirm: Option<bool>,
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
    tree_menu: Option<TreeMenu>,
    scroll: gpui::UniformListScrollHandle,
    generation: u64,
    loading: bool,
    /// Active pane and its folder when the panel last looked, to follow tab switches and `cd`.
    seen: Option<(gpui::EntityId, PathBuf)>,
    /// Example tree picked while the onboarding tour shows its example working trees.
    demo_pick: usize,
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

/// Working trees the onboarding tour shows in a folder that has none: a project with two AI sessions
/// in trees of their own. Nothing of it exists on disk or in git; (tool, status color) per row.
fn demo_trees() -> Vec<(TreeInfo, (&'static str, u32))> {
    let tree = |path: &str, branch: &str, main: bool| Worktree {
        path: PathBuf::from(path),
        branch: Some(branch.to_string()),
        head: String::new(),
        main,
        managed: false,
        prunable: false,
    };
    vec![
        (TreeInfo { tree: tree("/example/my-project", "main", true), changes: 1, ahead: 0 }, ("claude", Chrome::ORANGE)),
        (
            TreeInfo { tree: tree("/example/claude-0919-1121", "agentty/claude-0919-1121", false), changes: 2, ahead: 1 },
            ("claude", Chrome::SUCCESS),
        ),
        (
            TreeInfo { tree: tree("/example/codex-0919-1122", "agentty/codex-0919-1122", false), changes: 1, ahead: 0 },
            ("codex", Chrome::ATTENTION),
        ),
    ]
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
    // A tree whose folder was deleted by hand stays listed (as gone) so its menu can clean it up.
    snapshot.trees = trees
        .into_iter()
        .take(24)
        .map(|tree| {
            if tree.prunable {
                return TreeInfo { tree, changes: 0, ahead: 0 };
            }
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
                    tree_menu: None,
                    scroll: gpui::UniformListScrollHandle::new(),
                    generation: 0,
                    loading: false,
                    seen: None,
                    demo_pick: 0,
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

    /// Shown widths of (browser, files panel): see [`docked_widths`].
    pub(super) fn docked_widths(&self, cx: &gpui::App) -> (f32, f32) {
        let prefs = crate::settings::settings(cx);
        let sidebar = if self.sidebar_open { prefs.sidebar_width } else { 0. };
        let plugin = self.plugin_panel.as_ref().map_or(0., |_| super::plugin_panel::PANEL_WIDTH);
        let room = self.viewport_width - super::chrome::ACTIVITY_BAR_WIDTH - sidebar - plugin;
        docked_widths(room, self.browser.as_ref().map(|_| prefs.browser.width), self.files_panel.as_ref().map(|_| prefs.files_panel_width))
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

    /// Whether the working-tree section shows the tour's example trees: while the tour teaches the
    /// panel, in a project that has no linked trees of its own.
    pub(super) fn tour_shows_tree_demo(&self) -> bool {
        self.tour_teaches_files() && self.files_panel.as_ref().is_some_and(|p| !p.snapshot.trees.iter().any(|t| !t.tree.main))
    }

    /// Picks one of the tour's example trees (a click on it, or "do this step for me").
    pub(super) fn pick_demo_tree(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(panel) = self.files_panel.as_mut() {
            panel.demo_pick = index;
        }
        // The step asks for a session: the project folder of the examples does not count.
        if demo_trees().get(index).is_some_and(|(info, _)| !info.tree.main) {
            self.onboarding_event(super::onboarding::TourEvent::TreePicked, cx);
        }
        cx.notify();
    }

    /// The tour's "do this step for me" for picking a session's tree: the first real one, or the
    /// first example where the project has none.
    pub(super) fn pick_tree_for_tour(&mut self, cx: &mut Context<Self>) {
        if self.tour_shows_tree_demo() {
            return self.pick_demo_tree(1, cx);
        }
        let first = self.files_panel.as_ref().and_then(|p| p.snapshot.trees.iter().find(|t| !t.tree.main).map(|t| t.tree.path.clone()));
        if let (Some(path), Some(panel)) = (first, self.files_panel.as_mut()) {
            panel.pinned = Some(path);
            panel.error = None;
            self.refresh_files_panel(cx);
        }
        self.onboarding_event(super::onboarding::TourEvent::TreePicked, cx);
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

    /// Debug driver: the menu of the `index`-th tree in the list, as a right click would open it.
    pub(super) fn debug_tree_menu(&mut self, index: usize, cx: &mut Context<Self>) {
        let tree = self.files_panel.as_ref().and_then(|p| p.snapshot.trees.get(index)).map(|t| t.tree.clone());
        if let Some(tree) = tree {
            self.open_tree_menu(tree, gpui::point(px(1180.), px(140.)), cx);
        }
    }

    fn open_tree_menu(&mut self, tree: Worktree, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(panel) = self.files_panel.as_mut() {
            panel.tree_menu = Some(TreeMenu { tree, position, confirm: None });
            panel.error = None;
        }
        cx.notify();
    }

    fn close_tree_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = self.files_panel.as_mut() {
            panel.tree_menu = None;
        }
        cx.notify();
    }

    /// Shows the files of working tree `path` in the panel (what a click on its row does).
    fn view_tree(&mut self, path: PathBuf, main: bool, cx: &mut Context<Self>) {
        // The onboarding tour waits for a session's tree to be picked (the project folder is not one).
        if !main {
            self.onboarding_event(super::onboarding::TourEvent::TreePicked, cx);
        }
        if let Some(panel) = self.files_panel.as_mut() {
            panel.pinned = Some(path);
            panel.error = None;
            panel.tree_menu = None;
        }
        self.refresh_files_panel(cx);
        cx.notify();
    }

    /// From the menu: removes a linked working tree (Agentty's or the user's own), and its branch when
    /// asked. Git decides what is safe: a tree with changes and a branch with unmerged commits stay.
    fn remove_tree_from_menu(&mut self, tree: Worktree, delete_branch: bool, cx: &mut Context<Self>) {
        let Some(panel) = self.files_panel.as_mut() else { return };
        panel.tree_menu = None;
        let Some(repo) = panel.snapshot.trees.iter().find(|t| t.tree.main).map(|t| t.tree.path.clone()) else { return };
        // A session still working in it would lose its folder.
        if self.panes_in_tree(&tree.path, cx).into_iter().next().is_some() {
            let text = t(cx, "files.tree_in_use").to_string();
            if let Some(panel) = self.files_panel.as_mut() {
                panel.error = Some(text);
            }
            return cx.notify();
        }
        let path = tree.path.clone();
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx.background_spawn(async move { agentty_bridge::worktree::remove_linked(&repo, &target, delete_branch) }).await;
            let _ = this.update(cx, |this, cx| {
                let message = match &result {
                    Ok(Some(branch)) => Some(tf(cx, "files.branch_kept", &[("branch", branch)])),
                    Ok(None) => None,
                    Err(err) => {
                        let text = format!("{err:#}");
                        Some(if text.contains("modified or untracked") { t(cx, "files.tree_dirty").to_string() } else { text })
                    }
                };
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

    /// From the menu: forgets working trees whose folder was deleted by hand.
    fn prune_trees(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.files_panel.as_mut() else { return };
        panel.tree_menu = None;
        let Some(repo) = panel.snapshot.trees.iter().find(|t| t.tree.main).map(|t| t.tree.path.clone()) else { return };
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { agentty_bridge::worktree::prune(&repo) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(panel) = this.files_panel.as_mut() {
                    panel.error = result.err().map(|err| format!("{err:#}"));
                }
                this.refresh_files_panel(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The working tree menu: view its files, open a terminal there, show it, copy its path, and for
    /// a linked tree remove it (with its branch when asked; a second click confirms) or clean it up
    /// when its folder is gone.
    pub(super) fn render_tree_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.files_panel.as_ref()?.tree_menu.as_ref()?;
        let tree = menu.tree.clone();
        let (path, main) = (tree.path.clone(), tree.main);
        let in_use = self.panes_in_tree(&path, cx).into_iter().next().is_some();
        let separator = || div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER));
        let mut list = popover().w(px(250.)).on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_tree_menu(cx))).child(
            div()
                .px_3()
                .pt_1()
                .pb_1()
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .truncate()
                .child(tree.branch.clone().unwrap_or_else(|| tree.name())),
        );
        if !tree.prunable {
            let (view, terminal, reveal, copy) = (path.clone(), path.clone(), path.clone(), path.clone());
            list = list
                .child(menu_item(
                    "files-tree-menu-view",
                    t(cx, "files.menu.view"),
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.view_tree(view.clone(), main, cx)),
                ))
                .child(menu_item(
                    "files-tree-menu-terminal",
                    t(cx, "files.menu.terminal"),
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_tree_menu(cx);
                        this.open_tab(crate::launch::LaunchSpec::new(crate::launch::PaneKind::Shell, terminal.clone()), window, cx);
                    }),
                ))
                .child(menu_item(
                    "files-tree-menu-reveal",
                    t(cx, "files.reveal"),
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.close_tree_menu(cx);
                        crate::platform::reveal(&reveal);
                    }),
                ))
                .child(menu_item(
                    "files-tree-menu-copy",
                    t(cx, "files.menu.copy_path"),
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.display().to_string()));
                        this.close_tree_menu(cx);
                    }),
                ));
        }
        if main {
            return Some(self.place_tree_menu(menu.position, list));
        }
        list = list.child(separator());
        if tree.prunable {
            list = list.child(menu_item(
                "files-tree-menu-prune",
                t(cx, "files.menu.prune"),
                cx.listener(|this, _: &ClickEvent, _, cx| this.prune_trees(cx)),
            ));
        } else if in_use {
            list = list.child(div().px_3().py_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "files.tree_in_use")));
        } else {
            for (with_branch, id, label) in [
                (false, "files-tree-menu-remove", "files.menu.remove"),
                (true, "files-tree-menu-remove-branch", "files.menu.remove_branch"),
            ] {
                // The branch can only go along when there is one.
                if with_branch && tree.branch.is_none() {
                    continue;
                }
                let confirm = menu.confirm == Some(with_branch);
                let target = tree.clone();
                list = list.child(
                    menu_item(
                        id,
                        if confirm { t(cx, "files.remove_confirm") } else { t(cx, label) },
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let Some(menu) = this.files_panel.as_mut().and_then(|p| p.tree_menu.as_mut()) else { return };
                            if menu.confirm == Some(with_branch) {
                                this.remove_tree_from_menu(target.clone(), with_branch, cx);
                            } else {
                                menu.confirm = Some(with_branch);
                                cx.notify();
                            }
                        }),
                    )
                    .text_color(hex(Chrome::ERROR))
                    .when(confirm, |d| d.bg(hex_alpha(Chrome::ERROR, 0.25))),
                );
            }
        }
        Some(self.place_tree_menu(menu.position, list))
    }

    fn place_tree_menu(&self, position: Point<Pixels>, list: gpui::Div) -> AnyElement {
        gpui::deferred(gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(list))
            .with_priority(3)
            .into_any_element()
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
        let width = self.docked_widths(cx).1;
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

        let trees =
            (!snapshot.trees.is_empty() || self.tour_shows_tree_demo()).then(|| self.render_worktrees(panel, active_tree.as_deref(), cx));

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
        // The onboarding tour shows example trees where the project has none of its own.
        let demo = self.tour_shows_tree_demo();
        let (trees, demo_sessions): (Vec<TreeInfo>, Vec<(&'static str, u32)>) =
            if demo { demo_trees().into_iter().unzip() } else { (snapshot.trees.clone(), Vec::new()) };
        let linked = trees.iter().filter(|t| !t.tree.main).count();
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
            .when(demo, |d| {
                d.child(
                    div()
                        .ml_1()
                        .px_1()
                        .rounded_sm()
                        .bg(hex_alpha(Chrome::WARNING, 0.25))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(hex(Chrome::WARNING))
                        .child(t(cx, "flow.demo_tag")),
                )
            })
            .child(div().flex_1())
            .child(div().font_weight(FontWeight::NORMAL).child(tf(cx, "files.worktree_count", &[("n", &linked.to_string())])))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(panel) = this.files_panel.as_mut() {
                    panel.trees_folded = !panel.trees_folded;
                }
                cx.notify();
            }));
        let height = trees_height(trees.len(), crate::settings::settings(cx).files_panel_trees_height);
        // Scrolls inside its own height, which the handle below it changes.
        let mut list = div().id("files-trees-list").h(px(height)).overflow_y_scroll().flex().flex_col().pb_1();
        let count = trees.len();
        let ring = self.tour_target() == Some("files-trees");
        for (index, info) in trees.iter().enumerate().filter(|_| !folded) {
            let tree = &info.tree;
            let viewing = if demo { panel.demo_pick == index } else { tree.path == snapshot.root };
            let here = if demo { index == 0 } else { active_tree == Some(tree.path.as_path()) };
            let last = index + 1 == count;
            let sessions = if demo { Vec::new() } else { self.panes_in_tree(&tree.path, cx) };
            let (path, is_main) = (tree.path.clone(), tree.main);
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
            if let Some((tool, status_color)) = demo_sessions.get(index) {
                avatars = avatars.child(
                    div()
                        .relative()
                        .child(crate::brand::avatar(tool, 14.))
                        .child(div().absolute().right(px(-1.)).bottom(px(-1.)).size(px(5.)).rounded_full().bg(hex(*status_color))),
                );
            }
            let remove_path = tree.path.clone();
            let menu_tree = tree.clone();
            let gone = tree.prunable;
            list = list.child(
                div()
                    .id(SharedString::from(format!("files-tree-{index}")))
                    .group("files-tree")
                    .relative()
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
                    .tooltip(Tooltip::text(if demo { t(cx, "files.demo_note").to_string() } else { tilde(&tree.path) }, None))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        // A right click opens the menu (below); it must not also pick the tree and close it.
                        if event.is_right_click() || gone {
                            return;
                        }
                        if demo {
                            return this.pick_demo_tree(index, cx);
                        }
                        this.view_tree(path.clone(), is_main, cx);
                    }))
                    // Right click: what can be done with this tree (not with the tour's examples).
                    .when(!demo, |d| {
                        d.on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_tree_menu(menu_tree.clone(), event.position, cx);
                            }),
                        )
                    })
                    // The tour asks for a session's tree: those rows get its pulsing ring.
                    .when(ring && !tree.main, |d| d.child(crate::ui::pulse_ring("files-trees", false)))
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
                                    .when(gone, |d| d.child(div().text_color(hex(Chrome::WARNING)).child(t(cx, "files.tree_gone"))))
                                    .when(info.changes == 0 && info.ahead == 0 && !tree.main && !gone, |d| d.child(t(cx, "files.clean"))),
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
        // The handle between the trees and the files: a hairline at rest, lit while hovered or dragged.
        let resizing = self.files_trees_drag.is_some();
        let handle = div()
            .id("files-trees-resize")
            .group("files-trees-resize")
            .h(px(5.))
            .flex_shrink_0()
            .w_full()
            .flex()
            .flex_col()
            .justify_end()
            .cursor(gpui::CursorStyle::ResizeUpDown)
            .child(
                div()
                    .h(px(1.))
                    .w_full()
                    .bg(hex(Chrome::BORDER))
                    .group_hover("files-trees-resize", |s| s.h(px(5.)).bg(hex(Chrome::ACCENT)))
                    .when(resizing, |d| d.h(px(5.)).bg(hex(Chrome::ACCENT))),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.files_trees_drag = Some((f32::from(event.position.y), height));
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        div()
            .flex_shrink_0()
            .child(title)
            .when(!folded, |d| d.child(list).child(handle))
            .when(folded, |d| d.border_b_1().border_color(hex(Chrome::BORDER)))
            .into_any_element()
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
    fn the_tour_shows_a_project_with_sessions_in_trees_of_their_own() {
        let trees = demo_trees();
        assert_eq!(trees.iter().filter(|(info, _)| info.tree.main).count(), 1);
        assert!(trees.iter().filter(|(info, _)| !info.tree.main).count() >= 2);
        // Nothing of them exists: no remove button (it is only offered for trees Agentty made).
        assert!(trees.iter().all(|(info, _)| !info.tree.managed));
        assert!(trees.iter().all(|(info, _)| info.tree.path.starts_with("/example")));
    }

    #[test]
    fn the_tree_list_is_as_tall_as_asked_within_its_rows() {
        // Never dragged: all rows up to six and a half, then it scrolls.
        assert_eq!(trees_height(3, 0.), 3. * TREE_ROW_HEIGHT + 4.);
        assert_eq!(trees_height(12, 0.), DEFAULT_TREES_HEIGHT);
        // Dragged: that height, but never past the rows and never under one row.
        assert_eq!(trees_height(12, 400.), 400.);
        assert_eq!(trees_height(6, 400.), 6. * TREE_ROW_HEIGHT + 4.);
        assert_eq!(trees_height(6, 10.), MIN_TREES_HEIGHT);
        assert_eq!(trees_height(6, 90.), 90.);
    }

    #[test]
    fn docked_panels_leave_the_terminals_room() {
        // Plenty of room: both as the user sized them.
        assert_eq!(docked_widths(2000., Some(560.), Some(300.)), (560., 300.));
        // A 1400 pt window with the side bar open (about 1070 left): the browser gives way first.
        let (browser, files) = docked_widths(1070., Some(560.), Some(300.));
        assert_eq!((browser, files), (330., 300.));
        assert!(1070. - browser - files >= MIN_TERMINALS);
        // Tighter still: the browser stops at its minimum, then the files panel shrinks to its own.
        assert_eq!(docked_widths(900., Some(560.), Some(300.)), (MIN_BROWSER, MIN_WIDTH));
        // One panel alone keeps its width (the user can see what they resize).
        assert_eq!(docked_widths(700., Some(560.), None), (560., 0.));
        assert_eq!(docked_widths(700., None, Some(300.)), (0., 300.));
    }

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
