//! The waveform peak cache's location. The format and the reads and writes
//! live in [`rox_library::peaks`].

use std::path::{Path, PathBuf};

use rox_core::settings;

pub use rox_library::peaks::{PeakBin, PeakLanes, identity};

pub fn cache_dir() -> PathBuf {
    settings::data_dir().join("waveforms")
}

/// Blocking; run off the UI thread.
pub fn clear() {
    rox_library::peaks::clear(&cache_dir());
}

pub fn load(track: &Path) -> Option<PeakLanes> {
    rox_library::peaks::load(&cache_dir(), track)
}

/// Stamped with the identity the track had going into the decode.
pub fn store(track: &Path, stamped: Option<(u64, u64)>, lanes: &[Vec<PeakBin>]) {
    rox_library::peaks::store(&cache_dir(), track, stamped, lanes);
}
