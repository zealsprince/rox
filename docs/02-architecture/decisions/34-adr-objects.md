# ADR 34: Objects carry one canvas item and what it needs

**Status:** Decided 2026-11-15

Proposal: an object is a `.roxo` file, a zip holding `object.json` and a flat `assets/`
folder in the shape a `.roxw` uses ([ADR 31](31-adr-workspace-archives.md)). The object
is one canvas item ([ADR 33](33-adr-canvas-panel.md)), often a group, lifted out of a
canvas with everything it needs to work in another one: the assets it names, the pool
shaders its shader content uses, the routes aimed at it and the pool signals those
routes read. It carries the same card a workspace does (`WorkspaceMeta`), and it names
the plugins its grids need without carrying any, as a workspace does through `requires`.

Installing an object copies it into an `objects/` folder in the data directory and
places nothing. Placing one adds it to a canvas, and placing merges into the workspace's
pools. It never replaces anything already there. An asset whose sha256 is already in the
live store is reused. A shader or signal whose name is taken gets a suffix. Every item
id inside the object is made fresh, and the routes are re-aimed at the new ids. A
signal's id is local to the object: the pool hands back its own id when the signal is
added (`SignalHub::add`, `rox-viz/src/signal.rs:574`), and the routes are rewritten to
it.

A shader the object brings is unapproved until the user approves it, and renders as off
until then, as it would on any panel. The place confirm lists what needs approval, the
way a workspace's apply confirm does. An object opened from outside rox is copied into
`objects/` and is offered to the canvas that has focus. An object written by a newer rox
is refused with a line that says so. Export scrubs local paths, as workspace export
already does.

Alternatives: share only whole workspaces. That's the format that already exists, and
applying one replaces the look wholesale, so taking one clock or one badge from
someone's look means losing your own.

An object as a small workspace of its own, a `.roxw` holding one canvas. It reuses every
reader and writer. A workspace applies by replacing its pools and its layout, though, so
placing one would need a second apply mode that merges, inside a path whose whole
contract is replacement (ADR 20), and one file type would mean two different things
depending on where it was opened.

Objects that reference shared assets by hash and carry none themselves. Files would be
smaller and two objects using one image would share it. An object would then arrive dead
on any machine that doesn't already hold its bytes, which is the reason [ADR
23](23-adr-shader-pipeline.md) gave for carrying assets inside the bundle at all.

Costs. A third file type, with its own extension, association and confirm. Merge rules
for three pools (assets, shaders and signals), each with its own idea of a clash. Ids
rewritten on the way in, which means every reference inside the object has to be found
and changed, and a placed object's shader and signal names can differ from the ones its
author saw. Script associations aren't part of an object, since scripts aren't part of a
canvas.
