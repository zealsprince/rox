# ADR 12: Design tokens: non-color tokens are consts beside the palette

**Status:** Decided

Decision: the sizes, radii, and paces that panels share are plain consts in a `tokens`
module beside the palette, both under one `design` module in the app crate. Layout
tokens are typed `Pixels` and go straight into div chains. Paint tokens are plain `f32`,
because canvas closures do their arithmetic in f32 and only wrap the result in `px()` at
the end, so a typed token there would just be unwrapped again.

The set covers four things:

- The one easing pace, 0.35s. It had been redeclared separately in the palette, the
  cover fade, the backdrop crossfade, and the waveform reveal, which is four places to
  miss when the app's timing changes.
- The control corner radius.
- A three-step spacing ladder, replacing the tailwind-style suffix calls.
- The audio-control geometry: the play button, the compact control height that toolbar
  buttons and slider strips both use, the slider track and knob, the seek strip, the
  playhead width, and the bar rhythm the waveform and spectrum visualizers share.

A value used by one control in one place stays a local const there. A token earns its
slot in the shared module when either two files have to agree on it, or it's a knob
someone should be able to turn once and have the whole look follow.

Alternatives: adopting gpui-toolkit's `gpui-design` crate, which packages the same token
categories with platform-adaptive presets (Apple HIG, Material 3, Fluent); a
runtime-swappable token struct behind a setter, the shape the palette already uses;
keeping the tailwind suffix methods as the de facto spacing system.

Trade: `gpui-design` is out on two counts. It depends on gpui as a git dependency
pointing at the Zed tree, and cargo can't unify that with the exact crates.io version
ADR 1 pins, so taking it would mean two gpui builds in one binary. Its premise is also
the opposite of this app's: it exists to make an app look native to whichever platform
it's running on, where rox looks like itself everywhere. So we take the idea and leave
the dependency.

Consts rather than palette-style runtime data means no live editing of these values and
no swapping them per platform, neither of which anything needs today. The call is
reversible. If density or corner radius ever become real settings, the palette's setter
pattern is there to copy.

Named tokens instead of tailwind suffixes cost a little verbosity at the call site and
buy one place to change a value. They also reach somewhere the suffix methods never
could: canvas paint code, which does its own geometry and had no way to name a shared
size, now reads from the same source as the layout around it.
