//! The waveform peak cache: one binary file per track, keyed on the source's
//! path, size, and mtime so a changed file misses. Anything unreadable is a
//! miss, never an error.
//!
//! Layout, little-endian: magic, size (u64), mtime (u64), path length (u32)
//! and bytes, lane count (u32), then per lane a bin count (u32) and that
//! many (min, max, rms) f32 triples. Lane 0 is the mono mix.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::hash::fnv1a;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PeakBin {
    pub lo: f32,
    pub hi: f32,
    pub rms: f32,
}

/// The mono mix at 0, then left and right for a multi-channel source.
pub type PeakLanes = Vec<Vec<PeakBin>>;

/// Bump when the layout changes; old entries then read as misses.
const MAGIC: &[u8; 8] = b"roxwave3";

/// Blocking; run off the UI thread.
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// The stored path disambiguates a hash collision.
fn entry_path(dir: &Path, track: &Path) -> PathBuf {
    dir.join(format!(
        "{:016x}.peaks",
        fnv1a(track.as_os_str().as_encoded_bytes())
    ))
}

/// Callers stamp the file with this before decoding and hand it to
/// [`store`].
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
    // Clamp so a garbage lane count can't reserve the moon.
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

/// Writes nothing if the file changed since `stamped`, so a download
/// finishing under the decode never caches half a waveform.
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

        fn store(&self, track: &Path, lanes: &[Vec<PeakBin>]) {
            store(&self.cache(), track, identity(track), lanes);
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn bin(lo: f32, hi: f32, rms: f32) -> PeakBin {
        PeakBin { lo, hi, rms }
    }

    fn one_bin() -> Vec<Vec<PeakBin>> {
        vec![vec![bin(-1.0, 1.0, 0.7)]]
    }

    #[test]
    fn round_trip() {
        let scratch = Scratch::new("round-trip");
        let track = scratch.track("pcm");
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
        std::fs::write(&track, "different contents").unwrap();
        assert_eq!(load(&scratch.cache(), &track), None);
    }

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

    #[test]
    fn empty_peaks_round_trip() {
        let scratch = Scratch::new("empty");
        let track = scratch.track("pcm");
        scratch.store(&track, &[]);
        assert_eq!(load(&scratch.cache(), &track), Some(Vec::new()));
    }

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

    #[test]
    fn planted_entry_with_wrong_path_misses() {
        let scratch = Scratch::new("collision");
        let cache = scratch.cache();
        std::fs::create_dir_all(&cache).unwrap();
        let a = scratch.0.join("a.flac");
        let b = scratch.0.join("b.flac");
        // Same bytes, so only the stored-path check can reject b.
        std::fs::write(&a, "same-bytes").unwrap();
        std::fs::write(&b, "same-bytes").unwrap();

        scratch.store(&a, &one_bin());
        std::fs::copy(entry_path(&cache, &a), entry_path(&cache, &b)).unwrap();
        assert_eq!(load(&cache, &b), None);
        assert_eq!(load(&cache, &a), Some(one_bin()));
    }
}
