//! The actions panels dispatch and bind against. Keymap registration and
//! handlers stay in the app; only the types live here.

use gpui::actions;

actions!(
    rox,
    [
        TogglePlayback,
        SeekBackward,
        SeekForward,
        /// Step a panel's live type-ahead phrase to its next match.
        TypeAheadNext,
        TypeAheadPrev
    ]
);

// Named under canvas, where they were first bound: `roxctl action` and
// scripts call them by that name. The queue and playlists answer them too.
actions!(
    canvas,
    [
        /// Undo the focused panel's last edit.
        Undo,
        Redo
    ]
);

/// The key contexts the undo chords reach besides a canvas. A panel with
/// an undo stack sets one on its focused root.
pub const QUEUE_UNDO_CONTEXT: &str = "QueueUndo";
pub const PLAYLISTS_UNDO_CONTEXT: &str = "PlaylistsUndo";

actions!(
    lyrics,
    [
        /// Stamp the cursor's line with the playback position and step on.
        StampLine
    ]
);

/// The playback bindings' scope as a plain context, for key tooltips. A
/// lookup parses its argument as a context, and the binding's own scope
/// (which excludes the search box) isn't one, so passing it finds nothing.
pub const PLAYBACK_TIP_SCOPE: Option<&'static str> = Some("Workspace");
