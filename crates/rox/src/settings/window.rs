//! The settings window: one OS window opened from the menubar, a sidebar
//! of pages on the left and the picked page's sections on the right.
//! Appearance holds the song-theming switch, ADR 10's transparency pair,
//! and the palette editor, a labeled swatch grid per listing group;
//! Library manages the scanned folders over the shared catalog entity.
//! Edits apply live through the palette setters and persist to the
//! settings file per change, the volume slider's cadence. The window
//! edits a working copy of the user palette, so the swatches show the
//! base even while a playing track's seed tints the app over it; while
//! song theming is on the editor locks, because the track is driving.
//! Palettes import and export as the settings map's role-to-hex JSON,
//! so a file, the settings entry, and a shared theme are one shape.
//! Layout shows the opening workspace's dock tree (every split, tab
//! group, and panel) with each panel's settings a click away, and
//! moves whole compositions in and out as the layout dump's JSON.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, size, svg, AnyElement, AnyWindowHandle, App, Axis, Bounds, ClipboardItem,
    Context, Div, ElementId, Entity, EntityId, FocusHandle, Global, Hsla, MouseButton,
    MouseDownEvent, PathPromptOptions, Pixels, ScrollHandle, SharedString, Stateful, Subscription,
    WeakEntity, Window, WindowHandle,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::Scrollbar;
use gpui_component::text::TextView;
use gpui_component::{Root, Sizable as _};

use crate::convert;
use crate::embeddings;
use crate::integrations::tray;
use crate::lastfm::import;
use crate::panel_settings;
use crate::pass_prompt;
use crate::replaygain_job;
use crate::startup::updater;
use crate::tempo_job;
use crate::workspace::{ApplyShaders, Workspace};
use rox_core::settings::layouts::Preset;
use rox_core::settings::{
    self, data_dir, settings_path, AcousticSave, Frame, GainModeSetting, LayoutSize, LyricsSave,
    NamedLayout, Providers, RatingStyle, ReplayGainSave, Settings, ShuffleMode, Theme,
    WorkspaceMeta, BORDER_MAX, MARGIN_MAX, PADDING_MAX, ROUNDING_MAX,
};
use rox_design::assets::icons;
use rox_design::palette::{self, Palette, Role, Side, Sides, ROLES};
use rox_design::tokens;
use rox_dock::{DockAreaState, DockEvent, PanelView, StackPanel, TabPanel};
use rox_library::store::{BpmCoverage, GainCoverage, Stats, Storage};
use rox_net::lastfm::{has_builtin_keys, AuthPhase};
use rox_net::providers;
use rox_panel_api::panel::{self, AppState};
use rox_panel_api::panel_settings::{ShaderNameField, ShaderSource};
use rox_panel_api::query::search::{SearchBox, SearchEvent};
use rox_panel_api::signal_ui::{self, routes::RouteEditState};
use rox_panel_kit::ui::{
    self as settings_ui, chord, dialog_button, grid_columns, icon_button, kbd, kbd_line, sidebar,
    small_button, PageBody, Query, Rows, Section, Seg, SidesScrub, SECTION_GAP,
};
use rox_panel_kit::ScrubState;
use rox_playback::continuation;
use rox_playback::engine;
use rox_playback::output;
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::{Library, LibraryEvent};
use rox_services::discord_presence::DiscordPresence;
use rox_services::lastfm::Scrobbler;
use rox_services::player::Player;
use rox_services::thumbs::Thumbs;
use rox_viz::signal::Route;

mod keymap_page;
mod workspace_page;

// The folder table's fixed columns: the rollup numbers and the remove
// control, the last sized to `icon_button`'s footprint so the header
// aligns.
const TRACKS_COL_W: Pixels = px(56.);
const ALBUMS_COL_W: Pixels = px(56.);
const SIZE_COL_W: Pixels = px(72.);
const ACTION_COL_W: Pixels = px(22.);

/// The rates the exclusive picker offers: the two base clocks and their
/// doubles and quadruples, which is every rate consumer hardware actually
/// runs. A card that hasn't got one falls back to its nearest and reports that.
const RATES: &[u32] = &[44100, 48000, 88200, 96000, 176400, 192000];

/// The periods the buffer picker offers, in milliseconds, either side of the
/// backend's 10 ms default.
const PERIODS_MS: &[f64] = &[2.5, 5.0, 10.0, 20.0, 40.0];

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
/// and its window handle are the Layout page's subject: the tree renders
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
        if handle
            .update(cx, |_, window, _| window.activate_window())
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
        icons::RADIO,
    ),
    (Page::Keymap, "settings-page-keymap", icons::KEYBOARD),
    (Page::Library, "settings-page-library", icons::LIST_MUSIC),
    (Page::Mcp, "settings-page-mcp", icons::LINK),
    (Page::MlModels, "settings-page-ml-models", icons::LAYERS),
    (Page::Playback, "settings-page-playback", icons::PLAY),
    (Page::Providers, "settings-page-providers", icons::DOWNLOAD),
    (Page::Shader, "settings-page-shader", icons::BLEND),
    (Page::Storage, "settings-page-storage", icons::DATABASE),
    (
        Page::Workspace,
        "settings-page-workspace",
        icons::APP_WINDOW,
    ),
    (Page::Development, "settings-page-development", icons::FLASK),
];

/// Where a model for a given job comes from: the shelf rox keeps, or a file
/// the user supplies. Every model category on the ML Models page reads this
/// way, so a second category (whatever job it ends up doing) inherits the
/// same two halves rather than inventing its own arrangement.
#[derive(Clone, Copy, PartialEq)]
enum ModelKind {
    Recommended,
    Custom,
}

fn model_kinds() -> Vec<(SharedString, ModelKind)> {
    vec![
        (
            rox_i18n::t!("settings-mlmodels-kind-recommended"),
            ModelKind::Recommended,
        ),
        (
            rox_i18n::t!("settings-mlmodels-kind-custom"),
            ModelKind::Custom,
        ),
    ]
}

/// The orders shuffle can put the upcoming queue in, and what each one
/// means. Read on the Playback page.
fn shuffle_modes() -> Vec<panel::ModeSpec<ShuffleMode>> {
    vec![
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-shuffle-random"),
            description: rox_i18n::t!("settings-playback-shuffle-random.description"),
            value: ShuffleMode::Random,
        },
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-shuffle-similar"),
            description: rox_i18n::t!("settings-playback-shuffle-similar.description"),
            value: ShuffleMode::Similar,
        },
    ]
}

/// The strategies that refill a queue which has run dry (ADR 17).
///
/// Note how these differ from the orders above: every one of them is about
/// which tracks join the queue, and not one of them touches the order the
/// queue already has.
///
/// There's no Radio here. The Similar order does the radio draw when it runs
/// out, so it's part of that pick instead of a fourth strategy that only ever
/// made sense alongside it.
fn continuation_modes() -> Vec<panel::ModeSpec<continuation::Mode>> {
    vec![
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-continuation-off"),
            description: rox_i18n::t!("settings-playback-continuation-off.description"),
            value: continuation::Mode::Off,
        },
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-continuation-continue"),
            description: rox_i18n::t!("settings-playback-continuation-continue.description"),
            value: continuation::Mode::Continue,
        },
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-continuation-weighted"),
            description: rox_i18n::t!("settings-playback-continuation-weighted.description"),
            value: continuation::Mode::Weighted,
        },
    ]
}

/// The storage page's measurements, taken entering the page and after a
/// clear rather than per frame: the database walk and the store walks are
/// affordable once, nowhere near affordable every paint.
#[derive(Clone, Default)]
struct StorageInfo {
    /// The whole library's rollup: tracks, albums, bytes of music.
    music: Stats,
    /// Where library.db's pages went, bucket by bucket. The page reads out
    /// these buckets rather than the file's size, which says only that
    /// something in there is large.
    breakdown: Storage,
    /// Every acoustic model with vectors in the database, including ones
    /// this build doesn't recognize.
    models: Vec<rox_library::embeddings::ModelRows>,
    /// thumbs.db with its WAL sidecars.
    thumbs: u64,
    /// Everything under waveforms/.
    waveforms: u64,
    /// Everything in the lyrics store (lyrics/).
    lyrics: u64,
    /// Everything in the artist store (artists/).
    artists: u64,
    /// The downloaded model weights (models/).
    weights: u64,
    /// The look the app is using plus everything set up around it: the
    /// saved workspaces, the ejected shaders, the icon packs.
    app_data: u64,
    /// The log file and its rolled back file (logs/).
    logs: u64,
}

impl StorageInfo {
    /// Walk everything the storage page shows. Runs on the background
    /// executor, on a connection of its own because the library's belongs to
    /// the UI thread: the page accounting reads every page in the file, and
    /// the stores are counted file by file on top of that.
    ///
    /// A database that isn't there is left alone rather than opened, since
    /// opening one creates it, and this page has no business making a
    /// library.db for someone who has never scanned a folder.
    fn measure(db: &Path) -> Self {
        let data = data_dir();
        let conn = db
            .exists()
            .then(|| rox_library::store::open(db).ok())
            .flatten();
        let (music, breakdown, models) = match &conn {
            Some(conn) => (
                rox_library::store::stats(conn).unwrap_or_default(),
                rox_library::store::storage_breakdown(conn).unwrap_or_default(),
                rox_library::embeddings::models(conn).unwrap_or_default(),
            ),
            None => Default::default(),
        };
        Self {
            music,
            breakdown,
            models,
            thumbs: db_size(&data.join("thumbs.db")),
            waveforms: dir_size(&rox_services::peaks::cache_dir()),
            lyrics: dir_size(&settings::lyrics_dir()),
            artists: dir_size(&settings::artists_dir()),
            weights: dir_size(&rox_acoustic::models::dir()),
            app_data: file_size(&settings::look_path())
                + dir_size(&settings::workspaces_dir())
                + dir_size(&settings::shaders_dir())
                + dir_size(&crate::startup::icon_packs::packs_dir()),
            // The log file follows an override, so the size comes off
            // whichever folder the Reveal button opens rather than the
            // default one beside it.
            logs: rox_core::logging::log_path()
                .parent()
                .map(dir_size)
                .unwrap_or(0),
        }
    }
}

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
}

struct SettingsWindow {
    page: Page,
    /// The sidebar's search box: a non-empty query swaps the page area
    /// for the all-pages results stack, every page filtered through
    /// [`Query`] under its own breadcrumb.
    search: Entity<SearchBox>,
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
    /// The app-wide signal pool, for the screen shader's route editor: the
    /// routes it edits are the app's, and so are the signals they read.
    signals: Arc<rox_viz::signal::SignalHub>,
    /// The workspace that opened this window, the Layout page's subject:
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
    discord: Entity<DiscordPresence>,
    discord_enabled: bool,
    discord_show_lastfm_button: bool,
    discord_show_youtube_button: bool,
    /// The api credential inputs; edits write through to the scrobbler per
    /// keystroke, the pickers' cadence.
    lastfm_key: Entity<InputState>,
    lastfm_secret: Entity<InputState>,
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
    /// The Appearance page's new-icon-pack name field.
    pack_name: Entity<InputState>,
    /// The mini-player roles the Layout page assigns, by preset name, kept
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
    /// The stored language pick, copied from settings like the icon
    /// pack below: None is System, and the row marks its segment without
    /// re-reading the settings file per render.
    language: Option<String>,
    /// The active icon pack, copied from settings so the Appearance page's
    /// pack list marks the current one without re-reading the settings file
    /// (which contains the dock dumps) on every render.
    active_icon_pack: Option<String>,
    /// The pack folders as last listed, so the Icons section doesn't walk
    /// the directory on every Appearance render; create, switch, and
    /// delete refresh it.
    icon_packs: Vec<String>,
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
    /// The connect flow's phases arrive through here, so the page's status
    /// line updates with them.
    _scrobbler_changed: Subscription,
    _library_changed: Subscription,
    /// Scan progress ticks notify the library without emitting Updated;
    /// the Library page's busy line needs those repaints too.
    _library_repaint: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
    /// The Layout page's tree follows the dock: layout events catch
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
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        if this.broadcast_enabled {
                            crate::integrations::broadcast::apply();
                        }
                    }
                    InputEvent::Focus => {}
                }
            }));
        }
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
            base,
            editor_mode,
            keep_theme: settings.look.bundle.appearance.keep_theme,
            surface_opacity: settings.look.bundle.appearance.surface_opacity,
            backdrop_strength: settings.look.bundle.appearance.backdrop_strength,
            backdrop_all_windows: settings.look.bundle.appearance.backdrop_all_windows,
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
            signals: state.signals,
            workspace,
            workspace_window,
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            player,
            crossfade_scrub: ScrubState::default(),
            preamp_scrub: ScrubState::default(),
            fallback_scrub: ScrubState::default(),
            output_exclusive,
            output_devices,
            playback,
            thumbs: state.thumbs,
            scrobbler,
            discord: state.discord.clone(),
            discord_enabled: settings.accounts.discord.enabled,
            discord_show_lastfm_button: settings.accounts.discord.show_lastfm_button,
            discord_show_youtube_button: settings.accounts.discord.show_youtube_button,
            lastfm_key,
            lastfm_secret,
            broadcast_host,
            broadcast_port,
            broadcast_mount,
            broadcast_user,
            broadcast_password,
            broadcast_name,
            broadcast_enabled: settings.broadcast.enabled,
            broadcast_bitrate: settings.broadcast.bitrate,
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
            pack_name: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(rox_i18n::t!("settings-appearance-pack-name-placeholder"))
            }),
            primary_layout: settings.look.bundle.primary_layout.clone(),
            mini_layout: settings.look.bundle.mini_layout.clone(),
            pending: None,
            dialog_focus: cx.focus_handle(),
            focus: focus.clone(),
            check_updates: settings.check_updates,
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
            active_icon_pack: settings.icon_pack.clone(),
            icon_packs: crate::startup::icon_packs::all(),
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
            _scrobbler_changed,
            _library_changed,
            _library_repaint,
            _backdrop_changed,
            _dock_changes,
            _search_changes,
            _player_changed,
            _player_view,
            _record_keys,
        }
    }

    /// A picker's change: the role into the working palette, out through
    /// the one setter, into the file. Clearing the hex field reads as
    /// back to the role's default.
    fn role_edited(
        &mut self,
        index: usize,
        color: Option<Hsla>,
        picker: &Entity<ColorPickerState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let role = &ROLES[index];
        match color {
            Some(color) => (role.set)(&mut self.base, color.to_rgb()),
            None => {
                let default = (role.get)(&self.side_anchor());
                (role.set)(&mut self.base, default);
                picker.update(cx, |picker, cx| picker.set_value(default, window, cx));
            }
        }
        palette::set(self.base, cx);
        // The palette is live above; the file write goes through the
        // debounce, since a picker drag fires a change per tick like the
        // sliders do.
        self.persist_palette = true;
        self.persist_appearance_soon(cx);
    }

    /// The song-theming switch, the Window menu toggle's twin: through
    /// the palette pipe, which also gates the backdrop layers, and into
    /// the file. The toggle reads the palette static, not a cached field,
    /// so the two entry points never show different states.
    fn set_art_theming(&mut self, on: bool, cx: &mut Context<Self>) {
        palette::set_art_theming(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.art_theming = on);
        cx.notify();
    }

    /// The interface language. Through the settings pipe so every window
    /// repaints in the new locale at once; the pick persists as the
    /// registry id, and None keeps following the OS.
    fn set_language(&mut self, language: Option<String>, cx: &mut Context<Self>) {
        settings::set_language(language.as_deref(), cx);
        self.language = language.clone();
        Settings::update(move |s| s.language = language);
        cx.notify();
    }

    /// The theme pick: which palette side renders, with System following
    /// the OS. Through the settings pipe so the side re-resolves and every
    /// window eases over; render then re-seeds the editor onto that side.
    /// The radio reads the settings static, not a cached field, so this
    /// and the theme toggle panel never show different states.
    fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        settings::set_theme(theme, cx);
        Settings::update(move |s| s.theme = theme);
        cx.notify();
    }

    /// The keep-theme switch: holds the active theme's palette under any
    /// cover. Through the palette pipe so open windows ease over, and into
    /// the file.
    fn set_keep_theme(&mut self, on: bool, cx: &mut Context<Self>) {
        self.keep_theme = on;
        palette::set_keep_theme(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.keep_theme = on);
        cx.notify();
    }

    /// The editing side's designed anchor: what a cleared picker returns
    /// a role to and what Reset returns the whole palette to.
    fn side_anchor(&self) -> Palette {
        match self.editor_mode {
            palette::Mode::Dark => Palette::default(),
            palette::Mode::Light => Palette::light(),
        }
    }

    /// Persist the working palette onto its side of the settings file,
    /// the immediate writers' shared tail (inverse, import, song theme).
    fn persist_palette_now(&self) {
        let mode = self.editor_mode;
        let map = self.base.to_map();
        Settings::update(move |s| *s.palette_map_mut(mode) = map);
    }

    /// Point the editor at the side now rendering after a theme switch:
    /// the working copy and every picker move to that theme's palette.
    /// Runs from render, since every switch path (the toggle here, the OS
    /// flipping under System, a workspace apply) repaints all windows.
    fn sync_editor_side(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_mode == palette::mode() {
            return;
        }
        self.editor_mode = palette::mode();
        self.base = palette::theme_palette(self.editor_mode);
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let color = (role.get)(&self.base);
            picker.update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
    }

    /// Catch the Typography slider up to a size this window didn't write:
    /// the zoom shortcuts step the same live value from anywhere in the
    /// app. Runs from render, `sync_editor_side`'s shape, since every step
    /// repaints all windows. The slider's own scrub ends up back on the value
    /// it just wrote, so this only moves for outside writers.
    fn sync_font_size(&mut self) {
        self.font_size = palette::app_font_size();
    }

    /// Catch the Shader page's copies up to a config this window didn't
    /// write: a workspace apply swaps the screen shader wholesale, and the
    /// picker kept naming the one the old look used. Runs from render off
    /// the workspace's apply counter, `sync_editor_side`'s shape, since
    /// every apply repaints all windows. This window's own edits move the
    /// counter too, and end up back on the values they just wrote.
    fn sync_post_shader(&mut self) {
        let gen = crate::workspace::post_shader_gen();
        if gen == self.post_shader_gen {
            return;
        }
        self.post_shader_gen = gen;
        let Some(config) = crate::workspace::post_shader_applied() else {
            return;
        };
        self.post_shader_name = config.name;
        self.post_shader_source = config.source;
        self.post_shader_path = config.path;
        self.post_shader_all_windows = config.all_windows;
        self.post_shader_run_idle = config.run_when_idle;
        // The routes and hand-set slots update too: the apply already
        // pushed the file's copies into the live feed the shader reads, so
        // leaving the editor on the old lists would show one thing and
        // drive another.
        self.post_shader_routes = config.routes;
        self.post_shader_manual = config.manual;
    }

    /// The restore switch: straight into the file. Launch reads it there,
    /// so the flip is live for the next start without touching playback.
    fn set_restore_last_track(&mut self, on: bool, cx: &mut Context<Self>) {
        self.restore_last_track = on;
        Settings::update(move |s| s.restore_last_track = on);
        cx.notify();
    }

    /// The watch-folders switch: flip the local copy and hand it to the shared
    /// library, which persists it and arms or drops the watcher on the spot.
    fn set_watch_library(&mut self, on: bool, cx: &mut Context<Self>) {
        self.watch_library = on;
        self.library
            .update(cx, |library, cx| library.set_watch(on, cx));
        cx.notify();
    }

    /// The case-fold switch: flip the live flag, persist, and reload the
    /// projection so every symbol table re-interns under the new rule.
    /// The database never changes; this is a read-model rebuild.
    fn set_fold_case(&mut self, on: bool, cx: &mut Context<Self>) {
        self.fold_case = on;
        settings::set_fold_case(on);
        Settings::update(move |s| s.fold_case = on);
        self.library
            .update(cx, |library, cx| library.reload_projection(cx));
        cx.notify();
    }

    /// The genre-separator switch, the case-fold's twin: flip the live
    /// flag in the genre module, persist, and reload the projection so
    /// every genre surface re-splits under the new rule. Files and the
    /// database never change; matching splits stored strings at read.
    fn set_split_genre_compounds(&mut self, on: bool, cx: &mut Context<Self>) {
        self.split_genre_compounds = on;
        rox_library::genre::set_split_compounds(on);
        Settings::update(move |s| s.split_genre_compounds = on);
        self.library
            .update(cx, |library, cx| library.reload_projection(cx));
        cx.notify();
    }

    /// The quit-to-tray switch, the Window menu toggle's twin: flips the
    /// live flag the close path reads, persists, and puts the tray icon up
    /// or takes it down on the spot. The toggle reads the static, not a
    /// cached field, so the two entry points never show different states.
    fn set_quit_to_tray(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_quit_to_tray(on);
        Settings::update(move |s| s.quit_to_tray = on);
        tray::sync(cx);
        cx.notify();
    }

    fn set_discord_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.discord_enabled = on;
        Settings::update(move |s| s.accounts.discord.enabled = on);
        self.discord.update(cx, |d, cx| d.reload_config(cx));
        cx.notify();
    }

    fn set_discord_show_lastfm_button(&mut self, on: bool, cx: &mut Context<Self>) {
        self.discord_show_lastfm_button = on;
        Settings::update(move |s| s.accounts.discord.show_lastfm_button = on);
        self.discord.update(cx, |d, cx| d.reload_config(cx));
        cx.notify();
    }

    fn set_discord_show_youtube_button(&mut self, on: bool, cx: &mut Context<Self>) {
        self.discord_show_youtube_button = on;
        Settings::update(move |s| s.accounts.discord.show_youtube_button = on);
        self.discord.update(cx, |d, cx| d.reload_config(cx));
        cx.notify();
    }

    /// The portable switch. On creates rox-data beside the executable,
    /// seeds it from the current data folder when it's new, and drops
    /// the marker file launch checks for; off removes the marker and
    /// leaves rox-data where it is. Going back doesn't migrate; that
    /// data is the user's to keep or delete. Either way the running app
    /// stays on the folder it started with.
    fn set_portable(&mut self, on: bool, cx: &mut Context<Self>) {
        let (Some(marker), Some(portable_dir)) =
            (settings::portable_marker(), settings::portable_data_dir())
        else {
            return;
        };
        if !on {
            let _ = std::fs::remove_file(&marker);
            self.portable = marker.exists();
            cx.notify();
            return;
        }
        if portable_dir.exists() {
            // A rox-data from an earlier portable stint: reuse it rather
            // than overwrite it with the current state.
            let _ = std::fs::write(&marker, b"");
            self.portable = marker.exists();
            cx.notify();
            return;
        }
        // Seed rox-data from the live data folder off the UI thread (the
        // caches can be big) and only drop the marker once the copy
        // finishes, so a restart mid-copy never boots on a half folder. The
        // copy is best-effort over live databases, the same risk copying
        // the folder by hand takes; the restart requirement keeps the
        // window small.
        self.portable = true;
        self.portable_busy = true;
        let source = settings::data_dir();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    if copy_dir(&source, &portable_dir).is_ok() {
                        let _ = std::fs::write(&marker, b"");
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.portable_busy = false;
                this.portable = settings::portable_marker().is_some_and(|marker| marker.exists());
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The readings switch: through the live static, which every name cell
    /// reads as it draws, and into the file. Nothing is rebuilt, since the
    /// sort names are already in the projection; the windows just repaint.
    /// The toggle reads the static rather than a cached field, so it can't
    /// drift from what the panels are drawing.
    fn set_show_readings(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_show_readings(on, cx);
        Settings::update(move |s| s.show_readings = on);
        cx.notify();
    }

    /// The menubar switch, the Window menu toggle's twin: through the
    /// live static so every workspace window drops or regrows its bar,
    /// and into the file. The toggle reads the static, not a cached
    /// field, so the two entry points never show different states.
    fn set_hide_menubar(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_hide_menubar(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.hide_menubar = on);
        cx.notify();
    }

    /// The design-mode switch, the menubar's Window entry from this side.
    /// Same route as the menubar's: the live flag repaints every window,
    /// and the file keeps it across launches.
    fn set_design_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_design_mode(on, cx);
        Settings::update(move |s| s.design_mode = on);
        cx.notify();
    }

    /// The resize-lock switch, the design-mode setter's shape: the live
    /// flag repaints every window's handles, and the file keeps it.
    fn set_resize_lock(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_resize_lock(on, cx);
        Settings::update(move |s| s.resize_lock = on);
        cx.notify();
    }

    /// The seams switch: through the live static in the dock crate so
    /// every window's panel dividers repaint, and into the file. The
    /// toggle reads the static, like the menubar's.
    fn set_seams(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_seams(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.seams = on);
        cx.notify();
    }

    /// The decorations switch, the Window menu toggle's twin: flip the
    /// flag, persist, and renegotiate the workspace windows.
    fn set_os_decorations(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_os_decorations(on);
        Settings::update(move |s| s.look.bundle.appearance.os_decorations = on);
        crate::workspace::apply_decorations(cx);
        cx.notify();
    }

    /// The resize-border switch. Same shape as the decorations one above,
    /// and only ever shown on Windows.
    fn set_resize_border(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_resize_border(on);
        Settings::update(move |s| s.look.bundle.appearance.resize_border = on);
        crate::workspace::apply_resize_border(cx);
        cx.notify();
    }

    /// The app font: through the live static, so every open window
    /// repaints in the new family, and into the file. None follows the
    /// platform default.
    fn set_app_font(&mut self, font: Option<String>, cx: &mut Context<Self>) {
        settings::set_app_font(font.clone(), cx);
        Settings::update(move |s| s.look.bundle.appearance.app_font = font);
        cx.notify();
    }

    /// The rating scale: through the live static, so every open rating
    /// column redraws, and into the file.
    fn set_rating_style(&mut self, style: RatingStyle, cx: &mut Context<Self>) {
        self.rating_style = style;
        settings::set_rating_style(style, cx);
        Settings::update(move |s| s.look.bundle.appearance.rating_style = style);
        cx.notify();
    }

    /// The unrated dots, the scale's sibling: same live-static route.
    fn set_rating_dots(&mut self, on: bool, cx: &mut Context<Self>) {
        self.rating_dots = on;
        settings::set_rating_dots(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.rating_dots = on);
        cx.notify();
    }

    /// The app font size: the strip fraction mapped onto whole px across
    /// the shared range, through the palette pipe so every window's rem
    /// follows the scrub live.
    fn set_font_size(&mut self, value: f32, cx: &mut Context<Self>) {
        self.font_size = value;
        palette::set_app_font_size(self.font_size, cx);
        self.persist_appearance_soon(cx);
        cx.notify();
    }

    fn set_surface(&mut self, value: f32, cx: &mut Context<Self>) {
        self.surface_opacity = value;
        self.scalars_edited(cx);
    }

    fn set_backdrop(&mut self, value: f32, cx: &mut Context<Self>) {
        self.backdrop_strength = value;
        self.scalars_edited(cx);
    }

    /// The backdrop-everywhere switch: live into the palette static the
    /// layer's gate reads, straight into the file since a toggle is one
    /// write, not a scrub.
    fn set_backdrop_windows(&mut self, on: bool, cx: &mut Context<Self>) {
        self.backdrop_all_windows = on;
        palette::set_backdrop_all_windows(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.backdrop_all_windows = on);
        cx.notify();
    }

    fn scalars_edited(&mut self, cx: &mut Context<Self>) {
        palette::set_scalars(self.surface_opacity, self.backdrop_strength, cx);
        self.persist_appearance_soon(cx);
        cx.notify();
    }

    /// Persist the appearance scalars, frame, and any pending palette edit
    /// after the current scrub settles. Each slider tick or picker change
    /// would otherwise read, parse, and rewrite the whole settings file (dock
    /// dumps and all); the live statics already hold the value, so only the
    /// file write needs to wait for the last tick.
    fn persist_appearance_soon(&mut self, cx: &mut Context<Self>) {
        self.persist_gen += 1;
        let gen = self.persist_gen;
        let (surface, backdrop, frame) = (self.surface_opacity, self.backdrop_strength, self.frame);
        let palette = self
            .persist_palette
            .then(|| (self.editor_mode, self.base.to_map()));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            // A later tick bumped the gen past this capture, so only the last
            // edit in a burst writes. The palette rereads at fire time so an
            // immediate writer (reset, import) that runs inside the wait isn't
            // undone; the capture only stands in when the window closed
            // before the timer.
            let (latest, palette) = this
                .update(cx, |this, _| {
                    (
                        this.persist_gen,
                        this.persist_palette
                            .then(|| (this.editor_mode, this.base.to_map())),
                    )
                })
                .unwrap_or((gen, palette));
            if latest == gen {
                // The font size comes off the live static rather than a
                // capture: the zoom shortcuts write it from outside this
                // window, and one that runs inside the wait would otherwise
                // get rolled back to whatever the slider last wrote.
                let font_size = palette::app_font_size();
                Settings::update(move |s| {
                    s.look.bundle.appearance.surface_opacity = surface;
                    s.look.bundle.appearance.backdrop_strength = backdrop;
                    s.look.bundle.appearance.frame = frame;
                    s.app_font_size = font_size;
                    if let Some((mode, palette)) = palette {
                        *s.palette_map_mut(mode) = palette;
                    }
                });
            }
        })
        .detach();
    }

    // The app-wide frame setters: whole px in, the new default every
    // panel that sets no override of its own takes. A side of None comes
    // off the linked strip and moves all four together.

    fn set_margin(&mut self, side: Option<Side>, value: f32, cx: &mut Context<Self>) {
        self.frame.margin = self.frame.margin.edited(side, value);
        self.frame_edited(cx);
    }

    fn set_padding(&mut self, side: Option<Side>, value: f32, cx: &mut Context<Self>) {
        self.frame.padding = self.frame.padding.edited(side, value);
        self.frame_edited(cx);
    }

    fn set_rounding(&mut self, value: f32, cx: &mut Context<Self>) {
        self.frame.rounding = value;
        self.frame_edited(cx);
    }

    fn set_border(&mut self, side: Option<Side>, value: f32, cx: &mut Context<Self>) {
        self.frame.border = self.frame.border.edited(side, value);
        self.frame_edited(cx);
    }

    // The link toggles. Splitting only opens the sides up; linking
    // flattens them onto the widest, so nothing on screen disappears.

    fn split_margin(&mut self, split: bool, cx: &mut Context<Self>) {
        self.margin_split = split;
        if !split {
            self.frame.margin = self.frame.margin.linked();
        }
        self.frame_edited(cx);
    }

    fn split_padding(&mut self, split: bool, cx: &mut Context<Self>) {
        self.padding_split = split;
        if !split {
            self.frame.padding = self.frame.padding.linked();
        }
        self.frame_edited(cx);
    }

    fn split_border(&mut self, split: bool, cx: &mut Context<Self>) {
        self.border_split = split;
        if !split {
            self.frame.border = self.frame.border.linked();
        }
        self.frame_edited(cx);
    }

    fn frame_edited(&mut self, cx: &mut Context<Self>) {
        settings::set_app_frame(self.frame, cx);
        self.persist_appearance_soon(cx);
        cx.notify();
    }

    /// One app-frame knob's slider row: the value over its 0 to `max`
    /// range, the px readout alongside. Always set, since these are the
    /// defaults themselves; a panel's own settings are where an override
    /// forks off them. Typed values may run past the strip's top, the
    /// setters accept whatever comes in.
    fn frame_row(
        &self,
        scrub: &ScrubState,
        value: f32,
        max: f32,
        apply: fn(&mut Self, f32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        settings_ui::scalar(
            scrub,
            &self.value_edit,
            value,
            settings_ui::span(0., max, " px"),
            apply,
            cx,
        )
    }

    /// [`frame_row`](Self::frame_row) for a four-sided knob: the link
    /// toggle and, behind it, one strip or four.
    #[allow(clippy::too_many_arguments)]
    fn frame_sides_row(
        &self,
        scrub: &SidesScrub,
        value: Sides,
        split: bool,
        max: f32,
        on_split: fn(&mut Self, bool, &mut Context<Self>),
        apply: fn(&mut Self, Option<Side>, f32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        settings_ui::sides_control(
            scrub,
            &self.value_edit,
            value,
            split,
            settings_ui::span(0., max, " px"),
            on_split,
            apply,
            cx,
        )
    }

    /// A whole palette into the editor at once: the working copy, every
    /// picker, and the live palette. Persisting is the caller's, because
    /// reset writes an empty map where import writes a full one.
    fn apply_palette(&mut self, palette: Palette, window: &mut Window, cx: &mut Context<Self>) {
        self.base = palette;
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let color = (role.get)(&self.base);
            picker.update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
        palette::set(self.base, cx);
    }

    /// Back to the editing side's stock palette; the file's map empties
    /// rather than filling with defaults.
    fn reset_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_palette(self.side_anchor(), window, cx);
        // Back off the debounced palette writes too, or a settling picker
        // burst would refill the map this just emptied.
        self.persist_palette = false;
        let mode = self.editor_mode;
        Settings::update(move |s| s.palette_map_mut(mode).clear());
    }

    /// Seed the working palette from the other theme's, flipped across
    /// the designed ladders: editing dark, Inverse From Light Theme pulls
    /// the light side's look dark, and the other way around. The map
    /// persists like any other edit, so the flip is kept across a restart.
    fn inverse_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let other = match self.editor_mode {
            palette::Mode::Dark => palette::Mode::Light,
            palette::Mode::Light => palette::Mode::Dark,
        };
        self.apply_palette(palette::theme_palette(other).inverse(), window, cx);
        self.persist_palette_now();
    }

    /// Bake the song theme into the palette: the colors the playing track
    /// derives become the working palette, then song theming turns off so
    /// they hold. The look a track gave the app is kept as a fixed theme.
    /// The resolved palette is read before theming goes off, since turning
    /// it off retargets the tint back to the base.
    fn apply_song_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let themed = palette::resolved();
        self.set_art_theming(false, cx);
        self.apply_palette(themed, window, cx);
        self.persist_palette_now();
    }

    /// Pick a palette file and load it: the same role-to-hex map the
    /// settings file holds, so exports, settings, and shared themes are
    /// one shape. Unknown roles and bad values fall away silently, a
    /// file that isn't a map at all is ignored.
    fn import_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Some(map) = std::fs::read_to_string(path)
                .ok()
                .and_then(|json| serde_json::from_str::<BTreeMap<String, String>>(&json).ok())
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let anchor = this.side_anchor();
                this.apply_palette(Palette::from_map_over(anchor, &map), window, cx);
                this.persist_palette_now();
            })
            .ok();
        })
        .detach();
    }

    /// Save a palette file, [`Palette::to_map`]'s shape: the working
    /// palette, or the derived one while song theming drives the colors,
    /// so a look a track built can leave as a theme.
    fn export_palette(&mut self, cx: &mut Context<Self>) {
        let map = if palette::art_theming() {
            palette::resolved().to_map()
        } else {
            self.base.to_map()
        };
        let home = dirs::home_dir().unwrap_or_default();
        let rx = cx.prompt_for_new_path(&home, Some("palette.json"));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            if let Ok(json) = serde_json::to_string_pretty(&map) {
                std::fs::write(path, json).ok();
            }
        })
        .detach();
    }

    fn appearance_page(&self, q: &Query, columns: usize, cx: &mut Context<Self>) -> PageBody {
        PageBody::new()
            .section(Section::new(
                q,
                icons::MENU,
                rox_i18n::t!("settings-appearance-section-interface"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-language",
                        &["language", "locale", "translation"],
                        panel::language_picker(
                            "app-language",
                            self.language.clone(),
                            Self::set_language,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-design-mode",
                        &["edit", "layout", "rearrange", "lock"],
                        panel::toggle(settings::design_mode(), Self::set_design_mode, cx),
                    )
                    .keyed(
                        "settings-appearance-hide-menubar",
                        &["menu bar", "toolbar", "alt"],
                        panel::toggle(settings::hide_menubar(), Self::set_hide_menubar, cx),
                    )
                    .keyed(
                        "settings-appearance-os-decorations",
                        &["title bar", "chrome", "frameless"],
                        panel::toggle(settings::os_decorations(), Self::set_os_decorations, cx),
                    )
                    // Windows only: on Linux and macOS the borderless window has
                    // no edge resize to take away, so the row would be a switch
                    // that does nothing.
                    .when(cfg!(target_os = "windows"), |rows| {
                        rows.keyed(
                            "settings-appearance-resize-border",
                            &["border", "edge", "frame", "borderless"],
                            panel::toggle(settings::resize_border(), Self::set_resize_border, cx),
                        )
                    })
                },
            ))
            .section(Section::new(
                q,
                icons::CONTRAST,
                rox_i18n::t!("settings-appearance-section-theming"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-appearance-theme",
                        &["dark", "light", "mode", "appearance"],
                        panel::choices_shared(
                            &[
                                (rox_i18n::t!("settings-appearance-theme-dark"), Theme::Dark),
                                (
                                    rox_i18n::t!("settings-appearance-theme-light"),
                                    Theme::Light,
                                ),
                                (
                                    rox_i18n::t!("settings-appearance-theme-system"),
                                    Theme::System,
                                ),
                            ],
                            settings::theme(),
                            Self::set_theme,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-song-theming",
                        &["album art", "tint", "accent"],
                        panel::toggle(palette::art_theming(), Self::set_art_theming, cx),
                    )
                    .keyed(
                        "settings-appearance-keep-theme",
                        &["dark", "light", "lock", "pin"],
                        panel::toggle(self.keep_theme, Self::set_keep_theme, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::ALIGN_LEFT,
                rox_i18n::t!("settings-appearance-section-typography"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-appearance-font",
                        &["typeface", "family", "text"],
                        panel::font_picker(
                            "app-font",
                            settings::app_font().map(|font| font.to_string()),
                            Self::set_app_font,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-font-size",
                        &["text size", "scale", "zoom"],
                        settings_ui::scalar(
                            &self.font_size_scrub,
                            &self.value_edit,
                            self.font_size,
                            settings_ui::span(
                                palette::FONT_SIZE_MIN,
                                palette::FONT_SIZE_MAX,
                                " px",
                            )
                            .hard(),
                            Self::set_font_size,
                            cx,
                        ),
                    )
                },
            ))
            .section(self.icons_section(q, cx))
            .section(Section::new(
                q,
                icons::EYE,
                rox_i18n::t!("settings-appearance-section-transparency"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-appearance-surface-opacity",
                        &["transparency", "translucent", "blur"],
                        settings_ui::slider_edit(
                            &self.surface_scrub,
                            &self.value_edit,
                            self.surface_opacity,
                            Self::set_surface,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-backdrop-strength",
                        &["transparency", "opacity", "blur", "wallpaper"],
                        settings_ui::slider_edit(
                            &self.backdrop_scrub,
                            &self.value_edit,
                            self.backdrop_strength,
                            Self::set_backdrop,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-backdrop-all-windows",
                        &["transparency", "backdrop", "child windows", "everywhere"],
                        panel::toggle(self.backdrop_all_windows, Self::set_backdrop_windows, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::SQUARE_DASHED,
                rox_i18n::t!("settings-appearance-section-frame"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-appearance-margin",
                        &["spacing", "gap", "outside", "sides"],
                        self.frame_sides_row(
                            &self.margin_scrub,
                            self.frame.margin,
                            self.margin_split,
                            MARGIN_MAX,
                            Self::split_margin,
                            Self::set_margin,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-padding",
                        &["spacing", "inset", "inside", "sides"],
                        self.frame_sides_row(
                            &self.padding_scrub,
                            self.frame.padding,
                            self.padding_split,
                            PADDING_MAX,
                            Self::split_padding,
                            Self::set_padding,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-rounding",
                        &["corner radius", "rounded"],
                        self.frame_row(
                            &self.rounding_scrub,
                            self.frame.rounding,
                            ROUNDING_MAX,
                            Self::set_rounding,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-border",
                        &["outline", "stroke", "edge", "sides"],
                        self.frame_sides_row(
                            &self.border_scrub,
                            self.frame.border,
                            self.border_split,
                            BORDER_MAX,
                            Self::split_border,
                            Self::set_border,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-panel-seams",
                        &["divider", "gutter", "grid lines"],
                        panel::toggle(settings::seams(), Self::set_seams, cx),
                    )
                },
            ))
            .section(self.colors_section(q, columns, cx))
    }

    /// The Shader page: the whole-window post-process and what drives it.
    /// Its own page rather than a section under Appearance because it
    /// isn't a look setting: it's a program the app runs over every
    /// frame, with a file, a compile error, and sixteen signal routes,
    /// and it had already outgrown sitting between Transparency and
    /// Frame. Matches the panel settings window, where a panel's shader
    /// is its own page under the same icon.
    fn shader_page(&mut self, q: &Query, window: &mut Window, cx: &mut Context<Self>) -> PageBody {
        PageBody::new()
            .section(self.screen_shader_section(q, window, cx))
            .section(self.backdrop_shader_section(q, window, cx))
    }

    /// The Screen Shader section: a WGSL post-process over the whole
    /// window, run by the workspace's driver. The toggle and source are
    /// written to settings and reapply everywhere; the error line reads
    /// the driver's live readout, so a broken edit caught by the hot
    /// reload shows here without a round trip through this window.
    fn screen_shader_section(
        &mut self,
        q: &Query,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Section {
        let enabled = crate::workspace::post_shader_on();
        let all_windows = self.post_shader_all_windows;
        let run_idle = self.post_shader_run_idle;
        let error = crate::workspace::post_shader_error();

        // The picker the panel shader pages lead with, over the app-wide
        // config, so the examples and the workspace's shaders are one list
        // wherever a shader gets picked. The file case diverges from the
        // panels in one way: the driver reads and watches the file itself,
        // and an inline source wins over the bookmark, so the bookmark only
        // reads as the file choice while nothing is inlined over it. The
        // picker only checks whether something resolves, so a stand-in
        // spares the section a file read per render.
        let name = self.post_shader_name.clone();
        let file_mode = name.is_none()
            && self.post_shader_source.trim().is_empty()
            && self.post_shader_path.is_some();
        let resolved = match name.as_deref() {
            Some(name) => settings::shader_pool_get(name).map(|entry| entry.source),
            None if !self.post_shader_source.trim().is_empty() => {
                Some(self.post_shader_source.clone())
            }
            None if file_mode => Some("// the file the driver watches".to_string()),
            None => None,
        };
        let path = file_mode.then(|| self.post_shader_path.clone()).flatten();
        let picked = ShaderSource {
            id: "screen-shader",
            name: name.as_deref(),
            path: path.as_deref(),
            resolved: resolved.as_deref(),
            clear: Some(|this: &mut Self, cx| this.clear_post_shader_source(cx)),
            // The whole window is the one surface where a scene doesn't
            // decorate the app, it replaces it, so the list only offers
            // shaders that declare they leave it usable.
            overlays_only: true,
            use_example: |this: &mut Self, index, cx| this.use_post_shader_example(index, cx),
            use_named: |this: &mut Self, name, cx| this.use_post_shader_pool(name, cx),
            choose_file: |this: &mut Self, window, cx| this.pick_post_shader(window, cx),
            edit: |this: &mut Self, window, cx| this.edit_post_shader_in_app(window, cx),
            eject: |this: &mut Self, cx| this.eject_post_shader(cx),
            detach: |this: &mut Self, cx| this.detach_post_shader(cx),
            reload: |this: &mut Self, cx| this.reload_post_shader(cx),
            save: |this: &mut Self, name, cx| this.save_post_shader_to_pool(name, cx),
            field: &mut self.post_shader_save_name,
            fallback: rox_i18n::t_static("settings-shader-screen-fallback-name"),
        }
        .render(window, cx);
        // A scene over the whole window hides the app, this row included.
        // The countdown is the way back out and stays the real safety net;
        // this line means nobody has to find that out by watching their
        // library disappear. Read off what's installed, so it applies to
        // the file the driver compiled as well as to a source this window
        // can see.
        let covers = enabled && crate::workspace::post_shader_overlay() == Some(false);
        // The same route editor the panel Shader page and the Shader
        // panel's Bindings page use, over the app-wide list. Its slot
        // names come off the file the workspace compiled, so a shader that
        // declares them reads the same here as it does on a panel.
        let hub = self.signals.clone();
        let labels = crate::workspace::post_shader_slot_labels();
        // The Bindings page's slot list over the app-wide config: a routed
        // slot shows the value going to the shader, an unrouted one is a
        // hand-set knob, which is how a screen shader's named parameters get
        // tuned without editing WGSL.
        let slots = signal_ui::slots::SlotList {
            hub: &hub,
            routes: &self.post_shader_routes,
            manual: &self.post_shader_manual,
            labels: &labels,
            value_edit: &self.value_edit,
            scrubs: &self.post_shader_slot_scrubs,
            set: Arc::new(|this: &mut Self, slot, value, cx| {
                this.set_post_shader_manual(slot, value, cx)
            }),
        }
        .render(cx);
        let editor = signal_ui::routes::RouteEditor {
            id: "screen-shader-route",
            hub: &hub,
            routes: &self.post_shader_routes,
            labels: &labels,
            value_edit: &self.value_edit,
            ui: &self.post_shader_route_ui,
            ui_mut: |this: &mut Self| &mut this.post_shader_route_ui,
            mutate: Arc::new(
                |this: &mut Self, edit: &mut dyn FnMut(&mut Vec<Route>), cx: &mut Context<Self>| {
                    this.edit_post_shader_routes(edit, cx);
                },
            ),
        };
        let legacy = self.post_shader_routes.is_empty();
        Section::new(
            q,
            icons::BLEND,
            rox_i18n::t!("settings-shader-section-overlay"),
            None,
            move |mut rows| {
                rows = rows
                    .keyed(
                        "settings-shader-overlay-enabled",
                        &[
                            "shader",
                            "wgsl",
                            "post process",
                            "effect",
                            "crt",
                            "overlay",
                            "screen",
                        ],
                        panel::toggle(enabled, Self::set_post_shader_enabled, cx),
                    )
                    .custom(
                        &[
                            "shader",
                            "wgsl",
                            "file",
                            "reload",
                            "source",
                            "example",
                            "preset",
                            "workspace",
                        ],
                        || picked.into_any_element(),
                    )
                    .when(covers, |rows| {
                        rows.custom(&["shader", "scene", "covers", "hides", "overlay"], || {
                            coverage_note(
                                rox_i18n::t!("settings-shader-scene-covers-window").to_string(),
                            )
                            .into_any_element()
                        })
                    })
                    .keyed(
                        "settings-shader-screen-all-windows",
                        &["shader", "child windows", "settings", "everywhere"],
                        panel::toggle(all_windows, Self::set_post_shader_all_windows, cx),
                    )
                    .keyed(
                        "settings-shader-screen-run-idle",
                        &["shader", "idle", "pause", "freeze", "mouse"],
                        panel::toggle(run_idle, Self::set_post_shader_run_idle, cx),
                    );
                rows = match error {
                    Some(error) => rows.custom(&["shader", "error", "compile"], || {
                        // The callout the output section shows for a failed
                        // device, for the same reason: the switch above reads as
                        // on, and a muted line under it is not enough to say that
                        // nothing behind it is running. A backend with no shader
                        // pipeline rejects every source with one word, which
                        // on its own reads as a stray label rather than a reason.
                        match panel::shader::unsupported(&error) {
                            true => panel::banner(
                                panel::Tone::Bad,
                                panel::shader::NO_PIPELINE_TITLE,
                                vec![panel::shader::NO_PIPELINE_NOTE.into()],
                            ),
                            false => panel::banner(
                                panel::Tone::Bad,
                                rox_i18n::t!("settings-shader-compile-error-title"),
                                vec![error.into()],
                            ),
                        }
                        .into_any_element()
                    }),
                    None => rows,
                };
                rows.custom(
                    &[
                        "shader",
                        "signal",
                        "route",
                        "slot",
                        "bind",
                        "modulation",
                        "knob",
                        "manual",
                    ],
                    || {
                        let add = editor.add_button(cx);
                        let mut body = div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_MD)
                            .child(editor.list(cx));
                        if legacy {
                            // Nothing routed is not nothing happening here, and
                            // saying so is the only way the first route someone
                            // adds doesn't look like it broke the other fifteen
                            // slots.
                            body = body.child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("settings-shader-legacy-note")),
                            );
                        }
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_MD)
                            .child(panel::setting_block(
                                rox_i18n::t!("settings-shader-signals-block"),
                                Some(rox_i18n::t!("settings-shader-signals-block.description")),
                                Some(add.into_any_element()),
                                body,
                            ))
                            .child(panel::setting_block(
                                rox_i18n::t!("settings-shader-slots-block"),
                                Some(rox_i18n::t!("settings-shader-slots-block.description")),
                                None,
                                slots,
                            ))
                            .into_any_element()
                    },
                )
            },
        )
    }

    /// One edit to the screen shader's routes: into this window's copy,
    /// into the workspace's live feed so the shader follows the drag, and
    /// into the file once the burst settles. The file write waits because
    /// it reloads and reserializes every shard, dock dumps and all, which
    /// is not what a slider tick should cost.
    fn edit_post_shader_routes(
        &mut self,
        edit: &mut dyn FnMut(&mut Vec<Route>),
        cx: &mut Context<Self>,
    ) {
        edit(&mut self.post_shader_routes);
        let routes = self.post_shader_routes.clone();
        crate::workspace::set_post_shader_routes(routes.clone());
        // Its own generation, not the appearance one: a route drag must not
        // cancel a pending palette write, and the two bursts overlap the
        // moment someone tunes a shader against a color.
        self.route_persist_gen += 1;
        let gen = self.route_persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let latest = this
                .update(cx, |this, _| this.route_persist_gen)
                .unwrap_or(gen);
            if latest == gen {
                Settings::update(move |s| s.post_shader.routes = routes);
            }
        })
        .detach();
        cx.notify();
    }

    /// One hand-set slot edit: into this window's copy, into the
    /// workspace's live feed so the shader follows the drag, and into the
    /// file once the burst settles, the routes' exact write path.
    fn set_post_shader_manual(&mut self, slot: usize, value: f32, cx: &mut Context<Self>) {
        match self
            .post_shader_manual
            .iter_mut()
            .find(|(at, _)| *at as usize == slot)
        {
            Some(entry) => entry.1 = value,
            None => self.post_shader_manual.push((slot as u8, value)),
        }
        let manual = self.post_shader_manual.clone();
        crate::workspace::set_post_shader_manual(manual.clone());
        self.manual_persist_gen += 1;
        let gen = self.manual_persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let latest = this
                .update(cx, |this, _| this.manual_persist_gen)
                .unwrap_or(gen);
            if latest == gen {
                Settings::update(move |s| s.post_shader.manual = manual);
            }
        })
        .detach();
        cx.notify();
    }

    /// The shader switch: into the file, then every shaded window
    /// reapplies, which also clears the pass when it goes off.
    /// Turning it on runs the countdown confirm; a shader can bury the
    /// very toggle that would undo it, so the change has to prove itself
    /// or roll back on its own. Off needs no proof.
    fn set_post_shader_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let prior = Settings::load().post_shader;
        Settings::update(move |s| s.post_shader.enabled = on);
        crate::workspace::apply_post_shader(cx);
        // Anything that resolves to a source is worth proving, whether it
        // came from a file, from a bundle's inline copy, or from the
        // workspace's pool. Nothing to run needs no countdown.
        if on
            && crate::workspace::post_shader_source(&prior)
                .ok()
                .flatten()
                .is_some()
        {
            self.confirm_post_shader(prior, cx);
        }
        cx.notify();
    }

    /// The all-windows switch: no confirm of its own, the countdown window
    /// stays out of the shading regardless.
    fn set_post_shader_all_windows(&mut self, on: bool, cx: &mut Context<Self>) {
        self.post_shader_all_windows = on;
        Settings::update(move |s| s.post_shader.all_windows = on);
        crate::workspace::apply_post_shader(cx);
        cx.notify();
    }

    fn set_post_shader_run_idle(&mut self, on: bool, cx: &mut Context<Self>) {
        self.post_shader_run_idle = on;
        Settings::update(move |s| s.post_shader.run_when_idle = on);
        crate::workspace::apply_post_shader(cx);
        cx.notify();
    }

    /// One edit to the screen shader's source trio: the copies, the file,
    /// the reapply, and the countdown when the change applies to a running
    /// shader. Every picker action funnels through here; `confirm` is
    /// false for the moves that change where the text is stored without
    /// changing what draws (detach, eject, save), which have nothing for
    /// a countdown to revert.
    fn edit_post_shader_source(
        &mut self,
        name: Option<String>,
        source: String,
        path: Option<PathBuf>,
        confirm: bool,
        cx: &mut Context<Self>,
    ) {
        let prior = Settings::load().post_shader;
        self.post_shader_name = name.clone();
        self.post_shader_source = source.clone();
        self.post_shader_path = path.clone();
        Settings::update(move |s| {
            s.post_shader.name = name;
            s.post_shader.source = source;
            s.post_shader.path = path;
        });
        crate::workspace::apply_post_shader(cx);
        if confirm && prior.enabled {
            self.confirm_post_shader(prior, cx);
        }
        cx.notify();
    }

    /// Take the screen shader off whatever it was on: no name, no source,
    /// no bookmark. The switch above stays its own decision, the panel
    /// pages' split.
    fn clear_post_shader_source(&mut self, cx: &mut Context<Self>) {
        self.edit_post_shader_source(None, String::new(), None, false, cx);
    }

    /// Load one of the shipped examples. Builtin, so nothing to approve.
    fn use_post_shader_example(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(preset) = panel::shader::PRESETS.get(index) else {
            return;
        };
        self.edit_post_shader_source(None, preset.source.to_string(), None, true, cx);
    }

    /// Point the screen at one of the workspace's shaders. Nothing is
    /// approved on the way through, the same as a panel picking a name: a
    /// pool entry that arrived with a bundle still has to be read first.
    fn use_post_shader_pool(&mut self, name: String, cx: &mut Context<Self>) {
        self.edit_post_shader_source(Some(name), String::new(), None, true, cx);
    }

    /// Open the in-app editor over the screen shader. A named one edits
    /// the pool entry; anything else edits the inline text, seeded from
    /// the file in file mode, and an apply lands as an inline source with
    /// the file kept as its bookmark. The write goes straight to the
    /// settings and the reapply rather than through this window, since
    /// the editor outlives it; the generation counter brings this page's
    /// copies along. No countdown: an apply is the user's own text, the
    /// same trust a hot reload from their editor gets.
    fn edit_post_shader_in_app(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        use panel::shader::edit::{EditKey, ShaderEditTarget};

        self.sync_post_shader();
        let target = match self.post_shader_name.as_deref() {
            Some(name) => ShaderEditTarget::pool(name),
            None => {
                let path = self.post_shader_path.clone();
                let source = if self.post_shader_source.trim().is_empty() {
                    path.as_deref()
                        .and_then(|path| std::fs::read_to_string(path).ok())
                        .unwrap_or_default()
                } else {
                    self.post_shader_source.clone()
                };
                let bookmark = path.clone();
                Some(ShaderEditTarget {
                    key: EditKey::Screen,
                    title: rox_i18n::t!("shader-editor-target-screen"),
                    source,
                    ctx: panel::shader::ProgramCtx::of(None, path.as_deref()),
                    path,
                    write: Arc::new(move |source, cx| {
                        let bookmark = bookmark.clone();
                        Settings::update(move |s| {
                            s.post_shader.name = None;
                            s.post_shader.source = source;
                            s.post_shader.path = bookmark;
                        });
                        crate::workspace::apply_post_shader(cx);
                    }),
                })
            }
        };
        let Some(target) = target else {
            return;
        };
        // The front workspace's state, the same bundle every other window
        // opened from here runs on.
        let Some((_, state)) = rox_panel_api::windows::front_workspace(cx) else {
            return;
        };
        crate::shader_editor::open(state, target, cx);
    }

    /// Take a private copy of the pool shader the screen is using. The
    /// same text keeps running, so there's nothing for a countdown to do.
    fn detach_post_shader(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self
            .post_shader_name
            .as_deref()
            .and_then(settings::shader_pool_get)
        else {
            return;
        };
        self.edit_post_shader_source(None, entry.source, None, false, cx);
    }

    /// Write the screen shader out to a file and hand it to whatever opens
    /// `.wgsl`, the panel pages' authoring loop. A named shader ejects
    /// through its pool entry, so the edits apply to every surface using
    /// the name; an inline one is written under the live workspace's
    /// shaders and the config moves onto the file, the one mode the
    /// screen driver's own watch hot reloads.
    fn eject_post_shader(&mut self, cx: &mut Context<Self>) {
        let config = Settings::load().post_shader;
        let ejected = match config.name.as_deref() {
            Some(name) => panel::shader::eject_pool_entry(name).map(|path| (path, false)),
            None if !config.source.trim().is_empty() => {
                let name = panel::shader::eject_name("Screen", &config.source);
                panel::shader::eject(&name, &config.source).map(|path| (path, true))
            }
            None => return,
        };
        match ejected {
            Ok((path, rebind)) => {
                if rebind {
                    self.edit_post_shader_source(
                        None,
                        String::new(),
                        Some(path.clone()),
                        false,
                        cx,
                    );
                }
                cx.open_with_system(&path);
            }
            Err(error) => {
                crate::workspace::note_post_shader_error(
                    rox_i18n::t!("shader-eject-failed", error = error.to_string()).to_string(),
                );
                cx.notify();
            }
        }
    }

    /// Promote the screen shader's own source into the workspace's shaders
    /// and use it by name from there, the panel pages' move. A file-mode
    /// config hands over the file's text and the bookmark moves onto the
    /// pool entry, so the authoring loop carries on through the pool's
    /// watch.
    fn save_post_shader_to_pool(&mut self, name: String, cx: &mut Context<Self>) {
        let config = Settings::load().post_shader;
        let name = name.trim().to_string();
        if name.is_empty() || config.name.is_some() {
            return;
        }
        let source = if !config.source.trim().is_empty() {
            config.source.clone()
        } else if let Some(path) = &config.path {
            match std::fs::read_to_string(path) {
                Ok(source) => source,
                Err(error) => {
                    crate::workspace::note_post_shader_error(format!(
                        "reading {}: {error}",
                        path.display()
                    ));
                    cx.notify();
                    return;
                }
            }
        } else {
            return;
        };
        panel::shader::save_to_pool(&name, &source, config.path.clone());
        self.edit_post_shader_source(Some(name), String::new(), None, false, cx);
    }

    /// Browse for the shader file. Picking one turns nothing on by itself;
    /// the toggle stays the one switch. A pick made while the shader
    /// runs takes visible effect, so that path runs the confirm too. The
    /// name and inline source go with it: both would win over the file at
    /// resolve time, so leaving either behind would make the pick a no-op.
    fn pick_post_shader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.edit_post_shader_source(None, String::new(), Some(path), true, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Put the just-applied shader on the countdown clock, with this
    /// window's copies refreshed if the clock wins.
    fn confirm_post_shader(&mut self, prior: settings::PostShaderConfig, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        crate::settings::shader_confirm::open(
            prior,
            self.player,
            self.now_art.clone(),
            move |cx| {
                weak.update(cx, |this, cx| {
                    let config = Settings::load().post_shader;
                    this.post_shader_name = config.name;
                    this.post_shader_source = config.source;
                    this.post_shader_path = config.path;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
    }

    /// Recompile the file as it stands, for shader edits the mtime watch
    /// missed (a same-second rewrite) or a nudge after fixing an error.
    fn reload_post_shader(&mut self, cx: &mut Context<Self>) {
        crate::workspace::apply_post_shader(cx);
        cx.notify();
    }

    /// The backdrop shader as the look holds it, the base of every read
    /// and edit on the Backdrop section. Absent reads as an untouched
    /// default with All Windows on: shading every backdrop is the whole-app
    /// read, and a look that leaves its children bare turns it off
    /// explicitly, the way Diffuse does.
    fn backdrop_config() -> settings::PostShaderConfig {
        settings::backdrop_shader().unwrap_or_else(|| settings::PostShaderConfig {
            all_windows: true,
            ..Default::default()
        })
    }

    /// One write to the backdrop config: the cache the workspace roots
    /// read, the look's bundle in the file, and a repaint so the shader
    /// follows the knob. No countdown confirm anywhere on this page: the
    /// panels paint over this pass whatever it does, so it can never bury
    /// the switch that would undo it.
    ///
    /// A config cleared all the way back to nothing collapses to None, so
    /// clearing the shader leaves no empty block in the look's exports.
    fn write_backdrop(&mut self, config: settings::PostShaderConfig, cx: &mut Context<Self>) {
        let config =
            (config.configured() || !config.routes.is_empty() || !config.manual.is_empty())
                .then_some(config);
        settings::note_backdrop_shader(config.clone());
        Settings::update(move |s| s.look.bundle.backdrop_shader = config);
        crate::workspace::refresh_backdrop(cx);
        cx.notify();
    }

    fn set_backdrop_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = Self::backdrop_config();
        config.enabled = on;
        self.write_backdrop(config, cx);
    }

    fn set_backdrop_run_idle(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = Self::backdrop_config();
        config.run_when_idle = on;
        self.write_backdrop(config, cx);
    }

    fn set_backdrop_all_windows(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = Self::backdrop_config();
        config.all_windows = on;
        self.write_backdrop(config, cx);
    }

    /// One edit to the backdrop's source trio. Every picker action funnels
    /// through here, the screen shader's shape without the countdown.
    fn edit_backdrop_source(
        &mut self,
        name: Option<String>,
        source: String,
        path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let mut config = Self::backdrop_config();
        config.name = name;
        config.source = source;
        config.path = path;
        self.write_backdrop(config, cx);
    }

    fn clear_backdrop_source(&mut self, cx: &mut Context<Self>) {
        self.edit_backdrop_source(None, String::new(), None, cx);
    }

    fn use_backdrop_example(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(preset) = panel::shader::PRESETS.get(index) else {
            return;
        };
        self.edit_backdrop_source(None, preset.source.to_string(), None, cx);
    }

    fn use_backdrop_pool(&mut self, name: String, cx: &mut Context<Self>) {
        self.edit_backdrop_source(Some(name), String::new(), None, cx);
    }

    /// Open the in-app editor over the backdrop shader, the screen
    /// shader's twin: a name edits the pool entry, anything else the
    /// inline text, written through the same cache-file-repaint trio as
    /// [`write_backdrop`](Self::write_backdrop), outside this window so
    /// the editor outlives it.
    fn edit_backdrop_in_app(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        use panel::shader::edit::{EditKey, ShaderEditTarget};

        let config = Self::backdrop_config();
        let target = match config.name.as_deref() {
            Some(name) => ShaderEditTarget::pool(name),
            None => {
                let path = config.path.clone();
                let source = if config.source.trim().is_empty() {
                    path.as_deref()
                        .and_then(|path| std::fs::read_to_string(path).ok())
                        .unwrap_or_default()
                } else {
                    config.source.clone()
                };
                let bookmark = path.clone();
                Some(ShaderEditTarget {
                    key: EditKey::Backdrop,
                    title: rox_i18n::t!("shader-editor-target-backdrop"),
                    source,
                    ctx: panel::shader::ProgramCtx::of(None, path.as_deref()),
                    path,
                    write: Arc::new(move |source, cx| {
                        let mut config = Self::backdrop_config();
                        config.name = None;
                        config.source = source;
                        config.path = bookmark.clone();
                        let config = Some(config);
                        settings::note_backdrop_shader(config.clone());
                        Settings::update(move |s| s.look.bundle.backdrop_shader = config);
                        crate::workspace::refresh_backdrop(cx);
                    }),
                })
            }
        };
        let Some(target) = target else {
            return;
        };
        // The front workspace's state, the same bundle every other window
        // opened from here runs on.
        let Some((_, state)) = rox_panel_api::windows::front_workspace(cx) else {
            return;
        };
        crate::shader_editor::open(state, target, cx);
    }

    /// Take a private copy of the pool shader the backdrop is using.
    fn detach_backdrop(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = Self::backdrop_config()
            .name
            .as_deref()
            .and_then(settings::shader_pool_get)
        else {
            return;
        };
        self.edit_backdrop_source(None, entry.source, None, cx);
    }

    /// Write the backdrop shader out to a file and hand it to whatever
    /// opens `.wgsl`. A named shader ejects through its pool entry; an
    /// inline one keeps its source and takes the file as a bookmark, which
    /// puts it under the surface's own watch, the panel pages' loop.
    fn eject_backdrop(&mut self, cx: &mut Context<Self>) {
        let config = Self::backdrop_config();
        match config.name.as_deref() {
            Some(name) => match panel::shader::eject_pool_entry(name) {
                Ok(path) => cx.open_with_system(&path),
                Err(error) => {
                    crate::workspace::note_backdrop_shader_error(
                        rox_i18n::t!("shader-eject-failed", error = error.to_string()).to_string(),
                    );
                    cx.notify();
                }
            },
            None if !config.source.trim().is_empty() => {
                let name = panel::shader::eject_name("Backdrop", &config.source);
                match panel::shader::eject(&name, &config.source) {
                    Ok(path) => {
                        self.edit_backdrop_source(
                            None,
                            config.source.clone(),
                            Some(path.clone()),
                            cx,
                        );
                        cx.open_with_system(&path);
                    }
                    Err(error) => {
                        crate::workspace::note_backdrop_shader_error(
                            rox_i18n::t!("shader-eject-failed", error = error.to_string())
                                .to_string(),
                        );
                        cx.notify();
                    }
                }
            }
            None => {}
        }
    }

    /// Promote the backdrop's own source into the workspace's shaders and
    /// use it by name from there.
    fn save_backdrop_to_pool(&mut self, name: String, cx: &mut Context<Self>) {
        let config = Self::backdrop_config();
        let name = name.trim().to_string();
        if name.is_empty() || config.name.is_some() || config.source.trim().is_empty() {
            return;
        }
        panel::shader::save_to_pool(&name, &config.source, config.path.clone());
        self.edit_backdrop_source(Some(name), String::new(), None, cx);
    }

    fn pick_backdrop_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            this.update(cx, |this, cx| this.load_backdrop_file(path, cx))
                .ok();
        })
        .detach();
    }

    /// Read a file into the config's inline source, with the path as the
    /// bookmark the surface watches. Inline rather than file-mode: the
    /// backdrop runs through the panel surface machinery, which resolves
    /// a name or an inline source and nothing else.
    fn load_backdrop_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                panel::shader::approve(&source);
                self.edit_backdrop_source(None, source, Some(path), cx);
            }
            Err(error) => {
                crate::workspace::note_backdrop_shader_error(format!(
                    "reading {}: {error}",
                    path.display()
                ));
                cx.notify();
            }
        }
    }

    /// Re-read the file behind the shader, for an edit the watch missed.
    fn reload_backdrop(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = Self::backdrop_config().path {
            self.load_backdrop_file(path, cx);
        }
    }

    /// One edit to the backdrop's routes: into the cache so the shader
    /// follows the drag, into the file once the burst settles.
    fn edit_backdrop_routes(
        &mut self,
        edit: &mut dyn FnMut(&mut Vec<Route>),
        cx: &mut Context<Self>,
    ) {
        let mut config = Self::backdrop_config();
        edit(&mut config.routes);
        settings::note_backdrop_shader(Some(config.clone()));
        crate::workspace::refresh_backdrop(cx);
        self.backdrop_route_persist_gen += 1;
        let gen = self.backdrop_route_persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let latest = this
                .update(cx, |this, _| this.backdrop_route_persist_gen)
                .unwrap_or(gen);
            if latest == gen {
                Settings::update(move |s| s.look.bundle.backdrop_shader = Some(config));
            }
        })
        .detach();
        cx.notify();
    }

    /// One hand-set slot edit, the routes' exact write path.
    fn set_backdrop_manual(&mut self, slot: usize, value: f32, cx: &mut Context<Self>) {
        let mut config = Self::backdrop_config();
        panel::shader::set_manual_value(&mut config.manual, slot, value);
        settings::note_backdrop_shader(Some(config.clone()));
        crate::workspace::refresh_backdrop(cx);
        self.backdrop_manual_persist_gen += 1;
        let gen = self.backdrop_manual_persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let latest = this
                .update(cx, |this, _| this.backdrop_manual_persist_gen)
                .unwrap_or(gen);
            if latest == gen {
                Settings::update(move |s| s.look.bundle.backdrop_shader = Some(config));
            }
        })
        .detach();
        cx.notify();
    }

    /// The Backdrop Shader section: the same surface machinery a panel
    /// uses, painted between the art wash and the panels, so whatever it
    /// does stays under the whole window. It's stored in the look's bundle
    /// rather than the machine settings and travels with the workspace.
    fn backdrop_shader_section(
        &mut self,
        q: &Query,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Section {
        let config = Self::backdrop_config();
        let enabled = config.enabled;
        let all_windows = config.all_windows;
        let run_idle = config.run_when_idle;
        let error = crate::workspace::backdrop_shader_error();
        let resolved = match config.name.as_deref() {
            Some(name) => settings::shader_pool_get(name).map(|entry| entry.source),
            None => (!config.source.trim().is_empty()).then(|| config.source.clone()),
        };
        let labels = panel::shader::slot_labels(resolved.as_deref().unwrap_or_default());
        let path = config.name.is_none().then(|| config.path.clone()).flatten();
        let picked = ShaderSource {
            id: "backdrop-shader",
            name: config.name.as_deref(),
            path: path.as_deref(),
            resolved: resolved.as_deref(),
            clear: Some(|this: &mut Self, cx| this.clear_backdrop_source(cx)),
            // Everything here paints under the panels, so nothing it does
            // can take the app: the list stays unfiltered, scenes and all.
            overlays_only: false,
            use_example: |this: &mut Self, index, cx| this.use_backdrop_example(index, cx),
            use_named: |this: &mut Self, name, cx| this.use_backdrop_pool(name, cx),
            choose_file: |this: &mut Self, window, cx| this.pick_backdrop_file(window, cx),
            edit: |this: &mut Self, window, cx| this.edit_backdrop_in_app(window, cx),
            eject: |this: &mut Self, cx| this.eject_backdrop(cx),
            detach: |this: &mut Self, cx| this.detach_backdrop(cx),
            reload: |this: &mut Self, cx| this.reload_backdrop(cx),
            save: |this: &mut Self, name, cx| this.save_backdrop_to_pool(name, cx),
            field: &mut self.backdrop_save_name,
            fallback: rox_i18n::t_static("settings-shader-backdrop-fallback-name"),
        }
        .render(window, cx);
        let hub = self.signals.clone();
        let slots = signal_ui::slots::SlotList {
            hub: &hub,
            routes: &config.routes,
            manual: &config.manual,
            labels: &labels,
            value_edit: &self.value_edit,
            scrubs: &self.backdrop_slot_scrubs,
            set: Arc::new(|this: &mut Self, slot, value, cx| {
                this.set_backdrop_manual(slot, value, cx)
            }),
        }
        .render(cx);
        let editor = signal_ui::routes::RouteEditor {
            id: "backdrop-shader-route",
            hub: &hub,
            routes: &config.routes,
            labels: &labels,
            value_edit: &self.value_edit,
            ui: &self.backdrop_route_ui,
            ui_mut: |this: &mut Self| &mut this.backdrop_route_ui,
            mutate: Arc::new(
                |this: &mut Self, edit: &mut dyn FnMut(&mut Vec<Route>), cx: &mut Context<Self>| {
                    this.edit_backdrop_routes(edit, cx);
                },
            ),
        };
        Section::new(
            q,
            icons::LAYERS,
            rox_i18n::t!("settings-shader-section-backdrop"),
            None,
            move |mut rows| {
                rows = rows
                    .keyed(
                        "settings-shader-backdrop-enabled",
                        &["shader", "wgsl", "backdrop", "wash", "art", "bokeh"],
                        panel::toggle(enabled, Self::set_backdrop_enabled, cx),
                    )
                    .custom(
                        &[
                            "shader",
                            "wgsl",
                            "file",
                            "reload",
                            "source",
                            "example",
                            "preset",
                            "workspace",
                        ],
                        || picked.into_any_element(),
                    )
                    .keyed(
                        "settings-shader-backdrop-all-windows",
                        &[
                            "shader",
                            "child windows",
                            "settings",
                            "everywhere",
                            "backdrop",
                        ],
                        panel::toggle(all_windows, Self::set_backdrop_all_windows, cx),
                    )
                    .keyed(
                        "settings-shader-backdrop-run-idle",
                        &["shader", "idle", "pause", "freeze"],
                        panel::toggle(run_idle, Self::set_backdrop_run_idle, cx),
                    );
                rows = match error {
                    Some(error) => rows.custom(&["shader", "error", "compile"], || {
                        match panel::shader::unsupported(&error) {
                            true => panel::banner(
                                panel::Tone::Bad,
                                panel::shader::NO_PIPELINE_TITLE,
                                vec![panel::shader::NO_PIPELINE_NOTE.into()],
                            ),
                            false => panel::banner(
                                panel::Tone::Bad,
                                rox_i18n::t!("settings-shader-compile-error-title"),
                                vec![error.into()],
                            ),
                        }
                        .into_any_element()
                    }),
                    None => rows,
                };
                rows.custom(
                    &[
                        "shader",
                        "signal",
                        "route",
                        "slot",
                        "bind",
                        "modulation",
                        "knob",
                        "manual",
                    ],
                    || {
                        let add = editor.add_button(cx);
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_MD)
                            .child(panel::setting_block(
                                rox_i18n::t!("settings-shader-signals-block"),
                                Some(rox_i18n::t!("settings-shader-signals-block.description")),
                                Some(add.into_any_element()),
                                editor.list(cx),
                            ))
                            .child(panel::setting_block(
                                rox_i18n::t!("settings-shader-slots-block"),
                                Some(rox_i18n::t!("settings-shader-slots-block.description")),
                                None,
                                slots,
                            ))
                            .into_any_element()
                    },
                )
            },
        )
    }

    /// The Icons section: the built-in set and every pack the user has as a
    /// list, each a set to switch to; the current one gets an Active
    /// badge. Creating a new pack, seeded with the built-in icons for an
    /// author to edit, is in the header.
    fn icons_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let active = self.active_icon_pack.clone();
        let packs = self.icon_packs.clone();

        // New-pack-from-name is in the header, so a pack is one name away
        // and arrives pre-filled with the current icons.
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(Input::new(&self.pack_name).small().w(px(150.)))
            .child(small_button(
                rox_i18n::t!("settings-appearance-new-pack"),
                icons::FOLDER_PLUS,
                false,
                cx.listener(|this, _, window, cx| this.create_pack(window, cx)),
            ));

        Section::new(
            q,
            icons::IMAGE,
            rox_i18n::t!("settings-appearance-section-icons"),
            Some(controls.into_any_element()),
            |rows| {
                rows.custom(&["icon pack", "svg", "glyphs", "built-in"], || {
                    let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("settings-appearance-icons-intro")),
                    );
                    // The built-in set heads the list, its own row so switching back is
                    // one click like any pack.
                    list = list.child(self.icon_pack_row(None, active.is_none(), cx));
                    list = list.child(
                        div().flex().flex_col().children(
                            packs
                                .into_iter()
                                .map(|name| {
                                    let is_active = active.as_deref() == Some(name.as_str());
                                    self.icon_pack_row(Some(name), is_active, cx)
                                })
                                .collect::<Vec<_>>(),
                        ),
                    );
                    list.into_any_element()
                })
            },
        )
    }

    /// One icons row: the built-in set (None) or a pack by name, an Active
    /// badge on the current one and a Use button on the rest. A pack also
    /// gets Open Folder, to edit its SVGs, and Delete.
    fn icon_pack_row(
        &self,
        name: Option<String>,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label: SharedString = name
            .clone()
            .map(SharedString::from)
            .unwrap_or_else(|| rox_i18n::t!("settings-common-built-in"));
        div()
            // Named after the pack, which names its buttons: every row
            // here says Use and Open Folder. See
            // `rox_panel_kit::ui::control_focus`.
            .id(ElementId::Name(format!("icon-pack-row:{label}").into()))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .child(div().flex_1().min_w_0().truncate().child(label.clone()))
            .map(|d| {
                if active {
                    d.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("settings-common-active")),
                    )
                } else {
                    d.child(small_button(
                        rox_i18n::t!("settings-common-use"),
                        icons::CHECK,
                        false,
                        {
                            let name = name.clone();
                            cx.listener(move |this, _, _, cx| this.set_icon_pack(name.clone(), cx))
                        },
                    ))
                }
            })
            .when_some(name, |d, name| {
                // Open Folder reveals the pack so its SVGs can be edited in
                // place; delete drops the folder and everything in it.
                d.child(small_button(
                    rox_i18n::t!("settings-appearance-icons-open-folder"),
                    icons::FOLDER,
                    false,
                    {
                        let name = name.clone();
                        cx.listener(move |this, _, _, cx| this.reveal_pack(&name, cx))
                    },
                ))
                .child(icon_button(icons::TRASH, false, {
                    cx.listener(move |this, _, _, cx| this.delete_pack(&name, cx))
                }))
            })
            .into_any_element()
    }

    /// Switch the active icon pack, or the built-in set for None. Persists
    /// the pick and points the resolver at it; icons already on screen keep
    /// their tiles until the next launch, so the switch reads as pending.
    fn set_icon_pack(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        crate::startup::icon_packs::activate(name.as_deref());
        self.active_icon_pack = name.clone();
        self.icon_packs = crate::startup::icon_packs::all();
        let persist = name.clone();
        Settings::update(move |s| s.icon_pack = persist);
        // Repaint every window so any not-yet-cached icon picks up the pack.
        for window in cx.windows() {
            window.update(cx, |_, window, _| window.refresh()).ok();
        }
        cx.notify();
    }

    /// Create a new pack from the name field, seeded with the built-in
    /// icons, and switch to it. Clears the field on success; an empty name
    /// takes a default, and a collision gets a numbered suffix.
    fn create_pack(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.pack_name.read(cx).value().trim().to_string();
        match crate::startup::icon_packs::create(&name) {
            Ok(created) => {
                self.pack_name
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.set_icon_pack(Some(created), cx);
            }
            Err(e) => log::warn!("icon pack: creating {name:?}: {e}"),
        }
    }

    /// Delete a pack. If it was the active one, fall back to the built-in
    /// set so the resolver never points at a folder that's gone.
    fn delete_pack(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.active_icon_pack.as_deref() == Some(name) {
            self.set_icon_pack(None, cx);
        }
        crate::startup::icon_packs::delete(name);
        self.icon_packs = crate::startup::icon_packs::all();
        cx.notify();
    }

    /// Reveal a pack's folder in the OS file manager, so its SVGs can be
    /// swapped out with a text or vector editor.
    fn reveal_pack(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(dir) = crate::startup::icon_packs::resolve_dir(name) {
            cx.reveal_path(&dir);
        }
    }

    /// Everything that shapes the samples on their way to the device, in the
    /// order the audio meets it: the chain first (ADR 19), then the backend
    /// that hands it over.
    fn audio_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new()
            .section(Section::new(
                q,
                icons::AUDIO_LINES,
                rox_i18n::t!("settings-audio-section-playback"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-audio-transport",
                        &["play", "pause", "seek", "random", "preview"],
                        panel::transport_strip(&self.playback, &self.library, cx),
                    )
                    .custom(
                        &["crossfade", "fade", "gapless", "overlap", "transition"],
                        || self.crossfade_row(cx).into_any_element(),
                    )
                    .custom(&["crossfade", "fade", "gapless", "album", "splice"], || {
                        self.crossfade_albums_row(cx).into_any_element()
                    })
                },
            ))
            .section(self.replay_gain_section(q, cx))
            .section(Section::new(
                q,
                icons::SLIDERS,
                rox_i18n::t!("settings-audio-section-equalizer"),
                Some(
                    small_button(
                        rox_i18n::t!("settings-audio-open-equalizer"),
                        icons::AUDIO_LINES,
                        false,
                        cx.listener(|_, _, _, cx| crate::eq_window::open(cx)),
                    )
                    .into_any_element(),
                ),
                |rows| {
                    rows.custom(&["eq", "bands", "bass", "treble", "tone"], || {
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("settings-audio-equalizer-note"))
                            .into_any_element()
                    })
                },
            ))
            .section(self.output_section(q, cx))
    }

    /// How long one track overlaps the next. Zero is off, which is the
    /// gapless boundary rox has always had; anything else fades only where
    /// the music isn't continuous, so an album still splices.
    fn crossfade_row(&self, cx: &mut Context<Self>) -> Div {
        panel::setting_row(
            rox_i18n::t!("settings-audio-crossfade"),
            Some(rox_i18n::t!("settings-audio-crossfade.description")),
            settings_ui::scalar(
                &self.crossfade_scrub,
                &self.value_edit,
                self.playback.read(cx).crossfade_secs(),
                settings_ui::span(0., engine::CROSSFADE_MAX_SECS, " s")
                    .decimals(1)
                    .hard(),
                |this: &mut Self, secs, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_crossfade_secs(secs, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// Whether the fade takes an album's own boundaries too. Inert while
    /// the fade is off, since there'd be nothing for it to change.
    fn crossfade_albums_row(&self, cx: &mut Context<Self>) -> Div {
        let player = self.playback.read(cx);
        let on = player.crossfade_albums();
        let control: AnyElement = if player.crossfade_secs() > 0.0 {
            panel::toggle(
                on,
                |this: &mut Self, on, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_crossfade_albums(on, cx));
                    cx.notify();
                },
                cx,
            )
            .into_any_element()
        } else {
            panel::toggle_locked(on).into_any_element()
        };
        panel::setting_row(
            rox_i18n::t!("settings-audio-fade-inside-albums"),
            Some(rox_i18n::t!(
                "settings-audio-fade-inside-albums.description"
            )),
            control,
        )
    }

    /// The ReplayGain section: which of a file's two gains to level by, the
    /// two offsets around it, and where the measurement pass puts what it
    /// measures. The offsets only show once a mode is picked, since with
    /// leveling off there's nothing for them to offset.
    fn replay_gain_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let modes: Vec<(SharedString, GainModeSetting)> = vec![
            (
                rox_i18n::t!("settings-audio-replaygain-mode-off"),
                GainModeSetting::Off,
            ),
            (
                rox_i18n::t!("settings-audio-replaygain-mode-track"),
                GainModeSetting::Track,
            ),
            (
                rox_i18n::t!("settings-audio-replaygain-mode-album"),
                GainModeSetting::Album,
            ),
        ];
        let rg = self.playback.read(cx).replay_gain();
        // A running pass takes over the line under the section: its count, the
        // file it's on, and whatever it had to skip. With nothing scanned
        // there's no coverage to state either.
        let split = self.rg_coverage;
        let total = split.total();
        let note: Option<String> = if let Some(job) = &self.rg_job {
            Some(Self::measure_progress_line(job))
        } else if total == 0 {
            None
        } else if split.covered() == 0 {
            Some(format!(
                "None of the {total} tracks scanned have a ReplayGain to level by. Measure \
                 Missing analyzes them and saves the numbers{}",
                self.rg_estimate_suffix(split.missing)
            ))
        } else if split.missing > 0 {
            Some(format!(
                "{} of {total} scanned tracks have a gain to level by, {} of them measured \
                 by rox. The other {} play at the untagged setting{}",
                split.covered(),
                split.measured,
                split.missing,
                self.rg_estimate_suffix(split.missing),
            ))
        } else if split.measured > 0 {
            Some(
                rox_i18n::t!(
                    "settings-audio-replaygain-status-measured",
                    total = total,
                    measured = split.measured
                )
                .to_string(),
            )
        } else {
            Some(rox_i18n::t!("settings-audio-replaygain-status-tagged", total = total).to_string())
        };
        Section::new(
            q,
            icons::GAUGE,
            rox_i18n::t!("settings-audio-section-replaygain"),
            Some(self.measure_control(cx)),
            |rows| {
                let rows = rows
                    .keyed(
                        "settings-audio-replaygain-level-by",
                        &["volume", "normalization", "loudness", "leveling"],
                        panel::choices_shared(
                            &modes,
                            rg.mode,
                            |this: &mut Self, mode, cx| {
                                this.playback
                                    .update(cx, |player, cx| player.set_replay_gain_mode(mode, cx));
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                    .when(rg.mode != GainModeSetting::Off, |rows| {
                        rows.keyed(
                            "settings-audio-replaygain-preamp",
                            &["volume", "gain", "boost", "loudness"],
                            settings_ui::scalar(
                                &self.preamp_scrub,
                                &self.value_edit,
                                rg.preamp_db,
                                settings_ui::span(-15., 15., " dB").decimals(1).hard(),
                                |this: &mut Self, db, cx| {
                                    this.playback.update(cx, |player, cx| {
                                        player.set_replay_gain_preamp(db, cx)
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        )
                        .keyed(
                            "settings-audio-replaygain-untagged",
                            &["fallback", "default gain", "missing"],
                            settings_ui::scalar(
                                &self.fallback_scrub,
                                &self.value_edit,
                                rg.fallback_db,
                                settings_ui::span(-15., 15., " dB").decimals(1).hard(),
                                |this: &mut Self, db, cx| {
                                    this.playback.update(cx, |player, cx| {
                                        player.set_replay_gain_fallback(db, cx)
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        )
                    })
                    .keyed(
                        "settings-audio-replaygain-save",
                        &["write", "tags", "database", "analysis"],
                        panel::choices_shared(
                            &[
                                (
                                    rox_i18n::t!("settings-common-database"),
                                    ReplayGainSave::Database,
                                ),
                                (rox_i18n::t!("settings-common-tags"), ReplayGainSave::Tags),
                            ],
                            rg.save,
                            Self::set_replay_gain_save,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-audio-replaygain-measure-new",
                        &["automatic", "auto", "new files", "watch"],
                        panel::toggle(rg.auto, Self::set_replay_gain_auto, cx),
                    );
                match note {
                    Some(note) => rows
                        .custom(&["coverage", "measure", "missing", "progress"], || {
                            coverage_note(note).into_any_element()
                        }),
                    None => rows,
                }
            },
        )
    }

    /// Where a measured gain saves. Through the player like the other three
    /// leveling knobs, since it holds the live copy of the whole struct.
    fn set_replay_gain_save(&mut self, save: ReplayGainSave, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |player, cx| player.set_replay_gain_save(save, cx));
        cx.notify();
    }

    /// The follow-the-watcher switch, through the player like the rest of the
    /// section. On the way on it asks about the backlog: the pass's work list
    /// is everything with no gain, so a switch flipped over a library nobody
    /// has measured would start hours of decoding at the next watch sync
    /// without anyone having seen a number first. The prompt prices that
    /// backlog and measures it now; declining is a no to the switch too, and
    /// comes back here through `pass_refused`.
    ///
    /// Nothing to ask about with nothing missing, or with a pass already
    /// working through it, so the switch just goes on.
    fn set_replay_gain_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |player, cx| player.set_replay_gain_auto(on, cx));
        if on && self.rg_coverage.missing > 0 && self.rg_job.is_none() {
            let library = self.library.clone();
            pass_prompt::raise_for_switch(self, pass_prompt::Pass::ReplayGain, library, cx);
        }
        cx.notify();
    }

    /// The section header's control: start the pass, or stop the one that's
    /// running. Inert with nothing missing, and while the library is busy
    /// scanning, since a scan is rewriting the very rows the pass reads.
    fn measure_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = &self.rg_job {
            let stopping = job.stopping();
            return small_button(
                if stopping {
                    rox_i18n::t!("settings-common-stopping")
                } else {
                    rox_i18n::t!("settings-common-stop")
                },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| replaygain_job::stop(cx)),
            )
            .into_any_element();
        }
        let idle = self.rg_coverage.missing == 0 || self.library.read(cx).busy().is_some();
        small_button(
            rox_i18n::t!("settings-audio-replaygain-measure-missing-button"),
            icons::GAUGE,
            idle,
            cx.listener(|this, _, _, cx| {
                let library = this.library.clone();
                pass_prompt::raise(this, pass_prompt::Pass::ReplayGain, library, cx);
            }),
        )
        .into_any_element()
    }

    /// The Favourites section's header control: start the loved-tracks
    /// import, or stop the one that's running. The ReplayGain section's
    /// control in every respect but the work, since it's the same shape of
    /// thing: a job started from a page that doesn't have to stay open for
    /// it. Inert with its reason on the line below, so a disconnected
    /// account reads as a state rather than a dead button.
    fn import_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = import::progress(cx) {
            let stopping = job.stopping();
            return small_button(
                if stopping {
                    rox_i18n::t!("settings-common-stopping")
                } else {
                    rox_i18n::t!("settings-common-stop")
                },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| import::stop(cx)),
            )
            .into_any_element();
        }
        small_button(
            rox_i18n::t!("settings-integrations-lastfm-import-loved"),
            icons::DOWNLOAD,
            import::blocked_reason(cx).is_some(),
            cx.listener(|this, _, _, cx| {
                import::start(this.library.clone(), this.scrobbler.clone(), cx);
            }),
        )
        .into_any_element()
    }

    /// Copy the running pass into the section, the scan badge's cadence.
    /// Stops itself once the pass clears the global.
    fn poll_measuring(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(RG_POLL).await;
            let live = this.update(cx, |this, cx| {
                let was = this.rg_job.is_some();
                this.rg_job = replaygain_job::progress(cx);
                // The pass that just ended wrote what it measured per file;
                // pick it up so the next estimate prices off it.
                if was && this.rg_job.is_none() {
                    this.rg_pace = Settings::load().session.replaygain_pace;
                }
                cx.notify();
                this.rg_job.is_some()
            });
            if !matches!(live, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// A rough cost for measuring `missing` files at the current worker
    /// setting, ready to append to the coverage line, or nothing until a
    /// pass has measured this machine's pace. Off the last pass's own
    /// average, so it prices these files on this disk rather than an
    /// imagined library.
    fn rg_estimate_suffix(&self, missing: u64) -> String {
        match rox_core::pace::estimate(self.rg_pace, missing, self.rg_workers) {
            Some(estimate) => format!(
                " {}",
                rox_i18n::t!(
                    "tasks-estimate-at-workers",
                    estimate = estimate,
                    workers = rox_core::pace::workers_phrase(self.rg_workers)
                )
            ),
            None => String::new(),
        }
    }

    /// The running pass as one line: how far along, what it's on, and what
    /// it gave up on. The work list is built first, so a zero total means
    /// the pass hasn't finished building it.
    fn measure_progress_line(job: &replaygain_job::Progress) -> String {
        let total = job.total();
        if total == 0 {
            return rox_i18n::t!("settings-audio-replaygain-measuring-start").to_string();
        }
        let mut line = rox_i18n::t!(
            "settings-audio-replaygain-measuring-progress",
            done = job.done().min(total) as u64,
            total = total as u64
        )
        .to_string();
        if let Some(eta) = job.eta_secs() {
            line.push_str(&rox_i18n::t!(
                "tasks-time-left",
                left = rox_core::pace::human(eta)
            ));
        }
        let current = job.current();
        if let Some(name) = Path::new(&current).file_name() {
            line.push_str(&format!(
                " {}",
                rox_i18n::t!(
                    "tasks-file-suffix",
                    file = name.to_string_lossy().to_string()
                )
            ));
        }
        let failed = job.failed();
        if failed > 0 {
            line.push_str(&format!(
                " {}",
                rox_i18n::t!("tasks-failed-suffix", count = failed as u64)
            ));
        }
        line
    }

    /// Whether this platform's exclusive backend has ever been run by us on
    /// real hardware. ALSA and CoreAudio have; the WASAPI backend is written
    /// from the platform contract and shipped for testers, which is exactly
    /// what the badge and the issue link say.
    fn exclusive_experimental() -> bool {
        cfg!(target_os = "windows")
    }

    /// The prefilled new-issue page for exclusive-mode reports: the platform
    /// and version filled in, plus what the stream negotiated if one is up,
    /// so a report from a tester arrives with the part they'd forget.
    fn exclusive_issue_url(&self, cx: &Context<Self>) -> String {
        let negotiated = self
            .playback
            .read(cx)
            .output_status()
            .map(|status| {
                let negotiated = status.negotiated;
                format!(
                    "{:?} on {}, {} Hz, {} ch, {}{}",
                    negotiated.mode,
                    negotiated.device,
                    negotiated.sample_rate,
                    negotiated.channels,
                    negotiated.format,
                    negotiated
                        .fallback
                        .map(|why| format!("\nFallback reason: {why}"))
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_else(|| "Nothing playing".into());
        let title = format!("Exclusive output on {}: ", std::env::consts::OS);
        let body = format!(
            "rox {} on {} ({})\n\nNegotiated: {}\n\nWhat happened:\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            negotiated,
        );
        format!(
            "https://github.com/zealsprince/rox/issues/new?title={}&body={}",
            urlencode(&title),
            urlencode(&body)
        )
    }

    /// The badge and its report button are in the Output header rather than
    /// the Exclusive Mode row: they're about the whole backend, not the switch,
    /// and the header's right edge is where a section-wide caveat belongs.
    /// Returns None where nothing is being warned about.
    fn exclusive_notice(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !output::exclusive_supported() || !Self::exclusive_experimental() {
            return None;
        }
        let url = self.exclusive_issue_url(cx);
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(
                    div()
                        .id("exclusive-experimental")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .px(tokens::SPACE_SM)
                        .py(px(2.))
                        .rounded(tokens::RADIUS)
                        .bg(palette::alpha(palette::tone_warn(), 0x1c))
                        .text_xs()
                        .text_color(palette::tone_warn())
                        .child(
                            svg()
                                .path(icons::FLASK)
                                .size(px(12.))
                                .text_color(palette::tone_warn()),
                        )
                        .child(rox_i18n::t!("settings-audio-output-experimental-badge"))
                        .tooltip(|_, cx| {
                            cx.new(|_| {
                                ExperimentalTooltip(rox_i18n::t!(
                                    "settings-audio-output-experimental-tooltip"
                                ))
                            })
                            .into()
                        }),
                )
                .child(
                    div()
                        .id("exclusive-issue")
                        .child(settings_ui::icon_button(
                            icons::EXTERNAL_LINK,
                            false,
                            move |_, _, cx| cx.open_url(&url),
                        ))
                        .tooltip(|_, cx| {
                            cx.new(|_| {
                                ExperimentalTooltip(rox_i18n::t!(
                                    "settings-audio-output-issue-tooltip"
                                ))
                            })
                            .into()
                        }),
                )
                .into_any_element(),
        )
    }

    /// The Output section: the exclusive switch, the device list for
    /// whichever backend that picks, and what the running stream actually
    /// negotiated. The readout is the point of the section: the two rows
    /// above it are requests, and ADR 19 asks the UI to state the reality
    /// rather than repeat the ask.
    fn output_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        // Where no exclusive backend is built there's nothing to toggle:
        // every claim would fall back, and a switch that never does
        // anything reads as a bug in the hardware rather than a gap in rox.
        let exclusive: AnyElement = if output::exclusive_supported() {
            panel::toggle(self.output_exclusive, Self::set_output_exclusive, cx).into_any_element()
        } else {
            readout(rox_i18n::t!("settings-audio-output-not-built").to_string()).into_any_element()
        };
        Section::new(
            q,
            icons::VOLUME_2,
            rox_i18n::t!("settings-audio-section-output"),
            self.exclusive_notice(cx),
            |rows| {
                rows.keyed(
                    "settings-audio-exclusive-mode",
                    &["bit perfect", "wasapi", "asio", "hog"],
                    exclusive,
                )
                .custom(
                    &["device", "soundcard", "headphones", "interface", "rescan"],
                    || self.output_devices_block(cx).into_any_element(),
                )
                .custom(&["sample rate", "hz", "khz", "resample"], || {
                    self.output_rate_row(cx).into_any_element()
                })
                .custom(&["format", "bit depth", "float", "integer"], || {
                    self.output_format_row(cx).into_any_element()
                })
                .custom(&["buffer", "latency", "period", "underrun"], || {
                    self.output_period_row(cx).into_any_element()
                })
                .custom(&["status", "negotiated", "stream", "fallback"], || {
                    self.output_status_block(cx).into_any_element()
                })
            },
        )
    }

    /// The three hardware knobs below only mean anything on a device rox
    /// holds alone. In shared mode the server owns the rate, the format and
    /// the buffer, so they draw inert rather than pretending.
    fn exclusive_only(&self) -> bool {
        !self.output_exclusive || !output::exclusive_supported()
    }

    /// The rate the device runs at: following each file's own lets a
    /// mixed-rate library play without a resampler anywhere, so it leads.
    fn output_rate_row(&self, cx: &mut Context<Self>) -> Div {
        let mut options: Vec<(Option<u32>, SharedString)> =
            vec![(None, rox_i18n::t!("settings-audio-output-rate-follow"))];
        options.extend(RATES.iter().map(|hz| {
            (
                Some(*hz),
                rox_i18n::format::format_unit(f64::from(*hz) / 1000.0, 1, "kHz").into(),
            )
        }));
        panel::setting_row(
            rox_i18n::t!("settings-audio-output-sample-rate"),
            Some(rox_i18n::t!(
                "settings-audio-output-sample-rate.description"
            )),
            panel::picker(
                "output-rate",
                self.playback.read(cx).output_rate(),
                options,
                self.exclusive_only(),
                |this: &mut Self, rate, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_output_rate(rate, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// The sample format asked for. Widest-available is right almost always;
    /// the pick exists for a card whose driver works better on one of them.
    fn output_format_row(&self, cx: &mut Context<Self>) -> Div {
        let options: Vec<(Option<String>, SharedString)> = vec![
            (None, rox_i18n::t!("settings-audio-output-format-widest")),
            (
                Some("f32".into()),
                rox_i18n::t!("settings-audio-output-format-f32"),
            ),
            (
                Some("s32".into()),
                rox_i18n::t!("settings-audio-output-format-s32"),
            ),
            (
                Some("s16".into()),
                rox_i18n::t!("settings-audio-output-format-s16"),
            ),
        ];
        panel::setting_row(
            rox_i18n::t!("settings-audio-output-format"),
            Some(rox_i18n::t!("settings-audio-output-format.description")),
            panel::picker(
                "output-format",
                self.playback.read(cx).output_format().map(str::to_string),
                options,
                self.exclusive_only(),
                |this: &mut Self, format, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_output_format(format, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// The period, the latency trade stated plainly.
    fn output_period_row(&self, cx: &mut Context<Self>) -> Div {
        let mut options: Vec<(Option<f64>, SharedString)> =
            vec![(None, rox_i18n::t!("settings-audio-output-buffer-default"))];
        options.extend(PERIODS_MS.iter().map(|ms| {
            (
                Some(*ms),
                rox_i18n::format::format_unit(*ms, 1, "ms").into(),
            )
        }));
        panel::setting_row(
            rox_i18n::t!("settings-audio-output-buffer"),
            Some(rox_i18n::t!("settings-audio-output-buffer.description")),
            panel::picker(
                "output-period",
                self.playback.read(cx).output_period(),
                options,
                self.exclusive_only(),
                |this: &mut Self, ms, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_output_period(ms, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// The device picker for the mode that's on, the system default at the
    /// head so switching back is one pick. Rescan is beside it because the
    /// list is taken when the window opens: plugging an interface in while
    /// it's up shouldn't mean closing and reopening.
    fn output_devices_block(&self, cx: &mut Context<Self>) -> Div {
        let mut options: Vec<(Option<String>, SharedString)> = vec![(
            None,
            rox_i18n::t!("settings-audio-output-device-system-default"),
        )];
        options.extend(
            self.output_devices
                .iter()
                .map(|device| (Some(device.id.clone()), device.name.clone().into())),
        );
        // The toggle swaps which backend's list this is, and on Linux the two
        // don't even overlap: exclusive enumerates kernel sound cards, and a
        // Bluetooth headset only exists inside the sound server. The note has
        // to say so, or a device that was just here reads as lost.
        let description = if self.exclusive_only() {
            rox_i18n::t!("settings-audio-output-device.description-default")
        } else if cfg!(target_os = "linux") {
            rox_i18n::t!("settings-audio-output-device.description-linux")
        } else {
            rox_i18n::t!("settings-audio-output-device.description-other")
        };
        panel::setting_row(
            rox_i18n::t!("settings-audio-output-device"),
            Some(description),
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(panel::picker(
                    "output-device",
                    self.playback.read(cx).output_device().map(str::to_string),
                    options,
                    false,
                    |this: &mut Self, id, cx| this.set_output_device(id, cx),
                    cx,
                ))
                .child(small_button(
                    rox_i18n::t!("settings-common-rescan"),
                    icons::REFRESH_CW,
                    false,
                    cx.listener(|this, _, _, cx| this.rescan_output_devices(cx)),
                )),
        )
    }

    /// What the stream negotiated, in plain words. Nothing here is derived
    /// from the settings above: a fallback line only appears because a
    /// backend reported one, and the rate line compares the device's rate
    /// against the file's rather than against what was asked for.
    fn output_status_block(&self, cx: &mut Context<Self>) -> Div {
        let Some(status) = self.playback.read(cx).output_status() else {
            // No stream and an error means the last open failed, which is a
            // different thing from an idle player and shouldn't read the
            // same: one is waiting, the other is broken.
            return match self.playback.read(cx).error() {
                Some(error) => panel::banner(
                    panel::Tone::Bad,
                    rox_i18n::t!("settings-audio-output-status-error-title"),
                    vec![
                        error,
                        rox_i18n::t!("settings-audio-output-status-error-hint"),
                    ],
                ),
                None => panel::banner(
                    panel::Tone::Info,
                    rox_i18n::t!("settings-audio-output-status-idle-title"),
                    vec![rox_i18n::t!("settings-audio-output-status-idle-hint")],
                ),
            };
        };
        let negotiated = &status.negotiated;
        let mode = match negotiated.mode {
            output::Mode::Exclusive => rox_i18n::t_static("settings-audio-output-mode-exclusive"),
            output::Mode::Shared => rox_i18n::t_static("settings-audio-output-mode-shared"),
        };
        let resampling = status
            .source_rate
            .is_some_and(|source| source != negotiated.sample_rate);
        // The tone is the whole point of the callout, and the two bad cases
        // aren't the same size. A claim that failed is a setting that didn't
        // take, which is an error: exclusive is switched on and you aren't
        // hearing it. Resampling is the mode working and still not being
        // bit-perfect, which is worth flagging without crying wolf.
        let tone = if negotiated.fallback.is_some() {
            panel::Tone::Bad
        } else if resampling {
            panel::Tone::Warn
        } else {
            panel::Tone::Good
        };
        // The experimental note goes in the banner too: someone reading only
        // the status line should know the mode they're hearing is the one
        // nobody has hardware-tested.
        let experimental =
            negotiated.mode == output::Mode::Exclusive && Self::exclusive_experimental();
        let headline = rox_i18n::t!(
            "settings-audio-output-headline",
            mode = mode.to_string(),
            note = if experimental {
                rox_i18n::t!("settings-audio-output-experimental").to_string()
            } else {
                String::new()
            },
            device = negotiated.device.clone(),
            rate = negotiated.sample_rate as u64,
            channels = negotiated.channels as u64,
            format = negotiated.format.to_string()
        )
        .to_string();
        // The expanded register: this block has a page to itself, so each
        // reason keeps a sentence of its own where the output panel folds
        // them into one line.
        panel::banner(tone, headline, status.lines(true, true))
    }

    /// Ask for exclusive output, or give the device back. The player
    /// rebuilds its running session onto the other backend right here, so
    /// the switch takes effect without a restart, and the device list is
    /// the other backend's from this point.
    fn set_output_exclusive(&mut self, on: bool, cx: &mut Context<Self>) {
        self.output_exclusive = on;
        self.playback
            .update(cx, |player, cx| player.set_exclusive_output(on, cx));
        self.output_devices = output::devices(output_mode(on));
        cx.notify();
    }

    /// Pick a device for the mode that's on, None for the system default.
    fn set_output_device(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |player, cx| player.set_output_device(id, cx));
        cx.notify();
    }

    /// Re-enumerate, for an interface plugged in while this window is open.
    fn rescan_output_devices(&mut self, cx: &mut Context<Self>) {
        self.output_devices = output::devices(output_mode(self.output_exclusive));
        cx.notify();
    }

    /// The Application page: how the app itself behaves, from the AI gate
    /// through launch, layout, window residency, where the data is kept,
    /// and the control socket under it all. Everything about how the music
    /// plays is on the Playback page instead.
    fn application_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        // The portable row's control depends on the state: inert text
        // where the exe folder can't take writes or while the seed copy
        // runs, the live switch otherwise.
        let portable_control: AnyElement = if !self.portable_writable {
            readout(rox_i18n::t!("settings-application-portable-not-writable").to_string())
                .into_any_element()
        } else if self.portable_busy {
            readout(rox_i18n::t!("settings-application-portable-copying").to_string())
                .into_any_element()
        } else {
            panel::toggle(self.portable, Self::set_portable, cx).into_any_element()
        };
        let mut portable_row =
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
                .child(panel::setting_row(
                    rox_i18n::t!("settings-application-portable-mode"),
                    Some(rox_i18n::t!(
                        "settings-application-portable-mode.description"
                    )),
                    portable_control,
                ));
        // The restart note keys on the marker not matching the run, not
        // on a flip this session: it stays up across window reopens
        // until a launch actually applies the change.
        if self.portable != settings::portable() && !self.portable_busy {
            portable_row = portable_row.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-application-portable-restart-note")),
            );
        }
        PageBody::new()
            // At the head of the page rather than sorted in: it's the gate
            // for two whole pages (MCP, ML Models), and a gate that hides
            // below the fold is a setting people ask where to find.
            .section(Section::new(
                q,
                icons::LINK,
                rox_i18n::t!("settings-application-section-ai"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-application-enable-ai",
                        &["ai", "mcp", "agent", "assistant", "llm", "model"],
                        panel::toggle(self.ai_enabled, Self::set_ai_enabled, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::PLAY,
                rox_i18n::t!("settings-application-section-startup"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-application-check-updates",
                        &["release", "version", "upgrade"],
                        panel::toggle(self.check_updates, Self::set_check_updates, cx),
                    )
                    // Meaningless where the install can't replace itself (a
                    // distro package, a read-only folder), so the row only
                    // exists where the updater can act on it.
                    .when(updater::can_update(), |rows| {
                        rows.keyed(
                            "settings-application-download-updates",
                            &["release", "download", "auto", "update"],
                            panel::toggle(self.download_updates, Self::set_download_updates, cx),
                        )
                    })
                },
            ))
            .section(Section::new(
                q,
                icons::LAYOUT_DASHBOARD,
                rox_i18n::t!("settings-application-section-layout"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-application-lock-panel-resize",
                        &["resize", "lock", "design", "drag", "seam"],
                        panel::toggle(settings::resize_lock(), Self::set_resize_lock, cx),
                    )
                },
            ))
            // A resident process with no way back in is worse than quitting,
            // so the row only exists where something can bring a window back.
            .when(tray::supported(), |page| {
                page.section(Section::new(
                    q,
                    icons::APP_WINDOW,
                    rox_i18n::t!("settings-application-section-window"),
                    None,
                    |rows| {
                        rows.keyed(
                            "settings-application-remain-in-tray",
                            &["quit", "minimize", "background"],
                            panel::toggle(settings::quit_to_tray(), Self::set_quit_to_tray, cx),
                        )
                    },
                ))
            })
            .section(Section::new(
                q,
                icons::DATABASE,
                rox_i18n::t!("settings-application-section-data"),
                None,
                |rows| {
                    rows.custom(&["portable mode", "usb", "folder", "executable"], || {
                        portable_row.into_any_element()
                    })
                },
            ))
            // Here rather than on the MCP page: the socket is rox's one
            // machine interface, and rox-mcp is just one of its callers.
            .section(Section::new(
                q,
                icons::LINK,
                rox_i18n::t!("settings-application-section-control-socket"),
                None,
                |rows| {
                    rows.custom(&["socket", "ipc", "control", "roxctl", "mcp"], || {
                        let path = rox_ipc::socket_path(&settings::data_dir());
                        let text = path.display().to_string();
                        let copy = text.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(panel::setting_row(
                                rox_i18n::t!("settings-application-socket-path"),
                                Some(rox_i18n::t!("settings-application-socket-path.description")),
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(tokens::SPACE_SM)
                                    .child(small_button(
                                        rox_i18n::t!("settings-common-copy"),
                                        icons::COPY,
                                        false,
                                        move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy.clone(),
                                            ));
                                        },
                                    ))
                                    // A named pipe isn't in the filesystem,
                                    // so Windows has nothing to reveal.
                                    .when(!cfg!(windows), |d| {
                                        d.child(small_button(
                                            rox_i18n::t!("settings-common-reveal"),
                                            icons::FOLDER,
                                            false,
                                            move |_, _, cx| {
                                                cx.reveal_path(&path);
                                            },
                                        ))
                                    })
                                    .into_any_element(),
                            ))
                            // The path on its own line rather than squeezed
                            // beside the buttons: runtime dirs run long, and a
                            // readout that truncates is a readout that lies.
                            .child(readout(text))
                            .into_any_element()
                    })
                },
            ))
    }

    /// The Playback page: how the queue arranges and extends itself, what a
    /// launch brings back, and how tracks get rated along the way. Split off
    /// the Application page so the music behavior reads together instead of
    /// between window and data rows.
    fn playback_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new()
            .section(self.playback_behavior_section(q, cx))
            .section(Section::new(
                q,
                icons::PLAY,
                rox_i18n::t!("settings-playback-section-startup"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-playback-restore-last-session",
                        &["resume", "reopen", "track", "queue"],
                        panel::toggle(self.restore_last_track, Self::set_restore_last_track, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::STAR,
                rox_i18n::t!("settings-playback-section-ratings"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-playback-rating-scale",
                        &["stars", "numeric"],
                        panel::choices_shared(
                            &[
                                (
                                    rox_i18n::t!("settings-playback-rating-scale-stars"),
                                    RatingStyle::Stars,
                                ),
                                (
                                    rox_i18n::t!("settings-playback-rating-scale-numeric"),
                                    RatingStyle::Numeric,
                                ),
                            ],
                            self.rating_style,
                            Self::set_rating_style,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-playback-unrated-dots",
                        &["stars", "empty"],
                        panel::toggle(self.rating_dots, Self::set_rating_dots, cx),
                    )
                },
            ))
    }

    /// What the transport's shuffle and continue buttons are doing when
    /// they're on.
    ///
    /// Here rather than behind the buttons themselves, where these two
    /// lists used to be as press-and-hold menus. Both are a pick
    /// between strategies that differ in kind, and the difference is the
    /// whole question: a menu of four bare words next to a menu of two bare
    /// words made the two buttons read as the same button twice. A settings
    /// row has room to say what each one does, and the button goes back to
    /// being a plain on/off.
    fn playback_behavior_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        // Similar needs vectors to sort by, and the switch that builds them
        // being on isn't enough: it permits the pass, it doesn't run it. The
        // mode stays listed either way, so it's discoverable and its own row
        // can say what's missing.
        let analyzed = settings::similarity_ready();
        let shuffle_mode = self.playback.read(cx).shuffle_mode();
        let continuation = self.playback.read(cx).continuation_mode();
        Section::new(
            q,
            icons::LIST_MUSIC,
            rox_i18n::t!("settings-playback-section-queue"),
            None,
            move |rows| {
                rows.custom(
                    &[
                        "shuffle",
                        "order",
                        "random",
                        "similar",
                        "sound",
                        "play order",
                    ],
                    || {
                        panel::setting_block(
                            rox_i18n::t!("settings-playback-play-order"),
                            Some(rox_i18n::t!("settings-playback-play-order.description")),
                            None,
                            panel::mode_list(
                                &shuffle_modes(),
                                shuffle_mode,
                                move |mode| mode != ShuffleMode::Similar || analyzed,
                                |this: &mut Self, mode, cx| {
                                    this.playback
                                        .update(cx, |player, cx| player.set_shuffle_mode(mode, cx));
                                    cx.notify();
                                },
                                cx,
                            ),
                        )
                        .into_any_element()
                    },
                )
                .custom(
                    &[
                        "continue",
                        "continuation",
                        "endless",
                        "queue",
                        "radio",
                        "weighted",
                        "keep playing",
                    ],
                    || {
                        panel::setting_block(
                            rox_i18n::t!("settings-playback-keep-playing"),
                            Some(rox_i18n::t!("settings-playback-keep-playing.description")),
                            None,
                            panel::mode_list(
                                &continuation_modes(),
                                continuation,
                                |_| true,
                                |this: &mut Self, mode, cx| {
                                    this.playback.update(cx, |player, cx| {
                                        player.set_continuation_mode(mode, cx)
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        )
                        .into_any_element()
                    },
                )
            },
        )
    }

    /// The Integrations page: everything rox talks to that isn't the
    /// library or the audio device. Last.fm account & scrobbling, Discord
    /// Rich Presence, the icecast sink, and the ffmpeg binary Convert runs.
    fn integrations_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let scrobbler = self.scrobbler.read(cx);
        let config = scrobbler.config().clone();
        let phase = scrobbler.phase().clone();
        let (loves_pending, love_error) = (scrobbler.loves_pending(), scrobbler.love_error());
        let connected = scrobbler.connected();
        let username = scrobbler.username().to_string();
        // A session under a different api key: this machine connected from
        // an install that signs with its own identity (the nix package,
        // a release build, a local one). Sessions don't cross, so the fix
        // is a connect here, and saying so beats a bare "not connected"
        // for someone who knows they already did this.
        let elsewhere = scrobbler.connected_elsewhere();
        // A build with its own api identity connects in one click; only
        // one without needs the user's pair.
        let builtin = has_builtin_keys();
        let keys_ready = builtin || (!config.api_key.is_empty() && !config.api_secret.is_empty());

        // The connect strip: the connection state, and the one action
        // that moves it along.
        let status: SharedString = if connected {
            rox_i18n::t!(
                "settings-integrations-lastfm-status-connected",
                username = username
            )
        } else {
            match &phase {
                AuthPhase::Idle if elsewhere => {
                    rox_i18n::t!("settings-integrations-lastfm-status-elsewhere")
                }
                AuthPhase::Idle => {
                    rox_i18n::t!("settings-integrations-lastfm-status-not-connected")
                }
                AuthPhase::Requesting => {
                    rox_i18n::t!("settings-integrations-lastfm-status-requesting")
                }
                AuthPhase::Waiting(_) => {
                    rox_i18n::t!("settings-integrations-lastfm-status-waiting")
                }
                AuthPhase::Confirming => {
                    rox_i18n::t!("settings-integrations-lastfm-status-confirming")
                }
                AuthPhase::Rejected => {
                    rox_i18n::t!("settings-integrations-lastfm-status-rejected")
                }
                AuthPhase::Failed(e) => {
                    rox_i18n::t!(
                        "settings-integrations-lastfm-status-failed",
                        error = e.clone()
                    )
                }
            }
        };
        let action = if connected {
            small_button(
                rox_i18n::t!("settings-integrations-lastfm-disconnect"),
                icons::CLOSE,
                false,
                cx.listener(|this, _, _, cx| {
                    this.scrobbler.update(cx, |s, cx| s.disconnect(cx));
                }),
            )
        } else {
            match phase {
                AuthPhase::Requesting | AuthPhase::Confirming => small_button(
                    rox_i18n::t!("settings-integrations-lastfm-working"),
                    icons::REFRESH_CW,
                    true,
                    |_, _, _| {},
                ),
                AuthPhase::Waiting(_) => small_button(
                    rox_i18n::t!("settings-integrations-lastfm-finish-connecting"),
                    icons::REFRESH_CW,
                    false,
                    cx.listener(|this, _, _, cx| {
                        this.scrobbler.update(cx, |s, cx| s.finish_auth(cx));
                    }),
                ),
                // Reconnect where a session was lost rather than never
                // held: the button reads as picking something back up.
                phase => small_button(
                    if matches!(phase, AuthPhase::Rejected) || elsewhere {
                        rox_i18n::t!("settings-integrations-lastfm-reconnect")
                    } else {
                        rox_i18n::t!("settings-integrations-lastfm-connect")
                    },
                    icons::EXTERNAL_LINK,
                    !keys_ready,
                    cx.listener(|this, _, _, cx| {
                        this.scrobbler.update(cx, |s, cx| s.begin_auth(cx));
                    }),
                ),
            }
        };

        // What the love sync has left to do, and why it stopped if it did.
        // A love that failed into a log file is two sides out of sync with
        // nothing on screen to say so, so this line is why the queue keeps
        // its reason.
        let hearts = |n: usize| {
            rox_i18n::t!("settings-integrations-lastfm-hearts", n = n as u64).to_string()
        };
        let love_status: Option<SharedString> = match (loves_pending, love_error) {
            (0, None) => None,
            (0, Some(error)) => Some(rox_i18n::t!(
                "settings-integrations-lastfm-love-failed",
                error = error.to_string()
            )),
            (pending, None) => Some(rox_i18n::t!(
                "settings-integrations-lastfm-love-pending",
                hearts = hearts(pending)
            )),
            (pending, Some(error)) => Some(rox_i18n::t!(
                "settings-integrations-lastfm-love-pending-failed",
                hearts = hearts(pending),
                error = error.to_string()
            )),
        };

        let account = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(if builtin {
                        rox_i18n::t!("settings-integrations-lastfm-intro-builtin")
                    } else {
                        rox_i18n::t!("settings-integrations-lastfm-intro-custom")
                    }),
            )
            .when(!builtin, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("settings-integrations-lastfm-api-key-row"),
                    None,
                    Input::new(&self.lastfm_key).w(px(240.)),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("settings-integrations-lastfm-secret-row"),
                    None,
                    Input::new(&self.lastfm_secret).w(px(240.)),
                ))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(tokens::SPACE_MD)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(palette::text_muted())
                            .child(status),
                    )
                    .child(action),
            );

        PageBody::new()
            .section(Section::new(
                q,
                icons::RADIO,
                rox_i18n::t!("settings-integrations-section-lastfm"),
                None,
                |rows| {
                    rows.custom(
                        &["account", "connect", "login", "api key", "scrobble"],
                        || account.into_any_element(),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::UPLOAD,
                rox_i18n::t!("settings-integrations-section-scrobbling"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-integrations-scrobble-tracks",
                        &["listens", "history"],
                        panel::toggle(
                            config.scrobbling,
                            |this: &mut Self, on, cx| {
                                this.scrobbler.update(cx, |s, cx| s.set_scrobbling(on, cx));
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-integrations-scrobble-threshold",
                        &["Last.fm", "percent"],
                        settings_ui::slider_edit(
                            &self.threshold_scrub,
                            &self.value_edit,
                            config.threshold,
                            |this: &mut Self, fraction, cx| {
                                this.scrobbler
                                    .update(cx, |s, cx| s.set_threshold(fraction, cx));
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::HEART,
                rox_i18n::t!("settings-integrations-section-favourites"),
                Some(self.import_control(cx)),
                |rows| {
                    rows.keyed(
                        "settings-integrations-love-favourites",
                        &["Last.fm", "love", "loved", "heart", "mirror"],
                        panel::toggle(
                            config.love_favourites,
                            |this: &mut Self, on, cx| {
                                this.scrobbler
                                    .update(cx, |s, cx| s.set_love_favourites(on, cx));
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                    .when_some(love_status, |rows, status| {
                        rows.custom(&["love", "queue", "failed"], || {
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(status)
                                .into_any_element()
                        })
                    })
                },
            ))
            .section(Section::new(
                q,
                icons::GLOBE,
                rox_i18n::t!("settings-integrations-section-discord"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-integrations-discord-enable",
                        &["status", "now playing"],
                        panel::toggle(self.discord_enabled, Self::set_discord_enabled, cx),
                    )
                    .keyed(
                        "settings-integrations-discord-show-lastfm",
                        &["link", "profile"],
                        panel::toggle(
                            self.discord_show_lastfm_button,
                            Self::set_discord_show_lastfm_button,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-integrations-discord-show-youtube",
                        &["link", "video"],
                        panel::toggle(
                            self.discord_show_youtube_button,
                            Self::set_discord_show_youtube_button,
                            cx,
                        ),
                    )
                },
            ))
            .section(self.icecast_section(q, cx))
            // This row stays put when ffmpeg is missing, unlike every other
            // Convert surface: it's the one place that can fix the absence.
            .section(Section::new(
                q,
                icons::AUDIO_LINES,
                rox_i18n::t!("settings-integrations-section-conversion"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-integrations-ffmpeg-binary",
                        &["ffmpeg", "convert", "encoder", "binary", "test"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(Input::new(&self.ffmpeg_path).w(px(240.)))
                            .child(small_button(
                                rox_i18n::t!("settings-integrations-ffmpeg-test"),
                                icons::FLASK,
                                false,
                                cx.listener(|this, _, _, cx| this.test_ffmpeg(cx)),
                            )),
                    )
                    // The callout the test produces, in the output status
                    // block's register: what the binary returned, and
                    // whether that's fine, readable before any of it is.
                    .when_some(self.ffmpeg_test.as_ref(), |rows, answer| {
                        rows.custom(&["ffmpeg", "convert", "test", "version"], || {
                            match answer {
                                Ok(version) => panel::banner(
                                    panel::Tone::Good,
                                    version.clone(),
                                    vec![rox_i18n::t!("settings-integrations-ffmpeg-ok-note")],
                                ),
                                Err(reason) => panel::banner(
                                    panel::Tone::Bad,
                                    rox_i18n::t!("settings-integrations-ffmpeg-fail-title"),
                                    vec![
                                        reason.clone().into(),
                                        rox_i18n::t!("settings-integrations-ffmpeg-fail-note"),
                                    ],
                                ),
                            }
                            .into_any_element()
                        })
                    })
                    // The passive note keeps covering the case where nothing
                    // was pressed, in the same banner dress as the test's
                    // result; once a test has run, its result says it better.
                    // Warn rather than Bad: nothing failed, a capability is
                    // just absent.
                    .when(
                        !convert::available() && self.ffmpeg_test.is_none(),
                        |rows| {
                            rows.custom(&["ffmpeg", "convert", "missing"], || {
                                panel::banner(
                                    panel::Tone::Warn,
                                    rox_i18n::t!("settings-integrations-ffmpeg-missing-title"),
                                    vec![rox_i18n::t!("settings-integrations-ffmpeg-missing-note")],
                                )
                                .into_any_element()
                            })
                        },
                    )
                },
            ))
    }

    /// The Icecast section (ADR 22): the source client, which is the audio
    /// half of the refused web server. The switch connects and
    /// disconnects; the fields write through as they're typed and the sink
    /// re-applies when one is left.
    ///
    /// Everything under the switch only appears once it's on. A mount, a
    /// source login and a bitrate are four rows of setup for something most
    /// people never turn on, and with the switch off they'd only be a
    /// question nobody asked.
    fn icecast_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        Section::new(
            q,
            icons::RADIO,
            rox_i18n::t!("settings-audio-section-broadcast"),
            None,
            |rows| {
                rows.keyed(
                    "settings-audio-broadcast-enable",
                    &["icecast", "stream", "radio", "cast", "mount", "broadcast"],
                    panel::toggle(self.broadcast_enabled, Self::set_broadcast_enabled, cx),
                )
                .when(self.broadcast_enabled, |rows| {
                    rows.keyed(
                        "settings-audio-broadcast-server",
                        &["icecast", "server", "host", "port"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(Input::new(&self.broadcast_host).w(px(180.)))
                            .child(Input::new(&self.broadcast_port).w(px(64.))),
                    )
                    .keyed(
                        "settings-audio-broadcast-mount",
                        &["icecast", "mount", "name", "advertise"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(Input::new(&self.broadcast_mount).w(px(104.)))
                            .child(Input::new(&self.broadcast_name).w(px(140.))),
                    )
                    .keyed(
                        "settings-audio-broadcast-login",
                        &["icecast", "source", "login", "password", "credentials"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(Input::new(&self.broadcast_user).w(px(104.)))
                            .child(Input::new(&self.broadcast_password).w(px(140.))),
                    )
                    .custom(&["bitrate", "kbps", "quality", "encoder", "mp3"], || {
                        self.broadcast_bitrate_row(cx).into_any_element()
                    })
                })
            },
        )
    }

    /// The broadcast switch. Applying reads the file the field edits were
    /// written to, so the sink comes up on whatever the rows say; off tears
    /// the connection down, which releases the mount.
    fn set_broadcast_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.broadcast_enabled = on;
        Settings::update(move |s| s.broadcast.enabled = on);
        crate::integrations::broadcast::apply();
        cx.notify();
    }

    /// The encoder bitrate, the steps LAME takes. A change while streaming
    /// reconnects, since one stream can't change bitrate under a listener.
    fn broadcast_bitrate_row(&self, cx: &mut Context<Self>) -> Div {
        let options: Vec<(u32, SharedString)> = [96u32, 112, 128, 160, 192, 224, 256, 320]
            .into_iter()
            .map(|kbps| {
                (
                    kbps,
                    rox_i18n::format::format_unit(f64::from(kbps), 0, "kbps").into(),
                )
            })
            .collect();
        panel::setting_row(
            rox_i18n::t!("settings-audio-broadcast-bitrate"),
            Some(rox_i18n::t!("settings-audio-broadcast-bitrate.description")),
            panel::picker(
                "broadcast-bitrate",
                self.broadcast_bitrate,
                options,
                false,
                |this: &mut Self, kbps, cx| {
                    this.broadcast_bitrate = kbps;
                    Settings::update(move |s| s.broadcast.bitrate = kbps);
                    if this.broadcast_enabled {
                        crate::integrations::broadcast::apply();
                    }
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// Run the version probe against whatever the input holds, off the UI
    /// thread since it spawns a process, and keep the result for the
    /// callout. The probe cache records it too, so a pass flips the
    /// Convert surfaces on right here.
    fn test_ffmpeg(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_executor()
                .spawn(async { convert::test() })
                .await;
            this.update(cx, |this, cx| {
                this.ffmpeg_test = Some(answer);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The lrclib toggle: through the live static, so the lyrics panel's
    /// fetch action appears and hides with it, and into the file.
    fn set_lrclib(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.lrclib = on;
        providers::set_lyrics_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    /// Where a fetched sheet saves: straight into the file, read at
    /// fetch time.
    fn set_lyrics_save(&mut self, save: LyricsSave, cx: &mut Context<Self>) {
        self.providers.lyrics_save = save;
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    /// The MusicBrainz toggle: through the live static, so the metadata
    /// panel's lookup action appears and hides with it, and into the file.
    fn set_musicbrainz(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.musicbrainz = on;
        providers::set_metadata_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    /// The iTunes cover-art toggle: through the live static and into the
    /// file, so the cover editor's search follows it.
    fn set_itunes(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.itunes = on;
        providers::set_itunes_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    /// The Deezer cover-art toggle, iTunes's twin.
    fn set_deezer(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.deezer = on;
        providers::set_deezer_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    /// The Last.fm cover-art toggle, Deezer's twin.
    fn set_lastfm_art(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.lastfm_art = on;
        providers::set_lastfm_art_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    /// The artist-lookup toggle: through the live static, so the
    /// biography panel's fetches follow it.
    fn set_artist(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.artist = on;
        providers::set_artist_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    /// The Providers page: the online enrichment services (ADR 14), a
    /// section per domain. Nothing here fetches on its own; the toggles
    /// gate the actions the panels offer.
    fn providers_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new()
            .section(Section::new(
                q,
                icons::MIC,
                rox_i18n::t!("settings-providers-section-lyrics"),
                None,
                |rows| {
                    rows.custom(&["online", "network", "offline", "privacy"], || {
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("settings-providers-lyrics-intro"))
                            .into_any_element()
                    })
                    .keyed(
                        "settings-providers-lrclib",
                        &["online", "fetch"],
                        panel::toggle(self.providers.lrclib, Self::set_lrclib, cx),
                    )
                    .keyed(
                        "settings-providers-save-lyrics",
                        &["sidecar", "store"],
                        panel::choices_shared(
                            &[
                                (
                                    rox_i18n::t!("settings-providers-save-lyrics-data-folder"),
                                    LyricsSave::Store,
                                ),
                                (
                                    rox_i18n::t!("settings-providers-save-lyrics-sidecar"),
                                    LyricsSave::Sidecar,
                                ),
                                (
                                    rox_i18n::t!("settings-providers-save-lyrics-tag"),
                                    LyricsSave::Tag,
                                ),
                            ],
                            self.providers.lyrics_save,
                            Self::set_lyrics_save,
                            cx,
                        ),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::TAG,
                rox_i18n::t!("settings-providers-section-metadata"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-providers-musicbrainz",
                        &["lookup", "online"],
                        panel::toggle(self.providers.musicbrainz, Self::set_musicbrainz, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::DISC,
                rox_i18n::t!("settings-providers-section-cover-art"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-providers-itunes",
                        &["artwork", "covers", "album art"],
                        panel::toggle(self.providers.itunes, Self::set_itunes, cx),
                    )
                    .keyed(
                        "settings-providers-deezer",
                        &["artwork", "covers", "album art"],
                        panel::toggle(self.providers.deezer, Self::set_deezer, cx),
                    )
                    .keyed(
                        "settings-providers-lastfm-art",
                        &["artwork", "covers", "album art"],
                        panel::toggle(self.providers.lastfm_art, Self::set_lastfm_art, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::USER,
                rox_i18n::t!("settings-providers-section-artist"),
                None,
                |rows| {
                    rows.row(
                        rox_i18n::t!("settings-providers-artist"),
                        Some(rox_i18n::t!("settings-providers-artist.description")),
                        panel::toggle(self.providers.artist, Self::set_artist, cx),
                    )
                },
            ))
    }

    /// One cell of the color grid: the picker with its label beside it,
    /// or a dimmed inert swatch while song theming drives the palette.
    /// The inert swatch shows the derived color the track produced, the
    /// same values export saves, not the base underneath.
    fn color_cell(&self, role: &Role, picker: &Entity<ColorPickerState>, locked: bool) -> Div {
        let control: AnyElement = if locked {
            div()
                .size_5()
                .rounded(tokens::RADIUS)
                .border_1()
                .border_color(palette::border())
                .bg((role.get)(&palette::resolved()))
                .opacity(0.5)
                .into_any_element()
        } else {
            // The picker pads a 4px margin around its swatch square; the
            // counter-margin keeps the live cell the same 20px footprint
            // as the locked one, so the grid doesn't loosen when editable.
            ColorPicker::new(picker)
                .small()
                .m(px(-4.))
                .into_any_element()
        };
        settings_ui::color_cell(control, role.label, false, None)
    }

    fn colors_section(&self, q: &Query, columns: usize, cx: &mut Context<Self>) -> Section {
        let locked = palette::art_theming();

        // Import, inverse, and reset lock with the rest of the editor:
        // they change the palette too. Apply Song Theme is the opposite,
        // live only while theming drives the colors it bakes in. Export
        // stays live; unlocked it saves the base palette, locked the
        // derived one the swatches show.
        let inverse_label = match self.editor_mode {
            palette::Mode::Dark => rox_i18n::t!("settings-appearance-inverse-from-light"),
            palette::Mode::Light => rox_i18n::t!("settings-appearance-inverse-from-dark"),
        };
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                inverse_label,
                icons::CONTRAST,
                locked,
                cx.listener(|this, _, window, cx| this.inverse_palette(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("panel-apply-song-theme"),
                icons::DISC,
                !locked,
                cx.listener(|this, _, window, cx| this.apply_song_theme(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("settings-appearance-palette-import"),
                icons::DOWNLOAD,
                locked,
                cx.listener(|this, _, window, cx| this.import_palette(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("settings-appearance-palette-export"),
                icons::UPLOAD,
                false,
                cx.listener(|this, _, _, cx| this.export_palette(cx)),
            ))
            .child(small_button(
                rox_i18n::t!("panel-reset"),
                icons::REFRESH_CW,
                locked,
                cx.listener(|this, _, window, cx| this.reset_palette(window, cx)),
            ));
        Section::new(
            q,
            icons::PALETTE,
            rox_i18n::t!("settings-appearance-section-colors"),
            Some(controls.into_any_element()),
            |rows| {
                rows.custom(
                    &["palette", "accent", "swatch", "role", "import", "export"],
                    || {
                        let mut body = div().flex().flex_col().gap(tokens::SPACE_XS);
                        if locked {
                            body = body.child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("settings-appearance-colors-locked-note")),
                            );
                        }
                        body.child(settings_ui::role_grid(columns, |j| {
                            self.color_cell(&ROLES[j], &self.pickers[j], locked)
                                .into_any_element()
                        }))
                        .into_any_element()
                    },
                )
            },
        )
    }

    /// One row of the folder table: the path, its rollup numbers, and a
    /// remove control, inert while a scan runs.
    fn folder_row(
        &self,
        root: &Path,
        stats: Stats,
        scanning: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let path: SharedString = root.to_string_lossy().into_owned().into();
        let remove = icon_button(icons::CLOSE, scanning, {
            let root = root.to_path_buf();
            cx.listener(move |this, _, _, cx| {
                this.library
                    .update(cx, |library, cx| library.remove_root(&root, cx));
            })
        });
        div()
            // Named after the folder, so the row's remove button is its
            // own rather than every other row's. See
            // `rox_panel_kit::ui::control_focus`.
            .id(ElementId::Name(path.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .child(div().flex_1().min_w_0().truncate().child(path))
            .child(number_cell(TRACKS_COL_W, stats.tracks.to_string()))
            .child(number_cell(ALBUMS_COL_W, stats.albums.to_string()))
            .child(number_cell(SIZE_COL_W, human_size(stats.bytes)))
            .child(remove)
    }

    fn library_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let busy = self.library.read(cx).busy();
        let scanning = busy.is_some();
        // Past the ceiling the watch stays off, so the toggle grays
        // out at off and the note says why, with the numbers. Folders summed
        // off the cached rollups, not a per-frame count; the roots never
        // nest, so nothing counts twice. Matched to the catalog's own limit,
        // which is None where the platform prices watching flat.
        let dirs = self.root_stats.iter().map(|(_, s)| s.dirs).sum::<u64>();
        let over_limit = rox_services::catalog::watch_limit_dirs().filter(|limit| dirs > *limit);
        let lead_in = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-library-folders-intro"));
        // The rescan nudge, only while the separator rule has moved
        // this session: filtering and the genre wall follow the flip
        // right away, but genre lists earlier scans wrote into the
        // database keep their old shape until a rescan re-reads the
        // tags.
        let separators_moved = self.split_genre_compounds != self.split_genre_compounds_at_open;
        let nudge = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-library-genre-separator-nudge"));
        // The folder table: a column header line, then a hairlined row
        // per folder.
        let mut table = div().flex().flex_col().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_MD)
                .pb(tokens::SPACE_XS)
                .border_b_1()
                .border_color(palette::border())
                .text_xs()
                .text_color(palette::text_muted())
                .child(
                    div()
                        .flex_1()
                        .child(rox_i18n::t!("settings-library-folder-col-folder")),
                )
                .child(
                    div()
                        .w(TRACKS_COL_W)
                        .flex_none()
                        .text_right()
                        .child(rox_i18n::t!("settings-library-folder-col-tracks")),
                )
                .child(
                    div()
                        .w(ALBUMS_COL_W)
                        .flex_none()
                        .text_right()
                        .child(rox_i18n::t!("settings-library-folder-col-albums")),
                )
                .child(
                    div()
                        .w(SIZE_COL_W)
                        .flex_none()
                        .text_right()
                        .child(rox_i18n::t!("settings-library-folder-col-size")),
                )
                .child(div().w(ACTION_COL_W).flex_none()),
        );
        if self.root_stats.is_empty() {
            table = table.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-library-no-folders")),
            );
        }
        for (root, stats) in &self.root_stats {
            table = table.child(self.folder_row(root, *stats, scanning, cx));
        }
        // An add slot at the foot of the list, where the eye lands after
        // reading it. Same browse the header's Add Folder opens.
        table = table.child(div().flex().flex_row().items_center().child(icon_button(
            icons::PLUS,
            scanning,
            cx.listener(|this, _, _, cx| {
                rox_services::catalog::browse(&this.library, cx);
            }),
        )));
        // The library's badge and the file under the scan cursor, or the
        // resting status, under the table.
        let note: Option<SharedString> = busy.or_else(|| {
            let status = self.library.read(cx).status();
            (!status.is_empty()).then_some(status)
        });
        let table = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(table)
            .when_some(note, |d, note| {
                d.child(
                    // w_full: truncate with no definite width measures at
                    // min-content and the line collapses to a bare
                    // ellipsis.
                    div()
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(note),
                )
            });

        // Add folder and rescan are in the section header like the colors
        // controls.
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                rox_i18n::t!("settings-library-add-folder"),
                icons::FOLDER_PLUS,
                scanning,
                cx.listener(|this, _, _, cx| {
                    rox_services::catalog::browse(&this.library, cx);
                }),
            ))
            .child(small_button(
                rox_i18n::t!("settings-common-rescan"),
                icons::REFRESH_CW,
                scanning || self.root_stats.is_empty(),
                cx.listener(|this, _, _, cx| {
                    this.library.update(cx, |library, cx| library.rescan(cx));
                }),
            ))
            // The tag repair window: find and rewrite files with the
            // broken ID3v2.4 tag shape lofty reads mangled, where a user
            // ends up after seeing garbled tags.
            .child(small_button(
                rox_i18n::t!("settings-library-repair-tags"),
                icons::FILE_TEXT,
                scanning,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    let now_art = this.now_art.clone();
                    crate::tags::repair::open(library, now_art, cx);
                }),
            ))
            // The duplicates window: find tracks the library has more than
            // once and move the spare copies to the trash.
            .child(small_button(
                rox_i18n::t!("settings-library-duplicates"),
                icons::COPY,
                scanning,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    let thumbs = this.thumbs.clone();
                    let now_art = this.now_art.clone();
                    crate::duplicates::open(library, thumbs, now_art, cx);
                }),
            ));
        // The lead-in describes the table, so both use the same terms
        // and a search never turns up one without the other.
        let folders = ["scan", "rescan", "music", "add", "remove"];
        PageBody::new()
            .section(Section::new(
                q,
                icons::FOLDER,
                rox_i18n::t!("settings-library-section-folders"),
                Some(controls.into_any_element()),
                |rows| {
                    let rows = rows.custom(&folders, || lead_in.into_any_element());
                    let rows = match over_limit {
                        Some(limit) => rows.row_dyn(
                            &["monitor", "auto", "rescan", "folder"],
                            rox_i18n::t!("settings-library-watch-folders"),
                            Some(
                                format!(
                                    "Off: this library spans {dirs} folders and each needs \
                                 one Linux file watch, more than the {limit} the app \
                                 will take from the system's shared budget. Rescan by \
                                 hand to fold in changes"
                                )
                                .into(),
                            ),
                            panel::toggle_locked(false),
                        ),
                        None => rows.keyed(
                            "settings-library-watch-folders",
                            &["monitor", "auto", "live"],
                            panel::toggle(self.watch_library, Self::set_watch_library, cx),
                        ),
                    };
                    rows.keyed(
                        "settings-library-merge-case",
                        &["fold", "duplicates", "capitalization"],
                        panel::toggle(self.fold_case, Self::set_fold_case, cx),
                    )
                    .keyed(
                        "settings-show-readings",
                        &["romaji", "reading", "pronunciation", "sort name"],
                        panel::toggle(settings::show_readings(), Self::set_show_readings, cx),
                    )
                    .keyed(
                        "settings-library-split-genres",
                        &["separator", "multi-genre"],
                        panel::toggle(
                            self.split_genre_compounds,
                            Self::set_split_genre_compounds,
                            cx,
                        ),
                    )
                    .when(separators_moved, |rows| {
                        rows.custom(&["genre", "separator", "split", "rescan"], || {
                            nudge.into_any_element()
                        })
                    })
                    .custom(&folders, || table.into_any_element())
                },
            ))
            .section(self.acoustic_section(q, cx))
            .section(self.tempo_section(q, cx))
            .section(self.dictionary_section(q, cx))
            .section(self.embed_section(q, cx))
    }

    /// The catch-up for the three save settings: what rox is already holding
    /// written into the files themselves.
    ///
    /// It's here, under the acoustic radio, because this page already has two
    /// of the three questions it answers: the save mode for descriptions is
    /// the row above it, and the folder tools at the top of the page are the
    /// other things that rewrite a library's files. The
    /// counts belong to the dialog rather than this row: working out how many
    /// files each source would touch means reading their tags, which is not
    /// something a settings page should do on the way past.
    fn embed_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let button = small_button(
            rox_i18n::t!("settings-library-embed-button"),
            icons::UPLOAD,
            self.library.read(cx).busy().is_some(),
            cx.listener(|this, _, _, cx| {
                let library = this.library.clone();
                let now_art = this.now_art.clone();
                crate::bake_dialog::open(library, now_art, cx);
            }),
        )
        .into_any_element();
        Section::new(
            q,
            icons::TAG,
            rox_i18n::t!("settings-library-section-stored-metadata"),
            Some(button),
            |rows| {
                rows.keyed(
                    "settings-library-write-stored",
                    &[
                        "embed",
                        "bake",
                        "tags",
                        "lyrics",
                        "replaygain",
                        "acoustic",
                        "portable",
                    ],
                    div(),
                )
            },
        )
    }

    /// Roll each scan folder up off the UI thread. Every row of that table
    /// is a COUNT and a SUM over the tracks under one path, which is a
    /// full table scan on a big library, and opening this window used to
    /// pay for all of them before it drew anything. The rows are already
    /// on screen by then; their numbers fill in when this lands.
    ///
    /// Its own connection rather than the catalog's, since the catalog's
    /// lives on the UI thread. WAL gives readers concurrency for free, so
    /// a scan running alongside this doesn't block it.
    fn measure_root_stats(library: &Entity<Library>, cx: &mut Context<Self>) {
        let db = library.read(cx).db_path();
        let roots = library.read(cx).roots();
        cx.spawn(async move |this, cx| {
            let measured = cx
                .background_executor()
                .spawn(async move {
                    let conn = rox_library::store::open(&db).ok()?;
                    Some(
                        roots
                            .into_iter()
                            .map(|root| {
                                let stats = rox_library::store::stats_under(&conn, &root)
                                    .unwrap_or_default();
                                (root, stats)
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .await;
            // A database that wouldn't open is nothing to report: leave the
            // folders listed with whatever they last showed.
            let Some(measured) = measured else { return };
            this.update(cx, |this, cx| {
                this.root_stats = measured;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Measure everything the storage page shows, off the UI thread. It used
    /// to run whole on page entry, back when it was a handful of stat calls;
    /// the page accounting behind the database rows walks every page in the
    /// file, which is a tenth of a second on a described library, and the
    /// artist and model stores are counted file by file on top of that.
    ///
    /// The snapshot swaps in whole when it arrives, so a remeasure leaves the
    /// numbers already on screen up rather than blanking them, and the first
    /// one shows zeros for the moment it takes. One walk at a time: the
    /// library fires its update repeatedly through a scan, and every search
    /// keystroke asks for numbers until there are some.
    fn refresh_storage(&mut self, cx: &mut Context<Self>) {
        if self.storage_measuring {
            // A walk that's already out was started before whatever just
            // changed, so what it brings back is stale on arrival. Queue one
            // behind it rather than dropping the ask or running a second
            // walk over the same files alongside the first.
            self.storage_remeasure = true;
            return;
        }
        self.storage_measuring = true;
        let db = self.library.read(cx).db_path();
        cx.spawn(async move |this, cx| {
            let info = cx
                .background_executor()
                .spawn(async move { StorageInfo::measure(&db) })
                .await;
            this.update(cx, |this, cx| {
                this.storage_measuring = false;
                this.storage = Some(info);
                cx.notify();
                if std::mem::take(&mut this.storage_remeasure) {
                    this.refresh_storage(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Empty the thumbnail store. The delete runs off the UI thread on
    /// the service's own connection, so it serializes against in-flight
    /// loads; the sizes refresh when it finishes.
    fn clear_thumbs(&mut self, cx: &mut Context<Self>) {
        let Some(conn) = self.thumbs.read(cx).store_conn() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { rox_library::thumbs::clear(&conn) })
                .await;
            this.update(cx, |this, cx| this.refresh_storage(cx)).ok();
        })
        .detach();
    }

    /// Drop the waveform cache; strips re-decode on their next play.
    fn clear_waveforms(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { rox_services::peaks::clear() })
                .await;
            this.update(cx, |this, cx| this.refresh_storage(cx)).ok();
        })
        .detach();
    }

    /// Drop the artist store: bios, portraits, banners and fanart. The walk
    /// deletes a folder tree, so it goes off the UI thread the way the peak
    /// cache's clear does; the panels fetch again as they open.
    fn clear_artists(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { rox_services::artists::clear() })
                .await;
            this.update(cx, |this, cx| this.refresh_storage(cx)).ok();
        })
        .detach();
    }

    /// Drop one model's vectors, the confirm dialog's yes. The library holds
    /// the busy badge through the delete and the vacuum behind it and emits
    /// its update when they finish, which remeasures this page.
    fn clear_embeddings(&mut self, model: &str, cx: &mut Context<Self>) {
        let model = model.to_owned();
        self.library
            .update(cx, |library, cx| library.clear_embeddings(&model, cx));
    }

    /// The measured-tempos clear, the confirm dialog's yes. The library
    /// emits Updated once the clear finishes, which refreshes the coverage
    /// split this window shows and the row's count with it.
    fn clear_measured_bpm(&mut self, cx: &mut Context<Self>) {
        self.library
            .update(cx, |library, cx| library.clear_measured_bpm(cx));
    }

    /// A row per acoustic model with vectors in the library: what it
    /// described, and the clear that drops it. Built here rather than inside
    /// the page's section closure because the model id is the row's label,
    /// and rows with built labels don't go through [`Rows::keyed`].
    fn embedding_rows(
        &self,
        models: &[rox_library::embeddings::ModelRows],
        cx: &mut Context<Self>,
    ) -> Vec<(String, AnyElement)> {
        // The library rejects a clear while it's busy, but nothing there can
        // stop the analysis pass, which opens the database by path on its
        // own and would write vectors straight back in behind the delete. The
        // page that offers the button is the one that knows a pass is running.
        let inert = self.acoustic_job.is_some() || self.library.read(cx).busy().is_some();
        models
            .iter()
            .map(|entry| {
                let id = entry.model.clone();
                let known = rox_acoustic::models::find(&id).is_some()
                    || id == rox_acoustic::MODEL
                    || self
                        .acoustic_local
                        .as_ref()
                        .is_some_and(|local| local.id == id);
                let description = if known {
                    format!(
                        "{}, {} values a track. Clearing gives the space back, and having the \
                         descriptions again means a whole pass over the library",
                        self.label_for(
                            &id,
                            rox_i18n::t_static("settings-storage-model-fallback-this")
                        ),
                        entry.dim
                    )
                } else {
                    format!(
                        "{} values a track. Nothing in this build writes this model, so these \
                         are left over from one that was renamed or dropped, and clearing them \
                         costs the library nothing it uses",
                        entry.dim
                    )
                };
                let control = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(readout(
                        rox_i18n::t!("settings-common-tracks-count", count = entry.rows)
                            .to_string(),
                    ))
                    .child(small_button(
                        rox_i18n::t!("settings-common-clear"),
                        icons::TRASH,
                        inert,
                        cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                this.pending = Some(Pending::ClearEmbeddings(id.clone()));
                                cx.notify();
                            }
                        }),
                    ));
                let row = rox_panel_kit::setting_row_dyn(
                    SharedString::from(id.clone()),
                    Some(description.into()),
                    control,
                )
                .into_any_element();
                (id, row)
            })
            .collect()
    }

    fn storage_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let info = self.storage.clone().unwrap_or_default();
        let store = info.breakdown;
        let measured = self.storage.is_some();
        let models = self.embedding_rows(&info.models, cx);
        // The two counts come in already worded, so "1 track" never reads
        // as "1 tracks" and every locale gets its own plural rules.
        let music = rox_i18n::t!(
            "settings-storage-music-summary",
            tracks = rox_i18n::t!("status-count-tracks", count = info.music.tracks).to_string(),
            albums = rox_i18n::t!("status-count-albums", count = info.music.albums).to_string(),
            size = human_size(info.music.bytes)
        )
        .to_string();
        // Deletes leave pages behind and nothing vacuums on a schedule, so a
        // freelist is the ordinary state of the file rather than news. It's
        // worth a row once it's a real share of what library.db weighs.
        let reclaimable = store.free >= 1_000_000 && store.free * 10 >= store.total();
        // The tempo row's numbers come off the coverage split the window
        // already keeps live, not the storage walk: a tempo is a float a
        // row, so the interesting figure is how many, not how heavy. The
        // clear can't gate the tempo pass, which opens the database by path
        // on its own, so the button goes inert while one runs, the same
        // arrangement as the model rows above a running analysis.
        let measured_tempos = self.bpm_coverage.measured;
        let tempos_inert = measured_tempos == 0
            || self.tempo_job.is_some()
            || self.library.read(cx).busy().is_some();
        PageBody::new()
            .section(Section::new(
                q,
                icons::DATABASE,
                rox_i18n::t!("settings-storage-section-library"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-music-files",
                        &["size", "disk", "space"],
                        readout(music),
                    )
                    .keyed(
                        "settings-storage-catalog",
                        &["database", "catalog", "index", "size", "disk"],
                        readout(human_size(store.catalog)),
                    )
                    .keyed(
                        "settings-storage-playlists-history",
                        &["playlists", "history", "listens", "genres", "size"],
                        readout(human_size(store.playlists + store.history + store.genres)),
                    )
                    .when(reclaimable, |rows| {
                        rows.keyed(
                            "settings-storage-reclaimable",
                            &["free", "reclaim", "vacuum", "deleted", "size"],
                            readout(human_size(store.free)),
                        )
                    })
                    .keyed(
                        "settings-storage-lyrics",
                        &["size", "disk"],
                        readout(human_size(info.lyrics)),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::AUDIO_WAVEFORM,
                rox_i18n::t!("settings-storage-section-acoustic"),
                None,
                move |mut rows| {
                    rows = rows.keyed(
                        "settings-storage-vectors",
                        &["acoustic", "embeddings", "vectors", "similar", "size"],
                        readout(human_size(store.acoustic)),
                    );
                    if models.is_empty() {
                        // Only once a walk has actually come back. An empty list
                        // is also what the page holds for the beat before the
                        // first one arrives, and saying nothing has been
                        // described is a lie to tell a described library.
                        return rows.when(measured, |rows| {
                            rows.keyed(
                                "settings-storage-models-empty",
                                &["acoustic", "analysis", "model", "describe"],
                                readout(rox_i18n::t!("settings-storage-none").to_string()),
                            )
                        });
                    }
                    for (id, row) in models {
                        let terms = [
                            "acoustic",
                            "embeddings",
                            "vectors",
                            "clear",
                            "model",
                            id.as_str(),
                        ];
                        rows = rows.custom(&terms, || row);
                    }
                    rows
                },
            ))
            .section(Section::new(
                q,
                icons::CLOCK,
                rox_i18n::t!("settings-storage-section-tempo"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-measured-tempos",
                        &["tempo", "bpm", "measured", "clear"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(
                                rox_i18n::t!(
                                    "settings-common-tracks-count",
                                    count = measured_tempos
                                )
                                .to_string(),
                            ))
                            .child(small_button(
                                rox_i18n::t!("settings-common-clear"),
                                icons::TRASH,
                                tempos_inert,
                                cx.listener(|this, _, _, cx| {
                                    this.pending = Some(Pending::ClearMeasuredBpm);
                                    cx.notify();
                                }),
                            )),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::LAYERS,
                rox_i18n::t!("settings-storage-section-caches"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-cover-thumbnails",
                        &["cache", "clear", "artwork", "size"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.thumbs)))
                            .child(small_button(
                                rox_i18n::t!("settings-common-clear"),
                                icons::TRASH,
                                false,
                                cx.listener(|this, _, _, cx| this.clear_thumbs(cx)),
                            )),
                    )
                    .keyed(
                        "settings-storage-waveforms",
                        &["cache", "clear", "peaks", "size"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.waveforms)))
                            .child(small_button(
                                rox_i18n::t!("settings-common-clear"),
                                icons::TRASH,
                                false,
                                cx.listener(|this, _, _, cx| this.clear_waveforms(cx)),
                            )),
                    )
                    .keyed(
                        "settings-storage-artist-images",
                        &[
                            "cache",
                            "clear",
                            "artist",
                            "images",
                            "portrait",
                            "biography",
                            "size",
                        ],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.artists)))
                            .child(small_button(
                                rox_i18n::t!("settings-common-clear"),
                                icons::TRASH,
                                false,
                                cx.listener(|this, _, _, cx| this.clear_artists(cx)),
                            )),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::FOLDER,
                rox_i18n::t!("settings-storage-section-app-data"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-model-weights",
                        &["model", "weights", "download", "ml", "size"],
                        readout(human_size(info.weights)),
                    )
                    .keyed(
                        "settings-storage-looks-layouts",
                        &["workspace", "layout", "shader", "icons", "look", "size"],
                        readout(human_size(info.app_data)),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::FILE_TEXT,
                rox_i18n::t!("settings-storage-section-diagnostics"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-logs",
                        &["debug", "reveal", "diagnostics"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.logs)))
                            .child(small_button(
                                rox_i18n::t!("settings-common-reveal"),
                                icons::FILE_TEXT,
                                false,
                                cx.listener(|_, _, _, cx| {
                                    cx.reveal_path(&rox_core::logging::log_path());
                                }),
                            )),
                    )
                },
            ))
    }

    /// The launch-check toggle: into the file, so the next start reads the
    /// new setting. This run is already past its launch check either way.
    fn set_check_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.check_updates = on;
        Settings::update(move |s| s.check_updates = on);
        cx.notify();
    }

    /// The auto-download toggle, same shape: the next launch's check reads
    /// it.
    fn set_download_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.download_updates = on;
        Settings::update(move |s| s.download_updates = on);
        cx.notify();
    }

    fn set_ai_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ai_enabled = on;
        Settings::update(move |s| s.ai_enabled = on);
        // Turning it off takes the MCP and ML Models pages out of the
        // sidebar; a window on one of them goes back to the page with the
        // toggle rather than staying on an orphaned page.
        if !on && matches!(self.page, Page::Mcp | Page::MlModels) {
            self.page = Page::Application;
        }
        cx.notify();
    }

    fn set_mcp_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.mcp_enabled = on;
        Settings::update(move |s| s.mcp_enabled = on);
        cx.notify();
    }

    fn set_experimental(&mut self, on: bool, cx: &mut Context<Self>) {
        self.experimental = on;
        Settings::update(move |s| s.experimental = on);
        settings::set_experimental(on, cx);
        // The in-window menus read the flag as they draw, so the refresh
        // above is enough for them; the macOS bar is built once and held by
        // the system, so it has to be rebuilt.
        crate::workspace::native_menu::rebuild(cx);
        cx.notify();
    }

    fn set_acoustic_analysis(&mut self, on: bool, cx: &mut Context<Self>) {
        self.acoustic_analysis = on;
        Settings::update(move |s| s.acoustic_analysis = on);
        settings::set_acoustic_analysis(on, cx);
        // Switching it off mid-pass stops the pass: it's the only thing that
        // sanctioned the decoding in the first place.
        if !on {
            embeddings::stop(cx);
        }
        cx.notify();
    }

    /// The follow-the-watcher switch for the analysis pass,
    /// [`Self::set_replay_gain_auto`]'s shape: on the way on it prices the
    /// backlog through the prompt, and declining is a no to the switch too,
    /// coming back through `pass_refused`.
    fn set_acoustic_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.acoustic_auto = on;
        Settings::update(move |s| s.acoustic_auto = on);
        if on && self.acoustic_coverage.missing() > 0 && self.acoustic_job.is_none() {
            let library = self.library.clone();
            pass_prompt::raise_for_switch(self, pass_prompt::Pass::Acoustic, library, cx);
        }
        cx.notify();
    }

    /// Where an analyzed vector saves. Straight to the file: nothing holds a
    /// live copy of it, and the pass reads it once when it starts, so a pass
    /// already running keeps the destination it began with.
    fn set_acoustic_save(&mut self, save: AcousticSave, cx: &mut Context<Self>) {
        self.acoustic_save = save;
        Settings::update(move |s| s.acoustic_save = save);
        cx.notify();
    }

    /// The Development page: the switches for work that isn't finished, and
    /// the controls for whatever they turn on.
    fn development_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new().section(Section::new(
            q,
            icons::FLASK,
            rox_i18n::t!("settings-development-section-features"),
            None,
            |rows| {
                rows.keyed(
                    "settings-development-experimental-panels",
                    &["debug", "beta", "unfinished"],
                    panel::toggle(self.experimental, Self::set_experimental, cx),
                )
            },
        ))
    }

    /// The MCP page (ADR 22): where an MCP client is pointed at rox. The
    /// server is the rox-mcp binary beside the executable, proxying the
    /// control socket, so the page holds the switch that lets it serve
    /// requests and the copy-ready config snippet. Only in the sidebar while
    /// AI features are on, and off at its own toggle even then: revealing
    /// the page is not the same as opening the door. The socket itself is on
    /// the Application page; it's rox's surface, not MCP's.
    fn mcp_page(&self, q: &Query, window: &mut Window, cx: &mut Context<Self>) -> PageBody {
        let snippet = mcp_config_snippet();
        // A TextView rather than a styled div so the snippet can actually be
        // selected and copied in place; the markdown code block brings its
        // own frame, and the header button still copies the whole thing.
        let block =
            TextView::markdown("mcp-config", format!("```json\n{snippet}\n```"), window, cx)
                .selectable(true)
                .text_xs();
        let toggle = panel::toggle(self.mcp_enabled, Self::set_mcp_enabled, cx);
        PageBody::new().section(Section::new(
            q,
            icons::LINK,
            rox_i18n::t!("settings-page-mcp"),
            // The header's one-click copy only while the server is on: a
            // grab-this button on a switched-off surface reads as an
            // invitation the toggle just declined.
            self.mcp_enabled.then(|| {
                small_button(
                    rox_i18n::t!("settings-common-copy"),
                    icons::COPY,
                    false,
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(snippet.clone()));
                    },
                )
                .into_any_element()
            }),
            move |rows| {
                rows.keyed(
                    "settings-mcp-enable",
                    &["mcp", "enable", "server", "tools"],
                    toggle,
                )
                .custom(
                    &["mcp", "client", "config", "claude", "agent"],
                    move || {
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(panel::setting_row(
                                rox_i18n::t!("settings-mcp-client-config"),
                                Some(rox_i18n::t!("settings-mcp-client-config.description")),
                                div().into_any_element(),
                            ))
                            .child(block)
                            .into_any_element()
                    },
                )
            },
        ))
    }

    /// The models page: what can run a job that needs a network, what it
    /// costs to fetch, and which one the library uses.
    ///
    /// Its own page rather than a section under Library because a model is an
    /// asset with a lifecycle of its own. It's downloaded, it takes up disk
    /// space, it can be replaced by a file the user supplies, and it will one
    /// day serve more than one job. The Library page picks a job's extractor;
    /// this page is the shelf that pick reads from.
    ///
    /// One section per job the models do, which today is acoustic
    /// analysis and one day won't be. Each section is the same shape: a
    /// Recommended half rox keeps a catalog for, and a Custom half that is
    /// whatever file the user points at. That's the whole reason the split
    /// is a control on the section rather than a second section, since a
    /// standalone "Custom Model" would have nothing to say about which job
    /// it was custom for.
    fn ml_models_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new().section(self.acoustic_models_section(q, cx))
    }

    /// The Japanese dictionary behind kanji readings.
    ///
    /// On the Library page rather than beside the acoustic weights, which
    /// is where it looks like it belongs: it's a download with a size and
    /// a licence, the same shape those have. The ML Models page comes and
    /// goes with the AI switch, and with that switch off the dictionary
    /// would be unreachable while the romanization pass still ran and
    /// still pointed people at a page that wasn't in the sidebar. It also
    /// isn't a model. It's a lookup table of Japanese words and their
    /// readings, compiled in 2007, and nothing about it is learned.
    ///
    /// A second row rather than one shared with the model rows. The
    /// acoustic rows carry a Use button, an active mark and an extractor
    /// pick, because a library runs exactly one of several models and
    /// choosing between them is what that page half is for. There's one
    /// dictionary, nothing to choose, and no state for a Use button to
    /// move; factoring the two together would mean a row builder taking
    /// half its arguments as None from one caller. The download button,
    /// the progress readout and the licence line are the parts that
    /// repeat, and they're four lines each.
    fn dictionary_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let dictionary = &rox_romanize::dictionary::IPADIC;
        let note = self.dictionary_note(cx);
        Section::new(
            q,
            icons::GLOBE,
            rox_i18n::t!("settings-dictionary-heading"),
            None,
            move |rows| {
                let installed = dictionary.installed();
                let size = dictionary.size_on_disk();
                let summary =
                    rox_i18n::try_translate(&format!("dictionary-summary-{}", dictionary.id))
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| dictionary.summary.to_string());
                let mut description = rox_i18n::t!(
                    "settings-dictionary-description",
                    summary = summary,
                    licence = dictionary.licence
                )
                .to_string();
                if installed && size > 0 {
                    description.push_str(&rox_i18n::t!(
                        "settings-mlmodels-on-disk",
                        size = human_size(size)
                    ));
                } else {
                    description.push_str(&rox_i18n::t!(
                        "settings-mlmodels-to-download",
                        size = human_size(dictionary.bytes)
                    ));
                }
                let rows = rows.row_dyn(
                    &["dictionary", "japanese", "romanize", "kanji", "download"],
                    dictionary.label,
                    Some(description.into()),
                    self.dictionary_controls(dictionary, cx),
                );
                match note {
                    Some(note) => rows.custom(&["dictionary", "download", "progress"], || {
                        coverage_note(note).into_any_element()
                    }),
                    None => rows,
                }
            },
        )
    }

    /// The dictionary row's buttons: where it came from, then the one
    /// button that changes state.
    fn dictionary_controls(
        &self,
        dictionary: &'static rox_romanize::dictionary::Dictionary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let downloading = self
            .dictionary_job
            .as_ref()
            .is_some_and(|job| job.dictionary() == dictionary.id);
        let source = dictionary.source;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .id(SharedString::from(format!(
                        "dictionary-source-{}",
                        dictionary.id
                    )))
                    .child(settings_ui::icon_button(
                        icons::EXTERNAL_LINK,
                        false,
                        move |_, _, cx| cx.open_url(source),
                    )),
            )
            .child(if downloading {
                let job = self
                    .dictionary_job
                    .clone()
                    .expect("downloading implies a job");
                small_button(
                    rox_i18n::format::format_percent(f64::from(job.fraction() * 100.0).round()),
                    icons::STOP,
                    false,
                    cx.listener(|_, _, _, cx| crate::romanize_job::dictionary::stop(cx)),
                )
            } else if dictionary.installed() {
                small_button(
                    rox_i18n::t!("settings-common-delete"),
                    icons::TRASH,
                    // Deleting under a running pass would pull the
                    // dictionary out from under it mid-title.
                    crate::romanize_job::progress(cx).is_some(),
                    cx.listener(move |this, _, _, cx| this.delete_dictionary(dictionary, cx)),
                )
            } else {
                small_button(
                    rox_i18n::t!("settings-common-download"),
                    icons::DOWNLOAD,
                    self.dictionary_job.is_some(),
                    cx.listener(move |this, _, _, cx| this.download_dictionary(dictionary, cx)),
                )
            })
            .into_any_element()
    }

    /// What the dictionary row says under itself: the download's progress,
    /// or why the last one didn't finish.
    fn dictionary_note(&self, cx: &Context<Self>) -> Option<String> {
        if let Some(job) = &self.dictionary_job {
            if job.stopping() {
                return Some(rox_i18n::t!("settings-dictionary-stopping").to_string());
            }
            return Some(
                rox_i18n::t!(
                    "settings-dictionary-downloading",
                    done = human_size(job.done()),
                    total = human_size(job.total())
                )
                .to_string(),
            );
        }
        let (_, reason) = crate::romanize_job::dictionary::last_failure(cx)?;
        Some(rox_i18n::t!("settings-dictionary-download-failed", reason = reason).to_string())
    }

    /// Fetch and unpack the dictionary.
    fn download_dictionary(
        &mut self,
        dictionary: &'static rox_romanize::dictionary::Dictionary,
        cx: &mut Context<Self>,
    ) {
        crate::romanize_job::dictionary::start(dictionary, cx);
        self.dictionary_job = crate::romanize_job::dictionary::progress(cx);
        Self::poll_dictionary(cx);
        cx.notify();
    }

    /// Drop the dictionary. What it already romanized stays in the
    /// library's tables: those rows are still the best answer rox has, and
    /// making a delete cost a re-run would turn a reclaim-some-disk into a
    /// second pass.
    fn delete_dictionary(
        &mut self,
        dictionary: &'static rox_romanize::dictionary::Dictionary,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = dictionary.delete() {
            log::error!("deleting {}: {e}", dictionary.id);
        }
        // Whatever the shared accessor handed out stays mapped, but the
        // next caller has to find out the files are gone.
        rox_romanize::reload();
        cx.notify();
    }

    /// Keep the dictionary row moving while its download runs. Its own
    /// loop rather than a branch in [`Self::poll_analyzing`], since the two
    /// downloads are independent and either can run without the other.
    fn poll_dictionary(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(RG_POLL).await;
            let live = this.update(cx, |this, cx| {
                this.dictionary_job = crate::romanize_job::dictionary::progress(cx);
                cx.notify();
                this.dictionary_job.is_some()
            });
            if !matches!(live, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Where the acoustic vectors come from: the catalog's downloads, or a
    /// checkpoint of the user's own.
    fn acoustic_models_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let kind = self.models_kind;
        let picker = panel::choices_shared(
            &model_kinds(),
            kind,
            |this: &mut Self, kind, cx| {
                this.models_kind = kind;
                cx.notify();
            },
            cx,
        )
        .into_any_element();
        // The download's progress and the last pass's failure belong to the
        // whole category, so they show under either half; a refused file is
        // the Custom half's own business and only shows there.
        let note = match kind {
            ModelKind::Custom => self
                .acoustic_local_error
                .clone()
                .or_else(|| self.model_note(cx)),
            ModelKind::Recommended => self.model_note(cx),
        };
        // A search shows both halves. The picker is a way of putting one of
        // them away, and a row nobody can find because it's behind a control
        // the searcher can't see is worse than a longer section.
        let searching = q.active();
        Section::new(
            q,
            icons::AUDIO_WAVEFORM,
            rox_i18n::t!("settings-acoustic-analysis-heading"),
            Some(picker),
            move |mut rows| {
                if searching || kind == ModelKind::Recommended {
                    rows = self.recommended_model_rows(rows, cx);
                }
                if searching || kind == ModelKind::Custom {
                    rows = self.custom_model_row(rows, cx);
                }
                match note {
                    Some(note) => rows.custom(&["download", "progress", "model"], || {
                        coverage_note(note).into_any_element()
                    }),
                    None => rows,
                }
            },
        )
    }

    /// The catalog half: one row per model rox knows how to fetch, each
    /// saying what it is, what it costs, and what licence it arrives under.
    ///
    /// The licence is on the row rather than in a footnote because the user
    /// is the one accepting it. Nothing here is bundled, so a download is
    /// them fetching a model for their own use, and that only works as an
    /// arrangement if the terms are in front of them when they press the
    /// button.
    fn recommended_model_rows<'a>(&self, mut rows: Rows<'a>, cx: &mut Context<Self>) -> Rows<'a> {
        // Only the ones with weights to fetch. The built-in extractor is
        // code rather than a model, and it belongs on the Library page as
        // the other side of the extractor switch, not on a shelf of
        // downloads.
        for model in rox_acoustic::models::CATALOG
            .iter()
            .filter(|model| model.weights.is_some())
        {
            let size = self.model_size(model.id);
            // The catalog is a domain crate and stays English; the summary
            // maps to a key here, at the UI layer, the way `Source::label`
            // does. A model with no message falls back to its own prose.
            let summary = rox_i18n::try_translate(&format!("model-summary-{}", model.id))
                .map(|s| s.to_string())
                .unwrap_or_else(|| model.summary.to_string());
            let mut description = rox_i18n::t!(
                "settings-mlmodels-description",
                summary = summary,
                dim = model.dim as u64,
                licence = model.licence
            )
            .to_string();
            if size > 0 {
                description.push_str(&rox_i18n::t!(
                    "settings-mlmodels-on-disk",
                    size = human_size(size)
                ));
            } else if let Some(weights) = &model.weights {
                description.push_str(&rox_i18n::t!(
                    "settings-mlmodels-to-download",
                    size = human_size(weights.bytes)
                ));
            }
            rows = rows.row_dyn(
                &["model", "acoustic", "download", "embeddings", "similar"],
                model.label,
                Some(description.into()),
                self.model_controls(model, cx),
            );
        }
        rows
    }

    /// The custom half: a weights file on disk, for a CNN10 someone trained
    /// or fine-tuned themselves. No download, no size to state before
    /// fetching, and no licence rox can name, since the file is the user's
    /// own.
    fn custom_model_row<'a>(&self, rows: Rows<'a>, cx: &mut Context<Self>) -> Rows<'a> {
        let busy = self.model_job.is_some() || self.acoustic_job.is_some();
        let checking = self.acoustic_local_checking;
        let local = self.acoustic_local.clone();
        let description = match &local {
            Some(local) => format!(
                "{}. {} values per track, stored under {}, so its vectors never mix with \
                 the catalog's",
                local.path.display(),
                rox_acoustic::panns::DIM,
                local.id
            ),
            None => rox_i18n::t!("settings-mlmodels-custom-description-empty").to_string(),
        };
        let mut controls = div().flex().flex_row().items_center().gap(tokens::SPACE_SM);
        if local.is_some() {
            controls = controls.child(small_button(
                rox_i18n::t!("settings-common-clear"),
                icons::TRASH,
                busy || checking,
                cx.listener(|this, _, _, cx| this.clear_local_model(cx)),
            ));
        }
        controls = controls.child(small_button(
            if checking {
                rox_i18n::t!("settings-mlmodels-checking")
            } else {
                rox_i18n::t!("settings-mlmodels-choose-file")
            },
            icons::FOLDER,
            busy || checking,
            cx.listener(|this, _, window, cx| this.pick_local_model(window, cx)),
        ));
        let running = local
            .as_ref()
            .is_some_and(|local| self.model_running(&local.id));
        controls = controls.child(if running {
            readout(rox_i18n::t!("settings-common-active").to_string()).into_any_element()
        } else {
            small_button(
                rox_i18n::t!("settings-common-use"),
                icons::CHECK,
                local.is_none() || busy || checking,
                cx.listener(|this, _, _, cx| this.use_local_model(cx)),
            )
            .into_any_element()
        });
        rows.row_dyn(
            &["custom", "model", "local", "weights", "checkpoint", "file"],
            rox_i18n::t!("settings-mlmodels-weights-file"),
            Some(description.into()),
            controls.into_any_element(),
        )
    }

    /// Whether a model row is the extractor the library is actually running.
    /// Kept apart from the buttons because the shelf and the custom row draw
    /// differently and have to match exactly on this. Being the page's pick
    /// isn't a state a row shows: with the Library page on
    /// Built-in nothing here is running, and a row claiming otherwise reads
    /// as though it were describing the library.
    fn model_running(&self, id: &str) -> bool {
        self.acoustic_source.id() == id
    }

    /// Browse for a weights file and check what comes back.
    fn pick_local_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        self.acoustic_local_error = None;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            this.update(cx, |this, cx| this.check_local_model(path, cx))
                .ok();
        })
        .detach();
    }

    /// Name a weights file by its hash, then check it by loading it, and take
    /// it as the custom model. The hash and the load both happen off the UI
    /// thread: one reads 25 MB, the other builds the network and runs a probe
    /// pass over it.
    ///
    /// Loading it is the validation. There's no checksum to compare against,
    /// so the only way to find out whether a file is this network is to have
    /// candle build it, which fails with the name of the missing tensor when
    /// it isn't.
    fn check_local_model(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.acoustic_local_checking = true;
        self.acoustic_local_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let checked = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        // Stamped before the read rather than after it: a file
                        // rewritten while this hashes then leaves a stamp that
                        // matches nothing, which reads as changed and hashes
                        // again, instead of one that vouches for bytes nobody
                        // ever hashed.
                        let stamp = settings::file_stamp(&path).unwrap_or_default();
                        let digest = rox_acoustic::models::hash_file(&path)?;
                        rox_acoustic::panns::Cnn10::load_from(&path)?;
                        Ok::<_, String>((rox_acoustic::local_id(&digest), stamp))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.acoustic_local_checking = false;
                match checked {
                    Ok((id, (bytes, mtime))) => this.adopt_local_model(
                        settings::LocalModel {
                            path,
                            id,
                            bytes,
                            mtime,
                        },
                        cx,
                    ),
                    Err(reason) => this.acoustic_local_error = Some(reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Take a checked file as the custom model and make it the page's offer,
    /// since browsing to a file is a clearer "use this" than any button on
    /// the row would be.
    fn adopt_local_model(&mut self, local: settings::LocalModel, cx: &mut Context<Self>) {
        self.acoustic_local = Some(local.clone());
        let stored = local.clone();
        // Before the pick below: resolving a local id reads this back out of
        // the settings file.
        Settings::update(move |s| s.acoustic_local_model = Some(stored.clone()));
        self.set_acoustic_model(
            rox_acoustic::Source::Local(Arc::new(rox_acoustic::Local {
                path: local.path,
                id: local.id,
            })),
            cx,
        );
    }

    /// Offer the custom model again after something else was picked. A file
    /// whose bytes have moved since it was named goes back through the check
    /// instead: the id is the hash, so a checkpoint retrained in place is a
    /// different model and has to be adopted under its own name rather than
    /// filling the old one's coordinates. Nothing else can adopt it for the
    /// user, since resolving that id rejects the file until it's re-hashed.
    fn use_local_model(&mut self, cx: &mut Context<Self>) {
        let Some(local) = self.acoustic_local.clone() else {
            return;
        };
        if settings::file_stamp(&local.path) != Some((local.bytes, local.mtime)) {
            self.check_local_model(local.path, cx);
            return;
        }
        self.set_acoustic_model(
            rox_acoustic::Source::Local(Arc::new(rox_acoustic::Local {
                path: local.path,
                id: local.id,
            })),
            cx,
        );
    }

    /// Forget the custom model. What it described stays in the database under
    /// its own name, the way a deleted download's vectors do: point rox at
    /// the same file again and it comes back to the work it already did.
    fn clear_local_model(&mut self, cx: &mut Context<Self>) {
        let was = self.acoustic_local.take();
        self.acoustic_local_error = None;
        Settings::update(|s| s.acoustic_local_model = None);
        // Nothing to fall back to but the catalog, so a library running the
        // file that just went away moves off it rather than failing at the
        // next pass.
        if was.is_some_and(|local| self.acoustic_source.id() == local.id) {
            self.use_extractor(rox_acoustic::MODEL, cx);
        }
        self.acoustic_ml_source = rox_services::acoustic::acoustic_ml_source();
        cx.notify();
    }

    /// What each catalog model weighs on disk right now. Walked entering the
    /// page and after anything that changes it, never in a paint.
    fn measure_models() -> Vec<(&'static str, u64)> {
        rox_acoustic::models::CATALOG
            .iter()
            .map(|model| (model.id, model.size_on_disk()))
            .collect()
    }

    fn model_size(&self, id: &str) -> u64 {
        self.model_sizes
            .iter()
            .find(|(name, _)| *name == id)
            .map(|(_, size)| *size)
            .unwrap_or(0)
    }

    /// One model row's buttons: make it the page's offer, and fetch or drop
    /// its weights. A model whose download is running shows the stop instead,
    /// and one that isn't installed can't be offered, since selecting a model
    /// rox can't load would leave the pass failing with no explanation on the
    /// row that caused it.
    fn model_controls(
        &self,
        model: &'static rox_acoustic::models::Model,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Deleting the running model would leave the pass with nothing to
        // load, so it holds its weights until something else is picked.
        let running = self.model_running(model.id);
        let installed = model.installed();
        let downloading = self
            .model_job
            .as_ref()
            .is_some_and(|job| job.model() == model.id);
        let busy = self.model_job.is_some() || self.acoustic_job.is_some();

        // Where the model came from, so someone deciding whether to fetch
        // 24 MB and accept its licence can go and read about it first.
        let source = model.source;
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .id(SharedString::from(format!("model-source-{}", model.id)))
                    .child(settings_ui::icon_button(
                        icons::EXTERNAL_LINK,
                        false,
                        move |_, _, cx| cx.open_url(source),
                    )),
            );
        if model.weights.is_some() {
            row = row.child(if downloading {
                let job = self.model_job.clone().expect("downloading implies a job");
                small_button(
                    rox_i18n::format::format_percent(f64::from(job.fraction() * 100.0).round()),
                    icons::STOP,
                    false,
                    cx.listener(|_, _, _, cx| embeddings::models::stop(cx)),
                )
            } else if installed {
                small_button(
                    rox_i18n::t!("settings-common-delete"),
                    icons::TRASH,
                    busy || running,
                    cx.listener(move |this, _, _, cx| this.delete_model(model, cx)),
                )
            } else {
                small_button(
                    rox_i18n::t!("settings-common-download"),
                    icons::DOWNLOAD,
                    busy,
                    cx.listener(move |this, _, _, cx| this.download_model(model, cx)),
                )
            });
        }
        // Only the running extractor gets a word. Pressing Use on a row while
        // the Library page is on Built-in points the Model switch here, which
        // the Extractor row spells out by name, so the shelf doesn't need a
        // second mark for it.
        row = row.child(if running {
            readout(rox_i18n::t!("settings-common-active").to_string()).into_any_element()
        } else {
            small_button(
                rox_i18n::t!("settings-common-use"),
                icons::CHECK,
                !installed || busy,
                cx.listener(move |this, _, _, cx| {
                    this.set_acoustic_model(rox_acoustic::Source::Catalog(model), cx)
                }),
            )
            .into_any_element()
        });
        row.into_any_element()
    }

    /// The line under the model list: what a running download is doing, or
    /// why the last one or the last pass gave up.
    fn model_note(&self, cx: &Context<Self>) -> Option<String> {
        if let Some(job) = &self.model_job {
            let label = self.label_for(
                &job.model(),
                rox_i18n::t_static("settings-mlmodels-fallback-model"),
            );
            if job.stopping() {
                return Some(rox_i18n::t!("settings-mlmodels-stopping", label = label).to_string());
            }
            return Some(
                rox_i18n::t!(
                    "settings-mlmodels-downloading",
                    label = label,
                    done = human_size(job.done()),
                    total = human_size(job.total())
                )
                .to_string(),
            );
        }
        if let Some((id, reason)) = embeddings::models::last_failure(cx) {
            let label = self.label_for(
                &id,
                rox_i18n::t_static("settings-mlmodels-fallback-the-model"),
            );
            return Some(
                rox_i18n::t!(
                    "settings-mlmodels-download-failed",
                    label = label,
                    reason = reason
                )
                .to_string(),
            );
        }
        // A pass that failed to start is nearly always the model rather than
        // the library, so its reason belongs on this section.
        embeddings::last_failure(cx).map(|reason| {
            rox_i18n::t!("settings-mlmodels-pass-stopped", reason = reason).to_string()
        })
    }

    /// Make a model the active one: what the pass fills and what the
    /// similarity queries read. The coverage line re-counts against it, so
    /// switching immediately says how much of the library this model has
    /// actually described rather than keeping the last one's number. When
    /// the library is already running a model rather than the built-in
    /// extractor, the switch follows the new pick straight away; when it's
    /// on Built-in this only changes what Model would mean.
    fn set_acoustic_model(&mut self, source: rox_acoustic::Source, cx: &mut Context<Self>) {
        let id = source.id().to_string();
        self.acoustic_ml_source = source;
        let stored = id.clone();
        Settings::update(move |s| s.acoustic_ml_model = stored.clone());
        if !self.acoustic_source.is_builtin() {
            self.use_extractor(&id, cx);
        }
        cx.notify();
    }

    /// The Library page's switch: run the built-in sketch, or the model the
    /// ML Models page is offering.
    fn set_acoustic_uses_model(&mut self, on: bool, cx: &mut Context<Self>) {
        let id = if on {
            self.acoustic_ml_source.id().to_string()
        } else {
            rox_acoustic::MODEL.to_string()
        };
        self.use_extractor(&id, cx);
        cx.notify();
    }

    /// Point the library at an extractor and re-read its coverage. Every
    /// model describes the library separately, so the count has to follow the
    /// pick rather than the pick alone.
    ///
    /// The live pick is set from the id and then read back rather than
    /// assigned here, so this window can't end up showing a model the rest of
    /// the app refused to resolve.
    fn use_extractor(&mut self, id: &str, cx: &mut Context<Self>) {
        let owned = id.to_string();
        Settings::update(move |s| s.acoustic_model = owned.clone());
        rox_services::acoustic::set_acoustic_model(id, cx);
        self.acoustic_source = rox_services::acoustic::acoustic_source();
        self.acoustic_coverage = self
            .library
            .read(cx)
            .acoustic_coverage(self.acoustic_source.id());
        // Every model describes the library separately, so switching can turn
        // ordering by sound on or off for the surfaces that offer it.
        let described = self.library.read(cx).analyzed(self.acoustic_source.id());
        settings::set_acoustic_described(described, cx);
    }

    fn download_model(
        &mut self,
        model: &'static rox_acoustic::models::Model,
        cx: &mut Context<Self>,
    ) {
        embeddings::models::start(model, cx);
        self.model_job = embeddings::models::progress(cx);
        Self::poll_analyzing(cx);
        cx.notify();
    }

    /// Drop a model's weights. The vectors it already wrote stay: they're
    /// still valid, and making a delete cost a full re-analysis would turn a
    /// reclaim-some-disk into an afternoon.
    fn delete_model(
        &mut self,
        model: &'static rox_acoustic::models::Model,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = model.delete() {
            log::error!("deleting {}: {e}", model.id);
        }
        self.model_sizes = Self::measure_models();
        cx.notify();
    }

    /// Acoustic analysis, on the Library page because that's what it
    /// describes: the switch, which extractor runs, and how far it has got.
    ///
    /// The extractor choice is two options rather than a list of every model,
    /// because the shelf is on the ML Models page. This is a job picking a
    /// tool off it, so the question here is only built-in or the model, and
    /// which model is the other page's business.
    fn acoustic_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let on = self.acoustic_analysis;
        let auto = self.acoustic_auto;
        let note = on.then(|| self.acoustic_note());
        let ml_label = self.acoustic_ml_source.label();
        let installed = self.acoustic_ml_source.installed();
        Section::new(
            q,
            icons::AUDIO_WAVEFORM,
            rox_i18n::t!("settings-acoustic-analysis-heading"),
            on.then(|| self.acoustic_control(cx)),
            move |mut rows| {
                rows = rows.keyed(
                    "settings-library-acoustic-enable",
                    &["acoustic", "embeddings", "similar", "analysis"],
                    panel::toggle(on, Self::set_acoustic_analysis, cx),
                );
                if !on {
                    return rows;
                }
                rows = rows.row_dyn(
                    &["extractor", "model", "built-in", "quality"],
                    rox_i18n::t!("settings-library-acoustic-extractor"),
                    Some(
                        if installed {
                            format!(
                                "Built-in needs no download and describes timbre and rhythm. \
                                 {ml_label} hears more, and is the model the ML Models page is \
                                 offering. Switching re-describes the library under the new one"
                            )
                        } else {
                            format!(
                                "Built-in needs no download and describes timbre and rhythm. \
                                 No model is installed, so there's nothing to switch to yet: \
                                 download {ml_label}, or pick a weights file of your own, on \
                                 the ML Models page"
                            )
                        }
                        .into(),
                    ),
                    // Model is dimmed with nothing installed rather than
                    // simply refused. Pressing it used to fall straight back
                    // to Built-in, since the pick can't resolve, which looks
                    // like a broken button rather than a missing download.
                    panel::choices_gated(
                        &[
                            (rox_i18n::t!("settings-common-built-in"), false),
                            (
                                rox_i18n::t!("settings-library-acoustic-extractor-model"),
                                true,
                            ),
                        ],
                        !self.acoustic_source.is_builtin(),
                        move |model| !model || installed,
                        Self::set_acoustic_uses_model,
                        cx,
                    ),
                );
                rows = rows.keyed(
                    "settings-library-acoustic-save",
                    &["save", "write", "tags", "database", "vectors"],
                    panel::choices_shared(
                        &[
                            (
                                rox_i18n::t!("settings-common-database"),
                                AcousticSave::Database,
                            ),
                            (rox_i18n::t!("settings-common-tags"), AcousticSave::Tags),
                        ],
                        self.acoustic_save,
                        Self::set_acoustic_save,
                        cx,
                    ),
                );
                rows = rows.keyed(
                    "settings-library-acoustic-auto",
                    &["automatic", "auto", "new files", "watch"],
                    panel::toggle(auto, Self::set_acoustic_auto, cx),
                );
                match note {
                    Some(note) => rows
                        .custom(&["coverage", "analyze", "missing", "progress"], || {
                            coverage_note(note).into_any_element()
                        }),
                    None => rows,
                }
            },
        )
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

    fn acoustic_note(&self) -> String {
        if let Some(job) = &self.acoustic_job {
            // The running pass's own model, not the current pick: switching
            // models mid-pass is possible, and the line should say what's
            // actually being written.
            let running = self.label_for(
                &job.model(),
                rox_i18n::t_static("settings-library-acoustic-fallback"),
            );
            let total = job.total();
            if total == 0 {
                return rox_i18n::t!(
                    "settings-library-acoustic-progress-start",
                    running = running
                )
                .to_string();
            }
            let mut line = rox_i18n::t!(
                "settings-library-acoustic-progress",
                running = running,
                done = job.done().min(total) as u64,
                total = total as u64
            )
            .to_string();
            // The pass's own measured rate, which prices whatever worker
            // count it's actually running with.
            if let Some(eta) = job.eta_secs() {
                line.push_str(&rox_i18n::t!(
                    "tasks-time-left",
                    left = rox_core::pace::human(eta)
                ));
            }
            let current = job.current();
            if let Some(name) = Path::new(&current).file_name() {
                line.push_str(&format!(
                    " {}",
                    rox_i18n::t!(
                        "tasks-file-suffix",
                        file = name.to_string_lossy().to_string()
                    )
                ));
            }
            let failed = job.failed();
            if failed > 0 {
                line.push_str(&format!(
                    " {}",
                    rox_i18n::t!("tasks-failed-suffix", count = failed as u64)
                ));
            }
            return line;
        }
        let coverage = self.acoustic_coverage;
        if coverage.total == 0 {
            return rox_i18n::t!("settings-analyze-nothing-scanned").to_string();
        }
        // Named, because the count is per model: every model describes the
        // library separately, and a line that said "142 of 208" without
        // saying whose would read as the library's own progress.
        let label = self.acoustic_source.label();
        if coverage.missing() == 0 {
            return rox_i18n::t!(
                "settings-library-acoustic-all-described",
                total = coverage.total as u64,
                label = label
            )
            .to_string();
        }
        let mut line = rox_i18n::t!(
            "settings-library-acoustic-partial",
            label = label,
            done = coverage.embedded as u64,
            total = coverage.total as u64
        )
        .to_string();
        // Priced off what the last pass measured on this machine for this
        // model, scaled to the worker setting, so dragging the slider shows
        // what it buys. Quiet until a pass has measured anything: a number
        // invented from constants would be wrong on every machine but one.
        if let Some(estimate) = self.acoustic_estimate(coverage.missing()) {
            line.push_str(&format!(
                " {}",
                rox_i18n::t!(
                    "tasks-estimate-at-workers",
                    estimate = estimate,
                    workers = rox_core::pace::workers_phrase(self.acoustic_workers)
                )
            ));
        }
        line
    }

    /// A rough cost for analyzing `missing` tracks at the current worker
    /// setting, off the pace the last pass over this model measured here.
    /// None until one has.
    fn acoustic_estimate(&self, missing: usize) -> Option<String> {
        let pace = *self.acoustic_pace.get(self.acoustic_source.id())?;
        rox_core::pace::estimate(pace, missing as u64, self.acoustic_workers)
    }

    /// Start the pass, or stop the one running. Inert with nothing missing,
    /// and while the library is scanning, since a scan rewrites the very
    /// rows the pass reads.
    fn acoustic_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = &self.acoustic_job {
            let stopping = job.stopping();
            return small_button(
                if stopping {
                    rox_i18n::t!("settings-common-stopping")
                } else {
                    rox_i18n::t!("settings-common-stop")
                },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| embeddings::stop(cx)),
            )
            .into_any_element();
        }
        // Also inert while a model is coming down: the pass would load the
        // half-written file, and the download is the thing that has to
        // finish first anyway.
        let idle = self.acoustic_coverage.missing() == 0
            || self.library.read(cx).busy().is_some()
            || self.model_job.is_some();
        small_button(
            rox_i18n::t!("settings-common-analyze-missing"),
            icons::FLASK,
            idle,
            cx.listener(|this, _, _, cx| {
                let library = this.library.clone();
                pass_prompt::raise(this, pass_prompt::Pass::Acoustic, library, cx);
            }),
        )
        .into_any_element()
    }

    /// Copy the running pass into the section, `poll_measuring`'s twin.
    /// Stops itself once the pass clears the global, and refreshes the
    /// coverage once on the way out so the line ends on the final count.
    ///
    /// Covers the model download on the same timer rather than on one of its
    /// own: the two never run together (the analyze button is inert while a
    /// download runs), and one loop means one place that decides when the
    /// section has stopped moving.
    fn poll_analyzing(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(RG_POLL).await;
            let live = this.update(cx, |this, cx| {
                let was_analyzing = this.acoustic_job.is_some();
                this.acoustic_job = embeddings::progress(cx);
                let was_downloading = this.model_job.is_some();
                this.model_job = embeddings::models::progress(cx);
                // Only the pass moves the count, so it's re-read on the tick
                // the pass ends rather than on every tick: this loop also runs
                // for the whole length of a model download, and the count is a
                // walk of the tracks table on the UI thread.
                if was_analyzing && this.acoustic_job.is_none() {
                    this.acoustic_coverage = this
                        .library
                        .read(cx)
                        .acoustic_coverage(this.acoustic_source.id());
                    // The pass that just ended wrote what it measured per
                    // track; pick it up so the next estimate prices off it.
                    this.acoustic_pace = Settings::load().session.acoustic_pace.clone();
                }
                // A finished download changed what's on disk, so the sizes
                // and the install marks have to be re-walked once.
                if was_downloading && this.model_job.is_none() {
                    this.model_sizes = Self::measure_models();
                }
                cx.notify();
                this.acoustic_job.is_some() || this.model_job.is_some()
            });
            if !matches!(live, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Tempo analysis, under the acoustic section because it's the same
    /// kind of thing: a pass over the audio that fills a column in. One
    /// switch and one line, since there's nothing to pick: no model, and
    /// nowhere but the database for the numbers to go.
    fn tempo_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let on = self.tempo_analysis;
        let auto = self.tempo_auto;
        let note = on.then(|| self.tempo_note());
        Section::new(
            q,
            icons::CLOCK,
            rox_i18n::t!("settings-library-section-tempo"),
            on.then(|| self.tempo_control(cx)),
            move |mut rows| {
                rows = rows.keyed(
                    "settings-library-tempo-enable",
                    &["tempo", "bpm", "analysis"],
                    panel::toggle(on, Self::set_tempo_analysis, cx),
                );
                if on {
                    rows = rows.keyed(
                        "settings-library-tempo-auto",
                        &["automatic", "auto", "new files", "watch"],
                        panel::toggle(auto, Self::set_tempo_auto, cx),
                    );
                }
                match note {
                    Some(note) => rows.custom(
                        &["coverage", "analyze", "missing", "refused", "progress"],
                        || coverage_note(note).into_any_element(),
                    ),
                    None => rows,
                }
            },
        )
    }

    /// The tempo switch. It's the feature as well as the permission: with
    /// it off nothing measures, the BPM column isn't offered, and the pass
    /// no-ops even if something asks it to run.
    fn set_tempo_analysis(&mut self, on: bool, cx: &mut Context<Self>) {
        self.tempo_analysis = on;
        Settings::update(move |s| s.tempo_analysis = on);
        settings::set_tempo_analysis(on, cx);
        cx.notify();
    }

    /// The follow-the-watcher switch for the tempo pass, the acoustic
    /// setter's twin: the backlog gets priced on the way on, and a decline
    /// puts the switch back through `pass_refused`.
    fn set_tempo_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.tempo_auto = on;
        Settings::update(move |s| s.tempo_auto = on);
        if on && self.bpm_coverage.missing > 0 && self.tempo_job.is_none() {
            let library = self.library.clone();
            pass_prompt::raise_for_switch(
                self,
                pass_prompt::Pass::Tempo {
                    retry_refused: false,
                },
                library,
                cx,
            );
        }
        cx.notify();
    }

    /// The line under the tempo switch: what a running pass is doing, or
    /// where the library stands. The three-way split is worth spelling out
    /// for the ReplayGain section's reason: a number rox worked out and a
    /// number the file arrived with are not the same claim.
    fn tempo_note(&self) -> String {
        if let Some(job) = &self.tempo_job {
            let total = job.total();
            if total == 0 {
                return rox_i18n::t!("settings-library-tempo-progress-start").to_string();
            }
            let mut line = rox_i18n::t!(
                "settings-library-tempo-progress",
                done = job.done().min(total) as u64,
                total = total as u64
            )
            .to_string();
            if let Some(eta) = job.eta_secs() {
                line.push_str(&rox_i18n::t!(
                    "tasks-time-left",
                    left = rox_core::pace::human(eta)
                ));
            }
            let current = job.current();
            if let Some(name) = Path::new(&current).file_name() {
                line.push_str(&format!(
                    " {}",
                    rox_i18n::t!(
                        "tasks-file-suffix",
                        file = name.to_string_lossy().to_string()
                    )
                ));
            }
            let failed = job.failed();
            if failed > 0 {
                line.push_str(&format!(
                    " {}",
                    rox_i18n::t!("tasks-no-beat-suffix", count = failed as u64)
                ));
            }
            return line;
        }
        let split = self.bpm_coverage;
        let total = split.total();
        if total == 0 {
            return rox_i18n::t!("settings-analyze-nothing-scanned").to_string();
        }
        // Refused gets its own sentence rather than a share of either count.
        // They're not missing, since nothing will pick them up again on its
        // own, and they're not covered either, so folding them into one of
        // the two would misreport it.
        let refused = match split.refused {
            0 => String::new(),
            // Carries its own sentence break, the way the other appended
            // messages carry their leading comma: where one sentence ends
            // and the next starts is the translator's call, not something
            // to hard-code as ". " here and get wrong in Japanese.
            count => rox_i18n::t!("settings-library-tempo-refused", count = count).to_string(),
        };
        // The missing check rides along because a library where every track
        // was refused has no tempos and no work either, and this line offers
        // to do some.
        if split.covered() == 0 && split.missing > 0 {
            return format!(
                "{}{}{refused}",
                rox_i18n::t!("settings-library-tempo-status-none", total = total),
                self.tempo_estimate_suffix(split.missing)
            );
        }
        if split.missing > 0 {
            return format!(
                "{}{}{refused}",
                rox_i18n::t!(
                    "settings-library-tempo-status-partial",
                    covered = split.covered(),
                    total = total,
                    measured = split.measured,
                    missing = split.missing
                ),
                self.tempo_estimate_suffix(split.missing)
            );
        }
        // Nothing left to reach, but a refused pile still means the library
        // isn't fully timed, so the "all of them" wording is kept for the
        // case where it's true of every scanned track.
        if split.refused > 0 {
            let line = if split.measured > 0 {
                rox_i18n::t!(
                    "settings-library-tempo-status-measured-some",
                    covered = split.covered(),
                    total = total,
                    measured = split.measured
                )
            } else {
                rox_i18n::t!(
                    "settings-library-tempo-status-tagged-some",
                    covered = split.covered(),
                    total = total
                )
            };
            return format!("{line}{refused}");
        }
        if split.measured > 0 {
            return rox_i18n::t!(
                "settings-library-tempo-status-measured",
                total = total,
                measured = split.measured
            )
            .to_string();
        }
        rox_i18n::t!("settings-library-tempo-status-tagged", total = total).to_string()
    }

    /// A rough cost for working out `missing` tempos at the current worker
    /// setting, ready to append to the line above, or nothing until a pass
    /// has measured this machine's pace.
    fn tempo_estimate_suffix(&self, missing: u64) -> String {
        match rox_core::pace::estimate(self.tempo_pace, missing, self.tempo_workers) {
            Some(estimate) => format!(
                " {}",
                rox_i18n::t!(
                    "tasks-estimate-at-workers",
                    estimate = estimate,
                    workers = rox_core::pace::workers_phrase(self.tempo_workers)
                )
            ),
            None => String::new(),
        }
    }

    /// Start the pass, or stop the one running. Inert with nothing missing,
    /// and while the library is busy, since a scan rewrites the very rows
    /// the pass reads.
    fn tempo_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = &self.tempo_job {
            let stopping = job.stopping();
            return small_button(
                if stopping {
                    rox_i18n::t!("settings-common-stopping")
                } else {
                    rox_i18n::t!("settings-common-stop")
                },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| tempo_job::stop(cx)),
            )
            .into_any_element();
        }
        let busy = self.library.read(cx).busy().is_some();
        // Retry Refused stands beside Analyze Missing rather than replacing
        // it: the two work through different piles, and the refused one is
        // the only work left once missing hits zero. It goes inert with an
        // empty pile the way its neighbour does with nothing missing, so the
        // pair reads as two standing offers rather than a button that comes
        // and goes.
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                rox_i18n::t!("settings-common-analyze-missing"),
                icons::CLOCK,
                self.bpm_coverage.missing == 0 || busy,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    pass_prompt::raise(
                        this,
                        pass_prompt::Pass::Tempo {
                            retry_refused: false,
                        },
                        library,
                        cx,
                    );
                }),
            ))
            .child(small_button(
                rox_i18n::t!("settings-library-tempo-retry"),
                icons::REFRESH_CW,
                self.bpm_coverage.refused == 0 || busy,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    pass_prompt::raise(
                        this,
                        pass_prompt::Pass::Tempo {
                            retry_refused: true,
                        },
                        library,
                        cx,
                    );
                }),
            ))
            .into_any_element()
    }

    /// Copy the running tempo pass into the section, `poll_measuring`'s
    /// twin. Stops itself once the pass clears the global, and re-reads the
    /// split on the way out so the line ends on the final count.
    fn poll_timing(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(RG_POLL).await;
            let live = this.update(cx, |this, cx| {
                let was = this.tempo_job.is_some();
                this.tempo_job = tempo_job::progress(cx);
                if was && this.tempo_job.is_none() {
                    this.bpm_coverage = this.library.read(cx).bpm_breakdown();
                    // The pass that just ended wrote what it measured per
                    // track; pick it up so the next estimate prices off it.
                    this.tempo_pace = Settings::load().session.tempo_pace;
                }
                cx.notify();
                this.tempo_job.is_some()
            });
            if !matches!(live, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Open a page and leave search: what a sidebar click and a
    /// result breadcrumb both do, so arriving anywhere reads the same.
    /// Entering Storage measures the files fresh, so the numbers are
    /// current without a per-frame stat.
    fn open_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        if page == Page::Storage {
            self.refresh_storage(cx);
        }
        cx.notify();
    }

    /// One page filtered through the query: the single-page view passes
    /// the inactive query and gets the whole page, search passes the
    /// live one and takes the survivors.
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
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("settings-search-no-matches", text = text))
                .into_any_element();
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
                                    .child(label)
                                    .child(rule()),
                            )
                            .child(body.element())
                    }),
            )
            .into_any_element()
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

/// How far a layout tree row steps in per depth.
fn indent(depth: usize) -> Pixels {
    px(14. * depth as f32)
}

/// A layout tree node's position among its siblings, for the reorder
/// arrows: inside a split, inside a tab group, or nowhere movable (the
/// dock root, and a composite's hosted children, which the composite
/// orders itself).
#[derive(Clone)]
enum TreeSlot {
    Root,
    Stack {
        stack: Entity<StackPanel>,
        ix: usize,
        len: usize,
    },
    Tabs {
        tabs: Entity<TabPanel>,
        ix: usize,
        len: usize,
    },
    Hosted,
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

/// A structure line of the layout tree: a split or tab group, muted so
/// the panel rows lead the page, with the move controls on the right
/// edge when the node can move. Padded to the icon buttons' height so
/// the tree keeps one rhythm with and without controls.
fn chrome_row(
    ix: usize,
    depth: usize,
    label: &'static str,
    controls: Option<AnyElement>,
) -> AnyElement {
    div()
        // The tree's rows all carry the same arrows, so each is named
        // after its place in the tree to keep them apart for the
        // keyboard. See `rox_panel_kit::ui::control_focus`.
        .id(ElementId::NamedInteger("tree-row".into(), ix as u64))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .pl(indent(depth))
        .group(TREE_ROW_GROUP)
        .text_xs()
        .text_color(palette::text_muted())
        .child(label)
        .when_some(controls, |d, controls| d.child(controls))
        .into_any_element()
}

/// The badge a shipped layout or workspace gets in its list row, telling
/// the app's own read-only entries from the user's saved ones.
fn shipped_tag() -> Div {
    div()
        .flex_none()
        .px(tokens::SPACE_SM)
        .py(px(2.))
        .text_xs()
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .text_color(palette::text_muted())
        .child(rox_i18n::t!("settings-common-built-in"))
}

/// A role badge on a preset row: lit like a filled control when the preset
/// holds the role, a plain chip otherwise. Clicking toggles the role.
fn role_chip(
    label: &'static str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .px(tokens::SPACE_SM)
        .py(px(2.))
        .text_xs()
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .map(|d| {
            if active {
                d.bg(palette::accent())
                    .text_color(palette::text_on_accent())
            } else {
                d.bg(palette::bg_control())
                    .text_color(palette::text_muted())
                    .hover(|d| d.bg(palette::bg_control_hover()))
            }
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
}

/// One right-aligned numeric cell of the folder table.
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

/// The MCP page's copy-ready client config: the rox-mcp binary beside this
/// executable, in the mcpServers shape every stdio client reads. A portable
/// run points the proxy at its own data folder, since the socket is keyed
/// to it; the stock run needs no arguments at all.
fn mcp_config_snippet() -> String {
    let binary = format!("rox-mcp{}", std::env::consts::EXE_SUFFIX);
    let command = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&binary)))
        .map(|path| path.display().to_string())
        .unwrap_or(binary);
    let mut server = serde_json::json!({ "command": command });
    if settings::portable() {
        server["args"] =
            serde_json::json!(["--data-dir", settings::data_dir().display().to_string(),]);
    }
    let config = serde_json::json!({ "mcpServers": { "rox": server } });
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

/// The hover note behind the Experimental badge and its issue button. Same
/// card the track info chip's tooltip uses, so the explanation reads the
/// same wherever it pops up.
struct ExperimentalTooltip(SharedString);

impl Render for ExperimentalTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(tokens::SPACE_SM)
            .max_w(px(320.))
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border())
            .bg(palette::bg_menu_opaque())
            .shadow_md()
            .text_xs()
            .text_color(palette::text())
            .child(self.0.clone())
    }
}

/// Percent-encode a string for a GitHub issue URL's query. Only the handful
/// of characters that break a query string; anything else passes through,
/// since the issue form is forgiving and over-encoding makes the URL
/// unreadable in logs.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The exclusive toggle as the output layer's mode. The two device lists
/// don't share ids, so which one to ask for follows the toggle rather than
/// what happens to be running.
fn output_mode(exclusive: bool) -> output::Mode {
    if exclusive {
        output::Mode::Exclusive
    } else {
        output::Mode::Shared
    }
}

/// The scan folders with their rollups blank, what the table shows until
/// [`SettingsWindow::measure_root_stats`] comes back. Reading the roots
/// costs nothing; it's counting under them that's the scan.
fn seed_root_stats(library: &Entity<Library>, cx: &App) -> Vec<(PathBuf, Stats)> {
    library
        .read(cx)
        .roots()
        .into_iter()
        .map(|root| (root, Stats::default()))
        .collect()
}

/// Bytes as a short human size: whole numbers through KB, one decimal
/// from MB up, decimal units like the file managers show.
fn human_size(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["KB", "MB", "GB", "TB"] {
        if value < 1000. {
            break;
        }
        value /= 1000.;
        unit = next;
    }
    match unit {
        "B" => rox_i18n::format::format_unit(bytes as f64, 0, "B"),
        "KB" => rox_i18n::format::format_unit(value, 0, "KB"),
        _ => rox_i18n::format::format_unit(value, 1, unit),
    }
}

/// A SQLite database's weight on disk: the file plus its -wal and -shm
/// sidecars, which hold real data between checkpoints.
fn db_size(db: &Path) -> u64 {
    ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let mut file = db.as_os_str().to_owned();
            file.push(suffix);
            std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0)
        })
        .sum()
}

/// Copy a folder tree whole, files and subfolders. The portable seed:
/// stops on the first error so a half copy reports as one instead of
/// passing for done.
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Every file under one folder, subfolders and all. The caches are flat,
/// but the ejected shaders are nested a folder per workspace deep and the icon
/// packs a folder per pack, and a walk that stopped at the top would report
/// those as nothing. A symlink is measured as the link rather than followed,
/// so nothing here can walk in a circle.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            _ => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
        })
        .sum()
}

/// One file's weight, zero when it isn't there.
fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// The pass prompt's host side: where the dialog's state is kept on this
/// window, and what the window re-reads once the dialog has done something.
impl pass_prompt::Host for SettingsWindow {
    fn prompt(&self) -> Option<&pass_prompt::Prompt> {
        self.prompt.as_ref()
    }

    fn prompt_mut(&mut self) -> &mut Option<pass_prompt::Prompt> {
        &mut self.prompt
    }

    fn value_edit(&self) -> &panel::ValueEdit {
        &self.value_edit
    }

    fn dialog_focus(&self) -> &FocusHandle {
        &self.dialog_focus
    }

    /// Everything the pages state about the passes, re-read at once: the
    /// counts a start just changed, the pace a probe just measured, and the
    /// worker counts the dialog's slider wrote.
    fn pass_changed(&mut self, cx: &mut Context<Self>) {
        let settings = Settings::load();
        self.acoustic_workers = settings.acoustic_workers.max(1);
        self.rg_workers = settings.replaygain_workers.max(1);
        self.tempo_workers = settings.tempo_workers.max(1);
        self.acoustic_pace = settings.session.acoustic_pace.clone();
        self.rg_pace = settings.session.replaygain_pace;
        self.tempo_pace = settings.session.tempo_pace;
        self.acoustic_coverage = self
            .library
            .read(cx)
            .acoustic_coverage(self.acoustic_source.id());
        self.rg_coverage = self.library.read(cx).replaygain_breakdown();
        self.bpm_coverage = self.library.read(cx).bpm_breakdown();
        let (was_analyzing, was_measuring) = (self.acoustic_job.is_some(), self.rg_job.is_some());
        let was_timing = self.tempo_job.is_some();
        self.acoustic_job = embeddings::progress(cx);
        self.rg_job = replaygain_job::progress(cx);
        self.tempo_job = tempo_job::progress(cx);
        // A pass that just started needs its poll; one that was already
        // running has a loop and doesn't need a second.
        if !was_analyzing && self.acoustic_job.is_some() {
            Self::poll_analyzing(cx);
        }
        if !was_measuring && self.rg_job.is_some() {
            Self::poll_measuring(cx);
        }
        if !was_timing && self.tempo_job.is_some() {
            Self::poll_timing(cx);
        }
        cx.notify();
    }

    /// The backlog behind a follow-the-watcher switch was declined, so the
    /// switch was a no as well: put it back, rather than leave it on to start
    /// the pass it just refused at the next watch sync.
    fn pass_refused(&mut self, pass: pass_prompt::Pass, cx: &mut Context<Self>) {
        match pass {
            pass_prompt::Pass::Acoustic => {
                self.acoustic_auto = false;
                Settings::update(|s| s.acoustic_auto = false);
            }
            pass_prompt::Pass::ReplayGain => {
                self.playback
                    .update(cx, |player, cx| player.set_replay_gain_auto(false, cx));
            }
            pass_prompt::Pass::Tempo { .. } => {
                self.tempo_auto = false;
                Settings::update(|s| s.tempo_auto = false);
            }
            // No switch stands behind either of the last two, so nothing
            // here ever raises them through `raise_for_switch` and there's
            // nothing to put back.
            pass_prompt::Pass::SortNames { .. } | pass_prompt::Pass::Romanize => {}
        }
    }
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

        // The Shader page builds from `&self`, so the shader route
        // editor's sliders and folds are matched to the list here, before
        // any page renders. Search builds every page each keystroke, which
        // is the other reason it can't happen down there.
        self.post_shader_route_ui
            .sync(self.post_shader_routes.len());

        // A live query builds every page and stacks the survivors; the
        // sidebar dims the pages that kept nothing. No query builds just
        // the picked page through the same path, with the inactive query
        // keeping everything.
        let text = self.search.read(cx).query().trim().to_string();
        let q = Query::parse(&text);
        // The AI toggle takes the MCP and ML Models pages out of the list
        // entirely, search included: a page that isn't on offer shouldn't
        // surface its rows either.
        let pages: Vec<(Page, &str, &str)> = PAGES
            .iter()
            .copied()
            .filter(|&(page, ..)| self.ai_enabled || !matches!(page, Page::Mcp | Page::MlModels))
            .collect();
        let results: Option<Vec<_>> = q.active().then(|| {
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
                        .child(self.search.update(cx, |search, cx| search.element(cx))),
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

            let page = match results {
                Some(results) => self.search_results(&text, results, cx),
                None => self
                    .build_page(self.page, &q, columns, window, cx)
                    .element(),
            };

            div()
                .size_full()
                .track_focus(&self.focus)
                // The settings shortcut everywhere: focus goes to the
                // search box, the Apple way in.
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "f" && event.keystroke.modifiers.secondary() {
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
    use super::{Page, PAGES};
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
