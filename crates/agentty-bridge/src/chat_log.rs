//! A session's conversation as a chat app shows it: what the user wrote, and each reply as its
//! text, the tools it ran (with what they ran on and what came back) and its to-do list.
//!
//! [`crate::claude::transcript`] and [`crate::codex::transcript`] flatten a reply to text with
//! `[tool: …]` lines; this keeps the steps apart, so a chat can fold them into cards and open one.

use crate::fsutil::{one_line, truncate_chars};
use crate::model::{Agent, Role};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// What Claude Code writes as the reply of a turn stopped before any.
const NO_RESPONSE: &str = "No response requested.";
/// How much of a tool's output a step keeps.
const OUTPUT_CHARS: usize = 6_000;
const OUTPUT_LINES: usize = 80;
/// How much of what a tool ran on (a command, a patch) a step keeps.
const DETAIL_CHARS: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    fn parse(text: &str) -> Self {
        match text {
            "in_progress" | "in-progress" | "active" => TodoStatus::InProgress,
            "completed" | "done" => TodoStatus::Completed,
            _ => TodoStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Todo {
    pub text: String,
    /// How the agent says it while working on it ("Building the API").
    pub active: Option<String>,
    pub status: TodoStatus,
}

/// One tool the agent ran.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolStep {
    pub id: String,
    /// The tool's own name (`Bash`, `Edit`, `exec_command`, …).
    pub name: String,
    /// What the agent said the step is for, when it said so.
    pub description: Option<String>,
    /// What it ran on: the command, the file, the pattern, the URL.
    pub detail: Option<String>,
    /// What came back, once it has.
    pub output: Option<String>,
    pub failed: bool,
    /// What a file edit changed: lines starting with `-` (taken out) and `+` (put in).
    pub diff: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Tool(ToolStep),
    /// The agent's to-do list as it stood at this point of the reply.
    Todos(Vec<Todo>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub role: Role,
    /// What the user wrote (user entries).
    pub text: String,
    /// The reply (assistant entries).
    pub blocks: Vec<Block>,
}

/// The conversation of the transcript at `path`, written by `agent`.
pub fn read(agent: Agent, path: &Path) -> Result<Vec<Entry>> {
    let reader = BufReader::new(File::open(path)?);
    let lines = reader.lines().map_while(|line| line.ok());
    Ok(match agent {
        Agent::Codex => codex_entries(lines),
        _ => claude_entries(lines),
    })
}

/// Builds entries, keeping where each tool step is so its result can be put next to it.
#[derive(Default)]
struct Builder {
    entries: Vec<Entry>,
    steps: HashMap<String, (usize, usize)>,
    /// Claude Code's task tools change one list over the whole session.
    tasks: Vec<(String, Todo)>,
}

impl Builder {
    fn user(&mut self, text: String) {
        self.entries.push(Entry { role: Role::User, text, blocks: Vec::new() });
    }

    /// The reply being written: the last entry when it is one, else a new one.
    fn reply(&mut self) -> &mut Entry {
        if self.entries.last().is_none_or(|e| e.role != Role::Assistant) {
            self.entries.push(Entry { role: Role::Assistant, text: String::new(), blocks: Vec::new() });
        }
        self.entries.last_mut().expect("just pushed")
    }

    fn text(&mut self, text: &str) {
        let text = text.trim();
        // Claude Code's stand-in reply for a turn that was stopped before it said anything.
        if !text.is_empty() && text != NO_RESPONSE {
            self.reply().blocks.push(Block::Text(truncate_chars(text, 16_000)));
        }
    }

    fn step(&mut self, step: ToolStep) {
        let id = step.id.clone();
        let reply = self.reply();
        reply.blocks.push(Block::Tool(step));
        let at = (self.entries.len() - 1, self.entries.last().map_or(0, |e| e.blocks.len() - 1));
        if !id.is_empty() {
            self.steps.insert(id, at);
        }
    }

    /// A list replaces the one shown earlier in the same reply: the card shows where it stands.
    fn todos(&mut self, todos: Vec<Todo>) {
        let reply = self.reply();
        match reply.blocks.iter_mut().find(|b| matches!(b, Block::Todos(_))) {
            Some(block) => *block = Block::Todos(todos),
            None => reply.blocks.push(Block::Todos(todos)),
        }
    }

    fn result(&mut self, id: &str, output: String, failed: bool) {
        let Some(&(entry, block)) = self.steps.get(id) else { return };
        if let Some(Block::Tool(step)) = self.entries.get_mut(entry).and_then(|e| e.blocks.get_mut(block)) {
            step.output = Some(clip_output(&output));
            step.failed = failed;
        }
    }
}

fn clip_output(text: &str) -> String {
    let text = text.trim_end();
    let lines: Vec<&str> = text.lines().collect();
    let mut out = if lines.len() > OUTPUT_LINES { format!("{}\n…", lines[..OUTPUT_LINES].join("\n")) } else { text.to_string() };
    if out.chars().count() > OUTPUT_CHARS {
        out = format!("{}…", truncate_chars(&out, OUTPUT_CHARS));
    }
    out
}

fn str_field<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| input[*k].as_str()).filter(|s| !s.trim().is_empty())
}

fn claude_entries(lines: impl Iterator<Item = String>) -> Vec<Entry> {
    let mut b = Builder::default();
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
            continue;
        }
        match v["type"].as_str() {
            Some("user") => {
                if let Some(blocks) = v["message"]["content"].as_array() {
                    for block in blocks.iter().filter(|b| b["type"].as_str() == Some("tool_result")) {
                        let id = block["tool_use_id"].as_str().unwrap_or_default();
                        let failed = block["is_error"].as_bool() == Some(true);
                        b.result(id, result_text(&block["content"]), failed);
                    }
                }
                if let Some(text) = crate::claude::user_text(&v) {
                    b.user(text);
                }
            }
            Some("attachment") => {
                if let Some(text) = crate::claude::queued_text(&v) {
                    b.user(text);
                }
            }
            Some("assistant") => {
                let Some(blocks) = v["message"]["content"].as_array() else { continue };
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") => b.text(block["text"].as_str().unwrap_or_default()),
                        Some("tool_use") => claude_tool(&mut b, block),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    b.entries
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn claude_tool(b: &mut Builder, block: &Value) {
    let name = block["name"].as_str().unwrap_or("tool").to_string();
    let input = &block["input"];
    match name.as_str() {
        "TodoWrite" => {
            let todos = input["todos"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|t| {
                            Some(Todo {
                                text: str_field(t, &["content", "subject"])?.to_string(),
                                active: str_field(t, &["activeForm"]).map(str::to_string),
                                status: TodoStatus::parse(t["status"].as_str().unwrap_or_default()),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            return b.todos(todos);
        }
        "TaskCreate" => {
            if let Some(text) = str_field(input, &["subject", "content"]) {
                let id = (b.tasks.len() + 1).to_string();
                let active = str_field(input, &["activeForm"]).map(str::to_string);
                b.tasks.push((id, Todo { text: text.to_string(), active, status: TodoStatus::Pending }));
                let todos = b.tasks.iter().map(|(_, t)| t.clone()).collect();
                return b.todos(todos);
            }
        }
        "TaskUpdate" => {
            let id = input["taskId"].as_str().map(str::to_string).or_else(|| input["taskId"].as_u64().map(|n| n.to_string()));
            if let Some(id) = id {
                if input["status"].as_str() == Some("deleted") {
                    b.tasks.retain(|(task, _)| *task != id);
                } else if let Some((_, todo)) = b.tasks.iter_mut().find(|(task, _)| *task == id) {
                    if let Some(status) = input["status"].as_str() {
                        todo.status = TodoStatus::parse(status);
                    }
                    if let Some(subject) = str_field(input, &["subject"]) {
                        todo.text = subject.to_string();
                    }
                }
                let todos = b.tasks.iter().map(|(_, t)| t.clone()).collect();
                return b.todos(todos);
            }
        }
        _ => {}
    }
    let detail = str_field(input, &["command", "file_path", "notebook_path", "pattern", "url", "query", "skill", "prompt"])
        .map(|s| truncate_chars(s.trim(), DETAIL_CHARS));
    let description = str_field(input, &["description"]).map(|s| one_line(s, 160));
    let diff = edit_diff(&name, input);
    b.step(ToolStep {
        id: block["id"].as_str().unwrap_or_default().to_string(),
        name,
        description,
        detail,
        output: None,
        failed: false,
        diff,
    });
}

/// The lines a Claude Code file edit takes out and puts in (a new file: all put in).
fn edit_diff(name: &str, input: &Value) -> Option<String> {
    let pairs: Vec<(&str, &str)> = match name {
        "Edit" => vec![(input["old_string"].as_str()?, input["new_string"].as_str()?)],
        "MultiEdit" => {
            input["edits"].as_array()?.iter().filter_map(|e| Some((e["old_string"].as_str()?, e["new_string"].as_str()?))).collect()
        }
        "Write" => vec![("", input["content"].as_str()?)],
        _ => return None,
    };
    let mut out = Vec::new();
    for (old, new) in pairs {
        out.extend(old.lines().map(|l| format!("-{l}")));
        out.extend(new.lines().map(|l| format!("+{l}")));
    }
    (!out.is_empty()).then(|| clip_output(&out.join("\n")))
}

/// The first command an `exec` script runs (`tools.exec_command({cmd:"…"})`), unescaped.
fn script_command(script: &str) -> Option<String> {
    let start = script.find("cmd:\"").map(|at| at + 5).or_else(|| script.find("\"cmd\":\"").map(|at| at + 7))?;
    let mut out = String::new();
    let mut chars = script[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out).filter(|s| !s.is_empty()),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    None
}

fn codex_entries(lines: impl Iterator<Item = String>) -> Vec<Entry> {
    let mut b = Builder::default();
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["type"].as_str() != Some("response_item") {
            continue;
        }
        let payload = &v["payload"];
        match payload["type"].as_str() {
            Some("message") => {
                let Some(turn) = crate::codex::message_turn(payload) else { continue };
                match turn.role {
                    Role::User => b.user(turn.text),
                    Role::Assistant => b.text(&turn.text),
                }
            }
            Some("function_call" | "custom_tool_call") => {
                let name = payload["name"].as_str().unwrap_or("tool").to_string();
                let raw = payload["arguments"].as_str().or(payload["input"].as_str()).unwrap_or("");
                let args: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
                if name == "update_plan" {
                    let todos = args["plan"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|t| {
                                    Some(Todo {
                                        text: str_field(t, &["step"])?.to_string(),
                                        active: None,
                                        status: TodoStatus::parse(t["status"].as_str().unwrap_or_default()),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    b.todos(todos);
                    continue;
                }
                let command = match &args["command"] {
                    Value::Array(parts) => {
                        let parts: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
                        // `bash -lc "<script>"`: the script is what was run.
                        Some(match parts.as_slice() {
                            [_, flag, script] if flag.starts_with('-') && flag.ends_with('c') => script.to_string(),
                            _ => parts.join(" "),
                        })
                    }
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                };
                let detail = command
                    .or_else(|| str_field(&args, &["cmd", "path", "file_path", "query", "url"]).map(str::to_string))
                    // Codex's `exec` tool runs a script that calls its tools: the command is in it.
                    .or_else(|| script_command(raw))
                    .or_else(|| (!raw.is_empty() && args.is_null()).then(|| raw.to_string()))
                    .map(|s| truncate_chars(s.trim(), DETAIL_CHARS));
                b.step(ToolStep {
                    id: payload["call_id"].as_str().unwrap_or_default().to_string(),
                    name,
                    description: None,
                    detail,
                    output: None,
                    failed: false,
                    diff: None,
                });
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                let id = payload["call_id"].as_str().unwrap_or_default();
                let output = match &payload["output"] {
                    Value::String(s) => serde_json::from_str::<Value>(s)
                        .ok()
                        .and_then(|v| v["output"].as_str().map(str::to_string))
                        .unwrap_or_else(|| s.clone()),
                    other => other["output"].as_str().map(str::to_string).unwrap_or_else(|| result_text(other)),
                };
                b.result(id, output, false);
            }
            _ => {}
        }
    }
    b.entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> impl Iterator<Item = String> + '_ {
        text.lines().map(str::to_string)
    }

    #[test]
    fn claude_steps_carry_their_results_and_todos_update_in_place() {
        let log = r#"{"type":"user","message":{"content":"build it"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"On it."},{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[{"content":"Check node","status":"in_progress","activeForm":"Checking node"},{"content":"API","status":"pending"}]}}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"node -v","description":"Check Node version"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b1","content":"v24.22.0"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"TodoWrite","input":{"todos":[{"content":"Check node","status":"completed"},{"content":"API","status":"in_progress"}]}}]}}
{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}"#;
        let entries = claude_entries(lines(log));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "build it");
        let blocks = &entries[1].blocks;
        assert_eq!(blocks.len(), 4, "{blocks:?}");
        let Block::Todos(todos) = &blocks[1] else { panic!("no todos: {blocks:?}") };
        assert_eq!(todos[0].status, TodoStatus::Completed);
        assert_eq!(todos[1].status, TodoStatus::InProgress);
        let Block::Tool(step) = &blocks[2] else { panic!("no step") };
        assert_eq!(step.detail.as_deref(), Some("node -v"));
        assert_eq!(step.description.as_deref(), Some("Check Node version"));
        assert_eq!(step.output.as_deref(), Some("v24.22.0"));
        assert_eq!(blocks[3], Block::Text("Done.".into()));
        let stopped = r#"{"type":"user","message":{"content":"go"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"No response requested."}]}}"#;
        assert_eq!(claude_entries(lines(stopped)).len(), 1, "the stand-in reply is left out");
    }

    #[test]
    fn edits_carry_their_diff() {
        let edit = serde_json::json!({ "old_string": "a\nb", "new_string": "a\nc" });
        assert_eq!(edit_diff("Edit", &edit).as_deref(), Some("-a\n-b\n+a\n+c"));
        let write = serde_json::json!({ "content": "x" });
        assert_eq!(edit_diff("Write", &write).as_deref(), Some("+x"));
        assert_eq!(edit_diff("Bash", &edit), None);
    }

    #[test]
    fn claude_tasks_build_one_list() {
        let log = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"TaskCreate","input":{"subject":"One"}},{"type":"tool_use","id":"b","name":"TaskCreate","input":{"subject":"Two"}},{"type":"tool_use","id":"c","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}"#;
        let entries = claude_entries(lines(log));
        let Block::Todos(todos) = &entries[0].blocks[0] else { panic!() };
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].status, TodoStatus::Completed);
        assert_eq!(todos[1].status, TodoStatus::Pending);
    }

    #[test]
    fn codex_commands_plans_and_outputs() {
        let log = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}
{"type":"response_item","payload":{"type":"function_call","name":"update_plan","arguments":"{\"plan\":[{\"step\":\"Look\",\"status\":\"completed\"},{\"step\":\"Fix\",\"status\":\"in_progress\"}]}","call_id":"p"}}
{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"ls -la\"]}","call_id":"c1"}}
{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"{\"output\":\"a\\nb\",\"metadata\":{}}"}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}}"#;
        let entries = codex_entries(lines(log));
        assert_eq!(entries[0].text, "hi");
        let blocks = &entries[1].blocks;
        assert!(matches!(&blocks[0], Block::Todos(t) if t.len() == 2 && t[1].status == TodoStatus::InProgress));
        let Block::Tool(step) = &blocks[1] else { panic!() };
        assert_eq!(step.detail.as_deref(), Some("ls -la"));
        assert_eq!(step.output.as_deref(), Some("a\nb"));
        assert_eq!(blocks[2], Block::Text("Done".into()));
    }

    #[test]
    fn exec_scripts_name_their_command() {
        assert_eq!(
            script_command(r#"text(await tools.exec_command({cmd:"head -n 1 README.md",max_output_tokens:1000}))"#).as_deref(),
            Some("head -n 1 README.md")
        );
        assert_eq!(script_command(r#"{"cmd":"echo \"hi\""}"#).as_deref(), Some(r#"echo "hi""#));
        assert_eq!(script_command("console.log(1)"), None);
    }

    #[test]
    fn long_output_is_clipped() {
        let long = (0..200).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let clipped = clip_output(&long);
        assert_eq!(clipped.lines().count(), OUTPUT_LINES + 1);
        assert!(clipped.ends_with('…'));
    }
}
