//! A track's tempo in beats a minute: what its tags claim, and what rox
//! measures where they claim nothing. The scanner reads the tag on the same
//! pass that fills the rest of a row; the analysis pass writes past the
//! scanner straight onto the row through [`crate::store::set_measured_bpm`],
//! marked [`Source::Measured`] so a later rescan can tell its own numbers
//! from a tagger's.
//!
//! One value under two names. ID3v2 writes TBPM and Vorbis a BPM comment,
//! which lofty maps to `IntegerBpm` and `Bpm`; taggers that write both agree
//! on the number, so the integer key is asked first and the other only fills
//! a gap. MP4's `tmpo` is an integer atom lofty's generic tag drops on the
//! way out, so an m4a reads as untagged unless it carries iTunes' freeform
//! BPM beside it.

use lofty::tag::{ItemKey, Tag};

/// The slowest tempo a stored row may claim.
///
/// Below this a number is not a tempo anybody meant. Taggers write 0 for
/// "unset", and the odd file carries a beat period or a sample count in the
/// frame instead of a rate; either would sort ahead of the whole library
/// and pick wrong for anything built on tempo.
pub const SLOWEST: f32 = 40.0;

/// The fastest tempo a stored row may claim, the other end of [`SLOWEST`].
/// Wide enough for drum and bass counted straight, which is about as fast
/// as music gets counted before somebody halves it.
pub const FASTEST: f32 = 300.0;

/// Where a stored row's tempo came from. The store keeps this in its
/// `bpm_source` column so a rescan knows which number is the file's to
/// clear and which is rox's own, the same job [`crate::replaygain::Source`]
/// does for the gains.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// Read off the file's tags. The default, and what every row written
    /// before the tempo pass existed reads as.
    #[default]
    Tags,
    /// Estimated by rox from the audio, for a file whose tags carried no
    /// tempo.
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
    /// guessing `Measured` would let a rescan keep a number nobody measured.
    pub fn from_code(code: Option<i64>) -> Self {
        match code {
            Some(1) => Source::Measured,
            _ => Source::Tags,
        }
    }
}

/// Read the tempo off a parsed tag, None where the file carries none rox
/// will believe. One tag, not every tag the file holds: a second tag
/// disagreeing about the tempo is a conflict to resolve, not a gap to fill,
/// and the wide read [`crate::scanner`] does for ReplayGain exists because
/// mp3gain writes where nothing else looks.
pub fn read(tag: &Tag) -> Option<f32> {
    let bpm = |key| tag.get_string(key).and_then(parse);
    bpm(ItemKey::IntegerBpm).or_else(|| bpm(ItemKey::Bpm))
}

/// A tempo field: a plain number of beats a minute, written whole by most
/// taggers and with a fraction by the ones that estimated it. Anything
/// outside [`SLOWEST`]..=[`FASTEST`] reads as untagged, junk included, so a
/// file claiming 0 or 9999 lands on the measurement pass's list rather than
/// carrying a number the library would sort and mix by.
pub fn parse(value: &str) -> Option<f32> {
    let bpm: f32 = value.trim().parse().ok()?;
    (bpm.is_finite() && (SLOWEST..=FASTEST).contains(&bpm)).then_some(bpm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempos_parse_the_forms_taggers_write() {
        assert_eq!(parse("128"), Some(128.0));
        assert_eq!(parse("128.5"), Some(128.5));
        assert_eq!(parse("  92  "), Some(92.0));
        // The ends of the range are inside it.
        assert_eq!(parse("40"), Some(40.0));
        assert_eq!(parse("300"), Some(300.0));
    }

    #[test]
    fn junk_and_impossible_tempos_read_as_untagged() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("fast"), None);
        assert_eq!(parse("inf"), None);
        // The unset value plenty of taggers write.
        assert_eq!(parse("0"), None);
        // A beat period in milliseconds, or a sample count, in the frame
        // that was supposed to hold a rate.
        assert_eq!(parse("468.75"), None);
        assert_eq!(parse("39.9"), None);
        // A locale decimal comma, which parses as nothing rather than as 128.
        assert_eq!(parse("128,5"), None);
    }

    /// Each format's own key reads: ID3v2 keeps the tempo in TBPM, which
    /// lofty calls the integer key, and Vorbis in a BPM comment, which it
    /// calls the other one.
    #[test]
    fn both_keys_read_where_their_format_carries_them() {
        use lofty::tag::TagType;
        let mut id3 = Tag::new(TagType::Id3v2);
        id3.insert_text(ItemKey::IntegerBpm, "128".into());
        assert_eq!(read(&id3), Some(128.0));

        let mut vorbis = Tag::new(TagType::VorbisComments);
        vorbis.insert_text(ItemKey::Bpm, "174.3".into());
        assert_eq!(read(&vorbis), Some(174.3));
    }

    /// MP4 is the format that carries both, `tmpo` beside iTunes' freeform
    /// BPM. The integer key wins where they disagree, and a gap in it falls
    /// through to the other rather than reading as untagged outright.
    #[test]
    fn the_integer_key_wins_and_the_other_fills_a_gap() {
        use lofty::tag::TagType;
        let mut tag = Tag::new(TagType::Mp4Ilst);
        tag.insert_text(ItemKey::IntegerBpm, "128".into());
        tag.insert_text(ItemKey::Bpm, "64".into());
        assert_eq!(read(&tag), Some(128.0));

        let mut unset = Tag::new(TagType::Mp4Ilst);
        unset.insert_text(ItemKey::IntegerBpm, "0".into());
        unset.insert_text(ItemKey::Bpm, "97.5".into());
        assert_eq!(read(&unset), Some(97.5));
    }

    #[test]
    fn a_tag_without_a_tempo_reads_none() {
        use lofty::tag::TagType;
        assert_eq!(read(&Tag::new(TagType::Id3v2)), None);
    }

    #[test]
    fn the_source_codes_round_trip() {
        assert_eq!(Source::from_code(Some(Source::Tags.code())), Source::Tags);
        assert_eq!(
            Source::from_code(Some(Source::Measured.code())),
            Source::Measured
        );
        assert_eq!(Source::from_code(None), Source::Tags);
    }
}
