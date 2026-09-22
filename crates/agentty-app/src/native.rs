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

/// Sets the window's opacity outright (no animation).
pub fn set_alpha(window: Id, alpha: f64) {
    unsafe {
        let _: () = msg_send![window, setAlphaValue: alpha];
    }
}

/// Fades the window to `alpha` over `duration` seconds, driven by Core Animation on the
/// compositor rather than by GPUI redrawing this window's real content at each intermediate step.
/// Used for the mini-mode fold instead of animating the real window's frame down to a sliver: that
/// forces a full relayout of everything in it (every terminal, every pane) at each frame of the
/// animation, which is the actual cost behind the stutter animating the frame directly has.
pub fn fade(window: Id, alpha: f64, duration: f64) {
    unsafe {
        let _: () = msg_send![objc::class!(NSAnimationContext), beginGrouping];
        let context: Id = msg_send![objc::class!(NSAnimationContext), currentContext];
        let _: () = msg_send![context, setDuration: duration];
        let animator: Id = msg_send![window, animator];
        let _: () = msg_send![animator, setAlphaValue: alpha];
        let _: () = msg_send![objc::class!(NSAnimationContext), endGrouping];
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

/// Opens macOS's own Emoji & Symbols palette. What it picks is typed into whatever has the
/// keyboard, which is the text field the user right-clicked in.
pub fn show_character_palette() {
    unsafe {
        let app: Id = msg_send![objc::class!(NSApplication), sharedApplication];
        let nil: Id = std::ptr::null_mut();
        let _: () = msg_send![app, orderFrontCharacterPalette: nil];
    }
}

/// Opens macOS's own colour panel (wheel, sliders, crayons, hex) starting at `color`.
pub fn open_color_panel(color: u32) {
    unsafe {
        let panel: Id = msg_send![objc::class!(NSColorPanel), sharedColorPanel];
        if panel.is_null() {
            return;
        }
        let component = |shift: u32| ((color >> shift) & 0xff) as f64 / 255.0;
        let ns_color: Id = msg_send![
            objc::class!(NSColor),
            colorWithSRGBRed: component(16) green: component(8) blue: component(0) alpha: 1.0f64
        ];
        let _: () = msg_send![panel, setColor: ns_color];
        let _: () = msg_send![panel, setShowsAlpha: NO];
        let nil: Id = std::ptr::null_mut();
        let _: () = msg_send![panel, orderFront: nil];
    }
}

/// The colour the panel shows right now, as `0xRRGGBB`.
pub fn color_panel_color() -> Option<u32> {
    unsafe {
        let panel: Id = msg_send![objc::class!(NSColorPanel), sharedColorPanel];
        if panel.is_null() {
            return None;
        }
        let color: Id = msg_send![panel, color];
        if color.is_null() {
            return None;
        }
        let space: Id = msg_send![objc::class!(NSColorSpace), sRGBColorSpace];
        let color: Id = msg_send![color, colorUsingColorSpace: space];
        if color.is_null() {
            return None;
        }
        let channel = |selector_value: f64| ((selector_value.clamp(0., 1.) * 255.0).round() as u32) & 0xff;
        let red: f64 = msg_send![color, redComponent];
        let green: f64 = msg_send![color, greenComponent];
        let blue: f64 = msg_send![color, blueComponent];
        Some((channel(red) << 16) | (channel(green) << 8) | channel(blue))
    }
}

/// Whether the colour panel is still on screen (the live preview stops when it closes).
pub fn color_panel_visible() -> bool {
    unsafe {
        let panel: Id = msg_send![objc::class!(NSColorPanel), sharedColorPanel];
        if panel.is_null() {
            return false;
        }
        let visible: objc::runtime::BOOL = msg_send![panel, isVisible];
        visible == YES
    }
}

/// Sends the window behind every other one (the debug driver paints a hidden window and steps back).
pub fn order_back(window: Id) {
    unsafe {
        let nil: Id = std::ptr::null_mut();
        let () = msg_send![window, orderBack: nil];
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
