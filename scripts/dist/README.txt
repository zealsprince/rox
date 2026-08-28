                              ##
                              #####
               #             ### ####
          ######            ###     ###
        ####  ##          ###         ###
      ###    ###   ###  ####           ###
    ###      ############               ###
    ##        ##                         ##
   ##                                    ##
   ##                                    ##
   ##                                   ###
   ###                                 ###
    ##                                ###
    ###                             ###
     ###                          ###
      ###                       ###
        ###                  ####
         ###               ###
           ###           ###
             ###      ####
               #### ####
                 ###

                 rox

============================================================================

A desktop music player for large, carefully tagged local libraries: panels
you compose yourself, themes as shareable workspace files, deep tagging, and
playback that stays fast at tens of thousands of tracks.

============================================================================

This is about running the build you just unpacked. Screenshots, the feature
rundown, and the docs are at https://rox.music and
https://github.com/zealsprince/rox.


Running
-------

Linux: run ./rox from this folder. For an app menu entry, edit the Exec=
lines in rox.desktop to the binary's full path, then copy it to
~/.local/share/applications/ and rox.svg to
~/.local/share/icons/hicolor/scalable/apps/.

Windows: run rox.exe. If SmartScreen objects, choose More info, then Run
anyway. If you'd rather have a Start menu entry and in-place upgrades, the
releases page has a -setup.exe installer.

rox updates itself: it verifies each release's checksum and swaps the binary
when the folder it's in is writable. When it isn't, it notifies you about
new releases instead.


Command line
------------

rox <files or folders>   Play them now, replacing what's loaded. Folders
                         expand to the audio files directly inside them.
--enqueue / -e           Append the given files to the up-next queue instead
                         of playing.
--new-instance           Start a second rox against the same data directory.
                         Without it a launch passes its files to the rox
                         already running, which raises its window and takes
                         them. Linux and macOS only; on Windows every launch
                         is its own instance.
--portable               Keep all data (library, settings, caches) in a
                         rox-data folder beside the executable for this run.


Portable mode
-------------

To stay portable across launches instead of passing --portable every time,
drop an empty file named "portable" next to the executable, or flip the
toggle in the Behavior settings. Everything then stays in rox-data beside
the executable, and the whole folder moves between machines.


IPC
---

The control socket is rox's machine interface: newline-delimited
JSON-RPC 2.0 over a Unix domain socket on Linux and macOS, a named pipe on
Windows. Anything running as your user can read the library, watch the
player, and drive playback through it. The socket binds when rox launches
and stays up until it quits, including while rox is windowless in the
tray. Each data directory gets its own socket, so a --portable run has
its own control surface instead of steering the daily driver.

Where to find it:

  Linux, macOS   $XDG_RUNTIME_DIR/rox-ipc-<hash>.sock, or the same name
                 in the data directory when no runtime dir exists.
  Windows        \\.\pipe\rox-ipc-<hash>

The hash is derived from the data directory. Settings > Application >
Control Socket shows the exact path, with buttons to copy it or reveal it
in the file manager.

Wire format: one JSON object per line, LF-terminated, UTF-8, frames
capped at 1 MiB. A request has id, method, and optional params; every
request gets exactly one response frame echoing the id, with either
result or error. A connection opens with a handshake naming the protocol
generation it speaks, and every other method is refused until then:

    > {"id": 1, "method": "hello", "params": {"protocol": 1}}
    < {"jsonrpc": "2.0", "id": 1,
       "result": {"name": "rox", "version": "1.21.0", "protocol": 1}}

The protocol number moves only on breaking changes. New methods and new
response fields arrive without a bump, so ignore fields you don't know.

Methods. Transport verbs return the full player status, so a caller sees
what its command did without a second round trip:

  transport.status       Playing, position, duration, volume, mute, queue
                         revision, current track.
  transport.toggle       Also .play .pause .next .prev .stop; all return
                         status.
  transport.seek         {"to": secs} or {"by": secs}
  transport.set_volume   {"volume": 0..2}
  queue.list             Every entry with its stable id, path, explicit
                         flag, and current marker.
  queue.add              {"paths": [..], "mode": "end"/"next"/"now"}
  queue.remove           {"ids": [..]} or {"id": n}
  queue.move             {"id": n, "after": n?}
  queue.jump             {"id": n}
  library.search         {"query": "..", "limit": 1..500}; total hit
                         count plus rows with tags and the path that
                         plays them.
  library.now_playing    The playing track's full tags; null while
                         nothing plays.
  library.artwork        {"path": ".."} returns {"mime", "data_base64"}.
  ai.status              {"enabled", "mcp"}, the toggles rox-mcp checks.
  debug.settings         The settings as saved.
  debug.panels           The frontmost workspace's panel tree.

Queue entry ids are stable handles: queue.list returns them, and remove,
move, and jump name entries by them, so an edit can't hit the wrong row
when the queue shifts underneath it. queue.add takes files and folders,
filters to decodable audio, and accepts path#N for a cue sheet's Nth
track, the same spelling the m3u export uses. mode places the batch: end
behind what's queued, next right after the playing track, now splices and
plays.

Search uses the panels' query language: free terms match title, artist,
album, and genre, while artist:name, album:name, genre:name, and
year:1990 pin one field. limit defaults to 50 and caps at 500.

Failures come back as JSON-RPC error objects, standard codes where they
apply and the -32000 range for rox's own:

  -32700   parse error
  -32600   invalid request (including a frame past the 1 MiB cap)
  -32601   method not found
  -32602   invalid params
  -32000   the app looked and couldn't answer (no library, no art,
           bad path)
  -32001   handshake required: call hello first
  -32002   unsupported protocol generation
  -32003   no answer from the app in 30 seconds; retry rather than assume
  -32004   client-side only: the connection itself failed

The surface is local, never a network port. Auth is filesystem
permissions: the Unix socket is created user-only (0600), and the Windows
pipe uses the platform's default per-session access control. Remote
access means proxying the socket yourself.

roxctl, the reference client, doesn't ship with releases; build it from
the repository with cargo build --release --package rox-cli. It has a
verb for each method above, and its raw command covers the rest:

    roxctl raw queue.move '{"id": 3, "after": 7}'


MCP
---

rox-mcp is in this folder. It's a stdio MCP server that proxies a running
rox, so an MCP client can ask what's playing, search the library, work the
playback/transport, and read the queue. Every tool is a straight proxy of
one socket method (see IPC above).

Two switches gate it, both off by default:

  1. "Enable AI Features" at the top of Settings > Application. Reveals
     the MCP and ML Models pages.
  2. "Enable MCP Server" on Settings > MCP.

The proxy checks both on every tool call, so a flip applies to the next
call without restarting rox or the client. A switched-off toggle, or a rox
that isn't running, comes back as a tool error naming the reason.

Settings > MCP shows a copy-ready snippet with the right path for your
machine, in the mcpServers shape most clients read:

    {
      "mcpServers": {
        "rox": { "command": "/path/to/rox-mcp" }
      }
    }

Claude Code takes the same thing as

    claude mcp add rox /path/to/rox-mcp

and in Zed it's a custom context server with that command. rox has to be
running: the proxy connects to the socket on the first tool call and
reconnects by itself when rox restarts.

Two flags cover a non-default socket:

  --data-dir <path>   Derive the socket for this data directory (a
                      --portable rox).
  --socket <path>     Name the socket outright.

The tools:

  now_playing      The playing track's tags, its position, and whether
                   audio is playing.
  transport        action: toggle, play, pause, next, prev, or stop.
                   Returns the resulting player state.
  search_library   query, optional limit (1..500). Matching tracks with
                   tags; pins like artist:name narrow one field.
  get_queue        The play order with each entry's stable id and the one
                   playing.

The socket does everything the tools do and more. Queue edits, seeking,
volume, artwork, and the debug scope are socket-only.


License
-------

AGPL-3.0. The LICENSE file is in this folder; the source is at
https://github.com/zealsprince/rox.
