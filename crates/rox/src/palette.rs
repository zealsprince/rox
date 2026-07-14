//! The app palette per ADR 10: every color the UI draws, one token per
//! role, held as data in a process-global [`Palette`] behind one setter.
//! Panels keep pulling from the plain accessors instead of inlining hex
//! values; the accessors read the current palette, so a swap through
//! [`set`] recolors the whole app. ADR 10's transparency pair rides the
//! same pipe: surface opacity applies inside the background accessors at
//! read time, backdrop strength inside [`backdrop_wash`], neither stored
//! per token. While a track plays, [`set_seed`] layers the derived mode
//! on top: every role's hue and chroma move toward a seed color pulled
//! from the cover art while its lightness holds, so the contrast ladder
//! survives any album. Changes ease componentwise from wherever the
//! palette visibly is to the new target. The static sits outside GPUI's
//! reactivity, so the setters repaint explicitly - one choke point for
//! every writer.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

use gpui::{rgb, App, Rgba};
use gpui_component::{Theme, ThemeColor, ThemeMode};

/// How long a palette change takes, the cover fade's pace.
const EASE_SECS: f32 = 0.35;

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

// Oklch, the derivation's working space: perceptual lightness L, chroma
// C, hue h. Hand-rolled from Ottosson's reference so tinting can hold a
// token's lightness exactly, which sRGB or HSL math can't promise.

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// An sRGB color as (L, C, h); alpha does not participate.
#[allow(clippy::excessive_precision)]
pub(crate) fn rgba_to_oklch(color: Rgba) -> (f32, f32, f32) {
    let r = srgb_to_linear(color.r);
    let g = srgb_to_linear(color.g);
    let b = srgb_to_linear(color.b);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
    let b = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
    (lightness, (a * a + b * b).sqrt(), b.atan2(a))
}

/// (L, C, h) to linear sRGB channels, which may leave the gamut.
#[allow(clippy::excessive_precision)]
fn oklch_to_linear(lightness: f32, chroma: f32, hue: f32) -> (f32, f32, f32) {
    let ok_a = chroma * hue.cos();
    let ok_b = chroma * hue.sin();
    let l = lightness + 0.3963377774 * ok_a + 0.2158037573 * ok_b;
    let m = lightness - 0.1055613458 * ok_a - 0.0638541728 * ok_b;
    let s = lightness - 0.0894841775 * ok_a - 1.2914855480 * ok_b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    (
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    )
}

fn in_gamut((r, g, b): (f32, f32, f32)) -> bool {
    const EPS: f32 = 1e-4;
    let fits = |c: f32| (-EPS..=1.0 + EPS).contains(&c);
    fits(r) && fits(g) && fits(b)
}

/// (L, C, h) back to sRGB. A requested color can sit outside the gamut
/// (a light, vivid blue does not exist); clipping channels there would
/// shift lightness, so chroma walks down until the color fits instead -
/// lightness and hue are the promise, chroma is the budget.
pub(crate) fn oklch_to_rgba(lightness: f32, chroma: f32, hue: f32, a: f32) -> Rgba {
    let mut linear = oklch_to_linear(lightness, chroma, hue);
    if !in_gamut(linear) {
        let (mut lo, mut hi) = (0.0, chroma);
        for _ in 0..12 {
            let mid = (lo + hi) / 2.0;
            if in_gamut(oklch_to_linear(lightness, mid, hue)) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        linear = oklch_to_linear(lightness, lo, hue);
    }
    Rgba {
        r: linear_to_srgb(linear.0.clamp(0.0, 1.0)),
        g: linear_to_srgb(linear.1.clamp(0.0, 1.0)),
        b: linear_to_srgb(linear.2.clamp(0.0, 1.0)),
        a,
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

        impl Palette {
            /// Componentwise blend of every role, the easing step.
            fn mixed(from: &Palette, to: &Palette, t: f32) -> Palette {
                Palette {
                    $( $role: mix(from.$role, to.$role, t), )*
                    $( $srole: mix(from.$srole, to.$srole, t), )*
                    $( $trole: mix(from.$trole, to.$trole, t), )*
                    $( $irole: mix(from.$irole, to.$irole, t), )*
                }
            }

            /// Every role through one transform, how derivation re-tints
            /// the whole ladder.
            fn map(&self, f: impl Fn(Rgba) -> Rgba) -> Palette {
                Palette {
                    $( $role: f(self.$role), )*
                    $( $srole: f(self.$srole), )*
                    $( $trole: f(self.$trole), )*
                    $( $irole: f(self.$irole), )*
                }
            }
        }

        $(
            $(#[$doc])*
            pub fn $role() -> Rgba {
                CURRENT.read().unwrap().role(|p| p.$role)
            }
        )*

        $(
            $(#[$sdoc])*
            pub fn $srole() -> Rgba {
                let current = CURRENT.read().unwrap();
                scaled(current.role(|p| p.$srole), current.surface_opacity)
            }
        )*

        $(
            $(#[$tdoc])*
            pub fn $trole() -> Rgba {
                let current = CURRENT.read().unwrap();
                let opacity = current.surface_opacity;
                scaled(current.role(|p| p.$trole), opacity * opacity)
            }
        )*

        $(
            $(#[$idoc])*
            pub fn $irole() -> Rgba {
                let current = CURRENT.read().unwrap();
                mix(
                    current.role(|p| p.$irole),
                    current.role(|p| p.text_bright),
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

/// What the accessors read: the base palette and its writers' inputs,
/// plus the easing run the reads actually sample.
struct Current {
    /// The user palette. [`set`] writes it, editing targets it,
    /// derivation layers on top without touching it.
    base: Palette,
    /// The cover-art seed while a track plays; None reads as the plain
    /// base palette.
    seed: Option<Rgba>,
    /// The easing run: reads sample between these two by elapsed time.
    from: Palette,
    target: Palette,
    eased_at: Instant,
    surface_opacity: f32,
    backdrop_strength: f32,
}

impl Current {
    /// Where the easing run sits, 0 fresh, 1 settled, smoothstepped so
    /// changes ease out instead of stopping dead.
    fn progress(&self) -> f32 {
        let u = (self.eased_at.elapsed().as_secs_f32() / EASE_SECS).min(1.0);
        u * u * (3.0 - 2.0 * u)
    }

    /// One role sampled from the easing run.
    fn role(&self, pick: impl Fn(&Palette) -> Rgba) -> Rgba {
        let u = self.progress();
        if u >= 1.0 {
            pick(&self.target)
        } else {
            mix(pick(&self.from), pick(&self.target), u)
        }
    }

    /// The whole palette as it visibly is right now.
    fn snapshot(&self) -> Palette {
        let u = self.progress();
        if u >= 1.0 {
            self.target
        } else {
            Palette::mixed(&self.from, &self.target, u)
        }
    }

    /// Aim a fresh easing run at the current base and seed. Interrupting
    /// a run starts from wherever it visibly is, the waveform's rule, so
    /// nothing snaps.
    fn retarget(&mut self) {
        self.from = self.snapshot();
        self.target = derive(&self.base, self.seed);
        self.eased_at = Instant::now();
    }
}

/// The current palette and scalars. A static rather than a GPUI global so
/// the accessors keep their plain signatures and paint closures can read
/// them without a context.
static CURRENT: LazyLock<RwLock<Current>> = LazyLock::new(|| {
    let palette = Palette::default();
    RwLock::new(Current {
        base: palette,
        seed: None,
        from: palette,
        target: palette,
        eased_at: Instant::now(),
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
        ..current.role(|p| p.bg_elevated)
    }
}

/// The one setter every palette change goes through: swap the base
/// palette and ease toward it. User edits land here; derivation layers
/// over whatever this holds.
pub fn set(palette: Palette, cx: &mut App) {
    {
        let mut current = CURRENT.write().unwrap();
        current.base = palette;
        current.retarget();
    }
    drive(cx);
}

/// The transparency pair's setter, the same pipe as [`set`] but without
/// easing: the scalars are settings knobs, not palette colors. Runtime
/// values only; persisting them stays with the settings' writers.
pub fn set_scalars(surface_opacity: f32, backdrop_strength: f32, cx: &mut App) {
    {
        let mut current = CURRENT.write().unwrap();
        current.surface_opacity = surface_opacity.clamp(0.0, 1.0);
        current.backdrop_strength = backdrop_strength.clamp(0.0, 1.0);
    }
    apply(cx);
}

/// The derived mode's writer: the playing track's seed color in, None
/// when playback stops or the cover is achromatic. Eases like any other
/// palette change, so a track change washes the tint across instead of
/// snapping it.
pub fn set_seed(seed: Option<Rgba>, cx: &mut App) {
    {
        let mut current = CURRENT.write().unwrap();
        // Consecutive tracks off one album carry identical art; don't
        // restart the ease for a seed that isn't going anywhere.
        if current.seed.map(|c| (c.r, c.g, c.b)) == seed.map(|c| (c.r, c.g, c.b)) {
            return;
        }
        current.seed = seed;
        current.retarget();
    }
    drive(cx);
}

/// How far the near-gray roles' chroma moves toward the seed's, and the
/// ceiling it never crosses: enough for surfaces and text to pick up the
/// album's cast, never enough to become it.
const TINT_STRENGTH: f32 = 0.35;
const TINT_CAP: f32 = 0.045;
/// Chroma above this marks a role as already colorful (the accent
/// family): it keeps its own chroma and swings only its hue to the seed.
const CHROMATIC: f32 = 0.05;

/// The derived palette: the base with every role re-tinted toward the
/// seed, or the base itself while nothing seeds.
fn derive(base: &Palette, seed: Option<Rgba>) -> Palette {
    let Some(seed) = seed else { return *base };
    let (_, seed_chroma, seed_hue) = rgba_to_oklch(seed);
    base.map(|color| {
        let (lightness, chroma, _) = rgba_to_oklch(color);
        let chroma = if chroma > CHROMATIC {
            chroma
        } else {
            (chroma + seed_chroma * TINT_STRENGTH).min(TINT_CAP)
        };
        oklch_to_rgba(lightness, chroma, seed_hue, color.a)
    })
}

/// Repaint generations: each palette change starts a pump that re-feeds
/// the theme and refreshes windows until its run settles; a newer change
/// takes the loop over and the old pump dies on its next tick.
static PUMP: AtomicU64 = AtomicU64::new(0);

/// Land a palette change: paint it once right away, then keep painting
/// while the easing run moves.
fn drive(cx: &mut App) {
    apply(cx);
    let generation = PUMP.fetch_add(1, Ordering::Relaxed) + 1;
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            if PUMP.load(Ordering::Relaxed) != generation {
                return;
            }
            let settled = CURRENT.read().unwrap().progress() >= 1.0;
            if cx.update(apply).is_err() {
                return;
            }
            // The settled check ran before the apply, so the final frame
            // painted the target before the pump exits.
            if settled {
                return;
            }
        }
    })
    .detach();
}

/// The shared tail of every palette change: project the gpui-component
/// theme tokens the widgets draw from the palette as it visibly is,
/// then repaint every open window. Per ADR 10 the widget theme stays a
/// projection of our tokens, never the source; everything not projected
/// here keeps the stock dark set.
fn apply(cx: &mut App) {
    // Start over from the stock dark baseline so repeated feeds project
    // onto pristine values instead of compounding.
    Theme::change(ThemeMode::Dark, None, cx);
    let (palette, opacity) = {
        let current = CURRENT.read().unwrap();
        (current.snapshot(), current.surface_opacity)
    };
    let theme = Theme::global_mut(cx);
    // Selection follows the accent instead of the stock blue.
    theme.table_active = alpha(palette.accent, 0x26).into();
    theme.table_active_border = palette.accent.into();
    theme.list_active = alpha(palette.accent, 0x26).into();
    theme.list_active_border = palette.accent.into();
    // The chrome between the backdrop and the panel content, projected
    // from the palette roles whose ladder values sit nearest the stock
    // dark set, so palette edits and art tinting recolor the dock and
    // table along with everything else. One deref up front: field
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The hand-rolled Oklch math must survive a roundtrip, or every
    /// derived palette drifts.
    #[test]
    fn oklch_roundtrips() {
        for hex in [
            0xfdcb00, 0x121212, 0xe0e0e0, 0x4a6a55, 0x808080, 0xff0000, 0x00ff00, 0x0000ff,
            0xffffff, 0x000000,
        ] {
            let color = rgb(hex);
            let (l, c, h) = rgba_to_oklch(color);
            let back = oklch_to_rgba(l, c, h, 1.0);
            for (a, b) in [(color.r, back.r), (color.g, back.g), (color.b, back.b)] {
                assert!((a - b).abs() < 0.005, "{hex:06x} drifted: {a} vs {b}");
            }
        }
    }

    /// Derivation's core promise: whatever the seed, every role keeps
    /// its lightness, so the contrast ladder survives.
    #[test]
    fn derivation_preserves_lightness() {
        let base = Palette::default();
        for seed in [rgb(0xff2200), rgb(0x2244ff), rgb(0x88ff00), rgb(0xfdcb00)] {
            let derived = derive(&base, Some(seed));
            for (before, after) in [
                (base.bg_root, derived.bg_root),
                (base.bg_menu, derived.bg_menu),
                (base.text, derived.text),
                (base.text_muted, derived.text_muted),
                (base.accent, derived.accent),
                (base.border, derived.border),
            ] {
                let (l_before, ..) = rgba_to_oklch(before);
                let (l_after, ..) = rgba_to_oklch(after);
                assert!(
                    (l_before - l_after).abs() < 0.02,
                    "lightness drifted: {l_before} vs {l_after}"
                );
            }
        }
    }
}
