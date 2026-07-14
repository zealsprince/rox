//! The app palette per ADR 10: every color the UI draws, one token per
//! role, held as data in a process-global [`Palette`] behind one setter.
//! Panels keep pulling from the plain accessors instead of inlining hex
//! values; the accessors read the current palette, so a swap through
//! [`set`] recolors the whole app. The static sits outside GPUI's
//! reactivity, so [`set`] repaints explicitly - one choke point for every
//! writer the ADR plans (user edits, transparency scalars, art-derived
//! tinting).

use std::sync::{LazyLock, RwLock};

use gpui::{rgb, App, Rgba};
use gpui_component::Theme;

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

/// One listing defines each role three ways: the [`Palette`] field, its
/// default, and the accessor panels call. Adding a role means adding one
/// line here.
macro_rules! tokens {
    ($( $(#[$doc:meta])* $role:ident: $default:literal; )*) => {
        /// The palette as data: one color per role. The default is the
        /// hardcoded look the app has always rendered.
        #[derive(Clone, Copy)]
        pub struct Palette {
            $( $(#[$doc])* pub $role: Rgba, )*
        }

        impl Default for Palette {
            fn default() -> Self {
                Palette { $( $role: rgb($default), )* }
            }
        }

        $(
            $(#[$doc])*
            pub fn $role() -> Rgba {
                CURRENT.read().unwrap().$role
            }
        )*
    };
}

tokens! {
    // The accent family: the one brand color and its hover shift.
    accent: 0xfdcb00;
    /// The accent blended a quarter toward white, the lift hover states use.
    accent_hover: 0xfed840;
    /// The library search box focus ring. Predates the accent settling on
    /// yellow, a candidate to fold into it.
    focus_ring: 0x4a6a55;

    // Backgrounds, deepest to most raised.
    bg_root: 0x121212;
    bg_input: 0x141414;
    bg_panel: 0x181818;
    bg_elevated: 0x1c1c1c;
    bg_toolbar: 0x1f1f1f;
    bg_menubar: 0x242424;
    bg_menu: 0x262626;
    bg_control: 0x2a2a2a;
    bg_menu_hover: 0x2f2f2f;
    bg_control_active: 0x333333;
    bg_control_hover: 0x3a3a3a;

    // Borders.
    border: 0x333333;
    border_light: 0x3a3a3a;

    // Text, brightest to faintest.
    text_bright: 0xe0e0e0;
    text: 0xc0c0c0;
    text_secondary: 0xa0a0a0;
    text_dim: 0x9a9a9a;
    text_muted: 0x808080;
    text_faint: 0x707070;
    /// Dark text over accent-filled controls.
    text_on_accent: 0x121212;

    // Canvas-only strokes.
    gridline: 0x6e6e6e;
}

/// The current palette. A static rather than a GPUI global so the
/// accessors keep their plain signatures and paint closures can read them
/// without a context.
static CURRENT: LazyLock<RwLock<Palette>> = LazyLock::new(|| RwLock::new(Palette::default()));

/// The one setter every palette change goes through: swap the global,
/// re-feed the gpui-component theme tokens the widgets draw, and repaint
/// every open window. Callers hand in a whole palette; layering user
/// edits, scalars, or derivation together happens upstream of here.
pub fn set(palette: Palette, cx: &mut App) {
    *CURRENT.write().unwrap() = palette;
    // The widget baseline stays on gpui-component's stock dark set except
    // where our tokens reach what its widgets actually draw today:
    // selection follows the accent instead of the stock blue.
    let theme = Theme::global_mut(cx);
    theme.table_active = alpha(accent(), 0x26).into();
    theme.table_active_border = accent().into();
    theme.list_active = alpha(accent(), 0x26).into();
    theme.list_active_border = accent().into();
    // The static sits outside GPUI's reactivity, so the repaint is
    // explicit: wake every window, whichever entities they host.
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}
