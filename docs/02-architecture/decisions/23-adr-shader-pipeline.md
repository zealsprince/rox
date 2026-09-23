# ADR 23: Shader programs become pass chains, assets ship in the bundle

**Status:** Decided

Decision: a rox shader stays one WGSL text, and grows the ability to describe more
than one pass. `// @pass name` directives split the text into an ordered chain of
fragment stages; each pass renders to an offscreen target and later passes bind
earlier outputs by name, alongside the existing `screen`, `prev`, and `samp`
bindings. `// @asset name: file` directives declare image inputs, stored inside the
workspace bundle as encoded bytes and bound as textures under the declared name. A
text with no directives is a one-pass chain, which is every shader that exists today,
so nothing migrates. The chain semantics apply to all three surfaces the same way:
the whole-window post pass, the per-panel region pass, and the in-scene primitive,
whose plain-quad fast path remains the degenerate case for a single pass that reads
no screen.

The capability being added is the one the Critters prototype ran into the edge of:
a single fragment stage can fake pixel sorting with tap loops and cut shapes
procedurally, but it cannot run a real sorting scan, build a blur at more than one
scale, or stamp an image plate into a scene. All three need the same two things:
intermediate results a later stage can read, and inputs that aren't the screen.

Directives over structured pass arrays in the config is the central call. A
`passes: Vec<String>` on the pool entry is the cleaner shape at compile time, but it
loses everywhere else. The pool entry, the eject file, the hot-reload watch, the
approval fingerprint, and the bundle all assume one shader is one text, and a pass
array would fork every one of those code paths plus the authoring loop into
list-aware variants. A splitter at registration time is a few dozen lines against
that, and the `// @slot n: name` convention already establishes that rox shaders
declare their metadata as comment directives. The cost is that pass boundaries are
declared in comments rather than types, and caught at registration rather than
deserialization. That's where shader errors already surface and already have a readout.

Within a chain, a pass binds the composed frame as `screen`, its own last-frame
output as `prev` (the existing feedback contract, resize clears it), earlier passes
in this frame by their declared names, and declared assets by theirs. Binding
composition stays what it is today, on demand by name reference, so a pass pays only
for what it reads. The uniform block is identical for every pass: same slots, same
meta, same mouse, one clock per program. A pass may declare a resolution scale
(full size by default, halves for pyramid work); that goes in now because the target
allocator's contract is the expensive thing to reopen, and a blur pyramid is a
headline use, not a hypothetical. Chains are capped at eight passes, the same
pragmatism as sixteen slots: past that, the design being expressed needs a render
graph, which is refused below, not given a bigger cap.

Assets don't go through the approval gate, because the gate exists to stop untrusted
code from running and an asset isn't code. The program text still hashes and gates
exactly as it does today, one fingerprint over the trimmed source. An image that
approved code samples can make a look render wrong, but it can't execute anything, and
adding a second dialog for it would teach people to click through the dialog that
actually matters.

Assets travel inside the bundle as encoded bytes, next to the shader pool, for
the same reason shader source travels inline rather than as a path: a bundle that only
references a file on the author's disk imports as a dead look on anyone else's machine.
Eject writes them back out as real files beside the ejected WGSL, and the watch picks up
changes to both, so the authoring loop stays what it was, an external editor and a save.

Bundle size stays reasonable in practice without a cap, because the aesthetic that wants
image plates is 1-bit imagery, which compresses to almost nothing. Export shows a soft
size warning instead, since a hard limit would eventually block a legitimate look for
being large.

One asset value is reserved instead of naming a file. `// @asset art: @cover` binds the
playing track's cover art under the declared name. The bytes are read off the window's
player when the program registers, and the program re-registers when the track changes,
so the cost is one split and one compile per track switch and nothing per frame. A track
with no art binds a flat dark plate rather than nothing, so the binding always has
something to sample and a shader never has to handle the empty case. Because the art
comes from the player rather than from a folder, a shader that arrived inline in a bundle
resolves it just as well as one ejected to disk.

Registered covers are downscaled to a 512-pixel cap on the long edge. That's a direct
consequence of the renderer never evicting textures within a window's lifetime: every
cover bound during a session stays resident, so an uncapped size would accumulate
full-resolution art for as long as the window is open. Eviction is the follow-up that
would remove the cap rather than a redesign of any of this.

ShaderToy's multi-buffer model, where BufferA through BufferD feed an Image pass, is the
same shape as this one and is useful confirmation that the semantics work. The two
differences here are that the author names the passes rather than picking from fixed slot
names, and that everything stays in one file.

A node-graph compositor was considered and refused, and the reason is a product question
rather than a rendering one. What rox is aiming at is VJ-lite: live control and
look-switching over the surfaces it already has. The rendering ceiling that ambition
actually needs is chains running under a fixed compositor, with an A/B blend between two
chains as the most complex form of it, and that's reachable without user-authored
topologies at all.

Everything else on the VJ-lite path is control-plane work on top of this contract.
Hand-set slots and routes are the performance knobs. [ADR 22](22-adr-control-surface.md)'s
socket is where a MIDI or OSC bridge would attach. A popped-out Shader panel is the
projector output. None of those need a graph.

Authoring topology is a different product, and TouchDesigner is already it. Even
gig-grade VJ software is layers of linear chains under a fixed compositor, which is
evidence about how far the chain model goes rather than an argument from taste. And the
refusal isn't a one-way door: passes with named inputs and outputs are node-shaped
whether or not there's a graph over them, so if this read is ever revisited, a chain
lifts into a graph without breaking any bundle already written.

Compute passes are the right answer for large sorts and were deferred on capability
grounds. The gpui patches build on blade's render pipelines, and a compute stage is a
different tier of surgery. A fragment chain covers the visible aesthetic, and the pass
contract here is the one a compute stage would slot into later without redesign.

The contract for the implementing layer: the gpui patch surface grows texture
registration for caller-provided images and chain registration in place of
single-source registration, with intermediate targets owned by the renderer and
recycled across frames. The rox side grows the directive splitter and asset
plumbing (bundle field, eject, watch, export), and no surface driver changes shape:
post, region, and primitive keep their existing entry points, handing over a parsed
chain where they handed over a source string. The Critters bundle is the acceptance
look: a real pixel sort replacing the comet-tail approximation, and a plate stamped
into the Serpent panel.
