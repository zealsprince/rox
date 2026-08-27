//! The particles panel: a field of emitters, made musical by routing the
//! app's shared signals onto its knobs. An emitter itself is pure geometry
//! and throw (a point, a line, a box, or a ring, placed anywhere, with its
//! own size, life, and speed); unbound it fountains at its sliders,
//! independent of the music. Reactivity is all routes: bind a kick signal
//! to a rate and the field breathes, put a burst emitter on an onset
//! signal and it pops per hit, and the same signal can drive gravity,
//! turbulence, or another panel entirely, since the pool is app-wide and
//! evaluated once per frame in the [`SignalHub`]. The scene's gravity and
//! drag pull on everything in flight; the force field adds drift on top.
//! Everything is paint primitives on the UI thread, and once the last
//! particle dies the panel stops asking for frames.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, point, prelude::*, px, size, AnyElement, App, BorderStyle, Bounds, Context, Div,
    Entity, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Rgba, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Sizable as _;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::signal::{Route, SignalHub};
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit,
};
use crate::panel_settings;
use crate::settings::ui::{self as settings_ui, section, SECTION_GAP};
use crate::signal_ui::{self, RouteHost, RouteTargets, SignalHost, SignalUi};

/// Where a burst emitter's routed signal reads as a hit and where it
/// re-arms, with hysteresis between so one swell can't stutter-fire.
const BURST_FIRE: f32 = 0.6;
const BURST_REARM: f32 = 0.3;

/// The ceiling on live particles. A pinned emitter at the top rate would
/// run away over a long track otherwise; past this, spawns are dropped
/// until the older ones age out.
const MAX_PARTICLES: usize = 4000;

/// How far outside the panel a particle may drift before it's culled, as
/// a fraction of the panel's larger side with a floor in px. Generous
/// enough that one thrown past the edge can still arc back under gravity.
const CULL_MARGIN: f32 = 0.25;
const CULL_MARGIN_MIN: f32 = 64.0;

/// The emission rate slider's span, particles per second. The floor is
/// zero because the rate is the whole story now that emitters have no
/// threshold of their own: a route resting at its Quiet end has to be
/// able to stop the emitter outright, and a hand-dragged rate gets to
/// silence the same way.
const RATE_MIN: f32 = 0.0;
const RATE_MAX: f32 = 300.0;

/// The launch speed slider's span, px per second.
const SPEED_MIN: f32 = 0.0;
const SPEED_MAX: f32 = 600.0;

/// The burst slider's span, particles thrown per onset when an emitter
/// fires on transients instead of a steady rate.
const BURST_MIN: f32 = 1.0;
const BURST_MAX: f32 = 120.0;

/// The scene gravity slider's span, px per second squared.
const GRAVITY_MAX: f32 = 900.0;

/// The drag slider's span, per second: how much of a particle's speed the
/// medium eats each second. Zero is a vacuum.
const DRAG_MAX: f32 = 4.0;

/// The turbulence sliders' spans: how hard the field pushes (px per second
/// squared), how wide one swirl runs (px), and how fast the field drifts.
const TURB_MAX: f32 = 600.0;
const TURB_SCALE_MIN: f32 = 40.0;
const TURB_SCALE_MAX: f32 = 600.0;
const TURB_SPEED_MAX: f32 = 2.0;

/// The lifetime slider's span, seconds.
const LIFE_MIN: f32 = 0.2;
const LIFE_MAX: f32 = 6.0;

/// The particle size slider's span, px.
const SIZE_MIN: f32 = 1.0;
const SIZE_MAX: f32 = 16.0;

/// The footprint an emitter spawns across. A line along an edge aimed
/// outward is the plain visualizer look; a point or a ring is the burst
/// the edge-bound version could never do.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Point,
    #[default]
    Line,
    Box,
    Ring,
}

/// Where a particle heads when it spawns: the emitter's fixed angle, or
/// away from the emitter's center. Outward makes a ring burst and a point
/// spray in every direction.
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

/// How an emitter turns activation into spawns: a steady stream scaled by
/// how hard it fires, or a burst on each onset so a kick pops in one puff
/// instead of dribbling while the hit sustains.
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

/// One scene or force knob a route may drive: the persisted id, the label
/// the target list shows, and how a route's factor applies to it. The
/// factor scales the knob's own setting, so the slider stays the
/// reference a route works against rather than going dead once bound, and
/// a knob set to zero is off, route or no route. The ids only ever grow;
/// a config with an unknown one goes quiet rather than misfiring.
/// Making a new value bindable is one entry here plus wrapping its
/// settings row in [`crate::signal_ui::bindable_row`].
struct BindTarget {
    id: &'static str,
    /// A rox-i18n key, resolved at the point of use rather than here: a
    /// `const` array can't hold the locale-dependent `SharedString` `t!`
    /// returns.
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

/// [`BindTarget`]'s per-emitter counterpart: the knob id is part of an
/// `e<id>.<knob>` target against the emitter's stable id, and the factor
/// scales that emitter's own setting.
struct EmitterBindTarget {
    id: &'static str,
    /// See [`BindTarget::label_key`].
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

/// One emitter: pure geometry and throw. It has no audio of its own;
/// reactivity arrives by routing pool signals onto its knobs, and unbound
/// it just runs at its sliders, a fountain independent of the music.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Emitter {
    /// A stable handle routes point at, unique within the panel and
    /// persisted, so a route holds up when removals shift the list under it.
    /// 0 is unassigned; the panel assigns on load and on add.
    pub id: u64,
    /// Whether the emitter fires. Off keeps it in the list, tuned, silent.
    pub enabled: bool,
    /// Particles per second in continuous mode.
    pub rate: f32,
    /// Whether the emitter streams at `rate` or pops a `burst` when its
    /// routed signal rises. Burst without a route on the burst knob stays
    /// silent: the route is the trigger.
    pub mode: Trigger,
    /// Particles thrown per pop in burst mode.
    pub burst: f32,
    /// Particle size, px, and lifetime, seconds, varied a little per
    /// particle. Per emitter, so bass smoke and hat sparks coexist.
    pub size: f32,
    pub life: f32,
    /// The footprint particles spawn across.
    pub shape: Shape,
    /// The footprint's center, as fractions of the panel, so a resize
    /// keeps the arrangement instead of scattering it.
    pub x: f32,
    pub y: f32,
    /// The footprint's extent, as fractions of the panel. A line uses
    /// `width` as its length, a box uses both, a ring uses `width` as its
    /// radius, and a point uses neither.
    pub width: f32,
    pub height: f32,
    /// The footprint's rotation, degrees clockwise. A line at 0 runs
    /// horizontally. Rings and points ignore it.
    pub rotation: f32,
    /// Whether particles follow the fixed angle below or head away from
    /// the emitter's center.
    pub aim: Aim,
    /// The launch angle, degrees clockwise from up.
    pub direction: f32,
    /// The spread around the launch angle, degrees: 0 is a beam, 360
    /// throws every way.
    pub cone: f32,
    /// Launch speed, px per second, varied a little per particle and
    /// leaned on by how hard the emitter is firing.
    pub speed: f32,
    /// The particles' color, `#rrggbb`. None follows the theme accent, so
    /// an emitter left alone tracks song theming.
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

    /// The footprint's center and extent, clamped so a hand-edited file
    /// can't park an emitter off the panel or invert its size.
    fn center(&self) -> (f32, f32) {
        (self.x.clamp(0.0, 1.0), self.y.clamp(0.0, 1.0))
    }

    fn width(&self) -> f32 {
        self.width.clamp(0.0, 2.0)
    }

    fn height(&self) -> f32 {
        self.height.clamp(0.0, 2.0)
    }

    /// The emitter's color, falling back to the accent when unset and when
    /// a hand-edited hex doesn't parse.
    fn color(&self) -> Rgba {
        self.color
            .as_deref()
            .and_then(palette::parse_hex)
            .unwrap_or_else(palette::accent)
    }

    /// A fresh emitter for the Add button: the last one's look, nudged off
    /// its spot so the two don't paint as one.
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

/// The scene: the settings the whole field runs in, apart from any one emitter.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    /// Constant pull on everything in flight, px per second squared, and
    /// the direction it pulls, degrees clockwise from up. 180 is down.
    pub gravity: f32,
    pub gravity_angle: f32,
    /// How much speed the medium eats per second. Zero is a vacuum.
    pub drag: f32,
    /// Draw particles as dots rather than squares.
    pub round: bool,
    /// Lay a soft halo behind each particle so it reads as light rather than
    /// a flat chip.
    pub glow: bool,
    /// Freeze the field while playback is paused instead of letting it
    /// drift out.
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

/// The force field laid over the scene: drift that varies across the panel
/// rather than pulling one way. Where attractors would join.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Forces {
    /// How hard the field pushes, px per second squared. Zero is off.
    pub turbulence: f32,
    /// How wide one swirl runs, px: small values churn, large ones roll.
    pub turbulence_scale: f32,
    /// How fast the field itself drifts, so the swirls don't stand still.
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

/// The particles panel's per-view config: what a saved layout restores and
/// what the customize window edits. Missing fields take the defaults, so a
/// layout dumped before a field existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParticlesConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The emitters, in the order the customize window lists them.
    pub emitters: Vec<Emitter>,
    /// Attachments of the app's shared signals onto this panel's knobs. A
    /// route whose signal is gone from the pool goes quiet, never wrong.
    pub routes: Vec<Route>,
    pub scene: Scene,
    pub forces: Forces,
}

impl Default for ParticlesConfig {
    /// A fresh panel gets one emitter rather than an empty field, so it
    /// draws something the moment it's dropped into the dock. An emptied
    /// list is still respected: the layout dump writes `"emitters": []`,
    /// and only a config missing the field falls back to this.
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

/// Give every emitter a unique id, keeping the ones a loaded config
/// already has: zeroes (configs from before ids existed) and hand-edited
/// duplicates get fresh ones, and any binding that pointed at a replaced
/// id goes quiet rather than firing at the wrong emitter.
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

/// A binding target's emitter route, `e<id>.<knob>`, if it's one of those.
fn emitter_route(target: &str) -> Option<(u64, &str)> {
    let (id, knob) = target.strip_prefix('e')?.split_once('.')?;
    Some((id.parse().ok()?, knob))
}

/// The frame's working copies the routes write into, dispatching plain ids
/// through the scene and force table and `e<id>.<knob>` ids through the
/// emitter table. Unknown ids fall through quietly, the tables' contract.
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

/// Resolve the routes against the hub's live signals into the emitters,
/// scene, and forces this frame runs with.
fn modulated(config: &ParticlesConfig, hub: &SignalHub) -> (Vec<Emitter>, Scene, Forces) {
    let mut targets = Modulated {
        emitters: config.emitters.clone(),
        scene: config.scene.clone(),
        forces: config.forces.clone(),
    };
    signal_ui::apply_routes(&config.routes, hub, &mut targets);
    (targets.emitters, targets.scene, targets.forces)
}

/// The signal a burst emitter watches for its trigger: the last enabled
/// route onto its burst knob, matching the order [`modulated`] applies.
fn burst_signal(config: &ParticlesConfig, emitter_id: u64) -> Option<u64> {
    let target = format!("e{emitter_id}.burst");
    config
        .routes
        .iter()
        .rev()
        .find(|r| r.enabled && r.target == target)
        .map(|r| r.signal)
}

/// A heading in degrees clockwise from up as a unit vector in panel space,
/// where y runs down. 0 is up, 90 is right, 180 is down.
fn heading(degrees: f32) -> (f32, f32) {
    let r = degrees.to_radians();
    (r.sin(), -r.cos())
}

/// xorshift32. The field needs scatter, not statistics, and rolling it here
/// keeps the crate's dependency list where it is, the same call the FFT in
/// rox-viz makes.
fn rand01(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state >> 8) as f32 / (1u32 << 24) as f32
}

/// One lattice point of the turbulence field, hashed to 0..1.
fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1) ^ seed;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Smoothed value noise over that lattice, 0..1. Neighbouring particles
/// read nearly the same value, so the drift looks like wind rather than
/// per-particle jitter.
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

/// One live particle, in panel pixels.
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

/// Per-panel sim state, shared with the paint closure the way the spectrum
/// shares its bars: the entity holds the handle, the closure does the
/// per-frame work where the bounds are known. The audio analysis itself is
/// in the app's shared [`SignalHub`]; the sim only reads values.
struct Sim {
    last_tick: Option<Instant>,
    /// The fraction of a particle each emitter carried over from the last tick,
    /// so a slow rate still fires at its average instead of rounding to
    /// zero.
    carry: Vec<f32>,
    /// Whether each burst emitter is ready to fire again, re-armed once
    /// its routed signal falls back, so one rise throws one pop.
    armed: Vec<bool>,
    particles: Vec<Particle>,
    /// Seconds the sim has run, for drifting the turbulence field.
    clock: f32,
    rng: u32,
    /// Particles still on screen: render keeps requesting frames until this
    /// clears.
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

    /// One tick: advance the shared hub, resolve the routes into this
    /// frame's emitters and field, fire what they call for, and move what
    /// is already in the air. `hold` is the freeze-on-pause option, which
    /// parks the field where it stands.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        feed: &AudioFeed,
        hub: &SignalHub,
        track: Option<u64>,
        w: f32,
        h: f32,
        config: &ParticlesConfig,
        hold: bool,
    ) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32().min(0.1))
            .unwrap_or(1.0 / 60.0);
        self.last_tick = Some(now);

        if hold {
            // Frozen: the standing frame keeps painting, and nothing ages.
            self.alive = false;
            return;
        }
        self.clock += dt;

        hub.tick(feed, track);
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
                // Continuous runs at its rate, routed or not: an unbound
                // emitter is a fountain independent of the music.
                Trigger::Continuous => {
                    self.carry[i] += emitter.rate() * dt;
                    let due = self.carry[i].floor();
                    self.carry[i] -= due;
                    due as usize
                }
                // The pop fires on the routed signal's rising edge, with
                // hysteresis so one swell can't stutter-fire. No route on
                // the burst knob means no trigger at all.
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

    /// Launch one particle for an emitter: somewhere on its footprint,
    /// headed the way it aims, scattered enough that a steady emitter reads
    /// as a plume rather than a line.
    fn spawn(&mut self, emitter: &Emitter, w: f32, h: f32, color: Rgba) {
        let (fx, fy) = emitter.center();
        let (cx, cy) = (fx * w, fy * h);
        let rot = emitter.rotation.to_radians();
        // Where on the footprint the particle appears, and how far it is
        // from the center, the direction Outward aims along.
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

        // Outward heads away from the center; a particle sitting exactly on
        // it has no outward to speak of, so it takes a random heading and
        // the cone spreads from there.
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

    /// Move everything in flight one step, and drop what has aged out or
    /// drifted too far to come back. Retain keeps the order stable, so
    /// older particles paint under newer ones.
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
                // Two lookups off the same field, offset so the x and y
                // pushes don't march in lockstep.
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
            // Fade out over the back half of the life, so a particle dies
            // by dimming instead of blinking off mid-flight.
            let t = (p.age / p.life).clamp(0.0, 1.0);
            let fade = ((1.0 - t) * 2.0).min(1.0);
            // A dim, wide halo under the core uses the same fade, so a
            // particle glows out instead of blinking off.
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

/// How close to an emitter's center a press has to land to grab it in the
/// editor, px.
const GRAB_RADIUS: f32 = 24.0;

/// The editor overlay: every emitter's footprint dotted onto the field and
/// its center as the grab handle, in the emitter's own color so the markers
/// read against the settings list. Disabled emitters dim; the dragged one
/// swells. Dots are the one outline every shape can be drawn with under
/// axis-aligned quads, rotation included.
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
                    // Walk the perimeter as one 0..4 loop, a side per unit.
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

/// The settings sliders' painted bounds and drag state for one emitter, one
/// per slider so a drag on one never moves the others.
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

/// A labelled config toggle for the Display menu: the row label, a getter
/// for its current state, and a setter that flips it.
type ConfigToggle = (
    SharedString,
    fn(&ParticlesPanel) -> bool,
    fn(&mut ParticlesPanel),
);

pub struct ParticlesPanel {
    state: AppState,
    config: ParticlesConfig,
    feed: Arc<AudioFeed>,
    sim: Arc<Mutex<Sim>>,
    /// Per-emitter slider state, kept the same length as the list.
    emitter_scrubs: Vec<EmitterScrubs>,
    /// Per-emitter color pickers, built on the first settings render and
    /// rebuilt whenever the count changes: the panel itself constructs
    /// without a window, which the picker state needs, and a removed
    /// emitter shifts every index after it.
    emitter_pickers: Vec<Entity<ColorPickerState>>,
    _emitter_changes: Vec<Subscription>,
    /// The shared route and pool widgets' state, kept in step with the
    /// lists by [`signal_ui::sync`] on every settings render.
    signal_ui: SignalUi,
    gravity_scrub: ScrubState,
    gravity_angle_scrub: ScrubState,
    drag_scrub: ScrubState,
    turbulence_scrub: ScrubState,
    turb_scale_scrub: ScrubState,
    turb_speed_scrub: ScrubState,
    focus: FocusHandle,
    /// The one readout being typed into across all the settings sliders.
    value_edit: ValueEdit,
    /// The editor overlay: markers over the field for arranging emitters
    /// by hand. Session state, deliberately not persisted.
    edit: bool,
    /// The emitter following the pointer while the editor is on.
    drag: Option<usize>,
    /// The field canvas's painted bounds, for mapping editor presses into
    /// emitter fractions, the scrub strips' arrangement.
    canvas_bounds: Arc<Mutex<Bounds<Pixels>>>,
    /// The tab panel that currently hosts this panel, for duplicate and
    /// pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel when a session starts, so an idle window resumes
    /// animating without the player bar's frame pump.
    _player_changed: Subscription,
}

impl ParticlesPanel {
    pub fn new(state: AppState, mut config: ParticlesConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        assign_emitter_ids(&mut config.emitters);
        ParticlesPanel {
            config,
            feed: state.player.read(cx).feed(),
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
            focus: cx.focus_handle(),
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

    /// A press in the editor: pick the emitter whose center is nearest,
    /// within the grab radius, and let it follow the pointer.
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

    /// Carry the dragged emitter with the pointer, clamped to the panel.
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

    /// The panel's own dropdown entries: a Display flyout of the toggles
    /// the customize window also holds, for a quick flip without opening
    /// it.
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

/// The shared route and pool widgets read this panel through the trait:
/// its routes are per-view config, its widget state the embedded bundle,
/// and its value edit the panel-wide one so a route slider and an emitter
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

/// The routes are this view's own, unlike the pool they read from: two
/// particles panels bind their own knobs to the same signals.
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
        // No Signals page: the pool is app-wide, so it has a window of its
        // own, the app's signals window. What stays here is the binding,
        // which belongs to this panel's knobs.
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
        // Any page can host a route's tuning rows, so the route and signal
        // slider state syncs here: a route created from a Forces row must
        // find its scrubs on the very next render.
        signal_ui::sync(self);
        match page {
            "Forces" => self.forces_page(cx).into_any_element(),
            "Scene" => self.scene_page(cx).into_any_element(),
            _ => self.emitters_page(window, cx).into_any_element(),
        }
    }

    /// Hold on Pause sits on the shared Behavior page rather than on the
    /// Scene page: it's about how the panel acts when the audio stops, not
    /// what the scene looks like, and that's where every other panel keeps
    /// its behavior switches.
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
    /// The Emitters page: the list, each emitter a block of its own rows.
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

    /// Keep the per-emitter slider and picker state in step with the list.
    /// The pickers are rebuilt whole on a count change: their subscriptions
    /// write back by index, and a removal shifts every index after it.
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

    /// One emitter's block: the header carrying its switch and delete, then
    /// the trigger, the footprint, and the throw.
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

        // The color row forks off the accent on the first pick, and takes an
        // inline reset back to following it, the panel settings window's
        // pattern for a knob that inherits until it doesn't.
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

    /// The Forces page: the drift laid over the scene's steady pull. Every
    /// knob here is a binding target, so each row has the bind toggle.
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

    /// The Scene page: the settings the whole field runs in, apart from any
    /// one emitter.
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

    /// The layout dump stores the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
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
        // Icon on the row so it lines up with Rename and the rest of the tail
        // and the tick is on the right, the way every other top-level
        // check row in the app reads. The icon-less form is for flyouts.
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
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl ParticlesPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // While audio moves, the player observe re-renders on every pump
        // tick, which is the only rate new samples arrive at. Frame polling
        // is for the particles still in the air after playback stops; once
        // the last one dies the panel parks, and a resume wakes it through
        // the pump's play-state notify.
        let player = self.state.player.read(cx);
        let session = player.now_playing().is_some();
        let playing = player.is_playing();
        // Read here rather than in the paint closure, which has no cx: the
        // hub needs it to spot a song change for the aggregates that reset
        // on one, and a render happens every frame audio moves.
        let track = player.playing_entry();
        // Freeze on pause holds the standing field: paused mid-session, not
        // a played-out queue.
        let hold = self.config.scene.freeze && session && !playing && !player.queue_ended();
        if !playing && self.sim.lock().unwrap().alive {
            window.request_animation_frame();
        }

        let config = self.config.clone();
        let sim = self.sim.clone();
        let feed = self.feed.clone();
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
                    sim.step(&feed, &hub, track, w, h, &config, hold);
                    sim.paint(bounds, window, &config.scene);
                    if edit {
                        paint_markers(&config, drag, bounds, window);
                    }
                },
            )
            .size_full(),
        );
        // The editor runs on the panel itself: press near a center to grab,
        // drag to place, release to drop. The markers paint in the same
        // canvas, so arranging happens against the live field.
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
