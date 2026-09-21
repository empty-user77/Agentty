//! Reader for Gemini CLI chats: `~/.gemini/tmp/<project>/chats/session-*.jsonl`.
//! The first line is session metadata; then one message per line (`type`: user / gemini / …),
//! with `{"$set": …}` metadata updates and `{"$rewindTo": id}` markers. Project folders map to
//! paths in `~/.gemini/tmp/projects.json`.

use crate::fsutil::{self, one_line, truncate_chars};
use crate::model::{Agent, Role, SessionInfo, Turn};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where this agent keeps its sessions, for the guard on deleting one.
pub fn session_root() -> PathBuf {
    tmp_dir()
}

fn tmp_dir() -> PathBuf {
    fsutil::home().join(".gemini").join("tmp")
}

/// Project folder name → absolute path, from the project registry.
fn project_paths() -> HashMap<String, String> {
    let Ok(bytes) = std::fs::read(tmp_dir().join("projects.json")) else { return HashMap::new() };
    let Ok(json) = serde_json::from_slice::<Value>(&bytes) else { return HashMap::new() };
    json["projects"].as_object().into_iter().flatten().filter_map(|(path, slug)| Some((slug.as_str()?.to_string(), path.clone()))).collect()
}

fn files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(tmp_dir()) else { return out };
    for project in projects.flatten() {
        let Ok(chats) = std::fs::read_dir(project.path().join("chats")) else { continue };
        out.extend(chats.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "jsonl" || e == "json")));
    }
    out
}

static SUMMARIES: std::sync::LazyLock<fsutil::MtimeCache<Option<SessionInfo>>> = std::sync::LazyLock::new(Default::default);

pub fn list(limit: usize) -> Vec<SessionInfo> {
    let projects = project_paths();
    fsutil::newest(files(), limit.saturating_mul(2))
        .into_iter()
        .filter_map(|(path, updated_at)| {
            fsutil::cached_by_mtime(&SUMMARIES, &path, updated_at, || summarize(&path, updated_at, &projects).ok().flatten())
        })
        .take(limit)
        .collect()
}

fn text_of(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str().or(p.as_str())).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

/// (metadata, messages) with rewinds applied.
fn read(path: &Path) -> Result<(Value, Vec<Value>)> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok());
    let mut meta = lines.next().context("empty chat file")?;
    let mut messages: Vec<Value> = Vec::new();
    for line in lines {
        if let Some(set) = line.get("$set").and_then(Value::as_object) {
            for (key, value) in set {
                meta[key] = value.clone();
            }
        } else if let Some(id) = line.get("$rewindTo").and_then(Value::as_str) {
            if let Some(index) = messages.iter().position(|m| m["id"] == id) {
                messages.truncate(index);
            }
        } else if line.get("type").is_some() {
            messages.push(line);
        }
    }
    Ok((meta, messages))
}

fn summarize(path: &Path, updated_at: u64, projects: &HashMap<String, String>) -> Result<Option<SessionInfo>> {
    let (meta, messages) = read(path)?;
    if meta["kind"].as_str().is_some_and(|k| k != "main") {
        return Ok(None);
    }
    let Some(id) = meta["sessionId"].as_str().map(str::to_string) else { return Ok(None) };
    let user = |m: &&Value| m["type"] == "user";
    let Some(first) = messages.iter().filter(user).find_map(|m| text_of(&m["content"])) else { return Ok(None) };
    let project = path.parent().and_then(Path::parent).and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string());
    let cwd = meta["directories"][0].as_str().map(str::to_string).or_else(|| project.and_then(|p| projects.get(&p).cloned()));
    let title = meta["summary"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| one_line(&first, 80));
    Ok(Some(SessionInfo {
        agent: Agent::Gemini,
        resume_args: Agent::Gemini.resume_args(&id),
        title,
        cwd,
        updated_at,
        last_prompt: messages.iter().rev().filter(user).find_map(|m| text_of(&m["content"])).map(|t| one_line(&t, 160)),
        last_reply: messages.iter().rev().filter(|m| m["type"] == "gemini").find_map(|m| text_of(&m["content"])).map(|t| one_line(&t, 200)),
        path: path.to_path_buf(),
        id,
    }))
}

pub fn find(id: &str) -> Result<PathBuf> {
    files()
        .into_iter()
        .find(|p| read(p).is_ok_and(|(meta, _)| meta["sessionId"] == id))
        .with_context(|| format!("Gemini session {id} not found"))
}

pub fn transcript(path: &Path) -> Result<(Option<String>, Vec<Turn>)> {
    let (meta, messages) = read(path)?;
    let turns = messages
        .iter()
        .filter_map(|m| {
            let role = match m["type"].as_str()? {
                "user" => Role::User,
                "gemini" => Role::Assistant,
                _ => return None,
            };
            Some(Turn { role, text: truncate_chars(&text_of(&m["content"])?, 8_000) })
        })
        .collect();
    Ok((meta["directories"][0].as_str().map(str::to_string), turns))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_updates_and_rewinds() {
        let dir = std::env::temp_dir().join(format!("agentty-gemini-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("session.jsonl");
        std::fs::write(
            &file,
            [
                r#"{"sessionId":"s1","kind":"main","directories":["/work/app"]}"#,
                r#"{"id":"m1","type":"user","content":[{"text":"hi"}]}"#,
                r#"{"id":"m2","type":"gemini","content":[{"text":"hello"}]}"#,
                r#"{"id":"m3","type":"user","content":"undo me"}"#,
                r#"{"$rewindTo":"m3"}"#,
                r#"{"$set":{"summary":"Greeting"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let info = summarize(&file, 1, &HashMap::new()).unwrap().unwrap();
        assert_eq!((info.title.as_str(), info.cwd.as_deref(), info.last_prompt.as_deref()), ("Greeting", Some("/work/app"), Some("hi")));
        assert_eq!(transcript(&file).unwrap().1.len(), 2);
        std::fs::remove_dir_all(dir).ok();
    }
}
