# ADR 31: Workspaces are archives, assets are files

**Status:** Decided 2026-11-15

Proposal: a saved or shared workspace is a `.roxw` file, a zip holding `workspace.json`
and a flat `assets/` folder. `workspace.json` is the `WorkspaceBundle` at format version
2, and the bundle names every file it carries in an asset table: the id a config refers
to, the file's name under `assets/`, its kind, its size and its sha256. The live look
keeps its bundle in the data directory's `workspace.json`, as [ADR
20](20-adr-settings-split.md) has it, and its files in an `assets/` folder beside it. An
asset is one of four kinds: an image (still or animated), a font, a video or a 3D model.
Configs name assets by id and never hold their bytes.

Today the one encoded payload a workspace carries is `ShaderAsset.data`, a base64 image
on a pool shader (`rox-core/src/settings.rs:1414-1441`), the shape [ADR
23](23-adr-shader-pipeline.md) chose. It costs more than its size suggests. The live
shard holds a full copy of the applied bundle, images included (`LookState`,
`settings.rs:2912-2930`), and `Settings::update` reloads and reserializes every shard on
each edit to find the ones that moved (`settings.rs:3674-3686`). So a slot nudge on any
panel re-reads and re-serializes every image the look holds. The pool watch also
re-encodes an image to base64 each time its file is saved (`pool_reload`,
`rox-panel-api/src/panel/shader.rs`).

Three things argue for files. Base64 adds a third to every byte it encodes, and the
bundle pays that on disk, in memory and on every reserialize. For video and models the
cost rules JSON out: a JSON string has to be read whole and decoded before anything can
use it, and a clip or a mesh runs to megabytes where a 1-bit plate runs to a few hundred
bytes. And a file in an archive is something another tool can open. An author can pull a
font out of a look with any unzip tool, or swap an image with an image editor, without
rox in the loop.

A zip keeps the property that made saved workspaces files in the first place: a saved
workspace and an exported one are the same bytes (ADR 20). Saving packs the live bundle
with the files its table names. Applying one replaces the live `assets/` folder
wholesale, the way applying already replaces the bundle. A workspace is still one file
to share, drop or double-click.

The table bounds what comes out of an archive. A file the table doesn't name is never
extracted, and a named file stops at its stated size and has to match its hash. That
makes the table the zip-bomb guard, so a `.roxw` keeps ADR 23's soft limit: a large one
warns in its import confirm and still imports. ADR 23's other call carries over
unchanged. Assets never gate, because approval is over code and an image, a font or a
mesh can't run anything. [ADR 35](35-adr-package-security.md) holds the archive rules
that apply to every rox package.

Shipped workspaces stay JSON under the app crate's `assets/workspaces/`, with a sibling
folder for the ones that carry files, because a zip in git can't be diffed or reviewed.

The format moves one way. rox reads a version 1 bundle and writes only version 2.
Upgrading drains every `ShaderAsset.data` into a file and an asset entry, and keeps what
it replaced: each saved `workspaces/<name>.json` moves to `workspaces/v1-backup/`, and
the live shard is copied to `workspace.json.bak-v1`, the way ADR 20's split kept
`settings.json.bak-presplit`. A version 1 `.json` export from someone else still imports
and is upgraded on the way in. Going back to 1.x means restoring those copies.

Alternatives: keep JSON and inline only small assets, with large ones in a folder beside
the file. Small plates would stay where they are today, at the price of two shapes for
one thing and a threshold someone has to pick. The large files still need a carrier to
be shared in, which is an archive anyway, and every small image still pays the
reserialize on each edit.

A folder per workspace with no archive is readable in any file manager and diffs in git.
It can't be shared as it stands: sending one means zipping it, and a folder can't be
double-clicked, opened as a document, or downloaded and checked against one hash.

A content-addressed store shared by every workspace, files keyed by their hash, would
save disk when two looks use the same image. It turns deleting a workspace into garbage
collection across all of them, and export still has to gather files into something to
send. Within one look the duplication doesn't arise, since adding bytes the live store
already holds reuses that entry. Two saved workspaces that use one image each keep a
copy.

Costs. A saved workspace is no longer a text file. [ADR 22](22-adr-control-surface.md)'s
edit-on-disk loop is built on one: the watch on `workspaces/` re-applies the active
workspace when its JSON changes (`rox/src/workspaces.rs:742`), and `$schema` gives the
file's editor validation. A `.roxw` keeps `$schema` inside its `workspace.json`, pointing
at the schema as published at the release that wrote it, and no text editor opens it in
place. The loop moves to the live `workspace.json`, which stays text: ADR 22's
amendment holds the detail.

Export can now be large. A look carrying video runs to megabytes where today's run to
kilobytes. The confirm shows the unpacked size past a threshold, and nothing refuses it.
The data directory gains a folder that has to stay in step with the bundle that names
it, and a file the table no longer names is swept rather than kept.

A global preset detached from a pool shader keeps that shader's source and loses its
images (`rox-core/src/settings/panel_presets.rs`, `detach`). That gap exists today, and
this record gives presets no asset home, so it stands.

**Amended 2026-11-15: two kinds, image and font.** Andrew's product call, with [ADR
33](33-adr-canvas-panel.md)'s amendment. An asset is an image or a font. The video and 3D
model kinds are withdrawn before any release wrote one, since nothing in rox will play a
video or draw a model.

A workspace carries no video and no model file. rox doesn't import one, store it, list it
or export it. An archive's table entry for an `mp4`, `webm` or `glb` file gets what an
entry for any kind rox doesn't read gets. A renderer that plays a clip brings its own
file or is pointed at one by the user ([ADR 37](37-adr-render-targets.md), Proposed).
Export size, the cost above, now comes from images and fonts alone.
