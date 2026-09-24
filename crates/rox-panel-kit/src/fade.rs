//! The fade a visual surface runs when the audio stops and starts: the
//! Milkdrop panel and the Milkdrop backdrop share one curve so they can't
//! drift. Time in, opacity out; nothing here touches a window.

use std::time::{Duration, Instant};

/// `from == to` is a settled fade.
#[derive(Clone, Copy)]
pub struct Fade {
    pub from: f32,
    pub to: f32,
    pub since: Instant,
}

impl Default for Fade {
    /// Settled and fully drawn; the first idle tick starts the fade out.
    fn default() -> Self {
        Fade {
            from: 1.0,
            to: 1.0,
            since: Instant::now(),
        }
    }
}

impl Fade {
    pub fn settled(at: f32) -> Fade {
        Fade {
            from: at,
            to: at,
            since: Instant::now(),
        }
    }

    pub fn opacity(&self, duration: Duration) -> f32 {
        opacity(self.since.elapsed(), duration, self.from, self.to)
    }

    /// What keeps a surface asking for frames after the trigger is gone.
    pub fn running(&self, duration: Duration) -> bool {
        self.from != self.to && self.since.elapsed() < duration
    }

    /// Turns around from wherever the current fade got to rather than
    /// snapping back to full.
    pub fn retarget(&mut self, to: f32, duration: Duration) {
        if self.to == to {
            return;
        }
        *self = Fade {
            from: self.opacity(duration),
            to,
            since: Instant::now(),
        };
    }
}

/// A straight ramp pinned at `to` once the time is up. Zero duration is a cut.
pub fn opacity(elapsed: Duration, duration: Duration, from: f32, to: f32) -> f32 {
    if duration.is_zero() {
        return to;
    }
    let progress = (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0);
    from + (to - from) * progress
}

/// The exponent that makes the straight ramp look even, derived rather than
/// picked.
///
/// A shader chain writes into blade's `Bgra8Unorm` swapchain under
/// `ColorSpace::Srgb` with no hardware encode, so a blend moves gamma-encoded
/// code values linearly. Luminance goes as roughly code^2.2 and perceived
/// lightness as luminance^(1/3), so a mix `m` reads as `m^(2.2/3)`: bright
/// for most of the ramp, then a sudden drop. Raising the ramp to `3/2.2`
/// cancels that exactly.
pub const SHAPE: f32 = 3.0 / 2.2;

/// The ramp shaped by [`SHAPE`]. Both ends are fixed points. The derivation
/// assumes a near-black background, which every stock theme has.
pub fn mix(opacity: f32) -> f32 {
    // `max` after the clamp is the NaN guard: clamp passes NaN through, and a
    // NaN in the uniform block renders nothing.
    opacity.clamp(0.0, 1.0).max(0.0).powf(SHAPE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ramp_runs_on_the_clock_and_stops_at_its_target() {
        let second = Duration::from_secs(1);

        assert_eq!(opacity(Duration::ZERO, second, 1.0, 0.0), 1.0);
        assert_eq!(opacity(Duration::from_millis(500), second, 1.0, 0.0), 0.5);
        assert_eq!(opacity(second, second, 1.0, 0.0), 0.0);
        // A surface not rendered for a while comes back settled.
        assert_eq!(opacity(Duration::from_secs(30), second, 1.0, 0.0), 0.0);

        assert_eq!(opacity(Duration::ZERO, second, 0.0, 1.0), 0.0);
        assert_eq!(opacity(Duration::from_millis(250), second, 0.0, 1.0), 0.25);
        assert_eq!(opacity(second, second, 0.0, 1.0), 1.0);

        let turned = opacity(Duration::from_millis(500), second, 0.4, 1.0);
        assert!((turned - 0.7).abs() < 1e-6, "half way from 0.4 to 1.0");

        // The "no fade" setting: a cut, not a division by zero.
        assert_eq!(opacity(Duration::ZERO, Duration::ZERO, 1.0, 0.0), 0.0);
        assert_eq!(opacity(Duration::ZERO, Duration::ZERO, 0.0, 1.0), 1.0);
    }

    #[test]
    fn the_shaping_holds_both_ends_and_bends_the_middle() {
        assert_eq!(mix(0.0), 0.0);
        assert_eq!(mix(1.0), 1.0);

        for step in 1..10 {
            let ramp = step as f32 / 10.0;
            let mixed = mix(ramp);
            assert!(mixed < ramp, "{ramp} shaped to {mixed}");
            assert!(mixed > 0.0);
        }

        let mut last = mix(0.0);
        for step in 1..=100 {
            let mixed = mix(step as f32 / 100.0);
            assert!(mixed > last, "step {step}");
            last = mixed;
        }

        // powf on a negative base with a fractional exponent is NaN.
        assert_eq!(mix(-1.0), 0.0);
        assert_eq!(mix(2.0), 1.0);
        assert!(mix(f32::NAN).is_finite());

        // Half the travel reads as half the brightness.
        for step in 1..10 {
            let ramp = step as f32 / 10.0;
            let perceived = mix(ramp).powf(2.2 / 3.0);
            assert!(
                (perceived - ramp).abs() < 1e-5,
                "{ramp} read as {perceived}"
            );
        }
    }

    #[test]
    fn a_turnaround_starts_where_the_last_fade_got_to() {
        let second = Duration::from_secs(1);
        let mut fade = Fade::settled(1.0);
        fade.retarget(0.0, second);
        assert!(fade.running(second));
        // A repeated retarget must not restart the clock.
        let since = fade.since;
        fade.retarget(0.0, second);
        assert_eq!(fade.since, since);
        fade.retarget(1.0, second);
        assert!(fade.from < 1.0, "turned around at {}", fade.from);
    }
}
