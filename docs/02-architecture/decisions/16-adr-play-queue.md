# ADR 16: Play queue as a mutable timeline, playlists in the library store

**Status:** Decided

Decision: the play queue is one flat, mutable timeline with a cursor. It's an
append-only pool of paths plus an `order` of entries indexing into that pool and a
`pos` cursor into `order`. History is `order[0..pos]`, upcoming is `order[pos+1..]`, the
playing track is `order[pos]`. This is the structure the engine already runs on
([ADR 3](03-adr-gapless.md)); the change is to make it mutable and visible, not to invent
a new model. Playlists persist as two tables in the existing library database, following
the listen-events pattern ([ADR 11](11-adr-play-history.md)).

Every entry in `order` is one of two kinds, marked by a flag on its `OrderEntry`. Context
entries are the album or library run playback started from. Explicit entries are what
the listener hand-picked with Play Next and Add to Queue. The engine steps through one
merged list either way, so the gapless path and the position clock never see the
difference. The split only decides which entries the UI calls "the queue": the queue
panel and the queue widget list the explicit entries, and the widget's badge counts only
those. Plain library playback leaves both empty.

**One owner, one snapshot.** The engine is the sole owner of the timeline. It holds
`order`/`pos` and applies every edit (insert, remove, move, jump, reshuffle) from a
command, then publishes a read-only snapshot of the order through a `Shared` mutex, the
same way it already publishes `segments` and `tracks`, bumping a revision counter so the
UI can skip re-reading on the ticks nothing changed. The Player is a thin layer over
that: it sends the commands and reads the snapshot for the UI, holding no copy of the
order itself. With one writer there's no mirror to keep in sync. The engine stays close
to what it is: a gapless decoder stepping through a list. It learns to mutate that list,
and nothing more.

The snapshot contains the entries, not the playing position. The engine's `pos` is the
decode cursor, and it runs up to a ring ahead of the speakers because the next track
opens for the gapless boundary before the current one finishes. Anchoring on it would
put the highlight and a Play Next a track early near every boundary. Instead the playing
entry is resolved off the position clock the same way `now_playing` is, matched back to
a queue entry by path. So the snapshot only republishes when the entries change (a new
session, an insert, a remove, a move, a reshuffle), never on a plain advance, and the
cursor follows what you hear.

The pool is append-only. Queue edits only ever touch `order`, never remove entries from
the pool. That's what makes mid-playback edits safe. The frame-to-track position mapping
keys on the pool index ([`Segment.track`](../../../crates/rox-playback/src/shared.rs)), so
as long as pool indices never move, any reorder or removal in `order` leaves the position
math valid.

**Queue semantics.** Edits anchor on the audible track off the position clock, never on
the decode cursor.

- Play Next inserts explicit entries right after the playing track, at the front of the
  queue.
- Add to Queue inserts explicit entries after the last explicit entry in the run
  following the playing track, so they land at the tail of the queue and ahead of where
  the context picks back up.
- Playing a queued entry moves it to the front of the queue before jumping to it. A bare
  jump would leave everything above it behind the cursor as history, which reads as the
  queue clearing.
- Play Now (the drop zone) splices tracks in after the playing track and jumps to the
  first, so the rest of the queue plays on behind them.
- Remove and move edit `order`. Played entries stay behind the cursor, so Back steps
  through real history for free.
- Clear Queue drops the explicit entries only. The playing track and the context around
  it stay.
- Starting playback from a track list (a library run, an album or a selection in the
  library, the folder tree, history, a playlist, the album, artist, genre and art grids)
  replaces the context and keeps the queue. The new session's entries are all context,
  and the explicit entries that were still upcoming are spliced back in right after the
  track that starts. They go in after the session opens rather than as part of it,
  because a fresh context takes the shuffle mode and the queue would otherwise be
  scattered through the tail with it.
- The genre tagger's preview plays the way Play Now does, spliced in after the playing
  track at an offset, so a tagging pass leaves the queue alone.

Shuffle is in the engine, which owns `order`. The engine reshuffles only the upcoming
portion, `order[pos + 1..]`, leaving history and the playing entry in place and composing
with the explicit entries already in the list. Owning shuffle in the Player would mean
sending a permutation down and re-syncing a copy.

The whole timeline, flags included, is written to `session.json` as `last_queue` and
restored paused on the next launch, so Prev/Next and the queue panel come back as they
were. A file that predates it falls back to restoring the single track and position.

**Playlists** are two tables in `library.db`: `playlists` (id, name, timestamps) and
`playlist_tracks` (playlist id, track id, position, and a title/artist/album snapshot).
Track identity is the stable `tracks.id`; the snapshot is the same deletion hedge listens
use, so a playlist outlives a track being deleted and persists across a rescan on the
rowid ([ADR 5](05-adr-library-store.md)). Paths resolve at play time through the store,
the same `paths_for` the browse panels already call.

The queue and playlists are each their own panel, modeled on the history panel's track
list ([ADR 7](07-adr-panels.md)), not modes of the library view.

Alternatives: rebuild the queue by calling `play()` on every edit. That tears down the
audio session, so an add-to-queue would glitch the stream and reset the position.

Put the canonical timeline in the Player and sync a copy down to the engine. The dual
copy needs the Player to replay every edit and regenerate the order on shuffle, and it
fights the fact that the engine already owns the list for the gapless boundary.

One kind of entry, where the timeline is the queue. Playing from the library then seeds
the whole view into the timeline, and the queue panel shows the library.

Store playlists in `settings.json` next to layouts. That's lighter but has no deletion
durability and goes stale on rescan, where the listen-events pattern already solved this
in the same database.

Trade: the engine learns to mutate its list, which touches the audio thread and the
gapless boundary, so the risk is real but localized to the command drain. An edit that
arrives after the engine has already opened the next track for a gapless boundary
applies to the list but may not change what plays across that one boundary, which is
acceptable.

Keeping the queue across a new context means a hand-built queue outlives whatever it
was built on top of, and the listener clears it by hand (Clear Queue) when it's no
longer wanted. The alternative was losing a queue to a stray double-click.

**History.** The first cut of this ADR put the canonical timeline in the Player with a
synced copy in the engine. Implementation moved it to the engine, and shuffle moved with
it.

The first shipped layer had one kind of entry. The flaw showed the moment the queue
panel existed: playing from the library seeded the view into the timeline, and since
that timeline was the queue, the panel showed the whole library. The context/explicit
split fixed that and retired the album-scoping stopgap layer one had used.

The split first shipped with a new context still replacing the queue, and with the
grids, the library's album and selection plays, and the genre tagger's preview all
starting sessions of queued entries rather than context. Both cut
against the split's own premise that the queue is what you hand-picked, and both were
changed to the rule above.
