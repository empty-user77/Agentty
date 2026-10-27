//! Step-by-step view of a subagent transcript (`<session>/subagents/agent-<id>.jsonl`): every tool
//! call with its target, how long it took, whether it failed and a short excerpt of what it
//! returned, plus the plan the agent keeps (TodoWrite / TaskCreate), the files it changed and the
//! tokens it used. [`TraceReader`] parses only what was appended since its last poll.

use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Lines of a tool's output kept for the expanded view.
const OUTPUT_LINES: usize = 14;
/// Lines of a change kept for the expanded view.
const DIFF_LINES: usize = 60;
/// Characters kept of one line of output, and of the agent's own words.
const LINE_CHARS: usize = 220;
const NOTE_CHARS: usize = 6_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Running,
    Ok,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// What kind of work a tool does, for its icon and its color on the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Read,
    Edit,
    Run,
    Web,
    Agent,
    Plan,
    Other,
}

impl ToolKind {
    pub fn of(tool: &str) -> Self {
        match tool {
            "Read" | "Grep" | "Glob" | "LS" | "NotebookRead" | "LSP" => Self::Read,
            "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => Self::Edit,
            "Bash" | "BashOutput" | "KillShell" | "Monitor" | "PowerShell" => Self::Run,
            "WebFetch" | "WebSearch" => Self::Web,
            "Agent" | "Task" | "SendMessage" => Self::Agent,
            "TodoWrite" | "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet" => Self::Plan,
            _ if tool.starts_with("mcp__") => Self::Web,
            _ => Self::Other,
        }
    }
}

/// One thing the agent did: a tool call, or something it said (`tool` is `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub tool: Option<String>,
    /// File, command, pattern, URL… — what the call was about. For a note: the text.
    pub target: String,
    /// Why, when the tool says so (Bash's `description`).
    pub detail: Option<String>,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub status: StepStatus,
    /// Start (or, for a failure, the end) of what the tool returned.
    pub output: Vec<String>,
    /// Lines of output not kept in `output`.
    pub output_more: usize,
    pub diff: Vec<DiffLine>,
    pub added: usize,
    pub removed: usize,
}

impl Step {
    pub fn kind(&self) -> Option<ToolKind> {
        self.tool.as_deref().map(ToolKind::of)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStatus {
    Pending,
    Active,
    Done,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanItem {
    pub id: Option<String>,
    pub text: String,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the agent's folder when inside it.
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub created: bool,
    pub edits: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// What the model read for one message: its context.
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    fn add(&mut self, other: Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trace {
    /// What the subagent was asked to do.
    pub task: Option<String>,
    pub started_ms: Option<u64>,
    pub last_ms: Option<u64>,
    /// The subagent is done: its transcript ends with a final answer (`end_turn`), or — for a
    /// background agent — with a `SubagentHandback` that reached its caller.
    pub finished: bool,
    /// It was cut off: the agent process it ran in has gone (set by whoever knows when that
    /// process started; see [`TraceReader::mark_stopped_before`]).
    pub stopped: bool,
    /// Its turn was interrupted (Esc in the session that started it): the transcript ends with
    /// Claude Code's "[Request interrupted by user]". A later message sets it to work again.
    pub interrupted: bool,
    pub steps: Vec<Step>,
    pub plan: Vec<PlanItem>,
    pub files: Vec<FileChange>,
    pub errors: usize,
    /// Tokens of finished messages; the message being streamed is in `streaming`.
    tokens_done: Tokens,
    streaming: Option<(String, Tokens)>,
}

impl Trace {
    pub fn tokens(&self) -> Tokens {
        let mut tokens = self.tokens_done;
        if let Some((_, current)) = &self.streaming {
            tokens.add(*current);
        }
        tokens
    }

    /// How full its context is now (the latest message's input).
    pub fn context(&self) -> u64 {
        self.streaming.as_ref().map_or(0, |(_, t)| t.context())
    }

    pub fn tool_steps(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|s| s.tool.is_some())
    }

    /// Tool calls by name, most used first.
    pub fn tool_counts(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for tool in self.tool_steps().filter_map(|s| s.tool.as_ref()) {
            match counts.iter_mut().find(|(name, _)| name == tool) {
                Some((_, n)) => *n += 1,
                None => counts.push((tool.clone(), 1)),
            }
        }
        counts.sort_by_key(|c| std::cmp::Reverse(c.1));
        counts
    }

    /// It does nothing more: done, or cut off.
    pub fn ended(&self) -> bool {
        self.finished || self.stopped || self.interrupted
    }

    /// The call it is waiting on now, if any.
    pub fn current(&self) -> Option<&Step> {
        if self.ended() {
            return None;
        }
        self.steps.iter().rev().find(|s| s.tool.is_some() && s.status == StepStatus::Running)
    }

    /// The last thing it said, once it is done: its report.
    pub fn final_answer(&self) -> Option<&Step> {
        self.finished.then(|| self.steps.last()).flatten().filter(|s| s.tool.is_none())
    }

    /// Plan items done, out of all.
    pub fn plan_progress(&self) -> (usize, usize) {
        (self.plan.iter().filter(|p| p.status == PlanStatus::Done).count(), self.plan.len())
    }
}

/// Follows one transcript file, reading only what was appended since the last [`poll`](Self::poll).
#[derive(Debug, Clone)]
pub struct TraceReader {
    path: PathBuf,
    offset: u64,
    /// The end of the last read when it stopped inside a line.
    carry: Vec<u8>,
    cwd: Option<String>,
    /// Running tool calls: `tool_use_id` → index in `trace.steps`.
    pending: HashMap<String, usize>,
    pub trace: Trace,
}

impl TraceReader {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), offset: 0, carry: Vec::new(), cwd: None, pending: HashMap::new(), trace: Trace::default() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads what was appended; `true` when the trace changed.
    pub fn poll(&mut self) -> std::io::Result<bool> {
        let mut file = std::fs::File::open(&self.path)?;
        let len = file.metadata()?.len();
        if len < self.offset {
            // Rewritten from scratch: start over.
            *self = Self::new(self.path.clone());
        }
        if len == self.offset {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = std::mem::take(&mut self.carry);
        file.take(len - self.offset).read_to_end(&mut bytes)?;
        self.offset = len;
        let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        self.carry = bytes.split_off(complete);
        let mut changed = false;
        for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            if let Ok(v) = serde_json::from_slice::<Value>(line) {
                changed |= self.ingest(&v);
            }
        }
        Ok(changed)
    }

    /// Marks the trace cut off when it is not finished and did its last thing before `since` — when
    /// the agent process that would run it now started (`None`: no agent runs any more). Background
    /// subagents die with the process that started them, and their transcripts just stop.
    pub fn mark_stopped_before(&mut self, since: Option<u64>) {
        let last = self.trace.last_ms.or(self.trace.started_ms).unwrap_or(0);
        self.trace.stopped = !self.trace.finished && !self.trace.interrupted && since.is_none_or(|since| last < since);
    }

    /// Parses a whole transcript from text (tests, one-off reads).
    pub fn from_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Trace {
        let mut reader = Self::new(PathBuf::new());
        for line in lines {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                reader.ingest(&v);
            }
        }
        reader.trace
    }

    fn ingest(&mut self, v: &Value) -> bool {
        if v["isMeta"].as_bool() == Some(true) {
            return false;
        }
        let at = v["timestamp"].as_str().and_then(parse_ms);
        if self.cwd.is_none() {
            self.cwd = v["cwd"].as_str().map(str::to_string);
        }
        let trace = &mut self.trace;
        if let Some(at) = at {
            trace.started_ms.get_or_insert(at);
            trace.last_ms = Some(at);
        }
        let at = at.or(trace.last_ms).unwrap_or(0);
        match v["type"].as_str() {
            Some("user") => self.ingest_user(v, at),
            Some("assistant") => self.ingest_assistant(v, at),
            _ => false,
        }
    }

    fn ingest_user(&mut self, v: &Value, at: u64) -> bool {
        let content = &v["message"]["content"];
        if let Some(text) = content.as_str() {
            if self.trace.task.is_none() {
                self.trace.task = Some(text.trim().to_string());
                return true;
            }
            return self.later_message(text);
        }
        let Some(blocks) = content.as_array() else { return false };
        let results: Vec<&Value> = blocks.iter().filter(|b| b["type"] == "tool_result").collect();
        if results.is_empty() {
            if self.trace.task.is_none() {
                let text: Vec<&str> = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
                if !text.is_empty() {
                    self.trace.task = Some(text.join("\n").trim().to_string());
                    return true;
                }
            }
            let text: Vec<&str> = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
            return self.later_message(&text.join("\n"));
        }
        self.trace.finished = false;
        self.trace.interrupted = false;
        // The structured result belongs to the line's only tool result.
        let structured = (results.len() == 1).then_some(&v["toolUseResult"]);
        for block in results {
            let Some(index) = block["tool_use_id"].as_str().and_then(|id| self.pending.remove(id)) else { continue };
            let text = result_text(&block["content"]);
            let failed = block["is_error"].as_bool() == Some(true) || text.trim_start().starts_with("<tool_use_error>");
            let step = &mut self.trace.steps[index];
            step.end_ms = Some(at.max(step.start_ms));
            step.status = if failed { StepStatus::Error } else { StepStatus::Ok };
            let text = text.replace("<tool_use_error>", "").replace("</tool_use_error>", "");
            (step.output, step.output_more) = excerpt(&text, failed);
            if failed {
                self.trace.errors += 1;
            }
            let tool = step.tool.clone().unwrap_or_default();
            if let Some(structured) = structured {
                self.apply_result(index, &tool, structured, failed);
            }
            // A background agent's report reaching its caller is its end.
            if tool == "SubagentHandback" && !failed {
                self.trace.finished = true;
            }
        }
        true
    }

    /// What only the structured result tells: the real change of an edit, the id of a new task.
    fn apply_result(&mut self, index: usize, tool: &str, structured: &Value, failed: bool) {
        match tool {
            "Edit" | "MultiEdit" | "Write" if !failed => {
                let created = structured["type"] == "create";
                let mut diff = Vec::new();
                for hunk in structured["structuredPatch"].as_array().into_iter().flatten() {
                    for line in hunk["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                        diff.push(diff_line(line));
                    }
                }
                let step = &mut self.trace.steps[index];
                if !diff.is_empty() {
                    step.added = diff.iter().filter(|l| l.kind == DiffKind::Added).count();
                    step.removed = diff.iter().filter(|l| l.kind == DiffKind::Removed).count();
                    diff.truncate(DIFF_LINES);
                    step.diff = diff;
                }
                let (added, removed, path) = (step.added, step.removed, step.target.clone());
                self.record_file(&path, added, removed, created);
            }
            "TaskCreate" => {
                let id = structured["task"]["id"].as_str().map(str::to_string).or_else(|| {
                    let text = self.trace.steps[index].output.first()?;
                    text.split('#').nth(1)?.split(|c: char| !c.is_ascii_digit()).next().map(str::to_string)
                });
                let subject = self.trace.steps[index].target.clone();
                if let Some(item) = self.trace.plan.iter_mut().rev().find(|p| p.id.is_none() && p.text == subject) {
                    item.id = id;
                }
            }
            _ => {}
        }
    }

    fn record_file(&mut self, path: &str, added: usize, removed: usize, created: bool) {
        let files = &mut self.trace.files;
        match files.iter_mut().find(|f| f.path == path) {
            Some(file) => {
                file.added += added;
                file.removed += removed;
                file.edits += 1;
            }
            None => files.push(FileChange { path: path.to_string(), added, removed, created, edits: 1 }),
        }
    }

    /// A user message after the task: Claude Code's mark of an interrupted turn ends it; anything
    /// else (its caller writing to it after it handed back) sets it to work again.
    fn later_message(&mut self, text: &str) -> bool {
        let before = (self.trace.finished, self.trace.interrupted);
        if text.trim_start().starts_with("[Request interrupted") {
            self.trace.interrupted = true;
        } else {
            self.trace.finished = false;
            self.trace.interrupted = false;
        }
        before != (self.trace.finished, self.trace.interrupted)
    }

    fn ingest_assistant(&mut self, v: &Value, at: u64) -> bool {
        let message = &v["message"];
        self.count_tokens(message);
        self.trace.interrupted = false;
        self.trace.finished = message["stop_reason"] == "end_turn";
        let Some(blocks) = message["content"].as_array() else { return false };
        let mut changed = false;
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => {
                    let text = block["text"].as_str().unwrap_or_default().trim();
                    if text.is_empty() {
                        continue;
                    }
                    changed = true;
                    // Streaming writes one line per block: text right after text is one note.
                    match self.trace.steps.last_mut() {
                        Some(last) if last.tool.is_none() && last.start_ms + 60_000 > at => {
                            last.target.push('\n');
                            last.target.push_str(text);
                            last.target = truncate(&last.target, NOTE_CHARS);
                        }
                        _ => self.trace.steps.push(note(text, at)),
                    }
                }
                Some("tool_use") => {
                    changed = true;
                    self.start_tool(block, at);
                }
                _ => {}
            }
        }
        changed
    }

    fn start_tool(&mut self, block: &Value, at: u64) {
        let name = block["name"].as_str().unwrap_or("tool").to_string();
        let input = &block["input"];
        let (target, detail) = describe(&name, input, self.cwd.as_deref());
        let mut step = Step {
            tool: Some(name.clone()),
            target: target.clone(),
            detail,
            start_ms: at,
            end_ms: None,
            status: StepStatus::Running,
            output: Vec::new(),
            output_more: 0,
            diff: Vec::new(),
            added: 0,
            removed: 0,
        };
        // Until the result says otherwise, the change is what was asked for.
        match name.as_str() {
            "Edit" => {
                let old: Vec<&str> = input["old_string"].as_str().unwrap_or_default().lines().collect();
                let new: Vec<&str> = input["new_string"].as_str().unwrap_or_default().lines().collect();
                (step.removed, step.added) = (old.len(), new.len());
                step.diff = old
                    .iter()
                    .map(|l| DiffLine { kind: DiffKind::Removed, text: l.to_string() })
                    .chain(new.iter().map(|l| DiffLine { kind: DiffKind::Added, text: l.to_string() }))
                    .take(DIFF_LINES)
                    .collect();
            }
            "Write" => {
                let content = input["content"].as_str().unwrap_or_default();
                step.added = content.lines().count();
                step.diff = content.lines().take(DIFF_LINES).map(|l| DiffLine { kind: DiffKind::Added, text: l.to_string() }).collect();
            }
            "TodoWrite" => {
                self.trace.plan = input["todos"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|todo| PlanItem {
                        id: None,
                        text: todo["content"].as_str().unwrap_or_default().to_string(),
                        status: plan_status(todo["status"].as_str()),
                    })
                    .collect();
            }
            "TaskCreate" => {
                self.trace.plan.push(PlanItem { id: None, text: target.clone(), status: PlanStatus::Pending });
            }
            "TaskUpdate" => {
                if let Some(id) = input["taskId"].as_str() {
                    if input["status"] == "deleted" {
                        self.trace.plan.retain(|p| p.id.as_deref() != Some(id));
                    } else if let Some(item) = self.trace.plan.iter_mut().find(|p| p.id.as_deref() == Some(id)) {
                        if let Some(status) = input["status"].as_str() {
                            item.status = plan_status(Some(status));
                        }
                        if let Some(subject) = input["subject"].as_str() {
                            item.text = subject.to_string();
                        }
                    }
                }
            }
            _ => {}
        }
        if let Some(id) = block["id"].as_str() {
            self.pending.insert(id.to_string(), self.trace.steps.len());
        }
        self.trace.steps.push(step);
    }

    /// Every streamed block repeats its message's usage, so a message counts once, with its last numbers.
    fn count_tokens(&mut self, message: &Value) {
        let usage = &message["usage"];
        if usage.is_null() {
            return;
        }
        let n = |k: &str| usage[k].as_u64().unwrap_or(0);
        let tokens = Tokens {
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
        };
        let id = message["id"].as_str().unwrap_or_default().to_string();
        match &mut self.trace.streaming {
            Some((current, last)) if *current == id => *last = tokens,
            streaming => {
                if let Some((_, done)) = streaming.replace((id, tokens)) {
                    self.trace.tokens_done.add(done);
                }
            }
        }
    }
}

fn note(text: &str, at: u64) -> Step {
    Step {
        tool: None,
        target: truncate(text, NOTE_CHARS),
        detail: None,
        start_ms: at,
        end_ms: Some(at),
        status: StepStatus::Ok,
        output: Vec::new(),
        output_more: 0,
        diff: Vec::new(),
        added: 0,
        removed: 0,
    }
}

fn plan_status(status: Option<&str>) -> PlanStatus {
    match status {
        Some("completed") => PlanStatus::Done,
        Some("in_progress") => PlanStatus::Active,
        _ => PlanStatus::Pending,
    }
}

/// (what the call is about, why) for one tool call.
fn describe(tool: &str, input: &Value, cwd: Option<&str>) -> (String, Option<String>) {
    let s = |k: &str| input[k].as_str().map(|s| one_line(s, 240));
    let path = |k: &str| input[k].as_str().map(|p| relative(p, cwd));
    let target = match tool {
        // `cd <folder> && npm test` (or `cd <folder>; npm test`) → `npm test`: the folder is the
        // same in almost every call.
        "Bash" | "PowerShell" => s("command").map(|c| {
            let rest = c.strip_prefix("cd ").and_then(|rest| {
                let cut = [" && ", "; "].iter().filter_map(|sep| rest.find(sep).map(|at| at + sep.len())).min()?;
                Some(rest[cut..].trim_start())
            });
            match rest {
                Some(rest) if !rest.is_empty() => rest.to_string(),
                _ => c,
            }
        }),
        "Read" | "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => path("file_path").or_else(|| path("notebook_path")),
        "Grep" => s("pattern").map(|p| match path("path") {
            Some(dir) => format!("{p}  in {dir}"),
            None => p,
        }),
        "Glob" => s("pattern"),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        "Agent" | "Task" => s("description"),
        "TaskCreate" => s("subject"),
        "TaskUpdate" => input["taskId"].as_str().map(|id| format!("#{id} → {}", input["status"].as_str().unwrap_or("updated"))),
        "TodoWrite" => input["todos"].as_array().map(|t| format!("{} items", t.len())),
        "Skill" => s("skill"),
        _ => None,
    }
    .or_else(|| input.as_object()?.values().find_map(|v| v.as_str()).map(|s| one_line(s, 240)))
    .unwrap_or_default();
    let detail = match tool {
        "Bash" | "PowerShell" => s("description"),
        "Read" => input["offset"].as_u64().map(|o| format!("from line {o}")),
        _ => None,
    };
    (target, detail)
}

fn relative(path: &str, cwd: Option<&str>) -> String {
    cwd.and_then(|cwd| path.strip_prefix(cwd)).map(|rest| rest.trim_start_matches('/').to_string()).unwrap_or_else(|| path.to_string())
}

fn result_text(content: &Value) -> String {
    match content {
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
        _ => String::new(),
    }
}

/// The lines worth showing of a tool's output: the start, or for a failure the end, where the
/// error usually is. Returns them and how many were left out.
fn excerpt(text: &str, failed: bool) -> (Vec<String>, usize) {
    let lines: Vec<&str> = text.trim_end().lines().filter(|l| !l.trim().is_empty() || !failed).collect();
    let kept: Vec<String> = if failed && lines.len() > OUTPUT_LINES {
        lines[lines.len() - OUTPUT_LINES..].iter().map(|l| truncate(l, LINE_CHARS)).collect()
    } else {
        lines.iter().take(OUTPUT_LINES).map(|l| truncate(l, LINE_CHARS)).collect()
    };
    let more = lines.len().saturating_sub(kept.len());
    (kept, more)
}

fn diff_line(line: &str) -> DiffLine {
    let (kind, text) = match line.chars().next() {
        Some('+') => (DiffKind::Added, &line[1..]),
        Some('-') => (DiffKind::Removed, &line[1..]),
        Some(' ') => (DiffKind::Context, &line[1..]),
        _ => (DiffKind::Context, line),
    };
    DiffLine { kind, text: truncate(text, LINE_CHARS) }
}

fn parse_ms(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().and_then(|t| u64::try_from(t.timestamp_millis()).ok())
}

fn one_line(s: &str, max: usize) -> String {
    truncate(&s.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: Value) -> String {
        v.to_string()
    }

    fn sample() -> Vec<String> {
        let at = |s: u32| format!("2026-10-11T12:00:{s:02}.000Z");
        vec![
            line(serde_json::json!({"type": "user", "timestamp": at(0), "cwd": "/repo", "message": {"content": "Build the shop"}})),
            line(serde_json::json!({"type": "assistant", "timestamp": at(1), "message": {"id": "m1", "stop_reason": "tool_use",
                "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 100},
                "content": [{"type": "text", "text": "Reading the spec first."}]}})),
            line(serde_json::json!({"type": "assistant", "timestamp": at(1), "message": {"id": "m1", "stop_reason": "tool_use",
                "usage": {"input_tokens": 10, "output_tokens": 20, "cache_read_input_tokens": 100},
                "content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/repo/SPEC.md"}}]}})),
            line(serde_json::json!({"type": "user", "timestamp": at(2), "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "1\t# Shop\n2\tspec"}]}})),
            line(serde_json::json!({"type": "assistant", "timestamp": at(3), "message": {"id": "m2", "stop_reason": "tool_use",
                "usage": {"input_tokens": 1, "output_tokens": 7},
                "content": [{"type": "tool_use", "id": "t2", "name": "TodoWrite", "input": {"todos": [
                    {"content": "Write page", "status": "in_progress"}, {"content": "Run build", "status": "pending"}]}}]}})),
            line(serde_json::json!({"type": "user", "timestamp": at(3), "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t2", "content": "ok"}]}})),
            line(serde_json::json!({"type": "assistant", "timestamp": at(4), "message": {"id": "m3", "stop_reason": "tool_use",
                "content": [{"type": "tool_use", "id": "t3", "name": "Edit", "input": {"file_path": "/repo/app/page.tsx", "old_string": "a", "new_string": "b\nc"}}]}})),
            line(
                serde_json::json!({"type": "user", "timestamp": at(5), "toolUseResult": {"structuredPatch": [{"lines": [" x", "-a", "+b", "+c"]}]},
                "message": {"content": [{"type": "tool_result", "tool_use_id": "t3", "content": "updated"}]}}),
            ),
            line(serde_json::json!({"type": "assistant", "timestamp": at(6), "message": {"id": "m4", "stop_reason": "tool_use",
                "content": [{"type": "tool_use", "id": "t4", "name": "Bash", "input": {"command": "cd /repo && npm run build", "description": "Build"}}]}})),
            line(serde_json::json!({"type": "user", "timestamp": at(20), "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t4", "is_error": true, "content": "compiling\nType error: x is not y"}]}})),
            line(serde_json::json!({"type": "assistant", "timestamp": at(21), "message": {"id": "m5", "stop_reason": "tool_use",
                "content": [{"type": "tool_use", "id": "t5", "name": "Bash", "input": {"command": "cd /repo/app; npx vitest run"}}]}})),
        ]
    }

    #[test]
    fn follows_tool_calls_with_outcomes() {
        let lines = sample();
        let trace = TraceReader::from_lines(lines.iter().map(String::as_str));
        assert_eq!(trace.task.as_deref(), Some("Build the shop"));
        assert_eq!(trace.tool_steps().count(), 5);
        let read = trace.tool_steps().next().unwrap();
        assert_eq!((read.target.as_str(), read.status, read.end_ms.unwrap() - read.start_ms), ("SPEC.md", StepStatus::Ok, 1_000));
        assert_eq!(trace.steps[0].tool, None, "what the agent said is a step too");
        let build = trace.tool_steps().nth(3).unwrap();
        assert_eq!((build.target.as_str(), build.status, build.detail.as_deref()), ("npm run build", StepStatus::Error, Some("Build")));
        assert_eq!(trace.tool_steps().nth(4).unwrap().target, "npx vitest run");
        assert_eq!(build.output.last().map(String::as_str), Some("Type error: x is not y"));
        assert_eq!(trace.errors, 1);
        assert_eq!(trace.current().and_then(|s| s.tool.as_deref()), Some("Bash"));
        assert!(!trace.finished);
        assert_eq!(trace.tool_counts()[0], ("Bash".to_string(), 2));
    }

    #[test]
    fn edits_use_the_real_patch_and_add_up_per_file() {
        let lines = sample();
        let trace = TraceReader::from_lines(lines.iter().map(String::as_str));
        let edit = trace.tool_steps().find(|s| s.tool.as_deref() == Some("Edit")).unwrap();
        assert_eq!((edit.added, edit.removed, edit.diff.len()), (2, 1, 4));
        assert_eq!(trace.files, vec![FileChange { path: "app/page.tsx".into(), added: 2, removed: 1, created: false, edits: 1 }]);
    }

    #[test]
    fn plan_and_tokens() {
        let lines = sample();
        let trace = TraceReader::from_lines(lines.iter().map(String::as_str));
        assert_eq!(trace.plan.len(), 2);
        assert_eq!(trace.plan[0].status, PlanStatus::Active);
        assert_eq!(trace.plan_progress(), (0, 2));
        // m1 counted once with its last usage (10 + 20 + 100), m2 adds 8.
        assert_eq!(trace.tokens().total(), 138);
    }

    #[test]
    fn task_create_and_update_track_ids() {
        let lines = [
            line(serde_json::json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "a", "name": "TaskCreate", "input": {"subject": "Scaffold"}}]}})),
            line(serde_json::json!({"type": "user", "toolUseResult": {"task": {"id": "7"}}, "message": {"content": [
                {"type": "tool_result", "tool_use_id": "a", "content": "Task #7 created successfully: Scaffold"}]}})),
            line(serde_json::json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "b", "name": "TaskUpdate", "input": {"taskId": "7", "status": "completed"}}]}})),
        ];
        let trace = TraceReader::from_lines(lines.iter().map(String::as_str));
        assert_eq!(trace.plan, vec![PlanItem { id: Some("7".into()), text: "Scaffold".into(), status: PlanStatus::Done }]);
    }

    #[test]
    fn a_handback_that_reached_its_caller_ends_it_until_the_caller_writes_again() {
        let lines = vec![
            line(serde_json::json!({"type": "user", "message": {"content": "Compare the notes"}})),
            line(serde_json::json!({"type": "assistant", "message": {"stop_reason": "tool_use", "content": [
                {"type": "tool_use", "id": "h", "name": "SubagentHandback", "input": {"message": "Report"}}]}})),
        ];
        let mut reader = TraceReader::new(PathBuf::new());
        for l in &lines {
            reader.ingest(&serde_json::from_str(l).unwrap());
        }
        assert!(!reader.trace.finished);
        reader.ingest(&serde_json::json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "h", "content": "{\"success\":true}"}]}}));
        assert!(reader.trace.finished);
        assert!(reader.trace.current().is_none());
        reader.ingest(&serde_json::json!({"type": "user", "message": {"content": "The coordinator sent a message: fix it"}}));
        assert!(!reader.trace.finished);
    }

    #[test]
    fn an_interrupted_turn_ends_it_until_it_works_again() {
        let mut reader = TraceReader::new(PathBuf::new());
        reader.ingest(&serde_json::json!({"type": "user", "message": {"content": "Write mars.md"}}));
        reader.ingest(&serde_json::json!({"type": "assistant", "message": {"stop_reason": "tool_use", "content": [
            {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "mkdir -p planets"}}]}}));
        assert!(!reader.trace.ended());
        reader.ingest(&serde_json::json!({"type": "user", "message": {"content": [
            {"type": "text", "text": "[Request interrupted by user]"}]}}));
        assert!(reader.trace.interrupted && reader.trace.ended());
        assert!(reader.trace.current().is_none());
        // The process it ran in is still the same: not cut off, interrupted.
        reader.mark_stopped_before(Some(0));
        assert!(!reader.trace.stopped);
        reader.ingest(&serde_json::json!({"type": "user", "message": {"content": "Carry on"}}));
        assert!(!reader.trace.ended());
    }

    #[test]
    fn finished_run_has_a_final_answer() {
        let mut lines = sample();
        lines.push(line(serde_json::json!({"type": "user", "timestamp": "2026-10-11T12:00:30.000Z", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t5", "content": "passed"}]}})));
        lines.push(line(serde_json::json!({"type": "assistant", "timestamp": "2026-10-11T12:00:31.000Z", "message": {"id": "m6",
            "stop_reason": "end_turn", "content": [{"type": "text", "text": "Done: pages built."}]}})));
        let trace = TraceReader::from_lines(lines.iter().map(String::as_str));
        assert!(trace.finished);
        assert!(trace.current().is_none());
        assert_eq!(trace.final_answer().map(|s| s.target.as_str()), Some("Done: pages built."));
    }

    #[test]
    fn poll_reads_only_what_was_appended() {
        let dir = std::env::temp_dir().join(format!("agentty-trace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent-x.jsonl");
        let lines = sample();
        // The last line is cut in half, as when the agent is still writing it.
        let (head, tail) = lines[1].split_at(20);
        std::fs::write(&path, format!("{}\n{head}", lines[0])).unwrap();
        let mut reader = TraceReader::new(&path);
        assert!(reader.poll().unwrap());
        assert_eq!(reader.trace.steps.len(), 0);
        assert!(!reader.poll().unwrap(), "nothing new");
        std::fs::write(&path, format!("{}\n{head}{tail}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(reader.poll().unwrap());
        assert_eq!(reader.trace.steps.len(), 2);
        assert_eq!(reader.trace.current().map(|s| s.target.as_str()), Some("SPEC.md"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
