//! What the process looked like when it ended: how long it had run, how much memory, how many
//! open files and threads it held, and where it ran (system, session, graphics). A crash record
//! carries it, so "it closed with ten sessions open" can be told from "it ran out of files".
//!
//! The text is kept ready in a fixed buffer: a signal handler may not lock or allocate, and reads
//! it as it is.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static STARTED: OnceLock<Instant> = OnceLock::new();
/// Facts noted once or now and then (`session`, `gpu`, …), by name.
static NOTES: Mutex<BTreeMap<&'static str, String>> = Mutex::new(BTreeMap::new());

const ROOM: usize = 1024;
static TEXT: [AtomicU8; ROOM] = [const { AtomicU8::new(0) }; ROOM];
static LENGTH: AtomicUsize = AtomicUsize::new(0);
/// When the ready text was measured (seconds since 1970): a signal handler cannot measure, and
/// says how old what it quotes is.
static MEASURED_AT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Starts the clock and notes where the app runs.
pub fn start() {
    STARTED.get_or_init(Instant::now);
    for (name, value) in system_notes() {
        note(name, value);
    }
}

/// Notes a fact for crash records (`note("gpu", "…")`); a later value replaces an earlier one.
pub fn note(name: &'static str, value: impl Into<String>) {
    let value: String = value.into();
    if let Ok(mut notes) = NOTES.lock() {
        notes.insert(name, value.replace('\n', " "));
    }
    refresh();
}

/// Measures again and puts the text where a crash finds it; returns it.
pub fn refresh() -> String {
    let notes = NOTES.lock().map(|notes| notes.clone()).unwrap_or_default();
    let text = render(&measure(), &notes);
    let bytes = text.as_bytes();
    let length = bytes.len().min(ROOM);
    // Shorter first: a reader in between sees a cut text, never bytes past the new one's end.
    LENGTH.store(0, Ordering::Release);
    for (slot, byte) in TEXT.iter().zip(&bytes[..length]) {
        slot.store(*byte, Ordering::Relaxed);
    }
    LENGTH.store(length, Ordering::Release);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs() as i64);
    MEASURED_AT.store(now, Ordering::Relaxed);
    text
}

/// When the ready text was measured, in seconds since 1970 (0 before the first time).
#[cfg_attr(not(unix), allow(dead_code))]
pub fn measured_at() -> i64 {
    MEASURED_AT.load(Ordering::Relaxed)
}

/// Copies the ready text into `out` and says how much of it there is. Nothing here locks or
/// allocates: this is what a signal handler calls.
pub fn copy_into(out: &mut [u8; ROOM]) -> usize {
    let length = LENGTH.load(Ordering::Acquire).min(ROOM);
    for (byte, slot) in out.iter_mut().zip(&TEXT[..length]) {
        *byte = slot.load(Ordering::Relaxed);
    }
    length
}

pub const fn room() -> usize {
    ROOM
}

/// What can be measured of this process now; `None` where the system does not say.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Measured {
    pub uptime_secs: u64,
    /// Megabytes held now (Linux), or the most ever held (macOS, whose cheap figure is the peak).
    pub memory_mb: Option<u64>,
    pub memory_is_peak: bool,
    pub open_files: Option<usize>,
    pub open_files_limit: Option<u64>,
    pub threads: Option<usize>,
    /// Linux: megabytes the system can still give out, and of swap still free.
    pub system_free_mb: Option<u64>,
    pub swap_free_mb: Option<u64>,
    /// Linux: megabytes held by the control group the app shares with the agents it started (what
    /// a low-memory daemon weighs and ends as one), and the group's limit where it has one.
    pub group_mb: Option<u64>,
    pub group_limit_mb: Option<u64>,
}

/// A `Name:   123 kB` line of `/proc/meminfo` or `/proc/self/status`, in megabytes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn kilobytes_field(text: &str, name: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name))
        .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
        .map(|kb| kb / 1024)
}

/// The app's control group (version 2) from `/proc/self/cgroup`: the path after `0::`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn group_path(cgroup: &str) -> Option<&str> {
    cgroup.lines().find_map(|line| line.strip_prefix("0::")).map(str::trim).filter(|path| path.starts_with('/'))
}

/// A control group's `memory.current` or `memory.max` in megabytes (`max`: no limit).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn group_megabytes(text: &str) -> Option<u64> {
    text.trim().parse::<u64>().ok().map(|bytes| bytes / (1024 * 1024))
}

pub fn measure() -> Measured {
    let mut measured = Measured { uptime_secs: STARTED.get().map_or(0, |started| started.elapsed().as_secs()), ..Measured::default() };
    #[cfg(target_os = "linux")]
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        measured.memory_mb = kilobytes_field(&status, "VmRSS:");
        measured.threads = status.lines().find_map(|line| line.strip_prefix("Threads:")).and_then(|rest| rest.trim().parse::<usize>().ok());
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(memory) = std::fs::read_to_string("/proc/meminfo") {
            measured.system_free_mb = kilobytes_field(&memory, "MemAvailable:");
            measured.swap_free_mb = kilobytes_field(&memory, "SwapFree:");
        }
        if let Some(group) = std::fs::read_to_string("/proc/self/cgroup").ok().as_deref().and_then(group_path) {
            let read = |file: &str| std::fs::read_to_string(format!("/sys/fs/cgroup{group}/{file}")).ok();
            measured.group_mb = read("memory.current").as_deref().and_then(group_megabytes);
            measured.group_limit_mb = read("memory.max").as_deref().and_then(group_megabytes);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: fills a plain struct this function owns.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } == 0 {
            measured.memory_mb = Some(usage.ru_maxrss as u64 / (1024 * 1024));
            measured.memory_is_peak = true;
        }
    }
    #[cfg(unix)]
    {
        let folder = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
        // The listing itself holds one while it is read.
        measured.open_files = std::fs::read_dir(folder).ok().map(|entries| entries.count().saturating_sub(1));
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: fills a plain struct this function owns.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 && limit.rlim_cur != libc::RLIM_INFINITY {
            // The limit's type is not the same on every system.
            #[allow(clippy::unnecessary_cast)]
            let limit = limit.rlim_cur as u64;
            measured.open_files_limit = Some(limit);
        }
    }
    measured
}

fn uptime_text(seconds: u64) -> String {
    match (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60) {
        (0, 0, minutes) => format!("{minutes}m{}s", seconds % 60),
        (0, hours, minutes) => format!("{hours}h{minutes}m"),
        (days, hours, _) => format!("{days}d{hours}h"),
    }
}

/// Two lines: the measurements, then the notes.
pub fn render(measured: &Measured, notes: &BTreeMap<&'static str, String>) -> String {
    let mut parts = vec![format!("up {}", uptime_text(measured.uptime_secs))];
    if let Some(megabytes) = measured.memory_mb {
        parts.push(format!("{} {megabytes} MB", if measured.memory_is_peak { "peak memory" } else { "memory" }));
    }
    match (measured.open_files, measured.open_files_limit) {
        (Some(open), Some(limit)) => parts.push(format!("open files {open} of {limit}")),
        (Some(open), None) => parts.push(format!("open files {open}")),
        _ => {}
    }
    if let Some(threads) = measured.threads {
        parts.push(format!("threads {threads}"));
    }
    match (measured.group_mb, measured.group_limit_mb) {
        (Some(held), Some(limit)) => parts.push(format!("with its agents {held} MB of {limit}")),
        (Some(held), None) => parts.push(format!("with its agents {held} MB")),
        _ => {}
    }
    if let Some(free) = measured.system_free_mb {
        parts.push(format!("system memory free {free} MB"));
    }
    if let Some(swap) = measured.swap_free_mb {
        parts.push(format!("swap free {swap} MB"));
    }
    let mut text = parts.join(", ");
    text.push('\n');
    if !notes.is_empty() {
        text.push_str(&notes.iter().map(|(name, value)| format!("{name}: {value}")).collect::<Vec<_>>().join(", "));
        text.push('\n');
    }
    text
}

/// Where the app runs: the system and its version, and on Linux the kind of session.
fn system_notes() -> Vec<(&'static str, String)> {
    let mut notes = Vec::new();
    #[cfg(target_os = "linux")]
    {
        if let Ok(release) = std::fs::read_to_string("/etc/os-release") {
            if let Some(name) = release.lines().find_map(|line| line.strip_prefix("PRETTY_NAME=")) {
                notes.push(("system", name.trim_matches('"').to_string()));
            }
        }
        for (name, variable) in [("session", "XDG_SESSION_TYPE"), ("desktop", "XDG_CURRENT_DESKTOP")] {
            if let Some(value) = std::env::var(variable).ok().filter(|value| !value.is_empty()) {
                notes.push((name, value));
            }
        }
    }
    #[cfg(unix)]
    {
        // SAFETY: fills a plain struct this function owns; its fields are NUL-ended C strings.
        let mut name: libc::utsname = unsafe { std::mem::zeroed() };
        if unsafe { libc::uname(&mut name) } == 0 {
            let release = unsafe { std::ffi::CStr::from_ptr(name.release.as_ptr()) }.to_string_lossy().into_owned();
            notes.push(("kernel", release));
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_says_what_was_measured_and_noted() {
        let measured = Measured {
            uptime_secs: 3 * 3600 + 12 * 60 + 5,
            memory_mb: Some(812),
            memory_is_peak: false,
            open_files: Some(412),
            open_files_limit: Some(10_240),
            threads: Some(96),
            ..Measured::default()
        };
        let notes = BTreeMap::from([("session", "wayland".to_string()), ("gpu", "Intel Arc".to_string())]);
        assert_eq!(
            render(&measured, &notes),
            "up 3h12m, memory 812 MB, open files 412 of 10240, threads 96\ngpu: Intel Arc, session: wayland\n"
        );
        // What the system does not say is left out, not guessed.
        assert_eq!(render(&Measured { uptime_secs: 42, ..Measured::default() }, &BTreeMap::new()), "up 0m42s\n");
        assert_eq!(uptime_text(2 * 86_400 + 5 * 3600), "2d5h");
    }

    #[test]
    fn linux_says_how_much_memory_is_left_and_what_the_agents_hold() {
        let meminfo =
            "MemTotal:       32768000 kB\nMemFree:          204800 kB\nMemAvailable:    1048576 kB\nSwapFree:              0 kB\n";
        assert_eq!(kilobytes_field(meminfo, "MemAvailable:"), Some(1024));
        assert_eq!(kilobytes_field(meminfo, "SwapFree:"), Some(0));
        assert_eq!(kilobytes_field(meminfo, "Missing:"), None);
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-agentty-1234.scope\n";
        assert_eq!(group_path(cgroup), Some("/user.slice/user-1000.slice/user@1000.service/app.slice/app-agentty-1234.scope"));
        // Version 1 lines, or none: nothing is read from a made-up place.
        assert_eq!(group_path("12:memory:/user.slice\n"), None);
        assert_eq!(group_megabytes("6442450944\n"), Some(6144));
        assert_eq!(group_megabytes("max\n"), None);
        let measured = Measured {
            uptime_secs: 60,
            group_mb: Some(6144),
            group_limit_mb: None,
            system_free_mb: Some(310),
            swap_free_mb: Some(0),
            ..Measured::default()
        };
        assert_eq!(render(&measured, &BTreeMap::new()), "up 1m0s, with its agents 6144 MB, system memory free 310 MB, swap free 0 MB\n");
    }

    #[test]
    fn the_ready_text_is_what_a_crash_reads() {
        start();
        note("test-note", "one\ntwo");
        let text = refresh();
        assert!(text.contains("test-note: one two"), "{text}");
        let mut out = [0u8; ROOM];
        let length = copy_into(&mut out);
        assert_eq!(std::str::from_utf8(&out[..length]).unwrap(), text);
        #[cfg(unix)]
        assert!(measure().open_files.is_some_and(|open| open > 0));
    }
}
