//! The source-gain stage (ADR 19): gain applied to each decoded source before
//! the sources sum. Not a chain node, because a crossfade has two tracks live
//! and a node on the mix would apply one track's factor to both.
//!
//! ReplayGain gives a constant factor, crossfade a per-frame pair. Unity
//! short-circuits, so the bypass rule holds here too.

use std::f32::consts::FRAC_PI_2;

/// A file's ReplayGain tags. Any mix of the four can be missing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReplayGain {
    pub track_db: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_db: Option<f32>,
    pub album_peak: Option<f32>,
}

impl ReplayGain {
    fn track(self) -> Option<(f32, Option<f32>)> {
        self.track_db.map(|db| (db, self.track_peak))
    }

    fn album(self) -> Option<(f32, Option<f32>)> {
        self.album_db.map(|db| (db, self.album_peak))
    }

    pub fn any(self) -> bool {
        self.track_db.is_some() || self.album_db.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GainMode {
    /// Unity everywhere: the bypass rule.
    #[default]
    Off,
    Track,
    Album,
}

/// Which gain to read and the two offsets on top. Swappable mid-stream.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GainRule {
    pub mode: GainMode,
    /// Added to every tagged gain: ReplayGain's reference sits well below modern
    /// masters, and this takes that back.
    pub preamp_db: f32,
    /// What an untagged file plays at. Separate from the preamp: there's no tag to
    /// offset from.
    pub fallback_db: f32,
}

/// Clamp on the dB that reaches the multiply, so a garbage tag can't become a
/// factor of a thousand.
const DB_LIMIT: f32 = 40.0;

impl GainRule {
    /// Exactly 1.0 when there's nothing to apply, so [`apply`] short-circuits.
    pub fn factor(&self, rg: ReplayGain) -> f32 {
        let (db, peak) = match self.mode {
            GainMode::Off => return 1.0,
            // Either mode falls back to the other gain, with its own peak.
            GainMode::Track => match rg.track().or_else(|| rg.album()) {
                Some((db, peak)) => (db + self.preamp_db, peak),
                None => (self.fallback_db, None),
            },
            GainMode::Album => match rg.album().or_else(|| rg.track()) {
                Some((db, peak)) => (db + self.preamp_db, peak),
                None => (self.fallback_db, None),
            },
        };
        let factor = db_to_linear(db);
        // The tagged peak clamps the factor so a boost can't clip (ADR 19). Only
        // downward.
        match peak {
            Some(peak) if peak > 0.0 => factor.min(1.0 / peak),
            _ => factor,
        }
    }
}

/// dB to a linear multiplier, over the range a gain knob allows.
fn db_to_linear(db: f32) -> f32 {
    if db == 0.0 {
        // Exactly unity, without relying on powf returning it.
        return 1.0;
    }
    10f32.powf(db.clamp(-DB_LIMIT, DB_LIMIT) / 20.0)
}

/// Exactly unity is a no-op, so the samples stay the decoder's bits.
pub fn apply(buf: &mut [f32], gain: f32) {
    if gain == 1.0 {
        return;
    }
    for sample in buf {
        *sample *= gain;
    }
}

/// The fade pair at `t` in 0..1: (incoming, outgoing).
///
/// Equal power, sine over cosine: the two tracks are uncorrelated, so a
/// linear pair would dip audibly in the middle.
pub fn crossfade(t: f32) -> (f32, f32) {
    let (sin, cos) = (t.clamp(0.0, 1.0) * FRAC_PI_2).sin_cos();
    (sin, cos)
}

/// Mix `outgoing` under `incoming` from frame `done` of a `len`-frame window.
/// A short `outgoing` reads as silence.
pub fn crossfade_mix(incoming: &mut [f32], outgoing: &[f32], done: u64, len: u64) {
    let len = len.max(1) as f32;
    for (i, frame) in incoming.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        let (g_in, g_out) = crossfade((done + i as u64) as f32 / len);
        let l = outgoing.get(i * 2).copied().unwrap_or(0.0);
        let r = outgoing.get(i * 2 + 1).copied().unwrap_or(0.0);
        frame[0] = frame[0] * g_in + l * g_out;
        frame[1] = frame[1] * g_in + r * g_out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_gain_is_bit_exact() {
        let original = vec![0.1f32, -0.5, 1.0, f32::MIN_POSITIVE];
        let mut buf = original.clone();
        apply(&mut buf, 1.0);
        assert_eq!(buf, original, "unity never touches the samples");
    }

    #[test]
    fn gain_scales_every_sample() {
        let mut buf = vec![0.5f32, -0.25];
        apply(&mut buf, 0.5);
        assert_eq!(buf, vec![0.25, -0.125]);
    }

    fn both() -> ReplayGain {
        ReplayGain {
            track_db: Some(-6.0),
            track_peak: Some(0.5),
            album_db: Some(-3.0),
            album_peak: Some(0.9),
        }
    }

    #[test]
    fn off_is_exactly_unity() {
        // Leveling off must leave the decoder's bits, not multiply by a rounded 1.0.
        let rule = GainRule {
            mode: GainMode::Off,
            preamp_db: 6.0,
            fallback_db: -6.0,
        };
        assert_eq!(rule.factor(both()), 1.0);
    }

    #[test]
    fn each_mode_reads_its_own_gain() {
        let track = GainRule {
            mode: GainMode::Track,
            ..GainRule::default()
        };
        let album = GainRule {
            mode: GainMode::Album,
            ..GainRule::default()
        };
        // -6 dB is half amplitude, -3 dB is about 0.708.
        assert!((track.factor(both()) - 0.5012).abs() < 1e-3);
        assert!((album.factor(both()) - 0.7079).abs() < 1e-3);
    }

    #[test]
    fn a_mode_falls_back_to_the_gain_the_file_has() {
        // Album-tagged only, played in track mode.
        let rg = ReplayGain {
            album_db: Some(-6.0),
            ..ReplayGain::default()
        };
        let rule = GainRule {
            mode: GainMode::Track,
            ..GainRule::default()
        };
        assert!((rule.factor(rg) - 0.5012).abs() < 1e-3);
    }

    #[test]
    fn the_preamp_rides_on_top_and_the_fallback_stands_alone() {
        let rule = GainRule {
            mode: GainMode::Track,
            preamp_db: 6.0,
            fallback_db: -6.0,
        };
        // Tagged: -6 dB of tag plus 6 dB of preamp is unity.
        let rg = ReplayGain {
            track_db: Some(-6.0),
            ..ReplayGain::default()
        };
        assert!((rule.factor(rg) - 1.0).abs() < 1e-4);
        // Untagged: the fallback alone, no preamp.
        assert!((rule.factor(ReplayGain::default()) - 0.5012).abs() < 1e-3);
    }

    #[test]
    fn the_peak_clamps_a_boost_but_never_makes_one() {
        // +6 dB asked for, peak at 0.8: the boost stops at 1/0.8.
        let rule = GainRule {
            mode: GainMode::Track,
            ..GainRule::default()
        };
        let rg = ReplayGain {
            track_db: Some(6.0),
            track_peak: Some(0.8),
            ..ReplayGain::default()
        };
        assert!((rule.factor(rg) - 1.25).abs() < 1e-4);
        // A cut is left alone whatever the peak.
        let rg = ReplayGain {
            track_db: Some(-6.0),
            track_peak: Some(0.1),
            ..ReplayGain::default()
        };
        assert!((rule.factor(rg) - 0.5012).abs() < 1e-3);
    }

    #[test]
    fn a_garbage_tag_cannot_blow_the_output_up() {
        let rule = GainRule {
            mode: GainMode::Track,
            ..GainRule::default()
        };
        let rg = ReplayGain {
            track_db: Some(9999.0),
            ..ReplayGain::default()
        };
        assert_eq!(rule.factor(rg), db_to_linear(DB_LIMIT));
    }

    #[test]
    fn fade_endpoints_are_whole_tracks() {
        let (g_in, g_out) = crossfade(0.0);
        assert!(g_in.abs() < 1e-6, "the incoming track starts silent");
        assert!(
            (g_out - 1.0).abs() < 1e-6,
            "the outgoing track starts whole"
        );
        let (g_in, g_out) = crossfade(1.0);
        assert!((g_in - 1.0).abs() < 1e-6, "the incoming track ends whole");
        assert!(g_out.abs() < 1e-6, "the outgoing track ends silent");
    }

    #[test]
    fn fade_holds_power_across_the_window() {
        // Summed power stays 1 across the window.
        for step in 0..=20 {
            let t = step as f32 / 20.0;
            let (g_in, g_out) = crossfade(t);
            let power = g_in * g_in + g_out * g_out;
            assert!((power - 1.0).abs() < 1e-5, "power sags at t={t}: {power}");
        }
    }

    #[test]
    fn fade_clamps_past_the_window() {
        // Past the end the incoming plays alone.
        assert_eq!(crossfade(2.0), crossfade(1.0));
    }

    #[test]
    fn mix_walks_the_curve_from_the_offset() {
        let mut incoming = vec![1.0f32, 1.0, 1.0, 1.0];
        let outgoing = vec![1.0f32, 1.0, 1.0, 1.0];
        crossfade_mix(&mut incoming, &outgoing, 2, 4);
        let (g_in, g_out) = crossfade(0.5);
        assert!((incoming[0] - (g_in + g_out)).abs() < 1e-6);
        let (g_in, g_out) = crossfade(0.75);
        assert!((incoming[2] - (g_in + g_out)).abs() < 1e-6);
    }

    #[test]
    fn mix_treats_a_short_outgoing_as_silence() {
        // The outgoing ran out mid-window: the incoming keeps its fade-in gain.
        let mut incoming = vec![1.0f32, 1.0, 1.0, 1.0];
        crossfade_mix(&mut incoming, &[], 0, 4);
        assert_eq!(incoming[0], crossfade(0.0).0);
        assert_eq!(incoming[2], crossfade(0.25).0);
    }
}
