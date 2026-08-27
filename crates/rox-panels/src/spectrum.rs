//! The spectrum panel: live frequency bars over the player's PCM tap, the
//! classic analyzer look: log-spaced bands, snappy attack, eased decay,
//! peak-hold caps falling under gravity, dB gridlines behind. Everything is
//! paint primitives on the UI thread: one FFT per frame while audio flows,
//! and once the bars have settled the panel stops asking for frames, so an
//! idle app pays nothing. The analyzed range, the FFT window size (split
//! zoning trades reactivity for resolution per end of the range), the
//! render style (bars, LED blocks, or a solid line), the edge the bands
//! grow from and the mirrored symmetry, the bar width and fill, the
//! peak-hold caps and their gravity, and the axis scale (octave pitches or
//! frequencies) are per-view config the customize window edits and the
//! layout dump stores.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, fill, linear_color_stop, linear_gradient, point, prelude::*, px, relative, size,
    AnyElement, App, BorderStyle, Bounds, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, Path, Rgba, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::PopupMenu;
use gpui_component::Sizable as _;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_panel_kit::axis::{fmt_axis_hz, fmt_hz};
use serde::{Deserialize, Serialize};

use rox_viz::analysis::{hz_ladder, log_bands, Analyzer, MAX_FFT_SIZE, MIN_FFT_SIZE};
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, choices, choices_shared, setting_row, toggle, AppState, PanelChrome, PanelSettings,
    ScrubState,
};
use crate::panel_settings;
use crate::settings::ui as settings_ui;

// Bars follow the visualizer rhythm at the configured width over the shared
// gap (`tokens::BAR_GAP`); the count collapses on narrow panels instead of
// thinning the bars, so a small dock split doesn't smear. The ceiling covers
// a wide panel at the thinnest bars.
const MIN_BARS: usize = 16;
const MAX_BARS: usize = 512;

/// The panel's own height floor, under the dock's 40px default: the body is
/// one canvas that draws at whatever height it gets, so a layout is free to
/// run the bands as a thin strip along an edge. The width keeps the dock
/// floor, which the band count needs to stay readable.
const MIN_HEIGHT: gpui::Pixels = gpui::px(16.);

/// The bar width slider's span, px: thin bars pack more bands into the
/// width, thick ones read chunky. Values snap to whole pixels.
const BAR_W_MIN: f32 = 1.0;
const BAR_W_MAX: f32 = 12.0;

/// The bar gap slider's span, px: zero packs the bars edge to edge, the top
/// leaves a wide channel between them. Values snap to whole pixels.
const BAR_GAP_MIN: f32 = 0.0;
const BAR_GAP_MAX: f32 = 8.0;

/// The outline stroke slider's span, px: hairline up to a chunky frame.
/// Values snap to whole pixels; a stroke past half the bar width reads
/// as a filled bar again.
const OUTLINE_W_MIN: f32 = 1.0;
const OUTLINE_W_MAX: f32 = 4.0;

/// The block cell sliders' spans, px: how deep each cell draws and the
/// dark seam between cells in the block style. Values snap to whole
/// pixels; gap zero fuses a stack back into a solid bar.
const BLOCK_H_MIN: f32 = 2.0;
const BLOCK_H_MAX: f32 = 12.0;
const BLOCK_GAP_MIN: f32 = 0.0;
const BLOCK_GAP_MAX: f32 = 4.0;

/// The line style's stroke thickness, px.
const LINE_W: f32 = 1.5;

/// The frequency band the bounds sliders (and a hand-edited config) may pick
/// between: roughly the audible range up to a typical Nyquist ceiling.
const SLIDER_MIN_HZ: f32 = 20.0;
const SLIDER_MAX_HZ: f32 = 20_000.0;

/// The smallest span the low and high bounds keep between them, so the band
/// mapping always has room and never inverts.
const MIN_RATIO: f32 = 2.0;

/// C0's pitch; each octave up doubles it. The pitch markers step through these.
const C0_HZ: f32 = 16.352;

/// dB window the bars normalize into, on magnitudes where a full-scale sine
/// is 0 dB. The top leaves headroom so a busy mix pins near full height
/// without every band clipping there.
const FLOOR_DB: f32 = -66.0;
const MAX_DB: f32 = -12.0;

/// Per-second smoothing rates: bands jump up fast and fall slowly, so kicks
/// read as kicks instead of flicker.
const ATTACK: f32 = 40.0;
const RELEASE: f32 = 10.0;

/// The default rate peak-hold caps accelerate downward at, in bar heights
/// per second squared: a transient leaves a marker that drifts back down.
const HOLD_GRAVITY: f32 = 0.05;

/// The cap gravity slider's span, log-spaced so the floaty low end gets
/// most of the travel.
const GRAVITY_MIN: f32 = 0.01;
const GRAVITY_MAX: f32 = 1.0;

/// The FFT sizes the pickers offer: short windows react fast, long ones
/// resolve finer, especially down low.
const FFT_CHOICES: &[(&str, usize)] = &[
    ("512", 512),
    ("1k", 1024),
    ("2k", 2048),
    ("4k", 4096),
    ("8k", 8192),
    ("16k", 16384),
];

/// dB gridlines drawn behind the bars.
const DB_MARKS: [f32; 3] = [-20.0, -40.0, -60.0];

/// Everything below this reads as settled; the panel stops animating.
const EPSILON: f32 = 0.002;

/// How long the feed may sit still before it reads as stopped audio rather
/// than the gap between pump ticks (the tap drains on a ~16ms timer, so
/// frames between ticks see no new samples). Between ticks the bars hold
/// their targets instead of dipping toward silence. The dip-and-reattack
/// used to read as shimmer on high-refresh displays and as a full strobe
/// under load. Paused and stopped push nothing and cross this quickly;
/// playing audio always pushes, silence included.
const SILENT_AFTER: f32 = 0.15;

/// How the bands render: the classic solid bars, LED-style stacks of
/// blocks (the Winamp and Block Analyzer look), or a solid line over a
/// soft fill (the Fruity EQ look).
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpectrumStyle {
    #[default]
    Bars,
    Blocks,
    Line,
}

/// The edge the bands grow from. Left and right turn the panel sideways:
/// the frequency axis runs vertically, low end at the bottom.
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
    /// Whether the frequency axis runs along the panel's width.
    pub fn horizontal(self) -> bool {
        matches!(self, Orientation::Bottom | Orientation::Top)
    }

    /// The gradient angle pointing from the base edge toward the tips,
    /// degrees clockwise from up.
    pub fn tip_angle(self) -> f32 {
        match self {
            Orientation::Bottom => 0.0,
            Orientation::Top => 180.0,
            Orientation::Left => 90.0,
            Orientation::Right => 270.0,
        }
    }
}

/// How the bands color: flat accent, or a loudness ramp. The ramp is the
/// theme's dim floor up to the accent, the cover art's two extracted colors
/// while song theming derives (the accent and highlight hold the art's
/// primary and runner-up, and fall back to the plain palette when it
/// doesn't), or a custom two-color pair.
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
    /// By hand for the layouts dumped before the ramp had sources, when
    /// `gradient` was the Intensity Color bool: true was the theme ramp,
    /// false flat. An unknown name reads as flat rather than failing the
    /// whole panel config.
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

/// What the axis is marked with, if anything: the octave pitches a player
/// reads a range by, or the frequencies an engineer does. Both rule the
/// same dividers, so it's one choice rather than two overlays fighting for
/// the same edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Labels {
    #[default]
    Off,
    Pitch,
    Freq,
}

impl<'de> Deserialize<'de> for Labels {
    /// By hand for the layouts dumped while `labels` was the Pitch Labels
    /// bool: true was the octave marks, false none. [`Gradient`]'s shape,
    /// and an unknown name reads as off rather than failing the panel.
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

/// The symmetry modes: off, or the spectrum folded around the axis center
/// and painted mirrored into both halves. Forward runs the range
/// outside-in, lows at the outer edges; reverse runs it inside-out, lows
/// meeting at the middle.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Symmetry {
    #[default]
    None,
    Forward,
    Reverse,
}

impl Symmetry {
    /// Whether the spectrum folds into two mirrored halves.
    fn mirrored(self) -> bool {
        self != Symmetry::None
    }

    /// Whether the range runs backwards within its half.
    fn reversed(self) -> bool {
        self == Symmetry::Reverse
    }
}

/// The style and symmetry pickers' options, translated at render time.
fn style_choices() -> [(SharedString, SpectrumStyle); 3] {
    [
        (rox_i18n::t!("spectrum-style-bars"), SpectrumStyle::Bars),
        (rox_i18n::t!("spectrum-style-blocks"), SpectrumStyle::Blocks),
        (rox_i18n::t!("spectrum-style-line"), SpectrumStyle::Line),
    ]
}

/// Shared with the VU meter panel, which grows its meters from the same
/// four edges.
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

/// Shared with the VU meter panel, which colors its meters by the same
/// loudness ramp.
pub fn gradient_choices() -> [(SharedString, Gradient); 4] {
    [
        (rox_i18n::t!("panel-size-off"), Gradient::Off),
        (rox_i18n::t!("spectrum-gradient-theme"), Gradient::Theme),
        (rox_i18n::t!("spectrum-gradient-cover"), Gradient::Cover),
        (rox_i18n::t!("shader-pick-custom"), Gradient::Custom),
    ]
}

/// The spectrum panel's per-view config: what a saved layout restores, and
/// what the customize window edits. Missing fields take the defaults, so a
/// layout dumped before this config existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrumConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// How the bands render: bars, blocks, or a line.
    pub style: SpectrumStyle,
    /// The edge the bands grow from.
    pub orientation: Orientation,
    /// Fold the spectrum around the axis center, the symmetry look:
    /// forward runs the lows to the outer edges, reverse meets them at
    /// the middle.
    pub symmetry: Symmetry,
    /// Low bound of the analyzed range, Hz: the bars span log-spaced from
    /// here up to `freq_hi`.
    pub freq_lo: f32,
    /// High bound of the analyzed range, Hz. Capping below Nyquist drops the
    /// near-silent top octaves that would sit motionless on the right.
    pub freq_hi: f32,
    /// Bar thickness, px: thinner bars pack more bands into the width for
    /// a more detailed spectrum.
    pub bar_width: f32,
    /// Gap between bars, px: zero packs them edge to edge, wider spreads
    /// them out. Also sets the bar count, so a wider gap fits fewer bars.
    pub bar_gap: f32,
    /// Cell depth in the block style, px.
    pub block_height: f32,
    /// Dark seam between cells in the block style, px.
    pub block_gap: f32,
    /// FFT window size: short windows react fast, long ones resolve finer.
    /// With split zoning on this covers the bands below `split_hz`.
    pub fft_size: usize,
    /// Split zoning: analyze below and above `split_hz` at different
    /// window sizes, so each end of the range trades reactivity for
    /// resolution on its own.
    pub split: bool,
    /// Where the zones split, Hz. Snaps to the nearest bar edge so the
    /// log spacing runs unbroken across the seam.
    pub split_hz: f32,
    /// The window size for the bands above the split.
    pub fft_size_hi: usize,
    /// How the bands color: flat accent, or a loudness ramp from the
    /// theme, the cover art, or the custom pair below.
    pub gradient: Gradient,
    /// The custom ramp's ends, `#rrggbb`: the quiet base and the loud tip.
    pub gradient_lo: String,
    pub gradient_hi: String,
    /// Draw each bar as a hollow outline instead of a filled ramp.
    pub outline: bool,
    /// Stroke thickness of the hollow bars, px.
    pub outline_width: f32,
    /// Peak-hold caps above the bars.
    pub caps: bool,
    /// Freeze the bars while playback is paused instead of letting them
    /// fall to silence.
    pub freeze: bool,
    /// How hard the caps fall, bar heights per second squared.
    pub cap_gravity: f32,
    /// Mark the analyzed range across the panel, by pitch or by frequency.
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
            freeze: false,
            cap_gravity: HOLD_GRAVITY,
            labels: Labels::default(),
        }
    }
}

impl SpectrumConfig {
    /// The analyzed range, clamped to the slider band and the minimum span,
    /// so a hand-edited file can't invert or collapse the bands.
    fn range(&self) -> (f32, f32) {
        let lo = self.freq_lo.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
        let hi = self
            .freq_hi
            .clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ)
            .max(lo * MIN_RATIO)
            .min(SLIDER_MAX_HZ);
        (lo.min(hi / MIN_RATIO), hi)
    }

    /// The bar thickness, outline stroke, and cap gravity, clamped the way
    /// [`Self::range`] clamps the bounds. The px knobs read back to the
    /// typed ceiling rather than the strip's own top, or every value typed
    /// past the top would drop on the next load.
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

    /// The custom ramp's ends parsed, falling back to the theme ramp's
    /// when a hand-edited hex doesn't parse.
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

    /// The FFT sizes, snapped to the picker's power-of-two steps so a
    /// hand-edited file can't give the analyzer a bad size. The clamp comes
    /// first on purpose: `next_power_of_two` overflows on anything past the
    /// top power of two the type holds, which is a panic in debug and a wrap
    /// to zero in release, so rounding an unbounded number straight out of a
    /// layout file is the one input that gets past this. Every size in range
    /// rounds the same either way.
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

/// A strip fraction (0 to 1) as a log-spaced frequency across the slider
/// band, and back. Log so an octave takes the same travel anywhere.
fn frac_to_hz(fraction: f32) -> f32 {
    SLIDER_MIN_HZ * (SLIDER_MAX_HZ / SLIDER_MIN_HZ).powf(fraction.clamp(0.0, 1.0))
}

fn hz_to_frac(hz: f32) -> f32 {
    (hz / SLIDER_MIN_HZ).ln() / (SLIDER_MAX_HZ / SLIDER_MIN_HZ).ln()
}

/// One analysis zone: an analyzer at its own window size covering a run
/// of the bars. Unsplit runs one over everything; split zoning runs two,
/// each end of the range trading reactivity for resolution on its own.
struct Zone {
    analyzer: Analyzer,
    mono: Vec<f32>,
    /// Half-spectrum bin range per bar in this zone.
    bands: Vec<(usize, usize)>,
}

/// What the current zones and band mappings were built for; any change
/// rebuilds them, the way a bounds or device-rate change always remapped.
#[derive(PartialEq)]
struct Mapping {
    count: usize,
    rate: u32,
    freq_lo: f32,
    freq_hi: f32,
    fft_lo: usize,
    fft_hi: usize,
    /// The split frequency, or zero with split zoning off.
    split_hz: f32,
}

impl Mapping {
    /// The zones this mapping calls for. The split snaps to the bar edge
    /// nearest the split frequency, so the log spacing runs unbroken
    /// across the seam; a split outside the analyzed range leaves one
    /// zone at whichever size covers it.
    fn zones(&self) -> Vec<Zone> {
        let zone = |bars: usize, size: usize, lo: f32, hi: f32| Zone {
            analyzer: Analyzer::new(size),
            mono: vec![0.0; size],
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

/// Per-panel analyzer state, shared with the paint closure the way the old
/// sim shared its frames: the entity holds the handle, the closure does the
/// per-frame work where the bounds are known.
struct Bars {
    last_written: u64,
    last_tick: Option<Instant>,
    /// What the zones were built for; a mismatch rebuilds them.
    mapping: Option<Mapping>,
    zones: Vec<Zone>,
    /// What each bar eases toward: refreshed per analysis, held between
    /// them, zeroed once the feed reads as stopped (see [`SILENT_AFTER`]).
    targets: Vec<f32>,
    /// When the feed last carried new audio.
    last_fresh: Option<Instant>,
    levels: Vec<f32>,
    holds: Vec<f32>,
    hold_vel: Vec<f32>,
    /// Bars still moving: render keeps requesting frames until this clears.
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

    /// One tick: pull the newest window off the feed, fold it into the bar
    /// levels, advance the holds. No new audio means the bars decay, unless
    /// `hold` keeps the last frame standing (the freeze-on-pause option).
    /// `axis` is the length the bands lay along, the panel's width or
    /// height per the orientation, halved when mirrored.
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

        // Frozen: keep the levels and holds exactly where they are and stop
        // animating; paint keeps showing the standing frame. A settings edit
        // that remaps the bars still takes effect: the feed keeps the last window,
        // so the frame re-analyzes below at the new mapping instead of
        // ignoring the edit until playback resumes.
        if hold && !fresh && !remap {
            self.alive = false;
            return;
        }

        // New audio since last tick: analyze the latest window per zone and
        // refresh the targets. Nothing new: hold the targets (it's just the
        // gap between pump ticks) until the feed has sat still long enough
        // to read as stopped, then let the bars fall to silence.
        // A remap also re-analyzes: it just reset the targets, and the
        // buffered window rebuilds them at the new mapping without waiting
        // for the next pump tick.
        if fresh {
            self.last_fresh = Some(now);
        }
        let stopped = self
            .last_fresh
            .is_none_or(|t| (now - t).as_secs_f32() > SILENT_AFTER);

        let mut alive = false;
        let mut bar = 0;
        for zone in &mut self.zones {
            let Zone {
                analyzer,
                mono,
                bands,
            } = zone;
            let mags = ((fresh || remap) && feed.latest_mono(mono) == mono.len())
                .then(|| analyzer.magnitudes(mono));
            for &(lo, hi) in bands.iter() {
                let i = bar;
                bar += 1;
                if let Some(mags) = mags {
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
                    // Frozen: the frame changed mapping, not time. Jump to
                    // the new targets at once, since the next tick parks
                    // again and an ease would strand the bars partway.
                    self.levels[i] = target;
                } else {
                    let rate = if target > self.levels[i] {
                        ATTACK
                    } else {
                        RELEASE
                    };
                    self.levels[i] += (target - self.levels[i]) * (rate * dt).min(1.0);
                }

                // The cap follows the bar up and falls back under gravity
                // once the bar drops away. Caps off: the holds track the
                // bars so they don't keep the panel animating.
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

        // The bands lay along `axis`, levels grow into `depth`; symmetric
        // panels lay them into half the axis and paint each band twice,
        // the second half reflected. Reverse runs the range backwards
        // within its half, so the lows meet at the middle instead of
        // holding the outer edges.
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

        // Axis/depth space into panel space: `a` along the frequency axis
        // (rightward, or upward on the sideways orientations), `d` from
        // the base edge toward the tips.
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

        // dB gridlines behind the bars.
        for db in DB_MARKS {
            let d = (db - FLOOR_DB) / (MAX_DB - FLOOR_DB) * max_d;
            window.paint_quad(fill(
                rect(0.0, axis, d, 1.0),
                palette::alpha(palette::gridline(), 0x28),
            ));
        }

        // The block grid: cells stacked into the depth on a shared rhythm,
        // the caps quantizing onto the same grid.
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
                // The bar base color: flat accent, or the configured ramp
                // at the band's level so only the peaks read hot.
                let base = bar_color(config, level);
                for &a in slots {
                    if config.style == SpectrumStyle::Blocks {
                        // The stack: cells lit up to the level, each colored
                        // by its own height on the ramp, the classic look
                        // where only a tall stack's top runs hot.
                        let lit = (level * cells as f32).round() as usize;
                        for c in 0..lit {
                            let color = bar_color(config, (c as f32 + 0.5) / cells as f32);
                            window
                                .paint_quad(fill(rect(a, bar_w, c as f32 * cell, block_h), color));
                        }
                        // A ghosted cell at the base keeps a silent band's
                        // footprint, the block twin of the bars' 2px stub.
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
                        // Hollow variant: the bar as a frame in its base
                        // color, at the configured stroke width.
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
                            // Solid base at the baseline fading out toward
                            // the bar tip, whichever way the tips point.
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
        // Peak-hold caps at the held level above each band: position marks
        // like the playheads and slider knobs, so they use the highlight
        // and stay legible over accent-colored bars. Block style lights a
        // floating segment on the cell grid instead of a thin line.
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

    /// The line style: a solid stroke through the band tips over a soft
    /// fill down to the baseline, built as triangle strips the way the
    /// chart donut fans its ring. Intensity color follows the depth here:
    /// one path is one fill, so the ramp runs base to tip rather than
    /// per band.
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

        // The curve runs through the band centers, pinned to the half's
        // edges so it spans it fully; a mirrored panel's halves meet at
        // the center without a seam.
        let mut tips = Vec::with_capacity(count + 2);
        tips.push((0.0, (self.levels[0] * max_d).max(2.0)));
        for (i, &level) in self.levels.iter().enumerate() {
            tips.push((i as f32 * step + step / 2.0, (level * max_d).max(2.0)));
        }
        tips.push((half, (self.levels[count - 1] * max_d).max(2.0)));

        // The ramp's ends for the configured source; None paints the flat
        // accent. One path is one fill, so the ramp follows the depth as a
        // base-to-tip gradient rather than per band.
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
                if reflect {
                    axis - a
                } else {
                    a
                }
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

/// A band's color at ramp position `t`: its level, or a cell's height in
/// the block stack. The ramp itself is in [`ramp_color`], shared with the
/// VU meter panel.
fn bar_color(config: &SpectrumConfig, t: f32) -> Rgba {
    ramp_color(config.gradient, t, config.custom_ramp())
}

/// The loudness ramp at position `t`, shared with the VU meter panel so both
/// visualizers color the same way. Flat mode is the accent everywhere; the
/// ramps blend upward, curved so the mids stay muted and only the top lights
/// up. The cover ramp runs accent to highlight (the art's primary and
/// runner-up while song theming derives) and stops short of full highlight
/// so the peak caps stay legible on a pinned band. `custom` is the parsed
/// custom pair, ignored unless the source is [`Gradient::Custom`].
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

/// A hairline across the panel at an axis fraction: vertical at `frac` of
/// the width on the horizontal orientations, horizontal at `frac` of the
/// height (bottom-up) on the sideways ones.
fn axis_rule(orientation: Orientation, frac: f32, color: Rgba) -> Div {
    let rule = div().absolute().border_color(color);
    if orientation.horizontal() {
        rule.top_0().bottom_0().left(relative(frac)).border_l_1()
    } else {
        rule.left_0().right_0().bottom(relative(frac)).border_b_1()
    }
}

/// Where an axis fraction of the analyzed range maps to on the panel: one
/// spot as-is, or two under symmetry, folded into the halves: forward
/// outside-in, reverse inside-out.
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

/// Where the marks fall over the analyzed range and what each one says:
/// every C for the pitch scale, the 1-2-5 ladder's labelled steps for the
/// frequency one. Positions are log-frequency fractions along the axis, so
/// they line up with the bars at any panel size.
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

/// The scale over the analyzed range: a faint divider at each mark with its
/// label tucked against it, hugging the panel edge the config's orientation
/// leaves quiet. Symmetric panels rule both halves but label only the first:
/// the reflected half reads backwards, and twin labels would just clutter it.
fn labels_overlay(config: &SpectrumConfig) -> Div {
    let mut overlay = div().absolute().inset_0();
    for (frac, label) in scale_marks(config) {
        let fracs = axis_fracs(config.symmetry, frac);
        // A label pinned to the axis' far end would clip; drop it and keep
        // the divider. Folded halves never reach the end.
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

/// One marker: the divider across the panel with its label against it.
/// Horizontal orientations run the divider full height with the text along
/// the base edge; sideways ones put the text on the divider, against the
/// base edge.
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

/// How much of the axis a band has to cover before both its bounds get
/// their number: under this the two would print over each other, and the
/// low bound is the one that gets the name.
const BAND_LABEL_GAP: f32 = 0.08;

/// Mark a frequency band across a spectrum drawn with `config`: a rule at
/// each bound, each saying its own frequency, with the band's name leading
/// the low one. Positioned off the same log mapping the bars use, so a
/// bound picked here ends up where the eye put it.
///
/// Both labels hang inside the band, so the pair brackets what it covers
/// rather than trailing off one side, and neither runs off the panel when a
/// bound is near an edge. They use the edge the frequency scale's own
/// numbers leave alone, or the two would sit on top of each other.
///
/// `strong` is the drag: a band brightens while one of its bounds is
/// actually moving, so the one being edited stands out from the rest.
///
/// A bound outside the analyzed range draws nothing, the way the split
/// marker is hidden: pinning it to the edge would put a line where the
/// bound isn't, and the slider's own readout has the number.
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
    // The name leads the low bound's number, in the slider's own wording,
    // so the mark and the row that moves it read the same.
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
        // Symmetric panels fold the range into both halves, so a bound
        // stands in two places; the text only goes on the first, the way
        // the scale labels only the unreflected half.
        for (i, frac) in axis_fracs(config.symmetry, frac).into_iter().enumerate() {
            overlay = overlay.child(match text.clone().filter(|_| i == 0) {
                Some(text) => band_mark(config.orientation, frac, &text, color, far),
                None => axis_rule(config.orientation, frac, color),
            });
        }
    }
    overlay
}

/// One bound's rule with its text against it. `far` anchors the rule from
/// the other end of the axis, which hangs the text on the other side of the
/// line: the high bound reads inwards, the low bound outwards from it, and
/// the two bracket the band between them.
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
        // Along the top edge on the upright orientations, since the scale's
        // numbers run along the base.
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
    /// The settings sliders' painted bounds and drag state, one per slider
    /// so a drag on one never moves the others.
    lo_scrub: ScrubState,
    hi_scrub: ScrubState,
    bar_w_scrub: ScrubState,
    bar_gap_scrub: ScrubState,
    block_h_scrub: ScrubState,
    block_gap_scrub: ScrubState,
    outline_w_scrub: ScrubState,
    gravity_scrub: ScrubState,
    split_scrub: ScrubState,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
    /// The custom ramp's pickers, base then tip, built on the first
    /// settings render, since the panel itself constructs without a window
    /// and the picker state needs one.
    ramp_pickers: Option<[Entity<ColorPickerState>; 2]>,
    _ramp_changes: Vec<Subscription>,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel when a session starts, so an idle window resumes
    /// animating without the player bar's frame pump.
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
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    fn set_freq_lo(&mut self, fraction: f32, cx: &mut Context<Self>) {
        // The low bound stops a min-span short of the high one, so the range
        // never inverts as the strip drags past it. The ceiling is floored at
        // the slider minimum so a hand-edited-tiny high bound can't invert the
        // clamp.
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

    /// One log-frequency bounds slider: the shared scalar slider with the
    /// Hz readout alongside, click-to-type like the rest. The readout
    /// switches to kHz up top, but the input is always plain Hz, so the
    /// seed drops the unit and `hz_to_frac` reads what's typed straight.
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
        // The custom ramp's pickers on first need; each edit writes its
        // hex back into the config, the format the layout dump stores.
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
        div()
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
            })
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
            )
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
            })
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
            ))
            .child(setting_row(
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
            ))
            .into_any_element()
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

    /// The layout dump stores the panel's config; the builder registered in
    /// `workspace::register_panels` reads it back.
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
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl SpectrumPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // While audio moves the direct observe re-renders the panel on
        // every pump tick, the only rate new samples arrive at, so frames
        // past it re-analyze nothing. Frame polling is just for the falling
        // bars after audio stops, when no more ticks come; once they settle
        // the panel parks, and a resume wakes it through the pump's
        // play-state notify.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        // Freeze on pause holds the standing frame: paused mid-session, not
        // a played-out queue.
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
                    // The bands lay along the width, or the height on the
                    // sideways orientations; symmetric panels fold the
                    // range into half of it.
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
        // While the split slider drags, mark where the zones meet so the
        // pick can be made by eye; the playhead's alpha keeps it legible. A
        // symmetric panel's zones meet twice, once per half.
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

    /// Every shipped bundle still spells `labels` as the old bool, so most
    /// configs in the wild go through the legacy read.
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
        // Positions rise across the panel and stay inside it.
        for (frac, _) in &marks {
            assert!((0.0..=1.0).contains(frac));
        }
        assert!(marks.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn a_hand_edited_fft_size_lands_in_range_instead_of_overflowing() {
        // The number nobody types on purpose: rounding it up before the clamp
        // runs off the top of the type, which is what the accessor's ordering
        // is there to dodge.
        let junk = SpectrumConfig {
            fft_size: usize::MAX,
            fft_size_hi: usize::MAX,
            ..SpectrumConfig::default()
        };
        for size in [junk.fft_lo(), junk.fft_hi()] {
            assert!((MIN_FFT_SIZE..=MAX_FFT_SIZE).contains(&size));
            assert!(size.is_power_of_two());
        }

        // Zero from the other end, and the sizes the picker really offers,
        // which have to survive the reorder unchanged.
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
        // C0 is under the default range's floor and C10 over its ceiling.
        assert_eq!(
            labels,
            vec!["C1", "C2", "C3", "C4", "C5", "C6", "C7", "C8", "C9"]
        );
    }
}
