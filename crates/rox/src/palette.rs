//! The app palette: every color the UI draws, defined once and named by
//! role. Panels pull from here instead of inlining hex values, so the
//! planned token-level theme editing has one place to hook into. The
//! gpui-component widget theme stays on its stock dark set except where
//! main.rs feeds it these tokens, selection follows the accent.

use gpui::{rgb, Rgba};

/// A palette color with its alpha replaced, for washes and gradients.
pub fn alpha(color: Rgba, a: u8) -> Rgba {
    Rgba {
        a: a as f32 / 255.0,
        ..color
    }
}

/// A componentwise blend between two palette colors, `t` = 0 all `a`,
/// `t` = 1 all `b`. For animated transitions between roles.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

// The accent family: the one brand color and its hover shift.

pub fn accent() -> Rgba {
    rgb(0xfdcb00)
}

/// The accent blended a quarter toward white, the lift hover states use.
pub fn accent_hover() -> Rgba {
    rgb(0xfed840)
}

/// The library search box focus ring. Predates the accent settling on
/// yellow, a candidate to fold into it.
pub fn focus_ring() -> Rgba {
    rgb(0x4a6a55)
}

// Backgrounds, deepest to most raised.

pub fn bg_root() -> Rgba {
    rgb(0x121212)
}

pub fn bg_input() -> Rgba {
    rgb(0x141414)
}

pub fn bg_panel() -> Rgba {
    rgb(0x181818)
}

pub fn bg_elevated() -> Rgba {
    rgb(0x1c1c1c)
}

pub fn bg_toolbar() -> Rgba {
    rgb(0x1f1f1f)
}

pub fn bg_menubar() -> Rgba {
    rgb(0x242424)
}

pub fn bg_menu() -> Rgba {
    rgb(0x262626)
}

pub fn bg_control() -> Rgba {
    rgb(0x2a2a2a)
}

pub fn bg_menu_hover() -> Rgba {
    rgb(0x2f2f2f)
}

pub fn bg_control_active() -> Rgba {
    rgb(0x333333)
}

pub fn bg_control_hover() -> Rgba {
    rgb(0x3a3a3a)
}

// Borders.

pub fn border() -> Rgba {
    rgb(0x333333)
}

pub fn border_light() -> Rgba {
    rgb(0x3a3a3a)
}

// Text, brightest to faintest.

pub fn text_bright() -> Rgba {
    rgb(0xe0e0e0)
}

pub fn text() -> Rgba {
    rgb(0xc0c0c0)
}

pub fn text_secondary() -> Rgba {
    rgb(0xa0a0a0)
}

pub fn text_dim() -> Rgba {
    rgb(0x9a9a9a)
}

pub fn text_muted() -> Rgba {
    rgb(0x808080)
}

pub fn text_faint() -> Rgba {
    rgb(0x707070)
}

/// Dark text over accent-filled controls.
pub fn text_on_accent() -> Rgba {
    rgb(0x121212)
}

// Canvas-only strokes.

pub fn gridline() -> Rgba {
    rgb(0x6e6e6e)
}
