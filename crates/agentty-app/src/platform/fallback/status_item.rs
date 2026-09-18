//! Windows / Linux: no menu bar item (a system-tray icon needs a toolkit GPUI doesn't provide).
//! The tray state is still computed and its action queue still works, so notifications and the
//! app loop behave the same; closing the last window quits instead of hiding to the tray.

pub use crate::platform::tray::{drain_actions, TrayAction, TrayPane, TrayState};

#[derive(Default)]
pub struct StatusItem;

impl StatusItem {
    pub fn new() -> Self {
        Self
    }

    pub fn update(&mut self, _state: &TrayState) {}
}
