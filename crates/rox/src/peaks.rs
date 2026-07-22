//! The waveform peak cache: one small binary file per track under
//! `waveforms/` in the app's data directory, so a track's strip comes back
//! instantly after its first play instead of re-decoding the whole file.
//! Entries are keyed by file identity - the source's path, size, and mtime
//! are stored inside and a mismatch on any of them reads as a miss, so an
//! edited or replaced file re-decodes and overwrites its entry. Anything
//! unreadable or malformed is a miss too, never an error; the panel just
//! decodes fresh and stores again.
//!
//! Entry layout, little-endian throughout: the magic, source size (u64),
//! source mtime in unix seconds (u64), path length (u32) and the path's
//! bytes, pair count (u32), then count (min, max) f32 pairs.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rox_library::hash::fnv1a;
use crate::settings;

/// Identifies the layout; bump it when the format changes and old entries
/// read as misses and get rewritten.
const MAGIC: &[u8; 8] = b"roxwave1";

/// Where the cache lives, public so the storage page can size it.
pub fn cache_dir() -> PathBuf {
    settings::data_dir().join("waveforms")
}

/// Drop every entry; strips re-decode and re-store on their next play.
/// Blocking on the directory walk; run off the UI thread.
pub fn clear() {
    let _ = std::fs::remove_dir_all(cache_dir());
}

/// Where a track's entry lives: a hash of the path names the file, the
/// path stored inside the entry disambiguates a collision.
fn entry_path(dir: &Path, track: &Path) -> PathBuf {
    dir.join(format!(
        "{:016x}.peaks",
        fnv1a(track.as_os_str().as_encoded_bytes())
    ))
}

/// The source file's identity as entries store it: size and mtime in unix
/// seconds. None means the file itself is unreadable, so cache nothing.
fn identity(track: &Path) -> Option<(u64, u64)> {
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

/// The cached peaks for a track, or None on any kind of miss: no entry, a
/// stale one (the file changed since it was written), an old format, or a
/// filename collision with another track.
pub fn load(track: &Path) -> Option<Vec<(f32, f32)>> {
    load_from(&cache_dir(), track)
}

fn load_from(dir: &Path, track: &Path) -> Option<Vec<(f32, f32)>> {
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
    let count = take_u32(&mut rest)? as usize;
    let pairs = take(&mut rest, count.checked_mul(8)?)?;
    Some(
        pairs
            .chunks_exact(8)
            .map(|pair| {
                (
                    f32::from_le_bytes(pair[0..4].try_into().unwrap()),
                    f32::from_le_bytes(pair[4..8].try_into().unwrap()),
                )
            })
            .collect(),
    )
}

/// Write a track's entry. Failures log and move on, same stance as the
/// settings file: a lost cache entry only costs a re-decode next time.
pub fn store(track: &Path, peaks: &[(f32, f32)]) {
    store_in(&cache_dir(), track, peaks)
}

fn store_in(dir: &Path, track: &Path, peaks: &[(f32, f32)]) {
    let Some((size, mtime)) = identity(track) else {
        return;
    };
    let _ = std::fs::create_dir_all(dir);
    let path_bytes = track.as_os_str().as_encoded_bytes();
    let mut data = Vec::with_capacity(32 + path_bytes.len() + peaks.len() * 8);
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&size.to_le_bytes());
    data.extend_from_slice(&mtime.to_le_bytes());
    data.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
    data.extend_from_slice(path_bytes);
    data.extend_from_slice(&(peaks.len() as u32).to_le_bytes());
    for &(lo, hi) in peaks {
        data.extend_from_slice(&lo.to_le_bytes());
        data.extend_from_slice(&hi.to_le_bytes());
    }
    let path = entry_path(dir, track);
    if let Err(e) = std::fs::write(&path, data) {
        eprintln!("peaks cache: writing {}: {e}", path.display());
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
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn round_trip() {
        let scratch = Scratch::new("round-trip");
        let track = scratch.track("pcm");
        let peaks = vec![(-0.5, 0.5), (-1.0, 1.0), (0.0, 0.25)];
        store_in(&scratch.cache(), &track, &peaks);
        assert_eq!(load_from(&scratch.cache(), &track), Some(peaks));
    }

    #[test]
    fn changed_file_misses() {
        let scratch = Scratch::new("changed");
        let track = scratch.track("pcm");
        store_in(&scratch.cache(), &track, &[(-1.0, 1.0)]);
        // Same path, different size: the identity check has to fail.
        std::fs::write(&track, "different contents").unwrap();
        assert_eq!(load_from(&scratch.cache(), &track), None);
    }

    #[test]
    fn garbage_entry_misses() {
        let scratch = Scratch::new("garbage");
        let track = scratch.track("pcm");
        let cache = scratch.cache();
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(entry_path(&cache, &track), b"not a peaks file").unwrap();
        assert_eq!(load_from(&cache, &track), None);
    }

    #[test]
    fn missing_entry_misses() {
        let scratch = Scratch::new("missing");
        let track = scratch.track("pcm");
        assert_eq!(load_from(&scratch.cache(), &track), None);
    }

    /// An empty peak buffer stores and loads as empty, not a miss: the strip
    /// for a zero-length file is legitimately empty and should cache like any
    /// other.
    #[test]
    fn empty_peaks_round_trip() {
        let scratch = Scratch::new("empty");
        let track = scratch.track("pcm");
        store_in(&scratch.cache(), &track, &[]);
        assert_eq!(load_from(&scratch.cache(), &track), Some(Vec::new()));
    }

    /// The stored path disambiguates a hash collision: an entry written for
    /// one track but planted where another's entry would live reads as a miss,
    /// so two files that hash alike never hand each other the wrong waveform.
    /// The entry carries a's path bytes, which b's load compares against and
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
        store_in(&cache, &a, &[(-1.0, 1.0)]);
        std::fs::copy(entry_path(&cache, &a), entry_path(&cache, &b)).unwrap();
        // The entry stores a's path, not b's, so b reads a miss.
        assert_eq!(load_from(&cache, &b), None);
        // And a itself still loads from its own entry.
        assert_eq!(load_from(&cache, &a), Some(vec![(-1.0, 1.0)]));
    }
}
