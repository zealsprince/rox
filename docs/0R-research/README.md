# Research

Explorations with no commitment attached. Each entry needs a standalone prototype and a
writeup before any decision leans on it.

- [01 - Generative visualizer](01-generative-visualizer.md) - the curl-noise flow
  field from [ADR 8](../02-architecture/decisions/08-adr-visualizer-rendering.md),
  prototyped in `crates/rox-prototype-viz`. Both render paths hold the frame budget;
  the per-frame image blit wins on UI-thread cost and on the look.
- [02 - Library scale](02-library-scale.md) - does the
  [ADR 5](../02-architecture/decisions/05-adr-library-store.md) store shape and
  [ADR 6](../02-architecture/decisions/06-adr-search.md) substring search hold at 10
  million tracks, prototyped in `crates/rox-prototype-library`. It holds: worst-case
  search 31 ms, filters single-digit ms, ~1 GB of projection, 1.9 s cold open with
  sharded readers.
