//! The VU meter panel: per-channel loudness over the player's PCM tap.
//! One or two meters grow from the configured edge, colored by the loudness
//! ramp shared with the spectrum. VU integrates slowly for the needle feel;
//! Peak snaps up and eases down for the PPM look. The panel parks once the
//! meters settle, so an idle app pays nothing.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    Rgba, SharedString, Subscription, TextRun, WeakEntity, Window, canvas, div, fill,
    linear_color_stop, linear_gradient, point, prelude::*, px, size,
};
use gpui_component::Sizable as _;
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, AppState, PanelChrome, PanelSettings, ScrubState, choices_shared, setting_row, toggle,
};
use crate::panel_settings;
use crate::settings::ui as settings_ui;
use crate::spectrum::{Gradient, Orientation, gradient_choices, orientation_choices, ramp_color};

const MAX_METERS: usize = 2;

/// At 48 kHz, ~85 ms: long enough for a steady RMS, short enough for a
/// peak read to catch transients.
const WINDOW: usize = 4096;

/// A full-scale sine is 0 dB, the meter's top.
const FLOOR_DB: f32 = -60.0;
const MAX_DB: f32 = 0.0;

const DB_MARKS: [f32; 3] = [-6.0, -18.0, -36.0];

/// The same rate both ways, so the needle integrates rather than tracks
/// transients.
const VU_RATE: f32 = 9.0;

const PEAK_ATTACK: f32 = 60.0;
const PEAK_RELEASE: f32 = 7.0;

const HOLD_GRAVITY: f32 = 0.05;
const GRAVITY_MIN: f32 = 0.01;
const GRAVITY_MAX: f32 = 1.0;

const SEG_H_MIN: f32 = 2.0;
const SEG_H_MAX: f32 = 14.0;
const SEG_GAP_MIN: f32 = 0.0;
const SEG_GAP_MAX: f32 = 4.0;

const METER_GAP: f32 = 3.0;

const EPSILON: f32 = 0.002;

/// How long the feed may sit still before it reads as stopped rather than
/// a gap between pump ticks. Between ticks the meters hold instead of
/// dipping.
const SILENT_AFTER: f32 = 0.15;

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeterStyle {
    #[default]
    Continuous,
    Segments,
}

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

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ballistics {
    #[default]
    Vu,
    Peak,
}

fn style_choices() -> [(SharedString, MeterStyle); 2] {
    [
        (rox_i18n::t!("vu-style-continuous"), MeterStyle::Continuous),
        (rox_i18n::t!("vu-style-segments"), MeterStyle::Segments),
    ]
}

fn channel_choices() -> [(SharedString, Channels); 2] {
    [
        (rox_i18n::t!("vu-channels-stereo"), Channels::Stereo),
        (rox_i18n::t!("vu-channels-mono"), Channels::Mono),
    ]
}

fn ballistics_choices() -> [(SharedString, Ballistics); 2] {
    [
        // Untranslated: no locale key exists for this label.
        ("VU".into(), Ballistics::Vu),
        (rox_i18n::t!("vu-ballistics-peak"), Ballistics::Peak),
    ]
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VuConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub channels: Channels,
    pub style: MeterStyle,
    pub orientation: Orientation,
    pub ballistics: Ballistics,
    pub gradient: Gradient,
    /// `#rrggbb`: the quiet base and the loud tip.
    pub gradient_lo: String,
    pub gradient_hi: String,
    pub seg_height: f32,
    pub seg_gap: f32,
    pub caps: bool,
    /// Meter heights per second squared.
    pub cap_gravity: f32,
    pub freeze: bool,
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
            freeze: true,
            scale: false,
        }
    }
}

impl VuConfig {
    /// Clamped to the typed ceiling, not the strip's top, so a value typed
    /// past the top survives a reload.
    fn seg_h(&self) -> f32 {
        self.seg_height
            .clamp(SEG_H_MIN, settings_ui::ceiling(SEG_H_MIN, SEG_H_MAX))
    }

    fn seg_gap(&self) -> f32 {
        self.seg_gap
            .clamp(SEG_GAP_MIN, settings_ui::ceiling(SEG_GAP_MIN, SEG_GAP_MAX))
    }

    fn gravity(&self) -> f32 {
        self.cap_gravity.clamp(GRAVITY_MIN, GRAVITY_MAX)
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

struct Meters {
    last_written: u64,
    last_tick: Option<Instant>,
    last_fresh: Option<Instant>,
    left: Vec<f32>,
    right: Vec<f32>,
    count: usize,
    targets: [f32; MAX_METERS],
    levels: [f32; MAX_METERS],
    holds: [f32; MAX_METERS],
    hold_vel: [f32; MAX_METERS],
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

        // Nothing new but not yet stopped: hold across the pump-tick gap.
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
                // Frozen: jump straight to the target, since the next tick parks again.
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

            // Caps off: the holds track the meters so they don't keep the panel
            // animating.
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

        let orientation = config.orientation;
        let (axis, depth) = if orientation.horizontal() {
            (w, h)
        } else {
            (h, w)
        };
        let max_d = depth * 0.94;
        let slot = axis / self.count as f32;
        let meter_w = (slot - METER_GAP).max(1.0);

        // `a` runs along the meter axis, `d` from the base edge toward the tips.
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

        // Labels only once there's room to spread them; text costs more than
        // the lines.
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
                let lit = (level * cells as f32).round() as usize;
                for c in 0..lit {
                    let color =
                        ramp_color(config.gradient, (c as f32 + 0.5) / cells as f32, custom);
                    window.paint_quad(fill(rect(a, meter_w, c as f32 * cell, seg_h), color));
                }
                if lit == 0 {
                    window.paint_quad(fill(
                        rect(a, meter_w, 0.0, seg_h),
                        palette::alpha(ramp_color(config.gradient, 0.0, custom), 0x40),
                    ));
                }
            } else {
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
    seg_h_scrub: ScrubState,
    seg_gap_scrub: ScrubState,
    gravity_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    /// Built on the first settings render: the picker state needs a window.
    ramp_pickers: Option<[Entity<ColorPickerState>; 2]>,
    _ramp_changes: Vec<Subscription>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes an idle window when a session starts.
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
            value_edit: panel::ValueEdit::default(),
            ramp_pickers: None,
            _ramp_changes: Vec::new(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
        }
    }

    fn set_seg_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.config.seg_height = height;
        cx.notify();
    }

    fn set_seg_gap(&mut self, gap: f32, cx: &mut Context<Self>) {
        self.config.seg_gap = gap;
        cx.notify();
    }

    fn set_gravity(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.cap_gravity =
            GRAVITY_MIN * (GRAVITY_MAX / GRAVITY_MIN).powf(fraction.clamp(0.0, 1.0));
        cx.notify();
    }

    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        type ConfigToggle = (SharedString, fn(&VuPanel) -> bool, fn(&mut VuPanel));
        let toggles: Vec<ConfigToggle> = vec![
            (
                rox_i18n::t!("vu-peak-caps"),
                |this| this.config.caps,
                |this| this.config.caps = !this.config.caps,
            ),
            (
                rox_i18n::t!("vu-db-scale"),
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
        menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("library-menu-display"),
            submenu,
        ))
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The observe re-renders on every pump tick while audio moves. Frame
        // polling only runs the fall after audio stops, then the panel parks.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        // Paused mid-session, not a played-out queue.
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
        let seg_h = self.config.seg_h();
        let seg_gap = self.config.seg_gap();
        let gravity = self.config.gravity();
        let meter = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("vu-channels"),
                Some(rox_i18n::t!("vu-channels.description")),
                choices_shared(
                    &channel_choices(),
                    self.config.channels,
                    |this: &mut Self, channels, cx| {
                        this.config.channels = channels;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("vu-style"),
                Some(rox_i18n::t!("vu-style.description")),
                choices_shared(
                    &style_choices(),
                    self.config.style,
                    |this: &mut Self, style, cx| {
                        this.config.style = style;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("vu-orientation"),
                Some(rox_i18n::t!("vu-orientation.description")),
                choices_shared(
                    &orientation_choices(),
                    self.config.orientation,
                    |this: &mut Self, orientation, cx| {
                        this.config.orientation = orientation;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("vu-ballistics"),
                Some(rox_i18n::t!("vu-ballistics.description")),
                choices_shared(
                    &ballistics_choices(),
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
                    rox_i18n::t!("vu-segment-height"),
                    Some(rox_i18n::t!("vu-segment-height.description")),
                    settings_ui::scalar(
                        &self.seg_h_scrub,
                        &self.value_edit,
                        seg_h,
                        settings_ui::span(SEG_H_MIN, SEG_H_MAX, " px"),
                        Self::set_seg_height,
                        cx,
                    ),
                ))
                .child(setting_row(
                    rox_i18n::t!("vu-segment-gap"),
                    Some(rox_i18n::t!("vu-segment-gap.description")),
                    settings_ui::scalar(
                        &self.seg_gap_scrub,
                        &self.value_edit,
                        seg_gap,
                        settings_ui::span(SEG_GAP_MIN, SEG_GAP_MAX, " px"),
                        Self::set_seg_gap,
                        cx,
                    ),
                ))
            });
        let color = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("vu-gradient-mode"),
                Some(rox_i18n::t!("vu-gradient-mode.description")),
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
        let peaks = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("vu-peak-caps"),
                Some(rox_i18n::t!("vu-peak-caps.description")),
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
                rox_i18n::t!("vu-cap-gravity"),
                Some(rox_i18n::t!("vu-cap-gravity.description")),
                panel::value_slider_edit(
                    &self.gravity_scrub,
                    &self.value_edit,
                    (gravity / GRAVITY_MIN).ln() / (GRAVITY_MAX / GRAVITY_MIN).ln(),
                    format!("{gravity:.2}"),
                    format!("{gravity:.2}"),
                    |v| (v / GRAVITY_MIN).ln() / (GRAVITY_MAX / GRAVITY_MIN).ln(),
                    Self::set_gravity,
                    cx,
                ),
            ));
        div()
            .flex()
            .flex_col()
            .gap(settings_ui::SECTION_GAP)
            .child(settings_ui::section(
                rox_i18n::t!("vu-section-meter"),
                None,
                meter,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-color"),
                None,
                color,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-peaks"),
                None,
                peaks,
            ))
            .child(settings_ui::section(
                rox_i18n::t!("viz-section-scale"),
                None,
                setting_row(
                    rox_i18n::t!("vu-db-scale"),
                    Some(rox_i18n::t!("vu-db-scale.description")),
                    toggle(
                        self.config.scale,
                        |this: &mut Self, on, cx| {
                            this.config.scale = on;
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
                    rox_i18n::t!("vu-hold-on-pause"),
                    Some(rox_i18n::t!("vu-hold-on-pause.description")),
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

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-vu"),
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
                VuPanel::new(state, config, cx)
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

impl Render for VuPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}
