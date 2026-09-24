//! The open workspace windows, registered as each opens and dropped in its
//! close hook. The workspace entity is defined in the binary, so the
//! registry holds it type-erased and the binary downcasts it back.

use std::collections::BTreeMap;
use std::sync::RwLock;

use gpui::{AnyWeakEntity, AnyWindowHandle, App, Global, Window};

use crate::panel::AppState;

/// Ordered frontmost first: activation moves a window to the head. Launch
/// order is [`OpenWorkspace::opened`].
#[derive(Default)]
pub struct WorkspaceWindows {
    pub open: Vec<OpenWorkspace>,
    /// Counts up for the life of the process, so reordering never changes it.
    pub next_opened: u64,
}

pub struct OpenWorkspace {
    pub handle: AnyWindowHandle,
    /// Type-erased. `is_upgradable` tells a live entry from one on its way out.
    pub workspace: AnyWeakEntity,
    /// Kept here so app-level lookups don't need the workspace type.
    pub state: AppState,
    /// Launch serial. The list reorders on activation, so the oldest window
    /// is the smallest serial.
    pub opened: u64,
}

impl Global for WorkspaceWindows {}

/// Move a window to the head of the registry. A window that isn't
/// registered (one on its way out) is left alone.
pub fn note_activated(handle: AnyWindowHandle, cx: &mut App) {
    let open = &mut cx.default_global::<WorkspaceWindows>().open;
    if let Some(ix) = open.iter().position(|w| w.handle == handle) {
        let entry = open.remove(ix);
        open.insert(0, entry);
    }
}

/// The frontmost live workspace window: what the tray opens and whose
/// player its Play/Pause drives.
pub fn front_workspace(cx: &mut App) -> Option<(AnyWindowHandle, AppState)> {
    cx.default_global::<WorkspaceWindows>()
        .open
        .iter()
        .find(|w| w.workspace.is_upgradable())
        .map(|w| (w.handle, w.state.clone()))
}

/// The last title set on each window. gpui has no `get_title` off macOS, so
/// the control socket's window list and the fallback titlebar read this.
/// Every title goes through [`set_window_title`] to keep it true.
static WINDOW_TITLES: RwLock<BTreeMap<u64, String>> = RwLock::new(BTreeMap::new());

/// Set and remember a window's title. On Wayland only a post-open set
/// reaches the compositor; the creation-time title is ignored.
pub fn set_window_title(window: &mut Window, title: &str) {
    window.set_window_title(title);
    let id = window.window_handle().window_id().as_u64();
    WINDOW_TITLES.write().unwrap().insert(id, title.to_string());
}

pub fn window_title(id: u64) -> Option<String> {
    WINDOW_TITLES.read().unwrap().get(&id).cloned()
}
