#![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

//! Keeps plugin work going while nobody looks: out of sight (a locked screen, a covered or minimized
//! window) macOS puts an app into App Nap and stretches its timers from seconds into minutes, and a
//! plugin's automation waiting on one of them stops for as long as the screen stays locked.
//!
//! Held as an `NSProcessInfo` activity, in one of two strengths: while a plugin runs, App Nap is
//! off (the machine may still sleep on its own schedule); while an automation reports it is working,
//! idle system sleep waits for it too. Other platforms have no App Nap: nothing to do there.

/// How much of the system's saving is held off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Nothing held: App Nap as usual.
    None,
    /// A plugin runs: timers keep their pace, the machine may sleep.
    Running,
    /// An automation is working: timers keep their pace and the machine stays awake.
    Working,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Level;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::sync::Mutex;

    /// `NSActivityUserInitiatedAllowingIdleSystemSleep` and `NSActivityUserInitiated`.
    const ALLOWING_SLEEP: u64 = 0x00FF_FFFF & !(1 << 20);
    const NO_IDLE_SLEEP: u64 = 0x00FF_FFFF;

    /// The level held and its activity token (retained), as an address so it can sit in a static.
    static HELD: Mutex<(Level, usize)> = Mutex::new((Level::None, 0));

    pub fn hold(level: Level) {
        let Ok(mut held) = HELD.lock() else { return };
        if held.0 == level {
            return;
        }
        // Safety: plain Foundation calls on the shared process info; the token was retained when
        // it was made and is released exactly once, here, when it is replaced.
        unsafe {
            let info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
            if held.1 != 0 {
                let token = held.1 as *mut Object;
                let () = msg_send![info, endActivity: token];
                let () = msg_send![token, release];
            }
            *held = (level, 0);
            let options = match level {
                Level::None => return,
                Level::Running => ALLOWING_SLEEP,
                Level::Working => NO_IDLE_SLEEP,
            };
            let text = std::ffi::CString::new("Agentty plugin automations keep running while the screen is locked").unwrap_or_default();
            let reason: *mut Object = msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()];
            let token: *mut Object = msg_send![info, beginActivityWithOptions: options reason: reason];
            if !token.is_null() {
                let _: *mut Object = msg_send![token, retain];
                held.1 = token as usize;
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Level;
    pub fn hold(_: Level) {}
}

/// Holds `level` from now on (replacing what was held); cheap when it is already held.
pub fn hold(level: Level) {
    imp::hold(level);
}

/// The level plugins call for: any automation working, else any plugin running.
pub fn level_for(working: bool, running: bool) -> Level {
    match (working, running) {
        (true, _) => Level::Working,
        (false, true) => Level::Running,
        _ => Level::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_outranks_running() {
        assert_eq!(level_for(true, true), Level::Working);
        assert_eq!(level_for(true, false), Level::Working, "a working automation is a running plugin");
        assert_eq!(level_for(false, true), Level::Running);
        assert_eq!(level_for(false, false), Level::None);
    }
}
