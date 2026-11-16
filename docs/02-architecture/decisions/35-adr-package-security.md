# ADR 35: One security model for every package and every way in

**Status:** Decided 2026-11-15

Proposal: the three rox packages, a workspace (`.roxw`, [ADR
31](31-adr-workspace-archives.md)), a plugin (`.roxp`, [ADR
32](32-adr-plugin-packages.md)) and an object (`.roxo`, [ADR 34](34-adr-objects.md)),
follow one set of rules, and so does every route a file can take into rox. The archive
checks live in one leaf crate and nowhere else, so a format can't grow a laxer copy of
them.

**What counts as code.** Two things a package can carry are code: shader source, gated
by [ADR 23](23-adr-shader-pipeline.md)'s approval, and a plugin's entry, gated by [ADR
30](30-adr-plugins.md)'s switch. Everything else is data, and data never gates. No
package can switch its own code on. Import, install and place store the thing, list what
it holds and ask. Approval stays where it is today, one gate and one habit, with no
second dialog added anywhere.

**One way in.** A file dialog, a drop on the window, a double-click, a second launch, a
control-socket verb and an install link all end in the same three paths, one each for
importing a workspace, installing a plugin and installing an object, and they show the
same confirms. No route applies a workspace, places an object or switches a plugin on
without the confirm the menu shows. The socket verbs store and list and stop there, so
[ADR 22](22-adr-control-surface.md)'s machine surface can bring a file in and can't
approve it.

**Archives.** A name in an archive is UTF-8, relative and forward-slashed, and every
component is a plain name. Absolute paths, `..`, backslashes, empty components, symlink
entries and duplicate names are refused. So are two names that differ only by case,
because Windows and a default macOS volume fold them onto one file. A component Windows
reserves (`CON`, `NUL`, `COM1` and the rest, or one ending in a dot or a space) is
refused on every platform, so a package made on Linux opens the same on Windows. Counts
and sizes are enforced on the bytes actually read, never on what a header states. An
archive whose unpacked bytes pass a fixed multiple of its own size is refused, which is
the bomb check for the two formats that have no hard size cap. Unpacking goes to a fresh
folder that's renamed into place, so a failed unpack leaves nothing behind. Only a
plugin keeps an executable bit. A workspace or an object extracts every file as plain
data.

A workspace holds its manifest and flat assets, and an object the same. Anything else in
either is ignored and never extracted, and an asset's extension has to match a kind rox
recognizes. Names under `_rox/` are reserved in all three formats for signatures: rox
never extracts them and never writes them, and a plugin folder holding a `_rox/`
directory fails to load. That folder would be outside the trust hash, readable by code
the user approved without it.

**Decoders read untrusted bytes.** Images decode off the UI thread and are refused past
a pixel count before anything is allocated. Fonts are capped in count and bytes, and one
that fails to load is skipped. Manifests are read with a byte cap. A decoder for video
or 3D models comes with its record on hostile input before rox adopts it.

**The network.** rox talks to one catalog origin, compiled in, over https ([ADR
36](36-adr-catalog.md)). A redirect off that origin is refused. A download is capped at
the size its index record states, and its sha256 is checked before the file moves into
place. Catalog text is shown as plain text, never rendered. A request carries nothing
that identifies the user or what they have installed.

**What the hash proves.** The index's sha256 proves a file is the one that was reviewed.
It's fetched from the same origin as the file, so it proves nothing once that origin is
compromised. rox already says as much about its model downloads, where the checksum is
compiled in because one downloaded beside its file isn't a boundary
(`rox-acoustic/src/models.rs:22-23`). The catalog can't compile its hashes in, since
packages are published after rox ships. The slot for closing the gap is a detached
signature over the index, checked against a key compiled into rox, and the name it would
use is reserved in the catalog's layout. It isn't part of this record.

**Exports leak nothing.** A workspace and an object pass through the same path scrub a
workspace export runs today (`rox-core/src/settings.rs:2976`) before they're written.

Alternatives: publisher signatures from the start, each author signing their own
packages. It proves who made a package, and it needs key management for every author, a
revocation story and an authority that vouches for keys. ADR 30 turned signing down for
plugin trust because it needs an authority rox has chosen not to be, and that holds. A
signature over the catalog's index is a different thing: one key, held by whoever
merges, proving where the index came from. It would approve nothing. The switch would
still be the only approval.

Signing the index now. It closes the compromised-origin case. It costs key custody, a
signing step on every publish and a rotation plan, all before there's a catalog worth
attacking. The reserved name means it can arrive later without changing any format.

Sandboxing plugin processes. ADR 30 decided this: a plugin runs as a subprocess with the
user's permissions and its own network, unsandboxed, and the enable card says so. WASM
was weighed there and lost. A package changes how a plugin arrives and nothing about how
it runs, so this record leaves that call where it is.

**Amended 2026-11-16: a place with nothing to review doesn't ask.** Andrew's product
call. "Import, install and place store the thing, list what it holds and ask" holds for
import and install as written. Placing an object asks only when there is something to
review: a shader that isn't approved, a plugin it needs that isn't running, a size past
the warning threshold, or a weight warning. An object with none of those is data and
nothing else, and it is placed at once. Undo takes it back out. A confirm with nothing
to review teaches people to click through the one that has something.

Costs. Review is the only thing standing between a hostile plugin and a user who
approves it. The enable card shows what a plugin declares, and nothing in rox checks
what its code does. A compromised catalog origin isn't defended against while the index
is unsigned. The name rules refuse files that would be legal on the platform that made
them: a Linux author can't ship two files that differ only by case, or one named `aux`.
