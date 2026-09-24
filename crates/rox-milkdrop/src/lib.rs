//! MilkDrop visuals, rendered by libprojectM on a thread of their own.
//!
//! A preset runs its own GLSL, so rox embeds libprojectM (the reference
//! implementation) rather than reimplementing it. gpui draws through blade and
//! D3D11, none of which let a second renderer into their swapchain, so the
//! engine owns a private windowless GL context ([`context`]), renders into an
//! FBO, and publishes rgba8 pixels for the panel to upload. Nothing here knows
//! gpui exists.
//!
//! ## The readback
//!
//! ADR 8 refused a GPU readback for the generative visual; ADR 28 takes it on
//! here, since the alternative is porting MilkDrop's shader language to WGSL.
//! Two pixel buffers alternate, frame N reading back while N-1 is mapped: one
//! frame of latency for no pipeline stall, a smooth sixty over a sawtooth
//! thirty.
//!
//! Frames are handed out as an `Arc`, and the worker reclaims the buffer once
//! the last handle drops. Copying instead measured 2.6 ms of UI thread per
//! frame at 776x1049, nearly all page faults. Zero-copy import is the planned
//! follow-up; `examples/headless.rs` prints its baseline.
//!
//! ## Shape
//!
//! [`Engine::spawn`] returns before the context exists (a cold driver takes a
//! second). [`Command`]s go down a channel; frames, [`Event`]s and [`Status`]
//! come back through shared state. Failure is [`Status::Failed`], never a
//! panic: a machine with no GL should still get every other panel.

pub mod context;
mod gl;
pub mod library;
pub mod thumbs;
mod worker;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

pub use library::{PresetLibrary, Rotation};

/// Straight-alpha rgba8, top row first. Shared rather than owned, so handing
/// one out is a refcount bump.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba8: Arc<Vec<u8>>,
    /// Monotonic per engine; see [`Engine::frame_after`].
    pub seq: u64,
}

#[derive(Clone, Debug)]
pub enum Status {
    Starting,
    Running {
        preset: Option<PathBuf>,
        projectm_version: String,
        /// Shown when the engine runs but no frame arrives: the fact that
        /// decides most bug reports.
        renderer: String,
        gl_version: String,
    },
    /// Names the platform and the step; shown to the user.
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum Event {
    PresetChanged(PathBuf),
    PresetFailed { path: PathBuf, message: String },
}

/// Fire and forget: commands to a failed worker are dropped.
pub enum Command {
    /// Device pixels. Reallocates every target, so the panel debounces it.
    Resize {
        width: u32,
        height: u32,
    },
    LoadPreset {
        path: PathBuf,
        smooth: bool,
    },
    /// Random pick from the rotation.
    NextPreset {
        smooth: bool,
    },
    PreviousPreset {
        smooth: bool,
    },
    /// A rotation that selects nothing falls back to the whole library, so a
    /// deleted folder doesn't strand the panel on one preset.
    SetRotation(Rotation),
    /// After a rescan. A worker that had nothing to show loads a preset at
    /// once rather than sitting on projectM's idle one.
    SetLibrary {
        library: PresetLibrary,
        rotation: Rotation,
    },
    SetLocked(bool),
    SetPresetDuration(f64),
    SetBeatSensitivity(f32),
    SetHardCut(bool),
    SetFps(u32),
    /// Stop rendering, keep the context.
    Pause,
    Resume,
}

pub struct EngineOptions {
    pub feed: Arc<rox_viz::AudioFeed>,
    pub library: PresetLibrary,
    /// Loaded first, with no shuffle before it. Sent as a command instead, it
    /// would land behind the startup shuffle, and the backdrop (which saves
    /// what it sees while locked) would keep the random one.
    pub preset: Option<PathBuf>,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
}

/// `seq` is an atomic outside the frame lock so the per-frame "anything new"
/// check never contends with a publish.
pub(crate) struct Shared {
    pub(crate) seq: AtomicU64,
    /// Only the headless example reads this.
    pub(crate) readback_micros: AtomicU64,
    pub(crate) frame: Mutex<Option<Frame>>,
    pub(crate) status: Mutex<Status>,
    pub(crate) events: Mutex<Vec<Event>>,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            seq: AtomicU64::new(0),
            readback_micros: AtomicU64::new(0),
            frame: Mutex::new(None),
            status: Mutex::new(Status::Starting),
            events: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn set_status(&self, status: Status) {
        *self.status.lock().unwrap() = status;
    }

    pub(crate) fn push_event(&self, event: Event) {
        let mut events = self.events.lock().unwrap();
        // A parked or hidden panel stops draining; don't grow without bound.
        if events.len() < 64 {
            events.push(event);
        }
    }
}

/// How long a dropped engine waits for its worker. A wait at all avoids two
/// GL contexts alive at once; a bound, because the quit path runs this on the
/// UI thread and a hung driver call must not hang the quit.
const STAND_DOWN: Duration = Duration::from_millis(400);

/// The last clone dropped shuts the worker down.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

struct EngineInner {
    /// Behind a lock so [`Engine::stop`] can hang up through a shared handle.
    commands: Mutex<Option<Sender<Command>>>,
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// Never sent on: it disconnects when the worker thread returns, a join
    /// with a timeout.
    finished: Receiver<()>,
}

impl Engine {
    pub fn spawn(options: EngineOptions) -> Engine {
        let shared = Arc::new(Shared::new());
        let (commands, receiver) = crossbeam_channel::unbounded();

        let worker_shared = Arc::clone(&shared);
        // Never sent on; its drop at closure end, after GL teardown, is what
        // `finished` waits for.
        let (ran, finished) = crossbeam_channel::bounded::<()>(0);
        let worker = std::thread::Builder::new()
            .name("rox-milkdrop".to_string())
            .spawn(move || {
                let _ran = ran;
                worker::run(options, receiver, worker_shared);
            })
            .ok();

        if worker.is_none() {
            shared.set_status(Status::Failed(
                "rox could not start the Milkdrop render thread".to_string(),
            ));
        }

        Engine {
            inner: Arc::new(EngineInner {
                commands: Mutex::new(Some(commands)),
                shared,
                worker: Mutex::new(worker),
                finished,
            }),
        }
    }

    pub fn send(&self, command: Command) {
        if let Some(commands) = self.inner.commands.lock().unwrap().as_ref() {
            // A closed channel means the worker gave up; `status()` says why.
            let _ = commands.send(command);
        }
    }

    /// Hang up without waiting. Separate from drop so the quit path can hang
    /// up several engines before waiting on any, and so it works while a
    /// retained render tree still holds a clone.
    pub fn stop(&self) {
        self.inner.commands.lock().unwrap().take();
    }

    /// Only meaningful after [`Engine::stop`] or the last clone's drop.
    pub fn wait(&self) -> bool {
        self.inner.wait()
    }

    /// The newest frame if its seq is past `after`. Hold it no longer than
    /// the paint that draws it, or the next render allocates.
    pub fn frame_after(&self, after: u64) -> Option<Frame> {
        if self.inner.shared.seq.load(Ordering::Acquire) <= after {
            return None;
        }
        let frame = self.inner.shared.frame.lock().unwrap();
        let frame = frame.as_ref()?;
        if frame.seq <= after {
            return None;
        }
        Some(frame.clone())
    }

    /// The map-and-flip cost the zero-copy follow-up exists to remove.
    pub fn last_readback_micros(&self) -> u64 {
        self.inner.shared.readback_micros.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> Status {
        self.inner.shared.status.lock().unwrap().clone()
    }

    pub fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.inner.shared.events.lock().unwrap())
    }
}

impl EngineInner {
    /// Waits on `finished`, not the join: a join has no timeout.
    fn wait(&self) -> bool {
        !matches!(
            self.finished.recv_timeout(STAND_DOWN),
            Err(RecvTimeoutError::Timeout)
        )
    }
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        // Hanging up is the shutdown signal.
        self.commands.lock().unwrap().take();

        let Some(worker) = self.worker.lock().unwrap().take() else {
            return;
        };

        if self.wait() {
            let _ = worker.join();
            return;
        }

        // Stuck in a driver call, most likely. Detaching beats a quit that
        // never finishes; the exit guard parks the thread before GL is freed.
        log::warn!(
            "milkdrop engine did not stand down in {} ms; leaving its thread to the exit guard",
            STAND_DOWN.as_millis()
        );
    }
}
