//! The settings window: one OS window opened from the menubar, a sidebar
//! of pages on the left and the picked page's sections on the right.
//! Appearance holds the song-theming switch, ADR 10's transparency pair,
//! and the palette editor, a labeled swatch grid per listing group;
//! Library manages the scanned folders over the shared catalog entity.
//! Edits land live through the palette setters and persist to the
//! settings file per change, the volume slider's cadence. The window
//! edits a working copy of the user palette, so the swatches show the
//! base even while a playing track's seed tints the app over it; while
//! song theming is on the editor locks, because the track is driving.
//! Palettes import and export as the settings map's role-to-hex JSON,
//! so a file, the settings entry, and a shared theme are one shape.
//! Layout mirrors the opening workspace's dock tree - every split, tab
//! group, and panel - with each panel's settings a click away, and
//! moves whole compositions in and out as the layout dump's JSON.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, size, svg, AnyElement, AnyWindowHandle, App, Axis, Bounds, Context, Div,
    Entity, EntityId, Global, Hsla, MouseButton, MouseDownEvent, PathPromptOptions, Pixels,
    ScrollHandle, SharedString, Subscription, WeakEntity, Window, WindowHandle,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Root, Sizable as _};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::palette::{self, Palette, Role, ROLES};
use crate::design::tokens;
use crate::integrations::discord::DiscordPresence;
use crate::integrations::tray;
use crate::lastfm::{self, AuthPhase, Scrobbler};
use crate::panel::{self, AppState, ScrubState};
use crate::panel_settings;
use crate::panels::library::{Library, LibraryEvent};
use crate::player::Player;
use crate::providers;
use crate::query::search::{SearchBox, SearchEvent};
use crate::replaygain_job;
use crate::settings::layouts::Preset;
use crate::settings::ui::{
    self as settings_ui, grid_columns, icon_button, sidebar, small_button, PageBody, Query,
    Section, SECTION_GAP,
};
use crate::settings::{
    self, data_dir, settings_path, Frame, GainModeSetting, LayoutSize, LyricsSave, NamedLayout,
    Providers, RatingStyle, ReplayGainSave, Settings, Theme, WorkspaceBundle, BORDER_MAX,
    MARGIN_MAX, PADDING_MAX, ROUNDING_MAX,
};
use crate::thumbs::Thumbs;
use crate::workspace::Workspace;
use rox_dock::{DockAreaState, DockEvent, PanelView, StackPanel, TabPanel};
use rox_library::store::{GainCoverage, Stats};
use rox_playback::engine;
use rox_playback::output;

/// The folder table's fixed columns: the rollup numbers and the remove
/// control, the last sized to [`icon_button`]'s footprint so the header
/// aligns.
mod workspace_page;

const TRACKS_COL_W: Pixels = px(56.);
const ALBUMS_COL_W: Pixels = px(56.);
const SIZE_COL_W: Pixels = px(72.);
const ACTION_COL_W: Pixels = px(22.);

/// The rates the exclusive picker offers: the two base clocks and their
/// doubles and quadruples, which is every rate consumer hardware actually
/// runs. A card that hasn't got one lands on its nearest and reports that.
const RATES: &[u32] = &[44100, 48000, 88200, 96000, 176400, 192000];

/// The periods the buffer picker offers, in milliseconds, either side of the
/// backend's 10 ms default.
const PERIODS_MS: &[f64] = &[2.5, 5.0, 10.0, 20.0, 40.0];

/// How often the Leveling section samples a running measurement pass. Slower
/// than the scan badge on purpose: a file takes seconds to decode, so there
/// is nothing to see at 100 ms.
const RG_POLL: Duration = Duration::from_millis(250);

/// The open settings window, if any: opening again focuses it instead
/// of stacking a second editor over the same file.
struct OpenSettings(WindowHandle<Root>);

impl Global for OpenSettings {}

/// Open the settings window, or bring the open one to the front. The
/// state carries the library for the Library page, which edits it live,
/// and the shared art bake for the window's own backdrop. The workspace
/// and its window handle are the Layout page's subject: the tree walks
/// its dock, and an imported layout rebuilds in its window. The dock
/// rides along as its own handle because open runs inside a workspace
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
    let handle = crate::panel::open_child_window(
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
    Behavior,
    Audio,
    Workspace,
    Library,
    Providers,
    Integrations,
    Storage,
    Development,
}

const PAGES: &[(Page, &str, &str)] = &[
    (Page::Appearance, "Appearance", icons::PALETTE),
    (Page::Behavior, "Behavior", icons::SLIDERS),
    (Page::Audio, "Audio", icons::AUDIO_LINES),
    (Page::Workspace, "Workspace", icons::APP_WINDOW),
    (Page::Library, "Library", icons::LIST_MUSIC),
    (Page::Providers, "Providers", icons::DOWNLOAD),
    (Page::Integrations, "Integrations", icons::RADIO),
    (Page::Storage, "Storage", icons::DATABASE),
    (Page::Development, "Development", icons::FLASK),
];

/// The storage page's measurements, taken entering the page and after a
/// clear rather than per frame: the stats and the cache walk are cheap
/// once, not every paint.
#[derive(Clone, Copy, Default)]
struct StorageInfo {
    /// The whole library's rollup: tracks, albums, bytes of music.
    music: Stats,
    /// library.db with its WAL sidecars.
    catalog: u64,
    /// thumbs.db with its WAL sidecars.
    thumbs: u64,
    /// Everything under waveforms/.
    waveforms: u64,
    /// Everything in the lyrics store (lyrics/).
    lyrics: u64,
    /// The log file and its rolled back file (logs/).
    logs: u64,
}

/// A confirm dialog waiting on the user: each variant names what a yes does,
/// all of them destructive enough to ask before acting. None means no dialog.
enum Pending {
    /// Replace a saved preset's dump with the live layout.
    OverwritePreset(String),
    /// Replace a saved workspace with the current state.
    OverwriteWorkspace(String),
    /// Replace the whole live look with a workspace bundle's.
    ApplyWorkspace(String),
}

struct SettingsWindow {
    page: Page,
    /// The sidebar's search box: a non-empty query swaps the page area
    /// for the all-pages results stack, every page filtered through
    /// [`Query`] under its own breadcrumb.
    search: Entity<SearchBox>,
    /// The working copy of the user palette: what the swatches show and
    /// what edits write through [`palette::set`]. Mirrors the active
    /// theme's side; `editor_mode` tracks which.
    base: Palette,
    /// The theme side the working copy mirrors. Render re-seeds the copy
    /// and the pickers when the live mode moves off it: a theme switch
    /// here, the OS flipping under System, a workspace apply.
    editor_mode: palette::Mode,
    keep_theme: bool,
    surface_opacity: f32,
    backdrop_strength: f32,
    /// The app font size's working copy: what the Typography slider shows
    /// and writes through [`palette::set_app_font_size`].
    font_size: f32,
    /// The app-wide frame defaults' working copy: what the Frame sliders
    /// show and write through [`settings::set_app_frame`].
    frame: Frame,
    restore_last_track: bool,
    /// Whether the library watches its folders for changes, the Folders page
    /// toggle. Mirrors the setting; flipping it arms or drops the watcher on
    /// the shared library.
    watch_library: bool,
    /// Whether values differing only by case merge, the Folders page's
    /// case toggle. Mirrors the setting; flipping it reloads the
    /// projection so the symbol tables re-intern under the new rule.
    fold_case: bool,
    /// Whether commas and slashes split genre lists, the Folders page's
    /// separator toggle. Mirrors the setting; flipping it reloads the
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
    /// way, so a flip only lands on the next launch.
    portable: bool,
    /// Whether the executable's folder takes writes, probed once on
    /// open: install dirs are often read-only, and the toggle reads
    /// inert there.
    portable_writable: bool,
    /// A portable seed copy is running; the toggle sits out until it
    /// lands.
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
    margin_scrub: ScrubState,
    padding_scrub: ScrubState,
    rounding_scrub: ScrubState,
    border_scrub: ScrubState,
    /// The one readout being typed into across this window's sliders.
    value_edit: panel::ValueEdit,
    /// The page body's scroll position, shared with the scrollbar so it
    /// can show how much page hangs below the fold.
    scroll: ScrollHandle,
    /// The shared catalog, the Library page's subject.
    library: Entity<Library>,
    /// The workspace that opened this window, the Layout page's subject:
    /// the tree walks its dock and imports rebuild it. Weak, so the
    /// settings window never keeps a closed workspace alive.
    workspace: WeakEntity<Workspace>,
    /// The workspace's OS window, for reaching its `Window` when an
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
    /// Whether exclusive output is asked for, the Output toggle. What's
    /// actually running is the readout under it, and the two disagree
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
    /// The api credential inputs; edits mirror into the scrobbler per
    /// keystroke, the pickers' cadence.
    lastfm_key: Entity<InputState>,
    lastfm_secret: Entity<InputState>,
    threshold_scrub: ScrubState,
    /// The storage page's numbers; None until the page is first opened.
    storage: Option<StorageInfo>,
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
    /// Whether launch runs the daily update check, the Behavior page toggle.
    check_updates: bool,
    /// Whether the experimental panels show in the panel menus, the
    /// Development page toggle.
    experimental: bool,
    /// The active icon pack, mirrored from settings so the Appearance page's
    /// pack list marks the current one without re-reading the settings file
    /// (which carries the dock dumps) on every render.
    active_icon_pack: Option<String>,
    /// The pack folders as last listed, so the Icons section doesn't walk
    /// the directory on every Appearance render; create, switch, and
    /// delete refresh it.
    icon_packs: Vec<String>,
    /// Bumped on every appearance-slider tick; a debounced writer flushes the
    /// current values once the scrub settles instead of rewriting the whole
    /// settings file per tick.
    persist_gen: u64,
    /// Whether the debounced appearance write carries the palette map too.
    /// Picker edits set it; reset clears it, since stock persists as an
    /// empty map that a later write must not refill with explicit defaults.
    persist_palette: bool,
    _picker_changes: Vec<Subscription>,
    _lastfm_changes: Vec<Subscription>,
    /// The connect flow's phases land through here, so the page's status
    /// line follows along.
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
        let playback = state.player;
        let _player_changed = crate::player::observe_output(&playback, cx);
        // Off the player rather than the file: it holds the live copy, and a
        // toggle flipped here has to agree with the session it rebuilds.
        let output_exclusive = playback.read(cx).exclusive_output();
        let output_devices = output::devices(output_mode(output_exclusive));
        let library = state.library;
        let settings = Settings::load();
        let editor_mode = palette::mode();
        let base = match editor_mode {
            palette::Mode::Dark => settings.palette_dark(),
            palette::Mode::Light => settings.palette_light(),
        };
        let root_stats = library.read(cx).root_stats();
        let rg_coverage = library.read(cx).replaygain_breakdown();
        // A pass started from an earlier settings window may still be
        // running; pick it up rather than showing the button as idle.
        let rg_job = replaygain_job::progress(cx);
        if rg_job.is_some() {
            Self::poll_measuring(cx);
        }
        let _library_changed = cx.subscribe(
            &library,
            |this: &mut Self, library, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.root_stats = library.read(cx).root_stats();
                // A scan and a finished measurement pass both fill the
                // ReplayGain columns in, so the Audio page's coverage line
                // moves with either.
                this.rg_coverage = library.read(cx).replaygain_breakdown();
                // A finished scan moves the storage numbers too; remeasure
                // if they are on screen.
                if this.page == Page::Storage {
                    this.refresh_storage(cx);
                }
                cx.notify();
            },
        );
        let _library_repaint = cx.observe(&library, |_, _, cx| cx.notify());
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
                .placeholder("API key")
                .default_value(settings.accounts.lastfm.api_key.clone())
        });
        let lastfm_secret = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Shared secret")
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
        // The search box up top: typing filters every page at once. The
        // first search measures storage so the Storage rows have numbers
        // without a page visit; after that the numbers ride until the
        // page's own refresh paths run.
        let search = cx.new(|cx| SearchBox::new("Search", "", window, cx).small().icon());
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
            font_size: settings.app_font_size,
            frame: settings.look.bundle.appearance.frame,
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
            margin_scrub: ScrubState::default(),
            padding_scrub: ScrubState::default(),
            rounding_scrub: ScrubState::default(),
            border_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            scroll: ScrollHandle::new(),
            library,
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
            threshold_scrub: ScrubState::default(),
            storage: None,
            root_stats,
            rg_coverage,
            rg_job,
            layout_name: cx.new(|cx| InputState::new(window, cx).placeholder("Layout name")),
            workspace_name: cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name")),
            pack_name: cx.new(|cx| InputState::new(window, cx).placeholder("Pack name")),
            primary_layout: settings.look.bundle.primary_layout.clone(),
            mini_layout: settings.look.bundle.mini_layout.clone(),
            pending: None,
            check_updates: settings.check_updates,
            experimental: settings.experimental,
            active_icon_pack: settings.icon_pack.clone(),
            icon_packs: crate::startup::icon_packs::all(),
            persist_gen: 0,
            persist_palette: false,
            _picker_changes,
            _lastfm_changes,
            _scrobbler_changed,
            _library_changed,
            _library_repaint,
            _backdrop_changed,
            _dock_changes,
            _search_changes,
            _player_changed,
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
        // The palette is live above; the file write rides the debounce, since
        // a picker drag fires a change per tick like the sliders do.
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

    /// The restore switch: straight into the file. Launch reads it there,
    /// so the flip is live for the next start without touching playback.
    fn set_restore_last_track(&mut self, on: bool, cx: &mut Context<Self>) {
        self.restore_last_track = on;
        Settings::update(move |s| s.restore_last_track = on);
        cx.notify();
    }

    /// The watch-folders switch: flip the mirror and hand it to the shared
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
    /// seeds it from the current data folder when it is new, and drops
    /// the marker file launch checks for; off removes the marker and
    /// leaves rox-data where it is - going back doesn't migrate, that
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
        // Seed rox-data from the live data folder off the UI thread - the
        // caches can be big - and only drop the marker once the copy
        // lands, so a restart mid-copy never boots on a half folder. The
        // copy is best-effort over live databases, the same risk copying
        // the folder by hand takes; the restart requirement is what keeps
        // the window small.
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

    /// The menubar switch, the Window menu toggle's twin: through the
    /// live static so every workspace window drops or regrows its bar,
    /// and into the file. The toggle reads the static, not a cached
    /// field, so the two entry points never show different states.
    fn set_hide_menubar(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_hide_menubar(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.hide_menubar = on);
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
        let (surface, backdrop, frame, font_size) = (
            self.surface_opacity,
            self.backdrop_strength,
            self.frame,
            self.font_size,
        );
        let palette = self
            .persist_palette
            .then(|| (self.editor_mode, self.base.to_map()));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            // A later tick bumped the gen past this capture, so only the last
            // edit in a burst writes. The palette rereads at fire time so an
            // immediate writer (reset, import) landing inside the wait isn't
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
    // panel that sets no override of its own takes.

    fn set_margin(&mut self, value: f32, cx: &mut Context<Self>) {
        self.frame.margin = value;
        self.frame_edited(cx);
    }

    fn set_padding(&mut self, value: f32, cx: &mut Context<Self>) {
        self.frame.padding = value;
        self.frame_edited(cx);
    }

    fn set_rounding(&mut self, value: f32, cx: &mut Context<Self>) {
        self.frame.rounding = value;
        self.frame_edited(cx);
    }

    fn set_border(&mut self, value: f32, cx: &mut Context<Self>) {
        self.frame.border = value;
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
    /// setters take what lands.
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
    /// persists like any other edit, so the flip survives a restart.
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
    /// they hold. What a track dressed the app in leaves as a fixed theme.
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
            .section(Section::new(q, icons::MENU, "Interface", None, |rows| {
                rows.keyed(
                    &["menu bar", "toolbar", "alt"],
                    "Hide Menubar",
                    Some("Keep the menubar hidden, floating it over the dock while alt is held"),
                    panel::toggle(settings::hide_menubar(), Self::set_hide_menubar, cx),
                )
                .keyed(
                    &["title bar", "chrome", "frameless"],
                    "OS Decorations",
                    Some("The OS titlebar and borders on the main windows; off leans on the window controls and drag anchor panels"),
                    panel::toggle(settings::os_decorations(), Self::set_os_decorations, cx),
                )
            }))
            .section(Section::new(q, icons::CONTRAST, "Theming", None, |rows| {
                rows.keyed(
                    &["dark", "light", "mode", "appearance"],
                    "Theme",
                    Some("The palette the app renders and the one the color editor below targets; System follows the OS's light or dark preference"),
                    panel::choices(
                        &[
                            ("Dark", Theme::Dark),
                            ("Light", Theme::Light),
                            ("System", Theme::System),
                        ],
                        settings::theme(),
                        Self::set_theme,
                        cx,
                    ),
                )
                .keyed(
                    &["album art", "tint", "accent"],
                    "Song Theming",
                    Some("Tint the palette and back windows with the playing track's cover art"),
                    panel::toggle(palette::art_theming(), Self::set_art_theming, cx),
                )
                .keyed(
                    &["dark", "light", "lock", "pin"],
                    "Keep Theme",
                    Some("Hold the active theme even when a cover's brightness would flip it; song theming still tints the color"),
                    panel::toggle(self.keep_theme, Self::set_keep_theme, cx),
                )
            }))
            .section(Section::new(q, icons::ALIGN_LEFT, "Typography", None, |rows| {
                rows.keyed(
                    &["typeface", "family", "text"],
                    "Font",
                    Some("The app-wide typeface; panels can override it in their own settings"),
                    panel::font_picker(
                        "app-font",
                        settings::app_font().map(|font| font.to_string()),
                        Self::set_app_font,
                        cx,
                    ),
                )
                .keyed(
                    &["text size", "scale", "zoom"],
                    "Font Size",
                    Some("The base text size every panel's text scales from; controls and icons hold their size"),
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
            }))
            .section(self.icons_section(q, cx))
            .section(Section::new(q, icons::EYE, "Transparency", None, |rows| {
                rows.keyed(
                    &["transparency", "translucent", "blur"],
                    "Surface Opacity",
                    Some("How opaque the app's surfaces read over the backdrop"),
                    settings_ui::slider_edit(
                        &self.surface_scrub,
                        &self.value_edit,
                        self.surface_opacity,
                        Self::set_surface,
                        cx,
                    ),
                )
                .keyed(
                    &["transparency", "opacity", "blur", "wallpaper"],
                    "Backdrop Strength",
                    Some("How strongly the cover backdrop shows behind them"),
                    settings_ui::slider_edit(
                        &self.backdrop_scrub,
                        &self.value_edit,
                        self.backdrop_strength,
                        Self::set_backdrop,
                        cx,
                    ),
                )
            }))
            .section(Section::new(q, icons::SQUARE_DASHED, "Frame", None, |rows| {
                rows.keyed(
                    &["spacing", "gap", "outside"],
                    "Margin",
                    Some("Pull every panel in from its cell; a panel can override this in its own settings"),
                    self.frame_row(&self.margin_scrub, self.frame.margin, MARGIN_MAX, Self::set_margin, cx),
                )
                .keyed(
                    &["spacing", "inset", "inside"],
                    "Padding",
                    Some("Space inside every panel's edge, kept in its own background"),
                    self.frame_row(&self.padding_scrub, self.frame.padding, PADDING_MAX, Self::set_padding, cx),
                )
                .keyed(
                    &["corner radius", "rounded"],
                    "Rounding",
                    Some("Round every panel's corners off into the backdrop"),
                    self.frame_row(&self.rounding_scrub, self.frame.rounding, ROUNDING_MAX, Self::set_rounding, cx),
                )
                .keyed(
                    &["outline", "stroke", "edge"],
                    "Border",
                    Some("A line around every panel's edge, in the Border role's color"),
                    self.frame_row(&self.border_scrub, self.frame.border, BORDER_MAX, Self::set_border, cx),
                )
                .keyed(
                    &["divider", "gutter", "grid lines"],
                    "Panel Seams",
                    Some("The hairline between panel tiles; off leaves the resize grips invisible but still draggable"),
                    panel::toggle(settings::seams(), Self::set_seams, cx),
                )
            }))
            .section(self.colors_section(q, columns, cx))
    }

    /// The Icons section: the built-in set and every pack the user has as a
    /// list, each a set to switch to; the current one carries an Active
    /// badge. Creating a new pack, seeded with the built-in icons for an
    /// author to edit, rides the header.
    fn icons_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let active = self.active_icon_pack.clone();
        let packs = self.icon_packs.clone();

        // New-pack-from-name rides the header, so a pack is one name away
        // and lands pre-filled with the current icons.
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(Input::new(&self.pack_name).small().w(px(150.)))
            .child(small_button(
                "New Pack",
                icons::FOLDER_PLUS,
                false,
                cx.listener(|this, _, window, cx| this.create_pack(window, cx)),
            ));

        Section::new(
            q,
            icons::IMAGE,
            "Icons",
            Some(controls.into_any_element()),
            |rows| {
                rows.custom(&["icon pack", "svg", "glyphs", "built-in"], || {
                    let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
                        div().text_xs().text_color(palette::text_muted()).child(
                            "A pack is a folder of SVGs that replaces the built-in icons; \
                         switching takes effect on the next launch",
                        ),
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
    /// carries Open Folder, to edit its SVGs, and Delete.
    fn icon_pack_row(
        &self,
        name: Option<String>,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label: SharedString = name
            .clone()
            .map(SharedString::from)
            .unwrap_or_else(|| "Built-in".into());
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .child(div().flex_1().min_w_0().truncate().child(label))
            .map(|d| {
                if active {
                    d.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child("Active"),
                    )
                } else {
                    d.child(small_button("Use", icons::CHECK, false, {
                        let name = name.clone();
                        cx.listener(move |this, _, _, cx| this.set_icon_pack(name.clone(), cx))
                    }))
                }
            })
            .when_some(name, |d, name| {
                // Open Folder reveals the pack so its SVGs can be edited in
                // place; delete drops the folder and everything in it.
                d.child(small_button("Open Folder", icons::FOLDER, false, {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.reveal_pack(&name, cx))
                }))
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
    /// set so the resolver never points at a folder that is gone.
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
                "Playback",
                None,
                |rows| {
                    rows.keyed(
                        &["play", "pause", "skip", "preview"],
                        "Transport",
                        Some(
                            "Start and stop without leaving this page, since every setting \
                         below is judged by ear",
                        ),
                        panel::transport_strip(&self.playback, cx),
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
                "Equalizer",
                Some(
                    small_button(
                        "Open Equalizer",
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
                            .child(
                                "Ten octave bands over the output. It opens in its own window, \
                                 since it's worked while the music plays rather than set once",
                            )
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
            "Crossfade",
            Some(
                "How long a track overlaps the one after it. Shuffle and skipping are what \
                 the fade is for, so an album's own boundaries stay untouched unless the \
                 row below says otherwise. Zero turns it off",
            ),
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
            "Fade Inside Albums",
            Some(
                "Overlap tracks that belong to the same record as well. Off keeps a \
                 record's own splices exactly as they were mastered, which is where \
                 gapless matters most",
            ),
            control,
        )
    }

    /// The ReplayGain section: which of a file's two gains to level by, the
    /// two offsets around it, and where the measurement pass puts what it
    /// measures. The offsets only show once a mode is picked, since with
    /// leveling off there is nothing for them to offset.
    fn replay_gain_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        const MODES: &[(&str, GainModeSetting)] = &[
            ("Off", GainModeSetting::Off),
            ("Track", GainModeSetting::Track),
            ("Album", GainModeSetting::Album),
        ];
        let rg = self.playback.read(cx).replay_gain();
        // A running pass owns the line under the section: its count, the
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
                 Missing analyzes them and saves what it measures"
            ))
        } else if split.missing > 0 {
            Some(format!(
                "{} of {total} scanned tracks have a gain to level by, {} of them measured \
                 by rox. The other {} play at the untagged setting",
                split.covered(),
                split.measured,
                split.missing,
            ))
        } else if split.measured > 0 {
            Some(format!(
                "All {total} scanned tracks have a gain to level by, {} of them measured by \
                 rox",
                split.measured,
            ))
        } else {
            Some(format!("All {total} scanned tracks carry ReplayGain tags"))
        };
        Section::new(
            q,
            icons::GAUGE,
            "ReplayGain",
            Some(self.measure_control(cx)),
            |rows| {
                let rows = rows
                .keyed(
                    &["volume", "normalization", "loudness", "leveling"],
                    "Level By",
                    Some(
                        "Play every track at the loudness its ReplayGain tags measured, so a \
                         shuffle stops jumping between masters. Track levels each file on its \
                         own; Album uses the record's gain across all its tracks, which keeps \
                         an album's own quiet and loud passages where they were put",
                    ),
                    panel::choices(
                        MODES,
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
                        &["volume", "gain", "boost", "loudness"],
                        "Preamp",
                        Some(
                            "Added to every tagged gain. ReplayGain's reference sits below where \
                             modern records are cut, so a levelled library plays quieter than the \
                             same library raw; this is where that comes back. A boost never \
                             clips: the tagged peak caps it",
                        ),
                        settings_ui::scalar(
                            &self.preamp_scrub,
                            &self.value_edit,
                            rg.preamp_db,
                            settings_ui::span(-15., 15., " dB").decimals(1).hard(),
                            |this: &mut Self, db, cx| {
                                this.playback
                                    .update(cx, |player, cx| player.set_replay_gain_preamp(db, cx));
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                    .keyed(
                        &["fallback", "default gain", "missing"],
                        "Untagged Files",
                        Some(
                            "What a file with no ReplayGain tags plays at. Nothing measured it, \
                             so this is a guess standing in for one - leave it at zero and \
                             untagged tracks play as they always did",
                        ),
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
                    &["write", "tags", "database", "analysis"],
                    "Save Measured Gains",
                    Some(
                        "Where the measurement pass puts its numbers. The library database keeps \
                         your files untouched; tags put the same values where every other player \
                         reads them, at the cost of rewriting the audio files",
                    ),
                    panel::choices(
                        &[
                            ("Database", ReplayGainSave::Database),
                            ("Tags", ReplayGainSave::Tags),
                        ],
                        rg.save,
                        Self::set_replay_gain_save,
                        cx,
                    ),
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

    /// The section header's control: start the pass, or stop the one that's
    /// running. Inert with nothing missing, and while the library is busy
    /// scanning, since a scan is rewriting the very rows the pass reads.
    fn measure_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = &self.rg_job {
            let stopping = job.stopping();
            return small_button(
                if stopping { "Stopping..." } else { "Stop" },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| replaygain_job::stop(cx)),
            )
            .into_any_element();
        }
        let idle = self.rg_coverage.missing == 0 || self.library.read(cx).busy().is_some();
        small_button(
            "Measure Missing",
            icons::GAUGE,
            idle,
            cx.listener(|this, _, _, cx| this.start_measuring(cx)),
        )
        .into_any_element()
    }

    /// Kick off the measurement pass and start sampling it.
    fn start_measuring(&mut self, cx: &mut Context<Self>) {
        replaygain_job::start(self.library.clone(), cx);
        self.rg_job = replaygain_job::progress(cx);
        Self::poll_measuring(cx);
        cx.notify();
    }

    /// Mirror the running pass into the section, the scan badge's cadence.
    /// Stops itself once the pass clears the global.
    fn poll_measuring(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(RG_POLL).await;
            let live = this.update(cx, |this, cx| {
                this.rg_job = replaygain_job::progress(cx);
                cx.notify();
                this.rg_job.is_some()
            });
            if !matches!(live, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// The running pass as one line: how far along, what it's on, and what
    /// it gave up on. The work list is built first, so a zero total means
    /// the pass is still deciding what to measure.
    fn measure_progress_line(job: &replaygain_job::Progress) -> String {
        let total = job.total();
        if total == 0 {
            return "Measuring: working out what's missing...".into();
        }
        let mut line = format!("Measuring {} of {total}", job.done().min(total));
        let current = job.current();
        if let Some(name) = Path::new(&current).file_name() {
            line.push_str(&format!(" - {}", name.to_string_lossy()));
        }
        let failed = job.failed();
        if failed > 0 {
            line.push_str(&format!(" ({failed} skipped)"));
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

    /// The badge and its report button ride the Output header rather than the
    /// Exclusive Mode row: they're about the whole backend, not the switch,
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
                        .child("Experimental")
                        .tooltip(|_, cx| {
                            cx.new(|_| {
                                ExperimentalTooltip(
                                    "This platform's exclusive backend is written from the \
                                     platform's documented audio contract but has never been \
                                     run on real hardware by the developers. It should claim \
                                     the device or fall back to shared with a reason, never \
                                     go silent. If it misbehaves, turn it off and report \
                                     what happened with the button beside this badge."
                                        .into(),
                                )
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
                                ExperimentalTooltip(
                                    "Report how exclusive mode behaved on this machine. \
                                     Opens a GitHub issue with the platform and the \
                                     negotiated stream filled in."
                                        .into(),
                                )
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
            readout("Not built for this platform yet".into()).into_any_element()
        };
        Section::new(
            q,
            icons::VOLUME_2,
            "Output",
            self.exclusive_notice(cx),
            |rows| {
                rows.keyed(
                    &["bit perfect", "wasapi", "asio", "hog"],
                    "Exclusive Mode",
                    Some(
                        "Claim the device for rox alone and run it at the file's own rate where \
                     the hardware takes one; off shares the system mixer with everything \
                     else on the desktop",
                    ),
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

    /// The rate the device runs at: following each file's own is what makes
    /// a mixed-rate library play without a resampler anywhere, so it leads.
    fn output_rate_row(&self, cx: &mut Context<Self>) -> Div {
        let mut options: Vec<(Option<u32>, SharedString)> = vec![(None, "Follow the file".into())];
        options.extend(
            RATES
                .iter()
                .map(|hz| (Some(*hz), format!("{:.1} kHz", *hz as f32 / 1000.0).into())),
        );
        panel::setting_row(
            "Sample Rate",
            Some(
                "Following reopens the device at each file's own rate, which costs a gap \
                 at a boundary where the rate changes; pinning one rate never pays that \
                 and resamples anything that doesn't match",
            ),
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
    /// the pick exists for a card whose driver is happier on one of them.
    fn output_format_row(&self, cx: &mut Context<Self>) -> Div {
        let options: Vec<(Option<String>, SharedString)> = vec![
            (None, "Widest available".into()),
            (Some("f32".into()), "32-bit float".into()),
            (Some("s32".into()), "32-bit integer".into()),
            (Some("s16".into()), "16-bit integer".into()),
        ];
        panel::setting_row(
            "Format",
            Some(
                "What rox hands the card. A card that won't take the pick runs the widest \
                 it has and says so in the status below",
            ),
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

    /// The period, which is the latency trade stated as what it is.
    fn output_period_row(&self, cx: &mut Context<Self>) -> Div {
        let mut options: Vec<(Option<f64>, SharedString)> = vec![(None, "Default (10 ms)".into())];
        options.extend(
            PERIODS_MS
                .iter()
                .map(|ms| (Some(*ms), format!("{ms} ms").into())),
        );
        panel::setting_row(
            "Buffer",
            Some(
                "How much audio the card holds at a time. Shorter reacts quicker and \
                 crackles sooner on a busy machine; longer is safer and lazier",
            ),
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
    /// head so switching back is one pick. Rescan sits beside it because the
    /// list is taken when the window opens: plugging an interface in while
    /// it's up shouldn't mean closing and reopening.
    fn output_devices_block(&self, cx: &mut Context<Self>) -> Div {
        let mut options: Vec<(Option<String>, SharedString)> =
            vec![(None, "System Default".into())];
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
            "The system default follows whatever the desktop is set to"
        } else if cfg!(target_os = "linux") {
            "Exclusive claims a card straight from the kernel, so the list is sound \
             cards rather than the desktop's outputs. Bluetooth and other \
             sound-server devices have no card to claim and only show with \
             exclusive off"
        } else {
            "Exclusive takes the device for rox alone, so nothing else on the \
             desktop can sound through it until the mode is off"
        };
        panel::setting_row(
            "Device",
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
                    "Rescan",
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
                    "No output",
                    vec![error, "Pick another device, or turn exclusive off".into()],
                ),
                None => panel::banner(
                    panel::Tone::Info,
                    "Nothing playing",
                    vec!["Start a track and this says what the device agreed to".into()],
                ),
            };
        };
        let negotiated = &status.negotiated;
        let mode = match negotiated.mode {
            output::Mode::Exclusive => "Exclusive",
            output::Mode::Shared => "Shared",
        };
        let resampling = status
            .source_rate
            .is_some_and(|source| source != negotiated.sample_rate);
        // The tone is the whole point of the callout, and the two bad cases
        // aren't the same size. A claim that failed is a setting that didn't
        // take, which is an error: exclusive is switched on and you are not
        // hearing it. Resampling is the mode working and still not being
        // bit-perfect, which is worth flagging without crying wolf.
        let tone = if negotiated.fallback.is_some() {
            panel::Tone::Bad
        } else if resampling {
            panel::Tone::Warn
        } else {
            panel::Tone::Good
        };
        // The experimental note rides the banner too: someone reading only
        // the status line should know the mode they're hearing is the one
        // nobody has hardware-tested.
        let experimental =
            negotiated.mode == output::Mode::Exclusive && Self::exclusive_experimental();
        let headline = format!(
            "{mode}{} on {}, {} Hz, {} ch, {}",
            if experimental { " (experimental)" } else { "" },
            negotiated.device,
            negotiated.sample_rate,
            negotiated.channels,
            negotiated.format
        );
        let mut lines = Vec::new();
        // The fallback line is the whole reason a failed claim isn't a
        // mystery: the toggle stays on, and this says why it isn't what
        // you're hearing.
        if let Some(why) = &negotiated.fallback {
            lines.push(format!("Exclusive fell back to shared: {why}").into());
        }
        // Leveling multiplies the source on its way to the ring (ADR 19),
        // so it goes above the rate line: whatever the rates say, this is
        // the one that decides whether these are the file's own samples.
        // Only when something is actually applied, so an untagged file with
        // the fallback at zero says nothing.
        if let Some(db) = status.leveling_db {
            lines.push(format!("ReplayGain is levelling this file by {db:+.1} dB").into());
        }
        if let Some(source) = status.source_rate {
            lines.push(
                if resampling {
                    format!("The playing file is {source} Hz, resampled to reach the device")
                } else {
                    format!("The playing file is {source} Hz, so nothing is resampling it")
                }
                .into(),
            );
        }
        panel::banner(tone, headline, lines)
    }

    /// Ask for exclusive output, or give the device back. The player
    /// rebuilds its running session onto the other backend right here, so
    /// the switch lands without a restart, and the device list is the other
    /// backend's from this point.
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

    fn behavior_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        // The portable row's control by where the toggle stands: inert
        // text where the exe folder refuses writes or while the seed
        // copy runs, the live switch otherwise.
        let portable_control: AnyElement = if !self.portable_writable {
            readout("The app's folder is not writable".into()).into_any_element()
        } else if self.portable_busy {
            readout("Copying data...".into()).into_any_element()
        } else {
            panel::toggle(self.portable, Self::set_portable, cx).into_any_element()
        };
        let mut portable_row =
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
                .child(panel::setting_row(
                    "Portable Mode",
                    Some(
                        "Keep settings, library, and caches in a rox-data folder beside \
                     the executable, so the player travels with its data; turning it \
                     off goes back to the system folder and leaves rox-data in place",
                    ),
                    portable_control,
                ));
        // The restart note keys on the marker disagreeing with the run,
        // not on a flip this session: it stays up across window reopens
        // until a launch actually lands the change.
        if self.portable != settings::portable() && !self.portable_busy {
            portable_row = portable_row.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child("Applies on the next launch; this run stays on its current folder"),
            );
        }
        PageBody::new()
            .section(Section::new(q, icons::PLAY, "Startup", None, |rows| {
                rows.keyed(
                    &["resume", "reopen"],
                    "Restore Last Track",
                    Some("Launch with the last playing track loaded, paused where it left off"),
                    panel::toggle(self.restore_last_track, Self::set_restore_last_track, cx),
                )
                .keyed(
                    &["release", "version", "upgrade"],
                    "Check for Updates",
                    Some("Look for a newer release once a day when rox starts; the About window checks now either way"),
                    panel::toggle(self.check_updates, Self::set_check_updates, cx),
                )
            }))
            // No tray backend on Windows yet, and a resident process with no
            // way back in is worse than quitting, so the row sits out there.
            .when(tray::supported(), |page| {
                page.section(Section::new(q, icons::APP_WINDOW, "Window", None, |rows| {
                    rows.keyed(
                        &["quit", "minimize", "background"],
                        "Remain in Tray",
                        Some(
                            "Keep the music playing when the last window closes, with the \
                             tray icon (the dock on macOS) as the way back in",
                        ),
                        panel::toggle(settings::quit_to_tray(), Self::set_quit_to_tray, cx),
                    )
                }))
            })
            .section(Section::new(q, icons::DATABASE, "Data", None, |rows| {
                rows.custom(&["portable mode", "usb", "folder", "executable"], || {
                    portable_row.into_any_element()
                })
            }))
            .section(Section::new(q, icons::STAR, "Ratings", None, |rows| {
                rows.keyed(
                    &["stars", "numeric"],
                    "Rating Scale",
                    Some("Stars for quick clicks, 0-10 in half steps for finer review scores"),
                    panel::choices(
                        &[
                            ("Stars", RatingStyle::Stars),
                            ("0-10", RatingStyle::Numeric),
                        ],
                        self.rating_style,
                        Self::set_rating_style,
                        cx,
                    ),
                )
                .keyed(
                    &["stars", "empty"],
                    "Unrated Dots",
                    Some("Mark unfilled star slots with a faint dot instead of leaving them empty"),
                    panel::toggle(self.rating_dots, Self::set_rating_dots, cx),
                )
            }))
    }

    /// The Integrations page: Last.fm account & scrobbling settings,
    /// and Discord Rich Presence knobs.
    fn integrations_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let scrobbler = self.scrobbler.read(cx);
        let config = scrobbler.config().clone();
        let phase = scrobbler.phase().clone();
        let connected = !config.session_key.is_empty();
        // A build with its own api identity connects in one click; only
        // one without asks for the user's pair.
        let builtin = lastfm::has_builtin_keys();
        let keys_ready = builtin || (!config.api_key.is_empty() && !config.api_secret.is_empty());

        // The connect strip: where the connection stands, and the one
        // action that moves it along.
        let status: SharedString = if connected {
            format!("Connected as {}", config.username).into()
        } else {
            match &phase {
                AuthPhase::Idle => "Not connected".into(),
                AuthPhase::Requesting => "Requesting a token...".into(),
                AuthPhase::Waiting(_) => {
                    "Authorize rox in the browser, then finish connecting".into()
                }
                AuthPhase::Confirming => "Confirming...".into(),
                AuthPhase::Failed(e) => format!("Connection failed: {e}").into(),
            }
        };
        let action = if connected {
            small_button(
                "Disconnect",
                icons::CLOSE,
                false,
                cx.listener(|this, _, _, cx| {
                    this.scrobbler.update(cx, |s, cx| s.disconnect(cx));
                }),
            )
        } else {
            match phase {
                AuthPhase::Requesting | AuthPhase::Confirming => {
                    small_button("Working...", icons::REFRESH_CW, true, |_, _, _| {})
                }
                AuthPhase::Waiting(_) => small_button(
                    "Finish Connecting",
                    icons::REFRESH_CW,
                    false,
                    cx.listener(|this, _, _, cx| {
                        this.scrobbler.update(cx, |s, cx| s.finish_auth(cx));
                    }),
                ),
                _ => small_button(
                    "Connect",
                    icons::EXTERNAL_LINK,
                    !keys_ready,
                    cx.listener(|this, _, _, cx| {
                        this.scrobbler.update(cx, |s, cx| s.begin_auth(cx));
                    }),
                ),
            }
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
                        "Connect your last.fm account: authorize rox in the browser \
                     and played tracks scrobble to it"
                    } else {
                        "This build ships no api identity, so scrobbling needs your own \
                     api account (last.fm/api/account/create); paste its key and \
                     shared secret, then connect"
                    }),
            )
            .when(!builtin, |d| {
                d.child(panel::setting_row(
                    "API Key",
                    None,
                    Input::new(&self.lastfm_key).w(px(240.)),
                ))
                .child(panel::setting_row(
                    "Shared Secret",
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
            .section(Section::new(q, icons::RADIO, "Last.fm", None, |rows| {
                rows.custom(
                    &["account", "connect", "login", "api key", "scrobble"],
                    || account.into_any_element(),
                )
            }))
            .section(Section::new(q, icons::UPLOAD, "Scrobbling", None, |rows| {
                rows.keyed(
                    &["listens", "history"],
                    "Scrobble Tracks",
                    Some("Send played tracks to last.fm once they cross the threshold"),
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
                    &["last.fm", "percent"],
                    "Scrobble Threshold",
                    Some(
                        "How much of a track has to play before it scrobbles; \
                         the seek strip and waveform can mark it",
                    ),
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
            }))
            .section(Section::new(
                q,
                icons::GLOBE,
                "Discord Rich Presence",
                None,
                |rows| {
                    rows.keyed(
                        &["status", "now playing"],
                        "Enable Rich Presence",
                        Some("Show rox activity on Discord when playing music"),
                        panel::toggle(self.discord_enabled, Self::set_discord_enabled, cx),
                    )
                    .keyed(
                        &["link", "profile"],
                        "Show Last.fm Button",
                        Some("Include a clickable 'View on Last.fm' button in Discord status"),
                        panel::toggle(
                            self.discord_show_lastfm_button,
                            Self::set_discord_show_lastfm_button,
                            cx,
                        ),
                    )
                    .keyed(
                        &["link", "video"],
                        "Show YouTube Button",
                        Some("Include a clickable 'Search on YouTube' button in Discord status"),
                        panel::toggle(
                            self.discord_show_youtube_button,
                            Self::set_discord_show_youtube_button,
                            cx,
                        ),
                    )
                },
            ))
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
            .section(Section::new(q, icons::MIC, "Lyrics", None, |rows| {
                rows.custom(&["online", "network", "offline", "privacy"], || {
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(
                            "Online lookups run only when a panel action asks for one; \
                             playback and browsing never touch the network",
                        )
                        .into_any_element()
                })
                .keyed(
                    &["online", "fetch"],
                    "LRCLIB",
                    Some("Fetch missing lyrics from lrclib.net, synced sheets when it has them"),
                    panel::toggle(self.providers.lrclib, Self::set_lrclib, cx),
                )
                .keyed(
                    &["sidecar", "store"],
                    "Save Fetched Lyrics",
                    Some(
                        "Where a fetched sheet lands: rox's own data folder keeping the \
                         library clean, an .lrc next to the track, or the embedded tag",
                    ),
                    panel::choices(
                        &[
                            ("Data Folder", LyricsSave::Store),
                            ("Sidecar", LyricsSave::Sidecar),
                            ("Tag", LyricsSave::Tag),
                        ],
                        self.providers.lyrics_save,
                        Self::set_lyrics_save,
                        cx,
                    ),
                )
            }))
            .section(Section::new(q, icons::TAG, "Metadata", None, |rows| {
                rows.keyed(
                    &["lookup", "online"],
                    "MusicBrainz",
                    Some(
                        "Look up tags on musicbrainz.org; the metadata panel's search \
                         shows matches to confirm field by field before writing",
                    ),
                    panel::toggle(self.providers.musicbrainz, Self::set_musicbrainz, cx),
                )
            }))
            .section(Section::new(q, icons::DISC, "Cover Art", None, |rows| {
                rows.keyed(
                    &["artwork", "covers", "album art"],
                    "iTunes",
                    Some("Search iTunes for cover art; the cover editor's search shows matches to pick before setting"),
                    panel::toggle(self.providers.itunes, Self::set_itunes, cx),
                )
                .keyed(
                    &["artwork", "covers", "album art"],
                    "Deezer",
                    Some("Search Deezer for cover art, up to 1000 pixels"),
                    panel::toggle(self.providers.deezer, Self::set_deezer, cx),
                )
                .keyed(
                    &["artwork", "covers", "album art"],
                    "Last.fm",
                    Some("Search Last.fm for cover art"),
                    panel::toggle(self.providers.lastfm_art, Self::set_lastfm_art, cx),
                )
            }))
            .section(Section::new(q, icons::USER, "Artist", None, |rows| {
                rows.row(
                    "Last.fm",
                    Some(
                        "Fetch artist biographies, stats, and similar artists for the \
                         biography panel, with a portrait from Deezer; everything is \
                         kept in the data folder and reads offline afterwards",
                    ),
                    panel::toggle(self.providers.artist, Self::set_artist, cx),
                )
            }))
    }

    /// One cell of the color grid: the picker with its label beside it,
    /// or a dimmed inert swatch while song theming drives the palette.
    /// The inert swatch shows the derived color the track landed on, the
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
            palette::Mode::Dark => "Inverse From Light Theme",
            palette::Mode::Light => "Inverse From Dark Theme",
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
                "Apply Song Theme",
                icons::DISC,
                !locked,
                cx.listener(|this, _, window, cx| this.apply_song_theme(window, cx)),
            ))
            .child(small_button(
                "Import",
                icons::DOWNLOAD,
                locked,
                cx.listener(|this, _, window, cx| this.import_palette(window, cx)),
            ))
            .child(small_button(
                "Export",
                icons::UPLOAD,
                false,
                cx.listener(|this, _, _, cx| this.export_palette(cx)),
            ))
            .child(small_button(
                "Reset",
                icons::REFRESH_CW,
                locked,
                cx.listener(|this, _, window, cx| this.reset_palette(window, cx)),
            ));
        Section::new(
            q,
            icons::PALETTE,
            "Colors",
            Some(controls.into_any_element()),
            |rows| {
                rows.custom(
                    &["palette", "accent", "swatch", "role", "import", "export"],
                    || {
                        let mut body = div().flex().flex_col().gap(tokens::SPACE_XS);
                        if locked {
                            body =
                                body
                                    .child(div().text_xs().text_color(palette::text_muted()).child(
                                    "Song theming is on, so the playing track drives these colors \
                             and export saves them; turn it off above to edit them",
                                ));
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
    fn folder_row(&self, root: &Path, stats: Stats, scanning: bool, cx: &mut Context<Self>) -> Div {
        let path: SharedString = root.to_string_lossy().into_owned().into();
        let remove = icon_button(icons::CLOSE, scanning, {
            let root = root.to_path_buf();
            cx.listener(move |this, _, _, cx| {
                this.library
                    .update(cx, |library, cx| library.remove_root(&root, cx));
            })
        });
        div()
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
        // Past the ceiling the watch turns itself off, so the toggle grays
        // out at off and the note says why, with the numbers. Folders summed
        // off the cached rollups, not a per-frame count; the roots never
        // nest, so nothing counts twice. Matched to the catalog's own limit,
        // which is None where the platform prices watching flat.
        let dirs = self.root_stats.iter().map(|(_, s)| s.dirs).sum::<u64>();
        let over_limit = crate::catalog::watch_limit_dirs().filter(|limit| dirs > *limit);
        let lead_in = div().text_xs().text_color(palette::text_muted()).child(
            "Folders scanned into the library; removing one drops its \
             tracks from the catalog and leaves the files alone",
        );
        // The rescan nudge, only while the separator rule has moved
        // this session: filtering and the genre wall follow the flip
        // right away, but genre lists earlier scans wrote into the
        // database keep their old shape until a rescan re-reads the
        // tags.
        let separators_moved = self.split_genre_compounds != self.split_genre_compounds_at_open;
        let nudge = div().text_xs().text_color(palette::text_muted()).child(
            "Separators changed: browsing follows right away. Genre \
             lists stored by earlier scans keep their old shape \
             until you hit Rescan up in the Folders header",
        );
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
                .child(div().flex_1().child("Folder"))
                .child(
                    div()
                        .w(TRACKS_COL_W)
                        .flex_none()
                        .text_right()
                        .child("Tracks"),
                )
                .child(
                    div()
                        .w(ALBUMS_COL_W)
                        .flex_none()
                        .text_right()
                        .child("Albums"),
                )
                .child(div().w(SIZE_COL_W).flex_none().text_right().child("Size"))
                .child(div().w(ACTION_COL_W).flex_none()),
        );
        if self.root_stats.is_empty() {
            table = table.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child("No folders yet"),
            );
        }
        for (root, stats) in &self.root_stats {
            table = table.child(self.folder_row(root, *stats, scanning, cx));
        }
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
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(note),
                )
            });

        // Add folder and rescan ride the section header like the colors
        // controls do.
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                "Add Folder",
                icons::FOLDER_PLUS,
                scanning,
                cx.listener(|this, _, _, cx| {
                    this.library.update(cx, |library, cx| library.browse(cx));
                }),
            ))
            .child(small_button(
                "Rescan",
                icons::REFRESH_CW,
                scanning || self.root_stats.is_empty(),
                cx.listener(|this, _, _, cx| {
                    this.library.update(cx, |library, cx| library.rescan(cx));
                }),
            ))
            // The tag repair window: find and rewrite files carrying the
            // broken ID3v2.4 tag shape lofty reads mangled, where a user
            // lands after seeing garbled tags.
            .child(small_button(
                "Repair Tags...",
                icons::FILE_TEXT,
                scanning,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    let now_art = this.now_art.clone();
                    crate::tags::repair::open(library, now_art, cx);
                }),
            ))
            // The duplicates window: find tracks the library carries more
            // than once and move the spare copies to the trash.
            .child(small_button(
                "Duplicates...",
                icons::COPY,
                scanning,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    let thumbs = this.thumbs.clone();
                    let now_art = this.now_art.clone();
                    crate::duplicates::open(library, thumbs, now_art, cx);
                }),
            ));
        // The lead-in describes the table, so both carry the same terms
        // and a search never turns up one without the other.
        let folders = ["scan", "rescan", "music", "add", "remove"];
        PageBody::new().section(Section::new(
            q,
            icons::FOLDER,
            "Folders",
            Some(controls.into_any_element()),
            |rows| {
                let rows = rows.custom(&folders, || lead_in.into_any_element());
                let rows = match over_limit {
                    Some(limit) => rows.row_dyn(
                        &["monitor", "auto", "rescan", "folder"],
                        "Watch folders",
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
                        &["monitor", "auto", "live"],
                        "Watch folders",
                        Some(
                            "Fold added, edited, and deleted files into the library as \
                             they happen, without a manual rescan",
                        ),
                        panel::toggle(self.watch_library, Self::set_watch_library, cx),
                    ),
                };
                rows.keyed(
                    &["fold", "duplicates", "capitalization"],
                    "Merge case variants",
                    Some(
                        "Treat values differing only by case as one - Rock and \
                         rock become the same genre, artist, and album, shown \
                         under the casing most tracks carry. Files keep their \
                         tags as written",
                    ),
                    panel::toggle(self.fold_case, Self::set_fold_case, cx),
                )
                .keyed(
                    &["separator", "multi-genre"],
                    "Split genres on commas and slashes",
                    Some(
                        "\"Dubstep, Trap\" and \"Drum & Bass / Neurofunk\" count \
                         each value as its own genre; semicolons always split. \
                         Off keeps slashed names whole for tags where they mean \
                         one genre. Files keep their tags as written",
                    ),
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
    }

    /// Measure everything the storage page shows: the library rollup on
    /// the UI-side connection, the databases and the waveform cache by
    /// stat. Cheap enough to run whole on page entry, too heavy per frame.
    fn refresh_storage(&mut self, cx: &mut Context<Self>) {
        let data = data_dir();
        self.storage = Some(StorageInfo {
            music: self.library.read(cx).stats(),
            catalog: db_size(&data.join("library.db")),
            thumbs: db_size(&data.join("thumbs.db")),
            waveforms: dir_size(&crate::peaks::cache_dir()),
            lyrics: dir_size(&settings::lyrics_dir()),
            logs: dir_size(&data.join("logs")),
        });
        cx.notify();
    }

    /// Empty the thumbnail store. The delete runs off the UI thread on
    /// the service's own connection, so it serializes against in-flight
    /// loads; the sizes refresh when it lands.
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
                .spawn(async move { crate::peaks::clear() })
                .await;
            this.update(cx, |this, cx| this.refresh_storage(cx)).ok();
        })
        .detach();
    }

    fn storage_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let info = self.storage.unwrap_or_default();
        let music = format!(
            "{} tracks, {} albums, {}",
            info.music.tracks,
            info.music.albums,
            human_size(info.music.bytes)
        );
        PageBody::new()
            .section(Section::new(q, icons::DATABASE, "Library", None, |rows| {
                rows.keyed(
                    &["size", "disk", "space"],
                    "Music Files",
                    Some("What the scanned folders hold; the files stay where they are"),
                    readout(music),
                )
                .keyed(
                    &["database", "size", "disk"],
                    "Catalog",
                    Some("The track index scans build (library.db)"),
                    readout(human_size(info.catalog)),
                )
                .keyed(
                    &["size", "disk"],
                    "Lyrics",
                    Some("Fetched and edited sheets kept in the app's own store (lyrics/), so library folders stay clean"),
                    readout(human_size(info.lyrics)),
                )
            }))
            .section(Section::new(q, icons::LAYERS, "Caches", None, |rows| {
                rows.keyed(
                    &["cache", "clear", "artwork", "size"],
                    "Cover Thumbnails",
                    Some("Small covers kept after their first render (thumbs.db); cleared ones rebuild as they scroll into view"),
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .child(readout(human_size(info.thumbs)))
                        .child(small_button(
                            "Clear",
                            icons::TRASH,
                            false,
                            cx.listener(|this, _, _, cx| this.clear_thumbs(cx)),
                        )),
                )
                .keyed(
                    &["cache", "clear", "peaks", "size"],
                    "Waveforms",
                    Some("Each track's peak strip, kept after its first play; cleared ones re-decode next play"),
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .child(readout(human_size(info.waveforms)))
                        .child(small_button(
                            "Clear",
                            icons::TRASH,
                            false,
                            cx.listener(|this, _, _, cx| this.clear_waveforms(cx)),
                        )),
                )
            }))
            .section(Section::new(q, icons::FILE_TEXT, "Diagnostics", None, |rows| {
                rows.keyed(
                    &["debug", "reveal", "diagnostics"],
                    "Logs",
                    Some("What each run writes for bug reports (logs/rox.log), rolled at a size cap so it never grows large"),
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .child(readout(human_size(info.logs)))
                        .child(small_button(
                            "Reveal",
                            icons::FILE_TEXT,
                            false,
                            cx.listener(|_, _, _, cx| {
                                cx.reveal_path(&crate::logging::log_path());
                            }),
                        )),
                )
            }))
    }

    /// The launch-check toggle: into the file, so the next start reads the
    /// new setting. This run is already past its launch check either way.
    fn set_check_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.check_updates = on;
        Settings::update(move |s| s.check_updates = on);
        cx.notify();
    }

    fn set_experimental(&mut self, on: bool, cx: &mut Context<Self>) {
        self.experimental = on;
        Settings::update(move |s| s.experimental = on);
        settings::set_experimental(on, cx);
        cx.notify();
    }

    /// The Development page: the switches for work that isn't finished.
    /// One row today; the section is the place the next one lands.
    fn development_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new().section(Section::new(q, icons::FLASK, "Features", None, |rows| {
            rows.keyed(
                &["debug", "beta", "unfinished"],
                "Experimental Panels",
                Some(
                    "Show the panels still being built in the Panels menu and the \
                         launcher; they change shape between releases, and a layout that \
                         already holds one keeps it when this goes back off",
                ),
                panel::toggle(self.experimental, Self::set_experimental, cx),
            )
        }))
    }

    /// Land on a page and leave search: what a sidebar click and a
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
        &self,
        page: Page,
        q: &Query,
        columns: usize,
        cx: &mut Context<Self>,
    ) -> PageBody {
        match page {
            Page::Appearance => self.appearance_page(q, columns, cx),
            Page::Behavior => self.behavior_page(q, cx),
            Page::Audio => self.audio_page(q, cx),
            Page::Workspace => self.workspace_page(q, cx),
            Page::Library => self.library_page(q, cx),
            Page::Providers => self.providers_page(q, cx),
            Page::Integrations => self.integrations_page(q, cx),
            Page::Storage => self.storage_page(q, cx),
            Page::Development => self.development_page(q, cx),
        }
    }

    /// The results stack: every surviving page under a heading that
    /// jumps to it, in sidebar order, so a search reads as the settings
    /// laid flat. The heading centers over rules running to both edges,
    /// a level above the section headers underneath it. `pages` is what
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
                .child(format!("Nothing matches \"{text}\""))
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

    /// A sidebar footer row: hands something to the system - the raw
    /// settings file, the data folder - so it reads quieter than the
    /// pages above.
    fn sidebar_action(
        &self,
        label: &'static str,
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

/// Where a layout tree node sits among its siblings, for the reorder
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
/// carries state worth seeing without a hover.
fn reveal(control: Div) -> Div {
    control
        .opacity(0.)
        .group_hover(TREE_ROW_GROUP, |style| style.opacity(1.))
}

/// A structure line of the layout tree: a split or tab group, muted so
/// the panels carry the page, with the move controls riding the right
/// edge when the node can move. Padded to the icon buttons' height so
/// the tree keeps one rhythm with and without controls.
fn chrome_row(depth: usize, label: &'static str, controls: Option<AnyElement>) -> AnyElement {
    div()
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

/// A role badge on a preset row: lit like a filled control when the preset
/// holds the role, a plain chip otherwise. Clicking toggles the role.
/// The badge a shipped layout or workspace carries in its list row, telling
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
        .child("Built-in")
}

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

/// A confirm-dialog button: the primary one reads as a filled accent
/// control, the rest as plain controls.
fn dialog_button(
    label: &'static str,
    primary: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .map(|d| {
            if primary {
                d.bg(palette::accent())
                    .text_color(palette::text_on_accent())
                    .hover(|d| d.opacity(0.9))
            } else {
                d.bg(palette::bg_control())
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

/// What the library actually carries, under the section whose setting
/// depends on it. Quiet: it's context for the rows above, not a warning.
fn coverage_note(text: String) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(text)
}

/// A setting row's value where a control would sit.
fn readout(value: String) -> Div {
    div().text_color(palette::text_muted()).child(value)
}

/// The exclusive toggle as the output layer's mode. The two device lists
/// don't share ids, so which one to ask for follows the toggle rather than
/// what happens to be running.
/// The hover note behind the Experimental badge and its issue button. Same
/// card the track info chip's tooltip wears, so the explanation reads the
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

fn output_mode(exclusive: bool) -> output::Mode {
    if exclusive {
        output::Mode::Exclusive
    } else {
        output::Mode::Shared
    }
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
        "B" => format!("{bytes} B"),
        "KB" => format!("{} KB", value.round()),
        _ => format!("{value:.1} {unit}"),
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

/// Every file directly under one folder; the waveform cache is flat.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = grid_columns(window);

        // The window renders under the workspace player's art tint and
        // claims the widget theme while it holds focus, so the pages sit
        // in the same colors as the app they configure. The Appearance
        // page's swatches still edit the base palette underneath; the
        // locked swatches show the derived colors through `resolved`.
        let player = self.player;
        palette::note_focus(player, window.is_window_active(), cx);

        // A theme switch lands the live palette on the other side; the
        // editor follows it here since every switch path repaints all
        // windows.
        self.sync_editor_side(window, cx);

        // A live query builds every page and stacks the survivors; the
        // sidebar dims the pages that kept nothing. No query builds just
        // the picked page through the same path, with the inactive query
        // keeping everything.
        let text = self.search.read(cx).query().trim().to_string();
        let q = Query::parse(&text);
        let results: Option<Vec<_>> = q.active().then(|| {
            PAGES
                .iter()
                .map(|&(page, label, icon)| {
                    (page, label, icon, self.build_page(page, &q, columns, cx))
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
                .children(
                    PAGES
                        .iter()
                        .enumerate()
                        .map(|(index, &(page, label, icon))| {
                            let empty = results
                                .as_ref()
                                .is_some_and(|results| results[index].3.hits() == 0);
                            settings_ui::nav_item(
                                label,
                                icon,
                                self.page == page,
                                move |this: &mut Self, window, cx| {
                                    this.open_page(page, window, cx);
                                },
                                cx,
                            )
                            .when(empty, |d| d.opacity(0.4))
                        }),
                )
                // The escape hatches sink to the bottom: the raw file this
                // window edits and the folder it lives in.
                .child(div().flex_1())
                .child(self.sidebar_action("Settings File", icons::FILE_TEXT, settings_path, cx))
                .child(self.sidebar_action("Data Folder", icons::FOLDER, data_dir, cx));

            let page = match results {
                Some(results) => self.search_results(&text, results, cx),
                None => self.build_page(self.page, &q, columns, cx).element(),
            };

            div()
                .size_full()
                // The settings shortcut everywhere: focus lands in the
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
                        // The page's own surface, the window base the sidebar
                        // sits beside: opaque at full surface opacity so the
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
                .children(self.confirm_overlay(cx))
                .into_any_element()
        })
    }
}
