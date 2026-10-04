# Plugins

How the plugin host is built: the folder a plugin is installed in, the manifest, the wire rox
speaks to it, how its rows enter and leave the library, how its streams reach the engine,
and the switch that approves it. This makes [ADR 30](../02-architecture/decisions/30-adr-plugins.md)
concrete, along with ADR 29's amendments for plugin streams and rows by membership
([ADR 29](../02-architecture/decisions/29-adr-source-contract.md)). The numbers behind
the timeouts, the read size and the buffering are in
[research 04](../0R-research/04-plugin-host.md). A plugin author can start from the
example in [`examples/plugins/tones`](../../examples/plugins/tones/).

Version-sensitive: the host speaks plugin API version 1 and nothing else
(`SUPPORTED_API = 1..=1`, `rox-plugins/src/manifest.rs:42`). The wire is JSON-RPC 2.0,
one object per line. A plugin's bytes are decoded by symphonia 0.6, so its containers and
codecs are the ones the engine reads. Folder hashes are sha2 0.10's SHA-256, the folder
watch is notify-debouncer-full 0.7, and the Windows job object is windows-sys 0.61.

## Using a plugin

The README in the Linux and Windows release archives gives a user these same steps, and
the Plugins page links to [README_PLUGINS.md](../../README_PLUGINS.md), the author-facing
guide drawn from this document.

1. Plugins are behind one switch, Enable Plugins at the top of Settings > Plugins
   (`set_plugins_enabled`, `rox/src/settings/window/plugins_page.rs`). The rest of the
   page lists only while it's on.
2. Reveal Folder on the Plugins page creates the plugins folder and opens it. rox never
   creates it on its own.
3. Drop the plugin's folder in. The folder's name has to be the plugin's id. The page
   picks it up on its own, with its switch off.
4. Switch it on. The first time, a card says what the plugin is and asks to confirm.
   Confirming approves exactly the files in that folder.
5. If any file in the folder changes, the plugin switches itself off until it's switched
   on again, and the card shows what changed. Developer mode approves the change instead
   when the manifest declares nothing new.
6. A plugin source's tracks are browsed and searched in the External Sources panel:
   Add Panel > Plugins > the plugin opens one on its source, and the bare panel is in
   the Catalogue group while plugins can run (`live` in `rox/src/panel_catalog.rs`).
   Keep in the library on a collection syncs it into the library, and Add to Library
   adds a single track. Playing, queueing or adding a track to a playlist plays it
   without adding it.
7. Remove on the Plugins page drops the plugin's tracks, synced collections, settings
   and approval. Its folder is left alone.

The Plugins page links to that guide at the running build's release tag
(`GUIDE_URL`, `rox/src/settings/window/plugins_page.rs:24-28`).

## The folder

```
<data>/plugins/<id>/        the plugin, as the user dropped it
    plugin.json
    <entry and anything else it ships>
<data>/plugin-data/<id>/    where the plugin writes, created on its first start
```

`<data>` is rox's data directory, `rox-data` beside the executable in portable mode
(`plugins_dir` and `plugin_data_dir`, `rox-core/src/settings.rs:190-198`). The plugin is
told its data directory in `hello` and should write nothing anywhere else inside rox's
data. A write into its own folder would change its hash and switch it off.

A scan reads every subfolder, parses its manifest, hashes it and checks the programs it
lists (`scan`, `rox-plugins/src/loader.rs:60`). It runs nothing. A folder that can't
load still comes back with its reason, which the page shows under "This plugin can't
run". A stray file beside the folders is skipped. A folder whose manifest `id` isn't
the folder's name is refused (`loader.rs:111-115`), because rows are filed under the id
and the host finds the plugin by its folder.

The watch follows the plugins folder, and follows its parent until the folder exists
(`Watch`, `loader.rs:132-209`). Events are hints to scan again, 500 ms debounced; a scan
that finds what the last one found changes nothing (`rescan`,
`rox-services/src/plugins.rs:354-380`). macOS reports event paths with symlinks resolved,
so the watch matches both spellings of the folder.

## The folder hash

The hash is the plugin's identity for trust: SHA-256 over every file under the folder,
each fed as `<relative path>\0<length as u64 little-endian><bytes>`, in byte order of the
relative paths, with `/` as the separator on every OS (`folder_hash`,
`rox-plugins/src/hash.rs:17-38`). `.DS_Store`, `Thumbs.db` and `desktop.ini` are skipped,
since the OS writes them and nothing runs them. `__pycache__` is not skipped: Python
would run a planted `.pyc`.

A symlink anywhere inside refuses the plugin (`hash.rs:58-60`), and so does a plugin
folder that is itself a symlink (`loader.rs:73-75`). Following one would let an approved
hash stand for bytes outside the folder. A file name that isn't UTF-8 is refused too,
since it can't hash the same on every OS.

The host sets `PYTHONDONTWRITEBYTECODE=1` for every plugin
(`rox-plugins/src/process.rs:44`), so a Python plugin's first import doesn't write
`__pycache__/` into its own folder and change its own hash.

## The manifest

`plugin.json`, at most 256 KiB (`rox-plugins/src/manifest.rs:47`). The tones example's:

```json
{
  "id": "tones",
  "name": "Tones",
  "version": "0.1.0",
  "api": 1,
  "entry": {
    "script": { "path": "tones.py", "interpreter": "python3" }
  },
  "meta": {
    "author": "rox",
    "description": "An example source plugin. ...",
    "website": "https://github.com/zealsprince/rox",
    "license": "AGPL-3.0-only"
  },
  "capabilities": {
    "source": { "label": "Tones", "scrobble": false }
  },
  "programs": [],
  "config_schema": {
    "type": "object",
    "properties": {
      "volume": { "type": "integer", "title": "Volume", "minimum": 1, "maximum": 100 }
    }
  }
}
```

| Key | Meaning |
|---|---|
| `id` | `^[a-z0-9][a-z0-9-]{1,63}$` (`valid_id`, `manifest.rs:111-118`). Its rows are filed under `plugin:<id>` for good, so it never changes. |
| `name`, `version` | Shown on the Plugins page and the enable card. |
| `api` | The plugin API version it targets. Must be in `SUPPORTED_API`. |
| `entry` | Exactly one of `script` or `native`. |
| `meta` | `author`, `description`, `website`, `license`, `version`, all optional. The card shows the author and description. |
| `capabilities.source` | `label` names the source in rox. `scrobble` defaults to false. `lyrics` says it answers `source.lyrics`, which rox asks only once the user switches Lyrics on for it. |
| `capabilities.panels` | Extra presets of core panel kinds, listed under the plugin in Add Panel. See [Panels](#panels). |
| `programs` | Programs the plugin runs, by name. The page reports each as found on PATH or missing. rox enforces nothing with it. |
| `config_schema` | JSON Schema for the plugin's settings. |

A native entry names a binary per platform, keyed `<os>-<arch>` from Rust's own names
(`linux`, `windows`, `macos`; `x86_64`, `aarch64`):

```json
"entry": {
  "native": {
    "linux-x86_64": "bin/tones",
    "windows-x86_64": "bin/tones.exe",
    "macos-aarch64": "bin/tones-macos"
  }
}
```

A script entry runs `<interpreter> <path>`. `python3` tries `python3`, then `python`,
then on Windows `py -3`; `node` tries `node`; any other name is looked up on PATH as
written, trying Windows' executable extensions after the bare name (`aliases` and
`on_path`, `manifest.rs:219-282`). The entry path is resolved inside the plugin folder
and has to be a plain relative path to a file that exists (`inside`, `manifest.rs:200-215`).

The Plugins page draws `config_schema.properties` as rows: `string`, `string` with
`"format": "password"` (a masked field), `number`, `integer`, `boolean`, and any property
with an `enum` (`field`, `rox/src/settings/window/plugins_page.rs:62-76`). A property's
`title` labels the row and its `description` shows under it. Anything else shows as raw
JSON. Values are stored in the plugin's record in `accounts.json`, in plaintext like the
rest of that file, and a change restarts the plugin with the new config when the edit
ends.

What refuses a manifest, with the reason the page shows:

- It isn't valid JSON, is bigger than 256 KiB, or isn't a plain file.
- An unknown top-level key, or an unknown key inside `entry` or `entry.script`: those
  are strict (`deny_unknown_fields`). Keys inside `meta` and `capabilities` that the host
  doesn't know are ignored, so a field added there later is additive. A new top-level key
  moves `api`.
- An `id` that doesn't match the pattern, or an `api` outside `SUPPORTED_API`
  (`parse`, `manifest.rs:136-155`).
- `entry` naming both kinds or neither.
- No native build for this platform ("no build for macos-x86_64"), an interpreter not on
  PATH ("interpreter python3 not found"), an entry path that leaves the folder, or an
  entry file that's missing (`entry_with`, `manifest.rs:171-197`).
- A declared panel with no name or a repeated one, no `panel_name`, any children, `info`
  that isn't `{ "panel": { ... } }`, or a `source browser` naming any source but
  `plugin:<id>` (`check_panels`, `manifest.rs`).
- A plugin with no `capabilities.source` loads but gets no host, since source is the only
  capability that runs anything today (`apply`, `rox-services/src/plugins.rs`). It
  doesn't list under Plugins either, since that lists the running hosts.

## The wire

Newline-delimited JSON-RPC 2.0 over the plugin's stdin (rox to plugin) and stdout
(plugin to rox). One object per line, UTF-8, each line at most 1 MiB. stderr is free
text: every line goes to rox's log as `plugin <id>: <line>`, cut at 4 KiB
(`drain_stderr`, `rox-plugins/src/process.rs:95-112`). The plugin never calls rox.

Every request has an `id`, and the answer echoes it with either `result` or
`error: {code, message}`. rox may have many requests in flight, and a plugin may answer
them in any order. A plugin that answers one at a time is legal but stalls its own
playback behind its own searches, so the example runs everything but `shutdown` on a
thread pool. `jsonrpc` is optional on answers and has to be `"2.0"` when present.
`"result": null` is a real answer, told apart from a missing `result`
(`Response`, `rox-plugins/src/wire.rs:52-69`). An error's message is shown to the user
where the call was theirs (a browse, a sync) and logged otherwise.

Nothing is sent before `hello` answers. Every answer is parsed into a typed shape that
refuses unknown fields, and a line that doesn't parse or answers an id rox never sent
is logged and dropped (`parse_line`, `wire.rs:92-117`).

### `hello`

```json
> {"jsonrpc":"2.0","id":1,"method":"hello","params":{"api":1,"config":{"volume":30},"data_dir":"/home/me/.local/share/rox/plugin-data/tones","platform":"linux-x86_64","locale":"en-CA","features":["notice"]}}
< {"jsonrpc":"2.0","id":1,"result":{"name":"Tones","version":"0.1.0","api":1}}
```

`config` is the plugin's settings as the Plugins page stored them, `{}` or `null` when
there are none. The answer's `api` has to be in `SUPPORTED_API`, or rox hangs up
(`start`, `rox-plugins/src/host.rs:382-405`). `features` is `wire::FEATURES`, the
optional parts of API 1 this host reads. A plugin uses one only when it's listed, since
an older host refuses a result that carries it. `locale` is `rox_i18n::locale()`, taken
when `reconcile` (`rox-services/src/plugins.rs`) makes the host and left out when empty.
A language switch doesn't restart a running plugin, since that would cut off a track it's
playing; the new locale reaches it on its next start.

### `source.browse`

`node: null` asks for the roots.

```json
> {"jsonrpc":"2.0","id":2,"method":"source.browse","params":{"node":null,"cursor":null}}
< {"jsonrpc":"2.0","id":2,"result":{"entries":[{"node":{"id":"tones","title":"Tones","subtitle":"6 sine tones","collection":true}},{"node":{"id":"chords","title":"Chords","subtitle":"4 triads","collection":true}}],"cursor":null}}
```

An entry is `{"node": {id, title, subtitle, collection}}` or `{"track": Track}`. Node
ids are non-empty. `collection: true` marks a node the user can keep in the library. A
non-null `cursor` means there's another page, fetched by sending that cursor back:

```json
> {"jsonrpc":"2.0","id":3,"method":"source.browse","params":{"node":"tones","cursor":null}}
< {"jsonrpc":"2.0","id":3,"result":{"entries":[{"track":{"key":"tone:A3","title":"A3, 220 Hz","artist":"rox","album_artist":"rox","album":"Tones","genre":"Test Tone","year":0,"disc_no":1,"track_no":1,"duration_ms":10000,"codec":"PCM","bitrate_kbps":705,"live":false}}, ...],"cursor":"4"}}
```

A Track has `key`, `title`, `artist`, `album_artist`, `album`, `genre`, `year`,
`disc_no`, `track_no`, `duration_ms`, `codec`, `bitrate_kbps` and `live`. None is ever
null: unknown text is `""` and an unknown number is `0`. A missing field reads as empty
rather than refusing the track (`Track`, `wire.rs:229-247`). `key` is opaque to rox,
non-empty, and has to stay the same across sessions and plugin versions, because it
becomes the row's path. A plugin that changes its key scheme orphans every row it made.

With `go-to` listed, a track may also carry `go_to`: the nodes its album and artists
open, each checked like a browse node (`GoTo`, `wire.rs`). It's boxed, since most tracks
leave it out and the entry enum would otherwise carry its size on every row. `page` in
`rox-services/src/plugins.rs` copies it into `Page::go_to`, keyed by track key. The
source browser merges each page's map into its listing and builds Go to from it
(`go_to_submenu`, `rox-panels/src/source_browser.rs`). Keyed rather than lined up with
the entries, it survives a sort and a dropped row without upkeep.

A row keeps its Go to too (ADR 30, amended 2026-10-02). `track` serializes rox's own
`GoTo` onto the `PluginTrack`, so every library write (a sync, a pick, an add) passes
it to `members::upsert`, which stores it in `source_go_to` beside the row. A write
without one leaves the stored one alone. Every path that deletes plugin rows sweeps
`source_go_to` after itself (`drop_stale_go_to`, `rox-library/src/members.rs`), since a
trigger on `tracks` wouldn't survive a migration that rebuilds that table. The library
view reads it back for its rows (`library_go_to`). The `plugin-go-to` migration also
lists every kept collection in `source_resync`, and `sync_collection` sends no token for
those once, so collections kept before rows stored Go to fill it in.

With `flags` listed, a track or node may carry `flags`: a short list of names for what
the row is right now, like `favourite`. An action's `when` reads them (below). A missing
`flags` means the plugin doesn't know, and `[]` means none. `page` collects them into
`Page::flags` by track key or node id, the ids an action's items carry. Sync ignores
them, since they go stale. `listing` notes every page's flags, and actions report the
ones they changed, into one per-source map (`note`, `report` and `known_flags`,
`rox-services/src/plugin_actions.rs`). A library row's menu reads it, so a track keeps
the newest flags the session saw for it. `refresh_flags` (`rox-services/src/plugins.rs`)
asks `source.flags` for every row the source has (`members::keys`), 500 at a time, at
the end of `sync_now`, which runs once per plugin at launch, and after a newly kept
collection syncs. It asks only a plugin with a `when` on some action. Playing a track can change
its row too, like a copy the plugin keeps as it plays, so the engine's `Reader` asks for
that one track's flags when a read reaches its last byte and again ten seconds after the
stream closes, and reports the answer so an open listing merges it.

The shared track menu (`track_actions_with`, `rox-panel-api/src/panel.rs`) offers Go to
from `plugins::stored_go_to`. Picking an entry goes up through `openers::go_to_source` to
`source_browser::go_to_source`, which opens the node in the browser whose menu opened
last when it shows that plugin (`MenuOrigin`), else the first browser on it in
`TabHosts::groups`, else a new browser in `TabHosts::last_live`. When the listing has Go
to for the row, the browser builds its own, which skips the node shown, and passes it as
`Extras::go_to`, which stands in for the kept one. A listing that shows rows
already in the library stores their Go to too (`keep_listed_go_to`, then
`members::keep_go_to`), writing only what moved, so rows from before Go to was kept
pick it up wherever they're listed.

A plugin row's own items are one section of the shared menu, set apart by separators:
Open in Browser and Copy Link, the plugin's actions, then what holds the row in the
library, Remove from Library and Stop Keeping. A surface adds its own to the end of the
section through `Extras::plugin`, like the browser's Add to Library for a row that was
only played. `plugin_actions::offer` draws no separators, so the browser's node and
source menus bracket it the same way.

### `source.search`

```json
> {"jsonrpc":"2.0","id":4,"method":"source.search","params":{"query":"minor","cursor":null}}
< {"jsonrpc":"2.0","id":4,"result":{"entries":[{"track":{"key":"chord:A minor","title":"A minor", ...}}],"cursor":null}}
```

Same page shape as browse, so a search can return nodes as well as tracks.

### `source.sync`

Pages through one collection for the library.

```json
> {"jsonrpc":"2.0","id":5,"method":"source.sync","params":{"collection":"tones","token":null,"cursor":null}}
< {"jsonrpc":"2.0","id":5,"result":{"unchanged":false,"tracks":[ ...4 tracks... ],"cursor":"4","token":null}}
> {"jsonrpc":"2.0","id":6,"method":"source.sync","params":{"collection":"tones","token":null,"cursor":"4"}}
< {"jsonrpc":"2.0","id":6,"result":{"unchanged":false,"tracks":[ ...2 tracks... ],"cursor":null,"token":"bbeea25a853c2c83"}}
```

The first page includes the token rox stored after the last complete sync, and only the
first page does. Answering `"unchanged": true` there ends the sync with nothing written,
and rox keeps its stored token whatever the answer's says. The last page (null
`cursor`) holds the new token. rox replaces the collection's membership only after
the last page, so a sync that fails partway changes nothing
(`fetch_collection` and `sync_collection`, `rox-services/src/plugins.rs:895-975`). A
sync still paging after 2,000 pages is treated as a plugin looping on its own cursor
and fails.

### `source.open`

```json
> {"jsonrpc":"2.0","id":7,"method":"source.open","params":{"key":"tone:A4"}}
< {"jsonrpc":"2.0","id":7,"result":{"stream":"s1","hint":"wav","length":882044,"seekable":true,"live":false}}
```

`stream` is an id the plugin chooses. `hint` is a container extension for the probe, or
`""` to let it sniff. `length` is the total byte count, or null when unknown. `seekable`
says a read at any offset works. `live` is a stream with no end and no duration.
`buffer` is optional: `"ahead"` asks rox to read one chunk ahead instead of downloading
the whole track, for a service that meters or throttles fast downloads. A plugin can
lower the buffering this way, never raise it (`Open` and `Buffer`,
`rox-plugins/src/wire.rs`). `duration_ms` is optional too, sent only when `hello`
listed `open-duration`: the stream's length for a container that doesn't state one.
`Source::build` in `rox-playback/src/engine.rs` takes the container's length first, a
local fragmented MP4's `mehd` next, then the stated one, which is the open's
`duration_ms` or else the row's (`PluginStream::duration_ms`).

With `chapters` listed, the answer may carry `chapters`: `[{start_ms, title}]`, where
the stream's parts start. The wire refuses only the shape: more than `MAX_CHAPTERS`
(500), a title past `MAX_STRING`, or an unknown field (`Chapter`, `wire.rs`). Order and
blank titles are tidied in rox instead, since a bad list mustn't stop the track playing:
`tidy_chapters` (`rox-services/src/plugins.rs`) sorts by start, drops blank titles and
keeps the first of two that start together. The engine never sees them. `open` in
`plugins.rs` notes each non-live open's list against its source and key, keeping the
last `CHAPTERS_KEPT` (8) tracks', and a reopen with none forgets the old list.
`plugins::chapters` hands them out. The seek strip reads the playing track's on every
paint and draws them off the top edge as the cues' chevron at `CHAPTER_WEIGHT`, fainter
than the cues the user drops there, so the two read apart (`chapter_marks`,
`rox-panels/src/transport/seek.rs`). A chapter that starts past the track's length isn't
drawn. Hovering one shows its title and a click seeks to its start, through the same
overlay a station's songs use (`top_overlay`).

### `source.read`

```json
> {"jsonrpc":"2.0","id":8,"method":"source.read","params":{"stream":"s1","offset":0,"len":262144}}
< {"jsonrpc":"2.0","id":8,"result":{"data":"UklGRnR1DQBXQVZFZm10IBAAAAABAAEARKwAAIhYAQACABAAZGF0YVB1DQAAAGcCzQQtB4cJ1wsaDlAQ..."}}
```

At most `len` bytes from `offset`, base64. Fewer is fine, and an empty `data` means the
end of the stream. More than `len` is refused (`Read::bytes`, `wire.rs:344-356`). rox
asks for 256 KiB at a time and never more than 512 KiB, so an answer stays under the
frame cap once encoded. rox never sends overlapping ranges on one stream. A seekable
stream sees its next chunk asked for while the last is being used; a stream that can't
seek sees one read at a time, each starting where the last answer ended
(`rox-plugins/src/stream.rs:1-16`). A plugin still has to accept a read on one stream
while a read on another is running.

A plugin that loses its upstream mid-stream should recover inside `source.read` and
answer an error only when that failed. An error isn't the end of the track: rox closes
the stream and reopens it (see Streams below).

A live stream answers each read with what it has ready rather than waiting to fill
`len`: a 256 KiB read at 128 kbps would take 16 s to fill, past the read timeout. A
segmented upstream is the plugin's to join into one continuous stream in a container
symphonia decodes (CAF, MP4, Matroska, Ogg, AIFF or WAV, or ADTS AAC and MP3). symphonia
0.6 has no MPEG-TS reader.

### `source.close`

```json
> {"jsonrpc":"2.0","id":9,"method":"source.close","params":{"stream":"s1"}}
< {"jsonrpc":"2.0","id":9,"result":null}
```

Sent when rox drops the stream (`Drop for Stream`, `stream.rs:300-313`). Nothing waits on
the answer.

### `source.cover`

```json
> {"jsonrpc":"2.0","id":10,"method":"source.cover","params":{"key":"tone:A4"}}
< {"jsonrpc":"2.0","id":10,"result":null}
```

`{mime, data}` with the image base64, or `null` for no cover. Answer with the largest
size the service has that fits under `MAX_FRAME` once encoded: rox cuts its own
thumbnails for list rows, and the cover panel draws this answer as it is. Asked off the
UI thread, for rows with no stored thumbnail and for the track a cover panel is showing
(`cover`, `rox-services/src/plugins.rs:2310-2326`; `full_cover`,
`rox-services/src/sources.rs`).

### `source.lyrics`

```json
> {"jsonrpc":"2.0","id":11,"method":"source.lyrics","params":{"key":"chord:A minor"}}
< {"jsonrpc":"2.0","id":11,"result":{"text":"[00:00.00]A3\n[00:05.00]C4\n[00:10.00]E4","synced":true}}
```

A track's sheet: LRC when `synced`, plain lines otherwise, or `null` for none. Sent only
to a plugin whose manifest declares `lyrics` and whose user switched Lyrics on for it
(`lyrics_from`, `rox-services/src/plugins.rs`). The text has its own cap, `MAX_LYRICS`
(256 KiB), since an enhanced sheet runs well past `MAX_STRING`; a blank one is refused,
since `null` says none (`LyricsAnswer`, `wire.rs`). It takes the listing timeout.

`target_for` (`rox-services/src/lyrics.rs`) reads the switch once per track and, when
it's on, puts a `PluginLyrics` on the `LyricsTarget`. The Lyrics panel's automatic
lookup asks it first (`maybe_auto_search`, `rox-panels/src/lyrics.rs`) and saves its
sheet through the same path a provider's match takes, without `AUTO_SAVE_CONFIDENCE`:
the answer is for this very track, so it comes in at confidence 1. A plugin with none,
or that fails, falls through to the providers when online lookups are on. The No Lyrics
mark still stops both. The match window lists the plugin's sheet first, under the
plugin's label, then the providers' (`search_with`, `rox/src/lyrics/matcher.rs`). Find
Online shows for a plugin track with the switch on even while online lookups are off,
since the switch is the user's choice for that source.

### `shutdown`

```json
> {"jsonrpc":"2.0","id":12,"method":"shutdown"}
< {"jsonrpc":"2.0","id":12,"result":null}
```

Then the plugin exits. It should also exit whenever stdin closes: that's the one signal
that reaches a plugin on every OS when rox goes away without a shutdown.

## Timeouts and caps

| | Limit | Where |
|---|---|---|
| `hello` | 30 s | `Timeouts`, `rox-plugins/src/host.rs:53-65` |
| `source.browse`, `source.search`, each later sync page | 15 s | |
| A sync's first page | 60 s | |
| `source.open` | 20 s | |
| `source.read`, `source.cover` | 10 s | |
| `shutdown` | 2 s, then killed | |
| A line on stdout | 1 MiB | `MAX_FRAME`, `rox-plugins/src/wire.rs:16` |
| A string in a result | 4 KiB | `MAX_STRING`, `wire.rs:18` |
| Entries or tracks per page | 500 | `MAX_ENTRIES`, `wire.rs:21` |
| A read's `len` | 256 KiB asked, 512 KiB at most | `Options`, `stream.rs:35-42`; `MAX_READ`, `wire.rs:24` |
| A stderr line | 4 KiB | `STDERR_LINE`, `process.rs:20` |
| Sync pages per collection | 2,000 | `MAX_SYNC_PAGES`, `rox-services/src/plugins.rs:45` |

A call that times out fails on its own; the plugin keeps serving its other calls. A
longer stdout line is dropped with a warning. A result over a cap fails that call.

## Process lifecycle

A plugin gets one process, started by the first call that needs it
(`conn`, `host.rs:262-329`). A plugin switched on starts at once, since switching on
syncs its kept collections, and that's the first call. The host has one thread writing
stdin, one reading stdout and a table of pending requests by id, so any number of calls
can be in flight.

The process starts in the plugin's folder with rox's environment (PATH, HOME and the
rest) plus `PYTHONDONTWRITEBYTECODE=1` (`spawn`, `rox-plugins/src/process.rs:36-93`). A
plugin can read its own switches from the environment.

When a plugin exits unexpectedly, every call it had fails with "the plugin exited", and
the next call starts it again after a wait of 0, 1, 2 or 4 s for the first through fourth
crash inside ten minutes (`BACKOFF`, `host.rs:26-31`). The fifth stops it for good
with "Stopped after repeated crashes" on its row, until it's switched off and on
(`crashed` and `revive`, `host.rs:331-347` and `:522-528`). A plugin that can't start,
or fails `hello`, counts as a crash, so a broken one stops instead of retrying forever.

Switching a plugin off sends `shutdown`, waits up to 2 s for requests still in flight,
then closes stdin and kills what's left (`stop`, `host.rs:477-500`). Quitting rox skips
the wait and hangs up at once (`hang_up_now`, `host.rs:505-518`). A changed config or a
changed folder hash replaces the host, which stops the old process
(`reconcile`, `rox-services/src/plugins.rs:281-337`).

Per OS:

- Linux and macOS: the plugin leads its own process group (`process_group(0)`), and
  stopping it sends SIGKILL to the whole group, so whatever it started goes with it
  (`kill`, `process.rs:189-200`). A child that moves itself to another group escapes, as
  it would from a shell.
- Windows: the plugin starts with `CREATE_NO_WINDOW`, so no console window opens, and
  goes into a job object with kill-on-close right after spawn. Closing the job's handle
  ends everything in it. A grandchild started between the spawn and the assignment can
  escape the job; that window is accepted. If the job can't be created the plugin still
  runs, with a warning in the log.

## Streams

A plugin row plays through ADR 29's first amendment. The row stores no URL.
`store::locators_for` answers a `Locator::Plugin` holding the source, the key and the
live flag (`rox-library/src/store.rs:1742`), and the player hands the queue the opener
the services layer installed (`rox-services/src/player.rs:1600`,
`rox-services/src/openers.rs:17-30`). The engine never depends on the plugin host: the
opener is a boxed function (`Opener`, `rox-playback/src/plugin.rs:86`).

On the decode thread the engine calls the opener, names any failure after the plugin,
and builds a source from the result (`rox-playback/src/engine.rs:2298-2360`). A failed
open publishes a refusal naming the plugin, and the queue moves on. A dead or slow plugin
never stops local playback. A command that arrives during an open waits for it, since
commands drain at the top of the decode loop; research 04 measured pauses waiting up to
2.3 s behind cold opens.

The services side opens ahead (`follow` and `preopen`,
`rox-services/src/plugins.rs:701-804`). Once the audible track has held for 2 s, the
two entries after the one the engine is opening or adopted last are opened
(`upcoming_locators`, `rox-services/src/player.rs:1204-1232`), and one nobody takes
within 60 s is closed. Counting from the engine's entry, not the clock's, keeps a skip
from reopening the track it just opened. An engine
open that finds its stream pre-opened takes it, or waits for the pre-open still under
way (`take_preopened`, `plugins.rs:677-699`). Live streams are never pre-opened, since
that would start a broadcast nobody hears yet.

What the engine reads through (`reader_for`, `rox-playback/src/plugin.rs:56-74`):

- A seekable stream of known length up to 64 MB, unless it asked for `"ahead"`, is
  downloaded whole while it plays, front to back on its own thread
  (`rox-playback/src/download.rs`, `CAP` at `:21`). A read the download hasn't reached
  moves the download there, and what it skipped fills in after. The seekbar shows what's
  downloaded. The waveform fills in as the bytes land, from a decode that trails the
  download without steering it (`decode_peaks_trailing` and `Trailing`), and the one
  kept is decoded from the same bytes once they're all in. A pre-opened stream doesn't
  download until the engine opens it.
- Anything else reads one 256 KiB chunk ahead through the host's stream. With a length,
  its waveform fills in from the audio tap at the playhead as it plays, and isn't kept.

The decoder reads through `PluginSource`, which keeps a 1 MiB window of recent bytes so
the probe's small backward seeks never go back to the plugin
(`rox-playback/src/plugin.rs:88-93`). A seek outside the window is a read at the new
offset, and an error on a stream that can't seek.

A read that fails mid-track recovers before it refuses (`recover`,
`plugin.rs:169-242`). The stream is closed and reopened through the opener with radio's
backoff of 0, 1, 2 and 4 s (`BACKOFF`, `rox-playback/src/http.rs:58`), and the transport
shows Reconnecting. A host restart after a crash is one of those attempts. A seekable
stream of known length resumes at the byte it failed on, provided the reopened stream has
the same length; a different length is a different encode, and rox refuses to splice
two. A stream that can't seek, or has no length, can't resume and ends at once. Out of
attempts, the track ends with a refusal naming the plugin.

A live stream plays from radio's feed thread and tape (`open_live`,
`plugin.rs:380-411`). The feed pulls `source.read` whether or not anything decodes, so a
pause resumes where it stopped and the last minutes are seekable in the tape. A dropped
feed reopens at the live edge and marks a gap. A pause longer than 30 minutes hangs up
and rejoins live on Play (`LIVE_IDLE_HANGUP_SECS`, `rox-playback/src/engine.rs:147`).
The tape measures the stream's rate from the decoder, since the locator has no
bitrate.

## Rows

A plugin's rows come by membership, ADR 29's second amendment
(`rox-library/src/members.rs`). A plugin has no whole catalog to reconcile against, so
`source_members` records which collections hold each row and in what order. Tracks
picked one at a time are held in a collection of their own, `PICKED`, the empty string,
which no node id can be (`members.rs:22`).

- A track becomes a row through `row_for` (`members.rs:63-96`): the wire fields, the key
  as the path, no URL, and the live flag.
- Keeping a collection syncs it, and `set_collection` makes the collection hold exactly
  the synced tracks, then prunes rows nothing holds any more (`members.rs:130-153`).
  Letting it go is `drop_collection`, which prunes the same way.
- Playing, queueing or adding a browsed track to a playlist `pick`s it: the row is
  upserted and held in `PICKED`, and nothing is pruned (`pick`). A row held by nothing
  but `PICKED` stays out of the library (ADR 29, amended 2026-09-29):
  `picked_only_ids` feeds `Projection::hide_rows` in `load_projection`
  (`rox-services/src/catalog.rs`), which takes it out of browse and general search
  while the queue, history and playlists still resolve it.
- Add to Library is `save`, which records the track in `source_saved`, a table of its
  own since a node id can be any non-empty string. A saved row counts as held.
  Remove from Library is `unsave`, which prunes the row if nothing else holds it. The
  browser offers Remove only for rows `Library::is_saved` says were added on their own.
- The shared track menu (`track_actions_with`, `rox-panel-api/src/panel.rs`) takes plugin
  rows out too, through `rox-panel-api/src/plugin_library.rs`. `members::holds` reads
  which of the selected rows are saved and which kept collections hold any of them, one
  lookup per row id. Saved rows get Remove from Library, the browser's `unsave`. Each
  kept collection gets Stop Keeping, named by its `SyncedCollection` title. It calls
  `set_synced` with sync off, the same write as the browser's keep switch. A single row
  of a kept collection can't be removed, since the next sync would put it back. Both
  writes reload the projection, and a failure shows in a toast on the menu's window.
- The Plugins page counts each switched-on plugin's rows with `members::in_library`:
  rows a kept collection holds or the user saved, picked-only rows left out. It reads
  the counts again when the projection moves. Show in Library replaces the shared
  filter with a Source pick on `plugin:<id>`, clears the shared search text and raises
  the workspace window (`show_in_library`, `rox/src/settings/window/plugins_page.rs`).
- At launch, `expire_picks` deletes picked-only rows neither picked nor played in 30
  days (`PICK_KEEP_SECS`), keeping the saved queue's rows and the last track, which
  restore by row id (`rox-services/src/plugins.rs`).
- Remove on the Plugins page is `remove_source`: every row, membership and saved entry
  of the source goes.

Each of these writes the rows, the membership, the prune and the relinks in one
transaction. Playlist members and listens snapshot their source beside their path, so a
playlist entry or a play's history reattaches when a plugin row comes back after a
re-sync, a re-pick or a reinstall (`playlists::reattach` and `listens::reattach`). Every
membership write refuses local, radio and Subsonic sources outright
(`refuse_other_shapes`, `members.rs:247-262`), since their rows aren't held by
membership and a prune would delete them.

Kept collections sync once when the plugin starts, and on Sync Now on the Plugins page
(`sync_now`, `rox-services/src/plugins.rs:1062-1090`). A kept collection opens in the
External Sources panel from the library, with no plugin call, so it still browses with
the plugin stopped or the network down. Its row draws from what `SyncedCollection` cached
of the node: title, second line, kind and art key, taken when it's kept and refreshed
whenever a listing shows the node again (`restyle_kept`,
`rox-panels/src/source_browser.rs`). A kept node's second line is the plugin's, then the
library's count of its tracks (`kept_line`).

A browse or search waits for the first `apply` before it looks up its host
(`listing`, `rox-services/src/plugins.rs`), since a panel restored at launch lists
before the hosts exist. When a listing fails, the panel asks `plugins::unavailable` why
and, when rox knows, shows that in its own words with an Open Plugins button instead of
the error (`unavailable_banner`, `rox-panels/src/source_browser.rs`). It asks again once
`plugins::answers` says the source is back, checked on each draw and each library
reload. A page's `notice` shows as a banner over the list, Info or, for `setup`, Warn
with the same button (`notice_banner`).

A plugin's rows show only while it's switched on and its folder is loaded
(`live_ids`, `rox-services/src/sources.rs:228-232`). A switched-off plugin's rows are
hidden, not deleted. A Missing plugin, whose folder is gone, keeps its switch and its
rows, hidden, since deleting the old folder is how many people update a plugin. The
rows go only on Remove.

A plugin row scrobbles only when the loaded manifest declares `scrobble: true` and the
user leaves Scrobble Plays on (`may_scrobble`, `rox-services/src/lastfm.rs:150-171`).
A plugin that isn't loaded never scrobbles, whatever its record says. Capture never
applies: a plugin stream doesn't pass through the HTTP source or its ICY wrapper.

Lyrics are the same gate with the other default: a manifest that declares `lyrics`
gets a Lyrics switch under the plugin, off until the user turns it on, and approval
never turns it on (`set_lyrics`, `PluginRecord::lyrics`). The Providers page's online
switch covers the built-in providers only.

## Panels

Contract capability `panels` (WT-P9). Every running plugin is listed under Add Panel >
Plugins with an External Sources panel on its own source (`browser_preset`,
`rox/src/panel_catalog.rs`), whatever its manifest says. `capabilities.panels` adds
more: each entry is a name and a `preset`, a dock `PanelState` with `PanelInfo::Panel`,
the same dump a saved panel preset holds (`DeclaredPanel`, `rox-plugins/src/manifest.rs`).
Nothing executes. The manifest check holds the shape; whether the kind exists is the
app's call, since only the app has the catalog. `plugin_preset_for` skips a kind this
build lacks rather than refusing the plugin, so a plugin written for a later rox still
runs.

`running_plugins` (`rox-services/src/plugins.rs`) reads the host table, so a plugin
lists only while it's switched on and approved, and none list with plugins off.
`plugin_sections` builds each plugin's entries, External Sources first, keyed by the
declared name or, for External Sources, the blank name no declared panel can have
(`BROWSER_ENTRY`). A declared `source browser` takes the External Sources slot instead
of listing a second time, and any after the first are dropped (`plugin_entries`). Each
is a `PanelPreset` with `owner` set to `plugin:<id>`. They're
built at open time and kept out of `CATALOG`, since their labels are the plugin's own
text rather than message keys.

A plugin with nothing declared is one entry; one with declared panels branches.

| Picker | Shape | Code |
| --- | --- | --- |
| Add Panel, a composite's slot picker | Plugins > plugin, or Plugins > plugin > entry | `panel_presets::plugins_submenu` |
| Panels menu, in the menubar and the menu panel | Plugins > plugin, a branching plugin's entries each trailing its name | `MenuEntry::PluginsSubmenu` |
| New Window from Panel, same two | a Plugins group after the catalog's, same rows | `plugins_group`, `workspace/menubar.rs` |
| The macOS menu bar | Plugins > plugin, or Plugins > plugin > entry | `plugin_items`, `workspace/native_menu.rs` |
| An empty window's panel list | a Plugins section of single-entry plugins, and a section per branching one | `workspace.rs` |

The hand-built flyouts go two levels deep and number their rows for the keyboard, so
there a branching plugin's entries sit in the one run instead of nesting a third level
(`plugin_panels`). A pick resolves the entry again by plugin and key (`plugin_preset`),
so a plugin stopped since the menu opened picks as a no-op. The macOS bar is a snapshot,
so `plugins::after_apply` rebuilds it after every apply.

`owner` lives on `PanelChrome`, flattened into every config, so it survives a dump and a
restore. `WorkspaceBundle::from_settings` fills `requires` with the plugin ids found in
`owner` fields across the layouts and panel presets (`required_plugins`,
`rox-core/src/settings.rs`), and the apply card names the ones not running
(`requires_line`, `rox/src/workspaces.rs`). A plugin panel is a core kind, so it restores
without its plugin; only a kind the binary doesn't have comes back as the dock's
`InvalidPanel` stand-in (`PanelRegistry::build_panel`, `rox-dock/src/panel.rs`).

Declarative panels with plugin-defined readouts and verbs, and ADR 24 node trees, are out
of scope here.

## Trust

The switch is the approving act, following the shader gate's rule that only a direct
user action approves code to run. There's one gate and no second dialog behind it.

Approved hashes are stored in `session.json` as `approved_plugins`, id to hash, next to
the shader approvals (`plugin_approved` and `approve_plugin`,
`rox-core/src/settings.rs:1484-1521`). They're machine-local, so a copied settings file
doesn't carry someone else's trust decision. What rox remembers about a plugin is its
`PluginRecord` in `accounts.json` (`settings.rs:2048-2082`): the switch, the label, the
hash and manifest at the last approval, the scrobble and lyrics choices, the kept
collections and the config.

Switching a plugin on checks the folder's current hash against the machine's approval
(`switch_plugin`, `rox/src/settings/window/plugins_page.rs`). The same hash switches on
with no card. Any other hash opens the enable card, built from the folder as scanned
when the switch was pressed, so what gets approved is what the card showed. Confirming
it writes the approval and the record (`approve`, `rox-services/src/plugins.rs:419-461`).

Whenever the folders, the records or the approvals change, a switched-on plugin whose folder no longer
matches its approval is switched off (`apply`, `plugins.rs:209-232`). Its row reads
"Changed on disk". Switching it on again opens the card with the manifest diff against
the one approved last time (`changes`, `plugins.rs:553-601`): capabilities added or
dropped, programs added, the scrobble declaration turned on or off, and a changed entry.
When the manifest is the same, the card says other files changed.

Developer mode is a per-plugin toggle for its author, an icon button beside a
switched-on plugin's switch (`plugin_row`, `rox/src/settings/window/plugins_page.rs`). It's
an in-memory set in `rox-services/src/plugins.rs` (`DEVELOPING`), never saved, so it
ends when rox quits, and switching the plugin off or removing it clears it. Before
`apply` looks for changed folders, `redevelop` approves each switched-on plugin in the
set whose folder loads and whose manifest diff against the last approval is empty. The
host then restarts on the new hash like any re-approval. A diff that isn't empty falls
through to the usual switch-off and card. A developing plugin whose folder stops loading
keeps its switch rather than being switched off, since it can't run either way.
Turning the toggle on is the direct user action that approves the saves that follow.

A first approval of a manifest that declares scrobbling turns Scrobble Plays on, since
the card just said so. A user who turned it off keeps it off across re-approvals that
don't change the declaration.

The Plugins page's copy, in English, for finding each string in the other locales:

| Where | English |
|---|---|
| Page intro | Each folder in the plugins folder is one plugin. Nothing in it runs until you switch it on, and one that changes on disk switches off until you switch it on again |
| Buttons | Reveal Folder, Rescan, Plugin Guide |
| Empty page | No plugins yet. Drop a plugin's folder into the plugins folder |
| Row status | Missing, Failed, Changed on disk, Stopped after repeated crashes, Needs { $programs }, On, Off |
| Under a failed row | This plugin can't run |
| Card title | Switch on "{ $name }"? |
| Card body | It runs as a program on this computer with your permissions. rox doesn't sandbox it. |
| Card, source | Adds { $label } as a source: rox browses, searches, syncs and plays it through the plugin. |
| Card, scrobbling | Asks to scrobble what it plays. |
| Card, lyrics | Offers lyrics for its tracks. rox only asks once you switch them on. |
| Card, programs | Uses { $program }, found on this computer. / Uses { $program }, which isn't on this computer's PATH. |
| Card, re-approval | Changed since you last switched it on: / The manifest is the same as last time. Other files in the folder changed. |
| Card, changes | New capability: { $name } / Dropped capability: { $name } / New program: { $program } / Now asks to scrobble / No longer asks to scrobble / Starts a different way |
| Card button | Switch On |
| Page switch | Enable Plugins: Let the plugins in the plugins folder run. Each one still has its own switch below, and runs as a program on this computer with your permissions |
| Under a switched-on plugin | Scrobble Plays, Lyrics: Ask this plugin for its tracks' lyrics before the lyrics providers. Offered because the plugin answers them, Synced Collections, Sync Now, The last sync failed |
| Developer mode tooltip | Developer mode: until rox quits, a change to this plugin's folder is approved on its own and restarts it. A change to what its manifest declares still switches it off |
| Nothing kept | Nothing synced yet. Switch sync on for a collection in the plugin's source browser |
| Remove title | Remove "{ $name }"? |
| Remove body | Its tracks, synced collections and settings go. Its folder stays in the plugins folder, so delete it there to stop it showing here. |
| Remove body, Missing plugin | Its tracks, synced collections and settings go. Its folder is already gone from the plugins folder, so it won't show here again. |

The keys are `settings-plugins-*` in `crates/rox-i18n/locales/*/rox.ftl`. The manifest's
own text (name, description, the source label, config titles) isn't translated: a
plugin shows in its author's language.

The gate doesn't defend the machine against local software. Anything that can write the
plugins folder can write `session.json` too. It makes sure nothing runs that the user
didn't switch on, and shows what a plugin declared before any of it runs.

## What a plugin can't do

- Run code in the UI or draw anything. UI from a plugin waits on
  [ADR 24](../02-architecture/decisions/24-adr-script-panels.md), which is Proposed.
- Process audio or hook the engine. A plugin hands over container bytes and decoding
  stays in rox, so nothing a plugin does reaches the decode thread's DSP or the output
  callback.
- Hand rox a URL to fetch. A service's URLs expire, can point at a different encode each
  time, and are often segmented; bytes make all of that the plugin's problem, and rox
  never fetches an address a plugin chose.
- Call rox, or connect to the control socket. The socket authenticates by filesystem
  permission and can't tell a plugin from `roxctl`. The pipe needs no identity: rox
  spawned the process.
- Write into its own folder without switching itself off.
- Scrobble without declaring it, or record through capture.

A plugin does run with the user's permissions, its own network and its own filesystem
access. rox doesn't sandbox it, and the enable card says so.

## The example

[`examples/plugins/tones`](../../examples/plugins/tones/) is a source that makes its own
audio: sine tones and triads, each generated as a 16-bit mono WAV at 44.1 kHz the first
time it's opened, with its length known up front. It needs no network, no service and
no program beyond Python 3's standard library. It demonstrates two collections, paged
browsing and syncing with a token, search by name, a `null` cover, a config field, a
thread pool so reads never wait on searches, and exiting on stdin EOF.

Every tone and chord is built from whole numbers of hertz and lasts a whole number of
seconds, so it ends on a whole cycle and two tones played back to back join without a click.
`crates/rox-plugins/tests/tones.rs` runs the example against the real host: the folder
loads and hashes the same after running, `hello` answers, browse lists both collections,
a sync pages and then answers unchanged, and a full read decodes to the frame count the
track's duration states. The tests skip when no Python interpreter resolves.

Two tones queued back to back play gaplessly through the engine. This was measured on the
position clock, which counts only frames the output actually played: a gap plays silence
the clock doesn't count (`rox-playback/src/output.rs:362-366`), so it would show as wall
time pulling ahead of position. In a debug build on an Apple Silicon Mac, output at
48 kHz with crossfade off, the next tone opened from its pre-open in under a millisecond,
and wall time and position stayed within 16 ms of each other across the boundary, with
position ahead rather than behind. The same two tones played as local WAV files measured
the same to half a millisecond, so the plugin path adds nothing at the boundary. Nobody
listened to it. That confirms, for WAV at least, ADR 30's inference that a plugin with
only decoded samples can serve them in a container the engine reads.

## Reference

The host is `crates/rox-plugins`: `manifest.rs` (the manifest, `SUPPORTED_API`, the
entry and the interpreter table), `hash.rs` (the folder hash), `loader.rs` (the scan and
the folder watch), `wire.rs` (framing, the typed results and the caps), `process.rs`
(spawn, stderr, the process group and the job object), `host.rs` (one plugin's process,
calls, timeouts, crashes and shutdown) and `stream.rs` (reads by offset with read-ahead).
Its tests are `tests/echo.rs` against the fixture in `tests/fixtures/echo/`, and
`tests/tones.rs` against the example.

The services side is `crates/rox-services/src/plugins.rs` (the host table, apply and
approval, browse, search, sync, pick, covers, the opener and the pre-open), with
`sources.rs` (`live_ids`, hiding and departing rows), `lastfm.rs` (the scrobble gate),
`lyrics.rs` (`PluginLyrics`, the lyrics gate),
`openers.rs` (the opener slot) and `thumbs.rs` (covers through the plugin). Records and
approvals are `PluginRecord`, `SyncedCollection` and `approved_plugins` in
`crates/rox-core/src/settings.rs`. The library side is `crates/rox-library/src/members.rs`
(rows by membership), `locator.rs` (`Locator::Plugin`), `cue.rs` (`PLUGIN_PREFIX`,
`Origin::Plugin`) and `store.rs` (`locators_for`). The engine side is
`crates/rox-playback/src/plugin.rs` (`Opener`, `PluginSource`, recovery, live streams)
and `download.rs` (whole-track download). The UI is
`crates/rox/src/settings/window/plugins_page.rs` (the Plugins page and the enable card),
`application_page.rs` (Enable Plugins), `crates/rox-panels/src/source_browser.rs`
(the External Sources panel), `crates/rox/src/panel_catalog.rs` (`plugin_sections`) and
`panel_presets.rs` (`plugins_submenu`).
