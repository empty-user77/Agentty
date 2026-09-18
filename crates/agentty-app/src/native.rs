//! Thin AppKit helpers for window placement GPUI doesn't expose (animated frames anchored at the
//! top edge, ordering a window out without closing it).

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

use objc::runtime::{Object, NO, YES};
use objc::{msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub type Id = *mut Object;

pub use crate::platform::frame::Frame;

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

/// Registers the top-level menu at `index` as NSApp's Window menu, so AppKit lists open windows
/// there and in the Dock menu.
pub fn register_windows_menu(index: usize) {
    unsafe {
        let app: Id = msg_send![objc::class!(NSApplication), sharedApplication];
        let main: Id = msg_send![app, mainMenu];
        if main.is_null() {
            return;
        }
        let count: isize = msg_send![main, numberOfItems];
        if index as isize >= count {
            return;
        }
        let item: Id = msg_send![main, itemAtIndex: index as isize];
        let menu: Id = msg_send![item, submenu];
        if !menu.is_null() {
            let _: () = msg_send![app, setWindowsMenu: menu];
        }
    }
}

/// Adds a folder to the app's recent documents, which the Dock lists (also while Agentty is not
/// running) and which reopen through `on_open_urls`.
pub fn note_recent_folder(path: &std::path::Path) {
    use cocoa::base::nil;
    use cocoa::foundation::NSString;
    let Some(path) = path.to_str() else { return };
    unsafe {
        let string = NSString::alloc(nil).init_str(path);
        let url: Id = msg_send![objc::class!(NSURL), fileURLWithPath: string isDirectory: YES];
        let _: () = msg_send![string, release];
        if url.is_null() {
            return;
        }
        let controller: Id = msg_send![objc::class!(NSDocumentController), sharedDocumentController];
        let _: () = msg_send![controller, noteNewRecentDocumentURL: url];
    }
}
