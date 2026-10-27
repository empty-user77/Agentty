//! What a Claude Code session has scheduled for itself: cron jobs (`CronCreate`, also what `/loop 5m`
//! makes) and the next wake-up of a self-paced `/loop` (`ScheduleWakeup`). They live only inside
//! the running `claude` process and are written nowhere, so they are followed from the hooks the
//! pane already receives (`PostToolUse` carries each call's input and result).
//!
//! Codex has no counterpart in its CLI: its scheduled tasks belong to the Codex desktop app.

use chrono::{Datelike, Local, TimeZone, Timelike};

/// A recurring job is deleted by Claude Code a week after it was made.
const RECURRING_LIFETIME_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Jobs only fire while the session is idle, so one may run a little after its time. It stays
/// listed (as due) for this long before it counts as gone.
const LATE_GRACE_MS: i64 = 10 * 60 * 1000;
/// `ScheduleWakeup` delays are clamped to this range by Claude Code.
const WAKEUP_RANGE_S: (i64, i64) = (60, 3600);

/// A schedule call seen in a `PostToolUse` hook.
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleEvent {
    Created { id: String, cron: String, prompt: String, recurring: bool, next_ms: Option<i64> },
    Deleted { id: String },
    Wakeup { delay_s: i64, reason: Option<String>, prompt: Option<String> },
    WakeupStopped,
}

impl ScheduleEvent {
    /// The schedule call in a hook payload, if it is one. Only `PostToolUse`: the job exists (and
    /// has an id) once the tool has run.
    pub fn from_hook(event: Option<&str>, tool: Option<&str>, payload: &serde_json::Value) -> Option<Self> {
        if event != Some("PostToolUse") {
            return None;
        }
        let input = &payload["tool_input"];
        let response = &payload["tool_response"];
        let text = |v: &serde_json::Value, key: &str| v[key].as_str().map(str::to_string).filter(|s| !s.is_empty());
        // A job id goes into a message typed to the session when the user cancels the job.
        let job_id = |id: String| (id.len() <= 32 && id.chars().all(|c| c.is_ascii_alphanumeric())).then_some(id);
        match tool? {
            "CronCreate" => {
                // The result is `{id, nextFireAt, …}`; older versions only say "… task 690f5e4b (…)".
                let id = text(response, "id").or_else(|| response.as_str().and_then(job_id_in)).and_then(job_id)?;
                Some(Self::Created {
                    id,
                    cron: text(input, "cron")?,
                    prompt: text(input, "prompt").unwrap_or_default(),
                    recurring: response["recurring"].as_bool().or(input["recurring"].as_bool()).unwrap_or(true),
                    // Milliseconds, with a fraction.
                    next_ms: response["nextFireAt"].as_f64().map(|ms| ms as i64),
                })
            }
            "CronDelete" => Some(Self::Deleted { id: text(input, "id").and_then(job_id)? }),
            "ScheduleWakeup" if input["stop"].as_bool() == Some(true) => Some(Self::WakeupStopped),
            "ScheduleWakeup" => Some(Self::Wakeup {
                delay_s: input["delaySeconds"].as_f64()? as i64,
                reason: text(input, "reason"),
                prompt: text(input, "prompt").filter(|p| !p.starts_with("<<")),
            }),
            _ => None,
        }
    }
}

/// "Scheduled recurring job 690f5e4b (*/5 * * * *) …" → `690f5e4b`.
fn job_id_in(text: &str) -> Option<String> {
    let before = text.split('(').next()?;
    let id = before.split_whitespace().last()?;
    (id.len() >= 4 && id.chars().all(|c| c.is_ascii_hexdigit())).then(|| id.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleKind {
    /// A cron job: runs `prompt` at each match of `cron` (local time), once unless `recurring`.
    Cron { cron: String, recurring: bool },
    /// The next turn of a self-paced `/loop`.
    Wakeup,
}

/// One schedule of a session, as shown in its pane's header.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSchedule {
    /// The job id (`CronCreate`'s), or empty for the wake-up.
    pub id: String,
    pub kind: ScheduleKind,
    /// What runs: the job's prompt, or why the loop waits.
    pub prompt: String,
    /// When it runs first (epoch ms).
    pub next_ms: i64,
    /// Claude Code runs each job a fixed time after the cron match (spreading load); the
    /// difference seen on creation, kept for the runs after it.
    offset_ms: i64,
    pub created_ms: i64,
    /// The session was asked to cancel it, and has not yet.
    pub cancelling: bool,
}

impl AgentSchedule {
    /// When a recurring job is deleted.
    pub fn expires_ms(&self) -> Option<i64> {
        matches!(self.kind, ScheduleKind::Cron { recurring: true, .. }).then_some(self.created_ms + RECURRING_LIFETIME_MS)
    }

    /// Its next run at `now`, or `None` once it is over (ran, or expired).
    pub fn next_run(&self, now_ms: i64) -> Option<i64> {
        let ScheduleKind::Cron { cron, recurring: true } = &self.kind else {
            return (self.next_ms + LATE_GRACE_MS > now_ms).then_some(self.next_ms);
        };
        if self.next_ms > now_ms {
            return Some(self.next_ms);
        }
        let expires = self.expires_ms()?;
        let next = next_match(cron, now_ms - self.offset_ms)? + self.offset_ms;
        (next <= expires).then_some(next)
    }
}

/// What to tell the session to cancel `schedule`: only it can, the job lives in its process.
pub fn cancel_prompt(schedule: &AgentSchedule) -> String {
    match schedule.kind {
        ScheduleKind::Wakeup => "Stop the /loop now: call ScheduleWakeup with stop: true. The user asked for this from \
                                 Agentty; do only that and confirm in one short line."
            .to_string(),
        ScheduleKind::Cron { .. } => format!(
            "Cancel the scheduled job {} now with CronDelete. The user asked for this from Agentty; do only that and \
             confirm in one short line.",
            schedule.id
        ),
    }
}

/// Applies `event` (seen at `now_ms`) to a session's schedules.
pub fn apply(schedules: &mut Vec<AgentSchedule>, event: ScheduleEvent, now_ms: i64) {
    match event {
        ScheduleEvent::Created { id, cron, prompt, recurring, next_ms } => {
            let matched = next_match(&cron, now_ms);
            let Some(next_ms) = next_ms.or(matched) else { return };
            let offset_ms = matched.map_or(0, |m| next_ms - m);
            schedules.retain(|s| s.id != id);
            schedules.push(AgentSchedule {
                id,
                kind: ScheduleKind::Cron { cron, recurring },
                prompt,
                next_ms,
                offset_ms,
                created_ms: now_ms,
                cancelling: false,
            });
        }
        ScheduleEvent::Deleted { id } => schedules.retain(|s| s.id != id),
        ScheduleEvent::Wakeup { delay_s, reason, prompt } => {
            schedules.retain(|s| s.kind != ScheduleKind::Wakeup);
            let delay = delay_s.clamp(WAKEUP_RANGE_S.0, WAKEUP_RANGE_S.1) * 1000;
            schedules.push(AgentSchedule {
                id: String::new(),
                kind: ScheduleKind::Wakeup,
                prompt: reason.or(prompt).unwrap_or_default(),
                next_ms: now_ms + delay,
                offset_ms: 0,
                created_ms: now_ms,
                cancelling: false,
            });
        }
        ScheduleEvent::WakeupStopped => schedules.retain(|s| s.kind != ScheduleKind::Wakeup),
    }
    // Over ones go when the list changes; until then `next_run` hides them.
    schedules.retain(|s| s.next_run(now_ms).is_some());
    if schedules.len() > 50 {
        schedules.remove(0);
    }
}

/// The first minute after `after_ms` that `cron` (5 fields, local time) matches, within a year.
pub fn next_match(cron: &str, after_ms: i64) -> Option<i64> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = fields.as_slice() else { return None };
    let minutes = parse_field(minute, 0, 59)?;
    let hours = parse_field(hour, 0, 23)?;
    let doms = parse_field(dom, 1, 31)?;
    let months = parse_field(month, 1, 12)?;
    // 0 and 7 are both Sunday.
    let mut dows = parse_field(dow, 0, 7)?;
    if dows[7] {
        dows[0] = true;
    }
    // Standard cron: with both day fields restricted, either one matching is enough.
    let (dom_any, dow_any) = (dom.starts_with('*'), dow.starts_with('*'));
    let start = Local.timestamp_millis_opt(after_ms).single()?;
    // The next whole minute.
    let mut at = start.with_second(0)?.with_nanosecond(0)? + chrono::Duration::minutes(1);
    let limit = start + chrono::Duration::days(366);
    while at <= limit {
        let day_ok = match (dom_any, dow_any) {
            (true, true) => true,
            (false, true) => doms[at.day() as usize],
            (true, false) => dows[at.weekday().num_days_from_sunday() as usize],
            (false, false) => doms[at.day() as usize] || dows[at.weekday().num_days_from_sunday() as usize],
        };
        if !months[at.month() as usize] || !day_ok {
            // Skip to the next day.
            at = (at + chrono::Duration::days(1)).with_hour(0)?.with_minute(0)?;
            continue;
        }
        if !hours[at.hour() as usize] {
            at = (at + chrono::Duration::hours(1)).with_minute(0)?;
            continue;
        }
        if minutes[at.minute() as usize] {
            return Some(at.timestamp_millis());
        }
        at += chrono::Duration::minutes(1);
    }
    None
}

/// One cron field (`*`, `*/5`, `1-5`, `1,15`, `0-30/10`) as a table of the values it allows.
fn parse_field(field: &str, low: u32, high: u32) -> Option<Vec<bool>> {
    let mut allowed = vec![false; high as usize + 1];
    for part in field.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().ok().filter(|s| *s > 0)?),
            None => (part, 1),
        };
        let (from, to) = match range {
            "*" => (low, high),
            _ => match range.split_once('-') {
                Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
                // `5/15`: from 5 to the end.
                None if step > 1 => (range.parse().ok()?, high),
                None => {
                    let value = range.parse().ok()?;
                    (value, value)
                }
            },
        };
        if from < low || to > high || from > to {
            return None;
        }
        for value in (from..=to).step_by(step as usize) {
            allowed[value as usize] = true;
        }
    }
    Some(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().timestamp_millis()
    }

    #[test]
    fn cron_matches() {
        let now = local(2026, 10, 27, 17, 22);
        assert_eq!(next_match("*/5 * * * *", now), Some(local(2026, 10, 27, 17, 25)));
        assert_eq!(next_match("7 * * * *", now), Some(local(2026, 10, 27, 18, 7)));
        assert_eq!(next_match("57 8 * * *", now), Some(local(2026, 10, 28, 8, 57)));
        // 2026-10-31 is a Saturday: weekdays at 9 skip to Monday.
        assert_eq!(next_match("0 9 * * 1-5", local(2026, 10, 30, 10, 0)), Some(local(2026, 11, 2, 9, 0)));
        assert_eq!(next_match("17 4 1 1 *", now), Some(local(2027, 1, 1, 4, 17)));
        assert_eq!(next_match("0 0 * * 7", now), Some(local(2026, 11, 1, 0, 0)));
        assert_eq!(next_match("not a cron", now), None);
        assert_eq!(next_match("61 * * * *", now), None);
    }

    #[test]
    fn hook_payloads() {
        let post = |tool: &str, json: &str| ScheduleEvent::from_hook(Some("PostToolUse"), Some(tool), &serde_json::from_str(json).unwrap());
        assert_eq!(
            post(
                "CronCreate",
                r#"{"tool_input":{"cron":"*/5 * * * *","prompt":"check the deploy"},"tool_response":{"id":"690f5e4b","recurring":true,"nextFireAt":1798744620000.812}}"#
            ),
            Some(ScheduleEvent::Created {
                id: "690f5e4b".into(),
                cron: "*/5 * * * *".into(),
                prompt: "check the deploy".into(),
                recurring: true,
                next_ms: Some(1798744620000),
            })
        );
        let text = post(
            "CronCreate",
            r#"{"tool_input":{"cron":"17 4 1 1 *","prompt":"x","recurring":false},"tool_response":"Scheduled one-shot task 690f5e4b (17 4 1 1 *). Next run: Jan 1 4:17am."}"#,
        );
        assert!(matches!(text, Some(ScheduleEvent::Created { ref id, recurring: false, next_ms: None, .. }) if id == "690f5e4b"));
        assert_eq!(post("CronDelete", r#"{"tool_input":{"id":"690f5e4b"}}"#), Some(ScheduleEvent::Deleted { id: "690f5e4b".into() }));
        assert_eq!(post("ScheduleWakeup", r#"{"tool_input":{"stop":true}}"#), Some(ScheduleEvent::WakeupStopped));
        assert_eq!(
            post(
                "ScheduleWakeup",
                r#"{"tool_input":{"delaySeconds":1200,"reason":"watching CI run","prompt":"<<autonomous-loop-dynamic>>"}}"#
            ),
            Some(ScheduleEvent::Wakeup { delay_s: 1200, reason: Some("watching CI run".into()), prompt: None })
        );
        // Before the tool ran there is no job yet; other tools are not schedules.
        let pre = serde_json::json!({"tool_input": {"cron": "* * * * *"}});
        assert_eq!(ScheduleEvent::from_hook(Some("PreToolUse"), Some("CronCreate"), &pre), None);
        assert_eq!(post("Bash", r#"{"tool_input":{"command":"ls"}}"#), None);
    }

    #[test]
    fn schedules_come_and_go() {
        let now = local(2026, 10, 27, 17, 22);
        let mut list = Vec::new();
        let created = |id: &str, cron: &str, recurring: bool, next_ms: Option<i64>| ScheduleEvent::Created {
            id: id.into(),
            cron: cron.into(),
            prompt: "p".into(),
            recurring,
            next_ms,
        };
        // Claude Code runs this job 40 s after each match.
        apply(&mut list, created("a", "*/5 * * * *", true, Some(local(2026, 10, 27, 17, 25) + 40_000)), now);
        apply(&mut list, created("b", "30 17 * * *", false, None), now);
        apply(&mut list, ScheduleEvent::Wakeup { delay_s: 5, reason: Some("r".into()), prompt: None }, now);
        assert_eq!(list.len(), 3);
        // A wake-up shorter than a minute waits a minute.
        assert_eq!(list[2].next_ms, now + 60_000);
        // Later runs of the recurring job keep its offset; the one-shot job is over.
        let later = local(2026, 10, 27, 18, 1);
        assert_eq!(list[0].next_run(later), Some(local(2026, 10, 27, 18, 5) + 40_000));
        assert_eq!(list[1].next_run(later), None);
        assert_eq!(list[2].next_run(later), None);
        // A week on, the recurring job is gone too.
        assert_eq!(list[0].next_run(now + RECURRING_LIFETIME_MS + 60_000), None);
        apply(&mut list, ScheduleEvent::Deleted { id: "a".into() }, now);
        apply(&mut list, ScheduleEvent::WakeupStopped, now);
        assert_eq!(list.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["b"]);
    }
}
