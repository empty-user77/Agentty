//! What a crash leaves behind. Release builds abort on a panic (`panic = "abort"`), and an app
//! started from the desktop has no console: without this, a crash says nothing anywhere. The
//! panic's message, place and backtrace are appended to `<data dir>/crash.log` (the last few
//! hundred kilobytes are kept), then the default hook runs as before.
//!
//! What is not a Rust panic is recorded too. On Windows, an exception nothing handled (an access
//! violation, a stack overflow — in Agentty or in a DLL loaded into it, such as a graphics
//! driver or an overlay), with its code and the modules on the stack. On Linux and macOS, a fatal
//! signal with its address and stack, and a signal that ends the app from outside (`signals`).
//! On all three, a run that ended with no code of its own running — killed, out of memory, the
//! computer going down — is noticed at the next start and recorded then (`run`).
//!
//! Every record says how the process was doing (`context`: how long it ran, its memory, open
//! files and threads, the system, the session and the graphics device). Standard error goes to
//! `<data dir>/stderr.log` when nobody would see it otherwise, the run before's is kept as
//! `stderr.prev.log`, and the libraries' warnings and errors are printed there (`logger`): Rust's
//! own last words ("fatal runtime error: …") and a display server's or graphics layer's reason
//! for giving up land in it.

mod context;
mod logger;
mod run;
#[cfg(unix)]
mod signals;
#[cfg(unix)]
mod stall;

pub use context::note;
use std::io::Write;

/// Keeps the file from growing without end: past this, it starts over.
const MAX_BYTES: u64 = 512 * 1024;

/// Installs the hook (the app itself only, not its command-line subcommands).
pub fn install() {
    context::start();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The abort that follows is this crash, not another one.
        #[cfg(unix)]
        signals::PANICKED.store(true, std::sync::atomic::Ordering::Relaxed);
        write(info);
        run::recorded();
        default(info);
    }));
    #[cfg(windows)]
    native::install();
    // Standard error first: what the run before printed is moved aside here, and the record of
    // how it ended (below) quotes it.
    #[cfg(unix)]
    signals::install();
    logger::install();
    run::start();
}

/// A quit was asked for (⌘Q, the menu, the main window closed, an update's restart): the quit
/// hook that follows is the app ending as it should.
pub fn quit_asked() {
    run::quit_asked();
}

/// The app's quit hook. After [`quit_asked`], nothing is recorded of this run. Without it, on
/// Linux, the event loop ended on its own while windows were open — the connection to the display
/// server was lost, or the loop met an error — and that is recorded: the app closed without
/// anyone closing it.
pub fn quit(windows_open: bool) {
    run::quit(windows_open);
}

/// One more entry in `crash.log`: `what`, under a line saying when and which Agentty.
fn append(what: &str) {
    if let Some(mut file) = open_log() {
        let _ = writeln!(file, "{}\n", heading(what));
    }
}

/// Windows: notices the UI thread not answering (see [`hang`]). The window then shows "Not
/// Responding" and the user ends it, which leaves nothing behind on its own.
pub fn watch_ui(cx: &mut gpui::App) {
    #[cfg(windows)]
    hang::watch(cx);
    #[cfg(unix)]
    stall::watch(cx);
}

/// `agentty hang-dump <pid>`: the separate process that writes a minidump of a hung Agentty
/// (see [`hang`]).
pub fn hang_dump(args: &[String]) -> i32 {
    #[cfg(windows)]
    return hang::dump(args);
    #[cfg(not(windows))]
    {
        let _ = args;
        1
    }
}

/// `crash.log`, opened for one more entry.
fn open_log() -> Option<std::fs::File> {
    let path = agentty_bridge::fsutil::data_dir().join("crash.log");
    let restart = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES);
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true);
    if restart {
        options.truncate(true);
    } else {
        options.append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&path).ok()
}

fn heading(what: &str) -> String {
    let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
    format!(
        "=== {} Agentty {} ({} {}) thread '{thread}'\n{what}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

fn write(info: &std::panic::PanicHookInfo<'_>) {
    let Some(mut file) = open_log() else { return };
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into());
    let place = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
    let _ = writeln!(
        file,
        "{}\n{message}\n{}{}\n",
        heading(&format!("panicked at {place}:")),
        context::refresh(),
        std::backtrace::Backtrace::force_capture(),
    );
}

#[cfg(windows)]
mod native {
    use std::io::Write;
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::Diagnostics::Debug::{RtlCaptureStackBackTrace, SetUnhandledExceptionFilter, EXCEPTION_POINTERS};
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };

    /// Lets the search for a handler go on (Windows Error Reporting still sees the crash).
    const CONTINUE_SEARCH: i32 = 0;

    pub fn install() {
        // SAFETY: plain process-wide settings: a top-level exception filter, room on this
        // thread's stack for it to run after a stack overflow, and where standard error goes.
        unsafe {
            SetUnhandledExceptionFilter(Some(on_crash));
            let mut room: u32 = 64 * 1024;
            windows_sys::Win32::System::Threading::SetThreadStackGuarantee(&mut room);
        }
        redirect_stderr();
    }

    /// A GUI app started from Explorer has no standard error: Rust's own messages before an abort
    /// would go nowhere. They go to `stderr.log` instead (this run only).
    fn redirect_stderr() {
        use std::os::windows::io::IntoRawHandle;
        use windows_sys::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};
        // SAFETY: a plain query.
        let current = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        if !current.is_null() && current != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return; // Started from a terminal, or with its output piped: left as it is.
        }
        let folder = agentty_bridge::fsutil::data_dir();
        let path = folder.join("stderr.log");
        // The run before's: what it printed last is what explains how it ended.
        let _ = std::fs::rename(&path, folder.join("stderr.prev.log"));
        let Ok(file) = std::fs::File::create(path) else { return };
        // The handle stays open for the life of the process.
        let handle = file.into_raw_handle();
        // SAFETY: an open file handle this process owns.
        unsafe { SetStdHandle(STD_ERROR_HANDLE, handle) };
    }

    /// The module `address` is in and the offset into it, e.g. `nvwgf2umx.dll+0x1a2b3c`.
    fn module_of(address: usize) -> String {
        let mut module: HMODULE = std::ptr::null_mut();
        // SAFETY: asks which loaded module holds an address, without changing its reference count.
        let found = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                address as *const u16,
                &mut module,
            )
        };
        if found == 0 || module.is_null() {
            return format!("{address:#x}");
        }
        let mut name = [0u16; 260];
        // SAFETY: `name` is as long as the size passed.
        let len = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
        let path = String::from_utf16_lossy(&name[..len.min(name.len())]);
        let file = path.rsplit('\\').next().unwrap_or(&path);
        format!("{file}+{:#x}", address - module as usize)
    }

    fn describe(code: i32) -> &'static str {
        match code as u32 {
            0xC0000005 => "access violation",
            0xC00000FD => "stack overflow",
            0xC0000409 => "stack buffer overrun / fast fail",
            0xC000001D => "illegal instruction",
            0xC0000094 => "integer division by zero",
            0xE06D7363 => "C++ exception",
            0x80000003 => "breakpoint",
            _ => "exception",
        }
    }

    unsafe extern "system" fn on_crash(info: *const EXCEPTION_POINTERS) -> i32 {
        let Some(mut file) = super::open_log() else { return CONTINUE_SEARCH };
        // SAFETY: Windows hands the filter valid exception pointers.
        let record = unsafe { info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) };
        let (code, address, detail) = match record {
            Some(record) => {
                let detail = if record.ExceptionCode as u32 == 0xC0000005 && record.NumberParameters >= 2 {
                    let kind = match record.ExceptionInformation[0] {
                        0 => "reading",
                        1 => "writing",
                        8 => "executing",
                        _ => "touching",
                    };
                    format!(" ({kind} {:#x})", record.ExceptionInformation[1])
                } else {
                    String::new()
                };
                (record.ExceptionCode, record.ExceptionAddress as usize, detail)
            }
            None => (0, 0, String::new()),
        };
        let mut frames = [std::ptr::null_mut(); 62];
        // SAFETY: `frames` is as long as the count passed.
        let count = unsafe { RtlCaptureStackBackTrace(0, frames.len() as u32, frames.as_mut_ptr(), std::ptr::null_mut()) } as usize;
        let stack: Vec<String> = frames[..count].iter().map(|f| format!("  {}", module_of(*f as usize))).collect();
        let _ = writeln!(
            file,
            "{}\n{} {:#010x} at {}{detail}\n{}stack (this handler first):\n{}\n",
            super::heading("crashed:"),
            describe(code),
            code as u32,
            module_of(address),
            super::context::refresh(),
            stack.join("\n"),
        );
        super::run::recorded();
        CONTINUE_SEARCH
    }
}

/// A UI thread that stops answering. It beats every half second (a timer on the UI thread); a
/// watcher thread that hears nothing for [`hang::AFTER`] notes it in `crash.log` and starts
/// `agentty hang-dump`, a separate process (a dump taken from inside could wait on a lock the stuck
/// thread holds), which writes a minidump of the threads' stacks to `<data dir>/hangs/` for WinDbg.
/// When the thread answers again, how long it was gone is noted too: a slow moment, not a deadlock.
/// The dumps stay on this computer; only the newest few are kept.
#[cfg(windows)]
mod hang {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    /// Silence that counts as a hang (Windows says "Not Responding" after 5 s).
    pub const AFTER: Duration = Duration::from_secs(5);
    /// Dumps kept in `hangs/`.
    const KEEP: usize = 3;

    /// When the UI thread last beat, in milliseconds since [`now_ms`] started counting.
    static BEAT: AtomicU64 = AtomicU64::new(0);

    fn now_ms() -> u64 {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    fn folder() -> std::path::PathBuf {
        agentty_bridge::fsutil::data_dir().join("hangs")
    }

    pub fn watch(cx: &mut gpui::App) {
        BEAT.store(now_ms(), Ordering::Relaxed);
        cx.spawn(async move |cx| loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            BEAT.store(now_ms(), Ordering::Relaxed);
        })
        .detach();
        let _ = std::thread::Builder::new().name("hang-watch".into()).spawn(|| {
            let mut gone_since: Option<u64> = None;
            loop {
                let before = now_ms();
                std::thread::sleep(Duration::from_secs(1));
                let now = now_ms();
                // The computer slept: every thread stopped, the UI thread did not hang.
                if now - before > 3_000 {
                    BEAT.store(now, Ordering::Relaxed);
                    continue;
                }
                let beat = BEAT.load(Ordering::Relaxed);
                match gone_since {
                    None if now.saturating_sub(beat) >= AFTER.as_millis() as u64 => {
                        gone_since = Some(beat);
                        started(now - beat);
                    }
                    Some(since) if beat != since => {
                        gone_since = None;
                        note(&format!("the UI thread answered again after {:.1} s\n", (beat - since) as f64 / 1000.0));
                    }
                    _ => {}
                }
            }
        });
    }

    fn note(what: &str) {
        if let Some(mut file) = super::open_log() {
            let _ = writeln!(file, "{}", super::heading(what));
        }
    }

    fn started(silent_ms: u64) {
        let pid = std::process::id().to_string();
        let helper =
            std::env::current_exe().ok().and_then(|exe| agentty_bridge::process::command(exe).args(["hang-dump", &pid]).spawn().ok());
        let dump =
            if helper.is_some() { format!("a dump goes to {}", folder().display()) } else { "no dump (the helper did not start)".into() };
        note(&format!("not responding: the UI thread has been silent for {:.1} s; {dump}\n", silent_ms as f64 / 1000.0));
    }

    /// `agentty hang-dump <pid>`: only another process of this same executable, into `hangs/`.
    pub fn dump(args: &[String]) -> i32 {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Diagnostics::Debug::{MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
        };
        let Some(pid) = args.get(2).and_then(|pid| pid.parse::<u32>().ok()) else { return 2 };
        // SAFETY: a handle to a process this user may read, closed before returning.
        let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if process.is_null() {
            return 1;
        }
        let mut name = [0u16; 1024];
        let mut len = name.len() as u32;
        // SAFETY: `name` holds `len` units.
        let named = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, name.as_mut_ptr(), &mut len) } != 0;
        let target = std::path::PathBuf::from(String::from_utf16_lossy(&name[..len as usize]));
        let same = |path: &std::path::Path| path.canonicalize().ok();
        let ours = std::env::current_exe().ok().and_then(|exe| same(&exe));
        if !named || ours.is_none() || same(&target) != ours {
            // SAFETY: the handle opened above.
            unsafe { CloseHandle(process) };
            return 3;
        }
        let dir = folder();
        let _ = std::fs::create_dir_all(&dir);
        let mut old: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "dmp"))
            .collect();
        old.sort();
        for stale in old.iter().take((old.len() + 1).saturating_sub(KEEP)) {
            let _ = std::fs::remove_file(stale);
        }
        let path = dir.join(format!("agentty-hang-{}-{pid}.dmp", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        let written = std::fs::File::create(&path).is_ok_and(|file| {
            // SAFETY: an open process handle and file; no extra streams or callbacks.
            unsafe {
                MiniDumpWriteDump(
                    process,
                    pid,
                    file.as_raw_handle(),
                    MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                ) != 0
            }
        });
        // SAFETY: the handle opened above.
        unsafe { CloseHandle(process) };
        if written {
            0
        } else {
            let _ = std::fs::remove_file(&path);
            1
        }
    }
}
