//! Token usage extracted from local transcripts, and aggregation into dashboard reports.
//!
//! Claude Code writes one JSONL line per content block, so an API response appears several times
//! with the same `message.id`; records are de-duplicated by that id. Codex writes one
//! `token_usage_record` per response (`response_id`).

use crate::fsutil;
use crate::model::Agent;
use crate::pricing::PriceTable;
use chrono::{DateTime, Duration, Local, NaiveDate};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecord {
    pub key: String,
    pub timestamp_ms: i64,
    pub model: String,
    pub project: String,
    pub session_id: String,
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    /// `None` when no price is known for the model.
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRecord {
    pub key: String,
    pub timestamp_ms: i64,
    pub name: String,
}

#[derive(Debug, Default, Clone)]
pub struct FileUsage {
    pub requests: Vec<RequestRecord>,
    pub tools: Vec<ToolRecord>,
}

/// Incremental scanner: files whose size and mtime are unchanged are not parsed again.
#[derive(Default)]
pub struct UsageScanner {
    cache: HashMap<PathBuf, (u64, u64, FileUsage)>,
}

pub fn root_dir(agent: Agent) -> PathBuf {
    match agent {
        Agent::Claude => fsutil::home().join(".claude").join("projects"),
        Agent::Codex => fsutil::home().join(".codex").join("sessions"),
        // No local token accounting for the other CLIs.
        _ => PathBuf::new(),
    }
}

impl UsageScanner {
    pub fn scan(&mut self, agent: Agent) -> FileUsage {
        let prices = PriceTable::load();
        let mut files = Vec::new();
        fsutil::jsonl_files(&root_dir(agent), 4, &mut files);

        let mut merged = FileUsage::default();
        let mut seen_requests = HashSet::new();
        let mut seen_tools = HashSet::new();
        for path in files {
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let stamp = (meta.len(), fsutil::mtime_ms(&path));
            let fresh = !matches!(self.cache.get(&path), Some((len, mtime, _)) if (*len, *mtime) == stamp);
            if fresh {
                let usage = match agent {
                    Agent::Claude => parse_claude(&path, &prices),
                    Agent::Codex => parse_codex(&path, &prices),
                    _ => FileUsage::default(),
                };
                self.cache.insert(path.clone(), (stamp.0, stamp.1, usage));
            }
            let (_, _, usage) = &self.cache[&path];
            // Resumed/forked sessions copy earlier lines into new files; count each response once.
            for r in &usage.requests {
                if seen_requests.insert(r.key.clone()) {
                    merged.requests.push(r.clone());
                }
            }
            for t in &usage.tools {
                if seen_tools.insert(t.key.clone()) {
                    merged.tools.push(t.clone());
                }
            }
        }
        merged
    }
}

fn parse_timestamp(value: &Value) -> Option<i64> {
    DateTime::parse_from_rfc3339(value.as_str()?).ok().map(|t| t.timestamp_millis())
}

fn parse_claude(path: &Path, prices: &PriceTable) -> FileUsage {
    let mut usage = FileUsage::default();
    let Ok(file) = File::open(path) else { return usage };
    let mut seen = HashSet::new();
    // Group by the directory the session started in; later lines follow the agent's `cd`s.
    let mut project: Option<String> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if project.is_none() && line.contains("\"cwd\":") {
            project = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["cwd"].as_str().map(str::to_string));
        }
        if !line.contains("\"type\":\"assistant\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let message = &v["message"];
        let Some(timestamp_ms) = parse_timestamp(&v["timestamp"]) else { continue };

        if let Some(blocks) = message["content"].as_array() {
            for block in blocks.iter().filter(|b| b["type"] == "tool_use") {
                if let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) {
                    usage.tools.push(ToolRecord { key: id.to_string(), timestamp_ms, name: name.to_string() });
                }
            }
        }

        let u = &message["usage"];
        let Some(message_id) = message["id"].as_str() else { continue };
        if u.is_null() || !seen.insert(message_id.to_string()) {
            continue;
        }
        let model = message["model"].as_str().unwrap_or("unknown").to_string();
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let (input, output, cache_read) = (n("input_tokens"), n("output_tokens"), n("cache_read_input_tokens"));
        let cache_write = n("cache_creation_input_tokens");
        let write_1h = u["cache_creation"]["ephemeral_1h_input_tokens"].as_u64().unwrap_or(0).min(cache_write);
        let write_5m = cache_write - write_1h;
        let fast = u["speed"].as_str() == Some("fast");
        let cost = prices.lookup(&model, fast).map(|p| {
            (input as f64 * p.input
                + output as f64 * p.output
                + cache_read as f64 * p.cache_read()
                + write_5m as f64 * p.cache_write_5m()
                + write_1h as f64 * p.cache_write_1h())
                / 1_000_000.0
        });
        usage.requests.push(RequestRecord {
            key: format!("claude:{message_id}"),
            timestamp_ms,
            model,
            project: project.clone().or_else(|| v["cwd"].as_str().map(str::to_string)).unwrap_or_default(),
            session_id: v["sessionId"].as_str().unwrap_or("").to_string(),
            input,
            output,
            cache_write,
            cache_read,
            cost,
        });
    }
    usage
}

fn parse_codex(path: &Path, prices: &PriceTable) -> FileUsage {
    let mut usage = FileUsage::default();
    let Ok(file) = File::open(path) else { return usage };
    let mut session_id = String::new();
    let mut project = String::new();
    let mut turn_models: HashMap<String, String> = HashMap::new();
    let mut last_model = String::from("unknown");
    // Older Codex builds only emit cumulative `token_count` events.
    let mut fallback: Vec<(i64, Value)> = Vec::new();
    let mut has_records = false;

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let interesting = ["\"session_meta\"", "\"turn_context\"", "\"token_usage_record\"", "\"token_count\"", "_call\""]
            .iter()
            .any(|needle| line.contains(needle));
        if !interesting {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let payload = &v["payload"];
        let timestamp_ms = parse_timestamp(&v["timestamp"]).unwrap_or(0);
        match v["type"].as_str() {
            Some("session_meta") => {
                session_id = payload["id"].as_str().unwrap_or("").to_string();
                project = payload["cwd"].as_str().unwrap_or("").to_string();
            }
            Some("turn_context") => {
                if let Some(model) = payload["model"].as_str() {
                    last_model = model.to_string();
                    if let Some(turn) = payload["turn_id"].as_str() {
                        turn_models.insert(turn.to_string(), model.to_string());
                    }
                }
            }
            Some("token_usage_record") => {
                has_records = true;
                let model = payload["turn_id"].as_str().and_then(|t| turn_models.get(t)).cloned().unwrap_or_else(|| last_model.clone());
                let key = payload["response_id"].as_str().map(str::to_string).unwrap_or_else(|| format!("{session_id}:{timestamp_ms}"));
                usage.requests.push(codex_record(
                    format!("codex:{key}"),
                    timestamp_ms,
                    model,
                    &project,
                    &session_id,
                    &payload["usage"],
                    prices,
                ));
            }
            Some("event_msg") if payload["type"] == "token_count" => {
                fallback.push((timestamp_ms, payload["info"]["last_token_usage"].clone()));
            }
            Some("response_item") if matches!(payload["type"].as_str(), Some("function_call" | "custom_tool_call")) => {
                if let Some(name) = payload["name"].as_str() {
                    let call = payload["call_id"].as_str().unwrap_or("");
                    usage.tools.push(ToolRecord {
                        key: format!("codex:{session_id}:{call}:{timestamp_ms}"),
                        timestamp_ms,
                        name: name.to_string(),
                    });
                }
            }
            _ => {}
        }
    }

    if !has_records {
        let mut previous: Option<&Value> = None;
        for (i, (timestamp_ms, last)) in fallback.iter().enumerate() {
            // Codex repeats the same token_count when nothing new was billed.
            if last.is_null() || previous == Some(last) {
                continue;
            }
            previous = Some(last);
            let key = format!("codex:{session_id}:count:{i}");
            usage.requests.push(codex_record(key, *timestamp_ms, last_model.clone(), &project, &session_id, last, prices));
        }
    }
    usage
}

fn codex_record(
    key: String,
    timestamp_ms: i64,
    model: String,
    project: &str,
    session_id: &str,
    u: &Value,
    prices: &PriceTable,
) -> RequestRecord {
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    // OpenAI reports cached tokens as a subset of input tokens.
    let cache_read = n("cached_input_tokens");
    let input = n("input_tokens").saturating_sub(cache_read);
    let (output, cache_write) = (n("output_tokens"), n("cache_write_input_tokens"));
    let cost = prices.lookup(&model, false).map(|p| {
        (input as f64 * p.input + output as f64 * p.output + cache_read as f64 * p.cache_read() + cache_write as f64 * p.cache_write_5m())
            / 1_000_000.0
    });
    RequestRecord {
        key,
        timestamp_ms,
        model,
        project: project.to_string(),
        session_id: session_id.to_string(),
        input,
        output,
        cache_write,
        cache_read,
        cost,
    }
}

// ---------------------------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub cost: f64,
    pub calls: u64,
    pub unpriced_calls: u64,
    pub sessions: usize,
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl Totals {
    fn add(&mut self, r: &RequestRecord) {
        self.calls += 1;
        match r.cost {
            Some(cost) => self.cost += cost,
            None => self.unpriced_calls += 1,
        }
        self.input += r.input;
        self.output += r.output;
        self.cache_write += r.cache_write;
        self.cache_read += r.cache_read;
    }

    pub fn total_tokens(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }

    /// Share of prompt tokens served from cache.
    pub fn cache_hit_rate(&self) -> f64 {
        let prompt = self.input + self.cache_write + self.cache_read;
        if prompt == 0 {
            0.0
        } else {
            self.cache_read as f64 / prompt as f64
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayUsage {
    pub date: NaiveDate,
    pub totals: Totals,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedUsage {
    pub name: String,
    pub cost: f64,
    pub calls: u64,
    pub sessions: usize,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub days: Vec<DayUsage>,
    pub totals: Totals,
    pub models: Vec<NamedUsage>,
    pub projects: Vec<NamedUsage>,
    pub tools: Vec<(String, u64)>,
}

impl UsageReport {
    /// Report for the `days` local calendar days ending with `today`.
    pub fn build(usage: &FileUsage, today: NaiveDate, days: u32) -> Self {
        let start = today - Duration::days(days.saturating_sub(1) as i64);
        let local_date = |ms: i64| DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&Local).date_naive());
        let in_range = |ms: i64| local_date(ms).filter(|d| *d >= start && *d <= today);

        let mut by_day: BTreeMap<NaiveDate, Totals> = (0..days).map(|i| (start + Duration::days(i as i64), Totals::default())).collect();
        let mut totals = Totals::default();
        let mut sessions = HashSet::new();
        let mut day_sessions: HashMap<NaiveDate, HashSet<&str>> = HashMap::new();
        let mut models: HashMap<&str, (Totals, HashSet<&str>)> = HashMap::new();
        let mut projects: HashMap<&str, (Totals, HashSet<&str>)> = HashMap::new();

        for r in &usage.requests {
            let Some(day) = in_range(r.timestamp_ms) else { continue };
            totals.add(r);
            sessions.insert(r.session_id.as_str());
            by_day.entry(day).or_default().add(r);
            day_sessions.entry(day).or_default().insert(r.session_id.as_str());
            let model = models.entry(r.model.as_str()).or_default();
            model.0.add(r);
            model.1.insert(r.session_id.as_str());
            let project = projects.entry(r.project.as_str()).or_default();
            project.0.add(r);
            project.1.insert(r.session_id.as_str());
        }
        totals.sessions = sessions.len();

        let named = |map: HashMap<&str, (Totals, HashSet<&str>)>| {
            let mut list: Vec<NamedUsage> = map
                .into_iter()
                .map(|(name, (t, s))| NamedUsage {
                    name: name.to_string(),
                    cost: t.cost,
                    calls: t.calls,
                    sessions: s.len(),
                    tokens: t.total_tokens(),
                })
                .collect();
            list.sort_by(|a, b| b.cost.total_cmp(&a.cost).then(b.tokens.cmp(&a.tokens)));
            list
        };

        let mut tool_counts: HashMap<&str, u64> = HashMap::new();
        for t in usage.tools.iter().filter(|t| in_range(t.timestamp_ms).is_some()) {
            *tool_counts.entry(t.name.as_str()).or_default() += 1;
        }
        let mut tools: Vec<(String, u64)> = tool_counts.into_iter().map(|(n, c)| (n.to_string(), c)).collect();
        tools.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        let days = by_day
            .into_iter()
            .map(|(date, mut t)| {
                t.sessions = day_sessions.get(&date).map_or(0, HashSet::len);
                DayUsage { date, totals: t }
            })
            .collect();

        Self { days, totals, models: named(models), projects: named(projects), tools }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(key: &str, day: NaiveDate, cost: Option<f64>, model: &str) -> RequestRecord {
        let ts = day.and_hms_opt(12, 0, 0).unwrap().and_local_timezone(Local).unwrap().timestamp_millis();
        RequestRecord {
            key: key.into(),
            timestamp_ms: ts,
            model: model.into(),
            project: "/p".into(),
            session_id: "s1".into(),
            input: 10,
            output: 5,
            cache_write: 20,
            cache_read: 70,
            cost,
        }
    }

    #[test]
    fn aggregates_within_range() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
        let usage = FileUsage {
            requests: vec![
                record("a", today, Some(1.0), "claude-opus-5"),
                record("b", today - Duration::days(1), Some(2.0), "claude-sonnet-5"),
                record("c", today - Duration::days(10), Some(4.0), "claude-opus-5"),
                record("d", today, None, "gpt-x"),
            ],
            tools: vec![ToolRecord { key: "t".into(), timestamp_ms: record("x", today, None, "").timestamp_ms, name: "Bash".into() }],
        };
        let report = UsageReport::build(&usage, today, 7);
        assert_eq!(report.days.len(), 7);
        assert_eq!(report.totals.calls, 3);
        assert_eq!(report.totals.unpriced_calls, 1);
        assert!((report.totals.cost - 3.0).abs() < 1e-9);
        assert_eq!(report.models[0].name, "claude-sonnet-5");
        assert_eq!(report.tools, vec![("Bash".to_string(), 1)]);
        assert!((report.totals.cache_hit_rate() - 210.0 / 300.0).abs() < 1e-9);
    }

    #[test]
    fn claude_duplicates_are_counted_once() {
        let dir = std::env::temp_dir().join(format!("agentty-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        let line = r#"{"type":"assistant","timestamp":"2026-09-16T01:00:00Z","cwd":"/p","sessionId":"s","message":{"id":"m1","model":"claude-opus-5","usage":{"input_tokens":1000000,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{}}]}}"#;
        std::fs::write(&path, format!("{line}\n{line}\n")).unwrap();
        let usage = parse_claude(&path, &PriceTable::empty());
        assert_eq!(usage.requests.len(), 1);
        assert_eq!(usage.requests[0].cost, Some(5.0));
        std::fs::remove_dir_all(dir).ok();
    }
}
