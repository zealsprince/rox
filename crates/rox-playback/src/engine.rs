//! The decode thread: Symphonia decode, gapless track boundary, seek, and the
//! producer side of the sample ring. Everything here is allowed to allocate,
//! lock, and block; the RT line is the ring in output.rs.
//!
//! Gapless (ADR 3): one long-lived stream, the decoder swaps at EOF and the
//! next track's first frame lands in the ring right behind the last. Encoder
//! delay/padding comes from the LAME/iTunes headers: symphonia 0.6 exposes it
//! as packet trim metadata and the mp3 decoder applies it, so the samples we
//! see are already the playable range. The spike verifies that claim against
//! real files; if it falls short we trim from Track::delay/padding ourselves,
//! which the ADR anticipated.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use rtrb::Producer;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

use crate::resample::Resampler;
use crate::shared::{QueueEntry, QueueSnapshot, Segment, Shared, TrackInfo};

pub enum Cmd {
    TogglePause,
    Seek(f64),
    Next,
    Prev,
    Volume(f32),
    SetLoop(LoopMode),
    SetShuffle(bool),
    /// Splice tracks into the queue right after entry `after` (its stable id),
    /// or at the end when `after` is None. `explicit` marks them as user-queued
    /// (Play Next, Add to Queue) rather than part of the playing context, so
    /// the queue widgets can show them apart from the album or library that
    /// plays on around them.
    Insert {
        after: Option<u64>,
        paths: Vec<PathBuf>,
        explicit: bool,
        /// Jump to the first of the batch and play it now, keeping the rest of
        /// the queue behind it. A drag onto Play now sets this; Play Next and
        /// Add to Queue leave it off so the current track keeps playing.
        and_play: bool,
    },
    /// Drop the entry with this id from the queue. Removing the playing entry
    /// is ignored; the UI never offers it.
    Remove { id: u64 },
    /// Drop a whole set of entries in one pass, with a single queue publish at
    /// the end. Clear Queue and multi-select delete route here so a big queue
    /// empties in one O(n) sweep instead of one O(n) remove per id. The playing
    /// entry is kept even if named.
    RemoveMany { ids: Vec<u64> },
    /// Move the entry with this id to just after entry `after`, or to the
    /// front when `after` is None.
    Move { id: u64, after: Option<u64> },
    /// Jump straight to the entry with this id and play it now.
    Jump { id: u64 },
    Quit,
}

/// What happens when a track or the queue runs out. Lives on the decode
/// thread only; the RT callback never looks at it.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    /// Play the queue through once and stop.
    #[default]
    Off,
    /// Wrap from the last track back to the first; Next and Prev wrap too.
    All,
    /// Repeat the current track at EOF. Skips still move through the queue.
    One,
}

/// One open file: reader, decoder, and the per-track conversion state.
struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    device_rate: u32,
    resampler: Resampler,
    /// Scratch for one decoded packet, interleaved in the file's channel
    /// count, reused across packets.
    scratch: Vec<f32>,
}

/// One slot in the play order: a stable id the UI addresses it by, and the
/// index of the file in the append-only `queue` pool. The pool never shrinks,
/// so this index stays valid for the position mapping no matter how the order
/// is reshuffled or trimmed.
struct OrderEntry {
    id: u64,
    idx: usize,
    /// User-queued (Play Next, Add to Queue) rather than part of the playing
    /// context. The queue widgets list only these.
    explicit: bool,
}

pub struct Engine {
    /// Append-only pool of file paths. Order entries index into it; nothing is
    /// ever removed so `Segment.track` indices stay valid.
    queue: Vec<PathBuf>,
    idx: usize,
    /// The play order. All navigation walks this, so `order[pos]` is the
    /// playing entry and Prev retraces the path. Editable in place: insert,
    /// remove, move, reshuffle.
    order: Vec<OrderEntry>,
    /// Position within `order`; kept in sync with `idx` on every open.
    pos: usize,
    /// Where the first open lands, so playback can start partway into a
    /// seeded context with history sitting behind the cursor.
    start: usize,
    /// Next stable id to hand out to a new order entry.
    next_id: u64,
    shared: Arc<Shared>,
    producer: Producer<f32>,
    device_rate: u32,
    rx: Receiver<Cmd>,
    loop_mode: LoopMode,
    /// Frames pushed on the frames_consumed clock; resynced after each flush.
    pushed_playable: u64,
    /// Decoded, converted samples waiting for ring space.
    pending: Vec<f32>,
    pending_pos: usize,
}

impl Engine {
    pub fn new(
        queue: Vec<PathBuf>,
        start: usize,
        shared: Arc<Shared>,
        producer: Producer<f32>,
        device_rate: u32,
        rx: Receiver<Cmd>,
        explicit: Vec<bool>,
    ) -> Self {
        // The starting queue is the playing context: an album, a library run,
        // whatever the caller handed over. A fresh context passes an empty
        // `explicit`, so every entry is context; a launch restore passes the
        // saved flags so the up-next queue comes back marked. Later Play Next
        // and Add to Queue splice in more explicit entries through Insert.
        let order = (0..queue.len())
            .map(|idx| OrderEntry {
                id: idx as u64,
                idx,
                explicit: explicit.get(idx).copied().unwrap_or(false),
            })
            .collect();
        Engine {
            order,
            pos: 0,
            start: start.min(queue.len().saturating_sub(1)),
            next_id: queue.len() as u64,
            queue,
            idx: 0,
            shared,
            producer,
            device_rate,
            rx,
            loop_mode: LoopMode::default(),
            pushed_playable: 0,
            pending: Vec::new(),
            pending_pos: 0,
        }
    }

    pub fn run(mut self) {
        self.publish_queue();
        let mut source = self.open_at(self.start);

        loop {
            // Commands first so pause/seek stay responsive even when the
            // ring is full and decode is idle.
            let mut flush_to: Option<FlushAction> = None;
            // Running navigation target across this drain, so back-to-back
            // Next/Prev each step from the last intended position instead of
            // all recomputing off the stale self.pos. Two Next presses in one
            // drain otherwise collapse into a single advance.
            let mut nav_pos: Option<usize> = None;
            while let Ok(cmd) = self.rx.try_recv() {
                match cmd {
                    Cmd::TogglePause => {
                        let now = self.shared.playing.load(Ordering::Relaxed);
                        self.shared.playing.store(!now, Ordering::Relaxed);
                    }
                    Cmd::Volume(v) => {
                        let v = v.clamp(0.0, 2.0);
                        self.shared
                            .volume_bits
                            .store(v.to_bits(), Ordering::Relaxed);
                    }
                    Cmd::Seek(secs) => {
                        flush_to = Some(FlushAction::Seek(secs.max(0.0)));
                        nav_pos = None;
                    }
                    Cmd::Next => {
                        // Off the audible track, not the decode cursor, which
                        // has run a track ahead for the gapless boundary; from
                        // there Next would skip two near the end of a track.
                        let from = nav_pos.unwrap_or_else(|| self.audible_pos());
                        if from + 1 < self.order.len() {
                            nav_pos = Some(from + 1);
                        } else if self.loop_mode == LoopMode::All && !self.order.is_empty() {
                            nav_pos = Some(0);
                        }
                        flush_to = None;
                    }
                    Cmd::Prev => {
                        let from = nav_pos.unwrap_or_else(|| self.audible_pos());
                        let target = if from == 0 && self.loop_mode == LoopMode::All {
                            self.order.len().saturating_sub(1)
                        } else {
                            from.saturating_sub(1)
                        };
                        nav_pos = Some(target);
                        flush_to = None;
                    }
                    Cmd::SetLoop(mode) => {
                        self.loop_mode = mode;
                        // From the ended state the source is None, so just
                        // storing the mode leaves playback dead. Route through
                        // the nav path: a wrapping mode reopens and resumes,
                        // clearing ended on the way.
                        if source.is_none() {
                            nav_pos = match mode {
                                LoopMode::One => Some(self.pos),
                                LoopMode::All if !self.order.is_empty() => Some(0),
                                _ => None,
                            };
                        }
                    }
                    Cmd::SetShuffle(on) => self.set_shuffle(on),
                    Cmd::Insert {
                        after,
                        paths,
                        explicit,
                        and_play,
                    } => {
                        let at = self.insert(after, paths, explicit);
                        // From the ended state the source is None, so the new
                        // entries land in order but nothing opens them and we
                        // stay silent. Route the first of the batch through the
                        // nav path so it reopens and resumes, clearing ended on
                        // the way. Play now jumps the same way from a live
                        // session; Play Next and Add to Queue leave the current
                        // track playing.
                        if and_play || source.is_none() {
                            nav_pos = at;
                        }
                        // Play now means play: resume if we were paused, so a
                        // drop onto Play now starts audio instead of loading it
                        // silent.
                        if and_play {
                            self.shared.playing.store(true, Ordering::Relaxed);
                        }
                    }
                    Cmd::Remove { id } => self.remove(id),
                    Cmd::RemoveMany { ids } => self.remove_many(&ids),
                    Cmd::Move { id, after } => self.move_entry(id, after),
                    // Reuse the nav path: setting the target flushes and opens
                    // it just like a Next would.
                    Cmd::Jump { id } => {
                        if let Some(p) = self.find(id) {
                            nav_pos = Some(p);
                        }
                        flush_to = None;
                    }
                    Cmd::Quit => return,
                }
            }
            if let Some(p) = nav_pos {
                flush_to = Some(FlushAction::Track(p));
            }

            if let Some(action) = flush_to {
                self.flush_ring();
                match action {
                    FlushAction::Track(p) => {
                        self.shared.ended.store(false, Ordering::Relaxed);
                        source = self.open_at(p);
                    }
                    FlushAction::Seek(secs) => {
                        // The decode cursor leads the audible track by up to a
                        // ring during the gapless preroll, so the open source
                        // is already the next track and seeking it would scrub
                        // inside the following track. Reopen the audible track
                        // first, the same anchor Next/Prev use.
                        let ap = self.audible_pos();
                        if ap != self.pos {
                            if let Some(src) = self.open_at(ap) {
                                source = Some(src);
                            }
                        }
                        if let Some(src) = source.as_mut() {
                            let landed = src.seek(secs);
                            self.register_segment(landed);
                        }
                    }
                }
                continue;
            }

            // Move pending samples into the ring. Ring full means we're
            // comfortably ahead; nap and go back to command handling.
            while self.pending_pos < self.pending.len() {
                match self.producer.push(self.pending[self.pending_pos]) {
                    Ok(()) => self.pending_pos += 1,
                    Err(_) => break,
                }
            }
            if self.pending_pos < self.pending.len() {
                std::thread::sleep(StdDuration::from_millis(3));
                continue;
            }
            self.pushed_playable += (self.pending.len() / 2) as u64;
            self.pending.clear();
            self.pending_pos = 0;

            // Refill from the decoder.
            match source.as_mut() {
                Some(src) => {
                    let device_rate = self.device_rate;
                    if !src.next_chunk(device_rate, &mut self.pending) {
                        // EOF: swap the decoder under the live stream. No
                        // flush, no stream teardown; this IS the gapless
                        // boundary. Loop modes pick the next open: One
                        // reopens the same track, All wraps the queue.
                        source = if self.loop_mode == LoopMode::One {
                            self.open_at(self.pos)
                        } else if self.pos + 1 < self.order.len() {
                            self.open_at(self.pos + 1)
                        } else if self.loop_mode == LoopMode::All && !self.order.is_empty() {
                            self.open_at(0)
                        } else {
                            None
                        };
                    }
                }
                None => {
                    // Queue exhausted: report ended once the ring drains.
                    let cap = self.producer.buffer().capacity();
                    if self.producer.slots() == cap {
                        self.shared.ended.store(true, Ordering::Relaxed);
                    }
                    std::thread::sleep(StdDuration::from_millis(20));
                }
            }
        }
    }

    /// Open the track at play-order position `p`, falling forward through
    /// unreadable files in play order. Registers the position segment for
    /// the new track.
    fn open_at(&mut self, mut p: usize) -> Option<Source> {
        while p < self.order.len() {
            let i = self.order[p].idx;
            match Source::open(&self.queue[i], self.device_rate) {
                Ok((src, info)) => {
                    self.pos = p;
                    self.idx = i;
                    self.shared.tracks.lock().unwrap()[i] = Some(info);
                    let at_frame = self.pushed_playable;
                    self.shared.segments.lock().unwrap().push(Segment {
                        at_frame,
                        track: i,
                        track_frame: 0,
                    });
                    return Some(src);
                }
                Err(e) => {
                    eprintln!("\nskipping {}: {e}", self.queue[i].display());
                    p += 1;
                }
            }
        }
        None
    }

    /// Rewrite the UI's queue view from the live order. Called when the
    /// entries change, not on a plain advance: the UI resolves the playing
    /// entry off the position clock, so the cursor here is only a hint for
    /// before audio starts. Bumps the revision so the UI knows to re-read.
    fn publish_queue(&self) {
        let entries = self
            .order
            .iter()
            .map(|e| QueueEntry {
                id: e.id,
                path: self.queue[e.idx].clone(),
                explicit: e.explicit,
                idx: e.idx,
            })
            .collect();
        *self.shared.queue.lock().unwrap() = QueueSnapshot {
            entries,
            cursor: self.pos,
        };
        self.shared
            .queue_rev
            .fetch_add(1, Ordering::Release);
    }

    /// Order position of the entry with this id, if it is still queued.
    fn find(&self, id: u64) -> Option<usize> {
        self.order.iter().position(|e| e.id == id)
    }

    /// The order position of the track actually coming out of the speakers,
    /// resolved off the output clock like `Shared::position`. Navigation
    /// anchors on this rather than `pos`, the decode cursor, which leads by up
    /// to a ring near a track boundary once the next track has opened for the
    /// gapless handoff. Each entry has a distinct pool index, so the lookup is
    /// unambiguous. Falls back to the decode cursor before any frame plays.
    fn audible_pos(&self) -> usize {
        let consumed = self.shared.frames_consumed.load(Ordering::Relaxed);
        let track = {
            let segments = self.shared.segments.lock().unwrap();
            segments
                .iter()
                .rev()
                .find(|s| s.at_frame <= consumed)
                .map(|s| s.track)
        };
        match track {
            Some(pool_idx) => self
                .order
                .iter()
                .position(|e| e.idx == pool_idx)
                .unwrap_or(self.pos),
            None => self.pos,
        }
    }

    /// Splice paths into the pool and order right after entry `after` (or at
    /// the end). Never flushes: the current track keeps playing, only the
    /// future changes. If the splice lands before the cursor the cursor rides
    /// along so the playing entry stays put. Returns the order position of the
    /// first appended entry, or None when nothing was inserted, so a revive
    /// from the ended state can navigate to it.
    fn insert(&mut self, after: Option<u64>, paths: Vec<PathBuf>, explicit: bool) -> Option<usize> {
        if paths.is_empty() {
            return None;
        }
        let at = match after {
            Some(id) => match self.find(id) {
                Some(p) => p + 1,
                None => self.order.len(),
            },
            None => self.order.len(),
        };
        let mut new = Vec::with_capacity(paths.len());
        for path in paths {
            let idx = self.queue.len();
            self.queue.push(path);
            self.shared.tracks.lock().unwrap().push(None);
            new.push(OrderEntry {
                id: self.next_id,
                idx,
                explicit,
            });
            self.next_id += 1;
        }
        let count = new.len();
        self.order.splice(at..at, new);
        if at <= self.pos {
            self.pos += count;
        }
        self.publish_queue();
        Some(at)
    }

    /// Drop an entry from the order. Removing the audibly playing entry is
    /// refused; skipping is a separate action. The check is on the audible
    /// position, not the decode cursor, which has run ahead to the next entry
    /// near a boundary and would otherwise refuse removing the very item the
    /// queue is about to play.
    fn remove(&mut self, id: u64) {
        let Some(p) = self.find(id) else {
            return;
        };
        if p == self.audible_pos() {
            return;
        }
        self.order.remove(p);
        // Removing at or before the decode cursor shifts it down one. When p
        // equals the cursor it's the pre-decoded next track (p can't be the
        // audible entry, that's refused above), and the still-open source
        // hands off to pos+1 at EOF, so pos must land on the audible entry or
        // that handoff skips a track.
        if p <= self.pos {
            self.pos = self.pos.saturating_sub(1);
        }
        self.publish_queue();
    }

    /// Drop every entry named in `ids` in one sweep, keeping the audible one so
    /// playback never cuts, then re-find the decode cursor by id and publish
    /// once. One pass over the order rather than a find-and-remove per id, so
    /// clearing a huge queue stays O(n) with a single UI wake instead of O(n^2)
    /// with a wake per entry.
    fn remove_many(&mut self, ids: &[u64]) {
        if ids.is_empty() {
            return;
        }
        let drop: std::collections::HashSet<u64> = ids.iter().copied().collect();
        let keep = self.order.get(self.audible_pos()).map(|e| e.id);
        let cursor = self.order.get(self.pos).map(|e| e.id);
        let before = self.order.len();
        self.order
            .retain(|e| !drop.contains(&e.id) || Some(e.id) == keep);
        if self.order.len() == before {
            return;
        }
        // Re-anchor the decode cursor by id. If the cursor entry itself was
        // dropped (the pre-decoded next track), fall back to the audible entry
        // so the still-open source hands off to the right next track at EOF
        // instead of clamping and skipping one. Last resort clamps into range.
        self.pos = cursor
            .and_then(|id| self.find(id))
            .or_else(|| keep.and_then(|id| self.find(id)))
            .unwrap_or_else(|| self.pos.min(self.order.len().saturating_sub(1)));
        self.publish_queue();
    }

    /// Move an entry to just after `after` (or to the front). The cursor is
    /// re-found by id so the playing entry stays current through any shuffle
    /// of indices around it.
    fn move_entry(&mut self, id: u64, after: Option<u64>) {
        let Some(from) = self.find(id) else {
            return;
        };
        let cur_id = self.order[self.pos].id;
        let entry = self.order.remove(from);
        let at = match after {
            Some(a) => match self.find(a) {
                Some(p) => p + 1,
                None => self.order.len(),
            },
            None => 0,
        };
        self.order.insert(at, entry);
        self.pos = self.find(cur_id).unwrap_or(self.pos);
        self.publish_queue();
    }

    /// Reorder only the upcoming portion, `order[pos + 1..]`. History and the
    /// playing entry stay put, so shuffle never scrambles what already played
    /// and the current track keeps playing. Nothing flushes. Off restores
    /// pool order (ascending idx), which is library order for a fresh context;
    /// play-next inserts, being later pool entries, settle at the tail.
    fn set_shuffle(&mut self, on: bool) {
        let start = self.pos + 1;
        if start >= self.order.len() {
            self.publish_queue();
            return;
        }
        let tail = &mut self.order[start..];
        if on {
            shuffle_slice(tail);
        } else {
            tail.sort_by_key(|e| e.idx);
        }
        self.publish_queue();
    }

    /// Have the callback discard everything queued, wait for the ring to
    /// drain, then resync our clock to what actually played.
    fn flush_ring(&mut self) {
        self.pending.clear();
        self.pending_pos = 0;
        self.shared.flush.store(true, Ordering::Release);
        let cap = self.producer.buffer().capacity();
        while self.producer.slots() < cap {
            std::thread::sleep(StdDuration::from_millis(2));
        }
        // One callback period of grace so an in-flight callback that read
        // flush=true finishes before new samples land. A late callback could
        // still eat the first few ms after a seek; acceptable for the spike.
        std::thread::sleep(StdDuration::from_millis(25));
        self.shared.flush.store(false, Ordering::Release);
        self.pushed_playable = self.shared.frames_consumed.load(Ordering::Relaxed);
    }

    fn register_segment(&self, track_secs: f64) {
        self.shared.segments.lock().unwrap().push(Segment {
            at_frame: self.pushed_playable,
            track: self.idx,
            track_frame: (track_secs * self.device_rate as f64).round() as u64,
        });
    }
}

enum FlushAction {
    Seek(f64),
    /// Jump to this play-order position.
    Track(usize),
}

/// Fisher-Yates over a slice in place, xorshift64 off the std hasher's
/// per-process random keys; a play order does not need a rand dependency.
fn shuffle_slice<T>(slice: &mut [T]) {
    use std::hash::{BuildHasher, Hasher};
    let mut state = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
        | 1;
    for i in (1..slice.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        slice.swap(i, j);
    }
}

/// Decode a whole file through the same path playback uses and report
/// (decoded frames, frames the container claims are playable). Equal numbers
/// mean the encoder delay/padding trim is exact, i.e. the gapless boundary
/// is sample-accurate by construction. No audio device involved.
pub fn count_frames(path: &PathBuf) -> Result<(u64, Option<u64>), String> {
    // Probe once for the source rate, then open for real with the device
    // rate equal to it, so the resampler is a passthrough and the count is
    // in source frames.
    let (probe, info) = Source::open(path, 48000)?;
    drop(probe);
    let (mut src, info) = Source::open(path, info.sample_rate)?;

    let mut decoded: u64 = 0;
    let mut chunk = Vec::new();
    loop {
        chunk.clear();
        if !src.next_chunk(info.sample_rate, &mut chunk) {
            break;
        }
        decoded += (chunk.len() / 2) as u64;
    }
    Ok((decoded, info.num_frames))
}

/// Decode a whole file through the same path playback uses and reduce it to
/// at most `bins` (min, max) mono sample pairs spanning the track, the data
/// behind a waveform strip. Pairs are normalized so the loudest bin hits 1,
/// with a gentle perceptual curve so quiet passages stay visible. No audio
/// device involved; run it on a background thread, a long track is a full
/// decode.
pub fn decode_peaks(path: &PathBuf, bins: usize) -> Result<Vec<(f32, f32)>, String> {
    // Probe once for the source rate, then open for real with the device
    // rate equal to it, so the resampler is a passthrough.
    let (probe, info) = Source::open(path, 48000)?;
    drop(probe);
    let (mut src, info) = Source::open(path, info.sample_rate)?;

    // Coarse pass: one pair per fixed block of frames, so memory stays a few
    // thousand pairs whatever the track length, then fold down to `bins`.
    const BLOCK_FRAMES: usize = 2048;
    let mut coarse: Vec<(f32, f32)> = Vec::new();
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    let mut in_block = 0usize;
    let mut chunk = Vec::new();
    loop {
        chunk.clear();
        if !src.next_chunk(info.sample_rate, &mut chunk) {
            break;
        }
        for frame in chunk.chunks_exact(2) {
            let s = (frame[0] + frame[1]) * 0.5;
            lo = lo.min(s);
            hi = hi.max(s);
            in_block += 1;
            if in_block == BLOCK_FRAMES {
                coarse.push((lo, hi));
                lo = f32::MAX;
                hi = f32::MIN;
                in_block = 0;
            }
        }
    }
    if in_block > 0 {
        coarse.push((lo, hi));
    }
    if coarse.is_empty() {
        return Err("no decodable audio".into());
    }

    // Fold the coarse pairs into the requested resolution, keeping each
    // bucket's extremes so transients survive the downsample.
    let mut peaks: Vec<(f32, f32)> = if coarse.len() <= bins.max(1) {
        coarse
    } else {
        let per = coarse.len() as f64 / bins as f64;
        (0..bins)
            .map(|i| {
                let from = (i as f64 * per) as usize;
                let to = (((i + 1) as f64 * per) as usize).clamp(from + 1, coarse.len());
                coarse[from..to]
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), &(bl, bh)| {
                        (lo.min(bl), hi.max(bh))
                    })
            })
            .collect()
    };

    let loudest = peaks
        .iter()
        .fold(0.0f32, |m, &(lo, hi)| m.max(lo.abs()).max(hi.abs()));
    if loudest > 0.0 {
        for (lo, hi) in peaks.iter_mut() {
            *lo = (lo.abs() / loudest).powf(0.7).copysign(*lo);
            *hi = (hi.abs() / loudest).powf(0.7).copysign(*hi);
        }
    }
    Ok(peaks)
}

/// Decode one window of audio starting at `position_secs`, resampled to
/// `device_rate` and interleaved stereo, at least `frames` frames when the
/// track has them. This is the paused-load prime for the spectrum: playback
/// only feeds the visualizer's tap while it renders, so a track loaded paused
/// has nothing to show. Decoding a single window off-thread gives the frozen
/// bars a real frame to stand on. No audio device involved; run it on a
/// background thread.
pub fn decode_window(
    path: &PathBuf,
    position_secs: f64,
    device_rate: u32,
    frames: usize,
) -> Result<Vec<f32>, String> {
    let (mut src, _) = Source::open(path, device_rate)?;
    if position_secs > 0.0 {
        src.seek(position_secs);
    }
    let mut out = Vec::with_capacity(frames * 2);
    let mut chunk = Vec::new();
    while out.len() < frames * 2 {
        chunk.clear();
        if !src.next_chunk(device_rate, &mut chunk) {
            break;
        }
        out.extend_from_slice(&chunk);
    }
    if out.is_empty() {
        return Err("no decodable audio".into());
    }
    Ok(out)
}

impl Source {
    fn open(path: &PathBuf, device_rate: u32) -> Result<(Source, TrackInfo), String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| format!("probe: {e}"))?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or("no audio track")?;
        let track_id = track.id;
        let time_base = track.time_base;

        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or("no audio codec parameters")?;
        let sample_rate = params.sample_rate.ok_or("unknown sample rate")?;
        let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(2);

        // num_frames already excludes encoder delay and padding in 0.6.
        let duration_secs = track
            .duration
            .zip(time_base)
            .and_then(|(dur, tb)| tb.calc_time(Timestamp::from(dur.get() as i64)))
            .map(|t| t.as_secs_f64())
            .or_else(|| track.num_frames.map(|n| n as f64 / sample_rate as f64));

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| format!("decoder: {e}"))?;

        let info = TrackInfo {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            duration_secs,
            num_frames: track.num_frames,
            sample_rate,
            channels,
        };

        Ok((
            Source {
                format,
                decoder,
                track_id,
                time_base,
                device_rate,
                resampler: Resampler::new(sample_rate, device_rate),
                scratch: Vec::new(),
            },
            info,
        ))
    }

    /// Decode packets until one yields samples, appending device-rate stereo
    /// to `out`. Returns false at end of stream.
    fn next_chunk(&mut self, device_rate: u32, out: &mut Vec<f32>) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return false,
                Err(e) => {
                    eprintln!("\npacket error, ending track: {e}");
                    return false;
                }
            };
            if packet.track_id != self.track_id {
                continue;
            }

            let (frames, rate, ch) = match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let frames = decoded.frames();
                    if frames == 0 {
                        continue;
                    }
                    let spec = decoded.spec();
                    let rate = spec.rate();
                    let ch = spec.channels().count();
                    self.scratch.resize(decoded.samples_interleaved(), 0.0);
                    decoded.copy_to_slice_interleaved(&mut self.scratch);
                    (frames, rate, ch)
                }
                // Corrupt or truncated packet: skip it, keep the track going.
                Err(Error::DecodeError(e)) => {
                    eprintln!("\ndecode error, skipping packet: {e}");
                    continue;
                }
                Err(Error::IoError(e)) => {
                    eprintln!("\nio error, skipping packet: {e}");
                    continue;
                }
                Err(e) => {
                    eprintln!("\nfatal decode error, ending track: {e}");
                    return false;
                }
            };

            if rate != self.resampler.src_rate() {
                self.resampler = Resampler::new(rate, device_rate);
            }

            // Fold to stereo: mono duplicates, extra channels drop. Real
            // downmix is engine work, not spike work.
            let stereo: Vec<f32> = match ch {
                2 => std::mem::take(&mut self.scratch),
                1 => {
                    let mut v = Vec::with_capacity(frames * 2);
                    for &s in &self.scratch {
                        v.push(s);
                        v.push(s);
                    }
                    v
                }
                n => {
                    let mut v = Vec::with_capacity(frames * 2);
                    for f in self.scratch.chunks_exact(n) {
                        v.push(f[0]);
                        v.push(f[1]);
                    }
                    v
                }
            };

            self.resampler.process(&stereo, out);
            if ch == 2 {
                self.scratch = stereo;
            }
            return true;
        }
    }

    /// Accurate seek. Returns the track position actually landed on, in
    /// seconds, which can differ from the request.
    fn seek(&mut self, secs: f64) -> f64 {
        let time = Time::try_from_secs_f64(secs).unwrap_or(Time::ZERO);
        match self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time,
                track_id: Some(self.track_id),
            },
        ) {
            Ok(seeked) => {
                self.decoder.reset();
                self.resampler = Resampler::new(self.resampler.src_rate(), self.device_rate);
                self.time_base
                    .and_then(|tb| tb.calc_time(seeked.actual_ts))
                    .map(|t| t.as_secs_f64().max(0.0))
                    .unwrap_or(secs)
            }
            Err(e) => {
                eprintln!("\nseek failed: {e}");
                // Position is unchanged; report where we were by falling back
                // to the request so the display does not lie wildly.
                secs
            }
        }
    }
}
