//! Stand-ins for the AppKit window helpers on Windows and Linux. GPUI owns the window there and
//! exposes no handle to animate or order out, so these report "no native window" and the
//! features built on them (mini mode's fold animation, hiding to the menu bar) stay off; see
//! `crate::platform::HAS_MINI_MODE` / `HAS_STATUS_ITEM`.

pub use crate::platform::frame::Frame;

pub type Id = *mut std::ffi::c_void;

pub fn ns_window(_window: &gpui::Window) -> Option<Id> {
    None
}

pub fn frame(_window: Id) -> Frame {
    Frame { x: 0., y: 0., width: 0., height: 0. }
}

pub fn visible_frame(_window: Id) -> Option<Frame> {
    None
}

pub fn set_frame(_window: Id, _frame: Frame, _animate: bool) {}

pub fn order_out(_window: Id) {}

pub fn is_visible(_window: Id) -> bool {
    true
}

pub fn order_front_regardless(_window: Id) {}

pub fn order_back(_window: Id) {}

pub fn window_number(_window: Id) -> isize {
    0
}

pub fn register_windows_menu(_index: usize) {}

pub fn note_recent_folder(_path: &std::path::Path) {}
