# ADR 6: Search: in-memory substring first, FTS5 next, tantivy only if needed

**Status:** Decided

Decision: start with in-memory substring filtering over the interned columns, behind a
debounced search box. Add SQLite FTS5 if we decide we want BM25 ranking and
phrase/boolean queries. Reach for tantivy only if typo-tolerant fuzzy search becomes a
hard requirement.

Alternatives: FTS5 from the start, or tantivy from the start.

Trade: at 50-100k tracks with short fields, all three of these are fast enough, so
latency doesn't decide it and simplicity does. In-memory substring is already sub-frame
over a library we're holding in RAM anyway, and it costs nothing to build because the
projection is already there. FTS5 comes with the SQLite we're already linking, so it's
nearly free to reach. It has no native fuzzy matching, though, and that's the one
capability worth escalating for. tantivy is the only one of the three with real
edit-distance fuzzy, and it's also the most expensive to integrate: a schema, a writer,
and a commit/reload cycle to keep in step with the catalog. Its other advantage is
scale, which is irrelevant at the sizes this library reaches. So we don't pay for fuzzy
until someone actually needs it.

Measured at scale in [research 02](../../0R-research/02-library-scale.md): substring over
the projection is 31 ms in the worst case at 10M tracks, so moving to FTS5 or tantivy is
about ranking and fuzzy matching rather than speed. That holds until a library
approaches roughly 50M tracks, where in-memory scans stop fitting and the reason
changes.

**Amended 2026-07-12:** shipped without the debounce. Search runs per keystroke,
synchronously on the UI thread, because the measurement above makes a debounce
counterproductive rather than merely unnecessary. A query costs a fraction of a frame at
any realistic library size, so a debounce can't save work worth saving and can only add
lag between typing and results. It was insurance written before the numbers existed. It
comes back if search escalates to FTS5 or tantivy, where a query stops being sub-frame,
or if search ever moves off the UI thread.

**Amended 2026-09-02:** the matching key now folds accents as well as case. Both sides of
every comparison run through `rox_library::fold`, which lowercases, spells out the sharp
s, normalizes to NFD, and drops combining marks. So "beyonce" finds Beyoncé and "strasse"
finds Straße. People type without accents, out of habit and because the keys are awkward
to reach. Before this, an accented tag could only be found from a keyboard that could
spell it.

The algorithm itself is untouched: still substring, still in memory, still per keystroke.
Browse ordering moves with it, because the sort ranks
are built on the same folded key, so Émilie now files between Dana and Frank instead of
trailing every unaccented name. Symbol identity doesn't move, because interning still
keys on plain lowercase. A library holding both "Beyonce" and "Beyoncé"
keeps them as two separate values you can filter between, even though a single search
term now reaches both.

**Amended 2026-09-02, later the same day:** romanized sort names are written by words.
The romanization pass spaces the reading of each token the dictionary segments, so "Aki
no kaze" rather than "akinokaze", reads particles the way they're spoken (は as wa, へ as
e, を as o), and capitalises the first letter.

Search is still substring over the folded sort name, and spacing the reading changes what
matches. "kaze" and "aki no" find the row; "akinokaze" typed as one word no longer does.
That's the trade taken for readings a person recognises when they see them. Stored
readings record which version of the crate's spelling produced them, in their source
marker, so changing the spelling rules later re-runs only the rows this pass generated
and never touches a reading a person or a service supplied.
