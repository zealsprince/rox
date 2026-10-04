//! One plugin's process and the calls into it. A [`Host`] starts the plugin
//! on first use, speaks `hello` before anything else, and keeps as many
//! requests in flight as its callers send: one writer thread, one reader
//! thread, and a map from request id to the caller waiting on it.
//!
//! A plugin that exits fails every request it had and is started again by
//! the next call, after a backoff. Five exits inside ten minutes stop it for
//! good, with a reason the Plugins page shows. Nothing here ever blocks rox
//! for longer than a call's timeout, and nothing here runs on the UI thread.

use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::manifest::{self, Manifest};
use crate::process::{self, Line, Pipes, Process};
use crate::wire::{self, Inbound, Request};

/// Waits before each restart after a crash; the fifth crash stops the plugin.
pub const BACKOFF: [Duration; 4] = [
    Duration::from_secs(0),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

const CRASH_WINDOW: Duration = Duration::from_secs(10 * 60);

pub const STOPPED_AFTER_CRASHES: &str = "Stopped after repeated crashes";

/// The contract's, as the prototype's measurements left them.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    /// Counted from the spawn, so it covers the interpreter starting and the
    /// plugin's imports. Those run long on slow machines.
    pub hello: Duration,
    pub listing: Duration,
    /// A sync's first page, where a plugin may list the whole collection to
    /// know whether it changed.
    pub sync_first: Duration,
    pub open: Duration,
    pub read: Duration,
    pub cover: Duration,
    pub shutdown: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            hello: Duration::from_secs(30),
            listing: Duration::from_secs(15),
            sync_first: Duration::from_secs(60),
            open: Duration::from_secs(20),
            read: Duration::from_secs(10),
            cover: Duration::from_secs(10),
            shutdown: Duration::from_secs(2),
        }
    }
}

pub struct HostConfig {
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// The record's config, handed over in `hello`.
    pub config: Value,
    /// The plugin's own writable folder, outside its plugin folder so nothing
    /// it writes changes its hash. Created on the first `hello`.
    pub data_dir: PathBuf,
    /// The interface language as a BCP 47 tag, handed over in `hello`. Empty
    /// leaves it out.
    pub locale: String,
    pub timeouts: Timeouts,
    /// [`BACKOFF`] outside tests.
    pub backoff: [Duration; 4],
}

impl HostConfig {
    pub fn new(dir: PathBuf, manifest: Manifest, config: Value, data_dir: PathBuf) -> HostConfig {
        HostConfig {
            dir,
            manifest,
            config,
            data_dir,
            locale: String::new(),
            timeouts: Timeouts::default(),
            backoff: BACKOFF,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not running; the next call starts it.
    Idle,
    Running,
    Stopped(String),
}

/// Cheap to clone; every clone drives the same process.
#[derive(Clone)]
pub struct Host(Arc<Shared>);

struct Shared {
    config: HostConfig,
    life: Mutex<Life>,
    /// Woken when a start finishes, for callers that found one under way.
    started: Condvar,
    next_id: AtomicU64,
}

struct Life {
    phase: Phase,
    crashes: VecDeque<Instant>,
}

enum Phase {
    Down,
    Starting,
    Up(Arc<Conn>),
    Stopped(String),
}

/// One run of the plugin process.
pub(crate) struct Conn {
    writer: Sender<Vec<u8>>,
    pending: Mutex<HashMap<u64, Sender<Result<Value, String>>>>,
    process: Mutex<Process>,
    dead: AtomicBool,
}

/// A request on its way. Waiting consumes it; dropping it abandons the
/// answer, which the reader then throws away.
pub struct Pending {
    rx: Receiver<Result<Value, String>>,
    id: u64,
    method: &'static str,
    sent: Instant,
    timeout: Duration,
    conn: Weak<Conn>,
}

impl Pending {
    pub fn wait(self) -> Result<Value, String> {
        // An answer that landed while nobody was waiting counts, however long
        // ago it was sent.
        if let Ok(result) = self.rx.try_recv() {
            return result;
        }

        let left = self.timeout.saturating_sub(self.sent.elapsed());

        match self.rx.recv_timeout(left) {
            Ok(result) => result,

            Err(RecvTimeoutError::Timeout) => {
                if let Some(conn) = self.conn.upgrade() {
                    conn.forget(self.id);
                }

                Err(format!(
                    "{} got no answer within {}s",
                    self.method,
                    self.timeout.as_secs_f32()
                ))
            }

            // The reader drops every sender when the plugin exits.
            Err(RecvTimeoutError::Disconnected) => Err("the plugin exited".into()),
        }
    }

    /// When it was sent, for timing a round trip.
    pub fn sent(&self) -> Instant {
        self.sent
    }
}

impl Conn {
    fn forget(&self, id: u64) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
    }

    fn fail_all(&self, reason: &str) {
        let waiting: Vec<_> = match self.pending.lock() {
            Ok(mut pending) => pending.drain().collect(),
            Err(_) => return,
        };

        for (_, tx) in waiting {
            let _ = tx.send(Err(reason.to_string()));
        }
    }

    pub(crate) fn alive(&self) -> bool {
        !self.dead.load(Ordering::Acquire)
    }
}

impl Host {
    pub fn new(config: HostConfig) -> Host {
        Host(Arc::new(Shared {
            config,
            life: Mutex::new(Life {
                phase: Phase::Down,
                crashes: VecDeque::new(),
            }),
            started: Condvar::new(),
            next_id: AtomicU64::new(1),
        }))
    }

    pub fn id(&self) -> &str {
        &self.0.config.manifest.id
    }

    pub fn manifest(&self) -> &Manifest {
        &self.0.config.manifest
    }

    pub fn timeouts(&self) -> Timeouts {
        self.0.config.timeouts
    }

    pub fn status(&self) -> Status {
        let life = self.0.life.lock().unwrap();

        match &life.phase {
            Phase::Down => Status::Idle,
            Phase::Starting => Status::Running,
            Phase::Up(conn) if conn.alive() => Status::Running,
            Phase::Up(_) => Status::Idle,
            Phase::Stopped(reason) => Status::Stopped(reason.clone()),
        }
    }

    /// The pid while it runs, for a test or a person killing it by hand.
    pub fn pid(&self) -> Option<u32> {
        let conn = self.current()?;
        let process = conn.process.lock().ok()?;

        Some(process.pid())
    }

    /// The running process, without starting one.
    pub(crate) fn current(&self) -> Option<Arc<Conn>> {
        let life = self.0.life.lock().unwrap();

        match &life.phase {
            Phase::Up(conn) if conn.alive() => Some(Arc::clone(conn)),
            _ => None,
        }
    }

    /// Starts the plugin if it isn't running. Blocks for the backoff and the
    /// `hello` round trip.
    pub fn ensure(&self) -> Result<(), String> {
        self.conn().map(|_| ())
    }

    pub(crate) fn conn(&self) -> Result<Arc<Conn>, String> {
        let mut life = self.0.life.lock().unwrap();

        loop {
            match &life.phase {
                Phase::Up(conn) if conn.alive() => return Ok(Arc::clone(conn)),

                // Its reader hasn't filed the exit yet; do it here.
                Phase::Up(_) => life.phase = Phase::Down,

                Phase::Starting => life = self.0.started.wait(life).unwrap(),

                Phase::Stopped(reason) => return Err(reason.clone()),

                Phase::Down => break,
            }
        }

        let now = Instant::now();
        life.crashes
            .retain(|at| now.duration_since(*at) < CRASH_WINDOW);
        let backoff = self.0.config.backoff;
        let wait = match life.crashes.len() {
            0 => Duration::ZERO,
            n => backoff[(n - 1).min(backoff.len() - 1)],
        };

        life.phase = Phase::Starting;
        drop(life);

        if !wait.is_zero() {
            log::info!("plugin {}: restarting in {wait:?}", self.id());
            std::thread::sleep(wait);
        }

        let started = self.start();

        let mut life = self.0.life.lock().unwrap();
        let result = match (started, &life.phase) {
            // Stopped while it was starting: the new process goes too.
            (Ok(conn), Phase::Stopped(reason)) => {
                let reason = reason.clone();
                drop(life);
                self.hang_up(&conn);

                return Err(reason);
            }

            (Ok(conn), _) => {
                life.phase = Phase::Up(Arc::clone(&conn));
                Ok(conn)
            }

            (Err(e), Phase::Stopped(reason)) => Err(format!("{reason} ({e})")),

            (Err(e), _) => {
                // A plugin that can't start counts the same as one that crashes,
                // so a broken one stops rather than being retried forever.
                Self::crashed(&mut life, self.id());
                Err(e)
            }
        };

        drop(life);
        self.0.started.notify_all();

        result
    }

    fn crashed(life: &mut Life, id: &str) {
        let now = Instant::now();
        life.crashes.push_back(now);
        life.crashes
            .retain(|at| now.duration_since(*at) < CRASH_WINDOW);

        life.phase = match life.crashes.len() > BACKOFF.len() {
            true => {
                log::warn!(
                    "plugin {id}: {} crashes in ten minutes, stopping it",
                    life.crashes.len()
                );
                Phase::Stopped(STOPPED_AFTER_CRASHES.into())
            }
            false => Phase::Down,
        };
    }

    /// Spawn, wire up, `hello`. Nothing else goes out before `hello` answers:
    /// every other caller is waiting in [`Host::conn`] until this returns.
    fn start(&self) -> Result<Arc<Conn>, String> {
        let config = &self.0.config;
        let id = self.id().to_string();

        let command = manifest::entry_for(&config.manifest, &config.dir)?;
        let began = Instant::now();
        let (process, Pipes { stdin, stdout }) = process::spawn(&id, command, &config.dir)?;
        let spawned = began.elapsed();

        let (writer, lines) = mpsc::channel::<Vec<u8>>();
        let conn = Arc::new(Conn {
            writer,
            pending: Mutex::new(HashMap::new()),
            process: Mutex::new(process),
            dead: AtomicBool::new(false),
        });

        let tag = id.clone();
        std::thread::Builder::new()
            .name(format!("plugin-{id}-in"))
            .spawn(move || write_lines(&tag, stdin, lines))
            .map_err(|e| format!("could not start the writer: {e}"))?;

        let host = Arc::downgrade(&self.0);
        let reading = Arc::clone(&conn);
        let tag = id.clone();
        std::thread::Builder::new()
            .name(format!("plugin-{id}-out"))
            .spawn(move || read_answers(&tag, host, reading, stdout))
            .map_err(|e| format!("could not start the reader: {e}"))?;

        let mut params = json!({
            "api": config.manifest.api,
            "config": config.config,
            "data_dir": config.data_dir.to_string_lossy(),
            "platform": manifest::platform(),
            "features": wire::FEATURES,
        });
        if !config.locale.is_empty() {
            params["locale"] = json!(config.locale);
        }
        let asked = Instant::now();
        let answer = std::fs::create_dir_all(&config.data_dir)
            .map_err(|e| format!("could not create {}: {e}", config.data_dir.display()))
            .and_then(|_| self.send_on(&conn, "hello", params, config.timeouts.hello))
            .and_then(Pending::wait);

        let hello: wire::Hello = match answer.and_then(wire::decode) {
            Ok(hello) => hello,
            Err(e) => {
                self.hang_up(&conn);
                return Err(format!("hello: {e}"));
            }
        };

        if !manifest::SUPPORTED_API.contains(&hello.api) {
            self.hang_up(&conn);
            return Err(format!("hello: the plugin speaks api {}", hello.api));
        }

        log::info!(
            "plugin {id}: {} {} up, spawned in {:.1} ms, hello answered in {:.1} ms",
            hello.name,
            hello.version,
            spawned.as_secs_f64() * 1000.0,
            asked.elapsed().as_secs_f64() * 1000.0
        );

        Ok(conn)
    }

    /// Sends and returns at once; the answer arrives on the [`Pending`].
    pub(crate) fn send_on(
        &self,
        conn: &Arc<Conn>,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Pending, String> {
        // Nothing answers a dead process; failing now beats waiting out the timeout.
        if !conn.alive() {
            return Err("the plugin exited".into());
        }

        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();

        conn.pending
            .lock()
            .map_err(|_| "the plugin's request table is poisoned")?
            .insert(id, tx);

        if conn.writer.send(Request::line(id, method, params)).is_err() {
            conn.forget(id);
            return Err("the plugin exited".into());
        }

        Ok(Pending {
            rx,
            id,
            method,
            sent: Instant::now(),
            timeout,
            conn: Arc::downgrade(conn),
        })
    }

    /// Starts the plugin if it has to, then sends.
    pub fn send(
        &self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Pending, String> {
        let conn = self.conn()?;
        self.send_on(&conn, method, params, timeout)
    }

    pub fn call(
        &self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.send(method, params, timeout)?.wait()
    }

    /// Asks the plugin to exit, waits out the grace, then kills it and
    /// everything it started. Requests still in flight keep their chance to
    /// answer inside the grace and fail after it. The host stays stopped.
    pub fn stop(&self, reason: &str) {
        let conn = {
            let mut life = self.0.life.lock().unwrap();
            let phase = std::mem::replace(&mut life.phase, Phase::Stopped(reason.to_string()));

            match phase {
                Phase::Up(conn) => Some(conn),
                _ => None,
            }
        };
        self.0.started.notify_all();

        if let Some(conn) = conn {
            let grace = self.timeouts().shutdown;
            let asked = self
                .send_on(&conn, "shutdown", Value::Null, grace)
                .and_then(Pending::wait);
            if let Err(e) = asked {
                log::info!("plugin {}: shutdown: {e}", self.id());
            }

            self.hang_up(&conn);
        }
    }

    /// [`stop`](Self::stop) without the grace, for rox quitting: the app
    /// won't wait two seconds per plugin. Closing stdin is the signal the
    /// contract gives every plugin, and the group goes with it.
    pub fn hang_up_now(&self, reason: &str) {
        let conn = {
            let mut life = self.0.life.lock().unwrap();
            match std::mem::replace(&mut life.phase, Phase::Stopped(reason.to_string())) {
                Phase::Up(conn) => Some(conn),
                _ => None,
            }
        };
        self.0.started.notify_all();

        if let Some(conn) = conn {
            self.hang_up_within(&conn, Duration::ZERO);
        }
    }

    /// Lets a stopped host start again on its next call, the crash count
    /// cleared. What switching a plugin back on does.
    pub fn revive(&self) {
        let mut life = self.0.life.lock().unwrap();
        if matches!(life.phase, Phase::Stopped(_)) {
            life.phase = Phase::Down;
            life.crashes.clear();
        }
    }

    /// Waits up to `grace` for the process to leave on its own (it does after
    /// answering `shutdown`), then kills it and its group.
    fn hang_up_within(&self, conn: &Arc<Conn>, grace: Duration) {
        conn.dead.store(true, Ordering::Release);
        conn.fail_all("the plugin stopped");

        if let Ok(mut process) = conn.process.lock() {
            if !grace.is_zero() {
                process.wait_for(grace);
            }
            process.kill();
        }
    }

    fn hang_up(&self, conn: &Arc<Conn>) {
        self.hang_up_within(conn, Duration::from_millis(200));
    }
}

fn write_lines(id: &str, mut stdin: std::process::ChildStdin, lines: Receiver<Vec<u8>>) {
    for line in lines {
        let written = stdin.write_all(&line).and_then(|_| stdin.flush());
        if let Err(e) = written {
            log::debug!("plugin {id}: stdin closed: {e}");
            return;
        }
    }
}

fn read_answers(id: &str, host: Weak<Shared>, conn: Arc<Conn>, stdout: std::process::ChildStdout) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();

    loop {
        match process::read_line(&mut reader, &mut line, wire::MAX_FRAME) {
            Ok(Line::Whole) if line.is_empty() => {}

            Ok(Line::Whole) => match wire::parse_line(&line) {
                Inbound::Answer { id: rid, result } => {
                    let waiting = conn.pending.lock().ok().and_then(|mut p| p.remove(&rid));
                    match waiting {
                        Some(tx) => {
                            let _ = tx.send(result);
                        }
                        None => log::debug!("plugin {id}: late or unknown answer to {rid}"),
                    }
                }

                Inbound::Junk(why) => log::warn!("plugin {id}: {why}"),
            },

            Ok(Line::Clipped) => {
                log::warn!(
                    "plugin {id}: dropped a frame over {} bytes",
                    wire::MAX_FRAME
                )
            }

            Ok(Line::End) | Err(_) => break,
        }
    }

    // Stdout closed: the plugin exited or was killed. The crash is filed
    // before the connection reads as dead, so no caller can see a dead one
    // still up and start over it without counting the crash.
    let expected = conn.dead.load(Ordering::Acquire);
    if !expected && let Some(host) = host.upgrade() {
        let mut life = host.life.lock().unwrap();
        if matches!(&life.phase, Phase::Up(up) if Arc::ptr_eq(up, &conn)) {
            log::warn!("plugin {id}: exited unexpectedly");
            Host::crashed(&mut life, id);
        }
    }

    conn.dead.store(true, Ordering::Release);
    conn.fail_all("the plugin exited");

    if let Ok(mut process) = conn.process.lock() {
        if process.wait_for(Duration::from_millis(500)) {
            log::debug!("plugin {id}: exit status {:?}", process.exited());
        }

        // Whatever it started goes with it.
        process.kill();
    }
}
