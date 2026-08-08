//! The equalizer window: ten octave bands, an enable switch, and a flatten
//! button, opened from the Application menu beside Stats and the Console.
//! Its own window rather than a settings page because it's an instrument you
//! work while the music plays, not a preference you set and close.
//!
//! Global, so it takes no state of its own. The curve is one set of live
//! atomics for the whole process (see [`crate::player::eq_gain`] and ADR 19),
//! which is what lets a band move under whatever is playing without the
//! window holding a player: every workspace builds its own world, and the EQ
//! is meant to sit across all of them. It borrows the front workspace's art
//! tint the way the console does.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, fill, point, prelude::*, px, relative, size, App, Bounds, Context, Div, Global,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path, Pixels, Point,
    ScrollWheelEvent, Subscription, WeakEntity, Window, WindowHandle,
};
use gpui_component::Root;

use rox_panel_kit::axis::fmt_axis_hz;
use rox_playback::eq::{BANDS, FREQ_MAX, FREQ_MIN, GAIN_MAX_DB, Q_MAX, Q_MIN};
use rox_playback::latency::{self, LatencyHold};
use rox_viz::analysis::{self, Analyzer};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, ScrubState};
use crate::player;
use crate::settings::ui::{self as settings_ui, small_button};
use crate::settings::{AnalyzerStyle, LayoutSize, Settings};

/// The plot's dB range either side of flat. Wider than a single band's
/// ceiling on purpose: a stack of overlapping boosts sums past 12 dB, and a
/// curve that clipped at the edge would hide exactly the case worth seeing.
const PLOT_DB: f32 = 18.0;

/// How many points the curve is sampled at across the plot. Enough that a
/// narrow band at high Q still draws as a bell rather than a spike, cheap
/// enough to redo every frame of a drag.
const CURVE_POINTS: usize = 192;

/// A band handle's radius, and the reach of its grab area. The grab is
/// bigger than the dot because a 7px target is a fiddle on a trackpad.
const NODE_R: f32 = 7.0;
const NODE_GRAB: f32 = 20.0;

/// How wide a label's box on the scale is. The box is what gets centered on
/// the gridline, since an element can't be offset by its own width.
const AXIS_LABEL_W: f32 = 40.0;

/// How many bars the analyzer behind the curve is folded into. Fine enough
/// to read as a spectrum, coarse enough that each bar still gathers several
/// FFT bins down at the bottom where they're sparse.
const SPECTRUM_BARS: usize = 96;

/// The analysis windows the picker offers. Long windows resolve the bottom
/// of the range, where a whole EQ band spans a few dozen Hz and a short FFT
/// smears the lot into one bin; short windows keep up with the music. The
/// default sits at the long end, since what's being read here is where a
/// band sits rather than how hard a kick landed.
const FFT_CHOICES: [usize; 6] = [512, 1024, 2048, 4096, 8192, 16384];

/// The dB range the analyzer is drawn across. Anything under the floor is
/// silence as far as the backdrop is concerned.
const SPECTRUM_FLOOR_DB: f32 = -78.0;
const SPECTRUM_CEIL_DB: f32 = -6.0;

/// How fast a bar falls once the music lets go of it, in fractions of full
/// scale per second. Rising is instant: a transient should show up on the
/// frame it arrives, and only the decay wants smoothing.
const SPECTRUM_FALL: f32 = 1.6;

/// How far either side the wave averages when it smooths the bands. Enough
/// to pull the FFT's bin-to-bin jitter out of the outline without flattening
/// the peak that says which band to reach for.
const WAVE_SMOOTH: usize = 3;

/// How tall the analyzer is allowed to stand, as a fraction of the plot. It
/// sits behind the curve as context, and at full height it reads as the
/// subject instead.
const SPECTRUM_HEIGHT: f32 = 0.62;

/// The band levels with their neighbours folded in, for the wave outline. A
/// triangular kernel rather than a box, so a real peak still stands while
/// the noise between bins settles.
fn smoothed(bars: &[f32]) -> Vec<f32> {
    (0..bars.len())
        .map(|i| {
            let (mut sum, mut weight) = (0.0, 0.0);
            for offset in -(WAVE_SMOOTH as isize)..=(WAVE_SMOOTH as isize) {
                let Some(level) = usize::try_from(i as isize + offset)
                    .ok()
                    .and_then(|j| bars.get(j))
                else {
                    continue;
                };
                let w = (WAVE_SMOOTH as f32 + 1.0) - offset.unsigned_abs() as f32;
                sum += level * w;
                weight += w;
            }
            if weight > 0.0 {
                sum / weight
            } else {
                0.0
            }
        })
        .collect()
}

/// Where a frequency sits across the plot, 0 at the left edge and 1 at the
/// right. Log, because that's how the ear reads the spectrum and how every
/// EQ has drawn it since the first one had a screen.
fn freq_frac(hz: f32) -> f32 {
    let (lo, hi) = (FREQ_MIN.log10(), FREQ_MAX.log10());
    ((hz.max(FREQ_MIN).log10() - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// The frequency at a fraction across the plot, the inverse of the above.
fn frac_freq(frac: f32) -> f32 {
    let (lo, hi) = (FREQ_MIN.log10(), FREQ_MAX.log10());
    10f32.powf(lo + frac.clamp(0.0, 1.0) * (hi - lo))
}

/// Every gridline across the plot: its frequency, where it lands, and
/// whether it's one of the labelled ladder steps. The spectrum panel's
/// frequency scale rules the same ladder.
fn axis_marks() -> Vec<(f32, f32, bool)> {
    analysis::hz_ladder(FREQ_MIN, FREQ_MAX)
}

/// The FFT picker's label for a window size.
fn fmt_fft(size: usize) -> String {
    if size >= 1024 {
        format!("{}k FFT", size / 1024)
    } else {
        format!("{size} FFT")
    }
}

/// A window size snapped to what the analyzer will take, so a hand-edited
/// settings file can't hand it a size it panics on.
fn fft_size(size: usize) -> usize {
    size.next_power_of_two()
        .clamp(analysis::MIN_FFT_SIZE, analysis::MAX_FFT_SIZE)
}

/// Where a gain sits down the plot, 0 at the top and 1 at the bottom.
fn gain_frac(db: f32) -> f32 {
    (0.5 - db / (2.0 * PLOT_DB)).clamp(0.0, 1.0)
}

/// The gain at a fraction down the plot, the inverse of the above.
fn frac_gain(frac: f32) -> f32 {
    ((0.5 - frac) * 2.0 * PLOT_DB).clamp(-GAIN_MAX_DB, GAIN_MAX_DB)
}

/// The open equalizer window, if any: opening again focuses it rather than
/// stacking a second one, the stats and console move.
struct OpenEq(WindowHandle<Root>);

impl Global for OpenEq {}

/// Open the equalizer, or bring the open one to the front.
///
/// Deferred for the same reason the console is: the menu action that opens it
/// runs inside the workspace's own update, and reading the front workspace for
/// the tint mid-update would panic.
pub fn open(cx: &mut App) {
    cx.defer(open_now);
}

fn open_now(cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenEq>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let state = crate::workspace::front_workspace(cx).map(|(_, state)| state);
    let min = settings_ui::MIN_SIZE;
    let (width, height) = Settings::load()
        .windows
        .eq
        .filter(|s| s.width >= f32::from(min.width) && s.height >= f32::from(min.height))
        .map(|s| (s.width, s.height))
        // Sized for the plot rather than the readouts under it: a curve you
        // drag bands around wants room, and the old height was measured back
        // when this was ten slider rows.
        .unwrap_or((620., 660.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = crate::panel::open_child_window(
        cx,
        "rox - Equalizer",
        bounds,
        Some(min),
        move |window, cx| cx.new(|cx| EqWindow::new(state, window, cx)),
    );
    cx.set_global(OpenEq(handle));
}

struct EqWindow {
    /// The workspace that was in front when this opened, for the art tint and
    /// the transport strip. None when the EQ was opened with no workspace up,
    /// which leaves the strip out rather than guessing at a player.
    state: Option<AppState>,
    /// Where each strip painted last and whether its drag is live. On the
    /// window, so two of these could coexist if the single-instance rule ever
    /// relaxes.
    scrubs: [ScrubState; BANDS],
    /// The one typed-readout slot, so only one band is ever being typed into.
    value_edit: panel::ValueEdit,
    /// Where the plot landed, off its own paint. The drag maps pointer
    /// positions through this, so it has to come from the layout rather than
    /// from anything assumed about the window.
    plot: Arc<Mutex<Option<Bounds<Pixels>>>>,
    /// The band a drag is carrying, None when nothing is held. A drag that
    /// wanders off the plot keeps its grip, which is what makes pulling a
    /// band to the ceiling feel like a fader instead of a slippery dot.
    grabbed: Option<usize>,
    /// How far the pointer sat from the handle's center when the grab
    /// started. The drag carries it, so grabbing a dot by its rim moves the
    /// band with the pointer instead of snapping it under one.
    grab_offset: Point<Pixels>,
    /// The band the readouts below the plot are editing. Clicking a handle
    /// moves it; it survives the drag ending so the numbers stay put to be
    /// typed into.
    selected: usize,
    /// The analyzer behind the curve, and what it needs to keep going: the
    /// FFT, a mono window to fill from the feed, the folded bar levels, and
    /// the bin ranges those bars gather. The mapping is rebuilt whenever the
    /// device rate changes under it.
    analyzer: Analyzer,
    mono: Vec<f32>,
    bars: Vec<f32>,
    bins: Vec<(usize, usize)>,
    /// The rate `bins` was mapped for, so a device switch remaps instead of
    /// drawing the old layout against new audio.
    bin_rate: u32,
    /// The analysis window in use, snapped to a size the analyzer takes.
    /// Held here rather than read back off the settings file every frame.
    fft: usize,
    /// The feed's write counter as of the last analysis, for telling new
    /// audio from a repaint that just happens to land between pump ticks.
    last_written: u64,
    last_tick: Instant,
    /// How the analyzer is drawn. Held here rather than read off the
    /// settings file each frame, and written back when the picker moves.
    analyzer_style: AnalyzerStyle,
    /// Repaints the transport strip when playback moves under it.
    _player_changed: Option<Subscription>,
    /// Repaints when the curve moves somewhere else, an EQ widget's toggle in
    /// a workspace window being the one that's easy to hit while this is open.
    _eq_changed: Subscription,
    /// Keeps the sample ring shallow for as long as this window lives (ADR
    /// 19), so a band lands about a tenth of a second behind the drag
    /// instead of half a second. Dropped with the window entity, which the
    /// OS close button takes down the same as the menu item does.
    _latency: LatencyHold,
}

impl EqWindow {
    fn new(state: Option<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The OS close button never runs remove_window, so the frame persists
        // through the should-close hook, the stats window's move. The curve
        // itself writes as it is dragged.
        window.on_window_should_close(cx, move |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                let size = s.windows.eq.get_or_insert_with(LayoutSize::default);
                size.width = frame.size.width.into();
                size.height = frame.size.height.into();
            });
            true
        });
        // The transport reads the player every frame, so the window has to
        // wake when playback moves or the play/pause face goes stale the
        // moment a track ends on its own.
        let _player_changed = state
            .as_ref()
            .map(|state| cx.observe(&state.player, |_, _, cx| cx.notify()));
        let eq = Settings::load().eq;
        let fft = fft_size(eq.fft_size);
        EqWindow {
            state,
            scrubs: std::array::from_fn(|_| ScrubState::default()),
            value_edit: panel::ValueEdit::default(),
            plot: Arc::new(Mutex::new(None)),
            grabbed: None,
            grab_offset: point(px(0.), px(0.)),
            selected: 0,
            analyzer: Analyzer::new(fft),
            mono: vec![0.0; fft],
            bars: vec![0.0; SPECTRUM_BARS],
            bins: Vec::new(),
            bin_rate: 0,
            fft,
            last_written: 0,
            last_tick: Instant::now(),
            analyzer_style: eq.analyzer,
            _player_changed,
            _eq_changed: player::observe_eq(cx),
            _latency: latency::hold(),
        }
    }

    /// Retune the analyzer to a different window size: a new FFT, a window
    /// to fill it, and the band mapping dropped so the next frame remaps it
    /// against the new bin count rather than reading the old one short.
    fn set_fft(&mut self, size: usize, cx: &mut Context<Self>) {
        let size = fft_size(size);
        if size == self.fft {
            return;
        }
        self.fft = size;
        self.analyzer = Analyzer::new(size);
        self.mono = vec![0.0; size];
        self.bins.clear();
        Settings::update(move |s| s.eq.fft_size = size);
        cx.notify();
    }

    /// Pull the newest window off the feed and fold it into the bars behind
    /// the curve. Returns whether anything is still moving, which is what
    /// decides if the window asks for another frame.
    ///
    /// Pausing mid-track holds the last frame rather than letting the bars
    /// fall away, the spectrum panel's freeze: the curve is dragged against
    /// what the music was doing, and pausing on the bar worth looking at is
    /// how you get to look at it. A queue that played out has nothing to
    /// hold, so that decays as before and the window stops costing frames.
    fn step_spectrum(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let player = state.player.read(cx);
        let hold = !player.is_playing() && player.now_playing().is_some() && !player.queue_ended();
        let feed = player.feed();
        let rate = feed.sample_rate();
        if rate == 0 {
            return false;
        }
        if self.bin_rate != rate || self.bins.len() != SPECTRUM_BARS {
            self.bins = analysis::log_bands(
                SPECTRUM_BARS,
                FREQ_MIN,
                FREQ_MAX,
                rate,
                self.analyzer.size() / 2,
            );
            self.bin_rate = rate;
        }
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();

        let written = feed.written();
        let fresh = written != self.last_written;
        self.last_written = written;
        // Frozen: the bars stay exactly where the pause caught them, and
        // nothing here asks for another frame. The bar count is fixed, so
        // there's no remap that could leave the standing frame drawn
        // against a mapping it wasn't analyzed at.
        if hold && !fresh {
            return false;
        }
        // Only analyze on new audio. Between pump ticks the last frame still
        // stands, and the decay below is what keeps it from looking frozen.
        if fresh && feed.latest_mono(&mut self.mono) == self.mono.len() {
            let mags = self.analyzer.magnitudes(&self.mono);
            for (bar, &(lo, hi)) in self.bins.iter().enumerate() {
                let peak = mags[lo..hi].iter().copied().fold(0.0f32, f32::max);
                let db = 20.0 * (peak + 1e-9).log10();
                let level = ((db - SPECTRUM_FLOOR_DB) / (SPECTRUM_CEIL_DB - SPECTRUM_FLOOR_DB))
                    .clamp(0.0, 1.0);
                // Straight up, eased down: a hit should land on the frame it
                // happened and the tail is the only part worth smoothing.
                self.bars[bar] = if level > self.bars[bar] {
                    level
                } else {
                    (self.bars[bar] - SPECTRUM_FALL * dt).max(level)
                };
            }
        } else {
            for bar in &mut self.bars {
                *bar = (*bar - SPECTRUM_FALL * dt).max(0.0);
            }
        }
        self.bars.iter().any(|level| *level > 0.001)
    }

    /// Where a pointer sits inside the plot, as fractions across and down.
    /// Outside the plot it clamps rather than returning None: a drag that
    /// leaves the box should pin to the edge, not stall.
    fn plot_frac(&self, at: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = (*self.plot.lock().unwrap())?;
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let x = (f32::from(at.x) - f32::from(bounds.origin.x)) / w;
        let y = (f32::from(at.y) - f32::from(bounds.origin.y)) / h;
        Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
    }

    /// Where a band's handle is painted, in window pixels.
    fn node_center(&self, band: usize) -> Option<Point<Pixels>> {
        let bounds = (*self.plot.lock().unwrap())?;
        Some(point(
            bounds.origin.x + bounds.size.width * freq_frac(player::eq_freq(band)),
            bounds.origin.y + bounds.size.height * gain_frac(player::eq_gain(band)),
        ))
    }

    /// The band whose handle is nearest a pointer, if one is within reach.
    /// Distance is measured in the plot's own pixels rather than in
    /// parameter space, so a grab feels the same everywhere on the curve
    /// instead of getting fussy where the log axis is dense.
    fn band_at(&self, at: Point<Pixels>) -> Option<usize> {
        (0..BANDS)
            .filter_map(|band| {
                let center = self.node_center(band)?;
                let distance = f32::from(center.x - at.x).hypot(f32::from(center.y - at.y));
                (distance <= NODE_GRAB).then_some((band, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(band, _)| band)
    }

    /// Take hold of a band, remembering where in the dot it was caught.
    fn grab(&mut self, band: usize, at: Point<Pixels>) {
        self.grabbed = Some(band);
        self.grab_offset = self
            .node_center(band)
            .map(|center| at - center)
            .unwrap_or_else(|| point(px(0.), px(0.)));
    }

    /// Let go of whatever is held, if anything.
    fn release(&mut self, cx: &mut Context<Self>) {
        if self.grabbed.take().is_some() {
            cx.notify();
        }
    }

    /// Put the held band where the pointer is: across for center, down for
    /// gain. Both at once, which is the whole reason a curve editor beats a
    /// column of sliders. The grab offset comes off first, so the band moves
    /// by what the pointer moved rather than jumping under it.
    fn drag_to(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(band) = self.grabbed else { return };
        let Some((x, y)) = self.plot_frac(at - self.grab_offset) else {
            return;
        };
        player::set_eq_freq(band, frac_freq(x), cx);
        player::set_eq_gain(band, frac_gain(y), cx);
        cx.notify();
    }

    /// A transport strip, so the curve can be judged against something
    /// playing without going back to the workspace window for every pause.
    /// Only the four verbs worth having here: the rest of the transport is
    /// a panel away. Centered under the plot, where the eye already is.
    fn transport(&self, cx: &mut Context<Self>) -> Option<Div> {
        let state = self.state.as_ref()?;
        let strip = panel::transport_strip(&state.player.clone(), &state.library.clone(), cx);
        Some(div().flex().flex_row().justify_center().child(strip))
    }

    /// The enable switch and Flatten, the two controls that act on the whole
    /// curve rather than one band.
    fn controls(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(small_button(
                "Flatten",
                icons::MINUS,
                false,
                cx.listener(|_, _, _, cx| player::flatten_eq(cx)),
            ))
            .child(small_button(
                "Reset Bands",
                icons::REFRESH_CW,
                false,
                cx.listener(|_, _, _, cx| player::reset_eq_shape(cx)),
            ))
            .child(panel::picker(
                "eq-analyzer",
                self.analyzer_style,
                vec![
                    (AnalyzerStyle::Wave, "Wave".into()),
                    (AnalyzerStyle::Bars, "Bars".into()),
                    (AnalyzerStyle::Off, "No analyzer".into()),
                ],
                false,
                |this: &mut Self, style, cx| {
                    this.analyzer_style = style;
                    Settings::update(move |s| s.eq.analyzer = style);
                    cx.notify();
                },
                cx,
            ))
            // The window size only means something while something is being
            // drawn with it, so it rides along with the style picker rather
            // than sitting there inert with the analyzer off.
            .when(self.analyzer_style != AnalyzerStyle::Off, |row| {
                row.child(panel::picker(
                    "eq-fft",
                    self.fft,
                    FFT_CHOICES
                        .iter()
                        .map(|&size| (size, fmt_fft(size).into()))
                        .collect(),
                    false,
                    |this: &mut Self, size, cx| this.set_fft(size, cx),
                    cx,
                ))
            })
            .child(panel::toggle(
                player::eq_enabled(),
                |_, on, cx| player::set_eq_enabled(on, cx),
                cx,
            ))
    }

    /// The curve editor: the grid, the cascade's actual response, and a
    /// handle per band that drags in both axes at once. The response comes
    /// from the same coefficients the node runs, so what's drawn is what's
    /// being heard rather than a sketch of the intent.
    fn plot(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.state.as_ref().map(|state| state.player.clone());
        // Sampled once per frame off the live parameters, then handed to the
        // paint closure. Doing it here rather than inside the closure keeps
        // the entity read out of paint, where there's no cx to read with.
        let curve: Vec<f32> = (0..CURVE_POINTS)
            .map(|i| {
                let hz = frac_freq(i as f32 / (CURVE_POINTS - 1) as f32);
                player
                    .as_ref()
                    .map(|player| player.read(cx).eq_response_db(hz))
                    .unwrap_or(0.0)
            })
            .collect();
        let enabled = player::eq_enabled();
        // Handed to paint so a live drag can re-arm its window handlers,
        // None whenever nothing is held.
        let dragging = self.grabbed.map(|_| cx.entity().downgrade());
        let plot = self.plot.clone();
        let bars = self.bars.clone();
        let analyzer = self.analyzer_style;
        // The fill sits back further than the bars did: an outline reads
        // from a fainter wash than a row of solid blocks needs.
        let spectrum = palette::alpha(palette::text_muted(), 0x2a);
        let spectrum_edge = palette::alpha(palette::text_muted(), 0x66);
        let marks = axis_marks();
        let grid = palette::alpha(palette::text_muted(), 0x22);
        // The steps between the labelled marks, drawn far enough back that
        // they read as ruling rather than as ten more gridlines.
        let grid_minor = palette::alpha(palette::text_muted(), 0x0f);
        let zero_line = palette::alpha(palette::text_muted(), 0x44);
        let accent = palette::accent();
        let face = canvas(
            move |bounds, _, _| *plot.lock().unwrap() = Some(bounds),
            move |bounds, _, window, _| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                let at = |fx: f32, fy: f32| point(px(x0 + fx * w), px(y0 + fy * h));
                let line = |a: Point<Pixels>, b: Point<Pixels>| {
                    Bounds::from_corners(a, point(b.x.max(a.x + px(1.)), b.y.max(a.y + px(1.))))
                };
                // The analyzer goes down first, so the grid and the curve
                // both read over it. It's context, not the subject: muted,
                // and rising off the floor rather than off the flat line,
                // since it's level rather than gain and shares only the axis.
                let top = |level: f32| 1.0 - level * SPECTRUM_HEIGHT;
                match analyzer {
                    AnalyzerStyle::Off => {}
                    AnalyzerStyle::Bars if !bars.is_empty() => {
                        let bar_w = 1.0 / bars.len() as f32;
                        for (i, level) in bars.iter().enumerate() {
                            if *level <= 0.001 {
                                continue;
                            }
                            let fx = i as f32 * bar_w;
                            window.paint_quad(fill(
                                Bounds::from_corners(
                                    at(fx, top(*level)),
                                    at((fx + bar_w * 0.85).min(1.0), 1.0),
                                ),
                                spectrum,
                            ));
                        }
                    }
                    AnalyzerStyle::Wave if bars.len() > 1 => {
                        let wave = smoothed(&bars);
                        let step = 1.0 / (wave.len() - 1) as f32;
                        let solid = (point(0., 1.), point(0., 1.), point(0., 1.));
                        let mut area = Path::new(at(0.0, 1.0));
                        let mut line = Path::new(at(0.0, top(wave[0])));
                        for i in 0..wave.len() - 1 {
                            let (fx0, fx1) = (i as f32 * step, (i + 1) as f32 * step);
                            let (t0, t1) = (top(wave[i]), top(wave[i + 1]));
                            area.push_triangle((at(fx0, t0), at(fx1, t1), at(fx1, 1.0)), solid);
                            area.push_triangle((at(fx0, t0), at(fx1, 1.0), at(fx0, 1.0)), solid);
                            let thick = 1.5 / h;
                            line.push_triangle(
                                (at(fx0, t0), at(fx1, t1), at(fx1, t1 + thick)),
                                solid,
                            );
                            line.push_triangle(
                                (at(fx0, t0), at(fx1, t1 + thick), at(fx0, t0 + thick)),
                                solid,
                            );
                        }
                        window.paint_path(area, spectrum);
                        window.paint_path(line, spectrum_edge);
                    }
                    _ => {}
                }
                // The frequency ladder, then flat, then the gain steps either
                // side of it. Flat reads stronger than the rest: it's the
                // line the whole picture is measured against.
                for (_, fx, major) in &marks {
                    let color = if *major { grid } else { grid_minor };
                    window.paint_quad(fill(line(at(*fx, 0.0), at(*fx, 1.0)), color));
                }
                for db in [-12.0, -6.0, 6.0, 12.0] {
                    let fy = gain_frac(db);
                    window.paint_quad(fill(line(at(0.0, fy), at(1.0, fy)), grid));
                }
                let flat = gain_frac(0.0);
                window.paint_quad(fill(line(at(0.0, flat), at(1.0, flat)), zero_line));

                // The curve, as a filled band between flat and the response
                // plus a stroke along the top of it. Triangles because
                // that's what gpui's path takes, the spectrum panel's shape.
                let solid = (point(0., 1.), point(0., 1.), point(0., 1.));
                let fy = |db: f32| gain_frac(db);
                let mut area = Path::new(at(0.0, flat));
                let mut stroke = Path::new(at(0.0, fy(curve[0])));
                for i in 0..CURVE_POINTS - 1 {
                    let (fx0, fx1) = (
                        i as f32 / (CURVE_POINTS - 1) as f32,
                        (i + 1) as f32 / (CURVE_POINTS - 1) as f32,
                    );
                    let (d0, d1) = (fy(curve[i]), fy(curve[i + 1]));
                    area.push_triangle((at(fx0, d0), at(fx1, d1), at(fx1, flat)), solid);
                    area.push_triangle((at(fx0, d0), at(fx1, flat), at(fx0, flat)), solid);
                    // A stroke with real thickness, since a path has no pen
                    // width: the ribbon between the curve and itself nudged
                    // down by a pixel and a half.
                    let thick = 1.5 / h;
                    stroke.push_triangle((at(fx0, d0), at(fx1, d1), at(fx1, d1 + thick)), solid);
                    stroke.push_triangle(
                        (at(fx0, d0), at(fx1, d1 + thick), at(fx0, d0 + thick)),
                        solid,
                    );
                }
                window.paint_path(
                    area,
                    palette::alpha(accent, if enabled { 0x33 } else { 0x14 }),
                );
                window.paint_path(
                    stroke,
                    palette::alpha(accent, if enabled { 0xff } else { 0x66 }),
                );
                if let Some(this) = dragging {
                    drag_on_paint(this, window);
                }
            },
        )
        .absolute()
        .inset_0();

        let mut face = div()
            .id("eq-plot")
            .relative()
            .flex_1()
            .min_h(px(150.))
            .rounded(tokens::RADIUS)
            .bg(palette::bg_root())
            .child(face);
        // Handles ride above the canvas as real elements rather than painted
        // dots, so each one carries its own grab and the hit test is the
        // layout's job instead of arithmetic.
        for band in 0..BANDS {
            face = face.child(self.handle(band));
        }
        // The press lands on the plot rather than on the dots, so NODE_GRAB's
        // reach is what catches a band: a 14px target is a fiddle on a
        // trackpad, and the handles themselves stay decoration.
        face.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                let Some(band) = this.band_at(event.position) else {
                    return;
                };
                this.selected = band;
                // Double-click puts the band back where it started and takes
                // no grip: the second press would otherwise start a drag from
                // the pointer, dragging the band straight back off the home it
                // was just sent to.
                if event.click_count > 1 {
                    this.grabbed = None;
                    player::reset_eq_band(band, cx);
                } else {
                    this.grab(band, event.position);
                }
                cx.notify();
            }),
        )
        // The wheel widens and narrows whatever is under the pointer, the
        // gesture every EQ with a curve uses for Q.
        .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
            let Some(band) = this.grabbed.or_else(|| this.band_at(event.position)) else {
                return;
            };
            let delta = event.delta.pixel_delta(window.line_height()).y;
            if delta == px(0.) {
                return;
            }
            // Multiplicative, so a turn of the wheel changes the width by
            // the same proportion whether the band is wide or narrow.
            let step = if f32::from(delta) > 0.0 {
                1.12
            } else {
                1.0 / 1.12
            };
            player::set_eq_q(band, player::eq_q(band) * step, cx);
            this.selected = band;
            cx.notify();
        }))
    }

    /// The frequency scale under the plot: every labelled step of the grid,
    /// each number centered on its own line. Its own strip rather than text
    /// tucked inside the plot, since the analyzer fills the bottom of the
    /// plot and a number over the bars is a number you have to hunt for.
    fn axis(&self) -> Div {
        let mut strip = div()
            .relative()
            .flex_none()
            .text_xs()
            .text_color(palette::text_faint());
        for (hz, frac, major) in axis_marks() {
            if !major {
                continue;
            }
            let mark = div().whitespace_nowrap().child(fmt_axis_hz(hz));
            // The ends pin to their edge instead of centering: half of 20 Hz
            // would hang off the left of the plot it's labelling. The left
            // one rides in flow rather than absolute, so the strip stands as
            // tall as its own text at whatever the app font is set to.
            strip = strip.child(if frac <= 0.005 {
                mark
            } else if frac >= 0.995 {
                mark.absolute().top_0().right_0()
            } else {
                mark.absolute()
                    .top_0()
                    .left(relative(frac))
                    .ml(px(-AXIS_LABEL_W / 2.0))
                    .w(px(AXIS_LABEL_W))
                    .flex()
                    .justify_center()
            });
        }
        strip
    }

    /// One band's handle on the plot: a dot at its center and gain, carrying
    /// its own number so a curve with ten bells can still be read. The press
    /// is the plot's job, so this is what a band looks like, nothing more.
    fn handle(&self, band: usize) -> impl IntoElement {
        let gain = player::eq_gain(band);
        let held = self.grabbed == Some(band);
        let selected = self.selected == band;
        // A band doing nothing fades back rather than disappearing: it still
        // has to be findable to be dragged into use.
        let strength = if held || selected {
            0xff
        } else if gain.abs() > 0.05 {
            0xcc
        } else {
            0x66
        };
        let color = palette::alpha(palette::accent(), strength);
        div()
            .absolute()
            .left(relative(freq_frac(player::eq_freq(band))))
            .top(relative(gain_frac(gain)))
            .ml(px(-NODE_R))
            .mt(px(-NODE_R))
            .w(px(NODE_R * 2.0))
            .h(px(NODE_R * 2.0))
            .rounded_full()
            .bg(color)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_on(color))
                    .child(format!("{}", band + 1)),
            )
    }

    /// The numbers for whichever band is selected: type them where a drag is
    /// too coarse. Three rows rather than ten, since the plot is where a
    /// band is picked now.
    fn readouts(&self, cx: &mut Context<Self>) -> Div {
        let band = self.selected.min(BANDS - 1);
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(format!("Band {}", band + 1)),
            )
            .child(self.freq_row(band, cx))
            .child(
                self.readout_row(
                    "Gain",
                    band,
                    player::eq_gain(band),
                    settings_ui::span(-GAIN_MAX_DB, GAIN_MAX_DB, " dB")
                        .decimals(1)
                        .hard(),
                    player::set_eq_gain,
                    1,
                    cx,
                ),
            )
            .child(self.readout_row(
                "Width",
                band,
                player::eq_q(band),
                settings_ui::span(Q_MIN, Q_MAX, " Q").decimals(2).hard(),
                player::set_eq_q,
                2,
                cx,
            ))
    }

    /// The Freq strip, mapped the way the plot's axis is rather than
    /// straight across: on a linear span everything under 500 Hz shares the
    /// first fortieth of the strip, which is no way to place a band.
    fn freq_row(&self, band: usize, cx: &mut Context<Self>) -> Div {
        let hz = player::eq_freq(band);
        labelled(
            "Freq",
            panel::value_slider_edit_sized(
                &self.scrubs[0],
                &self.value_edit,
                freq_frac(hz),
                format!("{hz:.0} Hz"),
                format!("{hz:.0}"),
                1.0,
                panel::SliderWidth::Fill,
                freq_frac,
                move |_: &mut Self, fraction, cx| {
                    player::set_eq_freq(band, frac_freq(fraction), cx);
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// One labelled strip under the plot. `slot` picks which scrub state it
    /// uses, so the three rows keep their drags apart while the band they
    /// point at changes under them.
    #[allow(clippy::too_many_arguments)]
    fn readout_row(
        &self,
        label: &'static str,
        band: usize,
        value: f32,
        span: settings_ui::Span,
        apply: fn(usize, f32, &mut App),
        slot: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        labelled(
            label,
            settings_ui::scalar_sized(
                &self.scrubs[slot],
                &self.value_edit,
                value,
                span,
                panel::SliderWidth::Fill,
                move |_: &mut Self, value, cx| {
                    apply(band, value, cx);
                    cx.notify();
                },
                cx,
            ),
        )
    }
}

/// A readout row's label beside its control, the shape the three rows share.
/// The strip takes the rest of the window: there's no settings-page control
/// column to line up with here, and a short slider under a full-width plot
/// reads as a mistake.
fn labelled(label: &'static str, control: Div) -> Div {
    div()
        .flex()
        .flex_row()
        .w_full()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(
            div()
                .w(px(44.))
                .flex_none()
                .text_xs()
                .text_color(palette::text_muted())
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// Keep a live band drag following the pointer and let go when the button
/// does. Window handlers rather than the plot div's own: a grab pulled past
/// the edge has to keep tracking so the clamp can pin it there, and a
/// release outside the window never reaches an element at all. They live one
/// frame, so the plot's paint re-arms them every frame of the drag - the
/// [`panel::scrub_on_paint`] idiom.
fn drag_on_paint(this: WeakEntity<EqWindow>, window: &mut Window) {
    window.on_mouse_event({
        let this = this.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            if !phase.bubble() {
                return;
            }
            // A release outside the window never reaches the up handler; a
            // move with the button no longer held ends the drag instead.
            if event.pressed_button != Some(MouseButton::Left) {
                let _ = this.update(cx, |this, cx| this.release(cx));
                return;
            }
            let position = event.position;
            let _ = this.update(cx, |this, cx| this.drag_to(position, cx));
        }
    });
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
        if phase.bubble() {
            let _ = this.update(cx, |this, cx| this.release(cx));
        }
    });
}

impl Render for EqWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // With no workspace player to theme to, tint to this window's own id,
        // which the palette map doesn't know, so it reads the base palette.
        let player = self
            .state
            .as_ref()
            .map(|state| state.player.entity_id())
            .unwrap_or_else(|| cx.entity().entity_id());
        palette::note_focus(player, window.is_window_active(), cx);
        // The whole tree builds inside the closure: an element made outside it
        // reads the palette before the tint is in place and paints untinted.
        // The analyzer steps once per frame. While audio moves the player
        // observe re-renders on every pump tick, the only rate new samples
        // arrive at; frame polling is just for the falling bars after
        // playback stops, the same gate every meter panel keeps. Without it
        // the bars rarely read fully settled mid-track, so the window
        // repainted at monitor refresh instead of the pump's clock.
        let playing = self
            .state
            .as_ref()
            .is_some_and(|state| state.player.read(cx).is_playing());
        if self.step_spectrum(cx) && !playing {
            window.request_animation_frame();
        }
        panel::window_body(player, || {
            let plot = self.plot(cx);
            let axis = self.axis();
            let readouts = self.readouts(cx);
            let transport = self.transport(cx);
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .p(tokens::SPACE_MD)
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(div().child("Equalizer"))
                        .child(self.controls(cx)),
                )
                .child(div().text_xs().text_color(palette::text_muted()).child(
                    "Drag a band to move it, scroll over one to widen or narrow it. The \
                         processing sits ahead of the buffer that feeds the sound card, so a \
                         move takes up to half a second to reach the speakers.",
                ))
                // The scale belongs to the plot, so they share a column and
                // the gap between them stays tighter than the window's own.
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap(tokens::SPACE_XS)
                        .child(plot)
                        .child(axis),
                )
                // Straight under the plot: the transport belongs to what's
                // being listened to, and the readouts below are the tail of
                // the curve rather than something a play button sits over.
                .when_some(transport, |d, transport| d.child(transport))
                .child(readouts)
                .into_any_element()
        })
    }
}
