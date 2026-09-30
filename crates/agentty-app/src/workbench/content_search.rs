//! What the workspace and session search boxes found inside conversations, and how it is shown.
//!
//! Both boxes search the transcripts in the background while the user types. Typing on narrows
//! the words ("dep" → "deploy"), so a search remembers what the last one found: a file that did
//! not hold the shorter words cannot hold the longer ones, and only files still worth reading are
//! read again (and any that changed since).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use agentty_bridge::search::{HitRole, Needle, TranscriptHit};
use gpui::{div, prelude::*, Div, HighlightStyle, SharedString, StyledText};

use crate::i18n::t;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;

/// The last search's words, the files it read (with their size then) and what it found.
struct Memo {
    words: String,
    read: HashMap<PathBuf, u64>,
    hits: HashMap<PathBuf, TranscriptHit>,
}

/// One search box's memory of its last search, shared with the background task.
#[derive(Clone, Default)]
pub(crate) struct SearchMemo(Arc<Mutex<Option<Memo>>>);

/// Searches `paths` for `words`; `None` when the search was called off before it finished.
pub(crate) fn search(
    paths: Vec<PathBuf>,
    words: &str,
    memo: &SearchMemo,
    cancelled: &AtomicBool,
) -> Option<HashMap<PathBuf, TranscriptHit>> {
    let needle = Needle::new(words)?;
    let size = |path: &PathBuf| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let sizes: Vec<u64> = paths.iter().map(size).collect();
    let mut found: HashMap<PathBuf, TranscriptHit> = HashMap::new();
    let mut to_read: Vec<PathBuf> = Vec::new();
    {
        let memo = memo.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let narrower = memo.as_ref().filter(|m| needle.words().contains(&m.words));
        for (path, size) in paths.iter().zip(&sizes) {
            match narrower {
                // Unchanged since the shorter words were looked for, and they were not in it.
                Some(m) if m.read.get(path) == Some(size) && !m.hits.contains_key(path) => {}
                // Unchanged, and the same words again: the answer is already known.
                Some(m) if m.words == needle.words() && m.read.get(path) == Some(size) => {
                    if let Some(hit) = m.hits.get(path) {
                        found.insert(path.clone(), hit.clone());
                    }
                }
                _ => to_read.push(path.clone()),
            }
        }
    }
    let results = agentty_bridge::search::search_transcripts(&to_read, &needle, cancelled);
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    found.extend(to_read.into_iter().zip(results).filter_map(|(path, hit)| Some((path, hit?))));
    {
        let mut memo = memo.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *memo = Some(Memo { words: needle.words().to_string(), read: paths.into_iter().zip(sizes).collect(), hits: found.clone() });
    }
    Some(found)
}

/// A search running in the background, to call off when the words change.
#[derive(Default)]
pub(crate) struct Running(Option<Arc<AtomicBool>>);

impl Running {
    /// Calls off the search before (if any) and hands out the flag of the next one.
    pub fn restart(&mut self) -> Arc<AtomicBool> {
        self.stop();
        let flag = Arc::new(AtomicBool::new(false));
        self.0 = Some(flag.clone());
        flag
    }

    pub fn stop(&mut self) {
        if let Some(flag) = self.0.take() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// What a workspace search found in one workspace's conversations: the most recent session that
/// holds the words, where, and how many sessions do.
#[derive(Clone)]
pub(crate) struct WorkspaceHit {
    pub session_title: String,
    pub hit: TranscriptHit,
    pub sessions: usize,
}

/// The ranges of `text` that hold `words` (lower case), for highlighting a title or a folder.
pub(crate) fn ranges_of(text: &str, words: &str) -> Vec<std::ops::Range<usize>> {
    if words.is_empty() {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    // Lower-casing kept every byte offset (it nearly always does): the ranges carry over as they
    // are. Otherwise nothing is highlighted rather than something wrong.
    if lower.len() != text.len() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find(words).map(|i| i + from) {
        let end = at + words.len();
        if !text.is_char_boundary(at) || !text.is_char_boundary(end) {
            break;
        }
        ranges.push(at..end);
        from = end;
    }
    ranges
}

/// The marker colour behind found words.
const MARK: u32 = 0xe5c07b;

fn highlight() -> HighlightStyle {
    HighlightStyle {
        color: Some(hex(Chrome::BRIGHT)),
        background_color: Some(hex_alpha(MARK, 0.35)),
        font_weight: Some(crate::theme::EMPHASIS),
        ..Default::default()
    }
}

/// `text` with every place `words` appears in it marked.
pub(crate) fn marked(text: impl Into<SharedString>, words: &str) -> StyledText {
    let text: SharedString = text.into();
    let ranges = ranges_of(&text, words);
    StyledText::new(text).with_highlights(ranges.into_iter().map(|r| (r, highlight())))
}

/// A snippet with the words marked where they were found.
/// Only the last `before` characters of what came first are kept, so the words themselves are
/// in view however narrow the line.
pub(crate) fn snippet(hit: &TranscriptHit, before: usize) -> StyledText {
    let lead: Vec<char> = hit.before.trim_start_matches('…').chars().collect();
    let lead = if lead.len() > before || hit.before.starts_with('…') {
        let kept: String = lead[lead.len().saturating_sub(before)..].iter().collect();
        format!("…{}", kept.trim_start())
    } else {
        hit.before.clone()
    };
    let text = format!("{lead}{}{}", hit.matched, hit.after);
    let start = lead.len();
    StyledText::new(text).with_highlights([(start..start + hit.matched.len(), highlight())])
}

/// Who said it, as a short label.
pub(crate) fn role_label(role: HitRole, cx: &gpui::App) -> &'static str {
    match role {
        HitRole::User => t(cx, "search.role_user"),
        HitRole::Assistant => t(cx, "search.role_agent"),
        HitRole::Other => t(cx, "search.role_tool"),
    }
}

/// A small label naming where a result matched (name, folder, conversation).
pub(crate) fn tag(label: impl Into<SharedString>) -> Div {
    div()
        .flex_shrink_0()
        .px_1()
        .rounded_sm()
        .bg(hex_alpha(Chrome::ACCENT, 0.18))
        .t_caption()
        .text_color(hex(Chrome::FOREGROUND))
        .child(label.into())
}

/// One line of a result: a label saying where, then the text with the words marked.
pub(crate) fn match_line(label: impl Into<SharedString>, text: StyledText) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .min_w_0()
        .child(tag(label))
        .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(0xa8a8a8)).child(text))
}

/// [`match_line`] with the text given two lines: the label above, the sentence below it.
pub(crate) fn match_block(label: impl Into<SharedString>, text: StyledText) -> Div {
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap_0p5()
        .min_w_0()
        .child(tag(label))
        .child(div().w_full().min_w_0().line_clamp(2).t_small().text_color(hex(0xa8a8a8)).child(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn keys(map: &HashMap<PathBuf, TranscriptHit>) -> HashSet<PathBuf> {
        map.keys().cloned().collect()
    }

    #[test]
    fn every_place_the_words_appear_is_marked() {
        assert_eq!(ranges_of("Deploy the deploy script", "deploy"), vec![0..6, 11..17]);
        assert_eq!(ranges_of("작업공간 목록", "목록"), vec![13..19]);
        assert!(ranges_of("anything", "").is_empty());
        assert!(ranges_of("nothing here", "zzz").is_empty());
    }

    fn write(dir: &std::path::Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, serde_json::json!({"type":"user","message":{"role":"user","content":text}}).to_string()).unwrap();
        path
    }

    #[test]
    fn typing_on_reads_only_what_can_still_match_and_notices_changed_files() {
        let dir = std::env::temp_dir().join(format!("agentty-content-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = write(&dir, "a.jsonl", "deploy the app");
        let b = write(&dir, "b.jsonl", "depends on it");
        let c = write(&dir, "c.jsonl", "unrelated");
        let memo = SearchMemo::default();
        let flag = AtomicBool::new(false);
        let paths = vec![a.clone(), b.clone(), c.clone()];

        let first = search(paths.clone(), "dep", &memo, &flag).unwrap();
        assert_eq!(keys(&first), HashSet::from([a.clone(), b.clone()]));

        // c changed after the first search: it is read again even though "dep" was not in it.
        write(&dir, "c.jsonl", "deploy later, once more");
        let second = search(paths.clone(), "deploy", &memo, &flag).unwrap();
        assert_eq!(keys(&second), HashSet::from([a.clone(), c.clone()]));

        // Words that do not extend the last ones start over.
        let third = search(paths.clone(), "unrelated", &memo, &flag).unwrap();
        assert!(third.is_empty());
        let fourth = search(paths, "it", &memo, &flag).unwrap();
        assert_eq!(keys(&fourth), HashSet::from([b]));

        assert!(search(vec![a], "deploy", &memo, &AtomicBool::new(true)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
