//! The open workspace windows, registered as each one opens and dropped in
//! its close hook. Handles rather than a count, so code that only has a
//! window can find the state behind it, and the decorations and shader
//! sweeps apply to exactly these windows.
//!
//! The workspace entity itself is defined up in the binary, so the registry
//! holds it type-erased: everything down here needs is the handle, the
//! shared state, and whether the entity is still alive. The binary's own
//! wrappers downcast it back when they need the typed entity.

use std::collections::BTreeMap;
use std::sync::RwLock;

use gpui::{AnyWeakEntity, AnyWindowHandle, App, Global, Window};

use crate::panel::AppState;

/// The registry itself, a gpui global.
///
/// Ordered frontmost first: taking focus moves a window to the head, so
/// [`front_workspace`] and everything routed through it follow the user
/// instead of the launch order. Code that wants the launch order reads
/// [`OpenWorkspace::opened`] rather than the list's head.
#[derive(Default)]
pub struct WorkspaceWindows {
    pub open: Vec<OpenWorkspace>,
    /// The serial the next window to open takes, counting up for the life
    /// of the process so reordering the list never changes it.
    pub next_opened: u64,
}

/// One registered workspace window.
pub struct OpenWorkspace {
    pub handle: AnyWindowHandle,
    /// The workspace behind the window, type-erased. Downcast it back
    /// where the concrete entity is needed; `is_upgradable` is enough to
    /// tell a live entry from one on its way out.
    pub workspace: AnyWeakEntity,
    /// The shared entities this window renders over, kept on the entry so
    /// the app-level lookups resolve without touching the workspace type.
    pub state: AppState,
    /// When this window opened, counting up from the first. The list
    /// reorders on activation, so anything that needs the oldest window
    /// picks the smallest serial rather than the head.
    pub opened: u64,
}

impl Global for WorkspaceWindows {}

/// Move a workspace window to the head of the registry, so the frontmost
/// lookups return it. Every workspace window's activation observer
/// calls this; a window that isn't registered (one on its way out) is
/// left alone.
pub fn note_activated(handle: AnyWindowHandle, cx: &mut App) {
    let open = &mut cx.default_global::<WorkspaceWindows>().open;
    if let Some(ix) = open.iter().position(|w| w.handle == handle) {
        let entry = open.remove(ix);
        open.insert(0, entry);
    }
}

/// The frontmost open workspace window and its shared state: what the tray
/// activates on Open, and whose player its Play/Pause drives. The registry
/// is frontmost first, so this is the last workspace window the user was
/// in. Skips entries whose entity is already gone.
pub fn front_workspace(cx: &mut App) -> Option<(AnyWindowHandle, AppState)> {
    cx.default_global::<WorkspaceWindows>()
        .open
        .iter()
        .find(|w| w.workspace.is_upgradable())
        .map(|w| (w.handle, w.state.clone()))
}

/// The last title each window took, by window id. The platform has no
/// `get_title` off macOS, so the control socket's window list reads this
/// instead; every title set in rox goes through [`set_window_title`] to keep
/// it true. Entries for closed windows linger, which is fine: the only
/// reader iterates live handles and looks their ids up here.
static WINDOW_TITLES: RwLock<BTreeMap<u64, String>> = RwLock::new(BTreeMap::new());

/// Set a window's title and remember it. The direct gpui call is one-way,
/// so this wrapper is the app's titling path. The Wayland note applies here
/// too: only a post-open set gets through to the compositor.
pub fn set_window_title(window: &mut Window, title: &str) {
    window.set_window_title(title);
    let id = window.window_handle().window_id().as_u64();
    WINDOW_TITLES.write().unwrap().insert(id, title.to_string());
}

/// The remembered title for a window id, for the control socket's listing.
pub fn window_title(id: u64) -> Option<String> {
    WINDOW_TITLES.read().unwrap().get(&id).cloned()
}
