//! What an agent session currently holds in its context window, reconstructed from the transcript:
//! loaded memory/instruction files, skills, files it read or edited, compactions and a token
//! breakdown. Sizes of individual parts are estimates (bytes / 4), scaled to the reported total.

use crate::fsutil::{cached_by_mtime, MtimeCache};
use crate::model::Agent;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    /// System prompt, tool definitions and other harness instructions.
    System,
    /// CLAUDE.md / AGENTS.md / rules / auto memory.
    Memory,
    /// Skill listings and loaded skill bodies.
    Skills,
    /// The summary that replaced earlier turns at the last compaction.
    Summary,
    /// Prompts, replies, tool calls and their results since the last compaction.
    Conversation,
}

impl Part {
    pub const ALL: [Part; 5] = [Part::System, Part::Memory, Part::Skills, Part::Summary, Part::Conversation];
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryFile {
    pub path: PathBuf,
    /// Where it comes from as the agent labels it (`User`, `Project`, `Local`, `AutoMem`, …).
    pub scope: String,
    pub tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileUse {
    Read,
    Edited,
    Attached,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextFile {
    pub path: PathBuf,
    pub usage: FileUse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Compaction {
    pub before: u64,
    pub after: u64,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContextSnapshot {
    pub model: Option<String>,
    /// Tokens in the window at the latest request, and the window size (0 when unknown).
    pub used: u64,
    pub window: u64,
    /// Estimated tokens per part, scaled so they add up to `used` when it is known.
    pub parts: Vec<(Part, u64)>,
    pub memory: Vec<MemoryFile>,
    pub skills_available: usize,
    pub skills_loaded: Vec<String>,
    /// Files read, edited or attached since the last compaction, most recent first.
    pub files: Vec<ContextFile>,
    pub compactions: usize,
    pub last_compaction: Option<Compaction>,
    pub prompts: usize,
    pub tool_calls: usize,
}

const MAX_FILES: usize = 60;

fn estimate(bytes: usize) -> u64 {
    bytes.div_ceil(4) as u64
}

static SNAPSHOTS: std::sync::LazyLock<MtimeCache<Option<ContextSnapshot>>> = std::sync::LazyLock::new(Default::default);

/// The context of session `id`, or `None` when the agent's transcripts aren't supported or found.
pub fn snapshot(agent: Agent, id: &str) -> Option<ContextSnapshot> {
    let path = match agent {
        Agent::Claude => crate::claude::find(id).ok()?,
        Agent::Codex => crate::codex::find(id).ok()?,
        _ => return None,
    };
    let mtime = crate::fsutil::mtime_ms(&path);
    cached_by_mtime(&SNAPSHOTS, &path, mtime, || {
        let file = File::open(&path).ok()?;
        let lines = BufReader::new(file).lines().map_while(Result::ok);
        Some(match agent {
            Agent::Claude => claude_snapshot(lines),
            _ => codex_snapshot(lines),
        })
    })
}

#[derive(Default)]
struct Builder {
    snapshot: ContextSnapshot,
    sizes: HashMap<Part, usize>,
    memory: Vec<(PathBuf, String, usize)>,
    files: Vec<ContextFile>,
    skills_loaded: Vec<String>,
}

impl Builder {
    fn add(&mut self, part: Part, bytes: usize) {
        *self.sizes.entry(part).or_default() += bytes;
    }

    /// Everything from before a compaction left the window.
    fn compacted(&mut self, before: u64, after: u64) {
        self.sizes.remove(&Part::Conversation);
        self.sizes.remove(&Part::Summary);
        self.files.clear();
        self.skills_loaded.clear();
        self.snapshot.prompts = 0;
        self.snapshot.tool_calls = 0;
        self.snapshot.compactions += 1;
        self.snapshot.last_compaction = Some(Compaction { before, after, summary: None });
    }

    fn remember(&mut self, path: PathBuf, scope: String, bytes: usize) {
        self.memory.retain(|(p, _, _)| *p != path);
        self.memory.push((path, scope, bytes));
    }

    fn touch(&mut self, path: &str, usage: FileUse) {
        if path.is_empty() {
            return;
        }
        let path = PathBuf::from(path);
        // Keep the strongest use (an edit beats a read) and move it to the front.
        let usage = match self.files.iter().position(|f| f.path == path) {
            Some(index) => {
                let previous = self.files.remove(index).usage;
                if previous == FileUse::Edited {
                    previous
                } else {
                    usage
                }
            }
            None => usage,
        };
        self.files.insert(0, ContextFile { path, usage });
    }

    fn finish(mut self) -> ContextSnapshot {
        let memory_bytes: usize = self.memory.iter().map(|(_, _, b)| b).sum();
        self.add(Part::Memory, memory_bytes);
        let estimated: u64 = self.sizes.values().map(|b| estimate(*b)).sum();
        let used = self.snapshot.used;
        // The system prompt and tool definitions are mostly not in the transcript: whatever the
        // reported total leaves after the parts we can see.
        if used > estimated {
            let rest = (used - estimated) as usize * 4;
            self.add(Part::System, rest);
        }
        let total: u64 = self.sizes.values().map(|b| estimate(*b)).sum();
        let scale = if used > 0 && total > 0 { used as f64 / total as f64 } else { 1.0 };
        self.snapshot.parts = Part::ALL
            .iter()
            .filter_map(|part| {
                let tokens = (estimate(*self.sizes.get(part)?) as f64 * scale).round() as u64;
                (tokens > 0).then_some((*part, tokens))
            })
            .collect();
        self.snapshot.memory =
            self.memory.into_iter().map(|(path, scope, bytes)| MemoryFile { path, scope, tokens: estimate(bytes) }).collect();
        self.files.truncate(MAX_FILES);
        self.snapshot.files = self.files;
        self.snapshot.skills_loaded = self.skills_loaded;
        self.snapshot
    }
}

fn claude_snapshot(lines: impl Iterator<Item = String>) -> ContextSnapshot {
    let mut b = Builder::default();
    let mut expect_summary = false;
    for line in lines {
        let len = line.len();
        // Bookkeeping lines never reach the model; skip parsing them.
        if !line.contains("\"type\":\"user\"")
            && !line.contains("\"type\":\"assistant\"")
            && !line.contains("\"type\":\"attachment\"")
            && !line.contains("\"compact_boundary\"")
        {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str().unwrap_or_default() {
            "system" if v["subtype"] == "compact_boundary" => {
                let meta = &v["compactMetadata"];
                b.compacted(meta["preTokens"].as_u64().unwrap_or(0), meta["postTokens"].as_u64().unwrap_or(0));
                expect_summary = true;
            }
            "user"
                if v["isCompactSummary"].as_bool() == Some(true)
                    || (expect_summary && v["isVisibleInTranscriptOnly"].as_bool() == Some(true)) =>
            {
                expect_summary = false;
                let text = message_text(&v["message"]["content"]);
                b.add(Part::Summary, text.len());
                if let Some(compaction) = b.snapshot.last_compaction.as_mut() {
                    compaction.summary = Some(text);
                }
            }
            "user" => {
                let content = &v["message"]["content"];
                match content {
                    Value::String(_) => b.snapshot.prompts += 1,
                    Value::Array(items)
                        if items.iter().any(|i| i["type"] == "text") && !items.iter().any(|i| i["type"] == "tool_result") =>
                    {
                        b.snapshot.prompts += 1
                    }
                    _ => {}
                }
                b.add(Part::Conversation, len);
            }
            "assistant" => {
                if let Some(items) = v["message"]["content"].as_array() {
                    for item in items.iter().filter(|i| i["type"] == "tool_use") {
                        b.snapshot.tool_calls += 1;
                        let input = &item["input"];
                        let path = input["file_path"].as_str().or(input["notebook_path"].as_str()).unwrap_or_default();
                        match item["name"].as_str().unwrap_or_default() {
                            "Read" => b.touch(path, FileUse::Read),
                            "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => b.touch(path, FileUse::Edited),
                            "Skill" => {
                                if let Some(name) = input["skill"].as_str() {
                                    b.skills_loaded.retain(|s| s != name);
                                    b.skills_loaded.push(name.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let usage = &v["message"]["usage"];
                let model = v["message"]["model"].as_str().unwrap_or_default();
                if !usage.is_null() && !model.starts_with('<') {
                    let n = |k: &str| usage[k].as_u64().unwrap_or(0);
                    b.snapshot.used = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
                    b.snapshot.window = crate::claude_context_window(model);
                    b.snapshot.model = Some(model.to_string());
                }
                b.add(Part::Conversation, len);
            }
            "attachment" => {
                let attachment = &v["attachment"];
                match attachment["type"].as_str().unwrap_or_default() {
                    "instructions" => {
                        for file in attachment["files"].as_array().into_iter().flatten() {
                            let (Some(path), Some(content)) = (file["path"].as_str(), file["content"].as_str()) else { continue };
                            b.remember(PathBuf::from(path), file["type"].as_str().unwrap_or_default().to_string(), content.len());
                        }
                    }
                    "skill_listing" => {
                        b.snapshot.skills_available = attachment["skillCount"].as_u64().unwrap_or(0) as usize;
                        b.add(Part::Skills, attachment["content"].as_str().map_or(0, str::len));
                    }
                    "invoked_skills" => {
                        for skill in attachment["skills"].as_array().into_iter().flatten() {
                            if let Some(name) = skill["name"].as_str() {
                                b.skills_loaded.retain(|s| s != name);
                                b.skills_loaded.push(name.to_string());
                            }
                            b.add(Part::Skills, skill["content"].as_str().map_or(0, str::len));
                        }
                    }
                    "edited_text_file" => {
                        b.touch(attachment["filename"].as_str().unwrap_or_default(), FileUse::Edited);
                        b.add(Part::Conversation, len);
                    }
                    "file" | "compact_file_reference" => {
                        b.touch(attachment["filename"].as_str().unwrap_or_default(), FileUse::Attached);
                        b.add(Part::Conversation, len);
                    }
                    "deferred_tools_delta" | "deferred_tools_record" | "agent_listing_delta" | "model" | "environment" => {
                        b.add(Part::System, len)
                    }
                    // Bookkeeping (queued prompts, token counters, hook results) isn't sent as is.
                    _ => {}
                }
            }
            _ => {}
        }
    }
    b.finish()
}

fn message_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().filter_map(|i| i["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn codex_snapshot(lines: impl Iterator<Item = String>) -> ContextSnapshot {
    let mut b = Builder::default();
    for line in lines {
        let len = line.len();
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let payload = &v["payload"];
        match v["type"].as_str().unwrap_or_default() {
            "session_meta" => b.add(Part::System, payload["base_instructions"]["text"].as_str().map_or(0, str::len)),
            "compacted" => {
                let before = b.snapshot.used;
                b.compacted(before, 0);
                let kept: usize =
                    payload["replacement_history"].as_array().map_or(0, |items| items.iter().map(|i| i.to_string().len()).sum());
                b.add(Part::Summary, kept + payload["message"].as_str().map_or(0, str::len));
                if let Some(compaction) = b.snapshot.last_compaction.as_mut() {
                    compaction.summary = payload["message"].as_str().filter(|m| !m.is_empty()).map(str::to_string);
                }
            }
            "world_state" => {
                for (key, value) in payload["state"]["agents_md"].as_object().into_iter().flatten() {
                    let path = value["path"].as_str().unwrap_or(key);
                    let content = value.as_str().or(value["content"].as_str()).or(value["text"].as_str()).unwrap_or_default();
                    b.remember(PathBuf::from(path), "AGENTS.md".into(), content.len());
                }
            }
            "turn_context" => {
                if let Some(model) = payload["model"].as_str() {
                    b.snapshot.model = Some(model.to_string());
                }
            }
            "event_msg" if payload["type"] == "token_count" => {
                let info = &payload["info"];
                if !info.is_null() {
                    b.snapshot.used = info["last_token_usage"]["input_tokens"].as_u64().unwrap_or(b.snapshot.used);
                    b.snapshot.window = info["model_context_window"].as_u64().unwrap_or(b.snapshot.window);
                }
            }
            "response_item" => match payload["type"].as_str().unwrap_or_default() {
                "message" => match payload["role"].as_str().unwrap_or_default() {
                    "developer" => {
                        let text = message_text_items(&payload["content"]);
                        if text.contains("<skills_instructions>") {
                            b.snapshot.skills_available =
                                text.lines().filter(|l| l.trim_start().starts_with("- ") && l.contains("SKILL.md")).count();
                            b.add(Part::Skills, len);
                        } else {
                            b.add(Part::System, len);
                        }
                    }
                    "user" => {
                        let kinds = payload["internal_chat_message_metadata_passthrough"]["content_item_kinds"].to_string();
                        if kinds.contains("user.text") {
                            b.snapshot.prompts += 1;
                            b.add(Part::Conversation, len);
                        } else {
                            b.add(Part::System, len);
                        }
                    }
                    _ => b.add(Part::Conversation, len),
                },
                "function_call" | "custom_tool_call" | "local_shell_call" => {
                    b.snapshot.tool_calls += 1;
                    b.add(Part::Conversation, len);
                }
                // Encrypted reasoning isn't re-sent as text.
                "reasoning" => {}
                _ => b.add(Part::Conversation, len),
            },
            _ => {}
        }
    }
    b.finish()
}

fn message_text_items(content: &Value) -> String {
    content.as_array().into_iter().flatten().filter_map(|i| i["text"].as_str()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn lines(items: &[Value]) -> impl Iterator<Item = String> {
        items.iter().map(|v| v.to_string()).collect::<Vec<_>>().into_iter()
    }

    #[test]
    fn claude_context_since_last_compaction() {
        let transcript = [
            serde_json::json!({"type":"attachment","attachment":{"type":"instructions","files":[
                {"path":"/home/example/.claude/CLAUDE.md","type":"User","content":"x".repeat(400)},
                {"path":"/repo/CLAUDE.md","type":"Project","content":"old"}]}}),
            serde_json::json!({"type":"user","message":{"role":"user","content":"first prompt"}}),
            serde_json::json!({"type":"assistant","message":{"model":"claude-opus-5","content":[
                {"type":"tool_use","name":"Read","input":{"file_path":"/repo/old.rs"}}],"usage":{"input_tokens":10}}}),
            serde_json::json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"preTokens":900000,"postTokens":20000}}),
            serde_json::json!({"type":"user","isCompactSummary":true,"message":{"role":"user","content":"Summary of earlier work"}}),
            serde_json::json!({"type":"attachment","attachment":{"type":"instructions","files":[
                {"path":"/repo/CLAUDE.md","type":"Project","content":"y".repeat(800)}]}}),
            serde_json::json!({"type":"attachment","attachment":{"type":"invoked_skills","skills":[{"name":"example-skill","content":"body"}]}}),
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"next prompt"}]}}),
            serde_json::json!({"type":"assistant","message":{"model":"claude-opus-5","content":[
                {"type":"tool_use","name":"Read","input":{"file_path":"/repo/a.rs"}},
                {"type":"tool_use","name":"Edit","input":{"file_path":"/repo/b.rs"}}],
                "usage":{"input_tokens":1000,"cache_read_input_tokens":30000,"cache_creation_input_tokens":500}}}),
        ];
        let s = claude_snapshot(lines(&transcript));
        assert_eq!(s.used, 31_500);
        assert_eq!(s.window, 1_000_000);
        assert_eq!(s.compactions, 1);
        assert_eq!(s.last_compaction.as_ref().map(|c| (c.before, c.after)), Some((900_000, 20_000)));
        assert_eq!(s.last_compaction.as_ref().and_then(|c| c.summary.as_deref()), Some("Summary of earlier work"));
        assert_eq!(s.prompts, 1);
        assert_eq!(s.tool_calls, 2);
        // Memory keeps the latest copy of each file across compactions.
        assert_eq!(s.memory.len(), 2);
        assert_eq!(s.memory.iter().find(|m| m.path == Path::new("/repo/CLAUDE.md")).map(|m| m.tokens), Some(200));
        // Files from before the compaction are gone; the newest is first.
        let files: Vec<_> = s.files.iter().map(|f| (f.path.to_string_lossy().to_string(), f.usage)).collect();
        assert_eq!(files, vec![("/repo/b.rs".into(), FileUse::Edited), ("/repo/a.rs".into(), FileUse::Read)]);
        assert_eq!(s.skills_loaded, vec!["example-skill".to_string()]);
        // Parts add up to the reported total.
        let sum: u64 = s.parts.iter().map(|(_, t)| t).sum();
        assert!(sum.abs_diff(s.used) <= Part::ALL.len() as u64, "{sum} vs {}", s.used);
        assert!(s.parts.iter().any(|(p, _)| *p == Part::System));
    }

    #[test]
    fn codex_context_from_rollout() {
        let rollout = [
            serde_json::json!({"type":"session_meta","payload":{"base_instructions":{"text":"You are Codex"}}}),
            serde_json::json!({"type":"turn_context","payload":{"model":"gpt-example"}}),
            serde_json::json!({"type":"world_state","payload":{"state":{"agents_md":{"/repo/AGENTS.md":"z".repeat(80)}}}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<skills_instructions>\n- example: does things (file: /s/SKILL.md)"}]}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}],
                "internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]}}}),
            serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"exec"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":12000},"model_context_window":258400}}}),
        ];
        let s = codex_snapshot(lines(&rollout));
        assert_eq!(s.model.as_deref(), Some("gpt-example"));
        assert_eq!((s.used, s.window), (12_000, 258_400));
        assert_eq!((s.prompts, s.tool_calls, s.skills_available), (1, 1, 1));
        assert_eq!(s.memory.first().map(|m| (m.path.clone(), m.tokens)), Some((PathBuf::from("/repo/AGENTS.md"), 20)));
    }
}
