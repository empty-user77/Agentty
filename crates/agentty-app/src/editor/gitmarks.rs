//! Lines changed since the last commit, marked in the gutter as VS Code does: green bars for lines
//! added, blue for lines changed, a red notch where lines were taken away. The file as committed
//! (`HEAD`) is read in the background and looked at again now and then (a commit made in a
//! terminal); the marks follow the text once typing pauses.

use super::review::{hunk, split};
use super::{CodeEditor, Content};
use gpui::{AppContext, Context};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// A file's lines against `HEAD`, and where reading and marking stand.
#[derive(Default)]
pub(super) struct GitMarks {
    /// (first line, lines, what happened) in the text as it is.
    pub marks: Vec<(usize, usize, Mark)>,
    /// The committed text (`None`: not in a repository, or not committed yet).
    base: Option<Arc<String>>,
    /// When the committed text was read last.
    read_at: Option<Instant>,
    /// The text revision the marks are for, and the one seen at the last tick (since when).
    marked: Option<u64>,
    seen: Option<(u64, Instant)>,
    running: bool,
}

/// The marks between `base` (committed) and `text`.
fn marks(base: &str, text: &str) -> Vec<(usize, usize, Mark)> {
    let Some(whole) = hunk(base, text) else { return Vec::new() };
    split(&whole)
        .into_iter()
        .map(|part| match (part.old.len(), part.new.len()) {
            (_, 0) => (part.start, 0, Mark::Deleted),
            (0, added) => (part.start, added, Mark::Added),
            (_, changed) => (part.start, changed, Mark::Modified),
        })
        .collect()
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

    pub(super) fn debug_git_marks(&self) -> serde_json::Value {
        let Some(doc) = self.docs.get(self.active) else { return serde_json::Value::Null };
        serde_json::json!(doc.git.marks.iter().map(|(line, n, mark)| format!("{line}+{n} {mark:?}")).collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_changed_and_taken_away() {
        let base = "a\nb\nc\nd\ne";
        assert_eq!(marks(base, base), []);
        assert_eq!(marks(base, "a\nb\nnew\nc\nd\ne"), [(2, 1, Mark::Added)]);
        assert_eq!(marks(base, "a\nB\nc\nd\ne"), [(1, 1, Mark::Modified)]);
        assert_eq!(marks(base, "a\nc\nd\ne"), [(1, 0, Mark::Deleted)]);
        assert_eq!(marks(base, "a\nB\nc\nd\ne\nf"), [(1, 1, Mark::Modified), (5, 1, Mark::Added)]);
    }
}
