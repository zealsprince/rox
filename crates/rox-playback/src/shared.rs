//! State shared between the decode thread, the RT output callback, and the
//! status display. The callback only ever touches the atomics; the mutex side
//! is decode-thread and UI-thread only.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

use rox_library::locator::Locator;

use crate::http::StationInfo;
use crate::icy::IcyTitle;

/// Where a stream stands, for the surfaces that have to say something while a
/// station is doing anything other than playing.
///
/// A file has no version of this: it opens in a millisecond and never drops,
/// so the states below would all be over before a frame rendered. A station
/// spends real seconds in `Opening`, and a broadcast that goes away mid-song
/// is routine rather than exceptional, so the difference between "gone" and
/// "coming back" is something the listener should be able to see rather than
/// guess at from a stalled clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    /// The open is in flight: the request is out and no audio exists yet.
    Opening,
    /// Connected and reading.
    Live,
    /// The connection went away and a reconnect is in progress.
    Reconnecting,
    /// The reconnects ran out. The entry is done, and the queue moves on.
    Dropped,
}

/// Where a transport reports its state. Same shape and same reason as
/// [`TitleSink`](crate::icy::TitleSink): the reconnect loop sits well below
/// anything that knows which queue entry it belongs to, so the entry is bound
/// into the closure at the open.
pub type StreamSink = Arc<dyn Fn(StreamState) + Send + Sync>;

/// A sink that drops everything, for an open with nobody watching: every
/// local file, and the off-thread analysis passes.
pub fn no_stream() -> StreamSink {
    Arc::new(|_| {})
}

/// Where a live stream is being played from, relative to the broadcast.
///
/// A station is taped as it arrives, so what comes out of the speakers is
/// a cursor into the last few minutes rather than whatever the socket
/// delivered a moment ago. `behind_secs` is the distance from that cursor
/// to the live edge, zero meaning live, and `window_secs` is how much tape
/// there is to move through. Both grow while a pause holds the cursor
/// still and the connection keeps taping.
///
/// Seconds rather than bytes because every surface reading this draws
/// time, and the byte arithmetic behind it belongs to the transport that
/// measured the rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shift {
    /// How far behind the live edge the cursor sits, in seconds.
    pub behind_secs: f64,
    /// How much of the broadcast is held, in seconds. Shorter than
    /// `cap_secs` until a station has been on long enough to fill the
    /// buffer, and equal to it from then on.
    pub window_secs: f64,
    /// How much the buffer is set to hold, in seconds: the setting's value
    /// as this session is running it. What a strip drawing the buffer spans,
    /// with the part past `window_secs` still to arrive.
    pub cap_secs: f64,
    /// How many bytes a second of this broadcast comes to, as the tape
    /// works it out: what playback has measured where it has enough to go
    /// on, what `icy-br` claimed otherwise, the socket's own average
    /// failing that. The one number that turns a length of buffer into a
    /// weight of memory, which is what the setting is really spending.
    pub bytes_per_sec: f64,
    /// How far into the song under the cursor, in seconds, off the in-band
    /// titles the station announced and their place in the tape. None until
    /// a title has been announced behind the cursor.
    ///
    /// This is what a song clock reads on a station, and the reason it
    /// survives a step backwards: the tape knows where the song the cursor
    /// landed in began, so landing in the middle of one shows the middle of
    /// it rather than a fresh zero.
    pub song_secs: Option<f64>,
    /// How long that song ran, mark to mark. None for the newest song,
    /// which hasn't ended, and for one whose start has been trimmed off the
    /// back of the tape, where what's left isn't the whole of it.
    pub song_len_secs: Option<f64>,
}

/// A song boundary in the buffer, as the strip over it draws one.
///
/// A broadcast announces its songs in band and nowhere else, so the points
/// where one gave way to the next are the only structure a station's
/// timeline has. The tape keeps them as byte offsets; this is the same
/// thing in the units a strip works in, measured from the live edge back
/// the way [`Shift::behind_secs`] is, so a mark and the playhead sit on one
/// axis without the reader converting anything.
///
/// One per mark still inside the buffer. The song under the oldest byte
/// held has no mark here: its start is off the back, so there's no point on
/// the strip to draw it at.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveMark {
    /// How far behind the live edge the song began, in seconds.
    pub behind_secs: f64,
    pub artist: String,
    pub title: String,
}

/// A break in the buffer, as the strips over it draw one.
///
/// A reconnect splices two connections' bytes together and nothing decodes
/// across the join, so a seek back over one stops at it. That wall is
/// invisible until a click hits it, which is what these are for.
///
/// Two numbers rather than the one a [`LiveMark`] carries, because the two
/// strips measure on different axes and a break has to sit in the right
/// place on both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveGap {
    /// How far behind the live edge the break sits, in seconds, measured
    /// the way [`LiveMark::behind_secs`] is. What a strip spanning the
    /// tape places it by.
    pub behind_secs: f64,
    /// How long ago the listener crossed it, in seconds of audio actually
    /// heard. Negative for a break the cursor hasn't reached, which is
    /// every one of them for somebody who has stepped back into the tape.
    ///
    /// The distance above can't stand in for this. It's measured against
    /// the live edge, and a listener standing on that edge reports a
    /// distance of exactly zero while really sitting a second or two back,
    /// since the edge arrives in socket-sized chunks and inside one there's
    /// nothing to know. That rounding is invisible on a strip spanning ten
    /// minutes of tape, and it's a fifth of the width of a rolling trace
    /// spanning eight seconds of what has been heard.
    pub heard_ago_secs: f64,
}

/// The pool index slot saying no live entry is publishing a shift.
const NO_SHIFT: u64 = u64::MAX;

/// An optional second as the bits of an f64, with NaN standing for nothing.
/// Two of [`Shift`]'s four numbers are absent for whole stretches of a
/// stream, and a sentinel inside the number keeps the pair of atomics from
/// becoming a pair plus a pair of flags.
fn secs_bits(secs: Option<f64>) -> u64 {
    secs.unwrap_or(f64::NAN).to_bits()
}

/// The same read back. Anything that isn't a real number reads as nothing,
/// which covers the sentinel and any infinity a divide by a silly rate
/// could have produced.
fn secs_of(bits: u64) -> Option<f64> {
    let secs = f64::from_bits(bits);

    secs.is_finite().then_some(secs)
}

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

/// One entry in the play queue as the UI sees it: a stable id that stays the
/// same across reorders and removals, where its bytes come from, and whether
/// it was queued explicitly (Play Next, Add to Queue) or came from the playing
/// context (the album or library view). The queue widgets show only the
/// explicit ones; the context plays on in the background. The id is the handle
/// the UI passes back to remove or move an entry, so an index shift between a
/// read and the edit can't act on the wrong track.
#[derive(Clone)]
pub struct QueueEntry {
    pub id: u64,
    pub locator: Locator,
    pub explicit: bool,
    /// The pool index this entry points at, distinct per entry even when two
    /// entries share a path. The UI matches the audible track on this rather
    /// than the path, so a file that appears in the order more than once
    /// resolves to the right occurrence instead of the first one by path.
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
    ///
    /// It means the same thing on a live station as on a file now. The
    /// connection stays up through a pause and the broadcast keeps being
    /// taped ([`crate::tape`]), so the flag coming back plays on from the
    /// byte it stopped at rather than from wherever the station has got to.
    /// Only a pause left running for half an hour gives the socket up, and
    /// everything published here survives that too: the entry keeps its
    /// `TrackInfo` and its title, so nothing reading a paused station sees a
    /// queue that lost a track.
    pub playing: AtomicBool,
    /// Device frames left in an audition blip, zero when nothing is
    /// auditioning. A step taken while paused arms this: the callback plays
    /// that many frames through the pause without `playing` ever moving, so
    /// nothing reading the pause flag reacts and a step landing mid-blip
    /// can't strand the transport running. Counted here rather than timed
    /// from the UI thread, where a short blip would be mostly buffer
    /// latency.
    pub audition_left: AtomicU64,
    /// The flush epoch, bumped by the decode thread on a seek or a skip.
    /// A backend that sees a number it hasn't handled discards the whole
    /// ring exactly once and echoes it back in `flush_ack`. An epoch rather
    /// than a flag because a flag has to be cleared, and the clearing races
    /// the callback that's already inside it: whoever loses eats the first
    /// milliseconds of the new track. Handled-once is the same discard with
    /// no window to lose.
    pub flush_seq: AtomicU64,
    /// The newest epoch a backend has finished discarding. The decode
    /// thread waits for this to catch up before it resyncs its clock,
    /// replacing what used to be a fixed grace sleep.
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
    /// The A-B loop's two marks, track-relative seconds as `f64::to_bits`,
    /// with `u64::MAX` in `ab_a` meaning nothing is looping. Two atomics
    /// rather than a lock: the transport asks on every frame and the decode
    /// thread writes only when the loop is set, moved, or cleared. Written
    /// b first and read a first, so a reader never sees a new A against an
    /// old B.
    pub ab_a: AtomicU64,
    pub ab_b: AtomicU64,
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
    /// A command is waiting that the listener expects an answer to now:
    /// pause, a skip, a jump, a play now, a clear, quit. Set by the player
    /// as it sends one, cleared by the engine as it drains its channel.
    ///
    /// It exists for a station that has gone off the air, which is the one
    /// thing here that waits for seconds at a time. Two readers act on it.
    /// The feed thread gives up its reconnect schedule rather than serving
    /// it in full, and the decode thread stops waiting at the live edge for
    /// a tape nothing is filling, so the entry ends and the queue moves on.
    /// Neither reads it while the station is healthy: a pause is meant to
    /// hold a connection now, not end one.
    ///
    /// An `Arc` because it's handed down into the transport, which lives
    /// under symphonia and has no way back to this struct. One per session
    /// rather than one per entry: it means "a command is waiting", and only
    /// one station is ever in trouble at a time.
    pub interrupt: Arc<AtomicBool>,
    /// Position mapping, appended by the decode thread.
    pub segments: Mutex<Vec<Segment>>,
    /// Display info per queue entry, filled in as tracks open.
    pub tracks: Mutex<Vec<Option<TrackInfo>>>,
    /// The station title per queue entry, for the entries that have one:
    /// web radio sends its now-playing in band and nowhere else, so this is
    /// the only thing that ever says what a stream is playing.
    ///
    /// Parallel to `tracks` rather than a field inside it because the two
    /// are written on different clocks. `tracks[i]` is rewritten whole every
    /// time the entry opens, and a title arrives whenever the station feels
    /// like it, including during the probe, before the `TrackInfo` that
    /// would have carried it exists. Keeping them apart means an open can't
    /// wipe a title and a title can't race an open.
    pub titles: Mutex<Vec<Option<IcyTitle>>>,
    /// What each station said about itself when its entry opened: its own
    /// name, genre, bitrate, homepage and description off the response
    /// headers. Parallel to `titles` for the same reason, and written once
    /// per open rather than whenever the station feels like it, since the
    /// headers go past exactly once on the way into the body.
    pub station: Mutex<Vec<Option<StationInfo>>>,
    /// Where each stream stands, for the entries that are one. None means a
    /// local file, which has no state worth showing: it opens instantly and
    /// the only thing that can go wrong with it is not opening at all.
    ///
    /// Parallel to `station` and written from both sides of the open: the
    /// engine marks the open starting and finishing, the transport marks the
    /// drops and reconnects it answers on its own. Two writers on one slot is
    /// fine because they never overlap; the transport doesn't exist until the
    /// open that publishes `Live` has returned it.
    pub stream: Mutex<Vec<Option<StreamState>>>,
    /// The play queue for the UI, rewritten by the decode thread when its
    /// entries change: a new session, an insert, a remove, a move, a
    /// reshuffle. Not on a plain track advance; the playing entry is resolved
    /// off the position clock, so the queue view only needs republishing when
    /// its contents change.
    pub queue: Mutex<QueueSnapshot>,
    /// Bumped on every queue rewrite, so the UI can skip cloning the snapshot
    /// on the ticks where nothing changed.
    pub queue_rev: AtomicU64,
    /// Bumped on every title that actually changed, the same deal `queue_rev`
    /// offers: poll the atomic on the pump's clock and only take the lock on
    /// the ticks where a station moved to the next song.
    pub title_rev: AtomicU64,
    /// The timeshift the audible live entry is playing at: the pool index
    /// it belongs to, then the two seconds of [`Shift`] as `f64::to_bits`.
    /// `u64::MAX` in the index means nothing live is publishing one.
    ///
    /// Atomics rather than a slot beside the titles, and deliberately off
    /// the title revision. Both numbers move on every decoded chunk and
    /// again on every tick of a pause, and the revision means "this entry
    /// moved to another song" to everything that polls it: bumping it sixty
    /// times a second would have the station panel re-read the library and
    /// the scrobbler re-examine its boundary for a cursor that slid a
    /// millisecond. Written by the decode thread, read by the pump.
    pub shift_idx: AtomicU64,
    pub shift_behind: AtomicU64,
    pub shift_window: AtomicU64,
    pub shift_cap: AtomicU64,
    pub shift_rate: AtomicU64,
    pub shift_song: AtomicU64,
    pub shift_song_len: AtomicU64,
    /// Where the audible station's songs turned over, oldest first, for the
    /// surface drawing the buffer. Belongs to whichever entry `shift_idx`
    /// names, the same as the shift itself.
    ///
    /// A lock rather than atomics because it's a list of strings, and on the
    /// pump's side of the fence: the decode thread rewrites it as the
    /// distances move, and the revision below only moves when the set does,
    /// so a surface that only cares about song changes can poll the atomic
    /// and leave the lock alone.
    pub live_marks: Mutex<Vec<LiveMark>>,
    /// Where the audible station's connection broke and picked up again,
    /// oldest first. Belongs to the entry `shift_idx` names, like
    /// everything else here.
    ///
    /// Beside the marks rather than inside them, because a listener reads
    /// the two for opposite reasons: a song boundary is a point worth
    /// jumping to, and this is a point nothing can play across. They share
    /// a publish and a revision because they move on one clock, which is
    /// the tape rolling under both.
    pub live_gaps: Mutex<Vec<LiveGap>>,
    /// Why the last entry the queue gave up on couldn't be opened, until
    /// somebody reads it. None once it's been read, and None on a session
    /// that has never lost an entry.
    ///
    /// A skip is the one failure with nothing left behind to look at: the
    /// entry stops being the playing one, the next entry opens over the top
    /// of it, and the only record is a log line nobody has open. So the
    /// reason waits here for the pump to pick it up on its next tick.
    ///
    /// Not parallel to the queue like `stream` is, because it isn't a
    /// property of an entry. It's the last thing that went wrong, and the
    /// surface showing it has room for one line.
    pub refusal: Mutex<Option<String>>,
    /// Whether `refusal` holds something unread. The pump ticks sixty times
    /// a second and a refusal is rare, so the common tick is one relaxed
    /// load and no lock at all.
    pub refusal_pending: AtomicBool,
}

impl Shared {
    pub fn new(queue_len: usize) -> Self {
        Shared {
            playing: AtomicBool::new(true),
            audition_left: AtomicU64::new(0),
            flush_seq: AtomicU64::new(0),
            flush_ack: AtomicU64::new(0),
            fade_at: AtomicU64::new(0),
            fade_len: AtomicU64::new(0),
            fade_back: AtomicBool::new(false),
            ab_a: AtomicU64::new(u64::MAX),
            ab_b: AtomicU64::new(0),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
            frames_consumed: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            device_lost: AtomicBool::new(false),
            interrupt: Arc::new(AtomicBool::new(false)),
            segments: Mutex::new(Vec::new()),
            tracks: Mutex::new(vec![None; queue_len]),
            titles: Mutex::new(vec![None; queue_len]),
            station: Mutex::new(vec![None; queue_len]),
            stream: Mutex::new(vec![None; queue_len]),
            queue: Mutex::new(QueueSnapshot::default()),
            queue_rev: AtomicU64::new(0),
            title_rev: AtomicU64::new(0),
            shift_idx: AtomicU64::new(NO_SHIFT),
            shift_behind: AtomicU64::new(0),
            shift_window: AtomicU64::new(0),
            shift_cap: AtomicU64::new(0),
            shift_rate: AtomicU64::new(0),
            shift_song: AtomicU64::new(secs_bits(None)),
            shift_song_len: AtomicU64::new(secs_bits(None)),
            live_marks: Mutex::new(Vec::new()),
            live_gaps: Mutex::new(Vec::new()),
            refusal: Mutex::new(None),
            refusal_pending: AtomicBool::new(false),
        }
    }

    /// Record why an entry couldn't be opened, which the decode thread does
    /// as it falls past one. The newest reason wins: an unread one is about
    /// a track the queue has already moved on from, and two lines don't fit
    /// where one goes.
    pub fn publish_refusal(&self, reason: String) {
        *self.refusal.lock().unwrap() = Some(reason);
        self.refusal_pending
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// The reason, once. Reading it clears it, so a surface that showed it
    /// keeps showing it on its own terms rather than being told again every
    /// tick.
    pub fn take_refusal(&self) -> Option<String> {
        if !self
            .refusal_pending
            .swap(false, std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }

        self.refusal.lock().unwrap().take()
    }

    /// Record the station title for pool entry `idx` and bump the revision,
    /// which is what the decode thread does from inside the ICY reader.
    ///
    /// A repeat is dropped rather than republished. Stations resend the
    /// current title in every metadata block, several times a minute, and
    /// the revision has to mean "the song changed" for anything downstream
    /// to hang a scrobble off it.
    pub fn publish_title(&self, idx: usize, title: IcyTitle) {
        let mut titles = self.titles.lock().unwrap();
        let Some(slot) = titles.get_mut(idx) else {
            return;
        };
        if slot.as_ref() == Some(&title) {
            return;
        }

        *slot = Some(title);
        drop(titles);

        self.title_rev
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    /// The station title published for pool entry `idx`, None for anything
    /// that isn't a stream or hasn't sent one yet.
    pub fn live_title(&self, idx: usize) -> Option<IcyTitle> {
        self.titles.lock().unwrap().get(idx).cloned().flatten()
    }

    /// Record what the station at pool entry `idx` said about itself, which
    /// the decode thread does once per open.
    ///
    /// The bump rides the title revision rather than one of its own. Both
    /// mean the same thing to every reader downstream, "what this entry is
    /// playing has changed under you", and a second atomic to poll would
    /// buy a distinction nobody acts on.
    pub fn publish_station(&self, idx: usize, info: StationInfo) {
        let mut station = self.station.lock().unwrap();
        let Some(slot) = station.get_mut(idx) else {
            return;
        };
        if slot.as_ref() == Some(&info) {
            return;
        }

        *slot = Some(info);
        drop(station);

        self.title_rev
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    /// What the station at pool entry `idx` said about itself, None for
    /// anything that isn't a station or hasn't been opened yet.
    pub fn station_info(&self, idx: usize) -> Option<StationInfo> {
        self.station.lock().unwrap().get(idx).cloned().flatten()
    }

    /// Record where the stream at pool entry `idx` stands, which both the
    /// engine's open and the transport's reconnect loop do.
    ///
    /// On the title revision for the same reason the description is: a state
    /// change means "what this entry is doing has changed under you", which
    /// is the one question every reader here polls. One clock, and a station
    /// that reconnects without changing song still wakes the surfaces that
    /// have to stop drawing it as playing.
    pub fn publish_stream(&self, idx: usize, state: StreamState) {
        let mut stream = self.stream.lock().unwrap();
        let Some(slot) = stream.get_mut(idx) else {
            return;
        };
        if *slot == Some(state) {
            return;
        }

        *slot = Some(state);
        drop(stream);

        self.title_rev
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    /// Where the stream at pool entry `idx` stands, None for a local file and
    /// for an entry nothing has opened yet.
    pub fn stream_state(&self, idx: usize) -> Option<StreamState> {
        self.stream.lock().unwrap().get(idx).copied().flatten()
    }

    /// The title revision, bumped on every real change. Cheap to poll each
    /// tick, the same way [`queue_rev`](Self::queue_rev) is.
    pub fn title_rev(&self) -> u64 {
        self.title_rev.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Say where pool entry `idx` is playing from relative to its live
    /// edge, which the decode thread does as it moves through the tape.
    ///
    /// The index is stored last and read first, so a reader that finds the
    /// entry it cares about is looking at numbers that were written for it
    /// rather than at one entry's window against another's distance.
    pub fn publish_shift(&self, idx: usize, shift: Shift) {
        use std::sync::atomic::Ordering;

        self.shift_behind
            .store(shift.behind_secs.to_bits(), Ordering::Relaxed);
        self.shift_window
            .store(shift.window_secs.to_bits(), Ordering::Relaxed);
        self.shift_cap
            .store(shift.cap_secs.to_bits(), Ordering::Relaxed);
        self.shift_rate
            .store(shift.bytes_per_sec.to_bits(), Ordering::Relaxed);
        self.shift_song
            .store(secs_bits(shift.song_secs), Ordering::Relaxed);
        self.shift_song_len
            .store(secs_bits(shift.song_len_secs), Ordering::Relaxed);
        self.shift_idx.store(idx as u64, Ordering::Release);
    }

    /// Withdraw the shift, for a station that stopped being the thing
    /// playing: a skip onto a file, a hang-up, the end of the queue.
    pub fn clear_shift(&self) {
        self.shift_idx
            .store(NO_SHIFT, std::sync::atomic::Ordering::Release);

        // The song boundaries and the breaks went with it. Emptying a list
        // that was already empty is the ordinary case, on every pass of a
        // session playing a file, so the revision only moves when there
        // was something there to take away.
        let marks = std::mem::take(&mut *self.live_marks.lock().unwrap());
        let gaps = std::mem::take(&mut *self.live_gaps.lock().unwrap());
        if !marks.is_empty() || !gaps.is_empty() {
            self.title_rev
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }

    /// Say where the audible station's songs turned over and where its
    /// connection broke, which are the two things a strip over the tape
    /// has to draw.
    ///
    /// `changed` is the sets themselves having changed, a song announced,
    /// a reconnect, or either dropping off the back of the buffer, as
    /// against the distances moving because the live edge did. Only the
    /// first is worth a revision: the second happens every time a chunk
    /// lands and means nothing to anyone who isn't already redrawing.
    pub fn publish_live_marks(&self, marks: Vec<LiveMark>, gaps: Vec<LiveGap>, changed: bool) {
        *self.live_marks.lock().unwrap() = marks;
        *self.live_gaps.lock().unwrap() = gaps;

        if changed {
            self.title_rev
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }

    /// The audible station's song boundaries, oldest first, cloned for
    /// whoever is drawing them.
    pub fn live_marks(&self) -> Vec<LiveMark> {
        self.live_marks.lock().unwrap().clone()
    }

    /// The audible station's reconnects, oldest first, cloned for whoever
    /// is drawing them.
    pub fn live_gaps(&self) -> Vec<LiveGap> {
        self.live_gaps.lock().unwrap().clone()
    }

    /// Where pool entry `idx` is playing from, None when the shift on
    /// offer belongs to another entry or nothing is taping.
    pub fn shift(&self, idx: usize) -> Option<Shift> {
        use std::sync::atomic::Ordering;

        if self.shift_idx.load(Ordering::Acquire) != idx as u64 {
            return None;
        }

        Some(Shift {
            behind_secs: f64::from_bits(self.shift_behind.load(Ordering::Relaxed)),
            window_secs: f64::from_bits(self.shift_window.load(Ordering::Relaxed)),
            cap_secs: f64::from_bits(self.shift_cap.load(Ordering::Relaxed)),
            bytes_per_sec: f64::from_bits(self.shift_rate.load(Ordering::Relaxed)),
            song_secs: secs_of(self.shift_song.load(Ordering::Relaxed)),
            song_len_secs: secs_of(self.shift_song_len.load(Ordering::Relaxed)),
        })
    }

    /// The current play queue, cloned for the UI.
    pub fn queue_snapshot(&self) -> QueueSnapshot {
        self.queue.lock().unwrap().clone()
    }

    /// The queue's revision, bumped on every rewrite. Cheap to poll each tick.
    pub fn queue_rev(&self) -> u64 {
        self.queue_rev.load(std::sync::atomic::Ordering::Acquire)
    }

    /// How many entries come after the one holding pool index `audible`, and
    /// that entry's own position. Falls back to the published cursor when
    /// nothing is audible yet or the index isn't in the order, which is the
    /// case for a freshly started context.
    ///
    /// Its own read rather than a [`queue_snapshot`](Self::queue_snapshot)
    /// the caller measures, because the continuation trigger (ADR 17) calls
    /// this on the pump's 16 ms clock and the snapshot clones a `Locator` per
    /// entry. Counting under the lock costs a scan of the order and no
    /// allocation at all. None while nothing is queued.
    pub fn upcoming_from(&self, audible: Option<usize>) -> Option<(usize, usize)> {
        let queue = self.queue.lock().unwrap();
        if queue.entries.is_empty() {
            return None;
        }
        let at = audible
            .and_then(|idx| queue.entries.iter().position(|e| e.idx == idx))
            .unwrap_or(queue.cursor);
        Some((at, queue.entries.len().saturating_sub(at + 1)))
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Say that a command worth answering now is on its way, which is what
    /// the player does as it sends one.
    ///
    /// Only a stalled reconnect reads it, and only to stop waiting. Sending
    /// a command nothing is blocked on costs a relaxed store and changes
    /// nothing, so the caller doesn't have to know whether a station is in
    /// trouble to decide whether to say this.
    pub fn interrupt(&self) {
        self.interrupt
            .store(true, std::sync::atomic::Ordering::Relaxed);
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
        // Length first and with acquire ordering: it publishes the pair, so
        // a nonzero read here means the frame it starts at is already in
        // place.
        //
        // The pair isn't atomic together, so a re-publish between these two
        // loads can hand back the old length beside the new frame. One UI
        // tick of a wrong progress number, on a bar that
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

    /// The section on repeat, both marks in track-relative seconds. None
    /// when nothing is looping, which is nearly always. The engine owns the
    /// loop and this is the only copy anyone else reads (ADR 16).
    pub fn ab(&self) -> Option<(f64, f64)> {
        let a = self.ab_a.load(std::sync::atomic::Ordering::Acquire);
        if a == u64::MAX {
            return None;
        }
        let b = self.ab_b.load(std::sync::atomic::Ordering::Relaxed);
        Some((f64::from_bits(a), f64::from_bits(b)))
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
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    /// The refusal is a one-shot handoff: the decode thread leaves it, the
    /// pump takes it, and a second look finds nothing. A tick that finds
    /// nothing never touches the lock.
    #[test]
    fn a_refusal_is_read_once() {
        let shared = Shared::new(2);
        assert_eq!(shared.take_refusal(), None);

        shared.publish_refusal("the server refused the stream: nope".into());
        assert_eq!(
            shared.take_refusal(),
            Some("the server refused the stream: nope".to_string())
        );
        assert_eq!(shared.take_refusal(), None);
    }

    /// Two entries refused before anybody looked. The one that survives is
    /// the newer one, since the older is about a track the queue has
    /// already fallen past.
    #[test]
    fn the_newest_refusal_wins() {
        let shared = Shared::new(2);
        shared.publish_refusal("first".into());
        shared.publish_refusal("second".into());

        assert_eq!(shared.take_refusal(), Some("second".to_string()));
    }

    fn push_segment(shared: &Shared, at_frame: u64, track: usize, track_frame: u64) {
        shared.segments.lock().unwrap().push(Segment {
            at_frame,
            track,
            track_frame,
        });
    }

    /// The continuation trigger's read: how much music is left ahead of the
    /// track coming out of the speakers. Matched on the pool index, so the
    /// same file appearing in the order twice still resolves to the
    /// occurrence playing now rather than the first one by path.
    #[test]
    fn upcoming_counts_from_the_audible_entry() {
        let shared = Shared::new(5);
        *shared.queue.lock().unwrap() = QueueSnapshot {
            entries: (0..5)
                .map(|i| QueueEntry {
                    id: i as u64,
                    // One file queued twice, at the front and in the middle.
                    locator: Locator::Local(PathBuf::from(if i == 3 {
                        "t0".to_string()
                    } else {
                        format!("t{i}")
                    })),
                    explicit: false,
                    idx: i,
                    group: None,
                })
                .collect(),
            cursor: 1,
        };
        assert_eq!(shared.upcoming_from(Some(0)), Some((0, 4)));
        assert_eq!(shared.upcoming_from(Some(3)), Some((3, 1)));
        // The last entry has nothing ahead of it, which is the case for a
        // queue that played out to its end.
        assert_eq!(shared.upcoming_from(Some(4)), Some((4, 0)));
        // Nothing audible yet falls back to the published cursor, and so
        // does an index the order doesn't hold.
        assert_eq!(shared.upcoming_from(None), Some((1, 3)));
        assert_eq!(shared.upcoming_from(Some(99)), Some((1, 3)));
        // An empty queue returns None rather than a count of zero, which
        // would read as dry and fire the trigger on a dead session.
        assert!(Shared::new(0).upcoming_from(None).is_none());
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
