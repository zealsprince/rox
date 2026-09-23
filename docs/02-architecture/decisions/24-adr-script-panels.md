# ADR 24: Script panels, and the scripting refusal narrows

**Status:** Proposed

Proposal: rox grows a Script panel. A script is one Rhai text held in a named workspace
pool that matches the shader pool field for field. The inline source is canonical and the
path is only a local bookmark. Entries travel inside bundles, eject writes a working file
that the watch reads edits back from, and nothing runs until its fingerprint is in the
same machine-local approval list shaders already use.

The script itself is a pure function. It receives a snapshot of app state and returns a
tree of layout nodes, which rox renders with the panel kit. It never touches gpui, the
dock, windows, settings, the filesystem, or the network, and that isn't a rule it's asked
to follow: none of those things exist inside the engine it runs in, so there's nothing to
reach for.

This narrows the scope doc's refusal of "scripted theming or UI extensions" rather than
dropping it. What made foobar's component ecosystem fragile was native code with reach.
A panel that could call into anything could also break along with anything, so a single
component could take the app down or fail on an update it had no part in.

The shader pipeline already ships user code inside bundles behind a fingerprint gate, and
it stays safe for a structural reason rather than a careful one: WGSL can only produce
pixels. A script panel keeps that shape. What a script can do is bounded by what the
snapshot and the node schema expose, so the worst outcome is a panel that draws wrongly,
plus whatever transport verbs the command table hands it.

The refusal in the scope doc is a product call. Accepting this ADR narrows it, and the
scope edit follows from that rather than preceding it. The standing rule that
extensions add sources holds either way, for anything with real reach.

**Why Rhai.** mlua is the one with an existing audience, since the foobar and Rainmeter
crowd already writes Lua, and that's worth real weight. It costs two things. Vendored Lua
needs a C toolchain on every target, which means an MSVC dependency on the Windows build
that the scope doc calls first-class. And the sandbox has to be built by hand, because
Lua ships `io`, `os`, and `load` and you remove them yourself, which makes safety a
checklist rather than a default.

Rune is pure Rust with good ergonomics, but it has the smallest ecosystem and least
stable API of the three, which is a lot of risk for a surface other people's bundles
depend on.

WASM has the strongest sandbox by a wide margin and fails on the product constraint. The
authoring loop here is edit a file and save it; "install a toolchain and compile" is a
different activity. And a bundle carrying compiled binaries makes the approval dialog
meaningless, since "here's what you're agreeing to run" can't be read by the person
agreeing to it. WASM stays the right answer for the source and playback extension host,
which is a different problem with different constraints.

Rhai wins on three counts. It's pure Rust, so no toolchain follows it onto any platform.
It starts capability-free, since `Engine::new_raw` exposes nothing that wasn't explicitly
registered, so the sandbox is the default state rather than something maintained. And it
caps runaway scripts through built-in operation and depth limits, which matters when the
code came from a bundle someone downloaded. Its interpreter is slow, and that costs
nothing here because per-frame execution is refused below.

The node-tree contract is language-agnostic, so this isn't a permanent commitment. If
Lua's familiarity ever turns out to be decisive, a second frontend can target the same
schema alongside the first.

A node tree rather than a canvas is the other central call. Returning
structure gets palette scoping, the text system, the panel kit's widgets, and hit
testing for free, and it keeps the two user-code surfaces complementary: shaders
own pixels, scripts own structure, text, and interaction, and a script panel gets
a surface shader through the same chrome every panel has. There's no per-pixel
API and no framebuffer in the schema. The schema itself stays an internal contract,
designed beside the arrange-items and status-item vocabulary the panels already
serialize rather than invented fresh. It goes public the day a second frontend or an
external tool needs it, and gets versioned then.

Execution is event-driven, never per frame. Docked panels render cached, and a
script that ran on the frame loop would be the fragility the refusal was about,
rebuilt. A script runs when the discrete player state turns over, when selection
or library events arrive, and on an optional low-capped tick. Each run has an
operation budget and a wall-clock abort. A script that trips either goes quiet,
the last good tree stays on screen, and the message goes to the same readouts a
broken shader uses. Reads come from pre-baked snapshot tables (now playing,
signals, selection, stats, a queue summary); commands go through a whitelisted
verb table onto the player. Library queries never run synchronously inside a
script: they're bounded, run off the UI thread, and their results arrive as a
later invalidation.

Storage forks, the gate doesn't. A `NamedScript` is defined beside `NamedShader` and
the bundle carries both; generalizing the two pools into one generic container was
considered and refused, since two instances isn't a pattern and the pools already
differ (shaders have assets, scripts won't). The approval list is shared: one
list, one dialog, one habit, because a second "are you sure" trains the
click-through that empties the first. A script is code that can move the queue,
which argues for a scarier gate, and the answer is capability bounds rather than
dialog copy: the gate stays one mechanism and the verb table stays short. The
export scrubber that finds shader source hiding in layout JSON grows a script
twin, or scripts travel in bundles ungated, which would be the whole gate lost.

The contract for the implementing layer: rox-core grows the script pool and its
generation counter beside the shader pool's; the bundle format grows a `scripts`
field with the same apply-replaces-wholesale semantics; panel-api grows the engine
host, the snapshot builder, the node schema, and the renderer from nodes to
elements; the panel registers like any other and starts behind the experimental
gate. The signals binding UI comes free once the panel declares its parameters as
source directives the way shaders declare slots.
