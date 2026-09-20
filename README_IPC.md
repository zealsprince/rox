# The control socket

rox's machine interface: newline-delimited JSON-RPC 2.0 over a Unix domain socket on
Linux and macOS, a named pipe on Windows. Anything running as your user can read the
library, watch the player, and drive playback through it. Media keys and desktop
now-playing use MPRIS as usual; this is the surface for front ends and scripts.

The socket binds when rox launches and stays up until it quits, including while rox
is windowless in the tray. Each data directory gets its own socket, so a
`--portable` or `--fresh` run has its own control surface instead of steering the
daily driver.

## Where to find it

| Platform     | Path                                                                                        |
| ------------ | ------------------------------------------------------------------------------------------- |
| Linux, macOS | `$XDG_RUNTIME_DIR/rox-ipc-<hash>.sock`, or the same name in the data directory when no runtime dir exists |
| Windows      | `\\.\pipe\rox-ipc-<hash>`                                                                    |

The hash is derived from the data directory. Settings > Application > Control Socket
shows the exact path, with buttons to copy it or reveal it in the file manager.

## Wire format

One JSON object per line, LF-terminated, UTF-8, frames capped at 1 MiB. A request
has `id`, `method`, and optional `params`; every request gets exactly one response
frame echoing the id, with either `result` or `error`.

A connection opens with a handshake naming the protocol generation it speaks. Every
other method is refused until then:

```
→ {"id": 1, "method": "hello", "params": {"protocol": 1}}
← {"jsonrpc": "2.0", "id": 1, "result": {"name": "rox", "version": "1.19.0", "protocol": 1}}
```

The protocol number moves only on breaking changes. New methods and new response
fields arrive without a bump, so clients should ignore fields they don't know.

## Methods

Transport verbs return the full player status, so a caller sees what its command
did without a second round trip.

| Method                                                     | Params                                        | Answers                                                                |
| ---------------------------------------------------------- | --------------------------------------------- | ---------------------------------------------------------------------- |
| `transport.status`                                         |                                               | playing, position, duration, volume, mute, A-B section, queue revision, current track |
| `transport.toggle` `.play` `.pause` `.next` `.prev` `.stop` |                                               | status                                                                 |
| `transport.seek`                                           | `{"to": secs}` or `{"by": secs}`              | status                                                                 |
| `transport.set_volume`                                     | `{"volume": 0..2}`                            | status                                                                 |
| `transport.ab`                                             | `{"action": "mark"/"clear"}` or `{"a": secs, "b": secs}` | status                                                      |
| `queue.list`                                               |                                               | every entry with its stable id, path, explicit flag, and current marker |
| `queue.add`                                                | `{"paths": [..], "mode": "end"/"next"/"now"}` | `{"queued": n}`                                                        |
| `queue.remove`                                             | `{"ids": [..]}` or `{"id": n}`                | null                                                                   |
| `queue.move`                                               | `{"id": n, "after": n?}`                      | null                                                                   |
| `queue.jump`                                               | `{"id": n}`                                   | null                                                                   |
| `library.search`                                           | `{"query": "..", "limit": 1..500}`            | total hit count plus rows with tags and the path that plays them        |
| `library.now_playing`                                      |                                               | the playing track's full tags, null while nothing plays                 |
| `library.artwork`                                          | `{"path": ".."}`                              | `{"mime": "..", "data_base64": ".."}`                                   |
| `library.rescan`                                           |                                               | `{"started": true}`; an error while busy or without library folders     |
| `tasks.status`                                             |                                               | the analysis passes: switch state, tracks to do, progress while running |
| `tasks.start`                                              | `{"pass": "acoustic"/"replaygain"/"tempo"/"sortnames"/"romanize"}` | what the pass took on: count, workers, estimate, save mode              |
| `tasks.stop`                                               | `{"pass": ..}`                                | `{"stopping": true}`; the pass drops out at the next file               |
| `ai.status`                                                |                                               | `{"enabled": bool, "mcp": bool}`, the toggles rox-mcp checks            |
| `subscribe`                                                |                                               | the pushed frames start: `{"subscribed": true}`                          |
| `debug.settings`                                           |                                               | the settings as saved                                                  |
| `debug.panels`                                             |                                               | the frontmost workspace's panel tree, as the layout persist writes it   |
| `debug.thumbs`                                             |                                               | the artwork service's counters: requests, starts, lands, refused, pending, entries |
| `debug.workspace`                                          | `{"name": ".."}`                              | null; applies that workspace to the front window                         |
| `debug.entities`                                           |                                               | `{"total": n, "by_type": [[name, count], ..]}`, live gpui entity counts  |
| `debug.device_lost`                                        |                                               | `{"faulted": true}`; raises the output's device-lost flag, as an unplug does |
| `debug.windows`                                            |                                               | every open window: id, title, logical size, scale, and which is active   |
| `debug.actions`                                            | `{"filter": ".."}`, optional                  | `{"actions": [..]}`, the dispatchable action names                       |
| `debug.action`                                             | `{"name": "..", "data": any?, "window": id?}` | null; dispatched down that window's focus chain                          |
| `debug.key`                                                | `{"keys": "ctrl-comma escape", "window": id?}` | `{"handled": [bool, ..]}`, one per keystroke                             |
| `debug.type`                                               | `{"text": "..", "window": id?}`               | `{"typed": n}`, characters typed into the focused element                |
| `debug.click`                                              | `{"x": .., "y": .., "button": ..?, "count": 1..3?, "modifiers": {..}?, "window": id?}` | null                                                                     |
| `debug.hover`                                              | `{"x": .., "y": .., "window": id?}`           | null                                                                     |
| `debug.scroll`                                             | `{"x": .., "y": .., "dy": .., "dx": ..?, "window": id?}` | null                                                                     |

Queue entry ids are stable handles: `queue.list` returns them, and remove, move,
and jump name entries by them, so an edit can't hit the wrong row when the queue
shifts underneath it. `queue.add` takes files and folders, filters to decodable
audio, and accepts `path#N` for a cue sheet's Nth track, the same spelling the m3u
export uses. `mode` places the batch: `end` behind what's queued, `next` right after
the playing track, `now` splices and plays.

`transport.ab` drives the A-B repeat the transport button runs. `mark` steps the
same three-press cycle: the first marks A at the current position, the second marks
B and starts the section repeating, the third clears it. `clear` drops the section
wherever the cycle stands, and `a` with `b` sets one outright, both in track
seconds and at least a quarter second apart. Status carries the section as `ab`:
null with nothing marked, `{"a": secs}` while the cycle waits for B, and both ends
once it repeats. A skip or a seek outside the section clears it, and the change
shows up as an `event.playback` frame.

Search uses the panels' query language: free terms match title, artist, album artist,
album, and genre, while a pin like `artist:name` or `year:1990` narrows one field. The
fields that take a pin are title, artist, albumartist, album, genre, year, folder,
codec, rating, plays, and added. `limit` defaults to 50 and caps at 500.

The tasks methods drive the long passes the tasks window runs. The UI puts an
estimate and a worker slider in front of every start because a pass can cost an
afternoon and, in tags save mode, rewrites audio files; over the socket that
context comes back in the start reply instead, so the caller sees what it just
set going.

The debug methods let a script or an agent verify player and layout state against a
live instance without eyes on the screen. The drive half goes further and works the
UI: `debug.action` dispatches an action as its keybinding would, and key, type,
click, hover, and scroll land synthetic input in gpui's own event pipeline, so no OS
input tools are involved. Coordinates are window-local logical pixels, and every
drive method takes an optional `window` id from `debug.windows`, defaulting to the
active window.

`subscribe` enrols the connection in pushed frames. They arrive on the same
connection as `event.track`, `event.playback`, and `event.queue` objects with a
`method` but no `id`, which is what tells a push apart from a response. A subscriber
that stops reading is dropped rather than allowed to block the sender.

## Errors

Failures come back as JSON-RPC error objects with the standard codes where they
apply and the `-32000` range for rox's own:

| Code   | Meaning                                                            |
| ------ | ------------------------------------------------------------------ |
| -32700 | parse error                                                        |
| -32600 | invalid request (including a frame past the 1 MiB cap)             |
| -32601 | method not found                                                   |
| -32602 | invalid params                                                     |
| -32000 | the app looked and couldn't answer (no library, no art, bad path)  |
| -32001 | handshake required: call `hello` first                             |
| -32002 | unsupported protocol generation                                    |
| -32003 | no answer from the app in 30 seconds; retry rather than assume     |
| -32004 | client-side only: the connection itself failed                     |

## roxctl

The reference client, for developing against the socket and testing it. It
doesn't ship with releases; build it from the repo with
`cargo build --release --package rox-cli`. One call per invocation; `--json`
prints raw results for scripts, the default output is lines for people.

```
roxctl [options] <command> [args]

options:
  --socket <path>    talk to this socket instead of deriving it
  --data-dir <path>  derive the socket for this data dir (a --portable rox)
  --window <id>      aim drive commands at this window (see `windows`)
  --json             print raw JSON results

commands:
  status                     what's playing and where its clock sits
  toggle | play | pause      the deck
  next | prev | stop
  seek <secs|+secs|-secs>    absolute, or relative when signed
  volume <0..2>
  ab [mark|clear|<a> <b>]    A-B repeat: step the cycle (default), clear
                             it, or set a section in seconds outright
  queue                      the play order with entry ids
  add [--next|--now] <paths> queue files (default: end of the queue)
  remove <id...>             drop queued entries by id
  jump <id>                  play a queued entry now
  search [--limit N] <terms> search the library
  now                        the playing track's full tags
  rescan                     scan the library folders again
  tasks                      the long analysis passes and their progress
  task-start <pass>          start a pass: acoustic, replaygain, tempo,
                             sortnames, romanize
  task-stop <pass>           stop a running pass at the next file
  watch                      follow playback, track, and queue events
  art <path> <out-file>      save a track's cover art
  raw <method> [json]        any method, params as one JSON argument

drive commands (the debug scope: work the UI without OS input tools):
  windows                    open windows with the ids drive commands take
  actions [filter]           dispatchable action names
  action <name> [json]       dispatch an action by name, data as JSON
  key <keystrokes...>        send keystrokes, e.g. ctrl-comma escape
  type <text...>             type into the focused element
  click <x> <y>              click at window-local logical pixels
                             (--right, --middle, --double)
  hover <x> <y>              move the mouse to a point
  scroll <x> <y> <dy> [dx]   scroll at a point, wheel lines, signed
  panels                     the frontmost workspace's dock tree
```

No rox listening exits 2 with a sentence on stderr; a refused method exits 1 with
the server's error. `raw` covers anything the verbs don't:

```
roxctl raw queue.move '{"id": 3, "after": 7}'
```

## Security

The surface is local, never a network port. Auth is filesystem permissions: the
Unix socket is created user-only (0600), and the Windows pipe uses the platform's
default per-session access control. Remote access means proxying the socket
yourself.
