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

/// Name of the linked working tree `path` is in (`None` in a project's own tree or outside git).
pub fn linked_tree_name(path: &Path) -> Option<String> {
    let root: PathBuf = tree_root(path)?;
    root.join(".git").is_file().then(|| root.file_name().map(|n| n.to_string_lossy().to_string())).flatten()
}
