//! macOS menu bar item: always-on Agentty icon with a running animation while agents work, a
//! menu listing agent panes, and entries to show the window, enter mini mode or quit.
//!
//! AppKit calls happen on the main thread (GPUI's foreground executor). Menu clicks are queued
//! and drained by the app loop, so no Rust closure is ever called from Objective-C.

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel, NO, YES};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CString;
use std::sync::{Mutex, Once};

type Id = *mut Object;
const NIL: Id = std::ptr::null_mut();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    ToggleMini,
    Focus(u64),
    OpenUsage,
    Quit,
}

static ACTIONS: Mutex<Vec<TrayAction>> = Mutex::new(Vec::new());

pub fn push_action(action: TrayAction) {
    if let Ok(mut actions) = ACTIONS.lock() {
        actions.push(action);
    }
}

pub fn drain_actions() -> Vec<TrayAction> {
    ACTIONS.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default()
}

const TAG_SHOW: isize = 1;
const TAG_MINI: isize = 2;
const TAG_QUIT: isize = 3;
const TAG_USAGE: isize = 4;
const TAG_PANE: isize = 1000;

extern "C" fn tray_action(_: &Object, _: Sel, sender: Id) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    let action = match tag {
        TAG_SHOW => TrayAction::Show,
        TAG_MINI => TrayAction::ToggleMini,
        TAG_QUIT => TrayAction::Quit,
        TAG_USAGE => TrayAction::OpenUsage,
        t if t >= TAG_PANE => TrayAction::Focus((t - TAG_PANE) as u64),
        _ => return,
    };
    push_action(action);
}

fn target_class() -> &'static Class {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let mut decl = ClassDecl::new("AgenttyTrayTarget", class!(NSObject)).expect("AgenttyTrayTarget registered twice");
        unsafe { decl.add_method(sel!(trayAction:), tray_action as extern "C" fn(&Object, Sel, Id)) };
        decl.register();
    });
    Class::get("AgenttyTrayTarget").expect("AgenttyTrayTarget class")
}

fn ns_string(text: &str) -> Id {
    let c = CString::new(text.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
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

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct StatusItem {
    item: Id,
    target: Id,
    menu_state: Option<TrayState>,
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
            }
            if !image.is_null() {
                let _: () = msg_send![image, setTemplate: YES];
                let _: () = msg_send![button, setImage: image];
                let _: () = msg_send![button, setImagePosition: 2isize]; // NSImageLeft
            }
            let _: () = msg_send![button, setToolTip: ns_string("Agentty")];
            let target: Id = msg_send![target_class(), new];
            Self { item, target, menu_state: None, title: String::new(), frame: 0 }
        }
    }

    /// Advances the running animation and rebuilds the menu when its contents changed.
    pub fn update(&mut self, state: &TrayState) {
        let working = state.working();
        let title = if working > 0 {
            self.frame = (self.frame + 1) % SPINNER.len();
            format!(" {} {working}", SPINNER[self.frame])
        } else if state.waiting() > 0 {
            format!(" ● {}", state.waiting())
        } else {
            String::new()
        };
        if title != self.title {
            unsafe {
                let button: Id = msg_send![self.item, button];
                let _: () = msg_send![button, setTitle: ns_string(&title)];
            }
            self.title = title;
        }
        if self.menu_state.as_ref() != Some(state) {
            self.rebuild_menu(state);
            self.menu_state = Some(state.clone());
        }
    }

    fn add_item(&self, menu: Id, title: &str, tag: isize, enabled: bool) -> Id {
        unsafe {
            let item: Id = msg_send![class!(NSMenuItem), alloc];
            let item: Id = msg_send![item, initWithTitle: ns_string(title) action: sel!(trayAction:) keyEquivalent: ns_string("")];
            let _: () = msg_send![item, setTarget: self.target];
            let _: () = msg_send![item, setTag: tag];
            if !enabled {
                let _: () = msg_send![item, setAction: sel!(noSuchAction:)];
                let _: () = msg_send![item, setEnabled: NO];
            }
            let _: () = msg_send![menu, addItem: item];
            let _: () = msg_send![item, release];
            item
        }
    }

    fn rebuild_menu(&self, state: &TrayState) {
        unsafe {
            let menu: Id = msg_send![class!(NSMenu), new];
            let _: () = msg_send![menu, setAutoenablesItems: NO];
            self.add_item(menu, &state.show_label, TAG_SHOW, true);
            let mini = self.add_item(menu, &state.mini_label, TAG_MINI, true);
            let _: () = msg_send![mini, setState: if state.mini { 1isize } else { 0isize }];
            let separator: Id = msg_send![class!(NSMenuItem), separatorItem];
            let _: () = msg_send![menu, addItem: separator];
            if !state.usage_lines.is_empty() {
                self.add_item(menu, &state.usage_title, 0, false);
                for line in &state.usage_lines {
                    self.add_item(menu, &format!("   {line}"), TAG_USAGE, true);
                }
                let separator: Id = msg_send![class!(NSMenuItem), separatorItem];
                let _: () = msg_send![menu, addItem: separator];
            }
            if state.panes.is_empty() {
                self.add_item(menu, &state.empty_label, 0, false);
            }
            for pane in &state.panes {
                let marker = if pane.working {
                    "◐"
                } else if pane.waiting {
                    "●"
                } else {
                    "○"
                };
                self.add_item(menu, &format!("{marker}  {}", pane.label), TAG_PANE + pane.id as isize, true);
            }
            let separator: Id = msg_send![class!(NSMenuItem), separatorItem];
            let _: () = msg_send![menu, addItem: separator];
            self.add_item(menu, &state.quit_label, TAG_QUIT, true);
            let _: () = msg_send![self.item, setMenu: menu];
            let _: () = msg_send![menu, release];
        }
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
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
