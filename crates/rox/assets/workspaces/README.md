# Shipped workspaces

Each `.json` file here is a workspace bundle that ships in the app: a whole
look under one name, holding layout presets, the palette, and the appearance.
The file is the same JSON the settings Workspace page's export writes, and its
file name (without `.json`) names it when the bundle carries no name of its own.

To add one: set up the workspace in the app, open Settings, Workspace, export
it, then drop the file here and rebuild. Files that don't parse, or that come
from a newer bundle format, are skipped.

A `.png` named like the bundle's file with a theme suffix (`Foobar.json` ->
`Foobar_Dark.png` and `Foobar_Light.png`) becomes the preview picture on the
welcome window's quick-start tiles, the side picked by the app's live theme.
A plain `Foobar.png` serves both sides where a themed one is missing; a
bundle with no picture shows a placeholder there.
