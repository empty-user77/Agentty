//! Keeps the machine awake while "Prevent sleep" is on (Settings → General).
//!
//! macOS and Linux hold the lock in a child process (`caffeinate`, `systemd-inhibit`) that is
//! killed when the setting goes off or Agentty exits; Windows tells the power manager directly.

#[cfg(not(windows))]
use std::sync::Mutex;

#[cfg(not(windows))]
static LOCK: Mutex<Option<std::process::Child>> = Mutex::new(None);

/// Whether a lock is being held right now (the UI only reports what actually took effect).
pub fn active() -> bool {
    #[cfg(not(windows))]
    {
        LOCK.lock().map(|guard| guard.is_some()).unwrap_or(false)
    }
    #[cfg(windows)]
    {
        windows_state()
    }
}

pub fn set(on: bool) {
    #[cfg(not(windows))]
    {
        let Ok(mut guard) = LOCK.lock() else { return };
        if on == guard.is_some() {
            return;
        }
        match guard.take() {
            Some(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            None => *guard = spawn_lock(),
        }
    }
    #[cfg(windows)]
    set_windows(on);
}

#[cfg(target_os = "macos")]
fn spawn_lock() -> Option<std::process::Child> {
    // -d display, -i idle system sleep; the lock ends with this process.
    std::process::Command::new("/usr/bin/caffeinate")
        .args(["-d", "-i"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn spawn_lock() -> Option<std::process::Child> {
    std::process::Command::new("systemd-inhibit")
        .args(["--what=idle:sleep", "--who=Agentty", "--why=Prevent sleep is on", "--mode=block", "sleep", "infinity"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

#[cfg(windows)]
static WINDOWS_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
fn windows_state() -> bool {
    WINDOWS_ON.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(windows)]
fn set_windows(on: bool) {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
    let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
    // Safety: a plain power-manager hint; the flags are the documented constants.
    let applied = unsafe { SetThreadExecutionState(flags) } != 0;
    WINDOWS_ON.store(on && applied, std::sync::atomic::Ordering::Relaxed);
}
