# ADR 5: SQLite source of truth plus an in-memory projection

**Status:** Decided

Decision: SQLite via `rusqlite` (bundled, WAL) as the durable store and write path, with a
full in-memory columnar projection as the read path for browse, sort, and filter.

Alternatives: redb (pure-Rust embedded KV), sled, a plain serialized in-memory cache, or
in-memory only.

Trade: the browse workload wants arbitrary filter, group-by-album, and sort-by-any-field,
which SQL and secondary indexes give for free and a KV store makes us hand-build. redb is
the credible pure-Rust runner-up if avoiding the C dependency matters more than SQL; sled
is effectively abandoned. The catalog is small in RAM (tens of MB even at 100k tracks), so
holding a full projection is cheap and turns browse/sort/filter into microsecond in-memory
work rather than per-keystroke queries. The cost is the sync machinery: every scan result,
tag edit, and filesystem event has to update SQLite and the projection consistently without
a full rebuild. That sync is the most complex part of the library service, and the
[non-functional model](../03-non-functional.md) treats it as the main
library risk.
