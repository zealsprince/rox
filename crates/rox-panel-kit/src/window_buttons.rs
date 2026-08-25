//! The macOS traffic lights, for the surfaces that draw their own window
//! chrome: the window controls panel and the menubar's left edge. Pure
//! widget factory, so both callers get the same three buttons and only the
//! close handler differs.

use gpui::{div, prelude::*, px, rgb, App, Div, MouseButton, MouseDownEvent, Stateful, Window};
use rox_design::assets::icons;

use crate::Tip;

/// The macOS traffic light colors, close to minimize to zoom.
const TRAFFIC_CLOSE: u32 = 0xff5f57;
const TRAFFIC_MIN: u32 = 0xfebc2e;
const TRAFFIC_ZOOM: u32 = 0x28c840;

/// The maximize control. On macOS it matches the native green button: native
/// fullscreen (its own Space, honoring the user's Mission Control setup) by
/// default, and zoom - fill the screen in place - on Option-click. Everywhere
/// else it just maximizes.
pub fn maximize(event: &MouseDownEvent, window: &mut Window, _: &mut App) {
    if cfg!(target_os = "macos") && !event.modifiers.alt {
        window.toggle_fullscreen();
    } else {
        window.zoom_window();
    }
}

/// What the maximize control does right now, which is two things on macOS and
/// one everywhere else. The modifier is the part nobody guesses, so the tip is
/// where it gets said. From inside a fullscreen Space the same click is the way
/// back out, and the tip says that instead.
pub fn maximize_tip(window: &Window) -> &'static str {
    if !cfg!(target_os = "macos") {
        "Maximize"
    } else if window.is_fullscreen() {
        "Exit Fullscreen"
    } else {
        "Fullscreen, or Option-click to zoom"
    }
}

/// The glyph for the maximize control, for the surfaces that draw icons
/// rather than traffic lights. Corner brackets folding in from inside a
/// fullscreen Space, and the plain square otherwise. Brackets rather than
/// shrink arrows: the mini toggle wears the arrows, and it can sit right
/// beside this button.
pub fn maximize_icon(window: &Window) -> &'static str {
    if cfg!(target_os = "macos") && window.is_fullscreen() {
        icons::FULLSCREEN_EXIT
    } else {
        icons::STOP
    }
}

/// The three traffic lights in macOS order - close, minimize, zoom - over
/// the caller's close handler; minimize and zoom are the window's own, so
/// they're the same wherever these are drawn. Handed back as children
/// rather than a row, so each caller keeps its own spacing: the window
/// controls panel spaces them with the mini toggle beside them, the macOS
/// menubar sits them at its left edge. The window is read for the zoom
/// light's tip, which follows the fullscreen state.
pub fn traffic_lights(
    window: &Window,
    close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> [Stateful<Div>; 3] {
    [
        traffic_light(TRAFFIC_CLOSE, "Close", close),
        traffic_light(TRAFFIC_MIN, "Minimize", |_, w, _| w.minimize_window()),
        traffic_light(TRAFFIC_ZOOM, maximize_tip(window), maximize),
    ]
}

/// One traffic light: a colored circle that runs its click handler. No
/// hover glyphs, the color carries the meaning like macOS without focus,
/// and the tip is there for anyone who reads the color the other way
/// round.
fn traffic_light(
    color: u32,
    tip: &'static str,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    Tip::from(tip).apply(
        div()
            .size(px(12.))
            .rounded_full()
            .bg(rgb(color))
            .cursor_pointer()
            .hover(|d| d.opacity(0.8))
            .on_mouse_down(MouseButton::Left, handler),
    )
}
