//! One Agentty process per data folder.
//!
//! A window, a tab or a session never needs a second process: the running app opens as many
//! windows as asked. A second process only fights the first over settings and workspace files,
//! resumes the same agents again, and puts another icon in the menu bar. It used to happen on
//! macOS whenever `agentty` ran with arguments it does not know (the app's own folder is first on
//! a pane's `PATH`, so an agent trying `agentty --help` started a whole new app).
//!
//! So the app takes a lock on `<data dir>/agentty.lock` before anything else. A launch that finds
//! it taken brings the running app forward and exits. The one that holds it also closes Agentty
//! processes that run without the lock (versions from before it, or ones left from a crash in
//! the middle of an update), so a machine that already has several ends up with one.

use std::fs::File;
use std::path::{Path, PathBuf};

/// Commands the executable answers without starting the app (see `main`).
#[cfg_attr(not(unix), allow(dead_code))] // the cleanup runs on macOS and Linux only
pub const CLI_COMMANDS: &[&str] =
    &["mcp-connector", "worktree-guard", "statusline", "mcp-browser", "browser", "signal", "tasks", "db", "worktree-for", "notify"];

pub enum Lock {
    /// This process is the one Agentty for its data folder (keep the file open for its lifetime).
    Acquired(File),
    /// Another Agentty holds it.
    Held,
    /// The lock file could not be opened (read-only disk, odd permissions): go on without one
    /// rather than refuse to start.
    Unavailable,
}

pub fn acquire() -> Lock {
    acquire_at(&agentty_bridge::fsutil::data_dir().join("agentty.lock"))
}

fn acquire_at(path: &Path) -> Lock {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let Ok(file) = options.open(path) else { return Lock::Unavailable };
    match file.try_lock() {
        Ok(()) => Lock::Acquired(file),
        Err(std::fs::TryLockError::WouldBlock) => Lock::Held,
        Err(std::fs::TryLockError::Error(_)) => Lock::Unavailable,
    }
}

/// How this launch was asked for, from its arguments (without the program name).
#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    /// Start (or bring forward) the app.
    App,
    Help,
    Version,
    /// A command the executable does not know: say so, never start the app for it.
    Unknown(String),
}

pub fn launch_kind(args: &[String]) -> Launch {
    let Some(first) = args.first() else { return Launch::App };
    match first.as_str() {
        "help" | "--help" | "-h" => Launch::Help,
        "version" | "--version" | "-V" => Launch::Version,
        // What macOS itself may pass to an app it starts: a process serial number (older macOS)
        // and `-NS…` / `-Apple…` defaults (Xcode, `open --args`).
        arg if arg.starts_with("-psn_") || arg.starts_with("-NS") || arg.starts_with("-Apple") => Launch::App,
        // Windows and Linux hand folders and `agentty://` links over as arguments.
        _ if !cfg!(target_os = "macos") => Launch::App,
        arg => Launch::Unknown(arg.to_string()),
    }
}

pub fn help() -> String {
    format!(
        "Agentty {version}

Usage: agentty [command]

Without a command, opens Agentty (or brings the running one forward).
Inside an Agentty terminal:

  agentty browser …       drive the in-app browser (agentty browser --help)
  agentty tasks …         split work into parallel sessions (agentty tasks --help)
  agentty db …            read this project's databases (agentty db --help)
  agentty notify <text>   show a notification for this pane

  agentty --version       print the version
  agentty --help          print this help
",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// Brings the Agentty that holds the lock to the front (macOS: the running app with this app's
/// bundle identifier; elsewhere the caller has already handed its arguments over).
pub fn bring_running_forward() {
    #[cfg(target_os = "macos")]
    macos::activate_other_instance();
}

/// Closes other Agentty processes of this installation that run without the lock. Returns how many
/// there were. Called by the process that holds the lock, before it reads any saved state.
pub fn close_strays() -> usize {
    #[cfg(unix)]
    {
        let Ok(exe) = std::env::current_exe() else { return 0 };
        let me = std::process::id();
        let strays: Vec<u32> = crate::procinfo::process_parents()
            .into_keys()
            .filter(|pid| *pid != me)
            .filter(|pid| crate::procinfo::executable_path(*pid).is_some_and(|path| same_installation(&path, &exe)))
            .filter(|pid| runs_the_app(&crate::procinfo::process_args(*pid)))
            // A development or test copy with a data folder of its own is another Agentty.
            .filter(|pid| same_data_dir(&crate::procinfo::process_env(*pid)))
            .collect();
        if strays.is_empty() {
            return 0;
        }
        eprintln!("agentty: closing {} other Agentty process(es) of this installation: {strays:?}", strays.len());
        terminate(&strays);
        strays.len()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Whether `other` is this installation's executable: the same file, or — during an update — the
/// previous bundle it was moved to (`update.rs` renames it to `.Agentty-previous.app`).
#[cfg_attr(not(unix), allow(dead_code))] // the cleanup runs on macOS and Linux only
fn same_installation(other: &Path, exe: &Path) -> bool {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if canonical(other) == canonical(exe) {
        return true;
    }
    previous_bundle_exe(exe).is_some_and(|previous| other == previous)
}

#[cfg_attr(not(unix), allow(dead_code))] // the cleanup runs on macOS and Linux only
fn previous_bundle_exe(exe: &Path) -> Option<PathBuf> {
    // …/Agentty.app/Contents/MacOS/agentty → …/.Agentty-previous.app/Contents/MacOS/agentty
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    if bundle.extension()? != "app" {
        return None;
    }
    let relative = exe.strip_prefix(bundle).ok()?;
    Some(bundle.with_file_name(".Agentty-previous.app").join(relative))
}

/// Whether a process with environment `env` keeps its data where this one does (`AGENTTY_DATA_DIR`,
/// or the default when neither sets it).
#[cfg(unix)]
fn same_data_dir(env: &[String]) -> bool {
    let theirs = env.iter().find_map(|entry| entry.strip_prefix("AGENTTY_DATA_DIR=")).filter(|v| !v.is_empty());
    let ours = std::env::var("AGENTTY_DATA_DIR").ok().filter(|v| !v.is_empty());
    theirs == ours.as_deref()
}

/// Whether a process with `args` (program first) is the app rather than one of the executable's
/// short-lived commands (a hook, the status line, `agentty browser`).
#[cfg_attr(not(unix), allow(dead_code))] // the cleanup runs on macOS and Linux only
fn runs_the_app(args: &[String]) -> bool {
    !args.get(1).is_some_and(|first| CLI_COMMANDS.contains(&first.as_str()))
}

/// Asks the processes to quit, then ends the ones still there after a few seconds.
#[cfg(unix)]
fn terminate(pids: &[u32]) {
    let alive = |pid: u32| unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    for pid in pids {
        // SAFETY: plain signal to a process of this user.
        unsafe { libc::kill(*pid as libc::pid_t, libc::SIGTERM) };
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while pids.iter().any(|pid| alive(*pid)) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for pid in pids.iter().filter(|pid| alive(**pid)) {
        // SAFETY: as above.
        unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL) };
    }
}

#[cfg(target_os = "macos")]
mod macos {
    #![allow(unexpected_cfgs)] // objc 0.2 macros check a `cargo-clippy` cfg

    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    type Id = *mut Object;

    /// `NSApplicationActivateAllWindows | NSApplicationActivateIgnoringOtherApps`.
    const ACTIVATE: usize = 1 | 2;

    pub fn activate_other_instance() {
        unsafe {
            let bundle: Id = msg_send![class!(NSBundle), mainBundle];
            let identifier: Id = msg_send![bundle, bundleIdentifier];
            // A development build outside an app bundle has no identifier: nothing to find.
            if identifier.is_null() {
                return;
            }
            let apps: Id = msg_send![class!(NSRunningApplication), runningApplicationsWithBundleIdentifier: identifier];
            let count: usize = msg_send![apps, count];
            let me = std::process::id() as i32;
            for index in 0..count {
                let app: Id = msg_send![apps, objectAtIndex: index];
                let pid: i32 = msg_send![app, processIdentifier];
                if pid != me {
                    let _: bool = msg_send![app, activateWithOptions: ACTIVATE];
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_second_lock_on_the_same_folder_is_refused() {
        let path = std::env::temp_dir().join(format!("agentty-lock-test-{}", std::process::id())).join("agentty.lock");
        let first = acquire_at(&path);
        assert!(matches!(first, Lock::Acquired(_)));
        assert!(matches!(acquire_at(&path), Lock::Held));
        drop(first);
        assert!(matches!(acquire_at(&path), Lock::Acquired(_)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn unknown_commands_never_start_the_app_on_macos() {
        assert_eq!(launch_kind(&[]), Launch::App);
        assert_eq!(launch_kind(&args(&["--help"])), Launch::Help);
        assert_eq!(launch_kind(&args(&["--version"])), Launch::Version);
        assert_eq!(launch_kind(&args(&["-psn_0_12345"])), Launch::App);
        assert_eq!(launch_kind(&args(&["-NSDocumentRevisionsDebugMode", "YES"])), Launch::App);
        if cfg!(target_os = "macos") {
            assert_eq!(launch_kind(&args(&["status"])), Launch::Unknown("status".into()));
        } else {
            assert_eq!(launch_kind(&args(&["/home/me/project"])), Launch::App);
        }
    }

    #[test]
    fn short_lived_commands_are_not_the_app() {
        assert!(runs_the_app(&args(&["/Applications/Agentty.app/Contents/MacOS/agentty"])));
        assert!(runs_the_app(&args(&["agentty", "--help"])));
        assert!(!runs_the_app(&args(&["/Applications/Agentty.app/Contents/MacOS/agentty", "statusline"])));
        assert!(!runs_the_app(&args(&["agentty", "worktree-guard"])));
    }

    #[test]
    #[cfg(unix)]
    fn another_data_folder_is_another_agentty() {
        let ours = std::env::var("AGENTTY_DATA_DIR").ok().filter(|v| !v.is_empty());
        let same: Vec<String> = ours.iter().map(|dir| format!("AGENTTY_DATA_DIR={dir}")).collect();
        assert!(same_data_dir(&same));
        assert!(!same_data_dir(&["AGENTTY_DATA_DIR=/tmp/agentty-some-other-test-dir".to_string()]));
    }

    #[test]
    fn the_previous_bundle_during_an_update_is_the_same_installation() {
        let exe = Path::new("/Applications/Agentty.app/Contents/MacOS/agentty");
        assert!(same_installation(Path::new("/Applications/.Agentty-previous.app/Contents/MacOS/agentty"), exe));
        assert!(!same_installation(Path::new("/Users/me/dev/target/debug/agentty"), exe));
    }
}
