# Scope

What's core, what's peripheral, what's out on purpose, and the requirements handed down
to architecture.

## What's core versus peripheral

Core, the center of gravity, get these wrong and there's no product:

- Local library management that stays fast on a huge library.
- Deep tag and metadata editing.
- Composable panel UI: reorder, split, resize, duplicate-with-config, pop-out.
- Theming a person can build and share.
- Broad-format local playback.
- Visualizers as a first-class surface.

Peripheral, edges that can exist without being the point:

- Last.fm scrobbling. Wanted, and the community expects it, but it isn't the reason to
  switch.
- Lyric display. A lyrics panel that shows words for the playing track, fetched or from a
  local file.
- Auto-tagging. Fingerprint a track and pull correct metadata (MusicBrainz / AcoustID) to
  fix a messy import. Sits behind manual tagging, which is the core.
- Internet radio.
- ReplayGain: closer to core than the rest, a large-library person leans on it, but it
  sits behind the tagging and browsing experience.
- DSP / audio effect chain. Foobar has it, most people never touch it.

Out of scope, deliberately, not by oversight:

- **Streaming service integration** (Spotify, Tidal, Apple Music). rox is about a library
  you own and control. Reaching into a streaming catalog is a different product with
  different constraints, and chasing it dilutes the thing that makes rox worth building.
- **Mobile.** This is a desktop composition tool. The panel model doesn't translate to a
  phone and pretending otherwise wastes effort.
- **Cloud library sync.** Your library is local files. Syncing them across machines is a
  storage problem someone else already solves.
- **CD ripping.** Adjacent, well-served elsewhere, not part of the core loop.
- **A plugin or scripting SDK.** Foobar's component ecosystem was its deepest magic and
  its biggest maintenance burden. rox is built-in composition and token theming, no
  third-party extension layer. This is the largest thing left out, and it's on purpose.

## Constraints handed to architecture

Requirements product owns, structure is the architect's call:

- **All three desktop platforms, first-class.** Linux, Mac, and Windows. GPUI is
  cross-platform, so there's no reason to treat any of them as a second citizen. A Foobar
  user on Windows should be able to try rox without leaving their OS first.
- **Fast on a huge library.** Tens of thousands of tracks with no felt lag on scan,
  browse, search, or tag edit. This is a product requirement, caching and indexing are
  the architect's to design.
- **Local audio, network enrichment allowed.** rox plays a library you own, files on
  disk. No streaming services, no YouTube or yt-dlp source, no network catalog as a
  playback source. Reaching the network to enrich that local library is fine and wanted:
  Last.fm scrobbling, tag lookup, lyrics. rox has to work fully offline, the network only
  adds to a library that stands on its own.
- **Themes are tokens, layouts are shareable, nothing is scripted.** A theme is colors,
  fonts, spacing, and accent. A layout is a saved arrangement of panels and their configs.
  Both are artifacts a person can hand to someone else and have work. No scripting layer,
  that's where Foobar's theming turned fragile.
- **Panels pop out into real OS windows**, not fake in-app floats. Multi-monitor is the
  whole reason this matters.
