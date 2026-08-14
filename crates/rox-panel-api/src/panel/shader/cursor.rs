//! Cursor presence: how much the pointer still counts for, as the one
//! `meta` float a shader reads to know the hand has left the mouse.
//!
//! A shader that reads `params.mouse` can't tell a cursor parked over the
//! window from one that walked off to another app an hour ago. The uniform
//! carries the last position either way, so a light that follows the
//! pointer stays lit wherever it last saw it, usually pinned to whichever
//! edge the pointer left by. Presence is the missing fact: 1 while the
//! pointer is moving, held at 1 for [`CURSOR_HOLD`] after it stops, then
//! eased to 0 over [`CURSOR_FADE`], and eased back up over [`CURSOR_RISE`]
//! when the hand returns to a faded surface. A shader multiplies its
//! cursor effect by it and the effect swells in and bows out with the
//! hand, never popping at either end.
//!
//! Sampled, not listened to. [`cursor_presence`] compares the window's
//! pointer against where it stood on the last frame that asked, so the
//! whole thing costs one map lookup per shaded surface per frame and needs
//! no event plumbing to work. [`watch_cursor`] adds the two things
//! sampling on its own can't see: a pointer that left the window, which
//! stops producing moves to compare and would otherwise sit out the full
//! hold first, and a pointer that comes back after a surface has faded to
//! nothing and parked its frames.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui::{DispatchPhase, MouseExitEvent, MouseMoveEvent, Pixels, Point, Window, WindowId};

/// How long presence stays at full after the pointer stops moving. Long
/// enough to read as "the hand is still here" through the pauses in normal
/// pointing, short enough that walking away clears the screen.
pub const CURSOR_HOLD: Duration = Duration::from_millis(1500);

/// How long the ease from full to nothing takes once the hold is up.
pub const CURSOR_FADE: Duration = Duration::from_millis(1000);

/// How long the ease back to full takes when the hand returns to a
/// surface that had faded. Short enough to read as the surface answering,
/// long enough that the answer swells instead of popping on.
pub const CURSOR_RISE: Duration = Duration::from_millis(250);

/// How long an untouched window sticks around before the next insert drops
/// it, same shape as the surface registry's own sweep. A window pruned
/// while its shader was parked comes back at full presence, which is why
/// this is minutes rather than seconds.
const WATCH_TTL: Duration = Duration::from_secs(600);

/// One window's pointer, as of the last frame that looked.
struct Watch {
    /// Where the pointer stood, window-local. A different reading next
    /// frame is the only definition of movement here.
    position: Point<Pixels>,
    /// When it last moved, or when it left. The fade measures from this.
    at: Instant,
    /// The pointer left the window. Nothing to compare against any more,
    /// so the fade skips the hold and starts from `at`.
    gone: bool,
    /// When the current rise started and the level it started from: a
    /// move landing mid-fade swells back from wherever the fade stood
    /// rather than snapping to full.
    rose: Instant,
    from: f32,
    /// The last frame that read this window, for the sweep below.
    touched: Instant,
}

static WATCHED: LazyLock<Mutex<HashMap<WindowId, Watch>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Where this window's cursor presence stands, 1 down to 0, and the sample
/// that keeps it moving: call it once per shaded surface per frame.
///
/// A window nothing has sampled yet reads 1. Whoever turned the shader on
/// was holding the mouse a moment ago, and a fresh surface that opened
/// dark reads as broken rather than idle.
pub fn cursor_presence(window: &Window) -> f32 {
    let now = Instant::now();
    let position = window.mouse_position();
    let id = window.window_handle().window_id();
    let mut watched = WATCHED.lock().unwrap();
    let watch = watched.entry(id).or_insert(Watch {
        position,
        at: now,
        gone: false,
        // From full, so the fresh-surface-reads-1 rule above holds: a
        // rise starting at 1 is no rise at all.
        rose: now,
        from: 1.0,
        touched: now,
    });
    watch.touched = now;
    if watch.position != position {
        // A move landing while presence was falling starts a rise from
        // wherever the fall had gotten to. A move during the hold or an
        // unfinished rise changes nothing but the clock, so continuous
        // movement doesn't pin the level to its own start.
        let idle = now.saturating_duration_since(watch.at);
        if watch.gone || idle > CURSOR_HOLD {
            watch.from = level(
                idle,
                watch.gone,
                now.saturating_duration_since(watch.rose),
                watch.from,
            );
            watch.rose = now;
        }
        watch.position = position;
        watch.at = now;
        watch.gone = false;
    }
    level(
        now.saturating_duration_since(watch.at),
        watch.gone,
        now.saturating_duration_since(watch.rose),
        watch.from,
    )
}

/// Presence as the fall capped by the rise: full through the hold and off
/// over the fade, but never above the swell easing back in from wherever
/// the last fade left off. Both ends are smoothsteps, so the hand's
/// answer lands and leaves at zero slope.
fn level(idle: Duration, gone: bool, since_rise: Duration, from: f32) -> f32 {
    let up = (since_rise.as_secs_f32() / CURSOR_RISE.as_secs_f32()).clamp(0.0, 1.0);
    let rise = from + (1.0 - from) * (up * up * (3.0 - 2.0 * up));
    ease(idle, gone).min(rise)
}

/// The curve itself: full through the hold, then off over the fade. A
/// pointer that left the window is already past the hold.
fn ease(idle: Duration, gone: bool) -> f32 {
    let falling = match gone {
        true => idle,
        false => idle.saturating_sub(CURSOR_HOLD),
    };
    let t = (falling.as_secs_f32() / CURSOR_FADE.as_secs_f32()).clamp(0.0, 1.0);
    // Smoothstep rather than a straight ramp: the ease leaves at zero
    // slope, so a light dimming under it settles instead of snapping off
    // the last few percent.
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Keep the view painting this frame following the pointer. Paint phase,
/// once per shaded view, and only worth calling for a source that
/// [`reads_cursor`].
///
/// Two window listeners, both cheap. A move wakes the view, which is what
/// brings a shader that faded to nothing and stopped asking for frames
/// back when the hand comes back; without it the surface waits for
/// whatever else happens to dirty it. A window exit starts the fade
/// straight away instead of after the hold, which is the case that reads
/// worst: the pointer leaves by an edge and the light sticks to it.
///
/// The exit half is Linux and macOS only. Windows tracks the leave but
/// doesn't turn it into an input event, so there the pointer walking off
/// the app is indistinguishable from one holding still, and the hold runs
/// first.
pub fn watch_cursor(window: &mut Window) {
    let view = window.current_view();
    let id = window.window_handle().window_id();
    window.on_mouse_event(move |_: &MouseMoveEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            cx.notify(view);
        }
    });
    window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            left(id);
            cx.notify(view);
        }
    });
}

/// The pointer left this window: hold the position where it was and start
/// the fade now. Repeats are ignored, so a second exit before the first
/// has run out doesn't rewind the fade.
fn left(id: WindowId) {
    let now = Instant::now();
    let mut watched = WATCHED.lock().unwrap();
    let Some(watch) = watched.get_mut(&id) else {
        return;
    };
    if !watch.gone {
        watch.gone = true;
        watch.at = now;
    }
}

/// Whether a shader source cares where the pointer is, so the drivers know
/// whether to keep frames coming for a fade nothing would otherwise see.
///
/// A text scan, like the cover binding's. It reads wide on purpose: the
/// mouse arrives as `params.mouse` and presence as `params.user_meta[1]`,
/// but either can be pulled into a local first, and the cost of a false
/// yes is a couple of seconds of frames after the pointer stops while the
/// cost of a false no is a shader that never fades. The one carve-out is
/// a read going straight to `.w`: that lane is the panel's content shape,
/// nothing about the pointer, and a shader hugging a picture shouldn't
/// pay for frames it never looks at. Grabbing the whole vector still
/// counts as cursor, keeping the wide read for locals.
pub fn reads_cursor(source: &str) -> bool {
    source.contains("mouse")
        || source
            .match_indices("user_meta[1]")
            .any(|(at, key)| !source[at + key.len()..].starts_with(".w"))
}

/// Drop the windows nothing has read in a while. Called from the surface
/// registry's own sweep, so the two maps age together.
pub(super) fn sweep_cursor() {
    let mut watched = WATCHED.lock().unwrap();
    if watched.len() > 32 {
        let now = Instant::now();
        watched.retain(|_, watch| now.saturating_duration_since(watch.touched) < WATCH_TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_holds_then_fades() {
        assert_eq!(ease(Duration::ZERO, false), 1.0);
        assert_eq!(ease(CURSOR_HOLD, false), 1.0);
        let half = ease(CURSOR_HOLD + CURSOR_FADE / 2, false);
        assert!((half - 0.5).abs() < 0.01, "{half}");
        assert_eq!(ease(CURSOR_HOLD + CURSOR_FADE, false), 0.0);
        assert_eq!(ease(CURSOR_HOLD + CURSOR_FADE * 4, false), 0.0);
    }

    #[test]
    fn a_pointer_that_left_skips_the_hold() {
        assert!(ease(CURSOR_FADE / 2, true) < 0.6);
        assert_eq!(ease(CURSOR_FADE, true), 0.0);
    }

    #[test]
    fn a_returning_pointer_eases_back_in() {
        // Faded to nothing, then the hand comes back: the rise climbs
        // from zero to full over CURSOR_RISE instead of popping on.
        assert_eq!(level(Duration::ZERO, false, Duration::ZERO, 0.0), 0.0);
        let mid = level(Duration::ZERO, false, CURSOR_RISE / 2, 0.0);
        assert!((mid - 0.5).abs() < 0.01, "{mid}");
        assert_eq!(level(Duration::ZERO, false, CURSOR_RISE, 0.0), 1.0);
        // A move landing mid-fade swells from where the fade stood.
        let caught = level(Duration::ZERO, false, Duration::ZERO, 0.4);
        assert!((caught - 0.4).abs() < 0.001, "{caught}");
        // The fall still wins once the rise is over.
        assert_eq!(
            level(CURSOR_HOLD + CURSOR_FADE, false, CURSOR_RISE, 0.0),
            0.0
        );
    }

    #[test]
    fn only_a_cursor_reader_pays_for_the_fade() {
        assert!(reads_cursor("let c = params.mouse.xy;"));
        assert!(reads_cursor("let here = params.user_meta[1].z;"));
        assert!(reads_cursor("let m = params.user_meta[1];"));
        assert!(!reads_cursor(
            "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(params.time); }"
        ));
        // The content shape lane: a shader that only wants where the
        // picture ends isn't watching the hand.
        assert!(!reads_cursor("let shape = params.user_meta[1].w;"));
        assert!(reads_cursor(
            "let shape = params.user_meta[1].w;\nlet here = params.user_meta[1].z;"
        ));
    }
}
