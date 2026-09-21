//! The decode thread: Symphonia decode, gapless track boundary, seek, and the
//! producer side of the sample ring. Everything here is allowed to allocate,
//! lock, and block; the RT line is the ring in output.rs.
//!
//! Gapless (ADR 3): one long-lived stream, the decoder swaps at EOF and the
//! next track's first frame goes into the ring right behind the last. Encoder
//! delay/padding comes from the LAME/iTunes headers: symphonia 0.6 exposes it
//! as packet trim metadata and the mp3 decoder applies it, so the samples we
//! see are already the playable range. The spike verifies that claim against
//! real files; if it falls short we trim from Track::delay/padding ourselves,
//! which the ADR anticipated. Opus is the one format where that fallback
//! already had to happen: the Ogg reader signals the end padding but never the
//! pre-skip, so [`crate::opus`] reads it out of the OpusHead and drops it
//! before the buffer ever reaches here.
//!
//! Pause is one shape here now, files and stations alike: the flag flips, the
//! callback stops consuming, the ring keeps what it holds and the decode loop
//! parks on a full ring. A station stays connected through it. The socket is
//! drained by a thread of its own into a tape ([`crate::tape`]), so the bytes
//! that arrive during a pause are kept rather than thrown away, and Play
//! carries on from the byte the pause stopped at instead of rejoining the
//! broadcast wherever it has got to. That's the timeshift, and it's what
//! [`Cmd::SeekLive`] steps back and forth through: the cursor moves inside
//! the tape and the decoder is rebuilt over it, which is a flush like any
//! other seek. Two rules come with it. A seek never crosses a gap, the mark a
//! reconnect leaves, because two connections' bytes don't decode as one
//! stream. And a pause that outlasts the tape leaves the cursor off the back
//! of it, which snaps to the oldest byte held and re-syncs there.
//!
//! Hanging up survives for the two cases a tape can't answer. A paused
//! restore at launch must not dial at all ([`Engine::open_start`]): the
//! session comes up on a station nobody has pressed Play on yet, and opening
//! it would put a live socket under a pause from the app's first second. And
//! a station left paused past [`LIVE_IDLE_HANGUP_SECS`] hangs up, because a
//! tab forgotten overnight shouldn't pull bytes until morning; the resume
//! rejoins at the live edge the way it always did. `hang_up` and `rejoin` are
//! the pair, and `hung_up` holding a value with no open source is the whole
//! state between them.

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
    /// Play a live stream from this many seconds behind its live edge, zero
    /// being live. The cursor moves inside the tape the station is being
    /// recorded into and the decoder is rebuilt over it, so this costs the
    /// same flush a scrub costs and nothing over the wire.
    ///
    /// Held to what the tape holds, and to the live side of any gap it would
    /// have crossed. Ignored for anything that isn't the audible live entry:
    /// a file has a real timeline and [`Cmd::Seek`] is how you move in it.
    SeekLive(f64),
    /// Play this many seconds through the pause, for a step taken while
    /// paused: hearing where the step landed is the point of stepping by
    /// milliseconds, and a silent one is just a number moving. Sent right
    /// after the Seek, so the blip starts on the first sample the seek's
    /// refill delivers rather than on the flushed audio before it. Another
    /// step re-arms it from the new position; nothing while playing.
    Audition(f64),
    Next,
    Prev,
    Volume(f32),
    SetLoop(LoopMode),
    SetShuffle(bool),
    /// Reorder the upcoming portion into the given entry order, for an
    /// ordering the engine has no way to work out for itself. The player
    /// sends this for shuffle's similarity mode, where what comes next is a
    /// question about the music that only the library can answer.
    ///
    /// Ids that aren't in the tail are ignored, and tail entries the list
    /// doesn't name keep their relative order behind the ones it does. So a
    /// partial list moves what it knows about to the front and disturbs
    /// nothing else.
    OrderTail(Vec<u64>),
    /// Arm or clear stop-after-current: armed, the track playing now ends
    /// the session's motion. The engine lets the ring drain so the last
    /// samples play out, then pauses with the next track cued at 0:00.
    /// Sticky until cleared, so every track end stops while armed.
    SetStopAfter(bool),
    /// Loop a section of the audible track, or clear it with None. Both
    /// seconds are track-relative. Setting one lands the clock on A right
    /// away, which costs the one flush a seek costs; every wrap after that
    /// is the gapless splice, so the section repeats without a hole.
    SetAbLoop(Option<(f64, f64)>),
    /// Splice tracks into the queue right after entry `after` (its stable id),
    /// or at the end when `after` is None. `explicit` marks them as user-queued
    /// (Play Next, Add to Queue) rather than part of the playing context, so
    /// the queue widgets can show them apart from the album or library that
    /// plays on around them.
    Insert {
        after: Option<u64>,
        /// Where each track's bytes come from: a file on disk, or a URL the
        /// transport opens. The engine never looks inside one, it hands it
        /// to [`Source::open`].
        locators: Vec<Locator>,
        /// Album group per track, parallel to `locators` (ADR 17). The player
        /// resolves these from the library at insert time; the engine only
        /// compares them. Shorter than `locators` pads with None.
        groups: Vec<Option<u64>>,
        /// ReplayGain tags per track, parallel the same way and resolved
        /// from the library beside the groups. Shorter pads with the
        /// untagged default, which the rule's fallback then handles.
        gains: Vec<gain::ReplayGain>,
        /// The slice of the file each entry plays, parallel the same way. A
        /// cue track is a span inside one image file, so two entries can
        /// share a path and still be different music. Shorter than
        /// `locators` pads with None, which means play the whole file.
        spans: Vec<Option<Span>>,
        explicit: bool,
        /// Jump to the first of the batch and play it now, keeping the rest of
        /// the queue behind it. A drag onto Play now sets this; Play Next and
        /// Add to Queue leave it off so the current track keeps playing.
        and_play: bool,
        /// With `and_play`, open the first of the batch this far in, track
        /// seconds, rather than at its start. A play-from-bookmark lands
        /// here in one move: a Seek sent beside the insert would cancel the
        /// jump, since one pass over the commands keeps a single flush, and
        /// one sent after it plays the track's head until it arrives.
        start_secs: Option<f64>,
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
    /// disables it: every boundary is the gapless splice again. Sent over
    /// the command channel rather than an atomic because the engine reads
    /// it while deciding to open a track, not per sample.
    SetCrossfade {
        secs: f32,
        /// Fade at album-contiguous boundaries too, overriding the rule
        /// that leaves a record's own splices alone. Off by default: an
        /// album that runs track into track was mastered that way, and
        /// fading it is a change to the record. On for a listener who
        /// wants every boundary soft whatever the tags say.
        albums: bool,
    },
    /// How much of a live stream to keep behind the playhead, in seconds.
    ///
    /// Takes the station on air with it rather than waiting for the next
    /// connect: the tape is re-capped in place, which grows by raising the
    /// ceiling and shrinks by trimming on the spot. The engine keeps the
    /// number too, so the next station this session opens gets it without
    /// the setting having to ride another [`StartQueue`].
    ///
    /// Held to [`LIVE_BUFFER_MIN_SECS`] and up, the same floor the start
    /// applies, so no path into the engine can leave a tape too short for a
    /// pause to mean anything.
    SetLiveBuffer(u32),
    /// How tagged loudness is levelled (ADR 19): the mode and the two
    /// offsets. Applied to every source in hand as it arrives, so a mode
    /// switch is heard on the track playing rather than the one after it.
    /// The command channel rather than an atomic because the engine reads
    /// it when a source opens, not per sample.
    SetGainRule(gain::GainRule),
    /// Take the producer end of a fresh sample ring: the output device
    /// faulted and the player opened another one under the same
    /// [`Shared`](crate::shared::Shared), at the same rate.
    ///
    /// The point of it is everything it doesn't touch. A device dropping out
    /// used to cost the whole session, which on a station meant a re-dial, a
    /// thrown-away tape, and a capture cut in the middle; here the decoder,
    /// the socket, the tape and the idle clock all carry on and only the ring
    /// changes hands. The rate is the one thing that can't change this way,
    /// since the resampler, the segments and every frame count in here are
    /// denominated in it, so the player checks it before sending and rebuilds
    /// the session instead when the device came back at another rate.
    SwapOutput(Producer<f32>),
    Quit,
}

/// The longest crossfade on offer. Past this the overlap stops reading as
/// a transition between two tracks and starts reading as both playing at
/// once; the UI's slider tops out here and the engine clamps to it.
pub const CROSSFADE_MAX_SECS: f32 = 12.0;

/// How long a station may sit paused before the connection is given up.
///
/// The tape is what makes a pause worth holding a socket open for, and past
/// half an hour there's nothing left to come back to: the window has rolled
/// over several times, and the resume is going to be a rejoin at the live
/// edge whatever happens. So the socket goes, the bytes stop, and a tab
/// forgotten overnight isn't still pulling a station's bandwidth in the
/// morning.
pub const LIVE_IDLE_HANGUP_SECS: u64 = 1800;

/// How often the station's song boundaries are refreshed for their
/// distances alone. The pump's own clock, since the distances slide
/// continuously with the drawn edge and a slower refresh draws them in
/// steps behind a playhead that isn't stepping. Still nowhere near the rate
/// the decode loop turns at.
const MARKS_REFRESH: StdDuration = StdDuration::from_millis(16);

/// The shortest tape the engine will keep, whatever it was asked for. The
/// setting clamps to a real band before it gets here; this is the floor for a
/// session started without one, which is every test and every embedder that
/// left the field at its default.
const LIVE_BUFFER_MIN_SECS: u32 = 30;

/// How far short of a track's end a seek is allowed to land. See
/// [`Source::inside_track`]: the last frame is not a place a reader can go.
const SEEK_END_MARGIN_SECS: f64 = 0.1;

/// What happens when a track or the queue runs out. Held on the decode
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
    /// What's behind the reader, the full path or the URL, kept so a decoder
    /// that falls over names what it fell over on. A string rather than the
    /// locator itself because every use of it is a log line, and resolving it
    /// once at the open keeps the per-packet guard from formatting anything.
    origin: String,
    /// What the track is called, as the entry's published info names it.
    /// Kept so a station rebuilt over its own tape comes back with the name
    /// it already had rather than a fresh read of the locator.
    name: String,
    /// Set when a read or a decode panicked. The reader and decoder are
    /// unusable from that point: the panic unwound out of the middle of
    /// their state, and calling back in could land on the same broken
    /// arithmetic or on state it never finished updating. A poisoned source
    /// reports end of stream and touches neither again.
    poisoned: bool,
    track_id: u32,
    time_base: Option<TimeBase>,
    device_rate: u32,
    resampler: Resampler,
    /// Scratch for one decoded packet, interleaved in the file's channel
    /// count, reused across packets.
    scratch: Vec<f32>,
    /// What this file's ReplayGain tags said, kept so the gain can be
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
    /// to "when does the fade window open": it doesn't. For a spanned source
    /// this is the span's length, not the file's, so the fade window and the
    /// EOF anticipation key off the track the listener picked.
    total_frames: Option<u64>,
    /// The slice of the file this source plays, where it plays only part of
    /// one. None is the ordinary case, the whole file start to end.
    span: Option<SpanFrames>,
    /// Where the decoder stands in the file, in the file's own frames. Only
    /// a spanned source needs this: the span's end is a sample position in
    /// the file, and the cut has to be made against the file's clock before
    /// the resampler turns those frames into device-rate ones.
    src_frame: u64,
    /// When a remote open began, kept until the first chunk of audio comes
    /// out of it and then taken. Closes the open-latency measurement the
    /// transport and the probe log the first two thirds of: the number that
    /// matters to a listener is when audio exists, not when the reader was
    /// built. None on a local file, which has nothing to explain.
    opened_at: Option<Instant>,
    /// The tape a live station is being recorded into, None for everything
    /// else. Held here because it's the only handle back down: once the probe
    /// is done the reader is buried under a `MediaSourceStream` and a format
    /// reader with no way through. The engine reads it for the timeshift
    /// readout and reopens over it for a [`Cmd::SeekLive`].
    tape: Option<Arc<Tape>>,
}

/// A [`Span`] resolved onto the file's own frame clock, which is where the
/// boundary has to be honored: milliseconds are how a cue sheet writes a
/// timestamp, frames are what a decoder hands over. Integer math both ways,
/// so the end of one track and the start of the next fall on the same frame
/// and two consecutive spans splice with nothing missing or doubled.
#[derive(Clone, Copy)]
struct SpanFrames {
    /// First playable frame of the span.
    start: u64,
    /// One past the span's last playable frame. None on the last track of an
    /// image, which runs to the file's own end.
    end: Option<u64>,
}

/// Milliseconds on a stream of `rate` as a count of its frames. Truncating
/// integer math: every caller that asks about the same timestamp
/// gets the same frame back, so a boundary shared by two spans is one frame
/// rather than two that differ by a rounding step.
fn ms_frames(ms: u32, rate: u32) -> u64 {
    (ms as u64 * rate as u64) / 1000
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

/// A section the source plays on repeat: both ends in the source's own
/// device-rate frame clock, pinned to the pool index it was set on so a
/// stale loop can never take hold of a different track.
#[derive(Clone, Copy)]
struct AbLoop {
    track: usize,
    a: u64,
    b: u64,
}

/// The shortest section worth looping. Under this the wrap lands inside the
/// packet it just decoded and the source spends the whole time seeking, so a
/// set this tight is refused rather than played badly.
/// Public so the player refuses the same set the engine would, and refuses
/// it before the round trip rather than sending a command that dies quietly.
pub const AB_MIN_SECS: f64 = 0.25;

pub struct Engine {
    /// Append-only pool of track locators. Order entries index into it;
    /// nothing is ever removed so `Segment.track` indices stay valid.
    queue: Vec<Locator>,
    /// Album group per pool entry, parallel to `queue` (ADR 17). Grows with
    /// the pool on insert, never shrinks. The engine never derives these,
    /// only compares them: same group means tracks that belong together.
    groups: Vec<Option<u64>>,
    /// ReplayGain tags per pool entry, parallel the same way. Read off the
    /// file's tags by the library, never by the engine; what happens here
    /// is the rule below turning them into one factor per source.
    gains: Vec<gain::ReplayGain>,
    /// The slice of the file each pool entry plays, parallel the same way.
    /// None is the whole file, which is every plain track; a cue track
    /// has the span its sheet gave it, and several pool entries then
    /// point at one image file with different spans.
    spans: Vec<Option<Span>>,
    idx: usize,
    /// The play order. All navigation steps through this, so `order[pos]` is the
    /// playing entry and Prev retraces the path. Editable in place: insert,
    /// remove, move, reshuffle.
    order: Vec<OrderEntry>,
    /// Position within `order`; kept in sync with `idx` on every open.
    pos: usize,
    /// Where the first open goes, so playback can start partway into a
    /// seeded context with history behind the cursor.
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
    /// drains, where the pause happens and the next track cues up.
    stop_pending: bool,
    /// The section on repeat, or None when nothing loops. Session state:
    /// a skip clears it, and it never outlives the engine.
    ab: Option<AbLoop>,
    /// Frames pushed on the frames_consumed clock; resynced after each flush.
    pushed_playable: u64,
    /// Decoded, converted samples waiting for ring space.
    pending: Vec<f32>,
    pending_pos: usize,
    /// The processing chain (ADR 19): runs over each decoded chunk after the
    /// fold and resample, immediately before the ring, at the device rate.
    /// Empty is the bypass rule: samples go into the ring untouched.
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
    /// Set while a pause has hung up on a live station: how far into the
    /// station the elapsed clock had got when the socket closed, in
    /// device-rate frames. None the rest of the time, which is every pause
    /// on a file and on a seekable stream.
    ///
    /// `Some` with no open source is the whole hung-up state. Nothing
    /// decodes, nothing has ended, and the next Play reopens the station at
    /// its live edge with this much already on the clock. One field rather
    /// than a flag beside a position: the entry to come back to is resolved
    /// off the output clock the same way Next and Prev resolve theirs, so a
    /// queue edit arriving during the pause moves it without this having to
    /// know.
    hung_up: Option<u64>,
    /// How many seconds of a live stream to tape behind the playhead, handed
    /// to every station this session opens.
    live_buffer_secs: u32,
    /// The station mark set as last published, and when. See
    /// [`Engine::publish_marks`].
    marks_rev: u64,
    marks_at: Instant,
    /// When the pause holding a station started, for the idle cap. None while
    /// something is playing, and None for a pause on anything that isn't a
    /// live stream: a paused file costs nothing to leave sitting.
    paused_since: Option<Instant>,
}

/// A crossfade in flight (ADR 19). The engine holds two open sources for
/// the length of the window: the new one is `source` and drives the loop,
/// this is the old one, decoded alongside and mixed underneath. One summed
/// stream goes into the ring, so the ring keeps its single producer.
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
    /// The outgoing track ran out. Past this the mix reads silence, the
    /// case for a track shorter than its own fade window.
    ended: bool,
}

/// A track a skip has wound back, on its way into a fade. Held between the
/// wind-back and the install, which are either side of the flush.
struct Wound {
    src: Source,
    /// The output frame the clock stood at when the wind-back was aimed.
    at: u64,
    /// How far short of that spot the seek actually landed, in device-rate
    /// frames. Zero for anything with a seek index; positive where a coarse
    /// seek undershot, negative where it overshot.
    short: i64,
}

/// The playing context handed to a new engine: the ordered tracks, where in
/// them to start, which entries are user-queued rather than part of the
/// context, the album group per entry, its ReplayGain tags, and the slice of
/// the file it plays. The four parallel vecs pad out where they run short of
/// `locators`, with false, None, the untagged default, and None again.
#[derive(Default)]
pub struct StartQueue {
    pub locators: Vec<Locator>,
    pub start: usize,
    pub explicit: Vec<bool>,
    pub groups: Vec<Option<u64>>,
    pub gains: Vec<gain::ReplayGain>,
    /// Which part of each entry to play, None being all of it. A cue track
    /// comes through as a span inside its image file, so a restored session
    /// of one disc rip is one path repeated with a span apiece.
    pub spans: Vec<Option<Span>>,
    /// How much of a live stream to keep behind the playhead, in seconds,
    /// straight off the setting. Held to [`LIVE_BUFFER_MIN_SECS`] and up, so
    /// a session started with nothing to say about it still tapes enough for
    /// a pause to mean something.
    ///
    /// Part of the start rather than a command because the first open happens
    /// before the channel is read: a session that starts playing a station
    /// would otherwise tape the default for its first entry whatever the
    /// setting says.
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

    /// Drop any audition blip still counting down. Every command that moves
    /// the transport calls this, so a step's blip can never outlive the
    /// position it was taken at and pause a track someone has since started.
    fn cancel_audition(&self) {
        self.shared.audition_left.store(0, Ordering::Relaxed);
    }

    pub fn run(mut self) {
        // Stream open: the chain learns the device rate before any sample
        // passes through it. It resets again on every flush, never at the
        // gapless boundary, so filter history persists across a track splice.
        self.chain.reset(self.device_rate);
        self.publish_queue();
        let mut source = self.open_start();

        loop {
            // Whatever was waiting is about to be read, so the flag that said
            // so comes down first. Set by the player as it sends a command a
            // listener expects answered now; read only by a stalled reconnect,
            // which uses it to stop retrying a station and let the command
            // through. Cleared ahead of the drain rather than after it, since
            // the sender stores before it sends: a store that lands in between
            // leaves its command in the channel for this pass or the next one,
            // where a clear after the drain could swallow the flag and leave
            // the command behind a full retry schedule.
            self.shared.interrupt.store(false, Ordering::Relaxed);

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
            // Where in the track a navigation lands, track seconds; only
            // an Insert that plays now sets it, and a plain Next, Prev, or
            // Jump arriving after it starts at the top like they always do.
            let mut nav_at: Option<f64> = None;
            // A queue edit moved the pre-decoded next track out from under its
            // open source: a remove dropped it, or a reorder put something else
            // in the slot after the audible entry. Set here, acted on after the
            // drain: drop that stale source and reopen the track that's really
            // next now. The audible track is fully in the ring, so no flush.
            let mut reopen_runahead = false;
            // A play now arrived and the session owes it a resume, paid once
            // the flush below has landed rather than here. See the store.
            let mut resume = false;
            while let Ok(cmd) = self.rx.try_recv() {
                match cmd {
                    Cmd::TogglePause => {
                        // At the ended state there's nothing coming out to
                        // pause, so the button can only mean play again: the
                        // finished track comes back from its start through the
                        // nav path, which clears ended on the way. Without this
                        // the flag flips under a source that isn't there and
                        // the transport stays dead once the queue plays out.
                        self.cancel_audition();
                        if source.is_none() && self.shared.ended.load(Ordering::Relaxed) {
                            self.shared.playing.store(true, Ordering::Relaxed);
                            nav_pos = Some(self.audible_pos());
                            flush_to = None;
                        } else if source.is_none() && self.hung_up.is_some() {
                            // Coming back to a station the last pause hung up
                            // on. The rejoin flips the flag itself, since a
                            // station that died meanwhile skips forward and
                            // whatever it lands on should be playing.
                            source = self.rejoin();
                        } else {
                            // One shape for everything now. A station holds
                            // its connection through a pause and keeps
                            // taping, so the ring and the pending buffer are
                            // kept too: what they hold is the broadcast from
                            // the moment the listener stopped, which is
                            // exactly what the resume should play. The idle
                            // cap below is the only thing that still hangs
                            // up, and only after half an hour of nobody
                            // coming back.
                            let now = self.shared.playing.load(Ordering::Relaxed);
                            self.shared.playing.store(!now, Ordering::Relaxed);
                        }
                    }
                    Cmd::Audition(secs) => {
                        // Only through a pause with something open: playing
                        // already hears the step, and nothing open has
                        // nothing to hear.
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
                        // Any blip still counting belongs to the position
                        // this seek is leaving. An Audition sent right
                        // behind this one re-arms it for the new one.
                        self.cancel_audition();
                        // A broadcast has no timeline to land in, and its
                        // tape isn't seekable, so symphonia would read this
                        // as a scan forward through the buffer: past the
                        // live edge and into bytes the station hasn't sent.
                        // The one seek a station has is `SeekLive`, and
                        // this is dropped rather than approximated. Dropped
                        // here rather than at the flush, because a flush
                        // that does nothing still cuts the ring and still
                        // ends the hung-up state a paused station holds.
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
                    Cmd::SetShuffle(on) => reopen_runahead |= self.set_shuffle(on),
                    Cmd::OrderTail(ids) => reopen_runahead |= self.order_tail(&ids),
                    Cmd::SetStopAfter(on) => self.stop_after = on,
                    Cmd::SetAbLoop(marks) => {
                        // Setting one has to be audible now, and the ring
                        // already holds up to half a second past B by the
                        // time the press arrives. So the set pays for one
                        // seek: `seek_to` reopens on the audible track,
                        // flushes, and lands on A. After that flush every
                        // wrap is a splice. Straight to `seek_to` rather
                        // than through `FlushAction::Seek`, since the flush
                        // arms are keyed on seconds this command doesn't
                        // carry once the marks are stored.
                        self.ab = None;
                        match marks {
                            Some((a, b)) if b - a >= AB_MIN_SECS => {
                                source = self.seek_to(source.take(), a.max(0.0));
                                // Where the seek actually landed, which is
                                // A for anything with an index and a packet
                                // early for a CBR MP3 without one. Read off
                                // the source so the wrap goes back to the
                                // frame the listener just heard.
                                //
                                // A seek that failed outright leaves the
                                // decoder wherever it already was, which is
                                // past B: no section is better than one
                                // whose ends are the wrong way round.
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
                        // From the ended state the source is None, so the new
                        // entries are added in order but nothing opens them
                        // and we stay silent. Route the first of the batch
                        // through the nav path so it reopens and resumes,
                        // clearing ended on the way. Play now jumps the same
                        // way from a live session; Play Next and Add to Queue
                        // leave the current track playing.
                        if and_play || source.is_none() {
                            nav_pos = at;
                            nav_at = start_secs.filter(|_| and_play);
                        }
                        // Play now means play: resume if we were paused, so a
                        // drop onto Play now starts audio instead of loading it
                        // silent. Owed rather than done, and paid below.
                        resume |= and_play;
                    }
                    Cmd::Remove { id } => reopen_runahead |= self.remove(id),
                    Cmd::RemoveMany { ids } => reopen_runahead |= self.remove_many(&ids),
                    Cmd::Move { id, after } => self.move_entry(id, after),
                    // Reuse the nav path: setting the target flushes and opens
                    // it just like a Next would.
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
                        // The station playing has its own tape, allocated
                        // when the connection was made, so the new length
                        // has to reach it directly. Whatever is open is the
                        // right one to re-cap: a station only ever has one
                        // tape, and an outgoing one inside a fade is a few
                        // seconds from being dropped either way.
                        if let Some(tape) = source.as_ref().and_then(|src| src.tape.as_ref()) {
                            tape.set_cap_secs(secs);
                        }
                    }
                    Cmd::SetGainRule(rule) => {
                        self.rule = rule;
                        // Both sources in hand, so a switch made during a
                        // fade takes on the track going out as well as the
                        // one coming in. Each keeps its own tags, so they
                        // resolve to different factors.
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

            // A flush is the one thing here the listener hears as a hole, so
            // the work either side of it is arranged around that: everything
            // that can be done while the ring is still playing happens
            // before the cut, and what's left after it is arithmetic.
            let flushed = flush_to.is_some();
            if let Some(action) = flush_to {
                // Anything that opens a track of its own ends the hung-up
                // state: a Next off a paused station, a Jump, a drop onto Play
                // now. The station's frozen clock belongs to a track nobody is
                // going back to. A timeshift seek is the exception, since it
                // only ever moves inside a station that's already open: with
                // nothing open it has nothing to do and leaves the hung-up
                // state exactly as it found it.
                if !matches!(action, FlushAction::SeekLive(_)) {
                    self.hung_up = None;
                }
                match action {
                    FlushAction::Track { pos, back, at } => {
                        // A loop belongs to the track it was marked on, so
                        // any move through the queue ends it: Next, Prev,
                        // Jump, a drop onto Play now. The natural advance
                        // can't get here while looping, since the wrap
                        // takes EOF before the boundary does.
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

            // The resume a play now owes, paid here rather than where the
            // command was read. Between those two points sits the open, and
            // on a station that's a request and a probe over the wire: a
            // callback told to start consuming at the command spends the
            // whole round trip playing whatever the paused ring still holds,
            // which is a second of the last track arriving out of nowhere
            // before the station does. By the time this lands the flush has
            // thrown those samples away. A local file is unaffected either
            // way, its open being too fast to hear, and a session already
            // playing stores true over true.
            if resume {
                self.shared.playing.store(true, Ordering::Relaxed);
            }

            if flushed {
                continue;
            }

            // The pre-decoded next track was removed or reordered away with its
            // source open. Drop that stale source and reopen the track now in
            // its slot, right after the audible one. The audible track already
            // filled the ring, so this reopen is silent, no flush needed. Any
            // half-decoded pending samples belong to the stale track, so
            // clear them too. Skipped when a flush already reopened above.
            //
            // Residual risk: if the decode cursor got far enough ahead that
            // some of the stale track's samples already went into the ring
            // (bounded by RING_SECS), that fraction still plays before the
            // reopened next track takes over. Flushing the ring would drop it
            // but would also cut the untouched audible track mid-note, a worse
            // glitch, so we accept the short tail here. A removal is worth that
            // splice, the listener asked for the track to be gone; an automatic
            // reorder isn't, so `reorder_tail` never asks for a reopen once the
            // runahead has fed the ring.
            // Hung up on a station, there's no runahead to reopen: the source
            // is gone because a pause closed it, not because a queue edit
            // invalidated it, and opening the track after it would leave a
            // connection running under a pause.
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

            // A pause on a station holds a socket open, so it can't hold it
            // forever. Both of these run on every pass rather than off a
            // decoded chunk, because a pause is exactly when nothing is
            // decoding and exactly when the tape is moving underneath.
            source = self.idle_hangup(source);
            source = self.follow_tape(source);

            // Move pending samples into the ring. Ring full means we're
            // comfortably ahead; sleep and go back to command handling.
            //
            // Full is whatever the latency hold says it is (ADR 19). With an
            // EQ editor open the gate closes early, so the ring keeps its
            // 500 ms of capacity as the underrun cushion but only holds a
            // fraction of it, and a knob is heard that much sooner. The sleep
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
                    let mut more = src.next_chunk(device_rate, &mut self.pending);
                    // The A-B wrap, before anything downstream sees the
                    // chunk: the overshoot past B comes off and the reader
                    // winds back to A, so the samples that go into the ring
                    // run straight from B into A with nothing between them.
                    // That's what makes it a loop rather than a seek every
                    // few seconds. `!more` carries EOF in, for a B set on
                    // the last second of the track.
                    if let Some(ab) = self.ab.filter(|ab| ab.track == self.idx)
                        && let Some(landed) = src.wrap_ab(ab.a, ab.b, !more, &mut self.pending)
                    {
                        // The wrap is at the end of what survived the
                        // cut, not the start of the chunk, so the
                        // position readout flips back to A on the frame
                        // the ear hears it.
                        let at = (self.pending.len() / 2) as u64;
                        self.register_segment_after(landed, at);
                        more = true;
                    }
                    // A fade in flight mixes the outgoing track underneath
                    // before anything downstream sees the samples, so the
                    // chain shapes the mix and the ring gets one stream.
                    self.mix_fade();
                    // The last step before the ring (ADR 19): chain output
                    // goes through flush, seek, and the gapless boundary
                    // like any other sample data, and the tap downstream
                    // sees what the chain produced.
                    self.chain.process(&mut self.pending);
                    // The broadcast sink taps the same processed stream
                    // (ADR 22), here because each chunk passes exactly once:
                    // a full ring retries the push loop above without
                    // decoding again. Never blocks; off is one atomic load.
                    crate::broadcast::feed(&self.pending, device_rate);
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
                        // armed stop-after skips the open instead: the
                        // drain below is where the pause happens, so the
                        // track's tail still plays out of the ring.
                        source = if self.stop_after {
                            self.stop_pending = true;
                            None
                        } else {
                            self.next_pos().and_then(|p| self.open_at(p))
                        };
                    }
                }
                // Paused on a station we hung up on. There's nothing to decode
                // until Play comes back and nothing has ended either: the
                // station is still on air, we just stopped listening to it.
                // Without this arm the drained ring below would read as a
                // played-out queue and the transport would go dead on a pause.
                None if self.hung_up.is_some() => {
                    std::thread::sleep(StdDuration::from_millis(20));
                }

                None => {
                    // Nothing to mix a fade under: the incoming track drives
                    // the mix, and there isn't one. Whatever was fading out
                    // is done, and its publish goes with it: unmixed,
                    // nothing would ever run past it to clear it.
                    self.drop_fade();
                    // Queue exhausted, or an armed stop-after cut the
                    // gapless open: either way the ring drains first so the
                    // last samples play out.
                    if self.ring_drained() {
                        if self.stop_pending {
                            // The stop took effect: pause, then cue what EOF
                            // would have opened so Play resumes right
                            // there. With nothing to cue (last track, loop
                            // off) fall through to the ended state, the
                            // pause having gone in all the same.
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

    /// The session's first open, which is the one open that can arrive on an
    /// already-paused session: a launch restore comes up where it left off,
    /// silent, and the player puts the pause flag down before this thread
    /// starts.
    ///
    /// A station is the one entry that can't be opened and left sitting. The
    /// open is a live socket and a broadcast has no pause, which is the whole
    /// reason [`hang_up`](Self::hang_up) exists; connecting here would put
    /// the session in exactly the state that path closes, except from the
    /// first second of the app's life. So a paused start on a station parks
    /// in the hung-up state instead, with nothing open and the clock at its
    /// top, and the first Play rejoins the way a resume from any other pause
    /// does. No new state for the rest of the loop to answer for: `hung_up`
    /// with no source is a state it already handles everywhere.
    ///
    /// Everything else opens the way it always has. A file and a seekable
    /// stream hold through a pause perfectly well, and the restore wants
    /// their duration and their name on screen before anyone presses Play.
    fn open_start(&mut self) -> Option<Source> {
        let paused = !self.shared.playing.load(Ordering::Relaxed);
        if !paused || !self.live_at(self.start) {
            return self.open_at(self.start);
        }

        // The cursor moves even though nothing opened: it's what the rejoin
        // resolves the entry to open off, and what the segment below names.
        // The pair `adopt` sets, without the rest of what `adopt` does,
        // which belongs to a track that's really playing.
        self.pos = self.start;
        self.idx = self.order[self.start].idx;
        self.hung_up = Some(0);

        // The position clock is how anything upstream knows what's loaded, so
        // the entry gets a segment at its own zero without a source behind
        // it. Otherwise a restore onto a station comes up showing nothing at
        // all: no name, no transport, nothing to press Play on except the
        // queue. There's no track info to publish beside it, which is right
        // for a station; the length of a broadcast is a question with no
        // answer whether or not we're connected.
        self.register_segment(0.0);

        None
    }

    /// Open the track at play-order position `p`, falling forward through
    /// unreadable files in play order. Registers the position segment for
    /// the new track.
    fn open_at(&mut self, p: usize) -> Option<Source> {
        self.open_at_from(p, 0)
    }

    /// [`open_at`](Self::open_at) with the new track's position segment
    /// registered `after` frames later than its first sample goes into the
    /// ring, and starting `after` frames into the track to match. Zero
    /// everywhere except a crossfade, where the boundary the listener hears
    /// is the middle of the window rather than its start: the clock, the
    /// track-change notification, and MPRIS all flip there, so nothing
    /// announces a track before it's audible (ADR 19).
    fn open_at_from(&mut self, p: usize, after: u64) -> Option<Source> {
        let (src, at, info) = self.open_file_at(p)?;
        self.adopt(at, info, 0, after);
        Some(src)
    }

    /// Open the file at play-order position `p`, falling forward through
    /// unreadable ones, and hand back the source with the position it
    /// actually opened at. Changes nothing: no cursor move, no segment, no
    /// track info published.
    ///
    /// Split out so a skip can pay for the open (the file, the probe, the
    /// decoder) while the old track is still coming out of the ring, and
    /// leave the flush with nothing to hold the silence open for.
    fn open_file_at(&mut self, mut p: usize) -> Option<(Source, usize, TrackInfo)> {
        while p < self.order.len() {
            let i = self.order[p].idx;

            // A station's in-band titles are the only now-playing it has, and
            // they keep arriving for as long as it plays, so the sink is bound
            // to this pool entry and lives as long as the source does. Local
            // files never fire it.
            let shared = Arc::clone(&self.shared);
            let on_title: TitleSink = Arc::new(move |title| shared.publish_title(i, title));

            // The same binding for the transport's own state. It fires from
            // deeper still, inside a read that's already lost its connection,
            // so it has no way of knowing which entry it belongs to either.
            let shared = Arc::clone(&self.shared);
            let on_stream: StreamSink = Arc::new(move |state| shared.publish_stream(i, state));

            // A remote open is the one that takes long enough for the wait to
            // be worth showing. It's published before the call rather than
            // after it because after it there's nothing left to wait for.
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

                    // What the station said about itself on the way in. Once
                    // per open and never again, so it's published here rather
                    // than through a sink: a reconnect mid-stream re-reads the
                    // same headers off the same mount and has nothing new to
                    // say.
                    if let Some(station) = station.filter(|s| !s.is_empty()) {
                        self.shared.publish_station(i, station);
                    }

                    // The gain is set at the track open (ADR 19), so it
                    // changes exactly where the source does.
                    src.level(self.gains[i], &self.rule);
                    return Some((src, p, info));
                }
                Err(e) => {
                    // A dead server takes this path the same way a missing
                    // file does: name it and try the next entry. The state
                    // goes with it, or the entry we just fell past would sit
                    // at `Opening` for the rest of the session.
                    if remote {
                        self.shared.publish_stream(i, StreamState::Dropped);
                        // A server that refused has a reason the listener
                        // can act on: a password that stopped working, a
                        // track the library still lists and the server
                        // doesn't. A missing local file is its own kind of
                        // obvious, so only a stream sends one up.
                        self.shared.publish_refusal(e.clone());
                    }

                    log::warn!("skipping {}: {e}", self.queue[i].label());
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
    ///
    /// `start` is where in the track the source was opened, in device-rate
    /// frames: zero for the ordinary open at the top, the landing spot for
    /// a play-from-bookmark, whose first pushed frame is already that far
    /// in. It adds to `after` the same way, since both are audio the clock
    /// has to count as played before the segment is reached.
    fn adopt(&mut self, p: usize, info: TrackInfo, start: u64, after: u64) {
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
            track_frame: start + after,
        });
        prune_segments(&mut segments, consumed);
    }

    /// Whether the entry at play-order position `p` is a live stream. The one
    /// thing that has to be asked of the locator rather than of the source:
    /// once symphonia owns the transport there's no way back down to it, and
    /// the answer is in the queue anyway.
    fn live_at(&self, p: usize) -> bool {
        self.order
            .get(p)
            .is_some_and(|e| matches!(&self.queue[e.idx], Locator::Remote(r) if r.live))
    }

    /// Move the audible station to `behind` seconds back from its live edge
    /// and re-sync the decoder there. Zero is live.
    ///
    /// Nothing goes over the wire: the bytes are already in the tape, and
    /// what moves is the cursor into it. What it does cost is a decoder,
    /// because there's no way to tell a running one to start reading
    /// somewhere else, so the source is rebuilt over the same tape at the new
    /// offset and the ring is cut the way any seek cuts it.
    ///
    /// The elapsed clock carries on rather than jumping back with the cursor.
    /// It counts the listen, not the position in a timeline a broadcast
    /// doesn't have, and a station stepped back through has been listened to
    /// for longer, not less. The title comes back the other way: the tape
    /// knows which song was playing at the byte the cursor landed on, and
    /// publishes that one.
    ///
    /// Everything about this is refused unless the station is the entry
    /// actually coming out of the speakers. The decode cursor can be a track
    /// ahead at a boundary, and moving a stream nobody is hearing yet would
    /// cut the file still playing out of the ring.
    fn seek_live_to(&mut self, source: Option<Source>, behind: f64) -> Option<Source> {
        let src = source?;
        let Some(tape) = src.tape.clone().filter(|_| self.pos == self.audible_pos()) else {
            return Some(src);
        };

        // The rebuild happens while the old source is still coming out of the
        // ring, so the only silence it costs is the cut itself.
        let was = tape.cursor();
        let at = tape.seek_target(behind);
        let hint = self.live_hint(self.idx);
        let (mut fresh, info) = match src.reopen_live(at, &hint) {
            Ok(rebuilt) => rebuilt,

            // Staying put is the right answer to a failed re-sync. The
            // listener asked to move inside a broadcast and the container
            // wouldn't have it; the station they were listening to is still
            // playing, and the cursor goes back to where that station is
            // rather than where the attempt left it.
            Err(e) => {
                log::warn!("live seek to {behind:.1}s behind failed: {e}");
                tape.restore_cursor(was);

                return Some(src);
            }
        };
        fresh.level(self.gains[self.idx], &self.rule);

        // Read before the flush moves `pushed_playable` out from under it.
        let elapsed = self.elapsed_frames();
        drop(src);
        self.flush_ring();
        self.adopt(self.pos, info, elapsed, 0);

        Some(fresh)
    }

    /// The container hint for reopening the station at pool entry `idx`: what
    /// the locator stored, or what the response's `Content-Type` implied.
    /// Empty leaves the probe to sniff the bytes, which is what a first open
    /// does when neither is known.
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

    /// Say where the audible station is being played from, and answer a tape
    /// that has rolled over the cursor.
    ///
    /// The readout can't ride a decoded chunk: both numbers keep moving
    /// through a pause, which is the one state where nothing is decoding, and
    /// a listener watching how far back they are during a pause is watching
    /// the feature work. So it rides the loop instead, which still turns
    /// while paused.
    ///
    /// The underrun is the other half. A pause that outlasted the window
    /// leaves the cursor pointing at bytes that have been dropped, the reader
    /// snaps to the oldest byte held, and the decoder is then mid-frame on a
    /// stream that jumped. Re-syncing at the back of the tape is the same
    /// move a seek makes, and it's the whole recovery.
    fn follow_tape(&mut self, source: Option<Source>) -> Option<Source> {
        let Some(tape) = source.as_ref().and_then(|src| src.tape.clone()) else {
            self.shared.clear_shift();

            return source;
        };

        // What's decoded but not yet out of the speakers: the ring plus
        // whatever is pending behind it. The tape takes it off the decoded
        // seconds so the playhead follows the audio, not the decoder's
        // top-ups.
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

    /// Hand the station's song boundaries and its reconnects up for
    /// whatever is drawing the buffer.
    ///
    /// Two clocks, because the lists change for two different reasons. A
    /// song announced, a connection spliced, or either falling off the back
    /// is a real change and gets a revision, which is what a reader polling
    /// for song changes watches. The distances moving as the live edge
    /// advances is not, and it happens on every chunk, so it rides a timer
    /// instead: often enough that a strip drawn at sixty frames a second is
    /// never looking at a stale one, rarely enough that this isn't
    /// rebuilding a list of strings on every pass of the decode loop.
    ///
    /// The gaps ride along rather than getting a pass of their own. They
    /// slide with the same edge the songs do, and a strip that redrew its
    /// songs against a set of breaks measured a tick earlier would put the
    /// two out of line with each other for as long as the station stays on.
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

    /// Hang up on a station that has been sitting paused long enough for the
    /// tape to be worthless.
    ///
    /// The timeshift is what makes holding a connection through a pause worth
    /// anything, and past [`LIVE_IDLE_HANGUP_SECS`] there's nothing left in
    /// it that the listener paused on: the window has rolled several times
    /// over. So the socket goes and the resume rejoins live, which is what a
    /// pause on a station used to do immediately and now only does when the
    /// pause has stopped being a pause and started being a forgotten tab.
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

    /// How far into the audible track the position clock stands, in
    /// device-rate frames, and zero when the clock is somewhere else
    /// entirely. What a station hands to the open that replaces it, so the
    /// elapsed readout carries on rather than restarting.
    fn elapsed_frames(&self) -> u64 {
        self.shared
            .position(self.device_rate)
            .filter(|(track, _)| *track == self.idx)
            .map(|(_, secs)| (secs * self.device_rate as f64).round() as u64)
            .unwrap_or(0)
    }

    /// Hang up on a live station, which is what every radio does when you stop
    /// listening to it. Hands the source straight back for everything else, so
    /// a file and a seekable stream pause exactly the way they always have.
    ///
    /// A station keeps broadcasting whether or not anyone is on the socket.
    /// Holding the connection open through a pause leaves the server either
    /// dropping us or feeding us at the rate we read, which is not at all, so
    /// the resume would hear a minute-old broadcast and stay a minute behind
    /// it for the rest of the session. Closing means the resume asks for now
    /// and gets now.
    ///
    /// The close is the drop. `HttpSource` has no way to shut its body without
    /// leaving itself half alive, and by the time the source exists the whole
    /// transport is buried under the decoder and the format reader with no
    /// handle out. Dropping the `Source` takes all of it, socket included.
    fn hang_up(&mut self, source: Option<Source>) -> Option<Source> {
        // Only the station the listener is actually hearing. The decode cursor
        // can be a track ahead of the speakers at a gapless boundary, and
        // hanging up on a stream that hasn't started playing yet would cut the
        // file still coming out of the ring.
        let pos = self.audible_pos();
        if source.is_none() || pos != self.pos || !self.live_at(pos) {
            return source;
        }

        // Where the clock stood, read before the flush moves `pushed_playable`
        // out from under it. The resume hands this back to `adopt`, so the
        // elapsed readout carries on from where the listener stopped hearing
        // rather than restarting at zero. Freezing it is the ordinary
        // transport behaviour and the honest one here too: the counter says
        // how much of the station has been heard, and through a pause that
        // number doesn't move. Counting on through the pause would have it
        // claim a minute nobody listened to, and resetting on resume would
        // leak the hang-up into a transport that has no business showing it.
        let elapsed = self.elapsed_frames();

        drop(source);

        // Nothing from the old connection survives. The ring holds up to half a
        // second of a broadcast that has moved on since, and the pending buffer
        // holds a chunk more; playing either on resume is the exact thing this
        // path exists to stop.
        self.flush_ring();
        self.hung_up = Some(elapsed);

        None
    }

    /// Come back to the station a pause hung up on: one fresh open at the live
    /// edge, down the same [`open_file_at`](Self::open_file_at) path a first
    /// open takes. So the title sink, the probe and the skip-on-failure are
    /// the ones already tested, and a station that died while nobody was
    /// listening fails here the way it would have failed on the first open.
    ///
    /// The published title survives the pause on purpose. It's the only
    /// now-playing a stream has, and blanking it would leave the transport
    /// naming nothing for as long as the pause lasts. A station still on the
    /// same song republishes the same text, which `publish_title` drops as a
    /// repeat: the slot already holds it, so the display is right and the
    /// revision correctly says no song changed.
    fn rejoin(&mut self) -> Option<Source> {
        let elapsed = self.hung_up.take()?;
        let pos = self.audible_pos();

        // Play means play whether or not the station answers. A rejoin that
        // dies falls forward to the next entry, and that entry should not land
        // on a transport still reading as paused.
        self.shared.playing.store(true, Ordering::Relaxed);
        self.shared.ended.store(false, Ordering::Relaxed);

        let (src, at, info) = self.open_file_at(pos)?;
        // Falling forward past a dead station lands on a different track, and
        // the station's elapsed means nothing there: that one starts at its
        // top like any other open.
        let start = if at == pos { elapsed } else { 0 };
        self.adopt(at, info, start, 0);

        Some(src)
    }

    /// Which position the next open uses when the playing track ends: the same
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

    /// Apply an armed stop-after once the ring has drained: the session goes
    /// quiet, and what EOF would have opened comes back so the caller can cue
    /// it for the next Play. None when there's nothing to cue.
    ///
    /// The pause is stored whether or not there's a next position, or the
    /// last track of a queue with looping off would end with the session
    /// still reading as playing. Anything that wakes it later, a queue edit
    /// or a continuation batch arriving behind it, would then start audio
    /// against a stop the listener asked for. A stop disarmed during the
    /// drain rolls on instead, which is the whole point of asking here rather
    /// than when the flag was set.
    fn land_stop(&mut self) -> Option<usize> {
        self.stop_pending = false;
        if self.stop_after {
            self.shared.playing.store(false, Ordering::Relaxed);
        }
        self.next_pos()
    }

    /// Whether everything pushed has been heard: the ring is empty, so
    /// nothing is playing over whatever happens next.
    fn ring_drained(&self) -> bool {
        self.producer.slots() == self.producer.buffer().capacity()
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
    /// more than half the track: a fade longer than what it's leaving
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
        // An armed stop-after ends the session at this boundary, so there's
        // nothing to fade into.
        // Same for an A-B loop on the playing track: it never reaches the
        // boundary, so a section set in the last seconds would otherwise
        // open a fade into the next track on every wrap.
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

    /// Open the next track early and hand it back as the one driving the
    /// loop, with the track it overlaps moved into the fade. Returns the
    /// old source untouched when there's nothing to open, so the boundary
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
    ///
    /// One case takes no cut at all, see below: an ending still coming out of
    /// the ring is left to finish.
    /// `at` opens the new track that many seconds in, the play-from-bookmark
    /// landing: the seek happens on the fresh source before anything of it
    /// reaches the ring, so the listener never hears its head, and the
    /// segment registers at the landing spot so the clock reads right from
    /// the first frame.
    fn skip_to(
        &mut self,
        mut old: Option<Source>,
        p: usize,
        back: bool,
        at: Option<f64>,
    ) -> Option<Source> {
        // A station is the one open slow enough for the wait to be a state of
        // its own, and everything upstream answers "what is loaded" off the
        // position clock. Leave that clock on the track being left and the
        // whole connect renders as the old track still playing, with the
        // station's `Opening` published on an entry nobody upstream is
        // looking at. So the entry is claimed here, before the open, and the
        // open catches up to it.
        //
        // Only a station. A file opens in a millisecond, and a skip that
        // lands anywhere but the top (a bookmark, a cue) would show a 0:00
        // that was never true on the way past.
        let live = self.live_at(p);
        // The wind-back asks the clock where the listener has actually got
        // to, and the claim below is about to answer that question with the
        // station at its top, so on the live path the fade is prepared first.
        // It costs nothing to move: it and the open both happen while the
        // ring is still playing, and the cut is still after both.
        let had_source = old.is_some();
        let wound = live.then(|| self.prepare_skip_fade(old.take())).flatten();
        if live {
            // The pair `adopt` sets, without the rest of what `adopt` does,
            // which belongs to a track that really has a source behind it.
            // The same move `open_start` makes for a restore that comes up
            // on a station, and `adopt` writes over both when the source
            // arrives; this is only about the seconds in between.
            self.pos = p;
            self.idx = self.order[p].idx;
            self.claim_segment();
        }

        let opened = self.open_file_at(p);
        // Nothing decoding, nothing mixing, and the ring still holding
        // samples: this is the half second between the last track's EOF and
        // the ended state. There's no music to hurry along, only an ending to
        // let play out, and a batch arriving here (ADR 17) or a queue edit
        // would otherwise chop it with no fade. The new track goes in behind
        // what's left the way it would at any gapless boundary, and
        // `pushed_playable` already points past the tail, so the segment
        // goes where the new track really becomes audible.
        let draining = !had_source && self.fade.is_none() && !self.ring_drained();
        let leaving = wound.or_else(|| self.prepare_skip_fade(old));
        let cut = if draining {
            self.pushed_playable
        } else {
            self.flush_ring()
        };
        self.shared.ended.store(false, Ordering::Relaxed);
        // Nothing opened means nothing drives the mix, so there's no fade to
        // install either. Publishing one here would leave the transport
        // showing an overlap that never renders and never clears: the mix
        // never runs, so nothing is ever there to close it.
        let (mut src, pos, info) = opened?;
        // A failed seek is a track that starts at its top, which is what
        // every other skip does anyway; nothing to report.
        let landed = at.and_then(|secs| src.seek(secs)).unwrap_or(0.0);
        let midpoint = self.install_skip_fade(leaving, cut, back);
        let start = (landed * self.device_rate as f64).round() as u64;
        self.adopt(pos, info, start, midpoint);
        Some(src)
    }

    /// Scrub the audible track to `secs` and cut the ring so the listener
    /// ends up there. A seek cuts rather than fades: scrubbing is meant to be
    /// heard as a jump.
    ///
    /// The decode cursor leads the audible track by up to a ring during the
    /// gapless preroll, so the open source is already the next track and
    /// seeking it would scrub inside the following track. Reopen the audible
    /// track first, the same anchor Next and Prev use.
    ///
    /// No source at all is the ended state (or the drain ahead of a
    /// stop-after about to take effect), and that same reopen brings the
    /// played-out track back under the strip. Without it the seek has
    /// nothing to seek and a click on a finished queue does nothing at all.
    fn seek_to(&mut self, mut source: Option<Source>, secs: f64) -> Option<Source> {
        let ap = self.audible_pos();
        let mut reopened = None;
        if ap != self.pos || source.is_none() {
            // A reopen that fails leaves the open source on the wrong track
            // (the pre-rolled next one), so it's dropped either way rather
            // than scrubbing audio the user isn't hearing.
            reopened = self.open_file_at(ap);
            source = None;
        }
        // The seek is the expensive half and it doesn't depend on the cut, so
        // it runs here too, while the ring is still playing.
        let landed = match reopened.as_mut() {
            Some((src, _, _)) => src.seek(secs),
            None => source.as_mut().and_then(|src| src.seek(secs)),
        };
        self.flush_ring();
        if let Some((src, at, info)) = reopened {
            self.adopt(at, info, 0, 0);
            // A seek that revived a played-out queue is playing again, so
            // nothing downstream should still read as finished.
            self.shared.ended.store(false, Ordering::Relaxed);
            source = Some(src);
        }
        if let Some(landed) = landed {
            self.register_segment(landed);
            // Scrubbing out of the section is how you leave it: the seek
            // said go somewhere the loop would never have taken you, so
            // the loop is done. A seek that lands inside keeps it, which
            // is how you replay the first half of a bar you're drilling.
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

    /// Wind the track a skip is leaving back to the spot the listener has
    /// actually reached, ready to carry the fade under the new one, and
    /// report the output frame that spot is at.
    ///
    /// The decode cursor runs up to a ring ahead of the speakers, so the
    /// open source is well past what was heard; the wind-back makes the fade
    /// start under the last sample that got out. It happens before
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
        // CBR MP3 without one. Keep the difference so the install can
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
        // cut ended at rather than replaying the last few milliseconds.
        let owed = cut.saturating_sub(at);
        // Past a quarter second the flush didn't take a period, it stalled
        // (a dead backend running the deadline out). Skipping that much of a
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
        // No longer than what's left of the track being left: past its
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
    /// off the output clock, so a stale publish stays there forever.
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
    /// window that closed on time: the ear is still inside it.
    fn close_fade_fast(&mut self) {
        let ramp = (self.device_rate as u64 / 50).max(1);
        if let Some(fade) = self.fade.as_mut() {
            fade.len = fade.len.min(fade.done + ramp);
        }
    }

    /// Mix the outgoing track under the chunk just decoded, and close the
    /// window once it has run its length. The two sources sum here, in the
    /// engine, so the chain and the ring get one stream.
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

    /// Splice tracks into the pool and order right after entry `after` (or at
    /// the end). Never flushes: the current track keeps playing, only the
    /// future changes. If the splice goes in before the cursor the cursor
    /// moves with it so the playing entry stays put. Returns the order
    /// position of the first appended entry, or None when nothing was
    /// inserted, so a revive from the ended state can navigate to it.
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
            // Every parallel-to-the-pool vector grows here, or a slot written
            // by index for one of these entries lands past the end and the
            // publish silently does nothing. That's what a station added by
            // Play now was hitting: opened, described itself, and published
            // into a vector that had no room for it.
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
        // hands off to pos+1 at EOF, so pos must end up on the audible entry or
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

    /// The last entry a tail reorder has to leave alone, everything past it
    /// being fair game.
    ///
    /// That's the audible entry, not the decode cursor. Near a boundary the
    /// cursor has already stepped onto the next entry and opened it for the
    /// gapless handoff, so anchoring on the cursor would freeze the very next
    /// track out of the reorder: switch to Similar during the last seconds of
    /// a track and the radio wouldn't kick in until the track after next.
    ///
    /// A boundary crossfade is the one exception. There the entry at the
    /// cursor is the incoming track, already mixing into the output and
    /// rising under the one going out, so it counts as playing even though
    /// the clock hasn't flipped to it yet. Reordering around it would mean
    /// cutting audio the listener is hearing, so the fade window keeps the
    /// old cursor anchor and the reorder lands on what comes after the fade.
    fn reorder_anchor(&self) -> usize {
        if self.fade.is_some() {
            self.pos
        } else {
            self.audible_pos()
        }
    }

    /// True when the pre-decoded next track already has samples sitting in the
    /// ring, waiting behind the audible track rather than merely being open.
    ///
    /// [`adopt`](Self::adopt) registers the segment at the open, before a
    /// single frame of the new track is pushed, so "a segment exists" doesn't
    /// answer this. What does is where that segment sits against
    /// `pushed_playable`: once we've pushed past its start frame, those frames
    /// are in the ring, and since the cursor still leads the audible entry
    /// they haven't played yet either.
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

    /// Reorder the upcoming portion with `sort`, everything past the anchor
    /// from [`reorder_anchor`](Self::reorder_anchor). History and the playing
    /// entry stay put, and the ring is never flushed.
    ///
    /// Under Loop All the upcoming portion wraps. When the last entry is the
    /// audible one, everything in front of it is what plays next, so that's
    /// the stretch the sort gets; without the wrap a Similar or shuffle
    /// command on the final track reordered nothing at all.
    ///
    /// Returns true when the reorder displaced the runahead, the pre-decoded
    /// next track whose source the decode cursor already opened. The caller
    /// then drops that source and reopens whatever the sort put right after
    /// the audible entry, same contract as [`remove`](Self::remove) has for a
    /// removed runahead. Without it the open source plays on and the newly
    /// ranked next track waits a whole extra boundary.
    ///
    /// The trade the runahead check buys: a reopen only drops the open source
    /// and the half-decoded pending samples, so any frames of the pre-rolled
    /// track already pushed into the ring still play before the reopened one
    /// takes over. Silent while the runahead is only open, audible as a splice
    /// once it has fed the ring. Similar mode fires a reorder on every batch
    /// landing, and one landing in the last ring of a track would otherwise
    /// splice the old next track into the new one. So when the runahead is
    /// already feeding the ring the sort starts past it instead: the pre-rolled
    /// track keeps its slot and the reorder lands behind it, one boundary
    /// later than it might have but never mid-note.
    fn reorder_tail(&mut self, sort: impl FnOnce(&mut [OrderEntry])) -> bool {
        let len = self.order.len();
        let anchor = self.reorder_anchor();
        // Half-open span of slots the sort may touch: everything past the
        // anchor, or the whole order in front of it once Loop All has the
        // cursor wrapping around the last entry.
        let (mut start, mut end) = if anchor + 1 < len {
            (anchor + 1, len)
        } else if self.loop_mode == LoopMode::All && len > 1 && anchor == len - 1 {
            (0, anchor)
        } else {
            self.publish_queue();
            return false;
        };
        // A boundary fade under Loop All can have the cursor already wrapped
        // onto the front of the order while the last entry is still coming out
        // of the speakers. The anchor is the cursor through a fade, so the span
        // would run right over that audible entry: stop short of it.
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
        // The stretch from the first reorderable slot through the decode
        // cursor: these are the entries the open source's handoff depends on.
        // If the sort leaves them in the same sequence the cursor still points
        // at its own entry and the pre-roll is still correct, whatever it did
        // further out. The cursor sits outside the span when nothing has run
        // ahead, and that's the mid-track case where there's no pre-roll to
        // invalidate at all.
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
            // Re-anchor onto the audible entry the way a removed runahead
            // does, so the caller's reopen goes to the entry now sitting in
            // the next slot rather than one past it. On the wrap that slot is
            // past the end of the order, and the reopen wraps to the front
            // with it.
            self.pos = anchor;
        }
        self.publish_queue();
        changed
    }

    /// Shuffle the upcoming portion, or put it back in pool order (ascending
    /// idx), which is library order for a fresh context; play-next inserts,
    /// being later pool entries, settle at the tail. Returns the displaced
    /// runahead flag from [`reorder_tail`](Self::reorder_tail).
    fn set_shuffle(&mut self, on: bool) -> bool {
        self.reorder_tail(|tail| {
            if on {
                shuffle_slice(tail);
            } else {
                tail.sort_by_key(|e| e.idx);
            }
        })
    }

    /// [`Self::set_shuffle`]'s twin for an order computed elsewhere: put the
    /// upcoming portion into the sequence `ids` names. Same guarantees, so
    /// history and the playing entry stay put and nothing flushes.
    ///
    /// The sort is stable and unnamed entries rank last, which lets a
    /// partial list work: tracks the caller had no opinion about keep the
    /// order they were already in, behind the ones it ranked.
    fn order_tail(&mut self, ids: &[u64]) -> bool {
        let rank = |id: u64| {
            ids.iter()
                .position(|&want| want == id)
                .unwrap_or(usize::MAX)
        };
        self.reorder_tail(|tail| tail.sort_by_key(|e| rank(e.id)))
    }

    /// Have the backend discard everything queued and tell us it has, then
    /// resync our clock to what actually played. Returns the output frame
    /// the cut ended at, which is where the next sample pushed will play.
    ///
    /// The wait is the whole gap a skip costs: the ring is clear the moment
    /// the ack arrives, so the sooner this returns the sooner audio comes
    /// back. Everything the caller can do beforehand (opening the next
    /// file, winding a source back) belongs before the call, while the ring
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
        // A live backend acks within one period; bound the wait so a dead
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

    /// Move the session onto a fresh output ring, for a device that faulted
    /// under it. Everything upstream of the ring stays exactly as it was:
    /// the decoder, a station's socket, its tape, and the pause clock the
    /// idle hangup counts on.
    ///
    /// What the dead stream took with it is the half second the ring still
    /// held. Those samples were pushed and never heard, so the decoder now
    /// stands that far ahead of the speakers and has to come back before it
    /// carries on, or the swap would cost the listener the gap twice over:
    /// once in silence and once in music nobody heard. The two ways back are
    /// the two already here. A station re-syncs over its own tape, which
    /// costs a decoder and nothing over the wire, and lands the listener on
    /// the exact spot they were standing, behind live if that's where they
    /// were. Everything else seeks to where the position clock stopped,
    /// through the path that also answers a decode cursor sitting a track
    /// ahead of the speakers.
    ///
    /// Nothing here opens anything. A hung-up station and a played-out queue
    /// both arrive with no source at all, and dialling one back up because
    /// the speakers changed would be a connection nobody asked for. They take
    /// the clock resync on its own.
    fn swap_output(&mut self, producer: Producer<f32>, source: Option<Source>) -> Option<Source> {
        // Read while the dead ring is still in hand: how far back the
        // listener stands is measured off what it held.
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

        // Not a station that couldn't take the tape path, though: the seek
        // would reopen it from scratch, and a swap has no business dialling
        // anything. It keeps the source it has and takes the resync alone.
        // Only a decode cursor a track off the speakers gets here, and that
        // lasts until the boundary.
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

    /// Publish the fade window for the transport, in output-clock frames.
    /// Read back through [`Shared::crossfade`], which only shows it once the
    /// speakers reach it.
    fn publish_fade(&self, at: u64, len: u64, back: bool) {
        self.shared.fade_at.store(at, Ordering::Relaxed);
        self.shared.fade_back.store(back, Ordering::Relaxed);
        self.shared.fade_len.store(len, Ordering::Release);
    }

    fn register_segment(&self, track_secs: f64) {
        self.register_segment_after(track_secs, 0);
    }

    /// [`register_segment`](Self::register_segment) with the spot reached
    /// `after` frames later than the next sample pushed. Zero for a seek,
    /// where the chunk that follows starts at the landing spot; the A-B
    /// wrap uses it because the chunk in hand runs up to B before it goes
    /// back to A, and claiming zero would flip the readout a chunk early.
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

    /// Register a position segment for the cursor's track at its own top,
    /// landing on the frame the speakers are at rather than on the one the
    /// next push will reach.
    ///
    /// The claim a station makes before its open, and the only segment here
    /// with no audio behind it. Everything else registers at
    /// `pushed_playable`, which is where the samples about to be written
    /// really become audible; a claim has nothing to line up with and wants
    /// to be read now. A ring still holding the last track's tail, or a
    /// pause holding it for as long as the pause lasts, would leave a
    /// segment out at `pushed_playable` waiting on a drain the cut is about
    /// to cancel, and the clock would never reach it at all.
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

    /// Drop the section, if there was one, and say so. Idempotent, so the
    /// paths that clear defensively don't have to ask first.
    fn clear_ab(&mut self) {
        if self.ab.take().is_some() {
            self.publish_ab();
        }
    }

    /// Publish the loop for the player to read (ADR 16): frames back to
    /// seconds, `u64::MAX` in A for off. Called from every place `ab`
    /// changes, so the snapshot can't fall behind the engine's own copy.
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
/// comparison NaN returns false for, so the fade would read as on while
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
    /// Move to this many seconds behind a station's live edge.
    SeekLive(f64),
    /// Jump to this play-order position.
    Track {
        pos: usize,
        /// The jump came from a Previous. Only the transport's fade
        /// readout uses it; the engine treats both directions the same.
        back: bool,
        /// Open the track this far in, track seconds, instead of at its
        /// start: the play-from-bookmark landing.
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
/// per-process random keys; a play order doesn't need a rand dependency.
///
/// Public because the play order isn't the only thing that needs an unbiased
/// shuffle without a dependency: a continuation provider (ADR 17) shuffles
/// its candidate pool before it picks, and a second copy of this would be a
/// second thing to get wrong.
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

/// `count` items drawn uniformly from `items`, in one pass and without
/// holding more than the draw: reservoir sampling, xorshift64 off the same
/// per-process keys [`shuffle_slice`] uses.
///
/// For the draws that would otherwise list a whole pool to keep a hundred
/// of it: a shuffle-on click across a million-row view, a continuation batch
/// out of everything the session hasn't played. The output keeps no
/// particular order, which every caller either shuffles again or doesn't
/// care about.
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

/// Shuffle the first `width` of a slice among themselves, leaving the rest
/// in place. The radio's band: what comes next is drawn from the nearest
/// `width` entries, and everything behind them keeps its ranking.
///
/// Off the std hasher's per-process keys, the same trick the Random button
/// uses. Picking a track doesn't need a rand dependency.
///
/// Shared between the player's skip band and the radio continuation
/// provider (ADR 17), which draws its batch the same way: rank the whole
/// pool, then shuffle the band at the front of it so two sessions off one
/// seed don't play the same list.
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

/// Run a call into symphonia or a codec crate so a panic inside it comes
/// back as an error on the path the caller already has for a file it can't
/// read. `origin` is the path or the URL the bytes came from, for the log
/// line and the error.
///
/// Decoders parse whatever bytes a file holds, and a malformed one has
/// already found an arithmetic overflow deep inside a third-party codec and
/// taken the whole app down with it. The blast radius that belongs to
/// someone else's bug in someone else's crate is the track, not the process.
///
/// `AssertUnwindSafe` is on the call site, and it holds because every one of
/// them abandons the reader and decoder it panicked in: a half-finished
/// decode can leave that state inconsistent, so nothing here decodes another
/// packet through it. The functions that own a whole `Source` return an
/// error and drop it; [`Source::decode_chunk`] sets `poisoned` and never
/// touches the decoder again.
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

/// The message out of a caught panic's payload, for the log line and the
/// error the caller gets. Panics carry a `String` when the message was
/// formatted and a `&'static str` when it wasn't; anything else is a payload
/// from a `panic_any` we have no way to read.
fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else {
        "no message".to_string()
    }
}

/// Decode a whole file through the same path playback uses and report
/// (decoded frames, frames the container claims are playable). Equal numbers
/// mean the encoder delay/padding trim is exact, i.e. the gapless boundary
/// is sample-accurate by construction. No audio device involved.
pub fn count_frames(path: &Path) -> Result<(u64, Option<u64>), String> {
    // Probe once for the source rate, then open for real with the device
    // rate equal to it, so the resampler is a passthrough and the count is
    // in source frames.
    let (probe, info) = Source::open(&Locator::Local(path.to_path_buf()), 48000, None)?;
    drop(probe);
    let (mut src, info) =
        Source::open(&Locator::Local(path.to_path_buf()), info.sample_rate, None)?;

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
/// peak lanes of at most `bins` [`PeakBin`]s spanning the track, the data
/// behind a waveform strip: each bin holds the sample extremes over its
/// frames and the RMS level across them. Lane 0 is the mono mix; a source
/// with more channels adds a left and a right lane after it (the decode
/// path folds wider layouts to front left/right, so two is as many as come
/// out). Bins are normalized so the loudest extreme hits 1 (the channel
/// lanes against a shared loudest, so the balance between them stays
/// honest) with a gentle perceptual curve so quiet passages stay visible.
/// The RMS runs through the same scale and curve, so it always sits inside
/// the extremes. No audio device involved; run it on a background thread,
/// a long track is a full decode.
pub fn decode_peaks(path: &Path, bins: usize) -> Result<PeakLanes, String> {
    // Probe once for the source rate, then open for real with the device
    // rate equal to it, so the resampler is a passthrough.
    let (probe, info) = Source::open(&Locator::Local(path.to_path_buf()), 48000, None)?;
    drop(probe);
    let (mut src, info) =
        Source::open(&Locator::Local(path.to_path_buf()), info.sample_rate, None)?;

    // Coarse pass: one bin per lane per fixed block of frames, so memory
    // stays a few thousand bins whatever the track length, then fold down
    // to `bins`. The lanes run through the loop in mono, left, right order.
    // A block's RMS is the root of its mean square, and the last, short
    // block averages over the frames it actually has.
    const BLOCK_FRAMES: usize = 2048;
    let mut coarse: [Vec<PeakBin>; 3] = Default::default();
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut sq = [0.0f64; 3];
    let mut in_block = 0usize;
    let mut chunk = Vec::new();
    loop {
        chunk.clear();
        // The EOF call flushes the resampler's final frame into `chunk`, so
        // fold it in before honouring the end signal.
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

    // A mono source's channel lanes would duplicate the mix; drop them at
    // the door so the strip has no lanes to split.
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

/// Fold coarse bins into the requested resolution, keeping each bucket's
/// extremes so transients come through the downsample. The RMS folds as
/// the root of the mean square, which the equal-sized coarse blocks make
/// exact (the short tail block is one among thousands).
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

/// One bin spanning a run of bins: the extremes of the run and the RMS
/// across it.
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

/// Scale the lanes so the loudest extreme among them hits 1, with the
/// perceptual curve that keeps quiet passages visible. The RMS takes the
/// same scale and curve, so a bin's band stays inside its envelope. Lanes
/// normalized together keep their relative loudness.
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

/// Decode one window of audio starting at `position_secs`, resampled to
/// `device_rate` and interleaved stereo, at least `frames` frames when the
/// track has them. This is the paused-load prime for the spectrum: playback
/// only feeds the visualizer's tap while it renders, so a track loaded paused
/// has nothing to show. Decoding a single window off-thread gives the frozen
/// bars a real frame to stand on. No audio device involved; run it on a
/// background thread.
///
/// Local tracks only. A remote one would have to open a second connection to
/// the server, and paying a network round trip to decorate a paused load is
/// the wrong trade: the bars stay blank until playback feeds the tap, which
/// is what they do today on any track that fails to decode.
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
    /// Open `locator` and hand back a source for the part of it named by
    /// `span`, None meaning all of it. A cue track is a span inside a whole
    /// disc image, and from here on the source behaves as if the span were
    /// the whole file: it opens positioned at the span's first frame, counts
    /// its position from there, reports the span's length, and calls the
    /// span's end the end of the track.
    ///
    /// A remote locator changes where the bytes come from and nothing else.
    /// Everything past the stream construction below reads the same for both,
    /// because [`MediaSourceStream`] takes a `MediaSource` and an
    /// [`crate::http::HttpSource`] goes in exactly where a `File` did.
    ///
    /// Station titles are dropped. The openers that come through here are the
    /// off-thread analysis passes (ReplayGain, peaks, the spectrum window),
    /// and none of them is playing anything for anyone to see a title on.
    /// Playback opens through [`Source::open_titled`].
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
            // Nothing is waiting on an analysis pass the way a listener waits
            // on a transport, and the passes have their own cancellation.
            Arc::new(AtomicBool::new(false)),
            // Nothing pauses an analysis pass either, so the tape only ever
            // has to hold what the decoder hasn't caught up with.
            LIVE_BUFFER_MIN_SECS,
        )?;

        Ok((source, info))
    }

    /// The same open with somewhere for a station's in-band titles to go,
    /// and with what the station said about itself handed back beside the
    /// track info. `on_title` fires on this thread from inside the decode's
    /// reads, for as long as the source lives, so it has to be short; the
    /// description is read once here and returned, because that's the only
    /// time the headers carrying it exist.
    ///
    /// A local file answers None for the description, which is the shape of
    /// the thing: there are no headers on a file.
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
        // Where the open-latency lines measure from. A local open is over
        // before the first of them would print, so only a remote one logs.
        let began = Instant::now();
        let remote_open = matches!(locator, Locator::Remote(_));

        // A file names its container in its extension. A URL doesn't, so the
        // hint comes off what the source stored or off the `Content-Type` the
        // server answered with, and failing both the probe sniffs the bytes.
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

    /// Re-sync a decoder onto another point in the tape this source is
    /// already reading, `at` being an absolute offset in the stream.
    ///
    /// The tape and the connection behind it carry on untouched: what's
    /// rebuilt is the probe, the format reader and the decoder, over a fresh
    /// cursor. That rebuild is the whole trick. Nothing about a station's
    /// bytes lets a running decoder be told to look somewhere else, and MP3
    /// and ADTS both find their footing wherever a reader drops in, so
    /// starting over at the new offset is the cheapest honest way to land
    /// there. Ogg needs the cursor on a page boundary, which the tape's own
    /// seek already scanned to.
    ///
    /// Errors leave the caller's source alone. A probe that can't make sense
    /// of a mid-stream drop-in (the codec's setup headers went past hours
    /// ago, which is Ogg's problem and nobody else's) has to be survivable:
    /// the listener asked to move inside a broadcast, and the answer to
    /// failing at that is to keep playing what they had.
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

    /// Everything after the bytes: probe the container, build the decoder,
    /// work out how long the track is, and hand back a source positioned at
    /// the start of whatever it plays.
    ///
    /// Shared by the two things that make a source. A locator open builds the
    /// stream off a file or a fresh connection; a timeshift seek builds one
    /// over a tape that's already running, at another point in it. Neither
    /// knows anything the other doesn't past this line, which is the point of
    /// the split: a station re-synced mid-stream goes through the same probe
    /// and the same decoder setup as one opened from scratch.
    ///
    /// `began` and `remote` are only the open-latency lines, and `path` is
    /// the one thing a remote track can't have: a second read of the file for
    /// a fragmented MP4's duration.
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
        // The probe reads the container's headers, which is third-party
        // parsing of file bytes like the decode below. A panic in there
        // leaves nothing to be inconsistent: the reader it was building
        // never got out of the call, and everything it borrowed is dropped
        // on the way to the error. Bytes off a socket are the same bet: the
        // reader owns the connection and drops it on the way out.
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

        // Second of the open-latency timings, and the one that's ours to
        // shorten if it's the long pole: the probe reads until it recognises
        // a container, and on a stream every byte it wants is a byte off the
        // wire at the station's own bitrate.
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

        // num_frames already excludes encoder delay and padding in 0.6. A
        // zero out of either of these is the reader saying it doesn't know
        // rather than the file being empty, so it reads as no value and
        // falls through to the next one.
        let stated_frames = track.num_frames.filter(|n| *n > 0);
        let stated_secs = track
            .duration
            .filter(|dur| dur.get() > 0)
            .zip(time_base)
            .and_then(|(dur, tb)| tb.calc_time(Timestamp::from(dur.get() as i64)))
            .map(|t| t.as_secs_f64())
            .or_else(|| stated_frames.map(|n| n as f64 / sample_rate as f64));

        // A fragmented MP4 states its length in the movie header and
        // nowhere symphonia looks, so without this the whole file reads as
        // zero seconds long: no seek bar range, and a fade window that
        // treats the track as ended before it started. It reads the file a
        // second time, so a remote track goes without and keeps whatever the
        // container stated.
        let file_secs =
            stated_secs.or_else(|| path.and_then(rox_library::mp4::fragment_duration_secs));
        let file_frames =
            stated_frames.or_else(|| file_secs.map(|s| (s * sample_rate as f64).round() as u64));

        // Building the decoder runs the codec's own setup over the header
        // fields, so it panics on the same class of bad file the decode
        // does. Nothing survives the failure either: the half-built decoder
        // is dropped inside the call and the source is never constructed.
        let decoder = guard_decode("decoder setup", &origin, || {
            crate::codecs::registry().make_audio_decoder(params, &AudioDecoderOptions::default())
        })?
        .map_err(|e| format!("decoder: {e}"))?;

        // The span on the file's own frame clock, which is the only clock
        // its end can be honored on: everything downstream of the resampler
        // counts device-rate frames, and a boundary cut there would fall a
        // sample or two either side of where the sheet put it.
        let span_frames = span.map(|s| SpanFrames {
            start: ms_frames(s.start_ms, sample_rate),
            end: s.end_ms.map(|end| ms_frames(end, sample_rate)),
        });

        // How long the track is, which for a spanned source is how long the
        // span is. Measured between the two frames its boundaries resolve
        // to rather than from the millisecond difference, because at a rate
        // where a millisecond isn't a whole number of frames those two
        // answers differ by one and the frame clock is the one the decode
        // actually follows. The open-ended span, the last track of an
        // image, borrows the file's own end and takes its start off it.
        let start_secs = span_frames.map_or(0.0, |sf| sf.start as f64 / sample_rate as f64);
        let (duration_secs, num_frames) = match span_frames {
            Some(sf) => {
                let frames = match sf.end {
                    // A sheet whose last timestamp runs past the file gets
                    // the file's end, so the length it reports is one the
                    // track can actually reach.
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

        // The playable length at the device rate, which is the clock the
        // fade window is measured on. Prefer the frame count the container
        // states (encoder delay and padding already out of it) and fall
        // back to the duration.
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

        // Seek to the span's first frame before the caller ever asks for a
        // packet, so the source hands back the track from its very first
        // chunk. Same accurate seek a scrub uses, decoder and resampler
        // reset with it, and the decode drops whatever the packet-granular
        // landing left in front of the span. A span starting at zero is
        // already where it needs to be.
        if span_frames.is_some_and(|sf| sf.start > 0) && source.seek_file(start_secs).is_none() {
            return Err(format!("seek to span start {start_secs}s failed"));
        }

        Ok((source, info))
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

    /// Frames of the file still owed before the span's end, on the file's
    /// own clock. None where nothing bounds the decode: a whole file, or the
    /// open-ended span that runs to the file's end anyway and lets natural
    /// EOF do the stopping.
    fn span_left(&self) -> Option<u64> {
        self.span?.end.map(|end| end.saturating_sub(self.src_frame))
    }

    /// Decode packets until one yields samples, appending device-rate stereo
    /// to `out` with this source's own gain applied. Returns false at end of
    /// stream.
    fn next_chunk(&mut self, device_rate: u32, out: &mut Vec<f32>) -> bool {
        let from = out.len();
        let more = self.decode_chunk(device_rate, out);

        // How many bytes that audio cost, which is the only exact answer to
        // how long a byte of this station is. The tape holds the byte count
        // itself; this is the other half of the ratio, and the window it
        // sizes and the distance it reports both come out of it.
        if let Some(tape) = self.tape.as_ref() {
            tape.note_audio((out.len() - from) as f64 / 2.0 / device_rate as f64);
        }
        // The source-gain stage (ADR 19), before this source's samples meet
        // any other's. A fade's per-frame pair applies over in the engine,
        // where both sources are in hand.
        gain::apply(&mut out[from..], self.gain);
        self.pos_frames += ((out.len() - from) / 2) as u64;

        // Last of the open-latency timings: audio exists. Taken rather than
        // read, so this prints once per stream and the branch after it is an
        // `Option` that's already None.
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

    /// The decode itself: packets in, device-rate stereo appended to `out`.
    fn decode_chunk(&mut self, device_rate: u32, out: &mut Vec<f32>) -> bool {
        // A span already at its end is a track already over, and it does
        // exactly what the file's own end does: drain the resampler's
        // carried frame and stop. Everything upstream of here then takes the
        // gapless boundary it would have taken at EOF.
        if self.span_left() == Some(0) {
            self.resampler.flush(out);
            return false;
        }
        // A source that already panicked is done. The flush happened on the
        // way into the poison, so there is nothing left to hand out.
        if self.poisoned {
            return false;
        }
        loop {
            // The reader and the decoder are both third-party code over
            // file bytes. A panic in either ends the track the way a fatal
            // error does, and poisons the source so the loop never re-enters
            // the state the unwind came out of. See [`guard_decode`] for why
            // asserting unwind safety holds here.
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

            // The copy out of the decoder's buffer happens inside the guard
            // too: the borrow of the decoded audio lives as long as the
            // decoder call it came from, and reading it is as much the
            // codec's code as producing it was.
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
                // A packet that decoded to nothing: nothing to hand on, so
                // take the next one.
                Ok(None) => continue,
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

            // The span's first frame. A format resolves an accurate seek to
            // the packet holding the timestamp, which is near enough for a
            // scrub and nowhere near enough for a cue seam: the frames
            // between where the seek landed and where the track starts
            // belong to the track before it, and playing them would double
            // them up at the boundary. They go here, in the file's own
            // frames, before anything downstream can hear them.
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

            // The span's end falls somewhere inside a packet, so the packet
            // is cut here too, still in the file's own frames, before the
            // resampler smears the boundary across a fractional step. Past
            // the cut this is the natural EOF path to the sample: flush the
            // resampler's carried frame, report the track over, and let the
            // caller splice the next one in behind it.
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

    /// Past B, or at EOF with a section marked: trim the overshoot off the
    /// chunk just decoded and wind the reader back to A. Returns where it
    /// landed, track-relative seconds, when it wrapped; None when there was
    /// nothing to do.
    ///
    /// The cut and the seek are one call so the wrap is testable without a
    /// ring behind it. `out` holds only the chunk this pass decoded, which
    /// the engine guarantees by clearing it before every refill, so the
    /// truncate takes off exactly the frames that ran past B.
    ///
    /// A failed seek comes back as None and the caller carries on into
    /// normal playback. That's the right failure: a loop that can't wrap
    /// keeps playing the track rather than stalling on it.
    fn wrap_ab(&mut self, a: u64, b: u64, eof: bool, out: &mut Vec<f32>) -> Option<f64> {
        if self.pos_frames < b && !eof {
            return None;
        }
        let over = self.pos_frames.saturating_sub(b) as usize * 2;
        out.truncate(out.len().saturating_sub(over));
        self.seek(a as f64 / self.device_rate as f64)
    }

    /// Accurate seek. Returns the track position actually landed on, in
    /// seconds, which can differ from the request. None when the seek failed,
    /// so the caller doesn't register a segment that jumps the position
    /// display to a spot playback never reached.
    ///
    /// Both the request and the result are track-relative, so a span's 0:00
    /// is its own first frame rather than the image file's. That keeps the
    /// seek strip and the position clock reading a cue track the way they
    /// read a plain file, with no arithmetic of their own.
    fn seek(&mut self, secs: f64) -> Option<f64> {
        let secs = self.inside_track(secs);
        let Some(span) = self.span else {
            let landed = self.seek_file(secs)?;
            // The track position moved, so the fade window's countdown
            // moves with it: a seek into the last seconds of a track
            // opens the window, a seek back out of it closes it again.
            self.pos_frames = (landed * self.device_rate as f64).round() as u64;
            return Some(landed);
        };
        let rate = self.resampler.src_rate() as f64;
        let start = span.start as f64 / rate;
        // Clamped to the span at both ends: a scrub past the end of a cue
        // track belongs on that track's last frame, never inside the one
        // after it, which is the same file a few seconds along.
        let target = match span.end {
            Some(end) => (start + secs.max(0.0)).clamp(start, end as f64 / rate),
            None => start + secs.max(0.0),
        };
        let landed = self.seek_file(target)?;
        let rel = (landed - start).max(0.0);
        self.pos_frames = (rel * self.device_rate as f64).round() as u64;
        Some(rel)
    }

    /// Pull a seek target back inside the track. The last frame isn't
    /// somewhere a reader can land: the seek comes back "unexpected end of
    /// file" and the attempt leaves the reader parked at the end, so the next
    /// chunk reads as the track finishing. Dragging the seek strip to its
    /// right edge asks for exactly the duration, so that scrub would end the
    /// track, and the whole queue with it on the last entry.
    ///
    /// The margin falls inside the final packet either way, which is where a
    /// drag to the edge means to go. A track that never claimed a length has
    /// no ceiling to clamp against and goes through as asked.
    fn inside_track(&self, secs: f64) -> f64 {
        let Some(total) = self.total_frames else {
            return secs;
        };
        let end = total as f64 / self.device_rate as f64 - SEEK_END_MARGIN_SECS;
        secs.min(end.max(0.0))
    }

    /// The seek itself, against the file's own timeline: move the reader,
    /// reset the decoder and the resampler around the jump, and report where
    /// it actually landed. Split out because a spanned source seeks twice
    /// over, once to enter its span and again for every scrub inside it, and
    /// only the caller knows which clock the seconds are on.
    fn seek_file(&mut self, secs: f64) -> Option<f64> {
        let time = Time::try_from_secs_f64(secs).unwrap_or(Time::ZERO);
        // An accurate seek is a read and, on some containers, a decode up to
        // the target, so it lands on the same third-party code a chunk does.
        // A panic there poisons the source and reads as a seek that failed,
        // which every caller already handles.
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
                // Re-arm rather than rebuild: the rates either side of a
                // seek are the same ones, and the sinc table costs a
                // millisecond and a half to recompute. Clearing the filter
                // history is the whole of what the jump needs, and an A-B
                // wrap pays this on every pass around the section.
                self.resampler.reset();
                let landed = self
                    .time_base
                    .and_then(|tb| tb.calc_time(seeked.actual_ts))
                    .map(|t| t.as_secs_f64().max(0.0))
                    .unwrap_or(secs);
                // The span's end is an absolute spot in the file, so where
                // the reader really is counts toward it, not where the seek
                // was aimed. A coarse landing shortens or lengthens
                // the decode rather than moving the boundary.
                self.src_frame = (landed * self.resampler.src_rate() as f64).round() as u64;
                Some(landed)
            }
            Err(e) => {
                log::warn!("seek failed: {e}");
                // No landing to report, so the caller leaves the clock where
                // it was. The reader itself may well have moved (a target past
                // the last frame parks it at the end), which is why the target
                // is clamped inside the track before it gets here.
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

    /// A local locator for a fixture path, since the engine asks where a
    /// track's bytes come from and every fixture here is a file.
    fn local(path: impl Into<PathBuf>) -> Locator {
        Locator::Local(path.into())
    }

    /// An engine wired over synthetic paths and a throwaway ring, no audio
    /// device and no decode thread. Enough to drive the pure queue-edit math:
    /// order, cursor, and the runahead detection. `n` context entries with
    /// stable ids 0..n.
    fn test_engine(n: usize) -> Engine {
        engine_over(
            (0..n)
                .map(|i| Locator::Local(PathBuf::from(format!("t{i}"))))
                .collect(),
        )
    }

    /// The same over locators the caller picked, for the tests that open real
    /// files instead of driving the queue math over synthetic ones.
    fn engine_over(locators: Vec<Locator>) -> Engine {
        engine_with_ring(locators, 8).0
    }

    /// [`engine_over`] with the ring's read side kept, for the tests that have
    /// to see what the audio callback would find in it. `frames` stereo frames
    /// of capacity.
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

    /// The similarity mode's reorder: the named entries lead in the order
    /// given, the playing track and everything behind it never move, and
    /// entries nobody ranked keep their own order behind the ranked ones.
    /// That last part lets a partly analyzed library work at all.
    #[test]
    fn order_tail_ranks_what_it_knows_and_leaves_the_rest() {
        let mut engine = test_engine(6);
        engine.pos = 1;
        let ids: Vec<u64> = engine.order.iter().map(|e| e.id).collect();
        // Rank two of the four upcoming entries, last one first.
        engine.order_tail(&[ids[5], ids[3]]);
        let after: Vec<u64> = engine.order.iter().map(|e| e.id).collect();
        assert_eq!(
            after,
            vec![ids[0], ids[1], ids[5], ids[3], ids[2], ids[4]],
            "ranked entries lead in the given order, the rest keep theirs"
        );
        // History and the playing entry are untouched, which is the whole
        // contract this shares with set_shuffle.
        assert_eq!(&after[..2], &ids[..2]);
        assert_eq!(engine.pos, 1);
    }

    /// An empty ranking, what an unanalyzed library produces, leaves the
    /// queue exactly as it found it rather than scrambling it.
    #[test]
    fn order_tail_with_nothing_ranked_changes_nothing() {
        let mut engine = test_engine(4);
        engine.pos = 0;
        let before: Vec<u64> = engine.order.iter().map(|e| e.id).collect();
        engine.order_tail(&[]);
        let after: Vec<u64> = engine.order.iter().map(|e| e.id).collect();
        assert_eq!(before, after);
        // An id that isn't in the queue is ignored rather than panicking.
        engine.order_tail(&[9999]);
        assert_eq!(
            before,
            engine.order.iter().map(|e| e.id).collect::<Vec<_>>()
        );
    }

    /// Nothing upcoming is not an error: the call publishes and returns.
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

    /// The bug this anchor exists for: turn on the radio during the last
    /// seconds of a track, where the decode cursor has already stepped onto
    /// the next entry and opened it, and the reorder has to reach that entry
    /// too. Anchored on the cursor it wouldn't, and the pre-rolled track would
    /// play next whatever the ranking said.
    #[test]
    fn order_tail_reaches_the_pre_rolled_next_track() {
        let mut e = test_engine(5);
        // Audible on track 1, decode cursor pre-rolled onto track 2.
        set_audible(&e, 1);
        e.pos = 2;
        // Rank the last entry to the front of what's coming.
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
        // Re-anchored onto the audible entry, so the caller's reopen at
        // audible + 1 lands on the newly ranked track rather than past it.
        assert_eq!(e.pos, 1);
        assert_eq!(e.order[e.pos + 1].id, 4);
    }

    /// A reorder that leaves the pre-rolled track where it was keeps the open
    /// source: nothing about the handoff changed, so there's nothing to redo.
    #[test]
    fn order_tail_keeps_the_runahead_when_the_next_slot_holds() {
        let mut e = test_engine(5);
        set_audible(&e, 1);
        e.pos = 2;
        // Rank the entry that's already next first, then reshuffle behind it.
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

    /// Mid-track, cursor and audible together, there's no pre-roll to
    /// invalidate: the reorder covers everything after the playing entry and
    /// reports nothing back.
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

    /// Shuffle rides the same anchor, so flipping it on near a boundary
    /// reaches the pre-rolled track the same way the similarity reorder does.
    #[test]
    fn set_shuffle_off_reaches_the_pre_rolled_next_track() {
        let mut e = test_engine(5);
        set_audible(&e, 1);
        e.pos = 2;
        // Scramble the tail by hand, then let shuffle-off sort it back to pool
        // order: entry 4 is dragged into the slot the pre-roll holds.
        e.order.swap(2, 4);
        let moved_runahead = e.set_shuffle(false);
        assert_eq!(
            e.order.iter().map(|en| en.id).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert!(moved_runahead, "the open source was entry 4, now not next");
        assert_eq!(e.pos, 1);
    }

    /// On the last entry under Loop All the upcoming portion is everything in
    /// front of it, and the decode cursor has already wrapped there. Anchored
    /// on the audible entry alone the reorder found nothing past it and every
    /// Similar batch or shuffle toggle on the final track did nothing.
    #[test]
    fn order_tail_wraps_to_the_front_under_loop_all() {
        let mut e = test_engine(4);
        e.loop_mode = LoopMode::All;
        // Audible on the last entry, cursor wrapped onto entry 0 and holding
        // it open for the handoff, nothing pushed for it yet.
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
        // Re-anchored onto the audible entry, which puts the reopen's
        // audible + 1 past the end and wraps it to the new front.
        assert_eq!(e.pos, 3);
        assert_eq!(e.order[0].id, 2);
    }

    /// The same wrap for the shuffle toggle, with no pre-roll to invalidate.
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

    /// Once the runahead has frames in the ring, displacing it would splice:
    /// the reopen drops the source but not what it already pushed. Similar
    /// mode reorders on every batch landing, so the sort starts past the
    /// pre-rolled track instead and the new ranking lands behind it.
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

    /// A boundary crossfade is the narrow window that keeps the old cursor
    /// anchor: the entry at the cursor is already mixing into the output, so
    /// the reorder starts past it instead of cutting audio the listener is
    /// hearing.
    #[test]
    fn a_reorder_during_a_boundary_fade_leaves_the_incoming_track_alone() {
        let fx = Fixtures::new("fade-reorder");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 0.2)),
            local(fx.wav("b.wav", 0.2)),
        ]);
        // Two more entries so there's a tail to reorder behind the fade.
        e.insert(
            None,
            vec![local("c"), local("d")],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
        );
        // Track 0 is going out under the fade, track 1 is the one coming in,
        // so the cursor has adopted 1 while the clock still reads 0.
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

    /// The wrap itself, over a real decoder and no ring: a 0.5 s section of
    /// a 2 s file, drained past where the file would have ended. Three
    /// claims in one drive, because they're the same drive: the source
    /// keeps going, the cut lands on B and not a packet past it, and what
    /// plays after the wrap is what plays at A.
    #[test]
    fn an_ab_loop_wraps_on_b_and_comes_back_to_a() {
        let fx = Fixtures::new("ab-wrap");
        let path = fx.wav("t.wav", 2.0);
        let rate = 48_000u32;
        let a_secs = 0.5;
        let a = (a_secs * rate as f64) as u64;
        let b = rate as u64;

        // What A sounds like, off a source that only seeks there. The
        // fixture is a sine, so a window this size is a real comparison
        // rather than two runs of silence matching.
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
        // Four seconds off a two second file. Without the loop the source
        // is finished halfway through this.
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

    /// B past the end of the track, which is where a container that
    /// overstates its length puts it. `next_chunk` runs out first, and the
    /// EOF flag is the whole reason the section still repeats instead of
    /// the track quietly ending under it.
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

    /// The published snapshot is the only copy anyone outside the engine
    /// reads (ADR 16), so it has to survive the round trip through frames
    /// and back, and it has to go quiet the moment the section does.
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

    /// A section belongs to the track it was marked on, so scrubbing out of
    /// it ends it. Scrubbing around inside it doesn't: replaying the first
    /// half of a bar you're drilling is the point of the feature.
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

    /// A skip is a move to different music, and the section doesn't come
    /// with it. Covers Next, Prev, and Jump alike: they all reach the same
    /// clear on the way through the flush.
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

    /// A section set in the last seconds of a track would otherwise open
    /// the boundary crossfade on every wrap, fading into a track the loop
    /// is never going to reach.
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
    /// wait out its deadline.
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

    /// Take the pre-decoded next track from open to feeding the ring: a
    /// segment for it ahead of the output clock, with the push counter past
    /// that segment's start. `set_audible` on its own leaves the runahead
    /// merely open, which is the silent-to-reopen case.
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
        // A four second window opening at 100_000, so the clock flips to the
        // incoming track at the midpoint, two seconds later.
        engine.adopt(1, info, 0, 96_000);
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
        // Consumed is between segment 1 and 2: drop segment 0, keep 1 (the
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
        // goes to the right next track.
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
            vec![local("a"), local("b"), local("c")],
            vec![Some(7), Some(7), None],
            Vec::new(),
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

    /// A continuation batch (ADR 17) is an append into the running session:
    /// no `after`, not explicit, added behind everything already queued
    /// while the cursor and the history in front of it stay exactly put.
    #[test]
    fn a_continuation_batch_appends_behind_the_cursor() {
        let mut e = test_engine(4);
        // Three tracks in, one to go, which is where the pump fires.
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
        // Context, not queue: what continuation adds plays on around the
        // listener rather than showing up as tracks they picked.
        assert!(
            snap.entries[4..].iter().all(|en| !en.explicit),
            "an appended batch is context"
        );
        // The pool grew alongside the order, which keeps the position
        // mapping and the shared track list resolving.
        assert_eq!(e.queue.len(), 6);
        assert_eq!(e.groups[4..], [Some(9), Some(9)]);
        assert_eq!(e.shared.tracks.lock().unwrap().len(), 6);
    }

    /// Shuffle folds an appended batch into the upcoming permutation instead
    /// of leaving it in provider order at the tail. Under the similarity mode
    /// the player sends the reorder straight after the insert on this same
    /// channel, so the engine sees the pair, and the appended entries have to
    /// be reachable by it.
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
        // Rank two of the appended entries to the front of what's coming.
        e.order_tail(&[6, 4]);
        // History and the playing entry are untouched, which is the contract
        // every tail reorder holds to.
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

    /// The last acceptance of #36, at the queue-math level: a session that
    /// played out to the end and got a batch appended has somewhere to go.
    /// `insert` hands back that position, which the run loop routes through
    /// the nav path to wake the session (see the Insert arm).
    #[test]
    fn appending_to_a_played_out_queue_names_the_track_to_wake_into() {
        let mut e = test_engine(2);
        // Played through: the cursor is on the last entry and the source
        // is gone, which is the ended state.
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
        // Group, gain, and span vecs shorter than paths pad rather than panic.
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
        // A cue track's span goes in beside its gain, and the entry the
        // caller said nothing about plays its whole file.
        assert_eq!(e.spans[1].map(|s| s.start_ms), Some(1_000));
        assert_eq!(e.spans[2], None);
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
        // Right on the end of the track, which is where a window would open
        // if there were one.
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
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 4.0)),
            local(fx.wav("b.wav", 4.0)),
        ]);
        e.fade_secs = 4.0;
        let source = ready_to_skip(&mut e, 48_000);

        let after = e.skip_to(source, 1, false, None);
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
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 4.0)),
            local(fx.missing("gone.wav")),
        ]);
        e.fade_secs = 4.0;
        let source = ready_to_skip(&mut e, 48_000);

        // Nothing past the skip target opens, so nothing drives a mix.
        let after = e.skip_to(source, 1, false, None);
        assert!(after.is_none());
        assert!(e.fade.is_none());
        // The clock is frozen at the cut with no source to move it, so a
        // fade published here would stay on the transport for good.
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
        let mut e = engine_over(vec![local(&path)]);
        let (src, _) = Source::open(&local(&path), 48_000, None).expect("the fixture opens");
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
        // Off is off all the way down: the window never opens, so no skip
        // pays for a wind-back it can't use.
        let mut e = test_engine(2);
        set_groups(&mut e, &[None, None]);
        e.fade_secs = crossfade_secs(f32::NAN);
        assert!(!e.window_open(Some(100), Some(0)));
    }

    /// The stop falls on the last track of a queue with looping off, so
    /// there's nothing to cue. The pause still has to go in: a session left
    /// reading as playing gets started again by the next thing that wakes it,
    /// a queue edit or a continuation batch, against a stop the listener
    /// asked for.
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

    /// The ordinary case, mid-queue: paused, with the track EOF would have
    /// opened handed back for Play to resume into.
    #[test]
    fn a_stop_mid_queue_pauses_and_names_what_play_resumes() {
        let mut e = test_engine(3);
        e.pos = 1;
        e.stop_after = true;
        e.stop_pending = true;
        assert_eq!(e.land_stop(), Some(2));
        assert!(!e.shared.playing.load(Ordering::Relaxed));
    }

    /// Disarmed while the ring drained: the session rolls on rather than
    /// pausing, which is why the flag is read here and not where it was set.
    #[test]
    fn a_stop_disarmed_during_the_drain_rolls_on() {
        let mut e = test_engine(3);
        e.pos = 1;
        e.stop_after = false;
        e.stop_pending = true;
        assert_eq!(e.land_stop(), Some(2));
        assert!(e.shared.playing.load(Ordering::Relaxed), "no pause landed");
    }

    /// A batch arriving (ADR 17) between the last track's EOF and the ended
    /// state finds no source open, and the nav route it takes must not cut
    /// the ring: the ending is still coming out of it, and there's nothing
    /// playing over it that a cut would hurry along.
    #[test]
    fn a_skip_into_a_draining_ring_lets_the_ending_finish() {
        let fx = Fixtures::new("skip-draining");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 1.0)),
            local(fx.wav("b.wav", 1.0)),
        ]);
        // Four frames of the last track still queued and unheard, with no
        // source decoding: the state the run loop is in while the ring drains.
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
        // The new track's segment is where its first sample will actually
        // be heard, behind the tail rather than on top of it.
        let segments = e.shared.segments.lock().unwrap();
        assert_eq!(segments.last().map(|s| s.at_frame), Some(1_000));
    }

    /// The ring already empty is the ordinary ended state, and a skip out of
    /// it cuts as it always did: there's nothing left to protect, and the
    /// flush resyncs the clock onto the new track.
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

    /// A play-from-bookmark: the skip opens the new track at the offset it
    /// was asked for, so its head never reaches the ring, and the segment
    /// says the track is that far in from its first frame rather than
    /// reading zero until a later seek corrects it.
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
        // A seek lands on a packet boundary, so the spot is near the ask
        // rather than on the frame; what has to hold exactly is that the
        // clock's segment says the same thing the source does.
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
        // And with no landing asked for, the same skip starts at the top.
        let src = e
            .skip_to(None, 0, false, None)
            .expect("the first fixture opens");
        assert_eq!(src.pos_frames, 0);
        let segments = e.shared.segments.lock().unwrap();
        assert_eq!(segments.last().map(|s| s.track_frame), Some(0));
    }

    /// The queue played out and the last track's decoder is long gone, so a
    /// click on the seek strip has nothing open to scrub. It reopens the
    /// track the listener is looking at and lands there, instead of leaving
    /// the transport dead until something else revives the session.
    #[test]
    fn a_seek_out_of_the_ended_state_reopens_the_finished_track() {
        let fx = Fixtures::new("seek-ended");
        let mut e = engine_over(vec![
            local(fx.wav("a.wav", 1.0)),
            local(fx.wav("b.wav", 1.0)),
        ]);
        // Both tracks played, the ring drained, nothing decoding: the ended
        // state exactly as the run loop leaves it.
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
        // Where it lands is the nearest packet boundary the decoder can
        // start on, not the sample the click named, so this asks for the
        // neighborhood rather than the frame.
        let segments = e.shared.segments.lock().unwrap();
        let landed = segments.last().map(|s| s.track_frame).expect("a segment");
        assert!(
            landed.abs_diff(24_000) < 4_800,
            "the clock lands where the click asked, half a second in, got {landed}"
        );
    }

    /// Dragging the seek strip to its right edge asks for exactly the
    /// duration, and the reader has no frame to land on there: the seek comes
    /// back end-of-file and leaves it parked at the end, so the run loop reads
    /// the very next chunk as the track finishing. On the last entry of a
    /// queue that's the whole thing stopping on a scrub.
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

    /// A cue span, written the way a sheet gives one: milliseconds in, an
    /// open end for the last track of an image.
    fn span(start_ms: u32, end_ms: Option<u32>) -> Span {
        Span { start_ms, end_ms }
    }

    /// Drain a source start to finish the way the run loop does, and hand
    /// back the interleaved stereo it produced. The fixtures are 48 kHz and
    /// so is the device rate here, so the resampler is a passthrough and the
    /// samples come back bit for bit.
    fn decode_all(path: &PathBuf, span: Option<Span>) -> Vec<f32> {
        let (mut src, _) = Source::open(&local(path), 48_000, span).expect("the fixture opens");

        drain_source(&mut src)
    }

    /// The same over a source already open, for the tests that built one
    /// themselves.
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

    /// The transparency proof for the whole unit: the same bytes opened over
    /// the HTTP transport come back as the same track they do off the disk,
    /// header and samples both. Nothing downstream of [`Source::open`] gets
    /// to know which one it's playing.
    ///
    /// The transport is a fake serving the fixture's bytes, so this needs no
    /// network and no server.
    #[test]
    fn a_remote_open_reads_the_same_track_as_the_file() {
        let fx = Fixtures::new("remote-transparency");
        let path = fx.wav("tone.wav", 2.0);
        let bytes = std::fs::read(&path).expect("the fixture is readable");
        let served = crate::http::testing::Fake::serving(bytes, "audio/wav");

        let (mut local, local_info) =
            Source::open(&local(&path), 48_000, None).expect("the file opens");

        // The hint is empty on purpose: the probe gets the container off the
        // fake's `Content-Type`, which is the path a real server takes.
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

        // And the audio itself, not just what the header claimed about it.
        assert_eq!(drain_source(&mut remote), drain_source(&mut local));
    }

    /// The station title reaches the slot the player reads, keyed to the pool
    /// entry that's playing, and the revision counts songs rather than
    /// metadata blocks. A station resends the current title every few seconds
    /// between changes, so a revision that moved on every block would make a
    /// scrobble fire dozens of times per song.
    #[test]
    fn a_station_title_reaches_the_published_slot() {
        let fx = Fixtures::new("icy-title");
        let path = fx.wav("stream.wav", 1.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");

        // One title, the same title again, then a change, then hold. The
        // metaint is small enough that a one-second fixture carries all four.
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
            // Where the revision stood once the open was done with it. The
            // stream's own states move the same clock, so the songs are
            // counted as a delta from here rather than from zero.
            let opened = shared.title_rev();

            // Drain it, which is what carries the cursor past the marks the
            // feed thread left in the tape and fires the sink. A title lands
            // when the audio it was announced over is decoded, not when it
            // comes off the socket, so this has to be a decode rather than a
            // read. The fixture is a WAV and states its own length, so the
            // decode ends even though the station doesn't.
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

    /// What the station said about itself reaches the slot beside the title.
    /// Same trip, different clock: the headers are read once at the open,
    /// where the titles keep arriving for as long as the stream plays.
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
        // Everything one open publishes about a station rides the one
        // revision: the open starting, the open landing, and the description
        // read off the headers on the way through.
        assert_eq!(shared.title_rev(), 3, "the description moves the revision");
        assert_eq!(shared.stream_state(0), Some(StreamState::Live));
    }

    /// A locator for a live station, which is the one shape `hang_up` acts on.
    fn station(url: &str) -> Locator {
        Locator::Remote(Remote {
            url: url.into(),
            headers: Vec::new(),
            hint: "wav".into(),
            live: true,
        })
    }

    /// The same URL as an ordinary file over HTTP, which is what a Subsonic
    /// stream looks like from in here: seekable, a length, no hang-up.
    fn hosted(url: &str) -> Locator {
        Locator::Remote(Remote {
            url: url.into(),
            headers: Vec::new(),
            hint: "wav".into(),
            live: false,
        })
    }

    /// A fake answering the way a station does: 200, no length, in-band
    /// metadata carrying `titles`.
    fn live_fake(wav: &[u8], metaint: usize, titles: &[&str]) -> Arc<crate::http::testing::Fake> {
        // Padded past the audio so the stream outlasts the decode. A station
        // never ends, and a fake that does would have the feed thread
        // reconnecting in the background of every assertion; the trailing
        // silence carries the last title and nothing else.
        let mut served = wav.to_vec();
        served.resize(wav.len() * 2, 0);

        let body = crate::http::testing::interleave(&served, metaint, titles);
        let mut fake = crate::http::testing::Fake::serving(body, "audio/wav");
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            f.metaint = Some(metaint);
            // And served at a broadcast's pace rather than all at once, so
            // the tape fills the way one really does and the window never
            // rolls over the cursor mid-test.
            f.pace = Some((StdDuration::from_millis(1), 1024));
        }

        fake
    }

    /// Decode until the station has named a song, so the test has a real title
    /// in the slot rather than an empty one it can prove nothing about. The
    /// cap is generous: a metadata block lands every few packets at this
    /// `metaint` and the fixture carries dozens.
    ///
    /// On the title slot rather than the revision, which the stream's own
    /// state changes also move: entry 0, since every caller here is a
    /// single-station queue.
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

    /// The timeshift, from the pause's side. A broadcast has no pause of its
    /// own, so rox keeps one: the connection stays up, the feed thread keeps
    /// taping, and everything decoded before the press stays in the ring
    /// because that's the moment the listener stopped at and the moment the
    /// resume has to start from.
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

            // A second of it heard, samples in the ring: the engine where a
            // pause actually arrives.
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

            // The pause itself, the way the run loop takes it now: the flag
            // goes down and nothing else happens.
            shared.playing.store(false, Ordering::Relaxed);
            source = e.idle_hangup(source);

            assert!(source.is_some(), "still connected");
            assert_eq!(fake.closed_count(), 0, "the socket is still open");
            assert_eq!(fake.ask_count(), 1, "and nothing reopened anything");
            assert_eq!(ring.slots(), held, "the ring kept the pre-pause audio");
            assert!(e.hung_up.is_none(), "there's nothing to come back to");

            // And the tape is still filling under the pause, which is the
            // whole point: the resume has somewhere to carry on from.
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

    /// The one pause that still hangs up. Half an hour in there's nothing
    /// left in the tape that the listener paused on, so holding the socket
    /// open is only bandwidth: it goes, and the resume rejoins live.
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

            // One second heard, then paused, and the pause dated back past
            // the cap rather than waited out.
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

            // Nothing a reader depends on went with the connection.
            assert!(shared.tracks.lock().unwrap()[0].is_some());
            assert!(shared.live_title(0).is_some());
            assert!(!shared.ended.load(Ordering::Relaxed));
        });
    }

    /// The step back through the buffer. Nothing goes over the wire: the
    /// bytes are in the tape already, so what a seek costs is a decoder and
    /// the cut every seek costs.
    ///
    /// The fixture is a WAV, which can only be probed from its own header, so
    /// the point seeked to here is the top of the tape. A real station is MP3
    /// or AAC and re-syncs on a frame header wherever the cursor lands; that
    /// half is not testable without an encoder and is verified by ear.
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

            // Decode a while, so there's something behind the cursor to go
            // back into.
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

            // Further back than the tape holds, which lands on the oldest
            // byte in it.
            source = e.seek_live_to(source, 3600.0);

            assert!(source.is_some(), "still playing");
            assert!(
                tape.cursor() < was,
                "the cursor went back: {} from {was}",
                tape.cursor()
            );
            assert_eq!(fake.ask_count(), 1, "and asked the station for nothing");

            // And it plays from there, which is the part that says the
            // decoder was really rebuilt rather than merely replaced.
            chunk.clear();
            assert!(source.as_mut().unwrap().next_chunk(48_000, &mut chunk));
            assert!(!chunk.is_empty(), "audio out of the new cursor");
        });
    }

    /// A seek into the middle of the stream, where a container may or may
    /// not be able to pick the stream back up. Either answer has to leave a
    /// station playing: the listener asked to move inside a broadcast, and
    /// the worst outcome would be silence for having asked.
    ///
    /// Which way it goes isn't asserted because it isn't knowable from here.
    /// A refusal keeps the old decoder and the old cursor, a re-sync builds a
    /// new pair, and the fixture is a WAV whose probe hunts forward for a
    /// header rather than failing where it stands.
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

            // Half a second before the cursor. Measured off the cursor
            // rather than off the edge: the feed thread fills the tape
            // faster than a decode drains it, so "just behind the edge" is
            // nowhere near where the listener is.
            let behind = tape.shift().behind_secs + 0.5;
            source = e.seek_live_to(source, behind);

            assert!(source.is_some(), "still playing");
            assert_eq!(fake.ask_count(), 1, "and nothing dialled the station");

            chunk.clear();
            assert!(source.as_mut().unwrap().next_chunk(48_000, &mut chunk));
            assert!(!chunk.is_empty(), "audio carries on");
        });
    }

    /// The output device faulting under a station. The stream is dead and the
    /// ring with it, but nothing above the ring has anything wrong with it:
    /// the swap hands the engine a new producer and the connection, the tape
    /// and the decoder all carry on. Tearing the session down for this is
    /// what used to re-dial the station and throw its timeshift away.
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

            // The fault: a fresh ring arrives and the old one is gone,
            // everything else untouched.
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

            // The ring really did change hands: a push lands in the new one.
            e.producer.push(0.5).expect("the new ring has room");
            assert_eq!(fresh.pop().ok(), Some(0.5));

            // And the tape is still being filled by the same feed thread,
            // which is the half a re-dial would have restarted.
            wait_for("the tape to keep filling", || {
                tape.shift().window_secs > window
            });

            chunk.clear();
            assert!(source.as_mut().unwrap().next_chunk(48_000, &mut chunk));
            assert!(!chunk.is_empty(), "audio carries on out of the new cursor");
        });
    }

    /// The same fault taken while paused. The clock behind the idle hangup
    /// counts how long nobody has been listening, and a device dropping out
    /// is not somebody coming back: restarting it would hold a socket open
    /// for another half hour every time an output glitched.
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

            // Paused, and the pause dated most of the way to the cap.
            shared.playing.store(false, Ordering::Relaxed);
            source = e.idle_hangup(source);
            let since = Instant::now() - StdDuration::from_secs(LIVE_IDLE_HANGUP_SECS - 5);
            e.paused_since = Some(since);

            let (producer, _fresh) = rtrb::RingBuffer::<f32>::new(64 * 2);
            source = e.swap_output(producer, source);

            assert!(source.is_some(), "the pause still holds the connection");
            assert_eq!(e.paused_since, Some(since), "the pause clock didn't move");
            assert_eq!(fake.ask_count(), 1, "and nothing was dialled");

            // Which is to say the cap still lands where it would have.
            e.paused_since = Some(Instant::now() - StdDuration::from_secs(LIVE_IDLE_HANGUP_SECS));
            source = e.idle_hangup(source);
            assert!(source.is_none(), "the cap still hangs up");
        });
    }

    /// A fault arriving on a station a pause already hung up on. There's no
    /// source, no socket and no tape; the one thing the swap must not do is
    /// decide that a new output device is a reason to dial the station back
    /// up under a pause nobody has lifted.
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

    /// The same fault on a local file. There's no tape to re-sync over, so
    /// the swap seeks to the spot the position clock stopped at: the half
    /// second the dead ring still held was pushed and never heard, and
    /// carrying on from the decode cursor would skip it.
    #[test]
    fn a_device_swap_on_a_file_resumes_where_the_clock_stopped() {
        let fx = Fixtures::new("file-swap");
        let path = fx.wav("track.wav", 4.0);

        let (mut e, _ring) = engine_with_ring(vec![local(path)], 64);
        e.shared.flush_ack.store(u64::MAX, Ordering::Release);

        let mut source = e.open_at(0);
        assert!(source.is_some(), "the file opens");

        // A second heard, and the decoder a way past it, which is where a
        // device faults in practice: the ring full of audio nobody got.
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
        // Within a packet either way: the seek lands where the container's
        // granularity lets it, and the clock follows it there rather than
        // claiming the spot that was asked for.
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

    /// Play rejoins at the live edge: one fresh open, the same one a first
    /// open makes, and the elapsed clock picks up where the pause froze it
    /// rather than restarting. The counter says how much of the station has
    /// been heard, and a pause adds nothing to that either way.
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

            // One second heard before the pause, which is what the resume has
            // to carry forward.
            shared.frames_consumed.store(48_000, Ordering::Relaxed);
            shared.playing.store(false, Ordering::Relaxed);
            source = e.hang_up(source.take());
            assert!(source.is_none() && e.hung_up == Some(48_000));

            // Where the revision stood going into the rejoin, since the
            // reopen's own two states move it as well as any song change.
            let paused_at = shared.title_rev();
            source = e.rejoin();

            assert!(source.is_some(), "the station comes back");
            assert!(e.hung_up.is_none(), "and the pause state is spent");
            assert!(shared.playing.load(Ordering::Relaxed), "playing again");
            assert_eq!(fake.ask_count(), 2, "exactly one new request");
            // The same request a first open makes. A station serves now
            // whatever offset is named, so the open has no reason to ask
            // differently; naming no range is the reconnect path's business.
            let range = fake.asks.lock().unwrap()[1].range;
            assert_eq!(range, Some(0));

            // The elapsed readout carries on rather than restarting: the clock
            // hasn't moved, and it still reads the second the listener heard.
            assert_eq!(shared.position(48_000), Some((0, 1.0)));

            // The station opened again on the same song, so the slot still
            // names it and the dedupe correctly counted no song change: the
            // revision moved for the reopen and for nothing else.
            assert_eq!(shared.live_title(0), titled);
            assert_eq!(
                shared.title_rev() - paused_at,
                2,
                "the reopen's two states, and no song change under them"
            );
        });
    }

    /// A transport that says what the session was doing at the moment each
    /// request went out, and takes its time answering the way a station on
    /// the other side of an ocean does. The gap between a play now and a
    /// stream's first byte is the only place the bug in question lived, and
    /// this is the one seam that can see into it.
    struct Slow {
        inner: Arc<crate::http::testing::Fake>,
        shared: Arc<Shared>,
        /// What the session was doing as each GET went out.
        at_get: std::sync::Mutex<Vec<AtGet>>,
    }

    /// One reading of the session, taken at a request.
    #[derive(Clone, Copy)]
    struct AtGet {
        /// The pause flag.
        playing: bool,
        /// The flush epoch, which counts the cuts.
        flushed: u64,
        /// What the position clock named, which is the answer every surface
        /// upstream gives to "what is loaded".
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

    /// Spin until `cond` holds, for the tests that drive the run loop from
    /// another thread. The deadline is generous: it's there to fail a broken
    /// build rather than to time anything.
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

    /// Play now onto a station, from a pause, with a local track still in the
    /// ring. The open is the expensive half of a skip and it deliberately
    /// happens before the cut, so that a file still coming out of the ring
    /// covers it. For a local file that open is instant. For a station it's a
    /// request, a probe and a decode, and a callback resumed at the command
    /// would spend all of it playing the track the listener had paused.
    ///
    /// So the resume waits for the flush. This drives the real run loop,
    /// because the ordering being tested is the run loop's.
    #[test]
    fn a_play_now_onto_a_station_stays_silent_until_the_ring_is_cut() {
        let fx = Fixtures::new("play-now-paused");
        let path = fx.wav("local.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, 4096, &["Boards of Canada - Roygbiv"]);

        let shared = Arc::new(Shared::new(1));
        // No backend here to handle the flush epoch, so the cut would sit out
        // its whole deadline. An ack from the future clears it.
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

        // Paused over the local file, which is where the listener was: the
        // ring holds its samples and nothing is coming out.
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
        // The wait has to show somewhere, and the only surface that can show
        // it is the one reading the position clock. So the clock names the
        // station before the request leaves, at its own zero, and the entry
        // whose `Opening` is published a line later is the entry upstream is
        // already looking at.
        assert_eq!(
            at.position,
            Some((1, 0.0)),
            "the clock had already moved to the station"
        );

        // The resume lands on the far side of the cut, so the first thing
        // heard is the station.
        wait_for("the resume", || shared.playing.load(Ordering::Relaxed));
        assert!(
            shared.flush_seq.load(Ordering::Relaxed) > 0,
            "the ring was cut"
        );

        // The entry the insert created has a slot of its own in everything
        // published per pool entry. It didn't before: the vectors parallel to
        // the pool stopped growing at the session's starting length, so a
        // station arriving by Play now published into nothing.
        assert_eq!(
            shared.stream_state(1),
            Some(StreamState::Live),
            "the inserted entry has somewhere to publish"
        );

        tx.send(Cmd::Quit).expect("the engine is listening");
        decode.join().expect("the decode thread ends cleanly");
    }

    /// The buffer setting moved while a station is on air.
    ///
    /// The tape is allocated when the connection is made, so nothing about
    /// the length can reach it except a command. This is the whole of that
    /// path: the number lands on the open tape, the shift the transport
    /// reads follows it, and the engine keeps it for the next station this
    /// session opens. The floor holds against anything a caller asks for,
    /// the same way the start applies it.
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

    /// A launch restore comes up paused, and the engine's very first act is
    /// to open the entry it starts on. For a station that would be a socket
    /// held open through a pause, which is the exact state a pause on a
    /// playing station goes out of its way to close: the server either drops
    /// us or keeps us however far behind the pause lasted. So a paused start
    /// on a station connects to nothing and waits, and Play is what dials.
    #[test]
    fn a_paused_start_on_a_station_waits_for_play_before_it_connects() {
        let fx = Fixtures::new("paused-start");
        let path = fx.wav("stream.wav", 2.0);
        let wav = std::fs::read(&path).expect("the fixture is readable");
        let fake = live_fake(&wav, 4096, &["Boards of Canada - Roygbiv"]);

        let shared = Arc::new(Shared::new(1));
        shared.flush_ack.store(u64::MAX, Ordering::Release);
        // The restore's own pause, down before the decode thread exists,
        // which is how the player does it.
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

        // The queue publish is the first thing the run loop does and the
        // open is the second, so past the one is past the other.
        wait_for("the session to come up", || shared.queue_rev() > 0);
        assert_eq!(
            fake.ask_count(),
            0,
            "a restore that came up paused asked the station for nothing"
        );
        // Loaded at its own zero all the same, so the transport has
        // something to show and something to press Play on.
        assert_eq!(shared.position(48_000), Some((0, 0.0)));
        assert_eq!(
            shared.stream_state(0),
            None,
            "nothing opened, nothing to say"
        );

        tx.send(Cmd::TogglePause).expect("the engine is listening");
        // Waiting on the open landing rather than on the pause flag, which
        // the rejoin puts up before it dials: Play means Play whether or not
        // the station answers, so the flag is true a moment before there's a
        // request to count.
        wait_for("the station to come on", || {
            shared.stream_state(0) == Some(StreamState::Live)
        });

        assert!(shared.playing.load(Ordering::Relaxed));
        assert_eq!(fake.ask_count(), 1, "and Play is one open, not two");

        tx.send(Cmd::Quit).expect("the engine is listening");
        decode.join().expect("the decode thread ends cleanly");
    }

    /// A seekable stream is a file that happens to arrive over a socket, and
    /// its pause is the one every file has always taken: the connection holds,
    /// the ring keeps what it holds, and the resume carries on mid-track.
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
            // Where the probe left things. It rewinds over the container
            // header while it settles on a format, and a rewind past the
            // window is its own request, so the pause is measured as a
            // difference rather than against one.
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

    /// A spanned source is the slice, not the file: it reports the slice's
    /// length, starts at its own 0:00 before anything decodes, and the first
    /// sample out of it is the one a second into the image.
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

        // And the audio really is the middle of the image rather than its
        // head: one second in for two seconds, sample for sample.
        let whole = decode_all(&path, None);
        let spanned = decode_all(&path, Some(span(1_000, Some(3_000))));
        assert_eq!(spanned, whole[48_000 * 2..144_000 * 2]);
    }

    /// The span's end is the track's end, cut on the frame the sheet named
    /// rather than wherever the packet carrying it happened to finish.
    #[test]
    fn a_span_ends_on_its_boundary_frame() {
        let fx = Fixtures::new("span-boundary");
        let path = fx.wav("image.wav", 4.0);
        // A boundary that falls mid-packet whatever the block size: 1234 ms
        // is 59_232 frames, which is no round number of anything.
        let spanned = decode_all(&path, Some(span(0, Some(1_234))));
        assert_eq!(spanned.len() / 2, 59_232, "cut on the exact frame");

        // And it's the head of the file, unaltered up to that frame.
        let whole = decode_all(&path, None);
        assert_eq!(spanned, whole[..59_232 * 2]);
    }

    /// The whole point of the exercise: two cue tracks of one image, played
    /// back to back, are the image. Nothing dropped at the seam, nothing
    /// played twice, and the frame counts add up to the region they cover.
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

    /// Seeking a cue track is seeking the track: the seconds going in and
    /// the seconds coming back are both measured from the span's own start,
    /// so the seek strip and the position clock need no arithmetic of their
    /// own. Past the end it clamps to the span rather than scrubbing into
    /// the next track, which is the same file a few seconds along.
    #[test]
    fn seeking_inside_a_span_stays_inside_it_and_reads_track_relative() {
        let fx = Fixtures::new("span-seek");
        let path = fx.wav("image.wav", 4.0);
        let (mut src, _) = Source::open(&local(&path), 48_000, Some(span(1_000, Some(3_000))))
            .expect("the image opens");

        // A container resolves a seek to the packet holding the timestamp,
        // so the landing is a few milliseconds coarse. The point is which
        // clock it's on: half a second, not the second and a half into the
        // image that spot really is.
        let landed = src.seek(0.5).expect("the wav seeks");
        assert!(
            (landed - 0.5).abs() < 0.05,
            "landed at {landed}, which is not half a second into the track"
        );
        assert_eq!(src.pos_frames, (landed * 48_000.0).round() as u64);
        assert!(src.pos_frames > 0 && src.pos_frames < 96_000);

        // Well past the span's end: it clamps into this track rather than
        // scrubbing on into the next one, which is the same file a few
        // seconds along. Short of the end by the seek margin, so it lands on
        // a spot with music left rather than the track's own finish line.
        let landed = src.seek(30.0).expect("the wav seeks");
        assert!(landed <= 2.0, "landed at {landed}, past the span's end");
        assert!(
            2.0 - landed < SEEK_END_MARGIN_SECS + 0.05,
            "landed at {landed}, well short of the span's end"
        );
        let left = src.remaining().expect("the span states its length");
        // From there the track has exactly its own tail left: the end is a
        // spot in the file, so a coarse landing shortens the decode instead
        // of moving the boundary.
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

    /// The last track of an image has no end in the sheet, so it takes the
    /// file's: everything from its start to natural EOF, and the length it
    /// reports is the file's minus that start.
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

    /// The pool holds the span the same way it holds the group and the
    /// gain, so the source the engine opens for an entry is that entry's
    /// slice. One image, two entries, two different tracks.
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
        // Each entry publishes its own length, which the transport and the
        // fade window read off.
        let tracks = e.shared.tracks.lock().unwrap();
        assert_eq!(tracks[0].as_ref().and_then(|t| t.duration_secs), Some(1.0));
        assert_eq!(tracks[1].as_ref().and_then(|t| t.duration_secs), Some(3.0));
    }

    #[test]
    fn remove_many_keeps_audible_even_if_named() {
        let mut e = test_engine(5);
        set_audible(&e, 1);
        e.pos = 1;
        // Name the audible entry in the drop set; it must be kept.
        let _ = e.remove_many(&[0, 1, 2]);
        assert!(
            e.order.iter().any(|entry| entry.id == 1),
            "audible entry kept"
        );
    }

    /// A decoder that falls over the way the real one did: an arithmetic
    /// overflow deep inside a third-party codec, on a file that reads fine
    /// everywhere else. Both entry points a source calls into go down, since
    /// a codec with state bad enough to panic on a packet has no reason to
    /// survive being reset either.
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

    /// The one this whole guard exists for: a panic inside the codec ends
    /// the track and leaves the thread standing. Before the guard it
    /// unwound out of the decode worker, and the panic printed below is the
    /// caught one, not a failure.
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

        // The second call is the other half of it: a poisoned source reports
        // the end again without going back into the state the unwind came
        // out of, which would panic a second time.
        assert!(!src.next_chunk(48_000, &mut out));
        assert!(out.is_empty());
    }

    /// A seek into a decoder that panics reads as a seek that failed, which
    /// every caller already handles, and poisons the source with it.
    #[test]
    fn a_panic_on_seek_reads_as_a_seek_that_failed() {
        let fx = Fixtures::new("seek-panic");
        let path = fx.wav("tone.wav", 2.0);
        let (mut src, _) = Source::open(&local(&path), 48_000, None).expect("the fixture opens");
        src.decoder = Box::new(PanickingDecoder);
        // The wav reader seeks without decoding, so what goes down here is
        // the decoder reset that follows the landing.
        assert_eq!(src.seek_file(1.0), None, "no landing to report");
        assert!(src.poisoned);
    }

    /// The guard itself: a panic comes back as an error naming the file and
    /// carrying the panic's own message, and a call that doesn't panic is
    /// untouched.
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

        // A `&'static str` payload reads the same way a formatted one does.
        let err = guard_decode("probe", path, || panic!("static message"))
            .expect_err("a panic is an error");
        assert!(err.contains("static message"), "{err}");

        assert_eq!(guard_decode("decode", path, || 7).ok(), Some(7));
    }
}
