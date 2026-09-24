//! The library service per ADR 5 and ADR 6: SQLite is the durable source of
//! truth and the write path, an in-memory columnar projection is the read
//! path, and search is a folded substring scan over it. Validated at 10
//! million tracks (rox-prototype-library, commit bd22dc1).

pub mod album_meta;
pub mod art;
pub mod artist_meta;
pub mod bake;
pub mod bookmarks;
pub mod cue;
pub mod duplicates;
pub mod embed_tag;
pub mod embeddings;
pub mod exclude;
pub mod fold;
pub mod folders;
pub mod genre;
pub mod genre_meta;
pub mod genre_suggest;
pub mod hash;
pub mod health;
pub mod listens;
pub mod locator;
pub mod lyrics;
pub mod m3u;
pub mod migrate;
pub mod mp4;
pub mod open_files;
pub mod peaks;
pub mod playlist_file;
pub mod playlists;
pub mod pls;
pub mod projection;
pub mod rating;
pub mod replaygain;
pub mod scanner;
pub mod song;
pub mod sort;
pub mod stations;
pub mod store;
pub mod tag_source;
pub mod tempo;
pub mod thumbs;
pub mod track_meta;
pub mod view;
pub mod watch;
pub mod writer;
pub mod xspf;

// Re-exported so embedders can name a Connection without the dep.
pub use rusqlite;

/// Relaxed parsing: BestAttempt hard-errors on a malformed frame (a TDRC of
/// "06-08"), and one bad frame must cost that frame, never the file.
pub(crate) fn parse_opts() -> lofty::config::ParseOptions {
    lofty::config::ParseOptions::new().parsing_mode(lofty::config::ParsingMode::Relaxed)
}

/// Field equality under the library's case rule: exact, or case-insensitive
/// when `fold`.
pub fn value_eq(a: &str, b: &str, fold: bool) -> bool {
    a == b || (fold && a.to_lowercase() == b.to_lowercase())
}

/// The cue half of a track row: the sheet that claimed the image, and this
/// track's span of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueSlice {
    pub cue_path: String,
    pub span: cue::Span,
}

/// One track row as it crosses scanner -> SQLite -> projection.
pub struct TrackRow {
    pub path: String,
    /// 0 for a plain file, the cue sheet's 1-based TRACK number for a span.
    /// Identity is (source, path, sub).
    pub sub: u16,
    pub cue: Option<CueSlice>,
    /// A non-local row's stream URL and whether it's live; empty for a file.
    /// Credentials never land here.
    pub remote_url: String,
    pub remote_live: bool,
    pub title: String,
    pub artist: String,
    /// Falls back to the track artist when the tag is missing.
    pub album_artist: String,
    pub album: String,
    /// The four sort names off the file's tags, empty when absent.
    pub title_sort: String,
    pub artist_sort: String,
    pub album_artist_sort: String,
    pub album_sort: String,
    pub genre: String,
    pub year: u16,
    pub disc_no: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub sample_rate_hz: u32,
    /// 0 for lossy formats and failed parses.
    pub bit_depth: u8,
    /// The app's 0-100 scale, 0 when unrated.
    pub rating: u8,
    /// The file's ReplayGain tags (ADR 19), all None when it has none.
    pub replay_gain: replaygain::ReplayGain,
    /// The tagged tempo; None when absent or unbelievable (see [`tempo::parse`]).
    pub bpm: Option<f32>,
    pub size: u64,
    pub mtime: i64,
}
