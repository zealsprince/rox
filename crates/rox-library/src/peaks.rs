//! The waveform peak cache: one small binary file per track under a cache
//! directory the caller names, so a track's strip comes back instantly
//! after its first play instead of re-decoding the whole file.
//! Entries are keyed by file identity: the source's path, size, and mtime
//! are stored inside and a mismatch on any of them reads as a miss, so an
//! edited or replaced file re-decodes and overwrites its entry. The
//! identity is the one the file had going into the decode, checked again
//! at the write, so a track still being written to disk never keys a short
//! waveform to the file it finishes as. Anything unreadable or malformed
//! is a miss too, never an error; the panel just decodes fresh and stores
//! again.
//!
//! Entry layout, little-endian throughout: the magic, source size (u64),
//! source mtime in unix seconds (u64), path length (u32) and the path's
//! bytes, lane count (u32), then per lane a bin count (u32) followed by
//! that many (min, max, rms) f32 triples. Lane 0 is the mono mix, further
//! lanes are per-channel.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::hash::fnv1a;

/// One bin of a peak lane: the sample extremes over the bin's frames, and
/// the RMS level across them. The extremes draw the outer envelope, the
/// RMS the flatter loudness band inside it. All three run through the
/// same normalization, so the band never leaves the envelope.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PeakBin {
    pub lo: f32,
    pub hi: f32,
    pub rms: f32,
}

/// A track's peak lanes: the mono mix at 0, then a left and a right lane
/// when the source has more than one channel.
pub type PeakLanes = Vec<Vec<PeakBin>>;

/// Identifies the layout; bump it when the format changes and old entries
/// read as misses and get rewritten.
const MAGIC: &[u8; 8] = b"roxwave3";

/// Drop every entry; strips re-decode and re-store on their next play.
/// Blocking on the directory removal; run off the UI thread.
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Where a track's entry is stored: a hash of the path names the file, the
/// path stored inside the entry disambiguates a collision.
fn entry_path(dir: &Path, track: &Path) -> PathBuf {
    dir.join(format!(
        "{:016x}.peaks",
        fnv1a(track.as_os_str().as_encoded_bytes())
    ))
}

/// The source file's identity as entries store it: size and mtime in unix
/// seconds. None means the file itself is unreadable, so cache nothing.
/// Callers stamp the file with this before they decode it and hand the
/// stamp back to [`store`].
pub fn identity(track: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(track).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((meta.len(), mtime))
}

fn take<'a>(data: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if data.len() < n {
        return None;
    }
    let (head, tail) = data.split_at(n);
    *data = tail;
    Some(head)
}

fn take_u32(data: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(take(data, 4)?.try_into().ok()?))
}

fn take_u64(data: &mut &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(take(data, 8)?.try_into().ok()?))
}

/// The cached peak lanes for a track, or None on any kind of miss: no
/// entry, a stale one (the file changed since it was written), an old
/// format, or a filename collision with another track.
pub fn load(dir: &Path, track: &Path) -> Option<PeakLanes> {
    let (size, mtime) = identity(track)?;
    let data = std::fs::read(entry_path(dir, track)).ok()?;
    let mut rest = data.as_slice();
    if take(&mut rest, MAGIC.len())? != MAGIC {
        return None;
    }
    if take_u64(&mut rest)? != size || take_u64(&mut rest)? != mtime {
        return None;
    }
    let path_len = take_u32(&mut rest)? as usize;
    if take(&mut rest, path_len)? != track.as_os_str().as_encoded_bytes() {
        return None;
    }
    let lane_count = take_u32(&mut rest)? as usize;
    // The capacity clamp keeps a garbage lane count from reserving the
    // moon; the takes below run out of data long before the loop does.
    let mut lanes = Vec::with_capacity(lane_count.min(8));
    for _ in 0..lane_count {
        let count = take_u32(&mut rest)? as usize;
        let bins = take(&mut rest, count.checked_mul(12)?)?;
        lanes.push(
            bins.as_chunks::<12>()
                .0
                .iter()
                .map(|bin| PeakBin {
                    lo: f32::from_le_bytes(bin[0..4].try_into().unwrap()),
                    hi: f32::from_le_bytes(bin[4..8].try_into().unwrap()),
                    rms: f32::from_le_bytes(bin[8..12].try_into().unwrap()),
                })
                .collect(),
        );
    }
    Some(lanes)
}

/// Write a track's entry against `stamped`, the identity the file had
/// before the decode read it. A file that moved in between (a download
/// finishing under the decode) writes nothing, so the finished file
/// decodes fresh instead of showing a waveform of the half that existed.
/// Failures log and move on, same stance as the settings file: a lost
/// cache entry only costs a re-decode next time.
pub fn store(dir: &Path, track: &Path, stamped: Option<(u64, u64)>, lanes: &[Vec<PeakBin>]) {
    let Some((size, mtime)) = stamped.filter(|id| identity(track) == Some(*id)) else {
        return;
    };
    let _ = std::fs::create_dir_all(dir);
    let path_bytes = track.as_os_str().as_encoded_bytes();
    let bins: usize = lanes.iter().map(Vec::len).sum();
    let mut data = Vec::with_capacity(32 + path_bytes.len() + lanes.len() * 4 + bins * 12);
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&size.to_le_bytes());
    data.extend_from_slice(&mtime.to_le_bytes());
    data.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
    data.extend_from_slice(path_bytes);
    data.extend_from_slice(&(lanes.len() as u32).to_le_bytes());
    for lane in lanes {
        data.extend_from_slice(&(lane.len() as u32).to_le_bytes());
        for bin in lane {
            data.extend_from_slice(&bin.lo.to_le_bytes());
            data.extend_from_slice(&bin.hi.to_le_bytes());
            data.extend_from_slice(&bin.rms.to_le_bytes());
        }
    }
    let path = entry_path(dir, track);
    if let Err(e) = std::fs::write(&path, data) {
        log::warn!("peaks cache: writing {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch dir holding the fake track and the cache, cleaned on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("rox-peaks-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn track(&self, contents: &str) -> PathBuf {
            let track = self.0.join("track.flac");
            std::fs::write(&track, contents).unwrap();
            track
        }

        fn cache(&self) -> PathBuf {
            self.0.join("waveforms")
        }

        /// Store the way the panel does: stamp the track, then write.
        fn store(&self, track: &Path, lanes: &[Vec<PeakBin>]) {
            store(&self.cache(), track, identity(track), lanes);
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A bin from its parts, keeping the fixtures readable.
    fn bin(lo: f32, hi: f32, rms: f32) -> PeakBin {
        PeakBin { lo, hi, rms }
    }

    /// The one-bin lane the negative tests plant, where the values don't
    /// matter.
    fn one_bin() -> Vec<Vec<PeakBin>> {
        vec![vec![bin(-1.0, 1.0, 0.7)]]
    }

    #[test]
    fn round_trip() {
        let scratch = Scratch::new("round-trip");
        let track = scratch.track("pcm");
        // Three lanes the way the decoder hands them over: the mono mix,
        // then left and right.
        let lanes = vec![
            vec![
                bin(-0.5, 0.5, 0.35),
                bin(-1.0, 1.0, 0.7),
                bin(0.0, 0.25, 0.1),
            ],
            vec![
                bin(-0.25, 0.75, 0.4),
                bin(-1.0, 0.5, 0.6),
                bin(0.0, 0.5, 0.2),
            ],
            vec![
                bin(-0.75, 0.25, 0.3),
                bin(-0.5, 1.0, 0.65),
                bin(0.0, 0.125, 0.05),
            ],
        ];
        scratch.store(&track, &lanes);
        assert_eq!(load(&scratch.cache(), &track), Some(lanes));
    }

    #[test]
    fn changed_file_misses() {
        let scratch = Scratch::new("changed");
        let track = scratch.track("pcm");
        scratch.store(&track, &one_bin());
        // Same path, different size: the identity check has to fail.
        std::fs::write(&track, "different contents").unwrap();
        assert_eq!(load(&scratch.cache(), &track), None);
    }

    /// A file that grows while the decode reads it (a download finishing
    /// under the strip) writes no entry at all. Storing against the
    /// identity it finished with would pin a waveform of the half that
    /// existed to the whole file, and it would never read as stale.
    #[test]
    fn file_that_moves_under_the_decode_writes_nothing() {
        let scratch = Scratch::new("moved");
        let track = scratch.track("half the pcm");
        let stamp = identity(&track);
        std::fs::write(&track, "half the pcm and then the rest").unwrap();
        store(&scratch.cache(), &track, stamp, &one_bin());
        assert!(!entry_path(&scratch.cache(), &track).exists());
        assert_eq!(load(&scratch.cache(), &track), None);
    }

    #[test]
    fn garbage_entry_misses() {
        let scratch = Scratch::new("garbage");
        let track = scratch.track("pcm");
        let cache = scratch.cache();
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(entry_path(&cache, &track), b"not a peaks file").unwrap();
        assert_eq!(load(&cache, &track), None);
    }

    #[test]
    fn missing_entry_misses() {
        let scratch = Scratch::new("missing");
        let track = scratch.track("pcm");
        assert_eq!(load(&scratch.cache(), &track), None);
    }

    /// An empty lane set stores and loads as empty, not a miss: the strip
    /// for a zero-length file is legitimately empty and should cache like
    /// any other.
    #[test]
    fn empty_peaks_round_trip() {
        let scratch = Scratch::new("empty");
        let track = scratch.track("pcm");
        scratch.store(&track, &[]);
        assert_eq!(load(&scratch.cache(), &track), Some(Vec::new()));
    }

    /// An entry from before the loudness band (pairs, not triples) reads
    /// as a miss off its magic, so the track re-decodes and rewrites
    /// instead of erroring.
    #[test]
    fn old_format_misses() {
        let scratch = Scratch::new("old-format");
        let track = scratch.track("pcm");
        let cache = scratch.cache();
        scratch.store(&track, &one_bin());
        let entry = entry_path(&cache, &track);
        let mut data = std::fs::read(&entry).unwrap();
        data[..8].copy_from_slice(b"roxwave2");
        std::fs::write(&entry, data).unwrap();
        assert_eq!(load(&cache, &track), None);
    }

    /// The stored path disambiguates a hash collision: an entry written for
    /// one track but planted where another's entry would go reads as a miss,
    /// so two files that hash alike never hand each other the wrong waveform.
    /// The entry holds a's path bytes, which b's load compares against and
    /// rejects.
    #[test]
    fn planted_entry_with_wrong_path_misses() {
        let scratch = Scratch::new("collision");
        let cache = scratch.cache();
        std::fs::create_dir_all(&cache).unwrap();
        let a = scratch.0.join("a.flac");
        let b = scratch.0.join("b.flac");
        // Same bytes, so the size check can't be what rejects b; the load has
        // to fall through to the stored-path comparison.
        std::fs::write(&a, "same-bytes").unwrap();
        std::fs::write(&b, "same-bytes").unwrap();

        // Write a's entry, then drop it at b's entry path to fake the clash.
        scratch.store(&a, &one_bin());
        std::fs::copy(entry_path(&cache, &a), entry_path(&cache, &b)).unwrap();
        // The entry stores a's path, not b's, so b reads a miss.
        assert_eq!(load(&cache, &b), None);
        // And a itself still loads from its own entry.
        assert_eq!(load(&cache, &a), Some(one_bin()));
    }
}
