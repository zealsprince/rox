//! The spectrum panel: live frequency bars over the player's PCM tap, the
//! classic analyzer look - log-spaced bands, snappy attack, eased decay,
//! peak-hold caps falling under gravity, dB gridlines behind. Everything is
//! paint primitives on the UI thread: one FFT per frame while audio flows,
//! and once the bars have settled the panel stops asking for frames, so an
//! idle app pays nothing. The analyzed range, the bar coloring, and the
//! octave pitch markers are per-view config the customize window edits and
//! the layout dump carries.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, fill, linear_color_stop, linear_gradient, point, prelude::*, px, relative, size,
    AnyElement, App, Bounds, Context, Div, EventEmitter, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, Rgba, SharedString, Size, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::analysis::{log_bands, Analyzer, FFT_SIZE};
use rox_viz::AudioFeed;

use crate::design::{palette, tokens};
use crate::panel::{self, setting_row, toggle, AppState, Customizable, ScrubState};

// Bars follow the shared visualizer rhythm (`tokens::BAR_W`, `BAR_GAP`);
// the count collapses on narrow panels instead of thinning the bars, so a
// small dock split doesn't smear.
const MIN_BARS: usize = 16;
const MAX_BARS: usize = 192;

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

/// Peak-hold caps accelerate downward at this rate, in bar heights per
/// second squared: a transient leaves a marker that drifts back down.
const HOLD_GRAVITY: f32 = 0.05;

/// dB gridlines drawn behind the bars.
const DB_MARKS: [f32; 3] = [-20.0, -40.0, -60.0];

/// Everything below this reads as settled; the panel stops animating.
const EPSILON: f32 = 0.002;

/// The bounds sliders' strip width and the Hz readout beside them.
const SLIDER_W: gpui::Pixels = px(150.);
const READOUT_W: gpui::Pixels = px(60.);

/// The spectrum panel's per-view config: what a saved layout restores, and
/// what the customize window edits. Missing fields take the defaults, so a
/// layout dumped before this config existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrumConfig {
    /// Low bound of the analyzed range, Hz: the bars span log-spaced from
    /// here up to `freq_hi`.
    pub freq_lo: f32,
    /// High bound of the analyzed range, Hz. Capping below Nyquist drops the
    /// near-silent top octaves that would sit motionless on the right.
    pub freq_hi: f32,
    /// Color bars by loudness - a ramp from a dim floor up to the accent -
    /// instead of a flat accent fill, so only the peaks light up.
    pub gradient: bool,
    /// Draw octave pitch markers (C1, C2, ...) across the analyzed range.
    pub labels: bool,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        SpectrumConfig {
            freq_lo: 30.0,
            freq_hi: 16_000.0,
            gradient: false,
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

/// Per-panel analyzer state, shared with the paint closure the way the old
/// sim shared its frames: the entity holds the handle, the closure does the
/// per-frame work where the bounds are known.
struct Bars {
    analyzer: Analyzer,
    mono: [f32; FFT_SIZE],
    last_written: u64,
    last_tick: Option<Instant>,
    sample_rate: u32,
    /// The range the current band mapping was built for, so a bounds change
    /// remaps just like a bar-count or device-rate change does.
    freq_lo: f32,
    freq_hi: f32,
    /// Half-spectrum bin range per bar, remapped when the width-driven bar
    /// count, the device rate, or the range changes.
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
            freq_lo: 0.0,
            freq_hi: 0.0,
            bands: Vec::new(),
            levels: Vec::new(),
            holds: Vec::new(),
            hold_vel: Vec::new(),
            alive: false,
        }
    }

    /// One tick: pull the newest window off the feed, fold it into the bar
    /// levels, advance the holds. No new audio means the bars decay.
    fn step(&mut self, feed: &AudioFeed, width: f32, freq_lo: f32, freq_hi: f32) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32().min(0.1))
            .unwrap_or(1.0 / 60.0);
        self.last_tick = Some(now);

        let count =
            ((width / (tokens::BAR_W + tokens::BAR_GAP)) as usize).clamp(MIN_BARS, MAX_BARS);
        let rate = feed.sample_rate();
        if count != self.levels.len()
            || rate != self.sample_rate
            || freq_lo != self.freq_lo
            || freq_hi != self.freq_hi
        {
            self.sample_rate = rate;
            self.freq_lo = freq_lo;
            self.freq_hi = freq_hi;
            self.bands = log_bands(count, freq_lo, freq_hi, rate);
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
            let rate = if target > self.levels[i] {
                ATTACK
            } else {
                RELEASE
            };
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

    fn paint(&self, bounds: Bounds<gpui::Pixels>, window: &mut Window, gradient: bool) {
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        let count = self.levels.len();
        if count == 0 || w <= 0.0 || h <= 0.0 {
            return;
        }

        let max_h = h * 0.94;
        let step = w / count as f32;
        let bar_w = (step - tokens::BAR_GAP).max(1.0);

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
            let base = bar_color(self.levels[i], gradient);
            window.paint_quad(fill(
                Bounds::new(
                    point(x, bounds.origin.y + px(h - bar_h)),
                    size(px(bar_w), px(bar_h)),
                ),
                // Angle 0 points the gradient line at the top: solid base at
                // the baseline fading out toward the bar tip.
                linear_gradient(
                    0.0,
                    linear_color_stop(base, 0.0),
                    linear_color_stop(palette::alpha(base, 0x40), 1.0),
                ),
            ));

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

pub struct SpectrumPanel {
    state: AppState,
    config: SpectrumConfig,
    feed: Arc<AudioFeed>,
    bars: Arc<Mutex<Bars>>,
    /// The bounds sliders' painted bounds and drag state, one per slider so a
    /// drag on one never moves the other.
    lo_scrub: ScrubState,
    hi_scrub: ScrubState,
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

    /// One log-frequency bounds slider: the shared slider chrome over a scrub
    /// strip, applying live on click and drag, with the Hz readout alongside.
    /// The same shape as the settings window's scalar sliders.
    fn freq_slider(
        &self,
        scrub: &ScrubState,
        hz: f32,
        apply: fn(&mut Self, f32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        let entity = cx.entity();
        let fraction = hz_to_frac(hz);
        let strip = div()
            .w(SLIDER_W)
            .h(tokens::CONTROL_H)
            .flex_none()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let scrub = scrub.clone();
                    move |this: &mut Self, event: &MouseDownEvent, _, cx| {
                        scrub.begin();
                        if let Some(fraction) = scrub.fraction(event.position.x) {
                            apply(this, fraction, cx);
                        }
                        cx.notify();
                    }
                }),
            )
            .child(
                canvas(
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, _| scrub.set_bounds(bounds)
                    },
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, window, _| {
                            panel::paint_slider(fraction, false, bounds, window);
                            panel::scrub_on_paint(&scrub, window, {
                                let entity = entity.clone();
                                move |fraction, cx| {
                                    entity.update(cx, |this, cx| apply(this, fraction, cx));
                                }
                            });
                        }
                    },
                )
                .size_full(),
            );
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(strip)
            .child(
                div()
                    .w(READOUT_W)
                    .flex_none()
                    .text_right()
                    .text_color(palette::text_muted())
                    .child(fmt_hz(hz)),
            )
    }

    /// The panel's own dropdown entries: the display toggles the customize
    /// window also holds, for a quick flip without opening it.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let mut menu = menu;
        for (label, on, set) in [
            (
                "Intensity Color",
                self.config.gradient,
                (|this: &mut Self| this.config.gradient = !this.config.gradient) as fn(&mut Self),
            ),
            (
                "Pitch Labels",
                self.config.labels,
                (|this: &mut Self| this.config.labels = !this.config.labels) as fn(&mut Self),
            ),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(label)
                    .checked(on)
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            set(this);
                            cx.notify();
                        });
                    }),
            );
        }
        menu
    }
}

impl Customizable for SpectrumPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn customize(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                "low bound",
                Some("lowest frequency the bars analyze"),
                self.freq_slider(&self.lo_scrub, self.config.freq_lo, Self::set_freq_lo, cx),
            ))
            .child(setting_row(
                "high bound",
                Some("highest frequency the bars analyze"),
                self.freq_slider(&self.hi_scrub, self.config.freq_hi, Self::set_freq_hi, cx),
            ))
            .child(setting_row(
                "intensity color",
                Some("color bars by loudness so only the peaks light up, instead of a flat fill"),
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
                "pitch labels",
                Some("mark the octaves (C1, C2, ...) across the range"),
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

    fn customize_size(&self) -> Size<gpui::Pixels> {
        size(px(400.), px(300.))
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

    /// The layout dump carries the panel's config; the builder registered in
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The config block: the panel's quick toggles and the customize
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, cx);
        let menu = panel::customize_item(menu, &cx.entity());
        let menu = menu.separator();
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the config along, like the cover panel's.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate Panel").on_click(move |_, window, cx| {
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
        // Keep frames coming while a session runs (the tap only moves while
        // the player pumps it, but the position and pause state do not
        // notify) and while the bars are still falling. Otherwise: parked.
        let session = self.state.player.read(cx).now_playing().is_some();
        if session || self.bars.lock().unwrap().alive {
            window.request_animation_frame();
        }

        let (freq_lo, freq_hi) = self.config.range();
        let gradient = self.config.gradient;
        let bars = self.bars.clone();
        let feed = self.feed.clone();
        let mut root = div().size_full().relative().bg(palette::bg_root()).child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    let mut bars = bars.lock().unwrap();
                    bars.step(&feed, f32::from(bounds.size.width), freq_lo, freq_hi);
                    bars.paint(bounds, window, gradient);
                },
            )
            .size_full(),
        );
        if self.config.labels {
            root = root.child(labels_overlay(freq_lo, freq_hi));
        }
        root
    }
}
