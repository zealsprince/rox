//! The VU meter panel: per-channel loudness over the player's PCM tap, the
//! classic level meter. One or two meters (mono fold or stereo L/R) grow from
//! the configured edge, colored by the shared loudness ramp, with peak-hold
//! marks riding above. Two ballistics: VU integrates slowly for the needle
//! feel, Peak snaps up and eases down for the PPM look. Like the spectrum, it
//! is paint primitives on the UI thread and parks once the meters settle, so
//! an idle app pays nothing. The frequency vocabulary (the growth edge, the
//! color ramp) is shared with the spectrum panel.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, fill, linear_color_stop, linear_gradient, point, prelude::*, px, size, AnyElement,
    App, Bounds, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, Hsla, Rgba,
    SharedString, Subscription, TextRun, WeakEntity, Window,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Sizable as _;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, choices, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState,
};
use crate::panel_settings;
use crate::panels::spectrum::{
    ramp_color, Gradient, Orientation, GRADIENT_CHOICES, ORIENTATION_CHOICES,
};

/// The most meters the panel draws: stereo is two, mono folds to one.
const MAX_METERS: usize = 2;

/// Frames pulled off the feed each tick to measure the current level. At
/// 48 kHz this is a ~85 ms window, long enough for a steady RMS and short
/// enough that a peak read still catches transients.
const WINDOW: usize = 4096;

/// dB window the meters normalize into, on samples where a full-scale sine
/// sits at 0 dB - the top of the meter. The floor is the quiet end the bar
/// falls to.
const FLOOR_DB: f32 = -60.0;
const MAX_DB: f32 = 0.0;

/// dB marks the scale draws behind the meters: a gridline each, tagged with
/// the level when there's room for the labels.
const DB_MARKS: [f32; 3] = [-6.0, -18.0, -36.0];

/// The VU ballistic's smoothing rate, per second, applied both ways: the
/// needle rises and falls at the same slow rate, integrating the loudness
/// rather than tracking every transient.
const VU_RATE: f32 = 9.0;

/// The peak ballistic's rates, per second: snap up near-instantly, ease down
/// slowly - the PPM look where a transient pins the meter and drifts back.
const PEAK_ATTACK: f32 = 60.0;
const PEAK_RELEASE: f32 = 7.0;

/// The default rate peak-hold caps accelerate downward at, meter heights per
/// second squared - the same floaty drift the spectrum caps use.
const HOLD_GRAVITY: f32 = 0.05;
const GRAVITY_MIN: f32 = 0.01;
const GRAVITY_MAX: f32 = 1.0;

/// The segment sliders' spans, px: how deep each LED cell draws and the dark
/// seam between cells. Values snap to whole pixels; gap zero fuses a stack
/// into a solid bar.
const SEG_H_MIN: f32 = 2.0;
const SEG_H_MAX: f32 = 14.0;
const SEG_GAP_MIN: f32 = 0.0;
const SEG_GAP_MAX: f32 = 4.0;

/// The gap between the two stereo meters, px.
const METER_GAP: f32 = 3.0;

/// Everything below this reads as settled; the panel stops animating.
const EPSILON: f32 = 0.002;

/// How long the feed may sit still before it reads as stopped audio rather
/// than the gap between pump ticks - same as the spectrum's, and for the
/// same reason: between ticks the meters hold instead of dipping.
const SILENT_AFTER: f32 = 0.15;

/// How the meters render: a solid gradient column, or a stack of LED-style
/// segments (the hardware meter look).
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeterStyle {
    #[default]
    Continuous,
    Segments,
}

/// How many meters: fold to one, or split the stereo pair.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channels {
    #[default]
    Stereo,
    Mono,
}

impl Channels {
    fn count(self) -> usize {
        match self {
            Channels::Stereo => 2,
            Channels::Mono => 1,
        }
    }
}

/// How the level is measured and smoothed: VU integrates the RMS slowly,
/// Peak tracks the sample peak with a fast attack and slow release.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ballistics {
    #[default]
    Vu,
    Peak,
}

const STYLE_CHOICES: &[(&str, MeterStyle)] = &[
    ("Continuous", MeterStyle::Continuous),
    ("Segments", MeterStyle::Segments),
];

const CHANNEL_CHOICES: &[(&str, Channels)] =
    &[("Stereo", Channels::Stereo), ("Mono", Channels::Mono)];

const BALLISTICS_CHOICES: &[(&str, Ballistics)] =
    &[("VU", Ballistics::Vu), ("Peak", Ballistics::Peak)];

/// The VU panel's per-view config: what a saved layout restores and what the
/// customize window edits. Missing fields take the defaults, so a layout
/// dumped before a field existed still loads. The growth edge and color ramp
/// reuse the spectrum's types so the two visualizers speak the same terms.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VuConfig {
    /// The rename, theme override, and placement locks shared by every panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Fold to one meter or split the stereo pair.
    pub channels: Channels,
    /// A solid gradient column, or LED-style segments.
    pub style: MeterStyle,
    /// The edge the meters grow from.
    pub orientation: Orientation,
    /// How the level is measured and smoothed.
    pub ballistics: Ballistics,
    /// How the meters color by level: flat accent, or a ramp from the theme,
    /// the cover art, or the custom pair below.
    pub gradient: Gradient,
    /// The custom ramp's ends, `#rrggbb`: the quiet base and the loud tip.
    pub gradient_lo: String,
    pub gradient_hi: String,
    /// Cell depth in the segment style, px.
    pub seg_height: f32,
    /// Dark seam between cells in the segment style, px.
    pub seg_gap: f32,
    /// Peak-hold marks riding above the meters.
    pub caps: bool,
    /// How hard the caps fall, meter heights per second squared.
    pub cap_gravity: f32,
    /// Freeze the meters while playback is paused instead of letting them
    /// fall to silence.
    pub freeze: bool,
    /// Draw the dB scale behind the meters: gridlines with level labels.
    pub scale: bool,
}

impl Default for VuConfig {
    fn default() -> Self {
        VuConfig {
            chrome: PanelChrome::default(),
            channels: Channels::default(),
            style: MeterStyle::default(),
            orientation: Orientation::default(),
            ballistics: Ballistics::default(),
            gradient: Gradient::default(),
            gradient_lo: "#22aa44".into(),
            gradient_hi: "#dd3322".into(),
            seg_height: 4.0,
            seg_gap: 1.0,
            caps: true,
            cap_gravity: HOLD_GRAVITY,
            freeze: false,
            scale: false,
        }
    }
}

impl VuConfig {
    fn seg_h(&self) -> f32 {
        self.seg_height.clamp(SEG_H_MIN, SEG_H_MAX)
    }

    fn seg_gap(&self) -> f32 {
        self.seg_gap.clamp(SEG_GAP_MIN, SEG_GAP_MAX)
    }

    fn gravity(&self) -> f32 {
        self.cap_gravity.clamp(GRAVITY_MIN, GRAVITY_MAX)
    }

    /// The custom ramp's ends parsed, falling back to the theme ramp's when a
    /// hand-edited hex doesn't parse - the same fallback the spectrum uses.
    fn custom_ramp(&self) -> (Rgba, Rgba) {
        (
            palette::parse_hex(&self.gradient_lo)
                .unwrap_or_else(|| palette::alpha(palette::text_faint(), 0x66)),
            palette::parse_hex(&self.gradient_hi).unwrap_or_else(palette::accent),
        )
    }
}

/// A level in [0, 1] from a channel's samples: the sample peak for the Peak
/// ballistic, the RMS for VU. dB-mapped into the meter's window.
fn level_of(samples: &[f32], ballistics: Ballistics) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let metric = match ballistics {
        Ballistics::Peak => samples.iter().fold(0.0f32, |m, &s| m.max(s.abs())),
        Ballistics::Vu => {
            let sum: f32 = samples.iter().map(|&s| s * s).sum();
            (sum / samples.len() as f32).sqrt()
        }
    };
    let db = 20.0 * (metric + 1e-9).log10();
    ((db - FLOOR_DB) / (MAX_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// Per-panel meter state, shared with the paint closure the way the spectrum
/// shares its bars: the entity holds the handle, the closure does the
/// per-frame work where the bounds are known.
struct Meters {
    last_written: u64,
    last_tick: Option<Instant>,
    /// When the feed last carried new audio.
    last_fresh: Option<Instant>,
    /// Per-channel sample scratch, refilled from the feed each fresh tick.
    left: Vec<f32>,
    right: Vec<f32>,
    /// How many meters are live: one folded, or two split.
    count: usize,
    /// What each meter eases toward, and where it sits now.
    targets: [f32; MAX_METERS],
    levels: [f32; MAX_METERS],
    holds: [f32; MAX_METERS],
    hold_vel: [f32; MAX_METERS],
    /// Meters still moving: render keeps requesting frames until this clears.
    alive: bool,
}

impl Meters {
    fn new() -> Self {
        Meters {
            last_written: 0,
            last_tick: None,
            last_fresh: None,
            left: vec![0.0; WINDOW],
            right: vec![0.0; WINDOW],
            count: 1,
            targets: [0.0; MAX_METERS],
            levels: [0.0; MAX_METERS],
            holds: [0.0; MAX_METERS],
            hold_vel: [0.0; MAX_METERS],
            alive: false,
        }
    }

    /// One tick: read the newest window, fold it into per-channel levels,
    /// advance the peak-hold caps. No new audio means the meters decay,
    /// unless `hold` keeps the last frame standing (freeze on pause).
    fn step(&mut self, feed: &AudioFeed, config: &VuConfig, hold: bool) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32().min(0.1))
            .unwrap_or(1.0 / 60.0);
        self.last_tick = Some(now);

        let written = feed.written();
        let fresh = written != self.last_written;
        self.last_written = written;

        self.count = config.channels.count();

        // Frozen and nothing new: keep the standing frame and stop animating.
        if hold && !fresh {
            self.alive = false;
            return;
        }

        if fresh {
            self.last_fresh = Some(now);
        }
        let stopped = self
            .last_fresh
            .is_none_or(|t| (now - t).as_secs_f32() > SILENT_AFTER);

        // New audio: measure the newest window per channel. Nothing new but
        // not yet stopped: hold the targets across the pump-tick gap. Stopped:
        // fall to silence.
        if fresh {
            match config.channels {
                Channels::Stereo => {
                    let n = feed.latest_stereo(&mut self.left, &mut self.right);
                    self.targets[0] = level_of(&self.left[..n], config.ballistics);
                    self.targets[1] = level_of(&self.right[..n], config.ballistics);
                }
                Channels::Mono => {
                    let n = feed.latest_mono(&mut self.left);
                    self.targets[0] = level_of(&self.left[..n], config.ballistics);
                }
            }
        } else if stopped {
            self.targets = [0.0; MAX_METERS];
        }

        let gravity = config.gravity();
        let mut alive = false;
        for i in 0..self.count {
            let target = self.targets[i];
            if hold {
                // Frozen: land on the target at once - the next tick parks
                // again, so an ease would strand the meter partway.
                self.levels[i] = target;
            } else {
                let rate = match config.ballistics {
                    Ballistics::Vu => VU_RATE,
                    Ballistics::Peak => {
                        if target > self.levels[i] {
                            PEAK_ATTACK
                        } else {
                            PEAK_RELEASE
                        }
                    }
                };
                self.levels[i] += (target - self.levels[i]) * (rate * dt).min(1.0);
            }

            // The cap rides up with the meter and falls back under gravity
            // once it drops away. Caps off: the holds shadow the meters so
            // they don't keep the panel animating.
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
        self.alive = alive;
    }

    fn paint(
        &self,
        bounds: Bounds<gpui::Pixels>,
        window: &mut Window,
        cx: &mut App,
        config: &VuConfig,
    ) {
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        if self.count == 0 || w <= 0.0 || h <= 0.0 {
            return;
        }

        // The meters lay along `axis`, levels grow into `depth`, matching the
        // spectrum's axis/depth split so both read the orientation the same.
        let orientation = config.orientation;
        let (axis, depth) = if orientation.horizontal() {
            (w, h)
        } else {
            (h, w)
        };
        let max_d = depth * 0.94;
        let slot = axis / self.count as f32;
        let meter_w = (slot - METER_GAP).max(1.0);

        // Axis/depth space into panel space, the same mapping the spectrum
        // uses: `a` along the meter axis, `d` from the base edge toward the
        // tips.
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

        // dB gridlines behind the meters, each tagged with its level. Text is
        // pricier than the lines, so labels only draw once the meter has room
        // to spread them without stacking; below that the bare lines stand in.
        if config.scale {
            let ox = f32::from(origin.x);
            let oy = f32::from(origin.y);
            let font = window.text_style().font();
            let color: Hsla = palette::text_muted().into();
            let fs = px((9.0 * palette::font_scale()).max(8.0));
            let fh = f32::from(fs);
            let labels = depth >= 56.0 && axis >= 24.0;
            for db in DB_MARKS {
                let d = (db - FLOOR_DB) / (MAX_DB - FLOOR_DB) * max_d;
                window.paint_quad(fill(
                    rect(0.0, axis, d, 1.0),
                    palette::alpha(palette::gridline(), 0x28),
                ));
                if !labels {
                    continue;
                }
                let text: SharedString = format!("{db:.0}").into();
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
                // Sit the tag just clear of its line, along the base edge, then
                // clamp so a mark near a corner never spills out of the panel.
                let (tx, ty) = match orientation {
                    Orientation::Bottom => (ox + 2.0, oy + (h - d) - fh - 1.0),
                    Orientation::Top => (ox + 2.0, oy + d + 1.0),
                    Orientation::Left => (ox + d + 2.0, oy + h - fh - 2.0),
                    Orientation::Right => (ox + (w - d) - lw - 2.0, oy + h - fh - 2.0),
                };
                let tx = tx.clamp(ox, ox + w - lw);
                let ty = ty.clamp(oy, oy + h - fh);
                let _ = line.paint(point(px(tx), px(ty)), fs, window, cx);
            }
        }

        let custom = config.custom_ramp();
        let seg_h = config.seg_h();
        let cell = seg_h + config.seg_gap();
        let cells = ((max_d / cell) as usize).max(1);

        for i in 0..self.count {
            let level = self.levels[i];
            let a = i as f32 * slot;
            if config.style == MeterStyle::Segments {
                // The stack: cells lit up to the level, each colored by its
                // own height on the ramp, so only a full meter's top runs hot.
                let lit = (level * cells as f32).round() as usize;
                for c in 0..lit {
                    let color =
                        ramp_color(config.gradient, (c as f32 + 0.5) / cells as f32, custom);
                    window.paint_quad(fill(rect(a, meter_w, c as f32 * cell, seg_h), color));
                }
                if lit == 0 {
                    // A ghosted cell at the base keeps a silent meter's
                    // footprint, the segment twin of the column's stub.
                    window.paint_quad(fill(
                        rect(a, meter_w, 0.0, seg_h),
                        palette::alpha(ramp_color(config.gradient, 0.0, custom), 0x40),
                    ));
                }
            } else {
                // The solid column, filled with the ramp from the base color
                // up to the color at its own tip, so a short meter shows only
                // the cool end and a pinned one reveals the hot top.
                let bar = rect(a, meter_w, 0.0, (level * max_d).max(2.0));
                let base = ramp_color(config.gradient, 0.0, custom);
                let tip = ramp_color(config.gradient, level, custom);
                window.paint_quad(fill(
                    bar,
                    linear_gradient(
                        orientation.tip_angle(),
                        linear_color_stop(base, 0.0),
                        linear_color_stop(tip, 1.0),
                    ),
                ));
            }
        }

        if !config.caps {
            return;
        }
        // Peak-hold marks at the held level: the highlight, like the spectrum
        // caps and the slider knobs, so they stay legible over the meters.
        for i in 0..self.count {
            let a = i as f32 * slot;
            let cap = if config.style == MeterStyle::Segments {
                let c = ((self.holds[i] * cells as f32).ceil() as usize)
                    .saturating_sub(1)
                    .min(cells - 1);
                rect(a, meter_w, c as f32 * cell, seg_h)
            } else {
                rect(a, meter_w, (self.holds[i] * max_d).min(depth - 1.0), 1.0)
            };
            window.paint_quad(fill(cap, palette::highlight()));
        }
    }
}

pub struct VuPanel {
    state: AppState,
    config: VuConfig,
    feed: Arc<AudioFeed>,
    meters: Arc<Mutex<Meters>>,
    /// The settings sliders' painted bounds and drag state, one per slider so
    /// a drag on one never moves the others.
    seg_h_scrub: ScrubState,
    seg_gap_scrub: ScrubState,
    gravity_scrub: ScrubState,
    /// The custom ramp's pickers, base then tip, built on the first settings
    /// render - the panel itself constructs without a window, which the picker
    /// state needs.
    ramp_pickers: Option<[Entity<ColorPickerState>; 2]>,
    _ramp_changes: Vec<Subscription>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel when a session starts, so an idle window resumes
    /// animating without the player bar's frame pump.
    _player_changed: Subscription,
}

impl VuPanel {
    pub fn new(state: AppState, config: VuConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        VuPanel {
            config,
            feed: state.player.read(cx).feed(),
            state,
            meters: Arc::new(Mutex::new(Meters::new())),
            seg_h_scrub: ScrubState::default(),
            seg_gap_scrub: ScrubState::default(),
            gravity_scrub: ScrubState::default(),
            ramp_pickers: None,
            _ramp_changes: Vec::new(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    fn set_seg_height(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.seg_height = (SEG_H_MIN + fraction * (SEG_H_MAX - SEG_H_MIN)).round();
        cx.notify();
    }

    fn set_seg_gap(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.seg_gap = (SEG_GAP_MIN + fraction * (SEG_GAP_MAX - SEG_GAP_MIN)).round();
        cx.notify();
    }

    fn set_gravity(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.cap_gravity =
            GRAVITY_MIN * (GRAVITY_MAX / GRAVITY_MIN).powf(fraction.clamp(0.0, 1.0));
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
        type ConfigToggle = (&'static str, fn(&VuPanel) -> bool, fn(&mut VuPanel));
        let toggles: Vec<ConfigToggle> = vec![
            (
                "Peak Caps",
                |this| this.config.caps,
                |this| this.config.caps = !this.config.caps,
            ),
            (
                "dB Scale",
                |this| this.config.scale,
                |this| this.config.scale = !this.config.scale,
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

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // While audio moves the direct observe re-renders on every pump tick,
        // the only rate new samples arrive at. Frame polling is just for the
        // falling meters after audio stops; once they settle the panel parks,
        // and a resume wakes it through the pump's play-state notify.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        // Freeze on pause holds the standing frame: paused mid-session, not a
        // played-out queue.
        let hold = self.config.freeze && session && !playing && !player.queue_ended();
        if !playing && self.meters.lock().unwrap().alive {
            window.request_animation_frame();
        }

        let config = self.config.clone();
        let meters = self.meters.clone();
        let feed = self.feed.clone();
        div().size_full().relative().bg(palette::bg_root()).child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, cx| {
                    let mut meters = meters.lock().unwrap();
                    meters.step(&feed, &config, hold);
                    meters.paint(bounds, window, cx, &config);
                },
            )
            .size_full(),
        )
    }
}

impl PanelSettings for VuPanel {
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The custom ramp's pickers on first need; each edit writes its hex
        // back into the config, the format the layout dump carries.
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
        let seg_h = self.config.seg_h();
        let seg_gap = self.config.seg_gap();
        let gravity = self.config.gravity();
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                "Channels",
                Some("Split the stereo pair, or fold to one meter"),
                choices(
                    CHANNEL_CHOICES,
                    self.config.channels,
                    |this: &mut Self, channels, cx| {
                        this.config.channels = channels;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Style",
                Some("A solid column, or LED-style segments"),
                choices(
                    STYLE_CHOICES,
                    self.config.style,
                    |this: &mut Self, style, cx| {
                        this.config.style = style;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Orientation",
                Some("The edge the meters grow from"),
                choices(
                    ORIENTATION_CHOICES,
                    self.config.orientation,
                    |this: &mut Self, orientation, cx| {
                        this.config.orientation = orientation;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Ballistics",
                Some("VU integrates the loudness slowly; Peak snaps up and eases down"),
                choices(
                    BALLISTICS_CHOICES,
                    self.config.ballistics,
                    |this: &mut Self, ballistics, cx| {
                        this.config.ballistics = ballistics;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.style == MeterStyle::Segments, |d| {
                d.child(setting_row(
                    "Segment Height",
                    Some("How tall each cell in a stack draws"),
                    panel::value_slider(
                        &self.seg_h_scrub,
                        (seg_h - SEG_H_MIN) / (SEG_H_MAX - SEG_H_MIN),
                        format!("{seg_h:.0} px"),
                        Self::set_seg_height,
                        cx,
                    ),
                ))
                .child(setting_row(
                    "Segment Gap",
                    Some("The seam between cells in a stack"),
                    panel::value_slider(
                        &self.seg_gap_scrub,
                        (seg_gap - SEG_GAP_MIN) / (SEG_GAP_MAX - SEG_GAP_MIN),
                        format!("{seg_gap:.0} px"),
                        Self::set_seg_gap,
                        cx,
                    ),
                ))
            })
            .child(setting_row(
                "Gradient",
                Some("Color the meters by level: the theme's ramp, the cover art's colors under song theming, or a custom pair"),
                choices(
                    GRADIENT_CHOICES,
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
                        "Base Color",
                        Some("The quiet end of the custom ramp"),
                        ColorPicker::new(&lo).small(),
                    ))
                    .child(setting_row(
                        "Tip Color",
                        Some("The loud end of the custom ramp"),
                        ColorPicker::new(&hi).small(),
                    ))
                },
            )
            .child(setting_row(
                "Peak Caps",
                Some("Hold a mark at each meter's recent peak"),
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
                "Cap Gravity",
                Some("How hard the peak marks fall once the meter drops away"),
                panel::value_slider(
                    &self.gravity_scrub,
                    (gravity / GRAVITY_MIN).ln() / (GRAVITY_MAX / GRAVITY_MIN).ln(),
                    format!("{gravity:.2}"),
                    Self::set_gravity,
                    cx,
                ),
            ))
            .child(setting_row(
                "Hold on Pause",
                Some("Freeze the meters while paused instead of letting them fall to silence"),
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
                "dB Scale",
                Some("Draw labeled gridlines at the dB marks behind the meters"),
                toggle(
                    self.config.scale,
                    |this: &mut Self, on, cx| {
                        this.config.scale = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for VuPanel {}

impl Focusable for VuPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for VuPanel {
    fn panel_name(&self) -> &'static str {
        "vu meter"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "VU Meter")
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
                VuPanel::new(state, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for VuPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}
