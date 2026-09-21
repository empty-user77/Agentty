//! Reader for Amp threads: `~/.local/share/amp/threads/T-<id>.json`.

use crate::fsutil::{self, one_line, truncate_chars};
use crate::model::{Agent, Role, SessionInfo, Turn};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Where this agent keeps its sessions, for the guard on deleting one.
pub fn session_root() -> PathBuf {
    threads_dir()
}

fn threads_dir() -> PathBuf {
    fsutil::home().join(".local").join("share").join("amp").join("threads")
}

fn files() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(threads_dir()) else { return Vec::new() };
    entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect()
}

static SUMMARIES: std::sync::LazyLock<fsutil::MtimeCache<Option<SessionInfo>>> = std::sync::LazyLock::new(Default::default);

pub fn list(limit: usize) -> Vec<SessionInfo> {
    fsutil::newest(files(), limit)
        .into_iter()
        .filter_map(|(path, updated_at)| {
            fsutil::cached_by_mtime(&SUMMARIES, &path, updated_at, || summarize(&path, updated_at).ok().flatten())
        })
        .collect()
}

fn message_text(message: &Value) -> Option<String> {
    let text: Vec<&str> =
        message["content"].as_array()?.iter().filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str()).collect();
    let joined = text.join("\n");
    (!joined.trim().is_empty()).then_some(joined)
}

fn summarize(path: &Path, updated_at: u64) -> Result<Option<SessionInfo>> {
    let thread: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let Some(id) = thread["id"].as_str().map(str::to_string) else { return Ok(None) };
    let messages = thread["messages"].as_array().cloned().unwrap_or_default();
    let first_prompt = messages.iter().filter(|m| m["role"] == "user").find_map(message_text);
    let Some(first) = first_prompt else { return Ok(None) };
    let last_prompt = messages.iter().rev().filter(|m| m["role"] == "user").find_map(message_text);
    let last_reply = messages.iter().rev().filter(|m| m["role"] == "assistant").find_map(message_text);
    let title = thread["title"].as_str().map(str::to_string).unwrap_or_else(|| one_line(&first, 80));
    Ok(Some(SessionInfo {
        agent: Agent::Amp,
        resume_args: Agent::Amp.resume_args(&id),
        title,
        cwd: cwd(&thread),
        updated_at,
        last_prompt: last_prompt.map(|t| one_line(&t, 160)),
        last_reply: last_reply.map(|t| one_line(&t, 200)),
        path: path.to_path_buf(),
        id,
    }))
}

fn cwd(thread: &Value) -> Option<String> {
    let uri = thread["env"]["initial"]["trees"].as_array()?.first()?["uri"].as_str()?;
    url::Url::parse(uri).ok()?.to_file_path().ok().map(|p| p.display().to_string())
}

pub fn find(id: &str) -> Result<PathBuf> {
    let path = threads_dir().join(format!("{id}.json"));
    anyhow::ensure!(path.is_file(), "Amp thread {id} not found");
    Ok(path)
}

pub fn transcript(path: &Path) -> Result<(Option<String>, Vec<Turn>)> {
    let thread: Value = serde_json::from_slice(&std::fs::read(path).context("thread not readable")?)?;
    let mut turns = Vec::new();
    for message in thread["messages"].as_array().into_iter().flatten() {
        let role = if message["role"] == "user" { Role::User } else { Role::Assistant };
        let mut parts = Vec::new();
        for block in message["content"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => parts.extend(block["text"].as_str().map(str::to_string)),
                Some("tool_use") => {
                    let name = block["name"].as_str().unwrap_or("tool");
                    let hint = ["cmd", "command", "path", "pattern", "url"]
                        .iter()
                        .find_map(|k| block["input"][*k].as_str())
                        .map(|s| format!(" {}", one_line(s, 160)))
                        .unwrap_or_default();
                    parts.push(format!("[tool: {name}{hint}]"));
                }
                _ => {}
            }
        }
        if parts.is_empty() {
            continue;
        }
        let text = truncate_chars(&parts.join("\n"), 8_000);
        match turns.last_mut() {
            Some(Turn { role: last, text: previous }) if *last == role => {
                previous.push('\n');
                previous.push_str(&text);
            }
            _ => turns.push(Turn { role, text }),
        }
    }
    Ok((cwd(&thread), turns))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_thread_fields() {
        let (uri, path) =
            if cfg!(windows) { ("file:///C:/Users/me/app", r"C:\Users\me\app") } else { ("file:///Users/me/app", "/Users/me/app") };
        let thread = serde_json::json!({ "env": { "initial": { "trees": [{ "uri": uri }] } } });
        assert_eq!(cwd(&thread).as_deref(), Some(path));
        let message = serde_json::json!({ "content": [{ "type": "thinking" }, { "type": "text", "text": "hello" }] });
        assert_eq!(message_text(&message).as_deref(), Some("hello"));
    }
}
