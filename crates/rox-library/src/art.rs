//! Cover art resolution: the picture embedded in a file's tags, read
//! through lofty (ADR 4's single metadata layer), with a cover image file
//! next to the track as the fallback. Hands back the encoded bytes and
//! their mime type; decoding and display stay with the caller. Blocking
//! file reads; run it off the UI thread.
//!
//! One carve-out from the single layer: ID3v2.4 tags whose header and APIC
//! frame both flag unsynchronisation get their picture read raw here,
//! because lofty (through 0.24) de-unsynchronises that shape twice and
//! hands back mangled image bytes. Bandcamp's tagger writes exactly this,
//! so it covers a large slice of real libraries. Drop the workaround once
//! lofty reads these tags clean.

use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use lofty::picture::{MimeType, PictureType};
use lofty::prelude::*;

/// File stems that count as folder art, best first.
const FOLDER_ART: &[&str] = &["cover", "folder", "front", "album"];
/// Image extensions folder art may carry.
const ART_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// The picture slot a caller asks for, the tag picture types a player
/// shows. Every pick falls back through the front cover, any embedded
/// picture, and folder art, so asking for a slot a file doesn't carry
/// still resolves to something.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtKind {
    #[default]
    Front,
    Back,
    /// The disc scan, ID3's "media" picture.
    Media,
    Artist,
}

impl ArtKind {
    /// The lofty picture types that satisfy this slot exactly, best
    /// first; APIC keeps two artist types, so that pick accepts both.
    fn types(self) -> &'static [PictureType] {
        match self {
            ArtKind::Front => &[PictureType::CoverFront],
            ArtKind::Back => &[PictureType::CoverBack],
            ArtKind::Media => &[PictureType::Media],
            ArtKind::Artist => &[PictureType::Artist, PictureType::LeadArtist],
        }
    }

    /// The raw APIC type bytes for the same slot, the unsync path's copy
    /// of [`ArtKind::types`].
    fn apic_bytes(self) -> &'static [u8] {
        match self {
            ArtKind::Front => &[3],
            ArtKind::Back => &[4],
            ArtKind::Media => &[6],
            ArtKind::Artist => &[8, 7],
        }
    }

    /// The folder-art stems this slot prefers, ranked ahead of the
    /// generic cover names so a disc.jpg beside the track wins the media
    /// pick but a lone cover.jpg still answers.
    fn stems(self) -> &'static [&'static str] {
        match self {
            ArtKind::Front => FOLDER_ART,
            ArtKind::Back => &["back", "cover", "folder", "front", "album"],
            ArtKind::Media => &["disc", "media", "cd", "cover", "folder", "front", "album"],
            ArtKind::Artist => &["artist", "cover", "folder", "front", "album"],
        }
    }
}

/// The cover art for a track: the front cover from its tags (any embedded
/// picture failing that), else a cover image file in its folder. None when
/// neither exists or nothing identifies as an image.
pub fn cover_art(path: &Path) -> Option<(Vec<u8>, String)> {
    cover_art_source(path).map(|(bytes, mime, _)| (bytes, mime))
}

/// [`cover_art`] with the slot pick: the asked-for picture type from the
/// tags, falling back through the front cover, any embedded picture, and
/// the folder art ranked for the slot.
pub fn cover_art_of(path: &Path, kind: ArtKind) -> Option<(Vec<u8>, String)> {
    if let Some(art) = embedded(path, kind) {
        return Some(art);
    }
    folder_art(path, kind).map(|(bytes, mime, _)| (bytes, mime))
}

/// Where a track's resolved cover came from, so a cache keyed on file
/// identity can tell an embedded picture (covered by the audio file's own
/// mtime/size) from a folder cover that changes without touching the track.
pub enum ArtSource {
    /// The picture lived in the track's own tags.
    Embedded,
    /// The picture came from this image file beside the track, stamped
    /// with the identity it carried just before its bytes were read.
    /// Statting after the read instead would pin a cover that finished
    /// downloading mid-read to the identity it settled on, so a thumbnail
    /// built from half an image would match forever.
    Folder {
        file: std::path::PathBuf,
        mtime: i64,
        size: i64,
    },
}

/// [`cover_art`] plus where the picture came from. The thumbnail cache uses
/// the source to key a folder cover on that file's identity, so replacing
/// or adding cover.jpg invalidates a thumb the audio file's own mtime/size
/// would never notice.
pub fn cover_art_source(path: &Path) -> Option<(Vec<u8>, String, ArtSource)> {
    if let Some((bytes, mime)) = embedded(path, ArtKind::Front) {
        return Some((bytes, mime, ArtSource::Embedded));
    }
    folder_art(path, ArtKind::Front)
}

/// The (mtime, size) of a path, both zero when it will not stat. Caches
/// key on this, so a changed file reads as a fresh identity.
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

/// Whether an image's bytes carry their end marker, so a file caught
/// halfway through a download reads as what it is. Only the formats with
/// an unambiguous end answer; anything else passes, since guessing wrong
/// costs a cover. The marker scan tolerates the trailing padding some
/// encoders leave after it.
pub fn complete(bytes: &[u8]) -> bool {
    // Far enough back to clear padding, short enough that a marker found
    // here really is the end.
    const TAIL: usize = 32;
    let tail = &bytes[bytes.len().saturating_sub(TAIL)..];
    let ends_with = |marker: &[u8]| tail.windows(marker.len()).any(|w| w == marker);
    match sniff(bytes) {
        // FF D9 cannot occur inside entropy-coded scan data, where every
        // FF is stuffed with a zero, so finding it means the real end.
        Some("image/jpeg") => ends_with(&[0xFF, 0xD9]),
        Some("image/png") => ends_with(b"IEND\xAEB\x60\x82"),
        Some("image/gif") => tail.last() == Some(&0x3B),
        // RIFF containers count their own length in the header.
        Some("image/webp") => bytes
            .get(4..8)
            .and_then(|n| n.try_into().ok())
            .is_some_and(|n| u32::from_le_bytes(n) as usize + 8 <= bytes.len()),
        _ => true,
    }
}

/// The embedded picture for a slot, isolated like the scanner's tag
/// reads: a file that errors or panics lofty's parser just has no art.
/// Tags lofty is known to mangle take the raw path first. The pick runs
/// the slot's exact types, then the front cover, then any picture.
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
    // Tags lie about mime types often enough that a missing or unknown one
    // is worth rescuing off the magic bytes.
    let mime = match picture.mime_type() {
        Some(MimeType::Unknown(_)) | None => sniff(picture.data())?.into(),
        Some(mime) => mime.as_str().to_string(),
    };
    Some((picture.data().to_vec(), mime))
}

/// A cover image sitting next to the track, the slot's best-ranked stem
/// winning. Hands back the file it read, and the identity that file had
/// going in, so a cache can key on it.
fn folder_art(path: &Path, kind: ArtKind) -> Option<(Vec<u8>, String, ArtSource)> {
    let stems = kind.stems();
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
            .and_then(|stem| stems.iter().position(|n| stem.eq_ignore_ascii_case(n)))
        else {
            continue;
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, candidate));
        }
    }
    let file = best?.1;
    // Stat first, read second: a cover that finishes downloading between
    // the two then reads as a file the cache has never seen, instead of
    // stamping its finished identity on the bytes we caught mid-write.
    let (mtime, size) = identity(&file);
    let bytes = std::fs::read(&file).ok()?;
    let mime = sniff(&bytes)?.into();
    Some((bytes, mime, ArtSource::Folder { file, mtime, size }))
}

/// The picture pulled raw out of an ID3v2.4 tag whose header sets the
/// unsynchronisation flag. lofty de-unsynchronises such a tag whole, then
/// again per frame for the frame's own flag, so every stuffed `ff 00 00`
/// collapses to `ff` instead of `ff 00` and the image never decodes. Only
/// frames carrying their own flag qualify - per the v2.4 spec the scheme
/// is applied frame by frame, so their sizes count the stuffed bytes as
/// stored and the walk below stays aligned. A tag unsynchronised as one
/// stream (header flag alone) reads fine through lofty and stays there.
/// None means the tag is not this shape; the lofty path takes over. The
/// writer leans on the same probe to carry the picture through a commit,
/// since lofty would hand it the mangled bytes to write back.
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
    // The extended header sits before the frames and counts itself in its
    // own size.
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
        // The data length indicator, when flagged, prefixes the content.
        let body = if flags & 0x01 != 0 {
            body.get(4..)?
        } else {
            body
        };
        let Some((pic_type, art)) = parse_apic(&resync(body)) else {
            continue;
        };
        // The slot's own types win outright, the front cover stands in,
        // any other picture last, mirroring the lofty path's pick.
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

/// A de-unsynchronised APIC body split into its picture type and the
/// image bytes with their mime type.
fn parse_apic(body: &[u8]) -> Option<(u8, (Vec<u8>, String))> {
    let encoding = *body.first()?;
    let mime_end = 1 + body.get(1..)?.iter().position(|b| *b == 0)?;
    let declared = String::from_utf8_lossy(&body[1..mime_end]).into_owned();
    let pic_type = *body.get(mime_end + 1)?;
    // The description ends on one nul for latin1/utf8, a nul pair on
    // utf16's two-byte grid.
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
    // The same rescue the lofty path runs: magic bytes beat a lying tag,
    // the tag's claim stands when the magic says nothing.
    let mime = sniff(&data).map_or(declared, str::to_string);
    if mime.is_empty() {
        return None;
    }
    Some((pic_type, (data, mime)))
}

/// One pass of un-unsynchronisation: every `ff 00` collapses back to `ff`,
/// restoring the bytes the stuffing hid from mpeg sync scanners.
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

/// A 4-byte synchsafe integer; None when a byte has its high bit set,
/// which no conforming tag writes.
pub(crate) fn synchsafe(bytes: &[u8]) -> Option<u32> {
    let quad: [u8; 4] = bytes.try_into().ok()?;
    if quad.iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    Some(quad.iter().fold(0u32, |acc, b| acc << 7 | u32::from(*b)))
}

/// The 4-byte synchsafe encode, the write-side mirror of [`synchsafe`].
/// The value must fit 28 bits; callers bound it before asking.
pub(crate) fn synchsafe_encode(n: u32) -> [u8; 4] {
    [
        (n >> 21) as u8 & 0x7F,
        (n >> 14) as u8 & 0x7F,
        (n >> 7) as u8 & 0x7F,
        n as u8 & 0x7F,
    ]
}

/// The mime type off an image's magic bytes.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The stuffing the resync must undo exactly once: `ff 00 00` back to
    /// `ff 00`, `ff 00 xx` back to `ff xx`. Stripping the pair twice is
    /// the lofty bug the raw path exists for.
    #[test]
    fn resync_is_a_single_pass() {
        assert_eq!(resync(&[0xFF, 0x00, 0x00, 0x59]), [0xFF, 0x00, 0x59]);
        assert_eq!(resync(&[0xFF, 0x00, 0xE0]), [0xFF, 0xE0]);
        assert_eq!(resync(&[0x01, 0x00, 0xFF]), [0x01, 0x00, 0xFF]);
    }

    /// The end-marker check the thumbnail cache leans on: a whole file
    /// passes, the front of one still downloading does not, and a format
    /// with no marker to read passes rather than lose its cover.
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

    /// The unsynchronisation an encoder applies: a zero stuffed after
    /// every `ff` that precedes a zero or a sync-shaped byte.
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

    /// A 4-byte synchsafe encode, the write-side mirror of `synchsafe`.
    fn synch(n: u32) -> [u8; 4] {
        [
            (n >> 21) as u8 & 0x7F,
            (n >> 14) as u8 & 0x7F,
            (n >> 7) as u8 & 0x7F,
            n as u8 & 0x7F,
        ]
    }

    /// A file holding just an ID3v2.4 tag shaped like Bandcamp's: header
    /// unsynchronisation flag set, the APIC frame flagged unsynchronised
    /// with a data length indicator, a utf16 description. The picture must
    /// come back byte-identical through the raw path.
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

        // An empty scratch dir so the folder-art fallback can never answer
        // for a broken raw path.
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
}
