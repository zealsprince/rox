//! ReplayGain as a file carries it: how far off the reference loudness an
//! analysis pass measured the track and its album, in dB, each beside the
//! peak sample it found. rox reads these, stores them beside the rest of a
//! row, and hands them to the engine at play time (ADR 19).
//!
//! Reading tags is all this module does. A file that carries none gets its
//! numbers from rox's own measurement pass instead: the EBU R128 analyzer is
//! `rox_playback::analysis`, the app drives it from `rox/src/replaygain_job.rs`
//! over the files [`crate::store::albums_missing_replaygain`] hands back, and
//! the result lands through [`crate::store::set_measured_replaygain`] marked
//! [`Source::Measured`] so a later rescan can tell it apart from what a tagger
//! wrote. The same pass writes the numbers into the files themselves through
//! [`crate::writer::commit_replay_gain`] when the setting asks for it.
//!
//! The tags live under the same four names everywhere lofty looks -
//! `REPLAYGAIN_TRACK_GAIN` and friends, as TXXX frames in ID3v2, Vorbis
//! comments in FLAC, freeform atoms in MP4 - so one generic read covers
//! every format the scanner indexes.
//!
//! Two other levelling schemes exist and neither is handled here. Opus files
//! carry `R128_TRACK_GAIN`/`R128_ALBUM_GAIN` per RFC 7845: a Q7.8 fixed-point
//! number of dB relative to -23 LUFS, so converting one to a ReplayGain figure
//! means dividing by 256 and adding 5 dB for the reference difference against
//! RG's -18. That conversion stays unwritten while `.opus` is off
//! `scanner::EXTENSIONS`; when Opus lands, this module grows it, or the claim
//! above about covering every indexed format stops being true. iTunes' own
//! `iTunNORM` atom is out of scope entirely, since nothing else writes it and
//! its per-channel millwatt figures are not a dB gain.

use lofty::tag::{ItemKey, Tag};

/// One file's four ReplayGain numbers. None per field: a file carries any
/// mix of the four, and plenty carry none at all.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReplayGain {
    /// The track's gain in dB, negative for a loud master.
    pub track_db: Option<f32>,
    /// The loudest sample in the track, 1.0 being full scale. What clamps
    /// a boost at playback so a quiet track turned up cannot clip.
    pub track_peak: Option<f32>,
    pub album_db: Option<f32>,
    pub album_peak: Option<f32>,
}

impl ReplayGain {
    /// Whether the file carries anything to level by. The peaks alone do
    /// not count: they bound a gain, they are not one.
    pub fn any(self) -> bool {
        self.track_db.is_some() || self.album_db.is_some()
    }
}

/// Where a stored row's ReplayGain came from. The store keeps this in its
/// `rg_source` column so a rescan knows which numbers are the file's to
/// clear and which are rox's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// Read off the file's tags. The default, and what every row written
    /// before the measurement pass existed reads as.
    #[default]
    Tags,
    /// Measured by rox from the audio, for a file whose tags carried no
    /// gain (ADR 19).
    Measured,
}

impl Source {
    /// The integer the store writes. A NULL column reads back as `Tags`,
    /// so rows from before the column need no backfill.
    pub fn code(self) -> i64 {
        match self {
            Source::Tags => 0,
            Source::Measured => 1,
        }
    }

    /// The column back. Anything unexpected, NULL included, reads as `Tags`:
    /// an older binary's row is tag-sourced until proven otherwise, and
    /// guessing `Measured` would let a rescan keep numbers nobody measured.
    pub fn from_code(code: Option<i64>) -> Self {
        match code {
            Some(1) => Source::Measured,
            _ => Source::Tags,
        }
    }
}

/// Read the four values off a parsed tag. Anything missing or unparseable
/// comes back None, which plays as untagged rather than as zero: a wrong
/// number here is a track at the wrong volume for its whole length.
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

/// A gain field: a signed decibel figure, conventionally written with its
/// unit (`-7.35 dB`) but not always, and sometimes with a leading `+`.
/// Everything after the number is dropped, so a stray unit or a trailing
/// comment costs nothing.
///
/// A comma cutting the number short is the exception and reads as untagged.
/// Some taggers write the decimal separator by locale, and `-3,5 dB` truncated
/// to -3 is a track playing half a dB off for its whole length with nothing to
/// show anything went wrong. No gain at all is the safer wrong answer.
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

/// A peak field: a linear sample value, 1.0 full scale. Values above 1 are
/// real (a clipped master measures over), zero and below are not: a peak of
/// zero would clamp the track to silence, so it reads as no peak at all.
pub fn parse_peak(value: &str) -> Option<f32> {
    let peak: f32 = value.trim().parse().ok()?;
    (peak.is_finite() && peak > 0.0).then_some(peak)
}

/// A gain back in the form a tag holds it, which is also how the tag editor
/// shows it.
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
        // A gain of exactly zero is a measurement, not an absence.
        assert_eq!(parse_gain("0 dB"), Some(0.0));
    }

    #[test]
    fn junk_gains_read_as_untagged() {
        assert_eq!(parse_gain(""), None);
        assert_eq!(parse_gain("dB"), None);
        assert_eq!(parse_gain("loud"), None);
        assert_eq!(parse_gain("inf dB"), None);
        // A locale decimal comma, which used to truncate to -3 and level the
        // track half a dB off without a word.
        assert_eq!(parse_gain("-3,5 dB"), None);
        assert_eq!(parse_gain("0,00"), None);
    }

    #[test]
    fn peaks_keep_overs_and_drop_the_impossible() {
        assert_eq!(parse_peak("0.987654"), Some(0.987654));
        // A clipped master measures over full scale, which is worth
        // knowing rather than rounding away.
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
        assert!(ReplayGain {
            album_db: Some(-6.0),
            ..ReplayGain::default()
        }
        .any());
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
