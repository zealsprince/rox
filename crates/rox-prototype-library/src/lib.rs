//! Research prototype for the library scale question: ADR 5 (SQLite source of
//! truth plus a full in-memory columnar projection) and ADR 6 (in-memory
//! substring search) were sized against 50-100k tracks. Does the same shape
//! hold at 10 million - cold open, projection RAM, sub-second search, instant
//! sort and filter, scroll windows - and where does it have to split across
//! cores to get there?
//!
//! No real files are involved. A deterministic generator writes a synthetic
//! catalog into SQLite, the projection loads from there exactly as the real
//! library service would, and the binary times every operation the browse UI
//! depends on. Run with --release; see the crate binary for the harness.

pub mod gen;
pub mod projection;
pub mod store;

/// One track row as it crosses generator -> SQLite -> projection.
pub struct TrackRow {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: &'static str,
    pub year: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    pub size: u64,
    pub mtime: i64,
}
