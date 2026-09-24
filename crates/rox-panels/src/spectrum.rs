//! The spectrum panel: live frequency bars over the player's PCM tap, the
//! classic analyzer look: log-spaced bands, snappy attack, eased decay,
//! peak-hold caps falling under gravity. One FFT per frame while audio
//! flows, and the panel parks once the bars settle, so an idle app pays
//! nothing.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    AnyElement, App, BorderStyle, Bounds, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, Path, Rgba, SharedString, Subscription, WeakEntity, Window, canvas, div, fill,
    linear_color_stop, linear_gradient, point, prelude::*, px, relative, size,
};
use gpui_component::Sizable as _;
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_panel_kit::axis::{fmt_axis_hz, fmt_hz};
use serde::{Deserialize, Serialize};

use rox_viz::AudioFeed;
use rox_viz::analysis::{MAX_FFT_SIZE, MIN_FFT_SIZE, hz_ladder, log_bands};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, AppState, PanelChrome, PanelSettings, ScrubState, choices, choices_shared, setting_row,
    toggle,
};
use crate::panel_settings;
use crate::settings::ui as settings_ui;

// The count collapses on narrow panels instead of thinning the bars, so
// a small dock split doesn't smear.
const MIN_BARS: usize = 16;
const MAX_BARS: usize = 512;

/// Under the dock's 40px default, so a layout can run the bands as a thin
/// strip. The width keeps the floor so the band count stays readable.
const MIN_HEIGHT: gpui::Pixels = gpui::px(16.);

const BAR_W_MIN: f32 = 1.0;
const BAR_W_MAX: f32 = 12.0;

const BAR_GAP_MIN: f32 = 0.0;
const BAR_GAP_MAX: f32 = 8.0;

/// A stroke past half the bar width reads as a filled bar again.
const OUTLINE_W_MIN: f32 = 1.0;
const OUTLINE_W_MAX: f32 = 4.0;

const BLOCK_H_MIN: f32 = 2.0;
const BLOCK_H_MAX: f32 = 12.0;
const BLOCK_GAP_MIN: f32 = 0.0;
const BLOCK_GAP_MAX: f32 = 4.0;

const LINE_W: f32 = 1.5;

/// Roughly the audible range up to a typical Nyquist ceiling.
const SLIDER_MIN_HZ: f32 = 20.0;
const SLIDER_MAX_HZ: f32 = 20_000.0;

const MIN_RATIO: f32 = 2.0;

const C0_HZ: f32 = 16.352;

/// A full-scale sine is 0 dB. The top leaves headroom so a busy mix pins
/// near full height without every band clipping.
const FLOOR_DB: f32 = -66.0;
const MAX_DB: f32 = -12.0;

/// Up fast, down slowly, so kicks read as kicks instead of flicker.
const ATTACK: f32 = 40.0;
const RELEASE: f32 = 10.0;

/// Bar heights per second squared.
const HOLD_GRAVITY: f32 = 0.05;

const GRAVITY_MIN: f32 = 0.01;
const GRAVITY_MAX: f32 = 1.0;

const FFT_CHOICES: &[(&str, usize)] = &[
    ("512", 512),
    ("1k", 1024),
    ("2k", 2048),
    ("4k", 4096),
    ("8k", 8192),
    ("16k", 16384),
];

const DB_MARKS: [f32; 3] = [-20.0, -40.0, -60.0];

const EPSILON: f32 = 0.002;

/// How long the feed may sit still before it reads as stopped rather than
/// a gap between pump ticks (the tap drains on a ~16ms timer). Dipping
/// between ticks reads as shimmer on high-refresh displays and a strobe
/// under load.
const SILENT_AFTER: f32 = 0.15;

/// Blocks is the Winamp look, Line the Fruity EQ look.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpectrumStyle {
    #[default]
    Bars,
    Blocks,
    Line,
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

impl Orientation {
    pub fn horizontal(self) -> bool {
        matches!(self, Orientation::Bottom | Orientation::Top)
    }

    pub fn tip_angle(self) -> f32 {
        match self {
            Orientation::Bottom => 0.0,
            Orientation::Top => 180.0,
            Orientation::Left => 90.0,
            Orientation::Right => 270.0,
        }
    }
}

/// The cover ramp uses the art's two extracted colors while song theming
/// derives, and the plain palette otherwise.
#[derive(Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Gradient {
    #[default]
    Off,
    Theme,
    Cover,
    Custom,
}

impl<'de> Deserialize<'de> for Gradient {
    /// By hand for layouts where `gradient` was a bool (true was the theme
    /// ramp). An unknown name reads as flat rather than failing the config.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Legacy(bool),
            Named(String),
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Legacy(false) => Gradient::Off,
            Raw::Legacy(true) => Gradient::Theme,
            Raw::Named(name) => match name.as_str() {
                "theme" => Gradient::Theme,
                "cover" => Gradient::Cover,
                "custom" => Gradient::Custom,
                _ => Gradient::Off,
            },
        })
    }
}

/// One choice rather than two overlays, since both rule the same dividers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Labels {
    #[default]
    Off,
    Pitch,
    Freq,
}

impl<'de> Deserialize<'de> for Labels {
    /// By hand for layouts where `labels` was a bool (true was the octave
    /// marks). An unknown name reads as off.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Legacy(bool),
            Named(String),
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Legacy(false) => Labels::Off,
            Raw::Legacy(true) => Labels::Pitch,
            Raw::Named(name) => match name.as_str() {
                "pitch" => Labels::Pitch,
                "freq" => Labels::Freq,
                _ => Labels::Off,
            },
        })
    }
}

/// Forward puts the lows at the outer edges; reverse meets them at the
/// middle.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Symmetry {
    #[default]
    None,
    Forward,
    Reverse,
}

impl Symmetry {
    fn mirrored(self) -> bool {
        self != Symmetry::None
    }

    fn reversed(self) -> bool {
        self == Symmetry::Reverse
    }
}

fn style_choices() -> [(SharedString, SpectrumStyle); 3] {
    [
        (rox_i18n::t!("spectrum-style-bars"), SpectrumStyle::Bars),
        (rox_i18n::t!("spectrum-style-blocks"), SpectrumStyle::Blocks),
        (rox_i18n::t!("spectrum-style-line"), SpectrumStyle::Line),
    ]
}

/// Shared with the VU meter panel.
pub fn orientation_choices() -> [(SharedString, Orientation); 4] {
    [
        (rox_i18n::t!("valign-bottom"), Orientation::Bottom),
        (rox_i18n::t!("valign-top"), Orientation::Top),
        (rox_i18n::t!("side-left"), Orientation::Left),
        (rox_i18n::t!("side-right"), Orientation::Right),
    ]
}

fn label_choices() -> [(SharedString, Labels); 3] {
    [
        (rox_i18n::t!("panel-size-off"), Labels::Off),
        (rox_i18n::t!("spectrum-labels-pitch"), Labels::Pitch),
        (rox_i18n::t!("spectrum-labels-frequency"), Labels::Freq),
    ]
}

fn symmetry_choices() -> [(SharedString, Symmetry); 3] {
    [
        (rox_i18n::t!("shader-pick-none"), Symmetry::None),
        (rox_i18n::t!("spectrum-symmetry-forward"), Symmetry::Forward),
        (rox_i18n::t!("spectrum-symmetry-reverse"), Symmetry::Reverse),
    ]
}

/// Shared with the VU meter panel.
pub fn gradient_choices() -> [(SharedString, Gradient); 4] {
    [
        (rox_i18n::t!("panel-size-off"), Gradient::Off),
        (rox_i18n::t!("spectrum-gradient-theme"), Gradient::Theme),
        (rox_i18n::t!("spectrum-gradient-cover"), Gradient::Cover),
        (rox_i18n::t!("shader-pick-custom"), Gradient::Custom),
    ]
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrumConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub style: SpectrumStyle,
    pub orientation: Orientation,
    /// Forward runs the lows to the outer edges, reverse meets them at the
    /// middle.
    pub symmetry: Symmetry,
    pub freq_lo: f32,
    /// Hz. Capping below Nyquist drops the near-silent top octaves.
    pub freq_hi: f32,
    pub bar_width: f32,
    /// Px; also sets the bar count, so a wider gap fits fewer bars.
    pub bar_gap: f32,
    pub block_height: f32,
    pub block_gap: f32,
    /// With split zoning on, this covers the bands below `split_hz`.
    pub fft_size: usize,
    /// Analyze each side of `split_hz` at its own window size.
    pub split: bool,
    /// Hz, snapped to the nearest bar edge so the log spacing runs unbroken.
    pub split_hz: f32,
    pub fft_size_hi: usize,
    pub gradient: Gradient,
    pub gradient_lo: String,
    pub gradient_hi: String,
    pub outline: bool,
    pub outline_width: f32,
    pub caps: bool,
    /// Freeze the bars while paused instead of letting them fall.
    pub freeze: bool,
    pub cap_gravity: f32,
    pub labels: Labels,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        SpectrumConfig {
            chrome: PanelChrome::default(),
            style: SpectrumStyle::default(),
            orientation: Orientation::default(),
            symmetry: Symmetry::default(),
            freq_lo: 30.0,
            freq_hi: 16_000.0,
            bar_width: tokens::BAR_W,
            bar_gap: tokens::BAR_GAP,
            block_height: 3.0,
            block_gap: 1.0,
            fft_size: 8192,
            split: false,
            split_hz: 1_000.0,
            fft_size_hi: MAX_FFT_SIZE,
            gradient: Gradient::default(),
            gradient_lo: "#33aacc".into(),
            gradient_hi: "#cc5588".into(),
            outline: false,
            outline_width: 1.0,
            caps: true,
            freeze: true,
            cap_gravity: HOLD_GRAVITY,
            labels: Labels::default(),
        }
    }
}

impl SpectrumConfig {
    fn range(&self) -> (f32, f32) {
        let lo = self.freq_lo.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
        let hi = self
            .freq_hi
            .clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ)
            .max(lo * MIN_RATIO)
            .min(SLIDER_MAX_HZ);
        (lo.min(hi / MIN_RATIO), hi)
    }

    /// The px knobs read back to the typed ceiling rather than the strip's
    /// top, so a value typed past the top survives a reload.
    fn bar_w(&self) -> f32 {
        self.bar_width
            .clamp(BAR_W_MIN, settings_ui::ceiling(BAR_W_MIN, BAR_W_MAX))
    }

    fn bar_gap(&self) -> f32 {
        self.bar_gap
            .clamp(BAR_GAP_MIN, settings_ui::ceiling(BAR_GAP_MIN, BAR_GAP_MAX))
    }

    fn outline_w(&self) -> f32 {
        self.outline_width.clamp(
            OUTLINE_W_MIN,
            settings_ui::ceiling(OUTLINE_W_MIN, OUTLINE_W_MAX),
        )
    }

    fn block_h(&self) -> f32 {
        self.block_height
            .clamp(BLOCK_H_MIN, settings_ui::ceiling(BLOCK_H_MIN, BLOCK_H_MAX))
    }

    fn custom_ramp(&self) -> (Rgba, Rgba) {
        (
            palette::parse_hex(&self.gradient_lo)
                .unwrap_or_else(|| palette::alpha(palette::text_faint(), 0x66)),
            palette::parse_hex(&self.gradient_hi).unwrap_or_else(palette::accent),
        )
    }

    fn block_gap(&self) -> f32 {
        self.block_gap.clamp(
            BLOCK_GAP_MIN,
            settings_ui::ceiling(BLOCK_GAP_MIN, BLOCK_GAP_MAX),
        )
    }

    fn gravity(&self) -> f32 {
        self.cap_gravity.clamp(GRAVITY_MIN, GRAVITY_MAX)
    }

    /// Clamp before rounding: `next_power_of_two` overflows past the top
    /// power of two (a panic in debug, zero in release), and a layout file can
    /// hold any number.
    fn fft_lo(&self) -> usize {
        self.fft_size
            .clamp(MIN_FFT_SIZE, MAX_FFT_SIZE)
            .next_power_of_two()
    }

    fn fft_hi(&self) -> usize {
        self.fft_size_hi
            .clamp(MIN_FFT_SIZE, MAX_FFT_SIZE)
            .next_power_of_two()
    }
}

fn frac_to_hz(fraction: f32) -> f32 {
    SLIDER_MIN_HZ * (SLIDER_MAX_HZ / SLIDER_MIN_HZ).powf(fraction.clamp(0.0, 1.0))
}

fn hz_to_frac(hz: f32) -> f32 {
    (hz / SLIDER_MIN_HZ).ln() / (SLIDER_MAX_HZ / SLIDER_MIN_HZ).ln()
}

/// Unsplit runs one zone over everything, split runs two. The transform
/// is the feed's, shared with every view at the same size.
struct Zone {
    size: usize,
    bands: Vec<(usize, usize)>,
}

#[derive(PartialEq)]
struct Mapping {
    count: usize,
    rate: u32,
    freq_lo: f32,
    freq_hi: f32,
    fft_lo: usize,
    fft_hi: usize,
    split_hz: f32,
}

impl Mapping {
    /// The split snaps to the bar edge nearest the split frequency; a split
    /// outside the range leaves one zone.
    fn zones(&self) -> Vec<Zone> {
        let zone = |bars: usize, size: usize, lo: f32, hi: f32| Zone {
            size,
            bands: log_bands(bars, lo, hi, self.rate, size / 2),
        };
        if self.split_hz <= 0.0 {
            return vec![zone(self.count, self.fft_lo, self.freq_lo, self.freq_hi)];
        }
        let span = (self.freq_hi / self.freq_lo).ln();
        let frac = (self.split_hz / self.freq_lo).ln() / span;
        let split_bar = ((frac * self.count as f32).round()).clamp(0.0, self.count as f32) as usize;
        if split_bar == 0 {
            return vec![zone(self.count, self.fft_hi, self.freq_lo, self.freq_hi)];
        }
        if split_bar == self.count {
            return vec![zone(self.count, self.fft_lo, self.freq_lo, self.freq_hi)];
        }
        let edge =
            self.freq_lo * (self.freq_hi / self.freq_lo).powf(split_bar as f32 / self.count as f32);
        vec![
            zone(split_bar, self.fft_lo, self.freq_lo, edge),
            zone(self.count - split_bar, self.fft_hi, edge, self.freq_hi),
        ]
    }
}

struct Bars {
    last_written: u64,
    last_tick: Option<Instant>,
    mapping: Option<Mapping>,
    zones: Vec<Zone>,
    /// Held between analyses, zeroed once the feed reads as stopped.
    targets: Vec<f32>,
    last_fresh: Option<Instant>,
    levels: Vec<f32>,
    holds: Vec<f32>,
    hold_vel: Vec<f32>,
    alive: bool,
}

impl Bars {
    fn new() -> Self {
        Bars {
            last_written: 0,
            last_tick: None,
            mapping: None,
            zones: Vec::new(),
            targets: Vec::new(),
            last_fresh: None,
            levels: Vec::new(),
            holds: Vec::new(),
            hold_vel: Vec::new(),
            alive: false,
        }
    }

    /// `axis` is the length the bands lay along, halved when mirrored.
    fn step(&mut self, feed: &AudioFeed, axis: f32, config: &SpectrumConfig, hold: bool) {
        let (freq_lo, freq_hi) = config.range();
        let gravity = config.gravity();
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32().min(0.1))
            .unwrap_or(1.0 / 60.0);
        self.last_tick = Some(now);

        let written = feed.written();
        let fresh = written != self.last_written;
        self.last_written = written;

        let count =
            ((axis / (config.bar_w() + config.bar_gap())) as usize).clamp(MIN_BARS, MAX_BARS);
        let mapping = Mapping {
            count,
            rate: feed.sample_rate(),
            freq_lo,
            freq_hi,
            fft_lo: config.fft_lo(),
            fft_hi: config.fft_hi(),
            split_hz: if config.split {
                config.split_hz.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ)
            } else {
                0.0
            },
        };
        let remap = self.mapping.as_ref() != Some(&mapping);
        if remap {
            self.zones = mapping.zones();
            self.mapping = Some(mapping);
            self.targets = vec![0.0; count];
            self.levels = vec![0.0; count];
            self.holds = vec![0.0; count];
            self.hold_vel = vec![0.0; count];
        }

        // Frozen: keep the frame. A remap still re-analyzes below, off the last
        // window the feed keeps, so a settings edit shows while paused.
        if hold && !fresh && !remap {
            self.alive = false;
            return;
        }

        // Nothing new: hold the targets across the pump-tick gap until the feed
        // reads as stopped. A remap re-analyzes the buffered window right away.
        if fresh {
            self.last_fresh = Some(now);
        }
        let stopped = self
            .last_fresh
            .is_none_or(|t| (now - t).as_secs_f32() > SILENT_AFTER);

        let mut alive = false;
        let mut bar = 0;
        for zone in &self.zones {
            let mags = if fresh || remap {
                feed.magnitudes(zone.size)
            } else {
                None
            };
            for &(lo, hi) in zone.bands.iter() {
                let i = bar;
                bar += 1;
                if let Some(mags) = &mags {
                    let mut peak = 0.0f32;
                    for &m in &mags[lo..hi] {
                        peak = peak.max(m);
                    }
                    let db = 20.0 * (peak + 1e-9).log10();
                    self.targets[i] = ((db - FLOOR_DB) / (MAX_DB - FLOOR_DB)).clamp(0.0, 1.0);
                } else if stopped {
                    self.targets[i] = 0.0;
                }
                let target = self.targets[i];
                if hold {
                    // Frozen: jump straight to the target, since the next tick parks again.
                    self.levels[i] = target;
                } else {
                    let rate = if target > self.levels[i] {
                        ATTACK
                    } else {
                        RELEASE
                    };
                    self.levels[i] += (target - self.levels[i]) * (rate * dt).min(1.0);
                }

                // Caps off: the holds track the bars so they don't keep the panel
                // animating.
                if !config.caps || self.levels[i] >= self.holds[i] {
                    self.holds[i] = self.levels[i];
                    self.hold_vel[i] = 0.0;
                } else {
                    self.hold_vel[i] += gravity * dt;
                    self.holds[i] = (self.holds[i] - self.hold_vel[i] * dt).max(self.levels[i]);
                }
                if self.levels[i] > EPSILON || self.holds[i] > EPSILON {
                    alive = true;
                }
            }
        }
        self.alive = alive;
    }

    fn paint(&self, bounds: Bounds<gpui::Pixels>, window: &mut Window, config: &SpectrumConfig) {
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        let count = self.levels.len();
        if count == 0 || w <= 0.0 || h <= 0.0 {
            return;
        }

        // Symmetric panels lay the bands into half the axis and paint each twice,
        // the second half reflected.
        let orientation = config.orientation;
        let mirror = config.symmetry.mirrored();
        let reversed = config.symmetry.reversed();
        let (axis, depth) = if orientation.horizontal() {
            (w, h)
        } else {
            (h, w)
        };
        let half = if mirror { axis / 2.0 } else { axis };
        let max_d = depth * 0.94;
        let step = half / count as f32;
        let bar_w = (step - config.bar_gap()).max(1.0);

        let origin = bounds.origin;
        let rect = move |a: f32, aw: f32, d: f32, dw: f32| {
            let (x, y, rw, rh) = match orientation {
                Orientation::Bottom => (a, h - d - dw, aw, dw),
                Orientation::Top => (a, d, aw, dw),
                Orientation::Left => (d, h - a - aw, dw, aw),
                Orientation::Right => (w - d - dw, h - a - aw, dw, aw),
            };
            Bounds::new(
                point(origin.x + px(x), origin.y + px(y)),
                size(px(rw), px(rh)),
            )
        };

        for db in DB_MARKS {
            let d = (db - FLOOR_DB) / (MAX_DB - FLOOR_DB) * max_d;
            window.paint_quad(fill(
                rect(0.0, axis, d, 1.0),
                palette::alpha(palette::gridline(), 0x28),
            ));
        }

        let block_h = config.block_h();
        let cell = block_h + config.block_gap();
        let cells = ((max_d / cell) as usize).max(1);

        if config.style == SpectrumStyle::Line {
            self.paint_line(bounds, window, config, axis, half, step, max_d);
        } else {
            for i in 0..count {
                let level = self.levels[i];
                let a0 = i as f32 * step;
                let a0 = if reversed { half - a0 - bar_w } else { a0 };
                let slots = [a0, axis - a0 - bar_w];
                let slots = if mirror { &slots[..] } else { &slots[..1] };
                let base = bar_color(config, level);
                for &a in slots {
                    if config.style == SpectrumStyle::Blocks {
                        // Each cell colored by its own height, so only a tall stack's top runs
                        // hot.
                        let lit = (level * cells as f32).round() as usize;
                        for c in 0..lit {
                            let color = bar_color(config, (c as f32 + 0.5) / cells as f32);
                            window
                                .paint_quad(fill(rect(a, bar_w, c as f32 * cell, block_h), color));
                        }
                        // A ghosted base cell keeps a silent band's footprint.
                        if lit == 0 {
                            window.paint_quad(fill(
                                rect(a, bar_w, 0.0, block_h),
                                palette::alpha(base, 0x40),
                            ));
                        }
                        continue;
                    }
                    let bar = rect(a, bar_w, 0.0, (level * max_d).max(2.0));
                    if config.outline {
                        window.paint_quad(gpui::quad(
                            bar,
                            0.,
                            gpui::transparent_black(),
                            config.outline_w(),
                            base,
                            BorderStyle::default(),
                        ));
                    } else {
                        window.paint_quad(fill(
                            bar,
                            linear_gradient(
                                orientation.tip_angle(),
                                linear_color_stop(base, 0.0),
                                linear_color_stop(palette::alpha(base, 0x40), 1.0),
                            ),
                        ));
                    }
                }
            }
        }

        if !config.caps {
            return;
        }
        // The highlight, like the playheads, so the caps stay legible over
        // accent-colored bars.
        for i in 0..count {
            let a0 = i as f32 * step;
            let a0 = if reversed { half - a0 - bar_w } else { a0 };
            let slots = [a0, axis - a0 - bar_w];
            let slots = if mirror { &slots[..] } else { &slots[..1] };
            for &a in slots {
                let cap = if config.style == SpectrumStyle::Blocks {
                    let c = ((self.holds[i] * cells as f32).ceil() as usize)
                        .saturating_sub(1)
                        .min(cells - 1);
                    rect(a, bar_w, c as f32 * cell, block_h)
                } else {
                    rect(a, bar_w, (self.holds[i] * max_d).min(depth - 1.0), 1.0)
                };
                window.paint_quad(fill(cap, palette::highlight()));
            }
        }
    }

    /// One path is one fill, so the ramp runs base to tip rather than per band.
    #[allow(clippy::too_many_arguments)]
    fn paint_line(
        &self,
        bounds: Bounds<gpui::Pixels>,
        window: &mut Window,
        config: &SpectrumConfig,
        axis: f32,
        half: f32,
        step: f32,
        max_d: f32,
    ) {
        let count = self.levels.len();
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        let orientation = config.orientation;
        let origin = bounds.origin;
        let at = move |a: f32, d: f32| {
            let (x, y) = match orientation {
                Orientation::Bottom => (a, h - d),
                Orientation::Top => (a, d),
                Orientation::Left => (d, h - a),
                Orientation::Right => (w - d, h - a),
            };
            point(origin.x + px(x), origin.y + px(y))
        };

        // Pinned to the half's edges, so mirrored halves meet without a seam.
        let mut tips = Vec::with_capacity(count + 2);
        tips.push((0.0, (self.levels[0] * max_d).max(2.0)));
        for (i, &level) in self.levels.iter().enumerate() {
            tips.push((i as f32 * step + step / 2.0, (level * max_d).max(2.0)));
        }
        tips.push((half, (self.levels[count - 1] * max_d).max(2.0)));

        let ramp = match config.gradient {
            Gradient::Off => None,
            Gradient::Theme => Some((
                palette::alpha(palette::text_faint(), 0x66),
                palette::accent(),
            )),
            Gradient::Cover => Some((
                palette::accent(),
                palette::mix(palette::accent(), palette::highlight(), 0.85),
            )),
            Gradient::Custom => Some(config.custom_ramp()),
        };

        let reversed = config.symmetry.reversed();
        let halves: &[bool] = if config.symmetry.mirrored() {
            &[false, true]
        } else {
            &[false]
        };
        for &reflect in halves {
            let pos = move |a: f32| {
                let a = if reversed { half - a } else { a };
                if reflect { axis - a } else { a }
            };
            let solid = (point(0., 1.), point(0., 1.), point(0., 1.));
            let mut area = Path::new(at(pos(tips[0].0), 0.0));
            let mut stroke = Path::new(at(pos(tips[0].0), tips[0].1));
            for pair in tips.windows(2) {
                let (a0, d0) = pair[0];
                let (a1, d1) = pair[1];
                area.push_triangle((at(pos(a0), d0), at(pos(a1), d1), at(pos(a1), 0.0)), solid);
                area.push_triangle((at(pos(a0), d0), at(pos(a1), 0.0), at(pos(a0), 0.0)), solid);
                let (u0, u1) = ((d0 - LINE_W).max(0.0), (d1 - LINE_W).max(0.0));
                stroke.push_triangle((at(pos(a0), d0), at(pos(a1), d1), at(pos(a1), u1)), solid);
                stroke.push_triangle((at(pos(a0), d0), at(pos(a1), u1), at(pos(a0), u0)), solid);
            }
            if let Some((lo, hi)) = ramp {
                let angle = orientation.tip_angle();
                window.paint_path(
                    area,
                    linear_gradient(
                        angle,
                        linear_color_stop(palette::alpha(lo, 0x22), 0.0),
                        linear_color_stop(palette::alpha(hi, 0x40), 1.0),
                    ),
                );
                window.paint_path(
                    stroke,
                    linear_gradient(
                        angle,
                        linear_color_stop(lo, 0.0),
                        linear_color_stop(hi, 1.0),
                    ),
                );
            } else {
                window.paint_path(area, palette::alpha(palette::accent(), 0x33));
                window.paint_path(stroke, palette::accent());
            }
        }
    }
}

fn bar_color(config: &SpectrumConfig, t: f32) -> Rgba {
    ramp_color(config.gradient, t, config.custom_ramp())
}

/// Curved so the mids stay muted and only the top lights up. The cover
/// ramp stops short of full highlight so the caps stay legible on a pinned
/// band.
pub fn ramp_color(gradient: Gradient, t: f32, custom: (Rgba, Rgba)) -> Rgba {
    let t = t.clamp(0.0, 1.0).powf(1.5);
    match gradient {
        Gradient::Off => palette::accent(),
        Gradient::Theme => palette::mix(
            palette::alpha(palette::text_faint(), 0x66),
            palette::accent(),
            t,
        ),
        Gradient::Cover => palette::mix(palette::accent(), palette::highlight(), 0.85 * t),
        Gradient::Custom => {
            let (lo, hi) = custom;
            palette::mix(lo, hi, t)
        }
    }
}

fn axis_rule(orientation: Orientation, frac: f32, color: Rgba) -> Div {
    let rule = div().absolute().border_color(color);
    if orientation.horizontal() {
        rule.top_0().bottom_0().left(relative(frac)).border_l_1()
    } else {
        rule.left_0().right_0().bottom(relative(frac)).border_b_1()
    }
}

/// Under symmetry, two spots: forward outside-in, reverse inside-out.
fn axis_fracs(symmetry: Symmetry, frac: f32) -> Vec<f32> {
    if !symmetry.mirrored() {
        return vec![frac];
    }
    let frac = if symmetry.reversed() {
        1.0 - frac
    } else {
        frac
    };
    vec![frac / 2.0, 1.0 - frac / 2.0]
}

fn scale_marks(config: &SpectrumConfig) -> Vec<(f32, String)> {
    let (freq_lo, freq_hi) = config.range();
    match config.labels {
        Labels::Off => Vec::new(),
        Labels::Pitch => {
            let span = (freq_hi / freq_lo).ln();
            (0..=10)
                .map(|octave| (C0_HZ * 2f32.powi(octave), format!("C{octave}")))
                .filter(|(freq, _)| (freq_lo..=freq_hi).contains(freq))
                .map(|(freq, label)| ((freq / freq_lo).ln() / span, label))
                .collect()
        }
        Labels::Freq => hz_ladder(freq_lo, freq_hi)
            .into_iter()
            .filter(|(_, _, major)| *major)
            .map(|(hz, frac, _)| (frac, fmt_axis_hz(hz)))
            .collect(),
    }
}

/// Symmetric panels label only the first half: the reflected one reads
/// backwards.
fn labels_overlay(config: &SpectrumConfig) -> Div {
    let mut overlay = div().absolute().inset_0();
    for (frac, label) in scale_marks(config) {
        let fracs = axis_fracs(config.symmetry, frac);
        // A label at the far end would clip; keep only its divider.
        let labeled = fracs.len() > 1 || frac <= 0.97;
        overlay = overlay.child(axis_mark(
            config.orientation,
            fracs[0],
            labeled.then_some(label),
        ));
        if let Some(&mirrored) = fracs.get(1) {
            overlay = overlay.child(axis_mark(config.orientation, mirrored, None));
        }
    }
    overlay
}

fn axis_mark(orientation: Orientation, frac: f32, label: Option<String>) -> Div {
    let mark = axis_rule(orientation, frac, palette::alpha(palette::gridline(), 0x1f));
    let Some(text) = label else {
        return mark;
    };
    let label = div()
        .text_xs()
        .text_color(palette::text_faint())
        .whitespace_nowrap()
        .child(text);
    match orientation {
        Orientation::Bottom => mark
            .flex()
            .flex_col()
            .justify_end()
            .child(label.pl(px(3.)).pb(px(2.))),
        Orientation::Top => mark
            .flex()
            .flex_col()
            .justify_start()
            .child(label.pl(px(3.)).pt(px(2.))),
        Orientation::Left => mark
            .flex()
            .justify_start()
            .child(label.pl(px(3.)).pb(px(2.))),
        Orientation::Right => mark.flex().justify_end().child(label.pr(px(3.)).pb(px(2.))),
    }
}

/// Under this the two numbers would overlap, so only the low bound (with
/// the name) prints.
const BAND_LABEL_GAP: f32 = 0.08;

/// Both labels hang inside the band and use the edge the scale's numbers
/// leave alone. `strong` brightens the band whose bound is being dragged.
/// A bound outside the analyzed range draws nothing.
pub fn band_overlay(
    config: &SpectrumConfig,
    lo: f32,
    hi: f32,
    label: Option<String>,
    strong: bool,
) -> Div {
    let (freq_lo, freq_hi) = config.range();
    let span = (freq_hi / freq_lo).ln();
    let color = palette::alpha(palette::highlight(), if strong { 0xe6 } else { 0x8c });
    let frac = |hz: f32| (hz / freq_lo).ln() / span;
    let (frac_lo, frac_hi) = (frac(lo), frac(hi));
    // In the slider's wording, so the mark and its row read the same.
    let low = match &label {
        Some(name) => format!("{name}, {}", fmt_hz(lo)),
        None => fmt_hz(lo),
    };
    let bounds = [
        (frac_lo, Some(low), false),
        (
            frac_hi,
            (frac_hi - frac_lo >= BAND_LABEL_GAP).then(|| fmt_hz(hi)),
            true,
        ),
    ];
    let mut overlay = div().absolute().inset_0();
    for (frac, text, far) in bounds {
        if !(0.0..=1.0).contains(&frac) {
            continue;
        }
        // Text only on the first of a mirrored pair.
        for (i, frac) in axis_fracs(config.symmetry, frac).into_iter().enumerate() {
            overlay = overlay.child(match text.clone().filter(|_| i == 0) {
                Some(text) => band_mark(config.orientation, frac, &text, color, far),
                None => axis_rule(config.orientation, frac, color),
            });
        }
    }
    overlay
}

/// `far` anchors from the other end of the axis, hanging the text on the
/// other side of the line so the pair brackets the band.
fn band_mark(orientation: Orientation, frac: f32, text: &str, color: Rgba, far: bool) -> Div {
    let label = div()
        .text_xs()
        .text_color(color)
        .whitespace_nowrap()
        .child(text.to_string());
    let rule = div().absolute().border_color(color);
    if orientation.horizontal() {
        let rule = if far {
            rule.top_0()
                .bottom_0()
                .right(relative(1.0 - frac))
                .border_r_1()
        } else {
            rule.top_0().bottom_0().left(relative(frac)).border_l_1()
        };
        // Along the top edge, since the scale's numbers run along the base.
        rule.flex().flex_col().justify_start().child(if far {
            label.pr(px(3.)).pt(px(2.))
        } else {
            label.pl(px(3.)).pt(px(2.))
        })
    } else {
        let rule = if far {
            rule.left_0()
                .right_0()
                .top(relative(1.0 - frac))
                .border_t_1()
        } else {
            rule.left_0().right_0().bottom(relative(frac)).border_b_1()
        };
        rule.flex().justify_end().child(label.pr(px(3.)).py(px(2.)))
    }
}

pub struct SpectrumPanel {
    state: AppState,
    config: SpectrumConfig,
    feed: Arc<AudioFeed>,
    bars: Arc<Mutex<Bars>>,
    lo_scrub: ScrubState,
    hi_scrub: ScrubState,
    bar_w_scrub: ScrubState,
    bar_gap_scrub: ScrubState,
    block_h_scrub: ScrubState,
    block_gap_scrub: ScrubState,
    outline_w_scrub: ScrubState,
    gravity_scrub: ScrubState,
    split_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    /// Built on the first settings render: the picker state needs a window.
    ramp_pickers: Option<[Entity<ColorPickerState>; 2]>,
    _ramp_changes: Vec<Subscription>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes an idle window when a session starts.
    _player_changed: Subscription,
}

impl SpectrumPanel {
    pub fn new(state: AppState, config: SpectrumConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        SpectrumPanel {
            config,
            feed: state.player.read(cx).feed(),
            state,
            bars: Arc::new(Mutex::new(Bars::new())),
            lo_scrub: ScrubState::default(),
            hi_scrub: ScrubState::default(),
            bar_w_scrub: ScrubState::default(),
            bar_gap_scrub: ScrubState::default(),
            block_h_scrub: ScrubState::default(),
            block_gap_scrub: ScrubState::default(),
            outline_w_scrub: ScrubState::default(),
            gravity_scrub: ScrubState::default(),
            split_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            ramp_pickers: None,
            _ramp_changes: Vec::new(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
        }
    }

    fn set_freq_lo(&mut self, fraction: f32, cx: &mut Context<Self>) {
        // Stops a min-span short of the high bound so the range never inverts,
        // floored at the slider minimum for a hand-edited tiny high bound.
        let hi = self.config.freq_hi.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
        let ceil = (hi / MIN_RATIO).max(SLIDER_MIN_HZ);
        self.config.freq_lo = frac_to_hz(fraction).clamp(SLIDER_MIN_HZ, ceil);
        cx.notify();
    }

    fn set_freq_hi(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let lo = self.config.freq_lo.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
        let floor = (lo * MIN_RATIO).min(SLIDER_MAX_HZ);
        self.config.freq_hi = frac_to_hz(fraction).clamp(floor, SLIDER_MAX_HZ);
        cx.notify();
    }

    fn set_bar_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.config.bar_width = width;
        cx.notify();
    }

    fn set_bar_gap(&mut self, gap: f32, cx: &mut Context<Self>) {
        self.config.bar_gap = gap;
        cx.notify();
    }

    fn set_block_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.config.block_height = height;
        cx.notify();
    }

    fn set_block_gap(&mut self, gap: f32, cx: &mut Context<Self>) {
        self.config.block_gap = gap;
        cx.notify();
    }

    fn set_outline_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.config.outline_width = width;
        cx.notify();
    }

    fn set_split_hz(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.split_hz = frac_to_hz(fraction).clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
        cx.notify();
    }

    fn set_gravity(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.cap_gravity =
            GRAVITY_MIN * (GRAVITY_MAX / GRAVITY_MIN).powf(fraction.clamp(0.0, 1.0));
        cx.notify();
    }

    /// The readout switches to kHz, but the input is always plain Hz.
    fn freq_slider(
        &self,
        scrub: &ScrubState,
        hz: f32,
        apply: fn(&mut Self, f32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        panel::value_slider_edit(
            scrub,
            &self.value_edit,
            hz_to_frac(hz),
            fmt_hz(hz),
            format!("{hz:.0}"),
            hz_to_frac,
            apply,
            cx,
        )
    }
}

impl PanelSettings for SpectrumPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Layout", icons::ALIGN_LEFT)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.config.gradient == Gradient::Custom && self.ramp_pickers.is_none() {
            let (lo, hi) = self.config.custom_ramp();
            let mut build = |seed: Rgba, write: fn(&mut Self, Rgba)| {
                let picker = cx.new(|cx| ColorPickerState::new(window, cx).default_value(seed));
                let sub = cx.subscribe_in(
                    &picker,
                    window,
                    move |this, _, event: &ColorPickerEvent, _, cx| {
                        let ColorPickerEvent::Change(color) = event;
                        if let Some(color) = color {
                            write(this, Rgba::from(*color));
                            cx.notify();
                        }
                    },
                );
                self._ramp_changes.push(sub);
                picker
            };
            let lo = build(lo, |this, c| this.config.gradient_lo = palette::to_hex(c));
            let hi = build(hi, |this, c| this.config.gradient_hi = palette::to_hex(c));
            self.ramp_pickers = Some([lo, hi]);
        }
        let bar_w = self.config.bar_w();
        let bar_gap = self.config.bar_gap();
        let block_h = self.config.block_h();
        let block_gap = self.config.block_gap();
        let outline_w = self.config.outline_w();
        let gravity = self.config.gravity();
        let bands = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("spectrum-style"),
                Some(rox_i18n::t!("spectrum-style.description")),
                choices_shared(
                    &style_choices(),
                    self.config.style,
                    |this: &mut Self, style, cx| {
                        this.config.style = style;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrum-orientation"),
                Some(rox_i18n::t!("spectrum-orientation.description")),
                choices_shared(
                    &orientation_choices(),
                    self.config.orientation,
                    |this: &mut Self, orientation, cx| {
                        this.config.orientation = orientation;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrum-symmetry"),
                Some(rox_i18n::t!("spectrum-symmetry.description")),
                choices_shared(
                    &symmetry_choices(),
                    self.config.symmetry,
                    |this: &mut Self, symmetry, cx| {
                        this.config.symmetry = symmetry;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrum-bar-width"),
                Some(rox_i18n::t!("spectrum-bar-width.description")),
                settings_ui::scalar(
                    &self.bar_w_scrub,
                    &self.value_edit,
                    bar_w,
                    settings_ui::span(BAR_W_MIN, BAR_W_MAX, " px"),
                    Self::set_bar_width,
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrum-bar-gap"),
                Some(rox_i18n::t!("spectrum-bar-gap.description")),
                settings_ui::scalar(
                    &self.bar_gap_scrub,
                    &self.value_edit,
                    bar_gap,
                    settings_ui::span(BAR_GAP_MIN, BAR_GAP_MAX, " px"),
                    Self::set_bar_gap,
                    cx,
                ),
            ))
            .when(self.config.style == SpectrumStyle::Blocks, |d| {
                d.child(setting_row(
                    rox_i18n::t!("spectrum-block-height"),
                    Some(rox_i18n::t!("spectrum-block-height.description")),
                    settings_ui::scalar(
                        &self.block_h_scrub,
                        &self.value_edit,
                        block_h,
                        settings_ui::span(BLOCK_H_MIN, BLOCK_H_MAX, " px"),
                        Self::set_block_height,
                        cx,
                    ),
                ))
                .child(setting_row(
                    rox_i18n::t!("spectrum-block-gap"),
                    Some(rox_i18n::t!("spectrum-block-gap.description")),
                    settings_ui::scalar(
                        &self.block_gap_scrub,
                        &self.value_edit,
                        block_gap,
                        settings_ui::span(BLOCK_GAP_MIN, BLOCK_GAP_MAX, " px"),
                        Self::set_block_gap,
                        cx,
                    ),
                ))
            })
            .when(self.config.style == SpectrumStyle::Bars, |d| {
                d.child(setting_row(
                    rox_i18n::t!("spectrum-outline-bars"),
                    Some(rox_i18n::t!("spectrum-outline-bars.description")),
                    toggle(
                        self.config.outline,
                        |this: &mut Self, on, cx| {
                            this.config.outline = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .when(self.config.outline, |d| {
                    d.child(setting_row(
                        rox_i18n::t!("spectrum-outline-width"),
                        Some(rox_i18n::t!("spectrum-outline-width.description")),
                        settings_ui::scalar(
                            &self.outline_w_scrub,
                            &self.value_edit,
                            outline_w,
                            settings_ui::span(OUTLINE_W_MIN, OUTLINE_W_MAX, " px"),
                            Self::set_outline_width,
                            cx,
                        ),
                    ))
                })
            });
        let analysis = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("signal-low-bound"),
                Some(rox_i18n::t!("spectrum-low-bound-description")),
                self.freq_slider(&self.lo_scrub, self.config.freq_lo, Self::set_freq_lo, cx),
            ))
            .child(setting_row(
                rox_i18n::t!("signal-high-bound"),
                Some(rox_i18n::t!("spectrum-high-bound-description")),
                self.freq_slider(&self.hi_scrub, self.config.freq_hi, Self::set_freq_hi, cx),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrum-fft-size"),
                Some(rox_i18n::t!("spectrum-fft-size.description")),
                choices(
                    FFT_CHOICES,
                    self.config.fft_lo(),
                    |this: &mut Self, size, cx| {
                        this.config.fft_size = size;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrum-split-zones"),
                Some(rox_i18n::t!("spectrum-split-zones.description")),
                toggle(
                    self.config.split,
                    |this: &mut Self, on, cx| {
                        this.config.split = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.split, |d| {
                d.child(setting_row(
                    rox_i18n::t!("spectrum-split-at"),
                    Some(rox_i18n::t!("spectrum-split-at.description")),
                    self.freq_slider(
                        &self.split_scrub,
                        self.config.split_hz,
                        Self::set_split_hz,
                        cx,
                    ),
                ))
                .child(setting_row(
                    rox_i18n::t!("spectrum-high-fft-size"),
                    Some(rox_i18n::t!("spectrum-high-fft-size.description")),
                    choices(
                        FFT_CHOICES,
                        self.config.fft_hi(),
                        |this: &mut Self, size, cx| {
                            this.config.fft_size_hi = size;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            });
        let color = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("spectrum-gradient-mode"),
                Some(rox_i18n::t!("spectrum-gradient-mode.description")),
                choices_shared(
                    &gradient_choices(),
                    self.config.gradient,
                    |this: &mut Self, gradient, cx| {
                        this.config.gradient = gradient;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when_some(
                (self.config.gradient == Gradient::Custom)
                    .then(|| self.ramp_pickers.clone())
                    .flatten(),
                |d, [lo, hi]| {
                    d.child(setting_row(
                        rox_i18n::t!("spectrum-gradient-base-color"),
                        Some(rox_i18n::t!("spectrum-gradient-base-color.description")),
                        ColorPicker::new(&lo).small(),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("spectrum-gradient-tip-color"),
                        Some(rox_i18n::t!("spectrum-gradient-tip-color.description")),
                        ColorPicker::new(&hi).small(),
                    ))
                },
            );
        let peaks = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("spectrum-peak-caps"),
                Some(rox_i18n::t!("spectrum-peak-caps.description")),
                toggle(
                    self.config.caps,
                    |this: &mut Self, on, cx| {
                        this.config.caps = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrum-cap-gravity"),
                Some(rox_i18n::t!("spectrum-cap-gravity.description")),
                panel::value_slider_edit(
                    &self.gravity_scrub,
                    &self.value_edit,
                    (gravity / GRAVITY_MIN).ln() / (GRAVITY_MAX / GRAVITY_MIN).ln(),
                    format!("{gravity:.2}"),
                    format!("{gravity:.2}"),
                    |v| (v / GRAVITY_MIN).ln() / (GRAVITY_MAX / GRAVITY_MIN).ln(),
                    Self::set_gravity,
                    cx,
                ),
            ));
        div()
            .flex()
            .flex_col()
            .gap(settings_ui::SECTION_GAP)
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-analysis"),
                None,
                analysis,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("spectrum-section-bands"),
                None,
                bands,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-color"),
                None,
                color,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-peaks"),
                None,
                peaks,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-scale"),
                None,
                setting_row(
                    rox_i18n::t!("spectrum-axis-labels"),
                    Some(rox_i18n::t!("spectrum-axis-labels.description")),
                    choices_shared(
                        &label_choices(),
                        self.config.labels,
                        |this: &mut Self, labels, cx| {
                            this.config.labels = labels;
                            cx.notify();
                        },
                        cx,
                    ),
                ),
            ))
            .into_any_element()
    }

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(
            settings_ui::section(
                rox_i18n::t!("viz-section-playback"),
                None,
                setting_row(
                    rox_i18n::t!("spectrum-hold-on-pause"),
                    Some(rox_i18n::t!("spectrum-hold-on-pause.description")),
                    toggle(
                        self.config.freeze,
                        |this: &mut Self, on, cx| {
                            this.config.freeze = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ),
            )
            .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for SpectrumPanel {}

impl Focusable for SpectrumPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for SpectrumPanel {
    fn panel_name(&self) -> &'static str {
        "spectrum"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-spectrum"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(rox_dock::resizable::PANEL_MIN_SIZE, MIN_HEIGHT),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, _window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config.clone())
                };
                SpectrumPanel::new(state, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for SpectrumPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl SpectrumPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The observe re-renders on every pump tick while audio moves. Frame
        // polling only runs the fall after audio stops, then the panel parks.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        let hold = self.config.freeze && session && !playing && !player.queue_ended();
        if !playing && self.bars.lock().unwrap().alive {
            window.request_animation_frame();
        }

        let (freq_lo, freq_hi) = self.config.range();
        let config = self.config.clone();
        let bars = self.bars.clone();
        let feed = self.feed.clone();
        let mut root = div().size_full().relative().bg(palette::bg_root()).child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    let axis = if config.orientation.horizontal() {
                        bounds.size.width
                    } else {
                        bounds.size.height
                    };
                    let axis = f32::from(axis) / if config.symmetry.mirrored() { 2.0 } else { 1.0 };
                    let mut bars = bars.lock().unwrap();
                    bars.step(&feed, axis, &config, hold);
                    bars.paint(bounds, window, &config);
                },
            )
            .size_full(),
        );
        if self.config.labels != Labels::Off {
            root = root.child(labels_overlay(&self.config));
        }
        // While the split slider drags, mark where the zones meet.
        if self.config.split && self.split_scrub.is_dragging() {
            let split = self.config.split_hz.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
            let frac = (split / freq_lo).ln() / (freq_hi / freq_lo).ln();
            if (0.0..=1.0).contains(&frac) {
                for frac in axis_fracs(self.config.symmetry, frac) {
                    root = root.child(axis_rule(
                        self.config.orientation,
                        frac,
                        palette::alpha(palette::highlight(), 0xd9),
                    ));
                }
            }
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_read_the_old_bool_as_the_pitch_scale() {
        let on: Labels = serde_json::from_str("true").unwrap();
        let off: Labels = serde_json::from_str("false").unwrap();
        assert_eq!(on, Labels::Pitch);
        assert_eq!(off, Labels::Off);
    }

    #[test]
    fn labels_round_trip_by_name_and_shrug_off_junk() {
        for mode in [Labels::Off, Labels::Pitch, Labels::Freq] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(serde_json::from_str::<Labels>(&json).unwrap(), mode);
        }
        let unknown: Labels = serde_json::from_str("\"notes\"").unwrap();
        assert_eq!(unknown, Labels::Off);
    }

    #[test]
    fn the_frequency_scale_marks_the_ladder_across_the_range() {
        let config = SpectrumConfig {
            freq_lo: 30.0,
            freq_hi: 16_000.0,
            labels: Labels::Freq,
            ..SpectrumConfig::default()
        };
        let marks = scale_marks(&config);
        let labels: Vec<&str> = marks.iter().map(|(_, text)| text.as_str()).collect();
        assert_eq!(
            labels,
            vec!["50", "100", "200", "500", "1k", "2k", "5k", "10k"]
        );
        for (frac, _) in &marks {
            assert!((0.0..=1.0).contains(frac));
        }
        assert!(marks.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn a_hand_edited_fft_size_lands_in_range_instead_of_overflowing() {
        // Rounding before the clamp would run off the top of the type.
        let junk = SpectrumConfig {
            fft_size: usize::MAX,
            fft_size_hi: usize::MAX,
            ..SpectrumConfig::default()
        };
        for size in [junk.fft_lo(), junk.fft_hi()] {
            assert!((MIN_FFT_SIZE..=MAX_FFT_SIZE).contains(&size));
            assert!(size.is_power_of_two());
        }

        let low = SpectrumConfig {
            fft_size: 0,
            fft_size_hi: 0,
            ..SpectrumConfig::default()
        };
        assert_eq!(low.fft_lo(), MIN_FFT_SIZE);
        assert_eq!(low.fft_hi(), MIN_FFT_SIZE);

        for (&(_, offered), want) in FFT_CHOICES.iter().zip(FFT_CHOICES.iter().map(|c| c.1)) {
            let config = SpectrumConfig {
                fft_size: offered,
                fft_size_hi: offered,
                ..SpectrumConfig::default()
            };
            assert_eq!(config.fft_lo(), want);
            assert_eq!(config.fft_hi(), want);
        }
    }

    #[test]
    fn the_pitch_scale_still_marks_the_octaves() {
        let config = SpectrumConfig {
            labels: Labels::Pitch,
            ..SpectrumConfig::default()
        };
        let labels: Vec<String> = scale_marks(&config)
            .into_iter()
            .map(|(_, text)| text)
            .collect();
        // C0 is under the default floor and C10 over the ceiling.
        assert_eq!(
            labels,
            vec!["C1", "C2", "C3", "C4", "C5", "C6", "C7", "C8", "C9"]
        );
    }
}
