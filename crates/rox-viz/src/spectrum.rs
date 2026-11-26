//! The spectrum a renderer can be granted (ADR 37): two rows of normalised
//! magnitudes worked out from the hub's own FFT, for the spectrum mapping
//! rox writes. One row is log-spaced across 20 Hz to 20 kHz, the shape bars
//! and particles want. The other is linear, laid out the way ShaderToy-style
//! audio shaders read theirs: 512 bins of 44,100 / 2048 Hz each from 0 Hz.
//!
//! Magnitudes only, never samples and never phase. A full-rate waveform is
//! the audio itself, and a renderer could record what the user plays. Rows
//! with no phase can't give the recording back. Values sit in the hub's dB window ([`FLOOR_DB`] to [`MAX_DB`]), so
//! they line up with what the pool's bands read before their Response.

use crate::signal::{FLOOR_DB, MAX_DB};

/// Bins in the log row.
pub const LOG_BINS: usize = 128;
/// The log row's lowest edge and highest edge, in Hz.
pub const LOG_LOW_HZ: f32 = 20.0;
pub const LOG_HIGH_HZ: f32 = 20_000.0;

/// Bins in the linear row.
pub const LINEAR_BINS: usize = 512;
/// The linear row's spacing in Hz: bin `j` is the magnitude at
/// `j * LINEAR_BIN_HZ`, the bins a 2048-point FFT gives at 44.1 kHz,
/// whatever rate the audio is at.
pub const LINEAR_BIN_HZ: f32 = 44_100.0 / 2048.0;

/// Log bin `i`'s lower edge in Hz, for `i` from 0 to [`LOG_BINS`]: bin `i`
/// runs from `log_edge(i)` to `log_edge(i + 1)`.
pub fn log_edge(i: usize) -> f32 {
    LOG_LOW_HZ * libm::powf(LOG_HIGH_HZ / LOG_LOW_HZ, i as f32 / LOG_BINS as f32)
}

/// Log bin `i`'s centre in Hz, the geometric middle of its edges.
pub fn log_centre(i: usize) -> f32 {
    LOG_LOW_HZ * libm::powf(LOG_HIGH_HZ / LOG_LOW_HZ, (i as f32 + 0.5) / LOG_BINS as f32)
}

/// A magnitude in the hub's dB window, 0 at [`FLOOR_DB`] and below, 1 at
/// [`MAX_DB`] and above: the same mapping a pool band reads.
pub fn normalise(magnitude: f32) -> f32 {
    db_to_unit(db(magnitude))
}

fn db(magnitude: f32) -> f32 {
    20.0 * (magnitude + 1e-9).log10()
}

fn db_to_unit(db: f32) -> f32 {
    ((db - FLOOR_DB) / (MAX_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// How one log bin reads the FFT.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LogPlan {
    /// The loudest of the FFT bins whose centres fall inside it, the
    /// peak a pool band reads.
    Peak { lo: usize, hi: usize },
    /// Narrower than an FFT bin: a parabola in dB through FFT bins `k - 1`,
    /// `k` and `k + 1`, read `x` bins from `k`.
    Between { k: usize, x: f32 },
    /// Above the audio's Nyquist frequency.
    Silent,
}

/// How one linear bin reads the FFT: between bins `k` and `k + 1`, `t` of
/// the way.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LinearPlan {
    k: usize,
    t: f32,
}

/// The two rows, and the plan that maps an FFT onto them. The plan is
/// worked out again only when the audio's rate or the FFT's size changes,
/// so filling the rows allocates nothing.
pub struct Rows {
    log: [f32; LOG_BINS],
    linear: [f32; LINEAR_BINS],
    planned: Option<(u32, usize)>,
    log_plan: [LogPlan; LOG_BINS],
    linear_plan: [LinearPlan; LINEAR_BINS],
}

impl Default for Rows {
    fn default() -> Self {
        Rows {
            log: [0.0; LOG_BINS],
            linear: [0.0; LINEAR_BINS],
            planned: None,
            log_plan: [LogPlan::Silent; LOG_BINS],
            linear_plan: [LinearPlan { k: 0, t: 0.0 }; LINEAR_BINS],
        }
    }
}

impl Rows {
    pub fn log(&self) -> &[f32; LOG_BINS] {
        &self.log
    }

    pub fn linear(&self) -> &[f32; LINEAR_BINS] {
        &self.linear
    }

    /// Both rows from `mags`, the lower half-spectrum of an FFT of the
    /// audio at `sample_rate`, as [`crate::AudioFeed::magnitudes`] gives it.
    pub fn fill(&mut self, mags: &[f32], sample_rate: u32) {
        let half = mags.len();
        if half < 2 || sample_rate == 0 {
            self.clear();
            return;
        }

        if self.planned != Some((sample_rate, half)) {
            self.plan(sample_rate, half);
        }

        for (out, plan) in self.log.iter_mut().zip(&self.log_plan) {
            *out = match *plan {
                LogPlan::Peak { lo, hi } => {
                    normalise(mags[lo..hi].iter().copied().fold(0.0, f32::max))
                }
                LogPlan::Between { k, x } => {
                    let (a, b, c) = (db(mags[k - 1]), db(mags[k]), db(mags[k + 1]));
                    db_to_unit(b + 0.5 * x * (c - a) + 0.5 * x * x * (a - 2.0 * b + c))
                }
                LogPlan::Silent => 0.0,
            };
        }

        for (out, plan) in self.linear.iter_mut().zip(&self.linear_plan) {
            let here = mags.get(plan.k).copied().unwrap_or(0.0);
            let next = mags.get(plan.k + 1).copied().unwrap_or(0.0);
            *out = normalise(here + (next - here) * plan.t);
        }
    }

    /// Both rows to 0, for silence.
    pub fn clear(&mut self) {
        self.log = [0.0; LOG_BINS];
        self.linear = [0.0; LINEAR_BINS];
    }

    fn plan(&mut self, sample_rate: u32, half: usize) {
        let bin_hz = sample_rate as f32 / (2 * half) as f32;
        let nyquist = sample_rate as f32 / 2.0;

        for (i, plan) in self.log_plan.iter_mut().enumerate() {
            let (f0, f1) = (log_edge(i), log_edge(i + 1));

            // A bin at least as wide as an FFT bin holds the centre of one or
            // more, and takes the loudest. One narrower would read the same
            // FFT bin as its neighbours and step every 23 Hz.
            *plan = if f0 >= nyquist {
                LogPlan::Silent
            } else if f1 - f0 >= bin_hz {
                let lo = ((f0 / bin_hz).ceil() as usize).clamp(1, half - 1);
                let hi = ((f1 / bin_hz).ceil() as usize).clamp(lo + 1, half);
                LogPlan::Peak { lo, hi }
            } else {
                let at = log_centre(i) / bin_hz;
                let k = (at.round() as usize).clamp(1, half - 2);
                LogPlan::Between {
                    k,
                    x: (at - k as f32).clamp(-1.0, 1.0),
                }
            };
        }

        for (j, plan) in self.linear_plan.iter_mut().enumerate() {
            let at = j as f32 * LINEAR_BIN_HZ / bin_hz;
            let k = at.floor() as usize;
            *plan = LinearPlan {
                k,
                t: at - k as f32,
            };
        }

        self.planned = Some((sample_rate, half));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analyzer;
    use std::f32::consts::TAU;

    const FFT: usize = 2048;

    fn spectrum_of(samples: &[f32]) -> Vec<f32> {
        Analyzer::new(FFT).magnitudes(samples).to_vec()
    }

    fn tone(hz: f32, rate: u32, amplitude: f32) -> Vec<f32> {
        (0..FFT)
            .map(|i| amplitude * (TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn loudest(row: &[f32]) -> usize {
        row.iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .unwrap()
            .0
    }

    /// The log bin a frequency falls in, by the documented formula.
    fn log_bin_of(hz: f32) -> usize {
        let at = (hz / LOG_LOW_HZ).ln() / (LOG_HIGH_HZ / LOG_LOW_HZ).ln() * LOG_BINS as f32;
        at.floor() as usize
    }

    #[test]
    fn the_edges_run_from_20_hz_to_20_khz() {
        assert!((log_edge(0) - 20.0).abs() < 1e-3);
        assert!((log_edge(LOG_BINS) - 20_000.0).abs() < 0.5);
        assert!(
            (log_edge(64) - 632.46).abs() < 0.05,
            "the middle is 20 * sqrt(1000)"
        );
        assert!(log_edge(10) < log_centre(10) && log_centre(10) < log_edge(11));
        assert!((LINEAR_BIN_HZ - 21.533_203).abs() < 1e-4);
    }

    #[test]
    fn a_tone_lands_in_the_bin_its_frequency_names() {
        for rate in [44_100, 48_000] {
            for hz in [60.0, 100.0, 250.0, 1000.0, 3150.0, 10_000.0, 16_000.0] {
                let mut rows = Rows::default();
                // -30 dB, inside the window, so the peak isn't clipped flat.
                rows.fill(&spectrum_of(&tone(hz, rate, 0.03)), rate);

                let want = log_bin_of(hz) as i64;
                let got = loudest(rows.log()) as i64;
                assert!(
                    (got - want).abs() <= 1,
                    "{hz} Hz at {rate}: bin {got}, the formula says {want}"
                );

                // The linear row's own bin for it, where the row reaches.
                if hz < LINEAR_BIN_HZ * LINEAR_BINS as f32 {
                    let want = (hz / LINEAR_BIN_HZ).round() as i64;
                    let got = loudest(rows.linear()) as i64;
                    assert!((got - want).abs() <= 1, "{hz} Hz at {rate}: linear {got}");
                }
            }
        }
    }

    #[test]
    fn a_tone_reads_what_the_pool_reads() {
        // A full-scale sine is 0 dB, past the window's top.
        let mut rows = Rows::default();
        rows.fill(&spectrum_of(&tone(1000.0, 48_000, 1.0)), 48_000);
        assert_eq!(rows.log()[log_bin_of(1000.0)], 1.0);

        // -39 dB is the middle of the -66 to -12 window.
        let quiet = 10f32.powf(-39.0 / 20.0);
        rows.fill(&spectrum_of(&tone(1000.0, 48_000, quiet)), 48_000);
        let read = rows.log()[log_bin_of(1000.0)];
        assert!((read - 0.5).abs() < 0.03, "read {read}");
    }

    #[test]
    fn a_flat_spectrum_is_a_flat_row() {
        // -39 dB in every FFT bin.
        let mags = vec![10f32.powf(-39.0 / 20.0); FFT / 2];

        for rate in [44_100, 48_000] {
            let mut rows = Rows::default();
            rows.fill(&mags, rate);

            for (i, &v) in rows.log().iter().enumerate() {
                assert!((v - 0.5).abs() < 1e-3, "log bin {i} at {rate}: {v}");
            }
            for (j, &v) in rows.linear().iter().enumerate() {
                assert!((v - 0.5).abs() < 1e-3, "linear bin {j} at {rate}: {v}");
            }
        }
    }

    #[test]
    fn white_noise_is_flat_within_what_a_peak_reads() {
        // A fixed LCG, so the test never flakes.
        let mut state = 0x2545_f491_u32;
        let noise: Vec<f32> = (0..FFT)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / (1 << 24) as f32 * 0.6 - 0.3
            })
            .collect();

        let mut rows = Rows::default();
        rows.fill(&spectrum_of(&noise), 48_000);

        // No slope across the row: the top third reads no lower than the
        // bottom third. A wide bin's peak of many noisy FFT bins sits higher
        // than one bin's value, so the top can read higher, by a bounded
        // amount.
        let third = LOG_BINS / 3;
        let mean = |row: &[f32]| row.iter().sum::<f32>() / row.len() as f32;
        let (low, high) = (mean(&rows.log()[..third]), mean(&rows.log()[2 * third..]));
        assert!(low > 0.3, "noise reads in the window: {low}");
        assert!(
            high >= low - 0.02 && high - low < 0.2,
            "low {low}, high {high}"
        );
    }

    /// Noise from a fixed LCG, so the tests never flake: white, or pink
    /// through Paul Kellet's refined filter, which holds -3 dB an octave to
    /// within 0.05 dB above about 10 Hz.
    fn noise(samples: usize, pink: bool) -> Vec<f32> {
        let mut state = 0x9e37_79b9_u32;
        let mut b = [0.0f32; 7];

        (0..samples)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let white = (state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0;
                if !pink {
                    return white * 0.3;
                }

                b[0] = 0.99886 * b[0] + white * 0.055_517_9;
                b[1] = 0.99332 * b[1] + white * 0.075_075_9;
                b[2] = 0.96900 * b[2] + white * 0.153_852;
                b[3] = 0.86650 * b[3] + white * 0.310_485_6;
                b[4] = 0.55000 * b[4] + white * 0.532_952_2;
                b[5] = -0.7616 * b[5] - white * 0.016_898;
                let pink = b.iter().sum::<f32>() + white * 0.5362;
                b[6] = white * 0.115_926;

                // Keeps the whole row inside the window, about 0.57 at the
                // bottom and 0.16 at the top.
                pink * 0.02
            })
            .collect()
    }

    /// The log row and the FFT's power, each averaged over `windows`
    /// back-to-back windows of noise at `rate`.
    fn averaged(pink: bool, rate: u32, windows: usize) -> (Vec<f32>, Vec<f64>) {
        let samples = noise(FFT * (windows + 1), pink);
        let mut analyzer = Analyzer::new(FFT);
        let mut rows = Rows::default();
        let mut log = vec![0.0f32; LOG_BINS];
        let mut power = vec![0.0f64; FFT / 2];

        // The first window lets the filter settle.
        for window in samples.as_chunks::<FFT>().0.iter().skip(1) {
            let mags = analyzer.magnitudes(window);
            for (p, &m) in power.iter_mut().zip(mags) {
                *p += f64::from(m) * f64::from(m);
            }

            rows.fill(mags, rate);
            for (sum, &v) in log.iter_mut().zip(rows.log()) {
                *sum += v;
            }
        }

        log.iter_mut().for_each(|v| *v /= windows as f32);
        power.iter_mut().for_each(|p| *p /= windows as f64);
        (log, power)
    }

    /// The least-squares slope of `row` per octave over the log bins `bins`.
    fn slope_per_octave(row: &[f32], bins: std::ops::Range<usize>) -> f32 {
        let points: Vec<(f32, f32)> = bins.map(|i| (log_centre(i).log2(), row[i])).collect();
        let n = points.len() as f32;
        let (mx, my) = points
            .iter()
            .fold((0.0, 0.0), |(x, y), &(px, py)| (x + px / n, y + py / n));

        let (sxy, sxx) = points.iter().fold((0.0, 0.0), |(sxy, sxx), &(x, y)| {
            (sxy + (x - mx) * (y - my), sxx + (x - mx) * (x - mx))
        });
        sxy / sxx
    }

    /// The first log bin at least an FFT bin wide: below it the row
    /// interpolates one FFT bin, from it up it takes a peak over several.
    fn crossover(rate: u32) -> usize {
        let bin_hz = rate as f32 / FFT as f32;
        (0..LOG_BINS)
            .find(|&i| log_edge(i + 1) - log_edge(i) >= bin_hz)
            .unwrap()
    }

    fn mean(values: &[f32]) -> f32 {
        values.iter().sum::<f32>() / values.len() as f32
    }

    /// The guide's numbers for pink noise, measured. Under 40 Hz the window
    /// holds less than two cycles, so the fits start there.
    #[test]
    fn pink_noise_falls_3_db_an_octave_below_the_crossover_and_half_that_above() {
        // 3 dB in the 54 dB window.
        const THREE_DB: f32 = 3.0 / 54.0;

        assert_eq!((crossover(44_100), crossover(48_000)), (55, 57));
        assert!((log_edge(55) - 389.1).abs() < 0.1 && (log_edge(57) - 433.5).abs() < 0.1);

        for rate in [44_100, 48_000] {
            let (log, power) = averaged(true, rate, 2000);
            let cross = crossover(rate);
            let start = log_bin_of(40.0);

            // The input is pink: every octave from 62.5 Hz holds the same power.
            let bin_hz = rate as f32 / FFT as f32;
            let octaves: Vec<f64> = (0..8)
                .map(|n| {
                    let f = 62.5 * 2f32.powi(n);
                    let (lo, hi) = (
                        (f / bin_hz).ceil() as usize,
                        (2.0 * f / bin_hz).ceil() as usize,
                    );
                    10.0 * power[lo..hi].iter().sum::<f64>().log10()
                })
                .collect();
            let (quiet, loud) = octaves
                .iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), &e| (lo.min(e), hi.max(e)));
            assert!(loud - quiet < 1.0, "{rate}: octaves {octaves:?}");

            // One FFT bin's level, interpolated, falls as pink noise's power
            // per bin does.
            let below = slope_per_octave(&log, start..cross);
            assert!((below + THREE_DB).abs() < 0.005, "{rate}: {below} below");

            // A peak over more FFT bins as the log bins widen makes up about
            // half of it.
            let above = slope_per_octave(&log, cross..LOG_BINS);
            assert!(
                (above + THREE_DB / 2.0).abs() < 0.005,
                "{rate}: {above} above"
            );

            // The two slopes, measured from the crossover's edge, flatten it
            // to within about 1.6 dB.
            let at = log_edge(cross);
            let flattened: Vec<f32> = (start..LOG_BINS)
                .map(|i| {
                    let slope = if i < cross { 0.056 } else { 0.028 };
                    log[i] + slope * (log_centre(i) / at).log2()
                })
                .collect();
            let level = mean(&flattened);
            for (i, v) in (start..).zip(&flattened) {
                assert!(
                    (v - level).abs() < 0.035,
                    "{rate}: bin {i} reads {v}, the mean {level}"
                );
            }

            // One 3 dB slope across the whole row tips it up instead.
            let tipped: Vec<f32> = (start..LOG_BINS)
                .map(|i| log[i] + THREE_DB * (log_centre(i) / 1000.0).log2())
                .collect();
            let third = tipped.len() / 3;
            let rise = mean(&tipped[2 * third..]) - mean(&tipped[..third]);
            assert!(rise > 0.1, "{rate}: {rise}");
        }
    }

    #[test]
    fn white_noise_reads_higher_where_the_bins_are_wider() {
        for rate in [44_100, 48_000] {
            let (log, _) = averaged(false, rate, 500);

            // Flat while a log bin reads one FFT bin.
            let cross = crossover(rate);
            let below = slope_per_octave(&log, log_bin_of(40.0)..cross);
            assert!(below.abs() < 0.005, "{rate}: {below}");

            // The top third reads about 7 dB over the bottom third.
            let third = LOG_BINS / 3;
            let rise = (mean(&log[2 * third..]) - mean(&log[..third])) * 54.0;
            assert!((6.0..8.0).contains(&rise), "{rate}: {rise} dB");
        }
    }

    #[test]
    fn silence_is_zeros() {
        let mut rows = Rows::default();
        rows.fill(&spectrum_of(&vec![0.0; FFT]), 48_000);
        assert!(rows.log().iter().all(|&v| v == 0.0));
        assert!(rows.linear().iter().all(|&v| v == 0.0));

        rows.fill(&spectrum_of(&tone(440.0, 48_000, 0.5)), 48_000);
        rows.clear();
        assert!(rows.log().iter().chain(rows.linear()).all(|&v| v == 0.0));
    }

    #[test]
    fn the_linear_row_is_the_fft_itself_at_44_1_khz() {
        let mags = spectrum_of(&tone(5000.0, 44_100, 0.25));
        let mut rows = Rows::default();
        rows.fill(&mags, 44_100);

        for (j, &v) in rows.linear().iter().enumerate() {
            assert_eq!(v, normalise(mags[j]), "bin {j}");
        }
    }

    #[test]
    fn nothing_past_nyquist_reads() {
        // At 32 kHz the audio stops at 16 kHz.
        let mags = vec![1.0; FFT / 2];
        let mut rows = Rows::default();
        rows.fill(&mags, 32_000);

        let last = log_bin_of(16_000.0);
        assert!(rows.log()[..last].iter().all(|&v| v == 1.0));
        assert!(rows.log()[last + 1..].iter().all(|&v| v == 0.0));

        // A 22.05 kHz file on a device opened at its rate: bin 116 straddles
        // 11,025 Hz and reads what's under it, and the linear row, which
        // stops at 11,003.5 Hz, reads all of its bins.
        rows.fill(&mags, 22_050);
        assert_eq!(log_bin_of(11_025.0), 116);
        assert!(rows.log()[..117].iter().all(|&v| v == 1.0));
        assert!(rows.log()[117..].iter().all(|&v| v == 0.0));
        assert!(rows.linear().iter().all(|&v| v == 1.0));
    }
}
