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
}
