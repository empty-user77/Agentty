//! Reviewing what agents changed: every change an agent made to an open file stays marked in the
//! gutter (in the agent's color) until it is kept or undone, the file's tab carries a dot, and a
//! bar at the bottom of the editor steps through the changes — *Undo* puts the text back as it
//! was before the agent touched it (and saves, so the agent sees it too), *Keep* drops the mark.
//!
//! A change is a hunk: the lines it replaced and the lines it wrote. Later edits by agents that
//! overlap an earlier change are folded into it, so undoing goes back to the text before both.

use super::agent::LinkedAgent;
use super::buffer::{Buffer, Pos};
use super::CodeEditor;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use gpui::{actions, div, prelude::*, px, AnyElement, ClickEvent, Context};

actions!(code_editor, [NextChange, PreviousChange, ToggleChangeDiff]);

pub fn bind_keys(cx: &mut gpui::App) {
    use crate::key;
    let c = Some(super::CONTEXT);
    // VS Code's keys for the next and previous change.
    cx.bind_keys([key("alt-f5", NextChange, c), key("shift-alt-f5", PreviousChange, c)]);
}

/// Lines of a change shown at most in its before/after view.
const DIFF_LINES: usize = 14;
use std::path::{Path, PathBuf};

/// Lines replaced: `old` (as they were) at `start` became `new`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub start: usize,
    pub old: Vec<String>,
    pub new: Vec<String>,
}

/// The one hunk between two texts (the lines from the first difference to the last); `None` when
/// they are the same. Lines are split as the buffer splits them (a final line break leaves an
/// empty last line).
pub fn hunk(old: &str, new: &str) -> Option<Hunk> {
    if old == new {
        return None;
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (old.split('\n').collect(), new.split('\n').collect());
    let first = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[first..].iter().rev().zip(b[first..].iter().rev()).take_while(|(x, y)| x == y).count();
    Some(Hunk {
        start: first,
        old: a[first..a.len() - tail].iter().map(|s| s.to_string()).collect(),
        new: b[first..b.len() - tail].iter().map(|s| s.to_string()).collect(),
    })
}

/// Most lines compared to split a hunk into the separate changes inside it (the old lines times
/// the new); past this it stays one change.
const SPLIT_LIMIT: usize = 1_000_000;

/// The separate changes inside one hunk: lines both sides share (the longest common run of them)
/// are left out, so two edits far apart in one write are two changes, not one with everything
/// between them.
pub fn split(hunk: &Hunk) -> Vec<Hunk> {
    let (a, b) = (&hunk.old, &hunk.new);
    if a.is_empty() || b.is_empty() || a.len() * b.len() > SPLIT_LIMIT {
        return vec![hunk.clone()];
    }
    // Longest common subsequence of lines, from the end.
    let (n, m) = (a.len(), b.len());
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if a[i] == b[j] { table[i + 1][j + 1] + 1 } else { table[i + 1][j].max(table[i][j + 1]) };
        }
    }
    let mut parts = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut open: Option<(usize, usize, usize)> = None; // (start in new, i, j) where a part began
    let close = |open: &mut Option<(usize, usize, usize)>, i: usize, j: usize, parts: &mut Vec<Hunk>| {
        if let Some((start, oi, oj)) = open.take() {
            parts.push(Hunk { start, old: a[oi..i].to_vec(), new: b[oj..j].to_vec() });
        }
    };
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            close(&mut open, i, j, &mut parts);
            i += 1;
            j += 1;
        } else {
            open.get_or_insert((hunk.start + j, i, j));
            if j < m && (i == n || table[i][j + 1] >= table[i + 1][j]) {
                j += 1;
            } else {
                i += 1;
            }
        }
    }
    close(&mut open, i, j, &mut parts);
    // Starts are in the new text; as events applied one after another they are in the text
    // before each, which is the same line once the earlier parts are applied.
    parts
}

/// The changes from `base` to `text`, each as (first line in `base`, lines it replaces there, the
/// lines written instead).
fn changes_from(base: &str, text: &str) -> Vec<(usize, usize, Vec<String>)> {
    let Some(whole) = hunk(base, text) else { return Vec::new() };
    let mut grown = 0isize;
    split(&whole)
        .into_iter()
        .map(|part| {
            // A part's start counts the lines of the parts above it as written: back to `base`'s.
            let start = (part.start as isize - grown) as usize;
            grown += part.new.len() as isize - part.old.len() as isize;
            (start, part.old.len(), part.new)
        })
        .collect()
}

/// `mine` and `theirs` both changed `base` (the user's unsaved edits, an agent's write to disk):
/// both sets of changes in one text, when they touch different lines. `None` when a change of one
/// touches a change of the other: then the user decides.
pub fn merge(base: &str, mine: &str, theirs: &str) -> Option<String> {
    let (a, b) = (changes_from(base, mine), changes_from(base, theirs));
    // Touching counts as overlapping: an insertion right at the other's edit is ambiguous.
    let clash = a.iter().any(|(s1, l1, _)| b.iter().any(|(s2, l2, _)| *s1 <= s2 + l2 && *s2 <= s1 + l1));
    if clash {
        return None;
    }
    let mut lines: Vec<String> = base.split('\n').map(str::to_string).collect();
    let mut all: Vec<&(usize, usize, Vec<String>)> = a.iter().chain(b.iter()).collect();
    all.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    for (start, len, new) in all {
        lines.splice(*start..start + len, new.iter().cloned());
    }
    Some(lines.join("\n"))
}

/// Folds `event` (a change from `before` to the text after it) into the earlier changes it
/// overlaps; returns the change to record and the earlier ones it replaces (their indexes in
/// `earlier`). Earlier changes are in `before`'s lines and never overlap each other.
pub fn fold(before: &[&str], after: &[&str], event: &Hunk, earlier: &[Hunk]) -> (Hunk, Vec<usize>) {
    let event_end = event.start + event.old.len();
    // Touching counts: two edits on neighbouring lines read as one change.
    let overlapping: Vec<usize> =
        earlier.iter().enumerate().filter(|(_, e)| e.start <= event_end && event.start <= e.start + e.new.len()).map(|(i, _)| i).collect();
    if overlapping.is_empty() {
        return (event.clone(), overlapping);
    }
    let from = overlapping.iter().map(|&i| earlier[i].start).min().unwrap_or(event.start).min(event.start);
    let to = overlapping.iter().map(|&i| earlier[i].start + earlier[i].new.len()).max().unwrap_or(event_end).max(event_end);
    // The original text of the whole span: `before`, with each earlier change put back.
    let mut old: Vec<String> = Vec::new();
    let mut line = from;
    let mut by_start: Vec<&Hunk> = overlapping.iter().map(|&i| &earlier[i]).collect();
    by_start.sort_by_key(|e| e.start);
    for e in by_start {
        old.extend(before[line..e.start].iter().map(|s| s.to_string()));
        old.extend(e.old.iter().cloned());
        line = e.start + e.new.len();
    }
    old.extend(before[line.min(to)..to].iter().map(|s| s.to_string()));
    let grown = event.new.len() as isize - event.old.len() as isize;
    let new_to = (to as isize + grown).max(from as isize) as usize;
    let new = after[from..new_to.min(after.len())].iter().map(|s| s.to_string()).collect();
    (Hunk { start: from, old, new }, overlapping)
}

/// A change an agent made, waiting to be kept or undone.
#[derive(Clone, Debug)]
pub struct AgentChange {
    pub id: u64,
    pub pane_id: u64,
    pub label: String,
    pub color: u32,
    pub path: PathBuf,
    pub hunk: Hunk,
}

/// Where a change's lines are in the buffer now: where it was made if they are still there, else
/// where those lines are found (the user may have edited above them since).
pub fn locate(buffer: &Buffer, hunk: &Hunk) -> Option<usize> {
    let count = buffer.line_count();
    let matches_at =
        |start: usize| start + hunk.new.len() <= count && hunk.new.iter().enumerate().all(|(i, l)| buffer.line(start + i) == l);
    if hunk.new.is_empty() {
        return (hunk.start <= count).then_some(hunk.start.min(count.saturating_sub(1)));
    }
    if matches_at(hunk.start) {
        return Some(hunk.start);
    }
    (0..count).find(|&start| matches_at(start))
}

impl CodeEditor {
    /// An agent's edit was read from disk (`before` → the buffer's text now): it is recorded for
    /// review, folded into the changes it touches.
    pub(super) fn record_change(&mut self, path: &Path, before: &str, agent: Option<&LinkedAgent>) {
        let Some(doc) = self.docs.iter().find(|d| d.path == path) else { return };
        let after = doc.buffer.text();
        let Some(whole) = hunk(before, &after) else { return };
        // Each separate change of the write, top to bottom, as if made one after another.
        let mut text: Vec<String> = before.split('\n').map(str::to_string).collect();
        for part in split(&whole) {
            let mut next = text.clone();
            next.splice(part.start..part.start + part.old.len(), part.new.iter().cloned());
            let (from, to): (Vec<&str>, Vec<&str>) = (text.iter().map(String::as_str).collect(), next.iter().map(String::as_str).collect());
            self.record_event(path, &from, &to, &part, agent);
            text = next;
        }
    }

    fn record_event(&mut self, path: &Path, before_lines: &[&str], after_lines: &[&str], event: &Hunk, agent: Option<&LinkedAgent>) {
        let event = event.clone();
        let mine: Vec<usize> = (0..self.changes.len()).filter(|&i| self.changes[i].path == path).collect();
        let earlier: Vec<Hunk> = mine.iter().map(|&i| self.changes[i].hunk.clone()).collect();
        let (folded, replaced) = fold(before_lines, after_lines, &event, &earlier);
        let grown = event.new.len() as isize - event.old.len() as isize;
        let event_end = event.start + event.old.len();
        let remove: Vec<usize> = replaced.iter().map(|&k| mine[k]).collect();
        // Changes further down move with the lines the event added or took away.
        for &i in &mine {
            if !remove.contains(&i) && self.changes[i].hunk.start >= event_end {
                self.changes[i].hunk.start = (self.changes[i].hunk.start as isize + grown).max(0) as usize;
            }
        }
        let mut index = 0;
        self.changes.retain(|_| {
            let keep = !remove.contains(&index);
            index += 1;
            keep
        });
        // An edit nobody claimed (git, a formatter, another program) is not an agent's to review.
        let Some(agent) = agent else { return };
        if folded.old == folded.new {
            return; // changed and changed back
        }
        self.next_change += 1;
        self.changes.push(AgentChange {
            id: self.next_change,
            pane_id: agent.pane_id,
            label: agent.label.clone(),
            color: agent.color,
            path: path.to_path_buf(),
            hunk: folded,
        });
    }

    /// Changes to review in the file in front, with where each is now, top to bottom.
    pub(super) fn changes_here(&self) -> Vec<(&AgentChange, usize)> {
        let Some(doc) = self.docs.get(self.active) else { return Vec::new() };
        let mut found: Vec<(&AgentChange, usize)> =
            self.changes.iter().filter(|c| c.path == doc.path).filter_map(|c| locate(&doc.buffer, &c.hunk).map(|at| (c, at))).collect();
        found.sort_by_key(|(_, at)| *at);
        found
    }

    /// The agent color of changes to review in `path`, for its tab.
    pub fn review_color(&self, path: &Path) -> Option<u32> {
        self.changes.iter().rev().find(|c| c.path == path).map(|c| c.color)
    }

    /// The change the cursor is in or the next one below it (the first again past the last).
    fn current_change(&self) -> Option<(u64, usize, usize)> {
        let doc = self.docs.get(self.active)?;
        let here = self.changes_here();
        let line = doc.buffer.cursor().line;
        let index = here.iter().position(|(c, at)| line < at + c.hunk.new.len().max(1)).unwrap_or(0);
        here.get(index).map(|(c, at)| (c.id, *at, index))
    }

    pub fn review_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let here: Vec<usize> = self.changes_here().iter().map(|(_, at)| *at).collect();
        let Some(doc) = self.docs.get(self.active) else { return };
        if here.is_empty() {
            return;
        }
        let line = doc.buffer.cursor().line;
        let target = if forward {
            here.iter().copied().find(|&at| at > line).unwrap_or(here[0])
        } else {
            here.iter().rev().copied().find(|&at| at < line).unwrap_or(here[here.len() - 1])
        };
        self.reveal(Pos::new(target, 0), cx);
    }

    fn reveal(&mut self, pos: Pos, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let pos = doc.buffer.clamp(pos);
        doc.buffer.move_to(pos, false);
        doc.center = true;
        doc.autoscroll = true;
        cx.notify();
    }

    pub fn keep_change(&mut self, id: u64, cx: &mut Context<Self>) {
        self.changes.retain(|c| c.id != id);
        cx.notify();
    }

    /// Keeps every change in the file in front.
    pub fn keep_all_changes(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.docs.get(self.active).map(|d| d.path.clone()) else { return };
        self.changes.retain(|c| c.path != path);
        cx.notify();
    }

    /// Puts the lines back as they were before the agent changed them, as one step that ⌘Z takes
    /// back, and saves the file so the agent works on from there.
    pub fn undo_change(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(change) = self.changes.iter().find(|c| c.id == id).cloned() else { return };
        let Some(index) = self.docs.iter().position(|d| d.path == change.path) else { return };
        let doc = &mut self.docs[index];
        if !doc.editable() {
            return;
        }
        let Some(at) = locate(&doc.buffer, &change.hunk) else {
            self.changes.retain(|c| c.id != id);
            return self.flash(t(cx, "review.moved").to_string(), cx);
        };
        let mut lines: Vec<String> = (0..doc.buffer.line_count()).map(|i| doc.buffer.line(i).to_string()).collect();
        let end = (at + change.hunk.new.len()).min(lines.len());
        lines.splice(at..end, change.hunk.old.iter().cloned());
        doc.buffer.set_text(&lines.join("\n"));
        doc.buffer.move_to(Pos::new(at, 0), false);
        doc.sync_highlights();
        doc.autoscroll = true;
        let grown = change.hunk.old.len() as isize - change.hunk.new.len() as isize;
        let path = change.path.clone();
        self.changes.retain(|c| c.id != id);
        for other in self.changes.iter_mut().filter(|c| c.path == path && c.hunk.start > at) {
            other.hunk.start = (other.hunk.start as isize + grown).max(0) as usize;
        }
        if self.active == index {
            self.save(false, cx);
        }
        self.tabs_changed(cx);
        cx.notify();
    }

    /// The bar at the bottom of the editor while the file in front has changes to review.
    pub(super) fn render_review_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.picker.is_some() || self.confirm.is_some() {
            return None;
        }
        let here = self.changes_here();
        let (id, _, index) = self.current_change()?;
        let change = here.iter().find(|(c, _)| c.id == id).map(|(c, _)| *c)?;
        let (count, color, label) = (here.len(), change.color, change.label.clone());
        let lines = (change.hunk.new.len() as isize - change.hunk.old.len() as isize, change.hunk.new.len().max(change.hunk.old.len()));
        let delta = match lines.0 {
            d if d > 0 => format!("+{d}"),
            d if d < 0 => format!("{d}"),
            _ => format!("~{}", lines.1),
        };
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(label)
        };
        Some(
            div()
                .absolute()
                .bottom(px(38.))
                .left_0()
                .right(px(self.beside_width()))
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("editor-review")
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl_2()
                        .pr_1()
                        .py_1()
                        .rounded_lg()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex_alpha(color, 0.6))
                        .shadow_lg()
                        .occlude()
                        .child(div().size(px(8.)).rounded_full().bg(hex(color)))
                        .child(div().t_small().text_color(hex(Chrome::BRIGHT)).child(label))
                        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(
                            tf(cx, "review.position", &[("i", &(index + 1).to_string()), ("n", &count.to_string())]) + " · " + &delta,
                        ))
                        .child(
                            crate::ui::icon_only(
                                "editor-review-prev",
                                "chevron-up",
                                cx.listener(|this, _: &ClickEvent, _, cx| this.review_step(false, cx)),
                            )
                            .tooltip(crate::ui::Tooltip::text(t(cx, "review.previous"), None)),
                        )
                        .child(
                            crate::ui::icon_only(
                                "editor-review-next",
                                "chevron-down",
                                cx.listener(|this, _: &ClickEvent, _, cx| this.review_step(true, cx)),
                            )
                            .tooltip(crate::ui::Tooltip::text(t(cx, "review.next"), None)),
                        )
                        .child(div().w(px(1.)).h(px(16.)).bg(hex(Chrome::BORDER)))
                        .child(
                            button("editor-review-diff", t(cx, "review.compare").to_string())
                                .when(self.review_diff, |d| d.bg(hex(Chrome::SELECTED)))
                                .tooltip(crate::ui::Tooltip::text(t(cx, "review.compare_tip"), None))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.review_diff = !this.review_diff;
                                    cx.notify();
                                })),
                        )
                        .child(
                            button("editor-review-undo", t(cx, "review.undo").to_string())
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.undo_change(id, cx))),
                        )
                        .child(
                            button("editor-review-keep", t(cx, "review.keep").to_string())
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.keep_change(id, cx))),
                        )
                        .when(count > 1, |d| {
                            d.child(
                                button("editor-review-keep-all", t(cx, "review.keep_all").to_string())
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.keep_all_changes(cx))),
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    /// The change the cursor is in, as it was before and as it is now, under its last line
    /// (while *Compare* is on).
    pub(super) fn render_change_diff(&self, cx: &gpui::App) -> Option<AnyElement> {
        if !self.review_diff || self.picker.is_some() || self.preview.covered {
            return None;
        }
        let (id, at, _) = self.current_change()?;
        let change = self.changes.iter().find(|c| c.id == id)?;
        let layout = self.layout.as_ref()?;
        let below = layout.point_below(Pos::new(at + change.hunk.new.len().max(1) - 1, 0))?;
        let row = |sign: &'static str, text: &str, color: u32| {
            div()
                .flex()
                .gap_2()
                .px_2()
                .bg(hex_alpha(color, 0.14))
                .child(div().w(px(10.)).flex_shrink_0().text_color(hex(color)).child(sign))
                .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(text.to_string()))
        };
        let more = |count: usize| {
            (count > DIFF_LINES).then(|| div().px_2().text_color(hex(Chrome::MUTED)).child(format!("… +{}", count - DIFF_LINES)))
        };
        let panel = div()
            .id("editor-change-diff")
            .w(px(620.))
            .max_w(px(900.))
            .py_1()
            .rounded_md()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex_alpha(change.color, 0.6))
            .shadow_lg()
            .occlude()
            .font_family(crate::settings::terminal_font(cx))
            .text_size(px(12.))
            .children(change.hunk.old.iter().take(DIFF_LINES).map(|l| row("−", l, Chrome::ERROR)))
            .children(more(change.hunk.old.len()))
            .children(change.hunk.new.iter().take(DIFF_LINES).map(|l| row("+", l, Chrome::SUCCESS)))
            .children(more(change.hunk.new.len()));
        Some(
            gpui::deferred(gpui::anchored().position(below).snap_to_window_with_margin(self.popup_margin()).child(panel))
                .with_priority(2)
                .into_any_element(),
        )
    }

    /// Every change waiting for review, in every open file, as a list to pick from (with the code
    /// beside it): what the agents did, in one place.
    pub fn show_all_changes(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let rows: Vec<super::navigate::Row> = self
            .changes
            .iter()
            .filter_map(|c| {
                let doc = self.docs.iter().find(|d| d.path == c.path)?;
                let at = locate(&doc.buffer, &c.hunk)?;
                let (added, removed) = (c.hunk.new.len(), c.hunk.old.len());
                let first = c.hunk.new.iter().chain(c.hunk.old.iter()).map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("").to_string();
                let rel = c.path.strip_prefix(&doc.project).unwrap_or(&c.path).to_string_lossy().replace('\\', "/");
                Some(super::navigate::Row {
                    tag: None,
                    label: format!("+{added} −{removed}  {first}").into(),
                    detail: format!("{} · {rel}:{}", c.label, at + 1).into(),
                    target: super::navigate::Target::At { path: c.path.clone(), pos: Pos::new(at, 0), len: 0 },
                })
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        let title = tf(cx, "review.all", &[("n", &rows.len().to_string())]);
        self.open_found(title, rows, window, cx);
    }

    /// The toolbar's count of changes waiting for review (all files), a click lists them.
    pub(super) fn render_changes_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = self.changes.len();
        let color = self.changes.last()?.color;
        Some(
            div()
                .id("editor-all-changes")
                .flex_shrink_0()
                .h(px(22.))
                .px_1p5()
                .flex()
                .items_center()
                .gap_1()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(hex_alpha(color, 0.5))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .t_caption()
                .text_color(hex(Chrome::BRIGHT))
                .child(div().w(px(3.)).h(px(12.)).rounded_sm().bg(hex(color)))
                .child(tf(cx, "review.count", &[("n", &count.to_string())]))
                .tooltip(crate::ui::Tooltip::text(t(cx, "review.all_tip"), None))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.show_all_changes(window, cx)))
                .into_any_element(),
        )
    }

    /// Debug driver: the changes to review in the file in front.
    pub(super) fn debug_changes(&self) -> serde_json::Value {
        serde_json::json!(self
            .changes_here()
            .iter()
            .map(|(c, at)| serde_json::json!({ "id": c.id, "pane": c.pane_id, "label": c.label, "at": at, "old": c.hunk.old.len(), "new": c.hunk.new.len() }))
            .collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(start: usize, old: &[&str], new: &[&str]) -> Hunk {
        Hunk { start, old: old.iter().map(|s| s.to_string()).collect(), new: new.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn one_hunk_between_two_texts() {
        assert_eq!(hunk("a\nb\nc", "a\nb\nc"), None);
        assert_eq!(hunk("a\nb\nc", "a\nB\nc"), Some(h(1, &["b"], &["B"])));
        assert_eq!(hunk("a\nc", "a\nb\nc"), Some(h(1, &[], &["b"])));
        assert_eq!(hunk("a\nb\nc\n", "a\nc\n"), Some(h(1, &["b"], &[])));
    }

    #[test]
    fn separate_changes_stay_separate() {
        let before = ["a", "b", "c", "d", "e"];
        let after = ["a", "b", "c", "D", "e"];
        let event = h(3, &["d"], &["D"]);
        let (folded, replaced) = fold(&before, &after, &event, &[h(0, &["x"], &["a"])]);
        assert_eq!(folded, event);
        assert!(replaced.is_empty());
    }

    #[test]
    fn overlapping_changes_fold_back_to_the_original_text() {
        // Original "f\ng\nh" → agent made line 1 "G" → then again "G2\nG3".
        let before = ["f", "G", "h"];
        let after = ["f", "G2", "G3", "h"];
        let event = hunk(&before.join("\n"), &after.join("\n")).unwrap();
        let (folded, replaced) = fold(&before, &after, &event, &[h(1, &["g"], &["G"])]);
        assert_eq!(replaced, vec![0]);
        assert_eq!(folded, h(1, &["g"], &["G2", "G3"]));
    }

    #[test]
    fn one_write_with_two_edits_is_two_changes() {
        let whole = hunk("a\nb\nc\nd\ne", "a\nB\nc\nd\nE\nF").unwrap();
        assert_eq!(split(&whole), vec![h(1, &["b"], &["B"]), h(4, &["e"], &["E", "F"])]);
        let insert = hunk("a\nb", "a\nx\nb").unwrap();
        assert_eq!(split(&insert), vec![h(1, &[], &["x"])]);
    }

    #[test]
    fn edits_on_different_lines_merge() {
        let base = "a\nb\nc\nd\ne\nf";
        let mine = "a\nB\nc\nd\ne\nf";
        let theirs = "a\nb\nc\nd\nE\nE2\nf";
        assert_eq!(merge(base, mine, theirs).as_deref(), Some("a\nB\nc\nd\nE\nE2\nf"));
        assert_eq!(merge(base, theirs, mine).as_deref(), Some("a\nB\nc\nd\nE\nE2\nf"));
        // Lines added above shift the other's lines.
        let theirs = "x\ny\na\nb\nc\nd\ne\nf";
        assert_eq!(merge(base, "a\nb\nc\nd\ne\nF", theirs).as_deref(), Some("x\ny\na\nb\nc\nd\ne\nF"));
    }

    #[test]
    fn edits_on_the_same_lines_do_not_merge() {
        let base = "a\nb\nc";
        assert_eq!(merge(base, "a\nB\nc", "a\nb2\nc"), None);
        assert_eq!(merge(base, "a\nB\nc", "a\nb\nC"), None, "neighbouring lines are too close to tell");
    }

    #[test]
    fn changes_are_found_after_lines_moved() {
        let buffer = Buffer::new("new top\na\nB\nc");
        assert_eq!(locate(&buffer, &h(1, &["b"], &["B"])), Some(2));
        assert_eq!(locate(&buffer, &h(1, &["b"], &["gone"])), None);
    }
}
