# ADR 15: Shared query: an app-wide search entity panels opt into per view

**Status:** Decided

Decision: the app-wide query lives in a `SharedQuery` entity on the app state,
shared the way `Selection` already is - it publishes, panels subscribe. Each
searching panel carries a query-source knob on its config, shared or own,
following the `TrackSource` pattern in the settings page and the dropdown, and
gets the follow-and-mirror behavior from one `QueryFilter` trait rather than
hand-rolling it - the library, the grids, and the art shelf all ride it. Shared
is the default, so a fresh panel filters the moment the search box has text, no
per-panel setup. A panel set to shared mirrors the query and its own box writes
it, so any shared box edits the one value and every other shared-following box
follows live; a panel switched to its own query keeps a private, per-view filter,
the duplicate-with-config story. The source choice persists per view through the
layout dump like other config. A dedicated search panel (#63) is then a shell on
the standard panel pattern: one box bound to the shared query, driving every view
that follows it, so a library and two grids can sit query-less while one box
controls them all. It reuses what search already has - the projection substring
match of ADR 6 and the query-syntax completion provider, reattached to the
projection on each reload the way the play launcher does.

Alternatives: a global query that replaces every panel's local one whenever it is
set, with one writer and the rest read-only mirrors and no per-panel knob;
intersecting the global query with each panel's local one; a fixed toolbar strip
instead of a panel.

Trade: shared-by-default makes the search panel work with no setup - drop it in
and typing filters every other panel - at the cost that two grids scoped to
different filters is now an opt-out, each grid switched to its own query, rather
than the default. That's the right trade once search is a first-class panel: the
common case is one query across the layout, and the duplicate-with-config story
still holds for anyone who wants it. The `QueryFilter` trait is what makes the
default cheap to carry, since three panels share one implementation instead of
three copies drifting apart. Shared edit across boxes is the point of the feature
- the search panel drives its followers and any follower's own box edits back -
and it costs one guard: a programmatic fill of a mirroring box must not re-fire
the change back through the entity, which the entity's set-if-changed and the
box's sync-on-drift together enforce. Riding the `Selection` and `TrackSource`
patterns means no new sharing mechanism and a knob users already understand from
track sources. A panel, not a toolbar, keeps the everything-is-a-panel shape of
ADR 7 - it docks, tabs, pops out, and themes like the rest. Separate from the
play launcher on purpose: that box speaks the same query syntax but launches
playback of the first hit, where this one narrows what the following panels show
and plays nothing.

Alternatives also considered: own-query as the default (the shipped-then-revised
choice), which left the search panel doing nothing until each panel was flipped
to follow it.
