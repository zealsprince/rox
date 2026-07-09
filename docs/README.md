# rox docs

The rox spec, split into three layers by altitude plus a research track. Product says
what, why, and for whom. Architecture says what the parts are and where the lines
between them run. Implementation is real and runnable: schemas, formats, sequences.
Start with the [Architecture Overview](02-architecture/01-overview.md) for the system
picture, or [Vision](01-product/01-vision.md) for the reasoning behind it.

## Product

- [Vision](01-product/01-vision.md) - The problem, who rox is for, what success looks like
- [Experience](01-product/02-experience.md) - What using rox feels like, in the moments that matter
- [Scope](01-product/03-scope.md) - Core versus peripheral, deliberate exclusions, constraints handed to architecture

## Architecture

- [Overview](02-architecture/01-overview.md) - Inherited constraints, system diagram, the four domains, ADR index
- [Components](02-architecture/02-components.md) - Responsibility, boundary, and contract per component, plus the network enrichment boundary
- [Non-functional model](02-architecture/03-non-functional.md) - Speed at scale, failure and safety, platform, cost
- [ADR 1: GPUI](02-architecture/decisions/01-adr-gpui.md) - GPUI as the UI framework
- [ADR 2: Audio stack](02-architecture/decisions/02-adr-audio-stack.md) - cpal + Symphonia directly, not rodio
- [ADR 3: Gapless](02-architecture/decisions/03-adr-gapless.md) - Single-stream, swap-decoder queue
- [ADR 4: Tagging](02-architecture/decisions/04-adr-tagging.md) - lofty plus an atomic-write layer we own
- [ADR 5: Library store](02-architecture/decisions/05-adr-library-store.md) - SQLite source of truth plus in-memory projection
- [ADR 6: Search](02-architecture/decisions/06-adr-search.md) - In-memory substring first, FTS5 next, tantivy only if needed
- [ADR 7: Panels](02-architecture/decisions/07-adr-panels.md) - GPUI primitives with gpui-component as the widget baseline
- [ADR 8: Visualizer rendering](02-architecture/decisions/08-adr-visualizer-rendering.md) - CPU-side rendering, forced by GPUI
- [ADR 9: Audio output](02-architecture/decisions/09-adr-audio-output.md) - Output layer swappable, bit-perfect deferred

## Implementation

- [Implementation](03-implementation/README.md) - The planned per-domain doc set; each gets written when its detail is real, not before

## Research

- [Research](0R-research/README.md) - Explorations that need a standalone prototype before any decision leans on them

## Shared

- [Open Questions](OPEN-QUESTIONS.md) - The working decision log: what's genuinely undecided
