//! Local Claude Code / Codex session discovery and cross-agent context handoff.

pub mod agent_auth;
pub mod agy;
pub mod amp;
pub mod claude;
pub mod claude_trust;
pub mod codex;
pub mod connectors;
pub mod context;
pub mod disk;
pub mod docker;
pub mod extensions;
pub mod fsutil;
pub mod gemini;
pub mod git;
pub mod github;
pub mod handoff;
pub mod harness;
pub mod http;
pub mod idea;
pub mod inventory;
pub mod kimi;
pub mod limits;
pub mod metrics;
pub mod model;
pub mod notify;
pub mod plugins;
pub mod pricing;
pub mod process;
pub mod protobuf;
pub mod secret_store;
pub mod service_status;
pub mod sync;
pub mod update;
pub mod usage;
pub mod worktree;

use anyhow::{Context as _, Result};
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

/// Deletes a session's transcript. The file is what the agent resumes from, so removing it is
/// what "delete this session" means; nothing else on disk refers to it.
pub fn delete(session: &SessionInfo) -> Result<()> {
    let path = &session.path;
    anyhow::ensure!(path.is_file(), "session file is gone: {}", path.display());
    // Only ever a transcript inside an agent's own session folder. Each reader says where its own
    // sessions live, so the guard cannot drift from where the files actually are — spelling these
    // out here had it refusing every Amp session, whose folder is not the one the list guessed.
    let roots = [
        claude::session_root(),
        codex::session_root(),
        agy::session_root(),
        amp::session_root(),
        gemini::session_root(),
        kimi::session_root(),
    ];
    // `..` is refused rather than resolved: a transcript path never has one, and without this the
    // check below would accept a path that climbs back out of the folder it starts in.
    anyhow::ensure!(!path.components().any(|c| c == std::path::Component::ParentDir), "not an agent session file: {}", path.display());
    anyhow::ensure!(roots.iter().any(|root| path.starts_with(root)), "not an agent session file: {}", path.display());
    std::fs::remove_file(path).with_context(|| format!("could not delete {}", path.display()))?;
    Ok(())
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
/// The newest session of `agent` in `cwd` written after `since_ms`, from its files.
pub fn find_recent(agent: Agent, cwd: &std::path::Path, since_ms: u64) -> Option<String> {
    match agent {
        Agent::Claude => claude::find_recent(cwd, since_ms),
        Agent::Codex => codex::find_recent(cwd, since_ms),
        Agent::Gemini => gemini::find_recent(cwd, since_ms),
        Agent::Kimi => kimi::find_recent(cwd, since_ms),
        Agent::Agy | Agent::Amp => None,
    }
}

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
        Agent::Claude => stats = claude_stats(tail.lines().rev())?,
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

/// Model and context of a Claude Code session, from its transcript lines newest first.
///
/// The prompt size of the latest request is the context in use — until `/compact` (or an automatic
/// compaction) replaces the conversation with a summary. Nothing with `usage` is written then until
/// the next prompt, so the boundary's own `postTokens` is the reading in between; without it the
/// meter would keep showing the full window the compaction just emptied.
fn claude_stats<'a>(lines: impl Iterator<Item = &'a str>) -> Option<SessionStats> {
    let mut compacted_to = None;
    // A `/model` newer than the latest answer: the model the next answer will come from.
    let mut switched: Option<(String, u64)> = None;
    for line in lines {
        if switched.is_none() && (line.contains("Set model to") || line.contains("Kept model as")) {
            switched = model_switch(line);
            if switched.is_some() {
                continue;
            }
        }
        if compacted_to.is_none() && line.contains("\"compact_boundary\"") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if v["type"] == "system" && v["subtype"] == "compact_boundary" {
                    // Older versions wrote no `postTokens`: a small floor still reads as "just compacted".
                    compacted_to = Some(v["compactMetadata"]["postTokens"].as_u64().filter(|n| *n > 0).unwrap_or(1));
                }
            }
            continue;
        }
        if !(line.contains("\"type\":\"assistant\"") && line.contains("\"usage\"")) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(model) = v["message"]["model"].as_str().filter(|m| !m.starts_with('<')) else { continue };
        let u = &v["message"]["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let prompt = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
        let (model, context_window) = switched.unwrap_or_else(|| (model.to_string(), claude_context_window(model)));
        return Some(SessionStats {
            context_used: compacted_to.unwrap_or(prompt),
            context_window,
            model: Some(model),
            rate_limit_percent: None,
        });
    }
    // Nothing answered yet, but a model was chosen: name it, with an empty meter.
    switched.map(|(model, context_window)| SessionStats { context_used: 0, context_window, model: Some(model), rate_limit_percent: None })
}

/// The model `/model` switched to, as Claude Code wrote it the moment the command ran — long before
/// an answer from that model names it: "Set model to `Opus 5.5 (1M context) (default)`" →
/// `("Opus 5.5", 1_000_000)`. Only Claude Code's own command output counts, never a message that
/// quotes it.
fn model_switch(line: &str) -> Option<(String, u64)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v["type"] != "user" {
        return None;
    }
    let content = v["message"]["content"].as_str()?.strip_prefix("<local-command-stdout>")?;
    // "Kept model as" is `/model` confirming the one in use: just as telling as a switch.
    let rest = content.strip_prefix("Set model to").or_else(|| content.strip_prefix("Kept model as"))?.trim_start();
    let name = match rest.strip_prefix('`') {
        Some(quoted) => quoted.split('`').next()?,
        None => rest.split(" and saved").next()?.split("</local-command-stdout>").next()?,
    };
    parse_model_display(name.trim().trim_end_matches("(default)"))
}

/// A model as Claude Code writes it for people — "Opus 5.5 (1M context)", "Sonnet 5" — as a name
/// and a context window: the one it states, else the one that model has by default.
pub fn parse_model_display(text: &str) -> Option<(String, u64)> {
    let text = text.trim();
    // "(1M context)" / "(200K context)" is Claude Code's own way of naming the window.
    let (name, window) = match text.rsplit_once(" (").filter(|(_, tail)| tail.ends_with(" context)")) {
        Some((bare, tail)) => (bare.trim(), parse_token_count(tail.trim_end_matches(" context)"))),
        None => (text, None),
    };
    // A model's name: a word and a version ("Opus 5.5", "Haiku 4.5"), nothing longer.
    let plausible = name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name.chars().any(|c| c.is_ascii_digit())
        && name.len() <= 40
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '.' || c == '-');
    if !plausible {
        return None;
    }
    // Without a stated window, the one the model has by default: "Haiku 4.5" → claude-haiku-4-5.
    let window = window.unwrap_or_else(|| claude_context_window(&format!("claude-{}", name.to_lowercase().replace([' ', '.'], "-"))));
    Some((name.to_string(), window))
}

/// The model in Claude Code's welcome banner, from the line under "Claude Code vX":
/// "Opus 5.5 (1M context) · Claude Max" → ("Opus 5.5", 1M). A session that has not answered yet
/// has written nothing else that says which model it runs.
pub fn banner_model(line: &str) -> Option<(String, u64)> {
    let (model, _plan) = line.split_once(" · ")?;
    parse_model_display(model)
}

/// "1M" → 1 000 000, "200K" → 200 000.
fn parse_token_count(text: &str) -> Option<u64> {
    let text = text.trim();
    let (number, scale) = match text.chars().last()? {
        'M' | 'm' => (&text[..text.len() - 1], 1_000_000.0),
        'K' | 'k' => (&text[..text.len() - 1], 1_000.0),
        _ => (text, 1.0),
    };
    let value: f64 = number.trim().parse().ok()?;
    (value > 0.0).then(|| (value * scale).round() as u64)
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
    use std::io::{BufRead, Read};
    /// How much of a transcript a search reads. A transcript is one line per message, and a search
    /// runs over every session while someone is still typing — without a limit here, one file left
    /// without a line break (a corrupted transcript, or one huge tool result) would be read into
    /// memory whole. Anything real is far below this.
    const MAX_SCAN: u64 = 16 * 1024 * 1024;
    let needle = needle.to_lowercase();
    let Ok(file) = std::fs::File::open(path) else { return false };
    std::io::BufReader::new(file.take(MAX_SCAN)).lines().map_while(Result::ok).any(|line| line.to_lowercase().contains(&needle))
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
    fn the_banner_says_which_model_a_new_session_runs() {
        use super::banner_model;
        assert_eq!(banner_model("Opus 5.5 (1M context) · Claude Max"), Some(("Opus 5.5".into(), 1_000_000)));
        assert_eq!(banner_model("  Sonnet 5 · Claude Max   "), Some(("Sonnet 5".into(), 1_000_000)));
        assert_eq!(banner_model("Haiku 4.5 · API Usage Billing"), Some(("Haiku 4.5".into(), 200_000)));
        assert_eq!(banner_model("Opus 5.5 (200K context) · Claude Pro"), Some(("Opus 5.5".into(), 200_000)));
        // Not a banner line: no plan after a dot, or no model before it.
        assert_eq!(banner_model("Opus 5.5 (1M context)"), None);
        assert_eq!(banner_model("~/Agentty/Agentty · main"), None);
        assert_eq!(banner_model("some prose, with a · in it"), None);
    }

    #[test]
    fn the_model_changes_the_moment_model_switches_it() {
        let answer = r#"{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"cache_read_input_tokens":42000,"cache_creation_input_tokens":0}}}"#;
        let switch = r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Set model to `Opus 5.5 (1M context) (default)` and saved as your default for new sessions</local-command-stdout>"}}"#;
        // Newest first: the switch came after the last answer, so the bar names the new model at once,
        // and the conversation it carries over is still the size it was.
        let stats = super::claude_stats([switch, answer].into_iter()).unwrap();
        assert_eq!(stats.model.as_deref(), Some("Opus 5.5"));
        assert_eq!((stats.context_used, stats.context_window), (42_010, 1_000_000));
        assert_eq!(pretty_model(stats.model.as_deref().unwrap()), "Opus 5.5");

        // Once the new model has answered, the answer is the truth again.
        let later = r#"{"type":"assistant","message":{"model":"claude-opus-5-5","usage":{"input_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        assert_eq!(super::claude_stats([later, switch, answer].into_iter()).unwrap().model.as_deref(), Some("claude-opus-5-5"));

        // A model with a smaller window, named without one: its own default window.
        let haiku = r#"{"type":"user","message":{"content":"<local-command-stdout>Set model to `Haiku 4.5`</local-command-stdout>"}}"#;
        let stats = super::claude_stats([haiku, answer].into_iter()).unwrap();
        assert_eq!((stats.model.as_deref(), stats.context_window), (Some("Haiku 4.5"), 200_000));

        // `/model` keeping the model it is on says which one that is, just the same.
        let kept = r#"{"type":"user","message":{"content":"<local-command-stdout>Kept model as `Opus 5.5 (1M context) (default)`</local-command-stdout>"}}"#;
        assert_eq!(super::claude_stats([kept].into_iter()).unwrap().model.as_deref(), Some("Opus 5.5"));

        // Chosen before anything was said: named, with an empty meter.
        let fresh = super::claude_stats([switch].into_iter()).unwrap();
        assert_eq!((fresh.model.as_deref(), fresh.context_used), (Some("Opus 5.5"), 0));

        // Someone quoting the command's output is not the command.
        let quoted = r#"{"type":"user","message":{"content":"why does it say Set model to `Sonnet 5`?"}}"#;
        let said = r#"{"type":"assistant","message":{"model":"claude-opus-5","content":"Set model to `Sonnet 5`"}}"#;
        assert_eq!(super::claude_stats([quoted, said, answer].into_iter()).unwrap().model.as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn context_drops_as_soon_as_the_session_is_compacted() {
        let turn = r#"{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"cache_read_input_tokens":900000,"cache_creation_input_tokens":90}}}"#;
        let boundary = r#"{"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"manual","preTokens":900100,"postTokens":14253}}"#;
        let summary =
            r#"{"type":"user","isCompactSummary":true,"message":{"content":"the word compact_boundary in prose changes nothing"}}"#;
        // Newest first, as the tail of a transcript is read.
        let full = super::claude_stats([turn].into_iter()).unwrap();
        assert_eq!(full.context_used, 900_100);
        let compacted = super::claude_stats([summary, boundary, turn].into_iter()).unwrap();
        assert_eq!((compacted.context_used, compacted.context_window), (14_253, 1_000_000));
        assert_eq!(compacted.model.as_deref(), Some("claude-opus-5"));
        // The next prompt is the truth again, whatever the boundary below it said.
        let next = r#"{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":5,"cache_read_input_tokens":30000,"cache_creation_input_tokens":0}}}"#;
        assert_eq!(super::claude_stats([next, summary, boundary, turn].into_iter()).unwrap().context_used, 30_005);
        // A boundary from a version that wrote no `postTokens` still empties the meter.
        let old = r#"{"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"auto","preTokens":5}}"#;
        assert_eq!(super::claude_stats([old, turn].into_iter()).unwrap().context_used, 1);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A path outside every agent's session folder is refused, whatever the SessionInfo claims.
    #[test]
    fn delete_only_touches_agent_transcripts() {
        let path = std::env::temp_dir().join(format!("agentty-delete-guard-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{}").unwrap();
        let session = SessionInfo {
            agent: Agent::Claude,
            id: "x".into(),
            title: "x".into(),
            cwd: None,
            updated_at: 0,
            path: path.clone(),
            resume_args: Vec::new(),
            last_prompt: None,
            last_reply: None,
        };
        assert!(delete(&session).is_err());
        assert!(path.is_file(), "the file must still be there");
        std::fs::remove_file(&path).ok();

        // A file that is gone is reported rather than silently accepted.
        assert!(delete(&session).is_err());

        // Every agent's own folder is covered: a session file under any of them is accepted by
        // the root check (this one still fails, on the file being gone).
        for root in [amp::session_root(), agy::session_root(), kimi::session_root()] {
            let under = SessionInfo { path: root.join("whatever.json"), ..session.clone() };
            let refusal = delete(&under).unwrap_err().to_string();
            assert!(refusal.contains("session file is gone"), "{} was refused by the root check: {refusal}", root.display());
        }

        // A path that climbs out of an agent's session folder is refused too.
        let climbing = SessionInfo {
            path: fsutil::home().join(".claude").join("projects").join("..").join("..").join(".ssh").join("id_ed25519"),
            ..session
        };
        assert!(delete(&climbing).is_err());
    }
}
