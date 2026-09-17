//! Thin AppKit helpers for window placement GPUI doesn't expose (animated frames anchored at the
//! top edge, ordering a window out without closing it).

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

use cocoa::foundation::{NSPoint, NSRect, NSSize};
use objc::runtime::{Object, NO, YES};
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub type Id = *mut Object;

/// A rectangle in AppKit screen coordinates (origin at the bottom-left of the primary screen).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    fn ns(self) -> NSRect {
        NSRect::new(NSPoint::new(self.x, self.y), NSSize::new(self.width, self.height))
    }

    fn from_ns(rect: NSRect) -> Self {
        Self { x: rect.origin.x, y: rect.origin.y, width: rect.size.width, height: rect.size.height }
    }

    /// A `width`×`height` frame in the top-right corner of `area`, `margin` points from both edges.
    pub fn top_right(area: Frame, width: f64, height: f64, margin: f64) -> Frame {
        Frame { x: area.x + area.width - width - margin, y: area.y + area.height - height - margin, width, height }
    }

    /// Same frame with a new height, keeping the top edge fixed.
    pub fn with_height_from_top(self, height: f64) -> Frame {
        Frame { y: self.y + self.height - height, height, ..self }
    }
}

/// GPUI's own NSView (a subview of the window's content view).
pub fn ns_view(window: &gpui::Window) -> Option<Id> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::AppKit(appkit) => Some(appkit.ns_view.as_ptr() as Id),
        _ => None,
    }
}

pub fn ns_window(window: &gpui::Window) -> Option<Id> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::AppKit(appkit) => {
            let view = appkit.ns_view.as_ptr() as Id;
            let window: Id = unsafe { msg_send![view, window] };
            (!window.is_null()).then_some(window)
        }
        _ => None,
    }
}

pub fn frame(window: Id) -> Frame {
    Frame::from_ns(unsafe { msg_send![window, frame] })
}

/// The screen area not covered by the menu bar and Dock, for the screen showing `window`.
pub fn visible_frame(window: Id) -> Option<Frame> {
    unsafe {
        let screen: Id = msg_send![window, screen];
        if screen.is_null() {
            return None;
        }
        Some(Frame::from_ns(msg_send![screen, visibleFrame]))
    }
}

pub fn set_frame(window: Id, frame: Frame, animate: bool) {
    unsafe {
        let _: () = msg_send![window, setFrame: frame.ns() display: YES animate: if animate { YES } else { NO }];
    }
}

/// Hides the window without closing it (no Dock minimize animation).
pub fn order_out(window: Id) {
    unsafe {
        let _: () = msg_send![window, orderOut: std::ptr::null_mut::<Object>()];
    }
}

pub fn is_visible(window: Id) -> bool {
    unsafe {
        let visible: objc::runtime::BOOL = msg_send![window, isVisible];
        visible == YES
    }
}

/// Shows the window above others without activating Agentty (keyboard focus stays where it is).
pub fn order_front_regardless(window: Id) {
    unsafe {
        let () = msg_send![window, orderFrontRegardless];
    }
}

pub fn window_number(window: Id) -> isize {
    unsafe { msg_send![window, windowNumber] }
}

#[cfg(test)]
mod tests {
    use super::Frame;

    #[test]
    fn anchors_to_top_right() {
        let area = Frame { x: 0., y: 80., width: 1440., height: 820. };
        let mini = Frame::top_right(area, 300., 200., 16.);
        assert_eq!((mini.x, mini.y), (1124., 684.));
        let taller = mini.with_height_from_top(260.);
        assert_eq!(taller.y + taller.height, mini.y + mini.height);
    }
}
