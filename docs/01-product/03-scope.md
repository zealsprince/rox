# Scope

What's core, what's peripheral, what's delivered through plugins, what's out of scope,
and the requirements handed down to architecture.

## What's core versus peripheral

Core. Get these wrong and there's no product:

- Local library management that stays fast on a huge library.
- Deep tag and metadata editing.
- Composable panel UI: reorder, split, resize, duplicate-with-config, pop-out.
- Theming a person can build and share.
- Broad-format local playback.
- Visualizers as a first-class surface.

Peripheral. These can exist without being the point:

- Listening stats. Every real listen recorded on disk, rolled up per track, artist,
  album, and genre, surfaced in a history panel (most played, never played, recently
  played) and a stats panel. Closest to core of anything on this list, since the library
  obsessive treats their history as part of the library. It ranks below the library
  because it's worthless if browsing doesn't hold up first.
- Last.fm scrobbling. Wanted, and the community expects it, but it isn't the reason to
  switch. It measures a listen the way stats does, played time rather than wall time,
  with its own send threshold as a user knob.
- Lyric display. A lyrics panel that shows words for the playing track, fetched or from a
  local file.
- Auto-tagging. Fingerprint a track and pull correct metadata (MusicBrainz / AcoustID) to
  fix a messy import. Ranks below manual tagging, which is the core.
- Internet radio.
- ReplayGain. A large-library person relies on it, which puts it closer to core than the
  rest. It still ranks below tagging and browsing.
- DSP / audio effect chain. Foobar has one. Most people never touch it.
- System integration. Media keys and the OS transport surface (MPRIS on Linux), and a
  tray presence with quit-to-tray. Expected of a desktop player, invisible until missing.

## Plugins

The local library is the core, but it's one source among several. A plugin brings
something from outside into rox. The first thing people will want is a streaming
service inside rox: browse its catalog, see their likes and playlists, and play any of
it like a local track. That gives rox a life beyond people who keep a large local
collection.

The community builds plugins. rox publishes the contract a plugin is written against, an
example plugin that talks to no service at all, and the host that runs them. It never
publishes a plugin for a particular service, which keeps a layer between the project and
any one service's terms. A user installs a plugin from a package file or from the
catalog, sees it listed, and switches it on. Nothing runs before that. An author can work
in a plain plugin folder instead of a package. Switching one on says plainly that the
plugin runs as a program with the user's permissions, and shows what it declared. A plugin
that changes on disk turns off until the user turns it on again.

Plugins are the vehicle rather than core code for a practical reason: the viable
integration paths for these services are unofficial client libraries and downloaders
that break whenever the service changes something. A community plugin updates on its own
release cycle, and rox itself is never the thing that's broken.

What a plugin can do is a short list, and it grows only by a product decision:

- Add a source. The user browses the service's catalog and their own collections in
  the plugin's panel and searches it there. Its tracks become library rows the user
  plays, queues, and keeps in playlists like any other source's.
- List panels under its own name in Add Panel. Each one is a preset of a panel rox
  already ships, set up for the plugin's source.
- Offer a radio. Started from one of its tracks, albums or artists, the service picks
  what plays next, and rox keeps playing it through its own engine as the queue runs
  down.
- Offer actions, like downloading a track. rox lists them in the menus for the plugin's
  tracks and albums, asks for any choices the action needs, and shows its progress and
  outcome. The work runs in the plugin.

A plugin can use programs the user has installed, such as a command-line downloader. It
lists the ones it needs, and rox says which are missing.

A plugin never runs code in the UI, processes audio, or stands in for a core surface
like a visualizer.

The user decides what a plugin puts in the library. Syncing a collection, such as their
liked tracks or one playlist, keeps its tracks in the library, where local search finds
them. Anything else becomes a library track when someone plays, queues, or saves it from
the plugin's panel. Browsing and searching the service happen in that panel. The
library's search box never waits on a plugin.

Sources aren't equal, and the product shows the difference:

- **Full.** The source provides rox decodable audio (files a server serves, audio a
  plugin fetched or decoded and streams to rox). It plays through rox's engine, so
  gapless, ReplayGain, and visualizers all work. A self-hosted source like Subsonic is
  Full without the fragility that put the other sources behind plugins, since the
  server is the user's own. A live stream is Full on transport and gets visualizers,
  while gapless and ReplayGain have nothing to act on.
- **Tapped.** rox remote-controls playback elsewhere but captures the local audio
  output, so visualizers work while engine features don't. Only possible when the audio
  actually plays on this machine.
- **Remote.** Browse and control only.

Plugin sources are Full. Tapped and Remote would need a plugin capability for
controlling playback somewhere else, and the set doesn't include one yet.

A unified library, one view merging local and streaming catalogs with matching across
them, is an ambition rather than a promise. All the core owes it is track identity that
isn't welded to file paths.

## Out of scope

- **Mobile.** This is a desktop composition tool. The panel model doesn't translate to a
  phone and pretending otherwise wastes effort.
- **Cloud library sync.** Your library is local files. Syncing them across machines is a
  storage problem someone else already solves.
- **CD ripping.** Well served by other tools, and outside the core loop.
- **Scripted theming or UI extensions.** Foobar's component ecosystem was its deepest
  magic and its biggest maintenance burden. The fragility came from scripted panels.
  Plugins bring things in from outside rox, not behavior inside the UI: themes stay
  tokens, layouts stay declarative artifacts. A plugin's panels are presets of panels
  rox already ships, so nothing a plugin supplies executes in the UI.

## Constraints handed to architecture

Product owns these requirements. The structure behind them is the architect's call:

- **All three desktop platforms, first-class.** Linux, Mac, and Windows. gpui is
  cross-platform, so there's no reason to treat any of them as second-class. A Foobar
  user on Windows should be able to try rox without leaving their OS first.
- **Fast on a huge library.** Tens of thousands of tracks with no felt lag on scan,
  browse, search, or tag edit.
- **Local-first, offline always.** The core is a library you own, files on disk, and rox
  works fully offline: playback, browse, search, and tag editing never depend on the
  network. Enriching that library over the network (Last.fm scrobbling, tag lookup,
  lyrics) is fine and wanted. Streaming sources are plugins and purely additive; the
  offline core doesn't grow dependencies on them. The catalog of shared workspaces,
  plugins and objects is the same: with no network it shows its last saved copy or
  nothing, and whatever was installed from it keeps working.
- **Don't paint sources into a corner.** Streaming isn't core, but two things are cheap
  in the initial design and brutal to retrofit. Track identity is source-qualified, with
  local files as the first source rather than the assumption baked into every key. And
  playback keeps a clean command-in, state-out seam so a second source engine can
  implement the same contract. How plugins are hosted doesn't constrain the core.
- **Plugin tracks scrobble only when declared, and never save to disk.** The user can
  still turn scrobbling off for a plugin that declares it. rox never writes plugin
  tracks to disk the way it saves songs from a station.
- **Themes are tokens, layouts are shareable, nothing is scripted.** A theme is colors,
  fonts, spacing, and accent. A layout is a saved arrangement of panels and their
  configs. Both are artifacts a person can hand to someone else and have work. A layout
  can hold canvases: a canvas is a panel that places other panels, images, text and shaders
  in depth under one camera, and one canvas can fill the window. An item in a canvas has a
  place, its content, one command from rox's own list that a click runs, a line of text
  that can show what's playing, and signals that move it. None of those is code. No
  scripting layer, for the reason under Out of scope.
- **Listening history is a record, not counters.** A real listen (a skip isn't a listen)
  is written to disk as an event with when it happened, keyed to track identity. History
  persists across rescans and file moves, and any stat someone thinks of later can be
  derived from what was kept. Data volume isn't a concern worth trading the raw record
  against. Recording never touches the audio path and never slows browse.
- **Panels pop out into real OS windows**, not fake in-app floats. Multi-monitor is the
  whole reason this matters.
