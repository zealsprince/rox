# TODO

## Panels

- New panels dock to the bottom by default instead of joining the center tabs.
  `DockArea::add_panel` already takes `DockPlacement::Bottom`; note docks clamp
  panels to a 100px minimum height, which suits the audio views fine.
- Per-panel settings menu over a generic panel data system (styling,
  configuration). Design the schema together with layout persistence below:
  gpui-component restores layouts through a `PanelRegistry` that deserializes
  each panel's state, so one serializable config per panel type serves both.
- Show tabs only when a group holds two or more panels; a single panel gets a
  right-click context menu (pop-out, duplicate, close) instead of header
  buttons. gpui-component has no hook to suppress the tab bar, so this is the
  item that decides vendoring the dock (the escape hatch ADR 7 reserved)
  versus patching upstream. Two papercuts land in the same decision:
  middle-click close only hits the title label rather than the whole tab, and
  a private zoomed flag makes the zoom menu label lag one click after popping
  out a zoomed panel.
- Break the transport bar into composable panels: playback controls, volume,
  and a seek strip (the waveform minus peaks - the position and seek plumbing
  already exists on `Player`). Together these unlock real layout construction.
  The constraint to design around: the transport bar's render pass is the
  single pump draining the PCM tap into the `AudioFeed`, and that works
  because the bar renders unconditionally. Once controls become closable
  panels, the pump has to move to something that always renders - the
  workspace root, or a headless frame driver.
- Icons for buttons and menus. gpui-component-assets is already bundled with
  the widget icon set; check its `IconName` coverage before drawing our own.
- Bring the generative visualizer back once it is a real GPU shader, zero CPU
  rasterization. ADR 8 still describes the removed flow field and should be
  rewritten around that decision.

## Persistence

- Save window and panel layout, restore on open. `DockArea::dump()/load()`
  plus the panel registry; window bounds belong to the same pass.
- Persist playback state: volume, loop mode. One config-dir story next to the
  library DB (`dirs` is already a dependency).
- Waveform peak cache on disk keyed by file identity, with a generating
  animation while peaks compute so the panel never sits blank. The
  implementation doc set already reserves a waveform cache file format.

## Library

- A true settings window; the first tenant is library management - which
  folders get scanned. That means multi-root scanning; Open Folder is
  single-root today.
- Playlists: schema, membership and ordering, display - likely a playlist
  panel plus library schema work. Track identity is already source-qualified,
  so playlist entries should reference track ids from the start.
- Scanner sync gaps: rows for files deleted from disk are never removed, and
  there is no filesystem watching. Both belong to the sync machinery the
  implementation docs list as unbuilt.

## Playback

- Keyboard shortcuts and media keys: space for play/pause, seek arrows, and
  platform media keys if gpui exposes them (check what 0.2.2 offers before
  designing around it).

## Docs

- Contract drift to resolve one way or the other: ADR 6 says search sits
  behind a debounce but the code searches per keystroke, the components doc
  says the UI never touches SQLite but `LibraryPanel` holds a connection for
  id-to-path resolution, and the `browse`/`watch` API exists only on paper.
