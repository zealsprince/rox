# ADR 11: Append-only listen events in the library store

**Status:** Decided; amended below

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

**Amended 2026-10-04: a scrobble with no track gets a row of its own.** The Last.fm import
files each dated scrobble as a listen on the track it names. One that named no library
track was dropped, and since a rerun only asks for scrobbles newer than the last, adding
the album later never brought those plays back.

Now such a scrobble lands on an Unknown row: a `tracks` row under the reserved source
`unknown`, one per song, its path the folded artist and title so case and accent variants
share it (`rox-library/src/unknown.rs`). It never plays and never loads into the
projection, so browse and search can't reach it. The history reads it through the same
SQL as any other row. Ratings and every playlist but Favourites refuse it, and a play
request drops it the way it drops a deleted id.

The loved-tracks import files a love that names no library track the same way: the song
gets its Unknown row and the row gets the heart, so Favourites lists it. Double-clicking
it there searches a plugin the way History does. The prune that clears Unknown rows with
no listens left spares a hearted one.

Four things move its listens to a real track. After a scan or a watched-file reindex,
`relink` (`rox-services/src/unknown.rs`) matches every Unknown row against the local
tracks with the loved-tracks import's name rules, and the copy played most takes them.
A hearted row gets a second pass, `relink_hearts`, against every plugin track the library
holds, synced, saved or picked. It runs after the local pass and on every plain load,
which is how a plugin sync ends. When a listen lands on a plugin track, `claim` hands it
every hearted Unknown row naming the same song. And when a plugin search finds and plays
the song, `adopt` hands them to the plugin's row.

A service credits everyone on a track in one string ("Lemaitre, Sofiloud") where Last.fm
names the lead, so the matcher files a comma list under its first name too, and a
plugin's album artist counts as a second name.

Every way, the listens take the new row's source and path, keep the tags they were heard
with, and the heart moves along with them. Then the Unknown row is deleted. A hearted
pick no longer expires, the same way a bookmarked one doesn't. A listen the new row
already holds at the same second is the same play counted twice, rox's record and
Last.fm's echo of it, and is dropped. That's the one delete of a listen outside a
confirmed clear. With no Unknown row in the library, the relink is a single indexed
probe.
