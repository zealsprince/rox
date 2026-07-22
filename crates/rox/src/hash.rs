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
