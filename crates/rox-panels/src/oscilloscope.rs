//! The oscilloscope panel: the player's PCM tap drawn in time, one window of
//! samples spanning the panel. What makes it readable is the trigger. Rather
//! than drawing whatever the newest window happens to hold, the panel pulls
//! more audio than it shows and starts the drawn frame at the first crossing
//! of the trigger level, so periodic material stands still instead of
//! sliding sideways. Each column holds a min/max pair rather than one
//! decimated sample, which keeps a transient from falling between pixels,
//! and the trace colors by its own excursion through the loudness ramp the
//! spectrum and VU panels share. Like them it's paint primitives on the UI
//! thread: a frame per pump tick while audio flows, and once the audio stops
//! and the phosphor trail has burned off the panel stops asking for frames,
//! so an idle app pays nothing.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, fill, point, prelude::*, px, size, AnyElement, App, Bounds, Context, Div, Entity,
    EventEmitter, FocusHandle, Focusable, Path, Pixels, Point, Rgba, SharedString, Subscription,
    WeakEntity, Window,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Sizable as _;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::analysis::MAX_FFT_SIZE;
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, choices_shared, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState,
};
use crate::panel_settings;
use crate::settings::ui as settings_ui;
use crate::spectrum::{gradient_choices, ramp_color, Gradient};

/// The time window slider's span, ms: a millisecond resolves a single cycle
/// of a high note, a tenth of a second holds a bar of a slow bassline.
const WINDOW_MS_MIN: f32 = 1.0;
const WINDOW_MS_MAX: f32 = 100.0;

/// The vertical scale slider's span, log-spaced so the quiet end that
/// actually needs the boost gets most of the travel.
const GAIN_MIN: f32 = 0.25;
const GAIN_MAX: f32 = 16.0;

/// The trace thickness slider's span, px.
const LINE_W_MIN: f32 = 0.5;
const LINE_W_MAX: f32 = 4.0;

/// The persistence slider's top. Full 1.0 would never fade, so the trail
/// stops just short of it.
const PERSIST_MAX: f32 = 0.95;

/// The most previous frames the phosphor trail keeps. Bounded, because
/// each frame is a column of pairs per channel, and an unbounded ring on a
/// wide panel adds up.
const MAX_TRAILS: usize = 8;

/// How much wider than the drawn window the pull runs. The extra frames are
/// the slack the trigger searches for its crossing, so a period longer than
/// this can't be locked. Two windows of room covers anything periodic
/// enough to stand still in the first place.
const SEARCH_SPAN: usize = 3;

/// The most frames a pull can ask for: what the feed itself holds, since
/// it buffers interleaved stereo and hands back frames.
const MAX_PULL: usize = MAX_FFT_SIZE * 2;

/// How much of a frame's half-height full scale maps to, so a pinned trace
/// doesn't touch the panel edge.
const HEADROOM: f32 = 0.94;

/// How many steps the trace's color ramp quantizes into. One path is one
/// fill, so the trace is built as this many paths and each segment joins
/// the one nearest its own excursion; past a handful of steps the eye
/// stops reading the difference and the paint calls keep adding up.
const RAMP_STEPS: usize = 8;

/// Vertical divisions the graticule rules the window into.
const GRID_DIVS: usize = 8;

/// The column count clamps: two is the fewest a segment can be built from,
/// and the ceiling covers a wide panel on a dense display.
const MIN_COLS: usize = 2;
const MAX_COLS: usize = 4096;

/// How long the feed may sit still before it reads as stopped audio rather
/// than the gap between pump ticks. Same as the spectrum's and the VU's,
/// and for the same reason: between ticks the trace holds instead of
/// flattening.
const SILENT_AFTER: f32 = 0.15;

/// Where the drawn frame starts: at a crossing of the trigger level going
/// up, going down, or wherever the newest window happens to begin.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Off,
    #[default]
    Rising,
    Falling,
}

/// How many traces and where they go: the stereo fold, both channels over
/// each other in one frame, or a frame each stacked down the panel.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeChannels {
    #[default]
    Mono,
    Overlay,
    Split,
}

impl ScopeChannels {
    /// Whether the panel needs the channels split rather than folded.
    fn stereo(self) -> bool {
        self != ScopeChannels::Mono
    }
}

fn trigger_choices() -> [(SharedString, Trigger); 3] {
    [
        (rox_i18n::t!("oscilloscope-trigger-off"), Trigger::Off),
        (rox_i18n::t!("oscilloscope-trigger-rising"), Trigger::Rising),
        (
            rox_i18n::t!("oscilloscope-trigger-falling"),
            Trigger::Falling,
        ),
    ]
}

fn channel_choices() -> [(SharedString, ScopeChannels); 3] {
    [
        (
            rox_i18n::t!("oscilloscope-channels-mono"),
            ScopeChannels::Mono,
        ),
        (
            rox_i18n::t!("oscilloscope-channels-overlay"),
            ScopeChannels::Overlay,
        ),
        (
            rox_i18n::t!("oscilloscope-channels-split"),
            ScopeChannels::Split,
        ),
    ]
}

/// A clamp that swallows NaN too. `f32::clamp` passes it straight through,
/// and one NaN out of a hand-edited layout would take the whole trace with
/// it, so every config accessor goes through here.
fn sane(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_nan() {
        fallback
    } else {
        value.clamp(min, max)
    }
}

/// The oscilloscope panel's per-view config: what a saved layout restores
/// and what the customize window edits. Missing fields take the defaults, so
/// a layout dumped before a field existed still loads. The color ramp reuses
/// the spectrum's type so the visualizers use the same terms.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OscilloscopeConfig {
    /// The rename, theme override, and placement locks shared by every panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// How much time the trace spans, ms.
    pub window_ms: f32,
    /// Vertical scale: what a full-scale sample is multiplied by before it
    /// reaches the frame edge.
    pub gain: f32,
    /// Where the drawn frame starts, and which way the signal has to cross
    /// to start it.
    pub trigger: Trigger,
    /// The level the trigger looks for the crossing at, in samples.
    pub trigger_level: f32,
    /// How many traces and where they go.
    pub channels: ScopeChannels,
    /// Trace thickness, px.
    pub line_width: f32,
    /// A soft fill between the trace and the center line.
    pub fill: bool,
    /// The graticule behind the trace.
    pub grid: bool,
    /// How the trace colors by excursion: flat accent, or a ramp from the
    /// theme, the cover art, or the custom pair below.
    pub gradient: Gradient,
    /// The custom ramp's ends, `#rrggbb`: the quiet base and the loud tip.
    pub gradient_lo: String,
    pub gradient_hi: String,
    /// How long previous frames linger behind the trace, the phosphor
    /// afterglow look. Zero draws the standing frame alone.
    pub persistence: f32,
    /// Freeze the trace while playback is paused instead of letting it fall
    /// flat.
    pub freeze: bool,
}

impl Default for OscilloscopeConfig {
    fn default() -> Self {
        OscilloscopeConfig {
            chrome: PanelChrome::default(),
            window_ms: 20.0,
            gain: 1.0,
            trigger: Trigger::default(),
            trigger_level: 0.0,
            channels: ScopeChannels::default(),
            line_width: 1.5,
            fill: false,
            grid: true,
            gradient: Gradient::default(),
            gradient_lo: "#22aa44".into(),
            gradient_hi: "#dd3322".into(),
            persistence: 0.0,
            freeze: true,
        }
    }
}

impl OscilloscopeConfig {
    /// The window and trace thickness read back to the typed ceiling rather
    /// than the strip's own top, or every value typed past the top would
    /// drop on the next load.
    fn window_ms(&self) -> f32 {
        sane(
            self.window_ms,
            WINDOW_MS_MIN,
            settings_ui::ceiling(WINDOW_MS_MIN, WINDOW_MS_MAX),
            20.0,
        )
    }

    fn line_w(&self) -> f32 {
        sane(
            self.line_width,
            LINE_W_MIN,
            settings_ui::ceiling(LINE_W_MIN, LINE_W_MAX),
            1.5,
        )
    }

    /// The knobs whose ends mean something: full scale either way for the
    /// trigger level, the log slider's own span for the gain.
    fn gain(&self) -> f32 {
        sane(self.gain, GAIN_MIN, GAIN_MAX, 1.0)
    }

    fn trigger_level(&self) -> f32 {
        sane(self.trigger_level, -1.0, 1.0, 0.0)
    }

    fn persistence(&self) -> f32 {
        sane(self.persistence, 0.0, PERSIST_MAX, 0.0)
    }

    /// How many previous frames the trail keeps at the current persistence:
    /// none at zero, the full ring at the top of the slider.
    fn trails(&self) -> usize {
        ((self.persistence() / PERSIST_MAX) * MAX_TRAILS as f32).round() as usize
    }

    /// The custom ramp's ends parsed, falling back to the theme ramp's when
    /// a hand-edited hex doesn't parse, the same fallback the spectrum and
    /// the VU meter use.
    fn custom_ramp(&self) -> (Rgba, Rgba) {
        (
            palette::parse_hex(&self.gradient_lo)
                .unwrap_or_else(|| palette::alpha(palette::text_faint(), 0x66)),
            palette::parse_hex(&self.gradient_hi).unwrap_or_else(palette::accent),
        )
    }
}

/// One drawn channel: a min/max sample pair per column, raw. The gain is
/// applied at paint, so dragging the scale slider on a paused panel moves the
/// standing trace instead of waiting for the next tick.
type Lane = Vec<(f32, f32)>;

/// Where the drawn window starts inside the pull: the first crossing of
/// `level` in the configured direction within the slack the pull left in
/// front of it. None means no crossing was there, and the caller falls back
/// to a fixed offset: hunting for whatever looked closest is exactly the
/// smear the trigger exists to kill.
fn trigger_at(samples: &[f32], slack: usize, trigger: Trigger, level: f32) -> Option<usize> {
    if trigger == Trigger::Off || slack == 0 {
        return None;
    }
    let end = slack.min(samples.len().saturating_sub(1));
    (0..end).find(|&i| {
        let (a, b) = (samples[i], samples[i + 1]);
        match trigger {
            Trigger::Rising => a <= level && b > level,
            Trigger::Falling => a >= level && b < level,
            Trigger::Off => false,
        }
    })
}

/// The drawn window as one min/max pair per column. More than one sample to
/// a column keeps both ends, so a transient between column centers still
/// shows instead of being decimated away; fewer than one interpolates, which
/// keeps a 1 ms window off a staircase. Callers guarantee at least two
/// samples and two columns.
fn resample(window: &[f32], cols: usize) -> Lane {
    let n = window.len();
    let mut lane = Vec::with_capacity(cols);
    if n >= cols {
        for c in 0..cols {
            let from = c * n / cols;
            let to = ((c + 1) * n / cols).max(from + 1).min(n);
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for &s in &window[from..to] {
                lo = lo.min(s);
                hi = hi.max(s);
            }
            // An all-NaN column leaves the seeds untouched; a garbage tap is
            // still a flat line rather than a path with infinite bounds.
            if lo.is_finite() && hi.is_finite() {
                lane.push((lo, hi));
            } else {
                lane.push((0.0, 0.0));
            }
        }
    } else {
        let span = (n - 1) as f32;
        let steps = (cols - 1) as f32;
        for c in 0..cols {
            let pos = c as f32 * span / steps;
            let i = (pos as usize).min(n - 1);
            let next = window[(i + 1).min(n - 1)];
            let v = window[i] + (next - window[i]) * (pos - i as f32);
            let v = if v.is_finite() { v } else { 0.0 };
            lane.push((v, v));
        }
    }
    lane
}

/// The panel-space geometry a paint pass works in: where a lane's frame is
/// and how a column maps across it. Split stacks a frame per lane; the other
/// modes lay every lane into the one frame.
struct Geometry {
    ox: f32,
    oy: f32,
    w: f32,
    /// A frame's height: the whole panel, or a share of it under Split.
    fh: f32,
    /// How far full scale extends from a frame's center line.
    amp: f32,
    split: bool,
}

impl Geometry {
    fn center(&self, lane: usize) -> f32 {
        let frame = if self.split { lane } else { 0 };
        self.oy + frame as f32 * self.fh + self.fh / 2.0
    }

    fn x(&self, col: usize, cols: usize) -> f32 {
        self.ox + col as f32 * self.w / (cols - 1).max(1) as f32
    }
}

/// A strip segment as the two triangles a gpui path takes. Corners run
/// top-left, top-right, bottom-right, bottom-left, so the pair tiles the
/// quad without overlapping itself and a translucent fill blends once.
fn push_quad(
    path: &mut Path<Pixels>,
    tl: Point<Pixels>,
    tr: Point<Pixels>,
    br: Point<Pixels>,
    bl: Point<Pixels>,
) {
    let solid = (point(0., 1.), point(0., 1.), point(0., 1.));
    path.push_triangle((tl, tr, br), solid);
    path.push_triangle((tl, br, bl), solid);
}

/// Per-panel scope state, shared with the paint closure the way the spectrum
/// shares its bars: the entity holds the handle, the closure does the
/// per-frame work where the bounds are known.
struct Scope {
    last_written: u64,
    /// When the feed last carried new audio.
    last_fresh: Option<Instant>,
    /// Sample scratch, grown to whatever the window slider asks for.
    left: Vec<f32>,
    right: Vec<f32>,
    /// The standing frame, one lane per drawn channel.
    trace: Vec<Lane>,
    /// Previous frames for the phosphor trail, newest at the back.
    trail: VecDeque<Vec<Lane>>,
    /// Whether the trace has already been flattened for silence, so the
    /// flattening happens once instead of every parked frame.
    silent: bool,
    /// Something still needs to move: render keeps requesting frames until
    /// this clears.
    alive: bool,
}

impl Scope {
    fn new() -> Self {
        Scope {
            last_written: 0,
            last_fresh: None,
            left: Vec::new(),
            right: Vec::new(),
            trace: Vec::new(),
            trail: VecDeque::new(),
            silent: true,
            alive: false,
        }
    }

    /// One tick: pull the newest audio, trigger on it, and resample the drawn
    /// window into columns. No new audio holds the standing frame across the
    /// gap between pump ticks; audio that's really stopped burns the trail
    /// off a frame at a time and then flattens, unless `hold` keeps the frame
    /// standing (freeze on pause).
    fn step(&mut self, feed: &AudioFeed, cols: usize, config: &OscilloscopeConfig, hold: bool) {
        let written = feed.written();
        let fresh = written != self.last_written;
        self.last_written = written;

        // Frozen and nothing new: keep the standing frame and stop animating.
        if hold && !fresh {
            self.alive = false;
            return;
        }

        let now = Instant::now();
        if fresh {
            self.last_fresh = Some(now);
        }
        let stopped = self
            .last_fresh
            .is_none_or(|t| (now - t).as_secs_f32() > SILENT_AFTER);

        let trails = config.trails();
        while self.trail.len() > trails {
            self.trail.pop_front();
        }

        if fresh {
            let lanes = self.sample(feed, cols, config);
            if lanes.is_empty() {
                // Underfed feed: nothing to draw yet, but audio is flowing,
                // so keep asking for frames until it catches up.
                self.alive = true;
                return;
            }
            let previous = std::mem::replace(&mut self.trace, lanes);
            if trails > 0 && !previous.is_empty() {
                self.trail.push_back(previous);
                while self.trail.len() > trails {
                    self.trail.pop_front();
                }
            }
            self.silent = false;
            self.alive = true;
        } else if stopped {
            // The oldest trail frame goes first, so the afterglow burns off
            // from the back the way it built up.
            if self.trail.pop_front().is_some() {
                self.alive = true;
            } else if !self.silent {
                for lane in &mut self.trace {
                    lane.fill((0.0, 0.0));
                }
                self.silent = true;
                // Paint follows this step, so the flat frame is still drawn
                // before the panel parks.
                self.alive = false;
            } else {
                self.alive = false;
            }
        } else {
            // Between pump ticks: no new samples to draw, hold what's up.
            self.alive = true;
        }
    }

    /// The newest audio as one lane per drawn channel, triggered. Returns
    /// empty when the feed hasn't buffered enough to fill a window yet.
    fn sample(&mut self, feed: &AudioFeed, cols: usize, config: &OscilloscopeConfig) -> Vec<Lane> {
        // A device rate off the far end of plausible would blow the frame
        // count out; the clamp keeps the pull inside what the feed holds.
        let rate = feed.sample_rate().clamp(8_000, 384_000) as f32;
        let want = (config.window_ms() / 1000.0 * rate) as usize;
        let want = want.clamp(2, MAX_PULL / SEARCH_SPAN);
        let total = (want * SEARCH_SPAN).min(MAX_PULL);
        let stereo = config.channels.stereo();
        if self.left.len() < total {
            self.left.resize(total, 0.0);
        }
        if stereo && self.right.len() < total {
            self.right.resize(total, 0.0);
        }

        // The feed returns short when it's underfed, so everything below
        // measures off what actually arrived rather than what was asked for.
        let n = if stereo {
            feed.latest_stereo(&mut self.left[..total], &mut self.right[..total])
        } else {
            feed.latest_mono(&mut self.left[..total])
        };
        let draw = want.min(n);
        if draw < 2 || cols < 2 {
            return Vec::new();
        }
        let slack = n - draw;
        // Stereo triggers off the left channel and both lanes take the same
        // offset, so the phase between them stays visible instead of each
        // locking to its own crossing.
        let start = trigger_at(
            &self.left[..n],
            slack,
            config.trigger,
            config.trigger_level(),
        )
        .unwrap_or(slack);

        let mut lanes = vec![resample(&self.left[start..start + draw], cols)];
        if stereo {
            lanes.push(resample(&self.right[start..start + draw], cols));
        }
        lanes
    }

    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, config: &OscilloscopeConfig) {
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        if w <= 0.0 || h <= 0.0 || self.trace.is_empty() {
            return;
        }
        let split = config.channels == ScopeChannels::Split && self.trace.len() > 1;
        let frames = if split { self.trace.len() } else { 1 };
        let fh = h / frames as f32;
        let geo = Geometry {
            ox: f32::from(bounds.origin.x),
            oy: f32::from(bounds.origin.y),
            w,
            fh,
            amp: fh / 2.0 * HEADROOM,
            split,
        };

        if config.grid {
            paint_grid(&geo, frames, h, window);
        }

        // The trail goes down first, oldest at the front, so the standing
        // frame sits on top of its own afterglow. Each step back dims by the
        // persistence, so the knob reads as trail length and brightness at
        // once.
        let fade = config.persistence();
        let depth = self.trail.len();
        for (i, frame) in self.trail.iter().enumerate() {
            paint_lanes(
                frame,
                &geo,
                window,
                config,
                fade.powi((depth - i) as i32),
                false,
            );
        }
        paint_lanes(&self.trace, &geo, window, config, 1.0, config.fill);
    }
}

/// The graticule: a center line per frame with a division either side of it,
/// and the vertical rules the window splits into. Faint enough to read
/// behind the trace rather than through it.
fn paint_grid(geo: &Geometry, frames: usize, h: f32, window: &mut Window) {
    let center = palette::alpha(palette::gridline(), 0x3a);
    let rule = palette::alpha(palette::gridline(), 0x1c);
    let hair = |x: f32, y: f32, w: f32, h: f32| {
        Bounds::new(point(px(x), px(y)), size(px(w.max(1.0)), px(h.max(1.0))))
    };
    for frame in 0..frames {
        let yc = geo.oy + frame as f32 * geo.fh + geo.fh / 2.0;
        window.paint_quad(fill(hair(geo.ox, yc, geo.w, 1.0), center));
        for step in [0.5, 1.0] {
            for side in [-1.0, 1.0f32] {
                let y = yc + side * step * geo.amp;
                window.paint_quad(fill(hair(geo.ox, y, geo.w, 1.0), rule));
            }
        }
    }
    for div in 1..GRID_DIVS {
        let x = geo.ox + div as f32 * geo.w / GRID_DIVS as f32;
        window.paint_quad(fill(hair(x, geo.oy, 1.0, h), rule));
    }
}

/// One frame's traces. The trace is a ribbon between the column tops and
/// bottoms rather than a polyline: where the wave is steep the min/max span
/// gives the width, where it's flat the stroke width does, and the two meet
/// without a seam. `fade` is the trail's alpha multiplier, 1.0 for the
/// standing frame.
fn paint_lanes(
    lanes: &[Lane],
    geo: &Geometry,
    window: &mut Window,
    config: &OscilloscopeConfig,
    fade: f32,
    filled: bool,
) {
    let gain = config.gain();
    let custom = config.custom_ramp();
    let half = config.line_w() / 2.0;
    let base = ramp_color(config.gradient, 0.0, custom);
    for (i, lane) in lanes.iter().enumerate() {
        let cols = lane.len();
        if cols < 2 {
            continue;
        }
        let yc = geo.center(i);
        // Overlay lays both channels into one frame, so the second runs
        // dimmer and the pair stays tellable where they cross.
        let lane_fade = if !geo.split && i > 0 {
            fade * 0.55
        } else {
            fade
        };
        let alpha = (255.0 * lane_fade).clamp(0.0, 255.0) as u8;
        if alpha == 0 {
            continue;
        }

        // Column geometry up front: the ribbon's two edges, and where the
        // column's excursion falls on the ramp.
        let mut edges = Vec::with_capacity(cols);
        for &(lo, hi) in lane {
            let lo = (lo * gain).clamp(-1.0, 1.0);
            let hi = (hi * gain).clamp(-1.0, 1.0);
            edges.push((
                yc - hi * geo.amp - half,
                yc - lo * geo.amp + half,
                lo.abs().max(hi.abs()),
            ));
        }

        let mut buckets: Vec<Option<Path<Pixels>>> = vec![None; RAMP_STEPS];
        let mut area: Option<Path<Pixels>> = None;
        for (c, pair) in edges.windows(2).enumerate() {
            let (up0, dn0, t0) = pair[0];
            let (up1, dn1, t1) = pair[1];
            let x0 = px(geo.x(c, cols));
            let x1 = px(geo.x(c + 1, cols));
            let (tl, tr) = (point(x0, px(up0)), point(x1, px(up1)));
            let (br, bl) = (point(x1, px(dn1)), point(x0, px(dn0)));

            // The soft fill runs from the trace to the center line, as one
            // band per segment rather than one either side, so the two never
            // overlap where the wave crosses.
            if filled {
                let ftl = point(x0, px(up0.min(yc)));
                let ftr = point(x1, px(up1.min(yc)));
                let fbr = point(x1, px(dn1.max(yc)));
                let fbl = point(x0, px(dn0.max(yc)));
                push_quad(
                    area.get_or_insert_with(|| Path::new(ftl)),
                    ftl,
                    ftr,
                    fbr,
                    fbl,
                );
            }

            let t = t0.max(t1).clamp(0.0, 1.0);
            let step = ((t * RAMP_STEPS as f32) as usize).min(RAMP_STEPS - 1);
            push_quad(
                buckets[step].get_or_insert_with(|| Path::new(tl)),
                tl,
                tr,
                br,
                bl,
            );
        }

        if let Some(area) = area {
            window.paint_path(area, palette::alpha(base, alpha / 4));
        }
        for (step, path) in buckets.into_iter().enumerate() {
            let Some(path) = path else {
                continue;
            };
            let t = (step as f32 + 0.5) / RAMP_STEPS as f32;
            window.paint_path(
                path,
                palette::alpha(ramp_color(config.gradient, t, custom), alpha),
            );
        }
    }
}

pub struct OscilloscopePanel {
    state: AppState,
    config: OscilloscopeConfig,
    feed: Arc<AudioFeed>,
    scope: Arc<Mutex<Scope>>,
    /// The settings sliders' painted bounds and drag state, one per slider so
    /// a drag on one never moves the others.
    window_scrub: ScrubState,
    gain_scrub: ScrubState,
    level_scrub: ScrubState,
    line_w_scrub: ScrubState,
    persist_scrub: ScrubState,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
    /// The custom ramp's pickers, base then tip, built on the first settings
    /// render, since the panel itself constructs without a window and the
    /// picker state needs one.
    ramp_pickers: Option<[Entity<ColorPickerState>; 2]>,
    _ramp_changes: Vec<Subscription>,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel when a session starts, so an idle window resumes
    /// animating without the player bar's frame pump.
    _player_changed: Subscription,
}

impl OscilloscopePanel {
    pub fn new(state: AppState, config: OscilloscopeConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        OscilloscopePanel {
            config,
            feed: state.player.read(cx).feed(),
            state,
            scope: Arc::new(Mutex::new(Scope::new())),
            window_scrub: ScrubState::default(),
            gain_scrub: ScrubState::default(),
            level_scrub: ScrubState::default(),
            line_w_scrub: ScrubState::default(),
            persist_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            ramp_pickers: None,
            _ramp_changes: Vec::new(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
        }
    }

    fn set_window_ms(&mut self, ms: f32, cx: &mut Context<Self>) {
        self.config.window_ms = ms;
        cx.notify();
    }

    fn set_trigger_level(&mut self, level: f32, cx: &mut Context<Self>) {
        self.config.trigger_level = level;
        cx.notify();
    }

    fn set_line_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.config.line_width = width;
        cx.notify();
    }

    fn set_persistence(&mut self, persistence: f32, cx: &mut Context<Self>) {
        self.config.persistence = persistence;
        cx.notify();
    }

    fn set_gain(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.gain = GAIN_MIN * (GAIN_MAX / GAIN_MIN).powf(fraction.clamp(0.0, 1.0));
        cx.notify();
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
            fn(&OscilloscopePanel) -> bool,
            fn(&mut OscilloscopePanel),
        );
        let toggles: Vec<ConfigToggle> = vec![
            (
                rox_i18n::t!("oscilloscope-grid"),
                |this| this.config.grid,
                |this| this.config.grid = !this.config.grid,
            ),
            (
                rox_i18n::t!("oscilloscope-fill"),
                |this| this.config.fill,
                |this| this.config.fill = !this.config.fill,
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
        // the only rate new samples arrive at. Frame polling is just for the
        // trail burning off after audio stops; once it's gone the panel
        // parks, and a resume wakes it through the pump's play-state notify.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        // Freeze on pause holds the standing frame: paused mid-session, not a
        // played-out queue.
        let hold = self.config.freeze && session && !playing && !player.queue_ended();
        if !playing && self.scope.lock().unwrap().alive {
            window.request_animation_frame();
        }

        let config = self.config.clone();
        let scope = self.scope.clone();
        let feed = self.feed.clone();
        div().size_full().relative().bg(palette::bg_root()).child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    // A column per horizontal pixel: any denser and the
                    // min/max pairs are resampling into subpixels nobody
                    // sees.
                    let cols = (f32::from(bounds.size.width) as usize).clamp(MIN_COLS, MAX_COLS);
                    let mut scope = scope.lock().unwrap();
                    scope.step(&feed, cols, &config, hold);
                    scope.paint(bounds, window, &config);
                },
            )
            .size_full(),
        )
    }
}

impl PanelSettings for OscilloscopePanel {
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
        // The custom ramp's pickers on first need; each edit writes its hex
        // back into the config, the format the layout dump stores.
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
        let window_ms = self.config.window_ms();
        let gain = self.config.gain();
        let level = self.config.trigger_level();
        let line_w = self.config.line_w();
        let persistence = self.config.persistence();
        // The slice of samples a frame draws: how long a window, how far it's
        // lifted, where it's cut, and which channels reach the trace.
        let signal = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("oscilloscope-window"),
                Some(rox_i18n::t!("oscilloscope-window.description")),
                settings_ui::scalar(
                    &self.window_scrub,
                    &self.value_edit,
                    window_ms,
                    settings_ui::span(WINDOW_MS_MIN, WINDOW_MS_MAX, " ms"),
                    Self::set_window_ms,
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("oscilloscope-gain"),
                Some(rox_i18n::t!("oscilloscope-gain.description")),
                panel::value_slider_edit(
                    &self.gain_scrub,
                    &self.value_edit,
                    (gain / GAIN_MIN).ln() / (GAIN_MAX / GAIN_MIN).ln(),
                    format!("{gain:.2}x"),
                    format!("{gain:.2}"),
                    |v| (v.max(GAIN_MIN) / GAIN_MIN).ln() / (GAIN_MAX / GAIN_MIN).ln(),
                    Self::set_gain,
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("oscilloscope-trigger"),
                Some(rox_i18n::t!("oscilloscope-trigger.description")),
                choices_shared(
                    &trigger_choices(),
                    self.config.trigger,
                    |this: &mut Self, trigger, cx| {
                        this.config.trigger = trigger;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.trigger != Trigger::Off, |d| {
                d.child(setting_row(
                    rox_i18n::t!("oscilloscope-trigger-level"),
                    Some(rox_i18n::t!("oscilloscope-trigger-level.description")),
                    settings_ui::scalar(
                        &self.level_scrub,
                        &self.value_edit,
                        level,
                        settings_ui::span(-1.0, 1.0, "").decimals(2).hard(),
                        Self::set_trigger_level,
                        cx,
                    ),
                ))
            })
            .child(setting_row(
                rox_i18n::t!("oscilloscope-channels"),
                Some(rox_i18n::t!("oscilloscope-channels.description")),
                choices_shared(
                    &channel_choices(),
                    self.config.channels,
                    |this: &mut Self, channels, cx| {
                        this.config.channels = channels;
                        cx.notify();
                    },
                    cx,
                ),
            ));
        // How the line itself is drawn.
        let trace = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("oscilloscope-line-width"),
                Some(rox_i18n::t!("oscilloscope-line-width.description")),
                settings_ui::scalar(
                    &self.line_w_scrub,
                    &self.value_edit,
                    line_w,
                    settings_ui::span(LINE_W_MIN, LINE_W_MAX, " px").decimals(1),
                    Self::set_line_width,
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("oscilloscope-fill"),
                Some(rox_i18n::t!("oscilloscope-fill.description")),
                toggle(
                    self.config.fill,
                    |this: &mut Self, on, cx| {
                        this.config.fill = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("oscilloscope-persistence"),
                Some(rox_i18n::t!("oscilloscope-persistence.description")),
                settings_ui::scalar(
                    &self.persist_scrub,
                    &self.value_edit,
                    persistence,
                    settings_ui::span(0.0, PERSIST_MAX, "").decimals(2).hard(),
                    Self::set_persistence,
                    cx,
                ),
            ));
        // The ramp the trace is painted with.
        let color = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("oscilloscope-gradient-mode"),
                Some(rox_i18n::t!("oscilloscope-gradient-mode.description")),
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
        div()
            .flex()
            .flex_col()
            .gap(settings_ui::SECTION_GAP)
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-signal"),
                None,
                signal,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("oscilloscope-section-trace"),
                None,
                trace,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-color"),
                None,
                color,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-scale"),
                None,
                setting_row(
                    rox_i18n::t!("oscilloscope-grid"),
                    Some(rox_i18n::t!("oscilloscope-grid.description")),
                    toggle(
                        self.config.grid,
                        |this: &mut Self, on, cx| {
                            this.config.grid = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ),
            ))
            .into_any_element()
    }

    /// Hold on Pause sits on the shared Behavior page rather than here: it's
    /// about how the panel acts when the audio stops, not how the trace is
    /// drawn, and that's where every other panel keeps its behavior switches.
    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(
            settings_ui::section(
                rox_i18n::t!("viz-section-playback"),
                None,
                setting_row(
                    rox_i18n::t!("oscilloscope-hold-on-pause"),
                    Some(rox_i18n::t!("oscilloscope-hold-on-pause.description")),
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

impl EventEmitter<PanelEvent> for OscilloscopePanel {}

impl Focusable for OscilloscopePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for OscilloscopePanel {
    fn panel_name(&self) -> &'static str {
        "oscilloscope"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-oscilloscope"),
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
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
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
                OscilloscopePanel::new(state, config, cx)
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

impl Render for OscilloscopePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        // The panel is a focus stop: a click puts the keyboard here and
        // tab walks to it, which is also what puts its tab group on the
        // focus path for the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trigger locks to a sine's rising zero crossing; the frame has to
    /// start on one rather than wherever the pull happened to begin.
    #[test]
    fn the_trigger_finds_the_first_rising_crossing() {
        let samples: Vec<f32> = (0..400)
            .map(|i| (i as f32 * std::f32::consts::TAU / 40.0).sin())
            .collect();
        let at = trigger_at(&samples, 200, Trigger::Rising, 0.0).unwrap();
        assert!(at < 200);
        assert!(samples[at] <= 0.0 && samples[at + 1] > 0.0);
    }

    #[test]
    fn the_falling_trigger_takes_the_other_side() {
        let samples: Vec<f32> = (0..400)
            .map(|i| (i as f32 * std::f32::consts::TAU / 40.0).sin())
            .collect();
        let at = trigger_at(&samples, 200, Trigger::Falling, 0.0).unwrap();
        assert!(samples[at] >= 0.0 && samples[at + 1] < 0.0);
    }

    /// Silence never crosses, and the caller has to be told so rather than
    /// handed an offset that would jitter frame to frame.
    #[test]
    fn no_crossing_reports_none() {
        let flat = vec![0.5f32; 100];
        assert_eq!(trigger_at(&flat, 50, Trigger::Rising, 0.0), None);
        assert_eq!(trigger_at(&flat, 50, Trigger::Off, 0.0), None);
        assert_eq!(trigger_at(&flat, 0, Trigger::Rising, 0.0), None);
    }

    #[test]
    fn columns_keep_both_ends_of_what_they_cover() {
        // Four samples into two columns: each column spans a pair, and the
        // spike in the second must be kept rather than decimated out.
        let lane = resample(&[0.0, -0.5, 1.0, 0.25], 2);
        assert_eq!(lane, vec![(-0.5, 0.0), (0.25, 1.0)]);
    }

    #[test]
    fn a_short_window_interpolates_instead_of_stepping() {
        let lane = resample(&[0.0, 1.0], 3);
        assert_eq!(lane.len(), 3);
        assert_eq!(lane[0], (0.0, 0.0));
        assert_eq!(lane[1], (0.5, 0.5));
        assert_eq!(lane[2], (1.0, 1.0));
    }

    #[test]
    fn garbage_samples_flatten_rather_than_escaping() {
        let lane = resample(&[f32::NAN, f32::NAN, f32::NAN, f32::NAN], 2);
        assert!(lane.iter().all(|&(lo, hi)| lo == 0.0 && hi == 0.0));
    }

    /// A hand-edited layout is the one place these can arrive broken, and a
    /// NaN would take the whole trace with it.
    #[test]
    fn config_accessors_swallow_junk() {
        let config = OscilloscopeConfig {
            window_ms: f32::NAN,
            gain: -4.0,
            trigger_level: 12.0,
            line_width: f32::NAN,
            persistence: f32::INFINITY,
            ..OscilloscopeConfig::default()
        };
        assert_eq!(config.window_ms(), 20.0);
        assert_eq!(config.gain(), GAIN_MIN);
        assert_eq!(config.trigger_level(), 1.0);
        assert_eq!(config.line_w(), 1.5);
        assert_eq!(config.persistence(), PERSIST_MAX);
        assert_eq!(config.trails(), MAX_TRAILS);
    }

    #[test]
    fn persistence_off_keeps_no_trail() {
        assert_eq!(OscilloscopeConfig::default().trails(), 0);
    }
}
