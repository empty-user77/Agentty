//! Reader for Codex rollouts: `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`.

use crate::fsutil::{self, one_line, truncate_chars};
use crate::model::{Agent, Role, SessionInfo, Turn};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Where this agent keeps its sessions, for the guard on deleting one.
pub fn session_root() -> PathBuf {
    codex_dir().join("sessions")
}

fn codex_dir() -> PathBuf {
    fsutil::home().join(".codex")
}

fn rollouts() -> Vec<PathBuf> {
    let mut files = Vec::new();
    fsutil::jsonl_files(&codex_dir().join("sessions"), 4, &mut files);
    // Agentty's private Codex home (API-key / auth.json sign-in) when its sessions aren't linked here.
    for private in crate::agent_auth::codex_home_sessions() {
        fsutil::jsonl_files(&private, 4, &mut files);
    }
    files.retain(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-")));
    files
}

/// Thread names Codex generates, keyed by session id (later entries win).
fn thread_names() -> HashMap<String, String> {
    let Ok(file) = File::open(codex_dir().join("session_index.jsonl")) else { return HashMap::new() };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .filter_map(|v| Some((v["id"].as_str()?.to_string(), v["thread_name"].as_str()?.to_string())))
        .collect()
}

pub fn list(limit: usize) -> Vec<SessionInfo> {
    let names = thread_names();
    fsutil::newest(rollouts(), limit.saturating_mul(2))
        .into_iter()
        .filter_map(|(path, updated_at)| {
            fsutil::cached_by_mtime(&SUMMARIES, &path, updated_at, || summarize(&path, updated_at, &names).ok().flatten())
        })
        .take(limit)
        .collect()
}

static SUMMARIES: std::sync::LazyLock<fsutil::MtimeCache<Option<SessionInfo>>> = std::sync::LazyLock::new(Default::default);

fn summarize(path: &Path, updated_at: u64, names: &HashMap<String, String>) -> Result<Option<SessionInfo>> {
    let reader = BufReader::new(File::open(path)?);
    let mut id = None;
    let mut cwd = None;
    let mut first_prompt = None;

    for line in reader.lines() {
        let line = line?;
        if id.is_some() && first_prompt.is_some() {
            break;
        }
        let is_meta = id.is_none() && line.contains("\"type\":\"session_meta\"");
        let is_message = first_prompt.is_none() && line.contains("\"role\":\"user\"");
        if !(is_meta || is_message) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if is_meta {
            id = v["payload"]["id"].as_str().map(str::to_string);
            cwd = v["payload"]["cwd"].as_str().map(str::to_string);
        } else if let Some(turn) = message_turn(&v["payload"]) {
            first_prompt = Some(turn.text);
        }
    }

    let (Some(id), Some(prompt)) = (id, first_prompt) else { return Ok(None) };
    let title = names.get(&id).cloned().unwrap_or_else(|| one_line(&prompt, 80));
    let (last_prompt, last_reply) = last_exchange(path);
    Ok(Some(SessionInfo {
        last_prompt: last_prompt.or_else(|| Some(one_line(&prompt, 160))),
        last_reply,
        agent: Agent::Codex,
        resume_args: Agent::Codex.resume_args(&id),
        id,
        title,
        cwd,
        updated_at,
        path: path.to_path_buf(),
    }))
}

/// Latest user prompt and assistant reply, read from the end of the rollout.
pub fn last_exchange(path: &Path) -> (Option<String>, Option<String>) {
    let (mut prompt, mut reply) = (None, None);
    for line in fsutil::tail_lines_rev(path, 384 * 1024) {
        if prompt.is_some() && reply.is_some() {
            break;
        }
        if !line.contains("\"type\":\"message\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match message_turn(&v["payload"]) {
            Some(Turn { role: Role::User, text }) if prompt.is_none() => prompt = Some(one_line(&text, 160)),
            Some(Turn { role: Role::Assistant, text }) if reply.is_none() => reply = Some(one_line(&text, 200)),
            _ => {}
        }
    }
    (prompt, reply)
}

/// Distinct models used by the newest `limit` sessions, most recent first (from `turn_context`).
pub fn recent_models(limit: usize) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    for (path, _) in fsutil::newest(rollouts(), limit) {
        let Ok(file) = File::open(&path) else { continue };
        let found = BufReader::new(file).lines().map_while(Result::ok).take(400).find_map(|line| {
            if !line.contains("\"type\":\"turn_context\"") {
                return None;
            }
            let v: Value = serde_json::from_str(&line).ok()?;
            v["payload"]["model"].as_str().map(str::to_string)
        });
        if let Some(model) = found.filter(|m| !models.contains(m)) {
            models.push(model);
        }
    }
    models
}

pub fn find(id: &str) -> Result<PathBuf> {
    let suffix = format!("{id}.jsonl");
    rollouts().into_iter().find(|p| p.to_string_lossy().ends_with(&suffix)).with_context(|| format!("Codex session {id} not found"))
}

pub fn transcript(path: &Path) -> Result<(Option<String>, Vec<Turn>)> {
    let reader = BufReader::new(File::open(path)?);
    let mut cwd = None;
    let mut turns: Vec<Turn> = Vec::new();

    for line in reader.lines() {
        let Ok(v) = serde_json::from_str::<Value>(&line?) else { continue };
        let payload = &v["payload"];
        match v["type"].as_str() {
            Some("session_meta") => cwd = payload["cwd"].as_str().map(str::to_string),
            Some("response_item") => {
                let turn = match payload["type"].as_str() {
                    Some("message") => message_turn(payload),
                    Some("function_call" | "custom_tool_call") => {
                        let name = payload["name"].as_str().unwrap_or("tool");
                        let args = payload["arguments"].as_str().or(payload["input"].as_str()).unwrap_or("");
                        Some(Turn { role: Role::Assistant, text: format!("[tool: {name} {}]", one_line(args, 160)) })
                    }
                    _ => None,
                };
                let Some(turn) = turn else { continue };
                match turns.last_mut() {
                    Some(last) if last.role == Role::Assistant && turn.role == Role::Assistant => {
                        last.text.push('\n');
                        last.text.push_str(&turn.text);
                    }
                    _ => turns.push(turn),
                }
            }
            _ => {}
        }
    }
    Ok((cwd, turns))
}

/// User/assistant message text. Developer prompts and injected context blocks are excluded.
fn message_turn(payload: &Value) -> Option<Turn> {
    let role = match payload["role"].as_str()? {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return None,
    };
    let text = payload["content"].as_array()?.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n");
    let text = text.trim();
    if text.is_empty() || (role == Role::User && (text.starts_with('<') || text.starts_with("# AGENTS.md"))) {
        return None;
    }
    Some(Turn { role, text: truncate_chars(text, 8_000) })
}

/// Id of the newest Codex session started in `cwd` whose rollout was written after `since_ms`.
/// Used to find the session behind a Codex pane (Codex cannot be told its session id up front).
pub fn find_recent(cwd: &Path, since_ms: u64) -> Option<String> {
    let cwd = cwd.to_string_lossy();
    fsutil::newest(rollouts(), usize::MAX).into_iter().take_while(|(_, mtime)| *mtime + 1_000 >= since_ms).find_map(|(path, _)| {
        let first = BufReader::new(File::open(&path).ok()?).lines().next()?.ok()?;
        let v: Value = serde_json::from_str(&first).ok()?;
        let meta = &v["payload"];
        (meta["cwd"].as_str() == Some(&*cwd)).then(|| meta["id"].as_str().map(str::to_string)).flatten()
    })
}
