//! Lines changed since the last commit, marked in the gutter as VS Code does: green bars for lines
//! added, blue for lines changed, a red notch where lines were taken away. A click on a mark shows
//! the lines as committed above the lines as they are, with **Revert** to put them back (one undo
//! step). The file as committed (`HEAD`) is read in the background and looked at again now and
//! then (a commit made in a terminal); the marks follow the text once typing pauses.

use super::buffer::Pos;
use super::review::{hunk, split};
use super::{CodeEditor, Content};
use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon_only, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, AppContext, ClickEvent, Context};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Lines of each side a change's window shows at most.
const PEEK_LINES: usize = 12;

/// A pause this long in typing marks the lines again.
const QUIET: Duration = Duration::from_millis(500);
/// How often the committed text is read again (a commit made meanwhile changes it).
const BASE_AGE: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mark {
    Added,
    Modified,
    /// Lines taken away just above this one.
    Deleted,
}

/// One change against `HEAD`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Change {
    /// Its first line in the text as it is, and how many lines it covers there.
    pub line: usize,
    pub rows: usize,
    pub mark: Mark,
    /// The lines as committed.
    pub old: Vec<String>,
}

impl Change {
    /// Whether a click on `line`'s mark is on this change.
    pub fn on(&self, line: usize) -> bool {
        line >= self.line && line < self.line + self.rows.max(1)
    }
}

/// A file's lines against `HEAD`, and where reading and marking stand.
#[derive(Default)]
pub(super) struct GitMarks {
    pub marks: Vec<Change>,
    /// The committed text (`None`: not in a repository, or not committed yet).
    base: Option<Arc<String>>,
    /// When the committed text was read last.
    read_at: Option<Instant>,
    /// The text revision the marks are for, and the one seen at the last tick (since when).
    marked: Option<u64>,
    seen: Option<(u64, Instant)>,
    running: bool,
}

/// The changes between `base` (committed) and `text`.
fn marks(base: &str, text: &str) -> Vec<Change> {
    let Some(whole) = hunk(base, text) else { return Vec::new() };
    split(&whole)
        .into_iter()
        .map(|part| {
            let (rows, mark) = match (part.old.len(), part.new.len()) {
                (_, 0) => (0, Mark::Deleted),
                (0, added) => (added, Mark::Added),
                (_, changed) => (changed, Mark::Modified),
            };
            Change { line: part.start, rows, mark, old: part.old }
        })
        .collect()
}

/// `text` with `change` put back as committed.
fn reverted(text: &str, change: &Change) -> String {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let end = (change.line + change.rows).min(lines.len());
    let start = change.line.min(end);
    lines.splice(start..end, change.old.iter().cloned());
    lines.join("\n")
}

impl CodeEditor {
    pub(super) fn git_tick(&mut self, cx: &mut Context<Self>) {
        let active = self.active;
        let Some(doc) = self.docs.get_mut(active).filter(|d| matches!(d.content, Content::Text)) else { return };
        if doc.git.running {
            return;
        }
        // The committed text, read again now and then.
        if doc.git.read_at.is_none_or(|at| at.elapsed() >= BASE_AGE) {
            doc.git.running = true;
            doc.git.read_at = Some(Instant::now());
            let path = doc.path.clone();
            cx.spawn(async move |this, cx| {
                let read = path.clone();
                let base = cx.background_spawn(async move { agentty_bridge::git::committed_text(&read) }).await;
                let _ = this.update(cx, |this, cx| {
                    let Some(doc) = this.docs.iter_mut().find(|d| d.path == path) else { return };
                    doc.git.running = false;
                    let base = base.map(|text| Arc::new(text.replace("\r\n", "\n")));
                    if base != doc.git.base {
                        doc.git.base = base;
                        doc.git.marked = None;
                        if doc.git.base.is_none() && !doc.git.marks.is_empty() {
                            doc.git.marks.clear();
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
            return;
        }
        let Some(base) = doc.git.base.clone() else { return };
        let revision = doc.buffer.revision();
        match doc.git.seen {
            Some((seen, _)) if seen == revision => {}
            _ => doc.git.seen = Some((revision, Instant::now())),
        }
        // The first marks at once; after that, once typing pauses.
        let paused = doc.git.marked.is_none() || doc.git.seen.is_some_and(|(_, since)| since.elapsed() >= QUIET);
        if doc.git.marked == Some(revision) || !paused {
            return;
        }
        doc.git.running = true;
        doc.git.marked = Some(revision);
        let (path, text) = (doc.path.clone(), doc.buffer.text());
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { marks(&base, &text) }).await;
            let _ = this.update(cx, |this, cx| {
                let Some(doc) = this.docs.iter_mut().find(|d| d.path == path) else { return };
                doc.git.running = false;
                if doc.git.marks != found {
                    doc.git.marks = found;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A click on the change marks at `line`: its window opens (or closes).
    pub(super) fn toggle_git_peek(&mut self, line: usize, cx: &mut Context<Self>) -> bool {
        let Some(doc) = self.docs.get(self.active) else { return false };
        let Some(change) = doc.git.marks.iter().find(|c| c.on(line)) else { return false };
        let key = (doc.path.clone(), change.line);
        self.git_peek = if self.git_peek.as_ref() == Some(&key) { None } else { Some(key) };
        cx.notify();
        true
    }

    fn peeked_change(&self) -> Option<&Change> {
        let (path, line) = self.git_peek.as_ref()?;
        let doc = self.docs.get(self.active).filter(|d| &d.path == path)?;
        doc.git.marks.iter().find(|c| c.line == *line)
    }

    /// Puts the change in the window back as committed (one undo step). The changes are worked
    /// out again from the text as it is: typed since the marks were, the lines may have moved,
    /// and a change no longer there is left alone.
    pub(super) fn revert_git_change(&mut self, cx: &mut Context<Self>) {
        let Some(change) = self.peeked_change().cloned() else { return };
        self.git_peek = None;
        let doc = &self.docs[self.active];
        let Some(base) = doc.git.base.clone() else { return cx.notify() };
        let text = doc.buffer.text();
        let now = marks(&base, &text);
        let found = now.iter().find(|c| *c == &change).or_else(|| now.iter().find(|c| c.old == change.old && c.mark == change.mark));
        if let Some(found) = found {
            let new = reverted(&text, found);
            self.replace_text(self.active, &new, cx);
        }
        cx.notify();
    }

    pub(super) fn render_git_peek(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.picker.is_some() || self.confirm.is_some() || self.preview.covered {
            return None;
        }
        let change = self.peeked_change()?;
        let doc = self.docs.get(self.active)?;
        let last = (change.line + change.rows.max(1)).saturating_sub(1);
        let at = self.layout.as_ref()?.point_below(Pos::new(last, 0))?;
        let font = crate::settings::terminal_font(cx);
        let row = |sign: &'static str, text: String, color: u32| {
            div()
                .flex()
                .gap_2()
                .px_2()
                .bg(hex_alpha(color, 0.12))
                .child(div().w(px(10.)).flex_shrink_0().text_color(hex(color)).child(sign))
                .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::FOREGROUND)).child(text))
        };
        let old = change.old.iter().take(PEEK_LINES).map(|l| row("−", l.clone(), Chrome::ERROR));
        let new =
            (change.line..change.line + change.rows).take(PEEK_LINES).map(|i| row("+", doc.buffer.line(i).to_string(), Chrome::SUCCESS));
        let panel = div()
            .id("editor-git-peek")
            .w(px(560.))
            .rounded_md()
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
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(hex(Chrome::BORDER))
                    .t_caption()
                    .text_color(hex(Chrome::MUTED))
                    .child(div().flex_1().child("HEAD"))
                    .child(
                        div()
                            .id("editor-git-revert")
                            .px_1p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(hex(Chrome::FOREGROUND))
                            .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                            .child(t(cx, "editor.git_revert"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.revert_git_change(cx))),
                    )
                    .child(icon_only(
                        "editor-git-peek-close",
                        "x",
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.git_peek = None;
                            cx.notify();
                        }),
                    )),
            )
            .child(div().py_1().font_family(font).text_size(px(12.)).children(old).children(new));
        Some(
            gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(self.popup_margin()).child(panel))
                .with_priority(2)
                .into_any_element(),
        )
    }

    pub(super) fn debug_git_marks(&self) -> serde_json::Value {
        let Some(doc) = self.docs.get(self.active) else { return serde_json::Value::Null };
        serde_json::json!(doc.git.marks.iter().map(|c| format!("{}+{} {:?}", c.line, c.rows, c.mark)).collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short(found: Vec<Change>) -> Vec<(usize, usize, Mark)> {
        found.into_iter().map(|c| (c.line, c.rows, c.mark)).collect()
    }

    #[test]
    fn added_changed_and_taken_away() {
        let base = "a\nb\nc\nd\ne";
        assert_eq!(short(marks(base, base)), []);
        assert_eq!(short(marks(base, "a\nb\nnew\nc\nd\ne")), [(2, 1, Mark::Added)]);
        assert_eq!(short(marks(base, "a\nB\nc\nd\ne")), [(1, 1, Mark::Modified)]);
        assert_eq!(short(marks(base, "a\nc\nd\ne")), [(1, 0, Mark::Deleted)]);
        assert_eq!(short(marks(base, "a\nB\nc\nd\ne\nf")), [(1, 1, Mark::Modified), (5, 1, Mark::Added)]);
    }

    #[test]
    fn each_change_goes_back_as_committed() {
        let base = "a\nb\nc\nd\ne";
        for text in ["a\nb\nnew\nc\nd\ne", "a\nB\nc\nd\ne", "a\nc\nd\ne"] {
            let found = marks(base, text);
            assert_eq!(found.len(), 1);
            assert_eq!(reverted(text, &found[0]), base, "{text:?}");
        }
        // One of two changes: the other stays.
        let text = "a\nB\nc\nd\ne\nf";
        let found = marks(base, text);
        assert_eq!(reverted(text, &found[1]), "a\nB\nc\nd\ne");
        assert!(found[0].on(1) && !found[0].on(2));
    }
}
