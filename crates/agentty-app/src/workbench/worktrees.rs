//! A working tree per agent session: a new agent session started in a working tree that another
//! agent already works in gets its own (`agentty_bridge::worktree`), so two sessions never edit the
//! same files. Shells, resumed sessions and the first session of a project stay where they are.

use super::{LaunchTarget, Workbench};
use crate::i18n::{t, tf};
use crate::launch::{LaunchChoice, PaneKind};
use agentty_bridge::worktree::tree_root;
use gpui::{AppContext, Context, Window};
use std::path::{Path, PathBuf};

/// Folder name and branch suffix for the session's tree.
fn label(choice: &LaunchChoice) -> String {
    match choice {
        LaunchChoice::Kind(PaneKind::Codex) | LaunchChoice::Model(PaneKind::Codex, _) => "codex".into(),
        LaunchChoice::Kind(_) | LaunchChoice::Model(..) => "claude".into(),
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

    /// Starts `choice` in a working tree of its own when the one at `cwd` is taken. Returns whether
    /// it took over the launch.
    pub(super) fn launch_in_own_tree(
        &mut self,
        choice: &LaunchChoice,
        target: LaunchTarget,
        cwd: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !crate::settings::settings(cx).auto_worktree || matches!(choice, LaunchChoice::Kind(PaneKind::Shell)) {
            return false;
        }
        let Some(root) = tree_root(cwd) else { return false };
        if !self.tree_is_taken(&root, cx) {
            return false;
        }
        self.show_toast(t(cx, "worktree.creating").to_string(), cx);
        let (choice, from, label) = (choice.clone(), cwd.to_path_buf(), label(choice));
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
        true
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
    use crate::agent_signal::browser_reply;
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
    let _ = holder.update(cx, |this, _, cx| this.show_toast(t(cx, "worktree.creating").to_string(), cx));
    let (cwd, label) = (request.cwd.clone(), request.label.clone());
    let task = cx.background_spawn(async move { agentty_bridge::worktree::create(&cwd, &label) });
    cx.spawn(async move |cx| {
        let created = task.await;
        let _ = holder.update(cx, |this, _, cx| match created {
            Ok(tree) => {
                // The same folder inside the new tree (a package of a monorepo, say).
                let inside = request.cwd.strip_prefix(&root).ok().map(|rest| tree.path.join(rest)).filter(|dir| dir.is_dir());
                let branch = tree.branch.clone().unwrap_or_else(|| tree.name());
                let answer = serde_json::json!({
                    "path": inside.unwrap_or_else(|| tree.path.clone()),
                    "message": tf(cx, "worktree.started_shell", &[("branch", &branch)]),
                });
                let _ = request.reply.send(browser_reply(Ok(answer.to_string())));
                this.show_toast(tf(cx, "worktree.started", &[("branch", &branch)]), cx);
                this.refresh_files_panel(cx);
            }
            Err(err) => {
                eprintln!("agentty: could not create a working tree: {err:#}");
                stay(&request);
                this.show_toast(tf(cx, "worktree.failed", &[("error", &format!("{err:#}"))]), cx);
            }
        });
    })
    .detach();
}

/// Name of the linked working tree `path` is in (`None` in a project's own tree or outside git).
pub fn linked_tree_name(path: &Path) -> Option<String> {
    let root: PathBuf = tree_root(path)?;
    root.join(".git").is_file().then(|| root.file_name().map(|n| n.to_string_lossy().to_string())).flatten()
}
