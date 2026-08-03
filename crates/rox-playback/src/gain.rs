//! The source-gain stage (ADR 19): gain applied to each decoded source on
//! its own, after the fold and resample and before the sources sum. It has
//! to sit here rather than in the chain because a crossfade window has two
//! tracks live at once, and one node multiplying the mix would apply one
//! track's factor to both.
//!
//! Two things ride this stage. ReplayGain turns a track's tagged loudness
//! into one constant factor, and crossfade turns the window into a
//! per-frame pair; a source in a fade carries both, folded into one
//! multiply before the sum.
//!
//! Unity short-circuits, so a source with nothing to apply reaches the mix
//! bit-identical. That is the bypass rule the chain holds, kept here too.

use std::f32::consts::FRAC_PI_2;

/// What a file's ReplayGain tags say: how far off the reference loudness
/// the track and its album measured, in dB, each beside the peak sample
/// the same pass found. None per field, since a file carries any mix of
/// the four, and plenty carry none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReplayGain {
    pub track_db: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_db: Option<f32>,
    pub album_peak: Option<f32>,
}

impl ReplayGain {
    /// The track pair, gain beside the peak measured with it.
    fn track(self) -> Option<(f32, Option<f32>)> {
        self.track_db.map(|db| (db, self.track_peak))
    }

    /// The album pair, the same way.
    fn album(self) -> Option<(f32, Option<f32>)> {
        self.album_db.map(|db| (db, self.album_peak))
    }

    /// Whether the file carries anything to level by.
    pub fn any(self) -> bool {
        self.track_db.is_some() || self.album_db.is_some()
    }
}

/// Which of a file's two gains to level by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GainMode {
    /// No leveling at all: every source plays at unity, which is the
    /// bypass rule.
    #[default]
    Off,
    /// Each track to the same loudness, so a shuffle of unrelated music
    /// stops jumping between masters.
    Track,
    /// The album's gain across all its tracks, so a record's own quiet and
    /// loud passages stay where the engineer put them.
    Album,
}

/// How tagged loudness becomes a factor: which gain to read, and the two
/// offsets on top of it. Held by the engine and swappable while a stream
/// runs, since it changes a multiply and nothing structural.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GainRule {
    pub mode: GainMode,
    /// Added to every tagged gain. ReplayGain's reference sits well below
    /// the level modern masters are cut at, so a whole library levelled to
    /// it plays quieter than the same library raw; this is where that gets
    /// taken back.
    pub preamp_db: f32,
    /// What a file with no tags plays at. Its own knob rather than the
    /// preamp, because an untagged track has nothing to be offset from:
    /// the number is the whole decision.
    pub fallback_db: f32,
}

/// The dB either knob is allowed to reach. Wide enough for any real tag
/// plus a preamp, narrow enough that a garbage value in a file can't turn
/// into a factor of a thousand.
const DB_LIMIT: f32 = 40.0;

impl GainRule {
    /// The linear factor for a source carrying `rg`. Exactly 1.0 whenever
    /// nothing asks otherwise, so [`apply`] short-circuits and the samples
    /// stay the decoder's.
    pub fn factor(&self, rg: ReplayGain) -> f32 {
        let (db, peak) = match self.mode {
            GainMode::Off => return 1.0,
            // Either mode falls back to the gain the file does carry: a
            // track tagged one way and played the other is better levelled
            // by the wrong pass than not at all, and the peak that comes
            // with it is the one measured alongside.
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
        // The peak clamps the result (ADR 19): a quiet track boosted past
        // where its loudest sample fits would clip, and the tag already
        // says where that sample is. Only ever downward, so a peak can't
        // turn into a boost.
        match peak {
            Some(peak) if peak > 0.0 => factor.min(1.0 / peak),
            _ => factor,
        }
    }
}

/// dB to a linear multiplier, over the range a gain knob may ask for.
fn db_to_linear(db: f32) -> f32 {
    if db == 0.0 {
        // Exactly unity, so the bypass rule holds without leaning on
        // powf returning it.
        return 1.0;
    }
    10f32.powf(db.clamp(-DB_LIMIT, DB_LIMIT) / 20.0)
}

/// Multiply a chunk by a constant gain, in place. Exactly unity is a
/// no-op, not a multiply by 1.0: the samples come out of this stage the
/// bits the decoder produced.
pub fn apply(buf: &mut [f32], gain: f32) {
    if gain == 1.0 {
        return;
    }
    for sample in buf {
        *sample *= gain;
    }
}

/// The fade pair at `t`, where 0 is the start of the fade window and 1 its
/// end: (incoming, outgoing).
///
/// Equal power, sine over cosine. The two tracks in a fade are unrelated
/// material, so their sum behaves like uncorrelated signals: powers add
/// where amplitudes wouldn't, and a linear pair (both at 0.5 halfway)
/// audibly dips in the middle. At 0.707 each the perceived level holds
/// across the window. ADR 19 left the curve to the implementation; this is
/// the pick, and the album-contiguous boundary never reaches it.
pub fn crossfade(t: f32) -> (f32, f32) {
    let (sin, cos) = (t.clamp(0.0, 1.0) * FRAC_PI_2).sin_cos();
    (sin, cos)
}

/// Mix `outgoing` under `incoming` in place, walking the fade curve from
/// frame `done` of a `len`-frame window. Both buffers are interleaved
/// stereo at the same rate; `outgoing` running short is silence, which is
/// what a track that ended before its fade window closed leaves behind.
pub fn crossfade_mix(incoming: &mut [f32], outgoing: &[f32], done: u64, len: u64) {
    let len = len.max(1) as f32;
    for (i, frame) in incoming.chunks_exact_mut(2).enumerate() {
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

    /// A track tagged both ways, the shape most of a tagged library takes.
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
        // The bypass rule reaches back this far: leveling off has to leave
        // the samples the decoder's, not multiply them by a rounded 1.0.
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
        // Album-tagged only, played in track mode: levelled by the album
        // pass rather than not at all.
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
        // Untagged: the fallback is the whole decision, the preamp stays out
        // of it.
        assert!((rule.factor(ReplayGain::default()) - 0.5012).abs() < 1e-3);
    }

    #[test]
    fn the_peak_clamps_a_boost_but_never_makes_one() {
        // +6 dB asked for, but the loudest sample sits at 0.8: the boost
        // stops at 1/0.8 so the track can't clip.
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
        // A cut is left alone: a peak well under full scale is not a reason
        // to turn a quiet track up.
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
        // The point of equal power: summed power stays 1 the whole way, so
        // the middle of a fade doesn't sag the way a linear pair does.
        for step in 0..=20 {
            let t = step as f32 / 20.0;
            let (g_in, g_out) = crossfade(t);
            let power = g_in * g_in + g_out * g_out;
            assert!((power - 1.0).abs() < 1e-5, "power sags at t={t}: {power}");
        }
    }

    #[test]
    fn fade_clamps_past_the_window() {
        // Past the end the incoming plays alone, which is what lets a chunk
        // straddle the close of the window without special-casing it.
        assert_eq!(crossfade(2.0), crossfade(1.0));
    }

    #[test]
    fn mix_walks_the_curve_from_the_offset() {
        // Two frames starting halfway through a four-frame window.
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
        // The outgoing track ran out mid-window: the incoming carries on at
        // its fade-in gain instead of reading past the buffer.
        let mut incoming = vec![1.0f32, 1.0, 1.0, 1.0];
        crossfade_mix(&mut incoming, &[], 0, 4);
        assert_eq!(incoming[0], crossfade(0.0).0);
        assert_eq!(incoming[2], crossfade(0.25).0);
    }
}
