# ADR 10: Theming: the palette is data behind one setter, the backdrop is CPU-baked

**Status:** Decided

Decision: the palette becomes a token struct with one field per role, whose defaults are
the values that used to be hardcoded. The `palette::*` accessors keep the signatures they
already have and read a process-global current palette, so no call site changes. Every
change to that palette goes through a single setter, which swaps the struct, rewrites the
gpui-component `Theme` tokens its widgets draw from, and refreshes every open window.

Three different writers go through that one setter:

- **User edits**, made in a settings window and persisted to the settings file.
- **A transparency pair**, surface opacity and backdrop strength. These are two scalars
  applied inside the background accessors when they're read, rather than baked into each
  token as an alpha channel.
- **A derived mode.** While a track plays, each token's hue and chroma are pulled toward
  a seed color extracted from the cover art, while its lightness is left alone. Keeping
  lightness fixed preserves the palette's contrast ladder, so the album can
  recolor the app without any pair of roles collapsing into each other.

Changes ease componentwise from the current value to the target rather than snapping.
Editing always writes to the user palette, and derivation is layered over the top of it,
so an album's tinting never overwrites what someone actually set.

The backdrop is the playing track's cover art, downscaled and gaussian-blurred once per
track change on the background art path that already exists. A shared now-playing-art
entity resolves it and hands it to gpui as a `RenderImage`. Because it's rendered small
and then upscaled bilinearly to window size, the upscale multiplies the blur rather than
fighting it, which is why the small buffer is enough. When several windows are playing
different tracks, the window that most recently started one owns the seed, and each
window keeps its own backdrop.

Alternatives: threading a context parameter through every palette call site, or using a
gpui global, instead of the static; adopting gpui-component's `Theme` outright as the
single token set; exposing alpha on every token instead of the two scalars; extracting a
full palette from the art with color-thief or material-colors instead of re-tinting the
existing ladder; and for the blur, a runtime GPU pass or compositor-level window
transparency.

Trade: the blur has to be baked into the image because gpui can't do one at runtime. Its
`blur_radius` applies to shadows only, and the blurred window appearance blurs the
desktop behind the window rather than anything inside it. Baking costs nothing per frame,
since it happens once per track on a thread that was already loading the art. The cost is
a fixed blur that can't respond to whatever ends up drawn over it.

The static sits outside gpui's reactivity, so nothing repaints on its own when the
palette changes and the setter has to trigger that explicitly. At the rate palettes
change that's cheap, and it keeps the entire pipeline behind one choke point. Threading
a context parameter through instead would touch every render function in the app to
arrive at the same behavior.

gpui-component's `Theme` only covers what its own widgets draw, which is a subset of the
app's surfaces, so it can't be the single source. It stays a projection that the setter
writes into from our tokens.

Two scalars are less expressive than per-token alpha. Readability against a backdrop is
one property, and per-token alphas turn it into a combinatorial
space where a user can author a palette that's unreadable in ways that are hard to trace
back. Fitting the extracted color to the existing ladder rather than trusting a full
extracted palette makes the same kind of trade. It gives up some of the album's
character and keeps text legible over a near-black cover or a neon one, which a wholesale
extraction doesn't.
