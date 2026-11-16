# ADR 33: A canvas is a panel that places items in 3D

**Status:** Decided 2026-11-15

Proposal: rox grows a Canvas panel, a composite listed in Add Panel's Arrangement section
beside group, overlay, drawer and slide. A canvas places items in 3D space under one
camera. An item is a grid, a group, an image, text or a shader surface, and the format
leaves room for video and 3D models. Every piece of live UI in a canvas is a grid item,
and a grid is a nested dock tree: a lone panel is a grid holding one panel, and a canvas
inside a canvas is a Canvas panel in a grid. Items can be grouped, and a group's children
take its transform, so layers move and tilt together. A canvas stays inside the rox
window. There's no desktop wallpaper mode.

The window's root stays the dock. A full-window canvas is a dock holding one Canvas panel,
and that needs nothing new: a tab group holding one panel draws no tab chrome
(`rox-dock/src/tab_panel.rs:921-934`). There is no second layout root and no second dump
shape.

Rendering is staged by what gpui can draw today. Images, animated textures and shader
surfaces take real perspective, because `paint_image_quad` already puts an atlas image
on a perspective quad (`vendor/gpui/src/window.rs:3311`, the art shelf's turned covers
at `rox-panels/src/art.rs:1211`). A caller-owned texture or the in-scene shader
primitive on the same kind of face is a gpui patch of that shape. Live UI and text stay
flat to the screen. gpui has no offscreen render of an element subtree, and hit testing
is a rectangle against the raw pointer (`vendor/gpui/src/window.rs:775-797`), so a
tilted panel could be neither drawn nor clicked correctly. A flat item's center is still
projected through the whole chain, which gives it depth order and parallax. A grid keeps
its own size, since live UI can't be scaled. Text reshapes at any size, so its font size
follows depth. Every item stores a full 3D transform regardless, so tilted live panels
can arrive later without changing a saved canvas.

No render pass has a depth attachment, so order is painter's order by depth among
siblings. A group takes one slot in its parent's order and paints its children there, so
two groups never interleave. Two planes that cross each other paint in center order.
Adding a depth buffer is out of this record.

A canvas's saved state is its panel config. Each grid item holds its own dock dump inside
that config, and `PanelState.children` stays empty, because the dock builds every child
eagerly and then discards the result for a panel (`rox-dock/src/state.rs:201-205`), so
children stored there would be built twice. The dump walkers already recurse every
object and array (`rox-core/src/settings.rs:2695-2713`, `:2975-3118`), so the plugins a
layout requires, the shader sources the approval gate checks and the path scrub on
export all reach a nested dump with no change. The walkers that descend a live tree have
to be taught that a canvas hosts docks. Canvases and groups nest to a fixed depth, and past
it an item is kept as saved and not built.

A canvas item can do three things beyond sitting in place, all of them data. It can move
with signals: routes on the canvas's config target `<item id>.<property>`, using the
`Route` type every other signal consumer stores (`rox-viz/src/signal.rs:162-188`). It
can run one command on click, an id from the command table a Custom Controls button
already picks from (`rox/src/keymap.rs`, `COMMANDS`), and a grid ignores it since panels
own their input. And text can show live values: a pattern line with `%field%`
placeholders (`rox-core/src/pattern.rs:128`) over the now-playing fields.

A canvas is edited in place, through an overlay inside the panel built from the vendored
widget kit and rox's own settings rows, with handles drawn from the canvas's own
projection. Settings gets no Canvas section. A tab drags between grids the way it drags
within one dock, which means changing the vendored dock where it assumes one area. A
heavy canvas says so before it's applied or placed and while it's being edited, and the
warning never blocks.

**How the existing records hold.** [ADR 7](07-adr-panels.md) and the rule that every UI
surface is a panel: the canvas is one. It docks, tabs, pops out and themes like the rest,
and a full-window canvas is a dock arrangement.

[ADR 23](23-adr-shader-pipeline.md)'s refusal of a node graph was about shaders, and it
stands. An item tree is placement: a child's transform is its parent's times its own,
and no item reads another's output. A route still runs from the signal pool to one
property, as it does for a shader slot. A shader item is an ordinary rox shader under
the same approval gate.

The scope's "nothing is scripted" line holds. An item has a place, content, one command
id from a fixed list, a text pattern and routes. None of it is code, and none of it can
name anything the command table and the pattern fields don't already expose. Scripting
inside a canvas would change that line, and it would take its own record.

Alternatives: a second layout root beside the dock, a canvas tree the window could hold
in place of a dock. It's the more direct model of "a canvas fills the window", and it
forks every path through restore, dump, persist and each layout walker into two shapes.

The vendored `Tiles` layout as the root (`rox-dock/src/tiles.rs`). It's free placement
that already exists, unused by rox, and it's 2D only: no camera, no depth and no
grouping, so the canvas would be built beside it anyway.

A flat item list with no groups. Projection gets simpler and every item sorts in one
order. Layers that should move and tilt together would each need the same edit and the
same routes, and keeping them in step would be the user's job.

Waiting for offscreen layers before shipping any of it, so live panels tilt from the
start. That's gpui surgery on two GPU paths, blade (which macOS runs as well as Linux,
`rox/Cargo.toml:113-117`) and DirectX 11, and it would hold back the parts that draw
today on the part that doesn't.

Costs. An editor, and it's the largest piece: placing, grouping, handles under
perspective and undo, all inside a panel. Painter's order with no depth test, so
crossing planes and groups that would interleave paint in an order the user can't
change. Nested docks that every live layout walker has to reach, including the ones
written after this. Changes to the vendored dock for a drag between areas, which adds to
the re-diff every gpui-component bump already costs (ADR 7's amendment). And a second
place a dock dump can be stored, inside a panel's config.

**Amended 2026-11-15: a canvas, held to flat items.** Andrew's product calls, made after
weighing what real 3D inside a canvas would take. Three things change.

The panel is renamed Canvas. "Scene" promised a 3D world, and the shader settings
already use the word for a shader that covers its panel
(`rox-i18n/locales/en-CA/rox.ftl`, `panel-shader-is-scene`). This record, the code,
the saved panel name and the strings say canvas.

The canvas composes rox's own things freely: placement, tilt, parallax and painter's
order under one camera. It gets no depth buffer, no mesh pipeline and no second GPU
device, in this record or a later one. The decision above left a depth buffer "out of
this record". It's now out of core. The cost named above therefore stays: planes that
cross each other paint in center order.

"The format leaves room for video and 3D models" is withdrawn. The model item and the
video item both go, and the model asset kind goes with them
([ADR 31](31-adr-workspace-archives.md)'s amendment). No released build has saved
either. A saved canvas that holds one is kept as saved and not built, as any kind the
build doesn't know is (`rox-canvas/src/state.rs`, `Content::Unknown`). The caller-owned
texture on a quad stays (`rox/src/canvas/texture.rs`), with nothing feeding it.

What those items would have shown comes through a render target or not at all:
[ADR 37](37-adr-render-targets.md) has a program outside rox draw the picture and rox
paint its frames on a canvas item. Without a render target, the canvas shows no video
and no 3D, and a WGSL shader item is the furthest it reaches.

The alternative was a renderer in core, either patched into gpui's passes or run on a
device of its own. ADR 37 weighs both. It lost because it's an engine: lighting,
materials, mesh import and post effects, owned by a music player.

**Amended 2026-11-16: item interactivity, with no language.** Andrew's product call,
with [ADR 24](24-adr-script-panels.md) declined. A canvas has no language. This
supersedes the line above that scripting inside a canvas "would take its own record".
When an item's look or visibility follows player state, it does so the way a Custom
Controls button's look does (`rox/src/panels/controls.rs`). The user picks a piece of
state from a fixed table (`rox-panel-api/src/buttons.rs`, `STATES`), the state has a
finite list of cases, and the user sets the item's look, or whether it shows, for each
case from lists. Nothing is written as an expression. A case can hide an item, fade it,
give an image another image or give text another color, and nothing else.
