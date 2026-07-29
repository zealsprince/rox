//! The particles panel: a field of emitters driven by the player's PCM tap.
//! Each emitter watches a frequency range and, once that range crosses its
//! threshold, spawns particles at a rate scaled by how far past it the music
//! sits, so a kick emitter puffs on every hit while a wide one breathes with
//! the mix. An emitter carries its own placement (a point, a line, a box, or
//! a ring, anywhere in the panel) and its own aim, so the field is arranged
//! rather than stuck to an edge. The scene's gravity and drag pull on
//! everything in flight; the force field adds drift on top. Everything is
//! paint primitives on the UI thread, one FFT per frame while audio flows,
//! and once the last particle dies the panel stops asking for frames.

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

use rox_viz::analysis::{log_bands, Analyzer, MAX_FFT_SIZE, MIN_FFT_SIZE};
use rox_viz::signal::{Binding, Signals, Source};
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState};
use crate::panel_settings;
use crate::settings::ui::{self as settings_ui, section, SECTION_GAP};

/// The frequency band an emitter's trigger may pick between, and the
/// smallest span it keeps between its bounds: tight enough for a kick,
/// wide enough that the mapping never inverts.
const SLIDER_MIN_HZ: f32 = 20.0;
const SLIDER_MAX_HZ: f32 = 20_000.0;
const MIN_RATIO: f32 = 1.2;

/// dB window the activations normalize into, on magnitudes where a
/// full-scale sine sits at 0 dB. Shared with the spectrum's bars so a
/// threshold reads against the same scale those bars draw.
const FLOOR_DB: f32 = -66.0;
const MAX_DB: f32 = -12.0;

/// Per-second smoothing rates for the activations: jump up fast, fall
/// slowly, so a transient reads as one burst instead of a stutter.
const ATTACK: f32 = 40.0;
const RELEASE: f32 = 10.0;

/// How long the feed may sit still before it reads as stopped audio rather
/// than the gap between pump ticks. The spectrum's reasoning applies here:
/// between ticks the activations hold instead of dipping, which would
/// otherwise chop the emission into a strobe on high-refresh displays.
const SILENT_AFTER: f32 = 0.15;

/// The ceiling on live particles. A pinned emitter at the top rate would
/// run away over a long track otherwise; past this, spawns are dropped
/// until the older ones age out.
const MAX_PARTICLES: usize = 4000;

/// How far outside the panel a particle may drift before it is culled, as
/// a fraction of the panel's larger side with a floor in px. Generous
/// enough that one thrown past the edge can still arc back under gravity.
const CULL_MARGIN: f32 = 0.25;
const CULL_MARGIN_MIN: f32 = 64.0;

/// The emission rate slider's span, particles per second at full
/// activation.
const RATE_MIN: f32 = 2.0;
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

/// The FFT sizes the picker offers. Emitters pool whole bands rather than
/// resolving single bins, so the long windows the spectrum offers buy
/// nothing here and cost reactivity.
const FFT_CHOICES: &[(&str, usize)] = &[
    ("512", 512),
    ("1k", 1024),
    ("2k", 2048),
    ("4k", 4096),
    ("8k", 8192),
];

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

/// The scene and force knobs a binding may drive, label and target id. The
/// ids are what the config persists, so they only ever grow.
const TARGET_CHOICES: &[(&str, &str)] = &[
    ("Gravity", "gravity"),
    ("Drag", "drag"),
    ("Size", "size"),
    ("Life", "life"),
    ("Turbulence", "turbulence"),
    ("Scale", "scale"),
    ("Drift", "drift"),
];

/// The knobs a binding may drive on one emitter, label and knob id; the
/// persisted target is `e<id>.<knob>` against the emitter's stable id.
const EMITTER_KNOB_CHOICES: &[(&str, &str)] = &[
    ("Speed", "speed"),
    ("Rate", "rate"),
    ("Burst", "burst"),
    ("Cone", "cone"),
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

/// One emitter: the range it listens to, how loud that range has to get
/// before it fires, where it sits, and which way it throws.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Emitter {
    /// A stable handle bindings point at, unique within the panel and
    /// persisted, so a route survives removals shifting the list under it.
    /// 0 is unassigned; the panel assigns on load and on add.
    pub id: u64,
    /// Whether the emitter fires. Off keeps it in the list, tuned, silent.
    pub enabled: bool,
    /// The watched range's bounds, Hz.
    pub freq_lo: f32,
    pub freq_hi: f32,
    /// Where the emitter starts firing, on the normalized loudness the
    /// spectrum's bars draw: 0 fires on anything, 1 never fires.
    pub threshold: f32,
    /// Particles per second at full activation. The live rate scales with
    /// how far past the threshold the range sits.
    pub rate: f32,
    /// Whether the emitter streams at `rate` or fires a `burst` on each
    /// onset.
    pub mode: Trigger,
    /// Particles thrown per onset in burst mode.
    pub burst: f32,
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
            freq_lo: 30.0,
            freq_hi: 120.0,
            threshold: 0.35,
            rate: 60.0,
            mode: Trigger::Continuous,
            burst: 24.0,
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
    /// The watched range, clamped to the slider band and the minimum span,
    /// so a hand-edited file can't invert or collapse it.
    fn range(&self) -> (f32, f32) {
        let lo = self.freq_lo.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
        let hi = self
            .freq_hi
            .clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ)
            .max(lo * MIN_RATIO)
            .min(SLIDER_MAX_HZ);
        (lo.min(hi / MIN_RATIO), hi)
    }

    fn threshold(&self) -> f32 {
        self.threshold.clamp(0.0, 0.99)
    }

    fn rate(&self) -> f32 {
        self.rate.clamp(RATE_MIN, RATE_MAX)
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

    /// A fresh emitter for the Add button, stepped up the spectrum from the
    /// last one so a second emitter doesn't listen to the same band.
    fn next_after(previous: Option<&Emitter>) -> Emitter {
        let Some(previous) = previous else {
            return Emitter::default();
        };
        let (_, hi) = previous.range();
        let lo = (hi * 1.2).min(SLIDER_MAX_HZ / MIN_RATIO);
        Emitter {
            freq_lo: lo,
            freq_hi: (lo * 3.0).min(SLIDER_MAX_HZ),
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
    /// Particle size, px, varied a little per particle.
    pub size: f32,
    /// How long a particle lasts before it fades out, seconds.
    pub life: f32,
    /// Draw particles as dots rather than squares.
    pub round: bool,
    /// Lay a soft halo behind each particle so it reads as light rather than
    /// a flat chip.
    pub glow: bool,
    /// FFT window size the activations are read from.
    pub fft_size: usize,
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
            size: 4.0,
            life: 2.5,
            round: true,
            glow: false,
            fft_size: 2048,
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

    fn size(&self) -> f32 {
        self.size.clamp(SIZE_MIN, SIZE_MAX)
    }

    fn life(&self) -> f32 {
        self.life.clamp(LIFE_MIN, LIFE_MAX)
    }

    /// The FFT size, snapped to the picker's power-of-two steps so a
    /// hand-edited file can't feed the analyzer a bad size.
    fn fft(&self) -> usize {
        self.fft_size
            .next_power_of_two()
            .clamp(MIN_FFT_SIZE, MAX_FFT_SIZE)
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
    /// Routes from the music into scene and force knobs, index-aligned
    /// with the signal engine's values.
    pub bindings: Vec<Binding>,
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
            bindings: Vec::new(),
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

/// Resolve the bindings against the live signals into the emitters, scene,
/// and forces this frame runs with. A binding's span maps through the same
/// range its target's slider covers, so a route can do exactly what a hand
/// on the slider could and nothing more. Later routes to the same target
/// win.
fn modulated(config: &ParticlesConfig, signals: &[f32]) -> (Vec<Emitter>, Scene, Forces) {
    let mut emitters = config.emitters.clone();
    let mut scene = config.scene.clone();
    let mut forces = config.forces.clone();
    for (binding, &signal) in config.bindings.iter().zip(signals) {
        if !binding.enabled {
            continue;
        }
        let f = (binding.from + (binding.to - binding.from) * signal).clamp(0.0, 1.0);
        if let Some((id, knob)) = emitter_route(&binding.target) {
            if let Some(emitter) = emitters.iter_mut().find(|e| e.id == id) {
                match knob {
                    "speed" => emitter.speed = SPEED_MIN + f * (SPEED_MAX - SPEED_MIN),
                    "rate" => emitter.rate = RATE_MIN + f * (RATE_MAX - RATE_MIN),
                    "burst" => emitter.burst = BURST_MIN + f * (BURST_MAX - BURST_MIN),
                    "cone" => emitter.cone = f * 360.0,
                    _ => {}
                }
            }
            continue;
        }
        match binding.target.as_str() {
            "gravity" => scene.gravity = f * GRAVITY_MAX,
            "drag" => scene.drag = f * DRAG_MAX,
            "size" => scene.size = SIZE_MIN + f * (SIZE_MAX - SIZE_MIN),
            "life" => scene.life = LIFE_MIN + f * (LIFE_MAX - LIFE_MIN),
            "turbulence" => forces.turbulence = f * TURB_MAX,
            "scale" => {
                forces.turbulence_scale = TURB_SCALE_MIN + f * (TURB_SCALE_MAX - TURB_SCALE_MIN)
            }
            "drift" => forces.turbulence_speed = f * TURB_SPEED_MAX,
            _ => {}
        }
    }
    (emitters, scene, forces)
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
/// per-frame work where the bounds are known.
struct Sim {
    last_written: u64,
    last_tick: Option<Instant>,
    /// When the feed last carried new audio.
    last_fresh: Option<Instant>,
    /// The analyzer and its scratch window, rebuilt when the size changes.
    analyzer: Option<Analyzer>,
    mono: Vec<f32>,
    /// Smoothed activation per emitter, index-aligned with the config's
    /// list.
    levels: Vec<f32>,
    /// The fraction of a particle each emitter carried past the last tick,
    /// so a slow rate still fires at its average instead of rounding to
    /// zero.
    carry: Vec<f32>,
    /// Whether each burst emitter is ready to fire. Set once the band drops
    /// back under the threshold, cleared on the pop, so one onset throws one
    /// burst.
    armed: Vec<bool>,
    /// The binding signals, fed off the same spectrum as the emitters.
    signals: Signals,
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
            last_written: 0,
            last_tick: None,
            last_fresh: None,
            analyzer: None,
            mono: Vec::new(),
            levels: Vec::new(),
            carry: Vec::new(),
            armed: Vec::new(),
            signals: Signals::new(),
            particles: Vec::new(),
            clock: 0.0,
            rng: 0x9e37_79b9,
            alive: false,
        }
    }

    /// One tick: read the newest window off the feed, fold it into each
    /// emitter's activation, fire what the emitters call for, and move what
    /// is already in the air. `hold` is the freeze-on-pause option, which
    /// parks the field where it stands.
    fn step(&mut self, feed: &AudioFeed, w: f32, h: f32, config: &ParticlesConfig, hold: bool) {
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

        let written = feed.written();
        let fresh = written != self.last_written;
        self.last_written = written;
        if fresh {
            self.last_fresh = Some(now);
        }
        // Nothing new is just the gap between pump ticks until the feed has
        // sat still long enough to read as stopped; then the activations
        // fall away and the emitters stop firing.
        let stopped = self
            .last_fresh
            .is_none_or(|t| (now - t).as_secs_f32() > SILENT_AFTER);

        let size = config.scene.fft();
        if self.analyzer.as_ref().is_none_or(|a| a.size() != size) {
            self.analyzer = Some(Analyzer::new(size));
            self.mono = vec![0.0; size];
        }
        self.levels.resize(config.emitters.len(), 0.0);
        self.carry.resize(config.emitters.len(), 0.0);
        self.armed.resize(config.emitters.len(), true);

        // One transform per frame, shared by the emitters' activations and
        // the bindings' signals. A range's bin span is a couple of float
        // ops, so it is recomputed rather than cached against a mapping.
        // The read is its own scope: the magnitudes borrow the analyzer,
        // and firing below takes the whole sim.
        let rate = feed.sample_rate();
        let half = size / 2;
        let mut targets: Vec<Option<f32>> = vec![None; config.emitters.len()];
        let (emitters, scene, forces) = {
            let Sim {
                analyzer,
                mono,
                signals,
                ..
            } = self;
            let analyzer = analyzer.as_mut().expect("analyzer built above");
            let mags: Option<&[f32]> = if fresh && feed.latest_mono(mono) == mono.len() {
                Some(analyzer.magnitudes(mono))
            } else {
                None
            };
            if let Some(mags) = mags {
                for (target, emitter) in targets.iter_mut().zip(&config.emitters) {
                    let (freq_lo, freq_hi) = emitter.range();
                    let (lo, hi) = log_bands(1, freq_lo, freq_hi, rate, half)[0];
                    let mut peak = 0.0f32;
                    for &m in &mags[lo..hi] {
                        peak = peak.max(m);
                    }
                    let db = 20.0 * (peak + 1e-9).log10();
                    *target = Some(((db - FLOOR_DB) / (MAX_DB - FLOOR_DB)).clamp(0.0, 1.0));
                }
            }
            let values = signals.step(mags, rate, stopped, dt, &config.bindings);
            modulated(config, values)
        };

        for (i, emitter) in emitters.iter().enumerate() {
            if let Some(target) = targets[i] {
                let ease = if target > self.levels[i] {
                    ATTACK
                } else {
                    RELEASE
                };
                self.levels[i] += (target - self.levels[i]) * (ease * dt).min(1.0);
            } else if stopped {
                self.levels[i] += (0.0 - self.levels[i]) * (RELEASE * dt).min(1.0);
            }

            // How hard the emitter is firing: where its activation sits in
            // the stretch above its threshold, so one just clearing the bar
            // trickles and a pinned one runs at its full rate.
            let threshold = emitter.threshold();
            let drive = ((self.levels[i] - threshold) / (1.0 - threshold)).clamp(0.0, 1.0);
            if !emitter.enabled || drive <= 0.0 {
                self.carry[i] = 0.0;
                // A burst emitter re-arms once its band falls back under the
                // threshold, so the next transient fires a fresh pop.
                self.armed[i] = true;
                continue;
            }
            let color = emitter.color();
            let due = match emitter.mode {
                Trigger::Continuous => {
                    self.carry[i] += drive * emitter.rate() * dt;
                    let due = self.carry[i].floor();
                    self.carry[i] -= due;
                    due as usize
                }
                // The whole burst lands on the rising edge into the
                // threshold, then holds until the band drops and re-arms.
                Trigger::Burst if self.armed[i] => {
                    self.armed[i] = false;
                    emitter.burst().round() as usize
                }
                Trigger::Burst => 0,
            };
            for _ in 0..due {
                if self.particles.len() >= MAX_PARTICLES {
                    break;
                }
                self.spawn(emitter, w, h, drive, color, &scene);
            }
        }

        self.advance(w, h, dt, &scene, &forces);
    }

    /// Launch one particle for an emitter: somewhere on its footprint,
    /// headed the way it aims, scattered enough that a steady emitter reads
    /// as a plume rather than a line.
    fn spawn(&mut self, emitter: &Emitter, w: f32, h: f32, drive: f32, color: Rgba, scene: &Scene) {
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
        let speed = emitter.speed() * (0.6 + 0.4 * rand01(&mut self.rng)) * (0.4 + 0.6 * drive);
        let life = scene.life() * (0.7 + 0.6 * rand01(&mut self.rng));
        let size = scene.size() * (0.6 + 0.8 * rand01(&mut self.rng));
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

/// A thin live meter for the customize window: the value read off the sim
/// at paint time, so tuning happens against the signal itself instead of
/// blind. Keeps frames coming while the audio is fresh or the field is
/// settling, since that window renders on its own clock, not the panel's.
fn meter(
    sim: Arc<Mutex<Sim>>,
    read: impl Fn(&Sim) -> f32 + 'static,
    fill: Rgba,
    marker: Option<f32>,
) -> Div {
    div().h(px(6.)).w_full().child(
        canvas(
            move |_, _, _| {},
            move |bounds, _, window, _| {
                let (value, live) = {
                    let sim = sim.lock().unwrap();
                    let live = sim.alive
                        || sim
                            .last_fresh
                            .is_some_and(|t| t.elapsed().as_secs_f32() < 0.3);
                    (read(&sim).clamp(0.0, 1.0), live)
                };
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
    lo: ScrubState,
    hi: ScrubState,
    threshold: ScrubState,
    rate: ScrubState,
    burst: ScrubState,
    x: ScrubState,
    y: ScrubState,
    width: ScrubState,
    height: ScrubState,
    rotation: ScrubState,
    direction: ScrubState,
    cone: ScrubState,
    speed: ScrubState,
}

/// One binding's slider state, the [`EmitterScrubs`] arrangement.
#[derive(Default)]
struct BindingScrubs {
    lo: ScrubState,
    hi: ScrubState,
    smooth: ScrubState,
    from: ScrubState,
    to: ScrubState,
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
    /// Per-binding slider state, kept the same length as the list.
    binding_scrubs: Vec<BindingScrubs>,
    gravity_scrub: ScrubState,
    gravity_angle_scrub: ScrubState,
    drag_scrub: ScrubState,
    life_scrub: ScrubState,
    size_scrub: ScrubState,
    turbulence_scrub: ScrubState,
    turb_scale_scrub: ScrubState,
    turb_speed_scrub: ScrubState,
    focus: FocusHandle,
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
            binding_scrubs: Vec::new(),
            gravity_scrub: ScrubState::default(),
            gravity_angle_scrub: ScrubState::default(),
            drag_scrub: ScrubState::default(),
            life_scrub: ScrubState::default(),
            size_scrub: ScrubState::default(),
            turbulence_scrub: ScrubState::default(),
            turb_scale_scrub: ScrubState::default(),
            turb_speed_scrub: ScrubState::default(),
            focus: cx.focus_handle(),
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

    fn add_binding(&mut self, cx: &mut Context<Self>) {
        self.config.bindings.push(Binding {
            target: "turbulence".into(),
            ..Binding::default()
        });
        cx.notify();
    }

    fn remove_binding(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.config.bindings.len() {
            self.config.bindings.remove(index);
            cx.notify();
        }
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
            ("Bindings", icons::AUDIO_WAVEFORM),
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
        match page {
            "Bindings" => self.bindings_page(cx).into_any_element(),
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
        let (freq_lo, freq_hi) = emitter.range();
        let threshold = emitter.threshold();
        let rate = emitter.rate();
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
        let color = emitter.color();

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(format!("Emitter {}", index + 1)),
            )
            .child(
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
            // The band's live activation against the threshold tick, so the
            // threshold tunes against the music instead of blind.
            .child(meter(
                self.sim.clone(),
                move |sim| sim.levels.get(index).copied().unwrap_or(0.0),
                color,
                Some(threshold),
            ))
            .child(setting_row(
                "Low Bound",
                None,
                panel::value_slider(
                    &scrubs.lo,
                    hz_to_frac(freq_lo),
                    fmt_hz(freq_lo),
                    move |this: &mut Self, fraction, cx| {
                        let Some(emitter) = this.config.emitters.get_mut(index) else {
                            return;
                        };
                        // The low bound stops a min-span short of the high
                        // one, so the range never inverts as the strip drags
                        // past it.
                        let hi = emitter.freq_hi.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
                        let ceil = (hi / MIN_RATIO).max(SLIDER_MIN_HZ);
                        emitter.freq_lo = frac_to_hz(fraction).clamp(SLIDER_MIN_HZ, ceil);
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "High Bound",
                None,
                panel::value_slider(
                    &scrubs.hi,
                    hz_to_frac(freq_hi),
                    fmt_hz(freq_hi),
                    move |this: &mut Self, fraction, cx| {
                        let Some(emitter) = this.config.emitters.get_mut(index) else {
                            return;
                        };
                        let lo = emitter.freq_lo.clamp(SLIDER_MIN_HZ, SLIDER_MAX_HZ);
                        let floor = (lo * MIN_RATIO).min(SLIDER_MAX_HZ);
                        emitter.freq_hi = frac_to_hz(fraction).clamp(floor, SLIDER_MAX_HZ);
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Threshold",
                None,
                panel::value_slider(
                    &scrubs.threshold,
                    threshold,
                    format!("{}%", (threshold * 100.0).round() as i32),
                    move |this: &mut Self, fraction, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.threshold = fraction.clamp(0.0, 0.99);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
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
                d.child(setting_row(
                    "Rate",
                    None,
                    panel::value_slider(
                        &scrubs.rate,
                        (rate - RATE_MIN) / (RATE_MAX - RATE_MIN),
                        format!("{rate:.0}/s"),
                        move |this: &mut Self, fraction, cx| {
                            if let Some(emitter) = this.config.emitters.get_mut(index) {
                                emitter.rate = RATE_MIN + fraction * (RATE_MAX - RATE_MIN);
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .when(mode == Trigger::Burst, |d| {
                d.child(setting_row(
                    "Burst",
                    None,
                    panel::value_slider(
                        &scrubs.burst,
                        (burst - BURST_MIN) / (BURST_MAX - BURST_MIN),
                        format!("{burst:.0}"),
                        move |this: &mut Self, fraction, cx| {
                            if let Some(emitter) = this.config.emitters.get_mut(index) {
                                emitter.burst = BURST_MIN + fraction * (BURST_MAX - BURST_MIN);
                            }
                            cx.notify();
                        },
                        cx,
                    ),
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
                panel::value_slider(
                    &scrubs.x,
                    x,
                    format!("{}%", (x * 100.0).round() as i32),
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
                panel::value_slider(
                    &scrubs.y,
                    y,
                    format!("{}%", (y * 100.0).round() as i32),
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
                    panel::value_slider(
                        &scrubs.width,
                        width / 2.0,
                        format!("{}%", (width * 100.0).round() as i32),
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
                    panel::value_slider(
                        &scrubs.height,
                        height / 2.0,
                        format!("{}%", (height * 100.0).round() as i32),
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
                    panel::value_slider(
                        &scrubs.rotation,
                        rotation / 360.0,
                        format!("{rotation:.0}°"),
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
                    panel::value_slider(
                        &scrubs.direction,
                        direction / 360.0,
                        format!("{direction:.0}°"),
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
            .child(setting_row(
                "Cone",
                None,
                panel::value_slider(
                    &scrubs.cone,
                    cone / 360.0,
                    format!("{cone:.0}°"),
                    move |this: &mut Self, fraction, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.cone = fraction.clamp(0.0, 1.0) * 360.0;
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Speed",
                None,
                panel::value_slider(
                    &scrubs.speed,
                    (speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN),
                    format!("{speed:.0} px/s"),
                    move |this: &mut Self, fraction, cx| {
                        if let Some(emitter) = this.config.emitters.get_mut(index) {
                            emitter.speed = SPEED_MIN + fraction * (SPEED_MAX - SPEED_MIN);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row("Color", None, color_row))
    }

    /// The Bindings page: routes from the music into the scene and force
    /// knobs, each one a block of its own rows.
    fn bindings_page(&mut self, cx: &mut Context<Self>) -> Div {
        let count = self.config.bindings.len();
        if self.binding_scrubs.len() != count {
            self.binding_scrubs
                .resize_with(count, BindingScrubs::default);
        }
        let add = settings_ui::small_button(
            "Add Binding",
            icons::PLUS,
            false,
            cx.listener(|this, _, _, cx| this.add_binding(cx)),
        );
        let mut list = div().flex().flex_col().gap(tokens::SPACE_MD);
        if count == 0 {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child("No bindings yet - add one to drive a knob with the music."),
            );
        }
        for i in 0..count {
            list = list.child(self.binding_block(i, cx));
        }
        div().flex().flex_col().gap(SECTION_GAP).child(section(
            "Bindings",
            Some(add.into_any_element()),
            list,
        ))
    }

    /// One binding's block: the header carrying its switch and delete, then
    /// what it listens to, how it responds, and the span it sweeps. While a
    /// binding drives a knob, that knob's own slider sets nothing; the
    /// binding's span is the whole say.
    fn binding_block(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let binding = &self.config.bindings[index];
        let scrubs = &self.binding_scrubs[index];
        // The target splits into a scope (the scene, or one emitter) and a
        // knob within it. A route whose emitter is gone reads as Scene here
        // and stays quiet in the sim until the next pick rewrites it.
        let scope = emitter_route(&binding.target)
            .map(|(id, _)| id)
            .filter(|id| self.config.emitters.iter().any(|e| e.id == *id));
        let scene_knob = TARGET_CHOICES
            .iter()
            .map(|(_, id)| *id)
            .find(|id| *id == binding.target)
            .unwrap_or("turbulence");
        let emitter_knob = emitter_route(&binding.target)
            .and_then(|(_, knob)| {
                EMITTER_KNOB_CHOICES
                    .iter()
                    .map(|(_, k)| *k)
                    .find(|k| *k == knob)
            })
            .unwrap_or("speed");
        let (kind, freq_lo, freq_hi) = match binding.source {
            Source::Band { lo, hi } => (SourceKind::Band, lo, hi),
            Source::Onset { lo, hi } => (SourceKind::Onset, lo, hi),
            Source::Level => (SourceKind::Level, 30.0, 120.0),
        };
        let smooth = binding.smooth.clamp(0.0, 1.0);
        let from = binding.from.clamp(0.0, 1.0);
        let to = binding.to.clamp(0.0, 1.0);

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(format!("Binding {}", index + 1)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .child(toggle(
                        binding.enabled,
                        move |this: &mut Self, on, cx| {
                            if let Some(binding) = this.config.bindings.get_mut(index) {
                                binding.enabled = on;
                            }
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(settings_ui::icon_button(
                        icons::TRASH,
                        false,
                        cx.listener(move |this, _, _, cx| this.remove_binding(index, cx)),
                    )),
            );

        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(header)
            // The route's live signal, so the span and smoothing tune
            // against what the music is actually sending.
            .child(meter(
                self.sim.clone(),
                move |sim| sim.signals.values().get(index).copied().unwrap_or(0.0),
                palette::accent(),
                None,
            ))
            .child(panel::setting_block(
                "Target",
                Some("The knob the signal drives; its own slider yields while bound"),
                None,
                {
                    let mut col = div().flex().flex_col().gap(tokens::SPACE_XS);
                    if !self.config.emitters.is_empty() {
                        let mut row =
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap(px(1.))
                                .child(scope_chip(
                                    "Scene".to_string(),
                                    scope.is_none(),
                                    move |this, cx| {
                                        if let Some(binding) = this.config.bindings.get_mut(index) {
                                            if emitter_route(&binding.target).is_some() {
                                                binding.target = "turbulence".to_string();
                                            }
                                        }
                                        cx.notify();
                                    },
                                    cx,
                                ));
                        for (n, emitter) in self.config.emitters.iter().enumerate() {
                            let id = emitter.id;
                            row = row.child(scope_chip(
                                format!("Emitter {}", n + 1),
                                scope == Some(id),
                                move |this, cx| {
                                    if let Some(binding) = this.config.bindings.get_mut(index) {
                                        if emitter_route(&binding.target)
                                            .is_none_or(|(prev, _)| prev != id)
                                        {
                                            binding.target = format!("e{id}.speed");
                                        }
                                    }
                                    cx.notify();
                                },
                                cx,
                            ));
                        }
                        col = col.child(row);
                    }
                    col.child(if let Some(id) = scope {
                        panel::choices(
                            EMITTER_KNOB_CHOICES,
                            emitter_knob,
                            move |this: &mut Self, knob, cx| {
                                if let Some(binding) = this.config.bindings.get_mut(index) {
                                    binding.target = format!("e{id}.{knob}");
                                }
                                cx.notify();
                            },
                            cx,
                        )
                    } else {
                        panel::choices(
                            TARGET_CHOICES,
                            scene_knob,
                            move |this: &mut Self, target, cx| {
                                if let Some(binding) = this.config.bindings.get_mut(index) {
                                    binding.target = target.to_string();
                                }
                                cx.notify();
                            },
                            cx,
                        )
                        .flex_wrap()
                    })
                },
            ))
            .child(setting_row(
                "Source",
                None,
                panel::choices(
                    SOURCE_CHOICES,
                    kind,
                    move |this: &mut Self, kind, cx| {
                        let Some(binding) = this.config.bindings.get_mut(index) else {
                            return;
                        };
                        // Switching kinds carries the band along, so Band to
                        // Onset keeps the range the ear already picked.
                        let (lo, hi) = match binding.source {
                            Source::Band { lo, hi } | Source::Onset { lo, hi } => (lo, hi),
                            Source::Level => (30.0, 120.0),
                        };
                        binding.source = match kind {
                            SourceKind::Band => Source::Band { lo, hi },
                            SourceKind::Onset => Source::Onset { lo, hi },
                            SourceKind::Level => Source::Level,
                        };
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(kind != SourceKind::Level, |d| {
                d.child(setting_row(
                    "Low Bound",
                    None,
                    panel::value_slider(
                        &scrubs.lo,
                        hz_to_frac(freq_lo),
                        fmt_hz(freq_lo),
                        move |this: &mut Self, fraction, cx| {
                            let Some(binding) = this.config.bindings.get_mut(index) else {
                                return;
                            };
                            if let Source::Band { lo, hi } | Source::Onset { lo, hi } =
                                &mut binding.source
                            {
                                let ceil = (*hi / MIN_RATIO).max(SLIDER_MIN_HZ);
                                *lo = frac_to_hz(fraction).clamp(SLIDER_MIN_HZ, ceil);
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(setting_row(
                    "High Bound",
                    None,
                    panel::value_slider(
                        &scrubs.hi,
                        hz_to_frac(freq_hi),
                        fmt_hz(freq_hi),
                        move |this: &mut Self, fraction, cx| {
                            let Some(binding) = this.config.bindings.get_mut(index) else {
                                return;
                            };
                            if let Source::Band { lo, hi } | Source::Onset { lo, hi } =
                                &mut binding.source
                            {
                                let floor = (*lo * MIN_RATIO).min(SLIDER_MAX_HZ);
                                *hi = frac_to_hz(fraction).clamp(floor, SLIDER_MAX_HZ);
                            }
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
                panel::value_slider(
                    &scrubs.smooth,
                    smooth,
                    format!("{}%", (smooth * 100.0).round() as i32),
                    move |this: &mut Self, fraction, cx| {
                        if let Some(binding) = this.config.bindings.get_mut(index) {
                            binding.smooth = fraction.clamp(0.0, 1.0);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Quiet",
                Some("Where the knob sits at silence"),
                panel::value_slider(
                    &scrubs.from,
                    from,
                    format!("{}%", (from * 100.0).round() as i32),
                    move |this: &mut Self, fraction, cx| {
                        if let Some(binding) = this.config.bindings.get_mut(index) {
                            binding.from = fraction.clamp(0.0, 1.0);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Loud",
                Some("Where it sits at full signal; below Quiet modulates down"),
                panel::value_slider(
                    &scrubs.to,
                    to,
                    format!("{}%", (to * 100.0).round() as i32),
                    move |this: &mut Self, fraction, cx| {
                        if let Some(binding) = this.config.bindings.get_mut(index) {
                            binding.to = fraction.clamp(0.0, 1.0);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
    }

    /// The Forces page: the drift laid over the scene's steady pull.
    fn forces_page(&mut self, cx: &mut Context<Self>) -> Div {
        let turbulence = self.config.forces.turbulence();
        let scale = self.config.forces.scale();
        let speed = self.config.forces.speed();
        div().flex().flex_col().gap(SECTION_GAP).child(section(
            "Turbulence",
            None,
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(setting_row(
                    "Strength",
                    Some("How hard the field pushes particles around; zero is off"),
                    panel::value_slider(
                        &self.turbulence_scrub,
                        turbulence / TURB_MAX,
                        format!("{turbulence:.0}"),
                        |this: &mut Self, fraction, cx| {
                            this.config.forces.turbulence = fraction * TURB_MAX;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(setting_row(
                    "Scale",
                    Some("How wide one swirl runs; small churns, large rolls"),
                    panel::value_slider(
                        &self.turb_scale_scrub,
                        (scale - TURB_SCALE_MIN) / (TURB_SCALE_MAX - TURB_SCALE_MIN),
                        format!("{scale:.0} px"),
                        |this: &mut Self, fraction, cx| {
                            this.config.forces.turbulence_scale =
                                TURB_SCALE_MIN + fraction * (TURB_SCALE_MAX - TURB_SCALE_MIN);
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(setting_row(
                    "Drift",
                    Some("How fast the field itself moves, so the swirls don't stand still"),
                    panel::value_slider(
                        &self.turb_speed_scrub,
                        speed / TURB_SPEED_MAX,
                        format!("{speed:.2}"),
                        |this: &mut Self, fraction, cx| {
                            this.config.forces.turbulence_speed = fraction * TURB_SPEED_MAX;
                            cx.notify();
                        },
                        cx,
                    ),
                )),
        ))
    }

    /// The Scene page: what the whole field sits in, apart from any one
    /// emitter.
    fn scene_page(&mut self, cx: &mut Context<Self>) -> Div {
        let gravity = self.config.scene.gravity();
        let angle = self.config.scene.gravity_angle.rem_euclid(360.0);
        let drag = self.config.scene.drag();
        let life = self.config.scene.life();
        let particle_size = self.config.scene.size();
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
                    .child(setting_row(
                        "Strength",
                        Some("Constant pull on everything in flight"),
                        panel::value_slider(
                            &self.gravity_scrub,
                            gravity / GRAVITY_MAX,
                            format!("{gravity:.0}"),
                            |this: &mut Self, fraction, cx| {
                                this.config.scene.gravity = fraction * GRAVITY_MAX;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        "Direction",
                        Some("Which way it pulls; 0 is up, 180 is down"),
                        panel::value_slider(
                            &self.gravity_angle_scrub,
                            angle / 360.0,
                            format!("{angle:.0}°"),
                            |this: &mut Self, fraction, cx| {
                                this.config.scene.gravity_angle = fraction.clamp(0.0, 1.0) * 360.0;
                                cx.notify();
                            },
                            cx,
                        ),
                    )),
            ))
            .child(section(
                "Medium",
                None,
                setting_row(
                    "Drag",
                    Some("How much speed the air eats each second; zero is a vacuum"),
                    panel::value_slider(
                        &self.drag_scrub,
                        drag / DRAG_MAX,
                        format!("{drag:.2}"),
                        |this: &mut Self, fraction, cx| {
                            this.config.scene.drag = fraction * DRAG_MAX;
                            cx.notify();
                        },
                        cx,
                    ),
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
                        "Size",
                        Some("Particle size, varied a little per particle"),
                        panel::value_slider(
                            &self.size_scrub,
                            (particle_size - SIZE_MIN) / (SIZE_MAX - SIZE_MIN),
                            format!("{particle_size:.0} px"),
                            |this: &mut Self, fraction, cx| {
                                this.config.scene.size =
                                    SIZE_MIN + fraction * (SIZE_MAX - SIZE_MIN);
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        "Lifetime",
                        Some("How long a particle lasts before it fades out"),
                        panel::value_slider(
                            &self.life_scrub,
                            (life - LIFE_MIN) / (LIFE_MAX - LIFE_MIN),
                            format!("{life:.1} s"),
                            |this: &mut Self, fraction, cx| {
                                this.config.scene.life =
                                    LIFE_MIN + fraction * (LIFE_MAX - LIFE_MIN);
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
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
                "Analysis",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(setting_row(
                        "FFT Size",
                        Some("Analysis window; short reacts fast, long resolves finer"),
                        panel::choices(
                            FFT_CHOICES,
                            self.config.scene.fft(),
                            |this: &mut Self, size, cx| {
                                this.config.scene.fft_size = size;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
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
                    sim.step(&feed, w, h, &config, hold);
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
