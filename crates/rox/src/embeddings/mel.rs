//! Log-mel spectrograms, the input every acoustic model here eats.
//!
//! ## Why this is written out longhand
//!
//! A network's mel front end is part of the network. The weights were fit
//! against one exact spectrogram recipe, and feeding them a different one
//! doesn't produce a worse embedding, it produces a meaningless one that
//! looks fine: the vectors still have the right shape, the cosines still
//! land between -1 and 1, and the nearest-neighbour lists still come back
//! ranked. Nothing anywhere says the input was wrong. So every knob a
//! training config can set is a field on [`Config`] rather than a constant,
//! and each model in the catalog states its own.
//!
//! The knobs that actually bite, in rough order of how badly:
//!
//! - **The log convention.** `ln(x + 1e-6)`, `log10(x)`, and dB-with-a-floor
//!   are three different functions with three different dynamic ranges. Get
//!   this wrong and every value the first conv layer sees is off by a scale
//!   factor and an offset.
//! - **Mel scale flavour.** HTK's single log curve and Slaney's
//!   linear-below-1 kHz curve put the band centers in noticeably different
//!   places. Nobody labels which one they used.
//! - **Filterbank normalization.** Slaney-normalized triangles have equal
//!   area, so high bands (which are wide) come out much lower than
//!   unit-peak triangles would put them. This is a per-band gain tilt across
//!   the whole input.
//! - **Power.** Magnitude or magnitude squared, before the log. A factor of
//!   two in log space.
//! - **Framing.** Whether the signal is center-padded by half a window
//!   (librosa's default, and what most PyTorch training code inherits) or
//!   framed from sample zero (TensorFlow's). Changes the frame count and
//!   shifts every frame by half a window.
//!
//! ## What this is not
//!
//! `rox_viz::analysis::log_bands` groups FFT bins into log-spaced ranges and
//! sums them. That is a display device for the spectrum bars and it is not a
//! mel filterbank: no triangular weights, no mel curve, no normalization,
//! and each bin belongs to exactly one band instead of being shared between
//! overlapping neighbours. It's the right thing for drawing and the wrong
//! thing for feeding a network, and the two are easy to confuse because both
//! produce "N log-spaced bands".
//!
//! ## Checked against librosa
//!
//! The whole front end is pinned to golden values generated from librosa
//! 0.11; `the_whole_front_end_matches_librosa_band_by_band` below carries
//! the numbers and the script that produced them. librosa is what most of
//! these models' training code runs on one layer down, so matching it is the
//! closest thing to a proof that the recipe is right.

/// Which mel curve. Nobody agrees, everybody omits it from the readme, and
/// the two disagree by enough to move every band center.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    /// `2595 * log10(1 + f/700)`, one curve all the way down. What HTK,
    /// torchaudio's default, and TensorFlow use.
    Htk,
    /// Linear at 200/3 Hz per mel below 1 kHz, logarithmic above. Slaney's
    /// Auditory Toolbox, and librosa's default.
    Slaney,
}

/// How the triangles are scaled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Norm {
    /// Every triangle peaks at 1. Wide high bands therefore pass much more
    /// energy than narrow low ones.
    UnitPeak,
    /// Every triangle has the same area, so a flat-spectrum input produces a
    /// flat mel spectrum. librosa's `norm="slaney"`, and its default.
    Area,
}

/// The window applied to each frame. Periodic (the `fftbins=True` /
/// `sym=False` flavour) because that's what every spectrogram library uses;
/// the symmetric variant belongs to filter design.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Hann,
    Hamming,
}

/// What runs over the mel energies at the end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Log {
    /// `ln(x + offset)`. The PANNs and torchlibrosa convention, and what
    /// most PyTorch audio training code ends up with.
    Natural { offset: f32 },
    /// `log10(x + offset)`.
    Base10 { offset: f32 },
    /// `10 * log10(max(x, floor))`, then clamped to `top_db` below the
    /// loudest value in this clip. librosa's `power_to_db`. The clamp is
    /// per clip, so it makes the result depend on the whole clip rather than
    /// on each frame alone.
    Db { floor: f32, top_db: Option<f32> },
}

/// One model's spectrogram recipe, copied from its training config. Every
/// field here is load-bearing; see the module header for which ones bite
/// hardest when they're guessed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// What the audio must be resampled to before any of this runs.
    pub sample_rate: u32,
    /// Transform length. Powers of two only: the FFT below is radix-2, and
    /// a config wanting 400 zero-pads a 400-sample window into 512, which is
    /// what librosa does anyway.
    pub n_fft: usize,
    /// How many samples the window actually covers, zero-padded up to
    /// `n_fft`. Equal to `n_fft` in most configs.
    pub win_length: usize,
    pub hop_length: usize,
    pub n_mels: usize,
    pub fmin: f32,
    /// The top of the filterbank. Never above Nyquist; a config that says so
    /// is describing a different sample rate.
    pub fmax: f32,
    pub window: WindowKind,
    /// Whether the signal is reflect-padded by `n_fft / 2` so frame `t` is
    /// centered on sample `t * hop`. librosa's default is true; TensorFlow's
    /// framing is false.
    pub center: bool,
    /// Magnitude (1.0) or power (2.0) before the filterbank.
    pub power: f32,
    pub scale: Scale,
    pub norm: Norm,
    pub log: Log,
}

impl Config {
    /// Bins in the half spectrum the filterbank projects from.
    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// How many frames a clip of `samples` samples produces. Zero when the
    /// clip is shorter than one uncentered window, which is a clip with no
    /// spectrogram rather than an error.
    pub fn frames(&self, samples: usize) -> usize {
        if self.center {
            samples / self.hop_length + 1
        } else if samples < self.n_fft {
            0
        } else {
            (samples - self.n_fft) / self.hop_length + 1
        }
    }

    /// Whether the numbers describe a transform that can actually run.
    /// Checked once when a model is loaded rather than trusted, since a
    /// catalog entry is data and data gets edited.
    pub fn valid(&self) -> Result<(), String> {
        if !self.n_fft.is_power_of_two() {
            return Err(format!("n_fft {} is not a power of two", self.n_fft));
        }
        if self.win_length == 0 || self.win_length > self.n_fft {
            return Err(format!(
                "win_length {} must be between 1 and n_fft {}",
                self.win_length, self.n_fft
            ));
        }
        if self.hop_length == 0 {
            return Err("hop_length must be at least 1".into());
        }
        if self.n_mels == 0 {
            return Err("n_mels must be at least 1".into());
        }
        let nyquist = self.sample_rate as f32 / 2.0;
        if !(0.0..self.fmax).contains(&self.fmin) || self.fmax > nyquist {
            return Err(format!(
                "the band {}..{} Hz doesn't fit under Nyquist at {} Hz",
                self.fmin, self.fmax, self.sample_rate
            ));
        }
        Ok(())
    }
}

/// Hz to mels on the given curve.
pub fn hz_to_mel(hz: f64, scale: Scale) -> f64 {
    match scale {
        Scale::Htk => 2595.0 * (1.0 + hz / 700.0).log10(),
        Scale::Slaney => {
            // Slaney's curve is linear at 200/3 Hz per mel up to 1 kHz, then
            // logarithmic with 27 mels per decade-and-a-bit above it. The
            // two constants below are the join: mel 15 is exactly 1 kHz, and
            // mel 42 is exactly 6.4 kHz.
            const F_SP: f64 = 200.0 / 3.0;
            const MIN_LOG_HZ: f64 = 1000.0;
            const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
            let logstep = (6.4f64).ln() / 27.0;
            if hz >= MIN_LOG_HZ {
                MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() / logstep
            } else {
                hz / F_SP
            }
        }
    }
}

/// Mels back to Hz, the exact inverse of [`hz_to_mel`].
pub fn mel_to_hz(mel: f64, scale: Scale) -> f64 {
    match scale {
        Scale::Htk => 700.0 * (10f64.powf(mel / 2595.0) - 1.0),
        Scale::Slaney => {
            const F_SP: f64 = 200.0 / 3.0;
            const MIN_LOG_HZ: f64 = 1000.0;
            const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
            let logstep = (6.4f64).ln() / 27.0;
            if mel >= MIN_LOG_MEL {
                MIN_LOG_HZ * (logstep * (mel - MIN_LOG_MEL)).exp()
            } else {
                mel * F_SP
            }
        }
    }
}

/// The filterbank: `n_mels` rows of `bins()` weights, each row a triangle
/// spanning three consecutive mel-spaced points.
///
/// This is librosa's `filters.mel` written out. The `n_mels + 2` points give
/// every triangle a left foot, a peak, and a right foot, with neighbours
/// sharing feet so the bank overlaps at half height. Rows can come out all
/// zero when the FFT is too coarse to resolve a band, which is a real
/// outcome (a 64-band bank at 16 kHz over a 400-sample window has none, a
/// 128-band one over the same window has several) and not an error.
pub fn filterbank(config: &Config) -> Vec<Vec<f32>> {
    let bins = config.bins();
    // Bin center frequencies, exactly linspace(0, sr/2, bins).
    let fft_hz: Vec<f64> = (0..bins)
        .map(|k| k as f64 * config.sample_rate as f64 / config.n_fft as f64)
        .collect();

    let lo = hz_to_mel(config.fmin as f64, config.scale);
    let hi = hz_to_mel(config.fmax as f64, config.scale);
    let points: Vec<f64> = (0..config.n_mels + 2)
        .map(|i| {
            let mel = lo + (hi - lo) * i as f64 / (config.n_mels + 1) as f64;
            mel_to_hz(mel, config.scale)
        })
        .collect();

    (0..config.n_mels)
        .map(|m| {
            let (left, center, right) = (points[m], points[m + 1], points[m + 2]);
            // Slaney's area normalization: a triangle spanning more Hz gets
            // scaled down so every band passes the same energy from a flat
            // spectrum. Without it the top bands, which are several times
            // wider, dominate the input.
            let gain = match config.norm {
                Norm::UnitPeak => 1.0,
                Norm::Area => 2.0 / (right - left),
            };
            fft_hz
                .iter()
                .map(|&hz| {
                    // The two ramps, each guarding its own zero-width case:
                    // duplicate points happen when the FFT resolution can't
                    // separate two band edges.
                    let up = if center > left {
                        (hz - left) / (center - left)
                    } else {
                        0.0
                    };
                    let down = if right > center {
                        (right - hz) / (right - center)
                    } else {
                        0.0
                    };
                    (up.min(down).max(0.0) * gain) as f32
                })
                .collect()
        })
        .collect()
}

/// The analysis window, `win_length` long, periodic.
fn window(config: &Config) -> Vec<f64> {
    let n = config.win_length as f64;
    (0..config.win_length)
        .map(|i| {
            let phase = std::f64::consts::TAU * i as f64 / n;
            match config.window {
                WindowKind::Hann => 0.5 - 0.5 * phase.cos(),
                WindowKind::Hamming => 0.54 - 0.46 * phase.cos(),
            }
        })
        .collect()
}

/// The signal a frame reads from, in the framing the config asks for.
///
/// Centered framing reflect-pads by half a transform at both ends so frame
/// `t` is centered on sample `t * hop`, which is what librosa does and what
/// every PyTorch training pipeline inherits from it. Reflection rather than
/// zeros because a zero pad puts a step discontinuity at both ends of the
/// clip and rings across the whole spectrum in the first and last frames.
fn padded(samples: &[f32], config: &Config) -> Vec<f32> {
    if !config.center {
        return samples.to_vec();
    }
    let pad = config.n_fft / 2;
    let mut out = Vec::with_capacity(samples.len() + 2 * pad);
    // numpy's "reflect" mirrors without repeating the edge sample, so a pad
    // of 3 over [a b c d] prepends [d c b]. A clip shorter than the pad
    // reflects off both ends in turn, which is what the modular walk below
    // does; a clip of one sample just repeats it.
    let reflect = |i: isize| -> f32 {
        let n = samples.len() as isize;
        if n == 1 {
            return samples[0];
        }
        let period = 2 * (n - 1);
        let mut j = i.rem_euclid(period);
        if j >= n {
            j = period - j;
        }
        samples[j as usize]
    };
    for i in 0..pad {
        out.push(reflect(-(pad as isize) + i as isize));
    }
    out.extend_from_slice(samples);
    for i in 0..pad {
        out.push(reflect(samples.len() as isize + i as isize));
    }
    out
}

/// In-place iterative radix-2 Cooley-Tukey, f64. Same shape as rox-viz's,
/// in double precision and in this crate: rox-viz's is private, drives the
/// sixty-times-a-second spectrum bars, and has no business growing a second
/// caller with different precision needs.
fn fft(re: &mut [f64], im: &mut [f64]) {
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
        let ang = -std::f64::consts::TAU / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
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

/// A configured front end, holding the window and filterbank so a run over
/// a whole library builds them once rather than per track.
pub struct Mel {
    config: Config,
    window: Vec<f64>,
    bank: Vec<Vec<f32>>,
}

impl Mel {
    /// Build the front end, or say why the config can't run.
    pub fn new(config: Config) -> Result<Self, String> {
        config.valid()?;
        Ok(Mel {
            window: window(&config),
            bank: filterbank(&config),
            config,
        })
    }

    /// The same, with a filterbank supplied rather than derived.
    ///
    /// For models that ship the exact bank they were trained with, which
    /// PANNs does: using the file's own matrix removes the last place a
    /// mel reimplementation could be subtly off, and leaves
    /// [`Mel::bank_deviation`] to say whether the config is right anyway.
    pub fn with_bank(config: Config, bank: Vec<Vec<f32>>) -> Result<Self, String> {
        config.valid()?;
        if bank.len() != config.n_mels || bank.iter().any(|row| row.len() != config.bins()) {
            return Err(format!(
                "the supplied filterbank isn't {} rows of {} weights",
                config.n_mels,
                config.bins()
            ));
        }
        Ok(Mel {
            window: window(&config),
            bank,
            config,
        })
    }

    /// The largest single weight this front end's bank differs from the one
    /// its config describes, relative to the bank's loudest weight.
    ///
    /// Zero for a bank built from the config. For a supplied one it's the
    /// answer to "is the config actually the recipe these weights were
    /// trained with", which is the question no readme ever answers.
    pub fn bank_deviation(&self) -> f32 {
        let derived = filterbank(&self.config);
        let peak = derived
            .iter()
            .flatten()
            .fold(0.0f32, |m, &w| m.max(w.abs()))
            .max(f32::MIN_POSITIVE);
        self.bank
            .iter()
            .zip(&derived)
            .flat_map(|(mine, theirs)| mine.iter().zip(theirs))
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()))
            / peak
    }

    /// The recipe this front end runs.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The log-mel spectrogram of one mono clip, frame-major: `frames`
    /// rows of `n_mels` values. An empty result means the clip was too
    /// short to frame at all.
    ///
    /// `samples` must already be at [`Config::sample_rate`]. Nothing here
    /// resamples, on purpose: a resample belongs to the decode, where the
    /// original rate is known and a proper band-limited filter can run.
    pub fn spectrogram(&self, samples: &[f32]) -> Vec<Vec<f32>> {
        let config = &self.config;
        let frames = config.frames(samples.len());
        if frames == 0 {
            return Vec::new();
        }
        let signal = padded(samples, config);
        let bins = config.bins();

        let mut re = vec![0.0f64; config.n_fft];
        let mut im = vec![0.0f64; config.n_fft];
        let mut spectrum = vec![0.0f32; bins];
        let mut out = Vec::with_capacity(frames);

        for frame in 0..frames {
            let start = frame * config.hop_length;
            re.fill(0.0);
            im.fill(0.0);
            // A centered clip's last frames read past the padded end when
            // the signal doesn't divide evenly by the hop; those samples are
            // zero, which is what librosa's own centered tail does too.
            for (i, w) in self.window.iter().enumerate() {
                if let Some(&sample) = signal.get(start + i) {
                    re[i] = sample as f64 * w;
                }
            }
            fft(&mut re, &mut im);
            for (k, value) in spectrum.iter_mut().enumerate() {
                let power = re[k] * re[k] + im[k] * im[k];
                // power == 2 is the common case and squaring is what the
                // FFT already handed us, so the sqrt only runs when a config
                // actually asks for magnitude.
                *value = if config.power == 2.0 {
                    power as f32
                } else {
                    (power.sqrt().powf(config.power as f64)) as f32
                };
            }
            out.push(self.project(&spectrum));
        }
        self.apply_log(&mut out);
        out
    }

    /// One frame's spectrum through the filterbank.
    fn project(&self, spectrum: &[f32]) -> Vec<f32> {
        self.bank
            .iter()
            .map(|weights| {
                // f64 accumulation over a few hundred products: the mel
                // energies span a wide range and the low bands are tiny, and
                // a log of a value that lost its bottom bits is a value that
                // moved.
                let sum: f64 = weights
                    .iter()
                    .zip(spectrum)
                    .map(|(&w, &s)| w as f64 * s as f64)
                    .sum();
                sum as f32
            })
            .collect()
    }

    /// The log, in whichever convention the model was trained with. dB is
    /// the one that reaches across frames: its ceiling is the loudest mel
    /// value in the whole clip, so it needs every frame before it can scale
    /// any of them.
    fn apply_log(&self, frames: &mut [Vec<f32>]) {
        match self.config.log {
            Log::Natural { offset } => {
                for frame in frames.iter_mut() {
                    for value in frame.iter_mut() {
                        *value = (*value + offset).max(f32::MIN_POSITIVE).ln();
                    }
                }
            }
            Log::Base10 { offset } => {
                for frame in frames.iter_mut() {
                    for value in frame.iter_mut() {
                        *value = (*value + offset).max(f32::MIN_POSITIVE).log10();
                    }
                }
            }
            Log::Db { floor, top_db } => {
                let mut peak = f32::MIN;
                for frame in frames.iter_mut() {
                    for value in frame.iter_mut() {
                        *value = 10.0 * value.max(floor).log10();
                        peak = peak.max(*value);
                    }
                }
                if let Some(top_db) = top_db {
                    let cut = peak - top_db;
                    for frame in frames.iter_mut() {
                        for value in frame.iter_mut() {
                            *value = value.max(cut);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// librosa's own defaults, for the structural checks that want a
    /// spectrogram and don't care whose.
    fn librosa_default() -> Config {
        Config {
            sample_rate: 22050,
            n_fft: 2048,
            win_length: 2048,
            hop_length: 512,
            n_mels: 128,
            fmin: 0.0,
            fmax: 11025.0,
            window: WindowKind::Hann,
            center: true,
            power: 2.0,
            scale: Scale::Slaney,
            norm: Norm::Area,
            log: Log::Db {
                floor: 1e-10,
                top_db: Some(80.0),
            },
        }
    }

    /// The two published anchors on Slaney's curve: the linear-to-log join
    /// sits at exactly 1 kHz / 15 mel, and 27 mels above it is exactly
    /// 6.4 kHz. Getting either wrong moves every band center in the bank.
    #[test]
    fn slaney_mels_hit_their_published_anchors() {
        assert!((hz_to_mel(1000.0, Scale::Slaney) - 15.0).abs() < 1e-12);
        assert!((hz_to_mel(6400.0, Scale::Slaney) - 42.0).abs() < 1e-12);
        // Below the join it's a straight 200/3 Hz per mel.
        assert!((hz_to_mel(200.0, Scale::Slaney) - 3.0).abs() < 1e-12);
        assert_eq!(hz_to_mel(0.0, Scale::Slaney), 0.0);
    }

    /// HTK's curve is the one where a mel is roughly a Hz at 1 kHz, which
    /// is the whole reason the constants were picked.
    #[test]
    fn htk_mels_land_near_a_thousand_at_a_kilohertz() {
        assert_eq!(hz_to_mel(0.0, Scale::Htk), 0.0);
        let mel = hz_to_mel(1000.0, Scale::Htk);
        assert!((mel - 1000.0).abs() < 1.0, "expected ~1000, got {mel}");
    }

    #[test]
    fn both_curves_invert_exactly() {
        for scale in [Scale::Htk, Scale::Slaney] {
            for hz in [0.0, 50.0, 700.0, 999.9, 1000.0, 1000.1, 6400.0, 22050.0] {
                let back = mel_to_hz(hz_to_mel(hz, scale), scale);
                assert!(
                    (back - hz).abs() < 1e-8,
                    "{scale:?}: {hz} came back as {back}"
                );
            }
        }
    }

    /// Every triangle peaks at its own center and is zero at its
    /// neighbours' centers, which is what makes the bank a partition of the
    /// spectrum rather than a set of overlapping boxes.
    #[test]
    fn triangles_peak_at_their_center_and_vanish_at_their_feet() {
        let config = Config {
            n_mels: 40,
            norm: Norm::UnitPeak,
            ..librosa_default()
        };
        let bank = filterbank(&config);
        assert_eq!(bank.len(), 40);
        for row in &bank {
            assert_eq!(row.len(), config.bins());
            let peak = row.iter().cloned().fold(0.0f32, f32::max);
            // 40 bands over a 2048-point transform: every band is several
            // bins wide, so every triangle has a bin near its apex.
            assert!(peak > 0.9, "unit-peak triangle only reached {peak}");
            // Weights never go negative, whatever the ramps did.
            assert!(row.iter().all(|&w| w >= 0.0));
        }
        // Bands climb: each row's center of mass sits above the last one's.
        let centers: Vec<f32> = bank
            .iter()
            .map(|row| {
                let total: f32 = row.iter().sum();
                row.iter()
                    .enumerate()
                    .map(|(k, &w)| k as f32 * w)
                    .sum::<f32>()
                    / total
            })
            .collect();
        for pair in centers.windows(2) {
            assert!(pair[0] < pair[1], "band centers went backwards");
        }
    }

    /// Area normalization is the difference between a bank that tilts up
    /// with frequency and one that doesn't. Unit-peak rows get heavier as
    /// the bands widen; area-normalized rows stay put.
    #[test]
    fn area_normalization_flattens_the_tilt_unit_peak_leaves() {
        let unit = filterbank(&Config {
            n_mels: 40,
            norm: Norm::UnitPeak,
            ..librosa_default()
        });
        let area = filterbank(&Config {
            n_mels: 40,
            norm: Norm::Area,
            ..librosa_default()
        });
        let weight = |bank: &[Vec<f32>], row: usize| bank[row].iter().sum::<f32>();
        // The top band of a unit-peak bank passes many times what the
        // bottom one does, purely because it covers more spectrum.
        assert!(weight(&unit, 39) > weight(&unit, 2) * 5.0);
        // Area normalization is what removes that: every row sums to about
        // the same thing once the FFT resolves the band.
        let sums: Vec<f32> = (4..40).map(|row| weight(&area, row)).collect();
        let lo = sums.iter().cloned().fold(f32::MAX, f32::min);
        let hi = sums.iter().cloned().fold(f32::MIN, f32::max);
        assert!(hi / lo < 1.3, "area-normalized rows spread {lo} to {hi}");
    }

    /// Frame counts follow the framing mode, and a clip too short to frame
    /// comes back empty rather than panicking on a slice.
    #[test]
    fn frame_counts_follow_the_framing_mode() {
        let centered = librosa_default();
        // librosa's centered count: 1 + len // hop.
        assert_eq!(centered.frames(22050), 22050 / 512 + 1);
        assert_eq!(centered.frames(0), 1);

        let uncentered = Config {
            center: false,
            ..centered
        };
        assert_eq!(uncentered.frames(22050), (22050 - 2048) / 512 + 1);
        assert_eq!(uncentered.frames(2048), 1);
        assert_eq!(uncentered.frames(2047), 0);

        let mel = Mel::new(uncentered).unwrap();
        assert!(mel.spectrogram(&[0.0; 100]).is_empty());
    }

    /// Reflect padding mirrors without repeating the edge sample, numpy's
    /// rule. A zero pad here would ring across the whole spectrum in the
    /// first and last frames.
    #[test]
    fn reflect_padding_mirrors_without_doubling_the_edge() {
        let config = Config {
            n_fft: 8,
            win_length: 8,
            ..librosa_default()
        };
        let out = padded(&[1.0, 2.0, 3.0, 4.0, 5.0], &config);
        // Pad of 4 either side of a 5-sample clip.
        assert_eq!(
            out,
            vec![5.0, 4.0, 3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0, 2.0, 1.0]
        );
        // A single sample has nothing to mirror and repeats instead.
        assert_eq!(padded(&[7.0], &config), vec![7.0; 9]);
    }

    /// A pure tone lands in the mel bands that cover it and essentially
    /// nowhere else, which is the end-to-end check that the filterbank is
    /// wired to the right FFT bins.
    #[test]
    fn a_tone_lights_the_bands_that_cover_it() {
        let config = Config {
            sample_rate: 16000,
            n_fft: 1024,
            win_length: 1024,
            hop_length: 512,
            n_mels: 64,
            fmin: 0.0,
            fmax: 8000.0,
            window: WindowKind::Hann,
            center: false,
            power: 2.0,
            scale: Scale::Slaney,
            norm: Norm::Area,
            log: Log::Natural { offset: 1e-10 },
        };
        let tone_hz = 1000.0f32;
        let samples: Vec<f32> = (0..16000)
            .map(|i| (std::f32::consts::TAU * tone_hz * i as f32 / config.sample_rate as f32).sin())
            .collect();
        let mel = Mel::new(config).unwrap();
        let frames = mel.spectrogram(&samples);
        assert!(!frames.is_empty());

        // Which band should hold 1 kHz, worked out from the mel spacing
        // rather than from the answer.
        let lo = hz_to_mel(config.fmin as f64, config.scale);
        let hi = hz_to_mel(config.fmax as f64, config.scale);
        let position = (hz_to_mel(tone_hz as f64, config.scale) - lo) / (hi - lo);
        let expected = (position * (config.n_mels + 1) as f64).round() as usize - 1;

        let frame = &frames[frames.len() / 2];
        let loudest = frame
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap()
            .0;
        assert!(
            loudest.abs_diff(expected) <= 1,
            "a {tone_hz} Hz tone peaked in band {loudest}, expected near {expected}"
        );
        // And the far end of the spectrum is decades down, not merely lower.
        assert!(frame[loudest] - frame[config.n_mels - 1] > 10.0);
    }

    /// The dB convention is absolute, not per clip: a reference of 1.0 means
    /// turning a clip up moves every value by the same number of dB rather
    /// than leaving them where they were.
    ///
    /// Worth pinning because the opposite is so widely assumed. librosa's
    /// `power_to_db(ref=np.max)` is the relative one, and models trained
    /// against it see level-independent input; PANNs is trained against
    /// `ref=1.0`, so the absolute level of the recording is part of what it
    /// reads, and mixing the two conventions up costs you a constant offset
    /// on every value.
    #[test]
    fn the_db_convention_is_absolute_and_only_the_floor_is_relative() {
        let mel = Mel::new(Config {
            log: Log::Db {
                floor: 1e-10,
                top_db: None,
            },
            ..librosa_default()
        })
        .unwrap();
        let quiet: Vec<f32> = (0..8192).map(|i| 0.01 * (i as f32 * 0.37).sin()).collect();
        let loud: Vec<f32> = quiet.iter().map(|s| s * 10.0).collect();
        let a = mel.spectrogram(&quiet);
        let b = mel.spectrogram(&loud);
        // Ten times the amplitude is a hundred times the power, which is
        // exactly 20 dB, everywhere the floor isn't in the way.
        for (fa, fb) in a.iter().zip(&b) {
            for (x, y) in fa.iter().zip(fb) {
                if *x > -99.0 {
                    assert!((y - x - 20.0).abs() < 0.01, "{x} became {y}");
                }
            }
        }

        // With a ceiling, the clamp is the one relative piece: nothing sits
        // more than top_db under the loudest value in the clip.
        let clamped = Mel::new(librosa_default()).unwrap().spectrogram(&quiet);
        let peak = clamped.iter().flatten().cloned().fold(f32::MIN, f32::max);
        assert!(clamped.iter().flatten().all(|&v| v >= peak - 80.0 - 1e-3));
        assert!(
            clamped
                .iter()
                .flatten()
                .any(|&v| (v - (peak - 80.0)).abs() < 1e-3),
            "something should actually be sitting on the clamp"
        );
    }

    /// PANNs CNN10's recipe, the one the golden values below were generated
    /// against. Spelled out here rather than imported from the model catalog
    /// so this test fails if either copy drifts.
    fn panns() -> Config {
        Config {
            sample_rate: 32_000,
            n_fft: 1024,
            win_length: 1024,
            hop_length: 320,
            n_mels: 64,
            fmin: 50.0,
            fmax: 14_000.0,
            window: WindowKind::Hann,
            center: true,
            power: 2.0,
            scale: Scale::Slaney,
            norm: Norm::Area,
            log: Log::Db {
                floor: 1e-10,
                top_db: None,
            },
        }
    }

    /// The test signal the golden values were taken over: three tones an
    /// octave-and-a-half apart with offset phases, one second at 32 kHz.
    /// Deterministic and identical on both sides, which is the only reason
    /// a golden comparison means anything.
    fn golden_signal() -> Vec<f32> {
        let rate = 32_000.0f64;
        (0..32_000)
            .map(|i| {
                let t = i as f64 / rate;
                let tau = std::f64::consts::TAU;
                (0.6 * (tau * 440.0 * t).sin()
                    + 0.3 * (tau * 3000.0 * t + 1.0).sin()
                    + 0.1 * (tau * 11000.0 * t + 2.0).sin()) as f32
            })
            .collect()
    }

    /// Each band's average over the 101 frames of [`golden_signal`], as
    /// librosa 0.11 computes it:
    ///
    /// ```python
    /// S = np.abs(librosa.stft(x, n_fft=1024, hop_length=320, win_length=1024,
    ///                         window='hann', center=True, pad_mode='reflect'))**2
    /// mel = librosa.filters.mel(sr=32000, n_fft=1024, n_mels=64, fmin=50, fmax=14000)
    /// L = (10.0 * np.log10(np.maximum(mel.dot(S), 1e-10))).T
    /// L.mean(axis=0)
    /// ```
    ///
    /// Note `pad_mode='reflect'`: librosa's own default went to `'constant'`
    /// in 0.10, and torchlibrosa (which is what PANNs actually trains
    /// through) asks for reflect, so the default would compare the wrong
    /// thing.
    const LIBROSA_BAND_MEANS: [f32; 64] = [
        -52.5597, -47.6234, -41.7836, -34.1515, -24.0316, 15.4932, 26.3506, 22.5684, -12.9836,
        -30.4306, -39.2264, -45.4518, -50.4682, -55.1357, -58.6262, -61.6374, -64.3298, -67.3219,
        -69.6228, -71.9891, -74.3194, -76.4764, -78.8137, -80.9949, -83.1867, -85.0203, -86.9208,
        -88.8974, -90.7530, -92.2842, -93.9891, -95.2589, -96.1197, -96.5563, -96.4701, -96.2078,
        15.6040, 12.1844, -96.3341, -96.5299, -96.6542, -96.7505, -96.8303, -96.8999, -96.9629,
        -97.0211, -97.0753, -97.1266, -97.1765, -97.2251, -97.2733, -97.3222, -97.3745, -97.4339,
        -97.5123, -97.6466, -97.6115, -97.3959, -97.1781, -2.5008, 0.1803, -97.1043, -97.3830,
        -97.5625,
    ];

    /// The whole front end against librosa, band by band, on PANNs' own
    /// recipe. This is the test that says the spectrogram is right rather
    /// than merely self-consistent: framing, window, padding, transform,
    /// filterbank, and log all have to agree at once for these numbers to
    /// come out.
    ///
    /// Two tolerances, for an honest reason. The bands carrying signal are
    /// compared to a hundredth of a dB; the bands sitting near the -100 dB
    /// floor hold nothing but window leakage, where this transform's f64
    /// arithmetic and librosa's f32 genuinely differ, and a tight bound
    /// there would be measuring float noise rather than correctness.
    #[test]
    fn the_whole_front_end_matches_librosa_band_by_band() {
        let mel = Mel::new(panns()).unwrap();
        let frames = mel.spectrogram(&golden_signal());
        assert_eq!(frames.len(), 101, "librosa framed this clip into 101");

        for (band, &expected) in LIBROSA_BAND_MEANS.iter().enumerate() {
            let mine: f32 = frames.iter().map(|frame| frame[band]).sum::<f32>() / 101.0;
            let tolerance = if expected > -90.0 { 0.01 } else { 0.5 };
            assert!(
                (mine - expected).abs() < tolerance,
                "band {band}: {mine} against librosa's {expected}"
            );
        }
    }

    /// The tones land where librosa puts them and nowhere else, which is
    /// the same claim as the band-by-band check stated so a reader can see
    /// what it means: three tones, three bands, the rest at the floor.
    #[test]
    fn the_golden_signals_three_tones_land_in_three_bands() {
        let mel = Mel::new(panns()).unwrap();
        let frames = mel.spectrogram(&golden_signal());
        let means: Vec<f32> = (0..64)
            .map(|band| frames.iter().map(|frame| frame[band]).sum::<f32>() / 101.0)
            .collect();
        // 440 Hz, 3 kHz and 11 kHz, in the bands librosa's own mel spacing
        // puts them in.
        for band in [6usize, 36, 60] {
            assert!(means[band] > 0.0, "band {band} should carry a tone");
        }
        // And every band four or more away from all three is at the floor.
        for (band, &mean) in means.iter().enumerate() {
            if [6usize, 36, 60].iter().all(|t| band.abs_diff(*t) > 3) {
                assert!(mean < -20.0, "band {band} has no tone but reads {mean}");
            }
        }
    }

    /// A config the transform can't run says so at build time rather than
    /// panicking somewhere in the middle of a library pass.
    #[test]
    fn an_impossible_config_is_refused_up_front() {
        let base = librosa_default();
        assert!(Mel::new(Config {
            n_fft: 1000,
            ..base
        })
        .is_err());
        assert!(Mel::new(Config {
            win_length: 4096,
            ..base
        })
        .is_err());
        assert!(Mel::new(Config {
            hop_length: 0,
            ..base
        })
        .is_err());
        // fmax past Nyquist means the config was written for another rate.
        assert!(Mel::new(Config {
            fmax: 12000.0,
            ..base
        })
        .is_err());
        assert!(Mel::new(base).is_ok());
    }
}
