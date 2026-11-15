# ADR 32: Plugins ship as packages, folders stay for authoring

**Status:** Proposed

Proposal: a plugin is shared and installed as a `.roxp` file, a zip with `plugin.json`
at its root and everything else the plugin folder holds around it. Install takes a
package, copies it into the plugins directory and lists it. A plain folder in the
plugins directory still loads, for authors working on one and for every install made
before packages existed. A plugin's trust identity stays [ADR 30](30-adr-plugins.md)'s
content hash, computed so a folder and the package made from it hash the same.

rox unpacks a package into a folder of its own before it runs it, because a subprocess
needs a real directory. The host starts a plugin with its folder as the working
directory and `<folder>/bin` on its PATH (`rox-plugins/src/process.rs:35-52`), and the
entry is a file path inside the folder (`rox-plugins/src/manifest.rs:465-481`). None of
that works against a zip. The unpacked copy is kept in a cache under the package's hash,
so a new version unpacks beside the old one. A running native plugin's files are never
replaced in place, which Windows wouldn't allow anyway while the plugin holds them. The
copy is rox's own: it's rehashed when plugins are scanned, a copy that no longer matches
its package is unpacked again, and stale copies are swept.

The hash is the folder hash run over the archive's entries: every file's relative path
and contents in sorted order, the same framing and the same skipped names
(`rox-plugins/src/hash.rs`). So packing a folder and installing the result never asks
for approval again, and an update is a new hash that takes the Changed and re-approval
path ADR 30 already has. No version compare is added. The manifest is unchanged and
`api` stays 1: `Manifest` rejects unknown keys, and a 1.x folder has to stay loadable.

`plugin.json` is at the root of a package. A package whose files are wrapped in one
top-level folder is refused, with a message that says so. When a folder and a package
share an id, the folder wins, so an author's working copy shadows the release they
installed. A second package with an id already taken fails to load and names the file
that holds it.

Nothing about the gate changes. Installing lists a plugin and never switches it on. The
user's switch is still the approving act, the enable card still says the plugin runs as
a program with the user's permissions, and nothing runs before that switch.

Alternatives: packages only, with every 1.x folder converted on first launch. One shape
is simpler for the loader, and it breaks the authoring loop. An author edits files in a
folder and saves, and Developer mode (ADR 30's amendment) approves those saves while the
manifest's diff is empty. With packages only, every save would be a repack.

Hashing the archive's bytes as they sit on disk is the cheaper identity, one digest over
one file. It would make packing a folder a new identity, so an author's own approval
wouldn't carry over to the package they ship, and two zips of the same files could hash
differently on compression level or timestamps alone. Hashing what's inside ties the
identity to what runs.

Running straight out of the archive, with no unpacked copy, was ruled out by the spawn
contract above.

Costs. Every installed package is on disk twice, once as the `.roxp` and once unpacked.
Installing is now an action in rox where it used to be a folder someone dropped in,
though dropping a folder still works. Remove deletes a package and its unpacked copies,
and a folder stays on disk as it does today.
