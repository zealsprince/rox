# ADR 20: Persistence split by what a file is for, saved workspaces as files

**Status:** Decided

Decision: what the app persists outside the library database splits into five pieces in
the data directory, each holding one kind of thing.

- `settings.json` holds preferences and the library setup. The only file a person is
  meant to open, the only one worth carrying to another machine, and the one the
  settings window hands to the system editor.
- `workspace.json` holds the current look: a `LookState` with the live
  `WorkspaceBundle` plus the dock state being worked on top of it.
- `windows.json` records where this machine's windows are: the main frame and what each
  auxiliary window remembers.
- `session.json` holds what was playing and where the library stood.
- `accounts.json` holds the account connections and their keys.
- `workspaces/` holds the user's saved workspaces, one JSON file per bundle, named after
  the workspace. Shipped bundles stay compiled into the app from `assets/workspaces/`.

The single file had grown to where the preferences were a tenth of it, and everything in
it belonged to a different owner. Two saved workspaces, the live dock dump, and the
preset pool made up most of the bytes, so the file someone might open to check a library
path was 126k of dock dumps pretty-printed around it. But size was only the visible
half. The same file also held window geometry that means nothing on another machine, a
volume and a play position rewritten on every track, and a Last.fm session key sitting in
the file the app itself offers to open in a text editor.

Three properties fall out of splitting on what a file is *for* rather than on size.

**Sync.** Copying `settings.json` to another machine now carries the preferences and
nothing else. Window frames from a 4K desktop don't come along to a laptop, `last_scan`
doesn't tell the second machine its library is already current, and credentials stay
behind unless someone brings them.

**Churn.** `volume`, `last_track`, and `last_queue` move constantly while music plays;
`theme` and `library_roots` change a few times a year. `Settings::update` compares each
file's serialized form across the edit and writes only what moved, so a volume nudge
touches a 300-byte file instead of rewriting every dock dump beside it.

**Disposability.** `windows.json` and `session.json` can be deleted with no loss beyond
the obvious: windows reopen at their defaults and playback starts cold. Neither
`settings.json`, `accounts.json`, nor the workspaces can, which is a useful line to be
able to draw when telling someone how to clear a bad state.

Two fields ended up where the shape of the data wouldn't have put them. `last_scan`
describes this machine's disk rather than the library setup, so it's stored in
`session.json` and can't travel with a copied settings file. `queue_view` is a view, not
geometry, but it belongs to the queue window the widget opens, and the simple rule
(anything an auxiliary window remembers goes in `windows.json`) is worth more than the
exception.

Saved workspaces become files because a saved workspace and an exported one were already
the same bytes. Keeping them in an array inside settings meant export wrote a copy of
something the app was holding anyway, and import parsed a file back into that array. As
files, the folder is the collection: drop a shared bundle in and it's in the list, delete
one and it's gone, and the export dialog stays only for putting a copy somewhere else.
The list reads names off the filenames and parses a bundle only when one is applied,
which also takes the per-frame settings parse out of the workspace menu flyouts.

`Settings` stays one object in memory with the four states nested under it, so callers
still read and write through `Settings::update` and one lock still serializes the
read-modify-write. The writes aren't atomic across files: a crash partway leaves one an
edit behind the others, which costs a repaint's worth of drift and never a corrupt file,
and each file individually still goes through the temp-then-rename that kept a truncated
write from taking everything down.

Nesting the live look in the same `WorkspaceBundle` the saved files hold collapses the
two field-by-field transcriptions that used to run between them. Saving a workspace is
now a clone plus the live-dock fold, and applying one is an assignment. Those two
functions used to be the place a new appearance knob got forgotten; there's nothing left
to forget. It also gives the look a name it didn't have, recording which workspace it was
applied from, which is what a "modified" marker in the UI would read.

What stays out of the look is what stayed out of a bundle already: the theme pick, the
app font size, and the icon pack. Theme and font size are per-user choices a shared look
has no business moving, and an icon pack names a folder on one machine.

Migration reads every piece out of the pre-split file's flat map. Each state's fields
kept their names through the move, so three of the four deserialize straight out of it
with no field list to keep in sync; the look needs its own pass only because its
appearance knobs went from flat siblings to a nested object, and the three window fields
that were renamed have a serde alias for the name they had. The saved workspaces drain
to their own files once, leaving alone any name that already has a file, and telling apart
a replay from two names that fold to one filename so neither is dropped. The old file is
copied to `settings.json.bak-presplit` on the way through, and a migrated load force-writes
every file: the shards are all absent so they write themselves anyway, but `settings.json`
is already on disk in the old shape and a no-op edit serializes to the same bytes either
way, so without the force the stale flat keys would never be stripped.

There's no migration in the other direction. An older build reading the new
`settings.json` sees defaults for everything that moved out, which reads as a fresh look
and a cold playback state over an intact library. Recovery is copying the backup back.

Alternatives: split on size alone, moving only the look and the saved workspaces, which
shrinks the file but leaves geometry, credentials, and per-track churn sharing it. Keep
`Settings` flat in memory and partition a `serde_json::Map` on the way to disk, which
needs no callsite changes but leaves the transcription functions in place and makes the
split a property of the writer rather than of the type. Keep one file and write it
compactly, which buys bytes and none of the three properties above.

Splitting a file five ways sharpens what a parse failure costs, so the pieces inside each
one narrow it further. Serde is all or nothing by default: a preset whose dump went
missing fails the list, which fails the look, which resets `workspace.json` over one bad
entry. The collections that hold independent things (layout presets, the working copies,
the signal pool) drop the entries that don't parse and keep the rest, and the optional
fields that hold a remembered thing (each window's shape, the last track, the queue) read
as None on their own rather than failing the file around them.

Which of the two a field takes follows from the data, not from taste. A queue's `cursor`
indexes its `entries`, so dropping a bad entry shifts the cursor and resumes the wrong
track: that one fails whole or not at all. A list of presets has no such coupling, so it
drops the one and keeps the others. Both paths log what they dropped, since a preset or a
queue disappearing silently leaves no thread back to why.

Trade: five files can disagree by one edit after a crash, and a file that fails to parse
outright still resets to its defaults rather than falling back to a stale copy elsewhere.
Both are accepted for the same reason: everything disposable is rebuildable, and the two
files that aren't are the two that almost never get written.
