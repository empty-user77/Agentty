//! Problems the language servers report: a wavy line under each (red errors, yellow warnings,
//! blue notes), the line number and a tick on the scrollbar in the same color, the message when
//! the pointer rests on one, their count in the status bar, and **F8** / **⇧F8** to step through
//! them (VS Code's keys). The file in front is handed to its server once the typing stops; the
//! server answers in its own time, and what it says is read again whenever it says something.

use super::buffer::Pos;
use super::lsp::{self, Problem, Severity};
use super::{CodeEditor, Content};
use crate::i18n::tf;
use crate::theme::{hex, Chrome};
use crate::ui::{icon, TypeScale};
use gpui::{actions, div, prelude::*, px, AnyElement, ClickEvent, Context};
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

impl CodeEditor {
    pub(super) fn problems_tick(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let generation = lsp::generation();
        if generation != self.problems_generation {
            self.problems_generation = generation;
            for doc in self.docs.iter_mut().filter(|d| matches!(d.content, Content::Text)) {
                let found = lsp::problems(&doc.project, doc.language, &doc.path, &doc.buffer);
                if found != doc.checks.problems {
                    doc.checks.problems = found;
                    changed = true;
                }
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
        let quiet = doc.checks.seen.is_some_and(|(_, since)| since.elapsed() >= QUIET);
        if quiet && !doc.checks.running && doc.checks.checked != Some(revision) {
            doc.checks.running = true;
            doc.checks.checked = Some(revision);
            let (project, language, path, text) = (doc.project.clone(), doc.language, doc.path.clone(), doc.buffer.text());
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

    /// The pointer over the text (`None`: off it, or ⌘ held for a link): the message of the
    /// problem there shows after a moment.
    pub(super) fn hover_problem(&mut self, pos: Option<Pos>, cx: &mut Context<Self>) {
        let found = pos.and_then(|pos| self.docs.get(self.active)?.checks.problems.iter().find(|p| covers(p, pos)).cloned());
        if found.as_ref() == self.problem_hover.as_ref().map(|(p, _)| p) {
            return;
        }
        let shown = self.problem_hover.as_ref().is_some_and(|(_, shown)| *shown);
        self.problem_hover = found.clone().map(|p| (p, false));
        if shown {
            cx.notify();
        }
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
        if self.hover_fixes.as_ref().is_some_and(|(p, _)| *p == problem) {
            return;
        }
        self.hover_fixes = None;
        let Some(doc) = self.docs.get(self.active).filter(|d| d.editable()) else { return };
        let (project, language, path, text) = (doc.project.clone(), doc.language, doc.path.clone(), doc.buffer.text());
        let (start, end) = (problem.start, problem.end);
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { lsp::actions(&project, language, &path, &text, start, end) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.problem_hover.as_ref().is_some_and(|(p, _)| *p == problem) {
                    let fixes: Vec<lsp::Action> = found.unwrap_or_default().into_iter().filter(|a| a.fixes).take(4).collect();
                    this.hover_fixes = Some((problem, fixes));
                    cx.notify();
                }
            });
        })
        .detach();
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
        Some(
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
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.show_problems(window, cx)))
                .into_any_element(),
        )
    }

    /// The message of the problem under the pointer (or stepped to), under its start.
    pub(super) fn render_problem_hover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.picker.is_some() || self.confirm.is_some() || self.preview.covered || self.link.is_some() {
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
        let fixes = self.hover_fixes.as_ref().filter(|(p, _)| p == problem).map(|(_, f)| f.clone()).unwrap_or_default();
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
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.apply_fix(action.clone(), window, cx)))
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
                    .children(fix_rows),
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

#[cfg(test)]
mod tests {
    use super::*;

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
