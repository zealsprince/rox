# Open questions

Undecided items that need answering before or during implementation. Resolved items are
removed once their resolution lands in the spec; the commit history is the record.

1. **Generative visualizer viability** -
   [ADR 8](02-architecture/decisions/08-adr-visualizer-rendering.md) commits to CPU-side
   rendering because GPUI exposes no custom shader path. Whether a curl-noise flow field
   on a worker thread hits the reference look inside a sane frame budget, and whether
   polylines or a per-frame image blit draws it better, only a prototype answers. This is
   the first thing to build; the writeup goes in [0R-research/](0R-research/).

2. **Pop-out on Linux/Wayland** - the multi-window pop-out leans hardest on GPUI's
   platform layer, and Wayland is where the bug budget goes
   ([non-functional model, platform](02-architecture/03-non-functional.md#platform)).
   Needs an early test before the panel system design hardens.
