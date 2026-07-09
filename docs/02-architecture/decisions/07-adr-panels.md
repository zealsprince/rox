# ADR 7: Panel/dock: build on GPUI primitives, adopt gpui-component as the widget baseline

**Status:** Decided

Decision: adopt `gpui-component` (longbridge) for the heavy widgets it already solves,
virtualized tables, image elements, and its dock, and build our own layer for the two
things it doesn't give us: duplicate-a-panel-with-config and pop-out into an OS window.

Alternatives: vendor and strip down Zed's `workspace::dock`, or roll the entire panel
system on raw GPUI primitives.

Trade: Zed's dock is proven but coupled to its `workspace` concept and comes with editor
baggage; extracting it is real work. Rolling everything from scratch is the most control
and the most code. `gpui-component` is a permissively-licensed, actively-maintained library
built for exactly this, and it collapses a lot of the widget work, but it's another pre-1.0
dependency tracking GPUI's churn, and its dock is in-window only. Our pop-out requirement is
a separate mechanism regardless of which dock we pick (GPUI multi-window with shared
entities), so we build that ourselves either way. The call: take the acceleration, own the
two panel behaviors that are core to the product, and be ready to vendor the dock if we
outgrow it.
