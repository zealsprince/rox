//! The seam between playback and the audio views. The app drains the
//! engine's PCM tap on the UI thread and pushes what it got here; the views
//! copy the most recent window back out for analysis. Neither side is
//! real-time, so a short mutex hold is fine - the RT boundary is the tap
//! ring itself, inside rox-playback.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::analysis::MAX_FFT_SIZE;

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
}

impl AudioFeed {
    pub fn new() -> Self {
        AudioFeed {
            buf: Mutex::new(VecDeque::with_capacity(KEEP_SAMPLES)),
            sample_rate: AtomicU32::new(48_000),
            written: AtomicU64::new(0),
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

    /// Copy the newest frames into `out`, mono-folded, newest last. Returns
    /// how many frames landed; short means not enough audio buffered yet.
    pub fn latest_mono(&self, out: &mut [f32]) -> usize {
        let buf = self.buf.lock().unwrap();
        let n = (buf.len() / 2).min(out.len());
        let start = buf.len() - n * 2;
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = (buf[start + i * 2] + buf[start + i * 2 + 1]) * 0.5;
        }
        n
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
    fn latest_mono_short_when_underfed() {
        let feed = AudioFeed::new();
        feed.push(&[1.0, 1.0]);
        let mut out = [0.0f32; 8];
        // Only one frame buffered, so only one lands even though out is longer.
        let n = feed.latest_mono(&mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], 1.0);
    }

    #[test]
    fn ring_drops_oldest_past_capacity() {
        let feed = AudioFeed::new();
        // Overrun the ring by one frame, then the newest sample must survive
        // and the write counter must reflect everything ever pushed.
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
    fn sample_rate_round_trips() {
        let feed = AudioFeed::new();
        assert_eq!(feed.sample_rate(), 48_000);
        feed.set_sample_rate(44_100);
        assert_eq!(feed.sample_rate(), 44_100);
    }
}
