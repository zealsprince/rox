//! The seam between playback and the audio views. The app drains the
//! engine's PCM tap on the UI thread and pushes what it got here; the views
//! copy the most recent window back out for analysis. Neither side is
//! real-time, so a short mutex hold is fine - the RT boundary is the tap
//! ring itself, inside rox-playback.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::analysis::FFT_SIZE;

/// Interleaved stereo samples kept for analysis: one FFT window with slack.
/// Older samples fall off the front.
const KEEP_SAMPLES: usize = FFT_SIZE * 2 * 2;

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
        self.written.fetch_add(samples.len() as u64, Ordering::Relaxed);
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
