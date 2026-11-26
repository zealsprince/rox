//! Frames for motion only time drives (ADR 33). A canvas whose time routes
//! are all that moves doesn't need a frame on every vsync: Aero's drift moves
//! its fastest bubble under thirty pixels a second. After each frame the
//! canvas measures how far every item moved on screen since the one before,
//! and wakes for the next when the fastest will have gone [`STEP`] at that
//! speed. A music route, a hover ease and the pointer's parallax still ask
//! for every frame as they always did; this paces only what's left.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{Context, Task};

use crate::panels::canvas::CanvasPanel;

/// The furthest an item's corner travels between two paced frames, in
/// pixels. A step this small keeps slow drift reading as continuous.
pub(crate) const STEP: f32 = 1.0;

/// The most an item's opacity changes between two paced frames: two of the
/// 255 levels a colour channel has.
pub(crate) const FADE_STEP: f32 = 2. / 255.;

/// The longest a drifting canvas waits, so a route turning slowly at the top
/// of a sine is looked at again before it picks up speed.
const LONGEST: Duration = Duration::from_millis(250);

/// The wait after a first frame, with nothing yet to measure against.
const FIRST: Duration = Duration::from_millis(16);

/// One item as a frame drew it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seen {
    pub corners: [[f32; 2]; 4],
    pub opacity: f32,
}

#[derive(Default)]
pub(crate) struct Pace {
    /// The last frame that drifted: when, and every item in it.
    last: Option<(Instant, HashMap<String, Seen>)>,
    wake: Option<Task<()>>,
}

impl Pace {
    /// After a frame that drew drifting items: wake the canvas once the
    /// fastest of them will have moved a step. A frame asked for sooner, by
    /// anything, replaces the wake.
    pub(crate) fn follow(
        &mut self,
        seen: HashMap<String, Seen>,
        now: Instant,
        cx: &mut Context<CanvasPanel>,
    ) {
        let wait = match self.last.take() {
            Some((at, before)) => wait(&before, &seen, now.saturating_duration_since(at)),
            None => FIRST,
        };
        self.last = Some((now, seen));

        self.wake = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    /// Nothing drifts: no wake, and the next drift measures afresh.
    pub(crate) fn stop(&mut self) {
        self.last = None;
        self.wake = None;
    }
}

/// How long until the fastest item has moved a step, from how far every item
/// moved in `elapsed`. An item new since `before` has no speed yet.
fn wait(
    before: &HashMap<String, Seen>,
    now: &HashMap<String, Seen>,
    elapsed: Duration,
) -> Duration {
    let secs = elapsed.as_secs_f32();
    if secs <= 0. {
        return Duration::ZERO;
    }

    let mut moved = 0f32;
    let mut faded = 0f32;
    for (id, seen) in now {
        let Some(was) = before.get(id) else {
            continue;
        };

        for (a, b) in seen.corners.iter().zip(&was.corners) {
            moved = moved.max((a[0] - b[0]).hypot(a[1] - b[1]));
        }
        faded = faded.max((seen.opacity - was.opacity).abs());
    }

    // Steps a second the motion needs, for the furthest corner and the
    // biggest fade.
    let rate = (moved / STEP).max(faded / FADE_STEP) / secs;
    if !rate.is_finite() {
        return Duration::ZERO;
    }
    if rate <= 0. {
        return LONGEST;
    }

    Duration::from_secs_f32(1. / rate).min(LONGEST)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, opacity: f32) -> HashMap<String, Seen> {
        let corners = [[x, 0.], [x + 10., 0.], [x + 10., 10.], [x, 10.]];
        HashMap::from([("bubble".to_string(), Seen { corners, opacity })])
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn close(a: Duration, b: Duration) -> bool {
        (a.as_secs_f32() - b.as_secs_f32()).abs() < 1e-4
    }

    #[test]
    fn the_wait_is_how_long_a_step_takes_at_the_speed_measured() {
        // Half a pixel in 20 ms is 25 px/s, so a pixel takes 40 ms.
        assert!(close(wait(&at(0., 1.), &at(0.5, 1.), ms(20)), ms(40)));

        // Twice as fast, half the wait.
        assert!(close(wait(&at(0., 1.), &at(1., 1.), ms(20)), ms(20)));
    }

    #[test]
    fn a_fade_paces_by_colour_levels() {
        // One level in 20 ms: two levels take 40 ms.
        let level = 1. / 255.;
        let waited = wait(&at(0., 0.5), &at(0., 0.5 + level), ms(20));
        assert!(close(waited, ms(40)));
    }

    #[test]
    fn nothing_moving_waits_the_longest_and_a_jump_draws_at_once() {
        assert_eq!(wait(&at(0., 1.), &at(0., 1.), ms(20)), LONGEST);
        assert_eq!(wait(&at(0., 1.), &at(0.001, 1.), ms(20)), LONGEST);

        // A saw wrapping from the top back to the bottom.
        assert!(wait(&at(0., 1.), &at(900., 1.), ms(30)) < ms(1));
    }

    #[test]
    fn an_item_new_since_the_last_frame_has_no_speed() {
        let before = HashMap::new();
        assert_eq!(wait(&before, &at(500., 1.), ms(20)), LONGEST);
    }
}
