//! Keeps the in-app browser within bounds. Every terminal can have a browser whose pages keep
//! running out of sight (`terminal_browser`); left alone, a day of work would leave dozens of
//! pages running. Two limits, both in Settings → Browser:
//!
//! - **How many** terminals' browsers run out of sight (`background_limit`): beyond it, the one
//!   seen longest ago is unloaded.
//! - **How much memory** all in-app browser pages use together, plugins' included, as a share of
//!   this computer's memory (`memory_limit`): above it, the browsers out of sight are unloaded,
//!   oldest first, until the estimate is under the limit.
//!
//! Unloading keeps where each page was: the page loads again when its terminal is selected, or
//! when its agent sends a command. The browser on screen is never unloaded, and a browser whose
//! agent worked in the last two minutes goes after all the idle ones.

use super::browser::BrowserPanel;
use super::Workbench;
use crate::settings::settings;
use gpui::Context;
use std::collections::HashSet;
use std::time::{Duration, Instant};

const CHECK_EVERY: Duration = Duration::from_secs(15);
/// An agent that sent a command this recently is at work: its browser is unloaded last.
const AT_WORK: Duration = Duration::from_secs(120);

/// What one check found and did.
#[derive(Debug, Default)]
pub(super) struct BudgetReport {
    pub total_memory: u64,
    pub browser_memory: u64,
    pub unloaded: Vec<u64>,
}

impl Workbench {
    pub(super) fn start_browser_budget(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(CHECK_EVERY).await;
            if this.update(cx, |this, cx| this.enforce_browser_budget(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    /// Unloads the browsers out of sight that are over the limits, oldest first.
    pub(super) fn enforce_browser_budget(&mut self, cx: &mut Context<Self>) -> BudgetReport {
        let prefs = settings(cx).browser.clone();
        // Idle ones before those whose agent is at work, and among each the one seen longest ago.
        let mut candidates: Vec<(bool, Instant, u64)> =
            self.pane_browsers.iter().filter(|(_, b)| !b.unloaded).map(|(pane, b)| (at_work(b), b.last_shown, *pane)).collect();
        candidates.sort();
        let mut report = BudgetReport { total_memory: system_memory(), ..Default::default() };
        let limit = prefs.background_limit.max(1) as usize;
        while candidates.len() > limit {
            let (_, _, pane) = candidates.remove(0);
            self.unload_terminal_browser(pane);
            report.unloaded.push(pane);
        }
        report.browser_memory = self.browser_memory();
        if prefs.memory_limit > 0 && report.total_memory > 0 {
            let cap = report.total_memory / 100 * u64::from(prefs.memory_limit.min(100));
            let mut used = report.browser_memory;
            while used > cap && !candidates.is_empty() {
                let (_, _, pane) = candidates.remove(0);
                let freed = self.pane_browsers.get(&pane).map_or(0, |b| memory_of(views_of(b)));
                self.unload_terminal_browser(pane);
                report.unloaded.push(pane);
                used = used.saturating_sub(freed);
            }
        }
        if !report.unloaded.is_empty() {
            cx.notify();
        }
        report
    }

    fn unload_terminal_browser(&mut self, pane: u64) {
        if let Some(browser) = self.pane_browsers.get_mut(&pane) {
            for tab in &mut browser.tabs {
                tab.unload();
            }
            browser.unloaded = true;
        }
    }

    /// Memory of every in-app browser page: the browser on screen, the one set aside for a plugin
    /// workspace, the terminals' out of sight, and plugins' pages. A process two pages share is
    /// counted once.
    fn browser_memory(&self) -> u64 {
        let mut pids = HashSet::new();
        for browser in self.browser.iter().chain(self.stashed_browser.iter()).chain(self.pane_browsers.values()) {
            pids.extend(views_of(browser));
        }
        for page in &self.plugin_browsers {
            if let Some(pid) = page.webview.borrow().as_ref().and_then(|view| view.web_process_id()) {
                pids.insert(pid);
            }
        }
        memory_of(pids)
    }

    /// `debug browser-budget unload <pane>`: unloads that terminal's browser now, as a limit would.
    pub(super) fn debug_unload_browser(&mut self, pane: u64) {
        self.unload_terminal_browser(pane);
        eprintln!("browser-budget: unloaded terminal {pane}: {}", self.pane_browsers.get(&pane).is_some_and(|b| b.unloaded));
    }

    /// `debug browser-budget`: a check now, and what each browser out of sight uses.
    pub(super) fn debug_browser_budget(&mut self, cx: &mut Context<Self>) {
        let report = self.enforce_browser_budget(cx);
        let mut lines = vec![format!(
            "browser-budget: memory {} MB of {} MB, unloaded {:?}",
            report.browser_memory / 1_048_576,
            report.total_memory / 1_048_576,
            report.unloaded
        )];
        let mut kept: Vec<_> = self.pane_browsers.iter().collect();
        kept.sort_by_key(|(pane, _)| **pane);
        for (pane, browser) in kept {
            lines.push(format!(
                "browser-budget: terminal {pane}: {} MB, unloaded={}, at work={}",
                memory_of(views_of(browser)) / 1_048_576,
                browser.unloaded,
                at_work(browser)
            ));
        }
        eprintln!("{}", lines.join("\n"));
    }
}

fn at_work(browser: &BrowserPanel) -> bool {
    browser.tabs.iter().any(|tab| tab.driven_at.is_some_and(|at| at.elapsed() < AT_WORK))
}

/// The page processes of a browser's tabs.
fn views_of(browser: &BrowserPanel) -> HashSet<i32> {
    browser.tabs.iter().filter_map(|tab| tab.webview.borrow().as_ref().and_then(|view| view.web_process_id())).collect()
}

fn memory_of(pids: HashSet<i32>) -> u64 {
    pids.into_iter().map(footprint).sum()
}

/// Memory a process uses as the system counts it (Activity Monitor's "Memory").
#[cfg(target_os = "macos")]
fn footprint(pid: i32) -> u64 {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a rusage_info_v4, matching the RUSAGE_INFO_V4 flavor.
    let result = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, (&mut info as *mut libc::rusage_info_v4).cast()) };
    if result == 0 {
        info.ri_phys_footprint
    } else {
        0
    }
}

#[cfg(not(target_os = "macos"))]
fn footprint(_pid: i32) -> u64 {
    0
}

/// This computer's memory in bytes.
#[cfg(target_os = "macos")]
fn system_memory() -> u64 {
    let mut size: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: `hw.memsize` is a 64-bit integer, and `size` / `len` describe a buffer of that size.
    let result = unsafe { libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut size as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) };
    if result == 0 {
        size
    } else {
        0
    }
}

#[cfg(not(target_os = "macos"))]
fn system_memory() -> u64 {
    0
}
