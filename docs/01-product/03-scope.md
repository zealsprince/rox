# Scope

What's core, what's peripheral, what's delivered through extensions, what's out on
purpose, and the requirements handed down to architecture.

## What's core versus peripheral

Core, the center of gravity, get these wrong and there's no product:

- Local library management that stays fast on a huge library.
- Deep tag and metadata editing.
- Composable panel UI: reorder, split, resize, duplicate-with-config, pop-out.
- Theming a person can build and share.
- Broad-format local playback.
- Visualizers as a first-class surface.

Peripheral, edges that can exist without being the point:

- Listening stats. Every real listen recorded on disk, rolled up per track, artist,
  album, and genre, surfaced in browse (most played, never played, recently played) and
  a stats panel. Closest to core of anything on this list: the library obsessive treats
  their history as part of the library. It sits behind the library because it's
  worthless if browsing doesn't hold up first.
- Last.fm scrobbling. Wanted, and the community expects it, but it isn't the reason to
  switch. It reports the same listens stats records, so the two never disagree about
  what counted.
- Lyric display. A lyrics panel that shows words for the playing track, fetched or from a
  local file.
- Auto-tagging. Fingerprint a track and pull correct metadata (MusicBrainz / AcoustID) to
  fix a messy import. Sits behind manual tagging, which is the core.
- Internet radio.
- ReplayGain: closer to core than the rest, a large-library person leans on it, but it
  sits behind the tagging and browsing experience.
- DSP / audio effect chain. Foobar has it, most people never touch it.

## Sources as extensions

The local library is the core, but it's one source among several. rox grows an
extension system whose first job is playback sources:
Spotify, YouTube / YouTube Music, Tidal, each showing up as its own library view backed
by a community-maintained extension, think VSCode extensions by proxy. This is what
gives rox a life beyond people who keep a large local collection.

Extensions are the vehicle rather than core code for a practical reason: the viable
integration paths for these services (librespot for Spotify, yt-dlp for YouTube) are
unofficial and break whenever the service changes something. A community extension
updates on its own release cycle, and rox itself is never the thing that's broken.

Sources aren't equal, and the product shows the difference instead of papering over it:

- **Full.** The source hands rox decodable audio (Tidal's API, yt-dlp streams,
  librespot's decoded samples). It plays through rox's engine, so gapless, ReplayGain,
  and visualizers all work.
- **Tapped.** rox remote-controls playback elsewhere but captures the local audio
  output, so visualizers work while engine features don't. Only possible when the audio
  actually plays on this machine.
- **Remote.** Browse and control only.

A unified library, one view merging local and streaming catalogs with matching across
them, is an ambition rather than a promise. What the core owes it is not closing the
door: track identity that isn't welded to file paths.

The extension surface stays narrow: a source is a library provider plus a playback
provider. It is not a scripting layer for the UI.

## Out of scope

Deliberately, not by oversight:

- **Mobile.** This is a desktop composition tool. The panel model doesn't translate to a
  phone and pretending otherwise wastes effort.
- **Cloud library sync.** Your library is local files. Syncing them across machines is a
  storage problem someone else already solves.
- **CD ripping.** Adjacent, well-served elsewhere, not part of the core loop.
- **Scripted theming or UI extensions.** Foobar's component ecosystem was its deepest
  magic and its biggest maintenance burden, and the fragility lived in scripted panels.
  Extensions add sources, not behavior inside the UI: themes stay tokens, layouts stay
  declarative artifacts.

## Constraints handed to architecture

Requirements product owns, structure is the architect's call:

- **All three desktop platforms, first-class.** Linux, Mac, and Windows. GPUI is
  cross-platform, so there's no reason to treat any of them as a second citizen. A Foobar
  user on Windows should be able to try rox without leaving their OS first.
- **Fast on a huge library.** Tens of thousands of tracks with no felt lag on scan,
  browse, search, or tag edit. This is a product requirement, caching and indexing are
  the architect's to design.
- **Local-first, offline always.** The core is a library you own, files on disk, and
  rox works fully offline: playback, browse, search, and tag editing never depend on the
  network. Enriching that library over the network (Last.fm scrobbling, tag lookup,
  lyrics) is fine and wanted. Streaming sources are extensions and purely additive; the
  offline core doesn't grow dependencies on them.
- **Don't paint sources into a corner.** Streaming isn't core, but two things are cheap
  in the initial design and brutal to retrofit. Track identity is source-qualified, with
  local files as the first source rather than the assumption baked into every key. And
  playback keeps a clean command-in, state-out seam so a second source engine can sit
  behind the same contract. How extensions are hosted is an open question and doesn't
  constrain the core.
- **Themes are tokens, layouts are shareable, nothing is scripted.** A theme is colors,
  fonts, spacing, and accent. A layout is a saved arrangement of panels and their configs.
  Both are artifacts a person can hand to someone else and have work. No scripting layer,
  that's where Foobar's theming turned fragile.
- **Listening history is a record, not counters.** A real listen (a skip isn't a
  listen) lands on disk as an event with when it happened, keyed to track identity, so
  history survives rescans and file moves, and any stat someone thinks of later can be
  derived from what was kept. Data volume isn't a concern worth trading the raw record
  against. Recording never touches the audio path and never slows browse.
- **Panels pop out into real OS windows**, not fake in-app floats. Multi-monitor is the
  whole reason this matters.
