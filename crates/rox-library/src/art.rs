//! Cover art resolution: the picture embedded in a file's tags, read
//! through lofty (ADR 4's single metadata layer), with a cover image file
//! next to the track as the fallback. Hands back the encoded bytes and
//! their mime type; decoding and display stay with the caller. Blocking
//! file reads; run it off the UI thread.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use lofty::picture::{MimeType, PictureType};
use lofty::prelude::*;

/// File stems that count as folder art, best first.
const FOLDER_ART: &[&str] = &["cover", "folder", "front", "album"];
/// Image extensions folder art may carry.
const ART_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// The cover art for a track: the front cover from its tags (any embedded
/// picture failing that), else a cover image file in its folder. None when
/// neither exists or nothing identifies as an image.
pub fn cover_art(path: &Path) -> Option<(Vec<u8>, String)> {
    embedded(path).or_else(|| folder_art(path))
}

/// The embedded picture, isolated like the scanner's tag reads: a file
/// that errors or panics lofty's parser just has no art.
fn embedded(path: &Path) -> Option<(Vec<u8>, String)> {
    let file = catch_unwind(AssertUnwindSafe(|| lofty::read_from_path(path)))
        .ok()?
        .ok()?;
    let pictures: Vec<_> = file.tags().iter().flat_map(|tag| tag.pictures()).collect();
    let picture = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())?;
    // Tags lie about mime types often enough that a missing or unknown one
    // is worth rescuing off the magic bytes.
    let mime = match picture.mime_type() {
        Some(MimeType::Unknown(_)) | None => sniff(picture.data())?.into(),
        Some(mime) => mime.as_str().to_string(),
    };
    Some((picture.data().to_vec(), mime))
}

/// A cover image sitting next to the track, the best-ranked stem winning.
fn folder_art(path: &Path) -> Option<(Vec<u8>, String)> {
    let dir = path.parent()?;
    let mut best: Option<(usize, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let candidate = entry.path();
        let has_art_ext = candidate
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ART_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)));
        if !has_art_ext {
            continue;
        }
        let Some(rank) = candidate
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| FOLDER_ART.iter().position(|n| stem.eq_ignore_ascii_case(n)))
        else {
            continue;
        };
        if best.as_ref().map_or(true, |(r, _)| rank < *r) {
            best = Some((rank, candidate));
        }
    }
    let bytes = std::fs::read(best?.1).ok()?;
    let mime = sniff(&bytes)?.into();
    Some((bytes, mime))
}

/// The mime type off an image's magic bytes.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}
