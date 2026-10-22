# Lyrics Folder

An example rox provider plugin, written to be copied. It has no tracks of its own.
Instead it answers lyrics for any track in your library from a folder of `.lrc` files,
so a sheet you keep yourself, word-timed or not, can reach the Lyrics panel. It needs no
network, no account and nothing beyond Python 3's standard library.

The protocol it speaks is in the [plugin guide](../../../README_PLUGINS.md), under
`provider.lyrics`.

## Trying it

1. Turn on Enable Plugins at the top of Settings > Plugins.
2. Press Reveal Folder on the Plugins page and copy this `lyrics-folder` folder into it.
3. Switch Lyrics Folder on and confirm the card.
4. Under Lyrics Folder on the Plugins page, set Folder to the full path of a folder of
   `.lrc` files, and switch Lyrics on.
5. Play a track that has a sheet in the folder. With automatic lookups on, the Lyrics
   panel can save a match by tags on its own. A match by file name scores under that
   bar, so it waits in Find Lyrics Online, listed under Lyrics Folder.

It doesn't show under Add Panel: it's no source, so there's nothing to browse.

Python 3 has to be on PATH as `python3` or `python`, or as `py` on Windows.

## How it matches

A file is a match when its artist and title equal the track's, compared the way rox
compares names: lowercase, with every run of spaces and punctuation as one space. The
artist and title come from the file's `[ar:]` and `[ti:]` tags when it has both, and
from its name otherwise, split at the first ` - `, so `Some Artist - Some Song.lrc`
matches with no tags at all. A match by tags tells rox it's sure (`confidence` 1.0). A
match by name leaves the ranking to rox's own comparison.

Subfolders are searched too. A link that points out of the folder is skipped, and the
plugin writes nothing anywhere.

## Talking to it without rox

The plugin reads one JSON-RPC request per line on stdin and answers on stdout, so a
shell can drive it:

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"hello","params":{"api":1,"config":{"folder":"/home/me/lyrics"},"data_dir":"/tmp/lyrics-folder","platform":"linux-x86_64"}}' \
  '{"jsonrpc":"2.0","id":2,"method":"provider.lyrics","params":{"artist":"Some Artist","title":"Some Song","album":"","duration_secs":null}}' \
  | python3 -B lyrics_folder.py
```

`-B` keeps Python from writing `__pycache__` into the folder. rox sets the same thing
for every plugin, since any new file in the folder changes its hash and switches it off.

## Making it yours

- Turn on Developer mode, the terminal button beside the plugin's switch, while you
  work on it. Otherwise every save switches the plugin off until you approve it again.
- Change `id` in `plugin.json` and rename the folder to match.
- Declare only the domains you answer. Each one gets its own switch on the Plugins page,
  and rox never asks a plugin about a domain it didn't declare.
- Send `confidence` only when you have a reason to be sure. rox ranks by its own
  comparison and blends yours in, so a confident wrong answer still ranks low.
- Answer at most 50 candidates, and null or an empty list when you have none.
- Keep exiting when stdin closes. It's the one signal that reaches a plugin on every OS
  when rox goes away without a shutdown.
- Read and write only what the user pointed you at, and the `data_dir` that `hello`
  hands you.
