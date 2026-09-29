//! What a crash leaves behind. Release builds abort on a panic (`panic = "abort"`), and on Windows
//! the app has no console: without this, a crash says nothing anywhere. The panic's message,
//! place and backtrace are appended to `<data dir>/crash.log` (the last few hundred kilobytes are
//! kept), then the default hook runs as before.

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
}

fn write(info: &std::panic::PanicHookInfo<'_>) {
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
    let Ok(mut file) = options.open(&path) else { return };
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into());
    let place = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
    let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
    let _ = writeln!(
        file,
        "=== {} Agentty {} ({} {}) thread '{thread}'\npanicked at {place}:\n{message}\n{}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::backtrace::Backtrace::force_capture(),
    );
}
