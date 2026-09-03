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

Measured at scale in [research 02](../../0R-research/02-library-scale.md): substring over
the projection is 31 ms worst case at 10M tracks, so the escalation to FTS5 or tantivy is
about ranking and fuzzy matching, not latency, until a library approaches ~50M tracks and
in-memory scans stop fitting.

**Amended 2026-07-12:** shipped without the debounce. Search runs per keystroke,
synchronously on the UI thread, because the measurement above makes a debounce pointless:
a query costs a fraction of a frame at any realistic library size, so delaying it only
adds typing-to-results lag. The debounce was insurance written before the measurement.
It comes back if search escalates to FTS5 or tantivy, where a query stops being
sub-frame, or if search ever moves off the UI thread.

**Amended 2026-09-02:** the matching key folds accents as well as case. Both sides of every
comparison run through `rox_library::fold` (lowercase, sharp s spelled out, NFD, combining
marks dropped), so "beyonce" finds Beyoncé and "strasse" finds Straße; people type without
accents out of habit and because the keys are awkward, and an accented tag was previously
reachable only from a keyboard that could spell it. The algorithm above is untouched: still
substring, still in memory, still per keystroke. Two consequences are worth naming. Browse
ordering moves with it, since the ranks sort on the same key, so Émilie files between Dana
and Frank instead of trailing every unaccented name. Symbol identity does not: interning
still keys on plain lowercase, so a library holding both "Beyonce" and "Beyoncé" keeps them
as two values to filter between, even though one needle now reaches both.

**Amended 2026-09-02, later the same day:** romanized sort names are written by words.
The romanization pass now spaces the reading of each token the dictionary segments
("Aki no kaze", not "akinokaze"), reads particles the spoken way (は as wa, へ as e, を as
o), and capitalises the first letter. Search is still substring on the folded sort name, so
"kaze" and "aki no" find the row and "akinokaze" typed without spaces no longer does; that
is the trade for readings a person recognises. Stored readings carry the crate's spelling
version in their source marker, so a later change to the spelling redoes only the pass's
own rows on the next run and never a person's or a service's.
