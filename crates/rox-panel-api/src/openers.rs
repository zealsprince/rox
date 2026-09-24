//! The way up into the app's windows. The tag editor, the stats page and
//! the other windows panels open live in the binary, a crate above this
//! one, so the binary installs a table of function pointers at startup and
//! everything here calls through it.
//!
//! Entries only use types this crate or one below it owns. A call before
//! install logs and does nothing; that only happens in a unit test with no
//! windows, so it must never panic.

use std::sync::{Arc, OnceLock};

use gpui::{AnyWeakEntity, App, Div, Entity, EntityId, SharedString, WeakEntity, Window, div};
use gpui_component::menu::PopupMenu;
use rox_dock::{PanelView, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::lyrics::Subject;
use rox_services::backdrop::NowPlayingArt;
use rox_services::catalog::Library;
use rox_services::lyrics::LyricsTarget;

use crate::panel::AppState;
use crate::panel::shader::edit::ShaderEditTarget;
use crate::preset_browser::PresetHost;

pub struct Openers {
    pub tags_editor: fn(AppState, Vec<i64>, &mut App),
    /// The metadata compare for one file.
    pub tags_matcher: fn(Entity<Library>, Entity<NowPlayingArt>, TrackKey, &mut App),
    pub cover_editor: fn(AppState, Vec<i64>, &mut App),
    pub rename_dialog: fn(AppState, Vec<i64>, &mut App),
    pub convert_dialog: fn(AppState, Vec<i64>, &mut App),
    /// Whether ffmpeg is installed. Menus hide "Convert..." without it.
    pub convert_available: fn() -> bool,
    pub playlist_create: fn(AppState, Vec<i64>, &mut App),
    pub playlist_rename: fn(AppState, i64, String, &mut App),
    /// The position is taken at the press, not at the save, so typing a name
    /// doesn't drift the mark down the track.
    pub bookmark_new: fn(AppState, TrackKey, f64, &mut App),
    pub bookmark_edit: fn(AppState, i64, &mut App),
    /// None for a new smart playlist, Some to edit that playlist's query.
    pub smart_playlist: fn(AppState, Option<i64>, &mut App),
    pub eq_window: fn(&mut App),
    pub stats_window: fn(AppState, &mut App),
    pub health_window: fn(AppState, &mut App),
    pub station_directory: fn(AppState, &mut App),
    pub signals_window: fn(&mut App),
    pub shader_editor: fn(AppState, ShaderEditTarget, &mut App),
    pub milkdrop_picker: fn(Box<dyn PresetHost>, &mut App),
    /// The failed-with-a-reason placeholder a panel shows, with a button into the console.
    pub console_notice: fn(SharedString) -> Div,
    /// Register a lyrics panel for the reload broadcast. The handle is type-erased.
    pub lyrics_watch: fn(AnyWeakEntity, &mut App),
    pub lyrics_edit: fn(AppState, LyricsTarget, &mut App),
    pub lyrics_matcher: fn(AppState, LyricsTarget, &mut App),
    /// Tell every watching lyrics panel a subject's sheet changed.
    pub lyrics_saved: fn(&Subject, &mut App),
    /// Hand every watching lyrics panel the editor's unsaved draft, or None to
    /// take it back, so offset nudges show in the panel while the editor's open.
    pub lyrics_preview: fn(&Subject, Option<&str>, &mut App),
    pub add_panel_submenu:
        fn(PopupMenu, Option<WeakEntity<TabPanel>>, &mut Window, &mut App) -> PopupMenu,
    /// The "Group Settings" row that reaches a hosted panel's composite.
    pub host_settings_item: fn(PopupMenu, EntityId, &App) -> PopupMenu,
    /// Confirm and close a pinned panel. No-ops in a popout window, which has no
    /// workspace to float the dialog over.
    pub confirm_close_locked: fn(Arc<dyn PanelView>, WeakEntity<TabPanel>, &mut Window, &mut App),
}

static OPENERS: OnceLock<Openers> = OnceLock::new();

/// Called once from `main`, before any window opens.
pub fn install(openers: Openers) {
    let _ = OPENERS.set(openers);
}

fn openers(what: &str) -> Option<&'static Openers> {
    match OPENERS.get() {
        Some(openers) => Some(openers),
        None => {
            log::warn!("{what} was called before the app installed its openers");
            None
        }
    }
}

pub fn tags_editor(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if let Some(openers) = openers("the tag editor") {
        (openers.tags_editor)(state, ids, cx);
    }
}

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

pub fn cover_editor(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if let Some(openers) = openers("the cover editor") {
        (openers.cover_editor)(state, ids, cx);
    }
}

pub fn rename_dialog(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if let Some(openers) = openers("the rename dialog") {
        (openers.rename_dialog)(state, ids, cx);
    }
}

pub fn convert_dialog(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if let Some(openers) = openers("the convert dialog") {
        (openers.convert_dialog)(state, ids, cx);
    }
}

/// False before the app installs its table, which keeps unit-test menus quiet.
pub fn convert_available() -> bool {
    match OPENERS.get() {
        Some(openers) => (openers.convert_available)(),
        None => false,
    }
}

pub fn playlist_create(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if let Some(openers) = openers("the new-playlist prompt") {
        (openers.playlist_create)(state, ids, cx);
    }
}

pub fn playlist_rename(state: AppState, id: i64, current: String, cx: &mut App) {
    if let Some(openers) = openers("the playlist rename prompt") {
        (openers.playlist_rename)(state, id, current, cx);
    }
}

pub fn bookmark_new(state: AppState, key: TrackKey, secs: f64, cx: &mut App) {
    if let Some(openers) = openers("the new-bookmark prompt") {
        (openers.bookmark_new)(state, key, secs, cx);
    }
}

pub fn bookmark_edit(state: AppState, id: i64, cx: &mut App) {
    if let Some(openers) = openers("the bookmark edit prompt") {
        (openers.bookmark_edit)(state, id, cx);
    }
}

pub fn smart_playlist(state: AppState, id: Option<i64>, cx: &mut App) {
    if let Some(openers) = openers("the smart playlist editor") {
        (openers.smart_playlist)(state, id, cx);
    }
}

pub fn eq_window(cx: &mut App) {
    if let Some(openers) = openers("the equalizer window") {
        (openers.eq_window)(cx);
    }
}

pub fn stats_window(state: AppState, cx: &mut App) {
    if let Some(openers) = openers("the stats window") {
        (openers.stats_window)(state, cx);
    }
}

pub fn health_window(state: AppState, cx: &mut App) {
    if let Some(openers) = openers("the health window") {
        (openers.health_window)(state, cx);
    }
}

pub fn station_directory(state: AppState, cx: &mut App) {
    if let Some(openers) = openers("the station directory") {
        (openers.station_directory)(state, cx);
    }
}

pub fn signals_window(cx: &mut App) {
    if let Some(openers) = openers("the signals window") {
        (openers.signals_window)(cx);
    }
}

pub fn milkdrop_picker(host: Box<dyn PresetHost>, cx: &mut App) {
    if let Some(openers) = openers("the preset picker") {
        (openers.milkdrop_picker)(host, cx);
    }
}

pub fn console_notice(message: impl Into<SharedString>) -> Div {
    match openers("the console notice") {
        Some(openers) => (openers.console_notice)(message.into()),
        None => div(),
    }
}

pub fn lyrics_watch(panel: AnyWeakEntity, cx: &mut App) {
    if let Some(openers) = openers("the lyrics watch") {
        (openers.lyrics_watch)(panel, cx);
    }
}

pub fn lyrics_edit(state: AppState, target: LyricsTarget, cx: &mut App) {
    if let Some(openers) = openers("the lyrics editor") {
        (openers.lyrics_edit)(state, target, cx);
    }
}

pub fn lyrics_matcher(state: AppState, target: LyricsTarget, cx: &mut App) {
    if let Some(openers) = openers("the lyrics search") {
        (openers.lyrics_matcher)(state, target, cx);
    }
}

pub fn shader_editor(state: AppState, target: ShaderEditTarget, cx: &mut App) {
    if let Some(openers) = openers("the shader editor") {
        (openers.shader_editor)(state, target, cx);
    }
}

pub fn lyrics_saved(subject: &Subject, cx: &mut App) {
    if let Some(openers) = openers("the lyrics reload broadcast") {
        (openers.lyrics_saved)(subject, cx);
    }
}

pub fn lyrics_preview(subject: &Subject, text: Option<&str>, cx: &mut App) {
    if let Some(openers) = openers("the lyrics preview broadcast") {
        (openers.lyrics_preview)(subject, text, cx);
    }
}

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

pub fn host_settings_item(menu: PopupMenu, child: EntityId, cx: &App) -> PopupMenu {
    match openers("the host settings row") {
        Some(openers) => (openers.host_settings_item)(menu, child, cx),
        None => menu,
    }
}

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
