# Plugins

A plugin brings a source from outside rox into the library: rox browses, searches,
syncs and plays it through a program the plugin supplies. The plugin is a folder
holding a manifest and that program. rox starts the program as a subprocess and
speaks newline-delimited JSON-RPC 2.0 with it over stdin and stdout.

rox doesn't ship plugins for streaming services. Two examples are there to copy, both on
nothing beyond Python 3's standard library. [`examples/plugins/tones`](examples/plugins/tones/)
generates sine tones and chords, so it needs no network, and it's the place to start.
[`examples/plugins/internet-archive`](examples/plugins/internet-archive/) browses the
Internet Archive's freely licensed netlabel releases and uses every optional part of the
protocol.

This guide covers plugin API version 1, the only one the host supports.

## Installing one

1. Turn on Enable Plugins at the top of Settings > Plugins.
2. Press Reveal Folder on the Plugins page. It opens the plugins folder, creating it
   the first time.
3. Drop the plugin's folder in. The folder's name has to be the plugin's id. The page
   picks it up with its switch off.
4. Switch it on. The first time, a card shows what the plugin declares and which
   programs it runs, and asks you to confirm.

The plugin then shows up under Add Panel > Plugins, which opens the External Sources
panel on its source. The checkmark that shows when you hover a collection, Keep in the
Library, syncs it into the library. Playing or queueing a track plays it without adding
it; the checkmark on a hovered track, or Add to Library in its menu, keeps it. A check
stays lit while its collection or track is in the library. Library, beside the panel's
search box, lists what the plugin has put there, its kept collections and the tracks
added one at a time, without asking the plugin.

Remove on the Plugins page drops the plugin's tracks, synced collections, settings and
approval. Its folder stays where it is.

## Trust

A plugin runs as a program on your computer with your permissions, its own network
access and its own filesystem access. rox doesn't sandbox it, and the enable card says
so.

Switching a plugin on approves exactly the files in its folder, by a SHA-256 hash over
every file in it. The approval is stored per machine, so a copied settings file
doesn't bring someone else's trust decision along. When any file in the folder
changes, the plugin switches off and its row reads Changed on disk. Switching it on
again shows what changed in the manifest since the last approval: capabilities added
or dropped, new programs, the scrobble declaration, a different entry.

Some things about the hash matter when you write a plugin:

- A write into the plugin's own folder changes its hash and switches it off. Write
  only under the `data_dir` that `hello` hands you.
- Editing your own plugin changes its hash too. [Developer mode](#developer-mode)
  approves those edits for one session.
- A symlink anywhere in the folder refuses the plugin, and so does a plugin folder
  that is itself a symlink. So does a file name that isn't UTF-8.
- `.DS_Store`, `Thumbs.db` and `desktop.ini` are left out of the hash, since the OS
  writes them. `__pycache__` is hashed, because Python would run a planted `.pyc`.
  rox sets `PYTHONDONTWRITEBYTECODE=1` for every plugin so a Python plugin's first
  import doesn't write one.
- A program the plugin ships under `bin/` is hashed like every other file. Replacing
  one there switches the plugin off until it's approved again, the same as editing
  the script or manifest.

The gate doesn't defend against other software on the machine: anything that can
write the plugins folder can write rox's settings too. It makes sure nothing runs
that the user didn't switch on, and shows what a plugin declares before any of it
runs.

## The folder

```
<data>/plugins/<id>/        the plugin, as the user dropped it
    plugin.json
    <entry and anything else it ships>
<data>/plugin-data/<id>/    where the plugin writes, created on its first start
```

`<data>` is rox's data directory, or `rox-data` beside the executable in portable
mode. rox reads every subfolder of `plugins`, parses its manifest, hashes it and
checks the programs it lists, all without running anything. A folder that can't load
still shows on the Plugins page with the reason. A folder whose manifest `id` doesn't
match the folder's name is refused.

## The manifest

`plugin.json`, at most 256 KiB. The tones example's:

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

| Key                   | Meaning                                                                                                          |
| --------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `id`                  | `^[a-z0-9][a-z0-9-]{1,63}$`. The plugin's rows are filed under it for good, so it never changes once you have users. |
| `name`, `version`     | Shown on the Plugins page and the enable card.                                                                   |
| `api`                 | The plugin API version it targets: `1`.                                                                          |
| `entry`               | Exactly one of `script` or `native`.                                                                             |
| `meta`                | `author`, `description`, `website`, `license`, `version`, all optional. The card shows the author and description. |
| `capabilities.source` | `label` names the source in rox. `scrobble` defaults to false. `icon` is optional, see [The icon](#the-icon). `radio: true` says the plugin answers [`source.radio`](#sourceradio). `links: true` says it answers [`source.link`](#sourcelink). `lyrics: true` says it answers [`source.lyrics`](#sourcelyrics). `actions` lists what it can do with its items, see [Actions](#actions). |
| `capabilities.panels` | Extra panels listed under the plugin in Add Panel. Optional. See [Panels](#panels).                              |
| `programs`            | Programs the plugin runs, by name. The page checks the plugin's own `bin/` folder, then Program Folders, then PATH, and reports each name as found or missing. rox doesn't enforce the list. |
| `config_schema`       | JSON Schema for the plugin's settings.                                                                           |

A script entry runs `<interpreter> <path>` from inside the plugin folder. Every
program name, the interpreter included, is looked up in the plugin's own `bin/`
folder first, then the folders in Program Folders (Settings > Plugins, and beside
Convert's ffmpeg row on the Integrations page), then PATH.

On Windows, `python3` tries `py -3`, then `python3`, then `python`. Elsewhere it tries
`python3`, then `python`.
rox runs each candidate with `--version` and keeps it only if it prints a Python 3
line within a few seconds, which skips the Microsoft Store's placeholder and rules
out a `python` that's actually Python 2. `node` tries `node` with no version check.
Any other name is looked up as written, with Windows' executable extensions tried
after the bare name. The path has to be a plain relative path to a file in the
folder.

A native entry names a binary per platform, keyed `<os>-<arch>` with Rust's names
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

The Plugins page draws `config_schema.properties` as settings rows. It understands
`string`, `string` with `"format": "password"` (a masked field), `number`, `integer`,
`boolean`, and any property with an `enum`. A property's `title` labels the row and its
`description` shows under it. Anything else shows as raw JSON. Values are stored
in plaintext with the rest of rox's account settings. A change restarts the plugin
with the new config once the edit ends.

### The icon

`capabilities.source.icon` names an SVG in the plugin's folder, like `"icon.svg"`. The
External Sources panel shows it in its header in place of the source's name, which moves
to its tooltip. rox draws it as a mask in the theme's text colour, the way it draws its
own icons, so only its shape counts: colours are ignored, and it reads on every theme. It
should be square, and at most 64 KiB. An icon that holds an `<image>`, `<feImage>` or
`<foreignObject>` is refused, since the renderer would read the file one of those names
from disk. The icon is part of the folder hash like every other file.

A manifest is refused, with the reason on the Plugins page, when:

- it isn't valid JSON, is over 256 KiB, or isn't a plain file
- it has an unknown top-level key, or an unknown key inside `entry` or `entry.script`
- the `id` doesn't match the pattern, or `api` isn't a version the host supports
- `entry` names both kinds or neither
- there's no native build for this platform, the interpreter isn't on PATH, or the
  entry path leaves the folder or doesn't exist
- the icon isn't an `.svg`, leaves the folder, doesn't exist, is over 64 KiB, or embeds
  an image
- a declared panel has no name or shares one with another, has no `panel_name`, holds
  children, doesn't have `info` as `{ "panel": { ... } }`, or is a `source browser`
  whose `source` isn't the plugin's own `plugin:<id>`

Unknown keys inside `meta` and `capabilities` are ignored, so fields added there later
don't break older hosts. A new top-level key means a new `api` version. A plugin with
no `capabilities.source` loads but never starts, since source is the only capability
the host runs.

## Wire format

rox writes requests to the plugin's stdin and reads answers from its stdout. One JSON
object per line, UTF-8, each line at most 1 MiB. Every request has an `id`, and the
answer echoes it with either `result` or `error: {"code": n, "message": ".."}`.
`jsonrpc` is optional on answers and has to be `"2.0"` when present. `"result": null`
is a valid answer.

A plugin has to write UTF-8 and flush stdout after every line it writes, whatever
language it's in. An answer left in a buffer never arrives, and its call times out.

rox can have many requests in flight, and a plugin may answer them in any order. A
plugin that answers one at a time works, but its own playback then waits behind its
own searches. The tones example runs everything but `shutdown` on a thread pool.

Answers are parsed strictly. A result with a field rox doesn't know fails that call,
with an error naming the field. A line that doesn't parse, has a key the frame doesn't
define, or answers an id rox never sent is logged and dropped, and the call it was
meant for waits out its timeout.

An error's `code` has to be an integer, but rox reads only its `message`. That message is
shown to the user when the call was theirs (a browse, a sync, a link) and logged otherwise. The
tones example follows JSON-RPC's codes: -32601 for an unknown method, -32602 for bad
params, -32000 for anything else.

stderr is free text. Each line goes to rox's log as `plugin <id>: <line>`, cut at 4 KiB.
The log is live in the Console window (F12), and on disk at `<data>/logs/rox.log`, which
rolls to `rox.log.1` at 2 MiB. The plugin never sends requests to rox.

## Methods

| Method           | Params                            | Answers                                          |
| ---------------- | --------------------------------- | ------------------------------------------------ |
| `hello`          | `api`, `config`, `data_dir`, `platform`, `features` | `{name, version, api}`           |
| `source.browse`  | `node`, `cursor`, `view`          | a page of entries                                |
| `source.search`  | `query`, `cursor`, `view`         | a page of entries                                |
| `source.sync`    | `collection`, `token`, `cursor`   | a page of tracks with the collection's token     |
| `source.open`    | `key`                             | a stream id and what rox needs to read it        |
| `source.read`    | `stream`, `offset`, `len`         | `{data}`, base64                                 |
| `source.close`   | `stream`                          | null                                             |
| `source.cover`   | `key`                             | `{mime, data}` with the image base64, or null    |
| `source.radio`   | `seed`, `cursor`, `count`         | a batch of tracks and where the next starts      |
| `source.link`    | `item`                            | `{url}` for its web page, or null                |
| `source.lyrics`  | `key`                             | `{text, synced}` for the track's sheet, or null  |
| `source.action`  | `action`, `items`, `params`       | an outcome, or `{job}` for work rox polls        |
| `source.job`     | `job`                             | the job's progress, and its outcome once it ends |
| `source.cancel`  | `job`                             | null                                             |
| `source.flags`   | `items`                           | `{flags}`, each item's flags now                 |
| `shutdown`       |                                   | null, then the plugin exits                      |

### hello

The first request, and nothing else is sent until it's answered:

```
→ {"jsonrpc":"2.0","id":1,"method":"hello","params":{"api":1,"config":{"volume":30},"data_dir":"/home/me/.local/share/rox/plugin-data/tones","platform":"linux-x86_64","locale":"en-CA","features":["notice"]}}
← {"jsonrpc":"2.0","id":1,"result":{"name":"Tones","version":"0.1.0","api":1}}
```

`config` is the plugin's settings as the Plugins page stored them, `{}` or null when
there are none. The answer's `api` has to be one the host supports, or rox hangs up.

`locale` is the language rox's interface is in, as a BCP 47 tag like `en-CA` or
`zh-Hans`. A plugin can use it for the text it writes itself: titles and lines it makes
up, errors, and action messages. Text from the manifest, like an action's label, still
shows as written. rox takes the locale when it starts the plugin, so a language change
reaches a running plugin the next time it starts. A host from before `locale` doesn't
send it, so a plugin treats a missing `locale` as unknown.

`features` names the optional parts of API 1 this host reads. A host from before a
feature refuses a result that uses it, so a plugin uses one only when it's listed, and
treats a missing `features` as an empty list. This host lists `notice`, `notice-link`,
`node-kind`, `node-art`, `sections`, `views`, `fields`, `tiles`, `home`,
`open-duration`, `go-to`, `flags` and `chapters`.

### source.browse and source.search

Browse with `node: null` asks for the roots:

```
→ {"jsonrpc":"2.0","id":2,"method":"source.browse","params":{"node":null,"cursor":null}}
← {"jsonrpc":"2.0","id":2,"result":{"entries":[{"node":{"id":"tones","title":"Tones","subtitle":"6 sine tones","collection":true}},{"node":{"id":"chords","title":"Chords","subtitle":"4 triads","collection":true}}],"cursor":null}}
```

An entry is `{"node": {id, title, subtitle, collection}}` or `{"track": Track}`. Node
ids are non-empty. `collection: true` marks a node the user can keep in the library. A
non-null `cursor` means there's another page, fetched by sending that cursor back:

```
→ {"jsonrpc":"2.0","id":3,"method":"source.browse","params":{"node":"tones","cursor":null}}
← {"jsonrpc":"2.0","id":3,"result":{"entries":[{"track":{"key":"tone:A3","title":"A3, 220 Hz","artist":"rox","album_artist":"rox","album":"Tones","genre":"Test Tone","year":0,"disc_no":1,"track_no":1,"duration_ms":10000,"codec":"PCM","bitrate_kbps":705,"live":false}}, ...],"cursor":"4"}}
```

A Track has `key`, `title`, `artist`, `album_artist`, `album`, `genre`, `year`,
`disc_no`, `track_no`, `duration_ms`, `codec`, `bitrate_kbps` and `live`. Only `key` is
required. A field left out reads as empty, and null is refused: unknown text is `""`
and an unknown number is `0`.

`key` is opaque to rox and non-empty. It becomes the track's path in the library, so it
has to stay the same across sessions and plugin versions. A plugin that changes how it
builds keys orphans every row it made.

Search answers in the same page shape, so it can return nodes as well as tracks:

```
→ {"jsonrpc":"2.0","id":4,"method":"source.search","params":{"query":"minor","cursor":null}}
← {"jsonrpc":"2.0","id":4,"result":{"entries":[{"track":{"key":"chord:A minor","title":"A minor", ...}}],"cursor":null}}
```

A node can say what it is and carry a cover, each when `hello` listed the feature:

- `kind` (`node-kind`) is `album`, `playlist`, `artist` or `folder`, and picks the node's
  icon. Without one, a collection shows a playlist icon and anything else a folder.
- `art` (`node-art`) is a key rox passes to `source.cover` for the node's cover, like a
  track's `key`. It's opaque to rox and only has to mean something to the plugin's own
  `source.cover`. Without one, the node shows its icon in the same place, so node and
  track rows line up.

```
{"node": {"id": "album:500", "title": "Example Album", "subtitle": "Example Artist, 2019", "collection": true, "kind": "album", "art": "album:500"}}
```

An entry can also be a section, `{"section": {"title": "Albums"}}`, when `hello` listed
`sections`. rox draws it as a heading over the entries after it, and it can't be opened
or picked.

With `tiles` listed too, a section can ask for `"layout": "tiles"`. The entries under it,
up to the next section, show as a shelf: a row of covers over their titles that scrolls
sideways, the way a service's home page lays out new albums or mixes. A node on a shelf
opens on a click and a track plays on a double click, the same as their rows. Shelves
suit nodes with `art`; tracks read better as rows. Without `tiles`, the same section
shows its entries as rows. A page that's nothing but one tiles section wraps its covers
into a grid instead, so a node like a list of mixes opens as a wall of them.

```
{"section": {"title": "New albums", "layout": "tiles"}}
```

A plugin's pages can be whole screens of these. A node whose page is sections of shelves
and rows is how a service's home, its new releases or its genres reach rox.

The roots are what the panel shows before anyone searches, so a source with something to
discover, like a feed or a front page, lists it there as nodes.

A root node can mark itself the service's home with `"home": true`, when `hello` listed
`home`. rox then leaves it out of the roots and lists its page under them, paging on as
the user scrolls, so the home needs no click to open. The roots show first and don't wait
on it. The home page's fields, notice and views don't carry over, so a column the home
declares shows only when its node is opened on its own. Only the first node marked home counts, and only among the roots. The user
can turn this off in the panel's settings, and the node lists like any other.

```
{"node": {"id": "home", "title": "Home", "home": true}}
```

A track can name the nodes its album and artists open, in `go_to`, when `hello` listed
`go-to`. The source browser offers them under Go to in the track's menu. A node already
on the panel's trail is stepped back to, an album or artist page gives way to the next
one, and anywhere else the node opens on from the place shown. Each is a node in the same shape as a browse entry, and its `id` has to be one
`source.browse` answers for. `album` and `artists` are both optional, and several
artists are listed by name. The node the panel is already showing isn't offered. Sync
keeps `go_to` with the row, and so does a track the user plays or adds from the panel,
so the library's view of the source offers Go to too, and so does every other menu on
the row, like the queue's. From those, the node opens in an External Sources panel on
the plugin, a new one when none is open. A listing without `go_to` leaves
the kept one alone.

```
{"track": {"key": "t1", "title": "Example Song", "go_to": {"album": {"id": "album:500", "title": "Example Album", "collection": true, "kind": "album"}, "artists": [{"id": "artist:7", "title": "Example Artist", "kind": "artist"}]}}}
```

Either answer can carry a `notice`, a line rox shows above the entries, or on its own
when there are none:

```
← {"jsonrpc":"2.0","id":2,"result":{"entries":[],"cursor":null,"notice":{"text":"Add a cookies file in this plugin's settings to see your feeds.","kind":"setup"}}}
```

Send one only when `hello` listed `notice` in `features`. `kind` is `info`, the default,
or `setup` for a setting the plugin needs before it can
list something. A setup notice comes with a button that opens the Plugins page, where
the plugin's settings are. rox shows the notice from a place's first page and ignores it
on later ones. The text is the plugin's own and isn't translated, like the rest of what
it sends.

A notice can also carry a `link`, which puts a button on it that opens a web page in
the browser, like a sign-in page the user has to visit:

```
← {"jsonrpc":"2.0","id":2,"result":{"entries":[],"cursor":null,"notice":{"text":"Sign in at https://example.com/device with the code ABCDE.","link":{"url":"https://example.com/device?code=ABCDE","label":"Sign In"}}}}
```

Send one only when `hello` listed `notice-link` in `features`. The `url` has to be an
`http` or `https` address, or the page fails. `label` names the button and is optional;
without it the button reads Open Link. The page opens only when the user clicks.

#### Views

A place's first page can offer other ways to list it, when `hello` listed `views`: a
filter or an order the service applies, like search results narrowed to albums, or
favourites sorted by artist. rox shows them as chips over the list.

```
← {"jsonrpc":"2.0","id":4,"result":{"entries":[ ... ],"cursor":"50","views":[{"id":"all","label":"All"},{"id":"albums","label":"Albums"}],"view":"all"}}
```

`views` lists up to 12, each an `id` and a `label`. `view` names the one this page is,
and has to be one of them. Picking a chip asks for the place again with that `view`, and
every later page of it carries the same `view`:

```
→ {"jsonrpc":"2.0","id":5,"method":"source.search","params":{"query":"minor","cursor":null,"view":"albums"}}
```

rox only sends `view` once the place offered views, so a plugin that offers none never
sees it. Opening a node, searching again or going up a crumb starts over with no `view`.
A collection kept in the library lists from the library, so it shows no views.

#### Fields

A page can declare columns the service knows and a track's tags don't, like a
popularity or a play count, when `hello` listed `fields`. Tracks and nodes carry their
values under `values`, by field id:

```
← {"jsonrpc":"2.0","id":4,"result":{"entries":[{"track":{"key":"t1","title":"...","values":{"popularity":78}}}],"cursor":"50","fields":[{"id":"popularity","label":"Popularity","kind":"percent"}]}}
```

A page declares up to 4 fields, each an `id`, a `label` and a `kind`: `count` (a whole
number, shown short, like 12k), `percent` (0 to 100), `date` (`YYYY-MM-DD` or a prefix of
it) or `text`. A value is a number or a string, and a value for a field the page didn't
declare fails the page. rox takes the fields from a place's first page.

The External Sources panel shows each field as a column at the right of the rows, and its
heading sorts by it: counts, percents and dates biggest first, text A to Z, rows without
a value last, each heading's rows kept under it. A sort reads the rest of the list first,
up to 1,000 rows, since sorting one page would put the wrong rows on top. Fields stay in
the panel: a track kept in the library holds its tags and nothing from a field.

### source.sync

Pages through one collection the user keeps in the library:

```
→ {"jsonrpc":"2.0","id":5,"method":"source.sync","params":{"collection":"tones","token":null,"cursor":null}}
← {"jsonrpc":"2.0","id":5,"result":{"unchanged":false,"tracks":[ ...4 tracks... ],"cursor":"4","token":null}}
→ {"jsonrpc":"2.0","id":6,"method":"source.sync","params":{"collection":"tones","token":null,"cursor":"4"}}
← {"jsonrpc":"2.0","id":6,"result":{"unchanged":false,"tracks":[ ...2 tracks... ],"cursor":null,"token":"bbeea25a853c2c83"}}
```

The first page's request includes the token from the last complete sync, and only the
first page's does. Answering `"unchanged": true` there ends the sync with nothing
written, and rox keeps the token it has. The last page, the one with a null `cursor`,
holds the new token. rox replaces the collection's tracks only after the last page, so
a sync that fails partway changes nothing. A sync still paging after 2,000 pages is
treated as a plugin looping on its own cursor and fails.

Kept collections sync when the plugin starts and on Sync Now on the Plugins page.
Between syncs they browse from the library with no plugin call, so they still work
with the plugin stopped or the network down.

### source.open

```
→ {"jsonrpc":"2.0","id":7,"method":"source.open","params":{"key":"tone:A4"}}
← {"jsonrpc":"2.0","id":7,"result":{"stream":"s1","hint":"wav","length":882044,"seekable":true,"live":false}}
```

| Field      | Meaning                                                                                |
| ---------- | -------------------------------------------------------------------------------------- |
| `stream`   | An id the plugin chooses for this stream.                                               |
| `hint`     | A container extension for the probe, or `""` to let it sniff.                          |
| `length`   | The total byte count, or null when unknown.                                             |
| `seekable` | A read at any offset works.                                                             |
| `live`     | A stream with no end and no duration.                                                   |
| `buffer`   | Optional. `"ahead"` asks rox to read one chunk ahead instead of downloading the whole track. |
| `duration_ms` | Optional, when `hello` listed `open-duration`. The stream's length in milliseconds. |
| `chapters` | Optional, when `hello` listed `chapters`. Where the stream's parts start: `[{start_ms, title}]`. |

`buffer: "ahead"` is for a service that meters or throttles fast downloads. A plugin can
lower rox's buffering this way but never raise it.

`duration_ms` is for a container that doesn't state its own length, like fragmented MP4
without a segment index. rox needs a length to seek and to draw the waveform. The
container's own length wins over it, and without either rox falls back to the track's
`duration_ms` from browse or sync.

`chapters` mark an episode's segments or a mix's tracks. rox draws them along the top of
the seek strip, names one on hover and seeks to its start on a click. Send at most 500,
each title under the 4 KiB string cap. rox sorts them and skips blank titles, so a list
that isn't tidy still plays, but a malformed one (a missing field, an unknown one) fails
the open. A live stream's chapters are ignored.

### source.read

```
→ {"jsonrpc":"2.0","id":8,"method":"source.read","params":{"stream":"s1","offset":0,"len":262144}}
← {"jsonrpc":"2.0","id":8,"result":{"data":"UklGRnR1DQBXQVZFZm10IBAAAAABAAEARKwAAIhYAQACABAAZGF0YVB1DQAAAGcCzQQtB4cJ1wsaDlAQ..."}}
```

Up to `len` bytes from `offset`, base64. Fewer is fine, and an empty `data` means the
end of the stream. More than `len` is refused. rox asks for 256 KiB at a time and never
more than 512 KiB, so an answer stays under the 1 MiB line cap once encoded.

rox never sends overlapping ranges on one stream. A seekable stream gets its next chunk
requested while the last one is still in use. A stream that can't seek gets one read at
a time, each starting where the last answer ended. A plugin still has to accept a read
on one stream while a read on another is running.

A plugin that loses its upstream mid-stream should recover inside `source.read` and
answer an error only when that fails. See [Streams](#streams) for what rox does with
the error.

### source.close

```
→ {"jsonrpc":"2.0","id":9,"method":"source.close","params":{"stream":"s1"}}
← {"jsonrpc":"2.0","id":9,"result":null}
```

Sent when rox drops the stream. Nothing waits on the answer.

### source.cover

```
→ {"jsonrpc":"2.0","id":10,"method":"source.cover","params":{"key":"tone:A4"}}
← {"jsonrpc":"2.0","id":10,"result":null}
```

`{"mime": "..", "data": ".."}` with the image base64, or null for no cover. rox asks
only for tracks with no stored cover.

### source.radio

Sent only to a plugin whose manifest says `"radio": true`. rox offers Play Similar on its
tracks and nodes, and asks for a station seeded from the one picked, a track's `key` or
a node's `id`:

```
→ {"jsonrpc":"2.0","id":12,"method":"source.radio","params":{"seed":"t1","cursor":null,"count":20}}
← {"jsonrpc":"2.0","id":12,"result":{"tracks":[ ...20 tracks... ],"cursor":"20"}}
```

A node's own tracks play first (an album before its artist's radio), read through
`source.browse` in the plugin's default view. A track only seeds: it never plays as part
of its own station, so it's left out of every batch. The station's first batch follows,
less anything already in the lead, and rox
turns Similar shuffle on. A plugin with a radio is enough to offer Similar shuffle
without the library's acoustic analysis, and the random button's Similar draw while one
of the plugin's tracks plays starts the same way from that track. The rest
come as the queue runs down, the way rox's own
continuation fills it, each asking with the last answer's `cursor`. A null `cursor`
means the station ran out, and rox seeds a new one from the last of the plugin's tracks
that played. A batch holds at most `count` tracks, 500 at the most. Its tracks play
without joining the library, the way tracks played from the panel do, and ones that
already played this session are skipped. A queue started from the plugin's panel,
say an album, goes on the same way when it runs low, seeded from the last of its tracks
that played, rather than falling through to the local library. rox saves the station
with the queue, so it carries on after a restart. A radio's tracks keep coming only while
continuation is on in rox's playback settings.

### source.link

Sent only to a plugin whose manifest says `"links": true`. rox then offers Open in
Browser and Copy Link on the plugin's tracks and nodes, everywhere they show: the
External Sources panel, the queue, the library, playlists, history and what's playing.
They show on one item at a time, not on a selection. When the user picks one, rox asks
for the page of the item, a track's `key` or a node's `id`:

```
→ {"jsonrpc":"2.0","id":13,"method":"source.link","params":{"item":"t1"}}
← {"jsonrpc":"2.0","id":13,"result":{"url":"https://example.com/track/t1"}}
```

The `url` has to be an `http` or `https` address, or the call fails. Answer null for an
item with no page. rox asks on every click rather than keeping the answer, so a link
can change without anything going stale. The page opens in the user's browser or goes
to the clipboard, and rox itself never fetches it.

### source.lyrics

Sent only to a plugin whose manifest says `"lyrics": true`, and only once the user
switches Lyrics on for it on the Plugins page. That switch starts off. With it on, the
Lyrics panel asks the plugin for its own tracks' sheets before it asks the lyrics
providers, and saves the answer like a match the user picked:

```
→ {"jsonrpc":"2.0","id":14,"method":"source.lyrics","params":{"key":"chord:A minor"}}
← {"jsonrpc":"2.0","id":14,"result":{"text":"[00:00.00]A3\n[00:05.00]C4\n[00:10.00]E4","synced":true}}
```

`text` is LRC when `synced` is true, timed lines rox highlights as they play, and plain
lines otherwise. Answer null for a track with no sheet, and rox goes on to the
providers. The text can run to 256 KiB, past the usual string cap.

### Actions

A source can offer things to do with its tracks and nodes, like downloading one. Each
action in `capabilities.source.actions` has an `id`, a `label`, and `on`, the places rox
offers it:

```json
"actions": [
  {
    "id": "export",
    "label": "Export WAV",
    "on": ["track", "node"],
    "params": {
      "type": "object",
      "properties": {
        "seconds": { "type": "integer", "title": "Length", "minimum": 1, "maximum": 60, "default": 10 }
      }
    }
  }
]
```

| `on`     | Where rox offers it                                                                   |
| -------- | ------------------------------------------------------------------------------------- |
| `track`  | The menu of the plugin's tracks, everywhere they show, on one or a selection of only its tracks. |
| `node`   | The menu of its nodes in the External Sources panel, on one or a selection.            |
| `source` | The External Sources panel's own menu, with no item.                                    |

A name rox doesn't know is skipped. A plugin can declare up to 16 actions, and an `id`
that repeats, an empty `label` or an empty `on` refuses the plugin. The label shows as
written, untranslated.

`icon` is optional: an SVG in the plugin's folder, held to the same rules as the source
icon (see [The icon](#the-icon)), and a bad one refuses the plugin the same way. Without
one, rox draws a plug.

`when` is optional too: a flag name, or one negated with `!`, that rows have to carry or
lack for rox to offer the action. A pair like Add to Favourites and Remove from
Favourites declares `"when": "!favourite"` and `"when": "favourite"`. Flag names are
lowercase letters, digits, `-` and `_`.

When `hello` listed `flags`, a track or node can carry `flags`, a list of what it is
right now, up to 16. Leaving `flags` out says the plugin doesn't know, and `[]` says the
row has none. rox offers an action with a `when` if any picked row can take it, and a row
whose flags it doesn't know can take any. Sync ignores `flags`, since they go stale.

A library row holds only its tags, so rox asks for its flags with `source.flags`, once the
plugin is up and again after each sync, up to 500 keys a call. rox only asks a plugin
that gives some action a `when`. An item left out of the answer stays unknown, and a
plugin that doesn't answer the call leaves every library row offering every action:

```
→ {"jsonrpc":"2.0","id":20,"method":"source.flags","params":{"items":["t1","t2"]}}
← {"jsonrpc":"2.0","id":20,"result":{"flags":{"t1":["favourite"],"t2":[]}}}
```

Between those calls, a library row's menu uses the newest flags rox saw for its track, in
a listing, a `source.flags` answer or an action's answer.

```
{"track": {"key": "t1", "title": "Example Song", "flags": ["favourite"]}}
```

An action's answer, or a job's last state, can carry `flags` for the items it changed,
keyed by the same keys and ids as `items`. rox merges them into what it shows, so the
next menu on those rows reflects the action without the panel listing the place again:

```
← {"jsonrpc":"2.0","id":14,"result":{"message":"Added to favourites","flags":{"t1":["favourite"]}}}
```

`params` is optional. When an action has it, picking the action opens a dialog with a row
per property, drawn from the same subset of JSON Schema as `config_schema`: strings,
`"format": "password"`, numbers, integers, booleans and enums. A property's `default`
fills its row, and the names in `required` have to be filled before Run works. Without
`params`, picking the action runs it at once.

A selection is one call. `items` holds the tracks' keys or the nodes' ids, and is empty
for an action offered on `source`:

```
→ {"jsonrpc":"2.0","id":14,"method":"source.action","params":{"action":"export","items":["tones"],"params":{"seconds":10}}}
← {"jsonrpc":"2.0","id":14,"result":{"job":"j1"}}
```

An action that finishes within the call answers its outcome instead of a job: a
`message`, and optionally a `link` and a `reveal`. rox shows the message in a toast, or
"<label> finished" when there's none. A `link` has to be an `http` or `https` address and
becomes an Open Link button. A `reveal` has to be an absolute path and becomes a Show in
Folder button, which shows it in the file manager and never opens it. An answer that's
only a `reveal`, with no `message` and no `link`, is the action itself: rox shows the
path in the file manager at once, with no toast. A toast with a button stays until the
user presses one or dismisses it. `null` is an outcome with
nothing in it. An error shows in a toast that stays until dismissed.

A job is work that outlasts a call. rox lists it in the Tasks window and asks
`source.job` about once a second until it ends:

```
→ {"jsonrpc":"2.0","id":15,"method":"source.job","params":{"job":"j1"}}
← {"jsonrpc":"2.0","id":15,"result":{"done":3,"total":6,"text":"C4, 262 Hz"}}
→ {"jsonrpc":"2.0","id":19,"method":"source.job","params":{"job":"j1"}}
← {"jsonrpc":"2.0","id":19,"result":{"done":6,"total":6,"text":"","finished":true,"message":"Exported 6 tones","reveal":"/home/me/.local/share/rox/plugin-data/tones/exports"}}
```

| Field      | Meaning                                                                       |
| ---------- | ----------------------------------------------------------------------------- |
| `done`     | How far it got, in whatever unit suits it, like files or bytes.                  |
| `total`    | What `done` counts up to, or 0 when the plugin can't tell.                       |
| `text`     | A line about what it's doing now.                                                |
| `finished` | The job ended well. The outcome fields (`message`, `link`, `reveal`) come with it. |
| `error`    | The job failed, and why. rox shows it in a toast that stays until dismissed.      |

A job has no time limit, but every poll has the listing timeout, and a poll that fails
ends the job as failed. A plugin that restarts loses its jobs that way, so it should
keep a job's state for as long as the plugin runs and forget it once it has answered the
end.

Stop in the Tasks window sends `source.cancel` with the job's id, then keeps polling. The
plugin stops the work and answers the next poll with an `error`. rox stops asking ten
seconds after the cancel either way.

Work runs in the plugin, with the plugin's own access. A file it saves goes under the
`data_dir` from `hello`, or a folder the user names in its settings, and never into its
own folder.

### shutdown

```
→ {"jsonrpc":"2.0","id":11,"method":"shutdown"}
← {"jsonrpc":"2.0","id":11,"result":null}
```

Then the plugin exits. It should also exit whenever stdin closes, since that's the one
signal that reaches a plugin on every OS when rox goes away without a shutdown.

## Streams

The plugin hands rox container bytes, and rox decodes them with the same decoder it
uses for files. It reads CAF, MP4, Matroska, Ogg, AIFF, WAV, and raw ADTS AAC and MP3.
There's no MPEG-TS reader, so a segmented upstream is the plugin's to join into one
continuous stream in one of those containers.

How rox reads depends on what `source.open` said:

- A seekable stream of known length up to 64 MB, without `buffer: "ahead"`, is
  downloaded whole while it plays, front to back. A seek past the download moves it
  there, and the skipped part fills in after. The seekbar shows what's downloaded.
- Anything else is read one 256 KiB chunk ahead of playback.

Once a track has played for 2 seconds, rox opens the next two entries in the queue
ahead of time when they're plugin tracks, so the next track doesn't wait on
`source.open`. A pre-opened stream nobody plays within 60 seconds is closed. Live
streams are never pre-opened.

When a read fails mid-track, rox closes the stream and opens it again, waiting 0, 1, 2
and 4 seconds before the attempts. The transport shows Reconnecting meanwhile. A
seekable stream of known length resumes at the byte it failed on, provided the reopened
stream has the same length. A different length is a different encode, and rox won't
splice two. A stream that can't seek or has no length can't resume and ends at once.
When the attempts run out, the track ends with an error naming the plugin, and the
queue moves on. A dead or slow plugin never stops local playback.

A live stream answers each read with what it has ready instead of waiting to fill `len`.
A 256 KiB read at 128 kbps takes 16 seconds to fill, past the read timeout. rox keeps
pulling a live stream while it's paused, so a pause resumes where it stopped and the
last minutes stay seekable. A pause longer than 30 minutes hangs up, and Play rejoins
at the live edge.

## Tracks in the library

A track enters the library when the user keeps a collection that holds it, or adds it
on its own with Add to Library. Syncing a collection makes it hold exactly the tracks
the sync returned, and a track no kept collection holds any more, and that the user
didn't add, leaves the library.

Playing, queueing or adding a track to a playlist from the External Sources panel plays
it without adding it to the library. It still has a place in the queue, history and
the playlist, and it stays out of the library's views and search. One that isn't
played or picked again for 30 days, and isn't in the saved queue, is deleted when rox
starts. Its playlist entries and play history reattach if it comes back.

A track's menu in the library, a playlist, the queue or history takes it back out the
way it came in. A track added on its own gets Remove from Library. A track a kept
collection holds gets Stop Keeping for that collection, which lets go of the whole
collection like its switch in the External Sources panel. There's no way to take one
track out of a kept collection, since the next sync would put it back. Under each
switched-on plugin, the Plugins page counts its tracks in the library and those added
on their own, and Show in Library narrows the shared search to the plugin's source.

A plugin's tracks show only while it's switched on and its folder is present. A
switched-off plugin's tracks are hidden, and so are those of a plugin whose folder is
gone, since deleting the old folder is how many people update a plugin. They're
deleted only on Remove. Playlist entries and play history reattach when a track comes
back with the same key.

A plugin's tracks scrobble only when its manifest declares `"scrobble": true` and the
user leaves Scrobble Plays on for it. Approving a manifest that declares scrobbling
turns Scrobble Plays on, since the card just said so.

## Panels

Every running plugin is listed under Add Panel > Plugins, and in the Panels menu, New
Window from Panel and an empty window's panel list. Picking it opens the External Sources
panel on the plugin's source. A plugin needs nothing in its manifest for that.

`capabilities.panels` adds more panels beside it, and then the plugin's entry opens into
a list: External Sources first, then its own. Each one is a preset of a panel rox
already has: a name for the entry, and a `preset` holding the panel's saved state.
Nothing in it runs. A plugin can't add a new kind of panel or draw one. A library panel
showing only the plugin's kept tracks looks like this:

```json
"panels": [
  {
    "name": "Library",
    "preset": {
      "panel_name": "library",
      "info": { "panel": { "query": "source:tones", "search": true } }
    }
  }
]
```

`panel_name` is the kind, and `info.panel` is that kind's settings, the same object rox
saves for a panel preset. The easy way to get one is to set a panel up in rox, save it
with Save As Preset from its menu, and copy its `panel` from `bundle.panel_presets` in
`workspace.json` in rox's data folder. A `source browser`, the External Sources panel,
has to name the plugin's own source, and it takes the External Sources entry's place
rather than adding a second one, so a plugin can give that panel its own title or look.

A `panel_name` this version of rox doesn't have is skipped, and the rest of the plugin
works as usual, so a plugin can declare a kind a newer rox added. Entries list only
while the plugin runs, which takes a `source` capability.

A panel added from one of these entries remembers its plugin. A saved workspace lists
the plugins its panels came from under `requires`, and applying it names any that
aren't running. The panels restore either way, since they're panels rox has and the
plugin only fills them.

## Process lifecycle

A plugin gets one process, started by the first call that needs it. Switching a plugin
on syncs its kept collections, so it starts right away. The process runs in the
plugin's folder with rox's environment (HOME and the rest), except PATH: that's the
plugin's own `bin/` folder, then Program Folders, then rox's own PATH, joined the way
the OS joins one (`:` on Linux and macOS, `;` on Windows). A program the Plugins page
found is the same file the plugin's own PATH resolves to. Changing Program Folders
rescans every plugin's `programs` row; a plugin already running keeps its old PATH
until it next restarts.

rox also sets `PYTHONDONTWRITEBYTECODE=1`, `PYTHONUNBUFFERED=1` and `PYTHONUTF8=1` for
every plugin. The last two only help a Python plugin, and only cover what rox itself
starts: running a script by hand, the way [Trying a plugin without
rox](#trying-a-plugin-without-rox) does, doesn't set them, so a script still has to
flush after every line and write UTF-8 on its own.

When the plugin exits unexpectedly, every call it had in flight fails, and the next
call starts it again after 0, 1, 2 or 4 seconds for the first through fourth crash
within ten minutes. The fifth stops it with "Stopped after repeated crashes" on its row
until the user switches it off and on. A plugin that can't start or fails `hello`
counts as a crash.

Switching a plugin off sends `shutdown`, waits up to 2 seconds for requests still in
flight, then closes stdin and kills what's left. Quitting rox skips the wait. A changed
config restarts the plugin.

On Linux and macOS the plugin leads its own process group, and stopping it kills the
whole group, so programs it started go with it. A child that moves itself to another
group escapes, as it would from a shell. On Windows the plugin starts with no console
window and goes into a job object, and stopping it ends everything in the job. A
grandchild started in the instant between the spawn and the job assignment can escape.

## Platforms

The Flatpak build runs plugins inside rox's own sandbox, which can't see programs on
the host. The Flatpak runtime includes Python 3, so a script plugin that only needs
the standard library runs as is. Anything else has to ship under the plugin's own
`bin/`: a Python zipapp runs through that same Python, and a native Linux binary
needs its executable bit set before it's dropped in. Program Folders only helps with
directories the sandbox can already see.

A macOS app launched from Finder or the Dock doesn't inherit a shell's PATH, so
Homebrew's folders are usually missing from it. Add them to Program Folders. Plugins
and Convert both search it.

Script plugins need Python installed on the machine running them. Most Linux
installs already have it. Windows and macOS usually don't.

On Windows, a native plugin's own files can't be replaced or deleted while it's
running. Switch it off on the Plugins page before updating its folder.

## Timeouts and caps

| What                                                 | Limit                          |
| ---------------------------------------------------- | ------------------------------ |
| `hello`                                              | 30 s                           |
| `source.browse`, `source.search`, later sync pages   | 15 s                           |
| `source.radio`, `source.link`, `source.lyrics`       | 15 s                           |
| `source.action`, `source.job`, `source.cancel`       | 15 s                           |
| A sync's first page                                  | 60 s                           |
| `source.open`                                        | 20 s                           |
| `source.read`, `source.cover`                        | 10 s                           |
| `shutdown`                                           | 2 s, then killed               |
| A line on stdout                                     | 1 MiB                          |
| A string in a result                                 | 4 KiB                          |
| Entries or tracks per page                           | 500                            |
| Actions a plugin declares                            | 16                             |
| Views a page offers                                  | 12                             |
| Fields a page declares                               | 4                              |
| Chapters an open names                               | 500                            |
| A lyrics sheet                                       | 256 KiB                        |
| Rows read to sort by a field                         | 1,000                          |
| Tracks read from a node to play it or start a radio  | 1,000                          |
| A read's `len`                                       | 256 KiB asked, 512 KiB at most |
| A stderr line                                        | 4 KiB                          |
| Sync pages per collection                            | 2,000                          |

A call that times out fails on its own, and the plugin keeps serving its other calls.
A longer stdout line is dropped with a warning in the log. A result over a cap fails
that call.

## What a plugin can't do

- Run code in rox's UI or draw anything.
- Process audio or hook the engine. Decoding stays in rox.
- Hand rox a URL to fetch. rox never fetches an address a plugin chose, and bytes make
  expiring, segmented or shifting URLs the plugin's problem.
- Call rox or connect to the [control socket](README_IPC.md). The socket authenticates
  by filesystem permission and can't tell a plugin from any other local program.
- Write into its own folder without switching itself off.
- Scrobble without declaring it.

## Developer mode

Every save to a plugin you're writing changes its folder's hash, which switches it off
until you approve it again. Developer mode, the terminal button beside a switched-on
plugin's switch on the Plugins page, approves each change on its own instead, and the
plugin restarts on the new files.
It lasts until rox quits or you switch the plugin off, and it's never saved.

It approves only changes that keep the manifest's declarations. A save that adds a
capability or a program, changes the entry, or turns scrobbling on or off switches the
plugin off as usual, and switching it back on shows the card with what changed.
Developer mode stays on through that. A folder that stops loading mid-edit, say with a
half-written manifest, keeps its switch and starts again once it loads.

## Trying a plugin without rox

A plugin is a program that reads lines on stdin, so a shell can drive it. From the
tones folder:

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"hello","params":{"api":1,"config":{},"data_dir":"/tmp/tones","platform":"linux-x86_64"}}' \
  '{"jsonrpc":"2.0","id":2,"method":"source.browse","params":{"node":null,"cursor":null}}' \
  | python3 -B -u tones.py
```

`-B` keeps Python from writing `__pycache__` into the folder, which would change its
hash. `-u` (or `PYTHONUNBUFFERED=1`) flushes stdout after every line. rox sets it for a plugin
it starts, and a shell doesn't. Without it, answers show up only when the process exits.
