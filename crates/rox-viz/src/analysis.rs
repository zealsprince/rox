//! Spectrum analysis for the audio views: Hann window, radix-2 FFT, normalized
//! magnitudes, and the log-spaced band mapping the bars use. Hand-rolled: an
//! FFT at these sizes at 60 Hz is nothing, and it keeps DSP dependencies out.

/// The ceiling matches what [`crate::AudioFeed`] keeps buffered.
pub const FFT_SIZE: usize = 4096;
pub const MIN_FFT_SIZE: usize = 512;
pub const MAX_FFT_SIZE: usize = 16384;

pub struct Analyzer {
    window: Vec<f32>,
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

    pub fn size(&self) -> usize {
        self.window.len()
    }

    /// Magnitudes of the lower half-spectrum of one mono frame, normalized so a
    /// full-scale sine reads near 1.0.
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

/// `bands` log-spaced bands across `lo_hz..hi_hz` as half-spectrum bin ranges.
/// Each is at least one bin wide, so neighbours share bins where the FFT is
/// too coarse.
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

/// The 1-2-5 ladder a log frequency axis is ruled on: (hz, position 0..1,
/// labelled). The other steps of each decade come back as minor marks.
pub fn hz_ladder(lo_hz: f32, hi_hz: f32) -> Vec<(f32, f32, bool)> {
    if lo_hz <= 0.0 || hi_hz <= lo_hz {
        return Vec::new();
    }
    let span = (hi_hz / lo_hz).ln();
    let mut marks = Vec::new();
    let mut decade = 10f32.powf(lo_hz.log10().floor());
    while decade <= hi_hz {
        for step in 1..=9u32 {
            let hz = decade * step as f32;
            if hz < lo_hz || hz > hi_hz {
                continue;
            }
            marks.push((hz, (hz / lo_hz).ln() / span, matches!(step, 1 | 2 | 5)));
        }
        decade *= 10.0;
    }
    marks
}

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two());

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

    fn bin_of(freq: f32, size: usize, rate: u32) -> usize {
        (freq * size as f32 / rate as f32).round() as usize
    }

    fn sine(buf: &mut [f32], freq: f32, rate: u32) {
        for (i, s) in buf.iter_mut().enumerate() {
            *s = (TAU * freq * i as f32 / rate as f32).sin();
        }
    }

    #[test]
    fn window_is_symmetric_with_zero_endpoints() {
        let a = Analyzer::new(1024);
        assert!(a.window[0].abs() < 1e-6);
        assert!(a.window[a.window.len() - 1].abs() < 1e-6);
        let mid = a.window[a.window.len() / 2];
        assert!(mid > 0.999, "hann midpoint should be ~1.0, got {mid}");
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
        // A frequency exactly on a bin keeps windowing leakage in the neighbours.
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
        assert!(
            (peak_ix as i32 - target_bin as i32).abs() <= 1,
            "peak at {peak_ix}, expected near {target_bin}"
        );
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

        let floor = mags[bin_a + 20];
        assert!(mags[bin_a] > floor * 10.0);
        assert!(mags[bin_b] > floor * 10.0);
    }

    #[test]
    fn fft_matches_naive_dft() {
        // Cross-check against a direct DFT so a butterfly bug can't hide.
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
            assert!(lo < hi, "band {lo}..{hi} is empty");
            assert!(lo >= 1);
            assert!(hi <= 2048);
        }
        for pair in bands.windows(2) {
            assert!(pair[0].0 <= pair[1].0, "band lows went backwards");
        }
    }

    #[test]
    fn log_bands_are_wider_toward_the_top() {
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

    #[test]
    fn hz_ladder_labels_the_one_two_five_steps() {
        let labelled: Vec<f32> = hz_ladder(20.0, 20_000.0)
            .into_iter()
            .filter(|(_, _, major)| *major)
            .map(|(hz, _, _)| hz)
            .collect();
        assert_eq!(
            labelled,
            vec![
                20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0
            ]
        );
    }

    #[test]
    fn hz_ladder_stays_inside_the_range_it_was_given() {
        // Ends that aren't ladder steps: the top mark falls short of the edge.
        let marks = hz_ladder(40.0, 16_000.0);
        let (first, last) = (marks[0], marks[marks.len() - 1]);
        assert_eq!(first.0, 40.0);
        assert!(first.1.abs() < 1e-6, "first mark should sit at 0");
        assert_eq!(last.0, 10_000.0);
        assert!(last.1 < 1.0);
        for pair in marks.windows(2) {
            assert!(pair[0].1 < pair[1].1, "fractions went backwards");
        }
    }

    #[test]
    fn hz_ladder_rejects_an_empty_or_inverted_range() {
        assert!(hz_ladder(1000.0, 100.0).is_empty());
        assert!(hz_ladder(0.0, 20_000.0).is_empty());
        assert!(hz_ladder(1000.0, 1000.0).is_empty());
    }
}
