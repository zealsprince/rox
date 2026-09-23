# ADR 25: Every shader surface has a stack, edited as folds

**Status:** Proposed

Proposal: every shader surface holds an ordered list of shaders instead of one.
A panel's chrome, the app-wide overlay shader, and the Shader panel all grow the
same shape, and the settings UI for each becomes the Signals page's: a section
with an Add button, one fold per entry, the entry's identity and its enable switch
in the header, and today's source picker, slot knobs, and routes in the body. The
list composes into a single pass chain at registration, so nothing downstream of
the composer learns a new concept.

The stack is a list and never a graph. This is ADR 23's refusal held, not
revisited: that record put the rendering ceiling at "chains under a fixed
compositor" and named an A/B blend as the shape it expected. A stack is that
compositor, generalized from two slots to N and made ordinal instead of a
crossfade. Entries see the accumulation below them and nothing else; there's no
routing an entry's output sideways, no naming another entry's passes, and no
topology in the config. What a bundle stores stays a list of shaders in an order,
which is the thing a person can read.

Redefining `screen` to mean "whatever is underneath you" is the call that makes a stack
worth building at all. Today that binding is the composed frame under the surface's rect.
In a stack it becomes the frame with every entry below this one already drawn onto it.

That one change makes filters compose. Tube stacked over Dither gives a dithered
CRT, because Tube reads a frame that Dither has already processed, and neither shader
contains a line of code about the other.

It also collapses the two shapes a shader can have into one rule instead of two special
cases. An entry that binds `screen` composites itself, exactly as it does today, because
reading the accumulation and printing it back out *is* compositing. An entry that doesn't
bind `screen` has nothing underneath to print, so it draws over a transparent target and
the composer appends a synthetic pass that blends its output over the accumulation
instead. Which of the two an entry gets is read from `// @overlay`. That directive
already existed to tell the picker whether a shader hides the app behind it. The
composer is its second consumer: the same declaration now tells it whether a blend pass
has to be appended.

**Signals go per entry**, and this is the one change with real teeth in the renderer.
Today a chain shares one `ShaderParams` across all of its passes, filled once per draw,
because every pass belongs to the same program and so all sixteen slots do too. Stacked
entries each bring their own routes, so that assumption breaks: the uniform now has to be
filled per pass, from whichever entry that pass came from.

The change is smaller than it sounds, because the renderer already builds a
`ShaderParams` inside the per-pass loop. What moves is where the values come from, plus
carrying an entry index on each composed pass so the fill knows which routes to read.
Nothing about the bind group layout changes. And a caller handing over a single shader
behaves exactly as it did before, since a one-entry stack fills every pass from the same
set of routes.

The pass cap becomes a budget on the stack. Eight passes and eight images are per
program today; they stay per composed program, which means a stack spends them
together and Dither's four leave room for three more entries plus their blends.
That's a real ceiling and it's the right one: past it the thing being expressed
needs a render graph, which is the refusal above. The error has to name the entry
that overflowed rather than reporting a number about a program nobody wrote, and
the settings fold is where it shows, next to the entry.

Nothing migrates, the same way nothing migrated for chains. A config holding one
shader deserializes as a one-entry stack, so every layout, bundle, and settings
file in the wild reads without a version bump, and a stack of one serializes back
to the old shape when it can. Approval stays per source rather than per stack:
each entry keeps its own fingerprint, the machine-local list is untouched, and a
bundle with a four-deep stack asks about the sources in it that this machine
hasn't already approved. Hot reload likewise stays per entry, which the pool
watch already does for named shaders and the per-surface watch does for files.

The UI is the Signals page's. That page already solved this problem: a variable-length
list of things with too many knobs to show at once, where the identity has to stay
visible and the tuning folds away. Copying it means the shader sections stop being a
wall of rows the moment a surface has more than one shader, and it means the two pages
read as one app. The header shows what the entry is, its Scene or Overlay mark, and its
switch, so a stack can be read without opening anything. Reorder controls belong in that
header too, since order is the whole semantic, and up-down buttons are the right v1 over
drag: the list is short, and the dock already owns every drag gesture in the app.

Refused or deferred, with reasons. Blend modes past premultiplied-over (add,
multiply, screen) were considered and left out of v1: `over` plus a shader that
reads what's under it covers the cases, and a mode picker per entry is a knob that
wants examples before it wants a menu. Per-entry opacity is cheap and stays out
for the same reason, since a stack's first job is composition and a fade is a
performance control. Naming a whole stack and putting *that* in the workspace pool
was refused outright: the pool holds shaders, entries reference pool shaders by
name already, and a second kind of named thing in the same field is how a format
stops being readable. Making the Shader panel's stack and a panel's chrome stack
share one config type is desirable and shouldn't be forced; they differ today in
whether an empty list is legal, and that difference is real.

The contract for the implementing layer: rox-core's `PanelShader`, `ShaderConfig`,
and `PostShaderConfig` grow a list with a lenient reader for the single-shader
shape; panel-api's chain module grows the composer that concatenates entries with
prefixed pass and asset names, rebinds `screen` per entry, and appends the blend
pass for entries that declare `// @overlay`; the gpui patch grows per-pass uniform
fill and an entry index on the composed pass description; the three settings
surfaces grow the fold list, which is the Signals page's block shape over a
different item. Acceptance is a Critters variant running Dither with Tube over
it, and a panel running Wall with Badge on top, neither shader edited to know
about the other.
