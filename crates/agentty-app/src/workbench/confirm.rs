//! "Close this?" confirmation for panes, tabs and workspaces (unless turned off in settings).

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::settings::{settings, update_settings};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use gpui::{div, prelude::*, px, ClickEvent, Context, Window};

#[derive(Clone)]
pub enum CloseTarget {
    Pane(Pane),
    /// Tabs of the active workspace, by their panes (indices shift while closing).
    Tabs(Vec<Pane>),
    Workspace(u64),
}

pub struct CloseConfirm {
    pub target: CloseTarget,
    pub dont_ask: bool,
    /// Closing it leaves the workspace without tabs (it stays in the list, folded back to this tab).
    pub removes_workspace: bool,
    /// Linked working trees only the closing panes work in: they can go with them.
    pub trees: Vec<std::path::PathBuf>,
    /// "Also remove these working trees and their branches" (off unless ticked).
    pub remove_trees: bool,
    /// The workspace has data in the sync repository: the dialog offers to delete it too.
    pub synced: bool,
    /// "Also delete sync data" (off unless ticked).
    pub delete_sync: bool,
}

impl Workbench {
    fn target_panes(&self, target: &CloseTarget) -> Vec<Pane> {
        match target {
            CloseTarget::Pane(pane) => vec![pane.clone()],
            CloseTarget::Tabs(panes) => panes.clone(),
            CloseTarget::Workspace(id) => {
                self.workspaces.iter().filter(|w| w.id == *id).flat_map(|w| w.tabs.iter().flat_map(|t| t.root.leaves())).collect()
            }
        }
    }

    /// True when closing the panes of `target` empties the workspace they belong to.
    fn empties_workspace(&self, target: &CloseTarget) -> bool {
        let panes = self.target_panes(target);
        let Some(first) = panes.first() else { return false };
        let Some((w, _)) = self.locate(first) else { return false };
        let ws = &self.workspaces[w];
        ws.dormant.is_none() && ws.tabs.iter().flat_map(|t| t.root.leaves()).all(|leaf| panes.contains(&leaf))
    }

    /// Linked working trees (not a project's own folder) that the panes of `target` work in and no
    /// other pane does: closing them can take the trees along.
    fn trees_left_behind(&self, target: &CloseTarget, cx: &gpui::App) -> Vec<std::path::PathBuf> {
        let closing = self.target_panes(target);
        let tree_of =
            |pane: &Pane| agentty_bridge::worktree::tree_root(&pane.read(cx).display_cwd()).filter(|root| root.join(".git").is_file());
        let mut trees: Vec<std::path::PathBuf> = closing.iter().filter_map(tree_of).collect();
        trees.sort();
        trees.dedup();
        let staying: Vec<std::path::PathBuf> = self.all_panes().iter().filter(|p| !closing.contains(p)).filter_map(tree_of).collect();
        trees.retain(|tree| !staying.contains(tree));
        trees
    }

    /// Asks before closing unless the user turned confirmations off in settings. Panes in a working
    /// tree of their own always ask: that is where the tree can go with them.
    pub(super) fn request_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        let trees = if settings(cx).ask_remove_trees { self.trees_left_behind(&target, cx) } else { Vec::new() };
        // Removing a synced workspace always asks: whether its sync data goes too.
        let synced = matches!(target, CloseTarget::Workspace(id) if self.workspace_synced(id));
        if !settings(cx).confirm_close && trees.is_empty() && !synced {
            return self.perform_close(target, window, cx);
        }
        let removes_workspace = !matches!(target, CloseTarget::Workspace(_)) && self.empties_workspace(&target);
        self.close_confirm =
            Some(CloseConfirm { target, dont_ask: false, removes_workspace, trees, remove_trees: false, synced, delete_sync: false });
        cx.notify();
    }

    /// After their panes closed: removes the working trees and their branches. Git keeps what would be
    /// lost — a tree with uncommitted changes, a branch with commits nothing else has — and a toast
    /// says what went and what stayed.
    fn remove_trees_after_close(&mut self, trees: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // The panes' processes leave the folders first.
            cx.background_executor().timer(std::time::Duration::from_millis(600)).await;
            let results = cx
                .background_spawn(async move {
                    trees
                        .into_iter()
                        .map(|tree| {
                            let name = tree.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            // Closing a tab never touches a remote branch.
                            (name, agentty_bridge::worktree::remove_linked(&tree, &tree, true, false))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let lines: Vec<String> = results
                    .iter()
                    .map(|(name, result)| match result {
                        Ok(removal) => match &removal.branch_kept {
                            None => tf(cx, "worktree.removed", &[("name", name)]),
                            Some(branch) => tf(cx, "worktree.removed_branch_kept", &[("name", name), ("branch", branch)]),
                        },
                        Err(err) if format!("{err:#}").contains("modified or untracked") => {
                            tf(cx, "worktree.kept_dirty", &[("name", name)])
                        }
                        Err(err) => tf(cx, "worktree.remove_failed", &[("name", name), ("error", &format!("{err:#}"))]),
                    })
                    .collect();
                this.show_toast_for(lines.join("\n"), 8000, cx);
                this.refresh_files_panel(cx);
            });
        })
        .detach();
    }

    pub(super) fn request_close_pane(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(CloseTarget::Pane(pane.clone()), window, cx);
    }

    pub(super) fn request_close_tabs(&mut self, indices: &[usize], window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get(self.active_workspace) else { return };
        let panes = indices.iter().filter_map(|i| ws.tabs.get(*i)).flat_map(|t| t.root.leaves()).collect();
        self.request_close(CloseTarget::Tabs(panes), window, cx);
    }

    pub(super) fn perform_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            CloseTarget::Workspace(id) => self.close_workspace(id, window, cx),
            other => {
                let panes = self.target_panes(&other);
                // A tab closing as a whole is remembered with its splits, so "recently closed tabs"
                // can put it back the way it was. Closing one split of a tab is not a tab closing.
                self.remember_closed_tabs(&panes, cx);
                let mut workspaces: Vec<u64> = panes.iter().filter_map(|p| self.locate(p)).map(|(w, _)| self.workspaces[w].id).collect();
                workspaces.dedup();
                for pane in panes {
                    self.remove_pane(&pane, cx);
                }
                // The sync repository records which sessions were closed.
                for id in workspaces {
                    self.sync_after_close(id, cx);
                }
                self.focus_active(window, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn render_close_confirm(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let confirm = self.close_confirm.as_ref()?;
        let (title, body) = match &confirm.target {
            _ if confirm.removes_workspace => (t(cx, "confirm.close_last_tab"), t(cx, "confirm.close_last_tab_body").to_string()),
            CloseTarget::Pane(_) => (t(cx, "confirm.close_pane"), t(cx, "confirm.close_pane_body").to_string()),
            CloseTarget::Tabs(panes) => (t(cx, "confirm.close_tab"), tf(cx, "confirm.close_tab_body", &[("n", &panes.len().to_string())])),
            CloseTarget::Workspace(id) => (
                t(cx, "confirm.close_workspace"),
                tf(
                    cx,
                    "confirm.close_workspace_body",
                    &[("name", &self.workspaces.iter().find(|w| w.id == *id).map(|w| self.workspace_title(w, cx)).unwrap_or_default())],
                ),
            ),
        };
        let dont_ask = confirm.dont_ask;
        let button = |id: &'static str, label: &str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ERROR) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(label.to_string())
        };
        Some(
            div()
                .id("close-confirm-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .child(
                    div()
                        .w(px(360.))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(title))
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(body))
                        .when(!confirm.trees.is_empty(), |d| {
                            let remove = confirm.remove_trees;
                            let names = confirm
                                .trees
                                .iter()
                                .map(|t| t.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
                                .collect::<Vec<_>>()
                                .join(", ");
                            d.child(
                                div()
                                    .id("close-confirm-remove-trees")
                                    .flex()
                                    .items_start()
                                    .gap_2()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Some(confirm) = this.close_confirm.as_mut() {
                                            confirm.remove_trees = !confirm.remove_trees;
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .mt(px(2.))
                                            .size(px(14.))
                                            .flex_shrink_0()
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(hex(if remove { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                                            .bg(if remove { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(remove, |d| d.child(crate::ui::icon("check", 11., hex(Chrome::BRIGHT)))),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(tf(
                                                cx,
                                                "confirm.remove_trees",
                                                &[("n", &confirm.trees.len().to_string())],
                                            )))
                                            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(names))
                                            .child(
                                                div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "confirm.remove_trees_hint")),
                                            ),
                                    ),
                            )
                        })
                        .when(confirm.synced, |d| {
                            let delete = confirm.delete_sync;
                            d.child(
                                div()
                                    .id("close-confirm-delete-sync")
                                    .flex()
                                    .items_start()
                                    .gap_2()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Some(confirm) = this.close_confirm.as_mut() {
                                            confirm.delete_sync = !confirm.delete_sync;
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .mt(px(2.))
                                            .size(px(14.))
                                            .flex_shrink_0()
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(hex(if delete { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                                            .bg(if delete { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(delete, |d| d.child(crate::ui::icon("check", 11., hex(Chrome::BRIGHT)))),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "confirm.delete_sync")))
                                            .child(
                                                div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "confirm.delete_sync_hint")),
                                            ),
                                    ),
                            )
                        })
                        // Asked only because the workspace is synced: there is no question to silence.
                        .when(settings(cx).confirm_close || !confirm.trees.is_empty(), |d| {
                            d.child(
                                div()
                                    .id("close-confirm-dont-ask")
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .t_small()
                                    .text_color(hex(Chrome::FOREGROUND))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Some(confirm) = this.close_confirm.as_mut() {
                                            confirm.dont_ask = !confirm.dont_ask;
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .size(px(14.))
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(hex(if dont_ask { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                                            .bg(if dont_ask { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(dont_ask, |d| d.child(crate::ui::icon("check", 11., hex(Chrome::BRIGHT)))),
                                    )
                                    .child(t(cx, "confirm.dont_ask")),
                            )
                        })
                        .child(
                            div()
                                .pt_1()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(button("close-confirm-cancel", t(cx, "confirm.cancel"), false).on_click(cx.listener(
                                    |this, _: &ClickEvent, window, cx| {
                                        this.close_confirm = None;
                                        this.focus_active(window, cx);
                                        cx.notify();
                                    },
                                )))
                                .child(button("close-confirm-ok", t(cx, "confirm.close"), true).on_click(cx.listener(
                                    |this, _: &ClickEvent, window, cx| {
                                        let Some(confirm) = this.close_confirm.take() else { return };
                                        if confirm.dont_ask {
                                            // Asked only because a working tree would be left
                                            // behind? Then that is the question to silence — the
                                            // user's "ask before closing" setting stays as it is.
                                            let only_trees = !settings(cx).confirm_close;
                                            update_settings(cx, |s| {
                                                if only_trees {
                                                    s.ask_remove_trees = false;
                                                } else {
                                                    s.confirm_close = false;
                                                }
                                            });
                                        }
                                        if let (CloseTarget::Workspace(id), true) = (&confirm.target, confirm.delete_sync) {
                                            this.sync_workspace_removed(*id, true, cx);
                                        }
                                        this.perform_close(confirm.target, window, cx);
                                        if confirm.remove_trees && !confirm.trees.is_empty() {
                                            this.remove_trees_after_close(confirm.trees, cx);
                                        }
                                    },
                                ))),
                        ),
                ),
        )
    }
}
