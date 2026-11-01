//! A pane's conversation for the remote apps (`GET /api/chat`): what `chat_log` reads from its
//! transcript, on the connection's own thread, never the UI's. The pane decides which transcript:
//! a request names a pane only, and the session comes from the app, so nothing a page sends reaches
//! a path. What goes out is bounded, and shown as a page may show it (`page_text`).

use agentty_bridge::chat_log::{self, Block, Entry, Reader, TodoStatus};
use agentty_bridge::fsutil::truncate_chars;
use agentty_bridge::model::{Agent, Role};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Entries of a conversation sent at most, and blocks of one reply.
const MAX_ENTRIES: usize = 200;
const MAX_BLOCKS: usize = 150;
/// Longest text of a message or a reply's block.
const MAX_TEXT: usize = 16_000;
/// Longest name or short label (a tool's name, a to-do).
const MAX_LABEL: usize = 500;
/// A whole answer at most; older entries go first. A poll every second or two may fetch this.
pub const MAX_BODY: usize = 512 * 1024;
/// Kept for what goes around the entries.
const HEAD_ROOM: usize = 16 * 1024;
/// How often a session whose file is not there yet is looked for.
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// Which conversation a pane shows. From the app, never from a request.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatSource {
    pub agent: Agent,
    /// Its session, once it has one.
    pub session: Option<String>,
}

/// What the agent's screen says, read on the UI thread for the panes an app has open.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatLive {
    pub busy: bool,
    /// The screen shows something only the terminal can answer.
    pub needs_terminal: bool,
    pub model: Option<String>,
    pub mode: Option<&'static str>,
    pub effort: Option<&'static str>,
    pub question: Option<ChatQuestion>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatQuestion {
    pub text: String,
    /// The options, top to bottom: the first is key `1`.
    pub options: Vec<String>,
    /// The one highlighted.
    pub current: usize,
}

/// What the pane is like now, for an answer.
pub struct ChatState<'a> {
    pub pane: u64,
    pub status: &'a str,
    pub working: bool,
    pub needs_user: bool,
    pub elapsed: Option<u64>,
    pub live: Option<&'a ChatLive>,
}

/// The entries as JSON, made again only when the conversation changed.
struct Rendered {
    json: String,
    hash: u64,
    truncated: bool,
}

/// One pane's conversation being followed: each poll reads only what the transcript gained.
pub struct Follow {
    source: Option<ChatSource>,
    path: Option<PathBuf>,
    looked: Option<Instant>,
    reader: Option<Reader>,
    /// What Gemini CLI and Antigravity said, read whole (their logs are small).
    said: Vec<Entry>,
    said_cut: bool,
    size: Option<u64>,
    rendered: Option<Rendered>,
}

impl Follow {
    pub fn new() -> Self {
        Follow { source: None, path: None, looked: None, reader: None, said: Vec::new(), said_cut: false, size: None, rendered: None }
    }

    /// Follows `path` as `source`'s transcript, without looking it up.
    #[cfg(test)]
    pub fn at_path(source: ChatSource, path: PathBuf) -> Self {
        Follow { source: Some(source), path: Some(path), ..Follow::new() }
    }

    fn refresh(&mut self, source: &ChatSource) {
        // Another agent or another session in the pane (`/clear`): what was read is not its own.
        if self.source.as_ref() != Some(source) {
            *self = Follow { source: Some(source.clone()), ..Follow::new() };
        }
        let Some(session) = &source.session else { return };
        if self.path.is_none() && self.looked.is_none_or(|at| at.elapsed() >= LOOK_EVERY) {
            self.looked = Some(Instant::now());
            self.path = chat_log::transcript_file(source.agent, session);
        }
        let Some(path) = self.path.clone() else { return };
        // A log that can't be read now keeps what was read of it; the next poll tries again.
        if matches!(source.agent, Agent::Gemini | Agent::Agy) {
            let size = chat_log::transcript_size(&path);
            if size != self.size {
                self.size = size;
                let turns = match source.agent {
                    Agent::Agy => agentty_bridge::agy::transcript(&path),
                    _ => agentty_bridge::gemini::transcript(&path),
                };
                if let Ok((_, turns)) = turns {
                    self.said_cut = turns.len() > MAX_ENTRIES;
                    self.said = turns[turns.len().saturating_sub(MAX_ENTRIES)..]
                        .iter()
                        .map(|turn| match turn.role {
                            Role::User => Entry { role: Role::User, text: turn.text.clone(), blocks: Vec::new() },
                            Role::Assistant => {
                                Entry { role: Role::Assistant, text: String::new(), blocks: vec![Block::Text(turn.text.clone())] }
                            }
                        })
                        .collect();
                    self.rendered = None;
                }
            }
            return;
        }
        let reader = self.reader.get_or_insert_with(|| Reader::windowed(source.agent, MAX_ENTRIES, MAX_BLOCKS));
        if matches!(reader.update(&path), Ok(Some(_))) {
            self.rendered = None;
        }
    }

    /// `(rev, body)` of the conversation now. `rev` changes with what the answer says, except how
    /// long the turn has run, which an app counts up itself.
    pub fn answer(&mut self, source: &ChatSource, state: &ChatState, home: &str) -> (String, Vec<u8>) {
        self.refresh(source);
        let rendered = self.rendered.get_or_insert_with(|| {
            let (entries, cut) = match &self.reader {
                Some(reader) => (reader.entries(), reader.cut()),
                None => (self.said.as_slice(), self.said_cut),
            };
            render(entries, cut, home)
        });
        let live = state.live;
        let mut head = json!({
            "pane": state.pane,
            "agent": source.agent.id(),
            "busy": live.map_or(state.working, |live| live.busy),
            "needsUser": state.needs_user,
            "needsTerminal": live.is_some_and(|live| live.needs_terminal),
            "status": state.status,
            "model": live.and_then(|live| live.model.as_deref()),
            "mode": live.and_then(|live| live.mode),
            "effort": live.and_then(|live| live.effort),
            "truncated": rendered.truncated,
        });
        if let Some(question) = live.and_then(|live| live.question.as_ref()) {
            let options: Vec<Value> = question
                .options
                .iter()
                .enumerate()
                .map(|(index, label)| json!({ "key": (index + 1).to_string(), "label": truncate_chars(label, MAX_LABEL) }))
                .collect();
            head["question"] =
                json!({ "text": truncate_chars(&question.text, MAX_LABEL), "current": question.current, "options": options });
        }
        clean_value(&mut head, home);
        let mut hasher = DefaultHasher::new();
        head.to_string().hash(&mut hasher);
        rendered.hash.hash(&mut hasher);
        let rev = format!("{:016x}", hasher.finish());
        head["rev"] = json!(rev);
        head["elapsed"] = json!(state.elapsed);
        let head = head.to_string();
        let mut body = Vec::with_capacity(head.len() + rendered.json.len() + 12);
        body.extend_from_slice(&head.as_bytes()[..head.len() - 1]);
        body.extend_from_slice(b",\"entries\":");
        body.extend_from_slice(rendered.json.as_bytes());
        body.push(b'}');
        (rev, body)
    }
}

/// The newest of `entries` that fit, oldest first, in what an answer may hold.
fn render(entries: &[Entry], mut truncated: bool, home: &str) -> Rendered {
    let mut budget = MAX_BODY - HEAD_ROOM;
    let mut rows: Vec<String> = Vec::new();
    'entries: for entry in entries.iter().rev() {
        for mut row in entry_rows(entry).into_iter().rev() {
            clean_value(&mut row, home);
            let text = row.to_string();
            if text.len() < budget {
                budget -= text.len() + 1;
                rows.push(text);
                continue;
            }
            // Too long whole (a long turn's steps): its newest blocks, as far as they fit.
            if let Some(blocks) = row["blocks"].as_array() {
                let mut kept: Vec<&Value> = Vec::new();
                let mut room = budget.saturating_sub(64);
                for block in blocks.iter().rev() {
                    let size = block.to_string().len() + 1;
                    if size >= room {
                        break;
                    }
                    room -= size;
                    kept.push(block);
                }
                if !kept.is_empty() {
                    kept.reverse();
                    rows.push(json!({ "role": "assistant", "blocks": kept }).to_string());
                }
            }
            truncated = true;
            break 'entries;
        }
    }
    rows.reverse();
    let json = format!("[{}]", rows.join(","));
    let mut hasher = DefaultHasher::new();
    json.hash(&mut hasher);
    truncated.hash(&mut hasher);
    Rendered { hash: hasher.finish(), json, truncated }
}

/// An entry as the page reads it: a user's turn cleaned as the chat view cleans it (instructions to
/// a lead, paste tags and the harness's own blocks left out), and each reply with its blocks.
fn entry_rows(entry: &Entry) -> Vec<Value> {
    match entry.role {
        Role::User => crate::workbench::user_parts(&entry.text)
            .into_iter()
            .map(|(role, text)| json!({ "role": role, "text": truncate_chars(&text, MAX_TEXT) }))
            .collect(),
        Role::Assistant if entry.blocks.is_empty() => Vec::new(),
        Role::Assistant => vec![json!({ "role": "assistant", "blocks": entry.blocks.iter().map(block_json).collect::<Vec<_>>() })],
    }
}

fn block_json(block: &Block) -> Value {
    match block {
        Block::Text(text) => json!({ "type": "text", "text": truncate_chars(text, MAX_TEXT) }),
        Block::Tool(step) => json!({
            "type": "tool",
            "id": truncate_chars(&step.id, MAX_LABEL),
            "name": truncate_chars(&step.name, MAX_LABEL),
            "description": step.description,
            "detail": step.detail,
            "output": step.output,
            "failed": step.failed,
            "diff": step.diff,
        }),
        Block::Todos(todos) => json!({
            "type": "todos",
            "items": todos.iter().map(|todo| json!({
                "text": truncate_chars(&todo.text, MAX_LABEL),
                "active": todo.active.as_deref().map(|active| truncate_chars(active, MAX_LABEL)),
                "status": match todo.status {
                    TodoStatus::Pending => "pending",
                    TodoStatus::InProgress => "in_progress",
                    TodoStatus::Completed => "completed",
                },
            })).collect::<Vec<_>>(),
        }),
    }
}

/// Text as it may leave the computer: no control characters or terminal escapes (lines and tabs
/// stay), the home folder as `~`, and what looks like a credential masked. It is the owner's own
/// session, as the terminal is; this is a second layer for what the shapes catch, not a promise that
/// nothing sensitive an agent printed ever passes.
pub fn page_text(text: &str, home: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // A colour or a cursor move: up to its last character.
            if chars.next_if_eq(&'[').is_some() {
                while chars.next().is_some_and(|c| !('@'..='~').contains(&c)) {}
            }
        } else if !c.is_control() || matches!(c, '\n' | '\t') {
            plain.push(c);
        }
    }
    agentty_bridge::sync::mask::mask(&without_home(&plain, home))
}

/// `text` with the folder `home` as `~`, wherever a path starts with it; a folder whose name only
/// begins the same is not in it.
fn without_home(text: &str, home: &str) -> String {
    if home.len() < 2 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(home) {
        let after = &rest[at + home.len()..];
        let same_name = after.chars().next().is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
        out.push_str(&rest[..at]);
        out.push_str(if same_name { home } else { "~" });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Every string in `value` as `page_text` leaves it.
pub fn clean_value(value: &mut Value, home: &str) {
    match value {
        Value::String(text) => *text = page_text(text, home),
        Value::Array(items) => items.iter_mut().for_each(|item| clean_value(item, home)),
        Value::Object(fields) => fields.values_mut().for_each(|field| clean_value(field, home)),
        _ => {}
    }
}

/// The home folder as `page_text` looks for it.
pub fn home_text() -> String {
    home_of(&agentty_bridge::fsutil::home())
}

fn home_of(home: &Path) -> String {
    home.display().to_string().trim_end_matches(['/', '\\']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn state<'a>(live: Option<&'a ChatLive>) -> ChatState<'a> {
        ChatState { pane: 7, status: "Working", working: true, needs_user: false, elapsed: Some(3), live }
    }

    fn temp_log(name: &str, text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("agentty-remote-chat-{name}-{}.jsonl", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn source() -> ChatSource {
        ChatSource { agent: Agent::Claude, session: Some("session".into()) }
    }

    #[test]
    fn page_text_leaves_no_escapes_home_or_credentials() {
        assert_eq!(page_text("a\u{1b}[31mred\u{1b}[0m\u{7}\r\n\tb", HOME), "ared\n\tb");
        let (longer, bak) = (format!("{HOME}2"), format!("{HOME}.bak"));
        assert_eq!(page_text(&format!("cd {HOME}/x; ls {HOME} {longer} {bak}"), HOME), format!("cd ~/x; ls ~ {longer} {bak}"));
        let token = format!("ghp_{}", "example_not_a_real_token_".repeat(2));
        let shown = page_text(&format!("token {token} end"), HOME);
        assert!(!shown.contains("example_not_a_real"), "{shown}");
        assert_eq!(page_text("x", ""), "x");
        assert_eq!(page_text("\u{1b}[", HOME), "", "an escape cut short");
    }

    #[test]
    fn answers_follow_the_log_and_rev_ignores_only_the_elapsed_time() {
        let log = "{\"type\":\"user\",\"message\":{\"content\":\"build /Users/me/x\"}}\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"On it.\"},{\"type\":\"tool_use\",\"id\":\"b1\",\"name\":\"Bash\",\"input\":{\"command\":\"ls /Users/me\"}},{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"TodoWrite\",\"input\":{\"todos\":[{\"content\":\"API\",\"status\":\"in_progress\",\"activeForm\":\"Building\"}]}}]}}\n";
        let path = temp_log("follow", log);
        let mut follow = Follow::at_path(source(), path.clone());
        let (rev, body) = follow.answer(&source(), &state(None), HOME);
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["agent"], "claude");
        assert_eq!(value["rev"], rev);
        assert_eq!(value["elapsed"], 3);
        assert_eq!(value["busy"], true);
        assert_eq!(value["entries"][0], json!({ "role": "user", "text": "build ~/x" }));
        let blocks = &value["entries"][1]["blocks"];
        assert_eq!(blocks[0], json!({ "type": "text", "text": "On it." }));
        assert_eq!(blocks[1]["type"], "tool");
        assert_eq!(blocks[1]["name"], "Bash");
        assert_eq!(blocks[1]["detail"], "ls ~");
        assert_eq!(blocks[2]["items"][0], json!({ "text": "API", "active": "Building", "status": "in_progress" }));

        let later = ChatState { elapsed: Some(9), ..state(None) };
        assert_eq!(follow.answer(&source(), &later, HOME).0, rev, "only the time passed");
        let live = ChatLive {
            question: Some(ChatQuestion { text: "Run it?".into(), options: vec!["Yes".into(), "No".into()], current: 1 }),
            ..Default::default()
        };
        let (asked_rev, asked) = follow.answer(&source(), &state(Some(&live)), HOME);
        assert_ne!(asked_rev, rev, "a question changes the answer");
        let asked: Value = serde_json::from_slice(&asked).unwrap();
        assert_eq!(
            asked["question"],
            json!({ "text": "Run it?", "current": 1, "options": [{ "key": "1", "label": "Yes" }, { "key": "2", "label": "No" }] })
        );

        std::fs::write(&path, format!("{log}{{\"type\":\"user\",\"message\":{{\"content\":\"more\"}}}}\n")).unwrap();
        let (grown_rev, grown) = follow.answer(&source(), &state(None), HOME);
        assert_ne!(grown_rev, rev);
        let grown: Value = serde_json::from_slice(&grown).unwrap();
        assert_eq!(grown["entries"].as_array().unwrap().len(), 3);

        // Another session in the pane: nothing of the last one stays.
        let other = ChatSource { agent: Agent::Claude, session: None };
        let (_, gone) = follow.answer(&other, &state(None), HOME);
        let gone: Value = serde_json::from_slice(&gone).unwrap();
        assert_eq!(gone["entries"], json!([]));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_big_conversation_stays_within_the_body_limit_newest_first() {
        let words = "w".repeat(15_000);
        let turn = |n: usize| {
            format!(
                "{{\"type\":\"user\",\"message\":{{\"content\":\"ask {n}\"}}}}\n{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{words}\"}}]}}}}\n"
            )
        };
        let path = temp_log("big", &(0..300).map(turn).collect::<String>());
        let mut follow = Follow::at_path(source(), path.clone());
        let (_, body) = follow.answer(&source(), &state(None), HOME);
        assert!(body.len() <= MAX_BODY, "{}", body.len());
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["truncated"], true);
        let entries = value["entries"].as_array().unwrap();
        assert!(entries.len() < MAX_ENTRIES);
        assert_eq!(entries[entries.len() - 2]["text"], "ask 299", "the newest are kept");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn one_long_reply_sends_its_newest_steps() {
        let step = |n: usize| {
            let long = "x".repeat(1_900);
            format!(
                "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{n} {long}\"}},{{\"type\":\"text\",\"text\":\"{long}\"}},{{\"type\":\"text\",\"text\":\"{long}\"}}]}}}}\n"
            )
        };
        let path = temp_log("reply", &(0..120).map(step).collect::<String>());
        let mut follow = Follow::at_path(source(), path.clone());
        let (_, body) = follow.answer(&source(), &state(None), HOME);
        let value: Value = serde_json::from_slice(&body).unwrap();
        // 150 blocks of about 2 KB fit whole.
        assert_eq!(value["entries"][0]["blocks"].as_array().unwrap().len(), MAX_BLOCKS);
        assert_eq!(value["truncated"], true);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_home_folder_is_compared_without_its_last_slash() {
        assert_eq!(home_of(Path::new("/Users/me/")), HOME);
    }
}
