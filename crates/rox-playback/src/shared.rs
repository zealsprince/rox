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
    /// The pool index this entry points at, distinct per entry even when two
    /// entries share a path. The UI matches the audible track on this rather
    /// than the path, so a file that sits in the order more than once resolves
    /// to the right occurrence instead of the first one by path.
    pub idx: usize,
    /// The album group this entry belongs to (ADR 17), supplied by the player
    /// at insert time; the engine only ever compares ids. Adjacent entries
    /// sharing a group are tracks that belong together: album shuffle keeps
    /// them as a unit, and the crossfade boundary rule (ADR 19) leaves their
    /// gapless splice untouched. None means ungrouped.
    pub group: Option<u64>,
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
    /// The flush epoch, bumped by the decode thread on a seek or a skip.
    /// A backend that sees a number it hasn't handled discards the whole
    /// ring exactly once and echoes it back in `flush_ack`. An epoch rather
    /// than a flag because a flag has to be cleared, and the clearing races
    /// the callback that's already inside it: whoever loses eats the first
    /// milliseconds of the new track. Handled-once is the same discard with
    /// no window to lose.
    pub flush_seq: AtomicU64,
    /// The newest epoch a backend has finished discarding. The decode
    /// thread waits for this to catch up before it resyncs its clock, which
    /// is what used to be a fixed grace sleep.
    pub flush_ack: AtomicU64,
    /// The output-clock frame where the crossfade in flight becomes
    /// audible, with its length in frames beside it; zero length means no
    /// fade. Published by the decode thread when a window opens, read by
    /// the transport so a skip can show the overlap while the ear is in it.
    /// Derived off the output clock like every other position here, so what
    /// shows is what's playing rather than what's been decoded.
    pub fade_at: AtomicU64,
    pub fade_len: AtomicU64,
    /// The fade came from a Previous rather than a Next or a track
    /// boundary, so the transport can sweep the way the skip went.
    pub fade_back: AtomicBool,
    /// Linear volume as f32 bits.
    pub volume_bits: AtomicU32,
    /// Frames the callback actually sent to the device (excludes flushed
    /// frames and silence). This is the global output clock.
    pub frames_consumed: AtomicU64,
    /// True once the queue is exhausted and the ring has drained.
    pub ended: AtomicBool,
    /// Set by the output stream's error callback when the device drops out
    /// (unplugged, format change, backend fault). The RT callback stops
    /// running, so the ring fills and the engine parks; the app polls this to
    /// tear the dead stream down and reopen instead of showing a frozen
    /// "playing". Only ever set on the audio backend's error thread and
    /// cleared by the app on reopen.
    pub device_lost: AtomicBool,
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
            flush_seq: AtomicU64::new(0),
            flush_ack: AtomicU64::new(0),
            fade_at: AtomicU64::new(0),
            fade_len: AtomicU64::new(0),
            fade_back: AtomicBool::new(false),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
            frames_consumed: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            device_lost: AtomicBool::new(false),
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
        self.queue_rev.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Whether the output stream reported a fatal error and stopped. The app
    /// polls this to reopen the device instead of parking on a dead stream.
    pub fn device_lost(&self) -> bool {
        self.device_lost.load(std::sync::atomic::Ordering::Acquire)
    }

    /// How far into the crossfade the ear is, 0 to 1, and whether the skip
    /// that started it went backwards. None when no fade is running, and
    /// also while one is decoded but not yet reached: a window that opens a
    /// ring ahead of the speakers isn't something to show yet.
    pub fn crossfade(&self) -> Option<(f32, bool)> {
        use std::sync::atomic::Ordering::{Acquire, Relaxed};
        // Length first and with acquire ordering: it's the field that
        // publishes the pair, so a nonzero read here means the frame it
        // starts at is already in place.
        //
        // The pair isn't atomic together, so a re-publish landing between
        // these two loads can hand back the old length beside the new
        // frame. One UI tick of a wrong progress number, on a bar that
        // redraws at frame rate, and packing both into one word would make
        // every reader here do shift arithmetic to save it. Tolerated.
        let len = self.fade_len.load(Acquire);
        if len == 0 {
            return None;
        }
        let at = self.fade_at.load(Relaxed);
        let consumed = self.frames_consumed.load(Relaxed);
        let done = consumed.checked_sub(at)?;
        if done >= len {
            return None;
        }
        Some((done as f32 / len as f32, self.fade_back.load(Relaxed)))
    }

    /// Resolve the current position from the output clock: which track, and
    /// how many seconds in. `device_rate` converts frames to seconds.
    pub fn position(&self, device_rate: u32) -> Option<(usize, f64)> {
        let consumed = self
            .frames_consumed
            .load(std::sync::atomic::Ordering::Relaxed);
        self.position_at(consumed, device_rate)
    }

    /// [`position`](Self::position) against a clock reading already taken,
    /// for a caller that has to line something else up with the same frame
    /// and can't have the two loads drift a callback apart.
    pub fn position_at(&self, consumed: u64, device_rate: u32) -> Option<(usize, f64)> {
        let segments = self.segments.lock().unwrap();
        let seg = segments.iter().rev().find(|s| s.at_frame <= consumed)?;
        let frame = seg.track_frame + (consumed - seg.at_frame);
        Some((seg.track, frame as f64 / device_rate as f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn push_segment(shared: &Shared, at_frame: u64, track: usize, track_frame: u64) {
        shared.segments.lock().unwrap().push(Segment {
            at_frame,
            track,
            track_frame,
        });
    }

    #[test]
    fn position_none_before_any_segment() {
        let shared = Shared::new(1);
        assert!(shared.position(48000).is_none());
    }

    #[test]
    fn position_resolves_track_and_seconds() {
        let shared = Shared::new(2);
        // Track 0 starts at output frame 0, at track offset 0.
        push_segment(&shared, 0, 0, 0);
        shared.frames_consumed.store(48000, Ordering::Relaxed);
        // One second in at 48 kHz.
        assert_eq!(shared.position(48000), Some((0, 1.0)));
    }

    #[test]
    fn position_takes_newest_reached_segment() {
        let shared = Shared::new(3);
        push_segment(&shared, 0, 0, 0);
        // Track 1 begins at output frame 96000, mid-track (its own frame 24000).
        push_segment(&shared, 96000, 1, 24000);
        // A future segment for track 2 that hasn't been reached yet.
        push_segment(&shared, 200000, 2, 0);
        shared
            .frames_consumed
            .store(96000 + 48000, Ordering::Relaxed);
        // On track 1: track_frame 24000 + 48000 played = 72000 frames = 1.5s.
        assert_eq!(shared.position(48000), Some((1, 1.5)));
    }

    #[test]
    fn crossfade_reads_off_the_output_clock() {
        let shared = Shared::new(1);
        assert!(
            shared.crossfade().is_none(),
            "nothing published, nothing to show"
        );
        // A two-second window at 48 kHz opening one second out.
        shared.fade_at.store(48_000, Ordering::Relaxed);
        shared.fade_back.store(true, Ordering::Relaxed);
        shared.fade_len.store(96_000, Ordering::Release);
        // Decoded but not reached: the speakers are still short of it.
        assert!(shared.crossfade().is_none());
        shared
            .frames_consumed
            .store(48_000 + 24_000, Ordering::Relaxed);
        let (progress, back) = shared.crossfade().expect("in the window");
        assert!((progress - 0.25).abs() < 1e-6);
        assert!(back, "the skip that started it went backwards");
        // Past the end it stops showing on its own, without the decode
        // thread having to come back and clear anything.
        shared
            .frames_consumed
            .store(48_000 + 96_000, Ordering::Relaxed);
        assert!(shared.crossfade().is_none());
    }

    #[test]
    fn device_lost_flag_round_trips() {
        let shared = Shared::new(1);
        assert!(!shared.device_lost());
        shared.device_lost.store(true, Ordering::Release);
        assert!(shared.device_lost());
    }
}
