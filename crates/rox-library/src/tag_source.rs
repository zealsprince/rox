//! A tag read source that works around lofty's double de-unsynchronisation
//! of ID3v2.4 tags (through 0.24). When a v2.4 header sets the
//! unsynchronisation flag, lofty de-unsynchronises the whole tag stream,
//! then de-unsynchronises each frame again for the frame's own flag, so
//! every stuffed `ff 00` collapses one pass too far. The bytes shift, a
//! UTF-16 text frame lands on an odd length, and lofty aborts the entire
//! tag read: no title, no artist, nothing, and the tag editor shows the
//! parse error. The art module carves the picture out of the same shape
//! raw, but the text frames blow up before any of them are read, so the
//! whole file comes back blank.
//!
//! The tag is only ever unsynchronised once, at the frame level per the
//! v2.4 spec, so the header flag is redundant. Clearing it drops lofty to
//! a single frame-by-frame de-sync, which reads clean. This hands lofty a
//! copy of the file with that one bit cleared and leaves the original on
//! disk untouched; a commit through the writer already zeroes the header
//! flag on write, so editing such a file repairs it for good. Drop the
//! whole module once lofty reads these tags clean.

use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use crate::art::synchsafe;

/// The unsynchronisation flag in an ID3v2 header's flags byte.
const HEADER_UNSYNC: u8 = 0x80;
/// The extended-header flag in an ID3v2 header's flags byte.
const HEADER_EXTENDED: u8 = 0x40;
/// The unsynchronisation flag in an ID3v2.4 frame's format-flags byte.
const FRAME_UNSYNC: u8 = 0x02;

/// A source lofty parses: the file untouched, or an in-memory copy of it
/// with the ID3v2.4 header unsynchronisation flag cleared. Both read and
/// seek, so `Probe`, `MpegFile::read_from`, and `FlacFile::read_from` all
/// take it directly.
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

/// Open `path` for a tag read. When the file carries the v2.4 tag shape
/// lofty de-unsynchronises twice, this returns an in-memory copy with the
/// header flag cleared; otherwise it hands back the file untouched, so the
/// common path never reads more than the ten-byte header.
pub(crate) fn open(path: &Path) -> io::Result<TagSource> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 10];
    // A short read, a non-ID3 file, a version other than v2.4, or a tag
    // without the header flag all read fine through lofty as they are.
    if file.read_exact(&mut header).is_err()
        || &header[..3] != b"ID3"
        || header[3] != 4
        || header[5] & HEADER_UNSYNC == 0
    {
        file.rewind()?;
        return Ok(TagSource::File(file));
    }
    // The header flag alone (a tag unsynchronised as one stream) reads
    // fine; only a frame carrying its own flag triggers the second pass.
    // Reading the whole file is the price of that shape, paid only for it.
    file.rewind()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    if !frames_flagged(&buf) {
        return Ok(TagSource::Patched(Cursor::new(buf)));
    }
    buf[5] &= !HEADER_UNSYNC;
    Ok(TagSource::Patched(Cursor::new(buf)))
}

/// Whether `path` carries the ID3v2.4 double-unsync shape this module
/// works around: a v2.4 header with the unsynchronisation flag set and at
/// least one frame carrying its own unsync flag. That is the exact shape
/// [`open`] clears the header flag for and the writer repairs on commit, so
/// a repair pass uses it to find the files worth rewriting. Cheap on the
/// common file: the ten-byte header rules out anything that is not a v2.4
/// unsynchronised tag, and only a real candidate is read in full. Any read
/// or open error reads as "no repair needed", the same tolerance the scan
/// gives a file it cannot open.
pub fn needs_unsync_repair(path: &Path) -> bool {
    needs_unsync_repair_inner(path).unwrap_or(false)
}

fn needs_unsync_repair_inner(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 10];
    if file.read_exact(&mut header).is_err()
        || &header[..3] != b"ID3"
        || header[3] != 4
        || header[5] & HEADER_UNSYNC == 0
    {
        return Ok(false);
    }
    file.rewind()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(frames_flagged(&buf))
}

/// Whether any frame carries the per-frame unsync flag, the signal lofty
/// will de-unsynchronise it a second time. Walks the stored bytes: v2.4
/// frame sizes count the stuffing, so the walk stays aligned without a
/// de-sync pass, the same walk the art module's raw picture path runs.
fn frames_flagged(buf: &[u8]) -> bool {
    frames_flagged_inner(buf).unwrap_or(false)
}

fn frames_flagged_inner(buf: &[u8]) -> Option<bool> {
    let size = synchsafe(buf.get(6..10)?)? as usize;
    let tag = buf.get(10..10 + size)?;
    let mut pos = 0;
    // The extended header sits before the frames and counts itself in its
    // own size.
    if buf[5] & HEADER_EXTENDED != 0 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::config::ParsingMode;
    use lofty::file::AudioFile;
    use lofty::mpeg::MpegFile;
    use lofty::prelude::*;

    /// The unsynchronisation an encoder applies: a zero stuffed after every
    /// `ff` that precedes a zero or a sync-shaped byte.
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

    /// One UTF-16 text frame, unsynchronised with the frame flag set and a
    /// data length indicator, the Vide Noir shape.
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

    /// A few MPEG-1 Layer III frames (128kbps, 44100Hz), enough silent
    /// audio for lofty to recognise the file as MPEG and read its tag.
    fn mpeg_audio() -> Vec<u8> {
        let mut frame = vec![0xFF, 0xFB, 0x90, 0x64];
        frame.extend(std::iter::repeat_n(0u8, 413)); // 417-byte frame
        frame.repeat(4)
    }

    /// A file shaped like Vide Noir's: an ID3v2.4 tag with the header
    /// unsynchronisation flag set and every text frame flagged
    /// unsynchronised, followed by MPEG audio. The title text is chosen so
    /// the stuffing shifts it onto an odd length under lofty's double pass.
    fn vide_noir_file(title: &str) -> Vec<u8> {
        let mut frames = utf16_text_frame(b"TIT2", title);
        frames.extend(utf16_text_frame(b"TPE1", "Lord Huron"));
        let mut file = b"ID3\x04\x00\x80".to_vec();
        file.extend(synch(frames.len() as u32));
        file.extend(&frames);
        file.extend(mpeg_audio());
        file
    }

    /// The bare shape read straight through lofty aborts on the odd-length
    /// UTF-16 frame; the sanitiser clears the header flag so the same bytes
    /// parse and the title comes back intact.
    #[test]
    fn sanitiser_recovers_the_double_unsync_shape() {
        let opts = crate::parse_opts().parsing_mode(ParsingMode::Relaxed);
        let file = vide_noir_file("Back from the Edge");

        // Straight through lofty: the double de-sync mangles the frame, so
        // the title never survives.
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

    /// The repair gate: the double-unsync shape flags for repair, a file
    /// with no ID3 tag does not. The same gate `open` clears the header
    /// flag for, so the repair pass rewrites exactly the files it patches.
    #[test]
    fn needs_repair_flags_only_the_broken_shape() {
        let dir = std::env::temp_dir().join("rox-tag-source-needs-repair");
        std::fs::create_dir_all(&dir).unwrap();

        let broken = dir.join("broken.mp3");
        std::fs::write(&broken, vide_noir_file("Ends of the Earth")).unwrap();
        assert!(needs_unsync_repair(&broken), "the broken shape flags");

        let plain = dir.join("plain.mp3");
        std::fs::write(&plain, mpeg_audio()).unwrap();
        assert!(!needs_unsync_repair(&plain), "a plain file is left alone");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
