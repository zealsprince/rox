//! Linear resampler from the track rate to the device rate, interleaved
//! stereo. Quality is spike-grade on purpose; the real engine gets a proper
//! windowed-sinc resampler (rubato) when the implementation doc is written.

pub struct Resampler {
    src_rate: u32,
    dst_rate: u32,
    /// Source frames advanced per output frame.
    step: f64,
    /// Fractional read position, measured in source frames where 0 is `prev`.
    pos: f64,
    /// Last source frame of the previous chunk, carried for interpolation
    /// across chunk boundaries.
    prev: [f32; 2],
    have_prev: bool,
}

impl Resampler {
    pub fn new(src_rate: u32, dst_rate: u32) -> Self {
        Resampler {
            src_rate,
            dst_rate,
            step: src_rate as f64 / dst_rate as f64,
            pos: 0.0,
            prev: [0.0; 2],
            have_prev: false,
        }
    }

    pub fn src_rate(&self) -> u32 {
        self.src_rate
    }

    /// Resample one interleaved stereo chunk, appending to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.src_rate == self.dst_rate {
            out.extend_from_slice(input);
            return;
        }

        let n_in = input.len() / 2;
        if n_in == 0 {
            return;
        }
        let carry = usize::from(self.have_prev);
        let total = n_in + carry;

        // Frame i of the virtual stream `[prev?] + input`.
        let get = |i: usize| -> [f32; 2] {
            if i < carry {
                self.prev
            } else {
                let j = (i - carry) * 2;
                [input[j], input[j + 1]]
            }
        };

        while (self.pos.floor() as usize) + 1 < total {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = get(i);
            let b = get(i + 1);
            out.push(a[0] + (b[0] - a[0]) * frac);
            out.push(a[1] + (b[1] - a[1]) * frac);
            self.pos += self.step;
        }

        // Carry the last frame; re-anchor pos so 0 means that frame again.
        self.prev = get(total - 1);
        self.have_prev = true;
        self.pos -= (total - 1) as f64;
    }

    /// Emit the carried final source frame at end of stream, then clear the
    /// carry. `process` always stops short of the last source frame (it becomes
    /// `prev` and is never output), so without this the final frame of a track
    /// is dropped and gapless boundaries lose a sample when src != device rate.
    /// There is no frame after `prev` to interpolate toward, so any remaining
    /// output positions before the next source frame resolve to `prev` itself.
    /// Idempotent: a second flush emits nothing.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if self.src_rate == self.dst_rate || !self.have_prev {
            self.have_prev = false;
            return;
        }
        // One or more output frames still fall on or before `prev` (pos < 1
        // means the reader hasn't reached the frame past it). With nothing
        // past prev to interpolate toward, each resolves to prev exactly.
        while self.pos < 1.0 {
            out.push(self.prev[0]);
            out.push(self.prev[1]);
            self.pos += self.step;
        }
        self.have_prev = false;
        self.pos = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A passthrough (src == dst) copies the input verbatim and flush adds
    /// nothing. Bit-exact, no interpolation path touched.
    #[test]
    fn passthrough_is_bit_exact() {
        let mut r = Resampler::new(48000, 48000);
        let input = vec![0.1, -0.2, 0.3, -0.4, 0.5, -0.6];
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);
        assert_eq!(out, input);
    }

    /// Flush is a no-op when the resampler never saw a frame, whatever the
    /// ratio, so a track that decoded nothing doesn't emit phantom samples.
    #[test]
    fn flush_without_input_emits_nothing() {
        let mut r = Resampler::new(44100, 48000);
        let mut out = Vec::new();
        r.flush(&mut out);
        assert!(out.is_empty());
    }

    /// Upsampling 2x: the interpolation loop stops one frame short (the last
    /// source frame becomes `prev`), so without flush the final frame is
    /// dropped. Flush must emit it. Input [f0, f1] at step 0.5 yields f0, the
    /// f0/f1 midpoint, then f1 from the flush.
    #[test]
    fn flush_emits_final_frame_on_upsample() {
        let mut r = Resampler::new(24000, 48000);
        // Two stereo frames: (0, 0) and (1, -1).
        let input = vec![0.0, 0.0, 1.0, -1.0];
        let mut without = Vec::new();
        r.process(&input, &mut without);
        // Before flush the last source frame is absent.
        let last = [without[without.len() - 2], without[without.len() - 1]];
        assert_ne!(last, [1.0, -1.0], "last frame should not be emitted yet");

        r.flush(&mut without);
        let last = [without[without.len() - 2], without[without.len() - 1]];
        assert_eq!(last, [1.0, -1.0], "flush must emit the final source frame");
    }

    /// A known upsample produces the expected number of output frames. Feeding
    /// N source frames at ratio dst/src and flushing lands close to N*ratio
    /// output frames, with the last frame present. Guards the frame count from
    /// drifting if the loop bounds change.
    #[test]
    fn upsample_frame_count_and_last_frame() {
        let mut r = Resampler::new(24000, 48000);
        // Ten source frames, left = index, right = -index.
        let mut input = Vec::new();
        for i in 0..10 {
            input.push(i as f32);
            input.push(-(i as f32));
        }
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.flush(&mut out);
        let out_frames = out.len() / 2;
        // 2x upsample of 10 frames is ~20 output frames; the boundary rules
        // put it within a frame or two either side.
        assert!(
            (18..=21).contains(&out_frames),
            "expected ~20 output frames, got {out_frames}"
        );
        // The last source frame (9, -9) must be the final output.
        assert_eq!(out[out.len() - 2], 9.0);
        assert_eq!(out[out.len() - 1], -9.0);
    }

    /// Downsampling drops frames by design (decimation), so the exact last
    /// source frame need not appear; what matters is flush is safe to call and
    /// leaves the resampler reusable.
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
}
