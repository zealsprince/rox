//! ReplayGain as a file stores it: track and album gain in dB, each with its
//! peak. rox reads these and hands them to the engine at play time (ADR 19).
//! Files without them get rox's own measurement, written through
//! [`crate::store::set_measured_replaygain`] marked [`Source::Measured`].
//!
//! lofty maps the four `REPLAYGAIN_*` names on every indexed format but
//! Opus. Opus levels by RFC 7845's `R128_*_GAIN` instead (Q7.8 dB against
//! -23 LUFS, no peaks), unmapped Vorbis keys the scanner reads off its native
//! parse through [`read_r128`]. The writer doesn't write Opus, so measured
//! Opus gains stay in the database.
//!
//! iTunes' `iTunNORM` is out of scope: per-channel milliwatts, not a dB gain.

use lofty::ogg::VorbisComments;
use lofty::tag::{ItemKey, Tag};

/// One file's four ReplayGain numbers. Any mix may be missing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReplayGain {
    pub track_db: Option<f32>,
    /// 1.0 is full scale. Clamps a boost so a quiet track can't clip.
    pub track_peak: Option<f32>,
    pub album_db: Option<f32>,
    pub album_peak: Option<f32>,
}

impl ReplayGain {
    /// Whether the file has a gain to level by. Peaks alone don't count.
    pub fn any(self) -> bool {
        self.track_db.is_some() || self.album_db.is_some()
    }
}

/// Where a stored row's ReplayGain came from, so a rescan knows which
/// numbers are the file's to clear and which are rox's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// Read off the file's tags. Also what pre-column rows read as.
    #[default]
    Tags,
    Measured,
}

impl Source {
    /// A NULL column reads back as `Tags`, so old rows need no backfill.
    pub fn code(self) -> i64 {
        match self {
            Source::Tags => 0,
            Source::Measured => 1,
        }
    }

    /// Anything unexpected reads as `Tags`: guessing `Measured` would let a
    /// rescan keep numbers nobody measured.
    pub fn from_code(code: Option<i64>) -> Self {
        match code {
            Some(1) => Source::Measured,
            _ => Source::Tags,
        }
    }
}

/// Anything missing or unparseable is None, which plays as untagged rather
/// than as zero.
pub fn read(tag: &Tag) -> ReplayGain {
    let gain = |key| tag.get_string(key).and_then(parse_gain);
    let peak = |key| tag.get_string(key).and_then(parse_peak);
    ReplayGain {
        track_db: gain(ItemKey::ReplayGainTrackGain),
        track_peak: peak(ItemKey::ReplayGainTrackPeak),
        album_db: gain(ItemKey::ReplayGainAlbumGain),
        album_peak: peak(ItemKey::ReplayGainAlbumPeak),
    }
}

/// None when neither key is there, so "no levelling" differs from 0 dB.
pub fn read_r128(comments: &VorbisComments) -> Option<ReplayGain> {
    let rg = ReplayGain {
        track_db: comments.get("R128_TRACK_GAIN").and_then(parse_r128),
        track_peak: None,
        album_db: comments.get("R128_ALBUM_GAIN").and_then(parse_r128),
        album_peak: None,
    };
    rg.any().then_some(rg)
}

/// Q7.8 dB against -23 LUFS: divide by 256, add 5 for ReplayGain's -18 LUFS
/// reference. Strict, unlike [`parse_gain`]: machines write this field.
pub fn parse_r128(value: &str) -> Option<f32> {
    let q78: i32 = value.trim().parse().ok()?;
    let db = q78 as f32 / 256.0 + 5.0;
    db.is_finite().then_some(db)
}

/// A signed dB figure, unit optional. A decimal comma reads as untagged:
/// `-3,5 dB` truncated to -3 would level half a dB off, silently.
pub fn parse_gain(value: &str) -> Option<f32> {
    let value = value.trim();
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    if value[end..].starts_with(',') {
        return None;
    }
    let db: f32 = value[..end].trim().parse().ok()?;
    db.is_finite().then_some(db)
}

/// Values above 1 are real (a clipped master); zero or below reads as no
/// peak, since it would clamp the track to silence.
pub fn parse_peak(value: &str) -> Option<f32> {
    let peak: f32 = value.trim().parse().ok()?;
    (peak.is_finite() && peak > 0.0).then_some(peak)
}

pub fn format_gain(db: f32) -> String {
    format!("{db:+.2} dB")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gains_parse_the_forms_taggers_write() {
        assert_eq!(parse_gain("-7.35 dB"), Some(-7.35));
        assert_eq!(parse_gain("-7.35dB"), Some(-7.35));
        assert_eq!(parse_gain("+2.10 dB"), Some(2.10));
        assert_eq!(parse_gain("  0.00  "), Some(0.0));
        assert_eq!(parse_gain("0 dB"), Some(0.0));
    }

    #[test]
    fn junk_gains_read_as_untagged() {
        assert_eq!(parse_gain(""), None);
        assert_eq!(parse_gain("dB"), None);
        assert_eq!(parse_gain("loud"), None);
        assert_eq!(parse_gain("inf dB"), None);
        assert_eq!(parse_gain("-3,5 dB"), None);
        assert_eq!(parse_gain("0,00"), None);
    }

    #[test]
    fn peaks_keep_overs_and_drop_the_impossible() {
        assert_eq!(parse_peak("0.987654"), Some(0.987654));
        assert_eq!(parse_peak("1.023"), Some(1.023));
        assert_eq!(parse_peak("0"), None);
        assert_eq!(parse_peak("-0.5"), None);
        assert_eq!(parse_peak("none"), None);
    }

    #[test]
    fn any_is_about_gains_not_peaks() {
        let peaks_only = ReplayGain {
            track_peak: Some(0.9),
            album_peak: Some(0.95),
            ..ReplayGain::default()
        };
        assert!(!peaks_only.any(), "a peak bounds a gain, it isn't one");
        assert!(
            ReplayGain {
                album_db: Some(-6.0),
                ..ReplayGain::default()
            }
            .any()
        );
    }

    #[test]
    fn reads_all_four_off_a_tag() {
        use lofty::tag::TagType;
        let mut tag = Tag::new(TagType::VorbisComments);
        tag.insert_text(ItemKey::ReplayGainTrackGain, "-7.35 dB".into());
        tag.insert_text(ItemKey::ReplayGainTrackPeak, "0.98".into());
        tag.insert_text(ItemKey::ReplayGainAlbumGain, "-8.10 dB".into());
        tag.insert_text(ItemKey::ReplayGainAlbumPeak, "1.01".into());
        let rg = read(&tag);
        assert_eq!(rg.track_db, Some(-7.35));
        assert_eq!(rg.track_peak, Some(0.98));
        assert_eq!(rg.album_db, Some(-8.10));
        assert_eq!(rg.album_peak, Some(1.01));
    }

    #[test]
    fn a_tag_without_the_frames_reads_untagged() {
        use lofty::tag::TagType;
        let tag = Tag::new(TagType::Id3v2);
        assert_eq!(read(&tag), ReplayGain::default());
    }
}
