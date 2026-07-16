//! The read path of ADR 5 at scale. Columnar: artist, album, and genre are
//! interned to u32 symbols, titles live in one contiguous byte arena with an
//! offset table (never ten million heap Strings), and every browse order is a
//! precomputed Vec<u32> of row indices over integer ranks. Search per ADR 6 is
//! substring: the interned tables are scanned whole (they are a hundredth the
//! row count), only titles need the full-row scan, and that scan splits across
//! cores in fixed chunks.

use std::collections::HashMap;
use std::path::Path;

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
        for c in s.chars() {
            self.bytes.extend(c.to_lowercase());
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
    track_no: Vec<u16>,
    duration_ms: Vec<u32>,
    codec: Vec<u32>,
    bitrate_kbps: Vec<u16>,
    artists: Interner,
    album_artists: Interner,
    albums: Interner,
    genres: Interner,
    codecs: Interner,
}

impl Builder {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        id: i64,
        title: &str,
        artist: &str,
        album_artist: &str,
        album: &str,
        genre: &str,
        year: u16,
        track_no: u16,
        duration_ms: u32,
        codec: &str,
        bitrate_kbps: u16,
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
        self.track_no.push(track_no);
        self.duration_ms.push(duration_ms);
        self.codec.push(self.codecs.intern(codec));
        self.bitrate_kbps.push(bitrate_kbps);
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
    pub track_no: Vec<u16>,
    pub duration_ms: Vec<u32>,
    pub codec: Vec<u32>,
    pub bitrate_kbps: Vec<u16>,
    pub artists: SymTable,
    pub album_artists: SymTable,
    pub albums: SymTable,
    pub genres: SymTable,
    pub codecs: SymTable,
}

pub struct RowView<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album_artist: &'a str,
    pub album: &'a str,
    pub genre: &'a str,
    pub year: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    pub codec: &'a str,
    pub bitrate_kbps: u16,
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
            |id, title, artist, album_artist, album, genre, year, tn, dur, codec, kbps| {
                b.push(
                    id,
                    title,
                    artist,
                    album_artist,
                    album,
                    genre,
                    year,
                    tn,
                    dur,
                    codec,
                    kbps,
                );
            },
        )?;
        Ok(Self::merge(vec![b]))
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
                             title,
                             artist,
                             album_artist,
                             album,
                             genre,
                             year,
                             tn,
                             dur,
                             codec,
                             kbps| {
                                b.push(
                                    id,
                                    title,
                                    artist,
                                    album_artist,
                                    album,
                                    genre,
                                    year,
                                    tn,
                                    dur,
                                    codec,
                                    kbps,
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
        Ok(Self::merge(shards))
    }

    fn merge(shards: Vec<Builder>) -> Self {
        let mut artists = Interner::default();
        let mut album_artists = Interner::default();
        let mut albums = Interner::default();
        let mut genres = Interner::default();
        let mut codecs = Interner::default();
        let total: usize = shards.iter().map(|s| s.db_id.len()).sum();

        let mut out = Builder::default();
        out.db_id.reserve(total);
        out.artist.reserve(total);
        out.album_artist.reserve(total);
        out.album.reserve(total);
        out.genre.reserve(total);
        out.year.reserve(total);
        out.track_no.reserve(total);
        out.duration_ms.reserve(total);
        out.codec.reserve(total);
        out.bitrate_kbps.reserve(total);

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
            out.track_no.extend_from_slice(&shard.track_no);
            out.duration_ms.extend_from_slice(&shard.duration_ms);
            out.codec
                .extend(shard.codec.iter().map(|&s| map_c[s as usize]));
            out.bitrate_kbps.extend_from_slice(&shard.bitrate_kbps);
        }

        Projection {
            db_id: out.db_id,
            title: out.title,
            title_lower: out.title_lower,
            artist: out.artist,
            album_artist: out.album_artist,
            album: out.album,
            genre: out.genre,
            year: out.year,
            track_no: out.track_no,
            duration_ms: out.duration_ms,
            codec: out.codec,
            bitrate_kbps: out.bitrate_kbps,
            artists: SymTable::from(artists),
            album_artists: SymTable::from(album_artists),
            albums: SymTable::from(albums),
            genres: SymTable::from(genres),
            codecs: SymTable::from(codecs),
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
            track_no: self.track_no[i],
            duration_ms: self.duration_ms[i],
            codec: &self.codecs.strings[self.codec[i] as usize],
            bitrate_kbps: self.bitrate_kbps[i],
        }
    }

    /// Case-folded substring over title, artist, album artist, album, and
    /// genre. Symbol tables are matched whole first; the row scan then only
    /// does per-title memmem plus four table lookups.
    pub fn search(&self, query: &str) -> Vec<u32> {
        let q = query.to_lowercase();
        let hit = |table: &SymTable| -> Vec<bool> {
            table.lower.par_iter().map(|s| s.contains(&q)).collect()
        };
        let ((a_hit, aa_hit), (b_hit, g_hit)) = rayon::join(
            || rayon::join(|| hit(&self.artists), || hit(&self.album_artists)),
            || rayon::join(|| hit(&self.albums), || hit(&self.genres)),
        );

        let finder = memmem::Finder::new(q.as_bytes());
        self.scan_rows(|i| {
            a_hit[self.artist[i] as usize]
                || aa_hit[self.album_artist[i] as usize]
                || b_hit[self.album[i] as usize]
                || g_hit[self.genre[i] as usize]
                || finder.find(self.title_lower.get(i).as_bytes()).is_some()
        })
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

    /// The canonical browse order: album artist, album, track number. The
    /// album artist keys it so an album's tracks stay one run under its
    /// credited artist, per-track guests and all.
    pub fn sort_canonical(&self) -> Vec<u32> {
        let a_rank = Self::ranks(&self.album_artists);
        let b_rank = Self::ranks(&self.albums);
        let mut idx: Vec<u32> = (0..self.len() as u32).collect();
        idx.par_sort_unstable_by_key(|&i| {
            let i = i as usize;
            (
                a_rank[self.album_artist[i] as usize],
                b_rank[self.album[i] as usize],
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
                let rank = Self::ranks(&self.artists);
                self.order_view(view, descending, move |i| rank[self.artist[i] as usize])
            }
            SortKey::AlbumArtist => {
                let rank = Self::ranks(&self.album_artists);
                self.order_view(view, descending, move |i| {
                    rank[self.album_artist[i] as usize]
                })
            }
            SortKey::Album => {
                let rank = Self::ranks(&self.albums);
                self.order_view(view, descending, move |i| rank[self.album[i] as usize])
            }
            SortKey::Genre => {
                let rank = Self::ranks(&self.genres);
                self.order_view(view, descending, move |i| rank[self.genre[i] as usize])
            }
            SortKey::Year => self.order_view(view, descending, |i| self.year[i]),
            SortKey::TrackNo => self.order_view(view, descending, |i| self.track_no[i]),
            SortKey::Duration => self.order_view(view, descending, |i| self.duration_ms[i]),
            SortKey::Codec => {
                let rank = Self::ranks(&self.codecs);
                self.order_view(view, descending, move |i| rank[self.codec[i] as usize])
            }
            SortKey::Bitrate => self.order_view(view, descending, |i| self.bitrate_kbps[i]),
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
        let a_rank = Self::ranks(&self.album_artists);
        let b_rank = Self::ranks(&self.albums);
        let canonical = |i: usize| {
            (
                a_rank[self.album_artist[i] as usize],
                b_rank[self.album[i] as usize],
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
        self.db_id.capacity() * 8
            + self.title.heap_bytes()
            + self.title_lower.heap_bytes()
            + (self.artist.capacity()
                + self.album_artist.capacity()
                + self.album.capacity()
                + self.genre.capacity()
                + self.codec.capacity())
                * 4
            + (self.year.capacity() + self.track_no.capacity() + self.bitrate_kbps.capacity()) * 2
            + self.duration_ms.capacity() * 4
            + self.artists.heap_bytes()
            + self.album_artists.heap_bytes()
            + self.albums.heap_bytes()
            + self.genres.heap_bytes()
            + self.codecs.heap_bytes()
    }
}
