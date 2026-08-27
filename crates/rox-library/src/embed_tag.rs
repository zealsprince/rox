//! An acoustic vector as a tag a file can hold, so a description outlives
//! the database it was computed into.
//!
//! [`crate::embeddings`] is the query engine and stays that way: every read
//! path goes through SQLite. This is the optional second copy, written into
//! the file itself when the acoustic setting asks for it, so a wiped library
//! or a folder moved to another machine gets its vectors back off the files
//! instead of decoding a library again.
//!
//! ## The key
//!
//! `ROX_ACOUSTIC:<model-id>` works untranslated as both an ID3v2 TXXX
//! description and a Vorbis comment key, which is the whole reason for the
//! spelling: one string, two formats, no per-format table to keep in step.
//! The model id is part of the key rather than the value, so two models'
//! vectors coexist in one file the same way they occupy two database rows,
//! and a reader asking for one never has to parse the other's.
//!
//! ## The value
//!
//! `v1;dim=<n>;f16;<base64>`: a version, the width, the number format, and
//! the vector. Everything before the payload is there so a reader can refuse
//! a value it doesn't understand instead of guessing at one.
//!
//! Half floats, and deliberately not integers. The vectors go in raw and
//! unnormalized (see [`crate::embeddings`]'s header) and their dimensions
//! span wildly different scales, so an int8 quantization would need a
//! per-dimension scale factor to mean anything, and getting one wrong turns
//! a neighbour list into noise. f16 has no such knob: it keeps three decimal
//! digits at every magnitude, and the query z-scores each dimension against
//! the corpus anyway, which throws away far more precision than the encoding
//! does. Half the bytes of f32 for a difference nothing downstream can see.

use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use half::f16;
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::Frame;
use lofty::mpeg::MpegFile;
use lofty::probe::Probe;

/// What every acoustic key starts with. The tag editor and the metadata
/// panel skip anything with it: these are numbers a machine wrote for
/// another machine, and a row of base64 in a field list is noise.
pub const PREFIX: &str = "ROX_ACOUSTIC:";

/// The one version this module writes and the only one it reads.
const VERSION: &str = "v1";

/// The tag key one model's vectors are stored under.
pub fn key(model: &str) -> String {
    format!("{PREFIX}{model}")
}

/// Whether a tag key belongs to this module. Case-insensitive, because
/// Vorbis keys are case-insensitive by spec and a tagger that round-tripped
/// a file may have changed the casing.
pub fn is_key(key: &str) -> bool {
    key.len() > PREFIX.len() && key[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
}

/// Whether a path is one the writer can put a vector into. Extension rather
/// than content, because this is the cheap pre-check that keeps an
/// unsupported format's skip quiet: the writer handles MP3 and FLAC only,
/// and probing every OGG in a library to be told so again would cost a file
/// open per track. A file whose extension lies still fails in the writer,
/// where it's a real error worth logging.
pub fn writable(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3") || e.eq_ignore_ascii_case("flac"))
}

/// A vector as the tag value spells it.
pub fn encode(vec: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(vec.len() * 2);
    for v in vec {
        bytes.extend_from_slice(&f16::from_f32(*v).to_le_bytes());
    }
    format!("{VERSION};dim={};f16;{}", vec.len(), BASE64.encode(&bytes))
}

/// The vector back out, or None for anything this module didn't write and
/// anything that rotted since it did.
///
/// `dim` is what the caller's model produces. A value of another width is
/// refused rather than returned short: it would come from a model whose
/// output changed under the same name, and the store's own width check would
/// only drop the row later, after the read had already claimed the track was
/// covered and skipped its decode.
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
    // Nothing may follow the payload: a fifth field means a spelling this
    // version doesn't know, and reading the first four of it would be
    // guessing.
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
    // A NaN or an infinity poisons every score in the library once it's in
    // the table (see [`crate::embeddings::upsert`]), and a tag anyone can
    // edit by hand is exactly where one would come from.
    vec.iter().all(|v| v.is_finite()).then_some(vec)
}

/// One model's vector out of a file's tags, or None when the file has
/// none, isn't a format that can hold one, or holds one this build can't
/// read.
///
/// Reads only: the write side is [`crate::writer::commit_embedding`], which
/// needs the whole atomic clone-verify-rename layer this doesn't.
pub fn read(path: &Path, model: &str, dim: usize) -> Option<Vec<f32>> {
    let value = read_value(path, &key(model))?;
    decode(&value, dim)
}

/// The raw value under one key, through the same sanitising source and
/// relaxed parse the writer's reads use, so a tag that only lofty's strict
/// mode objects to still gives its vector up.
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
            let found = file
                .vorbis_comments()?
                .items()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.to_string());
            found
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spelling is the contract: two formats read the same key, and the
    /// value says what it is before it says what it holds.
    #[test]
    fn the_key_carries_the_model_and_the_prefix_is_recognized() {
        assert_eq!(key("builtin-v1"), "ROX_ACOUSTIC:builtin-v1");
        assert!(is_key("ROX_ACOUSTIC:builtin-v1"));
        // Vorbis keys are case-insensitive, so a tagger that upper-cased the
        // file's keys must not hide the row from the editor's skip.
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
            // Half floats hold about three decimal digits, so the error is
            // relative rather than absolute: a band energy in the thousands
            // is allowed to move by a few, a rate near one is not.
            let tolerance = (a.abs() * 1e-3).max(1e-6);
            assert!((a - b).abs() <= tolerance, "{a} came back as {b}");
        }
    }

    /// Nothing but this module's own output is accepted. Every one of these
    /// would otherwise read as a vector the pass then trusts enough to skip
    /// a decode over, which is the expensive kind of wrong: the track ends
    /// up in the corpus describing something it isn't.
    #[test]
    fn a_corrupt_value_is_refused_rather_than_guessed_at() {
        let vec = vec![1.0f32, 2.0, 3.0, 4.0];
        let good = encode(&vec);
        assert!(decode(&good, 4).is_some());

        // The width the caller's model produces has to match the width the
        // value claims, and the payload has to be as long as it claims.
        assert!(decode(&good, 5).is_none(), "a wider model refuses it");
        assert!(
            decode("v1;dim=4;f16;AAAA", 4).is_none(),
            "payload too short"
        );
        // A version, a number format, and a field count this build doesn't
        // know are all refusals rather than best guesses.
        assert!(decode("v2;dim=4;f16;AAAAAAAAAAAAAAAA", 4).is_none());
        assert!(decode("v1;dim=4;i8;AAAAAAAAAAAAAAAA", 4).is_none());
        assert!(decode(&format!("{good};extra"), 4).is_none());
        // Garbage in every shape it arrives in: a hand-edited tag, a
        // truncated one, an empty one, and a number that isn't one.
        assert!(decode("not a vector at all", 4).is_none());
        assert!(decode("v1;dim=4;f16;not base64!!", 4).is_none());
        assert!(decode("v1;dim=four;f16;AAAA", 4).is_none());
        assert!(decode("v1;dim=4;f16", 4).is_none());
        assert!(decode("", 4).is_none());

        // A value with a NaN in it reads as nothing. It's finite-checked here
        // as well as at the store, because a tag is a text field anyone can
        // type into and one NaN makes every score in the library NaN.
        let poisoned = encode(&[1.0, f32::NAN, 3.0, 4.0]);
        assert!(decode(&poisoned, 4).is_none());
    }

    /// The formats the writer can reach, off the name alone. Everything else
    /// keeps its database row and skips the tag without a word.
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
