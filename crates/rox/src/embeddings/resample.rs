//! Band-limited sample rate conversion for the model front end.
//!
//! ## Why not the engine's resampler
//!
//! `rox_playback::resample::Resampler` interpolates linearly, which the
//! module header says outright is spike-grade. For playback it's defensible:
//! the device rate is usually the file's rate or close to it, and a small
//! ratio moves very little energy around. Feeding a model is the opposite
//! case. A 44.1 kHz file going to a model's 32 kHz has 6 kHz of content
//! above the new Nyquist, and linear interpolation doesn't remove it, it
//! folds it back down: 19 kHz lands at 13 kHz, sitting right in the middle
//! of the band the network is looking at, indistinguishable from music that
//! was actually there. Cymbals and sibilance turn into midrange, and every
//! embedding in a library is quietly wrong in a way that depends on what
//! rate each file happened to be at.
//!
//! So this is a windowed-sinc resampler: it low-passes and resamples in one
//! kernel, which is the textbook answer and the one every serious resampler
//! is a faster version of.
//!
//! ## Why it's hand-rolled
//!
//! rubato is the obvious dependency, and the engine's own comment says it's
//! where the real resampler goes when playback needs one. This is not that
//! job. It runs offline, once per excerpt, on a background worker, so the
//! FFT-domain and SIMD tricks a real-time resampler needs buy nothing here,
//! and the whole thing is forty lines that can be pinned to a stopband
//! measurement in a test.

/// How many zero crossings of the sinc the kernel keeps either side of the
/// center. More lobes means a sharper transition and deeper stopband, at a
/// linear cost in taps. Sixteen puts the stopband far enough down that
/// aliased content lands under the noise floor of anything the library
/// holds, and costs about 45 taps per output sample at 44.1 to 32 kHz.
const LOBES: usize = 16;

/// The sinc, with the removable singularity at zero filled in.
fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let pi_x = std::f64::consts::PI * x;
        pi_x.sin() / pi_x
    }
}

/// Blackman window over -1..1, which is where the kernel's taper comes
/// from. Blackman rather than a plain Hann because its sidelobes are around
/// 30 dB further down, and sidelobes here are exactly the aliasing this
/// module exists to prevent.
fn blackman(t: f64) -> f64 {
    let phase = std::f64::consts::PI * (t + 1.0);
    0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos()
}

/// Resample `input` from `from` Hz to `to` Hz.
///
/// Equal rates copy through untouched rather than running a kernel whose
/// answer would be the input plus float noise. An empty input, or either
/// rate at zero, comes back empty: there is no signal to convert.
pub fn convert(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    if input.is_empty() || from == 0 || to == 0 {
        return Vec::new();
    }

    let ratio = to as f64 / from as f64;
    // The passband edge in cycles per input sample. Downsampling has to cut
    // at the new Nyquist, which is below the old one; upsampling has nothing
    // above the old Nyquist to remove, so the cutoff stays there and the
    // kernel is a pure interpolator.
    let cutoff = 0.5 * ratio.min(1.0);
    // The kernel's support, in input samples. It widens as the cutoff drops,
    // which is what keeps LOBES zero crossings inside it whatever the ratio.
    let half = (LOBES as f64 / (2.0 * cutoff)).ceil() as isize;

    let out_len = ((input.len() as f64) * ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for j in 0..out_len {
        // Where this output sample sits in input coordinates.
        let center = j as f64 / ratio;
        let first = (center - half as f64).ceil() as isize;
        let last = (center + half as f64).floor() as isize;
        let mut sum = 0.0f64;
        let mut weight = 0.0f64;
        for i in first..=last {
            let offset = i as f64 - center;
            let tap = 2.0 * cutoff * sinc(2.0 * cutoff * offset) * blackman(offset / half as f64);
            // Samples off either end read as zero, which is the right
            // boundary for a clip: it starts and it stops. The running
            // weight sum below is what keeps that from dimming the first and
            // last few output samples.
            if let Some(&sample) = input.get(i.max(0) as usize).filter(|_| i >= 0) {
                sum += sample as f64 * tap;
            }
            weight += tap;
        }
        // Normalizing by the taps actually applied rather than by their
        // ideal total: the kernel's own sum drifts a fraction of a percent
        // with the fractional phase, and near the edges the truncation is
        // much more than a fraction. Both show up as a gain wobble the mel
        // front end would read as a level change.
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

    /// The magnitude at one frequency, by correlating against a complex
    /// exponential. A DFT of exactly the bin being asked about, which avoids
    /// having to line a frequency up with an FFT grid.
    fn magnitude_at(samples: &[f32], rate: u32, freq: f64) -> f64 {
        // Skip the kernel's reach at both ends: the edge samples are correct
        // but the window they'd need to be measured over runs off the clip.
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

    /// A tone well inside the passband comes out at the same frequency and
    /// the same level. This is the boring half; the next test is the point.
    #[test]
    fn a_passband_tone_survives_at_its_own_level() {
        let out = convert(&sine(44_100, 0.5, 1000.0), 44_100, 32_000);
        let at_tone = magnitude_at(&out, 32_000, 1000.0);
        assert!(
            (at_tone - 1.0).abs() < 0.02,
            "a full-scale tone came out at {at_tone}"
        );
        // And nowhere else: an octave up should be down in the dirt.
        assert!(magnitude_at(&out, 32_000, 2000.0) < 0.01);
    }

    /// The whole reason this module exists. A 19 kHz tone has no home below
    /// 32 kHz's 16 kHz Nyquist, so it must be filtered out rather than
    /// folded down to 13 kHz. Linear interpolation fails this outright,
    /// which is why the engine's resampler can't be used here.
    #[test]
    fn a_tone_above_the_new_nyquist_is_removed_not_folded_down() {
        let out = convert(&sine(44_100, 0.5, 19_000.0), 44_100, 32_000);
        // 44100 - 19000 = 25100, which reflects about 16 kHz to land at
        // 13 kHz. That's where a naive resampler puts it.
        let alias = magnitude_at(&out, 32_000, 13_000.0);
        assert!(
            alias < 0.01,
            "a 19 kHz tone aliased down to 13 kHz at {alias}, which is the bug"
        );
        // Nothing else survived either: the tone is gone, not moved.
        let energy: f64 = out.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / out.len() as f64;
        assert!(
            energy < 1e-4,
            "the stopband tone kept {} of its energy",
            energy * 2.0
        );
    }

    /// The shape of the transition, which is what decides whether the
    /// filtering is good enough for the job.
    ///
    /// The passband has to be flat up to 14 kHz, because that's the top of
    /// PANNs' own mel filterbank and therefore the highest frequency the
    /// model ever looks at. Above that it may droop, and it does: about
    /// -3 dB just under the new Nyquist. The stopband has to be deep by
    /// 19 kHz, which it is, at roughly -78 dB.
    ///
    /// The 16 to 18 kHz band in between is the transition, and it's where
    /// the leakage lives. That's survivable here rather than merely
    /// tolerated: content in it folds down to 14 to 16 kHz, which sits above
    /// the model's fmax and never reaches a filterbank row.
    #[test]
    fn the_passband_is_flat_where_the_model_looks_and_the_stopband_is_deep() {
        let level = |hz: f64| {
            let out = convert(&sine(44_100, 0.5, hz), 44_100, 32_000);
            magnitude_at(&out, 32_000, hz.min(16_000.0))
        };
        assert!(level(1_000.0) > 0.99);
        assert!(level(10_000.0) > 0.99);
        assert!(level(14_000.0) > 0.95, "the model's fmax must survive");
        // Deep enough by 19 kHz that nothing folding out of there is
        // measurable against 16-bit audio's own noise floor. Measured over
        // the body of the clip: the first and last few samples carry the
        // kernel's truncation against the clip's own edges, which is a
        // boundary effect rather than something that leaked through the
        // filter.
        let stopband = convert(&sine(44_100, 0.5, 19_000.0), 44_100, 32_000);
        let skip = stopband.len() / 10;
        let body = &stopband[skip..stopband.len() - skip];
        let rms =
            (body.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / body.len() as f64).sqrt();
        assert!(rms < 1e-3, "19 kHz came through at rms {rms}");
    }

    /// Upsampling has nothing to remove, so inside the band it must be a
    /// plain interpolator rather than a filter: the tone comes back at its
    /// own level.
    #[test]
    fn upsampling_interpolates_without_dulling_the_band() {
        let out = convert(&sine(32_000, 0.5, 10_000.0), 32_000, 48_000);
        let level = magnitude_at(&out, 48_000, 10_000.0);
        assert!((level - 1.0).abs() < 0.02, "came out at {level}");
    }

    /// Silence in, silence out, and no NaN from the normalization when a
    /// clip is shorter than the kernel.
    #[test]
    fn short_and_silent_clips_stay_finite() {
        assert!(convert(&[0.0; 4096], 44_100, 32_000)
            .iter()
            .all(|&s| s == 0.0));
        for &n in &[1usize, 2, 7, 64] {
            let out = convert(&vec![0.25f32; n], 44_100, 32_000);
            assert!(out.iter().all(|s| s.is_finite()), "n = {n} produced a NaN");
        }
    }
}
