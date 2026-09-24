//! The oscilloscope panel: the player's PCM tap drawn in time. The trigger
//! makes it readable: the panel pulls more audio than it shows and starts
//! the frame at the first crossing of the trigger level, so periodic
//! material stands still. Each column holds a min/max pair so a transient
//! can't fall between pixels. The panel parks once the audio stops and the
//! phosphor trail burns off.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, Path,
    Pixels, Point, Rgba, SharedString, Subscription, WeakEntity, Window, canvas, div, fill, point,
    prelude::*, px, size,
};
use gpui_component::Sizable as _;
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::AudioFeed;
use rox_viz::analysis::MAX_FFT_SIZE;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, AppState, PanelChrome, PanelSettings, ScrubState, choices_shared, setting_row, toggle,
};
use crate::panel_settings;
use crate::settings::ui as settings_ui;
use crate::spectrum::{Gradient, gradient_choices, ramp_color};

const WINDOW_MS_MIN: f32 = 1.0;
const WINDOW_MS_MAX: f32 = 100.0;

/// Log-spaced, so the quiet end that needs the boost gets most of the
/// travel.
const GAIN_MIN: f32 = 0.25;
const GAIN_MAX: f32 = 16.0;

const LINE_W_MIN: f32 = 0.5;
const LINE_W_MAX: f32 = 4.0;

/// Full 1.0 would never fade.
const PERSIST_MAX: f32 = 0.95;

/// Bounded: each frame is a column of pairs per channel.
const MAX_TRAILS: usize = 8;

/// The extra frames are the trigger's search slack, so a period longer
/// than two windows can't lock.
const SEARCH_SPAN: usize = 3;

/// What the feed itself holds.
const MAX_PULL: usize = MAX_FFT_SIZE * 2;

/// Keeps a pinned trace off the panel edge.
const HEADROOM: f32 = 0.94;

/// One path is one fill, so the trace is built as one path per ramp step.
const RAMP_STEPS: usize = 8;

const GRID_DIVS: usize = 8;

const MIN_COLS: usize = 2;
const MAX_COLS: usize = 4096;

/// How long the feed may sit still before it reads as stopped rather than
/// a gap between pump ticks. Between ticks the trace holds instead of
/// flattening.
const SILENT_AFTER: f32 = 0.15;

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Off,
    #[default]
    Rising,
    Falling,
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeChannels {
    #[default]
    Mono,
    Overlay,
    Split,
}

impl ScopeChannels {
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

/// A clamp that swallows NaN, which `f32::clamp` passes through. One NaN
/// from a hand-edited layout would take the whole trace.
fn sane(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_nan() {
        fallback
    } else {
        value.clamp(min, max)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OscilloscopeConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub window_ms: f32,
    pub gain: f32,
    pub trigger: Trigger,
    pub trigger_level: f32,
    pub channels: ScopeChannels,
    pub line_width: f32,
    pub fill: bool,
    pub grid: bool,
    pub gradient: Gradient,
    /// `#rrggbb`: the quiet base and the loud tip.
    pub gradient_lo: String,
    pub gradient_hi: String,
    /// Zero draws the standing frame alone.
    pub persistence: f32,
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
    /// Clamped to the typed ceiling, not the strip's top, so a value typed
    /// past the top survives a reload.
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

    fn gain(&self) -> f32 {
        sane(self.gain, GAIN_MIN, GAIN_MAX, 1.0)
    }

    fn trigger_level(&self) -> f32 {
        sane(self.trigger_level, -1.0, 1.0, 0.0)
    }

    fn persistence(&self) -> f32 {
        sane(self.persistence, 0.0, PERSIST_MAX, 0.0)
    }

    fn trails(&self) -> usize {
        ((self.persistence() / PERSIST_MAX) * MAX_TRAILS as f32).round() as usize
    }

    /// Falls back to the theme ramp's ends when a hand-edited hex doesn't
    /// parse.
    fn custom_ramp(&self) -> (Rgba, Rgba) {
        (
            palette::parse_hex(&self.gradient_lo)
                .unwrap_or_else(|| palette::alpha(palette::text_faint(), 0x66)),
            palette::parse_hex(&self.gradient_hi).unwrap_or_else(palette::accent),
        )
    }
}

/// Raw: the gain applies at paint, so dragging the scale on a paused panel
/// moves the standing trace.
type Lane = Vec<(f32, f32)>;

/// None means no crossing. The caller then uses a fixed offset; hunting
/// for the closest match would smear.
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

/// More samples than columns keeps both ends, so a transient still shows;
/// fewer interpolates, so a 1 ms window isn't a staircase. Needs at least
/// two samples and two columns.
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
            // An all-NaN column draws flat rather than with infinite bounds.
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

struct Geometry {
    ox: f32,
    oy: f32,
    w: f32,
    fh: f32,
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

/// Corners run clockwise from top-left, so the pair tiles the quad without
/// overlap and a translucent fill blends once.
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

struct Scope {
    last_written: u64,
    last_fresh: Option<Instant>,
    left: Vec<f32>,
    right: Vec<f32>,
    trace: Vec<Lane>,
    trail: VecDeque<Vec<Lane>>,
    /// Flattened for silence already, so that happens once.
    silent: bool,
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

    /// No new audio holds the frame across the pump-tick gap. Stopped audio
    /// burns the trail off, then flattens, unless `hold` freezes it.
    fn step(&mut self, feed: &AudioFeed, cols: usize, config: &OscilloscopeConfig, hold: bool) {
        let written = feed.written();
        let fresh = written != self.last_written;
        self.last_written = written;

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
                // Underfed but flowing: keep asking for frames.
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
            // Oldest first, so the afterglow burns off from the back.
            if self.trail.pop_front().is_some() {
                self.alive = true;
            } else if !self.silent {
                for lane in &mut self.trace {
                    lane.fill((0.0, 0.0));
                }
                self.silent = true;
                // Paint follows this step, so the flat frame still draws before the
                // panel parks.
                self.alive = false;
            } else {
                self.alive = false;
            }
        } else {
            self.alive = true;
        }
    }

    /// Empty until the feed has buffered a window.
    fn sample(&mut self, feed: &AudioFeed, cols: usize, config: &OscilloscopeConfig) -> Vec<Lane> {
        // A wild device rate would blow the pull past what the feed holds.
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

        // The feed returns short when underfed; measure off what arrived.
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
        // Stereo triggers off the left channel and both lanes share the offset,
        // so the phase between them stays visible.
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

        // Oldest trail first so the standing frame sits on top; each step back
        // dims by the persistence.
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

/// A ribbon between column tops and bottoms rather than a polyline, so
/// steep and flat stretches meet without a seam.
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
        // Overlay: the second channel runs dimmer so the pair stays tellable
        // where they cross.
        let lane_fade = if !geo.split && i > 0 {
            fade * 0.55
        } else {
            fade
        };
        let alpha = (255.0 * lane_fade).clamp(0.0, 255.0) as u8;
        if alpha == 0 {
            continue;
        }

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

            // One band per segment from the trace to the center line, so the halves
            // never overlap where the wave crosses.
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
    window_scrub: ScrubState,
    gain_scrub: ScrubState,
    level_scrub: ScrubState,
    line_w_scrub: ScrubState,
    persist_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    /// Built on the first settings render: the picker state needs a window.
    ramp_pickers: Option<[Entity<ColorPickerState>; 2]>,
    _ramp_changes: Vec<Subscription>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes an idle window when a session starts.
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
        // The observe re-renders on every pump tick while audio moves. Frame
        // polling only burns the trail off after audio stops, then the panel parks.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        // Paused mid-session, not a played-out queue.
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
                    // A column per pixel; denser resamples into subpixels.
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
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
