//! Small stable hashes shared across the crate. The std hasher is
//! randomized per run, so anything that names a cache file or a scratch
//! path by content needs a hash that stays put between runs.

/// FNV-1a over the bytes, stable across runs. The waveform peak cache, the
/// media-control cover scratch files, and the artist cache all key their
/// files on this, so the same track or name keeps its filename between
/// launches.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// The queue group id for an album: a stable hash of the (album artist,
/// album) pair, the same pair the queue panel groups its headings by. Only
/// equality ever matters; the engine treats matching ids as tracks that
/// belong together (ADR 17). None when the album tag is empty, so untagged
/// tracks stay ungrouped instead of all merging into one giant group.
pub fn album_group(album_artist: &str, album: &str) -> Option<u64> {
    if album.is_empty() {
        return None;
    }
    let mut key = Vec::with_capacity(album_artist.len() + album.len() + 1);
    key.extend_from_slice(album_artist.as_bytes());
    // A separator no tag contains, so ("ab", "c") and ("a", "bc") differ.
    key.push(0x1f);
    key.extend_from_slice(album.as_bytes());
    Some(fnv1a(&key))
}
