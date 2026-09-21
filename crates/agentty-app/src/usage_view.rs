//! AI usage dashboard built from local Claude Code / Codex transcripts.

use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use crate::ui::{action_button, chip, compact_number, money, popover};
use agentty_bridge::model::Agent;
use agentty_bridge::usage::{root_dir, FileUsage, NamedUsage, UsageReport, UsageScanner};
use chrono::Local;
use gpui::{canvas, div, point, prelude::*, px, relative, ClickEvent, Context, Div, PathBuilder, Pixels, Point, Window};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const PERIODS: [u32; 4] = [1, 7, 30, 90];

pub struct UsageView {
    agent: Agent,
    days: u32,
    scanner: Arc<Mutex<UsageScanner>>,
    data: HashMap<Agent, FileUsage>,
    loading: bool,
    agent_menu: bool,
    /// When the agent menu was closed by an outside click (so that click doesn't reopen it).
    agent_menu_closed: Option<std::time::Instant>,
    /// Day under the mouse in the daily chart.
    hover_day: Option<usize>,
    /// Plot area of the daily chart, from the last paint.
    chart_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>,
    scroll: gpui::ScrollHandle,
    /// Report for (agent, days, record count); rebuilt only when one of them changes.
    report: Option<((Agent, u32, usize), std::rc::Rc<UsageReport>)>,
}

impl UsageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            agent: Agent::Claude,
            days: 7,
            scanner: Arc::default(),
            data: HashMap::new(),
            loading: false,
            agent_menu: false,
            agent_menu_closed: None,
            hover_day: None,
            chart_bounds: Default::default(),
            scroll: gpui::ScrollHandle::new(),
            report: None,
        };
        view.load(cx);
        view
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let agent = self.agent;
        let scanner = self.scanner.clone();
        let task = cx.background_spawn(async move { scanner.lock().map(|mut s| s.scan(agent)).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let usage = task.await;
            let _ = this.update(cx, |this, cx| {
                this.data.insert(agent, usage);
                this.loading = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select_agent(&mut self, agent: Agent, cx: &mut Context<Self>) {
        self.agent = agent;
        self.agent_menu = false;
        if !self.data.contains_key(&agent) {
            self.load(cx);
        }
        cx.notify();
    }
}

pub(crate) fn card() -> Div {
    div().rounded_lg().border_1().border_color(hex(Chrome::BORDER)).bg(hex(0x232323))
}

fn panel(title: &str, color: u32) -> Div {
    card().flex().flex_col().child(
        div().px_4().py_2p5().border_b_1().border_color(hex(Chrome::BORDER)).t_body().text_color(hex(color)).child(title.to_string()),
    )
}

pub(crate) fn kpi(label: &str, value: String, sub: String, color: u32) -> Div {
    card()
        .flex_1()
        .min_w(px(140.))
        .px_4()
        .py_3()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(label.to_string()))
        .child(div().t_display().font_weight(crate::theme::EMPHASIS).text_color(hex(color)).child(value))
        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(sub))
}

fn bar_row(rank: usize, name: String, value: String, detail: String, fraction: f32, color: u32) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .t_body()
                .child(div().w(px(16.)).t_small().text_color(hex(Chrome::MUTED)).child(rank.to_string()))
                .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(name))
                .child(div().t_small().text_color(hex(color)).child(value))
                .child(div().w(px(64.)).flex().justify_end().t_small().text_color(hex(Chrome::MUTED)).child(detail)),
        )
        .child(
            div()
                .ml(px(24.))
                .h(px(4.))
                .rounded_full()
                .bg(hex(0x333333))
                .child(div().h_full().rounded_full().bg(hex(color)).w(relative(fraction.clamp(0.0, 1.0).max(0.004)))),
        )
}

fn tooltip_row(label: &str, value: String, color: u32) -> Div {
    div()
        .flex()
        .justify_between()
        .gap_2()
        .child(div().text_color(hex(Chrome::MUTED)).child(label.to_string()))
        .child(div().text_color(hex(color)).child(value))
}

fn short_project(path: &str) -> String {
    let home = crate::launch::home_dir();
    let trimmed = path.strip_prefix(&*home.to_string_lossy()).unwrap_or(path);
    let trimmed = trimmed.trim_start_matches('/');
    if trimmed.is_empty() {
        "~".into()
    } else {
        trimmed.to_string()
    }
}

impl Render for UsageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let today = Local::now().date_naive();
        let report = self.data.get(&self.agent).map(|usage| {
            let key = (self.agent, self.days, usage.requests.len() + usage.tools.len());
            match &self.report {
                Some((cached, report)) if *cached == key => report.clone(),
                _ => {
                    let report = std::rc::Rc::new(UsageReport::build(usage, today, self.days));
                    self.report = Some((key, report.clone()));
                    report
                }
            }
        });
        let agent_label = self.agent.display_name();

        let mut periods = div().flex().gap_1();
        for days in PERIODS {
            let label = if days == 1 { t(cx, "usage.today").to_string() } else { tf(cx, "usage.days", &[("n", &days.to_string())]) };
            periods = periods.child(chip(
                ("period", days as usize),
                label,
                self.days == days,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.days = days;
                    cx.notify();
                }),
            ));
        }

        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().t_heading().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.usage")))
            .child(
                div()
                    .relative()
                    .child(action_button(
                        "agent-select",
                        format!("{agent_label} ▾"),
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if this.agent_menu_closed.take().is_some_and(|at| at.elapsed().as_millis() < 350) {
                                return;
                            }
                            this.agent_menu = !this.agent_menu;
                            cx.notify();
                        }),
                    ))
                    .when(self.agent_menu, |d| {
                        // Deferred: painted above the cards below the header.
                        d.child(
                            gpui::deferred(
                                popover()
                                    .id("agent-menu")
                                    .absolute()
                                    .top(px(26.))
                                    .left_0()
                                    .w(px(160.))
                                    .occlude()
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        if std::mem::take(&mut this.agent_menu) {
                                            this.agent_menu_closed = Some(std::time::Instant::now());
                                        }
                                        cx.notify();
                                    }))
                                    .child(crate::ui::menu_item(
                                        "agent-claude",
                                        "Claude Code",
                                        cx.listener(|this, _: &ClickEvent, _, cx| this.select_agent(Agent::Claude, cx)),
                                    ))
                                    .child(crate::ui::menu_item(
                                        "agent-codex",
                                        "Codex",
                                        cx.listener(|this, _: &ClickEvent, _, cx| this.select_agent(Agent::Codex, cx)),
                                    )),
                            )
                            .with_priority(2),
                        )
                    }),
            )
            .child(
                div()
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .bg(hex(0x2a2a2a))
                    .t_small()
                    .text_color(hex(Chrome::MUTED))
                    .child(crate::ui::tilde(&root_dir(self.agent))),
            )
            .child(action_button("usage-refresh", t(cx, "usage.refresh"), cx.listener(|this, _: &ClickEvent, _, cx| this.load(cx))))
            .when(self.loading, |d| d.child(crate::ui::spinner(crate::ui::IconSize::INLINE, hex(Chrome::MUTED))))
            .child(div().flex_1())
            .child(periods);

        let body: gpui::AnyElement = match report {
            None => div().pt_10().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "usage.loading")).into_any_element(),
            Some(report) if report.totals.calls == 0 => div()
                .pt_10()
                .t_body()
                .text_color(hex(Chrome::MUTED))
                .child(if self.loading { t(cx, "usage.loading") } else { t(cx, "usage.empty") })
                .into_any_element(),
            Some(report) => self.render_report(&report, cx).into_any_element(),
        };

        div()
            .relative()
            .size_full()
            .bg(hex(Chrome::EDITOR))
            .child(
                div()
                    .id("usage-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().px_6().py_5().flex().flex_col().gap_4().child(header).child(body)),
            )
            .group(crate::ui::SCROLL_GROUP)
            .child(crate::ui::scrollbar(self.scroll.clone()))
    }
}

impl UsageView {
    fn render_report(&self, report: &UsageReport, cx: &mut Context<Self>) -> impl IntoElement {
        let totals = &report.totals;
        let priced = totals.unpriced_calls < totals.calls;
        let period =
            if self.days == 1 { t(cx, "usage.today").to_string() } else { tf(cx, "usage.period", &[("n", &self.days.to_string())]) };

        let kpis = div()
            .flex()
            .gap_3()
            .child(kpi(t(cx, "usage.total_cost"), if priced { money(totals.cost) } else { "—".into() }, period, Chrome::PURPLE))
            .child(kpi(t(cx, "usage.calls"), compact_number(totals.calls), String::new(), Chrome::BLUE))
            .child(kpi(t(cx, "usage.sessions"), totals.sessions.to_string(), String::new(), Chrome::GREEN))
            .child(kpi(t(cx, "usage.cache_hit"), format!("{:.1}%", totals.cache_hit_rate() * 100.0), String::new(), Chrome::ORANGE))
            .child(kpi(t(cx, "usage.input"), compact_number(totals.input), "input".into(), 0x8a9bb0))
            .child(kpi(t(cx, "usage.output"), compact_number(totals.output), "output".into(), 0x8a9bb0));

        let unpriced = (totals.unpriced_calls > 0).then(|| {
            div().t_small().text_color(hex(Chrome::WARNING)).child(tf(cx, "usage.unpriced", &[("n", &totals.unpriced_calls.to_string())]))
        });

        // Daily chart: cost (purple) and calls (blue), each scaled to its own maximum.
        let max_cost = report.days.iter().map(|d| d.totals.cost).fold(0.0, f64::max);
        let max_calls = report.days.iter().map(|d| d.totals.calls).max().unwrap_or(0).max(1);
        let series_cost: Vec<f32> =
            report.days.iter().map(|d| if max_cost > 0.0 { (d.totals.cost / max_cost) as f32 } else { 0.0 }).collect();
        let series_calls: Vec<f32> = report.days.iter().map(|d| d.totals.calls as f32 / max_calls as f32).collect();
        let axis_label =
            |fraction: f64| if priced { money(max_cost * fraction) } else { compact_number((max_calls as f64 * fraction) as u64) };
        let label_step = (report.days.len() / 7).max(1);
        let chart_bounds = self.chart_bounds.clone();
        let hover = self.hover_day.filter(|i| *i < report.days.len());
        let day_count = report.days.len();
        let tooltip = hover.map(|i| {
            let day = &report.days[i];
            let n = day_count.max(2) - 1;
            let plot_width = self.chart_bounds.get().map(|b| f32::from(b.size.width)).unwrap_or(0.);
            let x = 56. + plot_width * (i as f32 / n as f32);
            // Beside the guide line, flipping left near the right edge.
            let left = if x + 14. + 180. <= 56. + plot_width { x + 14. } else { (x - 194.).max(0.) };
            div()
                .absolute()
                .top(px(0.))
                .left(px(left))
                .w(px(180.))
                .p_2()
                .flex()
                .flex_col()
                .gap_0p5()
                .rounded_md()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .shadow_lg()
                .t_small()
                .child(
                    div()
                        .font_weight(crate::theme::EMPHASIS)
                        .text_color(hex(Chrome::BRIGHT))
                        .child(day.date.format("%Y-%m-%d").to_string()),
                )
                .child(tooltip_row(t(cx, "usage.cost"), if priced { money(day.totals.cost) } else { "—".into() }, Chrome::PURPLE))
                .child(tooltip_row(t(cx, "usage.calls"), compact_number(day.totals.calls), Chrome::BLUE))
                .child(tooltip_row(t(cx, "usage.input"), compact_number(day.totals.input), Chrome::FOREGROUND))
                .child(tooltip_row(t(cx, "usage.output"), compact_number(day.totals.output), Chrome::FOREGROUND))
                .child(tooltip_row(t(cx, "usage.cache_read"), compact_number(day.totals.cache_read), Chrome::FOREGROUND))
        });
        let x_labels = div().flex().justify_between().pl(px(56.)).t_small().text_color(hex(Chrome::MUTED)).children(
            report
                .days
                .iter()
                .enumerate()
                .filter(|(i, _)| i % label_step == 0 || *i == report.days.len() - 1)
                .map(|(_, d)| d.date.format("%m-%d").to_string()),
        );
        let chart = panel(t(cx, "usage.daily"), Chrome::BLUE).child(
            div()
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .id("usage-chart")
                        .relative()
                        .h(px(170.))
                        .flex()
                        // Hovering picks the nearest day for the tooltip.
                        .on_mouse_move(cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                            let Some(bounds) = this.chart_bounds.get() else { return };
                            let width = f32::from(bounds.size.width);
                            if width <= 0. || !bounds.contains(&event.position) || day_count == 0 {
                                if this.hover_day.take().is_some() {
                                    cx.notify();
                                }
                                return;
                            }
                            let fraction = (f32::from(event.position.x - bounds.origin.x) / width).clamp(0., 1.);
                            let index = (fraction * (day_count.max(2) - 1) as f32).round() as usize;
                            if this.hover_day != Some(index.min(day_count - 1)) {
                                this.hover_day = Some(index.min(day_count - 1));
                                cx.notify();
                            }
                        }))
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if !hovered && this.hover_day.take().is_some() {
                                cx.notify();
                            }
                        }))
                        .child(
                            div()
                                .w(px(56.))
                                .flex()
                                .flex_col()
                                .justify_between()
                                .t_small()
                                .text_color(hex(Chrome::MUTED))
                                .children([1.0, 0.75, 0.5, 0.25, 0.0].map(axis_label)),
                        )
                        .child(
                            canvas(
                                move |bounds, _, _| chart_bounds.set(Some(bounds)),
                                move |bounds, _, window, _| {
                                    let (o, w, h) = (bounds.origin, bounds.size.width, bounds.size.height);
                                    for step in 0..=4 {
                                        let y = o.y + h * (step as f32 / 4.0);
                                        let mut grid = PathBuilder::stroke(px(1.));
                                        grid.move_to(point(o.x, y));
                                        grid.line_to(point(o.x + w, y));
                                        if let Ok(path) = grid.build() {
                                            window.paint_path(path, hex_alpha(0xffffff, 0.06));
                                        }
                                    }
                                    let plot = |series: &[f32]| -> Vec<Point<Pixels>> {
                                        let n = series.len().max(2) - 1;
                                        series
                                            .iter()
                                            .enumerate()
                                            .map(|(i, v)| point(o.x + w * (i as f32 / n as f32), o.y + h * (1.0 - v)))
                                            .collect()
                                    };
                                    if let Some(i) = hover {
                                        let n = series_calls.len().max(2) - 1;
                                        let x = o.x + w * (i as f32 / n as f32);
                                        let mut guide = PathBuilder::stroke(px(1.));
                                        guide.move_to(point(x, o.y));
                                        guide.line_to(point(x, o.y + h));
                                        if let Ok(path) = guide.build() {
                                            window.paint_path(path, hex_alpha(0xffffff, 0.25));
                                        }
                                    }
                                    for (series, color) in [(&series_calls, Chrome::BLUE), (&series_cost, Chrome::PURPLE)] {
                                        let points = plot(series);
                                        if points.len() < 2 {
                                            continue;
                                        }
                                        let mut line = PathBuilder::stroke(px(2.));
                                        line.move_to(points[0]);
                                        points.iter().skip(1).for_each(|p| line.line_to(*p));
                                        if let Ok(path) = line.build() {
                                            window.paint_path(path, hex(color));
                                        }
                                        for p in &points {
                                            let mut dot = PathBuilder::fill();
                                            dot.move_to(point(p.x - px(2.5), p.y));
                                            dot.line_to(point(p.x, p.y - px(2.5)));
                                            dot.line_to(point(p.x + px(2.5), p.y));
                                            dot.line_to(point(p.x, p.y + px(2.5)));
                                            dot.close();
                                            if let Ok(path) = dot.build() {
                                                window.paint_path(path, hex(color));
                                            }
                                        }
                                    }
                                },
                            )
                            .flex_1()
                            .h_full(),
                        )
                        .children(tooltip),
                )
                .child(x_labels)
                .child(
                    div()
                        .flex()
                        .gap_4()
                        .pl(px(56.))
                        .t_small()
                        .text_color(hex(Chrome::MUTED))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().w(px(14.)).h(px(2.)).bg(hex(Chrome::PURPLE)))
                                .child(t(cx, "usage.cost")),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().w(px(14.)).h(px(2.)).bg(hex(Chrome::BLUE)))
                                .child(t(cx, "usage.calls")),
                        ),
                ),
        );

        // Token composition.
        let all = totals.total_tokens().max(1) as f32;
        let parts = [
            ("Input", totals.input, 0x4f8ef7),
            ("Output", totals.output, Chrome::PURPLE),
            (t(cx, "usage.cache_write"), totals.cache_write, Chrome::ORANGE),
            (t(cx, "usage.cache_read"), totals.cache_read, 0x4caf7d),
        ];
        let mix_bar = div().h(px(14.)).rounded_full().overflow_hidden().flex().children(
            parts.iter().filter(|(_, v, _)| *v > 0).map(|(_, v, c)| div().h_full().bg(hex(*c)).w(relative((*v as f32 / all).max(0.004)))),
        );
        let legend = div().flex().gap_4().t_small().children(parts.iter().map(|(name, v, c)| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().size(px(8.)).rounded_sm().bg(hex(*c)))
                .child(div().text_color(hex(Chrome::FOREGROUND)).child(name.to_string()))
                .child(div().text_color(hex(Chrome::BRIGHT)).child(compact_number(*v)))
                .child(div().text_color(hex(Chrome::MUTED)).child(format!("({:.1}%)", *v as f32 / all * 100.0)))
        }));
        let mix =
            panel(t(cx, "usage.mix"), Chrome::FOREGROUND).child(div().p_4().flex().flex_col().gap_3().child(mix_bar).child(legend).child(
                div().t_small().text_color(hex(Chrome::MUTED)).child(format!(
                    "{}: {}",
                    t(cx, "usage.cache_reuse"),
                    compact_number(totals.cache_read)
                )),
            ));

        let ranked = |items: &[NamedUsage], by_cost: bool, color: u32, project: bool, sess: &str| {
            let max = items.iter().map(|i| if by_cost { i.cost } else { i.tokens as f64 }).fold(0.0, f64::max).max(f64::EPSILON);
            div().p_4().flex().flex_col().gap_3().children(items.iter().take(8).enumerate().map(|(i, item)| {
                let value = if by_cost { money(item.cost) } else { compact_number(item.tokens) };
                let detail = if project { format!("{} {sess}", item.sessions) } else { format!("{} calls", item.calls) };
                let name = if project { short_project(&item.name) } else { item.name.clone() };
                let amount = if by_cost { item.cost } else { item.tokens as f64 };
                bar_row(i + 1, name, value, detail, (amount / max) as f32, color)
            }))
        };
        let sess = t(cx, "usage.sess");
        let models =
            panel(t(cx, "usage.models"), Chrome::PURPLE).flex_1().child(ranked(&report.models, priced, Chrome::PURPLE, false, sess));
        let projects = panel(t(cx, "usage.projects"), Chrome::GREEN).flex_1().child(ranked(&report.projects, priced, 0x4caf7d, true, sess));

        let max_tool = report.tools.first().map(|(_, c)| *c).unwrap_or(1).max(1);
        let tools = panel(t(cx, "usage.tools"), Chrome::ORANGE).child(div().p_4().flex().flex_col().gap_3().children(
            report.tools.iter().take(16).enumerate().collect::<Vec<_>>().chunks(2).map(|pair| {
                div()
                    .flex()
                    .gap_8()
                    .children(pair.iter().map(|(i, (name, count))| {
                        div().flex_1().min_w_0().child(bar_row(
                            i + 1,
                            name.clone(),
                            count.to_string(),
                            String::new(),
                            *count as f32 / max_tool as f32,
                            Chrome::ORANGE,
                        ))
                    }))
                    .when(pair.len() == 1, |d| d.child(div().flex_1()))
            }),
        ));

        let header_cell = |text: &str| div().flex_1().t_small().text_color(hex(Chrome::MUTED)).child(text.to_string());
        let mut table = div().p_4().flex().flex_col().child(
            div()
                .flex()
                .pb_2()
                .border_b_1()
                .border_color(hex(Chrome::BORDER))
                .child(header_cell(t(cx, "usage.date")))
                .child(header_cell(t(cx, "usage.cost")))
                .child(header_cell(t(cx, "usage.calls")))
                .child(header_cell(t(cx, "usage.input")))
                .child(header_cell(t(cx, "usage.output"))),
        );
        for day in report.days.iter().rev().filter(|d| d.totals.calls > 0) {
            let is_today = day.date == Local::now().date_naive();
            table = table.child(
                div()
                    .flex()
                    .py_1p5()
                    .border_b_1()
                    .border_color(hex(0x2a2a2a))
                    .t_body()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .gap_2()
                            .items_center()
                            .text_color(hex(Chrome::PURPLE))
                            .child(day.date.format("%Y-%m-%d").to_string())
                            .when(is_today, |d| {
                                d.child(div().px_1().rounded_sm().bg(hex_alpha(Chrome::PURPLE, 0.25)).t_small().child(t(cx, "usage.today")))
                            }),
                    )
                    .child(div().flex_1().text_color(hex(Chrome::PURPLE)).child(if priced { money(day.totals.cost) } else { "—".into() }))
                    .child(div().flex_1().text_color(hex(Chrome::BLUE)).child(day.totals.calls.to_string()))
                    .child(div().flex_1().text_color(hex(Chrome::FOREGROUND)).child(compact_number(day.totals.input)))
                    .child(div().flex_1().text_color(hex(Chrome::FOREGROUND)).child(compact_number(day.totals.output))),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(kpis)
            .children(unpriced)
            .child(chart)
            .child(mix)
            .child(div().flex().gap_4().child(models).child(projects))
            .child(tools)
            .child(panel(t(cx, "usage.table"), Chrome::FOREGROUND).child(table))
    }
}
