//! The spectrogram panel: the player's PCM tap as a scrolling waterfall,
//! frequency across one axis, time along the other, loudness as color. It
//! keeps the last few seconds standing where the spectrum and the
//! oscilloscope forget each frame.
//!
//! A per-pixel heatmap is tens of thousands of quads a frame, too many for
//! paint primitives, so the history lives in a ring of reduced columns
//! baked into a small [`RenderImage`] the renderer scales. Frames between
//! columns repaint the same texture.
//!
//! Deliberately no peak tracking, per-column normalization, or scroll
//! interpolation: the dB window is the config's, so a quiet passage reads
//! as quiet.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Context, Corners, Div, EventEmitter, FocusHandle, Focusable, Hsla,
    Pixels, RenderImage, Rgba, SharedString, Subscription, TextRun, WeakEntity, Window, canvas,
    div, fill, point, prelude::*, px, size,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use image::{Frame, RgbaImage};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_panel_kit::axis::{fmt_axis_hz, fmt_hz};
use serde::{Deserialize, Serialize};

use rox_viz::AudioFeed;
use rox_viz::analysis::{MAX_FFT_SIZE, MIN_FFT_SIZE, hz_ladder};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, AppState, PanelChrome, PanelSettings, ScrubState, choices, choices_shared, setting_row,
    toggle,
};
use crate::panel_settings;
use crate::settings::ui as settings_ui;
use crate::spectrum::{Orientation, orientation_choices};

/// Fixed rather than following the FFT size, so window size and history
/// memory are independent. Finer than any panel resolves vertically.
const ROWS: usize = 256;

/// The lookup keeps the per-pixel loop down to a copy.
const LUT_STEPS: usize = 256;

/// Below the bottom of hearing up to a typical Nyquist ceiling.
const HZ_MIN: f32 = 10.0;
const HZ_MAX: f32 = 24_000.0;

const MIN_RATIO: f32 = 2.0;

/// A full-scale sine is 0 dB.
const FLOOR_MIN: f32 = -120.0;
const FLOOR_MAX: f32 = -40.0;
const CEIL_MIN: f32 = -40.0;
const CEIL_MAX: f32 = 0.0;

/// The two sliders overlap at -40, so without this the window could
/// collapse onto one value.
const MIN_DB_SPAN: f32 = 6.0;

/// The slow end holds most of a minute on a wide panel.
const SPEED_MIN: f32 = 5.0;
const SPEED_MAX: f32 = 120.0;

/// The texture's long side and the memory bound: the top is 2 MB of cells.
const HISTORY_MIN: usize = 128;
const HISTORY_MAX: usize = 2048;

/// Past 8k a column stops meaning one moment.
const FFT_CHOICES: &[(&str, usize)] = &[("1k", 1024), ("2k", 2048), ("4k", 4096), ("8k", 8192)];

/// How long the feed may sit still before it reads as stopped rather than
/// a gap between pump ticks. Treating the gap as silence would print a
/// black stripe through a loud track.
const SILENT_AFTER: f32 = 0.15;

const MIN_SIDE: Pixels = px(24.);

/// The first four are perceptual ramps; Theme and Cover build one of the
/// same shape from the palette ([`heat_color`]).
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Colormap {
    #[default]
    Magma,
    Viridis,
    Ice,
    Grayscale,
    Theme,
    Cover,
}

/// A handful of stops is indistinguishable from the full 256-entry tables
/// at this size, and keeps them out of the binary.
const MAGMA: [[u8; 3]; 5] = [
    [0, 0, 4],
    [81, 18, 124],
    [183, 55, 121],
    [252, 137, 97],
    [252, 253, 191],
];
const VIRIDIS: [[u8; 3]; 5] = [
    [68, 1, 84],
    [59, 82, 139],
    [33, 145, 140],
    [94, 201, 98],
    [253, 231, 37],
];
const ICE: [[u8; 3]; 4] = [[0, 2, 16], [8, 52, 120], [32, 150, 200], [230, 250, 255]];
const GRAYSCALE: [[u8; 3]; 2] = [[0, 0, 0], [255, 255, 255]];

fn colormap_choices() -> [(SharedString, Colormap); 6] {
    [
        (rox_i18n::t!("spectrogram-colormap-magma"), Colormap::Magma),
        (
            rox_i18n::t!("spectrogram-colormap-viridis"),
            Colormap::Viridis,
        ),
        (rox_i18n::t!("spectrogram-colormap-ice"), Colormap::Ice),
        (
            rox_i18n::t!("spectrogram-colormap-grayscale"),
            Colormap::Grayscale,
        ),
        (rox_i18n::t!("spectrogram-colormap-theme"), Colormap::Theme),
        (rox_i18n::t!("spectrogram-colormap-cover"), Colormap::Cover),
    ]
}

/// A clamp that swallows NaN, which `f32::clamp` passes through. One NaN
/// from a hand-edited layout would take a whole column.
fn sane(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_nan() {
        fallback
    } else {
        value.clamp(min, max)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrogramConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub fft_size: usize,
    pub lo_hz: f32,
    /// Hz. Capping below Nyquist drops the near-silent top octaves.
    pub hi_hz: f32,
    /// Linear is the lab-tool view, where a harmonic stack reads evenly
    /// spaced.
    pub log_scale: bool,
    pub floor_db: f32,
    pub ceil_db: f32,
    pub speed: f32,
    pub history: usize,
    pub colormap: Colormap,
    /// The frequency axis runs across the scroll direction.
    pub direction: Orientation,
    pub grid: bool,
    pub labels: bool,
    pub freeze: bool,
}

impl Default for SpectrogramConfig {
    fn default() -> Self {
        SpectrogramConfig {
            chrome: PanelChrome::default(),
            fft_size: 4096,
            lo_hz: 20.0,
            hi_hz: 20_000.0,
            log_scale: true,
            floor_db: -90.0,
            ceil_db: -20.0,
            speed: 40.0,
            history: 512,
            colormap: Colormap::default(),
            direction: Orientation::Right,
            grid: true,
            labels: true,
            freeze: true,
        }
    }
}

impl SpectrogramConfig {
    /// Clamp before rounding: `next_power_of_two` overflow-panics near the top
    /// of `usize`, and the feed answers nothing for a size out of range.
    fn fft(&self) -> usize {
        self.fft_size
            .clamp(MIN_FFT_SIZE, MAX_FFT_SIZE)
            .next_power_of_two()
    }

    fn range(&self) -> (f32, f32) {
        let lo = sane(self.lo_hz, HZ_MIN, HZ_MAX, 20.0);
        let hi = sane(self.hi_hz, HZ_MIN, HZ_MAX, 20_000.0)
            .max(lo * MIN_RATIO)
            .min(HZ_MAX);
        (lo.min(hi / MIN_RATIO), hi)
    }

    /// The two sliders meet at -40, so the ceiling is held clear of whatever
    /// the floor ended up at.
    fn db_window(&self) -> (f32, f32) {
        let floor = sane(self.floor_db, FLOOR_MIN, FLOOR_MAX, -90.0);
        let ceil = sane(self.ceil_db, CEIL_MIN, CEIL_MAX, -20.0).max(floor + MIN_DB_SPAN);
        (floor, ceil)
    }

    fn speed(&self) -> f32 {
        sane(self.speed, SPEED_MIN, SPEED_MAX, 40.0)
    }

    fn history(&self) -> usize {
        self.history.clamp(HISTORY_MIN, HISTORY_MAX)
    }

    fn axis_frac(&self, hz: f32) -> f32 {
        let (lo, hi) = self.range();
        if self.log_scale {
            (hz / lo).ln() / (hi / lo).ln()
        } else {
            (hz - lo) / (hi - lo)
        }
    }
}

fn frac_to_hz(fraction: f32) -> f32 {
    HZ_MIN * (HZ_MAX / HZ_MIN).powf(fraction.clamp(0.0, 1.0))
}

fn hz_to_frac(hz: f32) -> f32 {
    (hz / HZ_MIN).ln() / (HZ_MAX / HZ_MIN).ln()
}

fn freq_at(t: f32, lo: f32, hi: f32, log: bool) -> f32 {
    if log {
        lo * (hi / lo).powf(t)
    } else {
        lo + (hi - lo) * t
    }
}

/// Wall clock rather than frame count, keeping the fraction, so the scroll
/// rate holds at any refresh and a slow speed doesn't round to nothing.
fn columns_due(accum: &mut f32, dt: f32, speed: f32) -> usize {
    *accum += dt * speed;
    if !accum.is_finite() {
        *accum = 0.0;
        return 0;
    }
    let whole = accum.floor().max(0.0);
    *accum -= whole;
    whole as usize
}

/// Two regimes: under a bin wide the row interpolates, or a log axis draws
/// the low end as a staircase; wider, it takes the max so a narrow partial
/// survives. Bin 0 (DC) never reaches a row.
fn fold(mags: &[f32], b0: f32, b1: f32) -> f32 {
    let half = mags.len();
    if half < 2 {
        return 0.0;
    }
    let top = (half - 1) as f32;
    if !b0.is_finite() || !b1.is_finite() {
        return 0.0;
    }
    if b1 - b0 < 1.0 {
        let center = ((b0 + b1) * 0.5).clamp(1.0, top);
        let i = center as usize;
        let t = center - i as f32;
        let a = mags[i];
        let b = mags[(i + 1).min(half - 1)];
        a + (b - a) * t
    } else {
        let from = (b0.max(1.0) as usize).clamp(1, half - 1);
        let to = ((b1.max(0.0).ceil() as usize).max(from + 1)).min(half);
        mags[from..to].iter().copied().fold(0.0f32, f32::max)
    }
}

/// A mismatch clears the ring: stored columns were reduced under the old
/// mapping and can't be reinterpreted.
#[derive(Clone, PartialEq)]
struct Mapping {
    rate: u32,
    fft: usize,
    lo: f32,
    hi: f32,
    log: bool,
    floor: f32,
    ceil: f32,
    history: usize,
}

/// Stored as a position in the dB window, so the history's size is
/// independent of the FFT size and a bake is a lookup per cell.
fn reduce(mags: &[f32], map: &Mapping, out: &mut [f32]) {
    let nyquist = (map.rate.clamp(8_000, 384_000) as f32) / 2.0;
    let half = mags.len() as f32;
    let span = map.ceil - map.floor;
    for (r, slot) in out.iter_mut().enumerate() {
        let f0 = freq_at(r as f32 / ROWS as f32, map.lo, map.hi, map.log);
        let f1 = freq_at((r + 1) as f32 / ROWS as f32, map.lo, map.hi, map.log);
        let mag = fold(mags, f0 / nyquist * half, f1 / nyquist * half);
        let db = 20.0 * (mag.max(0.0) + 1e-9).log10();
        let t = (db - map.floor) / span;
        *slot = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    }
}

fn cell_color(map: Colormap, t: f32) -> [u8; 3] {
    match map {
        Colormap::Magma => sample_stops(&MAGMA, t),
        Colormap::Viridis => sample_stops(&VIRIDIS, t),
        Colormap::Ice => sample_stops(&ICE, t),
        Colormap::Grayscale => sample_stops(&GRAYSCALE, t),
        Colormap::Theme => heat_color(palette::bg_root(), palette::accent(), palette::accent(), t),
        Colormap::Cover => heat_color(
            palette::bg_root(),
            palette::accent(),
            palette::highlight(),
            t,
        ),
    }
}

/// Past the middle, so most of the travel is the climb out of the
/// background.
const HEAT_MID: f32 = 0.6;

/// A seed already at the end still gets a ramp that travels.
const HEAT_PEAK_L: f32 = 0.96;
const HEAT_GAP_L: f32 = 0.15;

/// The bright end is mostly white with the hue still in it.
const HEAT_TOP_C: f32 = 0.35;

/// The panel background at the quiet end, `seed` through the middle, a
/// wash of `top`'s hue at the loud end. Not [`crate::spectrum::ramp_color`]:
/// its quiet end is a visible color, which would wash the whole heatmap.
/// OkLCh so lightness climbs evenly even when the cover's two colors are
/// equally bright.
fn heat_color(floor: Rgba, seed: Rgba, top: Rgba, t: f32) -> [u8; 3] {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    let (floor_l, _, _) = palette::rgba_to_oklch(floor);
    let (seed_l, seed_c, seed_h) = palette::rgba_to_oklch(seed);
    let (_, top_c, top_h) = palette::rgba_to_oklch(top);
    // A light panel sits near the top of the lightness range, so the ramp
    // runs toward dark instead.
    let peak_l = if floor_l < 0.5 {
        HEAT_PEAK_L
    } else {
        1.0 - HEAT_PEAK_L
    };
    // Held off both ends so neither half of the ramp collapses.
    let lo = floor_l.min(peak_l) + HEAT_GAP_L;
    let hi = (floor_l.max(peak_l) - HEAT_GAP_L).max(lo);
    let mid_l = sane(seed_l, lo, hi, lo);
    let lerp = |a: f32, b: f32, k: f32| a + (b - a) * k;
    let (l, c, h) = if t < HEAT_MID {
        let k = t / HEAT_MID;
        // No chroma at the floor, which is the panel background.
        (lerp(floor_l, mid_l, k), lerp(0.0, seed_c, k), seed_h)
    } else {
        let k = (t - HEAT_MID) / (1.0 - HEAT_MID);
        (
            lerp(mid_l, peak_l, k),
            lerp(seed_c, top_c * HEAT_TOP_C, k),
            hue_lerp(seed_h, top_h, k),
        )
    };
    let color = palette::oklch_to_rgba(l, c.max(0.0), h, 1.0);
    let byte = |v: f32| (sane(v, 0.0, 1.0, 0.0) * 255.0).round() as u8;
    [byte(color.r), byte(color.g), byte(color.b)]
}

/// The short way round, or the ramp crosses half the spectrum.
fn hue_lerp(from: f32, to: f32, t: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let delta = (to - from + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI;
    from + delta * t
}

fn sample_stops(stops: &[[u8; 3]], t: f32) -> [u8; 3] {
    let last = stops.len() - 1;
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    let pos = t * last as f32;
    let i = (pos as usize).min(last);
    let f = pos - i as f32;
    let a = stops[i];
    let b = stops[(i + 1).min(last)];
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f).round() as u8;
    [lerp(a[0], b[0]), lerp(a[1], b[1]), lerp(a[2], b[2])]
}

/// The three samples catch a theme or cover change under the Theme and
/// Cover maps.
#[derive(PartialEq)]
struct Skin {
    colormap: Colormap,
    direction: Orientation,
    ends: [[u8; 3]; 3],
}

impl Skin {
    fn of(config: &SpectrogramConfig) -> Self {
        Skin {
            colormap: config.colormap,
            direction: config.direction,
            ends: [
                cell_color(config.colormap, 0.0),
                cell_color(config.colormap, 0.5),
                cell_color(config.colormap, 1.0),
            ],
        }
    }
}

struct Waterfall {
    last_written: u64,
    last_tick: Option<Instant>,
    last_fresh: Option<Instant>,
    mapping: Option<Mapping>,
    /// Held between pump ticks, so a frame with no audio doesn't re-run the
    /// FFT.
    rows: Vec<f32>,
    cells: Vec<f32>,
    /// `head` is the next slot, which is also the oldest column.
    history: usize,
    head: usize,
    /// Once it reaches the history the picture is all silence.
    quiet: usize,
    accum: f32,
    /// The standing texture and the one it replaced.
    image: Option<Arc<RenderImage>>,
    retired: Option<Arc<RenderImage>>,
    skin: Option<Skin>,
    dirty: bool,
    /// BGRA, rebuilt when the skin changes.
    lut: Vec<[u8; 4]>,
    /// Kept between bakes, so a scroll shifts them and recolors only the new
    /// columns.
    raw: Vec<u8>,
    /// `usize::MAX` forces a full rebuild.
    pending: usize,
    alive: bool,
}

impl Waterfall {
    fn new() -> Self {
        Waterfall {
            last_written: 0,
            last_tick: None,
            last_fresh: None,
            mapping: None,
            rows: Vec::new(),
            cells: Vec::new(),
            history: 0,
            head: 0,
            quiet: 0,
            accum: 0.0,
            image: None,
            retired: None,
            skin: None,
            dirty: false,
            lut: Vec::new(),
            raw: Vec::new(),
            pending: usize::MAX,
            alive: false,
        }
    }

    fn reset(&mut self, mapping: &Mapping) {
        self.rows = vec![0.0; ROWS];
        self.cells = vec![0.0; mapping.history * ROWS];
        self.history = mapping.history;
        self.head = 0;
        self.quiet = mapping.history;
        self.accum = 0.0;
        self.dirty = true;
        self.pending = usize::MAX;
        self.mapping = Some(mapping.clone());
    }

    /// Stopped audio scrolls silence in until the panel is empty, unless
    /// `hold` keeps the picture.
    fn step(&mut self, feed: &AudioFeed, config: &SpectrogramConfig, hold: bool) {
        let (lo, hi) = config.range();
        let (floor, ceil) = config.db_window();
        let mapping = Mapping {
            rate: feed.sample_rate(),
            fft: config.fft(),
            lo,
            hi,
            log: config.log_scale,
            floor,
            ceil,
            history: config.history(),
        };
        if self.mapping.as_ref() != Some(&mapping) {
            self.reset(&mapping);
        }

        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32().min(0.1))
            .unwrap_or(1.0 / 60.0);
        self.last_tick = Some(now);

        let written = feed.written();
        let fresh = written != self.last_written;
        self.last_written = written;

        // An axis edit while frozen clears the ring and leaves the panel empty
        // until playback resumes, rather than redrawing columns under the wrong
        // mapping.
        if hold && !fresh {
            self.alive = false;
            return;
        }

        if fresh {
            self.last_fresh = Some(now);
            self.analyze(feed, &mapping);
        }
        let stopped = self
            .last_fresh
            .is_none_or(|t| (now - t).as_secs_f32() > SILENT_AFTER);
        if stopped {
            self.rows.fill(0.0);
        }

        let due = columns_due(&mut self.accum, dt, config.speed()).min(mapping.history);
        for _ in 0..due {
            self.push_column();
        }
        self.alive = self.quiet < self.history;
    }

    /// The feed has no spectrum while it fills, so the previous column stands.
    fn analyze(&mut self, feed: &AudioFeed, mapping: &Mapping) {
        let Some(mags) = feed.magnitudes(mapping.fft) else {
            return;
        };
        reduce(&mags, mapping, &mut self.rows);
    }

    fn push_column(&mut self) {
        if self.history == 0 || self.cells.len() < self.history * ROWS || self.rows.len() < ROWS {
            return;
        }
        let silent = self.rows.iter().all(|&v| v <= 0.0);
        if silent && self.quiet >= self.history {
            // Already all silence: another empty column changes no pixel.
            return;
        }
        let base = self.head * ROWS;
        self.cells[base..base + ROWS].copy_from_slice(&self.rows[..ROWS]);
        self.head = (self.head + 1) % self.history;
        self.quiet = if silent { self.quiet + 1 } else { 0 };
        self.dirty = true;
        self.pending = self.pending.saturating_add(1);
    }

    /// One texel per column and row, scaled by the renderer; sizing to the
    /// panel would rebuild on every resize. The steady case shifts the pixels
    /// and recolors only what arrived; a skin or mapping change rebuilds all.
    fn bake(&mut self, config: &SpectrogramConfig) -> Option<Arc<RenderImage>> {
        let history = self.history;
        if history == 0 || self.cells.len() < history * ROWS {
            return None;
        }
        // BGRA, the renderer's order, the same swizzle the backdrop's bake uses.
        if self.lut.len() != LUT_STEPS {
            self.lut = (0..LUT_STEPS)
                .map(|i| {
                    let [r, g, b] = cell_color(config.colormap, i as f32 / (LUT_STEPS - 1) as f32);
                    [b, g, r, 0xff]
                })
                .collect();
        }

        // Low frequencies sit at the bottom of an upright axis and at the left of
        // a flat one.
        let flat = config.direction.horizontal();
        let (w, h) = if flat {
            (ROWS, history)
        } else {
            (history, ROWS)
        };
        let stride = w * 4;
        let fresh = if self.pending >= history || self.raw.len() != w * h * 4 {
            self.raw.resize(w * h * 4, 0);
            0..history
        } else {
            // After the shift every kept column sits where the full loop would put
            // it, so both paths share the write-out below.
            let k = self.pending;
            match config.direction {
                Orientation::Right => {
                    for row in 0..h {
                        let at = row * stride;
                        self.raw.copy_within(at + k * 4..at + stride, at);
                    }
                }
                Orientation::Left => {
                    for row in 0..h {
                        let at = row * stride;
                        self.raw.copy_within(at..at + stride - k * 4, at + k * 4);
                    }
                }
                Orientation::Bottom => self.raw.copy_within(k * stride.., 0),
                Orientation::Top => self.raw.copy_within(..(h - k) * stride, k * stride),
            }
            history - k..history
        };
        for i in fresh {
            let slot = (self.head + i) % history;
            let base = slot * ROWS;
            for row in 0..ROWS {
                let t = self.cells[base + row];
                let step = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
                let color = self.lut[((step * (LUT_STEPS - 1) as f32) as usize).min(LUT_STEPS - 1)];
                let (x, y) = match config.direction {
                    Orientation::Right => (i, ROWS - 1 - row),
                    Orientation::Left => (history - 1 - i, ROWS - 1 - row),
                    Orientation::Bottom => (row, i),
                    Orientation::Top => (row, history - 1 - i),
                };
                let at = (y * w + x) * 4;
                self.raw[at..at + 4].copy_from_slice(&color);
            }
        }
        self.pending = 0;
        let buf = RgbaImage::from_raw(w as u32, h as u32, self.raw.clone())?;
        Some(Arc::new(RenderImage::new(vec![Frame::new(buf)])))
    }

    fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
        config: &SpectrogramConfig,
    ) {
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        if w <= 0.0 || h <= 0.0 {
            return;
        }

        // Keep this. RenderImage::new mints a fresh ImageId, and gpui's sprite
        // atlas holds each tile until it's handed back: without the drop, 40 new
        // tiles a second exhaust the atlas within seconds. It runs one paint late
        // so the tile outlives the frame that still points at it.
        if let Some(old) = self.retired.take() {
            let _ = window.drop_image(old);
        }

        let skin = Skin::of(config);
        if self.skin.as_ref() != Some(&skin) {
            self.skin = Some(skin);
            self.lut.clear();
            self.pending = usize::MAX;
            self.dirty = true;
        }
        if self.dirty {
            self.dirty = false;
            if let Some(image) = self.bake(config) {
                self.retired = self.image.replace(image);
            }
        }
        if let Some(image) = self.image.clone() {
            let _ = window.paint_image(bounds, Corners::default(), image, 0, false);
        }
        if config.grid || config.labels {
            paint_scale(bounds, window, cx, config);
        }
    }
}

/// Tags only once the panel can spread them, and one that would land on
/// the previous tag is dropped.
fn paint_scale(
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
    config: &SpectrogramConfig,
) {
    let (lo, hi) = config.range();
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let ox = f32::from(bounds.origin.x);
    let oy = f32::from(bounds.origin.y);
    let flat = config.direction.horizontal();
    let (along, across) = if flat { (w, h) } else { (h, w) };
    if along < 24.0 || across < 8.0 {
        return;
    }

    let font = window.text_style().font();
    let color: Hsla = palette::text_muted().into();
    let fs = px((9.0 * palette::font_scale()).max(8.0));
    let fh = f32::from(fs);
    let tags = config.labels && along >= 72.0 && across >= 40.0;
    let rule = palette::alpha(palette::gridline(), 0x33);
    let mut last: Option<f32> = None;

    for (hz, _, major) in hz_ladder(lo, hi) {
        if !major {
            continue;
        }
        let frac = config.axis_frac(hz);
        if !(0.0..=1.0).contains(&frac) {
            continue;
        }
        let (rx, ry, rw, rh) = if flat {
            (ox + frac * w, oy, 1.0, h)
        } else {
            (ox, oy + h - frac * h, w, 1.0)
        };
        if config.grid {
            window.paint_quad(fill(
                Bounds::new(point(px(rx), px(ry)), size(px(rw), px(rh))),
                rule,
            ));
        }
        if !tags {
            continue;
        }
        let along_pos = if flat { rx } else { ry };
        if last.is_some_and(|prev: f32| (along_pos - prev).abs() < fh + 4.0) {
            continue;
        }
        last = Some(along_pos);

        let text: SharedString = fmt_axis_hz(hz).into();
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(text, fs, &[run], None);
        let lw = f32::from(line.width);
        let (tx, ty) = if flat {
            (rx + 3.0, oy + 2.0)
        } else {
            (ox + 3.0, ry - fh - 1.0)
        };
        let tx = tx.clamp(ox, (ox + w - lw).max(ox));
        let ty = ty.clamp(oy, (oy + h - fh).max(oy));
        let _ = line.paint(point(px(tx), px(ty)), fs, window, cx);
    }
}

pub struct SpectrogramPanel {
    state: AppState,
    config: SpectrogramConfig,
    feed: Arc<AudioFeed>,
    view: Arc<Mutex<Waterfall>>,
    lo_scrub: ScrubState,
    hi_scrub: ScrubState,
    floor_scrub: ScrubState,
    ceil_scrub: ScrubState,
    speed_scrub: ScrubState,
    history_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl SpectrogramPanel {
    pub fn new(state: AppState, config: SpectrogramConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        SpectrogramPanel {
            config,
            feed: state.player.read(cx).feed(),
            state,
            view: Arc::new(Mutex::new(Waterfall::new())),
            lo_scrub: ScrubState::default(),
            hi_scrub: ScrubState::default(),
            floor_scrub: ScrubState::default(),
            ceil_scrub: ScrubState::default(),
            speed_scrub: ScrubState::default(),
            history_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
        }
    }

    /// Stops a min-span short of the high bound so the axis never inverts.
    fn set_lo_hz(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let hi = sane(self.config.hi_hz, HZ_MIN, HZ_MAX, 16_000.0);
        let ceil = (hi / MIN_RATIO).max(HZ_MIN);
        self.config.lo_hz = frac_to_hz(fraction).clamp(HZ_MIN, ceil);
        cx.notify();
    }

    fn set_hi_hz(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let lo = sane(self.config.lo_hz, HZ_MIN, HZ_MAX, 40.0);
        let floor = (lo * MIN_RATIO).min(HZ_MAX);
        self.config.hi_hz = frac_to_hz(fraction).clamp(floor, HZ_MAX);
        cx.notify();
    }

    fn set_floor_db(&mut self, db: f32, cx: &mut Context<Self>) {
        self.config.floor_db = db;
        cx.notify();
    }

    fn set_ceil_db(&mut self, db: f32, cx: &mut Context<Self>) {
        self.config.ceil_db = db;
        cx.notify();
    }

    fn set_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        self.config.speed = speed;
        cx.notify();
    }

    fn set_history(&mut self, columns: f32, cx: &mut Context<Self>) {
        // A NaN from the typed readout casts to zero, and the clamp catches it.
        self.config.history = (columns.round() as i64).clamp(0, HISTORY_MAX as i64) as usize;
        cx.notify();
    }

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

    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        type ConfigToggle = (
            SharedString,
            fn(&SpectrogramPanel) -> bool,
            fn(&mut SpectrogramPanel),
        );
        let toggles: Vec<ConfigToggle> = vec![
            (
                rox_i18n::t!("spectrogram-grid"),
                |this| this.config.grid,
                |this| this.config.grid = !this.config.grid,
            ),
            (
                rox_i18n::t!("spectrogram-labels"),
                |this| this.config.labels,
                |this| this.config.labels = !this.config.labels,
            ),
            (
                rox_i18n::t!("spectrogram-log-scale"),
                |this| this.config.log_scale,
                |this| this.config.log_scale = !this.config.log_scale,
            ),
        ];
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            for (label, is_on, set) in toggles {
                submenu = submenu.item(panel::check_row(
                    label,
                    None,
                    is_on,
                    move |this, _| set(this),
                    &panel,
                ));
            }
            submenu
        });
        menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("library-menu-display"),
            submenu,
        ))
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The observe re-renders on every pump tick while audio moves. Frame
        // polling only scrolls the silence in after audio stops, then the panel
        // parks.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        // Paused mid-session, not a played-out queue.
        let hold = self.config.freeze && session && !playing && !player.queue_ended();
        if !playing && self.view.lock().unwrap().alive {
            window.request_animation_frame();
        }

        let config = self.config.clone();
        let view = self.view.clone();
        let feed = self.feed.clone();
        div().size_full().relative().bg(palette::bg_root()).child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, cx| {
                    let mut view = view.lock().unwrap();
                    view.step(&feed, &config, hold);
                    view.paint(bounds, window, cx, &config);
                },
            )
            .size_full(),
        )
    }
}

impl PanelSettings for SpectrogramPanel {
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (floor, ceil) = self.config.db_window();
        let speed = self.config.speed();
        let history = self.config.history() as f32;
        let analysis = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("spectrogram-fft-size"),
                Some(rox_i18n::t!("spectrogram-fft-size.description")),
                choices(
                    FFT_CHOICES,
                    self.config.fft(),
                    |this: &mut Self, size, cx| {
                        this.config.fft_size = size;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrogram-low-bound"),
                Some(rox_i18n::t!("spectrogram-low-bound.description")),
                self.freq_slider(&self.lo_scrub, self.config.lo_hz, Self::set_lo_hz, cx),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrogram-high-bound"),
                Some(rox_i18n::t!("spectrogram-high-bound.description")),
                self.freq_slider(&self.hi_scrub, self.config.hi_hz, Self::set_hi_hz, cx),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrogram-log-scale"),
                Some(rox_i18n::t!("spectrogram-log-scale.description")),
                toggle(
                    self.config.log_scale,
                    |this: &mut Self, on, cx| {
                        this.config.log_scale = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrogram-floor"),
                Some(rox_i18n::t!("spectrogram-floor.description")),
                settings_ui::scalar(
                    &self.floor_scrub,
                    &self.value_edit,
                    floor,
                    settings_ui::span(FLOOR_MIN, FLOOR_MAX, " dB").hard(),
                    Self::set_floor_db,
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrogram-ceiling"),
                Some(rox_i18n::t!("spectrogram-ceiling.description")),
                settings_ui::scalar(
                    &self.ceil_scrub,
                    &self.value_edit,
                    ceil,
                    settings_ui::span(CEIL_MIN, CEIL_MAX, " dB").hard(),
                    Self::set_ceil_db,
                    cx,
                ),
            ));
        let picture = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("spectrogram-speed"),
                Some(rox_i18n::t!("spectrogram-speed.description")),
                settings_ui::scalar(
                    &self.speed_scrub,
                    &self.value_edit,
                    speed,
                    settings_ui::span(SPEED_MIN, SPEED_MAX, " col/s").hard(),
                    Self::set_speed,
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrogram-history"),
                Some(rox_i18n::t!("spectrogram-history.description")),
                settings_ui::scalar(
                    &self.history_scrub,
                    &self.value_edit,
                    history,
                    settings_ui::span(HISTORY_MIN as f32, HISTORY_MAX as f32, " col").hard(),
                    Self::set_history,
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrogram-colormap"),
                Some(rox_i18n::t!("spectrogram-colormap.description")),
                choices_shared(
                    &colormap_choices(),
                    self.config.colormap,
                    |this: &mut Self, colormap, cx| {
                        this.config.colormap = colormap;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrogram-direction"),
                Some(rox_i18n::t!("spectrogram-direction.description")),
                choices_shared(
                    &orientation_choices(),
                    self.config.direction,
                    |this: &mut Self, direction, cx| {
                        this.config.direction = direction;
                        cx.notify();
                    },
                    cx,
                ),
            ));
        let scale = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("spectrogram-grid"),
                Some(rox_i18n::t!("spectrogram-grid.description")),
                toggle(
                    self.config.grid,
                    |this: &mut Self, on, cx| {
                        this.config.grid = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("spectrogram-labels"),
                Some(rox_i18n::t!("spectrogram-labels.description")),
                toggle(
                    self.config.labels,
                    |this: &mut Self, on, cx| {
                        this.config.labels = on;
                        cx.notify();
                    },
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
                rox_i18n::t!("spectrogram-section-picture"),
                None,
                picture,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-scale"),
                None,
                scale,
            ))
            .into_any_element()
    }

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(
            settings_ui::section(
                rox_i18n::t!("viz-section-playback"),
                None,
                setting_row(
                    rox_i18n::t!("spectrogram-hold-on-pause"),
                    Some(rox_i18n::t!("spectrogram-hold-on-pause.description")),
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

impl EventEmitter<PanelEvent> for SpectrogramPanel {}

impl Focusable for SpectrogramPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for SpectrogramPanel {
    fn panel_name(&self) -> &'static str {
        "spectrogram"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-spectrogram"),
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
        crate::panel::chrome_min_size(&self.config.chrome, gpui::size(MIN_SIDE, MIN_SIDE))
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
        let menu = self.config_menu(menu, window, cx);
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
                SpectrogramPanel::new(state, config, cx)
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

impl Render for SpectrogramPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(history: usize) -> Mapping {
        Mapping {
            rate: 48_000,
            fft: 1024,
            lo: 40.0,
            hi: 16_000.0,
            log: true,
            floor: -90.0,
            ceil: -20.0,
            history,
        }
    }

    #[test]
    fn a_narrow_row_interpolates_between_its_neighbours() {
        let mags = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        assert!((fold(&mags, 3.4, 3.6) - 0.5).abs() < 1e-6);
        assert!((fold(&mags, 3.9, 4.1) - 1.0).abs() < 1e-6);
        assert!((fold(&mags, 3.2, 3.3) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_wide_row_keeps_the_narrow_partial() {
        let mut mags = [0.0f32; 32];
        mags[7] = 1.0;
        assert_eq!(fold(&mags, 2.0, 12.0), 1.0);
        // A span that misses it stays at the floor.
        assert_eq!(fold(&mags, 12.0, 24.0), 0.0);
    }

    #[test]
    fn the_dc_bin_never_reaches_a_row() {
        let mut mags = [0.0f32; 16];
        mags[0] = 1.0;
        assert_eq!(fold(&mags, 0.0, 0.4), 0.0);
        assert_eq!(fold(&mags, 0.0, 4.0), 0.0);
    }

    #[test]
    fn a_junk_bin_span_folds_to_the_floor() {
        let mags = [0.5f32; 16];
        assert_eq!(fold(&mags, f32::NAN, 4.0), 0.0);
        assert_eq!(fold(&mags, 0.0, f32::INFINITY), 0.0);
        assert_eq!(fold(&[], 0.0, 4.0), 0.0);
    }

    #[test]
    fn a_tone_lands_in_its_own_row() {
        let map = mapping(128);
        let half = 512;
        let mut mags = vec![0.0f32; half];
        // 1 kHz at 48 kHz over a 1024 window: bin 1000 / (24000 / 512).
        let bin = (1000.0 / 24_000.0 * half as f32) as usize;
        mags[bin] = 1.0;
        let mut rows = vec![0.0f32; ROWS];
        reduce(&mags, &map, &mut rows);
        let hot = rows
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap()
            .0;
        let f0 = freq_at(hot as f32 / ROWS as f32, map.lo, map.hi, map.log);
        let f1 = freq_at((hot + 1) as f32 / ROWS as f32, map.lo, map.hi, map.log);
        // The bin is 46.9 Hz wide, so the lit row is the bin's, not exactly
        // 1 kHz's.
        assert!(
            (f0 - 100.0..f1 + 100.0).contains(&1000.0),
            "1 kHz should light the row covering {f0}..{f1}"
        );
        assert_eq!(rows[hot], 1.0);
        assert_eq!(rows[0], 0.0);
    }

    #[test]
    fn the_ring_drops_the_oldest_column_on_wrap() {
        let mut view = Waterfall::new();
        view.reset(&mapping(4));
        for v in [0.1f32, 0.2, 0.3, 0.4, 0.5] {
            view.rows.fill(v);
            view.push_column();
        }
        // Five columns into four slots: the first is gone.
        let read: Vec<f32> = (0..4)
            .map(|i| view.cells[((view.head + i) % 4) * ROWS])
            .collect();
        assert_eq!(read, vec![0.2, 0.3, 0.4, 0.5]);
        assert_eq!(view.quiet, 0);
    }

    #[test]
    fn a_silent_ring_stops_taking_columns() {
        let mut view = Waterfall::new();
        view.reset(&mapping(4));
        assert_eq!(view.quiet, 4);
        view.dirty = false;
        for _ in 0..8 {
            view.push_column();
        }
        assert!(!view.dirty);
        assert_eq!(view.head, 0);

        // A loud column wakes it, and the silence after scrolls until empty again.
        view.rows.fill(0.7);
        view.push_column();
        assert_eq!(view.quiet, 0);
        view.rows.fill(0.0);
        for _ in 0..4 {
            view.push_column();
        }
        assert_eq!(view.quiet, 4);
    }

    #[test]
    fn an_incremental_bake_matches_a_full_one() {
        for (i, direction) in [
            Orientation::Right,
            Orientation::Left,
            Orientation::Bottom,
            Orientation::Top,
        ]
        .into_iter()
        .enumerate()
        {
            let config = SpectrogramConfig {
                colormap: Colormap::Viridis,
                direction,
                ..SpectrogramConfig::default()
            };
            let mut inc = Waterfall::new();
            let mut all = Waterfall::new();
            inc.reset(&mapping(6));
            all.reset(&mapping(6));
            for step in 0..9 {
                for (row, v) in inc.rows.iter_mut().enumerate() {
                    *v = ((step + row) % 5) as f32 / 4.0;
                }
                all.rows.clone_from(&inc.rows);
                inc.push_column();
                all.push_column();
                // Every other push leaves batches of two, the shape a slow frame hands
                // the incremental path.
                if step % 2 == 0 {
                    assert!(inc.bake(&config).is_some());
                }
            }
            assert!(inc.bake(&config).is_some());
            assert!(all.bake(&config).is_some());
            assert_eq!(inc.raw, all.raw, "direction {i} diverged");
        }
    }

    /// Dropping the fraction each tick would leave the fast run short.
    #[test]
    fn columns_follow_the_wall_clock_not_the_frame_rate() {
        let mut slow = 0.0;
        let slow_total: usize = (0..3).map(|_| columns_due(&mut slow, 0.5, 5.0)).sum();
        let mut fast = 0.0;
        let fast_total: usize = (0..6).map(|_| columns_due(&mut fast, 0.25, 5.0)).sum();
        assert_eq!(slow_total, 7);
        assert_eq!(fast_total, 7);
    }

    #[test]
    fn a_tick_shorter_than_a_column_still_carries() {
        let mut accum = 0.0;
        for _ in 0..3 {
            assert_eq!(columns_due(&mut accum, 0.25, 1.0), 0);
        }
        assert_eq!(columns_due(&mut accum, 0.25, 1.0), 1);
    }

    #[test]
    fn a_junk_tick_scrolls_nothing() {
        let mut accum = 0.0;
        assert_eq!(columns_due(&mut accum, f32::NAN, 40.0), 0);
        assert_eq!(accum, 0.0);
        assert_eq!(columns_due(&mut accum, 0.5, 5.0), 2);
    }

    #[test]
    fn the_colormaps_run_from_their_floor_to_their_top() {
        assert_eq!(cell_color(Colormap::Grayscale, 0.0), [0, 0, 0]);
        assert_eq!(cell_color(Colormap::Grayscale, 1.0), [255, 255, 255]);
        assert_eq!(cell_color(Colormap::Magma, 0.0), MAGMA[0]);
        assert_eq!(cell_color(Colormap::Magma, 1.0), MAGMA[MAGMA.len() - 1]);
        assert_eq!(cell_color(Colormap::Viridis, 0.0), VIRIDIS[0]);
        assert_eq!(
            cell_color(Colormap::Viridis, 1.0),
            VIRIDIS[VIRIDIS.len() - 1]
        );
        assert_eq!(cell_color(Colormap::Ice, 0.0), ICE[0]);
        assert_eq!(cell_color(Colormap::Ice, 1.0), ICE[ICE.len() - 1]);
        // An interior anchor is hit exactly.
        assert_eq!(cell_color(Colormap::Magma, 0.5), MAGMA[2]);
    }

    #[test]
    fn a_colormap_position_off_the_ends_stays_on_the_ramp() {
        assert_eq!(cell_color(Colormap::Grayscale, -5.0), [0, 0, 0]);
        assert_eq!(cell_color(Colormap::Grayscale, 9.0), [255, 255, 255]);
        assert_eq!(cell_color(Colormap::Grayscale, f32::NAN), [0, 0, 0]);
        assert_eq!(cell_color(Colormap::Magma, f32::INFINITY), MAGMA[4]);
    }

    fn heat_lightness(floor: Rgba, seed: Rgba, top: Rgba) -> Vec<f32> {
        (0..=10)
            .map(|i| {
                let [r, g, b] = heat_color(floor, seed, top, i as f32 / 10.0);
                let color = Rgba {
                    r: r as f32 / 255.0,
                    g: g as f32 / 255.0,
                    b: b as f32 / 255.0,
                    a: 1.0,
                };
                palette::rgba_to_oklch(color).0
            })
            .collect()
    }

    /// Both themes, since the ramp's direction depends on the background.
    #[test]
    fn the_palette_colormaps_walk_away_from_the_background() {
        let accent = gpui::rgb(0xffb300);
        let highlight = gpui::rgb(0xfacc15);
        for (floor, rising) in [(gpui::rgb(0x121212), true), (gpui::rgb(0xededed), false)] {
            for top in [accent, highlight] {
                let steps = heat_lightness(floor, accent, top);
                for pair in steps.windows(2) {
                    let moved = if rising {
                        pair[1] - pair[0]
                    } else {
                        pair[0] - pair[1]
                    };
                    assert!(moved >= -0.01, "the ramp turned back on itself: {steps:?}");
                }
                let start = palette::rgba_to_oklch(floor).0;
                assert!((steps[0] - start).abs() < 0.02, "the floor isn't the panel");
                assert!(
                    (steps[10] - start).abs() > 0.5,
                    "the ramp never left the background: {steps:?}"
                );
            }
        }
    }

    #[test]
    fn the_ramp_takes_the_short_way_between_hues() {
        let pi = std::f32::consts::PI;
        let mid = hue_lerp(pi - 0.1, -pi + 0.1, 0.5);
        assert!(
            mid.abs() > pi - 0.05,
            "the hue crossed the whole wheel: {mid}"
        );
    }

    #[test]
    fn config_accessors_swallow_junk() {
        let config = SpectrogramConfig {
            lo_hz: f32::NAN,
            hi_hz: -20.0,
            floor_db: f32::INFINITY,
            ceil_db: f32::NAN,
            speed: -5.0,
            history: 4,
            ..SpectrogramConfig::default()
        };
        let (lo, hi) = config.range();
        assert!(lo.is_finite() && hi.is_finite());
        assert!(hi >= lo * MIN_RATIO, "the axis inverted: {lo}..{hi}");
        let (floor, ceil) = config.db_window();
        assert!(ceil >= floor + MIN_DB_SPAN, "the dB window collapsed");
        assert_eq!(config.speed(), SPEED_MIN);
        assert_eq!(config.history(), HISTORY_MIN);
    }

    #[test]
    fn the_axis_never_inverts_from_either_end() {
        for (lo_hz, hi_hz) in [(20_000.0, 30.0), (HZ_MAX, HZ_MAX), (HZ_MIN, HZ_MIN)] {
            let config = SpectrogramConfig {
                lo_hz,
                hi_hz,
                ..SpectrogramConfig::default()
            };
            let (lo, hi) = config.range();
            assert!(lo >= HZ_MIN && hi <= HZ_MAX);
            assert!(hi >= lo * MIN_RATIO, "{lo_hz}..{hi_hz} gave {lo}..{hi}");
            assert!((config.axis_frac(lo)).abs() < 1e-4);
            assert!((config.axis_frac(hi) - 1.0).abs() < 1e-4);
        }
    }

    /// A hand-edited size must not panic or blank the panel.
    #[test]
    fn a_hand_edited_fft_size_cant_panic_or_reach_the_analyzer() {
        for size in [0, 1, 300, 1023, 5000, usize::MAX / 2, usize::MAX] {
            let config = SpectrogramConfig {
                fft_size: size,
                ..SpectrogramConfig::default()
            };
            let fft = config.fft();
            assert!(fft.is_power_of_two(), "{size} gave {fft}");
            assert!(
                (MIN_FFT_SIZE..=MAX_FFT_SIZE).contains(&fft),
                "{size} gave {fft}"
            );
        }
        for &(_, size) in FFT_CHOICES {
            let config = SpectrogramConfig {
                fft_size: size,
                ..SpectrogramConfig::default()
            };
            assert_eq!(config.fft(), size);
        }
    }

    #[test]
    fn the_history_stays_inside_its_bounds() {
        for asked in [0, 1, HISTORY_MIN, 900, HISTORY_MAX, usize::MAX] {
            let config = SpectrogramConfig {
                history: asked,
                ..SpectrogramConfig::default()
            };
            assert!((HISTORY_MIN..=HISTORY_MAX).contains(&config.history()));
        }
    }
}
