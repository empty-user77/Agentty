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

/// Banner + list, also while Agentty is in front (the app decides when to notify). No sound.
const PRESENT_OPTIONS: usize = 16 | 8;

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
        let action: Id = msg_send![response, actionIdentifier];
        let action_utf8: *const std::os::raw::c_char = msg_send![action, UTF8String];
        let action =
            if action_utf8.is_null() { String::new() } else { std::ffi::CStr::from_ptr(action_utf8).to_string_lossy().into_owned() };
        if !utf8.is_null() {
            let identifier = std::ffi::CStr::from_ptr(utf8).to_string_lossy();
            if let Some(tray) = response_action(&identifier, &action) {
                crate::status_item::push_action(tray);
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

/// Whether macOS lets Agentty notify, as last read by [`check_permission`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Permission {
    /// Not read yet, or no way to tell (a development build outside an app bundle).
    Unknown,
    NotAsked,
    Denied,
    Allowed,
}

static PERMISSION: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn permission() -> Permission {
    match PERMISSION.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Permission::NotAsked,
        2 => Permission::Denied,
        3 => Permission::Allowed,
        _ => Permission::Unknown,
    }
}

/// Reads the permission again (the answer comes a moment later, see [`permission`]).
pub fn check_permission() {
    let Some(center) = center() else { return };
    let completion = ConcreteBlock::new(|settings: Id| {
        if settings.is_null() {
            return;
        }
        // UNAuthorizationStatus: notDetermined 0, denied 1, authorized 2, provisional 3, ephemeral 4.
        let status: isize = unsafe { msg_send![settings, authorizationStatus] };
        let value = match status {
            0 => 1,
            1 => 2,
            2..=4 => 3,
            _ => 0,
        };
        PERMISSION.store(value, std::sync::atomic::Ordering::Relaxed);
    })
    .copy();
    unsafe {
        let _: () = msg_send![center, getNotificationSettingsWithCompletionHandler: &*completion];
    }
}

/// Asks macOS for permission (its dialog shows only while it was never answered), then reads it.
pub fn ask_permission() {
    let Some(center) = center() else { return };
    let completion = ConcreteBlock::new(|_granted: BOOL, _error: Id| check_permission()).copy();
    unsafe {
        let _: () = msg_send![center, requestAuthorizationWithOptions: (4usize | 2 | 1) completionHandler: &*completion];
    }
}

/// System Settings → Notifications, where a denied permission is turned back on.
pub const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";

pub fn show(pane_id: u64, title: &str, body: &str) {
    let Some(center) = center() else {
        return show_with_osascript(title, body);
    };
    unsafe {
        let content: Id = msg_send![class!(UNMutableNotificationContent), new];
        let _: () = msg_send![content, setTitle: ns_string(title)];
        let _: () = msg_send![content, setBody: ns_string(body)];
        // Silent on purpose: the system's alert played at full alert volume, one per notification,
        // so a few panes finishing together stacked into a loud, clipped burst over a call.
        let identifier = format!("agentty-pane-{pane_id}-{}", crate::ui::now_ms());
        let request: Id = msg_send![class!(UNNotificationRequest), requestWithIdentifier: ns_string(&identifier) content: content trigger: std::ptr::null_mut::<Object>()];
        let _: () = msg_send![center, addNotificationRequest: request withCompletionHandler: std::ptr::null_mut::<Object>()];
        let _: () = msg_send![content, release];
        remember(pane_id, identifier);
    }
}

/// The category of permission requests: Allow and Deny buttons.
const REQUEST_CATEGORY: &str = "agentty.request";
const ALLOW: &str = "agentty.allow";
const DENY: &str = "agentty.deny";

/// A permission request (`ask`, see `TerminalView::ask_serial`) of `pane_id`, with Allow / Deny
/// buttons that answer it without bringing Agentty up. `body` says what would be allowed.
pub fn show_request(pane_id: u64, ask: u64, title: &str, body: &str, allow: &str, deny: &str) {
    let Some(center) = center() else {
        return show_with_osascript(title, body);
    };
    unsafe {
        // Set again each time: the button titles follow the app's language.
        // Allow needs the Mac unlocked (authentication required); Deny is marked destructive.
        let allow: Id =
            msg_send![class!(UNNotificationAction), actionWithIdentifier: ns_string(ALLOW) title: ns_string(allow) options: 1usize];
        let deny: Id =
            msg_send![class!(UNNotificationAction), actionWithIdentifier: ns_string(DENY) title: ns_string(deny) options: 2usize];
        let actions: Id = msg_send![class!(NSMutableArray), array];
        let _: () = msg_send![actions, addObject: allow];
        let _: () = msg_send![actions, addObject: deny];
        let intents: Id = msg_send![class!(NSArray), array];
        let category: Id = msg_send![class!(UNNotificationCategory), categoryWithIdentifier: ns_string(REQUEST_CATEGORY) actions: actions intentIdentifiers: intents options: 0usize];
        let categories: Id = msg_send![class!(NSSet), setWithObject: category];
        let _: () = msg_send![center, setNotificationCategories: categories];

        let content: Id = msg_send![class!(UNMutableNotificationContent), new];
        let _: () = msg_send![content, setTitle: ns_string(title)];
        let _: () = msg_send![content, setBody: ns_string(body)];
        let _: () = msg_send![content, setCategoryIdentifier: ns_string(REQUEST_CATEGORY)];
        let identifier = format!("agentty-pane-{pane_id}-{}-ask-{ask}", crate::ui::now_ms());
        let request: Id = msg_send![class!(UNNotificationRequest), requestWithIdentifier: ns_string(&identifier) content: content trigger: std::ptr::null_mut::<Object>()];
        let _: () = msg_send![center, addNotificationRequest: request withCompletionHandler: std::ptr::null_mut::<Object>()];
        let _: () = msg_send![content, release];
        remember(pane_id, identifier);
    }
}

/// What a click on notification `identifier` asks for: its Allow / Deny button answers the
/// request it names; anything else (the notification itself) opens its pane.
fn response_action(identifier: &str, action: &str) -> Option<crate::status_item::TrayAction> {
    use crate::status_item::TrayAction;
    let rest = identifier.strip_prefix("agentty-pane-")?;
    let pane: u64 = rest.split('-').next()?.parse().ok()?;
    let ask = rest.split_once("-ask-").and_then(|(_, ask)| ask.parse::<u64>().ok());
    match (action, ask) {
        (ALLOW, Some(ask)) => Some(TrayAction::Answer { pane, ask, allow: true }),
        (DENY, Some(ask)) => Some(TrayAction::Answer { pane, ask, allow: false }),
        _ => Some(TrayAction::Focus(pane)),
    }
}

fn remember(pane_id: u64, identifier: String) {
    if pane_id == 0 {
        return;
    }
    let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    let list = shown.entry(pane_id).or_default();
    list.push(identifier);
    // A pane that keeps notifying keeps only its latest ones on record.
    let excess = list.len().saturating_sub(20);
    list.drain(..excess);
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
    use super::{applescript_string, response_action, ALLOW, DENY};
    use crate::status_item::TrayAction;

    #[test]
    fn escapes_applescript() {
        assert_eq!(applescript_string("say \"hi\" \\ now\n"), "\"say \\\"hi\\\" \\\\ now\"");
    }

    #[test]
    fn buttons_answer_the_request_they_came_with() {
        let default = "com.apple.UNNotificationDefaultActionIdentifier";
        let answer = |pane, ask, allow| Some(TrayAction::Answer { pane, ask, allow });
        assert_eq!(response_action("agentty-pane-7-1700000000000-ask-3", ALLOW), answer(7, 3, true));
        assert_eq!(response_action("agentty-pane-7-1700000000000-ask-3", DENY), answer(7, 3, false));
        // The notification itself opens its pane; so does a button on one that names no request.
        assert_eq!(response_action("agentty-pane-7-1700000000000-ask-3", default), Some(TrayAction::Focus(7)));
        assert_eq!(response_action("agentty-pane-7-1700000000000", ALLOW), Some(TrayAction::Focus(7)));
        assert_eq!(response_action("something-else", ALLOW), None);
    }
}
