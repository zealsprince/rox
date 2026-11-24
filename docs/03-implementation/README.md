# Implementation

Real and runnable: schemas, serialization formats, exact sequences, thread and channel
wiring, config. These docs consume the contracts in [architecture](../02-architecture/)
and make them concrete. Nothing here gets to move a boundary; when a contract doesn't
hold up in implementation, that goes back up to architecture rather than getting quietly
redesigned here.

An implementation doc gets written when its detail is real, prototyped or built, not
speculated ahead of the code. The set, one per domain:

- [01-playback.md](01-playback.md) - decode thread and RT callback wiring, ring buffer
  sizing, the gapless boundary swap and LAME delay/padding trimming, the flush protocol,
  the position clock, the shared and exclusive output backends, crossfade, and where
  ReplayGain and the processing chain go in the sample path
- [02-library.md](02-library.md) - the SQLite schema and its migration ladder, the
  in-memory projection layout and interning, the scanner pipeline, the sharded cold-open
  load, and the rebuild-and-swap sequence that keeps store and projection consistent
- `03-metadata.md` - the copy-verify-rename sequence step by step, per-format tag field
  mapping (ID3v2 / Vorbis / MP4 atoms), batch semantics and failure shapes
- `04-artwork.md` - thumbnail DB schema and content-addressed keying, worker pool and
  texture LRU budgets, cancellation
- [05-visualizer.md](05-visualizer.md) - the PCM tap and the analysis feed, the FFT and
  band mapping, the binary peaks cache format, and the frame pacing between the feed and
  the paint callback
- [06-panels.md](06-panels.md) - the layout tree and its JSON serialization, the panel
  config model and shared chrome, the customize windows, pop-out mechanics and entity
  sharing, and how workspaces bundle it
- `07-workspace.md` - crate layout, build commands, CI, the gpui version pin policy
- `08-play-history.md` - the events schema and tag snapshot, listen-rule wiring against
  the position clock, rollup queries and their indexes
- [09-i18n.md](09-i18n.md) - key conventions and the ftl layout, the extraction moves
  per kind of string, ICU formatting helpers, adding a locale, and the pseudo-locale
  check
- [10-plugins.md](10-plugins.md) - the plugin folder and its hash, the manifest and what
  refuses one, the wire with each method's request and answer, timeouts and caps, the
  process lifecycle and per-OS spawn, how a plugin's streams reach the engine and its
  rows enter the library, and the approval switch
- [11-renderers.md](11-renderers.md) - the renderer package and its process, the frame
  mapping byte for byte, the counter a renderer writes, the lines in both directions, the
  limits that stop one, and the two examples
