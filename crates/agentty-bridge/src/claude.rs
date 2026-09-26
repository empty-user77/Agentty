//! Reader for Claude Code transcripts: `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`.

use crate::fsutil::{self, one_line, truncate_chars};
use crate::model::{Agent, Role, SessionInfo, Turn};
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Where this agent keeps its sessions, for the guard on deleting one.
pub fn session_root() -> PathBuf {
    projects_dir()
}

fn projects_dir() -> PathBuf {
    fsutil::home().join(".claude").join("projects")
}

pub fn list(limit: usize) -> Vec<SessionInfo> {
    let mut files = Vec::new();
    // Only top-level session files; subagent transcripts live in nested directories.
    fsutil::jsonl_files(&projects_dir(), 1, &mut files);
    files.retain(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("agent-")));

    fsutil::newest(files, limit.saturating_mul(2))
        .into_iter()
        .filter_map(|(path, updated_at)| {
            fsutil::cached_by_mtime(&SUMMARIES, &path, updated_at, || summarize(&path, updated_at).ok().flatten())
        })
        .take(limit)
        .collect()
}

static SUMMARIES: std::sync::LazyLock<fsutil::MtimeCache<Option<SessionInfo>>> = std::sync::LazyLock::new(Default::default);

fn summarize(path: &Path, updated_at: u64) -> Result<Option<SessionInfo>> {
    let id = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let reader = BufReader::new(File::open(path)?);
    let mut cwd = None;
    let mut ai_title = None;
    let mut first_prompt = None;

    for line in reader.lines() {
        let line = line?;
        let is_title = line.contains("\"type\":\"ai-title\"");
        let is_user = first_prompt.is_none() && line.contains("\"type\":\"user\"");
        let wants_cwd = cwd.is_none() && line.contains("\"cwd\":");
        if !(is_title || is_user || wants_cwd) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(str::to_string);
        }
        if is_title {
            ai_title = v["aiTitle"].as_str().map(str::to_string);
        }
        if is_user {
            first_prompt = user_text(&v);
        }
    }

    // Sessions without a real prompt are noise (e.g. aborted launches).
    let Some(prompt) = first_prompt else { return Ok(None) };
    let title = ai_title.unwrap_or_else(|| one_line(&prompt, 80));
    let (last_prompt, last_reply) = last_exchange(path);
    Ok(Some(SessionInfo {
        last_prompt: last_prompt.or_else(|| Some(one_line(&prompt, 160))),
        last_reply,
        agent: Agent::Claude,
        resume_args: Agent::Claude.resume_args(&id),
        id,
        title,
        cwd,
        updated_at,
        path: path.to_path_buf(),
    }))
}

/// Latest user prompt and assistant text reply, read from the end of the transcript.
pub fn last_exchange(path: &Path) -> (Option<String>, Option<String>) {
    let (mut prompt, mut reply) = (None, None);
    // Long tool-heavy sessions push the last human prompt far back; widen the window once.
    for window in [384 * 1024, 6 * 1024 * 1024] {
        if prompt.is_some() && reply.is_some() {
            break;
        }
        scan_exchange(path, window, &mut prompt, &mut reply);
    }
    (prompt, reply)
}

fn scan_exchange(path: &Path, window: u64, prompt: &mut Option<String>, reply: &mut Option<String>) {
    for line in fsutil::tail_lines_rev(path, window) {
        if prompt.is_some() && reply.is_some() {
            break;
        }
        let user = prompt.is_none() && line.contains("\"type\":\"user\"");
        let assistant = reply.is_none() && line.contains("\"type\":\"assistant\"") && line.contains("\"type\":\"text\"");
        if !(user || assistant) || line.contains("\"isSidechain\":true") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if user {
            *prompt = user_text(&v).filter(|t| !t.starts_with('<')).map(|t| one_line(&t, 160));
        } else if let Some(blocks) = v["message"]["content"].as_array() {
            let text: Vec<&str> = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
            *reply = Some(one_line(&text.join(" "), 200)).filter(|t| !t.is_empty());
        }
    }
}

/// What the agent said last, for the notification that it is done: the first line of its latest
/// reply, without Markdown markers. Claude Code's `Stop` hook says nothing about the answer.
pub fn last_reply_headline(path: &Path) -> Option<String> {
    for line in fsutil::tail_lines_rev(path, 384 * 1024) {
        if !(line.contains("\"type\":\"assistant\"") && line.contains("\"type\":\"text\"")) || line.contains("\"isSidechain\":true") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(blocks) = v["message"]["content"].as_array() else { continue };
        let text: Vec<&str> = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
        if let Some(headline) = headline(&text.join("\n")) {
            return Some(headline);
        }
    }
    None
}

/// The first line that says something: headings, list and quote markers, emphasis and code ticks
/// are dropped, and a line of nothing but those (`---`, "```") is skipped.
fn headline(text: &str) -> Option<String> {
    text.lines()
        .map(|line| {
            let line = line.trim().trim_start_matches(['#', '>', '-', '*', '+', ' ']).trim();
            line.replace("**", "").replace("__", "").replace('`', "")
        })
        .map(|line| one_line(&line, 140))
        .find(|line| line.chars().any(char::is_alphanumeric))
}

/// Distinct models (`claude-…` ids) used by the newest `limit` sessions, most recent first.
pub fn recent_models(limit: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut files = Vec::new();
    fsutil::jsonl_files(&projects_dir(), 1, &mut files);
    let mut models: Vec<String> = Vec::new();
    for (path, _) in fsutil::newest(files, limit) {
        let Ok(mut file) = std::fs::File::open(&path) else { continue };
        // The model a session is on now is its latest answer's, not the first one's: a session
        // started on one model and moved to another with `/model` would otherwise keep naming the
        // old one — and the newest session is the one the next pane is most likely to resemble.
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if file.seek(SeekFrom::Start(len.saturating_sub(256 * 1024))).is_err() {
            continue;
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            continue;
        }
        if let Some(model) = last_model(&String::from_utf8_lossy(&bytes)).filter(|m| !models.contains(m)) {
            models.push(model);
        }
    }
    models
}

/// The model of the latest answer in some transcript text.
fn last_model(text: &str) -> Option<String> {
    text.lines().rev().filter(|line| line.contains("\"model\":\"claude-")).find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        v["message"]["model"].as_str().filter(|m| m.starts_with("claude-")).map(str::to_string)
    })
}

/// The default model from Claude Code's user settings (`model` in settings.json), if set.
pub fn configured_model() -> Option<String> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| fsutil::home().join(".claude"));
    let text = std::fs::read_to_string(dir.join("settings.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json["model"].as_str().map(str::to_string).filter(|m| !m.trim().is_empty())
}

pub fn find(id: &str) -> Result<PathBuf> {
    let file = format!("{id}.jsonl");
    std::fs::read_dir(projects_dir())?
        .flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())
        .with_context(|| format!("Claude session {id} not found"))
}

pub fn transcript(path: &Path) -> Result<(Option<String>, Vec<Turn>)> {
    read_turns(path, false)
}

/// Turns of a subagent transcript (all of its lines are sidechain lines).
pub fn subagent_transcript(path: &Path) -> Result<Vec<Turn>> {
    read_turns(path, true).map(|(_, turns)| turns)
}

fn read_turns(path: &Path, sidechain: bool) -> Result<(Option<String>, Vec<Turn>)> {
    let reader = BufReader::new(File::open(path)?);
    let mut cwd = None;
    let mut turns = Vec::new();

    for line in reader.lines() {
        let Ok(v) = serde_json::from_str::<Value>(&line?) else { continue };
        if (!sidechain && v["isSidechain"].as_bool() == Some(true)) || v["isMeta"].as_bool() == Some(true) {
            continue;
        }
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(str::to_string);
        }
        let turn = match v["type"].as_str() {
            Some("user") => user_text(&v).map(|text| Turn { role: Role::User, text }),
            Some("assistant") => assistant_text(&v).map(|text| Turn { role: Role::Assistant, text }),
            _ => None,
        };
        if let Some(turn) = turn {
            // Streaming writes one line per content block; merge consecutive assistant blocks.
            match turns.last_mut() {
                Some(Turn { role: Role::Assistant, text }) if turn.role == Role::Assistant => {
                    text.push('\n');
                    text.push_str(&turn.text);
                }
                _ => turns.push(turn),
            }
        }
    }
    Ok((cwd, turns))
}

/// Text the human typed. Tool results and harness-injected wrappers are excluded.
fn user_text(v: &Value) -> Option<String> {
    let content = &v["message"]["content"];
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| match b["type"].as_str() {
                Some("text") => b["text"].as_str().map(str::to_string),
                Some("image") => Some("[image]".to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    let injected = ["<command-", "<local-command", "<system-reminder>", "Caveat:", "[Request interrupted"];
    if text.is_empty() || injected.iter().any(|p| text.starts_with(p)) {
        return None;
    }
    Some(text.to_string())
}

fn assistant_text(v: &Value) -> Option<String> {
    let blocks = v["message"]["content"].as_array()?;
    let parts: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b["type"].as_str() {
            Some("text") => b["text"].as_str().map(|t| t.trim().to_string()),
            Some("tool_use") => {
                let name = b["name"].as_str().unwrap_or("tool");
                let input = &b["input"];
                let hint = ["command", "file_path", "pattern", "description", "url"]
                    .iter()
                    .find_map(|k| input[*k].as_str())
                    .map(|s| format!(" {}", one_line(s, 160)))
                    .unwrap_or_default();
                Some(format!("[tool: {name}{hint}]"))
            }
            _ => None,
        })
        .filter(|s| !s.is_empty())
        .collect();
    (!parts.is_empty()).then(|| truncate_chars(&parts.join("\n"), 8_000))
}

/// A subagent run by a session (Task / Agent tool), from `<session>/subagents/agent-<id>.jsonl`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Subagent {
    pub id: String,
    pub agent_type: String,
    pub description: String,
    pub model: Option<String>,
    pub path: PathBuf,
    /// Last write (epoch ms) and first line time, for durations.
    pub updated_ms: u64,
    /// The transcript ends with a final answer (`end_turn`).
    pub finished: bool,
    pub turns: usize,
    /// Most recent tool call: "Bash · cargo test".
    pub last_tool: Option<String>,
}

/// (subagent transcripts, those written in the last 20 s) — a cheap check for the status bar.
pub fn subagent_activity(session_id: &str) -> (usize, usize) {
    let Ok(transcript) = find(session_id) else { return (0, 0) };
    let Ok(entries) = std::fs::read_dir(transcript.with_extension("").join("subagents")) else { return (0, 0) };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let files: Vec<u64> =
        entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "jsonl")).map(|p| fsutil::mtime_ms(&p)).collect();
    (files.len(), files.iter().filter(|m| now.saturating_sub(**m) < 20_000).count())
}

/// Subagents of a session, oldest first.
pub fn subagents(session_id: &str) -> Vec<Subagent> {
    let Ok(transcript) = find(session_id) else { return Vec::new() };
    let dir = transcript.with_extension("").join("subagents");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut found: Vec<Subagent> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter_map(|path| {
            let id = path.file_stem()?.to_string_lossy().trim_start_matches("agent-").to_string();
            let meta: Value =
                std::fs::read(path.with_extension("meta.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            let tail = fsutil::tail_lines_rev(&path, 256 * 1024);
            let finished =
                tail.iter().find(|l| l.contains("\"type\":\"assistant\"")).is_some_and(|l| l.contains("\"stop_reason\":\"end_turn\""));
            let last_tool = tail.iter().filter(|l| l.contains("\"tool_use\"")).find_map(|l| {
                let v: Value = serde_json::from_str(l).ok()?;
                let block = v["message"]["content"].as_array()?.iter().rev().find(|b| b["type"] == "tool_use")?.clone();
                let name = block["name"].as_str()?.to_string();
                let hint = ["description", "command", "file_path", "pattern", "url"]
                    .iter()
                    .find_map(|k| block["input"][*k].as_str())
                    .map(|s| one_line(s, 80));
                Some(match hint {
                    Some(hint) => format!("{name} · {hint}"),
                    None => name,
                })
            });
            let turns = tail.iter().filter(|l| l.contains("\"type\":\"assistant\"")).count();
            Some(Subagent {
                id,
                agent_type: meta["agentType"].as_str().unwrap_or("agent").to_string(),
                description: meta["description"].as_str().unwrap_or_default().to_string(),
                model: meta["model"].as_str().map(str::to_string),
                updated_ms: fsutil::mtime_ms(&path),
                path,
                finished,
                turns,
                last_tool,
            })
        })
        .collect();
    found.sort_by_key(|s| s.path.metadata().and_then(|m| m.created()).ok());
    found
}

pub fn exists(id: &str) -> bool {
    find(id).is_ok()
}

/// Claude Code stores transcripts under a directory named after the working directory with
/// every character other than ASCII letters and digits replaced by `-`.
pub fn project_dir_for(cwd: &Path) -> PathBuf {
    let encoded: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    projects_dir().join(encoded)
}

/// Id of the newest session in `cwd` written after `since_ms` (for agents started by hand).
pub fn find_recent(cwd: &Path, since_ms: u64) -> Option<String> {
    let mut files = Vec::new();
    fsutil::jsonl_files(&project_dir_for(cwd), 0, &mut files);
    fsutil::newest(files, 1)
        .into_iter()
        .find(|(_, mtime)| *mtime + 1_000 >= since_ms)
        .and_then(|(path, _)| path.file_stem().map(|s| s.to_string_lossy().to_string()))
}

/// A running Claude Code session as it registers itself in `~/.claude/sessions/<pid>.json`.
/// Sessions message each other by `name` (the `SendMessage` / `ListAgents` tools), so Agentty can
/// tell two of its own sessions how to reach one another.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerSession {
    /// The address other sessions use (`agentty-be`, `agentty-web-2f`, …).
    pub name: String,
    pub session_id: String,
    pub pid: u32,
    pub cwd: PathBuf,
    /// `idle`, `busy`, … as the session last reported it.
    pub status: String,
    /// Interactive sessions only; background and print runs are not worth messaging.
    pub interactive: bool,
}

fn sessions_dir() -> PathBuf {
    fsutil::home().join(".claude").join("sessions")
}

/// Claude Code sessions registered on this machine. Read from disk at most once a second: the UI
/// asks for this while it repaints.
pub fn peer_sessions() -> Vec<PeerSession> {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(std::time::Instant, Vec<PeerSession>)>> = Mutex::new(None);
    let fresh = |cache: &Option<(std::time::Instant, Vec<PeerSession>)>| {
        cache.as_ref().filter(|(at, _)| at.elapsed() < std::time::Duration::from_secs(1)).map(|(_, peers)| peers.clone())
    };
    if let Some(peers) = CACHE.lock().ok().as_deref().and_then(fresh) {
        return peers;
    }
    let peers = read_peer_sessions();
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((std::time::Instant::now(), peers.clone()));
    }
    peers
}

fn read_peer_sessions() -> Vec<PeerSession> {
    let Ok(entries) = std::fs::read_dir(sessions_dir()) else { return Vec::new() };
    let mut peers: Vec<PeerSession> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|entry| {
            let value: Value = serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()?;
            let text = |key: &str| value[key].as_str().map(str::to_string);
            Some(PeerSession {
                name: text("name").filter(|n| !n.is_empty())?,
                session_id: text("sessionId")?,
                pid: value["pid"].as_u64()? as u32,
                cwd: PathBuf::from(text("cwd").unwrap_or_default()),
                status: text("status").unwrap_or_default(),
                interactive: text("kind").as_deref() == Some("interactive"),
            })
        })
        .collect();
    peers.sort_by(|a, b| a.name.cmp(&b.name));
    peers
}

/// The registered session with this id (the id Agentty started the pane with).
pub fn peer_session(session_id: &str) -> Option<PeerSession> {
    peer_sessions().into_iter().find(|p| p.session_id == session_id)
}

/// The session a running Claude Code process is writing right now.
///
/// Every session registers itself under its own pid, so this is the only way to tell two sessions
/// in the same folder apart, and it keeps up with `/clear` and resumes, which give the same process
/// a new session id.
pub fn session_of_pid(pid: u32) -> Option<String> {
    peer_sessions().into_iter().find(|p| p.pid == pid).map(|p| p.session_id)
}

/// Whether a different running Claude Code process registered this session. Two sessions in one
/// folder share a project directory, so the newest transcript there can belong to the neighbour.
pub fn owned_by_other_process(session_id: &str, pid: Option<u32>) -> bool {
    owned_by_other(&peer_sessions(), session_id, pid)
}

fn owned_by_other(peers: &[PeerSession], session_id: &str, pid: Option<u32>) -> bool {
    peers.iter().any(|p| p.session_id == session_id && Some(p.pid) != pid)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_session_is_on_the_model_of_its_latest_answer() {
        let text = [
            r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[]}}"#,
            r#"{"type":"user","message":{"content":"<local-command-stdout>Set model to `Opus 5.5`</local-command-stdout>"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[]}}"#,
            r#"{"type":"user","message":{"content":"a question"}}"#,
        ]
        .join("\n");
        assert_eq!(super::last_model(&text).as_deref(), Some("claude-opus-5-5"));
        assert_eq!(super::last_model("no answers here"), None);
    }

    use super::*;

    #[test]
    fn reads_the_session_registry() {
        // The real registry: every entry that has a name is addressable and has an id and a pid.
        // Sessions rename themselves while they run, so only the id is compared.
        let peers = peer_sessions();
        for peer in &peers {
            assert!(!peer.name.is_empty() && !peer.session_id.is_empty() && peer.pid > 0);
        }
        if let Some(first) = peers.first() {
            assert_eq!(peer_session(&first.session_id).map(|p| p.session_id), Some(first.session_id.clone()));
        }
        assert!(peer_session("not-a-session-id").is_none());
    }

    #[test]
    fn the_headline_is_the_first_line_that_says_something() {
        assert_eq!(headline("## Done\n\nDetails follow.").as_deref(), Some("Done"));
        assert_eq!(headline("---\n**Fixed** the `login` bug.\nMore.").as_deref(), Some("Fixed the login bug."));
        assert_eq!(headline("- first item\n- second").as_deref(), Some("first item"));
        assert_eq!(headline("```\n\n").as_deref(), None);
        assert_eq!(headline(&"word ".repeat(100)).map(|h| h.chars().count() <= 141), Some(true));

        let dir = std::env::temp_dir().join(format!("agentty-headline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("session.jsonl");
        let line = |kind: &str, sidechain: bool, text: &str| {
            serde_json::json!({ "type": kind, "isSidechain": sidechain, "message": { "content": [{ "type": "text", "text": text }] } })
                .to_string()
        };
        let lines = [
            line("assistant", false, "An older answer."),
            line("user", false, "and now?"),
            line("assistant", false, "Pinned the download.\n\nIt now fetches one commit."),
            // A subagent's reply is not what the pane's agent said.
            line("assistant", true, "Subagent report."),
        ];
        std::fs::write(&transcript, lines.join("\n") + "\n").unwrap();
        assert_eq!(last_reply_headline(&transcript).as_deref(), Some("Pinned the download."));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_neighbours_session_is_not_this_panes() {
        let peer = |session_id: &str, pid| PeerSession {
            name: "agentty".into(),
            session_id: session_id.into(),
            pid,
            cwd: PathBuf::from("/Users/me/app"),
            status: "idle".into(),
            interactive: true,
        };
        let peers = [peer("neighbour", 4094), peer("mine", 8034)];
        // Started in the same folder before registering: the neighbour's newer transcript is not ours.
        assert!(owned_by_other(&peers, "neighbour", Some(8034)));
        assert!(owned_by_other(&peers, "neighbour", None));
        assert!(!owned_by_other(&peers, "mine", Some(8034)));
        // Nobody registered it (a finished session, or a Claude Code too old to register).
        assert!(!owned_by_other(&peers, "unregistered", Some(8034)));
    }

    #[test]
    fn encodes_project_dirs() {
        let dir = project_dir_for(Path::new("/Users/me/my.app/src_x"));
        assert!(dir.ends_with("-Users-me-my-app-src-x"));
    }
}
