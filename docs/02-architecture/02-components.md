# Components and contracts

Per-component responsibility, boundary, and contract, within the domain split laid out
in the [overview](01-overview.md).

## Playback engine

Responsibility: turn a queue of tracks into sample-accurate audio, and expose a live PCM
tap. Owns decode (Symphonia), output, the gapless queue, the processing chain,
volume/ReplayGain, and the tap ring.

Boundary: the real-time output callback is the hard line. It only reads from a
pre-allocated ring buffer and writes to the device. No allocation, no lock, no logging,
no database. Everything else in the engine runs on a normal decode thread behind that
line.

Formats: the whole Symphonia codec and container matrix, taken wholesale because every
one of them is pure Rust and none costs a C dependency (FLAC, MP3, AAC, ALAC, Vorbis,
PCM and friends, in wav, ogg, mkv, mp4, caf, aiff). The contract is format-agnostic, so
adding a format is additive. Opus was the one gap Symphonia 0.6 left, and it's closed
with `opus-pure` rather than through C: Symphonia's Ogg reader still parses the stream
and only the decoder is swapped in. Multistream Opus (more than two channels) refuses to
open and says why.

Processing ([ADR 19](decisions/19-adr-processing-chain.md)): per-source gain (ReplayGain,
the crossfade curve) multiplies each decoded source on its own, then the chain of DSP
nodes processes the mixed stream, both on the decode thread immediately before the ring.
Crossfade isn't a node. It's a second open source summed in the engine, so the ring
keeps one producer. With the chain empty and volume at unity the device gets the
decoder's samples unchanged, which makes bit-perfect a claim you can check.

Output: one backend contract, a request in and what it negotiated back (mode, rate,
format). cpal implements it for shared mode. Exclusive mode is per-platform (ALSA `hw`
direct, WASAPI exclusive, CoreAudio hog mode) and follows the source rate where the
device allows. A claim that fails opens shared and reports the reason, never silence.

Contract to the UI:
- In: `play`, `pause`, `seek(pos)`, `next`, `prev`, `enqueue(track)`, `set_volume`,
  `set_loop`, `set_shuffle`, `set_output_device`, `set_crossfade`, `set_gain_rule`, and
  structural chain edits. Commands cross a channel rather than calling into the RT thread.
  Node parameters don't: a knob is an atomic shared with the UI, so turning one is a
  store the node reads on its next buffer.
- Out: playback state (current track, position, playing/paused, device), emitted as the
  UI's shared entity updates so views re-render on the next frame.
- Out: the PCM tap, a second SPSC ring the visualizer drains. Lossy: a slow UI drops
  stale samples rather than back-pressuring audio.

## Library service

Responsibility: hold the catalog, keep it fast, keep it current. SQLite is the durable
source of truth and the write path. A full in-memory projection is the read path that
makes browse, sort, and filter instant. Track identity is source-qualified: the key is
(source, path, sub) and the id is the rowid behind it. Local files are the first source,
so source extensions extend the catalog instead of forcing a migration (see
[source extensibility](01-overview.md#source-extensibility) and
[ADR 29](decisions/29-adr-source-contract.md)). The projection stores the source column
too, so a view can filter on it without going back to SQLite.

Boundary: browsing never touches SQLite. The UI reads the shared in-memory projection
and derives its views from it; paths stay in the store, so playing a row costs one
id-to-path read back through the service. Consistency is by rebuild: a scan, an explicit
reload, a removal, or a prune rebuilds the projection from SQLite and swaps it whole. The
one exception is a filesystem watch event or a reindex, which patches the live projection
with just the rows it touched (append plus tombstone) and leaves the next rebuild to
compact. The rebuilt projection is the reference state either way; a patch is only a
cheaper route to what a rebuild would produce.

Contract to the UI:
- In: `rescan(root)`, `watch(on/off)`, and `paths_for(ids)`, the id-to-path hop that
  playback and selection resolve through.
- Out: the projection, shared read-only, that browse order, search, filter, and sort
  derive from, and a change event per swap so open views refresh together.

Contract to the metadata writer: after a successful tag write, the library applies the
committed changes to its rows and reloads the projection, so a tag edit and the browse
view converge without a full rescan. Fields the projection contains update at once;
fields it doesn't (comment, composer) reflect on the next rescan.

## Play history

Responsibility: turn playback into a durable record of listens and answer the stat
queries panels ask: play count and recency per track, rolled up by artist,
album, and genre. Product hands down the shape ([scope](../01-product/03-scope.md)):
events with timestamps keyed to track identity, never bare counters, because every
future stat derives from the raw record.

Boundary: nothing here touches the audio path. The playback engine already emits state
(current track, position, transitions); play history consumes that state on the control
side, applies the listen rule, and appends to the store off the UI thread. The listen
rule matches the scrobble standard, half the track or four minutes of it, whichever
comes first. Storage is the library database per
[ADR 11](decisions/11-adr-play-history.md). Aggregates are derived from events. Stats
are read when a view opens rather than per keystroke, so they stay in SQL rather than
the projection.

Contract:
- In: playback state transitions (track opened, position advanced, track
  ended or skipped), and the track's identity from the library.
- Out: one listen event appended per real listen, stat queries (per-track count and
  last-played, artist / album / genre rollups, recents), and a change event per append
  so open views refresh.
- To enrichment: the scrobbler accrues played time off the same position clock, so
  seeks and pauses don't count for either. Every scrobble destination (Last.fm,
  Libre.fm, ListenBrainz) sends on one shared threshold, a user knob, rather than on
  the listen rule.

## Metadata writer

Responsibility: read and write tags across the format matrix, safely, in bulk. Wraps
lofty with an atomic-write layer, because lofty rewrites files in place and a crash
mid-write can leave a file unrecoverable. The core formats all write through lofty:
ID3v2 for MP3, Vorbis comments for FLAC, `ilst` atoms for MP4.

Boundary: this is the only component that writes audio files. Every write goes through
copy, verify, atomic rename. Reads are isolated per file so a malformed file that panics
lofty's parser takes down one worker, not the batch.

Contract:
- In: `read(path)`, `commit(path, changes)`, `commit_batch(edits)`.
- Out: per-file success or failure, never a partial corrupt file. The library service
  applies the committed changes to its rows on success.
- Custom and arbitrary tag fields go through lofty's format-specific tag types (ID3v2
  TXXX, MP4 freeform atoms, Vorbis keys), not the generic key abstraction, which has no
  slot for unknown keys and can drop them.

## Artwork service

Responsibility: supply the album-art grid without stalling the scroll. Generates 256px
thumbnails once, caches them, and returns decoded textures to the UI.

Boundary: everything here is bounded, because the obvious implementation isn't. Spawning
a load per visible tile means a fast scroll through a large grid queues thousands of
loads for art nobody is looking at any more. So there are two bounded layers instead. A
worker pool of fixed size loads and resizes thumbnails out of a dedicated SQLite
thumbnail database, and a bounded LRU of already-decoded textures is checked before that
pool is asked for anything. The LRU is sized to the viewport plus a margin rather than to
the library, so its memory cost doesn't grow with the collection.

Contract:
- In: `thumbnail(key, size)` where key is content-addressed (path + mtime + size).
- Out: a texture handle, or a placeholder plus a pending load. Off-screen requests cancel.
- A catalog change marks the texture cache stale rather than clearing it. A stale entry
  is still served while it re-reads in the background. Only a cover whose bytes changed
  swaps, so a track added to a watched folder never blanks the wall.

## Visualizer subsystem

Responsibility: turn the playback PCM tap into everything the visual surfaces read. Owns
the FFT analysis, the per-track waveform cache, and the signal pool that shaders route
from.

Three rendering paths read from it, and they differ in what does the drawing:

- Spectrum and waveform draw as gpui primitives in a paint callback, a handful of shapes
  per frame ([ADR 8](decisions/08-adr-visualizer-rendering.md)).
- User WGSL runs on the GPU through the vendored gpui shader API, as a whole-window post
  pass, a per-panel surface, or the Shader panel's whole body, composed into pass chains
  ([ADR 23](decisions/23-adr-shader-pipeline.md)).
- MilkDrop presets render in a libprojectM worker with its own GL context and come back
  as a read-back buffer the panel uploads as a texture
  ([ADR 28](decisions/28-adr-milkdrop.md)).

Boundary: the real-time line is the tap ring. The callback pushes into it and never
waits, a timer on the player service drains it into the shared feed rather than any
render pass, and a slow consumer loses samples instead of slowing audio. Past the ring
the work splits by what it reads. Windowed analysis (the spectrum's bars, the
spectrogram's columns, the signal hub's bands) runs inline in the paint of the view that
shows it. Its cost is bounded by the window size rather than the track, and a view that
isn't painting costs nothing. The transform itself belongs to the feed: it runs once per
window size each time the feed moves, and every view asking for that size gets the same
spectrum. The signal hub is bound to its player's feed and advances when it's read, so
whatever reads a signal also keeps the hub's clock running. Anything that reads a whole
file, like the waveform peaks or the frame primed for a paused start, runs on the
background executor. The projectM worker has a thread of its own, and one that can't get
a GL context reports a failure status instead of taking the app down.

Contract:
- In: the PCM tap ring, plus the current track for waveform precompute.
- Out: analysis frames (spectrum bands, recent samples) the UI draws, a cached min/max
  peak waveform per track (a few KB, keyed on file identity: path, size, mtime), and the
  signal pool a shader's sixteen slots route from.

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
  Neither contains executable behavior.
- Settings split by scope: an app settings window edits the app-wide state, and a
  per-panel customize window edits that panel's config. The app-wide half is split again
  on disk by what each file is for, so preferences travel between machines while window
  geometry, playback state, and credentials stay put
  ([ADR 20](decisions/20-adr-settings-split.md)). Per-view state is stored in panel
  config: columns, sort, density, theme overrides, and the search query (entered through
  one shared box component). Duplicated panels diverge, and a layout stores all of it.

## Network enrichment boundary

Scrobbling, tag lookup / auto-tagging, and lyrics all use the network to enrich a local
library. They share one architectural rule: rox works fully offline, and the network only
adds. This is a distinct domain, isolated from playback and library, so a slow or dead
network never blocks the UI, the audio path, or a browse query.

- **Offline-first.** Every enrichment feature degrades to nothing when there's no network.
  Playback, browse, search, and manual tagging never depend on it.
- **Off the hot paths.** Enrichment runs on the background executor, off the UI and audio
  threads. It never touches the real-time audio callback. It reaches the library and
  metadata writer through their existing contracts: the scrobbler reads the same position
  clock the listen rule does, and an auto-tag result goes through the same atomic
  tag-write path as a manual edit.
- **The pieces exist.** Last.fm scrobbling is a straightforward HTTP client. The rest are
  per-domain providers per [ADR 14](decisions/14-adr-online-providers.md): lyrics, tag
  lookup, and cover art, each matching the track's own tags against a service, ranking
  the results, and writing the picked one through the existing paths. Fingerprint
  identification (`rusty-chromaprint` plus an AcoustID lookup) covers the files whose
  tags are missing or wrong. None of this is load-bearing for the core, so it stays a
  thin, isolated domain.
