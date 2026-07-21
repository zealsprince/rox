//! The spectrum panel: live frequency bars over the player's PCM tap, the
//! classic analyzer look - log-spaced bands, snappy attack, eased decay,
//! peak-hold caps falling under gravity, dB gridlines behind. Everything is
//! paint primitives on the UI thread: one FFT per frame while audio flows,
//! and once the bars have settled the panel stops asking for frames, so an
//! idle app pays nothing. The analyzed range, the FFT window size (split
//! zoning trades reactivity for resolution per end of the range), the bar
//! width and fill style, the peak-hold caps and their gravity, and the
//! octave pitch markers are per-view config the customize window edits and
//! the layout dump carries.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, fill, linear_color_stop, linear_gradient, point, prelude::*, px, relative, size,
    AnyElement, App, BorderStyle, Bounds, Context, Div, EventEmitter, FocusHandle, Focusable, Rgba,
    SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::analysis::{log_bands, Analyzer, MAX_FFT_SIZE, MIN_FFT_SIZE};
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, choices, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState,
};
use crate::panel_settings;

// Bars follow the visualizer rhythm at the configured width over the shared
// gap (`tokens::BAR_GAP`); the count collapses on narrow panels instead of
// thinning the bars, so a small dock split doesn't smear. The ceiling covers
// a wide panel at the thinnest bars.
const MIN_BARS: usize = 16;
const MAX_BARS: usize = 512;

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

/// The frequency band the bounds sliders (and a hand-edited config) may pick
/// between: roughly the audible range up to a typical Nyquist ceiling.
const SLIDER_MIN_HZ: f32 = 20.0;
const SLIDER_MAX_HZ: f32 = 20_000.0;

/// The smallest span the low and high bounds keep between them, so the band
/// mapping always has room and never inverts.
const MIN_RATIO: f32 = 2.0;

/// C0's pitch; each octave up doubles it. The pitch markers walk these.
const C0_HZ: f32 = 16.352;

/// dB window the bars normalize into, on magnitudes where a full-scale sine
/// sits at 0 dB. The top leaves headroom so a busy mix pins near full height
/// without every band clipping there.
const FLOOR_DB: f32 = -66.0;
const MAX_DB: f32 = -12.0;

/// Per-second smoothing rates: bands jump up fast and fall slowly, which is
/// what makes kicks read as kicks instead of flicker.
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
/// their targets instead of dipping toward silence - the dip-and-reattack
/// used to read as shimmer on high-refresh displays and as a full strobe
/// under load. Paused and stopped push nothing and cross this quickly;
/// playing audio always pushes, silence included.
const SILENT_AFTER: f32 = 0.15;

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
    /// them out. Also feeds the bar count, so a wider gap fits fewer bars.
    pub bar_gap: f32,
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
    /// Color bars by loudness - a ramp from a dim floor up to the accent -
    /// instead of a flat accent fill, so only the peaks light up.
    pub gradient: bool,
    /// Draw each bar as a hollow outline instead of a filled ramp.
    pub outline: bool,
    /// Stroke thickness of the hollow bars, px.
    pub outline_width: f32,
    /// Peak-hold caps riding above the bars.
    pub caps: bool,
    /// Freeze the bars while playback is paused instead of letting them
    /// fall to silence.
    pub freeze: bool,
    /// How hard the caps fall, bar heights per second squared.
    pub cap_gravity: f32,
    /// Draw octave pitch markers (C1, C2, ...) across the analyzed range.
    pub labels: bool,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        SpectrumConfig {
            chrome: PanelChrome::default(),
            freq_lo: 30.0,
            freq_hi: 16_000.0,
            bar_width: tokens::BAR_W,
            bar_gap: tokens::BAR_GAP,
            fft_size: 8192,
            split: false,
            split_hz: 1_000.0,
            fft_size_hi: MAX_FFT_SIZE,
            gradient: false,
            outline: false,
            outline_width: 1.0,
            caps: true,
            freeze: false,
            cap_gravity: HOLD_GRAVITY,
            labels: false,
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

    /// The bar thickness, outline stroke, and cap gravity, clamped to
    /// their slider spans like [`Self::range`] clamps the bounds.
    fn bar_w(&self) -> f32 {
        self.bar_width.clamp(BAR_W_MIN, BAR_W_MAX)
    }

    fn bar_gap(&self) -> f32 {
        self.bar_gap.clamp(BAR_GAP_MIN, BAR_GAP_MAX)
    }

    fn outline_w(&self) -> f32 {
        self.outline_width.clamp(OUTLINE_W_MIN, OUTLINE_W_MAX)
    }

    fn gravity(&self) -> f32 {
        self.cap_gravity.clamp(GRAVITY_MIN, GRAVITY_MAX)
    }

    /// The FFT sizes, snapped to the picker's power-of-two steps so a
    /// hand-edited file can't feed the analyzer a bad size.
    fn fft_lo(&self) -> usize {
        self.fft_size
            .next_power_of_two()
            .clamp(MIN_FFT_SIZE, MAX_FFT_SIZE)
    }

    fn fft_hi(&self) -> usize {
        self.fft_size_hi
            .next_power_of_two()
            .clamp(MIN_FFT_SIZE, MAX_FFT_SIZE)
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

/// A bound's Hz for the slider readout, compact enough for the strip.
fn fmt_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{:.0} Hz", hz.round())
    }
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
    fn step(&mut self, feed: &AudioFeed, width: f32, config: &SpectrumConfig, hold: bool) {
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
            ((width / (config.bar_w() + config.bar_gap())) as usize).clamp(MIN_BARS, MAX_BARS);
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
        // that remaps the bars still lands: the feed keeps the last window,
        // so the frame re-analyzes below at the new mapping instead of
        // ignoring the edit until playback resumes.
        if hold && !fresh && !remap {
            self.alive = false;
            return;
        }

        // New audio since last tick: analyze the latest window per zone and
        // refresh the targets. Nothing new: hold the targets - it's just
        // the gap between pump ticks - until the feed has sat still long
        // enough to read as stopped, then let the bars fall to silence.
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
                    // Frozen: the frame changed mapping, not time. Land on
                    // the new targets at once - the next tick parks again,
                    // so an ease would strand the bars partway.
                    self.levels[i] = target;
                } else {
                    let rate = if target > self.levels[i] {
                        ATTACK
                    } else {
                        RELEASE
                    };
                    self.levels[i] += (target - self.levels[i]) * (rate * dt).min(1.0);
                }

                // The cap rides up with the bar and falls back under gravity
                // once the bar drops away. Caps off: the holds shadow the
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

        let max_h = h * 0.94;
        let step = w / count as f32;
        let bar_w = (step - config.bar_gap()).max(1.0);

        // dB gridlines behind the bars.
        for db in DB_MARKS {
            let y = h - (db - FLOOR_DB) / (MAX_DB - FLOOR_DB) * max_h;
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x, bounds.origin.y + px(y)),
                    size(px(w), px(1.0)),
                ),
                palette::alpha(palette::gridline(), 0x28),
            ));
        }

        for i in 0..count {
            let bar_h = (self.levels[i] * max_h).max(2.0);
            let x = bounds.origin.x + px(i as f32 * step);
            // The bar base color: flat accent, or a loudness ramp from a dim
            // floor up to the accent so only the peaks read hot.
            let base = bar_color(self.levels[i], config.gradient);
            let bar = Bounds::new(
                point(x, bounds.origin.y + px(h - bar_h)),
                size(px(bar_w), px(bar_h)),
            );
            if config.outline {
                // Hollow variant: the bar as a frame in its base color, at
                // the configured stroke width.
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
                    // Angle 0 points the gradient line at the top: solid base
                    // at the baseline fading out toward the bar tip.
                    linear_gradient(
                        0.0,
                        linear_color_stop(base, 0.0),
                        linear_color_stop(palette::alpha(base, 0x40), 1.0),
                    ),
                ));
            }

            if !config.caps {
                continue;
            }
            // Solid peak-hold cap sitting at the held level above the bar:
            // a position mark like the playheads and slider knobs, so it
            // wears the highlight and stays legible over accent-colored
            // bars.
            let cap_y = (h - self.holds[i] * max_h - 1.0).max(0.0);
            window.paint_quad(fill(
                Bounds::new(
                    point(x, bounds.origin.y + px(cap_y)),
                    size(px(bar_w), px(1.0)),
                ),
                palette::highlight(),
            ));
        }
    }
}

/// A bar's base color for its level. Flat mode is the accent everywhere;
/// intensity mode blends from a dim floor up to the accent, curved so mid
/// bars stay muted and only the loud ones light up.
fn bar_color(level: f32, gradient: bool) -> Rgba {
    if !gradient {
        return palette::accent();
    }
    let t = level.clamp(0.0, 1.0).powf(1.5);
    palette::mix(
        palette::alpha(palette::text_faint(), 0x66),
        palette::accent(),
        t,
    )
}

/// The octave pitch markers over the analyzed range: a faint divider at each
/// C with its label at the bottom. Positions are log-frequency fractions, so
/// they line up with the bars at any panel width without knowing the pixels.
fn labels_overlay(freq_lo: f32, freq_hi: f32) -> Div {
    let span = (freq_hi / freq_lo).ln();
    let mut overlay = div().absolute().inset_0();
    for octave in 0..=10 {
        let freq = C0_HZ * 2f32.powi(octave);
        if freq < freq_lo || freq > freq_hi {
            continue;
        }
        let frac = (freq / freq_lo).ln() / span;
        // A label pinned to the far right edge would clip; drop it and keep
        // the divider.
        let labeled = frac <= 0.97;
        overlay = overlay.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(frac))
                .border_l_1()
                .border_color(palette::alpha(palette::gridline(), 0x1f))
                .flex()
                .flex_col()
                .justify_end()
                .when(labeled, |d| {
                    d.child(
                        div()
                            .pl(px(3.))
                            .pb(px(2.))
                            .text_xs()
                            .text_color(palette::text_faint())
                            .child(format!("C{octave}")),
                    )
                }),
        );
    }
    overlay
}

/// A labelled config toggle for the Display menu: the row label, a getter for
/// its current state, and a setter that flips it.
type ConfigToggle = (&'static str, fn(&SpectrumPanel) -> bool, fn(&mut SpectrumPanel));

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
    outline_w_scrub: ScrubState,
    gravity_scrub: ScrubState,
    split_scrub: ScrubState,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
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
            outline_w_scrub: ScrubState::default(),
            gravity_scrub: ScrubState::default(),
            split_scrub: ScrubState::default(),
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

    fn set_bar_width(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.bar_width = (BAR_W_MIN + fraction * (BAR_W_MAX - BAR_W_MIN)).round();
        cx.notify();
    }

    fn set_bar_gap(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.bar_gap = (BAR_GAP_MIN + fraction * (BAR_GAP_MAX - BAR_GAP_MIN)).round();
        cx.notify();
    }

    fn set_outline_width(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.outline_width =
            (OUTLINE_W_MIN + fraction * (OUTLINE_W_MAX - OUTLINE_W_MIN)).round();
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
    /// Hz readout alongside.
    fn freq_slider(
        &self,
        scrub: &ScrubState,
        hz: f32,
        apply: fn(&mut Self, f32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        panel::value_slider(scrub, hz_to_frac(hz), fmt_hz(hz), apply, cx)
    }

    /// The panel's own dropdown entries: a Display flyout of the toggles the
    /// customize window also holds, for a quick flip without opening it.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let toggles: [ConfigToggle; 4] = [
            (
                "Intensity Color",
                |this| this.config.gradient,
                |this| this.config.gradient = !this.config.gradient,
            ),
            (
                "Outline Bars",
                |this| this.config.outline,
                |this| this.config.outline = !this.config.outline,
            ),
            (
                "Peak Caps",
                |this| this.config.caps,
                |this| this.config.caps = !this.config.caps,
            ),
            (
                "Pitch Labels",
                |this| this.config.labels,
                |this| this.config.labels = !this.config.labels,
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
        menu.item(PopupMenuItem::submenu("Display", submenu))
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
        &[("Display", icons::EYE)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bar_w = self.config.bar_w();
        let bar_gap = self.config.bar_gap();
        let outline_w = self.config.outline_w();
        let gravity = self.config.gravity();
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                "Low Bound",
                Some("Lowest frequency the bars analyze"),
                self.freq_slider(&self.lo_scrub, self.config.freq_lo, Self::set_freq_lo, cx),
            ))
            .child(setting_row(
                "High Bound",
                Some("Highest frequency the bars analyze"),
                self.freq_slider(&self.hi_scrub, self.config.freq_hi, Self::set_freq_hi, cx),
            ))
            .child(setting_row(
                "Bar Width",
                Some("How thick each bar draws, thinner bars fit more bands"),
                panel::value_slider(
                    &self.bar_w_scrub,
                    (bar_w - BAR_W_MIN) / (BAR_W_MAX - BAR_W_MIN),
                    format!("{bar_w:.0} px"),
                    Self::set_bar_width,
                    cx,
                ),
            ))
            .child(setting_row(
                "Bar Gap",
                Some("Space between bars, wider gaps fit fewer bars"),
                panel::value_slider(
                    &self.bar_gap_scrub,
                    (bar_gap - BAR_GAP_MIN) / (BAR_GAP_MAX - BAR_GAP_MIN),
                    format!("{bar_gap:.0} px"),
                    Self::set_bar_gap,
                    cx,
                ),
            ))
            .child(setting_row(
                "FFT Size",
                Some("Analysis window; short reacts fast, long resolves finer"),
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
                "Split Zones",
                Some("Analyze below and above a split frequency at different window sizes"),
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
                    "Split At",
                    Some("Where the zones meet, snapped to the nearest bar"),
                    self.freq_slider(
                        &self.split_scrub,
                        self.config.split_hz,
                        Self::set_split_hz,
                        cx,
                    ),
                ))
                .child(setting_row(
                    "High FFT Size",
                    Some("Analysis window for the bands above the split"),
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
                "Intensity Color",
                Some("Color bars by loudness so only the peaks light up, instead of a flat fill"),
                toggle(
                    self.config.gradient,
                    |this: &mut Self, on, cx| {
                        this.config.gradient = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Outline Bars",
                Some("Draw each bar as a hollow outline instead of a filled ramp"),
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
                    "Outline Width",
                    Some("Stroke thickness of the hollow bars"),
                    panel::value_slider(
                        &self.outline_w_scrub,
                        (outline_w - OUTLINE_W_MIN) / (OUTLINE_W_MAX - OUTLINE_W_MIN),
                        format!("{outline_w:.0} px"),
                        Self::set_outline_width,
                        cx,
                    ),
                ))
            })
            .child(setting_row(
                "Peak Caps",
                Some("Hold a mark at each band's recent peak"),
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
                "Hold on Pause",
                Some("Freeze the bars while paused instead of letting them fall to silence"),
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
                "Cap Gravity",
                Some("How hard the peak marks fall once the band drops away"),
                panel::value_slider(
                    &self.gravity_scrub,
                    (gravity / GRAVITY_MIN).ln() / (GRAVITY_MAX / GRAVITY_MIN).ln(),
                    format!("{gravity:.2}"),
                    Self::set_gravity,
                    cx,
                ),
            ))
            .child(setting_row(
                "Pitch Labels",
                Some("Mark the octaves (C1, C2, ...) across the range"),
                toggle(
                    self.config.labels,
                    |this: &mut Self, on, cx| {
                        this.config.labels = on;
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
        panel::title_text(self.config.chrome.title.as_deref(), "Spectrum")
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

    /// The layout dump carries the panel's config; the builder registered in
    /// `workspace::register_panels` reads it back.
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
        // The config block: the panel's quick toggles and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, window, cx);
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the config along, like the cover panel's.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| SpectrumPanel::new(state, config, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
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
        // every pump tick - the only rate new samples arrive at, so frames
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
                    let mut bars = bars.lock().unwrap();
                    bars.step(&feed, f32::from(bounds.size.width), &config, hold);
                    bars.paint(bounds, window, &config);
                },
            )
            .size_full(),
        );
        if self.config.labels {
            root = root.child(labels_overlay(freq_lo, freq_hi));
        }
        // While the split slider drags, mark where the zones meet so the
        // pick lands by eye; the playhead's alpha keeps it legible.
        if self.config.split && self.split_scrub.is_dragging() {
            let split = self.config.split_hz.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
            let frac = (split / freq_lo).ln() / (freq_hi / freq_lo).ln();
            if (0.0..=1.0).contains(&frac) {
                root = root.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(frac))
                        .border_l_1()
                        .border_color(palette::alpha(palette::highlight(), 0xd9)),
                );
            }
        }
        root
    }
}
