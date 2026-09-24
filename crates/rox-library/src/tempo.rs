//! A track's tempo: what its tags claim, and what rox measures where they
//! claim nothing (written through [`crate::store::set_measured_bpm`]).
//!
//! lofty maps ID3v2's TBPM to `IntegerBpm` and Vorbis's BPM to `Bpm`; the
//! integer key wins. MP4's `tmpo` is dropped by lofty's generic tag, so an
//! m4a reads as untagged unless it also has iTunes' freeform BPM.

use lofty::tag::{ItemKey, Tag};

/// The slowest tempo a stored row may claim. Below it is a tagger's 0 or a
/// beat period in the wrong frame.
pub const SLOWEST: f32 = 40.0;

/// The fastest tempo a stored row may claim: drum and bass counted straight.
pub const FASTEST: f32 = 300.0;

/// Where a stored row's tempo came from, so a rescan knows which number is
/// the file's to clear and which is rox's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// Read off the file's tags. Also what pre-column rows read as.
    #[default]
    Tags,
    Measured,
    /// Measured and no beat found: NULL on purpose, which keeps beatless tracks
    /// off every later pass's work list.
    Refused,
}

impl Source {
    /// A NULL column reads back as `Tags`, so old rows need no backfill.
    pub fn code(self) -> i64 {
        match self {
            Source::Tags => 0,
            Source::Measured => 1,
            Source::Refused => 2,
        }
    }

    /// Anything unexpected reads as `Tags`: guessing `Measured` would let a
    /// rescan keep a number nobody measured.
    pub fn from_code(code: Option<i64>) -> Self {
        match code {
            Some(1) => Source::Measured,
            Some(2) => Source::Refused,
            _ => Source::Tags,
        }
    }
}

/// The tempo off one parsed tag, None where rox won't believe it.
pub fn read(tag: &Tag) -> Option<f32> {
    let bpm = |key| tag.get_string(key).and_then(parse);
    bpm(ItemKey::IntegerBpm).or_else(|| bpm(ItemKey::Bpm))
}

/// Outside [`SLOWEST`]..=[`FASTEST`] reads as untagged, so junk goes to the
/// measurement pass instead of sorting.
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
        assert_eq!(parse("40"), Some(40.0));
        assert_eq!(parse("300"), Some(300.0));
    }

    #[test]
    fn junk_and_impossible_tempos_read_as_untagged() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("fast"), None);
        assert_eq!(parse("inf"), None);
        assert_eq!(parse("0"), None);
        assert_eq!(parse("468.75"), None);
        assert_eq!(parse("39.9"), None);
        assert_eq!(parse("128,5"), None);
    }

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
            Source::from_code(Some(Source::Refused.code())),
            Source::Refused
        );
        assert_eq!(
            Source::from_code(Some(Source::Measured.code())),
            Source::Measured
        );
        assert_eq!(Source::from_code(None), Source::Tags);
    }
}
