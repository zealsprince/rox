# TODO

## Panels

- Per-panel settings menu over a generic panel data system (styling,
  configuration). Design the schema together with layout persistence below:
  gpui-component restores layouts through a `PanelRegistry` that deserializes
  each panel's state, so one serializable config per panel type serves both.
- Show tabs only when a group holds two or more panels; a single panel gets a
  right-click context menu (pop-out, duplicate, close) instead of header
  buttons. The dock is vendored now (rox-dock, ADR 7 amendment), so this
  lands directly in our TabPanel render path. Two papercuts live in the same
  code: middle-click close only hits the title label rather than the whole
  tab (our workaround in panel.rs, should move into the vendored tab element
  and cover the whole tab), and the private zoomed flag makes the zoom menu
  label lag one click after popping out a zoomed panel.
- Icons for buttons and menus. gpui-component-assets is already bundled with
  the widget icon set; check its `IconName` coverage before drawing our own.
- Bring the generative visualizer back once it is a real GPU shader, zero CPU
  rasterization. ADR 8 still describes the removed flow field and should be
  rewritten around that decision.

## Persistence

- Save window and panel layout, restore on open. `DockArea::dump()/load()`
  plus the panel registry; window bounds belong to the same pass. The
  settings file (`settings.rs`, next to the library DB) is where the layout
  should land.
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

- Media keys: gpui 0.2.2 offers nothing - the Linux backend maps no XF86
  audio keysyms and there is no macOS media-key hook. Needs upstream gpui
  work or platform integration (MPRIS on Linux).
- Keyboard shortcuts don't reach popped-out panel windows: the bindings are
  scoped to the Workspace key context and the action handlers live on the
  workspace root, and a popout hosts neither. Fixing it means giving the
  popout host the app state and the same context plus handlers.
- Restore-from-ended: with loop off, once the queue finishes the engine
  drops its source, and switching loop mode on afterwards doesn't restart
  playback until next/prev.
- Closing the application should store the last played file. We should make
  sure to properly store the track ID in the library from last run.

## Docs

- Contract drift to resolve one way or the other: the components doc says
  the UI never touches SQLite but `LibraryPanel` holds a connection for
  id-to-path resolution, and the `browse`/`watch` API exists only on paper.
