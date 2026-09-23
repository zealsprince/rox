//! The seam between playback and the audio views. The app drains the
//! engine's PCM tap on the UI thread and pushes what it got here; the views
//! copy the most recent window back out for analysis. Neither side is
//! real-time, so a short mutex hold is fine. The RT boundary is the tap
//! ring itself, inside rox-playback.
//!
//! The spectrum of the newest window is kept here too, one per window size.
//! Every view that wants one (the spectrum, the spectrogram, the EQ's
//! analyzer, the signal hub) asks the feed rather than running its own FFT,
//! so views sharing a window size share the transform, and a second window
//! showing the same player costs nothing extra.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::analysis::{Analyzer, MAX_FFT_SIZE, MIN_FFT_SIZE};

/// Interleaved stereo samples kept for analysis: the largest FFT window
/// with slack. Older samples fall off the front.
const KEEP_SAMPLES: usize = MAX_FFT_SIZE * 2 * 2;

pub struct AudioFeed {
    /// Interleaved stereo, newest at the back.
    buf: Mutex<VecDeque<f32>>,
    /// Device rate of the samples, set by the app per playback session.
    sample_rate: AtomicU32,
    /// Total samples ever pushed. Lets a view tell silence (nothing new)
    /// from a repeat of the same window.
    written: AtomicU64,
    /// The queue entry that's audible, stamped by the player's pump, or
    /// [`NO_TRACK`]. Lets a reader of the feed see a song change without
    /// holding the player.
    track: AtomicU64,
    /// The newest window's spectrum per window size, see
    /// [`AudioFeed::magnitudes`]. At most one entry per power of two in the
    /// analyzer's range, so it never grows past a handful.
    spectra: Mutex<Vec<Spectrum>>,
}

/// [`AudioFeed::track`]'s empty value. Queue entry ids count up from zero,
/// so the top of the range is one no entry will reach.
const NO_TRACK: u64 = u64::MAX;

/// One window size's transform and the last spectrum it produced.
struct Spectrum {
    analyzer: Analyzer,
    mono: Vec<f32>,
    /// The `written` count the spectrum was taken at. A different count
    /// means the feed has moved and the spectrum is stale.
    written: u64,
    mags: Option<Arc<[f32]>>,
}

impl AudioFeed {
    pub fn new() -> Self {
        AudioFeed {
            buf: Mutex::new(VecDeque::with_capacity(KEEP_SAMPLES)),
            sample_rate: AtomicU32::new(48_000),
            written: AtomicU64::new(0),
            track: AtomicU64::new(NO_TRACK),
            spectra: Mutex::new(Vec::new()),
        }
    }

    /// Append interleaved stereo samples drained from the PCM tap.
    pub fn push(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let mut buf = self.buf.lock().unwrap();
        buf.extend(samples.iter().copied());
        let excess = buf.len().saturating_sub(KEEP_SAMPLES);
        buf.drain(..excess);
        self.written
            .fetch_add(samples.len() as u64, Ordering::Relaxed);
    }

    pub fn set_sample_rate(&self, rate: u32) {
        self.sample_rate.store(rate, Ordering::Relaxed);
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }

    /// Stamp the queue entry that's audible, `None` between tracks or with
    /// nothing loaded. The player's pump does this on the tick it drains the
    /// tap on.
    pub fn set_track(&self, track: Option<u64>) {
        self.track
            .store(track.unwrap_or(NO_TRACK), Ordering::Relaxed);
    }

    /// The queue entry the samples coming in belong to, as of the last pump
    /// tick.
    pub fn track(&self) -> Option<u64> {
        let track = self.track.load(Ordering::Relaxed);
        (track != NO_TRACK).then_some(track)
    }

    /// Copy the newest frames into `out`, mono-folded, newest last. Returns
    /// how many frames were copied; short means not enough audio buffered yet.
    pub fn latest_mono(&self, out: &mut [f32]) -> usize {
        self.latest_mono_at(out).0
    }

    /// [`latest_mono`](Self::latest_mono) plus the `written` count the window
    /// ends at. Read under the same lock as the copy, since `push` bumps the
    /// count before it lets go, so the two always describe the same window.
    fn latest_mono_at(&self, out: &mut [f32]) -> (usize, u64) {
        let buf = self.buf.lock().unwrap();
        let written = self.written.load(Ordering::Relaxed);
        let n = (buf.len() / 2).min(out.len());
        let start = buf.len() - n * 2;
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = (buf[start + i * 2] + buf[start + i * 2 + 1]) * 0.5;
        }
        (n, written)
    }

    /// The half-spectrum magnitudes of the newest `size` frames, mono-folded
    /// and Hann-windowed (see [`Analyzer::magnitudes`]). `None` until the
    /// feed holds that many frames.
    ///
    /// The transform runs once per window size per feed advance. The first
    /// view to ask after a push pays for it and every other view asking for
    /// the same size gets the same spectrum back, so ten panels at 4096 cost
    /// one FFT. A `size` [`Analyzer::new`] wouldn't take (not a power of two,
    /// or outside its range) also answers `None`, rather than panicking with
    /// the lock held and taking every other view's spectrum down with it.
    pub fn magnitudes(&self, size: usize) -> Option<Arc<[f32]>> {
        if !size.is_power_of_two() || !(MIN_FFT_SIZE..=MAX_FFT_SIZE).contains(&size) {
            return None;
        }

        let mut spectra = self.spectra.lock().unwrap();

        let ix = match spectra.iter().position(|s| s.analyzer.size() == size) {
            Some(ix) => ix,
            None => {
                spectra.push(Spectrum {
                    analyzer: Analyzer::new(size),
                    mono: vec![0.0; size],
                    written: u64::MAX,
                    mags: None,
                });
                spectra.len() - 1
            }
        };
        let spectrum = &mut spectra[ix];

        // Still the window the last spectrum was taken over.
        if spectrum.written == self.written()
            && let Some(mags) = &spectrum.mags
        {
            return Some(mags.clone());
        }

        let (n, written) = self.latest_mono_at(&mut spectrum.mono);
        if n < size {
            return None;
        }

        let mags: Arc<[f32]> = spectrum.analyzer.magnitudes(&spectrum.mono).into();
        spectrum.written = written;
        spectrum.mags = Some(mags.clone());
        Some(mags)
    }

    /// Copy the newest frames split into their two channels, newest last:
    /// what a stereo meter (the VU panel, a goniometer) needs instead of the
    /// mono fold. `left` and `right` fill to the same length; returns how
    /// many frames were copied, short when the ring hasn't buffered enough yet.
    pub fn latest_stereo(&self, left: &mut [f32], right: &mut [f32]) -> usize {
        let buf = self.buf.lock().unwrap();
        let n = (buf.len() / 2).min(left.len()).min(right.len());
        let start = buf.len() - n * 2;
        for i in 0..n {
            left[i] = buf[start + i * 2];
            right[i] = buf[start + i * 2 + 1];
        }
        n
    }

    /// Copy the interleaved samples pushed since `cursor` (a `written()`
    /// value), newest last, and return the new cursor. Samples older than the
    /// ring still holds are skipped, so a slow reader gets a gap, never a
    /// repeat. `out` is cleared first.
    ///
    /// This is the streaming counterpart to `latest_mono` and `latest_stereo`:
    /// those hand back a fixed analysis window every time they're asked, this
    /// hands back everything that arrived since the last ask. A consumer that
    /// has to see every sample once, like the Milkdrop worker feeding
    /// projectM's PCM buffer, wants this one.
    pub fn since(&self, cursor: u64, out: &mut Vec<f32>) -> u64 {
        out.clear();
        let buf = self.buf.lock().unwrap();
        let written = self.written.load(Ordering::Relaxed);
        // Everything before this fell off the front of the ring already.
        let oldest = written - buf.len() as u64;
        let start = cursor.max(oldest).min(written);
        let offset = (start - oldest) as usize;
        out.reserve(buf.len() - offset);
        out.extend(buf.iter().skip(offset).copied());
        written
    }
}

impl Default for AudioFeed {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_push_is_a_noop() {
        let feed = AudioFeed::new();
        feed.push(&[]);
        assert_eq!(feed.written(), 0);
        let mut out = [0.0f32; 4];
        assert_eq!(feed.latest_mono(&mut out), 0);
    }

    #[test]
    fn written_counts_every_sample_pushed() {
        let feed = AudioFeed::new();
        feed.push(&[0.0, 0.0]);
        feed.push(&[0.0, 0.0, 0.0, 0.0]);
        // Counts interleaved samples, not frames.
        assert_eq!(feed.written(), 6);
    }

    #[test]
    fn latest_mono_folds_stereo_pairs() {
        let feed = AudioFeed::new();
        // Two frames: (L, R) = (1, 3) and (2, 4). Mono fold is the average.
        feed.push(&[1.0, 3.0, 2.0, 4.0]);
        let mut out = [0.0f32; 2];
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, 2);
        assert_eq!(out, [2.0, 3.0]);
    }

    #[test]
    fn latest_mono_returns_newest_frames_last() {
        let feed = AudioFeed::new();
        // Four frames, out buffer only fits two: the two newest, in order.
        feed.push(&[10.0, 10.0, 20.0, 20.0, 30.0, 30.0, 40.0, 40.0]);
        let mut out = [0.0f32; 2];
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, 2);
        assert_eq!(out, [30.0, 40.0]);
    }

    #[test]
    fn latest_stereo_splits_channels_newest_last() {
        let feed = AudioFeed::new();
        // Three frames: (L, R) = (1, 2), (3, 4), (5, 6). The out buffers only
        // fit two, so only the two newest are copied out, in order, split by
        // channel.
        feed.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let mut left = [0.0f32; 2];
        let mut right = [0.0f32; 2];
        let n = feed.latest_stereo(&mut left, &mut right);
        assert_eq!(n, 2);
        assert_eq!(left, [3.0, 5.0]);
        assert_eq!(right, [4.0, 6.0]);
    }

    #[test]
    fn latest_mono_short_when_underfed() {
        let feed = AudioFeed::new();
        feed.push(&[1.0, 1.0]);
        let mut out = [0.0f32; 8];
        // Only one frame buffered, so only one is copied out even though out
        // is longer.
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], 1.0);
    }

    #[test]
    fn ring_drops_oldest_past_capacity() {
        let feed = AudioFeed::new();
        // Overrun the ring by one frame, then the newest sample must still be
        // there and the write counter must reflect everything ever pushed.
        let total = KEEP_SAMPLES + 2;
        let samples: Vec<f32> = (0..total).map(|i| i as f32).collect();
        feed.push(&samples);
        assert_eq!(feed.written(), total as u64);

        let mut out = vec![0.0f32; 1];
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, 1);
        // Newest frame is (total-2, total-1); their average is total-1.5.
        assert_eq!(out[0], total as f32 - 1.5);
    }

    #[test]
    fn ring_never_grows_past_capacity() {
        let feed = AudioFeed::new();
        for _ in 0..4 {
            let chunk = vec![0.5f32; KEEP_SAMPLES];
            feed.push(&chunk);
        }
        // Never more frames retrievable than half the kept sample budget.
        let mut out = vec![0.0f32; KEEP_SAMPLES];
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, KEEP_SAMPLES / 2);
    }

    #[test]
    fn since_zero_returns_everything_retained() {
        let feed = AudioFeed::new();
        feed.push(&[1.0, 2.0, 3.0, 4.0]);
        let mut out = Vec::new();
        let cursor = feed.since(0, &mut out);
        assert_eq!(cursor, 4);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn since_the_current_cursor_returns_nothing() {
        let feed = AudioFeed::new();
        feed.push(&[1.0, 2.0, 3.0, 4.0]);
        let mut out = vec![9.0f32; 3];
        let cursor = feed.since(feed.written(), &mut out);
        assert_eq!(cursor, 4);
        // Cleared even when there's nothing new to put in it.
        assert!(out.is_empty());
    }

    #[test]
    fn since_picks_up_only_what_arrived_after_the_cursor() {
        let feed = AudioFeed::new();
        feed.push(&[1.0, 2.0]);
        let mut out = Vec::new();
        let cursor = feed.since(0, &mut out);
        feed.push(&[3.0, 4.0, 5.0, 6.0]);
        let cursor = feed.since(cursor, &mut out);
        assert_eq!(cursor, 6);
        assert_eq!(out, vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn since_skips_the_gap_when_the_reader_fell_behind() {
        let feed = AudioFeed::new();
        // Push more than the ring keeps, so a cursor of 0 is older than
        // anything still retained. The reader gets a gap, and the first
        // sample it sees is the oldest retained one rather than a repeat of
        // what already fell off the front.
        let total = KEEP_SAMPLES + 8;
        let samples: Vec<f32> = (0..total).map(|i| i as f32).collect();
        feed.push(&samples);

        let mut out = Vec::new();
        let cursor = feed.since(0, &mut out);
        assert_eq!(cursor, total as u64);
        assert_eq!(out.len(), KEEP_SAMPLES);
        assert_eq!(out[0], (total - KEEP_SAMPLES) as f32);
        assert_eq!(out[out.len() - 1], (total - 1) as f32);
    }

    #[test]
    fn sample_rate_round_trips() {
        let feed = AudioFeed::new();
        assert_eq!(feed.sample_rate(), 48_000);
        feed.set_sample_rate(44_100);
        assert_eq!(feed.sample_rate(), 44_100);
    }

    /// A 1 kHz tone at 48 kHz, `frames` stereo frames of it.
    fn tone(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect()
    }

    #[test]
    fn magnitudes_are_shared_until_the_feed_moves() {
        let feed = AudioFeed::new();
        feed.push(&tone(2048));

        // Two views at the same size get the one spectrum, not two
        // transforms of the same window.
        let a = feed.magnitudes(1024).expect("a full window is buffered");
        let b = feed.magnitudes(1024).expect("a full window is buffered");
        assert!(Arc::ptr_eq(&a, &b), "same window, same spectrum");
        assert_eq!(a.len(), 512, "the lower half-spectrum");

        // A push moves the window, and the next ask transforms it fresh.
        feed.push(&tone(16));
        let c = feed.magnitudes(1024).expect("still a full window");
        assert!(!Arc::ptr_eq(&a, &c), "a moved feed is a new spectrum");
    }

    #[test]
    fn magnitudes_match_a_private_analyzer() {
        let feed = AudioFeed::new();
        feed.push(&tone(4096));

        // What each view used to compute for itself, bin for bin.
        let mut mono = vec![0.0f32; 2048];
        assert_eq!(feed.latest_mono(&mut mono), 2048);
        let mut analyzer = Analyzer::new(2048);
        let own = analyzer.magnitudes(&mono).to_vec();

        let shared = feed.magnitudes(2048).expect("a full window is buffered");
        assert_eq!(&shared[..], &own[..]);
    }

    #[test]
    fn magnitudes_wait_for_a_full_window_and_refuse_bad_sizes() {
        let feed = AudioFeed::new();
        feed.push(&tone(100));
        assert!(feed.magnitudes(512).is_none(), "100 frames isn't a window");

        // Sizes the analyzer would assert on answer None instead, so a bad
        // size from one view can't poison the lock for the rest.
        feed.push(&tone(4096));
        assert!(feed.magnitudes(1000).is_none(), "not a power of two");
        assert!(feed.magnitudes(256).is_none(), "under the range");
        assert!(feed.magnitudes(512).is_some(), "the lock still works");
    }

    #[test]
    fn track_round_trips_and_clears() {
        let feed = AudioFeed::new();
        assert_eq!(feed.track(), None);
        feed.set_track(Some(0));
        assert_eq!(feed.track(), Some(0), "entry 0 is a real entry");
        feed.set_track(None);
        assert_eq!(feed.track(), None);
    }
}
