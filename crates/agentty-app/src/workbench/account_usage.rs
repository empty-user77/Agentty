//! Per-agent spend and plan-limit resets for the menu bar (local transcripts and saved limits).

use super::Workbench;
use crate::i18n::{t, tf};
use agentty_bridge::limits::{time_left, AgentLimits};
use agentty_bridge::model::Agent;
use agentty_bridge::usage::UsageScanner;
use gpui::{App, AppContext as _, Context};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DAYS: i64 = 7;
const REFRESH: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsage {
    pub agent: Agent,
    /// Spend over the last `DAYS` days (`None` when no request had a known price).
    pub cost: Option<f64>,
    pub limits: Option<AgentLimits>,
}

impl AccountUsage {
    /// "Claude Code  $661 · weekly 42% · resets in 2d 5h".
    pub fn menu_line(&self, cx: &App) -> String {
        let mut parts = vec![format!("{}  {}", self.agent.display_name(), self.cost_label())];
        if let Some(limit) = self.limit() {
            parts.push(format!("{} {:.0}%", t(cx, limit.label), limit.used_percent));
            parts.push(limit.resets_in(cx));
        }
        parts.join(" · ")
    }

    /// "$661", or "—" when no request had a known price.
    pub fn cost_label(&self) -> String {
        self.cost.map(crate::ui::money).unwrap_or_else(|| "—".into())
    }

    /// Every known plan-limit window: the 5-hour one, then the weekly one.
    pub fn windows(&self) -> Vec<Limit> {
        let Some(limits) = self.limits.as_ref() else { return Vec::new() };
        [("tray.session", limits.session), ("tray.weekly", limits.weekly)]
            .into_iter()
            .filter_map(|(label, window)| window.map(|w| Limit { label, used_percent: w.used_percent, resets_at: w.resets_at }))
            .collect()
    }

    /// The plan limit that matters most: weekly when known, else the 5-hour window.
    pub fn limit(&self) -> Option<Limit> {
        let limits = self.limits.as_ref()?;
        let (label, window) = limits.weekly.map(|w| ("tray.weekly", w)).or(limits.session.map(|w| ("tray.session", w)))?;
        Some(Limit { label, used_percent: window.used_percent, resets_at: window.resets_at })
    }
}

/// One plan-limit window, ready to show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limit {
    /// i18n key: "weekly" or "5h".
    pub label: &'static str,
    pub used_percent: f64,
    pub resets_at: i64,
}

impl Limit {
    /// "resets in 2d 5h".
    pub fn resets_in(&self, cx: &App) -> String {
        let (days, hours, minutes) = time_left(self.resets_at, crate::ui::now_ms());
        if days > 0 {
            tf(cx, "tray.reset_days", &[("d", &days.to_string()), ("h", &hours.to_string())])
        } else {
            tf(cx, "tray.reset_hours", &[("h", &hours.to_string()), ("m", &minutes.to_string())])
        }
    }
}

fn compute(scanner: &Mutex<UsageScanner>, now_ms: u64) -> Vec<AccountUsage> {
    let since = now_ms as i64 - DAYS * 86_400_000;
    [Agent::Claude, Agent::Codex]
        .into_iter()
        .filter(|agent| agentty_bridge::usage::root_dir(*agent).is_dir())
        .map(|agent| {
            let usage = scanner.lock().map(|mut s| s.scan(agent)).unwrap_or_default();
            let recent: Vec<_> = usage.requests.iter().filter(|r| r.timestamp_ms >= since).collect();
            let priced: Vec<f64> = recent.iter().filter_map(|r| r.cost).collect();
            let cost = (!priced.is_empty()).then(|| priced.iter().sum());
            AccountUsage { agent, cost, limits: agentty_bridge::limits::latest(agent, now_ms) }
        })
        .filter(|u| u.cost.is_some() || u.limits.is_some())
        .collect()
}

impl Workbench {
    pub(super) fn start_account_usage(&mut self, cx: &mut Context<Self>) {
        let scanner: Arc<Mutex<UsageScanner>> = Arc::default();
        cx.spawn(async move |this, cx| loop {
            let scanner = scanner.clone();
            let usage = cx.background_spawn(async move { compute(&scanner, crate::ui::now_ms()) }).await;
            if this
                .update(cx, |this, cx| {
                    if this.account_usage != usage {
                        this.account_usage = usage;
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
            cx.background_executor().timer(REFRESH).await;
        })
        .detach();
    }
}
