//! The spectrum panel: live frequency bars over the player's PCM tap, the
//! classic analyzer look - log-spaced bands, snappy attack, eased decay,
//! peak-hold caps falling under gravity, dB gridlines behind. Everything is
//! paint primitives on the UI thread: one FFT per frame while audio flows,
//! and once the bars have settled the panel stops asking for frames, so an
//! idle app pays nothing.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, fill, linear_color_stop, linear_gradient, point, prelude::*, px, rgb, rgba, size,
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, SharedString, Subscription,
    WeakEntity, Window,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent, TabPanel};

use rox_viz::analysis::{log_bands, Analyzer, FFT_SIZE};
use rox_viz::AudioFeed;

use crate::panel::{self, AppState, StatePanel};

/// Bars never get thinner than this; the count collapses on narrow panels
/// instead, so a small dock split doesn't smear.
const MIN_BAR_PX: f32 = 3.0;
const BAR_GAP: f32 = 2.0;
const MIN_BARS: usize = 16;
const MAX_BARS: usize = 192;

/// Frequency range the bars span, log-spaced the way we hear pitch. Capping
/// below Nyquist drops the near-silent top octaves that would otherwise sit
/// motionless on the right.
const FREQ_LO: f32 = 30.0;
const FREQ_HI: f32 = 16_000.0;

/// dB window the bars normalize into, on magnitudes where a full-scale sine
/// sits at 0 dB. The top leaves headroom so a busy mix pins near full height
/// without every band clipping there.
const FLOOR_DB: f32 = -66.0;
const MAX_DB: f32 = -12.0;

/// Per-second smoothing rates: bands jump up fast and fall slowly, which is
/// what makes kicks read as kicks instead of flicker.
const ATTACK: f32 = 40.0;
const RELEASE: f32 = 10.0;

/// Peak-hold caps accelerate downward at this rate, in bar heights per
/// second squared: a transient leaves a marker that drifts back down.
const HOLD_GRAVITY: f32 = 0.05;

/// dB gridlines drawn behind the bars.
const DB_MARKS: [f32; 3] = [-20.0, -40.0, -60.0];

/// Everything below this reads as settled; the panel stops animating.
const EPSILON: f32 = 0.002;

/// Per-panel analyzer state, shared with the paint closure the way the old
/// sim shared its frames: the entity holds the handle, the closure does the
/// per-frame work where the bounds are known.
struct Bars {
    analyzer: Analyzer,
    mono: [f32; FFT_SIZE],
    last_written: u64,
    last_tick: Option<Instant>,
    sample_rate: u32,
    /// Half-spectrum bin range per bar, remapped when the width-driven bar
    /// count or the device rate changes.
    bands: Vec<(usize, usize)>,
    levels: Vec<f32>,
    holds: Vec<f32>,
    hold_vel: Vec<f32>,
    /// Bars still moving: render keeps requesting frames until this clears.
    alive: bool,
}

impl Bars {
    fn new() -> Self {
        Bars {
            analyzer: Analyzer::new(),
            mono: [0.0; FFT_SIZE],
            last_written: 0,
            last_tick: None,
            sample_rate: 0,
            bands: Vec::new(),
            levels: Vec::new(),
            holds: Vec::new(),
            hold_vel: Vec::new(),
            alive: false,
        }
    }

    /// One tick: pull the newest window off the feed, fold it into the bar
    /// levels, advance the holds. No new audio means the bars decay.
    fn step(&mut self, feed: &AudioFeed, width: f32) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32().min(0.1))
            .unwrap_or(1.0 / 60.0);
        self.last_tick = Some(now);

        let count = ((width / (MIN_BAR_PX + BAR_GAP)) as usize).clamp(MIN_BARS, MAX_BARS);
        let rate = feed.sample_rate();
        if count != self.levels.len() || rate != self.sample_rate {
            self.sample_rate = rate;
            self.bands = log_bands(count, FREQ_LO, FREQ_HI, rate);
            self.levels = vec![0.0; count];
            self.holds = vec![0.0; count];
            self.hold_vel = vec![0.0; count];
        }

        // New audio since last tick: analyze the latest window. Nothing new
        // (paused, stopped, no session): let the bars fall to silence.
        let written = feed.written();
        let fresh = written != self.last_written && feed.latest_mono(&mut self.mono) == FFT_SIZE;
        self.last_written = written;
        let mags = fresh.then(|| self.analyzer.magnitudes(&self.mono));

        let mut alive = false;
        for i in 0..self.levels.len() {
            let target = match mags {
                Some(mags) => {
                    let (lo, hi) = self.bands[i];
                    let mut peak = 0.0f32;
                    for &m in &mags[lo..hi] {
                        peak = peak.max(m);
                    }
                    let db = 20.0 * (peak + 1e-9).log10();
                    ((db - FLOOR_DB) / (MAX_DB - FLOOR_DB)).clamp(0.0, 1.0)
                }
                None => 0.0,
            };
            let rate = if target > self.levels[i] { ATTACK } else { RELEASE };
            self.levels[i] += (target - self.levels[i]) * (rate * dt).min(1.0);

            // The cap rides up with the bar and falls back under gravity
            // once the bar drops away.
            if self.levels[i] >= self.holds[i] {
                self.holds[i] = self.levels[i];
                self.hold_vel[i] = 0.0;
            } else {
                self.hold_vel[i] += HOLD_GRAVITY * dt;
                self.holds[i] = (self.holds[i] - self.hold_vel[i] * dt).max(self.levels[i]);
            }
            if self.levels[i] > EPSILON || self.holds[i] > EPSILON {
                alive = true;
            }
        }
        self.alive = alive;
    }

    fn paint(&self, bounds: Bounds<gpui::Pixels>, window: &mut Window) {
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        let count = self.levels.len();
        if count == 0 || w <= 0.0 || h <= 0.0 {
            return;
        }

        let max_h = h * 0.94;
        let step = w / count as f32;
        let bar_w = (step - BAR_GAP).max(1.0);

        // dB gridlines behind the bars.
        for db in DB_MARKS {
            let y = h - (db - FLOOR_DB) / (MAX_DB - FLOOR_DB) * max_h;
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x, bounds.origin.y + px(y)),
                    size(px(w), px(1.0)),
                ),
                rgba(0x6e6e6e28),
            ));
        }

        for i in 0..count {
            let bar_h = (self.levels[i] * max_h).max(2.0);
            let x = bounds.origin.x + px(i as f32 * step);
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(x, bounds.origin.y + px(h - bar_h)),
                        size(px(bar_w), px(bar_h)),
                    ),
                    // Angle 0 points the gradient line at the top: solid
                    // accent at the baseline fading out toward the bar tip.
                    linear_gradient(
                        0.0,
                        linear_color_stop(rgba(0x3dff9cff), 0.0),
                        linear_color_stop(rgba(0x3dff9c40), 1.0),
                    ),
                ),
            );

            // Solid peak-hold cap sitting at the held level above the bar.
            let cap_y = (h - self.holds[i] * max_h - 1.0).max(0.0);
            window.paint_quad(fill(
                Bounds::new(
                    point(x, bounds.origin.y + px(cap_y)),
                    size(px(bar_w), px(1.0)),
                ),
                rgba(0x3dff9cff),
            ));
        }
    }
}

pub struct SpectrumPanel {
    state: AppState,
    feed: Arc<AudioFeed>,
    bars: Arc<Mutex<Bars>>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel when a session starts, so an idle window resumes
    /// animating without the player bar's frame pump.
    _player_changed: Subscription,
}

impl SpectrumPanel {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        SpectrumPanel {
            feed: state.player.read(cx).feed(),
            state,
            bars: Arc::new(Mutex::new(Bars::new())),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }
}

impl StatePanel for SpectrumPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn tab_panel(&self) -> Option<WeakEntity<TabPanel>> {
        self.tab_panel.clone()
    }

    fn duplicate(state: AppState, cx: &mut Context<Self>) -> Self {
        SpectrumPanel::new(state, cx)
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
        SharedString::from("spectrum")
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel);
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![
            panel::duplicate_button(&cx.entity()),
            panel::popout_button(&cx.entity(), "spectrum", self.tab_panel.clone()),
        ])
    }
}

impl Render for SpectrumPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep frames coming while a session runs (the tap only moves while
        // the player pumps it, but the position and pause state do not
        // notify) and while the bars are still falling. Otherwise: parked.
        let session = self.state.player.read(cx).now_playing().is_some();
        if session || self.bars.lock().unwrap().alive {
            window.request_animation_frame();
        }

        let bars = self.bars.clone();
        let feed = self.feed.clone();
        div().size_full().bg(rgb(0x121212)).child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    let mut bars = bars.lock().unwrap();
                    bars.step(&feed, f32::from(bounds.size.width));
                    bars.paint(bounds, window);
                },
            )
            .size_full(),
        )
    }
}
