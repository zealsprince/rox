//! The spectrogram panel: the player's PCM tap as a scrolling waterfall, with
//! frequency across one axis, time across the other, and loudness as color.
//! The spectrum panel beside it shows one instant and the oscilloscope shows
//! one window of samples, so both forget everything the moment the next frame
//! lands. This one keeps the last few seconds standing, and that's the whole
//! point of it: a melody line draws itself as a moving ridge, a snare prints
//! as a full-height stripe, a filter sweep climbs, and a track's texture
//! becomes a shape you can read instead of a flicker you can only feel.
//!
//! It's also the one audio view that can't be drawn with paint primitives. A
//! per-pixel heatmap over a 600x300 panel is tens of thousands of quads a
//! frame, which is not a thing to ask of the UI thread. So the history lives
//! in a ring of reduced columns, and a new column bakes it into a small
//! [`RenderImage`] that the renderer scales to whatever size the panel
//! happens to be. Frames between columns repaint the same texture, which
//! uploads nothing, and once the audio stops and the last of it has scrolled
//! off the panel parks and stops asking for frames.
//!
//! What it deliberately doesn't do: no peak tracking, no per-column
//! normalization, no interpolation between columns to smooth the scroll. The
//! dB window is whatever the config says rather than whatever the loudest
//! thing on screen is, so two tracks look different when they are different
//! and a quiet passage reads as quiet instead of being stretched to fill the
//! ramp.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, fill, point, prelude::*, px, size, AnyElement, App, Bounds, Context, Corners, Div,
    EventEmitter, FocusHandle, Focusable, Hsla, Pixels, RenderImage, SharedString, Subscription,
    TextRun, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use image::{Frame, RgbaImage};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_panel_kit::axis::{fmt_axis_hz, fmt_hz};
use serde::{Deserialize, Serialize};

use rox_viz::analysis::{hz_ladder, Analyzer, MAX_FFT_SIZE, MIN_FFT_SIZE};
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, choices, choices_shared, setting_row, toggle, AppState, PanelChrome, PanelSettings,
    ScrubState,
};
use crate::panel_settings;
use crate::settings::ui as settings_ui;
use crate::spectrum::{orientation_choices, ramp_color, Gradient, Orientation};

/// The frequency resolution a stored column keeps, rows. Fixed rather than
/// following the FFT size, so the window size and the history's memory are
/// independent knobs: 256 rows is finer than any panel this sits in resolves
/// vertically, and the renderer's own filtering handles the rest.
const ROWS: usize = 256;

/// How many steps the colormap is sampled into for a bake. Past this the eye
/// stops reading the difference, and the lookup is what keeps the per-pixel
/// loop down to a copy.
const LUT_STEPS: usize = 256;

/// The frequency bounds the sliders (and a hand-edited config) may pick
/// between: below the bottom of hearing up to a typical Nyquist ceiling.
const HZ_MIN: f32 = 10.0;
const HZ_MAX: f32 = 24_000.0;

/// The smallest span the low and high bounds keep between them, so the log
/// mapping always has room and can never invert.
const MIN_RATIO: f32 = 2.0;

/// The dB window sliders' spans, on magnitudes where a full-scale sine is
/// 0 dB. The floor is the quiet end that maps to the colormap's dark stop,
/// the ceiling the loud end that maps to its bright one.
const FLOOR_MIN: f32 = -120.0;
const FLOOR_MAX: f32 = -40.0;
const CEIL_MIN: f32 = -40.0;
const CEIL_MAX: f32 = 0.0;

/// The narrowest the dB window may be squeezed to. The two sliders overlap at
/// -40, so without this a config could collapse the window onto one value and
/// leave every cell at the same end of the ramp.
const MIN_DB_SPAN: f32 = 6.0;

/// The scroll speed slider's span, columns per second: the slow end holds
/// most of a minute on a wide panel, the fast end reads nearly as a live
/// spectrum smeared sideways.
const SPEED_MIN: f32 = 5.0;
const SPEED_MAX: f32 = 120.0;

/// The history slider's span, columns. This is the texture's long side and
/// what bounds the panel's memory: the top end is 2 MB of cells.
const HISTORY_MIN: usize = 128;
const HISTORY_MAX: usize = 2048;

/// The FFT sizes the picker offers. Short windows follow a transient, long
/// ones separate two notes down low; past 8k the window covers enough time
/// that a column stops meaning one moment.
const FFT_CHOICES: &[(&str, usize)] = &[("1k", 1024), ("2k", 2048), ("4k", 4096), ("8k", 8192)];

/// How long the feed may sit still before it reads as stopped audio rather
/// than the gap between pump ticks. Same as the spectrum's and the VU's, and
/// for the same reason: the tap drains on a ~16ms timer, so frames between
/// ticks see no new samples, and treating that as silence would print a black
/// stripe through every column of an otherwise loud track.
const SILENT_AFTER: f32 = 0.15;

/// The panel's own size floor. The body is one canvas that draws at whatever
/// size it gets, so a layout is free to run the waterfall as a thin strip.
const MIN_SIDE: Pixels = px(24.);

/// How the loudness maps to color. The first four are perceptual ramps that
/// carry the same order everywhere along them, which is the whole reason a
/// heatmap is readable at all; the last two route through the ramp the
/// spectrum and VU panels share, so the waterfall follows the palette and the
/// cover art like every other visualizer.
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

/// The colormap anchors, evenly spaced and interpolated between. A handful of
/// stops off the real tables is indistinguishable from the full 256 entries
/// once it's a few pixels tall, and it keeps the tables out of the binary.
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

/// A clamp that swallows NaN too. `f32::clamp` passes it straight through, and
/// one NaN out of a hand-edited layout would take a whole column with it, so
/// every config accessor goes through here.
fn sane(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_nan() {
        fallback
    } else {
        value.clamp(min, max)
    }
}

/// The spectrogram panel's per-view config: what a saved layout restores and
/// what the customize window edits. Missing fields take the defaults, so a
/// layout dumped before a field existed still loads. The scroll edge reuses
/// the spectrum's type so the visualizers use the same terms.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrogramConfig {
    /// The rename, theme override, and placement locks shared by every panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// FFT window size: short windows follow transients, long ones separate
    /// neighbouring notes down low.
    pub fft_size: usize,
    /// Low bound of the frequency axis, Hz.
    pub lo_hz: f32,
    /// High bound of the frequency axis, Hz. Capping below Nyquist drops the
    /// near-silent top octaves that would sit black across the whole panel.
    pub hi_hz: f32,
    /// Log frequency axis, where an octave takes the same room anywhere, or
    /// linear, which is what a lab tool shows and what makes a harmonic stack
    /// read as evenly spaced.
    pub log_scale: bool,
    /// The quiet end of the mapped dB window: anything at or under this takes
    /// the colormap's dark stop.
    pub floor_db: f32,
    /// The loud end: anything at or over this takes the bright stop.
    pub ceil_db: f32,
    /// How fast the picture scrolls, columns per second.
    pub speed: f32,
    /// How many columns the history keeps.
    pub history: usize,
    /// How loudness maps to color.
    pub colormap: Colormap,
    /// The edge new columns enter from. The frequency axis runs across the
    /// scroll, so a sideways scroll means an upright frequency axis.
    pub direction: Orientation,
    /// The frequency ruler's dividers over the picture.
    pub grid: bool,
    /// The ruler's numbers, where the panel has room for them.
    pub labels: bool,
    /// Hold the standing picture while playback is paused instead of
    /// scrolling silence into it.
    pub freeze: bool,
}

impl Default for SpectrogramConfig {
    fn default() -> Self {
        SpectrogramConfig {
            chrome: PanelChrome::default(),
            fft_size: 4096,
            lo_hz: 40.0,
            hi_hz: 16_000.0,
            log_scale: true,
            floor_db: -90.0,
            ceil_db: -20.0,
            speed: 40.0,
            history: 512,
            colormap: Colormap::default(),
            direction: Orientation::Right,
            grid: true,
            labels: true,
            freeze: false,
        }
    }
}

impl SpectrogramConfig {
    /// The window size, snapped to the picker's power-of-two steps. The clamp
    /// comes first on purpose: `next_power_of_two` overflow-panics near the
    /// top of `usize` and [`Analyzer::new`] asserts on anything outside its
    /// range, so a hand-edited layout must not be able to reach either.
    fn fft(&self) -> usize {
        self.fft_size
            .clamp(MIN_FFT_SIZE, MAX_FFT_SIZE)
            .next_power_of_two()
    }

    /// The frequency axis, clamped to the slider band and the minimum span, so
    /// a hand-edited file can't invert or collapse the mapping.
    fn range(&self) -> (f32, f32) {
        let lo = sane(self.lo_hz, HZ_MIN, HZ_MAX, 40.0);
        let hi = sane(self.hi_hz, HZ_MIN, HZ_MAX, 16_000.0)
            .max(lo * MIN_RATIO)
            .min(HZ_MAX);
        (lo.min(hi / MIN_RATIO), hi)
    }

    /// The mapped dB window, with the ceiling kept clear of the floor. The two
    /// sliders meet at -40, so the ceiling reads back above whatever the floor
    /// ended up at rather than trusting the pair.
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

    /// Where a frequency falls along the axis, 0 at the low bound and 1 at the
    /// high one.
    fn axis_frac(&self, hz: f32) -> f32 {
        let (lo, hi) = self.range();
        if self.log_scale {
            (hz / lo).ln() / (hi / lo).ln()
        } else {
            (hz - lo) / (hi - lo)
        }
    }
}

/// A slider fraction (0 to 1) as a log-spaced frequency across the band, and
/// back. Log so an octave takes the same travel anywhere on the strip.
fn frac_to_hz(fraction: f32) -> f32 {
    HZ_MIN * (HZ_MAX / HZ_MIN).powf(fraction.clamp(0.0, 1.0))
}

fn hz_to_frac(hz: f32) -> f32 {
    (hz / HZ_MIN).ln() / (HZ_MAX / HZ_MIN).ln()
}

/// The frequency an axis fraction stands for, the inverse of
/// [`SpectrogramConfig::axis_frac`].
fn freq_at(t: f32, lo: f32, hi: f32, log: bool) -> f32 {
    if log {
        lo * (hi / lo).powf(t)
    } else {
        lo + (hi - lo) * t
    }
}

/// Whole columns due since the last tick, keeping the fraction for the next
/// one. Wall clock rather than frame count, so the picture scrolls at the
/// configured rate on a 60 Hz and a 144 Hz display alike, and a speed slower
/// than the refresh doesn't round down to nothing every tick.
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

/// One row's magnitude out of the half-spectrum, over the bin span `b0..b1`
/// the row covers. Two regimes, because a log axis puts a row well inside one
/// bin down low and across dozens of them up top: under a bin wide the row
/// interpolates between its neighbours, or the low end draws as a staircase of
/// flat blocks; wider than a bin it takes the max, so a narrow partial
/// survives the fold instead of being averaged into the noise around it.
///
/// Bin 0 is DC and never reaches a row: a DC offset in the tap would otherwise
/// print as a solid bar along the bottom of everything.
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

/// What the history was built for. Any change to the frequency axis or the dB
/// window invalidates every stored column: they were reduced under the old
/// mapping, and reinterpreting them would draw a lie rather than old data. So
/// a mismatch clears the ring instead of keeping the picture.
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

/// One column of the spectrogram: the half-spectrum folded into [`ROWS`] rows,
/// each stored as its position in the dB window rather than a raw magnitude.
/// That's what keeps the history's size independent of the FFT size, and it
/// means a bake is a lookup per cell rather than a log per cell.
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

/// A colormap's color at position `t`, as straight RGB.
fn cell_color(map: Colormap, t: f32) -> [u8; 3] {
    match map {
        Colormap::Magma => sample_stops(&MAGMA, t),
        Colormap::Viridis => sample_stops(&VIRIDIS, t),
        Colormap::Ice => sample_stops(&ICE, t),
        Colormap::Grayscale => sample_stops(&GRAYSCALE, t),
        Colormap::Theme | Colormap::Cover => {
            let gradient = if map == Colormap::Theme {
                Gradient::Theme
            } else {
                Gradient::Cover
            };
            // The custom pair is only read for Gradient::Custom, which this
            // never asks for.
            let accent = palette::accent();
            let color = ramp_color(gradient, t, (accent, accent));
            // That ramp carries alpha at its quiet end, since it's built for
            // bars drawn over the panel. A heatmap cell is opaque, so it gets
            // composed over the panel background here rather than handed to
            // the renderer as a hole.
            let bg = palette::bg_root();
            let a = if color.a.is_nan() {
                1.0
            } else {
                color.a.clamp(0.0, 1.0)
            };
            let mix = |base: f32, over: f32| {
                let v = base * (1.0 - a) + over * a;
                (v.clamp(0.0, 1.0) * 255.0).round() as u8
            };
            [mix(bg.r, color.r), mix(bg.g, color.g), mix(bg.b, color.b)]
        }
    }
}

/// A stop table sampled at `t`, the stops evenly spaced across 0 to 1.
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

/// What the standing texture was baked with, apart from the cells themselves.
/// The colormap and the scroll edge are obvious; the three samples catch a
/// theme switch or a new cover under the Theme and Cover maps, which change
/// the colors without changing anything in the config.
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

/// Per-panel waterfall state, shared with the paint closure the way the
/// spectrum shares its bars: the entity holds the handle, the closure does the
/// per-frame work where the bounds are known.
struct Waterfall {
    last_written: u64,
    last_tick: Option<Instant>,
    /// When the feed last carried new audio.
    last_fresh: Option<Instant>,
    /// What the ring was built for; a mismatch clears it.
    mapping: Option<Mapping>,
    analyzer: Option<Analyzer>,
    /// Sample scratch, one FFT window wide.
    mono: Vec<f32>,
    /// The newest column, held between pump ticks so a frame that brought no
    /// audio scrolls what's in hand instead of re-running the FFT.
    rows: Vec<f32>,
    /// The ring, column-major: `cells[slot * ROWS + row]`.
    cells: Vec<f32>,
    /// Columns the ring holds, and the slot the next one goes into, which is
    /// also where the oldest one currently sits.
    history: usize,
    head: usize,
    /// Consecutive silent columns at the head. Once it reaches the history the
    /// whole picture is silence and there's nothing left to scroll.
    quiet: usize,
    /// Fractional columns carried across ticks.
    accum: f32,
    /// The standing texture and the one it replaced, plus whether the cells
    /// have moved since the bake.
    image: Option<Arc<RenderImage>>,
    retired: Option<Arc<RenderImage>>,
    skin: Option<Skin>,
    dirty: bool,
    /// Something still needs to move: render keeps requesting frames until
    /// this clears.
    alive: bool,
}

impl Waterfall {
    fn new() -> Self {
        Waterfall {
            last_written: 0,
            last_tick: None,
            last_fresh: None,
            mapping: None,
            analyzer: None,
            mono: Vec::new(),
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
            alive: false,
        }
    }

    /// Build the analyzer and the ring for a mapping, dropping whatever was
    /// stored under the old one.
    fn reset(&mut self, mapping: &Mapping) {
        self.analyzer = Some(Analyzer::new(mapping.fft));
        self.mono = vec![0.0; mapping.fft];
        self.rows = vec![0.0; ROWS];
        self.cells = vec![0.0; mapping.history * ROWS];
        self.history = mapping.history;
        self.head = 0;
        self.quiet = mapping.history;
        self.accum = 0.0;
        self.dirty = true;
        self.mapping = Some(mapping.clone());
    }

    /// One tick: re-analyze if the feed moved, then scroll however many
    /// columns the wall clock is owed. No new audio holds the current column
    /// across the gap between pump ticks; audio that's really stopped scrolls
    /// silence in until the panel is empty, unless `hold` keeps the picture
    /// standing (freeze on pause).
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

        // Frozen: hold the picture exactly where it is and stop animating. An
        // axis edit made while frozen clears the ring above and leaves the
        // panel empty until playback resumes, which beats redrawing the old
        // columns under a mapping they were never reduced for.
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

    /// The newest window folded into the current column. The feed returns
    /// short while it fills, and [`Analyzer::magnitudes`] wants exactly a
    /// window, so a partial read leaves the previous column standing rather
    /// than analyzing a buffer with a stale tail on it.
    fn analyze(&mut self, feed: &AudioFeed, mapping: &Mapping) {
        let Some(analyzer) = self.analyzer.as_mut() else {
            return;
        };
        if self.mono.is_empty() || feed.latest_mono(&mut self.mono) != self.mono.len() {
            return;
        }
        let mags = analyzer.magnitudes(&self.mono);
        reduce(mags, mapping, &mut self.rows);
    }

    /// The current column into the ring, oldest one out.
    fn push_column(&mut self) {
        if self.history == 0 || self.cells.len() < self.history * ROWS || self.rows.len() < ROWS {
            return;
        }
        let silent = self.rows.iter().all(|&v| v <= 0.0);
        if silent && self.quiet >= self.history {
            // Already scrolled entirely to silence: pushing another empty
            // column changes no pixel, so don't dirty the texture for it.
            return;
        }
        let base = self.head * ROWS;
        self.cells[base..base + ROWS].copy_from_slice(&self.rows[..ROWS]);
        self.head = (self.head + 1) % self.history;
        self.quiet = if silent { self.quiet + 1 } else { 0 };
        self.dirty = true;
    }

    /// The ring as a texture at its own resolution, one texel per column and
    /// per row, which the renderer then scales to the panel. Sizing it to the
    /// panel's pixels instead would rebuild everything on every resize and
    /// make a wide panel expensive for no more detail than this.
    fn bake(&self, config: &SpectrogramConfig) -> Option<Arc<RenderImage>> {
        let history = self.history;
        if history == 0 || self.cells.len() < history * ROWS {
            return None;
        }
        let lut: Vec<[u8; 4]> = (0..LUT_STEPS)
            .map(|i| {
                let [r, g, b] = cell_color(config.colormap, i as f32 / (LUT_STEPS - 1) as f32);
                [r, g, b, 0xff]
            })
            .collect();

        // The frequency axis runs across the scroll, so a sideways scroll puts
        // time along the width and frequency up the height, and a vertical one
        // swaps them. Low frequencies sit at the bottom of an upright axis and
        // at the left of a flat one, the way every other scale in the app runs.
        let flat = config.direction.horizontal();
        let (w, h) = if flat {
            (ROWS, history)
        } else {
            (history, ROWS)
        };
        let mut raw = vec![0u8; w * h * 4];
        for i in 0..history {
            // `i` counts up from the oldest column.
            let slot = (self.head + i) % history;
            let base = slot * ROWS;
            for row in 0..ROWS {
                let t = self.cells[base + row];
                let step = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
                let color = lut[((step * (LUT_STEPS - 1) as f32) as usize).min(LUT_STEPS - 1)];
                let (x, y) = match config.direction {
                    Orientation::Right => (i, ROWS - 1 - row),
                    Orientation::Left => (history - 1 - i, ROWS - 1 - row),
                    Orientation::Bottom => (row, i),
                    Orientation::Top => (row, history - 1 - i),
                };
                let at = (y * w + x) * 4;
                raw[at..at + 4].copy_from_slice(&color);
            }
        }
        // The renderer needs BGRA, the same swizzle the backdrop's bake does.
        for pixel in raw.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let buf = RgbaImage::from_raw(w as u32, h as u32, raw)?;
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

        // Keep this. RenderImage::new mints a fresh ImageId off a global
        // counter, and gpui's sprite atlas keys its tiles by that id and holds
        // them until something hands them back. A column a frame without this
        // is a new tile 40 times a second and an exhausted atlas within
        // seconds. The drop runs one paint late rather than at the moment the
        // texture is replaced, so the tile outlives the frame that still
        // points at it.
        if let Some(old) = self.retired.take() {
            let _ = window.drop_image(old);
        }

        let skin = Skin::of(config);
        if self.skin.as_ref() != Some(&skin) {
            self.skin = Some(skin);
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

/// The frequency ruler over the picture: the 1-2-5 ladder's labelled steps as
/// dividers across the scroll, each tagged with its frequency where the panel
/// has room. Text is pricier than the lines, so the tags only draw once the
/// panel can spread them, the same rule the VU scale follows, and a tag that
/// would land on the previous one is dropped rather than printed over it.
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
        // Low frequencies sit at the bottom of an upright axis, so the
        // fraction counts up from the base edge rather than down from the top.
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
        // The tag hangs just clear of its own divider, then clamps so one near
        // a corner never spills out of the panel.
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
    /// The settings sliders' painted bounds and drag state, one per slider so
    /// a drag on one never moves the others.
    lo_scrub: ScrubState,
    hi_scrub: ScrubState,
    floor_scrub: ScrubState,
    ceil_scrub: ScrubState,
    speed_scrub: ScrubState,
    history_scrub: ScrubState,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel when a session starts, so an idle window resumes
    /// animating without the player bar's frame pump.
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
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    /// The low bound stops a min-span short of the high one, so the axis never
    /// inverts as the strip drags past it.
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
        // A NaN out of the typed readout casts to zero rather than through,
        // and the clamp catches it from there.
        self.config.history = (columns.round() as i64).clamp(0, HISTORY_MAX as i64) as usize;
        cx.notify();
    }

    /// One log-frequency bounds slider: the shared scalar slider with the Hz
    /// readout alongside, click-to-type like the rest. The readout switches to
    /// kHz up top, but the input is always plain Hz, so the seed drops the
    /// unit and `hz_to_frac` reads what's typed straight.
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

    /// The panel's own dropdown entries: a Display flyout of the quick toggles
    /// the customize window also holds, for a flip without opening it.
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
        // While audio moves the direct observe re-renders on every pump tick,
        // and the wall clock decides how many columns that tick is worth.
        // Frame polling is just for the silence scrolling in after audio
        // stops; once the panel is empty it parks, and a resume wakes it
        // through the pump's play-state notify.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        // Freeze on pause holds the standing picture: paused mid-session, not
        // a played-out queue.
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
        div()
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
            ))
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
            ))
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
            ))
            .child(setting_row(
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
            ))
            .into_any_element()
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
        panel::themed(&chrome, || self.body(window, cx))
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

    /// Down low a log row sits well inside one bin, and taking that bin whole
    /// would draw the bottom of the panel as a staircase of flat blocks.
    #[test]
    fn a_narrow_row_interpolates_between_its_neighbours() {
        let mags = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        // Centered halfway between a cold bin and the hot one.
        assert!((fold(&mags, 3.4, 3.6) - 0.5).abs() < 1e-6);
        // Sitting right on the hot bin takes all of it.
        assert!((fold(&mags, 3.9, 4.1) - 1.0).abs() < 1e-6);
        // And a quarter of the way onto it takes a quarter.
        assert!((fold(&mags, 3.2, 3.3) - 0.25).abs() < 1e-6);
    }

    /// Up top a row covers dozens of bins, and averaging them would bury a
    /// narrow partial under the quiet either side of it.
    #[test]
    fn a_wide_row_keeps_the_narrow_partial() {
        let mut mags = [0.0f32; 32];
        mags[7] = 1.0;
        assert_eq!(fold(&mags, 2.0, 12.0), 1.0);
        // A span that misses it entirely stays at the floor.
        assert_eq!(fold(&mags, 12.0, 24.0), 0.0);
    }

    /// A DC offset in the tap would print as a solid bar along the bottom of
    /// everything, so bin 0 is out of reach in both regimes.
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

    /// A tone at a known frequency has to land in the row that covers it, and
    /// the dB window has to place it inside 0 to 1.
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
        // The bin is 46.9 Hz wide, so the row it lights is the one that bin
        // falls in rather than exactly the one 1 kHz falls in.
        assert!(
            (f0 - 100.0..f1 + 100.0).contains(&1000.0),
            "1 kHz should light the row covering {f0}..{f1}"
        );
        // Full scale is 0 dB, well over the -20 dB ceiling.
        assert_eq!(rows[hot], 1.0);
        // And silence sits on the floor.
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
        // Five columns into four slots: the first one is gone and the rest
        // read back oldest first from the head.
        let read: Vec<f32> = (0..4)
            .map(|i| view.cells[((view.head + i) % 4) * ROWS])
            .collect();
        assert_eq!(read, vec![0.2, 0.3, 0.4, 0.5]);
        assert_eq!(view.quiet, 0);
    }

    /// A parked panel must not keep rebuilding its texture out of silence it
    /// already scrolled in.
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

        // A loud column wakes it, and the silence after it scrolls until the
        // ring is empty again.
        view.rows.fill(0.7);
        view.push_column();
        assert_eq!(view.quiet, 0);
        view.rows.fill(0.0);
        for _ in 0..4 {
            view.push_column();
        }
        assert_eq!(view.quiet, 4);
    }

    /// The scroll rate is wall clock, so the same stretch of time moves the
    /// same distance whatever the display's refresh is. Dropping the fraction
    /// each tick would leave the fast run short.
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
        // Quarter of a column a tick: three ticks of nothing, then one.
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
        // An interior anchor is hit exactly, so the ramp really runs through
        // the stops rather than near them.
        assert_eq!(cell_color(Colormap::Magma, 0.5), MAGMA[2]);
    }

    #[test]
    fn a_colormap_position_off_the_ends_stays_on_the_ramp() {
        assert_eq!(cell_color(Colormap::Grayscale, -5.0), [0, 0, 0]);
        assert_eq!(cell_color(Colormap::Grayscale, 9.0), [255, 255, 255]);
        assert_eq!(cell_color(Colormap::Grayscale, f32::NAN), [0, 0, 0]);
        assert_eq!(cell_color(Colormap::Magma, f32::INFINITY), MAGMA[4]);
    }

    /// A hand-edited layout is the one place these arrive broken, and a NaN
    /// would take a whole column with it.
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
            // Both ends of the axis map to both ends of the panel.
            assert!((config.axis_frac(lo)).abs() < 1e-4);
            assert!((config.axis_frac(hi) - 1.0).abs() < 1e-4);
        }
    }

    /// `next_power_of_two` overflow-panics near the top of `usize` and
    /// [`Analyzer::new`] asserts on anything outside its range, so the clamp
    /// has to come first. A hand-edited size must not be able to take the app
    /// down, which is exactly what the other ordering does.
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
        // The picker's own steps come back untouched.
        for &(_, size) in FFT_CHOICES {
            let config = SpectrogramConfig {
                fft_size: size,
                ..SpectrogramConfig::default()
            };
            assert_eq!(config.fft(), size);
        }
    }

    /// The history is what bounds the panel's memory, so a config that asks
    /// for more than the ceiling has to be held to it.
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
