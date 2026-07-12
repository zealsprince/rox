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
concurrent readers, which is what the sharded projection load rides on. One table:

```sql
CREATE TABLE IF NOT EXISTS tracks (
    id          INTEGER PRIMARY KEY,
    source      TEXT NOT NULL DEFAULT 'local',
    path        TEXT NOT NULL,
    title       TEXT NOT NULL,
    artist      TEXT NOT NULL,
    album       TEXT NOT NULL,
    genre       TEXT NOT NULL,
    year        INTEGER NOT NULL,
    track_no    INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    mtime       INTEGER NOT NULL,
    UNIQUE (source, path)
);
```

- `id` is the SQLite rowid and the durable track identity. The projection carries it
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
  range in id order. Everything the projection needs comes through it; paths do not,
  they stay in SQLite until playback asks.

## The projection

The read model per ADR 5, columnar rather than a vector of row structs. Every column
is one flat array indexed by row:

```rust
pub struct Projection {
    pub db_id: Vec<i64>,        // SQLite rowid per row
    pub title: Arena,           // contiguous bytes + offset table
    pub title_lower: Arena,     // lowercased copy for case-folded search
    pub artist: Vec<u32>,       // symbol into artists
    pub album: Vec<u32>,        // symbol into albums
    pub genre: Vec<u32>,        // symbol into genres
    pub year: Vec<u16>,
    pub track_no: Vec<u16>,
    pub duration_ms: Vec<u32>,
    pub artists: SymTable,      // symbol -> string, plus lowercase copy
    pub albums: SymTable,
    pub genres: SymTable,
}
```

- **Arena**: one `String` buffer plus a `Vec<u32>` of offsets, one per row boundary.
  `get(i)` is a slice, never an allocation. Titles are the one field too distinct to
  intern, so they get the arena instead of millions of heap `String`s. The lowercase
  copy is folded per character at build time so search never lowercases at query time.
- **Interning**: artist, album, and genre repeat heavily, so each interns to a `u32`
  symbol through a hash map during load. The finished `SymTable` is the symbol table
  plus a lowercase copy of every entry, built in parallel. Symbol tables run a
  hundredth the row count or less, which is what makes search and sort cheap.
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
2. Walk `root` recursively, keeping files whose extension matches what the playback
   engine decodes (`flac`, `mp3`, `wav`, case-insensitive), and sort the list so scan
   order is deterministic.
3. Per file: stat it, and if `(mtime, size)` matches the stored row, skip it without
   opening the file. This is what makes a rescan of an unchanged library cheap.
4. Otherwise read tags through lofty, wrapped in `catch_unwind`: a malformed file
   that errors or panics the parser costs that one file its tags, never the scan.
   Title falls back to the filename stem if the tag is missing or empty; a file whose
   tags will not read at all is still indexed under its filename with empty fields,
   so the library never silently loses a playable file.
5. Upsert in batches of 512 rows, one transaction each.

The scan returns a `ScanSummary` (`indexed`, `unchanged`, `untagged`) that feeds the
status line. Tag fields carried: title, artist, album, genre, year, track number from
the primary (or first) tag, duration from the stream properties.

## Cold open

Cold open is a projection load with no scan in front of it: open the database, run
`init_schema` (pure `CREATE IF NOT EXISTS`, so first launch and every launch take the
same path), then build the projection on the background executor while the UI stays
live behind a loading status.

The load is sharded, one reader per core (`available_parallelism`):

1. One connection reads `MAX(id)` and the rowid space splits into equal ranges.
2. One thread per shard opens its own connection and streams its range through
   `scan_range` into a shard-local builder with shard-local interners. WAL is what
   lets the readers run concurrently.
3. Shards merge: each shard's symbol table is re-interned into a global table once
   (a `u32` remap array per shard), symbol columns rewrite through the remap, arenas
   append with offsets rebased, plain columns concatenate.

Measured, this is the difference between 7.1 s serial and 1.9 s sharded to first
paint at 10M tracks, 711 ms against 259 ms at 1M; at 100k both shapes are tens of
milliseconds. The canonical artist, album, track order is built in the same
background task, so the UI receives projection and order together and paints once.

## Rescan and swap

The projection is never patched in place; it is rebuilt from SQLite and swapped
whole. That is the entire consistency mechanism between store and projection.

```
 folder walk + lofty tags           SQLite (WAL)                  projection (RAM)
 ────────────────────────           ────────────                  ────────────────
 scanner ──batched upserts──▶ tracks table ──sharded readers──▶ columnar arrays + symbols
                                     ▲                                 │
 paths_for(db_ids) ◀─────────────────┘                                 ▼
 (play resolution, UI conn)                              order / search / filter views
```

The sequence, driven by the library panel in `crates/rox/src/library.rs`:

1. The panel marks itself busy and spawns one background task.
2. On the background executor: if a scan root was given, open a connection and run
   the scan to completion; then load the projection sharded and build the canonical
   order. Scans and loads always open their own connections, the UI-side connection
   is never lent out.
3. Back on the UI thread, `Arc<Projection>`, the order, and the view swap in one
   update. The previous projection stays alive until the last in-flight render drops
   its `Arc`, then frees.

Because the swap is whole, the projection cannot half-reflect a scan. Because upserts
keep rowids, identity survives the swap: a queue built against the old projection
still resolves. The view re-derives on every search keystroke, an empty query shares
the canonical order's `Arc` and a non-empty one allocates a fresh hit vector.

Playback resolution is the one projection-to-store hop: clicking a row queues it and
up to 999 rows behind it in view order, mapping view rows to `db_id`s and `db_id`s to
paths through `paths_for` on the panel's connection. Ids that no longer resolve drop
out of the queue.

## Reference

The service lives in `crates/rox-library`: `store.rs` (schema, upsert, range reads),
`projection.rs` (arena, interning, search, sort, sharded load), `scanner.rs` (walk,
change key, lofty). The app wires it in `crates/rox/src/library.rs`. The scale
harness is `crates/rox-prototype-library`, which reuses these modules against a
generated catalog: `cargo run -p rox-prototype-library --release -- --tracks
10_000_000` reproduces the measurements in
[research 02](../0R-research/02-library-scale.md).
