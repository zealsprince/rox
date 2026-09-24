//! Log-mel spectrograms, the input every acoustic model here takes.
//!
//! A network's mel front end is part of the network: a different recipe
//! yields vectors that look fine and mean nothing. So every knob a training
//! config sets is a [`Config`] field, stated per model. The ones that bite
//! hardest: the log convention, the mel scale (HTK or Slaney), filterbank
//! normalization, power, and centered or uncentered framing.
//!
//! Not `rox_viz::analysis::log_bands`, which sums bins into display bands
//! with no triangles, curve, or normalization.
//!
//! Pinned against librosa 0.11 golden values
//! (`the_whole_front_end_matches_librosa_band_by_band`), since most of these
//! models trained on librosa underneath.

/// Rarely documented, and the two move every band center.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    /// HTK, torchaudio's default, and TensorFlow.
    Htk,
    /// Linear below 1 kHz. Slaney's Auditory Toolbox and librosa's default.
    Slaney,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Norm {
    /// Wide high bands pass more energy than narrow low ones.
    UnitPeak,
    /// Flat spectrum in, flat mel out. librosa's `norm="slaney"`, its default.
    Area,
}

/// Periodic (`fftbins=True`), as every spectrogram library uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Hann,
    Hamming,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Log {
    /// What most PyTorch audio training code ends up with.
    Natural { offset: f32 },
    /// `log10(x + offset)`.
    Base10 { offset: f32 },
    /// librosa's `power_to_db`. The `top_db` clamp is relative to the loudest
    /// value in the clip, so it depends on the whole clip.
    Db { floor: f32, top_db: Option<f32> },
}

/// One model's recipe, copied from its training config. Every field matters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// The rate audio must be resampled to first.
    pub sample_rate: u32,
    /// Powers of two only (radix-2); a 400-sample window goes in `win_length`.
    pub n_fft: usize,
    /// Zero-padded to `n_fft` and centered, like librosa's `util.pad_center`.
    pub win_length: usize,
    pub hop_length: usize,
    pub n_mels: usize,
    pub fmin: f32,
    /// Never above Nyquist.
    pub fmax: f32,
    pub window: WindowKind,
    /// Reflect-pad by `n_fft / 2` so frame `t` centers on sample `t * hop`.
    /// librosa: true; TensorFlow: false.
    pub center: bool,
    /// 1.0 magnitude, 2.0 power.
    pub power: f32,
    pub scale: Scale,
    pub norm: Norm,
    pub log: Log,
}

impl Config {
    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Zero for a clip shorter than one uncentered window.
    pub fn frames(&self, samples: usize) -> usize {
        if self.center {
            samples / self.hop_length + 1
        } else if samples < self.n_fft {
            0
        } else {
            (samples - self.n_fft) / self.hop_length + 1
        }
    }

    /// Checked at load: a catalog entry is data, and data gets edited.
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

pub fn hz_to_mel(hz: f64, scale: Scale) -> f64 {
    match scale {
        Scale::Htk => 2595.0 * (1.0 + hz / 700.0).log10(),
        Scale::Slaney => {
            // Linear at 200/3 Hz per mel to 1 kHz, then log with 27 mels per factor of
            // 6.4. Mel 15 is exactly 1 kHz, mel 42 exactly 6.4 kHz.
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

/// The exact inverse of [`hz_to_mel`].
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

/// librosa's `filters.mel`: `n_mels` triangles over `n_mels + 2` mel-spaced
/// points. Rows can be all zero when the FFT is too coarse for a band, which
/// is a real outcome, not an error.
pub fn filterbank(config: &Config) -> Vec<Vec<f32>> {
    let bins = config.bins();
    // Exactly linspace(0, sr/2, bins).
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
            // Slaney area norm, or the wide top bands dominate.
            let gain = match config.norm {
                Norm::UnitPeak => 1.0,
                Norm::Area => 2.0 / (right - left),
            };
            fft_hz
                .iter()
                .map(|&hz| {
                    // Each ramp guards its own zero width: coarse FFTs duplicate points.
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

/// Centered framing reflect-pads half a transform at each end. Reflection,
/// not zeros: a zero pad's step rings across the first and last frames.
fn padded(samples: &[f32], config: &Config) -> Vec<f32> {
    // Nothing to mirror, and the period below would be negative.
    if !config.center || samples.is_empty() {
        return samples.to_vec();
    }
    let pad = config.n_fft / 2;
    let mut out = Vec::with_capacity(samples.len() + 2 * pad);
    // numpy's "reflect": no repeated edge, so a pad of 3 over [a b c d]
    // prepends [d c b]. Short clips bounce off both ends.
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

/// Radix-2, f64. rox-viz has its own private f32 one for the spectrum bars.
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

/// Holds the window and filterbank, so a library pass builds them once.
pub struct Mel {
    config: Config,
    window: Vec<f64>,
    bank: Vec<Vec<f32>>,
}

impl Mel {
    pub fn new(config: Config) -> Result<Self, String> {
        config.valid()?;
        Ok(Mel {
            window: window(&config),
            bank: filterbank(&config),
            config,
        })
    }

    /// With a supplied filterbank, for models like PANNs that ship theirs.
    /// [`Mel::bank_deviation`] still says whether the config matches it.
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

    /// The largest weight difference from the config's bank, relative to its
    /// peak: whether the config really is the recipe these weights were
    /// trained with.
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

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Frame-major log-mel of a mono clip; empty if too short to frame.
    /// `samples` must already be at [`Config::sample_rate`]: resampling belongs
    /// to the decode, where the source rate is known.
    pub fn spectrogram(&self, samples: &[f32]) -> Vec<Vec<f32>> {
        let config = &self.config;
        // An empty clip, which a one-sample read resampled down produces, has no
        // frame to transform.
        if samples.is_empty() {
            return Vec::new();
        }
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

        // A short window sits centered in the frame, as librosa pads it; its slot
        // is only a phase shift the power spectrum drops.
        let offset = (config.n_fft - config.win_length) / 2;
        for frame in 0..frames {
            let start = frame * config.hop_length + offset;
            re.fill(0.0);
            im.fill(0.0);
            // Reads past the padded end are zero, as librosa's centered tail.
            for (i, w) in self.window.iter().enumerate() {
                if let Some(&sample) = signal.get(start + i) {
                    re[offset + i] = sample as f64 * w;
                }
            }
            fft(&mut re, &mut im);
            for (k, value) in spectrum.iter_mut().enumerate() {
                let power = re[k] * re[k] + im[k] * im[k];
                // The FFT already gives power; only take the sqrt for another exponent.
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

    fn project(&self, spectrum: &[f32]) -> Vec<f32> {
        self.bank
            .iter()
            .map(|weights| {
                // f64 accumulation: the low bands are tiny, and lost bits move the log.
                let sum: f64 = weights
                    .iter()
                    .zip(spectrum)
                    .map(|(&w, &s)| w as f64 * s as f64)
                    .sum();
                sum as f32
            })
            .collect()
    }

    /// dB needs every frame first: its ceiling is the clip's loudest value.
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

    /// Slaney's published anchors: 1 kHz is mel 15, 6.4 kHz mel 42.
    #[test]
    fn slaney_mels_hit_their_published_anchors() {
        assert!((hz_to_mel(1000.0, Scale::Slaney) - 15.0).abs() < 1e-12);
        assert!((hz_to_mel(6400.0, Scale::Slaney) - 42.0).abs() < 1e-12);
        assert!((hz_to_mel(200.0, Scale::Slaney) - 3.0).abs() < 1e-12);
        assert_eq!(hz_to_mel(0.0, Scale::Slaney), 0.0);
    }

    /// A mel is roughly a Hz at 1 kHz.
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

    /// Each triangle peaks at its center and vanishes at its neighbours'.
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
            // 40 bands over 2048 points: every triangle has a bin near its apex.
            assert!(peak > 0.9, "unit-peak triangle only reached {peak}");
            assert!(row.iter().all(|&w| w >= 0.0));
        }
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
        // Unit-peak's top band passes many times the bottom's.
        assert!(weight(&unit, 39) > weight(&unit, 2) * 5.0);
        // Area-normalized rows sum to about the same once resolved.
        let sums: Vec<f32> = (4..40).map(|row| weight(&area, row)).collect();
        let lo = sums.iter().cloned().fold(f32::MAX, f32::min);
        let hi = sums.iter().cloned().fold(f32::MIN, f32::max);
        assert!(hi / lo < 1.3, "area-normalized rows spread {lo} to {hi}");
    }

    /// A clip too short to frame returns empty, never panics.
    #[test]
    fn frame_counts_follow_the_framing_mode() {
        let centered = librosa_default();
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

    /// Empty in, empty out, in either framing: a one-sample read resampled down
    /// is zero samples, and would otherwise run the reflect pad off an empty slice.
    #[test]
    fn a_clip_of_no_samples_describes_nothing() {
        let centered = librosa_default();
        assert!(Mel::new(centered).unwrap().spectrogram(&[]).is_empty());
        assert!(padded(&[], &centered).is_empty());
        let uncentered = Config {
            center: false,
            ..centered
        };
        assert!(Mel::new(uncentered).unwrap().spectrogram(&[]).is_empty());
    }

    /// A short window covers the middle of its frame, like `pad_center`.
    /// Nothing shipped uses one, which is why it's pinned.
    #[test]
    fn a_short_window_covers_the_middle_of_its_frame() {
        let config = Config {
            sample_rate: 16_000,
            n_fft: 8,
            win_length: 4,
            hop_length: 8,
            n_mels: 2,
            fmin: 0.0,
            fmax: 8000.0,
            window: WindowKind::Hann,
            center: false,
            power: 2.0,
            scale: Scale::Slaney,
            norm: Norm::Area,
            log: Log::Natural { offset: 1e-10 },
        };
        let mel = Mel::new(config).unwrap();
        // The window covers samples 2 through 5 only.
        let covered = mel.spectrogram(&[0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0]);
        let noisy_edges = mel.spectrogram(&[9.0, 9.0, 1.0, 1.0, 1.0, 1.0, 9.0, 9.0]);
        assert_eq!(covered.len(), 1);
        assert_eq!(
            covered, noisy_edges,
            "samples outside the window changed the frame"
        );
        let changed = mel.spectrogram(&[0.0, 0.0, 1.0, 1.0, 2.0, 1.0, 0.0, 0.0]);
        assert_ne!(covered, changed);
    }

    #[test]
    fn reflect_padding_mirrors_without_doubling_the_edge() {
        let config = Config {
            n_fft: 8,
            win_length: 8,
            ..librosa_default()
        };
        let out = padded(&[1.0, 2.0, 3.0, 4.0, 5.0], &config);
        assert_eq!(
            out,
            vec![
                5.0, 4.0, 3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0, 2.0, 1.0
            ]
        );
        assert_eq!(padded(&[7.0], &config), vec![7.0; 9]);
    }

    /// A tone lands only in its bands: the bank is wired to the right bins.
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

        // Worked out from the mel spacing, not from the answer.
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
        // Decades down at the far end.
        assert!(frame[loudest] - frame[config.n_mels - 1] > 10.0);
    }

    /// Absolute dB (ref=1.0): scaling a clip shifts every value. PANNs trained
    /// this way; librosa's `ref=np.max` is relative, and mixing them up offsets
    /// every value.
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
        // 10x amplitude is exactly +20 dB above the floor.
        for (fa, fb) in a.iter().zip(&b) {
            for (x, y) in fa.iter().zip(fb) {
                if *x > -99.0 {
                    assert!((y - x - 20.0).abs() < 0.01, "{x} became {y}");
                }
            }
        }

        // The top_db clamp is the only relative piece.
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

    /// Spelled out, not imported, so drift in either copy fails.
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

    /// Three tones with offset phases, one second at 32 kHz, identical on both sides.
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
    /// `pad_mode='reflect'`: librosa's default became `'constant'` in 0.10, but
    /// torchlibrosa, which PANNs trained through, asks for reflect.
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

    /// The whole front end against librosa, band by band. Signal bands to 0.01
    /// dB; bands near the -100 dB floor hold only leakage, where f64 and
    /// librosa's f32 differ, so they get 0.5.
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

    /// Three tones, three bands, the rest at the floor.
    #[test]
    fn the_golden_signals_three_tones_land_in_three_bands() {
        let mel = Mel::new(panns()).unwrap();
        let frames = mel.spectrogram(&golden_signal());
        let means: Vec<f32> = (0..64)
            .map(|band| frames.iter().map(|frame| frame[band]).sum::<f32>() / 101.0)
            .collect();
        for band in [6usize, 36, 60] {
            assert!(means[band] > 0.0, "band {band} should carry a tone");
        }
        for (band, &mean) in means.iter().enumerate() {
            if [6usize, 36, 60].iter().all(|t| band.abs_diff(*t) > 3) {
                assert!(mean < -20.0, "band {band} has no tone but reads {mean}");
            }
        }
    }

    #[test]
    fn an_impossible_config_is_refused_up_front() {
        let base = librosa_default();
        assert!(
            Mel::new(Config {
                n_fft: 1000,
                ..base
            })
            .is_err()
        );
        assert!(
            Mel::new(Config {
                win_length: 4096,
                ..base
            })
            .is_err()
        );
        assert!(
            Mel::new(Config {
                hop_length: 0,
                ..base
            })
            .is_err()
        );
        assert!(
            Mel::new(Config {
                fmax: 12000.0,
                ..base
            })
            .is_err()
        );
        assert!(Mel::new(base).is_ok());
    }
}
