//! State of the menu bar / tray item and the actions its menu queues for the app loop.

use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    ToggleMini,
    Focus(u64),
    OpenUsage,
    Quit,
}

static ACTIONS: Mutex<Vec<TrayAction>> = Mutex::new(Vec::new());

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // menu clicks and notification clicks (macOS)
pub fn push_action(action: TrayAction) {
    if let Ok(mut actions) = ACTIONS.lock() {
        actions.push(action);
    }
}

pub fn drain_actions() -> Vec<TrayAction> {
    ACTIONS.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default()
}

/// One agent pane in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayPane {
    pub id: u64,
    pub label: String,
    pub working: bool,
    pub waiting: bool,
}

/// Everything the menu shows, with strings already localized.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrayState {
    pub panes: Vec<TrayPane>,
    pub show_label: String,
    pub mini_label: String,
    pub quit_label: String,
    pub empty_label: String,
    pub mini: bool,
    /// "Usage · last 7 days" and one line per agent with cost and limit resets.
    pub usage_title: String,
    pub usage_lines: Vec<String>,
}

impl TrayState {
    pub fn working(&self) -> usize {
        self.panes.iter().filter(|p| p.working).count()
    }

    pub fn waiting(&self) -> usize {
        self.panes.iter().filter(|p| p.waiting).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_working_and_waiting() {
        let pane = |id, working, waiting| TrayPane { id, label: String::new(), working, waiting };
        let state = TrayState { panes: vec![pane(1, true, false), pane(2, false, true), pane(3, true, false)], ..Default::default() };
        assert_eq!((state.working(), state.waiting()), (2, 1));
    }
}
