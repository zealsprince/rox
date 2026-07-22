//! One rating, three shapes. The app holds a 0-100 value (0 unrated,
//! a star is 20 points, the numeric scale's 7.5 is 75) and every tag
//! write carries it twice: a whole-star POPM/RATING for the players that
//! only speak stars, and an exact FMPS_Rating decimal so half points
//! survive the round trip. This module owns every conversion between
//! those shapes, so the writer, the scanner, and the store agree on one
//! set of thresholds - lofty's MusicBee mapping, the de-facto default.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::mpeg::MpegFile;
use lofty::ogg::VorbisComments;
use lofty::probe::Probe;

/// The exact-value key, the FreeDesktop media player spec's 0.0-1.0
/// fraction: a TXXX description on ID3v2, a comment key on Vorbis.
pub const FMPS_KEY: &str = "FMPS_Rating";

/// The whole stars a value rounds to, 1-5; a set value never rounds to
/// zero, so half a star still shows and writes as one.
pub fn stars(value: u8) -> u8 {
    ((value + 10) / 20).clamp(1, 5)
}

/// The 0-10 display form, the value the writer's `Field::Rating` speaks:
/// whole numbers bare, halves and finer with their decimal ("8", "7.5").
pub fn display(value: u8) -> String {
    if value.is_multiple_of(10) {
        (value / 10).to_string()
    } else {
        format!("{}.{}", value / 10, value % 10)
    }
}

/// A 0-10 display number back to the value; None for anything that does
/// not read as one. "0" parses to Some(0), the explicit clear.
pub fn parse_display(s: &str) -> Option<u8> {
    let n: f32 = s.trim().parse().ok()?;
    if !n.is_finite() || !(0.0..=10.0).contains(&n) {
        return None;
    }
    Some((n * 10.0).round() as u8)
}

/// The FMPS 0.0-1.0 fraction for a value.
pub fn fmps(value: u8) -> String {
    format!("{:.2}", f32::from(value) / 100.0)
}

/// An FMPS fraction back to the value; out-of-range values are noise,
/// not ratings.
pub fn parse_fmps(s: &str) -> Option<u8> {
    let n: f32 = s.trim().parse().ok()?;
    if !n.is_finite() || !(0.0..=1.0).contains(&n) {
        return None;
    }
    Some((n * 100.0).round() as u8)
}

/// The generic popularimeter text lofty's split produces, and what its
/// merge expects back: "email|stars|counter" off a POPM frame or a
/// RATING:email key, or the bare number a plain Vorbis RATING passes
/// through raw. The bare form has no standard scale; small values read
/// as stars, the rest as 0-100.
pub fn parse_popm_text(s: &str) -> Option<u8> {
    let parts: Vec<&str> = s.split('|').collect();
    if parts.len() == 3 {
        let stars: u8 = parts[1].trim().parse().ok()?;
        return (1..=5).contains(&stars).then_some(stars * 20);
    }
    let n: u8 = s.trim().parse().ok()?;
    Some(match n {
        0 => 0,
        1..=5 => n * 20,
        n => n.min(100),
    })
}

/// The popularimeter text for a value, empty email on purpose: lofty
/// merges an empty email to a bare POPM frame on ID3v2 and a bare
/// RATING key on Vorbis, the forms other players read without knowing
/// us. The counter stays zero; rox counts plays in its own listens.
pub fn popm_text(value: u8) -> String {
    format!("|{}|0", stars(value))
}

/// A raw POPM byte to the value, lofty's MusicBee ID3v2 thresholds; zero
/// is unrated, not one star.
pub fn from_popm_byte(byte: u8) -> u8 {
    let stars = match byte {
        0 => return 0,
        1 => 1,
        2..=64 => 2,
        65..=128 => 3,
        129..=196 => 4,
        _ => 5,
    };
    stars * 20
}

/// A file's rating for the scanner: FMPS first, the exact value, then
/// the star forms. One targeted tag parse (properties off); the formats
/// the writer cannot write read the same way they were written by
/// whoever wrote them. None (never an error) when nothing readable
/// carries one - a scan must not lose a file over its rating.
pub fn read(path: &Path, kind: FileType) -> Option<u8> {
    catch_unwind(AssertUnwindSafe(|| read_inner(path, kind)))
        .ok()
        .flatten()
}

fn read_inner(path: &Path, kind: FileType) -> Option<u8> {
    let opts = crate::parse_opts().read_properties(false);
    match kind {
        FileType::Mpeg => {
            let mut source = crate::tag_source::open(path).ok()?;
            let tag = MpegFile::read_from(&mut source, opts).ok()?.id3v2().cloned()?;
            from_id3v2(&tag)
        }
        FileType::Flac => {
            let mut source = crate::tag_source::open(path).ok()?;
            let tag = FlacFile::read_from(&mut source, opts)
                .ok()?
                .vorbis_comments()
                .cloned()?;
            from_vorbis(&tag)
        }
        _ => None,
    }
}

/// The rating carried by an already-parsed ID3v2 tag: FMPS first, the
/// exact value, then the popularimeter's stars. The scanner parses the
/// MPEG file once for its generic tags and hands that same tag here, so a
/// scan never re-opens the file just for the rating. FMPS lives in a TXXX
/// frame and POPM in its own frame, neither of which the generic tag
/// carries, so this reads the native frames directly.
pub fn from_id3v2(tag: &Id3v2Tag) -> Option<u8> {
    let mut popm = None;
    for frame in tag {
        match frame {
            Frame::UserText(f) if f.description.eq_ignore_ascii_case(FMPS_KEY) => {
                if let Some(value) = parse_fmps(&f.content) {
                    return Some(value);
                }
            }
            Frame::Popularimeter(f) if popm.is_none() => {
                popm = Some(from_popm_byte(f.rating));
            }
            _ => {}
        }
    }
    popm
}

/// The rating carried by an already-parsed Vorbis comment block, the FLAC
/// mirror of [`from_id3v2`]: FMPS first, then a bare RATING or a
/// RATING:email key. The scanner's single FLAC parse feeds this so a scan
/// reads the file once, not twice.
pub fn from_vorbis(tag: &VorbisComments) -> Option<u8> {
    let mut popm = None;
    for (key, value) in tag.items() {
        if key.eq_ignore_ascii_case(FMPS_KEY) {
            if let Some(value) = parse_fmps(value) {
                return Some(value);
            }
        }
        // The bare key and the RATING:email convention both count;
        // provider-specific email scales (Picard's 0-25) are rare
        // enough to read on the common 0-100 assumption.
        if popm.is_none()
            && (key.eq_ignore_ascii_case("RATING")
                || key
                    .get(..7)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("RATING:")))
        {
            popm = parse_popm_text(value);
        }
    }
    popm
}

/// Probe a path's format and read its rating, the reindex-free entry.
pub fn read_path(path: &Path) -> Option<u8> {
    let kind = Probe::open(path).ok()?.guess_file_type().ok()?.file_type()?;
    read(path, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_round_trip() {
        assert_eq!(display(75), "7.5");
        assert_eq!(display(80), "8");
        assert_eq!(parse_display("7.5"), Some(75));
        assert_eq!(parse_display("8.0"), Some(80));
        assert_eq!(parse_display("0"), Some(0));
        assert_eq!(parse_display("11"), None);
        assert_eq!(fmps(75), "0.75");
        assert_eq!(parse_fmps("0.75"), Some(75));
        assert_eq!(parse_fmps("2.0"), None);
    }

    #[test]
    fn popm_text_speaks_both_shapes() {
        assert_eq!(parse_popm_text("|4|0"), Some(80));
        assert_eq!(parse_popm_text("MusicBee|2|15"), Some(40));
        assert_eq!(parse_popm_text("80"), Some(80), "bare 0-100 passes through");
        assert_eq!(parse_popm_text("4"), Some(80), "a small bare value reads as stars");
        assert_eq!(parse_popm_text("|9|0"), None);
        assert_eq!(popm_text(75), "|4|0");
        assert_eq!(from_popm_byte(196), 80);
        assert_eq!(from_popm_byte(0), 0);
    }
}
