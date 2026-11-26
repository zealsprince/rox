# Shipped workspaces

Each `.json` file here is a workspace bundle that ships in the app: a whole
look under one name, holding layout presets, the palette, and the appearance.
It is the `workspace.json` inside a `.roxw` archive, and its file name (without
`.json`) names it when the bundle carries no name of its own.

A bundle whose asset table lists files keeps them in a folder named like its
file: `Critters.json` reads its images from `Critters/assets/`. Each file there
has to match the size and SHA-256 its table entry gives, or applying the look
fails.

To add one: set up the workspace in the app, open Settings, Workspace, export
it, then unpack the `.roxw`. Its `workspace.json` goes here under the
workspace's name, and its `assets/` folder goes beside it as `<Name>/assets/`.
Rebuild. Files that don't parse, or that come from a newer bundle format, are
skipped.

A look can also be written by a script. `Aero.json` comes from
`scripts/workspaces/aero_workspace.py`, which lays its one canvas out from a
compact description and fills the asset table from the files on disk, and
every image in `Aero/assets/` is drawn by `scripts/workspaces/aero_assets.py`
with nothing but the Python standard library. The asset script also deletes
any picture in that folder it no longer draws, and the workspace script
imports it for the glass panes' nine-slice insets. Edit the scripts rather
than the JSON or the PNGs, then run the asset script before the workspace one:

    python3 scripts/workspaces/aero_assets.py
    python3 scripts/workspaces/aero_workspace.py

Aero's previews are screenshots of the look applied in a 1440x900 window with
a track playing, one in each theme.

A `.png` named like the bundle's file with a theme suffix (`Foobar.json` ->
`Foobar_Dark.png` and `Foobar_Light.png`) becomes the preview picture on the
welcome window's quick-start tiles, the side picked by the app's live theme.
A plain `Foobar.png` serves both sides where a themed one is missing; a
bundle with no picture shows a placeholder there.
