//! The waveform peak cache, as the app sees it. The format and the reads
//! and writes over it are in [`rox_library::peaks`], which takes the cache
//! directory as an argument; what stays here is where that directory is,
//! so callers still just name a track.

use std::path::{Path, PathBuf};

use rox_core::settings;

pub use rox_library::peaks::identity;

/// Where the cache is stored, public so the storage page can size it.
pub fn cache_dir() -> PathBuf {
    settings::data_dir().join("waveforms")
}

/// Drop every entry; strips re-decode and re-store on their next play.
/// Blocking while it scans the directory; run off the UI thread.
pub fn clear() {
    rox_library::peaks::clear(&cache_dir());
}

/// The cached peak lanes for a track, or None on any kind of miss.
pub fn load(track: &Path) -> Option<Vec<Vec<(f32, f32)>>> {
    rox_library::peaks::load(&cache_dir(), track)
}

/// Write a track's entry against the identity it had going into the decode.
pub fn store(track: &Path, stamped: Option<(u64, u64)>, lanes: &[Vec<(f32, f32)>]) {
    rox_library::peaks::store(&cache_dir(), track, stamped, lanes);
}
