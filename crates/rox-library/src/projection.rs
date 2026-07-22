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
    map: HashMap<Box<str>, u32>,
    table: Vec<String>,
}

impl Interner {
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = self.table.len() as u32;
        self.map.insert(s.into(), sym);
        self.table.push(s.to_string());
        sym
    }
}

/// Interned strings plus a lowercase copy for case-folded search.
pub struct SymTable {
    pub strings: Vec<String>,
    pub lower: Vec<String>,
}

impl From<Interner> for SymTable {
    fn from(interner: Interner) -> Self {
        let lower = interner
            .table
            .par_iter()
            .map(|s| s.to_lowercase())
            .collect();
        SymTable {
            strings: interner.table,
            lower,
        }
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
    rating: Vec<u8>,
    added: Vec<i64>,
    folder: Vec<u32>,
    artists: Interner,
    album_artists: Interner,
    albums: Interner,
    genres: Interner,
    codecs: Interner,
    folders: Interner,
}

impl Builder {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        id: i64,
        path: &str,
        title: &str,
        artist: &str,
        album_artist: &str,
        album: &str,
        genre: &str,
        year: u16,
        disc_no: u16,
        track_no: u16,
        duration_ms: u32,
        codec: &str,
        bitrate_kbps: u16,
        rating: u8,
        added: i64,
    ) {
        self.db_id.push(id);
        self.title.push(title);
        self.title_lower.push_lowercased(title);
        self.artist.push(self.artists.intern(artist));
        self.album_artist
            .push(self.album_artists.intern(album_artist));
        self.album.push(self.albums.intern(album));
        self.genre.push(self.genres.intern(genre));
        self.year.push(year);
        self.disc_no.push(disc_no);
        self.track_no.push(track_no);
        self.duration_ms.push(duration_ms);
        self.codec.push(self.codecs.intern(codec));
        self.bitrate_kbps.push(bitrate_kbps);
        self.rating.push(rating);
        self.added.push(added);
        // Interned per album directory, so it stays cheap even at ten
        // million rows; an empty parent (a bare filename) folds to "".
        let folder = Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default();
        self.folder.push(self.folders.intern(&folder));
    }
}

pub struct Projection {
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
    pub rating: u8,
    pub plays: u32,
    pub added: i64,
    pub folder: &'a str,
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
];

/// One parsed query term: a lowercased needle, maybe pinned to one field.
pub struct Term {
    pub field: Option<QueryField>,
    pub needle: String,
}

/// Split a query into terms. Whitespace separates, double quotes keep a
/// value together, and a known `field:` prefix pins the term to that
/// field; every term must match for a row to hit. So
/// `stronger artist:"daft punk"` is a free term and an artist term, and
/// an unknown prefix like `ac:dc` stays one free term.
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
                    if let Some(&(_, field)) =
                        QUERY_FIELDS.iter().find(|(n, _)| *n == name)
                    {
                        return Term {
                            field: Some(field),
                            needle: strip(&raw[i + 1..]).to_lowercase(),
                        };
                    }
                }
            }
            Term {
                field: None,
                needle: strip(raw).to_lowercase(),
            }
        })
        .filter(|t| !t.needle.is_empty())
        .collect()
}

/// A field the structured filter can pin exact values to: the interned
/// columns plus the year. Titles stay out; a text term already reaches
/// them, and a filter over ten million distinct titles filters nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilterSet {
    pub fields: Vec<(FilterField, Vec<String>)>,
}

impl FilterSet {
    pub fn is_empty(&self) -> bool {
        self.fields.iter().all(|(_, values)| values.is_empty())
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
    /// the mask over the catalog.
    pub fn matches(&self, fields: &TrackFields) -> bool {
        self.fields.iter().all(|(field, values)| {
            if values.is_empty() {
                return true;
            }
            match field {
                FilterField::Artist => values.iter().any(|v| v == fields.artist),
                FilterField::AlbumArtist => values.iter().any(|v| v == fields.album_artist),
                FilterField::Album => values.iter().any(|v| v == fields.album),
                FilterField::Genre => values.iter().any(|v| v == fields.genre),
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
    pub title: &'a str,
    pub artist: &'a str,
    pub album_artist: &'a str,
    pub album: &'a str,
    pub genre: &'a str,
    pub year: u16,
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

/// Whether one track's fields satisfy every parsed query term. Free terms
/// match title, artist, album artist, album, or genre; a pinned term only its
/// field, the same rule [`Projection::search`] applies over the catalog.
/// Terms AND together; needles come lowercased from [`parse_query`].
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
        Some(QueryField::Year) => fields.year.to_string().contains(t.needle.as_str()),
    })
}

/// A sortable column of the projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    Rating,
    Plays,
    Added,
}

impl Projection {
    pub fn len(&self) -> usize {
        self.db_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.db_id.is_empty()
    }

    /// Load on one connection, one thread: the ADR 5 shape as written.
    pub fn load_serial(conn: &rusqlite::Connection) -> rusqlite::Result<Self> {
        let max = store::max_rowid(conn)?;
        let mut b = Builder::default();
        store::scan_range(
            conn,
            0,
            max,
            |id, path, title, artist, album_artist, album, genre, year, dn, tn, dur, codec, kbps, rating, added| {
                b.push(
                    id,
                    path,
                    title,
                    artist,
                    album_artist,
                    album,
                    genre,
                    year,
                    dn,
                    tn,
                    dur,
                    codec,
                    kbps,
                    rating,
                    added,
                );
            },
        )?;
        let projection = Self::merge(vec![b]);
        projection.fill_plays(conn)?;
        Ok(projection)
    }

    /// Load with one reader per shard over disjoint rowid ranges (WAL allows
    /// concurrent readers), then merge shards by remapping local symbols.
    pub fn load_parallel(db_path: &Path, shards: usize) -> rusqlite::Result<Self> {
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
                        let mut b = Builder::default();
                        store::scan_range(
                            &conn,
                            lo,
                            hi,
                            |id,
                             path,
                             title,
                             artist,
                             album_artist,
                             album,
                             genre,
                             year,
                             dn,
                             tn,
                             dur,
                             codec,
                             kbps,
                             rating,
                             added| {
                                b.push(
                                    id,
                                    path,
                                    title,
                                    artist,
                                    album_artist,
                                    album,
                                    genre,
                                    year,
                                    dn,
                                    tn,
                                    dur,
                                    codec,
                                    kbps,
                                    rating,
                                    added,
                                );
                            },
                        )?;
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
        let projection = Self::merge(shards);
        let conn = store::open(db_path)?;
        projection.fill_plays(&conn)?;
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

    fn merge(shards: Vec<Builder>) -> Self {
        let mut artists = Interner::default();
        let mut album_artists = Interner::default();
        let mut albums = Interner::default();
        let mut genres = Interner::default();
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
        out.rating.reserve(total);
        out.added.reserve(total);
        out.folder.reserve(total);

        for shard in shards {
            let map_a: Vec<u32> = shard
                .artists
                .table
                .iter()
                .map(|s| artists.intern(s))
                .collect();
            let map_aa: Vec<u32> = shard
                .album_artists
                .table
                .iter()
                .map(|s| album_artists.intern(s))
                .collect();
            let map_b: Vec<u32> = shard
                .albums
                .table
                .iter()
                .map(|s| albums.intern(s))
                .collect();
            let map_g: Vec<u32> = shard
                .genres
                .table
                .iter()
                .map(|s| genres.intern(s))
                .collect();
            let map_c: Vec<u32> = shard
                .codecs
                .table
                .iter()
                .map(|s| codecs.intern(s))
                .collect();
            let map_f: Vec<u32> = shard
                .folders
                .table
                .iter()
                .map(|s| folders.intern(s))
                .collect();
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
            out.rating.extend_from_slice(&shard.rating);
            out.added.extend_from_slice(&shard.added);
            out.folder
                .extend(shard.folder.iter().map(|&s| map_f[s as usize]));
        }

        let plays = (0..out.db_id.len()).map(|_| AtomicU32::new(0)).collect();
        Projection {
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
            added: out.added,
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
            rating: self.rating[i].load(Ordering::Relaxed),
            plays: self.plays[i].load(Ordering::Relaxed),
            added: self.added[i],
            folder: &self.folders.strings[self.folder[i] as usize],
        }
    }

    /// Case-folded substring search, one term at a time per
    /// [`parse_query`]: a free term matches title, artist, album artist,
    /// album, or genre; a pinned term only its field. Terms AND together.
    /// Symbol tables are matched whole first; the row scan then only does
    /// per-title memmem plus table lookups.
    pub fn search(&self, query: &str) -> Vec<u32> {
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
                Some(QueryField::Title) => {
                    Hits::Title(memmem::Finder::new(t.needle.as_bytes()))
                }
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
                Hits::Title(finder) => {
                    finder.find(self.title_lower.get(i).as_bytes()).is_some()
                }
                Hits::Year(mask) => mask[self.year[i] as usize],
            })
        })
    }

    /// The distinct album artists whose name matches the query, each with a
    /// representative row for the cover and count. For the search's grouped
    /// hits, so typing an artist's name surfaces the artist itself above the
    /// tracks. A term pinned to a track-only field (title, album, genre,
    /// year) excludes every artist, since it can't match an artist name.
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
    /// artist; a title, genre, or year term excludes every album. Ordered
    /// by artist then album; first-seen row per pair.
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

        let sym_ok = |table: &SymTable, values: &[String]| -> Vec<bool> {
            table
                .strings
                .iter()
                .map(|s| values.iter().any(|v| v == s))
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
                FilterField::Genre => Check::Sym {
                    column: &self.genre,
                    ok: sym_ok(&self.genres, values),
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

        Some(
            (0..self.len())
                .into_par_iter()
                .map(|i| {
                    checks.iter().all(|c| match c {
                        Check::Sym { column, ok } => ok[column[i] as usize],
                        Check::Year(ok) => ok[self.year[i] as usize],
                    })
                })
                .collect(),
        )
    }

    pub fn filter_genre(&self, genre: &str) -> Vec<u32> {
        match self.genres.strings.iter().position(|g| g == genre) {
            Some(sym) => {
                let sym = sym as u32;
                self.scan_rows(|i| self.genre[i] == sym)
            }
            None => Vec::new(),
        }
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
            SortKey::Rating => {
                self.order_view(view, descending, |i| self.rating[i].load(Ordering::Relaxed))
            }
            SortKey::Plays => {
                self.order_view(view, descending, |i| self.plays[i].load(Ordering::Relaxed))
            }
            SortKey::Added => self.order_view(view, descending, |i| self.added[i]),
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
            + self.duration_ms.capacity() * 4
            + self.rating.capacity()
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
    use crate::TrackRow;

    fn row(path: &str, album: &str, disc_no: u16, track_no: u16) -> TrackRow {
        TrackRow {
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
            rating: 0,
            size: 0,
            mtime: 0,
        }
    }

    fn track(path: &str, title: &str, artist: &str, year: u16) -> TrackRow {
        TrackRow {
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
            rating: 0,
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
        assert_eq!((terms[0].field, terms[0].needle.as_str()), (None, "stronger"));
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
        let p = Projection::load_serial(&conn).unwrap();

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

    /// The per-track matcher the queue, history, and playlists filter their
    /// own rows with agrees with `search` over the catalog: free terms sweep
    /// the text fields, pins isolate one, and the structured filter matches
    /// whole values.
    #[test]
    fn track_matcher_mirrors_search() {
        let fields = TrackFields {
            title: "Stronger",
            artist: "Daft Punk",
            album_artist: "Daft Punk",
            album: "Discovery",
            genre: "Electronic",
            year: 2001,
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
        assert!(track_matches(&parse_query(r#"artist:"daft punk""#), &fields));
        assert!(!track_matches(&parse_query("title:discovery"), &fields));
        // Year matches on the digits; folder pins to the parent directory.
        assert!(track_matches(&parse_query("year:200"), &fields));
        assert!(track_matches(&parse_query("folder:discovery"), &fields));
        assert!(!track_matches(&parse_query("folder:other"), &fields));

        // The structured filter matches whole values, never substrings.
        let mut filter = FilterSet::default();
        filter.toggle(FilterField::Artist, "Daft Punk");
        assert!(filter.matches(&fields));
        let mut narrower = filter.clone();
        narrower.toggle(FilterField::Artist, "Air");
        // Values OR within a field, so the extra pick still passes.
        assert!(narrower.matches(&fields));
        let mut year = FilterSet::default();
        year.toggle(FilterField::Year, "2001");
        assert!(year.matches(&fields));
        year.clear(FilterField::Year);
        year.toggle(FilterField::Year, "1999");
        assert!(!year.matches(&fields));
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
        let p = Projection::load_serial(&conn).unwrap();

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
        let p = Projection::load_serial(&conn).unwrap();

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
            title: "",
            artist: "",
            album_artist: "",
            album: "",
            genre: "",
            year: 0,
            path,
        };
        assert!(filter.matches(&fields("/music/Air/Moon Safari/2.mp3")));
        assert!(!filter.matches(&fields("/music/Airborne/3.mp3")));
    }

    /// The search surfaces whole albums and artists whose name matches,
    /// above the tracks: a free term hits either name, `album:` and
    /// `artist:` pin, and a track-only field (title) excludes both.
    #[test]
    fn search_surfaces_albums_and_artists() {
        fn full(path: &str, album_artist: &str, album: &str, title: &str) -> TrackRow {
            TrackRow {
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
                rating: 0,
                size: 0,
                mtime: 0,
            }
        }
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                full("/m/1.mp3", "Fleet Foxes", "Fleet Foxes", "White Winter Hymnal"),
                full("/m/2.mp3", "Fleet Foxes", "Helplessness Blues", "Montezuma"),
                full("/m/3.mp3", "ODESZA", "A Moment Apart", "Line Of Sight"),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn).unwrap();

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
        let p = Projection::load_serial(&conn).unwrap();

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

        let p = Projection::load_serial(&conn).unwrap();
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
                || !matches(&p.album_artists.lower[aa as usize], &p.albums.lower[al as usize])
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
                rating: 0,
                size: 0,
                mtime: 0,
            }
        }
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                full("/m/1.mp3", "Fleet Foxes", "Fleet Foxes", "White Winter Hymnal"),
                full("/m/2.mp3", "Fleet Foxes", "Helplessness Blues", "Montezuma"),
                full("/m/3.mp3", "ODESZA", "A Moment Apart", "Line Of Sight"),
                full("/m/4.mp3", "Daft Punk", "Discovery", "One More Time"),
                full("/m/5.mp3", "Daft Punk", "Discovery", "Aerodynamic"),
            ],
        )
        .unwrap();
        let p = Projection::load_serial(&conn).unwrap();

        let check = |q: &str| {
            let got_artists: Vec<(String, u32)> = p
                .search_artists(q)
                .iter()
                .map(|h| (p.album_artists.strings[h.album_artist as usize].clone(), h.row))
                .collect();
            assert_eq!(got_artists, ref_artists(&p, q), "artists mismatch for {q:?}");
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
                rating: 0,
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
        let p = Projection::load_serial(&conn).unwrap();

        let artists1: Vec<u32> = p.search_artists("a").iter().map(|h| h.album_artist).collect();
        let artists2: Vec<u32> = p.search_artists("a").iter().map(|h| h.album_artist).collect();
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
        let p = Projection::load_serial(&conn).unwrap();

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
        let small = Projection::load_serial(&conn).unwrap();

        store::insert_batch(
            &mut conn,
            &[
                track("/m/2.mp3", "Two", "A", 2000),
                track("/m/3.mp3", "Three", "A", 2000),
                track("/m/4.mp3", "Four", "A", 2000),
            ],
        )
        .unwrap();
        let big = Projection::load_serial(&conn).unwrap();

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

        let p = Projection::load_serial(&conn).unwrap();
        let keys: Vec<(u16, u16)> = p
            .sort_canonical()
            .iter()
            .map(|&i| (p.disc_no[i as usize], p.track_no[i as usize]))
            .collect();
        assert_eq!(keys, [(1, 1), (1, 2), (2, 1), (2, 2)]);
    }
}
