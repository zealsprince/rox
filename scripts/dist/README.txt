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
       "result": {"name": "rox", "version": "1.26.1", "protocol": 1}}

The protocol number moves only on breaking changes. New methods and new
response fields arrive without a bump, so ignore fields you don't know.

A connection can also ask to be pushed to. subscribe (no params)
answers {"subscribed": true}, and from then on the server writes event
frames onto that connection whenever something moves. An event frame is
a JSON-RPC notification, method and params without an id, and it lands
between responses, so a reader has to route by id rather than assume
the next line answers its last call:

  event.track      The track's full tags when playback turns over; null
                   when it stops.
  event.playback   The player status when play state, volume, mute, or
                   the A-B section changes.
  event.queue      {"queue_rev": n} when the queue changes; fetch
                   queue.list for the contents.

A subscriber that stops reading is disconnected instead of buffered,
since a consumer that fell behind holds a stale picture anyway.
Reconnect and subscribe again for a fresh one.

Methods. Transport verbs return the full player status, so a caller sees
what its command did without a second round trip:

  transport.status       Playing, position, duration, volume, mute, A-B
                         section, queue revision, current track.
  transport.toggle       Also .play .pause .next .prev .stop; all return
                         status.
  transport.seek         {"to": secs} or {"by": secs}
  transport.set_volume   {"volume": 0..2}
  transport.ab           {"action": "mark"/"clear"} steps or clears the
                         A-B repeat; {"a": secs, "b": secs} sets a
                         section outright.
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
  library.rescan         Scan the library folders again; {"started": true},
                         or an error while busy or without folders.
  tasks.status           The analysis passes (acoustic, ReplayGain, tempo,
                         sort names, romanize): switch state, tracks to
                         do, progress while one runs. plugin_jobs lists
                         the jobs plugin actions are running.
  tasks.start            {"pass": "acoustic"/"replaygain"/"tempo"/
                         "sortnames"/"romanize"}; answers with count,
                         workers, estimate, and save mode.
  tasks.stop             {"pass": ..}; the pass stops at the next file,
                         keeping what's done. {"job": n} stops a plugin
                         action's job instead.
  plugins.list           The running plugins, each with its source and the
                         actions it declares.
  plugins.browse         {"source", "node"?, "view"?, "cursor"?}; one page
                         of a plugin's catalog, its roots when there's no
                         node.
  plugins.search         {"source", "query", "view"?, "cursor"?}; the same
                         page shape. The query takes the criteria the
                         External Sources search box does.
  plugins.action         {"source", "action", "items"?, "params"?}; runs a
                         declared action as its menu item would, and
                         answers its message or a job number.
  ai.status              {"enabled", "mcp", "plugins"}, the toggles
                         rox-mcp checks.
  debug.*                Diagnostics for working on rox itself; the
                         repository documents them.

Queue entry ids are stable handles: queue.list returns them, and remove,
move, and jump name entries by them, so an edit can't hit the wrong row
when the queue shifts underneath it. queue.add takes files and folders,
filters to decodable audio, and accepts path#N for a cue sheet's Nth
track, the same spelling the m3u export uses. A plugin's or a server's
track goes in by the source|path key library.search gives it, and only
once it's in the library. mode places the batch: end behind what's queued,
next right after the playing track, now splices and plays.

Search uses the panels' query language: free terms match title, artist,
album, and genre, while a field: prefix pins one, as in artist:name or
year:1990. The fields are title, artist, albumartist, album, genre,
year, folder, codec, rating, plays, and added. limit defaults to 50 and
caps at 500.

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

The plugins.* methods make the same calls the External Sources panel and
its menus make, so a plugin can't tell a socket client was behind one.
They answer while plugins are on, whatever the MCP switches say.

The surface is local, never a network port. Auth is filesystem
permissions: the Unix socket is created user-only (0600), and the Windows
pipe uses the platform's default per-session access control. Remote
access means proxying the socket yourself.

roxctl, the reference client, doesn't ship with releases; build it from
the repository with cargo build --release --package rox-cli. It has a
verb for each method above, watch follows the event stream, and its raw
command covers the rest:

    roxctl raw queue.move '{"id": 3, "after": 7}'


MCP
---

rox-mcp is in this folder. It's a stdio MCP server that proxies a running
rox, so an MCP client can ask what's playing, search the library, work the
playback/transport, read and add to the queue, and kick off library scans
and the long analysis passes. With a third switch it can also browse
plugins and run their actions. Every tool is a straight proxy of one
socket method (see IPC above).

Two switches gate it, both off by default:

  1. "Enable AI Features" at the top of Settings > Application. Reveals
     the MCP and ML Models pages.
  2. "Enable MCP Server" on Settings > MCP.

A third switch, "Let MCP Clients Use Plugins" on Settings > MCP, also off
by default, lets clients browse and search plugins and run the actions
they declare. What a plugin answers reaches the model as it is, and an
action acts on the plugin's service the way picking it from a menu would.
Each action a client runs shows a toast.

The proxy checks the switches on every tool call, so a flip applies to the
next call without restarting rox or the client. A switched-off toggle, or
a rox that isn't running, comes back as a tool error naming the reason.

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
  ab_repeat        action: mark, clear, or set (with a and b in track
                   seconds). Marks, clears, or sets the A-B section.
  search_library   query, optional limit (1..500). Matching tracks with
                   tags; pins like artist:name narrow one field.
  get_queue        The play order with each entry's stable id and the one
                   playing.
  add_to_queue     items, optional mode: end, next, or now. Files,
                   folders, station stream URLs, or the source|path keys
                   search_library gives, placed as queue.add places them.
  rescan_library   Starts a background rescan of the library folders.
  get_tasks        The analysis passes (acoustic, ReplayGain, tempo, sort
                   names, romanize): switch state, tracks to do, progress
                   while one runs. With plugins allowed, also the jobs
                   plugin actions are running.
  start_task       pass: acoustic, replaygain, tempo, sortnames, or
                   romanize. Starts the pass; answers with count,
                   workers, estimate, and save mode.
  stop_task        pass: acoustic, replaygain, tempo, sortnames, or
                   romanize. Stops the pass at the next file, keeping
                   what's done. Or job, a plugin action's job number from
                   get_tasks.
  plugins          The running plugins, each with the source the other
                   plugin tools take and the actions it declares.
  plugin_browse    source, optional node, view, and cursor. A place in a
                   plugin's catalog: its roots, or a node's contents.
  plugin_search    source and query, optional view and cursor. Answers in
                   plugin_browse's shape. The query takes artist:, title:
                   and album: terms and "quoted phrases", which every
                   result has to match.
  plugin_action    source and action, optional items and params. Runs the
                   action as its menu item would; a long one answers with
                   a job number for get_tasks and stop_task.

The socket does everything the tools do and more. Queue edits, seeking,
volume, artwork, and the event stream are socket-only.


Plugins
-------

A plugin brings a source from outside rox into your library: rox browses,
searches, syncs and plays it through a program the plugin supplies. rox
doesn't ship plugins for any service.

To add one:

  1. Turn on Enable Plugins at the top of Settings > Plugins.
  2. Press Reveal Folder on the Plugins page. It opens the plugins folder,
     creating it the first time. In portable mode that's rox-data/plugins
     beside the executable.
  3. Drop the plugin's folder in. The folder's name has to be the plugin's
     id.
  4. Switch it on. The card that opens says what the plugin declares and
     which programs it runs, and asks you to confirm.

A plugin runs as a program on this computer with your permissions. rox
doesn't sandbox it. Switching it on approves exactly the files in its
folder. If any of them change, it switches off until you switch it on
again. The card then shows what changed.

Browse and search a plugin's source by picking it under Add Panel >
Plugins, which opens the External Sources panel on it. Hover a
collection and click its checkmark to keep it in your library, which
syncs it and follows its changes. Playing or queueing a track plays it
without adding it; the checkmark on a hovered track, or Add to Library
in its menu, keeps it. A check stays lit while its collection or track
is in the library. Library, beside the panel's search box, lists what
the plugin has put there: its kept collections and the tracks added one
at a time. Remove on the Plugins page drops the plugin's tracks, synced
collections and settings, and leaves its folder where it is.

The panel's search box takes artist:, title: and album: terms and
"quoted phrases", and every result has to match them. That finds a song
the service would rank pages under famous ones sharing a word. Quote a
term with spaces, as in artist:"Bright White Lightning". Plain words only
steer the service's own search.

A plugin track's menu in the library, a playlist, the queue or history
takes it back out the way it came in. A track added on its own gets
Remove from Library. One that a kept collection holds gets Stop Keeping,
which lets go of the whole collection, since the next sync would put a
single track back.

A track you played or queued without keeping it stays out of the library's
views and search, and keeps its place in the queue, playlists and history.
One that isn't played again for 30 days, and isn't in the saved queue, is
forgotten when rox starts. Its playlist entries and plays come back with
it if it returns. Switching a plugin off hides its tracks without deleting
them, and so does deleting its folder, which is how many plugins update.
Only Remove deletes them.

The chevron beside a switched-on plugin on the Plugins page unfolds its
details. They count its tracks in the library and those added one at a
time, and Show in Library narrows the library search to them. Kept
collections sync each time the plugin starts, and Sync Now syncs them
again. A plugin that asks to scrobble gets a Scrobble Plays switch there,
on once you approve it. A plugin that offers lyrics gets a Lyrics switch,
off until you turn it on. With it on, the Lyrics panel asks the plugin for
its own tracks' lyrics before it asks the lyrics providers.

Sync Favourites on the Plugins page lets the heart reach a plugin's
service, for plugins that offer it. Hearting one of its tracks favourites
it there too, and taking the heart back removes it there. Hearts from
before you switch it on aren't sent. While it's on, the heart on what's
playing, in the transport, the track info and custom controls, shows half
filled when it's a favourite on one side only, and clicking it makes it a
favourite on both.

History and the Biography panel's top tracks list songs your library may
not have. Double-click one to have a plugin search for it and play the
match, and pick the plugin under Play From in its menu. Nothing plays when
the plugin has no match for that very song, so a cover or a karaoke take
never stands in for it.

What the panel offers depends on the plugin:

  - Right-click an album, playlist or artist to Play it, Play Next or
    Add to Queue. Inside one, the same buttons sit over its tracks.
  - Play Similar on an album, playlist or artist plays its tracks, then
    keeps the queue going with the service's picks. On a track it plays
    only the picks, leaving the track itself out. It turns Similar
    shuffle on, and an album or playlist you play goes on the same way
    when it ends. A radio carries on after a restart, and stops when
    continuation is Off in the playback settings.
  - Go to in a track's right-click menu opens its album or artist in the
    External Sources panel, wherever the track shows.
  - A long track can mark its parts, like an episode's segments, along
    the top of the Seek panel. Hover a mark for its name, and click it
    to jump there.
  - Chips over a list switch between the plugin's views, like search
    narrowed to albums. A column heading, like a popularity, sorts by
    it.
  - Covers can come as shelves you scroll sideways: shift and the wheel,
    or a sideways swipe.
  - Open in Browser and Copy Link take you to an item's page on the
    service, wherever its tracks show in rox.
  - Actions the plugin adds, like a download, sit in the right-click
    menu of its tracks wherever they show, of its collections in the
    panel, or of the panel itself. One with settings opens a dialog
    first. A notice says when it's done, with Open Link or Show in
    Folder when the plugin hands back a page or a file. Longer work
    shows in the Tasks window, where Stop cancels it. An action that
    depends on a track's state, like adding to or removing from the
    service's favourites, only shows where it applies.

While one of the plugin's tracks plays, the panel's top level shows it
and what's next. Click the plugin's name or logo at the top of the panel
to get back there. Beside Now Playing, a link names where you played the
track from, and clicking it opens that list with the track picked out.
When the playing track is in the list you're looking at, Jump to Playing
in the panel's menu scrolls to it.

The Plugin Guide button on the Plugins page opens the guide to writing a
plugin, with the protocol and an example to copy. It's also at
https://github.com/zealsprince/rox/blob/main/README_PLUGINS.md


License
-------

AGPL-3.0. The LICENSE file is in this folder; the source is at
https://github.com/zealsprince/rox.
