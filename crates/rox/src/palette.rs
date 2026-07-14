//! The app palette per ADR 10: every color the UI draws, one token per
//! role, held as data in a process-global [`Palette`] behind one setter.
//! Panels keep pulling from the plain accessors instead of inlining hex
//! values; the accessors read the current palette, so a swap through
//! [`set`] recolors the whole app. ADR 10's transparency pair rides the
//! same pipe: surface opacity applies inside the background accessors at
//! read time, backdrop strength inside [`backdrop_wash`], neither stored
//! per token. The static sits outside GPUI's reactivity, so the setters
//! repaint explicitly - one choke point for every writer the ADR plans
//! (user edits, the scalars, art-derived tinting).

use std::sync::{LazyLock, RwLock};

use gpui::{rgb, App, Rgba};
use gpui_component::{Theme, ThemeColor, ThemeMode};

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

/// A color with its alpha scaled by a unit scalar, the surface accessors'
/// read-time application of surface opacity.
fn scaled(color: Rgba, opacity: f32) -> Rgba {
    Rgba {
        a: color.a * opacity,
        ..color
    }
}

/// One listing defines each role three ways: the [`Palette`] field, its
/// default, and the accessor panels call. Adding a role means adding one
/// line here. Roles in the `surfaces` block are the backgrounds the
/// backdrop can show through: their accessors read out at surface opacity.
/// Roles in the `tints` block are sub-surface texture riding on a surface
/// that already carries the wash: they read out at the square of surface
/// opacity, thinning to a whisper under translucency instead of stacking
/// a second coat. Roles in the `ink` block are foregrounds drawn over the
/// surfaces: they read out lifted toward `text_bright` as surfaces thin,
/// so contrast survives whatever the backdrop shows through. The rest
/// read plain.
macro_rules! tokens {
    (
        $( $(#[$doc:meta])* $role:ident: $default:literal; )*
        @surfaces {
            $( $(#[$sdoc:meta])* $srole:ident: $sdefault:literal; )*
        }
        @tints {
            $( $(#[$tdoc:meta])* $trole:ident: $tdefault:literal; )*
        }
        @ink {
            $( $(#[$idoc:meta])* $irole:ident: $idefault:literal; )*
        }
    ) => {
        /// The palette as data: one color per role. The default is the
        /// hardcoded look the app has always rendered.
        #[derive(Clone, Copy)]
        pub struct Palette {
            $( $(#[$doc])* pub $role: Rgba, )*
            $( $(#[$sdoc])* pub $srole: Rgba, )*
            $( $(#[$tdoc])* pub $trole: Rgba, )*
            $( $(#[$idoc])* pub $irole: Rgba, )*
        }

        impl Default for Palette {
            fn default() -> Self {
                Palette {
                    $( $role: rgb($default), )*
                    $( $srole: rgb($sdefault), )*
                    $( $trole: rgb($tdefault), )*
                    $( $irole: rgb($idefault), )*
                }
            }
        }

        $(
            $(#[$doc])*
            pub fn $role() -> Rgba {
                CURRENT.read().unwrap().palette.$role
            }
        )*

        $(
            $(#[$sdoc])*
            pub fn $srole() -> Rgba {
                let current = CURRENT.read().unwrap();
                scaled(current.palette.$srole, current.surface_opacity)
            }
        )*

        $(
            $(#[$tdoc])*
            pub fn $trole() -> Rgba {
                let current = CURRENT.read().unwrap();
                let opacity = current.surface_opacity;
                scaled(current.palette.$trole, opacity * opacity)
            }
        )*

        $(
            $(#[$idoc])*
            pub fn $irole() -> Rgba {
                let current = CURRENT.read().unwrap();
                mix(
                    current.palette.$irole,
                    current.palette.text_bright,
                    1.0 - current.surface_opacity,
                )
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

    // Borders.
    border: 0x333333;
    border_light: 0x3a3a3a;

    // The two text roles that stay fixed: the top of the ladder, which is
    // also what the ink roles lift toward, and the dark text over
    // accent-filled controls, which sit on opaque accent, not on a
    // thinning surface.
    text_bright: 0xe0e0e0;
    /// Dark text over accent-filled controls.
    text_on_accent: 0x121212;

    // Backgrounds, deepest to most raised.
    @surfaces {
        bg_root: 0x121212;
        bg_panel: 0x181818;
        bg_elevated: 0x1c1c1c;
        bg_menubar: 0x242424;
        bg_menu: 0x262626;
        bg_control: 0x2a2a2a;
        bg_menu_hover: 0x2f2f2f;
        bg_control_active: 0x333333;
        bg_control_hover: 0x3a3a3a;
    }

    // Layered fills that always ride on one of the surfaces above: the
    // library toolbar strip on the panel, the search box on the toolbar.
    @tints {
        bg_input: 0x141414;
        bg_toolbar: 0x1f1f1f;
    }

    // Text, brightest to faintest, and the canvas strokes with it.
    @ink {
        text: 0xc0c0c0;
        text_secondary: 0xa0a0a0;
        text_dim: 0x9a9a9a;
        text_muted: 0x808080;
        text_faint: 0x707070;
        gridline: 0x6e6e6e;
    }
}

/// What the accessors read: the palette plus the transparency pair, held
/// together so one lock serves a read.
struct Current {
    palette: Palette,
    surface_opacity: f32,
    backdrop_strength: f32,
}

/// The current palette and scalars. A static rather than a GPUI global so
/// the accessors keep their plain signatures and paint closures can read
/// them without a context.
static CURRENT: LazyLock<RwLock<Current>> = LazyLock::new(|| {
    RwLock::new(Current {
        palette: Palette::default(),
        surface_opacity: 1.0,
        backdrop_strength: 1.0,
    })
});

/// The wash the backdrop layer paints over the baked image: the floor
/// color at the inverse of backdrop strength. Strength 1 shows the bake
/// bare, 0 sinks it back into the floor.
pub fn backdrop_wash() -> Rgba {
    let current = CURRENT.read().unwrap();
    Rgba {
        a: 1.0 - current.backdrop_strength,
        ..current.palette.bg_elevated
    }
}

/// The one setter every palette change goes through: swap the global,
/// re-feed the widget theme, repaint every window. Callers hand in a whole
/// palette; layering user edits, scalars, or derivation together happens
/// upstream of here.
pub fn set(palette: Palette, cx: &mut App) {
    CURRENT.write().unwrap().palette = palette;
    apply(cx);
}

/// The transparency pair's setter, the same pipe as [`set`]. Runtime
/// values only; persisting them stays with the settings' writers.
pub fn set_scalars(surface_opacity: f32, backdrop_strength: f32, cx: &mut App) {
    {
        let mut current = CURRENT.write().unwrap();
        current.surface_opacity = surface_opacity.clamp(0.0, 1.0);
        current.backdrop_strength = backdrop_strength.clamp(0.0, 1.0);
    }
    apply(cx);
}

/// The shared tail of every palette change: project the gpui-component
/// theme tokens the widgets draw from the palette, then repaint every
/// open window. Per ADR 10 the widget theme stays a projection of our
/// tokens, never the source; everything not projected here keeps the
/// stock dark set.
fn apply(cx: &mut App) {
    // Start over from the stock dark baseline so repeated feeds project
    // onto pristine values instead of compounding.
    Theme::change(ThemeMode::Dark, None, cx);
    let (palette, opacity) = {
        let current = CURRENT.read().unwrap();
        (current.palette, current.surface_opacity)
    };
    let theme = Theme::global_mut(cx);
    // Selection follows the accent instead of the stock blue.
    theme.table_active = alpha(palette.accent, 0x26).into();
    theme.table_active_border = palette.accent.into();
    theme.list_active = alpha(palette.accent, 0x26).into();
    theme.list_active_border = palette.accent.into();
    // The chrome between the backdrop and the panel content, projected
    // from the palette roles whose ladder values sit nearest the stock
    // dark set, so palette edits and later art tinting recolor the dock
    // and table along with everything else. One deref up front: field
    // borrows through the Theme wrapper would each re-borrow it.
    let colors: &mut ThemeColor = theme;
    // Washes: visible chrome with nothing of ours underneath - the tab
    // strip, the active tab, toolbar buttons, the table's row hover -
    // reading out at surface opacity like our own surface tokens.
    colors.tab_bar = scaled(palette.bg_panel, opacity).into();
    colors.tab_active = scaled(palette.bg_root, opacity).into();
    colors.secondary = scaled(palette.bg_panel, opacity).into();
    colors.table_hover = scaled(palette.bg_menu, opacity).into();
    // Tints: the table's striping and header ride on the panel's own
    // wash, so like the palette's tint roles they thin by the square.
    let stripe = scaled(alpha(palette.bg_panel, 0xcc), opacity * opacity);
    colors.table_even = stripe.into();
    colors.table_head = stripe.into();
    // Structural backstops always sit under a surface that already
    // carries the wash: the stack body under the panel tiles, the tab
    // panel body under panel content, the table body over the panel's
    // own background. Scaling them would stack a second and third fog
    // layer over the backdrop, so translucency drops them out entirely.
    let structural = if opacity < 1.0 { 0.0 } else { 1.0 };
    colors.background = scaled(palette.bg_root, structural).into();
    colors.table = scaled(palette.bg_root, structural).into();
    // The ink rule again, for the chrome's own labels and icons: as
    // surfaces thin, foregrounds lift toward text_bright so tab titles,
    // dock buttons, and the table header keep contrast.
    let lift = 1.0 - opacity;
    for token in [
        &mut colors.tab_foreground,
        &mut colors.muted_foreground,
        &mut colors.secondary_foreground,
        &mut colors.table_head_foreground,
    ] {
        *token = mix((*token).into(), palette.text_bright, lift).into();
    }
    // The static sits outside GPUI's reactivity, so the repaint is
    // explicit: wake every window, whichever entities they host.
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}
