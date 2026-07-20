# ADR 16: Play queue as a mutable timeline, playlists in the library store

**Status:** Decided

Decision: the play queue is one flat, mutable timeline with a cursor. It is an
append-only pool of paths plus an `order: Vec<usize>` of indices into that pool and a
`pos` cursor into `order`. History is `order[0..pos]`, upcoming is `order[pos+1..]`, the
playing track is `order[pos]`. This is the structure the engine already runs on
([ADR 3](03-adr-gapless.md)); the change is to make it mutable and visible, not to invent
a new model. Playlists persist as two tables in the existing library database, following
the listen-events pattern ([ADR 11](11-adr-play-history.md)).

The engine stays the sole owner of the timeline. It holds `order`/`pos` and applies every
edit (enqueue, play-next, remove, move, jump) from a command, then publishes a read-only
snapshot of the order through a `Shared` mutex, the same way it already publishes
`segments` and `tracks`, bumping a revision counter so the UI can skip re-reading on the
ticks nothing changed. The Player is a thin layer over that: it sends the commands and
reads the snapshot for the UI, holding no copy of the order itself. One writer, the engine,
so there is no mirror to keep in sync. The engine stays close to what it is: a gapless
decoder walking a list. It learns to mutate that list, and nothing more.

The snapshot carries the entries, not the playing position. The engine's `pos` is the
decode cursor and runs up to a ring ahead of the speakers, since the next track opens for
the gapless boundary before the current one finishes, so anchoring on it would put the
highlight and a Play Next a track early near every boundary. Instead the playing entry is
resolved off the position clock the same way `now_playing` is, matched back to a queue
entry by path. So the snapshot only republishes when the entries change (a new session, an
insert, a remove, a move, a reshuffle), never on a plain advance, and the cursor follows
what you hear.

The first cut of this ADR put the canonical timeline in the Player and had the engine keep
a synced copy. Implementation showed that dual copy needs the Player to replay every edit
and regenerate the order on shuffle, and it fights the fact that the engine already owns
the list for the gapless boundary. The single-owner-plus-snapshot shape is less code and
matches the existing `segments`/`tracks` pattern, and the "two writers" worry that argued
against a `Shared` mutex does not apply when the Player keeps no copy.

The pool is append-only. Queue edits only ever touch `order`, never remove entries from
the pool. This is the load-bearing trick: the frame-to-track position mapping keys on the
pool index ([`Segment.track`](../../../crates/rox-playback/src/shared.rs)), so as long as
pool indices never move, any reorder or removal in `order` leaves the position math valid
and mid-playback mutation is safe instead of a rewrite.

Queue semantics for this layer, one flat list:

- Play next inserts right after the cursor, `order.insert(pos + 1, i)`.
- Add to queue appends, `order.push(i)`.
- Remove and move edit `order`; played items stay behind the cursor, so Back walks real
  history for free.
- Starting a new context (double-clicking an album or a library row) replaces the upcoming
  portion. Explicitly queued items are cleared with it. The two-layer model that keeps an
  explicit queue alive above a "playing next from X" continuation is out of scope here.

Shuffle stays in the engine, which owns `order`. An earlier cut of this ADR moved it to the
Player, back when the Player held the canonical timeline; the single-owner decision above
put `order` in the engine, so the permutation lives there too. The engine reshuffles only
the upcoming portion, `order[pos + 1..]`, leaving history and the playing entry in place and
composing with the explicit queued entries already in the list. Owning shuffle in the Player
would mean sending a permutation down and re-syncing a copy, the dual-copy cost this ADR
rejects.

Playlists are two tables in `library.db`: `playlists` (id, name, timestamps) and
`playlist_tracks` (playlist id, track id, position, and a title/artist/album snapshot).
Track identity is the stable `tracks.id`; the snapshot is the same deletion hedge listens
use, so a playlist outlives a track being deleted and survives a rescan on the rowid
([ADR 5](05-adr-library-store.md)). Paths resolve at play time through the store, the same
`paths_for` the browse panels already call.

The queue and playlists are each their own panel, modeled on the history panel's track
list ([ADR 7](07-adr-panels.md)), not modes of the library view.

Alternatives: rebuild the queue by calling `play()` on every edit, which is how playback
starts today, tears down the audio session and starts fresh, so an add-to-queue would
glitch the stream and reset position, unusable for live editing. Put the canonical timeline
in the Player and sync a copy down to the engine, rejected above for the dual-copy cost.
Store playlists in `settings.json` next to layouts, which is lighter but has no deletion
durability and goes stale on rescan, where the listen-events pattern already solved exactly
this in the same database.

Trade: the engine learns to mutate its list, which touches the audio thread and the gapless
boundary, so the risk is real but localized to the command drain. An edit that lands after
the engine has already opened the next track for a gapless boundary applies to the list but
may not change what plays across that one boundary; acceptable. Layer one clears the
explicit queue when a new context starts, which is the simplification that buys most of the
feel for a fraction of the machinery; the richer continuation is a later layer built on the
same timeline.

Open: whether the queue persists across an app restart, today `restore()` recovers a single
track and position, not a list. Shuffle semantics on a partially played order, reshuffle
upcoming only, leaving history and queued items in place. These are decided when layer one
lands, not before.

**Amendment: layer two is built.** The flat model surfaced its own flaw the moment the
queue panel existed: playing from the library seeded the view into the timeline, and since
that timeline was the queue, the panel showed the whole library. The queue should be what
you hand-pick, not what you happen to be playing. So the timeline splits into two kinds of
entry by a flag on each `OrderEntry`: context (the album or library run you started from,
seeded by play) and explicit (Play Next, Add to Queue). The engine still walks one merged
list, so the gapless path and the position clock are untouched; the split is only which
entries the UI calls "the queue". Play Next inserts an explicit entry right after the
playing track, Add to Queue after the last explicit entry in the run before the context
resumes, both anchored on the audible track off the position clock. The queue panel and the
queue widget list only the explicit entries, so plain library playback leaves them empty and
the widget's badge counts only what you queued. Playing from the library goes back to
natural progression through the view, as context, not shown as the queue. This supersedes
layer one's "starting a context clears the queue" and its album-scoping stopgap.
