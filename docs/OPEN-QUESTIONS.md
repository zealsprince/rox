# Open questions

Undecided items that need answering before or during implementation. Resolved items are
removed once their resolution lands in the spec; the commit history is the record.

1. **Pop-out on Linux/Wayland** - the multi-window pop-out leans hardest on GPUI's
   platform layer, and Wayland is where the bug budget goes
   ([non-functional model, platform](02-architecture/03-non-functional.md#platform)).
   Needs an early test before the panel system design hardens.
