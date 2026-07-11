# Non-functional model

How the structure holds up under load and failure, what it demands of the platform, and
what it costs to run and maintain.

## Fast on a huge library

The requirement maps to specific structure, not hope:

- Browse and scroll: GPUI `uniform_list` virtualizes fixed-height rows, so a 100k-track
  view renders only what's visible. Library rows are designed to a fixed height to stay on
  that fast path.
- Sort and filter: the in-memory projection interns repeated strings (artist, album,
  genre) to integer symbols and keeps per-order sorted index vectors, so a re-sort reorders
  `u32` indices against precomputed keys, never moving track data or comparing strings.
  Measured in [research 02](../0R-research/02-library-scale.md): tens of milliseconds at
  1M tracks, a quarter second at 10M, so index builds run off the UI thread.
- Cold open: first paint comes from a view snapshot persisted at close (the visible
  slice of the last sort order, resolved to display strings, a few hundred KB), so the
  window shows the library where it left off before SQLite is touched. The projection
  loads behind it and swaps in: from an on-disk snapshot of its flat arrays when the
  store's generation counter matches, otherwise rebuilt with one SQLite reader per core
  over disjoint rowid ranges (WAL allows concurrent readers; 1.9 s at 10M tracks).
- Scale envelope: the resident projection is measured to 10M tracks, worst-case search
  31 ms and filters in single-digit milliseconds at ~1 GB of RAM
  ([research 02](../0R-research/02-library-scale.md)). The design ceiling is around 50M:
  past that, in-memory scans stop fitting and search and sort move to disk indexes
  (ADR 6's FTS5/tantivy escalation, persisted sort orders) behind the same
  browse/search contract, which is what keeps that swap invisible to the UI.
- Scan: `jwalk` walks the tree in parallel, `rayon` parses tags across cores with lofty.
  Incremental rescan uses a tiered check, size and mtime as a cheap gate, content hash only
  on the candidates that changed.
- Album art: 256px thumbnails generated once with `fast_image_resize`, stored in a
  dedicated SQLite thumbnail DB, served through a bounded worker pool behind a bounded
  texture LRU. Scrolling the grid never re-decodes full-res art.

## Failure and safety

- Real-time audio: the output callback is memcpy-only. Ring buffers are pre-allocated. This
  is a hard invariant, not a guideline, a lock or allocation there is an audible glitch.
- Tag writes: non-atomic at the lofty layer, so the metadata writer's copy-verify-rename is
  the safety boundary. Per-file panic isolation keeps one malformed file from killing a
  batch.
- Filesystem watching: `notify` is not fully reliable, it drops events under load and can
  blow the inotify watch limit on deep trees. So filesystem events are treated as hints to
  re-stat, never as authoritative diffs, with a fallback to polling when the watch limit is
  hit and a periodic full incremental rescan as the self-healing backstop.
- Library sync: the in-memory projection, SQLite, and the filesystem are reconciled through
  the tiered stat-then-hash check plus that periodic rescan, so any missed event heals on
  the next pass.

## Platform

- macOS is the solid target, Windows is acceptable with testing, Linux/Wayland is where the
  bug budget goes, especially the multi-window pop-out, which leans hardest on the platform
  layer. Opening multiple top-level windows on Wayland works (the shell keeps a New Window
  action so this stays exercised); the exposure that remains is pop-out behavior once
  panels exist, window placement and cross-window drag, where Wayland gives clients the
  least control.
- GPUI renders through Vulkan on Linux and Windows, so a Vulkan-capable driver is a hard
  runtime dependency worth stating to users.
- GPUI's pre-1.0 churn is a standing tax: pin the exact version, treat upgrades as work,
  and keep the three GPUI distributions straight (upstream `gpui`, the CE fork, and
  `gpui-component`), since API snippets circulating online often come from the forks.

## Cost and operability

No server, so cost is the desktop footprint and the maintenance tax. The footprint is
modest: tens of MB for the library projection at 100k tracks (about 1 GB at the 10M
extreme), a bounded texture cache, playback buffers.
The maintenance tax is the real cost, several pre-1.0 dependencies (GPUI, gpui-component,
parts of the audio stack) that will break across upgrades. That's the price of building on
the cutting edge, and it's a standing line item, not a one-time setup.
