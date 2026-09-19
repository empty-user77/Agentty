//! Windows / Linux: no menu bar item (a system-tray icon needs a toolkit GPUI doesn't provide).
//! The action queue still works, so notifications and the app loop behave the same; closing the
//! last window quits instead of hiding to the tray, and the tray popover never opens.

pub use crate::platform::tray::{drain_actions, push_action, wake_channel, TrayAction, TrayAnchor, TrayState};

pub fn anchor() -> Option<TrayAnchor> {
    None
}

pub fn set_highlighted(_on: bool) {}

pub fn watch_outside_clicks(_on: bool) {}

#[derive(Default)]
pub struct StatusItem;

impl StatusItem {
    pub fn new() -> Self {
        Self
    }

    pub fn update(&mut self, _state: &TrayState) {}
}
