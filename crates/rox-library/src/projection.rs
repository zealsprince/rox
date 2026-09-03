//! The read path of ADR 5 at scale. Columnar: artist, album, and genre are
//! interned to u32 symbols, titles are stored in one contiguous byte arena
//! with an offset table (never ten million heap Strings), and every browse
//! order is a precomputed Vec<u32> of row indices over integer ranks. Search
//! per ADR 6 is substring: the interned tables are scanned whole (they're a
//! hundredth the row count), only titles need the full-row scan, and that scan
//! splits across cores in fixed chunks. A query is terms ANDed per
//! [`parse_query`], each free or pinned to one field with `field:value`
//! syntax, and a leading hyphen turns a pinned term into an exclusion
//! (`-genre:rock`) or, bare, into a test for the field being absent
//! (`-genre`).

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

/// Say so the first time an arena refuses a push, and only then: past the
/// ceiling every following row refuses too, and a line each would bury the
/// one that matters under a million copies of itself.
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
    /// Where a push of `added` bytes would leave the arena, or None when
    /// that offset no longer fits the u32 offset table. Factored out of the
    /// pushes so the refusal is testable without four gigabytes of titles
    /// on hand: the arena's own length is the only thing it reads.
    fn checked_end(current: usize, added: usize) -> Option<u32> {
        u32::try_from(current.checked_add(added)?).ok()
    }

    /// Whether `added` more bytes would still fit under the ceiling.
    fn fits(&self, added: usize) -> bool {
        Self::checked_end(self.bytes.len(), added).is_some()
    }

    fn bytes_len(&self) -> usize {
        self.bytes.len()
    }

    /// Append a string, or refuse and leave the arena exactly as it was.
    /// False means the caller has to drop the whole row: the columns are
    /// positional, so a row that lands in some of them and not the rest
    /// shifts every row after it.
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
        // Query needles run through crate::fold::fold, which lowercases,
        // spells out the sharp s, and strips accents; the stored key has to
        // go through the same function or the two sides stop meeting.
        // ASCII folds to plain lowercase, so it skips the allocation.
        if s.is_ascii() {
            if !self.fits(s.len()) {
                note_arena_overflow();
                return false;
            }
            self.bytes
                .extend(s.bytes().map(|b| b.to_ascii_lowercase() as char));
        } else {
            // Folding can grow a string, so the check waits on the folded
            // length rather than guessing a worst case off the input.
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

    /// Undo the last push, for rolling a half-written row back out.
    fn pop(&mut self) {
        if self.offsets.len() > 1 {
            self.offsets.pop();
            let end = *self.offsets.last().expect("the base offset always stays");
            self.bytes.truncate(end as usize);
        }
    }

    /// Whether the arena holds no text at all, however many rows pushed
    /// into it. What the merge asks before deciding a library has no sort
    /// titles worth keeping.
    fn is_blank(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn get(&self, i: usize) -> &str {
        &self.bytes[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }

    /// Fold another arena in whole, the shard merge's path. False when the
    /// two together would pass the ceiling, in which case nothing is
    /// appended and the merge drops that shard rather than half of it.
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
    /// Whether values differing only by case intern to one symbol. Keys
    /// in `map` are lowercased when set; the display casing gets picked
    /// from `variants` when the table finalizes.
    fold: bool,
    map: HashMap<Box<str>, u32>,
    table: Vec<String>,
    /// Per symbol, every casing seen and how many rows use it, so the
    /// most common spelling wins the display. Only filled when folding,
    /// so the exact path pays nothing.
    variants: Vec<HashMap<String, u32>>,
    /// Per symbol, every sort name seen and how many rows carry it, the
    /// same weighted pick `variants` makes. Filled whether or not the
    /// table folds, since a sort name isn't a casing question, but only
    /// by rows that actually carry one: a library without the tags leaves
    /// every map empty and costs a pointer per symbol.
    sorts: Vec<HashMap<String, u32>>,
}

impl Interner {
    fn folded(fold: bool) -> Self {
        Interner {
            fold,
            ..Default::default()
        }
    }

    /// Intern a value along with the sort name the row gave it, empty
    /// when it gave none.
    fn intern(&mut self, s: &str, sort: &str) -> u32 {
        self.intern_weighted(s, sort, 1)
    }

    /// Intern with a pre-counted weight, the shard merge's path: a shard
    /// hands over each casing with the row count it saw, so the display
    /// pick still reflects rows, not shards. The sort name is counted the
    /// same way and on the same weight.
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

    /// The casing a symbol will finalize under: the spelling the most rows
    /// voted for when the table folds, the one and only spelling when it
    /// doesn't. What the sort-name layers key on, since a lookup was made
    /// against a display name and the two sides have to meet on the same
    /// one.
    fn display(&self, sym: usize) -> &str {
        if self.fold {
            weighted_pick(&self.variants[sym])
                .map(String::as_str)
                .unwrap_or(&self.table[sym])
        } else {
            &self.table[sym]
        }
    }

    /// Fold another interner's symbols in, returning the old-to-new
    /// symbol map the shard merge remaps columns with.
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

    /// Replay a shard's sort-name counts onto the merged symbol, so the
    /// pick still reflects rows across the whole library rather than
    /// whichever shard happened to land last.
    fn absorb_sorts(&mut self, sym: u32, sorts: &HashMap<String, u32>) {
        for (sort, &weight) in sorts {
            *self.sorts[sym as usize].entry(sort.clone()).or_default() += weight;
        }
    }
}

/// The winner of a weighted vote over the casings or the sort names a value
/// was seen with: the spelling the most rows use, ties to the
/// lexicographically smaller so two loads of the same library land on the
/// same answer. Both the finalizing merge and the incremental append vote
/// this way, so a symbol a patch adds carries the name a rebuild would give
/// it.
fn weighted_pick(counts: &HashMap<String, u32>) -> Option<&String> {
    counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(s, _)| s)
}

/// Interned strings plus a folded copy for search: case and accents gone,
/// per [`crate::fold`], so "beyonce" reaches Beyoncé.
pub struct SymTable {
    pub strings: Vec<String>,
    pub lower: Vec<String>,
    /// The sort name each symbol carries, and a folded copy of it. A sort
    /// name belongs to the value, not the row, so it rides the table
    /// rather than costing every row a column. Both stay empty when no
    /// symbol in the table has one, which is the whole story for a
    /// Latin-only library: it pays nothing.
    pub sort: Vec<String>,
    pub sort_lower: Vec<String>,
}

impl From<Interner> for SymTable {
    fn from(mut interner: Interner) -> Self {
        // Folded symbols display as the casing the most rows use, ties
        // to the lexicographically smaller so reloads stay stable.
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
        // The same weighted pick the display casing makes, on the same
        // tie-break, so a reload of the same library lands on the same
        // sort name. A table where nothing carried one keeps both vectors
        // empty and the accessors fall through to the display name.
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

    /// The sort name for a symbol, empty when it has none.
    pub fn sort_name(&self, sym: usize) -> &str {
        self.sort.get(sym).map(String::as_str).unwrap_or("")
    }

    /// The folded sort name alone, empty when the symbol has none. What
    /// the grouped searches OR against, since they've already scanned the
    /// display name and don't want [`SymTable::sort_key`]'s fallback
    /// repeating that work.
    fn sort_lowered(&self, sym: usize) -> &str {
        self.sort_lower.get(sym).map(String::as_str).unwrap_or("")
    }

    /// The key a value is looked up under when a patch asks whether the
    /// table already holds it: the lowered name under case folding, the
    /// exact one otherwise, matching what [`Interner::intern_name`] keys on.
    ///
    /// Lowered, not folded, and computed rather than read off `lower`:
    /// symbol identity is a casing question only. Two artists spelled
    /// "Beyonce" and "Beyoncé" are two values a case-insensitive library
    /// still keeps apart, even though search now reaches both from the
    /// same needle.
    fn lookup_key(&self, sym: usize, fold: bool) -> Box<str> {
        if fold {
            self.strings[sym].to_lowercase().into()
        } else {
            self.strings[sym].as_str().into()
        }
    }

    /// Append a value a patch found nothing to match, with the sort name it
    /// arrived carrying. The new symbol is the table's length, as it would
    /// be under a full build; ranks and anything else keyed on symbol order
    /// go stale here and the patch invalidates them.
    fn push_symbol(&mut self, display: &str, sort: &str) -> u32 {
        let sym = self.strings.len() as u32;
        // The sort columns pad against the table as it stands, before the
        // new value lands in it, or the pad would fill the slot the push
        // below is about to want.
        if !self.sort.is_empty() || !sort.is_empty() {
            self.fill_sorts();
            self.sort.push(sort.to_string());
            self.sort_lower.push(crate::fold::fold(sort));
        }
        self.strings.push(display.to_string());
        self.lower.push(crate::fold::fold(display));
        sym
    }

    /// Give a symbol the sort name a row just brought in, but only where it
    /// has none: a value's sort name is a fact about the value, so the first
    /// row to carry one settles it, and the vote that would weigh a second
    /// spelling belongs to the next full rebuild.
    fn adopt_sort(&mut self, sym: usize, sort: &str) -> bool {
        if sort.is_empty() || !self.sort_name(sym).is_empty() {
            return false;
        }
        self.fill_sorts();
        self.sort[sym] = sort.to_string();
        self.sort_lower[sym] = crate::fold::fold(sort);
        true
    }

    /// Lay the library's own sort names over the ones the files carried,
    /// filling only where a symbol has none: a tag beats a lookup, since
    /// the tag is what this library's owner chose to write down.
    /// Keyed by the display string, which is the spelling the pass looked
    /// the artist up under, so the two sides meet on the same casing.
    ///
    /// Returns how many symbols took one, so a caller can tell a table the
    /// merge moved from one it left alone.
    fn fill_from_meta(&mut self, meta: &HashMap<String, String>) -> usize {
        let mut filled = 0;
        for sym in 0..self.strings.len() {
            if !self.sort_name(sym).is_empty() {
                continue;
            }
            // Cloned rather than borrowed, since adopting it takes the
            // table mutably and the key came off the table.
            let Some(sort) = meta.get(&self.strings[sym]).cloned() else {
                continue;
            };
            if self.adopt_sort(sym, &sort) {
                filled += 1;
            }
        }
        filled
    }

    /// Give a table that carried no sort names at all the empty columns to
    /// hold one, the moment a patch brings the first.
    fn fill_sorts(&mut self) {
        if self.sort.len() < self.strings.len() {
            self.sort.resize(self.strings.len(), String::new());
            self.sort_lower.resize(self.strings.len(), String::new());
        }
    }

    /// What ordering and matching key off: the folded sort name when the
    /// symbol has one, its folded display name otherwise. Folded, so
    /// Émilie files under E with the rest of them rather than after Z.
    pub fn sort_key(&self, sym: usize) -> &str {
        match self.sort_lower.get(sym) {
            Some(s) if !s.is_empty() => s,
            _ => &self.lower[sym],
        }
    }
}

/// What a ReplayGain column holds for a row whose file has no gain of
/// that kind. Sorts ahead of every real value the way an unrated row or a
/// missing year does, and decodes back to None.
pub const NO_GAIN: i16 = i16::MIN;

/// A tagged gain packed for the projection: hundredths of a dB in an i16.
/// Every real gain falls well inside the +-40 dB the engine will act on, so
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
    /// The per-row sort titles, beside the title arenas because a title
    /// is never interned and so has nowhere else to live. Empty strings
    /// for the rows that carry none; the merge decides whether the whole
    /// library kept anything worth holding on to.
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
    artists: Interner,
    album_artists: Interner,
    albums: Interner,
    genres: Interner,
    codecs: Interner,
    folders: Interner,
    /// Whether any row was refused because the title arena is full. The
    /// shard is still coherent, it's just short rows, so the merge can go
    /// ahead and the incremental patch can't: a patch that silently drops
    /// what it was asked to apply would leave the projection disagreeing
    /// with the database until somebody rescanned.
    overflowed: bool,
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

    /// Lay the library's own sort names over what the files carried, the
    /// incremental counterpart to the three `fill_*_sorts` passes a full
    /// load runs after its merge. Same rule as there: a tag the owner wrote
    /// beats a name rox looked up, so this only fills where the shard's
    /// rows brought nothing.
    ///
    /// It lands on the shard rather than on the projection so the patch and
    /// the rebuild agree by construction. A sort name laid here goes
    /// through [`absorb_shard`] like any other, which means a value the
    /// projection already knows keeps whatever it settled on and a value
    /// arriving for the first time gets pushed carrying the same name a
    /// reload would have given it.
    ///
    /// The artist and album tables are read whole, since they're keyed by
    /// value and hold a row per name anyone ever looked up. Track sort
    /// titles are keyed by id and scale with the library, so those are
    /// asked for by id: a patch is meant to cost what changed.
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

    /// The title half of [`Builder::lay_meta_sorts`], and the awkward one
    /// for [`Projection::fill_track_sorts`]'s reason: sort titles live in
    /// an append-only arena addressed by row, so filling one row's means
    /// rebuilding the pair. A shard is a handful of rows, so that's a
    /// handful of pushes.
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
            // A refusal leaves the shard exactly as the files built it,
            // which is the state it was already in.
            if !display.push(sort) || !lower.push_folded(sort) {
                return;
            }
        }
        self.title_sort = display;
        self.title_sort_lower = lower;
    }

    /// The four title arenas for one row, all or none: a row that lands in
    /// two of them and not the other two puts every column after it out of
    /// step, so a refusal rolls the earlier pushes back out.
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

    /// Take one row. False when the arena is full and the row was dropped
    /// whole; the shard remembers it either way.
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
        // Nothing tags a sort genre, a sort codec or a sort folder, so
        // those three intern with no sort name and their tables stay empty.
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
        // Interned per album directory, so it stays cheap even at ten
        // million rows; an empty parent (a bare filename) folds to "".
        let folder = Path::new(row.path)
            .parent()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default();
        self.folder.push(self.folders.intern(&folder, ""));
        true
    }

    /// How many rows the shard took.
    pub fn len(&self) -> usize {
        self.db_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.db_id.is_empty()
    }

    /// The ids it holds, in the order they were read. What tells a caller
    /// which of the ids it asked for still have a row and which have gone.
    pub fn ids(&self) -> &[i64] {
        &self.db_id
    }
}

/// Give a shard's symbols the sort names a meta table holds for them,
/// filling only where no row voted one in. Counted as a single vote, so a
/// symbol some of whose rows do carry a tag keeps the tag: the weighted
/// pick never sees this at all, because it isn't asked when the map is
/// already occupied.
///
/// Keyed on the display casing, which is the spelling the lookup was made
/// under, matching what [`SymTable::fill_from_meta`] keys on.
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

/// The sort titles the track meta table holds for exactly these ids. The
/// full load reads that table whole because it's about to touch every row
/// anyway; a patch asks by id, since the table is keyed by one and grows
/// with the library rather than with what changed.
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

/// Fold one shard's symbol table into a finalized one, the incremental
/// counterpart to [`Interner::absorb`]: same question, asked of a table
/// that has already been voted on and can't be re-voted. A value the table
/// knows keeps its symbol and its display casing; one it doesn't gets
/// appended with the casing the shard's own rows voted for. Returns the
/// shard-local to projection symbol map, and whether anything about the
/// table moved, which is what makes the cached ranks stale.
///
/// A symbol that has no sort name yet takes the one an arriving row
/// carries: a sort name is a fact about the value, so the first row to know
/// it settles it, and a second spelling is the next full rebuild's problem.
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
                    // A value the table already held now files somewhere
                    // else. Every row that was already using it moves with
                    // it, which no amount of patching an existing order can
                    // express.
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

/// What folding one shard's symbols into a finalized table did, the answer
/// [`absorb_shard`] gives. Three facts rather than a tuple because the last
/// two read alike and mean very different things.
struct Absorbed {
    /// Shard-local symbol to projection symbol.
    map: Vec<u32>,
    /// The table changed at all, so the cached ranks are stale. A symbol
    /// appended is enough: ranks are positions, and a value landing in the
    /// middle of the alphabet shifts every rank after it.
    moved: bool,
    /// A symbol that already had rows on it took a sort name, so it can
    /// cross other symbols in the ranking. That reorders rows the caller
    /// already has in its order, which is the one thing a merge into that
    /// order can't do; the caller has to sort again instead.
    reordered: bool,
}

/// Read a named set of rows into a shard, the transport an incremental
/// patch travels in. The same [`Builder`] the sharded load fills, so the
/// interning, the folder derivation, and the sort-name bookkeeping are the
/// ones the full build does rather than a second copy of them that can
/// drift. `fold` has to be the live projection's, not the current setting:
/// the patch merges into symbol tables interned under the old one.
///
/// The ids are deduped first. A caller collecting them from both sides of a
/// reindex, or from a rename and the re-read that follows it, will name the
/// same row twice without meaning anything by it, and [`store::rows_for_ids`]
/// reads a range per id: without this the shard holds that row twice, both
/// copies land in the projection, and the second one tombstones the first
/// only because [`Projection::apply_upserts`] happens to walk them in order.
///
/// The three sort-name tables get laid over the shard here, the same layer
/// [`Projection::load_serial`] runs after its merge. Without it a row coming
/// back through a patch loses the sort name the library looked up for it,
/// and the projection quietly stops matching what a reload would give.
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

pub struct Projection {
    /// Whether the name symbols interned case-folded, the library's
    /// case-insensitive setting at load time. Matching against symbol
    /// strings folds the same way when set, so a stale pick made under
    /// the other casing still matches.
    pub fold: bool,
    pub db_id: Vec<i64>,
    pub title: Arena,
    pub title_lower: Arena,
    /// Display and lowered sort titles, one entry per row. None when no row
    /// in the library carries a sort title, which is the common case and
    /// costs nothing; Some pays two u32 offsets a row. Titles are the one
    /// sort name that can't ride a symbol table, since titles are never
    /// interned.
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
    /// the ratings' reason: a new listen bumps its track in place,
    /// so a play never pays a projection reload. Per ADR 11 the events
    /// stay the source; this column only caches their per-track count.
    pub plays: Vec<AtomicU32>,
    /// The two ReplayGain figures a row has (ADR 19), packed to
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
    /// The cue tracks' spans, keyed by row index. Sparse per the ADR 5
    /// memory discipline: a library with no cue sheets holds an empty map
    /// instead of a dense column of None, and even a library full of them
    /// only pays per cue row. Nothing reads this to show a duration, since
    /// duration_ms is already on the row; it's for the player.
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
    /// The same ranks again for the sort columns, which order by the sort
    /// name alone and never by the display name. Lazy per table like the
    /// ranks above, so a library nobody ever sorts by a sort column pays
    /// nothing, and boxed together like [`SymIndex`] because the whole
    /// projection travels inside a message: three more inline `OnceLock`s
    /// is a hundred bytes every load carries for a column most people
    /// never turn on.
    sort_ranks: OnceLock<Box<SortRanks>>,
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
    /// Boxed like [`SymIndex`] and for the same reason: a whole `SymTable`
    /// inline is a hundred bytes on every projection, and the projection
    /// itself rides inside the catalog's load message.
    genre_terms: OnceLock<Box<SymTable>>,
    /// Which rows a patch has retired: the row an upsert replaced, or one
    /// whose file is gone. The columnar arenas are append-only, so a row
    /// can't be taken out of them; a tombstone is how it leaves the
    /// library's answers without the whole projection being rebuilt for
    /// one changed file. Every scan skips them, and the count says when
    /// the dead weight has earned a rebuild.
    dead: Vec<bool>,
    dead_rows: usize,
    /// Value to symbol per table, built the first time the projection is
    /// patched and kept warm after: the finalized tables are vectors, so
    /// without this an append would scan a hundred thousand strings to ask
    /// whether it already knows an artist. Costs nothing on a projection
    /// nothing ever patches, which is every projection in a bench and most
    /// of them in a session.
    sym_index: Option<Box<SymIndex>>,
}

/// The sort-name-only ranks behind the sort columns, one per symbol table
/// that has them, each filled the first time its column is sorted. Boxed
/// as a set so an unsorted projection carries a pointer rather than three
/// caches it will never fill.
#[derive(Default)]
struct SortRanks {
    artists: OnceLock<Vec<u32>>,
    album_artists: OnceLock<Vec<u32>>,
    albums: OnceLock<Vec<u32>>,
}

/// The lookup maps behind an incremental append, one per symbol table.
/// Seeded from the finalized table on first use and grown with it after,
/// keyed the way [`Interner::intern_name`] keys: lowered under case
/// folding, exact otherwise.
#[derive(Default)]
struct SymIndex {
    artists: Option<HashMap<Box<str>, u32>>,
    album_artists: Option<HashMap<Box<str>, u32>>,
    albums: Option<HashMap<Box<str>, u32>>,
    genres: Option<HashMap<Box<str>, u32>>,
    codecs: Option<HashMap<Box<str>, u32>>,
    folders: Option<HashMap<Box<str>, u32>>,
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
        ]
        .into_iter()
        .flatten()
        .map(|map| map.keys().map(|k| k.len() + 32).sum::<usize>())
        .sum()
    }
}

/// What one patch changed, so whatever a caller keeps beside the projection
/// can be fixed instead of rebuilt: the catalog's id to row index, and the
/// canonical order the browse panels read.
#[derive(Default, Debug)]
pub struct Patch {
    /// Rows appended, in arrival order.
    pub added: Vec<u32>,
    /// Rows tombstoned, whether replaced by an upsert or removed outright.
    pub dropped: Vec<u32>,
    /// The ids that left the library altogether. An id whose row was
    /// replaced is not one of these: it's still here, at a new row.
    pub gone: Vec<i64>,
    /// Rows the caller already had can have changed places with each
    /// other, so an order built before this patch can't be merged into,
    /// only rebuilt. It happens when a value the projection already knew
    /// takes a sort name it didn't have: the value files somewhere else
    /// now, and every row using it moves. [`Projection::patch_order`]
    /// binary-searches the old order under the new ranks, so handing it an
    /// order this invalidated puts rows in arbitrary places rather than
    /// wrong ones, which is worse.
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
    /// The file's own ReplayGain figures in dB, None where it has none.
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
    /// folder and codec: a bare number is a plausible title or year, and
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

    /// Whether the field has an absent value a bare `-field` token can ask
    /// for. A scanned row always has a folder and a codec, and an added
    /// date is either a stamp or the epoch rather than a blank, so those
    /// three have nothing to be missing and `-folder` stays free text.
    /// What counts as absent per field: 0 for `year`, the empty string for
    /// the name fields, unrated for `rating`, and no plays for `plays`.
    pub fn absence(self) -> bool {
        !matches!(
            self,
            QueryField::Folder | QueryField::Codec | QueryField::Added
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
/// builds one (it drops back to a free term instead), so this only keeps
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

/// How a term reads against a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermMode {
    /// The plain form, `value` or `field:value`: the row has to match.
    Match,
    /// The negated form, `-field:value`: the row has to not match. Only a
    /// pinned term takes this; a leading hyphen on anything else is
    /// literal text.
    Exclude,
    /// The absence form, `-field`: the row's value for the field has to be
    /// missing, per [`QueryField::absence`]. The needle is empty and a
    /// numeric field's `num` is unset, since there's nothing to compare.
    Absent,
}

/// One parsed query term: a folded needle, maybe pinned to one field.
/// A term pinned to a numeric field holds its comparison in `num` and
/// leaves the needle as the raw value text. `mode` says whether the term
/// reads positive, negated, or as a test for the field being absent.
pub struct Term {
    pub field: Option<QueryField>,
    pub needle: String,
    /// The comparison behind a numeric pin; None for every text term.
    pub num: Option<NumTerm>,
    pub mode: TermMode,
}

impl Term {
    /// Whether the term's row test has to be flipped. Absence terms are
    /// already written as the test they mean, so only [`TermMode::Exclude`]
    /// inverts.
    fn negated(&self) -> bool {
        self.mode == TermMode::Exclude
    }
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
///
/// A leading hyphen on a pinned term negates it: `-artist:"daft punk"` is
/// every row that term wouldn't take, numeric pins included, so
/// `-rating:>=4` is everything under four stars and `-added:<90d` is
/// everything added before the last 90 days. A bare `-field` with no colon
/// asks for the field being absent, for the fields that have an absent
/// value (see [`QueryField::absence`]): `-year` is the untagged years,
/// `-genre` the rows with no genre at all.
///
/// The hyphen only means something in front of a known field. `-foo`,
/// `-folder` (a field with no absent value), a lone `-`, and `-word` all
/// stay free text terms for the literal characters, the same fallback an
/// unknown prefix takes. There's no negation of free text: `-stronger`
/// looks for the string "-stronger", it doesn't exclude "stronger".
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
    // The pinned forms, `field:value` and its negation. None sends the
    // token back to being free text.
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
        // A numeric pin with nothing numeric behind it is not a filter
        // anybody meant; let it read as text.
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
                    // No colon: a known field name on its own asks for the
                    // field being absent, everything else is literal text.
                    let name = body.to_lowercase();
                    if let Some(&(_, field)) = QUERY_FIELDS.iter().find(|(n, _)| *n == name) {
                        if field.absence() {
                            return Term {
                                field: Some(field),
                                needle: String::new(),
                                num: None,
                                mode: TermMode::Absent,
                            };
                        }
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
        // An absence term carries no needle by design; every other empty
        // one is a token that said nothing, `artist:` and its negation
        // alike.
        .filter(|t| t.mode == TermMode::Absent || !t.needle.is_empty())
        .collect()
}

/// A field the structured filter can pin exact values to: the interned
/// columns plus the year. Titles stay out; a text term already covers
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
/// leaves "Airborne" out. Years appear as their decimal strings ("0" for
/// untagged) to keep the value lists one shape. Folder picks are the one
/// exception to whole-value matching: a picked folder covers its whole
/// subtree, so the folder tree scopes to a branch with a single value
/// instead of enumerating every descendant.
///
/// A set can also pin an explicit set of track db ids, which is how a view
/// following the app-wide selection narrows to it. It's part of the set
/// rather than a field beside the filter because every searching panel
/// already threads a `FilterSet` down to its row scan, so honoring it in the
/// two matchers below covers all of them at once. `None` is no id
/// restriction at all; `Some` of an empty set matches nothing, which is
/// what an emptied selection should show.
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
            ids: Some(ids.into_iter().collect()),
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
    /// rows hold raw strings while picks hold the folded tables' display
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
/// track list that isn't the projection. The queue, history, and playlists
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

/// Whether a folder is at or under a picked one: the pick itself, or a
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

/// Substring test with a needle already folded by [`parse_query`]. The
/// haystack is a raw field off a row rather than a prepared column, so it
/// folds here, per call: these are the panels that hold their own rows and
/// have no interned table to have folded once. An empty needle matches
/// everything.
fn contains_fold(haystack: &str, needle_folded: &str) -> bool {
    needle_folded.is_empty() || crate::fold::fold(haystack).contains(needle_folded)
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
/// Terms AND together; needles come folded from [`parse_query`].
///
/// A negated term (`-artist:daft`) is the plain test flipped, and an
/// absence term (`-genre`) asks the field for its missing value.
///
/// The numeric pins read columns a plain row list doesn't have (rating,
/// play count, and added date are on the projection), so they match
/// nothing here. A `rating:>=4` typed into the queue or playlists box
/// comes back empty rather than quietly ignoring the term; the catalog's
/// own views (and smart playlists) run through [`Projection::search`],
/// where the columns exist. Their negated and absent forms come back
/// empty too, on the same grounds: a row whose rating this list never saw
/// isn't known to be under four stars either, so flipping the miss into a
/// hit would answer a question these fields can't answer.
pub fn track_matches(terms: &[Term], fields: &TrackFields) -> bool {
    terms.iter().all(|t| {
        // The three projection-only columns, in every mode.
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
                // Folder and codec have no absent form, so the parser
                // never builds one; a free term can't be absent either.
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
            Some(QueryField::Year) => fields.year.to_string().contains(t.needle.as_str()),
            Some(QueryField::Rating | QueryField::Plays | QueryField::Added) => false,
        };
        hit != t.negated()
    })
}

/// A sortable column of the projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    Title,
    /// The sort title alone, for the column that shows it. Rows carrying
    /// one come first in the column's own order and the rest sit at the
    /// bottom in both directions, the way a null sorts last. That's the
    /// difference from [`SortKey::Title`], which falls back to the display
    /// title and so files a tagged row among the untagged ones. The four
    /// sort keys below all read this way.
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
    /// The gain the Track leveling mode would read: the track figure, the
    /// album one where a file only has that, matching what the engine
    /// falls back to and what the Gain column draws.
    TrackGain,
    /// The same the other way round, for the Album mode.
    AlbumGain,
    /// How fast the track runs, whichever source wrote the number.
    Bpm,
}

/// The leading half of a sort column's key, which decides only whether a
/// row lands in the valued block or the empty one. `order_view` reverses
/// the whole key on a descending sort, so the two groups swap sides here
/// to come out the same way round after the reversal: values first,
/// empties last, in both directions.
fn valued_group(descending: bool) -> u8 {
    u8::from(descending)
}

fn empty_group(descending: bool) -> u8 {
    u8::from(!descending)
}

/// What a symbol-backed sort column orders a row by: its symbol's rank
/// among the symbols that carry a sort name, or the empty group with a
/// constant rank behind it, so every row without one ties and falls
/// through to the canonical tie-break instead of ordering by a rank it
/// was never given.
fn sort_only_key(table: &SymTable, rank: &[u32], sym: usize, descending: bool) -> (u8, u32) {
    if table.sort_lowered(sym).is_empty() {
        (empty_group(descending), 0)
    } else {
        (valued_group(descending), rank[sym])
    }
}

impl Projection {
    /// How many rows the columns hold, live and tombstoned together. The
    /// bound every row index is under, so this stays what an index loop
    /// asks for; [`Projection::live_len`] is the number to show a person.
    pub fn len(&self) -> usize {
        self.db_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.db_id.is_empty()
    }

    /// How many rows a browse actually sees.
    pub fn live_len(&self) -> usize {
        self.db_id.len() - self.dead_rows
    }

    /// Whether a row has been tombstoned by a patch. Nothing that goes
    /// through search, the filters or the canonical order ever hands one
    /// out; this is for callers holding a row index from before a patch.
    pub fn is_dead(&self, row: u32) -> bool {
        self.dead.get(row as usize).copied().unwrap_or(true)
    }

    pub fn dead_rows(&self) -> usize {
        self.dead_rows
    }

    /// What share of the columns is dead weight. The catalog watches this
    /// to decide when patching has cost more than a rebuild would: the
    /// arenas and the symbol tables only grow between rebuilds, so a
    /// library that churns pays for its own compaction.
    pub fn dead_fraction(&self) -> f64 {
        if self.db_id.is_empty() {
            return 0.;
        }
        self.dead_rows as f64 / self.db_id.len() as f64
    }

    /// Every live row in row order, the starting point for the passes that
    /// would otherwise be a bare range over the columns.
    fn live_rows(&self) -> Vec<u32> {
        if self.dead_rows == 0 {
            return (0..self.len() as u32).collect();
        }
        (0..self.len() as u32)
            .filter(|&row| !self.dead[row as usize])
            .collect()
    }

    /// Load on one connection, one thread: the ADR 5 shape as written.
    /// `fold` merges values differing only by case into one symbol, the
    /// case-insensitive library setting.
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

    /// Lay [`crate::artist_meta`] over the two artist tables, so an artist
    /// whose files carry no `ARTISTSORT` still files and searches under the
    /// sort name the library looked up for it.
    ///
    /// After the merge rather than inside it, for the reason `fill_spans`
    /// is: it reads a table the shard loaders never touch, and by here the
    /// symbols are final. Before anything reads a rank, since those are
    /// `OnceLock`s keyed on exactly what this changes.
    ///
    /// Only the artist and album-artist tables. MusicBrainz has no album
    /// sort name in its model; the album table gets its own layer from
    /// [`Projection::fill_album_sorts`], out of a table the romanization
    /// pass fills rather than a lookup.
    fn fill_artist_sorts(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        let meta = crate::artist_meta::load_all(conn)?;
        // The overwhelmingly common case, and the one the empty sort
        // vectors exist for: nothing has been looked up, so nothing is
        // laid over anything and both tables stay exactly as the files
        // built them.
        if meta.is_empty() {
            return Ok(());
        }
        self.artists.fill_from_meta(&meta);
        self.album_artists.fill_from_meta(&meta);
        Ok(())
    }

    /// Lay [`crate::album_meta`] over the album table, the same move
    /// [`Projection::fill_artist_sorts`] makes one table over.
    ///
    /// A separate table and a separate pass rather than a column on the
    /// artist one, because the two are keyed by different things and an
    /// album called "Home" must not inherit a sort name written for a band
    /// called "Home".
    fn fill_album_sorts(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        let meta = crate::album_meta::load_all(conn)?;
        if meta.is_empty() {
            return Ok(());
        }
        self.albums.fill_from_meta(&meta);
        Ok(())
    }

    /// Lay [`crate::track_meta`] over the per-row sort titles.
    ///
    /// The awkward one, and the reason it's worth saying why. The other
    /// two land on symbol tables, which carry a sort name per symbol and
    /// can take one at any time. Titles live in an arena addressed by row,
    /// and an arena is append-only: there's no way to write row 900's sort
    /// title into the middle of one. So this rebuilds both arenas, taking
    /// each row's existing sort title where it has one and the table's
    /// where it doesn't. That's one pass over the rows and one copy of the
    /// text, once per projection build, and only when the table holds
    /// something.
    ///
    /// A file's own sort tag wins, matching the rule everywhere else: what
    /// this library's owner wrote down beats what rox worked out.
    ///
    /// It also has to run here rather than inside the merge, and after the
    /// merge has decided whether to keep the arenas at all. A library
    /// whose files carry no sort titles collapses them to None, so this is
    /// where the option gets built back up when the pass has filled some.
    fn fill_track_sorts(&mut self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        let meta = crate::track_meta::load_all(conn)?;
        // The common case, and the one the zero-cost None exists for:
        // nothing has been romanized, so the arenas stay exactly as the
        // files built them.
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
            // The rebuild can't overflow where the original didn't: a
            // romanization is Latin and the arena ceiling is four
            // gigabytes. A push that refuses anyway leaves the projection
            // with what the files said, which is the state it was already
            // in.
            if !display.push(sort) || !lower.push_folded(sort) {
                return Ok(());
            }
        }
        self.title_sort = Some((display, lower));
        Ok(())
    }

    /// Fill the plays column from the listens table: one aggregate query,
    /// then one pass mapping counts onto rows by track id.
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
            // A shard whose text won't fit under the ceiling is dropped
            // whole rather than half-appended: the columns are positional,
            // and half a shard would shift every row that follows it. The
            // arena has already said so in the log.
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
            out.db_id.extend_from_slice(&shard.db_id);
            // Checked to fit above, so these can't refuse.
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
        }

        let rows = out.db_id.len();
        let plays = (0..rows).map(|_| AtomicU32::new(0)).collect();
        // No row in the library carried a sort title, the common case, so
        // the offsets go too and the whole feature costs nothing per row.
        // One row with one keeps both arenas whole, which is what lets the
        // search scan stay an arena `get` instead of a map probe.
        let title_sort = if out.title_sort.is_blank() {
            None
        } else {
            Some((out.title_sort, out.title_sort_lower))
        };
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
            sort_ranks: OnceLock::new(),
            distinct_artists: OnceLock::new(),
            distinct_albums: OnceLock::new(),
            genre_terms: OnceLock::new(),
            // A freshly built projection has no dead rows by construction:
            // it is exactly what the database holds.
            dead: vec![false; rows],
            dead_rows: 0,
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
            sub: self.sub[i],
        }
    }

    /// A row's sort title, empty when it has none or when no row in the
    /// library does.
    pub fn title_sort(&self, row: usize) -> &str {
        match &self.title_sort {
            Some((display, _)) => display.get(row),
            None => "",
        }
    }

    /// The folded sort title alone, empty when the row has none. What the
    /// Title sort column orders by, since it shows the sort title and a
    /// row without one belongs at the bottom rather than filed under its
    /// display title.
    fn title_sort_lowered(&self, row: usize) -> &str {
        match &self.title_sort {
            Some((_, lower)) => lower.get(row),
            None => "",
        }
    }

    /// What ordering and matching key off for a row's title: its lowered
    /// sort title where it has one, its lowered title otherwise.
    pub fn title_sort_key(&self, row: usize) -> &str {
        match &self.title_sort {
            Some((_, lower)) => match lower.get(row) {
                "" => self.title_lower.get(row),
                s => s,
            },
            None => self.title_lower.get(row),
        }
    }

    /// The cue span a row plays, None for a plain file. Sparse lookup, so
    /// this is a hash probe rather than an index.
    pub fn span(&self, row: u32) -> Option<crate::cue::Span> {
        self.spans.get(&row).copied()
    }

    /// Case-folded substring search, one term at a time per
    /// [`parse_query`]: a free term matches title, artist, album artist,
    /// album, or genre; a pinned term only its field, and a term with a
    /// leading hyphen excludes rather than includes. Terms AND together.
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
            return self.live_rows();
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
            /// A `-title` absence term. Titles live in an arena rather
            /// than a symbol table, so there's no mask to precompute,
            /// only the row's own length to look at.
            TitleEmpty,
            Year(Vec<bool>),
            /// A numeric pin: which column to read, and the comparison it
            /// has to satisfy. Nothing to precompute, the columns are
            /// already numbers.
            Num(QueryField, NumTerm),
        }

        // One mask per table, and the single place a sort name enters
        // search: a symbol matches on its displayed name or on the Latin
        // name it sorts under, so typing "yonezu" finds 米津玄師 through
        // free terms and through every `field:` pin at once. A table
        // where nothing carries a sort name has an empty `sort_lower`,
        // so genre, codec and folder behave exactly as they did.
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
        // An absence term is written here as the test it means rather than
        // carried through the scan as a mode: an empty symbol, the zero
        // year, or a comparison against a zero column. That keeps the row
        // scan on the shapes it already had, and leaves only the exclusion
        // terms to flip.
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
                // Only the untagged year, so the mask is one true cell
                // over the same 65k the positive form covers.
                QueryField::Year => {
                    let mut mask = vec![false; u16::MAX as usize + 1];
                    mask[0] = true;
                    Hits::Year(mask)
                }
                // Unrated and never played both read as a zero column.
                // Folder, codec and added have no absent form, so the
                // parser never sends one this way.
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
                    // An absence pinned to nothing is not a term the
                    // parser builds; falling through leaves it the empty
                    // free term, which takes every row.
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
                        Some(
                            field @ (QueryField::Rating | QueryField::Plays | QueryField::Added),
                        ) => Hits::Num(field, t.num.unwrap_or(NUM_NEVER)),
                    },
                };
                (t.negated(), hit)
            })
            .collect();

        self.scan_rows(|i| {
            // Tombstones are already out (scan_rows drops them before the
            // predicate), so a negated term flips only live rows.
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
                        // Days since the row was scanned in, so the query
                        // reads forward ("added in the last 90 days") while
                        // the column counts backward. A row with no added
                        // stamp (0) is ancient and drops out of every
                        // recency term, which is the right answer for a
                        // library that never got one.
                        _ => (now - self.added[i]) / 86_400,
                    }),
                };
                hit != *negated
            })
        })
    }

    /// Whether a row's title or its sort title holds the needle. The
    /// `Option` check is per row rather than per query, but it's a branch
    /// on a field the loop already has hot, and a library without sort
    /// titles never reaches the second find.
    fn title_hit(&self, finder: &memmem::Finder<'_>, i: usize) -> bool {
        if finder.find(self.title_lower.get(i).as_bytes()).is_some() {
            return true;
        }
        match &self.title_sort {
            Some((_, lower)) => finder.find(lower.get(i).as_bytes()).is_some(),
            None => false,
        }
    }

    /// The distinct album artists whose name matches the query, each with a
    /// representative row for the cover and count. For the search's grouped
    /// hits, so typing an artist's name surfaces the artist itself above the
    /// tracks. A term pinned to a track-only field (title, album, genre,
    /// year, codec) excludes every artist, since it can't match an artist
    /// name.
    ///
    /// A negated term reads the same way one field down: `-artist:daft`
    /// keeps the artists whose name doesn't hold "daft", and an exclusion
    /// on a track-only field (`-genre:rock`) excludes every artist, the
    /// same as its positive form. That's the honest answer for a head with
    /// no such column: an artist isn't "not rock", some of their tracks
    /// are. Absence terms drop every artist too, since a listed head
    /// always has a name and the rest are track columns.
    /// Ordered by name; first-seen row per artist.
    pub fn search_artists(&self, query: &str) -> Vec<ArtistHit> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return Vec::new();
        }
        // The sort name matches here too, or quick-play would miss a
        // romanized artist the library panel finds, which reads as a bug.
        let matches = |name_lower: &str, sort_lower: &str| {
            terms.iter().all(|t| {
                if t.mode == TermMode::Absent {
                    return false;
                }
                let hit = match t.field {
                    None | Some(QueryField::Artist) | Some(QueryField::AlbumArtist) => {
                        name_lower.contains(&t.needle) || sort_lower.contains(&t.needle)
                    }
                    // Nothing here to match, so the term takes no artist
                    // whichever way it points.
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

    /// The distinct albums whose album or album-artist name matches the
    /// query, each keyed by its (album artist, album) pair with a
    /// representative row for the cover and year. A free term matches
    /// either name; `album:` pins the album, `artist:`/`albumartist:` the
    /// artist; a title, genre, year, or codec term excludes every album.
    /// Negation follows the same split as [`Projection::search_artists`]:
    /// `-album:live` keeps the albums whose name doesn't hold "live", and
    /// an exclusion or absence on a field no album head carries excludes
    /// every album.
    /// Ordered by artist then album; first-seen row per pair.
    pub fn search_albums(&self, query: &str) -> Vec<AlbumHit> {
        let terms = parse_query(query);
        if terms.is_empty() {
            return Vec::new();
        }
        // Both names carry their sort form, for the same reason
        // `search_artists` does.
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

    /// The distinct release years present, newest first, zero (unknown)
    /// dropped. The year field has no symbol table to suggest from, so its
    /// value completions draw from this instead.
    pub fn distinct_years(&self) -> Vec<u16> {
        let mut years: Vec<u16> = self
            .year
            .iter()
            .enumerate()
            .filter(|&(row, &y)| y != 0 && !self.dead[row])
            .map(|(_, &y)| y)
            .collect();
        years.sort_unstable_by(|a, b| b.cmp(a));
        years.dedup();
        years
    }

    /// Row mask for a structured filter: a row passes when, for every
    /// filtered field, its value is one of that field's picks: values OR
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
                // holding it as one of its values, the same per-symbol
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

        // The pin is a set because the row scan runs the whole catalog, so
        // the per-row lookup has to be O(1) or the scan goes quadratic.
        let pinned = filter.ids.as_ref();

        Some(
            (0..self.len())
                .into_par_iter()
                .map(|i| {
                    // A tombstoned row passes nothing: masks are indexed by
                    // row, so a caller intersecting one against a view built
                    // before a patch still can't reach a dead row through it.
                    if self.dead[i] {
                        return false;
                    }
                    if let Some(pinned) = pinned {
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

    /// The rows with one genre value, "; " lists included: asking for
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
        // One branch on a byte the loop already has in cache, against a
        // whole-projection rebuild per changed file. Tombstones are the
        // only thing standing between a scan and a row.
        let live = |i: usize| !self.dead[i];
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

    /// Alphabetical rank per symbol, so sort comparisons are integer, never
    /// string (the non-functional model's precomputed-keys claim). Ranked
    /// on the sort key, which is the sort name where the symbol has one:
    /// the choice is per symbol, so a library where only some artists
    /// carry the tag still orders the rest by their display name.
    /// Ties break on the symbol id, which is the one thing about a symbol
    /// that doesn't move. The sort is unstable and runs in parallel, so two
    /// symbols with the same key ("Tie" and "tie" in a library that doesn't
    /// fold) would otherwise land in whichever order the threads happened
    /// to produce, and a patch that recomputes the ranks could swap rows
    /// nothing about the library changed.
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

    /// Alphabetical rank among the symbols that actually carry a sort
    /// name, for the columns that show the sort name and nothing else. The
    /// symbols without one are ranked nowhere and read 0 here; nothing
    /// looks their rank up, because [`sort_only_key`] answers for them
    /// before it indexes.
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

    // The cached lowered-order ranks per symbol table: ranked once on the first
    // sort that needs them, reused after. Every sort's tie-break needs the
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

    /// The distinct album artists in first-seen row order, cached. The
    /// per-query search_artists filters these by name, so the O(rows) distinct
    /// pass happens once instead of every keystroke.
    fn distinct_artists(&self) -> &[ArtistHit] {
        self.distinct_artists.get_or_init(|| {
            let mut seen: HashSet<u32> = HashSet::new();
            let mut out: Vec<ArtistHit> = Vec::new();
            for row in 0..self.len() as u32 {
                if self.dead[row as usize] {
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

    /// The distinct genre values across the library, "; " lists split
    /// into their parts, each once in first-seen order with the folded
    /// copy suggestion filtering wants. A folded library merges case
    /// variants here too, the display going to the casing the most rows use.
    /// The symbols only folded whole strings, so parts shared across
    /// different lists still need their own pass.
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
                let lower = strings.iter().map(|s| crate::fold::fold(s)).collect();
                return Box::new(SymTable {
                    strings,
                    lower,
                    sort: Vec::new(),
                    sort_lower: Vec::new(),
                });
            }
            let mut rows = vec![0u32; self.genres.strings.len()];
            for (row, &sym) in self.genre.iter().enumerate() {
                if !self.dead[row] {
                    rows[sym as usize] += 1;
                }
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
            let lower = strings.iter().map(|s| crate::fold::fold(s)).collect();
            // Genre parts carry no sort names: nothing tags a sort genre,
            // and this table only feeds value suggestions.
            Box::new(SymTable {
                strings,
                lower,
                sort: Vec::new(),
                sort_lower: Vec::new(),
            })
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
                if self.dead[row as usize] {
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

    /// The canonical browse order: album artist, album, disc, track number.
    /// The album artist keys it so an album's tracks stay one run under its
    /// credited artist, per-track guests and all; the disc keys ahead of the
    /// track so a multi-disc set plays through in order instead of
    /// interleaving its discs' track numbers.
    pub fn sort_canonical(&self) -> Vec<u32> {
        let a_rank = self.album_artist_ranks();
        let b_rank = self.album_ranks();
        let mut idx = self.live_rows();
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
        let mut idx = self.live_rows();
        idx.par_sort_unstable_by(|&a, &b| {
            self.title_sort_key(a as usize)
                .cmp(self.title_sort_key(b as usize))
        });
        idx
    }

    pub fn sort_year(&self) -> Vec<u32> {
        let mut idx = self.live_rows();
        idx.par_sort_unstable_by_key(|&i| self.year[i as usize]);
        idx
    }

    /// Sort a view (any subset of rows, in any order) by one key. Ties
    /// fall back to the canonical artist, album, track order so equal keys
    /// stay browsable; descending reverses the key alone, not the
    /// tie-break.
    pub fn sort_view(&self, view: &[u32], key: SortKey, descending: bool) -> Vec<u32> {
        match key {
            SortKey::Title => self.order_view(view, descending, |i| self.title_sort_key(i)),
            // The sort columns order by their own value and nothing else.
            // A row with no sort title has no place among the ones that
            // have them, so it takes the empty group's key and the string
            // comparison never runs for it.
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
            // unrated track goes too.
            SortKey::TrackGain => self.order_view(view, descending, |i| self.gain_key(i, false)),
            SortKey::AlbumGain => self.order_view(view, descending, |i| self.gain_key(i, true)),
            // Packed centi-bpm sorts as-is, and NO_BPM being zero puts the
            // tracks with no tempo first ascending, where the untagged
            // gains and the unrated tracks go too.
            SortKey::Bpm => self.order_view(view, descending, |i| self.bpm[i]),
        }
    }

    /// One row's leveling gain in dB: the mode's own figure, the other as
    /// the fallback, None for a file with neither. `album_first` is the
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
    /// except titles, which compare their lowered strings directly, since a
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
        // A view is normally already live: it came out of the search or
        // the canonical order, both of which skip tombstones. The filter is
        // for the one that didn't, a view a panel built before a patch and
        // sorted after it, which would otherwise put a retired row on screen.
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

    /// Fold a shard of freshly read rows into the projection in place,
    /// rather than rebuilding the whole thing because one file's tags
    /// changed. The row an id already had is tombstoned and the fresh one
    /// appended, so the columns only ever grow and every index into them
    /// that a panel is holding stays pointing at what it pointed at.
    ///
    /// `index` is the caller's id to row map as it stands before the patch;
    /// `plays` and `spans` carry the two columns a row can't read off its
    /// own `tracks` row. What comes back says which rows appeared and which
    /// retired, so the caller can fix that map and the canonical order
    /// through [`Projection::patch_order`] instead of rebuilding them.
    ///
    /// None means the patch was refused and the caller has to fall back to
    /// a full rebuild: either the shard lost rows to the arena ceiling, or
    /// the projection has no room left for the text.
    ///
    /// Two things a patch deliberately doesn't reproduce, both of which the
    /// next rebuild settles: a value already in a symbol table keeps the
    /// display casing the last full build voted for, and a symbol whose
    /// every row is now dead stays in the table. Both cost a stale
    /// suggestion at worst, and the counts behind that vote are gone once a
    /// table finalizes.
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
        // The first row in the library to carry a sort title turns the
        // arenas on, which means catching every row already here up with an
        // empty entry apiece. The merge makes the same call at the end of a
        // full build; this is that call arriving late.
        if self.title_sort.is_none() && !shard.title_sort.is_blank() {
            let mut display = Arena::default();
            let mut lower = Arena::default();
            for _ in 0..self.len() {
                display.push("");
                lower.push("");
            }
            self.title_sort = Some((display, lower));
        }
        // Room for the whole shard's text checked once, up front: a row
        // refused halfway through would leave the columns out of step, and
        // there is nowhere to put it back.
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
        // Codecs and folders intern exactly whatever the case setting is,
        // the way the full build does.
        let c = absorb_shard(&mut self.codecs, &mut sym.codecs, false, &shard.codecs);
        let f = absorb_shard(&mut self.folders, &mut sym.folders, false, &shard.folders);
        self.sym_index = Some(sym);
        let tables = [&a, &aa, &b, &g, &c, &f];
        let symbols_moved = tables.iter().any(|t| t.moved);
        // Every table, not just the two the canonical order keys on: a
        // caller is free to keep an order sorted on any of them, and the
        // flag is about whether an order can be patched at all.
        let reordered = tables.iter().any(|t| t.reordered);
        let (map_a, map_aa, map_b, map_g, map_c, map_f) =
            (a.map, aa.map, b.map, g.map, c.map, f.map);

        let mut patch = Patch {
            reordered,
            ..Patch::default()
        };
        for i in 0..shard.db_id.len() {
            let id = shard.db_id[i];
            let row = self.db_id.len() as u32;
            self.db_id.push(id);
            // The shard lowered its own text, so both arenas take the bytes
            // as they are rather than folding them a second time.
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
            self.dead.push(false);
            if let Some(&span) = spans.get(&id) {
                self.spans.insert(row, span);
            }
            // The row this id used to sit on retires now, span and all.
            if let Some(&old) = index.get(&id) {
                if !self.dead[old as usize] {
                    self.dead[old as usize] = true;
                    self.dead_rows += 1;
                    self.spans.remove(&old);
                    patch.dropped.push(old);
                }
            }
            patch.added.push(row);
        }
        self.invalidate(symbols_moved);
        Some(patch)
    }

    /// Tombstone the rows for these ids: their files are gone from the
    /// database, so they go out of every answer the projection gives
    /// without the columns moving under anyone.
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
            // No symbol moved: a removal only ever leaves a table holding
            // more than the library uses, which the next rebuild sweeps.
            self.invalidate(false);
        }
        patch
    }

    /// The canonical order with a patch folded in: the retired rows out,
    /// the fresh ones each at the position the sort would have put them.
    /// One merge pass rather than an insert apiece, so a renamed album's
    /// worth of rows costs the same walk as a single file's.
    ///
    /// Only valid while `order` is still sorted under the current ranks,
    /// which is what [`Patch::reordered`] answers: every search below
    /// assumes it can binary-search `order` by key, and a patch that moved
    /// a value the projection already knew breaks that assumption for rows
    /// this patch never touched. A caller holding a reordered patch owes
    /// its order a [`Projection::sort_canonical`] instead.
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
        // Where each row of the patch belongs, found by binary search
        // rather than by walking the order and asking every row for its
        // key. At a million rows that walk is two random reads into the
        // rank tables a million times over, which costs more than the
        // whole rest of a patch; this way the order is only ever copied.
        // Where a row sits in the order now: binary search to the run of
        // rows sharing its key, then walk that run for the row itself. Ties
        // are a handful of tracks; the linear fallback is there because a
        // dropped row left behind in the order is a tombstone on screen,
        // which is worth a scan to never do.
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

        let mut fresh = patch.added.clone();
        fresh.sort_unstable_by_key(|&row| key(row));
        let mut events: Vec<(usize, Option<u32>)> =
            Vec::with_capacity(fresh.len() + patch.dropped.len());
        for &row in &fresh {
            let here = key(row);
            events.push((order.partition_point(|&other| key(other) < here), Some(row)));
        }
        for &row in &patch.dropped {
            if let Some(at) = position_of(row) {
                events.push((at, None));
            }
        }
        // An insert and a removal at the same index: the insert goes first,
        // then the removal skips the row that was there. Stable, so two
        // inserts landing between the same pair of rows keep the order
        // their keys just put them in.
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
                // The row at `at` is the one leaving, so the copy resumes
                // past it.
                None => cursor = at + 1,
            }
        }
        out.extend_from_slice(&order[cursor.min(order.len())..]);
        out
    }

    /// Drop the memoized tables a patch has just falsified. The projection
    /// was immutable when these were written down, which is what made them
    /// safe to keep; a patch is the moment that stops being true, so they
    /// go and the next caller pays for the rebuild it needs.
    ///
    /// The distinct artist and album lists and the genre terms go on every
    /// patch: they name a first-seen row, and rows have come and gone. The
    /// symbol ranks only go when a symbol table actually moved, since a
    /// rank is a fact about the table and not about the rows.
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
            + self.dead.capacity()
            + self.sym_index.as_ref().map_or(0, |i| i.heap_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{listens, TrackRow};

    fn row(path: &str, album: &str, disc_no: u16, track_no: u16) -> TrackRow {
        TrackRow {
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

    /// A track with a track number, so a set of rows that would otherwise
    /// tie on the canonical order has something to break the tie with and
    /// an ordering assertion is deterministic.
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
            // The day suffix is optional, and a quoted value comes through
            // the tokenizer the same way a quoted artist does.
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

    /// The numeric columns the projection holds, compared the way the
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
        // A hyphen negates the comparison: everything under four stars,
        // everything played, everything older than the window.
        assert_eq!(titles("-rating:>=4"), ["Plain"]);
        assert_eq!(titles("-plays:0"), ["Liked"]);
        assert_eq!(titles("-added:<90d"), ["Liked", "Plain"]);
        assert_eq!(titles("-rating:0"), ["Loved", "Liked"]);
    }

    /// The hyphen forms parse: a bare `-field` asks for the field being
    /// absent, `-field:value` negates the pin, and anything else behind a
    /// hyphen stays literal text.
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
        // A quoted value negates the way the positive form takes it.
        let terms = parse_query(r#"-artist:"Daft Punk""#);
        assert_eq!(
            (terms[0].field, terms[0].needle.as_str(), terms[0].mode),
            (Some(QueryField::Artist), "daft punk", TermMode::Exclude)
        );
        // A numeric pin keeps its comparison behind the hyphen.
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

        // An unknown name, a field with no absent value, a lone hyphen,
        // and a plain word all stay free text for the literal characters.
        for query in ["-foo", "-folder", "-codec", "-added", "-", "-stronger"] {
            let terms = parse_query(query);
            assert_eq!(terms.len(), 1, "{query} is one term");
            assert_eq!(
                (terms[0].field, terms[0].needle.as_str(), terms[0].mode),
                (None, query, TermMode::Match),
                "{query} stays free text"
            );
        }
        // The colon forms fall back the same way: an unknown field, and a
        // numeric pin with nothing numeric behind it.
        for query in ["-foo:bar", "-rating:great"] {
            let terms = parse_query(query);
            assert_eq!(
                (terms[0].field, terms[0].needle.as_str(), terms[0].mode),
                (None, query, TermMode::Match),
                "{query} stays free text"
            );
        }
    }

    /// Three rows for the hyphen terms: one tagged and played all the way
    /// through, one with nothing but a title, and one whose title is the
    /// field that's missing.
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
        // One listen on the first row, so `-plays` has a row to leave out.
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

    /// A bare `-field` finds the rows whose value for it is missing: the
    /// empty string on the name fields, the zero year, unrated, unplayed.
    #[test]
    fn absence_terms_find_the_missing_values() {
        let p = hyphen_rows();
        for query in ["-year", "-genre", "-artist", "-albumartist", "-album"] {
            assert_eq!(titles_for(&p, query), ["Bare"], "{query}");
        }
        assert_eq!(titles_for(&p, "-rating"), ["Bare"], "unrated is absent");
        assert_eq!(titles_for(&p, "-plays"), ["Bare", ""], "never played");
        assert_eq!(titles_for(&p, "-title"), [""]);
        // Absences AND with everything else.
        assert_eq!(titles_for(&p, "-year -rating"), ["Bare"]);
        // The fields with no absent value are free text, and nothing here
        // has those characters in a name.
        for query in ["-folder", "-codec", "-added"] {
            assert!(titles_for(&p, query).is_empty(), "{query}");
        }
    }

    /// `-field:value` is the positive term inverted, and a tombstoned row
    /// stays out either way: negation flips the term, not the row's
    /// existence.
    #[test]
    fn exclusion_terms_invert_their_field() {
        let mut p = hyphen_rows();
        assert_eq!(titles_for(&p, "genre:rock"), [""]);
        assert_eq!(titles_for(&p, "-genre:rock"), ["Tagged", "Bare"]);
        assert_eq!(titles_for(&p, "-artist:a"), ["Bare", ""]);
        assert_eq!(titles_for(&p, "-title:tagged"), ["Bare", ""]);
        // The year needle matches on the digits, so its negation does too.
        assert_eq!(titles_for(&p, "-year:19"), ["Tagged", "Bare"]);
        assert_eq!(titles_for(&p, "-rating:>=4"), ["Bare", ""]);
        // Terms still AND, positive beside negative.
        assert_eq!(titles_for(&p, "-genre:rock -rating:>=4"), ["Bare"]);

        // Tombstone the bare row the way a rescan does.
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

    /// The per-track matcher the queue and playlists filter with agrees
    /// with the row scan on the hyphen terms, so a negated query narrows a
    /// panel's own list the way it narrows the library.
    #[test]
    fn hyphen_terms_agree_between_the_matchers() {
        let p = hyphen_rows();
        // The same three rows, named the way a panel names its own.
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
                        },
                    )
                })
                .map(|r| r.0.to_string())
                .collect();
            assert_eq!(titles_for(&p, query), matched, "{query}");
        }
        // The projection-only columns are the documented exception: the
        // row scan reads them, a panel's own rows can't, and a negated
        // miss stays a miss rather than flipping into a hit.
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
        };
        assert!(!track_matches(&parse_query("-rating:>=4"), &queue_row));
        assert!(!track_matches(&parse_query("-rating"), &queue_row));
        assert!(!track_matches(&parse_query("-plays"), &queue_row));
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
    /// which the queue and the playlists tree share. An off-catalog row
    /// has no db id, so an id pin leaves it out rather than folding
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
    /// files. A bare word never matches the folder, so the path text stays
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
        // A bare word never matches the folder path.
        assert!(titles_for(&p, "other").is_empty());
    }

    /// The stream numbers come through the store round trip intact and sort
    /// as numbers, which is what the kHz and Bits columns browse on. The
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

    /// A scratch library built from rows, both load paths available.
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

    /// A row whose names carry Latin sort forms is found by typing them,
    /// free and pinned, in the track search and in the grouped artist and
    /// album searches. The grouped ones match off their own copy of the
    /// name rather than through `hit`, so quick-play would otherwise miss
    /// what the library panel finds.
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
            // The displayed name still matches, and a pin still excludes.
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

            // The row's own sort name reads back off the view.
            let view = p.resolve(p.search("yonezu")[0]);
            assert_eq!(view.artist_sort, "Yonezu, Kenshi");
            assert_eq!(view.title_sort, "Lemon");
            assert_eq!(view.album_sort, "Bootleg");
        }
    }

    /// The change a user feels: a CJK-named artist files under its Latin
    /// initial instead of its own bucket at the end of the rail. The pick
    /// is per symbol, so the artists with no sort tag keep ordering by
    /// their display name around it.
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
        // 米津玄師 lands between Beta and Zebra, on the Y its sort name
        // starts with, rather than after every Latin name.
        assert_eq!(
            titles(p.sort_view(&view, SortKey::Artist, false)),
            ["Three", "Zoo", "Two", "One", "ゆめうつつ"]
        );
        // A sort title moves a row the same way: ゆめうつつ sorts on
        // "Yumeutsutsu" and lands ahead of Zoo, where without the tag it
        // would trail every Latin title.
        assert_eq!(
            titles(p.sort_title()),
            ["One", "Three", "Two", "ゆめうつつ", "Zoo"]
        );
    }

    /// The sort columns are the other half of that: they show the tag and
    /// nothing else, so they order by the tag and nothing else. The rows
    /// carrying one lead, the rows without sit at the bottom whichever way
    /// the column points, and the base Artist sort is untouched by any of
    /// it.
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
        // The two tagged rows A to Z, then the untagged pair in canonical
        // track order.
        assert_eq!(
            titles(p.sort_view(&view, SortKey::ArtistSort, false)),
            ["Four", "Two", "One", "Three"]
        );
        // Descending turns the tagged pair around and leaves the tail
        // exactly where it was.
        assert_eq!(
            titles(p.sort_view(&view, SortKey::ArtistSort, true)),
            ["Two", "Four", "One", "Three"]
        );
        // The Artist column keeps its own meaning: every row files under
        // its sort name where it has one and its display name otherwise.
        assert_eq!(
            titles(p.sort_view(&view, SortKey::Artist, false)),
            ["Three", "Four", "Two", "One"]
        );
    }

    /// The same rule on the one key that reads a per-row arena rather than
    /// a symbol table, including the case that arena isn't there at all.
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

        // No row in the library carries a sort title, so the projection
        // holds no arena for them and every row is an empty: the column
        // hands back the canonical order, both ways round.
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

    /// The whole point of the folded key: a name typed without its accents
    /// still finds the row that has them, everywhere a term can land.
    /// Titles come off the arena, names off the symbol tables, and both
    /// sides of the comparison go through the same fold, so the accented
    /// spelling keeps working too.
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
            // A symbol table hit, free and pinned.
            assert_eq!(titles_for(&p, "beyonce"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "artist:beyonce"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "albumartist:beyonce"), ["Déjà Vu"]);
            // The accented spelling still works, and so does the casing
            // the search always folded away.
            assert_eq!(titles_for(&p, "Beyoncé"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "BEYONCE"), ["Déjà Vu"]);
            // A title hit, which runs over the arena rather than a table.
            assert_eq!(titles_for(&p, "deja vu"), ["Déjà Vu"]);
            assert_eq!(titles_for(&p, "title:deja"), ["Déjà Vu"]);
            // The sharp s spells itself out on both sides.
            assert_eq!(titles_for(&p, "strasse"), ["Sonne"]);
            assert_eq!(titles_for(&p, "album:strasse"), ["Sonne"]);
            assert_eq!(titles_for(&p, "Straße"), ["Sonne"]);
            // Folding widens what a needle reaches, never what it means:
            // a name that shares no letters still misses.
            assert!(titles_for(&p, "artist:rammstein").len() == 1);
            assert!(titles_for(&p, "artist:beyonce").len() == 1);

            // The grouped searches quick-play runs match off their own
            // copies of the name, so they need the fold too.
            let artists = p.search_artists("beyonce");
            assert_eq!(artists.len(), 1);
            assert_eq!(
                p.album_artists.strings[artists[0].album_artist as usize],
                "Beyoncé"
            );
        }
    }

    /// The per-row matcher the queue and the playlists tree filter with
    /// folds the same way the catalog does. It has no prepared column to
    /// read, so it folds each field per call; the answers still have to
    /// agree with `search`.
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
        };
        assert!(track_matches(&parse_query("beyonce"), &fields));
        assert!(track_matches(&parse_query("artist:beyonce"), &fields));
        assert!(track_matches(&parse_query("deja"), &fields));
        assert!(track_matches(&parse_query("Beyoncé"), &fields));
        assert!(!track_matches(&parse_query("beyonc3"), &fields));
    }

    /// An exact filter pick still narrows to the accented value. The pick
    /// carries the display casing off the symbol table and `value_eq`
    /// compares it against the raw row, so neither side is the folded key,
    /// and folding the search key has to leave that alone: two values
    /// spelled with and without the accent stay two values to pick between.
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
            // Both are one needle away, which is the half that changed.
            assert_eq!(titles_for(&p, "artist:beyonce"), ["One", "Two"]);
        }
    }

    /// Ordering keys off the same folded string, so an accented name files
    /// under its base letter instead of trailing every unaccented one. The
    /// side effect Andrew asked for by name: Émilie sits between Dana and
    /// Frank rather than after Zebra.
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

    /// The zero-cost claim: a library with none of the tags carries no
    /// per-row arenas and no per-symbol vectors, so nothing but a branch
    /// changes for the libraries that will never have them.
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
            // The accessors still answer, falling through to the display.
            assert_eq!(p.artists.sort_name(0), "");
            assert_eq!(p.artists.sort_key(0), "alpha");
            assert_eq!(p.title_sort(0), "");
            assert_eq!(p.title_sort_key(0), "one");
        }
    }

    /// The whole reason [`crate::artist_meta`] exists: an artist whose
    /// files carry no sort tag, which is nearly all of them, still files
    /// and searches under the name the library looked up for it. Both
    /// artist tables take it off the one row, since a lookup answers for
    /// the value rather than for the column it appeared in.
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
            // The looked-up name is what ordering keys off, so the CJK
            // artist files under S rather than after every Latin name.
            let view: Vec<u32> = (0..p.len() as u32).collect();
            let order: Vec<&str> = p
                .sort_view(&view, SortKey::Artist, false)
                .iter()
                .map(|&i| p.title.get(i as usize))
                .collect();
            assert_eq!(order, ["One", "Two"]);
            // The fill materialises the columns a bare library doesn't
            // pay for, and only for the symbol that had an answer.
            assert_eq!(
                p.resolve(p.search("sakiyama")[0]).artist_sort,
                "Sakiyama, Soushi"
            );
            assert_eq!(p.resolve(p.search("zebra")[0]).artist_sort, "");
            // Nothing was laid over the album table: MusicBrainz has no
            // album sort name to look one up with.
            assert!(p.albums.sort.is_empty());
        }
    }

    /// A tag beats a lookup. The sort name in the file is what this
    /// library's owner wrote down, so a table row disagreeing with it
    /// loses, whichever of the two landed first.
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

    /// [`crate::album_meta`]'s half: an album title nobody could look up
    /// takes the sort name the romanization pass wrote, and the album
    /// table alone takes it.
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
            // It files under U, after the Latin album rather than after
            // every Latin album.
            let view: Vec<u32> = (0..p.len() as u32).collect();
            let order: Vec<&str> = p
                .sort_view(&view, SortKey::Album, false)
                .iter()
                .map(|&i| p.title.get(i as usize))
                .collect();
            assert_eq!(order, ["Two", "One"]);
            // The artist tables were left alone: the row is about an album.
            assert!(p.artists.sort.is_empty());
        }
    }

    /// [`crate::track_meta`]'s half, and the one that has to rebuild
    /// arenas rather than fill a symbol: a romanized title is searchable
    /// and sortable by its romaji, and the library that started with no
    /// sort titles at all grows the columns to hold them.
    #[test]
    fn a_track_meta_row_fills_a_row_the_files_left_bare() {
        let (db, conn) = sorted_library(
            "sort-track-meta",
            &[
                track("/m/1.mp3", "レモン", "Zebra", 2018),
                track("/m/2.mp3", "Aardvark", "Zebra", 2018),
            ],
        );
        // The library carries no sort titles, so the arenas collapsed.
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
            // A under R: without the row the kana would sort past every
            // Latin title instead.
            assert_eq!(order, ["Aardvark", "レモン"]);
            // The row that has no sort title of its own still keys off its
            // display title.
            let latin = (0..p.len())
                .find(|&row| p.title.get(row) == "Aardvark")
                .unwrap();
            assert_eq!(p.title_sort(latin), "");
            assert_eq!(p.title_sort_key(latin), "aardvark");
        }
    }

    /// A file's own sort title beats the table's, the same rule the artist
    /// side has.
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

    /// The ReplayGain figures load into the projection, come back as the
    /// dB the file holds, and sort by whichever one the leveling mode
    /// reads. A file tagged only one way is read by the other mode too,
    /// the same fallback the engine levels by, and one with neither
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
    /// minute the row holds, along with which source filled it so a display
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
    /// tempo for ahead of them: the packed zero is the floor, where an
    /// untagged gain and an unrated track go too.
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
        // A bare word only matches the text fields.
        assert_eq!(titles_for(&p, "flac"), ["Flac Tribute"]);
    }

    /// A folder pick covers its subtree: the folder itself and every
    /// descendant, bounded at a separator so a sibling sharing the prefix
    /// stays out. One value scopes a whole branch, which keeps the folder
    /// tree's click cheap.
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
    /// symbol whose display is the casing most rows use, picks match
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

        // A negated name term keeps the heads it doesn't name.
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
        // An exclusion or an absence on a field no head carries excludes
        // every head, the same as the positive form does.
        assert!(p.search_artists("-title:montezuma").is_empty());
        assert!(p.search_albums("-title:montezuma").is_empty());
        assert!(p.search_artists("-genre").is_empty());
        assert!(p.search_albums("-year").is_empty());
    }

    /// The structured filter matches whole values only, so "Air" leaves
    /// "Airborne" out where the text search would take both. Values OR
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
    /// selection-following view uses. It ANDs with the field picks, and an
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
    /// queue) has no db id, so an id pin leaves it out, and leaves it out
    /// on its own rather than lumped in with every other id-less row. Field
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
    /// second call returns the same thing: the OnceLock doesn't corrupt
    /// state between calls.
    #[test]
    fn search_cache_is_stable_across_calls() {
        fn full(path: &str, album_artist: &str, album: &str) -> TrackRow {
            TrackRow {
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

    /// A projection with the two indexes the catalog keeps beside it, so a
    /// test can drive a sync the way `Library::apply_patch` does: patch the
    /// projection, fold the patch into the canonical order, fix the id map.
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

        /// One sync: re-read `changed`, tombstone `gone`.
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
            // The same fork `Library::install_patch` takes: a patch that
            // moved an existing value has invalidated the order outright.
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

    /// Everything one row resolves to, the sort keys ordering runs on
    /// included, as one comparable line. This is what "a patched projection
    /// equals a rebuilt one" has to mean: not the same row indexes, which a
    /// tombstone-and-append projection can never have, but the same answers
    /// in the same order.
    fn row_snapshot(p: &Projection, row: u32) -> String {
        let i = row as usize;
        let v = p.resolve(row);
        format!(
            "{id} {title:?} {artist:?} {album_artist:?} {album:?} {genre:?} {year} {disc}/{track} \
             {duration} {codec:?} {bitrate} {rate}/{depth} r{rating} p{plays} a{added} \
             {track_gain:?}/{album_gain:?} {bpm:?} {source:?} {folder:?} sub{sub} \
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
            source = v.bpm_source,
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

    /// Every live row in canonical order, snapshotted.
    fn snapshot_all(p: &Projection, order: &[u32]) -> Vec<String> {
        order.iter().map(|&row| row_snapshot(p, row)).collect()
    }

    /// A full row for the patch tests: enough columns filled that the
    /// snapshot comparison has something to disagree about.
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

        // The tag editor wrote a new title; the row keeps its id.
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
        // The row count a person sees is unchanged; the retired row is
        // still in the columns and counted as dead weight.
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
        // An empty query is still a search, and it stops handing the row out too.
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

        // One before every existing row, one after, one in the middle of an
        // album's own run: the three places a merge can get wrong.
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
        // A library with sort names on some values and none on others, two
        // artists' worth of albums, so the comparison has symbol tables and
        // sort keys to disagree on rather than just strings.
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

        // A play, so the plays column has something to carry across a patch
        // rather than being zero everywhere by default.
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

        // The mix: two titles rewritten, one of them picking up a sort title
        // no row in the library had before, a new album by a new artist
        // added, and two rows deleted.
        let mut edited = full_row("/m/A/1.flac", "Alderwood", "Aviary", "First", 1, 1);
        edited.title_sort = "Alderwood, The".into();
        let mut moved = full_row("/m/C/1.flac", "Cedarwood", "Cormorant", "Third", 1, 1);
        moved.rating = 100;
        moved.year = 1994;
        // Sort names the library looked up rather than read off a file, on
        // all three of the tables that carry them: an artist the patch is
        // about to meet for the first time, an album the same, and one
        // artist and one title the projection already holds. The last two
        // are the interesting ones, since a patch has to lay a sort name
        // over a value it already knew, not just over an arriving one.
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

        // The load-bearing comparison: the same rows, resolving the same
        // way, in the same order, as a projection built from scratch off
        // the same database.
        let fresh = Projection::load_serial(&conn, false).unwrap();
        let fresh_order = fresh.sort_canonical();
        assert_eq!(live.projection.live_len(), fresh.len());
        assert_eq!(live.order, live.projection.sort_canonical());
        assert_eq!(
            snapshot_all(&live.projection, &live.order),
            snapshot_all(&fresh, &fresh_order)
        );
        // And the derived answers, which run off caches a patch has to
        // have invalidated.
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

        // A second round on top of the first, so the patch path is exercised
        // against a projection that is already patched rather than freshly
        // built.
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

        // Compaction is the full rebuild the catalog falls back to once the
        // dead weight passes its ceiling: the tombstones go, and what it
        // hands back is the projection the patches had been standing in for.
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

        // The third casing folds onto the symbol the other two share rather
        // than opening a fourth artist.
        assert_eq!(live.projection.artists.strings.len(), symbols);
        assert_eq!(titles_for(&live.projection, "artist:aviary").len(), 3);
    }

    /// Symbol identity is still a casing question after the search key
    /// started folding accents. A patch bringing another row by the same
    /// accented artist finds the symbol the table already has, and one
    /// bringing the unaccented spelling opens its own, which is what a
    /// full rebuild of the same library would do.
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

        // One needle reaches both spellings all the same.
        assert_eq!(titles_for(&live.projection, "artist:beyonce").len(), 3);
    }

    #[test]
    fn the_arena_refuses_an_offset_it_cannot_hold() {
        // The check the pushes run, driven with a fake length so the refusal
        // is testable without four gigabytes of titles on hand.
        assert_eq!(Arena::checked_end(0, 5), Some(5));
        assert_eq!(Arena::checked_end(u32::MAX as usize - 5, 5), Some(u32::MAX));
        assert_eq!(Arena::checked_end(u32::MAX as usize - 4, 5), None);
        assert_eq!(Arena::checked_end(usize::MAX, 1), None);

        // And a real arena keeps its word: a refused push leaves it exactly
        // as it was, so the row can be dropped whole.
        let mut arena = Arena::default();
        assert!(arena.push("kept"));
        assert!(!arena.fits(u32::MAX as usize));
        assert_eq!(arena.get(0), "kept");
        arena.pop();
        assert_eq!(arena.bytes_len(), 0);
    }
    #[test]
    fn the_order_survives_a_pile_of_ties() {
        // Thirty rows that all key the same: same album artist, same album,
        // same disc, same track number. The canonical key can't tell them
        // apart, so every insert and removal lands somewhere inside one long
        // run, which is where a position-based order patch would go wrong.
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

        // Ties mean no single right sequence, so the check is the two things
        // that have to hold whatever order the run comes out in: the order
        // holds every live row exactly once and nothing dead, and the ids in
        // it are the ids the database has.
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

    /// A value the projection already holds takes a sort name, which files
    /// it somewhere else and takes every row using it along. The order the
    /// caller was keeping can't absorb that, so the patch says so and the
    /// caller sorts again; before the flag existed, `patch_order` searched
    /// its own input under ranks that input was no longer sorted by and put
    /// the moved rows wherever the search happened to land.
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

        // One of Bellwether's three rows comes back carrying a sort name
        // that files the artist last instead of second. The other two never
        // arrive, so nothing but the ranks says they moved.
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

    /// The flag itself: a patch that only appends symbols leaves the order
    /// patchable, one that moves a symbol the table already had does not.
    /// Worth pinning apart, because taking the slow path always would hide
    /// the bug by paying a full sort on every watch event.
    #[test]
    fn only_an_adopted_sort_name_marks_a_patch_reordered() {
        let seed = vec![full_row("/m/A/1.flac", "Alder", "Aviary", "First", 1, 1)];
        let (_db, mut conn) = library_for("patch-reordered-flag", &seed);
        let mut live = Live::load(&conn, false);

        // A brand new artist: appended, so no row that was already in the
        // order changed places with another.
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

        // The same artist again, this time carrying a sort name it had none
        // of before.
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

    /// Two symbols whose sort keys are equal keep the order they were in
    /// when the table grows under them. The rank sort is unstable and runs
    /// across threads, so without a tie-break on the symbol itself the two
    /// land wherever the partitioning left them and rows nothing changed
    /// about swap places on the next patch.
    #[test]
    fn tied_sort_keys_hold_their_places_when_the_table_grows() {
        for n in [2usize, 50, 500, 5000, 50000] {
            let mut table = SymTable {
                strings: Vec::new(),
                lower: Vec::new(),
                sort: Vec::new(),
                sort_lower: Vec::new(),
            };
            // Every value spelled two ways, in a table that doesn't fold: two
            // symbols apiece, one sort key apiece.
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

    /// The three sort-name tables reach a patched row the same way they
    /// reach a freshly loaded one: a title romanized before the row came
    /// back, and an artist the library had already looked up before any row
    /// of theirs was in the library at all.
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

        // The row comes back through a patch, unchanged on disk and still
        // carrying no sort title of its own.
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

        // And an artist and album whose first row arrives now: the symbols
        // are pushed by the patch, so the sort names have to reach them on
        // the way in.
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

    /// A caller naming the same id twice gets one row back, not two. The
    /// watch path collects ids from both sides of a reindex and hands over
    /// whatever it found, so the duplicate is ordinary; two live rows for
    /// one track would not be.
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
