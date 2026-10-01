//! What a crash leaves behind. Release builds abort on a panic (`panic = "abort"`), and on Windows
//! the app has no console: without this, a crash says nothing anywhere. The panic's message,
//! place and backtrace are appended to `<data dir>/crash.log` (the last few hundred kilobytes are
//! kept), then the default hook runs as before.
//!
//! Windows also records what is not a Rust panic: an exception nothing handled (an access
//! violation, a stack overflow — in Agentty or in a DLL loaded into it, such as a graphics
//! driver or an overlay) goes to the same file with its code and the modules on the stack, and
//! the process's standard error goes to `<data dir>/stderr.log`, where Rust's own last words
//! ("fatal runtime error: …") land when it aborts without a panic.

use std::io::Write;

/// Keeps the file from growing without end: past this, it starts over.
const MAX_BYTES: u64 = 512 * 1024;

/// Installs the hook (the app itself only, not its command-line subcommands).
pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(info);
        default(info);
    }));
    #[cfg(windows)]
    native::install();
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
    let _ = writeln!(file, "{}\n{message}\n{}\n", heading(&format!("panicked at {place}:")), std::backtrace::Backtrace::force_capture(),);
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
        let path = agentty_bridge::fsutil::data_dir().join("stderr.log");
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
            "{}\n{} {:#010x} at {}{detail}\nstack (this handler first):\n{}\n",
            super::heading("crashed:"),
            describe(code),
            code as u32,
            module_of(address),
            stack.join("\n"),
        );
        CONTINUE_SEARCH
    }
}
