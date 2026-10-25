//! Problems the language servers report: a wavy line under each (red errors, yellow warnings,
//! blue notes), the line number and a tick on the scrollbar in the same color, the message when
//! the pointer rests on one, their count in the status bar, and **F8** / **⇧F8** to step through
//! them (VS Code's keys). The file in front is handed to its server once the typing stops; the
//! server answers in its own time, and what it says is read again whenever it says something.

use super::buffer::Pos;
use super::lsp::{self, Problem, Severity};
use super::{CodeEditor, Content};
use crate::i18n::tf;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, TypeScale};
use gpui::{actions, div, prelude::*, px, AnyElement, ClickEvent, Context, Window};
use std::time::{Duration, Instant};

actions!(code_editor, [NextProblem, PreviousProblem]);

pub fn bind_keys(cx: &mut gpui::App) {
    use crate::key;
    let c = Some(super::CONTEXT);
    cx.bind_keys([key("f8", NextProblem, c), key("shift-f8", PreviousProblem, c)]);
}

/// How often the editor looks for new reports and for a pause in typing.
pub(super) const TICK: Duration = Duration::from_millis(250);
/// A pause this long in typing hands the text to the server.
const QUIET: Duration = Duration::from_millis(400);
/// The message at the end of the line being typed on waits this long after the last key: a
/// message about half-typed code coming and going at every pause is only noise.
const SETTLE: Duration = Duration::from_millis(1500);
/// The pointer rests this long on a problem before its message shows.
const HOVER_DELAY: Duration = Duration::from_millis(350);

/// A file's problems and where its check stands.
#[derive(Default)]
pub(super) struct Checks {
    pub problems: Vec<Problem>,
    /// The text revision handed to the server last.
    checked: Option<u64>,
    /// The revision seen at the last tick, and since when (typing has paused once it holds).
    seen: Option<(u64, Instant)>,
    running: bool,
    /// The message on the cursor's line was held back at the last paint (shown once typing settles).
    pub held: bool,
    /// When the text was last edited.
    pub edited_at: Option<Instant>,
    /// An error in the project's build file (its libraries are unknown here because of it).
    pub build: Option<lsp::BuildError>,
    /// The build error the user closed (shown again once it changes).
    pub build_closed: Option<lsp::BuildError>,
}

pub(super) fn color(severity: Severity) -> u32 {
    match severity {
        Severity::Error => Chrome::ERROR,
        Severity::Warning => Chrome::WARNING,
        _ => Chrome::BLUE,
    }
}

/// Whether `pos` is on the problem (one without width covers the character it starts at).
fn covers(problem: &Problem, pos: Pos) -> bool {
    if problem.start == problem.end {
        return pos.line == problem.start.line && (problem.start.col..=problem.start.col + 1).contains(&pos.col);
    }
    problem.start <= pos && pos <= problem.end
}

impl Checks {
    /// The text changed a moment ago (the message on the line being typed on waits).
    pub(super) fn settling(&self) -> bool {
        self.edited_at.is_some_and(|at| at.elapsed() < SETTLE)
    }

    /// Typing in the file has paused (for as long as the server is left alone).
    pub(super) fn paused(&self, revision: u64) -> bool {
        self.seen.is_some_and(|(seen, since)| seen == revision && since.elapsed() >= QUIET)
    }
}

impl CodeEditor {
    pub(super) fn problems_tick(&mut self, cx: &mut Context<Self>) {
        // Servers turned off or on in Settings: stopped now, or started at the next file.
        let disabled = &crate::settings::settings(cx).lsp_disabled;
        if self.lsp_disabled.as_ref() != Some(disabled) {
            agentty_bridge::lsp::set_disabled(disabled);
            self.lsp_disabled = Some(disabled.clone());
            // A server back on knows nothing of the open files: each is handed over again.
            for doc in &mut self.docs {
                doc.checks.checked = None;
            }
            if let Some(doc) = self.docs.get(self.active) {
                lsp::warm(&doc.project, doc.language, &doc.path);
            }
            cx.notify();
        }
        // Maven projects the user let jdtls read (or no longer): its server starts again that way.
        let trusted = &crate::settings::settings(cx).lsp_trusted_builds;
        if self.lsp_trusted.as_ref() != Some(trusted) {
            let roots: Vec<std::path::PathBuf> = trusted.iter().map(std::path::PathBuf::from).collect();
            agentty_bridge::lsp::set_trusted_builds(&roots);
            self.lsp_trusted = Some(trusted.clone());
            for doc in &mut self.docs {
                doc.checks.checked = None;
            }
            if let Some(doc) = self.docs.get(self.active) {
                lsp::warm(&doc.project, doc.language, &doc.path);
            }
            cx.notify();
        }
        let mut changed = false;
        let generation = lsp::generation();
        if generation != self.problems_generation {
            self.problems_generation = generation;
            let mut worst_changed = false;
            for doc in self.docs.iter_mut().filter(|d| matches!(d.content, Content::Text)) {
                let build = lsp::build_error(&doc.project, doc.language, &doc.path);
                if build != doc.checks.build {
                    doc.checks.build = build;
                    changed = true;
                }
                let found = lsp::problems(&doc.project, doc.language, &doc.path, &doc.buffer);
                if found != doc.checks.problems {
                    let worst = |list: &[Problem]| list.iter().map(|p| p.severity).min();
                    worst_changed |= worst(&found) != worst(&doc.checks.problems);
                    doc.checks.problems = found;
                    changed = true;
                }
            }
            // The tabs show the worst one in their color.
            if worst_changed {
                cx.emit(super::EditorEvent::TabsChanged);
            }
        }
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if lsp::id(doc.language).is_none() || doc.project.as_os_str().is_empty() || !matches!(doc.content, Content::Text) {
            if changed {
                cx.notify();
            }
            return;
        }
        // A file brought back to the front is checked again: its last check may have come while
        // the server was still starting, or other files changed since.
        if self.problems_front.as_deref() != Some(doc.path.as_path()) {
            self.problems_front = Some(doc.path.clone());
            doc.checks.checked = None;
        }
        let revision = doc.buffer.revision();
        match doc.checks.seen {
            Some((seen, _)) if seen == revision => {}
            _ => {
                // Typing: the message under the pointer is about text that moved.
                if doc.checks.seen.is_some() && (self.problem_hover.take().is_some() | self.type_hover.take().is_some()) {
                    changed = true;
                }
                doc.checks.seen = Some((revision, Instant::now()));
            }
        }
        if doc.checks.held && !doc.checks.settling() {
            doc.checks.held = false;
            changed = true;
        }
        let quiet = doc.checks.seen.is_some_and(|(_, since)| since.elapsed() >= QUIET);
        if quiet && !doc.checks.running && doc.checks.checked != Some(revision) {
            doc.checks.running = true;
            doc.checks.checked = Some(revision);
            let (project, language, path, text) = (doc.project.clone(), doc.language, doc.path.clone(), lsp::Snapshot::of(&doc.buffer));
            let checked = path.clone();
            let task = cx.background_spawn(async move { lsp::check(&project, language, &path, &text) });
            cx.spawn(async move |this, cx| {
                task.await;
                let _ = this.update(cx, |this, _| {
                    if let Some(doc) = this.docs.iter_mut().find(|d| d.path == checked) {
                        doc.checks.running = false;
                    }
                });
            })
            .detach();
        }
        if changed {
            cx.notify();
        }
    }

    /// A row above the text while the project's build file has an error: the reason every library
    /// name in the file is unknown, with the way to it.
    pub(super) fn render_build_error(&self, doc: &super::Document, cx: &mut Context<Self>) -> Option<AnyElement> {
        let error = doc.checks.build.clone().filter(|e| doc.checks.build_closed.as_ref() != Some(e))?;
        let file = error.path.file_name()?.to_string_lossy().into_owned();
        let message: String = error.message.trim_start_matches("Project build error: ").chars().take(400).collect();
        let (path, project, line) = (error.path.clone(), doc.project.clone(), error.line + 1);
        let elsewhere = error.path != doc.path;
        let open =
            div()
                .id("editor-build-error-open")
                .flex_shrink_0()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .t_small()
                .cursor_pointer()
                .bg(hex(Chrome::ACCENT))
                .text_color(hex(Chrome::ON_ACCENT))
                .hover(|s| s.opacity(0.85))
                .child(tf(cx, "editor.build_error_open", &[("file", &file)]))
                .on_click(cx.listener(move |this, _: &ClickEvent, window: &mut Window, cx| {
                    this.open_at(&path, &project, Some((line, 1)), window, cx)
                }));
        let close = crate::ui::icon_only_sized(
            "editor-build-error-close",
            "x",
            20.,
            crate::ui::IconSize::INLINE,
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                if let Some(doc) = this.doc() {
                    doc.checks.build_closed = doc.checks.build.clone();
                }
                cx.notify();
            }),
        );
        Some(
            div()
                .flex_shrink_0()
                .px_3()
                .py_1p5()
                .flex()
                .items_start()
                .gap_2()
                .border_b_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex_alpha(Chrome::ERROR, 0.10))
                .child(div().pt(px(2.)).child(icon("circle-x", crate::ui::IconSize::INLINE, hex(Chrome::ERROR))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(tf(cx, "editor.build_error", &[("file", &file)])))
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(message)),
                )
                .when(elsewhere, |d| d.child(open))
                .child(close)
                .into_any_element(),
        )
    }

    /// The pointer over the text (`None`: off it, or ⌘ held for a link): the message of the
    /// problem there shows after a moment.
    pub(super) fn hover_problem(&mut self, pos: Option<Pos>, cx: &mut Context<Self>) {
        let found = self.problem_at(pos);
        if found.as_ref() == self.problem_hover.as_ref().map(|(p, _)| p) {
            return;
        }
        // A message that shows stays a moment: the pointer may be on its way to it.
        if self.problem_hover.as_ref().is_some_and(|(_, shown)| *shown) {
            return self.leave_hovers(cx);
        }
        self.problem_hover = found.clone().map(|p| (p, false));
        self.popup_over = false;
        let Some(problem) = found else { return };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if let Some((_, shown)) = this.problem_hover.as_mut().filter(|(p, _)| *p == problem) {
                    *shown = true;
                    this.find_hover_fixes(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// To the next problem after the cursor (or the previous one before it), round the file, with
    /// its message shown.
    pub(super) fn step_problem(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let problems = &doc.checks.problems;
        if problems.is_empty() {
            return;
        }
        let cursor = doc.buffer.cursor();
        let mut starts: Vec<usize> = (0..problems.len()).collect();
        starts.sort_by_key(|&i| (problems[i].start, problems[i].severity));
        let next = if forward {
            starts.iter().find(|&&i| problems[i].start > cursor).or(starts.first())
        } else {
            starts.iter().rev().find(|&&i| problems[i].start < cursor).or(starts.last())
        };
        let Some(problem) = next.map(|&i| problems[i].clone()) else { return };
        doc.buffer.move_to(problem.start, false);
        doc.center = true;
        self.problem_hover = Some((problem, true));
        self.find_hover_fixes(cx);
        cx.notify();
    }

    /// The fixes of the problem whose message shows, listed under it.
    fn find_hover_fixes(&mut self, cx: &mut Context<Self>) {
        let Some((problem, true)) = self.problem_hover.clone() else { return };
        if self.hover_fixes.as_ref().is_some_and(|(p, _, _)| *p == problem) {
            return;
        }
        self.hover_fixes = None;
        let Some(doc) = self.docs.get(self.active).filter(|d| d.editable()) else { return };
        let (project, language, path, text) = (doc.project.clone(), doc.language, doc.path.clone(), lsp::Snapshot::of(&doc.buffer));
        let since = text.revision;
        let (start, end) = (problem.start, problem.end);
        let (asked, problem_text) = (path.clone(), problem.clone());
        cx.spawn(async move |this, cx| {
            let (found, spelled) = cx
                .background_spawn(async move {
                    let found = lsp::actions(&project, language, &path, &text, start, end);
                    (found, super::spell::suggestions(&problem_text, &text.text, &project, language))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.problem_hover.as_ref().is_some_and(|(p, _)| *p == problem) {
                    // The server's fixes, then the names the editor finds spelled close.
                    let mut fixes: Vec<lsp::Action> = found.unwrap_or_default().into_iter().filter(|a| a.fixes).collect();
                    let own = super::spell::actions(&asked, spelled, &fixes, cx);
                    fixes.extend(own);
                    fixes.truncate(4);
                    this.hover_fixes = Some((problem, since, fixes));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// "Fix with AI": the problem, where it is and what the server said, to the linked session.
    pub(super) fn fix_with_ai(&mut self, problem: &Problem, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let rel = doc.path.strip_prefix(&doc.project).unwrap_or(&doc.path).to_string_lossy().replace('\\', "/");
        let path = doc.path.clone();
        let prompt = super::agent::ask_prompt(&rel, (problem.start.line, problem.end.line), &fix_instruction(problem));
        self.problem_hover = None;
        self.send_instruction(path, (problem.start.line, problem.end.line), prompt, true, cx);
        cx.notify();
    }

    /// "Fix all with AI": the errors of every open file (the file in front first), where each is
    /// and what the server said, to the linked session in one instruction.
    pub(super) fn fix_all_with_ai(&mut self, cx: &mut Context<Self>) {
        let mut order: Vec<usize> = (0..self.docs.len()).collect();
        order.sort_by_key(|&i| i != self.active);
        let mut errors: Vec<(String, &Problem)> = Vec::new();
        for doc in order.iter().map(|&i| &self.docs[i]) {
            let rel = doc.path.strip_prefix(&doc.project).unwrap_or(&doc.path).to_string_lossy().replace('\\', "/");
            errors.extend(doc.checks.problems.iter().filter(|p| p.severity == Severity::Error).map(|p| (rel.clone(), p)));
        }
        errors.truncate(MAX_FIX_ALL);
        let Some(doc) = self.docs.get(self.active).filter(|_| !errors.is_empty()) else { return };
        let (path, line) = (doc.path.clone(), doc.buffer.cursor().line);
        let prompt = fix_all_instruction(&errors);
        self.send_instruction(path, (line, line), prompt, false, cx);
        cx.notify();
    }

    /// Every problem in the open files, the file in front first, as a list to pick from (with the
    /// code beside it).
    pub(super) fn show_problems(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let mut order: Vec<usize> = (0..self.docs.len()).collect();
        order.sort_by_key(|&i| i != self.active);
        let mut rows = Vec::new();
        for doc in order.into_iter().map(|i| &self.docs[i]) {
            let rel = doc.path.strip_prefix(&doc.project).unwrap_or(&doc.path).to_string_lossy().replace('\\', "/");
            let mut problems: Vec<&Problem> = doc.checks.problems.iter().collect();
            problems.sort_by_key(|p| (p.severity, p.start));
            for p in problems {
                let place = format!("{rel}:{}:{}", p.start.line + 1, p.start.col + 1);
                rows.push(super::navigate::Row {
                    tag: Some(match p.severity {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                        _ => "info",
                    }),
                    label: p.message.lines().next().unwrap_or("").to_string().into(),
                    detail: match &p.origin {
                        Some(origin) => format!("{place} · {origin}"),
                        None => place,
                    }
                    .into(),
                    target: super::navigate::Target::At {
                        path: doc.path.clone(),
                        pos: p.start,
                        len: if p.end.line == p.start.line { p.end.col.saturating_sub(p.start.col) } else { 0 },
                    },
                });
            }
        }
        if rows.is_empty() {
            return self.flash(crate::i18n::t(cx, "editor.no_problems").to_string(), cx);
        }
        let title = tf(cx, "editor.problems_title", &[("n", &rows.len().to_string())]);
        self.open_found(title, rows, window, cx);
    }

    /// Errors and warnings in the file in front, in the status bar; a click lists them all.
    pub(super) fn render_problem_counts(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let problems = &self.docs.get(self.active)?.checks.problems;
        let count = |severity: Severity| problems.iter().filter(|p| p.severity == severity).count();
        let (errors, warnings) = (count(Severity::Error), count(Severity::Warning));
        if errors + warnings == 0 {
            return None;
        }
        let part = |glyph: &'static str, n: usize, color: u32| {
            div().flex().items_center().gap_1().child(icon(glyph, 11., hex(color))).child(n.to_string())
        };
        let all_errors: usize = self.docs.iter().map(|d| d.checks.problems.iter().filter(|p| p.severity == Severity::Error).count()).sum();
        let fix_all = (all_errors > 0).then(|| {
            div()
                .id("editor-problems-ai")
                .flex_shrink_0()
                .cursor_pointer()
                .hover(|s| s.opacity(0.7))
                .child(icon("sparkles", 11., hex(Chrome::ACCENT)))
                .tooltip(crate::ui::Tooltip::text(
                    tf(cx, "editor.fix_all_with_ai", &[("n", &all_errors.min(MAX_FIX_ALL).to_string())]),
                    None,
                ))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.fix_all_with_ai(cx)))
        });
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .children(fix_all)
                .child(
                    div()
                        .id("editor-problems")
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                        .when(errors > 0, |d| d.child(part("circle-x", errors, Chrome::ERROR)))
                        .when(warnings > 0, |d| d.child(part("triangle-alert", warnings, Chrome::WARNING)))
                        .tooltip(crate::ui::Tooltip::text(
                            super::with_shortcut(
                                tf(cx, "editor.problems_tip", &[("errors", &errors.to_string()), ("warnings", &warnings.to_string())]),
                                "F8",
                            ),
                            None,
                        ))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.show_problems(window, cx))),
                )
                .into_any_element(),
        )
    }

    pub(super) fn problem_at(&self, pos: Option<Pos>) -> Option<Problem> {
        pos.and_then(|pos| self.docs.get(self.active)?.checks.problems.iter().find(|p| covers(p, pos)).cloned())
    }

    /// The message of the problem under the pointer (or stepped to), under its start.
    pub(super) fn render_problem_hover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.picker.is_some() || self.confirm.is_some() || self.preview.covered || self.link.is_some() || self.completion.is_some() {
            return None;
        }
        let (problem, true) = self.problem_hover.as_ref()? else { return None };
        // Above the line, so the code it is about and what follows stay readable; under it when
        // the line is at the top.
        let layout = self.layout.as_ref()?;
        let top = layout.point_at(problem.start)?;
        let (corner, at) = if f32::from(top.y - layout.corners().0.y) > 72. {
            (gpui::Corner::BottomLeft, gpui::point(top.x, top.y - px(4.)))
        } else {
            (gpui::Corner::TopLeft, layout.point_below(problem.start)?)
        };
        let glyph = match problem.severity {
            Severity::Error => "circle-x",
            Severity::Warning => "triangle-alert",
            _ => "info",
        };
        let (since, fixes) =
            self.hover_fixes.as_ref().filter(|(p, _, _)| p == problem).map(|(_, since, f)| (*since, f.clone())).unwrap_or_default();
        let ask_ai = {
            let problem = problem.clone();
            div()
                .id("editor-problem-ai")
                .flex()
                .items_center()
                .gap_1p5()
                .cursor_pointer()
                .text_color(hex(Chrome::ATTENTION))
                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                .child(icon("sparkles", 11., hex(Chrome::ACCENT)))
                .child(crate::i18n::t(cx, "editor.fix_with_ai"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.fix_with_ai(&problem, cx)))
        };
        let fix_rows = fixes.into_iter().enumerate().map(|(i, action)| {
            let title = action.title.clone();
            div()
                .id(("editor-problem-fix", i))
                .flex()
                .items_center()
                .gap_1p5()
                .cursor_pointer()
                .text_color(hex(Chrome::ATTENTION))
                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                .child(icon("lightbulb", 11., hex(Chrome::WARNING)))
                .child(div().min_w_0().truncate().child(title))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.apply_fix(action.clone(), since, window, cx)))
        });
        let panel = div()
            .id("editor-problem")
            .max_w(px(560.))
            .px_2()
            .py_1p5()
            .rounded_md()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .occlude()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.over_popup(*hovered, cx)))
            .flex()
            .items_start()
            .gap_2()
            .child(div().pt(px(2.)).flex_shrink_0().child(icon(glyph, 12., hex(color(problem.severity)))))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .t_small()
                    .text_color(hex(Chrome::FOREGROUND))
                    .children(problem.message.lines().take(12).map(|line| div().child(line.to_string())))
                    .when_some(problem.origin.clone(), |d, origin| {
                        d.child(
                            div().t_caption().font_family(crate::settings::terminal_font(cx)).text_color(hex(Chrome::MUTED)).child(origin),
                        )
                    })
                    .children(fix_rows)
                    .child(ask_ai),
            );
        Some(
            gpui::deferred(gpui::anchored().anchor(corner).position(at).snap_to_window_with_margin(px(8.)).child(panel))
                .with_priority(2)
                .into_any_element(),
        )
    }

    pub(super) fn debug_problems(&self) -> serde_json::Value {
        let Some(doc) = self.docs.get(self.active) else { return serde_json::Value::Null };
        serde_json::json!({
            "held": doc.checks.held,
            "list": doc.checks.problems.iter().map(|p| serde_json::json!({
                "at": [p.start.line, p.start.col, p.end.line, p.end.col],
                "severity": format!("{:?}", p.severity),
                "message": p.message,
                "origin": p.origin,
            })).collect::<Vec<_>>(),
            "hover": self.problem_hover.as_ref().map(|(p, shown)| serde_json::json!({ "message": p.message, "shown": shown })),
        })
    }
}

/// Errors handed to the agent at once at most ("Fix all with AI").
const MAX_FIX_ALL: usize = 30;

/// A problem's message as one line without control characters: it is typed into the agent's
/// terminal, where a line break would send it early and Ctrl characters would act as keys.
fn one_line(message: &str) -> String {
    let message = message.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    message.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// What the agent is asked to do about the errors listed: (file, problem) (in English, as every
/// prompt is).
fn fix_all_instruction(errors: &[(String, &Problem)]) -> String {
    let mut prompt = String::from("Fix these errors the language server reports:");
    for (rel, problem) in errors {
        let origin = problem.origin.as_ref().map(|o| format!(" ({o})")).unwrap_or_default();
        prompt.push_str(&format!("\n- {rel}:{}:{} {}{origin}", problem.start.line + 1, problem.start.col + 1, one_line(&problem.message)));
    }
    prompt
}

/// What the agent is asked to do about `problem` (in English, as every prompt is).
fn fix_instruction(problem: &Problem) -> String {
    let kind = match problem.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        _ => "problem",
    };
    let message = one_line(&problem.message);
    match &problem.origin {
        Some(origin) => format!("fix this {kind} the language server reports ({origin}): {message}"),
        None => format!("fix this {kind} the language server reports: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_every_error_for_the_agent() {
        let mut first = problem((5, 12), (5, 16));
        first.message = "Cannot find name 'totl'.".into();
        first.origin = Some("ts(2552)".into());
        let mut second = problem((3, 6), (3, 10));
        second.message = "\"totl\" is not defined\n".into();
        let prompt = fix_all_instruction(&[("src/bad.ts".into(), &first), ("py/bad.py".into(), &second)]);
        assert_eq!(
            prompt,
            "Fix these errors the language server reports:\n- src/bad.ts:6:13 Cannot find name 'totl'. (ts(2552))\n- py/bad.py:4:7 \"totl\" is not defined"
        );
    }

    #[test]
    fn asks_the_agent_to_fix_what_the_server_said() {
        let mut problem = problem((3, 6), (3, 10));
        problem.message = "Cannot find name 'totl'.\n  Did you mean 'total'?".into();
        problem.origin = Some("ts(2552)".into());
        assert_eq!(
            fix_instruction(&problem),
            "fix this error the language server reports (ts(2552)): Cannot find name 'totl'. Did you mean 'total'?"
        );
        problem.message = "bad \u{3}\u{1b}[2J string".into();
        assert!(!fix_instruction(&problem).chars().any(char::is_control));
    }

    fn problem(start: (usize, usize), end: (usize, usize)) -> Problem {
        Problem {
            start: Pos::new(start.0, start.1),
            end: Pos::new(end.0, end.1),
            severity: Severity::Error,
            message: "bad".into(),
            origin: None,
        }
    }

    #[test]
    fn a_problem_covers_its_range_and_an_empty_one_its_character() {
        let wide = problem((2, 4), (2, 8));
        assert!(covers(&wide, Pos::new(2, 4)) && covers(&wide, Pos::new(2, 8)));
        assert!(!covers(&wide, Pos::new(2, 9)) && !covers(&wide, Pos::new(3, 5)));
        let empty = problem((1, 3), (1, 3));
        assert!(covers(&empty, Pos::new(1, 4)) && !covers(&empty, Pos::new(1, 5)));
    }
}
