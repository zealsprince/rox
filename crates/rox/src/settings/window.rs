//! The settings window: one OS window opened from the menubar, a sidebar
//! of pages on the left and the picked page's sections on the right.
//!
//! This file is the frame: the window's state and its constructor, the
//! sidebar, search, and the row helpers more than one page draws with.
//! Each page is `impl SettingsWindow` methods in a child module of its
//! own, with access to the window's private state. A method or helper
//! lives with the page that draws it, and stays here once two pages do.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, AnyWindowHandle, App, Axis, Bounds, ClipboardItem, Context, Div, ElementId, Entity,
    EntityId, FocusHandle, Global, Hsla, MouseButton, MouseDownEvent, PathPromptOptions, Pixels,
    ScrollHandle, SharedString, Stateful, Subscription, Task, WeakEntity, Window, WindowHandle,
    div, prelude::*, px, size, svg,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::text::TextView;
use gpui_component::{Icon, Root, Sizable as _};

use crate::backdrop_visual::BackdropRotation;
use crate::convert;
use crate::embeddings;
use crate::integrations::tray;
use crate::lastfm::{import, plays_import};
use crate::panel_settings;
use crate::pass_prompt;
use crate::replaygain_job;
use crate::startup::{updater, updates};
use crate::tempo_job;
use crate::workspace::{ApplyShaders, Workspace};
use rox_core::settings::layouts::Preset;
use rox_core::settings::{
    self, AcousticSave, BORDER_MAX, ChromeSide, ChromeStyle, DEFAULT_PRESENCE_FIRST_LINE,
    DEFAULT_PRESENCE_HOVER, DEFAULT_PRESENCE_SECOND_LINE, DiscordStatusLine, Frame,
    GainModeSetting, LayoutSize, LyricsSave, MARGIN_MAX, NamedLayout, PADDING_MAX, Providers,
    ROUNDING_MAX, RatingStyle, ReplayGainSave, Settings, ShuffleMode, Theme, WorkspaceMeta,
    data_dir, settings_path,
};
use rox_design::assets::icons;
use rox_design::palette::{self, Palette, ROLES, Role, Side, Sides};
use rox_design::tokens;
use rox_dock::{DockAreaState, DockEvent, PanelView, StackPanel, TabPanel};
use rox_library::stations::{self, Refusal, Station};
use rox_library::store::{BpmCoverage, GainCoverage, Stats, Storage};
use rox_net::lastfm::{AuthPhase, has_builtin_keys};
use rox_net::providers;
use rox_net::sources::stream_probe::Probe;
use rox_panel_api::panel::{self, AppState, PatternNote};
use rox_panel_api::panel_settings::{ShaderNameField, ShaderSource};
use rox_panel_api::query::search::{SearchBox, SearchEvent};
use rox_panel_api::signal_ui::{self, routes::RouteEditState};
use rox_panel_kit::ScrubState;
use rox_panel_kit::ui::{
    self as settings_ui, PageBody, Query, Rows, SECTION_GAP, Section, Seg, SidesScrub, chord,
    dialog_button, grid_columns, icon_button, kbd, kbd_line, sidebar, small_button,
};
use rox_playback::continuation;
use rox_playback::engine;
use rox_playback::output;
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::capture;
use rox_services::catalog::{Library, LibraryEvent};
use rox_services::discord_presence::DiscordPresence;
use rox_services::lastfm::Scrobbler;
use rox_services::librefm::LibreFm;
use rox_services::listenbrainz::{ListenBrainz, Status as ListenBrainzStatus};
use rox_services::player::Player;
use rox_services::thumbs::Thumbs;
use rox_viz::signal::Route;

mod analysis;
mod appearance_page;
mod application_page;
mod audio_page;
mod development_page;
mod integrations_page;
mod keymap_page;
mod library_page;
mod mcp_page;
mod ml_models_page;
mod playback_page;
mod providers_page;
mod radio_page;
mod shader_page;
mod storage_page;
mod subsonic;
mod workspace_page;

use audio_page::output_mode;
use library_page::seed_root_stats;
use ml_models_page::ModelKind;
use radio_page::read_stations;
use storage_page::StorageInfo;

// The folder table's fixed columns: the rollup numbers and the remove
// control, the last sized to `icon_button`'s footprint so the header
// aligns.
const TRACKS_COL_W: Pixels = px(56.);
const ALBUMS_COL_W: Pixels = px(56.);
const SIZE_COL_W: Pixels = px(72.);
/// Room for two row actions: a server row edits and removes, a folder row
/// only removes, and both line their numbers up against the same edge.
const ACTION_COL_W: Pixels = px(48.);

/// How often the Leveling section samples a running measurement pass. Slower
/// than the scan badge: a file takes seconds to decode, so there's nothing
/// to see at 100 ms.
const RG_POLL: Duration = Duration::from_millis(250);

/// The open settings window, if any: opening again focuses it instead
/// of stacking a second editor over the same file.
struct OpenSettings(WindowHandle<Root>);

impl Global for OpenSettings {}

/// Open the settings window, or bring the open one to the front. The
/// state holds the library for the Library page, which edits it live,
/// and the shared art bake for the window's own backdrop. The workspace
/// and its window handle are the Workspace page's subject: the tree renders
/// its dock, and an imported layout rebuilds in its window. The dock is
/// passed separately as its own handle because open runs inside a
/// workspace update, where the workspace entity can't be read.
pub fn open(
    state: AppState,
    workspace: WeakEntity<Workspace>,
    workspace_window: AnyWindowHandle,
    dock: Entity<rox_dock::DockArea>,
    cx: &mut App,
) {
    if let Some(open) = cx.try_global::<OpenSettings>() {
        let handle = open.0;
        // The refresh is for the page a caller may have asked for: the
        // window reads that request on its next draw, and activation
        // alone doesn't promise one.
        if handle
            .update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            })
            .is_ok()
        {
            return;
        }
    }
    // The last closed window's size, floored at MIN_SIZE so a stale small
    // frame never opens under the layout's minimum.
    let min = settings_ui::MIN_SIZE;
    let (width, height) = Settings::load()
        .windows
        .settings
        .filter(|s| s.width >= f32::from(min.width) && s.height >= f32::from(min.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((720., 520.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        "rox - Settings",
        bounds,
        Some(settings_ui::MIN_SIZE),
        move |window, cx| {
            cx.new(|cx| SettingsWindow::new(state, workspace, workspace_window, dock, window, cx))
        },
    );
    cx.set_global(OpenSettings(handle));
}

/// The sidebar's pages.
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Appearance,
    Application,
    Audio,
    Integrations,
    Keymap,
    Library,
    Mcp,
    MlModels,
    Playback,
    Providers,
    Radio,
    Shader,
    Storage,
    Workspace,
    Development,
}

/// The sidebar order: every page A-Z by its label, with Development
/// pinned after them. Alphabetical because no reading order still holds up
/// at a tenth page, and one that's only in someone's head costs a scan of
/// the whole list to find Storage. Development stays out of the sort
/// because it's the escape hatch: it goes with the raw file and the data
/// folder at the bottom rather than wedged between Audio and Integrations.
///
/// Nothing keys off a page's position here (the sidebar, the search
/// results stack, and every jump use the [`Page`] itself), so this list
/// can be resorted without touching anything else.
const PAGES: &[(Page, &str, &str)] = &[
    (Page::Appearance, "settings-page-appearance", icons::PALETTE),
    (
        Page::Application,
        "settings-page-application",
        icons::SLIDERS,
    ),
    (Page::Audio, "settings-page-audio", icons::AUDIO_LINES),
    (
        Page::Integrations,
        "settings-page-integrations",
        icons::GLOBE,
    ),
    (Page::Keymap, "settings-page-keymap", icons::KEYBOARD),
    (Page::Library, "settings-page-library", icons::LIST_MUSIC),
    (Page::Mcp, "settings-page-mcp", icons::LINK),
    (Page::MlModels, "settings-page-ml-models", icons::LAYERS),
    (Page::Playback, "settings-page-playback", icons::PLAY),
    (Page::Providers, "settings-page-providers", icons::DOWNLOAD),
    (Page::Radio, "settings-page-radio", icons::RADIO),
    (Page::Shader, "settings-page-shader", icons::BLEND),
    (Page::Storage, "settings-page-storage", icons::DATABASE),
    (
        Page::Workspace,
        "settings-page-workspace",
        icons::APP_WINDOW,
    ),
    (Page::Development, "settings-page-development", icons::FLASK),
];

/// A confirm dialog waiting on the user: each variant names what a yes does,
/// all of them destructive enough to ask before acting. None means no dialog.
enum Pending {
    /// Replace a saved preset's dump with the live layout.
    OverwritePreset(String),
    /// Replace a saved workspace with the current state.
    OverwriteWorkspace(String),
    /// Replace the whole live look with a workspace bundle's. Holds the
    /// card the dialog reads out, built when the dialog opens so the bundle
    /// behind it isn't reparsed every frame the dialog is up.
    ApplyWorkspace {
        card: crate::workspaces::ApplyCard,
        /// Whether the bundle just arrived from a file, which changes what
        /// the dialog says: an import has already saved it, so the offer is
        /// to apply it now rather than to replace what's there.
        imported: bool,
    },
    /// Drop one acoustic model's vectors out of the library, by model id.
    ///
    /// The first delete in this window that asks first, and the reason it
    /// breaks the rule is that nothing else here is expensive to undo. Every
    /// other clear on the storage page throws away work the app redoes on
    /// its own: thumbnails redraw as covers scroll past, waveforms decode on
    /// the next play, artist images come back the next time a panel opens.
    /// Descriptions don't come back on their own. Getting them is the
    /// analysis pass listening to every file in the library again, which is
    /// hours on a big one, so this yes gets asked for.
    ClearEmbeddings(String),
    /// Forget every tempo rox measured, the way the vectors go: the numbers
    /// don't come back until a pass has decoded every one of those tracks
    /// over, so this yes gets asked for too. It's for a better estimator:
    /// the pass only ever measures tracks with no tempo, so clearing is how
    /// improved beat counting gets applied to numbers already written.
    ClearMeasuredBpm,
    /// Throw the listening record away. The dialog this raises splits its
    /// yes the way the workspace apply does: one for the rows an import
    /// wrote, one for the whole table.
    ///
    /// Asked for like the two above, and for a reason neither of them
    /// has: there's nothing to run again afterwards. A play rox watched
    /// happen is gone, and what an import placed comes back only as far
    /// as Last.fm still remembers it.
    ClearListens,
    /// Forget one Subsonic server, by its block's id, and delete every
    /// track it filed. Asked for because the catalog only comes back by
    /// connecting and syncing again, and because the switch beside it is the
    /// non-destructive way to stop using a server; the dialog says so.
    RemoveSubsonic(u64),
}

struct SettingsWindow {
    page: Page,
    /// The sidebar's search box: a non-empty query swaps the page area
    /// for the all-pages results stack, every page filtered through
    /// [`Query`] under its own breadcrumb.
    search: Entity<SearchBox>,
    /// The button beside the box: on, the query filters the open page
    /// alone and the page stays where it is, so a hunt for "seek" on
    /// Keymap doesn't pull the scrobble threshold in from Integrations.
    search_scoped: bool,
    /// The working copy of the user palette: what the swatches show and
    /// what edits write through [`palette::set`]. A copy of the active
    /// theme's side; `editor_mode` tracks which.
    base: Palette,
    /// The theme side the working copy came from. Render re-seeds the copy
    /// and the pickers when the live mode moves off it: a theme switch
    /// here, the OS flipping under System, a workspace apply.
    editor_mode: palette::Mode,
    keep_theme: bool,
    surface_opacity: f32,
    backdrop_strength: f32,
    /// The Transparency section's All Windows switch, copied from settings
    /// like the scalars beside it.
    backdrop_all_windows: bool,
    /// The Milkdrop backdrop's working copy: the Shader page's Visual
    /// section edits this and pushes it to the live cache the backdrop
    /// layers read, with the file write debounced behind it.
    backdrop_visual_strength_scrub: ScrubState,
    backdrop_visual_scale_scrub: ScrubState,
    backdrop_visual_duration_scrub: ScrubState,
    backdrop_visual_fps_scrub: ScrubState,
    backdrop_visual_sensitivity_scrub: ScrubState,
    backdrop_visual_fade_scrub: ScrubState,
    backdrop_visual_persist_gen: u64,
    /// The app font size's working copy: what the Typography slider shows
    /// and writes through [`palette::set_app_font_size`].
    font_size: f32,
    /// The app-wide frame defaults' working copy: what the Frame sliders
    /// show and write through [`settings::set_app_frame`].
    frame: Frame,
    restore_last_track: bool,
    /// Whether the library watches its folders for changes, the Folders page
    /// toggle. Copies the setting; flipping it arms or drops the watcher on
    /// the shared library.
    watch_library: bool,
    /// Whether values differing only by case merge, the Folders page's
    /// case toggle. Copies the setting; flipping it reloads the
    /// projection so the symbol tables re-intern under the new rule.
    fold_case: bool,
    /// Whether commas and slashes split genre lists, the Folders page's
    /// separator toggle. Copies the setting; flipping it reloads the
    /// projection so the genre surfaces re-derive under the new rule.
    split_genre_compounds: bool,
    /// The separator toggle's value when the window opened. While the
    /// live value differs, the page shows the rescan note: matching
    /// follows the flip right away, but genre lists canonicalized into
    /// the database by earlier scans keep their old shape until a
    /// rescan re-reads the tags.
    split_genre_compounds_at_open: bool,
    /// The portable marker's presence, what the Behavior toggle shows;
    /// the running app stays on the data folder it started with either
    /// way, so a flip only takes effect on the next launch.
    portable: bool,
    /// Whether the executable's folder takes writes, probed once on
    /// open: install dirs are often read-only, and the toggle reads
    /// inert there.
    portable_writable: bool,
    /// A portable seed copy is running; the toggle is disabled until it
    /// finishes.
    portable_busy: bool,
    rating_style: RatingStyle,
    rating_dots: bool,
    /// The Providers page's working copy of the enrichment config.
    providers: Providers,
    /// The user's own AcoustID application key, written through per
    /// keystroke like the ffmpeg path. A build with a baked key leaves
    /// this empty and the row reads as optional.
    acoustid_key: Entity<InputState>,
    /// One picker per palette role, in [`ROLES`] order.
    pickers: Vec<Entity<ColorPickerState>>,
    surface_scrub: ScrubState,
    backdrop_scrub: ScrubState,
    font_size_scrub: ScrubState,
    margin_scrub: SidesScrub,
    padding_scrub: SidesScrub,
    rounding_scrub: ScrubState,
    border_scrub: SidesScrub,
    /// Which four-sided knobs the Frame rows have open per side. Window
    /// state rather than settings: a knob whose sides match is still
    /// split while it's being edited that way. Seeded from the knobs that
    /// already differ, so reopening shows what's set.
    margin_split: bool,
    padding_split: bool,
    border_split: bool,
    /// The one readout being typed into across this window's sliders.
    value_edit: panel::ValueEdit,
    /// The page body's scroll position, shared with the scrollbar so it
    /// can show how much page is left below the fold.
    scroll: ScrollHandle,
    /// The sidebar nav's own scroll position, for a window too short to
    /// show every page at once.
    nav_scroll: ScrollHandle,
    /// The shared catalog, the Library page's subject.
    library: Entity<Library>,
    /// The whole bundle of shared entities, kept alongside the pieces
    /// pulled out of it because the windows this page opens (the station
    /// directory) take the bundle itself.
    state: AppState,
    /// The app-wide signal pool, for the screen shader's route editor: the
    /// routes it edits are the app's, and so are the signals they read.
    signals: Arc<rox_viz::signal::SignalHub>,
    /// The workspace that opened this window, the Workspace page's subject:
    /// the tree renders its dock and imports rebuild it. Weak, so the
    /// settings window never keeps a closed workspace alive.
    workspace: WeakEntity<Workspace>,
    /// The workspace's OS window, for getting at its `Window` when an
    /// imported layout rebuilds the dock there.
    workspace_window: AnyWindowHandle,
    /// The shared art bake and this window's slice of the backdrop, so
    /// the window backs with the playing track's art like every other.
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    /// The workspace player's id, the key the window renders its art
    /// tint under. Just the id: the workspace owns the player, and the
    /// tint map drops the entry when its last player window closes.
    player: EntityId,
    /// The player itself, the Audio page's Output subject. It holds the
    /// running stream, so the negotiated readout comes off it and the mode
    /// and device picks go back through it.
    playback: Entity<Player>,
    /// The crossfade length slider's scrub, the Playback section.
    crossfade_scrub: ScrubState,
    /// The step size and preview length sliders' scrubs, the Playback
    /// page's Stepping section.
    step_scrub: ScrubState,
    step_preview_scrub: ScrubState,
    /// The live buffer slider's scrub, the Playback page's Radio section.
    /// No working copy beside it: the player owns the number and hands it
    /// to the station on air, so the row reads it back off the player the
    /// way the crossfade row does.
    live_buffer_scrub: ScrubState,
    /// The two ReplayGain dB sliders' scrubs, the Leveling section.
    preamp_scrub: ScrubState,
    fallback_scrub: ScrubState,
    /// Whether exclusive output is asked for, the Output toggle. The
    /// readout under it shows what's actually running, and the two differ
    /// whenever a claim failed.
    output_exclusive: bool,
    /// The devices the current mode can open, listed when the window opens
    /// and on the section's rescan rather than per frame: enumerating means
    /// talking to the sound system, which has no business in a paint.
    output_devices: Vec<output::Device>,
    /// The shared thumbnail service, whose durable store the storage
    /// page sizes and clears.
    thumbs: Entity<Thumbs>,
    /// The workspace's scrobbler, the Integration page's subject: the api
    /// credential edits, the connect flow, and the knobs all go through
    /// it, and it persists them.
    scrobbler: Entity<Scrobbler>,
    /// The workspace's ListenBrainz publisher, the second half of the
    /// Integration page's scrobbling: the token and the status readout
    /// go through it.
    listenbrainz: Entity<ListenBrainz>,
    /// The Libre.fm publisher, the third: its connect flow.
    librefm: Entity<LibreFm>,
    discord: Entity<DiscordPresence>,
    discord_enabled: bool,
    discord_show_lastfm_button: bool,
    discord_show_youtube_button: bool,
    discord_status_line: DiscordStatusLine,
    /// The two card lines, written through per keystroke like the capture
    /// pattern, with presence re-reading them on every write so the card
    /// follows the typing.
    discord_first_line: Entity<InputState>,
    discord_second_line: Entity<InputState>,
    discord_hover_line: Entity<InputState>,
    /// The api credential inputs; edits write through to the scrobbler per
    /// keystroke, the pickers' cadence.
    lastfm_key: Entity<InputState>,
    lastfm_secret: Entity<InputState>,
    /// The ListenBrainz token input. Unlike the Last.fm pair this doesn't
    /// write through per keystroke: storing a token starts a check
    /// against the service, so it waits for Connect.
    listenbrainz_token: Entity<InputState>,
    /// The Icecast section's connection fields, seeded from the file and
    /// written through per keystroke. The sink itself re-applies on blur or
    /// enter, never per keystroke, so typing a host doesn't dial half names.
    broadcast_host: Entity<InputState>,
    broadcast_port: Entity<InputState>,
    broadcast_mount: Entity<InputState>,
    broadcast_user: Entity<InputState>,
    broadcast_password: Entity<InputState>,
    broadcast_name: Entity<InputState>,
    /// The switch and the bitrate, copied from settings so the section
    /// renders without re-reading the file.
    broadcast_enabled: bool,
    broadcast_bitrate: u32,
    /// Whether an Icecast field has been typed into since the sink last
    /// re-applied. Set on the keystroke, cleared by the commit, so the
    /// commit can be run from anywhere the edit ends rather than only from
    /// a blur, and so a blur through a field nobody touched doesn't tear
    /// down a live connection.
    broadcast_dirty: bool,
    /// The capture switch and the folder it writes into, copied from
    /// settings so the Playback page renders without re-reading the file.
    capture_enabled: bool,
    capture_folder: PathBuf,
    /// The pattern a saved song is named by, written through per
    /// keystroke like the fields above it.
    capture_pattern: Entity<InputState>,
    capture_album: Entity<InputState>,
    /// One block per Subsonic server, in the order accounts.json lists
    /// them, so a block's position is its account's index there. Seeded at
    /// open and changed only by Add Server and Remove Server.
    subsonic: Vec<subsonic::SubsonicForm>,
    /// The id the next block gets, see [`subsonic::SubsonicForm`].
    subsonic_next_id: u64,
    /// Whether a sync is in flight on any server. One runs at a time, so
    /// every Sync Now reads as busy while it does, and a poll keeps the
    /// album count on the syncing server's line moving.
    subsonic_syncing: bool,
    /// The server whose setup dialog is open, by its block's id.
    subsonic_editing: Option<u64>,
    /// The prune and rebuild a left field, a switch or a removal kicks off.
    /// Held so the work survives the callback that started it, and so a
    /// second one replaces the first rather than racing it.
    subsonic_follow: Option<Task<()>>,
    /// The radio stations the library holds, re-read at open and on every
    /// catalog write rather than per frame. The Radio page lists them
    /// and is where they're added, imported and removed.
    stations: Vec<Station>,
    /// The two fields of the station add row. The URL is the identity, so
    /// it's the only one that has to be filled in.
    station_url: Entity<InputState>,
    station_name: Entity<InputState>,
    /// Why the last add or import did nothing, shown under the add row. A
    /// pasted line that isn't a stream is the common case, and failing
    /// silently reads as the button being broken. It also carries the
    /// "checking" line while an add is out at the stream, since that's
    /// the same place the eye is already looking.
    station_notice: Option<SharedString>,
    /// Whether an add is waiting on what the URL serves. The button is
    /// held while it is: the round trip is a second at worst, and a
    /// second press would put the same station in twice.
    station_probing: bool,
    /// The ffmpeg path input; writes through like the credentials, and the
    /// probe is keyed by value, so a pasted path shows Convert everywhere
    /// without a restart.
    ffmpeg_path: Entity<InputState>,
    /// What the last press of the Test button learned: the version line the
    /// binary returned, or why it didn't. An edit to the path clears
    /// it, so the callout never describes a binary the input has moved past.
    ffmpeg_test: Option<Result<String, String>>,
    threshold_scrub: ScrubState,
    /// The storage page's numbers; None until the first walk finishes.
    storage: Option<StorageInfo>,
    /// Whether a measurement is out on the background executor, so the
    /// things that ask for fresh numbers can ask as often as they like
    /// without stacking walks over the same files.
    storage_measuring: bool,
    /// Whether something asked for numbers while that walk was out, which
    /// means the numbers it brings back are already behind and one more
    /// walk has to follow it.
    storage_remeasure: bool,
    /// The folder list with per-folder rollups, recounted on every
    /// library event rather than per frame.
    root_stats: Vec<(PathBuf, Stats)>,
    /// What the library has to level by, split into tagged, measured, and
    /// missing. Counted alongside the rollups above, for the same reason: a
    /// COUNT over the catalog has no business in a paint.
    rg_coverage: GainCoverage,
    /// The running measurement pass, while one runs, so the Leveling
    /// section can show its count and offer the stop. Polled on a timer the
    /// way the scan badge is; the pass is app-global, so closing this
    /// window leaves it measuring.
    rg_job: Option<Arc<replaygain_job::Progress>>,
    /// The Workspace page's save-current-as-preset name field.
    layout_name: Entity<InputState>,
    /// The Workspace page's save-current-as-workspace name field.
    workspace_name: Entity<InputState>,
    /// The workspace whose card is open on the Workspace page, with an input
    /// per editable line. None while every row is collapsed, which is how
    /// the page opens.
    workspace_card: Option<workspace_page::CardEditor>,
    /// Who made each saved workspace, by name, for the credit line under a
    /// list row. Read once here rather than per render: the saved list is a
    /// directory read, and pulling an author out means parsing a bundle's
    /// worth of layout dumps. Refreshed by the page's own writes,
    /// which are the only thing that moves it while the window is up.
    workspace_authors: BTreeMap<String, String>,
    /// The mini-player roles the Workspace page assigns, by preset name, kept
    /// beside the settings file so the badges reflect edits without a
    /// reload; pushed back to the workspace so its button follows along.
    primary_layout: Option<String>,
    mini_layout: Option<String>,
    /// The confirm dialog waiting on the user, if any: an overwrite or a
    /// workspace apply. None when no dialog is up.
    pending: Option<Pending>,
    /// The keyboard's home while a dialog is up, the confirm and the pass
    /// prompt both. A key event only reaches listeners along the path to
    /// whatever has focus, so a dialog that wants Enter and Escape has to
    /// hold it.
    dialog_focus: FocusHandle,
    /// The window's own focus, claimed on open. Not a tab stop itself, so
    /// the first Tab moves to the first control on the page; it exists
    /// because a window with focus nowhere gets no keys at all, which is
    /// what kept Tab and the search shortcut from working here.
    focus: FocusHandle,
    /// The chords moved off their defaults, copied from the file so the
    /// Keymap page doesn't load settings per render. Every edit on that
    /// page writes the file and re-reads this.
    keymap: BTreeMap<String, Vec<String>>,
    /// The override map as it stood before the last reset, row or all,
    /// what the Keymap page's Undo puts back. One level deep, cleared by
    /// any other keymap edit so it never resurrects a stale map. Dies
    /// with the window, which is as long as an accidental reset takes
    /// to notice.
    keymap_undo: Option<BTreeMap<String, Vec<String>>>,
    /// The command whose next keystroke the Keymap page is waiting for,
    /// while a row is recording. The interceptor below reads it to decide
    /// whether to swallow a press.
    recording: Option<&'static str>,
    /// Whether launch runs the daily update check, the Application page toggle.
    check_updates: bool,
    prerelease_updates: bool,
    /// Whether a check that finds a newer release also downloads it, the
    /// row under the check toggle. Only shown where the install can
    /// replace itself.
    download_updates: bool,
    /// Whether anything of rox talks to AI tooling, the toggle at the top
    /// of the Application page. Also gates whether the MCP and ML Models
    /// pages show in the sidebar.
    ai_enabled: bool,
    /// Whether the MCP surface serves tool calls, the MCP page's own
    /// toggle under the AI gate above.
    mcp_enabled: bool,
    /// Whether the experimental panels show in the panel menus, the
    /// Development page toggle.
    experimental: bool,
    /// Whether the library may build acoustic vectors, the Library page's
    /// acoustic switch.
    acoustic_analysis: bool,
    /// Whether the analysis pass follows the watcher, the row under the
    /// acoustic switch.
    acoustic_auto: bool,
    /// Whether the library may work out how fast its tracks run, the
    /// Library page's tempo switch.
    tempo_analysis: bool,
    /// Whether the tempo pass follows the watcher, the row under the
    /// tempo switch.
    tempo_auto: bool,
    /// Where the analysis pass puts its vectors, the row under the switch.
    acoustic_save: AcousticSave,
    /// The start prompt for a long pass, while it's up. It owns the worker
    /// slider, the estimate, and the start itself; the section buttons only
    /// raise it, and the tasks window raises the same one.
    prompt: Option<pass_prompt::Prompt>,
    /// How many tracks each pass works on at once, copied from settings so
    /// the coverage notes can price a pass per render without re-reading the
    /// file. The prompt's slider moves them.
    acoustic_workers: usize,
    rg_workers: usize,
    tempo_workers: usize,
    /// What the last acoustic pass measured on this machine, worker-seconds
    /// per track by model id, copied from the session file so the coverage
    /// note can price a pass per render without re-reading it. Refreshed
    /// when a pass ends, which is the only time it changes.
    acoustic_pace: std::collections::HashMap<String, f32>,
    /// The same for ReplayGain measurement, seconds per track. Zero until a
    /// pass has measured one.
    rg_pace: f32,
    /// The same for the tempo pass. One number rather than a map, since
    /// there's no model behind it to key by.
    tempo_pace: f32,
    /// How much of the library the acoustic pass has described, counted
    /// alongside the rollups above rather than in a paint.
    acoustic_coverage: rox_library::embeddings::Coverage,
    /// The running acoustic pass, while one runs. Polled like `rg_job`, and
    /// app-global for the same reason: closing this window leaves it going.
    acoustic_job: Option<Arc<rox_acoustic::Progress>>,
    /// The library's tempo split, counted alongside the rollups above
    /// rather than in a paint.
    bpm_coverage: BpmCoverage,
    /// The running tempo pass, while one runs. App-global like the other
    /// two: closing this window leaves it going.
    tempo_job: Option<Arc<tempo_job::Progress>>,
    /// Which extractor the pass runs and the similarity queries read, the
    /// Library page's switch. Copies the live pick; the coverage above is
    /// counted against whatever this names.
    acoustic_source: rox_acoustic::Source,
    /// The model the ML Models page has marked as the one to use, which the
    /// Library page's extractor switch turns on. Separate from the field
    /// above because that one is the extractor the library runs right now:
    /// the two differ whenever the switch is set to the built-in extractor.
    acoustic_ml_source: rox_acoustic::Source,
    /// Which half of a model category is showing: the ones rox recommends
    /// and can fetch, or the file the user supplies. A view state rather
    /// than a setting, so flipping it to look at the other half doesn't
    /// change what the library runs.
    models_kind: ModelKind,
    /// The weights file the user pointed at, if any, and why the last pick
    /// was refused. The error is kept here rather than in a log because a
    /// file that isn't this network is the ordinary outcome of browsing to
    /// the wrong `.safetensors`, and the reason belongs on the row that
    /// caused it.
    acoustic_local: Option<settings::LocalModel>,
    acoustic_local_error: Option<String>,
    /// Whether a picked file is being hashed and loaded. It's a 25 MB read
    /// and a forward pass, so the row shows it's working rather than sitting
    /// still for a second.
    acoustic_local_checking: bool,
    /// The running model download, while one runs. Polled on the same timer
    /// as the pass, and app-global for the same reason.
    model_job: Option<Arc<rox_acoustic::models::Progress>>,
    /// What each catalog model weighs on disk, and whether it's installed at
    /// all. Measured entering the page and after a download or a delete
    /// rather than per frame: a stat per model per paint is a syscall per
    /// model per paint.
    model_sizes: Vec<(&'static str, u64)>,
    /// The running dictionary download, while one runs. Its own field
    /// rather than a second arm on `model_job`: the two are different
    /// downloads of different things for different jobs, and one Stop
    /// button that could cancel either would be a bug waiting to happen.
    dictionary_job: Option<Arc<rox_romanize::dictionary::Progress>>,
    /// The stored language pick, copied from settings: None is System, and
    /// the row marks its segment without re-reading the settings file per
    /// render.
    language: Option<String>,
    /// The screen shader's file and all-windows option, copied from
    /// settings so the Shader page doesn't re-read the file per
    /// render. The enable switch isn't copied: the hotkey and menu row
    /// flip it from outside this window, so the row reads the workspace's
    /// live static, like the menubar toggle does. The compile error reads
    /// the workspace's live readout the same way, since the hot reload
    /// rewrites it without notifying this window.
    post_shader_path: Option<PathBuf>,
    /// The pool name and inline source, copied beside the path so the
    /// picker can show which entry the config names without a settings
    /// load per render.
    post_shader_name: Option<String>,
    post_shader_source: String,
    /// The apply generation every copied field below was seeded from. Render
    /// re-seeds them when the workspace's counter moves past it, the way
    /// the palette editor follows a theme switch: a workspace apply
    /// replaces the whole shader config from outside this window.
    post_shader_gen: u64,
    /// The name field of the picker's save block, the panel pages' shape.
    post_shader_save_name: ShaderNameField,
    post_shader_all_windows: bool,
    post_shader_run_idle: bool,
    /// The screen shader's routes, copied for the same reason the path
    /// is: the section renders per keystroke under search and the settings
    /// file contains the dock dumps. Edits write here, into the workspace's
    /// live feed, and into the file on a debounce.
    post_shader_routes: Vec<Route>,
    /// The route editor's span sliders and fold state, kept in step with
    /// the list above on every render.
    post_shader_route_ui: RouteEditState,
    /// The screen shader's hand-set slot values, copied like the routes
    /// and written through the same three layers: this copy, the
    /// workspace's live feed, the file on a debounce.
    post_shader_manual: Vec<(u8, f32)>,
    /// One scrub state per slot for the hand-set sliders.
    post_shader_slot_scrubs: Vec<panel::ScrubState>,
    /// The Backdrop section's editor state. The config itself is stored in
    /// the look's bundle behind a cache the section reads per render, so
    /// only what has to persist across a render is kept here: the route
    /// editor's folds, the slot scrubs, the save field, and the write
    /// debounces.
    backdrop_route_ui: RouteEditState,
    backdrop_slot_scrubs: Vec<panel::ScrubState>,
    backdrop_save_name: ShaderNameField,
    backdrop_route_persist_gen: u64,
    backdrop_manual_persist_gen: u64,
    /// The route write's own debounce generation, kept apart from the
    /// appearance one so neither burst cancels the other's write.
    route_persist_gen: u64,
    /// And the hand-set write's own, apart from both for the same reason.
    manual_persist_gen: u64,
    /// Bumped on every appearance-slider tick; a debounced writer flushes the
    /// current values once the scrub settles instead of rewriting the whole
    /// settings file per tick.
    persist_gen: u64,
    /// Whether the debounced appearance write includes the palette map too.
    /// Picker edits set it; reset clears it, since stock persists as an
    /// empty map that a later write must not refill with explicit defaults.
    persist_palette: bool,
    _picker_changes: Vec<Subscription>,
    _lastfm_changes: Vec<Subscription>,
    _broadcast_changes: Vec<Subscription>,
    _ffmpeg_changed: Subscription,
    _capture_pattern_changed: Subscription,
    _discord_line_changes: Vec<Subscription>,
    _capture_album_changed: Subscription,
    _acoustid_key_changed: Subscription,
    /// The connect flow's phases arrive through here, so the page's status
    /// line updates with them.
    _scrobbler_changed: Subscription,
    /// The token check's result arrives through here, so the status line
    /// follows it.
    _listenbrainz_changed: Subscription,
    _librefm_changed: Subscription,
    _library_changed: Subscription,
    /// Scan progress ticks notify the library without emitting Updated;
    /// the Library page's busy line needs those repaints too.
    _library_repaint: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
    /// The Workspace page's tree follows the dock: layout events catch
    /// drags and resizes, the observe catches an import's set_center,
    /// which notifies without an event.
    _dock_changes: Vec<Subscription>,
    _search_changes: Subscription,
    /// The Output readout has to follow a rebuild it didn't ask for, a
    /// device dropping out or the rate follow reopening the stream. Gated
    /// on the output state alone, since a playing session notifies sixty
    /// times a second for a clock this window never draws.
    _player_changed: Subscription,
    /// The mode rows follow the transport buttons: shuffle, continuation and
    /// the crossfade length are all things this window draws and the strip
    /// can change underneath it. Gated on [`rox_services::player::PlayerView`], so
    /// it wakes on the press and not on the position clock.
    _player_view: Subscription,
    /// Catches a keystroke for a recording Keymap row before the keymap
    /// gets to resolve it. Live for the window's lifetime and gated on
    /// `recording`, rather than subscribed per record: dropping a
    /// subscription from inside its own callback is not a thing to do.
    _record_keys: Subscription,
    /// Commits the fields on the way out. The window can be closed, or the
    /// app quit, with the caret still in a field that never blurred, and
    /// the entity takes its pending commit down with it. See
    /// [`SettingsWindow::flush_pending_edits`].
    _flush_on_close: Subscription,
    _flush_on_quit: Subscription,
}

impl SettingsWindow {
    fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        workspace_window: AnyWindowHandle,
        dock: Entity<rox_dock::DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let player = state.player.entity_id();
        // The bundle taken whole before the pieces below move out of it.
        // Cloning shares the handles, and the windows this window opens
        // want the bundle rather than a field of it.
        let app_state = state.clone();
        // Claimed here so the window has the keyboard from the moment it
        // opens, the workspace window's move.
        let focus = cx.focus_handle();
        window.focus(&focus);
        let playback = state.player;
        let _player_changed = rox_services::player::observe_output(&playback, cx);
        let _player_view = rox_services::player::observe_view(&playback, cx);
        // Off the player rather than the file: it holds the live copy, and a
        // toggle flipped here has to match the session it rebuilds.
        let output_exclusive = playback.read(cx).exclusive_output();
        let output_devices = output::devices(output_mode(output_exclusive));
        let library = state.library;
        let settings = Settings::load();
        let editor_mode = palette::mode();
        let base = match editor_mode {
            palette::Mode::Dark => settings.palette_dark(),
            palette::Mode::Light => settings.palette_light(),
        };
        // The folders show as soon as the window does; their rollups land
        // when the measure comes back.
        let root_stats = seed_root_stats(&library, cx);
        Self::measure_root_stats(&library, cx);
        let rg_coverage = library.read(cx).replaygain_breakdown();
        // A pass started from an earlier settings window may still be
        // running; pick it up rather than showing the button as idle.
        let rg_job = replaygain_job::progress(cx);
        if rg_job.is_some() {
            Self::poll_measuring(cx);
        }
        let acoustic_source = rox_services::acoustic::acoustic_source();
        let acoustic_ml_source = rox_services::acoustic::acoustic_ml_source();
        let acoustic_coverage = library.read(cx).acoustic_coverage(acoustic_source.id());
        let acoustic_job = embeddings::progress(cx);
        let model_job = embeddings::models::progress(cx);
        if acoustic_job.is_some() || model_job.is_some() {
            Self::poll_analyzing(cx);
        }
        let dictionary_job = crate::romanize_job::dictionary::progress(cx);
        if dictionary_job.is_some() {
            Self::poll_dictionary(cx);
        }
        let bpm_coverage = library.read(cx).bpm_breakdown();
        let tempo_job = tempo_job::progress(cx);
        if tempo_job.is_some() {
            Self::poll_timing(cx);
        }
        let _library_changed = cx.subscribe(
            &library,
            |this: &mut Self, library, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                if this.root_stats.len() != library.read(cx).roots().len() {
                    this.root_stats = seed_root_stats(&library, cx);
                }
                Self::measure_root_stats(&library, cx);
                // A scan and a finished measurement pass both fill the
                // ReplayGain columns in, so the Audio page's coverage line
                // moves with either.
                this.rg_coverage = library.read(cx).replaygain_breakdown();
                // A scan and a finished tempo pass both fill the bpm column
                // in, so the Library page's tempo line moves with either.
                this.bpm_coverage = library.read(cx).bpm_breakdown();
                // A finished scan moves the storage numbers too; remeasure
                // if they're on screen.
                if this.page == Page::Storage {
                    this.refresh_storage(cx);
                }
                // Stations are rows in the same catalog, so a write from
                // anywhere else (the panel's import, a sync) moves this
                // list too.
                this.stations = read_stations(&library, cx);
                cx.notify();
            },
        );
        let _library_repaint = cx.observe(&library, |_, _, cx| cx.notify());
        let _record_keys = Self::record_keys(window, cx);
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // The OS close button never runs a teardown of ours, so save the
        // frame through the should-close hook, the stats window's move.
        window.on_window_should_close(cx, move |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                s.windows.settings = Some(LayoutSize {
                    width: frame.size.width.into(),
                    height: frame.size.height.into(),
                });
            });
            true
        });
        // The window can go away with the caret still in a field: the OS
        // close button, rox::CloseWindow (which removes the window without
        // ever asking should-close) and a quit all do it. Releasing the
        // entity is the one point every one of those passes through, so the
        // commit runs from there. A detached task is fine here because the
        // app is still up and still pumping its executor.
        let weak = cx.weak_entity();
        let _flush_on_close = cx.on_release({
            let this = weak.clone();
            move |window: &mut Self, cx: &mut App| window.flush_pending_edits(this, cx).detach()
        });
        // Quitting drops the window too, but it gets there before the
        // release above: `App::shutdown` runs the quit hooks first and only
        // then clears the windows. So the flush happens here, while there
        // is still an app to act on.
        let _flush_on_quit = cx.on_app_quit({
            let this = weak.clone();
            move |window: &mut Self, cx: &mut Context<Self>| {
                // The Subsonic prune is dropped rather than handed back to
                // be awaited, because shutdown can't finish it: gpui parks
                // the main thread on the quit futures and never pumps the
                // foreground executor the prune is spawned on, so the whole
                // 100 ms budget goes to a task that gets no poll at all.
                // Measured under the harness, not reasoned. The rows wait
                // for the next Sync Now, which is where they sat before.
                drop(window.flush_pending_edits(this.clone(), cx));

                async {}
            }
        });
        // Subscribe to the dock handed in rather than reading it off the
        // workspace: this constructor runs inside the workspace update
        // that opened the window, so the workspace entity can't be read
        // here. Subscribing never reads.
        let _dock_changes = vec![
            cx.subscribe(&dock, |_, _, event: &DockEvent, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    cx.notify();
                }
            }),
            cx.observe(&dock, |_, _, cx| cx.notify()),
        ];
        let _scrobbler_changed = cx.observe(&state.scrobbler, |_, _, cx| cx.notify());
        let _listenbrainz_changed = cx.observe(&state.listenbrainz, |_, _, cx| cx.notify());
        let _librefm_changed = cx.observe(&state.librefm, |_, _, cx| cx.notify());
        let listenbrainz_token = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!(
                    "settings-integrations-listenbrainz-token-placeholder"
                ))
                .masked(true)
                .default_value(settings.accounts.listenbrainz.token.clone())
        });
        // The credential inputs seed from the file and write through the
        // scrobbler per keystroke, so a paste is connected-ready with no
        // save step.
        let lastfm_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-integrations-lastfm-key-placeholder"))
                .default_value(settings.accounts.lastfm.api_key.clone())
        });
        let lastfm_secret = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!(
                    "settings-integrations-lastfm-secret-placeholder"
                ))
                .masked(true)
                .default_value(settings.accounts.lastfm.api_secret.clone())
        });
        let scrobbler = state.scrobbler.clone();
        let mut _lastfm_changes = Vec::with_capacity(2);
        for (input, apply) in [
            (
                &lastfm_key,
                (|s: &mut Scrobbler, value, cx: &mut gpui::Context<Scrobbler>| {
                    s.set_api_key(value, cx)
                }) as fn(&mut Scrobbler, String, &mut gpui::Context<Scrobbler>),
            ),
            (&lastfm_secret, |s, value, cx| s.set_api_secret(value, cx)),
        ] {
            _lastfm_changes.push(cx.subscribe(input, {
                let scrobbler = scrobbler.clone();
                move |_, input, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        let value = input.read(cx).value().trim().to_string();
                        scrobbler.update(cx, |s, cx| apply(s, value, cx));
                    }
                }
            }));
        }
        // The broadcast fields write through per keystroke like the
        // Last.fm pair; the sink only re-applies when a field is left
        // (blur or enter), so a host mid-type never gets dialed.
        let broadcast_host = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-audio-broadcast-host-placeholder"))
                .default_value(settings.broadcast.host.clone())
        });
        let broadcast_port = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("8000")
                .default_value(settings.broadcast.port.to_string())
        });
        let broadcast_mount = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("/rox")
                .default_value(settings.broadcast.mount.clone())
        });
        let broadcast_user = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-audio-broadcast-user-placeholder"))
                .default_value(settings.broadcast.user.clone())
        });
        let broadcast_password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!(
                    "settings-audio-broadcast-password-placeholder"
                ))
                .masked(true)
                .default_value(settings.broadcast.password.clone())
        });
        let broadcast_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-audio-broadcast-name-placeholder"))
                .default_value(settings.broadcast.name.clone())
        });
        let mut _broadcast_changes = Vec::with_capacity(6);
        for (input, write) in [
            (
                &broadcast_host,
                (|s: &mut Settings, value: String| s.broadcast.host = value)
                    as fn(&mut Settings, String),
            ),
            (&broadcast_port, |s, value| {
                // A half-typed or emptied port falls back to icecast's
                // stock one rather than writing a lie.
                s.broadcast.port = value.parse().unwrap_or(8000);
            }),
            (&broadcast_mount, |s, value| s.broadcast.mount = value),
            (&broadcast_user, |s, value| s.broadcast.user = value),
            (&broadcast_password, |s, value| s.broadcast.password = value),
            (&broadcast_name, |s, value| s.broadcast.name = value),
        ] {
            _broadcast_changes.push(cx.subscribe(input, {
                move |this: &mut Self, input, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        let value = input.read(cx).value().trim().to_string();
                        Settings::update(move |s| write(s, value));
                        this.broadcast_dirty = true;
                    }

                    // Leaving the field is the commit. The same one runs
                    // from the flush, for a field the user never left.
                    InputEvent::Blur | InputEvent::PressEnter { .. } => this.broadcast_moved(),

                    InputEvent::Focus => {}
                }
            }));
        }
        // One block per Subsonic server, its fields writing through into
        // accounts.json rather than the settings file, since a server
        // password is a real credential. Nothing dials on a keystroke:
        // Connect and Sync Now are both a round trip to someone's server,
        // so both wait to be asked.
        let subsonic: Vec<subsonic::SubsonicForm> = settings
            .accounts
            .subsonic_servers
            .iter()
            .enumerate()
            .map(|(id, account)| {
                subsonic::SubsonicForm::new(id as u64, account, &library, window, cx)
            })
            .collect();
        let subsonic_next_id = subsonic.len() as u64;

        // The station add row. Nothing writes through on a keystroke here:
        // a station lands in the library on the Add press, so a half-typed
        // URL never becomes a row.
        let station_url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-sources-stations-url-placeholder"))
        });
        let station_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-sources-stations-name-placeholder"))
        });
        let stations = read_stations(&library, cx);

        // The ffmpeg path takes the same per-keystroke write-through, and
        // since the probe caches per value, a path that resolves flips the
        // Convert surfaces on with no restart.
        let ffmpeg_path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("ffmpeg")
                .default_value(settings.convert.ffmpeg.clone())
        });
        let _ffmpeg_changed = cx.subscribe(&ffmpeg_path, |this, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let value = input.read(cx).value().trim().to_string();
                Settings::update(move |s| s.convert.ffmpeg = value);
                this.ffmpeg_test = None;
                cx.notify();
            }
        });
        // The Discord card's two lines, write-through per keystroke like
        // the capture pattern below, plus a reload so the card on someone
        // else's screen follows the typing rather than the next track.
        let discord_first_line = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_PRESENCE_FIRST_LINE)
                .default_value(settings.accounts.discord.first_line.clone())
        });
        let discord_second_line = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_PRESENCE_SECOND_LINE)
                .default_value(settings.accounts.discord.second_line.clone())
        });
        let discord_hover_line = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_PRESENCE_HOVER)
                .default_value(settings.accounts.discord.hover_line.clone())
        });
        let _discord_line_changes = vec![
            cx.subscribe(
                &discord_first_line,
                |this: &mut Self, input, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        let value = input.read(cx).value().trim().to_string();
                        Settings::update(move |s| s.accounts.discord.first_line = value);
                        this.discord.update(cx, |d, cx| d.reload_config(cx));
                        cx.notify();
                    }
                },
            ),
            cx.subscribe(
                &discord_second_line,
                |this: &mut Self, input, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        let value = input.read(cx).value().trim().to_string();
                        Settings::update(move |s| s.accounts.discord.second_line = value);
                        this.discord.update(cx, |d, cx| d.reload_config(cx));
                        cx.notify();
                    }
                },
            ),
            cx.subscribe(
                &discord_hover_line,
                |this: &mut Self, input, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        let value = input.read(cx).value().trim().to_string();
                        Settings::update(move |s| s.accounts.discord.hover_line = value);
                        this.discord.update(cx, |d, cx| d.reload_config(cx));
                        cx.notify();
                    }
                },
            ),
        ];
        // The capture pattern writes through the same way. Nothing
        // re-applies on it: the service reads the pattern when a song
        // finishes, so the next one saved is already named by whatever is
        // in the box.
        let capture_pattern = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(settings::DEFAULT_CAPTURE_PATTERN)
                .default_value(settings.capture.pattern.clone())
        });
        let _capture_pattern_changed =
            cx.subscribe(&capture_pattern, |_, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let value = input.read(cx).value().trim().to_string();
                    Settings::update(move |s| s.capture.pattern = value);
                    cx.notify();
                }
            });
        // The album a capture is tagged with, same write-through. Blank is
        // the default and means no album tag at all.
        let capture_album = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-playback-capture-album-placeholder"))
                .default_value(settings.capture.album.clone())
        });
        let _capture_album_changed =
            cx.subscribe(&capture_album, |_, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let value = input.read(cx).value().trim().to_string();
                    Settings::update(move |s| s.capture.album = value);
                    cx.notify();
                }
            });
        // The AcoustID key takes the same write-through, into the
        // working copy as well as the file, so a toggle that saves the
        // whole providers struct after doesn't put the old key back.
        let acoustid_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-providers-acoustid-key-placeholder"))
                .default_value(settings.accounts.providers.acoustid_key.clone())
        });
        let _acoustid_key_changed =
            cx.subscribe(&acoustid_key, |this, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let value = input.read(cx).value().trim().to_string();
                    this.providers.acoustid_key = value.clone();
                    Settings::update(move |s| s.accounts.providers.acoustid_key = value);
                    cx.notify();
                }
            });
        // The search box up top: typing filters every page at once. The
        // first search measures storage so the Storage rows have numbers
        // without a page visit; after that the numbers stay as they are
        // until the page's own refresh paths run.
        let search = cx.new(|cx| {
            SearchBox::new(rox_i18n::t!("query-search"), "", window, cx)
                .small()
                .icon()
        });
        let _search_changes = cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, search, event, window, cx| match event {
                SearchEvent::Changed => {
                    if !search.read(cx).query().trim().is_empty() && this.storage.is_none() {
                        this.refresh_storage(cx);
                    }
                    cx.notify();
                }
                // Escape on an empty box: nothing else here takes focus,
                // so the way out is plain blur.
                SearchEvent::Dismissed => {
                    window.blur();
                    cx.notify();
                }
                SearchEvent::Submitted | SearchEvent::FocusChanged => {}
            },
        );
        // The Frame rows open split where a knob's sides already differ.
        let appearance_frame = settings.look.bundle.appearance.frame;
        let mut pickers = Vec::with_capacity(ROLES.len());
        let mut _picker_changes = Vec::with_capacity(ROLES.len());
        for (index, role) in ROLES.iter().enumerate() {
            let picker =
                cx.new(|cx| ColorPickerState::new(window, cx).default_value((role.get)(&base)));
            _picker_changes.push(cx.subscribe_in(
                &picker,
                window,
                move |this, picker, event: &ColorPickerEvent, window, cx| {
                    let ColorPickerEvent::Change(color) = event;
                    this.role_edited(index, *color, picker, window, cx);
                },
            ));
            pickers.push(picker);
        }
        SettingsWindow {
            page: Page::Appearance,
            search,
            search_scoped: false,
            base,
            editor_mode,
            keep_theme: settings.look.bundle.appearance.keep_theme,
            surface_opacity: settings.look.bundle.appearance.surface_opacity,
            backdrop_strength: settings.look.bundle.appearance.backdrop_strength,
            backdrop_all_windows: settings.look.bundle.appearance.backdrop_all_windows,
            backdrop_visual_strength_scrub: ScrubState::default(),
            backdrop_visual_scale_scrub: ScrubState::default(),
            backdrop_visual_duration_scrub: ScrubState::default(),
            backdrop_visual_fps_scrub: ScrubState::default(),
            backdrop_visual_sensitivity_scrub: ScrubState::default(),
            backdrop_visual_fade_scrub: ScrubState::default(),
            backdrop_visual_persist_gen: 0,
            font_size: settings.app_font_size,
            frame: appearance_frame,
            restore_last_track: settings.restore_last_track,
            watch_library: settings.watch_library,
            fold_case: settings.fold_case,
            split_genre_compounds: settings.split_genre_compounds,
            split_genre_compounds_at_open: settings.split_genre_compounds,
            portable: settings::portable_marker().is_some_and(|marker| marker.exists()),
            portable_writable: settings::portable_available(),
            portable_busy: false,
            rating_style: settings.look.bundle.appearance.rating_style,
            rating_dots: settings.look.bundle.appearance.rating_dots,
            providers: settings.accounts.providers.clone(),
            acoustid_key,
            pickers,
            surface_scrub: ScrubState::default(),
            backdrop_scrub: ScrubState::default(),
            font_size_scrub: ScrubState::default(),
            margin_scrub: SidesScrub::default(),
            padding_scrub: SidesScrub::default(),
            rounding_scrub: ScrubState::default(),
            border_scrub: SidesScrub::default(),
            margin_split: appearance_frame.margin.uniform().is_none(),
            padding_split: appearance_frame.padding.uniform().is_none(),
            border_split: appearance_frame.border.uniform().is_none(),
            value_edit: panel::ValueEdit::default(),
            scroll: ScrollHandle::new(),
            nav_scroll: ScrollHandle::new(),
            library,
            state: app_state,
            signals: state.signals,
            workspace,
            workspace_window,
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            player,
            crossfade_scrub: ScrubState::default(),
            step_scrub: ScrubState::default(),
            step_preview_scrub: ScrubState::default(),
            live_buffer_scrub: ScrubState::default(),
            preamp_scrub: ScrubState::default(),
            fallback_scrub: ScrubState::default(),
            output_exclusive,
            output_devices,
            playback,
            thumbs: state.thumbs,
            scrobbler,
            listenbrainz: state.listenbrainz.clone(),
            librefm: state.librefm.clone(),
            discord: state.discord.clone(),
            discord_enabled: settings.accounts.discord.enabled,
            discord_show_lastfm_button: settings.accounts.discord.show_lastfm_button,
            discord_show_youtube_button: settings.accounts.discord.show_youtube_button,
            discord_status_line: settings.accounts.discord.status_line,
            discord_first_line,
            discord_second_line,
            discord_hover_line,
            lastfm_key,
            lastfm_secret,
            listenbrainz_token,
            broadcast_host,
            broadcast_port,
            broadcast_mount,
            broadcast_user,
            broadcast_password,
            broadcast_name,
            broadcast_enabled: settings.broadcast.enabled,
            broadcast_bitrate: settings.broadcast.bitrate,
            broadcast_dirty: false,
            capture_enabled: settings.capture.enabled,
            capture_folder: settings.capture.folder.clone(),
            capture_pattern,
            capture_album,
            subsonic,
            subsonic_next_id,
            subsonic_syncing: rox_services::sources::syncing(),
            subsonic_editing: None,
            subsonic_follow: None,
            stations,
            station_url,
            station_name,
            station_notice: None,
            station_probing: false,
            ffmpeg_path,
            ffmpeg_test: None,
            threshold_scrub: ScrubState::default(),
            storage: None,
            storage_measuring: false,
            storage_remeasure: false,
            root_stats,
            rg_coverage,
            rg_job,
            layout_name: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(rox_i18n::t!("settings-workspace-layout-name-placeholder"))
            }),
            workspace_name: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(rox_i18n::t!("settings-workspace-name-placeholder"))
            }),
            workspace_card: None,
            workspace_authors: crate::workspaces::saved_authors(),
            primary_layout: settings.look.bundle.primary_layout.clone(),
            mini_layout: settings.look.bundle.mini_layout.clone(),
            pending: None,
            dialog_focus: cx.focus_handle(),
            focus: focus.clone(),
            check_updates: settings.check_updates,
            prerelease_updates: settings.prerelease_updates,
            download_updates: settings.download_updates,
            ai_enabled: settings.ai_enabled,
            mcp_enabled: settings.mcp_enabled,
            experimental: settings.experimental,
            acoustic_analysis: settings.acoustic_analysis,
            acoustic_auto: settings.acoustic_auto,
            tempo_analysis: settings.tempo_analysis,
            tempo_auto: settings.tempo_auto,
            acoustic_save: settings.acoustic_save,
            prompt: None,
            acoustic_workers: settings.acoustic_workers.max(1),
            rg_workers: settings.replaygain_workers.max(1),
            tempo_workers: settings.tempo_workers.max(1),
            acoustic_pace: settings.session.acoustic_pace.clone(),
            rg_pace: settings.session.replaygain_pace,
            tempo_pace: settings.session.tempo_pace,
            acoustic_coverage,
            acoustic_job,
            bpm_coverage,
            tempo_job,
            // Open on whichever half holds the model the page is offering,
            // so someone running their own file opens on it rather than on a
            // shelf that looks like nothing is picked.
            models_kind: match &settings.acoustic_local_model {
                Some(local) if local.id == acoustic_ml_source.id() => ModelKind::Custom,
                _ => ModelKind::Recommended,
            },
            acoustic_source,
            acoustic_ml_source,
            acoustic_local: settings.acoustic_local_model.clone(),
            acoustic_local_error: None,
            acoustic_local_checking: false,
            model_job,
            model_sizes: Self::measure_models(),
            dictionary_job,
            language: settings.language.clone(),
            post_shader_path: settings.post_shader.path.clone(),
            post_shader_name: settings.post_shader.name.clone(),
            post_shader_source: settings.post_shader.source.clone(),
            post_shader_gen: crate::workspace::post_shader_gen(),
            post_shader_save_name: ShaderNameField::default(),
            post_shader_all_windows: settings.post_shader.all_windows,
            post_shader_run_idle: settings.post_shader.run_when_idle,
            post_shader_routes: settings.post_shader.routes.clone(),
            post_shader_route_ui: RouteEditState::default(),
            post_shader_manual: settings.post_shader.manual.clone(),
            post_shader_slot_scrubs: (0..panel::shader::SLOTS)
                .map(|_| panel::ScrubState::default())
                .collect(),
            backdrop_route_ui: RouteEditState::default(),
            backdrop_slot_scrubs: (0..panel::shader::SLOTS)
                .map(|_| panel::ScrubState::default())
                .collect(),
            backdrop_save_name: ShaderNameField::default(),
            backdrop_route_persist_gen: 0,
            backdrop_manual_persist_gen: 0,
            route_persist_gen: 0,
            manual_persist_gen: 0,
            persist_gen: 0,
            persist_palette: false,
            keymap: settings.keymap.clone(),
            keymap_undo: None,
            recording: None,
            _picker_changes,
            _lastfm_changes,
            _broadcast_changes,
            _ffmpeg_changed,
            _capture_pattern_changed,
            _discord_line_changes,
            _capture_album_changed,
            _acoustid_key_changed,
            _scrobbler_changed,
            _listenbrainz_changed,
            _librefm_changed,
            _library_changed,
            _library_repaint,
            _backdrop_changed,
            _dock_changes,
            _search_changes,
            _player_changed,
            _player_view,
            _record_keys,
            _flush_on_close,
            _flush_on_quit,
        }
    }

    /// Run the commit every dirty field would have run on its way out.
    ///
    /// These fields write their value through on the keystroke but hold
    /// the act on it until the field is left. Closing the window,
    /// switching page and quitting all end the edit without any field ever
    /// blurring, and the held task dies with the entity, so the typing
    /// lands in the file with nothing acting on it: an Icecast port that
    /// never reaches the sink, a moved server whose old rows stay in the
    /// library. This is that same commit, run from the outside.
    ///
    /// Destructive and slow is not a reason to skip one here. Leaving the
    /// field would have pruned; so does this.
    ///
    /// The returned task is the Subsonic prune. A caller that outlives it
    /// detaches it; the quit hook drops it, for the reason spelled out
    /// there. Everything else here is synchronous and runs either way.
    fn flush_pending_edits(&mut self, this: WeakEntity<Self>, cx: &mut App) -> Task<()> {
        self.broadcast_moved();

        self.subsonic_commit(this, cx).unwrap_or(Task::ready(()))
    }

    /// What to call a model id on screen: the catalog's label, or the name of
    /// the file behind a local pick. Ids from a newer build, and local ones
    /// that have since been replaced, get the fallback.
    fn label_for(&self, id: &str, fallback: &str) -> String {
        if let Some(model) = rox_acoustic::models::find(id) {
            return model.label.to_string();
        }
        self.acoustic_local
            .as_ref()
            .filter(|local| local.id == id)
            .and_then(|local| local.path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| fallback.to_string())
    }

    /// Open a page: what a sidebar click and a result breadcrumb both
    /// do, so arriving anywhere reads the same. A window-wide search
    /// ends here, since landing on a page is leaving the results stack;
    /// a page-bound one carries over, since the sidebar is how a bound
    /// search picks the page it runs on. Entering Storage measures the
    /// files fresh, so the numbers are current without a per-frame stat.
    fn open_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        // Leaving the page is leaving the field: the broadcast and Subsonic
        // rows are gone from the screen after this, so whatever was typed
        // into them commits now or never.
        let this = cx.weak_entity();
        self.flush_pending_edits(this, cx).detach();

        self.page = page;
        if !self.search_scoped {
            self.search
                .update(cx, |search, cx| search.set_value("", window, cx));
        }
        if page == Page::Storage {
            self.refresh_storage(cx);
        }
        cx.notify();
    }

    /// Open the page a panel asked for, if one did. The request is a nav
    /// key rather than a [`Page`], since the panels are a crate below this
    /// one and the enum isn't theirs to name. A key that isn't in the
    /// sidebar leaves the window where it was.
    fn sync_requested_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = rox_panel_api::panel_settings::requested_app_page(cx) else {
            return;
        };

        let Some(&(page, ..)) = PAGES.iter().find(|&&(_, label, _)| label == key) else {
            return;
        };

        self.open_page(page, window, cx);
    }

    /// One page filtered through the query: the single-page view passes
    /// the inactive query and gets the whole page, or the live one under
    /// a page-bound search; the results stack passes the live one to
    /// every page and takes the survivors.
    fn build_page(
        &mut self,
        page: Page,
        q: &Query,
        columns: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PageBody {
        match page {
            Page::Appearance => self.appearance_page(q, columns, cx),
            Page::Audio => self.audio_page(q, cx),
            Page::Application => self.application_page(q, cx),
            Page::Integrations => self.integrations_page(q, cx),
            Page::Keymap => self.keymap_page(q, cx),
            Page::Library => self.library_page(q, cx),
            Page::Mcp => self.mcp_page(q, window, cx),
            Page::MlModels => self.ml_models_page(q, cx),
            Page::Playback => self.playback_page(q, cx),
            Page::Providers => self.providers_page(q, cx),
            Page::Shader => self.shader_page(q, window, cx),
            Page::Radio => self.radio_page(q, cx),
            Page::Storage => self.storage_page(q, cx),
            Page::Workspace => self.workspace_page(q, cx),
            Page::Development => self.development_page(q, cx),
        }
    }

    /// The results stack: every surviving page under a heading that
    /// jumps to it, in sidebar order, so a search reads as the settings
    /// laid flat. The heading centers over rules running to both edges,
    /// a level above the section headers underneath it. `pages` holds what
    /// [`Self::build_page`] kept per sidebar entry; a search that kept
    /// nothing says so instead.
    fn search_results(
        &self,
        text: &str,
        pages: Vec<(Page, &'static str, &'static str, PageBody)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if pages.iter().all(|(_, _, _, body)| body.hits() == 0) {
            return Self::no_matches(text);
        }
        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .children(
                pages
                    .into_iter()
                    .filter(|(_, _, _, body)| body.hits() > 0)
                    .map(|(page, label, icon, body)| {
                        // The hairline halves the heading centers over.
                        let rule = || div().flex_1().h(px(1.)).bg(palette::border());
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(tokens::SPACE_SM)
                                    .cursor_pointer()
                                    .text_color(palette::text_muted())
                                    .hover(|d| d.text_color(palette::text()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.open_page(page, window, cx);
                                        }),
                                    )
                                    .child(rule())
                                    .child(svg().path(icon).size(px(14.)).flex_none())
                                    .child(rox_i18n::t!(label))
                                    .child(rule()),
                            )
                            .child(body.element())
                    }),
            )
            .into_any_element()
    }

    /// What stands in for the page when a query kept nothing, at
    /// either scope.
    fn no_matches(text: &str) -> AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-search-no-matches", text = text))
            .into_any_element()
    }

    /// The button that binds the search to the open page, riding inside
    /// the search box at its tail: the transport panels' flat icon
    /// control at the menubar's glyph size. Inside the box because the
    /// sidebar is 160px wide and a neighbour would eat the query's room.
    /// The glyph goes accent while bound, the open-picker caret's cue.
    fn scope_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let color = if self.search_scoped {
            palette::accent()
        } else {
            palette::text_muted()
        };
        panel::icon_control_sized(
            icons::FUNNEL,
            px(12.),
            color,
            panel::Tip::keyed("search-scope", rox_i18n::t!("settings-search-scope")),
            |this, cx| {
                this.search_scoped = !this.search_scoped;
                cx.notify();
            },
            cx,
        )
        .flex_none()
    }

    /// A sidebar footer row: hands something to the system (the raw
    /// settings file, the data folder), so it reads quieter than the
    /// pages above.
    fn sidebar_action(
        &self,
        label: SharedString,
        icon: &'static str,
        open: fn() -> PathBuf,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_muted())
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_menu_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, _, cx| cx.open_with_system(&open())),
            )
            .child(
                svg()
                    .path(icon)
                    .size(px(12.))
                    .text_color(palette::text_muted()),
            )
            .child(label)
    }
}

/// The hover group a layout tree row forms with its controls, so the
/// controls only show while the pointer is on the row.
const TREE_ROW_GROUP: &str = "tree-row";

/// Hide a tree row control until its row is hovered, so the tree reads
/// as names at rest. The closed lock skips this in `panel_row`: it
/// shows state worth seeing without a hover.
fn reveal<E: Styled + InteractiveElement>(control: E) -> E {
    control
        .opacity(0.)
        .group_hover(TREE_ROW_GROUP, |style| style.opacity(1.))
}

/// One right-aligned numeric cell of the folder table.
/// The trailing column of a sources row: its actions, right-aligned in a
/// fixed width so every row's numbers line up whatever it can do.
fn action_cell(actions: impl IntoElement) -> Div {
    div()
        .w(ACTION_COL_W)
        .flex_none()
        .flex()
        .flex_row()
        .justify_end()
        .child(actions)
}

fn number_cell(width: Pixels, value: String) -> Div {
    div()
        .w(width)
        .flex_none()
        .text_right()
        .text_color(palette::text_muted())
        .child(value)
}

/// What the library actually has, under the section whose setting
/// depends on it. Quiet: it's context for the rows above, not a warning.
fn coverage_note(text: String) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(text)
}

/// A setting row's value in place of a control.
fn readout(value: String) -> Div {
    div().text_color(palette::text_muted()).child(value)
}

/// Bytes as a short human size, the shared formatter the metadata
/// panel's size row reads too.
fn human_size(bytes: u64) -> String {
    rox_core::fmt::fmt_bytes(bytes)
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = grid_columns(window);

        // The window renders under the workspace player's art tint and
        // claims the widget theme while it holds focus, so the pages use
        // the same colors as the app they configure. The Appearance
        // page's swatches still edit the base palette underneath; the
        // locked swatches show the derived colors through `resolved`.
        let player = self.player;
        palette::note_focus(player, window.is_window_active(), cx);

        // A theme switch moves the live palette to the other side; the
        // editor follows it here since every switch path repaints all
        // windows.
        self.sync_editor_side(window, cx);

        // Same for the app font size, which the zoom shortcuts step from
        // outside this window.
        self.sync_font_size();

        // Same for the shader config, which a workspace apply replaces
        // from outside this window; the route sync below has to run over
        // the list this brings in.
        self.sync_post_shader();

        // A panel can name the page it wants on the way in, and it gets
        // read here rather than at open so a window that was already up
        // jumps too.
        self.sync_requested_page(window, cx);

        // The Shader page builds from `&self`, so the shader route
        // editor's sliders and folds are matched to the list here, before
        // any page renders. Search builds every page each keystroke, which
        // is the other reason it can't happen down there.
        self.post_shader_route_ui
            .sync(self.post_shader_routes.len());

        // A live query builds every page and stacks the survivors; the
        // sidebar dims the pages that kept nothing. Bound to the open
        // page it builds that page alone and leaves the sidebar be, since
        // the rest was never searched. No query builds just the picked
        // page through the same path, with the inactive query keeping
        // everything.
        let text = self.search.read(cx).query().trim().to_string();
        let q = Query::parse(&text);
        let scoped = self.search_scoped;
        // The AI toggle takes the MCP and ML Models pages out of the list
        // entirely, search included: a page that isn't on offer shouldn't
        // surface its rows either.
        let pages: Vec<(Page, &str, &str)> = PAGES
            .iter()
            .copied()
            .filter(|&(page, ..)| self.ai_enabled || !matches!(page, Page::Mcp | Page::MlModels))
            .collect();
        let results: Option<Vec<_>> = (q.active() && !scoped).then(|| {
            pages
                .iter()
                .map(|&(page, label, icon)| {
                    (
                        page,
                        label,
                        icon,
                        self.build_page(page, &q, columns, window, cx),
                    )
                })
                .collect()
        });
        // The scope button rides inside the search box, so both are built
        // out here: the button needs this window's context to listen
        // against, and the box's builder only carries the box's own.
        let scope = self.scope_button(cx).into_any_element();
        let search = self
            .search
            .update(cx, |search, cx| search.element_with_suffix(Some(scope), cx));

        panel::window_body(player, || {
            let sidebar = sidebar()
                .child(
                    div()
                        // A click anywhere off the box hands focus back,
                        // the same way out the escape ladder gives; only
                        // while it holds focus, so a stray outside click
                        // never blurs some other input mid-edit.
                        .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                            if this.search.read(cx).is_focused(window, cx) {
                                window.blur();
                            }
                        }))
                        .child(search),
                )
                .child(settings_ui::nav_scroll(
                    "settings-nav",
                    &self.nav_scroll,
                    |nav| {
                        nav.children(pages.iter().enumerate().flat_map(
                            |(index, &(page, label, icon))| {
                                let empty = results
                                    .as_ref()
                                    .is_some_and(|results| results[index].3.hits() == 0);
                                // Development isn't one of the subjects, so the
                                // list breaks before it and it reads back a
                                // shade, closer to the escape hatches under it
                                // than to the pages above.
                                let apart = matches!(page, Page::Development);
                                let picked = self.page == page;
                                let pick =
                                move |this: &mut Self,
                                      window: &mut Window,
                                      cx: &mut Context<Self>| {
                                    this.open_page(page, window, cx);
                                };
                                let row = if apart {
                                    settings_ui::nav_item_quiet(
                                        rox_i18n::t!(label),
                                        icon,
                                        picked,
                                        pick,
                                        cx,
                                    )
                                } else {
                                    settings_ui::nav_item(
                                        rox_i18n::t!(label),
                                        icon,
                                        picked,
                                        pick,
                                        cx,
                                    )
                                }
                                .when(empty, |d| d.opacity(0.4));
                                apart
                                    .then(settings_ui::nav_divider)
                                    .map(IntoElement::into_any_element)
                                    .into_iter()
                                    .chain(std::iter::once(row.into_any_element()))
                            },
                        ))
                    },
                ))
                // The escape hatches sink to the bottom: the raw file this
                // window edits and the folder it's in. The nav above takes
                // the slack, so they stay against the bottom edge with no
                // spacer of their own.
                .child(self.sidebar_action(
                    rox_i18n::t!("settings-sidebar-settings-file"),
                    icons::FILE_TEXT,
                    settings_path,
                    cx,
                ))
                .child(self.sidebar_action(
                    rox_i18n::t!("settings-sidebar-data-folder"),
                    icons::FOLDER,
                    data_dir,
                    cx,
                ));

            // A bound query that kept nothing on the page says so the
            // way the results stack does.
            let page = match results {
                Some(results) => self.search_results(&text, results, cx),
                None => {
                    let body = self.build_page(self.page, &q, columns, window, cx);
                    if q.active() && body.hits() == 0 {
                        Self::no_matches(&text)
                    } else {
                        body.element()
                    }
                }
            };

            div()
                .size_full()
                .track_focus(&self.focus)
                // The settings shortcut everywhere: focus goes to the
                // search box, the Apple way in. L answers too, since that
                // is Focus Search in a workspace window and reaching for
                // it here and landing on nothing is the surprise.
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if !event.keystroke.modifiers.secondary() {
                        return;
                    }

                    if matches!(event.keystroke.key.as_str(), "f" | "l") {
                        window.focus(&this.search.read(cx).focus_handle(cx));
                    }
                }))
                .flex()
                .flex_row()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .when_some(settings::app_font(), |d, font| d.font_family(font))
                // The backdrop paints first, under the pages; without it
                // translucent surfaces would sink into the window's own
                // black instead of the playing track's art.
                .children(self.backdrop.layer(&self.now_art, window, cx))
                .child(sidebar)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .relative()
                        // The page's own surface, the window base beside the
                        // sidebar: opaque at full surface opacity so the
                        // backdrop only reads through as the surfaces thin,
                        // never at 100% like the sidebar already holds.
                        .bg(palette::bg_elevated())
                        .child(
                            div()
                                .id("settings-page")
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll)
                                .p(tokens::SPACE_MD)
                                .child(page),
                        )
                        // Fades out when idle, same as the panels. The absolute
                        // wrapper gives the scrollbar its bounds; on its own it
                        // lays out to nothing.
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&self.scroll)),
                        ),
                )
                // A Subsonic server's setup floats the same way, under the
                // confirm so Remove Server's question lands on top of it.
                .children(self.subsonic_dialog(window, cx))
                // The overwrite confirm floats over the whole window on its own
                // occluding layer, last so it paints on top of the page.
                .children(self.confirm_overlay(window, cx))
                // The pass prompt shares that layer. Only one of the two can
                // be up: nothing on a page raises both.
                .children(pass_prompt::overlay(self, window, cx))
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{PAGES, Page};
    use rox_design::assets::icons;

    /// The key each page uses in the sidebar. Exhaustive: a new variant
    /// doesn't compile until it's named here, and the checks below then
    /// hold it to the ordering.
    fn label(page: Page) -> &'static str {
        match page {
            Page::Appearance => "settings-page-appearance",
            Page::Application => "settings-page-application",
            Page::Audio => "settings-page-audio",
            Page::Keymap => "settings-page-keymap",
            Page::Integrations => "settings-page-integrations",
            Page::Library => "settings-page-library",
            Page::Mcp => "settings-page-mcp",
            Page::MlModels => "settings-page-ml-models",
            Page::Playback => "settings-page-playback",
            Page::Providers => "settings-page-providers",
            Page::Radio => "settings-page-radio",
            Page::Shader => "settings-page-shader",
            Page::Storage => "settings-page-storage",
            Page::Workspace => "settings-page-workspace",
            Page::Development => "settings-page-development",
        }
    }

    const ALL: &[Page] = &[
        Page::Appearance,
        Page::Application,
        Page::Audio,
        Page::Integrations,
        Page::Keymap,
        Page::Library,
        Page::Mcp,
        Page::MlModels,
        Page::Playback,
        Page::Providers,
        Page::Radio,
        Page::Shader,
        Page::Storage,
        Page::Workspace,
        Page::Development,
    ];

    /// Every page is in the sidebar under its own label, and the list
    /// holds nothing else: a page that exists but isn't in [`PAGES`] can
    /// only be reached by search, which is never what was meant.
    #[test]
    fn every_page_is_in_the_sidebar() {
        for &page in ALL {
            let label = label(page);
            assert!(
                PAGES.iter().any(|&(p, l, _)| p == page && l == label),
                "{label} is missing from the sidebar"
            );
        }
        assert_eq!(PAGES.len(), ALL.len(), "the sidebar has a stray entry");
    }

    /// The nav sorts A-Z, with Development pinned to the tail as the
    /// escape hatch beside the raw file and the data folder.
    #[test]
    fn the_nav_sorts_alphabetically_with_development_last() {
        let (last, sorted) = PAGES.split_last().expect("pages");
        assert_eq!(last.1, "settings-page-development");
        let labels: Vec<String> = sorted.iter().map(|&(_, l, _)| l.to_lowercase()).collect();
        let mut want = labels.clone();
        want.sort();
        assert_eq!(labels, want, "the sidebar is out of alphabetical order");
    }

    /// A panel opening this window on one of its pages dispatches the
    /// action by name, since the type is declared up in the workspace and
    /// the panels are two crates below it. The name is a string on that
    /// side, so it's held to the real one here.
    #[test]
    fn the_panels_name_the_action_that_opens_this_window() {
        use gpui::Action as _;

        assert_eq!(
            crate::workspace::OpenSettings.name(),
            rox_panel_api::panel_settings::SETTINGS_ACTION
        );
    }

    /// The shader has its own page, under the icon the panel
    /// settings window's Shader page uses.
    #[test]
    fn the_shader_page_wears_the_blend_icon() {
        let entry = PAGES
            .iter()
            .find(|&&(_, label, _)| label == "settings-page-shader")
            .expect("a Shader page");
        assert_eq!(entry.2, icons::BLEND);
    }
}
