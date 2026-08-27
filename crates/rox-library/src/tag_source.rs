//! A tag read source that works around the ID3v2 shapes lofty (through
//! 0.24) reads mangled or not at all, handing lofty a corrected in-memory
//! copy while the original stays untouched on disk. A commit through the
//! writer rewrites the tag from the sanitised read, so editing such a
//! file repairs it for good. Drop the module once lofty handles these
//! shapes itself.
//!
//! The shapes:
//!
//! - The double de-unsynchronisation of ID3v2.4 tags. When a v2.4 header
//!   sets the unsynchronisation flag, lofty de-unsynchronises the whole
//!   tag stream, then de-unsynchronises each frame again for the frame's
//!   own flag, so every stuffed `ff 00` collapses one pass too far. The
//!   bytes shift, a UTF-16 text frame ends up an odd length, and lofty
//!   aborts the entire tag read. The tag is only ever unsynchronised
//!   once, at the frame level per the v2.4 spec, so the header flag is
//!   redundant; clearing it drops lofty to a single frame-by-frame
//!   de-sync, which reads clean.
//!
//! - The stray null some taggers leave on a UTF-16 text frame: the text,
//!   its two-byte terminator, then one extra null byte. lofty rejects the
//!   odd byte count outright and aborts the tag read, even in Relaxed
//!   mode, so one surplus byte blanks the whole file. Trimming the stray
//!   byte and knocking the frame size down one reads the frame as
//!   written.
//!
//! - Junk between the declared tag end and the first MPEG frame: padding
//!   a tagger left outside the tag size, or the headless remainder of a
//!   frame the tag was written over (the Bandcamp shape: the declared
//!   end falls nine bytes into the old LAME Info frame, leaving a
//!   sync-less carcass in front of the audio). Every tool re-finds the
//!   audio through it, and lofty's write path gives up after 1024 junk
//!   bytes, so such a file reads fine but refuses every tag write. Reads
//!   need no help here; [`needs_repair`] flags the shape and the writer
//!   folds the junk into the tag on commit, which drops it on the next
//!   rewrite.

use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use crate::art::{synchsafe, synchsafe_encode};

/// The unsynchronisation flag in an ID3v2 header's flags byte.
const HEADER_UNSYNC: u8 = 0x80;
/// The extended-header flag in an ID3v2 header's flags byte.
const HEADER_EXTENDED: u8 = 0x40;
/// The footer flag in an ID3v2.4 header's flags byte.
const HEADER_FOOTER: u8 = 0x10;
/// The unsynchronisation flag in an ID3v2.4 frame's format-flags byte.
const FRAME_UNSYNC: u8 = 0x02;
/// How much junk past the tag end lofty's write probe tolerates before
/// it fails to re-detect the format (its DEFAULT_MAX_JUNK_BYTES): a gap
/// past this reads fine but refuses every write, so it flags for repair.
const GAP_FLAG: u64 = 1024;
/// How far the gap scan reads before giving up: past this the "gap" is a
/// corrupt file, not a tagger's slack.
pub(crate) const GAP_SCAN_CAP: u64 = 1 << 24;

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

/// Open `path` for a tag read. When the file has a shape lofty reads
/// mangled, this returns an in-memory copy with the shape corrected;
/// otherwise it hands back the file untouched. The check reads the tag
/// region and no further, bytes lofty is about to read again anyway, so
/// the common path costs one extra pass over cached pages.
pub(crate) fn open(path: &Path) -> io::Result<TagSource> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 10];
    // A short read, a non-ID3 file, or a version outside v2.3/v2.4 reads
    // fine through lofty as it is.
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
        // The declared size runs past the file; nothing here can mend
        // that, so let lofty report it.
        file.rewind()?;
        return Ok(TagSource::File(file));
    }
    // The header flag alone (a tag unsynchronised as one stream) reads
    // fine; only a frame with its own flag set triggers the second pass.
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
    // The cursor is at the tag end; the rest of the file is copied over
    // unchanged.
    file.read_to_end(&mut buf)?;
    Ok(TagSource::Patched(Cursor::new(buf)))
}

/// Whether `path` has a tag shape a rewrite through the writer repairs:
/// the ID3v2.4 double-unsync shape, a UTF-16 text frame with a stray
/// trailing null, or more junk between the declared tag end and the first
/// MPEG sync than lofty's write probe tolerates. The first two are the
/// exact shapes [`open`] corrects in memory, the third the writer folds
/// into the tag on commit, so a repair pass uses this to find the files
/// worth rewriting. Cheap on the common file: the ten-byte header rules
/// out anything that isn't an ID3v2.3/4 tag, and only a candidate has its
/// tag region read. Any read or open error reads as "no repair needed",
/// the same tolerance the scan gives a file it cannot open.
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

/// The junk between `file`'s declared ID3v2 tag end and the first MPEG
/// sync, for the repair gate here and the writer's fold on commit: the
/// declared tag size, how many bytes to the sync, and whether one turned
/// up at all within [`GAP_SCAN_CAP`]. `None` when the file isn't
/// ID3v2.3/4 or the tag has a footer, which nothing can legally come
/// before. Seeks freely; callers position themselves after.
pub(crate) struct TagGap {
    pub size: u32,
    pub junk: u64,
    pub sync: bool,
}

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

/// Read forward from `file`'s cursor to the first MPEG sync pair (`ff`
/// then a byte with its top three bits set), up to `cap` bytes: how many
/// bytes come before the sync, and whether one turned up at all. The scan
/// the gap probe and the writer's audio span share, so the two always
/// agree on where audio starts.
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
                // The pair starts at the ff the previous pass counted.
                return Ok((skipped - 1, true));
            }
            prev = b;
            skipped += 1;
        }
    }
    Ok((skipped, false))
}

/// Whether any frame has the per-frame unsync flag set, the signal lofty
/// will de-unsynchronise it a second time. Steps through the stored bytes
/// of the tag region past the ten-byte header: v2.4 frame sizes count the
/// stuffing, so the walk stays aligned without a de-sync pass, the same
/// walk the art module's raw picture path runs.
fn frames_flagged(flags: u8, tag: &[u8]) -> bool {
    frames_flagged_inner(flags, tag).unwrap_or(false)
}

fn frames_flagged_inner(flags: u8, tag: &[u8]) -> Option<bool> {
    let mut pos = 0;
    // The extended header comes before the frames and counts itself in its
    // own size.
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

/// Trim the stray trailing null some taggers leave on a UTF-16 text
/// frame, the byte that leaves the frame an odd length and makes lofty
/// abort the whole tag read. Fires only on the exact shape: a plain text
/// frame (`T...`), no format flags, UTF-16 content of odd length whose
/// last byte is null. The frame shrinks by one and the slack the shifts
/// open becomes padding at the frames' end, so the tag's declared size
/// and everything past it stay put. Returns whether anything changed.
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
        // Odd text length means an even frame size: the encoding byte
        // comes in front of the UTF-16 run.
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
        // The bytes the shifts left behind, now padding before whatever
        // padding the tag already carried.
        tag[write..read].fill(0);
    }
    changed
}

/// A stored frame size: synchsafe in v2.4, a plain big-endian word in
/// v2.3.
fn frame_size(version: u8, bytes: &[u8]) -> Option<u32> {
    match version {
        4 => synchsafe(bytes),
        _ => Some(u32::from_be_bytes(bytes.try_into().ok()?)),
    }
}

/// The write-side counterpart of [`frame_size`].
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

    /// A 4-byte synchsafe encode, the write-side counterpart of `synchsafe`.
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
        // the title never comes through.
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
        assert!(needs_repair(&broken), "the broken shape flags");

        let plain = dir.join("plain.mp3");
        std::fs::write(&plain, mpeg_audio()).unwrap();
        assert!(!needs_repair(&plain), "a plain file is left alone");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// One UTF-16 text frame in v2.3 dress, its size a plain big-endian
    /// word, with a stray null after the terminator when asked: the
    /// Paris Texas shape, one surplus byte lofty aborts the whole tag
    /// read over.
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

    /// An ID3v2.3 file: a title (optionally with the stray null), an
    /// artist, then MPEG audio, with `gap` zero bytes wedged between the
    /// tag end and the first frame.
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

    /// The stray-null shape read straight through lofty aborts on the
    /// odd-length UTF-16 frame; the sanitiser trims the surplus byte so
    /// the same tag parses whole, later frames included.
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

    /// A healthy v2.3 file passes through untouched: no repair flag, and
    /// `open` hands back the file rather than a patched copy.
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

    /// The gap gate: zero padding past the tag end flags once it is
    /// deeper than lofty's write probe tolerates, and stays quiet under
    /// that line, where writes still work as the file lies.
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
