//! Local Claude Code / Codex session discovery and cross-agent context handoff.

pub mod agent_auth;
pub mod agy;
pub mod amp;
pub mod claude;
pub mod codex;
pub mod connectors;
pub mod context;
pub mod extensions;
pub mod fsutil;
pub mod gemini;
pub mod git;
pub mod handoff;
pub mod harness;
pub mod http;
pub mod idea;
pub mod kimi;
pub mod limits;
pub mod metrics;
pub mod model;
pub mod plugins;
pub mod pricing;
pub mod process;
pub mod protobuf;
pub mod secret_store;
pub mod service_status;
pub mod update;
pub mod usage;

use anyhow::Result;
use model::{Agent, SessionInfo, Turn};
use std::path::PathBuf;

/// Sessions from the selected agents, newest first.
pub fn list(agent: Option<Agent>, limit: usize) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    if agent.is_none_or(|a| a == Agent::Claude) {
        sessions.extend(claude::list(limit));
    }
    if agent.is_none_or(|a| a == Agent::Codex) {
        sessions.extend(codex::list(limit));
    }
    if agent.is_none_or(|a| a == Agent::Agy) {
        sessions.extend(agy::list(limit));
    }
    if agent.is_none_or(|a| a == Agent::Amp) {
        sessions.extend(amp::list(limit));
    }
    if agent.is_none_or(|a| a == Agent::Gemini) {
        sessions.extend(gemini::list(limit));
    }
    if agent.is_none_or(|a| a == Agent::Kimi) {
        sessions.extend(kimi::list(limit));
    }
    sessions.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
    sessions.truncate(limit);
    sessions
}

/// Working directory and conversation turns of a session.
pub fn load(agent: Agent, id: &str) -> Result<(Option<String>, Vec<Turn>)> {
    match agent {
        Agent::Claude => claude::transcript(&claude::find(id)?),
        Agent::Codex => codex::transcript(&codex::find(id)?),
        Agent::Agy => agy::transcript(&agy::find(id)?),
        Agent::Amp => amp::transcript(&amp::find(id)?),
        Agent::Gemini => gemini::transcript(&gemini::find(id)?),
        Agent::Kimi => kimi::transcript(&kimi::find(id)?),
    }
}

/// Live facts about a session, read from the end of its transcript.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub model: Option<String>,
    /// Prompt tokens of the most recent request.
    pub context_used: u64,
    pub context_window: u64,
    /// Plan rate-limit usage (Codex reports it; Claude Code does not store it locally).
    pub rate_limit_percent: Option<f64>,
}

impl SessionStats {
    pub fn context_percent(&self) -> Option<f64> {
        (self.context_window > 0 && self.context_used > 0)
            .then(|| (self.context_used as f64 / self.context_window as f64 * 100.0).min(100.0))
    }
}

/// Context window by model family. Current Claude models (Opus 5, Sonnet 5, Fable) have 1M tokens;
/// Haiku 4.5 and older 4.5-generation models have 200K.
pub fn claude_context_window(model: &str) -> u64 {
    let small = ["claude-haiku", "claude-sonnet-4-5", "claude-opus-4-5", "claude-opus-4-1", "claude-sonnet-4-2", "claude-3"];
    if small.iter().any(|p| model.starts_with(p)) {
        200_000
    } else {
        1_000_000
    }
}

/// Path of a session transcript, for the agents that keep one Agentty can read.
fn transcript_path(agent: Agent, id: &str) -> Option<PathBuf> {
    match agent {
        Agent::Claude => claude::find(id).ok(),
        Agent::Codex => codex::find(id).ok(),
        _ => None,
    }
}

/// Which of two session ids a pane should be reading right now.
///
/// A pane pins the session id it launched with, but the agent does not always keep writing to it:
/// `/clear` and a resume both fork Claude Code into a *new* transcript with a new id, and the pinned
/// one stops growing. Reading it forever freezes the model, the context meter and the subagent count
/// at whatever they were when the fork happened. Whichever transcript was written most recently is
/// the live one.
pub fn live_session_id(agent: Agent, pinned: Option<String>, recent: Option<String>) -> Option<String> {
    pick_live(pinned, recent, |id| transcript_path(agent, id).map(|p| fsutil::mtime_ms(&p)).unwrap_or(0))
}

fn pick_live(pinned: Option<String>, recent: Option<String>, written_at: impl Fn(&str) -> u64) -> Option<String> {
    match (pinned, recent) {
        (Some(pinned), Some(recent)) if pinned != recent => {
            if written_at(&recent) > written_at(&pinned) {
                Some(recent)
            } else {
                Some(pinned)
            }
        }
        (pinned, recent) => pinned.or(recent),
    }
}

pub fn session_stats(agent: Agent, id: &str) -> Option<SessionStats> {
    use std::io::{Read, Seek, SeekFrom};
    let path = match agent {
        Agent::Claude => claude::find(id).ok()?,
        Agent::Codex => codex::find(id).ok()?,
        _ => return None,
    };
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(512 * 1024))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    // The seek may land inside a multi-byte character; lossy decoding only affects the first line.
    let tail = String::from_utf8_lossy(&bytes);
    let mut stats = SessionStats::default();
    match agent {
        Agent::Claude => {
            let line = tail.lines().rev().filter(|l| l.contains("\"type\":\"assistant\"") && l.contains("\"usage\"")).find_map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).ok()?;
                let model = v["message"]["model"].as_str()?;
                (!model.starts_with('<')).then_some(v)
            })?;
            let model = line["message"]["model"].as_str().unwrap_or_default().to_string();
            let u = &line["message"]["usage"];
            let n = |k: &str| u[k].as_u64().unwrap_or(0);
            stats.context_used = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
            stats.context_window = claude_context_window(&model);
            stats.model = Some(model);
        }
        Agent::Codex => {
            for line in tail.lines().rev() {
                if stats.model.is_none() && line.contains("\"type\":\"turn_context\"") {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                        stats.model = v["payload"]["model"].as_str().map(str::to_string);
                    }
                }
                if stats.context_window == 0 && line.contains("\"token_count\"") {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                        let info = &v["payload"]["info"];
                        if !info.is_null() {
                            stats.context_used = info["last_token_usage"]["input_tokens"].as_u64().unwrap_or(0);
                            stats.context_window = info["model_context_window"].as_u64().unwrap_or(0);
                        }
                        stats.rate_limit_percent = v["payload"]["rate_limits"]["primary"]["used_percent"].as_f64();
                    }
                }
                if stats.model.is_some() && stats.context_window > 0 {
                    break;
                }
            }
        }
        _ => {}
    }
    Some(stats)
}

/// Whether the session's latest turn was stopped by the user (Esc), from the end of its transcript.
pub fn last_turn_interrupted(agent: Agent, id: &str) -> bool {
    let path = match agent {
        Agent::Claude => claude::find(id),
        Agent::Codex => codex::find(id),
        _ => return false,
    };
    let Ok(path) = path else { return false };
    turn_interrupted(agent, fsutil::tail_lines_rev(&path, 96 * 1024).iter().map(String::as_str))
}

/// `lines` newest first.
fn turn_interrupted<'a>(agent: Agent, lines: impl Iterator<Item = &'a str>) -> bool {
    for line in lines {
        match agent {
            Agent::Claude => {
                if line.contains("[Request interrupted by user") {
                    return true;
                }
                if line.contains("\"stop_reason\":\"end_turn\"") || (line.contains("\"type\":\"user\"") && !line.contains("tool_result")) {
                    return false;
                }
            }
            Agent::Codex => {
                if line.contains("\"type\":\"turn_aborted\"") {
                    return true;
                }
                if line.contains("\"type\":\"task_complete\"")
                    || line.contains("\"type\":\"turn_complete\"")
                    || line.contains("\"type\":\"user_message\"")
                {
                    return false;
                }
            }
            _ => return false,
        }
    }
    false
}

/// Compact token count for labels: 1_000_000 → "1M", 200_000 → "200K".
pub fn short_tokens(value: u64) -> String {
    match value {
        v if v >= 1_000_000 && v % 1_000_000 == 0 => format!("{}M", v / 1_000_000),
        v if v >= 1_000_000 => format!("{:.1}M", v as f64 / 1_000_000.0),
        v if v >= 1_000 => format!("{}K", v / 1_000),
        v => v.to_string(),
    }
}

/// Human-friendly model name: `claude-opus-5` → `Opus 5`, `claude-haiku-4-5-20251001` → `Haiku 4.5`.
pub fn pretty_model(model: &str) -> String {
    let Some(rest) = model.strip_prefix("claude-") else { return model.to_string() };
    let mut parts = rest.split('-');
    let Some(family) = parts.next() else { return model.to_string() };
    let version: Vec<&str> = parts.take_while(|p| p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit())).collect();
    let mut name = family.to_string();
    if let Some(first) = name.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    if version.is_empty() {
        name
    } else {
        format!("{name} {}", version.join("."))
    }
}

/// Whether a transcript file mentions `needle` (case-insensitive), for full-text session search.
pub fn transcript_contains(path: &std::path::Path, needle: &str) -> bool {
    use std::io::BufRead;
    let needle = needle.to_lowercase();
    let Ok(file) = std::fs::File::open(path) else { return false };
    std::io::BufReader::new(file).lines().map_while(Result::ok).any(|line| line.to_lowercase().contains(&needle))
}

#[cfg(test)]
mod model_name_tests {
    use super::pretty_model;

    #[test]
    fn detects_interrupted_turns() {
        use super::{turn_interrupted, Agent};
        let claude = [
            r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#,
            r#"{"type":"assistant","message":{"stop_reason":null}}"#,
        ];
        assert!(turn_interrupted(Agent::Claude, claude.into_iter()));
        let finished = [
            r#"{"type":"assistant","message":{"stop_reason":"end_turn"}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#,
        ];
        assert!(!turn_interrupted(Agent::Claude, finished.into_iter()));
        let codex = [r#"{"type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}"#];
        assert!(turn_interrupted(Agent::Codex, codex.into_iter()));
    }

    #[test]
    fn context_windows_and_labels() {
        assert_eq!(super::claude_context_window("claude-opus-5"), 1_000_000);
        assert_eq!(super::claude_context_window("claude-haiku-4-5-20251001"), 200_000);
        assert_eq!(super::short_tokens(1_000_000), "1M");
        assert_eq!(super::short_tokens(258_400), "258K");
        let stats = super::SessionStats { context_used: 890_000, context_window: 1_000_000, ..Default::default() };
        assert_eq!(stats.context_percent().map(|p| p.round()), Some(89.0));
    }

    #[test]
    fn follows_the_transcript_that_is_still_being_written() {
        let at = |id: &str| match id {
            "pinned" => 100,
            "fork" => 200,
            _ => 0,
        };
        let pick =
            |p: Option<&str>, r: Option<&str>| super::pick_live(p.map(str::to_string), r.map(str::to_string), at).unwrap_or_default();
        // `/clear` forked the session: the newer file wins, so the context meter keeps moving.
        assert_eq!(pick(Some("pinned"), Some("fork")), "fork");
        // A stale neighbour must not steal a pane that is still writing its own transcript.
        assert_eq!(pick(Some("fork"), Some("pinned")), "fork");
        assert_eq!(pick(Some("pinned"), None), "pinned");
        assert_eq!(pick(None, Some("fork")), "fork");
        assert_eq!(pick(None, None), "");
    }

    #[test]
    fn prettifies_claude_models() {
        assert_eq!(pretty_model("claude-opus-5"), "Opus 5");
        assert_eq!(pretty_model("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(pretty_model("claude-fable-5-1"), "Fable 5.1");
        assert_eq!(pretty_model("gpt-6-astra"), "gpt-6-astra");
    }
}
