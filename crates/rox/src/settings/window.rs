//! The settings window: a sidebar of pages and the picked page's sections. This
//! file is the frame: the window's state and constructor, the sidebar, search,
//! and the row helpers more than one page draws with. Each page is a child
//! module; a helper lives with the page that draws it, and moves here once two
//! pages do.

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
use rox_services::catalog::{Library, LibraryEvent, LibraryJob};
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

const TRACKS_COL_W: Pixels = px(56.);
const ALBUMS_COL_W: Pixels = px(56.);
const SIZE_COL_W: Pixels = px(72.);
const ACTION_COL_W: Pixels = px(48.);

/// Slower than the scan badge: a file takes seconds to decode.
const RG_POLL: Duration = Duration::from_millis(250);

struct OpenSettings(WindowHandle<Root>);

impl Global for OpenSettings {}

/// The dock comes in as its own handle because this runs inside a workspace
/// update, where the workspace entity can't be read.
pub fn open(
    state: AppState,
    workspace: WeakEntity<Workspace>,
    workspace_window: AnyWindowHandle,
    dock: Entity<rox_dock::DockArea>,
    cx: &mut App,
) {
    if let Some(open) = cx.try_global::<OpenSettings>() {
        let handle = open.0;
        // Refresh so the window reads a page a caller asked for.
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

/// A-Z by label, with Development pinned last beside the raw file and data
/// folder. Nothing keys off a page's position.
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

enum Pending {
    OverwritePreset(String),
    OverwriteWorkspace(String),
    /// Holds the card the dialog reads out, built once so the bundle isn't
    /// reparsed per frame.
    ApplyWorkspace {
        card: crate::workspaces::ApplyCard,
        /// An import has already saved the bundle, so the dialog offers to
        /// apply it rather than replace.
        imported: bool,
    },
    /// Asked for because it's expensive to undo: getting the vectors back is
    /// the analysis pass over every file.
    ClearEmbeddings(String),
    /// Asked for too: the numbers only come back by decoding every track again.
    /// Clearing is how a better beat estimator reaches numbers already written.
    ClearMeasuredBpm,
    /// Asked for because nothing can rebuild it. Splits its yes: the imported
    /// rows, or the whole table.
    ClearListens,
    /// Asked for because the catalog only comes back by syncing again; the
    /// switch is the non-destructive way to stop using a server.
    RemoveSubsonic(u64),
}

struct SettingsWindow {
    page: Page,
    search: Entity<SearchBox>,
    /// Scopes the search to the open page.
    search_scoped: bool,
    /// A copy of the active theme's palette side; `editor_mode` tracks which.
    base: Palette,
    /// Render re-seeds the copy when the live mode moves off it: a theme
    /// switch, the OS flipping under System, a workspace apply.
    editor_mode: palette::Mode,
    keep_theme: bool,
    surface_opacity: f32,
    backdrop_strength: f32,
    backdrop_all_windows: bool,
    /// The Milkdrop backdrop's sliders. The config itself lives in a live
    /// cache, with the file write debounced behind it.
    backdrop_visual_strength_scrub: ScrubState,
    backdrop_visual_scale_scrub: ScrubState,
    backdrop_visual_duration_scrub: ScrubState,
    backdrop_visual_fps_scrub: ScrubState,
    backdrop_visual_sensitivity_scrub: ScrubState,
    backdrop_visual_fade_scrub: ScrubState,
    backdrop_visual_persist_gen: u64,
    font_size: f32,
    frame: Frame,
    restore_last_track: bool,
    watch_library: bool,
    /// Flipping it reloads the projection so the symbol tables re-intern.
    fold_case: bool,
    split_genre_compounds: bool,
    /// The separator toggle as of the last full scan. While the live value
    /// differs the page shows the rescan note: stored genre lists keep their
    /// old shape until a rescan.
    split_genre_compounds_scanned: bool,
    library_exclude: Vec<String>,
    /// The exclusion list as of the last full scan, for its rescan note.
    library_exclude_scanned: Vec<String>,
    exclude_input: Entity<InputState>,
    exclude_notice: Option<SharedString>,
    /// What the running scan started under. A finished scan makes these the
    /// values the rescan notes compare against; taken at the start, since a
    /// mid-scan change still needs another scan.
    scan_started_with: Option<(bool, Vec<String>)>,
    /// The marker's presence; a flip only takes effect on the next launch.
    portable: bool,
    /// Probed once on open: install dirs are often read-only.
    portable_writable: bool,
    portable_busy: bool,
    rating_style: RatingStyle,
    rating_dots: bool,
    providers: Providers,
    /// Written through per keystroke. Empty on a build with a baked key.
    acoustid_key: Entity<InputState>,
    pickers: Vec<Entity<ColorPickerState>>,
    surface_scrub: ScrubState,
    backdrop_scrub: ScrubState,
    font_size_scrub: ScrubState,
    margin_scrub: SidesScrub,
    padding_scrub: SidesScrub,
    rounding_scrub: ScrubState,
    border_scrub: SidesScrub,
    /// Window state rather than settings, seeded from the knobs whose sides
    /// already differ.
    margin_split: bool,
    padding_split: bool,
    border_split: bool,
    value_edit: panel::ValueEdit,
    scroll: ScrollHandle,
    nav_scroll: ScrollHandle,
    library: Entity<Library>,
    /// Kept whole because the windows this one opens take the bundle.
    state: AppState,
    signals: Arc<rox_viz::signal::SignalHub>,
    /// Weak, so this window never keeps a closed workspace alive.
    workspace: WeakEntity<Workspace>,
    /// For the `Window` an imported layout rebuilds the dock in.
    workspace_window: AnyWindowHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    /// Just the id: the workspace owns the player.
    player: EntityId,
    playback: Entity<Player>,
    crossfade_scrub: ScrubState,
    step_scrub: ScrubState,
    step_preview_scrub: ScrubState,
    /// No working copy: the player owns the number.
    live_buffer_scrub: ScrubState,
    preamp_scrub: ScrubState,
    fallback_scrub: ScrubState,
    /// What was asked for; the readout shows what runs, which differs when a
    /// claim failed.
    output_exclusive: bool,
    /// Listed at open and on rescan, never per frame: enumerating talks to the
    /// sound system.
    output_devices: Vec<output::Device>,
    thumbs: Entity<Thumbs>,
    scrobbler: Entity<Scrobbler>,
    listenbrainz: Entity<ListenBrainz>,
    librefm: Entity<LibreFm>,
    discord: Entity<DiscordPresence>,
    discord_enabled: bool,
    discord_show_lastfm_button: bool,
    discord_show_youtube_button: bool,
    discord_status_line: DiscordStatusLine,
    /// Written through per keystroke, and presence re-reads them on every
    /// write.
    discord_first_line: Entity<InputState>,
    discord_second_line: Entity<InputState>,
    discord_hover_line: Entity<InputState>,
    lastfm_key: Entity<InputState>,
    lastfm_secret: Entity<InputState>,
    /// Not written through: storing a token starts a check, so it waits for
    /// Connect.
    listenbrainz_token: Entity<InputState>,
    /// Written through per keystroke; the sink re-applies on blur or enter,
    /// never mid-type.
    broadcast_host: Entity<InputState>,
    broadcast_port: Entity<InputState>,
    broadcast_mount: Entity<InputState>,
    broadcast_user: Entity<InputState>,
    broadcast_password: Entity<InputState>,
    broadcast_name: Entity<InputState>,
    broadcast_enabled: bool,
    broadcast_bitrate: u32,
    /// An Icecast field changed since the sink last re-applied, so the commit
    /// can run wherever the edit ends and an untouched blur never drops a live
    /// connection.
    broadcast_dirty: bool,
    capture_enabled: bool,
    capture_folder: PathBuf,
    capture_pattern: Entity<InputState>,
    capture_album: Entity<InputState>,
    /// In accounts.json order, so a block's position is its account's index.
    subsonic: Vec<subsonic::SubsonicForm>,
    subsonic_next_id: u64,
    /// One sync runs at a time across all servers.
    subsonic_syncing: bool,
    subsonic_editing: Option<u64>,
    /// Held so the work survives its callback, and a second one replaces the
    /// first rather than racing it.
    subsonic_follow: Option<Task<()>>,
    /// Re-read at open and on every catalog write.
    stations: Vec<Station>,
    station_url: Entity<InputState>,
    station_name: Entity<InputState>,
    /// Also carries the "checking" line while an add probes the stream.
    station_notice: Option<SharedString>,
    /// Holds the Add button: a second press would add the station twice.
    station_probing: bool,
    ffmpeg_path: Entity<InputState>,
    /// Cleared by an edit to the path.
    ffmpeg_test: Option<Result<String, String>>,
    /// CJK text this machine's fonts can only draw through the slow fallback
    /// (see [`crate::cjk_fonts`]). Checked once at open, off the UI thread.
    cjk_fonts_missing: bool,
    threshold_scrub: ScrubState,
    storage: Option<StorageInfo>,
    /// One walk at a time.
    storage_measuring: bool,
    /// Something asked mid-walk, so one more walk follows.
    storage_remeasure: bool,
    root_stats: Vec<(PathBuf, Stats)>,
    /// Counted on library events, never in a paint.
    rg_coverage: GainCoverage,
    rg_job: Option<Arc<replaygain_job::Progress>>,
    layout_name: Entity<InputState>,
    workspace_name: Entity<InputState>,
    workspace_card: Option<workspace_page::CardEditor>,
    /// Read once: pulling an author out parses a bundle. Refreshed by this
    /// page's own writes.
    workspace_authors: BTreeMap<String, String>,
    /// Mirrors the settings file so the badges update without a reload; pushed
    /// to the workspace so its button follows.
    primary_layout: Option<String>,
    mini_layout: Option<String>,
    pending: Option<Pending>,
    /// Keys only reach the focus path, so a dialog wanting Enter and Escape
    /// holds this.
    dialog_focus: FocusHandle,
    /// Not a tab stop. A window with focus nowhere gets no keys at all.
    focus: FocusHandle,
    keymap: BTreeMap<String, Vec<String>>,
    /// The override map before the last reset, for Undo. One level deep,
    /// cleared by any other keymap edit.
    keymap_undo: Option<BTreeMap<String, Vec<String>>>,
    recording: Option<&'static str>,
    check_updates: bool,
    prerelease_updates: bool,
    download_updates: bool,
    ai_enabled: bool,
    mcp_enabled: bool,
    experimental: bool,
    acoustic_analysis: bool,
    acoustic_auto: bool,
    tempo_analysis: bool,
    tempo_auto: bool,
    acoustic_save: AcousticSave,
    prompt: Option<pass_prompt::Prompt>,
    /// Copied from settings so the coverage notes price a pass per render
    /// without reading the file.
    acoustic_workers: usize,
    rg_workers: usize,
    tempo_workers: usize,
    /// Worker-seconds per track by model id. Refreshed when a pass ends.
    acoustic_pace: std::collections::HashMap<String, f32>,
    rg_pace: f32,
    tempo_pace: f32,
    acoustic_coverage: rox_library::embeddings::Coverage,
    acoustic_job: Option<Arc<rox_acoustic::Progress>>,
    bpm_coverage: BpmCoverage,
    tempo_job: Option<Arc<tempo_job::Progress>>,
    /// The extractor the library runs now; the coverage is counted against it.
    acoustic_source: rox_acoustic::Source,
    /// The ML Models page's pick. Differs from `acoustic_source` while the
    /// Library page's switch is on Built-in.
    acoustic_ml_source: rox_acoustic::Source,
    /// View state, not a setting: flipping it doesn't change what the library
    /// runs.
    models_kind: ModelKind,
    /// The error stays on the row that caused it; browsing to the wrong
    /// `.safetensors` is the ordinary case.
    acoustic_local: Option<settings::LocalModel>,
    acoustic_local_error: Option<String>,
    acoustic_local_checking: bool,
    model_job: Option<Arc<rox_acoustic::models::Progress>>,
    /// Measured on entering the page and after a download or delete, never per
    /// frame.
    model_sizes: Vec<(&'static str, u64)>,
    /// Its own field, not an arm of `model_job`, so one Stop button can't
    /// cancel the wrong download.
    dictionary_job: Option<Arc<rox_romanize::dictionary::Progress>>,
    language: Option<String>,
    /// Copied so the Shader page doesn't read the file per render. The enable
    /// switch and compile error aren't copied: the hotkey and hot reload change
    /// them from outside, so those rows read the workspace's live statics.
    post_shader_path: Option<PathBuf>,
    post_shader_name: Option<String>,
    post_shader_source: String,
    /// The apply generation the copies were seeded from. Render re-seeds them
    /// when a workspace apply moves the counter.
    post_shader_gen: u64,
    post_shader_save_name: ShaderNameField,
    post_shader_all_windows: bool,
    post_shader_run_idle: bool,
    /// Edits write here, into the workspace's live feed, and into the file on a
    /// debounce.
    post_shader_routes: Vec<Route>,
    post_shader_route_ui: RouteEditState,
    post_shader_manual: Vec<(u8, f32)>,
    post_shader_slot_scrubs: Vec<panel::ScrubState>,
    /// The config lives in the look's bundle behind a cache, so only per-render
    /// editor state is kept here.
    backdrop_route_ui: RouteEditState,
    backdrop_slot_scrubs: Vec<panel::ScrubState>,
    backdrop_save_name: ShaderNameField,
    backdrop_route_persist_gen: u64,
    backdrop_manual_persist_gen: u64,
    /// Separate debounce generations per write, so one burst never cancels
    /// another's write.
    route_persist_gen: u64,
    manual_persist_gen: u64,
    persist_gen: u64,
    /// Reset clears it: stock persists as an empty map a later write must not
    /// refill.
    persist_palette: bool,
    _picker_changes: Vec<Subscription>,
    _lastfm_changes: Vec<Subscription>,
    _broadcast_changes: Vec<Subscription>,
    _ffmpeg_changed: Subscription,
    _exclude_submit: Subscription,
    _capture_pattern_changed: Subscription,
    _discord_line_changes: Vec<Subscription>,
    _capture_album_changed: Subscription,
    _acoustid_key_changed: Subscription,
    _scrobbler_changed: Subscription,
    _listenbrainz_changed: Subscription,
    _librefm_changed: Subscription,
    _library_changed: Subscription,
    /// Scan progress notifies without emitting Updated.
    _library_repaint: Subscription,
    _library_jobs: Subscription,
    _backdrop_changed: Subscription,
    /// Layout events catch drags; the observe catches an import's set_center,
    /// which notifies without an event.
    _dock_changes: Vec<Subscription>,
    _search_changes: Subscription,
    /// Gated on the output state alone: a playing session notifies sixty times
    /// a second.
    _player_changed: Subscription,
    /// Gated on [`rox_services::player::PlayerView`], so it wakes on a
    /// transport press, not the position clock.
    _player_view: Subscription,
    /// Live for the window's lifetime and gated on `recording`: a subscription
    /// must not be dropped from inside its own callback.
    _record_keys: Subscription,
    /// See [`SettingsWindow::flush_pending_edits`].
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
        let app_state = state.clone();
        let focus = cx.focus_handle();
        window.focus(&focus);
        let playback = state.player;
        let _player_changed = rox_services::player::observe_output(&playback, cx);
        let _player_view = rox_services::player::observe_view(&playback, cx);
        // Off the player, which holds the live copy.
        let output_exclusive = playback.read(cx).exclusive_output();
        let output_devices = output::devices(output_mode(output_exclusive));
        let library = state.library;
        let settings = Settings::load();
        let editor_mode = palette::mode();
        let base = match editor_mode {
            palette::Mode::Dark => settings.palette_dark(),
            palette::Mode::Light => settings.palette_light(),
        };
        let root_stats = seed_root_stats(&library, cx);
        Self::measure_root_stats(&library, cx);
        Self::check_cjk_fonts(&library, cx);
        let rg_coverage = library.read(cx).replaygain_breakdown();
        // A pass started from an earlier settings window may still be running.
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
                this.rg_coverage = library.read(cx).replaygain_breakdown();
                this.bpm_coverage = library.read(cx).bpm_breakdown();
                if this.page == Page::Storage {
                    this.refresh_storage(cx);
                }
                // Stations are rows in the same catalog.
                this.stations = read_stations(&library, cx);
                cx.notify();
            },
        );
        let _library_repaint = cx.observe(&library, |_, _, cx| cx.notify());
        let _library_jobs =
            cx.subscribe(
                &library,
                |this: &mut Self, _, event: &LibraryJob, cx| match event {
                    LibraryJob::ScanStarted => {
                        this.scan_started_with =
                            Some((this.split_genre_compounds, this.library_exclude.clone()));
                    }

                    LibraryJob::ScanFinished => {
                        if let Some((split, exclude)) = this.scan_started_with.take() {
                            this.split_genre_compounds_scanned = split;
                            this.library_exclude_scanned = exclude;
                            cx.notify();
                        }
                    }

                    LibraryJob::WatchSettled => {}
                },
            );
        let _record_keys = Self::record_keys(window, cx);
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // The OS close button never runs our teardown, so save the frame
        // through the should-close hook.
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
        // Release is the one point the OS close button, rox::CloseWindow and a
        // quit all pass through, so the commit runs there.
        let weak = cx.weak_entity();
        let _flush_on_close = cx.on_release({
            let this = weak.clone();
            move |window: &mut Self, cx: &mut App| window.flush_pending_edits(this, cx).detach()
        });
        // `App::shutdown` runs the quit hooks before clearing the windows, so
        // the quit flush happens here.
        let _flush_on_quit = cx.on_app_quit({
            let this = weak.clone();
            move |window: &mut Self, cx: &mut Context<Self>| {
                // The Subsonic prune is dropped, not awaited: gpui parks the
                // main thread on the quit futures and never polls the
                // foreground executor the prune is spawned on, so it would burn
                // the whole 100 ms budget. Measured under the harness. The rows
                // wait for the next Sync Now.
                drop(window.flush_pending_edits(this.clone(), cx));

                async {}
            }
        });
        // Subscribe to the dock handed in: this runs inside the workspace
        // update that opened the window, so the workspace entity can't be read.
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
                // A half-typed port falls back to Icecast's stock one.
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

                    // Leaving the field is the commit; the flush runs the same
                    // one.
                    InputEvent::Blur | InputEvent::PressEnter { .. } => this.broadcast_moved(),

                    InputEvent::Focus => {}
                }
            }));
        }
        // Into accounts.json, not the settings file: a server password is a
        // real credential. Nothing dials on a keystroke.
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

        let station_url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-sources-stations-url-placeholder"))
        });
        let station_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-sources-stations-name-placeholder"))
        });
        let stations = read_stations(&library, cx);

        let exclude_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("settings-library-exclude-placeholder"))
        });
        let _exclude_submit = cx.subscribe_in(
            &exclude_input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_exclusion(window, cx);
                }
            },
        );

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
        // Plus a reload, so the card on someone else's screen follows the
        // typing.
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
        // Nothing re-applies: the service reads the pattern when a song
        // finishes.
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
        // Blank means no album tag.
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
        // Into the working copy too, so a toggle that saves the whole providers
        // struct doesn't put the old key back.
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
        // The first search measures storage, so the Storage rows have numbers
        // without a page visit.
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
                SearchEvent::Dismissed => {
                    window.blur();
                    cx.notify();
                }
                SearchEvent::Submitted | SearchEvent::FocusChanged => {}
            },
        );
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
            split_genre_compounds_scanned: settings.split_genre_compounds,
            library_exclude: settings.library_exclude.clone(),
            library_exclude_scanned: settings.library_exclude.clone(),
            exclude_input,
            exclude_notice: None,
            scan_started_with: None,
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
            cjk_fonts_missing: false,
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
            // Open on the half holding the offered model.
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
            _exclude_submit,
            _capture_pattern_changed,
            _discord_line_changes,
            _capture_album_changed,
            _acoustid_key_changed,
            _scrobbler_changed,
            _listenbrainz_changed,
            _librefm_changed,
            _library_changed,
            _library_repaint,
            _library_jobs,
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

    /// Run the commit every dirty field would have run on its way out. Closing
    /// the window, switching page and quitting all end an edit without a blur.
    ///
    /// Returns the Subsonic prune; the quit hook drops it (see there).
    fn flush_pending_edits(&mut self, this: WeakEntity<Self>, cx: &mut App) -> Task<()> {
        self.broadcast_moved();

        self.subsonic_commit(this, cx).unwrap_or(Task::ready(()))
    }

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

    /// A window-wide search ends here; a page-bound one carries over, since the
    /// sidebar is how it picks its page.
    fn open_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        // The broadcast and Subsonic rows leave the screen, so whatever was
        // typed commits now.
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

    /// The request is a nav key, since the panels are a crate below this one
    /// and can't name [`Page`].
    fn sync_requested_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = rox_panel_api::panel_settings::requested_app_page(cx) else {
            return;
        };

        let Some(&(page, ..)) = PAGES.iter().find(|&&(_, label, _)| label == key) else {
            return;
        };

        self.open_page(page, window, cx);
    }

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

    /// Inside the box because the sidebar is 160px wide.
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

const TREE_ROW_GROUP: &str = "tree-row";

/// The closed lock skips this in `panel_row`: it shows state worth seeing at
/// rest.
fn reveal<E: Styled + InteractiveElement>(control: E) -> E {
    control
        .opacity(0.)
        .group_hover(TREE_ROW_GROUP, |style| style.opacity(1.))
}

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

fn coverage_note(text: String) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(text)
}

fn readout(value: String) -> Div {
    div().text_color(palette::text_muted()).child(value)
}

fn human_size(bytes: u64) -> String {
    rox_core::fmt::fmt_bytes(bytes)
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = grid_columns(window);

        // The Appearance swatches still edit the base palette underneath the
        // tint.
        let player = self.player;
        palette::note_focus(player, window.is_window_active(), cx);

        // Follow a theme switch; every switch path repaints all windows.
        self.sync_editor_side(window, cx);

        // The zoom shortcuts step the font size from outside.
        self.sync_font_size();

        // A workspace apply replaces the shader config from outside; the route
        // sync below runs over what this brings in.
        self.sync_post_shader();

        // Read at render so a window that was already up jumps too.
        self.sync_requested_page(window, cx);

        // Here rather than in the page: the Shader page builds from `&self`,
        // and search builds every page per keystroke.
        self.post_shader_route_ui
            .sync(self.post_shader_routes.len());

        let text = self.search.read(cx).query().trim().to_string();
        let q = Query::parse(&text);
        let scoped = self.search_scoped;
        // The AI toggle hides the MCP and ML Models pages from search too.
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
        // Built out here: the button listens on this window's context.
        let scope = self.scope_button(cx).into_any_element();
        let search = self
            .search
            .update(cx, |search, cx| search.element_with_suffix(Some(scope), cx));

        panel::window_body(player, || {
            let sidebar = sidebar()
                .child(
                    div()
                        // Only while the box holds focus, so an outside click
                        // never blurs another input mid-edit.
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
                                // Development breaks off from the pages and reads back a
                                // shade.
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
                // L too: it's Focus Search in a workspace window.
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
                .children(self.backdrop.layer(&self.now_art, window, cx))
                .child(sidebar)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .relative()
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
                        // The absolute wrapper gives the scrollbar its bounds.
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&self.scroll)),
                        ),
                )
                // Under the confirm, so Remove Server's question lands on top.
                .children(self.subsonic_dialog(window, cx))
                .children(self.confirm_overlay(window, cx))
                // Shares the confirm's layer; nothing raises both.
                .children(pass_prompt::overlay(self, window, cx))
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{PAGES, Page};
    use rox_design::assets::icons;

    /// Exhaustive, so a new variant doesn't compile until it's named here.
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

    #[test]
    fn the_nav_sorts_alphabetically_with_development_last() {
        let (last, sorted) = PAGES.split_last().expect("pages");
        assert_eq!(last.1, "settings-page-development");
        let labels: Vec<String> = sorted.iter().map(|&(_, l, _)| l.to_lowercase()).collect();
        let mut want = labels.clone();
        want.sort();
        assert_eq!(labels, want, "the sidebar is out of alphabetical order");
    }

    /// The panels dispatch this action by name from two crates below, so the
    /// string is held to the real one here.
    #[test]
    fn the_panels_name_the_action_that_opens_this_window() {
        use gpui::Action as _;

        assert_eq!(
            crate::workspace::OpenSettings.name(),
            rox_panel_api::panel_settings::SETTINGS_ACTION
        );
    }

    #[test]
    fn the_shader_page_wears_the_blend_icon() {
        let entry = PAGES
            .iter()
            .find(|&&(_, label, _)| label == "settings-page-shader")
            .expect("a Shader page");
        assert_eq!(entry.2, icons::BLEND);
    }
}
