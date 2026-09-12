//! Preset thumbnails: one tiny engine that renders a preset for under a
//! second against a synthetic tone, grabs a frame, and writes it to a PNG
//! cache. What the preset browser shows beside each name.
//!
//! ## Why on demand and not a scan
//!
//! A pack runs to ten thousand presets and the big one past a hundred
//! thousand. A preset takes a compile plus most of a second to grow into
//! something worth looking at, so a full pass is hours of GPU for pictures
//! nobody scrolls to. The browser asks for what's on screen instead, in
//! list order, and every ask replaces the last: scrolling away drops the
//! rows that left before they cost anything. What lands stays on disk, so
//! a second look at the same folder is free.
//!
//! ## Why a tone
//!
//! Several presets draw nothing at all on silence, and the ones that do
//! draw draw their idle state, which is rarely the point of them. A sine
//! under a slow pulse gives the beat detector something to chew on without
//! needing the player to be running.
//!
//! ## Why several engines, and how many
//!
//! A preset's cost is mostly its compile and its hold, and neither uses
//! the GPU hard at this size, so engines side by side fill a screen
//! several times faster than one. Each worker thread owns its own engine
//! and context; they share the queue and nothing else.
//!
//! How many is measured rather than guessed. Every job counts the frames
//! its engine managed inside the hold: an engine on a free GPU delivers
//! close to its rate, and one fighting other contexts for the GPU
//! delivers fewer. Workers are added one at a time while that count
//! stays healthy and stop being added once it drops, which is the point
//! past which another engine only slows the ones already running. New
//! workers start half a hold out of phase with the last, so finished
//! thumbnails trickle in rather than land in batches.
//!
//! Workers are dropped after a quiet spell, so a browser that's closed
//! costs no GL context.

use std::collections::{HashMap, HashSet};
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::{Command, Engine, EngineOptions, Event, PresetLibrary, Status};

/// The thumbnail's size: 16:9 at the smallest the worker will render,
/// which floors each side at 128 and would square anything shorter. Small
/// enough that the readback and the encode are nothing, big enough to tell
/// a fractal from a waveform at a glance.
pub const WIDTH: u32 = 256;
pub const HEIGHT: u32 = 144;

/// How long a preset runs before its frame is taken. Long enough for a
/// preset that builds up over time to have built something; short enough
/// that a screenful of rows fills while someone's still reading it.
const HOLD: Duration = Duration::from_millis(900);

/// How long a compile may take before the preset is given up on.
const LOAD_TIMEOUT: Duration = Duration::from_secs(6);

/// How long the engine sits idle with nothing asked before it's dropped.
const IDLE: Duration = Duration::from_secs(30);

/// How long to wait for a cold driver to hand out a context.
const CONTEXT_TIMEOUT: Duration = Duration::from_secs(20);

/// The engine's rate. It paces the worker's render loop, and a thumbnail
/// wants as many frames as it can get inside its hold.
const FPS: u32 = 120;

/// The most engines run side by side whatever the frame count says.
/// Each is a thread, a context and a projectM instance, and past this
/// the app's own visual starts paying for them.
const MAX_WORKERS: usize = 8;

/// How many workers the first ask starts. Two, so the very first screen
/// already gets the stagger; the rest come from the measurement.
const FIRST_WORKERS: usize = 2;

/// The frames-per-hold an engine has to keep delivering for another
/// worker to be added. At [`FPS`] over [`HOLD`] a free GPU manages about
/// a hundred; a third of that still reads as motion in the thumbnail and
/// says the GPU has room for one more.
const HEALTHY_FRAMES: f64 = 36.0;

/// How many jobs feed the frame average before it's trusted to grow on.
const WARMUP_JOBS: u64 = 2;

/// The ceiling on workers this machine allows: one per core, capped.
fn max_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, MAX_WORKERS)
}

/// What the tone is pushed at, and how much per push.
const TONE_RATE: u32 = 48_000;
const TONE_STEP: Duration = Duration::from_millis(20);

/// What the browser is told about one preset's thumbnail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Thumb {
    /// On disk at this path.
    Ready(PathBuf),
    /// Not rendered yet. Asking for it puts it in the queue.
    Pending,
    /// The preset wouldn't load, or the engine wouldn't start, and why.
    /// Not asked for again this run, so a broken preset doesn't spin the
    /// worker.
    Failed(String),
}

struct State {
    /// What to render next, in the order the browser wants it. Replaced
    /// whole on every ask.
    queue: Vec<PathBuf>,
    /// What a worker has in hand right now, so an ask that names it again
    /// doesn't queue a second render of the same preset.
    active: HashSet<PathBuf>,
    /// Thumbnails known to be on disk, so a row doesn't stat per render.
    ready: HashSet<PathBuf>,
    failed: HashMap<PathBuf, String>,
    /// How many worker threads are up. Each exits after an idle spell,
    /// and the next ask starts as many as its queue can use.
    running: usize,
    /// How many workers have ever started, for the stagger: each new one
    /// waits a different fraction of a hold before its first job.
    started: usize,
    /// Frames per hold, averaged over recent jobs, and how many jobs fed
    /// it. What decides whether another worker is worth adding.
    frames_per_hold: f64,
    jobs: u64,
}

struct Inner {
    dir: PathBuf,
    textures: Option<PathBuf>,
    state: Mutex<State>,
    wake: Condvar,
    /// Moves when a thumbnail lands, for a view to poll.
    gen: AtomicU64,
}

/// The thumbnail service. Cheap to clone; every clone is the same queue
/// and cache.
#[derive(Clone)]
pub struct Thumbnailer {
    inner: Arc<Inner>,
}

impl Thumbnailer {
    /// A thumbnailer caching into `dir`, rendering with the pack textures
    /// at `textures` if there are any.
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
                gen: AtomicU64::new(0),
            }),
        }
    }

    /// Where a preset's thumbnail lives, whether or not it's there yet.
    /// Keyed by a hash of the path: preset names carry characters no
    /// filesystem wants, and two packs can hold the same name.
    pub fn thumb_path(&self, preset: &Path) -> PathBuf {
        self.inner
            .dir
            .join(format!("{:016x}.png", path_hash(preset)))
    }

    /// Where a preset's failure note lives: the compiler's message,
    /// beside where the thumbnail would be. On disk so a preset that
    /// won't compile isn't tried again on every run.
    fn failed_path(&self, preset: &Path) -> PathBuf {
        self.thumb_path(preset).with_extension("failed")
    }

    /// What's known about a preset's thumbnail. A hit on disk, picture
    /// or failure note, is remembered so the next ask is a set lookup.
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

    /// A preset loaded on a real engine, so whatever was held against it
    /// is stale: drop the note and let the next ask render it.
    pub fn loaded(&self, preset: &Path) {
        let mut state = self.inner.state.lock().unwrap();
        state.failed.remove(preset);
        // The note may be on disk from an earlier run without ever having
        // been read this one, so the file goes whether or not the map
        // knew it.
        std::fs::remove_file(self.failed_path(preset)).ok();
    }

    /// Say which presets are on screen, in order. Replaces the queue:
    /// what's no longer wanted is dropped, what's done, failed or in hand
    /// is skipped, and as many workers start as the queue can use. An
    /// empty ask is how a browser that stopped showing previews lets the
    /// workers wind down.
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

    /// The measured rate, for anyone curious: frames the engines manage
    /// inside a hold, averaged over recent jobs, and how many workers are
    /// up. Zero before the first job.
    pub fn stats(&self) -> (f64, usize) {
        let state = self.inner.state.lock().unwrap();
        (state.frames_per_hold, state.running)
    }

    /// Moves every time a thumbnail lands. A view polls it and repaints
    /// when it moved.
    pub fn gen(&self) -> u64 {
        self.inner.gen.load(Ordering::Acquire)
    }
}

/// Start one more worker. `state` is the held lock.
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

/// A job finished with `frames` rendered inside its hold: fold it into
/// the average, and add a worker if the queue has more than the workers
/// can take and the engines still have GPU to spare.
fn note_job(inner: &Arc<Inner>, state: &mut State, frames: u64) {
    state.jobs += 1;
    let frames = frames as f64;
    state.frames_per_hold = if state.jobs == 1 {
        frames
    } else {
        // A short memory: the GPU the app's own visual shares can get
        // busy or free between one screen and the next.
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

/// FNV-1a over the path's bytes. Stable across runs, which the standard
/// hasher isn't promised to be, and the cache is only worth having if it
/// survives a restart.
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

/// A sine sweep under a two-beat-a-second pulse, interleaved stereo.
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
            // A sharp attack and a decay, so the beat detector sees a
            // beat rather than a hum.
            let envelope = 0.35 + 0.65 * (1.0 - self.beat).powi(4);
            let value = self.phase.sin() * 0.6 * envelope;
            out.push(value);
            out.push(value);
        }
        out
    }
}

/// One worker: take the next preset, render it, write it, until the
/// queue has been empty for [`IDLE`]. `index` is its place in the
/// stagger: the first job waits a fraction of a hold so this worker's
/// finishes land between the others'.
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
    // Spread the workers over a hold: the second starts half a hold
    // after the first, the third a quarter, and so on, so their finishes
    // interleave instead of arriving together.
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
            // Dropping the engine joins its thread and frees the context.
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
                        inner.gen.fetch_add(1, Ordering::Release);
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
                inner.gen.fetch_add(1, Ordering::Release);
            }
        }
    }
}

/// Spawn the engine and wait for its context.
fn start(inner: &Inner) -> Result<(Engine, Arc<rox_viz::AudioFeed>), String> {
    let feed = Arc::new(rox_viz::AudioFeed::new());
    feed.set_sample_rate(TONE_RATE);
    // An empty library: every load names its path outright, which the
    // worker takes for a preset outside its roots.
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

/// Load one preset, let it run under the tone, and take its last frame,
/// with how many frames the engine managed inside the hold.
fn render(
    engine: &Engine,
    feed: &rox_viz::AudioFeed,
    tone: &mut Tone,
    preset: &Path,
) -> Result<(crate::Frame, u64), String> {
    // Anything the last preset left in the mailbox would read as this
    // one's answer.
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
    // The frame count is read off the sequence: the worker numbers every
    // frame it publishes, so the span across the hold is the frames it
    // managed, whether or not this loop saw each one.
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

/// Encode a frame as PNG bytes. The readback's alpha is whatever projectM
/// left in the framebuffer, and a transparent image is a blank one, so the
/// pixels go out opaque. Shared by the thumbnail cache and the control
/// socket's frame dump.
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

/// Write a frame as PNG beside its final name and rename it in, so a
/// browser reading the folder never sees half a file.
fn write_png(path: &Path, frame: &crate::Frame) -> Result<(), String> {
    let bytes = encode_png(frame)?;
    let tmp = path.with_extension("png.part");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dump is a real PNG, and the alpha projectM left behind is gone.
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

    /// The cache key is a function of the path alone, the same on every
    /// run, and different for two presets that share a name.
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

    /// Nothing on disk and nothing asked reads as pending; a failure is
    /// remembered.
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

    /// The tone is stereo, bounded, and not silence.
    #[test]
    fn the_tone_has_something_in_it() {
        let mut tone = Tone::new();
        let samples = tone.samples(960);
        assert_eq!(samples.len(), 1920);
        assert!(samples.iter().all(|s| s.abs() <= 1.0));
        assert!(samples.iter().any(|s| s.abs() > 0.1));
    }
}
