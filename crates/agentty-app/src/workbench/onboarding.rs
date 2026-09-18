//! First-run onboarding: a welcome, the handful of settings worth choosing on day one, then a
//! follow-along tour. The first two steps are a dialog; the tour is a card that stays out of the way
//! and teaches one feature at a time as numbered steps. Only the step to do now is lit: it says
//! where to click (the real button gets a pulsing ring) and which keys do the same, the app notices
//! when it was done, ticks it, says what just happened on screen, and lights the next step. "Do it
//! for me" performs exactly that one step. Nothing moves on to the next feature by itself.
//! It can be reopened from Settings → About and the command palette.

use super::{Page, Workbench};
use crate::i18n::{t, tf};
use crate::launch::{home_dir, LaunchSpec, PaneKind};
use crate::settings::{settings, update_settings, Language, LinkOpener, Settings};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{chip, icon, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, SharedString, Window};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Welcome,
    Basics,
    Tour,
}

/// A feature of the tour.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Task {
    Split,
    Browser,
    Files,
    Flow,
    Mini,
    Monitoring,
}

/// Something the user did that a step waits for and that leaves no state behind to look at.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum TourEvent {
    Split,
    PaneClosed,
    MiniEntered,
    MiniLeft,
    FolderOpened,
}

/// How the tour knows a step was done.
enum Check {
    /// The app is in this state (a page is open, a panel is closed, …).
    State(fn(&Workbench) -> bool),
    Event(TourEvent),
    /// Nothing to detect (something to look at or try freely): the user says "got it".
    Manual,
}

type Demo = fn(&mut Workbench, &mut Window, &mut Context<Workbench>);

/// One numbered step: what to do, and what happened once it is done.
struct TourStep {
    text: &'static str,
    done: &'static str,
    shortcut: Option<&'static str>,
    /// Element id of the real control, which gets a ring while this step is the one to do.
    target: Option<&'static str>,
    check: Check,
    /// "Do it for me".
    demo: Option<Demo>,
}

struct Guide {
    glyph: &'static str,
    title: &'static str,
    /// What the feature is for.
    what: &'static str,
    steps: Vec<TourStep>,
}

fn in_monitoring(w: &Workbench) -> bool {
    matches!(w.page, Some(Page::Usage | Page::Processes | Page::Proxy))
}

impl Task {
    fn all() -> Vec<Task> {
        let mut tasks = vec![Task::Split, Task::Flow];
        if crate::platform::HAS_MINI_MODE {
            tasks.push(Task::Mini);
        }
        if crate::platform::HAS_WEBVIEW {
            tasks.push(Task::Browser);
        }
        tasks.extend([Task::Files, Task::Monitoring]);
        tasks
    }

    fn guide(self) -> Guide {
        let step = |text, done, shortcut, target, check, demo| TourStep { text, done, shortcut, target, check, demo };
        match self {
            Task::Split => Guide {
                glyph: "columns-2",
                title: "onboarding.split",
                what: "onboarding.split_what",
                steps: vec![
                    step(
                        "onboarding.split_1",
                        "onboarding.split_1_done",
                        Some("⌘T"),
                        None,
                        Check::State(|w| w.active_pane().is_some()),
                        Some(|w, window, cx| w.create_workspace(LaunchSpec::new(PaneKind::Shell, home_dir()), window, cx)),
                    ),
                    step(
                        "onboarding.split_2",
                        "onboarding.split_2_done",
                        Some("⌘D"),
                        Some("header-split-right"),
                        Check::Event(TourEvent::Split),
                        Some(|w, window, cx| {
                            w.page = None;
                            w.split(super::Axis::Horizontal, window, cx)
                        }),
                    ),
                    step(
                        "onboarding.split_3",
                        "onboarding.split_3_done",
                        Some("⌘W"),
                        None,
                        Check::Event(TourEvent::PaneClosed),
                        Some(|w, _, cx| {
                            // Only ever a pane of a split: the last pane of a tab is left alone.
                            let split = w
                                .workspaces
                                .get(w.active_workspace)
                                .and_then(|ws| ws.tabs.get(ws.active_tab))
                                .is_some_and(|tab| tab.root.leaves().len() > 1);
                            match w.active_pane().filter(|_| split) {
                                Some(pane) => w.remove_pane(&pane, cx),
                                None => w.onboarding_event(TourEvent::PaneClosed, cx),
                            }
                        }),
                    ),
                ],
            },
            Task::Flow => Guide {
                glyph: "workflow",
                title: "onboarding.flow",
                what: "onboarding.flow_what",
                steps: vec![
                    step(
                        "onboarding.flow_1",
                        "onboarding.flow_1_done",
                        Some("⇧⌘F"),
                        Some("activity-flow"),
                        Check::State(|w| w.page == Some(Page::Flow)),
                        Some(|w, _, cx| w.open_page(Page::Flow, cx)),
                    ),
                    step("onboarding.flow_2", "onboarding.flow_2_done", None, None, Check::Manual, None),
                    step(
                        "onboarding.flow_3",
                        "onboarding.flow_3_done",
                        Some("⇧⌘F"),
                        Some("activity-flow"),
                        Check::State(|w| w.page != Some(Page::Flow)),
                        Some(|w, _, cx| {
                            w.page = None;
                            cx.notify();
                        }),
                    ),
                ],
            },
            Task::Mini => Guide {
                glyph: "picture-in-picture-2",
                title: "onboarding.mini",
                what: "onboarding.mini_what",
                steps: vec![
                    step(
                        "onboarding.mini_1",
                        "onboarding.mini_1_done",
                        Some("⌃⌘M"),
                        Some("header-mini"),
                        Check::Event(TourEvent::MiniEntered),
                        Some(|w, window, cx| w.toggle_mini(window, cx)),
                    ),
                    step("onboarding.mini_2", "onboarding.mini_2_done", Some("⌃⌘M"), None, Check::Event(TourEvent::MiniLeft), None),
                ],
            },
            Task::Browser => Guide {
                glyph: "globe",
                title: "onboarding.browser",
                what: "onboarding.browser_what",
                steps: vec![
                    step(
                        "onboarding.browser_1",
                        "onboarding.browser_1_done",
                        Some("⇧⌘B"),
                        Some("header-browser"),
                        Check::State(|w| w.browser.is_some()),
                        Some(|w, window, cx| w.toggle_browser(window, cx)),
                    ),
                    step("onboarding.browser_2", "onboarding.browser_2_done", None, None, Check::Manual, None),
                    step(
                        "onboarding.browser_3",
                        "onboarding.browser_3_done",
                        Some("⇧⌘B"),
                        Some("header-browser"),
                        Check::State(|w| w.browser.is_none()),
                        Some(|w, window, cx| w.toggle_browser(window, cx)),
                    ),
                ],
            },
            Task::Files => Guide {
                glyph: "panel-right",
                title: "onboarding.files",
                what: "onboarding.files_what",
                steps: vec![
                    step(
                        "onboarding.files_1",
                        "onboarding.files_1_done",
                        Some("⌥⌘B"),
                        Some("header-files"),
                        Check::State(|w| w.files_panel.is_some()),
                        Some(|w, _, cx| w.toggle_files_panel(cx)),
                    ),
                    step(
                        "onboarding.files_2",
                        "onboarding.files_2_done",
                        None,
                        None,
                        Check::Event(TourEvent::FolderOpened),
                        Some(|w, _, cx| w.open_first_folder(cx)),
                    ),
                    step(
                        "onboarding.files_3",
                        "onboarding.files_3_done",
                        Some("⌥⌘B"),
                        Some("header-files"),
                        Check::State(|w| w.files_panel.is_none()),
                        Some(|w, _, cx| w.toggle_files_panel(cx)),
                    ),
                ],
            },
            Task::Monitoring => Guide {
                glyph: "chart-column",
                title: "onboarding.monitoring",
                what: "onboarding.monitoring_what",
                steps: vec![
                    step(
                        "onboarding.monitoring_1",
                        "onboarding.monitoring_1_done",
                        Some("⌥⌘U"),
                        Some("activity-usage"),
                        Check::State(in_monitoring),
                        Some(|w, _, cx| w.open_page(Page::Usage, cx)),
                    ),
                    step(
                        "onboarding.monitoring_2",
                        "onboarding.monitoring_2_done",
                        None,
                        None,
                        Check::State(|w| w.page == Some(Page::Proxy)),
                        Some(|w, _, cx| {
                            w.page = Some(Page::Proxy);
                            cx.notify();
                        }),
                    ),
                    step(
                        "onboarding.monitoring_3",
                        "onboarding.monitoring_3_done",
                        Some("⌥⌘U"),
                        Some("activity-usage"),
                        Check::State(|w| !in_monitoring(w)),
                        Some(|w, _, cx| {
                            w.page = None;
                            cx.notify();
                        }),
                    ),
                ],
            },
        }
    }
}

pub(super) struct Onboarding {
    step: Step,
    /// Task the tour card is teaching right now.
    current: Task,
    /// Steps finished, per task.
    progress: HashMap<Task, usize>,
    /// Events the open task still waits for, as they happen.
    seen: HashSet<TourEvent>,
}

impl Onboarding {
    /// The dialog steps sit on top of everything (the native browser view has to hide for them).
    pub(super) fn is_modal(&self) -> bool {
        self.step != Step::Tour
    }

    fn finished(&self, task: Task) -> bool {
        self.progress.get(&task).copied().unwrap_or(0) >= task.guide().steps.len()
    }
}

impl Workbench {
    /// Shown once, in the first window of a fresh install.
    pub(super) fn first_run_onboarding(&mut self, cx: &mut Context<Self>) {
        if self.slot == 0 && !settings(cx).onboarding_done && std::env::var("AGENTTY_BACKGROUND").as_deref() != Ok("1") {
            self.open_onboarding(cx);
        }
    }

    pub(super) fn open_onboarding(&mut self, cx: &mut Context<Self>) {
        self.page = None;
        self.onboarding = Some(Onboarding { step: Step::Welcome, current: Task::all()[0], progress: HashMap::new(), seen: HashSet::new() });
        cx.notify();
    }

    fn close_onboarding(&mut self, cx: &mut Context<Self>) {
        self.onboarding = None;
        update_settings(cx, |s| s.onboarding_done = true);
        cx.notify();
    }

    /// Something a step may be waiting for just happened. Only what the open task still waits for is
    /// kept, so doing things out of order (or before the tour) never ticks a step later on.
    pub(super) fn onboarding_event(&mut self, event: TourEvent, cx: &mut Context<Self>) {
        let Some(onboarding) = self.onboarding.as_mut().filter(|o| o.step == Step::Tour) else { return };
        let from = onboarding.progress.get(&onboarding.current).copied().unwrap_or(0);
        let awaited = onboarding.current.guide().steps.iter().skip(from).any(|s| matches!(s.check, Check::Event(e) if e == event));
        if awaited && onboarding.seen.insert(event) {
            cx.notify();
        }
    }

    /// Ticks every step of the open task that is done by now. Called on render.
    pub(super) fn advance_tour(&mut self, cx: &mut Context<Self>) {
        let Some((task, mut at)) = self
            .onboarding
            .as_ref()
            .filter(|o| o.step == Step::Tour)
            .map(|o| (o.current, o.progress.get(&o.current).copied().unwrap_or(0)))
        else {
            return;
        };
        let steps = task.guide().steps;
        let start = at;
        while let Some(step) = steps.get(at) {
            let done = match step.check {
                Check::State(check) => check(self),
                Check::Event(event) => self.onboarding.as_mut().is_some_and(|o| o.seen.remove(&event)),
                Check::Manual => false,
            };
            if !done {
                break;
            }
            at += 1;
        }
        if at != start {
            if let Some(onboarding) = self.onboarding.as_mut() {
                onboarding.progress.insert(task, at);
            }
            cx.notify();
        }
    }

    /// Element id of the control the step to do now wants pressed, for a ring around it.
    pub(super) fn tour_target(&self) -> Option<&'static str> {
        let onboarding = self.onboarding.as_ref().filter(|o| o.step == Step::Tour)?;
        let at = onboarding.progress.get(&onboarding.current).copied().unwrap_or(0);
        onboarding.current.guide().steps.get(at).and_then(|step| step.target)
    }

    /// "Do it for me" (or "Got it" for a step with nothing to detect): exactly the step to do now.
    fn onboarding_do_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((task, at)) = self.onboarding.as_ref().map(|o| (o.current, o.progress.get(&o.current).copied().unwrap_or(0))) else {
            return;
        };
        let steps = task.guide().steps;
        let Some(step) = steps.get(at) else { return };
        match (&step.check, step.demo) {
            (Check::Manual, _) | (_, None) => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.progress.insert(task, at + 1);
                }
            }
            (_, Some(demo)) => demo(self, window, cx),
        }
        cx.notify();
    }

    /// Moves the card to `task` (the next one, or one picked from the progress dots).
    fn onboarding_go(&mut self, task: Task, cx: &mut Context<Self>) {
        if let Some(onboarding) = self.onboarding.as_mut() {
            onboarding.current = task;
            onboarding.seen.clear();
        }
        cx.notify();
    }

    /// Debug driver: `tour` (state), `tour do` (the step to do now, for me), `tour next`, `tour start`.
    pub(super) fn debug_tour(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) -> serde_json::Value {
        match argument {
            "start" => {
                self.open_onboarding(cx);
                self.go_to(Step::Tour, cx);
            }
            "do" => self.onboarding_do_step(window, cx),
            "next" => {
                let next = self.onboarding.as_ref().and_then(|o| Task::all().into_iter().find(|t| !o.finished(*t)));
                if let Some(next) = next {
                    self.onboarding_go(next, cx);
                }
            }
            _ => {}
        }
        self.advance_tour(cx);
        match self.onboarding.as_ref() {
            Some(o) => serde_json::json!({
                "current": format!("{:?}", o.current),
                "step": o.progress.get(&o.current).copied().unwrap_or(0),
                "of": o.current.guide().steps.len(),
                "finished": Task::all().into_iter().filter(|t| o.finished(*t)).map(|t| format!("{t:?}")).collect::<Vec<_>>(),
                "ring": self.tour_target(),
            }),
            None => serde_json::json!(null),
        }
    }

    pub(super) fn render_onboarding(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let onboarding = self.onboarding.as_ref()?;
        Some(match onboarding.step {
            Step::Welcome => self.render_onboarding_dialog(self.render_onboarding_welcome(cx)),
            Step::Basics => self.render_onboarding_dialog(self.render_onboarding_basics(cx)),
            Step::Tour => self.render_onboarding_tour(onboarding, cx),
        })
    }

    fn render_onboarding_dialog(&self, card: gpui::Div) -> AnyElement {
        div()
            .id("onboarding-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hex_alpha(0x000000, 0.55))
            .occlude()
            .child(
                card.w(px(620.))
                    .max_h(gpui::relative(0.9))
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .rounded_xl()
                    .bg(hex(Chrome::OVERLAY))
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .shadow_lg(),
            )
            .into_any_element()
    }

    fn onboarding_button(
        &self,
        id: &'static str,
        label: &str,
        primary: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .h(px(30.))
            .px_4()
            .flex()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .t_body()
            .when(primary, |d| d.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)).hover(|s| s.opacity(0.9)))
            .when(!primary, |d| d.text_color(hex(Chrome::MUTED)).hover(|s| s.text_color(hex(Chrome::BRIGHT)).bg(hex(Chrome::HOVER))))
            .child(label.to_string())
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| on_click(this, window, cx)))
    }

    fn go_to(&mut self, step: Step, cx: &mut Context<Self>) {
        if let Some(onboarding) = self.onboarding.as_mut() {
            onboarding.step = step;
        }
        cx.notify();
    }

    fn render_onboarding_welcome(&self, cx: &mut Context<Self>) -> gpui::Div {
        let prefs = settings(cx).clone();
        let mut languages = div().flex().flex_wrap().gap_1();
        for language in Language::ALL {
            languages = languages.child(chip(
                SharedString::from(format!("onboarding-lang-{language:?}")),
                if language == Language::System { t(cx, "settings.language_system") } else { language.native_name() },
                prefs.language == language,
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    update_settings(cx, move |s| s.language = language);
                    crate::set_app_menus(cx);
                }),
            ));
        }
        let point = |glyph: &'static str, color: u32, title: &'static str, body: &'static str, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(crate::brand::tinted_tile(color, 28.).mt_0p5().child(icon(glyph, IconSize::BUTTON, hex(color))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().t_body().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, title)))
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, body))),
                )
        };
        div()
            .child(
                div().flex().items_center().gap_3().child(gpui::img("brand/logo.png").size(px(52.))).child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .t_heading()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(hex(Chrome::BRIGHT))
                                .child(t(cx, "onboarding.welcome")),
                        )
                        .child(div().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "tagline"))),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(point("columns-2", Chrome::BLUE, "onboarding.point_terminals", "onboarding.point_terminals_body", cx))
                    .child(point("workflow", Chrome::PURPLE, "onboarding.point_sessions", "onboarding.point_sessions_body", cx))
                    .child(point("rocket", Chrome::ORANGE, "onboarding.point_ship", "onboarding.point_ship_body", cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "settings.language")))
                    .child(languages),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.onboarding_button(
                        "onboarding-skip",
                        t(cx, "onboarding.skip"),
                        false,
                        |this, _, cx| this.close_onboarding(cx),
                        cx,
                    ))
                    .child(self.onboarding_button(
                        "onboarding-start",
                        t(cx, "onboarding.start"),
                        true,
                        |this, _, cx| this.go_to(Step::Basics, cx),
                        cx,
                    )),
            )
    }

    fn render_onboarding_basics(&self, cx: &mut Context<Self>) -> gpui::Div {
        let prefs = settings(cx).clone();
        let switch = |id: &'static str, on: bool, change: fn(&mut Settings), cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_shrink_0()
                .w(px(36.))
                .h(px(20.))
                .rounded_full()
                .cursor_pointer()
                .bg(if on { hex(Chrome::ACCENT) } else { hex(0x3c3c3c) })
                .flex()
                .items_center()
                .when(on, |d| d.justify_end())
                .px_0p5()
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, change)))
                .child(div().size(px(16.)).rounded_full().bg(hex(Chrome::BRIGHT)))
        };
        let row = |title: &str, body: &str, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .py_1p5()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(title.to_string()))
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(body.to_string())),
                )
                .child(control)
        };
        let opener = |id: &'static str, label: &'static str, value: LinkOpener, cx: &mut Context<Self>| {
            chip(
                id,
                t(cx, label),
                prefs.link_opener == value,
                cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| s.link_opener = value)),
            )
        };
        let openers = div()
            .flex()
            .flex_shrink_0()
            .gap_1()
            .when(crate::platform::HAS_WEBVIEW, |d| d.child(opener("onboarding-link-inapp", "settings.link_inapp", LinkOpener::InApp, cx)))
            .child(opener("onboarding-link-external", "settings.link_external", LinkOpener::External, cx));
        let in_app = prefs.link_opener == LinkOpener::InApp && crate::platform::HAS_WEBVIEW;

        let list = div()
            .id("onboarding-basics")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(row(t(cx, "settings.link_opener"), t(cx, "onboarding.links_body"), openers.into_any_element()))
            .when(in_app, |d| {
                d.child(row(
                    t(cx, "settings.browser_auto_open"),
                    t(cx, "settings.browser_auto_open_hint"),
                    switch(
                        "onboarding-auto-open",
                        prefs.browser.auto_open_servers,
                        |s| s.browser.auto_open_servers = !s.browser.auto_open_servers,
                        cx,
                    )
                    .into_any_element(),
                ))
            })
            .child(row(
                t(cx, "settings.auto_worktree"),
                t(cx, "settings.auto_worktree_hint"),
                switch("onboarding-worktree", prefs.auto_worktree, |s| s.auto_worktree = !s.auto_worktree, cx).into_any_element(),
            ))
            .child(row(
                t(cx, "settings.stop_servers"),
                t(cx, "settings.stop_servers_hint"),
                switch("onboarding-stop-servers", prefs.stop_servers_on_close, |s| s.stop_servers_on_close = !s.stop_servers_on_close, cx)
                    .into_any_element(),
            ))
            .child(row(
                t(cx, "settings.system_notifications"),
                t(cx, "onboarding.notifications_body"),
                switch("onboarding-notifications", prefs.system_notifications, |s| s.system_notifications = !s.system_notifications, cx)
                    .into_any_element(),
            ))
            .child(row(
                t(cx, "settings.idea_mode"),
                t(cx, "settings.idea_mode_hint"),
                switch("onboarding-idea", prefs.idea_mode, |s| s.idea_mode = !s.idea_mode, cx).into_any_element(),
            ));

        div()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .t_heading()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(t(cx, "onboarding.basics")),
                    )
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "onboarding.basics_body"))),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.onboarding_button(
                        "onboarding-back",
                        t(cx, "onboarding.back"),
                        false,
                        |this, _, cx| this.go_to(Step::Welcome, cx),
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.onboarding_button(
                                "onboarding-finish-now",
                                t(cx, "onboarding.finish"),
                                false,
                                |this, _, cx| this.close_onboarding(cx),
                                cx,
                            ))
                            .child(self.onboarding_button(
                                "onboarding-tour",
                                t(cx, "onboarding.to_tour"),
                                true,
                                |this, _, cx| this.go_to(Step::Tour, cx),
                                cx,
                            )),
                    ),
            )
    }

    /// The tour card: bottom left, clear of the browser and the panels docked at the right.
    fn render_onboarding_tour(&self, onboarding: &Onboarding, cx: &mut Context<Self>) -> AnyElement {
        let tasks = Task::all();
        let task = onboarding.current;
        let guide = task.guide();
        let position = tasks.iter().position(|t| *t == task).unwrap_or(0);
        let at = onboarding.progress.get(&task).copied().unwrap_or(0).min(guide.steps.len());
        let task_done = at >= guide.steps.len();
        let all_done = tasks.iter().all(|t| onboarding.finished(*t));
        let next = tasks.iter().copied().skip(position + 1).chain(tasks.iter().copied()).find(|t| *t != task && !onboarding.finished(*t));

        // One dot per feature: green when done, wide for the one on the card; a click jumps there.
        let mut dots = div().flex().items_center().gap_1();
        for (index, other) in tasks.iter().copied().enumerate() {
            dots = dots.child(
                div()
                    .id(("onboarding-dot", index))
                    .h(px(6.))
                    .w(px(if other == task { 18. } else { 6. }))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(hex(if onboarding.finished(other) {
                        Chrome::SUCCESS
                    } else if other == task {
                        Chrome::BRIGHT
                    } else {
                        0x4a4a4a
                    }))
                    .tooltip(crate::ui::Tooltip::text(t(cx, other.guide().title), None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.onboarding_go(other, cx))),
            );
        }

        // The steps: done ones ticked with what happened, the one to do now lit, later ones dimmed.
        let mut steps = div().flex().flex_col().gap_1p5();
        for (index, step) in guide.steps.iter().enumerate() {
            let (is_done, is_now) = (index < at, index == at);
            let number = div()
                .flex_shrink_0()
                .size(px(20.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .t_caption()
                .font_weight(FontWeight::SEMIBOLD)
                .when(is_done, |d| d.bg(hex_alpha(Chrome::SUCCESS, 0.25)).child(icon("check", 12., hex(Chrome::SUCCESS))))
                .when(is_now, |d| d.bg(hex(Chrome::WARNING)).text_color(hex(0x1e1e1e)).child((index + 1).to_string()))
                .when(!is_done && !is_now, |d| d.bg(hex(0x3a3a3a)).text_color(hex(Chrome::MUTED)).child((index + 1).to_string()));
            let keys = step.shortcut.map(|shortcut| {
                div()
                    .flex_shrink_0()
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .bg(hex(0x2d2d30))
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .t_small()
                    .font_family("JetBrains Mono")
                    .text_color(hex(if is_now { Chrome::BRIGHT } else { Chrome::MUTED }))
                    .child(crate::keymap::display(shortcut).into_owned())
            });
            steps = steps.child(
                div()
                    .p_2()
                    .flex()
                    .items_start()
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(if is_now { hex_alpha(Chrome::WARNING, 0.55) } else { hex_alpha(0, 0.) })
                    .when(is_now, |d| d.bg(hex_alpha(Chrome::WARNING, 0.1)))
                    .child(number)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .t_small()
                                    .text_color(hex(if is_now { Chrome::BRIGHT } else { Chrome::MUTED }))
                                    .child(t(cx, step.text)),
                            )
                            // What just happened, once the step is done.
                            .when(is_done, |d| {
                                d.child(div().t_small().text_color(hex(Chrome::SUCCESS)).child(format!("→ {}", t(cx, step.done))))
                            })
                            .when(is_now && step.target.is_some(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .t_caption()
                                        .text_color(hex(Chrome::WARNING))
                                        .child(div().size(px(12.)).rounded_sm().border_2().border_color(hex(Chrome::WARNING)))
                                        .child(t(cx, "onboarding.ring_hint")),
                                )
                            }),
                    )
                    .children(keys),
            );
        }

        let now = guide.steps.get(at);
        let manual = now.is_some_and(|step| matches!(step.check, Check::Manual));
        let can_help = now.is_some_and(|step| manual || step.demo.is_some());
        div()
            .id("onboarding-tour")
            .absolute()
            // Right of the activity bar, and narrow enough to stay clear of the dialogs in the middle
            // of the window (the folder picker a step may open).
            .left(px(56.))
            .bottom(px(32.))
            .w(px(340.))
            .p_3()
            .flex()
            .flex_col()
            .gap_2p5()
            .rounded_xl()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon("graduation-cap", IconSize::BUTTON, hex(Chrome::BLUE)))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "onboarding.tour")))
                    .child(div().flex_1())
                    .child(dots)
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(format!("{}/{}", position + 1, tasks.len())))
                    .child(crate::ui::icon_only(
                        "onboarding-tour-close",
                        "x",
                        cx.listener(|this, _: &ClickEvent, _, cx| this.close_onboarding(cx)),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(crate::brand::tinted_tile(Chrome::BLUE, 30.).child(icon(guide.glyph, IconSize::BUTTON, hex(Chrome::BLUE))))
                    .child(div().t_title().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, guide.title))),
            )
            .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, guide.what)))
            .child(steps)
            .when(task_done, |d| {
                d.child(
                    div()
                        .p_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_lg()
                        .bg(hex_alpha(Chrome::SUCCESS, 0.12))
                        .child(icon("circle-check", IconSize::BUTTON, hex(Chrome::SUCCESS)))
                        .child(
                            div()
                                .flex_1()
                                .t_small()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(t(cx, if all_done { "onboarding.all_done" } else { "onboarding.task_done" })),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(if !task_done && can_help {
                        self.onboarding_button(
                            "onboarding-do-step",
                            t(cx, if manual { "onboarding.got_it" } else { "onboarding.show_me" }),
                            manual,
                            |this, window, cx| this.onboarding_do_step(window, cx),
                            cx,
                        )
                        .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .child(match (next, task_done) {
                        (Some(next), true) => {
                            let label = tf(cx, "onboarding.next", &[("title", t(cx, next.guide().title))]);
                            self.onboarding_button("onboarding-next", &label, true, move |this, _, cx| this.onboarding_go(next, cx), cx)
                                .into_any_element()
                        }
                        (Some(next), false) => self
                            .onboarding_button(
                                "onboarding-skip-task",
                                t(cx, "onboarding.skip_task"),
                                false,
                                move |this, _, cx| this.onboarding_go(next, cx),
                                cx,
                            )
                            .into_any_element(),
                        (None, _) => self
                            .onboarding_button(
                                "onboarding-done",
                                t(cx, if all_done { "onboarding.finish" } else { "onboarding.later" }),
                                all_done,
                                |this, _, cx| this.close_onboarding(cx),
                                cx,
                            )
                            .into_any_element(),
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Check, Task};
    use crate::settings::Language;

    #[test]
    fn every_task_is_a_set_of_steps_that_can_be_followed() {
        let tasks = Task::all();
        let unique: std::collections::HashSet<_> = tasks.iter().collect();
        assert_eq!(unique.len(), tasks.len());
        assert_eq!(tasks[0], Task::Split);
        for task in tasks {
            let guide = task.guide();
            assert!(crate::ui::ICONS.contains(&guide.glyph), "{}", guide.glyph);
            assert!(guide.steps.len() >= 2, "{task:?} is not step by step");
            let mut keys = vec![guide.title, guide.what];
            for step in &guide.steps {
                keys.extend([step.text, step.done]);
                // A step the app cannot do for the user must be one the user can always finish.
                if step.demo.is_none() && !matches!(step.check, Check::Manual) {
                    assert!(step.shortcut.is_some(), "{} has neither a demo nor a shortcut", step.text);
                }
            }
            // Every text of the card exists, in every language (a missing key would show as the key).
            for key in keys {
                for language in [Language::En, Language::Ko, Language::Ja, Language::Zh] {
                    assert_ne!(crate::i18n::tr(language, key), key, "{key} is not translated");
                }
            }
        }
    }
}
