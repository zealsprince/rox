//! State shared between the decode thread, the RT output callback, and the UI.
//! The callback only ever touches the atomics; the mutexes are decode-thread
//! and UI-thread only.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

use rox_library::locator::Locator;

use crate::http::StationInfo;
use crate::icy::IcyTitle;

/// Where a stream stands. Files have no version of this: they open instantly
/// and never drop, where a station spends seconds opening and routinely drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Opening,
    Live,
    Reconnecting,
    /// The reconnects ran out; the queue moves on.
    Dropped,
}

/// Bound to its queue entry at the open, like
/// [`TitleSink`](crate::icy::TitleSink).
pub type StreamSink = Arc<dyn Fn(StreamState) + Send + Sync>;

/// For local files and analysis passes.
pub fn no_stream() -> StreamSink {
    Arc::new(|_| {})
}

/// Where a live stream is played from. A station is taped as it arrives, so
/// playback is a cursor into the last few minutes. Seconds, since every
/// reader draws time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shift {
    /// Distance to the live edge; zero is live.
    pub behind_secs: f64,
    /// Tape held so far; reaches `cap_secs` once the station has been on long enough.
    pub window_secs: f64,
    /// What the buffer will hold: the setting, or less under the memory ceiling
    /// at this station's rate.
    pub cap_secs: f64,
    /// Measured by playback, else `icy-br`, else the socket average. Turns a
    /// length of buffer into memory.
    pub bytes_per_sec: f64,
    /// How far into the song under the cursor, off the in-band title marks. None
    /// until a title has been announced behind the cursor. Survives a step back:
    /// landing mid-song shows mid-song.
    pub song_secs: Option<f64>,
    /// Mark to mark. None for the newest song and for one trimmed off the back.
    pub song_len_secs: Option<f64>,
}

/// A song boundary in the buffer, measured back from the live edge like
/// [`Shift::behind_secs`]. The song under the oldest byte has no mark: its
/// start is off the back.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveMark {
    pub behind_secs: f64,
    pub artist: String,
    pub title: String,
}

/// A reconnect splice in the buffer: nothing decodes across it, so a seek
/// stops there. Placed on both strips' axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveGap {
    /// Back from the live edge, like [`LiveMark::behind_secs`], for the tape strip.
    pub behind_secs: f64,
    /// Seconds of heard audio since the listener crossed it, negative if ahead,
    /// for the rolling trace. The edge distance can't stand in: it reads zero
    /// while the listener is really a chunk back, which is a fifth of an
    /// eight-second trace.
    pub heard_ago_secs: f64,
}

const NO_SHIFT: u64 = u64::MAX;

/// NaN stands for None, so two optional numbers need no extra flag atomics.
fn secs_bits(secs: Option<f64>) -> u64 {
    secs.unwrap_or(f64::NAN).to_bits()
}

/// Non-finite reads as None: the sentinel, or a divide by a silly rate.
fn secs_of(bits: u64) -> Option<f64> {
    let secs = f64::from_bits(bits);

    secs.is_finite().then_some(secs)
}

/// Maps the output clock back to a track position. Appended on every track
/// start and seek.
pub struct Segment {
    /// Output clock frame where this segment begins.
    pub at_frame: u64,
    /// Pool index of the track.
    pub track: usize,
    /// Device-rate frames into the track at segment start.
    pub track_frame: u64,
}

/// One queue entry as the UI sees it. The id is stable across reorders, so
/// an edit can't hit the wrong track. `explicit` is Play Next / Add to Queue;
/// the rest is context playing on behind them.
#[derive(Clone)]
pub struct QueueEntry {
    pub id: u64,
    pub locator: Locator,
    pub explicit: bool,
    /// Distinct per entry even when two share a path, so the UI resolves the
    /// right occurrence.
    pub idx: usize,
    /// Album group (ADR 17), supplied by the player; the engine only compares.
    /// Album shuffle keeps a group together and crossfade leaves its splices alone.
    pub group: Option<u64>,
}

/// History is `entries[..cursor]`, upcoming `entries[cursor + 1..]`.
/// Rewritten on queue edits, not on a plain advance.
#[derive(Clone, Default)]
pub struct QueueSnapshot {
    pub entries: Vec<QueueEntry>,
    pub cursor: usize,
}

#[derive(Clone)]
pub struct TrackInfo {
    pub name: String,
    pub duration_secs: Option<f64>,
    /// Excludes encoder delay and padding.
    pub num_frames: Option<u64>,
    pub sample_rate: u32,
    pub channels: usize,
}

pub struct Shared {
    /// False is paused: the callback plays silence and stops consuming, so the
    /// position freezes to the sample. On a station the connection keeps taping,
    /// so resuming plays on from where it stopped.
    pub playing: AtomicBool,
    /// Frames left in an audition blip: a step taken while paused plays this
    /// many frames without touching `playing`. Counted here because timing it
    /// from the UI would be mostly buffer latency.
    pub audition_left: AtomicU64,
    /// The flush epoch, bumped on a seek or skip. A backend discards the ring
    /// once per new number and echoes it in `flush_ack`. An epoch, not a flag: a
    /// flag's clear races the callback and eats the new track's first samples.
    pub flush_seq: AtomicU64,
    /// The decode thread waits on this before resyncing its clock.
    pub flush_ack: AtomicU64,
    /// Where the crossfade in flight becomes audible, on the output clock, and
    /// its length; zero length is no fade. The transport shows the overlap while
    /// the ear is in it.
    pub fade_at: AtomicU64,
    pub fade_len: AtomicU64,
    /// From a Previous, so the transport sweeps the right way.
    pub fade_back: AtomicBool,
    /// The A-B marks as f64 bits, `u64::MAX` in `ab_a` for none. Written b first
    /// and read a first, so a reader never pairs a new A with an old B.
    pub ab_a: AtomicU64,
    pub ab_b: AtomicU64,
    pub volume_bits: AtomicU32,
    /// Frames actually sent to the device, excluding flushes and silence: the
    /// global output clock.
    pub frames_consumed: AtomicU64,
    pub ended: AtomicBool,
    /// Set by the backend when the device drops out. The ring fills and the
    /// engine parks, so the app polls this to reopen rather than freeze on
    /// "playing". Cleared by the app.
    pub device_lost: AtomicBool,
    /// A command the listener wants answered now. Set by the player as it sends,
    /// cleared by the engine as it drains. Only read while a station is down:
    /// the feed thread abandons its retries and the decode thread stops waiting
    /// at the live edge. An `Arc` because the transport under symphonia has no
    /// other way back here.
    pub interrupt: Arc<AtomicBool>,
    pub segments: Mutex<Vec<Segment>>,
    pub tracks: Mutex<Vec<Option<TrackInfo>>>,
    /// Station titles per entry. Separate from `tracks` because the two are
    /// written on different clocks: an open rewrites `tracks[i]` whole, and a
    /// title can arrive mid-probe.
    pub titles: Mutex<Vec<Option<IcyTitle>>>,
    /// Each station's `icy-` headers, read once per open.
    pub station: Mutex<Vec<Option<StationInfo>>>,
    /// Stream state per entry, None for local files. Written by the engine's open
    /// and the transport's reconnects, which never overlap.
    pub stream: Mutex<Vec<Option<StreamState>>>,
    /// Rewritten on queue edits, not on a plain advance; the playing entry comes
    /// off the position clock.
    pub queue: Mutex<QueueSnapshot>,
    /// Lets the UI skip cloning the snapshot on unchanged ticks.
    pub queue_rev: AtomicU64,
    /// Bumped only on a real change, so the pump polls an atomic.
    pub title_rev: AtomicU64,
    /// The audible live entry's [`Shift`] as f64 bits, with its pool index
    /// (`u64::MAX` for none). Off the title revision on purpose: these move every
    /// chunk, and bumping the revision would make every poller re-read.
    pub shift_idx: AtomicU64,
    pub shift_behind: AtomicU64,
    pub shift_window: AtomicU64,
    pub shift_cap: AtomicU64,
    pub shift_rate: AtomicU64,
    pub shift_song: AtomicU64,
    pub shift_song_len: AtomicU64,
    /// The audible station's song boundaries for the buffer strip. Behind a lock,
    /// with changes signalled by the title revision.
    pub live_marks: Mutex<Vec<LiveMark>>,
    /// Where the audible station's connection broke. Published with the marks
    /// since both move as the tape rolls.
    pub live_gaps: Mutex<Vec<LiveGap>>,
    /// Why the last entry the queue skipped couldn't open, until read. A skip
    /// leaves nothing else to look at.
    pub refusal: Mutex<Option<String>>,
    /// The pump ticks sixty times a second; this keeps the common tick lock-free.
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

    /// The newest reason wins.
    pub fn publish_refusal(&self, reason: String) {
        *self.refusal.lock().unwrap() = Some(reason);
        self.refusal_pending
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Reading clears it.
    pub fn take_refusal(&self) -> Option<String> {
        if !self
            .refusal_pending
            .swap(false, std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }

        self.refusal.lock().unwrap().take()
    }

    /// Repeats are dropped: stations resend the title every block, and the
    /// revision has to mean "the song changed" for scrobbling.
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

    pub fn live_title(&self, idx: usize) -> Option<IcyTitle> {
        self.titles.lock().unwrap().get(idx).cloned().flatten()
    }

    /// Rides the title revision: to every reader it means the same thing.
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

    pub fn station_info(&self, idx: usize) -> Option<StationInfo> {
        self.station.lock().unwrap().get(idx).cloned().flatten()
    }

    /// On the title revision too, so a reconnect without a song change still
    /// wakes surfaces drawing the station.
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

    pub fn stream_state(&self, idx: usize) -> Option<StreamState> {
        self.stream.lock().unwrap().get(idx).copied().flatten()
    }

    pub fn title_rev(&self) -> u64 {
        self.title_rev.load(std::sync::atomic::Ordering::Acquire)
    }

    /// The index is stored last and read first, so a reader never pairs one
    /// entry's numbers with another's.
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

    /// Withdraw the shift for a station that stopped playing.
    pub fn clear_shift(&self) {
        self.shift_idx
            .store(NO_SHIFT, std::sync::atomic::Ordering::Release);

        // The revision only moves when there was something to clear.
        let marks = std::mem::take(&mut *self.live_marks.lock().unwrap());
        let gaps = std::mem::take(&mut *self.live_gaps.lock().unwrap());
        if !marks.is_empty() || !gaps.is_empty() {
            self.title_rev
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }

    /// `changed` means the sets changed; only that is worth a revision, not the
    /// distances sliding every chunk.
    pub fn publish_live_marks(&self, marks: Vec<LiveMark>, gaps: Vec<LiveGap>, changed: bool) {
        *self.live_marks.lock().unwrap() = marks;
        *self.live_gaps.lock().unwrap() = gaps;

        if changed {
            self.title_rev
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }

    pub fn live_marks(&self) -> Vec<LiveMark> {
        self.live_marks.lock().unwrap().clone()
    }

    pub fn live_gaps(&self) -> Vec<LiveGap> {
        self.live_gaps.lock().unwrap().clone()
    }

    /// None when the shift belongs to another entry or nothing is taping.
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

    pub fn queue_snapshot(&self) -> QueueSnapshot {
        self.queue.lock().unwrap().clone()
    }

    pub fn queue_rev(&self) -> u64 {
        self.queue_rev.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Entries after the one at pool index `audible`, and its position; falls
    /// back to the published cursor. Counted under the lock because the
    /// continuation trigger calls this every 16 ms and a snapshot clones every
    /// `Locator`. None while nothing is queued.
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

    /// Only a stalled reconnect reads it, so it's cheap to set on any command.
    pub fn interrupt(&self) {
        self.interrupt
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn device_lost(&self) -> bool {
        self.device_lost.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Progress through the audible crossfade and its direction. None until the
    /// speakers reach it.
    pub fn crossfade(&self) -> Option<(f32, bool)> {
        use std::sync::atomic::Ordering::{Acquire, Relaxed};
        // Length first, with acquire: it publishes the pair. The pair isn't atomic
        // together, so a re-publish can give one tick of wrong progress. Tolerated.
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

    /// None when nothing loops. The engine owns the loop (ADR 16).
    pub fn ab(&self) -> Option<(f64, f64)> {
        let a = self.ab_a.load(std::sync::atomic::Ordering::Acquire);
        if a == u64::MAX {
            return None;
        }
        let b = self.ab_b.load(std::sync::atomic::Ordering::Relaxed);
        Some((f64::from_bits(a), f64::from_bits(b)))
    }

    pub fn position(&self, device_rate: u32) -> Option<(usize, f64)> {
        let consumed = self
            .frames_consumed
            .load(std::sync::atomic::Ordering::Relaxed);
        self.position_at(consumed, device_rate)
    }

    /// Against a clock reading already taken, so a caller can line two things up.
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

    /// Matched on pool index, so a file queued twice resolves to the right one.
    #[test]
    fn upcoming_counts_from_the_audible_entry() {
        let shared = Shared::new(5);
        *shared.queue.lock().unwrap() = QueueSnapshot {
            entries: (0..5)
                .map(|i| QueueEntry {
                    id: i as u64,
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
        assert_eq!(shared.upcoming_from(Some(4)), Some((4, 0)));
        assert_eq!(shared.upcoming_from(None), Some((1, 3)));
        assert_eq!(shared.upcoming_from(Some(99)), Some((1, 3)));
        // None, not zero, or a dead session would read as dry and fire the trigger.
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
        push_segment(&shared, 0, 0, 0);
        shared.frames_consumed.store(48000, Ordering::Relaxed);
        assert_eq!(shared.position(48000), Some((0, 1.0)));
    }

    #[test]
    fn position_takes_newest_reached_segment() {
        let shared = Shared::new(3);
        push_segment(&shared, 0, 0, 0);
        push_segment(&shared, 96000, 1, 24000);
        push_segment(&shared, 200000, 2, 0);
        shared
            .frames_consumed
            .store(96000 + 48000, Ordering::Relaxed);
        assert_eq!(shared.position(48000), Some((1, 1.5)));
    }

    #[test]
    fn crossfade_reads_off_the_output_clock() {
        let shared = Shared::new(1);
        assert!(
            shared.crossfade().is_none(),
            "nothing published, nothing to show"
        );
        shared.fade_at.store(48_000, Ordering::Relaxed);
        shared.fade_back.store(true, Ordering::Relaxed);
        shared.fade_len.store(96_000, Ordering::Release);
        assert!(shared.crossfade().is_none());
        shared
            .frames_consumed
            .store(48_000 + 24_000, Ordering::Relaxed);
        let (progress, back) = shared.crossfade().expect("in the window");
        assert!((progress - 0.25).abs() < 1e-6);
        assert!(back, "the skip that started it went backwards");
        // Past the end it stops showing without anyone clearing it.
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
