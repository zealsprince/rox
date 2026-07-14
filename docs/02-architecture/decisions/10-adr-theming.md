# ADR 10: Theming: the palette is data behind one setter, the backdrop is CPU-baked

**Status:** Decided

Decision: the palette is a token struct, one field per role, whose defaults are the
current hardcoded values. The `palette::*` accessors keep their signatures and read a
process-global current palette; every change goes through one setter that swaps it,
re-feeds the gpui-component `Theme` tokens the widgets draw, and refreshes all windows.
Three writers feed that one pipe. User edits, made in a settings window and persisted in
the settings file. A transparency pair of scalars, surface opacity and backdrop strength,
applied inside the background accessors rather than stored per token. And a derived mode
that, while a track plays, re-tints each token's hue and chroma toward a seed color
extracted from the cover art while preserving the token's lightness, so the palette's
contrast ladder survives any album. Palette changes ease componentwise from current to
target; editing always targets the user palette, with derivation layered on top. The
backdrop is the playing track's art, downscaled and gaussian-blurred once per track
change on the existing background art path, resolved by a shared now-playing-art entity
and handed to GPUI as a `RenderImage`; the bilinear upscale to window size multiplies
the blur. With several windows playing different tracks, the window that most recently
started one owns the seed and the backdrop is per window.

Alternatives: threading a context parameter through every palette call site, or a GPUI
global, instead of the static; adopting gpui-component's `Theme` outright as the single
token set; exposing alpha on every token instead of the two scalars; extracting a full
palette from the art (color-thief, material-colors) instead of re-tinting the ladder;
and for the blur, a runtime GPU pass or compositor-level window transparency.

Trade: GPUI has no runtime blur (`blur_radius` is shadow-only, and the blurred window
appearance blurs the desktop behind the window), so the blur must be baked into the
image; doing it once per track on a thread that already loads the art costs nothing per
frame, at the price of a fixed blur that can't respond to what's over it. The static
sits outside GPUI's reactivity, so repaints are explicit, which is fine at the rate
palettes change and keeps the whole pipeline in one choke point; threading a context
through would touch every render in the app for the same result. gpui-component's
`Theme` covers only what its widgets draw, so it stays a projection of our tokens
rather than the source. Two scalars are less expressive than per-token alpha but keep
readability one knob instead of a combinatorial space of user-authored alphas. Fitting
the extraction to the ladder rather than trusting it wholesale gives up some of the
album's character in exchange for text that stays legible on a near-black or neon
cover.
