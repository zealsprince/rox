//! Windowed-sinc resampler from the track rate to the device rate, interleaved
//! stereo, over rubato. Runs on the decode thread (ADR 19), never the callback.
//!
//! Three properties the engine relies on:
//! - Same-rate is a bit-exact passthrough, which keeps ADR 19's bypass rule.
//! - The frame count is exact: N input frames give `round(N * dst / src)` out
//!   once flushed, in integer arithmetic. A resampler that ran short would
//!   shift every gapless seam by a filter length.
//! - Flush is idempotent and resets.
//!
//! The exact count takes two corrections: the first `output_delay()` frames
//! (group delay) are dropped, and flush pushes silence until the tail is out,
//! then truncates the surplus.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Indexing, Resampler as RubatoResampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};

/// Input frames per rubato call. Fixed input, variable output.
const CHUNK: usize = 1024;

/// 128 taps at Blackman-Harris is past transparent for 44.1-to-48.
const SINC_LEN: usize = 128;

/// Cap on silence chunks in a flush, so a stalled rubato can't spin the decode thread.
const FLUSH_CHUNK_LIMIT: usize = 64;

pub struct Resampler {
    src_rate: u32,
    dst_rate: u32,
    inner: Inner,
}

enum Inner {
    /// Rates match, or the filter couldn't be built.
    Passthrough,
    Sinc(Box<Sinc>),
}

struct Sinc {
    rs: Async<f32>,
    /// Input held back short of a chunk. Never over `CHUNK` frames, so it never reallocates.
    chunk: Vec<f32>,
    scratch: Vec<f32>,
    consumed: u64,
    emitted: u64,
    /// Group delay still to drop off the front.
    skip: usize,
}

impl Resampler {
    pub fn new(src_rate: u32, dst_rate: u32) -> Self {
        let inner = if src_rate == dst_rate || src_rate == 0 || dst_rate == 0 {
            Inner::Passthrough
        } else {
            Self::build(src_rate, dst_rate)
        };
        Resampler {
            src_rate,
            dst_rate,
            inner,
        }
    }

    fn build(src_rate: u32, dst_rate: u32) -> Inner {
        let params = SincInterpolationParameters {
            sinc_len: SINC_LEN,
            // Rubato picks the highest cutoff that keeps aliasing under the sidelobes.
            f_cutoff: None,
            oversampling_factor: 256,
            interpolation: SincInterpolationType::Cubic,
            window: WindowFunction::BlackmanHarris2,
        };
        // Relative ratio fixed at 1.0: a rate change builds a new resampler.
        let rs = match Async::<f32>::new_sinc(
            f64::from(dst_rate) / f64::from(src_rate),
            1.0,
            &params,
            CHUNK,
            2,
            FixedAsync::Input,
        ) {
            Ok(rs) => rs,
            Err(e) => {
                // Only on a nonsense rate. Wrong-speed audio beats panicking on the decode thread.
                log::error!("resampler {src_rate} -> {dst_rate} could not be built: {e}");
                return Inner::Passthrough;
            }
        };
        let skip = rs.output_delay();
        let scratch = vec![0.0; rs.output_frames_max() * 2];
        Inner::Sinc(Box::new(Sinc {
            rs,
            chunk: Vec::with_capacity(CHUNK * 2),
            scratch,
            consumed: 0,
            emitted: 0,
            skip,
        }))
    }

    pub fn src_rate(&self) -> u32 {
        self.src_rate
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let Inner::Sinc(s) = &mut self.inner else {
            out.extend_from_slice(input);
            return;
        };

        let n_in = input.len() / 2;
        if n_in == 0 {
            return;
        }
        s.consumed += n_in as u64;

        // Whole chunks straight off the caller's slice; only ragged ends touch `chunk`.
        let mut pos = 0;
        loop {
            let held = s.chunk.len() / 2;
            if held + (n_in - pos) < CHUNK {
                break;
            }
            if held == 0 {
                let end = pos + CHUNK;
                s.feed(&input[pos * 2..end * 2], None, out);
                pos = end;
            } else {
                let take = CHUNK - held;
                s.chunk.extend_from_slice(&input[pos * 2..(pos + take) * 2]);
                pos += take;
                let full = std::mem::take(&mut s.chunk);
                s.feed(&full, None, out);
                s.chunk = full;
                s.chunk.clear();
            }
        }
        s.chunk.extend_from_slice(&input[pos * 2..]);
    }

    /// End of stream: push the tail out, land on the exact frame count, and
    /// reset. Idempotent. Without it every gapless boundary drifts by a filter length.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let Inner::Sinc(s) = &mut self.inner else {
            return;
        };
        if s.consumed == 0 {
            s.chunk.clear();
            return;
        }

        // Round half up in integers: 44100 frames at 44.1k into 48k is 48000.
        let src = u128::from(self.src_rate);
        let target = ((u128::from(s.consumed) * u128::from(self.dst_rate) + src / 2) / src) as u64;
        let start = out.len();

        let tail = std::mem::take(&mut s.chunk);
        s.feed(&tail, Some(tail.len() / 2), out);
        s.chunk = tail;

        // `chunk` already has CHUNK frames of capacity, so zeroing it doesn't allocate.
        s.chunk.clear();
        s.chunk.resize(CHUNK * 2, 0.0);
        let zeros = std::mem::take(&mut s.chunk);
        let mut pushed = 0;
        while s.emitted < target && pushed < FLUSH_CHUNK_LIMIT {
            s.feed(&zeros, None, out);
            pushed += 1;
        }
        if s.emitted < target {
            log::warn!(
                "resampler flush stalled at {} of {target} frames",
                s.emitted
            );
        }
        s.chunk = zeros;
        s.chunk.clear();

        // Trim the overshoot, never past what this flush appended.
        if s.emitted > target {
            let surplus = (((s.emitted - target) as usize) * 2).min(out.len() - start);
            out.truncate(out.len() - surplus);
        }

        s.reset_state();
    }

    /// Re-arm for a seek at the same rates: clears history, keeps the sinc table.
    pub fn reset(&mut self) {
        if let Inner::Sinc(s) = &mut self.inner {
            s.reset_state();
        }
    }
}

impl Sinc {
    /// `partial` is the count of real frames in a short chunk; rubato pads the rest.
    fn feed(&mut self, input: &[f32], partial: Option<usize>, out: &mut Vec<f32>) {
        let Sinc {
            rs,
            scratch,
            emitted,
            skip,
            ..
        } = self;

        let need = rs.output_frames_next();
        if scratch.len() < need * 2 {
            scratch.resize(need * 2, 0.0);
        }
        let frames_in = partial.unwrap_or(CHUNK);

        let src = match InterleavedSlice::new(input, 2, frames_in) {
            Ok(src) => src,
            Err(e) => {
                log::error!("resampler input buffer rejected: {e}");
                return;
            }
        };
        let mut dst = match InterleavedSlice::new_mut(scratch.as_mut_slice(), 2, need) {
            Ok(dst) => dst,
            Err(e) => {
                log::error!("resampler output buffer rejected: {e}");
                return;
            }
        };
        let indexing = partial.map(|n| Indexing::new().partial_len(n));
        let written = match rs.process_into_buffer(&src, &mut dst, indexing.as_ref()) {
            Ok((_, written)) => written,
            Err(e) => {
                log::error!("resample failed: {e}");
                return;
            }
        };

        // Drop the group delay so output lines up with input.
        let dropped = (*skip).min(written);
        *skip -= dropped;
        out.extend_from_slice(&scratch[dropped * 2..written * 2]);
        *emitted += (written - dropped) as u64;
    }

    fn reset_state(&mut self) {
        self.rs.reset();
        self.chunk.clear();
        self.consumed = 0;
        self.emitted = 0;
        self.skip = self.rs.output_delay();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, freq: f64, rate: f64) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let s = (std::f64::consts::TAU * freq * n as f64 / rate).sin() as f32;
            v.push(s);
            v.push(-s);
        }
        v
    }

    /// Fixed seed, so a failing chunking is reproducible.
    struct Lcg(u64);

    impl Lcg {
        fn upto(&mut self, hi: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as usize % hi + 1
        }
    }

    #[test]
    fn passthrough_is_bit_exact() {
        let mut r = Resampler::new(48000, 48000);
        let input = vec![0.1, -0.2, 0.3, -0.4, 0.5, -0.6];
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn flush_without_input_emits_nothing() {
        let mut r = Resampler::new(44100, 48000);
        let mut out = Vec::new();
        r.flush(&mut out);
        assert!(out.is_empty());
    }

    /// Before flush the 2x upsample is short by the filter tail; flush makes up
    /// the exact count.
    #[test]
    fn flush_emits_final_frame_on_upsample() {
        let mut r = Resampler::new(24000, 48000);
        let input = vec![0.0, 0.0, 1.0, -1.0];
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert!(
            out.len() / 2 < 4,
            "the filter tail should still be held before flush, got {} frames",
            out.len() / 2
        );

        r.flush(&mut out);
        assert_eq!(out.len() / 2, 4, "flush must land on the exact frame count");
    }

    /// round(N * ratio) is what the gapless boundary is measured against.
    #[test]
    fn upsample_frame_count_is_exact() {
        let mut r = Resampler::new(24000, 48000);
        let mut input = Vec::new();
        for i in 0..10 {
            input.push(i as f32);
            input.push(-(i as f32));
        }
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);
        assert_eq!(out.len() / 2, 20);
    }

    #[test]
    fn flush_is_idempotent() {
        let mut r = Resampler::new(24000, 48000);
        let input = vec![0.0, 0.0, 1.0, 1.0];
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);
        let after_first = out.len();
        r.flush(&mut out);
        assert_eq!(out.len(), after_first, "second flush emits nothing");
    }

    /// Exact at 44.1-to-48 over a real-length run, however the input is chopped.
    #[test]
    fn ratio_is_exact_over_a_long_run() {
        let input = sine(44100, 1000.0, 44100.0);
        let mut r = Resampler::new(44100, 48000);
        let mut out = Vec::new();
        let mut rng = Lcg(0x5EED);
        let mut pos = 0;
        while pos < 44100 {
            let n = rng.upto(4096).min(44100 - pos);
            r.process(&input[pos * 2..(pos + n) * 2], &mut out);
            pos += n;
        }
        r.flush(&mut out);
        assert_eq!(out.len() / 2, 48000);
    }

    #[test]
    fn frequency_and_amplitude_survive() {
        let input = sine(44100, 1000.0, 44100.0);
        let mut r = Resampler::new(44100, 48000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);

        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!(
            (1980..=2020).contains(&crossings),
            "expected ~2000 zero crossings, got {crossings}"
        );

        let peak_in = input.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let peak_out = left.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            (peak_out - peak_in).abs() / peak_in < 0.02,
            "peak moved from {peak_in} to {peak_out}"
        );
    }

    /// Chunking changes nothing, bit for bit: rubato sees the same full chunks either way.
    #[test]
    fn chunking_does_not_change_the_output() {
        let input = sine(3000, 440.0, 44100.0);

        let mut whole = Vec::new();
        let mut r = Resampler::new(44100, 48000);
        r.process(&input, &mut whole);
        r.flush(&mut whole);

        let mut dripped = Vec::new();
        let mut r = Resampler::new(44100, 48000);
        for frame in input.chunks(2) {
            r.process(frame, &mut dripped);
        }
        r.flush(&mut dripped);

        assert_eq!(whole, dripped);
    }

    #[test]
    fn downsample_frame_count_is_exact() {
        let input = sine(9600, 1000.0, 96000.0);
        let mut r = Resampler::new(96000, 48000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);
        assert_eq!(out.len() / 2, 4800);
    }

    #[test]
    fn reset_matches_a_fresh_resampler() {
        let input = sine(2000, 440.0, 44100.0);

        let mut reused = Resampler::new(44100, 48000);
        let mut scratch = Vec::new();
        reused.process(&input, &mut scratch);
        reused.flush(&mut scratch);
        reused.reset();

        let mut after_reset = Vec::new();
        reused.process(&input, &mut after_reset);
        reused.flush(&mut after_reset);

        let mut fresh = Resampler::new(44100, 48000);
        let mut expected = Vec::new();
        fresh.process(&input, &mut expected);
        fresh.flush(&mut expected);

        assert_eq!(after_reset, expected);
    }
}
