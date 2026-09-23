//! Monitoring → Worktrees: every git working tree on this computer, grouped by project, with when
//! it was last worked on, uncommitted changes, commits the default branch doesn't have, its pull
//! request and its size — and removing several at once. The project's own tree is listed but never
//! removable, and neither is a tree an open pane works in.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use crate::usage_view::{card, kpi};
use agentty_bridge::inventory::TreeStatus;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, SharedString};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum TreeFilter {
    #[default]
    All,
    /// Trees Agentty made for sessions.
    Agentty,
    /// Nothing would be lost if they went.
    Removable,
}

/// "Remove these?" with its options.
pub struct TreeRemoval {
    pub trees: Vec<PathBuf>,
    /// Their branches too, when nothing on them would be lost.
    pub delete_branch: bool,
    /// Even with uncommitted changes (off unless ticked).
    pub force: bool,
}

#[derive(Default)]
pub struct TreeManager {
    scroll: gpui::ScrollHandle,
    trees: Vec<TreeStatus>,
    /// A scan is running; which phase shows in the header.
    loading: bool,
    prs_loading: bool,
    sizes_loading: bool,
    loaded: bool,
    /// Bumped by every scan, so an older scan's late answer is dropped.
    generation: u64,
    selected: HashSet<PathBuf>,
    filter: TreeFilter,
    pub(super) confirm: Option<TreeRemoval>,
    removing: bool,
    updated: Option<chrono::DateTime<chrono::Local>>,
}

/// `1.5 GB`.
pub(super) fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} B"),
        _ if value >= 100. => format!("{value:.0} {}", UNITS[unit]),
        _ => format!("{value:.1} {}", UNITS[unit]),
    }
}

/// A square check box (the same as the close dialog's).
pub(super) fn check_box(checked: bool, enabled: bool) -> gpui::Div {
    div()
        .size(px(14.))
        .flex_shrink_0()
        .rounded_sm()
        .border_1()
        .border_color(hex(if checked { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
        .bg(if checked { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
        .when(!enabled, |d| d.opacity(0.35))
        .flex()
        .items_center()
        .justify_center()
        .when(checked, |d| d.child(icon("check", 11., hex(Chrome::BRIGHT))))
}

/// A small coloured label (`2 uncommitted`, `PR #12 merged`).
pub(super) fn badge(text: impl Into<SharedString>, color: u32) -> gpui::Div {
    div().flex_shrink_0().px_1p5().rounded_sm().t_caption().bg(hex_alpha(color, 0.16)).text_color(hex(color)).child(text.into())
}

/// A dialog over a page: title, body, then Cancel and a red confirm button.
pub(super) fn confirm_dialog(
    id: &'static str,
    title: String,
    body: Vec<AnyElement>,
    confirm_label: String,
    on_cancel: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    on_confirm: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    cancel_label: String,
) -> AnyElement {
    let button = |id: &'static str, label: String, primary: bool| {
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
            .child(label)
    };
    div()
        .id(id)
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(hex_alpha(0x000000, 0.45))
        .occlude()
        .child(
            div()
                .w(px(420.))
                .max_w(gpui::relative(0.9))
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
                .children(body)
                .child(
                    div()
                        .pt_1()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(button("confirm-dialog-cancel", cancel_label, false).on_click(on_cancel))
                        .child(button("confirm-dialog-ok", confirm_label, true).on_click(on_confirm)),
                ),
        )
        .into_any_element()
}

fn now_secs() -> i64 {
    chrono::Local::now().timestamp()
}

/// `3d ago`-style, from seconds since the epoch.
fn ago(cx: &gpui::App, secs: i64) -> String {
    if secs <= 0 {
        return "—".into();
    }
    let minutes = (now_secs() - secs).max(0) / 60;
    match minutes {
        0 => t(cx, "trees.just_now").to_string(),
        1..=59 => tf(cx, "trees.minutes_ago", &[("n", &minutes.to_string())]),
        60..=1439 => tf(cx, "trees.hours_ago", &[("n", &(minutes / 60).to_string())]),
        _ => tf(cx, "trees.days_ago", &[("n", &(minutes / 1440).to_string())]),
    }
}

impl Workbench {
    /// Folders the open workspaces and panes are in: their repositories are listed even when they
    /// sit outside the folders searched.
    pub(super) fn known_project_folders(&self, cx: &gpui::App) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = self.workspaces.iter().map(|ws| ws.cwd.clone()).collect();
        folders.extend(self.all_panes().iter().map(|pane| pane.read(cx).display_cwd()));
        folders.sort();
        folders.dedup();
        folders
    }

    /// Trees an open pane works in: removing them would pull the folder out from under it.
    fn trees_in_use(&self, cx: &gpui::App) -> HashSet<PathBuf> {
        self.all_panes()
            .iter()
            .filter_map(|pane| agentty_bridge::worktree::tree_root(&pane.read(cx).display_cwd()))
            .map(|root| root.canonicalize().unwrap_or(root))
            .collect()
    }

    /// Lists every tree, then fills in pull requests, then sizes — each shows as soon as it's in.
    pub(super) fn scan_trees(&mut self, cx: &mut Context<Self>) {
        let manager = &mut self.tree_manager;
        manager.generation += 1;
        let generation = manager.generation;
        manager.loading = true;
        manager.loaded = true;
        let known = self.known_project_folders(cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let background = cx.background_executor().clone();
            let trees = background
                .spawn(async move {
                    let never = AtomicBool::new(false);
                    let repos = agentty_bridge::inventory::find_repos(&known, &never);
                    let mut trees = agentty_bridge::inventory::all_trees(&repos, &never);
                    // Resolved once here, so telling a tree an open pane is in costs nothing per frame.
                    for tree in &mut trees {
                        if let Ok(path) = tree.tree.path.canonicalize() {
                            tree.tree.path = path;
                        }
                    }
                    trees
                })
                .await;
            let current = |this: &mut Workbench| this.tree_manager.generation == generation;
            let keep = this
                .update(cx, |this, cx| {
                    if !current(this) {
                        return false;
                    }
                    let manager = &mut this.tree_manager;
                    // Selections of trees that are gone go with them.
                    let listed: HashSet<PathBuf> = trees.iter().map(|t| t.tree.path.clone()).collect();
                    manager.selected.retain(|p| listed.contains(p));
                    manager.trees = trees.clone();
                    manager.loading = false;
                    manager.prs_loading = true;
                    manager.sizes_loading = true;
                    manager.updated = Some(chrono::Local::now());
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !keep {
                return;
            }
            let mut with_prs = trees.clone();
            let with_prs = background
                .spawn(async move {
                    agentty_bridge::inventory::attach_pull_requests(&mut with_prs);
                    with_prs
                })
                .await;
            let prs: Vec<(PathBuf, Option<agentty_bridge::github::PullRequest>, bool)> =
                with_prs.iter().map(|t| (t.tree.path.clone(), t.pr.clone(), t.pr_merged_here)).collect();
            let _ = this.update(cx, |this, cx| {
                if !current(this) {
                    return;
                }
                for tree in &mut this.tree_manager.trees {
                    if let Some((_, pr, merged)) = prs.iter().find(|(p, ..)| *p == tree.tree.path) {
                        tree.pr = pr.clone();
                        tree.pr_merged_here = *merged;
                    }
                }
                this.tree_manager.prs_loading = false;
                cx.notify();
            });
            let paths: Vec<PathBuf> = trees.iter().filter(|t| t.tree.path.is_dir()).map(|t| t.tree.path.clone()).collect();
            let sizes = background
                .spawn(async move {
                    let never = AtomicBool::new(false);
                    agentty_bridge::disk::in_parallel(paths, 4, |path| {
                        let size = agentty_bridge::disk::size_of(&path, &never);
                        (path, size)
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !current(this) {
                    return;
                }
                for tree in &mut this.tree_manager.trees {
                    tree.size = sizes.iter().find(|(p, _)| *p == tree.tree.path).map(|(_, s)| *s);
                }
                this.tree_manager.sizes_loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Removes the trees the dialog was confirmed for, one after another, then lists again.
    fn remove_selected_trees(&mut self, removal: TreeRemoval, cx: &mut Context<Self>) {
        let jobs: Vec<(PathBuf, PathBuf, bool, Option<String>)> = removal
            .trees
            .iter()
            .filter_map(|path| self.tree_manager.trees.iter().find(|t| &t.tree.path == path))
            .filter(|t| !t.tree.main)
            .map(|t| (t.repo.clone(), t.tree.path.clone(), t.dirty > 0, t.pr_merged_here.then(|| t.tree.head.clone())))
            .collect();
        let (delete_branch, force) = (removal.delete_branch, removal.force);
        self.tree_manager.removing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (removed, skipped, kept, failed) = cx
                .background_executor()
                .spawn(async move {
                    let (mut removed, mut skipped, mut kept, mut failed) = (0usize, 0usize, 0usize, Vec::new());
                    for (repo, tree, dirty, merged_head) in jobs {
                        if dirty && !force {
                            skipped += 1;
                            continue;
                        }
                        match agentty_bridge::inventory::remove(&repo, &tree, delete_branch, force, merged_head.as_deref()) {
                            Ok(result) => {
                                removed += 1;
                                kept += usize::from(result.branch_kept.is_some());
                            }
                            Err(err) => {
                                failed.push(format!("{}: {err:#}", tree.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()))
                            }
                        }
                    }
                    (removed, skipped, kept, failed)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.tree_manager.removing = false;
                this.tree_manager.selected.clear();
                let mut text = tf(cx, "trees.removed", &[("n", &removed.to_string())]);
                if skipped > 0 {
                    text.push_str(&tf(cx, "trees.removed_skipped", &[("n", &skipped.to_string())]));
                }
                if kept > 0 {
                    text.push_str(&tf(cx, "trees.removed_branch_kept", &[("n", &kept.to_string())]));
                }
                if let Some(first) = failed.first() {
                    text.push_str(&tf(cx, "trees.removed_failed", &[("n", &failed.len().to_string()), ("error", first)]));
                }
                this.show_toast_for(text, 6000, cx);
                this.scan_trees(cx);
            });
        })
        .detach();
    }

    pub(super) fn render_tree_manager(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.tree_manager.loaded {
            self.scan_trees(cx);
        }
        let scroll = self.tree_manager.scroll.clone();
        let in_use = self.trees_in_use(cx);
        let used = |tree: &TreeStatus| in_use.contains(&tree.tree.path);
        let manager = &self.tree_manager;
        let removable = |tree: &TreeStatus| !tree.tree.main && !used(tree);

        // Totals.
        let linked: Vec<&TreeStatus> = manager.trees.iter().filter(|t| !t.tree.main).collect();
        let safe = linked.iter().filter(|t| t.safe_to_remove() && !used(t)).count();
        let dirty = linked.iter().filter(|t| t.dirty > 0).count();
        let size: u64 = linked.iter().filter_map(|t| t.size).sum();
        // Projects with trees besides their own folder: the others have nothing to manage here.
        let with_trees: HashSet<&PathBuf> = linked.iter().map(|t| &t.repo).collect();
        let repos = with_trees.len();

        let loading_note = if manager.loading {
            Some(t(cx, "trees.scanning"))
        } else if manager.prs_loading {
            Some(t(cx, "trees.loading_prs"))
        } else if manager.sizes_loading {
            Some(t(cx, "trees.measuring"))
        } else {
            None
        };
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().t_heading().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.worktrees")))
            .children(loading_note.map(|note| {
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(crate::ui::spinner(IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(note))
            }))
            .child(div().flex_1())
            .children(manager.updated.map(|at| {
                div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "processes.updated",
                    &[("time", &at.format("%H:%M:%S").to_string())],
                ))
            }))
            .child(
                crate::ui::icon_only(
                    "trees-refresh",
                    "rotate-cw",
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if !this.tree_manager.loading {
                            this.scan_trees(cx);
                        }
                    }),
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, "trees.refresh"), None)),
            );

        let kpis = div()
            .flex()
            .flex_wrap()
            .gap_3()
            .child(kpi(
                t(cx, "trees.kpi_total"),
                linked.len().to_string(),
                tf(cx, "trees.kpi_total_sub", &[("n", &repos.to_string())]),
                Chrome::BLUE,
            ))
            .child(kpi(t(cx, "trees.kpi_safe"), safe.to_string(), t(cx, "trees.kpi_safe_sub").to_string(), Chrome::SUCCESS))
            .child(kpi(t(cx, "trees.kpi_dirty"), dirty.to_string(), t(cx, "trees.kpi_dirty_sub").to_string(), Chrome::WARNING))
            .child(kpi(
                t(cx, "trees.kpi_size"),
                if manager.sizes_loading && size == 0 { "…".into() } else { size_text(size) },
                t(cx, "trees.kpi_size_sub").to_string(),
                Chrome::PURPLE,
            ));

        // Toolbar: filters, select the safe ones, remove the selection.
        let filter = manager.filter;
        let chip = |id: &'static str, key: &'static str, value: TreeFilter, cx: &mut Context<Self>| {
            crate::ui::chip(
                id,
                t(cx, key),
                filter == value,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.tree_manager.filter = value;
                    cx.notify();
                }),
            )
        };
        let selectable_safe: Vec<PathBuf> =
            linked.iter().filter(|t| t.safe_to_remove() && removable(t)).map(|t| t.tree.path.clone()).collect();
        let selected = manager.selected.len();
        let removing = manager.removing;
        let toolbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(chip("trees-filter-all", "trees.filter_all", TreeFilter::All, cx))
            .child(chip("trees-filter-agentty", "trees.filter_agentty", TreeFilter::Agentty, cx))
            .child(chip("trees-filter-safe", "trees.filter_safe", TreeFilter::Removable, cx))
            .child(div().flex_1())
            .when(!selectable_safe.is_empty(), |d| {
                d.child(crate::ui::action_button(
                    "trees-select-safe",
                    tf(cx, "trees.select_safe", &[("n", &selectable_safe.len().to_string())]),
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.tree_manager.selected.extend(selectable_safe.iter().cloned());
                        cx.notify();
                    }),
                ))
            })
            .when(selected > 0, |d| {
                d.child(crate::ui::action_button(
                    "trees-clear-selection",
                    t(cx, "trees.clear_selection"),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.tree_manager.selected.clear();
                        cx.notify();
                    }),
                ))
            })
            .child(
                div()
                    .id("trees-remove-selected")
                    .flex_shrink_0()
                    .px_2p5()
                    .py_1()
                    .rounded_md()
                    .t_small()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .bg(hex(if selected > 0 && !removing { Chrome::ERROR } else { 0x2d2d30 }))
                    .text_color(hex(if selected > 0 { Chrome::BRIGHT } else { Chrome::MUTED }))
                    .when(selected > 0 && !removing, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                    .child(if removing {
                        crate::ui::spinner(IconSize::INLINE, hex(Chrome::BRIGHT)).into_any_element()
                    } else {
                        icon("trash-2", IconSize::INLINE, hex(if selected > 0 { Chrome::BRIGHT } else { Chrome::MUTED })).into_any_element()
                    })
                    .child(tf(cx, "trees.remove_selected", &[("n", &selected.to_string())]))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if this.tree_manager.selected.is_empty() || this.tree_manager.removing {
                            return;
                        }
                        let mut trees: Vec<PathBuf> = this.tree_manager.selected.iter().cloned().collect();
                        trees.sort();
                        this.tree_manager.confirm = Some(TreeRemoval { trees, delete_branch: true, force: false });
                        cx.notify();
                    })),
            );

        // The table, one section per project.
        let columns = div()
            .px_4()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .child(div().w(px(14.)))
            .child(div().flex_1().min_w_0().child(t(cx, "trees.col_tree")))
            .child(div().w(px(250.)).child(t(cx, "trees.col_state")))
            .child(div().w(px(90.)).flex().justify_end().child(t(cx, "trees.col_last")))
            .child(div().w(px(76.)).flex().justify_end().child(t(cx, "trees.col_size")))
            .child(div().w(px(24.)));
        let mut table = card().flex().flex_col().overflow_hidden().child(columns);
        let shown: Vec<&TreeStatus> = manager
            .trees
            .iter()
            .filter(|t| with_trees.contains(&t.repo))
            .filter(|t| match filter {
                TreeFilter::All => true,
                TreeFilter::Agentty => t.tree.managed,
                TreeFilter::Removable => t.safe_to_remove() && removable(t),
            })
            .collect();
        if shown.is_empty() {
            table =
                table.child(crate::ui::hint(if manager.loading { t(cx, "trees.scanning") } else { t(cx, "trees.empty") }).px_4().py_4());
        }
        let mut current_repo: Option<&Path> = None;
        for tree in shown {
            if current_repo != Some(tree.repo.as_path()) {
                current_repo = Some(tree.repo.as_path());
                let name = tree.repo.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                table = table.child(
                    div()
                        .px_4()
                        .pt_2p5()
                        .pb_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .bg(hex(0x1f1f1f))
                        .child(icon("folder-open", 12., hex(Chrome::MUTED)))
                        .child(div().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::FOREGROUND)).child(name))
                        .child(div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(crate::ui::tilde(&tree.repo))),
                );
            }
            table = table.child(self.render_tree_row(tree, used(tree), cx));
        }

        let confirm = self.render_tree_confirm(cx);
        div()
            .size_full()
            .relative()
            .child(
                div().id("trees-page").size_full().bg(hex(Chrome::EDITOR)).overflow_y_scroll().track_scroll(&scroll).child(
                    div()
                        .px_6()
                        .py_5()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(header)
                        .child(kpis)
                        .child(toolbar)
                        .child(table)
                        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "trees.footnote"))),
                ),
            )
            .group(crate::ui::SCROLL_GROUP)
            .child(crate::ui::scrollbar(scroll))
            .children(confirm)
    }

    fn render_tree_row(&self, tree: &TreeStatus, in_use: bool, cx: &mut Context<Self>) -> AnyElement {
        let path = tree.tree.path.clone();
        let selectable = !tree.tree.main && !in_use;
        let checked = self.tree_manager.selected.contains(&path);
        let mut states = div().w(px(250.)).flex().flex_wrap().items_center().gap_1();
        if tree.tree.main {
            states = states.child(badge(t(cx, "trees.main"), Chrome::MUTED));
        }
        if in_use {
            states = states.child(badge(t(cx, "trees.in_use"), Chrome::BLUE));
        }
        if tree.tree.prunable {
            states = states.child(badge(t(cx, "trees.missing"), Chrome::WARNING));
        } else {
            if tree.dirty > 0 {
                states = states.child(badge(tf(cx, "trees.dirty", &[("n", &tree.dirty.to_string())]), Chrome::WARNING));
            }
            if !tree.tree.main {
                match &tree.pr {
                    Some(pr) => {
                        let (key, color) = if pr.is_merged() {
                            ("trees.pr_merged", Chrome::PURPLE)
                        } else if pr.is_open() {
                            ("trees.pr_open", Chrome::SUCCESS)
                        } else {
                            ("trees.pr_closed", Chrome::MUTED)
                        };
                        let url = pr.url.clone();
                        states = states.child(
                            div()
                                .id(SharedString::from(format!("tree-pr-{}", path.display())))
                                .cursor_pointer()
                                .on_click(move |_, _, cx| cx.open_url(&url))
                                .child(badge(tf(cx, key, &[("n", &pr.number.to_string())]), color)),
                        );
                    }
                    None if tree.ahead > 0 => {
                        states = states.child(badge(tf(cx, "trees.ahead", &[("n", &tree.ahead.to_string())]), Chrome::ORANGE));
                    }
                    None => {}
                }
                if tree.pr.as_ref().is_some_and(|pr| pr.is_merged()) && tree.ahead > 0 && !tree.pr_merged_here {
                    // Merged, but this tree has commits made after it.
                    states = states.child(badge(tf(cx, "trees.ahead", &[("n", &tree.ahead.to_string())]), Chrome::ORANGE));
                }
                if tree.safe_to_remove() {
                    states = states.child(badge(t(cx, "trees.safe"), Chrome::SUCCESS));
                }
            }
        }
        let branch = tree.tree.branch.clone().unwrap_or_else(|| {
            let short: String = tree.tree.head.chars().take(7).collect();
            tf(cx, "trees.detached", &[("head", &short)])
        });
        let name = tree.tree.name();
        let toggle = path.clone();
        let reveal = path.clone();
        div()
            .id(SharedString::from(format!("tree-row-{}", path.display())))
            .px_4()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(hex(0x2a2a2a))
            .when(checked, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.10)))
            .when(selectable, |d| {
                d.cursor_pointer().hover(|s| s.bg(hex(Chrome::HOVER))).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if !this.tree_manager.selected.remove(&toggle) {
                        this.tree_manager.selected.insert(toggle.clone());
                    }
                    cx.notify();
                }))
            })
            .child(check_box(checked, selectable))
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
                            .gap_1p5()
                            .min_w_0()
                            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(name))
                            .when(tree.tree.managed, |d| d.child(badge("Agentty", Chrome::ACCENT)))
                            .child(div().flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(icon(
                                "git-branch",
                                11.,
                                hex(Chrome::MUTED),
                            )))
                            .child(div().min_w_0().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(branch)),
                    )
                    .child(div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(crate::ui::tilde(&path))),
            )
            .child(states)
            .child(div().w(px(90.)).flex().justify_end().t_small().text_color(hex(Chrome::FOREGROUND)).child(ago(cx, tree.last_worked())))
            .child(
                div()
                    .w(px(76.))
                    .flex()
                    .justify_end()
                    .t_small()
                    .text_color(hex(Chrome::FOREGROUND))
                    .child(tree.size.map(size_text).unwrap_or_else(|| "…".into())),
            )
            .child(div().w(px(24.)).flex().justify_end().when(!tree.tree.prunable, |d| {
                d.child(
                    crate::ui::icon_only(SharedString::from(format!("tree-reveal-{}", path.display())), "folder-open", move |_, _, cx| {
                        cx.stop_propagation();
                        crate::platform::open_folder(&reveal);
                    })
                    .tooltip(crate::ui::Tooltip::text(t(cx, "trees.reveal"), None)),
                )
            }))
            .into_any_element()
    }

    fn render_tree_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let removal = self.tree_manager.confirm.as_ref()?;
        let chosen: Vec<&TreeStatus> =
            removal.trees.iter().filter_map(|p| self.tree_manager.trees.iter().find(|t| &t.tree.path == p)).collect();
        let dirty = chosen.iter().filter(|t| t.dirty > 0).count();
        let unmerged = chosen.iter().filter(|t| t.ahead > 0 && !t.pr_merged_here).count();
        let names = chosen.iter().map(|t| t.tree.name()).collect::<Vec<_>>().join(", ");
        let option =
            |id: &'static str, checked: bool, label: String, hint: String, toggle: fn(&mut TreeRemoval), cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .flex()
                    .items_start()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(removal) = this.tree_manager.confirm.as_mut() {
                            toggle(removal);
                        }
                        cx.notify();
                    }))
                    .child(check_box(checked, true).mt(px(2.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(label))
                            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(hint)),
                    )
                    .into_any_element()
            };
        let mut body = vec![
            div()
                .t_body()
                .text_color(hex(Chrome::FOREGROUND))
                .child(tf(cx, "trees.confirm_body", &[("n", &chosen.len().to_string())]))
                .into_any_element(),
            div().t_caption().text_color(hex(Chrome::MUTED)).max_h(px(72.)).overflow_hidden().child(names).into_any_element(),
            option(
                "trees-confirm-branch",
                removal.delete_branch,
                t(cx, "trees.confirm_branch").to_string(),
                t(cx, "trees.confirm_branch_hint").to_string(),
                |r| r.delete_branch = !r.delete_branch,
                cx,
            ),
        ];
        if dirty > 0 {
            body.push(option(
                "trees-confirm-force",
                removal.force,
                tf(cx, "trees.confirm_force", &[("n", &dirty.to_string())]),
                t(cx, if removal.force { "trees.confirm_force_on" } else { "trees.confirm_force_off" }).to_string(),
                |r| r.force = !r.force,
                cx,
            ));
        }
        if unmerged > 0 {
            body.push(
                div()
                    .t_caption()
                    .text_color(hex(Chrome::WARNING))
                    .child(tf(cx, "trees.confirm_unmerged", &[("n", &unmerged.to_string())]))
                    .into_any_element(),
            );
        }
        Some(confirm_dialog(
            "trees-confirm",
            t(cx, "trees.confirm_title").to_string(),
            body,
            t(cx, "trees.confirm_ok").to_string(),
            cx.listener(|this, _: &ClickEvent, _, cx| {
                this.tree_manager.confirm = None;
                cx.notify();
            }),
            cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(removal) = this.tree_manager.confirm.take() {
                    this.remove_selected_trees(removal, cx);
                }
            }),
            t(cx, "confirm.cancel").to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::size_text;

    #[test]
    fn sizes_read_like_the_finder() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(1536 * 1024), "1.5 MB");
        assert_eq!(size_text(250 * 1024 * 1024 * 1024), "250 GB");
        assert_eq!(size_text(0), "0 B");
    }
}
