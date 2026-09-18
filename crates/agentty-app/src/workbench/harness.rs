//! Agent harnesses: when a terminal enters a project that ships its own commands, skills or
//! workflow files for Claude Code / Codex, the resume bar offers to start work through it, and a
//! dialog picks the entry point (e.g. `/implement <ticket>`) before any agent is running.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::launch::{LaunchSpec, PaneKind};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{chip, icon, tilde, IconSize, TypeScale};
use agentty_bridge::harness::{Entry, EntryKind, Harness};
use agentty_bridge::model::Agent;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, FontWeight, SharedString, Subscription, Window};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A folder is checked again after this long (files may have been added).
const RECHECK: Duration = Duration::from_secs(60);

pub struct HarnessProbe {
    checked: Instant,
    patterns: Vec<String>,
    harness: Option<Arc<Harness>>,
}

#[derive(Default)]
pub struct HarnessCache {
    probes: HashMap<PathBuf, HarnessProbe>,
    pending: std::collections::HashSet<PathBuf>,
}

pub struct HarnessDialog {
    pane: Pane,
    harness: Arc<Harness>,
    /// Index into `harness.entries`; `None` is a free prompt.
    selected: Option<usize>,
    input: Entity<TextInput>,
    agent: Agent,
    submit: bool,
    error: Option<String>,
    _subscription: Subscription,
}

impl Workbench {
    /// The harness of `dir`, if it was detected already.
    pub(super) fn harness_for(&self, dir: &Path) -> Option<Arc<Harness>> {
        self.harness_cache.probes.get(dir).and_then(|p| p.harness.clone())
    }

    /// Detects the harness of a shell pane's folder in the background (cached per folder).
    pub(super) fn watch_harness(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        let prefs = crate::settings::settings(cx);
        if !prefs.harness_detect {
            return;
        }
        let patterns = prefs.harness_patterns.clone();
        let view = pane.read(cx);
        if view.is_agent() || !view.is_running() {
            return;
        }
        let dir = view.display_cwd();
        if dir == crate::launch::home_dir() || self.harness_cache.pending.contains(&dir) {
            return;
        }
        if let Some(probe) = self.harness_cache.probes.get(&dir) {
            if probe.patterns == patterns && probe.checked.elapsed() < RECHECK {
                return;
            }
        }
        self.harness_cache.pending.insert(dir.clone());
        let task = {
            let (dir, patterns) = (dir.clone(), patterns.clone());
            cx.background_spawn(async move { agentty_bridge::harness::detect(&dir, &patterns) })
        };
        cx.spawn(async move |this, cx| {
            let harness = task.await;
            let _ = this.update(cx, |this, cx| {
                this.harness_cache.pending.remove(&dir);
                let changed = this.harness_cache.probes.get(&dir).map(|p| p.harness.as_deref() != harness.as_ref()).unwrap_or(true);
                this.harness_cache.probes.insert(dir, HarnessProbe { checked: Instant::now(), patterns, harness: harness.map(Arc::new) });
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Forgets detection results, e.g. after the patterns changed.
    pub(super) fn recheck_harnesses(&mut self, cx: &mut Context<Self>) {
        self.harness_cache.probes.clear();
        for pane in self.all_panes() {
            self.watch_harness(&pane, cx);
        }
    }

    pub(super) fn open_harness_dialog(&mut self, pane: Pane, harness: Arc<Harness>, window: &mut Window, cx: &mut Context<Self>) {
        self.resume_menu = None;
        let prefs = crate::settings::settings(cx);
        let agent = prefs.harness_agent.resolve(&harness);
        let submit = prefs.harness_submit;
        let input = cx.new(|cx| TextInput::localized("", "harness.input_placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this: &mut Self, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed => this.start_harness(window, cx),
            TextInputEvent::Cancelled => this.close_harness_dialog(window, cx),
            TextInputEvent::Changed => {
                if let Some(dialog) = this.harness_dialog.as_mut() {
                    dialog.error = None;
                }
                cx.notify();
            }
            _ => {}
        });
        // The first entry that takes an argument is the likely way in; otherwise a free prompt.
        let selected = harness.entries.iter().position(|e| e.kind == EntryKind::Configured || e.hint.is_some());
        let focus = input.clone();
        self.harness_dialog =
            Some(HarnessDialog { pane, harness, selected, input, agent, submit, error: None, _subscription: subscription });
        self.select_harness_entry(selected, cx);
        window.focus(&gpui::Focusable::focus_handle(&focus, cx));
        cx.notify();
    }

    fn select_harness_entry(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        let Some(dialog) = self.harness_dialog.as_mut() else { return };
        dialog.selected = index;
        dialog.error = None;
        if let Some(agent) = index.and_then(|i| dialog.harness.entries.get(i)).and_then(|e| e.agent) {
            dialog.agent = agent;
        }
        cx.notify();
    }

    pub(super) fn debug_select_harness_entry(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.select_harness_entry(index, cx);
    }

    pub(super) fn debug_harness_input(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = self.harness_dialog.as_ref() {
            let text = text.to_string();
            dialog.input.update(cx, |input, cx| input.set_text(text, cx));
        }
    }

    fn close_harness_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.harness_dialog = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn harness_prompt(dialog: &HarnessDialog, cx: &gpui::App) -> String {
        let input = dialog.input.read(cx).text().to_string();
        match dialog.selected.and_then(|i| dialog.harness.entries.get(i)) {
            Some(entry) => entry.prompt(&input),
            None => input.trim().to_string(),
        }
    }

    /// Replaces the shell pane with the agent, started in the harness folder with the prompt.
    fn start_harness(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.harness_dialog.as_ref() else { return };
        let prompt = Self::harness_prompt(dialog, cx);
        if prompt.is_empty() {
            if let Some(dialog) = self.harness_dialog.as_mut() {
                dialog.error = Some(t(cx, "harness.empty").to_string());
            }
            return cx.notify();
        }
        let (pane, agent, submit, root) = (dialog.pane.clone(), dialog.agent, dialog.submit, dialog.harness.root.clone());
        let title = dialog.selected.and_then(|i| dialog.harness.entries.get(i)).map(|e| e.name.clone()).unwrap_or_default();
        let spec = if submit {
            let mut spec = LaunchSpec::with_prompt(agent, prompt.clone(), title.clone(), root);
            if title.is_empty() {
                spec.title = LaunchSpec::new(PaneKind::from(agent), PathBuf::new()).title;
            }
            spec
        } else {
            LaunchSpec::new(PaneKind::from(agent), root)
        };
        self.harness_dialog = None;
        let Some(started) = self.replace_pane(&pane, spec, window, cx) else { return };
        if !submit {
            // Typed once the agent is ready, for the user to read and send.
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(Duration::from_millis(4000)).await;
                let _ = cx.update(|cx| started.update(cx, |view, _| view.insert_text(&prompt)));
            })
            .detach();
        }
    }

    pub(super) fn render_harness_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.harness_dialog.as_ref()?;
        let harness = &dialog.harness;
        let selected_entry: Option<&Entry> = dialog.selected.and_then(|i| harness.entries.get(i));

        let entry_row = |id: SharedString, title: String, code: Option<String>, detail: String, badge: Option<String>, selected: bool| {
            div()
                .id(id)
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap_0p5()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(if selected { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                .bg(if selected { hex_alpha(Chrome::ACCENT, 0.18) } else { hex_alpha(0, 0.) })
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(title))
                        .children(code.map(|c| {
                            div().t_small().font_family(crate::settings::BUNDLED_FONT).text_color(hex(Chrome::BLUE)).truncate().child(c)
                        }))
                        .child(div().flex_1())
                        .children(
                            badge
                                .map(|b| div().px_1p5().rounded_sm().bg(hex(0x2a2a2a)).t_caption().text_color(hex(Chrome::MUTED)).child(b)),
                        ),
                )
                .when(!detail.is_empty(), |d| d.child(div().t_small().text_color(hex(Chrome::MUTED)).truncate().child(detail)))
        };

        let mut entries = div().flex().flex_col().gap_1();
        entries = entries.child(
            entry_row(
                "harness-entry-free".into(),
                t(cx, "harness.free").to_string(),
                None,
                t(cx, "harness.free_hint").to_string(),
                None,
                dialog.selected.is_none(),
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.select_harness_entry(None, cx))),
        );
        for (index, entry) in harness.entries.iter().enumerate() {
            let badge = match entry.kind {
                EntryKind::Configured => t(cx, "harness.kind_configured"),
                EntryKind::Command => t(cx, "harness.kind_command"),
                EntryKind::Skill => t(cx, "harness.kind_skill"),
            };
            let agent = entry.agent.map(|a| if a == Agent::Codex { "Codex" } else { "Claude" });
            let badge = match agent {
                Some(agent) => format!("{agent} · {badge}"),
                None => badge.to_string(),
            };
            let code = match (entry.invocation.contains("{input}"), &entry.hint) {
                (true, _) => None,
                (false, Some(hint)) => Some(format!("{} {hint}", entry.invocation)),
                (false, None) => Some(entry.invocation.clone()),
            };
            entries = entries.child(
                entry_row(
                    SharedString::from(format!("harness-entry-{index}")),
                    entry.name.clone(),
                    code.filter(|c| c.trim() != entry.name),
                    entry.description.clone(),
                    Some(badge),
                    dialog.selected == Some(index),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_harness_entry(Some(index), cx))),
            );
        }

        let counts: Vec<String> = [
            ("harness.commands", harness.commands),
            ("harness.skills", harness.skills),
            ("harness.agents", harness.agents),
            ("harness.mcp", harness.mcp),
            ("harness.hooks", harness.hooks),
        ]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(key, n)| tf(cx, key, &[("n", &n.to_string())]))
        .collect();
        let summary = if counts.is_empty() { harness.matched.join(", ") } else { counts.join(" · ") };

        let mut agents = div().flex().gap_1();
        for (agent, label) in [(Agent::Claude, "Claude Code"), (Agent::Codex, "Codex")] {
            agents = agents.child(chip(
                SharedString::from(format!("harness-agent-{label}")),
                label,
                dialog.agent == agent,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(dialog) = this.harness_dialog.as_mut() {
                        dialog.agent = agent;
                    }
                    cx.notify();
                }),
            ));
        }
        let input_label = match selected_entry.and_then(|e| e.hint.clone()) {
            Some(hint) => tf(cx, "harness.input_for", &[("hint", &hint)]),
            None if selected_entry.is_some() => t(cx, "harness.input_optional").to_string(),
            None => t(cx, "harness.input_prompt").to_string(),
        };
        let prompt = Self::harness_prompt(dialog, cx);
        let submit = dialog.submit;
        let focus = dialog.input.clone();
        let button = |id: &'static str, label: &str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(label.to_string())
        };
        let manage_pane = dialog.pane.clone();

        Some(
            div()
                .id("harness-dialog-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .child(
                    div()
                        .id("harness-dialog")
                        .w(px(640.))
                        .max_h(gpui::relative(0.9))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(icon("workflow", IconSize::BUTTON, hex(Chrome::BRIGHT)))
                                .child(
                                    div()
                                        .t_title()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hex(Chrome::BRIGHT))
                                        .child(t(cx, "harness.title")),
                                )
                                .child(div().flex_1())
                                .child(div().t_small().text_color(hex(Chrome::MUTED)).truncate().child(tilde(&harness.root))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .t_small()
                                .text_color(hex(Chrome::MUTED))
                                .child(div().flex_1().min_w_0().truncate().child(summary))
                                .child(crate::ui::action_button(
                                    "harness-manage",
                                    t(cx, "harness.view_setup"),
                                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                                        this.harness_dialog = None;
                                        this.mark_active(&manage_pane, cx);
                                        this.focus_pane(&manage_pane, window, cx);
                                        this.open_page(super::Page::Extensions, cx);
                                    }),
                                )),
                        )
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "harness.pick")))
                        .child(div().id("harness-entries").max_h(px(260.)).overflow_y_scroll().child(entries))
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(input_label))
                        .child(
                            div()
                                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                    window.focus(&gpui::Focusable::focus_handle(&focus, cx))
                                })
                                .flex()
                                .px_2()
                                .py_1p5()
                                .rounded_md()
                                .border_1()
                                .border_color(hex(Chrome::BORDER))
                                .bg(hex(0x1a1a1a))
                                .t_body()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(dialog.input.clone()),
                        )
                        .when(!prompt.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .t_small()
                                    .child(div().flex_shrink_0().text_color(hex(Chrome::MUTED)).child(t(cx, "harness.will_send")))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(crate::settings::BUNDLED_FONT)
                                            .text_color(hex(Chrome::FOREGROUND))
                                            .child(prompt.clone()),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "prompt.agent")))
                                .child(agents)
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .id("harness-submit")
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .cursor_pointer()
                                        .t_small()
                                        .text_color(hex(Chrome::FOREGROUND))
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Some(dialog) = this.harness_dialog.as_mut() {
                                                dialog.submit = !dialog.submit;
                                            }
                                            cx.notify();
                                        }))
                                        .child(
                                            div()
                                                .size(px(14.))
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(hex(if submit { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                                                .bg(if submit { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .when(submit, |d| d.child(icon("check", 11., hex(Chrome::BRIGHT)))),
                                        )
                                        .child(t(cx, "prompt.submit")),
                                ),
                        )
                        .children(dialog.error.clone().map(|e| div().t_small().text_color(hex(Chrome::ERROR)).child(e)))
                        .child(
                            div()
                                .pt_1()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    button("harness-cancel", t(cx, "confirm.cancel"), false)
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_harness_dialog(window, cx))),
                                )
                                .child(
                                    button("harness-start", t(cx, "harness.start"), true)
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.start_harness(window, cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
