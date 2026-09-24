//! The window buttons for surfaces that draw their own chrome: the window
//! controls panel, the menubar, and the fallback titlebar. Two styles, each
//! in its platform's order: flat icons (Windows) and traffic lights (macOS).

use gpui::{App, Div, MouseButton, MouseDownEvent, Stateful, Window, div, prelude::*, px, rgb};
use rox_design::assets::icons;
use rox_design::{palette, tokens};

use crate::Tip;

const TRAFFIC_CLOSE: u32 = 0xff5f57;
const TRAFFIC_MIN: u32 = 0xfebc2e;
const TRAFFIC_ZOOM: u32 = 0x28c840;

/// On macOS this matches the native green button: fullscreen by default,
/// zoom on Option-click. Everywhere else it maximizes.
pub fn maximize(event: &MouseDownEvent, window: &mut Window, _: &mut App) {
    if cfg!(target_os = "macos") && !event.modifiers.alt {
        window.toggle_fullscreen();
    } else {
        window.zoom_window();
    }
}

pub fn maximize_tip(window: &Window) -> &'static str {
    if !cfg!(target_os = "macos") {
        "Maximize"
    } else if window.is_fullscreen() {
        "Exit Fullscreen"
    } else {
        "Fullscreen, or Option-click to zoom"
    }
}

/// Brackets rather than shrink arrows inside fullscreen: the mini toggle,
/// which can sit right beside this button, uses the arrows.
pub fn maximize_icon(window: &Window) -> &'static str {
    if cfg!(target_os = "macos") && window.is_fullscreen() {
        icons::FULLSCREEN_EXIT
    } else {
        icons::STOP
    }
}

/// Handed back as children rather than a row so each caller keeps its own
/// spacing.
pub fn traffic_lights(
    window: &Window,
    close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> [Stateful<Div>; 3] {
    [
        traffic_light(TRAFFIC_CLOSE, rox_i18n::t_static("panel-close"), close),
        traffic_light(
            TRAFFIC_MIN,
            rox_i18n::t_static("window-controls-minimize"),
            |_, w, _| w.minimize_window(),
        ),
        traffic_light(TRAFFIC_ZOOM, maximize_tip(window), maximize),
    ]
}

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

pub fn icon_controls(
    window: &Window,
    close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> [Stateful<Div>; 3] {
    [
        icon_button(
            icons::MINUS,
            rox_i18n::t_static("window-controls-minimize"),
            |_, w, _| w.minimize_window(),
        ),
        icon_button(maximize_icon(window), maximize_tip(window), maximize),
        icon_button(
            icons::WINDOW_CLOSE,
            rox_i18n::t_static("panel-close"),
            close,
        ),
    ]
}

pub fn icon_button(
    icon: &'static str,
    tip: &'static str,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    Tip::from(tip).apply(
        div()
            .size(px(24.))
            .rounded(tokens::RADIUS)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_mouse_down(MouseButton::Left, handler)
            .child(
                gpui::svg()
                    .path(icon)
                    .size(px(14.))
                    .text_color(palette::text_muted()),
            ),
    )
}
