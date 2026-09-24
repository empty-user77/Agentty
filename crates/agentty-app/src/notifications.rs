//! Desktop notifications. Inside the signed app bundle they go through UserNotifications, so they
//! carry Agentty's icon and clicking one opens the pane that sent it. Unbundled development
//! builds fall back to `osascript` (UserNotifications requires a bundle identifier).

#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

use block::{Block, ConcreteBlock};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel, BOOL};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CString;
use std::sync::Once;

type Id = *mut Object;

#[link(name = "UserNotifications", kind = "framework")]
extern "C" {}

fn ns_string(text: &str) -> Id {
    let c = CString::new(text.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn bundled() -> bool {
    unsafe {
        let bundle: Id = msg_send![class!(NSBundle), mainBundle];
        let identifier: Id = msg_send![bundle, bundleIdentifier];
        !identifier.is_null()
    }
}

/// Banner + list + sound, also while Agentty is in front (the app decides when to notify).
const PRESENT_OPTIONS: usize = 16 | 8 | 2;

extern "C" fn will_present(_: &Object, _: Sel, _center: Id, _notification: Id, handler: Id) {
    let handler = handler as *mut Block<(usize,), ()>;
    unsafe { (*handler).call((PRESENT_OPTIONS,)) };
}

extern "C" fn did_receive(_: &Object, _: Sel, _center: Id, response: Id, handler: Id) {
    unsafe {
        let notification: Id = msg_send![response, notification];
        let request: Id = msg_send![notification, request];
        let identifier: Id = msg_send![request, identifier];
        let utf8: *const std::os::raw::c_char = msg_send![identifier, UTF8String];
        if !utf8.is_null() {
            let identifier = std::ffi::CStr::from_ptr(utf8).to_string_lossy();
            if let Some(pane) =
                identifier.strip_prefix("agentty-pane-").and_then(|rest| rest.split('-').next()).and_then(|id| id.parse().ok())
            {
                crate::status_item::push_action(crate::status_item::TrayAction::Focus(pane));
            }
        }
        let handler = handler as *mut Block<(), ()>;
        (*handler).call(());
    }
}

fn center() -> Option<Id> {
    static SETUP: Once = Once::new();
    if !bundled() {
        return None;
    }
    let center: Id = unsafe { msg_send![class!(UNUserNotificationCenter), currentNotificationCenter] };
    if center.is_null() {
        return None;
    }
    SETUP.call_once(|| unsafe {
        let mut decl = ClassDecl::new("AgenttyNotificationDelegate", class!(NSObject)).expect("delegate class registered twice");
        decl.add_method(
            sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
            will_present as extern "C" fn(&Object, Sel, Id, Id, Id),
        );
        decl.add_method(
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            did_receive as extern "C" fn(&Object, Sel, Id, Id, Id),
        );
        decl.register();
        let delegate: Id = msg_send![Class::get("AgenttyNotificationDelegate").expect("delegate class"), new];
        let _: () = msg_send![center, setDelegate: delegate]; // kept for the app's lifetime
        let completion = ConcreteBlock::new(|_granted: BOOL, _error: Id| {}).copy();
        // alert | sound | badge
        let _: () = msg_send![center, requestAuthorizationWithOptions: (4usize | 2 | 1) completionHandler: &*completion];
    });
    Some(center)
}

/// Asks for notification permission up front (called at launch in the bundled app).
pub fn prepare() {
    let _ = center();
}

pub fn show(pane_id: u64, title: &str, body: &str) {
    let Some(center) = center() else {
        return show_with_osascript(title, body);
    };
    unsafe {
        let content: Id = msg_send![class!(UNMutableNotificationContent), new];
        let _: () = msg_send![content, setTitle: ns_string(title)];
        let _: () = msg_send![content, setBody: ns_string(body)];
        let sound: Id = msg_send![class!(UNNotificationSound), defaultSound];
        let _: () = msg_send![content, setSound: sound];
        let identifier = format!("agentty-pane-{pane_id}-{}", crate::ui::now_ms());
        let request: Id = msg_send![class!(UNNotificationRequest), requestWithIdentifier: ns_string(&identifier) content: content trigger: std::ptr::null_mut::<Object>()];
        let _: () = msg_send![center, addNotificationRequest: request withCompletionHandler: std::ptr::null_mut::<Object>()];
        let _: () = msg_send![content, release];
        if pane_id != 0 {
            let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
            let list = shown.entry(pane_id).or_default();
            list.push(identifier);
            // A pane that keeps notifying keeps only its latest ones on record.
            let excess = list.len().saturating_sub(20);
            list.drain(..excess);
        }
    }
}

/// Identifiers of the notifications each pane has in Notification Center.
static SHOWN: std::sync::Mutex<std::collections::BTreeMap<u64, Vec<String>>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Takes the pane's notifications out of Notification Center: what they asked for is done (the
/// question was answered, the pane was looked at), so none of them should still call the user.
pub fn withdraw(pane_id: u64) {
    let Some(identifiers) = SHOWN.lock().unwrap_or_else(|e| e.into_inner()).remove(&pane_id) else { return };
    let Some(center) = center() else { return };
    unsafe {
        let array: Id = msg_send![class!(NSMutableArray), array];
        for identifier in &identifiers {
            let _: () = msg_send![array, addObject: ns_string(identifier)];
        }
        let _: () = msg_send![center, removeDeliveredNotificationsWithIdentifiers: array];
    }
}

fn show_with_osascript(title: &str, body: &str) {
    let script = format!("display notification {} with title \"Agentty\" subtitle {}", applescript_string(body), applescript_string(title));
    std::thread::spawn(move || {
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

pub fn applescript_string(text: &str) -> String {
    let cleaned: String = text.chars().filter(|c| !c.is_control()).take(200).collect();
    format!("\"{}\"", cleaned.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::applescript_string;

    #[test]
    fn escapes_applescript() {
        assert_eq!(applescript_string("say \"hi\" \\ now\n"), "\"say \\\"hi\\\" \\\\ now\"");
    }
}
