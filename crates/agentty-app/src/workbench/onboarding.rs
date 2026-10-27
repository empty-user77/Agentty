//! First-run onboarding: a few steps on a full card (the agents found, the look, how to work,
//! notifications; see `onboarding_setup`), then a follow-along tour. The tour is a card that stays out of the way
//! and teaches one feature at a time as numbered steps. Only the step to do now is lit: it says
//! where to click (the real button gets a pulsing ring) and which keys do the same, the app notices
//! when it was done, ticks it, says what just happened on screen, and lights the next step. "Do it
//! for me" performs exactly that one step. Nothing moves on to the next feature by itself.
//! It can be reopened from Settings → About and the command palette.

use super::{Page, Workbench};
use crate::i18n::{t, tf};
use crate::launch::{home_dir, LaunchSpec, PaneKind};
use crate::settings::{settings, update_settings};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Window};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Step {
    Appearance,
    Agents,
    Look,
    Work,
    Notify,
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
}

/// Something the user did that a step waits for and that leaves no state behind to look at.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum TourEvent {
    Split,
    PaneClosed,
    MiniEntered,
    MiniLeft,
    FolderOpened,
    /// The two example cards of Session Flow were linked.
    DemoLinked,
    /// A working tree was picked in the files panel (a real one, or one of the tour's examples).
    TreePicked,
}

/// How the tour knows a step was done.
enum Check {
    /// The app is in this state (a page is open, a panel is closed, …).
    State(fn(&Workbench) -> bool),
    Event(TourEvent),
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

impl Task {
    fn all() -> Vec<Task> {
        let mut tasks = vec![Task::Split, Task::Flow];
        if crate::platform::HAS_MINI_MODE {
            tasks.push(Task::Mini);
        }
        if crate::platform::HAS_WEBVIEW {
            tasks.push(Task::Browser);
        }
        tasks.push(Task::Files);
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
                glyph: "orbit",
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
                    // Two example cards are on the page for this step (`flow.rs`), so there is always
                    // something to drag, and nothing real is ever sent.
                    step(
                        "onboarding.flow_2",
                        "onboarding.flow_2_done",
                        None,
                        Some("flow-demo-handle"),
                        Check::Event(TourEvent::DemoLinked),
                        Some(|w, _, cx| w.flow_demo_link(super::flow::DEMO_SOURCE, super::flow::DEMO_TARGET, cx)),
                    ),
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
                    step(
                        "onboarding.browser_2",
                        "onboarding.browser_2_done",
                        Some("⇧⌘B"),
                        Some("header-browser"),
                        Check::State(|w| w.browser.is_none()),
                        Some(|w, window, cx| w.toggle_browser(window, cx)),
                    ),
                ],
            },
            Task::Files => Guide {
                glyph: "list-tree",
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
                    // The project may have no working trees yet: the panel shows example ones for this step.
                    step(
                        "onboarding.files_2",
                        "onboarding.files_2_done",
                        None,
                        Some("files-trees"),
                        Check::Event(TourEvent::TreePicked),
                        Some(|w, _, cx| w.pick_tree_for_tour(cx)),
                    ),
                    step(
                        "onboarding.files_3",
                        "onboarding.files_3_done",
                        None,
                        None,
                        Check::Event(TourEvent::FolderOpened),
                        Some(|w, _, cx| w.open_first_folder(cx)),
                    ),
                    step(
                        "onboarding.files_4",
                        "onboarding.files_4_done",
                        Some("⌥⌘B"),
                        Some("header-files"),
                        Check::State(|w| w.files_panel.is_none()),
                        Some(|w, _, cx| w.toggle_files_panel(cx)),
                    ),
                ],
            },
        }
    }
}

pub(super) struct Onboarding {
    pub(super) step: Step,
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
    /// The tour is for an environment that works: it opens once the system check finds nothing
    /// important missing (right after it runs at start, or after a Recheck), not before.
    pub(super) fn first_run_onboarding(&mut self, cx: &mut Context<Self>) {
        if self.slot == 0 && !settings(cx).onboarding_done && std::env::var("AGENTTY_BACKGROUND").as_deref() != Ok("1") {
            self.onboarding_waits_for_setup = true;
        }
    }

    pub(super) fn open_onboarding(&mut self, cx: &mut Context<Self>) {
        self.page = None;
        self.onboarding =
            Some(Onboarding { step: Step::Appearance, current: Task::all()[0], progress: HashMap::new(), seen: HashSet::new() });
        cx.notify();
    }

    /// Every feature was tried: the tour closes and the start page is what comes next.
    pub(super) fn finish_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_onboarding(cx);
        self.open_welcome(window, cx);
    }

    pub(super) fn close_onboarding(&mut self, cx: &mut Context<Self>) {
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

    /// Whether the tour is on the files panel (its working-tree section then shows example trees).
    pub(super) fn tour_teaches_files(&self) -> bool {
        self.onboarding.as_ref().is_some_and(|o| o.step == Step::Tour && o.current == Task::Files && !o.finished(Task::Files))
    }

    /// Whether the Session Flow page shows its two example cards: only while the tour teaches it.
    pub(super) fn tour_shows_flow_demo(&self) -> bool {
        self.onboarding.as_ref().is_some_and(|o| o.step == Step::Tour && o.current == Task::Flow && !o.finished(Task::Flow))
    }

    /// Element id of the control the step to do now wants pressed, for a ring around it.
    pub(super) fn tour_target(&self) -> Option<&'static str> {
        let onboarding = self.onboarding.as_ref().filter(|o| o.step == Step::Tour)?;
        let at = onboarding.progress.get(&onboarding.current).copied().unwrap_or(0);
        onboarding.current.guide().steps.get(at).and_then(|step| step.target)
    }

    /// "Do this step for me": exactly the step to do now.
    fn onboarding_do_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((task, at)) = self.onboarding.as_ref().map(|o| (o.current, o.progress.get(&o.current).copied().unwrap_or(0))) else {
            return;
        };
        let steps = task.guide().steps;
        let Some(step) = steps.get(at) else { return };
        match step.demo {
            Some(demo) => demo(self, window, cx),
            // Nothing the app can do in the user's place (coming back from mini mode): the button
            // is not offered for such a step; the debug driver just moves on.
            None => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.progress.insert(task, at + 1);
                }
            }
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

    /// Debug driver: `tour` (state), `tour do` (the step to do now, for me), `tour next`, `tour start`,
    /// `tour finish` (what the last button does).
    pub(super) fn debug_tour(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) -> serde_json::Value {
        match argument {
            "start" => {
                self.open_onboarding(cx);
                self.go_to(Step::Tour, cx);
            }
            "do" => self.onboarding_do_step(window, cx),
            "finish" => self.finish_onboarding(window, cx),
            "next" => {
                let next = self.onboarding.as_ref().and_then(|o| Task::all().into_iter().find(|t| !o.finished(*t)));
                if let Some(next) = next {
                    self.onboarding_go(next, cx);
                }
            }
            // A task by name (`tour files`): straight to it, whatever is finished.
            name => {
                if let Some(task) = Task::all().into_iter().find(|t| format!("{t:?}").eq_ignore_ascii_case(name)) {
                    self.open_onboarding(cx);
                    self.go_to(Step::Tour, cx);
                    self.onboarding_go(task, cx);
                }
            }
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
            Step::Tour => self.render_onboarding_tour(onboarding, cx),
            step => self.render_setup(step, cx),
        })
    }

    fn onboarding_button(
        &self,
        id: &'static str,
        label: &str,
        primary: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // At least 30px tall; a label that doesn't fit the tour card wraps and the button grows with it.
        div()
            .id(id)
            .min_w_0()
            .min_h(px(30.))
            .px_4()
            .flex()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .t_body()
            .when(primary, |d| d.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::ON_ACCENT)).hover(|s| s.opacity(0.9)))
            .when(!primary, |d| d.text_color(hex(Chrome::MUTED)).hover(|s| s.text_color(hex(Chrome::BRIGHT)).bg(hex(Chrome::HOVER))))
            .child(label.to_string())
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| on_click(this, window, cx)))
    }

    pub(super) fn go_to(&mut self, step: Step, cx: &mut Context<Self>) {
        if let Some(onboarding) = self.onboarding.as_mut() {
            onboarding.step = step;
        }
        cx.notify();
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
                .font_weight(crate::theme::EMPHASIS)
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
                    .when(crate::theme::is_light(), crate::ui::outlined)
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
        let can_help = now.is_some_and(|step| step.demo.is_some());
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
                    .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, guide.title))),
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
                                // Without this a flex child is as wide as its text and runs out of the card.
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .t_small()
                                .child(
                                    div()
                                        .text_color(hex(Chrome::BRIGHT))
                                        .child(t(cx, if all_done { "onboarding.all_done" } else { "onboarding.task_done" })),
                                )
                                // What "Finish" does, on a line of its own (two short lines read better
                                // than one that breaks in the middle of a word).
                                .when(all_done, |d| d.child(div().text_color(hex(Chrome::MUTED)).child(t(cx, "onboarding.all_done_next")))),
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
                            t(cx, "onboarding.show_me"),
                            false,
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
                                move |this, window, cx| {
                                    if all_done {
                                        this.finish_onboarding(window, cx)
                                    } else {
                                        this.close_onboarding(cx)
                                    }
                                },
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
    use super::Task;
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
                if step.demo.is_none() {
                    assert!(step.shortcut.is_some(), "{} has neither a demo nor a shortcut", step.text);
                }
            }
            // Every text of the card exists, in every language (a missing key would show as the key).
            for key in keys {
                for language in [Language::En, Language::Ko, Language::Ja, Language::Zh, Language::De] {
                    assert_ne!(crate::i18n::tr(language, key), key, "{key} is not translated");
                }
            }
        }
    }
}
