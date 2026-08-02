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
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::signal::{Route, Signal, SignalHub, Source};
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit,
};
use crate::panel_settings;
use crate::settings::ui::{self as settings_ui, section, SECTION_GAP};

/// The frequency band an emitter's trigger may pick between, and the
/// smallest span it keeps between its bounds: tight enough for a kick,
/// wide enough that the mapping never inverts.
const SLIDER_MIN_HZ: f32 = 20.0;
const SLIDER_MAX_HZ: f32 = 20_000.0;
const MIN_RATIO: f32 = 1.2;

/// How far past its own setting a route may push a knob: the span reads
/// as a share of what the slider says, and a route is allowed to overshoot
/// it before the knob's own range clamps the result.
const SPAN_OVER: f32 = 4.0;

/// Where a burst emitter's routed signal reads as a hit and where it
/// re-arms, with hysteresis between so one swell can't stutter-fire.
const BURST_FIRE: f32 = 0.6;
const BURST_REARM: f32 = 0.3;

/// The ceiling on live particles. A pinned emitter at the top rate would
/// run away over a long track otherwise; past this, spawns are dropped
/// until the older ones age out.
const MAX_PARTICLES: usize = 4000;

/// How far outside the panel a particle may drift before it is culled, as
/// a fraction of the panel's larger side with a floor in px. Generous
/// enough that one thrown past the edge can still arc back under gravity.
const CULL_MARGIN: f32 = 0.25;
const CULL_MARGIN_MIN: f32 = 64.0;

/// The emission rate slider's span, particles per second. The floor is
/// zero because the rate is the whole story now that emitters carry no
/// threshold of their own: a route resting at its Quiet end has to be
/// able to stop the emitter outright, and a hand-dragged rate reaches
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
/// away from the emitter's center. Outward is what makes a ring burst and
/// a point spray in every direction.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Aim {
    #[default]
    Fixed,
    Outward,
}

const SHAPE_CHOICES: &[(&str, Shape)] = &[
    ("Point", Shape::Point),
    ("Line", Shape::Line),
    ("Box", Shape::Box),
    ("Ring", Shape::Ring),
];

const AIM_CHOICES: &[(&str, Aim)] = &[("Fixed", Aim::Fixed), ("Outward", Aim::Outward)];

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

const TRIGGER_CHOICES: &[(&str, Trigger)] = &[
    ("Continuous", Trigger::Continuous),
    ("Burst", Trigger::Burst),
];

/// One scene or force knob a route may drive: the persisted id and how a
/// route's factor lands on it. The factor scales the knob's own setting,
/// so the slider stays the reference a route works against rather than
/// going dead once bound, and a knob set to zero is off, route or no
/// route. The ids only ever grow; a config carrying an unknown one goes
/// quiet rather than misfiring. Making a new value bindable is one entry
/// here plus wrapping its settings row in
/// [`ParticlesPanel::bindable_row`].
struct BindTarget {
    id: &'static str,
    apply: fn(&mut Scene, &mut Forces, f32),
}

const BIND_TARGETS: &[BindTarget] = &[
    BindTarget {
        id: "gravity",
        apply: |scene, _, k| scene.gravity *= k,
    },
    BindTarget {
        id: "drag",
        apply: |scene, _, k| scene.drag *= k,
    },
    BindTarget {
        id: "turbulence",
        apply: |_, forces, k| forces.turbulence *= k,
    },
    BindTarget {
        id: "scale",
        apply: |_, forces, k| forces.turbulence_scale *= k,
    },
    BindTarget {
        id: "drift",
        apply: |_, forces, k| forces.turbulence_speed *= k,
    },
];

/// [`BindTarget`]'s per-emitter counterpart: the knob id rides in an
/// `e<id>.<knob>` target against the emitter's stable id, and the factor
/// scales that emitter's own setting.
struct EmitterBindTarget {
    id: &'static str,
    apply: fn(&mut Emitter, f32),
}

const EMITTER_BIND_TARGETS: &[EmitterBindTarget] = &[
    EmitterBindTarget {
        id: "speed",
        apply: |emitter, k| emitter.speed *= k,
    },
    EmitterBindTarget {
        id: "rate",
        apply: |emitter, k| emitter.rate *= k,
    },
    EmitterBindTarget {
        id: "burst",
        apply: |emitter, k| emitter.burst *= k,
    },
    EmitterBindTarget {
        id: "cone",
        apply: |emitter, k| emitter.cone *= k,
    },
    EmitterBindTarget {
        id: "size",
        apply: |emitter, k| emitter.size *= k,
    },
    EmitterBindTarget {
        id: "life",
        apply: |emitter, k| emitter.life *= k,
    },
];

/// The source picker's face for [`Source`], which carries band bounds the
/// segmented control can't.
#[derive(Clone, Copy, PartialEq)]
enum SourceKind {
    Band,
    Level,
    Onset,
}

const SOURCE_CHOICES: &[(&str, SourceKind)] = &[
    ("Band", SourceKind::Band),
    ("Level", SourceKind::Level),
    ("Onset", SourceKind::Onset),
];

/// One emitter: pure geometry and throw. It carries no audio of its own;
/// reactivity arrives by routing pool signals onto its knobs, and unbound
/// it just runs at its sliders, a fountain independent of the music.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Emitter {
    /// A stable handle routes point at, unique within the panel and
    /// persisted, so a route survives removals shifting the list under it.
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
    /// carries the arrangement instead of scattering it.
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
            shape: Shape::Line,
            x: 0.5,
            y: 1.0,
            width: 1.0,
            height: 0.2,
            rotation: 0.0,
            aim: Aim::Fixed,
            direction: 0.0,
            cone: 30.0,
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

/// The scene: what the whole field sits in, apart from any one emitter.
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
            gravity: 140.0,
            gravity_angle: 180.0,
            drag: 0.4,
            round: true,
            glow: false,
            freeze: false,
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
            turbulence: 0.0,
            turbulence_scale: 220.0,
            turbulence_speed: 0.4,
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
    /// A fresh panel carries one emitter rather than an empty field, so it
    /// draws something the moment it lands in the dock. An emptied list is
    /// still respected: the layout dump carries `"emitters": []` explicitly,
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

/// A strip fraction (0 to 1) as a log-spaced frequency across the slider
/// band, and back. Log so an octave takes the same travel anywhere, the way
/// the spectrum's bounds sliders map.
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

/// Give every emitter a unique id, keeping the ones a loaded config
/// carries: zeroes (configs from before ids existed) and hand-edited
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

/// A binding target's emitter route, `e<id>.<knob>`, if that is what it is.
fn emitter_route(target: &str) -> Option<(u64, &str)> {
    let (id, knob) = target.strip_prefix('e')?.split_once('.')?;
    Some((id.parse().ok()?, knob))
}

/// Resolve the routes against the hub's live signals into the emitters,
/// scene, and forces this frame runs with. A route's span maps through the
/// same range its target's slider covers, so it can do exactly what a hand
/// on the slider could and nothing more. Later routes to the same target
/// win; routes whose signal is gone contribute nothing.
fn modulated(config: &ParticlesConfig, hub: &SignalHub) -> (Vec<Emitter>, Scene, Forces) {
    let mut emitters = config.emitters.clone();
    let mut scene = config.scene.clone();
    let mut forces = config.forces.clone();
    for route in &config.routes {
        if !route.enabled {
            continue;
        }
        let Some(signal) = hub.value(route.signal) else {
            continue;
        };
        // The span is a share of the knob's own setting: at full signal a
        // route reaches `to` of what the slider says, at silence `from`.
        // Overshoot past 100% is allowed and the knob's own accessor
        // clamps it to the range the sim will take.
        let factor = (route.from + (route.to - route.from) * signal).max(0.0);
        if let Some((id, knob)) = emitter_route(&route.target) {
            if let (Some(emitter), Some(target)) = (
                emitters.iter_mut().find(|e| e.id == id),
                EMITTER_BIND_TARGETS.iter().find(|t| t.id == knob),
            ) {
                (target.apply)(emitter, factor);
            }
            continue;
        }
        if let Some(target) = BIND_TARGETS.iter().find(|t| t.id == route.target) {
            (target.apply)(&mut scene, &mut forces, factor);
        }
    }
    (emitters, scene, forces)
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

/// xorshift32. The field wants scatter, not statistics, and rolling it here
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
/// read nearly the same value, which is what makes the drift look like
/// wind rather than per-particle jitter.
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
/// per-frame work where the bounds are known. The audio analysis itself
/// lives in the app's shared [`SignalHub`]; the sim only reads values.
struct Sim {
    last_tick: Option<Instant>,
    /// The fraction of a particle each emitter carried past the last tick,
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
    fn step(
        &mut self,
        feed: &AudioFeed,
        hub: &SignalHub,
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

        hub.tick(feed);
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
                // The pop lands on the routed signal's rising edge, with
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
        // Where on the footprint the particle appears, and how far it sits
        // from the center, which is what Outward aims along.
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
            // A dim, wide halo under the core carries the same fade, so a
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

/// Write the shared pool through to settings, the hub's one persistence
/// path, so a relaunch finds what every open panel was riding.
fn persist_pool(pool: Vec<Signal>) {
    crate::settings::Settings::update(move |s| s.look.bundle.signals = pool);
}

/// Apply one edit to a pool signal through the hub and persist the result.
/// Editing tunes the signal for every route riding it, which is the point
/// of sharing.
fn edit_signal(panel: &ParticlesPanel, id: u64, edit: impl FnOnce(&mut Signal)) {
    let pool = panel.state.signals.edit(|pool| {
        if let Some(signal) = pool.iter_mut().find(|s| s.id == id) {
            edit(signal);
        }
    });
    persist_pool(pool);
}

/// A thin live meter for the customize window: one signal's value read off
/// the hub at paint time, so tuning happens against what the music is
/// actually sending. Keeps frames coming while audio moves, since that
/// window renders on its own clock, not the panel's.
fn meter(hub: Arc<SignalHub>, id: u64, fill: Rgba, marker: Option<f32>) -> Div {
    div().h(px(6.)).w_full().child(
        canvas(
            move |_, _, _| {},
            move |bounds, _, window, _| {
                let value = hub.value(id).unwrap_or(0.0).clamp(0.0, 1.0);
                let live = hub.live();
                let radius = bounds.size.height / 2.0;
                window.paint_quad(gpui::quad(
                    bounds,
                    radius,
                    palette::bg_control(),
                    0.,
                    gpui::transparent_black(),
                    BorderStyle::default(),
                ));
                if value > 0.0 {
                    window.paint_quad(gpui::quad(
                        Bounds::new(
                            bounds.origin,
                            size(bounds.size.width * value, bounds.size.height),
                        ),
                        radius,
                        palette::alpha(fill, 210),
                        0.,
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
                if let Some(marker) = marker {
                    window.paint_quad(gpui::quad(
                        Bounds::new(
                            point(
                                bounds.origin.x + bounds.size.width * marker - px(0.75),
                                bounds.origin.y,
                            ),
                            size(px(1.5), bounds.size.height),
                        ),
                        0.,
                        palette::text_faint(),
                        0.,
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
                if live {
                    window.request_animation_frame();
                }
            },
        )
        .size_full(),
    )
}

/// One chip of a binding's scope row: the segmented control's look, built
/// by hand because the scope list follows the live emitter list, which the
/// static segmented options can't carry.
fn scope_chip(
    label: String,
    picked: bool,
    on_pick: impl Fn(&mut ParticlesPanel, &mut Context<ParticlesPanel>) + 'static,
    cx: &mut Context<ParticlesPanel>,
) -> Div {
    div()
        .px(tokens::SPACE_SM)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .bg(if picked {
            palette::accent()
        } else {
            palette::bg_control()
        })
        .when(!picked, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
        .text_color(if picked {
            palette::text_on_accent()
        } else {
            palette::text()
        })
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| on_pick(this, cx)),
        )
        .child(label)
}

/// The editor overlay: every emitter's footprint dotted onto the field and
/// its center as the grab handle, in the emitter's own color so the markers
/// read against the settings list. Disabled emitters dim; the dragged one
/// swells. Dots are the one outline every shape can wear under
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

/// One route's span sliders, index-aligned with the config's list.
#[derive(Default)]
struct RouteScrubs {
    from: ScrubState,
    to: ScrubState,
}

/// One pool signal's tuning sliders, keyed by signal id since the same
/// signal can be edited from several surfaces.
#[derive(Default)]
struct SignalScrubs {
    lo: ScrubState,
    hi: ScrubState,
    smooth: ScrubState,
}

/// A labelled config toggle for the Display menu: the row label, a getter
/// for its current state, and a setter that flips it.
type ConfigToggle = (
    &'static str,
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
    /// Per-route slider state, kept the same length as the list, and the
    /// pool signals' tuning state by id.
    route_scrubs: Vec<RouteScrubs>,
    signal_scrubs: std::collections::HashMap<u64, SignalScrubs>,
    gravity_scrub: ScrubState,
    gravity_angle_scrub: ScrubState,
    drag_scrub: ScrubState,
    turbulence_scrub: ScrubState,
    turb_scale_scrub: ScrubState,
    turb_speed_scrub: ScrubState,
    focus: FocusHandle,
    /// The one readout being typed into across all the settings sliders.
    value_edit: ValueEdit,
    /// The one signal being renamed: the input holding the draft and the
    /// subscription that commits it on Enter. The bounds cell backs the
    /// click-outside cancel, since nothing else in the settings window
    /// takes focus and blur alone never fires.
    rename: Option<(u64, Entity<InputState>, Subscription)>,
    rename_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    /// The target whose route is expanded inline under its settings row.
    open_bind: Option<String>,
    /// The editor overlay: markers over the field for arranging emitters
    /// by hand. Session state, deliberately not persisted.
    edit: bool,
    /// The emitter riding the pointer while the editor is on.
    drag: Option<usize>,
    /// The field canvas's painted bounds, for mapping editor presses into
    /// emitter fractions, the scrub strips' arrangement.
    canvas_bounds: Arc<Mutex<Bounds<Pixels>>>,
    /// The tab panel this panel currently sits in, for duplicate and
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
            route_scrubs: Vec::new(),
            signal_scrubs: std::collections::HashMap::new(),
            gravity_scrub: ScrubState::default(),
            gravity_angle_scrub: ScrubState::default(),
            drag_scrub: ScrubState::default(),
            turbulence_scrub: ScrubState::default(),
            turb_scale_scrub: ScrubState::default(),
            turb_speed_scrub: ScrubState::default(),
            focus: cx.focus_handle(),
            value_edit: ValueEdit::default(),
            rename: None,
            rename_bounds: Arc::new(Mutex::new(None)),
            open_bind: None,
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

    /// Attach the row's route to ride `signal`, repointing an existing
    /// route rather than stacking a second, and open its editor.
    fn attach_signal(&mut self, target: String, signal: u64, cx: &mut Context<Self>) {
        if let Some(route) = self
            .config
            .routes
            .iter_mut()
            .rev()
            .find(|r| r.target == target)
        {
            route.signal = signal;
        } else {
            self.config.routes.push(Route {
                signal,
                target: target.clone(),
                ..Route::default()
            });
        }
        self.open_bind = Some(target);
        cx.notify();
    }

    /// The context menu's deliberate "Add Signal": a fresh pool signal,
    /// routed to the row on the spot.
    fn attach_new_signal(&mut self, target: String, cx: &mut Context<Self>) {
        let (id, pool) = self.state.signals.add(
            Source::Band {
                lo: 30.0,
                hi: 120.0,
            },
            0.3,
        );
        persist_pool(pool);
        self.attach_signal(target, id, cx);
    }

    /// Start renaming a signal: an input seeded with the given name (not
    /// the derived label, so clearing the field is how a name goes back
    /// to following the source). Enter commits, clicking away cancels.
    fn begin_rename(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let current = self
            .state
            .signals
            .pool()
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Signal name")
                .default_value(current)
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            move |this: &mut Self, input, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let name = input.read(cx).value().trim().to_string();
                    edit_signal(this, id, |signal| signal.name = name);
                    this.rename = None;
                    cx.notify();
                }
                InputEvent::Blur => {
                    this.rename = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        window.focus(&input.read(cx).focus_handle(cx));
        self.rename = Some((id, input, sub));
        cx.notify();
    }

    fn remove_route(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.config.routes.len() {
            self.config.routes.remove(index);
            cx.notify();
        }
    }

    /// Drop a signal from the shared pool. Routes riding it stay where
    /// they are and go quiet, so re-adding or repointing restores them.
    fn remove_signal(&mut self, id: u64, cx: &mut Context<Self>) {
        let pool = self.state.signals.edit(|pool| pool.retain(|s| s.id != id));
        persist_pool(pool);
        cx.notify();
    }

    /// A press in the editor: pick the emitter whose center sits nearest,
    /// within the grab radius, and let it ride the pointer.
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
                "Round Particles",
                |this| this.config.scene.round,
                |this| this.config.scene.round = !this.config.scene.round,
            ),
            (
                "Glow",
                |this| this.config.scene.glow,
                |this| this.config.scene.glow = !this.config.scene.glow,
            ),
            (
                "Hold on Pause",
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
        menu.item(PopupMenuItem::submenu("Display", submenu))
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
        &[
            ("Emitters", icons::AUDIO_LINES),
            ("Signals", icons::AUDIO_WAVEFORM),
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
        let count = self.config.routes.len();
        if self.route_scrubs.len() != count {
            self.route_scrubs.resize_with(count, RouteScrubs::default);
        }
        let pool = self.state.signals.pool();
        self.signal_scrubs
            .retain(|id, _| pool.iter().any(|s| s.id == *id));
        for signal in &pool {
            self.signal_scrubs.entry(signal.id).or_default();
        }
        match page {
            "Signals" => self.signals_page(cx).into_any_element(),
            "Forces" => self.forces_page(cx).into_any_element(),
            "Scene" => self.scene_page(cx).into_any_element(),
            _ => self.emitters_page(window, cx).into_any_element(),
        }
    }
}

impl ParticlesPanel {
    /// The Emitters page: the list, each emitter a block of its own rows.
    fn emitters_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.sync_emitter_state(window, cx);
        let count = self.config.emitters.len();
        let add = settings_ui::small_button(
            "Add Emitter",
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
                    .child("No emitters yet - add one to start the field."),
            );
        }
        for i in 0..count {
            list = list.child(self.emitter_block(i, cx));
        }
        div().flex().flex_col().gap(SECTION_GAP).child(section(
            "Emitters",
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
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(format!("Emitter {}", index + 1)),
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
                "Trigger",
                None,
                panel::choices(
                    TRIGGER_CHOICES,
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
                d.child(self.bindable_row(
                    "Rate",
                    None,
                    format!("e{eid}.rate"),
                    panel::value_slider_edit(
                        &scrubs.rate,
                        &self.value_edit,
                        (rate - RATE_MIN) / (RATE_MAX - RATE_MIN),
                        format!("{rate:.0}/s"),
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
                d.child(self.bindable_row(
                    "Burst",
                    None,
                    format!("e{eid}.burst"),
                    panel::value_slider_edit(
                        &scrubs.burst,
                        &self.value_edit,
                        (burst - BURST_MIN) / (BURST_MAX - BURST_MIN),
                        format!("{burst:.0}"),
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
                "Shape",
                None,
                panel::choices(
                    SHAPE_CHOICES,
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
                "Position X",
                None,
                panel::value_slider_edit(
                    &scrubs.x,
                    &self.value_edit,
                    x,
                    format!("{}%", (x * 100.0).round() as i32),
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
                "Position Y",
                None,
                panel::value_slider_edit(
                    &scrubs.y,
                    &self.value_edit,
                    y,
                    format!("{}%", (y * 100.0).round() as i32),
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
                        Shape::Ring => "Radius",
                        Shape::Box => "Width",
                        _ => "Length",
                    },
                    None,
                    panel::value_slider_edit(
                        &scrubs.width,
                        &self.value_edit,
                        width / 2.0,
                        format!("{}%", (width * 100.0).round() as i32),
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
                    "Height",
                    None,
                    panel::value_slider_edit(
                        &scrubs.height,
                        &self.value_edit,
                        height / 2.0,
                        format!("{}%", (height * 100.0).round() as i32),
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
                    "Rotation",
                    None,
                    panel::value_slider_edit(
                        &scrubs.rotation,
                        &self.value_edit,
                        rotation / 360.0,
                        format!("{rotation:.0}°"),
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
                "Aim",
                None,
                panel::choices(
                    AIM_CHOICES,
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
                    "Direction",
                    None,
                    panel::value_slider_edit(
                        &scrubs.direction,
                        &self.value_edit,
                        direction / 360.0,
                        format!("{direction:.0}°"),
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
            .child(self.bindable_row(
                "Cone",
                None,
                format!("e{eid}.cone"),
                panel::value_slider_edit(
                    &scrubs.cone,
                    &self.value_edit,
                    cone / 360.0,
                    format!("{cone:.0}°"),
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
            .child(self.bindable_row(
                "Speed",
                None,
                format!("e{eid}.speed"),
                panel::value_slider_edit(
                    &scrubs.speed,
                    &self.value_edit,
                    (speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN),
                    format!("{speed:.0} px/s"),
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
            .child(self.bindable_row(
                "Size",
                None,
                format!("e{eid}.size"),
                panel::value_slider_edit(
                    &scrubs.size,
                    &self.value_edit,
                    (size - SIZE_MIN) / (SIZE_MAX - SIZE_MIN),
                    format!("{size:.0} px"),
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
            .child(self.bindable_row(
                "Lifetime",
                None,
                format!("e{eid}.life"),
                panel::value_slider_edit(
                    &scrubs.life,
                    &self.value_edit,
                    (life - LIFE_MIN) / (LIFE_MAX - LIFE_MIN),
                    format!("{life:.1} s"),
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
            .child(setting_row("Color", None, color_row))
    }

    /// The Signals page: the app's shared pool, tended from any particles
    /// panel because it is one pool. Routes live inline under the knobs
    /// they drive; this page is where the signals themselves are tuned,
    /// and an edit lands on every route riding the signal, in every panel.
    fn signals_page(&mut self, cx: &mut Context<Self>) -> Div {
        let pool = self.state.signals.pool();
        let add = settings_ui::small_button(
            "Add Signal",
            icons::PLUS,
            false,
            cx.listener(|this, _, _, cx| {
                let (_, pool) = this.state.signals.add(
                    Source::Band {
                        lo: 30.0,
                        hi: 120.0,
                    },
                    0.3,
                );
                persist_pool(pool);
                cx.notify();
            }),
        );
        let mut list = div().flex().flex_col().gap(tokens::SPACE_MD);
        if pool.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child("No signals yet - add one, or right-click any bindable knob."),
            );
        }
        for signal in &pool {
            list = list.child(self.signal_block(signal.id, cx));
        }
        div().flex().flex_col().gap(SECTION_GAP).child(section(
            "Signals",
            Some(add.into_any_element()),
            list,
        ))
    }

    /// One pool signal's block on the Signals page: its derived name, the
    /// live meter, its tuning, how many of this panel's routes ride it,
    /// and the delete that lets those routes go quiet.
    fn signal_block(&self, id: u64, cx: &mut Context<Self>) -> Div {
        let pool = self.state.signals.pool();
        let Some(signal) = pool.iter().find(|s| s.id == id) else {
            return div();
        };
        let riders = self.config.routes.iter().filter(|r| r.signal == id).count();
        // While this signal is being renamed the label swaps for the
        // input; committing or clicking away swaps it back. A one-frame
        // window handler cancels on any press outside the field.
        let name: AnyElement = match &self.rename {
            Some((rid, input, _)) if *rid == id => {
                let entity = cx.entity();
                let cell = self.rename_bounds.clone();
                div()
                    .relative()
                    .w(px(180.))
                    .child(
                        canvas(
                            {
                                let cell = cell.clone();
                                move |bounds, _, _| *cell.lock().unwrap() = Some(bounds)
                            },
                            move |_, _, window, _| {
                                let cell = cell.clone();
                                let entity = entity.clone();
                                window.on_mouse_event(
                                    move |event: &MouseDownEvent, phase, _, cx| {
                                        if !phase.bubble() {
                                            return;
                                        }
                                        let inside = cell
                                            .lock()
                                            .unwrap()
                                            .is_some_and(|b| b.contains(&event.position));
                                        if inside {
                                            return;
                                        }
                                        entity.update(cx, |this, cx| {
                                            if this.rename.is_some() {
                                                this.rename = None;
                                                cx.notify();
                                            }
                                        });
                                    },
                                );
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(Input::new(input).small().w_full())
                    .into_any_element()
            }
            _ => div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(signal.label())
                .into_any_element(),
        };
        let header = settings_ui::block_header(
            name,
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child(match riders {
                            0 => "no routes in this panel".to_string(),
                            1 => "1 route in this panel".to_string(),
                            n => format!("{n} routes in this panel"),
                        }),
                )
                .child(settings_ui::icon_button(
                    icons::PENCIL,
                    false,
                    cx.listener(move |this, _, window, cx| this.begin_rename(id, window, cx)),
                ))
                .child(settings_ui::icon_button(
                    icons::TRASH,
                    false,
                    cx.listener(move |this, _, _, cx| this.remove_signal(id, cx)),
                )),
        );
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(header)
            .child(meter(
                self.state.signals.clone(),
                id,
                palette::accent(),
                None,
            ))
            .child(self.signal_tuning(id, cx))
    }

    /// One shared signal's tuning rows: what it listens to and how it
    /// responds. Edits go through the hub, so every route riding it, in
    /// every panel, follows.
    fn signal_tuning(&self, id: u64, cx: &mut Context<Self>) -> Div {
        let pool = self.state.signals.pool();
        let Some(signal) = pool.iter().find(|s| s.id == id) else {
            return div();
        };
        let Some(scrubs) = self.signal_scrubs.get(&id) else {
            return div();
        };
        let (kind, freq_lo, freq_hi) = match signal.source {
            Source::Band { lo, hi } => (SourceKind::Band, lo, hi),
            Source::Onset { lo, hi } => (SourceKind::Onset, lo, hi),
            Source::Level => (SourceKind::Level, 30.0, 120.0),
        };
        let smooth = signal.smooth.clamp(0.0, 1.0);
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(setting_row(
                "Source",
                Some(
                    "What the signal listens to: Band follows one frequency range, \
                     Level the whole mix, Onset pulses on each hit in the range",
                ),
                panel::choices(
                    SOURCE_CHOICES,
                    kind,
                    move |this: &mut Self, kind, cx| {
                        // Switching kinds carries the band along, so Band
                        // to Onset keeps the range the ear already picked.
                        edit_signal(this, id, |signal| {
                            let (lo, hi) = match signal.source {
                                Source::Band { lo, hi } | Source::Onset { lo, hi } => (lo, hi),
                                Source::Level => (30.0, 120.0),
                            };
                            signal.source = match kind {
                                SourceKind::Band => Source::Band { lo, hi },
                                SourceKind::Onset => Source::Onset { lo, hi },
                                SourceKind::Level => Source::Level,
                            };
                        });
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(kind != SourceKind::Level, |d| {
                d.child(setting_row(
                    "Low Bound",
                    None,
                    panel::value_slider_edit(
                        &scrubs.lo,
                        &self.value_edit,
                        hz_to_frac(freq_lo),
                        fmt_hz(freq_lo),
                        format!("{freq_lo:.0}"),
                        hz_to_frac,
                        move |this: &mut Self, fraction, cx| {
                            edit_signal(this, id, |signal| {
                                if let Source::Band { lo, hi } | Source::Onset { lo, hi } =
                                    &mut signal.source
                                {
                                    let ceil = (*hi / MIN_RATIO).max(SLIDER_MIN_HZ);
                                    *lo = frac_to_hz(fraction).clamp(SLIDER_MIN_HZ, ceil);
                                }
                            });
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(setting_row(
                    "High Bound",
                    None,
                    panel::value_slider_edit(
                        &scrubs.hi,
                        &self.value_edit,
                        hz_to_frac(freq_hi),
                        fmt_hz(freq_hi),
                        format!("{freq_hi:.0}"),
                        hz_to_frac,
                        move |this: &mut Self, fraction, cx| {
                            edit_signal(this, id, |signal| {
                                if let Source::Band { lo, hi } | Source::Onset { lo, hi } =
                                    &mut signal.source
                                {
                                    let floor = (*lo * MIN_RATIO).min(SLIDER_MAX_HZ);
                                    *hi = frac_to_hz(fraction).clamp(floor, SLIDER_MAX_HZ);
                                }
                            });
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(setting_row(
                "Response",
                Some(if kind == SourceKind::Onset {
                    "How long each pulse rings before it dies away"
                } else {
                    "0 snaps to the music, 100 drifts after it"
                }),
                panel::value_slider_edit(
                    &scrubs.smooth,
                    &self.value_edit,
                    smooth,
                    format!("{}%", (smooth * 100.0).round() as i32),
                    format!("{}", (smooth * 100.0).round() as i32),
                    |v| v / 100.0,
                    move |this: &mut Self, fraction, cx| {
                        edit_signal(this, id, |signal| {
                            signal.smooth = fraction.clamp(0.0, 1.0);
                        });
                        cx.notify();
                    },
                    cx,
                ),
            ))
    }

    /// One route's tuning rows for the inline editor: which shared signal
    /// it rides (with the pool as a picker), that signal's tuning in
    /// place, and the span it sweeps. A route whose signal is gone says so
    /// and waits for a repoint instead of pretending.
    fn route_tuning(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let route = &self.config.routes[index];
        let scrubs = &self.route_scrubs[index];
        let pool = self.state.signals.pool();
        let known = pool.iter().any(|s| s.id == route.signal);
        let from = route.from.clamp(0.0, SPAN_OVER);
        let to = route.to.clamp(0.0, SPAN_OVER);

        let mut chips = div().flex().flex_row().flex_wrap().gap(px(1.));
        for signal in &pool {
            let id = signal.id;
            chips = chips.child(scope_chip(
                signal.label(),
                known && route.signal == id,
                move |this, cx| {
                    if let Some(route) = this.config.routes.get_mut(index) {
                        route.signal = id;
                    }
                    cx.notify();
                },
                cx,
            ));
        }
        chips = chips.child(scope_chip(
            "New Signal".to_string(),
            false,
            move |this, cx| {
                let (id, pool) = this.state.signals.add(
                    Source::Band {
                        lo: 30.0,
                        hi: 120.0,
                    },
                    0.3,
                );
                persist_pool(pool);
                if let Some(route) = this.config.routes.get_mut(index) {
                    route.signal = id;
                }
                cx.notify();
            },
            cx,
        ));

        let mut col = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(panel::setting_block(
                "Signal",
                Some(
                    "Which shared signal this route rides; tuning it here tunes every route on it",
                ),
                None,
                chips,
            ));
        if known {
            col = col
                .child(meter(
                    self.state.signals.clone(),
                    route.signal,
                    palette::accent(),
                    None,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child("Shared by every route on this signal"),
                )
                .child(self.signal_tuning(route.signal, cx));
        } else {
            col = col.child(div().text_xs().text_color(palette::text_muted()).child(
                "This route's signal is gone; the knob holds its slider value \
                        until another is picked above.",
            ));
        }
        // The span belongs to this route alone, where everything above it
        // is the shared signal: the same signal can pull one knob all the
        // way and nudge another, so the two halves are labelled apart.
        col.child(
            div()
                .pt(tokens::SPACE_XS)
                .text_xs()
                .text_color(palette::text_faint())
                .child("Range for this parameter only"),
        )
        .child(setting_row(
            "Quiet",
            Some("What the knob reaches at silence, as a share of its own setting"),
            panel::value_slider_edit_over(
                &scrubs.from,
                &self.value_edit,
                from,
                format!("{}%", (from * 100.0).round() as i32),
                format!("{}", (from * 100.0).round() as i32),
                SPAN_OVER,
                |v| v / 100.0,
                move |this: &mut Self, fraction, cx| {
                    if let Some(route) = this.config.routes.get_mut(index) {
                        route.from = fraction.clamp(0.0, SPAN_OVER);
                    }
                    cx.notify();
                },
                cx,
            ),
        ))
        .child(setting_row(
            "Loud",
            Some("What it reaches at full signal; 100% is the slider's own value, below Quiet modulates down"),
            panel::value_slider_edit_over(
                &scrubs.to,
                &self.value_edit,
                to,
                format!("{}%", (to * 100.0).round() as i32),
                format!("{}", (to * 100.0).round() as i32),
                SPAN_OVER,
                |v| v / 100.0,
                move |this: &mut Self, fraction, cx| {
                    if let Some(route) = this.config.routes.get_mut(index) {
                        route.to = fraction.clamp(0.0, SPAN_OVER);
                    }
                    cx.notify();
                },
                cx,
            ),
        ))
    }

    /// A settings row whose knob a route can drive: the row itself with a
    /// bind toggle at its edge, and the route's tuning expanded beneath
    /// while open. The slider keeps working while bound, since the route's
    /// span is a share of it: the slider sets what full signal reaches and
    /// the span decides how far the music pulls it back. Clicking the
    /// toggle on an unbound row creates the route on the spot, and a
    /// right-click anywhere on the row's control does the same, so binding
    /// never needs the little icon found first. Removing the route lives
    /// on the trash inside the expanded editor.
    fn bindable_row(
        &self,
        label: &'static str,
        description: Option<&'static str>,
        target: String,
        control: Div,
        cx: &mut Context<Self>,
    ) -> Div {
        let bound = self.config.routes.iter().rposition(|r| r.target == target);
        let open = self.open_bind.as_deref() == Some(target.as_str());
        let weak = cx.entity().downgrade();
        let menu_target = target.clone();
        let control = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            // Right-click routes: pick a pool signal to ride, or add one
            // deliberately. The menu shows even over an empty pool, so the
            // way in is never invisible.
            .context_menu(move |mut menu, _, cx| {
                let Some(this) = weak.upgrade() else {
                    return menu;
                };
                let pool = this.read(cx).state.signals.pool();
                for signal in &pool {
                    let id = signal.id;
                    let panel = weak.clone();
                    let target = menu_target.clone();
                    menu = menu.item(PopupMenuItem::new(signal.label()).on_click(
                        move |_, _, cx| {
                            if let Some(this) = panel.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.attach_signal(target.clone(), id, cx)
                                });
                            }
                        },
                    ));
                }
                if !pool.is_empty() {
                    menu = menu.separator();
                }
                let panel = weak.clone();
                let target = menu_target.clone();
                menu.item(
                    PopupMenuItem::new("Add Signal")
                        .icon(Icon::default().path(icons::PLUS))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = panel.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.attach_new_signal(target.clone(), cx)
                                });
                            }
                        }),
                )
            })
            // The slider keeps its full weight while bound: it is what the
            // route's span is a share of, so it still sets the ceiling.
            .child(control)
            // The bind mark only exists once a route does; an unbound row
            // keeps an empty slot the same size so the sliders stay in
            // column, and the context menu is the way in.
            .map(|d| {
                if bound.is_some() {
                    d.child(settings_ui::icon_button(
                        icons::AUDIO_WAVEFORM,
                        false,
                        cx.listener({
                            let target = target.clone();
                            move |this: &mut Self, _, _, cx| {
                                this.open_bind =
                                    if this.open_bind.as_deref() == Some(target.as_str()) {
                                        None
                                    } else {
                                        Some(target.clone())
                                    };
                                cx.notify();
                            }
                        }),
                    ))
                } else {
                    d.child(
                        div()
                            .flex_none()
                            .w(tokens::SPACE_XS * 2.0 + px(14.))
                            .h(px(14.)),
                    )
                }
            });
        // The context menu keys its open state on the element id path, and
        // `context_menu` names every one of them the same thing. Several
        // bindable rows on a page would land on one shared state, rendering
        // one menu entity in several places and swallowing its clicks, so
        // each row's control sits under an id of its own.
        let control = div()
            .id(SharedString::from(format!("bind-row-{target}")))
            .child(control);
        let mut row = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(setting_row(label, description, control));
        if open {
            if let Some(index) = bound {
                let header = settings_ui::block_header(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("Route"),
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .child(toggle(
                            self.config.routes[index].enabled,
                            move |this: &mut Self, on, cx| {
                                if let Some(route) = this.config.routes.get_mut(index) {
                                    route.enabled = on;
                                }
                                cx.notify();
                            },
                            cx,
                        ))
                        .child(settings_ui::icon_button(
                            icons::TRASH,
                            false,
                            cx.listener(move |this, _, _, cx| {
                                this.open_bind = None;
                                this.remove_route(index, cx);
                            }),
                        )),
                );
                row = row.child(settings_ui::nested(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_SM)
                        .child(header)
                        .child(self.route_tuning(index, cx)),
                ));
            }
        }
        row
    }
    /// The Forces page: the drift laid over the scene's steady pull. Every
    /// knob here is a binding target, so each row carries the bind toggle.
    fn forces_page(&mut self, cx: &mut Context<Self>) -> Div {
        let turbulence = self.config.forces.turbulence();
        let scale = self.config.forces.scale();
        let speed = self.config.forces.speed();
        let strength_slider = panel::value_slider_edit(
            &self.turbulence_scrub,
            &self.value_edit,
            turbulence / TURB_MAX,
            format!("{turbulence:.0}"),
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
            format!("{scale:.0} px"),
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
            format!("{speed:.2}"),
            format!("{speed:.2}"),
            |v| v / TURB_SPEED_MAX,
            |this: &mut Self, fraction, cx| {
                this.config.forces.turbulence_speed = fraction * TURB_SPEED_MAX;
                cx.notify();
            },
            cx,
        );
        div().flex().flex_col().gap(SECTION_GAP).child(section(
            "Turbulence",
            None,
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(self.bindable_row(
                    "Strength",
                    Some("How hard the field pushes particles around; zero is off"),
                    "turbulence".to_string(),
                    strength_slider,
                    cx,
                ))
                .child(self.bindable_row(
                    "Scale",
                    Some("How wide one swirl runs; small churns, large rolls"),
                    "scale".to_string(),
                    scale_slider,
                    cx,
                ))
                .child(self.bindable_row(
                    "Drift",
                    Some("How fast the field itself moves, so the swirls don't stand still"),
                    "drift".to_string(),
                    drift_slider,
                    cx,
                )),
        ))
    }

    /// The Scene page: what the whole field sits in, apart from any one
    /// emitter.
    fn scene_page(&mut self, cx: &mut Context<Self>) -> Div {
        let gravity = self.config.scene.gravity();
        let angle = self.config.scene.gravity_angle.rem_euclid(360.0);
        let drag = self.config.scene.drag();
        let gravity_slider = panel::value_slider_edit(
            &self.gravity_scrub,
            &self.value_edit,
            gravity / GRAVITY_MAX,
            format!("{gravity:.0}"),
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
            format!("{angle:.0}°"),
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
            format!("{drag:.2}"),
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
                "Gravity",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(self.bindable_row(
                        "Strength",
                        Some("Constant pull on everything in flight"),
                        "gravity".to_string(),
                        gravity_slider,
                        cx,
                    ))
                    .child(setting_row(
                        "Direction",
                        Some("Which way it pulls; 0 is up, 180 is down"),
                        angle_slider,
                    )),
            ))
            .child(section(
                "Medium",
                None,
                self.bindable_row(
                    "Drag",
                    Some("How much speed the air eats each second; zero is a vacuum"),
                    "drag".to_string(),
                    drag_slider,
                    cx,
                ),
            ))
            .child(section(
                "Particles",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(setting_row(
                        "Round Particles",
                        Some("Draw dots instead of squares"),
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
                        "Glow",
                        Some("Lay a soft halo behind each particle"),
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
            .child(section(
                "Playback",
                None,
                setting_row(
                    "Hold on Pause",
                    Some("Freeze the field while paused instead of letting it drift out"),
                    toggle(
                        self.config.scene.freeze,
                        |this: &mut Self, on, cx| {
                            this.config.scene.freeze = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ),
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
        panel::title_text(self.config.chrome.title.as_deref(), "Particles")
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

    /// The layout dump carries the panel's config; the builder registered
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
        let menu = menu.item(panel::check_row(
            "Edit Emitters",
            None,
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
                    sim.step(&feed, &hub, w, h, &config, hold);
                    sim.paint(bounds, window, &config.scene);
                    if edit {
                        paint_markers(&config, drag, bounds, window);
                    }
                },
            )
            .size_full(),
        );
        // The editor rides the panel itself: press near a center to grab,
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
