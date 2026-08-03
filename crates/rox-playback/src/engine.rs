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
use std::time::Instant;

use rtrb::Producer;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

use crate::chain::{Chain, Node};
use crate::gain;
use crate::latency;
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
    /// Arm or clear stop-after-current: armed, the track playing now ends
    /// the session's motion - the engine lets the ring drain so the last
    /// samples play out, then pauses with the next track cued at 0:00.
    /// Sticky until cleared, so every track end stops while armed.
    SetStopAfter(bool),
    /// Splice tracks into the queue right after entry `after` (its stable id),
    /// or at the end when `after` is None. `explicit` marks them as user-queued
    /// (Play Next, Add to Queue) rather than part of the playing context, so
    /// the queue widgets can show them apart from the album or library that
    /// plays on around them.
    Insert {
        after: Option<u64>,
        paths: Vec<PathBuf>,
        /// Album group per path, parallel to `paths` (ADR 17). The player
        /// resolves these from the library at insert time; the engine only
        /// compares them. Shorter than `paths` pads with None.
        groups: Vec<Option<u64>>,
        /// ReplayGain tags per path, parallel the same way and resolved
        /// from the library beside the groups. Shorter pads with the
        /// untagged default, which the rule's fallback then answers for.
        gains: Vec<gain::ReplayGain>,
        explicit: bool,
        /// Jump to the first of the batch and play it now, keeping the rest of
        /// the queue behind it. A drag onto Play now sets this; Play Next and
        /// Add to Queue leave it off so the current track keeps playing.
        and_play: bool,
    },
    /// Drop the entry with this id from the queue. Removing the playing entry
    /// is ignored; the UI never offers it.
    Remove {
        id: u64,
    },
    /// Drop a whole set of entries in one pass, with a single queue publish at
    /// the end. Clear Queue and multi-select delete route here so a big queue
    /// empties in one O(n) sweep instead of one O(n) remove per id. The playing
    /// entry is kept even if named.
    RemoveMany {
        ids: Vec<u64>,
    },
    /// Move the entry with this id to just after entry `after`, or to the
    /// front when `after` is None.
    Move {
        id: u64,
        after: Option<u64>,
    },
    /// Jump straight to the entry with this id and play it now.
    Jump {
        id: u64,
    },
    /// Append a node to the processing chain (ADR 19). Structural chain
    /// edits come this way. Parameter changes don't: a node's knobs are
    /// atomics the sender still holds a handle to, so turning one costs a
    /// store and no round trip. The node is reset to the live device rate
    /// on arrival, so it can be built before the stream's rate is known.
    ChainPush(Box<dyn Node>),
    /// How long a crossfade runs at a boundary that takes one, in seconds,
    /// and whether tracks of the same album count as one. Zero seconds
    /// disables it: every boundary is the gapless splice again. Rides the
    /// command channel rather than an atomic because the engine reads it
    /// while deciding to open a track, not per sample.
    SetCrossfade {
        secs: f32,
        /// Fade at album-contiguous boundaries too, overriding the rule
        /// that leaves a record's own splices alone. Off by default: an
        /// album that runs track into track was mastered that way, and
        /// fading it is a change to the record. On for a listener who
        /// wants every boundary soft whatever the tags say.
        albums: bool,
    },
    /// How tagged loudness is levelled (ADR 19): the mode and the two
    /// offsets. Applied to every source in hand as it arrives, so a mode
    /// switch is heard on the track playing rather than the one after it.
    /// The command channel rather than an atomic because the engine reads
    /// it when a source opens, not per sample.
    SetGainRule(gain::GainRule),
    Quit,
}

/// The longest crossfade on offer. Past this the overlap stops reading as
/// a transition between two tracks and starts reading as both playing at
/// once; the UI's slider tops out here and the engine clamps to it.
pub const CROSSFADE_MAX_SECS: f32 = 12.0;

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
    /// What this file's ReplayGain tags said, carried so the gain can be
    /// recomputed in place when the rule changes under a live stream.
    rg: gain::ReplayGain,
    /// This source's constant gain, the source-gain stage (ADR 19): the
    /// rule applied to the tags above. Per source rather than per stream
    /// because a fade window has two tracks live and each needs its own.
    /// Unity short-circuits, so the bypass rule holds.
    gain: f32,
    /// How far into the track this source has decoded, in device-rate
    /// frames. Counts what came out of the resampler and resets to the
    /// landing spot on a seek, so it's the track position rather than a
    /// count of work done. The crossfade window is measured off it.
    pos_frames: u64,
    /// The track's length in device-rate frames, where the container says.
    /// None for a stream that never claimed one, which is also the answer
    /// to "when does the fade window open": it doesn't.
    total_frames: Option<u64>,
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
    /// Album group per pool entry, parallel to `queue` (ADR 17). Grows with
    /// the pool on insert, never shrinks. The engine never derives these,
    /// only compares them: same group means tracks that belong together.
    groups: Vec<Option<u64>>,
    /// ReplayGain tags per pool entry, parallel the same way. Read off the
    /// file's tags by the library, never by the engine; what happens here
    /// is the rule below turning them into one factor per source.
    gains: Vec<gain::ReplayGain>,
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
    /// Stop at the end of the playing track instead of rolling on. Sticky:
    /// stays armed until cleared, so every boundary stops.
    stop_after: bool,
    /// An armed stop cut the gapless open at EOF; consumed once the ring
    /// drains, where the pause lands and the next track cues up.
    stop_pending: bool,
    /// Frames pushed on the frames_consumed clock; resynced after each flush.
    pushed_playable: u64,
    /// Decoded, converted samples waiting for ring space.
    pending: Vec<f32>,
    pending_pos: usize,
    /// The processing chain (ADR 19): runs over each decoded chunk after the
    /// fold and resample, immediately before the ring, at the device rate.
    /// Empty is the bypass rule: samples reach the ring untouched.
    chain: Chain,
    /// How tagged loudness becomes each source's constant gain (ADR 19).
    /// Off by default, which is unity everywhere and the bypass rule
    /// intact.
    rule: gain::GainRule,
    /// How long a crossfade runs, in seconds. Zero is off, and off means
    /// every boundary is exactly the gapless splice it was before.
    fade_secs: f32,
    /// Fade album-contiguous boundaries too, instead of leaving a record's
    /// own splices alone.
    fade_albums: bool,
    /// The fade in flight: the outgoing track, still decoding, mixed under
    /// what the open source produces until the window closes. None the
    /// rest of the time, which is nearly always.
    fade: Option<Fade>,
    /// Whether the open source has already had its boundary fade decided.
    /// Set the one time the window opens, cleared on every open, so a
    /// track whose next file wouldn't open doesn't retry every chunk.
    fade_armed: bool,
}

/// A crossfade in flight (ADR 19). The engine holds two open sources for
/// the length of the window: the new one is `source` and drives the loop,
/// this is the old one, decoded alongside and mixed underneath. One summed
/// stream reaches the ring, so the ring keeps its single producer.
struct Fade {
    /// The outgoing track, still decoding its own tail.
    src: Source,
    /// Its samples, decoded ahead of what the current chunk needs. The
    /// two sources' packet boundaries never line up, so the remainder of
    /// one chunk's decode waits here for the next.
    buf: Vec<f32>,
    /// How far into `buf` the mix has read.
    read: usize,
    /// Exactly as many of the outgoing's samples as the incoming chunk
    /// needs, gathered per chunk and reused.
    take: Vec<f32>,
    /// Frames of the window already mixed.
    done: u64,
    /// The window's length in device-rate frames.
    len: u64,
    /// The outgoing track ran out. Past this the mix reads silence, which
    /// is what a track shorter than its own fade window leaves.
    ended: bool,
}

/// A track a skip has wound back, on its way into a fade. Held between the
/// wind-back and the install, which sit either side of the flush.
struct Wound {
    src: Source,
    /// The output frame the clock stood at when the wind-back was aimed.
    at: u64,
    /// How far short of that spot the seek actually landed, in device-rate
    /// frames. Zero for anything with a seek index; positive where a coarse
    /// seek undershot, negative where it overshot.
    short: i64,
}

/// The playing context handed to a new engine: the ordered paths, where in
/// them to start, which entries are user-queued rather than part of the
/// context, the album group per entry, and its ReplayGain tags. The three
/// parallel vecs pad out where they run short of `paths`, with false, None,
/// and the untagged default.
#[derive(Default)]
pub struct StartQueue {
    pub paths: Vec<PathBuf>,
    pub start: usize,
    pub explicit: Vec<bool>,
    pub groups: Vec<Option<u64>>,
    pub gains: Vec<gain::ReplayGain>,
}

impl Engine {
    pub fn new(
        queue: StartQueue,
        shared: Arc<Shared>,
        producer: Producer<f32>,
        device_rate: u32,
        rx: Receiver<Cmd>,
    ) -> Self {
        let StartQueue {
            paths: queue,
            start,
            explicit,
            groups,
            gains,
        } = queue;
        // The starting queue is the playing context: an album, a library run,
        // whatever the caller handed over. A fresh context passes an empty
        // `explicit`, so every entry is context; a launch restore passes the
        // saved flags so the up-next queue comes back marked. Later Play Next
        // and Add to Queue splice in more explicit entries through Insert.
        // `groups` runs parallel the same way; short vecs pad with None.
        let order = (0..queue.len())
            .map(|idx| OrderEntry {
                id: idx as u64,
                idx,
                explicit: explicit.get(idx).copied().unwrap_or(false),
            })
            .collect();
        let mut groups = groups;
        groups.resize(queue.len(), None);
        let mut gains = gains;
        gains.resize(queue.len(), gain::ReplayGain::default());
        Engine {
            order,
            groups,
            gains,
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
            stop_after: false,
            stop_pending: false,
            pushed_playable: 0,
            pending: Vec::new(),
            pending_pos: 0,
            chain: Chain::new(),
            rule: gain::GainRule::default(),
            fade_secs: 0.0,
            fade_albums: false,
            fade: None,
            fade_armed: false,
        }
    }

    pub fn run(mut self) {
        // Stream open: the chain learns the device rate before any sample
        // passes through it. It resets again on every flush, never at the
        // gapless boundary, so filter history carries across a track splice.
        self.chain.reset(self.device_rate);
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
            // Which way the navigation went, for the transport's fade
            // readout: a Previous sweeps the other way from a Next.
            let mut nav_back = false;
            // A remove dropped the pre-decoded next track while its source was
            // already open. Set here, acted on after the drain: drop that stale
            // source and reopen the real next track so the removed one doesn't
            // play on. The audible track is fully in the ring, so no flush.
            let mut reopen_runahead = false;
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
                        nav_back = false;
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
                        nav_back = true;
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
                    Cmd::SetStopAfter(on) => self.stop_after = on,
                    Cmd::Insert {
                        after,
                        paths,
                        groups,
                        gains,
                        explicit,
                        and_play,
                    } => {
                        let at = self.insert(after, paths, groups, gains, explicit);
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
                    Cmd::Remove { id } => reopen_runahead |= self.remove(id),
                    Cmd::RemoveMany { ids } => reopen_runahead |= self.remove_many(&ids),
                    Cmd::Move { id, after } => self.move_entry(id, after),
                    // Reuse the nav path: setting the target flushes and opens
                    // it just like a Next would.
                    Cmd::Jump { id } => {
                        if let Some(p) = self.find(id) {
                            nav_pos = Some(p);
                        }
                        flush_to = None;
                    }
                    Cmd::ChainPush(node) => self.chain.push(node),
                    Cmd::SetCrossfade { secs, albums } => {
                        self.fade_secs = crossfade_secs(secs);
                        self.fade_albums = albums;
                    }
                    Cmd::SetGainRule(rule) => {
                        self.rule = rule;
                        // Both sources in hand, so a switch made during a
                        // fade takes on the track going out as well as the
                        // one coming in. Each keeps its own tags, so they
                        // land on different factors.
                        if let Some(src) = source.as_mut() {
                            src.relevel(&rule);
                        }
                        if let Some(fade) = self.fade.as_mut() {
                            fade.src.relevel(&rule);
                        }
                    }
                    Cmd::Quit => return,
                }
            }
            if let Some(p) = nav_pos {
                flush_to = Some(FlushAction::Track {
                    pos: p,
                    back: nav_back,
                });
            }

            // A flush is the one thing here the listener hears as a hole, so
            // the work either side of it is arranged around that: everything
            // that can be done while the ring is still playing happens
            // before the cut, and what's left after it is arithmetic.
            if let Some(action) = flush_to {
                match action {
                    FlushAction::Track { pos, back } => {
                        source = self.skip_to(source.take(), pos, back);
                    }
                    FlushAction::Seek(secs) => {
                        // The decode cursor leads the audible track by up to a
                        // ring during the gapless preroll, so the open source
                        // is already the next track and seeking it would scrub
                        // inside the following track. Reopen the audible track
                        // first, the same anchor Next/Prev use.
                        let ap = self.audible_pos();
                        let mut reopened = None;
                        if ap != self.pos {
                            // A reopen that fails leaves the open source on
                            // the wrong track (the pre-rolled next one), so
                            // it's dropped either way rather than scrubbing
                            // audio the user isn't hearing.
                            reopened = self.open_file_at(ap);
                            source = None;
                        }
                        // The seek is the expensive half and it doesn't
                        // depend on the cut, so it runs here too, while the
                        // ring is still playing.
                        let landed = match reopened.as_mut() {
                            Some((src, _, _)) => src.seek(secs),
                            None => source.as_mut().and_then(|src| src.seek(secs)),
                        };
                        self.flush_ring();
                        if let Some((src, at, info)) = reopened {
                            self.adopt(at, info, 0);
                            source = Some(src);
                        }
                        if let Some(landed) = landed {
                            self.register_segment(landed);
                        }
                    }
                }
                continue;
            }

            // The pre-decoded next track was removed with its source open.
            // Drop that stale source and reopen the track now sitting in its
            // slot, right after the audible one. The audible track already
            // filled the ring, so this reopen is silent, no flush needed. Any
            // half-decoded pending samples belong to the removed track, so
            // clear them too. Skipped when a flush already reopened above.
            //
            // Residual risk: if the decode cursor got far enough ahead that
            // some of the removed track's samples already reached the ring
            // (bounded by RING_SECS), that fraction still plays before the
            // reopened next track takes over. Flushing the ring would drop it
            // but would also cut the untouched audible track mid-note, a worse
            // glitch, so we accept the short tail here.
            if reopen_runahead {
                let next = self.audible_pos() + 1;
                self.pending.clear();
                self.pending_pos = 0;
                source = if next < self.order.len() {
                    self.open_at(next)
                } else if self.loop_mode == LoopMode::All && !self.order.is_empty() {
                    self.open_at(0)
                } else {
                    None
                };
                continue;
            }

            // Move pending samples into the ring. Ring full means we're
            // comfortably ahead; nap and go back to command handling.
            //
            // Full is whatever the latency hold says it is (ADR 19). With an
            // EQ editor open the gate closes early, so the ring keeps its
            // 500 ms of capacity as the underrun cushion but only carries a
            // fraction of it, and a knob is heard that much sooner. The nap
            // below already covers "couldn't push it all".
            let capacity = self.producer.buffer().capacity();
            let room = latency::push_room(capacity, self.producer.slots(), self.device_rate);
            let stop = self.pending.len().min(self.pending_pos + room);
            while self.pending_pos < stop {
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

            // The boundary crossfade opens its window here, one fade length
            // out from the end of the playing track (ADR 19): the next track
            // opens early and the two overlap, where a gapless boundary
            // would have waited for EOF. Only for boundaries that take a
            // fade; an album's own tracks fall through to the splice below
            // untouched.
            if self.fade_due(source.as_ref()) {
                source = self.start_boundary_fade(source.take());
            }

            // Refill from the decoder.
            match source.as_mut() {
                Some(src) => {
                    let device_rate = self.device_rate;
                    let more = src.next_chunk(device_rate, &mut self.pending);
                    // A fade in flight mixes the outgoing track underneath
                    // before anything downstream sees the samples, so the
                    // chain shapes the mix and the ring gets one stream.
                    self.mix_fade();
                    // The last step before the ring (ADR 19): chain output
                    // rides through flush, seek, and the gapless boundary
                    // like any other sample data, and the tap downstream
                    // sees what the chain produced.
                    self.chain.process(&mut self.pending);
                    if !more {
                        // A window still open at this track's own EOF means
                        // the track was shorter than the fade: ramp what's
                        // left of the outgoing tail out now, so it doesn't
                        // carry on under the track opening below.
                        self.close_fade_fast();
                        // EOF: swap the decoder under the live stream. No
                        // flush, no stream teardown; this IS the gapless
                        // boundary. Loop modes pick the next open: One
                        // reopens the same track, All wraps the queue. An
                        // armed stop-after skips the open instead - the
                        // drain below is where the pause lands, so the
                        // track's tail still plays out of the ring.
                        source = if self.stop_after {
                            self.stop_pending = true;
                            None
                        } else {
                            self.next_pos().and_then(|p| self.open_at(p))
                        };
                    }
                }
                None => {
                    // Nothing to mix a fade under: the incoming track is
                    // what drives the mix, and there isn't one. Whatever was
                    // fading out is done, and its publish goes with it -
                    // unmixed, nothing would ever run past it to clear it.
                    self.drop_fade();
                    // Queue exhausted, or an armed stop-after cut the
                    // gapless open: either way the ring drains first so the
                    // last samples play out.
                    let cap = self.producer.buffer().capacity();
                    if self.producer.slots() == cap {
                        if self.stop_pending {
                            // The stop landed: pause, then cue what EOF
                            // would have opened so Play resumes right
                            // there. A stop disarmed during the drain just
                            // rolls on. With nothing to cue (last track,
                            // loop off) fall through to the ended state.
                            self.stop_pending = false;
                            if let Some(p) = self.next_pos() {
                                if self.stop_after {
                                    self.shared.playing.store(false, Ordering::Relaxed);
                                }
                                source = self.open_at(p);
                                if source.is_some() {
                                    continue;
                                }
                            }
                        }
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
    fn open_at(&mut self, p: usize) -> Option<Source> {
        self.open_at_from(p, 0)
    }

    /// [`open_at`](Self::open_at) with the new track's position segment
    /// registered `after` frames later than its first sample reaches the
    /// ring, and starting `after` frames into the track to match. Zero
    /// everywhere except a crossfade, where the boundary the listener hears
    /// is the middle of the window rather than its start: the clock, the
    /// track-change notification, and MPRIS all flip there, so nothing
    /// announces a track before it is audible (ADR 19).
    fn open_at_from(&mut self, p: usize, after: u64) -> Option<Source> {
        let (src, at, info) = self.open_file_at(p)?;
        self.adopt(at, info, after);
        Some(src)
    }

    /// Open the file at play-order position `p`, falling forward through
    /// unreadable ones, and hand back the source with the position it
    /// actually landed on. Changes nothing: no cursor move, no segment, no
    /// track info published.
    ///
    /// Split out so a skip can pay for the open - the file, the probe, the
    /// decoder - while the old track is still coming out of the ring, and
    /// leave the flush with nothing to hold the silence open for.
    fn open_file_at(&mut self, mut p: usize) -> Option<(Source, usize, TrackInfo)> {
        while p < self.order.len() {
            let i = self.order[p].idx;
            match Source::open(&self.queue[i], self.device_rate) {
                Ok((mut src, info)) => {
                    // The gain rides with the track open (ADR 19), so it
                    // changes exactly where the source does.
                    src.level(self.gains[i], &self.rule);
                    return Some((src, p, info));
                }
                Err(e) => {
                    log::warn!("skipping {}: {e}", self.queue[i].display());
                    p += 1;
                }
            }
        }
        None
    }

    /// Take an opened source on as the playing one: move the cursor, publish
    /// the track info, and register the position segment `after` frames out.
    ///
    /// The segment starts at `after` rather than zero. Delaying it moves when
    /// the clock flips tracks, it doesn't move where the new track starts
    /// playing: its first sample goes into the mix at `pushed_playable`, so by
    /// the time the segment is reached the track is already `after` frames in.
    /// Claiming zero there would leave the position half a fade behind the
    /// audio for the rest of the track, and a later skip winds the outgoing
    /// track back to that position, which is a jump backwards you can hear.
    fn adopt(&mut self, p: usize, info: TrackInfo, after: u64) {
        let i = self.order[p].idx;
        self.pos = p;
        self.idx = i;
        // A fresh track has its own boundary to decide about.
        self.fade_armed = false;
        self.shared.tracks.lock().unwrap()[i] = Some(info);
        let at_frame = self.pushed_playable + after;
        let consumed = self.shared.frames_consumed.load(Ordering::Relaxed);
        let mut segments = self.shared.segments.lock().unwrap();
        segments.push(Segment {
            at_frame,
            track: i,
            track_frame: after,
        });
        prune_segments(&mut segments, consumed);
    }

    /// Where the next open lands when the playing track ends: the same
    /// track under repeat-one, the next one in the order, the top under
    /// repeat-all. None when the queue is played out.
    fn next_pos(&self) -> Option<usize> {
        if self.loop_mode == LoopMode::One {
            Some(self.pos)
        } else if self.pos + 1 < self.order.len() {
            Some(self.pos + 1)
        } else if self.loop_mode == LoopMode::All && !self.order.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    /// Whether the boundary from order position `from` to `to` takes a
    /// fade. The album group decides (ADR 17, ADR 19): two tracks of the
    /// same album keep the splice they were mastered for, bit-identical to
    /// what gapless does today. Anything else is a cut between unrelated
    /// music, which is what crossfade exists to soften, and two ungrouped
    /// tracks fade because nothing says they belong together. With
    /// `fade_albums` the record's own splices go too, for a listener who
    /// wants every boundary soft.
    fn fades_between(&self, from: usize, to: usize) -> bool {
        let (Some(a), Some(b)) = (self.order.get(from), self.order.get(to)) else {
            return false;
        };
        // Repeat-one comes back to the same file. A track overlapping its
        // own head is an effect, not a transition, and it stays out even
        // when every other boundary is fading.
        if a.idx == b.idx {
            return false;
        }
        if self.fade_albums {
            return true;
        }
        match (self.groups[a.idx], self.groups[b.idx]) {
            (Some(x), Some(y)) => x != y,
            _ => true,
        }
    }

    /// The fade window for a track of `total` device-rate frames. Never
    /// more than half the track: a fade longer than what it is leaving
    /// would open before the track had got going.
    fn fade_window(&self, total: Option<u64>) -> u64 {
        let len = (self.fade_secs.max(0.0) as f64 * self.device_rate as f64) as u64;
        match total {
            Some(total) => len.min(total / 2),
            None => len,
        }
    }

    /// Whether the playing track has reached its fade window and the
    /// boundary ahead is one that fades. Re-asked every chunk rather than
    /// latched, so queueing something else during the last seconds of a
    /// track is still honored; `fade_armed` only stops the retry once the
    /// window has actually been acted on.
    fn fade_due(&self, src: Option<&Source>) -> bool {
        match src {
            Some(src) => self.window_open(src.total_frames, src.remaining()),
            None => false,
        }
    }

    /// The window test over the numbers alone: how long the playing track
    /// is and how much of it is left.
    fn window_open(&self, total: Option<u64>, remaining: Option<u64>) -> bool {
        // An armed stop-after ends the session at this boundary, so there
        // is nothing to fade into.
        if self.fade_armed || self.fade.is_some() || self.stop_after {
            return false;
        }
        let len = self.fade_window(total);
        if len == 0 {
            return false;
        }
        // A track of unknown length never opens a window: there is no
        // honest answer to how far from the end it is.
        if remaining.is_none_or(|left| left > len) {
            return false;
        }
        self.next_pos()
            .is_some_and(|next| self.fades_between(self.pos, next))
    }

    /// Open the next track early and hand it back as the one driving the
    /// loop, with the track it overlaps moved into the fade. Returns the
    /// old source untouched when there is nothing to open, so the boundary
    /// falls back to the gapless splice.
    fn start_boundary_fade(&mut self, old: Option<Source>) -> Option<Source> {
        let old = old?;
        // One attempt per track: a next file that won't open shouldn't be
        // retried on every chunk for the rest of this one.
        self.fade_armed = true;
        let Some(next) = self.next_pos() else {
            return Some(old);
        };
        // Never longer than what's actually left of the outgoing track.
        // At a boundary reached by playing, the two are the same number;
        // they part company when a seek lands inside the window, and then
        // the fade is as long as the tail it has to work with rather than
        // the new track rising out of silence.
        let len = self
            .fade_window(old.total_frames)
            .min(old.remaining().unwrap_or(u64::MAX));
        // Nothing left to fade with: a seek that landed on or past the end
        // the container claims, or a container claiming fewer frames than
        // the file holds. Fall through to the plain splice rather than
        // build a one-frame window, which is what Fade::new would clamp
        // this to and it would zero the incoming track's first frame.
        if len == 0 {
            return Some(old);
        }
        match self.open_at_from(next, len / 2) {
            Some(new) => {
                // A boundary fade is the queue moving forward, so the
                // transport shows it the way a Next would.
                self.publish_fade(self.pushed_playable, len, false);
                self.fade = Some(Fade::new(old, len));
                Some(new)
            }
            None => Some(old),
        }
    }

    /// Jump to play-order position `p`: open what's there, wind the track
    /// being left back to what the listener has actually heard, cut the
    /// ring, and take the new source on with the fade under it (ADR 19).
    /// `back` only says which way the skip went, for the transport.
    ///
    /// The open happens first so its probe and decoder build are paid for
    /// while the ring is still playing, and the flush has nothing left to
    /// hold the silence open for.
    fn skip_to(&mut self, old: Option<Source>, p: usize, back: bool) -> Option<Source> {
        let opened = self.open_file_at(p);
        let leaving = self.prepare_skip_fade(old);
        let cut = self.flush_ring();
        self.shared.ended.store(false, Ordering::Relaxed);
        // Nothing opened means nothing drives the mix, so there's no fade to
        // install either. Publishing one here would leave the transport
        // showing an overlap that never renders and never clears: the mix
        // never runs, so nothing is ever there to close it.
        let (src, at, info) = opened?;
        let midpoint = self.install_skip_fade(leaving, cut, back);
        self.adopt(at, info, midpoint);
        Some(src)
    }

    /// Wind the track a skip is leaving back to the spot the listener has
    /// actually reached, ready to carry the fade under the new one, and
    /// report the output frame that spot sits at.
    ///
    /// The decode cursor runs up to a ring ahead of the speakers, so the
    /// open source is well past what was heard; the wind-back is what makes
    /// the fade start under the last sample that got out. It happens before
    /// the flush, since a seek is the expensive part and paying for it
    /// during the cut would hold the silence open;
    /// [`install_skip_fade`](Self::install_skip_fade) takes the drift back
    /// off the far side.
    ///
    /// None where nothing should fade: the fade is off, the skip came while
    /// paused (nobody is hearing the old track, and its tail would arrive as
    /// a surprise on the next Play), the open source isn't the audible track
    /// at all (the gapless preroll already swapped it, or another fade is
    /// halfway through), or the wind-back failed.
    fn prepare_skip_fade(&mut self, old: Option<Source>) -> Option<Wound> {
        let mut old = old?;
        if self.fade_secs <= 0.0
            || self.pos != self.audible_pos()
            || !self.shared.playing.load(Ordering::Relaxed)
        {
            return None;
        }
        // One clock reading for both, so the position and the frame it
        // belongs to can't drift a callback apart.
        let at = self.shared.frames_consumed.load(Ordering::Relaxed);
        let (_, secs) = self.shared.position_at(at, self.device_rate)?;
        // Where the seek asked to go and where it actually landed are the
        // same number for anything with an index, and seconds apart for a
        // CBR MP3 without one. Carry the difference so the install can
        // discard it instead of replaying music that already played.
        let landed = old.seek(secs)?;
        let short = ((secs - landed) * self.device_rate as f64).round() as i64;
        Some(Wound {
            src: old,
            at,
            short,
        })
    }

    /// Install the wound-back track as the fade under the new one, and
    /// report where the new track's segment goes: the middle of the window,
    /// or zero when nothing fades and the skip cuts as it always did.
    fn install_skip_fade(&mut self, leaving: Option<Wound>, cut: u64, back: bool) -> u64 {
        let Some(Wound { src, at, short }) = leaving else {
            return 0;
        };
        // The wind-back aimed at where the clock stood before the flush, and
        // the frames that went out during the flush itself are still ahead
        // of it. Hand those to nobody, so the fade starts on the sample the
        // cut landed on rather than replaying the last few milliseconds.
        let owed = cut.saturating_sub(at);
        // Past a quarter second the flush didn't take a period, it stalled
        // (a dead backend riding the deadline out). Skipping that much of a
        // track to line up with it isn't worth doing; cut instead.
        if owed > self.device_rate as u64 / 4 {
            return 0;
        }
        let Some(discard) = skip_fade_discard(owed, short, self.device_rate) else {
            return 0;
        };
        let mut fade = Fade::new(src, 1);
        if discard > 0 {
            fade.pull(self.device_rate, discard as usize * 2);
        }
        // No longer than what is left of the track being left: past its
        // end the mix is silence, and the new track would be rising out of
        // nothing rather than out of music.
        let len = self
            .fade_window(fade.src.total_frames)
            .min(fade.src.remaining().unwrap_or(u64::MAX));
        if len == 0 {
            return 0;
        }
        fade.len = len;
        self.publish_fade(cut, len, back);
        self.fade = Some(fade);
        len / 2
    }

    /// Drop the fade in flight and take back what was published for it.
    /// For the cases where a window is abandoned before the mix ever runs:
    /// nothing downstream would close it, and the transport reads the fade
    /// off the output clock, so a stale publish sits there forever.
    fn drop_fade(&mut self) {
        if self.fade.take().is_some() {
            self.shared.fade_len.store(0, Ordering::Release);
        }
    }

    /// Bring the fade in flight to its end within a few milliseconds, for
    /// when the track driving it ended first. A track shorter than the fade
    /// window leaves one open at its own EOF, and left alone the outgoing
    /// track keeps playing under the track after this one as well.
    ///
    /// Shrinking the window is the whole of it: the curve reads its progress
    /// off `len`, so the tail runs out its ramp over the next chunk or two
    /// and the mix closes itself. What was published stands, same as a
    /// window that closed on time - the ear is still inside it.
    fn close_fade_fast(&mut self) {
        let ramp = (self.device_rate as u64 / 50).max(1);
        if let Some(fade) = self.fade.as_mut() {
            fade.len = fade.len.min(fade.done + ramp);
        }
    }

    /// Mix the outgoing track under the chunk just decoded, and close the
    /// window once it has run its length. The two sources sum here, in the
    /// engine, so what reaches the chain and the ring is one stream.
    fn mix_fade(&mut self) {
        let device_rate = self.device_rate;
        let closed = {
            let Some(fade) = self.fade.as_mut() else {
                return;
            };
            fade.pull(device_rate, self.pending.len());
            gain::crossfade_mix(&mut self.pending, &fade.take, fade.done, fade.len);
            fade.done += (self.pending.len() / 2) as u64;
            fade.done >= fade.len
        };
        if closed {
            // Only the mixing side is done here. What was published stands
            // until the output clock runs past it, which is up to a ring
            // later: the transport shows the fade while the ear is in it,
            // not while the decode thread is.
            self.fade = None;
        }
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
                group: self.groups[e.idx],
            })
            .collect();
        *self.shared.queue.lock().unwrap() = QueueSnapshot {
            entries,
            cursor: self.pos,
        };
        self.shared.queue_rev.fetch_add(1, Ordering::Release);
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
    fn insert(
        &mut self,
        after: Option<u64>,
        paths: Vec<PathBuf>,
        groups: Vec<Option<u64>>,
        gains: Vec<gain::ReplayGain>,
        explicit: bool,
    ) -> Option<usize> {
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
        for (i, path) in paths.into_iter().enumerate() {
            let idx = self.queue.len();
            self.queue.push(path);
            self.groups.push(groups.get(i).copied().flatten());
            self.gains.push(gains.get(i).copied().unwrap_or_default());
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
    ///
    /// Returns true when the entry removed was the runahead, the pre-decoded
    /// next track the decode cursor already opened into the ring. The caller
    /// must then flush that open source, or it plays on in full even though
    /// it's no longer in the queue.
    fn remove(&mut self, id: u64) -> bool {
        let Some(p) = self.find(id) else {
            return false;
        };
        let audible = self.audible_pos();
        if p == audible {
            return false;
        }
        // The open source is the entry at the decode cursor. When that leads
        // the audible track, the cursor entry is the pre-rolled next track;
        // removing it strands an open source on a track no longer queued.
        let removed_runahead = p == self.pos && self.pos != audible;
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
        removed_runahead
    }

    /// Drop every entry named in `ids` in one sweep, keeping the audible one so
    /// playback never cuts, then re-find the decode cursor by id and publish
    /// once. One pass over the order rather than a find-and-remove per id, so
    /// clearing a huge queue stays O(n) with a single UI wake instead of O(n^2)
    /// with a wake per entry.
    ///
    /// Returns true when the sweep dropped the runahead, the pre-decoded next
    /// track the decode cursor already opened. The caller flushes the stale
    /// open source in that case, same as single remove.
    fn remove_many(&mut self, ids: &[u64]) -> bool {
        if ids.is_empty() {
            return false;
        }
        let drop: std::collections::HashSet<u64> = ids.iter().copied().collect();
        let audible = self.audible_pos();
        let keep = self.order.get(audible).map(|e| e.id);
        let cursor = self.order.get(self.pos).map(|e| e.id);
        // The cursor entry is the runahead when it leads the audible track and
        // it's actually being dropped (not the kept audible one).
        let removed_runahead =
            self.pos != audible && cursor.is_some_and(|id| drop.contains(&id) && Some(id) != keep);
        let before = self.order.len();
        self.order
            .retain(|e| !drop.contains(&e.id) || Some(e.id) == keep);
        if self.order.len() == before {
            return false;
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
        removed_runahead
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

    /// Have the backend discard everything queued and tell us it has, then
    /// resync our clock to what actually played. Returns the output frame
    /// the cut landed on, which is where the next sample pushed will play.
    ///
    /// The wait is the whole gap a skip costs: the ring is clear the moment
    /// the ack lands, so the sooner this returns the sooner audio comes
    /// back. Everything the caller can do beforehand - opening the next
    /// file, winding a source back - belongs before the call, while the ring
    /// is still playing.
    fn flush_ring(&mut self) -> u64 {
        self.pending.clear();
        self.pending_pos = 0;
        // Whatever was fading is gone with the flushed samples. A skip
        // starts its own fade after this, from the spot the clock stopped
        // at; a seek just cuts, which is what scrubbing should do.
        self.fade = None;
        self.shared.fade_len.store(0, Ordering::Release);
        // A flush is a discontinuity: stateful nodes re-anchor rather than
        // smear filter history across the jump.
        self.chain.reset(self.device_rate);
        let seq = self.shared.flush_seq.fetch_add(1, Ordering::Release) + 1;
        // A live backend answers within one period; bound the wait so a dead
        // output stream (unplugged device, callback stopped) can't spin here
        // forever. Past the deadline we resync anyway, at worst a few stale ms.
        let deadline = Instant::now() + StdDuration::from_millis(500);
        while self.shared.flush_ack.load(Ordering::Acquire) < seq {
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(StdDuration::from_millis(1));
        }
        self.pushed_playable = self.shared.frames_consumed.load(Ordering::Relaxed);
        self.pushed_playable
    }

    /// Publish the fade window for the transport, in output-clock frames.
    /// Read back through [`Shared::crossfade`], which only shows it once the
    /// speakers reach it.
    fn publish_fade(&self, at: u64, len: u64, back: bool) {
        self.shared.fade_at.store(at, Ordering::Relaxed);
        self.shared.fade_back.store(back, Ordering::Relaxed);
        self.shared.fade_len.store(len, Ordering::Release);
    }

    fn register_segment(&self, track_secs: f64) {
        let consumed = self.shared.frames_consumed.load(Ordering::Relaxed);
        let mut segments = self.shared.segments.lock().unwrap();
        segments.push(Segment {
            at_frame: self.pushed_playable,
            track: self.idx,
            track_frame: (track_secs * self.device_rate as f64).round() as u64,
        });
        prune_segments(&mut segments, consumed);
    }
}

/// Drop position segments that can never resolve again. Both readers take the
/// newest segment with `at_frame <= consumed`, and the output clock only ever
/// advances, so once it passes a later segment no earlier one is ever the
/// answer. Keep the newest already-reached segment plus every future one; the
/// vec stays a handful of entries instead of growing one per open and seek for
/// the whole session.
fn prune_segments(segments: &mut Vec<Segment>, consumed: u64) {
    let cutoff = segments
        .iter()
        .filter(|s| s.at_frame <= consumed)
        .map(|s| s.at_frame)
        .max();
    if let Some(cutoff) = cutoff {
        segments.retain(|s| s.at_frame >= cutoff);
    }
}

/// The fade length a `SetCrossfade` really means, in seconds. NaN sails
/// straight through a clamp, and every test downstream of it is a
/// comparison NaN answers false to, so the fade would read as on while
/// never rendering: each skip pays for a wind-back seek and then cuts.
/// Normalized here, at the one place a length arrives.
fn crossfade_secs(secs: f32) -> f32 {
    if secs.is_nan() {
        return 0.0;
    }
    secs.clamp(0.0, CROSSFADE_MAX_SECS)
}

/// How much of the wound-back track to throw away before its tail goes
/// under the new one: the frames that went out during the flush, plus
/// whatever a coarse seek undershot by. None means give the fade up and let
/// the skip cut.
fn skip_fade_discard(owed: u64, short: i64, device_rate: u32) -> Option<u64> {
    let total = owed as i64 + short;
    // The seek overshot the spot instead, and decoding forward can't undo
    // that. Start the tail where it landed rather than throw more away.
    if total <= 0 {
        return Some(0);
    }
    // The wind-back runs before the flush precisely so the cut isn't held
    // open for decode work; a seek that landed a second-plus short would
    // put that work back inside it. Not worth the silence: cut instead.
    if total > device_rate as i64 {
        return None;
    }
    Some(total as u64)
}

enum FlushAction {
    Seek(f64),
    /// Jump to this play-order position.
    Track {
        pos: usize,
        /// The jump came from a Previous. Only the transport's fade
        /// readout cares; the engine treats both directions the same.
        back: bool,
    },
}

impl Fade {
    fn new(src: Source, len: u64) -> Fade {
        Fade {
            src,
            buf: Vec::new(),
            read: 0,
            take: Vec::new(),
            done: 0,
            // A zero-length window would divide the curve by nothing; the
            // callers that mean "no fade" never build one.
            len: len.max(1),
            ended: false,
        }
    }

    /// Gather exactly `samples` of the outgoing track for this chunk,
    /// decoding as far as it takes. Comes up short only at the track's own
    /// end, which the mix reads as silence.
    fn pull(&mut self, device_rate: u32, samples: usize) {
        self.take.clear();
        while self.take.len() < samples {
            let have = self.buf.len() - self.read;
            if have > 0 {
                let take = have.min(samples - self.take.len());
                self.take
                    .extend_from_slice(&self.buf[self.read..self.read + take]);
                self.read += take;
                continue;
            }
            self.buf.clear();
            self.read = 0;
            if self.ended {
                break;
            }
            if !self.src.next_chunk(device_rate, &mut self.buf) {
                self.ended = true;
            }
        }
    }
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
        // The EOF call flushes the resampler's final frame into `chunk`, so
        // count what it returns before honouring the end signal.
        let more = src.next_chunk(info.sample_rate, &mut chunk);
        decoded += (chunk.len() / 2) as u64;
        if !more {
            break;
        }
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
        // The EOF call flushes the resampler's final frame into `chunk`, so
        // fold it in before honouring the end signal.
        let more = src.next_chunk(info.sample_rate, &mut chunk);
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
        if !more {
            break;
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
        let _ = src.seek(position_secs);
    }
    let mut out = Vec::with_capacity(frames * 2);
    let mut chunk = Vec::new();
    while out.len() < frames * 2 {
        chunk.clear();
        // The EOF call flushes the resampler's final frame into `chunk`, so
        // take it before honouring the end signal.
        let more = src.next_chunk(device_rate, &mut chunk);
        out.extend_from_slice(&chunk);
        if !more {
            break;
        }
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

        // The playable length at the device rate, which is the clock the
        // fade window is measured on. Prefer the frame count the container
        // states (encoder delay and padding already out of it) and fall
        // back to the duration.
        let total_frames = track
            .num_frames
            .map(|n| (n as f64 * device_rate as f64 / sample_rate as f64).round() as u64)
            .or_else(|| duration_secs.map(|secs| (secs * device_rate as f64).round() as u64));

        Ok((
            Source {
                format,
                decoder,
                track_id,
                time_base,
                device_rate,
                resampler: Resampler::new(sample_rate, device_rate),
                scratch: Vec::new(),
                rg: gain::ReplayGain::default(),
                gain: 1.0,
                pos_frames: 0,
                total_frames,
            },
            info,
        ))
    }

    /// Take on this file's ReplayGain tags and the rule to read them by.
    /// Called on the way out of the open, and again on every source in
    /// hand when the rule changes, so a mode switch is heard on the track
    /// playing rather than the one after it.
    fn level(&mut self, rg: gain::ReplayGain, rule: &gain::GainRule) {
        self.rg = rg;
        self.relevel(rule);
    }

    /// The same against the tags already in hand, for a rule that changed
    /// under an open source.
    fn relevel(&mut self, rule: &gain::GainRule) {
        self.gain = rule.factor(self.rg);
    }

    /// Frames left before this track ends, at the device rate. None where
    /// the container never said how long it is.
    fn remaining(&self) -> Option<u64> {
        self.total_frames
            .map(|total| total.saturating_sub(self.pos_frames))
    }

    /// Decode packets until one yields samples, appending device-rate stereo
    /// to `out` with this source's own gain applied. Returns false at end of
    /// stream.
    fn next_chunk(&mut self, device_rate: u32, out: &mut Vec<f32>) -> bool {
        let from = out.len();
        let more = self.decode_chunk(device_rate, out);
        // The source-gain stage (ADR 19), before this source's samples meet
        // any other's. A fade's per-frame pair applies over in the engine,
        // where both sources are in hand.
        gain::apply(&mut out[from..], self.gain);
        self.pos_frames += ((out.len() - from) / 2) as u64;
        more
    }

    /// The decode itself: packets in, device-rate stereo appended to `out`.
    fn decode_chunk(&mut self, device_rate: u32, out: &mut Vec<f32>) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                // End of stream: flush the resampler's carried final frame so
                // the last source sample isn't dropped at the track boundary.
                Ok(None) => {
                    self.resampler.flush(out);
                    return false;
                }
                Err(e) => {
                    log::warn!("packet error, ending track: {e}");
                    self.resampler.flush(out);
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
                    log::warn!("decode error, skipping packet: {e}");
                    continue;
                }
                Err(Error::IoError(e)) => {
                    log::warn!("io error, skipping packet: {e}");
                    continue;
                }
                Err(e) => {
                    log::error!("fatal decode error, ending track: {e}");
                    self.resampler.flush(out);
                    return false;
                }
            };

            if rate != self.resampler.src_rate() {
                // Mid-stream rate change (a VBR container switching, a chained
                // stream). Flush the old resampler's carried final frame before
                // swapping it out, otherwise that frame is dropped at the seam.
                self.resampler.flush(out);
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
    /// seconds, which can differ from the request. None when the seek failed
    /// and the reader never moved, so the caller doesn't register a segment
    /// that jumps the position display to a spot playback never reached.
    fn seek(&mut self, secs: f64) -> Option<f64> {
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
                let landed = self
                    .time_base
                    .and_then(|tb| tb.calc_time(seeked.actual_ts))
                    .map(|t| t.as_secs_f64().max(0.0))
                    .unwrap_or(secs);
                // The track position moved, so the fade window's countdown
                // moves with it: a seek into the last seconds of a track
                // opens the window, a seek back out of it closes it again.
                self.pos_frames = (landed * self.device_rate as f64).round() as u64;
                Some(landed)
            }
            Err(e) => {
                log::warn!("seek failed: {e}");
                // Position is unchanged; the reader never moved, so report no
                // landing and let the caller leave the clock where it was.
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// An engine wired over synthetic paths and a throwaway ring, no audio
    /// device and no decode thread. Enough to drive the pure queue-edit math:
    /// order, cursor, and the runahead detection. `n` context entries with
    /// stable ids 0..n.
    fn test_engine(n: usize) -> Engine {
        engine_over((0..n).map(|i| PathBuf::from(format!("t{i}"))).collect())
    }

    /// The same over paths the caller picked, for the tests that open real
    /// files instead of driving the queue math over synthetic ones.
    fn engine_over(paths: Vec<PathBuf>) -> Engine {
        let shared = Arc::new(Shared::new(paths.len()));
        let (producer, _consumer) = rtrb::RingBuffer::<f32>::new(16);
        let (_tx, rx) = mpsc::channel::<Cmd>();
        Engine::new(
            StartQueue {
                paths,
                ..StartQueue::default()
            },
            shared,
            producer,
            48000,
            rx,
        )
    }

    /// A directory of fixture files that clears itself when the test ends.
    /// The path is unique per call, so the suite's threads never share one.
    struct Fixtures(PathBuf);

    impl Drop for Fixtures {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Fixtures {
        fn new(name: &str) -> Fixtures {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("rox-playback-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("fixture directory");
            Fixtures(dir)
        }

        /// A real decodable file, for the tests that need a Source rather
        /// than numbers: 16-bit stereo PCM at 48 kHz, a quiet tone so the
        /// samples aren't all zero and the container states its length.
        fn wav(&self, name: &str, secs: f64) -> PathBuf {
            let rate = 48_000u32;
            let frames = (secs * rate as f64) as u32;
            let data_len = frames * 4;
            let mut out: Vec<u8> = Vec::with_capacity(44 + data_len as usize);
            out.extend_from_slice(b"RIFF");
            out.extend_from_slice(&(36 + data_len).to_le_bytes());
            out.extend_from_slice(b"WAVEfmt ");
            out.extend_from_slice(&16u32.to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes()); // PCM
            out.extend_from_slice(&2u16.to_le_bytes()); // stereo
            out.extend_from_slice(&rate.to_le_bytes());
            out.extend_from_slice(&(rate * 4).to_le_bytes()); // byte rate
            out.extend_from_slice(&4u16.to_le_bytes()); // block align
            out.extend_from_slice(&16u16.to_le_bytes()); // bits
            out.extend_from_slice(b"data");
            out.extend_from_slice(&data_len.to_le_bytes());
            for i in 0..frames {
                let t = i as f64 * 440.0 * std::f64::consts::TAU / rate as f64;
                let s = (t.sin() * 8000.0) as i16;
                out.extend_from_slice(&s.to_le_bytes());
                out.extend_from_slice(&s.to_le_bytes());
            }
            let path = self.0.join(name);
            std::fs::write(&path, out).expect("writing the fixture");
            path
        }

        /// A path in the fixture directory with nothing behind it.
        fn missing(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    /// Put the engine where a skip starts from: a second into the first
    /// track, playing, with no backend to ack the flush so the cut doesn't
    /// sit out its deadline.
    fn ready_to_skip(e: &mut Engine, consumed: u64) -> Option<Source> {
        let source = e.open_at(0);
        assert!(source.is_some(), "the fixture opens");
        e.shared.playing.store(true, Ordering::Relaxed);
        e.shared.frames_consumed.store(consumed, Ordering::Relaxed);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);
        source
    }

    /// Point the audible clock at pool index `track`, so `audible_pos` resolves
    /// there instead of falling back to the decode cursor. Lets a test set up
    /// the runahead window where the cursor leads the audible track.
    fn set_audible(engine: &Engine, track: usize) {
        engine.shared.frames_consumed.store(10, Ordering::Relaxed);
        let mut segments = engine.shared.segments.lock().unwrap();
        segments.clear();
        segments.push(Segment {
            at_frame: 0,
            track,
            track_frame: 0,
        });
    }

    #[test]
    fn a_faded_in_track_is_already_playing_when_the_clock_flips() {
        let mut engine = test_engine(2);
        engine.pushed_playable = 100_000;
        let info = TrackInfo {
            name: "t1".into(),
            duration_secs: Some(200.0),
            num_frames: Some(9_600_000),
            sample_rate: 48_000,
            channels: 2,
        };
        // A four second window opening at 100_000, so the clock flips to the
        // incoming track at the midpoint, two seconds later.
        engine.adopt(1, info, 96_000);
        let flip = 100_000 + 96_000;
        // The incoming track has been audible since the window opened, so at
        // the flip it's two seconds in rather than at its start.
        assert_eq!(engine.shared.position_at(flip, 48_000), Some((1, 2.0)));
        // And it keeps step with the audio from there.
        assert_eq!(
            engine.shared.position_at(flip + 48_000, 48_000),
            Some((1, 3.0))
        );
    }

    #[test]
    fn prune_keeps_newest_reached_and_all_future() {
        let mut segments = vec![
            Segment {
                at_frame: 0,
                track: 0,
                track_frame: 0,
            },
            Segment {
                at_frame: 100,
                track: 1,
                track_frame: 0,
            },
            Segment {
                at_frame: 200,
                track: 2,
                track_frame: 0,
            },
            Segment {
                at_frame: 300,
                track: 3,
                track_frame: 0,
            },
        ];
        // Consumed sits between segment 1 and 2: drop segment 0, keep 1 (the
        // newest already reached) plus the two future ones.
        prune_segments(&mut segments, 150);
        let ats: Vec<u64> = segments.iter().map(|s| s.at_frame).collect();
        assert_eq!(ats, vec![100, 200, 300]);
    }

    #[test]
    fn prune_before_any_segment_keeps_all() {
        let mut segments = vec![
            Segment {
                at_frame: 100,
                track: 0,
                track_frame: 0,
            },
            Segment {
                at_frame: 200,
                track: 1,
                track_frame: 0,
            },
        ];
        // Nothing reached yet, so there's no cutoff and every segment stays.
        prune_segments(&mut segments, 50);
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn remove_non_runahead_entry_shifts_cursor_no_flush() {
        let mut e = test_engine(5);
        // Audible and decode cursor both on track 0; remove a later entry.
        set_audible(&e, 0);
        e.pos = 0;
        let removed_runahead = e.remove(3);
        assert!(!removed_runahead, "a future entry is not the runahead");
        assert_eq!(e.pos, 0, "cursor before the removal is unaffected");
        assert_eq!(e.order.len(), 4);
    }

    #[test]
    fn remove_audible_entry_is_refused() {
        let mut e = test_engine(5);
        set_audible(&e, 2);
        e.pos = 3; // decode cursor ran ahead of the audible track
                   // id 2 is the audible track; removing it must be refused.
        let removed_runahead = e.remove(2);
        assert!(!removed_runahead);
        assert_eq!(e.order.len(), 5, "audible entry stays");
    }

    #[test]
    fn remove_runahead_reports_and_reanchors() {
        let mut e = test_engine(5);
        // Audible on track 2, decode cursor pre-rolled to track 3.
        set_audible(&e, 2);
        e.pos = 3;
        // id 3 is the pre-decoded next track the open source holds.
        let removed_runahead = e.remove(3);
        assert!(
            removed_runahead,
            "the pre-decoded next track is the runahead"
        );
        // Cursor re-anchors down onto the audible entry so the caller's reopen
        // lands on the right next track.
        assert_eq!(e.pos, 2);
        assert_eq!(e.order.len(), 4);
    }

    #[test]
    fn remove_many_reports_runahead_when_cursor_dropped() {
        let mut e = test_engine(6);
        set_audible(&e, 2);
        e.pos = 3;
        // Drop the runahead (id 3) plus an unrelated later entry.
        let removed_runahead = e.remove_many(&[3, 5]);
        assert!(removed_runahead);
        // Cursor re-anchors to the audible entry (id 2) since its own entry went.
        assert_eq!(e.order[e.pos].id, 2);
    }

    #[test]
    fn remove_many_no_runahead_when_cursor_kept() {
        let mut e = test_engine(6);
        set_audible(&e, 2);
        e.pos = 3;
        // Drop only later entries, leave the runahead (id 3) in place.
        let removed_runahead = e.remove_many(&[4, 5]);
        assert!(!removed_runahead);
        // Cursor still on its own entry, re-found by id.
        assert_eq!(e.order[e.pos].id, 3);
    }

    #[test]
    fn insert_carries_groups_into_pool_and_snapshot() {
        let mut e = test_engine(2);
        // Splice two grouped tracks and one ungrouped behind the head.
        let at = e.insert(
            Some(0),
            vec!["a".into(), "b".into(), "c".into()],
            vec![Some(7), Some(7), None],
            Vec::new(),
            true,
        );
        assert_eq!(at, Some(1));
        let snap = e.shared.queue_snapshot();
        let groups: Vec<Option<u64>> = snap.entries.iter().map(|en| en.group).collect();
        // Seed entries came without groups; the spliced ones keep theirs in
        // splice order.
        assert_eq!(groups, vec![None, Some(7), Some(7), None, None]);
    }

    #[test]
    fn insert_pads_missing_groups_with_none() {
        let mut e = test_engine(1);
        // Groups and gains vecs shorter than paths pad rather than panic.
        e.insert(
            None,
            vec!["a".into(), "b".into()],
            vec![Some(3)],
            vec![gain::ReplayGain {
                track_db: Some(-6.0),
                ..gain::ReplayGain::default()
            }],
            false,
        );
        let snap = e.shared.queue_snapshot();
        assert_eq!(snap.entries[1].group, Some(3));
        assert_eq!(snap.entries[2].group, None);
        assert_eq!(e.gains[1].track_db, Some(-6.0));
        assert_eq!(e.gains[2], gain::ReplayGain::default());
    }

    /// Give pool entries their album groups, the way the player resolves
    /// them at insert time.
    fn set_groups(engine: &mut Engine, groups: &[Option<u64>]) {
        engine.groups = groups.to_vec();
        engine.groups.resize(engine.queue.len(), None);
    }

    #[test]
    fn album_contiguous_boundary_keeps_its_splice() {
        let mut e = test_engine(3);
        // Two tracks of one album, then something else.
        set_groups(&mut e, &[Some(1), Some(1), Some(2)]);
        assert!(
            !e.fades_between(0, 1),
            "same album, same splice gapless always made"
        );
        assert!(e.fades_between(1, 2), "a different album is a cut");
    }

    #[test]
    fn ungrouped_boundaries_fade() {
        let mut e = test_engine(3);
        // Loose files: nothing says these belong together.
        set_groups(&mut e, &[None, None, Some(1)]);
        assert!(e.fades_between(0, 1));
        assert!(e.fades_between(1, 2), "one side ungrouped still fades");
    }

    #[test]
    fn fading_albums_takes_the_record_s_own_splices_too() {
        let mut e = test_engine(2);
        set_groups(&mut e, &[Some(1), Some(1)]);
        assert!(!e.fades_between(0, 1), "off by default, the splice stands");
        e.fade_albums = true;
        assert!(e.fades_between(0, 1));
    }

    #[test]
    fn repeat_one_never_fades_into_itself() {
        let mut e = test_engine(1);
        set_groups(&mut e, &[None]);
        e.loop_mode = LoopMode::One;
        assert_eq!(e.next_pos(), Some(0));
        assert!(!e.fades_between(0, 0));
        // Not even with every other boundary fading.
        e.fade_albums = true;
        assert!(!e.fades_between(0, 0));
    }

    #[test]
    fn next_pos_follows_the_loop_mode() {
        let mut e = test_engine(3);
        e.pos = 2;
        assert_eq!(e.next_pos(), None, "played out with looping off");
        e.loop_mode = LoopMode::All;
        assert_eq!(e.next_pos(), Some(0), "repeat-all wraps to the top");
        e.loop_mode = LoopMode::One;
        assert_eq!(e.next_pos(), Some(2), "repeat-one stays put");
        e.loop_mode = LoopMode::Off;
        e.pos = 0;
        assert_eq!(e.next_pos(), Some(1));
    }

    #[test]
    fn fade_window_never_takes_more_than_half_a_track() {
        let mut e = test_engine(2);
        e.fade_secs = 8.0;
        // A 4-second track at 48 kHz: half of it, not the whole 8.
        assert_eq!(e.fade_window(Some(4 * 48_000)), 2 * 48_000);
        // A long one takes the setting as it stands.
        assert_eq!(e.fade_window(Some(300 * 48_000)), 8 * 48_000);
    }

    #[test]
    fn zero_seconds_disables_the_fade_entirely() {
        let mut e = test_engine(2);
        set_groups(&mut e, &[None, None]);
        e.fade_secs = 0.0;
        // Sitting right on the end of the track, which is where a window
        // would open if there were one.
        assert!(!e.window_open(Some(100), Some(0)));
    }

    #[test]
    fn fade_opens_inside_the_window_and_not_before() {
        let mut e = test_engine(2);
        set_groups(&mut e, &[None, None]);
        e.fade_secs = 2.0;
        let total = Some(60 * 48_000);
        // Three seconds out: too early.
        assert!(!e.window_open(total, Some(3 * 48_000)));
        // One second out: inside the two-second window.
        assert!(e.window_open(total, Some(48_000)));
    }

    #[test]
    fn fade_stays_shut_for_album_tracks_and_armed_stops() {
        let mut e = test_engine(2);
        e.fade_secs = 2.0;
        let (total, left) = (Some(60 * 48_000), Some(48_000));
        // Same album: the splice stands.
        set_groups(&mut e, &[Some(9), Some(9)]);
        assert!(!e.window_open(total, left));
        // Different albums, but the session stops at this boundary.
        set_groups(&mut e, &[Some(9), Some(4)]);
        assert!(e.window_open(total, left));
        e.stop_after = true;
        assert!(!e.window_open(total, left));
    }

    #[test]
    fn a_track_of_unknown_length_never_opens_a_window() {
        let mut e = test_engine(2);
        set_groups(&mut e, &[None, None]);
        e.fade_secs = 2.0;
        assert!(!e.window_open(None, None));
    }

    #[test]
    fn a_skip_that_lands_publishes_its_fade() {
        let fx = Fixtures::new("skip-lands");
        let mut e = engine_over(vec![fx.wav("a.wav", 4.0), fx.wav("b.wav", 4.0)]);
        e.fade_secs = 4.0;
        let source = ready_to_skip(&mut e, 48_000);

        let after = e.skip_to(source, 1, false);
        assert!(after.is_some(), "the second fixture opens");
        assert!(e.fade.is_some(), "the track left is under the new one");
        // Half the track is the ceiling, and the fixture is four seconds
        // long, so a four second setting fades for two.
        assert_eq!(e.shared.fade_len.load(Ordering::Acquire), 96_000);
        let (progress, back) = e.shared.crossfade().expect("the transport shows it");
        assert_eq!(progress, 0.0, "the cut landed right on the window");
        assert!(!back);
    }

    #[test]
    fn a_skip_to_a_dead_end_leaves_no_fade_behind() {
        let fx = Fixtures::new("skip-dead-end");
        let mut e = engine_over(vec![fx.wav("a.wav", 4.0), fx.missing("gone.wav")]);
        e.fade_secs = 4.0;
        let source = ready_to_skip(&mut e, 48_000);

        // Nothing past the skip target opens, so nothing drives a mix.
        let after = e.skip_to(source, 1, false);
        assert!(after.is_none());
        assert!(e.fade.is_none());
        // The clock is frozen at the cut with no source to move it, so a
        // fade published here would sit on the transport for good.
        assert_eq!(e.shared.fade_len.load(Ordering::Acquire), 0);
        assert!(e.shared.crossfade().is_none());
    }

    #[test]
    fn a_boundary_with_nothing_left_to_fade_splices_instead() {
        let fx = Fixtures::new("boundary-empty");
        let mut e = engine_over(vec![fx.wav("a.wav", 4.0), fx.wav("b.wav", 4.0)]);
        e.fade_secs = 4.0;
        let mut src = e.open_at(0).expect("the fixture opens");
        // The container says the track is over while the decoder still has
        // frames: a seek that landed on the claimed end, or a container
        // under-claiming its own length.
        src.pos_frames = src.total_frames.expect("wav states its length");
        assert_eq!(src.remaining(), Some(0));

        let after = e.start_boundary_fade(Some(src));
        assert!(after.is_some(), "the old source drives on");
        assert!(e.fade.is_none(), "no one-frame pseudo fade");
        assert_eq!(e.shared.fade_len.load(Ordering::Acquire), 0);
        assert_eq!(e.pos, 0, "the next track wasn't opened early");
    }

    #[test]
    fn the_wind_back_discards_flush_drift_and_a_short_landing() {
        // Nothing owed and the seek landed where it aimed: the tail starts
        // on the next sample.
        assert_eq!(skip_fade_discard(0, 0, 48_000), Some(0));
        // Both together: 25 ms went out during the cut, and the seek landed
        // a tenth of a second short of it.
        assert_eq!(skip_fade_discard(1_200, 4_800, 48_000), Some(6_000));
        // A coarse seek that overshot can't be undone by decoding forward,
        // so the tail starts where it landed rather than going negative.
        assert_eq!(skip_fade_discard(100, -4_800, 48_000), Some(0));
        // Seconds short would put a long decode inside the cut; give the
        // fade up instead.
        assert_eq!(skip_fade_discard(0, 96_000, 48_000), None);
    }

    #[test]
    fn a_track_shorter_than_the_fade_closes_it_at_its_own_end() {
        let fx = Fixtures::new("short-track");
        let path = fx.wav("a.wav", 4.0);
        let mut e = engine_over(vec![path.clone()]);
        let (src, _) = Source::open(&path, 48_000).expect("the fixture opens");
        // A twelve second window five seconds in, which is where a five
        // second track would hit its own EOF.
        let mut fade = Fade::new(src, 12 * 48_000);
        fade.done = 5 * 48_000;
        e.fade = Some(fade);

        e.close_fade_fast();
        let fade = e.fade.as_ref().expect("still mixing, just not for long");
        // 20 ms of ramp left, rather than seven more seconds of the old
        // track playing under whatever opens next.
        assert_eq!(fade.len, 5 * 48_000 + 960);
    }

    #[test]
    fn dropping_an_unmixed_fade_takes_its_publish_with_it() {
        let fx = Fixtures::new("drop-fade");
        let path = fx.wav("a.wav", 4.0);
        let mut e = engine_over(vec![path.clone()]);
        let (src, _) = Source::open(&path, 48_000).expect("the fixture opens");
        e.fade = Some(Fade::new(src, 96_000));
        e.publish_fade(0, 96_000, false);

        e.drop_fade();
        assert!(e.fade.is_none());
        assert_eq!(e.shared.fade_len.load(Ordering::Acquire), 0);
        assert!(e.shared.crossfade().is_none());
    }

    #[test]
    fn a_nan_crossfade_setting_reads_as_off() {
        assert_eq!(crossfade_secs(f32::NAN), 0.0);
        assert_eq!(crossfade_secs(-3.0), 0.0);
        assert_eq!(crossfade_secs(4.5), 4.5);
        assert_eq!(crossfade_secs(90.0), CROSSFADE_MAX_SECS);
        // Off is off all the way down: the window never opens, so no skip
        // pays for a wind-back it can't use.
        let mut e = test_engine(2);
        set_groups(&mut e, &[None, None]);
        e.fade_secs = crossfade_secs(f32::NAN);
        assert!(!e.window_open(Some(100), Some(0)));
    }

    #[test]
    fn remove_many_keeps_audible_even_if_named() {
        let mut e = test_engine(5);
        set_audible(&e, 1);
        e.pos = 1;
        // Name the audible entry in the drop set; it must survive.
        let _ = e.remove_many(&[0, 1, 2]);
        assert!(
            e.order.iter().any(|entry| entry.id == 1),
            "audible entry kept"
        );
    }
}
