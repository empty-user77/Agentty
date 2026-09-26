//! State of the menu bar / tray item and the actions its popover queues for the app loop.

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    ToggleMini,
    Focus(u64),
    /// A plugin automation row was clicked: brings up that plugin's workspace with this
    /// automation's tab in front.
    FocusPlugin {
        plugin: String,
        instance: String,
    },
    OpenUsage,
    /// Read spend and plan limits again now.
    RefreshUsage,
    Quit,
    /// The menu bar icon was clicked.
    TogglePopover,
    /// A click landed outside the popover (in another app).
    ClosePopover,
}

static ACTIONS: Mutex<Vec<TrayAction>> = Mutex::new(Vec::new());
static WAKE: Mutex<Option<UnboundedSender<()>>> = Mutex::new(None);

/// Queues an action and wakes the app loop, so clicks are handled right away.
pub fn push_action(action: TrayAction) {
    if let Ok(mut actions) = ACTIONS.lock() {
        actions.push(action);
    }
    if let Ok(wake) = WAKE.lock() {
        if let Some(wake) = wake.as_ref() {
            let _ = wake.unbounded_send(());
        }
    }
}

pub fn drain_actions() -> Vec<TrayAction> {
    ACTIONS.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default()
}

/// Receives one message per queued action (the app loop drains the queue when it arrives).
pub fn wake_channel() -> UnboundedReceiver<()> {
    let (tx, rx) = unbounded();
    if let Ok(mut wake) = WAKE.lock() {
        *wake = Some(tx);
    }
    rx
}

/// Where the menu bar icon is, for placing the popover under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrayAnchor {
    /// CGDirectDisplayID of the screen showing the icon.
    pub display: u32,
    /// Icon's horizontal center, from the screen's left edge.
    pub center_x: f64,
    /// Bottom of the menu bar, from the screen's top edge.
    pub bottom: f64,
    pub screen_width: f64,
}

impl TrayAnchor {
    /// Left edge of a `width` wide popover centered under the icon, kept `margin` inside the screen.
    pub fn popover_x(&self, width: f64, margin: f64) -> f64 {
        (self.center_x - width / 2.).clamp(margin, (self.screen_width - width - margin).max(margin))
    }
}

/// What the menu bar icon shows next to the mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrayState {
    /// Agents in the middle of a turn.
    pub working: usize,
    /// Agents waiting for a permission or an answer.
    pub asking: usize,
    /// Agents that finished and haven't been looked at yet.
    pub done: usize,
}

/// Spinner frames while agents work.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The menu bar item's text: a turning spinner with how many work, then `●` with how many need an
/// answer, and — only when nothing else is going on — `✓` with how many finished unseen.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn tray_title(state: &TrayState, frame: usize) -> String {
    let mut parts = Vec::new();
    if state.working > 0 {
        parts.push(format!("{} {}", SPINNER[frame % SPINNER.len()], state.working));
    }
    if state.asking > 0 {
        parts.push(format!("● {}", state.asking));
    }
    if parts.is_empty() && state.done > 0 {
        parts.push(format!("✓ {}", state.done));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join("  "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_says_working_asking_or_done() {
        let state = |working, asking, done| TrayState { working, asking, done };
        assert_eq!(tray_title(&state(0, 0, 0), 0), "");
        assert_eq!(tray_title(&state(2, 0, 0), 0), " ⠋ 2");
        assert_eq!(tray_title(&state(2, 0, 0), 1), " ⠙ 2", "the spinner turns");
        assert_eq!(tray_title(&state(0, 1, 0), 0), " ● 1");
        assert_eq!(tray_title(&state(0, 0, 3), 0), " ✓ 3", "finished agents get a check");
        assert_eq!(tray_title(&state(1, 1, 3), 0), " ⠋ 1  ● 1", "work and questions come before done");
    }

    #[test]
    fn keeps_popover_on_screen() {
        let anchor = |center_x| TrayAnchor { display: 1, center_x, bottom: 24., screen_width: 1440. };
        assert_eq!(anchor(700.).popover_x(340., 8.), 530.);
        assert_eq!(anchor(1420.).popover_x(340., 8.), 1092.);
        assert_eq!(anchor(20.).popover_x(340., 8.), 8.);
    }

    #[test]
    fn pushed_actions_wake_the_loop() {
        let mut rx = wake_channel();
        push_action(TrayAction::TogglePopover);
        assert!(rx.try_recv().is_ok());
        assert!(drain_actions().contains(&TrayAction::TogglePopover));
    }
}
