# ADR 23: Shader programs become pass chains, assets ship in the bundle

**Status:** Decided

Decision: a rox shader stays one WGSL text, and grows the ability to describe more
than one pass. `// @pass name` directives split the text into an ordered chain of
fragment stages; each pass renders to an offscreen target and later passes bind
earlier outputs by name, alongside the existing `screen`, `prev`, and `samp`
bindings. `// @asset name: file` directives declare image inputs, carried inside the
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

Directives over structured pass arrays in the config is the load-bearing call. A
`passes: Vec<String>` on the pool entry is the cleaner shape at compile time, and it
loses everywhere else: the pool entry, the eject file, the hot-reload watch, the
approval fingerprint, and the bundle all assume one shader is one text, and a pass
array would fork every one of those code paths plus the authoring loop into
list-aware variants. A splitter at registration time is a few dozen lines against
that, and the `// @slot n: name` convention already establishes that rox shaders
declare their metadata as comment directives. The cost is that pass boundaries are
declared in comments rather than types, caught at registration rather than
deserialization, which is where shader errors already surface and already have a readout.

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

Assets are data, not code, and the approval gate is about code, so assets don't gate.
The program text hashes and gates exactly as today, one fingerprint over the trimmed
text; an image the approved code samples can misrender a look but can't execute, and
gating it would train people to click through the dialog that matters. Assets travel
inside the bundle as encoded bytes next to the shader pool, for the same reason
shader source travels inline: a path-only reference imports as a dead look on the
next machine. Eject writes them as real files beside the ejected WGSL and the watch
reloads both, so the authoring loop stays an external editor plus a save. Bundles
with plates stay small in practice because the aesthetic that calls for plates is
1-bit imagery, which compresses to almost nothing; a soft size warning at export
beats a hard cap that a legitimate look would hit.

One asset value is reserved rather than being a file: `// @asset art: @cover` binds
the playing track's cover under the declared name. The bytes come off the window's
player at registration and the program re-registers when the track turns over, one
split and one compile per switch, nothing per frame. A track without art binds a
flat dark plate, so the binding always samples something, and a shader that arrived
inline still resolves it, since the art belongs to the player rather than to a
folder. Registered covers are downscaled to a 512 cap on the long edge because the
renderer never evicts textures within a window's life; eviction is the follow-up
this trades against, not a redesign.

ShaderToy's multi-buffer model (BufferA through BufferD feeding an Image pass) is
the same shape and confirms the semantics; ours differs in letting the author name
the passes and in keeping one file. A node-graph compositor was considered and
refused. The product question underneath it is rox's performance ambition:
VJ-lite, live control and look-switching over surfaces it already has, where the
rendering ceiling that direction needs is chains under a fixed compositor (an A/B
blend between two chains, when that gets built), never user-authored topologies.
Everything else on the VJ-lite path is control-plane work over this contract:
hand-set slots and routes are the performance knobs, ADR 22's socket is where a
MIDI or OSC bridge would go, and a popped-out Shader panel is the projector output.
User-authored topology is TouchDesigner's product, and even gig-grade VJ software
is layers of linear chains under a fixed compositor, which is how far the chain
model demonstrably goes. Passes with named inputs and outputs remain node-shaped
regardless, so if this read is ever revisited a chain lifts into a graph without
breaking a bundle. Compute passes are the right answer for large sorts and were
deferred on capability grounds, since the gpui patches build on blade's render
pipelines and a compute stage is a different tier of surgery; a fragment chain covers the
visible aesthetic, and the pass contract here is likewise the one a compute stage would
slot into later without redesign.

The contract for the implementing layer: the gpui patch surface grows texture
registration for caller-provided images and chain registration in place of
single-source registration, with intermediate targets owned by the renderer and
recycled across frames. The rox side grows the directive splitter and asset
plumbing (bundle field, eject, watch, export), and no surface driver changes shape:
post, region, and primitive keep their existing entry points, handing over a parsed
chain where they handed over a source string. The Critters bundle is the acceptance
look: a real pixel sort replacing the comet-tail approximation, and a plate stamped
into the Serpent panel.
