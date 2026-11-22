//! Linux and macOS: what is not a Rust panic. A fault (a bad address, an illegal instruction, an
//! abort — in Agentty or in a library loaded into it, such as a graphics driver) and a signal
//! that ends the app from outside (`SIGTERM` from a session ending or a low-memory daemon) are
//! written to `crash.log` with the address, the stack and the process's measurements, and then
//! go on to whatever handled them before (the system's own crash report still happens).
//!
//! Standard error goes to `stderr.log` when the app was started from the desktop, where it would
//! go nowhere a user looks; the run before's is kept as `stderr.prev.log`.
//!
//! The handler does only what a signal handler may: it writes bytes prepared beforehand to a
//! file opened beforehand, with no lock and no allocation.

use super::context;
use std::ffi::{c_int, c_void, CString};
use std::os::fd::IntoRawFd;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicUsize, Ordering};

/// Faults: the record gets the address and the stack.
const FAULTS: &[(c_int, &str)] = &[
    (libc::SIGSEGV, "SIGSEGV (a bad memory address)"),
    (libc::SIGBUS, "SIGBUS (a bad memory access)"),
    (libc::SIGILL, "SIGILL (an illegal instruction)"),
    (libc::SIGFPE, "SIGFPE (an arithmetic fault)"),
    (libc::SIGABRT, "SIGABRT (the process aborted itself)"),
];
/// Asked to end from outside: one line says so.
const ENDED: &[(c_int, &str)] = &[
    (libc::SIGTERM, "SIGTERM (asked to end: the session closing, `kill`, or a low-memory daemon)"),
    (libc::SIGHUP, "SIGHUP (the terminal or session it was started from closed)"),
];

/// `crash.log`, open for appending since start.
static LOG: AtomicI32 = AtomicI32::new(-1);
/// "Agentty 0.4.8 (linux x86_64) pid 123\n", made at start.
static HEADING: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static HEADING_LENGTH: AtomicUsize = AtomicUsize::new(0);
/// The run's state file, removed once this run's end is on record.
static STATE_PATH: AtomicPtr<libc::c_char> = AtomicPtr::new(std::ptr::null_mut());
/// Seconds east of UTC, for the record's time.
static UTC_OFFSET: AtomicI64 = AtomicI64::new(0);
/// A panic wrote its record and is aborting: the abort is not a second crash.
pub static PANICKED: AtomicBool = AtomicBool::new(false);
static WRITING: AtomicBool = AtomicBool::new(false);
/// The pipe [`expect_the_end`] writes a fault's number to.
static SENT_FAULT: AtomicI32 = AtomicI32::new(-1);

const SIGNALS: usize = 32;
/// What handled each signal before this did, to hand it on to.
static mut BEFORE: [Option<libc::sigaction>; SIGNALS] = [None; SIGNALS];

#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
extern "C" {
    fn backtrace(buffer: *mut *mut c_void, size: c_int) -> c_int;
    fn backtrace_symbols_fd(buffer: *const *mut c_void, size: c_int, fd: c_int);
}

pub fn install() {
    keep_stderr();
    let Ok(path) = CString::new(agentty_bridge::fsutil::data_dir().join("crash.log").to_string_lossy().into_owned()) else { return };
    // SAFETY: a NUL-ended path; the descriptor stays open for the life of the process.
    let log = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT | libc::O_CLOEXEC, 0o600 as c_int) };
    if log < 0 {
        return;
    }
    LOG.store(log, Ordering::Relaxed);
    let heading =
        format!("Agentty {} ({} {}) pid {}\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH, std::process::id())
            .into_bytes()
            .into_boxed_slice();
    HEADING_LENGTH.store(heading.len(), Ordering::Relaxed);
    HEADING.store(Box::leak(heading).as_mut_ptr(), Ordering::Relaxed);
    if let Ok(state) = CString::new(super::run::state_path().to_string_lossy().into_owned()) {
        STATE_PATH.store(state.into_raw(), Ordering::Relaxed);
    }
    UTC_OFFSET.store(i64::from(chrono::Local::now().offset().local_minus_utc()), Ordering::Relaxed);
    load_the_unwinder();
    end_after_a_sent_fault();

    for (signal, _) in FAULTS.iter().chain(ENDED) {
        // SAFETY: installs a handler that keeps to what a signal handler may do, and keeps what
        // was there before; `BEFORE` is written here only, before any signal can use it.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as usize;
            // On the alternate stack where the thread has one (Rust gives its threads one): a
            // stack overflow leaves no room on the stack it happened on.
            action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            libc::sigemptyset(&mut action.sa_mask);
            let mut before: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(*signal, &action, &mut before) == 0 {
                let slots = &mut *std::ptr::addr_of_mut!(BEFORE);
                if let Some(slot) = slots.get_mut(*signal as usize) {
                    *slot = Some(before);
                }
            }
        }
    }
}

/// glibc's `backtrace` loads `libgcc_s` the first time it runs. Done first inside the handler,
/// that load ran on the thread's small alternate signal stack, overflowed it, and the process
/// died there: the record stopped at its stack line, and the run's state file was left behind,
/// so the next start reported a run that "ended without a record". One call now loads it, as
/// glibc's documentation asks of a program that calls `backtrace` from a signal handler.
fn load_the_unwinder() {
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
    {
        let mut frames = [std::ptr::null_mut::<c_void>(); 2];
        // SAFETY: `frames` is as long as the count passed.
        unsafe { backtrace(frames.as_mut_ptr(), frames.len() as c_int) };
    }
}

/// How long a fault handed back to the system has to end the process before it is taken as sent.
const A_FAULT_ENDS_WITHIN: std::time::Duration = std::time::Duration::from_millis(300);

/// A fault is handed back to the system's default and the handler returns: the faulting
/// instruction runs again and ends the process. A fault signal that was sent (`kill -SEGV`) has no
/// such instruction, and macOS does not say which it was (its record reads the same for both), so
/// the app ran on after its record said it crashed, with no handler left for a real crash. The
/// handler names the signal on a pipe; a thread that waits on it (asleep until then) sends the
/// signal again if the process is still there a moment later.
fn end_after_a_sent_fault() {
    let mut ends = [0 as c_int; 2];
    // SAFETY: `ends` holds the two descriptors `pipe` fills in; they stay open for the life of
    // the process, are not inherited, and the handler's end never blocks.
    unsafe {
        if libc::pipe(ends.as_mut_ptr()) != 0 {
            return;
        }
        for end in ends {
            libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        libc::fcntl(ends[1], libc::F_SETFL, libc::O_NONBLOCK);
    }
    let [reading, writing] = ends;
    let waiting = std::thread::Builder::new().name("sent-fault".into()).spawn(move || {
        let mut signal = [0u8; 1];
        loop {
            // SAFETY: one byte into a buffer of one.
            let read = unsafe { libc::read(reading, signal.as_mut_ptr().cast(), 1) };
            if read == 1 {
                break;
            }
            if read == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return;
            }
        }
        std::thread::sleep(A_FAULT_ENDS_WITHIN);
        // SAFETY: the default for a fault signal ends the process.
        unsafe {
            libc::signal(c_int::from(signal[0]), libc::SIG_DFL);
            libc::raise(c_int::from(signal[0]));
        }
    });
    if waiting.is_ok() {
        SENT_FAULT.store(writing, Ordering::Relaxed);
    }
}

/// From the handler: `signal` was handed back to the system and should have ended the process.
fn expect_the_end(signal: c_int) {
    let pipe = SENT_FAULT.load(Ordering::Relaxed);
    if pipe >= 0 {
        let byte = [signal as u8];
        // SAFETY: one byte from a buffer of one, to a descriptor that never blocks.
        unsafe { libc::write(pipe, byte.as_ptr().cast(), 1) };
    }
}

/// Started from the desktop, standard error is the session's journal or nothing at all: it goes
/// to `stderr.log` instead. A terminal, a file or a pipe it was started with is left alone.
fn keep_stderr() {
    // SAFETY: plain queries about descriptor 2.
    let unseen = unsafe {
        let mut status: libc::stat = std::mem::zeroed();
        let closed = libc::fstat(2, &mut status) != 0;
        let kind = status.st_mode & libc::S_IFMT;
        closed || (kind == libc::S_IFCHR && libc::isatty(2) == 0) || kind == libc::S_IFSOCK
    };
    if !unseen {
        return;
    }
    let folder = agentty_bridge::fsutil::data_dir();
    let path = folder.join("stderr.log");
    // The run before's: what it printed last is what explains how it ended.
    let _ = std::fs::rename(&path, folder.join("stderr.prev.log"));
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let Ok(file) = options.open(&path) else { return };
    let descriptor = file.into_raw_fd();
    // SAFETY: both are open descriptors of this process.
    unsafe {
        libc::dup2(descriptor, 2);
        libc::close(descriptor);
    }
}

/// Writes all of `bytes`, going on after an interruption. Only `write` is called.
fn put(log: c_int, bytes: &[u8]) {
    let mut rest = bytes;
    while !rest.is_empty() {
        // SAFETY: `rest` is valid for its length.
        let written = unsafe { libc::write(log, rest.as_ptr().cast(), rest.len()) };
        if written <= 0 {
            return;
        }
        rest = &rest[written as usize..];
    }
}

/// `value` in decimal, at least `width` digits, into `out`; the part of `out` used.
fn digits(mut value: u64, width: usize, out: &mut [u8; 20]) -> &[u8] {
    let mut at = out.len();
    loop {
        at -= 1;
        out[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 && out.len() - at >= width {
            break;
        }
        if at == 0 {
            break;
        }
    }
    &out[at..]
}

fn hex(mut value: usize, out: &mut [u8; 18]) -> &[u8] {
    let mut at = out.len();
    loop {
        at -= 1;
        out[at] = b"0123456789abcdef"[value & 15];
        value >>= 4;
        if value == 0 || at == 2 {
            break;
        }
    }
    at -= 2;
    out[at] = b'0';
    out[at + 1] = b'x';
    &out[at..]
}

/// A moment as year, month, day, hour, minute, second, `offset` seconds east of UTC: plain
/// arithmetic, for where the system's own time formatting may not be called.
fn civil(unix: i64, offset: i64) -> (u64, u64, u64, u64, u64, u64) {
    let local = unix + offset;
    let (days, seconds) = (local.div_euclid(86_400), local.rem_euclid(86_400) as u64);
    // Days to a date (Howard Hinnant's `civil_from_days`).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year as u64, month as u64, day as u64, seconds / 3600, seconds / 60 % 60, seconds % 60)
}

/// Now, in seconds since 1970.
fn now() -> i64 {
    // SAFETY: `time` with a null pointer only returns the time.
    unsafe { libc::time(std::ptr::null_mut()) as i64 }
}

fn put_time(log: c_int) {
    let now = now();
    let (year, month, day, hour, minute, second) = civil(now, UTC_OFFSET.load(Ordering::Relaxed));
    let mut buffer = [0u8; 20];
    for (value, width, after) in [(year, 4, b'-'), (month, 2, b'-'), (day, 2, b' '), (hour, 2, b':'), (minute, 2, b':'), (second, 2, b' ')]
    {
        put(log, digits(value, width, &mut buffer));
        put(log, &[after]);
    }
}

fn fault_address(info: *mut libc::siginfo_t) -> usize {
    if info.is_null() {
        return 0;
    }
    // SAFETY: the system hands the handler a valid record for a fault.
    unsafe { (*info).si_addr() as usize }
}

/// Whether a process sent the signal (`kill`), rather than the processor raising it for a fault.
fn sent_by_a_process(info: *mut libc::siginfo_t) -> bool {
    // SAFETY: the system hands the handler a valid record.
    // `SI_USER`, which the libc crate does not name on every system. macOS has the name and does
    // not use it: a sent fault signal reads as a fault there (see `end_after_a_sent_fault`).
    const SENT_BY_KILL: c_int = if cfg!(target_os = "macos") { 0x10001 } else { 0 };
    !info.is_null() && unsafe { (*info).si_code } == SENT_BY_KILL
}

extern "C" fn on_signal(signal: c_int, info: *mut libc::siginfo_t, context: *mut c_void) {
    let fault = FAULTS.iter().find(|(number, _)| *number == signal);
    let name = fault.or_else(|| ENDED.iter().find(|(number, _)| *number == signal)).map_or("a signal", |(_, name)| name);
    let log = LOG.load(Ordering::Relaxed);
    // A panic already wrote its record and aborts; a second fault while writing would loop.
    let skip = (signal == libc::SIGABRT && PANICKED.load(Ordering::Relaxed)) || WRITING.swap(true, Ordering::Relaxed);
    if log >= 0 && !skip {
        put(log, b"=== ");
        put_time(log);
        let heading = HEADING.load(Ordering::Relaxed);
        if !heading.is_null() {
            // SAFETY: a leaked buffer of that length, never changed after start.
            put(log, unsafe { std::slice::from_raw_parts(heading, HEADING_LENGTH.load(Ordering::Relaxed)) });
        }
        put(log, if fault.is_some() { b"crashed: " } else { b"ended by " });
        put(log, name.as_bytes());
        if fault.is_some_and(|(number, _)| *number != libc::SIGABRT) {
            let mut buffer = [0u8; 18];
            put(log, b" at address ");
            put(log, hex(fault_address(info), &mut buffer));
        }
        put(log, b"\n");
        // Nothing can be measured from here: what was measured last is quoted, with its age.
        let measured = context::measured_at();
        if measured > 0 {
            let mut buffer = [0u8; 20];
            put(log, b"measured ");
            put(log, digits((now() - measured).max(0) as u64, 1, &mut buffer));
            put(log, b" s before this: ");
        }
        let mut text = [0u8; context::room()];
        let length = context::copy_into(&mut text);
        put(log, &text[..length]);
        if fault.is_some() {
            #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
            {
                put(log, b"stack (this handler first; module+offset where the name is not known):\n");
                let mut frames = [std::ptr::null_mut::<c_void>(); 64];
                // SAFETY: `frames` is as long as the count passed, and `log` is open.
                unsafe {
                    let count = backtrace(frames.as_mut_ptr(), frames.len() as c_int);
                    backtrace_symbols_fd(frames.as_ptr(), count, log);
                }
            }
        }
        put(log, b"\n");
        let state = STATE_PATH.load(Ordering::Relaxed);
        if !state.is_null() {
            // SAFETY: a leaked NUL-ended path.
            unsafe { libc::unlink(state) };
        }
        super::run::note_recorded_by_signal();
        WRITING.store(false, Ordering::Relaxed);
    }
    // On to what was there before: Rust's own stack-overflow report, another library's handler,
    // or the system's default (which ends the process and makes its crash report).
    // SAFETY: `BEFORE` was filled at start and is only read here.
    let before = unsafe { (*std::ptr::addr_of!(BEFORE)).get(signal as usize).copied().flatten() };
    match before {
        Some(before) if before.sa_sigaction != libc::SIG_DFL && before.sa_sigaction != libc::SIG_IGN => {
            // SAFETY: the handler that was installed for this signal, called the way it asked for.
            unsafe {
                if before.sa_flags & libc::SA_SIGINFO != 0 {
                    let handler: extern "C" fn(c_int, *mut libc::siginfo_t, *mut c_void) = std::mem::transmute(before.sa_sigaction);
                    handler(signal, info, context);
                } else {
                    let handler: extern "C" fn(c_int) = std::mem::transmute(before.sa_sigaction);
                    handler(signal);
                }
                // Rust's handler (and others like it) hands a fault back to the default and returns,
                // for the faulting instruction to run again and end the process. A fault signal sent
                // by `kill` has no instruction to run again: without this the app ran on after its
                // record said it crashed, with no handler left for a real crash.
                if fault.is_some() && sent_by_a_process(info) {
                    libc::signal(signal, libc::SIG_DFL);
                    libc::raise(signal);
                } else if fault.is_some() {
                    expect_the_end(signal);
                }
            }
        }
        Some(before) if before.sa_sigaction == libc::SIG_IGN => {}
        _ => {
            // SAFETY: back to the default; a fault happens again on return and ends the process,
            // anything else is sent again.
            unsafe {
                libc::signal(signal, libc::SIG_DFL);
                if fault.is_none() || signal == libc::SIGABRT || sent_by_a_process(info) {
                    libc::raise(signal);
                } else {
                    expect_the_end(signal);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The process the test below sends a fault signal to: this handler installed, then idle.
    #[test]
    fn a_process_that_waits_for_a_sent_fault() {
        if std::env::var_os("AGENTTY_TEST_SENT_FAULT").is_none() {
            return;
        }
        install();
        println!("waiting");
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    /// `kill -SEGV` ends the app and leaves a record (it ran on after the record on macOS, which
    /// reports a sent fault signal as a fault).
    #[test]
    fn a_sent_fault_signal_ends_the_process() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::process::ExitStatusExt;
        let data = std::env::temp_dir().join(format!("agentty-sent-fault-test-{}", std::process::id()));
        std::fs::create_dir_all(&data).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_log::signals::tests::a_process_that_waits_for_a_sent_fault", "--nocapture", "--test-threads=1"])
            .env("AGENTTY_TEST_SENT_FAULT", "1")
            .env("AGENTTY_DATA_DIR", &data)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        assert!(lines.any(|line| line.is_ok_and(|line| line.contains("waiting"))), "the process never got ready");
        // SAFETY: a signal to a process this test started.
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGSEGV) };
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed() > std::time::Duration::from_secs(10) {
                child.kill().ok();
                panic!("the process ran on after the fault signal");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(status.signal(), Some(libc::SIGSEGV));
        let record = std::fs::read_to_string(data.join("crash.log")).unwrap_or_default();
        assert!(record.contains("crashed: SIGSEGV"), "{record}");
        std::fs::remove_dir_all(data).ok();
    }

    #[test]
    fn numbers_are_written_without_formatting() {
        let mut buffer = [0u8; 20];
        assert_eq!(digits(7, 2, &mut buffer), b"07");
        assert_eq!(digits(2026, 4, &mut buffer), b"2026");
        assert_eq!(digits(0, 1, &mut buffer), b"0");
        assert_eq!(digits(u64::MAX, 1, &mut buffer), b"18446744073709551615");
        let mut buffer = [0u8; 18];
        assert_eq!(hex(0, &mut buffer), b"0x0");
        assert_eq!(hex(0x7ffe_1234, &mut buffer), b"0x7ffe1234");
        assert_eq!(hex(usize::MAX, &mut buffer), format!("{:#x}", usize::MAX).as_bytes());
    }

    #[test]
    fn a_moment_is_a_date_by_arithmetic_alone() {
        assert_eq!(civil(0, 0), (1970, 1, 1, 0, 0, 0));
        // 2026-11-20 12:34:56 UTC, as itself and nine hours east.
        assert_eq!(civil(1_795_178_096, 0), (2026, 11, 20, 12, 34, 56));
        assert_eq!(civil(1_795_178_096, 9 * 3600), (2026, 11, 20, 21, 34, 56));
        // A leap day, and a day a western offset takes back over midnight.
        assert_eq!(civil(1_709_164_800, 0), (2024, 2, 29, 0, 0, 0));
        assert_eq!(civil(1_709_164_800, -5 * 3600), (2024, 2, 28, 19, 0, 0));
        // The same as the date library says, a day at a time over some years.
        for day in (0..20_000i64).step_by(37) {
            let unix = day * 86_400 + 4321;
            let expected = chrono::DateTime::from_timestamp(unix, 0).unwrap();
            use chrono::{Datelike, Timelike};
            let (year, month, date, hour, minute, second) = civil(unix, 0);
            assert_eq!((year as i32, month as u32, date as u32), (expected.year(), expected.month(), expected.day()), "{unix}");
            assert_eq!((hour as u32, minute as u32, second as u32), (expected.hour(), expected.minute(), expected.second()), "{unix}");
        }
    }
}
