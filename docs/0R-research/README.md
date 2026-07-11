# Research

Explorations with no commitment attached. Each entry needs a standalone prototype and a
writeup before any decision leans on it.

- [01 - Generative visualizer](01-generative-visualizer.md) - the curl-noise flow
  field from [ADR 8](../02-architecture/decisions/08-adr-visualizer-rendering.md),
  prototyped in `crates/rox-prototype-viz`. Both render paths hold the frame budget;
  the per-frame image blit wins on UI-thread cost and on the look.
