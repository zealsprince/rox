//! The app palette per ADR 10: every color the UI draws, one token per role,
//! behind plain accessors. Panels never inline hex values.
//!
//! The base palettes (one per theme) and the transparency scalars are
//! app-wide. The art tint is per playback, keyed by player, so a second
//! window's track tints only its own windows. With song theming on,
//! [`set_seed`] moves every role's hue and chroma toward the cover while
//! lightness holds, so the contrast ladder survives any album; a bright
//! cover swaps in the light palette first. The one gpui-component widget
//! theme is a global, so it follows the focused window's playback.
//!
//! Changes ease from wherever the palette visibly is. The statics sit
//! outside gpui's reactivity, so every setter repaints explicitly. Per ADR
//! 13 a panel can carry a [`PanelTheme`], a sparse override the accessors
//! read inside [`scoped`].

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

use gpui::{App, EntityId, Rgba, px, rgb};
use gpui_component::{Theme, ThemeColor, ThemeMode};
use serde::{Deserialize, Serialize};

use super::tokens::EASE_SECS;

pub fn alpha(color: Rgba, a: u8) -> Rgba {
    Rgba {
        a: a as f32 / 255.0,
        ..color
    }
}

/// Deterministic off the alias-resolved, folded name, so a genre keeps its
/// hue everywhere and as the library grows (an index-spaced scheme would
/// reshuffle on every addition). Saturation and lightness pin per theme.
pub fn genre_color(name: &str) -> Rgba {
    match genre_hash(name) {
        Some(hash) => {
            let dark = matches!(mode(), Mode::Dark);
            let (saturation, lightness): (f32, f32) =
                if dark { (0.42, 0.34) } else { (0.52, 0.74) };
            // A tone step so two genres in one hue family still split.
            let jitter = ((hash >> 9) % 16) as f32 / 16.0 * 0.12 - 0.06;
            Rgba::from(gpui::hsla(
                hue_of(hash),
                saturation,
                (lightness + jitter).clamp(0.15, 0.85),
                1.0,
            ))
        }
        None => untagged_color(),
    }
}

/// The genre color and a partner drifted 25 to 60 degrees along the wheel,
/// so every gradient leans its own way.
pub fn genre_color_pair(name: &str) -> (Rgba, Rgba) {
    let base = genre_color(name);
    let Some(hash) = genre_hash(name) else {
        return (base, mix(base, rgb(0x808088), 0.25));
    };
    let dark = matches!(mode(), Mode::Dark);
    let (saturation, lightness): (f32, f32) = if dark { (0.42, 0.34) } else { (0.52, 0.74) };
    // Independent hash bits, so genres sharing a hue still differ.
    let drift = 0.07 + ((hash >> 17) % 64) as f32 / 64.0 * 0.10;
    let signed = if (hash >> 23) & 1 == 0 { drift } else { -drift };
    let partner_hue = (hue_of(hash) + signed).rem_euclid(1.0);
    // Off the base's jittered tone, so the lean stays the same size.
    let jitter = ((hash >> 9) % 16) as f32 / 16.0 * 0.12 - 0.06;
    let step = if (hash >> 29) & 1 == 0 { 0.07 } else { -0.07 };
    let partner = Rgba::from(gpui::hsla(
        partner_hue,
        saturation,
        (lightness + jitter + step).clamp(0.15, 0.85),
        1.0,
    ));
    (base, partner)
}

/// For derived looks beyond color (the genre grid's motifs). 0 for untagged.
pub fn genre_seed(name: &str) -> u64 {
    genre_hash(name).unwrap_or(0)
}

/// FNV-1a finished with splitmix64, since raw FNV mixes its low bits poorly
/// on short keys. None for the untagged bucket.
///
/// Consumers slice disjoint fields off this hash; keep the map here: hue
/// `% 360` over the whole word, lightness jitter bits 9-12, motif 13-16,
/// drift 17-29, placement and scale 33-41, symmetry 42-44, gradient angle
/// 45-53, arrangement rotation 55-59. Never take `% 8` or `% 16` of the raw
/// value: both divide 360, so the field would track the hue.
fn genre_hash(name: &str) -> Option<u64> {
    let key = rox_library::genre::resolve(name).to_lowercase();
    if key.is_empty() {
        return None;
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in key.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    Some(hash ^ (hash >> 31))
}

fn hue_of(hash: u64) -> f32 {
    (hash % 360) as f32 / 360.0
}

fn untagged_color() -> Rgba {
    if matches!(mode(), Mode::Dark) {
        rgb(0x3c3c40)
    } else {
        rgb(0xc8c8cc)
    }
}

/// By the color's own luminance rather than the theme, so a dim light
/// palette still gets legible cards.
pub fn text_on(color: Rgba) -> Rgba {
    let luminance = 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
    if luminance > 0.55 {
        rgb(0x1c1c1e)
    } else {
        rgb(0xf3f3f5)
    }
}

pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

fn scaled(color: Rgba, opacity: f32) -> Rgba {
    Rgba {
        a: color.a * opacity,
        ..color
    }
}

// Oklch, hand-rolled from Ottosson's reference so tinting can hold a
// token's lightness exactly, which sRGB or HSL math can't.

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

#[allow(clippy::excessive_precision)]
pub fn rgba_to_oklch(color: Rgba) -> (f32, f32, f32) {
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

/// Out of gamut, chroma shrinks until the color fits: clipping channels
/// would shift lightness, and lightness and hue are the promise.
pub fn oklch_to_rgba(lightness: f32, chroma: f32, hue: f32, a: f32) -> Rgba {
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

/// `name` keys the settings map and stays stable; `label` is the editor's.
pub struct Role {
    pub name: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub get: fn(&Palette) -> Rgba,
    pub set: fn(&mut Palette, Rgba),
}

/// One line per role defines its [`Palette`] field, default, editor label,
/// accessor, and [`ROLES`] entry. `surfaces` read out at surface opacity;
/// `tints` draw on a surface that already has the wash, so they read at its
/// square; `ink` lifts toward `text_bright` as surfaces thin, so contrast
/// holds. The rest read plain.
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
        /// The default is the stock dark look.
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
            fn mixed(from: &Palette, to: &Palette, t: f32) -> Palette {
                Palette {
                    $( $role: mix(from.$role, to.$role, t), )*
                    $( $srole: mix(from.$srole, to.$srole, t), )*
                    $( $trole: mix(from.$trole, to.$trole, t), )*
                    $( $irole: mix(from.$irole, to.$irole, t), )*
                }
            }

            fn map(&self, f: impl Fn(Rgba) -> Rgba) -> Palette {
                Palette {
                    $( $role: f(self.$role), )*
                    $( $srole: f(self.$srole), )*
                    $( $trole: f(self.$trole), )*
                    $( $irole: f(self.$irole), )*
                }
            }
        }

        // An accessor can go uncalled while its role is still read through
        // the field. Each checks the panel scope first.
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
                let opacity = effective_opacity();
                scaled(color, opacity)
            }
        )*

        $(
            $(#[$tdoc])*
            #[allow(dead_code)]
            pub fn $trole() -> Rgba {
                let color = scope_color(stringify!($trole))
                    .unwrap_or_else(|| active_role(|p| p.$trole));
                let opacity = effective_opacity();
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
                let opacity = effective_opacity();
                mix(color, bright, 1.0 - opacity)
            }
        )*

        pub const ROLES: &[Role] = &[
            $( Role { name: stringify!($role), label: $label, group: "Core", get: |p| p.$role, set: |p, c| p.$role = c }, )*
            $( Role { name: stringify!($srole), label: $slabel, group: "Surfaces", get: |p| p.$srole, set: |p, c| p.$srole = c }, )*
            $( Role { name: stringify!($trole), label: $tlabel, group: "Tints", get: |p| p.$trole, set: |p, c| p.$trole = c }, )*
            $( Role { name: stringify!($irole), label: $ilabel, group: "Ink", get: |p| p.$irole, set: |p, c| p.$irole = c }, )*
        ];
    };
}

tokens! {
    accent: 0xffb300, "Accent";
    /// The accent lifted toward white, for hover.
    accent_hover: 0xfed840, "Accent hover";
    /// The mark over accent fills: playheads, knobs, peak caps. Song theming
    /// gives it the cover's runner-up color.
    highlight: 0xfacc15, "Highlight";

    border: 0x333333, "Border";
    border_light: 0x3a3a3a, "Border light";

    // Fixed text roles: the top of the ladder ink lifts toward, and text on
    // opaque accent fills.
    text_bright: 0xe0e0e0, "Bright text";
    /// Text over accent-filled controls.
    text_on_accent: 0x121212, "Text on accent";

    @surfaces {
        bg_root: 0x121212, "Root";
        bg_panel: 0x181818, "Panel";
        bg_elevated: 0x1c1c1c, "Elevated";
        /// The grouped lists' heading strips.
        bg_header: 0x1c1c1c, "Header";
        bg_menubar: 0x242424, "Menubar";
        bg_menu: 0x262626, "Menu";
        bg_control: 0x2a2a2a, "Control";
        bg_menu_hover: 0x2f2f2f, "Menu hover";
        bg_control_active: 0x333333, "Control active";
        bg_control_hover: 0x3a3a3a, "Control hover";
    }

    // Fills that always draw on one of the surfaces above.
    @tints {
        bg_input: 0x141414, "Input";
        bg_toolbar: 0x1f1f1f, "Toolbar";
    }

    @ink {
        text: 0xc0c0c0, "Text";
        text_secondary: 0xa0a0a0, "Secondary";
        text_dim: 0x9a9a9a, "Dim";
        text_muted: 0x808080, "Muted";
        text_faint: 0x707070, "Faint";
        gridline: 0x6e6e6e, "Gridline";
    }
}

/// Tolerates a missing `#` from a hand edit.
pub fn parse_hex(hex: &str) -> Option<Rgba> {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(rgb)
}

pub fn to_hex(c: Rgba) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8
    )
}

impl Palette {
    pub fn to_map(self) -> BTreeMap<String, String> {
        ROLES
            .iter()
            .map(|role| (role.name.to_string(), to_hex((role.get)(&self))))
            .collect()
    }

    /// Unknown keys and bad values fall away, so the file stays valid across
    /// role changes in both directions.
    pub fn from_map(map: &BTreeMap<String, String>) -> Palette {
        Palette::from_map_over(Palette::default(), map)
    }

    /// The light theme's read, over [`Palette::light`].
    pub fn from_map_over(anchor: Palette, map: &BTreeMap<String, String>) -> Palette {
        let mut palette = anchor;
        for role in ROLES {
            if let Some(color) = map.get(role.name).and_then(|hex| parse_hex(hex)) {
                (role.set)(&mut palette, color);
            }
        }
        // bg_header used to be bg_elevated, so a palette naming only the
        // latter keeps its look. [compat]
        if !map.contains_key("bg_header")
            && let Some(color) = map.get("bg_elevated").and_then(|hex| parse_hex(hex))
        {
            palette.bg_header = color;
        }
        palette
    }
}

/// A panel's sparse palette override, stored in its config in the layout
/// dump. An overridden role reads as written; the rest follow the app
/// palette. A value may name another app role instead of a hex color, and
/// then follows it live. References only ever point into the app palette,
/// so there's nothing to recurse through. The frame knobs are geometry, so
/// the themed wrapper applies them directly rather than through the scope.
#[derive(Clone, Default, PartialEq, Serialize)]
pub struct PanelTheme {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<Sides>,
    /// Kept in the panel's own background: the content pulls in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<Sides>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounding: Option<f32>,
    /// A side at zero draws nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Sides>,
    /// An older config's edge mask, folded over whichever width wins by
    /// [`border_sides`](PanelTheme::border_sides). Editing the border bakes it
    /// in and clears this. [compat]
    #[serde(skip_serializing)]
    pub legacy_border_edges: Option<BorderEdges>,
    /// A name that isn't installed falls back at render.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// A multiplier over the app font size, clamped to
    /// [`PANEL_FONT_SCALE_MIN`]..=[`PANEL_FONT_SCALE_MAX`] at render.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_scale: Option<f32>,
}

/// Hand-written on the reading side so the legacy border mask has a field.
#[derive(Default, Deserialize)]
#[serde(default)]
struct PanelThemeRepr {
    colors: BTreeMap<String, String>,
    surface_opacity: Option<f32>,
    margin: Option<Sides>,
    padding: Option<Sides>,
    rounding: Option<f32>,
    border: Option<Sides>,
    border_edges: Option<BorderEdges>,
    font: Option<String>,
    font_scale: Option<f32>,
}

impl<'de> Deserialize<'de> for PanelTheme {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<PanelTheme, D::Error> {
        let repr = PanelThemeRepr::deserialize(deserializer)?;
        Ok(PanelTheme {
            colors: repr.colors,
            surface_opacity: repr.surface_opacity,
            margin: repr.margin,
            padding: repr.padding,
            rounding: repr.rounding,
            border: repr.border,
            legacy_border_edges: repr.border_edges.filter(|edges| *edges != BorderEdges::ALL),
            font: repr.font,
            font_scale: repr.font_scale,
        })
    }
}

impl PanelTheme {
    pub fn border_sides(&self, app: Sides) -> Sides {
        let sides = self.border.unwrap_or(app);
        match self.legacy_border_edges {
            Some(edges) => sides.masked(edges),
            None => sides,
        }
    }

    /// Empty themes skip the scope and serialize away.
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
            && self.surface_opacity.is_none()
            && self.margin.is_none()
            && self.padding.is_none()
            && self.rounding.is_none()
            && self.border.is_none()
            && self.legacy_border_edges.is_none()
            && self.font.is_none()
            && self.font_scale.is_none()
    }

    /// Seeds the settings pickers; the live read path goes through
    /// [`PanelTheme::scope`], where a reference stays attached.
    pub fn color(&self, role: &str) -> Option<Rgba> {
        let value = self.colors.get(role)?;
        if let Some(color) = parse_hex(value) {
            return Some(color);
        }
        reference_role(value).map(|get| get(&resolved()))
    }

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

    pub fn reference(&self, role: &str) -> Option<&'static str> {
        let value = self.colors.get(role)?;
        if parse_hex(value).is_some() {
            return None;
        }
        let value = value.trim();
        ROLES
            .iter()
            .find(|role| role.name == value)
            .map(|role| role.name)
    }

    pub fn set_reference(&mut self, role: &str, target: &str) {
        self.colors.insert(role.to_string(), target.to_string());
    }

    /// Unknown and unparsable entries are dropped. None while no color or
    /// opacity overrides, so renders skip the scope push.
    pub fn scope(&self) -> Option<Scope> {
        if self.colors.is_empty() && self.surface_opacity.is_none() {
            return None;
        }
        let colors: Vec<(&'static str, ScopeColor)> = ROLES
            .iter()
            .filter_map(|role| {
                let value = self.colors.get(role.name)?;
                let entry = match parse_hex(value) {
                    Some(color) => ScopeColor::Literal(color),
                    None => ScopeColor::Reference(reference_role(value)?),
                };
                Some((role.name, entry))
            })
            .collect();
        Some(Scope {
            colors: colors.into(),
            surface_opacity: self.surface_opacity.map(|o| o.clamp(0.0, 1.0)),
        })
    }
}

fn reference_role(name: &str) -> Option<fn(&Palette) -> Rgba> {
    let name = name.trim();
    ROLES
        .iter()
        .find(|role| role.name == name)
        .map(|role| role.get)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    /// Clockwise from the top, the order the editors stack them.
    pub const ALL: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];
}

/// Serializes as a bare number while every side matches, so configs from
/// before the split read straight through.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sides {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Sides {
    pub const ZERO: Sides = Sides::all(0.0);

    pub const fn all(value: f32) -> Sides {
        Sides {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    pub fn uniform(self) -> Option<f32> {
        (self.top == self.right && self.right == self.bottom && self.bottom == self.left)
            .then_some(self.top)
    }

    pub fn any(self) -> bool {
        self.top > 0.0 || self.right > 0.0 || self.bottom > 0.0 || self.left > 0.0
    }

    pub fn max(self) -> f32 {
        self.top.max(self.right).max(self.bottom).max(self.left)
    }

    pub fn get(self, side: Side) -> f32 {
        match side {
            Side::Top => self.top,
            Side::Right => self.right,
            Side::Bottom => self.bottom,
            Side::Left => self.left,
        }
    }

    pub fn with(mut self, side: Side, value: f32) -> Sides {
        match side {
            Side::Top => self.top = value,
            Side::Right => self.right = value,
            Side::Bottom => self.bottom = value,
            Side::Left => self.left = value,
        }
        self
    }

    /// A hand-edited config can hold anything, and a negative inset would push
    /// a panel out of its cell.
    pub fn positive(self) -> Sides {
        self.clamped(f32::INFINITY)
    }

    pub fn edited(self, side: Option<Side>, value: f32) -> Sides {
        match side {
            Some(side) => self.with(side, value),
            None => Sides::all(value),
        }
    }

    /// Relinks at the widest side, so what's on screen stays.
    pub fn linked(self) -> Sides {
        Sides::all(self.max())
    }

    pub fn clamped(self, max: f32) -> Sides {
        let hold = |value: f32| {
            if value.is_finite() {
                value.clamp(0.0, max)
            } else {
                0.0
            }
        };
        Sides {
            top: hold(self.top),
            right: hold(self.right),
            bottom: hold(self.bottom),
            left: hold(self.left),
        }
    }

    pub fn masked(self, edges: BorderEdges) -> Sides {
        Sides {
            top: if edges.top { self.top } else { 0.0 },
            right: if edges.right { self.right } else { 0.0 },
            bottom: if edges.bottom { self.bottom } else { 0.0 },
            left: if edges.left { self.left } else { 0.0 },
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct SidesRepr {
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SidesIn {
    All(f32),
    Each(SidesRepr),
}

impl Serialize for Sides {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.uniform() {
            Some(value) => value.serialize(serializer),
            None => SidesRepr {
                top: self.top,
                right: self.right,
                bottom: self.bottom,
                left: self.left,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Sides {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Sides, D::Error> {
        Ok(match SidesIn::deserialize(deserializer)? {
            SidesIn::All(value) => Sides::all(value),
            SidesIn::Each(each) => Sides {
                top: each.top,
                right: each.right,
                bottom: each.bottom,
                left: each.left,
            },
        })
    }
}

/// Hand-written because the serde above is: the derive would miss the
/// bare-number form, and the workspace schema (ADR 22) would flag every
/// file whose knobs were never split.
impl schemars::JsonSchema for Sides {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Sides".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A frame knob's four sides in px: one number while linked, per-side once split",
            "anyOf": [
                { "type": "number" },
                {
                    "type": "object",
                    "properties": {
                        "top": { "type": "number" },
                        "right": { "type": "number" },
                        "bottom": { "type": "number" },
                        "left": { "type": "number" },
                    },
                },
            ],
        })
    }
}

/// Read from old configs and folded onto per-side widths; never written.
/// [compat]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BorderEdges {
    #[serde(default = "edge_on")]
    pub top: bool,
    #[serde(default = "edge_on")]
    pub right: bool,
    #[serde(default = "edge_on")]
    pub bottom: bool,
    #[serde(default = "edge_on")]
    pub left: bool,
}

fn edge_on() -> bool {
    true
}

impl BorderEdges {
    pub const ALL: BorderEdges = BorderEdges {
        top: true,
        right: true,
        bottom: true,
        left: true,
    };
}

/// A reference holds its target's accessor into the app palette, never
/// another override, so resolution is one hop.
#[derive(Clone, Copy)]
enum ScopeColor {
    Literal(Rgba),
    Reference(fn(&Palette) -> Rgba),
}

/// Cheap to clone, so the themed wrapper can pass it into every render phase.
#[derive(Clone)]
pub struct Scope {
    colors: Arc<[(&'static str, ScopeColor)]>,
    surface_opacity: Option<f32>,
}

thread_local! {
    /// Innermost last, so nested themed subtrees stack. Thread-local is
    /// enough: rendering and the paint closures run on the UI thread.
    static SCOPES: RefCell<Vec<Scope>> = const { RefCell::new(Vec::new()) };
}

/// A reference samples the live app palette at read time, so it eases.
fn scope_color(role: &str) -> Option<Rgba> {
    SCOPES.with(|scopes| {
        scopes.borrow().last().and_then(|scope| {
            scope
                .colors
                .iter()
                .find(|(name, _)| *name == role)
                .map(|(_, color)| match color {
                    ScopeColor::Literal(color) => *color,
                    ScopeColor::Reference(get) => active_role(get),
                })
        })
    })
}

thread_local! {
    /// Set by workspaces, which always paint the cover backdrop. Other
    /// windows follow the All Windows switch, and read opaque without it,
    /// since transparency over a bare root makes a settings page illegible.
    static BACKDROPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn backdropped<R>(on: bool, f: impl FnOnce() -> R) -> R {
    BACKDROPPED.with(|flag| {
        let prior = flag.replace(on);
        let out = f();
        flag.set(prior);
        out
    })
}

fn effective_opacity() -> f32 {
    if BACKDROPPED.with(std::cell::Cell::get) || BASE.read().unwrap().backdrop_all_windows {
        scope_opacity().unwrap_or_else(base_surface_opacity)
    } else {
        1.0
    }
}

fn scope_opacity() -> Option<f32> {
    SCOPES.with(|scopes| {
        scopes
            .borrow()
            .last()
            .and_then(|scope| scope.surface_opacity)
    })
}

thread_local! {
    /// A panel font override's rem multiplier, innermost last. The twin of
    /// [`SCOPES`], so [`scaled_px`] reads what the window rem uses.
    static REM_SCALES: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

fn panel_rem_scale() -> f32 {
    REM_SCALES.with(|scales| scales.borrow().last().copied().unwrap_or(1.0))
}

/// The pop happens in a drop guard, so an unwinding `f` can't leave the
/// scale stuck.
pub fn rem_scaled<R>(scale: f32, f: impl FnOnce() -> R) -> R {
    REM_SCALES.with(|scales| scales.borrow_mut().push(scale));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            REM_SCALES.with(|scales| {
                scales.borrow_mut().pop();
            });
        }
    }
    let _pop = Pop;
    f()
}

/// The pop happens in a drop guard, so an unwinding `f` can't leave the
/// scope stuck.
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

/// What a cover contributes to derivation. The backdrop bake extracts one
/// per track.
#[derive(Clone, Copy)]
pub struct Seed {
    /// None for an achromatic cover, which still picks the ladder by lightness.
    pub primary: Option<Rgba>,
    /// Far enough from the primary in hue to read as a second color.
    pub secondary: Option<Rgba>,
    /// Mean perceptual lightness over the whole cover, gray mass and all.
    pub lightness: f32,
}

impl Seed {
    fn same(&self, other: &Seed) -> bool {
        let key = |c: Option<Rgba>| c.map(|c| (c.r, c.g, c.b));
        key(self.primary) == key(other.primary)
            && key(self.secondary) == key(other.secondary)
            && self.lightness == other.lightness
    }
}

/// System is resolved against the OS in the settings layer, so only a
/// concrete side gets here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Clone, Copy)]
struct Base {
    /// [`set`] writes the active side; derivation layers on top without
    /// touching it.
    dark: Palette,
    light: Palette,
    mode: Mode,
    /// Off, each tint's seed is only remembered for a later enable.
    art_theming: bool,
    /// Song theming still tints, but a cover's brightness never swaps the
    /// theme.
    keep_theme: bool,
    surface_opacity: f32,
    backdrop_strength: f32,
    backdrop_all_windows: bool,
    /// [`apply`] projects it into the widget theme, whose root pushes it to
    /// each window's rem per frame.
    font_size: f32,
}

impl Base {
    fn active(&self) -> &Palette {
        match self.mode {
            Mode::Dark => &self.dark,
            Mode::Light => &self.light,
        }
    }
}

/// One playback's easing run toward its seed, per player. The seed is kept
/// so a base or song-theming change can re-derive without the caller.
#[derive(Clone, Copy)]
pub struct Tint {
    seed: Option<Seed>,
    from: Palette,
    target: Palette,
    eased_at: Instant,
}

impl Tint {
    fn settled(palette: Palette) -> Tint {
        Tint {
            seed: None,
            from: palette,
            target: palette,
            eased_at: Instant::now(),
        }
    }

    /// Smoothstepped, so changes ease out instead of stopping dead.
    fn progress(&self) -> f32 {
        let u = (self.eased_at.elapsed().as_secs_f32() / EASE_SECS).min(1.0);
        u * u * (3.0 - 2.0 * u)
    }

    fn role(&self, pick: impl Fn(&Palette) -> Rgba) -> Rgba {
        let u = self.progress();
        if u >= 1.0 {
            pick(&self.target)
        } else {
            mix(pick(&self.from), pick(&self.target), u)
        }
    }

    fn snapshot(&self) -> Palette {
        let u = self.progress();
        if u >= 1.0 {
            self.target
        } else {
            Palette::mixed(&self.from, &self.target, u)
        }
    }

    /// Starts from wherever the run visibly is, so nothing snaps.
    fn retarget(&mut self, base: &Base) {
        self.from = self.snapshot();
        let seed = if base.art_theming { self.seed } else { None };
        self.target = derive(base, seed);
        self.eased_at = Instant::now();
    }
}

/// A static rather than a gpui global so the accessors keep their plain
/// signatures and paint closures can read them without a context.
static BASE: LazyLock<RwLock<Base>> = LazyLock::new(|| {
    RwLock::new(Base {
        dark: Palette::default(),
        light: Palette::light(),
        mode: Mode::Dark,
        art_theming: false,
        keep_theme: false,
        surface_opacity: 1.0,
        backdrop_strength: 1.0,
        backdrop_all_windows: true,
        font_size: FONT_SIZE_DEFAULT,
    })
});

/// A window pushes [`window_tint`] before it renders.
static TINTS: LazyLock<RwLock<HashMap<EntityId, Tint>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The gpui-component theme is one global, so it follows one tint: the
/// focused window's.
static FOCUSED: LazyLock<RwLock<Option<EntityId>>> = LazyLock::new(|| RwLock::new(None));

/// The player whose window last took focus, for a surface that can't name
/// its own.
pub fn focused_player() -> Option<EntityId> {
    *FOCUSED.read().unwrap()
}

thread_local! {
    /// Innermost last, matching [`SCOPES`].
    static TINT_STACK: RefCell<Vec<Tint>> = const { RefCell::new(Vec::new()) };
}

fn active_role(pick: impl Fn(&Palette) -> Rgba) -> Rgba {
    TINT_STACK.with(|stack| match stack.borrow().last() {
        Some(tint) => tint.role(&pick),
        None => pick(BASE.read().unwrap().active()),
    })
}

fn base_surface_opacity() -> f32 {
    BASE.read().unwrap().surface_opacity
}

/// The floor color at the inverse of backdrop strength.
pub fn backdrop_wash() -> Rgba {
    Rgba {
        a: 1.0 - BASE.read().unwrap().backdrop_strength,
        ..active_role(|p| p.bg_elevated)
    }
}

// Status tones, outside the user palette and the art tint: a warning that
// turns the album's color stops reading as a warning. Here rather than in
// tokens because ADR 12 keeps every color on this side.

/// Bit-perfect, matched, claimed.
pub fn tone_good() -> Rgba {
    rgb(0x4ade80)
}

/// A fallback, a resample, a setting the hardware rejected.
pub fn tone_warn() -> Rgba {
    rgb(0xfbbf24)
}

pub fn tone_bad() -> Rgba {
    rgb(0xf87171)
}

/// The choke point for an app-wide change, so callers never replay seeds.
fn retarget_all() {
    let base = *BASE.read().unwrap();
    let mut tints = TINTS.write().unwrap();
    for tint in tints.values_mut() {
        tint.retarget(&base);
    }
}

/// The one setter every palette edit goes through.
pub fn set(palette: Palette, cx: &mut App) {
    {
        let mut base = BASE.write().unwrap();
        match base.mode {
            Mode::Dark => base.dark = palette,
            Mode::Light => base.light = palette,
        }
    }
    retarget_all();
    drive(cx);
}

/// Startup and workspace apply, so the inactive side is set without a
/// second ease.
pub fn set_palettes(dark: Palette, light: Palette, cx: &mut App) {
    {
        let mut base = BASE.write().unwrap();
        base.dark = dark;
        base.light = light;
    }
    retarget_all();
    drive(cx);
}

/// A no-op resolution returns early instead of restarting every ease.
pub fn set_mode(mode: Mode, cx: &mut App) {
    {
        let mut base = BASE.write().unwrap();
        if base.mode == mode {
            return;
        }
        base.mode = mode;
    }
    retarget_all();
    drive(cx);
}

pub fn mode() -> Mode {
    BASE.read().unwrap().mode
}

pub fn theme_palette(mode: Mode) -> Palette {
    let base = BASE.read().unwrap();
    match mode {
        Mode::Dark => base.dark,
        Mode::Light => base.light,
    }
}

/// No easing: the scalars are knobs, not palette colors. Persisting is the
/// settings' writers' job.
pub fn set_scalars(surface_opacity: f32, backdrop_strength: f32, cx: &mut App) {
    {
        let mut base = BASE.write().unwrap();
        base.surface_opacity = surface_opacity.clamp(0.0, 1.0);
        base.backdrop_strength = backdrop_strength.clamp(0.0, 1.0);
    }
    apply(cx);
}

pub fn backdrop_all_windows() -> bool {
    BASE.read().unwrap().backdrop_all_windows
}

pub fn set_backdrop_all_windows(on: bool, cx: &mut App) {
    BASE.write().unwrap().backdrop_all_windows = on;
    apply(cx);
}

/// Modest: the rem text classes scale while the px chrome holds, and past
/// this the fixed chrome crowds the text.
pub const FONT_SIZE_MIN: f32 = 12.0;
pub const FONT_SIZE_MAX: f32 = 20.0;
pub const FONT_SIZE_DEFAULT: f32 = 16.0;

pub fn set_app_font_size(size: f32, cx: &mut App) {
    let size = if size.is_finite() {
        size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX)
    } else {
        FONT_SIZE_DEFAULT
    };
    {
        let mut base = BASE.write().unwrap();
        if base.font_size == size {
            return;
        }
        base.font_size = size;
    }
    apply(cx);
}

/// For fixed px heights where no `Window` is at hand to read the rem.
pub fn font_scale() -> f32 {
    BASE.read().unwrap().font_size / FONT_SIZE_DEFAULT
}

pub fn app_font_size() -> f32 {
    BASE.read().unwrap().font_size
}

/// The app size times any panel override in scope, which is the window rem
/// over 16, so sizes derived here match the vendored table's.
pub fn row_scale() -> f32 {
    font_scale() * panel_rem_scale()
}

/// For the hand-rolled `uniform_list` rows, so they track the text the way
/// the library table's rows do.
pub fn scaled_px(length: f32) -> gpui::Pixels {
    px(length * row_scale())
}

pub const PANEL_FONT_SCALE_MIN: f32 = 0.75;
pub const PANEL_FONT_SCALE_MAX: f32 = 1.5;

/// Keyed by player, so a track change tints only that player's windows.
pub fn set_seed(player: EntityId, seed: Option<Seed>, cx: &mut App) {
    let base = *BASE.read().unwrap();
    {
        let mut tints = TINTS.write().unwrap();
        let tint = tints
            .entry(player)
            .or_insert_with(|| Tint::settled(*base.active()));
        // Consecutive tracks off one album have identical art.
        let unchanged = match (&tint.seed, &seed) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same(b),
            _ => false,
        };
        if unchanged {
            return;
        }
        tint.seed = seed;
        // With song theming off the seed is only remembered.
        if !base.art_theming {
            return;
        }
        tint.retarget(&base);
    }
    drive(cx);
}

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

pub fn art_theming() -> bool {
    BASE.read().unwrap().art_theming
}

pub fn set_keep_theme(on: bool, cx: &mut App) {
    {
        let mut base = BASE.write().unwrap();
        if base.keep_theme == on {
            return;
        }
        base.keep_theme = on;
    }
    retarget_all();
    drive(cx);
}

pub fn app_surface_opacity() -> f32 {
    BASE.read().unwrap().surface_opacity
}

/// The active window tint's target, else the focused window's, else the
/// base. What the locked editor swatches show and what export saves.
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
    *BASE.read().unwrap().active()
}

/// Snapshotted per frame; the easing reads live off `eased_at`.
pub fn window_tint(player: EntityId) -> Tint {
    match TINTS.read().unwrap().get(&player) {
        Some(tint) => *tint,
        None => Tint::settled(*BASE.read().unwrap().active()),
    }
}

/// Kept whatever the song-theming switch says, for surfaces that color
/// themselves off the art directly.
pub fn seed(player: EntityId) -> Option<Seed> {
    TINTS
        .read()
        .unwrap()
        .get(&player)
        .and_then(|tint| tint.seed)
}

/// The pop happens in a drop guard, so an unwinding `f` can't leave the
/// tint stuck.
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

/// The theme change is deferred out of the render pass.
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

// The menu overlays read opaque: a floating dropdown has no backdrop
// behind it.

pub fn bg_menu_opaque() -> Rgba {
    active_role(|p| p.bg_menu)
}

/// For a panel that lays text over its own image: the floor must hide the
/// window backdrop, or two images fight under the words.
pub fn bg_root_opaque() -> Rgba {
    active_role(|p| p.bg_root)
}

pub fn bg_control_hover_opaque() -> Rgba {
    active_role(|p| p.bg_control_hover)
}

/// Enough for surfaces and text to pick up the album's cast, never enough
/// to become it.
const TINT_STRENGTH: f32 = 0.35;
const TINT_CAP: f32 = 0.045;
/// A colorful role keeps its chroma and swings only its hue.
const CHROMATIC: f32 = 0.05;
/// Borders sit on surfaces the backdrop saturates past the capped tint, so
/// they take a share of the seed's chroma to stay a darker shade of the
/// field.
const BORDER_TINT: f32 = 0.6;
/// The same cut [`apply`] makes to pick the widget baseline.
const LIGHT_COVER: f32 = 0.70;
/// One band per ladder. Pinning it to the ladder's own mark lightness would
/// crush a vivid red to maroon on a light album.
const HIGHLIGHT_DARK_BAND: (f32, f32) = (0.60, 0.85);
const HIGHLIGHT_LIGHT_BAND: (f32, f32) = (0.30, 0.55);

impl Palette {
    /// The dark defaults' designed counterpart, and the far anchor
    /// [`Palette::inverse`] applies edits to.
    pub fn light() -> Palette {
        Palette {
            accent: rgb(0xb07d00),
            accent_hover: rgb(0x976a00),
            highlight: rgb(0x1f1f1f),
            // Deeper than a mirror of the dark deltas: the backdrop eats
            // hairline contrast on translucent surfaces.
            border: rgb(0xb3b3b3),
            border_light: rgb(0xa9a9a9),
            text_bright: rgb(0x1a1a1a),
            text_on_accent: rgb(0xfafafa),
            bg_root: rgb(0xededed),
            bg_panel: rgb(0xe7e7e7),
            bg_elevated: rgb(0xe3e3e3),
            bg_header: rgb(0xe3e3e3),
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

    fn mean_surface_lightness(&self) -> f32 {
        let (sum, count) = ROLES
            .iter()
            .filter(|role| role.group == "Surfaces")
            .map(|role| rgba_to_oklch((role.get)(self)).0)
            .fold((0.0, 0), |(sum, count), lightness| {
                (sum + lightness, count + 1)
            });
        sum / count.max(1) as f32
    }

    /// Not a raw lightness mirror: each role's oklch distance from the nearer
    /// designed ladder is re-applied over the far one, so an untouched
    /// palette returns the hand-tuned counterpart exactly and pinned pairs
    /// like text on accent stay readable.
    pub fn inverse(&self) -> Palette {
        let dark = Palette::default();
        let light = Palette::light();
        let midpoint = (dark.mean_surface_lightness() + light.mean_surface_lightness()) / 2.0;
        let (near, far) = if self.mean_surface_lightness() <= midpoint {
            (&dark, &light)
        } else {
            (&light, &dark)
        };
        let mut out = *far;
        for role in ROLES {
            let color = (role.get)(self);
            let (self_l, self_c, self_h) = rgba_to_oklch(color);
            let (near_l, near_c, near_h) = rgba_to_oklch((role.get)(near));
            let (far_l, far_c, far_h) = rgba_to_oklch((role.get)(far));
            let lightness = (far_l + (self_l - near_l)).clamp(0.0, 1.0);
            let chroma = (far_c + (self_c - near_c)).max(0.0);
            let hue = far_h + (self_h - near_h);
            (role.set)(&mut out, oklch_to_rgba(lightness, chroma, hue, color.a));
        }
        out
    }
}

/// A bright cover derives over the light palette unless keep-theme pins
/// the active side. An achromatic cover strips the colorful roles, so a
/// black-and-white album gets a black-and-white app.
fn derive(base: &Base, seed: Option<Seed>) -> Palette {
    let Some(seed) = seed else {
        return *base.active();
    };
    let mode = if base.keep_theme {
        base.mode
    } else if seed.lightness > LIGHT_COVER {
        Mode::Light
    } else {
        Mode::Dark
    };
    let light = mode == Mode::Light;
    let ladder = match mode {
        Mode::Dark => base.dark,
        Mode::Light => base.light,
    };
    let Some(primary) = seed.primary else {
        // No hue: drop the colorful roles' chroma, or the accent stays a lone
        // spot of color against a gray cover.
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
    // The runner-up takes the highlight as itself, its lightness clamped into
    // the mark band.
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

/// A newer change takes the loop over and the old pump dies on its tick.
static PUMP: AtomicU64 = AtomicU64::new(0);

fn any_tint_easing() -> bool {
    TINTS.read().unwrap().values().any(|t| t.progress() < 1.0)
}

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
            // Checked before the apply, so the final frame paints the target.
            if settled {
                return;
            }
        }
    })
    .detach();
}

/// Per ADR 10 the widget theme is a projection of our tokens, never the
/// source; what isn't projected keeps the stock set.
fn apply(cx: &mut App) {
    let base = *BASE.read().unwrap();
    // Drop settled seedless tints, so idle windows don't accumulate slots.
    TINTS
        .write()
        .unwrap()
        .retain(|_, tint| tint.seed.is_some() || tint.progress() < 1.0);
    let focused_tint = {
        let tints = TINTS.read().unwrap();
        FOCUSED
            .read()
            .unwrap()
            .and_then(|id| tints.get(&id).copied())
    };
    let (palette, opacity) = match focused_tint {
        Some(tint) => (tint.snapshot(), base.surface_opacity),
        None => (*base.active(), base.surface_opacity),
    };
    // Start from the stock baseline, chosen by the palette's own lightness
    // rather than the theme pick, so unprojected tokens read on it.
    let light = palette.mean_surface_lightness() > LIGHT_COVER;
    let mode = if light {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    Theme::change(mode, None, cx);
    let theme = Theme::global_mut(cx);
    // `Theme::change` reset the font size, so reproject it.
    theme.font_size = px(base.font_size);
    theme.table_active = alpha(palette.accent, 0x26).into();
    theme.table_active_border = palette.accent.into();
    theme.list_active = alpha(palette.accent, 0x26).into();
    theme.list_active_border = palette.accent.into();
    // Stock hairlines read near-white against the tinted light ladder.
    theme.border = palette.border.into();
    theme.sidebar_border = palette.border.into();
    theme.title_bar_border = palette.border.into();
    theme.table_row_border = palette.border.into();
    theme.scrollbar_thumb = alpha(palette.text_faint, 0xe6).into();
    theme.scrollbar_thumb_hover = palette.text_faint.into();
    theme.input = palette.border.into();
    theme.ring = palette.accent.into();
    // One deref up front: field borrows through the Theme wrapper would
    // each re-borrow it.
    let colors: &mut ThemeColor = theme;
    // Floating menus have no backdrop behind them, so they read the raw
    // fields. gpui-component's `accent` is its highlight surface, not our
    // brand accent.
    colors.popover = palette.bg_menu.into();
    colors.popover_foreground = palette.text.into();
    colors.foreground = palette.text.into();
    colors.accent = palette.bg_menu_hover.into();
    colors.accent_foreground = palette.text_bright.into();
    // The completion menu highlights matches with the stock `blue`.
    colors.blue = palette.accent.into();
    colors.tab_bar = scaled(palette.bg_panel, opacity).into();
    colors.tab_active = scaled(palette.bg_root, opacity).into();
    colors.secondary = scaled(palette.bg_panel, opacity).into();
    colors.table_hover = scaled(palette.bg_menu, opacity).into();
    // Striping must be a step above the panel. The tint rule cancels out on
    // a translucent skin, so use the elevated surface at a fixed half alpha.
    let stripe = alpha(palette.bg_elevated, 0x80);
    colors.table_even = stripe.into();
    colors.table_head = stripe.into();
    // Structural backstops sit under a surface that already has the wash;
    // scaling them would stack fog, so translucency drops them.
    let structural = if opacity < 1.0 { 0.0 } else { 1.0 };
    colors.background = scaled(palette.bg_root, structural).into();
    colors.table = scaled(palette.bg_root, structural).into();
    // Chrome labels lift toward text_bright as surfaces thin. Left stock they
    // stay gray, which reads as a bug on a high-chroma skin like Phosphor.
    colors.tab_foreground = palette.text.into();
    colors.muted_foreground = palette.text_muted.into();
    colors.secondary_foreground = palette.text_bright.into();
    colors.table_head_foreground = palette.text_faint.into();
    let lift = 1.0 - opacity;
    for token in [
        &mut colors.tab_foreground,
        &mut colors.muted_foreground,
        &mut colors.secondary_foreground,
        &mut colors.table_head_foreground,
    ] {
        *token = mix((*token).into(), palette.text_bright, lift).into();
    }
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn sides_parse() {
        let theme: PanelTheme =
            serde_json::from_str(r#"{"margin": 4.0, "padding": {"top": 2.0, "left": 8.0}}"#)
                .unwrap();
        assert_eq!(theme.margin, Some(Sides::all(4.0)));
        let padding = theme.padding.unwrap();
        assert_eq!((padding.top, padding.left), (2.0, 8.0));
        // Sides an object leaves out are off, not inherited.
        assert_eq!((padding.right, padding.bottom), (0.0, 0.0));

        let json = serde_json::to_string(&theme).unwrap();
        assert!(json.contains(r#""margin":4.0"#), "{json}");
        assert!(json.contains(r#""padding":{"#), "{json}");
        let back: PanelTheme = serde_json::from_str(&json).unwrap();
        assert_eq!(back.margin, theme.margin);
        assert_eq!(back.padding, theme.padding);
    }

    #[test]
    fn legacy_border_edges_fold() {
        let own: PanelTheme =
            serde_json::from_str(r#"{"border": 2.0, "border_edges": {"top": false}}"#).unwrap();
        let sides = own.border_sides(Sides::all(1.0));
        assert_eq!(
            (sides.top, sides.right, sides.bottom, sides.left),
            (0.0, 2.0, 2.0, 2.0)
        );

        let inherited: PanelTheme =
            serde_json::from_str(r#"{"border_edges": {"top": false, "right": false}}"#).unwrap();
        let sides = inherited.border_sides(Sides::all(1.0));
        assert_eq!(
            (sides.top, sides.right, sides.bottom, sides.left),
            (0.0, 0.0, 1.0, 1.0)
        );

        let unmasked: PanelTheme = serde_json::from_str(
            r#"{"border": 2.0, "border_edges": {"top": true, "right": true, "bottom": true, "left": true}}"#,
        )
        .unwrap();
        assert!(unmasked.legacy_border_edges.is_none());

        let json = serde_json::to_string(&own).unwrap();
        assert!(!json.contains("border_edges"), "{json}");
    }

    fn dark_seed(color: Rgba) -> Seed {
        Seed {
            primary: Some(color),
            secondary: None,
            lightness: 0.3,
        }
    }

    fn test_base(keep_theme: bool) -> Base {
        Base {
            dark: Palette::default(),
            light: Palette::light(),
            mode: Mode::Dark,
            art_theming: true,
            keep_theme,
            surface_opacity: 1.0,
            backdrop_strength: 1.0,
            backdrop_all_windows: true,
            font_size: FONT_SIZE_DEFAULT,
        }
    }

    #[test]
    fn derivation_preserves_lightness() {
        let base = Palette::default();
        for seed in [rgb(0xff2200), rgb(0x2244ff), rgb(0x88ff00), rgb(0xfdcb00)] {
            let derived = derive(&test_base(false), Some(dark_seed(seed)));
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

    #[test]
    fn borders_outrun_the_gray_cap() {
        let base = Palette::default();
        let seed = rgb(0x88ff00);
        let (.., seed_h) = rgba_to_oklch(seed);
        let derived = derive(&test_base(false), Some(dark_seed(seed)));
        let (l, c, h) = rgba_to_oklch(derived.border);
        let (base_l, ..) = rgba_to_oklch(base.border);
        assert!((l - base_l).abs() < 0.02, "border lightness drifted");
        assert!(c > TINT_CAP + 0.02, "border stuck at the gray cap: {c}");
        assert!((h - seed_h).abs() < 0.05, "border missed the hue");
    }

    #[test]
    fn achromatic_cover_neutralizes_accent() {
        let base = Palette::default();
        let (accent_l, accent_c, _) = rgba_to_oklch(base.accent);
        assert!(
            accent_c > CHROMATIC,
            "premise: the default accent is colorful"
        );
        let derived = derive(
            &test_base(false),
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
        let (text_l, ..) = rgba_to_oklch(derived.text);
        let (base_text_l, ..) = rgba_to_oklch(base.text);
        assert!((text_l - base_text_l).abs() < 0.02, "text drifted");
    }

    #[test]
    fn bright_cover_goes_light() {
        for primary in [Some(rgb(0xff2200)), None] {
            let derived = derive(
                &test_base(false),
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

    #[test]
    fn bright_cover_uses_light_theme_palette() {
        let mut base = test_base(false);
        base.light.bg_root = rgb(0xc9c9c9);
        let derived = derive(
            &base,
            Some(Seed {
                primary: Some(rgb(0xff2200)),
                secondary: None,
                lightness: 0.9,
            }),
        );
        let (root_l, ..) = rgba_to_oklch(derived.bg_root);
        let (edit_l, ..) = rgba_to_oklch(base.light.bg_root);
        let (stock_l, ..) = rgba_to_oklch(Palette::light().bg_root);
        assert!(
            (root_l - edit_l).abs() < 0.02,
            "root ignored the light theme's edit: {root_l}"
        );
        assert!((edit_l - stock_l).abs() > 0.04, "premise: the edit moved");
    }

    #[test]
    fn dark_cover_flips_a_light_theme_dark() {
        let mut base = test_base(false);
        base.mode = Mode::Light;
        let derived = derive(&base, Some(dark_seed(rgb(0xff2200))));
        let (root_l, ..) = rgba_to_oklch(derived.bg_root);
        let (text_l, ..) = rgba_to_oklch(derived.text);
        assert!(root_l < 0.3, "root stayed light: {root_l}");
        assert!(text_l > 0.5, "text stayed dark: {text_l}");
    }

    #[test]
    fn keep_theme_holds_the_active_palette() {
        let base = Palette::default();
        let derived = derive(
            &test_base(true),
            Some(Seed {
                primary: Some(rgb(0xff2200)),
                secondary: None,
                lightness: 0.9,
            }),
        );
        let (root_l, ..) = rgba_to_oklch(derived.bg_root);
        let (text_l, ..) = rgba_to_oklch(derived.text);
        let (base_root_l, ..) = rgba_to_oklch(base.bg_root);
        let (base_text_l, ..) = rgba_to_oklch(base.text);
        assert!(
            (root_l - base_root_l).abs() < 0.02,
            "root left the dark ladder: {root_l}"
        );
        assert!(
            (text_l - base_text_l).abs() < 0.02,
            "text left the dark ladder: {text_l}"
        );
    }

    #[test]
    fn inverse_lands_on_designed_ladder() {
        let dark = Palette::default();
        let light = Palette::light();
        let flipped = dark.inverse();

        // Compared in sRGB: the near-gray roles have no meaningful hue.
        for role in ROLES {
            let flip = (role.get)(&flipped);
            let want = (role.get)(&light);
            assert!(
                (flip.r - want.r).abs() < 0.02
                    && (flip.g - want.g).abs() < 0.02
                    && (flip.b - want.b).abs() < 0.02,
                "role {} didn't land on the light ladder",
                role.name
            );
        }

        assert!(rgba_to_oklch(flipped.bg_root).0 > rgba_to_oklch(dark.bg_root).0);
        assert!(rgba_to_oklch(flipped.text).0 < rgba_to_oklch(dark.text).0);

        let round = flipped.inverse();
        for role in ROLES {
            let (rl, ..) = rgba_to_oklch((role.get)(&round));
            let (dl, ..) = rgba_to_oklch((role.get)(&dark));
            assert!(
                (rl - dl).abs() < 0.02,
                "round trip drifted on {}",
                role.name
            );
        }

        let mut edited = dark;
        edited.bg_panel = rgb(0x0a1a2e);
        let flipped = edited.inverse();
        let (edited_l, ..) = rgba_to_oklch(edited.bg_panel);
        let (dark_l, ..) = rgba_to_oklch(dark.bg_panel);
        let (light_l, light_c, _) = rgba_to_oklch(light.bg_panel);
        let (flip_l, flip_c, _) = rgba_to_oklch(flipped.bg_panel);
        assert!(
            (flip_l - (light_l + (edited_l - dark_l))).abs() < 0.02,
            "edit lightness didn't carry"
        );
        assert!(flip_c > light_c, "edit chroma didn't carry");
    }

    #[test]
    fn secondary_takes_highlight() {
        let blue = rgb(0x2244ff);
        let (.., blue_h) = rgba_to_oklch(blue);
        let derived = derive(
            &test_base(false),
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
            let root = bg_root();
            assert!((root.a - 0.5).abs() < 0.001, "surface kept app opacity");
            assert_rgb_eq(text(), mix(outside_text, text_bright(), 0.5), "lifted ink");
        });
        assert_rgb_eq(accent(), outside_accent, "accent after the scope");
        assert!((bg_root().a - 1.0).abs() < 0.001, "opacity after the scope");
    }

    #[test]
    fn scope_reference_follows_role() {
        let mut theme = PanelTheme::default();
        theme.set_reference("bg_root", "accent");
        theme.set_reference("border", "no_such_role");
        assert_eq!(theme.reference("bg_root"), Some("accent"));
        assert_eq!(theme.reference("border"), None);
        assert_eq!(theme.reference("accent"), None);
        assert_rgb_eq(
            theme.color("bg_root").unwrap(),
            resolved().accent,
            "seed resolve",
        );

        let scope = theme.scope().unwrap();
        let outside_border = border();
        scoped(&scope, || {
            assert_rgb_eq(bg_root(), accent(), "referenced root");
            assert_rgb_eq(border(), outside_border, "bad reference fell through");
        });
    }

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
