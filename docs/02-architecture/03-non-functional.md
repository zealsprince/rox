# Non-functional model

How the structure holds up under load and failure, what it demands of the platform, and
what it costs to run and maintain.

## Fast on a huge library

The requirement maps to specific structure:

- **Browse and scroll.** gpui's `uniform_list` virtualizes fixed-height rows, so a
  100k-track view only renders the rows on screen and render cost stops tracking library
  size. Library rows are designed to a fixed height so they stay on that path, since a
  variable-height row would force the list to measure everything.
- **Sort and filter.** The in-memory projection interns repeated strings (artist, album,
  genre) down to integer symbols, and keeps a sorted index vector per order. A re-sort
  therefore reorders `u32` indices against precomputed keys, without moving any track
  data or comparing a single string. Measured in
  [research 02](../0R-research/02-library-scale.md): tens of milliseconds at 1M tracks and
  about a quarter second at 10M, which is why index builds run off the UI thread.
- **Cold open.** First paint doesn't wait for the library at all. A view snapshot is
  persisted at close: the visible slice of the last sort order, already resolved to
  display strings, a few hundred KB in total. The window comes up showing the library
  where it was left before SQLite has been touched. The projection loads behind that and
  swaps in when it's ready. If the store's generation counter says nothing has changed,
  it's read from an on-disk snapshot of its flat arrays. Otherwise it's rebuilt with one
  SQLite reader per core over disjoint rowid ranges, which WAL allows because it supports
  concurrent readers. That rebuild is 1.9 s at 10M tracks.
- **Scale envelope.** The resident projection is measured to 10M tracks: worst-case search
  31 ms, filters in single-digit milliseconds, about 1 GB of RAM
  ([research 02](../0R-research/02-library-scale.md)). The design ceiling is around 50M,
  where in-memory scans stop fitting in a reasonable amount of memory. Past that, search
  and sort move to disk indexes, meaning ADR 6's FTS5 or tantivy escalation and persisted
  sort orders. Because both are behind the same browse/search contract, that swap is
  invisible to the UI.
- **Scan.** `jwalk` walks the tree in parallel and `rayon` parses tags across cores
  through lofty. Incremental rescan uses a tiered check, with size and mtime as a cheap
  first gate and a content hash run only on the files that gate flags, so an unchanged
  library costs a stat per file rather than a read.
- **Album art.** 256px thumbnails are generated once with `fast_image_resize` and stored
  in a dedicated SQLite thumbnail database, then served through a bounded worker pool
  behind a bounded texture LRU. Scrolling the grid never re-decodes full-resolution
  art.

## Failure and safety

- **Real-time audio.** The output callback is memcpy-only and every ring buffer is
  pre-allocated. This is a hard invariant rather than a preference, because a lock or an
  allocation on that thread can block past the device's deadline, and a missed deadline is
  an audible glitch.
- **Tag writes.** lofty writes in place and isn't atomic, so the metadata writer's
  copy-verify-rename sequence provides the safety. Panic isolation is
  per file, so one malformed file that trips lofty's parser costs that file rather than
  the batch it was in.
- **Filesystem watching.** `notify` can't be trusted as a complete record: it drops events
  under load, and on a deep tree it can exhaust the inotify watch limit. So a filesystem
  event is treated as a hint to go and re-stat something, never as an authoritative
  statement of what changed. When the watch limit is hit the watcher falls back to
  polling, and a periodic full incremental rescan runs underneath everything as the
  backstop that heals whatever was missed.
- **Library sync.** The projection, SQLite, and the filesystem are reconciled through that
  same tiered stat-then-hash check plus the periodic rescan, so a missed event costs
  staleness until the next pass rather than permanent drift.

## Platform

- **Where the bug budget goes.** macOS is the solid target and Windows is acceptable with
  testing. Linux under Wayland is where the problems are, particularly multi-window
  pop-out, since that leans harder on the platform layer than anything else in the app.
  Opening several top-level windows on Wayland works today, and the shell keeps a New
  Window action partly so that stays exercised. What's still exposed is the behavior
  around pop-out rather than pop-out itself: window placement and cross-window drag.
  Wayland gives clients the least control over both.
- **Each platform has its own GPU floor.** gpui renders through blade on Vulkan on Linux,
  through blade on Metal on macOS (the `macos-blade` feature, see
  [ADR 8](decisions/08-adr-visualizer-rendering.md)), and through its own Direct3D 11
  renderer on Windows. A Vulkan-capable driver is required on Linux, and a Direct3D 11
  device at feature level 10.1 or above on Windows. Both are worth stating to users up
  front rather than discovering at launch. Shader surfaces need shader model 5.0 on
  Windows and hide themselves on a device below it. The Milkdrop panel adds an OpenGL
  context of its own, and a machine that can't provide one gets a message in that panel
  rather than a dead app ([ADR 28](decisions/28-adr-milkdrop.md)).
- **gpui's pre-1.0 churn is a standing tax.** Pin the exact version, treat every upgrade
  as real work, and keep the three gpui distributions straight: upstream `gpui`, the CE
  fork, and `gpui-component`. That last point matters more than it sounds, because API
  snippets circulating online frequently come from the forks and won't compile against
  what we pin.

## Cost and operability

There's no server, so the cost is the desktop footprint plus the maintenance tax. The
footprint is modest: tens of MB for the library projection at 100k tracks, rising to about
1 GB at the 10M extreme, plus a bounded texture cache and the playback buffers.

The maintenance tax is the part that actually costs something. Several pre-1.0
dependencies are load-bearing here, including gpui, gpui-component, and parts of the audio
stack, and they break across upgrades. That makes maintenance a line item on every
release cycle rather than one-time setup work.
