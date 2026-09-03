# Library

How the library service is built: the SQLite store, the columnar projection it loads
into, the scanner that fills it, and the sequence that keeps store and projection
consistent. This makes the library contract from
[components](../02-architecture/02-components.md#library-service) concrete, within the
calls made in [ADR 5](../02-architecture/decisions/05-adr-library-store.md) (SQLite
source of truth plus an in-memory projection) and
[ADR 6](../02-architecture/decisions/06-adr-search.md) (in-memory substring search).
The shape was measured at 10 million tracks in
[research 02](../0R-research/02-library-scale.md); the numbers cited below are from
that run. Version-sensitive: the store is rusqlite 0.34 with bundled SQLite, tag reads
are lofty 0.24, the parallel scans are rayon, the title finder is memchr's memmem.

## The store

One database at `data_dir/rox/library.db` (so `~/.local/share/rox/library.db` on
Linux), opened in WAL mode with `synchronous = NORMAL`. WAL is load-bearing: it gives
concurrent readers, which the sharded projection load depends on. The catalog is
one table, with the listens (ADR 11), playlists (ADR 16), and genre opinions in the
same database:

```sql
CREATE TABLE IF NOT EXISTS tracks (
    id            INTEGER PRIMARY KEY,
    source        TEXT NOT NULL DEFAULT 'local',
    path          TEXT NOT NULL,
    title         TEXT NOT NULL,
    artist        TEXT NOT NULL,
    album_artist  TEXT NOT NULL DEFAULT '',
    album         TEXT NOT NULL,
    genre         TEXT NOT NULL,
    year          INTEGER NOT NULL,
    disc_no       INTEGER NOT NULL DEFAULT 0,
    track_no      INTEGER NOT NULL,
    duration_ms   INTEGER NOT NULL,
    codec         TEXT NOT NULL DEFAULT '',
    bitrate       INTEGER NOT NULL DEFAULT 0,
    sample_rate   INTEGER NOT NULL DEFAULT 0,
    bit_depth     INTEGER NOT NULL DEFAULT 0,
    rating        INTEGER NOT NULL DEFAULT 0,
    added         INTEGER NOT NULL DEFAULT 0,
    size          INTEGER NOT NULL,
    mtime         INTEGER NOT NULL,
    rg_track_gain REAL, rg_track_peak REAL,
    rg_album_gain REAL, rg_album_peak REAL,
    rg_source     INTEGER,
    UNIQUE (source, path)
);
```

- `id` is the SQLite rowid and the durable track identity. The projection stores it
  as `db_id`, and playback resolves it back to a path through `paths_for`.
- Identity is source-qualified per the components contract: `(source, path)` is
  unique, `local` is the first source, and a streaming extension adds rows under its
  own source string instead of forcing a migration.
- The write path is `insert_batch`: one transaction per batch of rows, with
  `INSERT ... ON CONFLICT (source, path) DO UPDATE` on every column except the key. A
  rescanned file keeps its `id`, so projection `db_id`s and anything built on them (a
  play queue, a selection) stay valid across a rescan.
- `mtime` (seconds since epoch) and `size` are the scanner's change key, read back in
  one pass by `local_files` before a scan.
- The read path is `scan_range`, which streams the projection columns for one rowid
  range in id order. Everything the projection needs comes through it; paths don't,
  they stay in SQLite until playback asks for them.
- The four ReplayGain columns are nullable rather than defaulted (ADR 19): 0 dB is a
  real measurement, and a column that couldn't tell it from an untagged file would level
  every untagged track to the reference. `rg_source` says which filled them, the file's
  tags or rox's own measurement pass, and the upsert's `KEEPS_MEASURED_GAIN` condition
  is the precedence rule: tags win wherever a file has them, a measurement is kept
  through a rescan that still finds none.
- `rating` and `added` are the app's own, never read from a tag, which is why they're
  the two columns whose migrations don't reset `mtime`.

Schema changes go through `migrate::run`, an ordered slice of steps over SQLite's
`PRAGMA user_version`, each in its own transaction. Step 1 is the baseline, the old
idempotent init with its column probes, so a pre-ladder file converges to it and stamps
1; every step after that is a clean forward one. Steps are additive by policy: an older
binary pointed at a newer file runs nothing and works against the columns it knows.
A step adding something the scanner reads out of tags resets every `mtime` with it, or
the next scan skips unchanged files and the column stays empty forever.

## The projection

The read model per ADR 5, columnar rather than a vector of row structs. Every column
is one flat array indexed by row:

```rust
pub struct Projection {
    pub fold: bool,             // whether the symbols interned case-folded
    pub db_id: Vec<i64>,        // SQLite rowid per row
    pub title: Arena,           // contiguous bytes + offset table
    pub title_lower: Arena,     // lowercased copy for case-folded search
    pub artist: Vec<u32>,       // symbol into artists
    pub album_artist: Vec<u32>,
    pub album: Vec<u32>,        // symbol into albums
    pub genre: Vec<u32>,        // symbol into genres
    pub codec: Vec<u32>,
    pub folder: Vec<u32>,       // parent directory, interned like the rest
    pub year: Vec<u16>,
    pub disc_no: Vec<u16>,
    pub track_no: Vec<u16>,
    pub duration_ms: Vec<u32>,
    pub bitrate_kbps: Vec<u16>,
    pub sample_rate_hz: Vec<u32>,
    pub bit_depth: Vec<u8>,
    pub added: Vec<i64>,
    pub track_gain: Vec<i16>,   // ReplayGain in centi-dB, i16::MIN for untagged
    pub album_gain: Vec<i16>,
    pub rating: Vec<AtomicU8>,  // written in place, no reload to rate a track
    pub plays: Vec<AtomicU32>,  // the listens table's per-track count, same reason
    pub artists: SymTable,      // symbol -> string, plus lowercase copy
    pub album_artists: SymTable,
    pub albums: SymTable,
    pub genres: SymTable,
    pub codecs: SymTable,
    pub folders: SymTable,
    // plus per-table rank arrays and the distinct artist/album lists, all
    // OnceLock: the projection is immutable once loaded, so memoizing is safe.
}
```

- **Arena**: one `String` buffer plus a `Vec<u32>` of offsets, one per row boundary.
  `get(i)` is a slice, never an allocation. Titles are the one field too distinct to
  intern, so they get the arena instead of millions of heap `String`s. The lowercase
  copy is folded per character at build time so search never lowercases at query time.
- **Interning**: artist, album artist, album, genre, codec, and folder all repeat
  heavily, so each interns to a `u32` symbol through a hash map during load. The finished `SymTable` is the symbol table
  plus a lowercase copy of every entry, built in parallel. Symbol tables run a
  hundredth the row count or less, which makes search and sort cheap.
- **ReplayGain**: the two gains are stored in the projection so the library's Gain
  column can draw and sort without a query per row, packed to hundredths of a dB in an
  `i16` since every real gain falls inside the +-40 dB the engine acts on. `i16::MIN` is
  untagged, which also sorts it ahead of every real value. The peaks stay in SQLite:
  they bound playback and nothing browsing reads them. `gain_db(row, album_first)`
  applies the leveling mode's pick and its fallback, the same one the engine levels
  by, so the column and playback never disagree about which figure a track has.
- **Resolve**: the UI renders through `resolve(row) -> RowView`, which borrows title,
  artist, and album straight out of the arena and tables. Resolving a visible window
  is O(visible), microseconds at any library size.
- The whole thing costs about 70 MB of heap per million tracks, so tens of MB at the
  100k scale ADR 5 was sized against.

Views over the projection are `Vec<u32>` of row indices: the canonical browse order,
a search result, a filter result. The projection itself never reorders.

## Search, filter, and sort

Search is the ADR 6 first stage: case-folded substring over title, artist, album, and
genre, entirely in memory.

1. The query is lowercased once.
2. Each symbol table is matched whole, in parallel, producing a hit mask per table.
   That covers artist, album, and genre for every row at symbol-table cost.
3. The row scan then does three mask lookups plus one memmem over the row's
   lowercased title, split across cores in fixed 65,536-row chunks. Chunk order keeps
   results in row order without a sort.

Worst case measured at 10M tracks is 31 ms (a single character matching 9.7M rows);
typical queries are under 20 ms and scale down linearly with library size.

Filters use the same chunked scan with integer predicates: genre resolves the string
to its symbol once and compares `u32`s, year range compares `u16`s. Both are
single-digit milliseconds at 10M.

Sorts never compare strings when a symbol exists: `ranks` precomputes each table's
alphabetical rank per symbol (sort the symbols once, invert to a rank array), so the
canonical artist, album, track-number order sorts ten million rows on a
`(u32, u32, u16)` key, 250 ms at 10M. Title sort has no symbols and compares arena
strings, 843 ms at 10M; year sort is a `u16` key. All sorts are parallel unstable
sorts producing a fresh index vector.

## The scan pipeline

`scanner::scan(conn, root)` is blocking and runs on the background executor. The
pipeline, per ADR 4's single metadata layer:

1. Load the change key map: every local path with its stored `(mtime, size)`.
2. Walk `root` recursively, keeping files whose extension is in `scanner::EXTENSIONS`
   (flac, mp3, wav, ogg, oga, m4a, m4b, aac, aif, aiff, aifc, mka, caf, case-insensitive,
   the one list an external open uses too), and sort the list so scan order is
   deterministic.
3. Per file: stat it, and if `(mtime, size)` matches the stored row, skip it without
   opening the file. That's why a rescan of an unchanged library is cheap.
4. Otherwise read tags through lofty, wrapped in `catch_unwind`: a malformed file
   that errors or panics the parser costs that one file its tags, never the scan.
   Title falls back to the filename stem if the tag is missing or empty; a file whose
   tags won't read at all is still indexed under its filename with empty fields,
   so the library never silently loses a playable file.
5. Upsert in batches of 512 rows, one transaction each.

The scan returns a `ScanSummary` (`indexed`, `unchanged`, `untagged`) for the status
line. Tag fields read: title, artist, album artist, album, genre, year, disc
and track number, rating (FMPS exact, POPM stars), and the four ReplayGain values, all
from the primary (or first) tag. Multi-value genres join on the `"; "` convention
`rox_library::genre` owns, so a list makes the round trip through one column intact.
Duration, codec, bitrate, sample rate, and bit depth come off the parsed stream
properties rather than any tag, so an untagged file still reports what it is.

## Cold open

Cold open is a projection load with no scan in front of it: open the database, run
`init_schema` (pure `CREATE IF NOT EXISTS`, so first launch and every launch take the
same path), then build the projection on the background executor while the UI stays
live behind a loading status.

The load is sharded, one reader per core (`available_parallelism`):

1. One connection reads `MAX(id)` and the rowid space splits into equal ranges.
2. One thread per shard opens its own connection and streams its range through
   `scan_range` into a shard-local builder with shard-local interners. WAL lets the
   readers run concurrently.
3. Shards merge: each shard's symbol table is re-interned into a global table once
   (a `u32` remap array per shard), symbol columns rewrite through the remap, arenas
   append with offsets rebased, plain columns concatenate.

Measured, this is the difference between 7.1 s serial and 1.9 s sharded to first
paint at 10M tracks, 711 ms against 259 ms at 1M; at 100k both shapes are tens of
milliseconds. The canonical artist, album, track order is built in the same
background task, so the UI receives projection and order together and paints once.

## Rescan and swap

The projection is rebuilt from SQLite and swapped whole on every scan, reload,
removal, and prune. That's the consistency mechanism between store and projection;
the watch patch below is the one path that changes a projection without a rebuild,
and it exists only to reach the same state cheaper.

```
 folder walk + lofty tags           SQLite (WAL)                  projection (RAM)
 ────────────────────────           ────────────                  ────────────────
 scanner ──batched upserts──▶ tracks table ──sharded readers──▶ columnar arrays + symbols
                                     ▲                                 │
 paths_for(db_ids) ◀─────────────────┘                                 ▼
 (play resolution, UI conn)                              order / search / filter views
```

The sequence, driven by the library panel in `crates/rox-panels/src/library.rs`:

1. The panel marks itself busy and spawns one background task.
2. On the background executor: if a scan root was given, open a connection and run
   the scan to completion; then load the projection sharded and build the canonical
   order. Scans and loads always open their own connections, the UI-side connection
   is never lent out.
3. Back on the UI thread, `Arc<Projection>`, the order, and the view swap in one
   update. The previous projection stays alive until the last in-flight render drops
   its `Arc`, then frees.

Because the swap is whole, the projection cannot half-reflect a scan. Because upserts
keep rowids, identity is preserved across the swap: a queue built against the old
projection still resolves. The view re-derives on every search keystroke, an empty
query shares the canonical order's `Arc` and a non-empty one allocates a fresh hit vector.

## Watch patches

A filesystem watch event or a reindex is the one place the projection changes
without a rebuild. The sync collects the ids it touched while the store
connection is open (a rename reads its ids before the move, a prune before the
delete), reads those rows back through the same shard builder a full load uses,
appends them to the live projection, and tombstones the rows they replace; a
removal is a tombstone alone. The canonical order takes the new rows at their
sorted position and drops the dead ones, and the id-to-row map is patched in
place. Search, filter, sort, and every scan inside the projection skip
tombstones, so a view never hands one out; code that walks a column by index
checks `is_dead` itself, and `live_len` is the count a browse sees where `len`
stays the physical row bound.

Tombstones accumulate until the next full rebuild. The catalog stops patching
and rebuilds instead once dead rows pass a tenth of the projection, or when a
single event touches more than a few thousand rows, since the patch reads its
rows one primary-key lookup at a time. Two things a patch doesn't reproduce
until that rebuild: the display casing of a value that already had a symbol
stays whatever the last build's vote chose, and a symbol whose rows have all
died still sits in its table. Measured on the generated 1M-track store, a
single-row upsert or remove costs about a tenth of a millisecond against a
350 ms rebuild (see the benchmarks in
[research 02](../0R-research/02-library-scale.md)).

Playback resolution is the one projection-to-store hop: double-clicking a row queues
it and up to 999 rows behind it in view order, mapping view rows to `db_id`s and
`db_id`s to paths through `paths_for` on the library's UI-side connection. Ids that no
longer resolve drop out of the queue. Single clicks publish the selected rows as
`db_id`s to the app-wide selection entity, and panels showing the selection resolve
those ids back through the same hop.

## Reference

The service is in `crates/rox-library`: `store.rs` (schema, upsert, range reads),
`migrate.rs` (the user_version ladder both databases run), `projection.rs` (arena,
interning, search, sort, sharded load), `scanner.rs` (walk, change key, lofty),
`genre.rs` and `genre_meta.rs` (the multi-value convention and the alias table behind
it), `replaygain.rs` (the tag values and where they came from), `art.rs` (cover art off
a track's tags, with a folder image as the fallback). The app wires it in `crates/rox-panels/src/library.rs`. The scale
harness was `crates/rox-prototype-library` (git history, commit bd22dc1), which
reuses these modules against a generated catalog: `cargo run -p
rox-prototype-library --release -- --tracks 10_000_000` reproduces the
measurements in
[research 02](../0R-research/02-library-scale.md).
