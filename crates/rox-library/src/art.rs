//! Cover art resolution: the embedded picture read through lofty (ADR 4),
//! with a cover file beside the track as the fallback. Returns encoded bytes
//! and mime; decoding is the caller's. Blocking.
//!
//! One carve-out: ID3v2.4 tags whose header and APIC frame both flag
//! unsynchronisation get their picture read raw, because lofty (through
//! 0.24) de-syncs that shape twice and mangles the image. Bandcamp writes
//! exactly this. Drop the workaround once lofty reads it clean.

use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use lofty::picture::{MimeType, PictureType};
use lofty::prelude::*;

/// Best first.
const FOLDER_ART: &[&str] = &["cover", "folder", "front", "album"];
const ART_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// Every pick falls back through the front cover, any embedded picture, and
/// folder art.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtKind {
    #[default]
    Front,
    Back,
    /// ID3's "media" picture, the disc scan.
    Media,
    Artist,
}

impl ArtKind {
    /// Best first; APIC has two artist types.
    fn types(self) -> &'static [PictureType] {
        match self {
            ArtKind::Front => &[PictureType::CoverFront],
            ArtKind::Back => &[PictureType::CoverBack],
            ArtKind::Media => &[PictureType::Media],
            ArtKind::Artist => &[PictureType::Artist, PictureType::LeadArtist],
        }
    }

    /// The unsync path's copy of [`ArtKind::types`].
    fn apic_bytes(self) -> &'static [u8] {
        match self {
            ArtKind::Front => &[3],
            ArtKind::Back => &[4],
            ArtKind::Media => &[6],
            ArtKind::Artist => &[8, 7],
        }
    }

    /// Slot-specific stems rank ahead of the generic cover names.
    fn stems(self) -> &'static [&'static str] {
        match self {
            ArtKind::Front => FOLDER_ART,
            ArtKind::Back => &["back", "cover", "folder", "front", "album"],
            ArtKind::Media => &["disc", "media", "cd", "cover", "folder", "front", "album"],
            ArtKind::Artist => &["artist", "cover", "folder", "front", "album"],
        }
    }
}

/// The front cover from the tags (any picture failing that), else a cover
/// file in the folder.
pub fn cover_art(path: &Path) -> Option<(Vec<u8>, String)> {
    cover_art_source(path).art()
}

pub fn cover_art_of(path: &Path, kind: ArtKind) -> Option<(Vec<u8>, String)> {
    if let Some(art) = embedded(path, kind) {
        return Some(art);
    }
    folder_art(path, kind).art()
}

/// Lets a cache tell an embedded picture (covered by the audio file's
/// identity) from a folder cover that changes on its own.
pub enum ArtSource {
    Embedded,
    /// Stamped with the identity the file had just before its bytes were read.
    /// Statting after would pin half a download to the finished identity.
    Folder {
        file: std::path::PathBuf,
        mtime: i64,
        size: i64,
    },
}

/// Keeps "no art" apart from "art still landing": a downloader bumps the
/// folder's mtime once, on create, so caching the half-written state would
/// stick forever.
pub enum Cover {
    Found {
        bytes: Vec<u8>,
        mime: String,
        source: ArtSource,
    },
    /// A cover file the pick would take whose bytes aren't an image yet.
    Settling,
    None,
}

impl Cover {
    pub fn art(self) -> Option<(Vec<u8>, String)> {
        match self {
            Cover::Found { bytes, mime, .. } => Some((bytes, mime)),
            Cover::Settling | Cover::None => None,
        }
    }
}

/// [`cover_art`] plus its source, which the thumbnail cache keys a folder
/// cover's identity on.
pub fn cover_art_source(path: &Path) -> Cover {
    if let Some((bytes, mime)) = embedded(path, ArtKind::Front) {
        return Cover::Found {
            bytes,
            mime,
            source: ArtSource::Embedded,
        };
    }
    folder_art(path, ArtKind::Front)
}

/// (mtime, size), both zero when the path won't stat.
pub fn identity(path: &Path) -> (i64, i64) {
    match std::fs::metadata(path) {
        Ok(meta) => (
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            meta.len() as i64,
        ),
        Err(_) => (0, 0),
    }
}

/// Whether an image's bytes include their end marker, so a half-downloaded
/// file reads as one. Formats without an unambiguous end pass.
pub fn complete(bytes: &[u8]) -> bool {
    const TAIL: usize = 32;
    let tail = &bytes[bytes.len().saturating_sub(TAIL)..];
    let ends_with = |marker: &[u8]| tail.windows(marker.len()).any(|w| w == marker);
    match sniff(bytes) {
        // FF D9 can't occur inside stuffed scan data, so it's the real end.
        Some("image/jpeg") => ends_with(&[0xFF, 0xD9]),
        Some("image/png") => ends_with(b"IEND\xAEB\x60\x82"),
        // GIF's trailer is one byte, so take the last 0x3B and require zeros after.
        Some("image/gif") => tail
            .iter()
            .rposition(|b| *b == 0x3B)
            .is_some_and(|end| tail[end + 1..].iter().all(|b| *b == 0)),
        Some("image/webp") => bytes
            .get(4..8)
            .and_then(|n| n.try_into().ok())
            .is_some_and(|n| u32::from_le_bytes(n) as usize + 8 <= bytes.len()),
        _ => true,
    }
}

/// A file that errors or panics lofty just has no art. The pick runs the
/// slot's types, then the front cover, then any picture.
fn embedded(path: &Path, kind: ArtKind) -> Option<(Vec<u8>, String)> {
    if let Some(art) = unsync_apic(path, kind) {
        return Some(art);
    }
    let file = catch_unwind(AssertUnwindSafe(|| {
        lofty::probe::Probe::open(path).and_then(|p| p.options(crate::parse_opts()).read())
    }))
    .ok()?
    .ok()?;
    let pictures: Vec<_> = file.tags().iter().flat_map(|tag| tag.pictures()).collect();
    let picture = kind
        .types()
        .iter()
        .find_map(|t| pictures.iter().find(|p| p.pic_type() == *t))
        .or_else(|| {
            pictures
                .iter()
                .find(|p| p.pic_type() == PictureType::CoverFront)
        })
        .or_else(|| pictures.first())?;
    // Tags lie about mime types; rescue off the magic bytes.
    let mime = match picture.mime_type() {
        Some(MimeType::Unknown(_)) | None => sniff(picture.data())?.into(),
        Some(mime) => mime.as_str().to_string(),
    };
    Some((picture.data().to_vec(), mime))
}

/// The track's own-name image first, then the slot's stems. Unsniffable bytes
/// answer [`Cover::Settling`], not None.
fn folder_art(path: &Path, kind: ArtKind) -> Cover {
    let stems = kind.stems();
    let Some(dir) = path.parent() else {
        return Cover::None;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Cover::None;
    };

    // A picture named after the track outranks shared stems: a radio capture
    // writes one per song into a shared folder. Front slot only.
    let own = (kind == ArtKind::Front)
        .then(|| path.file_stem().and_then(|s| s.to_str()))
        .flatten();

    let mut best: Option<(usize, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let candidate = entry.path();
        let has_art_ext = candidate
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ART_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)));
        if !has_art_ext {
            continue;
        }
        let Some(stem) = candidate.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Rank 0 is the track's own name. Case-blind, like the stems.
        let rank = if own.is_some_and(|own| own.eq_ignore_ascii_case(stem)) {
            0
        } else {
            match stems.iter().position(|n| stem.eq_ignore_ascii_case(n)) {
                Some(rank) => rank + 1,
                None => continue,
            }
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, candidate));
        }
    }
    let Some((_, file)) = best else {
        return Cover::None;
    };
    let (mtime, size) = identity(&file);
    let Ok(bytes) = std::fs::read(&file) else {
        return Cover::Settling;
    };
    let Some(mime) = sniff(&bytes) else {
        return Cover::Settling;
    };
    Cover::Found {
        bytes,
        mime: mime.into(),
        source: ArtSource::Folder { file, mtime, size },
    }
}

/// The picture read raw out of an ID3v2.4 tag with the header unsync flag,
/// which lofty de-syncs twice. Only frames with their own unsync flag
/// qualify; their sizes count the stuffing, so the walk stays aligned. The
/// writer uses this too, so a commit doesn't write the mangled bytes back.
pub(crate) fn unsync_apic(path: &Path, kind: ArtKind) -> Option<(Vec<u8>, String)> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 10];
    file.read_exact(&mut header).ok()?;
    if &header[..3] != b"ID3" || header[3] != 4 || header[5] & 0x80 == 0 {
        return None;
    }
    let mut tag = vec![0u8; synchsafe(&header[6..10])? as usize];
    file.read_exact(&mut tag).ok()?;
    let mut pos = 0;
    // The extended header counts itself in its own size.
    if header[5] & 0x40 != 0 {
        pos = synchsafe(tag.get(..4)?)? as usize;
    }
    let preferred = kind.apic_bytes();
    let mut best: Option<(usize, (Vec<u8>, String))> = None;
    while pos + 10 <= tag.len() {
        let id = &tag[pos..pos + 4];
        if !id
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            break; // padding or junk: the frames are over
        }
        let size = synchsafe(&tag[pos + 4..pos + 8])? as usize;
        let flags = tag[pos + 9];
        let body = tag.get(pos + 10..pos + 10 + size)?;
        pos += 10 + size;
        if id != b"APIC" || flags & 0x02 == 0 {
            continue;
        }
        // A flagged data length indicator prefixes the content.
        let body = if flags & 0x01 != 0 {
            body.get(4..)?
        } else {
            body
        };
        let Some((pic_type, art)) = parse_apic(&resync(body)) else {
            continue;
        };
        // Same pick order as the lofty path.
        let rank = preferred
            .iter()
            .position(|b| *b == pic_type)
            .unwrap_or(if pic_type == 3 {
                preferred.len()
            } else {
                preferred.len() + 1
            });
        if rank == 0 {
            return Some(art);
        }
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, art));
        }
    }
    best.map(|(_, art)| art)
}

fn parse_apic(body: &[u8]) -> Option<(u8, (Vec<u8>, String))> {
    let encoding = *body.first()?;
    let mime_end = 1 + body.get(1..)?.iter().position(|b| *b == 0)?;
    let declared = String::from_utf8_lossy(&body[1..mime_end]).into_owned();
    let pic_type = *body.get(mime_end + 1)?;
    // One nul ends latin1/utf8, a nul pair ends utf16.
    let desc_start = mime_end + 2;
    let data_start = match encoding {
        1 | 2 => {
            let mut i = desc_start;
            while *body.get(i..i + 2)? != [0, 0] {
                i += 2;
            }
            i + 2
        }
        _ => desc_start + body.get(desc_start..)?.iter().position(|b| *b == 0)? + 1,
    };
    let data = body.get(data_start..)?.to_vec();
    if data.is_empty() {
        return None;
    }
    // Magic bytes beat a lying tag; the tag stands when the magic says nothing.
    let mime = sniff(&data).map_or(declared, str::to_string);
    if mime.is_empty() {
        return None;
    }
    Some((pic_type, (data, mime)))
}

fn resync(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        out.push(data[i]);
        if data[i] == 0xFF && data.get(i + 1) == Some(&0x00) {
            i += 1; // skip the stuffed zero
        }
        i += 1;
    }
    out
}

/// None when a byte has its high bit set.
pub(crate) fn synchsafe(bytes: &[u8]) -> Option<u32> {
    let quad: [u8; 4] = bytes.try_into().ok()?;
    if quad.iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    Some(quad.iter().fold(0u32, |acc, b| acc << 7 | u32::from(*b)))
}

/// The value must fit 28 bits.
pub(crate) fn synchsafe_encode(n: u32) -> [u8; 4] {
    [
        (n >> 21) as u8 & 0x7F,
        (n >> 14) as u8 & 0x7F,
        (n >> 7) as u8 & 0x7F,
        n as u8 & 0x7F,
    ]
}

pub(crate) fn sniff(bytes: &[u8]) -> Option<&'static str> {
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

/// The extension for a picture's bytes, only ones [`folder_art`] reads back.
/// GIF and BMP answer None.
pub fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    match sniff(bytes)? {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resync_is_a_single_pass() {
        assert_eq!(resync(&[0xFF, 0x00, 0x00, 0x59]), [0xFF, 0x00, 0x59]);
        assert_eq!(resync(&[0xFF, 0x00, 0xE0]), [0xFF, 0xE0]);
        assert_eq!(resync(&[0x01, 0x00, 0xFF]), [0x01, 0x00, 0xFF]);
    }

    #[test]
    fn complete_reads_the_end_marker() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x41, 0xFF, 0xD9];
        assert!(complete(&jpeg));
        assert!(!complete(&jpeg[..jpeg.len() - 2]));
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend(b"IEND\xAEB\x60\x82");
        assert!(complete(&png));
        assert!(!complete(&png[..png.len() - 1]));
        assert!(complete(b"not an image at all"));
    }

    #[test]
    fn complete_reads_a_padded_gif_trailer() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend([0x2C, 0x01, 0x02, 0x03]);
        assert!(!complete(&gif));
        gif.push(0x3B);
        assert!(complete(&gif));
        gif.extend([0u8; 8]);
        assert!(complete(&gif), "trailing zero padding still ends");
    }

    fn stuff(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, b) in data.iter().enumerate() {
            out.push(*b);
            if *b == 0xFF && data.get(i + 1).is_some_and(|n| *n == 0x00 || *n >= 0xE0) {
                out.push(0x00);
            }
        }
        out
    }

    fn synch(n: u32) -> [u8; 4] {
        [
            (n >> 21) as u8 & 0x7F,
            (n >> 14) as u8 & 0x7F,
            (n >> 7) as u8 & 0x7F,
            n as u8 & 0x7F,
        ]
    }

    #[test]
    fn unsync_apic_survives_ff_runs() {
        let image = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xFF, 0x00, 0x59, 0xFF, 0xFF, 0xD9,
        ];
        let mut body = vec![0x01];
        body.extend(b"image/jpeg\0");
        body.push(3); // front cover
        body.extend([0xFF, 0xFE, b'c', 0x00, 0x00, 0x00]); // BOM'd "c", nul pair
        body.extend(image);
        let stored = stuff(&body);
        let mut frame = b"APIC".to_vec();
        frame.extend(synch(stored.len() as u32 + 4)); // content plus the indicator
        frame.extend([0x00, 0x03]); // unsynchronised, data length indicator
        frame.extend(synch(body.len() as u32));
        frame.extend(&stored);
        let mut tag = b"ID3\x04\x00\x80".to_vec();
        tag.extend(synch(frame.len() as u32));
        tag.extend(&frame);

        // An empty dir, so folder art can't mask a broken raw path.
        let dir = std::env::temp_dir().join("rox-art-unsync-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("track.mp3");
        std::fs::write(&path, &tag).unwrap();
        let art = cover_art(&path);
        std::fs::remove_dir_all(&dir).unwrap();

        let (bytes, mime) = art.expect("the picture should resolve");
        assert_eq!(bytes, image);
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn a_picture_named_after_the_track_wins() {
        let dir = std::env::temp_dir().join("rox-art-sidecar-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let own = dir.join("Pendulum - Propane Nightmares.mp3");
        let other = dir.join("Koven - Take It Away.mp3");
        std::fs::write(&own, b"not really audio").unwrap();
        std::fs::write(&other, b"not really audio").unwrap();
        std::fs::write(dir.join("cover.png"), png(b"shared")).unwrap();
        std::fs::write(
            dir.join("pendulum - propane nightmares.png"),
            png(b"its own"),
        )
        .unwrap();

        let art = |path: &Path| cover_art(path).expect("a picture").0;
        assert_eq!(art(&own), png(b"its own"));
        assert_eq!(art(&other), png(b"shared"));

        let back = cover_art_of(&own, ArtKind::Back).expect("a picture").0;
        assert_eq!(back, png(b"shared"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn png(mark: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend(mark);
        bytes
    }
}
