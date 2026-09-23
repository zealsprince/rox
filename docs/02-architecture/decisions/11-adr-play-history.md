# ADR 11: Append-only listen events in the library store

**Status:** Decided

Decision: a listen is appended as an event to a table in the existing library database.
The event holds the track id, the timestamp, and a small snapshot of the identifying tags
as they read at play time. Every stat (play counts, artist / album / genre rollups,
recency) is derived from those events, and nothing anywhere stores a counter as the
source of truth.

Alternatives: play-count columns on the tracks table (foobar's foo_playcount shape), a
separate stats database file, an append-only log file outside SQLite.

Trade: a counter answers "most played" and can't answer anything else, because
incrementing a number throws away when it happened. How many tracks did I listen to this
year, when did I stop playing this album, what was I on in March: all of those need the
individual events, and there's no way back to them from a count. So the record is kept
raw and the volume paid for. That volume turns out to be small anyway. A heavy listener
logs well under a million rows in a decade, which is fewer than the tracks table already
holds at the scale ADR 5 was sized against.

Putting them in the library database rather than beside it does two things. The events
can be joined against track identity, which is what a per-artist or per-genre rollup is,
and they inherit the WAL durability the store already has. A separate file would need its
own consistency story on top of the one that exists and would buy nothing for it. A log
file outside SQLite would give up the query path the rollups are built on.

The snapshot is the hedge against deletion. While a track still exists, a rollup resolves
through the live catalog rather than the snapshot, so correcting a genre tag re-buckets
the history along with it. Once a track is deleted or its source removed, the live
catalog has nothing left to resolve through, and the events fall back on the tags they
recorded, so the history outlives the files it was made from. Rescans never needed this:
upserts keep rowids ([ADR 5](05-adr-library-store.md)), so a track keeps its identity
across a rescan and its events stay attached to it.

Stats stay out of the in-memory projection. The projection exists to answer per-keystroke
browse, where a query runs on every character typed. Stats are read when a panel opens
and when a listen is appended, thousands of times less often. SQL over an indexed events
table is quick enough at that rate, and the projection's sync machinery, already the main
library risk, is left alone.
