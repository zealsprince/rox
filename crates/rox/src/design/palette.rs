//! The app palette per ADR 10: every color the UI draws, one token per
//! role, held as data behind the plain accessors. The base palette and
//! the transparency scalars are app-wide; the art tint that rides on top
//! is per playback, keyed by the player entity, so a second window's
//! track tints only its own windows and a popped-out panel shares its
//! parent's run. Panels keep pulling from the plain accessors instead of
//! inlining hex values; the accessors answer from the window tint in
//! scope, so a swap through [`set`] recolors the whole app and a track
//! change recolors one playback's windows. ADR 10's transparency pair
//! rides the same pipe: surface opacity applies inside the background
//! accessors at read time, backdrop strength inside [`backdrop_wash`],
//! neither stored per token. While a track plays, [`set_seed`] layers the
//! derived mode on top of that player's tint: every role's hue and chroma
//! move toward a seed color pulled from the cover art while its lightness
//! holds, so the contrast ladder survives any album. The one
//! gpui-component widget theme is a single global, so it can carry only
//! one tint; it follows the focused window's playback. A bright cover swaps the dark ladder for the
//! designed light one before tinting, and when a second cover color
//! stands apart from the first it takes the highlight role whole. The
//! whole derived mode sits behind the
//! [`set_art_theming`] switch, off by default; the backdrop layers read
//! the same switch. Changes ease componentwise from wherever the
//! palette visibly is to the new target. The static sits outside GPUI's
//! reactivity, so the setters repaint explicitly - one choke point for
//! every writer. On top of all of it, per ADR 13 a panel can carry a
//! [`PanelTheme`]: a sparse override the accessors answer with while
//! the panel renders inside [`scoped`]. An overridden role reads as
//! written, passed by song theming and easing alike, while the rest
//! keep following the app palette.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

use gpui::{rgb, App, EntityId, Rgba};
use gpui_component::{Theme, ThemeColor, ThemeMode};
use serde::{Deserialize, Serialize};

use super::tokens::EASE_SECS;

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

/// One palette role by name: how the palette editor and the settings
/// file reach a field without naming it. `name` keys the settings map
/// and stays stable; `label` is the editor's display name, short because
/// it reads under its `group` header.
pub struct Role {
    pub name: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub get: fn(&Palette) -> Rgba,
    pub set: fn(&mut Palette, Rgba),
}

/// One listing defines each role four ways: the [`Palette`] field, its
/// default with the editor label beside it, the accessor panels call,
/// and its [`ROLES`] entry. Adding a role means adding one line here. Roles in the `surfaces` block are the backgrounds the
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
        $( $(#[$doc:meta])* $role:ident: $default:literal, $label:literal; )*
        @surfaces {
            $( $(#[$sdoc:meta])* $srole:ident: $sdefault:literal, $slabel:literal; )*
        }
        @tints {
            $( $(#[$tdoc:meta])* $trole:ident: $tdefault:literal, $tlabel:literal; )*
        }
        @ink {
            $( $(#[$idoc:meta])* $irole:ident: $idefault:literal, $ilabel:literal; )*
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

        // The accessors are the palette's read API, one per role, so a
        // role read only through its field (a raw wash in the theme
        // projection, an opaque overlay variant) leaves its generated
        // accessor uncalled without meaning the token is dead. Each one
        // asks the active panel scope first: an overridden role reads as
        // written, an overridden opacity replaces the app's in the same
        // scaling and lifting the global values get.
        $(
            $(#[$doc])*
            #[allow(dead_code)]
            pub fn $role() -> Rgba {
                scope_color(stringify!($role))
                    .unwrap_or_else(|| active_role(|p| p.$role))
            }
        )*

        $(
            $(#[$sdoc])*
            #[allow(dead_code)]
            pub fn $srole() -> Rgba {
                let color = scope_color(stringify!($srole))
                    .unwrap_or_else(|| active_role(|p| p.$srole));
                let opacity = scope_opacity().unwrap_or_else(base_surface_opacity);
                scaled(color, opacity)
            }
        )*

        $(
            $(#[$tdoc])*
            #[allow(dead_code)]
            pub fn $trole() -> Rgba {
                let color = scope_color(stringify!($trole))
                    .unwrap_or_else(|| active_role(|p| p.$trole));
                let opacity = scope_opacity().unwrap_or_else(base_surface_opacity);
                scaled(color, opacity * opacity)
            }
        )*

        $(
            $(#[$idoc])*
            #[allow(dead_code)]
            pub fn $irole() -> Rgba {
                let color = scope_color(stringify!($irole))
                    .unwrap_or_else(|| active_role(|p| p.$irole));
                let bright = scope_color("text_bright")
                    .unwrap_or_else(|| active_role(|p| p.text_bright));
                let opacity = scope_opacity().unwrap_or_else(base_surface_opacity);
                mix(color, bright, 1.0 - opacity)
            }
        )*

        /// Every role in listing order.
        pub const ROLES: &[Role] = &[
            $( Role { name: stringify!($role), label: $label, group: "Core", get: |p| p.$role, set: |p, c| p.$role = c }, )*
            $( Role { name: stringify!($srole), label: $slabel, group: "Surfaces", get: |p| p.$srole, set: |p, c| p.$srole = c }, )*
            $( Role { name: stringify!($trole), label: $tlabel, group: "Tints", get: |p| p.$trole, set: |p, c| p.$trole = c }, )*
            $( Role { name: stringify!($irole), label: $ilabel, group: "Ink", get: |p| p.$irole, set: |p, c| p.$irole = c }, )*
        ];
    };
}

tokens! {
    // The accent family: the one brand color and its hover shift.
    accent: 0xfdcb00, "Accent";
    /// The accent blended a quarter toward white, the lift hover states use.
    accent_hover: 0xfed840, "Accent hover";
    /// The contrast mark riding over accent fills: the playheads, the
    /// slider knobs, the spectrum's peak caps. Sits at the bright text by
    /// default; under song theming the cover's runner-up color takes it
    /// when one stands apart from the seed.
    highlight: 0xe0e0e0, "Highlight";

    // Borders.
    border: 0x333333, "Border";
    border_light: 0x3a3a3a, "Border light";

    // The two text roles that stay fixed: the top of the ladder, which is
    // also what the ink roles lift toward, and the dark text over
    // accent-filled controls, which sit on opaque accent, not on a
    // thinning surface.
    text_bright: 0xe0e0e0, "Bright text";
    /// Dark text over accent-filled controls.
    text_on_accent: 0x121212, "Text on accent";

    // Backgrounds, deepest to most raised.
    @surfaces {
        bg_root: 0x121212, "Root";
        bg_panel: 0x181818, "Panel";
        bg_elevated: 0x1c1c1c, "Elevated";
        bg_menubar: 0x242424, "Menubar";
        bg_menu: 0x262626, "Menu";
        bg_control: 0x2a2a2a, "Control";
        bg_menu_hover: 0x2f2f2f, "Menu hover";
        bg_control_active: 0x333333, "Control active";
        bg_control_hover: 0x3a3a3a, "Control hover";
    }

    // Layered fills that always ride on one of the surfaces above: the
    // library toolbar strip on the panel, the search box on the toolbar.
    @tints {
        bg_input: 0x141414, "Input";
        bg_toolbar: 0x1f1f1f, "Toolbar";
    }

    // Text, brightest to faintest, and the canvas strokes with it.
    @ink {
        text: 0xc0c0c0, "Text";
        text_secondary: 0xa0a0a0, "Secondary";
        text_dim: 0x9a9a9a, "Dim";
        text_muted: 0x808080, "Muted";
        text_faint: 0x707070, "Faint";
        gridline: 0x6e6e6e, "Gridline";
    }
}

/// A `#rrggbb` string as a color; anything else is None. The settings
/// map's format, tolerant of a missing `#` from a hand edit.
fn parse_hex(hex: &str) -> Option<Rgba> {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(rgb)
}

/// A color as the settings map's `#rrggbb`; alpha does not participate.
fn to_hex(c: Rgba) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8
    )
}

impl Palette {
    /// The palette as the settings file records it: every role as
    /// `#rrggbb`, in role-name keys. The same shape a shared theme is.
    pub fn to_map(self) -> BTreeMap<String, String> {
        ROLES
            .iter()
            .map(|role| (role.name.to_string(), to_hex((role.get)(&self))))
            .collect()
    }

    /// A palette from the settings map, over the defaults: unknown keys
    /// and unparsable values fall away silently, so the file survives
    /// role changes in both directions.
    pub fn from_map(map: &BTreeMap<String, String>) -> Palette {
        let mut palette = Palette::default();
        for role in ROLES {
            if let Some(color) = map.get(role.name).and_then(|hex| parse_hex(hex)) {
                (role.set)(&mut palette, color);
            }
        }
        palette
    }
}

/// A panel's palette override: only the roles it overrides, in the
/// settings map's role-to-hex shape, plus an optional surface opacity of
/// the panel's own. Rides the panel's config through the layout dump, so
/// it restores and duplicates like any other per-view knob. An overridden
/// role reads as written - song theming and palette easing pass it by -
/// while every other role keeps following the app palette, so a panel
/// that only recolors its accent still tracks edits and tinting
/// everywhere else. The frame knobs ride along: margin insets the panel
/// from its cell, padding opens space inside its own surface, rounding
/// and border shape its edge. They are geometry, not colors, so the
/// themed wrapper applies them directly instead of going through the
/// scope; the border draws in the border role's color, which the color
/// grid already covers.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelTheme {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_opacity: Option<f32>,
    /// Space between the panel and its cell, in px; the backdrop shows
    /// through the gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<f32>,
    /// Space inside the panel's edge, in px, kept in the panel's own
    /// background; the content pulls in, the surface stays whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<f32>,
    /// The panel's corner radius, in px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounding: Option<f32>,
    /// A border around the panel, in px of width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<f32>,
    /// The panel's font family, overriding the app font just here. None
    /// follows the app font. A name that is not installed falls back at
    /// render, so a config moved between machines still shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
}

impl PanelTheme {
    /// Whether the theme overrides nothing at all. Empty themes skip the
    /// scope entirely and serialize away, so an untouched panel's config
    /// stays what it was.
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
            && self.surface_opacity.is_none()
            && self.margin.is_none()
            && self.padding.is_none()
            && self.rounding.is_none()
            && self.border.is_none()
            && self.font.is_none()
    }

    /// A role's override, when one is set and parses.
    pub fn color(&self, role: &str) -> Option<Rgba> {
        self.colors.get(role).and_then(|hex| parse_hex(hex))
    }

    /// Set or clear a role's override.
    pub fn set_color(&mut self, role: &str, color: Option<Rgba>) {
        match color {
            Some(color) => {
                self.colors.insert(role.to_string(), to_hex(color));
            }
            None => {
                self.colors.remove(role);
            }
        }
    }

    /// The theme resolved for the read path: role names checked against
    /// the listing, unknown and unparsable entries dropped, so a
    /// hand-edited config degrades quietly. None while no color or
    /// opacity overrides, so renders skip the scope push - the frame
    /// knobs never need one, the wrapper reads them directly.
    pub fn scope(&self) -> Option<Scope> {
        if self.colors.is_empty() && self.surface_opacity.is_none() {
            return None;
        }
        let colors: Vec<(&'static str, Rgba)> = ROLES
            .iter()
            .filter_map(|role| self.color(role.name).map(|color| (role.name, color)))
            .collect();
        Some(Scope {
            colors: colors.into(),
            surface_opacity: self.surface_opacity.map(|o| o.clamp(0.0, 1.0)),
        })
    }
}

/// A resolved [`PanelTheme`], what the accessors actually consult: cheap
/// to clone, so the themed wrapper can carry it into every render phase.
#[derive(Clone)]
pub struct Scope {
    colors: Arc<[(&'static str, Rgba)]>,
    surface_opacity: Option<f32>,
}

thread_local! {
    /// The active panel scopes, innermost last. A stack rather than a
    /// slot so a themed subtree inside another themed subtree nests
    /// instead of clobbering. Thread-local is enough: rendering runs on
    /// the UI thread, and the paint closures that read the palette run
    /// inside the element phases [`scoped`] wraps.
    static SCOPES: RefCell<Vec<Scope>> = const { RefCell::new(Vec::new()) };
}

/// The innermost scope's override for a role, if any.
fn scope_color(role: &str) -> Option<Rgba> {
    SCOPES.with(|scopes| {
        scopes.borrow().last().and_then(|scope| {
            scope
                .colors
                .iter()
                .find(|(name, _)| *name == role)
                .map(|(_, color)| *color)
        })
    })
}

/// The innermost scope's surface opacity, if it carries one.
fn scope_opacity() -> Option<f32> {
    SCOPES.with(|scopes| {
        scopes
            .borrow()
            .last()
            .and_then(|scope| scope.surface_opacity)
    })
}

/// Run `f` with a panel scope active: every accessor answers with the
/// scope's roles and opacity, falling through to the app palette for the
/// rest. The pop rides a drop guard, so an unwinding `f` can't leave the
/// scope stuck on the stack.
pub fn scoped<R>(scope: &Scope, f: impl FnOnce() -> R) -> R {
    SCOPES.with(|scopes| scopes.borrow_mut().push(scope.clone()));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            SCOPES.with(|scopes| {
                scopes.borrow_mut().pop();
            });
        }
    }
    let _pop = Pop;
    f()
}

/// What a playing track's cover contributes to derivation: its dominant
/// color, the strongest color standing apart from it, and how bright the
/// cover reads overall. The backdrop bake extracts one per track.
#[derive(Clone, Copy)]
pub struct Seed {
    /// The dominant chromatic color; None when the cover is achromatic,
    /// which tints nothing but still picks the ladder by lightness.
    pub primary: Option<Rgba>,
    /// The strongest color far enough from the primary in hue to read as
    /// a second color rather than a shade of the first.
    pub secondary: Option<Rgba>,
    /// Mean perceptual lightness over the whole cover, gray mass and all.
    pub lightness: f32,
}

impl Seed {
    /// Whether two seeds would derive the same palette; alpha never
    /// participates.
    fn same(&self, other: &Seed) -> bool {
        let key = |c: Option<Rgba>| c.map(|c| (c.r, c.g, c.b));
        key(self.primary) == key(other.primary)
            && key(self.secondary) == key(other.secondary)
            && self.lightness == other.lightness
    }
}

/// What the accessors read: the base palette and its writers' inputs,
/// plus the easing run the reads actually sample.
#[derive(Clone, Copy)]
struct Base {
    /// The user palette. [`set`] writes it, editing targets it,
    /// derivation layers on top without touching it.
    base: Palette,
    /// The song-theming switch: whether a seed may derive at all. Off,
    /// each tint's seed is only remembered for a later enable.
    art_theming: bool,
    surface_opacity: f32,
    backdrop_strength: f32,
}

/// One playback's art tint: the easing run between the palette where it
/// visibly sat and where its current seed lands it. Held per player, so a
/// second window's playback tints only its own windows while a popped-out
/// panel shares its parent player's run. The seed rides along so a base
/// or song-theming change can re-derive without the caller replaying it.
#[derive(Clone, Copy)]
pub struct Tint {
    /// The cover-art seed while a track plays; None reads as the plain
    /// base palette.
    seed: Option<Seed>,
    /// The easing run: reads sample between these two by elapsed time.
    from: Palette,
    target: Palette,
    eased_at: Instant,
}

impl Tint {
    /// A settled run sitting on a palette, nothing easing. What a window
    /// reads when its player has never seeded.
    fn settled(palette: Palette) -> Tint {
        Tint {
            seed: None,
            from: palette,
            target: palette,
            eased_at: Instant::now(),
        }
    }

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

    /// Aim a fresh easing run at the base and this tint's seed. Interrupting
    /// a run starts from wherever it visibly is, the waveform's rule, so
    /// nothing snaps.
    fn retarget(&mut self, base: &Base) {
        self.from = self.snapshot();
        let seed = if base.art_theming { self.seed } else { None };
        self.target = derive(&base.base, seed);
        self.eased_at = Instant::now();
    }
}

/// The app-wide palette inputs: the user's base palette, the two
/// transparency scalars, and the song-theming switch. One for the whole
/// app. A static rather than a GPUI global so the accessors keep their
/// plain signatures and paint closures can read them without a context.
static BASE: LazyLock<RwLock<Base>> = LazyLock::new(|| {
    RwLock::new(Base {
        base: Palette::default(),
        art_theming: false,
        surface_opacity: 1.0,
        backdrop_strength: 1.0,
    })
});

/// The art tints, one per player entity. A window resolves its own by
/// pushing [`window_tint`] before it renders; a player with no entry
/// reads the plain base palette.
static TINTS: LazyLock<RwLock<HashMap<EntityId, Tint>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The player whose tint the app-wide widget theme follows. The
/// gpui-component theme is one global, so it can only carry one tint at a
/// time; it tracks the focused window's playback.
static FOCUSED: LazyLock<RwLock<Option<EntityId>>> = LazyLock::new(|| RwLock::new(None));

thread_local! {
    /// The active window tint, innermost last, mirroring [`SCOPES`]. The
    /// role accessors fall back to the top of this stack, and to the base
    /// palette when it is empty.
    static TINT_STACK: RefCell<Vec<Tint>> = const { RefCell::new(Vec::new()) };
}

/// A role off the active window tint, or the base palette when no window
/// tint is in scope: the accessors' fallback once a panel scope misses.
fn active_role(pick: impl Fn(&Palette) -> Rgba) -> Rgba {
    TINT_STACK.with(|stack| match stack.borrow().last() {
        Some(tint) => tint.role(&pick),
        None => pick(&BASE.read().unwrap().base),
    })
}

/// The app surface opacity, the accessors' scale when no panel scope
/// overrides it.
fn base_surface_opacity() -> f32 {
    BASE.read().unwrap().surface_opacity
}

/// The wash the backdrop layer paints over the baked image: the floor
/// color at the inverse of backdrop strength. Strength 1 shows the bake
/// bare, 0 sinks it back into the floor.
pub fn backdrop_wash() -> Rgba {
    Rgba {
        a: 1.0 - BASE.read().unwrap().backdrop_strength,
        ..active_role(|p| p.bg_elevated)
    }
}

/// Every tint re-aimed at the base and its own seed. The choke point for
/// an app-wide change: a base swap or a song-theming toggle moves the
/// target every window's playback eases toward, without the callers
/// replaying their seeds.
fn retarget_all() {
    let base = *BASE.read().unwrap();
    let mut tints = TINTS.write().unwrap();
    for tint in tints.values_mut() {
        tint.retarget(&base);
    }
}

/// The one setter every palette change goes through: swap the base
/// palette and ease every window toward it. User edits land here;
/// derivation layers over whatever this holds.
pub fn set(palette: Palette, cx: &mut App) {
    BASE.write().unwrap().base = palette;
    retarget_all();
    drive(cx);
}

/// The transparency pair's setter, the same pipe as [`set`] but without
/// easing: the scalars are settings knobs, not palette colors. Runtime
/// values only; persisting them stays with the settings' writers.
pub fn set_scalars(surface_opacity: f32, backdrop_strength: f32, cx: &mut App) {
    {
        let mut base = BASE.write().unwrap();
        base.surface_opacity = surface_opacity.clamp(0.0, 1.0);
        base.backdrop_strength = backdrop_strength.clamp(0.0, 1.0);
    }
    apply(cx);
}

/// The derived mode's writer, one playback at a time: the playing track's
/// seed color in, None when playback stops or the cover is achromatic.
/// Keyed by the player so a track change tints only that player's windows.
/// Eases like any other palette change, so a track change washes the tint
/// across instead of snapping it.
pub fn set_seed(player: EntityId, seed: Option<Seed>, cx: &mut App) {
    let base = *BASE.read().unwrap();
    {
        let mut tints = TINTS.write().unwrap();
        let tint = tints
            .entry(player)
            .or_insert_with(|| Tint::settled(base.base));
        // Consecutive tracks off one album carry identical art; don't
        // restart the ease for a seed that isn't going anywhere.
        let unchanged = match (&tint.seed, &seed) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same(b),
            _ => false,
        };
        if unchanged {
            return;
        }
        tint.seed = seed;
        // With song theming off the seed is only remembered, so a later
        // enable picks up the playing track; nothing repaints.
        if !base.art_theming {
            return;
        }
        tint.retarget(&base);
    }
    drive(cx);
}

/// The song-theming switch: whether the playing track's art re-tints the
/// palette and backs the windows. Toggling eases every window like any
/// other palette change, so the tint washes in or out instead of snapping.
pub fn set_art_theming(on: bool, cx: &mut App) {
    {
        let mut base = BASE.write().unwrap();
        if base.art_theming == on {
            return;
        }
        base.art_theming = on;
    }
    retarget_all();
    drive(cx);
}

/// Whether song theming is on, for the backdrop layers that paint outside
/// the palette's own pipe.
pub fn art_theming() -> bool {
    BASE.read().unwrap().art_theming
}

/// The app's surface opacity as it currently stands: what a panel's own
/// override starts from when it forks off.
pub fn app_surface_opacity() -> f32 {
    BASE.read().unwrap().surface_opacity
}

/// The palette as the current derivation lands it: the active window
/// tint's easing target, the focused window's while none is in scope, the
/// base itself when nothing derives. What the locked editor swatches show
/// and what export saves while song theming drives the colors, so a look a
/// track built can leave as a theme.
pub fn resolved() -> Palette {
    if let Some(tint) = TINT_STACK.with(|stack| stack.borrow().last().copied()) {
        return tint.target;
    }
    let tints = TINTS.read().unwrap();
    if let Some(tint) = FOCUSED
        .read()
        .unwrap()
        .and_then(|id| tints.get(&id).copied())
    {
        return tint.target;
    }
    BASE.read().unwrap().base
}

/// The tint a window should render under: its player's easing run, or a
/// settled base run when that player has never seeded. Snapshotted for the
/// frame; the easing reads live off the carried `eased_at`, so a window
/// wraps its body once per render and paint stays smooth from it.
pub fn window_tint(player: EntityId) -> Tint {
    match TINTS.read().unwrap().get(&player) {
        Some(tint) => *tint,
        None => Tint::settled(BASE.read().unwrap().base),
    }
}

/// Push a window tint for the duration of `f`, mirroring [`scoped`]. The
/// accessors' base fallback answers from it while it is active, and a
/// panel scope still layers on top. The pop rides a drop guard so an
/// unwinding `f` can't leave the tint stuck on the stack.
pub fn tinted<R>(tint: Tint, f: impl FnOnce() -> R) -> R {
    TINT_STACK.with(|stack| stack.borrow_mut().push(tint));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            TINT_STACK.with(|stack| {
                stack.borrow_mut().pop();
            });
        }
    }
    let _pop = Pop;
    f()
}

/// Note which window holds focus so the one app-wide widget theme follows
/// its playback's tint. Called from a workspace root's render with the
/// window's active flag; a change reprojects the theme once, deferred out
/// of the render pass.
pub fn note_focus(player: EntityId, active: bool, cx: &mut App) {
    if !active {
        return;
    }
    let changed = {
        let mut focused = FOCUSED.write().unwrap();
        if *focused == Some(player) {
            false
        } else {
            *focused = Some(player);
            true
        }
    };
    if changed {
        cx.defer(apply);
    }
}

/// Drop a player's art tint when its last window closes, so a closed
/// window's seed stops feeding the focused-theme projection.
pub fn forget(player: EntityId, cx: &mut App) {
    let removed = TINTS.write().unwrap().remove(&player).is_some();
    let unfocused = {
        let mut focused = FOCUSED.write().unwrap();
        if *focused == Some(player) {
            *focused = None;
            true
        } else {
            false
        }
    };
    if removed || unfocused {
        apply(cx);
    }
}

// The menu overlays read their fill and row hover opaque. A floating
// dropdown has no backdrop behind it - it hovers over panel content -
// so it stays filled while the menubar chrome it drops from thins with
// surface opacity. Same eased colors as the scaled accessors, the
// surface-opacity scale left off, matching the projected popover tokens
// the gpui-component context menus already read.

pub fn bg_menu_opaque() -> Rgba {
    active_role(|p| p.bg_menu)
}

/// The root surface read opaque, the surface-opacity scale left off. For a
/// panel that lays text over an image of its own (the biography panel's
/// dimmed artist background): the floor has to hide the window backdrop
/// bleeding up from behind, or two images fight under the words.
pub fn bg_root_opaque() -> Rgba {
    active_role(|p| p.bg_root)
}

pub fn bg_control_hover_opaque() -> Rgba {
    active_role(|p| p.bg_control_hover)
}

/// How far the near-gray roles' chroma moves toward the seed's, and the
/// ceiling it never crosses: enough for surfaces and text to pick up the
/// album's cast, never enough to become it.
const TINT_STRENGTH: f32 = 0.35;
const TINT_CAP: f32 = 0.045;
/// Chroma above this marks a role as already colorful (the accent
/// family): it keeps its own chroma and swings only its hue to the seed.
const CHROMATIC: f32 = 0.05;
/// How much of the seed's chroma the border roles carry, past the
/// near-gray cap: hairlines sit on surfaces the backdrop saturates well
/// beyond what the capped tint can answer, so a border must be a darker
/// shade of the field, not a gray line over it. Scales with the seed,
/// so a muted album still gets quiet borders.
const BORDER_TINT: f32 = 0.6;
/// Mean cover lightness above this reads as a light album: derivation
/// tints the light ladder instead of the base.
const LIGHT_COVER: f32 = 0.70;
/// The lightness band the highlight clamps into when a runner-up cover
/// color takes it, one band per ladder: far enough from the surfaces to
/// read as a mark, wide enough that the color keeps the chroma to stay
/// itself - pinning it to the ladder's own mark lightness crushed a
/// vivid red to maroon on a light album.
const HIGHLIGHT_DARK_BAND: (f32, f32) = (0.60, 0.85);
const HIGHLIGHT_LIGHT_BAND: (f32, f32) = (0.30, 0.55);

impl Palette {
    /// The light ladder, the dark defaults' designed counterpart:
    /// surfaces mirrored bright, ink mirrored dark, the accent pulled
    /// down to read over bright surfaces. Only derivation reads it - a
    /// bright cover swaps it in for the base before tinting, so the app
    /// goes light with the album instead of sitting dark under it.
    fn light() -> Palette {
        Palette {
            accent: rgb(0xb07d00),
            accent_hover: rgb(0x976a00),
            highlight: rgb(0x1f1f1f),
            // Deeper below the surfaces than a plain mirror of the dark
            // deltas: the backdrop bleeding through translucent surfaces
            // eats hairline contrast, so the light ladder buys more.
            border: rgb(0xb3b3b3),
            border_light: rgb(0xa9a9a9),
            text_bright: rgb(0x1a1a1a),
            text_on_accent: rgb(0xfafafa),
            bg_root: rgb(0xededed),
            bg_panel: rgb(0xe7e7e7),
            bg_elevated: rgb(0xe3e3e3),
            bg_menubar: rgb(0xdbdbdb),
            bg_menu: rgb(0xd9d9d9),
            bg_control: rgb(0xd5d5d5),
            bg_menu_hover: rgb(0xd0d0d0),
            bg_control_active: rgb(0xcccccc),
            bg_control_hover: rgb(0xc5c5c5),
            bg_input: rgb(0xebebeb),
            bg_toolbar: rgb(0xe0e0e0),
            text: rgb(0x3f3f3f),
            text_secondary: rgb(0x5f5f5f),
            text_dim: rgb(0x656565),
            text_muted: rgb(0x7f7f7f),
            text_faint: rgb(0x8f8f8f),
            gridline: rgb(0x919191),
        }
    }
}

/// The derived palette: the ladder the cover's lightness picks, every
/// role re-tinted toward the seed, or the base itself while nothing
/// seeds. An achromatic cover picks the ladder by lightness, then strips
/// the colorful roles to neutral so a black-and-white album gets a
/// black-and-white app.
fn derive(base: &Palette, seed: Option<Seed>) -> Palette {
    let Some(seed) = seed else { return *base };
    let light = seed.lightness > LIGHT_COVER;
    let ladder = if light { Palette::light() } else { *base };
    let Some(primary) = seed.primary else {
        // No hue to derive toward. Leaving the ladder as-is would keep
        // the brand accent as a lone spot of color against a gray cover,
        // so drop the colorful roles' chroma to zero at their own
        // lightness; the near-gray roles are already neutral and stay.
        return ladder.map(|color| {
            let (lightness, chroma, hue) = rgba_to_oklch(color);
            if chroma > CHROMATIC {
                oklch_to_rgba(lightness, 0.0, hue, color.a)
            } else {
                color
            }
        });
    };
    let (_, seed_chroma, seed_hue) = rgba_to_oklch(primary);
    let mut derived = ladder.map(|color| {
        let (lightness, chroma, _) = rgba_to_oklch(color);
        let chroma = if chroma > CHROMATIC {
            chroma
        } else {
            (chroma + seed_chroma * TINT_STRENGTH).min(TINT_CAP)
        };
        oklch_to_rgba(lightness, chroma, seed_hue, color.a)
    });
    // Borders re-tint past the gray cap, [`BORDER_TINT`]'s rule.
    for (derived_border, ladder_border) in [
        (&mut derived.border, ladder.border),
        (&mut derived.border_light, ladder.border_light),
    ] {
        let (lightness, ..) = rgba_to_oklch(ladder_border);
        *derived_border = oklch_to_rgba(
            lightness,
            seed_chroma * BORDER_TINT,
            seed_hue,
            ladder_border.a,
        );
    }
    // The runner-up color takes the highlight role as itself - its own
    // chroma, hue, and lightness, the last clamped into the mark band
    // opposite the ladder's surfaces so it still reads over them.
    if let Some(secondary) = seed.secondary {
        let (lightness, chroma, hue) = rgba_to_oklch(secondary);
        let (lo, hi) = if light {
            HIGHLIGHT_LIGHT_BAND
        } else {
            HIGHLIGHT_DARK_BAND
        };
        derived.highlight = oklch_to_rgba(lightness.clamp(lo, hi), chroma, hue, ladder.highlight.a);
    }
    derived
}

/// Repaint generations: each palette change starts a pump that re-feeds
/// the theme and refreshes windows until its run settles; a newer change
/// takes the loop over and the old pump dies on its next tick.
static PUMP: AtomicU64 = AtomicU64::new(0);

/// Whether any window's tint is still mid-ease, the signal the pump keeps
/// painting on. A base or theming change retargets every tint at once, so
/// one run can carry several windows.
fn any_tint_easing() -> bool {
    TINTS.read().unwrap().values().any(|t| t.progress() < 1.0)
}

/// Land a palette change: paint it once right away, then keep painting
/// while any window's easing run moves.
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
            let settled = !any_tint_easing();
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
    let base = *BASE.read().unwrap();
    // A settled seedless tint reads the same as no entry, so drop those
    // rather than let idle windows accumulate slots. Runs on the UI thread
    // between reads, so the write never races a paint.
    TINTS
        .write()
        .unwrap()
        .retain(|_, tint| tint.seed.is_some() || tint.progress() < 1.0);
    // The one widget theme follows the focused window's playback, the
    // gpui-component theme being a single global. Its own windows' panels
    // still tint per player through the accessors; this is only the dock
    // chrome, tables, and inputs the widget theme reaches.
    let focused_tint = {
        let tints = TINTS.read().unwrap();
        FOCUSED
            .read()
            .unwrap()
            .and_then(|id| tints.get(&id).copied())
    };
    let (palette, opacity, light) = match focused_tint {
        Some(tint) => {
            let light =
                base.art_theming && tint.seed.is_some_and(|seed| seed.lightness > LIGHT_COVER);
            (tint.snapshot(), base.surface_opacity, light)
        }
        None => (base.base, base.surface_opacity, false),
    };
    // Start over from the stock baseline so repeated feeds project onto
    // pristine values instead of compounding. The baseline follows the
    // ladder, so the widget tokens we never project (scrollbars,
    // popovers, dialogs) don't sit dark on a light album.
    let mode = if light {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    Theme::change(mode, None, cx);
    let theme = Theme::global_mut(cx);
    // Selection follows the accent instead of the stock blue.
    theme.table_active = alpha(palette.accent, 0x26).into();
    theme.table_active_border = palette.accent.into();
    theme.list_active = alpha(palette.accent, 0x26).into();
    theme.list_active_border = palette.accent.into();
    // Hairlines follow our border roles instead of the stock set: the
    // stock dark values sat near ours, but a light baseline's read
    // near-white against the tinted ladder. Borders read plain, like our
    // own border accessors - a thinned hairline is just a ghost.
    theme.border = palette.border.into();
    theme.sidebar_border = palette.border.into();
    theme.title_bar_border = palette.border.into();
    theme.table_row_border = palette.border.into();
    theme.input = palette.border_light.into();
    theme.ring = palette.accent.into();
    // The scrollbar thumb rides the ink ladder like the faint text it
    // sits beside; the track stays the stock transparent. Same alphas as
    // the stock thumb, resting slightly sheer, opaque under the pointer.
    theme.scrollbar_thumb = alpha(palette.text_faint, 0xe6).into();
    theme.scrollbar_thumb_hover = palette.text_faint.into();
    // Input boxes: resting border from our border role, focus ring from
    // the accent, the same pair the hand-rolled search box drew with.
    theme.input = palette.border.into();
    theme.ring = palette.accent.into();
    // The chrome between the backdrop and the panel content, projected
    // from the palette roles whose ladder values sit nearest the stock
    // dark set, so palette edits and art tinting recolor the dock and
    // table along with everything else. One deref up front: field
    // borrows through the Theme wrapper would each re-borrow it.
    let colors: &mut ThemeColor = theme;
    // Floating menus - the right-click context menus and their submenus -
    // are overlays with no backdrop behind them, so they read the raw
    // palette fields, not the opacity-scaled surface accessors: a popup
    // stays filled while the panels it floats over thin. gpui-component's
    // `accent` is its subtle highlight surface, our bg_menu_hover, not the
    // brand accent, so a selected row matches the menubar dropdown's own
    // hover instead of flooding with color. Selected text lands a step
    // brighter than the resting item, the ladder's own order.
    colors.popover = palette.bg_menu.into();
    colors.popover_foreground = palette.text.into();
    colors.foreground = palette.text.into();
    colors.accent = palette.bg_menu_hover.into();
    colors.accent_foreground = palette.text_bright.into();
    // The completion menu paints its matched-prefix highlight with the
    // stock `blue` token; route it through the brand accent so
    // suggestion matches read in the app's own highlight color.
    colors.blue = palette.accent.into();
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

    /// The settings map must carry a palette losslessly, or the user's
    /// colors drift a little on every restart.
    #[test]
    fn map_roundtrips() {
        let palette = Palette {
            accent: rgb(0x336699),
            ..Default::default()
        };
        let back = Palette::from_map(&palette.to_map());
        for role in ROLES {
            let (a, b) = ((role.get)(&palette), (role.get)(&back));
            for (a, b) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
                assert!((a - b).abs() < 0.003, "{} drifted: {a} vs {b}", role.name);
            }
        }
    }

    /// A seed with only a primary color, at a cover lightness that keeps
    /// the dark ladder.
    fn dark_seed(color: Rgba) -> Seed {
        Seed {
            primary: Some(color),
            secondary: None,
            lightness: 0.3,
        }
    }

    /// Derivation's core promise: whatever the seed, every role keeps
    /// its lightness, so the contrast ladder survives.
    #[test]
    fn derivation_preserves_lightness() {
        let base = Palette::default();
        for seed in [rgb(0xff2200), rgb(0x2244ff), rgb(0x88ff00), rgb(0xfdcb00)] {
            let derived = derive(&base, Some(dark_seed(seed)));
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

    /// Borders follow the seed past the near-gray cap: a saturated
    /// album gets dividers that are a shade of it, not gray.
    #[test]
    fn borders_outrun_the_gray_cap() {
        let base = Palette::default();
        let seed = rgb(0x88ff00);
        let (.., seed_h) = rgba_to_oklch(seed);
        let derived = derive(&base, Some(dark_seed(seed)));
        let (l, c, h) = rgba_to_oklch(derived.border);
        let (base_l, ..) = rgba_to_oklch(base.border);
        assert!((l - base_l).abs() < 0.02, "border lightness drifted");
        // The gamut walk may still trim the request at border lightness,
        // but the result must land clear of the near-gray cap and on the
        // seed's hue.
        assert!(c > TINT_CAP + 0.02, "border stuck at the gray cap: {c}");
        assert!((h - seed_h).abs() < 0.05, "border missed the hue");
    }

    /// An achromatic cover strips the brand accent to neutral instead of
    /// leaving it a lone spot of color: a grayscale album gets a grayscale
    /// app, the accent's lightness held while its color goes.
    #[test]
    fn achromatic_cover_neutralizes_accent() {
        let base = Palette::default();
        let (accent_l, accent_c, _) = rgba_to_oklch(base.accent);
        assert!(
            accent_c > CHROMATIC,
            "premise: the default accent is colorful"
        );
        let derived = derive(
            &base,
            Some(Seed {
                primary: None,
                secondary: None,
                lightness: 0.3,
            }),
        );
        for role in [derived.accent, derived.accent_hover] {
            let (_, c, _) = rgba_to_oklch(role);
            assert!(c < 0.02, "colorful role kept its chroma: {c}");
        }
        let (l, ..) = rgba_to_oklch(derived.accent);
        assert!((l - accent_l).abs() < 0.02, "accent lightness drifted");
        // The near-gray roles were already neutral and come through
        // untouched, so the ladder's contrast survives.
        let (text_l, ..) = rgba_to_oklch(derived.text);
        let (base_text_l, ..) = rgba_to_oklch(base.text);
        assert!((text_l - base_text_l).abs() < 0.02, "text drifted");
    }

    /// A bright cover flips the ladder: surfaces light, ink dark, even
    /// when the cover is too achromatic to tint anything.
    #[test]
    fn bright_cover_goes_light() {
        let base = Palette::default();
        for primary in [Some(rgb(0xff2200)), None] {
            let derived = derive(
                &base,
                Some(Seed {
                    primary,
                    secondary: None,
                    lightness: 0.9,
                }),
            );
            let (root_l, ..) = rgba_to_oklch(derived.bg_root);
            let (text_l, ..) = rgba_to_oklch(derived.text);
            assert!(root_l > 0.8, "root stayed dark: {root_l}");
            assert!(text_l < 0.5, "text stayed light: {text_l}");
        }
    }

    /// The runner-up cover color takes the highlight role as itself:
    /// its hue survives, its lightness lands in the mark band opposite
    /// the surfaces, and enough chroma survives the clamp to read as
    /// the cover's color rather than a gray.
    #[test]
    fn secondary_takes_highlight() {
        let base = Palette::default();
        let blue = rgb(0x2244ff);
        let (.., blue_h) = rgba_to_oklch(blue);
        let derived = derive(
            &base,
            Some(Seed {
                secondary: Some(blue),
                ..dark_seed(rgb(0xff2200))
            }),
        );
        let (l, c, h) = rgba_to_oklch(derived.highlight);
        let (lo, hi) = HIGHLIGHT_DARK_BAND;
        assert!(
            (lo - 0.01..=hi + 0.01).contains(&l),
            "highlight left the dark band: {l}"
        );
        assert!((h - blue_h).abs() < 0.05, "highlight missed the hue");
        assert!(c > 0.1, "highlight barely tinted: {c}");
    }

    fn assert_rgb_eq(a: Rgba, b: Rgba, what: &str) {
        for (a, b) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
            assert!((a - b).abs() < 0.003, "{what} drifted: {a} vs {b}");
        }
    }

    /// A panel scope's promise: overridden roles read as written, the
    /// rest fall through, an overridden opacity scales and lifts like the
    /// app's, and everything is back to the app palette once the scope
    /// drops.
    #[test]
    fn scope_overrides_and_falls_through() {
        let mut theme = PanelTheme::default();
        theme.set_color("accent", Some(rgb(0x2244ff)));
        theme.surface_opacity = Some(0.5);
        let scope = theme.scope().unwrap();

        let outside_accent = accent();
        let outside_text = text();
        scoped(&scope, || {
            assert_rgb_eq(accent(), rgb(0x2244ff), "overridden accent");
            // Not overridden: same color as outside, but at the scope's
            // opacity - a surface thins, ink lifts halfway to bright.
            let root = bg_root();
            assert!((root.a - 0.5).abs() < 0.001, "surface kept app opacity");
            assert_rgb_eq(text(), mix(outside_text, text_bright(), 0.5), "lifted ink");
        });
        assert_rgb_eq(accent(), outside_accent, "accent after the scope");
        assert!((bg_root().a - 1.0).abs() < 0.001, "opacity after the scope");
    }

    /// The theme's map shape survives a config roundtrip, and an empty
    /// theme serializes to nothing at all.
    #[test]
    fn panel_theme_roundtrips() {
        let mut theme = PanelTheme::default();
        theme.set_color("accent", Some(rgb(0x336699)));
        theme.surface_opacity = Some(0.8);
        let json = serde_json::to_string(&theme).unwrap();
        let back: PanelTheme = serde_json::from_str(&json).unwrap();
        assert_rgb_eq(
            back.color("accent").unwrap(),
            rgb(0x336699),
            "accent override",
        );
        assert_eq!(back.surface_opacity, Some(0.8));

        theme.set_color("accent", None);
        theme.surface_opacity = None;
        assert!(theme.is_empty());
        assert!(theme.scope().is_none());
        assert_eq!(serde_json::to_string(&theme).unwrap(), "{}");
    }
}
