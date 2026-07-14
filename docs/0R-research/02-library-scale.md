# Library scale prototype

Answers the question [ADR 5](../02-architecture/decisions/05-adr-library-store.md) and
[ADR 6](../02-architecture/decisions/06-adr-search.md) never asked: they were sized
against 50-100k tracks, so does SQLite plus a full in-memory projection with substring
search still deliver sub-second search, instant filtering, and smooth navigation at 10
million tracks, and where does the work have to split across cores to get there?

The prototype lived in `crates/rox-prototype-library` (git history, commit bd22dc1). No real files are involved: a
deterministic generator writes a synthetic catalog into SQLite with realistic
cardinalities (10M tracks lands at 272k artists and 433k distinct album names), and the
projection loads from there exactly as the real library service would. The projection is
columnar the way the [non-functional model](../02-architecture/03-non-functional.md)
prescribes, with the parts that only matter past a million tracks made concrete:

- Artist, album, and genre intern to `u32` symbols; titles live in one contiguous byte
  arena with an offset table, never ten million heap `String`s.
- Search scans the interned tables whole (they're a hundredth the row count), so only
  titles need the full-row pass, and that pass splits across cores in fixed chunks.
- Sort comparisons run on precomputed integer ranks per symbol.
- Cold open loads either serially (the ADR 5 shape as written) or with one SQLite reader
  per core over disjoint rowid ranges (WAL gives concurrent readers for free), merged by
  remapping shard-local symbols.

```sh
cargo run -p rox-prototype-library --release -- --tracks 10_000_000
```

The database is kept under `target/` and reused when the row count matches, so repeat
runs only measure the read path. `--skip-serial` and `--skip-like` trim the slow
contrast measurements.

## Numbers

Release build, 32 cores, 125 GB RAM, NVMe. Absolute times shift with the machine; the
scaling is the finding. Search times are warm runs behind what would be a debounced
search box; hits are total matches, not a capped page.

| operation                          | 1M tracks | 10M tracks |
| ---------------------------------- | --------- | ---------- |
| populate (generate + insert)       | 4.9 s     | 73 s       |
| database size                      | 0.23 GB   | 2.37 GB    |
| cold open, serial (1 reader)       | 711 ms    | 7.1 s      |
| cold open, parallel (32 readers)   | 259 ms    | 1.9 s      |
| projection heap                    | 0.07 GB   | 0.70 GB    |
| process RSS                        | 0.26 GB   | 1.27 GB    |
| sort: artist / album / track       | 28 ms     | 250 ms     |
| sort: title (string compare)       | 33 ms     | 843 ms     |
| sort: year                         | 8 ms      | 113 ms     |
| search "velvet thunder"            | 1.9 ms    | 19 ms      |
| search "moon" (409k hits at 10M)   | 2.6 ms    | 20 ms      |
| search "a" (9.7M hits at 10M)      | 1.8 ms    | 31 ms      |
| search, no match                   | 2.2 ms    | 16 ms      |
| filter: genre                      | 0.3 ms    | 4.7 ms     |
| filter: year range                 | 0.3 ms    | 6.0 ms     |
| scroll: resolve a 50-row window    | 1.8 µs    | 2.3 µs     |
| sqlite `LIKE '%moon%'` (contrast)  | 167 ms    | 1.7 s      |

## Reading

The architecture holds at 10 million tracks with two orders of magnitude of headroom on
the search budget. Worst-case search, a single character matching 9.7 million rows, is
31 ms. Every filter is single-digit milliseconds. Scroll windows are microseconds and
flat across scale, because resolving visible rows is O(visible), not O(library). The
numbers scale linearly from 1M to 10M with no cliff.

The splits are what buy it:

- The interned tables carry the search. A query hits artists, albums, and genres by
  scanning ~700k short strings, and the only per-row work is a memmem over the title
  arena, parallel in chunks. The SQLite `LIKE` contrast is the same question pushed to
  the durable store: 1.7 s, over budget on its own.
- Cold open is the one place the serial shape breaks the experience: 7.1 s to first
  paint at 10M. Sharded readers over rowid ranges bring it to 1.9 s. The speedup is
  ~4x, not 32x, because per-row rusqlite extraction and the merge dominate. A
  projection snapshot on disk (the columns are flat arrays and serialize as-is) would
  cut the per-row work entirely, and a view snapshot persisted at close can paint the
  last visible rows in milliseconds while the projection loads behind it.
- Sort-by-title is the only near-second click at 10M (843 ms), because it compares
  strings. The canonical artist/album/track order runs on integer ranks in 250 ms.
  Title ranks could be precomputed the same way if that click ever feels slow, and the
  standard orders can be built once at open, off the UI thread.

ADR 5's "tens of MB even at 100k" extrapolates to ~1 GB of projection at 10M, and
that's what it costs (0.70 GB heap, 1.27 GB RSS). That's fine on any machine that
would hold a 2.4 GB catalog database anyway, and the 100k library the ADRs were sized
for stays at tens of MB. Nothing in ADR 5 or ADR 6 needs to change; the projection
just has to be built columnar, interned, and chunk-parallel rather than as a vector of
row structs.

Linear scaling also makes the ceiling a number instead of a guess. At 100M tracks the
projection is ~7 GB and worst-case search ~300 ms; at 1B it's ~70 GB and ~3 s, past
the budget. Somewhere around 50M the resident-scan design stops fitting, and search
and sort have to move from scans to disk indexes (ADR 6's FTS5 and tantivy escalation,
persisted sort orders) behind the same library service contract. No local library
gets there; it matters because the browse/search boundary is what lets that backend
swap in without the UI noticing.

## What this doesn't settle

- The sync machinery, which ADR 5 names as the main library risk. Scan results, tag
  edits, and filesystem events updating SQLite and the projection incrementally is
  untested here; this prototype only proves the read path is worth that work.
- Album identity. The prototype interns album name strings, so same-named albums by
  different artists share a symbol, which is harmless under artist-first ordering but
  wrong for an album grid. Real album entities key on (album artist, title).
- Scanning 10 million real files. Populate here is a generator; a real initial scan
  runs jwalk plus lofty over a filesystem, is hours of tag parsing at this scale, and
  is a separate question.
- Search ranking. Results come back in row order; a search box wants relevance, which
  is where ADR 6's FTS5-next step comes in. Latency headroom says ranking can afford
  to cost something.
- GPUI rendering. Window resolution is microseconds, but nobody has scrolled a
  `uniform_list` bound to a 10M-row index vector yet.
