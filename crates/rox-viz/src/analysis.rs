//! Spectrum analysis shared by the audio views: Hann window, radix-2 FFT,
//! normalized magnitudes, and the log-spaced band mapping the spectrum bars
//! use. Hand-rolled for the same reason it always was: a 4096-point FFT at
//! 60 Hz is nothing, and it keeps the crate dependency-free until a real DSP
//! need justifies one.

pub const FFT_SIZE: usize = 4096;
pub const HALF: usize = FFT_SIZE / 2;

pub struct Analyzer {
    /// Hann coefficients, precomputed.
    window: [f32; FFT_SIZE],
    /// Sum of the window, for amplitude normalization.
    window_sum: f32,
    re: [f32; FFT_SIZE],
    im: [f32; FFT_SIZE],
    mags: [f32; HALF],
}

impl Analyzer {
    pub fn new() -> Self {
        let window: [f32; FFT_SIZE] = std::array::from_fn(|i| {
            let t = i as f32 / (FFT_SIZE - 1) as f32;
            0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
        });
        let window_sum = window.iter().sum();
        Analyzer {
            window,
            window_sum,
            re: [0.0; FFT_SIZE],
            im: [0.0; FFT_SIZE],
            mags: [0.0; HALF],
        }
    }

    /// Window one frame of mono samples (`FFT_SIZE` of them), transform it,
    /// and return the magnitudes of the lower half-spectrum, normalized so a
    /// full-scale sine lands near 1.0.
    pub fn magnitudes(&mut self, mono: &[f32]) -> &[f32; HALF] {
        debug_assert_eq!(mono.len(), FFT_SIZE);
        for ((re, &s), &w) in self.re.iter_mut().zip(mono).zip(&self.window) {
            *re = s * w;
        }
        self.im.fill(0.0);
        fft(&mut self.re, &mut self.im);
        for k in 0..HALF {
            let power = self.re[k] * self.re[k] + self.im[k] * self.im[k];
            self.mags[k] = power.sqrt() * 2.0 / self.window_sum;
        }
        &self.mags
    }
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Map `bands` log-spaced bands across `lo_hz..hi_hz` to half-spectrum bin
/// ranges at the given sample rate. Each range is at least one bin wide, so
/// neighbours share bins where the FFT is too coarse to split them.
pub fn log_bands(bands: usize, lo_hz: f32, hi_hz: f32, sample_rate: u32) -> Vec<(usize, usize)> {
    let nyquist = sample_rate as f32 / 2.0;
    let ratio = hi_hz / lo_hz;
    (0..bands)
        .map(|i| {
            let f0 = lo_hz * ratio.powf(i as f32 / bands as f32);
            let f1 = lo_hz * ratio.powf((i + 1) as f32 / bands as f32);
            let lo = ((f0 / nyquist * HALF as f32) as usize).clamp(1, HALF - 1);
            let hi = ((f1 / nyquist * HALF as f32) as usize).clamp(lo + 1, HALF);
            (lo, hi)
        })
        .collect()
}

/// In-place iterative radix-2 Cooley-Tukey. Length must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two());

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in start..start + len / 2 {
                let (er, ei) = (re[k], im[k]);
                let or = re[k + len / 2] * cr - im[k + len / 2] * ci;
                let oi = re[k + len / 2] * ci + im[k + len / 2] * cr;
                re[k] = er + or;
                im[k] = ei + oi;
                re[k + len / 2] = er - or;
                im[k + len / 2] = ei - oi;
                let next = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next;
            }
        }
        len <<= 1;
    }
}
