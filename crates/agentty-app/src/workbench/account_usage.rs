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
        let now = crate::ui::now_ms();
        let mut parts = vec![format!("{}  {}", self.agent.display_name(), self.cost.map(crate::ui::money).unwrap_or_else(|| "—".into()))];
        let window = self.limits.as_ref().and_then(|l| l.weekly.map(|w| ("tray.weekly", w)).or(l.session.map(|w| ("tray.session", w))));
        if let Some((label, window)) = window {
            let (days, hours, minutes) = time_left(window.resets_at, now);
            let left = if days > 0 {
                tf(cx, "tray.reset_days", &[("d", &days.to_string()), ("h", &hours.to_string())])
            } else {
                tf(cx, "tray.reset_hours", &[("h", &hours.to_string()), ("m", &minutes.to_string())])
            };
            parts.push(format!("{} {:.0}%", t(cx, label), window.used_percent));
            parts.push(left);
        }
        parts.join(" · ")
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
