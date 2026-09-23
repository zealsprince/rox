# ADR 28: MilkDrop presets through libprojectM, rendered off-thread and read back

**Status:** Decided

Decision: rox plays MilkDrop presets by linking libprojectM into the process rather
than reimplementing the format. A worker thread owns its own OpenGL context and a
projectM instance, renders each frame into a framebuffer object, reads the pixels
back, and hands them to the panel. The panel uploads that buffer as a dynamic user
texture and draws it through the shader chain path from ADR 23, so the result docks,
tabs, pops out, and takes a surface shader like any other panel body. Audio comes off
the same `AudioFeed` the spectrum panel reads, pulled by the worker rather than pushed
from a paint pass, which keeps the landmine in ADR 19's callback discipline where it
already is.

The alternatives were porting MilkDrop to WGSL on top of ADR 23's chains, and shipping
no preset support at all. The port is the one that looks tempting from inside the
existing renderer and doesn't survive contact. A preset isn't a fragment shader. It's
a per-frame equation script plus warp and composite stages plus custom waves and
shapes, and the twenty-year preset corpus is the entire reason anyone wants the
feature. A partial port renders most of that corpus wrong, which is worse than not
having it. A complete one is a rewrite of a program that already exists, is
maintained, and is LGPL.

**This is the readback ADR 8 refused.** ADR 8 killed the CPU
generative visual because a worker thread rasterizing and blitting a framebuffer every
visible frame is a standing tax, and the same look was available sharp and nearly free
as a shader. Both halves of that reasoning invert here. The tax is the same shape, but
what it buys isn't a decoration we'd have written ourselves. It's a format with a
preset library nobody can reproduce in the alternative. And there's no cheaper form
waiting: the shader path that made ADR 8's refusal correct is what can't express a
preset. The resolution softness ADR 8 also objected to is answered by
rendering at the panel's device-pixel size and reallocating on resize, so the buffer
matches the target instead of being scaled up to it.

Presets are data, not extensions. The standing refusal is scripted theming and UI
behavior, "extensions add sources, not behavior inside the UI", and a `.milk` file
doesn't touch that boundary: it's arithmetic in projectM's own sandboxed evaluator
over a fixed variable set, with no filesystem, no network, and no path back into rox.
That puts it with ADR 23's assets rather than with ADR 24's Rhai panels, so presets
load without an approval gate. Adding a second confirm dialog here would train
the click-through the one real gate depends on not having.

The pin is projectM master, not the 4.1.7 release, because 4.1.7's render path binds
the default framebuffer at the end of a frame and a surfaceless worker context has no
default framebuffer to bind. Master adds `projectm_opengl_render_frame_fbo` and
`projectm_create_with_opengl_load_proc`, which are the two calls this design needs:
render into our own target, and resolve GL through our loader instead of linking
libGL. The cost of pinning a commit is that the vendoring script and the nix
derivation both name it and have to move together, which is the same bookkeeping the
vendored gpui already needs. libprojectM is LGPL-2.1-only linked statically into an
AGPL-3.0-only binary whose source ships, so the relink clause is satisfied by the
source we publish anyway.

Zero-copy is the follow-up, not part of this. The frame could stay on the GPU through
blade's `ExternalMemorySource` on Linux and D3D11 interop on Windows. That's real work
in the vendored renderer on two backends, and it's gated behind a number this design has
to produce first: the measured cost of `glReadPixels` plus upload at a realistic panel
size. If that number is small against the frame budget, the interop work buys latency
and nothing else and can wait. Building the readback path first also makes the
comparison possible, since the texture and chain plumbing is the same either way.

The new costs are a cmake build inside `cargo build` (about 18 seconds cold on a
32-thread machine, cached after that), a hand-written FFI surface that has to be
re-checked against the headers on every bump, and a second GL context per open panel
on machines whose drivers dislike that. The last one is the real risk, and it's
contained: the worker reports a failure status instead of panicking, and a machine
with no usable context gets an empty panel with a message rather than a dead app.
