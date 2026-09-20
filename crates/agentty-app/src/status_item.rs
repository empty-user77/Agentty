//! macOS menu bar item: always-on Agentty icon with a running animation while agents work.
//! Clicking it toggles the popover (`tray_popover`), a GPUI panel placed under the icon.
//!
//! AppKit calls happen on the main thread (GPUI's foreground executor). Clicks are queued and
//! drained by the app loop, so no Rust closure is ever called from Objective-C.

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

use block::ConcreteBlock;
use cocoa::foundation::NSRect;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel, NO, YES};
use objc::{class, msg_send, sel, sel_impl};
use std::cell::Cell;
use std::ffi::CString;
use std::sync::Once;

type Id = *mut Object;
const NIL: Id = std::ptr::null_mut();

pub use crate::platform::tray::{drain_actions, push_action, wake_channel, TrayAction, TrayAnchor, TrayState};

// NSEventMask bits.
const LEFT_MOUSE_DOWN: u64 = 1 << 1;
const RIGHT_MOUSE_DOWN: u64 = 1 << 3;
const OTHER_MOUSE_DOWN: u64 = 1 << 25;

thread_local! {
    /// The status item's button (main thread only), for the popover's anchor and highlight.
    static BUTTON: Cell<Id> = const { Cell::new(NIL) };
    /// Global mouse monitor while the popover is open.
    static MONITOR: Cell<Id> = const { Cell::new(NIL) };
}

extern "C" fn button_clicked(_: &Object, _: Sel, _sender: Id) {
    push_action(TrayAction::TogglePopover);
}

fn target_class() -> &'static Class {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let mut decl = ClassDecl::new("AgenttyTrayTarget", class!(NSObject)).expect("AgenttyTrayTarget registered twice");
        unsafe { decl.add_method(sel!(buttonClicked:), button_clicked as extern "C" fn(&Object, Sel, Id)) };
        decl.register();
    });
    Class::get("AgenttyTrayTarget").expect("AgenttyTrayTarget class")
}

fn ns_string(text: &str) -> Id {
    let c = CString::new(text.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

/// Where the icon is on screen, or `None` while it is hidden (menu bar setting off).
pub fn anchor() -> Option<TrayAnchor> {
    let button = BUTTON.with(Cell::get);
    if button.is_null() {
        return None;
    }
    unsafe {
        let window: Id = msg_send![button, window];
        if window.is_null() {
            return None;
        }
        let screen: Id = msg_send![window, screen];
        if screen.is_null() {
            return None;
        }
        let icon: NSRect = msg_send![window, frame];
        let area: NSRect = msg_send![screen, frame];
        let description: Id = msg_send![screen, deviceDescription];
        let number: Id = msg_send![description, objectForKey: ns_string("NSScreenNumber")];
        let display: u32 = if number.is_null() { 0 } else { msg_send![number, unsignedIntValue] };
        Some(TrayAnchor {
            display,
            center_x: icon.origin.x + icon.size.width / 2. - area.origin.x,
            bottom: area.origin.y + area.size.height - icon.origin.y,
            screen_width: area.size.width,
        })
    }
}

/// Keeps the icon pressed while the popover is open, like a menu.
pub fn set_highlighted(on: bool) {
    let button = BUTTON.with(Cell::get);
    if !button.is_null() {
        unsafe {
            let _: () = msg_send![button, highlight: if on { YES } else { NO }];
        }
    }
}

/// While on, a click in any other app closes the popover. Clicks in Agentty's own windows take
/// key status from the popover, which closes it too.
pub fn watch_outside_clicks(on: bool) {
    let current = MONITOR.with(Cell::get);
    unsafe {
        if !current.is_null() {
            let _: () = msg_send![class!(NSEvent), removeMonitor: current];
            let _: () = msg_send![current, release];
            MONITOR.with(|m| m.set(NIL));
        }
        if on {
            let handler = ConcreteBlock::new(|_event: Id| push_action(TrayAction::ClosePopover)).copy();
            let monitor: Id = msg_send![class!(NSEvent), addGlobalMonitorForEventsMatchingMask: LEFT_MOUSE_DOWN | RIGHT_MOUSE_DOWN | OTHER_MOUSE_DOWN handler: &*handler];
            if !monitor.is_null() {
                let _: Id = msg_send![monitor, retain];
                MONITOR.with(|m| m.set(monitor));
            }
        }
    }
}

pub struct StatusItem {
    item: Id,
    target: Id,
    title: String,
    frame: usize,
}

impl StatusItem {
    pub fn new() -> Self {
        unsafe {
            let bar: Id = msg_send![class!(NSStatusBar), systemStatusBar];
            let item: Id = msg_send![bar, statusItemWithLength: -1.0f64]; // NSVariableStatusItemLength
            let _: Id = msg_send![item, retain];
            let button: Id = msg_send![item, button];
            // Agentty mark as a template image (adapts to light/dark menu bars), 18pt from a 36px bitmap.
            const MARK: &[u8] = include_bytes!("../assets/brand/menubar@2x.png");
            let data: Id = msg_send![class!(NSData), dataWithBytes: MARK.as_ptr() length: MARK.len()];
            let image: Id = msg_send![class!(NSImage), alloc];
            let image: Id = msg_send![image, initWithData: data];
            if !image.is_null() {
                let _: () = msg_send![image, setSize: cocoa::foundation::NSSize::new(18., 18.)];
                let _: () = msg_send![image, setTemplate: YES];
                let _: () = msg_send![button, setImage: image];
                let _: () = msg_send![button, setImagePosition: 2isize]; // NSImageLeft
                let _: () = msg_send![image, release];
            }
            let _: () = msg_send![button, setToolTip: ns_string("Agentty")];
            let target: Id = msg_send![target_class(), new];
            let _: () = msg_send![button, setTarget: target];
            let _: () = msg_send![button, setAction: sel!(buttonClicked:)];
            // On mouse down, like a menu.
            let _: isize = msg_send![button, sendActionOn: LEFT_MOUSE_DOWN | RIGHT_MOUSE_DOWN];
            BUTTON.with(|b| b.set(button));
            Self { item, target, title: String::new(), frame: 0 }
        }
    }

    /// Advances the running animation.
    pub fn update(&mut self, state: &TrayState) {
        if state.working > 0 {
            self.frame = (self.frame + 1) % crate::platform::tray::SPINNER.len();
        }
        let title = crate::platform::tray::tray_title(state, self.frame);
        if title != self.title {
            unsafe {
                let button: Id = msg_send![self.item, button];
                let _: () = msg_send![button, setTitle: ns_string(&title)];
            }
            self.title = title;
        }
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        BUTTON.with(|b| b.set(NIL));
        unsafe {
            let bar: Id = msg_send![class!(NSStatusBar), systemStatusBar];
            let _: () = msg_send![bar, removeStatusItem: self.item];
            let _: () = msg_send![self.item, release];
            if self.target != NIL {
                let _: () = msg_send![self.target, release];
            }
        }
    }
}
