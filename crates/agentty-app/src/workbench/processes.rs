//! "AI processes" page (next to AI Usage): every local AI agent process tree with its CPU, memory
//! and disk use and whether it is active, waiting or inactive — like cmux's task manager.

use super::{Page, Workbench};
use crate::ai_processes::AgentProcess;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use crate::usage_view::{card, kpi};
use gpui::{div, prelude::*, px, ClickEvent, Context, FontWeight};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// A process tree, the Agentty pane it runs in (label, working) and its activity.
type Row<'a> = (&'a AgentProcess, Option<(String, bool)>, Activity);

const REFRESH: Duration = Duration::from_secs(2);
/// Below this, a process tree counts as idle.
const ACTIVE_CPU: f32 = 2.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Cpu,
    Memory,
    Disk,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Activity {
    Active,
    Waiting,
    Inactive,
}

#[derive(Default)]
pub struct ProcessMonitor {
    groups: Vec<AgentProcess>,
    /// Disk bytes per root at the previous sample, to show read/write rates.
    previous_io: HashMap<u32, (u64, u64, Instant)>,
    rates: HashMap<u32, (f64, f64)>,
    loading: bool,
    running: bool,
    updated: Option<chrono::DateTime<chrono::Local>>,
    expanded: HashSet<u32>,
    sort: SortKey,
}

fn memory(kb: u64) -> String {
    let mb = kb as f64 / 1024.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.1} MB")
    }
}

fn bytes(value: f64) -> String {
    match value {
        v if v >= 1024.0 * 1024.0 * 1024.0 => format!("{:.1} GB", v / (1024.0 * 1024.0 * 1024.0)),
        v if v >= 1024.0 * 1024.0 => format!("{:.1} MB", v / (1024.0 * 1024.0)),
        v if v >= 1024.0 => format!("{:.0} KB", v / 1024.0),
        v => format!("{v:.0} B"),
    }
}

/// `ps` elapsed time (`[[dd-]hh:]mm:ss`) in a short form: `3d 4h`, `2h 5m`, `12m`.
fn uptime(elapsed: &str) -> String {
    let (days, rest) = elapsed.split_once('-').map_or((0, elapsed), |(d, r)| (d.parse().unwrap_or(0), r));
    let parts: Vec<u64> = rest.split(':').filter_map(|p| p.parse().ok()).collect();
    let (hours, minutes) = match parts.as_slice() {
        [h, m, _] => (*h, *m),
        [m, _] => (0, *m),
        _ => (0, 0),
    };
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".to_string()
    }
}

impl Workbench {
    /// Starts sampling while the page is open (stops by itself when it closes).
    pub(super) fn start_process_monitor(&mut self, cx: &mut Context<Self>) {
        if self.processes.running {
            return;
        }
        self.processes.running = true;
        self.processes.loading = true;
        cx.spawn(async move |this, cx| loop {
            let groups = cx.background_executor().spawn(async { crate::ai_processes::collect() }).await;
            let open = this
                .update(cx, |this, cx| {
                    this.apply_process_sample(groups);
                    cx.notify();
                    let open = this.page == Some(Page::Processes);
                    if !open {
                        this.processes.running = false;
                    }
                    open
                })
                .unwrap_or(false);
            if !open {
                break;
            }
            cx.background_executor().timer(REFRESH).await;
        })
        .detach();
    }

    fn apply_process_sample(&mut self, groups: Vec<AgentProcess>) {
        let monitor = &mut self.processes;
        let now = Instant::now();
        let mut io = HashMap::new();
        monitor.rates.clear();
        for group in &groups {
            if let Some((read, write, at)) = monitor.previous_io.get(&group.root) {
                let seconds = now.duration_since(*at).as_secs_f64().max(0.1);
                monitor.rates.insert(
                    group.root,
                    (group.disk_read.saturating_sub(*read) as f64 / seconds, group.disk_write.saturating_sub(*write) as f64 / seconds),
                );
            }
            io.insert(group.root, (group.disk_read, group.disk_write, now));
        }
        monitor.previous_io = io;
        monitor.groups = groups;
        monitor.loading = false;
        monitor.updated = Some(chrono::Local::now());
    }

    /// The Agentty pane a process tree runs in: (label, whether its agent is working).
    fn process_owner(&self, process: &AgentProcess, cx: &gpui::App) -> Option<(String, bool)> {
        for ws in &self.workspaces {
            for tab in &ws.tabs {
                for pane in tab.root.leaves() {
                    let view = pane.read(cx);
                    if view.shell_pid().is_some_and(|pid| pid == process.root || process.ancestors.contains(&pid)) {
                        let (workspace, title) = (self.workspace_title(ws, cx), view.display_title());
                        let label = if workspace == title { title } else { format!("{workspace} / {title}") };
                        return Some((label, view.status.in_turn()));
                    }
                }
            }
        }
        None
    }

    /// `in_pane`: `Some(working)` when the tree runs in an Agentty pane.
    fn activity(process: &AgentProcess, in_pane: Option<bool>) -> Activity {
        if process.stopped {
            return Activity::Inactive;
        }
        if process.cpu >= ACTIVE_CPU || in_pane == Some(true) {
            return Activity::Active;
        }
        // Detached from any terminal and doing nothing: left behind.
        if process.tty == "??" && in_pane.is_none() {
            return Activity::Inactive;
        }
        Activity::Waiting
    }

    pub(super) fn render_processes(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.start_process_monitor(cx);
        let monitor = &self.processes;
        let mut groups: Vec<&AgentProcess> = monitor.groups.iter().collect();
        let rate = |root: u32| monitor.rates.get(&root).copied().unwrap_or((0.0, 0.0));
        match monitor.sort {
            SortKey::Cpu => groups.sort_by(|a, b| b.cpu.total_cmp(&a.cpu)),
            SortKey::Memory => groups.sort_by_key(|p| std::cmp::Reverse(p.rss_kb)),
            SortKey::Disk => groups.sort_by(|a, b| {
                let (ra, wa) = rate(a.root);
                let (rb, wb) = rate(b.root);
                (rb + wb).total_cmp(&(ra + wa)).then((b.disk_read + b.disk_write).cmp(&(a.disk_read + a.disk_write)))
            }),
        }
        let rows: Vec<Row> = groups
            .into_iter()
            .map(|p| {
                let owner = self.process_owner(p, cx);
                let activity = Self::activity(p, owner.as_ref().map(|(_, s)| *s));
                (p, owner, activity)
            })
            .collect();

        // Totals.
        let cpu: f32 = rows.iter().map(|(p, _, _)| p.cpu).sum();
        let rss: u64 = rows.iter().map(|(p, _, _)| p.rss_kb).sum();
        let count: usize = rows.iter().map(|(p, _, _)| p.tree.len()).sum();
        let (read_rate, write_rate) = rows.iter().fold((0.0, 0.0), |(r, w), (p, _, _)| {
            let (pr, pw) = rate(p.root);
            (r + pr, w + pw)
        });
        let counts = |activity: Activity| rows.iter().filter(|(_, _, a)| *a == activity).count();

        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().t_heading().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.processes")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "processes.auto_refresh")))
            .when(monitor.loading, |d| d.child(crate::ui::spinner(IconSize::INLINE, hex(Chrome::MUTED))))
            .child(div().flex_1())
            .children(monitor.updated.map(|at| {
                div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "processes.updated",
                    &[("time", &at.format("%H:%M:%S").to_string())],
                ))
            }));

        let kpis = div()
            .flex()
            .flex_wrap()
            .gap_3()
            .child(kpi(t(cx, "processes.cpu"), format!("{cpu:.1}%"), t(cx, "processes.cpu_sub").to_string(), Chrome::BLUE))
            .child(kpi(t(cx, "processes.memory"), memory(rss), t(cx, "processes.memory_sub").to_string(), Chrome::PURPLE))
            .child(kpi(
                t(cx, "processes.disk"),
                format!("{}/s", bytes(read_rate + write_rate)),
                tf(
                    cx,
                    "processes.disk_sub",
                    &[("read", &format!("{}/s", bytes(read_rate))), ("write", &format!("{}/s", bytes(write_rate)))],
                ),
                Chrome::ORANGE,
            ))
            .child(kpi(
                t(cx, "processes.count"),
                rows.len().to_string(),
                tf(cx, "processes.count_sub", &[("n", &count.to_string())]),
                Chrome::GREEN,
            ))
            .child(kpi(
                t(cx, "processes.states"),
                counts(Activity::Active).to_string(),
                tf(
                    cx,
                    "processes.states_sub",
                    &[("waiting", &counts(Activity::Waiting).to_string()), ("inactive", &counts(Activity::Inactive).to_string())],
                ),
                Chrome::SUCCESS,
            ));

        // Table.
        let sort_header = |id: &'static str, label: &'static str, key: SortKey, width: f32, cx: &mut Context<Self>| {
            let active = self.processes.sort == key;
            div()
                .id(id)
                .w(px(width))
                .flex()
                .justify_end()
                .items_center()
                .gap_0p5()
                .cursor_pointer()
                .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::MUTED }))
                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                .child(t(cx, label))
                .when(active, |d| d.child(icon("chevron-down", 11., hex(Chrome::BRIGHT))))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.processes.sort = key;
                    cx.notify();
                }))
        };
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
            .child(div().flex_1().min_w_0().child(t(cx, "processes.name")))
            .child(div().w(px(76.)).child(t(cx, "processes.state")))
            .child(sort_header("sort-cpu", "processes.cpu", SortKey::Cpu, 64., cx))
            .child(sort_header("sort-memory", "processes.memory", SortKey::Memory, 84., cx))
            .child(sort_header("sort-disk", "processes.disk", SortKey::Disk, 130., cx))
            .child(div().w(px(64.)).flex().justify_end().child(t(cx, "processes.uptime")));

        let mut table = card().flex().flex_col().overflow_hidden().child(columns);
        if rows.is_empty() {
            table = table
                .child(crate::ui::hint(if monitor.loading { t(cx, "processes.loading") } else { t(cx, "processes.empty") }).px_4().py_4());
        }

        // Per-agent totals first, then each running process tree.
        let mut totals: Vec<(&'static str, f32, u64, usize, usize)> = Vec::new();
        for (p, _, _) in &rows {
            match totals.iter_mut().find(|(agent, ..)| *agent == p.agent) {
                Some(total) => {
                    total.1 += p.cpu;
                    total.2 += p.rss_kb;
                    total.3 += 1;
                    total.4 += p.tree.len();
                }
                None => totals.push((p.agent, p.cpu, p.rss_kb, 1, p.tree.len())),
            }
        }
        let section = |label: &str| {
            div()
                .px_4()
                .pt_2p5()
                .pb_1()
                .t_caption()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(hex(Chrome::MUTED))
                .bg(hex(0x1f1f1f))
                .child(label.to_string())
        };
        if totals.len() > 1 || rows.len() > 1 {
            table = table.child(section(t(cx, "processes.by_agent")));
            for (agent, cpu, rss, sessions, procs) in &totals {
                let brand = crate::brand::brand(agent);
                table = table.child(
                    div()
                        .px_4()
                        .py_1p5()
                        .flex()
                        .items_center()
                        .gap_3()
                        .border_b_1()
                        .border_color(hex(0x2a2a2a))
                        .t_body()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(crate::brand::avatar(agent, 20.))
                                .child(div().text_color(hex(Chrome::BRIGHT)).child(brand.name))
                                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                                    cx,
                                    "processes.sessions",
                                    &[("sessions", &sessions.to_string()), ("n", &procs.to_string())],
                                ))),
                        )
                        .child(div().w(px(76.)))
                        .child(div().w(px(64.)).flex().justify_end().child(format!("{cpu:.1}%")))
                        .child(div().w(px(84.)).flex().justify_end().child(memory(*rss)))
                        .child(div().w(px(130.)))
                        .child(div().w(px(64.))),
                );
            }
            table = table.child(section(t(cx, "processes.running")));
        }

        for (process, owner, activity) in &rows {
            let root = process.root;
            let expanded = self.processes.expanded.contains(&root);
            let brand = crate::brand::brand(process.agent);
            let (label, color) = match activity {
                Activity::Active => (t(cx, "processes.active"), Chrome::SUCCESS),
                Activity::Waiting => (t(cx, "processes.waiting"), Chrome::WARNING),
                Activity::Inactive => (t(cx, "processes.inactive"), Chrome::MUTED),
            };
            let location = match owner {
                Some((pane, _)) => pane.clone(),
                None if process.tty == "??" => t(cx, "processes.background").to_string(),
                None => tf(cx, "processes.external", &[("tty", &process.tty)]),
            };
            let folder = process.cwd.as_deref().map(crate::ui::tilde).unwrap_or_default();
            let (read, write) = rate(root);
            let disk = if read + write >= 1.0 {
                format!("R {}/s · W {}/s", bytes(read), bytes(write))
            } else {
                format!("R {} · W {}", bytes(process.disk_read as f64), bytes(process.disk_write as f64))
            };
            table = table.child(
                div()
                    .id(("process", root as usize))
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
                        if !this.processes.expanded.remove(&root) {
                            this.processes.expanded.insert(root);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon(if expanded { "chevron-down" } else { "chevron-right" }, 12., hex(Chrome::MUTED)))
                            .child(crate::brand::avatar(process.agent, 22.))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .t_body()
                                            .child(div().text_color(hex(Chrome::BRIGHT)).child(brand.name))
                                            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(format!("PID {root}"))),
                                    )
                                    .child(div().t_small().truncate().text_color(hex(Chrome::MUTED)).child(tf(
                                        cx,
                                        "processes.row_sub",
                                        &[("n", &process.tree.len().to_string()), ("where", &location), ("folder", &folder)],
                                    ))),
                            ),
                    )
                    .child(
                        div().w(px(76.)).child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .t_small()
                                .text_color(hex(color))
                                .child(div().size(px(7.)).rounded_full().bg(hex(color)))
                                .child(label),
                        ),
                    )
                    .child(div().w(px(64.)).flex().justify_end().t_body().child(format!("{:.1}%", process.cpu)))
                    .child(div().w(px(84.)).flex().justify_end().t_body().child(memory(process.rss_kb)))
                    .child(div().w(px(130.)).flex().justify_end().t_small().text_color(hex(Chrome::FOREGROUND)).child(disk))
                    .child(div().w(px(64.)).flex().justify_end().t_small().text_color(hex(Chrome::MUTED)).child(uptime(&process.elapsed))),
            );
            if expanded {
                let mut children: Vec<_> = process.tree.iter().collect();
                children.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.rss_kb.cmp(&a.rss_kb)));
                for child in children {
                    table = table.child(
                        div()
                            .px_4()
                            .py_1()
                            .pl(px(62.))
                            .flex()
                            .items_center()
                            .gap_3()
                            .bg(hex_alpha(0x000000, 0.18))
                            .t_small()
                            .text_color(hex(Chrome::FOREGROUND))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .gap_2()
                                    .child(div().flex_shrink_0().w(px(54.)).text_color(hex(Chrome::MUTED)).child(child.pid.to_string()))
                                    .child(div().flex_shrink_0().text_color(hex(Chrome::BRIGHT)).child(child.name()))
                                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(child.command.clone())),
                            )
                            .child(div().w(px(76.)))
                            .child(div().w(px(64.)).flex().justify_end().child(format!("{:.1}%", child.cpu)))
                            .child(div().w(px(84.)).flex().justify_end().child(memory(child.rss_kb)))
                            .child(div().w(px(130.)))
                            .child(div().w(px(64.)).flex().justify_end().text_color(hex(Chrome::MUTED)).child(uptime(&child.elapsed))),
                    );
                }
            }
        }

        div().id("processes-page").size_full().bg(hex(Chrome::EDITOR)).overflow_y_scroll().child(
            div()
                .px_6()
                .py_5()
                .flex()
                .flex_col()
                .gap_4()
                .child(header)
                .child(kpis)
                .child(table)
                .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "processes.footnote"))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{bytes, memory, uptime};

    #[test]
    fn formats_values() {
        assert_eq!(uptime("05:09"), "5m");
        assert_eq!(uptime("00:09"), "<1m");
        assert_eq!(uptime("02:03:04"), "2h 3m");
        assert_eq!(uptime("3-04:05:06"), "3d 4h");
        assert_eq!(memory(2048), "2.0 MB");
        assert_eq!(memory(3 * 1024 * 1024), "3.0 GB");
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1536.0 * 1024.0), "1.5 MB");
    }
}
