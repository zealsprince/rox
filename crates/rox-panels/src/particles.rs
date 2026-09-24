//! The particles panel: a field of emitters, made musical by routing the
//! app's shared signals onto its knobs. An emitter is pure geometry and
//! throw; unbound it fountains at its sliders. Reactivity is all routes,
//! evaluated once per frame in the app-wide [`SignalHub`]. The panel stops
//! asking for frames once the last particle dies.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    AnyElement, App, BorderStyle, Bounds, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Rgba, SharedString,
    Subscription, WeakEntity, Window, canvas, div, point, prelude::*, px, size,
};
use gpui_component::Sizable as _;
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::signal::{Route, SignalHub};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit, setting_row, toggle,
};
use crate::panel_settings;
use crate::settings::ui::{self as settings_ui, SECTION_GAP, section};
use crate::signal_ui::{self, RouteHost, RouteTargets, SignalHost, SignalUi};

/// Hysteresis between fire and re-arm, so one swell can't stutter-fire.
const BURST_FIRE: f32 = 0.6;
const BURST_REARM: f32 = 0.3;

/// Past this, spawns drop until older particles age out; a pinned
/// emitter would run away otherwise.
const MAX_PARTICLES: usize = 4000;

/// Past the edge by a fraction of the larger side, with a px floor, so a
/// thrown particle can arc back under gravity.
const CULL_MARGIN: f32 = 0.25;
const CULL_MARGIN_MIN: f32 = 64.0;

/// The floor is zero because emitters have no threshold of their own: a
/// route resting at its Quiet end has to be able to stop one.
const RATE_MIN: f32 = 0.0;
const RATE_MAX: f32 = 300.0;

const SPEED_MIN: f32 = 0.0;
const SPEED_MAX: f32 = 600.0;

const BURST_MIN: f32 = 1.0;
const BURST_MAX: f32 = 120.0;

const GRAVITY_MAX: f32 = 900.0;

/// Per second; zero is a vacuum.
const DRAG_MAX: f32 = 4.0;

const TURB_MAX: f32 = 600.0;
const TURB_SCALE_MIN: f32 = 40.0;
const TURB_SCALE_MAX: f32 = 600.0;
const TURB_SPEED_MAX: f32 = 2.0;

const LIFE_MIN: f32 = 0.2;
const LIFE_MAX: f32 = 6.0;

const SIZE_MIN: f32 = 1.0;
const SIZE_MAX: f32 = 16.0;

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Point,
    #[default]
    Line,
    Box,
    Ring,
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Aim {
    #[default]
    Fixed,
    Outward,
}

fn shape_choices() -> [(SharedString, Shape); 4] {
    [
        (rox_i18n::t!("particles-shape-point"), Shape::Point),
        (rox_i18n::t!("particles-shape-line"), Shape::Line),
        (rox_i18n::t!("particles-shape-box"), Shape::Box),
        (rox_i18n::t!("particles-shape-ring"), Shape::Ring),
    ]
}

fn aim_choices() -> [(SharedString, Aim); 2] {
    [
        (rox_i18n::t!("particles-aim-fixed"), Aim::Fixed),
        (rox_i18n::t!("particles-aim-outward"), Aim::Outward),
    ]
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    #[default]
    Continuous,
    Burst,
}

fn trigger_choices() -> [(SharedString, Trigger); 2] {
    [
        (
            rox_i18n::t!("particles-trigger-continuous"),
            Trigger::Continuous,
        ),
        (rox_i18n::t!("particles-burst"), Trigger::Burst),
    ]
}

/// The factor scales the knob's own setting, so the slider stays the
/// reference a route works against. The ids only ever grow. To make a
/// value bindable, add an entry here and wrap its row in
/// [`crate::signal_ui::bindable_row`].
struct BindTarget {
    id: &'static str,
    /// An i18n key: a `const` array can't hold the `SharedString` `t!` returns.
    label_key: &'static str,
    apply: fn(&mut Scene, &mut Forces, f32),
}

const BIND_TARGETS: &[BindTarget] = &[
    BindTarget {
        id: "gravity",
        label_key: "particles-gravity",
        apply: |scene, _, k| scene.gravity *= k,
    },
    BindTarget {
        id: "drag",
        label_key: "particles-drag",
        apply: |scene, _, k| scene.drag *= k,
    },
    BindTarget {
        id: "turbulence",
        label_key: "particles-turbulence",
        apply: |_, forces, k| forces.turbulence *= k,
    },
    BindTarget {
        id: "scale",
        label_key: "particles-turbulence-scale",
        apply: |_, forces, k| forces.turbulence_scale *= k,
    },
    BindTarget {
        id: "drift",
        label_key: "particles-turbulence-drift",
        apply: |_, forces, k| forces.turbulence_speed *= k,
    },
];

struct EmitterBindTarget {
    id: &'static str,
    label_key: &'static str,
    apply: fn(&mut Emitter, f32),
}

const EMITTER_BIND_TARGETS: &[EmitterBindTarget] = &[
    EmitterBindTarget {
        id: "speed",
        label_key: "particles-speed",
        apply: |emitter, k| emitter.speed *= k,
    },
    EmitterBindTarget {
        id: "rate",
        label_key: "particles-rate",
        apply: |emitter, k| emitter.rate *= k,
    },
    EmitterBindTarget {
        id: "burst",
        label_key: "particles-burst",
        apply: |emitter, k| emitter.burst *= k,
    },
    EmitterBindTarget {
        id: "cone",
        label_key: "particles-cone",
        apply: |emitter, k| emitter.cone *= k,
    },
    EmitterBindTarget {
        id: "size",
        label_key: "particles-size",
        apply: |emitter, k| emitter.size *= k,
    },
    EmitterBindTarget {
        id: "life",
        label_key: "particles-lifetime",
        apply: |emitter, k| emitter.life *= k,
    },
];

/// No audio of its own: reactivity arrives through routes onto its knobs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Emitter {
    /// Stable and persisted, so a route survives removals shifting the list.
    /// 0 is unassigned; the panel assigns on load and on add.
    pub id: u64,
    pub enabled: bool,
    pub rate: f32,
    /// Burst with no route on the burst knob stays silent: the route is the
    /// trigger.
    pub mode: Trigger,
    pub burst: f32,
    /// Size in px, life in seconds, both varied a little per particle.
    pub size: f32,
    pub life: f32,
    pub shape: Shape,
    /// Fractions of the panel, so a resize keeps the arrangement.
    pub x: f32,
    pub y: f32,
    /// Fractions of the panel. A line uses `width` as its length, a box both,
    /// a ring `width` as its radius, a point neither.
    pub width: f32,
    pub height: f32,
    /// Degrees clockwise; a line at 0 runs horizontally.
    pub rotation: f32,
    pub aim: Aim,
    /// Degrees clockwise from up.
    pub direction: f32,
    /// Degrees around the launch angle: 0 is a beam, 360 throws every way.
    pub cone: f32,
    pub speed: f32,
    /// `#rrggbb`; None follows the theme accent.
    pub color: Option<String>,
}

impl Default for Emitter {
    fn default() -> Self {
        Emitter {
            id: 0,
            enabled: true,
            rate: 60.0,
            mode: Trigger::Continuous,
            burst: 24.0,
            size: 4.0,
            life: 2.5,
            shape: Shape::Point,
            x: 0.5,
            y: 0.5,
            width: 1.0,
            height: 0.2,
            rotation: 0.0,
            aim: Aim::Fixed,
            direction: 0.0,
            cone: 360.0,
            speed: 200.0,
            color: None,
        }
    }
}

impl Emitter {
    fn rate(&self) -> f32 {
        self.rate.clamp(RATE_MIN, RATE_MAX)
    }

    fn size(&self) -> f32 {
        self.size.clamp(SIZE_MIN, SIZE_MAX)
    }

    fn life(&self) -> f32 {
        self.life.clamp(LIFE_MIN, LIFE_MAX)
    }

    fn burst(&self) -> f32 {
        self.burst.clamp(BURST_MIN, BURST_MAX)
    }

    fn speed(&self) -> f32 {
        self.speed.clamp(SPEED_MIN, SPEED_MAX)
    }

    fn cone(&self) -> f32 {
        self.cone.clamp(0.0, 360.0)
    }

    fn center(&self) -> (f32, f32) {
        (self.x.clamp(0.0, 1.0), self.y.clamp(0.0, 1.0))
    }

    fn width(&self) -> f32 {
        self.width.clamp(0.0, 2.0)
    }

    fn height(&self) -> f32 {
        self.height.clamp(0.0, 2.0)
    }

    fn color(&self) -> Rgba {
        self.color
            .as_deref()
            .and_then(palette::parse_hex)
            .unwrap_or_else(palette::accent)
    }

    fn next_after(previous: Option<&Emitter>) -> Emitter {
        let Some(previous) = previous else {
            return Emitter::default();
        };
        Emitter {
            x: (previous.x + 0.12).min(1.0),
            y: (previous.y - 0.12).max(0.0),
            ..previous.clone()
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    /// Px per second squared, and the angle in degrees clockwise from up.
    pub gravity: f32,
    pub gravity_angle: f32,
    pub drag: f32,
    pub round: bool,
    pub glow: bool,
    pub freeze: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            gravity: 0.0,
            gravity_angle: 180.0,
            drag: 0.4,
            round: true,
            glow: false,
            freeze: true,
        }
    }
}

impl Scene {
    fn gravity(&self) -> f32 {
        self.gravity.clamp(0.0, GRAVITY_MAX)
    }

    fn drag(&self) -> f32 {
        self.drag.clamp(0.0, DRAG_MAX)
    }
}

/// Drift that varies across the panel rather than pulling one way.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Forces {
    pub turbulence: f32,
    pub turbulence_scale: f32,
    pub turbulence_speed: f32,
}

impl Default for Forces {
    fn default() -> Self {
        Forces {
            turbulence: 280.0,
            turbulence_scale: 220.0,
            turbulence_speed: 1.0,
        }
    }
}

impl Forces {
    fn turbulence(&self) -> f32 {
        self.turbulence.clamp(0.0, TURB_MAX)
    }

    fn scale(&self) -> f32 {
        self.turbulence_scale.clamp(TURB_SCALE_MIN, TURB_SCALE_MAX)
    }

    fn speed(&self) -> f32 {
        self.turbulence_speed.clamp(0.0, TURB_SPEED_MAX)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParticlesConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub emitters: Vec<Emitter>,
    /// A route whose signal is gone from the pool goes quiet.
    pub routes: Vec<Route>,
    pub scene: Scene,
    pub forces: Forces,
}

impl Default for ParticlesConfig {
    fn default() -> Self {
        ParticlesConfig {
            chrome: PanelChrome::default(),
            emitters: vec![Emitter::default()],
            routes: Vec::new(),
            scene: Scene::default(),
            forces: Forces::default(),
        }
    }
}

/// Zeroes and hand-edited duplicates get fresh ids, so a stale binding
/// goes quiet instead of firing at the wrong emitter.
fn assign_emitter_ids(emitters: &mut [Emitter]) {
    let mut next = emitters.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    for i in 0..emitters.len() {
        let taken = emitters[..i].iter().any(|e| e.id == emitters[i].id);
        if emitters[i].id == 0 || taken {
            emitters[i].id = next;
            next += 1;
        }
    }
}

fn emitter_route(target: &str) -> Option<(u64, &str)> {
    let (id, knob) = target.strip_prefix('e')?.split_once('.')?;
    Some((id.parse().ok()?, knob))
}

/// Dispatches plain ids through the scene and force table and
/// `e<id>.<knob>` through the emitter table. Unknown ids fall through.
struct Modulated {
    emitters: Vec<Emitter>,
    scene: Scene,
    forces: Forces,
}

impl RouteTargets for Modulated {
    fn targets(&self) -> Vec<(String, String)> {
        let mut targets: Vec<(String, String)> = BIND_TARGETS
            .iter()
            .map(|t| (t.id.to_string(), rox_i18n::t!(t.label_key).to_string()))
            .collect();
        for (i, emitter) in self.emitters.iter().enumerate() {
            for t in EMITTER_BIND_TARGETS {
                targets.push((
                    format!("e{}.{}", emitter.id, t.id),
                    rox_i18n::t!(
                        "particles-emitter-target",
                        index = (i + 1) as u64,
                        target = rox_i18n::t!(t.label_key).to_string()
                    )
                    .to_string(),
                ));
            }
        }
        targets
    }

    fn apply(&mut self, id: &str, value: f32) {
        if let Some((eid, knob)) = emitter_route(id) {
            if let (Some(emitter), Some(target)) = (
                self.emitters.iter_mut().find(|e| e.id == eid),
                EMITTER_BIND_TARGETS.iter().find(|t| t.id == knob),
            ) {
                (target.apply)(emitter, value);
            }
            return;
        }
        if let Some(target) = BIND_TARGETS.iter().find(|t| t.id == id) {
            (target.apply)(&mut self.scene, &mut self.forces, value);
        }
    }
}

fn modulated(config: &ParticlesConfig, hub: &SignalHub) -> (Vec<Emitter>, Scene, Forces) {
    let mut targets = Modulated {
        emitters: config.emitters.clone(),
        scene: config.scene.clone(),
        forces: config.forces.clone(),
    };
    signal_ui::apply_routes(&config.routes, hub, &mut targets);
    (targets.emitters, targets.scene, targets.forces)
}

/// The last enabled route onto the burst knob, matching the order
/// [`modulated`] applies.
fn burst_signal(config: &ParticlesConfig, emitter_id: u64) -> Option<u64> {
    let target = format!("e{emitter_id}.burst");
    config
        .routes
        .iter()
        .rev()
        .find(|r| r.enabled && r.target == target)
        .map(|r| r.signal)
}

fn heading(degrees: f32) -> (f32, f32) {
    let r = degrees.to_radians();
    (r.sin(), -r.cos())
}

/// xorshift32: the field needs scatter, not statistics, and this keeps a
/// dependency out.
fn rand01(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state >> 8) as f32 / (1u32 << 24) as f32
}

fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1) ^ seed;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Smoothed, so neighbours read nearly the same value and the drift looks
/// like wind rather than jitter.
fn noise2(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let (xi, yi) = (x0 as i32, y0 as i32);
    let a = hash2(xi, yi, seed);
    let b = hash2(xi + 1, yi, seed);
    let c = hash2(xi, yi + 1, seed);
    let d = hash2(xi + 1, yi + 1, seed);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sy
}

struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    age: f32,
    life: f32,
    size: f32,
    color: Rgba,
}

/// The audio analysis lives in the shared [`SignalHub`]; the sim only
/// reads values.
struct Sim {
    last_tick: Option<Instant>,
    /// Fractional spawns carried between ticks, so a slow rate doesn't round
    /// to zero.
    carry: Vec<f32>,
    /// Re-armed once the routed signal falls back, so one rise throws one pop.
    armed: Vec<bool>,
    particles: Vec<Particle>,
    clock: f32,
    rng: u32,
    alive: bool,
}

impl Sim {
    fn new() -> Self {
        Sim {
            last_tick: None,
            carry: Vec::new(),
            armed: Vec::new(),
            particles: Vec::new(),
            clock: 0.0,
            rng: 0x9e37_79b9,
            alive: false,
        }
    }

    /// Reading the routes advances the shared hub. `hold` parks the field.
    fn step(&mut self, hub: &SignalHub, w: f32, h: f32, config: &ParticlesConfig, hold: bool) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32().min(0.1))
            .unwrap_or(1.0 / 60.0);
        self.last_tick = Some(now);

        if hold {
            self.alive = false;
            return;
        }
        self.clock += dt;

        let (emitters, scene, forces) = modulated(config, hub);
        self.carry.resize(emitters.len(), 0.0);
        self.armed.resize(emitters.len(), true);

        for (i, emitter) in emitters.iter().enumerate() {
            if !emitter.enabled {
                self.carry[i] = 0.0;
                continue;
            }
            let color = emitter.color();
            let due = match emitter.mode {
                Trigger::Continuous => {
                    self.carry[i] += emitter.rate() * dt;
                    let due = self.carry[i].floor();
                    self.carry[i] -= due;
                    due as usize
                }
                // Fires on the rising edge, with hysteresis.
                Trigger::Burst => {
                    let value = burst_signal(config, emitter.id)
                        .and_then(|id| hub.value(id))
                        .unwrap_or(0.0);
                    if self.armed[i] && value >= BURST_FIRE {
                        self.armed[i] = false;
                        emitter.burst().round() as usize
                    } else {
                        if !self.armed[i] && value <= BURST_REARM {
                            self.armed[i] = true;
                        }
                        0
                    }
                }
            };
            for _ in 0..due {
                if self.particles.len() >= MAX_PARTICLES {
                    break;
                }
                self.spawn(emitter, w, h, color);
            }
        }

        self.advance(w, h, dt, &scene, &forces);
    }

    fn spawn(&mut self, emitter: &Emitter, w: f32, h: f32, color: Rgba) {
        let (fx, fy) = emitter.center();
        let (cx, cy) = (fx * w, fy * h);
        let rot = emitter.rotation.to_radians();
        // The offset from the center is also the direction Outward aims along.
        let (ox, oy) = match emitter.shape {
            Shape::Point => (0.0, 0.0),
            Shape::Line => {
                let t = (rand01(&mut self.rng) - 0.5) * emitter.width() * w;
                (t * rot.cos(), t * rot.sin())
            }
            Shape::Box => {
                let lx = (rand01(&mut self.rng) - 0.5) * emitter.width() * w;
                let ly = (rand01(&mut self.rng) - 0.5) * emitter.height() * h;
                (
                    lx * rot.cos() - ly * rot.sin(),
                    lx * rot.sin() + ly * rot.cos(),
                )
            }
            Shape::Ring => {
                let radius = emitter.width() * w.min(h) * 0.5;
                let a = rand01(&mut self.rng) * std::f32::consts::TAU;
                (radius * a.cos(), radius * a.sin())
            }
        };

        // A particle exactly on the center has no outward, so it takes a random
        // heading.
        let base = match emitter.aim {
            Aim::Outward if ox.abs() > 1e-4 || oy.abs() > 1e-4 => ox.atan2(-oy).to_degrees(),
            Aim::Outward => rand01(&mut self.rng) * 360.0,
            Aim::Fixed => emitter.direction,
        };
        let spread = (rand01(&mut self.rng) - 0.5) * emitter.cone();
        let (hx, hy) = heading(base + spread);
        let speed = emitter.speed() * (0.6 + 0.4 * rand01(&mut self.rng));
        let life = emitter.life() * (0.7 + 0.6 * rand01(&mut self.rng));
        let size = emitter.size() * (0.6 + 0.8 * rand01(&mut self.rng));
        self.particles.push(Particle {
            x: cx + ox,
            y: cy + oy,
            vx: hx * speed,
            vy: hy * speed,
            age: 0.0,
            life,
            size,
            color,
        });
    }

    fn advance(&mut self, w: f32, h: f32, dt: f32, scene: &Scene, forces: &Forces) {
        let (gx, gy) = heading(scene.gravity_angle);
        let gravity = scene.gravity();
        let (gx, gy) = (gx * gravity, gy * gravity);
        let damp = (1.0 - scene.drag() * dt).clamp(0.0, 1.0);
        let turbulence = forces.turbulence();
        let inv_scale = 1.0 / forces.scale();
        let drift = self.clock * forces.speed();
        let margin = (w.max(h) * CULL_MARGIN).max(CULL_MARGIN_MIN);

        self.particles.retain_mut(|p| {
            let (mut ax, mut ay) = (gx, gy);
            if turbulence > 0.0 {
                // Offset lookups so the x and y pushes don't march in lockstep.
                let nx = noise2(p.x * inv_scale, p.y * inv_scale + drift, 0x51ed_2701) - 0.5;
                let ny = noise2(p.x * inv_scale + 37.5, p.y * inv_scale + drift, 0x9e17_84b5) - 0.5;
                ax += nx * 2.0 * turbulence;
                ay += ny * 2.0 * turbulence;
            }
            p.vx = (p.vx + ax * dt) * damp;
            p.vy = (p.vy + ay * dt) * damp;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.age += dt;
            p.age < p.life && p.x > -margin && p.x < w + margin && p.y > -margin && p.y < h + margin
        });
        self.alive = !self.particles.is_empty();
    }

    fn paint(&self, bounds: Bounds<gpui::Pixels>, window: &mut Window, scene: &Scene) {
        let origin = bounds.origin;
        for p in &self.particles {
            // Fade over the back half of the life instead of blinking off.
            let t = (p.age / p.life).clamp(0.0, 1.0);
            let fade = ((1.0 - t) * 2.0).min(1.0);
            // The halo shares the core's fade.
            if scene.glow {
                let halo = p.size * 2.5;
                let color = palette::alpha(p.color, (fade * 70.0) as u8);
                let radius = if scene.round { halo / 2.0 } else { halo * 0.2 };
                window.paint_quad(gpui::quad(
                    Bounds::new(
                        point(
                            origin.x + px(p.x - halo / 2.0),
                            origin.y + px(p.y - halo / 2.0),
                        ),
                        size(px(halo), px(halo)),
                    ),
                    radius,
                    color,
                    0.,
                    gpui::transparent_black(),
                    BorderStyle::default(),
                ));
            }
            let color = palette::alpha(p.color, (fade * 255.0) as u8);
            let radius = if scene.round { p.size / 2.0 } else { 0.0 };
            let rect = Bounds::new(
                point(
                    origin.x + px(p.x - p.size / 2.0),
                    origin.y + px(p.y - p.size / 2.0),
                ),
                size(px(p.size), px(p.size)),
            );
            window.paint_quad(gpui::quad(
                rect,
                radius,
                color,
                0.,
                gpui::transparent_black(),
                BorderStyle::default(),
            ));
        }
    }
}

const GRAB_RADIUS: f32 = 24.0;

/// Dots are the one outline every shape can draw with axis-aligned quads,
/// rotation included.
fn paint_markers(
    config: &ParticlesConfig,
    drag: Option<usize>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let dot = |window: &mut Window, x: f32, y: f32, r: f32, color: Rgba| {
        window.paint_quad(gpui::quad(
            Bounds::new(
                point(bounds.origin.x + px(x - r), bounds.origin.y + px(y - r)),
                size(px(r * 2.0), px(r * 2.0)),
            ),
            r,
            color,
            0.,
            gpui::transparent_black(),
            BorderStyle::default(),
        ));
    };
    for (i, emitter) in config.emitters.iter().enumerate() {
        let color = emitter.color();
        let strong = palette::alpha(color, if emitter.enabled { 200 } else { 80 });
        let faint = palette::alpha(color, if emitter.enabled { 120 } else { 50 });
        let (fx, fy) = emitter.center();
        let (ex, ey) = (fx * w, fy * h);
        let rot = emitter.rotation.to_radians();
        match emitter.shape {
            Shape::Point => {}
            Shape::Line => {
                let len = emitter.width() * w;
                let n = ((len / 14.0) as usize).clamp(8, 48);
                for k in 0..=n {
                    let t = (k as f32 / n as f32 - 0.5) * len;
                    dot(window, ex + t * rot.cos(), ey + t * rot.sin(), 1.5, faint);
                }
            }
            Shape::Box => {
                let bw = emitter.width() * w;
                let bh = emitter.height() * h;
                let n = (((bw + bh) / 14.0) as usize).clamp(8, 64);
                for k in 0..n {
                    // The perimeter as one 0..4 loop, a side per unit.
                    let t = k as f32 / n as f32 * 4.0;
                    let (lx, ly) = match t as usize {
                        0 => ((t - 0.5) * bw, -bh / 2.0),
                        1 => (bw / 2.0, (t - 1.5) * bh),
                        2 => ((2.5 - t) * bw, bh / 2.0),
                        _ => (-bw / 2.0, (3.5 - t) * bh),
                    };
                    dot(
                        window,
                        ex + lx * rot.cos() - ly * rot.sin(),
                        ey + lx * rot.sin() + ly * rot.cos(),
                        1.5,
                        faint,
                    );
                }
            }
            Shape::Ring => {
                let radius = emitter.width() * w.min(h) * 0.5;
                let n = ((radius / 6.0) as usize).clamp(12, 64);
                for k in 0..n {
                    let a = k as f32 / n as f32 * std::f32::consts::TAU;
                    dot(
                        window,
                        ex + radius * a.cos(),
                        ey + radius * a.sin(),
                        1.5,
                        faint,
                    );
                }
            }
        }
        let r = if drag == Some(i) { 7.0 } else { 5.0 };
        dot(window, ex, ey, r + 2.5, palette::alpha(color, 60));
        dot(window, ex, ey, r, strong);
    }
}

#[derive(Default)]
struct EmitterScrubs {
    rate: ScrubState,
    burst: ScrubState,
    size: ScrubState,
    life: ScrubState,
    x: ScrubState,
    y: ScrubState,
    width: ScrubState,
    height: ScrubState,
    rotation: ScrubState,
    direction: ScrubState,
    cone: ScrubState,
    speed: ScrubState,
}

type ConfigToggle = (
    SharedString,
    fn(&ParticlesPanel) -> bool,
    fn(&mut ParticlesPanel),
);

pub struct ParticlesPanel {
    state: AppState,
    config: ParticlesConfig,
    sim: Arc<Mutex<Sim>>,
    emitter_scrubs: Vec<EmitterScrubs>,
    /// Built on the first settings render (the picker state needs a window)
    /// and rebuilt on a count change, since a removal shifts every index.
    emitter_pickers: Vec<Entity<ColorPickerState>>,
    _emitter_changes: Vec<Subscription>,
    /// Kept in step with the lists by [`signal_ui::sync`].
    signal_ui: SignalUi,
    gravity_scrub: ScrubState,
    gravity_angle_scrub: ScrubState,
    drag_scrub: ScrubState,
    turbulence_scrub: ScrubState,
    turb_scale_scrub: ScrubState,
    turb_speed_scrub: ScrubState,
    focus: FocusHandle,
    value_edit: ValueEdit,
    /// Session state, not persisted.
    edit: bool,
    drag: Option<usize>,
    /// For mapping editor presses into emitter fractions.
    canvas_bounds: Arc<Mutex<Bounds<Pixels>>>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes an idle window when a session starts.
    _player_changed: Subscription,
}

impl ParticlesPanel {
    pub fn new(state: AppState, mut config: ParticlesConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        assign_emitter_ids(&mut config.emitters);
        ParticlesPanel {
            config,
            state,
            sim: Arc::new(Mutex::new(Sim::new())),
            emitter_scrubs: Vec::new(),
            emitter_pickers: Vec::new(),
            _emitter_changes: Vec::new(),
            signal_ui: SignalUi::default(),
            gravity_scrub: ScrubState::default(),
            gravity_angle_scrub: ScrubState::default(),
            drag_scrub: ScrubState::default(),
            turbulence_scrub: ScrubState::default(),
            turb_scale_scrub: ScrubState::default(),
            turb_speed_scrub: ScrubState::default(),
            focus: cx.focus_handle().tab_stop(true),
            value_edit: ValueEdit::default(),
            edit: false,
            drag: None,
            canvas_bounds: Arc::new(Mutex::new(Bounds::default())),
            tab_panel: None,
            _player_changed,
        }
    }

    fn add_emitter(&mut self, cx: &mut Context<Self>) {
        let mut emitter = Emitter::next_after(self.config.emitters.last());
        emitter.id = self.config.emitters.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        self.config.emitters.push(emitter);
        cx.notify();
    }

    fn remove_emitter(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.config.emitters.len() {
            self.config.emitters.remove(index);
            cx.notify();
        }
    }

    fn editor_grab(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let bounds = *self.canvas_bounds.lock().unwrap();
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let mx = f32::from(position.x - bounds.origin.x);
        let my = f32::from(position.y - bounds.origin.y);
        let mut best: Option<(usize, f32)> = None;
        for (i, emitter) in self.config.emitters.iter().enumerate() {
            let (fx, fy) = emitter.center();
            let (dx, dy) = (fx * w - mx, fy * h - my);
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= GRAB_RADIUS && best.is_none_or(|(_, d)| dist < d) {
                best = Some((i, dist));
            }
        }
        self.drag = best.map(|(i, _)| i);
        if self.drag.is_some() {
            cx.notify();
        }
    }

    fn editor_drag(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(index) = self.drag else { return };
        let bounds = *self.canvas_bounds.lock().unwrap();
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        if let Some(emitter) = self.config.emitters.get_mut(index) {
            emitter.x = (f32::from(position.x - bounds.origin.x) / w).clamp(0.0, 1.0);
            emitter.y = (f32::from(position.y - bounds.origin.y) / h).clamp(0.0, 1.0);
            cx.notify();
        }
    }

    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let toggles: Vec<ConfigToggle> = vec![
            (
                rox_i18n::t!("particles-round-particles"),
                |this| this.config.scene.round,
                |this| this.config.scene.round = !this.config.scene.round,
            ),
            (
                rox_i18n::t!("particles-glow"),
                |this| this.config.scene.glow,
                |this| this.config.scene.glow = !this.config.scene.glow,
            ),
            (
                rox_i18n::t!("particles-hold-on-pause"),
                |this| this.config.scene.freeze,
                |this| this.config.scene.freeze = !this.config.scene.freeze,
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
}

/// The value edit is the panel-wide one, so a route slider and an emitter
/// slider never type at once.
impl SignalHost for ParticlesPanel {
    fn hub(&self) -> &Arc<SignalHub> {
        &self.state.signals
    }

    fn routes(&self) -> &[Route] {
        &self.config.routes
    }

    fn signal_ui(&self) -> &SignalUi {
        &self.signal_ui
    }

    fn signal_ui_mut(&mut self) -> &mut SignalUi {
        &mut self.signal_ui
    }

    fn value_edit(&self) -> &ValueEdit {
        &self.value_edit
    }
}

impl RouteHost for ParticlesPanel {
    fn routes_mut(&mut self) -> &mut Vec<Route> {
        &mut self.config.routes
    }
}

impl PanelSettings for ParticlesPanel {
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
        // No Signals page: the pool is app-wide and has its own window.
        &[
            ("Emitters", icons::AUDIO_LINES),
            ("Forces", icons::MOVE),
            ("Scene", icons::GLOBE),
        ]
    }

    fn page(
        &mut self,
        page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Synced here because any page can host a route's rows: a route made
        // from a Forces row needs its scrubs on the next render.
        signal_ui::sync(self);
        match page {
            "Forces" => self.forces_page(cx).into_any_element(),
            "Scene" => self.scene_page(cx).into_any_element(),
            _ => self.emitters_page(window, cx).into_any_element(),
        }
    }

    /// Hold on Pause lives on the shared Behavior page with every other
    /// panel's behavior switches.
    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(
            section(
                rox_i18n::t!("viz-section-playback"),
                None,
                setting_row(
                    rox_i18n::t!("particles-hold-on-pause"),
                    Some(rox_i18n::t!("particles-hold-on-pause.description")),
                    toggle(
                        self.config.scene.freeze,
                        |this: &mut Self, on, cx| {
                            this.config.scene.freeze = on;
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

impl ParticlesPanel {
    fn emitters_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.sync_emitter_state(window, cx);
        let count = self.config.emitters.len();
        let add = settings_ui::small_button(
            rox_i18n::t!("particles-add-emitter"),
            icons::PLUS,
            false,
            cx.listener(|this, _, _, cx| this.add_emitter(cx)),
        );
        let mut list = div().flex().flex_col().gap(tokens::SPACE_MD);
        if count == 0 {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("particles-emitters-empty")),
            );
        }
        for i in 0..count {
            list = list.child(self.emitter_block(i, cx));
        }
        div().flex().flex_col().gap(SECTION_GAP).child(section(
            rox_i18n::t!("particles-section-emitters"),
            Some(add.into_any_element()),
            list,
        ))
    }

    /// The pickers rebuild whole on a count change: their subscriptions write
    /// back by index.
    fn sync_emitter_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.config.emitters.len();
        if self.emitter_scrubs.len() != count {
            self.emitter_scrubs
                .resize_with(count, EmitterScrubs::default);
        }
        if self.emitter_pickers.len() == count {
            return;
        }
        self.emitter_pickers.clear();
        self._emitter_changes.clear();
        for i in 0..count {
            let seed = self.config.emitters[i].color();
            let picker = cx.new(|cx| ColorPickerState::new(window, cx).default_value(seed));
            let sub = cx.subscribe_in(
                &picker,
                window,
                move |this: &mut Self, _, event: &ColorPickerEvent, _, cx| {
                    let ColorPickerEvent::Change(color) = event;
                    if let Some(color) = color {
                        if let Some(emitter) = this.config.emitters.get_mut(i) {
                            emitter.color = Some(palette::to_hex(Rgba::from(*color)));
                        }
                        cx.notify();
                    }
                },
            );
            self._emitter_changes.push(sub);
            self.emitter_pickers.push(picker);
        }
    }

    fn emitter_block(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let emitter = &self.config.emitters[index];
        let scrubs = &self.emitter_scrubs[index];
        let rate = emitter.rate();
        let size = emitter.size();
        let life = emitter.life();
        let shape = emitter.shape;
        let (x, y) = emitter.center();
        let width = emitter.width();
        let height = emitter.height();
        let rotation = emitter.rotation.rem_euclid(360.0);
        let aim = emitter.aim;
        let direction = emitter.direction.rem_euclid(360.0);
        let cone = emitter.cone();
        let speed = emitter.speed();
        let mode = emitter.mode;
        let burst = emitter.burst();
        let eid = emitter.id;

        let header = settings_ui::block_header(
            div().text_xs().text_color(palette::text_muted()).child(
                rox_i18n::t!("particles-emitter-label", index = (index + 1) as u64).to_string(),
            ),
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(toggle(
                    emitter.enabled,
                    move |this: &mut Self, on, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.enabled = on;
                        }
                        cx.notify();
                    },
                    cx,
                ))
                .child(settings_ui::icon_button(
                    icons::TRASH,
                    false,
                    cx.listener(move |this, _, _, cx| this.remove_emitter(index, cx)),
                )),
        );

        // The first pick forks off the accent; the reset follows it again.
        let mut color_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(ColorPicker::new(&self.emitter_pickers[index]).small());
        if emitter.color.is_some() {
            color_row = color_row.child(settings_ui::icon_button(
                icons::REFRESH_CW,
                false,
                cx.listener(move |this, _, window, cx| {
                    if let Some(emitter) = this.config.emitters.get_mut(index) {
                        emitter.color = None;
                    }
                    let accent = palette::accent();
                    this.emitter_pickers[index]
                        .update(cx, |picker, cx| picker.set_value(accent, window, cx));
                    cx.notify();
                }),
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(header)
            .child(setting_row(
                rox_i18n::t!("particles-trigger"),
                None,
                panel::choices_shared(
                    &trigger_choices(),
                    mode,
                    move |this: &mut Self, mode, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.mode = mode;
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(mode == Trigger::Continuous, |d| {
                d.child(signal_ui::bindable_row(
                    self,
                    rox_i18n::t!("particles-rate"),
                    None,
                    format!("e{eid}.rate"),
                    panel::value_slider_edit(
                        &scrubs.rate,
                        &self.value_edit,
                        (rate - RATE_MIN) / (RATE_MAX - RATE_MIN),
                        format!("{}/s", rox_i18n::format::format_int(rate.round() as i64)),
                        format!("{rate:.0}"),
                        |v| (v - RATE_MIN) / (RATE_MAX - RATE_MIN),
                        move |this: &mut Self, fraction, cx| {
                            if let Some(emitter) = this.config.emitters.get_mut(index) {
                                emitter.rate = RATE_MIN + fraction * (RATE_MAX - RATE_MIN);
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                    cx,
                ))
            })
            .when(mode == Trigger::Burst, |d| {
                d.child(signal_ui::bindable_row(
                    self,
                    rox_i18n::t!("particles-burst"),
                    None,
                    format!("e{eid}.burst"),
                    panel::value_slider_edit(
                        &scrubs.burst,
                        &self.value_edit,
                        (burst - BURST_MIN) / (BURST_MAX - BURST_MIN),
                        rox_i18n::format::format_int(burst.round() as i64),
                        format!("{burst:.0}"),
                        |v| (v - BURST_MIN) / (BURST_MAX - BURST_MIN),
                        move |this: &mut Self, fraction, cx| {
                            if let Some(emitter) = this.config.emitters.get_mut(index) {
                                emitter.burst = BURST_MIN + fraction * (BURST_MAX - BURST_MIN);
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                    cx,
                ))
            })
            .child(setting_row(
                rox_i18n::t!("particles-shape"),
                None,
                panel::choices_shared(
                    &shape_choices(),
                    shape,
                    move |this: &mut Self, shape, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.shape = shape;
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("particles-position-x"),
                None,
                panel::value_slider_edit(
                    &scrubs.x,
                    &self.value_edit,
                    x,
                    rox_i18n::format::format_percent((x * 100.0).round() as f64),
                    format!("{}", (x * 100.0).round() as i32),
                    |v| v / 100.0,
                    move |this: &mut Self, fraction, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.x = fraction.clamp(0.0, 1.0);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("particles-position-y"),
                None,
                panel::value_slider_edit(
                    &scrubs.y,
                    &self.value_edit,
                    y,
                    rox_i18n::format::format_percent((y * 100.0).round() as f64),
                    format!("{}", (y * 100.0).round() as i32),
                    |v| v / 100.0,
                    move |this: &mut Self, fraction, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.y = fraction.clamp(0.0, 1.0);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(shape != Shape::Point, |d| {
                d.child(setting_row(
                    match shape {
                        Shape::Ring => rox_i18n::t!("particles-radius"),
                        Shape::Box => rox_i18n::t!("particles-width"),
                        _ => rox_i18n::t!("particles-length"),
                    },
                    None,
                    panel::value_slider_edit(
                        &scrubs.width,
                        &self.value_edit,
                        width / 2.0,
                        rox_i18n::format::format_percent((width * 100.0).round() as f64),
                        format!("{}", (width * 100.0).round() as i32),
                        |v| v / 200.0,
                        move |this: &mut Self, fraction, cx| {
                            if let Some(emitter) = this.config.emitters.get_mut(index) {
                                emitter.width = (fraction * 2.0).clamp(0.0, 2.0);
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .when(shape == Shape::Box, |d| {
                d.child(setting_row(
                    rox_i18n::t!("particles-height"),
                    None,
                    panel::value_slider_edit(
                        &scrubs.height,
                        &self.value_edit,
                        height / 2.0,
                        rox_i18n::format::format_percent((height * 100.0).round() as f64),
                        format!("{}", (height * 100.0).round() as i32),
                        |v| v / 200.0,
                        move |this: &mut Self, fraction, cx| {
                            if let Some(emitter) = this.config.emitters.get_mut(index) {
                                emitter.height = (fraction * 2.0).clamp(0.0, 2.0);
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .when(matches!(shape, Shape::Line | Shape::Box), |d| {
                d.child(setting_row(
                    rox_i18n::t!("particles-rotation"),
                    None,
                    panel::value_slider_edit(
                        &scrubs.rotation,
                        &self.value_edit,
                        rotation / 360.0,
                        format!("{}°", rox_i18n::format::format_int(rotation.round() as i64)),
                        format!("{rotation:.0}"),
                        |v| v.rem_euclid(360.0) / 360.0,
                        move |this: &mut Self, fraction, cx| {
                            if let Some(emitter) = this.config.emitters.get_mut(index) {
                                emitter.rotation = fraction.clamp(0.0, 1.0) * 360.0;
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(setting_row(
                rox_i18n::t!("particles-aim"),
                None,
                panel::choices_shared(
                    &aim_choices(),
                    aim,
                    move |this: &mut Self, aim, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.aim = aim;
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(aim == Aim::Fixed, |d| {
                d.child(setting_row(
                    rox_i18n::t!("particles-direction"),
                    None,
                    panel::value_slider_edit(
                        &scrubs.direction,
                        &self.value_edit,
                        direction / 360.0,
                        format!(
                            "{}°",
                            rox_i18n::format::format_int(direction.round() as i64)
                        ),
                        format!("{direction:.0}"),
                        |v| v.rem_euclid(360.0) / 360.0,
                        move |this: &mut Self, fraction, cx| {
                            if let Some(emitter) = this.config.emitters.get_mut(index) {
                                emitter.direction = fraction.clamp(0.0, 1.0) * 360.0;
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(signal_ui::bindable_row(
                self,
                rox_i18n::t!("particles-cone"),
                None,
                format!("e{eid}.cone"),
                panel::value_slider_edit(
                    &scrubs.cone,
                    &self.value_edit,
                    cone / 360.0,
                    format!("{}°", rox_i18n::format::format_int(cone.round() as i64)),
                    format!("{cone:.0}"),
                    |v| v / 360.0,
                    move |this: &mut Self, fraction, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.cone = fraction.clamp(0.0, 1.0) * 360.0;
                        }
                        cx.notify();
                    },
                    cx,
                ),
                cx,
            ))
            .child(signal_ui::bindable_row(
                self,
                rox_i18n::t!("particles-speed"),
                None,
                format!("e{eid}.speed"),
                panel::value_slider_edit(
                    &scrubs.speed,
                    &self.value_edit,
                    (speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN),
                    format!(
                        "{} px/s",
                        rox_i18n::format::format_int(speed.round() as i64)
                    ),
                    format!("{speed:.0}"),
                    |v| (v - SPEED_MIN) / (SPEED_MAX - SPEED_MIN),
                    move |this: &mut Self, fraction, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.speed = SPEED_MIN + fraction * (SPEED_MAX - SPEED_MIN);
                        }
                        cx.notify();
                    },
                    cx,
                ),
                cx,
            ))
            .child(signal_ui::bindable_row(
                self,
                rox_i18n::t!("particles-size"),
                None,
                format!("e{eid}.size"),
                panel::value_slider_edit(
                    &scrubs.size,
                    &self.value_edit,
                    (size - SIZE_MIN) / (SIZE_MAX - SIZE_MIN),
                    format!("{} px", rox_i18n::format::format_int(size.round() as i64)),
                    format!("{size:.0}"),
                    |v| (v - SIZE_MIN) / (SIZE_MAX - SIZE_MIN),
                    move |this: &mut Self, fraction, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.size = SIZE_MIN + fraction * (SIZE_MAX - SIZE_MIN);
                        }
                        cx.notify();
                    },
                    cx,
                ),
                cx,
            ))
            .child(signal_ui::bindable_row(
                self,
                rox_i18n::t!("particles-lifetime"),
                None,
                format!("e{eid}.life"),
                panel::value_slider_edit(
                    &scrubs.life,
                    &self.value_edit,
                    (life - LIFE_MIN) / (LIFE_MAX - LIFE_MIN),
                    rox_i18n::format::format_unit(f64::from(life), 1, "s"),
                    format!("{life:.1}"),
                    |v| (v - LIFE_MIN) / (LIFE_MAX - LIFE_MIN),
                    move |this: &mut Self, fraction, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.life = LIFE_MIN + fraction * (LIFE_MAX - LIFE_MIN);
                        }
                        cx.notify();
                    },
                    cx,
                ),
                cx,
            ))
            .child(setting_row(
                rox_i18n::t!("particles-color"),
                None,
                color_row,
            ))
    }

    fn forces_page(&mut self, cx: &mut Context<Self>) -> Div {
        let turbulence = self.config.forces.turbulence();
        let scale = self.config.forces.scale();
        let speed = self.config.forces.speed();
        let strength_slider = panel::value_slider_edit(
            &self.turbulence_scrub,
            &self.value_edit,
            turbulence / TURB_MAX,
            rox_i18n::format::format_int(turbulence.round() as i64),
            format!("{turbulence:.0}"),
            |v| v / TURB_MAX,
            |this: &mut Self, fraction, cx| {
                this.config.forces.turbulence = fraction * TURB_MAX;
                cx.notify();
            },
            cx,
        );
        let scale_slider = panel::value_slider_edit(
            &self.turb_scale_scrub,
            &self.value_edit,
            (scale - TURB_SCALE_MIN) / (TURB_SCALE_MAX - TURB_SCALE_MIN),
            format!("{} px", rox_i18n::format::format_int(scale.round() as i64)),
            format!("{scale:.0}"),
            |v| (v - TURB_SCALE_MIN) / (TURB_SCALE_MAX - TURB_SCALE_MIN),
            |this: &mut Self, fraction, cx| {
                this.config.forces.turbulence_scale =
                    TURB_SCALE_MIN + fraction * (TURB_SCALE_MAX - TURB_SCALE_MIN);
                cx.notify();
            },
            cx,
        );
        let drift_slider = panel::value_slider_edit(
            &self.turb_speed_scrub,
            &self.value_edit,
            speed / TURB_SPEED_MAX,
            rox_i18n::format::format_float(f64::from(speed), 2),
            format!("{speed:.2}"),
            |v| v / TURB_SPEED_MAX,
            |this: &mut Self, fraction, cx| {
                this.config.forces.turbulence_speed = fraction * TURB_SPEED_MAX;
                cx.notify();
            },
            cx,
        );
        div().flex().flex_col().gap(SECTION_GAP).child(section(
            rox_i18n::t!("particles-turbulence"),
            None,
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(signal_ui::bindable_row(
                    self,
                    rox_i18n::t!("particles-turbulence-strength"),
                    Some(rox_i18n::t!("particles-turbulence-strength.description")),
                    "turbulence".to_string(),
                    strength_slider,
                    cx,
                ))
                .child(signal_ui::bindable_row(
                    self,
                    rox_i18n::t!("particles-scale"),
                    Some(rox_i18n::t!("particles-scale.description")),
                    "scale".to_string(),
                    scale_slider,
                    cx,
                ))
                .child(signal_ui::bindable_row(
                    self,
                    rox_i18n::t!("particles-drift"),
                    Some(rox_i18n::t!("particles-drift.description")),
                    "drift".to_string(),
                    drift_slider,
                    cx,
                )),
        ))
    }

    fn scene_page(&mut self, cx: &mut Context<Self>) -> Div {
        let gravity = self.config.scene.gravity();
        let angle = self.config.scene.gravity_angle.rem_euclid(360.0);
        let drag = self.config.scene.drag();
        let gravity_slider = panel::value_slider_edit(
            &self.gravity_scrub,
            &self.value_edit,
            gravity / GRAVITY_MAX,
            rox_i18n::format::format_int(gravity.round() as i64),
            format!("{gravity:.0}"),
            |v| v / GRAVITY_MAX,
            |this: &mut Self, fraction, cx| {
                this.config.scene.gravity = fraction * GRAVITY_MAX;
                cx.notify();
            },
            cx,
        );
        let angle_slider = panel::value_slider_edit(
            &self.gravity_angle_scrub,
            &self.value_edit,
            angle / 360.0,
            format!("{}°", rox_i18n::format::format_int(angle.round() as i64)),
            format!("{angle:.0}"),
            |v| v.rem_euclid(360.0) / 360.0,
            |this: &mut Self, fraction, cx| {
                this.config.scene.gravity_angle = fraction.clamp(0.0, 1.0) * 360.0;
                cx.notify();
            },
            cx,
        );
        let drag_slider = panel::value_slider_edit(
            &self.drag_scrub,
            &self.value_edit,
            drag / DRAG_MAX,
            rox_i18n::format::format_float(f64::from(drag), 2),
            format!("{drag:.2}"),
            |v| v / DRAG_MAX,
            |this: &mut Self, fraction, cx| {
                this.config.scene.drag = fraction * DRAG_MAX;
                cx.notify();
            },
            cx,
        );
        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(
                rox_i18n::t!("particles-gravity"),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(signal_ui::bindable_row(
                        self,
                        rox_i18n::t!("particles-gravity-strength"),
                        Some(rox_i18n::t!("particles-gravity-strength.description")),
                        "gravity".to_string(),
                        gravity_slider,
                        cx,
                    ))
                    .child(setting_row(
                        rox_i18n::t!("particles-direction"),
                        Some(rox_i18n::t!("particles-direction.description")),
                        angle_slider,
                    )),
            ))
            .child(section(
                rox_i18n::t!("particles-section-medium"),
                None,
                signal_ui::bindable_row(
                    self,
                    rox_i18n::t!("particles-drag"),
                    Some(rox_i18n::t!("particles-drag.description")),
                    "drag".to_string(),
                    drag_slider,
                    cx,
                ),
            ))
            .child(section(
                rox_i18n::t!("particles-section-particles"),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(setting_row(
                        rox_i18n::t!("particles-round-particles"),
                        Some(rox_i18n::t!("particles-round-particles.description")),
                        toggle(
                            self.config.scene.round,
                            |this: &mut Self, on, cx| {
                                this.config.scene.round = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("particles-glow"),
                        Some(rox_i18n::t!("particles-glow.description")),
                        toggle(
                            self.config.scene.glow,
                            |this: &mut Self, on, cx| {
                                this.config.scene.glow = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    )),
            ))
    }
}

impl EventEmitter<PanelEvent> for ParticlesPanel {}

impl Focusable for ParticlesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for ParticlesPanel {
    fn panel_name(&self) -> &'static str {
        "particles"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-particles"),
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
        // With an icon, so it lines up with Rename and the tick sits right. The
        // icon-less form is for flyouts.
        let menu = menu.item(panel::check_row(
            rox_i18n::t!("particles-edit-emitters"),
            Some(icons::MOVE),
            |this: &Self| this.edit,
            |this, _| {
                this.edit = !this.edit;
                this.drag = None;
            },
            &cx.entity(),
        ));
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
                ParticlesPanel::new(state, config, cx)
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

impl Render for ParticlesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl ParticlesPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The observe re-renders on every pump tick while audio moves. Frame
        // polling only runs the particles still in the air, then the panel parks.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        // Paused mid-session, not a played-out queue.
        let hold = self.config.scene.freeze && session && !playing && !player.queue_ended();
        if !playing && self.sim.lock().unwrap().alive {
            window.request_animation_frame();
        }

        let config = self.config.clone();
        let sim = self.sim.clone();
        let hub = self.state.signals.clone();
        let edit = self.edit;
        let drag = self.drag;
        let canvas_bounds = self.canvas_bounds.clone();
        let mut root = div().size_full().relative().bg(palette::bg_root()).child(
            canvas(
                move |bounds, _, _| {
                    *canvas_bounds.lock().unwrap() = bounds;
                },
                move |bounds, _, window, _| {
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    if w <= 0.0 || h <= 0.0 {
                        return;
                    }
                    let mut sim = sim.lock().unwrap();
                    sim.step(&hub, w, h, &config, hold);
                    sim.paint(bounds, window, &config.scene);
                    if edit {
                        paint_markers(&config, drag, bounds, window);
                    }
                },
            )
            .size_full(),
        );
        // Press near a center to grab, drag to place. The markers paint in the
        // same canvas, against the live field.
        if edit {
            root = root
                .cursor_grab()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.editor_grab(event.position, cx)
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    this.editor_drag(event.position, cx)
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.drag = None;
                        cx.notify();
                    }),
                );
        }
        root
    }
}
