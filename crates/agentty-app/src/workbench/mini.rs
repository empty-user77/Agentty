//! Mini background mode: the main window folds into a small always-on-top panel at the right
//! edge of the screen that lists agent panes. Finished or waiting agents pop up as speech
//! bubbles; clicking a row opens that terminal in a small peek window beside the panel, and the
//! peek's ⤢ button (or the expand button) brings the full window back.

use super::{status_label, Workbench};
use crate::i18n::{t, tf};
use crate::launch::PaneKind;
use crate::native::{self, Frame};
use crate::terminal::NoticeKind;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use gpui::{
    div, prelude::*, px, size, AnyWindowHandle, App, Bounds, ClickEvent, Context, Entity, Subscription, WeakEntity, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};
use std::time::{Duration, Instant};

pub const MINI_WIDTH: f32 = 300.;
const MARGIN: f64 = 16.;
const HEADER: f32 = 40.;
const ROW: f32 = 46.;
const BUBBLE: f32 = 64.;
const MAX_BUBBLES: usize = 3;
/// Idle agents listed before "More" (working and waiting ones are always listed).
const RECENT_ROWS: usize = 5;
const MAX_ROWS: usize = 16;
const MORE_ROW: f32 = 30.;
/// The peek window that a row opens: big enough to read a turn, small enough to stay a peek.
const PEEK_WIDTH: f32 = 560.;
const PEEK_HEIGHT: f32 = 380.;
const PEEK_HEADER: f32 = 30.;

/// One agent pane as the mini panel and the menu bar show it.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentSummary {
    pub pane_id: u64,
    pub kind: PaneKind,
    /// Brand id for the logo.
    pub tool: &'static str,
    /// Waiting for a permission or an answer.
    pub needs_user: bool,
    pub last_activity_ms: u64,
    pub title: String,
    pub workspace: String,
    pub status: String,
    pub color: u32,
    pub working: bool,
    pub waiting: bool,
    pub elapsed: Option<u64>,
}

impl AgentSummary {
    /// Status, with how long it has been working ("Working · 2m 10s").
    pub fn status_line(&self) -> String {
        match self.elapsed.filter(|_| self.working) {
            Some(seconds) => format!("{} · {}", self.status, super::layout::format_elapsed(seconds)),
            None => self.status.clone(),
        }
    }
}

#[derive(Clone)]
struct Bubble {
    pane_id: u64,
    title: String,
    text: String,
    color: u32,
    at: Instant,
}

pub struct MiniState {
    pub handle: WindowHandle<MiniView>,
    pub view: Entity<MiniView>,
    /// Main window frame to restore.
    pub saved_frame: Option<Frame>,
    /// The terminal being peeked at beside the panel, if any.
    pub peek: Option<(WindowHandle<MiniPeek>, u64)>,
}

/// One terminal shown beside the mini panel, without unfolding the whole app.
pub struct MiniPeek {
    workbench: WeakEntity<Workbench>,
    main_window: AnyWindowHandle,
    pane: super::Pane,
    pane_id: u64,
    _observe: Subscription,
}

impl MiniPeek {
    fn new(workbench: &Entity<Workbench>, main_window: AnyWindowHandle, pane: super::Pane, cx: &mut Context<Self>) -> Self {
        let pane_id = pane.read(cx).pane_id;
        Self { workbench: workbench.downgrade(), main_window, _observe: cx.observe(&pane, |_, _, cx| cx.notify()), pane, pane_id }
    }
}

impl gpui::Render for MiniPeek {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.pane.read(cx).display_title();
        let tool = self.pane.read(cx).tool_id();
        let pane_id = self.pane_id;
        let main = self.main_window;
        let workbench = self.workbench.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .rounded_lg()
            .overflow_hidden()
            .bg(hex(Chrome::EDITOR))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .child(
                div()
                    .h(px(PEEK_HEADER))
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(hex(Chrome::TAB_INACTIVE))
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(crate::brand::avatar(tool, 14.))
                    .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::BRIGHT)).child(title))
                    // The full app, with this pane in front: the old behaviour, now a button.
                    .child(
                        crate::ui::icon_only("mini-peek-expand", "maximize-2", move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            cx.defer(move |cx| {
                                let _ = main.update(cx, |root, window, cx| {
                                    if let Ok(handle) = root.downcast::<Workbench>() {
                                        handle.update(cx, |wb, cx| wb.exit_mini(Some(pane_id), window, cx));
                                    }
                                });
                            });
                        })
                        .tooltip(crate::ui::Tooltip::text(t(cx, "mini.peek_expand"), None)),
                    )
                    .child(crate::ui::icon_only("mini-peek-close", "x", move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let workbench = workbench.clone();
                        cx.defer(move |cx| {
                            let _ = workbench.update(cx, |wb, cx| wb.close_mini_peek(cx));
                        });
                    })),
            )
            .child(div().flex_1().min_h_0().child(self.pane.clone()))
    }
}

pub struct MiniView {
    workbench: WeakEntity<Workbench>,
    pub(super) main_window: AnyWindowHandle,
    bubbles: Vec<Bubble>,
    height: f32,
    /// "More" pressed: every agent is listed and the panel grows.
    expanded: bool,
    _observe: Subscription,
}

impl MiniView {
    fn new(workbench: &Entity<Workbench>, main_window: AnyWindowHandle, cx: &mut Context<Self>) -> Self {
        // Elapsed times tick while agents work.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();
        Self {
            workbench: workbench.downgrade(),
            main_window,
            bubbles: Vec::new(),
            height: 0.,
            expanded: false,
            _observe: cx.observe(workbench, |_, _, cx| cx.notify()),
        }
    }

    pub fn push_bubble(&mut self, pane_id: u64, title: String, text: String, kind: NoticeKind, cx: &mut Context<Self>) {
        let color = match kind {
            NoticeKind::Finished => Chrome::SUCCESS,
            NoticeKind::Permission | NoticeKind::Question => Chrome::ATTENTION,
            _ => Chrome::PURPLE,
        };
        self.bubbles.retain(|b| b.pane_id != pane_id);
        self.bubbles.insert(0, Bubble { pane_id, title, text, color, at: Instant::now() });
        self.bubbles.truncate(MAX_BUBBLES);
        cx.notify();
    }

    /// Shows one terminal beside the panel, leaving the app folded away.
    fn peek(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let workbench = self.workbench.clone();
        cx.defer(move |cx| {
            let _ = workbench.update(cx, |wb, cx| wb.open_mini_peek(pane_id, cx));
        });
    }

    fn restore(&mut self, focus: Option<u64>, cx: &mut Context<Self>) {
        let main = self.main_window;
        // Deferred: the restore closes this window, which can't happen inside its own update.
        cx.defer(move |cx| {
            let _ = main.update(cx, |root, window, cx| {
                if let Ok(workbench) = root.downcast::<Workbench>() {
                    workbench.update(cx, |wb, cx| wb.exit_mini(focus, window, cx));
                }
            });
        });
    }

    fn desired_height(rows: usize, bubbles: usize, more: bool) -> f32 {
        let rows = rows.clamp(1, MAX_ROWS);
        bubbles as f32 * (BUBBLE + 8.) + HEADER + rows as f32 * ROW + if more { MORE_ROW } else { 0. } + 10.
    }

    /// Working and waiting agents first, then the most recently active; `hidden` = how many "More" reveals.
    fn arrange(mut agents: Vec<AgentSummary>, expanded: bool) -> (Vec<AgentSummary>, usize) {
        agents.sort_by_key(|a| (!a.working, !a.needs_user, std::cmp::Reverse(a.last_activity_ms)));
        let busy = agents.iter().filter(|a| a.working || a.needs_user).count();
        let limit = if expanded { MAX_ROWS } else { (busy + RECENT_ROWS).min(MAX_ROWS) };
        let hidden = agents.len().saturating_sub(limit);
        agents.truncate(limit);
        (agents, if expanded { 0 } else { hidden })
    }
}

impl Render for MiniView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(workbench) = self.workbench.upgrade() else { return div().into_any_element() };
        let all = workbench.read(cx).agent_summaries(cx);
        let live: Vec<u64> = all.iter().map(|a| a.pane_id).collect();
        self.bubbles.retain(|b| live.contains(&b.pane_id));
        let total = all.len();
        let working = all.iter().filter(|a| a.working).count();
        let (agents, hidden) = Self::arrange(all, self.expanded);
        let can_collapse = self.expanded && total > agents.len().min(RECENT_ROWS);

        let height = Self::desired_height(agents.len(), self.bubbles.len(), hidden > 0 || can_collapse);
        if (height - self.height).abs() > 0.5 {
            self.height = height;
            if let Some(ns) = native::ns_window(window) {
                // After this frame: resizing synchronously re-enters the window we're rendering.
                cx.spawn(async move |_, _| {
                    let frame = native::frame(ns);
                    native::set_frame(ns, frame.with_height_from_top(height as f64), true);
                })
                .detach();
            }
        }

        let summary = if total == 0 {
            t(cx, "mini.no_agents").to_string()
        } else if working > 0 {
            tf(cx, "mini.working", &[("n", &working.to_string())])
        } else {
            tf(cx, "mini.agents", &[("n", &total.to_string())])
        };

        let mut bubbles = div().flex().flex_col().gap_2();
        for (index, bubble) in self.bubbles.clone().into_iter().enumerate() {
            let (pane_id, color) = (bubble.pane_id, bubble.color);
            let ago = crate::ui::relative_time(bubble.at.elapsed().as_millis() as u64, 0);
            bubbles = bubbles.child(
                div()
                    .id(("mini-bubble", index))
                    .relative()
                    .h(px(BUBBLE))
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap_0p5()
                    .rounded_xl()
                    .bg(hex(Chrome::OVERLAY))
                    .border_1()
                    .border_color(hex_alpha(bubble.color, 0.7))
                    .shadow_lg()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::SELECTED)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.restore(Some(pane_id), cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .t_small()
                            .child(div().size(px(7.)).rounded_full().bg(hex(bubble.color)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(bubble.title),
                            )
                            .child(div().text_color(hex(Chrome::MUTED)).child(ago)),
                    )
                    .child(div().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(bubble.text))
                    // Tail pointing down at the panel.
                    .child(
                        gpui::canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let (x, y) = (bounds.origin.x, bounds.origin.y);
                                let mut path = gpui::PathBuilder::fill();
                                path.move_to(gpui::point(x, y));
                                path.line_to(gpui::point(x + px(14.), y));
                                path.line_to(gpui::point(x + px(7.), y + px(7.)));
                                path.close();
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, hex_alpha(color, 0.7));
                                }
                            },
                        )
                        .absolute()
                        .bottom(px(-8.))
                        .right(px(26.))
                        .w(px(14.))
                        .h(px(8.)),
                    ),
            );
        }

        let mut rows = div().flex().flex_col();
        if agents.is_empty() {
            rows = rows.child(
                div().h(px(ROW)).px_3().flex().items_center().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "mini.empty_hint")),
            );
        }
        for agent in agents.iter() {
            let pane_id = agent.pane_id;
            let status = agent.status_line();
            rows = rows.child(
                div()
                    .id(("mini-row", pane_id as usize))
                    .h(px(ROW))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    // A row peeks at that terminal; the peek's ⤢ opens the full window.
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.peek(pane_id, cx)))
                    .child(
                        div()
                            .relative()
                            .child(crate::brand::avatar(agent.tool, 22.))
                            // Busy ring: working (orange) or waiting for the user (attention).
                            .when(agent.working || agent.needs_user, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .bottom(px(-1.))
                                        .right(px(-1.))
                                        .size(px(8.))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(hex(Chrome::SIDE_BAR))
                                        .bg(hex(if agent.needs_user { Chrome::ATTENTION } else { Chrome::ORANGE })),
                                )
                            }),
                    )
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
                            .child(status),
                    ),
            );
        }
        if hidden > 0 || can_collapse {
            let label = if hidden > 0 { tf(cx, "mini.more", &[("n", &hidden.to_string())]) } else { t(cx, "mini.less").to_string() };
            rows = rows.child(
                div()
                    .id("mini-more")
                    .h(px(MORE_ROW))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .cursor_pointer()
                    .t_caption()
                    .text_color(hex(Chrome::BLUE))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(label)
                    .child(crate::ui::icon(if hidden > 0 { "chevron-down" } else { "chevron-up" }, 12., hex(Chrome::BLUE)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.expanded = !this.expanded;
                        cx.notify();
                    })),
            );
        }

        let panel = div()
            .flex()
            .flex_col()
            .rounded_xl()
            .bg(hex(Chrome::SIDE_BAR))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .h(px(HEADER))
                    .pl_3()
                    .pr_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(
                        div().relative().child(gpui::img("brand/logo.png").size(px(20.))).child(
                            div()
                                .absolute()
                                .bottom(px(-1.))
                                .right(px(-2.))
                                .size(px(7.))
                                .rounded_full()
                                .border_1()
                                .border_color(hex(Chrome::SIDE_BAR))
                                .bg(hex(if working > 0 { Chrome::ORANGE } else { Chrome::SUCCESS })),
                        ),
                    )
                    .child(div().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child("Agentty"))
                    .child(div().flex_1().min_w_0().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(summary))
                    .child(crate::ui::icon_only(
                        "mini-expand",
                        "arrow-up-right",
                        cx.listener(|this, _: &ClickEvent, _, cx| this.restore(None, cx)),
                    )),
            )
            .child(rows);

        div().size_full().flex().flex_col().gap_2().p(px(4.)).child(bubbles).child(panel).into_any_element()
    }
}

impl Workbench {
    pub fn agent_summaries(&self, cx: &App) -> Vec<AgentSummary> {
        let mut out = Vec::new();
        for ws in &self.workspaces {
            let workspace = self.workspace_title(ws, cx);
            for tab in &ws.tabs {
                for pane in tab.root.leaves() {
                    let view = pane.read(cx);
                    if view.tool_id() == "shell" || !view.is_running() {
                        continue;
                    }
                    let (status, color) = status_label(view, cx);
                    out.push(AgentSummary {
                        pane_id: view.pane_id,
                        kind: view.display_kind(),
                        tool: view.tool_id(),
                        needs_user: view.status.needs_user(),
                        last_activity_ms: view.last_activity_ms,
                        title: view.display_title(),
                        workspace: workspace.clone(),
                        status,
                        color,
                        working: view.status.in_turn(),
                        waiting: view.attention,
                        elapsed: view.working_since.map(|t| t.elapsed().as_secs()),
                    });
                }
            }
        }
        out
    }

    pub fn tray_state(&self, cx: &App) -> crate::status_item::TrayState {
        let agents = self.agent_summaries(cx);
        crate::status_item::TrayState {
            working: agents.iter().filter(|a| a.working).count(),
            asking: agents.iter().filter(|a| a.needs_user).count(),
            done: agents.iter().filter(|a| a.waiting && !a.working && !a.needs_user).count(),
        }
    }

    /// Spend and plan limits per agent account, for the menu bar popover.
    pub fn account_usage(&self) -> &[super::AccountUsage] {
        &self.account_usage
    }

    pub fn handle_tray(&mut self, action: crate::status_item::TrayAction, window: &mut Window, cx: &mut Context<Self>) {
        use crate::status_item::TrayAction;
        match action {
            TrayAction::Show | TrayAction::Focus(_) if self.mini.is_some() => {
                let focus = if let TrayAction::Focus(id) = action { Some(id) } else { None };
                self.exit_mini(focus, window, cx);
            }
            TrayAction::Show => {
                window.activate_window();
                cx.activate(true);
            }
            TrayAction::Focus(id) => {
                window.activate_window();
                cx.activate(true);
                self.jump_to_pane_id(id, window, cx);
            }
            TrayAction::ToggleMini => self.toggle_mini(window, cx),
            TrayAction::OpenUsage => {
                if self.mini.is_some() {
                    self.exit_mini(None, window, cx);
                }
                window.activate_window();
                cx.activate(true);
                self.page = Some(super::Page::Usage);
                cx.notify();
            }
            TrayAction::RefreshUsage => self.refresh_account_usage(cx),
            // Outside this window's update, so it can ask about unsaved files first.
            TrayAction::Quit => cx.defer(crate::request_quit),
            // Handled by the app loop (`tray_popover`).
            TrayAction::TogglePopover | TrayAction::ClosePopover => {}
        }
    }

    pub fn is_mini(&self) -> bool {
        self.mini.is_some()
    }

    pub fn toggle_mini(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "mini" }));
        if self.mini.is_some() {
            self.exit_mini(None, window, cx);
        } else {
            self.onboarding_event(super::onboarding::TourEvent::MiniEntered, cx);
            self.enter_mini(window, cx);
        }
    }

    /// Opens (or moves) the peek window beside the mini panel, showing `pane_id`'s terminal.
    /// It goes on whichever side of the panel has room, so the panel stays readable.
    pub(super) fn open_mini_peek(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let Some(pane) = self.all_panes().into_iter().find(|p| p.read(cx).pane_id == pane_id) else { return };
        if self.mini.as_ref().is_some_and(|m| m.peek.as_ref().is_some_and(|(_, id)| *id == pane_id)) {
            return self.close_mini_peek(cx);
        }
        self.close_mini_peek(cx);
        let Some(mini) = self.mini.as_ref() else { return };
        let panel = mini.handle.update(cx, |_, window, _| native::ns_window(window).map(native::frame)).ok().flatten();
        let Some(panel) = panel else { return };
        let display = cx.displays().into_iter().next();
        let bounds = display.as_ref().map(|d| d.bounds());
        // Left of the panel by default; right of it when the panel sits at the left edge.
        let (screen_left, screen_width) =
            bounds.map(|b| (f32::from(b.origin.x) as f64, f32::from(b.size.width) as f64)).unwrap_or((0., PEEK_WIDTH as f64 * 3.));
        let gap = 10.;
        let left = if panel.x - gap - PEEK_WIDTH as f64 >= screen_left {
            panel.x - gap - PEEK_WIDTH as f64
        } else {
            (panel.x + panel.width + gap).min(screen_left + screen_width - PEEK_WIDTH as f64)
        };
        let top = f32::from(bounds.map(|b| b.origin.y).unwrap_or(px(0.))) as f64;
        let screen_height = bounds.map(|b| f32::from(b.size.height) as f64).unwrap_or(900.);
        let top_inset = (screen_height - (panel.y + panel.height)).max(0.) + top;
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                gpui::point(px(left as f32), px(top_inset as f32)),
                size(px(PEEK_WIDTH), px(PEEK_HEIGHT)),
            ))),
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: true,
            is_resizable: true,
            is_minimizable: false,
            display_id: display.as_ref().map(|d| d.id()),
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };
        let workbench = cx.entity();
        let main_handle = mini.view.read(cx).main_window;
        let Ok(handle) = cx.open_window(options, move |_, cx| cx.new(|cx| MiniPeek::new(&workbench, main_handle, pane, cx))) else {
            return;
        };
        if let Some(mini) = self.mini.as_mut() {
            mini.peek = Some((handle, pane_id));
        }
        cx.notify();
    }

    pub(super) fn close_mini_peek(&mut self, cx: &mut Context<Self>) {
        let Some((handle, _)) = self.mini.as_mut().and_then(|m| m.peek.take()) else { return };
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        cx.notify();
    }

    /// Folds the window into the top-right corner, then shows the mini panel there.
    pub fn enter_mini(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mini.is_some() || self.mini_opening {
            return;
        }
        let Some(main) = native::ns_window(window) else { return };
        let saved = native::frame(main);
        let visible = native::is_visible(main);
        let area = native::visible_frame(main).unwrap_or(saved);
        let rows = self.agent_summaries(cx).len().min(RECENT_ROWS);
        let height = MiniView::desired_height(rows, 0, false) as f64;
        // Each Agentty window gets its own mini panel, side by side from the right edge.
        let shift = self.slot as f64 * (MINI_WIDTH as f64 + 12.);
        let mut target = Frame::top_right(area, MINI_WIDTH as f64, height, MARGIN);
        target.x -= shift;
        // GPUI window bounds are top-left based on the window's display.
        let display = window.display(cx);
        let display_bounds =
            display.as_ref().map(|d| d.bounds()).unwrap_or_else(|| Bounds::new(gpui::point(px(0.), px(0.)), size(px(1440.), px(900.))));
        let top_inset = (f32::from(display_bounds.size.height) as f64 - (area.y + area.height)).max(0.);
        let origin = gpui::point(
            display_bounds.origin.x + display_bounds.size.width - px(MINI_WIDTH + MARGIN as f32 + shift as f32),
            display_bounds.origin.y + px((top_inset + MARGIN) as f32),
        );
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, size(px(MINI_WIDTH), px(height as f32))))),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: true,
            is_resizable: false,
            is_minimizable: false,
            display_id: display.as_ref().map(|d| d.id()),
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };
        let workbench = cx.entity();
        let main_handle = window.window_handle();
        self.mini_opening = true;
        // Outside this update: the fold animation re-enters the window, and the mini panel's
        // first frame reads the workbench.
        cx.spawn(async move |this, cx| {
            if visible {
                native::set_frame(main, target, true);
            }
            let opened = cx.update(|cx| {
                let slot: std::rc::Rc<std::cell::RefCell<Option<Entity<MiniView>>>> = Default::default();
                let captured = slot.clone();
                let handle = cx.open_window(options, move |_, cx| {
                    let view = cx.new(|cx| MiniView::new(&workbench, main_handle, cx));
                    *captured.borrow_mut() = Some(view.clone());
                    view
                });
                let view = slot.borrow_mut().take();
                (handle, view)
            });
            let Ok((Ok(handle), Some(view))) = opened else {
                native::set_frame(main, saved, false);
                let _ = this.update(cx, |this, _| this.mini_opening = false);
                return;
            };
            native::order_out(main);
            native::set_frame(main, saved, false);
            let _ = this.update(cx, |this, cx| {
                this.mini_opening = false;
                this.mini = Some(MiniState { handle, view, saved_frame: Some(saved), peek: None });
                cx.notify();
            });
        })
        .detach();
    }

    /// Closes the mini panel and grows the main window back from it.
    pub fn exit_mini(&mut self, focus: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_mini_peek(cx);
        let Some(mini) = self.mini.take() else { return };
        self.onboarding_event(super::onboarding::TourEvent::MiniLeft, cx);
        let mini_frame = mini.handle.update(cx, |_, w, _| native::ns_window(w).map(native::frame)).ok().flatten();
        let _ = mini.handle.update(cx, |_, w, _| w.remove_window());
        if let Some(main) = native::ns_window(window) {
            let saved = mini.saved_frame.unwrap_or_else(|| native::frame(main));
            if let Some(from) = mini_frame {
                native::set_frame(main, from, false);
            }
            window.activate_window();
            cx.activate(true);
            // Grow back after this update (the animation re-enters the window).
            cx.spawn(async move |_, _| native::set_frame(main, saved, true)).detach();
        } else {
            window.activate_window();
            cx.activate(true);
        }
        match focus {
            Some(pane_id) => {
                self.jump_to_pane_id(pane_id, window, cx);
            }
            None => self.focus_active(window, cx),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::MiniView;

    #[test]
    fn height_grows_with_rows_and_bubbles() {
        let base = MiniView::desired_height(0, 0, false);
        assert_eq!(base, MiniView::desired_height(1, 0, false));
        assert!(MiniView::desired_height(3, 1, false) > MiniView::desired_height(3, 0, false));
        assert_eq!(MiniView::desired_height(50, 0, false), MiniView::desired_height(super::MAX_ROWS, 0, false));
    }

    #[test]
    fn working_agents_first_then_recent_five() {
        let agent = |id: u64, working: bool, needs_user: bool, at: u64| super::AgentSummary {
            pane_id: id,
            kind: crate::launch::PaneKind::Claude,
            tool: "claude",
            needs_user,
            last_activity_ms: at,
            title: String::new(),
            workspace: String::new(),
            status: String::new(),
            color: 0,
            working,
            waiting: false,
            elapsed: None,
        };
        let agents = (1..=9).map(|i| agent(i, i == 7, i == 3, i * 10)).collect();
        let (shown, hidden) = MiniView::arrange(agents, false);
        assert_eq!(shown.iter().map(|a| a.pane_id).collect::<Vec<_>>(), vec![7, 3, 9, 8, 6, 5, 4]);
        assert_eq!(hidden, 2);
        let agents = (1..=9).map(|i| agent(i, false, false, i)).collect();
        assert_eq!(MiniView::arrange(agents, true).1, 0);
    }
}
