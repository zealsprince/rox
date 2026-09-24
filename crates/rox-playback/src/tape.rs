//! The last few minutes of a live stream, kept in memory so a broadcast can
//! be paused, resumed, and stepped back through.
//!
//! The feed thread keeps reading the socket whether or not anything decodes
//! and writes the station's raw container bytes here; the decoder reads at
//! its own cursor. Pause stops the cursor, not the connection, and the
//! cursor's distance from the live edge is how far back the listener is.
//!
//! The network thread only appends and the decode thread only reads, under
//! one mutex with a condvar: the decode thread waiting at the live edge for
//! bytes is the point, so lock-free wouldn't help.
//!
//! MP3 and ADTS resync on a frame header wherever a seek lands. A reconnect
//! records a gap, since two connections' bytes don't decode as one stream,
//! and seeks never cross one.
//!
//! The cap is a length of time, what the listener chose. Bytes per second is
//! measured from the decoder's progress, else `icy-br`, else the wall clock.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use crate::icy::IcyTitle;
use crate::icy::TitleSink;
use crate::shared::LiveGap;
use crate::shared::LiveMark;
use crate::shared::Shift;

/// The rate assumed before anything is known, and the sizing floor. 128 kbps,
/// what most stations run.
const RATE_FLOOR: f64 = 16.0 * 1024.0;

/// Sizing ceiling: a megabyte a second is past any stream left running.
const RATE_CEILING: f64 = 1024.0 * 1024.0;

/// Decoded audio needed before the measurement beats the station's claim; the
/// reader runs ahead of the decoder, so the ratio starts high.
const RATE_MIN_SECS: f64 = 10.0;

const RATE_MIN_WIRE: Duration = Duration::from_secs(5);

/// How far off the settled rate a measurement must be to disagree.
const RATE_DRIFT: f64 = 0.2;

/// How long it must keep disagreeing before the rate re-settles.
const RATE_DRIFT_SECS: Duration = Duration::from_secs(30);

/// Overrun allowed before a trim. A sixteenth keeps trims to about one per
/// half minute and the published window steady, where exact trims would
/// memmove per read and drop-half would sawtooth.
const TRIM_SLACK: usize = 16;

/// Inside this the published distance is exactly zero: the edge arrives in
/// socket-sized chunks, so a cursor keeping up is always inside the last one.
pub const LIVE_EDGE_SNAP_SECS: f64 = 2.0;

/// The narrowest the band gets. The band is the resolution of anything
/// measured against the head.
const BAND_MIN_SECS: f64 = 0.75;

/// Wide enough that a late chunk doesn't stall the drawn edge against the head.
const BAND_CHUNKS: f64 = 1.5;

/// Cap on a chunk's length in seconds (the slowest music stream is ~4 s per
/// read), so one oversized append can't hold the drawn edge back.
const CHUNK_MAX_SECS: f64 = 4.0;

/// How long the drawn edge takes to ease back into its span after a connect
/// or burst. Inside the span it isn't pulled at all.
const EDGE_SETTLE_SECS: f64 = 2.0;

/// The span behind the head the drawn edge is left alone in, in bands. Wider
/// than a chunk's saw, with room for a late chunk.
const EDGE_NEAR_BANDS: f64 = 0.5;
const EDGE_FAR_BANDS: f64 = 2.0;

/// Past this the edge jumps to the head instead of easing: easing across a
/// connect burst would leave the bar short for most of a minute.
const EDGE_SNAP_SECS: f64 = 4.0;

/// How far the cursor may slip from its closest approach before a live
/// session counts as behind. Catches a pause, where nobody seeked.
const LIVE_SLIP_BANDS: f64 = 1.0;

/// Bounds how stale a parked read's view of a dropped connection can get; the
/// condvar wakes it on bytes.
const EDGE_STEP: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    Live,
    Reconnecting,
    /// Nothing more will ever be appended.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Snap {
    /// MP3 and ADTS carry a sync word on every frame.
    Anywhere,
    /// Ogg only reads at a page, so a seek scans forward to `OggS`.
    OggPage,
}

const OGGS: &[u8; 4] = b"OggS";

/// Give up past this: a page is a few kilobytes, so beyond it the bytes
/// aren't Ogg.
const OGG_SCAN: usize = 1 << 20;

pub struct Tape {
    inner: Mutex<Inner>,
    /// Woken on append, state change, and give-up.
    wake: Condvar,
    /// The setting's length. Atomic because the setting can move under a
    /// playing station.
    cap_secs: AtomicU32,
    snap: Snap,
    /// Fired as the cursor reaches each title's byte. Held so a reopen over this
    /// tape inherits it.
    on_title: TitleSink,
    /// Read at the live edge so a quiet station can't leave a pause unanswered.
    interrupt: Arc<AtomicBool>,
}

struct Inner {
    buf: Vec<u8>,
    /// [`crate::memory::live_buffer_cap`], read once at the open.
    cap_bytes: usize,
    /// Stream offset of `buf[0]`. Every offset here is absolute, so a cursor
    /// survives its bytes being dropped.
    start: u64,
    /// Published for the engine's timeshift readout and seeks.
    cursor: u64,
    /// Reconnect joins. A seek lands on the live side of any it would cross, and
    /// they're published so the listener sees the wall.
    gaps: VecDeque<u64>,
    /// Kept for the whole window, so a seek back resolves the title that was on then.
    marks: VecDeque<Mark>,
    /// The last mark published, so an unchanged answer skips the sink.
    published: Option<u64>,
    /// The next title names music already playing. True at open and after a reconnect.
    joined: bool,
    /// Live is a state, not a distance: the connect burst leaves an unmoved cursor
    /// seconds behind, and that still reads as live.
    timeshifted: bool,
    /// The closest approach while live; slipping from it is how a pause stops
    /// being live.
    live_lead: f64,
    /// Bumped when the set of drawn marks or gaps changes, not when distances
    /// slide.
    marks_rev: u64,
    state: Feed,
    rate: Rate,
    /// Arrival is bursty, so this is the resolution of anything measured against
    /// the head, and sizes the band.
    last_chunk: usize,
    /// The live edge things are drawn against, in bytes. Never the head itself,
    /// which jumps a chunk at a time: this runs at a second a second, eased to
    /// sit a band or so behind it. See [`Inner::edge`].
    edge: Option<f64>,
    edge_since: Instant,
    /// Which side of its span the edge strayed to while being pulled back.
    edge_pull: Option<f64>,
    /// Where the listener is: the reader's starting offset plus seconds decoded
    /// since. Not the reader's cursor, which moves in 32 KiB jumps. See
    /// [`Inner::heard`].
    heard_from: u64,
    heard_secs: f64,
    /// The pair before the last reader was built, for a failed reopen.
    heard_was: (u64, f64),
    /// Last seen by the shift, to tell whether the listener is moving.
    heard_seen: f64,
    /// Decoded audio still in the output ring, taken off so the playhead follows
    /// the speakers, not the decoder's steps.
    queued_secs: f64,
    /// The reader fell off the back and snapped to the oldest byte. The engine
    /// answers with a re-sync, since the decoder is mid-frame on bytes that no
    /// longer follow.
    underran: bool,
}

struct Mark {
    at: u64,
    title: IcyTitle,
    /// Names what was on at connect or reconnect: not a boundary, so never drawn.
    joined: bool,
}

/// How bytes become seconds, settled once and held. Every published number
/// divides by this, so a moving rate would rescale the whole strip under
/// the listener. Radio is constant bitrate.
struct Rate {
    /// None while the first ten seconds are measured.
    held: Option<f64>,
    /// From `icy-br` and never replaced; measurements only check it.
    stated: bool,
    /// The provisional answer before anything has decoded.
    wire_bytes: u64,
    wire_since: Instant,
    /// The exact answer for the played part of the tape.
    played_bytes: u64,
    played_secs: f64,
    /// When the measurement started disagreeing by more than [`RATE_DRIFT`].
    drifting_since: Option<Instant>,
}

impl Rate {
    fn new(stated_kbps: u32) -> Rate {
        let stated = (stated_kbps > 0).then(|| stated_kbps as f64 * 1000.0 / 8.0);

        Rate {
            held: stated,
            stated: stated.is_some(),
            wire_bytes: 0,
            wire_since: Instant::now(),
            played_bytes: 0,
            played_secs: 0.0,
            drifting_since: None,
        }
    }

    /// Settle the rate off the decoder's own account of `secs`. The window opens
    /// at the first call, not at the open, so symphonia's read-ahead cancels out
    /// instead of inflating the rate by half.
    fn played(&mut self, secs: f64) {
        if self.played_secs == 0.0 {
            self.played_bytes = 0;
        }

        self.played_secs += secs;
        if self.played_secs < RATE_MIN_SECS {
            return;
        }

        let measured = self.played_bytes as f64 / self.played_secs;
        self.played_bytes = 0;
        self.played_secs = 0.0;

        // The first ten seconds decide, unless the station already said.
        let Some(held) = self.held else {
            self.held = Some(measured);

            return;
        };
        if self.stated {
            return;
        }

        // A disagreement must hold for half a minute: one window off is jitter.
        if (measured - held).abs() <= held * RATE_DRIFT {
            self.drifting_since = None;

            return;
        }

        match self.drifting_since {
            Some(since) if since.elapsed() >= RATE_DRIFT_SECS => {
                log::info!("station rate re-settled at {measured:.0} bytes/s from {held:.0}");
                self.held = Some(measured);
                self.drifting_since = None;
            }

            Some(_) => {}

            None => self.drifting_since = Some(Instant::now()),
        }
    }

    /// Unclamped on purpose: a floor would misreport a 128 kbps station's
    /// timeshift. The clamp lives in [`Inner::cap`], where error costs memory.
    fn bytes_per_sec(&self) -> f64 {
        if let Some(held) = self.held {
            return held;
        }

        let elapsed = self.wire_since.elapsed();
        match elapsed >= RATE_MIN_WIRE {
            true => self.wire_bytes as f64 / elapsed.as_secs_f64(),
            false => RATE_FLOOR,
        }
    }
}

impl Inner {
    fn head(&self) -> u64 {
        self.start + self.buf.len() as u64
    }

    /// Bytes for `cap_secs`, never more than [`cap_bytes`](Inner::cap_bytes).
    /// The rate is clamped here and only here: a made-up header shouldn't size
    /// the window. Past the memory ceiling the window comes up short, which
    /// [`held_secs`](Inner::held_secs) publishes.
    fn cap(&self, cap_secs: u32) -> usize {
        let bps = self.rate.bytes_per_sec().clamp(RATE_FLOOR, RATE_CEILING);

        ((bps * cap_secs as f64) as usize).min(self.cap_bytes)
    }

    /// The window as it will actually be held. Published instead of the setting
    /// so a capped window doesn't read as filling forever.
    fn held_secs(&self, cap_secs: u32, bps: f64) -> f64 {
        let asked = cap_secs as f64;

        match bps > 0.0 {
            true => (self.cap(cap_secs) as f64 / bps).min(asked),
            false => asked,
        }
    }

    /// Vec doubling near the ceiling would hold twice the window, so within a
    /// sixteenth of the cap growth steps by a sixteenth, what the trim hands back.
    fn reserve(&mut self, cap: usize, incoming: usize) {
        if self.buf.capacity() >= self.buf.len() + incoming {
            return;
        }

        let step = (cap / TRIM_SLACK).min(self.buf.len());
        self.buf.reserve_exact(incoming + step);
    }

    fn trim(&mut self, cap_secs: u32) {
        self.trim_to(self.cap(cap_secs));
    }

    fn trim_to(&mut self, cap: usize) {
        if self.buf.len() <= cap + cap / TRIM_SLACK {
            return;
        }

        let inside = self.inside_marks();
        let spliced = self.gaps.len();
        let drop = self.buf.len() - cap;
        self.buf.drain(..drop);
        self.start += drop as u64;

        // A gap off the back has nothing behind it to seek into.
        while self.gaps.front().is_some_and(|g| *g <= self.start) {
            self.gaps.pop_front();
        }

        // Keep the mark before the oldest byte: it names what's playing at the back.
        while self.marks.len() > 1 && self.marks[1].at <= self.start {
            self.marks.pop_front();
        }

        // A mark or gap rolling off the back changes the drawn set.
        if self.inside_marks() != inside || self.gaps.len() != spliced {
            self.marks_rev += 1;
        }

        // Drain frees length, not memory. Shrink once half a window clear of the
        // cap, which a steady tape never reaches.
        let keep = cap + cap / TRIM_SLACK;
        if self.buf.capacity() > keep + cap / 2 {
            self.buf.shrink_to(keep);
        }
    }

    fn inside_marks(&self) -> usize {
        self.marks.iter().filter(|mark| self.drawn(mark)).count()
    }

    /// As wide as the chunks arrive, at least [`BAND_MIN_SECS`]. The drawn edge
    /// sits this far behind the head, and a live listener must slip this far to
    /// count as behind.
    fn band(&self, bps: f64) -> f64 {
        BAND_MIN_SECS.max(BAND_CHUNKS * self.chunk_secs(bps))
    }

    /// Move the drawn edge to `now`, in bytes: a second a second, pulled back
    /// when it strays too near or far from the head, never backwards, never past
    /// double speed, never past the head.
    fn edge(&mut self, now: Instant, bps: f64) -> f64 {
        let head = self.head() as f64;
        let Some(edge) = self.edge else {
            self.edge = Some(head);
            self.edge_since = now;

            return head;
        };

        let dt = now.saturating_duration_since(self.edge_since).as_secs_f64();
        self.edge_since = now;

        // No pull between half a band and two behind the head, a span wider than a
        // chunk's saw, so a steady broadcast runs the edge at exactly real time.
        // Once out, pull toward the middle, not the boundary, or the saw nudges the
        // pace every chunk.
        let band = self.band(bps);
        let gap = (head - edge) / bps;
        let mid = (EDGE_NEAR_BANDS + EDGE_FAR_BANDS) / 2.0 * band;
        let side = (gap - mid).signum();
        if self.edge_pull.is_some_and(|from| from != side) {
            self.edge_pull = None;
        }
        if gap < EDGE_NEAR_BANDS * band || gap > EDGE_FAR_BANDS * band {
            self.edge_pull = Some(side);
        }

        let pull = match self.edge_pull {
            Some(_) => ((gap - mid) / EDGE_SETTLE_SECS).clamp(-1.0, 1.0),
            None => 0.0,
        };
        let mut edge = (edge + dt * bps * (1.0 + pull)).min(head);

        // A burst: jump rather than ease.
        if head - edge > EDGE_SNAP_SECS.max(2.0 * EDGE_FAR_BANDS * band) * bps {
            edge = head;
        }

        self.edge = Some(edge);
        edge
    }

    /// Where the listener is, in bytes, held between the oldest byte and the cursor.
    fn heard(&self, bps: f64) -> f64 {
        let secs = (self.heard_secs - self.queued_secs).max(0.0);

        (self.heard_from as f64 + secs * bps)
            .min(self.cursor as f64)
            .max(self.start as f64)
    }

    /// Zero before one lands.
    fn chunk_secs(&self, bps: f64) -> f64 {
        (self.last_chunk as f64 / bps).min(CHUNK_MAX_SECS)
    }

    fn mark_index(&self, cursor: u64) -> Option<usize> {
        self.marks.iter().rposition(|mark| mark.at <= cursor)
    }

    /// Inside the buffer and a real song start, not the name of what was on at connect.
    fn drawn(&self, mark: &Mark) -> bool {
        !mark.joined && mark.at >= self.start && mark.at <= self.head()
    }

    fn title_at(&self, cursor: u64) -> Option<&Mark> {
        self.marks.get(self.mark_index(cursor)?)
    }

    /// The song clock: offsets of the song under the cursor and the next one.
    fn song_bounds(&self, cursor: u64) -> (Option<u64>, Option<u64>) {
        let Some(i) = self.mark_index(cursor) else {
            return (None, None);
        };

        (
            Some(self.marks[i].at),
            self.marks.get(i + 1).map(|next| next.at),
        )
    }
}

impl Tape {
    /// `stated_kbps` is zero when the station claimed nothing.
    pub fn new(
        cap_secs: u32,
        stated_kbps: u32,
        snap: Snap,
        on_title: TitleSink,
        interrupt: Arc<AtomicBool>,
    ) -> Tape {
        Tape {
            inner: Mutex::new(Inner {
                buf: Vec::new(),
                cap_bytes: crate::memory::live_buffer_cap(),
                start: 0,
                cursor: 0,
                gaps: VecDeque::new(),
                marks: VecDeque::new(),
                joined: true,
                timeshifted: false,
                live_lead: f64::INFINITY,
                published: None,
                marks_rev: 0,
                state: Feed::Live,
                rate: Rate::new(stated_kbps),
                last_chunk: 0,
                edge: None,
                edge_since: Instant::now(),
                edge_pull: None,
                heard_from: 0,
                heard_secs: 0.0,
                heard_was: (0, 0.0),
                heard_seen: 0.0,
                queued_secs: 0.0,
                underran: false,
            }),
            wake: Condvar::new(),
            cap_secs: AtomicU32::new(cap_secs),
            snap,
            on_title,
            interrupt,
        }
    }

    /// How long a window this is set to hold, in seconds. What it will
    /// actually hold is this or what the memory ceiling leaves of it,
    /// whichever is shorter, which is the `cap_secs` the shift publishes.
    pub fn cap_secs(&self) -> u32 {
        self.cap_secs.load(Ordering::Relaxed)
    }

    /// Put a smaller machine under this tape, so a test can reach the
    /// memory ceiling at a size it can actually write.
    #[cfg(test)]
    fn set_cap_bytes(&self, bytes: usize) {
        self.inner.lock().unwrap().cap_bytes = bytes;
    }

    /// Re-cap the tape with a station already on air, for the setting being
    /// moved by someone listening to the thing it changes.
    ///
    /// Growing costs nothing and takes effect gradually: the ceiling rises,
    /// nothing is dropped, and the window reaches the new length once the
    /// station has been on that long. Shrinking takes effect now, because
    /// getting the memory back is the whole reason anyone turns it down,
    /// and a trim that waited for the next append would leave a station
    /// that has gone quiet holding the old window indefinitely.
    pub fn set_cap_secs(&self, secs: u32) {
        self.cap_secs.store(secs, Ordering::Relaxed);

        let mut inner = self.inner.lock().unwrap();
        inner.trim(secs);
    }

    /// Add what came off the socket. Network thread only.
    pub fn append(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let mut inner = self.inner.lock().unwrap();
        inner.rate.wire_bytes += bytes.len() as u64;
        inner.last_chunk = bytes.len();

        // One cap for the pair: the room made for this chunk and the trim
        // that follows it are the same number, so the window can't be grown
        // for and then cut against two different readings of the rate.
        let cap = inner.cap(self.cap_secs());
        inner.reserve(cap, bytes.len());
        inner.buf.extend_from_slice(bytes);
        inner.trim_to(cap);
        drop(inner);

        self.wake.notify_all();
    }

    /// Network thread only, from inside the ICY reader.
    pub fn mark_title(&self, title: IcyTitle) {
        let mut inner = self.inner.lock().unwrap();
        let at = inner.head();
        if inner.marks.back().is_some_and(|last| last.title == title) {
            return;
        }

        // The first title on a connection names what's already playing: kept for
        // the song clock, but not a boundary.
        let joined = std::mem::take(&mut inner.joined);
        inner.marks.push_back(Mark { at, title, joined });
        if !joined {
            inner.marks_rev += 1;
        }
    }

    /// A fresh connection: nothing may decode across this point.
    pub fn splice(&self) {
        let mut inner = self.inner.lock().unwrap();
        let at = inner.head();
        inner.gaps.push_back(at);
        // The strips draw gaps, so this changes the drawn set.
        inner.marks_rev += 1;
        // A reconnect joins mid-song like the first connect.
        inner.joined = true;
    }

    pub fn set_feed(&self, state: Feed) {
        let mut inner = self.inner.lock().unwrap();
        inner.state = state;
        drop(inner);

        self.wake.notify_all();
    }

    pub fn feed(&self) -> Feed {
        self.inner.lock().unwrap().state
    }

    /// Decode thread only, once per chunk.
    pub fn note_audio(&self, secs: f64) {
        if secs <= 0.0 {
            return;
        }

        let mut inner = self.inner.lock().unwrap();
        inner.rate.played(secs);
        inner.heard_secs += secs;
    }

    /// Decode thread only, before each look at the shift.
    pub fn note_queued(&self, secs: f64) {
        self.inner.lock().unwrap().queued_secs = secs.max(0.0);
    }

    /// Where the listener stands, in seconds. The song clock comes only from the
    /// in-band marks, so stepping back mid-song reads mid-song; None until a
    /// title lands behind the listener. The length is None wherever it would be
    /// a guess.
    pub fn shift(&self) -> Shift {
        self.shift_at(Instant::now())
    }

    fn shift_at(&self, now: Instant) -> Shift {
        let mut inner = self.inner.lock().unwrap();
        let bps = inner.rate.bytes_per_sec();
        let edge = inner.edge(now, bps);
        let heard = inner.heard(bps);
        let (song_at, next_at) = inner.song_bounds(heard as u64);

        // Both ends move smoothly, so at speed the distance only drifts with the
        // station's clock.
        let band = inner.band(bps);
        let edge_of = LIVE_EDGE_SNAP_SECS.max(inner.chunk_secs(bps));
        let behind = (edge - heard).max(0.0) / bps;

        // Live is a state, not a distance: the connect burst leaves an unmoved
        // listener seconds back, and that still reads as live. The lead follows the
        // distance while audio keeps coming, so only a listener who stopped
        // decoding can slip behind.
        let moving = inner.heard_secs != inner.heard_seen;
        inner.heard_seen = inner.heard_secs;
        if moving || inner.live_lead.is_infinite() {
            inner.live_lead = behind;
        } else if behind > inner.live_lead + LIVE_SLIP_BANDS * band {
            inner.timeshifted = true;
        }

        let behind_secs = match !inner.timeshifted || behind < edge_of {
            true => 0.0,
            false => behind,
        };

        Shift {
            behind_secs,
            window_secs: (edge - inner.start as f64).max(0.0) / bps,
            cap_secs: inner.held_secs(self.cap_secs(), bps),
            bytes_per_sec: bps,
            song_secs: song_at.map(|at| (heard - at as f64).max(0.0) / bps),
            song_len_secs: song_at
                .filter(|at| *at >= inner.start)
                .zip(next_at)
                .map(|(at, next)| next.saturating_sub(at) as f64 / bps),
        }
    }

    /// Song boundaries inside the window, measured from the same drawn edge as
    /// the shift so marks and playhead slide together.
    pub fn live_marks(&self) -> Vec<LiveMark> {
        self.live_marks_at(Instant::now())
    }

    fn live_marks_at(&self, now: Instant) -> Vec<LiveMark> {
        let mut inner = self.inner.lock().unwrap();
        let bps = inner.rate.bytes_per_sec();
        let edge = inner.edge(now, bps);

        inner
            .marks
            .iter()
            .filter(|mark| inner.drawn(mark))
            .map(|mark| LiveMark {
                behind_secs: (edge - mark.at as f64).max(0.0) / bps,
                artist: mark.title.artist.clone(),
                title: mark.title.title.clone(),
            })
            .collect()
    }

    /// Reconnect joins, placed on both axes from one read: back from the live
    /// edge for the seek strip, and back from the listener in heard seconds for
    /// the waveform trace. Converted here, the one place that knows the rate.
    pub fn live_gaps(&self) -> Vec<LiveGap> {
        self.live_gaps_at(Instant::now())
    }

    fn live_gaps_at(&self, now: Instant) -> Vec<LiveGap> {
        let mut inner = self.inner.lock().unwrap();
        let bps = inner.rate.bytes_per_sec();
        let edge = inner.edge(now, bps);
        let heard = inner.heard(bps);

        inner
            .gaps
            .iter()
            .map(|at| LiveGap {
                // A splice past the drawn edge reads zero and slides in as the edge catches up.
                behind_secs: (edge - *at as f64).max(0.0) / bps,
                // Signed: a listener who stepped back has breaks ahead.
                heard_ago_secs: (heard - *at as f64) / bps,
            })
            .collect()
    }

    /// Moves only when a mark or gap arrives or rolls off. Cheap; the lists allocate.
    pub fn marks_rev(&self) -> u64 {
        self.inner.lock().unwrap().marks_rev
    }

    pub fn cursor(&self) -> u64 {
        self.inner.lock().unwrap().cursor
    }

    /// Taken once. The engine re-syncs, since the next packet would start mid-frame.
    pub fn took_underrun(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        std::mem::replace(&mut inner.underran, false)
    }

    /// The offset for a cursor `behind` seconds back, held to the tape and the
    /// live side of any gap. "Live" lands a lead behind the head, not on it: a
    /// cursor on the head starves until it drifts back a chunk anyway.
    pub fn seek_target(&self, behind_secs: f64) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        let bps = inner.rate.bytes_per_sec();
        let lead = (LIVE_EDGE_SNAP_SECS * bps).max(inner.last_chunk as f64 * 1.5) as u64;
        let back = ((behind_secs.max(0.0) * bps) as u64).max(lead);
        let head = inner.head();

        // Asking for no more than the lead is the LIVE button. The state is set
        // here, not guessed from a distance later.
        inner.timeshifted = back > lead;
        inner.live_lead = f64::INFINITY;

        // Step backs measure from the drawn edge so a click lands where pointed;
        // live measures from the head to keep the decoder fed.
        let from = match inner.timeshifted {
            true => inner.edge.map_or(head, |edge| edge as u64),
            false => head,
        };
        let mut at = from.saturating_sub(back).max(inner.start);

        // Land on the live side of any gap in between.
        if let Some(gap) = inner.gaps.iter().rev().find(|gap| **gap > at) {
            at = *gap;
        }

        match self.snap {
            Snap::Anywhere => at,
            Snap::OggPage => next_page(&inner, at).unwrap_or(head),
        }
    }

    /// For a reopen that failed after building its reader: the old reader
    /// carries on, so the published cursor does too.
    pub fn restore_cursor(&self, at: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.cursor = at;
        (inner.heard_from, inner.heard_secs) = inner.heard_was;

        // The seek that set the state didn't happen; work it out from the cursor.
        let bps = inner.rate.bytes_per_sec();
        let lead = (LIVE_EDGE_SNAP_SECS * bps).max(inner.last_chunk as f64 * 1.5) as u64;
        inner.timeshifted = inner.head().saturating_sub(at) > lead;
        inner.live_lead = f64::INFINITY;
    }

    /// For re-syncing a decoder after a timeshift seek.
    pub fn reader(self: &Arc<Self>, at: u64) -> TapeReader {
        let mut inner = self.inner.lock().unwrap();
        let at = at.clamp(inner.start, inner.head());
        inner.cursor = at;
        // Keep the old place in case the reopen fails.
        inner.heard_was = (inner.heard_from, inner.heard_secs);
        inner.heard_from = at;
        inner.heard_secs = 0.0;
        // Republish from the new place, which after a step back is an older song.
        inner.published = None;
        drop(inner);

        TapeReader {
            tape: Arc::clone(self),
            cursor: at,
        }
    }
}

/// None when the scan runs out of tape.
fn next_page(inner: &Inner, at: u64) -> Option<u64> {
    let from = at.saturating_sub(inner.start) as usize;
    let end = inner.buf.len().min(from + OGG_SCAN);

    inner.buf[from..end]
        .windows(OGGS.len())
        .position(|w| w == OGGS)
        .map(|off| at + off as u64)
}

/// The decode side of a tape. A read at the live edge waits for the network
/// thread. A timeshift seek overlaps two readers for one probe; only the new
/// one reads.
pub struct TapeReader {
    tape: Arc<Tape>,
    /// Mirrored into the tape, where the engine reads it.
    cursor: u64,
}

impl io::Read for TapeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }

        loop {
            let mut inner = self.tape.inner.lock().unwrap();

            // The pause outlasted the window: snap to the oldest byte and flag it so the
            // decoder is rebuilt around the jump.
            if self.cursor < inner.start {
                self.cursor = inner.start;
                inner.underran = true;
            }

            let head = inner.head();
            if self.cursor < head {
                let off = (self.cursor - inner.start) as usize;
                let n = out.len().min(inner.buf.len() - off);
                out[..n].copy_from_slice(&inner.buf[off..off + n]);
                self.cursor += n as u64;
                inner.cursor = self.cursor;
                inner.rate.played_bytes += n as u64;

                // Resolve the title here, fire it after unlocking: the sink runs engine code
                // and mustn't hold the network thread up.
                let mark = inner
                    .title_at(self.cursor)
                    .filter(|mark| inner.published != Some(mark.at))
                    .map(|mark| (mark.at, mark.title.clone()));
                let title = mark.map(|(at, title)| {
                    inner.published = Some(at);
                    title
                });
                let on_title = Arc::clone(&self.tape.on_title);
                drop(inner);

                if let Some(title) = title {
                    on_title(title);
                }

                return Ok(n);
            }

            match inner.state {
                // Out of reconnects: the station is over, like any track ending.
                Feed::Done => {
                    return Err(io::Error::other("the station is gone"));
                }

                // A command is waiting and the station is down: end the entry rather than
                // leave the press unanswered for the whole outage. Never checked while the
                // feed is live, or a pause would kill the stream it means to hold.
                Feed::Reconnecting if self.tape.interrupt.load(Ordering::Relaxed) => {
                    return Err(io::Error::other(
                        "the station is down and a command is waiting",
                    ));
                }

                _ => {
                    let _ = self.tape.wake.wait_timeout(inner, EDGE_STEP).unwrap();
                }
            }
        }
    }
}

impl io::Seek for TapeReader {
    /// Only the probe's small rewinds and the engine use this; symphonia is told
    /// the source isn't seekable.
    fn seek(&mut self, from: io::SeekFrom) -> io::Result<u64> {
        let mut inner = self.tape.inner.lock().unwrap();
        let target = match from {
            io::SeekFrom::Start(at) => Some(at),

            io::SeekFrom::Current(delta) => self.cursor.checked_add_signed(delta),

            io::SeekFrom::End(_) => {
                return Err(io::Error::other("a live stream has no end to seek from"));
            }
        }
        .ok_or_else(|| io::Error::other("seek out of range"))?;

        if target < inner.start || target > inner.head() {
            return Err(io::Error::other("seek outside the buffered window"));
        }

        self.cursor = target;
        inner.cursor = target;

        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::io::Seek;
    use std::io::SeekFrom;

    fn tape(cap_secs: u32, kbps: u32) -> Arc<Tape> {
        Arc::new(Tape::new(
            cap_secs,
            kbps,
            Snap::Anywhere,
            crate::icy::no_titles(),
            Arc::new(AtomicBool::new(false)),
        ))
    }

    fn bytes(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    /// In 16 kB reads, so the live lead is the snap distance, not a chunk and a
    /// half of one huge append.
    fn feed(tape: &Tape, n: usize) {
        for chunk in bytes(n).chunks(16_000) {
            tape.append(chunk);
        }
    }

    #[test]
    fn a_read_behind_the_edge_comes_straight_out_of_the_window() {
        let tape = tape(60, 128);
        tape.append(&bytes(4096));

        let mut reader = tape.reader(0);
        let mut out = vec![0u8; 1024];
        assert_eq!(reader.read(&mut out).unwrap(), 1024);
        assert_eq!(out, bytes(4096)[..1024]);
        assert_eq!(tape.cursor(), 1024);
    }

    /// A time length: at 32 kB/s, ten seconds is 320 kB.
    #[test]
    fn the_window_drops_its_oldest_past_the_cap() {
        let tape = tape(10, 256);
        tape.append(&bytes(512 * 1024));

        let window = tape.shift().window_secs;
        let inner = tape.inner.lock().unwrap();
        assert_eq!(inner.buf.len(), 320 * 1000, "trimmed back to the cap");
        assert_eq!(inner.start, 512 * 1024 - 320 * 1000);
        assert!(
            (window - 10.0).abs() < 0.01,
            "ten seconds of tape: {window}"
        );
    }

    #[test]
    fn a_cursor_that_fell_off_the_back_snaps_and_says_so() {
        let tape = tape(10, 256);
        let mut reader = tape.reader(0);
        tape.append(&bytes(512 * 1024));

        let mut out = vec![0u8; 16];
        assert_eq!(reader.read(&mut out).unwrap(), 16);
        assert!(tape.took_underrun(), "the read fell off the back");
        assert!(!tape.took_underrun(), "and the flag is taken once");
        assert_eq!(tape.cursor(), 512 * 1024 - 320 * 1000 + 16);
    }

    #[test]
    fn a_read_at_the_edge_waits_for_the_next_chunk() {
        let tape = tape(60, 128);
        let mut reader = tape.reader(0);

        let writer = Arc::clone(&tape);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            writer.append(&bytes(64));
        });

        let mut out = vec![0u8; 64];
        let began = Instant::now();
        assert_eq!(reader.read(&mut out).unwrap(), 64);
        assert!(began.elapsed() >= Duration::from_millis(20), "it waited");
    }

    /// The one interrupt that ends a read: a command while the station is down.
    #[test]
    fn a_waiting_command_ends_a_read_at_a_dead_edge() {
        let interrupt = Arc::new(AtomicBool::new(false));
        let tape = Arc::new(Tape::new(
            60,
            128,
            Snap::Anywhere,
            crate::icy::no_titles(),
            Arc::clone(&interrupt),
        ));
        let mut reader = tape.reader(0);
        tape.set_feed(Feed::Reconnecting);
        interrupt.store(true, Ordering::Relaxed);

        let mut out = vec![0u8; 16];
        assert!(reader.read(&mut out).is_err());
    }

    #[test]
    fn a_dead_station_ends_the_read() {
        let tape = tape(60, 128);
        let mut reader = tape.reader(0);
        tape.set_feed(Feed::Done);

        let mut out = vec![0u8; 16];
        assert!(reader.read(&mut out).is_err());
    }

    #[test]
    fn a_seek_never_crosses_a_gap() {
        let tape = tape(600, 256);
        feed(&tape, 64_000);
        tape.splice();
        feed(&tape, 64_000);

        assert_eq!(tape.seek_target(3.0), 64_000, "held at the splice");
        // A second back is inside the lead, which still clears the splice.
        assert_eq!(
            tape.seek_target(1.0),
            64_000,
            "a second back is the lead, and the lead sits on the splice"
        );
        feed(&tape, 64_000);
        assert_eq!(
            tape.seek_target(1.0),
            64_000 + 64_000,
            "with room behind the edge the lead clears the splice"
        );
    }

    #[test]
    fn a_seek_target_holds_to_what_the_tape_has() {
        let tape = tape(600, 256);
        feed(&tape, 64_000);

        assert_eq!(tape.seek_target(600.0), 0, "the oldest byte held");
        // Live is a two-second lead behind the edge.
        assert_eq!(tape.seek_target(0.0), 0, "two seconds of a two-second tape");
        feed(&tape, 64_000);
        assert_eq!(tape.seek_target(0.0), 64_000, "the edge less the lead");
    }

    #[test]
    fn an_ogg_seek_lands_on_a_page_header() {
        let tape = Arc::new(Tape::new(
            600,
            256,
            Snap::OggPage,
            crate::icy::no_titles(),
            Arc::new(AtomicBool::new(false)),
        ));
        let mut stream = bytes(32_000);
        stream.extend_from_slice(OGGS);
        stream.extend_from_slice(&bytes(32_000));
        tape.append(&stream);

        assert_eq!(tape.seek_target(1.5), 32_000, "forward to the page");
    }

    /// The title under the cursor, not under the socket.
    #[test]
    fn titles_fire_as_the_cursor_reaches_them() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let tape = Arc::new(Tape::new(
            600,
            256,
            Snap::Anywhere,
            Arc::new(move |title: IcyTitle| sink.lock().unwrap().push(title.title)),
            Arc::new(AtomicBool::new(false)),
        ));

        let song = |title: &str| IcyTitle {
            artist: String::new(),
            title: title.to_string(),
        };

        let mut reader = tape.reader(0);
        tape.mark_title(song("first"));
        tape.append(&bytes(1000));
        tape.mark_title(song("second"));
        tape.append(&bytes(1000));

        let mut out = vec![0u8; 500];
        assert_eq!(reader.read(&mut out).unwrap(), 500);
        assert_eq!(*seen.lock().unwrap(), vec!["first".to_string()]);

        let mut out = vec![0u8; 1000];
        assert_eq!(reader.read(&mut out).unwrap(), 1000);
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["first".to_string(), "second".to_string()],
            "the second song lands once the cursor is past its mark"
        );
    }

    /// The connect burst reads as live, not six seconds behind jittering to seven.
    #[test]
    fn a_connect_burst_reads_as_live_until_the_listener_steps_back() {
        let tape = tape(600, 256);
        feed(&tape, 192_000);

        let mut reader = tape.reader(0);
        assert_eq!(tape.shift().behind_secs, 0.0, "at the front of the burst");

        let t0 = Instant::now();
        for second in 1..=10 {
            feed(&tape, 32_000);
            let mut out = vec![0u8; 32_000];
            assert_eq!(reader.read(&mut out).unwrap(), 32_000);
            tape.note_audio(1.0);

            let now = t0 + Duration::from_secs(second);
            assert_eq!(tape.shift_at(now).behind_secs, 0.0, "still live");
        }

        // Ten seconds back, inside the sixteen held.
        let at = tape.seek_target(10.0);
        let _reader = tape.reader(at);
        let behind = tape.shift().behind_secs;
        assert!(
            (behind - 10.0).abs() < 1.0,
            "ten seconds into the buffer: {behind}"
        );

        // The LIVE button puts it back.
        let at = tape.seek_target(0.0);
        let _reader = tape.reader(at);
        assert_eq!(tape.shift().behind_secs, 0.0, "live again");
    }

    /// A burst spread over the first second, after the first look, still reads
    /// as live.
    #[test]
    fn a_burst_that_lands_after_the_first_look_still_reads_as_live() {
        // 128 kbps: 10 ms is 160 bytes, the 64 kB burst four seconds.
        let tape = tape(600, 128);
        tape.append(&bytes(4_000));

        let t0 = Instant::now();
        let mut reader = tape.reader(0);
        assert_eq!(tape.shift_at(t0).behind_secs, 0.0, "live at the open");

        let mut buffered = 0usize;
        for tick in 1..=2000u64 {
            if tick <= 16 {
                tape.append(&bytes(4_000));
            } else if tick % 25 == 0 {
                tape.append(&bytes(4_000));
            }

            if buffered < 160 {
                let mut out = vec![0u8; 32 * 1024];
                buffered += reader.read(&mut out).unwrap();
            }
            buffered -= 160;
            tape.note_audio(0.01);

            let now = t0 + Duration::from_millis(tick * 10);
            let behind = tape.shift_at(now).behind_secs;
            assert_eq!(behind, 0.0, "still live {tick} ticks in: {behind}");
        }
    }

    /// A pause lets the broadcast run on: that distance is real.
    #[test]
    fn a_pause_that_lets_the_broadcast_run_on_stops_reading_as_live() {
        let tape = tape(600, 256);
        feed(&tape, 64_000);

        let _reader = tape.reader(0);
        assert_eq!(tape.shift().behind_secs, 0.0, "live at the open");

        feed(&tape, 960_000);

        let behind = tape.shift().behind_secs;
        assert!(
            (behind - 32.0).abs() < 2.0,
            "the pause is a real distance: {behind}"
        );
    }

    /// Steady playback behind the broadcast, with chunked arrival, 32 KiB reads,
    /// and 10 ms output. Neither saw may reach the playhead, bar, or marks: each
    /// moves no more than the clock allows, the bar and marks only one way.
    #[test]
    fn the_strip_moves_smoothly_while_the_edge_and_the_reads_arrive_in_bursts() {
        let song = |title: &str| IcyTitle {
            artist: String::new(),
            title: title.to_string(),
        };

        for chunk_secs in [0.25, 1.0, 2.0] {
            let chunk = (chunk_secs * 32_000.0) as usize;
            let period = (chunk_secs * 100.0) as usize;

            // A minute on tape, a song starting halfway; the first title is the
            // connect's, not a boundary.
            let tape = tape(600, 256);
            tape.mark_title(song("on at the connect"));
            feed(&tape, 960_000);
            tape.mark_title(song("second"));
            feed(&tape, 960_000);

            let t0 = Instant::now();
            let at = tape.seek_target(30.0);
            let mut reader = tape.reader(at);
            let mut buffered = 0usize;
            let mut seen = Vec::new();
            for tick in 1..=6000u64 {
                if tick % period as u64 == 0 {
                    tape.append(&bytes(chunk));
                }

                if buffered < 320 {
                    let mut out = vec![0u8; 32 * 1024];
                    buffered += reader.read(&mut out).unwrap();
                }
                buffered -= 320;
                tape.note_audio(0.01);

                let now = t0 + Duration::from_millis(tick * 10);
                let shift = tape.shift_at(now);
                let mark = tape.live_marks_at(now)[0].behind_secs;
                seen.push((shift.behind_secs, shift.window_secs, mark));
            }

            for pair in seen.windows(2) {
                let ((behind, window, mark), (behind_next, window_next, mark_next)) =
                    (pair[0], pair[1]);

                // The edge runs at most twice real time, so 10 ms moves the playhead at most 10 ms.
                assert!(
                    (behind_next - behind).abs() <= 0.011,
                    "the playhead jumped on a {chunk_secs}s chunk: {behind} to {behind_next}"
                );
                assert!(
                    (0.0..=0.021).contains(&(window_next - window)),
                    "the bar jumped on a {chunk_secs}s chunk: {window} to {window_next}"
                );
                assert!(
                    (0.0..=0.021).contains(&(mark_next - mark)),
                    "the mark jumped on a {chunk_secs}s chunk: {mark} to {mark_next}"
                );
            }

            // Once settled, the playhead doesn't wander at all.
            let settled = seen[3000..].iter().map(|(behind, _, _)| *behind);
            let (lo, hi) = settled.fold((f64::MAX, f64::MIN), |(lo, hi), b| (lo.min(b), hi.max(b)));
            assert!(
                hi - lo < 0.02,
                "the playhead wandered {:.3}s on a {chunk_secs}s chunk",
                hi - lo
            );

            // Give or take the band, which the seek was measured before.
            let (behind, _, _) = seen[seen.len() - 1];
            assert!(
                (behind - 30.0).abs() < 1.0 + 1.5 * chunk_secs,
                "still about thirty seconds behind on a {chunk_secs}s chunk: {behind}"
            );
        }
    }

    /// Turning the setting down trims on the spot.
    #[test]
    fn a_smaller_cap_trims_the_window_it_already_holds() {
        let tape = tape(60, 256);
        tape.append(&bytes(1_920_000));
        assert_eq!(tape.inner.lock().unwrap().buf.len(), 1_920_000);

        tape.set_cap_secs(10);

        assert_eq!(tape.cap_secs(), 10);
        assert_eq!(tape.inner.lock().unwrap().buf.len(), 320_000);
        assert_eq!(tape.shift().cap_secs, 10.0);
    }

    /// Turning it up keeps every byte; the window fills into the new length.
    #[test]
    fn a_larger_cap_keeps_every_byte_it_already_had() {
        let tape = tape(60, 256);
        tape.append(&bytes(1_920_000));

        tape.set_cap_secs(600);

        assert_eq!(tape.cap_secs(), 600);
        assert_eq!(tape.inner.lock().unwrap().buf.len(), 1_920_000);
        let window = tape.shift().window_secs;
        assert!(
            (window - 60.0).abs() < 0.5,
            "what's held, not the cap: {window}"
        );
        assert_eq!(tape.shift().cap_secs, 600.0);
    }

    /// The memory ceiling shortens a window the setting asked for.
    #[test]
    fn the_memory_ceiling_cuts_a_window_the_setting_asked_for() {
        // Ten minutes at 32 kB/s is 19.2 MB; a 2 MB ceiling holds 62.5 seconds.
        let tape = tape(600, 256);
        tape.set_cap_bytes(2_000_000);
        feed(&tape, 19_200_000);

        let held = tape.inner.lock().unwrap().buf.len();
        assert!(
            held <= 2_000_000 + 2_000_000 / TRIM_SLACK,
            "held {held} bytes"
        );
        assert_eq!(tape.cap_secs(), 600, "the setting itself is untouched");
        assert_eq!(tape.shift().cap_secs, 62.5, "and the strip is drawn to it");
    }

    /// The allocation stays inside the ceiling too, despite Vec doubling.
    #[test]
    fn the_allocation_stays_inside_the_ceiling() {
        // Twice the window, well past where a doubling would land.
        let tape = tape(600, 256);
        feed(&tape, 38_400_000);

        let inner = tape.inner.lock().unwrap();
        let capacity = inner.buf.capacity();
        assert!(inner.buf.len() <= 19_200_000 + 19_200_000 / TRIM_SLACK);
        assert!(capacity < 24_000_000, "allocation of {capacity} bytes");
    }

    /// Turning it down hands the allocation back, not just the length.
    #[test]
    fn a_smaller_cap_hands_the_allocation_back() {
        let tape = tape(600, 256);
        feed(&tape, 19_200_000);
        assert!(tape.inner.lock().unwrap().buf.capacity() >= 19_200_000);

        tape.set_cap_secs(60);

        let inner = tape.inner.lock().unwrap();
        assert_eq!(inner.buf.len(), 1_920_000);
        let capacity = inner.buf.capacity();
        assert!(capacity < 4_000_000, "allocation of {capacity} bytes");
    }

    #[test]
    fn the_shift_carries_the_rate_the_window_is_sized_at() {
        let tape = tape(600, 256);
        tape.append(&bytes(64_000));

        assert_eq!(tape.shift().bytes_per_sec, 32_000.0);
    }

    #[test]
    fn the_shift_carries_the_buffer_length_it_was_set_to() {
        let tape = tape(600, 256);
        tape.append(&bytes(64_000));

        let shift = tape.shift();
        assert_eq!(shift.cap_secs, 600.0);
        assert_eq!(shift.window_secs, 2.0, "and what has actually arrived");
    }

    #[test]
    fn the_marks_come_back_as_distances_from_the_edge() {
        let tape = tape(600, 256);
        let song = |name: &str| IcyTitle {
            artist: "Boards of Canada".into(),
            title: name.into(),
        };

        // The first title is the connect's; the three after are the boundaries.
        tape.mark_title(song("Telephasic Workshop"));
        tape.append(&bytes(32_000));
        tape.mark_title(song("Roygbiv"));
        tape.append(&bytes(96_000));
        tape.mark_title(song("Olson"));
        tape.append(&bytes(64_000));
        tape.mark_title(song("Rue the Whirl"));
        tape.append(&bytes(32_000));

        let marks = tape.live_marks();
        let seen: Vec<_> = marks
            .iter()
            .map(|mark| (mark.title.as_str(), mark.behind_secs))
            .collect();

        assert_eq!(
            seen,
            vec![("Roygbiv", 6.0), ("Olson", 3.0), ("Rue the Whirl", 1.0)],
            "oldest first, each back from the live edge"
        );
        assert!(marks.iter().all(|mark| mark.artist == "Boards of Canada"));
    }

    /// Gaps on both axes, and a new one moves the revision.
    #[test]
    fn the_gaps_come_back_on_both_axes() {
        let tape = tape(600, 256);
        let mut reader = tape.reader(0);

        feed(&tape, 64_000);
        let rev = tape.marks_rev();
        tape.splice();
        feed(&tape, 96_000);
        assert!(tape.marks_rev() > rev, "the reconnect changed the set");

        let mut out = vec![0u8; 160_000];
        assert_eq!(reader.read(&mut out).unwrap(), 160_000);
        tape.note_audio(5.0);

        let gaps = tape.live_gaps();
        assert_eq!(gaps.len(), 1, "one connection, one break");
        assert_eq!(
            gaps[0].behind_secs, 3.0,
            "two seconds in, three from the edge"
        );
        assert_eq!(
            gaps[0].heard_ago_secs, 3.0,
            "and three seconds of audio ago"
        );

        // A break ahead of the listener reads negative, not clamped.
        feed(&tape, 32_000);
        tape.splice();

        let gaps = tape.live_gaps();
        assert_eq!(gaps.len(), 2, "both still inside the window");
        assert_eq!(
            gaps[1].heard_ago_secs, -1.0,
            "a second of audio still to come"
        );
        assert!(gaps[0].behind_secs > gaps[1].behind_secs, "oldest first");
    }

    #[test]
    fn a_gap_trimmed_off_the_back_stops_being_published() {
        let tape = tape(10, 256);

        feed(&tape, 64_000);
        tape.splice();
        feed(&tape, 64_000);
        assert_eq!(tape.live_gaps().len(), 1, "inside the window");
        let rev = tape.marks_rev();

        // A whole window's worth takes the splice with it.
        feed(&tape, 320_000);
        assert!(tape.live_gaps().is_empty(), "rolled off with the tape");
        assert!(tape.marks_rev() > rev, "and the set said it changed");
    }

    /// A trimmed song start stops being drawn, though its mark is kept.
    #[test]
    fn a_mark_trimmed_off_the_back_stops_being_published() {
        let tape = tape(10, 256);
        let song = |name: &str| IcyTitle {
            artist: String::new(),
            title: name.into(),
        };

        tape.mark_title(song("tuning in"));
        tape.mark_title(song("first"));
        tape.append(&bytes(160_000));
        tape.mark_title(song("second"));
        assert_eq!(tape.live_marks().len(), 2, "both still inside");
        let rev = tape.marks_rev();

        tape.append(&bytes(320_000));

        let marks = tape.live_marks();
        assert_eq!(marks.len(), 1, "the trimmed one went");
        assert_eq!(marks[0].title, "second");
        assert!(tape.marks_rev() > rev, "and the set said it changed");

        // The back of the window still has a song clock.
        let _reader = tape.reader(0);
        assert!(tape.shift().song_secs.is_some());
    }

    /// The song clock: a seek back lands mid-song and reads mid-song.
    #[test]
    fn the_marks_say_how_far_into_a_song_the_cursor_is() {
        let tape = tape(600, 256);
        let song = |title: &str| IcyTitle {
            artist: String::new(),
            title: title.to_string(),
        };

        tape.mark_title(song("first"));
        tape.append(&bytes(64_000));
        tape.mark_title(song("second"));
        tape.append(&bytes(64_000));

        let shift = tape.shift();
        assert_eq!(shift.song_secs, Some(0.0));
        assert_eq!(shift.song_len_secs, Some(2.0), "mark to mark");

        let mut reader = tape.reader(32_000);
        let shift = tape.shift();
        assert_eq!(shift.song_secs, Some(1.0));
        assert_eq!(shift.song_len_secs, Some(2.0));

        // The newest song hasn't ended, so no length.
        let mut out = vec![0u8; 48_000];
        assert_eq!(reader.read(&mut out).unwrap(), 48_000);
        tape.note_audio(1.5);
        let shift = tape.shift();
        assert_eq!(shift.song_secs, Some(0.5));
        assert_eq!(shift.song_len_secs, None, "the newest song is unfinished");
    }

    /// No marks, no song clock; zero would be a claim.
    #[test]
    fn a_stream_with_no_marks_has_no_song_clock() {
        let tape = tape(600, 256);
        tape.append(&bytes(64_000));

        let shift = tape.shift();
        assert_eq!(shift.song_secs, None);
        assert_eq!(shift.song_len_secs, None);
    }

    /// A trimmed song keeps its clock but claims no length.
    #[test]
    fn a_song_trimmed_off_the_back_keeps_its_clock_and_loses_its_length() {
        let tape = tape(10, 256);
        let song = |title: &str| IcyTitle {
            artist: String::new(),
            title: title.to_string(),
        };

        tape.mark_title(song("first"));
        tape.append(&bytes(320_000));
        tape.mark_title(song("second"));
        tape.append(&bytes(64_000));

        let start = tape.inner.lock().unwrap().start;
        assert!(start > 0, "the oldest bytes were dropped");

        let _reader = tape.reader(start + 16_000);
        let shift = tape.shift();
        assert_eq!(
            shift.song_secs,
            Some((start + 16_000) as f64 / 32_000.0),
            "counted from the mark, gone bytes and all"
        );
        assert_eq!(shift.song_len_secs, None, "and no length to claim");
    }

    /// A stated rate is never replaced by a measurement: every drawn number is
    /// bytes over it, so revising it would rescale the whole strip.
    #[test]
    fn a_stated_rate_is_never_replaced_by_a_measurement() {
        let tape = tape(600, 256);
        let song = |name: &str| IcyTitle {
            artist: String::new(),
            title: name.into(),
        };

        tape.mark_title(song("tuning in"));
        feed(&tape, 320_000);
        tape.mark_title(song("first"));
        feed(&tape, 320_000);

        let mut reader = tape.reader(0);
        let window = tape.shift().window_secs;
        let mark = tape.live_marks()[0].behind_secs;
        assert_eq!(window, 20.0, "twenty seconds at the stated rate");

        // Measures 24 kB/s, a quarter off the claim, and is ignored.
        for _ in 0..20 {
            let mut out = vec![0u8; 24_000];
            assert_eq!(reader.read(&mut out).unwrap(), 24_000);
            tape.note_audio(1.0);
        }

        assert_eq!(tape.shift().window_secs, window, "the bar didn't move");
        assert_eq!(tape.live_marks()[0].behind_secs, mark, "nor the mark");
        assert_eq!(tape.shift().bytes_per_sec, 32_000.0);
    }

    /// Unstated: the rate settles once, on the first ten decoded seconds, and holds.
    #[test]
    fn an_unstated_rate_settles_once_and_then_holds() {
        let tape = tape(600, 0);
        feed(&tape, 640_000);

        let mut reader = tape.reader(0);
        let mut seen = vec![tape.shift().window_secs];

        // 32 kB per decoded second. The first tick opens the window, the tenth after
        // closes it.
        for _ in 0..16 {
            let mut out = vec![0u8; 32_000];
            assert_eq!(reader.read(&mut out).unwrap(), 32_000);
            tape.note_audio(1.0);
            seen.push(tape.shift().window_secs);
        }

        let mut distinct = seen.clone();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            2,
            "one settle and nothing after it: {seen:?}"
        );

        // Twenty seconds, give or take the first tick: opening the window discards
        // what was read before it.
        let settled = *seen.last().expect("samples");
        assert!(
            (settled - 20.0).abs() < 3.0,
            "about twenty seconds of tape at the measured rate: {settled}"
        );
    }

    #[test]
    fn a_seek_outside_the_window_is_refused() {
        let tape = tape(600, 256);
        tape.append(&bytes(1000));

        let mut reader = tape.reader(500);
        assert_eq!(reader.seek(SeekFrom::Start(750)).unwrap(), 750);
        assert!(reader.seek(SeekFrom::Start(2000)).is_err());
        assert!(reader.seek(SeekFrom::End(-10)).is_err());
    }

    /// Live lands a snap distance behind the edge; further back is honoured as asked.
    #[test]
    fn a_seek_to_live_lands_a_chunk_behind_the_edge() {
        let tape = tape(600, 128);
        tape.set_feed(Feed::Live);
        let bps = tape.inner.lock().unwrap().rate.bytes_per_sec();
        // Past the snap distance, so the lead is the snap, not the chunk.
        let chunk = bytes(16 * 1024);
        while (tape.inner.lock().unwrap().head() as f64) < bps * 60.0 {
            tape.append(&chunk);
        }
        let head = tape.inner.lock().unwrap().head();

        let at_live = tape.seek_target(0.0);
        let lead = head - at_live;
        let want = (LIVE_EDGE_SNAP_SECS * bps) as u64;
        assert!(
            lead >= want && lead <= want + chunk.len() as u64,
            "lead {lead} vs {want}"
        );

        let back = tape.seek_target(30.0);
        assert!(
            head - back > lead,
            "an asked-for distance is further than the lead"
        );
    }
}
