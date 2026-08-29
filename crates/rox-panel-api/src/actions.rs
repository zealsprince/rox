//! The actions panels dispatch and bind against. The keymap registration
//! and every handler stay up in the app; only the types are here, so a
//! panel's `on_action` and the workspace's binding name the same one.

use gpui::{actions, App, KeyBinding};

actions!(
    rox,
    [
        /// Play or pause the workspace's player, bound to space.
        TogglePlayback,
        /// Nudge the playing track back, bound to the left arrow.
        SeekBackward,
        /// Nudge the playing track forward, bound to the right arrow.
        SeekForward,
        /// Step a panel's live type-ahead phrase to its next match,
        /// bound to tab while a phrase is up.
        TypeAheadNext,
        /// The same backwards, bound to shift-tab.
        TypeAheadPrev
    ]
);

/// The type-ahead cycle bindings; call once at startup. Scoped to the
/// cycle context, which a panel carries for as long as it holds a phrase:
/// gpui-component's Root binds bare tab to focus traversal, and the
/// deeper context match wins while a phrase is up, handing tab back to
/// traversal once the phrase is dropped.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            "tab",
            TypeAheadNext,
            Some(rox_panel_kit::TYPE_AHEAD_CYCLE_CONTEXT),
        ),
        KeyBinding::new(
            "shift-tab",
            TypeAheadPrev,
            Some(rox_panel_kit::TYPE_AHEAD_CYCLE_CONTEXT),
        ),
    ]);
}

actions!(
    lyrics,
    [
        /// Stamp the cursor's line with the current playback position and
        /// step to the next, bound to Shift+Enter while the editor is open.
        StampLine
    ]
);

/// The playback bindings' scope as a plain context, for the tooltips that
/// show what key runs the button they're on. A lookup parses its argument
/// as a context to match predicates against, and the binding's own scope
/// (which excludes the search box) isn't one, so passing it finds no
/// binding and the tip loses its key.
pub const PLAYBACK_TIP_SCOPE: Option<&'static str> = Some("Workspace");
