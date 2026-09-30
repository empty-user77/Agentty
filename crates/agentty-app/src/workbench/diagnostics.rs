//! Help → Diagnose problems (and the wrench on a pane's bar): runs [`crate::diagnostics`] in the
//! background and lists what it found, each problem Agentty can put right with a Fix button, and
//! "Fix all" for every one of them.

use super::Workbench;
use crate::diagnostics::{Finding, Fix, Level};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, spinner, IconSize, TypeScale};
use gpui::{div, prelude::*, px, ClickEvent, Context, SharedString};

#[derive(Default)]
pub struct Diagnostics {
    /// The last answer; `None` before the first one.
    findings: Option<Vec<Finding>>,
    running: bool,
    /// A fix (or "Fix all") is being carried out.
    fixing: bool,
    /// What the fixes did: the summary of what was put right, and the error when it failed.
    outcomes: Vec<(String, Result<(), String>)>,
}

impl Workbench {
    /// Opens the dialog and runs the checks.
    pub(super) fn open_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.diagnostics = Some(Diagnostics::default());
        self.launcher_open = false;
        self.notices_open = false;
        self.run_diagnostics(cx);
    }

    fn run_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.diagnostics.as_mut() else { return };
        if state.running {
            return;
        }
        state.running = true;
        cx.notify();
        let settings = crate::settings::settings(cx);
        let context = crate::diagnostics::Context { aliases: settings.aliases.clone(), bypass: settings.always_bypass };
        let task = cx.background_spawn(async move { crate::diagnostics::run(&context) });
        cx.spawn(async move |this, cx| {
            let findings = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(state) = this.diagnostics.as_mut() {
                    state.findings = Some(findings);
                    state.running = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Carries out the fixes of `findings` one after another, then checks again.
    fn fix_findings(&mut self, findings: Vec<Finding>, cx: &mut Context<Self>) {
        let Some(state) = self.diagnostics.as_mut() else { return };
        if state.fixing || findings.is_empty() {
            return;
        }
        state.fixing = true;
        cx.notify();
        let jobs: Vec<(String, Fix)> = findings.into_iter().filter_map(|f| Some((title(&f, cx), f.fix?))).collect();
        let task = cx.background_spawn(async move {
            jobs.into_iter().map(|(title, fix)| (title, crate::diagnostics::apply(&fix).map_err(|e| format!("{e:#}")))).collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let outcomes = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(state) = this.diagnostics.as_mut() else { return };
                state.fixing = false;
                state.outcomes.extend(outcomes);
                this.run_diagnostics(cx);
            });
        })
        .detach();
    }

    fn close_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.diagnostics = None;
        cx.notify();
    }

    /// Debug driver: `diagnose` opens the dialog, `diagnose fix:<n>` / `fix-all` press its buttons,
    /// `diagnose dump:<file>` writes what it shows as JSON lines, `diagnose close` closes it.
    pub(super) fn debug_diagnose(&mut self, argument: &str, cx: &mut Context<Self>) {
        match argument.split_once(':').unwrap_or((argument, "")) {
            ("", _) => self.open_diagnostics(cx),
            ("close", _) => self.close_diagnostics(cx),
            ("fix-all", _) => {
                let fixable = self
                    .diagnostics
                    .as_ref()
                    .and_then(|s| s.findings.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| f.fix.is_some())
                    .collect();
                self.fix_findings(fixable, cx);
            }
            ("fix", index) => {
                let finding = self.diagnostics.as_ref().and_then(|s| s.findings.as_ref()?.get(index.parse::<usize>().ok()?).cloned());
                if let Some(finding) = finding {
                    self.fix_findings(vec![finding], cx);
                }
            }
            ("dump", path) => {
                let Some(state) = self.diagnostics.as_ref() else { return };
                let mut out = format!("{{\"running\":{},\"fixing\":{}}}\n", state.running, state.fixing);
                for finding in state.findings.iter().flatten() {
                    let line = serde_json::json!({
                        "level": format!("{:?}", finding.level),
                        "title": title(finding, cx),
                        "lines": finding.lines,
                        "fix": finding.fix.as_ref().map(|fix| format!("{fix:?}")),
                    });
                    out.push_str(&format!("{line}\n"));
                }
                for (what, outcome) in &state.outcomes {
                    out.push_str(&format!("{}\n", serde_json::json!({ "outcome": what, "error": outcome.as_ref().err() })));
                }
                let _ = std::fs::write(path, out);
            }
            _ => {}
        }
    }

    pub(super) fn render_diagnostics(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let state = self.diagnostics.as_ref()?;
        let findings = state.findings.as_deref().unwrap_or_default();
        let fixable: Vec<Finding> = findings.iter().filter(|f| f.fix.is_some()).cloned().collect();
        let busy = state.running || state.fixing;

        let mut list = div().id("diagnostics-list").flex().flex_col().gap_2().max_h(px(460.)).overflow_y_scroll();
        if state.findings.is_none() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_4()
                    .t_body()
                    .text_color(hex(Chrome::MUTED))
                    .child(spinner(IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(t(cx, "diag.running")),
            );
        }
        for (index, finding) in findings.iter().enumerate() {
            list = list.child(self.render_finding(index, finding, busy, cx));
        }
        for (summary, outcome) in &state.outcomes {
            let (name, color, text) = match outcome {
                Ok(()) => ("circle-check", Chrome::SUCCESS, tf(cx, "diag.fixed", &[("what", summary)])),
                Err(error) => ("circle-x", Chrome::ERROR, tf(cx, "diag.fix_failed", &[("what", summary), ("error", error)])),
            };
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .t_small()
                    .text_color(hex(Chrome::FOREGROUND))
                    .child(icon(name, IconSize::INLINE, hex(color)))
                    .child(text),
            );
        }

        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .when(busy, |d| d.opacity(0.5))
                .when(!busy, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                .child(t(cx, label))
        };
        let mut buttons = div().flex().flex_wrap().items_center().justify_end().gap_2();
        if busy && state.findings.is_some() {
            buttons = buttons.child(div().mr_auto().child(spinner(IconSize::INLINE, hex(Chrome::MUTED))));
        }
        buttons = buttons.child(
            button("diagnostics-rerun", "diag.rerun", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.run_diagnostics(cx))),
        );
        if fixable.len() > 1 {
            buttons = buttons.child(
                button("diagnostics-fix-all", "diag.fix_all", true)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.fix_findings(fixable.clone(), cx))),
            );
        }
        buttons = buttons.child(
            button("diagnostics-close", "diag.close", false)
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_diagnostics(cx))),
        );

        Some(
            div()
                .id("diagnostics-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_diagnostics(cx)))
                .child(
                    div()
                        .id("diagnostics-dialog")
                        .w(px(600.))
                        .max_w(gpui::relative(0.92))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "diag.title")),
                        )
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "diag.intro")))
                        .child(list)
                        .child(buttons),
                ),
        )
    }

    fn render_finding(&self, index: usize, finding: &Finding, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let (name, color) = match finding.level {
            Level::Ok => ("circle-check", Chrome::SUCCESS),
            Level::Info => ("info", Chrome::BLUE),
            Level::Warn => ("shield-alert", Chrome::WARNING),
            Level::Problem => ("circle-x", Chrome::ERROR),
        };
        let args: Vec<(&str, &str)> = finding.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let mut text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(SharedString::from(tf(cx, finding.title, &args))));
        if let Some(detail) = finding.detail {
            text = text.child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(SharedString::from(tf(cx, detail, &args))));
        }
        for line in &finding.lines {
            text = text.child(
                div().t_caption().font_family("JetBrains Mono").text_color(hex(Chrome::MUTED)).child(SharedString::from(line.clone())),
            );
        }
        let mut row = div()
            .flex()
            .items_start()
            .gap_2()
            .p_2()
            .rounded_md()
            .bg(hex(Chrome::PANEL))
            .child(div().pt(px(2.)).child(icon(name, IconSize::INLINE, hex(color))))
            .child(text);
        if finding.fix.is_some() {
            let one = finding.clone();
            row = row.child(
                div()
                    .id(("diagnostics-fix", index))
                    .flex_none()
                    .px_2p5()
                    .py_1()
                    .rounded_md()
                    .t_small()
                    .bg(hex(Chrome::ACCENT))
                    .text_color(hex(Chrome::BRIGHT))
                    .when(busy, |d| d.opacity(0.5))
                    .when(!busy, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                    .child(t(cx, "diag.fix"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.fix_findings(vec![one.clone()], cx))),
            );
        }
        row
    }
}

/// The finding's summary, in the user's language.
fn title(finding: &Finding, cx: &gpui::App) -> String {
    let args: Vec<(&str, &str)> = finding.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    tf(cx, finding.title, &args)
}
