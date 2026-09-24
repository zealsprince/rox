//! One rating, three shapes: the app's 0-100 value, a whole-star POPM/RATING
//! for star-only players, and an exact FMPS_Rating so half points survive.
//! Every conversion lives here, on lofty's MusicBee thresholds.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::mp4::{AtomData, AtomIdent, Ilst, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::ogg::{OpusFile, VorbisComments};
use lofty::probe::Probe;

/// FreeDesktop's 0.0-1.0 fraction: a TXXX description on ID3v2, a Vorbis key.
pub const FMPS_KEY: &str = "FMPS_Rating";

/// 1-5 stars. A set value never rounds to zero, so half a star writes as one.
pub fn stars(value: u8) -> u8 {
    ((value + 10) / 20).clamp(1, 5)
}

/// The 0-10 display form `Field::Rating` speaks ("8", "7.5").
pub fn display(value: u8) -> String {
    if value.is_multiple_of(10) {
        (value / 10).to_string()
    } else {
        format!("{}.{}", value / 10, value % 10)
    }
}

/// "0" parses to Some(0), the explicit clear.
pub fn parse_display(s: &str) -> Option<u8> {
    let n: f32 = s.trim().parse().ok()?;
    if !n.is_finite() || !(0.0..=10.0).contains(&n) {
        return None;
    }
    Some((n * 10.0).round() as u8)
}

pub fn fmps(value: u8) -> String {
    format!("{:.2}", f32::from(value) / 100.0)
}

pub fn parse_fmps(s: &str) -> Option<u8> {
    let n: f32 = s.trim().parse().ok()?;
    if !n.is_finite() || !(0.0..=1.0).contains(&n) {
        return None;
    }
    Some((n * 100.0).round() as u8)
}

/// Parse lofty's "email|stars|counter" popularimeter text, or a bare Vorbis
/// RATING number (small values read as stars, the rest as 0-100).
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

/// An empty email merges to a bare POPM frame or RATING key, the forms other
/// players read.
pub fn popm_text(value: u8) -> String {
    format!("|{}|0", stars(value))
}

/// lofty's MusicBee ID3v2 thresholds; zero is unrated, not one star.
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

/// FMPS first, then the star forms. Never an error: a scan must not lose a
/// file over its rating.
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
            let tag = MpegFile::read_from(&mut source, opts)
                .ok()?
                .id3v2()
                .cloned()?;
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
        FileType::Mp4 => {
            let mut source = std::fs::File::open(path).ok()?;
            let tag = Mp4File::read_from(&mut source, opts)
                .ok()?
                .ilst()
                .cloned()?;
            from_ilst(&tag)
        }
        // Must agree with the scanner, which reads Opus ratings off its native parse.
        FileType::Opus => {
            let mut source = std::fs::File::open(path).ok()?;
            let opus = OpusFile::read_from(&mut source, opts).ok()?;
            from_vorbis(opus.vorbis_comments())
        }
        _ => None,
    }
}

/// FMPS first, then POPM, off the native frames the generic tag hides.
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

/// FMPS first, then a bare RATING or RATING:email key.
pub fn from_vorbis(tag: &VorbisComments) -> Option<u8> {
    let mut popm = None;
    for (key, value) in tag.items() {
        if key.eq_ignore_ascii_case(FMPS_KEY)
            && let Some(value) = parse_fmps(value)
        {
            return Some(value);
        }
        // Provider email scales (Picard's 0-25) are read as 0-100.
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

/// FMPS freeform atom only. `rate` holds no stars and `rtng` is Apple's
/// content advisory, so reading either would invent a rating.
pub fn from_ilst(tag: &Ilst) -> Option<u8> {
    tag.into_iter()
        .filter(|atom| match atom.ident() {
            AtomIdent::Freeform { name, .. } => name.eq_ignore_ascii_case(FMPS_KEY),
            AtomIdent::Fourcc(_) => false,
        })
        .flat_map(|atom| atom.data())
        .find_map(|data| match data {
            AtomData::UTF8(text) | AtomData::UTF16(text) => parse_fmps(text),
            _ => None,
        })
}

pub fn read_path(path: &Path) -> Option<u8> {
    let kind = Probe::open(path)
        .ok()?
        .guess_file_type()
        .ok()?
        .file_type()?;
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
    fn an_m4a_rating_comes_off_any_freeform_fmps_atom() {
        use lofty::config::WriteOptions;
        use lofty::mp4::Atom;
        use lofty::prelude::TagExt;
        use std::borrow::Cow;

        let dir = crate::writer::scratch("rating-m4a");
        let path = crate::writer::m4a_file(&dir, "track.m4a");
        assert_eq!(read_path(&path), None);

        let mut tag = Ilst::default();
        tag.insert(Atom::new(
            AtomIdent::Freeform {
                mean: Cow::Borrowed("org.example.tagger"),
                name: Cow::Borrowed(FMPS_KEY),
            },
            AtomData::UTF8("0.75".into()),
        ));
        tag.insert(Atom::new(
            AtomIdent::Fourcc(*b"rtng"),
            AtomData::SignedInteger(4),
        ));
        tag.save_to_path(&path, WriteOptions::default()).unwrap();

        assert_eq!(read_path(&path), Some(75));
    }

    #[test]
    fn popm_text_speaks_both_shapes() {
        assert_eq!(parse_popm_text("|4|0"), Some(80));
        assert_eq!(parse_popm_text("MusicBee|2|15"), Some(40));
        assert_eq!(parse_popm_text("80"), Some(80), "bare 0-100 passes through");
        assert_eq!(
            parse_popm_text("4"),
            Some(80),
            "a small bare value reads as stars"
        );
        assert_eq!(parse_popm_text("|9|0"), None);
        assert_eq!(popm_text(75), "|4|0");
        assert_eq!(from_popm_byte(196), 80);
        assert_eq!(from_popm_byte(0), 0);
    }
}
