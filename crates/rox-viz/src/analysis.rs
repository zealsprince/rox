//! Spectrum analysis shared by the audio views: Hann window, radix-2 FFT,
//! normalized magnitudes, and the log-spaced band mapping the spectrum bars
//! use. The window size is picked per analyzer - short windows react fast,
//! long ones resolve fine - between [`MIN_FFT_SIZE`] and [`MAX_FFT_SIZE`],
//! with [`FFT_SIZE`] the default. Hand-rolled for the same reason it always
//! was: an FFT at these sizes at 60 Hz is nothing, and it keeps the crate
//! dependency-free until a real DSP need justifies one.

/// The default window size, and the bounds a caller may size an analyzer
/// between. The ceiling is what [`crate::AudioFeed`] keeps buffered.
pub const FFT_SIZE: usize = 4096;
pub const MIN_FFT_SIZE: usize = 512;
pub const MAX_FFT_SIZE: usize = 16384;

pub struct Analyzer {
    /// Hann coefficients, precomputed; their length is the window size.
    window: Vec<f32>,
    /// Sum of the window, for amplitude normalization.
    window_sum: f32,
    re: Vec<f32>,
    im: Vec<f32>,
    mags: Vec<f32>,
}

impl Analyzer {
    pub fn new(size: usize) -> Self {
        assert!(
            size.is_power_of_two() && (MIN_FFT_SIZE..=MAX_FFT_SIZE).contains(&size),
            "analyzer size must be a power of two in {MIN_FFT_SIZE}..={MAX_FFT_SIZE}"
        );
        let window: Vec<f32> = (0..size)
            .map(|i| {
                let t = i as f32 / (size - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
            })
            .collect();
        let window_sum = window.iter().sum();
        Analyzer {
            window,
            window_sum,
            re: vec![0.0; size],
            im: vec![0.0; size],
            mags: vec![0.0; size / 2],
        }
    }

    /// The window size this analyzer transforms.
    pub fn size(&self) -> usize {
        self.window.len()
    }

    /// Window one frame of mono samples ([`Self::size`] of them), transform
    /// it, and return the magnitudes of the lower half-spectrum, normalized
    /// so a full-scale sine lands near 1.0.
    pub fn magnitudes(&mut self, mono: &[f32]) -> &[f32] {
        debug_assert_eq!(mono.len(), self.size());
        for ((re, &s), &w) in self.re.iter_mut().zip(mono).zip(&self.window) {
            *re = s * w;
        }
        self.im.fill(0.0);
        fft(&mut self.re, &mut self.im);
        for (k, mag) in self.mags.iter_mut().enumerate() {
            let power = self.re[k] * self.re[k] + self.im[k] * self.im[k];
            *mag = power.sqrt() * 2.0 / self.window_sum;
        }
        &self.mags
    }
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new(FFT_SIZE)
    }
}

/// Map `bands` log-spaced bands across `lo_hz..hi_hz` to half-spectrum bin
/// ranges at the given sample rate, for an analyzer with `half` output bins.
/// Each range is at least one bin wide, so neighbours share bins where the
/// FFT is too coarse to split them.
pub fn log_bands(
    bands: usize,
    lo_hz: f32,
    hi_hz: f32,
    sample_rate: u32,
    half: usize,
) -> Vec<(usize, usize)> {
    let nyquist = sample_rate as f32 / 2.0;
    let ratio = hi_hz / lo_hz;
    (0..bands)
        .map(|i| {
            let f0 = lo_hz * ratio.powf(i as f32 / bands as f32);
            let f1 = lo_hz * ratio.powf((i + 1) as f32 / bands as f32);
            let lo = ((f0 / nyquist * half as f32) as usize).clamp(1, half - 1);
            let hi = ((f1 / nyquist * half as f32) as usize).clamp(lo + 1, half);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    // The bin a real frequency lands in for a given window and rate.
    fn bin_of(freq: f32, size: usize, rate: u32) -> usize {
        (freq * size as f32 / rate as f32).round() as usize
    }

    // Fill `buf` with a full-scale sine at `freq` Hz, sampled at `rate`.
    fn sine(buf: &mut [f32], freq: f32, rate: u32) {
        for (i, s) in buf.iter_mut().enumerate() {
            *s = (TAU * freq * i as f32 / rate as f32).sin();
        }
    }

    #[test]
    fn window_is_symmetric_with_zero_endpoints() {
        let a = Analyzer::new(1024);
        // Hann starts and ends at zero.
        assert!(a.window[0].abs() < 1e-6);
        assert!(a.window[a.window.len() - 1].abs() < 1e-6);
        // Peak sits at the middle and reaches ~1.0.
        let mid = a.window[a.window.len() / 2];
        assert!(mid > 0.999, "hann midpoint should be ~1.0, got {mid}");
        // Symmetric about the center.
        let n = a.window.len();
        for i in 0..n / 2 {
            assert!((a.window[i] - a.window[n - 1 - i]).abs() < 1e-5);
        }
    }

    #[test]
    fn window_sum_matches_coefficients() {
        let a = Analyzer::new(512);
        let sum: f32 = a.window.iter().sum();
        assert!((a.window_sum - sum).abs() < 1e-3);
        // Hann's mean is ~0.5, so the sum is ~half the window length.
        assert!((a.window_sum - 256.0).abs() < 1.0);
    }

    #[test]
    fn size_and_default_bounds() {
        assert_eq!(Analyzer::new(512).size(), 512);
        assert_eq!(Analyzer::default().size(), FFT_SIZE);
    }

    #[test]
    #[should_panic]
    fn rejects_non_power_of_two() {
        Analyzer::new(1000);
    }

    #[test]
    #[should_panic]
    fn rejects_out_of_range_size() {
        // 256 is a power of two but below MIN_FFT_SIZE.
        Analyzer::new(256);
    }

    #[test]
    fn silence_produces_near_zero_magnitudes() {
        let mut a = Analyzer::new(1024);
        let input = vec![0.0f32; 1024];
        let mags = a.magnitudes(&input);
        assert_eq!(mags.len(), 512);
        for &m in mags {
            assert!(m < 1e-6, "silence should stay near zero, got {m}");
        }
    }

    #[test]
    fn dc_input_lands_in_bin_zero() {
        let mut a = Analyzer::new(1024);
        let input = vec![1.0f32; 1024];
        let mags = a.magnitudes(&input);
        // A constant is all DC: bin 0 carries the energy, the rest is noise.
        let max_ix = mags
            .iter()
            .enumerate()
            .max_by(|(_, x), (_, y)| x.partial_cmp(y).unwrap())
            .unwrap()
            .0;
        assert_eq!(max_ix, 0);
    }

    #[test]
    fn sine_peaks_in_its_own_bin() {
        let rate = 48_000;
        let size = 4096;
        // Pick a frequency that lands exactly on a bin so windowing leakage
        // stays in the neighbours, not smeared across the spectrum.
        let target_bin = 100;
        let freq = target_bin as f32 * rate as f32 / size as f32;
        assert_eq!(bin_of(freq, size, rate), target_bin);

        let mut input = vec![0.0f32; size];
        sine(&mut input, freq, rate);

        let mut a = Analyzer::new(size);
        let mags = a.magnitudes(&input);

        let peak_ix = mags
            .iter()
            .enumerate()
            .max_by(|(_, x), (_, y)| x.partial_cmp(y).unwrap())
            .unwrap()
            .0;
        // Allow one bin of slop from windowing.
        assert!(
            (peak_ix as i32 - target_bin as i32).abs() <= 1,
            "peak at {peak_ix}, expected near {target_bin}"
        );
        // A full-scale sine normalizes to near 1.0 at its bin.
        assert!(
            (0.9..=1.1).contains(&mags[peak_ix]),
            "full-scale sine should normalize near 1.0, got {}",
            mags[peak_ix]
        );
    }

    #[test]
    fn two_tones_light_two_bins() {
        let rate = 48_000;
        let size = 4096;
        let bin_a = 50;
        let bin_b = 400;
        let fa = bin_a as f32 * rate as f32 / size as f32;
        let fb = bin_b as f32 * rate as f32 / size as f32;

        let mut input = vec![0.0f32; size];
        for (i, s) in input.iter_mut().enumerate() {
            *s = 0.5 * (TAU * fa * i as f32 / rate as f32).sin()
                + 0.5 * (TAU * fb * i as f32 / rate as f32).sin();
        }

        let mut a = Analyzer::new(size);
        let mags = a.magnitudes(&input);

        // Both target bins should stand well above the noise floor between them.
        let floor = mags[bin_a + 20];
        assert!(mags[bin_a] > floor * 10.0);
        assert!(mags[bin_b] > floor * 10.0);
    }

    #[test]
    fn fft_matches_naive_dft() {
        // Cross-check the hand-rolled radix-2 against a direct DFT on a small
        // arbitrary signal, so a subtle butterfly bug can't hide.
        let n = 8;
        let signal: Vec<f32> = (0..n).map(|i| (i as f32 * 0.7).sin() + 0.3).collect();

        let mut re = signal.clone();
        let mut im = vec![0.0f32; n];
        fft(&mut re, &mut im);

        for k in 0..n {
            let mut dr = 0.0f32;
            let mut di = 0.0f32;
            for (t, &x) in signal.iter().enumerate() {
                let ang = -TAU * k as f32 * t as f32 / n as f32;
                dr += x * ang.cos();
                di += x * ang.sin();
            }
            assert!((re[k] - dr).abs() < 1e-3, "re[{k}]: {} vs {dr}", re[k]);
            assert!((im[k] - di).abs() < 1e-3, "im[{k}]: {} vs {di}", im[k]);
        }
    }

    #[test]
    fn log_bands_count_and_monotonic_edges() {
        let bands = log_bands(24, 40.0, 16_000.0, 48_000, 2048);
        assert_eq!(bands.len(), 24);
        for &(lo, hi) in &bands {
            // Every band is at least one bin wide and stays in range.
            assert!(lo < hi, "band {lo}..{hi} is empty");
            assert!(lo >= 1);
            assert!(hi <= 2048);
        }
        // Lows are monotonically non-decreasing across bands.
        for pair in bands.windows(2) {
            assert!(pair[0].0 <= pair[1].0, "band lows went backwards");
        }
    }

    #[test]
    fn log_bands_are_wider_toward_the_top() {
        // Log spacing means high bands span more bins than low ones.
        let bands = log_bands(16, 40.0, 20_000.0, 48_000, 4096);
        let low_width = bands[0].1 - bands[0].0;
        let high_width = bands[bands.len() - 1].1 - bands[bands.len() - 1].0;
        assert!(
            high_width > low_width,
            "top band ({high_width}) should be wider than bottom ({low_width})"
        );
    }

    #[test]
    fn log_bands_no_panic_on_edge_counts() {
        // A single band, and a tiny half-spectrum, must not panic or produce
        // an inverted range.
        let one = log_bands(1, 40.0, 16_000.0, 48_000, 256);
        assert_eq!(one.len(), 1);
        assert!(one[0].0 < one[0].1);

        let many = log_bands(64, 20.0, 20_000.0, 44_100, 8);
        assert_eq!(many.len(), 64);
        for &(lo, hi) in &many {
            assert!(lo < hi);
            assert!(hi <= 8);
        }
    }
}
