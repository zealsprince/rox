# TODO

## Panels

- A customize window for the library panel: every other panel type edits its
  config through the shared customize rows, while the library's (query,
  column widths) is only editable in the panel itself.
- Bring the generative visualizer back once it is a real GPU shader, zero CPU
  rasterization. ADR 8 still describes the removed flow field and should be
  rewritten around that decision.

## Library

- A true settings window; the first tenant is library management - which
  folders get scanned. That means multi-root scanning; Open Folder is
  single-root today.
- Playlists: schema, membership and ordering, display - likely a playlist
  panel plus library schema work. Track identity is already source-qualified,
  so playlist entries should reference track ids from the start.
- Scanner sync gaps: rows for files deleted from disk are never removed, and
  there is no filesystem watching. Both belong to the `watch` half of the
  library contract, which is still design only.

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

- Write 05-visualizer.md and 06-panels.md; their subjects are built now (the
  peaks cache format, PCM tap and FFT wiring; the layout dump, panel config
  model, customize windows, popout mechanics).
