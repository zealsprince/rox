//! Modulation sources for audio-reactive parameters: the groundwork for
//! tying any panel knob to the music. A [`Binding`] routes one [`Source`]
//! (a frequency band's energy, or the whole mix's level) through response
//! smoothing into an output span over some host-defined parameter; the
//! [`Signals`] engine folds the spectrum into one smoothed value per
//! binding each frame. The engine is pure DSP: the host hands it the
//! magnitudes it already computed, so one FFT serves the emitters and the
//! bindings alike. What a target id means, and how a fraction maps into a
//! parameter's native units, stays with the host panel.

use serde::{Deserialize, Serialize};

use crate::analysis::log_bands;

/// dB window signals normalize into, on magnitudes where a full-scale sine
/// sits at 0 dB. The same window the spectrum's bars and the particles'
/// activations read against, so a signal level means the same loudness
/// everywhere.
pub const FLOOR_DB: f32 = -66.0;
pub const MAX_DB: f32 = -12.0;

/// The band a source may watch. Matches the hearing-range span the audio
/// panels' sliders cover.
pub const BAND_MIN_HZ: f32 = 20.0;
pub const BAND_MAX_HZ: f32 = 20_000.0;

/// What a binding listens to.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum Source {
    /// Peak magnitude across a frequency band, normalized into the dB
    /// window: a kick band, a hat band, wherever the ear points.
    Band { lo: f32, hi: f32 },
    /// The whole spectrum's peak: the mix's loudness at a glance.
    Level,
    /// A pulse on each transient in the band: 1 the moment the band jumps
    /// past its own recent average, decaying at the response rate. The
    /// signal a hit rides, where Band is the signal a swell rides.
    Onset { lo: f32, hi: f32 },
}

impl Source {
    /// The watched bin span for this source, clamped so a hand-edited file
    /// can't invert the band or walk off the spectrum.
    fn bins(&self, sample_rate: u32, half: usize) -> (usize, usize) {
        let (lo, hi) = match *self {
            Source::Band { lo, hi } | Source::Onset { lo, hi } => {
                let lo = lo.clamp(BAND_MIN_HZ, BAND_MAX_HZ);
                (lo, hi.clamp(lo * 1.01, BAND_MAX_HZ))
            }
            Source::Level => (BAND_MIN_HZ, BAND_MAX_HZ),
        };
        log_bands(1, lo, hi, sample_rate, half)[0]
    }
}

/// One modulation route: what it listens to, how it responds, and the span
/// it sweeps. `from`/`to` are fractions of the target parameter's own
/// range - the value at silence and the value at full signal - so a binding
/// sweeps exactly what a hand on the slider could, and an inverted span
/// (`from` above `to`) modulates downward.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Binding {
    /// Whether the route applies. Off keeps it in the list, tuned, silent.
    pub enabled: bool,
    /// The parameter this drives: an id the host panel defines. Unknown ids
    /// are ignored, so a config from a newer build degrades quietly.
    pub target: String,
    pub source: Source,
    /// Response smoothing, 0 to 1: 0 snaps to the music, 1 drifts after it.
    pub smooth: f32,
    /// The output span, fractions of the target's range.
    pub from: f32,
    pub to: f32,
}

impl Default for Binding {
    fn default() -> Self {
        Binding {
            enabled: true,
            target: String::new(),
            source: Source::Band {
                lo: 30.0,
                hi: 120.0,
            },
            smooth: 0.3,
            from: 0.0,
            to: 1.0,
        }
    }
}

impl Binding {
    fn smooth(&self) -> f32 {
        self.smooth.clamp(0.0, 1.0)
    }
}

/// The per-second easing rates the smoothing knob spans, snappy end and
/// floaty end, interpolated exponentially so the knob's travel feels even.
const ATTACK_FAST: f32 = 50.0;
const ATTACK_SLOW: f32 = 3.0;
const RELEASE_FAST: f32 = 12.0;
const RELEASE_SLOW: f32 = 1.0;

/// The onset detector's shape: the pulse decay rates the response knob
/// spans, how fast the reference envelope chases the band, how far past
/// the reference the band must jump to read as a hit, and the level below
/// which nothing counts, so the noise floor can't fire it.
const ONSET_DECAY_FAST: f32 = 16.0;
const ONSET_DECAY_SLOW: f32 = 1.5;
const ONSET_REF_ATTACK: f32 = 2.5;
const ONSET_REF_RELEASE: f32 = 2.0;
const ONSET_MARGIN: f32 = 0.12;
const ONSET_FLOOR: f32 = 0.15;

/// The engine: one smoothed value per binding, positionally aligned with
/// the host's list the way the particle sim aligns activations with its
/// emitters. Editing a binding keeps its slot; adding or removing resets
/// only from the change down, which a live field absorbs without a blink.
pub struct Signals {
    values: Vec<f32>,
    /// The onset slots' slow reference envelope: what "louder than before"
    /// is measured against. Idle for the other source kinds.
    refs: Vec<f32>,
    /// Whether each onset slot is ready to fire again, re-armed once its
    /// band falls back toward the reference.
    armed: Vec<bool>,
}

impl Signals {
    pub fn new() -> Self {
        Signals {
            values: Vec::new(),
            refs: Vec::new(),
            armed: Vec::new(),
        }
    }

    /// The current signal per binding, what [`Self::step`] last returned:
    /// for meters that read between steps, off another window's clock.
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Fold one frame into the signals. `mags` is the newest half-spectrum
    /// when a fresh window landed this frame, `None` between windows, where
    /// values hold rather than dip; `stopped` releases everything toward
    /// zero once the host decides the audio has actually ended.
    pub fn step(
        &mut self,
        mags: Option<&[f32]>,
        sample_rate: u32,
        stopped: bool,
        dt: f32,
        bindings: &[Binding],
    ) -> &[f32] {
        self.values.resize(bindings.len(), 0.0);
        self.refs.resize(bindings.len(), 0.0);
        self.armed.resize(bindings.len(), true);
        for (i, binding) in bindings.iter().enumerate() {
            let smooth = binding.smooth();
            let raw = mags.map(|mags| {
                let (lo, hi) = binding.source.bins(sample_rate, mags.len());
                let mut peak = 0.0f32;
                for &m in &mags[lo..hi] {
                    peak = peak.max(m);
                }
                let db = 20.0 * (peak + 1e-9).log10();
                ((db - FLOOR_DB) / (MAX_DB - FLOOR_DB)).clamp(0.0, 1.0)
            });
            let value = &mut self.values[i];
            match binding.source {
                Source::Band { .. } | Source::Level => {
                    let attack = ATTACK_FAST * (ATTACK_SLOW / ATTACK_FAST).powf(smooth);
                    let release = RELEASE_FAST * (RELEASE_SLOW / RELEASE_FAST).powf(smooth);
                    if let Some(raw) = raw {
                        let ease = if raw > *value { attack } else { release };
                        *value += (raw - *value) * (ease * dt).min(1.0);
                    } else if stopped {
                        *value += (0.0 - *value) * (release * dt).min(1.0);
                    }
                }
                Source::Onset { .. } => {
                    // The pulse decays on every frame; the trigger reads
                    // against the reference before the reference catches up,
                    // so a jump lands whole.
                    let decay =
                        ONSET_DECAY_FAST * (ONSET_DECAY_SLOW / ONSET_DECAY_FAST).powf(smooth);
                    *value -= *value * (decay * dt).min(1.0);
                    if let Some(raw) = raw {
                        let reference = self.refs[i];
                        if self.armed[i] && raw > ONSET_FLOOR && raw > reference + ONSET_MARGIN {
                            *value = 1.0;
                            self.armed[i] = false;
                        } else if !self.armed[i] && raw < reference + ONSET_MARGIN * 0.5 {
                            self.armed[i] = true;
                        }
                        let ease = if raw > reference {
                            ONSET_REF_ATTACK
                        } else {
                            ONSET_REF_RELEASE
                        };
                        self.refs[i] += (raw - reference) * (ease * dt).min(1.0);
                    } else if stopped {
                        self.refs[i] -= self.refs[i] * (ONSET_REF_RELEASE * dt).min(1.0);
                        self.armed[i] = true;
                    }
                }
            }
        }
        &self.values
    }
}

impl Default for Signals {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(lo: f32, hi: f32) -> Binding {
        Binding {
            source: Source::Band { lo, hi },
            smooth: 0.0,
            ..Binding::default()
        }
    }

    #[test]
    fn tracks_the_length_of_the_binding_list() {
        let mut signals = Signals::new();
        let bindings = vec![band(30.0, 120.0), band(2000.0, 8000.0)];
        assert_eq!(signals.step(None, 48_000, false, 0.016, &bindings).len(), 2);
        assert_eq!(signals.step(None, 48_000, false, 0.016, &[]).len(), 0);
    }

    #[test]
    fn loud_band_rises_quiet_band_stays_down() {
        let mut signals = Signals::new();
        // Energy in bin 100 of a 2048-bin half-spectrum at 48 kHz: about
        // 1.17 kHz. A midrange binding should light up, a top-end one not.
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        let bindings = vec![band(800.0, 2000.0), band(8000.0, 16000.0)];
        let mut out = [0.0; 2];
        for _ in 0..30 {
            let v = signals.step(Some(&mags), 48_000, false, 0.016, &bindings);
            out.copy_from_slice(v);
        }
        assert!(out[0] > 0.9, "hot band should saturate, got {}", out[0]);
        assert!(
            out[1] < 0.05,
            "cold band should stay near zero, got {}",
            out[1]
        );
    }

    #[test]
    fn holds_between_windows_and_releases_when_stopped() {
        let mut signals = Signals::new();
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        let bindings = vec![band(800.0, 2000.0)];
        for _ in 0..30 {
            signals.step(Some(&mags), 48_000, false, 0.016, &bindings);
        }
        // A gap between pump ticks holds the value.
        let held = signals.step(None, 48_000, false, 0.016, &bindings)[0];
        assert!(held > 0.9, "gap should hold, got {held}");
        // A stop releases it toward zero.
        for _ in 0..120 {
            signals.step(None, 48_000, true, 0.016, &bindings);
        }
        let released = signals.step(None, 48_000, true, 0.016, &bindings)[0];
        assert!(released < 0.05, "stop should release, got {released}");
    }

    #[test]
    fn onset_fires_once_decays_and_rearms() {
        let mut signals = Signals::new();
        let bindings = vec![Binding {
            source: Source::Onset {
                lo: 800.0,
                hi: 2000.0,
            },
            smooth: 0.0,
            ..Binding::default()
        }];
        let quiet = vec![0.0f32; 2048];
        let mut loud = vec![0.0f32; 2048];
        loud[100] = 1.0;

        // A hit out of silence pulses to 1.
        signals.step(Some(&quiet), 48_000, false, 0.016, &bindings);
        let hit = signals.step(Some(&loud), 48_000, false, 0.016, &bindings)[0];
        assert!(hit > 0.9, "onset should pulse on the jump, got {hit}");

        // Sustained loudness does not retrigger; the pulse decays away.
        for _ in 0..60 {
            signals.step(Some(&loud), 48_000, false, 0.016, &bindings);
        }
        let sustained = signals.step(Some(&loud), 48_000, false, 0.016, &bindings)[0];
        assert!(
            sustained < 0.1,
            "sustain should not hold the pulse, got {sustained}"
        );

        // Quiet re-arms it; the next hit fires again.
        for _ in 0..120 {
            signals.step(Some(&quiet), 48_000, false, 0.016, &bindings);
        }
        let again = signals.step(Some(&loud), 48_000, false, 0.016, &bindings)[0];
        assert!(again > 0.9, "onset should re-arm after quiet, got {again}");
    }

    #[test]
    fn inverted_band_from_a_hand_edited_file_does_not_panic() {
        let mut signals = Signals::new();
        let mags = vec![0.0f32; 1024];
        let bindings = vec![band(5000.0, 40.0), band(-10.0, 1e9)];
        signals.step(Some(&mags), 48_000, false, 0.016, &bindings);
    }
}
