# ADR 15: Shared query: an app-wide search entity panels opt into per view

**Status:** Decided

Decision: the app-wide query is a `SharedQuery` entity on the app state, shared the same
way `Selection` already is. It publishes, and panels subscribe.

Each searching panel gets a query-source knob on its config with two settings, shared or
own. That follows the `TrackSource` pattern already used in the settings page and the
dropdown, so it's a control users have met before. The follow-and-mirror behavior behind
the knob comes from one `QueryFilter` trait rather than being hand-rolled per panel, and
the library, the grids, and the art shelf all use it.

Shared is the default, so a panel dropped into a layout filters the moment the search box
has text in it, with nothing to configure first. A panel set to shared reads the query
and its own box writes to it, which means every shared box is editing the same value and
every other shared box updates as you type. Switching a panel to its own query gives it a
private filter that nothing else can move, which is the duplicate-with-config story
applied to search. The choice persists per view through the layout dump like the rest of
a panel's config.

That makes a dedicated search panel (#63) a thin shell on the standard panel pattern:
one box bound to the shared query, driving every view that follows it. A library and two
grids can then carry no query of their own while a single box controls all three. It
reuses what search already has rather than adding anything, meaning the projection
substring match from [ADR 6](06-adr-search.md) and the query-syntax completion provider,
which reattaches to the projection on each reload the same way the play launcher's does.

Alternatives: a global query that replaces every panel's local one whenever it's set,
with one writer and the rest as read-only mirrors and no per-panel knob; intersecting the
global query with each panel's local one; a fixed toolbar strip instead of a panel.
Also considered, and shipped before being revised: own-query as the default, which left a
search panel doing nothing at all until each other panel was flipped over to follow it.

Trade: making shared the default lets the search panel work with no setup. It also
inverts which case costs effort. Two grids scoped to different filters used to be the
default arrangement and is now an opt-out, with each grid switched to its own query
individually. That's the right way round once search is a first-class panel, because the
common case is one query across the layout, and anyone who wants the other arrangement
still has it.

The `QueryFilter` trait keeps that default cheap. Three panels share one
implementation, so the follow-and-mirror behavior can't drift into three
slightly-different copies.

Shared editing across boxes is the feature rather than a side effect: the search panel
drives its followers, and any follower's own box edits back into the same value. It costs
one guard. When a mirroring box is filled programmatically because the shared value
changed, that fill must not fire the change event back through the entity, or two boxes
would bounce updates off each other. The entity only publishes when the value actually
changed, and the box only syncs when it has drifted from the shared value, and together
those two close the loop.

Reusing `Selection` and `TrackSource` means no new sharing mechanism to explain and a
knob users already recognize from track sources. Building it as a panel rather than a
toolbar strip keeps the everything-is-a-panel shape of [ADR 7](07-adr-panels.md): it
docks, tabs, pops out, and themes like everything else.

It stays separate from the play launcher, which uses the same query syntax for a
different job. The launcher takes what you type and starts playing the first hit; this
one narrows what the following panels display and starts nothing.
