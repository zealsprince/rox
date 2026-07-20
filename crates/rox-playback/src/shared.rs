//! State shared between the decode thread, the RT output callback, and the
//! status display. The callback only ever touches the atomics; the mutex side
//! is decode-thread and UI-thread only.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Mutex;

/// A run of contiguous output starting at `at_frame` on the global output
/// clock. Maps the callback's consumed-frames counter back to a position in a
/// track. The decode thread appends one on every track start and seek.
pub struct Segment {
    /// Global output clock (frames actually played) where this segment begins.
    pub at_frame: u64,
    /// Queue index of the track playing in this segment.
    pub track: usize,
    /// Position within the track at segment start, in device-rate frames.
    pub track_frame: u64,
}

/// One entry in the play queue as the UI sees it: a stable id that survives
/// reorders and removals, the file it points at, and whether it was queued
/// explicitly (Play Next, Add to Queue) or came from the playing context (the
/// album or library view). The queue widgets show only the explicit ones; the
/// context plays on in the background. The id is the handle the UI passes back
/// to remove or move an entry, so an index shift between a read and the edit
/// can't act on the wrong track.
#[derive(Clone)]
pub struct QueueEntry {
    pub id: u64,
    pub path: PathBuf,
    pub explicit: bool,
}

/// The play queue published for the UI: the whole timeline in play order and
/// the cursor, the position of the track the decode thread is on. History is
/// `entries[..cursor]`, upcoming is `entries[cursor + 1..]`. The decode thread
/// rewrites this on every track change and every queue edit.
#[derive(Clone, Default)]
pub struct QueueSnapshot {
    pub entries: Vec<QueueEntry>,
    pub cursor: usize,
}

/// Per-track display info the decode thread fills in when it opens a file.
#[derive(Clone)]
pub struct TrackInfo {
    pub name: String,
    pub duration_secs: Option<f64>,
    /// Playable frames per the container, excluding encoder delay/padding.
    pub num_frames: Option<u64>,
    pub sample_rate: u32,
    pub channels: usize,
}

pub struct Shared {
    /// False = paused. The callback outputs silence and stops consuming, so
    /// the position freezes sample-accurately.
    pub playing: AtomicBool,
    /// Seek/skip in progress: the callback discards everything in the ring
    /// and outputs silence until the decode thread clears it.
    pub flush: AtomicBool,
    /// Linear volume as f32 bits.
    pub volume_bits: AtomicU32,
    /// Frames the callback actually sent to the device (excludes flushed
    /// frames and silence). This is the global output clock.
    pub frames_consumed: AtomicU64,
    /// True once the queue is exhausted and the ring has drained.
    pub ended: AtomicBool,
    /// Position mapping, appended by the decode thread.
    pub segments: Mutex<Vec<Segment>>,
    /// Display info per queue entry, filled in as tracks open.
    pub tracks: Mutex<Vec<Option<TrackInfo>>>,
    /// The play queue for the UI, rewritten by the decode thread when its
    /// entries change: a new session, an insert, a remove, a move, a
    /// reshuffle. Not on a plain track advance; the playing entry is resolved
    /// off the position clock, so the queue view only needs republishing when
    /// its contents change.
    pub queue: Mutex<QueueSnapshot>,
    /// Bumped on every queue rewrite, so the UI can skip cloning the snapshot
    /// on the ticks where nothing changed.
    pub queue_rev: AtomicU64,
}

impl Shared {
    pub fn new(queue_len: usize) -> Self {
        Shared {
            playing: AtomicBool::new(true),
            flush: AtomicBool::new(false),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
            frames_consumed: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            segments: Mutex::new(Vec::new()),
            tracks: Mutex::new(vec![None; queue_len]),
            queue: Mutex::new(QueueSnapshot::default()),
            queue_rev: AtomicU64::new(0),
        }
    }

    /// The current play queue, cloned for the UI.
    pub fn queue_snapshot(&self) -> QueueSnapshot {
        self.queue.lock().unwrap().clone()
    }

    /// The queue's revision, bumped on every rewrite. Cheap to poll each tick.
    pub fn queue_rev(&self) -> u64 {
        self.queue_rev
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Resolve the current position from the output clock: which track, and
    /// how many seconds in. `device_rate` converts frames to seconds.
    pub fn position(&self, device_rate: u32) -> Option<(usize, f64)> {
        let consumed = self
            .frames_consumed
            .load(std::sync::atomic::Ordering::Relaxed);
        let segments = self.segments.lock().unwrap();
        let seg = segments.iter().rev().find(|s| s.at_frame <= consumed)?;
        let frame = seg.track_frame + (consumed - seg.at_frame);
        Some((seg.track, frame as f64 / device_rate as f64))
    }
}
