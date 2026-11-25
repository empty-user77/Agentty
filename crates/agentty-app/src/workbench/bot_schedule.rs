//! When bots start work by themselves. One timer sleeps until the next schedule of any bot is
//! due; with no schedule there is no timer at all. A run puts the schedule's prompt in the bot's
//! mailbox, like any other message.
//!
//! A run missed while Agentty was closed (or the computer asleep) is made up once, when it is less
//! than a day old. Each schedule's last run is kept in the bot's folder, so a restart never runs
//! one twice.

use super::bots::Origin;
use super::Workbench;
use crate::agent_schedule::next_match;
use agentty_bridge::bots::Bot;
use gpui::{AppContext as _, Context};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Schedules run no closer together than this.
pub const MIN_GAP_MS: i64 = 15 * 60 * 1000;
/// A missed run older than this is left out.
pub const CATCH_UP_MS: i64 = 24 * 60 * 60 * 1000;
/// The timer never sleeps longer than this: the clock it sleeps on stops while the computer does,
/// so after a long sleep the next look at the wall clock is at most this far away. A look that
/// finds nothing due reads the clock and goes back to sleep, without drawing anything.
const NAP_MAX: Duration = Duration::from_secs(10 * 60);
const RUNS_FILE: &str = "schedule.json";

/// `Ok`, or the key of what to tell the user.
pub fn check(cron: &str) -> Result<(), &'static str> {
    check_at(cron, chrono::Local::now().timestamp_millis())
}

/// A five-field expression that matches some minute, never two less than [`MIN_GAP_MS`] apart.
pub fn check_at(cron: &str, from_ms: i64) -> Result<(), &'static str> {
    let mut at = next_match(cron, from_ms).ok_or("bots.error_schedule")?;
    // A week of runs shows the closest pair of any expression that can be written in five fields.
    let until = at + 7 * 24 * 60 * 60 * 1000;
    while let Some(next) = next_match(cron, at) {
        if next - at < MIN_GAP_MS {
            return Err("bots.error_schedule_gap");
        }
        if next > until {
            break;
        }
        at = next;
    }
    Ok(())
}

/// What is due at `now_ms`, and when the timer has to look again. `last` holds each schedule's
/// last run by (handle, id) and is brought up to date: a schedule seen for the first time counts
/// from now, a due one from now once it ran (several missed runs are one), and a run more than a
/// day late is passed over.
pub fn due(bots: &[Bot], last: &mut HashMap<(String, String), i64>, now_ms: i64) -> (Vec<(String, String, String)>, Option<i64>) {
    let mut runs = Vec::new();
    let mut next: Option<i64> = None;
    for bot in bots.iter().filter(|bot| bot.enabled) {
        for schedule in bot.schedules.iter().filter(|schedule| schedule.enabled) {
            let key = (bot.handle.clone(), schedule.id.clone());
            let since = *last.entry(key.clone()).or_insert(now_ms);
            let upcoming = match next_match(&schedule.cron, since) {
                Some(at) if at <= now_ms => {
                    // The newest run that was missed decides whether it is still worth making up.
                    let mut newest = at;
                    while let Some(later) = next_match(&schedule.cron, newest).filter(|later| *later <= now_ms) {
                        newest = later;
                    }
                    if now_ms - newest <= CATCH_UP_MS {
                        runs.push((bot.handle.clone(), schedule.id.clone(), schedule.prompt.clone()));
                    }
                    last.insert(key, now_ms);
                    next_match(&schedule.cron, now_ms)
                }
                other => other,
            };
            if let Some(at) = upcoming {
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        }
    }
    (runs, next)
}

#[derive(Default)]
pub struct ScheduleState {
    last: HashMap<(String, String), i64>,
    /// Bots whose saved runs were read.
    loaded: HashSet<String>,
    /// Bumped when the bots change: the timer of an older round ends.
    round: u64,
    /// Each bot's next run, worked out when the timer is set: its row reads it.
    next: HashMap<String, i64>,
}

impl ScheduleState {
    /// When `handle` starts by itself next, as its row says it: the time, with the day when it is not today.
    pub(super) fn next_run(&self, handle: &str) -> Option<String> {
        use chrono::TimeZone as _;
        let at = chrono::Local.timestamp_millis_opt(*self.next.get(handle)?).single()?;
        let today = at.date_naive() == chrono::Local::now().date_naive();
        Some(at.format(if today { "%H:%M" } else { "%m-%d %H:%M" }).to_string())
    }
}

fn read_runs(handle: &str) -> HashMap<String, i64> {
    crate::bots::store()
        .dir(handle)
        .ok()
        .and_then(|dir| std::fs::read(dir.join(RUNS_FILE)).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

impl Workbench {
    /// The bots or their schedules changed (or the window just opened): look at what is due and
    /// set the timer for the next run. Only the window that hosts the bots keeps one.
    pub(super) fn bot_schedules_changed(&mut self, cx: &mut Context<Self>) {
        // What the bots watch for starts and stops with their schedules.
        self.bot_watches_changed(cx);
        self.bots.schedule.round += 1;
        let round = self.bots.schedule.round;
        if !self.prefs(cx).bots_enabled || !self.hosts_bots() {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            let Ok(Some(wait)) =
                this.update(cx, |this, cx| (this.bots.schedule.round == round).then(|| this.run_due_bot_schedules(cx)).flatten())
            else {
                break;
            };
            cx.background_executor().timer(wait.min(NAP_MAX)).await;
        })
        .detach();
    }

    /// Runs what is due; `None` when no schedule is left to wait for.
    fn run_due_bot_schedules(&mut self, cx: &mut Context<Self>) -> Option<Duration> {
        let state = &mut self.bots.schedule;
        let known: HashSet<String> = self.bots.list.iter().map(|bot| bot.handle.clone()).collect();
        state.last.retain(|(handle, _), _| known.contains(handle));
        state.loaded.retain(|handle| known.contains(handle));
        for bot in self.bots.list.iter().filter(|bot| !bot.schedules.is_empty()) {
            if state.loaded.insert(bot.handle.clone()) {
                for (id, at) in read_runs(&bot.handle) {
                    state.last.insert((bot.handle.clone(), id), at);
                }
            }
        }
        let now = chrono::Local::now().timestamp_millis();
        let before = state.last.clone();
        let (runs, next) = due(&self.bots.list, &mut state.last, now);
        if state.last != before {
            let mut by_bot: HashMap<String, HashMap<String, i64>> = HashMap::new();
            for ((handle, id), at) in &state.last {
                by_bot.entry(handle.clone()).or_default().insert(id.clone(), *at);
            }
            cx.background_spawn(async move {
                for (handle, runs) in by_bot {
                    if let (Ok(dir), Ok(json)) = (crate::bots::store().dir(&handle), serde_json::to_vec(&runs)) {
                        let _ = agentty_bridge::fsutil::write_private(&dir.join(RUNS_FILE), &json);
                    }
                }
            })
            .detach();
        }
        state.next = self
            .bots
            .list
            .iter()
            .filter(|bot| bot.enabled)
            .filter_map(|bot| {
                let at = bot
                    .schedules
                    .iter()
                    .filter(|schedule| schedule.enabled)
                    .filter_map(|schedule| next_match(&schedule.cron, now))
                    .min()?;
                Some((bot.handle.clone(), at))
            })
            .collect();
        cx.notify();
        for (handle, id, prompt) in runs {
            if let Err(why) = self.bot_post(&handle, Origin::Schedule(id), prompt, 0, cx) {
                eprintln!("agentty: bot @{handle} schedule not run: {why}");
            }
        }
        next.map(|at| Duration::from_millis((at - now).max(1000) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentty_bridge::bots::{BotAgent, BotSchedule};
    use chrono::{Local, TimeZone};

    fn local(d: u32, h: u32, mi: u32) -> i64 {
        Local.with_ymd_and_hms(2026, 11, d, h, mi, 0).single().unwrap().timestamp_millis()
    }

    fn bot(handle: &str, schedules: &[(&str, &str)]) -> Bot {
        let mut bot = Bot::new(handle, handle, "", BotAgent::Claude, std::env::temp_dir());
        bot.schedules = schedules
            .iter()
            .map(|(id, cron)| BotSchedule { id: id.to_string(), cron: cron.to_string(), prompt: format!("run {id}"), enabled: true })
            .collect();
        bot
    }

    #[test]
    fn expressions_are_five_fields_at_least_a_quarter_hour_apart() {
        let now = local(23, 10, 0);
        for good in ["0 * * * *", "0 9 * * *", "0 9 * * 1-5", "0 9 * * 1", "*/15 * * * *", "0,30 8-18 * * *", "15 3 1 * *"] {
            assert_eq!(check_at(good, now), Ok(()), "{good}");
        }
        for close in ["* * * * *", "*/5 * * * *", "0,10 9 * * *", "55,5 * * * *"] {
            assert_eq!(check_at(close, now), Err("bots.error_schedule_gap"), "{close}");
        }
        for bad in ["", "9am", "0 9 * *", "61 * * * *", "0 9 * * * *", "0 9 31 2 *"] {
            assert_eq!(check_at(bad, now), Err("bots.error_schedule"), "{bad}");
        }
    }

    #[test]
    fn a_schedule_runs_when_its_minute_comes_and_once() {
        let bots = [bot("release", &[("morning", "0 9 * * *")])];
        let mut last = HashMap::new();
        // First seen at 08:00: nothing from the past is made up, the timer waits for 09:00.
        let (runs, next) = due(&bots, &mut last, local(23, 8, 0));
        assert!(runs.is_empty());
        assert_eq!(next, Some(local(23, 9, 0)));
        let (runs, _) = due(&bots, &mut last, local(23, 8, 59));
        assert!(runs.is_empty());
        let (runs, next) = due(&bots, &mut last, local(23, 9, 0));
        assert_eq!(runs, vec![("release".to_string(), "morning".to_string(), "run morning".to_string())]);
        assert_eq!(next, Some(local(24, 9, 0)));
        // Looked at again in the same minute, and after a restart with the saved time: no second run.
        assert!(due(&bots, &mut last, local(23, 9, 0)).0.is_empty());
        let mut restarted = last.clone();
        assert!(due(&bots, &mut restarted, local(23, 9, 5)).0.is_empty());
    }

    #[test]
    fn missed_runs_are_made_up_once_when_less_than_a_day_old() {
        let bots = [bot("release", &[("hourly", "0 * * * *")])];
        let mut last = HashMap::from([(("release".to_string(), "hourly".to_string()), local(23, 9, 0))]);
        // The computer slept from 09:00 to 14:20: five runs missed, one made up.
        let (runs, next) = due(&bots, &mut last, local(23, 14, 20));
        assert_eq!(runs.len(), 1);
        assert_eq!(next, Some(local(23, 15, 0)));

        // Closed for three days with a daily schedule: the newest missed run is under a day old.
        let daily = [bot("release", &[("morning", "0 9 * * *")])];
        let mut last = HashMap::from([(("release".to_string(), "morning".to_string()), local(20, 9, 0))]);
        assert_eq!(due(&daily, &mut last, local(23, 10, 0)).0.len(), 1);
        // A monthly one missed two weeks ago is not run now.
        let monthly = [bot("release", &[("first", "0 9 1 * *")])];
        let mut last = HashMap::from([(
            ("release".to_string(), "first".to_string()),
            Local.with_ymd_and_hms(2026, 10, 20, 9, 0, 0).single().unwrap().timestamp_millis(),
        )]);
        let (runs, next) = due(&monthly, &mut last, local(15, 10, 0));
        assert!(runs.is_empty());
        assert_eq!(next, Some(Local.with_ymd_and_hms(2026, 12, 1, 9, 0, 0).single().unwrap().timestamp_millis()));
    }

    #[test]
    fn switched_off_bots_and_schedules_do_not_run_or_keep_a_timer() {
        let mut off = bot("release", &[("morning", "0 9 * * *")]);
        off.enabled = false;
        let mut last = HashMap::new();
        assert_eq!(due(&[off], &mut last, local(23, 9, 0)), (Vec::new(), None));
        let mut paused = bot("release", &[("morning", "0 9 * * *")]);
        paused.schedules[0].enabled = false;
        assert_eq!(due(&[paused], &mut last, local(23, 9, 0)), (Vec::new(), None));
        assert_eq!(due(&[bot("release", &[])], &mut last, local(23, 9, 0)), (Vec::new(), None));
        // The nearest of several bots' schedules sets the timer.
        let bots = [bot("a", &[("x", "0 12 * * *")]), bot("b", &[("y", "30 10 * * *")])];
        assert_eq!(due(&bots, &mut HashMap::new(), local(23, 10, 0)).1, Some(local(23, 10, 30)));
    }
}
