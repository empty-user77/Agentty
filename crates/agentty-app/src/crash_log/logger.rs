//! Where the libraries' warnings and errors go. The window toolkit and the graphics layer say why
//! they give up (a display server's protocol error, a lost graphics device) through the `log`
//! crate, and with no logger installed those words went nowhere: the app just closed. They are
//! printed to standard error, which a crash record quotes (see `stderr.log`).

use std::sync::atomic::{AtomicUsize, Ordering};

/// A library stuck repeating itself must not fill the disk: after this many lines in a run, one
/// more says so and the rest are dropped.
const MOST_LINES: usize = 4000;

struct ToStderr {
    lines: AtomicUsize,
}

static LOGGER: ToStderr = ToStderr { lines: AtomicUsize::new(0) };

impl log::Log for ToStderr {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        match self.lines.fetch_add(1, Ordering::Relaxed) {
            line if line < MOST_LINES => {
                eprintln!("{} [{}] {}: {}", chrono::Local::now().format("%H:%M:%S"), record.level(), record.target(), record.args())
            }
            MOST_LINES => eprintln!("agentty: more warnings than {MOST_LINES} in this run; the rest are not printed"),
            _ => {}
        }
    }

    fn flush(&self) {}
}

pub fn install() {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}
