//! The orchestrator page's threads for the remote apps (`GET /api/board`): the workbench copies them
//! out while someone asks, and the server answers from that copy.

use super::chat::clean_value;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A whole answer at most; the oldest orchestrations go first.
pub const MAX_BODY: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    /// The orchestration setting.
    pub enabled: bool,
    /// The agents here that can lead one.
    pub leads: Vec<&'static str>,
    /// The profile a new one starts with.
    pub profile: &'static str,
    pub threads: Vec<Thread>,
}

impl Board {
    /// `other`, from another window, added to this one. An orchestration both windows know (each
    /// keeps the list) goes by the window that holds its terminal.
    pub fn merge(&mut self, other: Board) {
        self.enabled |= other.enabled;
        for lead in other.leads {
            if !self.leads.contains(&lead) {
                self.leads.push(lead);
            }
        }
        if self.profile.is_empty() {
            self.profile = other.profile;
        }
        for thread in other.threads {
            match self.threads.iter_mut().find(|known| known.id == thread.id) {
                Some(known) if known.lead.pane.is_none() => *known = thread,
                Some(_) => {}
                None => self.threads.push(thread),
            }
        }
        self.threads.sort_by_key(|thread| (thread.kind != "orchestration", std::cmp::Reverse(thread.started_ms)));
    }

    /// `(rev, body)` of the board as the page gets it. `rev` changes with everything but how long
    /// things have run, which an app counts up itself.
    pub fn answer(&self, folders: bool, home: &str) -> (String, Vec<u8>) {
        let mut threads: Vec<&Thread> = self.threads.iter().collect();
        loop {
            let mut value = json!({
                "enabled": self.enabled,
                "folders": folders,
                "leads": self.leads,
                "profile": self.profile,
                "threads": threads,
            });
            clean_value(&mut value, home);
            let mut timeless = value.clone();
            without_elapsed(&mut timeless);
            let mut hasher = DefaultHasher::new();
            timeless.to_string().hash(&mut hasher);
            let rev = format!("{:016x}", hasher.finish());
            value["rev"] = json!(rev);
            let body = value.to_string().into_bytes();
            if body.len() <= MAX_BODY || threads.is_empty() {
                return (rev, body);
            }
            // The list goes newest first, sessions last: the last orchestration is the oldest.
            let oldest = threads.iter().rposition(|thread| thread.kind == "orchestration").unwrap_or(threads.len() - 1);
            threads.remove(oldest);
        }
    }

    pub fn thread(&self, id: &str) -> Option<&Thread> {
        self.threads.iter().find(|thread| thread.id == id)
    }
}

fn without_elapsed(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(without_elapsed),
        Value::Object(fields) => {
            fields.remove("elapsed");
            fields.values_mut().for_each(without_elapsed);
        }
        _ => {}
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// `o<record id>` for an orchestration, `s<pane>` for a session.
    pub id: String,
    /// `orchestration` or `session`.
    pub kind: &'static str,
    pub title: String,
    pub goal: String,
    /// Home shortened to `~`.
    pub folder: String,
    /// `None` for a session.
    pub stage: Option<&'static str>,
    pub stage_text: String,
    pub lead: Lead,
    pub plan_awaiting: bool,
    pub plan: Vec<PlanItem>,
    /// What `POST /api/orchestration` takes for it now.
    pub actions: Vec<&'static str>,
    pub done: usize,
    pub total: usize,
    pub waves: Vec<Wave>,
    pub feed: Vec<FeedItem>,
    /// For the order of the list.
    #[serde(skip)]
    pub started_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lead {
    /// `None` while its terminal is closed or asleep.
    pub pane: Option<u64>,
    pub workspace_id: Option<u64>,
    pub state: &'static str,
    pub elapsed: u64,
    pub tool: String,
    pub model: Option<String>,
    pub last: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanItem {
    pub name: String,
    pub wave: u32,
    pub role: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Wave {
    pub wave: usize,
    pub done: usize,
    pub total: usize,
    pub agents: Vec<AgentNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentNode {
    pub name: String,
    /// `waiting`, `running`, `done` or `cut`.
    pub state: &'static str,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub role: Option<String>,
    pub elapsed: u64,
    pub last: Option<String>,
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FeedItem {
    pub at: u64,
    pub agent: String,
    pub model: Option<String>,
    pub text: String,
    pub state: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, kind: &'static str, started_ms: u64) -> Thread {
        Thread { id: id.into(), kind, started_ms, ..Default::default() }
    }

    #[test]
    fn rev_ignores_only_the_elapsed_time() {
        let mut board = Board { enabled: true, threads: vec![thread("o1", "orchestration", 1)], ..Default::default() };
        let (rev, body) = board.answer(true, "/Users/me");
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["rev"], rev);
        assert_eq!(value["folders"], true);
        assert_eq!(value["threads"][0]["id"], "o1");
        assert!(value["threads"][0].get("startedMs").is_none());
        board.threads[0].lead.elapsed = 99;
        assert_eq!(board.answer(true, "/Users/me").0, rev);
        board.threads[0].stage_text = "Running".into();
        assert_ne!(board.answer(true, "/Users/me").0, rev);
    }

    #[test]
    fn its_text_leaves_like_a_terminal_page_would_not() {
        let mut one = thread("o1", "orchestration", 1);
        one.folder = "/Users/me/x".into();
        one.goal = format!("use ghp_{}", "example_not_a_real_token_".repeat(2));
        one.feed = vec![FeedItem { at: 1, agent: "a".into(), model: None, text: "ls /Users/me\u{1b}[0m".into(), state: "running" }];
        let board = Board { threads: vec![one], ..Default::default() };
        let body = String::from_utf8(board.answer(true, "/Users/me").1).unwrap();
        assert!(!body.contains("/Users/me") && !body.contains("example_not_a_real") && !body.contains("[0m"), "{body}");
    }

    #[test]
    fn a_big_board_lets_its_oldest_orchestrations_go() {
        let mut threads: Vec<Thread> =
            (0..60u64).map(|n| Thread { goal: "g".repeat(50_000), ..thread(&format!("o{n}"), "orchestration", 1000 - n) }).collect();
        threads.push(thread("s1", "session", 0));
        let board = Board { threads, ..Default::default() };
        let (_, body) = board.answer(true, "/Users/me");
        assert!(body.len() <= MAX_BODY, "{}", body.len());
        let value: Value = serde_json::from_slice(&body).unwrap();
        let shown = value["threads"].as_array().unwrap();
        assert_eq!(shown[0]["id"], "o0");
        assert_eq!(shown.last().unwrap()["id"], "s1", "the session stays");
    }

    #[test]
    fn windows_merge_with_the_one_holding_the_terminal_winning() {
        let mut first =
            Board { enabled: true, leads: vec!["claude"], profile: "balanced", threads: vec![thread("o1", "orchestration", 5)] };
        let mut held = thread("o1", "orchestration", 5);
        held.lead.pane = Some(9);
        let second = Board {
            enabled: false,
            leads: vec!["claude", "codex"],
            profile: "economy",
            threads: vec![held, thread("o2", "orchestration", 7), thread("s3", "session", 8)],
        };
        first.merge(second);
        assert_eq!(first.leads, ["claude", "codex"]);
        assert_eq!(first.profile, "balanced");
        let ids: Vec<&str> = first.threads.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["o2", "o1", "s3"]);
        assert_eq!(first.threads[1].lead.pane, Some(9));
    }
}
