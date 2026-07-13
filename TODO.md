# TODO

## Panels

- Per-panel settings menu over the panel config schema layout persistence
  laid down (`LibraryConfig` and the dump/registry round-trip): each panel
  type's config struct is what the menu edits.
- Icons for buttons and menus. gpui-component-assets is already bundled with
  the widget icon set; check its `IconName` coverage before drawing our own.
- Bring the generative visualizer back once it is a real GPU shader, zero CPU
  rasterization. ADR 8 still describes the removed flow field and should be
  rewritten around that decision.

## Persistence

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
