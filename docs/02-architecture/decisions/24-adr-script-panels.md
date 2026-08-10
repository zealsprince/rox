# ADR 24: Script panels, and the scripting refusal narrows

**Status:** Proposed

Proposal: rox grows a Script panel. A script is one Rhai text in a named workspace
pool that mirrors the shader pool field for field: inline source is canonical, the
path is a local bookmark, entries travel in bundles, eject writes a working file and
the watch carries edits back, and nothing runs until its fingerprint is in the same
machine-local approval list shaders use. The script itself is a pure function: it
receives a snapshot of app state and returns a tree of layout nodes that rox renders
with the panel kit. It never touches gpui, the dock, windows, settings, the
filesystem, or the network, because none of those exist inside the engine it runs in.

This narrows the scope doc's refusal of "scripted theming or UI extensions" rather
than dropping it. What made foobar's component ecosystem fragile was native code with
reach: a panel that could call into anything could break with everything. The shader
pipeline already ships user code in bundles under a fingerprint gate, and it stays
safe because WGSL can only produce pixels. A script panel keeps that shape. What a
script can do is exactly what the snapshot and the node schema expose, so the
blast radius is a wrongly-drawn panel, plus whatever transport verbs the command
table grants. The scope doc's refusal is a product call, so accepting this ADR is
what narrows it, and the scope edit lands then; the standing rule that extensions
add sources holds for everything with real reach either way.

Rhai over the alternatives, with the trades named. mlua has the audience: the
foobar and Rainmeter crowd already writes Lua, and that's worth real weight. It
costs a C toolchain on every target (vendored Lua needs cc, an MSVC dependency on
the Windows build the scope doc calls first-class) and a hand-built sandbox, since
Lua ships `io`, `os`, and `load` and you scrub them by hand. Rune is pure Rust with
good ergonomics and the smallest ecosystem and least stable API of the three. WASM
has the strongest sandbox and dies on the product constraint: the authoring loop is
edit-a-file-and-save, and "install a toolchain and compile" is not that, while a
bundle carrying compiled binaries makes the approval dialog's "here's what you're
agreeing to run" unreadable. WASM stays the right answer for the source and
playback extension host, which is a different problem. Rhai is pure Rust, starts
capability-free (`Engine::new_raw` exposes nothing you didn't register), and caps
runaway scripts with built-in operation and depth limits. Its interpreter is slow,
which doesn't matter, because per-frame execution is refused below. The node-tree
contract is language-agnostic on purpose: if Lua's familiarity ever proves decisive,
a second frontend targets the same schema as an addition, not a rewrite.

A node tree rather than a canvas is the other load-bearing call. Returning
structure gets palette scoping, the text system, the panel kit's widgets, and hit
testing for free, and it keeps the two user-code surfaces complementary: shaders
own pixels, scripts own structure, text, and interaction, and a script panel wears
a surface shader through the same chrome every panel has. There is no per-pixel
API and no framebuffer in the schema. The schema itself stays an internal contract,
designed beside the arrange-items and status-item vocabulary the panels already
serialize rather than invented fresh; it goes public the day a second frontend or
an external tool needs it, and versions then.

Execution is event-driven, never per frame. Docked panels render cached, and a
script that ran on the frame loop would be the fragility the refusal was about,
rebuilt. A script runs when the discrete player state turns over, when selection
or library events land, and on an optional low-capped tick. Each run carries an
operation budget and a wall-clock abort; a script that trips either goes quiet,
the last good tree stays on screen, and the message lands in the same readouts a
broken shader uses. Reads come from pre-baked snapshot tables (now playing,
signals, selection, stats, a queue summary); commands go through a whitelisted
verb table onto the player. Library queries never run synchronously inside a
script: they're bounded, run off the UI thread, and their results arrive as a
later invalidation.

Storage forks, the gate doesn't. A `NamedScript` sits beside `NamedShader` and the
bundle carries both; generalizing the two pools into one generic container was
considered and refused, since two instances is not a pattern and the pools already
differ (shaders carry assets, scripts won't). The approval list is shared: one
list, one dialog, one habit, because a second "are you sure" trains the
click-through that empties the first. A script is code that can move the queue,
which argues for a scarier gate, and the answer is capability bounds rather than
dialog copy: the gate stays one mechanism and the verb table stays short. The
export scrubber that finds shader source hiding in layout JSON grows a script
twin, or scripts ride bundles ungated, which would be the whole gate lost.

The contract for the implementing layer: rox-core grows the script pool and its
generation counter beside the shader pool's; the bundle format grows a `scripts`
field with the same apply-replaces-wholesale semantics; panel-api grows the engine
host, the snapshot builder, the node schema, and the renderer from nodes to
elements; the panel registers like any other and starts behind the experimental
gate. The signals binding UI comes free once the panel declares its parameters as
source directives the way shaders declare slots.
