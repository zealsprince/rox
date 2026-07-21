//! Persisted app settings: one JSON file in the app's data directory, next
//! to the library database. Writers each own a few fields (the player its
//! playback state, the workspace its window and layout) and write through
//! [`Settings::update`], which reloads the file first so one writer's save
//! never reverts another's fields to what they were at startup.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

use gpui::{App, SharedString, WindowDecorations};
use serde::{Deserialize, Serialize};

use rox_playback::engine::LoopMode;

use crate::design::palette::Palette;

/// The folder holding the running executable, portable mode's anchor.
/// None when the exe path can't be read, which just leaves portable off.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
}

/// The marker file beside the executable that keeps portable mode on
/// across launches; the Behavior page's toggle creates and removes it.
pub fn portable_marker() -> Option<PathBuf> {
    exe_dir().map(|dir| dir.join("portable"))
}

/// The portable data folder beside the executable. Named rox-data rather
/// than data so it stays recognizable in a folder shared with other apps.
pub fn portable_data_dir() -> Option<PathBuf> {
    exe_dir().map(|dir| dir.join("rox-data"))
}

/// The resolved data root and whether it is the portable one, decided
/// once per process so a mid-run toggle can't split the stores: the
/// `portable` marker beside the executable, or a `--portable` flag for
/// one run, routes everything into rox-data; a flip lands on the next
/// launch.
static DATA_DIR: OnceLock<(PathBuf, bool)> = OnceLock::new();

fn resolve_data_dir() -> (PathBuf, bool) {
    let portable = std::env::args().any(|arg| arg == "--portable")
        || portable_marker().is_some_and(|marker| marker.exists());
    if portable {
        if let Some(dir) = portable_data_dir() {
            return (dir, true);
        }
    }
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rox");
    (dir, false)
}

/// The app's data directory, shared with the library database. Created on
/// first use. Portable runs read rox-data beside the executable instead
/// of the OS data dir.
pub fn data_dir() -> PathBuf {
    let (dir, _) = DATA_DIR.get_or_init(resolve_data_dir);
    let _ = std::fs::create_dir_all(dir);
    dir.clone()
}

/// Whether this run reads the portable folder, however it was asked for.
pub fn portable() -> bool {
    DATA_DIR.get_or_init(resolve_data_dir).1
}

/// Whether the executable's folder takes writes, the portable toggle's
/// gate: install dirs (app bundles, Program Files, /usr/bin) are often
/// read-only, and a directory permission read doesn't answer reliably
/// across platforms, so probe with a real file.
pub fn portable_available() -> bool {
    let Some(dir) = exe_dir() else {
        return false;
    };
    let probe = dir.join(".rox-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Whether this launch found no settings file, the welcome window's
/// first-run signal. Recorded once at startup through [`note_first_run`],
/// before anything can write the file.
static FIRST_RUN: AtomicBool = AtomicBool::new(false);

pub fn note_first_run() {
    FIRST_RUN.store(!settings_path().exists(), Ordering::Relaxed);
}

pub fn first_run() -> bool {
    FIRST_RUN.load(Ordering::Relaxed)
}

/// The settings file inside [`data_dir`], public so the settings window
/// can hand the raw file to the system editor.
pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// Everything the app persists outside the library database. Unknown fields
/// are dropped on load and missing ones take defaults, so the file survives
/// version drift in both directions.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Linear playback volume, same range the engine clamps to (0 to 2).
    pub volume: f32,
    /// Whether output is muted. The volume above is the level mute returns
    /// to, so muting never loses the setting.
    pub muted: bool,
    /// Loop mode as its wire name: "off", "all", or "one". The engine's
    /// `LoopMode` stays serde-free; convert through the accessors.
    pub loop_mode: String,
    /// Whether playback shuffles: the queue plays in a random order
    /// instead of front to back.
    pub shuffle: bool,
    /// The main window's last frame, restored on open. None until the first
    /// window closes.
    pub window: Option<WindowState>,
    /// Whether the in-window menubar stays hidden, showing only while alt
    /// is held or a menu is open. Off by default: the bar is the way into
    /// everything.
    pub hide_menubar: bool,
    /// Whether the main workspace windows carry the OS's own decorations
    /// (titlebar, borders). Off asks the compositor for a bare
    /// client-drawn window; the window controls panel stands in for the
    /// missing buttons. Child windows (settings, popouts, editors) keep
    /// the OS chrome either way.
    pub os_decorations: bool,
    /// The dock layout as the dock crate's own serialized state, kept as raw
    /// JSON so settings stay readable even when the layout schema moves; the
    /// workspace validates and versions it on restore. None until a layout
    /// has been saved.
    pub layout: Option<serde_json::Value>,
    /// The user's saved layout presets, each a full dock dump under a name.
    /// Shipped presets live in the app's assets and are not stored here; the
    /// settings window merges the two for its list. Empty until one is saved.
    pub layouts: Vec<NamedLayout>,
    /// The preset the mini-player button toggles back to, by name. None
    /// leaves the button hidden. Resolved against the saved and shipped
    /// presets on use, so a name that no longer exists just no-ops.
    pub primary_layout: Option<String>,
    /// The preset the mini-player button toggles to, by name. None leaves
    /// the button hidden.
    pub mini_layout: Option<String>,
    /// The named preset the window is currently on, by name, so a workspace
    /// save captures the layout in front of you and the mini button knows
    /// which side it is on. None means an unnamed arrangement (the default
    /// build, an empty window, or a one-off import).
    pub active_layout: Option<String>,
    /// The user's saved workspaces: full shareable bundles of layout presets,
    /// palette, and appearance, the sharing ecosystem's trade unit. Shipped
    /// bundles live in the app's assets and are not stored here; the settings
    /// window merges the two for its list. Empty until one is saved.
    pub workspaces: Vec<WorkspaceBundle>,
    /// The folders the library scans, in the order they were added. Empty
    /// until one has been opened.
    pub library_roots: Vec<PathBuf>,
    /// The single folder `library_roots` replaced. Read once on load to
    /// seed the list, never written back.
    #[serde(skip_serializing)]
    library_root: Option<PathBuf>,
    /// ADR 10's transparency pair, both 0 to 1. How opaque the app's
    /// surfaces read, 1 fully opaque...
    pub surface_opacity: f32,
    /// ...and how strongly the backdrop shows behind them, 1 the bare
    /// bake, 0 sunk into the floor.
    pub backdrop_strength: f32,
    /// The user palette as role-name-to-`#rrggbb` entries,
    /// [`Palette::to_map`]'s shape. Empty means the default palette;
    /// unknown roles fall away on load, like the file's own fields.
    pub palette: BTreeMap<String, String>,
    /// The app-wide font family, the base every window and panel inherits.
    /// None follows the platform default. A panel's own font override layers
    /// over this; a name that is not installed falls back at render, so the
    /// file survives moving between machines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_font: Option<String>,
    /// The active icon pack by name, a folder of SVGs under the packs dir
    /// that overrides the built-in icons. None uses the built-in set, as
    /// does a name whose folder is gone. Applied at startup; a switch lands
    /// on the next launch, since rendered icons keep their cached tiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_pack: Option<String>,
    /// Whether the playing track's art re-tints the palette and backs
    /// the windows (ADR 10's derived mode). Off by default: the look
    /// only follows the music when asked to.
    pub art_theming: bool,
    /// Whether a bright cover is held to the dark ladder. Song theming
    /// still tints hue and chroma, but the surfaces never flip light. Off
    /// by default: the app follows a bright album all the way.
    pub keep_dark: bool,
    /// Whether launch loads the last playing track back up, paused where
    /// it left off. The track below is written either way; this only
    /// gates the restore.
    pub restore_last_track: bool,
    /// Whether closing the last workspace window leaves the app resident,
    /// music playing, with the tray (Linux) or the dock (macOS) as the way
    /// back in. Off quits, the default. Ignored on Windows until a tray
    /// backend exists there; a headless process would have no way back.
    pub quit_to_tray: bool,
    /// What was playing when the app closed, as a library track id so it
    /// survives path changes, plus where the clock sat. None when nothing
    /// was playing; a stale id degrades to the cold start on restore.
    pub last_track: Option<LastTrack>,
    /// The whole play queue as it stood at close, restored on the next launch
    /// so Prev/Next and the queue panel come back. Preferred over
    /// [`Settings::last_track`]; None when nothing was playing or an older
    /// file predates it, when the single-track fallback takes over.
    pub last_queue: Option<QueueState>,
    /// The last.fm connection and scrobbling knobs, the settings window's
    /// Scrobbling page.
    pub lastfm: Lastfm,
    /// The online enrichment providers and their knobs (ADR 14), the
    /// settings window's Providers page.
    pub providers: Providers,
    /// The quick-play modal's appearance knobs, edited from its own config
    /// panel.
    pub quick_play: QuickPlayConfig,
    /// How ratings read and click everywhere they show.
    pub rating_style: RatingStyle,
    /// The tag editor's last window size and column widths, restored on
    /// the next open. None until an editor closes.
    pub tag_editor: Option<TagEditorState>,
    /// The stats window's last size and range pick, restored on the next
    /// open. None until the window closes.
    pub stats_window: Option<StatsWindowState>,
    /// The app settings window's last size, restored on the next open.
    /// None until the window closes.
    pub settings_window: Option<LayoutSize>,
    /// The panel settings window's last size, shared across panels and
    /// restored on the next open. None until a window closes.
    pub panel_settings_window: Option<LayoutSize>,
    /// The view for the queue window the widget opens (its columns and album
    /// headings), so the modal and popped-out queue come back the way you
    /// left them. A docked queue panel keeps its own view in the layout dump
    /// instead. Kept as raw JSON, like the dock layout, so settings stay
    /// readable when the queue's config schema moves. None until edited.
    pub queue_view: Option<serde_json::Value>,
}

/// The rating scale: five stars for quick clicks, or a 0-10 number in
/// half steps for finer review scores. Both write the library's one
/// 0-100 value (a star is 20 points, 7.5 is 75), so flipping the style
/// never loses a rating.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RatingStyle {
    #[default]
    Stars,
    Numeric,
}

/// The live rating style, a static like the palette's: rating cells read
/// it in render paths where a settings-file load has no place. Seeded at
/// startup, flipped by the settings window.
static RATING_NUMERIC: AtomicBool = AtomicBool::new(false);

pub fn rating_style() -> RatingStyle {
    if RATING_NUMERIC.load(Ordering::Relaxed) {
        RatingStyle::Numeric
    } else {
        RatingStyle::Stars
    }
}

/// Flip the live style and repaint every window: the static sits outside
/// gpui's reactivity, so nothing else would notice. Persisting is the
/// caller's, startup seeds from the file through here too.
pub fn set_rating_style(style: RatingStyle, cx: &mut App) {
    RATING_NUMERIC.store(style == RatingStyle::Numeric, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live menubar-hidden flag, a static like the rating style's: the
/// workspace reads it per frame where a settings-file load has no place.
/// Seeded at startup, flipped by the settings window.
static HIDE_MENUBAR: AtomicBool = AtomicBool::new(false);

pub fn hide_menubar() -> bool {
    HIDE_MENUBAR.load(Ordering::Relaxed)
}

/// Flip the live flag and repaint every window: the static sits outside
/// gpui's reactivity, so nothing else would notice. Persisting is the
/// caller's, startup seeds from the file through here too.
pub fn set_hide_menubar(on: bool, cx: &mut App) {
    HIDE_MENUBAR.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live OS-decorations flag, a static like the menubar's. Seeded at
/// startup, flipped from the Window menu. Only the main workspace
/// windows follow it; child windows always open with the OS chrome.
static OS_DECORATIONS: AtomicBool = AtomicBool::new(true);

pub fn os_decorations() -> bool {
    OS_DECORATIONS.load(Ordering::Relaxed)
}

/// The flag as the decoration mode new workspace windows open with.
pub fn window_decorations() -> WindowDecorations {
    if os_decorations() {
        WindowDecorations::Server
    } else {
        WindowDecorations::Client
    }
}

/// Flip the live flag. Persisting is the caller's, and so is
/// renegotiating the open workspace windows
/// (`workspace::apply_decorations`).
pub fn set_os_decorations(on: bool) {
    OS_DECORATIONS.store(on, Ordering::Relaxed);
}

/// The live quit-to-tray flag, a static like the ones above: the window
/// close path reads it where a settings-file load has no place. Seeded at
/// startup, flipped from the Window menu and the Behavior page.
static QUIT_TO_TRAY: AtomicBool = AtomicBool::new(false);

pub fn quit_to_tray() -> bool {
    QUIT_TO_TRAY.load(Ordering::Relaxed)
}

/// Flip the live flag. Persisting is the caller's, and so is reconciling
/// the tray icon (`tray::sync`).
pub fn set_quit_to_tray(on: bool) {
    QUIT_TO_TRAY.store(on, Ordering::Relaxed);
}

/// The live app font, a static like the rating style's: window and panel
/// render paths read it where a settings-file load has no place. None
/// follows the platform default. Seeded at startup, changed by the app
/// settings window.
static APP_FONT: RwLock<Option<SharedString>> = RwLock::new(None);

/// The app-wide font family as it currently stands, for the render paths
/// that apply it at a window root and the panels that fall back to it.
pub fn app_font() -> Option<SharedString> {
    APP_FONT.read().unwrap().clone()
}

/// Set the live app font and repaint every window: the static sits outside
/// gpui's reactivity, so nothing else would notice. Persisting is the
/// caller's, startup seeds from the file through here too.
pub fn set_app_font(font: Option<String>, cx: &mut App) {
    *APP_FONT.write().unwrap() = font.map(SharedString::from);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// How the quick-play modal draws its result list, the knobs its inline
/// config panel edits. Persisted so the look survives reopening the modal,
/// which the workspace rebuilds each time.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickPlayConfig {
    /// Show a cover thumbnail at the left of each result.
    pub show_cover: bool,
    /// Show the artist and album line under each result's title.
    pub show_subtitle: bool,
    /// Show each result's duration on the right.
    pub show_duration: bool,
    /// Give each result row more height.
    pub comfortable: bool,
}

impl Default for QuickPlayConfig {
    fn default() -> Self {
        QuickPlayConfig {
            show_cover: false,
            show_subtitle: true,
            show_duration: true,
            comfortable: false,
        }
    }
}

/// The last.fm account and how scrobbling behaves. The key and secret
/// override the build's own api identity (`lastfm::keys`), for builds
/// that ship none; the session key is what the connect flow lands and
/// never expires until revoked on last.fm.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Lastfm {
    pub api_key: String,
    pub api_secret: String,
    pub session_key: String,
    /// The account the session belongs to, for the settings readout.
    pub username: String,
    /// Whether playback scrobbles at all; the connection stays either way.
    pub scrobbling: bool,
    /// How much of a track has to actually play before it scrobbles, as a
    /// fraction of its duration. The seek strip and waveform can mark it.
    pub threshold: f32,
}

impl Default for Lastfm {
    fn default() -> Self {
        Lastfm {
            api_key: String::new(),
            api_secret: String::new(),
            session_key: String::new(),
            username: String::new(),
            scrobbling: true,
            threshold: 0.5,
        }
    }
}

/// Where a fetched lyrics sheet saves: the embedded tag through the
/// writer's atomic layer, an `.lrc` sidecar next to the audio file, or
/// the app's own lyrics store under [`lyrics_dir`].
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LyricsSave {
    Tag,
    Sidecar,
    /// The default: rox's data folder, so fetches never leave files in
    /// the library and never rewrite the audio.
    #[default]
    Store,
}

/// The app's own lyrics store inside [`data_dir`], one flat folder of
/// hashed-name `.lrc` files. Not created here: the first save makes it,
/// so an unused store never leaves an empty folder behind.
pub fn lyrics_dir() -> PathBuf {
    data_dir().join("lyrics")
}

/// The artist store inside [`data_dir`]: the biography panel's fetched
/// bios and portraits, one hashed-name pair per artist. Not created
/// here: the first fetch makes it, the lyrics store's rule.
pub fn artists_dir() -> PathBuf {
    data_dir().join("artists")
}

/// The online enrichment providers (ADR 14): per-service enable toggles
/// and the per-domain knobs. Providers only ever fetch on a user action,
/// so on-by-default keeps the offline-first rule intact.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Providers {
    /// Fetch lyrics from lrclib.net when the lyrics panel asks.
    pub lrclib: bool,
    /// Where a fetched sheet lands.
    pub lyrics_save: LyricsSave,
    /// Look up tags on MusicBrainz when the metadata compare asks.
    pub musicbrainz: bool,
    /// Search iTunes for cover art when the cover lookup asks.
    pub itunes: bool,
    /// Search Deezer for cover art when the cover lookup asks.
    pub deezer: bool,
    /// Fetch artist biographies from last.fm, a Deezer portrait along,
    /// when the biography panel asks.
    pub artist: bool,
}

impl Default for Providers {
    fn default() -> Self {
        Providers {
            lrclib: true,
            lyrics_save: LyricsSave::default(),
            musicbrainz: true,
            itunes: true,
            deezer: true,
            artist: true,
        }
    }
}

/// A dock layout the user saved as a named preset: a full dock dump under
/// a name. The dump stays raw JSON like [`Settings::layout`] so the file
/// survives layout-schema moves; the workspace validates it on apply.
#[derive(Clone, Serialize, Deserialize)]
pub struct NamedLayout {
    pub name: String,
    pub dump: serde_json::Value,
    /// The window size the preset restores to, in logical pixels. None for
    /// presets from before sizes were stored, which apply at whatever size
    /// the window already has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<LayoutSize>,
}

/// A window size in logical pixels, stored with a layout preset so applying
/// it can size the window to match.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct LayoutSize {
    pub width: f32,
    pub height: f32,
}

/// The workspace bundle format version, bumped when the bundle shape changes
/// so a reader can refuse a file from a newer format. Independent of the dock
/// layout version the dumps inside carry.
pub const WORKSPACE_VERSION: u32 = 1;

/// A shareable workspace: a named set of layout presets with their
/// mini-player roles, the palette, and the appearance that dress them. The
/// unit rox's sharing ecosystem trades - written to a file by export, shipped
/// in the app's assets, and imported into the collection. Versioned so a file
/// survives shape moves; the layouts inside carry their own dock-layout
/// version the workspace validates on apply. Machine- and account-bound state
/// (library folders, last.fm, window frames) is deliberately left out, so a
/// bundle travels between installs as pure look.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceBundle {
    /// Format version; a reader refuses a bundle from a newer format.
    pub version: u32,
    /// The bundle's name. A shipped bundle falls back to its file stem when
    /// this is empty, the layouts' own convention.
    pub name: String,
    /// The layout presets the workspace carries, each a named dock dump.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub layouts: Vec<NamedLayout>,
    /// The mini-player button's two roles, by preset name, scoped to this
    /// workspace's own layouts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mini_layout: Option<String>,
    /// The palette as role-name-to-`#rrggbb`, [`Palette::to_map`]'s shape;
    /// empty means the default palette.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub palette: BTreeMap<String, String>,
    /// The appearance knobs the workspace dresses the app with.
    pub appearance: AppearanceBundle,
}

impl Default for WorkspaceBundle {
    fn default() -> Self {
        WorkspaceBundle {
            version: WORKSPACE_VERSION,
            name: String::new(),
            layouts: Vec::new(),
            primary_layout: None,
            mini_layout: None,
            palette: BTreeMap::new(),
            appearance: AppearanceBundle::default(),
        }
    }
}

/// The appearance a workspace carries: the visual knobs it dresses the app
/// with, pulled from and pushed back to [`Settings`]. The subset that reads
/// as pure look, so a bundle recolors and rearranges without dragging along
/// another machine's folders or account.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceBundle {
    pub surface_opacity: f32,
    pub backdrop_strength: f32,
    pub art_theming: bool,
    pub keep_dark: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_font: Option<String>,
    pub rating_style: RatingStyle,
    pub quick_play: QuickPlayConfig,
    pub hide_menubar: bool,
    pub os_decorations: bool,
}

impl Default for AppearanceBundle {
    fn default() -> Self {
        AppearanceBundle {
            surface_opacity: 1.0,
            backdrop_strength: 1.0,
            art_theming: false,
            keep_dark: false,
            app_font: None,
            rating_style: RatingStyle::default(),
            quick_play: QuickPlayConfig::default(),
            hide_menubar: false,
            os_decorations: true,
        }
    }
}

impl WorkspaceBundle {
    /// Snapshot the current shareable state into a named bundle: the layouts
    /// and their roles, the palette, and the appearance. Reads the persisted
    /// settings, which every live edit already writes through.
    ///
    /// Folds the live dock into the layout the window is on, but only inside
    /// this bundle's own copy of the layouts: a save captures what is in front
    /// of you without editing the global preset pool other workspaces share,
    /// since layouts belong to the workspace they were saved in, not to a
    /// shared pool. An unnamed arrangement lands as "Untitled", and when the
    /// bundle has no primary the captured layout becomes it, so it fills the
    /// window on apply. No live dock yet leaves the layouts as they are.
    pub fn from_settings(name: String, s: &Settings) -> WorkspaceBundle {
        let mut layouts = s.layouts.clone();
        let mut primary_layout = s.primary_layout.clone();
        if let Some(dump) = s.layout.clone() {
            let active = s
                .active_layout
                .clone()
                .unwrap_or_else(|| "Untitled".to_string());
            let size = s.window.as_ref().map(|w| LayoutSize {
                width: w.width,
                height: w.height,
            });
            if let Some(existing) = layouts.iter_mut().find(|l| l.name == active) {
                existing.dump = dump;
                existing.size = size;
            } else {
                layouts.push(NamedLayout {
                    name: active.clone(),
                    dump,
                    size,
                });
            }
            if primary_layout.is_none() {
                primary_layout = Some(active);
            }
        }
        WorkspaceBundle {
            version: WORKSPACE_VERSION,
            name,
            layouts,
            primary_layout,
            mini_layout: s.mini_layout.clone(),
            palette: s.palette.clone(),
            appearance: AppearanceBundle {
                surface_opacity: s.surface_opacity,
                backdrop_strength: s.backdrop_strength,
                art_theming: s.art_theming,
                keep_dark: s.keep_dark,
                app_font: s.app_font.clone(),
                rating_style: s.rating_style,
                quick_play: s.quick_play.clone(),
                hide_menubar: s.hide_menubar,
                os_decorations: s.os_decorations,
            },
        }
    }

    /// Replace the settings' shareable state with this bundle's, the apply's
    /// persistence half. The live statics stay the caller's, since they need
    /// an `App` this layer doesn't reach.
    pub fn apply_to(self, s: &mut Settings) {
        s.layouts = self.layouts;
        s.primary_layout = self.primary_layout;
        s.mini_layout = self.mini_layout;
        s.palette = self.palette;
        let a = self.appearance;
        s.surface_opacity = a.surface_opacity;
        s.backdrop_strength = a.backdrop_strength;
        s.art_theming = a.art_theming;
        s.keep_dark = a.keep_dark;
        s.app_font = a.app_font;
        s.rating_style = a.rating_style;
        s.quick_play = a.quick_play;
        s.hide_menubar = a.hide_menubar;
        s.os_decorations = a.os_decorations;
    }
}

/// The closing snapshot of the playing track: its library id and the
/// position clock in seconds. Superseded by [`QueueState`] for files written
/// since; kept as the single-track fallback so an older settings file still
/// restores something.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct LastTrack {
    pub id: i64,
    pub position_secs: f64,
}

/// The closing snapshot of the whole play queue, restored as a full session
/// on the next launch so Prev and Next walk the same order and the up-next
/// queue panel comes back. Entries are library ids so they survive path
/// changes; one whose file has left the library drops out on restore, the
/// cursor shifting to stay on the track that was playing.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct QueueState {
    /// The play order, history and upcoming both, in the order the engine
    /// held them.
    pub entries: Vec<QueuedTrack>,
    /// Index into `entries` of the track that was playing.
    pub cursor: usize,
    /// Where that track's clock sat, in seconds.
    pub position_secs: f64,
}

/// One entry in a persisted [`QueueState`]: the track's library id and
/// whether it was hand-queued (Play Next, Add to Queue) rather than part of
/// the playing context. The queue panel lists only the explicit ones.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct QueuedTrack {
    pub id: i64,
    pub explicit: bool,
}

/// The tag editor's remembered shape: window size in logical pixels and
/// the table's column widths in field order. Every editor window writes
/// it on close, the last writer wins.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TagEditorState {
    pub width: f32,
    pub height: f32,
    pub columns: Vec<f32>,
}

/// The stats window's remembered shape: size in logical pixels and the
/// range pick, written on close and when the range changes. The range
/// rides as the pick's key ("all", "year", "month"), decoded back in
/// [`crate::stats_window`]; an unknown key falls back to all time.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StatsWindowState {
    pub width: f32,
    pub height: f32,
    pub range: String,
}

/// A window frame in logical pixels, plus whether the window was maximized
/// (the frame is then the restore size).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            volume: 1.0,
            muted: false,
            loop_mode: "off".into(),
            shuffle: false,
            window: None,
            hide_menubar: false,
            os_decorations: true,
            layout: None,
            layouts: Vec::new(),
            primary_layout: None,
            mini_layout: None,
            active_layout: None,
            workspaces: Vec::new(),
            library_roots: Vec::new(),
            library_root: None,
            surface_opacity: 1.0,
            backdrop_strength: 1.0,
            palette: BTreeMap::new(),
            app_font: None,
            icon_pack: None,
            art_theming: false,
            keep_dark: false,
            restore_last_track: true,
            quit_to_tray: false,
            last_track: None,
            last_queue: None,
            lastfm: Lastfm::default(),
            providers: Providers::default(),
            quick_play: QuickPlayConfig::default(),
            rating_style: RatingStyle::default(),
            tag_editor: None,
            stats_window: None,
            settings_window: None,
            panel_settings_window: None,
            queue_view: None,
        }
    }
}

impl Settings {
    /// Read the settings file, falling back to defaults if it is missing or
    /// unreadable. A corrupt file logs and resets rather than blocking start.
    pub fn load() -> Settings {
        let path = settings_path();
        let mut settings: Settings = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("settings: resetting {}: {e}", path.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        };
        // A hand-edited volume seeds the engine's atomics directly, so the
        // engine's clamp range applies here too.
        settings.volume = if settings.volume.is_finite() {
            settings.volume.clamp(0.0, 2.0)
        } else {
            1.0
        };
        // The transparency pair reads straight into color math, so
        // hand-edited values clamp to the unit range.
        for scalar in [
            &mut settings.surface_opacity,
            &mut settings.backdrop_strength,
        ] {
            *scalar = if scalar.is_finite() {
                scalar.clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        // The threshold reads straight into the scrobble math and the
        // marker paint, so a hand-edited value clamps to a sane band.
        settings.lastfm.threshold = if settings.lastfm.threshold.is_finite() {
            settings.lastfm.threshold.clamp(0.1, 1.0)
        } else {
            0.5
        };
        // A file from before multi-folder carries one library_root; it
        // seeds the list here and the next save drops it.
        if settings.library_roots.is_empty() {
            if let Some(root) = settings.library_root.take() {
                settings.library_roots.push(root);
            }
        }
        settings
    }

    /// Change some fields and persist: reload the file, apply, write it
    /// back. Writers hold their own in-memory copies for reads, so going
    /// through the file here is what keeps one writer's save from
    /// reverting another's fields.
    pub fn update(f: impl FnOnce(&mut Settings)) {
        let mut settings = Settings::load();
        f(&mut settings);
        settings.save();
    }

    /// Write the whole file. Failures log and move on; settings loss is not
    /// worth interrupting playback for.
    fn save(&self) {
        let path = settings_path();
        let text = serde_json::to_string_pretty(self).expect("settings serialize");
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("settings: writing {}: {e}", path.display());
        }
    }

    /// The user palette the map holds, over the defaults.
    pub fn palette(&self) -> Palette {
        Palette::from_map(&self.palette)
    }

    pub fn loop_mode(&self) -> LoopMode {
        match self.loop_mode.as_str() {
            "all" => LoopMode::All,
            "one" => LoopMode::One,
            _ => LoopMode::Off,
        }
    }

    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        self.loop_mode = match mode {
            LoopMode::Off => "off",
            LoopMode::All => "all",
            LoopMode::One => "one",
        }
        .into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bundle must survive the file trip and land back on a fresh settings
    /// intact, or a shared workspace drifts on every hop.
    #[test]
    fn workspace_bundle_roundtrips() {
        let mut src = Settings::default();
        src.surface_opacity = 0.5;
        src.art_theming = true;
        src.keep_dark = true;
        src.rating_style = RatingStyle::Numeric;
        src.hide_menubar = true;
        src.palette.insert("accent".into(), "#336699".into());
        src.layouts.push(NamedLayout {
            name: "one".into(),
            dump: serde_json::json!({ "k": "v" }),
            size: None,
        });
        src.primary_layout = Some("one".into());

        let bundle = WorkspaceBundle::from_settings("mine".into(), &src);
        let json = serde_json::to_string(&bundle).unwrap();
        let back: WorkspaceBundle = serde_json::from_str(&json).unwrap();

        let mut dst = Settings::default();
        back.apply_to(&mut dst);
        assert_eq!(dst.surface_opacity, 0.5);
        assert!(dst.art_theming);
        assert!(dst.keep_dark);
        assert!(dst.rating_style == RatingStyle::Numeric);
        assert!(dst.hide_menubar);
        assert_eq!(
            dst.palette.get("accent").map(String::as_str),
            Some("#336699")
        );
        assert_eq!(dst.layouts.len(), 1);
        assert_eq!(dst.primary_layout.as_deref(), Some("one"));
    }

    /// The bundle carries only the look, never machine- or account-bound
    /// state, so a shared file can't drag another install's folders or
    /// last.fm session along.
    #[test]
    fn workspace_bundle_omits_machine_state() {
        let bundle = WorkspaceBundle::from_settings("mine".into(), &Settings::default());
        let json = serde_json::to_string(&bundle).unwrap();
        assert!(!json.contains("library_root"));
        assert!(!json.contains("lastfm"));
        assert!(!json.contains("session_key"));
        assert!(!json.contains("last_track"));
    }
}
