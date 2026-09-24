//! Band-limited sample rate conversion for the model front end.
//!
//! Going from 44.1 kHz to a model's 32 kHz, anything above the new Nyquist
//! must be filtered, or it folds down into the band the network reads (19 kHz
//! lands at 13 kHz) and every embedding skews by the file's source rate. So
//! this is a windowed sinc: low-pass and resample in one kernel.
//!
//! Not `rox_playback::resample::Resampler`, which streams interleaved stereo
//! for the decode thread. This takes one mono excerpt offline, in forty lines
//! pinned to a stopband measurement.

/// Zero crossings kept either side of center. Sixteen puts aliasing under
/// the library's noise floor, about 45 taps per output at 44.1 to 32 kHz.
const LOBES: usize = 16;

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let pi_x = std::f64::consts::PI * x;
        pi_x.sin() / pi_x
    }
}

/// Blackman rather than Hann: sidelobes ~30 dB lower, and sidelobes are the
/// aliasing this exists to prevent.
fn blackman(t: f64) -> f64 {
    let phase = std::f64::consts::PI * (t + 1.0);
    0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos()
}

/// Equal rates copy through; empty input or a zero rate returns empty.
pub fn convert(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    if input.is_empty() || from == 0 || to == 0 {
        return Vec::new();
    }

    let ratio = to as f64 / from as f64;
    // Cycles per input sample: the new Nyquist when downsampling, the old one
    // when upsampling (a pure interpolator).
    let cutoff = 0.5 * ratio.min(1.0);
    // Widens as the cutoff drops, keeping LOBES crossings inside.
    let half = (LOBES as f64 / (2.0 * cutoff)).ceil() as isize;

    let out_len = ((input.len() as f64) * ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for j in 0..out_len {
        let center = j as f64 / ratio;
        let first = (center - half as f64).ceil() as isize;
        let last = (center + half as f64).floor() as isize;
        let mut sum = 0.0f64;
        let mut weight = 0.0f64;
        for i in first..=last {
            let offset = i as f64 - center;
            let tap = 2.0 * cutoff * sinc(2.0 * cutoff * offset) * blackman(offset / half as f64);
            // Taps past the clip's ends take no part in the sum or the weight; counting
            // them is zero padding and swings the edges by up to 14%.
            if let Some(&sample) = input.get(i.max(0) as usize).filter(|_| i >= 0) {
                sum += sample as f64 * tap;
                weight += tap;
            }
        }
        // Normalize by the taps applied, or the kernel's phase-dependent sum shows
        // up as a gain wobble the mel front end reads as level.
        out.push(if weight.abs() > 1e-12 {
            (sum / weight) as f32
        } else {
            0.0
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn sine(rate: u32, secs: f64, freq: f64) -> Vec<f32> {
        let n = (rate as f64 * secs) as usize;
        (0..n)
            .map(|i| (TAU * freq * i as f64 / rate as f64).sin() as f32)
            .collect()
    }

    /// One-bin DFT, so the frequency needn't sit on an FFT grid.
    fn magnitude_at(samples: &[f32], rate: u32, freq: f64) -> f64 {
        // Skip the kernel's reach at both ends.
        let skip = (rate as usize / 20).min(samples.len() / 4);
        let body = &samples[skip..samples.len() - skip];
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &s) in body.iter().enumerate() {
            let angle = TAU * freq * i as f64 / rate as f64;
            re += s as f64 * angle.cos();
            im -= s as f64 * angle.sin();
        }
        2.0 * re.hypot(im) / body.len() as f64
    }

    #[test]
    fn equal_rates_pass_through_untouched() {
        let input = sine(44_100, 0.1, 1000.0);
        assert_eq!(convert(&input, 44_100, 44_100), input);
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(convert(&[], 44_100, 32_000).is_empty());
        assert!(convert(&[1.0, 2.0], 0, 32_000).is_empty());
        assert!(convert(&[1.0, 2.0], 44_100, 0).is_empty());
    }

    #[test]
    fn the_output_length_follows_the_ratio() {
        let input = vec![0.0f32; 44_100];
        assert_eq!(convert(&input, 44_100, 32_000).len(), 32_000);
        assert_eq!(convert(&input, 44_100, 22_050).len(), 22_050);
        assert_eq!(convert(&input, 44_100, 88_200).len(), 88_200);
    }

    #[test]
    fn a_passband_tone_survives_at_its_own_level() {
        let out = convert(&sine(44_100, 0.5, 1000.0), 44_100, 32_000);
        let at_tone = magnitude_at(&out, 32_000, 1000.0);
        assert!(
            (at_tone - 1.0).abs() < 0.02,
            "a full-scale tone came out at {at_tone}"
        );
        assert!(magnitude_at(&out, 32_000, 2000.0) < 0.01);
    }

    /// The point of the module: 19 kHz must be removed, not folded to 13 kHz.
    /// Linear interpolation fails this.
    #[test]
    fn a_tone_above_the_new_nyquist_is_removed_not_folded_down() {
        let out = convert(&sine(44_100, 0.5, 19_000.0), 44_100, 32_000);
        // 44100 - 19000 = 25100, reflected about 16 kHz lands at 13 kHz.
        let alias = magnitude_at(&out, 32_000, 13_000.0);
        assert!(
            alias < 0.01,
            "a 19 kHz tone aliased down to 13 kHz at {alias}, which is the bug"
        );
        let energy: f64 = out.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / out.len() as f64;
        assert!(
            energy < 1e-4,
            "the stopband tone kept {} of its energy",
            energy * 2.0
        );
    }

    /// Flat to 14 kHz, the top of PANNs' filterbank; about -3 dB just under the
    /// new Nyquist; about -78 dB by 19 kHz. The 16-18 kHz transition folds to
    /// 14-16 kHz, above the model's fmax, so it never reaches a filterbank row.
    #[test]
    fn the_passband_is_flat_where_the_model_looks_and_the_stopband_is_deep() {
        let level = |hz: f64| {
            let out = convert(&sine(44_100, 0.5, hz), 44_100, 32_000);
            magnitude_at(&out, 32_000, hz.min(16_000.0))
        };
        assert!(level(1_000.0) > 0.99);
        assert!(level(10_000.0) > 0.99);
        assert!(level(14_000.0) > 0.95, "the model's fmax must survive");
        // Measured over the body: the edge samples show truncation, not leakage.
        let stopband = convert(&sine(44_100, 0.5, 19_000.0), 44_100, 32_000);
        let skip = stopband.len() / 10;
        let body = &stopband[skip..stopband.len() - skip];
        let rms =
            (body.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / body.len() as f64).sqrt();
        assert!(rms < 1e-3, "19 kHz came through at rms {rms}");
    }

    /// Upsampling interpolates the band at full level.
    #[test]
    fn upsampling_interpolates_without_dulling_the_band() {
        let out = convert(&sine(32_000, 0.5, 10_000.0), 32_000, 48_000);
        let level = magnitude_at(&out, 48_000, 10_000.0);
        assert!((level - 1.0).abs() < 0.02, "came out at {level}");
    }

    /// Constant from the first sample; dividing by the ideal total gives 0.863
    /// and 1.095 at the edges.
    #[test]
    fn a_constant_holds_its_level_right_up_to_the_edges() {
        let out = convert(&vec![1.0f32; 4096], 44_100, 32_000);
        assert_eq!(out.len(), 2972);
        for (i, &s) in out.iter().enumerate() {
            assert!((s - 1.0).abs() < 1e-4, "sample {i} came out at {s}");
        }
    }

    /// No NaN when a clip is shorter than the kernel.
    #[test]
    fn short_and_silent_clips_stay_finite() {
        assert!(
            convert(&[0.0; 4096], 44_100, 32_000)
                .iter()
                .all(|&s| s == 0.0)
        );
        for &n in &[1usize, 2, 7, 64] {
            let out = convert(&vec![0.25f32; n], 44_100, 32_000);
            assert!(out.iter().all(|s| s.is_finite()), "n = {n} produced a NaN");
        }
    }
}
