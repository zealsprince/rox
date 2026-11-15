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

A `.png` named like the bundle's file with a theme suffix (`Foobar.json` ->
`Foobar_Dark.png` and `Foobar_Light.png`) becomes the preview picture on the
welcome window's quick-start tiles, the side picked by the app's live theme.
A plain `Foobar.png` serves both sides where a themed one is missing; a
bundle with no picture shows a placeholder there.
