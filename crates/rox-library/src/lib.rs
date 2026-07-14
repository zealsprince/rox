//! The library service per ADR 5 and ADR 6: SQLite is the durable source of
//! truth and the write path, a full in-memory columnar projection is the read
//! path, search is a case-folded substring scan over the projection. The
//! shape was validated at 10 million tracks in rox-prototype-library, which
//! reuses these modules for its harness.

pub mod art;
pub mod projection;
pub mod scanner;
pub mod store;

// Embedders hold a Connection for store queries, so its type needs to be
// nameable without taking on the dep directly.
pub use rusqlite;

/// One track row as it crosses scanner -> SQLite -> projection.
pub struct TrackRow {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub year: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    pub size: u64,
    pub mtime: i64,
}
