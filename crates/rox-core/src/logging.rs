//! The app's logging backend behind the `log` facade: every `log::warn!`,
//! `error!`, or `info!` in rox and rox-playback lands here and fans three
//! ways - stderr, so a debug run still prints as it always did; a rolling
//! file under the data dir, so a crash or a weird session leaves a record a
//! bug report can attach; and an in-memory ring the console window reads,
//! so the same lines show live in the app without tailing a file.
//!
//! One backend, installed once at startup. The ring is capped and the file
//! rolls at a size ceiling, so neither grows without bound. Writes take a
//! mutex around the file and the ring; the log calls sit off the audio
//! path (decode-thread and UI-thread errors, never the sample callback),
//! so the lock is never on a realtime deadline.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::settings::data_dir;

/// How many lines the in-memory ring holds for the console window. Past
/// this the oldest fall off; the file on disk is the full record.
const RING_CAP: usize = 4000;

/// The active log file's size ceiling. Past it the file rolls to `.1` and a
/// fresh one starts, so the log never grows without bound. One back file is
/// kept - enough that a crash and the report written just after both fall
/// inside the same two files.
const FILE_CAP: u64 = 2 * 1024 * 1024;

/// The backend, reached through the `log` facade. A process-wide singleton:
/// [`init`] installs it once and the ring and file live for the run.
static LOGGER: OnceLock<Logger> = OnceLock::new();

/// One captured line: when it landed on the wall clock (so a reported log
/// reads in real time), how loud, and the message. The area is carried in
/// the message text itself ("history: ...", "settings: ..."), so there's no
/// separate target column to keep in sync.
#[derive(Clone)]
pub struct Line {
    pub time: String,
    pub level: Level,
    pub message: String,
}

/// The mutable half behind one lock: the ring and the open file with its
/// running size, so a write appends and rolls under the same guard.
struct Sink {
    ring: VecDeque<Line>,
    file: Option<File>,
    bytes: u64,
    path: PathBuf,
}

struct Logger {
    sink: Mutex<Sink>,
    /// Bumps on every line and on a clear, so the console window's poll can
    /// tell "nothing new" from "repaint" without diffing the ring.
    seq: AtomicU64,
}

impl Logger {
    fn new() -> Logger {
        let path = data_dir().join("logs").join("rox.log");
        let (file, bytes) = open_file(&path);
        Logger {
            sink: Mutex::new(Sink {
                ring: VecDeque::with_capacity(RING_CAP),
                file,
                bytes,
                path,
            }),
            seq: AtomicU64::new(0),
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if metadata.level() > Level::Info {
            return false;
        }
        // blade-graphics narrates every buffer and texture create/destroy at
        // info, and the shader region scratch texture recreates on each
        // resize. That's debug-grade noise, so it drops with the rest of
        // debug; its warnings and errors still pass.
        !(metadata.level() == Level::Info
            && metadata
                .target()
                .starts_with("blade_graphics::vulkan::resource"))
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = Line {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            level: record.level(),
            message: record.args().to_string(),
        };
        // Stderr first, so a debug build's console reads exactly as it did
        // before the backend existed.
        eprintln!("{} {:>5} {}", line.time, line.level, line.message);

        let mut sink = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        let formatted = format!("{} {:>5} {}\n", line.time, line.level, line.message);
        let mut wrote = 0u64;
        if let Some(file) = sink.file.as_mut() {
            if file.write_all(formatted.as_bytes()).is_ok() {
                wrote = formatted.len() as u64;
            }
        }
        sink.bytes += wrote;
        if sink.bytes >= FILE_CAP {
            roll(&mut sink);
        }
        if sink.ring.len() == RING_CAP {
            sink.ring.pop_front();
        }
        sink.ring.push_back(line);
        self.seq.fetch_add(1, Ordering::Relaxed);
    }

    fn flush(&self) {
        let mut sink = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = sink.file.as_mut() {
            let _ = file.flush();
        }
    }
}

/// Install the backend and open the log file. Idempotent and safe to call
/// before the window system is up; a second call is a no-op. Info and above
/// pass; debug and trace are dropped, so a release build stays quiet.
pub fn init() {
    let logger = LOGGER.get_or_init(Logger::new);
    if log::set_logger(logger).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
}

/// The console window's view of the ring, newest last. Cheap enough to
/// clone whole on each refresh at this cap.
pub fn snapshot() -> Vec<Line> {
    LOGGER
        .get()
        .map(|logger| {
            logger
                .sink
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .ring
                .iter()
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The line-and-clear counter the console poll watches; unchanged since the
/// last look means nothing to repaint.
pub fn seq() -> u64 {
    LOGGER
        .get()
        .map(|logger| logger.seq.load(Ordering::Relaxed))
        .unwrap_or(0)
}

/// Empty the console's view. The file on disk is untouched - clear tidies
/// the live pane, it doesn't erase the record a report needs.
pub fn clear() {
    if let Some(logger) = LOGGER.get() {
        logger
            .sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ring
            .clear();
        logger.seq.fetch_add(1, Ordering::Relaxed);
    }
}

/// Where the active log file sits, for the console's Reveal action. Valid
/// before [`init`] too, so a caller can point at it either way.
pub fn log_path() -> PathBuf {
    LOGGER
        .get()
        .map(|logger| {
            logger
                .sink
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .path
                .clone()
        })
        .unwrap_or_else(|| data_dir().join("logs").join("rox.log"))
}

/// Open the log file for append, making its folder first, and read back its
/// current size so the roll accounts from where the last run left off.
fn open_file(path: &Path) -> (Option<File>, u64) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => {
            let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
            (Some(file), bytes)
        }
        Err(_) => (None, 0),
    }
}

/// Roll the log once it passes the cap: drop the current handle to close
/// it, move `rox.log` to `rox.log.1` (replacing any older back file), then
/// reopen a fresh one. A rename that fails leaves the current file in place
/// and the next write tries again, so a locked back file never loses lines.
fn roll(sink: &mut Sink) {
    sink.file = None;
    let back = sink.path.with_extension("log.1");
    let _ = std::fs::remove_file(&back);
    let _ = std::fs::rename(&sink.path, &back);
    let (file, bytes) = open_file(&sink.path);
    sink.file = file;
    sink.bytes = bytes;
}
