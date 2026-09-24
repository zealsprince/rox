//! Preset thumbnails: a small engine renders each preset for under a second
//! against a synthetic tone and writes a frame to a PNG cache.
//!
//! On demand, not a scan: a pack can hold a hundred thousand presets, and a
//! full pass is hours of GPU. The browser asks for what's on screen, each ask
//! replacing the last. The tone exists because several presets draw nothing
//! on silence.
//!
//! Engines run side by side, each worker owning its engine and context. How
//! many is measured: workers are added while each engine still delivers
//! `HEALTHY_FRAMES` per hold, staggered so thumbnails trickle in, and dropped
//! after an idle spell so a closed browser costs no GL context.

use std::collections::{HashMap, HashSet};
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::{Command, Engine, EngineOptions, Event, PresetLibrary, Status};

/// 16:9 at the worker's minimum render size (128 a side); enough to tell a
/// fractal from a waveform.
pub const WIDTH: u32 = 256;
pub const HEIGHT: u32 = 144;

/// Long enough for a preset that builds up over time to have built something.
const HOLD: Duration = Duration::from_millis(900);

const LOAD_TIMEOUT: Duration = Duration::from_secs(6);

const IDLE: Duration = Duration::from_secs(30);

const CONTEXT_TIMEOUT: Duration = Duration::from_secs(20);

/// High, since a thumbnail wants as many frames as it can get inside the hold.
const FPS: u32 = 120;

/// Past this the app's own visual starts paying for the engines.
const MAX_WORKERS: usize = 8;

/// Two, so the first screen already gets the stagger.
const FIRST_WORKERS: usize = 2;

/// A free GPU manages about a hundred per hold; a third of that still has
/// room for one more engine.
const HEALTHY_FRAMES: f64 = 36.0;

const WARMUP_JOBS: u64 = 2;

fn max_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, MAX_WORKERS)
}

const TONE_RATE: u32 = 48_000;
const TONE_STEP: Duration = Duration::from_millis(20);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Thumb {
    Ready(PathBuf),
    Pending,
    /// Not asked for again this run, so a broken preset doesn't spin the worker.
    Failed(String),
}

struct State {
    /// Replaced whole on every ask.
    queue: Vec<PathBuf>,
    active: HashSet<PathBuf>,
    /// Known to be on disk, so a row doesn't stat per render.
    ready: HashSet<PathBuf>,
    failed: HashMap<PathBuf, String>,
    running: usize,
    /// Ever started, for the stagger.
    started: usize,
    frames_per_hold: f64,
    jobs: u64,
}

struct Inner {
    dir: PathBuf,
    textures: Option<PathBuf>,
    state: Mutex<State>,
    wake: Condvar,
    generation: AtomicU64,
}

/// Cheap to clone; every clone is the same queue and cache.
#[derive(Clone)]
pub struct Thumbnailer {
    inner: Arc<Inner>,
}

impl Thumbnailer {
    pub fn new(dir: PathBuf, textures: Option<PathBuf>) -> Thumbnailer {
        Thumbnailer {
            inner: Arc::new(Inner {
                dir,
                textures,
                state: Mutex::new(State {
                    queue: Vec::new(),
                    active: HashSet::new(),
                    ready: HashSet::new(),
                    failed: HashMap::new(),
                    running: 0,
                    started: 0,
                    frames_per_hold: 0.0,
                    jobs: 0,
                }),
                wake: Condvar::new(),
                generation: AtomicU64::new(0),
            }),
        }
    }

    /// Keyed by a hash of the path: names carry characters no filesystem
    /// wants, and two packs can share a name.
    pub fn thumb_path(&self, preset: &Path) -> PathBuf {
        self.inner
            .dir
            .join(format!("{:016x}.png", path_hash(preset)))
    }

    /// On disk so a preset that won't compile isn't retried every run.
    fn failed_path(&self, preset: &Path) -> PathBuf {
        self.thumb_path(preset).with_extension("failed")
    }

    pub fn thumb(&self, preset: &Path) -> Thumb {
        let mut state = self.inner.state.lock().unwrap();
        if let Some(message) = state.failed.get(preset) {
            return Thumb::Failed(message.clone());
        }
        let path = self.thumb_path(preset);
        if state.ready.contains(preset) {
            return Thumb::Ready(path);
        }
        if path.is_file() {
            state.ready.insert(preset.to_path_buf());
            return Thumb::Ready(path);
        }
        if let Ok(message) = std::fs::read_to_string(self.failed_path(preset)) {
            state.failed.insert(preset.to_path_buf(), message.clone());
            return Thumb::Failed(message);
        }
        Thumb::Pending
    }

    /// A real engine loaded it, so any failure note is stale.
    pub fn loaded(&self, preset: &Path) {
        let mut state = self.inner.state.lock().unwrap();
        state.failed.remove(preset);
        // The note may be on disk from an earlier run, unread this one.
        std::fs::remove_file(self.failed_path(preset)).ok();
    }

    /// Replaces the queue with what's on screen. An empty ask lets the workers
    /// wind down.
    pub fn want(&self, presets: Vec<PathBuf>) {
        let mut state = self.inner.state.lock().unwrap();
        let queue: Vec<PathBuf> = presets
            .into_iter()
            .filter(|preset| {
                !state.ready.contains(preset)
                    && !state.failed.contains_key(preset)
                    && !state.active.contains(preset)
            })
            .filter(|preset| !self.thumb_path(preset).is_file())
            .collect();
        state.queue = queue;
        if state.queue.is_empty() {
            return;
        }
        let wanted = FIRST_WORKERS.min(state.queue.len());
        while state.running < wanted {
            spawn_worker(&self.inner, &mut state);
        }
        self.inner.wake.notify_all();
    }

    pub fn stats(&self) -> (f64, usize) {
        let state = self.inner.state.lock().unwrap();
        (state.frames_per_hold, state.running)
    }

    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }
}

fn spawn_worker(inner: &Arc<Inner>, state: &mut State) {
    state.running += 1;
    let index = state.started;
    state.started += 1;
    let inner = Arc::clone(inner);
    std::thread::Builder::new()
        .name("milkdrop-thumbs".into())
        .spawn(move || run(inner, index))
        .expect("thread spawns");
}

fn note_job(inner: &Arc<Inner>, state: &mut State, frames: u64) {
    state.jobs += 1;
    let frames = frames as f64;
    state.frames_per_hold = if state.jobs == 1 {
        frames
    } else {
        // A short memory: the GPU the app's visual shares changes load.
        state.frames_per_hold * 0.6 + frames * 0.4
    };
    let room = state.queue.len() > state.running;
    let healthy = state.jobs >= WARMUP_JOBS && state.frames_per_hold >= HEALTHY_FRAMES;
    if room && healthy && state.running < max_workers() {
        log::debug!(
            "milkdrop thumbnails: {:.0} frames per hold with {} workers, adding one",
            state.frames_per_hold,
            state.running
        );
        spawn_worker(inner, state);
    }
}

/// FNV-1a, stable across runs where the standard hasher isn't promised to be.
fn path_hash(path: &Path) -> u64 {
    struct Fnv(u64);
    impl Hasher for Fnv {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for byte in bytes {
                self.0 ^= u64::from(*byte);
                self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    let mut hasher = Fnv(0xcbf2_9ce4_8422_2325);
    hasher.write(path.to_string_lossy().as_bytes());
    hasher.finish()
}

struct Tone {
    phase: f32,
    beat: f32,
}

impl Tone {
    fn new() -> Tone {
        Tone {
            phase: 0.0,
            beat: 0.0,
        }
    }

    fn samples(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            self.phase = (self.phase + 0.02) % std::f32::consts::TAU;
            self.beat = (self.beat + 2.0 / TONE_RATE as f32) % 1.0;
            // A sharp attack so the beat detector sees beats, not a hum.
            let envelope = 0.35 + 0.65 * (1.0 - self.beat).powi(4);
            let value = self.phase.sin() * 0.6 * envelope;
            out.push(value);
            out.push(value);
        }
        out
    }
}

fn run(inner: Arc<Inner>, index: usize) {
    if let Err(message) = std::fs::create_dir_all(&inner.dir) {
        log::warn!(
            "milkdrop thumbnails: can't create {}: {message}",
            inner.dir.display()
        );
        let mut state = inner.state.lock().unwrap();
        state.running -= 1;
        state.queue.clear();
        return;
    }
    let mut engine: Option<(Engine, Arc<rox_viz::AudioFeed>)> = None;
    let mut tone = Tone::new();
    // Stagger workers across a hold so their finishes interleave.
    let phase = match index % 4 {
        0 => 0.0,
        1 => 0.5,
        2 => 0.25,
        _ => 0.75,
    };
    std::thread::sleep(HOLD.mul_f64(phase));
    loop {
        let job = {
            let mut state = inner.state.lock().unwrap();
            loop {
                if !state.queue.is_empty() {
                    let next = state.queue.remove(0);
                    state.active.insert(next.clone());
                    break Some(next);
                }
                let (next, timed_out) = inner.wake.wait_timeout(state, IDLE).unwrap();
                state = next;
                if timed_out.timed_out() && state.queue.is_empty() {
                    state.running -= 1;
                    break None;
                }
            }
        };
        let Some(preset) = job else {
            return;
        };

        if engine.is_none() {
            engine = match start(&inner) {
                Ok(started) => Some(started),
                Err(message) => {
                    log::warn!("milkdrop thumbnails: {message}");
                    let mut state = inner.state.lock().unwrap();
                    let failed: Vec<PathBuf> = std::mem::take(&mut state.queue);
                    state
                        .failed
                        .extend(failed.into_iter().map(|path| (path, message.clone())));
                    state.active.remove(&preset);
                    state.failed.insert(preset, message);
                    state.running -= 1;
                    return;
                }
            };
        }
        let (engine, feed) = engine.as_ref().expect("just started");

        match render(engine, feed, &mut tone, &preset) {
            Ok((frame, frames)) => {
                let path = Thumbnailer {
                    inner: Arc::clone(&inner),
                }
                .thumb_path(&preset);
                match write_png(&path, &frame) {
                    Ok(()) => {
                        let mut state = inner.state.lock().unwrap();
                        state.active.remove(&preset);
                        state.ready.insert(preset);
                        note_job(&inner, &mut state, frames);
                        inner.generation.fetch_add(1, Ordering::Release);
                    }
                    Err(message) => {
                        log::warn!(
                            "milkdrop thumbnails: can't write {}: {message}",
                            path.display()
                        );
                        let mut state = inner.state.lock().unwrap();
                        state.active.remove(&preset);
                        state.failed.insert(preset, message);
                    }
                }
            }
            Err(message) => {
                log::debug!(
                    "milkdrop thumbnails: {} skipped: {message}",
                    preset.display()
                );
                let note = Thumbnailer {
                    inner: Arc::clone(&inner),
                }
                .failed_path(&preset);
                std::fs::write(&note, &message).ok();
                let mut state = inner.state.lock().unwrap();
                state.active.remove(&preset);
                state.failed.insert(preset, message);
                inner.generation.fetch_add(1, Ordering::Release);
            }
        }
    }
}

fn start(inner: &Inner) -> Result<(Engine, Arc<rox_viz::AudioFeed>), String> {
    let feed = Arc::new(rox_viz::AudioFeed::new());
    feed.set_sample_rate(TONE_RATE);
    // Empty: every load names its path outright.
    let library = PresetLibrary::scan(&[], inner.textures.clone());
    let engine = Engine::spawn(EngineOptions {
        feed: Arc::clone(&feed),
        library,
        preset: None,
        fps: FPS,
        width: WIDTH,
        height: HEIGHT,
    });
    engine.send(Command::SetLocked(true));
    let deadline = Instant::now() + CONTEXT_TIMEOUT;
    loop {
        match engine.status() {
            Status::Running { .. } => return Ok((engine, feed)),
            Status::Failed(message) => return Err(message),
            Status::Starting => {
                if Instant::now() > deadline {
                    return Err("the engine never came up".into());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn render(
    engine: &Engine,
    feed: &rox_viz::AudioFeed,
    tone: &mut Tone,
    preset: &Path,
) -> Result<(crate::Frame, u64), String> {
    // Leftover events from the last preset would read as this one's answer.
    engine.take_events();
    engine.send(Command::LoadPreset {
        path: preset.to_path_buf(),
        smooth: false,
    });
    let frames_per_step = (TONE_RATE as f64 * TONE_STEP.as_secs_f64()) as usize;
    let deadline = Instant::now() + LOAD_TIMEOUT;
    loop {
        feed.push(&tone.samples(frames_per_step));
        let mut up = false;
        for event in engine.take_events() {
            match event {
                Event::PresetChanged(path) if path == preset => up = true,
                Event::PresetFailed { path, message } if path == preset => return Err(message),
                _ => {}
            }
        }
        if up {
            break;
        }
        if Instant::now() > deadline {
            return Err("took too long to compile".into());
        }
        std::thread::sleep(TONE_STEP);
    }
    // Counted off the sequence numbers, so frames this loop didn't see
    // still count.
    let first = engine.frame_after(0).map(|frame| frame.seq).unwrap_or(0);
    let started = Instant::now();
    while started.elapsed() < HOLD {
        feed.push(&tone.samples(frames_per_step));
        std::thread::sleep(TONE_STEP);
    }
    let frame = engine
        .frame_after(0)
        .ok_or_else(|| "rendered no frame".to_string())?;
    let frames = frame.seq.saturating_sub(first);
    Ok((frame, frames))
}

/// Pixels go out opaque: the readback's alpha is whatever projectM left, and
/// a transparent image reads as blank.
pub fn encode_png(frame: &crate::Frame) -> Result<Vec<u8>, String> {
    let mut pixels = (*frame.rgba8).clone();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 0xff;
    }
    let image = image::RgbaImage::from_raw(frame.width, frame.height, pixels)
        .ok_or_else(|| "frame size doesn't match its buffer".to_string())?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(bytes.into_inner())
}

/// Written beside its final name and renamed in, so a reader never sees half a file.
fn write_png(path: &Path, frame: &crate::Frame) -> Result<(), String> {
    let bytes = encode_png(frame)?;
    let tmp = path.with_extension("png.part");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_encodes_as_an_opaque_png() {
        let frame = crate::Frame {
            width: 2,
            height: 1,
            rgba8: Arc::new(vec![1, 2, 3, 0, 4, 5, 6, 0]),
            seq: 1,
        };
        let bytes = encode_png(&frame).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(decoded.as_raw(), &[1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn the_thumb_path_is_stable_and_tells_packs_apart() {
        let thumbs = Thumbnailer::new(PathBuf::from("/cache"), None);
        let a = thumbs.thumb_path(Path::new("/packs/cream/Geiss - Bloom.milk"));
        let b = thumbs.thumb_path(Path::new("/packs/cream/Geiss - Bloom.milk"));
        let c = thumbs.thumb_path(Path::new("/packs/other/Geiss - Bloom.milk"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("/cache"));
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("png"));
    }

    #[test]
    fn a_preset_starts_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let thumbs = Thumbnailer::new(dir.path().to_path_buf(), None);
        let preset = Path::new("/packs/cream/Geiss - Bloom.milk");
        assert_eq!(thumbs.thumb(preset), Thumb::Pending);
        std::fs::write(thumbs.thumb_path(preset), b"png").expect("write");
        assert_eq!(
            thumbs.thumb(preset),
            Thumb::Ready(thumbs.thumb_path(preset))
        );
    }

    #[test]
    fn the_tone_has_something_in_it() {
        let mut tone = Tone::new();
        let samples = tone.samples(960);
        assert_eq!(samples.len(), 1920);
        assert!(samples.iter().all(|s| s.abs() <= 1.0));
        assert!(samples.iter().any(|s| s.abs() > 0.1));
    }
}
