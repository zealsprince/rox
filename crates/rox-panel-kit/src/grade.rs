//! The colour grade a Milkdrop frame passes through on its way to the screen.
//! Presets assume black, which on the light theme is a dark hole in a pale
//! window, so the frame gets one of three remaps before it's composited. The
//! Milkdrop panel and the backdrop share the same WGSL and slots.
//!
//! - `Theme` inverts Oklab lightness on the light theme, keeping chroma and hue.
//! - `Palette` maps lightness onto a background-to-accent ramp in Oklab.
//! - `Cover` runs a three-stop ramp through the cover's colour at full chroma,
//!   since a two-stop ramp from grey reads as a tint. The cover lends hue and
//!   chroma only; its lightness would kill the contrast on a dark cover. A
//!   pixel keeps its own chroma where that beats the ramp's, and its hue folds
//!   near the cover's, so the result isn't a duotone. With no colour to lend,
//!   `Cover` degrades to `Palette`.
//!
//! Everything runs in linear light: the frame texture decodes sRGB on read and
//! Oklab is defined from linear sRGB.

use gpui::Rgba;
use rox_design::palette;

/// The slot layout every Milkdrop pass shares. The fade, hue and tint are
/// the panel's; the backdrop leaves the tint at zero.
pub const SLOT_FADE: usize = 0;
pub const SLOT_HUE: usize = 1;
pub const SLOT_TINT: usize = 2;
pub const SLOT_MODE: usize = 3;
pub const SLOT_BG: usize = 4;
pub const SLOT_LIGHT: usize = 7;
pub const SLOT_ACCENT: usize = 8;
/// Two slots: the cover's Oklab hue in radians, then its chroma. Slot 13 is
/// free.
pub const SLOT_COVER: usize = 11;

/// Mirrors rox-core's on-disk `MilkdropColor` one for one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GradeMode {
    Preset,
    #[default]
    Theme,
    Palette,
    Cover,
}

/// Colours are in linear light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    pub mode: GradeMode,
    pub light: bool,
    pub bg: [f32; 3],
    pub accent: [f32; 3],
    /// Oklab hue in radians and chroma; zero chroma means no hue to lend.
    pub cover: (f32, f32),
}

impl Grade {
    /// `bg` and `accent` arrive sRGB and go into the slots linear. `cover` is
    /// None while nothing plays or the cover is grey.
    pub fn new(mode: GradeMode, light: bool, bg: Rgba, accent: Rgba, cover: Option<Rgba>) -> Grade {
        Grade {
            mode,
            light,
            bg: linear(bg),
            accent: linear(accent),
            cover: cover_stop(cover.unwrap_or(accent)),
        }
    }

    /// Light is read off the scope's root background rather than the theme
    /// pick, so a light panel in a dark app grades as light.
    pub fn from_scope(mode: GradeMode, cover: Option<Rgba>) -> Grade {
        let bg = palette::bg_root_opaque();
        Grade::new(mode, is_light(bg), bg, palette::accent(), cover)
    }

    /// The palette and cover ramps already chose their colours, so the album
    /// tint doesn't turn them.
    pub fn tints(&self) -> bool {
        !matches!(self.mode, GradeMode::Palette | GradeMode::Cover)
    }

    /// The fade, hue and tint slots are the caller's.
    pub fn write(&self, signals: &mut [f32; 16]) {
        signals[SLOT_MODE] = match self.mode {
            GradeMode::Preset => 0.0,
            GradeMode::Theme => 1.0,
            GradeMode::Palette => 2.0,
            GradeMode::Cover => 3.0,
        };
        signals[SLOT_BG..SLOT_BG + 3].copy_from_slice(&self.bg);
        signals[SLOT_LIGHT] = if self.light { 1.0 } else { 0.0 };
        signals[SLOT_ACCENT..SLOT_ACCENT + 3].copy_from_slice(&self.accent);
        signals[SLOT_COVER] = self.cover.0;
        signals[SLOT_COVER + 1] = self.cover.1;
    }
}

/// So a muted tan still reads as a colour rather than a warm grey.
const COVER_CHROMA_FLOOR: f32 = 0.1;

/// Below this a hue is rounding error: an amber accent stripped to grey comes
/// back at exactly 180 degrees and would paint the frame teal.
const COVER_CHROMA_MIN: f32 = 0.01;

/// Lightness is dropped: the palette already set the accent's distance from
/// the background. Zero chroma tells the shader to run the two-stop ramp.
pub fn cover_stop(cover: Rgba) -> (f32, f32) {
    let (_, chroma, hue) = palette::rgba_to_oklch(cover);
    if chroma < COVER_CHROMA_MIN {
        return (0.0, 0.0);
    }

    (hue, chroma.max(COVER_CHROMA_FLOOR))
}

pub fn is_light(bg: Rgba) -> bool {
    palette::rgba_to_oklch(bg).0 > 0.5
}

fn linear(color: Rgba) -> [f32; 3] {
    let channel = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    [channel(color.r), channel(color.g), channel(color.b)]
}

pub fn wgsl(body: &str) -> String {
    format!("{WGSL}\n{body}")
}

/// The helpers every Milkdrop pass gets in scope; `grade` reads the slots
/// [`Grade::write`] fills. The gamut fit trades chroma away by bisection, and
/// only pixels out of gamut pay for the loop.
pub const WGSL: &str = "
fn linear_to_oklab(rgb: vec3<f32>) -> vec3<f32> {
    let l = dot(rgb, vec3<f32>(0.4122214708, 0.5363325363, 0.0514459929));
    let m = dot(rgb, vec3<f32>(0.2119034982, 0.6806995451, 0.1073969566));
    let s = dot(rgb, vec3<f32>(0.0883024619, 0.2817188376, 0.6299787005));
    let root = pow(max(vec3<f32>(l, m, s), vec3<f32>(0.0)), vec3<f32>(1.0 / 3.0));
    return vec3<f32>(
        dot(root, vec3<f32>(0.2104542553, 0.7936177850, -0.0040720468)),
        dot(root, vec3<f32>(1.9779984951, -2.4285922050, 0.4505937099)),
        dot(root, vec3<f32>(0.0259040371, 0.7827717662, -0.8086757660)),
    );
}

fn oklab_to_linear(lab: vec3<f32>) -> vec3<f32> {
    let l = lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z;
    let m = lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z;
    let s = lab.x - 0.0894841775 * lab.y - 1.2914855480 * lab.z;
    let cubed = vec3<f32>(l * l * l, m * m * m, s * s * s);
    return vec3<f32>(
        dot(cubed, vec3<f32>(4.0767416621, -3.3077115913, 0.2309699292)),
        dot(cubed, vec3<f32>(-1.2684380046, 2.6097574011, -0.3413193965)),
        dot(cubed, vec3<f32>(-0.0041960863, -0.7034186147, 1.7076147010)),
    );
}

fn in_gamut(rgb: vec3<f32>) -> bool {
    return all(rgb >= vec3<f32>(-0.0001)) && all(rgb <= vec3<f32>(1.0001));
}

fn fit_gamut(lab: vec3<f32>) -> vec3<f32> {
    var out = oklab_to_linear(lab);
    if (!in_gamut(out)) {
        let chroma = length(lab.yz);
        let direction = select(vec2<f32>(0.0, 0.0), lab.yz / max(chroma, 0.0001), chroma > 0.0001);
        var lo = 0.0;
        var hi = chroma;
        for (var i = 0; i < 5; i++) {
            let mid = (lo + hi) * 0.5;
            if (in_gamut(oklab_to_linear(vec3<f32>(lab.x, direction * mid)))) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        out = oklab_to_linear(vec3<f32>(lab.x, direction * lo));
    }
    return clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn turn_hue(rgb: vec3<f32>, hue: f32, amount: f32) -> vec3<f32> {
    if (amount <= 0.0) {
        return rgb;
    }
    let lab = linear_to_oklab(rgb);
    let chroma = length(lab.yz);
    if (chroma <= 0.0001) {
        return rgb;
    }
    let start = atan2(lab.z, lab.y);
    // The short way around: fold the difference into a half turn either
    // side, so a red frame against a magenta cover goes the near way and
    // not the long way through green.
    let turned = start + fold_hue(hue - start) * amount;
    return fit_gamut(vec3<f32>(lab.x, vec2<f32>(cos(turned), sin(turned)) * chroma));
}

fn oklch(lightness: f32, chroma: f32, hue: f32) -> vec3<f32> {
    return vec3<f32>(lightness, chroma * cos(hue), chroma * sin(hue));
}

// The nearest way round the hue circle, in the range of a half turn
// either side.
fn fold_hue(apart: f32) -> f32 {
    return apart - 6.28318530718 * round(apart / 6.28318530718);
}

// The cover remap. `hue` and `chroma` are the cover's; the ramp climbs
// from the background to the cover at full chroma halfway up, then on to
// the cover's hue at the accent's lightness, where the gamut fit pales
// it. On top of the ramp the frame's own colour: its chroma where that
// beats the ramp's, and its hue folded into a quarter turn either side
// of the cover's, fading in with the chroma so grey pixels get the ramp
// exactly. Chroma at the fold's edge is the gamut fit's problem.
fn cover_grade(lab: vec3<f32>, floor: vec3<f32>, top_l: f32, hue: f32, chroma: f32) -> vec3<f32> {
    let t = clamp(lab.x, 0.0, 1.0);
    let peak = oklch(mix(floor.x, top_l, 0.5), chroma, hue);
    let top = oklch(top_l, chroma, hue);
    let ramp = select(
        mix(peak, top, t * 2.0 - 1.0),
        mix(floor, peak, t * 2.0),
        t < 0.5,
    );
    let own = length(lab.yz);
    let spread = 0.25 * smoothstep(0.0, 0.05, own);
    let turned = hue + fold_hue(atan2(lab.z, lab.y) - hue) * spread;
    return fit_gamut(oklch(ramp.x, max(length(ramp.yz), own), turned));
}

// Slot 3 is the mode, 4-6 the theme's root background, 7 the light flag,
// 8-10 the accent, 11 and 12 the cover's hue and chroma; 1 and 2 are the
// album tint's hue and amount.
fn grade(rgb: vec3<f32>) -> vec3<f32> {
    let mode = params.signals[0].w;
    let light = params.signals[1].w;
    var out = rgb;
    if (mode >= 1.5) {
        let lab = linear_to_oklab(rgb);
        let floor = linear_to_oklab(params.signals[1].xyz);
        let ceiling = linear_to_oklab(params.signals[2].xyz);
        let chroma = params.signals[3].x;
        // The cover ramp needs a colour to run on. A grey album, or a
        // grey accent standing in for one, arrives at zero chroma with no
        // hue behind it, and the two-stop ramp is the same colours
        // without a hue invented to grade toward.
        if (mode >= 2.5 && chroma > 0.0) {
            out = cover_grade(lab, floor, ceiling.x, params.signals[2].w, chroma);
        } else {
            out = fit_gamut(mix(floor, ceiling, clamp(lab.x, 0.0, 1.0)));
        }
    } else if (mode >= 0.5 && light > 0.5) {
        let lab = linear_to_oklab(rgb);
        out = fit_gamut(vec3<f32>(1.0 - lab.x, lab.yz));
    }
    return turn_hue(out, params.signals[0].y, clamp(params.signals[0].z, 0.0, 1.0));
}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grade_fills_its_own_slots_and_no_others() {
        let grade = Grade::new(
            GradeMode::Palette,
            true,
            gpui::rgb(0xededed),
            gpui::rgb(0xffb300),
            Some(gpui::rgb(0x0000ff)),
        );
        let mut signals = [0.5f32; 16];
        grade.write(&mut signals);
        assert_eq!(signals[SLOT_FADE], 0.5, "the fade is the caller's");
        assert_eq!(signals[SLOT_HUE], 0.5);
        assert_eq!(signals[SLOT_TINT], 0.5);
        assert_eq!(signals[SLOT_MODE], 2.0);
        assert_eq!(signals[SLOT_LIGHT], 1.0);
        assert!(signals[SLOT_BG] > 0.8 && signals[SLOT_BG] < 0.86);
        assert_eq!(signals[SLOT_BG], signals[SLOT_BG + 1]);
        assert!(signals[SLOT_ACCENT] > 0.99, "full red decodes to one");
        assert!(signals[SLOT_ACCENT + 2] < 0.01, "no blue decodes to none");
        let (_, blue_c, blue_h) = palette::rgba_to_oklch(gpui::rgb(0x0000ff));
        assert_eq!(
            signals[SLOT_COVER], blue_h,
            "the cover's hue lands in its slot"
        );
        assert_eq!(signals[SLOT_COVER + 1], blue_c, "then its chroma");
        assert_eq!(signals[13], 0.5, "the slot past the cover is untouched");
        assert_eq!(signals[14], 0.5, "and so are the callers'");
    }

    #[test]
    fn the_theme_mode_is_the_default_and_the_palette_drops_the_tint() {
        assert_eq!(GradeMode::default(), GradeMode::Theme);
        let bg = gpui::rgb(0x121212);
        let accent = gpui::rgb(0xffb300);
        assert!(Grade::new(GradeMode::Theme, false, bg, accent, None).tints());
        assert!(Grade::new(GradeMode::Preset, false, bg, accent, None).tints());
        assert!(!Grade::new(GradeMode::Palette, false, bg, accent, None).tints());
        assert!(!Grade::new(GradeMode::Cover, false, bg, accent, None).tints());
    }

    #[test]
    fn a_dark_cover_keeps_its_hue_and_is_floored() {
        let cover = gpui::rgb(0x3a2410);
        let (cover_l, cover_c, cover_h) = palette::rgba_to_oklch(cover);
        let (got_h, got_c) = cover_stop(cover);
        assert!(cover_l < 0.4, "the cover really is dark: {cover_l}");
        assert!(cover_c < COVER_CHROMA_FLOOR, "and muted: {cover_c}");
        assert_eq!(got_h, cover_h, "hue kept");
        assert_eq!(got_c, COVER_CHROMA_FLOOR, "chroma floored");
    }

    /// The shipped amber stripped to grey points at exactly 180 degrees.
    #[test]
    fn a_neutralised_accent_lends_no_hue() {
        let (lightness, _, hue) = palette::rgba_to_oklch(gpui::rgb(0xffb300));
        let neutral = palette::oklch_to_rgba(lightness, 0.0, hue, 1.0);
        let (_, left, _) = palette::rgba_to_oklch(neutral);
        assert!(
            left < COVER_CHROMA_MIN,
            "premise: it came back grey: {left}"
        );
        assert_eq!(cover_stop(neutral), (0.0, 0.0), "no colour, no stop");

        let grade = Grade::new(GradeMode::Cover, false, gpui::rgb(0x121212), neutral, None);
        let mut signals = [0.0f32; 16];
        grade.write(&mut signals);
        assert_eq!(signals[SLOT_COVER + 1], 0.0, "and none reaches the shader");
    }

    #[test]
    fn a_vivid_cover_keeps_its_chroma() {
        let cover = gpui::rgb(0xff0000);
        let (_, cover_c, cover_h) = palette::rgba_to_oklch(cover);
        assert!(cover_c > COVER_CHROMA_FLOOR);
        assert_eq!(cover_stop(cover), (cover_h, cover_c));
    }

    #[test]
    fn a_missing_cover_falls_back_to_the_accent() {
        let grade = Grade::new(
            GradeMode::Cover,
            false,
            gpui::rgb(0x121212),
            gpui::rgb(0xffb300),
            None,
        );
        assert_eq!(grade.cover, cover_stop(gpui::rgb(0xffb300)));
    }

    #[test]
    fn the_two_shipped_backgrounds_land_either_side_of_light() {
        assert!(!is_light(gpui::rgb(0x121212)));
        assert!(is_light(gpui::rgb(0xededed)));
    }
}
