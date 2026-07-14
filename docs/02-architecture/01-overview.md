# Architecture Overview

How rox is structured, where the boundaries sit, and the trades each choice makes. This
consumes the [product spec](../01-product/) and hands contracts down to
[implementation](../03-implementation/). It does not write the code and it does not
sequence the build.

Status: draft. Grounded in a research pass over GPUI, the Rust audio stack, tagging,
library indexing, and visualizers. Version-sensitive claims were true at research time
and need re-checking before anyone pins a `Cargo.lock`.

## Constraints inherited from product

The requirements this structure has to honor, from [scope](../01-product/03-scope.md):

- All three desktop platforms first-class (Linux, Mac, Windows).
- Fast on a huge library: tens of thousands of tracks, no felt lag on scan, browse,
  search, or tag edit.
- Deep tag editing over that library, safely and in bulk.
- Composable panels that reorder, split, resize, duplicate with independent configs, and
  pop out into real OS windows.
- Themes as tokens, layouts as shareable artifacts, no scripting layer.
- Visualizers as a first-class surface.
- Local-first and fully offline: playback, browse, search, and tag editing never depend
  on the network. Enrichment (scrobbling, tag lookup, lyrics) is allowed on top. Built
  on GPUI.
- Streaming sources are extensions, so track identity is source-qualified and playback
  keeps a command-in, state-out seam a second source engine can sit behind. See
  [source extensibility](#source-extensibility).
- Broad-format local playback and tagging, with MP3 and FLAC as the core formats and
  contracts that stay format-agnostic.

## System overview

rox is one process with four ownership domains that talk over channels, not shared
locks. The split follows the hard constraint that the audio output callback runs on an
OS real-time thread and must never block, allocate, or touch the database or UI.

```
        ┌─────────────────────────── UI domain (GPUI main thread) ───────────────────────────┐
        │  panel shell + dock + pop-out windows   theming   all views (Render)                │
        │  holds Entity handles to shared state, pulls state, never blocks                    │
        └───▲─────────────▲──────────────────▲──────────────────▲──────────────────▲──────────┘
            │ cmd/event    │ query/events     │ read tags/commit │ thumbnail req    │ frames
            │              │                  │                  │                  │
    ┌───────┴──────┐ ┌─────┴────────┐ ┌───────┴───────┐ ┌────────┴───────┐ ┌────────┴────────┐
    │  Playback    │ │  Library     │ │  Metadata     │ │  Artwork       │ │  Visualizer     │
    │  engine      │ │  service     │ │  writer       │ │  service       │ │  subsystem      │
    │              │ │              │ │               │ │                │ │                 │
    │ decode thr.  │ │ SQLite (WAL) │ │ lofty + safe  │ │ thumb SQLite   │ │ FFT analysis    │
    │ + RT output  │ │ + in-mem     │ │ atomic write  │ │ + resize pool  │ │ + waveform      │
    │ callback     │ │ projection   │ │ layer         │ │ + texture LRU  │ │ cache           │
    │ + gapless    │ │ + scanner    │ │               │ │                │ │                 │
    │   queue      │ │ (jwalk+rayon)│ │               │ │                │ │                 │
    └──────┬───────┘ └──────────────┘ └───────┬───────┘ └────────────────┘ └────────▲────────┘
           │ PCM tap (rtrb SPSC ring) ─────────┼──────────────────────────────────────┘
           │                                   │ writes -> reindex request
           └───────────────────────────────────┘
```

The four domains:

- **UI** owns every view and the panel system. It holds GPUI `Entity` handles to shared
  state (current track, playback position, library projection) and re-renders when told.
  It runs on the GPUI main thread and must stay responsive, so anything slow is a message
  to another domain, never a blocking call.
- **Playback** owns the audio pipeline end to end: a decode thread feeding a real-time
  output callback, the gapless queue, ReplayGain application, and the PCM tap that drives
  visualizers. It takes commands (play, pause, seek, enqueue) and emits state.
- **Library** owns the catalog: the SQLite store on disk, an in-memory projection for
  instant browse, the filesystem scanner, and search. It answers queries and emits change
  events.
- **Support services** (metadata writer, artwork, visualizer analysis) are narrower and
  hang off the two big domains.

Each component's responsibility, boundary, and contract is in
[Components](02-components.md). The cross-cutting performance, failure, platform, and
cost model is in [Non-functional model](03-non-functional.md).

## Source extensibility

Product delivers streaming sources (Spotify, YouTube / YouTube Music, Tidal) through
extensions rather than core code ([scope](../01-product/03-scope.md)). The structure
above already has the seams that matter, so honoring the constraint costs two calls,
not a redesign:

- **Track identity is source-qualified.** The library keys tracks by (source, id), and
  local files are the first source. This is the part that's cheap in the initial schema
  and a painful migration to retrofit, and it's what keeps a unified multi-source
  library possible.
- **Playback is already a contract.** Commands in, state out, PCM tap out. A source
  that can hand rox decodable audio (Tidal streams, yt-dlp, librespot's decoded
  samples) feeds the existing engine and gets everything: gapless, ReplayGain,
  visualizers. A source that can't gets remote control, with local output capture as a
  fallback tap so visualizers still work when the audio plays on this machine. The
  visualizer subsystem drains the same ring either way and never knows the difference.

The extension host mechanism (WASM in the style of Zed, or a subprocess model) is
undecided and tracked in [open questions](../OPEN-QUESTIONS.md). No ADR until a
prototype; nothing in the core blocks on it beyond the two calls above.

## Decisions (ADRs)

Each ADR records the call, the alternatives weighed, and what it costs. They live in
[decisions/](decisions/), one file each.

| ADR | Call | Status |
|-----|------|--------|
| [1 - GPUI](decisions/01-adr-gpui.md) | GPUI as the UI framework | Decided |
| [2 - Audio stack](decisions/02-adr-audio-stack.md) | cpal + Symphonia directly, not rodio | Decided |
| [3 - Gapless](decisions/03-adr-gapless.md) | Single-stream, swap-decoder queue | Decided |
| [4 - Tagging](decisions/04-adr-tagging.md) | lofty plus an atomic-write layer we own | Decided |
| [5 - Library store](decisions/05-adr-library-store.md) | SQLite source of truth plus in-memory projection | Decided |
| [6 - Search](decisions/06-adr-search.md) | In-memory substring first, FTS5 next, tantivy only if needed | Decided |
| [7 - Panels](decisions/07-adr-panels.md) | GPUI primitives with gpui-component as the widget baseline | Decided |
| [8 - Visualizer rendering](decisions/08-adr-visualizer-rendering.md) | CPU-side rendering, forced by GPUI | Decided, prototype pending |
| [9 - Audio output](decisions/09-adr-audio-output.md) | Output layer swappable, bit-perfect deferred | Decided |
| [10 - Theming](decisions/10-adr-theming.md) | Palette as data behind one setter, CPU-baked backdrop | Decided |
