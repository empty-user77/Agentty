//! A working tree per agent session: a new agent session started in a working tree that another
//! agent already works in can get its own (`agentty_bridge::worktree`), so two sessions never edit
//! the same files. Whether it does is asked — two sessions on one branch, to look at it or work on it
//! together, is just as real a need. Shells, resumed sessions and the first session of a project
//! stay where they are; so do prompts from links and plugins, which need their pane at once and get
//! a tree of their own without a question.

use super::ask::{Ask, AskAction, AskChoice};
use super::{LaunchTarget, Workbench};
use crate::agent_signal::browser_reply;
use crate::i18n::{t, tf};
use crate::launch::{LaunchChoice, PaneKind};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::icon;
use agentty_bridge::worktree::tree_root;
use gpui::{div, prelude::*, px, AppContext, Context, Window};
use std::path::{Path, PathBuf};

/// A new agent session asked for in a working tree another agent already works in, waiting for the
/// user to say where it goes.
#[derive(Clone)]
pub enum TreeRequest {
    /// From the launcher or the + menu.
    Launch { choice: LaunchChoice, target: LaunchTarget, cwd: PathBuf, root: PathBuf },
    /// An agent typed into a shell (`pane`): the shell wrapper waits on `reply` before starting it.
    Shell { reply: std::sync::mpsc::Sender<String>, pane: u64, cwd: PathBuf, root: PathBuf, label: String },
}

impl TreeRequest {
    fn root(&self) -> &Path {
        match self {
            TreeRequest::Launch { root, .. } | TreeRequest::Shell { root, .. } => root,
        }
    }

    /// The pane of an agent typed into a shell.
    pub(super) fn shell_pane(&self) -> Option<u64> {
        match self {
            TreeRequest::Shell { pane, .. } => Some(*pane),
            TreeRequest::Launch { .. } => None,
        }
    }

    /// Tells a waiting shell to start its agent where it was typed.
    pub(super) fn stay(&self) {
        if let TreeRequest::Shell { reply, .. } = self {
            let _ = reply.send(browser_reply(Ok("null".into())));
        }
    }
}

/// Folder name and branch suffix for the session's tree.
fn label(choice: &LaunchChoice) -> String {
    match choice {
        LaunchChoice::Kind(PaneKind::Codex) | LaunchChoice::Model(PaneKind::Codex, _) => "codex".into(),
        LaunchChoice::Kind(_) | LaunchChoice::Model(..) | LaunchChoice::Chat => "claude".into(),
        LaunchChoice::Command { title, .. } => title.split_whitespace().next().unwrap_or("agent").to_lowercase(),
    }
}

impl Workbench {
    /// Whether an agent is at work in the working tree `root` (in any workspace of this window).
    fn tree_is_taken(&self, root: &Path, cx: &gpui::App) -> bool {
        self.all_panes().iter().any(|pane| {
            let view = pane.read(cx);
            view.tool_id() != "shell" && tree_root(&view.display_cwd()).as_deref() == Some(root)
        })
    }

    /// Whether an agent in a pane other than `pane_id` works in the working tree `root`.
    pub(crate) fn tree_is_taken_by_other(&self, root: &Path, pane_id: u64, cx: &gpui::App) -> bool {
        self.all_panes().iter().any(|pane| {
            let view = pane.read(cx);
            view.pane_id != pane_id && view.tool_id() != "shell" && tree_root(&view.display_cwd()).as_deref() == Some(root)
        })
    }

    /// An agent typed into a shell (`claude` in a split, say) is normally moved to a tree of its own
    /// by the shell wrapper before it starts (`answer_worktree_request`); one that still shares a
    /// tree (a resumed session, a shell without Agentty's integration) can't be moved afterwards.
    /// Say so once, and how to get a tree of its own next time.
    pub(super) fn warn_about_shared_tree(&mut self, pane: &super::Pane, cx: &mut Context<Self>) {
        let view = pane.read(cx);
        let by_hand = view.spec.kind == PaneKind::Shell && view.tool_id() != "shell";
        if !by_hand || !crate::settings::settings(cx).auto_worktree || self.shared_tree_warned.contains(&view.pane_id) {
            return;
        }
        let Some(root) = tree_root(&view.display_cwd()) else { return };
        let (id, entity) = (view.pane_id, pane.entity_id());
        let shared = self.all_panes().iter().any(|other| {
            let other_view = other.read(cx);
            other.entity_id() != entity
                && other_view.tool_id() != "shell"
                && tree_root(&other_view.display_cwd()).as_deref() == Some(root.as_path())
        });
        if shared {
            self.shared_tree_warned.insert(id);
            self.show_toast_for(t(cx, "worktree.shared").to_string(), 9000, cx);
        }
    }

    /// For launches that must return their pane right away (prompts from links and plugins): when the
    /// working tree at `cwd` is taken by another agent, creates one for the new session now and
    /// returns the folder to start in (the same subfolder inside it).
    pub(super) fn own_tree_now(&mut self, kind: PaneKind, cwd: &Path, cx: &mut Context<Self>) -> Option<PathBuf> {
        if !crate::settings::settings(cx).auto_worktree || kind == PaneKind::Shell {
            return None;
        }
        let root = tree_root(cwd)?;
        if !self.tree_is_taken(&root, cx) {
            return None;
        }
        let label = label(&LaunchChoice::Kind(kind));
        match agentty_bridge::worktree::create(cwd, &label) {
            Ok(tree) => {
                let inside = cwd.strip_prefix(&root).ok().map(|rest| tree.path.join(rest)).filter(|dir| dir.is_dir());
                let branch = tree.branch.clone().unwrap_or_else(|| tree.name());
                self.show_toast(tf(cx, "worktree.started", &[("branch", &branch)]), cx);
                self.refresh_files_panel(cx);
                Some(inside.unwrap_or(tree.path))
            }
            Err(err) => {
                self.show_toast(tf(cx, "worktree.failed", &[("error", &format!("{err:#}"))]), cx);
                None
            }
        }
    }

    /// When the working tree at `cwd` is taken, asks whether `choice` gets a tree of its own or joins
    /// the one in use. Returns whether it took over the launch (the answer starts it).
    pub(super) fn launch_in_own_tree(&mut self, choice: &LaunchChoice, target: LaunchTarget, cwd: &Path, cx: &mut Context<Self>) -> bool {
        // A chat's lead works in the project itself: it reviews and merges what its workers did.
        if !crate::settings::settings(cx).auto_worktree || matches!(choice, LaunchChoice::Kind(PaneKind::Shell) | LaunchChoice::Chat) {
            return false;
        }
        let Some(root) = tree_root(cwd) else { return false };
        if !self.tree_is_taken(&root, cx) {
            return false;
        }
        self.ask_which_tree(TreeRequest::Launch { choice: choice.clone(), target, cwd: cwd.to_path_buf(), root }, cx);
        true
    }

    /// "An agent already works here": a new working tree, or the existing one.
    pub(super) fn ask_which_tree(&mut self, request: TreeRequest, cx: &mut Context<Self>) {
        let root = request.root().to_path_buf();
        let shell_pane = request.shell_pane();
        // Closing the question sends a waiting shell's agent into the existing tree: that is already an answer.
        let cancel = shell_pane.is_none();
        // The branch the other session is on, as its own bar shows it; the folder when it has none.
        let branch = self.all_panes().iter().find_map(|pane| {
            let view = pane.read(cx);
            (view.tool_id() != "shell" && tree_root(&view.display_cwd()).as_deref() == Some(root.as_path()))
                .then(|| view.git_branch.clone())
                .flatten()
        });
        // Joining says which branch it joins ("Work on branch main"); only a detached tree falls back to the folder's words.
        let join_label = match &branch {
            Some(branch) => tf(cx, "worktree.ask_on_branch", &[("branch", branch)]),
            None => String::new(),
        };
        let name = branch.unwrap_or_else(|| root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        // The project's own folder (main, say) is no worktree: calling it one reads as if a worktree were open.
        let linked = agentty_bridge::worktree::is_linked(&root);
        let (title, body, existing) = if linked {
            ("worktree.ask_title", "worktree.ask_body", "worktree.ask_existing")
        } else {
            ("worktree.ask_title_main", "worktree.ask_body_main", "worktree.ask_existing_main")
        };
        let ask = Ask {
            title: t(cx, title).into(),
            body: Some(tf(cx, body, &[("tree", &name)]).into()),
            choices: vec![
                AskChoice {
                    label: if join_label.is_empty() { t(cx, existing).into() } else { join_label.into() },
                    action: AskAction::SameTree(request.clone()),
                    primary: false,
                    danger: false,
                },
                AskChoice { label: t(cx, "worktree.ask_new").into(), action: AskAction::NewTree(request), primary: true, danger: false },
            ],
            cancel,
        };
        self.ask(ask, cx);
        // A shell stops waiting after a while and starts its agent where it was typed: the
        // question goes with it.
        if let Some(pane) = shell_pane {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(crate::agent_signal::ASK_WAIT).await;
                let _ = this.update(cx, |this, cx| {
                    if this.asks_for_shell(pane) {
                        this.dismiss_ask(cx);
                    }
                });
            })
            .detach();
        }
    }

    /// Joins the working tree in use: the session starts in the folder it was asked for.
    pub(super) fn start_in_same_tree(&mut self, request: TreeRequest, window: &mut Window, cx: &mut Context<Self>) {
        match request {
            TreeRequest::Launch { choice, target, cwd, .. } => self.launch_in(choice, target, cwd, None, window, cx),
            shell @ TreeRequest::Shell { pane, .. } => {
                // Sharing the tree was the answer to the question: no warning about it afterwards.
                self.shared_tree_warned.insert(pane);
                shell.stay();
            }
        }
    }

    /// Creates a working tree for the session and starts it there.
    pub(super) fn start_in_new_tree(&mut self, request: TreeRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.show_toast(t(cx, "worktree.creating").to_string(), cx);
        match request {
            TreeRequest::Launch { choice, target, cwd: from, root } => {
                let label = label(&choice);
                let handle = window.window_handle();
                cx.spawn(async move |this, cx| {
                    let source = from.clone();
                    let created = cx.background_spawn(async move { agentty_bridge::worktree::create(&source, &label) }).await;
                    let _ = cx.update_window(handle, |_, window, cx| {
                        let _ = this.update(cx, |this, cx| match created {
                            Ok(tree) => {
                                // The same folder inside the new tree (a package of a monorepo, say).
                                let inside = from.strip_prefix(&root).ok().map(|rest| tree.path.join(rest)).filter(|dir| dir.is_dir());
                                let project = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                                this.launch_in(choice, target, inside.unwrap_or_else(|| tree.path.clone()), Some(&root), window, cx);
                                if target == LaunchTarget::NewWorkspace {
                                    if let Some(ws) = this.workspaces.last_mut() {
                                        ws.name = Some(format!("{project} ⑂ {}", tree.name()));
                                    }
                                    this.persist(cx);
                                }
                                let branch = tree.branch.clone().unwrap_or_else(|| tree.name());
                                this.show_toast(tf(cx, "worktree.started", &[("branch", &branch)]), cx);
                                this.refresh_files_panel(cx);
                            }
                            Err(err) => {
                                // No tree (a repository without commits, a read-only disk…): work in the folder asked for.
                                eprintln!("agentty: could not create a working tree: {err:#}");
                                this.launch_in(choice, target, from.clone(), None, window, cx);
                                this.show_toast(tf(cx, "worktree.failed", &[("error", &format!("{err:#}"))]), cx);
                            }
                        });
                    });
                })
                .detach();
            }
            TreeRequest::Shell { reply, cwd, root, label, .. } => {
                let source = cwd.clone();
                let task = cx.background_spawn(async move { agentty_bridge::worktree::create(&source, &label) });
                cx.spawn(async move |this, cx| {
                    let created = task.await;
                    let _ = this.update(cx, |this, cx| match created {
                        Ok(tree) => {
                            // The same folder inside the new tree (a package of a monorepo, say).
                            let inside = cwd.strip_prefix(&root).ok().map(|rest| tree.path.join(rest)).filter(|dir| dir.is_dir());
                            let branch = tree.branch.clone().unwrap_or_else(|| tree.name());
                            let answer = serde_json::json!({
                                "path": inside.unwrap_or_else(|| tree.path.clone()),
                                "message": tf(cx, "worktree.started_shell", &[("branch", &branch)]),
                            });
                            let _ = reply.send(browser_reply(Ok(answer.to_string())));
                            this.show_toast(tf(cx, "worktree.started", &[("branch", &branch)]), cx);
                            this.refresh_files_panel(cx);
                        }
                        Err(err) => {
                            eprintln!("agentty: could not create a working tree: {err:#}");
                            let _ = reply.send(browser_reply(Ok("null".into())));
                            this.show_toast(tf(cx, "worktree.failed", &[("error", &format!("{err:#}"))]), cx);
                        }
                    });
                })
                .detach();
            }
        }
    }
}

/// `agentty worktree-for`: an agent typed into a shell pane is about to start in `request.cwd`. When
/// another agent pane (in any window) already works in that tree, create one for it and answer
/// with its path — the shell wrapper moves there before the agent starts. Otherwise answer `null`
/// and it starts where it is.
pub fn answer_worktree_request(
    windows: &[gpui::WindowHandle<Workbench>],
    request: crate::agent_signal::WorktreeRequest,
    cx: &mut gpui::App,
) {
    let stay = |request: &crate::agent_signal::WorktreeRequest| {
        let _ = request.reply.send(browser_reply(Ok("null".into())));
    };
    let holder = windows.iter().copied().find(|w| w.read(cx).is_ok_and(|wb| wb.has_pane(request.pane, cx)));
    let (Some(holder), Some(root)) = (holder, tree_root(&request.cwd)) else { return stay(&request) };
    if !crate::settings::settings(cx).auto_worktree {
        return stay(&request);
    }
    let taken = windows.iter().any(|w| w.read(cx).is_ok_and(|wb| wb.tree_is_taken_by_other(&root, request.pane, cx)));
    if !taken {
        return stay(&request);
    }
    // The answer is the user's now: tell the wrapper to wait for it (Agentty's dialog says what for).
    let _ = holder.update(cx, |this, _, cx| {
        let waiting = serde_json::json!({ "asking": true });
        let _ = request.reply.send(browser_reply(Ok(waiting.to_string())));
        let request = TreeRequest::Shell {
            reply: request.reply.clone(),
            pane: request.pane,
            cwd: request.cwd.clone(),
            root,
            label: request.label.clone(),
        };
        this.ask_which_tree(request, cx);
    });
}

/// The trees of a folder of projects in the order the list shows them: the project worked on last
/// first; in each project its own folder, then its worktrees, the one worked on last first. Ties go
/// by path, so the order holds still between scans.
fn most_recent_first(mut trees: Vec<agentty_bridge::inventory::TreeStatus>) -> Vec<agentty_bridge::inventory::TreeStatus> {
    let mut latest: std::collections::HashMap<PathBuf, i64> = std::collections::HashMap::new();
    for tree in &trees {
        let at = latest.entry(tree.repo.clone()).or_insert(0);
        *at = (*at).max(tree.last_worked());
    }
    trees.sort_by(|a, b| {
        let project = |t: &agentty_bridge::inventory::TreeStatus| latest.get(&t.repo).copied().unwrap_or(0);
        project(b)
            .cmp(&project(a))
            .then_with(|| a.repo.cmp(&b.repo))
            .then_with(|| b.tree.main.cmp(&a.tree.main))
            .then_with(|| b.last_worked().cmp(&a.last_worked()))
            .then_with(|| a.tree.path.cmp(&b.tree.path))
    });
    trees
}

/// Whether `folder` may hold projects to look through: no repository itself, and the home folder
/// or a folder under it (outside it, `/` or `/tmp`, nothing is searched).
/// `~/Library` never: it holds other apps' data, and reading it makes macOS ask for permissions.
pub(super) fn is_folder_of_projects(folder: &Path) -> bool {
    folder_of_projects_under(folder, &crate::launch::home_dir())
}

fn folder_of_projects_under(folder: &Path, home: &Path) -> bool {
    folder.starts_with(home) && !folder.starts_with(home.join("Library")) && tree_root(folder).is_none() && folder.is_dir()
}

/// Name of the linked working tree `path` is in (`None` in a project's own tree or outside git).
pub fn linked_tree_name(path: &Path) -> Option<String> {
    let root: PathBuf = tree_root(path)?;
    agentty_bridge::worktree::is_linked(&root).then(|| root.file_name().map(|n| n.to_string_lossy().to_string())).flatten()
}

/// The worktrees of every project under a folder that is no repository itself (the home folder, a
/// folder of projects): what a pane opened there offers to switch to. Only projects that have
/// linked worktrees are kept, each with its own folder first.
#[derive(Default)]
pub struct FolderScan {
    /// The last answer (`None` before the first one).
    pub trees: Option<Vec<agentty_bridge::inventory::TreeStatus>>,
    pub scanning: bool,
    /// A scan was asked for while one ran (a tree was removed meanwhile): the running one's answer
    /// may already be out of date, so another follows it at once.
    again: bool,
    /// When the last answer came: an answer older than [`FOLDER_SCAN_FRESH`] is looked for again.
    at: Option<std::time::Instant>,
}

/// How long a folder's list of worktrees is trusted before it is looked for again.
const FOLDER_SCAN_FRESH: std::time::Duration = std::time::Duration::from_secs(300);

impl FolderScan {
    /// (projects, linked worktrees) found.
    pub fn counts(&self) -> (usize, usize) {
        let trees = self.trees.as_deref().unwrap_or_default();
        let linked = trees.iter().filter(|t| !t.tree.main).count();
        let mut repos: Vec<&PathBuf> = trees.iter().map(|t| &t.repo).collect();
        repos.dedup();
        (repos.len(), linked)
    }
}

impl Workbench {
    fn pane_by_entity(&self, id: gpui::EntityId) -> Option<super::Pane> {
        self.all_panes().into_iter().find(|pane| pane.entity_id() == id)
    }

    /// The folder `pane` is in when it is no repository but may hold some: a folder under the home
    /// folder (or the home folder itself). Outside it (`/`, `/tmp`) nothing is searched.
    pub(super) fn scan_folder_of(&self, pane: &super::Pane, cx: &gpui::App) -> Option<PathBuf> {
        let cwd = pane.read(cx).display_cwd();
        self.is_folder_of_projects_cached(&cwd).then_some(cwd)
    }

    /// [`is_folder_of_projects`], remembered for a few seconds (renders ask it every frame).
    pub(super) fn is_folder_of_projects_cached(&self, folder: &Path) -> bool {
        let mut kinds = self.folder_kinds.borrow_mut();
        if let Some((answer, _)) = kinds.get(folder).filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(5)) {
            return *answer;
        }
        if kinds.len() > 64 {
            kinds.clear();
        }
        let answer = is_folder_of_projects(folder);
        kinds.insert(folder.to_path_buf(), (answer, std::time::Instant::now()));
        answer
    }

    pub(super) fn folder_scan(&self, folder: &Path) -> Option<&FolderScan> {
        self.folder_scans.get(folder)
    }

    /// From a render: starts a scan of `folder` right after it when there is no answer yet, or the
    /// last one is old (trees come and go outside Agentty too).
    pub(super) fn want_folder_scan(&self, folder: &Path, cx: &mut Context<Self>) {
        let fresh =
            self.folder_scans.get(folder).is_some_and(|scan| scan.scanning || scan.at.is_some_and(|at| at.elapsed() < FOLDER_SCAN_FRESH));
        if fresh {
            return;
        }
        let (this, folder) = (cx.entity().downgrade(), folder.to_path_buf());
        cx.defer(move |cx| {
            let _ = this.update(cx, |this, cx| this.scan_folder(folder, cx));
        });
    }

    /// Looks for the projects under `folder` and their worktrees, in the background (the home folder
    /// takes a few seconds). The last answer stays on screen meanwhile.
    pub(super) fn scan_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        // Folders no pane is in any more are forgotten.
        let open: std::collections::HashSet<PathBuf> = self.all_panes().iter().map(|pane| pane.read(cx).display_cwd()).collect();
        self.folder_scans.retain(|known, _| known == &folder || open.contains(known));
        let scan = self.folder_scans.entry(folder.clone()).or_default();
        if scan.scanning {
            scan.again = true;
            return;
        }
        scan.scanning = true;
        cx.notify();
        let source = folder.clone();
        let task = cx.background_spawn(async move {
            let never = std::sync::atomic::AtomicBool::new(false);
            // Only projects with linked worktrees are asked about their trees (git runs per tree):
            // counting them reads a folder, no git process.
            let repos: Vec<PathBuf> = agentty_bridge::inventory::repos_below(&source, &never)
                .into_iter()
                .filter(|repo| agentty_bridge::worktree::linked_count(repo) > 0)
                .collect();
            let trees = agentty_bridge::inventory::all_trees(&repos, &never);
            let with_linked: std::collections::HashSet<PathBuf> =
                trees.iter().filter(|t| !t.tree.main && !t.tree.prunable).map(|t| t.repo.clone()).collect();
            let trees: Vec<_> = trees.into_iter().filter(|t| with_linked.contains(&t.repo) && !t.tree.prunable).collect();
            most_recent_first(trees)
        });
        cx.spawn(async move |this, cx| {
            let trees = task.await;
            let _ = this.update(cx, |this, cx| {
                let scan = this.folder_scans.entry(folder.clone()).or_default();
                scan.trees = Some(trees);
                scan.scanning = false;
                scan.at = Some(std::time::Instant::now());
                if std::mem::take(&mut scan.again) {
                    this.scan_folder(folder.clone(), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Moves the pane to `tree`: its shell `cd`s there when it waits at its prompt. A pane busy with
    /// an agent (or anything else) can't be moved: the tree opens in a new tab instead, and says so.
    pub(super) fn switch_pane_to_tree(&mut self, pane: gpui::EntityId, tree: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.pane_by_entity(pane) else { return };
        if pane.read(cx).can_move() {
            pane.update(cx, |view, _| view.cd_to(&tree));
            self.focus_pane(&pane, window, cx);
            return;
        }
        let name = tree.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.open_tree_tab(Some(pane.entity_id()), tree, window, cx);
        // Windows can't tell whether something runs: a new tab there is the rule, nothing to explain.
        if cfg!(unix) {
            self.show_toast(tf(cx, "worktree.opened_in_tab", &[("name", &name)]), cx);
        }
    }

    /// Opens `tree` in a new tab: the same kind of session as `from` (an agent next to an agent), a
    /// terminal otherwise.
    pub(super) fn open_tree_tab(&mut self, from: Option<gpui::EntityId>, tree: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // An agent next to an agent; a terminal (whatever it was launched as) next to a terminal.
        let kind = from.and_then(|id| self.pane_by_entity(id)).and_then(|pane| pane.read(cx).agent_kind()).unwrap_or(PaneKind::Shell);
        self.open_tab(crate::launch::LaunchSpec::new(kind, tree), window, cx);
    }

    /// "Clean up this worktree": a tree with nothing to lose (merged, no changes) goes at once; one
    /// with changes or unmerged commits only after the user saw what would be lost. Either way the
    /// pane goes back to the project folder. Never while an agent works in it — this pane's or another's.
    pub(super) fn clean_up_tree(&mut self, pane: gpui::EntityId, cx: &mut Context<Self>) {
        let Some(view) = self.pane_by_entity(pane) else { return };
        let Some(tree) = tree_root(&view.read(cx).display_cwd()).filter(|root| agentty_bridge::worktree::is_linked(root)) else { return };
        let name = tree.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(reason) = self.cleanup_blocked(pane, &tree, cx) {
            return self.show_toast(reason, cx);
        }
        let source = tree.clone();
        let task = cx.background_spawn(async move { agentty_bridge::worktree::cleanup_check(&source) });
        cx.spawn(async move |this, cx| {
            let check = task.await;
            let _ = this.update(cx, |this, cx| match check {
                Ok(check) if check.is_safe() => this.clean_up_tree_now(pane, tree, false, cx),
                Ok(check) => this.ask_to_clean_up(pane, tree, check, cx),
                Err(err) => this.show_toast(tf(cx, "worktree.remove_failed", &[("name", &name), ("error", &format!("{err:#}"))]), cx),
            });
        })
        .detach();
    }

    /// Why `tree` can't be cleaned up from `pane` right now, if it can't: an agent works in this pane
    /// or something else runs in it (it couldn't be moved out and would be left in a removed folder),
    /// or another pane works in the tree. Checked again when the user confirms: that can take a while.
    fn cleanup_blocked(&self, pane: gpui::EntityId, tree: &Path, cx: &Context<Self>) -> Option<String> {
        let name = tree.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // The pane asked from was closed meanwhile (the dialog stays open): nothing is removed.
        let Some(view) = self.pane_by_entity(pane) else { return Some(tf(cx, "worktree.clean_pane_gone", &[("name", &name)])) };
        let view = view.read(cx);
        if view.is_agent() {
            return Some(tf(cx, "worktree.clean_agent", &[("name", &name)]));
        }
        // The pane must be moved out first, and only a shell waiting at its prompt can be (Windows
        // has no foreground process group to tell, and the item is not offered there).
        if !view.can_move() {
            return Some(tf(cx, "worktree.clean_busy", &[("name", &name)]));
        }
        // No project folder to go back to (a bare repository, `--separate-git-dir`): stay.
        if agentty_bridge::worktree::main_tree(tree).is_none() {
            return Some(tf(cx, "worktree.no_project_folder", &[("name", &name)]));
        }
        let in_use = self
            .panes_everywhere(cx)
            .iter()
            .any(|other| other.entity_id() != pane && tree_root(&other.read(cx).display_cwd()).as_deref() == Some(tree));
        in_use.then(|| tf(cx, "worktree.clean_in_use", &[("name", &name)]))
    }

    /// The panes of every Agentty window: a tree an agent works in from another window is in use too.
    /// This window's own come from `self`; its window is skipped by id, never read (gpui panics on a
    /// read of the window being drawn or updated).
    pub(super) fn panes_everywhere(&self, cx: &Context<Self>) -> Vec<super::Pane> {
        let mut panes = self.all_panes();
        for window in cx.windows() {
            if window.window_id() == self.window_id {
                continue;
            }
            let Some(window) = window.downcast::<Workbench>() else { continue };
            if let Ok(other) = window.read(cx) {
                panes.extend(other.all_panes());
            }
        }
        panes
    }

    /// Says what cleaning up `tree` would lose and asks.
    fn ask_to_clean_up(
        &mut self,
        pane: gpui::EntityId,
        tree: PathBuf,
        check: agentty_bridge::worktree::CleanupCheck,
        cx: &mut Context<Self>,
    ) {
        let name = tree.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut losses = Vec::new();
        if check.changed_files > 0 {
            losses.push(tf(cx, "worktree.clean_changes", &[("n", &check.changed_files.to_string())]));
        }
        if !check.ignored_files.is_empty() {
            let mut names = check.ignored_files.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            if check.ignored_files.len() > 3 {
                names.push_str(", …");
            }
            losses.push(tf(cx, "worktree.clean_ignored", &[("n", &check.ignored_files.len().to_string()), ("names", &names)]));
        }
        match &check.default_branch {
            Some(base) if check.unmerged_commits > 0 => {
                losses.push(tf(cx, "worktree.clean_commits", &[("n", &check.unmerged_commits.to_string()), ("base", base)]))
            }
            Some(_) => {}
            None => losses.push(t(cx, "worktree.clean_no_base").to_string()),
        }
        let project = agentty_bridge::worktree::main_tree(&tree)
            .and_then(|main| main.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        let branch = check.branch.clone().unwrap_or_else(|| name.clone());
        let ask = Ask {
            title: tf(cx, "worktree.clean_title", &[("name", &name)]).into(),
            body: Some({
                // "Not merged yet" only when it isn't; changes alone are said as changes.
                let unmerged = check.unmerged_commits > 0 || check.default_branch.is_none();
                let lead = t(cx, if unmerged { "worktree.clean_lead_unmerged" } else { "worktree.clean_lead_changes" });
                let rest = tf(cx, "worktree.clean_body", &[("losses", &losses.join(" · ")), ("branch", &branch), ("project", &project)]);
                format!("{lead} {rest}").into()
            }),
            choices: vec![AskChoice {
                label: t(cx, "worktree.clean_confirm").into(),
                action: AskAction::CleanTree { pane, tree },
                primary: false,
                danger: true,
            }],
            cancel: true,
        };
        self.ask(ask, cx);
    }

    /// Moves `pane` to the project folder, then removes `tree` and its branch (`force`: whatever it
    /// holds — the user said yes).
    pub(super) fn clean_up_tree_now(&mut self, pane: gpui::EntityId, tree: PathBuf, force: bool, cx: &mut Context<Self>) {
        let name = tree.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(reason) = self.cleanup_blocked(pane, &tree, cx) {
            return self.show_toast(reason, cx);
        }
        // First out of the folder: a shell left in a removed folder shows it as `.`. The `cd` is typed
        // into the shell, so the removal waits until the shell is really there (up to 2 s).
        let main = agentty_bridge::worktree::main_tree(&tree);
        let moving = main.clone().zip(self.pane_by_entity(pane).filter(|view| view.read(cx).can_move()));
        if let Some((main, view)) = &moving {
            view.update(cx, |view, _| view.cd_to(main));
        }
        let source = tree.clone();
        cx.spawn(async move |this, cx| {
            if let Some((main, view)) = moving {
                let target = main.canonicalize().unwrap_or(main);
                let mut arrived = false;
                for _ in 0..20 {
                    arrived =
                        view.read_with(cx, |view, _| view.current_dir().canonicalize().is_ok_and(|dir| dir == target)).unwrap_or(true);
                    if arrived {
                        break;
                    }
                    cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                }
                // Still in the tree (a slow prompt, a `cd` that failed): removing it now would leave
                // the shell in a folder that is gone. Nothing is removed.
                if !arrived {
                    let _ = this.update(cx, |this, cx| this.show_toast(tf(cx, "worktree.clean_not_moved", &[("name", &name)]), cx));
                    return;
                }
            }
            let result = cx.background_spawn(async move { agentty_bridge::worktree::clean_up(&source, force) }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(done) => {
                        let project = done.main.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        this.show_toast(tf(cx, "worktree.cleaned", &[("name", &name), ("project", &project)]), cx);
                    }
                    Err(err) => this.show_toast(tf(cx, "worktree.remove_failed", &[("name", &name), ("error", &format!("{err:#}"))]), cx),
                }
                this.refresh_files_panel(cx);
                for view in this.all_panes() {
                    view.update(cx, |view, cx| view.probe_git(cx));
                }
                // Lists of folders of projects may hold this tree: look again.
                let folders: Vec<PathBuf> = this.folder_scans.keys().cloned().collect();
                for folder in folders {
                    this.scan_folder(folder, cx);
                }
            });
        })
        .detach();
    }

    /// Linked worktrees around the active pane: of the repository it works in, else — in a folder
    /// that is no repository (the home folder) — of the projects under it, with whether that search
    /// is still running.
    pub(super) fn worktrees_around_active(&self, cx: &mut Context<Self>) -> (usize, bool) {
        let Some(pane) = self.active_pane() else { return (0, false) };
        let linked = pane.read(cx).linked_trees;
        if linked > 0 {
            return (linked, false);
        }
        let Some(folder) = self.scan_folder_of(&pane, cx) else { return (0, false) };
        // Projects under a folder are looked through only while the Files panel shows them (the user
        // opened it): never on their own, as reading folders like ~/Documents makes macOS ask first.
        if self.files_panel.is_some() {
            self.want_folder_scan(&folder, cx);
        }
        let scan = self.folder_scan(&folder);
        let scanning = scan.map_or(self.files_panel.is_some(), |s| s.scanning);
        (scan.map_or(0, |s| s.counts().1), scanning)
    }

    /// The Files button at the end of the tab bar. With worktrees around the active pane the same
    /// tree icon turns purple (how many is in its tooltip): the Files panel is where they are
    /// listed and managed.
    pub(super) fn files_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let open = self.files_panel.is_some();
        let (count, _) = self.worktrees_around_active(cx);
        let on_click = cx.listener(|this, _: &gpui::ClickEvent, _, cx| this.toggle_files_panel(cx));
        if count == 0 {
            return super::chrome::header_icon("header-files", "list-tree", open, (t(cx, "files.title"), Some("⌥⌘B")), on_click);
        }
        let tooltip = format!("{} · {}", t(cx, "files.title"), tf(cx, "worktree.button_count", &[("n", &count.to_string())]));
        div()
            .id("header-files")
            .relative()
            .flex_shrink_0()
            .my_auto()
            .size(px(crate::ui::ICON_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .bg(hex_alpha(Chrome::PURPLE, if open { 0.32 } else { 0.18 }))
            .hover(|s| s.bg(hex_alpha(Chrome::PURPLE, 0.3)))
            .tooltip(crate::ui::Tooltip::text(tooltip, Some("⌥⌘B")))
            .child(icon("list-tree", crate::ui::IconSize::BUTTON, hex(Chrome::PURPLE)))
            .on_click(on_click)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Folders of projects are looked through: the home folder and folders under it that are no
    /// repository. Never a repository (or a folder inside one), `~/Library`, or a folder outside home.
    #[test]
    fn tells_folders_of_projects() {
        let home = std::env::temp_dir().join(format!("agentty-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let (code, repo, library) = (home.join("code"), home.join("code").join("app"), home.join("Library").join("Containers"));
        for dir in [&code, &repo.join("src"), &library] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert!(folder_of_projects_under(&home, &home));
        assert!(folder_of_projects_under(&code, &home));
        assert!(!folder_of_projects_under(&repo, &home), "a repository is no folder of projects");
        assert!(!folder_of_projects_under(&repo.join("src"), &home), "nor a folder inside one");
        assert!(!folder_of_projects_under(&library, &home), "~/Library is never looked through");
        assert!(!folder_of_projects_under(&std::env::temp_dir(), &home), "outside home");
        assert!(!folder_of_projects_under(&home.join("gone"), &home), "a folder that is not there");
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn projects_worked_on_last_come_first() {
        let tree = |repo: &str, path: &str, main: bool, at: i64| agentty_bridge::inventory::TreeStatus {
            repo: PathBuf::from(repo),
            tree: agentty_bridge::worktree::Worktree {
                path: PathBuf::from(path),
                branch: None,
                head: String::new(),
                main,
                managed: false,
                prunable: false,
            },
            base: String::new(),
            dirty: 0,
            ahead: 0,
            last_commit: at,
            last_change: 0,
            pr: None,
            pr_merged_here: false,
            size: None,
        };
        let sorted = most_recent_first(vec![
            tree("/a", "/a", true, 100),
            tree("/a", "/a-old", false, 50),
            tree("/b", "/b", true, 10),
            tree("/b", "/b-old", false, 20),
            tree("/b", "/b-new", false, 300),
        ]);
        let paths: Vec<&str> = sorted.iter().map(|t| t.tree.path.to_str().unwrap()).collect();
        // /b was worked on last (300); its own folder first, then its trees newest first.
        assert_eq!(paths, ["/b", "/b-new", "/b-old", "/a", "/a-old"]);
    }

    #[test]
    fn counts_projects_and_linked_trees() {
        let tree = |repo: &str, path: &str, main: bool| agentty_bridge::inventory::TreeStatus {
            repo: PathBuf::from(repo),
            tree: agentty_bridge::worktree::Worktree {
                path: PathBuf::from(path),
                branch: None,
                head: String::new(),
                main,
                managed: false,
                prunable: false,
            },
            base: String::new(),
            dirty: 0,
            ahead: 0,
            last_commit: 0,
            last_change: 0,
            pr: None,
            pr_merged_here: false,
            size: None,
        };
        let scan = FolderScan {
            trees: Some(vec![
                tree("/a", "/a", true),
                tree("/a", "/a-1", false),
                tree("/a", "/a-2", false),
                tree("/b", "/b", true),
                tree("/b", "/b-1", false),
            ]),
            scanning: false,
            again: false,
            at: None,
        };
        assert_eq!(scan.counts(), (2, 3));
        assert_eq!(FolderScan::default().counts(), (0, 0));
    }
}
