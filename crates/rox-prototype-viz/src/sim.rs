//! The flow-field simulation, on its own thread. Mirrors the shape the real
//! visualizer will have: the sim never touches the UI, it publishes frames
//! into a latest-wins slot and a slow consumer just sees fewer of them.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::noise::Perlin;

pub const TRAIL: usize = 8;
pub const BANDS: usize = 16;
pub const BLIT_W: usize = 960;
pub const BLIT_H: usize = 540;

pub const PARTICLE_STEPS: [u32; 4] = [1_500, 3_000, 6_000, 12_000];

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Lines,
    Blit,
}

/// Sim-to-UI handoff. Latest-wins: the sim replaces, the UI takes.
pub struct Shared {
    pub mode: AtomicU8,
    pub particle_count: AtomicU32,
    pub latest: Mutex<Option<SimFrame>>,
    /// Set by the UI when the view drops; the sim thread exits.
    pub stop: AtomicBool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            mode: AtomicU8::new(0),
            particle_count: AtomicU32::new(PARTICLE_STEPS[2]),
            latest: Mutex::new(None),
            stop: AtomicBool::new(false),
        }
    }

    pub fn mode(&self) -> Mode {
        if self.mode.load(Ordering::Relaxed) == 0 {
            Mode::Lines
        } else {
            Mode::Blit
        }
    }

    pub fn toggle_mode(&self) {
        self.mode.fetch_xor(1, Ordering::Relaxed);
    }

    pub fn cycle_particles(&self) -> u32 {
        let current = self.particle_count.load(Ordering::Relaxed);
        let i = PARTICLE_STEPS
            .iter()
            .position(|&c| c == current)
            .unwrap_or(2);
        let next = PARTICLE_STEPS[(i + 1) % PARTICLE_STEPS.len()];
        self.particle_count.store(next, Ordering::Relaxed);
        next
    }
}

pub struct SimFrame {
    pub sim_ms: f32,
    pub payload: Payload,
}

pub enum Payload {
    /// Trails in unit coordinates, oldest point first, bucketed by intensity.
    Trails(Vec<Trail>),
    /// A BGRA8 buffer, alpha opaque, ready to wrap in a RenderImage.
    Pixels { w: usize, h: usize, bgra: Vec<u8> },
}

pub struct Trail {
    pub pts: [(f32, f32); TRAIL],
    pub bucket: u8,
}

struct Particle {
    x: f32,
    y: f32,
    trail: [(f32, f32); TRAIL],
    head: usize,
    energy: f32,
    receptivity: f32,
    life: f32,
}

struct Rng(u64);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Synthetic spectrum: a kick on the low bands, noise-driven shimmer on the
/// highs, a slow swell over the whole thing. Stands in for the FFT of the
/// PCM tap until a playback engine exists.
fn synth_bands(t: f32, noise: &Perlin) -> [f32; BANDS] {
    let kick = (1.0 - (t * 2.13).fract()).powi(3);
    let swell = 0.55 + 0.45 * (t * 0.55).sin();
    std::array::from_fn(|i| {
        let f = i as f32 / (BANDS - 1) as f32;
        let low = (1.0 - f) * (1.0 - f);
        let shimmer = 0.5 + 0.5 * noise.noise(t * (1.0 + 3.0 * f), i as f32 * 7.31, 4.7);
        ((kick * low + shimmer * f * 0.6) * swell).clamp(0.0, 1.0)
    })
}

pub fn spawn(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("viz-sim".into())
        .spawn(move || run(shared))
        .expect("failed to spawn sim thread");
}

fn run(shared: Arc<Shared>) {
    let noise = Perlin::new(0x726f78);
    let mut rng = Rng(0x5eed);
    let mut particles: Vec<Particle> = Vec::new();
    let mut framebuffer = vec![0u8; BLIT_W * BLIT_H * 4];
    for px in framebuffer.chunks_exact_mut(4) {
        px[3] = 255;
    }

    const DT: f32 = 1.0 / 60.0;
    const FIELD_SCALE: f32 = 2.6;
    let mut t = 0.0f32;
    let mut z = 0.0f32;

    while !shared.stop.load(Ordering::Relaxed) {
        let tick_start = Instant::now();

        let want = shared.particle_count.load(Ordering::Relaxed) as usize;
        while particles.len() < want {
            particles.push(spawn_particle(&mut rng));
        }
        particles.truncate(want);

        let bands = synth_bands(t, &noise);
        let bass = bands[..4].iter().sum::<f32>() / 4.0;
        let treble = bands[BANDS - 4..].iter().sum::<f32>() / 4.0;

        // Bass pushes the field: faster flow, faster evolution.
        z += DT * (0.12 + 0.9 * bass);
        let speed = 0.05 + 0.30 * bass;

        for p in particles.iter_mut() {
            let (u, v) = noise.curl(p.x * FIELD_SCALE, p.y * FIELD_SCALE, z);
            p.x += u * speed * DT;
            p.y += v * speed * DT;
            p.head = (p.head + 1) % TRAIL;
            p.trail[p.head] = (p.x, p.y);
            p.energy = (p.energy * (1.0 - 1.8 * DT)
                + (bass * p.receptivity + treble * (1.0 - p.receptivity)) * 2.2 * DT)
                .clamp(0.0, 1.0);
            p.life -= DT;
            let out = !(-0.02..=1.02).contains(&p.x) || !(-0.02..=1.02).contains(&p.y);
            if out || p.life <= 0.0 {
                *p = spawn_particle(&mut rng);
            }
        }

        let mode = shared.mode();
        let payload = match mode {
            Mode::Lines => Payload::Trails(
                particles
                    .iter()
                    .map(|p| {
                        let pts = std::array::from_fn(|i| p.trail[(p.head + 1 + i) % TRAIL]);
                        Trail {
                            pts,
                            bucket: (p.energy * 4.99) as u8,
                        }
                    })
                    .collect(),
            ),
            Mode::Blit => {
                rasterize(&mut framebuffer, &particles);
                Payload::Pixels {
                    w: BLIT_W,
                    h: BLIT_H,
                    bgra: framebuffer.clone(),
                }
            }
        };

        let sim_ms = tick_start.elapsed().as_secs_f32() * 1000.0;
        *shared.latest.lock().unwrap() = Some(SimFrame { sim_ms, payload });

        t += DT;
        let elapsed = tick_start.elapsed();
        if elapsed < Duration::from_secs_f32(DT) {
            std::thread::sleep(Duration::from_secs_f32(DT) - elapsed);
        }
    }
}

fn spawn_particle(rng: &mut Rng) -> Particle {
    let (x, y) = (rng.next_f32(), rng.next_f32());
    Particle {
        x,
        y,
        trail: [(x, y); TRAIL],
        head: 0,
        energy: 0.0,
        receptivity: rng.next_f32(),
        life: 3.0 + 5.0 * rng.next_f32(),
    }
}

/// Fade the persistent buffer toward black, then splat each particle's last
/// step as an additive line segment. Trails come from the fade, not from
/// history, which is what gives the smoke-like look.
fn rasterize(buf: &mut [u8], particles: &[Particle]) {
    for px in buf.chunks_exact_mut(4) {
        px[0] = (px[0] as u16 * 242 / 256) as u8;
        px[1] = (px[1] as u16 * 242 / 256) as u8;
        px[2] = (px[2] as u16 * 242 / 256) as u8;
    }
    for p in particles {
        let (x1, y1) = p.trail[p.head];
        let (x0, y0) = p.trail[(p.head + TRAIL - 1) % TRAIL];
        let e = 0.25 + 0.75 * p.energy;
        // BGRA, green-teal like the reference look.
        let add = [(90.0 * e) as u16, (230.0 * e) as u16, (45.0 * e) as u16];
        let (px0, py0) = (x0 * BLIT_W as f32, y0 * BLIT_H as f32);
        let (px1, py1) = (x1 * BLIT_W as f32, y1 * BLIT_H as f32);
        let steps = ((px1 - px0).abs().max((py1 - py0).abs()).ceil() as usize).clamp(1, 32);
        for s in 0..=steps {
            let f = s as f32 / steps as f32;
            let x = px0 + (px1 - px0) * f;
            let y = py0 + (py1 - py0) * f;
            if x < 0.0 || y < 0.0 || x >= BLIT_W as f32 || y >= BLIT_H as f32 {
                continue;
            }
            let i = (y as usize * BLIT_W + x as usize) * 4;
            buf[i] = (buf[i] as u16 + add[0]).min(255) as u8;
            buf[i + 1] = (buf[i + 1] as u16 + add[1]).min(255) as u8;
            buf[i + 2] = (buf[i + 2] as u16 + add[2]).min(255) as u8;
        }
    }
}
