//! The read path of ADR 5 at scale. Columnar: artist, album, and genre are
//! interned to u32 symbols, titles live in one contiguous byte arena with an
//! offset table (never ten million heap Strings), and every browse order is a
//! precomputed Vec<u32> of row indices over integer ranks. Search per ADR 6 is
//! substring: the interned tables are scanned whole (they are a hundredth the
//! row count), only titles need the full-row scan, and that scan splits across
//! cores in fixed chunks. A query is terms ANDed per [`parse_query`], each
//! free or pinned to one field with `field:value` syntax.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::OnceLock;

use memchr::memmem;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::store;

const CHUNK: usize = 65_536;

/// Contiguous strings: one byte buffer, one offset per row boundary.
pub struct Arena {
    bytes: String,
    offsets: Vec<u32>,
}

impl Default for Arena {
    fn default() -> Self {
        Arena {
            bytes: String::new(),
            offsets: vec![0],
        }
    }
}

impl Arena {
    fn push(&mut self, s: &str) {
        self.bytes.push_str(s);
        self.offsets.push(self.bytes.len() as u32);
    }

    fn push_lowercased(&mut self, s: &str) {
        // Query needles fold with str::to_lowercase, whose final-sigma
        // handling char::to_lowercase lacks; fold the same way here so
        // Greek titles match. ASCII skips the allocation.
        if s.is_ascii() {
            self.bytes
                .extend(s.bytes().map(|b| b.to_ascii_lowercase() as char));
        } else {
            self.bytes.push_str(&s.to_lowercase());
        }
        self.offsets.push(self.bytes.len() as u32);
    }

    pub fn get(&self, i: usize) -> &str {
        &self.bytes[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }

    fn append(&mut self, other: &Arena) {
        let base = self.bytes.len() as u32;
        self.bytes.push_str(&other.bytes);
        self.offsets
            .extend(other.offsets[1..].iter().map(|o| o + base));
    }

    pub fn heap_bytes(&self) -> usize {
        self.bytes.capacity() + self.offsets.capacity() * 4
    }
}

#[derive(Default)]
struct Interner {
    /// Whether values differing only by case intern to one symbol. Keys
    /// in `map` are lowercased when set; the display casing gets picked
    /// from `variants` when the table finalizes.
    fold: bool,
    map: HashMap<Box<str>, u32>,
    table: Vec<String>,
    /// Per symbol, every casing seen and how many rows carry it, so the
    /// most common spelling wins the display. Only filled when folding,
    /// so the exact path pays nothing.
    variants: Vec<HashMap<String, u32>>,
}

impl Interner {
    fn folded(fold: bool) -> Self {
        Interner {
            fold,
            ..Default::default()
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        self.intern_weighted(s, 1)
    }

    /// Intern with a pre-counted weight, the shard merge's path: a shard
    /// hands over each casing with the row count it saw, so the display
    /// pick still reflects rows, not shards.
    fn intern_weighted(&mut self, s: &str, weight: u32) -> u32 {
        if self.fold {
            let key = s.to_lowercase();
            if let Some(&sym) = self.map.get(key.as_str()) {
                *self.variants[sym as usize]
                    .entry(s.to_string())
                    .or_default() += weight;
                return sym;
            }
            let sym = self.table.len() as u32;
            self.map.insert(key.into_boxed_str(), sym);
            self.table.push(s.to_string());
            self.variants.push(HashMap::from([(s.to_string(), weight)]));
            return sym;
        }
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = self.table.len() as u32;
        self.map.insert(s.into(), sym);
        self.table.push(s.to_string());
        sym
    }

    /// Fold another interner's symbols in, returning the old-to-new
    /// symbol map the shard merge remaps columns with.
    fn absorb(&mut self, other: &Interner) -> Vec<u32> {
        if !self.fold {
            return other.table.iter().map(|s| self.intern(s)).collect();
        }
        other
            .table
            .iter()
            .enumerate()
            .map(|(sym, s)| {
                let mut mapped = self.intern_weighted(s, 0);
                for (variant, &weight) in &other.variants[sym] {
                    mapped = self.intern_weighted(variant, weight);
                }
                mapped
            })
            .collect()
    }
}

/// Interned strings plus a lowercase copy for case-folded search.
pub struct SymTable {
    pub strings: Vec<String>,
    pub lower: Vec<String>,
}

impl From<Interner> for SymTable {
    fn from(interner: Interner) -> Self {
        // Folded symbols display as the casing the most rows carry, ties
        // to the lexicographically smaller so reloads stay stable.
        let strings: Vec<String> = if interner.fold {
            interner
                .table
                .into_iter()
                .zip(&interner.variants)
                .map(|(first, variants)| {
                    variants
                        .iter()
                        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                        .map(|(s, _)| s.clone())
                        .unwrap_or(first)
                })
                .collect()
        } else {
            interner.table
        };
        let lower = strings.par_iter().map(|s| s.to_lowercase()).collect();
        SymTable { strings, lower }
    }
}

impl SymTable {
    fn heap_bytes(&self) -> usize {
        self.strings
            .iter()
            .chain(self.lower.iter())
            .map(|s| s.capacity() + 24)
            .sum()
    }
}

/// What a ReplayGain column holds for a row whose file carries no gain of
/// that kind. Sorts ahead of every real value the way an unrated row or a
/// missing year does, and decodes back to None.
pub const NO_GAIN: i16 = i16::MIN;

/// A tagged gain packed for the projection: hundredths of a dB in an i16.
/// Every real gain lands well inside the +-40 dB the engine will act on, so
/// the integer holds the tag exactly, sorts without a float comparator, and
/// costs a row two bytes instead of four plus a present flag. Nothing, and
/// anything too wild to be a gain, packs to [`NO_GAIN`].
fn pack_gain(db: Option<f32>) -> i16 {
    match db {
        Some(db) if db.is_finite() => {
            let cdb = (db * 100.).round();
            if cdb <= NO_GAIN as f32 + 1. || cdb >= i16::MAX as f32 {
                NO_GAIN
            } else {
                cdb as i16
            }
        }
        _ => NO_GAIN,
    }
}

/// The dB back out of a packed gain.
pub fn unpack_gain(cdb: i16) -> Option<f32> {
    (cdb != NO_GAIN).then(|| cdb as f32 / 100.)
}

/// What the tempo column holds for a row nothing has filled a tempo for.
/// Zero, since no track runs at no beats a minute: it sorts ahead of every
/// real value the way an unrated row does, and decodes back to None.
pub const NO_BPM: u16 = 0;

/// A tempo packed for the projection: hundredths of a beat a minute in a
/// u16. The store only holds tempos inside [`crate::tempo::SLOWEST`]..=
/// [`crate::tempo::FASTEST`], so the integer holds the value exactly and
/// leaves most of its range spare, at two bytes a row instead of four plus
/// a present flag. Nothing, and anything outside that range, packs to
/// [`NO_BPM`].
fn pack_bpm(bpm: Option<f32>) -> u16 {
    match bpm {
        Some(bpm) if (crate::tempo::SLOWEST..=crate::tempo::FASTEST).contains(&bpm) => {
            (bpm * 100.).round() as u16
        }
        _ => NO_BPM,
    }
}

/// The tempo back out of a packed one.
pub fn unpack_bpm(cbpm: u16) -> Option<f32> {
    (cbpm != NO_BPM).then(|| cbpm as f32 / 100.)
}

/// One shard of rows being loaded; also the whole library when loading serially.
#[derive(Default)]
pub struct Builder {
    db_id: Vec<i64>,
    title: Arena,
    title_lower: Arena,
    artist: Vec<u32>,
    album_artist: Vec<u32>,
    album: Vec<u32>,
    genre: Vec<u32>,
    year: Vec<u16>,
    disc_no: Vec<u16>,
    track_no: Vec<u16>,
    duration_ms: Vec<u32>,
    codec: Vec<u32>,
    bitrate_kbps: Vec<u16>,
    sample_rate_hz: Vec<u32>,
    bit_depth: Vec<u8>,
    rating: Vec<u8>,
    added: Vec<i64>,
    track_gain: Vec<i16>,
    album_gain: Vec<i16>,
    bpm: Vec<u16>,
    bpm_source: Vec<crate::tempo::Source>,
    sub: Vec<u16>,
    folder: Vec<u32>,
    artists: Interner,
    album_artists: Interner,
    albums: Interner,
    genres: Interner,
    codecs: Interner,
    folders: Interner,
}

impl Builder {
    /// A builder whose name fields (artist, album artist, album, genre)
    /// fold case per the library setting. Codecs are lowercase by
    /// construction and folders are filesystem paths, so those two stay
    /// exact either way.
    fn new(fold: bool) -> Self {
        Builder {
            artists: Interner::folded(fold),
            album_artists: Interner::folded(fold),
            albums: Interner::folded(fold),
            genres: Interner::folded(fold),
            ..Default::default()
        }
    }

    fn push(&mut self, row: store::ScanRow<'_>) {
        self.db_id.push(row.id);
        self.title.push(row.title);
        self.title_lower.push_lowercased(row.title);
        self.artist.push(self.artists.intern(row.artist));
        self.album_artist
            .push(self.album_artists.intern(row.album_artist));
        self.album.push(self.albums.intern(row.album));
        self.genre.push(self.genres.intern(row.genre));
        self.year.push(row.year);
        self.disc_no.push(row.disc_no);
        self.track_no.push(row.track_no);
        self.duration_ms.push(row.duration_ms);
        self.codec.push(self.codecs.intern(row.codec));
        self.bitrate_kbps.push(row.bitrate_kbps);
        self.sample_rate_hz.push(row.sample_rate_hz);
        self.bit_depth.push(row.bit_depth);
        self.rating.push(row.rating);
        self.added.push(row.added);
        self.track_gain.push(pack_gain(row.track_gain_db));
        self.album_gain.push(pack_gain(row.album_gain_db));
        self.bpm.push(pack_bpm(row.bpm));
        self.bpm_source.push(row.bpm_source);
        self.sub.push(row.sub);
        // Interned per album directory, so it stays cheap even at ten
        // million rows; an empty parent (a bare filename) folds to "".
        let folder = Path::new(row.path)
            .parent()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default();
        self.folder.push(self.folders.intern(&folder));
    }
}

pub struct Projection {
    /// Whether the name symbols interned case-folded, the library's
    /// case-insensitive setting at load time. Matching against symbol
    /// strings folds the same way when set, so a stale pick made under
    /// the other casing still lands.
    pub fold: bool,
    pub db_id: Vec<i64>,
    pub title: Arena,
    pub title_lower: Arena,
    pub artist: Vec<u32>,
    pub album_artist: Vec<u32>,
    pub album: Vec<u32>,
    pub genre: Vec<u32>,
    pub year: Vec<u16>,
    pub disc_no: Vec<u16>,
    pub track_no: Vec<u16>,
    pub duration_ms: Vec<u32>,
    pub codec: Vec<u32>,
    pub bitrate_kbps: Vec<u16>,
    /// The stream's sample rate in Hz and bits per sample. Plain columns
    /// rather than interned: a library holds a handful of distinct values
    /// but they are a u32 and a u8, so a symbol table would cost more than
    /// the numbers it replaced.
    pub sample_rate_hz: Vec<u32>,
    pub bit_depth: Vec<u8>,
    /// When each row was first scanned into the library, in unix seconds.
    /// Set on first insert and preserved across rescans, so a descending
    /// sort surfaces newly added tracks.
    pub added: Vec<i64>,
    /// Ratings on the app's 0-100 scale, 0 unrated. Atomics, unlike every
    /// other column: a rating click writes through the shared Arc in
    /// place, so rating a track never pays a projection reload.
    pub rating: Vec<AtomicU8>,
    /// Play counts derived from the listens table at load. Atomics for
    /// the ratings' reason: a landing listen bumps its track in place,
    /// so a play never pays a projection reload. Per ADR 11 the events
    /// stay the source; this column only caches their per-track count.
    pub plays: Vec<AtomicU32>,
    /// The two ReplayGain figures a row carries (ADR 19), packed to
    /// centi-dB per [`pack_gain`] with [`NO_GAIN`] for an untagged file.
    /// Only the gains: the peaks bound playback, and nothing browsing the
    /// library sorts or reads by them, so they stay in the database.
    pub track_gain: Vec<i16>,
    pub album_gain: Vec<i16>,
    /// What each row runs at, packed to centi-bpm per [`pack_bpm`] with
    /// [`NO_BPM`] for a track nothing has filled a tempo for.
    pub bpm: Vec<u16>,
    /// Which of the two sources filled the tempo beside it: the file's own
    /// tags, or rox's estimate. One byte a row, so a UI can mark an
    /// estimate as one without going back to the database per visible row.
    pub bpm_source: Vec<crate::tempo::Source>,
    /// Which subsong of its file each row is: 0 for a plain file, the cue
    /// sheet's 1-based track number for a span of an image. Dense because
    /// it's two bytes a track and every TrackKey the UI builds needs it.
    pub sub: Vec<u16>,
    /// The cue tracks' spans, keyed by row index. Sparse on purpose per the
    /// ADR 5 memory discipline: a library with no cue sheets holds an empty
    /// map instead of a dense column of None, and even a library full of
    /// them only pays per cue row. Nothing reads this to show a duration -
    /// duration_ms is already on the row - it's for the player.
    pub spans: HashMap<u32, crate::cue::Span>,
    /// Each track's parent directory, interned. Folders repeat once per
    /// album directory, so interning keeps this a handful of symbols even
    /// across a huge library. Searchable and filterable like artist/album.
    pub folder: Vec<u32>,
    pub artists: SymTable,
    pub album_artists: SymTable,
    pub albums: SymTable,
    pub genres: SymTable,
    pub codecs: SymTable,
    pub folders: SymTable,
    /// The lowered-order rank of each symbol, filled on the first sort that
    /// needs it and reused after. The projection is immutable once loaded, so
    /// these never go stale; every sort's canonical tie-break wants the album
    /// artist and album ranks, so ranking them once beats re-sorting the whole
    /// symbol table per sort.
    artist_ranks: OnceLock<Vec<u32>>,
    album_artist_ranks: OnceLock<Vec<u32>>,
    album_ranks: OnceLock<Vec<u32>>,
    genre_ranks: OnceLock<Vec<u32>>,
    codec_ranks: OnceLock<Vec<u32>>,
    /// The distinct album artists and (album artist, album) pairs, each with
    /// its first-seen row, in row order. Query-independent, so the per-keystroke
    /// search_artists/search_albums filter these instead of rescanning every row
    /// with a HashSet each call. Built lazily on first search like the ranks,
    /// and safe to memoize for the same reason: the projection is immutable once
    /// loaded, so first-seen never shifts.
    distinct_artists: OnceLock<Vec<ArtistHit>>,
    distinct_albums: OnceLock<Vec<AlbumHit>>,
    /// The distinct genre values with the "; " lists split apart, for
    /// value suggestions: the symbol table holds the lists whole, but a
    /// completion should offer "Shoegaze", never "Rock; Shoegaze". Built
    /// lazily like the ranks, safe to memoize the same way.
    genre_terms: OnceLock<SymTable>,
}

pub struct RowView<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album_artist: &'a str,
    pub album: &'a str,
    pub genre: &'a str,
    pub year: u16,
    pub disc_no: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    pub codec: &'a str,
    pub bitrate_kbps: u16,
    pub sample_rate_hz: u32,
    pub bit_depth: u8,
    pub rating: u8,
    pub plays: u32,
    pub added: i64,
    /// The file's own ReplayGain figures in dB, None where it carries none.
    pub track_gain_db: Option<f32>,
    pub album_gain_db: Option<f32>,
    /// What the row runs at in beats a minute, None where nothing has
    /// filled a tempo for it.
    pub bpm: Option<f32>,
    /// Where that tempo came from, so a display can tell an estimate from
    /// what a tagger wrote.
    pub bpm_source: crate::tempo::Source,
    pub folder: &'a str,
    /// Which subsong of its file the row is, 0 for a plain file.
    pub sub: u16,
}

/// One album-artist match from [`Projection::search_artists`]: the interned
/// album-artist symbol and a representative row for its cover.
#[derive(Clone, Copy)]
pub struct ArtistHit {
    pub album_artist: u32,
    pub row: u32,
}

/// One album match from [`Projection::search_albums`]: the (album artist,
/// album) symbol pair and a representative row for its cover and year.
#[derive(Clone, Copy)]
pub struct AlbumHit {
    pub album_artist: u32,
    pub album: u32,
    pub row: u32,
}

/// A field a query term can be pinned to with `field:value` syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryField {
    Title,
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Year,
    Folder,
    Codec,
    /// The three numeric pins, which take a comparison rather than a
    /// substring: `rating:>=4`, `plays:0`, `added:<90d`. Pin-only, like
    /// folder and codec - a bare number is a plausible title or year, and
    /// matching it here would bury the real hit.
    Rating,
    Plays,
    Added,
}

impl QueryField {
    /// Whether the field takes a numeric comparison instead of a substring.
    pub fn numeric(self) -> bool {
        matches!(
            self,
            QueryField::Rating | QueryField::Plays | QueryField::Added
        )
    }
}

/// The `field:` prefixes the query syntax accepts, shared with the
/// suggestion provider so both sides agree on the names.
pub const QUERY_FIELDS: &[(&str, QueryField)] = &[
    ("title", QueryField::Title),
    ("artist", QueryField::Artist),
    ("albumartist", QueryField::AlbumArtist),
    ("album", QueryField::Album),
    ("genre", QueryField::Genre),
    ("year", QueryField::Year),
    ("folder", QueryField::Folder),
    ("codec", QueryField::Codec),
    ("rating", QueryField::Rating),
    ("plays", QueryField::Plays),
    ("added", QueryField::Added),
];

/// How a numeric term compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumOp {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A parsed numeric term: the comparison and the number behind it. The
/// number means the column's own value for `rating:` (whole stars, 0
/// unrated) and `plays:`, and an age in days for `added:`, so
/// `added:<90d` is "added in the last 90 days".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumTerm {
    pub op: NumOp,
    pub value: i64,
}

/// A comparison nothing satisfies, the fallback for a numeric pin that
/// somehow reached a matcher without its number. [`parse_query`] never
/// builds one - it drops back to a free term instead - so this only keeps
/// the matchers total.
const NUM_NEVER: NumTerm = NumTerm {
    op: NumOp::Lt,
    value: i64::MIN,
};

impl NumTerm {
    /// Whether a column value satisfies the comparison.
    pub fn holds(&self, n: i64) -> bool {
        match self.op {
            NumOp::Eq => n == self.value,
            NumOp::Lt => n < self.value,
            NumOp::Le => n <= self.value,
            NumOp::Gt => n > self.value,
            NumOp::Ge => n >= self.value,
        }
    }
}

/// Split a numeric field's value into its comparison and number. The
/// operator is optional and defaults to equality, so `rating:3` is
/// `rating:=3`; a trailing `d` (the `added:` day suffix) is accepted and
/// dropped. None when what follows isn't a plain number, which sends the
/// whole token back to being a free text term.
fn parse_num(value: &str) -> Option<NumTerm> {
    let value = value.trim();
    let (op, rest) = if let Some(rest) = value.strip_prefix(">=") {
        (NumOp::Ge, rest)
    } else if let Some(rest) = value.strip_prefix("<=") {
        (NumOp::Le, rest)
    } else if let Some(rest) = value.strip_prefix('>') {
        (NumOp::Gt, rest)
    } else if let Some(rest) = value.strip_prefix('<') {
        (NumOp::Lt, rest)
    } else if let Some(rest) = value.strip_prefix('=') {
        (NumOp::Eq, rest)
    } else {
        (NumOp::Eq, value)
    };
    let rest = rest.trim();
    let digits = rest.strip_suffix('d').unwrap_or(rest);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().map(|value| NumTerm { op, value })
}

/// The whole stars a stored 0-100 rating reads as, 0 for unrated. What a
/// `rating:` term compares against, so the query speaks the same 0-5 the
/// star cells draw.
fn rating_stars(value: u8) -> i64 {
    if value == 0 {
        0
    } else {
        crate::rating::stars(value) as i64
    }
}

/// Unix seconds now, the clock a bare [`Projection::search`] resolves
/// `added:` ages against. The matcher itself takes the timestamp.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One parsed query term: a lowercased needle, maybe pinned to one field.
/// A term pinned to a numeric field carries its comparison in `num` and
/// leaves the needle as the raw value text.
pub struct Term {
    pub field: Option<QueryField>,
    pub needle: String,
    /// The comparison behind a numeric pin; None for every text term.
    pub num: Option<NumTerm>,
}

/// Split a query into terms. Whitespace separates, double quotes keep a
/// value together, and a known `field:` prefix pins the term to that
/// field; every term must match for a row to hit. So
/// `stronger artist:"daft punk"` is a free term and an artist term, and
/// an unknown prefix like `ac:dc` stays one free term.
///
/// The numeric fields take a comparison instead of a substring:
/// `rating:>=4`, `plays:0`, `added:<90d`. A numeric pin whose value isn't
/// a number (`rating:great`) falls back to a free term, the same rule an
/// unknown prefix follows. Operators on a text field stay literal, so
/// `year:>1990` looks for the characters ">1990" and finds nothing.
pub fn parse_query(query: &str) -> Vec<Term> {
    let mut tokens: Vec<String> = Vec::new();
    let mut token = String::new();
    let mut in_quotes = false;
    for c in query.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                token.push(c);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            c => token.push(c),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }

    let strip = |s: &str| -> String { s.chars().filter(|&c| c != '"').collect() };
    tokens
        .iter()
        .map(|raw| {
            if let Some(i) = raw.find(':') {
                let name = &raw[..i];
                if !name.contains('"') {
                    let name = name.to_lowercase();
                    if let Some(&(_, field)) = QUERY_FIELDS.iter().find(|(n, _)| *n == name) {
                        let needle = strip(&raw[i + 1..]).to_lowercase();
                        let num = field.numeric().then(|| parse_num(&needle));
                        // A numeric pin with nothing numeric behind it is
                        // not a filter anybody meant; let it read as text.
                        if !matches!(num, Some(None)) {
                            return Term {
                                field: Some(field),
                                needle,
                                num: num.flatten(),
                            };
                        }
                    }
                }
            }
            Term {
                field: None,
                needle: strip(raw).to_lowercase(),
                num: None,
            }
        })
        .filter(|t| !t.needle.is_empty())
        .collect()
}

/// A field the structured filter can pin exact values to: the interned
/// columns plus the year. Titles stay out; a text term already reaches
/// them, and a filter over ten million distinct titles filters nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterField {
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Year,
    Folder,
}

/// A structured filter over exact field values, the filter panel's state:
/// values OR within a field, fields AND across. Unlike [`parse_query`]'s
/// terms these match whole values, never substrings, so picking "Air"
/// leaves "Airborne" out. Years ride as their decimal strings ("0" for
/// untagged) to keep the value lists one shape. Folder picks are the one
/// exception to whole-value matching: a picked folder covers its whole
/// subtree, so the folder tree scopes to a branch with a single value
/// instead of enumerating every descendant.
///
/// A set can also pin an explicit list of track db ids, which is how a view
/// following the app-wide selection narrows to it. That rides here rather
/// than beside the filter because every searching panel already threads a
/// `FilterSet` down to its row scan, so honoring it in the two matchers
/// below reaches all of them at once. `None` is no id restriction at all;
/// `Some` of an empty list matches nothing, which is what an emptied
/// selection should show.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterSet {
    pub fields: Vec<(FilterField, Vec<String>)>,
    pub ids: Option<Vec<i64>>,
}

impl FilterSet {
    pub fn is_empty(&self) -> bool {
        self.ids.is_none() && self.fields.iter().all(|(_, values)| values.is_empty())
    }

    /// Whether any field value is picked, ignoring an id pin. The filter
    /// chips read this: the ids are not the filter panel's doing and have no
    /// chip to drop.
    pub fn fields_empty(&self) -> bool {
        self.fields.iter().all(|(_, values)| values.is_empty())
    }

    /// Narrow to an explicit set of track db ids.
    pub fn with_ids(ids: Vec<i64>) -> Self {
        FilterSet {
            fields: Vec::new(),
            ids: Some(ids),
        }
    }

    /// Whether one track's db id passes the id pin; no pin passes all. A
    /// row with no db id (a file dropped on the queue, never scanned) is
    /// in no id-keyed set there is, so a pin always leaves it out.
    fn id_ok(&self, db_id: Option<i64>) -> bool {
        match (&self.ids, db_id) {
            (Some(ids), Some(db_id)) => ids.contains(&db_id),
            (Some(_), None) => false,
            (None, _) => true,
        }
    }

    /// The picked values for one field; empty means the field passes all.
    pub fn values(&self, field: FilterField) -> &[String] {
        self.fields
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, values)| values.as_slice())
            .unwrap_or(&[])
    }

    /// Add the value to the field's picks, or drop it if already picked.
    pub fn toggle(&mut self, field: FilterField, value: &str) {
        match self.fields.iter_mut().find(|(f, _)| *f == field) {
            Some((_, values)) => match values.iter().position(|v| v == value) {
                Some(i) => {
                    values.remove(i);
                }
                None => values.push(value.to_string()),
            },
            None => self.fields.push((field, vec![value.to_string()])),
        }
        self.fields.retain(|(_, values)| !values.is_empty());
    }

    /// Drop every pick for one field.
    pub fn clear(&mut self, field: FilterField) {
        self.fields.retain(|(f, _)| *f != field);
    }

    /// Whether one track's fields satisfy the filter: within a field its
    /// value must be one of the picks, across fields all must pass. The
    /// whole-value counterpart to [`Projection::filter_mask`], for a panel
    /// filtering its own row list (the queue, history, playlists) instead of
    /// the projection. Values match whole, never as substrings, the same as
    /// the mask over the catalog. `fold` is the library's case rule: these
    /// rows carry raw strings while picks carry the folded tables' display
    /// casing, so a case-insensitive library must compare folded here.
    pub fn matches(&self, fields: &TrackFields, fold: bool) -> bool {
        if !self.id_ok(fields.db_id) {
            return false;
        }
        self.fields.iter().all(|(field, values)| {
            if values.is_empty() {
                return true;
            }
            match field {
                FilterField::Artist => values
                    .iter()
                    .any(|v| crate::value_eq(v, fields.artist, fold)),
                FilterField::AlbumArtist => values
                    .iter()
                    .any(|v| crate::value_eq(v, fields.album_artist, fold)),
                FilterField::Album => values
                    .iter()
                    .any(|v| crate::value_eq(v, fields.album, fold)),
                // Genre picks match against the "; " list's values, so a
                // "Shoegaze" pick takes a "Rock; Shoegaze" track too.
                FilterField::Genre => values
                    .iter()
                    .any(|v| crate::genre::has(fields.genre, v, fold)),
                FilterField::Folder => {
                    let folder = fields.folder();
                    values.iter().any(|v| folder_in_subtree(&folder, v))
                }
                FilterField::Year => values.contains(&fields.year.to_string()),
            }
        })
    }
}

/// The plain-string fields a query term or filter matches against, for a
/// track list that isn't the projection - the queue, history, and playlists
/// filter their own rows through [`track_matches`] and [`FilterSet::matches`]
/// rather than the column-optimized [`Projection::search`] and
/// [`Projection::filter_mask`] over the whole catalog.
pub struct TrackFields<'a> {
    /// The track's library db id, for [`FilterSet`]'s id pin. None for a
    /// row the catalog never gave one: a file dropped straight on the
    /// queue is off-catalog, and an id pin leaves every one of them out
    /// rather than folding them onto a shared phantom id.
    pub db_id: Option<i64>,
    pub title: &'a str,
    pub artist: &'a str,
    pub album_artist: &'a str,
    pub album: &'a str,
    pub genre: &'a str,
    pub year: u16,
    /// The file's format, for the `codec:` pin; the scanner's lowercase
    /// name ("mp3", "flac"), empty when the row never got one.
    pub codec: &'a str,
    /// The track's file path, for the `folder:` pin and the folder filter;
    /// empty when there is none. The folder itself is the parent directory,
    /// resolved the same way the projection interns it.
    pub path: &'a str,
}

/// Whether a folder sits at or under a picked one: the pick itself, or a
/// descendant by path prefix with a separator boundary, so "Music/Air"
/// never pulls in "Music/Airborne".
fn folder_in_subtree(folder: &str, pick: &str) -> bool {
    folder
        .strip_prefix(pick)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(std::path::MAIN_SEPARATOR))
}

impl TrackFields<'_> {
    /// The file's parent directory, the projection's folder value; an empty
    /// parent (a bare filename) folds to "".
    fn folder(&self) -> String {
        Path::new(self.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// The decimal string of every u16, built once and shared. A `year:` term
/// matches on the digits, so the search would else format all 65536 years to a
/// fresh String per keystroke; this holds them as one contiguous arena so a
/// year term only substring-tests borrowed slices.
fn year_strings() -> &'static Arena {
    static YEARS: OnceLock<Arena> = OnceLock::new();
    YEARS.get_or_init(|| {
        // Digits straight into a stack buffer, no per-year String. A u16 is at
        // most five digits, written back to front then pushed as one slice.
        let mut arena = Arena::default();
        let mut buf = [0u8; 5];
        for y in 0..=u16::MAX {
            let mut n = y;
            let mut i = buf.len();
            loop {
                i -= 1;
                buf[i] = b'0' + (n % 10) as u8;
                n /= 10;
                if n == 0 {
                    break;
                }
            }
            // The bytes are ASCII digits, so the slice is valid UTF-8.
            arena.push(std::str::from_utf8(&buf[i..]).unwrap());
        }
        arena
    })
}

/// Case-folded substring test with a needle already lowercased by
/// [`parse_query`]. An empty needle matches everything.
fn contains_fold(haystack: &str, needle_lower: &str) -> bool {
    needle_lower.is_empty() || haystack.to_lowercase().contains(needle_lower)
}

/// A row a panel filters its own list with. The panels that hold their own
/// rows rather than a projection slice (the queue, the playlists tree) all
/// ran the same two matchers over the same borrowed fields; naming the
/// fields is the only part that differs, so that's all an implementor
/// writes.
pub trait Filterable {
    /// This row's fields, borrowed for one match.
    fn fields(&self) -> TrackFields<'_>;

    /// Whether the row passes both halves of the active query: the free and
    /// pinned text terms, and the structured filter.
    fn passes(&self, terms: &[Term], filter: &FilterSet, fold: bool) -> bool {
        let fields = self.fields();
        track_matches(terms, &fields) && filter.matches(&fields, fold)
    }
}

/// Whether one track's fields satisfy every parsed query term. Free terms
/// match title, artist, album artist, album, or genre; a pinned term only its
/// field, the same rule [`Projection::search`] applies over the catalog.
/// Terms AND together; needles come lowercased from [`parse_query`].
///
/// The numeric pins read columns a plain row list doesn't carry - rating,
/// play count, and added date live on the projection - so they match
/// nothing here. A `rating:>=4` typed into the queue or playlists box
/// comes back empty rather than quietly ignoring the term; the catalog's
/// own views (and smart playlists) run through [`Projection::search`],
/// where the columns exist.
pub fn track_matches(terms: &[Term], fields: &TrackFields) -> bool {
    terms.iter().all(|t| match t.field {
        None => {
            contains_fold(fields.title, &t.needle)
                || contains_fold(fields.artist, &t.needle)
                || contains_fold(fields.album_artist, &t.needle)
                || contains_fold(fields.album, &t.needle)
                || contains_fold(fields.genre, &t.needle)
        }
        Some(QueryField::Title) => contains_fold(fields.title, &t.needle),
        Some(QueryField::Artist) => contains_fold(fields.artist, &t.needle),
        Some(QueryField::AlbumArtist) => contains_fold(fields.album_artist, &t.needle),
        Some(QueryField::Album) => contains_fold(fields.album, &t.needle),
        Some(QueryField::Genre) => contains_fold(fields.genre, &t.needle),
        Some(QueryField::Folder) => contains_fold(&fields.folder(), &t.needle),
        Some(QueryField::Codec) => contains_fold(fields.codec, &t.needle),
        Some(QueryField::Year) => fields.year.to_string().contains(t.needle.as_str()),
        Some(QueryField::Rating | QueryField::Plays | QueryField::Added) => false,
    })
}

/// A sortable column of the projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    Title,
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Year,
    TrackNo,
    Duration,
    Codec,
    Bitrate,
    SampleRate,
    BitDepth,
    Rating,
    Plays,
    Added,
    /// The gain the Track leveling mode would read: the track figure, the
    /// album one where a file only carries that, matching what the engine
    /// falls back to and what the Gain column draws.
    TrackGain,
    /// The same the other way round, for the Album mode.
    AlbumGain,
    /// How fast the track runs, whichever source wrote the number.
    Bpm,
}

impl Projection {
    pub fn len(&self) -> usize {
        self.db_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.db_id.is_empty()
    }

    /// Load on one connection, one thread: the ADR 5 shape as written.
    /// `fold` merges values differing only by case into one symbol, the
    /// case-insensitive library setting.
    pub fn load_serial(conn: &rusqlite::Connection, fold: bool) -> rusqlite::Result<Self> {
        let max = store::max_rowid(conn)?;
        let mut b = Builder::new(fold);
        store::scan_range(conn, 0, max, |row| b.push(row))?;
        let mut projection = Self::merge(vec![b], fold);
        projection.fill_plays(conn)?;
        projection.fill_spans(conn)?;
        Ok(projection)
    }

    /// Load with one reader per shard over disjoint rowid ranges (WAL allows
    /// concurrent readers), then merge shards by remapping local symbols.
    pub fn load_parallel(db_path: &Path, shards: usize, fold: bool) -> rusqlite::Result<Self> {
        let conn = store::open(db_path)?;
        let max = store::max_rowid(&conn)?;
        drop(conn);

        let step = (max + shards as i64 - 1) / shards as i64;
        let builders: Vec<rusqlite::Result<Builder>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..shards)
                .map(|s| {
                    let lo = s as i64 * step;
                    let hi = (lo + step).min(max);
                    scope.spawn(move || {
                        let conn = store::open(db_path)?;
                        let mut b = Builder::new(fold);
                        store::scan_range(&conn, lo, hi, |row| b.push(row))?;
                        Ok(b)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let mut shards = Vec::with_capacity(builders.len());
        for b in builders {
            shards.push(b?);
        }
        let mut projection = Self::merge(shards, fold);
        let conn = store::open(db_path)?;
        projection.fill_plays(&conn)?;
        projection.fill_spans(&conn)?;
        Ok(projection)
    }

    /// Fill the plays column from the listens table: one aggregate query,
    /// then a walk mapping counts onto rows by track id.
    fn fill_plays(&self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        let counts = crate::listens::counts(conn)?;
        if counts.is_empty() {
            return Ok(());
        }
        for (i, id) in self.db_id.iter().enumerate() {
            if let Some(&n) = counts.get(id) {
                self.plays[i].store(n, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    /// Fill the sparse span map from the cue side table, after the merge has
    /// fixed what row each track id landed on. One query for the whole
    /// library rather than per shard: the table holds a row per cue track and
    /// nothing at all in a library of plain files, so there's no work to
    /// split. A span whose track id is not in the projection is skipped.
    fn fill_spans(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        let spans = store::cue_spans(conn)?;
        if spans.is_empty() {
            return Ok(());
        }
        for (row, id) in self.db_id.iter().enumerate() {
            if let Some(&span) = spans.get(id) {
                self.spans.insert(row as u32, span);
            }
        }
        Ok(())
    }

    fn merge(shards: Vec<Builder>, fold: bool) -> Self {
        let mut artists = Interner::folded(fold);
        let mut album_artists = Interner::folded(fold);
        let mut albums = Interner::folded(fold);
        let mut genres = Interner::folded(fold);
        let mut codecs = Interner::default();
        let mut folders = Interner::default();
        let total: usize = shards.iter().map(|s| s.db_id.len()).sum();

        let mut out = Builder::default();
        out.db_id.reserve(total);
        out.artist.reserve(total);
        out.album_artist.reserve(total);
        out.album.reserve(total);
        out.genre.reserve(total);
        out.year.reserve(total);
        out.disc_no.reserve(total);
        out.track_no.reserve(total);
        out.duration_ms.reserve(total);
        out.codec.reserve(total);
        out.bitrate_kbps.reserve(total);
        out.sample_rate_hz.reserve(total);
        out.bit_depth.reserve(total);
        out.rating.reserve(total);
        out.added.reserve(total);
        out.track_gain.reserve(total);
        out.album_gain.reserve(total);
        out.bpm.reserve(total);
        out.bpm_source.reserve(total);
        out.sub.reserve(total);
        out.folder.reserve(total);

        for shard in shards {
            let map_a = artists.absorb(&shard.artists);
            let map_aa = album_artists.absorb(&shard.album_artists);
            let map_b = albums.absorb(&shard.albums);
            let map_g = genres.absorb(&shard.genres);
            let map_c = codecs.absorb(&shard.codecs);
            let map_f = folders.absorb(&shard.folders);
            out.db_id.extend_from_slice(&shard.db_id);
            out.title.append(&shard.title);
            out.title_lower.append(&shard.title_lower);
            out.artist
                .extend(shard.artist.iter().map(|&s| map_a[s as usize]));
            out.album_artist
                .extend(shard.album_artist.iter().map(|&s| map_aa[s as usize]));
            out.album
                .extend(shard.album.iter().map(|&s| map_b[s as usize]));
            out.genre
                .extend(shard.genre.iter().map(|&s| map_g[s as usize]));
            out.year.extend_from_slice(&shard.year);
            out.disc_no.extend_from_slice(&shard.disc_no);
            out.track_no.extend_from_slice(&shard.track_no);
            out.duration_ms.extend_from_slice(&shard.duration_ms);
            out.codec
                .extend(shard.codec.iter().map(|&s| map_c[s as usize]));
            out.bitrate_kbps.extend_from_slice(&shard.bitrate_kbps);
            out.sample_rate_hz.extend_from_slice(&shard.sample_rate_hz);
            out.bit_depth.extend_from_slice(&shard.bit_depth);
            out.rating.extend_from_slice(&shard.rating);
            out.added.extend_from_slice(&shard.added);
            out.track_gain.extend_from_slice(&shard.track_gain);
            out.album_gain.extend_from_slice(&shard.album_gain);
            out.bpm.extend_from_slice(&shard.bpm);
            out.bpm_source.extend_from_slice(&shard.bpm_source);
            out.sub.extend_from_slice(&shard.sub);
            out.folder
                .extend(shard.folder.iter().map(|&s| map_f[s as usize]));
        }

        let plays = (0..out.db_id.len()).map(|_| AtomicU32::new(0)).collect();
        Projection {
            fold,
            db_id: out.db_id,
            title: out.title,
            title_lower: out.title_lower,
            artist: out.artist,
            album_artist: out.album_artist,
            album: out.album,
            genre: out.genre,
            year: out.year,
            disc_no: out.disc_no,
            track_no: out.track_no,
            duration_ms: out.duration_ms,
            codec: out.codec,
            bitrate_kbps: out.bitrate_kbps,
            sample_rate_hz: out.sample_rate_hz,
            bit_depth: out.bit_depth,
            added: out.added,
            track_gain: out.track_gain,
            album_gain: out.album_gain,
            bpm: out.bpm,
            bpm_source: out.bpm_source,
            sub: out.sub,
            // Filled after the merge by fill_spans, which needs a connection
            // and reads a table the shard loaders never touch.
            spans: HashMap::new(),
            rating: out.rating.into_iter().map(AtomicU8::new).collect(),
            plays,
            folder: out.folder,
            artists: SymTable::from(artists),
            album_artists: SymTable::from(album_artists),
            albums: SymTable::from(albums),
            genres: SymTable::from(genres),
            codecs: SymTable::from(codecs),
            folders: SymTable::from(folders),
            artist_ranks: OnceLock::new(),
            album_artist_ranks: OnceLock::new(),
            album_ranks: OnceLock::new(),
            genre_ranks: OnceLock::new(),
            codec_ranks: OnceLock::new(),
            distinct_artists: OnceLock::new(),
            distinct_albums: OnceLock::new(),
            genre_terms: OnceLock::new(),
        }
    }

    pub fn resolve(&self, row: u32) -> RowView<'_> {
        let i = row as usize;
        RowView {
            title: self.title.get(i),
            artist: &self.artists.strings[self.artist[i] as usize],
            album_artist: &self.album_artists.strings[self.album_artist[i] as usize],
            album: &self.albums.strings[self.album[i] as usize],
            genre: &self.genres.strings[self.genre[i] as usize],
            year: self.year[i],
            disc_no: self.disc_no[i],
            track_no: self.track_no[i],
            duration_ms: self.duration_ms[i],
            codec: &self.codecs.strings[self.codec[i] as usize],
            bitrate_kbps: self.bitrate_kbps[i],
            sample_rate_hz: self.sample_rate_hz[i],
            bit_depth: self.bit_depth[i],
            rating: self.rating[i].load(Ordering::Relaxed),
            plays: self.plays[i].load(Ordering::Relaxed),
            added: self.added[i],
            track_gain_db: unpack_gain(self.track_gain[i]),
            album_gain_db: unpack_gain(self.album_gain[i]),
            bpm: unpack_bpm(self.bpm[i]),
            bpm_source: self.bpm_source[i],
            folder: &self.folders.strings[self.folder[i] as usize],
            sub: self.sub[i],
        }
    }

    /// The cue span a row plays, None for a plain file. Sparse lookup, so
    /// this is a hash probe rather than an index.
    pub fn span(&self, row: u32) -> Option<crate::cue::Span> {
        self.spans.get(&row).copied()
    }

    /// Case-folded substring search, one term at a time per
    /// [`parse_query`]: a free term matches title, artist, album artist,
    /// album, or genre; a pinned term only its field. Terms AND together.
    /// Symbol tables are matched whole first; the row scan then only does
    /// per-title memmem plus table lookups.
    ///
    /// `added:` ages resolve against the clock here. A caller that needs
    /// the same query to mean the same thing twice (a test, a saved smart
    /// playlist evaluated twice in a run) takes [`Projection::search_at`]
    /// and passes its own timestamp.
    pub fn search(&self, query: &str) -> Vec<u32> {
        self.search_at(query, now_secs())
    }

    /// [`Projection::search`] with the now-timestamp handed in: unix
    /// seconds, what an `added:<90d` term measures its age against.
    pub fn search_at(&self, query: &str, now: i64) -> Vec<u32> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return (0..self.len() as u32).collect();
        }

        /// What one term's row check needs, precomputed off the row scan.
        enum Hits<'a> {
            Any {
                a: Vec<bool>,
                aa: Vec<bool>,
                b: Vec<bool>,
                g: Vec<bool>,
                finder: memmem::Finder<'a>,
            },
            Sym {
                column: &'a [u32],
                mask: Vec<bool>,
            },
            Title(memmem::Finder<'a>),
            Year(Vec<bool>),
            /// A numeric pin: which column to read, and the comparison it
            /// has to satisfy. Nothing to precompute, the columns are
            /// already numbers.
            Num(QueryField, NumTerm),
        }

        let hit = |table: &SymTable, q: &str| -> Vec<bool> {
            table.lower.par_iter().map(|s| s.contains(q)).collect()
        };
        let hits: Vec<Hits> = terms
            .iter()
            .map(|t| match t.field {
                None => Hits::Any {
                    a: hit(&self.artists, &t.needle),
                    aa: hit(&self.album_artists, &t.needle),
                    b: hit(&self.albums, &t.needle),
                    g: hit(&self.genres, &t.needle),
                    finder: memmem::Finder::new(t.needle.as_bytes()),
                },
                Some(QueryField::Artist) => Hits::Sym {
                    column: &self.artist,
                    mask: hit(&self.artists, &t.needle),
                },
                Some(QueryField::AlbumArtist) => Hits::Sym {
                    column: &self.album_artist,
                    mask: hit(&self.album_artists, &t.needle),
                },
                Some(QueryField::Album) => Hits::Sym {
                    column: &self.album,
                    mask: hit(&self.albums, &t.needle),
                },
                Some(QueryField::Genre) => Hits::Sym {
                    column: &self.genre,
                    mask: hit(&self.genres, &t.needle),
                },
                // Folder pins only, never a free term: a bare word would
                // else drag in every track whose path happens to hold it.
                Some(QueryField::Folder) => Hits::Sym {
                    column: &self.folder,
                    mask: hit(&self.folders, &t.needle),
                },
                // Pins only as well: "flac" as a free word is a plausible
                // title or album, and matching the format there would bury
                // it under every lossless file in the library.
                Some(QueryField::Codec) => Hits::Sym {
                    column: &self.codec,
                    mask: hit(&self.codecs, &t.needle),
                },
                Some(QueryField::Title) => Hits::Title(memmem::Finder::new(t.needle.as_bytes())),
                // A year needle matches on the digits, so `year:199`
                // takes the whole decade; the mask covers every u16 once
                // over the shared year arena, so a keystroke never formats
                // 65k fresh Strings.
                Some(QueryField::Year) => {
                    let years = year_strings();
                    Hits::Year(
                        (0..=u16::MAX as usize)
                            .map(|y| years.get(y).contains(&t.needle))
                            .collect(),
                    )
                }
                Some(field @ (QueryField::Rating | QueryField::Plays | QueryField::Added)) => {
                    Hits::Num(field, t.num.unwrap_or(NUM_NEVER))
                }
            })
            .collect();

        self.scan_rows(|i| {
            hits.iter().all(|h| match h {
                Hits::Any {
                    a,
                    aa,
                    b,
                    g,
                    finder,
                } => {
                    a[self.artist[i] as usize]
                        || aa[self.album_artist[i] as usize]
                        || b[self.album[i] as usize]
                        || g[self.genre[i] as usize]
                        || finder.find(self.title_lower.get(i).as_bytes()).is_some()
                }
                Hits::Sym { column, mask } => mask[column[i] as usize],
                Hits::Title(finder) => finder.find(self.title_lower.get(i).as_bytes()).is_some(),
                Hits::Year(mask) => mask[self.year[i] as usize],
                Hits::Num(field, num) => num.holds(match field {
                    QueryField::Rating => rating_stars(self.rating[i].load(Ordering::Relaxed)),
                    QueryField::Plays => self.plays[i].load(Ordering::Relaxed) as i64,
                    // Days since the row was scanned in, so the query
                    // reads forward ("added in the last 90 days") while
                    // the column counts backward. A row with no added
                    // stamp (0) is ancient and drops out of every
                    // recency term, which is the right answer for a
                    // library that never got one.
                    _ => (now - self.added[i]) / 86_400,
                }),
            })
        })
    }

    /// The distinct album artists whose name matches the query, each with a
    /// representative row for the cover and count. For the search's grouped
    /// hits, so typing an artist's name surfaces the artist itself above the
    /// tracks. A term pinned to a track-only field (title, album, genre,
    /// year, codec) excludes every artist, since it can't match an artist
    /// name.
    /// Ordered by name; first-seen row per artist.
    pub fn search_artists(&self, query: &str) -> Vec<ArtistHit> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let matches = |name_lower: &str| {
            terms.iter().all(|t| match t.field {
                None | Some(QueryField::Artist) | Some(QueryField::AlbumArtist) => {
                    name_lower.contains(&t.needle)
                }
                _ => false,
            })
        };
        let mut hits: Vec<ArtistHit> = self
            .distinct_artists()
            .iter()
            .filter(|h| {
                let sym = h.album_artist as usize;
                !self.album_artists.strings[sym].is_empty()
                    && matches(&self.album_artists.lower[sym])
            })
            .copied()
            .collect();
        hits.sort_by(|a, b| {
            self.album_artists.strings[a.album_artist as usize]
                .cmp(&self.album_artists.strings[b.album_artist as usize])
        });
        hits
    }

    /// The distinct albums whose album or album-artist name matches the
    /// query, each keyed by its (album artist, album) pair with a
    /// representative row for the cover and year. A free term matches
    /// either name; `album:` pins the album, `artist:`/`albumartist:` the
    /// artist; a title, genre, year, or codec term excludes every album.
    /// Ordered by artist then album; first-seen row per pair.
    pub fn search_albums(&self, query: &str) -> Vec<AlbumHit> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let matches = |artist_lower: &str, album_lower: &str| {
            terms.iter().all(|t| match t.field {
                None => artist_lower.contains(&t.needle) || album_lower.contains(&t.needle),
                Some(QueryField::Album) => album_lower.contains(&t.needle),
                Some(QueryField::Artist) | Some(QueryField::AlbumArtist) => {
                    artist_lower.contains(&t.needle)
                }
                _ => false,
            })
        };
        let mut hits: Vec<AlbumHit> = self
            .distinct_albums()
            .iter()
            .filter(|h| {
                let album = h.album as usize;
                !self.albums.strings[album].is_empty()
                    && matches(
                        &self.album_artists.lower[h.album_artist as usize],
                        &self.albums.lower[album],
                    )
            })
            .copied()
            .collect();
        hits.sort_by(|a, b| {
            let artist = self.album_artists.strings[a.album_artist as usize]
                .cmp(&self.album_artists.strings[b.album_artist as usize]);
            artist.then_with(|| {
                self.albums.strings[a.album as usize].cmp(&self.albums.strings[b.album as usize])
            })
        });
        hits
    }

    /// The distinct release years present, newest first, zero (unknown)
    /// dropped. The year field has no symbol table to suggest from, so its
    /// value completions draw from this instead.
    pub fn distinct_years(&self) -> Vec<u16> {
        let mut years: Vec<u16> = self.year.iter().copied().filter(|&y| y != 0).collect();
        years.sort_unstable_by(|a, b| b.cmp(a));
        years.dedup();
        years
    }

    /// Row mask for a structured filter: a row passes when, for every
    /// filtered field, its value is one of that field's picks - values OR
    /// within a field, fields AND across. Exact matches against the symbol
    /// tables, never substrings. None when the filter is empty, so callers
    /// skip the scan and the intersection.
    pub fn filter_mask(&self, filter: &FilterSet) -> Option<Vec<bool>> {
        if filter.is_empty() {
            return None;
        }

        /// One field's row check: picked symbols for an interned column,
        /// picked years over every u16 once (the search's year trick).
        enum Check<'a> {
            Sym { column: &'a [u32], ok: Vec<bool> },
            Year(Vec<bool>),
        }

        let fold = self.fold;
        let sym_ok = |table: &SymTable, values: &[String]| -> Vec<bool> {
            table
                .strings
                .iter()
                .map(|s| values.iter().any(|v| crate::value_eq(v, s, fold)))
                .collect()
        };
        let checks: Vec<Check> = filter
            .fields
            .iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(field, values)| match field {
                FilterField::Artist => Check::Sym {
                    column: &self.artist,
                    ok: sym_ok(&self.artists, values),
                },
                FilterField::AlbumArtist => Check::Sym {
                    column: &self.album_artist,
                    ok: sym_ok(&self.album_artists, values),
                },
                FilterField::Album => Check::Sym {
                    column: &self.album,
                    ok: sym_ok(&self.albums, values),
                },
                // Genre symbols are "; " lists; a pick passes any symbol
                // carrying it as one of its values, the same per-symbol
                // trick the folder subtree check plays below.
                FilterField::Genre => Check::Sym {
                    column: &self.genre,
                    ok: self
                        .genres
                        .strings
                        .iter()
                        .map(|s| values.iter().any(|v| crate::genre::has(s, v, fold)))
                        .collect(),
                },
                // Folder picks cover their subtree, so the per-symbol check
                // is a prefix test instead of the exact match.
                FilterField::Folder => Check::Sym {
                    column: &self.folder,
                    ok: self
                        .folders
                        .strings
                        .iter()
                        .map(|s| values.iter().any(|v| folder_in_subtree(s, v)))
                        .collect(),
                },
                FilterField::Year => {
                    let mut ok = vec![false; usize::from(u16::MAX) + 1];
                    for v in values {
                        if let Ok(y) = v.parse::<u16>() {
                            ok[y as usize] = true;
                        }
                    }
                    Check::Year(ok)
                }
            })
            .collect();

        // The id pin goes to a set first: the row scan runs the whole
        // catalog, so a linear lookup per row would make it quadratic.
        let pinned: Option<std::collections::HashSet<i64>> =
            filter.ids.as_ref().map(|ids| ids.iter().copied().collect());

        Some(
            (0..self.len())
                .into_par_iter()
                .map(|i| {
                    if let Some(pinned) = &pinned {
                        if !pinned.contains(&self.db_id[i]) {
                            return false;
                        }
                    }
                    checks.iter().all(|c| match c {
                        Check::Sym { column, ok } => ok[column[i] as usize],
                        Check::Year(ok) => ok[self.year[i] as usize],
                    })
                })
                .collect(),
        )
    }

    /// The rows carrying one genre value, "; " lists included: asking for
    /// "Shoegaze" takes a "Rock; Shoegaze" track along with the plain ones.
    pub fn filter_genre(&self, genre: &str) -> Vec<u32> {
        let ok: Vec<bool> = self
            .genres
            .strings
            .iter()
            .map(|s| crate::genre::has(s, genre, self.fold))
            .collect();
        if !ok.contains(&true) {
            return Vec::new();
        }
        self.scan_rows(|i| ok[self.genre[i] as usize])
    }

    pub fn filter_year(&self, lo: u16, hi: u16) -> Vec<u32> {
        self.scan_rows(|i| (lo..=hi).contains(&self.year[i]))
    }

    /// Parallel predicate scan in fixed chunks; chunk order keeps results in
    /// row order without a sort.
    fn scan_rows(&self, pred: impl Fn(usize) -> bool + Sync) -> Vec<u32> {
        let n = self.len();
        let chunks = n.div_ceil(CHUNK);
        let per: Vec<Vec<u32>> = (0..chunks)
            .into_par_iter()
            .map(|c| {
                let start = c * CHUNK;
                let end = (start + CHUNK).min(n);
                let mut out = Vec::new();
                for i in start..end {
                    if pred(i) {
                        out.push(i as u32);
                    }
                }
                out
            })
            .collect();
        let mut flat = Vec::with_capacity(per.iter().map(Vec::len).sum());
        for v in per {
            flat.extend_from_slice(&v);
        }
        flat
    }

    /// Alphabetical rank per symbol, so sort comparisons are integer, never
    /// string (the non-functional model's precomputed-keys claim).
    fn ranks(table: &SymTable) -> Vec<u32> {
        let mut order: Vec<u32> = (0..table.strings.len() as u32).collect();
        order.par_sort_unstable_by(|&a, &b| table.lower[a as usize].cmp(&table.lower[b as usize]));
        let mut rank = vec![0u32; order.len()];
        for (pos, &sym) in order.iter().enumerate() {
            rank[sym as usize] = pos as u32;
        }
        rank
    }

    // The cached lowered-order ranks per symbol table: ranked once on the first
    // sort that reaches for them, reused after. Every sort's tie-break wants the
    // album artist and album ranks, so this saves re-sorting those tables per
    // sort; the keyed sorts save their own table's rank too.
    fn album_artist_ranks(&self) -> &[u32] {
        self.album_artist_ranks
            .get_or_init(|| Self::ranks(&self.album_artists))
    }
    fn album_ranks(&self) -> &[u32] {
        self.album_ranks.get_or_init(|| Self::ranks(&self.albums))
    }
    fn artist_ranks(&self) -> &[u32] {
        self.artist_ranks.get_or_init(|| Self::ranks(&self.artists))
    }
    fn genre_ranks(&self) -> &[u32] {
        self.genre_ranks.get_or_init(|| Self::ranks(&self.genres))
    }
    fn codec_ranks(&self) -> &[u32] {
        self.codec_ranks.get_or_init(|| Self::ranks(&self.codecs))
    }

    /// The distinct album artists in first-seen row order, cached. The
    /// per-query search_artists filters these by name, so the O(rows) distinct
    /// pass happens once instead of every keystroke.
    fn distinct_artists(&self) -> &[ArtistHit] {
        self.distinct_artists.get_or_init(|| {
            let mut seen: HashSet<u32> = HashSet::new();
            let mut out: Vec<ArtistHit> = Vec::new();
            for row in 0..self.len() as u32 {
                let sym = self.album_artist[row as usize];
                if seen.insert(sym) {
                    out.push(ArtistHit {
                        album_artist: sym,
                        row,
                    });
                }
            }
            out
        })
    }

    /// The distinct genre values across the library, "; " lists split
    /// into their parts, each once in first-seen order with the lowered
    /// copy suggestion filtering wants. A folded library merges case
    /// variants here too, the display going to the casing the most rows
    /// carry - the symbols only folded whole strings, so parts shared
    /// across different lists still need their own pass.
    pub fn genre_terms(&self) -> &SymTable {
        self.genre_terms.get_or_init(|| {
            if !self.fold {
                let mut seen: HashSet<String> = HashSet::new();
                let mut strings: Vec<String> = Vec::new();
                for s in &self.genres.strings {
                    for part in crate::genre::split(s) {
                        let part = crate::genre::resolve(part);
                        if seen.insert(part.clone()) {
                            strings.push(part);
                        }
                    }
                }
                let lower = strings.iter().map(|s| s.to_lowercase()).collect();
                return SymTable { strings, lower };
            }
            let mut rows = vec![0u32; self.genres.strings.len()];
            for &sym in &self.genre {
                rows[sym as usize] += 1;
            }
            // Folded part -> (first-seen order, casing -> row count).
            let mut order: Vec<String> = Vec::new();
            let mut casings: HashMap<String, HashMap<String, u32>> = HashMap::new();
            for (sym, s) in self.genres.strings.iter().enumerate() {
                for part in crate::genre::split(s) {
                    let part = crate::genre::resolve(part);
                    let key = part.to_lowercase();
                    let entry = casings.entry(key.clone()).or_insert_with(|| {
                        order.push(key);
                        HashMap::new()
                    });
                    *entry.entry(part).or_default() += rows[sym];
                }
            }
            let strings: Vec<String> = order
                .iter()
                .map(|key| {
                    casings[key]
                        .iter()
                        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                        .map(|(s, _)| s.to_string())
                        .expect("every ordered key has at least one casing")
                })
                .collect();
            let lower = strings.iter().map(|s| s.to_lowercase()).collect();
            SymTable { strings, lower }
        })
    }

    /// The distinct (album artist, album) pairs in first-seen row order,
    /// cached. The per-query search_albums filters these, moving the distinct
    /// pass off the keystroke path the same way distinct_artists does.
    fn distinct_albums(&self) -> &[AlbumHit] {
        self.distinct_albums.get_or_init(|| {
            let mut seen: HashSet<u64> = HashSet::new();
            let mut out: Vec<AlbumHit> = Vec::new();
            for row in 0..self.len() as u32 {
                let i = row as usize;
                let album_artist = self.album_artist[i];
                let album = self.album[i];
                let key = (album_artist as u64) << 32 | album as u64;
                if seen.insert(key) {
                    out.push(AlbumHit {
                        album_artist,
                        album,
                        row,
                    });
                }
            }
            out
        })
    }

    /// The canonical browse order: album artist, album, disc, track number.
    /// The album artist keys it so an album's tracks stay one run under its
    /// credited artist, per-track guests and all; the disc keys ahead of the
    /// track so a multi-disc set plays through in order instead of
    /// interleaving its discs' track numbers.
    pub fn sort_canonical(&self) -> Vec<u32> {
        let a_rank = self.album_artist_ranks();
        let b_rank = self.album_ranks();
        let mut idx: Vec<u32> = (0..self.len() as u32).collect();
        idx.par_sort_unstable_by_key(|&i| {
            let i = i as usize;
            (
                a_rank[self.album_artist[i] as usize],
                b_rank[self.album[i] as usize],
                self.disc_no[i],
                self.track_no[i],
            )
        });
        idx
    }

    pub fn sort_title(&self) -> Vec<u32> {
        let mut idx: Vec<u32> = (0..self.len() as u32).collect();
        idx.par_sort_unstable_by(|&a, &b| {
            self.title_lower
                .get(a as usize)
                .cmp(self.title_lower.get(b as usize))
        });
        idx
    }

    pub fn sort_year(&self) -> Vec<u32> {
        let mut idx: Vec<u32> = (0..self.len() as u32).collect();
        idx.par_sort_unstable_by_key(|&i| self.year[i as usize]);
        idx
    }

    /// Sort a view - any subset of rows, in any order - by one key. Ties
    /// fall back to the canonical artist, album, track order so equal keys
    /// stay browsable; descending reverses the key alone, not the
    /// tie-break.
    pub fn sort_view(&self, view: &[u32], key: SortKey, descending: bool) -> Vec<u32> {
        match key {
            SortKey::Title => self.order_view(view, descending, |i| self.title_lower.get(i)),
            SortKey::Artist => {
                let rank = self.artist_ranks();
                self.order_view(view, descending, move |i| rank[self.artist[i] as usize])
            }
            SortKey::AlbumArtist => {
                let rank = self.album_artist_ranks();
                self.order_view(view, descending, move |i| {
                    rank[self.album_artist[i] as usize]
                })
            }
            SortKey::Album => {
                let rank = self.album_ranks();
                self.order_view(view, descending, move |i| rank[self.album[i] as usize])
            }
            SortKey::Genre => {
                let rank = self.genre_ranks();
                self.order_view(view, descending, move |i| rank[self.genre[i] as usize])
            }
            SortKey::Year => self.order_view(view, descending, |i| self.year[i]),
            SortKey::TrackNo => self.order_view(view, descending, |i| self.track_no[i]),
            SortKey::Duration => self.order_view(view, descending, |i| self.duration_ms[i]),
            SortKey::Codec => {
                let rank = self.codec_ranks();
                self.order_view(view, descending, move |i| rank[self.codec[i] as usize])
            }
            SortKey::Bitrate => self.order_view(view, descending, |i| self.bitrate_kbps[i]),
            SortKey::SampleRate => self.order_view(view, descending, |i| self.sample_rate_hz[i]),
            SortKey::BitDepth => self.order_view(view, descending, |i| self.bit_depth[i]),
            SortKey::Rating => {
                self.order_view(view, descending, |i| self.rating[i].load(Ordering::Relaxed))
            }
            SortKey::Plays => {
                self.order_view(view, descending, |i| self.plays[i].load(Ordering::Relaxed))
            }
            SortKey::Added => self.order_view(view, descending, |i| self.added[i]),
            // Packed centi-dB sorts as-is, and NO_GAIN being the floor puts
            // the untagged rows first ascending, where a zero year or an
            // unrated track sits too.
            SortKey::TrackGain => self.order_view(view, descending, |i| self.gain_key(i, false)),
            SortKey::AlbumGain => self.order_view(view, descending, |i| self.gain_key(i, true)),
            // Packed centi-bpm sorts as-is, and NO_BPM being zero puts the
            // tracks with no tempo first ascending, where the untagged
            // gains and the unrated tracks sit too.
            SortKey::Bpm => self.order_view(view, descending, |i| self.bpm[i]),
        }
    }

    /// One row's leveling gain in dB: the mode's own figure, the other as
    /// the fallback, None for a file carrying neither. `album_first` is the
    /// Album mode. Same pick [`crate::replaygain`] hands the engine, so the
    /// number in the column is the one playback would act on, before the
    /// preamp and the peak clamp.
    pub fn gain_db(&self, row: u32, album_first: bool) -> Option<f32> {
        unpack_gain(self.gain_key(row as usize, album_first))
    }

    /// The same pick, still packed, for sorting.
    fn gain_key(&self, i: usize, album_first: bool) -> i16 {
        let (first, second) = if album_first {
            (self.album_gain[i], self.track_gain[i])
        } else {
            (self.track_gain[i], self.album_gain[i])
        };
        if first != NO_GAIN {
            first
        } else {
            second
        }
    }

    /// The shared sort skeleton behind [`Self::sort_view`]: primary key,
    /// direction, canonical tie-break, all on precomputed integer ranks
    /// except titles, which compare their lowered strings directly - a
    /// subset comparison stays cheaper than ranking every title.
    fn order_view<K, F>(&self, view: &[u32], descending: bool, primary: F) -> Vec<u32>
    where
        K: Ord,
        F: Fn(usize) -> K + Sync,
    {
        let a_rank = self.album_artist_ranks();
        let b_rank = self.album_ranks();
        let canonical = |i: usize| {
            (
                a_rank[self.album_artist[i] as usize],
                b_rank[self.album[i] as usize],
                self.disc_no[i],
                self.track_no[i],
            )
        };
        let mut idx = view.to_vec();
        idx.par_sort_unstable_by(|&a, &b| {
            let (a, b) = (a as usize, b as usize);
            let ord = primary(a).cmp(&primary(b));
            let ord = if descending { ord.reverse() } else { ord };
            ord.then_with(|| canonical(a).cmp(&canonical(b)))
        });
        idx
    }

    pub fn heap_bytes(&self) -> usize {
        (self.db_id.capacity() + self.added.capacity()) * 8
            + self.title.heap_bytes()
            + self.title_lower.heap_bytes()
            + (self.artist.capacity()
                + self.album_artist.capacity()
                + self.album.capacity()
                + self.genre.capacity()
                + self.codec.capacity()
                + self.folder.capacity())
                * 4
            + (self.year.capacity()
                + self.disc_no.capacity()
                + self.track_no.capacity()
                + self.bitrate_kbps.capacity())
                * 2
            + (self.duration_ms.capacity() + self.sample_rate_hz.capacity()) * 4
            + self.rating.capacity()
            + self.bit_depth.capacity()
            + self.plays.capacity() * 4
            + self.artists.heap_bytes()
            + self.album_artists.heap_bytes()
            + self.albums.heap_bytes()
            + self.genres.heap_bytes()
            + self.codecs.heap_bytes()
            + self.folders.heap_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{listens, TrackRow};

    fn row(path: &str, album: &str, disc_no: u16, track_no: u16) -> TrackRow {
        TrackRow {
            sub: 0,
            cue: None,
            path: path.into(),
            title: String::new(),
            artist: String::new(),
            album_artist: "Various Artists".into(),
            album: album.into(),
            genre: String::new(),
            year: 0,
            disc_no,
            track_no,
            duration_ms: 0,
            codec: String::new(),
            bitrate_kbps: 0,
            sample_rate_hz: 0,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    fn track(path: &str, title: &str, artist: &str, year: u16) -> TrackRow {
        TrackRow {
            sub: 0,
            cue: None,
            path: path.into(),
            title: title.into(),
            artist: artist.into(),
            album_artist: String::new(),
            album: String::new(),
            genre: String::new(),
            year,
            disc_no: 0,
            track_no: 0,
            duration_ms: 0,
            codec: String::new(),
            bitrate_kbps: 0,
            sample_rate_hz: 0,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    fn titles_for(p: &Projection, query: &str) -> Vec<String> {
        p.search(query)
            .iter()
            .map(|&i| p.title.get(i as usize).to_string())
            .collect()
    }

    #[test]
    fn query_parses_free_and_pinned_terms() {
        let terms = parse_query(r#"stronger artist:"Daft Punk" ac:dc year:199"#);
        assert_eq!(terms.len(), 4);
        assert_eq!(
            (terms[0].field, terms[0].needle.as_str()),
            (None, "stronger")
        );
        assert_eq!(
            (terms[1].field, terms[1].needle.as_str()),
            (Some(QueryField::Artist), "daft punk")
        );
        // An unknown prefix stays free text, colon and all.
        assert_eq!((terms[2].field, terms[2].needle.as_str()), (None, "ac:dc"));
        assert_eq!(
            (terms[3].field, terms[3].needle.as_str()),
            (Some(QueryField::Year), "199")
        );
    }

    /// The numeric pins parse into a comparison and a number: every
    /// operator, the bare form meaning equals, and the `added:` day suffix.
    #[test]
    fn query_parses_numeric_terms() {
        let cases = [
            ("rating:>=4", QueryField::Rating, NumOp::Ge, 4),
            ("rating:3", QueryField::Rating, NumOp::Eq, 3),
            ("rating:=5", QueryField::Rating, NumOp::Eq, 5),
            ("rating:<=2", QueryField::Rating, NumOp::Le, 2),
            ("plays:0", QueryField::Plays, NumOp::Eq, 0),
            ("plays:>10", QueryField::Plays, NumOp::Gt, 10),
            ("added:<90d", QueryField::Added, NumOp::Lt, 90),
            // The day suffix is optional, and a quoted value survives the
            // tokenizer the same way a quoted artist does.
            ("added:>7", QueryField::Added, NumOp::Gt, 7),
            (r#"rating:">= 4""#, QueryField::Rating, NumOp::Ge, 4),
        ];
        for (query, field, op, value) in cases {
            let terms = parse_query(query);
            assert_eq!(terms.len(), 1, "{query} is one term");
            assert_eq!(terms[0].field, Some(field), "{query} pins its field");
            assert_eq!(
                terms[0].num,
                Some(NumTerm { op, value }),
                "{query} carries its comparison"
            );
        }
    }

    /// An operator only means something on a numeric field. On a text one
    /// it stays literal, and a numeric pin with nothing numeric behind it
    /// drops back to a free term, the rule an unknown prefix follows.
    #[test]
    fn operators_stay_literal_off_the_numeric_fields() {
        let terms = parse_query("year:>1990");
        assert_eq!(terms[0].field, Some(QueryField::Year));
        assert_eq!(terms[0].needle, ">1990");
        assert_eq!(terms[0].num, None);

        let terms = parse_query("rating:great");
        assert_eq!(
            (terms[0].field, terms[0].needle.as_str()),
            (None, "rating:great"),
            "an unparseable number reads as free text, colon and all"
        );
    }

    /// The numeric columns the projection carries, compared the way the
    /// query spells them: whole stars for a rating, the raw count for
    /// plays, and an age in days for added, resolved against a timestamp
    /// the caller passes so the test has no clock in it.
    #[test]
    fn numeric_pins_compare_the_projection_columns() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let mut rated = track("/m/1.mp3", "Loved", "A", 2001);
        rated.rating = 100;
        let mut liked = track("/m/2.mp3", "Liked", "B", 2002);
        liked.rating = 80;
        let plain = track("/m/3.mp3", "Plain", "C", 2003);
        store::insert_batch(&mut conn, &[rated, liked, plain]).unwrap();

        // A fixed now, and added stamps a known distance behind it.
        let now = 1_700_000_000;
        let day = 86_400;
        conn.execute(
            "UPDATE tracks SET added = ?1 WHERE id = 1",
            [now - day * 10],
        )
        .unwrap();
        conn.execute(
            "UPDATE tracks SET added = ?1 WHERE id IN (2, 3)",
            [now - day * 400],
        )
        .unwrap();
        // One listen on the middle track, so the play counts differ.
        listens::append(
            &conn,
            &listens::Listen {
                track_id: 2,
                played_at: now,
                title: "Liked".into(),
                artist: "B".into(),
                album: String::new(),
                genre: String::new(),
                path: "/m/2.mp3".into(),
            },
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        let titles = |query: &str| -> Vec<String> {
            p.search_at(query, now)
                .iter()
                .map(|&i| p.title.get(i as usize).to_string())
                .collect()
        };
        assert_eq!(titles("rating:>=4"), ["Loved", "Liked"]);
        assert_eq!(titles("rating:5"), ["Loved"]);
        assert_eq!(titles("rating:0"), ["Plain"], "unrated is zero stars");
        assert_eq!(titles("plays:0"), ["Loved", "Plain"]);
        assert_eq!(titles("plays:>0"), ["Liked"]);
        assert_eq!(titles("added:<90d"), ["Loved"]);
        assert_eq!(titles("added:>=90d"), ["Liked", "Plain"]);
        // Terms still AND, numeric beside text.
        assert_eq!(titles("rating:>=4 plays:0"), ["Loved"]);
    }

    /// A pinned term narrows to its field only, and terms AND together,
    /// so a title term plus an artist term takes one artist's version.
    #[test]
    fn search_pins_terms_to_fields() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "Stronger", "Kanye West", 2007),
                track("/m/2.mp3", "Stronger", "Daft Punk", 2001),
                track("/m/3.mp3", "Daft Punk Tribute", "Nobody", 2010),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        // Free text still matches across fields.
        assert_eq!(titles_for(&p, "daft").len(), 2);
        // Pinned to the artist, the tribute title no longer hits.
        let hits = p.search(r#"stronger artist:"daft punk""#);
        assert_eq!(hits.len(), 1);
        assert_eq!(p.resolve(hits[0]).artist, "Daft Punk");
        // A year needle matches on the digits.
        assert_eq!(titles_for(&p, "year:200").len(), 2);
        assert_eq!(titles_for(&p, "stronger year:2007").len(), 1);
    }

    /// A row that names its own fields runs both matchers off one impl,
    /// which is what the queue and the playlists tree share. An off-catalog
    /// row carries no db id, so an id pin leaves it out rather than folding
    /// every such row onto a shared phantom id.
    #[test]
    fn a_filterable_row_runs_both_matchers() {
        struct Queued {
            id: Option<i64>,
            title: String,
            artist: String,
        }

        impl Filterable for Queued {
            fn fields(&self) -> TrackFields<'_> {
                TrackFields {
                    db_id: self.id,
                    title: &self.title,
                    artist: &self.artist,
                    album_artist: &self.artist,
                    album: "",
                    genre: "",
                    year: 0,
                    codec: "flac",
                    path: "/m/x.flac",
                }
            }
        }

        let known = Queued {
            id: Some(7),
            title: "Stronger".into(),
            artist: "Daft Punk".into(),
        };
        let dropped = Queued {
            id: None,
            title: "Stronger".into(),
            artist: "Daft Punk".into(),
        };
        let none = FilterSet::default();
        let terms = parse_query("stronger");
        assert!(known.passes(&terms, &none, false));
        assert!(dropped.passes(&terms, &none, false));
        // The text half still has to hold.
        assert!(!known.passes(&parse_query("harder"), &none, false));

        // An id pin takes the catalog row and leaves the dropped file out.
        let pinned = FilterSet::with_ids(vec![7]);
        assert!(known.passes(&terms, &pinned, false));
        assert!(!dropped.passes(&terms, &pinned, false));
    }

    /// The per-track matcher the queue, history, and playlists filter their
    /// own rows with agrees with `search` over the catalog: free terms sweep
    /// the text fields, pins isolate one, and the structured filter matches
    /// whole values.
    #[test]
    fn track_matcher_mirrors_search() {
        let fields = TrackFields {
            db_id: Some(1),
            title: "Stronger",
            artist: "Daft Punk",
            album_artist: "Daft Punk",
            album: "Discovery",
            genre: "Electronic",
            year: 2001,
            codec: "flac",
            path: "/music/Discovery/1.mp3",
        };
        // Free text sweeps title, artist, album, genre; case-folded.
        assert!(track_matches(&parse_query("stronger"), &fields));
        assert!(track_matches(&parse_query("DAFT"), &fields));
        assert!(track_matches(&parse_query("electronic"), &fields));
        assert!(!track_matches(&parse_query("kanye"), &fields));
        // Every term must hit.
        assert!(track_matches(&parse_query("stronger daft"), &fields));
        assert!(!track_matches(&parse_query("stronger kanye"), &fields));
        // Pins isolate their field; a title term never matches the artist.
        assert!(track_matches(
            &parse_query(r#"artist:"daft punk""#),
            &fields
        ));
        assert!(!track_matches(&parse_query("title:discovery"), &fields));
        // Year matches on the digits; folder pins to the parent directory.
        assert!(track_matches(&parse_query("year:200"), &fields));
        assert!(track_matches(&parse_query("folder:discovery"), &fields));
        assert!(!track_matches(&parse_query("folder:other"), &fields));
        // Codec pins to the file's format, and stays out of free terms.
        assert!(track_matches(&parse_query("codec:FLAC"), &fields));
        assert!(!track_matches(&parse_query("codec:mp3"), &fields));
        assert!(!track_matches(&parse_query("flac"), &fields));

        // The structured filter matches whole values, never substrings.
        let mut filter = FilterSet::default();
        filter.toggle(FilterField::Artist, "Daft Punk");
        assert!(filter.matches(&fields, false));
        let mut narrower = filter.clone();
        narrower.toggle(FilterField::Artist, "Air");
        // Values OR within a field, so the extra pick still passes.
        assert!(narrower.matches(&fields, false));
        let mut year = FilterSet::default();
        year.toggle(FilterField::Year, "2001");
        assert!(year.matches(&fields, false));
        year.clear(FilterField::Year);
        year.toggle(FilterField::Year, "1999");
        assert!(!year.matches(&fields, false));
    }

    /// `folder:` pins a term to the track's parent directory, case-folded
    /// substring like the other pinned fields, so it isolates one album's
    /// files. A bare word never reaches the folder, so the path text stays
    /// out of free-term matches.
    #[test]
    fn search_pins_folder() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/music/Wrong Album/1.mp3", "One", "A", 2000),
                track("/music/Wrong Album/2.mp3", "Two", "A", 2000),
                track("/music/Other/3.mp3", "Three", "B", 2001),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        // The pin isolates just the one folder's files, case-folded.
        assert_eq!(titles_for(&p, r#"folder:"wrong album""#).len(), 2);
        // The substring takes the folder, not the whole path, so "music"
        // matches every track under the shared root.
        assert_eq!(titles_for(&p, "folder:music").len(), 3);
        // A folder pin ANDs with a free term like any other field.
        assert_eq!(titles_for(&p, r#"one folder:"wrong album""#).len(), 1);
        // A bare word never reaches the folder path.
        assert!(titles_for(&p, "other").is_empty());
    }

    /// The stream numbers survive the store round trip and sort as
    /// numbers, which is what the kHz and Bits columns browse on. The
    /// parallel load merges shards, so it has to agree with the serial
    /// one row for row.
    #[test]
    fn stream_format_columns_load_and_sort() {
        let dir = std::env::temp_dir().join("rox-projection-stream-format");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.db");
        let mut conn = store::open(&db).unwrap();
        store::init_schema(&conn).unwrap();
        let encoded = |path, title, hz, bits| {
            let mut row = track(path, title, "A", 2000);
            row.sample_rate_hz = hz;
            row.bit_depth = bits;
            row
        };
        store::insert_batch(
            &mut conn,
            &[
                encoded("/m/1.flac", "Hi Res", 96000, 24),
                encoded("/m/2.mp3", "Lossy", 44100, 0),
                encoded("/m/3.flac", "CD", 44100, 16),
            ],
        )
        .unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        let by_title = |title: &str| {
            let row = (0..p.len()).find(|&i| p.title.get(i) == title).unwrap();
            let v = p.resolve(row as u32);
            (v.sample_rate_hz, v.bit_depth)
        };
        assert_eq!(by_title("Hi Res"), (96000, 24));
        assert_eq!(by_title("Lossy"), (44100, 0));
        assert_eq!(by_title("CD"), (44100, 16));

        // Sorting runs over the plain columns, ascending by depth then
        // by rate.
        let view: Vec<u32> = (0..p.len() as u32).collect();
        let titles = |order: Vec<u32>| -> Vec<String> {
            order
                .iter()
                .map(|&i| p.title.get(i as usize).to_string())
                .collect()
        };
        assert_eq!(
            titles(p.sort_view(&view, SortKey::BitDepth, false)),
            ["Lossy", "CD", "Hi Res"]
        );
        assert_eq!(
            titles(p.sort_view(&view, SortKey::SampleRate, true))[0],
            "Hi Res"
        );

        // The sharded load merges to the same rows.
        let parallel = Projection::load_parallel(&db, 3, false).unwrap();
        assert_eq!(parallel.sample_rate_hz, p.sample_rate_hz);
        assert_eq!(parallel.bit_depth, p.bit_depth);
    }

    /// The ReplayGain figures load into the projection, come back as the
    /// dB the file carries, and sort by whichever one the leveling mode
    /// reads. A file tagged only one way is read by the other mode too,
    /// the same fallback the engine levels by, and one carrying neither
    /// stays None instead of a zero that would read as levelled.
    #[test]
    fn replay_gain_loads_and_sorts_by_the_mode() {
        let dir = std::env::temp_dir().join("rox-projection-replay-gain");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.db");
        let mut conn = store::open(&db).unwrap();
        store::init_schema(&conn).unwrap();
        let levelled = |path, title, track_db, album_db| {
            let mut row = track(path, title, "A", 2000);
            row.replay_gain = crate::replaygain::ReplayGain {
                track_db,
                track_peak: None,
                album_db,
                album_peak: None,
            };
            row
        };
        store::insert_batch(
            &mut conn,
            &[
                levelled("/m/1.flac", "Loud", Some(-9.55), Some(-8.10)),
                levelled("/m/2.flac", "Quiet", Some(-2.40), Some(-8.10)),
                levelled("/m/3.flac", "Album Only", None, Some(-5.00)),
                levelled("/m/4.flac", "Untagged", None, None),
            ],
        )
        .unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        let row_of = |title: &str| (0..p.len()).find(|&i| p.title.get(i) == title).unwrap() as u32;
        assert_eq!(p.resolve(row_of("Loud")).track_gain_db, Some(-9.55));
        assert_eq!(p.resolve(row_of("Loud")).album_gain_db, Some(-8.10));
        assert_eq!(p.resolve(row_of("Untagged")).track_gain_db, None);

        // Track mode reads the track figure and falls back to the album
        // one; album mode the other way round.
        assert_eq!(p.gain_db(row_of("Loud"), false), Some(-9.55));
        assert_eq!(p.gain_db(row_of("Loud"), true), Some(-8.10));
        assert_eq!(p.gain_db(row_of("Album Only"), false), Some(-5.00));
        assert_eq!(p.gain_db(row_of("Untagged"), true), None);

        let view: Vec<u32> = (0..p.len() as u32).collect();
        let titles = |order: Vec<u32>| -> Vec<String> {
            order
                .iter()
                .map(|&i| p.title.get(i as usize).to_string())
                .collect()
        };
        // Ascending is quietest master first, the untagged row ahead of
        // them all the way a zero year sorts.
        assert_eq!(
            titles(p.sort_view(&view, SortKey::TrackGain, false)),
            ["Untagged", "Loud", "Album Only", "Quiet"]
        );
        // By album gain the two sharing a record tie and fall to the
        // canonical order, and Album Only reads its own.
        assert_eq!(
            titles(p.sort_view(&view, SortKey::AlbumGain, false)),
            ["Untagged", "Loud", "Quiet", "Album Only"]
        );

        // The sharded load merges to the same columns.
        let parallel = Projection::load_parallel(&db, 3, false).unwrap();
        assert_eq!(parallel.track_gain, p.track_gain);
        assert_eq!(parallel.album_gain, p.album_gain);
    }

    /// The tempo loads into the projection, comes back as the beats a
    /// minute the row holds, and carries which source filled it so a display
    /// can mark an estimate. A row with no tempo stays None rather than
    /// reading as a track that stands still.
    #[test]
    fn tempo_loads_with_the_source_that_filled_it() {
        let dir = std::env::temp_dir().join("rox-projection-tempo");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.db");
        let mut conn = store::open(&db).unwrap();
        store::init_schema(&conn).unwrap();
        let at = |path, title, bpm| {
            let mut row = track(path, title, "A", 2000);
            row.bpm = bpm;
            row
        };
        store::insert_batch(
            &mut conn,
            &[
                at("/m/1.flac", "Tagged", Some(174.0)),
                at("/m/2.flac", "Fractional", Some(128.25)),
                at("/m/3.flac", "Untagged", None),
            ],
        )
        .unwrap();
        store::set_measured_bpm(&mut conn, &[("/m/3.flac", 0, 92.5)]).unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        let row_of = |title: &str| (0..p.len()).find(|&i| p.title.get(i) == title).unwrap() as u32;
        assert_eq!(p.resolve(row_of("Tagged")).bpm, Some(174.0));
        assert_eq!(
            p.resolve(row_of("Tagged")).bpm_source,
            crate::tempo::Source::Tags
        );
        // The packing holds a fraction of a beat exactly.
        assert_eq!(p.resolve(row_of("Fractional")).bpm, Some(128.25));
        let estimated = p.resolve(row_of("Untagged"));
        assert_eq!(estimated.bpm, Some(92.5));
        assert_eq!(estimated.bpm_source, crate::tempo::Source::Measured);

        // A tempo outside what the store will hold reads as none rather than
        // as a number to sort or mix by.
        assert_eq!(pack_bpm(Some(0.0)), NO_BPM);
        assert_eq!(pack_bpm(Some(900.0)), NO_BPM);
        assert_eq!(pack_bpm(None), NO_BPM);
        assert_eq!(unpack_bpm(NO_BPM), None);

        // The sharded load merges to the same columns.
        let parallel = Projection::load_parallel(&db, 3, false).unwrap();
        assert_eq!(parallel.bpm, p.bpm);
        assert_eq!(parallel.bpm_source, p.bpm_source);
    }

    /// The BPM column sorts slowest first, with the tracks nothing has a
    /// tempo for ahead of them: the packed zero is the floor, which is where
    /// an untagged gain and an unrated track sit too.
    #[test]
    fn tempo_sorts_slowest_first_with_the_untimed_ahead() {
        let dir = std::env::temp_dir().join("rox-projection-tempo-sort");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.db");
        let mut conn = store::open(&db).unwrap();
        store::init_schema(&conn).unwrap();
        let at = |path, title, bpm| {
            let mut row = track(path, title, "A", 2000);
            row.bpm = bpm;
            row
        };
        store::insert_batch(
            &mut conn,
            &[
                at("/m/1.flac", "Fast", Some(174.0)),
                at("/m/2.flac", "Slow", Some(90.0)),
                at("/m/3.flac", "Untimed", None),
            ],
        )
        .unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        let view: Vec<u32> = (0..p.len() as u32).collect();
        let titles = |order: Vec<u32>| -> Vec<String> {
            order
                .iter()
                .map(|&i| p.title.get(i as usize).to_string())
                .collect()
        };
        assert_eq!(
            titles(p.sort_view(&view, SortKey::Bpm, false)),
            ["Untimed", "Slow", "Fast"]
        );
        assert_eq!(
            titles(p.sort_view(&view, SortKey::Bpm, true)),
            ["Fast", "Slow", "Untimed"]
        );
    }

    /// `codec:` pins a term to the file's format, so one term narrows a
    /// query to the lossless copies. Case-folded like the other pins, and
    /// pin-only for the same reason `folder:` is: "flac" typed bare is a
    /// plausible title, and matching the format there would bury it.
    #[test]
    fn search_pins_codec() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let encoded = |path, title, codec: &str| {
            let mut row = track(path, title, "A", 2000);
            row.codec = codec.into();
            row
        };
        store::insert_batch(
            &mut conn,
            &[
                encoded("/m/1.flac", "One", "flac"),
                encoded("/m/2.mp3", "Two", "mp3"),
                encoded("/m/3.mp3", "Flac Tribute", "mp3"),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        assert_eq!(titles_for(&p, "codec:flac"), ["One"]);
        assert_eq!(titles_for(&p, "codec:MP3").len(), 2);
        // A codec pin ANDs with a free term like any other field.
        assert_eq!(titles_for(&p, "tribute codec:mp3"), ["Flac Tribute"]);
        // A bare word only reaches the text fields.
        assert_eq!(titles_for(&p, "flac"), ["Flac Tribute"]);
    }

    /// A folder pick covers its subtree: the folder itself and every
    /// descendant, bounded at a separator so a sibling sharing the prefix
    /// stays out. One value scopes a whole branch, which is what keeps the
    /// folder tree's click cheap.
    #[test]
    fn folder_filter_scopes_subtree() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/music/Air/1.mp3", "One", "A", 2000),
                track("/music/Air/Moon Safari/2.mp3", "Two", "A", 1998),
                track("/music/Airborne/3.mp3", "Three", "B", 2001),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        let mut filter = FilterSet::default();
        filter.toggle(FilterField::Folder, "/music/Air");
        let mask = p.filter_mask(&filter).unwrap();
        // The folder and its nested album pass; the prefix-sharing sibling
        // does not.
        let hits: Vec<String> = (0..p.len())
            .filter(|&i| mask[i])
            .map(|i| p.title.get(i).to_string())
            .collect();
        assert_eq!(hits, ["One", "Two"]);

        // The per-track matcher agrees.
        let fields = |path| TrackFields {
            db_id: Some(1),
            title: "",
            artist: "",
            album_artist: "",
            album: "",
            genre: "",
            year: 0,
            codec: "",
            path,
        };
        assert!(filter.matches(&fields("/music/Air/Moon Safari/2.mp3"), false));
        assert!(!filter.matches(&fields("/music/Airborne/3.mp3"), false));
    }

    /// Genre picks match values inside "; " lists: the mask, the
    /// per-track matcher, filter_genre, and the suggestion terms all
    /// split the same way, and the empty pick keeps its untagged bucket.
    #[test]
    fn genre_filter_splits_lists() {
        fn genre_track(path: &str, title: &str, genre: &str) -> TrackRow {
            let mut row = track(path, title, "A", 2000);
            row.genre = genre.into();
            row
        }
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                genre_track("/m/1.mp3", "One", "Rock; Shoegaze"),
                genre_track("/m/2.mp3", "Two", "Rock"),
                genre_track("/m/3.mp3", "Three", "Electronic"),
                genre_track("/m/4.mp3", "Four", ""),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        let hits = |filter: &FilterSet| -> Vec<String> {
            let mask = p.filter_mask(filter).unwrap();
            (0..p.len())
                .filter(|&i| mask[i])
                .map(|i| p.title.get(i).to_string())
                .collect()
        };
        let mut filter = FilterSet::default();
        filter.toggle(FilterField::Genre, "Shoegaze");
        assert_eq!(hits(&filter), ["One"]);
        // Values OR within the field, lists and plain symbols alike.
        filter.toggle(FilterField::Genre, "Electronic");
        assert_eq!(hits(&filter), ["One", "Three"]);
        // The empty pick is the untagged bucket, not a substring of all.
        let mut unknown = FilterSet::default();
        unknown.toggle(FilterField::Genre, "");
        assert_eq!(hits(&unknown), ["Four"]);

        // The per-track matcher agrees with the mask.
        let fields = TrackFields {
            db_id: Some(1),
            title: "One",
            artist: "A",
            album_artist: "A",
            album: "",
            genre: "Rock; Shoegaze",
            year: 2000,
            codec: "mp3",
            path: "/m/1.mp3",
        };
        assert!(filter.matches(&fields, false));
        assert!(!unknown.matches(&fields, false));

        // filter_genre takes list members; the terms table splits them.
        assert_eq!(p.filter_genre("Rock").len(), 2);
        assert!(p.filter_genre("Rock; Shoegaze").is_empty());
        assert_eq!(p.genre_terms().strings, ["Rock", "Shoegaze", "Electronic"]);
    }

    /// A folded load merges values differing only by case into one
    /// symbol whose display is the casing most rows carry, picks match
    /// across casings, and the genre terms fold their parts the same
    /// way. An exact load keeps the variants apart.
    #[test]
    fn folded_load_merges_case_variants() {
        fn full(path: &str, artist: &str, genre: &str) -> TrackRow {
            let mut row = track(path, "T", artist, 2000);
            row.genre = genre.into();
            row
        }
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                full("/m/1.mp3", "Daft Punk", "Rock; Pop"),
                full("/m/2.mp3", "Daft Punk", "rock"),
                full("/m/3.mp3", "daft punk", "rock"),
            ],
        )
        .unwrap();

        let exact = Projection::load_serial(&conn, false).unwrap();
        assert_eq!(exact.artists.strings.len(), 2, "exact keeps casings apart");

        let folded = Projection::load_serial(&conn, true).unwrap();
        assert_eq!(
            folded.artists.strings,
            ["Daft Punk"],
            "one symbol, the majority casing"
        );
        // Two rows say "rock", one "Rock; Pop": the whole-string symbols
        // stay distinct, but the terms fold to two values and the casing
        // with more rows wins the display.
        assert_eq!(folded.genre_terms().strings, ["rock", "Pop"]);

        // A pick in either casing takes all three rows through the mask,
        // and the per-track matcher agrees over raw row strings.
        let mut filter = FilterSet::default();
        filter.toggle(FilterField::Artist, "daft punk");
        let mask = folded.filter_mask(&filter).unwrap();
        assert_eq!(mask.iter().filter(|&&b| b).count(), 3);
        let fields = TrackFields {
            db_id: Some(1),
            title: "T",
            artist: "DAFT PUNK",
            album_artist: "",
            album: "",
            genre: "ROCK",
            year: 2000,
            codec: "mp3",
            path: "/m/1.mp3",
        };
        assert!(filter.matches(&fields, true));
        assert!(!filter.matches(&fields, false));
        let mut genre_pick = FilterSet::default();
        genre_pick.toggle(FilterField::Genre, "Rock");
        let mask = folded.filter_mask(&genre_pick).unwrap();
        assert_eq!(mask.iter().filter(|&&b| b).count(), 3);
        assert_eq!(folded.filter_genre("POP").len(), 1);
    }

    /// The search surfaces whole albums and artists whose name matches,
    /// above the tracks: a free term hits either name, `album:` and
    /// `artist:` pin, and a track-only field (title) excludes both.
    #[test]
    fn search_surfaces_albums_and_artists() {
        fn full(path: &str, album_artist: &str, album: &str, title: &str) -> TrackRow {
            TrackRow {
                sub: 0,
                cue: None,
                path: path.into(),
                title: title.into(),
                artist: album_artist.into(),
                album_artist: album_artist.into(),
                album: album.into(),
                genre: String::new(),
                year: 0,
                disc_no: 0,
                track_no: 0,
                duration_ms: 0,
                codec: String::new(),
                bitrate_kbps: 0,
                sample_rate_hz: 0,
                bit_depth: 0,
                rating: 0,
                replay_gain: Default::default(),
                bpm: None,
                size: 0,
                mtime: 0,
            }
        }
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                full(
                    "/m/1.mp3",
                    "Fleet Foxes",
                    "Fleet Foxes",
                    "White Winter Hymnal",
                ),
                full("/m/2.mp3", "Fleet Foxes", "Helplessness Blues", "Montezuma"),
                full("/m/3.mp3", "ODESZA", "A Moment Apart", "Line Of Sight"),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        // A free term surfaces the one matching artist.
        let artists = p.search_artists("fleet");
        assert_eq!(artists.len(), 1);
        assert_eq!(
            p.album_artists.strings[artists[0].album_artist as usize],
            "Fleet Foxes"
        );

        // The album artist matches, so both its albums surface, sorted by
        // artist then album name.
        let albums = p.search_albums("fleet");
        assert_eq!(albums.len(), 2);
        assert_eq!(p.albums.strings[albums[0].album as usize], "Fleet Foxes");
        assert_eq!(
            p.albums.strings[albums[1].album as usize],
            "Helplessness Blues"
        );

        // A pin narrows to the album name.
        let pinned = p.search_albums("album:helpless");
        assert_eq!(pinned.len(), 1);
        assert_eq!(
            p.albums.strings[pinned[0].album as usize],
            "Helplessness Blues"
        );

        // A track-only field excludes every album and artist.
        assert!(p.search_albums("title:montezuma").is_empty());
        assert!(p.search_artists("title:montezuma").is_empty());
    }

    /// The structured filter matches whole values only - "Air" leaves
    /// "Airborne" out where the text search would take both - values OR
    /// within a field, and fields AND across.
    #[test]
    fn filter_mask_matches_exact_values() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "Air", 1998),
                track("/m/2.mp3", "Two", "Airborne", 1998),
                track("/m/3.mp3", "Three", "Air", 2001),
                track("/m/4.mp3", "Four", "Moby", 1999),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        let hits = |filter: &FilterSet| -> Vec<&str> {
            let mask = p.filter_mask(filter).unwrap();
            (0..p.len() as u32)
                .filter(|&i| mask[i as usize])
                .map(|i| p.resolve(i).title)
                .collect()
        };

        // Empty means no filtering; callers skip the scan.
        assert!(p.filter_mask(&FilterSet::default()).is_none());

        // Exact, so the substring neighbor stays out.
        let mut f = FilterSet::default();
        f.toggle(FilterField::Artist, "Air");
        assert_eq!(hits(&f), ["One", "Three"]);

        // A second value in the same field ORs in.
        f.toggle(FilterField::Artist, "Moby");
        assert_eq!(hits(&f), ["One", "Three", "Four"]);

        // Another field ANDs across.
        f.toggle(FilterField::Year, "1998");
        assert_eq!(hits(&f), ["One"]);

        // Toggling a picked value back off drops it.
        f.toggle(FilterField::Year, "1998");
        f.toggle(FilterField::Artist, "Moby");
        assert_eq!(hits(&f), ["One", "Three"]);
    }

    /// The id pin narrows to an explicit track set, the channel a
    /// selection-following view rides. It ANDs with the field picks, and an
    /// empty pin matches nothing rather than everything.
    #[test]
    fn filter_mask_pins_explicit_ids() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "Air", 1998),
                track("/m/2.mp3", "Two", "Airborne", 1998),
                track("/m/3.mp3", "Three", "Air", 2001),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        let hits = |filter: &FilterSet| -> Vec<&str> {
            let mask = p.filter_mask(filter).unwrap();
            (0..p.len() as u32)
                .filter(|&i| mask[i as usize])
                .map(|i| p.resolve(i).title)
                .collect()
        };
        let id_of = |title: &str| -> i64 {
            let row = (0..p.len() as u32)
                .find(|&i| p.resolve(i).title == title)
                .unwrap();
            p.db_id[row as usize]
        };

        // A pin on its own is a filter, so the mask is built rather than
        // skipped the way an all-empty set is.
        let pinned = FilterSet::with_ids(vec![id_of("One"), id_of("Three")]);
        assert!(!pinned.is_empty());
        assert_eq!(hits(&pinned), ["One", "Three"]);

        // An emptied selection shows nothing, not everything.
        assert_eq!(hits(&FilterSet::with_ids(Vec::new())), Vec::<&str>::new());

        // Field picks still AND across the pin.
        let mut both = FilterSet::with_ids(vec![id_of("One"), id_of("Three")]);
        both.toggle(FilterField::Year, "2001");
        assert_eq!(hits(&both), ["Three"]);

        // The chips read past the pin: it is not the filter panel's doing.
        assert!(pinned.fields_empty());
    }

    /// A queue entry the catalog never saw (a file dropped straight on the
    /// queue) carries no db id, so an id pin leaves it out - and leaves it
    /// out on its own, not lumped in with every other id-less row. Field
    /// picks still judge it on its tags.
    #[test]
    fn an_id_less_row_never_passes_an_id_pin() {
        let dropped = TrackFields {
            db_id: None,
            title: "Bootleg",
            artist: "Air",
            album_artist: "Air",
            album: "",
            genre: "Electronic",
            year: 2001,
            codec: "flac",
            path: "/tmp/bootleg.flac",
        };
        let catalogued = TrackFields {
            db_id: Some(7),
            ..dropped
        };

        // Any pin at all, including one that happens to hold 0, misses it.
        for ids in [vec![7], vec![0], vec![0, 7], Vec::new()] {
            let pinned = FilterSet::with_ids(ids);
            assert!(
                !pinned.matches(&dropped, false),
                "an off-catalog row has no id to pin"
            );
        }
        assert!(FilterSet::with_ids(vec![7]).matches(&catalogued, false));

        // No pin, and the row is judged on its fields like any other.
        let mut by_artist = FilterSet::default();
        by_artist.toggle(FilterField::Artist, "Air");
        assert!(by_artist.matches(&dropped, false));
        by_artist.toggle(FilterField::Artist, "Air");
        by_artist.toggle(FilterField::Artist, "Daft Punk");
        assert!(!by_artist.matches(&dropped, false));
    }

    /// The plays column loads the listens aggregate and sorts like any
    /// other key; a track with no events stays at zero.
    #[test]
    fn plays_fill_from_listens() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", 2000),
                track("/m/2.mp3", "Two", "B", 2001),
            ],
        )
        .unwrap();
        let listen = crate::listens::listen_for_path(&conn, "/m/2.mp3", 100)
            .unwrap()
            .unwrap();
        crate::listens::append(&conn, &listen).unwrap();
        crate::listens::append(&conn, &listen).unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        assert_eq!(p.resolve(0).plays, 0);
        assert_eq!(p.resolve(1).plays, 2);
        let by_plays = p.sort_view(&[0, 1], SortKey::Plays, true);
        assert_eq!(by_plays, [1, 0]);
    }

    /// A brute-force reference for search_artists: the distinct non-empty
    /// album artists whose lowered name matches every free/artist term, in
    /// first-seen row order then sorted by name. Mirrors the function's rule
    /// without the cache, so a mismatch flags the cached path drifting.
    fn ref_artists(p: &Projection, query: &str) -> Vec<(String, u32)> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let matches = |lower: &str| {
            terms.iter().all(|t| match t.field {
                None | Some(QueryField::Artist) | Some(QueryField::AlbumArtist) => {
                    lower.contains(&t.needle)
                }
                _ => false,
            })
        };
        let mut seen: HashSet<u32> = HashSet::new();
        let mut out: Vec<(String, u32)> = Vec::new();
        for row in 0..p.len() as u32 {
            let sym = p.album_artist[row as usize];
            if !seen.insert(sym) {
                continue;
            }
            let name = &p.album_artists.strings[sym as usize];
            if name.is_empty() || !matches(&p.album_artists.lower[sym as usize]) {
                continue;
            }
            out.push((name.clone(), row));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The search_albums counterpart to ref_artists: distinct non-empty
    /// (album artist, album) pairs matching every term, sorted by artist then
    /// album.
    fn ref_albums(p: &Projection, query: &str) -> Vec<(String, String, u32)> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let matches = |artist: &str, album: &str| {
            terms.iter().all(|t| match t.field {
                None => artist.contains(&t.needle) || album.contains(&t.needle),
                Some(QueryField::Album) => album.contains(&t.needle),
                Some(QueryField::Artist) | Some(QueryField::AlbumArtist) => {
                    artist.contains(&t.needle)
                }
                _ => false,
            })
        };
        let mut seen: HashSet<u64> = HashSet::new();
        let mut out: Vec<(String, String, u32)> = Vec::new();
        for row in 0..p.len() as u32 {
            let i = row as usize;
            let aa = p.album_artist[i];
            let al = p.album[i];
            let key = (aa as u64) << 32 | al as u64;
            if !seen.insert(key) {
                continue;
            }
            let album_name = &p.albums.strings[al as usize];
            if album_name.is_empty()
                || !matches(
                    &p.album_artists.lower[aa as usize],
                    &p.albums.lower[al as usize],
                )
            {
                continue;
            }
            out.push((
                p.album_artists.strings[aa as usize].clone(),
                album_name.clone(),
                row,
            ));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        out
    }

    /// The cached search_artists/search_albums return exactly what a
    /// straightforward brute-force distinct pass would, across empty,
    /// no-match, single, and multi-term queries. The cache moves the O(rows)
    /// work off the keystroke path, so this guards it hasn't changed results.
    #[test]
    fn search_grouped_matches_reference() {
        fn full(path: &str, album_artist: &str, album: &str, title: &str) -> TrackRow {
            TrackRow {
                sub: 0,
                cue: None,
                path: path.into(),
                title: title.into(),
                artist: album_artist.into(),
                album_artist: album_artist.into(),
                album: album.into(),
                genre: String::new(),
                year: 0,
                disc_no: 0,
                track_no: 0,
                duration_ms: 0,
                codec: String::new(),
                bitrate_kbps: 0,
                sample_rate_hz: 0,
                bit_depth: 0,
                rating: 0,
                replay_gain: Default::default(),
                bpm: None,
                size: 0,
                mtime: 0,
            }
        }
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                full(
                    "/m/1.mp3",
                    "Fleet Foxes",
                    "Fleet Foxes",
                    "White Winter Hymnal",
                ),
                full("/m/2.mp3", "Fleet Foxes", "Helplessness Blues", "Montezuma"),
                full("/m/3.mp3", "ODESZA", "A Moment Apart", "Line Of Sight"),
                full("/m/4.mp3", "Daft Punk", "Discovery", "One More Time"),
                full("/m/5.mp3", "Daft Punk", "Discovery", "Aerodynamic"),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        let check = |q: &str| {
            let got_artists: Vec<(String, u32)> = p
                .search_artists(q)
                .iter()
                .map(|h| {
                    (
                        p.album_artists.strings[h.album_artist as usize].clone(),
                        h.row,
                    )
                })
                .collect();
            assert_eq!(
                got_artists,
                ref_artists(&p, q),
                "artists mismatch for {q:?}"
            );
            let got_albums: Vec<(String, String, u32)> = p
                .search_albums(q)
                .iter()
                .map(|h| {
                    (
                        p.album_artists.strings[h.album_artist as usize].clone(),
                        p.albums.strings[h.album as usize].clone(),
                        h.row,
                    )
                })
                .collect();
            assert_eq!(got_albums, ref_albums(&p, q), "albums mismatch for {q:?}");
        };

        // Empty, no-match, single, multi-term, and pinned queries.
        check("");
        check("zzznomatch");
        check("fleet");
        check("daft");
        check("d");
        check("daft discovery");
        check("album:discovery");
        check("artist:fleet album:helpless");
        check("title:montezuma");
    }

    /// The distinct caches are built from immutable projection data, so a
    /// second call returns the same thing - the OnceLock doesn't corrupt
    /// state between calls.
    #[test]
    fn search_cache_is_stable_across_calls() {
        fn full(path: &str, album_artist: &str, album: &str) -> TrackRow {
            TrackRow {
                sub: 0,
                cue: None,
                path: path.into(),
                title: "t".into(),
                artist: album_artist.into(),
                album_artist: album_artist.into(),
                album: album.into(),
                genre: String::new(),
                year: 0,
                disc_no: 0,
                track_no: 0,
                duration_ms: 0,
                codec: String::new(),
                bitrate_kbps: 0,
                sample_rate_hz: 0,
                bit_depth: 0,
                rating: 0,
                replay_gain: Default::default(),
                bpm: None,
                size: 0,
                mtime: 0,
            }
        }
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                full("/m/1.mp3", "Air", "Moon Safari"),
                full("/m/2.mp3", "Air", "Talkie Walkie"),
                full("/m/3.mp3", "Moby", "Play"),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        let artists1: Vec<u32> = p
            .search_artists("a")
            .iter()
            .map(|h| h.album_artist)
            .collect();
        let artists2: Vec<u32> = p
            .search_artists("a")
            .iter()
            .map(|h| h.album_artist)
            .collect();
        assert_eq!(artists1, artists2);

        let albums1: Vec<(u32, u32)> = p
            .search_albums("a")
            .iter()
            .map(|h| (h.album_artist, h.album))
            .collect();
        let albums2: Vec<(u32, u32)> = p
            .search_albums("a")
            .iter()
            .map(|h| (h.album_artist, h.album))
            .collect();
        assert_eq!(albums1, albums2);

        // The full-catalog search is likewise stable call to call.
        assert_eq!(p.search("air"), p.search("air"));
    }

    /// The `year:` filter matches on the digits and holds at the boundary
    /// years (0 and 65535) without panicking on the mask index.
    #[test]
    fn search_year_filter_matches_and_boundaries() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "Zero", "A", 0),
                track("/m/2.mp3", "Nineties", "B", 1999),
                track("/m/3.mp3", "Two Thousand", "C", 2000),
                track("/m/4.mp3", "Max", "D", u16::MAX),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();

        // The decade needle takes the one nineties row.
        assert_eq!(titles_for(&p, "year:199"), ["Nineties"]);
        // An exact year.
        assert_eq!(titles_for(&p, "year:2000"), ["Two Thousand"]);
        // A bare digit matches on the substring, so "0" takes both years
        // whose decimal holds a zero.
        assert_eq!(titles_for(&p, "year:0"), ["Zero", "Two Thousand"]);
        // The max year matches its own digits, no index panic at the top.
        assert_eq!(titles_for(&p, &format!("year:{}", u16::MAX)), ["Max"]);
    }

    /// heap_bytes counts the `added` column, so growing it grows the total.
    #[test]
    fn heap_bytes_counts_added() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", 2000)]).unwrap();
        let small = Projection::load_serial(&conn, false).unwrap();

        store::insert_batch(
            &mut conn,
            &[
                track("/m/2.mp3", "Two", "A", 2000),
                track("/m/3.mp3", "Three", "A", 2000),
                track("/m/4.mp3", "Four", "A", 2000),
            ],
        )
        .unwrap();
        let big = Projection::load_serial(&conn, false).unwrap();

        assert!(big.added.len() > small.added.len());
        assert!(big.heap_bytes() > small.heap_bytes());
    }

    /// A two-disc set plays disc 1 through before disc 2 starts, instead
    /// of interleaving the discs' track numbers.
    #[test]
    fn canonical_order_keys_disc_before_track() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                row("/m/2-1.mp3", "Set", 2, 1),
                row("/m/1-2.mp3", "Set", 1, 2),
                row("/m/2-2.mp3", "Set", 2, 2),
                row("/m/1-1.mp3", "Set", 1, 1),
            ],
        )
        .unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        let keys: Vec<(u16, u16)> = p
            .sort_canonical()
            .iter()
            .map(|&i| (p.disc_no[i as usize], p.track_no[i as usize]))
            .collect();
        assert_eq!(keys, [(1, 1), (1, 2), (2, 1), (2, 2)]);
    }
}
