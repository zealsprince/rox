//! The read path of ADR 5 at scale. Columnar: artist, album and genre are
//! interned to u32 symbols, titles live in one byte arena with an offset
//! table (never ten million heap Strings), and every browse order is a
//! precomputed Vec<u32> of row indices over integer ranks. Search per ADR 6
//! is substring: the interned tables are scanned whole, and only titles need
//! the full-row scan, split across cores. Query syntax is [`parse_query`]'s.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use memchr::memmem;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::store;

const CHUNK: usize = 65_536;

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

/// Log the first refusal only; past the ceiling every row refuses.
fn note_arena_overflow() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        log::error!(
            "projection: the title arena hit its 4 GiB ceiling; \
             rows are being dropped and incremental updates fall back to a full reload"
        );
    });
}

impl Arena {
    /// None when the offset no longer fits u32. Separate so the refusal is
    /// testable without four gigabytes of titles.
    fn checked_end(current: usize, added: usize) -> Option<u32> {
        u32::try_from(current.checked_add(added)?).ok()
    }

    fn fits(&self, added: usize) -> bool {
        Self::checked_end(self.bytes.len(), added).is_some()
    }

    fn bytes_len(&self) -> usize {
        self.bytes.len()
    }

    /// Append, or refuse and leave the arena untouched. On false the caller must
    /// drop the whole row: the columns are positional, so a partial row shifts
    /// every row after it.
    fn push(&mut self, s: &str) -> bool {
        let Some(end) = Self::checked_end(self.bytes.len(), s.len()) else {
            note_arena_overflow();
            return false;
        };
        self.bytes.push_str(s);
        self.offsets.push(end);
        true
    }

    fn push_folded(&mut self, s: &str) -> bool {
        // Must go through the same crate::fold::fold as query needles, or the two
        // sides stop meeting. ASCII skips the allocation.
        if s.is_ascii() {
            if !self.fits(s.len()) {
                note_arena_overflow();
                return false;
            }
            self.bytes
                .extend(s.bytes().map(|b| b.to_ascii_lowercase() as char));
        } else {
            let folded = crate::fold::fold(s);
            if !self.fits(folded.len()) {
                note_arena_overflow();
                return false;
            }
            self.bytes.push_str(&folded);
        }
        self.offsets.push(self.bytes.len() as u32);
        true
    }

    fn pop(&mut self) {
        if self.offsets.len() > 1 {
            self.offsets.pop();
            let end = *self.offsets.last().expect("the base offset always stays");
            self.bytes.truncate(end as usize);
        }
    }

    fn is_blank(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn get(&self, i: usize) -> &str {
        &self.bytes[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }

    /// False when the two together pass the ceiling; nothing is appended.
    fn append(&mut self, other: &Arena) -> bool {
        let Some(_) = Self::checked_end(self.bytes.len(), other.bytes.len()) else {
            note_arena_overflow();
            return false;
        };
        let base = self.bytes.len() as u32;
        self.bytes.push_str(&other.bytes);
        self.offsets
            .extend(other.offsets[1..].iter().map(|o| o + base));
        true
    }

    pub fn heap_bytes(&self) -> usize {
        self.bytes.capacity() + self.offsets.capacity() * 4
    }
}

#[derive(Default)]
struct Interner {
    /// Keys in `map` are lowercased when set; the display casing is picked from
    /// `variants` at finalize.
    fold: bool,
    map: HashMap<Box<str>, u32>,
    table: Vec<String>,
    /// Per symbol, every casing and its row count. Only filled when folding.
    variants: Vec<HashMap<String, u32>>,
    /// Per symbol, every sort name and its row count. Empty maps when the
    /// library has no sort tags.
    sorts: Vec<HashMap<String, u32>>,
}

impl Interner {
    fn folded(fold: bool) -> Self {
        Interner {
            fold,
            ..Default::default()
        }
    }

    fn intern(&mut self, s: &str, sort: &str) -> u32 {
        self.intern_weighted(s, sort, 1)
    }

    /// The shard merge's path: weights are row counts, so the display pick
    /// reflects rows, not shards.
    fn intern_weighted(&mut self, s: &str, sort: &str, weight: u32) -> u32 {
        let sym = self.intern_name(s, weight);
        if !sort.is_empty() {
            *self.sorts[sym as usize]
                .entry(sort.to_string())
                .or_default() += weight;
        }
        sym
    }

    fn intern_name(&mut self, s: &str, weight: u32) -> u32 {
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
            self.sorts.push(HashMap::new());
            return sym;
        }
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = self.table.len() as u32;
        self.map.insert(s.into(), sym);
        self.table.push(s.to_string());
        self.sorts.push(HashMap::new());
        sym
    }

    /// The casing a symbol will finalize under. The sort-name layers key on this.
    fn display(&self, sym: usize) -> &str {
        if self.fold {
            weighted_pick(&self.variants[sym])
                .map(String::as_str)
                .unwrap_or(&self.table[sym])
        } else {
            &self.table[sym]
        }
    }

    fn absorb(&mut self, other: &Interner) -> Vec<u32> {
        if !self.fold {
            return other
                .table
                .iter()
                .enumerate()
                .map(|(sym, s)| {
                    let mapped = self.intern_weighted(s, "", 1);
                    self.absorb_sorts(mapped, &other.sorts[sym]);
                    mapped
                })
                .collect();
        }
        other
            .table
            .iter()
            .enumerate()
            .map(|(sym, s)| {
                let mut mapped = self.intern_weighted(s, "", 0);
                for (variant, &weight) in &other.variants[sym] {
                    mapped = self.intern_weighted(variant, "", weight);
                }
                self.absorb_sorts(mapped, &other.sorts[sym]);
                mapped
            })
            .collect()
    }

    fn absorb_sorts(&mut self, sym: u32, sorts: &HashMap<String, u32>) {
        for (sort, &weight) in sorts {
            *self.sorts[sym as usize].entry(sort.clone()).or_default() += weight;
        }
    }
}

/// Most rows wins, ties to the lexicographically smaller so reloads are
/// stable. The finalizing merge and the incremental append both vote this
/// way, so a patch gives a symbol the name a rebuild would.
fn weighted_pick(counts: &HashMap<String, u32>) -> Option<&String> {
    counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(s, _)| s)
}

/// Interned strings plus a folded copy for search, per [`crate::fold`], so
/// "beyonce" reaches Beyoncé.
pub struct SymTable {
    pub strings: Vec<String>,
    pub lower: Vec<String>,
    /// Per symbol, not per row. Both stay empty when no symbol has a sort name.
    pub sort: Vec<String>,
    pub sort_lower: Vec<String>,
}

impl From<Interner> for SymTable {
    fn from(mut interner: Interner) -> Self {
        let strings: Vec<String> = if interner.fold {
            std::mem::take(&mut interner.table)
                .into_iter()
                .zip(&interner.variants)
                .map(|(first, variants)| weighted_pick(variants).cloned().unwrap_or(first))
                .collect()
        } else {
            std::mem::take(&mut interner.table)
        };
        let lower = strings.par_iter().map(|s| crate::fold::fold(s)).collect();
        // Empty when nothing carried one; the accessors fall through to the display
        // name.
        let sort: Vec<String> = interner
            .sorts
            .iter()
            .map(|sorts| weighted_pick(sorts).cloned().unwrap_or_default())
            .collect();
        let (sort, sort_lower) = if sort.iter().all(String::is_empty) {
            (Vec::new(), Vec::new())
        } else {
            let folded = sort.par_iter().map(|s| crate::fold::fold(s)).collect();
            (sort, folded)
        };
        SymTable {
            strings,
            lower,
            sort,
            sort_lower,
        }
    }
}

impl SymTable {
    fn heap_bytes(&self) -> usize {
        self.strings
            .iter()
            .chain(self.lower.iter())
            .chain(self.sort.iter())
            .chain(self.sort_lower.iter())
            .map(|s| s.capacity() + 24)
            .sum()
    }

    pub fn sort_name(&self, sym: usize) -> &str {
        self.sort.get(sym).map(String::as_str).unwrap_or("")
    }

    /// The folded sort name alone, without [`SymTable::sort_key`]'s fallback.
    fn sort_lowered(&self, sym: usize) -> &str {
        self.sort_lower.get(sym).map(String::as_str).unwrap_or("")
    }

    /// Lowered under case folding, exact otherwise, matching
    /// [`Interner::intern_name`]. Lowered rather than folded: "Beyonce" and
    /// "Beyoncé" stay two symbols even though search reaches both.
    fn lookup_key(&self, sym: usize, fold: bool) -> Box<str> {
        if fold {
            self.strings[sym].to_lowercase().into()
        } else {
            self.strings[sym].as_str().into()
        }
    }

    /// The new symbol is the table's length. Symbol-order ranks go stale and the
    /// patch invalidates them.
    fn push_symbol(&mut self, display: &str, sort: &str) -> u32 {
        let sym = self.strings.len() as u32;
        // Pad the sort columns before the push, or the pad fills the new slot.
        if !self.sort.is_empty() || !sort.is_empty() {
            self.fill_sorts();
            self.sort.push(sort.to_string());
            self.sort_lower.push(crate::fold::fold(sort));
        }
        self.strings.push(display.to_string());
        self.lower.push(crate::fold::fold(display));
        sym
    }

    /// Only where the symbol has none: the first row settles it, and a second
    /// spelling waits for the next full rebuild.
    fn adopt_sort(&mut self, sym: usize, sort: &str) -> bool {
        if sort.is_empty() || !self.sort_name(sym).is_empty() {
            return false;
        }
        self.fill_sorts();
        self.sort[sym] = sort.to_string();
        self.sort_lower[sym] = crate::fold::fold(sort);
        true
    }

    /// Fill sort names from the library's meta tables, only where a symbol has
    /// none: a tag beats a lookup. Keyed by display string, the spelling the pass
    /// looked up. Returns how many symbols took one.
    fn fill_from_meta(&mut self, meta: &HashMap<String, String>) -> usize {
        let mut filled = 0;
        for sym in 0..self.strings.len() {
            if !self.sort_name(sym).is_empty() {
                continue;
            }
            // Cloned: adopting takes the table mutably.
            let Some(sort) = meta.get(&self.strings[sym]).cloned() else {
                continue;
            };
            if self.adopt_sort(sym, &sort) {
                filled += 1;
            }
        }
        filled
    }

    fn fill_sorts(&mut self) {
        if self.sort.len() < self.strings.len() {
            self.sort.resize(self.strings.len(), String::new());
            self.sort_lower.resize(self.strings.len(), String::new());
        }
    }

    /// Folded sort name, else folded display name, so Émilie files under E.
    pub fn sort_key(&self, sym: usize) -> &str {
        match self.sort_lower.get(sym) {
            Some(s) if !s.is_empty() => s,
            _ => &self.lower[sym],
        }
    }
}

/// Sorts ahead of every real gain and decodes to None.
pub const NO_GAIN: i16 = i16::MIN;

/// Hundredths of a dB in an i16: exact for any real gain, sorts without a
/// float comparator. Anything outside +-40 dB packs to [`NO_GAIN`].
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

pub fn unpack_gain(cdb: i16) -> Option<f32> {
    (cdb != NO_GAIN).then(|| cdb as f32 / 100.)
}

/// Zero, since no track runs at zero bpm. Sorts first and decodes to None.
pub const NO_BPM: u16 = 0;

/// Hundredths of a bpm in a u16, exact inside [`crate::tempo::SLOWEST`]..=
/// [`crate::tempo::FASTEST`]. Anything outside packs to [`NO_BPM`].
fn pack_bpm(bpm: Option<f32>) -> u16 {
    match bpm {
        Some(bpm) if (crate::tempo::SLOWEST..=crate::tempo::FASTEST).contains(&bpm) => {
            (bpm * 100.).round() as u16
        }
        _ => NO_BPM,
    }
}

pub fn unpack_bpm(cbpm: u16) -> Option<f32> {
    (cbpm != NO_BPM).then(|| cbpm as f32 / 100.)
}

/// One shard of rows being loaded; the whole library when loading serially.
#[derive(Default)]
pub struct Builder {
    db_id: Vec<i64>,
    title: Arena,
    title_lower: Arena,
    /// Empty strings for rows without one. Titles aren't interned, so sort titles
    /// need their own arena.
    title_sort: Arena,
    title_sort_lower: Arena,
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
    source: Vec<u32>,
    artists: Interner,
    album_artists: Interner,
    albums: Interner,
    genres: Interner,
    codecs: Interner,
    folders: Interner,
    sources: Interner,
    /// A row was refused because the title arena is full. The merge can go ahead;
    /// a patch can't, since dropping rows it was asked to apply leaves the
    /// projection disagreeing with the database.
    overflowed: bool,
}

impl Builder {
    /// Artist, album artist, album and genre fold case per the setting. Codecs
    /// and folders stay exact.
    fn new(fold: bool) -> Self {
        Builder {
            artists: Interner::folded(fold),
            album_artists: Interner::folded(fold),
            albums: Interner::folded(fold),
            genres: Interner::folded(fold),
            ..Default::default()
        }
    }

    /// The incremental counterpart to the full load's `fill_*_sorts` passes: a
    /// tag the owner wrote beats a looked-up name. Laid on the shard so it goes
    /// through [`absorb_shard`] and the patch agrees with a rebuild by
    /// construction. Track sort titles are fetched by id so a patch costs what
    /// changed.
    fn lay_meta_sorts(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        if self.db_id.is_empty() {
            return Ok(());
        }
        let artists = crate::artist_meta::load_all(conn)?;
        lay_over(&mut self.artists, &artists);
        lay_over(&mut self.album_artists, &artists);
        let albums = crate::album_meta::load_all(conn)?;
        lay_over(&mut self.albums, &albums);
        let titles = meta_titles_for(conn, &self.db_id)?;
        self.lay_meta_titles(&titles);
        Ok(())
    }

    /// Sort titles live in an append-only arena addressed by row, so filling one
    /// means rebuilding the pair. A shard is a handful of rows.
    fn lay_meta_titles(&mut self, meta: &HashMap<i64, String>) {
        if meta.is_empty() {
            return;
        }
        let mut display = Arena::default();
        let mut lower = Arena::default();
        for i in 0..self.db_id.len() {
            let from_file = self.title_sort.get(i);
            let sort = if from_file.is_empty() {
                meta.get(&self.db_id[i]).map(String::as_str).unwrap_or("")
            } else {
                from_file
            };
            if !display.push(sort) || !lower.push_folded(sort) {
                return;
            }
        }
        self.title_sort = display;
        self.title_sort_lower = lower;
    }

    /// All four title arenas or none: a partial row puts every column after it
    /// out of step.
    fn push_text(&mut self, title: &str, title_sort: &str) -> bool {
        if self.title.push(title) {
            if self.title_lower.push_folded(title) {
                if self.title_sort.push(title_sort) {
                    if self.title_sort_lower.push_folded(title_sort) {
                        return true;
                    }
                    self.title_sort.pop();
                }
                self.title_lower.pop();
            }
            self.title.pop();
        }
        false
    }

    /// False when the arena was full and the row was dropped whole.
    fn push(&mut self, row: store::ScanRow<'_>) -> bool {
        if !self.push_text(row.title, row.title_sort) {
            self.overflowed = true;
            return false;
        }
        self.db_id.push(row.id);
        self.artist
            .push(self.artists.intern(row.artist, row.artist_sort));
        self.album_artist.push(
            self.album_artists
                .intern(row.album_artist, row.album_artist_sort),
        );
        self.album
            .push(self.albums.intern(row.album, row.album_sort));
        self.genre.push(self.genres.intern(row.genre, ""));
        self.year.push(row.year);
        self.disc_no.push(row.disc_no);
        self.track_no.push(row.track_no);
        self.duration_ms.push(row.duration_ms);
        self.codec.push(self.codecs.intern(row.codec, ""));
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
        // A remote row's path is a URL or server id; splitting it would put a "http:"
        // root in the folder tree, so it folds to "".
        let folder = if row.source == crate::cue::LOCAL {
            Path::new(row.path)
                .parent()
                .map(|p| p.to_string_lossy())
                .unwrap_or_default()
        } else {
            std::borrow::Cow::Borrowed("")
        };
        self.folder.push(self.folders.intern(&folder, ""));
        self.source.push(self.sources.intern(row.source, ""));
        true
    }

    pub fn len(&self) -> usize {
        self.db_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.db_id.is_empty()
    }

    pub fn ids(&self) -> &[i64] {
        &self.db_id
    }
}

/// Fill a shard's sort names from a meta table where no row voted one in.
/// Keyed on display casing, like [`SymTable::fill_from_meta`].
fn lay_over(interner: &mut Interner, meta: &HashMap<String, String>) {
    if meta.is_empty() {
        return;
    }
    for sym in 0..interner.table.len() {
        if !interner.sorts[sym].is_empty() {
            continue;
        }
        let Some(sort) = meta.get(interner.display(sym)).cloned() else {
            continue;
        };
        interner.sorts[sym].insert(sort, 1);
    }
}

/// Fetched by id: the table grows with the library, not with the change.
fn meta_titles_for(
    conn: &rusqlite::Connection,
    ids: &[i64],
) -> rusqlite::Result<HashMap<i64, String>> {
    let mut stmt = conn.prepare_cached("SELECT title_sort FROM track_meta WHERE track_id = ?1")?;
    let mut out = HashMap::new();
    for &id in ids {
        let sort: Option<String> =
            stmt.query_row([id], |row| row.get(0))
                .or_else(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
        if let Some(sort) = sort.filter(|s| !s.is_empty()) {
            out.insert(id, sort);
        }
    }
    Ok(out)
}

/// Fold a shard's symbols into a finalized table that can't be re-voted. A
/// known value keeps its symbol and casing; a new one is appended with the
/// shard's vote. A symbol with no sort name takes an arriving one.
fn absorb_shard(
    table: &mut SymTable,
    slot: &mut Option<HashMap<Box<str>, u32>>,
    fold: bool,
    other: &Interner,
) -> Absorbed {
    let map = slot.get_or_insert_with(|| {
        (0..table.strings.len())
            .map(|sym| (table.lookup_key(sym, fold), sym as u32))
            .collect()
    });
    let mut absorbed = Absorbed {
        map: Vec::with_capacity(other.table.len()),
        moved: false,
        reordered: false,
    };
    for sym in 0..other.table.len() {
        let display = other.display(sym).to_string();
        let sort = weighted_pick(&other.sorts[sym])
            .cloned()
            .unwrap_or_default();
        let key: Box<str> = if fold {
            display.to_lowercase().into()
        } else {
            display.as_str().into()
        };
        let target = match map.get(&key) {
            Some(&known) => {
                if table.adopt_sort(known as usize, &sort) {
                    absorbed.moved = true;
                    // A known value now files elsewhere, so every row already using it moves.
                    absorbed.reordered = true;
                }
                known
            }
            None => {
                let fresh = table.push_symbol(&display, &sort);
                map.insert(key, fresh);
                absorbed.moved = true;
                fresh
            }
        };
        absorbed.map.push(target);
    }
    absorbed
}

struct Absorbed {
    map: Vec<u32>,
    /// The cached ranks are stale. Any append counts: ranks are positions.
    moved: bool,
    /// A symbol with rows took a sort name and can cross other symbols, so the
    /// caller's existing order must be rebuilt, not merged into.
    reordered: bool,
}

/// Read a set of rows into a shard for an incremental patch, through the same
/// [`Builder`] the full load uses so the two can't drift. `fold` must be the
/// live projection's, not the current setting.
///
/// The ids are deduped first: a caller can name a row twice, and
/// [`store::rows_for_ids`] would read it twice.
pub fn shard_for_ids(
    conn: &rusqlite::Connection,
    ids: &[i64],
    fold: bool,
) -> rusqlite::Result<Builder> {
    let mut ids: Vec<i64> = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut shard = Builder::new(fold);
    store::rows_for_ids(conn, &ids, |row| {
        shard.push(row);
    })?;
    shard.lay_meta_sorts(conn)?;
    Ok(shard)
}

/// The single definition behind [`Projection::is_browsable`]: radio is out.
/// Subsonic stays in because a server is a catalog with albums and artists; a
/// station is a live stream with a name.
fn source_browsable(source: &str) -> bool {
    crate::cue::Origin::of(source) != crate::cue::Origin::Radio
}

pub struct Projection {
    /// The case-insensitive setting at load time. Matching folds the same way.
    pub fold: bool,
    pub db_id: Vec<i64>,
    pub title: Arena,
    pub title_lower: Arena,
    /// None when no row has a sort title, the common case.
    title_sort: Option<(Arena, Arena)>,
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
    /// Plain columns: a symbol table would cost more than a u32 and a u8.
    pub sample_rate_hz: Vec<u32>,
    pub bit_depth: Vec<u8>,
    /// First-scan time in unix seconds, preserved across rescans.
    pub added: Vec<i64>,
    /// 0-100, 0 unrated. Atomic: a rating click writes through the shared Arc in
    /// place, so it never pays a projection reload.
    pub rating: Vec<AtomicU8>,
    /// Per ADR 11 the listens stay the source; this caches the count. Atomic for
    /// the ratings' reason.
    pub plays: Vec<AtomicU32>,
    /// Packed per [`pack_gain`] (ADR 19). Peaks stay in the database: nothing
    /// browsing sorts by them.
    pub track_gain: Vec<i16>,
    pub album_gain: Vec<i16>,
    /// Packed per [`pack_bpm`].
    pub bpm: Vec<u16>,
    pub bpm_source: Vec<crate::tempo::Source>,
    /// 0 for a plain file, the cue track number for an image span.
    pub sub: Vec<u16>,
    /// Cue spans by row index, sparse per ADR 5's memory discipline. For the
    /// player; duration is already on the row.
    pub spans: HashMap<u32, crate::cue::Span>,
    pub folder: Vec<u32>,
    pub source: Vec<u32>,
    pub artists: SymTable,
    pub album_artists: SymTable,
    pub albums: SymTable,
    pub genres: SymTable,
    pub codecs: SymTable,
    pub folders: SymTable,
    pub sources: SymTable,
    /// Filled on the first sort that needs it, reused until a patch moves the
    /// symbol table (see [`Projection::invalidate`]).
    artist_ranks: OnceLock<Vec<u32>>,
    album_artist_ranks: OnceLock<Vec<u32>>,
    album_ranks: OnceLock<Vec<u32>>,
    genre_ranks: OnceLock<Vec<u32>>,
    codec_ranks: OnceLock<Vec<u32>>,
    /// Ranks by sort name alone for the sort columns. Boxed like [`SymIndex`]:
    /// the projection travels inside a message, and three inline `OnceLock`s add
    /// a hundred bytes to every load.
    sort_ranks: OnceLock<Box<SortRanks>>,
    /// Distinct album artists and (album artist, album) pairs with their
    /// first-seen row, so per-keystroke search doesn't rescan every row. Dropped
    /// on every patch, since a patch can retire the first-seen row.
    distinct_artists: OnceLock<Vec<ArtistHit>>,
    distinct_albums: OnceLock<Vec<AlbumHit>>,
    /// Genre values with the "; " lists split, so a completion offers "Shoegaze",
    /// never "Rock; Shoegaze". Boxed like [`SymIndex`].
    genre_terms: OnceLock<Box<SymTable>>,
    /// Tombstones. The arenas are append-only, so a patch retires a row here
    /// instead of removing it. Every scan skips them, and the count says when a
    /// rebuild is due.
    dead: Vec<bool>,
    dead_rows: usize,
    /// False for a radio station. Stations are ordinary track rows so they get
    /// the queue, playlists and history, but they have no album, artist or year to
    /// browse by. Every browse surface reads this one answer.
    browsable: Vec<bool>,
    /// By source symbol, see [`Projection::hide_sources`]. Past the end is visible.
    hidden: Vec<bool>,
    /// Value to symbol per table, built on first patch. Without it an append
    /// scans every string to ask whether it knows an artist.
    sym_index: Option<Box<SymIndex>>,
}

/// Boxed so an unsorted projection carries a pointer, not three empty caches.
#[derive(Default)]
struct SortRanks {
    artists: OnceLock<Vec<u32>>,
    album_artists: OnceLock<Vec<u32>>,
    albums: OnceLock<Vec<u32>>,
}

/// Keyed like [`Interner::intern_name`]: lowered under case folding, exact
/// otherwise.
#[derive(Default)]
struct SymIndex {
    artists: Option<HashMap<Box<str>, u32>>,
    album_artists: Option<HashMap<Box<str>, u32>>,
    albums: Option<HashMap<Box<str>, u32>>,
    genres: Option<HashMap<Box<str>, u32>>,
    codecs: Option<HashMap<Box<str>, u32>>,
    folders: Option<HashMap<Box<str>, u32>>,
    sources: Option<HashMap<Box<str>, u32>>,
}

impl SymIndex {
    fn heap_bytes(&self) -> usize {
        [
            &self.artists,
            &self.album_artists,
            &self.albums,
            &self.genres,
            &self.codecs,
            &self.folders,
            &self.sources,
        ]
        .into_iter()
        .flatten()
        .map(|map| map.keys().map(|k| k.len() + 32).sum::<usize>())
        .sum()
    }
}

/// What one patch changed, so a caller can fix its id map and canonical order
/// instead of rebuilding them.
#[derive(Default, Debug)]
pub struct Patch {
    pub added: Vec<u32>,
    pub dropped: Vec<u32>,
    /// Ids that left the library. A replaced row's id isn't one of these.
    pub gone: Vec<i64>,
    /// A known value took a sort name, so existing rows changed places. An order
    /// built before this patch must be rebuilt: [`Projection::patch_order`]
    /// binary-searches it under the new ranks and would scatter rows.
    pub reordered: bool,
}

impl Patch {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.dropped.is_empty() && !self.reordered
    }
}

pub struct RowView<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album_artist: &'a str,
    pub album: &'a str,
    /// The four sort names, empty where the row or its symbol has none.
    pub title_sort: &'a str,
    pub artist_sort: &'a str,
    pub album_artist_sort: &'a str,
    pub album_sort: &'a str,
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
    pub track_gain_db: Option<f32>,
    pub album_gain_db: Option<f32>,
    pub bpm: Option<f32>,
    pub bpm_source: crate::tempo::Source,
    pub folder: &'a str,
    pub source: &'a str,
    pub sub: u16,
}

#[derive(Clone, Copy)]
pub struct ArtistHit {
    pub album_artist: u32,
    pub row: u32,
}

#[derive(Clone, Copy)]
pub struct AlbumHit {
    pub album_artist: u32,
    pub album: u32,
    pub row: u32,
}

/// Which rows a query is allowed to reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchScope {
    /// Tombstones and stations are out.
    Browse,
    /// Every live row, stations included. Hidden sources are still out.
    All,
}

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
    /// Matches the server name too, since the stored string is a digest.
    Source,
    /// Numeric pins take a comparison: `rating:>=4`, `plays:0`, `added:<90d`.
    /// Pin-only, since a bare number is a plausible title or year.
    Rating,
    Plays,
    Added,
}

impl QueryField {
    pub fn numeric(self) -> bool {
        matches!(
            self,
            QueryField::Rating | QueryField::Plays | QueryField::Added
        )
    }

    /// Whether a bare `-field` can ask for this field being absent. Folder,
    /// codec, source and added always have a value, so `-folder` stays free text.
    pub fn absence(self) -> bool {
        !matches!(
            self,
            QueryField::Folder | QueryField::Codec | QueryField::Source | QueryField::Added
        )
    }
}

/// Shared with the suggestion provider so both agree on the names.
pub const QUERY_FIELDS: &[(&str, QueryField)] = &[
    ("title", QueryField::Title),
    ("artist", QueryField::Artist),
    ("albumartist", QueryField::AlbumArtist),
    ("album", QueryField::Album),
    ("genre", QueryField::Genre),
    ("year", QueryField::Year),
    ("folder", QueryField::Folder),
    ("codec", QueryField::Codec),
    ("source", QueryField::Source),
    ("rating", QueryField::Rating),
    ("plays", QueryField::Plays),
    ("added", QueryField::Added),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumOp {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// `rating:` compares whole stars (0 unrated), `plays:` the count, and
/// `added:` an age in days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumTerm {
    pub op: NumOp,
    pub value: i64,
}

/// Keeps the matchers total. [`parse_query`] never builds one.
const NUM_NEVER: NumTerm = NumTerm {
    op: NumOp::Lt,
    value: i64::MIN,
};

impl NumTerm {
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

/// The operator defaults to equality and a trailing `d` is dropped. None
/// sends the token back to free text.
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

/// Whole stars, so `rating:` speaks the same 0-5 the star cells draw.
fn rating_stars(value: u8) -> i64 {
    if value == 0 {
        0
    } else {
        crate::rating::stars(value) as i64
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermMode {
    /// `value` or `field:value`.
    Match,
    /// `-field:value`. Only a pinned term takes this.
    Exclude,
    /// `-field`: the field is missing, per [`QueryField::absence`].
    Absent,
}

/// One parsed query term: a folded needle, maybe pinned to one field. A
/// numeric pin keeps the raw value as its needle.
pub struct Term {
    pub field: Option<QueryField>,
    pub needle: String,
    pub num: Option<NumTerm>,
    pub mode: TermMode,
}

impl Term {
    /// Absence terms are already the test they mean, so only
    /// [`TermMode::Exclude`] inverts.
    fn negated(&self) -> bool {
        self.mode == TermMode::Exclude
    }
}

/// Split a query into terms, all of which must match. Whitespace separates,
/// double quotes group, and a known `field:` prefix pins the term:
/// `stronger artist:"daft punk"`. An unknown prefix like `ac:dc` stays free
/// text.
///
/// Numeric fields take a comparison: `rating:>=4`, `plays:0`, `added:<90d`. A
/// non-numeric value falls back to free text; operators on a text field stay
/// literal.
///
/// A leading hyphen negates a pinned term (`-rating:>=4`), and a bare `-field`
/// asks for the field being absent (see [`QueryField::absence`]). Anywhere
/// else the hyphen is literal: `-stronger` looks for "-stronger".
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
    let pin = |body: &str, mode: TermMode| -> Option<Term> {
        let i = body.find(':')?;
        let name = &body[..i];
        if name.contains('"') {
            return None;
        }
        let name = name.to_lowercase();
        let &(_, field) = QUERY_FIELDS.iter().find(|(n, _)| *n == name)?;
        let needle = crate::fold::fold(&strip(&body[i + 1..]));
        let num = field.numeric().then(|| parse_num(&needle));
        if matches!(num, Some(None)) {
            return None;
        }
        Some(Term {
            field: Some(field),
            needle,
            num: num.flatten(),
            mode,
        })
    };
    tokens
        .iter()
        .map(|raw| {
            match raw.strip_prefix('-') {
                Some(body) => {
                    if let Some(term) = pin(body, TermMode::Exclude) {
                        return term;
                    }
                    let name = body.to_lowercase();
                    if let Some(&(_, field)) = QUERY_FIELDS.iter().find(|(n, _)| *n == name)
                        && field.absence()
                    {
                        return Term {
                            field: Some(field),
                            needle: String::new(),
                            num: None,
                            mode: TermMode::Absent,
                        };
                    }
                }
                None => {
                    if let Some(term) = pin(raw, TermMode::Match) {
                        return term;
                    }
                }
            }
            Term {
                field: None,
                needle: crate::fold::fold(&strip(raw)),
                num: None,
                mode: TermMode::Match,
            }
        })
        // An absence term has no needle; any other empty one said nothing.
        .filter(|t| t.mode == TermMode::Absent || !t.needle.is_empty())
        .collect()
}

/// A field the structured filter pins exact values to. Titles stay out: a
/// filter over ten million distinct titles filters nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterField {
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Year,
    Folder,
    /// The stored string, "local" or "subsonic:<digest>", so renaming a server
    /// keeps its pick.
    Source,
}

/// The filter panel's state: values OR within a field, fields AND across.
/// Whole-value matches, so "Air" leaves "Airborne" out, except folders, where
/// a pick covers its subtree. Years are decimal strings, "0" for untagged.
///
/// `ids` pins an explicit set of track ids for views following the app-wide
/// selection. Every searching panel already threads a `FilterSet` down, so
/// this covers them all. `Some` of an empty set matches nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterSet {
    pub fields: Vec<(FilterField, Vec<String>)>,
    pub ids: Option<std::collections::HashSet<i64>>,
}

impl FilterSet {
    pub fn is_empty(&self) -> bool {
        self.ids.is_none() && self.fields.iter().all(|(_, values)| values.is_empty())
    }

    /// Ignores the id pin, which has no filter chip.
    pub fn fields_empty(&self) -> bool {
        self.fields.iter().all(|(_, values)| values.is_empty())
    }

    pub fn with_ids(ids: Vec<i64>) -> Self {
        FilterSet {
            fields: Vec::new(),
            ids: Some(ids.into_iter().collect()),
        }
    }

    /// A row with no db id is in no id-keyed set, so a pin leaves it out.
    fn id_ok(&self, db_id: Option<i64>) -> bool {
        match (&self.ids, db_id) {
            (Some(ids), Some(db_id)) => ids.contains(&db_id),
            (Some(_), None) => false,
            (None, _) => true,
        }
    }

    pub fn values(&self, field: FilterField) -> &[String] {
        self.fields
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, values)| values.as_slice())
            .unwrap_or(&[])
    }

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

    pub fn clear(&mut self, field: FilterField) {
        self.fields.retain(|(f, _)| *f != field);
    }

    /// The whole-value counterpart to [`Projection::filter_mask`], for panels
    /// filtering their own rows (queue, history, playlists). Picks hold the
    /// folded tables' display casing, so a case-insensitive library compares
    /// folded.
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
                // A "Shoegaze" pick takes a "Rock; Shoegaze" track too.
                FilterField::Genre => values
                    .iter()
                    .any(|v| crate::genre::has(fields.genre, v, fold)),
                FilterField::Folder => {
                    let folder = fields.folder();
                    values.iter().any(|v| folder_in_subtree(&folder, v))
                }
                FilterField::Year => values.contains(&fields.year.to_string()),
                FilterField::Source => values.iter().any(|v| v == fields.source),
            }
        })
    }
}

/// The fields [`track_matches`] and [`FilterSet::matches`] read, for track
/// lists that aren't the projection.
pub struct TrackFields<'a> {
    /// None off-catalog; an id pin leaves those rows out.
    pub db_id: Option<i64>,
    pub title: &'a str,
    pub artist: &'a str,
    pub album_artist: &'a str,
    pub album: &'a str,
    pub genre: &'a str,
    pub year: u16,
    pub codec: &'a str,
    /// The folder is the parent directory, resolved like the projection's.
    pub path: &'a str,
    pub source: &'a str,
}

/// With a separator boundary, so "Music/Air" never pulls in "Music/Airborne".
fn folder_in_subtree(folder: &str, pick: &str) -> bool {
    folder
        .strip_prefix(pick)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(std::path::MAIN_SEPARATOR))
}

impl TrackFields<'_> {
    fn folder(&self) -> String {
        Path::new(self.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Every u16 as a decimal string, built once, so a `year:` term doesn't format
/// 65536 Strings per keystroke.
fn year_strings() -> &'static Arena {
    static YEARS: OnceLock<Arena> = OnceLock::new();
    YEARS.get_or_init(|| {
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
            arena.push(std::str::from_utf8(&buf[i..]).unwrap());
        }
        arena
    })
}

/// The haystack is a raw field, so it folds per call. An empty needle matches
/// everything.
fn contains_fold(haystack: &str, needle_folded: &str) -> bool {
    needle_folded.is_empty() || crate::fold::fold(haystack).contains(needle_folded)
}

/// Matches the shown name or the stored string, so `source:subsonic` takes
/// every server.
fn source_hit(source: &str, needle_folded: &str) -> bool {
    contains_fold(source, needle_folded)
        || contains_fold(&crate::cue::source_label(source), needle_folded)
}

/// A row a panel holding its own list (queue, playlists) filters with.
pub trait Filterable {
    fn fields(&self) -> TrackFields<'_>;

    /// Passes both the query terms and the structured filter.
    fn passes(&self, terms: &[Term], filter: &FilterSet, fold: bool) -> bool {
        let fields = self.fields();
        track_matches(terms, &fields) && filter.matches(&fields, fold)
    }
}

/// Whether one track's fields satisfy every term, the same rules as
/// [`Projection::search`].
///
/// Numeric pins match nothing here, in every mode: a plain row list has no
/// rating, plays or added columns, and a row whose rating was never seen isn't
/// known to be under four stars either.
pub fn track_matches(terms: &[Term], fields: &TrackFields) -> bool {
    terms.iter().all(|t| {
        if t.field.is_some_and(QueryField::numeric) {
            return false;
        }
        if t.mode == TermMode::Absent {
            return match t.field {
                Some(QueryField::Title) => fields.title.is_empty(),
                Some(QueryField::Artist) => fields.artist.is_empty(),
                Some(QueryField::AlbumArtist) => fields.album_artist.is_empty(),
                Some(QueryField::Album) => fields.album.is_empty(),
                Some(QueryField::Genre) => fields.genre.is_empty(),
                Some(QueryField::Year) => fields.year == 0,
                // Folder, codec and source have no absent form.
                _ => false,
            };
        }
        let hit = match t.field {
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
            Some(QueryField::Source) => source_hit(fields.source, &t.needle),
            Some(QueryField::Year) => fields.year.to_string().contains(t.needle.as_str()),
            Some(QueryField::Rating | QueryField::Plays | QueryField::Added) => false,
        };
        hit != t.negated()
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    Title,
    /// The sort title alone. Rows without one sit at the bottom in both
    /// directions, unlike [`SortKey::Title`], which falls back to the display
    /// title. The four sort keys all read this way.
    TitleSort,
    Artist,
    ArtistSort,
    AlbumArtist,
    AlbumArtistSort,
    Album,
    AlbumSort,
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
    /// The track figure, else the album one: what the engine and the Gain column
    /// read.
    TrackGain,
    /// The album figure, else the track one.
    AlbumGain,
    Bpm,
    /// Ordered by the name the source shows under.
    Source,
}

/// `order_view` reverses the whole key on a descending sort, so the groups
/// swap here to keep empties last in both directions.
fn valued_group(descending: bool) -> u8 {
    u8::from(descending)
}

fn empty_group(descending: bool) -> u8 {
    u8::from(!descending)
}

/// Rows without a sort name share the empty group and a constant rank, so
/// they fall through to the canonical tie-break.
fn sort_only_key(table: &SymTable, rank: &[u32], sym: usize, descending: bool) -> (u8, u32) {
    if table.sort_lowered(sym).is_empty() {
        (empty_group(descending), 0)
    } else {
        (valued_group(descending), rank[sym])
    }
}

impl Projection {
    /// Live and tombstoned rows: the bound for index loops.
    /// [`Projection::live_len`] is the number to show a person.
    pub fn len(&self) -> usize {
        self.db_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.db_id.is_empty()
    }

    /// Alive rows, stations included. The store's own count.
    pub fn live_len(&self) -> usize {
        self.db_id.len() - self.dead_rows
    }

    /// Alive and not a station: what every "N tracks" readout wants.
    pub fn browse_len(&self) -> usize {
        (0..self.len() as u32)
            .filter(|&row| self.is_browsable(row))
            .count()
    }

    /// For callers holding a row index from before a patch.
    pub fn is_dead(&self, row: u32) -> bool {
        self.dead.get(row as usize).copied().unwrap_or(true)
    }

    /// Alive and not a radio station. Browse surfaces ask this instead of
    /// [`Projection::is_dead`], so no panel has to know stations exist.
    pub fn is_browsable(&self, row: u32) -> bool {
        let i = row as usize;
        !self.dead.get(i).copied().unwrap_or(true)
            && self.browsable.get(i).copied().unwrap_or(false)
    }

    /// Hide a switched-off server's rows from browse and general search. They
    /// stay in the projection so playlists and history still resolve.
    ///
    /// Run on a fresh load before the canonical order is taken, since that order
    /// is built off the mask. A source a later patch brings in is visible.
    pub fn hide_sources(&mut self, hide: impl Fn(&str) -> bool) {
        let hidden: Vec<bool> = self.sources.strings.iter().map(|s| hide(s)).collect();
        self.hidden = if hidden.contains(&true) {
            hidden
        } else {
            Vec::new()
        };

        for (row, &sym) in self.source.iter().enumerate() {
            let source = &self.sources.strings[sym as usize];
            self.browsable[row] = source_browsable(source) && !self.source_hidden(sym);
        }
    }

    /// What `source:` suggests and the source filter lists.
    pub fn browse_sources(&self) -> impl Iterator<Item = &str> {
        self.sources
            .strings
            .iter()
            .enumerate()
            .filter(|(sym, s)| source_browsable(s) && !self.source_hidden(*sym as u32))
            .map(|(_, s)| s.as_str())
    }

    fn source_hidden(&self, sym: u32) -> bool {
        self.hidden.get(sym as usize).copied().unwrap_or(false)
    }

    /// Alive and not under a hidden source: general search's reach.
    fn is_listed(&self, row: u32) -> bool {
        if self.is_dead(row) {
            return false;
        }

        self.hidden.is_empty() || !self.source_hidden(self.source[row as usize])
    }

    pub fn dead_rows(&self) -> usize {
        self.dead_rows
    }

    /// The catalog rebuilds past a threshold of this: arenas and symbol tables
    /// only grow between rebuilds.
    pub fn dead_fraction(&self) -> f64 {
        if self.db_id.is_empty() {
            return 0.;
        }
        self.dead_rows as f64 / self.db_id.len() as f64
    }

    /// Browsable rows in row order, so stations are out without naming them.
    fn browse_rows(&self) -> Vec<u32> {
        (0..self.len() as u32)
            .filter(|&row| self.is_browsable(row))
            .collect()
    }

    /// Rows an empty [`SearchScope::All`] search falls back to.
    fn live_rows(&self) -> Vec<u32> {
        (0..self.len() as u32)
            .filter(|&row| self.is_listed(row))
            .collect()
    }

    /// One connection, one thread: the ADR 5 shape as written. `fold` is the
    /// case-insensitive setting.
    pub fn load_serial(conn: &rusqlite::Connection, fold: bool) -> rusqlite::Result<Self> {
        let max = store::max_rowid(conn)?;
        let mut b = Builder::new(fold);
        store::scan_range(conn, 0, max, |row| {
            b.push(row);
        })?;
        let mut projection = Self::merge(vec![b], fold);
        projection.fill_artist_sorts(conn)?;
        projection.fill_album_sorts(conn)?;
        projection.fill_track_sorts(conn)?;
        projection.fill_plays(conn)?;
        projection.fill_spans(conn)?;
        Ok(projection)
    }

    /// One reader per shard over disjoint rowid ranges (WAL allows concurrent
    /// readers), then merge by remapping symbols.
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
                        store::scan_range(&conn, lo, hi, |row| {
                            b.push(row);
                        })?;
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
        projection.fill_artist_sorts(&conn)?;
        projection.fill_album_sorts(&conn)?;
        projection.fill_track_sorts(&conn)?;
        projection.fill_plays(&conn)?;
        projection.fill_spans(&conn)?;
        Ok(projection)
    }

    /// Lay [`crate::artist_meta`] over the two artist tables. Runs after the merge,
    /// once symbols are final, and before anything reads a rank.
    fn fill_artist_sorts(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        let meta = crate::artist_meta::load_all(conn)?;
        // Nothing looked up: the tables stay as the files built them.
        if meta.is_empty() {
            return Ok(());
        }
        self.artists.fill_from_meta(&meta);
        self.album_artists.fill_from_meta(&meta);
        Ok(())
    }

    /// Lay [`crate::album_meta`] over the album table. A separate table from the
    /// artist one, so an album called "Home" never inherits a band's sort name.
    fn fill_album_sorts(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        let meta = crate::album_meta::load_all(conn)?;
        if meta.is_empty() {
            return Ok(());
        }
        self.albums.fill_from_meta(&meta);
        Ok(())
    }

    /// Lay [`crate::track_meta`] over the per-row sort titles. The arenas are
    /// append-only, so this rebuilds both, keeping a file's own sort tag where it
    /// has one. Runs after the merge has decided whether to keep the arenas.
    fn fill_track_sorts(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        let meta = crate::track_meta::load_all(conn)?;
        // Nothing romanized: the arenas stay as the files built them.
        if meta.is_empty() {
            return Ok(());
        }
        let mut display = Arena::default();
        let mut lower = Arena::default();
        for row in 0..self.db_id.len() {
            let from_file = self.title_sort(row);
            let sort = if from_file.is_empty() {
                meta.get(&self.db_id[row]).map(String::as_str).unwrap_or("")
            } else {
                from_file
            };
            // A refusal leaves what the files said.
            if !display.push(sort) || !lower.push_folded(sort) {
                return Ok(());
            }
        }
        self.title_sort = Some((display, lower));
        Ok(())
    }

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

    /// After the merge has fixed each track id's row. One query for the library:
    /// the table is empty without cue sheets.
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
        let mut sources = Interner::default();
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
        out.source.reserve(total);

        for shard in shards {
            // Drop a shard that won't fit whole: half a shard would shift every row
            // after it.
            if !out.title.fits(shard.title.bytes_len())
                || !out.title_lower.fits(shard.title_lower.bytes_len())
                || !out.title_sort.fits(shard.title_sort.bytes_len())
                || !out
                    .title_sort_lower
                    .fits(shard.title_sort_lower.bytes_len())
            {
                note_arena_overflow();
                continue;
            }
            let map_a = artists.absorb(&shard.artists);
            let map_aa = album_artists.absorb(&shard.album_artists);
            let map_b = albums.absorb(&shard.albums);
            let map_g = genres.absorb(&shard.genres);
            let map_c = codecs.absorb(&shard.codecs);
            let map_f = folders.absorb(&shard.folders);
            let map_s = sources.absorb(&shard.sources);
            out.db_id.extend_from_slice(&shard.db_id);
            out.title.append(&shard.title);
            out.title_lower.append(&shard.title_lower);
            out.title_sort.append(&shard.title_sort);
            out.title_sort_lower.append(&shard.title_sort_lower);
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
            out.source
                .extend(shard.source.iter().map(|&s| map_s[s as usize]));
        }

        let rows = out.db_id.len();
        let plays = (0..rows).map(|_| AtomicU32::new(0)).collect();
        // No sort titles anywhere, so drop the offsets. One row with one keeps both
        // arenas whole, so the search scan stays an arena `get`.
        let title_sort = if out.title_sort.is_blank() {
            None
        } else {
            Some((out.title_sort, out.title_sort_lower))
        };
        // Resolved per source symbol, a handful of checks, then read onto the rows.
        let sources = SymTable::from(sources);
        let browsable_source: Vec<bool> = sources
            .strings
            .iter()
            .map(|s| source_browsable(s))
            .collect();
        let browsable: Vec<bool> = out
            .source
            .iter()
            .map(|&sym| browsable_source[sym as usize])
            .collect();
        Projection {
            fold,
            db_id: out.db_id,
            title: out.title,
            title_lower: out.title_lower,
            title_sort,
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
            spans: HashMap::new(),
            rating: out.rating.into_iter().map(AtomicU8::new).collect(),
            plays,
            folder: out.folder,
            source: out.source,
            artists: SymTable::from(artists),
            album_artists: SymTable::from(album_artists),
            albums: SymTable::from(albums),
            genres: SymTable::from(genres),
            codecs: SymTable::from(codecs),
            folders: SymTable::from(folders),
            sources,
            artist_ranks: OnceLock::new(),
            album_artist_ranks: OnceLock::new(),
            album_ranks: OnceLock::new(),
            genre_ranks: OnceLock::new(),
            codec_ranks: OnceLock::new(),
            sort_ranks: OnceLock::new(),
            distinct_artists: OnceLock::new(),
            distinct_albums: OnceLock::new(),
            genre_terms: OnceLock::new(),
            dead: vec![false; rows],
            dead_rows: 0,
            browsable,
            hidden: Vec::new(),
            sym_index: None,
        }
    }

    pub fn resolve(&self, row: u32) -> RowView<'_> {
        let i = row as usize;
        RowView {
            title: self.title.get(i),
            artist: &self.artists.strings[self.artist[i] as usize],
            album_artist: &self.album_artists.strings[self.album_artist[i] as usize],
            album: &self.albums.strings[self.album[i] as usize],
            title_sort: self.title_sort(i),
            artist_sort: self.artists.sort_name(self.artist[i] as usize),
            album_artist_sort: self.album_artists.sort_name(self.album_artist[i] as usize),
            album_sort: self.albums.sort_name(self.album[i] as usize),
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
            source: &self.sources.strings[self.source[i] as usize],
            sub: self.sub[i],
        }
    }

    pub fn title_sort(&self, row: usize) -> &str {
        match &self.title_sort {
            Some((display, _)) => display.get(row),
            None => "",
        }
    }

    /// What the Title sort column orders by: a row without one belongs at the
    /// bottom.
    fn title_sort_lowered(&self, row: usize) -> &str {
        match &self.title_sort {
            Some((_, lower)) => lower.get(row),
            None => "",
        }
    }

    pub fn title_sort_key(&self, row: usize) -> &str {
        match &self.title_sort {
            Some((_, lower)) => match lower.get(row) {
                "" => self.title_lower.get(row),
                s => s,
            },
            None => self.title_lower.get(row),
        }
    }

    pub fn span(&self, row: u32) -> Option<crate::cue::Span> {
        self.spans.get(&row).copied()
    }

    /// Case- and accent-folded substring search per [`parse_query`]. Symbol
    /// tables are matched whole first; the row scan then only does per-title
    /// memmem plus table lookups.
    ///
    /// `added:` resolves against the clock. [`Projection::search_at`] takes a
    /// fixed timestamp.
    pub fn search(&self, query: &str) -> Vec<u32> {
        self.search_at(query, now_secs())
    }

    /// [`Projection::search`] over every live row, stations included, for the
    /// general query boxes. Stations come after the track hits.
    pub fn search_all(&self, query: &str) -> Vec<u32> {
        self.search_all_at(query, now_secs())
    }

    pub fn search_all_at(&self, query: &str, now: i64) -> Vec<u32> {
        let rows = self.search_scoped_at(query, now, SearchScope::All);
        let (mut tracks, stations): (Vec<u32>, Vec<u32>) =
            rows.into_iter().partition(|&row| self.is_browsable(row));
        tracks.extend(stations);
        tracks
    }

    /// The first local row whose artist and title match each pair exactly,
    /// folded. For radio listens, which name a song while the row behind them is
    /// the station.
    ///
    /// Exact, since a substring "Love" would play "Love Will Tear Us Apart".
    /// Batched because each lookup is a row scan; artists resolve to a mask first
    /// so most rows are rejected on an array read.
    pub fn find_locals(&self, names: &[(&str, &str)]) -> Vec<Option<u32>> {
        let folded: Vec<Option<(String, String)>> = names
            .iter()
            .map(|(artist, title)| {
                let artist = crate::fold::fold(artist.trim());
                let title = crate::fold::fold(title.trim());
                (!artist.is_empty() && !title.is_empty()).then_some((artist, title))
            })
            .collect();

        let mut wanted: HashMap<(&str, &str), Option<u32>> = folded
            .iter()
            .flatten()
            .map(|(artist, title)| ((artist.as_str(), title.as_str()), None))
            .collect();
        let Some(local) = self
            .sources
            .strings
            .iter()
            .position(|source| source == crate::cue::LOCAL)
        else {
            return vec![None; names.len()];
        };
        if wanted.is_empty() {
            return vec![None; names.len()];
        }

        // Unfolded, one name can sit under several casing symbols.
        let asked: HashSet<&str> = wanted.keys().map(|(artist, _)| *artist).collect();
        let by_artist: Vec<bool> = self
            .artists
            .lower
            .iter()
            .map(|lower| asked.contains(lower.as_str()))
            .collect();

        let local = local as u32;
        for row in 0..self.len() as u32 {
            let i = row as usize;
            if self.source[i] != local || !by_artist[self.artist[i] as usize] || self.is_dead(row) {
                continue;
            }
            let key = (
                self.artists.lower[self.artist[i] as usize].as_str(),
                self.title_lower.get(i),
            );
            if let Some(slot @ None) = wanted.get_mut(&key) {
                *slot = Some(row);
            }
        }

        folded
            .iter()
            .map(|pair| {
                let (artist, title) = pair.as_ref()?;
                wanted.get(&(artist.as_str(), title.as_str())).copied()?
            })
            .collect()
    }

    /// [`Projection::search`] with `now` in unix seconds, for `added:` terms.
    pub fn search_at(&self, query: &str, now: i64) -> Vec<u32> {
        self.search_scoped_at(query, now, SearchScope::Browse)
    }

    /// The scope only picks the liveness check and the empty-query row set.
    fn search_scoped_at(&self, query: &str, now: i64, scope: SearchScope) -> Vec<u32> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return match scope {
                SearchScope::Browse => self.browse_rows(),
                SearchScope::All => self.live_rows(),
            };
        }

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
            TitleEmpty,
            Year(Vec<bool>),
            Num(QueryField, NumTerm),
        }

        // The one place a sort name enters search: a symbol matches on its display
        // name or its sort name, so "yonezu" finds 米津玄師 through every term.
        let hit = |table: &SymTable, q: &str| -> Vec<bool> {
            if table.sort_lower.is_empty() {
                return table.lower.par_iter().map(|s| s.contains(q)).collect();
            }
            table
                .lower
                .par_iter()
                .zip(table.sort_lower.par_iter())
                .map(|(s, sort)| s.contains(q) || sort.contains(q))
                .collect()
        };
        // Absence terms become the test they mean (empty symbol, zero year, zero
        // column), leaving only exclusions to flip.
        let empty_syms = |table: &SymTable| -> Vec<bool> {
            table.strings.iter().map(|s| s.is_empty()).collect()
        };
        let absent = |field: QueryField| -> Hits {
            match field {
                QueryField::Title => Hits::TitleEmpty,
                QueryField::Artist => Hits::Sym {
                    column: &self.artist,
                    mask: empty_syms(&self.artists),
                },
                QueryField::AlbumArtist => Hits::Sym {
                    column: &self.album_artist,
                    mask: empty_syms(&self.album_artists),
                },
                QueryField::Album => Hits::Sym {
                    column: &self.album,
                    mask: empty_syms(&self.albums),
                },
                QueryField::Genre => Hits::Sym {
                    column: &self.genre,
                    mask: empty_syms(&self.genres),
                },
                QueryField::Year => {
                    let mut mask = vec![false; u16::MAX as usize + 1];
                    mask[0] = true;
                    Hits::Year(mask)
                }
                // Unrated and never played both read as a zero column.
                field => Hits::Num(
                    field,
                    NumTerm {
                        op: NumOp::Eq,
                        value: 0,
                    },
                ),
            }
        };
        let hits: Vec<(bool, Hits)> = terms
            .iter()
            .map(|t| {
                let hit = match (t.mode, t.field) {
                    (TermMode::Absent, Some(field)) => absent(field),
                    // The parser never builds a field-less absence; this takes every row.
                    _ => match t.field {
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
                        // Pin only: a bare word would drag in every path that holds it.
                        Some(QueryField::Folder) => Hits::Sym {
                            column: &self.folder,
                            mask: hit(&self.folders, &t.needle),
                        },
                        // Pin only: "flac" is a plausible title or album.
                        Some(QueryField::Codec) => Hits::Sym {
                            column: &self.codec,
                            mask: hit(&self.codecs, &t.needle),
                        },
                        Some(QueryField::Source) => Hits::Sym {
                            column: &self.source,
                            mask: self
                                .sources
                                .strings
                                .iter()
                                .map(|s| source_hit(s, &t.needle))
                                .collect(),
                        },
                        Some(QueryField::Title) => {
                            Hits::Title(memmem::Finder::new(t.needle.as_bytes()))
                        }
                        // Digit substring, so `year:199` takes the decade. Masked over the shared
                        // year arena.
                        Some(QueryField::Year) => {
                            let years = year_strings();
                            Hits::Year(
                                (0..=u16::MAX as usize)
                                    .map(|y| years.get(y).contains(&t.needle))
                                    .collect(),
                            )
                        }
                        Some(
                            field @ (QueryField::Rating | QueryField::Plays | QueryField::Added),
                        ) => Hits::Num(field, t.num.unwrap_or(NUM_NEVER)),
                    },
                };
                (t.negated(), hit)
            })
            .collect();

        self.scan_rows_in(scope, |i| {
            // `scan_rows_in` already dropped out-of-scope rows, so negation only flips
            // rows the caller would see anyway.
            hits.iter().all(|(negated, h)| {
                let hit = match h {
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
                            || self.title_hit(finder, i)
                    }
                    Hits::Sym { column, mask } => mask[column[i] as usize],
                    Hits::Title(finder) => self.title_hit(finder, i),
                    Hits::TitleEmpty => self.title_lower.get(i).is_empty(),
                    Hits::Year(mask) => mask[self.year[i] as usize],
                    Hits::Num(field, num) => num.holds(match field {
                        QueryField::Rating => rating_stars(self.rating[i].load(Ordering::Relaxed)),
                        QueryField::Plays => self.plays[i].load(Ordering::Relaxed) as i64,
                        // Days since the row was scanned in. A row with no stamp (0) is ancient and
                        // drops out of every recency term.
                        _ => (now - self.added[i]) / 86_400,
                    }),
                };
                hit != *negated
            })
        })
    }

    fn title_hit(&self, finder: &memmem::Finder<'_>, i: usize) -> bool {
        if finder.find(self.title_lower.get(i).as_bytes()).is_some() {
            return true;
        }
        match &self.title_sort {
            Some((_, lower)) => finder.find(lower.get(i).as_bytes()).is_some(),
            None => false,
        }
    }

    /// Distinct album artists whose name matches the query, with a representative
    /// row, ordered by name. A term pinned or negated on a track-only field
    /// excludes every artist: an artist isn't "not rock", some of their tracks
    /// are. Absence terms drop every artist too.
    pub fn search_artists(&self, query: &str) -> Vec<ArtistHit> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return Vec::new();
        }
        // Match the sort name too, or quick-play misses a romanized artist the
        // library panel finds.
        let matches = |name_lower: &str, sort_lower: &str| {
            terms.iter().all(|t| {
                if t.mode == TermMode::Absent {
                    return false;
                }
                let hit = match t.field {
                    None | Some(QueryField::Artist) | Some(QueryField::AlbumArtist) => {
                        name_lower.contains(&t.needle) || sort_lower.contains(&t.needle)
                    }
                    _ => return false,
                };
                hit != t.negated()
            })
        };
        let mut hits: Vec<ArtistHit> = self
            .distinct_artists()
            .iter()
            .filter(|h| {
                let sym = h.album_artist as usize;
                !self.album_artists.strings[sym].is_empty()
                    && matches(
                        &self.album_artists.lower[sym],
                        self.album_artists.sort_lowered(sym),
                    )
            })
            .copied()
            .collect();
        hits.sort_by(|a, b| {
            self.album_artists.strings[a.album_artist as usize]
                .cmp(&self.album_artists.strings[b.album_artist as usize])
        });
        hits
    }

    /// Distinct albums whose album or album-artist name matches, keyed by the
    /// (album artist, album) pair. Negation and track-only fields work as in
    /// [`Projection::search_artists`]. Ordered by artist then album.
    pub fn search_albums(&self, query: &str) -> Vec<AlbumHit> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let matches = |artist_lower: &str,
                       artist_sort: &str,
                       album_lower: &str,
                       album_sort: &str| {
            let artist = |n: &str| artist_lower.contains(n) || artist_sort.contains(n);
            let album = |n: &str| album_lower.contains(n) || album_sort.contains(n);
            terms.iter().all(|t| {
                if t.mode == TermMode::Absent {
                    return false;
                }
                let hit = match t.field {
                    None => artist(&t.needle) || album(&t.needle),
                    Some(QueryField::Album) => album(&t.needle),
                    Some(QueryField::Artist) | Some(QueryField::AlbumArtist) => artist(&t.needle),
                    _ => return false,
                };
                hit != t.negated()
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
                        self.album_artists.sort_lowered(h.album_artist as usize),
                        &self.albums.lower[album],
                        self.albums.sort_lowered(album),
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

    /// Newest first, unknown dropped. Feeds the year value completions.
    pub fn distinct_years(&self) -> Vec<u16> {
        let mut years: Vec<u16> = self
            .year
            .iter()
            .enumerate()
            .filter(|&(row, &y)| y != 0 && self.is_browsable(row as u32))
            .map(|(_, &y)| y)
            .collect();
        years.sort_unstable_by(|a, b| b.cmp(a));
        years.dedup();
        years
    }

    /// Values OR within a field, fields AND across, exact matches only. None when
    /// the filter is empty, so callers skip the scan.
    pub fn filter_mask(&self, filter: &FilterSet) -> Option<Vec<bool>> {
        if filter.is_empty() {
            return None;
        }

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
                // Genre symbols are "; " lists; a pick passes any list holding it.
                FilterField::Genre => Check::Sym {
                    column: &self.genre,
                    ok: self
                        .genres
                        .strings
                        .iter()
                        .map(|s| values.iter().any(|v| crate::genre::has(s, v, fold)))
                        .collect(),
                },
                FilterField::Folder => Check::Sym {
                    column: &self.folder,
                    ok: self
                        .folders
                        .strings
                        .iter()
                        .map(|s| values.iter().any(|v| folder_in_subtree(s, v)))
                        .collect(),
                },
                FilterField::Source => Check::Sym {
                    column: &self.source,
                    ok: self
                        .sources
                        .strings
                        .iter()
                        .map(|s| values.iter().any(|v| v == s))
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

        // A set, or the whole-catalog scan goes quadratic.
        let pinned = filter.ids.as_ref();

        Some(
            (0..self.len())
                .into_par_iter()
                .map(|i| {
                    // Masks are indexed by row, so a stale view intersected with one still can't
                    // reach a dead row or a station.
                    if !self.is_browsable(i as u32) {
                        return false;
                    }
                    if let Some(pinned) = pinned
                        && !pinned.contains(&self.db_id[i])
                    {
                        return false;
                    }
                    checks.iter().all(|c| match c {
                        Check::Sym { column, ok } => ok[column[i] as usize],
                        Check::Year(ok) => ok[self.year[i] as usize],
                    })
                })
                .collect(),
        )
    }

    /// "Shoegaze" takes a "Rock; Shoegaze" track too.
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

    /// Parallel scan in fixed chunks; chunk order keeps row order. Browse scope.
    fn scan_rows(&self, pred: impl Fn(usize) -> bool + Sync) -> Vec<u32> {
        self.scan_rows_in(SearchScope::Browse, pred)
    }

    fn scan_rows_in(&self, scope: SearchScope, pred: impl Fn(usize) -> bool + Sync) -> Vec<u32> {
        let n = self.len();
        let live = |i: usize| match scope {
            SearchScope::Browse => self.is_browsable(i as u32),
            SearchScope::All => self.is_listed(i as u32),
        };
        let chunks = n.div_ceil(CHUNK);
        let per: Vec<Vec<u32>> = (0..chunks)
            .into_par_iter()
            .map(|c| {
                let start = c * CHUNK;
                let end = (start + CHUNK).min(n);
                let mut out = Vec::new();
                for i in start..end {
                    if live(i) && pred(i) {
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

    /// Alphabetical rank per symbol on its sort key, so sort comparisons are
    /// integer. Ties break on symbol id: the sort is unstable and parallel, so
    /// without it a recompute could swap "Tie" and "tie" when nothing changed.
    fn ranks(table: &SymTable) -> Vec<u32> {
        let mut order: Vec<u32> = (0..table.strings.len() as u32).collect();
        order.par_sort_unstable_by(|&a, &b| {
            table.sort_key(a as usize).cmp(table.sort_key(b as usize))
        });
        let mut rank = vec![0u32; order.len()];
        for (pos, &sym) in order.iter().enumerate() {
            rank[sym as usize] = pos as u32;
        }
        rank
    }

    /// Rank among symbols with a sort name. The rest read 0, and
    /// [`sort_only_key`] answers for them before indexing.
    fn sort_only_ranks(table: &SymTable) -> Vec<u32> {
        let mut order: Vec<u32> = (0..table.strings.len() as u32)
            .filter(|&sym| !table.sort_lowered(sym as usize).is_empty())
            .collect();
        order.par_sort_unstable_by(|&a, &b| {
            table
                .sort_lowered(a as usize)
                .cmp(table.sort_lowered(b as usize))
                .then(a.cmp(&b))
        });
        let mut rank = vec![0u32; table.strings.len()];
        for (pos, &sym) in order.iter().enumerate() {
            rank[sym as usize] = pos as u32;
        }
        rank
    }

    // Ranked once on the first sort that needs them. Every tie-break needs the
    // album artist and album ranks.
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

    /// Not cached: a server rename changes the name under a standing projection.
    fn source_ranks(&self) -> Vec<u32> {
        let names: Vec<String> = self
            .sources
            .strings
            .iter()
            .map(|s| crate::fold::fold(&crate::cue::source_label(s)))
            .collect();

        let mut order: Vec<u32> = (0..names.len() as u32).collect();
        order.sort_by(|&a, &b| names[a as usize].cmp(&names[b as usize]));

        let mut rank = vec![0u32; order.len()];
        for (pos, &sym) in order.iter().enumerate() {
            rank[sym as usize] = pos as u32;
        }

        rank
    }
    fn artist_sort_ranks(&self) -> &[u32] {
        let cache = self.sort_ranks.get_or_init(Box::default);
        cache
            .artists
            .get_or_init(|| Self::sort_only_ranks(&self.artists))
    }
    fn album_artist_sort_ranks(&self) -> &[u32] {
        let cache = self.sort_ranks.get_or_init(Box::default);
        cache
            .album_artists
            .get_or_init(|| Self::sort_only_ranks(&self.album_artists))
    }
    fn album_sort_ranks(&self) -> &[u32] {
        let cache = self.sort_ranks.get_or_init(Box::default);
        cache
            .albums
            .get_or_init(|| Self::sort_only_ranks(&self.albums))
    }

    /// Cached so the O(rows) distinct pass runs once, not per keystroke.
    fn distinct_artists(&self) -> &[ArtistHit] {
        self.distinct_artists.get_or_init(|| {
            let mut seen: HashSet<u32> = HashSet::new();
            let mut out: Vec<ArtistHit> = Vec::new();
            for row in 0..self.len() as u32 {
                if !self.is_browsable(row) {
                    continue;
                }
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

    /// Distinct genre values with "; " lists split, in first-seen order. A folded
    /// library merges case variants to the most common casing.
    pub fn genre_terms(&self) -> &SymTable {
        self.genre_terms.get_or_init(|| {
            // Browsable rows per symbol, which also keeps a station's genre out of the
            // suggestions.
            let mut rows = vec![0u32; self.genres.strings.len()];
            for (row, &sym) in self.genre.iter().enumerate() {
                if self.is_browsable(row as u32) {
                    rows[sym as usize] += 1;
                }
            }
            if !self.fold {
                let mut seen: HashSet<String> = HashSet::new();
                let mut strings: Vec<String> = Vec::new();
                for (sym, s) in self.genres.strings.iter().enumerate() {
                    if rows[sym] == 0 {
                        continue;
                    }
                    for part in crate::genre::split(s) {
                        let part = crate::genre::resolve(part);
                        if seen.insert(part.clone()) {
                            strings.push(part);
                        }
                    }
                }
                let lower = strings.iter().map(|s| crate::fold::fold(s)).collect();
                return Box::new(SymTable {
                    strings,
                    lower,
                    sort: Vec::new(),
                    sort_lower: Vec::new(),
                });
            }
            let mut order: Vec<String> = Vec::new();
            let mut casings: HashMap<String, HashMap<String, u32>> = HashMap::new();
            for (sym, s) in self.genres.strings.iter().enumerate() {
                if rows[sym] == 0 {
                    continue;
                }
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
            let lower = strings.iter().map(|s| crate::fold::fold(s)).collect();
            Box::new(SymTable {
                strings,
                lower,
                sort: Vec::new(),
                sort_lower: Vec::new(),
            })
        })
    }

    /// Cached like [`Projection::distinct_artists`].
    fn distinct_albums(&self) -> &[AlbumHit] {
        self.distinct_albums.get_or_init(|| {
            let mut seen: HashSet<u64> = HashSet::new();
            let mut out: Vec<AlbumHit> = Vec::new();
            for row in 0..self.len() as u32 {
                if !self.is_browsable(row) {
                    continue;
                }
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

    /// Album artist, album, disc, track number. The album artist keeps an album
    /// one run despite guest credits; disc before track keeps multi-disc sets in
    /// order.
    pub fn sort_canonical(&self) -> Vec<u32> {
        let a_rank = self.album_artist_ranks();
        let b_rank = self.album_ranks();
        let mut idx = self.browse_rows();
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
        let mut idx = self.browse_rows();
        idx.par_sort_unstable_by(|&a, &b| {
            self.title_sort_key(a as usize)
                .cmp(self.title_sort_key(b as usize))
        });
        idx
    }

    pub fn sort_year(&self) -> Vec<u32> {
        let mut idx = self.browse_rows();
        idx.par_sort_unstable_by_key(|&i| self.year[i as usize]);
        idx
    }

    /// Ties fall back to the canonical order; descending reverses the key alone,
    /// not the tie-break.
    pub fn sort_view(&self, view: &[u32], key: SortKey, descending: bool) -> Vec<u32> {
        match key {
            SortKey::Title => self.order_view(view, descending, |i| self.title_sort_key(i)),
            // A row with no sort title takes the empty group's key and skips the string
            // comparison.
            SortKey::TitleSort => self.order_view(view, descending, move |i| {
                let sort = self.title_sort_lowered(i);
                if sort.is_empty() {
                    (empty_group(descending), "")
                } else {
                    (valued_group(descending), sort)
                }
            }),
            SortKey::Artist => {
                let rank = self.artist_ranks();
                self.order_view(view, descending, move |i| rank[self.artist[i] as usize])
            }
            SortKey::ArtistSort => {
                let rank = self.artist_sort_ranks();
                self.order_view(view, descending, move |i| {
                    sort_only_key(&self.artists, rank, self.artist[i] as usize, descending)
                })
            }
            SortKey::AlbumArtist => {
                let rank = self.album_artist_ranks();
                self.order_view(view, descending, move |i| {
                    rank[self.album_artist[i] as usize]
                })
            }
            SortKey::AlbumArtistSort => {
                let rank = self.album_artist_sort_ranks();
                self.order_view(view, descending, move |i| {
                    sort_only_key(
                        &self.album_artists,
                        rank,
                        self.album_artist[i] as usize,
                        descending,
                    )
                })
            }
            SortKey::Album => {
                let rank = self.album_ranks();
                self.order_view(view, descending, move |i| rank[self.album[i] as usize])
            }
            SortKey::AlbumSort => {
                let rank = self.album_sort_ranks();
                self.order_view(view, descending, move |i| {
                    sort_only_key(&self.albums, rank, self.album[i] as usize, descending)
                })
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
            SortKey::Source => {
                let rank = self.source_ranks();
                self.order_view(view, descending, move |i| rank[self.source[i] as usize])
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
            // NO_GAIN is the floor, so untagged rows come first ascending.
            SortKey::TrackGain => self.order_view(view, descending, |i| self.gain_key(i, false)),
            SortKey::AlbumGain => self.order_view(view, descending, |i| self.gain_key(i, true)),
            // NO_BPM is zero, so rows with no tempo come first ascending.
            SortKey::Bpm => self.order_view(view, descending, |i| self.bpm[i]),
        }
    }

    /// The mode's figure, else the other. `album_first` is the Album mode. The
    /// same pick [`crate::replaygain`] hands the engine, before preamp and clamp.
    pub fn gain_db(&self, row: u32, album_first: bool) -> Option<f32> {
        unpack_gain(self.gain_key(row as usize, album_first))
    }

    fn gain_key(&self, i: usize, album_first: bool) -> i16 {
        let (first, second) = if album_first {
            (self.album_gain[i], self.track_gain[i])
        } else {
            (self.track_gain[i], self.album_gain[i])
        };
        if first != NO_GAIN { first } else { second }
    }

    /// Integer ranks everywhere except titles, which compare lowered strings:
    /// cheaper for a subset than ranking every title.
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
        // A view built before a patch and sorted after can hold a retired row.
        let mut idx: Vec<u32> = if self.dead_rows == 0 {
            view.to_vec()
        } else {
            view.iter()
                .copied()
                .filter(|&row| !self.dead[row as usize])
                .collect()
        };
        idx.par_sort_unstable_by(|&a, &b| {
            let (a, b) = (a as usize, b as usize);
            let ord = primary(a).cmp(&primary(b));
            let ord = if descending { ord.reverse() } else { ord };
            ord.then_with(|| canonical(a).cmp(&canonical(b)))
        });
        idx
    }

    /// Fold freshly read rows into the projection in place. The row an id had is
    /// tombstoned and the fresh one appended, so the columns only grow and every
    /// row index a panel holds stays valid.
    ///
    /// `index` is the caller's id-to-row map before the patch. The returned
    /// [`Patch`] lets the caller fix that map and the order through
    /// [`Projection::patch_order`].
    ///
    /// None means refused, fall back to a full rebuild: the shard lost rows to the
    /// arena ceiling, or the text doesn't fit.
    ///
    /// Left to the next rebuild: a known value keeps its last-voted display
    /// casing, and a symbol with only dead rows stays in the table.
    pub fn apply_upserts(
        &mut self,
        shard: Builder,
        index: &HashMap<i64, u32>,
        plays: &HashMap<i64, u32>,
        spans: &HashMap<i64, crate::cue::Span>,
    ) -> Option<Patch> {
        if shard.overflowed {
            return None;
        }
        if shard.db_id.is_empty() {
            return Some(Patch::default());
        }
        // The first sort title in the library turns the arenas on, padding every
        // existing row with an empty entry.
        if self.title_sort.is_none() && !shard.title_sort.is_blank() {
            let mut display = Arena::default();
            let mut lower = Arena::default();
            for _ in 0..self.len() {
                display.push("");
                lower.push("");
            }
            self.title_sort = Some((display, lower));
        }
        // Check room for the whole shard up front: a row refused halfway leaves the
        // columns out of step.
        let sort_fits = match &self.title_sort {
            Some((display, lower)) => {
                display.fits(shard.title_sort.bytes_len())
                    && lower.fits(shard.title_sort_lower.bytes_len())
            }
            None => true,
        };
        if !sort_fits
            || !self.title.fits(shard.title.bytes_len())
            || !self.title_lower.fits(shard.title_lower.bytes_len())
        {
            note_arena_overflow();
            return None;
        }

        let fold = self.fold;
        let mut sym = self.sym_index.take().unwrap_or_default();
        let a = absorb_shard(&mut self.artists, &mut sym.artists, fold, &shard.artists);
        let aa = absorb_shard(
            &mut self.album_artists,
            &mut sym.album_artists,
            fold,
            &shard.album_artists,
        );
        let b = absorb_shard(&mut self.albums, &mut sym.albums, fold, &shard.albums);
        let g = absorb_shard(&mut self.genres, &mut sym.genres, fold, &shard.genres);
        let c = absorb_shard(&mut self.codecs, &mut sym.codecs, false, &shard.codecs);
        let f = absorb_shard(&mut self.folders, &mut sym.folders, false, &shard.folders);
        let s = absorb_shard(&mut self.sources, &mut sym.sources, false, &shard.sources);
        self.sym_index = Some(sym);
        let tables = [&a, &aa, &b, &g, &c, &f, &s];
        let symbols_moved = tables.iter().any(|t| t.moved);
        // Every table: a caller may keep an order sorted on any of them.
        let reordered = tables.iter().any(|t| t.reordered);
        let (map_a, map_aa, map_b, map_g, map_c, map_f, map_s) =
            (a.map, aa.map, b.map, g.map, c.map, f.map, s.map);

        let mut patch = Patch {
            reordered,
            ..Patch::default()
        };
        for i in 0..shard.db_id.len() {
            let id = shard.db_id[i];
            let row = self.db_id.len() as u32;
            self.db_id.push(id);
            // The shard already lowered its text.
            self.title.push(shard.title.get(i));
            self.title_lower.push(shard.title_lower.get(i));
            if let Some((display, lower)) = &mut self.title_sort {
                display.push(shard.title_sort.get(i));
                lower.push(shard.title_sort_lower.get(i));
            }
            self.artist.push(map_a[shard.artist[i] as usize]);
            self.album_artist
                .push(map_aa[shard.album_artist[i] as usize]);
            self.album.push(map_b[shard.album[i] as usize]);
            self.genre.push(map_g[shard.genre[i] as usize]);
            self.year.push(shard.year[i]);
            self.disc_no.push(shard.disc_no[i]);
            self.track_no.push(shard.track_no[i]);
            self.duration_ms.push(shard.duration_ms[i]);
            self.codec.push(map_c[shard.codec[i] as usize]);
            self.bitrate_kbps.push(shard.bitrate_kbps[i]);
            self.sample_rate_hz.push(shard.sample_rate_hz[i]);
            self.bit_depth.push(shard.bit_depth[i]);
            self.rating.push(AtomicU8::new(shard.rating[i]));
            self.plays
                .push(AtomicU32::new(plays.get(&id).copied().unwrap_or(0)));
            self.added.push(shard.added[i]);
            self.track_gain.push(shard.track_gain[i]);
            self.album_gain.push(shard.album_gain[i]);
            self.bpm.push(shard.bpm[i]);
            self.bpm_source.push(shard.bpm_source[i]);
            self.sub.push(shard.sub[i]);
            self.folder.push(map_f[shard.folder[i] as usize]);
            let source = map_s[shard.source[i] as usize];
            self.source.push(source);
            self.dead.push(false);
            self.browsable.push(
                source_browsable(&self.sources.strings[source as usize])
                    && !self.source_hidden(source),
            );
            if let Some(&span) = spans.get(&id) {
                self.spans.insert(row, span);
            }
            if let Some(&old) = index.get(&id)
                && !self.dead[old as usize]
            {
                self.dead[old as usize] = true;
                self.dead_rows += 1;
                self.spans.remove(&old);
                patch.dropped.push(old);
            }
            patch.added.push(row);
        }
        self.invalidate(symbols_moved);
        Some(patch)
    }

    /// Tombstone the rows for ids whose files are gone.
    pub fn remove_ids(&mut self, ids: &[i64], index: &HashMap<i64, u32>) -> Patch {
        let mut patch = Patch::default();
        for &id in ids {
            let Some(&row) = index.get(&id) else {
                continue;
            };
            if self.dead[row as usize] {
                continue;
            }
            self.dead[row as usize] = true;
            self.dead_rows += 1;
            self.spans.remove(&row);
            patch.dropped.push(row);
            patch.gone.push(id);
        }
        if !patch.dropped.is_empty() {
            // A removal never moves a symbol; the next rebuild sweeps leftovers.
            self.invalidate(false);
        }
        patch
    }

    /// The canonical order with a patch merged in, in one pass.
    ///
    /// Only valid while `order` is still sorted under the current ranks, which
    /// [`Patch::reordered`] answers. A reordered patch needs a
    /// [`Projection::sort_canonical`] instead.
    pub fn patch_order(&self, order: &[u32], patch: &Patch) -> Vec<u32> {
        let a_rank = self.album_artist_ranks();
        let b_rank = self.album_ranks();
        let key = |row: u32| {
            let i = row as usize;
            (
                a_rank[self.album_artist[i] as usize],
                b_rank[self.album[i] as usize],
                self.disc_no[i],
                self.track_no[i],
            )
        };
        // Binary search to the run of rows sharing the key, then walk the run. A
        // full walk costs two random rank reads per row, more than the rest of the
        // patch. The linear fallback exists because a dropped row left in the order
        // is a tombstone on screen.
        let position_of = |row: u32| -> Option<usize> {
            let here = key(row);
            let start = order.partition_point(|&other| key(other) < here);
            for (at, &other) in order[start..].iter().enumerate() {
                if other == row {
                    return Some(start + at);
                }
                if key(other) != here {
                    break;
                }
            }
            order.iter().position(|&other| other == row)
        };

        // Stations never entered the order.
        let mut fresh: Vec<u32> = patch
            .added
            .iter()
            .copied()
            .filter(|&row| self.browsable[row as usize])
            .collect();
        fresh.sort_unstable_by_key(|&row| key(row));
        let mut events: Vec<(usize, Option<u32>)> =
            Vec::with_capacity(fresh.len() + patch.dropped.len());
        for &row in &fresh {
            let here = key(row);
            events.push((order.partition_point(|&other| key(other) < here), Some(row)));
        }
        for &row in &patch.dropped {
            if !self.browsable[row as usize] {
                continue;
            }
            if let Some(at) = position_of(row) {
                events.push((at, None));
            }
        }
        // At the same index the insert goes first. Stable, so inserts between the
        // same pair keep their key order.
        events.sort_by_key(|&(at, row)| (at, row.is_none()));

        let mut out = Vec::with_capacity(order.len() + patch.added.len());
        let mut cursor = 0;
        for (at, row) in events {
            if at > cursor {
                out.extend_from_slice(&order[cursor..at]);
                cursor = at;
            }
            match row {
                Some(row) => out.push(row),
                None => cursor = at + 1,
            }
        }
        out.extend_from_slice(&order[cursor.min(order.len())..]);
        out
    }

    /// Drop the memoized tables a patch just falsified. The distinct lists and
    /// genre terms go every time, since they name a first-seen row. The ranks go
    /// only when a symbol table moved.
    fn invalidate(&mut self, symbols_moved: bool) {
        self.distinct_artists = OnceLock::new();
        self.distinct_albums = OnceLock::new();
        self.genre_terms = OnceLock::new();
        if symbols_moved {
            self.artist_ranks = OnceLock::new();
            self.album_artist_ranks = OnceLock::new();
            self.album_ranks = OnceLock::new();
            self.genre_ranks = OnceLock::new();
            self.codec_ranks = OnceLock::new();
            self.sort_ranks = OnceLock::new();
        }
    }

    pub fn heap_bytes(&self) -> usize {
        (self.db_id.capacity() + self.added.capacity()) * 8
            + self.title.heap_bytes()
            + self.title_lower.heap_bytes()
            + self
                .title_sort
                .as_ref()
                .map_or(0, |(d, l)| d.heap_bytes() + l.heap_bytes())
            + (self.artist.capacity()
                + self.album_artist.capacity()
                + self.album.capacity()
                + self.genre.capacity()
                + self.codec.capacity()
                + self.folder.capacity()
                + self.source.capacity())
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
            + self.sources.heap_bytes()
            + self.dead.capacity()
            + self.browsable.capacity()
            + self.hidden.capacity()
            + self.sym_index.as_ref().map_or(0, |i| i.heap_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TrackRow, listens};

    fn row(path: &str, album: &str, disc_no: u16, track_no: u16) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
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
            remote_url: String::new(),
            remote_live: false,
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
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

    /// A track number so otherwise-tied rows order deterministically.
    fn track_no(path: &str, title: &str, artist: &str, no: u16) -> TrackRow {
        let mut row = track(path, title, artist, 2018);
        row.track_no = no;
        row
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
        assert_eq!((terms[2].field, terms[2].needle.as_str()), (None, "ac:dc"));
        assert_eq!(
            (terms[3].field, terms[3].needle.as_str()),
            (Some(QueryField::Year), "199")
        );
    }

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
        assert_eq!(titles("rating:>=4 plays:0"), ["Loved"]);
        assert_eq!(titles("-rating:>=4"), ["Plain"]);
        assert_eq!(titles("-plays:0"), ["Liked"]);
        assert_eq!(titles("-added:<90d"), ["Liked", "Plain"]);
        assert_eq!(titles("-rating:0"), ["Loved", "Liked"]);
    }

    #[test]
    fn query_parses_hyphen_terms() {
        let terms = parse_query("-year");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].field, Some(QueryField::Year));
        assert_eq!(terms[0].mode, TermMode::Absent);
        assert!(terms[0].needle.is_empty(), "an absence carries no needle");

        let terms = parse_query("-genre:rock");
        assert_eq!(
            (terms[0].field, terms[0].needle.as_str(), terms[0].mode),
            (Some(QueryField::Genre), "rock", TermMode::Exclude)
        );
        let terms = parse_query(r#"-artist:"Daft Punk""#);
        assert_eq!(
            (terms[0].field, terms[0].needle.as_str(), terms[0].mode),
            (Some(QueryField::Artist), "daft punk", TermMode::Exclude)
        );
        let terms = parse_query("-rating:>=4");
        assert_eq!(terms[0].field, Some(QueryField::Rating));
        assert_eq!(
            terms[0].num,
            Some(NumTerm {
                op: NumOp::Ge,
                value: 4
            })
        );
        assert_eq!(terms[0].mode, TermMode::Exclude);

        for query in ["-foo", "-folder", "-codec", "-added", "-", "-stronger"] {
            let terms = parse_query(query);
            assert_eq!(terms.len(), 1, "{query} is one term");
            assert_eq!(
                (terms[0].field, terms[0].needle.as_str(), terms[0].mode),
                (None, query, TermMode::Match),
                "{query} stays free text"
            );
        }
        for query in ["-foo:bar", "-rating:great"] {
            let terms = parse_query(query);
            assert_eq!(
                (terms[0].field, terms[0].needle.as_str(), terms[0].mode),
                (None, query, TermMode::Match),
                "{query} stays free text"
            );
        }
    }

    fn hyphen_rows() -> Projection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let mut tagged = track("/m/1.mp3", "Tagged", "A", 2001);
        tagged.album_artist = "A".into();
        tagged.album = "Discovery".into();
        tagged.genre = "Electronic".into();
        tagged.rating = 100;
        let bare = track("/m/2.mp3", "Bare", "", 0);
        let mut untitled = track("/m/3.mp3", "", "C", 1999);
        untitled.album_artist = "C".into();
        untitled.album = "Later".into();
        untitled.genre = "Rock".into();
        untitled.rating = 60;
        store::insert_batch(&mut conn, &[tagged, bare, untitled]).unwrap();
        listens::append(
            &conn,
            &listens::Listen {
                track_id: 1,
                played_at: 1_700_000_000,
                title: "Tagged".into(),
                artist: "A".into(),
                album: "Discovery".into(),
                genre: "Electronic".into(),
                path: "/m/1.mp3".into(),
            },
        )
        .unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    #[test]
    fn absence_terms_find_the_missing_values() {
        let p = hyphen_rows();
        for query in ["-year", "-genre", "-artist", "-albumartist", "-album"] {
            assert_eq!(titles_for(&p, query), ["Bare"], "{query}");
        }
        assert_eq!(titles_for(&p, "-rating"), ["Bare"], "unrated is absent");
        assert_eq!(titles_for(&p, "-plays"), ["Bare", ""], "never played");
        assert_eq!(titles_for(&p, "-title"), [""]);
        assert_eq!(titles_for(&p, "-year -rating"), ["Bare"]);
        for query in ["-folder", "-codec", "-added"] {
            assert!(titles_for(&p, query).is_empty(), "{query}");
        }
    }

    #[test]
    fn exclusion_terms_invert_their_field() {
        let mut p = hyphen_rows();
        assert_eq!(titles_for(&p, "genre:rock"), [""]);
        assert_eq!(titles_for(&p, "-genre:rock"), ["Tagged", "Bare"]);
        assert_eq!(titles_for(&p, "-artist:a"), ["Bare", ""]);
        assert_eq!(titles_for(&p, "-title:tagged"), ["Bare", ""]);
        assert_eq!(titles_for(&p, "-year:19"), ["Tagged", "Bare"]);
        assert_eq!(titles_for(&p, "-rating:>=4"), ["Bare", ""]);
        assert_eq!(titles_for(&p, "-genre:rock -rating:>=4"), ["Bare"]);

        let index: HashMap<i64, u32> = p
            .db_id
            .iter()
            .enumerate()
            .map(|(row, id)| (*id, row as u32))
            .collect();
        let gone = p.db_id[1];
        p.remove_ids(&[gone], &index);
        assert_eq!(titles_for(&p, "-genre:rock"), ["Tagged"]);
        assert!(titles_for(&p, "-year").is_empty());
    }

    #[test]
    fn hyphen_terms_agree_between_the_matchers() {
        let p = hyphen_rows();
        let rows = [
            ("Tagged", "A", "A", "Discovery", "Electronic", 2001u16),
            ("Bare", "", "", "", "", 0),
            ("", "C", "C", "Later", "Rock", 1999),
        ];
        let queries = [
            "-genre:rock",
            "-artist:a",
            "-title:tagged",
            "-year:19",
            "-genre",
            "-year",
            "-title",
            "-album",
            "-albumartist",
            "-genre:rock -year:19",
        ];
        for query in queries {
            let terms = parse_query(query);
            let matched: Vec<String> = rows
                .iter()
                .filter(|r| {
                    track_matches(
                        &terms,
                        &TrackFields {
                            db_id: None,
                            title: r.0,
                            artist: r.1,
                            album_artist: r.2,
                            album: r.3,
                            genre: r.4,
                            year: r.5,
                            codec: "mp3",
                            path: "/m/x.mp3",
                            source: "local",
                        },
                    )
                })
                .map(|r| r.0.to_string())
                .collect();
            assert_eq!(titles_for(&p, query), matched, "{query}");
        }
        // Projection-only columns: a negated miss stays a miss.
        let queue_row = TrackFields {
            db_id: None,
            title: "Bare",
            artist: "",
            album_artist: "",
            album: "",
            genre: "",
            year: 0,
            codec: "mp3",
            path: "/m/x.mp3",
            source: "local",
        };
        assert!(!track_matches(&parse_query("-rating:>=4"), &queue_row));
        assert!(!track_matches(&parse_query("-rating"), &queue_row));
        assert!(!track_matches(&parse_query("-plays"), &queue_row));
    }

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

        assert_eq!(titles_for(&p, "daft").len(), 2);
        let hits = p.search(r#"stronger artist:"daft punk""#);
        assert_eq!(hits.len(), 1);
        assert_eq!(p.resolve(hits[0]).artist, "Daft Punk");
        assert_eq!(titles_for(&p, "year:200").len(), 2);
        assert_eq!(titles_for(&p, "stronger year:2007").len(), 1);
    }

    /// An off-catalog row has no db id, so an id pin leaves it out.
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
                    source: "local",
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
        assert!(!known.passes(&parse_query("harder"), &none, false));

        let pinned = FilterSet::with_ids(vec![7]);
        assert!(known.passes(&terms, &pinned, false));
        assert!(!dropped.passes(&terms, &pinned, false));
    }

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
            source: "local",
        };
        assert!(track_matches(&parse_query("stronger"), &fields));
        assert!(track_matches(&parse_query("DAFT"), &fields));
        assert!(track_matches(&parse_query("electronic"), &fields));
        assert!(!track_matches(&parse_query("kanye"), &fields));
        assert!(track_matches(&parse_query("stronger daft"), &fields));
        assert!(!track_matches(&parse_query("stronger kanye"), &fields));
        assert!(track_matches(
            &parse_query(r#"artist:"daft punk""#),
            &fields
        ));
        assert!(!track_matches(&parse_query("title:discovery"), &fields));
        assert!(track_matches(&parse_query("year:200"), &fields));
        assert!(track_matches(&parse_query("folder:discovery"), &fields));
        assert!(!track_matches(&parse_query("folder:other"), &fields));
        assert!(track_matches(&parse_query("codec:FLAC"), &fields));
        assert!(!track_matches(&parse_query("codec:mp3"), &fields));
        assert!(!track_matches(&parse_query("flac"), &fields));

        let mut filter = FilterSet::default();
        filter.toggle(FilterField::Artist, "Daft Punk");
        assert!(filter.matches(&fields, false));
        let mut narrower = filter.clone();
        narrower.toggle(FilterField::Artist, "Air");
        assert!(narrower.matches(&fields, false));
        let mut year = FilterSet::default();
        year.toggle(FilterField::Year, "2001");
        assert!(year.matches(&fields, false));
        year.clear(FilterField::Year);
        year.toggle(FilterField::Year, "1999");
        assert!(!year.matches(&fields, false));
    }

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

        assert_eq!(titles_for(&p, r#"folder:"wrong album""#).len(), 2);
        assert_eq!(titles_for(&p, "folder:music").len(), 3);
        assert_eq!(titles_for(&p, r#"one folder:"wrong album""#).len(), 1);
        assert!(titles_for(&p, "other").is_empty());
    }

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

        let parallel = Projection::load_parallel(&db, 3, false).unwrap();
        assert_eq!(parallel.sample_rate_hz, p.sample_rate_hz);
        assert_eq!(parallel.bit_depth, p.bit_depth);
    }

    fn sorted_library(name: &str, rows: &[TrackRow]) -> (std::path::PathBuf, rusqlite::Connection) {
        let dir = std::env::temp_dir().join(format!("rox-projection-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.db");
        let mut conn = store::open(&db).unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        (db, conn)
    }

    /// The only test that fills the source name table.
    #[test]
    fn a_source_is_searched_filtered_and_sorted_by_name() {
        let (_db, mut conn) = sorted_library(
            "by-source",
            &[track("/m/one.flac", "So What", "Miles Davis", 1959)],
        );
        let mut remote = track("sg-4", "Blue in Green", "Miles Davis", 1959);
        remote.remote_url = "https://home/stream/4".into();
        store::upsert_source_rows(&mut conn, "subsonic:aaa", &[remote]).unwrap();

        crate::cue::set_source_labels(HashMap::from([
            ("local".to_string(), "Local".to_string()),
            ("subsonic:aaa".to_string(), "Home".to_string()),
        ]));
        let p = Projection::load_serial(&conn, false).unwrap();

        let titles = |rows: Vec<u32>| -> Vec<String> {
            rows.into_iter()
                .map(|row| p.resolve(row).title.to_string())
                .collect()
        };

        assert_eq!(titles(p.search("source:home")), ["Blue in Green"]);
        assert_eq!(titles(p.search("source:subsonic")), ["Blue in Green"]);
        assert_eq!(titles(p.search("source:local")), ["So What"]);
        assert!(p.search("home").is_empty(), "a pin, never a free term");

        let mut filter = FilterSet::default();
        filter.toggle(FilterField::Source, "subsonic:aaa");
        let mask = p.filter_mask(&filter).expect("a live filter");
        let picked: Vec<u32> = (0..p.len() as u32).filter(|&r| mask[r as usize]).collect();
        assert_eq!(titles(picked), ["Blue in Green"]);

        let all: Vec<u32> = (0..p.len() as u32).collect();
        assert_eq!(
            titles(p.sort_view(&all, SortKey::Source, false)),
            ["Blue in Green", "So What"]
        );

        let fields = TrackFields {
            db_id: None,
            title: "Blue in Green",
            artist: "Miles Davis",
            album_artist: "",
            album: "",
            genre: "",
            year: 1959,
            codec: "",
            path: "sg-4",
            source: "subsonic:aaa",
        };
        assert!(track_matches(&parse_query("source:home"), &fields));
        assert!(!track_matches(&parse_query("source:local"), &fields));
    }

    #[test]
    fn a_hidden_source_leaves_browse_and_search() {
        let (_db, mut conn) = sorted_library(
            "hidden-source",
            &[track("/m/one.flac", "So What", "Miles Davis", 1959)],
        );

        let mut off = track("sg-9", "So What", "Miles Davis", 1959);
        off.remote_url = "https://off/stream/9".into();
        store::upsert_source_rows(&mut conn, "subsonic:off", &[off]).unwrap();

        let mut on = track("sg-4", "So What", "Miles Davis", 1959);
        on.remote_url = "https://on/stream/4".into();
        store::upsert_source_rows(&mut conn, "subsonic:on", &[on]).unwrap();

        let mut p = Projection::load_serial(&conn, false).unwrap();
        p.hide_sources(|source| source == "subsonic:off");

        let source_of =
            |p: &Projection, row: u32| p.sources.strings[p.source[row as usize] as usize].clone();
        let hidden = (0..p.len() as u32)
            .find(|&row| source_of(&p, row) == "subsonic:off")
            .expect("the hidden row is still loaded");

        assert!(!p.is_browsable(hidden));
        assert_eq!(p.resolve(hidden).title, "So What");

        let sources = |rows: Vec<u32>| -> Vec<String> {
            let mut out: Vec<String> = rows.into_iter().map(|row| source_of(&p, row)).collect();
            out.sort();
            out
        };
        let expected = ["local", "subsonic:on"];

        assert_eq!(sources(p.sort_canonical()), expected);
        assert_eq!(sources(p.search("so what")), expected);
        assert_eq!(sources(p.search_all("so what")), expected);
        assert_eq!(sources(p.search_all("")), expected);
    }

    #[test]
    fn a_song_name_finds_its_local_file() {
        let (_db, mut conn) = sorted_library(
            "find-local",
            &[
                track("/m/one.flac", "So What", "Miles Davis", 1959),
                track("/m/two.flac", "So What If", "Miles Davis", 1960),
            ],
        );

        let mut remote = track("cloud-9", "So What", "Miles Davis", 1959);
        remote.remote_url = "https://host/stream/9".into();
        store::upsert_source_rows(&mut conn, "subsonic:home", &[remote]).unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        let one = |artist: &str, title: &str| p.find_locals(&[(artist, title)])[0];

        let found = one("Miles Davis", "So What").expect("the file");
        assert_eq!(p.resolve(found).source, "local");
        assert_eq!(p.resolve(found).title, "So What");

        assert!(one("miles davis", "so what").is_some());
        assert!(one("Miles Davis", "So").is_none());
        assert!(one("Bill Evans", "So What").is_none());
    }

    #[test]
    fn a_list_of_songs_answers_in_order() {
        let (_db, conn) = sorted_library(
            "find-locals",
            &[
                track("/m/one.flac", "So What", "Miles Davis", 1959),
                track("/m/two.flac", "Blue In Green", "Miles Davis", 1959),
            ],
        );

        let p = Projection::load_serial(&conn, false).unwrap();
        let found = p.find_locals(&[
            ("Miles Davis", "Blue In Green"),
            ("Bill Evans", "Peace Piece"),
            ("", "So What"),
            ("miles davis", "SO WHAT"),
        ]);

        let titles: Vec<Option<&str>> = found
            .iter()
            .map(|row| row.map(|row| p.resolve(row).title))
            .collect();
        assert_eq!(
            titles,
            vec![Some("Blue In Green"), None, None, Some("So What")]
        );
    }

    #[test]
    fn a_row_resolves_its_own_source() {
        let (_db, mut conn) = sorted_library(
            "mixed-sources",
            &[track("/m/local.flac", "At Home", "Aviary", 1991)],
        );

        let mut remote = track("cloud-7", "On A Server", "Aviary", 1991);
        remote.remote_url = "https://host/stream/7".into();
        store::upsert_source_rows(&mut conn, "subsonic:home", &[remote]).unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        let by_title = |title: &str| {
            (0..p.len() as u32)
                .find(|&row| p.resolve(row).title == title)
                .map(|row| p.resolve(row).source.to_string())
                .unwrap()
        };

        assert_eq!(by_title("At Home"), "local");
        assert_eq!(by_title("On A Server"), "subsonic:home");
    }

    #[test]
    fn a_remote_row_has_no_folder() {
        let (_db, mut conn) = sorted_library(
            "remote-folders",
            &[track("/m/local.flac", "At Home", "Aviary", 1991)],
        );

        let mut station = track("http://play.example/stream", "Noise FM", "", 0);
        station.remote_url = "http://play.example/stream".into();
        station.remote_live = true;
        store::upsert_source_rows(&mut conn, "radio", &[station]).unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        let folder_of = |title: &str| {
            (0..p.len() as u32)
                .find(|&row| p.resolve(row).title == title)
                .map(|row| p.resolve(row).folder.to_string())
                .unwrap()
        };

        assert_eq!(folder_of("At Home"), "/m");
        assert_eq!(folder_of("Noise FM"), "");
    }

    /// The grouped searches match off their own copy of the name, so they need
    /// the sort name too.
    #[test]
    fn search_matches_a_sort_name() {
        let mut row = track("/m/1.mp3", "Lemon", "米津玄師", 2018);
        row.album_artist = "米津玄師".into();
        row.album = "BOOTLEG".into();
        row.title_sort = "Lemon".into();
        row.artist_sort = "Yonezu, Kenshi".into();
        row.album_artist_sort = "Yonezu, Kenshi".into();
        row.album_sort = "Bootleg".into();
        let mut other = track("/m/2.mp3", "Other", "Someone", 2018);
        other.album_artist = "Someone".into();
        other.album = "Elsewhere".into();
        let (db, conn) = sorted_library("sort-search", &[row, other]);

        for p in [
            Projection::load_serial(&conn, false).unwrap(),
            Projection::load_parallel(&db, 2, false).unwrap(),
        ] {
            assert_eq!(titles_for(&p, "yonezu"), ["Lemon"]);
            assert_eq!(titles_for(&p, "artist:yonezu"), ["Lemon"]);
            assert_eq!(titles_for(&p, "albumartist:yonezu"), ["Lemon"]);
            assert_eq!(titles_for(&p, "米津"), ["Lemon"]);
            assert!(titles_for(&p, "artist:elsewhere").is_empty());

            let artists = p.search_artists("yonezu");
            assert_eq!(artists.len(), 1);
            assert_eq!(
                p.album_artists.strings[artists[0].album_artist as usize],
                "米津玄師"
            );
            let albums = p.search_albums("yonezu");
            assert_eq!(albums.len(), 1);
            assert_eq!(p.albums.strings[albums[0].album as usize], "BOOTLEG");

            let view = p.resolve(p.search("yonezu")[0]);
            assert_eq!(view.artist_sort, "Yonezu, Kenshi");
            assert_eq!(view.title_sort, "Lemon");
            assert_eq!(view.album_sort, "Bootleg");
        }
    }

    #[test]
    fn ordering_keys_off_the_sort_name() {
        let named = |path: &str, title: &str, artist: &str, artist_sort: &str| {
            let mut row = track(path, title, artist, 2018);
            row.artist_sort = artist_sort.into();
            row
        };
        let mut titled = named("/m/4.mp3", "ゆめうつつ", "Zzz", "");
        titled.title_sort = "Yumeutsutsu".into();
        let (_, conn) = sorted_library(
            "sort-order",
            &[
                named("/m/1.mp3", "One", "Zebra", ""),
                named("/m/2.mp3", "Two", "米津玄師", "Yonezu, Kenshi"),
                named("/m/3.mp3", "Three", "Alpha", ""),
                named("/m/5.mp3", "Zoo", "Beta", ""),
                titled,
            ],
        );
        let p = Projection::load_serial(&conn, false).unwrap();
        let view: Vec<u32> = (0..p.len() as u32).collect();
        let titles = |order: Vec<u32>| -> Vec<String> {
            order
                .iter()
                .map(|&i| p.title.get(i as usize).to_string())
                .collect()
        };
        assert_eq!(
            titles(p.sort_view(&view, SortKey::Artist, false)),
            ["Three", "Zoo", "Two", "One", "ゆめうつつ"]
        );
        assert_eq!(
            titles(p.sort_title()),
            ["One", "Three", "Two", "ゆめうつつ", "Zoo"]
        );
    }

    #[test]
    fn a_sort_column_orders_on_its_own_value_and_parks_the_empties() {
        let named = |path: &str, title: &str, artist: &str, artist_sort: &str, track: u16| {
            let mut row = track_no(path, title, artist, track);
            row.artist_sort = artist_sort.into();
            row
        };
        let (_, conn) = sorted_library(
            "sort-column-order",
            &[
                named("/m/1.mp3", "One", "Zebra", "", 1),
                named("/m/2.mp3", "Two", "米津玄師", "Yonezu, Kenshi", 3),
                named("/m/3.mp3", "Three", "Alpha", "", 2),
                named("/m/4.mp3", "Four", "宇多田ヒカル", "Utada, Hikaru", 4),
            ],
        );
        let p = Projection::load_serial(&conn, false).unwrap();
        let view: Vec<u32> = (0..p.len() as u32).collect();
        let titles = |order: Vec<u32>| -> Vec<String> {
            order
                .iter()
                .map(|&i| p.title.get(i as usize).to_string())
                .collect()
        };
        assert_eq!(
            titles(p.sort_view(&view, SortKey::ArtistSort, false)),
            ["Four", "Two", "One", "Three"]
        );
        assert_eq!(
            titles(p.sort_view(&view, SortKey::ArtistSort, true)),
            ["Two", "Four", "One", "Three"]
        );
        assert_eq!(
            titles(p.sort_view(&view, SortKey::Artist, false)),
            ["Three", "Four", "Two", "One"]
        );
    }

    #[test]
    fn the_title_sort_column_reads_the_sort_title_alone() {
        let titled = |path: &str, title: &str, title_sort: &str, track: u16| {
            let mut row = track_no(path, title, "Artist", track);
            row.title_sort = title_sort.into();
            row
        };
        let (_, conn) = sorted_library(
            "title-sort-column-order",
            &[
                titled("/m/1.mp3", "Ichi", "Bravo", 1),
                titled("/m/2.mp3", "Ni", "Alpha", 2),
                titled("/m/3.mp3", "San", "", 3),
                titled("/m/4.mp3", "Shi", "", 4),
            ],
        );
        let p = Projection::load_serial(&conn, false).unwrap();
        let view: Vec<u32> = (0..p.len() as u32).collect();
        let titles = |order: Vec<u32>| -> Vec<String> {
            order
                .iter()
                .map(|&i| p.title.get(i as usize).to_string())
                .collect()
        };
        assert_eq!(
            titles(p.sort_view(&view, SortKey::TitleSort, false)),
            ["Ni", "Ichi", "San", "Shi"]
        );
        assert_eq!(
            titles(p.sort_view(&view, SortKey::TitleSort, true)),
            ["Ichi", "Ni", "San", "Shi"]
        );

        let (_, conn) = sorted_library(
            "title-sort-column-none",
            &[
                titled("/m/1.mp3", "Ichi", "", 1),
                titled("/m/2.mp3", "Ni", "", 2),
            ],
        );
        let p = Projection::load_serial(&conn, false).unwrap();
        let view: Vec<u32> = (0..p.len() as u32).collect();
        let canonical = p.sort_canonical();
        assert_eq!(p.sort_view(&view, SortKey::TitleSort, false), canonical);
        assert_eq!(p.sort_view(&view, SortKey::TitleSort, true), canonical);
    }

    #[test]
    fn search_folds_accents_off_the_names_and_the_titles() {
        let mut lead = track("/m/1.mp3", "Déjà Vu", "Beyoncé", 2006);
        lead.album_artist = "Beyoncé".into();
        lead.album = "B'Day".into();
        lead.genre = "Rhythm & Blues".into();
        let mut german = track("/m/2.mp3", "Sonne", "Rammstein", 2001);
        german.album = "Straße der Besten".into();
        let (db, conn) = sorted_library("fold-search", &[lead, german]);

        for p in [
            Projection::load_serial(&conn, false).unwrap(),
            Projection::load_parallel(&db, 2, false).unwrap(),
        ] {
            assert_eq!(titles_for(&p, "beyonce"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "artist:beyonce"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "albumartist:beyonce"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "Beyoncé"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "BEYONCE"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "deja vu"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "title:deja"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "strasse"), ["Sonne"]);
            assert_eq!(titles_for(&p, "album:strasse"), ["Sonne"]);
            assert_eq!(titles_for(&p, "Straße"), ["Sonne"]);
            assert!(titles_for(&p, "artist:rammstein").len() == 1);
            assert!(titles_for(&p, "artist:beyonce").len() == 1);

            let artists = p.search_artists("beyonce");
            assert_eq!(artists.len(), 1);
            assert_eq!(
                p.album_artists.strings[artists[0].album_artist as usize],
                "Beyoncé"
            );
        }
    }

    #[test]
    fn the_row_matcher_folds_accents_too() {
        let fields = TrackFields {
            db_id: Some(1),
            title: "Déjà Vu",
            artist: "Beyoncé",
            album_artist: "Beyoncé",
            album: "B'Day",
            genre: "Rhythm & Blues",
            year: 2006,
            codec: "flac",
            path: "/music/B'Day/1.mp3",
            source: "local",
        };
        assert!(track_matches(&parse_query("beyonce"), &fields));
        assert!(track_matches(&parse_query("artist:beyonce"), &fields));
        assert!(track_matches(&parse_query("deja"), &fields));
        assert!(track_matches(&parse_query("Beyoncé"), &fields));
        assert!(!track_matches(&parse_query("beyonc3"), &fields));
    }

    /// Picks carry the display casing, so accented and unaccented values stay two
    /// picks even though the search key folds.
    #[test]
    fn a_filter_pick_on_an_accented_value_still_narrows() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "Beyoncé", 2006),
                track("/m/2.mp3", "Two", "Beyonce", 2006),
                track("/m/3.mp3", "Three", "Moby", 1999),
            ],
        )
        .unwrap();

        for fold in [false, true] {
            let p = Projection::load_serial(&conn, fold).unwrap();
            let hits = |filter: &FilterSet| -> Vec<&str> {
                let mask = p.filter_mask(filter).unwrap();
                (0..p.len() as u32)
                    .filter(|&i| mask[i as usize])
                    .map(|i| p.resolve(i).title)
                    .collect()
            };
            let mut f = FilterSet::default();
            f.toggle(FilterField::Artist, "Beyoncé");
            assert_eq!(hits(&f), ["One"]);
            let mut plain = FilterSet::default();
            plain.toggle(FilterField::Artist, "Beyonce");
            assert_eq!(hits(&plain), ["Two"]);
            assert_eq!(titles_for(&p, "artist:beyonce"), ["One", "Two"]);
        }
    }

    #[test]
    fn ordering_folds_accents_into_the_latin_run() {
        let (_, conn) = sorted_library(
            "fold-order",
            &[
                track("/m/1.mp3", "One", "Frank", 2018),
                track("/m/2.mp3", "Two", "Émilie", 2018),
                track("/m/3.mp3", "Three", "Dana", 2018),
                track("/m/4.mp3", "Four", "Zebra", 2018),
            ],
        );
        let p = Projection::load_serial(&conn, false).unwrap();
        let view: Vec<u32> = (0..p.len() as u32).collect();
        let titles: Vec<String> = p
            .sort_view(&view, SortKey::Artist, false)
            .iter()
            .map(|&i| p.title.get(i as usize).to_string())
            .collect();
        assert_eq!(titles, ["Three", "Two", "One", "Four"]);
    }

    /// A library without sort tags carries no sort arenas or vectors.
    #[test]
    fn a_library_without_sort_tags_carries_nothing() {
        let (db, conn) = sorted_library(
            "sort-absent",
            &[
                track("/m/1.mp3", "One", "Alpha", 2000),
                track("/m/2.mp3", "Two", "Beta", 2001),
            ],
        );
        for p in [
            Projection::load_serial(&conn, false).unwrap(),
            Projection::load_parallel(&db, 2, false).unwrap(),
        ] {
            assert!(p.title_sort.is_none());
            for table in [&p.artists, &p.album_artists, &p.albums, &p.genres] {
                assert!(table.sort.is_empty());
                assert!(table.sort_lower.is_empty());
            }
            assert_eq!(p.artists.sort_name(0), "");
            assert_eq!(p.artists.sort_key(0), "alpha");
            assert_eq!(p.title_sort(0), "");
            assert_eq!(p.title_sort_key(0), "one");
        }
    }

    #[test]
    fn a_meta_row_fills_a_symbol_the_files_left_bare() {
        let mut looked_up = track("/m/1.mp3", "One", "崎山蒼志", 2018);
        looked_up.album_artist = "崎山蒼志".into();
        let mut bare = track("/m/2.mp3", "Two", "Zebra", 2018);
        bare.album_artist = "Zebra".into();
        let (db, conn) = sorted_library("sort-meta-fill", &[looked_up, bare]);
        crate::artist_meta::set(
            &conn,
            "崎山蒼志",
            "Sakiyama, Soushi",
            crate::artist_meta::MUSICBRAINZ,
        )
        .unwrap();

        for p in [
            Projection::load_serial(&conn, false).unwrap(),
            Projection::load_parallel(&db, 2, false).unwrap(),
        ] {
            assert_eq!(titles_for(&p, "sakiyama"), ["One"]);
            assert_eq!(titles_for(&p, "artist:sakiyama"), ["One"]);
            assert_eq!(titles_for(&p, "albumartist:sakiyama"), ["One"]);
            let view: Vec<u32> = (0..p.len() as u32).collect();
            let order: Vec<&str> = p
                .sort_view(&view, SortKey::Artist, false)
                .iter()
                .map(|&i| p.title.get(i as usize))
                .collect();
            assert_eq!(order, ["One", "Two"]);
            assert_eq!(
                p.resolve(p.search("sakiyama")[0]).artist_sort,
                "Sakiyama, Soushi"
            );
            assert_eq!(p.resolve(p.search("zebra")[0]).artist_sort, "");
            assert!(p.albums.sort.is_empty());
        }
    }

    /// A tag beats a lookup, whichever landed first.
    #[test]
    fn a_file_sort_name_beats_the_meta_table() {
        let mut tagged = track("/m/1.mp3", "One", "米津玄師", 2018);
        tagged.artist_sort = "Yonezu, Kenshi".into();
        let (_, conn) = sorted_library("sort-meta-loses", &[tagged]);
        crate::artist_meta::set(
            &conn,
            "米津玄師",
            "Wrong, Answer",
            crate::artist_meta::MUSICBRAINZ,
        )
        .unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();
        assert_eq!(p.artists.sort_name(0), "Yonezu, Kenshi");
        assert!(titles_for(&p, "wrong").is_empty());
    }

    #[test]
    fn an_album_meta_row_fills_the_album_table() {
        let mut ja = track("/m/1.mp3", "One", "Zebra", 2017);
        ja.album = "打上花火".into();
        let mut latin = track("/m/2.mp3", "Two", "Zebra", 2017);
        latin.album = "Aardvark".into();
        let (db, conn) = sorted_library("sort-album-meta", &[ja, latin]);
        crate::album_meta::set(
            &conn,
            "打上花火",
            "uchiagehanabi",
            crate::artist_meta::ROMANIZED,
        )
        .unwrap();

        for p in [
            Projection::load_serial(&conn, false).unwrap(),
            Projection::load_parallel(&db, 2, false).unwrap(),
        ] {
            assert_eq!(titles_for(&p, "uchiage"), ["One"]);
            assert_eq!(titles_for(&p, "album:uchiage"), ["One"]);
            let view: Vec<u32> = (0..p.len() as u32).collect();
            let order: Vec<&str> = p
                .sort_view(&view, SortKey::Album, false)
                .iter()
                .map(|&i| p.title.get(i as usize))
                .collect();
            assert_eq!(order, ["Two", "One"]);
            assert!(p.artists.sort.is_empty());
        }
    }

    #[test]
    fn a_track_meta_row_fills_a_row_the_files_left_bare() {
        let (db, conn) = sorted_library(
            "sort-track-meta",
            &[
                track("/m/1.mp3", "レモン", "Zebra", 2018),
                track("/m/2.mp3", "Aardvark", "Zebra", 2018),
            ],
        );
        let bare = Projection::load_serial(&conn, false).unwrap();
        assert!(bare.title_sort.is_none());
        let ja = bare
            .db_id
            .iter()
            .zip(0..bare.len())
            .find(|(_, row)| bare.title.get(*row) == "レモン")
            .map(|(id, _)| *id)
            .expect("the row is in the library");
        crate::track_meta::set(&conn, ja, "remon", crate::artist_meta::ROMANIZED).unwrap();

        for p in [
            Projection::load_serial(&conn, false).unwrap(),
            Projection::load_parallel(&db, 2, false).unwrap(),
        ] {
            assert_eq!(titles_for(&p, "remon"), ["レモン"]);
            assert_eq!(titles_for(&p, "title:remon"), ["レモン"]);
            let view: Vec<u32> = (0..p.len() as u32).collect();
            let order: Vec<&str> = p
                .sort_view(&view, SortKey::Title, false)
                .iter()
                .map(|&i| p.title.get(i as usize))
                .collect();
            assert_eq!(order, ["Aardvark", "レモン"]);
            let latin = (0..p.len())
                .find(|&row| p.title.get(row) == "Aardvark")
                .unwrap();
            assert_eq!(p.title_sort(latin), "");
            assert_eq!(p.title_sort_key(latin), "aardvark");
        }
    }

    #[test]
    fn a_file_sort_title_beats_the_track_meta_table() {
        let mut tagged = track("/m/1.mp3", "レモン", "Zebra", 2018);
        tagged.title_sort = "Lemon".into();
        let (_, conn) = sorted_library("sort-track-meta-loses", &[tagged]);
        let id = Projection::load_serial(&conn, false).unwrap().db_id[0];
        crate::track_meta::set(&conn, id, "remon", crate::artist_meta::ROMANIZED).unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();
        assert_eq!(p.title_sort(0), "Lemon");
        assert!(titles_for(&p, "remon").is_empty());
    }

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
        assert_eq!(
            titles(p.sort_view(&view, SortKey::TrackGain, false)),
            ["Untagged", "Loud", "Album Only", "Quiet"]
        );
        assert_eq!(
            titles(p.sort_view(&view, SortKey::AlbumGain, false)),
            ["Untagged", "Loud", "Quiet", "Album Only"]
        );

        let parallel = Projection::load_parallel(&db, 3, false).unwrap();
        assert_eq!(parallel.track_gain, p.track_gain);
        assert_eq!(parallel.album_gain, p.album_gain);
    }

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
        assert_eq!(p.resolve(row_of("Fractional")).bpm, Some(128.25));
        let estimated = p.resolve(row_of("Untagged"));
        assert_eq!(estimated.bpm, Some(92.5));
        assert_eq!(estimated.bpm_source, crate::tempo::Source::Measured);

        assert_eq!(pack_bpm(Some(0.0)), NO_BPM);
        assert_eq!(pack_bpm(Some(900.0)), NO_BPM);
        assert_eq!(pack_bpm(None), NO_BPM);
        assert_eq!(unpack_bpm(NO_BPM), None);

        let parallel = Projection::load_parallel(&db, 3, false).unwrap();
        assert_eq!(parallel.bpm, p.bpm);
        assert_eq!(parallel.bpm_source, p.bpm_source);
    }

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
        assert_eq!(titles_for(&p, "tribute codec:mp3"), ["Flac Tribute"]);
        assert_eq!(titles_for(&p, "flac"), ["Flac Tribute"]);
    }

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
        let hits: Vec<String> = (0..p.len())
            .filter(|&i| mask[i])
            .map(|i| p.title.get(i).to_string())
            .collect();
        assert_eq!(hits, ["One", "Two"]);

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
            source: "local",
        };
        assert!(filter.matches(&fields("/music/Air/Moon Safari/2.mp3"), false));
        assert!(!filter.matches(&fields("/music/Airborne/3.mp3"), false));
    }

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
        filter.toggle(FilterField::Genre, "Electronic");
        assert_eq!(hits(&filter), ["One", "Three"]);
        let mut unknown = FilterSet::default();
        unknown.toggle(FilterField::Genre, "");
        assert_eq!(hits(&unknown), ["Four"]);

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
            source: "local",
        };
        assert!(filter.matches(&fields, false));
        assert!(!unknown.matches(&fields, false));

        assert_eq!(p.filter_genre("Rock").len(), 2);
        assert!(p.filter_genre("Rock; Shoegaze").is_empty());
        assert_eq!(p.genre_terms().strings, ["Rock", "Shoegaze", "Electronic"]);
    }

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
        assert_eq!(folded.genre_terms().strings, ["rock", "Pop"]);

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
            source: "local",
        };
        assert!(filter.matches(&fields, true));
        assert!(!filter.matches(&fields, false));
        let mut genre_pick = FilterSet::default();
        genre_pick.toggle(FilterField::Genre, "Rock");
        let mask = folded.filter_mask(&genre_pick).unwrap();
        assert_eq!(mask.iter().filter(|&&b| b).count(), 3);
        assert_eq!(folded.filter_genre("POP").len(), 1);
    }

    #[test]
    fn search_surfaces_albums_and_artists() {
        fn full(path: &str, album_artist: &str, album: &str, title: &str) -> TrackRow {
            TrackRow {
                remote_url: String::new(),
                remote_live: false,
                title_sort: String::new(),
                artist_sort: String::new(),
                album_artist_sort: String::new(),
                album_sort: String::new(),
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

        let artists = p.search_artists("fleet");
        assert_eq!(artists.len(), 1);
        assert_eq!(
            p.album_artists.strings[artists[0].album_artist as usize],
            "Fleet Foxes"
        );

        let albums = p.search_albums("fleet");
        assert_eq!(albums.len(), 2);
        assert_eq!(p.albums.strings[albums[0].album as usize], "Fleet Foxes");
        assert_eq!(
            p.albums.strings[albums[1].album as usize],
            "Helplessness Blues"
        );

        let pinned = p.search_albums("album:helpless");
        assert_eq!(pinned.len(), 1);
        assert_eq!(
            p.albums.strings[pinned[0].album as usize],
            "Helplessness Blues"
        );

        assert!(p.search_albums("title:montezuma").is_empty());
        assert!(p.search_artists("title:montezuma").is_empty());

        let artists = p.search_artists("-artist:fleet");
        assert_eq!(artists.len(), 1);
        assert_eq!(
            p.album_artists.strings[artists[0].album_artist as usize],
            "ODESZA"
        );
        let albums = p.search_albums("-album:helpless");
        assert_eq!(albums.len(), 2);
        assert_eq!(p.albums.strings[albums[0].album as usize], "Fleet Foxes");
        assert_eq!(p.albums.strings[albums[1].album as usize], "A Moment Apart");
        assert!(p.search_artists("-title:montezuma").is_empty());
        assert!(p.search_albums("-title:montezuma").is_empty());
        assert!(p.search_artists("-genre").is_empty());
        assert!(p.search_albums("-year").is_empty());
    }

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

        assert!(p.filter_mask(&FilterSet::default()).is_none());

        let mut f = FilterSet::default();
        f.toggle(FilterField::Artist, "Air");
        assert_eq!(hits(&f), ["One", "Three"]);

        f.toggle(FilterField::Artist, "Moby");
        assert_eq!(hits(&f), ["One", "Three", "Four"]);

        f.toggle(FilterField::Year, "1998");
        assert_eq!(hits(&f), ["One"]);

        f.toggle(FilterField::Year, "1998");
        f.toggle(FilterField::Artist, "Moby");
        assert_eq!(hits(&f), ["One", "Three"]);
    }

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

        let pinned = FilterSet::with_ids(vec![id_of("One"), id_of("Three")]);
        assert!(!pinned.is_empty());
        assert_eq!(hits(&pinned), ["One", "Three"]);

        assert_eq!(hits(&FilterSet::with_ids(Vec::new())), Vec::<&str>::new());

        let mut both = FilterSet::with_ids(vec![id_of("One"), id_of("Three")]);
        both.toggle(FilterField::Year, "2001");
        assert_eq!(hits(&both), ["Three"]);

        assert!(pinned.fields_empty());
    }

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
            source: "local",
        };
        let catalogued = TrackFields {
            db_id: Some(7),
            ..dropped
        };

        for ids in [vec![7], vec![0], vec![0, 7], Vec::new()] {
            let pinned = FilterSet::with_ids(ids);
            assert!(
                !pinned.matches(&dropped, false),
                "an off-catalog row has no id to pin"
            );
        }
        assert!(FilterSet::with_ids(vec![7]).matches(&catalogued, false));

        let mut by_artist = FilterSet::default();
        by_artist.toggle(FilterField::Artist, "Air");
        assert!(by_artist.matches(&dropped, false));
        by_artist.toggle(FilterField::Artist, "Air");
        by_artist.toggle(FilterField::Artist, "Daft Punk");
        assert!(!by_artist.matches(&dropped, false));
    }

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

    #[test]
    fn search_grouped_matches_reference() {
        fn full(path: &str, album_artist: &str, album: &str, title: &str) -> TrackRow {
            TrackRow {
                remote_url: String::new(),
                remote_live: false,
                title_sort: String::new(),
                artist_sort: String::new(),
                album_artist_sort: String::new(),
                album_sort: String::new(),
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

    #[test]
    fn search_cache_is_stable_across_calls() {
        fn full(path: &str, album_artist: &str, album: &str) -> TrackRow {
            TrackRow {
                remote_url: String::new(),
                remote_live: false,
                title_sort: String::new(),
                artist_sort: String::new(),
                album_artist_sort: String::new(),
                album_sort: String::new(),
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

        assert_eq!(p.search("air"), p.search("air"));
    }

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

        assert_eq!(titles_for(&p, "year:199"), ["Nineties"]);
        assert_eq!(titles_for(&p, "year:2000"), ["Two Thousand"]);
        assert_eq!(titles_for(&p, "year:0"), ["Zero", "Two Thousand"]);
        assert_eq!(titles_for(&p, &format!("year:{}", u16::MAX)), ["Max"]);
    }

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

    struct Live {
        projection: Projection,
        order: Vec<u32>,
        index: HashMap<i64, u32>,
    }

    impl Live {
        fn load(conn: &rusqlite::Connection, fold: bool) -> Self {
            let projection = Projection::load_serial(conn, fold).unwrap();
            let order = projection.sort_canonical();
            let index = projection
                .db_id
                .iter()
                .enumerate()
                .map(|(row, &id)| (id, row as u32))
                .collect();
            Live {
                projection,
                order,
                index,
            }
        }

        fn sync(&mut self, conn: &rusqlite::Connection, changed: &[i64], gone: &[i64]) {
            let shard = shard_for_ids(conn, changed, self.projection.fold).unwrap();
            let plays = store::plays_for_ids(conn, shard.ids()).unwrap();
            let spans = store::cue_spans_for_ids(conn, shard.ids()).unwrap();
            let mut patch = self
                .projection
                .apply_upserts(shard, &self.index, &plays, &spans)
                .expect("the shard fits");
            let removed = self.projection.remove_ids(gone, &self.index);
            patch.dropped.extend(removed.dropped);
            patch.gone = removed.gone;
            self.order = if patch.reordered {
                self.projection.sort_canonical()
            } else {
                self.projection.patch_order(&self.order, &patch)
            };
            for &row in &patch.added {
                self.index.insert(self.projection.db_id[row as usize], row);
            }
            for id in &patch.gone {
                self.index.remove(id);
            }
        }
    }

    /// A patched projection can't have a rebuilt one's row indexes, so equality
    /// means the same answers in the same order.
    fn row_snapshot(p: &Projection, row: u32) -> String {
        let i = row as usize;
        let v = p.resolve(row);
        format!(
            "{id} {title:?} {artist:?} {album_artist:?} {album:?} {genre:?} {year} {disc}/{track} \
             {duration} {codec:?} {bitrate} {rate}/{depth} r{rating} p{plays} a{added} \
             {track_gain:?}/{album_gain:?} {bpm:?} {bpm_source:?} {source:?} {folder:?} sub{sub} \
             sorts {title_sort:?}/{artist_sort:?}/{aa_sort:?}/{album_sort:?} \
             keys {title_key:?}/{artist_key:?}/{aa_key:?}/{album_key:?}/{genre_key:?} \
             span {span:?}",
            id = p.db_id[i],
            title = v.title,
            artist = v.artist,
            album_artist = v.album_artist,
            album = v.album,
            genre = v.genre,
            year = v.year,
            disc = v.disc_no,
            track = v.track_no,
            duration = v.duration_ms,
            codec = v.codec,
            bitrate = v.bitrate_kbps,
            rate = v.sample_rate_hz,
            depth = v.bit_depth,
            rating = v.rating,
            plays = v.plays,
            added = v.added,
            track_gain = v.track_gain_db,
            album_gain = v.album_gain_db,
            bpm = v.bpm,
            bpm_source = v.bpm_source,
            source = v.source,
            folder = v.folder,
            sub = v.sub,
            title_sort = v.title_sort,
            artist_sort = v.artist_sort,
            aa_sort = v.album_artist_sort,
            album_sort = v.album_sort,
            title_key = p.title_sort_key(i),
            artist_key = p.artists.sort_key(p.artist[i] as usize),
            aa_key = p.album_artists.sort_key(p.album_artist[i] as usize),
            album_key = p.albums.sort_key(p.album[i] as usize),
            genre_key = p.genres.sort_key(p.genre[i] as usize),
            span = p.span(row),
        )
    }

    fn snapshot_all(p: &Projection, order: &[u32]) -> Vec<String> {
        order.iter().map(|&row| row_snapshot(p, row)).collect()
    }

    fn full_row(
        path: &str,
        title: &str,
        artist: &str,
        album: &str,
        disc_no: u16,
        track_no: u16,
    ) -> TrackRow {
        TrackRow {
            path: path.into(),
            title: title.into(),
            artist: artist.into(),
            album_artist: artist.into(),
            album: album.into(),
            genre: "Shoegaze".into(),
            year: 1991,
            disc_no,
            track_no,
            duration_ms: 200_000 + track_no as u32,
            codec: "flac".into(),
            bitrate_kbps: 900,
            sample_rate_hz: 44_100,
            bit_depth: 16,
            rating: 60,
            ..track(path, title, artist, 1991)
        }
    }

    fn library_for(name: &str, rows: &[TrackRow]) -> (std::path::PathBuf, rusqlite::Connection) {
        sorted_library(name, rows)
    }

    fn id_of(conn: &rusqlite::Connection, path: &str) -> i64 {
        store::ids_for_paths(conn, &[std::path::PathBuf::from(path)]).unwrap()[0]
    }

    fn library_and_station() -> Projection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let mut local = track("/m/1.mp3", "Sunset Drive", "Aviary", 2001);
        local.album_artist = "Aviary".into();
        local.album = "First".into();
        local.genre = "Shoegaze".into();
        store::insert_batch(&mut conn, &[local]).unwrap();
        crate::stations::put(
            &mut conn,
            &[crate::stations::Station {
                url: "http://127.0.0.1:8768/stream".into(),
                name: "Noise FM - EDM Radio".into(),
                genre: "EDM".into(),
            }],
        )
        .unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    fn rows_by_origin(p: &Projection) -> (u32, u32) {
        let origin =
            |row: u32| crate::cue::Origin::of(&p.sources.strings[p.source[row as usize] as usize]);
        let radio = (0..p.len() as u32)
            .find(|&row| origin(row) == crate::cue::Origin::Radio)
            .expect("the station row");
        let local = (0..p.len() as u32)
            .find(|&row| origin(row) == crate::cue::Origin::Local)
            .expect("the local row");
        (local, radio)
    }

    #[test]
    fn a_station_row_loads_but_does_not_browse() {
        let p = library_and_station();
        let (local, radio) = rows_by_origin(&p);

        assert_eq!(p.len(), 2);
        assert!(!p.is_dead(radio));
        assert_eq!(p.resolve(radio).title, "Noise FM - EDM Radio");

        assert!(p.is_browsable(local));
        assert!(!p.is_browsable(radio));
    }

    #[test]
    fn the_browse_seams_leave_a_station_out() {
        let p = library_and_station();
        let (local, radio) = rows_by_origin(&p);

        assert_eq!(p.sort_canonical(), [local], "the canonical order");
        assert_eq!(p.sort_title(), [local], "a title sort");
        assert_eq!(p.sort_year(), [local], "a year sort");

        // `search` is the browse entry point; `search_all` finds stations.
        assert_eq!(p.search(""), [local]);
        assert!(p.search("noise fm").is_empty());
        assert!(p.search("edm").is_empty(), "not through its genre either");
        assert_eq!(titles_for(&p, "sunset"), ["Sunset Drive"]);

        let mask = p
            .filter_mask(&FilterSet::with_ids(p.db_id.clone()))
            .expect("an id pin is not an empty filter");
        assert!(mask[local as usize]);
        assert!(!mask[radio as usize]);

        assert!(!p.genre_terms().strings.iter().any(|g| g == "EDM"));
    }

    #[test]
    fn a_general_search_finds_a_station_after_the_tracks() {
        let p = library_and_station();
        let (local, radio) = rows_by_origin(&p);

        assert_eq!(p.search_all("noise fm"), [radio]);
        assert_eq!(p.search_all("edm"), [radio]);

        assert_eq!(p.search_all("sunset"), [local]);
        assert_eq!(
            p.search_all("http://127.0.0.1:8768/stream"),
            Vec::<u32>::new(),
            "the URL is a path, not a matched field"
        );

        assert_eq!(p.search_all(""), [local, radio]);

        assert!(p.search("noise fm").is_empty());
        assert_eq!(p.sort_canonical(), [local]);
    }

    #[test]
    fn a_general_search_puts_the_station_last() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        crate::stations::put(
            &mut conn,
            &[crate::stations::Station {
                url: "http://127.0.0.1:8768/stream".into(),
                name: "Sunset Radio".into(),
                genre: "EDM".into(),
            }],
        )
        .unwrap();
        let mut local = track("/m/1.mp3", "Sunset Drive", "Aviary", 2001);
        local.album_artist = "Aviary".into();
        local.album = "First".into();
        store::insert_batch(&mut conn, &[local]).unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();
        let (local, radio) = rows_by_origin(&p);
        assert!(radio < local, "the station really is the earlier row");

        assert_eq!(p.search_all("sunset"), [local, radio]);
    }

    #[test]
    fn a_patched_in_station_stays_out_of_the_order() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let mut local = track("/m/1.mp3", "Sunset Drive", "Aviary", 2001);
        local.album_artist = "Aviary".into();
        local.album = "First".into();
        store::insert_batch(&mut conn, &[local]).unwrap();
        let mut p = Projection::load_serial(&conn, false).unwrap();
        let order = p.sort_canonical();
        assert_eq!(order.len(), 1);

        crate::stations::put(
            &mut conn,
            &[crate::stations::Station {
                url: "http://127.0.0.1:8768/stream".into(),
                name: "Noise FM - EDM Radio".into(),
                genre: "EDM".into(),
            }],
        )
        .unwrap();
        let index: HashMap<i64, u32> = p
            .db_id
            .iter()
            .enumerate()
            .map(|(row, id)| (*id, row as u32))
            .collect();
        let added = store::id_for_path(
            &conn,
            crate::stations::SOURCE,
            "http://127.0.0.1:8768/stream",
        )
        .unwrap()
        .expect("the station's id");
        let shard = shard_for_ids(&conn, &[added], false).unwrap();
        let patch = p
            .apply_upserts(shard, &index, &HashMap::new(), &HashMap::new())
            .expect("the shard fits");

        assert_eq!(patch.added.len(), 1, "the row did land in the columns");
        let patched = p.patch_order(&order, &patch);
        assert_eq!(patched, order, "and nowhere near the browse order");
        assert_eq!(p.sort_canonical(), order);
    }

    #[test]
    fn an_upsert_swaps_what_search_finds() {
        let (_db, mut conn) = library_for(
            "upsert-search",
            &[
                full_row("/m/A/1.flac", "Sunset Drive", "Aviary", "First", 1, 1),
                full_row("/m/A/2.flac", "Night Bus", "Aviary", "First", 1, 2),
            ],
        );
        let mut live = Live::load(&conn, false);
        assert_eq!(titles_for(&live.projection, "sunset").len(), 1);

        let id = id_of(&conn, "/m/A/1.flac");
        store::insert_batch(
            &mut conn,
            &[full_row(
                "/m/A/1.flac",
                "Sunrise Drive",
                "Aviary",
                "First",
                1,
                1,
            )],
        )
        .unwrap();
        live.sync(&conn, &[id], &[]);

        assert!(titles_for(&live.projection, "sunset").is_empty());
        assert_eq!(titles_for(&live.projection, "sunrise"), ["Sunrise Drive"]);
        assert_eq!(live.projection.live_len(), 2);
        assert_eq!(live.projection.len(), 3);
        assert_eq!(live.projection.dead_rows(), 1);
        assert_eq!(live.order.len(), 2);
    }

    #[test]
    fn a_removed_row_leaves_search_filter_and_sort() {
        let (_db, conn) = library_for(
            "remove-hides",
            &[
                full_row("/m/A/1.flac", "Sunset Drive", "Aviary", "First", 1, 1),
                full_row("/m/A/2.flac", "Night Bus", "Aviary", "First", 1, 2),
            ],
        );
        let mut live = Live::load(&conn, false);
        let gone = id_of(&conn, "/m/A/2.flac");
        let gone_row = live.index[&gone];
        store::remove_subtree(&conn, std::path::Path::new("/m/A/2.flac")).unwrap();
        live.sync(&conn, &[], &[gone]);

        assert!(titles_for(&live.projection, "night").is_empty());
        assert_eq!(titles_for(&live.projection, "sunset"), ["Sunset Drive"]);
        assert!(!live.projection.search("").contains(&gone_row));
        assert!(!live.projection.filter_genre("Shoegaze").contains(&gone_row));
        let mask = live
            .projection
            .filter_mask(&FilterSet {
                fields: vec![(FilterField::Artist, vec!["Aviary".into()])],
                ids: None,
            })
            .unwrap();
        assert!(!mask[gone_row as usize]);
        assert!(!live.projection.sort_canonical().contains(&gone_row));
        assert!(!live.projection.sort_title().contains(&gone_row));
        assert!(!live.order.contains(&gone_row));
        assert_eq!(live.projection.live_len(), 1);
        assert_eq!(live.projection.dead_fraction(), 0.5);
    }

    #[test]
    fn inserts_land_in_canonical_order() {
        let (_db, mut conn) = library_for(
            "insert-order",
            &[
                full_row("/m/C/1.flac", "Cedar", "Cormorant", "Third", 1, 1),
                full_row("/m/A/1.flac", "Alder", "Aviary", "First", 1, 1),
            ],
        );
        let mut live = Live::load(&conn, false);

        // Before every row, after, and mid-run: the three places a merge goes wrong.
        let fresh = [
            full_row("/m/B/1.flac", "Birch", "Bellwether", "Second", 1, 1),
            full_row("/m/A/2.flac", "Ash", "Aviary", "First", 1, 2),
            full_row("/m/Z/1.flac", "Zelkova", "Zenith", "Fourth", 1, 1),
        ];
        store::insert_batch(&mut conn, &fresh).unwrap();
        let ids: Vec<i64> = fresh.iter().map(|row| id_of(&conn, &row.path)).collect();
        live.sync(&conn, &ids, &[]);

        assert_eq!(live.order, live.projection.sort_canonical());
        let titles: Vec<&str> = live
            .order
            .iter()
            .map(|&row| live.projection.title.get(row as usize))
            .collect();
        assert_eq!(titles, ["Alder", "Ash", "Birch", "Cedar", "Zelkova"]);
    }

    #[test]
    fn a_patched_projection_matches_a_fresh_load() {
        let mut seed = vec![
            full_row("/m/A/1.flac", "Alder", "Aviary", "First", 1, 1),
            full_row("/m/A/2.flac", "Ash", "Aviary", "First", 1, 2),
            full_row("/m/A/3.flac", "Aspen", "Aviary", "First", 2, 1),
            full_row("/m/B/1.flac", "Birch", "Bellwether", "Second", 1, 1),
            full_row("/m/B/2.flac", "Beech", "Bellwether", "Second", 1, 2),
            full_row("/m/C/1.flac", "Cedar", "Cormorant", "Third", 1, 1),
        ];
        seed[3].artist_sort = "Bellwether, The".into();
        seed[3].album_artist_sort = "Bellwether, The".into();
        seed[4].artist_sort = "Bellwether, The".into();
        seed[4].album_artist_sort = "Bellwether, The".into();
        let (_db, mut conn) = library_for("patch-equals-load", &seed);
        let mut live = Live::load(&conn, false);

        let played = id_of(&conn, "/m/A/2.flac");
        listens::append(
            &conn,
            &listens::Listen {
                track_id: played,
                played_at: 1_700_000_000,
                title: "Ash".into(),
                artist: "Aviary".into(),
                album: "First".into(),
                genre: "Shoegaze".into(),
                path: "/m/A/2.flac".into(),
            },
        )
        .unwrap();

        let mut edited = full_row("/m/A/1.flac", "Alderwood", "Aviary", "First", 1, 1);
        edited.title_sort = "Alderwood, The".into();
        let mut moved = full_row("/m/C/1.flac", "Cedarwood", "Cormorant", "Third", 1, 1);
        moved.rating = 100;
        moved.year = 1994;
        crate::artist_meta::set(&conn, "Dovetail", "Dovetail, The", "musicbrainz").unwrap();
        crate::artist_meta::set(&conn, "Cormorant", "Cormorant, The", "musicbrainz").unwrap();
        crate::album_meta::set(&conn, "Fourth", "Fourth, The", "romanized").unwrap();
        crate::track_meta::set(&conn, played, "Ash, The", "romanized").unwrap();
        let arrivals = [
            edited,
            moved,
            full_row("/m/D/1.flac", "Dogwood", "Dovetail", "Fourth", 1, 1),
            full_row("/m/D/2.flac", "Douglas", "Dovetail", "Fourth", 1, 2),
        ];
        store::insert_batch(&mut conn, &arrivals).unwrap();
        let changed: Vec<i64> = arrivals
            .iter()
            .map(|row| id_of(&conn, &row.path))
            .chain(std::iter::once(played))
            .collect();
        let gone = vec![id_of(&conn, "/m/B/2.flac"), id_of(&conn, "/m/A/3.flac")];
        store::remove_subtree(&conn, std::path::Path::new("/m/B/2.flac")).unwrap();
        store::remove_subtree(&conn, std::path::Path::new("/m/A/3.flac")).unwrap();
        live.sync(&conn, &changed, &gone);

        // The load-bearing comparison: patched equals built from scratch.
        let fresh = Projection::load_serial(&conn, false).unwrap();
        let fresh_order = fresh.sort_canonical();
        assert_eq!(live.projection.live_len(), fresh.len());
        assert_eq!(live.order, live.projection.sort_canonical());
        assert_eq!(
            snapshot_all(&live.projection, &live.order),
            snapshot_all(&fresh, &fresh_order)
        );
        assert_eq!(
            titles_for(&live.projection, "wood"),
            titles_for(&fresh, "wood")
        );
        assert_eq!(live.projection.distinct_years(), fresh.distinct_years());
        assert_eq!(
            live.projection
                .search_albums("o")
                .iter()
                .map(|hit| live.projection.albums.strings[hit.album as usize].clone())
                .collect::<Vec<_>>(),
            fresh
                .search_albums("o")
                .iter()
                .map(|hit| fresh.albums.strings[hit.album as usize].clone())
                .collect::<Vec<_>>(),
        );

        let second = [full_row(
            "/m/D/3.flac",
            "Dawn Redwood",
            "Dovetail",
            "Fourth",
            1,
            3,
        )];
        store::insert_batch(&mut conn, &second).unwrap();
        let more = vec![id_of(&conn, "/m/D/3.flac")];
        let also_gone = vec![id_of(&conn, "/m/A/2.flac")];
        store::remove_subtree(&conn, std::path::Path::new("/m/A/2.flac")).unwrap();
        live.sync(&conn, &more, &also_gone);

        let fresh = Projection::load_serial(&conn, false).unwrap();
        let fresh_order = fresh.sort_canonical();
        assert_eq!(
            snapshot_all(&live.projection, &live.order),
            snapshot_all(&fresh, &fresh_order)
        );

        assert!(live.projection.dead_rows() > 0);
        let compacted = Live::load(&conn, false);
        assert_eq!(compacted.projection.dead_rows(), 0);
        assert_eq!(
            snapshot_all(&compacted.projection, &compacted.order),
            snapshot_all(&live.projection, &live.order)
        );
    }

    #[test]
    fn a_patch_keeps_a_folded_library_folded() {
        let (_db, mut conn) = library_for(
            "patch-folded",
            &[
                full_row("/m/A/1.flac", "Alder", "Aviary", "First", 1, 1),
                full_row("/m/A/2.flac", "Ash", "AVIARY", "First", 1, 2),
            ],
        );
        let mut live = Live::load(&conn, true);
        let symbols = live.projection.artists.strings.len();

        store::insert_batch(
            &mut conn,
            &[full_row("/m/A/3.flac", "Aspen", "aviary", "First", 2, 1)],
        )
        .unwrap();
        let id = id_of(&conn, "/m/A/3.flac");
        live.sync(&conn, &[id], &[]);

        assert_eq!(live.projection.artists.strings.len(), symbols);
        assert_eq!(titles_for(&live.projection, "artist:aviary").len(), 3);
    }

    #[test]
    fn a_patch_matches_an_accented_symbol_without_merging_it() {
        let (_db, mut conn) = library_for(
            "patch-accents",
            &[full_row("/m/A/1.flac", "First", "Beyoncé", "B'Day", 1, 1)],
        );
        let mut live = Live::load(&conn, true);
        let symbols = live.projection.artists.strings.len();

        store::insert_batch(
            &mut conn,
            &[full_row("/m/A/2.flac", "Second", "Beyoncé", "B'Day", 1, 2)],
        )
        .unwrap();
        let id = id_of(&conn, "/m/A/2.flac");
        live.sync(&conn, &[id], &[]);
        assert_eq!(live.projection.artists.strings.len(), symbols);

        store::insert_batch(
            &mut conn,
            &[full_row("/m/A/3.flac", "Third", "Beyonce", "B'Day", 1, 3)],
        )
        .unwrap();
        let id = id_of(&conn, "/m/A/3.flac");
        live.sync(&conn, &[id], &[]);
        assert_eq!(live.projection.artists.strings.len(), symbols + 1);

        assert_eq!(titles_for(&live.projection, "artist:beyonce").len(), 3);
    }

    #[test]
    fn the_arena_refuses_an_offset_it_cannot_hold() {
        // A fake length, so the refusal is testable without four gigabytes of titles.
        assert_eq!(Arena::checked_end(0, 5), Some(5));
        assert_eq!(Arena::checked_end(u32::MAX as usize - 5, 5), Some(u32::MAX));
        assert_eq!(Arena::checked_end(u32::MAX as usize - 4, 5), None);
        assert_eq!(Arena::checked_end(usize::MAX, 1), None);

        let mut arena = Arena::default();
        assert!(arena.push("kept"));
        assert!(!arena.fits(u32::MAX as usize));
        assert_eq!(arena.get(0), "kept");
        arena.pop();
        assert_eq!(arena.bytes_len(), 0);
    }
    #[test]
    fn the_order_survives_a_pile_of_ties() {
        // Thirty rows with identical canonical keys, so every insert and removal
        // lands inside one run.
        let seed: Vec<TrackRow> = (0..30)
            .map(|n| {
                full_row(
                    &format!("/m/T/{n}.flac"),
                    &format!("Tie {n}"),
                    "Twin",
                    "Same",
                    1,
                    1,
                )
            })
            .collect();
        let (_db, mut conn) = library_for("order-ties", &seed);
        let mut live = Live::load(&conn, false);

        let mut gone = Vec::new();
        for n in (0..30).step_by(6) {
            let path = format!("/m/T/{n}.flac");
            gone.push(id_of(&conn, &path));
            store::remove_subtree(&conn, std::path::Path::new(&path)).unwrap();
        }
        let arrivals: Vec<TrackRow> = (1..30)
            .step_by(7)
            .map(|n| {
                full_row(
                    &format!("/m/T/{n}.flac"),
                    &format!("Tie {n} again"),
                    "Twin",
                    "Same",
                    1,
                    1,
                )
            })
            .chain((0..3).map(|n| {
                full_row(
                    &format!("/m/T/new-{n}.flac"),
                    &format!("Fresh {n}"),
                    "Twin",
                    "Same",
                    1,
                    1,
                )
            }))
            .collect();
        store::insert_batch(&mut conn, &arrivals).unwrap();
        let changed: Vec<i64> = arrivals.iter().map(|row| id_of(&conn, &row.path)).collect();
        live.sync(&conn, &changed, &gone);

        let mut in_order = live.order.clone();
        in_order.sort_unstable();
        let mut expected: Vec<u32> = (0..live.projection.len() as u32)
            .filter(|&row| !live.projection.is_dead(row))
            .collect();
        expected.sort_unstable();
        assert_eq!(in_order, expected);
        let mut ids: Vec<i64> = live
            .order
            .iter()
            .map(|&row| live.projection.db_id[row as usize])
            .collect();
        ids.sort_unstable();
        let fresh = Projection::load_serial(&conn, false).unwrap();
        let mut want: Vec<i64> = fresh.db_id.clone();
        want.sort_unstable();
        assert_eq!(ids, want);
    }

    /// Without the reordered flag, `patch_order` would search an order that's no
    /// longer sorted under the new ranks.
    #[test]
    fn a_patch_that_moves_a_known_value_rebuilds_the_order() {
        let seed = vec![
            full_row("/m/A/1.flac", "Alder", "Aviary", "First", 1, 1),
            full_row("/m/B/1.flac", "Birch", "Bellwether", "Second", 1, 1),
            full_row("/m/B/2.flac", "Beech", "Bellwether", "Second", 1, 2),
            full_row("/m/B/3.flac", "Bay", "Bellwether", "Second", 1, 3),
            full_row("/m/C/1.flac", "Cedar", "Cormorant", "Third", 1, 1),
            full_row("/m/D/1.flac", "Dogwood", "Dovetail", "Fourth", 1, 1),
        ];
        let (_db, mut conn) = library_for("patch-adopts-a-sort-name", &seed);
        let mut live = Live::load(&conn, false);

        let mut edited = full_row("/m/B/2.flac", "Beech", "Bellwether", "Second", 1, 2);
        edited.artist_sort = "Zulu".into();
        edited.album_artist_sort = "Zulu".into();
        store::insert_batch(&mut conn, &[edited]).unwrap();
        live.sync(&conn, &[id_of(&conn, "/m/B/2.flac")], &[]);

        assert_eq!(
            live.order,
            live.projection.sort_canonical(),
            "the order a patch left behind is the order a fresh sort gives"
        );
        let fresh = Projection::load_serial(&conn, false).unwrap();
        assert_eq!(
            snapshot_all(&live.projection, &live.order),
            snapshot_all(&fresh, &fresh.sort_canonical())
        );
    }

    /// Always taking the slow path would hide a bug behind a full sort per watch
    /// event.
    #[test]
    fn only_an_adopted_sort_name_marks_a_patch_reordered() {
        let seed = vec![full_row("/m/A/1.flac", "Alder", "Aviary", "First", 1, 1)];
        let (_db, mut conn) = library_for("patch-reordered-flag", &seed);
        let mut live = Live::load(&conn, false);

        store::insert_batch(
            &mut conn,
            &[full_row(
                "/m/D/1.flac",
                "Dogwood",
                "Dovetail",
                "Fourth",
                1,
                1,
            )],
        )
        .unwrap();
        let shard = shard_for_ids(&conn, &[id_of(&conn, "/m/D/1.flac")], false).unwrap();
        let patch = live
            .projection
            .apply_upserts(shard, &live.index, &HashMap::new(), &HashMap::new())
            .expect("the shard fits");
        assert!(!patch.reordered);
        live.order = live.projection.patch_order(&live.order, &patch);
        for &row in &patch.added {
            live.index.insert(live.projection.db_id[row as usize], row);
        }

        let mut edited = full_row("/m/D/1.flac", "Dogwood", "Dovetail", "Fourth", 1, 1);
        edited.artist_sort = "Dovetail, The".into();
        edited.album_artist_sort = "Dovetail, The".into();
        store::insert_batch(&mut conn, &[edited]).unwrap();
        let shard = shard_for_ids(&conn, &[id_of(&conn, "/m/D/1.flac")], false).unwrap();
        let patch = live
            .projection
            .apply_upserts(shard, &live.index, &HashMap::new(), &HashMap::new())
            .expect("the shard fits");
        assert!(patch.reordered);
    }

    /// Without the symbol-id tie-break, equal sort keys swap on the next patch.
    #[test]
    fn tied_sort_keys_hold_their_places_when_the_table_grows() {
        for n in [2usize, 50, 500, 5000, 50000] {
            let mut table = SymTable {
                strings: Vec::new(),
                lower: Vec::new(),
                sort: Vec::new(),
                sort_lower: Vec::new(),
            };
            for i in 0..n {
                table.push_symbol(&format!("Name{i:06}"), "");
                table.push_symbol(&format!("name{i:06}"), "");
            }
            let before = Projection::ranks(&table);
            table.push_symbol("Name000123-arrival", "");
            let after = Projection::ranks(&table);
            let swaps = (0..n)
                .filter(|i| {
                    (before[2 * i] < before[2 * i + 1]) != (after[2 * i] < after[2 * i + 1])
                })
                .count();
            assert_eq!(
                swaps, 0,
                "{swaps} ties swapped when the table grew, at {n} pairs"
            );
        }
    }

    #[test]
    fn a_patch_carries_the_sort_names_the_meta_tables_hold() {
        let seed = vec![
            full_row("/m/A/1.flac", "Alder", "Aviary", "First", 1, 1),
            full_row("/m/A/2.flac", "Ash", "Aviary", "First", 1, 2),
        ];
        let (_db, mut conn) = library_for("patch-keeps-meta-sorts", &seed);
        let one = id_of(&conn, "/m/A/1.flac");
        crate::track_meta::set(&conn, one, "Romanized Alder", "romanized").unwrap();
        crate::artist_meta::set(&conn, "Dovetail", "Dovetail, The", "musicbrainz").unwrap();
        crate::album_meta::set(&conn, "Fourth", "Fourth, The", "romanized").unwrap();

        let mut live = Live::load(&conn, false);
        assert_eq!(
            live.projection.title_sort(live.index[&one] as usize),
            "Romanized Alder"
        );

        store::insert_batch(
            &mut conn,
            &[full_row("/m/A/1.flac", "Alder", "Aviary", "First", 1, 1)],
        )
        .unwrap();
        let one = id_of(&conn, "/m/A/1.flac");
        live.sync(&conn, &[one], &[]);
        assert_eq!(
            live.projection.title_sort(live.index[&one] as usize),
            "Romanized Alder",
            "a patched row kept the sort title the library looked up"
        );

        store::insert_batch(
            &mut conn,
            &[full_row(
                "/m/D/1.flac",
                "Dogwood",
                "Dovetail",
                "Fourth",
                1,
                1,
            )],
        )
        .unwrap();
        let four = id_of(&conn, "/m/D/1.flac");
        live.sync(&conn, &[four], &[]);
        let row = live.index[&four] as usize;
        assert_eq!(
            live.projection
                .album_artists
                .sort_name(live.projection.album_artist[row] as usize),
            "Dovetail, The"
        );
        assert_eq!(
            live.projection
                .albums
                .sort_name(live.projection.album[row] as usize),
            "Fourth, The"
        );

        let fresh = Projection::load_serial(&conn, false).unwrap();
        assert_eq!(
            snapshot_all(&live.projection, &live.order),
            snapshot_all(&fresh, &fresh.sort_canonical())
        );
    }

    #[test]
    fn a_shard_reads_a_repeated_id_once() {
        let (_db, conn) = library_for(
            "shard-dedups-ids",
            &[full_row("/m/A/1.flac", "Alder", "Aviary", "First", 1, 1)],
        );
        let one = id_of(&conn, "/m/A/1.flac");
        let shard = shard_for_ids(&conn, &[one, one, one], false).unwrap();
        assert_eq!(shard.ids(), [one]);
    }
}
