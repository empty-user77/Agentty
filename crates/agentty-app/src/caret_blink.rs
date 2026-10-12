//! The blinking caret shared by every field that draws its own (text inputs, the file editor).
//!
//! Like a native macOS field: shown for [`HALF_PERIOD`], hidden for [`HALF_PERIOD`], solid again as
//! soon as the caret moves or the text changes (so the blink restarts after a pause), and no caret
//! or timer at all while the field has no focus (callers count an inactive window as no focus). The
//! timer also stops by itself when the field was not painted since its last tick — hidden, or its
//! element gone while the entity lives — and starts again with the next paint.

use gpui::{Context, Task, WeakEntity};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// How long the caret stays shown, and then hidden.
pub const HALF_PERIOD: Duration = Duration::from_millis(530);

/// Whether the caret is shown `elapsed` after the last caret move.
pub fn visible_after(elapsed: Duration) -> bool {
    (elapsed.as_millis() / HALF_PERIOD.as_millis()).is_multiple_of(2)
}

/// Time left until the caret next changes between shown and hidden.
pub fn until_next_change(elapsed: Duration) -> Duration {
    let half = HALF_PERIOD.as_millis();
    Duration::from_millis((half - elapsed.as_millis() % half) as u64)
}

pub struct CaretBlink {
    /// The last caret move (or the moment the field got focus).
    epoch: Instant,
    /// What the caret last looked like (position and text), to notice a move.
    key: Option<u64>,
    /// Redraws the owner at every change; running only while the field is focused and painted.
    timer: Option<Task<()>>,
    /// Painted since the timer's last tick: a tick finding it unset stops the timer.
    painted: bool,
    /// The timer task ended on its own (nothing painted): the next paint starts a new one.
    ended: bool,
}

impl Default for CaretBlink {
    fn default() -> Self {
        Self { epoch: Instant::now(), key: None, timer: None, painted: false, ended: false }
    }
}

impl CaretBlink {
    /// Call at every paint of the field. `key` is anything that changes when the caret moves or the
    /// text does. Returns whether the caret is to be drawn now; `own` finds this state in the owner.
    pub fn update<T: 'static>(
        &mut self,
        focused: bool,
        key: impl Hash,
        cx: &mut Context<T>,
        own: impl Fn(&mut T) -> &mut CaretBlink + 'static,
    ) -> bool {
        if !focused {
            self.stop();
            return false;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        let key = hasher.finish();
        if self.key != Some(key) {
            self.key = Some(key);
            self.epoch = Instant::now();
        }
        self.painted = true;
        if self.timer.is_none() || self.ended {
            self.ended = false;
            self.timer = Some(cx.spawn(async move |this: WeakEntity<T>, cx| loop {
                let Ok(wait) = this.update(cx, |owner, _| until_next_change(own(owner).epoch.elapsed())) else { break };
                // A little past the change, so the redraw never lands just before it.
                cx.background_executor().timer(wait + Duration::from_millis(2)).await;
                let more = this.update(cx, |owner, cx| {
                    let blink = own(owner);
                    if !std::mem::take(&mut blink.painted) {
                        // Not painted since the last tick: nobody sees this caret now.
                        blink.ended = true;
                        return false;
                    }
                    cx.notify();
                    true
                });
                if !matches!(more, Ok(true)) {
                    break;
                }
            }));
        }
        visible_after(self.epoch.elapsed())
    }

    /// The field lost focus: no caret, no timer.
    pub fn stop(&mut self) {
        self.timer = None;
        self.key = None;
        self.painted = false;
        self.ended = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn solid_right_after_a_move_then_alternates() {
        assert!(visible_after(ms(0)));
        assert!(visible_after(ms(529)));
        assert!(!visible_after(ms(530)));
        assert!(!visible_after(ms(1059)));
        assert!(visible_after(ms(1060)));
        assert!(!visible_after(ms(1590)));
    }

    #[test]
    fn waits_for_the_next_change() {
        assert_eq!(until_next_change(ms(0)), ms(530));
        assert_eq!(until_next_change(ms(100)), ms(430));
        assert_eq!(until_next_change(ms(529)), ms(1));
        assert_eq!(until_next_change(ms(530)), ms(530));
    }
}
