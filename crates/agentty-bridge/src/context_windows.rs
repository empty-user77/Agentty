//! Context windows Claude Code has reported, per model, for when it has not (yet) said so for a
//! session: a `claude` typed into a plain shell pane, a pane before its first statusline render, a
//! session read from history. Claude Code's statusline input states the window of the model in use;
//! `agentty statusline` remembers it here. Nothing is ever derived from a model's name: a model
//! Claude Code never reported stays unknown.

use crate::fsutil;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

type Known = BTreeMap<String, u64>;

fn file() -> PathBuf {
    fsutil::data_dir().join("limits").join("context-windows.json")
}

fn read() -> Known {
    std::fs::read(file()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Remembers the window Claude Code reported for `model` (its id, `claude-haiku-5-5` or
/// `claude-opus-5-5[1m]`). Written only when it changes: the statusline runs on every redraw.
pub fn remember(model: &str, window: u64) {
    if model.is_empty() || model.len() > 80 || window == 0 {
        return;
    }
    let mut known = read();
    if known.get(model) == Some(&window) {
        return;
    }
    known.insert(model.to_string(), window);
    if let Ok(json) = serde_json::to_vec(&known) {
        let _ = fsutil::write_private(&file(), &json);
    }
}

/// The window Claude Code last reported for `model`, named by id (`claude-haiku-4-5-20251001`) or
/// as Claude Code writes it for people (`Haiku 4.5`). Read from disk at most every few seconds, so
/// the UI can ask while drawing.
pub fn known(model: &str) -> Option<u64> {
    static CACHE: Mutex<Option<(Instant, Known)>> = Mutex::new(None);
    let mut cache = CACHE.lock().ok()?;
    if cache.as_ref().is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(5)) {
        *cache = Some((Instant::now(), read()));
    }
    lookup(&cache.as_ref()?.1, model)
}

fn lookup(known: &Known, model: &str) -> Option<u64> {
    if let Some(window) = known.get(model) {
        return Some(*window);
    }
    // A long-window variant (`[1m]`) is only ever asked for by that exact id.
    if model.ends_with("[1m]") {
        return None;
    }
    // The same model under another spelling: a dated id, or its display name.
    let name = crate::pretty_model(model);
    known.iter().find(|(id, _)| !id.ends_with("[1m]") && crate::pretty_model(id) == name).map(|(_, window)| *window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_reported_window_by_id_or_name_and_never_guesses() {
        let known: Known =
            [("claude-haiku-5-5".into(), 1_000_000), ("claude-haiku-4-5".into(), 200_000), ("claude-opus-4-6[1m]".into(), 1_000_000)]
                .into();
        assert_eq!(lookup(&known, "claude-haiku-5-5"), Some(1_000_000));
        assert_eq!(lookup(&known, "claude-haiku-4-5-20251001"), Some(200_000));
        assert_eq!(lookup(&known, "Haiku 5.5"), Some(1_000_000));
        // The long-window variant says nothing about the plain one, and the reverse.
        assert_eq!(lookup(&known, "claude-opus-4-6"), None);
        assert_eq!(lookup(&known, "claude-opus-4-6[1m]"), Some(1_000_000));
        assert_eq!(lookup(&known, "claude-sonnet-4-6[1m]"), None);
        // Never reported: unknown.
        assert_eq!(lookup(&known, "claude-haiku-6"), None);
    }
}
