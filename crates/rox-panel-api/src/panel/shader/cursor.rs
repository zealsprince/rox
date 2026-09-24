//! Cursor presence: the `meta` float a shader reads to tell that the hand
//! has left the mouse.
//!
//! `params.mouse` holds the last position forever, so a light following
//! the pointer stays pinned to whichever edge it left by. Presence is 1
//! while the pointer moves, held for [`CURSOR_HOLD`] after it stops, eased
//! to 0 over [`CURSOR_FADE`], and back up over [`CURSOR_RISE`] on return.
//!
//! Sampled, not listened to: [`cursor_presence`] compares the pointer with
//! the last frame's, one map lookup per surface per frame. [`watch_cursor`]
//! covers what sampling can't see: a pointer leaving the window, and one
//! returning to a surface that faded out and parked its frames.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui::{DispatchPhase, MouseExitEvent, MouseMoveEvent, Pixels, Point, Window, WindowId};

/// Long enough to ride out the pauses in normal pointing.
pub const CURSOR_HOLD: Duration = Duration::from_millis(1500);

pub const CURSOR_FADE: Duration = Duration::from_millis(1000);

pub const CURSOR_RISE: Duration = Duration::from_millis(250);

/// Minutes, not seconds: a window pruned while its shader was parked comes
/// back at full presence.
const WATCH_TTL: Duration = Duration::from_secs(600);

struct Watch {
    /// Window-local. A different reading next frame is the only definition
    /// of movement here.
    position: Point<Pixels>,
    /// When it last moved, or when it left. The fade measures from this.
    at: Instant,
    /// The pointer left the window, so the fade skips the hold.
    gone: bool,
    /// When the current rise started and from what level, so a move
    /// mid-fade swells back from where the fade stood.
    rose: Instant,
    from: f32,
    touched: Instant,
}

static WATCHED: LazyLock<Mutex<HashMap<WindowId, Watch>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// This window's presence, 1 down to 0. Call once per shaded surface per
/// frame. An unsampled window reads 1: a surface that opens dark reads as
/// broken.
pub fn cursor_presence(window: &Window) -> f32 {
    let now = Instant::now();
    let position = window.mouse_position();
    let id = window.window_handle().window_id();
    let mut watched = WATCHED.lock().unwrap();
    let watch = watched.entry(id).or_insert(Watch {
        position,
        at: now,
        gone: false,
        rose: now,
        from: 1.0,
        touched: now,
    });
    watch.touched = now;
    if watch.position != position {
        // Only a move during the fall starts a rise. Otherwise continuous
        // movement would keep pinning the level to its own start.
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

/// The fall capped by the rise, both smoothsteps.
fn level(idle: Duration, gone: bool, since_rise: Duration, from: f32) -> f32 {
    let up = (since_rise.as_secs_f32() / CURSOR_RISE.as_secs_f32()).clamp(0.0, 1.0);
    let rise = from + (1.0 - from) * (up * up * (3.0 - 2.0 * up));
    ease(idle, gone).min(rise)
}

fn ease(idle: Duration, gone: bool) -> f32 {
    let falling = match gone {
        true => idle,
        false => idle.saturating_sub(CURSOR_HOLD),
    };
    let t = (falling.as_secs_f32() / CURSOR_FADE.as_secs_f32()).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Keep the view painting this frame following the pointer. Paint phase,
/// once per shaded view whose source [`reads_cursor`].
///
/// A move wakes a view that faded out and stopped requesting frames. An
/// exit starts the fade without waiting out the hold. The exit half is
/// Linux and macOS only: Windows tracks the leave but never emits it as an
/// input event, so there the hold runs first.
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

/// Start the fade now. Repeat exits don't rewind it.
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

/// Whether a shader uses the pointer, so the drivers know to keep frames
/// coming through a fade.
///
/// A text scan that reads wide, since a false yes costs a few seconds of
/// frames and a false no is a shader that never fades. The one carve-out
/// is `user_meta[1].w`, which is the panel's content shape, not the pointer.
pub fn reads_cursor(source: &str) -> bool {
    source.contains("mouse")
        || source
            .match_indices("user_meta[1]")
            .any(|(at, key)| !source[at + key.len()..].starts_with(".w"))
}

/// Called from the surface registry's sweep, so the two maps age together.
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
        assert!(!reads_cursor("let shape = params.user_meta[1].w;"));
        assert!(reads_cursor(
            "let shape = params.user_meta[1].w;\nlet here = params.user_meta[1].z;"
        ));
    }
}
