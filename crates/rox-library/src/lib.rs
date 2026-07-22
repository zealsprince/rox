//! The library service per ADR 5 and ADR 6: SQLite is the durable source of
//! truth and the write path, a full in-memory columnar projection is the read
//! path, search is a case-folded substring scan over the projection. The
//! shape was validated at 10 million tracks in rox-prototype-library, which
//! reused these modules for its harness (git history, commit bd22dc1).

pub mod art;
pub mod hash;
pub mod listens;
pub mod lyrics;
pub mod m3u;
pub mod migrate;
pub mod open_files;
pub mod playlists;
pub mod projection;
pub mod rating;
pub mod scanner;
pub mod store;
pub mod tag_source;
pub mod thumbs;
pub mod writer;

// Embedders hold a Connection for store queries, so its type needs to be
// nameable without taking on the dep directly.
pub use rusqlite;

/// The parse options every lofty read in this crate starts from. Relaxed
/// mode, because the default BestAttempt still hard-errors on a malformed
/// date frame (a TDRC holding "06-08", say), and one garbage frame must
/// cost that frame, never the file. Relaxed drops what it cannot parse,
/// so a commit through the writer rewrites such a tag without the frame.
pub(crate) fn parse_opts() -> lofty::config::ParseOptions {
    lofty::config::ParseOptions::new().parsing_mode(lofty::config::ParsingMode::Relaxed)
}

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
    /// The file's rating on the app's 0-100 scale, read off its tags
    /// (FMPS exact, POPM stars); 0 when it carries none.
    pub rating: u8,
    pub size: u64,
    pub mtime: i64,
}
