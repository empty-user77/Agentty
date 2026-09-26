//! Menu bar popover: clicking the Agentty icon opens a panel under it with plan usage, the agent
//! panes of every window and shortcuts to show the window, mini mode and quit.
//!
//! It is a GPUI pop-up window (a non-activating panel), so it can take keys without bringing
//! Agentty to the front. It closes when it loses key status (a click in another Agentty window),
//! on a click in another app (a global mouse monitor), on Escape, or when the icon is clicked again.

use crate::i18n::{t, tf};
use crate::status_item::{self, push_action, TrayAction};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use crate::workbench::mini::AgentSummary;
use crate::workbench::AccountUsage;
use gpui::{
    div, prelude::*, px, size, App, Bounds, ClickEvent, Context, FocusHandle, Global, KeyDownEvent, SharedString, Subscription, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};
use std::time::{Duration, Instant};

const WIDTH: f32 = 340.;
/// Space between the menu bar and the popover.
const GAP: f64 = 6.;
/// Distance kept from the screen's side edges.
const MARGIN: f64 = 8.;
/// Opening height before the first frame measures the real one.
const INITIAL_HEIGHT: f32 = 320.;
/// Agent rows shown before the list scrolls.
const MAX_AGENT_ROWS: usize = 7;
const AGENT_ROW: f32 = 44.;
/// A click on the icon right after the popover closed (it lost key status to that click) must
/// not reopen it.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

#[derive(Default)]
struct PopoverWindow {
    handle: Option<WindowHandle<TrayPopover>>,
    closed_at: Option<Instant>,
}

impl Global for PopoverWindow {}

/// Handles the popover's own actions; `false` for actions meant for a workbench.
pub fn handle(action: TrayAction, cx: &mut App) -> bool {
    match action {
        TrayAction::TogglePopover => {
            if is_open(cx) {
                close(cx);
            } else {
                open(cx);
            }
            true
        }
        TrayAction::ClosePopover => {
            close(cx);
            true
        }
        _ => false,
    }
}

fn is_open(cx: &App) -> bool {
    cx.try_global::<PopoverWindow>().is_some_and(|p| p.handle.is_some())
}

pub fn close(cx: &mut App) {
    let Some(handle) = cx.try_global::<PopoverWindow>().and_then(|p| p.handle) else { return };
    cx.set_global(PopoverWindow { handle: None, closed_at: Some(Instant::now()) });
    status_item::set_highlighted(false);
    status_item::watch_outside_clicks(false);
    let _ = handle.update(cx, |_, window, _| window.remove_window());
}

fn open(cx: &mut App) {
    let recently_closed = cx.try_global::<PopoverWindow>().and_then(|p| p.closed_at).is_some_and(|at| at.elapsed() < REOPEN_GUARD);
    if recently_closed {
        return;
    }
    let Some(anchor) = status_item::anchor() else { return };
    let display = cx.displays().into_iter().find(|d| u32::from(d.id()) == anchor.display);
    // Bounds are relative to the display's top-left corner.
    let origin = gpui::point(px(anchor.popover_x(WIDTH as f64, MARGIN) as f32), px((anchor.bottom + GAP) as f32));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, size(px(WIDTH), px(INITIAL_HEIGHT))))),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: display.map(|d| d.id()),
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| cx.new(|cx| TrayPopover::new(window, cx))) {
        Ok(handle) => {
            cx.set_global(PopoverWindow { handle: Some(handle), closed_at: None });
            status_item::set_highlighted(true);
            status_item::watch_outside_clicks(true);
        }
        Err(err) => eprintln!("agentty: menu bar popover failed to open: {err:#}"),
    }
}

/// Development snapshot of the open popover (`AGENTTY_DEBUG=1`).
pub fn debug_snapshot(path: std::path::PathBuf, cx: &mut App) {
    let Some(handle) = cx.try_global::<PopoverWindow>().and_then(|p| p.handle) else { return };
    let number = handle.update(cx, |_, window, _| crate::native::ns_window(window).map(crate::native::window_number)).ok().flatten();
    if let Some(number) = number {
        if let Err(err) = crate::debug::capture_window(number as u32, &path) {
            eprintln!("agentty: snapshot failed: {err:#}");
        }
    }
}

/// Everything the popover shows, gathered from all Agentty windows.
struct Snapshot {
    agents: Vec<AgentSummary>,
    plugins: Vec<crate::workbench::mini::PluginActivitySummary>,
    usage: Vec<AccountUsage>,
    mini: bool,
}

impl Snapshot {
    fn gather(cx: &App) -> Self {
        let mut snapshot = Snapshot { agents: Vec::new(), plugins: Vec::new(), usage: Vec::new(), mini: false };
        for window in crate::workbenches(cx) {
            let Ok(workbench) = window.read(cx) else { continue };
            snapshot.agents.extend(workbench.agent_summaries(cx));
            snapshot.plugins.extend(workbench.plugin_activity_summaries(cx));
            if snapshot.usage.is_empty() {
                snapshot.usage = workbench.account_usage().to_vec();
            }
            snapshot.mini |= workbench.is_mini();
        }
        // Agents that need the user first, then working ones, then the most recently active.
        snapshot.agents.sort_by_key(|a| (!a.needs_user, !a.working, std::cmp::Reverse(a.last_activity_ms)));
        // Working automations first, then the rest in the order their windows reported them.
        snapshot.plugins.sort_by_key(|p| p.state != crate::plugins::InstanceState::Working);
        snapshot
    }
}

pub struct TrayPopover {
    focus: FocusHandle,
    height: f32,
    _activation: Subscription,
}

impl TrayPopover {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let activation = cx.observe_window_activation(window, |_, window, _| {
            if !window.is_window_active() {
                push_action(TrayAction::ClosePopover);
            }
        });
        // Elapsed times and statuses move while it is open.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();
        Self { focus, height: INITIAL_HEIGHT, _activation: activation }
    }

    /// Queues `action` for the app loop and closes the popover.
    fn run(action: TrayAction) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        move |_, _, _| {
            push_action(TrayAction::ClosePopover);
            push_action(action.clone());
        }
    }

    fn header(&self, snapshot: &Snapshot, cx: &App) -> impl IntoElement {
        let working = snapshot.agents.iter().filter(|a| a.working).count()
            + snapshot.plugins.iter().filter(|p| p.state == crate::plugins::InstanceState::Working).count();
        let waiting = snapshot.agents.iter().filter(|a| a.needs_user).count();
        let summary = if snapshot.agents.is_empty() && snapshot.plugins.is_empty() {
            t(cx, "mini.no_agents").to_string()
        } else if working > 0 {
            tf(cx, "mini.working", &[("n", &working.to_string())])
        } else {
            tf(cx, "mini.agents", &[("n", &snapshot.agents.len().to_string())])
        };
        let mini_tip: SharedString = t(cx, "mini.enter").into();
        div()
            .px_3()
            .pt_3()
            .pb_2()
            .flex()
            .items_center()
            .gap_2p5()
            .child(gpui::img("brand/logo.png").size(px(28.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child("Agentty"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .t_caption()
                            .text_color(hex(Chrome::MUTED))
                            .when(working > 0, |d| d.child(div().size(px(6.)).rounded_full().bg(hex(Chrome::ORANGE))))
                            .child(summary)
                            .when(waiting > 0, |d| {
                                d.child(
                                    div()
                                        .px_1p5()
                                        .rounded_sm()
                                        .bg(hex_alpha(Chrome::ATTENTION, 0.18))
                                        .text_color(hex(Chrome::ATTENTION))
                                        .child(tf(cx, "tray.waiting", &[("n", &waiting.to_string())])),
                                )
                            }),
                    ),
            )
            .child(
                crate::ui::icon_only_sized("tray-mini", "picture-in-picture-2", 28., 15., Self::run(TrayAction::ToggleMini))
                    .when(snapshot.mini, |d| d.bg(hex(Chrome::SELECTED)))
                    .tooltip(crate::ui::Tooltip::text(mini_tip, None)),
            )
    }

    fn usage_section(usage: &[AccountUsage], cx: &App) -> impl IntoElement {
        let mut rows = div().flex().flex_col().gap_2p5().px_3().pb_3();
        for account in usage {
            let brand = crate::brand::brand(account.agent.id());
            let mut row = div().flex().flex_col().gap_1p5().child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(crate::brand::avatar(account.agent.id(), 20.))
                    .child(
                        div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::BRIGHT)).child(account.agent.display_name()),
                    )
                    .child(div().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(account.cost_label())),
            );
            // Limits are only written while an agent works: say so instead of passing off old numbers.
            if let Some(age) = account.stale_for(crate::ui::now_ms()) {
                row = row.child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                    cx,
                    "tray.limits_stale",
                    &[("ago", &crate::ui::relative_time(age, 0))],
                )));
            }
            for limit in account.windows() {
                let percent = limit.used_percent.clamp(0., 100.) as f32;
                let bar_color = if percent >= 90. {
                    Chrome::ERROR
                } else if percent >= 70. {
                    Chrome::WARNING
                } else {
                    brand.color
                };
                row = row.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .t_caption()
                                .text_color(hex(Chrome::MUTED))
                                .child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .child(t(cx, limit.label))
                                        .child(div().text_color(hex(Chrome::FOREGROUND)).child(format!("{percent:.0}%"))),
                                )
                                .child(limit.resets_in(cx)),
                        )
                        .child(
                            div()
                                .h(px(5.))
                                .w_full()
                                .rounded_full()
                                .bg(hex_alpha(Chrome::BRIGHT, 0.08))
                                .child(div().h_full().w(gpui::relative(percent / 100.)).rounded_full().bg(hex(bar_color))),
                        ),
                );
            }
            rows = rows.child(row);
        }
        Self::card(
            "tray-usage",
            div()
                .id("tray-usage-title")
                .h(px(30.))
                .px_3()
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                .child(div().flex_1().child(t(cx, "tray.usage_title")))
                .child(
                    div()
                        .id("tray-usage-refresh")
                        .p_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(hex_alpha(Chrome::BRIGHT, 0.08)))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "usage.refresh"), None))
                        // Stays open: the numbers land in the panel the user is looking at.
                        .on_click(|_, _, cx| {
                            cx.stop_propagation();
                            push_action(TrayAction::RefreshUsage);
                        })
                        .child(crate::ui::icon("refresh-cw", 11., hex(Chrome::MUTED))),
                )
                .child(crate::ui::icon("chevron-right", 12., hex(Chrome::MUTED)))
                .on_click(Self::run(TrayAction::OpenUsage)),
            rows,
        )
    }

    fn agents_section(agents: &[AgentSummary], plugins: &[crate::workbench::mini::PluginActivitySummary], cx: &App) -> impl IntoElement {
        let mut rows = div().id("tray-agents").flex().flex_col().pb_1().max_h(px(AGENT_ROW * MAX_AGENT_ROWS as f32)).overflow_y_scroll();
        if agents.is_empty() && plugins.is_empty() {
            rows = rows.child(div().px_3().py_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "mini.empty_hint")));
        }
        for agent in agents {
            let pane_id = agent.pane_id;
            rows = rows.child(
                div()
                    .id(("tray-agent", pane_id as usize))
                    .h(px(AGENT_ROW))
                    .mx_1()
                    .px_2()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(Self::run(TrayAction::Focus(pane_id)))
                    .child(div().relative().child(crate::brand::avatar(agent.tool, 24.)).when(agent.working || agent.needs_user, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(-1.))
                                .right(px(-1.))
                                .size(px(9.))
                                .rounded_full()
                                .border_2()
                                .border_color(hex(Chrome::OVERLAY))
                                .bg(hex(if agent.needs_user { Chrome::ATTENTION } else { Chrome::ORANGE })),
                        )
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().t_small().truncate().text_color(hex(Chrome::BRIGHT)).child(agent.title.clone()))
                            .child(div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(agent.workspace.clone())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(120.))
                            .truncate()
                            .t_caption()
                            .text_color(hex(agent.color))
                            .when(agent.needs_user, |d| d.px_1p5().rounded_sm().bg(hex_alpha(Chrome::ATTENTION, 0.18)))
                            .child(agent.status_line()),
                    ),
            );
        }
        for plugin in plugins {
            let (plugin_id, instance) = (plugin.plugin.clone(), plugin.instance.clone());
            let state_color = match plugin.state {
                crate::plugins::InstanceState::Working => Chrome::ORANGE,
                crate::plugins::InstanceState::Error => Chrome::ATTENTION,
                crate::plugins::InstanceState::Idle => Chrome::MUTED,
            };
            let status_key = match plugin.state {
                crate::plugins::InstanceState::Working => "plugins.activity.working",
                crate::plugins::InstanceState::Idle => "plugins.activity.idle",
                crate::plugins::InstanceState::Error => "plugins.activity.error",
            };
            let status = match plugin.elapsed {
                Some(seconds) => format!("{} · {}", t(cx, status_key), crate::workbench::format_elapsed(seconds)),
                None => t(cx, status_key).to_string(),
            };
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("tray-plugin-{plugin_id}-{instance}")))
                    .h(px(AGENT_ROW))
                    .mx_1()
                    .px_2()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(Self::run(TrayAction::FocusPlugin { plugin: plugin_id.clone(), instance: instance.clone() }))
                    .child(
                        div().relative().child(crate::ui::plugin_mark(plugin.logo.clone(), plugin.glyph, 24., hex(Chrome::BRIGHT))).when(
                            plugin.state != crate::plugins::InstanceState::Idle,
                            |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .bottom(px(-1.))
                                        .right(px(-1.))
                                        .size(px(9.))
                                        .rounded_full()
                                        .border_2()
                                        .border_color(hex(Chrome::OVERLAY))
                                        .bg(hex(state_color)),
                                )
                            },
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().t_small().truncate().text_color(hex(Chrome::BRIGHT)).child(plugin.title.clone()))
                            .children(
                                plugin.text.clone().map(|text| div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(text)),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(120.))
                            .truncate()
                            .t_caption()
                            .text_color(hex(state_color))
                            .when(plugin.state == crate::plugins::InstanceState::Error, |d| {
                                d.px_1p5().rounded_sm().bg(hex_alpha(Chrome::ATTENTION, 0.18))
                            })
                            .child(status),
                    ),
            );
        }
        Self::card("tray-agents-card", div().h(px(30.)).px_3().flex().items_center().child(t(cx, "tray.agents")), rows)
    }

    /// A rounded section with a small title.
    fn card(id: &'static str, title: impl IntoElement, body: impl IntoElement) -> impl IntoElement {
        div()
            .id(id)
            .mx_2()
            .flex()
            .flex_col()
            .rounded_lg()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .child(div().t_caption().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::MUTED)).child(title))
            .child(body)
    }

    fn footer(cx: &App) -> impl IntoElement {
        let button = |id: &'static str, icon: &'static str, label: SharedString, action: TrayAction| {
            div()
                .id(id)
                .h(px(28.))
                .px_2p5()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .text_color(hex(Chrome::FOREGROUND))
                .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                .on_click(Self::run(action))
                .child(crate::ui::icon(icon, 14., hex(Chrome::MUTED)))
                .child(label)
        };
        div()
            .px_2()
            .py_2()
            .flex()
            .items_center()
            .justify_between()
            .child(button("tray-show", "app-window", t(cx, "tray.show").into(), TrayAction::Show))
            .child(button("tray-quit", "power", t(cx, "tray.quit").into(), TrayAction::Quit))
    }
}

impl Render for TrayPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = Snapshot::gather(cx);
        // The window follows the panel's measured height (after this frame: resizing
        // synchronously re-enters the window being painted).
        let entity = cx.entity().downgrade();
        let measure = gpui::canvas(
            move |bounds, window, cx| {
                let height = f32::from(bounds.size.height);
                let Some(current) = entity.upgrade().map(|e| e.read(cx).height) else { return };
                if (height - current).abs() <= 0.5 {
                    return;
                }
                let ns = crate::native::ns_window(window);
                let entity = entity.clone();
                // Outside this update: AppKit calls back into GPUI while resizing.
                cx.spawn(async move |cx| {
                    let _ = entity.update(cx, |this, _| this.height = height);
                    if let Some(ns) = ns {
                        crate::native::set_frame(ns, crate::native::frame(ns).with_height_from_top(height as f64), false);
                    }
                })
                .detach();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let panel = div()
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_xl()
            .bg(hex(Chrome::SIDE_BAR))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .overflow_hidden()
            .child(measure)
            .child(self.header(&snapshot, cx))
            .when(!snapshot.usage.is_empty(), |d| d.child(Self::usage_section(&snapshot.usage, cx)))
            .child(Self::agents_section(&snapshot.agents, &snapshot.plugins, cx))
            .child(div().mx_2().h(px(1.)).bg(hex(Chrome::BORDER)))
            .child(Self::footer(cx));

        div()
            .track_focus(&self.focus)
            .size_full()
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, _| {
                if event.keystroke.key == "escape" {
                    push_action(TrayAction::ClosePopover);
                }
            }))
            .child(crate::ui::fade_in("tray-popover", panel))
    }
}
