//! State of the menu bar / tray item and the actions its popover queues for the app loop.

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    ToggleMini,
    Focus(u64),
    OpenUsage,
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
    pub working: usize,
    pub waiting: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

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
