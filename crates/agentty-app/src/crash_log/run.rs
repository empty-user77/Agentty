//! A run that ends without a word. A process killed from outside (the system out of memory, a
//! `kill -9`, the session or the computer going down) runs no code of its own and leaves nothing
//! in `crash.log`. So each run keeps a small file, `run.state`, saying it is alive and how it is
//! doing; a quit, a panic or a caught signal removes it. A file still there at the next start is a
//! run that ended unrecorded, and that start writes the record for it — with the last
//! measurements it left and the end of what it printed.

use super::context;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often the state file is brought up to date. A crash record is as fresh as this.
const BEAT: Duration = Duration::from_secs(120);
/// How long after the start the state is written the second time.
const SETTLE: Duration = Duration::from_secs(20);
/// How much of the last run's printed output goes into the record written for it.
const OUTPUT_TAIL: usize = 4096;

/// The app quit the way it should (its quit hooks ran).
static QUIT: AtomicBool = AtomicBool::new(false);
/// Someone asked for the quit (see `quit_asked`).
static ASKED: AtomicBool = AtomicBool::new(false);
/// A record of this run's end is in `crash.log` already (a panic, a signal).
static RECORDED: AtomicBool = AtomicBool::new(false);

pub fn state_path() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("run.state")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

fn local_time(unix: u64) -> String {
    chrono::DateTime::from_timestamp(unix as i64, 0)
        .map(|time| time.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| unix.to_string())
}

/// What a run says of itself in its state file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub pid: u32,
    pub version: String,
    pub started: u64,
    /// When the file was last brought up to date.
    pub seen: u64,
    /// The measurements and notes of then (see `context`).
    pub context: String,
}

impl State {
    fn to_text(&self) -> String {
        format!("pid={}\nversion={}\nstarted={}\nseen={}\n{}", self.pid, self.version, self.started, self.seen, self.context)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let mut field = |name: &str| lines.next()?.strip_prefix(name)?.strip_prefix('=').map(str::to_string);
        let (pid, version, started, seen) = (field("pid")?, field("version")?, field("started")?, field("seen")?);
        let context = lines.collect::<Vec<_>>().join("\n");
        Some(Self { pid: pid.parse().ok()?, version, started: started.parse().ok()?, seen: seen.parse().ok()?, context })
    }
}

fn write_state(started: u64) {
    let state = State {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        started,
        seen: now_unix(),
        context: context::refresh(),
    };
    let path = state_path();
    let temporary = path.with_extension("state.new");
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Written beside and moved over: a kill in the middle leaves the last whole file.
    let written = options.open(&temporary).and_then(|mut file| file.write_all(state.to_text().as_bytes()));
    if written.is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

/// When the computer was started, for telling a restart from a kill; `None` where unknown.
fn boot_time() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        return stat.lines().find_map(|line| line.strip_prefix("btime "))?.trim().parse().ok();
    }
    #[cfg(target_os = "macos")]
    {
        let mut time = libc::timeval { tv_sec: 0, tv_usec: 0 };
        let mut size = std::mem::size_of::<libc::timeval>();
        // SAFETY: `time` is as large as `size` says, and the name is a NUL-ended C string.
        let read = unsafe {
            libc::sysctlbyname(c"kern.boottime".as_ptr(), (&mut time as *mut libc::timeval).cast(), &mut size, std::ptr::null_mut(), 0)
        };
        return (read == 0 && time.tv_sec > 0).then_some(time.tv_sec as u64);
    }
    #[allow(unreachable_code)]
    None
}

/// The record for a run that left its state file behind.
pub fn unrecorded_end(last: &State, boot: Option<u64>, output_tail: &str) -> String {
    let restarted = boot.is_some_and(|boot| boot > last.seen);
    let cause = if restarted {
        "The computer was restarted (or lost power) after it was last seen: it went down with it."
    } else {
        "It did not quit and left no crash: it was ended from outside (killed, the system out of memory, the session ending) \
         or by something that cannot be caught."
    };
    let mut text = format!(
        "the run before this one (Agentty {}, pid {}) ended without a record\n{cause}\nstarted {}, last seen {} (its state is written every {} minutes)\n{}",
        last.version,
        last.pid,
        local_time(last.started),
        local_time(last.seen),
        BEAT.as_secs() / 60,
        last.context.trim_end(),
    );
    if !output_tail.trim().is_empty() {
        text.push_str("\nthe last it printed:\n");
        text.push_str(output_tail.trim_end());
    }
    text
}

/// The end of a file, from a line's start where it can be.
fn tail(path: &std::path::Path, bytes: usize) -> String {
    let Ok(data) = std::fs::read(path) else { return String::new() };
    let from = data.len().saturating_sub(bytes);
    let text = String::from_utf8_lossy(&data[from..]).into_owned();
    match (from, text.find('\n')) {
        (0, _) | (_, None) => text,
        (_, Some(line_end)) => text[line_end + 1..].to_string(),
    }
}

/// At start, after the lock that makes this the only Agentty on its data folder: writes the
/// record of a run that ended unrecorded, then starts this run's own state file.
pub fn start() {
    let path = state_path();
    if let Some(last) = std::fs::read_to_string(&path).ok().as_deref().and_then(State::parse) {
        let output = tail(&agentty_bridge::fsutil::data_dir().join("stderr.prev.log"), OUTPUT_TAIL);
        super::append(&unrecorded_end(&last, boot_time(), &output));
    }
    let started = now_unix();
    write_state(started);
    let beat = std::thread::Builder::new().name("crash-state".into()).spawn(move || loop {
        // Soon after the start once (the windows and what they hold are there by then), then at
        // its own pace.
        static SETTLED: AtomicBool = AtomicBool::new(false);
        std::thread::sleep(if SETTLED.swap(true, Ordering::Relaxed) { BEAT } else { SETTLE });
        if QUIT.load(Ordering::Relaxed) || RECORDED.load(Ordering::Relaxed) {
            return;
        }
        write_state(started);
    });
    if let Err(err) = beat {
        eprintln!("agentty: the crash state is not kept up to date: {err}");
    }
    // SAFETY: registers a plain function with no captured state.
    unsafe { libc_atexit(on_exit) };
}

#[cfg(unix)]
unsafe fn libc_atexit(function: extern "C" fn()) {
    // SAFETY: the caller's.
    unsafe { libc::atexit(function) };
}

#[cfg(not(unix))]
unsafe fn libc_atexit(_function: extern "C" fn()) {}

pub fn quit_asked() {
    ASKED.store(true, Ordering::Relaxed);
}

/// Whether a quit hook that nobody asked for is the event loop having ended on its own. Only
/// Linux's loop does (a lost display server ends it, and the toolkit then runs the quit hooks as
/// for any quit); elsewhere the system quits the app without going through ours (macOS at log
/// out, the Dock's Quit), and that is a quit. With no window left, the last one was closed.
fn ended_on_its_own(asked: bool, windows_open: bool, linux: bool) -> bool {
    linux && !asked && windows_open
}

/// The app's quit hook ran (see `crash_log::quit`).
pub fn quit(windows_open: bool) {
    QUIT.store(true, Ordering::Relaxed);
    let linux = cfg!(any(target_os = "linux", target_os = "freebsd"));
    if ended_on_its_own(ASKED.load(Ordering::Relaxed), windows_open, linux) && !RECORDED.swap(true, Ordering::Relaxed) {
        super::append(&format!(
            "the app closed without being asked to\nIts event loop ended on its own while windows were open: the connection to the display server (Wayland or X11) was \
             lost, or the loop met an error. What was printed about it is in stderr.log, beside this file.\n{}",
            context::refresh().trim_end()
        ));
    }
    let _ = std::fs::remove_file(state_path());
}

/// A record of this run's end was just written (a panic): the next start has nothing to add.
pub fn recorded() {
    RECORDED.store(true, Ordering::Relaxed);
    let _ = std::fs::remove_file(state_path());
}

/// Whether a signal handler already left this run's record (it removes the file itself).
pub fn note_recorded_by_signal() {
    RECORDED.store(true, Ordering::Relaxed);
}

/// `exit()` reached without the app quitting: a library ended the process (a display server
/// gone, a graphics error it could not go on from). What it said is in `stderr.log`.
extern "C" fn on_exit() {
    if QUIT.load(Ordering::Relaxed) || RECORDED.swap(true, Ordering::Relaxed) {
        return;
    }
    super::append(&format!(
        "the process ended through exit() without Agentty quitting\nSomething inside it ended the process (the display server gone, an error the graphics could not go on \
         from). What it printed last is in stderr.log, beside this file.\n{}",
        context::refresh().trim_end()
    ));
    let _ = std::fs::remove_file(state_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            pid: 4242,
            version: "0.4.8".into(),
            started: 1_700_000_000,
            seen: 1_700_003_600,
            context: "up 1h0m, memory 900 MB\nsession: wayland".into(),
        }
    }

    #[test]
    fn a_quit_nobody_asked_for_is_the_loop_ending_on_linux_only() {
        // The display server went away: the windows are still there, nobody asked.
        assert!(ended_on_its_own(false, true, true));
        // Asked for, or the last window closed: a quit.
        assert!(!ended_on_its_own(true, true, true));
        assert!(!ended_on_its_own(false, false, true));
        // macOS quits the app at log out without asking ours.
        assert!(!ended_on_its_own(false, true, false));
    }

    #[test]
    fn a_state_file_reads_back() {
        let state = state();
        assert_eq!(State::parse(&state.to_text()), Some(state));
        // A file cut short, or something else in its place, is not a run.
        assert_eq!(State::parse("pid=12\nversion=0.4.8\n"), None);
        assert_eq!(State::parse(""), None);
        assert_eq!(State::parse("pid=x\nversion=1\nstarted=1\nseen=1\n"), None);
    }

    #[test]
    fn an_unrecorded_end_says_what_is_known() {
        let killed = unrecorded_end(&state(), Some(1_699_000_000), "wayland: connection lost\n");
        assert!(killed.contains("Agentty 0.4.8, pid 4242"), "{killed}");
        assert!(killed.contains("ended from outside"), "{killed}");
        assert!(killed.contains("memory 900 MB") && killed.contains("session: wayland"), "{killed}");
        assert!(killed.ends_with("the last it printed:\nwayland: connection lost"), "{killed}");
        // The computer came up after the run was last seen: that is what ended it.
        let restarted = unrecorded_end(&state(), Some(1_700_003_700), "");
        assert!(restarted.contains("was restarted") && !restarted.contains("ended from outside"), "{restarted}");
        assert!(!restarted.contains("the last it printed"), "{restarted}");
        // Where the system does not say when it started, nothing is claimed about a restart.
        assert!(unrecorded_end(&state(), None, "").contains("ended from outside"));
    }

    #[test]
    fn the_tail_of_a_file_starts_at_a_line() {
        let path = std::env::temp_dir().join(format!("agentty-tail-test-{}", std::process::id()));
        std::fs::write(&path, "first line\nsecond line\nthird line\n").unwrap();
        assert_eq!(tail(&path, 16), "third line\n");
        assert_eq!(tail(&path, 4096), "first line\nsecond line\nthird line\n");
        assert_eq!(tail(&path.with_extension("missing"), 16), "");
        std::fs::remove_file(path).ok();
    }
}
