//! Plan rate limits (5-hour and weekly windows) with their reset times, for the menu bar.
//! Claude Code reports them to statusline commands (saved by `agentty statusline`); Codex writes
//! them into its rollouts.

use crate::fsutil;
use crate::model::Agent;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    pub used_percent: f64,
    /// Unix epoch seconds.
    pub resets_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentLimits {
    /// 5-hour session window.
    pub session: Option<LimitWindow>,
    pub weekly: Option<LimitWindow>,
    /// When this was observed (epoch ms).
    pub captured_ms: u64,
}

fn window(value: &Value, percent_key: &str) -> Option<LimitWindow> {
    let used_percent = value[percent_key].as_f64()?;
    let resets_at = value["resets_at"].as_i64().or_else(|| value["resets_at"].as_f64().map(|v| v as i64))?;
    Some(LimitWindow { used_percent, resets_at })
}

fn claude_file() -> PathBuf {
    fsutil::data_dir().join("limits").join("claude.json")
}

/// `rate_limits` from Claude Code's statusline input.
pub fn parse_claude(rate_limits: &Value, now_ms: u64) -> Option<AgentLimits> {
    let limits = AgentLimits {
        session: window(&rate_limits["five_hour"], "used_percentage"),
        weekly: window(&rate_limits["seven_day"], "used_percentage"),
        captured_ms: now_ms,
    };
    (limits.session.is_some() || limits.weekly.is_some()).then_some(limits)
}

pub fn save_claude(limits: &AgentLimits) {
    let path = claude_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(limits) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

/// `payload.rate_limits` of a Codex `token_count` event: windows are told apart by their length.
pub fn parse_codex(rate_limits: &Value, now_ms: u64) -> Option<AgentLimits> {
    let mut limits = AgentLimits { captured_ms: now_ms, ..Default::default() };
    for key in ["primary", "secondary"] {
        let entry = &rate_limits[key];
        let Some(found) = window(entry, "used_percent") else { continue };
        match entry["window_minutes"].as_u64() {
            Some(minutes) if minutes <= 24 * 60 => limits.session = Some(found),
            _ => limits.weekly = Some(found),
        }
    }
    (limits.session.is_some() || limits.weekly.is_some()).then_some(limits)
}

/// Latest known limits of an agent; windows that already reset are dropped.
pub fn latest(agent: Agent, now_ms: u64) -> Option<AgentLimits> {
    let mut limits = match agent {
        Agent::Claude => serde_json::from_slice::<AgentLimits>(&std::fs::read(claude_file()).ok()?).ok()?,
        Agent::Codex => codex_latest()?,
        _ => return None,
    };
    let now = (now_ms / 1000) as i64;
    limits.session = limits.session.filter(|w| w.resets_at > now);
    limits.weekly = limits.weekly.filter(|w| w.resets_at > now);
    Some(limits)
}

fn codex_latest() -> Option<AgentLimits> {
    let mut files = Vec::new();
    fsutil::jsonl_files(&fsutil::home().join(".codex").join("sessions"), 4, &mut files);
    for (path, mtime) in fsutil::newest(files, 5) {
        for line in fsutil::tail_lines_rev(&path, 512 * 1024) {
            if !line.contains("\"rate_limits\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(limits) = parse_codex(&v["payload"]["rate_limits"], mtime) {
                return Some(limits);
            }
        }
    }
    None
}

/// (days, hours, minutes) until `resets_at`.
pub fn time_left(resets_at: i64, now_ms: u64) -> (i64, i64, i64) {
    let seconds = (resets_at - (now_ms / 1000) as i64).max(0);
    (seconds / 86_400, seconds % 86_400 / 3_600, seconds % 3_600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_and_codex_limits() {
        let claude = serde_json::json!({ "five_hour": { "used_percentage": 12.5, "resets_at": 2000 }, "seven_day": { "used_percentage": 40, "resets_at": 9000 } });
        let parsed = parse_claude(&claude, 5).unwrap();
        assert_eq!(parsed.session, Some(LimitWindow { used_percent: 12.5, resets_at: 2000 }));
        assert_eq!(parsed.weekly.map(|w| w.used_percent), Some(40.0));
        assert!(parse_claude(&serde_json::json!({}), 5).is_none());

        let codex =
            serde_json::json!({ "primary": { "used_percent": 89.0, "window_minutes": 10080, "resets_at": 1789805418 }, "secondary": null });
        let parsed = parse_codex(&codex, 5).unwrap();
        assert!(parsed.session.is_none());
        assert_eq!(parsed.weekly.map(|w| w.resets_at), Some(1789805418));
    }

    #[test]
    fn countdown() {
        assert_eq!(time_left(86_400 * 2 + 3_600 * 5 + 60 * 7, 0), (2, 5, 7));
        assert_eq!(time_left(10, 50_000), (0, 0, 0));
    }
}
