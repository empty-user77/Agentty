//! Linux and macOS: the UI thread not answering. A window that stops answering is what a user
//! force-quits, and what a display server drops the connection of — both end the app with no
//! record of their own. So a stall of five seconds or more is written to `crash.log` when it
//! starts and when it ends, and the longest one is part of every crash record.
//!
//! (Windows has its own watch, which also writes a dump: see `hang` in the parent module.)

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Silence that counts as a stall (GNOME offers "Force Quit" after about this long).
const AFTER: Duration = Duration::from_secs(5);
/// How often the UI thread says it is there, and how often that is looked at.
const BEAT: Duration = Duration::from_secs(2);

/// When the UI thread last beat, in milliseconds since [`now_ms`] started counting.
static LAST: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// What the watcher makes of one look: `beat` is when the UI thread was last heard, `gone_since`
/// the beat a stall in progress started from.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Fine,
    /// Silent for this many milliseconds: a stall starts.
    Started(u64),
    /// Heard again after this many milliseconds of silence.
    Ended(u64),
}

fn look(now: u64, beat: u64, gone_since: Option<u64>) -> Seen {
    match gone_since {
        None if now.saturating_sub(beat) >= AFTER.as_millis() as u64 => Seen::Started(now - beat),
        Some(since) if beat != since => Seen::Ended(beat.saturating_sub(since)),
        _ => Seen::Fine,
    }
}

pub fn watch(cx: &mut gpui::App) {
    LAST.store(now_ms(), Ordering::Relaxed);
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(BEAT).await;
        LAST.store(now_ms(), Ordering::Relaxed);
    })
    .detach();
    let _ = std::thread::Builder::new().name("stall-watch".into()).spawn(|| {
        let mut gone_since: Option<u64> = None;
        let mut longest = 0u64;
        loop {
            let before = now_ms();
            std::thread::sleep(BEAT);
            let now = now_ms();
            // The computer slept: every thread stopped, the UI thread did not stall.
            if now - before > BEAT.as_millis() as u64 * 3 {
                LAST.store(now, Ordering::Relaxed);
                continue;
            }
            let beat = LAST.load(Ordering::Relaxed);
            match look(now, beat, gone_since) {
                Seen::Fine => {}
                Seen::Started(silent) => {
                    gone_since = Some(beat);
                    super::append(&format!("not responding: the UI thread has been silent for {:.1} s", silent as f64 / 1000.0));
                }
                Seen::Ended(silent) => {
                    gone_since = None;
                    longest = longest.max(silent);
                    super::append(&format!("the UI thread answered again after {:.1} s", silent as f64 / 1000.0));
                    super::note("longest stall", format!("{:.1} s", longest as f64 / 1000.0));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stall_starts_after_five_silent_seconds_and_ends_with_the_next_beat() {
        // Heard two seconds ago: nothing.
        assert_eq!(look(10_000, 8_000, None), Seen::Fine);
        // Silent for six: it starts, once.
        assert_eq!(look(14_000, 8_000, None), Seen::Started(6_000));
        assert_eq!(look(16_000, 8_000, Some(8_000)), Seen::Fine);
        // Heard again at 21 s: thirteen seconds of silence are over.
        assert_eq!(look(22_000, 21_000, Some(8_000)), Seen::Ended(13_000));
    }
}
