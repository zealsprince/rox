# ADR 6: Search: in-memory substring first, FTS5 next, tantivy only if needed

**Status:** Decided

Decision: start with in-memory substring filtering over the interned columns behind a
debounced search box. Add SQLite FTS5 if we want BM25 ranking and phrase/boolean queries.
Reach for tantivy only if typo-tolerant fuzzy search becomes a hard requirement.

Alternatives: FTS5 from the start, or tantivy from the start.

Trade: at 50-100k tracks with short fields, all three are fast enough, so latency isn't the
deciding axis, simplicity is. In-memory substring is already sub-frame over a library we
hold in RAM and costs nothing to build. FTS5 ships with the SQLite we already have but has
no native fuzzy matching. tantivy is the only one with real edit-distance fuzzy, at the
highest integration cost (schema, writer, commit/reload cycle) and a scale advantage that's
irrelevant here. We don't pay for fuzzy until a user needs it.
