//! The equalizer window: a ten-band parametric curve dragged on a plot over
//! a live analyzer, with presets. A window rather than a settings page
//! because it's worked while the music plays.
//!
//! The curve is one set of process-wide atomics (ADR 19,
//! [`rox_services::player::eq_gain`]), so the EQ spans every workspace and
//! the window holds no player of its own.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    App, Bounds, Context, Div, Entity, Global, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Path, Pixels, Point, ScrollWheelEvent, SharedString, Subscription, WeakEntity,
    Window, WindowHandle, canvas, div, fill, point, prelude::*, px, relative, size,
};
use gpui_component::Root;
use gpui_component::Sizable as _;
use gpui_component::input::{Input, InputEvent, InputState};

use rox_panel_kit::axis::fmt_axis_hz;
use rox_playback::eq::{BANDS, FREQ_MAX, FREQ_MIN, GAIN_MAX_DB, Q_MAX, Q_MIN};
use rox_playback::latency::{self, LatencyHold};
use rox_viz::analysis;

use rox_core::settings::{AnalyzerStyle, LayoutSize, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::sources::autoeq::{self, BandSetting};
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ScrubState;
use rox_panel_kit::ui::{self as settings_ui, icon_button, small_button};
use rox_services::player;

use crate::eq_presets;

const NAME_W: f32 = 140.0;

/// Per-band tolerance for the picker's label: a file written to a hundredth
/// of a dB comes back a hair off what was saved.
const SAME_CURVE: f32 = 0.01;

/// Wider than one band's ceiling: stacked boosts sum past 12 dB.
const PLOT_DB: f32 = 18.0;

/// Enough that a narrow band still draws as a bell, cheap enough per drag frame.
const CURVE_POINTS: usize = 192;

/// The grab reaches past the dot: a 14px target is a fiddle on a trackpad.
const NODE_R: f32 = 7.0;
const NODE_GRAB: f32 = 20.0;

/// The box is centered on the gridline, since an element can't offset by its own width.
const AXIS_LABEL_W: f32 = 40.0;

const SPECTRUM_BARS: usize = 96;

/// Long windows resolve the bottom octaves, where a short FFT smears a whole
/// band into one bin; short ones keep up with the music.
const FFT_CHOICES: [usize; 6] = [512, 1024, 2048, 4096, 8192, 16384];

const SPECTRUM_FLOOR_DB: f32 = -78.0;
const SPECTRUM_CEIL_DB: f32 = -6.0;

/// Full scale per second. Rising is instant; only the decay smooths.
const SPECTRUM_FALL: f32 = 1.6;

const WAVE_SMOOTH: usize = 3;

/// At full height the analyzer reads as the subject instead of context.
const SPECTRUM_HEIGHT: f32 = 0.62;

/// A triangular kernel, so a real peak still stands while bin noise settles.
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
            if weight > 0.0 { sum / weight } else { 0.0 }
        })
        .collect()
}

/// 0 at the left edge, 1 at the right, on a log scale.
fn freq_frac(hz: f32) -> f32 {
    let (lo, hi) = (FREQ_MIN.log10(), FREQ_MAX.log10());
    ((hz.max(FREQ_MIN).log10() - lo) / (hi - lo)).clamp(0.0, 1.0)
}

fn frac_freq(frac: f32) -> f32 {
    let (lo, hi) = (FREQ_MIN.log10(), FREQ_MAX.log10());
    10f32.powf(lo + frac.clamp(0.0, 1.0) * (hi - lo))
}

/// (hz, position, labelled): the same ladder the spectrum panel rules.
fn axis_marks() -> Vec<(f32, f32, bool)> {
    analysis::hz_ladder(FREQ_MIN, FREQ_MAX)
}

fn fmt_fft(size: usize) -> String {
    if size >= 1024 {
        format!("{}k FFT", size / 1024)
    } else {
        format!("{size} FFT")
    }
}

/// So a hand-edited settings file can't hand the analyzer a size it panics on.
fn fft_size(size: usize) -> usize {
    size.next_power_of_two()
        .clamp(analysis::MIN_FFT_SIZE, analysis::MAX_FFT_SIZE)
}

/// 0 at the top, 1 at the bottom.
fn gain_frac(db: f32) -> f32 {
    (0.5 - db / (2.0 * PLOT_DB)).clamp(0.0, 1.0)
}

fn frac_gain(frac: f32) -> f32 {
    ((0.5 - frac) * 2.0 * PLOT_DB).clamp(-GAIN_MAX_DB, GAIN_MAX_DB)
}

struct OpenEq(WindowHandle<Root>);

impl Global for OpenEq {}

/// Deferred: the menu action runs inside the workspace's update, and reading
/// the front workspace for the tint mid-update would panic.
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
    let state = rox_panel_api::windows::front_workspace(cx).map(|(_, state)| state);
    let min = settings_ui::MIN_SIZE;
    let (width, height) = Settings::load()
        .windows
        .eq
        .filter(|s| s.width >= f32::from(min.width) && s.height >= f32::from(min.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((620., 660.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("eq-window-title"),
        bounds,
        Some(min),
        move |window, cx| cx.new(|cx| EqWindow::new(state, window, cx)),
    );
    cx.set_global(OpenEq(handle));
}

struct EqWindow {
    /// For the tint and the transport strip; None leaves the strip out.
    state: Option<AppState>,
    scrubs: [ScrubState; BANDS],
    value_edit: panel::ValueEdit,
    /// From the layout: the drag maps pointer positions through it.
    plot: Arc<Mutex<Option<Bounds<Pixels>>>>,
    /// A drag that leaves the plot keeps its grip, so a band pulls like a fader.
    grabbed: Option<usize>,
    /// Kept through the drag so a band grabbed by its rim doesn't snap under the pointer.
    grab_offset: Point<Pixels>,
    /// Persists past the drag so the readouts stay put to type into.
    selected: usize,
    bars: Vec<f32>,
    bins: Vec<(usize, usize)>,
    /// A device switch remaps rather than drawing the old layout against new audio.
    bin_rate: u32,
    fft: usize,
    /// Tells new audio from a repaint between pump ticks.
    last_written: u64,
    last_tick: Instant,
    analyzer_style: AnalyzerStyle,
    preset_name: Entity<InputState>,
    /// Read on open and after writes, never per frame: this repaints at the pump's clock.
    presets: Vec<String>,
    /// Last frame's focus, to catch the moment the window comes forward.
    active: bool,
    /// The preset's name and the curve it applied. The picker names it only while
    /// the live curve still matches.
    picked: Option<(String, Vec<BandSetting>)>,
    _player_changed: Option<Subscription>,
    _eq_changed: Subscription,
    _name_changed: Subscription,
    _presets_changed: Subscription,
    /// Keeps the sample ring shallow while open (ADR 19), so a band follows the
    /// drag at about a tenth of a second instead of half.
    _latency: LatencyHold,
}

impl EqWindow {
    fn new(state: Option<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The OS close button never runs remove_window, so the size persists here.
        window.on_window_should_close(cx, move |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                let size = s.windows.eq.get_or_insert_with(LayoutSize::default);
                size.width = frame.size.width.into();
                size.height = frame.size.height.into();
            });
            true
        });
        // Or the play/pause face goes stale when a track ends on its own.
        let _player_changed = state
            .as_ref()
            .map(|state| cx.observe(&state.player, |_, _, cx| cx.notify()));
        let preset_name =
            cx.new(|cx| InputState::new(window, cx).placeholder(rox_i18n::t!("eq-preset-name")));
        let _name_changed = cx.subscribe_in(
            &preset_name,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.save_preset(cx),
                _ => {}
            },
        );
        let eq = Settings::load().eq;
        let fft = fft_size(eq.fft_size);
        EqWindow {
            state,
            preset_name,
            presets: eq_presets::list(),
            active: false,
            picked: None,
            scrubs: std::array::from_fn(|_| ScrubState::default()),
            value_edit: panel::ValueEdit::default(),
            plot: Arc::new(Mutex::new(None)),
            grabbed: None,
            grab_offset: point(px(0.), px(0.)),
            selected: 0,
            bars: vec![0.0; SPECTRUM_BARS],
            bins: Vec::new(),
            bin_rate: 0,
            fft,
            last_written: 0,
            last_tick: Instant::now(),
            analyzer_style: eq.analyzer,
            _player_changed,
            _eq_changed: player::observe_eq(cx),
            _name_changed,
            _presets_changed: eq_presets::observe(cx, |this, cx| {
                this.presets = eq_presets::list();
                cx.notify();
            }),
            _latency: latency::hold(),
        }
    }

    fn set_fft(&mut self, size: usize, cx: &mut Context<Self>) {
        let size = fft_size(size);
        if size == self.fft {
            return;
        }
        self.fft = size;
        self.bins.clear();
        Settings::update(move |s| s.eq.fft_size = size);
        cx.notify();
    }

    /// Returns whether anything still moves, which decides the next frame.
    /// Pausing mid-track freezes the bars, the spectrum panel's behavior; a queue
    /// that played out decays instead.
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
            self.bins = analysis::log_bands(SPECTRUM_BARS, FREQ_MIN, FREQ_MAX, rate, self.fft / 2);
            self.bin_rate = rate;
        }
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();

        let written = feed.written();
        let fresh = written != self.last_written;
        self.last_written = written;
        if hold && !fresh {
            return false;
        }
        // Only analyze new audio; between ticks the decay keeps the bars alive.
        if let Some(mags) = fresh.then(|| feed.magnitudes(self.fft)).flatten() {
            for (bar, &(lo, hi)) in self.bins.iter().enumerate() {
                let peak = mags[lo..hi].iter().copied().fold(0.0f32, f32::max);
                let db = 20.0 * (peak + 1e-9).log10();
                let level = ((db - SPECTRUM_FLOOR_DB) / (SPECTRUM_CEIL_DB - SPECTRUM_FLOOR_DB))
                    .clamp(0.0, 1.0);
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

    /// Clamps outside the plot, so a drag pins to the edge instead of stalling.
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

    fn node_center(&self, band: usize) -> Option<Point<Pixels>> {
        let bounds = (*self.plot.lock().unwrap())?;
        Some(point(
            bounds.origin.x + bounds.size.width * freq_frac(player::eq_freq(band)),
            bounds.origin.y + bounds.size.height * gain_frac(player::eq_gain(band)),
        ))
    }

    /// Measured in plot pixels, so grabs feel the same where the log axis is dense.
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

    fn grab(&mut self, band: usize, at: Point<Pixels>) {
        self.grabbed = Some(band);
        self.grab_offset = self
            .node_center(band)
            .map(|center| at - center)
            .unwrap_or_else(|| point(px(0.), px(0.)));
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        if self.grabbed.take().is_some() {
            cx.notify();
        }
    }

    /// Center and gain at once. The grab offset comes off first so the band doesn't jump.
    fn drag_to(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(band) = self.grabbed else { return };
        let Some((x, y)) = self.plot_frac(at - self.grab_offset) else {
            return;
        };
        player::set_eq_freq(band, frac_freq(x), cx);
        player::set_eq_gain(band, frac_gain(y), cx);
        cx.notify();
    }

    fn transport(&self, cx: &mut Context<Self>) -> Option<Div> {
        let state = self.state.as_ref()?;
        let strip = panel::transport_strip(&state.player.clone(), &state.library.clone(), cx);
        Some(div().flex().flex_row().justify_center().child(strip))
    }

    /// Compared band for band: nothing else in the window knows the picker exists.
    fn picked_name(&self) -> Option<SharedString> {
        let (name, applied) = self.picked.as_ref()?;
        let live = eq_presets::live_bands();
        let same = applied.len() == live.len()
            && applied.iter().zip(&live).all(|(a, b)| {
                (a.hz - b.hz).abs() < SAME_CURVE
                    && (a.gain_db - b.gain_db).abs() < SAME_CURVE
                    && (a.q - b.q).abs() < SAME_CURVE
            });
        same.then(|| SharedString::from(name.clone()))
    }

    /// Typed, else the preset the curve came from.
    fn name(&self, cx: &App) -> String {
        let typed = self.preset_name.read(cx).value().trim().to_string();
        if typed.is_empty() {
            self.picked_name()
                .map(|n| n.to_string())
                .unwrap_or_default()
        } else {
            typed
        }
    }

    fn save_preset(&mut self, cx: &mut Context<Self>) {
        let name = self.name(cx);
        if name.is_empty() {
            return;
        }
        let bands = eq_presets::live_bands();
        let Some(saved) = eq_presets::save(&name, &bands, None, cx) else {
            return;
        };

        self.presets = eq_presets::list();
        // Read back so the label survives whatever the engine clamped.
        self.picked = Some((saved, eq_presets::live_bands()));
        cx.notify();
    }

    /// Turns the EQ on too: a preset picked and not heard looks broken.
    fn apply_preset(&mut self, name: String, cx: &mut Context<Self>) {
        // The picker's leading group row names no preset.
        if name.is_empty() {
            return;
        }
        let Some(bands) = eq_presets::load(&name) else {
            self.presets = eq_presets::list();
            cx.notify();
            return;
        };

        let curve: Vec<(f32, f32, f32)> = bands
            .iter()
            .map(|band| (band.hz, band.gain_db, band.q))
            .collect();
        player::apply_eq_bands(&curve, cx);
        player::set_eq_enabled(true, cx);
        self.picked = Some((name, eq_presets::live_bands()));
        cx.notify();
    }

    fn delete_preset(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.picked_name() else {
            return;
        };
        eq_presets::remove(&name, cx);
        self.presets = eq_presets::list();
        self.picked = None;
        cx.notify();
    }

    /// The same text a preset is, readable by Equalizer APO and the rest.
    fn export_preset(&mut self, cx: &mut Context<Self>) {
        let name = {
            let typed = self.name(cx);
            if typed.is_empty() {
                rox_i18n::t!("eq-preset-export-default").to_string()
            } else {
                typed
            }
        };
        let text = autoeq::format_bands(&name, &eq_presets::live_bands(), None);

        let home = dirs::home_dir().unwrap_or_default();
        let file = format!("{name}.txt");
        let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            if let Err(e) = std::fs::write(&path, text) {
                log::warn!("eq presets: exporting to {}: {e}", path.display());
            }
        })
        .detach();
    }

    fn presets(&self, cx: &mut Context<Self>) -> Div {
        let picked = self.picked_name();
        // A leading group row, so the menu never ticks a preset that isn't applied.
        let mut options = vec![(String::new(), rox_i18n::t!("eq-presets"))];
        options.extend(
            self.presets
                .iter()
                .map(|name| (name.clone(), SharedString::from(name.clone()))),
        );
        let current = picked.clone().map(|n| n.to_string()).unwrap_or_default();
        let named = picked.is_some();
        let unnamed = self.name(cx).is_empty();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(panel::picker(
                "eq-preset",
                current,
                options,
                self.presets.is_empty(),
                |this: &mut Self, name, cx| this.apply_preset(name, cx),
                cx,
            ))
            .child(icon_button(
                icons::TRASH,
                !named,
                cx.listener(|this, _, _, cx| this.delete_preset(cx)),
            ))
            .child(div().flex_1())
            .child(Input::new(&self.preset_name).small().w(px(NAME_W)))
            .child(small_button(
                rox_i18n::t!("eq-preset-save"),
                icons::DOWNLOAD,
                unnamed,
                cx.listener(|this, _, _, cx| this.save_preset(cx)),
            ))
            .child(small_button(
                rox_i18n::t!("eq-preset-export"),
                icons::UPLOAD,
                false,
                cx.listener(|this, _, _, cx| this.export_preset(cx)),
            ))
    }

    fn controls(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(small_button(
                rox_i18n::t!("eq-flatten"),
                icons::MINUS,
                false,
                cx.listener(|_, _, _, cx| player::flatten_eq(cx)),
            ))
            .child(small_button(
                rox_i18n::t!("eq-reset-bands"),
                icons::REFRESH_CW,
                false,
                cx.listener(|_, _, _, cx| player::reset_eq_shape(cx)),
            ))
            .child(small_button(
                rox_i18n::t!("eq-autoeq"),
                icons::HEADPHONES,
                false,
                cx.listener(|_, _, _, cx| crate::autoeq_window::open(cx)),
            ))
            .child(panel::picker(
                "eq-analyzer",
                self.analyzer_style,
                vec![
                    (AnalyzerStyle::Wave, rox_i18n::t!("eq-analyzer-wave")),
                    (AnalyzerStyle::Bars, rox_i18n::t!("eq-analyzer-bars")),
                    (AnalyzerStyle::Off, rox_i18n::t!("eq-analyzer-off")),
                ],
                false,
                |this: &mut Self, style, cx| {
                    this.analyzer_style = style;
                    Settings::update(move |s| s.eq.analyzer = style);
                    cx.notify();
                },
                cx,
            ))
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

    /// The response comes from the same coefficients the node runs.
    fn plot(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let player = self.state.as_ref().map(|state| state.player.clone());
        // Sampled here: paint has no cx to read the entity with.
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
        let dragging = self.grabbed.map(|_| cx.entity().downgrade());
        let plot = self.plot.clone();
        let bars = self.bars.clone();
        let analyzer = self.analyzer_style;
        let spectrum = palette::alpha(palette::text_muted(), 0x2a);
        let spectrum_edge = palette::alpha(palette::text_muted(), 0x66);
        let marks = axis_marks();
        let grid = palette::alpha(palette::text_muted(), 0x22);
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
                // The analyzer first, rising off the floor: it's level, not gain.
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

                // Triangles because that's what gpui's path takes.
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
                    // A path has no pen width, so the stroke is a 1.5px ribbon.
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
        for band in 0..BANDS {
            face = face.child(self.handle(band));
        }
        // The press lands on the plot so NODE_GRAB's reach catches a band.
        face.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                let Some(band) = this.band_at(event.position) else {
                    return;
                };
                this.selected = band;
                // Double-click resets the band and takes no grip, or the second press would
                // drag it straight back off home.
                if event.click_count > 1 {
                    this.grabbed = None;
                    player::reset_eq_band(band, cx);
                } else {
                    this.grab(band, event.position);
                }
                cx.notify();
            }),
        )
        .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
            let Some(band) = this.grabbed.or_else(|| this.band_at(event.position)) else {
                return;
            };
            let delta = event.delta.pixel_delta(window.line_height()).y;
            if delta == px(0.) {
                return;
            }
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

    /// Its own strip below the plot: labels over the analyzer bars are hard to find.
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
            // The ends pin to their edge instead of centering. The left one stays in
            // flow, so the strip is as tall as its text at any app font.
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

    /// Decoration only: the press is the plot's job.
    fn handle(&self, band: usize) -> impl IntoElement + use<> {
        let gain = player::eq_gain(band);
        let held = self.grabbed == Some(band);
        let selected = self.selected == band;
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
                    .child(rox_i18n::t!("eq-band-label", number = (band + 1) as u64).to_string()),
            )
            .child(self.freq_row(band, cx))
            .child(
                self.readout_row(
                    rox_i18n::t!("eq-gain-label"),
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
                rox_i18n::t!("eq-width-label"),
                band,
                player::eq_q(band),
                settings_ui::span(Q_MIN, Q_MAX, " Q").decimals(2).hard(),
                player::set_eq_q,
                2,
                cx,
            ))
    }

    /// Log-mapped like the axis: linear would put everything under 500 Hz in the first fortieth.
    fn freq_row(&self, band: usize, cx: &mut Context<Self>) -> Div {
        let hz = player::eq_freq(band);
        labelled(
            rox_i18n::t!("eq-freq-label"),
            panel::value_slider_edit_sized(
                &self.scrubs[0],
                &self.value_edit,
                freq_frac(hz),
                rox_i18n::format::format_unit(hz as f64, 0, "Hz"),
                format!("{hz:.0}"),
                1.0,
                panel::SliderWidth::Fill,
                panel::SLIDER_STEP,
                freq_frac,
                move |_: &mut Self, fraction, cx| {
                    player::set_eq_freq(band, frac_freq(fraction), cx);
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// `slot` picks the scrub state, so the rows keep their drags apart.
    #[allow(clippy::too_many_arguments)]
    fn readout_row(
        &self,
        label: impl Into<SharedString>,
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

fn labelled(label: impl Into<SharedString>, control: Div) -> Div {
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
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// Window handlers, so a drag past the edge keeps tracking and a release
/// outside the window lands. They last one frame, so the plot's paint re-arms
/// them: the [`panel::scrub_on_paint`] idiom.
fn drag_on_paint(this: WeakEntity<EqWindow>, window: &mut Window) {
    window.on_mouse_event({
        let this = this.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            if !phase.bubble() {
                return;
            }
            // A buttonless move ends the drag: a release outside the window never arrives.
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
        let player = self
            .state
            .as_ref()
            .map(|state| state.player.entity_id())
            .unwrap_or_else(|| cx.entity().entity_id());
        let active = window.is_window_active();
        palette::note_focus(player, active, cx);
        // Re-read the folder on coming forward, so an AutoEq save shows up without polling.
        if active && !self.active {
            self.presets = eq_presets::list();
        }
        self.active = active;
        // Frame polling only for the falling bars after playback stops; while audio
        // moves, the player observe already re-renders per pump tick.
        let playing = self
            .state
            .as_ref()
            .is_some_and(|state| state.player.read(cx).is_playing());
        if self.step_spectrum(cx) && !playing {
            window.request_animation_frame();
        }
        panel::window_body(player, || {
            let presets = self.presets(cx);
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
                        .child(div().child(rox_i18n::t!("eq-heading")))
                        .child(self.controls(cx)),
                )
                .child(presets)
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("eq-help-text")),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap(tokens::SPACE_XS)
                        .child(plot)
                        .child(axis),
                )
                .when_some(transport, |d, transport| d.child(transport))
                .child(readouts)
                .into_any_element()
        })
    }
}
