# ADR 37: A render target shows frames another program draws

**Status:** Decided 2026-11-16; nothing built or planned yet

Decision: rox grows a render target, a surface whose picture is drawn by a program
outside rox. rox starts the program, sends it the signal pool's values and the input
that reaches the surface, and paints the frames it sends back. A render target is a
canvas item ([ADR 33](33-adr-canvas-panel.md), as amended), so rox composites its own
items over and under it in painter's order, and one canvas can hold several. The program
is called a renderer. It ships, is approved and is supervised the way a plugin is
([ADR 30](30-adr-plugins.md)), under a new capability. rox publishes the contract and a
neutral example renderer. It builds no 3D renderer of its own.

**Why.** ADR 33's amendment holds the canvas to flat items in painter's order with no
depth buffer. That covers composing a window. It doesn't cover a visualization made of
meshes, particles and lights, which needs a depth-tested render pass, materials and
asset import. Building that in core means a second GPU pipeline and everything on top
of it, maintained inside a music player. Andrew's call on 2026-11-15 was that this
isn't rox's place, and that people should still be able to take a visualization as far
as they want. A render target gives them a surface and the music's signals, and leaves
the drawing to a program they choose.

**What crosses the boundary.** Outward, rox sends the values of the signal pool every
route already reads (`rox-viz/src/signal.rs`), the surface's size and scale, and
pointer and key events in the surface's own coordinates while it has focus. gpui
delivers those events to the item as it does to any element, so the renderer never
touches the window. A target on a tilted item needs the pointer mapped back through the
canvas projection first.

Inward, the renderer sends frames. The path from a frame to the screen exists: a
caller-owned texture on an item's quad (`rox/src/canvas/texture.rs`), fed the way
Milkdrop's worker feeds its panel ([ADR 28](28-adr-milkdrop.md)). The plugin wire can't
transport the frames. It's newline-delimited JSON with frames capped at 1 MiB
(ADR 30, `rox-ipc/src/server.rs:21-22`), and one 1920x1080 RGBA frame is about 8 MB.
Frames need a second channel beside the wire.

**What a render target can't do.** A rox item can't be placed inside one, and two
targets never share a world. A rox item is wholly in front of a target or wholly behind
it. The editor sees one opaque item, with no handles on anything the renderer draws.
What a click inside the surface does is up to the renderer.

**How the existing records hold.** This record reverses a line in ADR 30 and narrows a
refusal in the scope doc. Both are product decisions, and Andrew made them with this
one.

ADR 30 says a plugin "never changes how rox itself behaves", and gives the example: "A
visualizer is the clearest case of what's out, since rendering is something rox does
rather than something it fetches." A renderer is that visualizer. The mechanism behind
ADR 30's line still holds: the picture is drawn in another process and arrives as
pixels, the way a source's audio arrives as bytes. Nothing a renderer supplies executes
in rox's process, joins the element tree or touches the engine. The capability set is
closed and "adding a capability is a product decision", so ADR 30 has an amendment for
it, decided with this record.

The [scope doc](../../01-product/03-scope.md) puts "Scripted theming or UI extensions"
out of scope, on the fragility of scripted panels, and promises that a layout is an
artifact a person can hand to someone else and have work. A renderer is third-party
code that decides what a region of the window shows. Its failure is contained where a
scripted panel's wasn't: a renderer that crashes leaves an empty surface, as a plugin
that crashes ends its own track (ADR 30). The promise about layouts weakens. A workspace
holding a render target works only where its renderer is installed. The apply card can
name the renderer through `requires`, as it does for plugin panels (ADR 30).

ADR 30 and [ADR 22](22-adr-control-surface.md) keep plugins off the control socket, and
that stands. A renderer that wants a click to skip a track needs a verb into rox. ADR 30
already reserved the route for one: "a typed allowlist in front of the socket's route
arms." Whether renderers get verbs, and which, is open below.

[ADR 8](08-adr-visualizer-rendering.md) refused a CPU readback as a standing tax and
ADR 28 accepted one for what it bought. A render target pays the same tax for the same
kind of reason: the shader path can't draw the picture. Milkdrop's measured readback is
1.7 ms a frame at 1920x1080 and 60 fps (`docs/03-implementation/05-visualizer.md`). A
render target adds a process boundary to that, unmeasured.

[ADR 2](02-adr-audio-stack.md) keeps decode in core free of C. A renderer that plays
video holds its decoder in its own process, so core takes no video decoder.

[ADR 35](35-adr-package-security.md) treats a plugin as code behind the approval gate,
and a renderer is gated the same way. A frame is bytes from a program rox doesn't trust,
so its size is checked against the dimensions rox asked for before it's uploaded.

**Alternatives.** A renderer in core. Either a mesh pipeline and a depth attachment
patched into gpui's main pass on blade and DirectX 11, or a separate offscreen pass on
a second GPU device (`wgpu` isn't in `Cargo.lock`). Both give per-pixel occlusion
between rox's own items, which a render target never will. Both are an engine rox would
own: lighting, materials, mesh import, instancing and post effects, written and kept
working on three platforms.

3D models as items, each rendered into its own plane. The 2.0 plans pinned this shape
before ADR 33's amendment withdrew it. A model drawn into a texture and then tilted with
its item is projected twice, and two models can't occlude each other, so it reads as a
picture of a model. It would have cost a renderer and delivered less than a render
target does.

An in-process native library. ADR 30 turned this down for plugins: Rust has no stable
ABI, and a crash takes the audio engine with it. Zero-copy frames change neither point.

A window of the renderer's own. It needs nothing from rox beyond the signals. It can't
dock, tab or sit under rox's items, and those are the reasons to want a surface.

Nothing. WGSL shader items stay the ceiling. Raymarching in a fragment shader still
reaches some 3D looks under the existing approval gate.

**Costs.** Every frame crosses the CPU twice, once out of the renderer's GPU and once
into rox's, until the two share a GPU texture. Sharing one is a gpui patch on each
backend.

A workspace with a render target isn't self-contained, and sharing a visualization
means sharing a program that runs with the user's permissions. A WGSL shader can't
touch the system, and a renderer can.

Key events go to a third-party program while its surface has focus. The enable card
has to say so.

A renderer's author builds for each platform or relies on an interpreter the user has,
as any plugin author does (ADR 30, Platforms).

The editor can't see inside a target. The inspector can only show what a renderer
declares about itself.

**Open.**

- The frame channel. Shared memory with the wire announcing each frame is the
  candidate. Nothing is measured.
- Where the capability goes in the manifest.
- Whether a render target is only a canvas item or also a panel of its own.
- Parameters a renderer declares, shown as inspector rows and usable as route targets.
- Verbs into rox through ADR 30's allowlist.
- What a hidden or idle window tells the renderer. Milkdrop's worker takes a pause
  command (`rox-milkdrop/src/worker.rs`), and a renderer needs the same.
- The example renderer. One idea on record: media playback, a looping clip whose
  timeline signals can move or jump between cue points, with the clip a file the
  renderer brings or the user points it at, since a workspace carries no video
  ([ADR 31](31-adr-workspace-archives.md)).

**Amended 2026-11-16: renderers are their own thing.** Andrew's product call. To the
user there are plugins and there are renderers. A plugin brings music in. A renderer
draws a render target. rox lists renderers apart from plugins and calls the surface a
render target and the program a renderer. Underneath nothing differs from
[ADR 30](30-adr-plugins.md): the same package, the same folder hash, the same approval
gate and the same switch.
