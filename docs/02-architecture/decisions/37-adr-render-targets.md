# ADR 37: A render target shows frames another program draws

**Status:** Decided 2026-11-16; the shared-memory channel is built, with renderer packages, the target item and its editor, signals, the pointer, two examples and [the guide](../../03-implementation/11-renderers.md). The shared GPU texture and receivers aren't built.

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

**Amended 2026-11-23: two ways in for frames, and receivers.** Andrew's product call,
after the frame channel was measured. A render target takes frames two ways, and
neither is exclusive.

The first is shared memory, and it is rox's own channel. rox makes a block of memory,
seals it against resizing, maps it read-only and hands it to the renderer, which writes
pixels into one of two slots and says so with a short line on its standard output. Any
language can do that on every platform with no library, which is what makes it the way
in for a third party. It is built first. Measured on Linux at 1920x1080 and 60 frames a
second, it costs rox 0.66 ms of UI thread a frame for the upload, beside 0.53 ms for
Milkdrop's, and 0.77 ms off the UI thread for the copy. The cost grows with pixels: 2.66
ms of UI thread at 3840x2160, and about 0.8 ms more for each further 1920x1080 target. A
pipe carrying whole frames was measured too and lost at every size.

The second is a GPU texture the two programs share, so a frame never reaches the CPU.
It is the fast way at high resolutions and with several targets, and it is added per
platform after the first. On Windows and macOS it is Spout and Syphon, the frameworks
creative tools already send through. On Linux it is a dmabuf.

This supersedes two sentences under Costs. "Every frame crosses the CPU twice"
understates the shared-memory channel: rox itself copies each frame twice, and the
second copy is on the UI thread. "Sharing one is a gpui patch on each backend" leaves
out Linux, where blade can't import a dmabuf as it stands and would have to be vendored
and patched as well. A shared texture also brings a hazard the CPU channel doesn't: if
rox's GPU work waits on a fence the renderer controls, a renderer can stall it. The
renderer has to finish its GPU work before it says a frame is ready.

Three rules for the shared-memory channel join the size check this record already
asks for. rox makes and seals the mapping and never maps one a renderer made, since a
renderer that shrinks the file would crash rox's next read. rox caps how often a
renderer may announce a frame, since a flood costs a core. And rox shares no lock with
the renderer, so one that dies mid-frame leaves nothing held.

The announce line is the renderer speaking first, which [ADR 30](30-adr-plugins.md)'s
wire doesn't allow a plugin. A renderer doesn't use that wire. It has a small protocol
of its own and answers no requests.

A render target has two kinds of source. A renderer is a package rox starts and
supervises, over either channel. A receiver is a render target pointed at a Spout or
Syphon sender by name: rox starts nothing, and there is nothing to install or approve,
because nothing of the sender's runs under rox. Receivers arrive with the GPU way.

**Amended 2026-11-25: what a renderer can ask for.** Andrew's product call. A renderer's manifest can ask for three things past the signals and pointer every renderer gets. The first is signals it follows, which rox offers to add to the user's pool when a render target for it is placed in design mode. The second is the theme's colours. The third is the playing track's cover as pixels. Each is a grant: the enable card names it, a session gets it only when the manifest the gate approved asks for it, and a manifest that gains one is a changed manifest that switches the renderer off until the user approves it again. Nothing enters the pool without the user's click. Signals are matched by kind and band, never by name, and a renderer can suggest only Response and Threshold for what it adds. The cover travels through a second block of shared memory with the trust direction reversed: rox makes it and writes it, and lends the renderer a handle that can only read where the platform allows, never a path and never any word about the track. With song theming on, the colours change with each cover, which tells a renderer when the album changes though not which album.

**Amended 2026-11-25: a GPU texture, Linux first.** Andrew's call after spike GF-S, built the same day. A render target can lend a renderer a GPU texture beside the frame mapping. rox allocates it in the window that shows the target, exports its memory as an opaque fd, and lends it at spawn to a renderer whose manifest says `gpu`. The frame mapping stays the default and the fallback: rox makes and lends it every time, and a renderer that can't import the texture writes slots as before. A frame drawn in the texture is announced with a word of its own, `t`, and rox copies nothing for it. rox never imports a renderer's memory, since a driver was seen to crash in the importing process on a lying import, and never waits on a fence, semaphore or lock the renderer controls. It is Linux first, on Vulkan. Elsewhere gpui answers unsupported and every target stays on shared memory. This supersedes "On Linux it is a dmabuf" in the amendment of 2026-11-23: both sides run one Vulkan driver on one device, where an opaque fd needs no format modifier, and a renderer on another GPU falls back to slots. `gpu` is not a grant, and the enable card says nothing of it. The renderer already draws its target's pixels, and the texture tells it nothing about the user. The one thing it changes is that rox can't check a counter on GPU memory, so such a renderer can change what its target shows at any time, announced or not. That stays inside its own target and is only ever pixels. The spec's queue-family acquire on rox's side before each sample isn't done: it would need a hook in gpui's draw, and the driver it was built on doesn't need it. Other drivers are unverified.
