//! Reader for Antigravity CLI (`agy`) conversations: `~/.gemini/antigravity-cli/`.
//! Summaries live in `conversation_summaries.db` (SQLite), steps in `conversations/<id>.db`
//! as protobuf blobs. SQLite is read through the `sqlite3` CLI (macOS's `/usr/bin/sqlite3`).

use crate::fsutil::{self, one_line};
use crate::model::{Agent, Role, SessionInfo, Turn};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    fsutil::home().join(".gemini").join("antigravity-cli")
}

fn sqlite_json(db: &Path, query: &str) -> Result<Vec<Value>> {
    // macOS ships sqlite3 in /usr/bin; elsewhere it is looked up on PATH.
    let program = if cfg!(target_os = "macos") { "/usr/bin/sqlite3" } else { "sqlite3" };
    let output =
        crate::process::command(program).args(["-readonly", "-json"]).arg(db).arg(query).output().context("sqlite3 is not available")?;
    anyhow::ensure!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr).trim());
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&text)?)
}

/// `2026-09-17 10:49:08.21223+00:00` → epoch ms.
fn parse_time(text: &str) -> u64 {
    chrono::DateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f%:z").map(|t| t.timestamp_millis().max(0) as u64).unwrap_or(0)
}

/// First `file://` workspace of a conversation as a path.
fn workspace(uris: &str) -> Option<String> {
    let list: Vec<String> = serde_json::from_str(uris).ok()?;
    let url = url::Url::parse(list.first()?).ok()?;
    url.to_file_path().ok().map(|p| p.display().to_string())
}

/// Latest typed prompt per conversation, from `history.jsonl`.
fn last_prompts() -> std::collections::HashMap<String, String> {
    let mut prompts = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(root().join("history.jsonl")) else { return prompts };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let (Some(id), Some(display)) = (v["conversationId"].as_str(), v["display"].as_str()) else { continue };
        if v["type"].as_str() != Some("slash_command") && !display.trim().is_empty() {
            prompts.insert(id.to_string(), one_line(display, 160));
        }
    }
    prompts
}

pub fn list(limit: usize) -> Vec<SessionInfo> {
    let db = root().join("conversation_summaries.db");
    if !db.is_file() {
        return Vec::new();
    }
    let query = format!(
        "select conversation_id, title, preview, last_modified_time, workspace_uris from conversation_summaries \
         where parent_conversation_id = '' order by last_modified_time desc limit {limit}"
    );
    let rows = sqlite_json(&db, &query).unwrap_or_default();
    let prompts = last_prompts();
    rows.iter()
        .filter_map(|row| {
            let id = row["conversation_id"].as_str()?.to_string();
            let path = root().join("conversations").join(format!("{id}.db"));
            let title = row["title"].as_str().filter(|t| !t.is_empty()).or(row["preview"].as_str()).unwrap_or("Antigravity");
            Some(SessionInfo {
                agent: Agent::Agy,
                resume_args: Agent::Agy.resume_args(&id),
                title: one_line(title, 80),
                cwd: row["workspace_uris"].as_str().and_then(workspace),
                updated_at: row["last_modified_time"].as_str().map(parse_time).unwrap_or(0),
                last_prompt: prompts.get(&id).cloned(),
                last_reply: None,
                path,
                id,
            })
        })
        .collect()
}

pub fn find(id: &str) -> Result<PathBuf> {
    let path = root().join("conversations").join(format!("{id}.db"));
    anyhow::ensure!(path.is_file(), "Antigravity conversation {id} not found");
    Ok(path)
}

/// Step types seen in conversation databases.
const USER_INPUT: i64 = 14;
const RESPONSE: i64 = 15;

pub fn transcript(path: &Path) -> Result<(Option<String>, Vec<Turn>)> {
    let rows = sqlite_json(path, "select step_type, hex(step_payload) as payload from steps order by idx")?;
    let mut cwd = None;
    let mut turns = Vec::new();
    for row in rows {
        let Some(bytes) = row["payload"].as_str().and_then(hex) else { continue };
        let fields = crate::protobuf::strings(&bytes);
        if cwd.is_none() {
            cwd = fields.iter().find_map(|f| f.text.strip_prefix("file://")).map(str::to_string);
        }
        let (role, number) = match row["step_type"].as_i64() {
            Some(USER_INPUT) => (Role::User, 2),
            Some(RESPONSE) => (Role::Assistant, 1),
            _ => continue,
        };
        if let Some(field) = fields.iter().find(|f| f.depth == 1 && f.number == number && !f.text.trim().is_empty()) {
            turns.push(Turn { role, text: field.text.clone() });
        }
    }
    Ok((cwd, turns))
}

fn hex(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times_and_workspaces() {
        assert_eq!(parse_time("2026-09-17 10:49:08.21223+00:00"), 1_789_642_148_212);
        // File URLs name a drive on Windows.
        if cfg!(windows) {
            assert_eq!(workspace(r#"["file:///C:/Users/me/my%20app"]"#).as_deref(), Some(r"C:\Users\me\my app"));
        } else {
            assert_eq!(workspace(r#"["file:///Users/me/my%20app"]"#).as_deref(), Some("/Users/me/my app"));
        }
        assert_eq!(hex("0a01"), Some(vec![0x0a, 0x01]));
    }
}
