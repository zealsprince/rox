//! The decode thread: Symphonia decode, gapless track boundary, seek, and the
//! producer side of the sample ring. Allowed to allocate, lock, and block; the
//! RT line is the ring in output.rs.
//!
//! Gapless (ADR 3): one long-lived stream, with the decoder swapped at EOF so
//! the next track's first frame follows the last. Symphonia 0.6 applies
//! encoder delay and padding as packet trims ([`count_frames`] checks it),
//! except Opus's pre-skip, which [`crate::opus`] drops itself.
//!
//! Pause is the same for files and stations: the callback stops consuming and
//! the decode loop parks on a full ring. A station stays connected, taping
//! into [`crate::tape`], so Play resumes where it stopped and
//! [`Cmd::SeekLive`] moves the cursor through the tape by rebuilding the
//! decoder over it. A seek never crosses a reconnect gap.
//!
//! A station still hangs up in two cases: a paused restore at launch never
//! dials ([`Engine::open_start`]), and a pause past
//! [`LIVE_IDLE_HANGUP_SECS`] lets the socket go. `hung_up` holding a value
//! with no open source is the whole state between `hang_up` and `rejoin`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::time::Duration as StdDuration;
use std::time::Instant;

use rox_library::cue::Span;
use rox_library::locator::Locator;
use rox_library::peaks::{PeakBin, PeakLanes};
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
use crate::http::StationInfo;
use crate::icy::TitleSink;
use crate::latency;
use crate::resample::Resampler;
use crate::shared::{
    QueueEntry, QueueSnapshot, Segment, Shared, StreamSink, StreamState, TrackInfo,
};
use crate::tape::Tape;

pub enum Cmd {
    TogglePause,
    Seek(f64),
    /// Play a live stream this many seconds behind its live edge, zero being
    /// live, by rebuilding the decoder over the tape: a flush and nothing over
    /// the wire. Held to the tape and the live side of any gap. Ignored for
    /// anything but the audible live entry.
    SeekLive(f64),
    /// Play this many seconds through the pause, for a step taken while paused.
    /// Sent right after the Seek, so the blip starts on the refill's first sample.
    Audition(f64),
    Next,
    Prev,
    Volume(f32),
    SetLoop(LoopMode),
    SetShuffle(bool),
    /// Reorder the upcoming portion into this entry order, for Similar shuffle.
    /// Unknown ids are ignored; unnamed entries keep their order behind the
    /// named ones.
    OrderTail(Vec<u64>),
    /// Stop-after-current. The ring drains so the last samples play, then the
    /// session pauses with the next track cued. Sticky until cleared.
    SetStopAfter(bool),
    /// Loop a section of the audible track (track-relative seconds), or clear it.
    /// Setting one costs a seek's flush; every wrap after is a gapless splice.
    SetAbLoop(Option<(f64, f64)>),
    /// Splice tracks in after entry `after`, or at the end. `explicit` marks them
    /// user-queued rather than context.
    Insert {
        after: Option<u64>,
        /// The engine never looks inside one; it hands it to [`Source::open`].
        locators: Vec<Locator>,
        /// Parallel to `locators` (ADR 17); the engine only compares them. Short pads
        /// with None.
        groups: Vec<Option<u64>>,
        /// Parallel the same way. Short pads with untagged.
        gains: Vec<gain::ReplayGain>,
        /// Parallel the same way. A cue track is a span of one image file; None plays
        /// the whole file.
        spans: Vec<Option<Span>>,
        explicit: bool,
        /// Jump to the first of the batch and play it now.
        and_play: bool,
        /// With `and_play`, open the first track this far in. A separate Seek would
        /// either cancel the jump (one flush per drain) or play the head first.
        start_secs: Option<f64>,
    },
    /// Removing the playing entry is ignored.
    Remove {
        id: u64,
    },
    /// One sweep and one publish, so a big clear is O(n). The playing entry is
    /// kept even if named.
    RemoveMany {
        ids: Vec<u64>,
    },
    /// To just after `after`, or the front.
    Move {
        id: u64,
        after: Option<u64>,
    },
    Jump {
        id: u64,
    },
    /// Append a chain node (ADR 19). Only structural edits come this way; knobs
    /// are atomics. Reset to the live rate on arrival.
    ChainPush(Box<dyn Node>),
    /// Crossfade length in seconds (zero disables) and whether same-album
    /// boundaries fade.
    SetCrossfade {
        secs: f32,
        /// Fade album-contiguous boundaries too. Off by default: an album that runs
        /// track into track was mastered that way.
        albums: bool,
    },
    /// Seconds of live stream to keep. Re-caps the station on air and applies to
    /// later ones. Floored at [`LIVE_BUFFER_MIN_SECS`] like the start.
    SetLiveBuffer(u32),
    /// How tagged loudness is levelled (ADR 19). Applied to every source in hand,
    /// so a switch is heard on the playing track.
    SetGainRule(gain::GainRule),
    /// A fresh sample ring after a device fault, at the same rate. Everything
    /// upstream (decoder, socket, tape, idle clock) carries on. A different rate
    /// can't come this way: the player rebuilds the session instead.
    SwapOutput(Producer<f32>),
    Quit,
}

/// Past this the overlap reads as both playing at once. The UI slider stops here.
pub const CROSSFADE_MAX_SECS: f32 = 12.0;

/// Pause length before a station's connection is let go: past half an hour
/// the tape has rolled over and the resume is a rejoin anyway.
pub const LIVE_IDLE_HANGUP_SECS: u64 = 1800;

/// Refresh interval for the song marks' sliding distances: the pump's clock,
/// so they don't step behind a smooth playhead.
const MARKS_REFRESH: StdDuration = StdDuration::from_millis(16);

/// Floor for sessions that never passed a length, like tests.
const LIVE_BUFFER_MIN_SECS: u32 = 30;

/// See [`Source::inside_track`].
const SEEK_END_MARGIN_SECS: f64 = 0.1;

/// Decode thread only.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    #[default]
    Off,
    All,
    /// Repeat the current track at EOF. Skips still move.
    One,
}

struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    /// Path or URL for the panic guard's log line, formatted once.
    origin: String,
    /// Kept so a station rebuilt over its tape keeps its name.
    name: String,
    /// A read or decode panicked: reader and decoder are never touched again,
    /// and the source reports end of stream.
    poisoned: bool,
    track_id: u32,
    time_base: Option<TimeBase>,
    device_rate: u32,
    resampler: Resampler,
    /// Reused across packets.
    scratch: Vec<f32>,
    /// Kept to recompute the gain when the rule changes.
    rg: gain::ReplayGain,
    /// Per source (ADR 19), because a fade has two tracks live.
    gain: f32,
    /// Track position in device-rate frames, reset to the landing on a seek. The
    /// fade window counts off it.
    pos_frames: u64,
    /// Device-rate length, the span's for a spanned source. None never opens a
    /// fade window.
    total_frames: Option<u64>,
    /// None plays the whole file.
    span: Option<SpanFrames>,
    /// File-clock position, for cutting a span's end before resampling.
    src_frame: u64,
    /// For the last open-latency line: when audio first exists. Remote only.
    opened_at: Option<Instant>,
    /// The station's tape, the only handle back down once symphonia owns the reader.
    tape: Option<Arc<Tape>>,
}

/// A span on the file's frame clock, where the boundary has to be honored.
/// Integer math, so consecutive spans share a frame and splice exactly.
#[derive(Clone, Copy)]
struct SpanFrames {
    start: u64,
    /// None runs to the file's end.
    end: Option<u64>,
}

/// Truncating, so every caller gets the same frame for a shared boundary.
fn ms_frames(ms: u32, rate: u32) -> u64 {
    (ms as u64 * rate as u64) / 1000
}

/// A stable id and an index into the append-only pool, which stays valid
/// however the order changes.
struct OrderEntry {
    id: u64,
    idx: usize,
    /// Play Next / Add to Queue. The queue widgets list only these.
    explicit: bool,
}

/// Ends on the source's device-rate clock, pinned to the pool index so a
/// stale loop can't grab another track.
#[derive(Clone, Copy)]
struct AbLoop {
    track: usize,
    a: u64,
    b: u64,
}

/// Tighter than this the wrap lands inside the packet it just decoded. Public
/// so the player refuses the same set before the round trip.
pub const AB_MIN_SECS: f64 = 0.25;

pub struct Engine {
    /// Append-only: nothing is ever removed, so `Segment.track` stays valid.
    queue: Vec<Locator>,
    /// Parallel to `queue` (ADR 17), supplied by the library, only compared here.
    groups: Vec<Option<u64>>,
    gains: Vec<gain::ReplayGain>,
    /// Parallel the same way; None is the whole file.
    spans: Vec<Option<Span>>,
    idx: usize,
    /// The play order: `order[pos]` is the decoding entry. Edited in place.
    order: Vec<OrderEntry>,
    /// Kept in sync with `idx` on every open.
    pos: usize,
    /// Where the first open goes, for starting partway into a context.
    start: usize,
    next_id: u64,
    shared: Arc<Shared>,
    producer: Producer<f32>,
    device_rate: u32,
    rx: Receiver<Cmd>,
    loop_mode: LoopMode,
    /// Sticky until cleared.
    stop_after: bool,
    /// Set when an armed stop skipped the EOF open; consumed once the ring drains.
    stop_pending: bool,
    /// A skip clears it.
    ab: Option<AbLoop>,
    /// On the frames_consumed clock; resynced after each flush.
    pushed_playable: u64,
    pending: Vec<f32>,
    pending_pos: usize,
    /// Runs just before the ring (ADR 19). Empty is the bypass rule.
    chain: Chain,
    /// Off by default: unity everywhere.
    rule: gain::GainRule,
    /// Zero is off: every boundary is the gapless splice.
    fade_secs: f32,
    fade_albums: bool,
    /// The outgoing track, mixed under the open source until the window closes.
    fade: Option<Fade>,
    /// Set once per track so a next file that won't open isn't retried every chunk.
    fade_armed: bool,
    /// Set while a pause has hung up on a station: the elapsed clock at hang-up,
    /// in device-rate frames. `Some` with no open source is the whole hung-up
    /// state; the entry to rejoin comes off the output clock, so queue edits
    /// during the pause just work.
    hung_up: Option<u64>,
    live_buffer_secs: u32,
    /// See [`Engine::publish_marks`].
    marks_rev: u64,
    marks_at: Instant,
    /// When a station's pause started, for the idle cap.
    paused_since: Option<Instant>,
}

/// A crossfade in flight (ADR 19): the old source, decoded alongside the new
/// one and mixed under it, so the ring keeps one producer.
struct Fade {
    src: Source,
    /// Decoded ahead: the two sources' packet boundaries never line up.
    buf: Vec<f32>,
    read: usize,
    /// Exactly the incoming chunk's worth, reused.
    take: Vec<f32>,
    done: u64,
    len: u64,
    /// Past this the mix reads silence.
    ended: bool,
}

/// A skipped-from track wound back, held across the flush until it's installed.
struct Wound {
    src: Source,
    /// The output frame the wind-back aimed at.
    at: u64,
    /// How far short of that the seek landed, in frames; negative if it overshot.
    short: i64,
}

/// The playing context for a new engine. The parallel vecs pad out with
/// false, None, untagged, and None.
#[derive(Default)]
pub struct StartQueue {
    pub locators: Vec<Locator>,
    pub start: usize,
    pub explicit: Vec<bool>,
    pub groups: Vec<Option<u64>>,
    pub gains: Vec<gain::ReplayGain>,
    /// None plays the whole file.
    pub spans: Vec<Option<Span>>,
    /// Floored at [`LIVE_BUFFER_MIN_SECS`]. In the start rather than a command
    /// because the first open happens before the channel is read.
    pub live_buffer_secs: u32,
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
            locators: queue,
            start,
            explicit,
            groups,
            gains,
            spans,
            live_buffer_secs,
        } = queue;
        // A fresh context passes an empty `explicit`; a launch restore passes the
        // saved flags. Short vecs pad.
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
        let mut spans = spans;
        spans.resize(queue.len(), None);
        Engine {
            order,
            groups,
            gains,
            spans,
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
            ab: None,
            pushed_playable: 0,
            pending: Vec::new(),
            pending_pos: 0,
            chain: Chain::new(),
            rule: gain::GainRule::default(),
            fade_secs: 0.0,
            fade_albums: false,
            fade: None,
            fade_armed: false,
            hung_up: None,
            live_buffer_secs: live_buffer_secs.max(LIVE_BUFFER_MIN_SECS),
            marks_rev: 0,
            marks_at: Instant::now(),
            paused_since: None,
        }
    }

    /// Every transport command calls this, so a blip never outlives its position.
    fn cancel_audition(&self) {
        self.shared.audition_left.store(0, Ordering::Relaxed);
    }

    pub fn run(mut self) {
        // The chain learns the rate before any sample; it resets on flushes, never
        // at the gapless boundary.
        self.chain.reset(self.device_rate);
        self.publish_queue();
        let mut source = self.open_start();

        loop {
            // Clear before the drain, not after: the sender stores before it sends, so a
            // clear after could swallow a flag whose command is still queued.
            self.shared.interrupt.store(false, Ordering::Relaxed);

            // Commands first so pause and seek respond even with the ring full.
            let mut flush_to: Option<FlushAction> = None;
            // Running target, so two Nexts in one drain advance twice.
            let mut nav_pos: Option<usize> = None;
            // For the transport's fade sweep direction.
            let mut nav_back = false;
            // Only an Insert that plays now sets this.
            let mut nav_at: Option<f64> = None;
            // A queue edit displaced the pre-decoded next track. Reopened after the
            // drain; the audible track is fully in the ring, so no flush.
            let mut reopen_runahead = false;
            // Paid after the flush lands. See the store.
            let mut resume = false;
            while let Ok(cmd) = self.rx.try_recv() {
                match cmd {
                    Cmd::TogglePause => {
                        // At the ended state the button means play again, through the nav path.
                        self.cancel_audition();
                        if source.is_none() && self.shared.ended.load(Ordering::Relaxed) {
                            self.shared.playing.store(true, Ordering::Relaxed);
                            nav_pos = Some(self.audible_pos());
                            flush_to = None;
                        } else if source.is_none() && self.hung_up.is_some() {
                            // The rejoin sets playing itself.
                            source = self.rejoin();
                        } else {
                            // Same for files and stations: a station keeps taping through the pause, so
                            // the ring and pending hold exactly what the resume should play.
                            let now = self.shared.playing.load(Ordering::Relaxed);
                            self.shared.playing.store(!now, Ordering::Relaxed);
                        }
                    }
                    Cmd::Audition(secs) => {
                        // Only while paused with something open.
                        if source.is_some() && !self.shared.playing.load(Ordering::Relaxed) {
                            let frames = (secs.max(0.0) * self.device_rate as f64).round() as u64;
                            self.shared
                                .audition_left
                                .store(frames.max(1), Ordering::Relaxed);
                        }
                    }
                    Cmd::Volume(v) => {
                        let v = v.clamp(0.0, 2.0);
                        self.shared
                            .volume_bits
                            .store(v.to_bits(), Ordering::Relaxed);
                    }
                    Cmd::Seek(secs) => {
                        // A blip belongs to the position this seek leaves; a following Audition re-arms it.
                        self.cancel_audition();
                        // Dropped for a station: its tape isn't seekable, and symphonia would scan
                        // forward past the live edge. `SeekLive` is a station's seek. Dropped here,
                        // not at the flush, since an empty flush still cuts the ring and ends the
                        // hung-up state.
                        flush_to = (!self.live_at(self.audible_pos()))
                            .then_some(FlushAction::Seek(secs.max(0.0)));
                        nav_pos = None;
                        nav_at = None;
                    }
                    Cmd::SeekLive(behind) => {
                        self.cancel_audition();
                        flush_to = Some(FlushAction::SeekLive(behind.max(0.0)));
                        nav_pos = None;
                        nav_at = None;
                    }
                    Cmd::Next => {
                        self.cancel_audition();
                        // Off the audible track, not the decode cursor, which may already be a track
                        // ahead: Next would skip two.
                        let from = nav_pos.unwrap_or_else(|| self.audible_pos());
                        if from + 1 < self.order.len() {
                            nav_pos = Some(from + 1);
                        } else if self.loop_mode == LoopMode::All && !self.order.is_empty() {
                            nav_pos = Some(0);
                        }
                        nav_back = false;
                        nav_at = None;
                        flush_to = None;
                    }
                    Cmd::Prev => {
                        self.cancel_audition();
                        let from = nav_pos.unwrap_or_else(|| self.audible_pos());
                        let target = if from == 0 && self.loop_mode == LoopMode::All {
                            self.order.len().saturating_sub(1)
                        } else {
                            from.saturating_sub(1)
                        };
                        nav_pos = Some(target);
                        nav_back = true;
                        nav_at = None;
                        flush_to = None;
                    }
                    Cmd::SetLoop(mode) => {
                        self.loop_mode = mode;
                        // From the ended state a wrapping mode reopens through the nav path.
                        if source.is_none() {
                            nav_pos = match mode {
                                LoopMode::One => Some(self.pos),
                                LoopMode::All if !self.order.is_empty() => Some(0),
                                _ => None,
                            };
                        }
                    }
                    Cmd::SetShuffle(on) => reopen_runahead |= self.set_shuffle(on),
                    Cmd::OrderTail(ids) => reopen_runahead |= self.order_tail(&ids),
                    Cmd::SetStopAfter(on) => self.stop_after = on,
                    Cmd::SetAbLoop(marks) => {
                        // Setting a loop must be heard now, and the ring holds up to half a second
                        // past B, so it pays one seek: straight to `seek_to`, since
                        // `FlushAction::Seek` is keyed on seconds.
                        self.ab = None;
                        match marks {
                            Some((a, b)) if b - a >= AB_MIN_SECS => {
                                source = self.seek_to(source.take(), a.max(0.0));
                                // A is where the seek actually landed (a packet early for an unindexed CBR
                                // MP3). A failed seek leaves the decoder past B: no section beats one whose
                                // ends are the wrong way round.
                                let rate = self.device_rate as f64;
                                let b = (b * rate).round() as u64;
                                let floor = (AB_MIN_SECS * rate) as u64;
                                if let Some(src) = source.as_ref()
                                    && src.pos_frames + floor <= b
                                {
                                    self.ab = Some(AbLoop {
                                        track: self.idx,
                                        a: src.pos_frames,
                                        b,
                                    });
                                }
                            }
                            _ => {}
                        }
                        self.publish_ab();
                    }
                    Cmd::Insert {
                        after,
                        locators,
                        groups,
                        gains,
                        spans,
                        explicit,
                        and_play,
                        start_secs,
                    } => {
                        let at = self.insert(after, locators, groups, gains, spans, explicit);
                        // From the ended state, the first new entry opens through the nav path.
                        // Play now jumps the same way; Play Next and Add to Queue don't.
                        if and_play || source.is_none() {
                            nav_pos = at;
                            nav_at = start_secs.filter(|_| and_play);
                        }
                        // Play now means play, even from a pause. Owed, and paid below.
                        resume |= and_play;
                    }
                    Cmd::Remove { id } => reopen_runahead |= self.remove(id),
                    Cmd::RemoveMany { ids } => reopen_runahead |= self.remove_many(&ids),
                    Cmd::Move { id, after } => self.move_entry(id, after),
                    // Through the nav path, like a Next.
                    Cmd::Jump { id } => {
                        self.cancel_audition();
                        if let Some(p) = self.find(id) {
                            nav_pos = Some(p);
                        }
                        nav_at = None;
                        flush_to = None;
                    }
                    Cmd::ChainPush(node) => self.chain.push(node),
                    Cmd::SetCrossfade { secs, albums } => {
                        self.fade_secs = crossfade_secs(secs);
                        self.fade_albums = albums;
                    }
                    Cmd::SetLiveBuffer(secs) => {
                        let secs = secs.max(LIVE_BUFFER_MIN_SECS);
                        self.live_buffer_secs = secs;
                        // Re-cap the station on air directly; its tape was sized at connect.
                        if let Some(tape) = source.as_ref().and_then(|src| src.tape.as_ref()) {
                            tape.set_cap_secs(secs);
                        }
                    }
                    Cmd::SetGainRule(rule) => {
                        self.rule = rule;
                        // Relevel both sources in hand, each by its own tags.
                        if let Some(src) = source.as_mut() {
                            src.relevel(&rule);
                        }
                        if let Some(fade) = self.fade.as_mut() {
                            fade.src.relevel(&rule);
                        }
                    }
                    Cmd::SwapOutput(producer) => {
                        source = self.swap_output(producer, source.take());
                    }
                    Cmd::Quit => return,
                }
            }
            if let Some(p) = nav_pos {
                flush_to = Some(FlushAction::Track {
                    pos: p,
                    back: nav_back,
                    at: nav_at,
                });
            }

            // A flush is the audible hole, so everything possible happens before the
            // cut and only arithmetic after.
            let flushed = flush_to.is_some();
            if let Some(action) = flush_to {
                // Opening any track ends the hung-up state. A timeshift seek doesn't: it
                // only moves inside an open station.
                if !matches!(action, FlushAction::SeekLive(_)) {
                    self.hung_up = None;
                }
                match action {
                    FlushAction::Track { pos, back, at } => {
                        // Any move through the queue ends the loop. The natural advance never gets
                        // here while looping: the wrap takes EOF first.
                        self.clear_ab();
                        source = self.skip_to(source.take(), pos, back, at);
                    }
                    FlushAction::Seek(secs) => {
                        source = self.seek_to(source.take(), secs);
                    }
                    FlushAction::SeekLive(behind) => {
                        source = self.seek_live_to(source.take(), behind);
                    }
                }
            }

            // Pay the owed resume after the flush, not at the command: on a station the
            // open is a network round trip, and resuming early would play the paused
            // ring's leftovers out of nowhere first.
            if resume {
                self.shared.playing.store(true, Ordering::Relaxed);
            }

            if flushed {
                continue;
            }

            // The pre-decoded next track was removed or reordered away. Drop its source
            // and pending samples and reopen what's next now; the audible track is in
            // the ring, so no flush. Any of the stale track already in the ring still
            // plays: cutting the audible track mid-note would be worse. Automatic
            // reorders avoid this by never asking once the runahead has fed the ring.
            // Never while hung up, which would reconnect under a pause.
            if reopen_runahead && self.hung_up.is_none() {
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

            // Every pass, not per decoded chunk: a pause is when nothing decodes and
            // the tape still moves.
            source = self.idle_hangup(source);
            source = self.follow_tape(source);

            // Fill the ring; full means we're ahead, so sleep. Full is what the latency
            // hold allows (ADR 19): with an EQ editor open the 500 ms ring only fills
            // part way.
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

            // The boundary crossfade opens one fade length before the end (ADR 19).
            // Album-internal boundaries take the plain splice.
            if self.fade_due(source.as_ref()) {
                source = self.start_boundary_fade(source.take());
            }

            match source.as_mut() {
                Some(src) => {
                    let device_rate = self.device_rate;
                    let mut more = src.next_chunk(device_rate, &mut self.pending);
                    // The A-B wrap, before anything downstream: cut the overshoot past B and wind
                    // back to A, so the ring runs B straight into A. `!more` carries EOF, for a
                    // B at the very end.
                    if let Some(ab) = self.ab.filter(|ab| ab.track == self.idx)
                        && let Some(landed) = src.wrap_ab(ab.a, ab.b, !more, &mut self.pending)
                    {
                        // The wrap is at the end of what survived, so the readout flips to A on the
                        // frame the ear hears it.
                        let at = (self.pending.len() / 2) as u64;
                        self.register_segment_after(landed, at);
                        more = true;
                    }
                    // Mix the outgoing track under before the chain, so the ring gets one stream.
                    self.mix_fade();
                    // The chain is the last step before the ring (ADR 19), so its output rides
                    // flushes and the gapless boundary like any sample.
                    self.chain.process(&mut self.pending);
                    // The broadcast taps here (ADR 22), where each chunk passes once. Never blocks.
                    crate::broadcast::feed(&self.pending, device_rate);
                    if !more {
                        // Track shorter than its fade: ramp the outgoing tail out now.
                        self.close_fade_fast();
                        // EOF: swap the decoder under the live stream. No flush; this is the gapless
                        // boundary. An armed stop-after skips the open so the tail drains first.
                        source = if self.stop_after {
                            self.stop_pending = true;
                            None
                        } else {
                            self.next_pos().and_then(|p| self.open_at(p))
                        };
                    }
                }
                // Paused on a hung-up station. Without this arm the drained ring would read
                // as a played-out queue.
                None if self.hung_up.is_some() => {
                    std::thread::sleep(StdDuration::from_millis(20));
                }

                None => {
                    // No incoming track, so the fade is done, and its publish with it.
                    self.drop_fade();
                    // Queue exhausted or stopped: let the ring drain first.
                    if self.ring_drained() {
                        if self.stop_pending {
                            // Pause, then cue what EOF would have opened. With nothing to cue, fall
                            // through to ended, still paused.
                            if let Some(p) = self.land_stop() {
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

    /// The session's first open, the only one that can arrive already paused (a
    /// launch restore). A paused start on a station parks in the hung-up state
    /// instead of dialing, and the first Play rejoins. Everything else opens as
    /// usual so the restore shows a duration and name.
    fn open_start(&mut self) -> Option<Source> {
        let paused = !self.shared.playing.load(Ordering::Relaxed);
        if !paused || !self.live_at(self.start) {
            return self.open_at(self.start);
        }

        // Move the cursor anyway: the rejoin resolves off it. Only the part of
        // `adopt` that applies without a source.
        self.pos = self.start;
        self.idx = self.order[self.start].idx;
        self.hung_up = Some(0);

        // A segment at zero with no source, so the restored station still shows up
        // as loaded.
        self.register_segment(0.0);

        None
    }

    /// Falls forward through unreadable files. Registers the segment.
    fn open_at(&mut self, p: usize) -> Option<Source> {
        self.open_at_from(p, 0)
    }

    /// The segment is registered `after` frames late, and the track opens that
    /// far in to match. Only a crossfade passes nonzero: the clock, track-change
    /// notification and MPRIS flip at the fade's midpoint (ADR 19).
    fn open_at_from(&mut self, p: usize, after: u64) -> Option<Source> {
        let (src, at, info) = self.open_file_at(p)?;
        self.adopt(at, info, 0, after);
        Some(src)
    }

    /// Open without adopting: no cursor move, segment, or publish. Split out so a
    /// skip pays for the open while the old track still plays.
    fn open_file_at(&mut self, mut p: usize) -> Option<(Source, usize, TrackInfo)> {
        while p < self.order.len() {
            let i = self.order[p].idx;

            // Titles bind to this pool entry for the source's lifetime.
            let shared = Arc::clone(&self.shared);
            let on_title: TitleSink = Arc::new(move |title| shared.publish_title(i, title));

            // The stream-state sink binds the same way.
            let shared = Arc::clone(&self.shared);
            let on_stream: StreamSink = Arc::new(move |state| shared.publish_stream(i, state));

            // Only a remote open is slow enough to show.
            let remote = matches!(self.queue[i], Locator::Remote(_));
            if remote {
                self.shared.publish_stream(i, StreamState::Opening);
            }

            match Source::open_titled(
                &self.queue[i],
                self.device_rate,
                self.spans[i],
                on_title,
                on_stream,
                Arc::clone(&self.shared.interrupt),
                self.live_buffer_secs,
            ) {
                Ok((mut src, info, station)) => {
                    if remote {
                        self.shared.publish_stream(i, StreamState::Live);
                    }

                    // Published once per open; a reconnect has nothing new to say.
                    if let Some(station) = station.filter(|s| !s.is_empty()) {
                        self.shared.publish_station(i, station);
                    }

                    // Set at the open (ADR 19), so it changes exactly where the source does.
                    src.level(self.gains[i], &self.rule);
                    return Some((src, p, info));
                }
                Err(e) => {
                    // Skip to the next entry, and clear `Opening` so it doesn't stick.
                    if remote {
                        self.shared.publish_stream(i, StreamState::Dropped);
                        // Streams send the refusal up; a missing local file is obvious enough.
                        self.shared.publish_refusal(e.clone());
                    }

                    log::warn!("skipping {}: {e}", self.queue[i].label());
                    p += 1;
                }
            }
        }
        None
    }

    /// Take an opened source on: cursor, track info, and a segment `after` frames
    /// out. The first sample still plays at `pushed_playable`, so the segment's
    /// track frame is `start + after`; claiming `start` there would leave the
    /// clock half a fade behind the audio. `start` is where the source opened,
    /// nonzero for a play-from-bookmark.
    fn adopt(&mut self, p: usize, info: TrackInfo, start: u64, after: u64) {
        let i = self.order[p].idx;
        self.pos = p;
        self.idx = i;
        self.fade_armed = false;
        self.shared.tracks.lock().unwrap()[i] = Some(info);
        let at_frame = self.pushed_playable + after;
        let consumed = self.shared.frames_consumed.load(Ordering::Relaxed);
        let mut segments = self.shared.segments.lock().unwrap();
        segments.push(Segment {
            at_frame,
            track: i,
            track_frame: start + after,
        });
        prune_segments(&mut segments, consumed);
    }

    /// Asked of the locator: once symphonia owns the transport there's no way down.
    fn live_at(&self, p: usize) -> bool {
        self.order
            .get(p)
            .is_some_and(|e| matches!(&self.queue[e.idx], Locator::Remote(r) if r.live))
    }

    /// Move the audible station `behind` seconds back and re-sync there, zero
    /// being live. Rebuilds the decoder over the same tape and cuts the ring;
    /// nothing over the wire. The elapsed clock carries on (it counts the
    /// listen); the title follows the tape. Refused unless the station is what's
    /// audible, since the decode cursor can be a track ahead.
    fn seek_live_to(&mut self, source: Option<Source>, behind: f64) -> Option<Source> {
        let src = source?;
        let Some(tape) = src.tape.clone().filter(|_| self.pos == self.audible_pos()) else {
            return Some(src);
        };

        // Rebuilt while the old source still plays, so only the cut is silent.
        let was = tape.cursor();
        let at = tape.seek_target(behind);
        let hint = self.live_hint(self.idx);
        let (mut fresh, info) = match src.reopen_live(at, &hint) {
            Ok(rebuilt) => rebuilt,

            // A failed re-sync stays put and puts the cursor back.
            Err(e) => {
                log::warn!("live seek to {behind:.1}s behind failed: {e}");
                tape.restore_cursor(was);

                return Some(src);
            }
        };
        fresh.level(self.gains[self.idx], &self.rule);

        let elapsed = self.elapsed_frames();
        drop(src);
        self.flush_ring();
        self.adopt(self.pos, info, elapsed, 0);

        Some(fresh)
    }

    /// The stored hint, else the `Content-Type`'s; empty lets the probe sniff.
    fn live_hint(&self, idx: usize) -> String {
        if let Locator::Remote(remote) = &self.queue[idx]
            && !remote.hint.is_empty()
        {
            return remote.hint.clone();
        }

        self.shared
            .station_info(idx)
            .and_then(|info| crate::http::extension_for(&info.content_type))
            .unwrap_or_default()
            .to_string()
    }

    /// Publish where the audible station plays from, and re-sync a tape that
    /// rolled over the cursor. Rides the loop rather than decoded chunks, since
    /// the numbers move through a pause.
    fn follow_tape(&mut self, source: Option<Source>) -> Option<Source> {
        let Some(tape) = source.as_ref().and_then(|src| src.tape.clone()) else {
            self.shared.clear_shift();

            return source;
        };

        // Ring plus pending, taken off so the playhead follows the speakers.
        let ringed = self.producer.buffer().capacity() - self.producer.slots();
        let pending = self.pending.len() - self.pending_pos;
        tape.note_queued((ringed + pending) as f64 / 2.0 / self.device_rate as f64);

        let shift = tape.shift();
        self.shared.publish_shift(self.idx, shift);
        self.publish_marks(&tape);

        if tape.took_underrun() {
            log::info!("the pause outlasted the buffer, re-syncing at the back of it");

            return self.seek_live_to(source, shift.window_secs);
        }

        source
    }

    /// Publish song marks and gaps. Changes to the sets get a revision; the
    /// sliding distances ride a timer fast enough for a 60 fps strip. Gaps go
    /// with the marks so the two never draw out of step.
    fn publish_marks(&mut self, tape: &Arc<Tape>) {
        let rev = tape.marks_rev();
        let changed = rev != self.marks_rev;
        if !changed && self.marks_at.elapsed() < MARKS_REFRESH {
            return;
        }

        self.marks_rev = rev;
        self.marks_at = Instant::now();
        self.shared
            .publish_live_marks(tape.live_marks(), tape.live_gaps(), changed);
    }

    /// Hang up on a station paused past [`LIVE_IDLE_HANGUP_SECS`]: the tape has
    /// rolled over, so the resume rejoins live anyway.
    fn idle_hangup(&mut self, source: Option<Source>) -> Option<Source> {
        let holding = source.as_ref().is_some_and(|src| src.tape.is_some())
            && !self.shared.playing.load(Ordering::Relaxed);
        if !holding {
            self.paused_since = None;

            return source;
        }

        let since = *self.paused_since.get_or_insert_with(Instant::now);
        if since.elapsed().as_secs() < LIVE_IDLE_HANGUP_SECS {
            return source;
        }

        log::info!("station paused for {LIVE_IDLE_HANGUP_SECS}s, letting the connection go");
        self.paused_since = None;

        self.hang_up(source)
    }

    /// The audible track's position in frames, or zero, so a station's elapsed
    /// readout carries across a reopen.
    fn elapsed_frames(&self) -> u64 {
        self.shared
            .position(self.device_rate)
            .filter(|(track, _)| *track == self.idx)
            .map(|(_, secs)| (secs * self.device_rate as f64).round() as u64)
            .unwrap_or(0)
    }

    /// Hang up on the audible live station, for the idle cap. Everything else is
    /// handed straight back. Dropping the `Source` is the close: the transport is
    /// buried under the decoder with no other handle.
    fn hang_up(&mut self, source: Option<Source>) -> Option<Source> {
        // Only the audible station: the decode cursor may be a track ahead.
        let pos = self.audible_pos();
        if source.is_none() || pos != self.pos || !self.live_at(pos) {
            return source;
        }

        // Freeze the elapsed clock at hang-up so the resume carries on from it:
        // through a pause nothing more was heard.
        let elapsed = self.elapsed_frames();

        drop(source);

        // Drop the ring and pending: the resume rejoins live.
        self.flush_ring();
        self.hung_up = Some(elapsed);

        None
    }

    /// Rejoin at the live edge through [`open_file_at`](Self::open_file_at), the
    /// tested first-open path. The title survives the pause; republishing the
    /// same text is dropped as a repeat.
    fn rejoin(&mut self) -> Option<Source> {
        let elapsed = self.hung_up.take()?;
        let pos = self.audible_pos();

        // Play means play even if the station fails and falls forward.
        self.shared.playing.store(true, Ordering::Relaxed);
        self.shared.ended.store(false, Ordering::Relaxed);

        let (src, at, info) = self.open_file_at(pos)?;
        // Falling forward lands elsewhere, which starts at its top.
        let start = if at == pos { elapsed } else { 0 };
        self.adopt(at, info, start, 0);

        Some(src)
    }

    /// None when the queue is played out.
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

    /// Apply an armed stop-after once the ring drains, and return what to cue.
    /// The pause is stored even with nothing to cue, or a later queue edit or
    /// batch would start audio against the stop. Read here, not when armed, so
    /// disarming during the drain rolls on.
    fn land_stop(&mut self) -> Option<usize> {
        self.stop_pending = false;
        if self.stop_after {
            self.shared.playing.store(false, Ordering::Relaxed);
        }
        self.next_pos()
    }

    /// The ring is empty.
    fn ring_drained(&self) -> bool {
        self.producer.slots() == self.producer.buffer().capacity()
    }

    /// Album groups decide (ADR 17, ADR 19): same album keeps its splice,
    /// anything else fades, ungrouped included. `fade_albums` fades everything.
    fn fades_between(&self, from: usize, to: usize) -> bool {
        let (Some(a), Some(b)) = (self.order.get(from), self.order.get(to)) else {
            return false;
        };
        // Repeat-one never fades into its own head.
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

    /// At most half the track.
    fn fade_window(&self, total: Option<u64>) -> u64 {
        let len = (self.fade_secs.max(0.0) as f64 * self.device_rate as f64) as u64;
        match total {
            Some(total) => len.min(total / 2),
            None => len,
        }
    }

    /// Re-asked every chunk so a late queue edit is honored; `fade_armed` stops
    /// retries once acted on.
    fn fade_due(&self, src: Option<&Source>) -> bool {
        match src {
            Some(src) => self.window_open(src.total_frames, src.remaining()),
            None => false,
        }
    }

    fn window_open(&self, total: Option<u64>, remaining: Option<u64>) -> bool {
        // No fade into an armed stop, or out of a track looping a section.
        if self.fade_armed
            || self.fade.is_some()
            || self.stop_after
            || self.ab.is_some_and(|ab| ab.track == self.idx)
        {
            return false;
        }
        let len = self.fade_window(total);
        if len == 0 {
            return false;
        }
        // A track of unknown length never opens a window: there's no honest
        // answer to how far from the end it is.
        if remaining.is_none_or(|left| left > len) {
            return false;
        }
        self.next_pos()
            .is_some_and(|next| self.fades_between(self.pos, next))
    }

    /// Open the next track early and put the current one into the fade. Returns
    /// the old source untouched when nothing opens.
    fn start_boundary_fade(&mut self, old: Option<Source>) -> Option<Source> {
        let old = old?;
        // One attempt per track.
        self.fade_armed = true;
        let Some(next) = self.next_pos() else {
            return Some(old);
        };
        // No longer than what's left, which differs from the window after a seek into it.
        let len = self
            .fade_window(old.total_frames)
            .min(old.remaining().unwrap_or(u64::MAX));
        // Nothing left to fade with (a seek at the end, or an under-claiming
        // container): splice plainly. A one-frame window would zero the incoming
        // track's first frame.
        if len == 0 {
            return Some(old);
        }
        match self.open_at_from(next, len / 2) {
            Some(new) => {
                // Shown like a Next.
                self.publish_fade(self.pushed_playable, len, false);
                self.fade = Some(Fade::new(old, len));
                Some(new)
            }
            None => Some(old),
        }
    }

    /// Jump to play-order position `p`: open it, wind the leaving track back to
    /// what was heard, cut the ring, and fade the old under the new (ADR 19).
    /// The open comes first so the flush has nothing to wait on. `at` opens the
    /// new track that far in before any of it reaches the ring. An ending still
    /// draining is left to finish, no cut.
    fn skip_to(
        &mut self,
        mut old: Option<Source>,
        p: usize,
        back: bool,
        at: Option<f64>,
    ) -> Option<Source> {
        // For a station, claim the entry on the clock before the slow open, so the
        // connect shows as the station opening rather than the old track playing.
        // Only stations: a file opens instantly, and a bookmark skip would flash a
        // false 0:00.
        let live = self.live_at(p);
        // Prepare the fade before the claim moves the clock; both still precede the cut.
        let had_source = old.is_some();
        let wound = live.then(|| self.prepare_skip_fade(old.take())).flatten();
        if live {
            // The part of `adopt` that applies without a source, as in `open_start`.
            self.pos = p;
            self.idx = self.order[p].idx;
            self.claim_segment();
        }

        let opened = self.open_file_at(p);
        // Nothing decoding or mixing but the ring still full: the gap between the
        // last EOF and ended. Don't chop that ending; queue behind it like a
        // gapless boundary.
        let draining = !had_source && self.fade.is_none() && !self.ring_drained();
        let leaving = wound.or_else(|| self.prepare_skip_fade(old));
        let cut = if draining {
            self.pushed_playable
        } else {
            self.flush_ring()
        };
        self.shared.ended.store(false, Ordering::Relaxed);
        // Nothing opened, so no fade to install: nothing would ever clear it.
        let (mut src, pos, info) = opened?;
        // A failed seek starts at the top, like any skip.
        let landed = at.and_then(|secs| src.seek(secs)).unwrap_or(0.0);
        let midpoint = self.install_skip_fade(leaving, cut, back);
        let start = (landed * self.device_rate as f64).round() as u64;
        self.adopt(pos, info, start, midpoint);
        Some(src)
    }

    /// Scrub the audible track to `secs` with a cut. Reopens the audible track
    /// first, since the decode cursor may already be on the next one; with no
    /// source (ended, or draining toward a stop) that same reopen revives the
    /// finished track.
    fn seek_to(&mut self, mut source: Option<Source>, secs: f64) -> Option<Source> {
        let ap = self.audible_pos();
        let mut reopened = None;
        if ap != self.pos || source.is_none() {
            // The open source is on the wrong track either way.
            reopened = self.open_file_at(ap);
            source = None;
        }
        // Seek before the cut, while the ring still plays.
        let landed = match reopened.as_mut() {
            Some((src, _, _)) => src.seek(secs),
            None => source.as_mut().and_then(|src| src.seek(secs)),
        };
        self.flush_ring();
        if let Some((src, at, info)) = reopened {
            self.adopt(at, info, 0, 0);
            // Revived from ended.
            self.shared.ended.store(false, Ordering::Relaxed);
            source = Some(src);
        }
        if let Some(landed) = landed {
            self.register_segment(landed);
            // A seek outside the section ends it; one inside keeps it.
            let rate = self.device_rate as f64;
            let outside = self.ab.is_some_and(|ab| {
                ab.track != self.idx || landed < ab.a as f64 / rate || landed > ab.b as f64 / rate
            });
            if outside {
                self.clear_ab();
            }
        }
        source
    }

    /// Wind the skipped-from track back to what the listener actually heard, and
    /// return the output frame that's at. Before the flush, so the seek doesn't
    /// hold the silence open. None when nothing should fade: fade off, paused,
    /// the open source isn't what's audible, or the seek failed.
    fn prepare_skip_fade(&mut self, old: Option<Source>) -> Option<Wound> {
        let mut old = old?;
        if self.fade_secs <= 0.0
            || self.pos != self.audible_pos()
            || !self.shared.playing.load(Ordering::Relaxed)
        {
            return None;
        }
        // One clock reading for both.
        let at = self.shared.frames_consumed.load(Ordering::Relaxed);
        let (_, secs) = self.shared.position_at(at, self.device_rate)?;
        // Keep how far short the seek landed (seconds for an unindexed CBR MP3), so
        // the install can discard it.
        let landed = old.seek(secs)?;
        let short = ((secs - landed) * self.device_rate as f64).round() as i64;
        Some(Wound {
            src: old,
            at,
            short,
        })
    }

    /// Install the wound-back track as the fade and return the new segment's
    /// offset: the window's middle, or zero for a cut.
    fn install_skip_fade(&mut self, leaving: Option<Wound>, cut: u64, back: bool) -> u64 {
        let Some(Wound { src, at, short }) = leaving else {
            return 0;
        };
        // Discard what played during the flush, so the fade starts where the cut ended.
        let owed = cut.saturating_sub(at);
        // Over a quarter second means the flush stalled; cut instead.
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
        // No longer than what's left of the leaving track.
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

    /// Drop a fade before it ever mixed, and its publish, which nothing else
    /// would clear.
    fn drop_fade(&mut self) {
        if self.fade.take().is_some() {
            self.shared.fade_len.store(0, Ordering::Release);
        }
    }

    /// Close the fade within a few milliseconds when the incoming track ended
    /// first. The curve reads progress off `len`, so shrinking it ramps the tail
    /// out; the publish stands.
    fn close_fade_fast(&mut self) {
        let ramp = (self.device_rate as u64 / 50).max(1);
        if let Some(fade) = self.fade.as_mut() {
            fade.len = fade.len.min(fade.done + ramp);
        }
    }

    /// Sum the outgoing track under the chunk, so the chain and ring get one stream.
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
            // Only the mix is done; the publish stands until the output clock passes it.
            self.fade = None;
        }
    }

    /// On entry changes only; the playing entry comes off the position clock.
    fn publish_queue(&self) {
        let entries = self
            .order
            .iter()
            .map(|e| QueueEntry {
                id: e.id,
                locator: self.queue[e.idx].clone(),
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

    fn find(&self, id: u64) -> Option<usize> {
        self.order.iter().position(|e| e.id == id)
    }

    /// Order position of the track the speakers are playing, off the output
    /// clock. Navigation anchors here, not `pos`, which leads near a boundary.
    /// Falls back to `pos` before any frame plays.
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

    /// Splice into the pool and order after `after`, or at the end. Never
    /// flushes; the cursor shifts with a splice before it. Returns the first
    /// new position, for reviving from ended.
    fn insert(
        &mut self,
        after: Option<u64>,
        locators: Vec<Locator>,
        groups: Vec<Option<u64>>,
        gains: Vec<gain::ReplayGain>,
        spans: Vec<Option<Span>>,
        explicit: bool,
    ) -> Option<usize> {
        if locators.is_empty() {
            return None;
        }
        let at = match after {
            Some(id) => match self.find(id) {
                Some(p) => p + 1,
                None => self.order.len(),
            },
            None => self.order.len(),
        };
        let mut new = Vec::with_capacity(locators.len());
        for (i, locator) in locators.into_iter().enumerate() {
            let idx = self.queue.len();
            self.queue.push(locator);
            self.groups.push(groups.get(i).copied().flatten());
            self.gains.push(gains.get(i).copied().unwrap_or_default());
            self.spans.push(spans.get(i).copied().flatten());
            // Every per-pool vector grows here, or an indexed publish silently misses.
            self.shared.tracks.lock().unwrap().push(None);
            self.shared.titles.lock().unwrap().push(None);
            self.shared.station.lock().unwrap().push(None);
            self.shared.stream.lock().unwrap().push(None);
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

    /// Removing the audible entry is refused (checked against the audible
    /// position, not the decode cursor). Returns true when it removed the
    /// runahead, which the caller must then drop and reopen.
    fn remove(&mut self, id: u64) -> bool {
        let Some(p) = self.find(id) else {
            return false;
        };
        let audible = self.audible_pos();
        if p == audible {
            return false;
        }
        // The cursor entry is the runahead when it leads the audible track.
        let removed_runahead = p == self.pos && self.pos != audible;
        self.order.remove(p);
        // The cursor shifts down, onto the audible entry when the runahead went,
        // so the open source's EOF handoff still lands right.
        if p <= self.pos {
            self.pos = self.pos.saturating_sub(1);
        }
        self.publish_queue();
        removed_runahead
    }

    /// One sweep, keeping the audible entry, then re-find the cursor and publish
    /// once: O(n) and one wake. Returns true when the runahead went.
    fn remove_many(&mut self, ids: &[u64]) -> bool {
        if ids.is_empty() {
            return false;
        }
        let drop: std::collections::HashSet<u64> = ids.iter().copied().collect();
        let audible = self.audible_pos();
        let keep = self.order.get(audible).map(|e| e.id);
        let cursor = self.order.get(self.pos).map(|e| e.id);
        // The runahead, if it leads and is being dropped.
        let removed_runahead =
            self.pos != audible && cursor.is_some_and(|id| drop.contains(&id) && Some(id) != keep);
        let before = self.order.len();
        self.order
            .retain(|e| !drop.contains(&e.id) || Some(e.id) == keep);
        if self.order.len() == before {
            return false;
        }
        // Re-find the cursor by id; if it went, fall back to the audible entry.
        self.pos = cursor
            .and_then(|id| self.find(id))
            .or_else(|| keep.and_then(|id| self.find(id)))
            .unwrap_or_else(|| self.pos.min(self.order.len().saturating_sub(1)));
        self.publish_queue();
        removed_runahead
    }

    /// The cursor is re-found by id.
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

    /// The last entry a tail reorder leaves alone: the audible one, not the
    /// decode cursor, or Similar switched on near a boundary wouldn't reach the
    /// next track. Except during a boundary fade, where the cursor's incoming
    /// track is already audible in the mix.
    fn reorder_anchor(&self) -> usize {
        if self.fade.is_some() {
            self.pos
        } else {
            self.audible_pos()
        }
    }

    /// Whether the runahead's samples are already in the ring. Its segment
    /// exists from the open, so compare against `pushed_playable`.
    fn runahead_ringed(&self) -> bool {
        let Some(entry) = self.order.get(self.pos) else {
            return false;
        };
        if self.pos == self.audible_pos() {
            return false;
        }
        let idx = entry.idx;
        let segments = self.shared.segments.lock().unwrap();
        segments
            .iter()
            .any(|s| s.track == idx && s.at_frame < self.pushed_playable)
    }

    /// Reorder the upcoming portion past [`reorder_anchor`](Self::reorder_anchor).
    /// Never flushes. Under Loop All on the last entry the span wraps to the front.
    ///
    /// Returns true when the runahead was displaced; the caller reopens, as for
    /// [`remove`](Self::remove). A reopen can't pull back frames already in the
    /// ring, so once the runahead has fed the ring the sort starts past it:
    /// Similar reorders on every batch, and must never splice mid-note.
    fn reorder_tail(&mut self, sort: impl FnOnce(&mut [OrderEntry])) -> bool {
        let len = self.order.len();
        let anchor = self.reorder_anchor();
        // Slots the sort may touch.
        let (mut start, mut end) = if anchor + 1 < len {
            (anchor + 1, len)
        } else if self.loop_mode == LoopMode::All && len > 1 && anchor == len - 1 {
            (0, anchor)
        } else {
            self.publish_queue();
            return false;
        };
        // During a boundary fade under Loop All the cursor may have wrapped while
        // the last entry still plays: stop short of it.
        if end == len && anchor + 1 < len && self.audible_pos() == len - 1 {
            end = len - 1;
        }
        if (start..end).contains(&self.pos) && self.runahead_ringed() {
            start = self.pos + 1;
        }
        if start >= end {
            self.publish_queue();
            return false;
        }
        // The handoff depends on the slots up to the cursor. If the sort leaves
        // them in order, the pre-roll stands.
        let leads = (start..end).contains(&self.pos);
        let watched = start..=if leads { self.pos } else { start };
        let before: Vec<u64> = self.order[watched.clone()].iter().map(|e| e.id).collect();
        sort(&mut self.order[start..end]);
        let changed = leads
            && self.order[watched]
                .iter()
                .map(|e| e.id)
                .ne(before.iter().copied());
        if changed {
            // Re-anchor on the audible entry so the reopen takes the new next slot.
            self.pos = anchor;
        }
        self.publish_queue();
        changed
    }

    /// Shuffle the upcoming portion, or restore pool order.
    fn set_shuffle(&mut self, on: bool) -> bool {
        self.reorder_tail(|tail| {
            if on {
                shuffle_slice(tail);
            } else {
                tail.sort_by_key(|e| e.idx);
            }
        })
    }

    /// Put the upcoming portion in `ids` order. Stable, with unnamed entries
    /// last, so a partial list works.
    fn order_tail(&mut self, ids: &[u64]) -> bool {
        let rank = |id: u64| {
            ids.iter()
                .position(|&want| want == id)
                .unwrap_or(usize::MAX)
        };
        self.reorder_tail(|tail| tail.sort_by_key(|e| rank(e.id)))
    }

    /// Have the backend discard the ring, wait for its ack, and resync the clock.
    /// Returns the frame the cut ended at. The wait is the whole gap a skip
    /// costs, so do everything possible before calling.
    fn flush_ring(&mut self) -> u64 {
        self.pending.clear();
        self.pending_pos = 0;
        // A skip starts its own fade after this; a seek just cuts.
        self.fade = None;
        self.shared.fade_len.store(0, Ordering::Release);
        // A discontinuity: nodes drop their history.
        self.chain.reset(self.device_rate);
        let seq = self.shared.flush_seq.fetch_add(1, Ordering::Release) + 1;
        // Bounded, so a dead stream can't spin this forever.
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

    /// Move onto a fresh output ring after a device fault. Everything upstream
    /// stays. The dead ring's half second was never heard, so come back to where
    /// the speakers were: a station re-syncs over its tape, anything else seeks
    /// to the position clock. Nothing is opened: a hung-up station or played-out
    /// queue just resyncs the clock.
    fn swap_output(&mut self, producer: Producer<f32>, source: Option<Source>) -> Option<Source> {
        // Measured off the dead ring before it's dropped.
        let behind = source
            .as_ref()
            .and_then(|src| src.tape.as_ref())
            .filter(|_| self.pos == self.audible_pos())
            .map(|tape| {
                let ringed = self.producer.buffer().capacity() - self.producer.slots();
                let pending = self.pending.len() - self.pending_pos;
                tape.note_queued((ringed + pending) as f64 / 2.0 / self.device_rate as f64);

                tape.shift().behind_secs
            });

        log::info!("output stream replaced under the running session");
        self.producer = producer;

        if let Some(behind) = behind {
            return self.seek_live_to(source, behind);
        }

        // A station off the tape path would redial on a seek, so it just resyncs.
        let at = self
            .shared
            .position(self.device_rate)
            .map(|(_, secs)| secs)
            .filter(|_| source.is_some() && !self.live_at(self.audible_pos()));
        let Some(secs) = at else {
            self.flush_ring();

            return source;
        };

        self.seek_to(source, secs)
    }

    /// Read back through [`Shared::crossfade`].
    fn publish_fade(&self, at: u64, len: u64, back: bool) {
        self.shared.fade_at.store(at, Ordering::Relaxed);
        self.shared.fade_back.store(back, Ordering::Relaxed);
        self.shared.fade_len.store(len, Ordering::Release);
    }

    fn register_segment(&self, track_secs: f64) {
        self.register_segment_after(track_secs, 0);
    }

    /// Registered `after` frames past the next push: the A-B wrap's chunk runs up
    /// to B first.
    fn register_segment_after(&self, track_secs: f64, after: u64) {
        let consumed = self.shared.frames_consumed.load(Ordering::Relaxed);
        let mut segments = self.shared.segments.lock().unwrap();
        segments.push(Segment {
            at_frame: self.pushed_playable + after,
            track: self.idx,
            track_frame: (track_secs * self.device_rate as f64).round() as u64,
        });
        prune_segments(&mut segments, consumed);
    }

    /// A segment at the cursor's top on the frame the speakers are at, for a
    /// station's claim before its open. Everything else registers at
    /// `pushed_playable`, but a claim has no audio to line up with, and a drain
    /// the cut will cancel would leave it unreached.
    fn claim_segment(&self) {
        let consumed = self.shared.frames_consumed.load(Ordering::Relaxed);
        let mut segments = self.shared.segments.lock().unwrap();
        segments.push(Segment {
            at_frame: consumed,
            track: self.idx,
            track_frame: 0,
        });
        prune_segments(&mut segments, consumed);
    }

    /// Idempotent.
    fn clear_ab(&mut self) {
        if self.ab.take().is_some() {
            self.publish_ab();
        }
    }

    /// Frames to seconds, `u64::MAX` in A for off. Called wherever `ab` changes.
    fn publish_ab(&self) {
        let rate = self.device_rate as f64;
        match self.ab {
            Some(ab) => {
                self.shared
                    .ab_b
                    .store((ab.b as f64 / rate).to_bits(), Ordering::Relaxed);
                self.shared
                    .ab_a
                    .store((ab.a as f64 / rate).to_bits(), Ordering::Release);
            }
            None => self.shared.ab_a.store(u64::MAX, Ordering::Release),
        }
    }
}

/// Keep the newest reached segment and every future one; earlier ones can
/// never resolve again.
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

/// NaN passes a clamp and fails every comparison, reading as on but never
/// fading. Normalized at the one place a length arrives.
fn crossfade_secs(secs: f32) -> f32 {
    if secs.is_nan() {
        return 0.0;
    }
    secs.clamp(0.0, CROSSFADE_MAX_SECS)
}

/// Frames of the wound-back track to skip: flush drift plus a coarse seek's
/// shortfall. None gives up the fade.
fn skip_fade_discard(owed: u64, short: i64, device_rate: u32) -> Option<u64> {
    let total = owed as i64 + short;
    // An overshoot can't be undone; start where it landed.
    if total <= 0 {
        return Some(0);
    }
    // Over a second short would put decode work inside the cut: cut instead.
    if total > device_rate as i64 {
        return None;
    }
    Some(total as u64)
}

enum FlushAction {
    Seek(f64),
    SeekLive(f64),
    Track {
        pos: usize,
        /// From a Previous, for the transport's fade direction only.
        back: bool,
        /// The play-from-bookmark landing, in track seconds.
        at: Option<f64>,
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
            // Callers meaning "no fade" never build one.
            len: len.max(1),
            ended: false,
        }
    }

    /// Short only at the track's end, which mixes as silence.
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

/// Fisher-Yates, xorshift64 seeded off the std hasher's per-process keys, so
/// no rand dependency. Public for the continuation providers (ADR 17).
pub fn shuffle_slice<T>(slice: &mut [T]) {
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

/// Reservoir sampling of `count` items in one pass, same seeding. For
/// drawing a hundred out of a million-row pool. Unordered.
pub fn reservoir<T>(items: impl IntoIterator<Item = T>, count: usize) -> Vec<T> {
    use std::hash::{BuildHasher, Hasher};
    let mut out = Vec::with_capacity(count);
    if count == 0 {
        return out;
    }
    let mut state = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
        | 1;
    for (seen, item) in items.into_iter().enumerate() {
        if out.len() < count {
            out.push(item);
            continue;
        }
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (seen as u64 + 1)) as usize;
        if j < count {
            out[j] = item;
        }
    }
    out
}

/// Shuffle only the first `width`, leaving the ranking behind them: the
/// radio's band. Shared by the player's skip band and the radio provider (ADR 17).
pub fn shuffle_head<T>(slice: &mut [T], width: usize) {
    use std::hash::{BuildHasher, Hasher};
    let width = width.min(slice.len());
    if width < 2 {
        return;
    }
    for i in (1..width).rev() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_usize(i);
        let j = (hasher.finish() % (i as u64 + 1)) as usize;
        slice.swap(i, j);
    }
}

/// Run a call into symphonia or a codec so a panic comes back as an error on
/// the file (`origin`) instead of taking the app down; malformed files have
/// done that. `AssertUnwindSafe` holds because callers never touch the
/// panicked reader or decoder again: `Source` owners drop it, and
/// [`Source::decode_chunk`] sets `poisoned`.
pub(crate) fn guard_decode<T>(
    what: &str,
    origin: &str,
    f: impl FnOnce() -> T,
) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|payload| {
        let msg = format!("{what} panicked on {origin}: {}", panic_detail(&*payload));
        log::error!("{msg}");
        msg
    })
}

/// `String` or `&'static str`; anything else came from `panic_any`.
fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else {
        "no message".to_string()
    }
}

/// Decode a whole file through the playback path and report (decoded frames,
/// claimed frames). Equal means the gapless trim is exact.
pub fn count_frames(path: &Path) -> Result<(u64, Option<u64>), String> {
    // Probe for the rate, then open at it so the resampler is a passthrough.
    let (probe, info) = Source::open(&Locator::Local(path.to_path_buf()), 48000, None)?;
    drop(probe);
    let (mut src, info) =
        Source::open(&Locator::Local(path.to_path_buf()), info.sample_rate, None)?;

    let mut decoded: u64 = 0;
    let mut chunk = Vec::new();
    loop {
        chunk.clear();
        // The EOF call returns the resampler's tail.
        let more = src.next_chunk(info.sample_rate, &mut chunk);
        decoded += (chunk.len() / 2) as u64;
        if !more {
            break;
        }
    }
    Ok((decoded, info.num_frames))
}

/// Decode a whole file into waveform peak lanes of at most `bins`: extremes
/// and RMS per bin. Lane 0 is the mono mix, then left and right for stereo.
/// Normalized to the loudest extreme (channels together) with a perceptual
/// curve. A full decode, so run it off the UI thread.
pub fn decode_peaks(path: &Path, bins: usize) -> Result<PeakLanes, String> {
    // Probe for the rate, then open at it so the resampler is a passthrough.
    let (probe, info) = Source::open(&Locator::Local(path.to_path_buf()), 48000, None)?;
    drop(probe);
    let (mut src, info) =
        Source::open(&Locator::Local(path.to_path_buf()), info.sample_rate, None)?;

    // Fixed blocks first, so memory stays small whatever the length, then fold
    // to `bins`.
    const BLOCK_FRAMES: usize = 2048;
    let mut coarse: [Vec<PeakBin>; 3] = Default::default();
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut sq = [0.0f64; 3];
    let mut in_block = 0usize;
    let mut chunk = Vec::new();
    loop {
        chunk.clear();
        // The EOF call returns the resampler's tail.
        let more = src.next_chunk(info.sample_rate, &mut chunk);
        for frame in chunk.as_chunks::<2>().0 {
            let s = [(frame[0] + frame[1]) * 0.5, frame[0], frame[1]];
            for lane in 0..3 {
                lo[lane] = lo[lane].min(s[lane]);
                hi[lane] = hi[lane].max(s[lane]);
                sq[lane] += (s[lane] as f64) * (s[lane] as f64);
            }
            in_block += 1;
            if in_block == BLOCK_FRAMES {
                for lane in 0..3 {
                    coarse[lane].push(PeakBin {
                        lo: lo[lane],
                        hi: hi[lane],
                        rms: (sq[lane] / BLOCK_FRAMES as f64).sqrt() as f32,
                    });
                }
                lo = [f32::MAX; 3];
                hi = [f32::MIN; 3];
                sq = [0.0; 3];
                in_block = 0;
            }
        }
        if !more {
            break;
        }
    }
    if in_block > 0 {
        for lane in 0..3 {
            coarse[lane].push(PeakBin {
                lo: lo[lane],
                hi: hi[lane],
                rms: (sq[lane] / in_block as f64).sqrt() as f32,
            });
        }
    }
    if coarse[0].is_empty() {
        return Err("no decodable audio".into());
    }

    // Mono has no channel lanes.
    let keep = if info.channels >= 2 { 3 } else { 1 };
    let mut lanes: PeakLanes = coarse
        .into_iter()
        .take(keep)
        .map(|lane| fold_bins(lane, bins))
        .collect();

    normalize_peaks(&mut lanes[..1]);
    normalize_peaks(&mut lanes[1..]);
    Ok(lanes)
}

/// Keeps each bucket's extremes so transients survive. RMS folds as root mean
/// square.
fn fold_bins(coarse: Vec<PeakBin>, bins: usize) -> Vec<PeakBin> {
    if coarse.len() <= bins.max(1) {
        return coarse;
    }
    let per = coarse.len() as f64 / bins as f64;
    (0..bins)
        .map(|i| {
            let from = (i as f64 * per) as usize;
            let to = (((i + 1) as f64 * per) as usize).clamp(from + 1, coarse.len());
            fold_bucket(&coarse[from..to])
        })
        .collect()
}

fn fold_bucket(run: &[PeakBin]) -> PeakBin {
    let (lo, hi, sq) = run
        .iter()
        .fold((f32::MAX, f32::MIN, 0.0f64), |(lo, hi, sq), b| {
            (
                lo.min(b.lo),
                hi.max(b.hi),
                sq + (b.rms as f64) * (b.rms as f64),
            )
        });
    PeakBin {
        lo,
        hi,
        rms: (sq / run.len().max(1) as f64).sqrt() as f32,
    }
}

/// Lanes normalized together keep their relative loudness; RMS stays inside
/// the envelope.
fn normalize_peaks(lanes: &mut [Vec<PeakBin>]) {
    let loudest = lanes
        .iter()
        .flatten()
        .fold(0.0f32, |m, b| m.max(b.lo.abs()).max(b.hi.abs()));
    if loudest <= 0.0 {
        return;
    }
    let curve = |v: f32| (v.abs() / loudest).powf(0.7).copysign(v);
    for b in lanes.iter_mut().flatten() {
        b.lo = curve(b.lo);
        b.hi = curve(b.hi);
        b.rms = curve(b.rms).min(b.lo.abs().max(b.hi.abs()));
    }
}

/// Decode one stereo window at `position_secs`, for the spectrum on a track
/// loaded paused, which the tap never fed. Local only: a network round trip
/// to decorate a paused load isn't worth it.
pub fn decode_window(
    locator: &Locator,
    position_secs: f64,
    device_rate: u32,
    frames: usize,
) -> Result<Vec<f32>, String> {
    if locator.path().is_none() {
        return Err("no decode window for a remote track".into());
    }

    let (mut src, _) = Source::open(locator, device_rate, None)?;
    if position_secs > 0.0 {
        let _ = src.seek(position_secs);
    }
    let mut out = Vec::with_capacity(frames * 2);
    let mut chunk = Vec::new();
    while out.len() < frames * 2 {
        chunk.clear();
        // The EOF call returns the resampler's tail.
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
    /// Open `locator` for `span`, None meaning the whole file. A spanned source
    /// behaves as if the span were the file. A remote locator only changes where
    /// the bytes come from. Drops station titles: this is for analysis passes;
    /// playback uses [`Source::open_titled`].
    fn open(
        locator: &Locator,
        device_rate: u32,
        span: Option<Span>,
    ) -> Result<(Source, TrackInfo), String> {
        let (source, info, _) = Source::open_titled(
            locator,
            device_rate,
            span,
            crate::icy::no_titles(),
            crate::shared::no_stream(),
            // Analysis passes have their own cancellation.
            Arc::new(AtomicBool::new(false)),
            // Nothing pauses an analysis pass.
            LIVE_BUFFER_MIN_SECS,
        )?;

        Ok((source, info))
    }

    /// With a sink for station titles, fired from inside decode reads, and the
    /// station description returned (headers exist only now). None for local files.
    #[allow(clippy::too_many_arguments)]
    fn open_titled(
        locator: &Locator,
        device_rate: u32,
        span: Option<Span>,
        on_title: TitleSink,
        on_stream: StreamSink,
        interrupt: Arc<AtomicBool>,
        live_buffer_secs: u32,
    ) -> Result<(Source, TrackInfo, Option<StationInfo>), String> {
        // Open-latency start; only remote opens log.
        let began = Instant::now();
        let remote_open = matches!(locator, Locator::Remote(_));

        // A URL's hint comes from the locator or the `Content-Type`, else the probe sniffs.
        let (mss, hint, origin, station, tape) = match locator {
            Locator::Local(path) => {
                let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                let mss = MediaSourceStream::new(Box::new(file), Default::default());

                let mut hint = Hint::new();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    hint.with_extension(ext);
                }

                (mss, hint, path.display().to_string(), None, None)
            }

            Locator::Remote(remote) => {
                let (opened, station) =
                    crate::http::open(remote, on_title, on_stream, interrupt, live_buffer_secs)?;
                let mss = MediaSourceStream::new(opened.source, Default::default());

                let mut hint = Hint::new();
                if !remote.hint.is_empty() {
                    hint.with_extension(&remote.hint);
                } else if let Some(ext) = crate::http::extension_for(&station.content_type) {
                    hint.with_extension(ext);
                }

                (mss, hint, remote.url.clone(), Some(station), opened.tape)
            }
        };

        let (source, info) = Source::build(
            mss,
            hint,
            locator.label(),
            origin,
            device_rate,
            span,
            locator.path(),
            began,
            remote_open,
            tape,
        )?;

        Ok((source, info, station))
    }

    /// Re-sync a decoder at absolute offset `at` in this source's tape: a fresh
    /// probe, reader, and decoder over a new cursor, while the tape and
    /// connection carry on. MP3 and ADTS find their footing anywhere; Ogg's
    /// cursor is already on a page. Errors leave the caller's source playing.
    fn reopen_live(&self, at: u64, hint_ext: &str) -> Result<(Source, TrackInfo), String> {
        let tape = self.tape.clone().ok_or("not a live stream")?;
        let mss = MediaSourceStream::new(
            Box::new(crate::http::live_source(&tape, at)),
            Default::default(),
        );

        let mut hint = Hint::new();
        if !hint_ext.is_empty() {
            hint.with_extension(hint_ext);
        }

        Source::build(
            mss,
            hint,
            self.name.clone(),
            self.origin.clone(),
            self.device_rate,
            None,
            None,
            Instant::now(),
            false,
            Some(tape),
        )
    }

    /// Probe, build the decoder, work out the length, and position at the start.
    /// Shared by locator opens and timeshift re-syncs, so both take the same path.
    /// `path` exists only for a local fragmented MP4's duration.
    #[allow(clippy::too_many_arguments)]
    fn build(
        mss: MediaSourceStream<'static>,
        hint: Hint,
        name: String,
        origin: String,
        device_rate: u32,
        span: Option<Span>,
        path: Option<&Path>,
        began: Instant,
        remote: bool,
        tape: Option<Arc<Tape>>,
    ) -> Result<(Source, TrackInfo), String> {
        // The probe parses file bytes too; a panic leaves nothing half-built behind.
        let probe_began = Instant::now();
        let format = guard_decode("probe", &origin, || {
            symphonia::default::get_probe().probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
        })?
        .map_err(|e| format!("probe: {e}"))?;

        // Second open-latency timing. On a stream the probe reads at the station's bitrate.
        if remote {
            log::debug!(
                "stream open: probe settled in {:?}, {:?} into the open",
                probe_began.elapsed(),
                began.elapsed()
            );
        }

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

        // Zero from either field means unknown, not empty.
        let stated_frames = track.num_frames.filter(|n| *n > 0);
        let stated_secs = track
            .duration
            .filter(|dur| dur.get() > 0)
            .zip(time_base)
            .and_then(|(dur, tb)| tb.calc_time(Timestamp::from(dur.get() as i64)))
            .map(|t| t.as_secs_f64())
            .or_else(|| stated_frames.map(|n| n as f64 / sample_rate as f64));

        // A fragmented MP4 states its length only in the movie header; without it
        // the track reads as zero seconds. Local files only, since it rereads.
        let file_secs =
            stated_secs.or_else(|| path.and_then(rox_library::mp4::fragment_duration_secs));
        let file_frames =
            stated_frames.or_else(|| file_secs.map(|s| (s * sample_rate as f64).round() as u64));

        // Decoder setup parses header fields and can panic like a decode.
        let decoder = guard_decode("decoder setup", &origin, || {
            crate::codecs::registry().make_audio_decoder(params, &AudioDecoderOptions::default())
        })?
        .map_err(|e| format!("decoder: {e}"))?;

        // The span on the file's frame clock: cut after resampling, the boundary
        // would miss by a sample or two.
        let span_frames = span.map(|s| SpanFrames {
            start: ms_frames(s.start_ms, sample_rate),
            end: s.end_ms.map(|end| ms_frames(end, sample_rate)),
        });

        // Length measured between the frames the span's ends resolve to, which is
        // what the decode follows. An open-ended span runs to the file's end.
        let start_secs = span_frames.map_or(0.0, |sf| sf.start as f64 / sample_rate as f64);
        let (duration_secs, num_frames) = match span_frames {
            Some(sf) => {
                let frames = match sf.end {
                    // A sheet past the file's end gets the file's end.
                    Some(end) => Some(
                        end.min(file_frames.unwrap_or(u64::MAX))
                            .saturating_sub(sf.start),
                    ),
                    None => file_frames.map(|n| n.saturating_sub(sf.start)),
                };
                let secs = frames
                    .map(|n| n as f64 / sample_rate as f64)
                    .or_else(|| file_secs.map(|secs| (secs - start_secs).max(0.0)));
                (secs, frames)
            }
            None => (file_secs, file_frames),
        };

        let info = TrackInfo {
            name,
            duration_secs,
            num_frames,
            sample_rate,
            channels,
        };

        // Device-rate length for the fade window: the container's frame count first.
        let total_frames = num_frames
            .map(|n| (n as f64 * device_rate as f64 / sample_rate as f64).round() as u64)
            .or_else(|| duration_secs.map(|secs| (secs * device_rate as f64).round() as u64));

        let mut source = Source {
            format,
            decoder,
            name: info.name.clone(),
            origin,
            poisoned: false,
            track_id,
            time_base,
            device_rate,
            resampler: Resampler::new(sample_rate, device_rate),
            scratch: Vec::new(),
            rg: gain::ReplayGain::default(),
            gain: 1.0,
            pos_frames: 0,
            total_frames,
            span: span_frames,
            src_frame: 0,
            opened_at: remote.then_some(began),
            tape,
        };

        // Seek to the span's start before the first packet; the decode drops what
        // the packet-granular landing left in front.
        if span_frames.is_some_and(|sf| sf.start > 0) && source.seek_file(start_secs).is_none() {
            return Err(format!("seek to span start {start_secs}s failed"));
        }

        Ok((source, info))
    }

    /// Called at the open and again on a rule change, so a switch is heard at once.
    fn level(&mut self, rg: gain::ReplayGain, rule: &gain::GainRule) {
        self.rg = rg;
        self.relevel(rule);
    }

    fn relevel(&mut self, rule: &gain::GainRule) {
        self.gain = rule.factor(self.rg);
    }

    /// None when the container never said.
    fn remaining(&self) -> Option<u64> {
        self.total_frames
            .map(|total| total.saturating_sub(self.pos_frames))
    }

    /// File frames left before the span's end. None when natural EOF bounds it.
    fn span_left(&self) -> Option<u64> {
        self.span?.end.map(|end| end.saturating_sub(self.src_frame))
    }

    /// Decode until a packet yields samples, gain applied. False at end of stream.
    fn next_chunk(&mut self, device_rate: u32, out: &mut Vec<f32>) -> bool {
        let from = out.len();
        let more = self.decode_chunk(device_rate, out);

        // Audio seconds against bytes read: the tape's exact rate measurement.
        if let Some(tape) = self.tape.as_ref() {
            tape.note_audio((out.len() - from) as f64 / 2.0 / device_rate as f64);
        }
        // Source gain (ADR 19), before this source meets any other.
        gain::apply(&mut out[from..], self.gain);
        self.pos_frames += ((out.len() - from) / 2) as u64;

        // Last open-latency timing: audio exists. Taken, so it logs once.
        if out.len() > from
            && let Some(began) = self.opened_at.take()
        {
            log::debug!(
                "stream open: first audio {:?} after the open began",
                began.elapsed()
            );
        }

        more
    }

    fn decode_chunk(&mut self, device_rate: u32, out: &mut Vec<f32>) -> bool {
        // A span at its end behaves like EOF: drain the tail and stop.
        if self.span_left() == Some(0) {
            self.resampler.flush(out);
            return false;
        }
        // Poisoned: already flushed on the way in.
        if self.poisoned {
            return false;
        }
        loop {
            // A panic in reader or decoder ends the track and poisons the source; see
            // [`guard_decode`].
            let read = {
                let (origin, format) = (&self.origin, &mut self.format);
                guard_decode("packet read", origin, || format.next_packet())
            };
            let Ok(read) = read else {
                self.poisoned = true;
                self.resampler.flush(out);
                return false;
            };
            let packet = match read {
                Ok(Some(p)) => p,
                // Flush the tail so the last samples aren't lost at the boundary.
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

            // The copy is inside the guard: the decoded audio is borrowed from the codec.
            let decoded = {
                let (origin, decoder, scratch) =
                    (&self.origin, &mut self.decoder, &mut self.scratch);
                guard_decode("decode", origin, || {
                    decoder.decode(&packet).map(|decoded| {
                        let frames = decoded.frames();
                        if frames == 0 {
                            return None;
                        }
                        let spec = decoded.spec();
                        let (rate, ch) = (spec.rate(), spec.channels().count());
                        scratch.resize(decoded.samples_interleaved(), 0.0);
                        decoded.copy_to_slice_interleaved(scratch);
                        Some((frames, rate, ch))
                    })
                })
            };
            let Ok(decoded) = decoded else {
                self.poisoned = true;
                self.resampler.flush(out);
                return false;
            };
            let (frames, rate, ch) = match decoded {
                Ok(Some(got)) => got,
                Ok(None) => continue,
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
                // Mid-stream rate change: flush the old resampler's tail first.
                self.resampler.flush(out);
                self.resampler = Resampler::new(rate, device_rate);
            }

            // Fold to stereo: mono duplicates, extra channels drop.
            let mut stereo: Vec<f32> = match ch {
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

            // Drop frames before the span's start: a seek lands on a packet, and those
            // frames belong to the previous track.
            if let Some(start) = self.span.map(|s| s.start)
                && self.src_frame < start
            {
                let skip = ((start - self.src_frame) as usize).min(frames);
                stereo.drain(..skip * 2);
                self.src_frame += skip as u64;
                if stereo.is_empty() {
                    if ch == 2 {
                        self.scratch = stereo;
                    }
                    continue;
                }
            }

            // Cut the span's end inside the packet on the file's clock, before the
            // resampler smears it, then end like EOF.
            let have = (stereo.len() / 2) as u64;
            let ends = match self.span_left() {
                Some(left) if have >= left => {
                    stereo.truncate(left as usize * 2);
                    true
                }
                _ => false,
            };
            self.src_frame += (stereo.len() / 2) as u64;
            self.resampler.process(&stereo, out);
            if ends {
                self.resampler.flush(out);
            }
            if ch == 2 {
                self.scratch = stereo;
            }
            return !ends;
        }
    }

    /// Past B, or at EOF with a section set: trim the overshoot off the chunk
    /// just decoded (the engine clears `out` per refill) and seek to A. Returns
    /// the landing, or None; a failed seek just keeps playing.
    fn wrap_ab(&mut self, a: u64, b: u64, eof: bool, out: &mut Vec<f32>) -> Option<f64> {
        if self.pos_frames < b && !eof {
            return None;
        }
        let over = self.pos_frames.saturating_sub(b) as usize * 2;
        out.truncate(out.len().saturating_sub(over));
        self.seek(a as f64 / self.device_rate as f64)
    }

    /// Accurate, track-relative seek (a span's 0:00 is its own). Returns where
    /// it landed, or None, so a failed seek never moves the clock.
    fn seek(&mut self, secs: f64) -> Option<f64> {
        let secs = self.inside_track(secs);
        let Some(span) = self.span else {
            let landed = self.seek_file(secs)?;
            // The fade window's countdown follows the position.
            self.pos_frames = (landed * self.device_rate as f64).round() as u64;
            return Some(landed);
        };
        let rate = self.resampler.src_rate() as f64;
        let start = span.start as f64 / rate;
        // Clamped to the span: past its end belongs on its last frame, not the next track.
        let target = match span.end {
            Some(end) => (start + secs.max(0.0)).clamp(start, end as f64 / rate),
            None => start + secs.max(0.0),
        };
        let landed = self.seek_file(target)?;
        let rel = (landed - start).max(0.0);
        self.pos_frames = (rel * self.device_rate as f64).round() as u64;
        Some(rel)
    }

    /// Pull a seek target back inside the track. A seek to the last frame fails
    /// and parks the reader at the end, so dragging the strip to its edge would
    /// end the track, or the queue.
    fn inside_track(&self, secs: f64) -> f64 {
        let Some(total) = self.total_frames else {
            return secs;
        };
        let end = total as f64 / self.device_rate as f64 - SEEK_END_MARGIN_SECS;
        secs.min(end.max(0.0))
    }

    /// The seek on the file's own clock: move the reader, reset decoder and
    /// resampler, report the landing. A spanned source seeks this way twice.
    fn seek_file(&mut self, secs: f64) -> Option<f64> {
        let time = Time::try_from_secs_f64(secs).unwrap_or(Time::ZERO);
        // An accurate seek can decode, so it's guarded; a panic reads as a failed seek.
        let seeked = {
            let (origin, format, track_id) = (&self.origin, &mut self.format, self.track_id);
            guard_decode("seek", origin, || {
                format.seek(
                    SeekMode::Accurate,
                    SeekTo::Time {
                        time,
                        track_id: Some(track_id),
                    },
                )
            })
        };
        let Ok(seeked) = seeked else {
            self.poisoned = true;
            return None;
        };
        match seeked {
            Ok(seeked) => {
                let (origin, decoder) = (&self.origin, &mut self.decoder);
                if guard_decode("decoder reset", origin, || decoder.reset()).is_err() {
                    self.poisoned = true;
                    return None;
                }
                // Re-arm, don't rebuild: the sinc table costs 1.5 ms, and an A-B wrap pays
                // this every pass.
                self.resampler.reset();
                let landed = self
                    .time_base
                    .and_then(|tb| tb.calc_time(seeked.actual_ts))
                    .map(|t| t.as_secs_f64().max(0.0))
                    .unwrap_or(secs);
                // The span's end is absolute, so count from where the reader really landed.
                self.src_frame = (landed * self.resampler.src_rate() as f64).round() as u64;
                Some(landed)
            }
            Err(e) => {
                log::warn!("seek failed: {e}");
                // The reader may have moved anyway, which is why targets are clamped first.
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;

    use rox_library::locator::Remote;

    use crate::icy::IcyTitle;

    fn local(path: impl Into<PathBuf>) -> Locator {
        Locator::Local(path.into())
    }

    /// Synthetic paths, a throwaway ring, no device or thread: enough for the
    /// queue-edit math.
    fn test_engine(n: usize) -> Engine {
        engine_over(
            (0..n)
                .map(|i| Locator::Local(PathBuf::from(format!("t{i}"))))
                .collect(),
        )
    }

    fn engine_over(locators: Vec<Locator>) -> Engine {
        engine_with_ring(locators, 8).0
    }

    /// Keeps the ring's read side.
    fn engine_with_ring(locators: Vec<Locator>, frames: usize) -> (Engine, rtrb::Consumer<f32>) {
        let shared = Arc::new(Shared::new(locators.len()));
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(frames * 2);
        let (_tx, rx) = mpsc::channel::<Cmd>();
        let engine = Engine::new(
            StartQueue {
                locators,
                ..StartQueue::default()
            },
            shared,
            producer,
            48000,
            rx,
        );

        (engine, consumer)
    }

    /// Named entries lead; unranked ones keep their order behind, so a partly
    /// analyzed library works.
    #[test]
    fn order_tail_ranks_what_it_knows_and_leaves_the_rest() {
        let mut engine = test_engine(6);
        engine.pos = 1;
        let ids: Vec<u64> = engine.order.iter().map(|e| e.id).collect();
        engine.order_tail(&[ids[5], ids[3]]);
        let after: Vec<u64> = engine.order.iter().map(|e| e.id).collect();
        assert_eq!(
            after,
            vec![ids[0], ids[1], ids[5], ids[3], ids[2], ids[4]],
            "ranked entries lead in the given order, the rest keep theirs"
        );
        // History and the playing entry never move.
        assert_eq!(&after[..2], &ids[..2]);
        assert_eq!(engine.pos, 1);
    }

    #[test]
    fn order_tail_with_nothing_ranked_changes_nothing() {
        let mut engine = test_engine(4);
        engine.pos = 0;
        let before: Vec<u64> = engine.order.iter().map(|e| e.id).collect();
        engine.order_tail(&[]);
        let after: Vec<u64> = engine.order.iter().map(|e| e.id).collect();
        assert_eq!(before, after);
        engine.order_tail(&[9999]);
        assert_eq!(
            before,
            engine.order.iter().map(|e| e.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn order_tail_at_the_end_of_the_queue_is_a_no_op() {
        let mut engine = test_engine(2);
        engine.pos = 1;
        let before: Vec<u64> = engine.order.iter().map(|e| e.id).collect();
        assert!(!engine.order_tail(&[before[0]]));
        assert_eq!(
            before,
            engine.order.iter().map(|e| e.id).collect::<Vec<_>>()
        );
    }

    /// Radio switched on in a track's last seconds must reach the pre-rolled next entry.
    #[test]
    fn order_tail_reaches_the_pre_rolled_next_track() {
        let mut e = test_engine(5);
        set_audible(&e, 1);
        e.pos = 2;
        let moved_runahead = e.order_tail(&[4]);
        let after: Vec<u64> = e.order.iter().map(|en| en.id).collect();
        assert_eq!(
            after,
            vec![0, 1, 4, 2, 3],
            "the slot right after the audible entry is part of the reorder"
        );
        assert!(
            moved_runahead,
            "the open source is no longer the next track, so the caller reopens"
        );
        // Re-anchored so the reopen at audible + 1 lands on the new track.
        assert_eq!(e.pos, 1);
        assert_eq!(e.order[e.pos + 1].id, 4);
    }

    /// The pre-rolled track still next: keep the open source.
    #[test]
    fn order_tail_keeps_the_runahead_when_the_next_slot_holds() {
        let mut e = test_engine(5);
        set_audible(&e, 1);
        e.pos = 2;
        let moved_runahead = e.order_tail(&[2, 4]);
        assert!(
            !moved_runahead,
            "the pre-rolled track is still next, so no reopen"
        );
        assert_eq!(e.order[e.pos].id, 2, "cursor stays on its own entry");
        assert_eq!(
            e.order.iter().map(|en| en.id).collect::<Vec<_>>(),
            vec![0, 1, 2, 4, 3]
        );
    }

    #[test]
    fn order_tail_mid_track_never_asks_for_a_reopen() {
        let mut e = test_engine(5);
        set_audible(&e, 2);
        e.pos = 2;
        let moved_runahead = e.order_tail(&[4, 3]);
        assert!(!moved_runahead);
        assert_eq!(e.pos, 2, "the playing entry stays put");
        assert_eq!(
            e.order.iter().map(|en| en.id).collect::<Vec<_>>(),
            vec![0, 1, 2, 4, 3]
        );
    }

    #[test]
    fn set_shuffle_off_reaches_the_pre_rolled_next_track() {
        let mut e = test_engine(5);
        set_audible(&e, 1);
        e.pos = 2;
        // Scrambled so shuffle-off drags entry 4 into the pre-roll's slot.
        e.order.swap(2, 4);
        let moved_runahead = e.set_shuffle(false);
        assert_eq!(
            e.order.iter().map(|en| en.id).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert!(moved_runahead, "the open source was entry 4, now not next");
        assert_eq!(e.pos, 1);
    }

    /// Under Loop All on the last entry the upcoming portion wraps to the front.
    #[test]
    fn order_tail_wraps_to_the_front_under_loop_all() {
        let mut e = test_engine(4);
        e.loop_mode = LoopMode::All;
        // Cursor wrapped onto entry 0, nothing pushed yet.
        set_audible(&e, 3);
        e.pos = 0;
        let moved_runahead = e.order_tail(&[2]);
        assert_eq!(
            e.order.iter().map(|en| en.id).collect::<Vec<_>>(),
            vec![2, 0, 1, 3],
            "the wrapped tail is the whole order except the audible entry"
        );
        assert!(
            moved_runahead,
            "the pre-rolled entry 0 is no longer first, so the caller reopens"
        );
        // audible + 1 is past the end and wraps to the new front.
        assert_eq!(e.pos, 3);
        assert_eq!(e.order[0].id, 2);
    }

    #[test]
    fn set_shuffle_wraps_to_the_front_under_loop_all() {
        let mut e = test_engine(4);
        e.loop_mode = LoopMode::All;
        set_audible(&e, 3);
        e.pos = 3;
        e.order.swap(0, 2);
        let moved_runahead = e.set_shuffle(false);
        assert!(!moved_runahead, "nothing ran ahead, so nothing reopens");
        assert_eq!(
            e.order.iter().map(|en| en.id).collect::<Vec<_>>(),
            vec![0, 1, 2, 3],
            "shuffle off sorts the wrapped tail back to pool order"
        );
        assert_eq!(e.pos, 3);
    }

    /// A runahead already in the ring keeps its slot; the ranking lands behind it.
    #[test]
    fn order_tail_leaves_a_runahead_that_already_reached_the_ring() {
        let mut e = test_engine(5);
        set_audible(&e, 1);
        e.pos = 2;
        set_runahead_ringed(&mut e, 2);
        let moved_runahead = e.order_tail(&[4]);
        assert!(
            !moved_runahead,
            "no reopen, so nothing the listener hears is cut"
        );
        assert_eq!(
            e.order.iter().map(|en| en.id).collect::<Vec<_>>(),
            vec![0, 1, 2, 4, 3],
            "entry 2 keeps the slot its samples are queued for"
        );
        assert_eq!(e.pos, 2, "the cursor still points at its own entry");
    }

    /// During a boundary fade the incoming track is audible, so it stays put.
    #[test]
    fn a_reorder_during_a_boundary_fade_leaves_the_incoming_track_alone() {
        let fx = Fixtures::new("fade-reorder");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 0.2)),
            local(fx.wav("b.wav", 0.2)),
        ]);
        e.insert(
            None,
            vec![local("c"), local("d")],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
        );
        // Track 1 is fading in: the cursor has adopted it, the clock still reads 0.
        let src = e.open_at(0).expect("the fixture opens");
        set_audible(&e, 0);
        e.pos = 1;
        e.fade = Some(Fade::new(src, 4_800));
        let moved_runahead = e.order_tail(&[1, 3]);
        assert!(
            !moved_runahead,
            "the incoming track keeps playing, so nothing reopens"
        );
        assert_eq!(
            e.order.iter().map(|en| en.id).collect::<Vec<_>>(),
            vec![0, 1, 3, 2],
            "entry 1 is audible in the mix and stays, the rest reorders"
        );
        assert_eq!(e.pos, 1);
    }

    /// The wrap on a real decoder: a 0.5 s section of a 2 s file keeps going,
    /// cuts exactly on B, and plays what A sounds like.
    #[test]
    fn an_ab_loop_wraps_on_b_and_comes_back_to_a() {
        let fx = Fixtures::new("ab-wrap");
        let path = fx.wav("t.wav", 2.0);
        let rate = 48_000u32;
        let a_secs = 0.5;
        let a = (a_secs * rate as f64) as u64;
        let b = rate as u64;

        // A sine, so matching windows is a real comparison.
        let (mut probe, _) = Source::open(&local(&path), rate, None).expect("the fixture opens");
        probe.seek(a_secs).expect("the probe lands on A");
        let mut want = Vec::new();
        while want.len() < 128 {
            assert!(
                probe.next_chunk(rate, &mut want),
                "half a second in, 2 s to go"
            );
        }
        want.truncate(128);

        let (mut src, _) = Source::open(&local(&path), rate, None).expect("the fixture opens");
        let mut out = Vec::new();
        let mut played: Vec<f32> = Vec::new();
        let mut wraps = 0;
        let mut cut = None;
        // Without the loop the source ends halfway through.
        while played.len() / 2 < 4 * rate as usize {
            out.clear();
            let more = src.next_chunk(rate, &mut out);
            match src.wrap_ab(a, b, !more, &mut out) {
                Some(landed) => {
                    wraps += 1;
                    cut.get_or_insert(played.len() / 2 + out.len() / 2);
                    assert!(
                        (landed - a_secs).abs() < 0.05,
                        "the wrap lands on A, got {landed}"
                    );
                }
                None => assert!(more, "a loop set means the source never runs out"),
            }
            played.extend_from_slice(&out);
        }

        assert!(
            wraps >= 5,
            "0.5 s sections across 4 s of output, got {wraps}"
        );
        assert_eq!(
            cut,
            Some(b as usize),
            "the overshoot comes off, so the cut is on B rather than a packet past it"
        );
        let after: Vec<f32> = played[b as usize * 2..][..128].to_vec();
        assert!(
            after.iter().any(|s| s.abs() > 1e-4),
            "the window after the wrap is music, not silence"
        );
        assert!(
            after
                .iter()
                .zip(&want)
                .all(|(got, want)| (got - want).abs() < 1e-5),
            "what plays after the wrap is what plays at A"
        );
    }

    /// B past the end, from an overstated length: the EOF flag still wraps.
    #[test]
    fn an_ab_loop_wraps_at_eof_instead_of_ending_the_track() {
        let fx = Fixtures::new("ab-eof");
        let path = fx.wav("t.wav", 2.0);
        let rate = 48_000u32;
        let a = (1.5 * rate as f64) as u64;
        let b = (2.1 * rate as f64) as u64;
        let (mut src, _) = Source::open(&local(&path), rate, None).expect("the fixture opens");

        let mut out = Vec::new();
        let landed = loop {
            out.clear();
            let more = src.next_chunk(rate, &mut out);
            let wrapped = src.wrap_ab(a, b, !more, &mut out);
            if !more {
                break wrapped.expect("EOF with a section set wraps");
            }
            assert!(wrapped.is_none(), "nothing to wrap short of B");
        };

        assert!((landed - 1.5).abs() < 0.05, "back on A, got {landed}");
        out.clear();
        assert!(
            src.next_chunk(rate, &mut out),
            "and the decoder carries on from there"
        );
    }

    /// The snapshot is the only outside copy (ADR 16).
    #[test]
    fn the_ab_snapshot_round_trips_and_clears() {
        let mut e = test_engine(2);
        assert_eq!(e.shared.ab(), None, "nothing loops on a fresh engine");

        e.ab = Some(AbLoop {
            track: 0,
            a: 48_000,
            b: 96_000,
        });
        e.publish_ab();
        let (a, b) = e.shared.ab().expect("the section is published");
        assert!(
            (a - 1.0).abs() < 1e-9 && (b - 2.0).abs() < 1e-9,
            "{a} to {b}"
        );

        e.clear_ab();
        assert_eq!(e.shared.ab(), None, "and clearing it is heard downstream");
    }

    /// Scrubbing out of the section ends it; scrubbing inside keeps it.
    #[test]
    fn a_seek_out_of_the_section_clears_it_and_one_inside_keeps_it() {
        let fx = Fixtures::new("ab-seek");
        let mut e = engine_over(vec![local(fx.wav("a.wav", 4.0))]);
        let source = ready_to_skip(&mut e, 0);
        e.ab = Some(AbLoop {
            track: 0,
            a: 48_000,
            b: 144_000,
        });
        e.publish_ab();

        let source = e.seek_to(source, 2.0);
        assert!(source.is_some(), "the seek stays on the track");
        assert!(
            e.shared.ab().is_some(),
            "a seek inside the section leaves it alone"
        );

        let _ = e.seek_to(source, 3.5);
        assert_eq!(
            e.shared.ab(),
            None,
            "and a seek past B is how you leave the section"
        );
    }

    /// Next, Prev, and Jump all reach this clear through the flush.
    #[test]
    fn a_skip_clears_the_section() {
        let mut e = test_engine(2);
        e.ab = Some(AbLoop {
            track: 0,
            a: 0,
            b: 48_000,
        });
        e.publish_ab();

        e.clear_ab();

        assert!(e.ab.is_none(), "the engine's own copy goes");
        assert_eq!(e.shared.ab(), None, "and so does the published one");
    }

    /// A section near the end must not open the boundary fade on every wrap.
    #[test]
    fn a_section_holds_the_boundary_fade_shut() {
        let mut e = test_engine(2);
        set_groups(&mut e, &[None, None]);
        e.fade_secs = 4.0;
        assert!(
            e.window_open(Some(480_000), Some(48_000)),
            "a second from the end of an ungrouped boundary, the window opens"
        );
        e.ab = Some(AbLoop {
            track: e.idx,
            a: 0,
            b: 48_000,
        });
        assert!(
            !e.window_open(Some(480_000), Some(48_000)),
            "with a section on this track it stays shut"
        );
    }

    /// Unique per call, so parallel tests never share one.
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

        /// 16-bit stereo 48 kHz tone, with a stated length.
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

        fn missing(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    /// `consumed` frames into the first track, playing, flush pre-acked.
    fn ready_to_skip(e: &mut Engine, consumed: u64) -> Option<Source> {
        let source = e.open_at(0);
        assert!(source.is_some(), "the fixture opens");
        e.shared.playing.store(true, Ordering::Relaxed);
        e.shared.frames_consumed.store(consumed, Ordering::Relaxed);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);
        source
    }

    /// Points `audible_pos` at pool index `track`, so the cursor can lead it.
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

    /// Puts the runahead's samples in the ring. `set_audible` alone leaves it only open.
    fn set_runahead_ringed(engine: &mut Engine, track: usize) {
        engine.pushed_playable = 30;
        engine.shared.segments.lock().unwrap().push(Segment {
            at_frame: 20,
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
        // A four second window at 100_000; the clock flips two seconds in.
        engine.adopt(1, info, 0, 96_000);
        let flip = 100_000 + 96_000;
        // At the flip the incoming track is already two seconds in.
        assert_eq!(engine.shared.position_at(flip, 48_000), Some((1, 2.0)));
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
        // Keep segment 1, the newest reached, and the future ones.
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
        prune_segments(&mut segments, 50);
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn remove_non_runahead_entry_shifts_cursor_no_flush() {
        let mut e = test_engine(5);
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
        let removed_runahead = e.remove(2);
        assert!(!removed_runahead);
        assert_eq!(e.order.len(), 5, "audible entry stays");
    }

    #[test]
    fn remove_runahead_reports_and_reanchors() {
        let mut e = test_engine(5);
        set_audible(&e, 2);
        e.pos = 3;
        let removed_runahead = e.remove(3);
        assert!(
            removed_runahead,
            "the pre-decoded next track is the runahead"
        );
        // Re-anchored on the audible entry.
        assert_eq!(e.pos, 2);
        assert_eq!(e.order.len(), 4);
    }

    #[test]
    fn remove_many_reports_runahead_when_cursor_dropped() {
        let mut e = test_engine(6);
        set_audible(&e, 2);
        e.pos = 3;
        let removed_runahead = e.remove_many(&[3, 5]);
        assert!(removed_runahead);
        assert_eq!(e.order[e.pos].id, 2);
    }

    #[test]
    fn remove_many_no_runahead_when_cursor_kept() {
        let mut e = test_engine(6);
        set_audible(&e, 2);
        e.pos = 3;
        let removed_runahead = e.remove_many(&[4, 5]);
        assert!(!removed_runahead);
        assert_eq!(e.order[e.pos].id, 3);
    }

    #[test]
    fn insert_carries_groups_into_pool_and_snapshot() {
        let mut e = test_engine(2);
        let at = e.insert(
            Some(0),
            vec![local("a"), local("b"), local("c")],
            vec![Some(7), Some(7), None],
            Vec::new(),
            Vec::new(),
            true,
        );
        assert_eq!(at, Some(1));
        let snap = e.shared.queue_snapshot();
        let groups: Vec<Option<u64>> = snap.entries.iter().map(|en| en.group).collect();
        assert_eq!(groups, vec![None, Some(7), Some(7), None, None]);
    }

    /// A continuation batch (ADR 17) appends behind everything; the cursor and
    /// history stay put.
    #[test]
    fn a_continuation_batch_appends_behind_the_cursor() {
        let mut e = test_engine(4);
        e.pos = 2;
        let before: Vec<u64> = e.order.iter().map(|en| en.id).collect();
        let at = e.insert(
            None,
            vec![local("c0"), local("c1")],
            vec![Some(9), Some(9)],
            Vec::new(),
            Vec::new(),
            false,
        );
        assert_eq!(at, Some(4), "the batch lands at the end of the order");
        assert_eq!(e.pos, 2, "the playing entry never moves for an append");
        let after: Vec<u64> = e.order.iter().map(|en| en.id).collect();
        assert_eq!(&after[..4], &before[..], "nothing already queued shifted");
        let snap = e.shared.queue_snapshot();
        // Context, not queue.
        assert!(
            snap.entries[4..].iter().all(|en| !en.explicit),
            "an appended batch is context"
        );
        assert_eq!(e.queue.len(), 6);
        assert_eq!(e.groups[4..], [Some(9), Some(9)]);
        assert_eq!(e.shared.tracks.lock().unwrap().len(), 6);
    }

    /// An appended batch must be reachable by the Similar reorder the player
    /// sends right after it.
    #[test]
    fn a_batch_landing_under_shuffle_joins_the_upcoming_order() {
        let mut e = test_engine(4);
        e.pos = 2;
        e.insert(
            None,
            vec![local("c0"), local("c1"), local("c2"), local("c3")],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
        );
        let head: Vec<u64> = e.order[..=e.pos].iter().map(|en| en.id).collect();
        e.order_tail(&[6, 4]);
        assert_eq!(
            e.order[..=e.pos].iter().map(|en| en.id).collect::<Vec<_>>(),
            head
        );
        let upcoming: Vec<u64> = e.order[e.pos + 1..].iter().map(|en| en.id).collect();
        assert_eq!(
            upcoming,
            vec![6, 4, 3, 5, 7],
            "appended entries rank alongside the ones that were already queued"
        );
    }

    /// Appending to a played-out queue names the position to wake into (#36).
    #[test]
    fn appending_to_a_played_out_queue_names_the_track_to_wake_into() {
        let mut e = test_engine(2);
        e.pos = 1;
        let at = e.insert(
            None,
            vec![local("c0")],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
        );
        assert_eq!(at, Some(2), "the first appended entry is what plays next");
        assert_eq!(e.order[at.unwrap()].idx, 2);
    }

    #[test]
    fn insert_pads_missing_groups_with_none() {
        let mut e = test_engine(1);
        e.insert(
            None,
            vec![local("a"), local("b")],
            vec![Some(3)],
            vec![gain::ReplayGain {
                track_db: Some(-6.0),
                ..gain::ReplayGain::default()
            }],
            vec![Some(Span {
                start_ms: 1_000,
                end_ms: Some(3_000),
            })],
            false,
        );
        let snap = e.shared.queue_snapshot();
        assert_eq!(snap.entries[1].group, Some(3));
        assert_eq!(snap.entries[2].group, None);
        assert_eq!(e.gains[1].track_db, Some(-6.0));
        assert_eq!(e.gains[2], gain::ReplayGain::default());
        assert_eq!(e.spans[1].map(|s| s.start_ms), Some(1_000));
        assert_eq!(e.spans[2], None);
    }

    fn set_groups(engine: &mut Engine, groups: &[Option<u64>]) {
        engine.groups = groups.to_vec();
        engine.groups.resize(engine.queue.len(), None);
    }

    #[test]
    fn album_contiguous_boundary_keeps_its_splice() {
        let mut e = test_engine(3);
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
        assert_eq!(e.fade_window(Some(4 * 48_000)), 2 * 48_000);
        assert_eq!(e.fade_window(Some(300 * 48_000)), 8 * 48_000);
    }

    #[test]
    fn zero_seconds_disables_the_fade_entirely() {
        let mut e = test_engine(2);
        set_groups(&mut e, &[None, None]);
        e.fade_secs = 0.0;
        assert!(!e.window_open(Some(100), Some(0)));
    }

    #[test]
    fn fade_opens_inside_the_window_and_not_before() {
        let mut e = test_engine(2);
        set_groups(&mut e, &[None, None]);
        e.fade_secs = 2.0;
        let total = Some(60 * 48_000);
        assert!(!e.window_open(total, Some(3 * 48_000)));
        assert!(e.window_open(total, Some(48_000)));
    }

    #[test]
    fn fade_stays_shut_for_album_tracks_and_armed_stops() {
        let mut e = test_engine(2);
        e.fade_secs = 2.0;
        let (total, left) = (Some(60 * 48_000), Some(48_000));
        set_groups(&mut e, &[Some(9), Some(9)]);
        assert!(!e.window_open(total, left));
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
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 4.0)),
            local(fx.wav("b.wav", 4.0)),
        ]);
        e.fade_secs = 4.0;
        let source = ready_to_skip(&mut e, 48_000);

        let after = e.skip_to(source, 1, false, None);
        assert!(after.is_some(), "the second fixture opens");
        assert!(e.fade.is_some(), "the track left is under the new one");
        // Capped at half the four second track.
        assert_eq!(e.shared.fade_len.load(Ordering::Acquire), 96_000);
        let (progress, back) = e.shared.crossfade().expect("the transport shows it");
        assert_eq!(progress, 0.0, "the cut landed right on the window");
        assert!(!back);
    }

    #[test]
    fn a_skip_to_a_dead_end_leaves_no_fade_behind() {
        let fx = Fixtures::new("skip-dead-end");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 4.0)),
            local(fx.missing("gone.wav")),
        ]);
        e.fade_secs = 4.0;
        let source = ready_to_skip(&mut e, 48_000);

        let after = e.skip_to(source, 1, false, None);
        assert!(after.is_none());
        assert!(e.fade.is_none());
        // No source moves the frozen clock, so a published fade would never clear.
        assert_eq!(e.shared.fade_len.load(Ordering::Acquire), 0);
        assert!(e.shared.crossfade().is_none());
    }

    #[test]
    fn a_boundary_with_nothing_left_to_fade_splices_instead() {
        let fx = Fixtures::new("boundary-empty");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 4.0)),
            local(fx.wav("b.wav", 4.0)),
        ]);
        e.fade_secs = 4.0;
        let mut src = e.open_at(0).expect("the fixture opens");
        // The container says it's over while frames remain.
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
        assert_eq!(skip_fade_discard(0, 0, 48_000), Some(0));
        // 25 ms of flush drift plus a seek 100 ms short.
        assert_eq!(skip_fade_discard(1_200, 4_800, 48_000), Some(6_000));
        // An overshoot can't be decoded back.
        assert_eq!(skip_fade_discard(100, -4_800, 48_000), Some(0));
        assert_eq!(skip_fade_discard(0, 96_000, 48_000), None);
    }

    #[test]
    fn a_track_shorter_than_the_fade_closes_it_at_its_own_end() {
        let fx = Fixtures::new("short-track");
        let path = fx.wav("a.wav", 4.0);
        let mut e = engine_over(vec![local(&path)]);
        let (src, _) = Source::open(&local(&path), 48_000, None).expect("the fixture opens");
        // A twelve second window, five seconds in.
        let mut fade = Fade::new(src, 12 * 48_000);
        fade.done = 5 * 48_000;
        e.fade = Some(fade);

        e.close_fade_fast();
        let fade = e.fade.as_ref().expect("still mixing, just not for long");
        // 20 ms of ramp left, not seven more seconds under the next track.
        assert_eq!(fade.len, 5 * 48_000 + 960);
    }

    #[test]
    fn dropping_an_unmixed_fade_takes_its_publish_with_it() {
        let fx = Fixtures::new("drop-fade");
        let path = fx.wav("a.wav", 4.0);
        let mut e = engine_over(vec![local(&path)]);
        let (src, _) = Source::open(&local(&path), 48_000, None).expect("the fixture opens");
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
        // Off all the way down, so no skip pays for a wind-back.
        let mut e = test_engine(2);
        set_groups(&mut e, &[None, None]);
        e.fade_secs = crossfade_secs(f32::NAN);
        assert!(!e.window_open(Some(100), Some(0)));
    }

    /// Nothing to cue, but the pause still lands, or the next queue edit or batch
    /// would start audio against the stop.
    #[test]
    fn a_stop_with_nothing_to_cue_still_pauses() {
        let mut e = test_engine(2);
        e.pos = 1;
        e.stop_after = true;
        e.stop_pending = true;
        assert_eq!(e.land_stop(), None, "played out, nothing to cue");
        assert!(!e.shared.playing.load(Ordering::Relaxed), "the stop landed");
        assert!(!e.stop_pending, "and it only lands once");
    }

    #[test]
    fn a_stop_mid_queue_pauses_and_names_what_play_resumes() {
        let mut e = test_engine(3);
        e.pos = 1;
        e.stop_after = true;
        e.stop_pending = true;
        assert_eq!(e.land_stop(), Some(2));
        assert!(!e.shared.playing.load(Ordering::Relaxed));
    }

    /// Disarmed during the drain: rolls on.
    #[test]
    fn a_stop_disarmed_during_the_drain_rolls_on() {
        let mut e = test_engine(3);
        e.pos = 1;
        e.stop_after = false;
        e.stop_pending = true;
        assert_eq!(e.land_stop(), Some(2));
        assert!(e.shared.playing.load(Ordering::Relaxed), "no pause landed");
    }

    /// A batch (ADR 17) arriving while the last track drains must not cut the ending.
    #[test]
    fn a_skip_into_a_draining_ring_lets_the_ending_finish() {
        let fx = Fixtures::new("skip-draining");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 1.0)),
            local(fx.wav("b.wav", 1.0)),
        ]);
        // Four frames still queued, nothing decoding.
        for _ in 0..8 {
            e.producer.push(0.25).expect("room in the test ring");
        }
        e.pushed_playable = 1_000;
        e.shared.playing.store(true, Ordering::Relaxed);
        e.shared.frames_consumed.store(996, Ordering::Relaxed);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);
        let seq = e.shared.flush_seq.load(Ordering::Acquire);

        let after = e.skip_to(None, 1, false, None);
        assert!(after.is_some(), "the second fixture opens");
        assert_eq!(
            e.shared.flush_seq.load(Ordering::Acquire),
            seq,
            "no cut, so the backend keeps what it's holding"
        );
        assert_eq!(e.pushed_playable, 1_000, "the tail still counts as pushed");
        // Behind the tail, where it's really heard.
        let segments = e.shared.segments.lock().unwrap();
        assert_eq!(segments.last().map(|s| s.at_frame), Some(1_000));
    }

    /// An empty ring has nothing to protect, so the skip cuts.
    #[test]
    fn a_skip_out_of_a_drained_ring_still_cuts() {
        let fx = Fixtures::new("skip-drained");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 1.0)),
            local(fx.wav("b.wav", 1.0)),
        ]);
        e.pushed_playable = 1_000;
        e.shared.frames_consumed.store(1_000, Ordering::Relaxed);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);
        let seq = e.shared.flush_seq.load(Ordering::Acquire);

        assert!(e.skip_to(None, 1, false, None).is_some());
        assert!(e.shared.flush_seq.load(Ordering::Acquire) > seq, "cut");
    }

    /// A bookmark skip opens at the offset, and the segment says so from the
    /// first frame.
    #[test]
    fn a_skip_with_a_landing_opens_the_track_there() {
        let fx = Fixtures::new("skip-landing");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 1.0)),
            local(fx.wav("b.wav", 3.0)),
        ]);
        e.pushed_playable = 1_000;
        e.shared.frames_consumed.store(1_000, Ordering::Relaxed);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        let src = e
            .skip_to(None, 1, false, Some(1.5))
            .expect("the second fixture opens");
        // The landing is packet-coarse; the segment must match the source exactly.
        let landed = src.pos_frames;
        assert!(
            (landed as i64 - 72_000).abs() < 4_800,
            "the source stands about a second and a half in, got {landed}"
        );
        let segments = e.shared.segments.lock().unwrap();
        let last = segments.last().expect("the skip registered its track");
        assert_eq!(last.track, 1);
        assert_eq!(last.track_frame, landed);
        drop(segments);
        let src = e
            .skip_to(None, 0, false, None)
            .expect("the first fixture opens");
        assert_eq!(src.pos_frames, 0);
        let segments = e.shared.segments.lock().unwrap();
        assert_eq!(segments.last().map(|s| s.track_frame), Some(0));
    }

    /// A seek on a played-out queue reopens the finished track.
    #[test]
    fn a_seek_out_of_the_ended_state_reopens_the_finished_track() {
        let fx = Fixtures::new("seek-ended");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 1.0)),
            local(fx.wav("b.wav", 1.0)),
        ]);
        e.pos = 1;
        e.pushed_playable = 96_000;
        e.shared.frames_consumed.store(96_000, Ordering::Relaxed);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);
        e.shared.ended.store(true, Ordering::Relaxed);

        let after = e.seek_to(None, 0.5);

        assert!(after.is_some(), "the finished track opens again");
        assert!(
            !e.shared.ended.load(Ordering::Relaxed),
            "playing again, so nothing downstream still reads as finished"
        );
        assert_eq!(e.pos, 1, "the seek stays on the track that was showing");
        // Packet-coarse, so check the neighbourhood.
        let segments = e.shared.segments.lock().unwrap();
        let landed = segments.last().map(|s| s.track_frame).expect("a segment");
        assert!(
            landed.abs_diff(24_000) < 4_800,
            "the clock lands where the click asked, half a second in, got {landed}"
        );
    }

    /// A seek to exactly the duration must land inside the track, or the scrub
    /// ends it.
    #[test]
    fn a_seek_to_the_very_end_lands_inside_the_track() {
        let fx = Fixtures::new("seek-edge");
        let path = fx.wav("a.wav", 1.0);
        for target in [1.0, 1.5, 60.0] {
            let (mut src, info) =
                Source::open(&local(&path), 48_000, None).expect("the fixture opens");
            let landed = src.seek(target).expect("a seek inside the track");
            assert!(
                landed < info.duration_secs.expect("the fixture states its length"),
                "seeking to {target} landed on {landed}, at or past the end"
            );
            let mut chunk = Vec::new();
            assert!(
                src.next_chunk(48_000, &mut chunk) && !chunk.is_empty(),
                "seeking to {target} left the track with audio to play"
            );
        }
    }

    fn span(start_ms: u32, end_ms: Option<u32>) -> Span {
        Span { start_ms, end_ms }
    }

    /// 48 kHz fixtures at 48 kHz, so the samples come back bit for bit.
    fn decode_all(path: &PathBuf, span: Option<Span>) -> Vec<f32> {
        let (mut src, _) = Source::open(&local(path), 48_000, span).expect("the fixture opens");

        drain_source(&mut src)
    }

    fn drain_source(src: &mut Source) -> Vec<f32> {
        let mut out = Vec::new();
        let mut chunk = Vec::new();
        loop {
            chunk.clear();
            let more = src.next_chunk(48_000, &mut chunk);
            out.extend_from_slice(&chunk);
            if !more {
                break;
            }
        }

        out
    }

    /// The same bytes over the (fake) HTTP transport and off disk are the same
    /// track, header and samples.
    #[test]
    fn a_remote_open_reads_the_same_track_as_the_file() {
        let fx = Fixtures::new("remote-transparency");
        let path = fx.wav("tone.wav", 2.0);
        let bytes = std::fs::read(&path).expect("the fixture is readable");
        let served = crate::http::testing::Fake::serving(bytes, "audio/wav");

        let (mut local, local_info) =
            Source::open(&local(&path), 48_000, None).expect("the file opens");

        // Empty hint: the probe goes off the `Content-Type`, as with a real server.
        let url = Locator::Remote(Remote {
            url: "http://example.invalid/tone.wav".into(),
            headers: Vec::new(),
            hint: String::new(),
            live: false,
        });
        let (mut remote, remote_info) =
            crate::http::testing::with_transport(served, || Source::open(&url, 48_000, None))
                .expect("the stream opens");

        assert_eq!(remote_info.name, local_info.name, "same display name");
        assert_eq!(remote_info.sample_rate, local_info.sample_rate);
        assert_eq!(remote_info.channels, local_info.channels);
        assert_eq!(remote_info.num_frames, local_info.num_frames);
        assert_eq!(remote_info.duration_secs, local_info.duration_secs);

        assert_eq!(drain_source(&mut remote), drain_source(&mut local));
    }

    /// The revision counts songs, not metadata blocks, or scrobbles would fire
    /// every few seconds.
    #[test]
    fn a_station_title_reaches_the_published_slot() {
        let fx = Fixtures::new("icy-title");
        let path = fx.wav("stream.wav", 1.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");

        // A title, a repeat, a change, then hold.
        const METAINT: usize = 8192;
        let fake = live_fake(
            &wav,
            METAINT,
            &["Aphex Twin - Xtal", "Aphex Twin - Xtal", "Autechre - Rae"],
        );

        let url = Locator::Remote(Remote {
            url: "http://example.invalid/stream".into(),
            headers: Vec::new(),
            hint: "wav".into(),
            live: true,
        });
        let mut e = engine_over(vec![url]);

        let shared = Arc::clone(&e.shared);
        let opened = crate::http::testing::with_transport(fake.clone(), || {
            let mut src = e.open_at(0).expect("the stream opens");
            // The open's own states move the revision too, so count from here.
            let opened = shared.title_rev();

            // Titles fire as the cursor decodes past their marks, so this has to decode.
            // The WAV states its length, so the decode ends.
            let mut chunk = Vec::new();
            for _ in 0..10_000 {
                chunk.clear();
                if !src.next_chunk(48_000, &mut chunk) {
                    break;
                }
            }

            opened
        });

        assert_eq!(
            shared.live_title(0),
            Some(IcyTitle {
                artist: "Autechre".into(),
                title: "Rae".into(),
            }),
            "the last song the station named"
        );
        assert_eq!(
            shared.title_rev() - opened,
            2,
            "two songs, not one per metadata block"
        );
    }

    /// Read once at the open.
    #[test]
    fn a_station_description_reaches_the_published_slot() {
        let fx = Fixtures::new("icy-headers");
        let path = fx.wav("stream.wav", 0.25);
        let wav = std::fs::read(&path).expect("the fixture is readable");

        let mut fake = live_fake(&wav, 8192, &["Jazz Forever - the standards"]);
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.station = crate::http::StationInfo {
                name: "Jazz Forever".into(),
                genre: "Jazz".into(),
                bitrate_kbps: 128,
                homepage: "https://jazzforever.example".into(),
                description: "All the standards, all night".into(),
                content_type: String::new(),
            };
        }

        let url = Locator::Remote(Remote {
            url: "http://example.invalid/stream".into(),
            headers: Vec::new(),
            hint: "wav".into(),
            live: true,
        });
        let mut e = engine_over(vec![url]);

        let shared = Arc::clone(&e.shared);
        crate::http::testing::with_transport(fake, || {
            e.open_at(0).expect("the stream opens");
        });

        let info = shared
            .station_info(0)
            .expect("the station described itself");
        assert_eq!(info.name, "Jazz Forever");
        assert_eq!(info.genre, "Jazz");
        assert_eq!(info.bitrate_kbps, 128);
        assert_eq!(info.homepage, "https://jazzforever.example");
        assert_eq!(info.content_type, "audio/wav", "the codec the row records");
        // Open starting, open landing, and the description: three bumps.
        assert_eq!(shared.title_rev(), 3, "the description moves the revision");
        assert_eq!(shared.stream_state(0), Some(StreamState::Live));
    }

    fn station(url: &str) -> Locator {
        Locator::Remote(Remote {
            url: url.into(),
            headers: Vec::new(),
            hint: "wav".into(),
            live: true,
        })
    }

    /// A seekable remote, like a Subsonic stream.
    fn hosted(url: &str) -> Locator {
        Locator::Remote(Remote {
            url: url.into(),
            headers: Vec::new(),
            hint: "wav".into(),
            live: false,
        })
    }

    /// 200, no length, in-band `titles`.
    fn live_fake(wav: &[u8], metaint: usize, titles: &[&str]) -> Arc<crate::http::testing::Fake> {
        // Padded so the stream outlasts the decode; an ending fake would reconnect
        // behind every assertion.
        let mut served = wav.to_vec();
        served.resize(wav.len() * 2, 0);

        let body = crate::http::testing::interleave(&served, metaint, titles);
        let mut fake = crate::http::testing::Fake::serving(body, "audio/wav");
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            f.metaint = Some(metaint);
            // Paced so the window never rolls over the cursor mid-test.
            f.pace = Some((StdDuration::from_millis(1), 1024));
        }

        fake
    }

    /// Decode until a title lands in entry 0's slot (the revision also moves on
    /// stream states).
    fn decode_until_titled(e: &Engine, src: &mut Source, chunk: &mut Vec<f32>) {
        for _ in 0..500 {
            if e.shared.live_title(0).is_some() {
                break;
            }
            chunk.clear();
            if !src.next_chunk(48_000, chunk) {
                break;
            }
        }

        assert!(
            e.shared.live_title(0).is_some(),
            "the fixture never carried a metadata block"
        );
    }

    /// Pausing a station keeps the connection, the taping, and the ring.
    #[test]
    fn pausing_a_live_station_keeps_the_connection_and_the_ring() {
        const METAINT: usize = 4096;
        let fx = Fixtures::new("live-pause");
        let path = fx.wav("stream.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, METAINT, &["Boards of Canada - Roygbiv"]);

        let (mut e, ring) = engine_with_ring(vec![station("http://example.invalid/live")], 64);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        let shared = Arc::clone(&e.shared);
        crate::http::testing::with_transport(fake.clone(), || {
            let mut source = e.open_at(0);
            assert!(source.is_some(), "the station opens");

            let mut chunk = Vec::new();
            decode_until_titled(&e, source.as_mut().unwrap(), &mut chunk);
            source.as_mut().unwrap().next_chunk(48_000, &mut e.pending);
            for i in 0..8 {
                let s = e.pending[i];
                e.producer.push(s).expect("the ring has room");
            }
            let held = ring.slots();
            assert!(held > 0, "the ring holds what was decoded");

            let tape = source
                .as_ref()
                .and_then(|src| src.tape.clone())
                .expect("a station tapes");
            let taped = tape.shift().window_secs;

            shared.playing.store(false, Ordering::Relaxed);
            source = e.idle_hangup(source);

            assert!(source.is_some(), "still connected");
            assert_eq!(fake.closed_count(), 0, "the socket is still open");
            assert_eq!(fake.ask_count(), 1, "and nothing reopened anything");
            assert_eq!(ring.slots(), held, "the ring kept the pre-pause audio");
            assert!(e.hung_up.is_none(), "there's nothing to come back to");

            wait_for("the tape to keep filling", || {
                tape.shift().window_secs > taped
            });

            assert!(shared.tracks.lock().unwrap()[0].is_some());
            assert!(shared.live_title(0).is_some());
            assert!(
                !shared.ended.load(Ordering::Relaxed),
                "a pause is not an end"
            );
        });
    }

    /// Past the idle cap the socket goes.
    #[test]
    fn a_station_paused_past_the_idle_cap_hangs_up() {
        const METAINT: usize = 4096;
        let fx = Fixtures::new("live-idle");
        let path = fx.wav("stream.wav", 1.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, METAINT, &["Boards of Canada - Roygbiv"]);

        let mut e = engine_over(vec![station("http://example.invalid/live")]);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        let shared = Arc::clone(&e.shared);
        crate::http::testing::with_transport(fake.clone(), || {
            let mut source = e.open_at(0);
            let mut chunk = Vec::new();
            decode_until_titled(&e, source.as_mut().unwrap(), &mut chunk);

            // The pause dated back past the cap.
            shared.frames_consumed.store(48_000, Ordering::Relaxed);
            shared.playing.store(false, Ordering::Relaxed);
            source = e.idle_hangup(source);
            assert!(source.is_some(), "a fresh pause holds the connection");

            e.paused_since =
                Some(Instant::now() - StdDuration::from_secs(LIVE_IDLE_HANGUP_SECS + 1));
            source = e.idle_hangup(source);

            assert!(source.is_none(), "the source went, and the socket with it");
            assert_eq!(e.hung_up, Some(48_000), "with the clock where it stopped");
            wait_for("the body to close", || fake.closed_count() == 1);

            assert!(shared.tracks.lock().unwrap()[0].is_some());
            assert!(shared.live_title(0).is_some());
            assert!(!shared.ended.load(Ordering::Relaxed));
        });
    }

    /// A timeshift seek costs a decoder, nothing over the wire. The WAV fixture
    /// can only be probed from its header, so this lands at the top of the tape;
    /// mid-stream MP3 or AAC re-sync is verified by ear.
    #[test]
    fn a_live_seek_moves_the_cursor_back_through_the_tape() {
        const METAINT: usize = 4096;
        let fx = Fixtures::new("live-seek");
        let path = fx.wav("stream.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, METAINT, &["Boards of Canada - Roygbiv"]);

        let mut e = engine_over(vec![station("http://example.invalid/live")]);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        crate::http::testing::with_transport(fake.clone(), || {
            let mut source = e.open_at(0);
            let mut chunk = Vec::new();
            decode_until_titled(&e, source.as_mut().unwrap(), &mut chunk);

            for _ in 0..40 {
                chunk.clear();
                if !source.as_mut().unwrap().next_chunk(48_000, &mut chunk) {
                    break;
                }
            }

            let tape = source
                .as_ref()
                .and_then(|src| src.tape.clone())
                .expect("a station tapes");
            let was = tape.cursor();
            assert!(was > 0, "something has been read");

            // Further back than held: the oldest byte.
            source = e.seek_live_to(source, 3600.0);

            assert!(source.is_some(), "still playing");
            assert!(
                tape.cursor() < was,
                "the cursor went back: {} from {was}",
                tape.cursor()
            );
            assert_eq!(fake.ask_count(), 1, "and asked the station for nothing");

            // Playing on proves the decoder was rebuilt.
            chunk.clear();
            assert!(source.as_mut().unwrap().next_chunk(48_000, &mut chunk));
            assert!(!chunk.is_empty(), "audio out of the new cursor");
        });
    }

    /// A mid-stream seek may re-sync or be refused; either way the station keeps
    /// playing. Which one isn't knowable from here.
    #[test]
    fn a_live_seek_into_the_middle_of_the_stream_keeps_playing() {
        const METAINT: usize = 4096;
        let fx = Fixtures::new("live-seek-refused");
        let path = fx.wav("stream.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, METAINT, &["Boards of Canada - Roygbiv"]);

        let mut e = engine_over(vec![station("http://example.invalid/live")]);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        crate::http::testing::with_transport(fake.clone(), || {
            let mut source = e.open_at(0);
            let mut chunk = Vec::new();
            decode_until_titled(&e, source.as_mut().unwrap(), &mut chunk);
            for _ in 0..40 {
                chunk.clear();
                if !source.as_mut().unwrap().next_chunk(48_000, &mut chunk) {
                    break;
                }
            }

            let tape = source
                .as_ref()
                .and_then(|src| src.tape.clone())
                .expect("a station tapes");

            // Measured off the cursor: the feed fills faster than the decode drains.
            let behind = tape.shift().behind_secs + 0.5;
            source = e.seek_live_to(source, behind);

            assert!(source.is_some(), "still playing");
            assert_eq!(fake.ask_count(), 1, "and nothing dialled the station");

            chunk.clear();
            assert!(source.as_mut().unwrap().next_chunk(48_000, &mut chunk));
            assert!(!chunk.is_empty(), "audio carries on");
        });
    }

    /// A device fault under a station swaps only the ring.
    #[test]
    fn a_device_swap_keeps_the_station_on_its_own_tape() {
        const METAINT: usize = 4096;
        let fx = Fixtures::new("live-swap");
        let path = fx.wav("stream.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, METAINT, &["Boards of Canada - Roygbiv"]);

        let (mut e, _ring) = engine_with_ring(vec![station("http://example.invalid/live")], 64);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        crate::http::testing::with_transport(fake.clone(), || {
            let mut source = e.open_at(0);
            let mut chunk = Vec::new();
            decode_until_titled(&e, source.as_mut().unwrap(), &mut chunk);
            for _ in 0..40 {
                chunk.clear();
                if !source.as_mut().unwrap().next_chunk(48_000, &mut chunk) {
                    break;
                }
            }

            let tape = source
                .as_ref()
                .and_then(|src| src.tape.clone())
                .expect("a station tapes");
            let window = tape.shift().window_secs;

            let (producer, mut fresh) = rtrb::RingBuffer::<f32>::new(64 * 2);
            source = e.swap_output(producer, source);

            assert!(source.is_some(), "the station is still playing");
            assert_eq!(fake.ask_count(), 1, "nothing dialled the station again");
            assert_eq!(fake.closed_count(), 0, "and the socket never closed");
            assert!(
                Arc::ptr_eq(
                    &tape,
                    source.as_ref().and_then(|src| src.tape.as_ref()).unwrap()
                ),
                "the same tape, not a new one"
            );

            e.producer.push(0.5).expect("the new ring has room");
            assert_eq!(fresh.pop().ok(), Some(0.5));

            // The same feed thread, which a re-dial would have restarted.
            wait_for("the tape to keep filling", || {
                tape.shift().window_secs > window
            });

            chunk.clear();
            assert!(source.as_mut().unwrap().next_chunk(48_000, &mut chunk));
            assert!(!chunk.is_empty(), "audio carries on out of the new cursor");
        });
    }

    /// A swap while paused doesn't reset the idle clock.
    #[test]
    fn a_device_swap_leaves_the_idle_hangup_clock_alone() {
        const METAINT: usize = 4096;
        let fx = Fixtures::new("live-swap-paused");
        let path = fx.wav("stream.wav", 1.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, METAINT, &["Boards of Canada - Roygbiv"]);

        let mut e = engine_over(vec![station("http://example.invalid/live")]);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        let shared = Arc::clone(&e.shared);
        crate::http::testing::with_transport(fake.clone(), || {
            let mut source = e.open_at(0);
            let mut chunk = Vec::new();
            decode_until_titled(&e, source.as_mut().unwrap(), &mut chunk);

            shared.playing.store(false, Ordering::Relaxed);
            source = e.idle_hangup(source);
            let since = Instant::now() - StdDuration::from_secs(LIVE_IDLE_HANGUP_SECS - 5);
            e.paused_since = Some(since);

            let (producer, _fresh) = rtrb::RingBuffer::<f32>::new(64 * 2);
            source = e.swap_output(producer, source);

            assert!(source.is_some(), "the pause still holds the connection");
            assert_eq!(e.paused_since, Some(since), "the pause clock didn't move");
            assert_eq!(fake.ask_count(), 1, "and nothing was dialled");

            e.paused_since = Some(Instant::now() - StdDuration::from_secs(LIVE_IDLE_HANGUP_SECS));
            source = e.idle_hangup(source);
            assert!(source.is_none(), "the cap still hangs up");
        });
    }

    /// A swap while hung up never redials.
    #[test]
    fn a_device_swap_while_hung_up_dials_nothing() {
        const METAINT: usize = 4096;
        let fx = Fixtures::new("live-swap-hung-up");
        let path = fx.wav("stream.wav", 1.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, METAINT, &["Boards of Canada - Roygbiv"]);

        let mut e = engine_over(vec![station("http://example.invalid/live")]);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        let shared = Arc::clone(&e.shared);
        crate::http::testing::with_transport(fake.clone(), || {
            let mut source = e.open_at(0);
            let mut chunk = Vec::new();
            decode_until_titled(&e, source.as_mut().unwrap(), &mut chunk);

            shared.frames_consumed.store(48_000, Ordering::Relaxed);
            shared.playing.store(false, Ordering::Relaxed);
            e.paused_since =
                Some(Instant::now() - StdDuration::from_secs(LIVE_IDLE_HANGUP_SECS + 1));
            source = e.idle_hangup(source);
            assert!(source.is_none(), "the cap hung up");
            assert_eq!(e.hung_up, Some(48_000));

            let asked = fake.ask_count();
            let (producer, _fresh) = rtrb::RingBuffer::<f32>::new(64 * 2);
            source = e.swap_output(producer, source);

            assert!(source.is_none(), "still nothing open");
            assert_eq!(e.hung_up, Some(48_000), "and still waiting on a Play");
            assert_eq!(fake.ask_count(), asked, "nothing was dialled");
        });
    }

    /// On a file the swap seeks back to the clock: the dead ring's half second
    /// was never heard.
    #[test]
    fn a_device_swap_on_a_file_resumes_where_the_clock_stopped() {
        let fx = Fixtures::new("file-swap");
        let path = fx.wav("track.wav", 4.0);

        let (mut e, _ring) = engine_with_ring(vec![local(path)], 64);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        let mut source = e.open_at(0);
        assert!(source.is_some(), "the file opens");

        // The decoder well past what was heard, as when a device faults.
        let mut chunk = Vec::new();
        for _ in 0..40 {
            chunk.clear();
            if !source.as_mut().unwrap().next_chunk(48_000, &mut chunk) {
                break;
            }
        }
        e.pushed_playable = 96_000;
        e.shared.frames_consumed.store(48_000, Ordering::Relaxed);
        let (_, was) = e.shared.position(48_000).expect("the clock reads");
        assert!((was - 1.0).abs() < 0.01, "a second in: {was}");

        let (producer, _fresh) = rtrb::RingBuffer::<f32>::new(64 * 2);
        source = e.swap_output(producer, source);

        assert!(source.is_some(), "the file is still playing");
        // Packet-coarse; the clock follows the landing.
        let (_, now) = e.shared.position(48_000).expect("the clock still reads");
        assert!(
            (now - was).abs() < 0.05,
            "the position carried over: {now} from {was}"
        );
        assert!(
            source.as_ref().unwrap().pos_frames.abs_diff(48_000) < 2_400,
            "and the decoder went back to it: {}",
            source.as_ref().unwrap().pos_frames
        );

        chunk.clear();
        assert!(source.as_mut().unwrap().next_chunk(48_000, &mut chunk));
        assert!(!chunk.is_empty(), "audio carries on");
    }

    /// Rejoin is one fresh open, and the elapsed clock resumes where it froze.
    #[test]
    fn resuming_a_hung_up_station_reopens_it_once() {
        const METAINT: usize = 4096;
        let fx = Fixtures::new("live-resume");
        let path = fx.wav("stream.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, METAINT, &["Boards of Canada - Roygbiv"]);

        let mut e = engine_over(vec![station("http://example.invalid/live")]);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        let shared = Arc::clone(&e.shared);
        crate::http::testing::with_transport(fake.clone(), || {
            let mut source = e.open_at(0);
            let mut chunk = Vec::new();
            decode_until_titled(&e, source.as_mut().unwrap(), &mut chunk);
            let titled = shared.live_title(0);

            shared.frames_consumed.store(48_000, Ordering::Relaxed);
            shared.playing.store(false, Ordering::Relaxed);
            source = e.hang_up(source.take());
            assert!(source.is_none() && e.hung_up == Some(48_000));

            // The reopen's own two states move the revision too.
            let paused_at = shared.title_rev();
            source = e.rejoin();

            assert!(source.is_some(), "the station comes back");
            assert!(e.hung_up.is_none(), "and the pause state is spent");
            assert!(shared.playing.load(Ordering::Relaxed), "playing again");
            assert_eq!(fake.ask_count(), 2, "exactly one new request");
            // The same request a first open makes; no-range is only for reconnects.
            let range = fake.asks.lock().unwrap()[1].range;
            assert_eq!(range, Some(0));

            assert_eq!(shared.position(48_000), Some((0, 1.0)));

            // Same song, so the revision moved only for the reopen.
            assert_eq!(shared.live_title(0), titled);
            assert_eq!(
                shared.title_rev() - paused_at,
                2,
                "the reopen's two states, and no song change under them"
            );
        });
    }

    /// Records the session's state at each request and answers slowly, to see
    /// into the gap between a play now and a station's first byte.
    struct Slow {
        inner: Arc<crate::http::testing::Fake>,
        shared: Arc<Shared>,
        at_get: std::sync::Mutex<Vec<AtGet>>,
    }

    #[derive(Clone, Copy)]
    struct AtGet {
        playing: bool,
        flushed: u64,
        /// What the position clock named: what upstream calls "loaded".
        position: Option<(usize, f64)>,
    }

    impl crate::http::Http for Slow {
        fn get(
            &self,
            url: &str,
            headers: &[(String, String)],
            range: Option<u64>,
        ) -> Result<crate::http::Resp, String> {
            self.at_get.lock().unwrap().push(AtGet {
                playing: self.shared.playing.load(Ordering::Relaxed),
                flushed: self.shared.flush_seq.load(Ordering::Relaxed),
                position: self.shared.position(48_000),
            });
            std::thread::sleep(StdDuration::from_millis(50));

            self.inner.get(url, headers, range)
        }
    }

    /// For tests driving the run loop from another thread. The deadline only
    /// catches a broken build.
    fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + StdDuration::from_secs(10);
        while Instant::now() < deadline {
            if cond() {
                return;
            }
            std::thread::sleep(StdDuration::from_millis(2));
        }

        panic!("timed out waiting for {what}");
    }

    /// Play now onto a station from a pause: the resume must wait for the flush,
    /// or the paused local track plays through the station's slow open. Drives
    /// the real run loop, since the ordering is the loop's.
    #[test]
    fn a_play_now_onto_a_station_stays_silent_until_the_ring_is_cut() {
        let fx = Fixtures::new("play-now-paused");
        let path = fx.wav("local.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, 4096, &["Boards of Canada - Roygbiv"]);

        let shared = Arc::new(Shared::new(1));
        // No backend; an ack from the future clears the flush wait.
        shared.flush_ack.store(u64::MAX, Ordering::Release);
        let (producer, _consumer) = rtrb::RingBuffer::<f32>::new(8192);
        let (tx, rx) = mpsc::channel::<Cmd>();
        let engine = Engine::new(
            StartQueue {
                locators: vec![local(&path)],
                ..StartQueue::default()
            },
            Arc::clone(&shared),
            producer,
            48_000,
            rx,
        );

        let watch = Arc::new(Slow {
            inner: fake,
            shared: Arc::clone(&shared),
            at_get: std::sync::Mutex::new(Vec::new()),
        });
        let driver: Arc<dyn crate::http::Http> = Arc::clone(&watch) as Arc<dyn crate::http::Http>;
        let decode = std::thread::spawn(move || {
            crate::http::testing::with_transport(driver, || engine.run());
        });

        tx.send(Cmd::TogglePause).expect("the engine is listening");
        wait_for("the pause to land", || {
            !shared.playing.load(Ordering::Relaxed)
        });

        tx.send(Cmd::Insert {
            after: None,
            locators: vec![station("http://example.invalid/live")],
            groups: Vec::new(),
            gains: Vec::new(),
            spans: Vec::new(),
            explicit: true,
            and_play: true,
            start_secs: None,
        })
        .expect("the engine is listening");

        wait_for("the station's request to go out", || {
            !watch.at_get.lock().unwrap().is_empty()
        });
        let at = watch.at_get.lock().unwrap()[0];

        assert!(
            !at.playing,
            "the callback was still silent when the request went out"
        );
        assert_eq!(
            at.flushed, 0,
            "and the ring still held the paused track, which is exactly why"
        );
        // The clock names the station before its request leaves, so upstream is
        // already looking at the entry whose `Opening` gets published.
        assert_eq!(
            at.position,
            Some((1, 0.0)),
            "the clock had already moved to the station"
        );

        wait_for("the resume", || shared.playing.load(Ordering::Relaxed));
        assert!(
            shared.flush_seq.load(Ordering::Relaxed) > 0,
            "the ring was cut"
        );

        // The inserted entry has a slot in everything published per pool entry.
        assert_eq!(
            shared.stream_state(1),
            Some(StreamState::Live),
            "the inserted entry has somewhere to publish"
        );

        tx.send(Cmd::Quit).expect("the engine is listening");
        decode.join().expect("the decode thread ends cleanly");
    }

    /// The buffer setting moved with a station on air reaches its tape, the
    /// shift, and later stations, floored like the start.
    #[test]
    fn the_live_buffer_command_recaps_the_station_on_air() {
        let fx = Fixtures::new("live-buffer-cmd");
        let path = fx.wav("stream.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, 4096, &["Boards of Canada - Roygbiv"]);

        let shared = Arc::new(Shared::new(1));
        shared.flush_ack.store(u64::MAX, Ordering::Release);
        let (producer, _consumer) = rtrb::RingBuffer::<f32>::new(8192);
        let (tx, rx) = mpsc::channel::<Cmd>();
        let engine = Engine::new(
            StartQueue {
                locators: vec![station("http://example.invalid/live")],
                live_buffer_secs: 600,
                ..StartQueue::default()
            },
            Arc::clone(&shared),
            producer,
            48_000,
            rx,
        );

        let driver = Arc::clone(&fake);
        let decode = std::thread::spawn(move || {
            crate::http::testing::with_transport(driver, || engine.run());
        });

        wait_for(
            "the station to come on at the length it started with",
            || shared.shift(0).is_some_and(|shift| shift.cap_secs == 600.0),
        );

        tx.send(Cmd::SetLiveBuffer(60))
            .expect("the engine is listening");
        wait_for("the tape to take the new length", || {
            shared.shift(0).is_some_and(|shift| shift.cap_secs == 60.0)
        });

        tx.send(Cmd::SetLiveBuffer(1))
            .expect("the engine is listening");
        wait_for("the floor to hold under it", || {
            shared.shift(0).is_some_and(|shift| shift.cap_secs == 30.0)
        });

        tx.send(Cmd::Quit).expect("the engine is listening");
        decode.join().expect("the decode thread ends cleanly");
    }

    /// A paused launch restore on a station dials nothing until Play.
    #[test]
    fn a_paused_start_on_a_station_waits_for_play_before_it_connects() {
        let fx = Fixtures::new("paused-start");
        let path = fx.wav("stream.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, 4096, &["Boards of Canada - Roygbiv"]);

        let shared = Arc::new(Shared::new(1));
        shared.flush_ack.store(u64::MAX, Ordering::Release);
        // Paused before the decode thread starts, as the player does it.
        shared.playing.store(false, Ordering::Relaxed);
        let (producer, _consumer) = rtrb::RingBuffer::<f32>::new(8192);
        let (tx, rx) = mpsc::channel::<Cmd>();
        let engine = Engine::new(
            StartQueue {
                locators: vec![station("http://example.invalid/live")],
                ..StartQueue::default()
            },
            Arc::clone(&shared),
            producer,
            48_000,
            rx,
        );

        let driver = Arc::clone(&fake);
        let decode = std::thread::spawn(move || {
            crate::http::testing::with_transport(driver, || engine.run());
        });

        // Publish comes first, the open second.
        wait_for("the session to come up", || shared.queue_rev() > 0);
        assert_eq!(
            fake.ask_count(),
            0,
            "a restore that came up paused asked the station for nothing"
        );
        // Loaded at zero, so there's something to press Play on.
        assert_eq!(shared.position(48_000), Some((0, 0.0)));
        assert_eq!(
            shared.stream_state(0),
            None,
            "nothing opened, nothing to say"
        );

        tx.send(Cmd::TogglePause).expect("the engine is listening");
        // Wait on the open, not the flag: the rejoin sets playing before dialing.
        wait_for("the station to come on", || {
            shared.stream_state(0) == Some(StreamState::Live)
        });

        assert!(shared.playing.load(Ordering::Relaxed));
        assert_eq!(fake.ask_count(), 1, "and Play is one open, not two");

        tx.send(Cmd::Quit).expect("the engine is listening");
        decode.join().expect("the decode thread ends cleanly");
    }

    /// A seekable remote pauses like a file.
    #[test]
    fn a_seekable_remote_pauses_the_way_a_file_does() {
        let fx = Fixtures::new("seekable-pause");
        let path = fx.wav("track.wav", 1.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = crate::http::testing::Fake::serving(wav, "audio/wav");

        let mut e = engine_over(vec![hosted("http://example.invalid/track.wav")]);
        crate::http::testing::with_transport(fake.clone(), || {
            let source = e.open_at(0);
            assert!(source.is_some(), "the stream opens");
            // The probe's rewinds can make requests, so measure the pause as a difference.
            let (asks, closed) = (fake.ask_count(), fake.closed_count());

            e.shared.playing.store(false, Ordering::Relaxed);
            let source = e.hang_up(source);

            assert!(source.is_some(), "the source is handed straight back");
            assert!(e.hung_up.is_none(), "nothing to come back to");
            assert_eq!(fake.closed_count(), closed, "the connection is still up");
            assert_eq!(fake.ask_count(), asks, "and it asked for nothing new");
            assert_eq!(
                e.shared.flush_seq.load(Ordering::Relaxed),
                0,
                "no flush, so the ring keeps what it was playing"
            );
        });
    }

    #[test]
    fn a_local_file_pauses_the_way_it_always_has() {
        let fx = Fixtures::new("file-pause");
        let path = fx.wav("track.wav", 1.0);

        let mut e = engine_over(vec![local(&path)]);
        let source = e.open_at(0);
        assert!(source.is_some(), "the fixture opens");

        e.shared.playing.store(false, Ordering::Relaxed);
        let source = e.hang_up(source);

        assert!(source.is_some(), "nothing here to hang up on");
        assert!(e.hung_up.is_none());
        assert_eq!(e.shared.flush_seq.load(Ordering::Relaxed), 0, "no flush");
    }

    /// A spanned source reports the slice's length, starts at 0:00, and plays
    /// the slice.
    #[test]
    fn a_span_opens_at_its_start_and_reports_its_own_length() {
        let fx = Fixtures::new("span-open");
        let path = fx.wav("image.wav", 4.0);
        let (src, info) = Source::open(&local(&path), 48_000, Some(span(1_000, Some(3_000))))
            .expect("the image opens");

        assert_eq!(info.duration_secs, Some(2.0), "the span's length, not 4s");
        assert_eq!(info.num_frames, Some(96_000));
        assert_eq!(src.total_frames, Some(96_000));
        assert_eq!(src.pos_frames, 0, "the clock starts at the span's own 0:00");
        drop(src);

        let whole = decode_all(&path, None);
        let spanned = decode_all(&path, Some(span(1_000, Some(3_000))));
        assert_eq!(spanned, whole[48_000 * 2..144_000 * 2]);
    }

    /// Cut on the frame the sheet named, not the packet end.
    #[test]
    fn a_span_ends_on_its_boundary_frame() {
        let fx = Fixtures::new("span-boundary");
        let path = fx.wav("image.wav", 4.0);
        // 1234 ms is 59_232 frames, mid-packet whatever the block size.
        let spanned = decode_all(&path, Some(span(0, Some(1_234))));
        assert_eq!(spanned.len() / 2, 59_232, "cut on the exact frame");

        let whole = decode_all(&path, None);
        assert_eq!(spanned, whole[..59_232 * 2]);
    }

    /// Two cue tracks back to back are the image, nothing dropped or doubled.
    #[test]
    fn two_spans_of_one_image_cover_it_end_to_end() {
        let fx = Fixtures::new("span-adjacent");
        let path = fx.wav("image.wav", 4.0);
        let first = decode_all(&path, Some(span(0, Some(2_000))));
        let second = decode_all(&path, Some(span(2_000, Some(4_000))));
        assert_eq!(first.len() / 2, 96_000);
        assert_eq!(second.len() / 2, 96_000);

        let mut spliced = first;
        spliced.extend_from_slice(&second);
        let whole = decode_all(&path, None);
        assert_eq!(spliced.len() / 2, 192_000, "the two spans are the image");
        assert_eq!(spliced, whole);
    }

    /// Seeks inside a span are track-relative and clamp to the span.
    #[test]
    fn seeking_inside_a_span_stays_inside_it_and_reads_track_relative() {
        let fx = Fixtures::new("span-seek");
        let path = fx.wav("image.wav", 4.0);
        let (mut src, _) = Source::open(&local(&path), 48_000, Some(span(1_000, Some(3_000))))
            .expect("the image opens");

        // Packet-coarse, but on the track's clock, not the image's.
        let landed = src.seek(0.5).expect("the wav seeks");
        assert!(
            (landed - 0.5).abs() < 0.05,
            "landed at {landed}, which is not half a second into the track"
        );
        assert_eq!(src.pos_frames, (landed * 48_000.0).round() as u64);
        assert!(src.pos_frames > 0 && src.pos_frames < 96_000);

        // Clamped short of the span's end by the seek margin.
        let landed = src.seek(30.0).expect("the wav seeks");
        assert!(landed <= 2.0, "landed at {landed}, past the span's end");
        assert!(
            2.0 - landed < SEEK_END_MARGIN_SECS + 0.05,
            "landed at {landed}, well short of the span's end"
        );
        let left = src.remaining().expect("the span states its length");
        // Exactly the span's tail is left: a coarse landing shortens the decode.
        let mut decoded = 0u64;
        let mut chunk = Vec::new();
        loop {
            chunk.clear();
            let more = src.next_chunk(48_000, &mut chunk);
            decoded += (chunk.len() / 2) as u64;
            if !more {
                break;
            }
        }
        assert_eq!(decoded, left);
    }

    /// An open-ended span runs to the file's end.
    #[test]
    fn an_open_ended_span_runs_to_the_files_end() {
        let fx = Fixtures::new("span-open-end");
        let path = fx.wav("image.wav", 4.0);
        let (src, info) =
            Source::open(&local(&path), 48_000, Some(span(3_000, None))).expect("it opens");
        assert_eq!(info.duration_secs, Some(1.0));
        assert_eq!(info.num_frames, Some(48_000));
        assert_eq!(src.total_frames, Some(48_000));
        drop(src);

        let tail = decode_all(&path, Some(span(3_000, None)));
        let whole = decode_all(&path, None);
        assert_eq!(tail.len() / 2, 48_000);
        assert_eq!(tail, whole[144_000 * 2..]);
    }

    /// Each pool entry opens its own slice of one image.
    #[test]
    fn the_pool_opens_each_entry_at_its_own_span() {
        let fx = Fixtures::new("span-pool");
        let path = fx.wav("image.wav", 4.0);
        let shared = Arc::new(Shared::new(2));
        let (producer, _consumer) = rtrb::RingBuffer::<f32>::new(16);
        let (_tx, rx) = mpsc::channel::<Cmd>();
        let mut e = Engine::new(
            StartQueue {
                locators: vec![local(&path), local(&path)],
                spans: vec![Some(span(0, Some(1_000))), Some(span(1_000, None))],
                ..StartQueue::default()
            },
            shared,
            producer,
            48_000,
            rx,
        );

        let first = e.open_at(0).expect("the image opens");
        assert_eq!(first.total_frames, Some(48_000));
        drop(first);
        let second = e.open_at(1).expect("the image opens again");
        assert_eq!(second.total_frames, Some(144_000));
        let tracks = e.shared.tracks.lock().unwrap();
        assert_eq!(tracks[0].as_ref().and_then(|t| t.duration_secs), Some(1.0));
        assert_eq!(tracks[1].as_ref().and_then(|t| t.duration_secs), Some(3.0));
    }

    #[test]
    fn remove_many_keeps_audible_even_if_named() {
        let mut e = test_engine(5);
        set_audible(&e, 1);
        e.pos = 1;
        let _ = e.remove_many(&[0, 1, 2]);
        assert!(
            e.order.iter().any(|entry| entry.id == 1),
            "audible entry kept"
        );
    }

    /// Panics like a real codec did: an arithmetic overflow inside, on both
    /// entry points.
    struct PanickingDecoder;

    impl AudioDecoder for PanickingDecoder {
        fn reset(&mut self) {
            panic!("attempt to shift left with overflow");
        }

        fn decode_ref(
            &mut self,
            _packet: &symphonia::core::packet::PacketRef<'_>,
        ) -> symphonia::core::errors::Result<symphonia::core::audio::GenericAudioBufferRef<'_>>
        {
            panic!("attempt to shift left with overflow");
        }

        fn codec_info(&self) -> &symphonia::core::codecs::CodecInfo {
            unimplemented!("a panicking decoder is never asked what it is")
        }

        fn codec_params(&self) -> &symphonia::core::codecs::audio::AudioCodecParameters {
            unimplemented!("a panicking decoder is never asked what it is")
        }

        fn finalize(&mut self) -> symphonia::core::codecs::audio::FinalizeResult {
            unimplemented!("nothing finishes a decode that panicked")
        }

        fn last_decoded(&self) -> symphonia::core::audio::GenericAudioBufferRef<'_> {
            unimplemented!("nothing decoded")
        }
    }

    /// A codec panic ends the track, not the thread. The printed panic is the
    /// caught one.
    #[test]
    fn a_decoder_panic_ends_the_track_rather_than_the_thread() {
        let fx = Fixtures::new("decoder-panic");
        let path = fx.wav("tone.wav", 1.0);
        let (mut src, _) = Source::open(&local(&path), 48_000, None).expect("the fixture opens");
        src.decoder = Box::new(PanickingDecoder);

        let mut out = Vec::new();
        assert!(
            !src.next_chunk(48_000, &mut out),
            "a panicked decode is the end of the stream"
        );
        assert!(out.is_empty(), "and it handed out no samples");
        assert!(src.poisoned, "the source is poisoned against a second try");

        // A poisoned source never re-enters, or it would panic again.
        assert!(!src.next_chunk(48_000, &mut out));
        assert!(out.is_empty());
    }

    /// A panic during seek reads as a failed seek.
    #[test]
    fn a_panic_on_seek_reads_as_a_seek_that_failed() {
        let fx = Fixtures::new("seek-panic");
        let path = fx.wav("tone.wav", 2.0);
        let (mut src, _) = Source::open(&local(&path), 48_000, None).expect("the fixture opens");
        src.decoder = Box::new(PanickingDecoder);
        // The wav seek doesn't decode, so the reset is what panics.
        assert_eq!(src.seek_file(1.0), None, "no landing to report");
        assert!(src.poisoned);
    }

    #[test]
    fn the_decode_guard_turns_a_panic_into_an_error_on_the_file() {
        let path = "/music/broken.opus";
        let err = guard_decode("decode", path, || {
            panic!("attempt to shift left with overflow")
        })
        .expect_err("a panic is an error");
        assert!(
            err.contains("/music/broken.opus"),
            "the file is named: {err}"
        );
        assert!(err.contains("shift left with overflow"), "and why: {err}");
        assert!(
            err.starts_with("decode panicked"),
            "and what was doing it: {err}"
        );

        let err = guard_decode("probe", path, || panic!("static message"))
            .expect_err("a panic is an error");
        assert!(err.contains("static message"), "{err}");

        assert_eq!(guard_decode("decode", path, || 7).ok(), Some(7));
    }
}
