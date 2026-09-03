//! The MP4 box walk rox does for itself, where lofty leaves it nothing to
//! read.
//!
//! Three questions come off the same walk. The first is how long a
//! fragmented file runs; the other two belong to the tag writer, and both
//! are about the file's structure rather than its metadata. [`mdat_spans`]
//! says where the audio actually sits, so a write that moves the audio can
//! still be checked against it, and [`is_fragmented`] answers the question
//! `writer::file_type` turns a file down on.
//!
//! A fragmented MP4, the shape anything assembled out of DASH segments
//! comes down in, leaves the `moov` sample tables empty. No `stts`
//! entries, `mvhd` and `mdhd` durations both zero, and the samples
//! themselves out in `moof`/`mdat` pairs past the header. Such a file
//! states its length in the `mehd` box's `fragment_duration`, or in a
//! `sidx` segment index, and lofty reads neither: `properties().duration()`
//! comes back 0ms. symphonia (through 0.6) reads the `sidx` but falls back
//! to `mdhd` when a file has none, which is the zero again.
//!
//! So both the scan and the playback open come away not knowing how long
//! the track is, which costs the seek bar its range and prints the
//! remaining time as -0:00. This reads the `mehd` directly: a walk of box
//! headers off the front of the file, a handful of seeks, no decode. A
//! fragmented file with neither `mehd` nor `sidx` still can't be
//! measured short of decoding it.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::{ControlFlow, Range};
use std::path::Path;

/// The playable length of a fragmented MP4, in seconds. None where the file
/// isn't an MP4, isn't fragmented, or is fragmented without ever saying how
/// long its fragments run.
pub fn fragment_duration_secs(path: &Path) -> Option<f64> {
    let mut file = File::open(path).ok()?;
    let end = file.seek(SeekFrom::End(0)).ok()?;

    let moov = find(&mut file, 0..end, b"moov")?;
    // `mehd` counts in movie ticks, and `mvhd` is the only box that says
    // how many of those go in a second.
    let mvhd = find(&mut file, moov.clone(), b"mvhd")?;
    let timescale = mvhd_timescale(&mut file, mvhd)?;
    // No `mvex` means no fragments, so nothing here applies: a plain MP4
    // that reports no duration is broken in some other way.
    let mvex = find(&mut file, moov, b"mvex")?;
    let mehd = find(&mut file, mvex, b"mehd")?;
    let duration = mehd_duration(&mut file, mehd)?;

    (duration > 0 && timescale > 0).then(|| duration as f64 / f64::from(timescale))
}

/// Every top-level `mdat` payload, in file order: where an MP4 keeps its
/// audio. None where the file isn't an MP4, holds no `mdat` at all, or
/// stops parsing partway.
///
/// One range would do for a plain file, which has a single `mdat` with the
/// whole stream in it. A fragmented file has one per fragment with a
/// `moof` between each pair, and hashing only the first would make the
/// writer's verify step a rubber stamp over most of the audio.
pub(crate) fn mdat_spans(path: &Path) -> Option<Vec<Range<u64>>> {
    let mut file = File::open(path).ok()?;
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let mut spans = Vec::new();
    walk(&mut file, 0..end, |kind, body| {
        if kind == b"mdat" {
            spans.push(body);
        }
        ControlFlow::Continue(())
    })?;
    (!spans.is_empty()).then_some(spans)
}

/// Whether the file's samples live out in fragments rather than in the
/// `moov` sample tables, the same `mvex` test
/// [`fragment_duration_secs`] makes. A file this says yes to is one the
/// tag writer turns down; a file it says no to (including anything that
/// isn't an MP4 at all) is left to the caller's own checks.
pub(crate) fn is_fragmented(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(end) = file.seek(SeekFrom::End(0)) else {
        return false;
    };
    find(&mut file, 0..end, b"moov")
        .and_then(|moov| find(&mut file, moov, b"mvex"))
        .is_some()
}

/// The payload range of the first box of type `want` sitting directly
/// inside `within`.
fn find(file: &mut File, within: Range<u64>, want: &[u8; 4]) -> Option<Range<u64>> {
    let mut found = None;
    let _ = walk(file, within, |kind, body| {
        if kind != want {
            return ControlFlow::Continue(());
        }
        found = Some(body);
        ControlFlow::Break(())
    });
    found
}

/// Hand every box sitting directly inside `within` to `visit`, as its
/// four-byte type and its payload range. Boxes are walked by their stated
/// size, so this seeks header to header rather than reading the range
/// through.
///
/// None where a box doesn't cover its own header or runs past the parent
/// holding it: a file to stop trusting rather than one to keep looping
/// over. A `visit` that breaks stops the walk where it stands and comes
/// back Some, so a caller that already has what it came for never fails
/// over bytes further down the file it was never going to read.
fn walk(
    file: &mut File,
    within: Range<u64>,
    mut visit: impl FnMut(&[u8; 4], Range<u64>) -> ControlFlow<()>,
) -> Option<()> {
    let mut at = within.start;
    while at + 8 <= within.end {
        file.seek(SeekFrom::Start(at)).ok()?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;

        let stated = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let (size, body) = match stated {
            // A 1 puts the real size in the eight bytes behind the header.
            1 => {
                let mut large = [0u8; 8];
                file.read_exact(&mut large).ok()?;
                (u64::from_be_bytes(large), at + 16)
            }
            // A 0 is the last box in its parent, running to the parent's end.
            0 => (within.end - at, at + 8),
            size => (u64::from(size), at + 8),
        };

        // A box that doesn't cover its own header, or that runs past the
        // parent holding it, is a file to stop trusting rather than one to
        // keep looping over.
        let box_end = at.checked_add(size)?;
        if body > box_end || box_end > within.end {
            return None;
        }
        let kind: [u8; 4] = header[4..].try_into().ok()?;
        if visit(&kind, body..box_end).is_break() {
            return Some(());
        }
        at = box_end;
    }
    Some(())
}

/// The movie timescale, in ticks per second. Version 1 widens the creation
/// and modification times either side of it to 64 bits, which moves it.
fn mvhd_timescale(file: &mut File, at: Range<u64>) -> Option<u32> {
    let buf = head(file, at, 24)?;
    let off = match *buf.first()? {
        0 => 12,
        1 => 20,
        _ => return None,
    };
    buf.get(off..off + 4)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_be_bytes)
}

/// How long the fragments run, on the movie clock. Version 1 widens the
/// field itself to 64 bits.
fn mehd_duration(file: &mut File, at: Range<u64>) -> Option<u64> {
    let buf = head(file, at, 12)?;
    match *buf.first()? {
        0 => buf
            .get(4..8)
            .and_then(|b| b.try_into().ok())
            .map(|b| u64::from(u32::from_be_bytes(b))),
        1 => buf
            .get(4..12)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_be_bytes),
        _ => None,
    }
}

/// The first `len` bytes of a box's payload, or all of it where it's
/// shorter than that.
fn head(file: &mut File, at: Range<u64>, len: usize) -> Option<Vec<u8>> {
    let len = len.min((at.end - at.start).try_into().ok()?);
    file.seek(SeekFrom::Start(at.start)).ok()?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// One box: its total size, its four-byte type, then the payload.
    fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    /// The same box written with a 64-bit size, the shape a large `mdat`
    /// takes and the walk has to step over to reach anything behind it.
    fn large_atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = 1u32.to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(&((payload.len() + 16) as u64).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// A version 0 `mvhd` with only its timescale filled in, the way a
    /// fragmented file writes one: every duration in it stays zero.
    fn mvhd(timescale: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 100];
        payload[12..16].copy_from_slice(&timescale.to_be_bytes());
        atom(b"mvhd", &payload)
    }

    /// A version 0 `mehd`: version and flags, then a 32-bit duration.
    fn mehd(duration: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 8];
        payload[4..8].copy_from_slice(&duration.to_be_bytes());
        atom(b"mehd", &payload)
    }

    /// A version 1 `mehd`, the one a long file needs.
    fn mehd64(duration: u64) -> Vec<u8> {
        let mut payload = vec![1u8, 0, 0, 0];
        payload.extend_from_slice(&duration.to_be_bytes());
        atom(b"mehd", &payload)
    }

    /// A whole file: `ftyp`, a `moov` holding the boxes given, then a
    /// fragment, where the samples of a real one are stored.
    fn file(moov_children: &[Vec<u8>]) -> Vec<u8> {
        let mut moov = Vec::new();
        for child in moov_children {
            moov.extend_from_slice(child);
        }
        let mut out = atom(b"ftyp", b"isom\0\0\0\0iso5");
        out.extend(atom(b"moov", &moov));
        out.extend(atom(b"moof", &[0u8; 16]));
        out.extend(atom(b"mdat", &[0u8; 64]));
        out
    }

    fn written(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// The common shape: sample tables empty, the length only in `mehd`.
    #[test]
    fn reads_the_fragment_duration() {
        let mvex = atom(b"mvex", &mehd(5_722_380));
        let path = written("fragmented.m4a", &file(&[mvhd(44_100), mvex]));
        let secs = fragment_duration_secs(&path).expect("the mehd states the length");
        assert!((secs - 129.759_183).abs() < 1e-5, "{secs}");
    }

    /// A 64-bit `mehd`, and a 64-bit box in front of the `moov` to step
    /// over on the way to it.
    #[test]
    fn reads_a_64_bit_duration_past_a_64_bit_box() {
        let mvex = atom(b"mvex", &mehd64(88_200));
        let mut bytes = large_atom(b"free", &[0u8; 32]);
        bytes.extend(file(&[mvhd(44_100), mvex]));
        let path = written("large.m4a", &bytes);
        assert_eq!(fragment_duration_secs(&path), Some(2.0));
    }

    /// A plain MP4 has no `mvex` at all, so there's nothing here to say
    /// about it. Its length comes off the sample tables like always.
    #[test]
    fn plain_mp4_reads_nothing() {
        let path = written("plain.m4a", &file(&[mvhd(44_100)]));
        assert_eq!(fragment_duration_secs(&path), None);
    }

    /// A fragmented file whose `mehd` is zero knows no more than the
    /// sample tables did, and saying "zero seconds" would be worse than
    /// saying nothing.
    #[test]
    fn a_zero_duration_is_no_answer() {
        let mvex = atom(b"mvex", &mehd(0));
        let path = written("zero.m4a", &file(&[mvhd(44_100), mvex]));
        assert_eq!(fragment_duration_secs(&path), None);
    }

    /// Fragmented, but the `mvex` holds only the `trex` defaults with no
    /// `mehd` beside them. Nothing to read, and nothing to invent.
    #[test]
    fn no_mehd_reads_nothing() {
        let mvex = atom(b"mvex", &atom(b"trex", &[0u8; 24]));
        let path = written("no-mehd.m4a", &file(&[mvhd(44_100), mvex]));
        assert_eq!(fragment_duration_secs(&path), None);
    }

    /// Not an MP4, and a box walk over arbitrary bytes has to end rather
    /// than run the file twice looking for a `moov`.
    #[test]
    fn junk_reads_nothing() {
        let path = written("junk.m4a", &[0xFFu8; 4096]);
        assert_eq!(fragment_duration_secs(&path), None);
    }

    /// The writer's question about where the audio is. [`file`] writes one
    /// fragment, so a second one has to show up as a second span rather
    /// than being lost behind the first: a single range would describe
    /// only a quarter of this file, and the hash taken over it would pass
    /// no matter what happened to the rest.
    #[test]
    fn every_mdat_is_a_span_of_its_own() {
        let mut bytes = file(&[mvhd(44_100)]);
        let second = bytes.len() as u64;
        bytes.extend(atom(b"moof", &[0u8; 16]));
        bytes.extend(atom(b"mdat", &[1u8; 32]));
        let path = written("spans.m4a", &bytes);

        let spans = mdat_spans(&path).expect("both fragments");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1], (second + 24 + 8)..(second + 24 + 8 + 32));
        assert_eq!(spans[0].end - spans[0].start, 64);
    }

    /// A file with no audio box at all reads as nothing rather than an
    /// empty list, so the writer can tell it apart from a file it hashed.
    #[test]
    fn no_mdat_is_no_span() {
        let path = written("tagless.m4a", &atom(b"ftyp", b"isom\0\0\0\0iso5"));
        assert_eq!(mdat_spans(&path), None);
    }

    /// The writer's other question. `mvex` is the whole test, the same one
    /// the duration read makes, so the two can't disagree about what
    /// fragmented means.
    #[test]
    fn fragmentation_is_the_mvex_test() {
        let mvex = atom(b"mvex", &mehd(5_722_380));
        let fragmented = written("frag-check.m4a", &file(&[mvhd(44_100), mvex]));
        let plain = written("plain-check.m4a", &file(&[mvhd(44_100)]));
        assert!(is_fragmented(&fragmented));
        assert!(!is_fragmented(&plain));
        assert!(!is_fragmented(&written("junk-check.m4a", &[0xFFu8; 4096])));
    }
}
