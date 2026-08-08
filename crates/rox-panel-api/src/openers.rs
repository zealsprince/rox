//! The way up into the app's windows. Panels and the shared helpers need to
//! open a tag editor, a stats page, a rename flyout - all of which live in
//! the binary, a crate above this one. Rather than depend upward, the binary
//! hands down a table of function pointers once at startup and everything
//! here calls through it.
//!
//! Every entry takes and returns types this crate or one below it owns, so
//! the table never leaks a concrete panel or a workspace. A call made before
//! the binary installs the table logs and does nothing; that only happens in
//! a unit test that never opens a window, so it must never panic.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use gpui::{div, AnyWeakEntity, App, Div, Entity, EntityId, SharedString, WeakEntity, Window};
use gpui_component::menu::PopupMenu;
use rox_dock::{PanelView, TabPanel};
use rox_library::cue::TrackKey;
use rox_services::backdrop::NowPlayingArt;
use rox_services::catalog::Library;

use crate::panel::AppState;

/// The app's windows, as plain function pointers. One field per call a
/// panel or a shared helper makes upward.
pub struct Openers {
    /// The tag editor over a track selection.
    pub tags_editor: fn(AppState, Vec<i64>, &mut App),
    /// The metadata compare for one file, writing what's applied.
    pub tags_matcher: fn(Entity<Library>, Entity<NowPlayingArt>, TrackKey, &mut App),
    /// The cover editor over a track selection.
    pub cover_editor: fn(AppState, Vec<i64>, &mut App),
    /// The new-playlist prompt, seeded with the tracks to file into it.
    pub playlist_create: fn(AppState, Vec<i64>, &mut App),
    /// The rename prompt for an existing playlist.
    pub playlist_rename: fn(AppState, i64, String, &mut App),
    /// The equalizer window.
    pub eq_window: fn(&mut App),
    /// The library stats page over a workspace's state.
    pub stats_window: fn(AppState, &mut App),
    /// The signals window, where the shared pool is tended.
    pub signals_window: fn(&mut App),
    /// The failed-with-a-reason placeholder a panel shows in place of its
    /// content, with the button into the console.
    pub console_notice: fn(SharedString) -> Div,
    /// Register a lyrics panel for the reload broadcast. The handle is
    /// type-erased on the way down and downcast on the way back up.
    pub lyrics_watch: fn(AnyWeakEntity, &mut App),
    /// The lyrics editor over one file.
    pub lyrics_edit: fn(AppState, PathBuf, &mut App),
    /// The lyrics search over one file.
    pub lyrics_matcher: fn(AppState, PathBuf, &mut App),
    /// Tell every watching lyrics panel a file changed on disk.
    pub lyrics_saved: fn(&Path, &mut App),
    /// The Add Panel flyout, built from the app's panel catalog.
    pub add_panel_submenu:
        fn(PopupMenu, Option<WeakEntity<TabPanel>>, &mut Window, &mut App) -> PopupMenu,
    /// The "Group Settings" row a hosted panel's menu carries, so the
    /// composite it sits in is reachable from the child.
    pub host_settings_item: fn(PopupMenu, EntityId, &App) -> PopupMenu,
    /// Put up the confirm a pinned panel's Close needs, and close from
    /// there. Needs a workspace behind the window to float the dialog, so
    /// it no-ops in a popout window with none.
    pub confirm_close_locked: fn(Arc<dyn PanelView>, WeakEntity<TabPanel>, &mut Window, &mut App),
}

static OPENERS: OnceLock<Openers> = OnceLock::new();

/// Install the app's window table. Called once from `main`, before any
/// window opens.
pub fn install(openers: Openers) {
    let _ = OPENERS.set(openers);
}

/// The installed table, or None with a line in the log. Callers fall back
/// to doing nothing rather than failing.
fn openers(what: &str) -> Option<&'static Openers> {
    match OPENERS.get() {
        Some(openers) => Some(openers),
        None => {
            log::warn!("{what} was called before the app installed its openers");
            None
        }
    }
}

/// Open the tag editor over `ids`.
pub fn tags_editor(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if let Some(openers) = openers("the tag editor") {
        (openers.tags_editor)(state, ids, cx);
    }
}

/// Open the metadata compare for `path`.
pub fn tags_matcher(
    library: Entity<Library>,
    now_art: Entity<NowPlayingArt>,
    key: TrackKey,
    cx: &mut App,
) {
    if let Some(openers) = openers("the metadata compare") {
        (openers.tags_matcher)(library, now_art, key, cx);
    }
}

/// Open the cover editor over `ids`.
pub fn cover_editor(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if let Some(openers) = openers("the cover editor") {
        (openers.cover_editor)(state, ids, cx);
    }
}

/// Prompt for a new playlist holding `ids`.
pub fn playlist_create(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if let Some(openers) = openers("the new-playlist prompt") {
        (openers.playlist_create)(state, ids, cx);
    }
}

/// Prompt to rename the playlist `id`, starting from `current`.
pub fn playlist_rename(state: AppState, id: i64, current: String, cx: &mut App) {
    if let Some(openers) = openers("the playlist rename prompt") {
        (openers.playlist_rename)(state, id, current, cx);
    }
}

/// Open the equalizer window.
pub fn eq_window(cx: &mut App) {
    if let Some(openers) = openers("the equalizer window") {
        (openers.eq_window)(cx);
    }
}

/// Open the library stats page.
pub fn stats_window(state: AppState, cx: &mut App) {
    if let Some(openers) = openers("the stats window") {
        (openers.stats_window)(state, cx);
    }
}

/// Open the signals window.
pub fn signals_window(cx: &mut App) {
    if let Some(openers) = openers("the signals window") {
        (openers.signals_window)(cx);
    }
}

/// The failed-with-a-reason placeholder, empty before the app installs it.
pub fn console_notice(message: impl Into<SharedString>) -> Div {
    match openers("the console notice") {
        Some(openers) => (openers.console_notice)(message.into()),
        None => div(),
    }
}

/// Register a lyrics panel for the reload broadcast.
pub fn lyrics_watch(panel: AnyWeakEntity, cx: &mut App) {
    if let Some(openers) = openers("the lyrics watch") {
        (openers.lyrics_watch)(panel, cx);
    }
}

/// Open the lyrics editor for `path`.
pub fn lyrics_edit(state: AppState, path: PathBuf, cx: &mut App) {
    if let Some(openers) = openers("the lyrics editor") {
        (openers.lyrics_edit)(state, path, cx);
    }
}

/// Open the lyrics search for `path`.
pub fn lyrics_matcher(state: AppState, path: PathBuf, cx: &mut App) {
    if let Some(openers) = openers("the lyrics search") {
        (openers.lyrics_matcher)(state, path, cx);
    }
}

/// Tell every watching lyrics panel that `path` changed on disk.
pub fn lyrics_saved(path: &Path, cx: &mut App) {
    if let Some(openers) = openers("the lyrics reload broadcast") {
        (openers.lyrics_saved)(path, cx);
    }
}

/// Append the Add Panel flyout, or leave the menu as it is.
pub fn add_panel_submenu(
    menu: PopupMenu,
    tab_panel: Option<WeakEntity<TabPanel>>,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    match openers("the Add Panel flyout") {
        Some(openers) => (openers.add_panel_submenu)(menu, tab_panel, window, cx),
        None => menu,
    }
}

/// Append the hosting composite's settings row, or leave the menu as it is.
pub fn host_settings_item(menu: PopupMenu, child: EntityId, cx: &App) -> PopupMenu {
    match openers("the host settings row") {
        Some(openers) => (openers.host_settings_item)(menu, child, cx),
        None => menu,
    }
}

/// Put up the confirm behind a pinned panel's Close.
pub fn confirm_close_locked(
    panel: Arc<dyn PanelView>,
    tabs: WeakEntity<TabPanel>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(openers) = openers("the pinned-panel close confirm") {
        (openers.confirm_close_locked)(panel, tabs, window, cx);
    }
}
