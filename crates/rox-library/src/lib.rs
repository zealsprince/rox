//! The library service per ADR 5 and ADR 6: SQLite is the durable source of
//! truth and the write path, a full in-memory columnar projection is the read
//! path, search is a case-folded substring scan over the projection. The
//! shape was validated at 10 million tracks in rox-prototype-library, which
//! reused these modules for its harness (git history, commit bd22dc1).

pub mod art;
pub mod projection;
pub mod scanner;
pub mod store;
pub mod thumbs;
pub mod writer;

// Embedders hold a Connection for store queries, so its type needs to be
// nameable without taking on the dep directly.
pub use rusqlite;

/// One track row as it crosses scanner -> SQLite -> projection.
pub struct TrackRow {
    pub path: String,
    pub title: String,
    pub artist: String,
    /// The album's credited artist, falling back to the track artist when
    /// the tag is missing, so a plain album groups the same either way.
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: u16,
    /// The disc this track sits on within a multi-disc set; 0 when untagged.
    pub disc_no: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    /// The container's short lowercase name (mp3, flac, wav), off the
    /// parsed file type, the extension when the parse fails.
    pub codec: String,
    /// The audio stream's bitrate in kbps; 0 when the parse fails.
    pub bitrate_kbps: u16,
    pub size: u64,
    pub mtime: i64,
}
