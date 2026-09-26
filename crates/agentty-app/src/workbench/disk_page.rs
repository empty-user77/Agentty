//! Monitoring → Disk: how full the disk is, which folders in the home folder take the room, and
//! what can be cleared without any project, program or the computer noticing — each project's
//! build output and caches, the tools' caches, the trash. Everything removed goes through
//! `agentty_bridge::disk::clean`, which checks it again first.

use super::tree_manager::{badge, confirm_dialog, size_text};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use crate::usage_view::{card, kpi};
use agentty_bridge::disk::{Item, Kind, Usage, Volume};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, SharedString};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

/// "Clear these?" — what, and the title that names it.
pub struct DiskClean {
    pub items: Vec<Item>,
    pub what: String,
}

#[derive(Default)]
pub struct DiskManager {
    scroll: gpui::ScrollHandle,
    volume: Option<Volume>,
    usage: Option<Usage>,
    usage_loading: bool,
    /// Build output and caches of projects, and the tools' own caches.
    items: Vec<Item>,
    items_loading: bool,
    loaded: bool,
    generation: u64,
    /// Home folders opened to show what is inside.
    expanded: HashSet<PathBuf>,
    /// Projects opened to show their folders one by one.
    expanded_projects: HashSet<PathBuf>,
    pub(super) confirm: Option<DiskClean>,
    /// Paths being cleared right now.
    cleaning: HashSet<PathBuf>,
}

/// The name the UI gives an item: the tool and what it is.
fn item_label(item: &Item, cx: &gpui::App) -> String {
    match item.id.as_str() {
        "trash" => t(cx, "disk.trash").to_string(),
        "logs" => t(cx, "disk.logs").to_string(),
        "xcode-derived" => t(cx, "disk.xcode_derived").to_string(),
        "node-cache" => item.tool.clone(),
        _ => match item.kind {
            Kind::Build => tf(cx, "disk.build_of", &[("tool", &item.tool)]),
            _ => tf(cx, "disk.cache_of", &[("tool", &item.tool)]),
        },
    }
}

/// Where an item is, shortened: relative to its project, else from the home folder.
fn item_where(item: &Item) -> String {
    match &item.project {
        Some(project) => item.path.strip_prefix(project).map(|p| p.display().to_string()).unwrap_or_else(|_| crate::ui::tilde(&item.path)),
        None => crate::ui::tilde(&item.path),
    }
}

/// Only what is worth a line: a few megabytes at least.
const SHOWN_FROM: u64 = 1024 * 1024;

impl Workbench {
    /// Measures everything again: the volume at once, the home folder and the cleanable items in
    /// the background, each shown as soon as it is in.
    pub(super) fn scan_disk(&mut self, cx: &mut Context<Self>) {
        let known = self.known_project_folders(cx);
        let manager = &mut self.disk_manager;
        manager.generation += 1;
        let generation = manager.generation;
        manager.loaded = true;
        manager.usage_loading = true;
        manager.items_loading = true;
        manager.volume = agentty_bridge::disk::volume(&agentty_bridge::fsutil::home());
        cx.notify();
        let background = cx.background_executor().clone();
        let usage_task = background.spawn(async move {
            let never = AtomicBool::new(false);
            agentty_bridge::disk::home_usage(&never)
        });
        let items_task = background.spawn(async move {
            let never = AtomicBool::new(false);
            let repos = agentty_bridge::inventory::find_repos(&known, &never);
            // Every tree of every project: linked trees build their own output.
            let mut roots: Vec<PathBuf> = Vec::new();
            for repo in &repos {
                match agentty_bridge::worktree::list(repo) {
                    Ok(trees) => roots.extend(trees.into_iter().filter(|t| t.path.is_dir()).map(|t| t.path)),
                    Err(_) => roots.push(repo.clone()),
                }
            }
            roots.sort();
            roots.dedup();
            let mut items: Vec<_> = agentty_bridge::disk::in_parallel(roots, 4, |root| agentty_bridge::disk::project_items(&root, &never))
                .into_iter()
                .flatten()
                .collect();
            items.extend(agentty_bridge::disk::global_items(&never));
            items
        });
        cx.spawn(async move |this, cx| {
            let items = items_task.await;
            let _ = this.update(cx, |this, cx| {
                if this.disk_manager.generation == generation {
                    this.disk_manager.items = items;
                    this.disk_manager.items_loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let usage = usage_task.await;
            let _ = this.update(cx, |this, cx| {
                if this.disk_manager.generation == generation {
                    this.disk_manager.usage = Some(usage);
                    this.disk_manager.usage_loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Clears `items` (confirmed), then measures them and the volume again.
    fn clean_disk_items(&mut self, items: Vec<Item>, cx: &mut Context<Self>) {
        self.disk_manager.cleaning.extend(items.iter().map(|i| i.path.clone()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (freed, failed, done) = cx
                .background_executor()
                .spawn(async move {
                    let (mut freed, mut failed, mut done) = (0u64, Vec::new(), Vec::new());
                    let never = AtomicBool::new(false);
                    for item in items {
                        match agentty_bridge::disk::clean(&item) {
                            Ok(bytes) => freed += bytes,
                            Err(err) => failed.push(format!("{err:#}")),
                        }
                        // What is left: nothing for a build folder, what stayed in a cache.
                        let left = if item.path.exists() { Some(agentty_bridge::disk::size_of(&item.path, &never)) } else { None };
                        done.push((item.path, left));
                    }
                    (freed, failed, done)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let manager = &mut this.disk_manager;
                for (path, _) in &done {
                    manager.cleaning.remove(path);
                }
                // A build folder is gone; a cache folder stays, emptied (as far as it could be).
                manager.items.retain_mut(|item| match done.iter().find(|(p, _)| *p == item.path) {
                    None => true,
                    Some((_, left)) => match left {
                        Some(size) => {
                            item.size = *size;
                            true
                        }
                        None => false,
                    },
                });
                manager.volume = agentty_bridge::disk::volume(&agentty_bridge::fsutil::home());
                let mut text = tf(cx, "disk.freed", &[("size", &size_text(freed))]);
                if let Some(first) = failed.first() {
                    text.push_str(&tf(cx, "disk.clean_failed", &[("n", &failed.len().to_string()), ("error", first)]));
                }
                this.show_toast_for(text, 6000, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn ask_clean(&mut self, items: Vec<Item>, what: String, cx: &mut Context<Self>) {
        if items.is_empty() {
            return;
        }
        self.disk_manager.confirm = Some(DiskClean { items, what });
        cx.notify();
    }

    /// A button that clears `items` after asking.
    fn clean_button(&self, id: SharedString, label: String, items: Vec<Item>, what: String, cx: &mut Context<Self>) -> AnyElement {
        let busy = items.iter().any(|i| self.disk_manager.cleaning.contains(&i.path));
        if busy {
            return div()
                .flex_shrink_0()
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .child(crate::ui::spinner(IconSize::INLINE, hex(Chrome::MUTED)))
                .child(t(cx, "disk.cleaning"))
                .into_any_element();
        }
        crate::ui::action_button_with_icon(
            id,
            "trash-2",
            label,
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.ask_clean(items.clone(), what.clone(), cx);
            }),
        )
        .into_any_element()
    }

    pub(super) fn render_disk_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.disk_manager.loaded {
            self.scan_disk(cx);
        }
        let scroll = self.disk_manager.scroll.clone();
        let manager = &self.disk_manager;
        let shown: Vec<&Item> = manager.items.iter().filter(|i| i.size >= SHOWN_FROM).collect();
        let cleanable: u64 = shown.iter().map(|i| i.size).sum();

        let loading = manager.usage_loading || manager.items_loading;
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().t_heading().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.disk")))
            .when(loading, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(crate::ui::spinner(IconSize::INLINE, hex(Chrome::MUTED)))
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "disk.scanning"))),
                )
            })
            .child(div().flex_1())
            .child(
                crate::ui::icon_only(
                    "disk-refresh",
                    "rotate-cw",
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if !(this.disk_manager.usage_loading || this.disk_manager.items_loading) {
                            this.scan_disk(cx);
                        }
                    }),
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, "trees.refresh"), None)),
            );

        // The volume: a bar of used / clearable / free, and the numbers.
        let volume = manager.volume.unwrap_or_default();
        let fraction = |bytes: u64| if volume.total == 0 { 0. } else { (bytes as f32 / volume.total as f32).clamp(0., 1.) };
        let clear_part = fraction(cleanable.min(volume.used()));
        let used_part = (fraction(volume.used()) - clear_part).max(0.);
        let bar = div()
            .h(px(12.))
            .w_full()
            .rounded_full()
            .overflow_hidden()
            .flex()
            .bg(hex(0x2d2d30))
            .child(div().h_full().w(gpui::relative(used_part)).bg(hex(Chrome::BLUE)))
            .child(div().h_full().w(gpui::relative(clear_part)).bg(hex(Chrome::ORANGE)));
        let legend = |color: u32, label: String| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .child(div().size(px(8.)).rounded_full().bg(hex(color)))
                .child(label)
        };
        let volume_card = card()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                cx,
                "disk.volume",
                &[("used", &size_text(volume.used())), ("total", &size_text(volume.total))],
            )))
            .child(bar)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .child(legend(
                        Chrome::BLUE,
                        tf(cx, "disk.legend_used", &[("size", &size_text(volume.used().saturating_sub(cleanable)))]),
                    ))
                    .child(legend(Chrome::ORANGE, tf(cx, "disk.legend_clearable", &[("size", &size_text(cleanable))])))
                    .child(legend(0x2d2d30, tf(cx, "disk.legend_free", &[("size", &size_text(volume.available))]))),
            );

        let sum = |kind: Kind| shown.iter().filter(|i| i.kind == kind).map(|i| i.size).sum::<u64>();
        let (build, caches, other) = (
            shown.iter().filter(|i| i.project.is_some() || i.id == "xcode-derived").map(|i| i.size).sum::<u64>(),
            shown.iter().filter(|i| i.project.is_none() && i.kind == Kind::Cache).map(|i| i.size).sum::<u64>(),
            sum(Kind::Other),
        );
        let pending = |bytes: u64| if manager.items_loading && bytes == 0 { "…".to_string() } else { size_text(bytes) };
        let kpis = div()
            .flex()
            .flex_wrap()
            .gap_3()
            .child(kpi(
                t(cx, "disk.kpi_free"),
                size_text(volume.available),
                tf(cx, "disk.kpi_free_sub", &[("total", &size_text(volume.total))]),
                Chrome::SUCCESS,
            ))
            .child(kpi(t(cx, "disk.kpi_build"), pending(build), t(cx, "disk.kpi_build_sub").to_string(), Chrome::ORANGE))
            .child(kpi(t(cx, "disk.kpi_cache"), pending(caches), t(cx, "disk.kpi_cache_sub").to_string(), Chrome::PURPLE))
            .child(kpi(t(cx, "disk.kpi_other"), pending(other), t(cx, "disk.kpi_other_sub").to_string(), Chrome::BLUE));

        let usage = self.render_disk_usage(cx);
        let projects = self.render_disk_projects(&shown, cx);
        let globals = self.render_disk_globals(&shown, cx);
        let confirm = self.render_disk_confirm(cx);
        div()
            .size_full()
            .relative()
            .child(
                div().id("disk-page").size_full().bg(hex(Chrome::EDITOR)).overflow_y_scroll().track_scroll(&scroll).child(
                    div()
                        .px_6()
                        .py_5()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(header)
                        .child(volume_card)
                        .child(kpis)
                        .child(projects)
                        .child(globals)
                        .child(usage)
                        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "disk.footnote"))),
                ),
            )
            .group(crate::ui::SCROLL_GROUP)
            .child(crate::ui::scrollbar(scroll))
            .children(confirm)
    }

    fn section_title(title: String, sub: Option<String>, right: Option<AnyElement>) -> gpui::Div {
        div()
            .px_4()
            .py_2p5()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(div().flex_shrink_0().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(title))
            // The explanation gives way to the button when the page is narrow or the button's label long.
            .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::MUTED)).children(sub))
            .children(right.map(|right| div().flex_shrink_0().child(right)))
    }

    /// Build output and caches, one row per project (its folders listed when opened).
    fn render_disk_projects(&self, shown: &[&Item], cx: &mut Context<Self>) -> AnyElement {
        let mut projects: Vec<(PathBuf, Vec<Item>)> = Vec::new();
        for item in shown.iter().filter(|i| i.project.is_some()) {
            let project = item.project.clone().unwrap_or_default();
            match projects.iter_mut().find(|(p, _)| *p == project) {
                Some((_, items)) => items.push((*item).clone()),
                None => projects.push((project, vec![(*item).clone()])),
            }
        }
        let total = |items: &[Item]| items.iter().map(|i| i.size).sum::<u64>();
        projects.sort_by_key(|(_, items)| std::cmp::Reverse(total(items)));
        let all_build: Vec<Item> = projects.iter().flat_map(|(_, items)| items.iter().filter(|i| i.kind == Kind::Build).cloned()).collect();
        let right = (!all_build.is_empty()).then(|| {
            let size = size_text(total(&all_build));
            self.clean_button(
                "disk-clean-all-build".into(),
                tf(cx, "disk.clean_all_build", &[("size", &size)]),
                all_build,
                tf(cx, "disk.what_all_build", &[("size", &size)]),
                cx,
            )
        });
        let mut section = card().flex().flex_col().overflow_hidden().child(Self::section_title(
            t(cx, "disk.projects").to_string(),
            Some(t(cx, "disk.projects_sub").to_string()),
            right,
        ));
        if projects.is_empty() {
            section = section.child(
                crate::ui::hint(if self.disk_manager.items_loading { t(cx, "disk.scanning") } else { t(cx, "disk.projects_empty") })
                    .px_4()
                    .py_3(),
            );
        }
        for (project, items) in projects {
            let name = project.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let build: Vec<Item> = items.iter().filter(|i| i.kind == Kind::Build).cloned().collect();
            let caches: Vec<Item> = items.iter().filter(|i| i.kind == Kind::Cache).cloned().collect();
            let open = self.disk_manager.expanded_projects.contains(&project);
            let toggle = project.clone();
            let tools = {
                let mut tools: Vec<&str> = items.iter().map(|i| i.tool.as_str()).collect();
                tools.dedup();
                tools.join(" · ")
            };
            let key = project.display().to_string();
            let mut row = div()
                .id(SharedString::from(format!("disk-project-{key}")))
                .px_4()
                .py_2()
                .flex()
                .items_center()
                .gap_3()
                .border_b_1()
                .border_color(hex(0x2a2a2a))
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if !this.disk_manager.expanded_projects.remove(&toggle) {
                        this.disk_manager.expanded_projects.insert(toggle.clone());
                    }
                    cx.notify();
                }))
                .child(icon(if open { "chevron-down" } else { "chevron-right" }, 12., hex(Chrome::MUTED)))
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
                                .gap_2()
                                .min_w_0()
                                .child(div().t_body().truncate().text_color(hex(Chrome::BRIGHT)).child(name.clone()))
                                .child(div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(tools)),
                        )
                        .child(div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(crate::ui::tilde(&project))),
                )
                .child(div().w(px(80.)).flex().justify_end().t_body().text_color(hex(Chrome::FOREGROUND)).child(size_text(total(&items))));
            let mut actions = div().flex().gap_1p5().on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
            if !build.is_empty() {
                let size = size_text(total(&build));
                actions = actions.child(self.clean_button(
                    SharedString::from(format!("disk-build-{key}")),
                    t(cx, "disk.clean_build").to_string(),
                    build,
                    tf(cx, "disk.what_build", &[("name", &name), ("size", &size)]),
                    cx,
                ));
            }
            if !caches.is_empty() {
                let size = size_text(total(&caches));
                actions = actions.child(self.clean_button(
                    SharedString::from(format!("disk-cache-{key}")),
                    t(cx, "disk.clean_cache").to_string(),
                    caches,
                    tf(cx, "disk.what_cache", &[("name", &name), ("size", &size)]),
                    cx,
                ));
            }
            row = row.child(actions);
            section = section.child(row);
            if open {
                for item in &items {
                    section = section.child(
                        div()
                            .pl(px(44.))
                            .pr_4()
                            .py_1()
                            .flex()
                            .items_center()
                            .gap_3()
                            .bg(hex_alpha(0x000000, 0.18))
                            .t_small()
                            .child(badge(
                                t(cx, if item.kind == Kind::Build { "disk.kind_build" } else { "disk.kind_cache" }),
                                if item.kind == Kind::Build { Chrome::ORANGE } else { Chrome::PURPLE },
                            ))
                            .child(div().flex_shrink_0().text_color(hex(Chrome::BRIGHT)).child(item_label(item, cx)))
                            .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(item_where(item)))
                            .child(div().w(px(80.)).flex().justify_end().text_color(hex(Chrome::FOREGROUND)).child(size_text(item.size))),
                    );
                }
            }
        }
        section.into_any_element()
    }

    /// The tools' caches, and the rest (trash, logs).
    fn render_disk_globals(&self, shown: &[&Item], cx: &mut Context<Self>) -> AnyElement {
        // One row per tool: Cargo keeps its downloads in more than one folder.
        let mut globals: Vec<Vec<Item>> = Vec::new();
        for item in shown.iter().filter(|i| i.project.is_none()) {
            match globals.iter_mut().find(|group| group[0].id == item.id) {
                Some(group) => group.push((*item).clone()),
                None => globals.push(vec![(*item).clone()]),
            }
        }
        let group_size = |group: &[Item]| group.iter().map(|i| i.size).sum::<u64>();
        globals.sort_by_key(|group| (group[0].kind == Kind::Other, std::cmp::Reverse(group_size(group))));
        let caches: Vec<Item> = globals.iter().flatten().filter(|i| i.kind != Kind::Other).cloned().collect();
        let right = (!caches.is_empty()).then(|| {
            let size = size_text(caches.iter().map(|i| i.size).sum());
            self.clean_button(
                "disk-clean-all-caches".into(),
                tf(cx, "disk.clean_all_caches", &[("size", &size)]),
                caches,
                tf(cx, "disk.what_all_caches", &[("size", &size)]),
                cx,
            )
        });
        let mut section = card().flex().flex_col().overflow_hidden().child(Self::section_title(
            t(cx, "disk.caches").to_string(),
            Some(t(cx, "disk.caches_sub").to_string()),
            right,
        ));
        if globals.is_empty() {
            section = section.child(
                crate::ui::hint(if self.disk_manager.items_loading { t(cx, "disk.scanning") } else { t(cx, "disk.caches_empty") })
                    .px_4()
                    .py_3(),
            );
        }
        let mut other_started = false;
        for group in globals {
            let item = &group[0];
            let size = group_size(&group);
            if item.kind == Kind::Other && !other_started {
                other_started = true;
                section = section.child(
                    div()
                        .px_4()
                        .pt_2p5()
                        .pb_1()
                        .t_caption()
                        .font_weight(crate::theme::EMPHASIS)
                        .text_color(hex(Chrome::MUTED))
                        .bg(hex(0x1f1f1f))
                        .child(t(cx, "disk.other")),
                );
            }
            let label = item_label(item, cx);
            let (button, what) = match item.id.as_str() {
                "trash" => (t(cx, "disk.empty_trash").to_string(), tf(cx, "disk.what_trash", &[("size", &size_text(size))])),
                "logs" => {
                    (t(cx, "disk.clean_logs").to_string(), tf(cx, "disk.what_named", &[("name", &label), ("size", &size_text(size))]))
                }
                _ if item.kind == Kind::Build => {
                    (t(cx, "disk.clean_build").to_string(), tf(cx, "disk.what_named", &[("name", &label), ("size", &size_text(size))]))
                }
                _ => (t(cx, "disk.clean_cache").to_string(), tf(cx, "disk.what_named", &[("name", &label), ("size", &size_text(size))])),
            };
            let key = item.path.display().to_string();
            section = section.child(
                div()
                    .px_4()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(hex(0x2a2a2a))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(label))
                            .child(
                                div()
                                    .t_caption()
                                    .truncate()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(group.iter().map(|i| crate::ui::tilde(&i.path)).collect::<Vec<_>>().join(" · ")),
                            ),
                    )
                    .child(div().w(px(80.)).flex().justify_end().t_body().text_color(hex(Chrome::FOREGROUND)).child(size_text(size)))
                    .child(self.clean_button(SharedString::from(format!("disk-item-{key}")), button, group.clone(), what, cx)),
            );
        }
        section.into_any_element()
    }

    /// The biggest folders of the home folder, each opening onto its own biggest folders.
    fn render_disk_usage(&self, cx: &mut Context<Self>) -> AnyElement {
        let manager = &self.disk_manager;
        let mut section = card().flex().flex_col().overflow_hidden().child(Self::section_title(
            t(cx, "disk.usage").to_string(),
            manager.usage.as_ref().map(|u| tf(cx, "disk.usage_sub", &[("size", &size_text(u.size))])),
            None,
        ));
        let Some(usage) = manager.usage.as_ref() else {
            return section.child(crate::ui::loading_row(t(cx, "disk.usage_loading")).px_4().py_3()).into_any_element();
        };
        let top = usage.children.first().map_or(1, |c| c.size.max(1));
        let row = |folder: &Usage, depth: usize, openable: bool, open: bool, cx: &mut Context<Self>| {
            let path = folder.path.clone();
            let toggle = path.clone();
            let reveal = path.clone();
            let name = if depth == 0 {
                crate::ui::tilde(&path)
            } else {
                path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            };
            div()
                .id(SharedString::from(format!("disk-usage-{}", path.display())))
                .pl(px(16. + depth as f32 * 22.))
                .pr_4()
                .py_1p5()
                .flex()
                .items_center()
                .gap_3()
                .border_b_1()
                .border_color(hex(0x2a2a2a))
                .when(depth > 0, |d| d.bg(hex_alpha(0x000000, 0.18)))
                .when(openable, |d| {
                    d.cursor_pointer().hover(|s| s.bg(hex(Chrome::HOVER))).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if !this.disk_manager.expanded.remove(&toggle) {
                            this.disk_manager.expanded.insert(toggle.clone());
                        }
                        cx.notify();
                    }))
                })
                .child(if openable {
                    icon(if open { "chevron-down" } else { "chevron-right" }, 12., hex(Chrome::MUTED)).into_any_element()
                } else {
                    div().w(px(12.)).into_any_element()
                })
                .child(div().w(px(220.)).flex_shrink_0().t_body().truncate().text_color(hex(Chrome::BRIGHT)).child(name))
                .child(
                    div().flex_1().min_w(px(40.)).h(px(6.)).rounded_full().bg(hex(0x2d2d30)).child(
                        div()
                            .h_full()
                            .rounded_full()
                            .w(gpui::relative((folder.size as f32 / top as f32).clamp(0.01, 1.)))
                            .bg(hex(if depth == 0 { Chrome::BLUE } else { Chrome::MUTED })),
                    ),
                )
                .child(div().w(px(80.)).flex().justify_end().t_small().text_color(hex(Chrome::FOREGROUND)).child(size_text(folder.size)))
                .child(
                    crate::ui::icon_only(SharedString::from(format!("disk-reveal-{}", path.display())), "folder-open", move |_, _, cx| {
                        cx.stop_propagation();
                        crate::platform::open_folder(&reveal);
                    })
                    .tooltip(crate::ui::Tooltip::text(t(cx, "trees.reveal"), None)),
                )
        };
        for folder in usage.children.iter().filter(|f| f.size >= SHOWN_FROM).take(15) {
            let open = manager.expanded.contains(&folder.path);
            let openable = !folder.children.is_empty();
            section = section.child(row(folder, 0, openable, open, cx));
            if open {
                for child in folder.children.iter().filter(|f| f.size >= SHOWN_FROM) {
                    section = section.child(row(child, 1, false, false, cx));
                }
            }
        }
        section.into_any_element()
    }

    fn render_disk_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let clean = self.disk_manager.confirm.as_ref()?;
        let trash = clean.items.iter().any(|i| i.id == "trash");
        let places: Vec<String> = clean.items.iter().take(6).map(|i| crate::ui::tilde(&i.path)).collect();
        let more = clean.items.len().saturating_sub(places.len());
        let mut list = places.join("\n");
        if more > 0 {
            list.push_str(&format!("\n{}", tf(cx, "disk.confirm_more", &[("n", &more.to_string())])));
        }
        let body = vec![
            div().t_body().text_color(hex(Chrome::FOREGROUND)).child(clean.what.clone()).into_any_element(),
            div().t_caption().font_family("JetBrains Mono").text_color(hex(Chrome::MUTED)).child(list).into_any_element(),
            div()
                .t_caption()
                .text_color(hex(if trash { Chrome::WARNING } else { Chrome::MUTED }))
                .child(t(cx, if trash { "disk.confirm_trash_hint" } else { "disk.confirm_hint" }))
                .into_any_element(),
        ];
        Some(confirm_dialog(
            "disk-confirm",
            t(cx, "disk.confirm_title").to_string(),
            body,
            t(cx, "disk.confirm_ok").to_string(),
            cx.listener(|this, _: &ClickEvent, _, cx| {
                this.disk_manager.confirm = None;
                cx.notify();
            }),
            cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(clean) = this.disk_manager.confirm.take() {
                    this.clean_disk_items(clean.items, cx);
                }
            }),
            t(cx, "confirm.cancel").to_string(),
        ))
    }
}
