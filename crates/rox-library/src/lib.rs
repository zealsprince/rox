//! The library service per ADR 5 and ADR 6: SQLite is the durable source of
//! truth and the write path, a full in-memory columnar projection is the read
//! path, search is a case-folded substring scan over the projection. The
//! shape was validated at 10 million tracks in rox-prototype-library, which
//! reused these modules for its harness (git history, commit bd22dc1).

pub mod art;
pub mod bake;
pub mod cue;
pub mod duplicates;
pub mod embed_tag;
pub mod embeddings;
pub mod folders;
pub mod genre;
pub mod genre_meta;
pub mod hash;
pub mod listens;
pub mod lyrics;
pub mod m3u;
pub mod migrate;
pub mod open_files;
pub mod peaks;
pub mod playlists;
pub mod projection;
pub mod rating;
pub mod replaygain;
pub mod scanner;
pub mod sort;
pub mod store;
pub mod tag_source;
pub mod tempo;
pub mod thumbs;
pub mod view;
pub mod watch;
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

/// Whether two field values match under the library's case rule: exact
/// when `fold` is off, case-insensitive when on. The exact check runs
/// first so folding costs nothing on identical strings.
pub fn value_eq(a: &str, b: &str, fold: bool) -> bool {
    a == b || (fold && a.to_lowercase() == b.to_lowercase())
}

/// The cue half of a track row: which sheet claimed the image, and the
/// slice of it this track is. Only cue tracks carry one, so a library of
/// plain files never allocates it and the store writes no side row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueSlice {
    /// The .cue file this span came out of. Kept so a scan can tell which
    /// sheet owns a row, and so a deleted sheet's rows are findable.
    pub cue_path: String,
    pub span: cue::Span,
}

/// One track row as it crosses scanner -> SQLite -> projection.
pub struct TrackRow {
    pub path: String,
    /// Which subsong of `path` this row is: 0 for a plain file, the cue
    /// sheet's 1-based TRACK number for a span of an image. Identity is
    /// (source, path, sub), so a whole-disc rip holds one row per track
    /// and playlists, listens, search and sort all inherit them.
    pub sub: u16,
    /// The span and sheet, for a cue track; None for a plain file. Rides
    /// the row so one upsert lands the track and its side row together.
    pub cue: Option<CueSlice>,
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
    /// The stream's sample rate in Hz (44100, 48000); 0 when the parse
    /// fails. Held in Hz, not kHz, so 44.1 survives the round trip.
    pub sample_rate_hz: u32,
    /// Bits per sample; 0 when the parse fails and for the lossy formats
    /// that have no fixed depth to report.
    pub bit_depth: u8,
    /// The file's rating on the app's 0-100 scale, read off its tags
    /// (FMPS exact, POPM stars); 0 when it carries none.
    pub rating: u8,
    /// What the file's ReplayGain tags measured, all None when it carries
    /// none. The engine levels by these at play time (ADR 19). A file with
    /// none can get them from rox's own measurement pass, which writes past
    /// the scanner straight onto the row.
    pub replay_gain: replaygain::ReplayGain,
    /// The tempo the file's tags claim, in beats a minute; None when it
    /// carries none rox will believe (see [`tempo::parse`]). A file with
    /// none can get one from rox's own analysis pass, which writes past the
    /// scanner straight onto the row.
    pub bpm: Option<f32>,
    pub size: u64,
    pub mtime: i64,
}
