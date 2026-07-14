# ADR 11: Append-only listen events in the library store

**Status:** Decided

Decision: a listen is appended as an event, track id, timestamp, and a small snapshot
of the identifying tags at play time, to a table in the existing library database.
Every stat (play counts, artist / album / genre rollups, recency) is derived from
events; nothing stores a counter as the source.

Alternatives: play-count columns on the tracks table (foobar's foo_playcount shape), a
separate stats database file, an append-only log file outside SQLite.

Trade: counters answer "most played" and nothing else. Every question someone asks
later, listens this year, when did I stop playing this album, needs the events, and
events can't be reconstructed from counters. Product trades volume for the raw
record, and the volume is small anyway: a heavy listener logs
well under a million rows a decade, less than the tracks table already holds at the
scale ADR 5 was sized against. Same-database keeps events joinable to track identity
for rollups and rides the WAL durability story the store already has; a separate file
adds a second consistency story and buys nothing, and a log file outside SQLite gives
up the query path the rollups live on.

The snapshot is the deletion hedge. While a track exists, rollups resolve through the
live catalog, so fixing a genre tag re-buckets history with it. When a track is deleted
or a source removed, the events keep their snapshot, so history outlives the files it
was made from. Rescans are already safe without it: upserts keep rowids
([ADR 5](05-adr-library-store.md)), so track identity survives a rescan and events
follow.

Stats stay out of the in-memory projection. The projection exists for per-keystroke
browse; stats read at panel-open and listen-append cadence, so SQL over an indexed
events table is fast enough and keeps the projection sync machinery, already the main
library risk, untouched.
