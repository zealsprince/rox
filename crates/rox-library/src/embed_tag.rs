//! An acoustic vector as a tag a file can hold, so a wiped library or a
//! moved folder gets its vectors back without decoding. [`crate::embeddings`]
//! stays the query engine; this is an optional second copy.
//!
//! Key: `ROX_ACOUSTIC:<model-id>`, valid untranslated as both a TXXX
//! description and a Vorbis key. Value: `v1;dim=<n>;f16;<base64>`, so a
//! reader can refuse what it doesn't understand.
//!
//! Half floats, not int8: the raw dimensions span wildly different scales,
//! so int8 would need a per-dimension scale, and the query z-scores away
//! more precision than f16 loses.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use half::f16;
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::Frame;
use lofty::mpeg::MpegFile;
use lofty::probe::Probe;

/// The editor and metadata panel skip anything with this prefix.
pub const PREFIX: &str = "ROX_ACOUSTIC:";

const VERSION: &str = "v1";

pub fn key(model: &str) -> String {
    format!("{PREFIX}{model}")
}

/// Case-insensitive, since Vorbis keys are.
pub fn is_key(key: &str) -> bool {
    key.len() > PREFIX.len() && key[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
}

/// Extension only: a cheap pre-check so unsupported formats skip quietly.
/// MP3 and FLAC only; the MP4 atom's round trip is unproven.
pub fn writable(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3") || e.eq_ignore_ascii_case("flac"))
}

pub fn encode(vec: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(vec.len() * 2);
    for v in vec {
        bytes.extend_from_slice(&f16::from_f32(*v).to_le_bytes());
    }
    format!("{VERSION};dim={};f16;{}", vec.len(), BASE64.encode(&bytes))
}

/// None for anything this module didn't write. A width other than `dim` is
/// refused: returning it would mark the track covered and skip its decode.
pub fn decode(value: &str, dim: usize) -> Option<Vec<f32>> {
    let mut parts = value.split(';');
    if parts.next()? != VERSION {
        return None;
    }
    let claimed: usize = parts.next()?.strip_prefix("dim=")?.parse().ok()?;
    if parts.next()? != "f16" {
        return None;
    }
    let payload = parts.next()?;
    // A fifth field is a spelling this version doesn't know.
    if parts.next().is_some() {
        return None;
    }
    if claimed != dim {
        return None;
    }
    let bytes = BASE64.decode(payload).ok()?;
    if bytes.len() != dim * 2 {
        return None;
    }
    let vec: Vec<f32> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| f16::from_le_bytes(*c).to_f32())
        .collect();
    // One NaN poisons every score in the library, and tags are hand-editable.
    vec.iter().all(|v| v.is_finite()).then_some(vec)
}

/// Reads only; writes go through [`crate::writer::commit_embedding`].
pub fn read(path: &Path, model: &str, dim: usize) -> Option<Vec<f32>> {
    let value = read_value(path, &key(model))?;
    decode(&value, dim)
}

/// Through the writer's sanitising source and relaxed parse.
fn read_value(path: &Path, key: &str) -> Option<String> {
    let kind = Probe::open(path)
        .ok()?
        .guess_file_type()
        .ok()?
        .file_type()?;
    let opts = crate::parse_opts().read_properties(false);
    match kind {
        FileType::Mpeg => {
            let mut source = crate::tag_source::open(path).ok()?;
            let file = MpegFile::read_from(&mut source, opts).ok()?;
            file.id3v2()?.into_iter().find_map(|frame| match frame {
                Frame::UserText(f) if f.description.eq_ignore_ascii_case(key) => {
                    Some(f.content.to_string())
                }
                _ => None,
            })
        }
        FileType::Flac => {
            let mut source = crate::tag_source::open(path).ok()?;
            let file = FlacFile::read_from(&mut source, opts).ok()?;

            file.vorbis_comments()?
                .items()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_carries_the_model_and_the_prefix_is_recognized() {
        assert_eq!(key("builtin-v1"), "ROX_ACOUSTIC:builtin-v1");
        assert!(is_key("ROX_ACOUSTIC:builtin-v1"));
        assert!(is_key("rox_acoustic:panns-cnn10"));
        assert!(!is_key("ROX_ACOUSTIC"), "the bare prefix names no model");
        assert!(!is_key("ROX_TEST"));
        assert!(!is_key("REPLAYGAIN_TRACK_GAIN"));
    }

    #[test]
    fn a_vector_survives_the_round_trip_to_f16_precision() {
        let vec = vec![0.5f32, -1.25, 0.0, 3.75, 128.0, -0.001_5, 12_000.0];
        let value = encode(&vec);
        assert!(value.starts_with("v1;dim=7;f16;"), "{value}");
        let back = decode(&value, vec.len()).unwrap();
        assert_eq!(back.len(), vec.len());
        for (a, b) in vec.iter().zip(&back) {
            // f16 holds about three significant digits, so the error is relative.
            let tolerance = (a.abs() * 1e-3).max(1e-6);
            assert!((a - b).abs() <= tolerance, "{a} came back as {b}");
        }
    }

    #[test]
    fn a_corrupt_value_is_refused_rather_than_guessed_at() {
        let vec = vec![1.0f32, 2.0, 3.0, 4.0];
        let good = encode(&vec);
        assert!(decode(&good, 4).is_some());

        assert!(decode(&good, 5).is_none(), "a wider model refuses it");
        assert!(
            decode("v1;dim=4;f16;AAAA", 4).is_none(),
            "payload too short"
        );
        assert!(decode("v2;dim=4;f16;AAAAAAAAAAAAAAAA", 4).is_none());
        assert!(decode("v1;dim=4;i8;AAAAAAAAAAAAAAAA", 4).is_none());
        assert!(decode(&format!("{good};extra"), 4).is_none());
        assert!(decode("not a vector at all", 4).is_none());
        assert!(decode("v1;dim=4;f16;not base64!!", 4).is_none());
        assert!(decode("v1;dim=four;f16;AAAA", 4).is_none());
        assert!(decode("v1;dim=4;f16", 4).is_none());
        assert!(decode("", 4).is_none());

        let poisoned = encode(&[1.0, f32::NAN, 3.0, 4.0]);
        assert!(decode(&poisoned, 4).is_none());
    }

    #[test]
    fn only_the_two_writable_formats_are_offered_a_tag() {
        assert!(writable(Path::new("/m/track.mp3")));
        assert!(writable(Path::new("/m/track.FLAC")));
        assert!(!writable(Path::new("/m/track.ogg")));
        assert!(!writable(Path::new("/m/track.m4a")));
        assert!(!writable(Path::new("/m/track.wav")));
        assert!(!writable(Path::new("/m/track")));
    }
}
