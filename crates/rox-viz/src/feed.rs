//! The seam between playback and the audio views. The app drains the engine's
//! PCM tap on the UI thread into this; views copy windows back out. Neither
//! side is real-time, so a short mutex hold is fine; the RT boundary is the
//! tap ring in rox-playback.
//!
//! The newest spectrum is kept per window size, so every view at a size
//! shares one FFT.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::analysis::{Analyzer, MAX_FFT_SIZE, MIN_FFT_SIZE};

/// The largest FFT window with slack, in interleaved samples.
const KEEP_SAMPLES: usize = MAX_FFT_SIZE * 2 * 2;

pub struct AudioFeed {
    buf: Mutex<VecDeque<f32>>,
    sample_rate: AtomicU32,
    /// Lets a view tell silence (nothing new) from a repeat of the same window.
    written: AtomicU64,
    /// The audible queue entry, stamped by the player's pump, or [`NO_TRACK`].
    track: AtomicU64,
    spectra: Mutex<Vec<Spectrum>>,
}

const NO_TRACK: u64 = u64::MAX;

struct Spectrum {
    analyzer: Analyzer,
    mono: Vec<f32>,
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

    /// The audible queue entry, `None` between tracks. Stamped by the pump.
    pub fn set_track(&self, track: Option<u64>) {
        self.track
            .store(track.unwrap_or(NO_TRACK), Ordering::Relaxed);
    }

    pub fn track(&self) -> Option<u64> {
        let track = self.track.load(Ordering::Relaxed);
        (track != NO_TRACK).then_some(track)
    }

    /// Newest frames, mono-folded, newest last. Returns how many were copied.
    pub fn latest_mono(&self, out: &mut [f32]) -> usize {
        self.latest_mono_at(out).0
    }

    /// Also returns the `written` count, read under the same lock so it matches
    /// the window.
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

    /// Half-spectrum magnitudes of the newest `size` frames, `None` until that
    /// many are buffered. Computed once per size per feed advance and shared.
    /// A size [`Analyzer::new`] would reject answers `None` rather than panicking
    /// with the lock held.
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

    /// Newest frames split by channel, for stereo meters. Returns how many.
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

    /// Interleaved samples pushed since `cursor` (a `written()` value), and the
    /// new cursor. A slow reader gets a gap, never a repeat. For consumers that
    /// must see every sample once, like the Milkdrop worker.
    pub fn since(&self, cursor: u64, out: &mut Vec<f32>) -> u64 {
        out.clear();
        let buf = self.buf.lock().unwrap();
        let written = self.written.load(Ordering::Relaxed);
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
        assert_eq!(feed.written(), 6);
    }

    #[test]
    fn latest_mono_folds_stereo_pairs() {
        let feed = AudioFeed::new();
        feed.push(&[1.0, 3.0, 2.0, 4.0]);
        let mut out = [0.0f32; 2];
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, 2);
        assert_eq!(out, [2.0, 3.0]);
    }

    #[test]
    fn latest_mono_returns_newest_frames_last() {
        let feed = AudioFeed::new();
        feed.push(&[10.0, 10.0, 20.0, 20.0, 30.0, 30.0, 40.0, 40.0]);
        let mut out = [0.0f32; 2];
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, 2);
        assert_eq!(out, [30.0, 40.0]);
    }

    #[test]
    fn latest_stereo_splits_channels_newest_last() {
        let feed = AudioFeed::new();
        // Three frames, room for two: the two newest, split by channel.
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
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], 1.0);
    }

    #[test]
    fn ring_drops_oldest_past_capacity() {
        let feed = AudioFeed::new();
        // Overrun by one frame: the newest sample survives, the counter counts all.
        let total = KEEP_SAMPLES + 2;
        let samples: Vec<f32> = (0..total).map(|i| i as f32).collect();
        feed.push(&samples);
        assert_eq!(feed.written(), total as u64);

        let mut out = vec![0.0f32; 1];
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], total as f32 - 1.5);
    }

    #[test]
    fn ring_never_grows_past_capacity() {
        let feed = AudioFeed::new();
        for _ in 0..4 {
            let chunk = vec![0.5f32; KEEP_SAMPLES];
            feed.push(&chunk);
        }
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
        // A cursor older than the ring gets a gap starting at the oldest retained sample.
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

        let a = feed.magnitudes(1024).expect("a full window is buffered");
        let b = feed.magnitudes(1024).expect("a full window is buffered");
        assert!(Arc::ptr_eq(&a, &b), "same window, same spectrum");
        assert_eq!(a.len(), 512, "the lower half-spectrum");

        feed.push(&tone(16));
        let c = feed.magnitudes(1024).expect("still a full window");
        assert!(!Arc::ptr_eq(&a, &c), "a moved feed is a new spectrum");
    }

    #[test]
    fn magnitudes_match_a_private_analyzer() {
        let feed = AudioFeed::new();
        feed.push(&tone(4096));

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

        // Sizes the analyzer would assert on answer None and don't poison the lock.
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
