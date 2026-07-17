# Components and contracts

Per-component responsibility, boundary, and contract, within the domain split laid out
in the [overview](01-overview.md).

## Playback engine

Responsibility: turn a queue of tracks into sample-accurate audio, and expose a live PCM
tap. Owns decode (Symphonia), output (cpal), the gapless queue, volume/ReplayGain, and
the tap ring.

Boundary: the real-time output callback is the hard line. It only reads from a
pre-allocated ring buffer and writes to the device. No allocation, no lock, no logging,
no database. Everything else in the engine lives on a normal decode thread behind that
line.

Formats: MP3 and FLAC decode through Symphonia with no C dependency (FLAC on by default,
MP3 pure-Rust behind a feature flag). The contract is format-agnostic, so adding a format
is additive; Opus is the first place a C dependency would enter.

Contract to the UI:
- In: `play`, `pause`, `seek(pos)`, `next`, `prev`, `enqueue(track)`, `set_volume`,
  `set_loop`, `set_shuffle`, `set_output_device`. Commands cross a channel, they don't
  call into the RT thread.
- Out: playback state (current track, position, playing/paused, device), emitted as the
  UI's shared entity updates so views re-render on the next frame.
- Out: the PCM tap, a second SPSC ring the visualizer drains. Lossy by design, a slow UI
  drops stale samples rather than back-pressuring audio.

## Library service

Responsibility: hold the catalog, keep it fast, keep it current. SQLite is the durable
source of truth and the write path. A full in-memory projection is the read path that
makes browse, sort, and filter instant. Track identity is source-qualified, (source, id)
with local files as the first source, so source extensions extend the catalog
instead of forcing a migration (see
[source extensibility](01-overview.md#source-extensibility)).

Boundary: browsing never touches SQLite. The UI reads the shared in-memory projection
and derives its views from it; paths stay in the store, so playing a row costs one
id-to-path read back through the service. Consistency is by rebuild: the projection is
never patched, it is rebuilt from SQLite and swapped whole.

Contract to the UI:
- In: `rescan(root)`, `watch(on/off)`, and `paths_for(ids)`, the id-to-path hop that
  playback and selection resolve through.
- Out: the projection, shared read-only, that browse order, search, filter, and sort
  derive from, and a change event per swap so open views refresh together.

Contract to the metadata writer: after a successful tag write, the writer sends a reindex
request for those paths, and the library re-reads them and emits change events. A tag edit
and the browse view converge without a full rescan.

## Play history

Responsibility: turn playback into a durable record of listens and answer the stat
queries panels ask: play count and recency per track, rolled up by artist,
album, and genre. Product hands down the shape ([scope](../01-product/03-scope.md)):
events with timestamps keyed to track identity, never bare counters, because the raw
record is what every future stat derives from.

Boundary: nothing here touches the audio path. The playback engine already emits state
(current track, position, transitions); play history consumes that state on the control
side, applies the listen rule, and appends to the store off the UI thread. The listen
rule matches the scrobble standard, half the track or four minutes of it, whichever
comes first. Storage is the library database per
[ADR 11](decisions/11-adr-play-history.md); aggregates are derived from events, and
stats read at panel-open cadence, not per keystroke, so they stay in SQL rather than
the projection.

Contract:
- In: playback state transitions (track opened, position advanced, track
  ended or skipped), and the track's identity from the library.
- Out: one listen event appended per real listen, stat queries (per-track count and
  last-played, artist / album / genre rollups, recents), and a change event per append
  so open views refresh.
- To enrichment: the scrobbler accrues played time off the same position clock, so
  seeks and pauses don't count for either, but sends on its own threshold, a user
  knob, rather than the listen rule.

## Metadata writer

Responsibility: read and write tags across the format matrix, safely, in bulk. Wraps
lofty with an atomic-write layer, because lofty rewrites files in place and a crash
mid-write can leave a file unrecoverable. The core formats both write through lofty:
ID3v2 for MP3, Vorbis comments for FLAC.

Boundary: this is the only component that writes audio files. Every write goes through
copy, verify, atomic rename. Reads are isolated per file so a malformed file that panics
lofty's parser takes down one worker, not the batch.

Contract:
- In: `read(path)`, `commit(path, changes)`, `commit_batch(edits)`.
- Out: per-file success or failure, never a partial corrupt file. On success, a reindex
  request to the library service.
- Custom and arbitrary tag fields go through lofty's format-specific tag types (ID3v2
  TXXX, MP4 freeform atoms, Vorbis keys), not the generic key abstraction, which has no
  slot for unknown keys and can drop them.

## Artwork service

Responsibility: feed the album-art grid without stalling the scroll. Generates 256px
thumbnails once, caches them, and hands the UI decoded textures.

Boundary: two bounded pools, not one thread per tile. A worker pool loads and resizes
thumbnails from a dedicated SQLite thumbnail DB; a bounded LRU of decoded textures sits in
front, sized to the viewport plus a margin, not the whole library.

Contract:
- In: `thumbnail(key, size)` where key is content-addressed (path + mtime + size).
- Out: a texture handle, or a placeholder plus a pending load. Off-screen requests cancel.

## Visualizer subsystem

Responsibility: the spectrum analyzer and the waveform seekbar. Consumes the playback
PCM tap, owns the FFT analysis and the per-track waveform cache. The generative visual
is gated on a real GPU shader ([ADR 8](decisions/08-adr-visualizer-rendering.md)).

Boundary: analysis runs off the UI thread; rendering is GPUI primitives in a paint
callback.

Contract:
- In: the PCM tap ring, plus the current track for waveform precompute.
- Out: analysis frames (spectrum bands, recent samples) the UI draws, and a cached
  min/max peak waveform per track (a few KB, keyed on file identity: path, size, mtime).

## UI shell and panel system

Responsibility: the composable window. The dock, panels, split/resize,
duplicate-with-config, pop-out into OS windows, layout persistence, and theming.

Boundary: panels are views over shared entities. A duplicated panel is a second view with
its own config over the same underlying state. A popped-out panel is a second OS window
whose views point at the same entities as the main window, so playback, library, and
selection state stay shared without any cross-window messaging.

Contract:
- Layouts and themes serialize to disk as shareable artifacts. A layout is an arrangement
  of panels and their configs; a theme is a token set (colors, fonts, spacing, accent).
  Neither carries executable behavior.
- Settings split by scope: an app settings window edits the settings file (appearance,
  behavior, library folders, scrobbling, storage), and a per-panel customize window edits
  that panel's config. Per-view state lives in panel config: columns, sort, density,
  theme overrides, and the search query, entered through one shared box component, so
  duplicated panels diverge and a layout carries all of it.

## Network enrichment boundary

Scrobbling, tag lookup / auto-tagging, and lyrics all reach the network to enrich a local
library. They share one architectural rule: rox works fully offline, and the network only
adds. This is a distinct domain, isolated from playback and library, so a slow or dead
network never blocks the UI, the audio path, or a browse query.

- **Offline-first.** Every enrichment feature degrades to nothing when there's no network.
  Playback, browse, search, and manual tagging never depend on it.
- **Off the hot paths.** Enrichment runs on its own workers behind channels. It never touches
  the real-time audio callback, and it reaches the library and metadata writer through their
  existing contracts (the scrobbler reads the same position clock the listen rule does, an
  auto-tag result goes through the same atomic tag-write path as a manual edit).
- **The pieces exist.** Last.fm scrobbling is a straightforward HTTP client. Auto-tagging is
  `rusty-chromaprint` (pure-Rust fingerprint) plus `musicbrainz_rs`, with a hand-rolled AcoustID
  call. Lyrics is a fetch-or-read-local panel. None of this is load-bearing for the core, so it
  stays a thin, well-isolated domain rather than growing into the system.

These are peripheral, so this section fixes the boundary and the offline-first rule, not the
detail. The point is that enrichment can't be allowed to leak into the domains that must stay
fast and must work offline.
