# ADR 30: Plugins are subprocesses that bring something external in

**Status:** Decided 2026-09-28, on the numbers in [research 04](../../0R-research/04-plugin-host.md); amended below

Decision: a plugin is a folder the user drops into rox's data directory, holding a
manifest and an entry program. rox runs it as a subprocess and talks to it over its
stdin and stdout. A plugin exists to bring something from outside rox in, and it
declares what it does from a closed set of capabilities. A source files its rows under
`plugin:<id>`, and rox browses, searches, syncs, and streams them through the plugin.
Panels are listed under the plugin's name in Add Panel as presets of core panel kinds.
A plugin source hands rox audio bytes, never a URL. Nothing a plugin supplies executes
in the UI, touches the engine or the ring, or connects to the control socket. rox
publishes the contract, a service-neutral example plugin, and the host. It never
publishes a plugin for a particular service ([scope](../../01-product/03-scope.md)).

The prototype for #8 measured what this record could only estimate, and the numbers are
in [research 04](../../0R-research/04-plugin-host.md). They set the timeouts, the read
size, whole-track buffering and the pre-open, recorded at the end.

The field-level shapes (manifest keys, wire methods, timeouts, size caps) are pinned in
a contract kept with the implementation plans, outside this repository
(`plugins-contract.md`), until the host's implementation doc exists to hold them. What
follows describes them in prose, so this record stands without that file.

**Plugins, where ADR 29 said extensions.** The scope doc and ADR 29 called these
extensions and gave them one job, sources. A source is still the first capability, and a
source needs a panel to browse and search it from. The line this ADR draws covers both:
a plugin reaches outside rox and brings the result back. It never changes how rox itself
behaves. A visualizer is the clearest case of what's out, since rendering is something
rox does rather than something it fetches.

**The host runs a subprocess.** A plugin runs as its own process with the user's
permissions, its own network, and its own filesystem access. rox doesn't sandbox it,
and the enable card says so in those words.

WASM with a spawn capability the host grants was the other candidate, and ADR 29 leaned
toward it (29:86-93). It lost on three counts. Its sandbox had a hole in the middle: a
WASM plugin could only reach a downloader through the spawn capability. The spawned
program runs unsandboxed, so the plugin ended up as trusted as the program it asked for.
A service with an ordinary web API would have needed a fetch capability in rox too,
since a WASM plugin has no network of its own. And every author would pay a toolchain
step to build the artifact, which is ADR 24's objection to WASM for scripts (24:43-48),
for a sandbox that already leaked. Either way the user ends up trusting someone else's
program. A subprocess says so plainly, and writing one needs nothing beyond a JSON
library.

In-process native libraries, the shape foobar2000's components take, were the third
option. They give a plugin the most reach and tie it to the host the most tightly. Rust
has no stable ABI, so it would be a C ABI rebuilt for every OS and architecture. A crash
in any plugin would take the audio engine down with it. A subprocess keeps the reach
without the coupling. A plugin that crashes ends its own track, and the host restarts
it.

A plugin built on a downloader runs that program itself. The manifest lists the programs
a plugin needs, and the Plugins page checks they're on PATH and names any that are
missing. The list informs the user and grants nothing. A subprocess can run whatever it
likes, and a declared list that implied otherwise would be a promise rox can't keep.

**Platforms.** A plugin has to run on the machine it's dropped onto, so the manifest
names its entry per platform. The entry is either a native binary per OS and
architecture (`linux-x86_64`, `windows-x86_64`, `macos-aarch64`, and so on) or a script
with an interpreter. The host resolves the interpreter per OS, since a Windows install
often has `py` where Linux has `python3`. A plugin with no entry for the current
platform is disabled with that reason on its row.

The host covers the rest of the difference. A plugin spawned on Windows gets
`CREATE_NO_WINDOW`, which convert's spawn path already sets because a console program
otherwise opens a window (`rox/src/convert.rs:46-63`). On quit the host kills each
plugin's whole process tree, through a job object on Windows and a process group on
Linux and macOS. The plugin's side is one rule: exit when stdin closes. When rox
crashes, stdin closing is the one signal that reaches a plugin on every OS.

**Identity and trust.** A plugin's identity for trust is the SHA-256 of its whole
folder, manifest included: every file's relative path and contents, in sorted order.
Any change turns the plugin off until the user turns it on again, so an update that
adds a capability or turns on scrobbling can't keep an old approval. A plugin that wrote
into its own folder would change its own hash, so the handshake hands each plugin a
separate data directory to write in. Dependencies outside the folder, like packages
installed globally for an interpreter, aren't covered. An author who wants them covered
vendors them into the folder.

Approved hashes are kept in `session.json` beside the shader approvals
(`rox-core/src/settings.rs:627-631`), machine-local for the same reason: a copied
settings file mustn't bring someone else's trust decision with it.

The opt-in has two layers, the pattern the AI features already use. A settings switch
reveals the Plugins page, the way the AI switch reveals the MCP and ML Models pages
(`rox/src/settings/window.rs:1455-1460`). Each plugin then has its own switch, and
turning it on is the approving act. The enable card says the plugin runs as a program
with the user's permissions and lists its capabilities and programs in words. On
re-approval it shows what changed in the manifest since the last approval. There's no
second dialog behind the switch, for ADR 24's reason (24:86-90): one gate and one habit,
since a second "are you sure" trains the user to click through both.

The gate doesn't defend the machine against local software. Any program that can write
the plugins folder can write `session.json` too, and rox already runs an ffmpeg dropped
into its data folder with no gate (`rox/src/convert.rs:81-85`). The switch enforces the
rule the shader gate follows, that only a direct user action approves code to run
(`rox-panel-api/src/panel/shader.rs:266-271`). It also shows the user what a plugin
declared before any of it runs. Signing was one alternative, and it needs an authority
rox has chosen not to be. A prompt on every launch was another, and it trains the same
click-through.

Plugin config, secrets included, goes in `accounts.json`, in plaintext, for [ADR
14](14-adr-online-providers.md)'s reason (14:54-57).

**The capability set.** Two capabilities, each granted only if the manifest declares
it:

- `source`: rows under `plugin:<id>`, browsed, searched, synced, streamed, and given
  covers through the plugin.
- `panels`: presets of core panel kinds, listed under the plugin's name. Nothing
  executes.

Several things stay outside the set. UI code and node trees wait on [ADR
24](24-adr-script-panels.md), which is Proposed and has its own gate. A plugin that
returned a node tree would be a script panel with network reach, the fragility the scope
doc's refusal exists for. Audio processing and engine hooks would put plugin code on the
decode path the callback invariant protects ([ADR 19](19-adr-processing-chain.md)) or
into the timeline the engine alone owns ([ADR 16](16-adr-play-queue.md)). A plugin
hands the engine container bytes, and decode stays in core. Controlling playback
somewhere else, which the scope doc's Tapped and Remote tiers need, isn't in the set
either. It gets a capability when a real plugin needs one.

Verbs into rox are out for now because nothing needs one. Every source call is one rox
makes into the plugin, so plugins never call back. When one does, it goes through a
typed allowlist in front of the socket's route arms. `route` takes the method and
params as plain values (`rox/src/integrations/ipc.rs:108-118`), so an allowlist can go
in front of it without a plugin ever holding a socket connection.

Adding a capability is a product decision, per the scope doc, and an amendment here.

**The wire, and why plugins never touch the socket.** A plugin speaks newline-delimited
JSON-RPC 2.0, the shape the control socket uses (`rox-ipc/src/protocol.rs:1-3`), over
the stdin and stdout rox spawned it with. It's a separate wire, so the two can version
apart. The socket authenticates by filesystem permission ([ADR
22](22-adr-control-surface.md), 22:34-36), so it can't tell a plugin from `roxctl`.
Giving plugins a scope on it would mean bolting an identity story onto a surface built
without one. The pipe needs no identity story: rox spawned the process, so nothing else
can be on the other end.

The host calls a handshake (API version, the plugin's config, its data directory),
browse, search, sync, open, read, close, cover, and shutdown. The plugin calls nothing
back. Every inbound frame parses into a typed shape that rejects unknown fields, and a
frame that doesn't parse is a plugin error, never a panic. Strings, page sizes, and
frames are capped, frames at the socket's 1 MiB (`rox-ipc/src/server.rs:21-22`). Every
call has a timeout, which rox's existing spawn path doesn't have: convert's runner polls
for a cancel with no wall-clock limit (`rox/src/convert.rs:898-913`).

There's one process per enabled plugin, started on first use, with its stderr logged
under the plugin's id. A crash restarts it with backoff, and a plugin that keeps
crashing is disabled with the reason on its row. It's shut down on disable and on quit.

**Browsing, searching, and syncing.** A source plugin exposes a tree. Its root nodes are
whatever the service has for the user, such as liked tracks, saved albums, playlists,
and followed artists. Browse pages through a node's children, which are more nodes or
tracks, with a cursor. Search takes a query and returns the same kind of page, so a
search can turn up albums and playlists as well as tracks.

Every track has a key the plugin chooses. rox treats it as opaque, and it has to stay
stable across sessions and plugin versions, because it's the row's path. A plugin that
changes its key scheme orphans every row it made.

A node the plugin marks as a collection can be synced. The user switches sync on for
their liked tracks or one playlist, and rox pages through it and writes every track as
a row. A version token lets the plugin answer that nothing changed, since services
rate-limit a client that re-reads ten thousand likes on every launch. Synced rows are
local, so library search covers them without touching the network.

Browse and search in the plugin's panel are live and do touch the network. That's why
searching a plugin is its own surface and never part of the shared query ([ADR
15](15-adr-global-filter.md)): the scope doc's local-first constraint says search never
depends on the network. Tracks the user plays, queues, or adds to a playlist from the
panel become rows one pick at a time, since the queue and playlists are made of library
rows. How synced and picked rows are kept and pruned is ADR 29's second amendment.

**Audio arrives as bytes.** Opening a plugin track asks the plugin to open a stream. It
answers with a stream id, a container hint, the total length when it knows it, and
whether the stream can seek. rox then pulls bytes with reads at an offset and length,
and closes the stream when it's done. The engine decodes those bytes like a file, so
gapless, ReplayGain, and visualizers work unchanged. A seek is a read at a new offset.
A live stream reads front to back and can't seek. How the engine opens and pre-opens
these streams is ADR 29's first amendment.

Everything specific to the service stays inside the plugin: auth, URLs and when they
expire, CDNs, request headers, and joining a segmented stream into one continuous file.
The plugin knows its service and rox doesn't.

The alternative was a URL. The plugin would resolve a track to a URL and headers, and
rox would fetch it with its own HTTP source. That fails in four places. A streaming
service's URLs expire, sometimes while the track is queued and sometimes halfway through
it. A fresh URL for the same track can point at a different encode, and `HttpSource`
resumes a reconnect at the old byte offset without checking the new response's length
(`rox-playback/src/http.rs:706-722`), so the decoder would get the middle of a
different file. Many services serve segmented streams, and rox has no assembler for
them. And rox would fetch any URL a plugin named, with any headers it named, which
needs an address policy of its own. All four become the plugin's problem once the
plugin serves bytes.

A plugin that only has decoded samples can serve them in a container rox already
decodes, such as WAV, so the PCM contract ADR 29 deferred may never be needed. That's
inferred from the formats the engine reads. Nobody has tried it.

Reads go over the same pipe as every other call, base64 in the JSON result. Encoding
adds a third to the byte count, so a lossless stream at about 1 Mbit/s becomes about 170
KB/s on the pipe. That's arithmetic, and the prototype measures it. A second channel
with binary frames was the alternative. It saves the encoding and costs every author a
second connection and a binary framing to write, for a bandwidth problem the arithmetic
doesn't show. A read stays under the 1 MiB frame cap once encoded. The host reads ahead,
so the decoder rarely waits on a read. Requests have ids, so a plugin can answer reads
while a slow search is still running. A plugin that answers one call at a time stalls
its own playback behind its own searches, and the example plugin shows how to avoid
that.

**The manifest.** An id (a lowercase slug, fixed once), a display name, the author's
version, the API version it targets, the entry per platform, author metadata in
`WorkspaceMeta`'s shape minus the dates (`rox-core/src/settings.rs:2446-2468`), the
declared capabilities, the programs it needs, and a JSON Schema for the plugin's config.
The Plugins page renders a small subset of JSON Schema as rows: strings, secrets,
numbers, booleans, and enums. There are no per-locale strings, so a plugin's labels
show in its author's language.

Unknown top-level keys are rejected, and so are unknown keys inside the entry, since
that's how the plugin runs. Inside `meta` and `capabilities` unknown keys are ignored,
so a field added there later doesn't refuse a plugin on an older rox; a new top-level
key bumps the API version. That's new for rox: the workspace bundle reads leniently so
an old look still loads, and nothing in the workspace rejects an unknown field today. A
manifest is a different kind of file. A key the host doesn't know is a key it can't
enforce, and a plugin that relies on one should fail loudly.

The API version is an integer. The host supports a range of versions and accepts a
plugin that targets any version in it, so one rox release doesn't break every plugin at
once. Additive changes, such as a new optional field, don't bump it. The prototype runs
at 0. Version 1 is fixed after the prototype, once the wire has had a real plugin on the
other end.

**Rows under `plugin:<id>`.** A plugin's source id is `plugin:` plus its manifest id,
fixed once and kept in rows and layouts for good. ADR 29's identity rules apply
unchanged. What changes is how the services layer finds which non-local ids are live.
Today that's a `subsonic:` prefix check in three places: hiding rows whose source is
switched off (`rox-services/src/sources.rs:340-342`), sweeping rows whose account was
removed (`sources.rs:208-214`), and fetching covers (`sources.rs:582-600`). Under those
checks a removed plugin's rows would never be swept. One function that returns the live
ids, from Subsonic accounts and plugin records together, replaces all three.

A plugin whose folder disappears shows as missing on the Plugins page, and its rows are
hidden like a switched-off source's. Nothing is swept, because deleting the old folder
is how many people will update a plugin. The rows go only when the user removes the
plugin on the Plugins page.

A plugin source scrobbles only if its manifest declares it, and the user can still turn
that off. Nothing in the scrobblers checks where a row came from
(`rox-services/src/lastfm.rs`, `listenbrainz.rs` and `librefm.rs` have no source test),
so without an explicit default every plugin play would scrobble.

Capture is never available to plugin rows. It only tees streams that contain ICY
metadata: `rox-playback/src/http.rs:774-784` wraps a body only when the server announces
a metadata interval, and the tee is installed inside that wrapper (`icy.rs:105`). A
plugin stream never passes through `HttpSource`, so capture can't reach it today.
Recording the refusal here gives a later change to capture a rule to check against.

**Panels: a Plugins category in Add Panel.** Add Panel grows a Plugins section listing
each enabled plugin by name, and under each, the panels it declares. A declared panel is
a preset of a core panel kind: the same saved dump a panel preset already is, built by
the path presets already take (`rox/src/panel_presets.rs:49-74`). The manifest names a
core panel and its config. It can't name a kind the binary doesn't have, or hold a
container.

A plugin source's own panel is a new core panel kind, the source browser, pinned to one
source id. It shows the plugin's tree, a search box, and a sync switch on each
collection. The nearest existing panel is the station directory, which searches a
remote directory and writes the stations the user picks
(`rox/src/station_directory.rs:188-222`). The source browser is core code, and a plugin
supplies none of it.

The alternative kept plugin names off the menu: the same source browser, reached from
the ordinary catalog with a source picked in its config. The mechanism is the same.
Only the name on the entry differs. It's the cheaper option. The catalog is a static
list (`rox/src/panel_catalog.rs:632`) whose build step is a plain function pointer
(`panel_catalog.rs:67-76`), so a section built at runtime is new code. Plugin-supplied
labels also can't go through the two tests that hold every catalog label to a message
key (`panel_catalog.rs:695`, `:721`). It lost on product value. A plugin's panels under
its own name are how a user finds what the plugin added.

A plugin panel keeps its core `panel_name` and records its plugin in an optional owner
field on the chrome every panel config flattens in
(`rox-panel-api/src/panel.rs:982-986`). The other shape, a panel name registered per
plugin, breaks the placeholder a layout gets for a missing panel. The dock builds its
invalid-panel stand-in only for a name nothing registered
(`rox-dock/src/panel.rs:414-436`), so a registered plugin name would need a placeholder
of its own. With the owner field, a layout that outlives its plugin restores the core
panel with the plugin's rows hidden, and an older build reads the config as it always
did.

A workspace bundle that contains plugin panels lists the plugins it needs in a
`requires` field, derived from those owner fields. The apply card names any that are
missing. The field doesn't exist yet (`WorkspaceBundle`,
`rox-core/src/settings.rs:2350-2352`) and arrives with the first plugin panel. An older
build drops it silently, which costs that build only the warning.

This narrows the scope doc's refusal of scripted UI extensions for presets only: a
plugin can put entries on the menu, and every entry is data. ADR 24's script panels stay
Proposed and separate, and plugin-supplied node trees wait on them.

**What the #8 prototype measured.** The prototype ran one external plugin, built on a
downloader the user installs, through the real folder, manifest, version check and wire,
never linking rox's crates. ADR 29 is the reason (29:78-81). What it found settles the
open numbers:

- A cold open took 2.3 to 3.8 s when the plugin ran its downloader, a median of 2.7 s
  over twenty. The open timeout stays at 20 s. Starting a plugin and its handshake took
  under 0.8 s, so the handshake's 5 s stays.
- A cold open does stall the engine. A pause pressed during one waits out the rest of
  it, measured at up to 2.3 s, because commands drain at the top of the decode loop.
  Pre-opening the next two entries answered every sequential advance in the test from a
  stream already open, in under 2 ms, so the stall is confined to jumps and the first
  play, and the transport, the waveform and the track's row show that wait while it
  lasts. The pre-open waits for the audible track to hold for 2 s, so skipping through a
  queue doesn't pay for opens it throws away.
- The pipe is not a bandwidth problem. Base64 in JSON carried 14 Mbit/s at the worst
  read size through the real plugin and 130 to 148 Mbit/s through a plugin with no
  network behind it, and decoding a full read cost the host 0.3 ms. A second binary
  channel stays unneeded.
- Reads are 256 KiB. That got nearly all of 512 KiB's throughput through the real plugin
  and four times 64 KiB's, and a seek cost the same one round trip, a median of 68 to 87
  ms, at every size.
- A seekable stream up to 64 MB is downloaded whole while it plays, rather than read a
  chunk ahead, and so is a server's file answered by range. The seekbar shows what's
  downloaded, a seek inside it touches nothing but memory, and the waveform is decoded
  from the same bytes, so the track is fetched once. Plugin tracks of 3.5 to 5.7 MB
  downloaded in under a second. Anything larger, unseekable or live reads as it plays,
  and a plugin may ask for that too, for a service that meters or throttles fast
  downloads: it can lower the buffering, never raise the cap. A pre-opened stream
  downloads only once it plays.
- A sync's first page may take 60 s, since a plugin may list a whole collection there to
  learn whether it changed. Every other listing page keeps 15 s.
- A plugin in an interpreted language cost about a thousand lines of standard-library
  Python and a test suite of about eight hundred, on macOS and Linux. The per-step time
  wasn't recorded.

The first page's 60 s is a margin, not a measurement: no collection of thousands was
synced.

**The contract for the implementing layer.** rox-library gains a plugin origin beside
Subsonic's (`rox-library/src/cue.rs:68`), a third `Locator` variant for a plugin
stream, the membership table, and the source in playlist member snapshots. rox-playback
opens a plugin stream through an opener passed in with the queue, per ADR 29's
amendment. rox-services gains the one live-id function, collection sync, the pre-open,
and a plugin module that installs each enabled plugin's opener. rox-core gains plugin
records in `accounts.json` (the id, the switch, a cached label, the folder hash and
manifest at last approval, the scrobble choice, the synced collections, and config),
the Plugins switch in `settings.json`, the approved hashes in `session.json`, and a
plugins folder and per-plugin data folders under the data directory. rox doesn't create
those until the user asks, as with MilkDrop's (`rox-core/src/settings.rs:182-186`). A
new host crate owns the manifest, the wire, the process lifecycle, and the per-OS spawn.
The settings window gets a Plugins page behind the switch, Add Panel gets the Plugins
section and the source browser, panel chrome gets the owner field, and the bundle gets
`requires`. All of it stays behind the experimental gate until the opt-in ships.

**Amended 2026-09-29: one switch opts in, and an author can approve their own saves.**
The switch that lets plugins run moves to the head of the Plugins page, and the page is
always listed. It replaces the two layers above: Experimental Panels on one page, then
Enable Plugins on another, before the Plugins page appeared at all, made plugins hard to
find. The experimental gate is gone with it. The Plugins switch is the opt-in the last
paragraph was waiting on, and it's off by default.

Every save an author makes changes the folder's hash, which switched their plugin off
and put the card in front of them each time. Developer mode, a per-plugin toggle beside a
switched-on plugin's switch, approves those changes on its own for the rest of the session, as
long as the manifest's diff against the last approval is empty. A diff that isn't empty
still goes to the card, so a plugin can't gain a capability, a program, scrobbling or a
new entry without the user seeing it. The toggle is never saved, so a launch approves
nothing by itself. This narrows "any change turns the plugin off" and keeps the shader
gate's rule: turning the toggle on is the direct user action, and it approves the saves
that follow it. There's still no second dialog.

A browse or search page can carry a notice: a line of the plugin's own text, marked
`info` or `setup`, which rox shows above the page with a way to the plugin's settings for
`setup`. Before it, a plugin with nothing to list until the user set something up could
only answer an empty page or an error, and neither says what to do. It joins API 1
rather than starting API 2, since a plugin that doesn't send one is unchanged. A host
from before it refuses a page that carries one, so `hello` now lists the optional
features the host reads, and a plugin sends a notice only when `notice` is among them. When the source itself
can't answer, rox says why in its own words (plugins off, the plugin switched off,
changed on disk, gone, failing to load, stopped after crashes) and offers the Plugins
page, rather than passing on the host's internal error.

**Amended 2026-09-29: radio, fields, and the panel's shape.** Andrew's product calls
after the first external plugin met the source browser.

- A source may declare `radio` in its capability. rox then offers Start Radio (Play
  Similar since, see below) on the
  plugin's tracks and nodes, and asks `source.radio` with the seed and a cursor for
  batches of tracks. The first batch plays at once and the rest arrive through queue
  continuation ([ADR 17](17-adr-queue-continuation.md)'s amendment), so the engine keeps
  the timeline and the tracks play through rox like any other plugin track. Controlling
  a service's own player is still outside the set: that's the Tapped and Remote tiers,
  and this isn't them.
- A page may declare up to four `fields`, columns the service knows and the tags don't,
  with values on its tracks and nodes. The source browser shows them and sorts by them,
  reading the rest of the list first up to a thousand rows, since a sort over one page
  is a wrong order. They live in the panel only. A kept row holds its tags and nothing
  from a field, so no library column depends on a plugin being there.
- A page may offer `views`, filters or orders the service applies, and entries may be
  `section` headings or nodes with a `kind` and `art` for a cover. A section may ask
  for `tiles`, which shows its entries as a shelf of covers scrolled sideways, so a
  plugin can bring a service's home or new releases as the screens they are. The list
  moved to a virtual list with a height per item to hold them. A page that's only one
  tiles section wraps into a grid, since it has no rows for a shelf to stack against.
- A root node may mark itself the service's `home`. The source browser lists the roots
  at once and follows them with that node's pages, so the service's home reads as the
  bottom of rox's own Home rather than a node inside it, and the roots never wait on
  what's usually the service's slowest page. A panel setting, on by default, puts it
  back as a node.
- A source may declare `links`. rox then offers Open in Browser and Copy Link on its
  tracks and nodes in every menu that lists them, and asks `source.link` with the key or
  id when one is picked. The menu items are rox's own, since a plugin still adds no
  behaviour inside the UI. The link is asked for at click time and never stored, so a
  kept row needs no new column and a changed URL is never stale. rox opens it in the
  browser or copies it, and never fetches it, the rule notice links already follow.
- A radio plays what it started from before the station: the track, or everything a
  node lists. A node's play items (Play, Play Next, Add to Queue) read the same list. A source may ship an
  `icon`, a square SVG drawn as a mask in the theme's colour.
- Each wire addition is gated by a name in `hello`'s `features`, so a plugin never sends
  what an older host would refuse. The manifest additions sit under `capabilities`,
  which ignores keys it doesn't know, so `api` stays 1.

**Amended 2026-09-30: a stream's length.** A fragmented MP4 without a segment index,
which is what a plugin gets by joining a DASH service's segments, doesn't state its
length. Without one the seekbar can't seek and the waveform has no timeline to lay bars
on. `source.open` may now answer `duration_ms`, gated by `open-duration`. The
container's own length still wins, the plugin's stated one comes next, and the track's
`duration_ms` from browse or sync is the last resort. A stated length stands in for the
container's everywhere the engine reads one. The decode still ends where the bytes do.

**Amended 2026-09-30: a station with no seed.** A service offers a station before
anything plays, and Start Radio with nothing to seed one did nothing. A source may
declare `personal_radio` beside `radio`, and then `source.radio` may come with a null
seed. The plugin picks the station and names its seed in the answer, so from there on
it's an ordinary seeded station: continuation, saving and the run-out reseed don't
change. The declaration sits in the manifest rather than `hello`, since it's the
plugin's ability the host asks about, and a host that doesn't know it never sends a
null seed.

**Amended 2026-09-30: Go to.** A track in the source browser can open its album or one
of its artists, the way a service's own client does. A track may carry `go_to`, gated by
`go-to`: the nodes its album and artists open, in the shape of browse nodes. rox builds
Go to in the track's menu from them. A node already on the trail is stepped back to,
and an album or artist page gives way to the next one, so hopping between them doesn't
stack crumbs. From anywhere else the node opens on from the place shown, so going back
returns to the track. The nodes ride on the page instead of coming from a method
asked at click time, because a menu is built whole when it opens: an answer that arrives
later can't list several artists by name or hide an album the track doesn't have. Go to
lives in the source browser alone, where a plugin's nodes can open. Sync ignores
`go_to`, so a kept row holds its tags and nothing more.

**Amended 2026-09-30: Play Similar instead of radio buttons.** Andrew's product call
after living with the radio. The source browser's header button grew a station from
whatever the place offered, which meant the first track of a playlist or the same
favourite on every press, and it did what Similar shuffle already does under another
name. The button is gone, and so is `personal_radio`, which only it asked for: a host
no longer sends a null seed, and a manifest that still declares it loads as before,
since `capabilities` ignores keys it doesn't know. Start Radio in a row's menu is now
Play Similar. It plays the station and turns Similar shuffle on. A node's tracks
still lead its station, but a track only seeds one and never plays in it, the way the
library's own Play Similar leaves out the track it started from. A running
plugin with a radio is enough to offer Similar without acoustic analysis, since its
own tracks draw from its station. A local queue under Similar with nothing analyzed
has no neighbours to draw, so its refill widens into the weighted draw, the same way a
thin pool does. The random
button's Similar draw starts Play Similar from a playing plugin track. Continuing a
queue started from a plugin's panel with its radio doesn't change.

**Amended 2026-10-01: actions.** Andrew's product call, after wanting a downloader plugin
to save the video behind a track. A source may declare `actions`: things the plugin does
with one of its tracks, one of its nodes, or no item at all, each with an id, a label and
where it's offered. rox lists a track or node action in every menu that lists the
plugin's tracks and nodes, the same places Open in Browser shows, and on a selection of
several as well as on one. An action on no item goes in the External Sources panel's own
menu. The menu item is
rox's, with the plugin's label, which isn't translated, like everything else a plugin
sends.

An action may declare `params`, a JSON Schema in the subset the Plugins page draws for a
plugin's config. rox shows those rows in a dialog when the action is picked and calls the
plugin once the user confirms. A download that asks for a quality, or an action on no
item that asks for a URL, is a form the plugin describes and rox draws.

Picking an action calls `source.action` with its id, the items (tracks' keys or nodes' ids,
empty for an action on no item) and the params. A selection is one call and, when the
plugin answers with a job, one job, so the plugin decides how to batch it. The answer is either a message, which rox shows in a toast, or a
job id for work that outlasts a call. rox polls a job with `source.job` about once a
second for its progress and a line of status, and lists it in the Tasks window with a
Stop that sends `source.cancel`. Both calls take the listing timeout. A job itself has no
wall-clock limit, since a download takes as long as it takes. It ends when the plugin
reports it finished or failed, or when the plugin stops, and rox reports the outcome in a
toast. A failed job's toast stays until it's dismissed.

Polling keeps every call one that rox makes. Progress the plugin pushed would be the
first frame a plugin sends unasked, and the paragraph on verbs above puts any callback
behind a typed allowlist. Progress doesn't justify one: a poll a second is a small
request next to the 256 KiB reads the same pipe already serves.

What a result can do stays rox's to decide. A message is text. A result may also name an
http or https link, which opens in the browser on a click, and a local path, which rox
shows in the file manager on a click and never opens or runs. They become the toast's
buttons, so a finished download offers Show in Folder.

Actions are the first capability that has a plugin do something instead of list
something, and they stay inside the line this record draws. The work runs in the
plugin's process, which already has the user's permissions and its own network, so an
action gives the plugin no reach it didn't have. The user gets a button for it. rox draws
the menu item, the form, the progress and the result, and nothing the plugin supplies
runs in the UI.

The declaration goes under `capabilities.source`, which ignores keys it doesn't know, so
`api` stays 1, and rox sends `source.action` only to a plugin that declares actions. The
enable card lists them by label. Adding or changing an action changes the manifest, so it
goes back to the card, and Developer mode doesn't approve it on its own.

**Amended 2026-10-02: Go to on kept rows.** Andrew's product call, after finding a kept
album's tracks in the library offered no Go to while the same tracks browsed did. Sync no
longer ignores `go_to`: rox keeps it beside the row, and a row added or picked from the
browser keeps the one it was listed with. The library's view of a source offers Go to
from what's kept, and a target the library doesn't keep opens from the plugin as it would
from a browsed row. A listing without `go_to` leaves the kept one alone, so a plugin's
older answer can't erase it. Collections kept before this sync once without their token,
so their rows fill it in.

**Amended 2026-10-02: action icons and conditions.** Andrew's product call, after the
favourites plugin offered Add to Favourites and Remove from Favourites on the same
track, both behind a plug. An action may name an `icon`, an SVG in the plugin's folder
held to the source icon's rules: inside the folder, small, drawing nothing from outside
itself. A bad one refuses the plugin with the reason, the way a bad source icon does,
rather than quietly falling back to the plug. It's drawn as a mask in the theme's colour,
so the plugin supplies a shape and rox does the drawing.

An action may also name a `when`: a flag the rows carry (`favourite`) or lack
(`!favourite`). Rows carry `flags` on the wire, gated by the `flags` feature. rox
offers the action when any picked row can take it, so a mixed selection shows both
halves of a pair. A row whose flags the plugin didn't say can take any action. A library
row has no flags of its own, so its menu reads the newest ones the session saw for its
track, from a listing or an action, and takes any action when there are none. Flags ride on the rows for the same
reason Go to does: a menu is built whole when it opens, and asking the plugin then would
hold it up.

An action's answer, or a job's last state, may carry `flags` for the items it changed.
rox merges them into what's on screen, so the menu after Add to Favourites offers
Remove without listing the place again, which would drop the pages already scrolled
through. Flags are hints for a menu, never a record: sync ignores them, and rox keeps
the ones it saw only for the session.

**Amended 2026-10-02: asking for a library row's flags, and Go to from any menu.**
Andrew's call, after a kept album's tracks in the library still offered both halves of
every pair: rows the library builds from tags carried no flags, and the ones the session
had seen in listings didn't reach them. rox now asks with `source.flags`, a list of keys
in and their flags out, once a plugin is up and again after each sync. It's still a call
rox makes, and only to a plugin that gives an action a `when`. A plugin that doesn't
answer it leaves its rows unknown, which offers every action, as before. The answers
are hints like any other flags: kept for the session, and replaced by a newer listing or
action report.

Go to also leaves the source browser. A row's kept Go to puts the entry in every menu
the row has, the queue's and the library's included, and the node opens in an External
Sources panel on that plugin: the one whose menu was open, else one in any tab group,
else a new one. The panel still does the opening, so a node still only opens where a
plugin's nodes can.

An answer that's only a `reveal`, with no message or link, shows the path in the file
manager at once instead of offering it on a toast. The click on the action is already
the user asking for it, so Show in Folder takes one click, not two. rox still only ever
shows the path and never opens it.

**Amended 2026-10-02: the locale in `hello`.** Andrew's call, after asking whether a
plugin can localize itself and finding it can't: everything a plugin supplies shows as
written, and nothing told it which language the user reads. `hello` now carries
`locale`, the interface language as a BCP 47 tag, so a plugin can write its own text in
it: listings, errors and action messages. It's a parameter rox sends, not something it
reads back, so it isn't a feature, and a plugin treats a missing one as unknown. rox
takes it when the plugin starts and doesn't restart a running plugin on a language
switch, which would cut off what it's playing. Manifest text (labels, action labels,
settings titles) still shows as written; a per-language map for it waits until a plugin
ships translations.

**Amended 2026-10-02: chapters and lyrics.** Andrew's product call, after skipping
through a long episode on a plugin and finding the service's own chapters and lyrics had
no way in. Both are things a service already knows about its own tracks, so the plugin
supplies them and rox draws them.

`source.open` may answer `chapters`, gated by the `chapters` feature: where the
stream's parts start, each a start in milliseconds and a title. They ride on the open
rather than the track, so only the track that plays pays for them, and a listing stays
the size it was. The engine never reads them. The wire refuses only their shape, and rox
tidies the order and drops blank titles itself, because a malformed list must not stop
a track playing. The seek strip draws them off its top edge as the cues' chevron, fainter,
since a chapter is the track's and a cue is the user's. Hovering one names it and a click
seeks to its start. A station ignores them: its top edge is the tape's songs.

A source may declare `lyrics`, and rox then asks `source.lyrics` for a track's sheet, LRC
or plain. Unlike links, the declaration alone doesn't reach the user: each plugin gets a
Lyrics switch on the Plugins page, off by default and never turned on by approval,
because the answer overrides the lookup the user configured and is saved into their
lyrics store. With it on, the Lyrics panel asks the plugin first for its own tracks and
saves the answer without the confidence bar automatic lookups have to clear, since it's
for that very track. That doesn't break [ADR 14](14-adr-online-providers.md)'s rule that
candidates rank by confidence, not by who answered: an exact-track answer is confidence
1. The No Lyrics mark still stops it, and a plugin with no sheet falls through to the
providers. The Providers page's online switch keeps covering the built-in providers
only, so each source has one switch and no second one controls the same thing.

The declaration sits under `capabilities.source` beside `links`, so `api` stays 1, and
turning it on shows on the re-approval card as a new capability.

**Amended 2026-10-05: actions from MCP.** An MCP client can browse and search a plugin
and run its actions through the control socket, behind a switch of its own. [ADR
22](22-adr-control-surface.md)'s amendment has the surface and the gate. The plugin
hears the same `source.action`, `source.browse` and `source.search` a menu or the source
browser sends, so nothing in this record moves.

**Amended 2026-10-05: favourites that follow the heart.** Andrew's product call: a heart
on a plugin's track should reach the service the track came from. A source may declare
`favourites`, naming two of its own track actions, one that adds to the service's
favourites and one that takes away. One switch on the Plugins page, Sync Favourites, off
by default, has rox run them when a heart moves on that plugin's tracks. rox diffs the
library's favourite set the way the Last.fm mirror does, so every way of moving a heart
counts, and favourites from before the switch went on are never pushed. Hearts the
Last.fm import writes are absorbed the same way.

It adds no wire method. The calls are `source.action` with the declared ids, the same
ones a menu sends, and rox reads the service's side from the rows' `favourite` flag,
which `source.flags` and action answers already carry. The plugin does nothing it
couldn't do from a menu, and the user's switch is what lets a heart reach it.

While the switch is on, a heart whose two sides disagree draws half: a favourite in rox
and not on the service, or the other way round. That's a favourite from before the
switch, a push that failed, or one the user made on the service directly. A click on a
half heart favourites on both sides, and a click on a full heart takes it off both. rox
never adds a heart by itself because the service has one. Flags are still hints kept for
the session, so a row the plugin said nothing about draws its local heart.

The declaration sits under `capabilities.source`, so `api` stays 1, and the re-approval
card shows it as a new capability.
