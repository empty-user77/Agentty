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
/// Background tasks kept at most: the oldest goes (it may never have said it ended).
const MAX_BACKGROUND: usize = 16;
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

/// Work the agent left running in the background: it may be idle, waiting for this to finish.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundTask {
    /// The tool call that started it.
    pub id: String,
    /// The agent's own id for it, once it has said (`KillShell` and the like go by this one).
    pub task_id: Option<String>,
    /// What it is: what the agent said it is for, else its command.
    pub label: String,
    /// When it started, in ms since the epoch: tasks die with the agent, and one started before the
    /// agent now running is not running any more, though no line in the transcript says so.
    pub started_ms: Option<i64>,
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
    /// What the user wrote (user entries), or the summary (summary entries).
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

/// Where `agent` keeps session `id`, whichever kind of log it writes. `None` for an id that could
/// name anything else than a session: ids are looked up as file names.
pub fn transcript_file(agent: Agent, id: &str) -> Option<std::path::PathBuf> {
    let plain = !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !plain || id.contains("..") {
        return None;
    }
    match agent {
        Agent::Gemini => crate::gemini::find(id).ok(),
        Agent::Agy => crate::agy::find(id).ok(),
        _ => crate::transcript_path(agent, id),
    }
}

/// How much a transcript holds, to tell when it grew. Antigravity's is an SQLite database whose
/// new rows wait in its write-ahead log first.
pub fn transcript_size(path: &Path) -> Option<u64> {
    let len = std::fs::metadata(path).ok()?.len();
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    Some(len + std::fs::metadata(Path::new(&wal)).map_or(0, |m| m.len()))
}

/// How much of a transcript [`Reader::update`] reads at a time.
const READ_CHUNK: usize = 1 << 20;

/// A line longer than this is left out by a [`Reader::windowed`] reader, which promises its memory
/// stays bounded: only a pasted picture comes near it.
const WINDOW_MAX_LINE: usize = 32 << 20;

/// What a [`Reader::windowed`] reader keeps of a conversation.
#[derive(Clone, Copy)]
struct Window {
    entries: usize,
    blocks: usize,
}

/// A transcript read as it grows: each [`Reader::update`] parses only what was written since the
/// last one, so a long session is not read again from the start on every new line.
pub struct Reader {
    agent: Agent,
    builder: Builder,
    /// How far the file has been read.
    offset: u64,
    /// The end of the file when it stops mid-line: the rest of that line comes later.
    partial: Vec<u8>,
    window: Option<Window>,
    /// Older entries or blocks went out of the window.
    cut: bool,
    /// In a line too long to keep: everything up to its end is left out.
    skipping: bool,
}

impl Reader {
    pub fn new(agent: Agent) -> Self {
        Reader { agent, builder: Builder::default(), offset: 0, partial: Vec::new(), window: None, cut: false, skipping: false }
    }

    /// A reader that holds only the newest `entries` entries, and the newest `blocks` blocks of
    /// each, however long the session: for readers that don't need all of it.
    pub fn windowed(agent: Agent, entries: usize, blocks: usize) -> Self {
        Reader { window: Some(Window { entries, blocks }), ..Reader::new(agent) }
    }

    /// The entries held now, oldest first.
    pub fn entries(&self) -> &[Entry] {
        &self.builder.entries
    }

    /// What the agent has running in the background, if it started at `since_ms` or later (when
    /// the agent now running started). Read apart from the entries, so a window doesn't lose them.
    pub fn background(&self, since_ms: u64) -> Vec<BackgroundTask> {
        self.builder.background.iter().filter(|task| task.started_ms.is_none_or(|at| at >= since_ms as i64)).cloned().collect()
    }

    /// Whether a windowed reader has let older entries or blocks go.
    pub fn cut(&self) -> bool {
        self.cut
    }

    /// Lets go of what is outside the window; a result arriving for a step let go is dropped.
    fn trim(&mut self) {
        let Some(window) = self.window else { return };
        let b = &mut self.builder;
        let extra = b.entries.len().saturating_sub(window.entries);
        if extra > 0 {
            b.entries.drain(..extra);
            b.steps.retain(|_, (entry, _)| {
                let kept = *entry >= extra;
                *entry = entry.saturating_sub(extra);
                kept
            });
            b.changed_from = b.changed_from.map(|from| from.saturating_sub(extra));
            self.cut = true;
        }
        for (index, entry) in b.entries.iter_mut().enumerate() {
            let extra = entry.blocks.len().saturating_sub(window.blocks);
            if extra == 0 {
                continue;
            }
            entry.blocks.drain(..extra);
            b.steps.retain(|_, (at, block)| {
                if *at != index {
                    return true;
                }
                let kept = *block >= extra;
                *block = block.saturating_sub(extra);
                kept
            });
            self.cut = true;
        }
    }

    /// Reads what was appended to `path`. When something changed: the first entry that did, and
    /// the entries from there on (what came before is as it was). A file that got shorter (a new
    /// one in its place) is read again from the start.
    pub fn update(&mut self, path: &Path) -> Result<Option<(usize, Vec<Entry>)>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        if len < self.offset {
            *self = Reader { window: self.window, ..Reader::new(self.agent) };
            self.builder.changed_from = Some(0);
        }
        if len > self.offset {
            file.seek(SeekFrom::Start(self.offset))?;
            // In pieces: a first read of a long session (pictures sent in it are long lines) holds
            // one piece and the line it ends in, not the whole file.
            let mut rest = file.take(len - self.offset);
            let mut chunk = vec![0; READ_CHUNK];
            loop {
                let read = rest.read(&mut chunk)?;
                if read == 0 {
                    break;
                }
                self.offset += read as u64;
                let mut new = &chunk[..read];
                if self.skipping {
                    let Some(end) = new.iter().position(|b| *b == b'\n') else { continue };
                    new = &new[end + 1..];
                    self.skipping = false;
                }
                let start = self.partial.len();
                self.partial.extend_from_slice(new);
                // Only the new bytes can hold the end of a line.
                let Some(end) = self.partial[start..].iter().rposition(|b| *b == b'\n') else {
                    if self.window.is_some() && self.partial.len() > WINDOW_MAX_LINE {
                        self.partial = Vec::new();
                        self.skipping = true;
                    }
                    continue;
                };
                let complete = start + end + 1;
                for line in self.partial[..complete].split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
                    let line = String::from_utf8_lossy(line);
                    match self.agent {
                        Agent::Codex => codex_line(&mut self.builder, &line),
                        _ => claude_line(&mut self.builder, &line),
                    }
                }
                self.partial.drain(..complete);
                self.trim();
            }
        }
        Ok(self.builder.changed_from.take().map(|from| {
            let from = from.min(self.builder.entries.len());
            (from, self.builder.entries[from..].to_vec())
        }))
    }
}

/// Builds entries, keeping where each tool step is so its result can be put next to it.
#[derive(Default)]
struct Builder {
    entries: Vec<Entry>,
    steps: HashMap<String, (usize, usize)>,
    /// Claude Code's task tools change one list over the whole session.
    tasks: Vec<(String, Todo)>,
    /// Tasks created so far: Claude Code numbers them on, deleted ones included.
    tasks_created: usize,
    /// What runs in the background now.
    background: Vec<BackgroundTask>,
    /// The first entry added or changed since [`Reader::update`] last looked.
    changed_from: Option<usize>,
}

impl Builder {
    fn changed(&mut self, entry: usize) {
        self.changed_from = Some(self.changed_from.map_or(entry, |from| from.min(entry)));
    }

    fn user(&mut self, text: String) {
        self.changed(self.entries.len());
        self.entries.push(Entry { role: Role::User, text, blocks: Vec::new() });
    }

    fn summary(&mut self, text: String) {
        self.changed(self.entries.len());
        self.entries.push(Entry { role: Role::Summary, text, blocks: Vec::new() });
    }

    /// The reply being written (to be changed): the last entry when it is one, else a new one.
    fn reply(&mut self) -> &mut Entry {
        if self.entries.last().is_none_or(|e| e.role != Role::Assistant) {
            self.entries.push(Entry { role: Role::Assistant, text: String::new(), blocks: Vec::new() });
        }
        self.changed(self.entries.len() - 1);
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

    fn background_started(&mut self, id: &str, label: String, started_ms: Option<i64>) {
        if self.background.len() >= MAX_BACKGROUND {
            self.background.remove(0);
        }
        if !self.background.iter().any(|task| task.id == id) {
            self.background.push(BackgroundTask { id: id.to_string(), task_id: None, label, started_ms });
        }
    }

    /// A tool's result says about the background: it never started (failed), or under which id it
    /// runs, which is also how a command the user sent to the background late shows up.
    fn background_result(&mut self, id: &str, task_id: Option<&str>, failed: bool, started_ms: Option<i64>) {
        if failed {
            return self.background.retain(|task| task.id != id);
        }
        let Some(task_id) = task_id else { return };
        if !self.background.iter().any(|task| task.id == id) {
            let label = self.steps.get(id).and_then(|&(entry, block)| match self.entries.get(entry)?.blocks.get(block)? {
                Block::Tool(step) => background_label(step.description.as_deref(), step.detail.as_deref()),
                _ => None,
            });
            self.background_started(id, label.unwrap_or_else(|| task_id.to_string()), started_ms);
        }
        if let Some(task) = self.background.iter_mut().find(|task| task.id == id) {
            task.task_id = Some(task_id.to_string());
        }
    }

    /// Finished or stopped: goes by the tool call's id or the agent's own.
    fn background_ended(&mut self, id: &str) {
        self.background.retain(|task| task.id != id && task.task_id.as_deref() != Some(id));
    }

    fn result(&mut self, id: &str, output: String, failed: bool) {
        let Some(&(entry, block)) = self.steps.get(id) else { return };
        self.changed(entry);
        if let Some(Block::Tool(step)) = self.entries.get_mut(entry).and_then(|e| e.blocks.get_mut(block)) {
            step.output = Some(clip_output(&output));
            step.failed = failed;
        }
    }
}

/// How a background task is named: one line, what it is for before what it runs.
fn background_label(description: Option<&str>, detail: Option<&str>) -> Option<String> {
    description.or(detail).map(|text| one_line(text, 120)).filter(|label| !label.is_empty())
}

/// The ids in a line's finished-task notices. Only a notice with a status says it ended.
fn ended_task_ids(text: &str) -> Vec<String> {
    const OPEN: &str = "<task-notification>";
    const CLOSE: &str = "</task-notification>";
    let tag = |notice: &str, name: &str| {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = notice.find(&open)? + open.len();
        let end = notice[start..].find(&close)?;
        Some(notice[start..start + end].trim().to_string())
    };
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let end = after.find(CLOSE).unwrap_or(after.len());
        let notice = &after[..end];
        if tag(notice, "status").is_some() {
            ids.extend(tag(notice, "tool-use-id"));
            ids.extend(tag(notice, "task-id"));
        }
        rest = &after[end..];
    }
    ids
}

/// Keeps the background list up to date from one line of the transcript.
fn claude_background(b: &mut Builder, v: &Value) {
    // Only the conversation's own lines: newer Claude Code versions also keep copies of what went to
    // the API, written after the result of the call they hold, with no time on them.
    if !matches!(v["type"].as_str(), Some("user" | "assistant" | "attachment")) {
        return;
    }
    let started_ms = v["timestamp"].as_str().and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()).map(|at| at.timestamp_millis());
    let mut texts: Vec<&str> = Vec::new();
    for value in [&v["message"]["content"], &v["attachment"]["prompt"]] {
        match value {
            Value::String(text) => texts.push(text),
            Value::Array(parts) => texts.extend(parts.iter().filter_map(|part| part["text"].as_str())),
            _ => {}
        }
    }
    for id in texts.iter().filter(|text| text.contains("<task-notification>")).flat_map(|text| ended_task_ids(text)) {
        b.background_ended(&id);
    }
    let Some(blocks) = v["message"]["content"].as_array() else { return };
    for block in blocks {
        match block["type"].as_str() {
            Some("tool_use") => {
                let (id, input) = (block["id"].as_str().unwrap_or_default(), &block["input"]);
                match block["name"].as_str() {
                    Some("KillShell" | "KillBash" | "TaskStop") => {
                        if let Some(task) = str_field(input, &["shell_id", "task_id", "bash_id"]) {
                            b.background_ended(task);
                        }
                    }
                    _ if !id.is_empty() && input["run_in_background"].as_bool() == Some(true) => {
                        let label = background_label(str_field(input, &["description"]), str_field(input, &["command", "prompt"]));
                        b.background_started(id, label.unwrap_or_else(|| block["name"].as_str().unwrap_or("tool").to_string()), started_ms);
                    }
                    _ => {}
                }
            }
            Some("tool_result") => {
                let id = block["tool_use_id"].as_str().unwrap_or_default();
                let failed = block["is_error"].as_bool() == Some(true);
                b.background_result(id, v["toolUseResult"]["backgroundTaskId"].as_str(), failed, started_ms);
            }
            _ => {}
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
        claude_line(&mut b, &line);
    }
    b.entries
}

fn claude_line(b: &mut Builder, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if v["isSidechain"].as_bool() == Some(true) {
        return;
    }
    claude_background(b, &v);
    if v["isMeta"].as_bool() == Some(true) {
        // The harness's own lines, except a message from another session reaching an idle one.
        if let Some(text) = crate::claude::peer_text(&v) {
            b.user(text);
        }
        return;
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
            if let Some(text) = crate::claude::compact_summary(&v) {
                b.summary(text);
            } else if let Some(text) = crate::claude::said_text(&v) {
                b.user(text);
            }
        }
        Some("attachment") => {
            if let Some(text) = crate::claude::queued_text(&v) {
                b.user(text);
            }
        }
        Some("assistant") => {
            let Some(blocks) = v["message"]["content"].as_array() else { return };
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => b.text(block["text"].as_str().unwrap_or_default()),
                    Some("tool_use") => claude_tool(b, block),
                    _ => {}
                }
            }
        }
        _ => {}
    }
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
                b.tasks_created += 1;
                let id = b.tasks_created.to_string();
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
        codex_line(&mut b, &line);
    }
    b.entries
}

fn codex_line(b: &mut Builder, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if v["type"].as_str() != Some("response_item") {
        return;
    }
    let payload = &v["payload"];
    match payload["type"].as_str() {
        Some("message") => {
            let Some(turn) = crate::codex::message_turn(payload) else { return };
            match turn.role {
                Role::User => b.user(turn.text),
                Role::Assistant => b.text(&turn.text),
                Role::Summary => b.summary(turn.text),
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
                return;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> impl Iterator<Item = String> + '_ {
        text.lines().map(str::to_string)
    }

    fn temp_log(name: &str, text: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("agentty-chat-log-{name}-{}.jsonl", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_windowed_reader_keeps_the_newest_and_still_places_results() {
        let turn = |n: usize| {
            format!(
                "{{\"type\":\"user\",\"message\":{{\"content\":\"ask {n}\"}}}}\n{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"id\":\"t{n}\",\"name\":\"Bash\",\"input\":{{\"command\":\"c{n}\"}}}}]}}}}\n"
            )
        };
        let path = temp_log("window", &(0..50).map(turn).collect::<String>());
        let mut reader = Reader::windowed(Agent::Claude, 4, 10);
        reader.update(&path).unwrap();
        assert!(reader.cut());
        let entries = reader.entries();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[2].text, "ask 49");
        // A result for a step still held lands on it; one for a step let go is ignored.
        let more = "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t49\",\"content\":\"ok\"},{\"type\":\"tool_result\",\"tool_use_id\":\"t3\",\"content\":\"late\"}]}}\n";
        std::fs::write(&path, format!("{}{more}", (0..50).map(turn).collect::<String>())).unwrap();
        assert!(reader.update(&path).unwrap().is_some());
        let Block::Tool(step) = &reader.entries()[3].blocks[0] else { panic!("no step") };
        assert_eq!(step.output.as_deref(), Some("ok"));
        // A new file in its place keeps the window.
        std::fs::write(&path, turn(1)).unwrap();
        reader.update(&path).unwrap();
        assert_eq!(reader.entries().len(), 2);
        assert!(!reader.cut());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_windowed_reader_keeps_the_newest_blocks_of_a_long_reply() {
        let step = |n: usize| {
            format!(
                "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"id\":\"t{n}\",\"name\":\"Bash\",\"input\":{{\"command\":\"c{n}\"}}}}]}}}}\n"
            )
        };
        let path = temp_log("blocks", &(0..30).map(step).collect::<String>());
        let mut reader = Reader::windowed(Agent::Claude, 4, 5);
        reader.update(&path).unwrap();
        let more =
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t29\",\"content\":\"ok\"}]}}\n";
        std::fs::write(&path, format!("{}{more}", (0..30).map(step).collect::<String>())).unwrap();
        reader.update(&path).unwrap();
        let blocks = &reader.entries()[0].blocks;
        assert_eq!(blocks.len(), 5);
        let Block::Tool(last) = &blocks[4] else { panic!("no step") };
        assert_eq!((last.detail.as_deref(), last.output.as_deref()), (Some("c29"), Some("ok")));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_windowed_reader_leaves_out_a_line_too_long_to_hold() {
        let long = "x".repeat(WINDOW_MAX_LINE + READ_CHUNK * 2);
        let good = "{\"type\":\"user\",\"message\":{\"content\":\"after\"}}\n";
        let path = temp_log("long", &format!("{long}\n{good}"));
        let mut reader = Reader::windowed(Agent::Claude, 4, 5);
        reader.update(&path).unwrap();
        assert_eq!(reader.entries().len(), 1);
        assert_eq!(reader.entries()[0].text, "after");
        assert!(reader.partial.capacity() <= READ_CHUNK * 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn only_plain_ids_are_looked_up() {
        for id in ["", "../../etc/passwd", "a/b", "..", "a..b", "a\\b", "x y"] {
            assert_eq!(transcript_file(Agent::Agy, id), None, "{id}");
            assert_eq!(transcript_file(Agent::Claude, id), None, "{id}");
        }
        assert_eq!(transcript_file(Agent::Agy, &"a".repeat(129)), None);
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
    fn a_message_from_another_session_is_kept_without_the_harness_words_around_it() {
        let tag = r#"<cross-session-message from=\"x\" from-name=\"ios\">hi</cross-session-message>"#;
        let log = format!(
            r#"{{"type":"user","isMeta":true,"message":{{"content":"Another session wrote: {tag} Answer it."}}}}
{{"type":"user","isMeta":true,"message":{{"content":"harness note"}}}}"#
        );
        let entries = claude_entries(lines(&log));
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].text, r#"<cross-session-message from="x" from-name="ios">hi</cross-session-message>"#);
    }

    #[test]
    fn a_message_typed_mid_turn_is_kept_with_or_without_pictures() {
        let log = r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"plain"}}
{"type":"attachment","attachment":{"type":"queued_command","prompt":[{"type":"text","text":"look at this"},{"type":"image","source":{"type":"base64","data":"x"}}]}}
{"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification>done</task-notification>"}}"#;
        let said: Vec<String> = claude_entries(lines(log)).into_iter().map(|e| e.text).collect();
        assert_eq!(said, ["plain", "look at this\n[image]"]);
    }

    fn background_of(log: &str, since_ms: u64) -> Vec<String> {
        let mut b = Builder::default();
        for line in lines(log) {
            claude_line(&mut b, &line);
        }
        b.background.iter().filter(|t| t.started_ms.is_none_or(|at| at >= since_ms as i64)).map(|t| t.label.clone()).collect()
    }

    const STARTED: &str = r#"{"type":"assistant","timestamp":"2026-11-02T06:55:12.609Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"sleep 4","description":"Sleep for four seconds","run_in_background":true}}]}}"#;
    const RUNNING: &str = r#"{"type":"user","timestamp":"2026-11-02T06:55:14.328Z","toolUseResult":{"backgroundTaskId":"bk1"},"message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"Command running in background with ID: bk1."}]}}"#;

    #[test]
    fn a_background_task_runs_until_its_notice_with_a_status() {
        let log = format!("{STARTED}\n{RUNNING}");
        assert_eq!(background_of(&log, 0), ["Sleep for four seconds"]);
        let notice = |status: &str| {
            format!(
                r#"{{"type":"user","message":{{"content":"<task-notification>\n<task-id>bk1</task-id>\n{status}<summary>x</summary>\n</task-notification>"}}}}"#
            )
        };
        assert_eq!(background_of(&format!("{log}\n{}", notice("")), 0).len(), 1, "no status: it did not say it ended");
        assert!(background_of(&format!("{log}\n{}", notice("<status>completed</status>")), 0).is_empty());
        assert!(background_of(&format!("{log}\n{}", notice("<status>failed</status>")), 0).is_empty());
    }

    #[test]
    fn a_background_task_ends_by_any_way_the_transcript_says() {
        let log = format!("{STARTED}\n{RUNNING}");
        let queued = r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification><tool-use-id>t1</tool-use-id><status>completed</status></task-notification>"}}"#;
        assert!(background_of(&format!("{log}\n{queued}"), 0).is_empty(), "the notice queued mid-turn");
        let killed =
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"KillShell","input":{"shell_id":"bk1"}}]}}"#;
        assert!(background_of(&format!("{log}\n{killed}"), 0).is_empty(), "stopped by the agent");
        let failed = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"no"}]}}"#;
        assert!(background_of(&format!("{STARTED}\n{failed}"), 0).is_empty(), "never started");
    }

    #[test]
    fn a_copy_of_an_api_request_starts_no_background_task() {
        let failed = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"no"}]}}"#;
        let copy = r#"{"type":"api-request-blob","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"sleep 4","run_in_background":true}}]}}"#;
        assert!(background_of(&format!("{STARTED}\n{failed}\n{copy}"), 0).is_empty(), "refused, then copied");
    }

    #[test]
    fn a_background_task_of_an_earlier_run_of_the_agent_is_not_running() {
        let started = chrono::DateTime::parse_from_rfc3339("2026-11-02T06:55:12.609Z").unwrap().timestamp_millis() as u64;
        let log = format!("{STARTED}\n{RUNNING}");
        assert_eq!(background_of(&log, started).len(), 1);
        assert!(background_of(&log, started + 1).is_empty());
    }

    #[test]
    fn a_command_sent_to_the_background_late_is_seen_by_its_result() {
        let call = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm run build","description":"Build it"}}]}}"#;
        assert!(background_of(call, 0).is_empty());
        assert_eq!(background_of(&format!("{call}\n{RUNNING}"), 0), ["Build it"]);
    }

    #[test]
    fn a_window_keeps_the_background_tasks_its_entries_let_go() {
        let rest: Vec<String> = (0..30).map(|n| format!(r#"{{"type":"user","message":{{"content":"m{n}"}}}}"#)).collect();
        let path = temp_log("background-window", &format!("{STARTED}\n{RUNNING}\n{}\n", rest.join("\n")));
        let mut reader = Reader::windowed(Agent::Claude, 3, 3);
        reader.update(&path).unwrap();
        assert!(reader.cut());
        assert_eq!(reader.background(0).len(), 1);
    }

    #[test]
    fn a_moved_transcript_goes_on_where_it_was_read_to() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("agentty-reader-moved-{}", std::process::id()));
        let (old, new) = (dir.join("repo"), dir.join("repo-worktree"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let say = |text: &str| format!(r#"{{"type":"user","message":{{"role":"user","content":"{text}"}}}}"#);
        std::fs::write(old.join("s.jsonl"), format!("{}\n", say("before"))).unwrap();
        let mut reader = Reader::new(Agent::Claude);
        assert_eq!(reader.update(&old.join("s.jsonl")).unwrap().unwrap().0, 0);
        std::fs::rename(old.join("s.jsonl"), new.join("s.jsonl")).unwrap();
        writeln!(std::fs::OpenOptions::new().append(true).open(new.join("s.jsonl")).unwrap(), "{}", say("after")).unwrap();
        let (from, entries) = reader.update(&new.join("s.jsonl")).unwrap().unwrap();
        assert_eq!((from, entries[0].text.as_str()), (1, "after"));
        let _ = std::fs::remove_dir_all(&dir);
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
    fn task_numbers_go_on_past_deleted_ones() {
        let call = |id: &str, name: &str, input: &str| format!(r#"{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}"#);
        let calls = [
            call("a", "TaskCreate", r#"{"subject":"One"}"#),
            call("b", "TaskCreate", r#"{"subject":"Two"}"#),
            call("c", "TaskCreate", r#"{"subject":"Three"}"#),
            call("d", "TaskUpdate", r#"{"taskId":"2","status":"deleted"}"#),
            call("e", "TaskCreate", r#"{"subject":"Four"}"#),
            call("f", "TaskUpdate", r#"{"taskId":"4","status":"completed"}"#),
        ];
        let log = format!(r#"{{"type":"assistant","message":{{"content":[{}]}}}}"#, calls.join(","));
        let entries = claude_entries(lines(&log));
        let Block::Todos(todos) = &entries[0].blocks[0] else { panic!() };
        let shown: Vec<(&str, TodoStatus)> = todos.iter().map(|t| (t.text.as_str(), t.status)).collect();
        assert_eq!(shown, vec![("One", TodoStatus::Pending), ("Three", TodoStatus::Pending), ("Four", TodoStatus::Completed)]);
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
    fn a_growing_transcript_is_read_from_where_it_was() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("agentty-reader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let user = |text: &str| format!(r#"{{"type":"user","message":{{"role":"user","content":"{text}"}}}}"#);
        let say = |text: &str| format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#);
        std::fs::write(&path, format!("{}\n{}\n", user("first"), say("one"))).unwrap();
        let mut reader = Reader::new(Agent::Claude);
        let (from, entries) = reader.update(&path).unwrap().unwrap();
        assert_eq!((from, entries.len()), (0, 2));
        assert!(reader.update(&path).unwrap().is_none(), "nothing new");
        // The reply goes on (it changes), half a line is written, then the rest of it.
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        let next = say("two");
        write!(file, "{}\n{}", next, &user("second")[..10]).unwrap();
        let (from, entries) = reader.update(&path).unwrap().unwrap();
        assert_eq!((from, entries.len()), (1, 1));
        assert_eq!(entries[0].blocks.len(), 2);
        writeln!(file, "{}", &user("second")[10..]).unwrap();
        let (from, entries) = reader.update(&path).unwrap().unwrap();
        assert_eq!((from, entries[0].text.as_str()), (2, "second"));
        // A new file in its place is read from the start.
        std::fs::write(&path, format!("{}\n", user("again"))).unwrap();
        let (from, entries) = reader.update(&path).unwrap().unwrap();
        assert_eq!((from, entries.len()), (0, 1));
        // Lines longer than a piece read at a time, and one ending right at a piece's end.
        let long = "x".repeat(READ_CHUNK + 10);
        let edge = "y".repeat(READ_CHUNK - user("").len() - 1);
        std::fs::write(&path, format!("{}\n{}\n{}\n", user(&edge), user(&long), say("after"))).unwrap();
        let mut reader = Reader::new(Agent::Claude);
        let (_, entries) = reader.update(&path).unwrap().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].text.len(), edge.len());
        assert_eq!(entries[1].text.len(), long.len());
        assert!(reader.partial.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_output_is_clipped() {
        let long = (0..200).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let clipped = clip_output(&long);
        assert_eq!(clipped.lines().count(), OUTPUT_LINES + 1);
        assert!(clipped.ends_with('…'));
    }
}
