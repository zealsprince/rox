//! The MP4 box walk rox does for itself, where lofty leaves nothing to read:
//! a fragmented file's length, and the structure questions the tag writer
//! asks ([`stream_spans`], [`has_absolute_fragment_offsets`]).
//!
//! A fragmented MP4 (anything assembled from DASH segments) leaves the
//! `moov` sample tables empty and states its length in `mehd` or `sidx`.
//! lofty reads neither and reports 0ms; symphonia (through 0.6) reads `sidx`
//! but falls back to the zero `mdhd` without one. This reads `mehd` directly.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::{ControlFlow, Range};
use std::path::Path;

/// None where the file isn't a fragmented MP4 or never says how long its
/// fragments run.
pub fn fragment_duration_secs(path: &Path) -> Option<f64> {
    let mut file = File::open(path).ok()?;
    let end = file.seek(SeekFrom::End(0)).ok()?;

    let moov = find(&mut file, 0..end, b"moov")?;
    // `mehd` counts in movie ticks; only `mvhd` says how many go in a second.
    let mvhd = find(&mut file, moov.clone(), b"mvhd")?;
    let timescale = mvhd_timescale(&mut file, mvhd)?;
    let mvex = find(&mut file, moov, b"mvex")?;
    let mehd = find(&mut file, mvex, b"mehd")?;
    let duration = mehd_duration(&mut file, mehd)?;

    (duration > 0 && timescale > 0).then(|| duration as f64 / f64::from(timescale))
}

/// Every top-level `mdat` and `moof` payload, in file order. Hashing only
/// the first `mdat` would rubber-stamp most of a fragmented file. The
/// `moof`s are included because a write may shift one but never patch it.
pub(crate) fn stream_spans(path: &Path) -> Option<Vec<Range<u64>>> {
    let mut file = File::open(path).ok()?;
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let mut spans = Vec::new();
    let mut audio = false;
    walk(&mut file, 0..end, |kind, body| {
        if kind == b"mdat" || kind == b"moof" {
            audio |= kind == b"mdat";
            spans.push(body);
        }
        ControlFlow::Continue(())
    })?;
    audio.then_some(spans)
}

/// Whether any fragment locates its samples by absolute file position (a
/// `tfhd` base-data-offset flag, or a `sidx`), which a resized `moov` would
/// leave stale. The tag writer refuses such files. A walk that stops partway
/// says no; the writer's own checks refuse those.
pub(crate) fn has_absolute_fragment_offsets(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(end) = file.seek(SeekFrom::End(0)) else {
        return false;
    };
    // Collect first, inspect after: a seek inside the visit would lose the
    // walk's place.
    let mut sidx = false;
    let mut moofs = Vec::new();
    let _ = walk(&mut file, 0..end, |kind, body| {
        match kind {
            b"sidx" => sidx = true,
            b"moof" => moofs.push(body),
            _ => {}
        }
        if sidx {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    sidx || moofs
        .into_iter()
        .any(|moof| moof_has_base_offset(&mut file, moof))
}

/// The base-data-offset flag is the low bit of the flag bytes.
fn moof_has_base_offset(file: &mut File, moof: Range<u64>) -> bool {
    let mut trafs = Vec::new();
    let _ = walk(file, moof, |kind, body| {
        if kind == b"traf" {
            trafs.push(body);
        }
        ControlFlow::Continue(())
    });
    for traf in trafs {
        let Some(tfhd) = find(file, traf, b"tfhd") else {
            continue;
        };
        let Some(header) = head(file, tfhd, 4) else {
            continue;
        };
        if header.get(3).is_some_and(|flags| flags & 1 != 0) {
            return true;
        }
    }
    false
}

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

/// Hand every box directly inside `within` to `visit` as (type, payload).
/// None for a box that doesn't fit its parent. A `visit` that breaks returns
/// Some, so an early answer never fails on bytes further down.
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
            1 => {
                let mut large = [0u8; 8];
                file.read_exact(&mut large).ok()?;
                (u64::from_be_bytes(large), at + 16)
            }
            // A 0 size runs to the parent's end.
            0 => (within.end - at, at + 8),
            size => (u64::from(size), at + 8),
        };

        // Box sizes come from the file and aren't trusted. One that doesn't
        // cover its header or overruns its parent ends the walk, so junk
        // bytes can't loop it or read past the parent.
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

/// Version 1 widens the times either side of it, which moves it.
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

    fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn large_atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = 1u32.to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(&((payload.len() + 16) as u64).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn mvhd(timescale: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 100];
        payload[12..16].copy_from_slice(&timescale.to_be_bytes());
        atom(b"mvhd", &payload)
    }

    fn mehd(duration: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 8];
        payload[4..8].copy_from_slice(&duration.to_be_bytes());
        atom(b"mehd", &payload)
    }

    /// Flag 1 is a base data offset, followed by eight bytes of position.
    fn moof(tfhd_flags: u32) -> Vec<u8> {
        let mut tfhd = tfhd_flags.to_be_bytes().to_vec();
        tfhd.extend_from_slice(&1u32.to_be_bytes());
        if tfhd_flags & 1 != 0 {
            tfhd.extend_from_slice(&[0u8; 8]);
        }
        atom(b"moof", &atom(b"traf", &atom(b"tfhd", &tfhd)))
    }

    fn mehd64(duration: u64) -> Vec<u8> {
        let mut payload = vec![1u8, 0, 0, 0];
        payload.extend_from_slice(&duration.to_be_bytes());
        atom(b"mehd", &payload)
    }

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

    #[test]
    fn reads_the_fragment_duration() {
        let mvex = atom(b"mvex", &mehd(5_722_380));
        let path = written("fragmented.m4a", &file(&[mvhd(44_100), mvex]));
        let secs = fragment_duration_secs(&path).expect("the mehd states the length");
        assert!((secs - 129.759_183).abs() < 1e-5, "{secs}");
    }

    #[test]
    fn reads_a_64_bit_duration_past_a_64_bit_box() {
        let mvex = atom(b"mvex", &mehd64(88_200));
        let mut bytes = large_atom(b"free", &[0u8; 32]);
        bytes.extend(file(&[mvhd(44_100), mvex]));
        let path = written("large.m4a", &bytes);
        assert_eq!(fragment_duration_secs(&path), Some(2.0));
    }

    #[test]
    fn plain_mp4_reads_nothing() {
        let path = written("plain.m4a", &file(&[mvhd(44_100)]));
        assert_eq!(fragment_duration_secs(&path), None);
    }

    #[test]
    fn a_zero_duration_is_no_answer() {
        let mvex = atom(b"mvex", &mehd(0));
        let path = written("zero.m4a", &file(&[mvhd(44_100), mvex]));
        assert_eq!(fragment_duration_secs(&path), None);
    }

    #[test]
    fn no_mehd_reads_nothing() {
        let mvex = atom(b"mvex", &atom(b"trex", &[0u8; 24]));
        let path = written("no-mehd.m4a", &file(&[mvhd(44_100), mvex]));
        assert_eq!(fragment_duration_secs(&path), None);
    }

    #[test]
    fn junk_reads_nothing() {
        let path = written("junk.m4a", &[0xFFu8; 4096]);
        assert_eq!(fragment_duration_secs(&path), None);
    }

    #[test]
    fn every_fragment_is_a_span_of_its_own() {
        let mut bytes = file(&[mvhd(44_100)]);
        let second = bytes.len() as u64;
        bytes.extend(atom(b"moof", &[0u8; 16]));
        bytes.extend(atom(b"mdat", &[1u8; 32]));
        let path = written("spans.m4a", &bytes);

        let spans = stream_spans(&path).expect("both fragments");
        assert_eq!(spans.len(), 4);
        assert_eq!(spans[1].end - spans[1].start, 64);
        assert_eq!(spans[2], (second + 8)..(second + 8 + 16));
        assert_eq!(spans[3], (second + 24 + 8)..(second + 24 + 8 + 32));
    }

    #[test]
    fn no_mdat_is_no_span() {
        let path = written("tagless.m4a", &atom(b"ftyp", b"isom\0\0\0\0iso5"));
        assert_eq!(stream_spans(&path), None);
        let mut headless = atom(b"ftyp", b"isom\0\0\0\0iso5");
        headless.extend(moof(0x02_0000));
        assert_eq!(stream_spans(&written("headless.m4a", &headless)), None);
    }

    #[test]
    fn absolute_offsets_are_the_tfhd_flag_or_a_sidx() {
        let mvex = atom(b"mvex", &mehd(5_722_380));
        let mut relative = file(&[mvhd(44_100), mvex.clone()]);
        relative.extend(moof(0x02_0000));
        relative.extend(atom(b"mdat", &[1u8; 32]));
        assert!(!has_absolute_fragment_offsets(&written(
            "relative.m4a",
            &relative
        )));

        let mut later = relative.clone();
        later.extend(moof(0x02_0001));
        later.extend(atom(b"mdat", &[1u8; 32]));
        assert!(has_absolute_fragment_offsets(&written("later.m4a", &later)));

        let mut indexed = file(&[mvhd(44_100), mvex]);
        indexed.extend(atom(b"sidx", &[0u8; 32]));
        assert!(has_absolute_fragment_offsets(&written(
            "indexed.m4a",
            &indexed
        )));

        assert!(!has_absolute_fragment_offsets(&written(
            "plain.m4a",
            &file(&[mvhd(44_100)])
        )));
        assert!(!has_absolute_fragment_offsets(&written(
            "junk-check.m4a",
            &[0xFFu8; 4096]
        )));
    }
}
