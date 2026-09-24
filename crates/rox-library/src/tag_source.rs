//! A tag read source working around ID3v2 shapes lofty (through 0.24) reads
//! mangled or not at all, by handing it a corrected in-memory copy. A commit
//! through the writer rewrites the tag clean. Drop the module once lofty
//! handles these itself.
//!
//! - Double de-unsynchronisation: with the v2.4 header unsync flag set,
//!   lofty de-syncs the stream and then each flagged frame again, so the
//!   bytes shift and the read aborts. Frames are only ever unsynced once, so
//!   clearing the header flag reads clean.
//! - A stray null after a UTF-16 frame's terminator: lofty rejects the odd
//!   length and aborts the tag, even in Relaxed mode. Trimming it fixes it.
//! - Junk between the declared tag end and the first MPEG frame (Bandcamp's
//!   leftover LAME frame): reads fine, but lofty's write path gives up past
//!   1024 junk bytes. [`needs_repair`] flags it and the writer folds the
//!   junk into the tag.

use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use crate::art::{synchsafe, synchsafe_encode};

const HEADER_UNSYNC: u8 = 0x80;
const HEADER_EXTENDED: u8 = 0x40;
const HEADER_FOOTER: u8 = 0x10;
const FRAME_UNSYNC: u8 = 0x02;
/// lofty's DEFAULT_MAX_JUNK_BYTES: past this a write can't find the audio.
const GAP_FLAG: u64 = 1024;
/// Past this the "gap" is a corrupt file, not slack.
pub(crate) const GAP_SCAN_CAP: u64 = 1 << 24;

/// The file untouched, or a corrected in-memory copy.
pub(crate) enum TagSource {
    File(File),
    Patched(Cursor<Vec<u8>>),
}

impl Read for TagSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            TagSource::File(f) => f.read(buf),
            TagSource::Patched(c) => c.read(buf),
        }
    }
}

impl Seek for TagSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            TagSource::File(f) => f.seek(pos),
            TagSource::Patched(c) => c.seek(pos),
        }
    }
}

/// Reads only the tag region, which lofty reads right after anyway.
pub(crate) fn open(path: &Path) -> io::Result<TagSource> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 10];
    if file.read_exact(&mut header).is_err()
        || &header[..3] != b"ID3"
        || !matches!(header[3], 3 | 4)
    {
        file.rewind()?;
        return Ok(TagSource::File(file));
    }
    let Some(size) = synchsafe(&header[6..10]) else {
        file.rewind()?;
        return Ok(TagSource::File(file));
    };
    let mut tag = vec![0u8; size as usize];
    if file.read_exact(&mut tag).is_err() {
        // A declared size past EOF: let lofty report it.
        file.rewind()?;
        return Ok(TagSource::File(file));
    }
    // The header flag alone reads fine; only a flagged frame triggers the
    // second pass.
    let unsync =
        header[3] == 4 && header[5] & HEADER_UNSYNC != 0 && frames_flagged(header[5], &tag);
    let trimmed = trim_odd_utf16(header[3], header[5], &mut tag);
    if !unsync && !trimmed {
        file.rewind()?;
        return Ok(TagSource::File(file));
    }
    let mut buf = Vec::with_capacity(10 + tag.len());
    buf.extend_from_slice(&header);
    if unsync {
        buf[5] &= !HEADER_UNSYNC;
    }
    buf.extend_from_slice(&tag);
    file.read_to_end(&mut buf)?;
    Ok(TagSource::Patched(Cursor::new(buf)))
}

/// Whether a rewrite through the writer would repair this file: the two
/// shapes [`open`] corrects, or a junk gap past lofty's limit. Errors read
/// as "no repair needed".
pub fn needs_repair(path: &Path) -> bool {
    needs_repair_inner(path).unwrap_or(false)
}

fn needs_repair_inner(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 10];
    if file.read_exact(&mut header).is_err()
        || &header[..3] != b"ID3"
        || !matches!(header[3], 3 | 4)
    {
        return Ok(false);
    }
    let Some(size) = synchsafe(&header[6..10]) else {
        return Ok(false);
    };
    let mut tag = vec![0u8; size as usize];
    if file.read_exact(&mut tag).is_err() {
        return Ok(false);
    }
    if header[3] == 4 && header[5] & HEADER_UNSYNC != 0 && frames_flagged(header[5], &tag) {
        return Ok(true);
    }
    if trim_odd_utf16(header[3], header[5], &mut tag) {
        return Ok(true);
    }
    Ok(tag_gap(&mut file)?.is_some_and(|gap| gap.junk > GAP_FLAG && gap.sync))
}

pub(crate) struct TagGap {
    pub size: u32,
    pub junk: u64,
    pub sync: bool,
}

/// `None` when the file isn't ID3v2.3/4 or has a footer. Seeks freely.
pub(crate) fn tag_gap(file: &mut File) -> io::Result<Option<TagGap>> {
    let mut header = [0u8; 10];
    file.seek(SeekFrom::Start(0))?;
    if file.read_exact(&mut header).is_err()
        || &header[..3] != b"ID3"
        || !matches!(header[3], 3 | 4)
        || header[5] & HEADER_FOOTER != 0
    {
        return Ok(None);
    }
    let Some(size) = synchsafe(&header[6..10]) else {
        return Ok(None);
    };
    file.seek(SeekFrom::Start(10 + u64::from(size)))?;
    let (junk, sync) = scan_to_sync(file, GAP_SCAN_CAP)?;
    Ok(Some(TagGap { size, junk, sync }))
}

/// Shared by the gap probe and the writer's audio span, so the two agree on
/// where audio starts.
pub(crate) fn scan_to_sync(file: &mut File, cap: u64) -> io::Result<(u64, bool)> {
    let mut skipped = 0u64;
    let mut prev = 0u8;
    let mut buf = [0u8; 8192];
    while skipped < cap {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if prev == 0xFF && b & 0xE0 == 0xE0 {
                return Ok((skipped - 1, true));
            }
            prev = b;
            skipped += 1;
        }
    }
    Ok((skipped, false))
}

/// v2.4 frame sizes count the stuffing, so this walks the raw bytes aligned.
fn frames_flagged(flags: u8, tag: &[u8]) -> bool {
    frames_flagged_inner(flags, tag).unwrap_or(false)
}

fn frames_flagged_inner(flags: u8, tag: &[u8]) -> Option<bool> {
    let mut pos = 0;
    // The extended header counts itself in its own size.
    if flags & HEADER_EXTENDED != 0 {
        pos = synchsafe(tag.get(..4)?)? as usize;
    }
    while pos + 10 <= tag.len() {
        let id = &tag[pos..pos + 4];
        if !id
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            break; // padding or junk: the frames are over
        }
        let fsize = synchsafe(&tag[pos + 4..pos + 8])? as usize;
        if tag[pos + 9] & FRAME_UNSYNC != 0 {
            return Some(true);
        }
        pos += 10 + fsize;
    }
    Some(false)
}

/// Fires only on the exact shape: a `T...` frame, no format flags, odd
/// UTF-16 content ending in null. The slack becomes trailing padding, so the
/// declared tag size stays put.
fn trim_odd_utf16(version: u8, flags: u8, tag: &mut [u8]) -> bool {
    if flags & HEADER_EXTENDED != 0 {
        return false; // rare enough not to be worth walking past
    }
    let mut changed = false;
    let mut read = 0usize;
    let mut write = 0usize;
    while read + 10 <= tag.len() {
        if !tag[read..read + 4]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            break; // padding or junk: the frames are over
        }
        let Some(fsize) = frame_size(version, &tag[read + 4..read + 8]) else {
            break;
        };
        let fsize = fsize as usize;
        if fsize == 0 || read + 10 + fsize > tag.len() {
            break;
        }
        let content = &tag[read + 10..read + 10 + fsize];
        // An odd text length means an even frame size, after the encoding byte.
        let stray = tag[read] == b'T'
            && tag[read + 9] == 0
            && fsize >= 2
            && matches!(content[0], 0x01 | 0x02)
            && fsize.is_multiple_of(2)
            && content[fsize - 1] == 0x00;
        let new_size = if stray { fsize - 1 } else { fsize };
        if stray {
            changed = true;
            tag.copy_within(read..read + 4, write);
            let encoded = encode_frame_size(version, new_size as u32);
            tag[write + 4..write + 8].copy_from_slice(&encoded);
            tag.copy_within(read + 8..read + 10, write + 8);
            tag.copy_within(read + 10..read + 10 + new_size, write + 10);
        } else if write != read {
            tag.copy_within(read..read + 10 + fsize, write);
        }
        read += 10 + fsize;
        write += 10 + new_size;
    }
    if changed {
        tag[write..read].fill(0);
    }
    changed
}

fn frame_size(version: u8, bytes: &[u8]) -> Option<u32> {
    match version {
        4 => synchsafe(bytes),
        _ => Some(u32::from_be_bytes(bytes.try_into().ok()?)),
    }
}

fn encode_frame_size(version: u8, size: u32) -> [u8; 4] {
    match version {
        4 => synchsafe_encode(size),
        _ => size.to_be_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::config::ParsingMode;
    use lofty::file::AudioFile;
    use lofty::mpeg::MpegFile;
    use lofty::prelude::*;

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

    fn utf16_text_frame(id: &[u8; 4], text: &str) -> Vec<u8> {
        let mut body = vec![0x01]; // utf16 encoding byte
        body.extend([0xFF, 0xFE]); // little-endian BOM
        for ch in text.encode_utf16() {
            body.extend(ch.to_le_bytes());
        }
        let stored = stuff(&body);
        let mut frame = id.to_vec();
        frame.extend(synch(stored.len() as u32 + 4)); // content plus the indicator
        frame.extend([0x00, 0x03]); // unsynchronised, data length indicator
        frame.extend(synch(body.len() as u32));
        frame.extend(&stored);
        frame
    }

    fn mpeg_audio() -> Vec<u8> {
        let mut frame = vec![0xFF, 0xFB, 0x90, 0x64];
        frame.extend(std::iter::repeat_n(0u8, 413)); // 417-byte frame
        frame.repeat(4)
    }

    /// Vide Noir's shape: header unsync flag set, every text frame flagged too.
    fn vide_noir_file(title: &str) -> Vec<u8> {
        let mut frames = utf16_text_frame(b"TIT2", title);
        frames.extend(utf16_text_frame(b"TPE1", "Lord Huron"));
        let mut file = b"ID3\x04\x00\x80".to_vec();
        file.extend(synch(frames.len() as u32));
        file.extend(&frames);
        file.extend(mpeg_audio());
        file
    }

    #[test]
    fn sanitiser_recovers_the_double_unsync_shape() {
        let opts = crate::parse_opts().parsing_mode(ParsingMode::Relaxed);
        let file = vide_noir_file("Back from the Edge");

        let mut raw = Cursor::new(file.clone());
        let mangled = MpegFile::read_from(&mut raw, opts)
            .ok()
            .and_then(|f| f.id3v2().and_then(|t| t.title().map(|s| s.into_owned())));
        assert_ne!(
            mangled.as_deref(),
            Some("Back from the Edge"),
            "the raw shape should not read the title back intact"
        );

        let dir = std::env::temp_dir().join("rox-tag-source-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("07 Back from the Edge.mp3");
        std::fs::write(&path, &file).unwrap();
        let mut source = open(&path).unwrap();
        let parsed = MpegFile::read_from(&mut source, opts).expect("the sanitised tag parses");
        std::fs::remove_dir_all(&dir).unwrap();

        let id3 = parsed.id3v2().expect("the tag survives");
        assert_eq!(id3.title().as_deref(), Some("Back from the Edge"));
        assert_eq!(id3.artist().as_deref(), Some("Lord Huron"));
    }

    #[test]
    fn needs_repair_flags_only_the_broken_shape() {
        let dir = std::env::temp_dir().join("rox-tag-source-needs-repair");
        std::fs::create_dir_all(&dir).unwrap();

        let broken = dir.join("broken.mp3");
        std::fs::write(&broken, vide_noir_file("Ends of the Earth")).unwrap();
        assert!(needs_repair(&broken), "the broken shape flags");

        let plain = dir.join("plain.mp3");
        std::fs::write(&plain, mpeg_audio()).unwrap();
        assert!(!needs_repair(&plain), "a plain file is left alone");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn v23_text_frame(id: &[u8; 4], text: &str, stray: bool) -> Vec<u8> {
        let mut body = vec![0x01]; // utf16 encoding byte
        body.extend([0xFF, 0xFE]); // little-endian BOM
        for ch in text.encode_utf16() {
            body.extend(ch.to_le_bytes());
        }
        body.extend([0x00, 0x00]); // terminator
        if stray {
            body.push(0x00);
        }
        let mut frame = id.to_vec();
        frame.extend((body.len() as u32).to_be_bytes());
        frame.extend([0x00, 0x00]);
        frame.extend(&body);
        frame
    }

    fn v23_file(title: &str, stray: bool, gap: usize) -> Vec<u8> {
        let mut frames = v23_text_frame(b"TIT2", title, stray);
        frames.extend(v23_text_frame(b"TPE1", "Paris Texas", false));
        let mut file = b"ID3\x03\x00\x00".to_vec();
        file.extend(synch(frames.len() as u32));
        file.extend(&frames);
        file.extend(std::iter::repeat_n(0u8, gap));
        file.extend(mpeg_audio());
        file
    }

    #[test]
    fn sanitiser_trims_the_stray_utf16_null() {
        let opts = crate::parse_opts();
        let title = "Everybody's Safe Until\u{2026}";
        let file = v23_file(title, true, 0);

        let mut raw = Cursor::new(file.clone());
        assert!(
            MpegFile::read_from(&mut raw, opts).is_err(),
            "the raw shape should abort the tag read"
        );

        let dir = std::env::temp_dir().join("rox-tag-source-stray-null");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Everybody's Safe Until.mp3");
        std::fs::write(&path, &file).unwrap();
        assert!(needs_repair(&path), "the stray null flags for repair");
        let mut source = open(&path).unwrap();
        let parsed = MpegFile::read_from(&mut source, opts).expect("the sanitised tag parses");
        std::fs::remove_dir_all(&dir).unwrap();

        let id3 = parsed.id3v2().expect("the tag survives");
        assert_eq!(id3.title().as_deref(), Some(title));
        assert_eq!(id3.artist().as_deref(), Some("Paris Texas"));
    }

    #[test]
    fn healthy_v23_file_is_left_alone() {
        let dir = std::env::temp_dir().join("rox-tag-source-healthy-v23");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plain.mp3");
        std::fs::write(&path, v23_file("Closed Caption", false, 0)).unwrap();
        assert!(!needs_repair(&path), "a healthy tag is left alone");
        assert!(
            matches!(open(&path).unwrap(), TagSource::File(_)),
            "a healthy tag reads straight off the file"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn needs_repair_flags_only_the_deep_gap() {
        let dir = std::env::temp_dir().join("rox-tag-source-gap");
        std::fs::create_dir_all(&dir).unwrap();

        let deep = dir.join("deep.mp3");
        std::fs::write(&deep, v23_file("Fading", false, 1500)).unwrap();
        assert!(needs_repair(&deep), "a gap past the probe limit flags");

        let shallow = dir.join("shallow.mp3");
        std::fs::write(&shallow, v23_file("Fading", false, 500)).unwrap();
        assert!(!needs_repair(&shallow), "a tolerable gap is left alone");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
