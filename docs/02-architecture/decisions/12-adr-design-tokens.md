# ADR 12: Design tokens: non-color tokens are consts beside the palette

**Status:** Decided

Decision: the sizes, radii, and paces panels share live as plain consts in a
`tokens` module beside the palette, both under one `design` module in the app
crate. Layout tokens are `Pixels` and feed div chains directly; paint tokens
are plain `f32` because canvas closures do their math in f32 before wrapping
in `px()`. The set covers the one easing pace (0.35s, previously redeclared
in the palette, the cover fade, the backdrop crossfade, and the waveform
reveal), the control corner radius, a three-step spacing ladder replacing the
tailwind-style suffix calls, and the audio-control geometry: the play button,
the compact control height shared by toolbar buttons and slider strips, the
slider track and knob, the seek strip, the playhead width, and the visualizer
bar rhythm the waveform and spectrum agree on. A value used by one control in
one place stays a local const there; a token earns its slot when two files
must agree or a look-wide knob should turn in one line.

Alternatives: adopting gpui-toolkit's `gpui-design` crate, which packages the
same token categories with platform-adaptive presets (Apple HIG, Material 3,
Fluent); a runtime-swappable token struct behind a setter, the palette's
shape; keeping the tailwind suffix methods as the de facto spacing system.

Trade: `gpui-design` pins gpui as a git dependency on the Zed tree, which
cannot unify with our exact crates.io pin, and its premise is looking
platform-native while rox is deliberately bespoke - so we take the idea, not
the dependency. Consts instead of palette-style runtime data mean no live
editing and no per-platform swap, which nothing needs yet; if density or
radius ever become settings, the palette's setter pattern is sitting right
there. Named tokens over tailwind suffixes trade a little verbosity for one
place to turn a knob, and give paint code, which the suffix methods never
reached, the same source as layout.
