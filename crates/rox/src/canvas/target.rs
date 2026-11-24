//! The render target item (ADR 37): a canvas item whose picture a renderer,
//! a program outside rox, draws. Each target runs one renderer session at a
//! time through [`renderers::start`]'s gate, hands it a frame mapping rox
//! made, and paints the frames it announces through the texture body. This
//! module starts nothing the gate refuses, and approves and enables nothing.
//!
//! Under ADR 35 everything a renderer sends is a stranger's: an announce is
//! a slot number checked by `rox-frames`, a frame is bytes copied out of a
//! read-only mapping and checked for size, and the pace of announces is
//! capped. A renderer that stalls, floods, lies or dies costs rox that
//! target's picture and nothing else. The UI thread never waits on one: it
//! takes an `Arc` out of a mutex only rox's own threads hold, and only ever
//! briefly.
//!
//! Each target has a supervisor thread, which makes the mapping, starts and
//! restarts the session, copies frames and writes to the renderer's stdin,
//! and per session a reader thread on its stdout. A frame is copied only
//! once the canvas has taken the last one, and published with a wake to
//! the canvas, which never asks for frames on its own. A target the canvas
//! doesn't draw pauses its renderer, so a hidden one costs nothing. The
//! supervisor also keeps the target's status and its last refusal for the
//! inspector, and wakes the canvas only when one of them moves.
//!
//! Outward a renderer gets what its enable card names and nothing more: the
//! signal pool's values with the playing position, read on frames the
//! canvas already draws, and the pointer over its own surface. The canvas
//! hands those lines to the supervisor through a small bounded outbox, and
//! a renderer that stops reading loses lines instead of holding rox up.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::{BufReader, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, BackgroundExecutor, Context, Entity, SharedString, Task, Window, div,
    prelude::*, px,
};
use rox_canvas::{CanvasItem, CanvasState, Content, Projected};
use rox_design::palette;
use rox_frames::{ANNOUNCE_MAX, Mapping, Refusal, Spec};
use rox_plugins::process::{self, Line};
use rox_services::renderers::{self, Refused, Session};
use rox_viz::signal::{Signal, SignalHub, Source as SignalSource};

use crate::canvas::bbox;
use crate::canvas::texture::{Frame, TextureView};

/// What a target's `owner` starts with: renderers are packages, named the
/// way a plugin's panel names its plugin.
const OWNER_PREFIX: &str = "plugin:";

/// Frame buffers per target: the one published, the one the canvas and
/// gpui's upload hold, and the one being copied into.
const POOL: usize = 3;

/// How long the supervisor waits for a reader to see its stdout close
/// after the renderer is killed. Only a grandchild that left the process
/// group keeps it open past that.
const READER_GRACE: Duration = Duration::from_secs(1);

/// How soon a line the renderer's full stdin refused is tried again.
const RESEND: Duration = Duration::from_millis(50);

/// How soon a copy waiting on a free buffer looks again.
const POOL_RETRY: Duration = Duration::from_millis(4);

/// How often a paused session, whose stdout isn't read, is checked for an
/// exit.
const PAUSED_CHECK: Duration = Duration::from_secs(1);

/// Pointer lines held for a renderer that isn't keeping up. Past this the
/// oldest move goes first, then a scroll, then a leave, and a press only
/// when nothing else is left to drop.
const POINTER_QUEUE: usize = 64;

/// The least time between two moves sent, about a frame at 120 Hz. Moves
/// that come sooner are folded into the one waiting.
const MOVE_GAP: Duration = Duration::from_millis(8);

/// A pool name past this many characters is cut on the wire. Names come
/// from the user and from imported workspaces and objects.
const NAME_MAX: usize = 64;

/// The rules a session is held to.
#[derive(Clone, Copy, Debug)]
struct Limits {
    /// A published frame the canvas hasn't taken for this long pauses the
    /// renderer: nothing is drawing the target.
    idle: Duration,
    /// More than this many stdout lines in each of `flood_spans` spans in a
    /// row stops the renderer. S5 measured a flood taking a core in rox's
    /// reader.
    flood_lines: u32,
    flood_span: Duration,
    flood_spans: u32,
    /// This many refused slots in a row stops the renderer: one that lies
    /// about its frames every time is broken.
    refused_run: u32,
}

const LIMITS: Limits = Limits {
    idle: Duration::from_millis(500),
    flood_lines: 500,
    flood_span: Duration::from_secs(1),
    flood_spans: 2,
    refused_run: 60,
};

/// One renderer session, as the supervisor holds it.
trait Run: Send {
    /// Writes what it can of `bytes` without waiting on the renderer.
    fn send(&mut self, bytes: &[u8]) -> Sent;

    /// Whether the renderer is gone. Asking reaps it.
    fn exited(&self) -> bool;

    /// rox ended it from outside: a switch-off or a quit. Such an exit is
    /// never counted against the renderer.
    fn stopped(&self) -> bool;

    /// Closes stdin, gives it a moment, then kills it and its group.
    fn stop(&mut self);
}

enum Sent {
    /// This many bytes went, from the front.
    Wrote(usize),
    /// The pipe had no room; try again later.
    Full,
    Closed,
}

/// A session and its stdout.
type Started = (Box<dyn Run>, Box<dyn Read + Send>);

/// Where sessions come from: the renderer host, or a test's pipes.
trait Host: Send + 'static {
    fn start(&mut self, mapping: &Mapping) -> Result<Started, Refused>;

    /// When it may start again after an exit, None once it's stopped.
    fn due(&self) -> Option<Duration>;

    /// Counts an exit rox didn't ask for.
    fn note_exit(&self);

    /// The renderer's display name.
    fn name(&self) -> Option<String>;

    /// Why it wouldn't start now, for a target rox stopped from outside.
    fn refusal(&self) -> Option<Refused> {
        None
    }

    /// Forgets its exits: the user pressed Start Again.
    fn forgive(&self) {}
}

/// A renderer package started through the gate.
struct Renderer {
    id: String,
    width: u32,
    height: u32,
}

impl Host for Renderer {
    fn start(&mut self, mapping: &Mapping) -> Result<Started, Refused> {
        let mut session = renderers::start(&self.id, lent(mapping), self.width, self.height)?;

        let Some(stdout) = session.stdout.take() else {
            return Err(Refused::Start("no stdout".into()));
        };

        // A renderer that stops reading its stdin must not stall the
        // supervisor, which also tears the session down.
        if let Some(stdin) = &session.stdin
            && let Err(e) = process::never_block(stdin)
        {
            log::warn!("render target {}: stdin may block: {e}", self.id);
        }

        Ok((Box::new(Live::new(session, &self.id)), Box::new(stdout)))
    }

    fn due(&self) -> Option<Duration> {
        renderers::due(&self.id)
    }

    fn note_exit(&self) {
        renderers::note_exit(&self.id);
    }

    fn name(&self) -> Option<String> {
        renderers::all()
            .into_iter()
            .find(|renderer| renderer.id == self.id)
            .map(|renderer| renderer.name)
    }

    fn refusal(&self) -> Option<Refused> {
        renderers::refusal(&self.id)
    }

    fn forgive(&self) {
        renderers::forgive(&self.id);
    }
}

#[cfg(unix)]
fn lent(mapping: &Mapping) -> process::Lent<'_> {
    mapping.child_handle()
}

/// Never compiled: no Windows target builds here.
#[cfg(windows)]
fn lent(mapping: &Mapping) -> process::Lent<'_> {
    process::Lent {
        handle: mapping.child_handle(),
        access: rox_frames::CHILD_ACCESS,
    }
}

/// A session with its stdin still inside, so a stop gets the grace.
#[cfg(unix)]
struct Live(Session);

#[cfg(unix)]
impl Live {
    fn new(session: Session, _id: &str) -> Live {
        Live(session)
    }
}

#[cfg(unix)]
impl Run for Live {
    fn send(&mut self, bytes: &[u8]) -> Sent {
        use std::io::{ErrorKind, Write};

        let Some(stdin) = self.0.stdin.as_mut() else {
            return Sent::Closed;
        };

        // Non-blocking: up to `PIPE_BUF` goes whole or not at all, and a
        // longer line can go in part.
        match stdin.write(bytes) {
            Ok(0) => Sent::Full,
            Ok(n) => Sent::Wrote(n),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                Sent::Full
            }
            Err(_) => Sent::Closed,
        }
    }

    fn exited(&self) -> bool {
        self.0.exited()
    }

    fn stopped(&self) -> bool {
        self.0.stopped()
    }

    fn stop(&mut self) {
        self.0.stop();
    }
}

/// Lines a Windows session's writer thread holds before `send` says full.
#[cfg(windows)]
const WRITER_LINES: usize = 16;

/// How long a stop waits for the writer thread to hand stdin back.
#[cfg(windows)]
const WRITER_GRACE: Duration = Duration::from_millis(100);

/// Never compiled: no Windows target builds here. Windows pipes stay
/// blocking (`process::never_block`), so the writes go on a thread of their
/// own and `send` only ever hands a line to its bounded channel.
#[cfg(windows)]
struct Live {
    session: Session,
    lines: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    /// Hands stdin back once the channel closes, so the stop still gets its
    /// grace. One stuck on a full pipe is killed with the renderer.
    writer: Option<JoinHandle<Option<std::process::ChildStdin>>>,
}

#[cfg(windows)]
impl Live {
    fn new(mut session: Session, id: &str) -> Live {
        let Some(mut stdin) = session.stdin.take() else {
            return Live {
                session,
                lines: None,
                writer: None,
            };
        };

        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(WRITER_LINES);
        let writer = std::thread::Builder::new()
            .name(format!("target-{id}-write"))
            .spawn(move || {
                use std::io::Write;

                while let Ok(line) = rx.recv() {
                    if stdin.write_all(&line).and_then(|_| stdin.flush()).is_err() {
                        return None;
                    }
                }
                Some(stdin)
            });

        match writer {
            Ok(writer) => Live {
                session,
                lines: Some(tx),
                writer: Some(writer),
            },
            Err(e) => {
                log::warn!("render target {id}: no writer thread, it gets no lines: {e}");
                Live {
                    session,
                    lines: None,
                    writer: None,
                }
            }
        }
    }
}

#[cfg(windows)]
impl Run for Live {
    fn send(&mut self, bytes: &[u8]) -> Sent {
        use std::sync::mpsc::TrySendError;

        let Some(lines) = &self.lines else {
            return Sent::Closed;
        };

        match lines.try_send(bytes.to_vec()) {
            Ok(()) => Sent::Wrote(bytes.len()),
            Err(TrySendError::Full(_)) => Sent::Full,
            Err(TrySendError::Disconnected(_)) => Sent::Closed,
        }
    }

    fn exited(&self) -> bool {
        self.session.exited()
    }

    fn stopped(&self) -> bool {
        self.session.stopped()
    }

    fn stop(&mut self) {
        self.lines.take();

        if let Some(writer) = self.writer.take() {
            let until = Instant::now() + WRITER_GRACE;
            while !writer.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }

            if writer.is_finished()
                && let Ok(Some(stdin)) = writer.join()
            {
                self.session.stdin = Some(stdin);
            }
        }

        self.session.stop();
    }
}

/// What the supervisor, the reader and the canvas share. The lock is held
/// only to read or flip these fields: never across a copy out of the
/// mapping, a write to the renderer or a read of its stdout.
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// Slot copies made, for the tests and the logs.
    copies: AtomicU64,
    /// Copies thrown away because the renderer wrote the slot under them.
    torn: AtomicU64,
}

struct State {
    /// The newest frame, for the canvas to take.
    frame: Option<Frame>,
    /// The canvas has the newest frame, so the next copy is worth making.
    taken: bool,
    /// When the newest frame was published.
    published_at: Instant,
    /// The newest slot announced and not yet copied.
    pending: Option<u32>,
    /// The canvas didn't draw the item.
    held: bool,
    /// A frame went untaken past the idle limit.
    idle: bool,
    /// The session being read, so an old session's reader stops.
    session: u64,
    /// How the current session's reader ended.
    ended: Option<End>,
    /// The last session whose reader has returned.
    read_done: u64,
    /// Try starting again: the renderers' switches moved, or Start Again.
    retry: bool,
    /// Start Again was pressed, so the exit count goes before the retry.
    forgive: bool,
    phase: Phase,
    /// The last refusal, kept for the status line until a session starts.
    refused: Option<Refused>,
    /// The target is gone.
    stop: bool,
    name: Option<String>,
    /// Lines from the canvas waiting for the renderer.
    out: Outbox,
}

impl State {
    fn paused(&self) -> bool {
        self.held || self.idle
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Not started yet, or waiting out the restart schedule.
    Starting,
    Running,
    /// Waiting for the switches to move or Start Again.
    Resting,
}

/// What a target's status line says, read from its state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Not started yet, or waiting to start again, with the reason the last
    /// start failed if it did.
    Starting(Option<String>),
    Running,
    /// Running, and paused because the canvas doesn't draw it.
    Paused,
    /// Not running until something changes. None when rox stopped it and
    /// the gate would let it run now.
    Stopped(Option<Refused>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// stdout closed: the renderer exited, or was killed.
    Closed,
    Flood,
}

/// Why a session ended.
#[derive(Debug, PartialEq, Eq)]
enum Exit {
    Stop,
    Closed,
    Flood,
    Lies,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wait<'a>(
        &self,
        state: MutexGuard<'a, State>,
        within: Option<Duration>,
    ) -> MutexGuard<'a, State> {
        match within {
            Some(within) => {
                self.wake
                    .wait_timeout(state, within)
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            }
            None => self.wake.wait(state).unwrap_or_else(|e| e.into_inner()),
        }
    }
}

/// Every target alive, so a change to the renderers' switches reaches the
/// ones resting on a refusal.
static LIVE: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());

/// The supervisor and its shared state, without gpui, so the tests drive it
/// with a fake host.
struct Source {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Source {
    fn spawn(
        tag: &str,
        host: Box<dyn Host>,
        spec: Spec,
        limits: Limits,
        notify: Box<dyn Fn() + Send>,
    ) -> Source {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                frame: None,
                taken: true,
                published_at: Instant::now(),
                pending: None,
                held: false,
                idle: false,
                session: 0,
                ended: None,
                read_done: 0,
                retry: false,
                forgive: false,
                phase: Phase::Starting,
                refused: None,
                stop: false,
                name: None,
                out: Outbox::default(),
            }),
            wake: Condvar::new(),
            copies: AtomicU64::new(0),
            torn: AtomicU64::new(0),
        });

        if let Ok(mut live) = LIVE.lock() {
            live.retain(|shared| shared.strong_count() > 0);
            live.push(Arc::downgrade(&shared));
        }

        let supervisor = Supervisor {
            shared: shared.clone(),
            host,
            spec,
            limits,
            notify,
            tag: tag.to_string(),
            pool: Pool::default(),
            published: 0,
        };
        let thread = std::thread::Builder::new()
            .name(format!("target-{tag}"))
            .spawn(move || supervisor.run());
        if let Err(e) = &thread {
            log::warn!("render target {tag}: no supervisor: {e}");
        }

        Source {
            shared,
            thread: thread.ok(),
        }
    }

    /// The newest frame. Taking it is what says the target is drawn: it
    /// lets the next copy go and resumes a paused renderer.
    fn take(&self) -> Option<Frame> {
        let mut state = self.shared.lock();
        let wanted = (!state.taken && state.pending.is_some()) || state.paused();

        state.taken = true;
        state.held = false;
        state.idle = false;
        let frame = state.frame.clone();
        drop(state);

        if wanted {
            self.shared.wake.notify_all();
        }
        frame
    }

    /// The canvas didn't draw the item this frame.
    fn hold(&self) {
        let mut state = self.shared.lock();
        if state.held {
            return;
        }

        state.held = true;
        state.out.pause();
        drop(state);
        self.shared.wake.notify_all();
    }

    /// This frame's reading, for a target drawn this frame.
    fn feed(&self, feed: &Feed) {
        self.send(|out| {
            out.name_pool(&feed.pool);
            if let Some(line) = &feed.signals {
                out.signals = Some(line.clone());
            }
        });
    }

    fn point(&self, pointer: Pointer) {
        self.send(|out| out.point(pointer));
    }

    /// Queues lines for a session that runs and isn't paused, and for no
    /// other: a paused or stopped target is sent nothing.
    fn send(&self, queue: impl FnOnce(&mut Outbox)) {
        let mut state = self.shared.lock();
        if state.phase != Phase::Running || state.paused() {
            return;
        }

        queue(&mut state.out);
        drop(state);
        self.shared.wake.notify_all();
    }

    fn name(&self) -> Option<String> {
        self.shared.lock().name.clone()
    }

    fn status(&self) -> Status {
        let state = self.shared.lock();

        match state.phase {
            Phase::Starting => Status::Starting(match &state.refused {
                Some(Refused::Start(why)) => Some(why.clone()),
                _ => None,
            }),
            Phase::Running if state.paused() => Status::Paused,
            Phase::Running => Status::Running,
            Phase::Resting => Status::Stopped(state.refused.clone()),
        }
    }

    /// Clears the exit count and tries again. The gate still decides, so
    /// this never runs what it refuses.
    fn start_again(&self) {
        {
            let mut state = self.shared.lock();
            state.retry = true;
            state.forgive = true;
        }
        self.shared.wake.notify_all();
    }

    /// Asks the supervisor to wind down. Joining is the caller's.
    fn stop(&mut self) -> Option<JoinHandle<()>> {
        self.shared.lock().stop = true;
        self.shared.wake.notify_all();
        self.thread.take()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // Detached when no one joined it. It ends on its own once it sees
        // the flag.
        let _ = self.stop();
    }
}

/// The buffers frames are copied into. One is free when only the pool
/// holds it.
#[derive(Default)]
struct Pool {
    buffers: Vec<Arc<Vec<u8>>>,
}

impl Pool {
    fn free(&mut self) -> Option<usize> {
        if let Some(ix) = self
            .buffers
            .iter_mut()
            .position(|buffer| Arc::get_mut(buffer).is_some())
        {
            return Some(ix);
        }

        if self.buffers.len() < POOL {
            self.buffers.push(Arc::new(Vec::new()));
            return Some(self.buffers.len() - 1);
        }
        None
    }

    fn has_free(&self) -> bool {
        self.buffers.len() < POOL
            || self
                .buffers
                .iter()
                .any(|buffer| Arc::strong_count(buffer) == 1)
    }
}

struct Supervisor {
    shared: Arc<Shared>,
    host: Box<dyn Host>,
    spec: Spec,
    limits: Limits,
    notify: Box<dyn Fn() + Send>,
    tag: String,
    pool: Pool,
    /// Frames published across every session, so a restarted renderer's
    /// first frame is new to the texture.
    published: u64,
}

enum Job {
    Pause(bool),
    /// A line from the outbox.
    Send(String),
    /// Finish what the pipe refused last time.
    Flush,
    Copy(u32),
}

impl Supervisor {
    fn run(mut self) {
        let mut session = 0u64;

        loop {
            let name = self.host.name();
            let forgive = {
                let mut state = self.shared.lock();
                if state.stop {
                    return;
                }
                state.retry = false;
                state.name = name;
                std::mem::take(&mut state.forgive)
            };
            if forgive {
                self.host.forgive();
            }

            let mapping = match Mapping::create(self.spec) {
                Ok(mapping) => mapping,
                Err(e) => {
                    log::warn!("render target {}: no frame mapping: {e}", self.tag);
                    self.set(
                        Phase::Resting,
                        Some(Refused::Start(format!("no frame mapping: {e}"))),
                    );
                    if !self.rest() {
                        return;
                    }
                    continue;
                }
            };

            let started = self.host.start(&mapping);
            let (mut run, stdout) = match started {
                Ok(started) => started,

                // A failed start already counts as an exit.
                Err(Refused::Start(why)) => {
                    log::info!("render target {}: didn't start: {why}", self.tag);
                    drop(mapping);
                    self.set(Phase::Starting, Some(Refused::Start(why)));
                    if !self.after_exit() {
                        return;
                    }
                    continue;
                }

                Err(refused) => {
                    log::info!("render target {}: not started: {refused}", self.tag);
                    drop(mapping);
                    self.set(Phase::Resting, Some(refused));
                    self.clear();
                    if !self.rest() {
                        return;
                    }
                    continue;
                }
            };

            session += 1;
            {
                let mut state = self.shared.lock();
                state.session = session;
                state.ended = None;
                state.pending = None;
                state.out.restart();
            }
            self.set(Phase::Running, None);

            let reader = {
                let shared = self.shared.clone();
                let limits = self.limits;
                std::thread::Builder::new()
                    .name(format!("target-{}-read", self.tag))
                    .spawn(move || read(shared, session, stdout, limits))
            };
            let exit = match &reader {
                Ok(_) => self.drive(run.as_mut(), &mapping),
                Err(e) => {
                    log::warn!("render target {}: nothing reads it: {e}", self.tag);
                    Exit::Stop
                }
            };

            // Asked before the stop, which sets it too.
            let ours = run.stopped();
            run.stop();
            drop(run);
            self.join_reader(session, reader.ok());
            drop(mapping);

            match exit {
                Exit::Stop => return,

                // A switch-off or a quit. Only a poke after it starts it
                // again, so a quit never restarts one.
                Exit::Closed if ours => {
                    log::info!("render target {}: stopped by rox", self.tag);
                    self.shared.lock().retry = false;
                    self.set(Phase::Resting, self.host.refusal());
                    self.clear();
                    if !self.rest() {
                        return;
                    }
                }

                Exit::Closed | Exit::Flood | Exit::Lies => {
                    match exit {
                        Exit::Flood => log::warn!(
                            "render target {}: stopped, more than {} lines a second",
                            self.tag,
                            self.limits.flood_lines
                        ),
                        Exit::Lies => log::warn!(
                            "render target {}: stopped, {} refused frames in a row",
                            self.tag,
                            self.limits.refused_run
                        ),
                        _ => log::info!("render target {}: the renderer exited", self.tag),
                    }

                    self.host.note_exit();
                    if !self.after_exit() {
                        return;
                    }
                }
            }
        }
    }

    /// Waits out the restart schedule with the last frame still showing, or
    /// rests empty once the renderer is stopped. False when the target goes.
    fn after_exit(&mut self) -> bool {
        match self.host.due() {
            Some(wait) => {
                // A failed start keeps its reason on show through the wait.
                let failed = match &self.shared.lock().refused {
                    Some(Refused::Start(why)) => Some(Refused::Start(why.clone())),
                    _ => None,
                };
                self.set(Phase::Starting, failed);
                self.sleep(wait)
            }
            None => {
                log::info!("render target {}: stopped after repeated exits", self.tag);
                self.set(Phase::Resting, Some(Refused::Stopped));
                self.clear();
                self.rest()
            }
        }
    }

    /// Moves the status line, and wakes the canvas only when it moved, so a
    /// target resting or retrying costs the canvas no frames.
    fn set(&self, phase: Phase, refused: Option<Refused>) {
        let moved = {
            let mut state = self.shared.lock();
            let moved = state.phase != phase || state.refused != refused;
            state.phase = phase;
            state.refused = refused;
            moved
        };

        if moved {
            (self.notify)();
        }
    }

    /// Until `wait` is up. False when the target goes.
    fn sleep(&self, wait: Duration) -> bool {
        let until = Instant::now() + wait;
        let mut state = self.shared.lock();

        loop {
            if state.stop {
                return false;
            }
            let now = Instant::now();
            if now >= until {
                return true;
            }
            state = self.shared.wait(state, Some(until - now));
        }
    }

    /// Until the renderers' switches move. False when the target goes.
    fn rest(&self) -> bool {
        let mut state = self.shared.lock();

        loop {
            if state.stop {
                return false;
            }
            if state.retry {
                return true;
            }
            state = self.shared.wait(state, None);
        }
    }

    /// A stopped target draws empty.
    fn clear(&self) {
        let had = self.shared.lock().frame.take().is_some();
        if had {
            (self.notify)();
        }
    }

    /// One session's life: copies on announce, pause lines on change, and
    /// the canvas's lines while it runs unpaused.
    fn drive(&mut self, run: &mut dyn Run, mapping: &Mapping) -> Exit {
        let limits = self.limits;
        let mut told = false;
        let mut pipe = Pipe::default();
        let mut resend_at: Option<Instant> = None;
        let mut refused = 0u32;
        let mut last_seq = 0u64;

        loop {
            let job = {
                let mut state = self.shared.lock();

                loop {
                    if state.stop {
                        return Exit::Stop;
                    }
                    match state.ended.take() {
                        Some(End::Closed) => return Exit::Closed,
                        Some(End::Flood) => return Exit::Flood,
                        None => {}
                    }

                    let now = Instant::now();
                    let idle_at = state.published_at + limits.idle;
                    if !state.taken && !state.idle && now >= idle_at {
                        state.idle = true;
                        state.out.pause();
                    }

                    let paused = state.paused();
                    if resend_at.is_some_and(|at| now >= at) {
                        resend_at = None;
                    }

                    // The pause line goes in order behind what the pipe
                    // holds, and nothing queued goes until it's told to go on.
                    let mut line_at = None;
                    if resend_at.is_none() {
                        if pipe.busy() {
                            break Job::Flush;
                        }
                        if paused != told {
                            break Job::Pause(paused);
                        }
                        if !paused {
                            match state.out.next(now) {
                                Ok(line) => break Job::Send(line),
                                Err(at) => line_at = at,
                            }
                        }
                    }

                    let copy = !paused && state.taken && state.pending.is_some();
                    if copy && self.pool.has_free() {
                        let slot = state.pending.take().unwrap_or_default();
                        break Job::Copy(slot);
                    }

                    // Only what can come due needs a timeout. With nothing
                    // pending the thread sleeps until it's woken.
                    let mut within: Option<Duration> = None;
                    let mut soonest = |at: Instant| {
                        let left = at.saturating_duration_since(now);
                        within = Some(within.map_or(left, |w| w.min(left)));
                    };
                    if !state.taken && !state.idle {
                        soonest(idle_at);
                    }
                    if let Some(at) = resend_at.or(line_at) {
                        soonest(at);
                    }
                    if copy {
                        soonest(now + POOL_RETRY);
                    }
                    if paused {
                        soonest(now + PAUSED_CHECK);
                    }

                    state = self.shared.wait(state, within);

                    // Its stdout goes unread while paused, so an exit shows
                    // only here.
                    if state.paused() {
                        drop(state);
                        if run.exited() {
                            return Exit::Closed;
                        }
                        state = self.shared.lock();
                    }
                }
            };

            let line = match job {
                Job::Pause(on) => {
                    told = on;
                    Some(pause_line(on))
                }
                Job::Send(line) => Some(line),
                Job::Flush => None,

                Job::Copy(slot) => {
                    match self.copy(mapping, slot, &mut last_seq) {
                        Copied::Shown | Copied::Old => refused = 0,
                        Copied::Torn => {}
                        Copied::Refused => {
                            refused += 1;
                            if refused >= limits.refused_run {
                                return Exit::Lies;
                            }
                        }
                    }
                    continue;
                }
            };

            if let Some(line) = line {
                pipe.put(line);
            }
            if let Sent::Full = pipe.flush(run) {
                resend_at = Some(Instant::now() + RESEND);
            }
        }
    }

    fn copy(&mut self, mapping: &Mapping, slot: u32, last_seq: &mut u64) -> Copied {
        let Some(ix) = self.pool.free() else {
            return Copied::Old;
        };
        let Some(buffer) = Arc::get_mut(&mut self.pool.buffers[ix]) else {
            return Copied::Old;
        };

        self.shared.copies.fetch_add(1, Ordering::Relaxed);
        let read = mapping.copy_slot(slot, buffer);

        let seq = match read {
            rox_frames::Read::Whole { seq, .. } => seq,

            // Mid-write when the copy began, or rewritten under it.
            rox_frames::Read::Torn | rox_frames::Read::Refused(Refusal::Writing) => {
                self.shared.torn.fetch_add(1, Ordering::Relaxed);
                return Copied::Torn;
            }

            rox_frames::Read::Refused(why) => {
                log::debug!("render target {}: slot {slot} refused: {why:?}", self.tag);
                return Copied::Refused;
            }
        };

        // An announce for a slot already shown, or rewound.
        if seq <= *last_seq {
            return Copied::Old;
        }
        *last_seq = seq;
        self.published += 1;

        let frame = Frame {
            width: self.spec.width(),
            height: self.spec.height(),
            rgba8: self.pool.buffers[ix].clone(),
            seq: self.published,
        };
        {
            let mut state = self.shared.lock();
            state.frame = Some(frame);
            state.taken = false;
            state.published_at = Instant::now();
        }
        (self.notify)();

        Copied::Shown
    }

    fn join_reader(&self, session: u64, reader: Option<JoinHandle<()>>) {
        let Some(reader) = reader else {
            return;
        };

        let until = Instant::now() + READER_GRACE;
        let mut state = self.shared.lock();
        while state.read_done < session && Instant::now() < until {
            let left = until.saturating_duration_since(Instant::now());
            state = self.shared.wait(state, Some(left));
        }
        let done = state.read_done >= session;
        drop(state);

        if done {
            let _ = reader.join();
        } else {
            log::warn!(
                "render target {}: its stdout is still open after the kill, leaving the reader",
                self.tag
            );
        }
    }
}

enum Copied {
    Shown,
    /// Nothing new: already shown, or no buffer free.
    Old,
    Torn,
    Refused,
}

fn pause_line(on: bool) -> String {
    format!("{}\n", serde_json::json!({ "pause": on }))
}

/// The bytes of the one line on its way, so a line the pipe took in part is
/// finished before the next starts.
#[derive(Default)]
struct Pipe {
    bytes: Vec<u8>,
    sent: usize,
}

impl Pipe {
    fn busy(&self) -> bool {
        self.sent < self.bytes.len()
    }

    fn put(&mut self, line: String) {
        debug_assert!(!self.busy(), "a line goes only into an empty pipe");
        self.bytes = line.into_bytes();
        self.sent = 0;
    }

    fn flush(&mut self, run: &mut dyn Run) -> Sent {
        while self.busy() {
            match run.send(&self.bytes[self.sent..]) {
                Sent::Wrote(0) | Sent::Full => return Sent::Full,
                Sent::Wrote(n) => self.sent += n,
                Sent::Closed => {
                    self.sent = self.bytes.len();
                    return Sent::Closed;
                }
            }
        }
        Sent::Wrote(self.bytes.len())
    }
}

// ── What a renderer is sent ──
//
// The boundary of ADR 37's trust rules. Past `hello` and `pause`, a renderer
// is sent exactly the two things its enable card names (`plugins::grants`):
// the music's signals, as the pool's values and names with the playing
// position, and the pointer while it's over the target's own surface. No
// track, path, key or other item's input crosses here. A new kind of line
// needs a new grant and new card copy first.

/// One frame's reading for every target a canvas draws.
pub(crate) struct Feed {
    pool: String,
    /// None once playback stopped and the signals came to rest.
    signals: Option<String>,
}

/// `{"pool": [{"id": 3, "name": "Bass"}, ...]}`, in pool order.
fn pool_line(pool: &[Signal]) -> String {
    let named: Vec<serde_json::Value> = pool
        .iter()
        .map(|signal| serde_json::json!({ "id": signal.id, "name": wire_name(signal) }))
        .collect();
    format!("{}\n", serde_json::json!({ "pool": named }))
}

/// The user's name for a signal, or one worded from its source. English in
/// any UI language, as every machine contract is.
fn wire_name(signal: &Signal) -> String {
    if let Some(name) = signal.given_name() {
        return name.chars().take(NAME_MAX).collect();
    }

    let band = |kind: &str, lo: f32, hi: f32| format!("{kind} {}-{} Hz", lo.round(), hi.round());
    match signal.source {
        SignalSource::Band { lo, hi } => band("band", lo, hi),
        SignalSource::Onset { lo, hi } => band("onset", lo, hi),
        SignalSource::Trigger { lo, hi } => band("trigger", lo, hi),
        SignalSource::Level => "level".to_string(),
        SignalSource::Aggregate { of, .. } => format!("total of {of}"),
    }
}

/// `{"signals": [...], "time": t}`: each pool signal's gated value in pool
/// order, as routes read it, and the playing position in seconds.
fn signals_line(pool: &[Signal], hub: &SignalHub, time: f64) -> String {
    let values: Vec<f64> = pool
        .iter()
        .map(|signal| rounded(f64::from(hub.value(signal.id).unwrap_or(0.)), 4))
        .collect();
    format!(
        "{}\n",
        serde_json::json!({ "signals": values, "time": rounded(time, 3) })
    )
}

/// A finite number to `places` decimals, so a line reads 0.3 and not
/// 0.30000001192092896.
fn rounded(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    if value.is_finite() {
        (value * scale).round() / scale
    } else {
        0.
    }
}

/// One pointer event on a target's surface, 0 to 1 across it from its
/// top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Pointer {
    Move([f32; 2]),
    Down([f32; 2], &'static str),
    Up([f32; 2], &'static str),
    /// Where, and how far in lines.
    Scroll([f32; 2], [f32; 2]),
    /// Off the surface. It carries no point, and it ends any press.
    Leave,
}

impl Pointer {
    fn line(&self) -> String {
        let at = |[x, y]: [f32; 2]| (rounded(f64::from(x), 4), rounded(f64::from(y), 4));

        let pointer = match *self {
            Pointer::Move(p) => {
                let (x, y) = at(p);
                serde_json::json!({ "kind": "move", "x": x, "y": y })
            }
            Pointer::Down(p, button) => {
                let (x, y) = at(p);
                serde_json::json!({ "kind": "down", "x": x, "y": y, "button": button })
            }
            Pointer::Up(p, button) => {
                let (x, y) = at(p);
                serde_json::json!({ "kind": "up", "x": x, "y": y, "button": button })
            }
            Pointer::Scroll(p, delta) => {
                let (x, y) = at(p);
                let (dx, dy) = at(delta);
                serde_json::json!({ "kind": "scroll", "x": x, "y": y, "dx": dx, "dy": dy })
            }
            Pointer::Leave => serde_json::json!({ "kind": "leave" }),
        };
        format!("{}\n", serde_json::json!({ "pointer": pointer }))
    }

    /// Which goes first from a full queue: the lowest.
    fn keep(&self) -> u8 {
        match self {
            Pointer::Move(_) => 0,
            Pointer::Scroll(..) => 1,
            Pointer::Leave => 2,
            Pointer::Down(..) | Pointer::Up(..) => 3,
        }
    }
}

/// What the canvas has for a renderer and the pipe hasn't taken. Bounded
/// whatever the renderer does: the newest pool line, the newest signals
/// line, and at most [`POINTER_QUEUE`] pointer events.
#[derive(Default)]
struct Outbox {
    /// The pool's line as the canvas last gave it.
    pool: Option<String>,
    /// This session hasn't been sent that line yet.
    pool_due: bool,
    /// Lossy: a newer reading replaces one not yet sent.
    signals: Option<String>,
    pointer: VecDeque<Pointer>,
    last_move: Option<Instant>,
}

impl Outbox {
    fn name_pool(&mut self, line: &str) {
        if self.pool.as_deref() != Some(line) {
            self.pool = Some(line.to_string());
            self.pool_due = true;
        }
    }

    fn point(&mut self, pointer: Pointer) {
        // A move behind a move still waiting folds into it.
        if let (Pointer::Move(at), Some(Pointer::Move(waiting))) =
            (pointer, self.pointer.back_mut())
        {
            *waiting = at;
            return;
        }

        if self.pointer.len() >= POINTER_QUEUE {
            let least = self.pointer.iter().map(Pointer::keep).min().unwrap_or(0);
            if let Some(ix) = self.pointer.iter().position(|p| p.keep() == least) {
                self.pointer.remove(ix);
            }
        }
        self.pointer.push_back(pointer);
    }

    /// A new session hasn't heard the pool, and nothing queued was for it.
    fn restart(&mut self) {
        self.pool_due = self.pool.is_some();
        self.signals = None;
        self.pointer.clear();
        self.last_move = None;
    }

    /// Paused: what was waiting is stale by the time it goes on.
    fn pause(&mut self) {
        self.signals = None;
        self.pointer.clear();
    }

    /// The next line due at `now`. Otherwise when a move held back for
    /// [`MOVE_GAP`] comes due, or None with nothing waiting.
    fn next(&mut self, now: Instant) -> Result<String, Option<Instant>> {
        if self.pool_due
            && let Some(pool) = &self.pool
        {
            self.pool_due = false;
            return Ok(pool.clone());
        }

        let mut held = None;
        if let Some(head) = self.pointer.front() {
            // Only a lone move waits: anything behind it keeps the order.
            let due = self.last_move.map(|at| at + MOVE_GAP);
            let lone = self.pointer.len() == 1;
            match (head, due) {
                (Pointer::Move(_), Some(due)) if lone && now < due => held = Some(due),
                _ => {
                    if let Some(pointer) = self.pointer.pop_front() {
                        if let Pointer::Move(_) = pointer {
                            self.last_move = Some(now);
                        }
                        return Ok(pointer.line());
                    }
                }
            }
        }

        if let Some(line) = self.signals.take() {
            return Ok(line);
        }
        Err(held)
    }
}

/// The session's stdout, line by line, until it closes or floods. Only the
/// newest announce among the lines already waiting is passed on. While the
/// session is paused nothing is read, so a renderer that keeps announcing
/// fills its own pipe and blocks there.
fn read(shared: Arc<Shared>, session: u64, stdout: Box<dyn Read + Send>, limits: Limits) {
    let mut reader = BufReader::with_capacity(4096, stdout);
    let mut line = Vec::with_capacity(ANNOUNCE_MAX);
    let mut rate = Rate::new(limits);
    let mut newest = None;

    let end = loop {
        if reader.buffer().is_empty() {
            let mut state = shared.lock();
            while state.paused() && state.session == session && !state.stop {
                state = shared.wait(state, None);
            }
            if state.session != session || state.stop {
                break End::Closed;
            }
        }

        // The cap counts the newline, which `read_line` strips.
        match process::read_line(&mut reader, &mut line, ANNOUNCE_MAX) {
            Ok(Line::End) | Err(_) => break End::Closed,
            Ok(Line::Clipped) => {}
            Ok(Line::Whole) => {
                if line.len() < ANNOUNCE_MAX
                    && let Some((_, slot)) = rox_frames::parse_announce(&line)
                {
                    newest = Some(slot);
                }
            }
        }

        if rate.note(Instant::now()) {
            break End::Flood;
        }

        if reader.buffer().contains(&b'\n') {
            continue;
        }
        if let Some(slot) = newest.take() {
            let mut state = shared.lock();
            if state.session != session {
                break End::Closed;
            }
            state.pending = Some(slot);

            // Until the canvas takes the last frame the supervisor has
            // nothing to do with it, and the take wakes it.
            let copies = state.taken && !state.paused();
            drop(state);
            if copies {
                shared.wake.notify_all();
            }
        }
    };

    let mut state = shared.lock();
    if state.session == session {
        state.ended = Some(end);
    }
    state.read_done = state.read_done.max(session);
    drop(state);
    shared.wake.notify_all();
}

/// Lines per span, and how many spans in a row ran over.
struct Rate {
    limits: Limits,
    since: Option<Instant>,
    lines: u32,
    over: u32,
}

impl Rate {
    fn new(limits: Limits) -> Rate {
        Rate {
            limits,
            since: None,
            lines: 0,
            over: 0,
        }
    }

    /// Counts one line. True once the flood rule is broken.
    fn note(&mut self, now: Instant) -> bool {
        let span = self.limits.flood_span;
        let since = *self.since.get_or_insert(now);
        let spans = now.saturating_duration_since(since).as_nanos() / span.as_nanos().max(1);

        if spans > 0 {
            let flooded = self.lines > self.limits.flood_lines;
            // A span with no lines in it breaks the run.
            self.over = if flooded && spans == 1 {
                self.over + 1
            } else {
                0
            };
            self.lines = 0;
            self.since = Some(since + span * u32::try_from(spans).unwrap_or(u32::MAX));
        }

        self.lines = self.lines.saturating_add(1);
        let running = u32::from(self.lines > self.limits.flood_lines);
        self.over + running >= self.limits.flood_spans
    }
}

/// One render target's session, as the canvas holds it. Dropping it stops
/// the renderer, off the UI thread.
pub(crate) struct Target {
    source: Source,
    executor: BackgroundExecutor,
    _wake: Task<()>,
}

impl Target {
    /// Starts supervising renderer `id` at `width` by `height`. Nothing
    /// blocks here: the gate and the spawn run on the supervisor.
    pub(crate) fn new<V: 'static>(
        id: &str,
        width: u32,
        height: u32,
        cx: &mut Context<V>,
    ) -> Target {
        let width = width.clamp(*renderers::WIDTHS.start(), *renderers::WIDTHS.end());
        let height = height.clamp(*renderers::HEIGHTS.start(), *renderers::HEIGHTS.end());

        // `Spec::new` clamps to the same range, so this only fails on a
        // platform that can't address the largest frame.
        let spec = match Spec::new(width, height) {
            Ok(spec) => spec,
            Err(e) => {
                log::warn!("render target {id}: {e}");
                Spec::new(16, 16).expect("the smallest spec fits")
            }
        };

        let (tx, rx) = async_channel::bounded::<()>(1);
        let wake = cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });

        let host = Renderer {
            id: id.to_string(),
            width,
            height,
        };
        let notify = Box::new(move || {
            let _ = tx.try_send(());
        });

        Target {
            source: Source::spawn(id, Box::new(host), spec, LIMITS, notify),
            executor: cx.background_executor().clone(),
            _wake: wake,
        }
    }

    /// The newest frame, if the renderer has drawn one. Call it only for a
    /// target that is drawn this frame.
    pub(crate) fn take(&self) -> Option<Frame> {
        self.source.take()
    }

    /// The canvas skipped the item, so its renderer can pause.
    pub(crate) fn hold(&self) {
        self.source.hold();
    }

    fn feed(&self, feed: &Feed) {
        self.source.feed(feed);
    }

    fn point(&self, pointer: Pointer) {
        self.source.point(pointer);
    }

    pub(crate) fn name(&self) -> Option<String> {
        self.source.name()
    }

    pub(crate) fn status(&self) -> Status {
        self.source.status()
    }

    pub(crate) fn start_again(&self) {
        self.source.start_again();
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        // The stop waits out the renderer's grace, so it's joined on the
        // background executor and never here.
        if let Some(thread) = self.source.stop() {
            self.executor
                .spawn(async move {
                    let _ = thread.join();
                })
                .detach();
        }
    }
}

/// The renderers' switches moved: every target resting on a refusal tries
/// again, and the gate decides. The plugins' apply calls it.
pub(crate) fn renderers_moved(_cx: &mut App) {
    let live: Vec<Arc<Shared>> = LIVE
        .lock()
        .map(|live| live.iter().filter_map(Weak::upgrade).collect())
        .unwrap_or_default();

    for shared in live {
        shared.lock().retry = true;
        shared.wake.notify_all();
    }
}

/// The renderer an owner names, if it names one.
pub(crate) fn renderer_id(owner: &str) -> Option<&str> {
    owner.strip_prefix(OWNER_PREFIX).filter(|id| !id.is_empty())
}

/// The owner a target saves for renderer `id`.
pub(crate) fn owner_of(id: &str) -> String {
    format!("{OWNER_PREFIX}{id}")
}

/// Every renderer a render target anywhere in `value` names, by id: an item,
/// an object's item or a whole layout dump.
pub(crate) fn named_renderers(value: &serde_json::Value, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            let target = map.get("kind").and_then(serde_json::Value::as_str) == Some("target");
            if target
                && let Some(id) = map
                    .get("owner")
                    .and_then(serde_json::Value::as_str)
                    .and_then(renderer_id)
            {
                out.insert(id.to_string());
            }

            map.values().for_each(|child| named_renderers(child, out));
        }

        serde_json::Value::Array(items) => {
            items.iter().for_each(|child| named_renderers(child, out));
        }

        _ => {}
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    owner: String,
    width: u32,
    height: u32,
}

fn key_of(content: &Content) -> Option<Key> {
    match content {
        Content::Target {
            owner,
            width,
            height,
            ..
        } => Some(Key {
            owner: owner.clone(),
            width: *width,
            height: *height,
        }),
        _ => None,
    }
}

struct Shown {
    key: Key,
    /// None for an owner that names no renderer.
    target: Option<Target>,
    view: Option<Entity<TextureView>>,
}

/// One canvas's render targets, by item id.
#[derive(Default)]
pub(crate) struct Targets {
    shown: HashMap<String, Shown>,
    /// This frame's reading, for every target drawn in it.
    feed: Option<Feed>,
    /// Playback stopped and the signals are still falling to rest.
    tail: bool,
}

/// Whether any item in the canvas, a group's child included, is a render
/// target.
pub(crate) fn holds_targets(canvas: &CanvasState) -> bool {
    let mut found = false;
    canvas.walk(&mut |item, _| found |= matches!(item.content, Content::Target { .. }));
    found
}

impl Targets {
    /// Reads the hub for this frame's targets. Once playback stops the
    /// signals go on only until they come to rest, so a paused canvas reads
    /// the hub no more. Call it only on a frame the canvas draws anyway.
    pub(crate) fn read(&mut self, hub: &SignalHub, running: bool, time: impl FnOnce() -> f64) {
        let send = if running {
            self.tail = true;
            true
        } else if self.tail {
            self.tail = hub.settling();
            true
        } else {
            false
        };

        let pool = hub.pool();
        self.feed = Some(Feed {
            pool: pool_line(&pool),
            signals: send.then(|| signals_line(&pool, hub, time())),
        });
    }

    /// A pointer event for target `id`. The canvas sends one only off
    /// design mode, over the front item, inside its quad.
    pub(crate) fn point(&self, id: &str, pointer: Pointer) {
        if let Some(target) = self.shown.get(id).and_then(|shown| shown.target.as_ref()) {
            target.point(pointer);
        }
    }

    /// The item's element this frame: its newest frame on `face`, or in
    /// design mode its box with the renderer's name while it has none.
    /// None for a face off the stage, which counts as not drawn.
    pub(crate) fn element<V: 'static>(
        &mut self,
        item: &CanvasItem,
        face: Option<Projected>,
        design: bool,
        cx: &mut Context<V>,
    ) -> Option<AnyElement> {
        let key = key_of(&item.content)?;

        let fresh = || Shown {
            key: key.clone(),
            target: None,
            view: None,
        };
        let shown = self.shown.entry(item.id.clone()).or_insert_with(fresh);
        if shown.key != key {
            *shown = fresh();
        }

        let Some(face) = face else {
            if let Some(target) = &shown.target {
                target.hold();
            }
            return None;
        };

        if shown.target.is_none()
            && let Some(id) = renderer_id(&key.owner)
        {
            shown.target = Some(Target::new(id, key.width, key.height, cx));
        }

        let frame = shown.target.as_ref().and_then(Target::take);
        if let (Some(target), Some(feed)) = (&shown.target, &self.feed) {
            target.feed(feed);
        }

        let Some(frame) = frame else {
            if !design {
                return None;
            }
            let name = shown
                .target
                .as_ref()
                .and_then(Target::name)
                .or_else(|| renderer_id(&key.owner).map(str::to_string))
                .unwrap_or_default();
            return Some(empty_box(&face.corners, name));
        };

        let view = match &shown.view {
            Some(view) => view.clone(),
            None => {
                let frame = frame.clone();
                let view = cx.new(|cx| TextureView::new(frame, cx));
                shown.view = Some(view.clone());
                view
            }
        };
        view.update(cx, |view, _| {
            view.show(frame);
            view.place(Some(face));
        });

        Some(view.into_any_element())
    }

    /// The status line for a target item. None when its owner names no
    /// renderer. One the canvas hasn't drawn yet has no session, and reads as
    /// paused until it's drawn.
    pub(crate) fn status(&self, item: &CanvasItem) -> Option<Status> {
        let key = key_of(&item.content)?;
        renderer_id(&key.owner)?;

        let target = self
            .shown
            .get(&item.id)
            .filter(|shown| shown.key == key)
            .and_then(|shown| shown.target.as_ref());
        Some(target.map_or(Status::Paused, Target::status))
    }

    pub(crate) fn start_again(&self, id: &str) {
        if let Some(target) = self.shown.get(id).and_then(|shown| shown.target.as_ref()) {
            target.start_again();
        }
    }

    /// The canvas skipped the item this frame.
    pub(crate) fn hold(&self, id: &str) {
        if let Some(target) = self.shown.get(id).and_then(|shown| shown.target.as_ref()) {
            target.hold();
        }
    }

    /// Stops the targets whose items are gone, or whose renderer or size
    /// moved: a new size is a new session.
    pub(crate) fn keep(&mut self, canvas: &CanvasState, window: &mut Window, cx: &mut App) {
        self.shown.retain(|id, shown| {
            let kept = canvas
                .find(id)
                .and_then(|item| key_of(&item.content))
                .is_some_and(|key| key == shown.key);

            if !kept && let Some(view) = &shown.view {
                view.update(cx, |view, cx| view.release(window, cx));
            }
            kept
        });
    }

    /// Every target stops: the canvas left its window.
    pub(crate) fn clear(&mut self, window: &mut Window, cx: &mut App) {
        for shown in self.shown.values() {
            if let Some(view) = &shown.view {
                view.update(cx, |view, cx| view.release(window, cx));
            }
        }
        self.shown.clear();
    }
}

/// Design mode's mark for a target with no picture: its box, and the
/// renderer's name as the package gives it.
fn empty_box(corners: &[[f32; 2]; 4], name: String) -> AnyElement {
    let (origin, extent) = bbox(corners);

    div()
        .absolute()
        .left(px(origin[0]))
        .top(px(origin[1]))
        .w(px(extent[0]))
        .h(px(extent[1]))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_dashed()
        .border_color(palette::text_faint())
        .text_xs()
        .text_color(palette::text_faint())
        .child(SharedString::from(name))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, PipeReader, PipeWriter, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::mpsc;

    use rox_frames::Writer;

    use super::*;

    const SIZE: (u32, u32) = (16, 16);

    /// What the test holds of one fake session.
    struct Fake {
        writer: Writer,
        stdout: Arc<Mutex<Option<PipeWriter>>>,
        /// Lines rox wrote to the renderer's stdin.
        stdin: mpsc::Receiver<String>,
    }

    impl Fake {
        fn announce(&self, lines: &str) {
            if let Some(out) = self.stdout.lock().unwrap().as_mut() {
                out.write_all(lines.as_bytes()).unwrap();
            }
        }

        /// The renderer exits: its stdout closes.
        fn exit(&self) {
            self.stdout.lock().unwrap().take();
        }

        fn told(&self) -> Option<String> {
            self.stdin.recv_timeout(Duration::from_secs(2)).ok()
        }
    }

    struct FakeRun {
        stdin: Option<PipeWriter>,
        stdout: Arc<Mutex<Option<PipeWriter>>>,
        /// A renderer that never reads: its pipe is always full.
        stuck: Arc<AtomicBool>,
    }

    impl Run for FakeRun {
        fn send(&mut self, bytes: &[u8]) -> Sent {
            if self.stuck.load(Ordering::SeqCst) {
                return Sent::Full;
            }

            match self.stdin.as_mut().map(|stdin| stdin.write_all(bytes)) {
                Some(Ok(())) => Sent::Wrote(bytes.len()),
                _ => Sent::Closed,
            }
        }

        fn exited(&self) -> bool {
            self.stdout.lock().unwrap().is_none()
        }

        fn stopped(&self) -> bool {
            false
        }

        fn stop(&mut self) {
            self.stdin.take();
            self.stdout.lock().unwrap().take();
        }
    }

    #[derive(Clone)]
    struct FakeHost {
        sessions: mpsc::Sender<Fake>,
        exits: Arc<AtomicUsize>,
        due: Arc<Mutex<Option<Duration>>>,
        /// What the gate says, None to let it start.
        refuse: Arc<Mutex<Option<Refused>>>,
        forgiven: Arc<AtomicUsize>,
        stuck: Arc<AtomicBool>,
    }

    impl Host for FakeHost {
        fn start(&mut self, mapping: &Mapping) -> Result<Started, Refused> {
            if let Some(refused) = self.refuse.lock().unwrap().clone() {
                return Err(refused);
            }

            // SAFETY: the only writer for this mapping.
            let writer = unsafe { Writer::open(mapping.child_handle(), *mapping.spec()) }.unwrap();
            let (stdout_r, stdout_w) = std::io::pipe().unwrap();
            let (stdin_r, stdin_w) = std::io::pipe().unwrap();
            let stdout = Arc::new(Mutex::new(Some(stdout_w)));

            self.sessions
                .send(Fake {
                    writer,
                    stdout: stdout.clone(),
                    stdin: lines_of(stdin_r),
                })
                .unwrap();

            let run = FakeRun {
                stdin: Some(stdin_w),
                stdout,
                stuck: self.stuck.clone(),
            };
            Ok((Box::new(run), Box::new(stdout_r)))
        }

        fn due(&self) -> Option<Duration> {
            *self.due.lock().unwrap()
        }

        fn note_exit(&self) {
            self.exits.fetch_add(1, Ordering::SeqCst);
        }

        fn name(&self) -> Option<String> {
            Some("Fake".into())
        }

        fn forgive(&self) {
            self.forgiven.fetch_add(1, Ordering::SeqCst);
            *self.due.lock().unwrap() = Some(Duration::ZERO);
        }
    }

    fn lines_of(pipe: PipeReader) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(pipe).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        rx
    }

    struct Rig {
        source: Source,
        host: FakeHost,
        sessions: mpsc::Receiver<Fake>,
    }

    impl Rig {
        fn new(limits: Limits) -> Rig {
            Rig::refusing(limits, None)
        }

        fn refusing(limits: Limits, refused: Option<Refused>) -> Rig {
            let (tx, sessions) = mpsc::channel();
            let host = FakeHost {
                sessions: tx,
                exits: Arc::default(),
                due: Arc::new(Mutex::new(Some(Duration::ZERO))),
                refuse: Arc::new(Mutex::new(refused)),
                forgiven: Arc::default(),
                stuck: Arc::default(),
            };
            let spec = Spec::new(SIZE.0, SIZE.1).unwrap();
            let source = Source::spawn(
                "test",
                Box::new(host.clone()),
                spec,
                limits,
                Box::new(|| {}),
            );

            Rig {
                source,
                host,
                sessions,
            }
        }

        fn session(&self) -> Fake {
            self.sessions
                .recv_timeout(Duration::from_secs(2))
                .expect("a session starts")
        }

        fn copies(&self) -> u64 {
            self.source.shared.copies.load(Ordering::SeqCst)
        }

        fn exits(&self) -> usize {
            self.host.exits.load(Ordering::SeqCst)
        }

        /// The published frame, without taking it.
        fn peek(&self) -> Option<Frame> {
            self.source.shared.lock().frame.clone()
        }

        fn finish(mut self) {
            if let Some(thread) = self.source.stop() {
                thread.join().unwrap();
            }
        }
    }

    /// Every pixel of frame `n`.
    fn pattern(n: u64) -> impl FnOnce(&mut [u8]) {
        move |pixels| pixels.fill(n as u8)
    }

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn settle() {
        std::thread::sleep(Duration::from_millis(100));
    }

    fn slow() -> Limits {
        Limits {
            idle: Duration::from_secs(60),
            ..LIMITS
        }
    }

    #[test]
    fn the_newest_announce_wins() {
        let rig = Rig::new(slow());
        let mut fake = rig.session();

        fake.writer.frame(1, pattern(1));
        fake.writer.frame(2, pattern(2));
        fake.announce("f 1 1\nf 2 0\n");

        until("a frame", || rig.peek().is_some());
        settle();

        let frame = rig.peek().unwrap();
        assert_eq!(rig.copies(), 1, "the older announce got no copy");
        assert_eq!(frame.seq, 1);
        assert!(frame.rgba8.iter().all(|&b| b == 2), "frame 2's pixels");

        rig.finish();
    }

    #[test]
    fn no_copy_is_made_until_the_last_is_taken() {
        let rig = Rig::new(slow());
        let mut fake = rig.session();

        fake.writer.frame(0, pattern(10));
        fake.announce("f 0 0\n");
        until("the first frame", || rig.peek().is_some());

        fake.writer.frame(1, pattern(11));
        fake.announce("f 1 1\n");
        settle();
        assert_eq!(rig.copies(), 1, "nothing took the first, so no second copy");
        assert_eq!(rig.peek().map(|f| f.seq), Some(1));

        let first = rig.source.take().unwrap();
        assert_eq!(first.seq, 1);

        until("the second frame", || {
            rig.peek().is_some_and(|f| f.seq == 2)
        });
        assert_eq!(rig.copies(), 2);
        assert!(rig.peek().unwrap().rgba8.iter().all(|&b| b == 11));

        rig.finish();
    }

    #[test]
    fn a_torn_frame_is_dropped() {
        let rig = Rig::new(slow());
        let mut fake = rig.session();
        let torn = || rig.source.shared.torn.load(Ordering::SeqCst);

        // Announced while the slot is still being written.
        let stdout = fake.stdout.clone();
        fake.writer.frame(0, |pixels| {
            pixels.fill(7);
            if let Some(out) = stdout.lock().unwrap().as_mut() {
                out.write_all(b"f 0 0\n").unwrap();
            }
            until("the copy to start", || torn() == 1);
        });

        settle();
        assert_eq!(rig.peek().map(|f| f.seq), None, "nothing torn is published");
        assert_eq!(rig.exits(), 0, "a torn copy isn't a lie");

        // The same slot announced whole shows.
        fake.announce("f 0 0\n");
        until("the frame", || rig.peek().is_some());
        assert!(rig.peek().unwrap().rgba8.iter().all(|&b| b == 7));

        rig.finish();
    }

    #[test]
    fn a_run_of_refused_frames_stops_it() {
        let rig = Rig::new(Limits {
            refused_run: 5,
            ..slow()
        });
        let fake = rig.session();

        // Slot 1 was never written, so every announce of it is refused.
        for _ in 0..5 {
            let before = rig.copies();
            fake.announce("f 3 1\n");
            until("the copy", || rig.copies() > before);
        }

        until("the exit", || rig.exits() == 1);
        assert!(
            fake.stdout.lock().unwrap().is_none(),
            "the session was stopped"
        );

        let _again = rig.session();
        assert_eq!(rig.peek().map(|f| f.seq), None);

        rig.finish();
    }

    #[test]
    fn a_flood_stops_it() {
        let rig = Rig::new(Limits {
            flood_lines: 20,
            flood_span: Duration::from_millis(50),
            ..slow()
        });
        let fake = rig.session();

        // Junk lines, so it's the rate and not a refusal that stops it.
        let stdout = fake.stdout.clone();
        let flooding = std::thread::spawn(move || {
            loop {
                let mut out = stdout.lock().unwrap();
                let Some(pipe) = out.as_mut() else { return };
                if pipe.write_all(b"junk\n").is_err() {
                    return;
                }
            }
        });

        until("the exit", || rig.exits() == 1);
        flooding.join().unwrap();
        assert_eq!(rig.copies(), 0);

        let _again = rig.session();
        rig.finish();
    }

    #[test]
    fn an_exit_keeps_the_last_frame() {
        let rig = Rig::new(slow());
        *rig.host.due.lock().unwrap() = Some(Duration::from_secs(60));
        let mut fake = rig.session();

        fake.writer.frame(0, pattern(5));
        fake.announce("f 0 0\n");
        until("a frame", || rig.peek().is_some());
        assert!(rig.source.take().is_some());

        fake.exit();
        until("the exit", || rig.exits() == 1);
        settle();
        assert!(
            rig.source.take().is_some(),
            "the last frame shows through the wait"
        );
        assert!(
            rig.sessions.try_recv().is_err(),
            "no restart before it's due"
        );
        rig.finish();

        // Once it's stopped for good, it draws empty.
        let rig = Rig::new(slow());
        *rig.host.due.lock().unwrap() = None;
        let mut fake = rig.session();

        fake.writer.frame(0, pattern(5));
        fake.announce("f 0 0\n");
        until("a frame", || rig.peek().is_some());

        fake.exit();
        until("the exit", || rig.exits() == 1);
        until("the frame to go", || rig.peek().is_none());
        rig.finish();
    }

    #[test]
    fn pause_stops_the_reading() {
        let rig = Rig::new(slow());
        let mut fake = rig.session();

        rig.source.hold();
        assert_eq!(fake.told().as_deref(), Some(r#"{"pause":true}"#));

        fake.writer.frame(0, pattern(9));
        fake.announce("f 0 0\n");
        settle();
        assert_eq!(rig.copies(), 0, "a paused target copies nothing");
        assert!(rig.peek().is_none());

        // Drawn again: told to go on, and the waiting announce is read.
        rig.source.take();
        assert_eq!(fake.told().as_deref(), Some(r#"{"pause":false}"#));
        until("the frame", || rig.peek().is_some());

        rig.finish();
    }

    #[test]
    fn an_untaken_frame_pauses_it() {
        let rig = Rig::new(Limits {
            idle: Duration::from_millis(50),
            ..LIMITS
        });
        let mut fake = rig.session();

        fake.writer.frame(0, pattern(1));
        fake.announce("f 0 0\n");

        // Nothing takes it, as when the canvas isn't drawn.
        assert_eq!(fake.told().as_deref(), Some(r#"{"pause":true}"#));

        rig.source.take();
        assert_eq!(fake.told().as_deref(), Some(r#"{"pause":false}"#));
        rig.finish();
    }

    #[test]
    fn the_flood_rule_needs_spans_in_a_row() {
        let limits = Limits {
            flood_lines: 3,
            flood_span: Duration::from_secs(1),
            flood_spans: 2,
            ..LIMITS
        };
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);

        // Four lines in one span, then a quiet span, then four more.
        let mut rate = Rate::new(limits);
        for ms in [0, 1, 2, 3, 2500, 2501, 2502] {
            assert!(!rate.note(at(ms)), "{ms}");
        }

        // Over in two spans running is a flood.
        let mut rate = Rate::new(limits);
        for ms in [0, 1, 2, 3, 1000, 1001, 1002] {
            assert!(!rate.note(at(ms)), "{ms}");
        }
        assert!(rate.note(at(1003)));
    }

    #[test]
    fn the_status_line_follows_the_session() {
        let rig = Rig::new(slow());
        let status = || rig.source.status();
        let fake = rig.session();

        until("running", || status() == Status::Running);

        rig.source.hold();
        assert_eq!(status(), Status::Paused, "held this frame");
        rig.source.take();
        assert_eq!(status(), Status::Running);

        // The fifth exit: stopped until Start Again.
        *rig.host.due.lock().unwrap() = None;
        fake.exit();
        until("stopped", || {
            status() == Status::Stopped(Some(Refused::Stopped))
        });

        rig.source.start_again();
        let _again = rig.session();
        until("running again", || status() == Status::Running);
        assert_eq!(rig.host.forgiven.load(Ordering::SeqCst), 1);

        rig.finish();
    }

    #[test]
    fn a_refusal_stays_on_the_status_line_until_it_starts() {
        let rig = Rig::refusing(slow(), Some(Refused::NotApproved));
        let status = || rig.source.status();

        until("the refusal", || {
            status() == Status::Stopped(Some(Refused::NotApproved))
        });
        assert!(rig.sessions.try_recv().is_err());

        // Approved on the Plugins page, and the switches moved.
        *rig.host.refuse.lock().unwrap() = None;
        rig.source.shared.lock().retry = true;
        rig.source.shared.wake.notify_all();

        let _fake = rig.session();
        until("running", || status() == Status::Running);
        rig.finish();
    }

    #[test]
    fn a_failed_start_says_why_while_it_waits() {
        let rig = Rig::refusing(slow(), Some(Refused::Start("no python3".into())));
        *rig.host.due.lock().unwrap() = Some(Duration::from_secs(60));

        until("the reason", || {
            rig.source.status() == Status::Starting(Some("no python3".into()))
        });
        rig.finish();
    }

    fn feed(pool: &str, signals: &str) -> Feed {
        Feed {
            pool: format!("{pool}\n"),
            signals: Some(format!("{signals}\n")),
        }
    }

    /// Every pointer line queued, oldest first.
    fn drain(out: &mut Outbox, now: Instant) -> Vec<String> {
        std::iter::from_fn(|| out.next(now).ok())
            .map(|line| line.trim_end().to_string())
            .collect()
    }

    #[test]
    fn moves_coalesce_and_presses_dont() {
        let mut out = Outbox::default();
        let start = Instant::now();

        for x in [0.1, 0.2, 0.3] {
            out.point(Pointer::Move([x, 0.5]));
        }
        out.point(Pointer::Down([0.3, 0.5], "left"));
        out.point(Pointer::Up([0.3, 0.5], "left"));
        out.point(Pointer::Down([0.3, 0.5], "left"));
        out.point(Pointer::Up([0.3, 0.5], "left"));
        out.point(Pointer::Move([0.4, 0.5]));
        out.point(Pointer::Move([0.5, 0.5]));

        assert_eq!(
            drain(&mut out, start),
            [
                r#"{"pointer":{"kind":"move","x":0.3,"y":0.5}}"#,
                r#"{"pointer":{"kind":"down","x":0.3,"y":0.5,"button":"left"}}"#,
                r#"{"pointer":{"kind":"up","x":0.3,"y":0.5,"button":"left"}}"#,
                r#"{"pointer":{"kind":"down","x":0.3,"y":0.5,"button":"left"}}"#,
                r#"{"pointer":{"kind":"up","x":0.3,"y":0.5,"button":"left"}}"#,
            ],
            "a press keeps its place between the moves around it"
        );

        // The move after the presses waits out the gap from the last move,
        // folding in any that come meanwhile.
        assert_eq!(out.next(start), Err(Some(start + MOVE_GAP)));
        out.point(Pointer::Move([0.6, 0.5]));
        assert_eq!(
            drain(&mut out, start + MOVE_GAP),
            [r#"{"pointer":{"kind":"move","x":0.6,"y":0.5}}"#]
        );

        // Anything behind a waiting move sends it at once, in order.
        let later = start + MOVE_GAP;
        out.point(Pointer::Move([0.8, 0.5]));
        out.point(Pointer::Leave);
        assert_eq!(
            drain(&mut out, later),
            [
                r#"{"pointer":{"kind":"move","x":0.8,"y":0.5}}"#,
                r#"{"pointer":{"kind":"leave"}}"#,
            ]
        );
    }

    #[test]
    fn a_full_channel_drops_moves_first() {
        let mut out = Outbox::default();
        let press = |n: usize| Pointer::Down([n as f32 / 100., 0.], "left");

        // Presses, scrolls and moves, a third each.
        for n in 0..POINTER_QUEUE {
            out.point(match n % 3 {
                0 => press(n),
                1 => Pointer::Scroll([0., 0.], [0., 1.]),
                _ => Pointer::Move([n as f32 / 100., 0.]),
            });
        }
        let moves = |out: &Outbox| {
            out.pointer
                .iter()
                .filter(|p| matches!(p, Pointer::Move(_)))
                .count()
        };
        let scrolls = |out: &Outbox| {
            out.pointer
                .iter()
                .filter(|p| matches!(p, Pointer::Scroll(..)))
                .count()
        };
        let (had_moves, had_scrolls) = (moves(&out), scrolls(&out));
        assert_eq!(out.pointer.len(), POINTER_QUEUE);

        // Each press past the bound takes the oldest move, then the scrolls.
        for n in 0..had_moves {
            out.point(press(100 + n));
            assert_eq!(moves(&out), had_moves - n - 1);
            assert_eq!(scrolls(&out), had_scrolls);
        }
        for n in 0..had_scrolls {
            out.point(press(200 + n));
            assert_eq!(scrolls(&out), had_scrolls - n - 1);
        }
        assert_eq!(out.pointer.len(), POINTER_QUEUE);
        assert!(
            out.pointer.iter().all(|p| p.keep() == 3),
            "only presses left"
        );

        // Full of presses, the oldest press goes, and the bound holds.
        let oldest = out.pointer[0];
        out.point(press(300));
        assert_eq!(out.pointer.len(), POINTER_QUEUE);
        assert_ne!(out.pointer[0], oldest);

        // The signals line is lossy: a newer one replaces one not sent.
        out.signals = Some("old".into());
        out.signals = Some("new".into());
        out.pointer.clear();
        assert_eq!(out.next(Instant::now()), Ok("new".to_string()));
        assert_eq!(out.next(Instant::now()), Err(None));
    }

    #[test]
    fn nothing_is_sent_while_paused() {
        let rig = Rig::new(slow());
        let fake = rig.session();
        until("running", || rig.source.status() == Status::Running);

        rig.source.hold();
        assert_eq!(fake.told().as_deref(), Some(r#"{"pause":true}"#));

        rig.source
            .feed(&feed(r#"{"pool":[]}"#, r#"{"signals":[0.5],"time":1.0}"#));
        rig.source.point(Pointer::Down([0.5, 0.5], "left"));
        rig.source.point(Pointer::Move([0.5, 0.5]));
        settle();
        assert!(
            fake.stdin.try_recv().is_err(),
            "a paused target gets nothing"
        );

        // Drawn again, it's told to go on before anything else.
        rig.source.take();
        assert_eq!(fake.told().as_deref(), Some(r#"{"pause":false}"#));
        settle();
        assert!(
            fake.stdin.try_recv().is_err(),
            "what came in while paused is dropped"
        );

        rig.source
            .feed(&feed(r#"{"pool":[]}"#, r#"{"signals":[0.25],"time":2.0}"#));
        assert_eq!(fake.told().as_deref(), Some(r#"{"pool":[]}"#));
        assert_eq!(
            fake.told().as_deref(),
            Some(r#"{"signals":[0.25],"time":2.0}"#)
        );

        // Stopped for good: nothing more goes.
        *rig.host.due.lock().unwrap() = None;
        fake.exit();
        until("stopped", || {
            rig.source.status() == Status::Stopped(Some(Refused::Stopped))
        });
        rig.source
            .feed(&feed(r#"{"pool":[]}"#, r#"{"signals":[1.0],"time":3.0}"#));
        rig.source.point(Pointer::Move([0.1, 0.1]));
        assert!(rig.source.shared.lock().out.pointer.is_empty());
        assert!(rig.source.shared.lock().out.signals.is_none());

        rig.finish();
    }

    #[test]
    fn the_pool_is_named_once_and_again_when_it_changes() {
        let rig = Rig::new(slow());
        let fake = rig.session();
        until("running", || rig.source.status() == Status::Running);

        let a = r#"{"pool":[{"id":1,"name":"Bass"}]}"#;
        let b = r#"{"pool":[{"id":1,"name":"Kick"}]}"#;

        rig.source.feed(&feed(a, r#"{"signals":[0.1],"time":0.0}"#));
        assert_eq!(fake.told().as_deref(), Some(a));
        assert_eq!(
            fake.told().as_deref(),
            Some(r#"{"signals":[0.1],"time":0.0}"#)
        );

        rig.source.feed(&feed(a, r#"{"signals":[0.2],"time":0.1}"#));
        assert_eq!(
            fake.told().as_deref(),
            Some(r#"{"signals":[0.2],"time":0.1}"#),
            "the same pool isn't named twice"
        );

        rig.source.feed(&feed(b, r#"{"signals":[0.3],"time":0.2}"#));
        assert_eq!(fake.told().as_deref(), Some(b));
        assert_eq!(
            fake.told().as_deref(),
            Some(r#"{"signals":[0.3],"time":0.2}"#)
        );

        // A restarted renderer hears the pool again after its hello.
        fake.exit();
        let again = rig.session();
        until("running again", || rig.source.status() == Status::Running);
        rig.source.feed(&feed(b, r#"{"signals":[0.4],"time":0.3}"#));
        assert_eq!(again.told().as_deref(), Some(b));
        assert_eq!(
            again.told().as_deref(),
            Some(r#"{"signals":[0.4],"time":0.3}"#)
        );

        rig.finish();
    }

    #[test]
    fn a_renderer_that_never_reads_holds_nothing_up() {
        let rig = Rig::new(slow());
        rig.host.stuck.store(true, Ordering::SeqCst);
        let fake = rig.session();
        until("running", || rig.source.status() == Status::Running);

        let start = Instant::now();
        for n in 0..10_000 {
            let x = (n % 100) as f32 / 100.;
            rig.source
                .feed(&feed(r#"{"pool":[]}"#, r#"{"signals":[0.5],"time":1.0}"#));
            rig.source.point(Pointer::Move([x, x]));
            rig.source.point(Pointer::Down([x, x], "left"));
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "queueing never waits on the renderer: {:?}",
            start.elapsed()
        );
        assert!(rig.source.shared.lock().out.pointer.len() <= POINTER_QUEUE);

        // It starts reading: the pool, then what's left, newest signals last.
        rig.host.stuck.store(false, Ordering::SeqCst);
        assert_eq!(fake.told().as_deref(), Some(r#"{"pool":[]}"#));
        let mut last = String::new();
        while let Some(line) = fake.told() {
            last = line;
            if last.starts_with(r#"{"signals""#) {
                break;
            }
        }
        assert_eq!(last, r#"{"signals":[0.5],"time":1.0}"#);

        rig.finish();
    }

    /// A pipe that takes a few bytes a call, and is full every other call.
    struct Trickle {
        got: Vec<u8>,
        calls: usize,
    }

    impl Run for Trickle {
        fn send(&mut self, bytes: &[u8]) -> Sent {
            self.calls += 1;
            if self.calls % 2 == 0 {
                return Sent::Full;
            }

            let n = bytes.len().min(3);
            self.got.extend_from_slice(&bytes[..n]);
            Sent::Wrote(n)
        }

        fn exited(&self) -> bool {
            false
        }

        fn stopped(&self) -> bool {
            false
        }

        fn stop(&mut self) {}
    }

    #[test]
    fn a_line_taken_in_part_is_finished_before_the_next() {
        let mut run = Trickle {
            got: Vec::new(),
            calls: 0,
        };
        let mut pipe = Pipe::default();

        for line in [
            "{\"pool\":[{\"id\":1,\"name\":\"Bass\"}]}\n",
            "{\"pause\":true}\n",
        ] {
            pipe.put(line.to_string());
            while let Sent::Full = pipe.flush(&mut run) {}
            assert!(!pipe.busy());
        }

        assert_eq!(
            String::from_utf8(run.got).unwrap(),
            "{\"pool\":[{\"id\":1,\"name\":\"Bass\"}]}\n{\"pause\":true}\n"
        );
    }

    #[test]
    fn the_lines_say_only_what_the_card_names() {
        let named = Signal {
            id: 3,
            name: " Bass ".into(),
            ..Signal::default()
        };
        let level = Signal {
            id: 4,
            source: SignalSource::Level,
            ..Signal::default()
        };
        let long = Signal {
            id: 5,
            name: "x".repeat(NAME_MAX * 2),
            ..Signal::default()
        };

        let pool = pool_line(&[named, level, long]);
        let read: serde_json::Value = serde_json::from_str(&pool).expect("json");
        assert_eq!(
            read["pool"][0],
            serde_json::json!({ "id": 3, "name": "Bass" })
        );
        assert_eq!(read["pool"][1]["name"], "level");
        assert_eq!(
            read["pool"][2]["name"].as_str().map(str::len),
            Some(NAME_MAX)
        );
        assert_eq!(pool.matches('\n').count(), 1, "one line");

        assert_eq!(
            Pointer::Scroll([0.25, 1.], [0., -1.]).line(),
            "{\"pointer\":{\"kind\":\"scroll\",\"x\":0.25,\"y\":1.0,\"dx\":0.0,\"dy\":-1.0}}\n"
        );
        assert_eq!(rounded(f64::NAN, 4), 0.);
    }

    #[test]
    fn a_dump_names_the_renderers_its_targets_use() {
        let dump = serde_json::json!({
            "items": [
                { "id": "a", "content": { "kind": "target", "owner": "plugin:bars" } },
                { "id": "g", "content": { "kind": "group", "items": [
                    { "id": "b", "content": { "kind": "target", "owner": "plugin:waves" } },
                ] } },
                { "id": "p", "panel_name": "plugin-panel", "owner": "plugin:tones" },
                { "id": "c", "content": { "kind": "target", "owner": "" } },
            ],
        });

        let mut named = BTreeSet::new();
        named_renderers(&dump, &mut named);
        assert_eq!(
            named.into_iter().collect::<Vec<_>>(),
            ["bars", "waves"],
            "a plugin panel's owner is a plugin"
        );
    }

    #[test]
    fn only_a_plugin_owner_names_a_renderer() {
        assert_eq!(renderer_id("plugin:bars"), Some("bars"));
        assert_eq!(renderer_id("plugin:"), None);
        assert_eq!(renderer_id("bars"), None);
        assert_eq!(renderer_id(""), None);
    }
}
