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
    Entity, EventEmitter, FocusHandle, Focusable, Rgba, SharedString, Subscription, WeakEntity,
    Window,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Sizable as _;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::analysis::{log_bands, Analyzer, MAX_FFT_SIZE, MIN_FFT_SIZE};
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

/// One emitter: the range it listens to, how loud that range has to get
/// before it fires, where it sits, and which way it throws.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Emitter {
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
            enabled: true,
            freq_lo: 30.0,
            freq_hi: 120.0,
            threshold: 0.35,
            rate: 60.0,
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
        self.levels.truncate(config.emitters.len());
        self.carry.truncate(config.emitters.len());

        // One transform per frame, pooled per emitter. A range's bin span is
        // a couple of float ops, so it is recomputed rather than cached
        // against a mapping. The read is its own scope: the magnitudes
        // borrow the analyzer, and firing below takes the whole sim.
        let rate = feed.sample_rate();
        let half = size / 2;
        let mut targets: Vec<Option<f32>> = vec![None; config.emitters.len()];
        {
            let Sim { analyzer, mono, .. } = self;
            let analyzer = analyzer.as_mut().expect("analyzer built above");
            if fresh && feed.latest_mono(mono) == mono.len() {
                let mags = analyzer.magnitudes(mono);
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
        }

        for (i, emitter) in config.emitters.iter().enumerate() {
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
                continue;
            }
            self.carry[i] += drive * emitter.rate() * dt;
            let due = self.carry[i].floor();
            self.carry[i] -= due;
            let color = emitter.color();
            for _ in 0..(due as usize) {
                if self.particles.len() >= MAX_PARTICLES {
                    break;
                }
                self.spawn(emitter, w, h, drive, color, &config.scene);
            }
        }

        self.advance(w, h, dt, &config.scene, &config.forces);
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

/// The settings sliders' painted bounds and drag state for one emitter, one
/// per slider so a drag on one never moves the others.
#[derive(Default)]
struct EmitterScrubs {
    lo: ScrubState,
    hi: ScrubState,
    threshold: ScrubState,
    rate: ScrubState,
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
    gravity_scrub: ScrubState,
    gravity_angle_scrub: ScrubState,
    drag_scrub: ScrubState,
    life_scrub: ScrubState,
    size_scrub: ScrubState,
    turbulence_scrub: ScrubState,
    turb_scale_scrub: ScrubState,
    turb_speed_scrub: ScrubState,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and
    /// pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel when a session starts, so an idle window resumes
    /// animating without the player bar's frame pump.
    _player_changed: Subscription,
}

impl ParticlesPanel {
    pub fn new(state: AppState, config: ParticlesConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        ParticlesPanel {
            config,
            feed: state.player.read(cx).feed(),
            state,
            sim: Arc::new(Mutex::new(Sim::new())),
            emitter_scrubs: Vec::new(),
            emitter_pickers: Vec::new(),
            _emitter_changes: Vec::new(),
            gravity_scrub: ScrubState::default(),
            gravity_angle_scrub: ScrubState::default(),
            drag_scrub: ScrubState::default(),
            life_scrub: ScrubState::default(),
            size_scrub: ScrubState::default(),
            turbulence_scrub: ScrubState::default(),
            turb_scale_scrub: ScrubState::default(),
            turb_speed_scrub: ScrubState::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    fn add_emitter(&mut self, cx: &mut Context<Self>) {
        let emitter = Emitter::next_after(self.config.emitters.last());
        self.config.emitters.push(emitter);
        cx.notify();
    }

    fn remove_emitter(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.config.emitters.len() {
            self.config.emitters.remove(index);
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
        div().size_full().relative().bg(palette::bg_root()).child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    if w <= 0.0 || h <= 0.0 {
                        return;
                    }
                    let mut sim = sim.lock().unwrap();
                    sim.step(&feed, w, h, &config, hold);
                    sim.paint(bounds, window, &config.scene);
                },
            )
            .size_full(),
        )
    }
}
