# Shipped workspaces

Each `.json` file here is a workspace bundle that ships in the app: a whole
look under one name, holding layout presets, the palette, and the appearance.
The file is the same JSON the settings Workspace page's export writes, and its
file name (without `.json`) names it when the bundle carries no name of its own.

To add one: set up the workspace in the app, open Settings, Workspace, export
it, then drop the file here and rebuild. Files that don't parse, or that come
from a newer bundle format, are skipped.
