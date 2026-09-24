//! The logging backend behind the `log` facade: every line fans to stderr,
//! a rolling file under the data dir, and an in-memory ring the console
//! window reads. Writes take one mutex; logging never happens on the sample
//! callback, so the lock is never on a realtime deadline.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::settings::data_dir;

const RING_CAP: usize = 4000;

/// Past this the file rolls to `.1`. One back file is kept.
const FILE_CAP: u64 = 2 * 1024 * 1024;

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// The area is part of the message text ("history: ..."), so there's no
/// separate target column.
#[derive(Clone)]
pub struct Line {
    pub time: String,
    pub level: Level,
    pub message: String,
}

struct Sink {
    ring: VecDeque<Line>,
    file: Option<File>,
    bytes: u64,
    path: PathBuf,
}

struct Logger {
    sink: Mutex<Sink>,
    /// Bumps on every line and on a clear, so the console's poll can skip
    /// unchanged frames without diffing the ring.
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
        // blade-graphics logs every buffer and texture create/destroy at
        // info. That's debug noise; its warnings and errors still pass.
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
        eprintln!("{} {:>5} {}", line.time, line.level, line.message);

        let mut sink = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        let formatted = format!("{} {:>5} {}\n", line.time, line.level, line.message);
        let mut wrote = 0u64;
        if let Some(file) = sink.file.as_mut()
            && file.write_all(formatted.as_bytes()).is_ok()
        {
            wrote = formatted.len() as u64;
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

/// Idempotent. Info and above pass.
pub fn init() {
    let logger = LOGGER.get_or_init(Logger::new);
    if log::set_logger(logger).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
}

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

pub fn seq() -> u64 {
    LOGGER
        .get()
        .map(|logger| logger.seq.load(Ordering::Relaxed))
        .unwrap_or(0)
}

/// The file on disk is untouched.
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

/// Valid before [`init`] too.
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

/// A rename that fails leaves the current file in place and the next write
/// tries again, so a locked back file never loses lines.
fn roll(sink: &mut Sink) {
    sink.file = None;
    let back = sink.path.with_extension("log.1");
    let _ = std::fs::remove_file(&back);
    let _ = std::fs::rename(&sink.path, &back);
    let (file, bytes) = open_file(&sink.path);
    sink.file = file;
    sink.bytes = bytes;
}
