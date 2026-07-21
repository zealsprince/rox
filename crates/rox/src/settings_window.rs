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

use gpui::{
    div, prelude::*, px, size, svg, AnyElement, AnyWindowHandle, App, Axis, Bounds, Context, Div,
    Entity, Global, Hsla, MouseButton, MouseDownEvent, PathPromptOptions, Pixels, ScrollHandle,
    SharedString, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowHandle,
    WindowOptions,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::{Root, Sizable as _};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::palette::{self, Palette, Role, ROLES};
use crate::design::tokens;
use crate::lastfm::{self, AuthPhase, Scrobbler};
use crate::layouts::Preset;
use crate::panel::{self, AppState, ScrubState};
use crate::panel_settings;
use crate::panels::library::{Library, LibraryEvent};
use crate::providers;
use crate::settings::{
    self, data_dir, settings_path, LayoutSize, LyricsSave, NamedLayout, Providers, RatingStyle,
    Settings, WorkspaceBundle,
};
use crate::settings_ui::{
    self, grid_columns, icon_button, section, sidebar, small_button, SECTION_GAP,
};
use crate::thumbs::Thumbs;
use crate::tray;
use crate::workspace::Workspace;
use rox_dock::{DockAreaState, DockEvent, PanelView, StackPanel, TabPanel};
use rox_library::store::Stats;

/// The folder table's fixed columns: the rollup numbers and the remove
/// control, the last sized to [`icon_button`]'s footprint so the header
/// aligns.
const TRACKS_COL_W: Pixels = px(56.);
const ALBUMS_COL_W: Pixels = px(56.);
const SIZE_COL_W: Pixels = px(72.);
const ACTION_COL_W: Pixels = px(22.);

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
        .settings_window
        .filter(|s| s.width >= f32::from(min.width) && s.height >= f32::from(min.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((720., 520.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(settings_ui::MIN_SIZE),
        titlebar: Some(TitlebarOptions {
            title: Some("rox - Settings".into()),
            ..Default::default()
        }),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            // The Wayland backend ignores the creation-time titlebar
            // title; only set_window_title reaches the compositor.
            window.set_window_title("rox - Settings");
            let view = cx.new(|cx| {
                SettingsWindow::new(state, workspace, workspace_window, dock, window, cx)
            });
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the settings window");
    cx.set_global(OpenSettings(handle));
}

/// The sidebar's pages.
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Appearance,
    Behavior,
    Workspace,
    Library,
    Providers,
    Scrobbling,
    Storage,
}

const PAGES: &[(Page, &str, &str)] = &[
    (Page::Appearance, "Appearance", icons::PALETTE),
    (Page::Behavior, "Behavior", icons::SLIDERS),
    (Page::Workspace, "Workspace", icons::APP_WINDOW),
    (Page::Library, "Library", icons::LIST_MUSIC),
    (Page::Providers, "Providers", icons::DOWNLOAD),
    (Page::Scrobbling, "Scrobbling", icons::RADIO),
    (Page::Storage, "Storage", icons::DATABASE),
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
    /// The working copy of the user palette: what the swatches show and
    /// what edits write through [`palette::set`].
    base: Palette,
    art_theming: bool,
    keep_dark: bool,
    surface_opacity: f32,
    backdrop_strength: f32,
    restore_last_track: bool,
    quit_to_tray: bool,
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
    /// The Providers page's working copy of the enrichment config.
    providers: Providers,
    /// One picker per palette role, in [`ROLES`] order.
    pickers: Vec<Entity<ColorPickerState>>,
    surface_scrub: ScrubState,
    backdrop_scrub: ScrubState,
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
    /// The shared thumbnail service, whose durable store the storage
    /// page sizes and clears.
    thumbs: Entity<Thumbs>,
    /// The workspace's scrobbler, the Scrobbling page's subject: the api
    /// credential edits, the connect flow, and the knobs all go through
    /// it, and it persists them.
    scrobbler: Entity<Scrobbler>,
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
        let library = state.library;
        let settings = Settings::load();
        let base = settings.palette();
        let root_stats = library.read(cx).root_stats();
        let _library_changed = cx.subscribe(
            &library,
            |this: &mut Self, library, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.root_stats = library.read(cx).root_stats();
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
                s.settings_window = Some(LayoutSize {
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
                .default_value(settings.lastfm.api_key.clone())
        });
        let lastfm_secret = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Shared secret")
                .masked(true)
                .default_value(settings.lastfm.api_secret.clone())
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
            base,
            art_theming: settings.art_theming,
            keep_dark: settings.keep_dark,
            surface_opacity: settings.surface_opacity,
            backdrop_strength: settings.backdrop_strength,
            restore_last_track: settings.restore_last_track,
            quit_to_tray: settings.quit_to_tray,
            portable: settings::portable_marker().is_some_and(|marker| marker.exists()),
            portable_writable: settings::portable_available(),
            portable_busy: false,
            rating_style: settings.rating_style,
            providers: settings.providers.clone(),
            pickers,
            surface_scrub: ScrubState::default(),
            backdrop_scrub: ScrubState::default(),
            scroll: ScrollHandle::new(),
            library,
            workspace,
            workspace_window,
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            thumbs: state.thumbs,
            scrobbler,
            lastfm_key,
            lastfm_secret,
            threshold_scrub: ScrubState::default(),
            storage: None,
            root_stats,
            layout_name: cx.new(|cx| InputState::new(window, cx).placeholder("Layout name")),
            workspace_name: cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name")),
            pack_name: cx.new(|cx| InputState::new(window, cx).placeholder("Pack name")),
            primary_layout: settings.primary_layout.clone(),
            mini_layout: settings.mini_layout.clone(),
            pending: None,
            _picker_changes,
            _lastfm_changes,
            _scrobbler_changed,
            _library_changed,
            _library_repaint,
            _backdrop_changed,
            _dock_changes,
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
                let default = (role.get)(&Palette::default());
                (role.set)(&mut self.base, default);
                picker.update(cx, |picker, cx| picker.set_value(default, window, cx));
            }
        }
        palette::set(self.base, cx);
        let map = self.base.to_map();
        Settings::update(move |s| s.palette = map);
    }

    /// The song-theming switch: through the palette pipe, which also
    /// gates the backdrop layers, and into the file.
    fn set_art_theming(&mut self, on: bool, cx: &mut Context<Self>) {
        self.art_theming = on;
        palette::set_art_theming(on, cx);
        Settings::update(move |s| s.art_theming = on);
        cx.notify();
    }

    /// The keep-dark switch: holds the dark ladder under a bright cover.
    /// Through the palette pipe so open windows ease over, and into the file.
    fn set_keep_dark(&mut self, on: bool, cx: &mut Context<Self>) {
        self.keep_dark = on;
        palette::set_keep_dark(on, cx);
        Settings::update(move |s| s.keep_dark = on);
        cx.notify();
    }

    /// The restore switch: straight into the file. Launch reads it there,
    /// so the flip is live for the next start without touching playback.
    fn set_restore_last_track(&mut self, on: bool, cx: &mut Context<Self>) {
        self.restore_last_track = on;
        Settings::update(move |s| s.restore_last_track = on);
        cx.notify();
    }

    /// The quit-to-tray switch: flips the live flag the close path reads,
    /// persists, and puts the tray icon up or takes it down on the spot.
    fn set_quit_to_tray(&mut self, on: bool, cx: &mut Context<Self>) {
        self.quit_to_tray = on;
        settings::set_quit_to_tray(on);
        Settings::update(move |s| s.quit_to_tray = on);
        tray::sync(cx);
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
        Settings::update(move |s| s.hide_menubar = on);
        cx.notify();
    }

    /// The decorations switch, the Window menu toggle's twin: flip the
    /// flag, persist, and renegotiate the workspace windows.
    fn set_os_decorations(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_os_decorations(on);
        Settings::update(move |s| s.os_decorations = on);
        crate::workspace::apply_decorations(cx);
        cx.notify();
    }

    /// The app font: through the live static, so every open window
    /// repaints in the new family, and into the file. None follows the
    /// platform default.
    fn set_app_font(&mut self, font: Option<String>, cx: &mut Context<Self>) {
        settings::set_app_font(font.clone(), cx);
        Settings::update(move |s| s.app_font = font);
        cx.notify();
    }

    /// The rating scale: through the live static, so every open rating
    /// column redraws, and into the file.
    fn set_rating_style(&mut self, style: RatingStyle, cx: &mut Context<Self>) {
        self.rating_style = style;
        settings::set_rating_style(style, cx);
        Settings::update(move |s| s.rating_style = style);
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
        let (surface, backdrop) = (self.surface_opacity, self.backdrop_strength);
        Settings::update(move |s| {
            s.surface_opacity = surface;
            s.backdrop_strength = backdrop;
        });
        cx.notify();
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

    /// Back to the stock palette; the file's map empties rather than
    /// filling with defaults.
    fn reset_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_palette(Palette::default(), window, cx);
        Settings::update(|s| s.palette.clear());
    }

    /// Flip the working palette light for dark, the accents held: a dark
    /// theme comes back light without redrawing every swatch by hand. The
    /// map persists like any other edit, so the flip survives a restart.
    fn inverse_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_palette(self.base.inverse(), window, cx);
        let map = self.base.to_map();
        Settings::update(move |s| s.palette = map);
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
        let map = self.base.to_map();
        Settings::update(move |s| s.palette = map);
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
                this.apply_palette(Palette::from_map(&map), window, cx);
                let map = this.base.to_map();
                Settings::update(move |s| s.palette = map);
            })
            .ok();
        })
        .detach();
    }

    /// Save a palette file, [`Palette::to_map`]'s shape: the working
    /// palette, or the derived one while song theming drives the colors,
    /// so a look a track built can leave as a theme.
    fn export_palette(&mut self, cx: &mut Context<Self>) {
        let map = if self.art_theming {
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

    fn appearance_page(&self, columns: usize, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(
                "Interface",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        "Hide Menubar",
                        Some("Keep the menubar hidden, floating it over the dock while alt is held"),
                        panel::toggle(settings::hide_menubar(), Self::set_hide_menubar, cx),
                    ))
                    .child(panel::setting_row(
                        "OS Decorations",
                        Some("The OS titlebar and borders on the main windows; off leans on the window controls and drag anchor panels"),
                        panel::toggle(settings::os_decorations(), Self::set_os_decorations, cx),
                    )),
            ))
            .child(section(
                "Theming",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        "Song Theming",
                        Some("Tint the palette and back windows with the playing track's cover art"),
                        panel::toggle(self.art_theming, Self::set_art_theming, cx),
                    ))
                    .child(panel::setting_row(
                        "Keep Dark",
                        Some("Hold the dark surfaces even when a bright cover would flip the app light; song theming still tints the color"),
                        panel::toggle(self.keep_dark, Self::set_keep_dark, cx),
                    )),
            ))
            .child(section(
                "Typography",
                None,
                panel::setting_row(
                    "Font",
                    Some("The app-wide typeface; panels can override it in their own settings"),
                    panel::font_picker(
                        "app-font",
                        settings::app_font().map(|font| font.to_string()),
                        Self::set_app_font,
                        cx,
                    ),
                ),
            ))
            .child(self.icons_section(cx))
            .child(section(
                "Transparency",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        "Surface Opacity",
                        Some("How opaque the app's surfaces read over the backdrop"),
                        settings_ui::slider(
                            &self.surface_scrub,
                            self.surface_opacity,
                            Self::set_surface,
                            cx,
                        ),
                    ))
                    .child(panel::setting_row(
                        "Backdrop Strength",
                        Some("How strongly the cover backdrop shows behind them"),
                        settings_ui::slider(
                            &self.backdrop_scrub,
                            self.backdrop_strength,
                            Self::set_backdrop,
                            cx,
                        ),
                    )),
            ))
            .child(self.colors_section(columns, cx))
    }

    /// The Icons section: the built-in set and every pack the user has as a
    /// list, each a set to switch to; the current one carries an Active
    /// badge. Creating a new pack, seeded with the built-in icons for an
    /// author to edit, rides the header.
    fn icons_section(&self, cx: &mut Context<Self>) -> Div {
        let active = Settings::load().icon_pack;
        let packs = crate::icon_packs::all();

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
        section("Icons", Some(controls.into_any_element()), list)
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
        crate::icon_packs::activate(name.as_deref());
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
        match crate::icon_packs::create(&name) {
            Ok(created) => {
                self.pack_name
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.set_icon_pack(Some(created), cx);
            }
            Err(e) => eprintln!("icon pack: creating {name:?}: {e}"),
        }
    }

    /// Delete a pack. If it was the active one, fall back to the built-in
    /// set so the resolver never points at a folder that is gone.
    fn delete_pack(&mut self, name: &str, cx: &mut Context<Self>) {
        if Settings::load().icon_pack.as_deref() == Some(name) {
            self.set_icon_pack(None, cx);
        }
        crate::icon_packs::delete(name);
        cx.notify();
    }

    /// Reveal a pack's folder in the OS file manager, so its SVGs can be
    /// swapped out with a text or vector editor.
    fn reveal_pack(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(dir) = crate::icon_packs::resolve_dir(name) {
            cx.reveal_path(&dir);
        }
    }

    fn behavior_page(&self, cx: &mut Context<Self>) -> Div {
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
        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(
                "Startup",
                None,
                panel::setting_row(
                    "Restore Last Track",
                    Some("Launch with the last playing track loaded, paused where it left off"),
                    panel::toggle(self.restore_last_track, Self::set_restore_last_track, cx),
                ),
            ))
            // No tray backend on Windows yet, and a resident process with no
            // way back in is worse than quitting, so the row sits out there.
            .when(tray::supported(), |page| {
                page.child(section(
                    "Window",
                    None,
                    panel::setting_row(
                        "Remain in Tray",
                        Some(
                            "Keep the music playing when the last window closes, with the \
                             tray icon (the dock on macOS) as the way back in",
                        ),
                        panel::toggle(self.quit_to_tray, Self::set_quit_to_tray, cx),
                    ),
                ))
            })
            .child(section("Data", None, portable_row))
            .child(section(
                "Ratings",
                None,
                panel::setting_row(
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
                ),
            ))
    }

    /// The Workspace page: the sharing hub. A workspace is a whole look -
    /// layout presets, palette, appearance - traded as one file; presets are
    /// single layouts under it. The composition tree below shows the opening
    /// window's dock, splits and tab groups as muted structure lines, panels
    /// as named rows with their settings a click away.
    fn workspace_page(&self, cx: &mut Context<Self>) -> Div {
        let live = self.workspace.upgrade().is_some();
        let mut body = div().flex().flex_col().gap(tokens::SPACE_XS).child(
            div().text_xs().text_color(palette::text_muted()).child(
                "The window's panels as they sit in splits and tab groups; \
                 the arrows reorder a row among its siblings, the lock pins \
                 a panel in place, and the gear opens its settings",
            ),
        );
        match self.workspace.upgrade() {
            Some(workspace) => {
                let root = workspace.read(cx).dock().read(cx).items().view();
                let mut rows = Vec::new();
                self.tree_rows(root, 0, TreeSlot::Root, &mut rows, cx);
                body = body.child(div().flex().flex_col().children(rows));
            }
            None => {
                body = body.child(
                    div()
                        .text_color(palette::text_muted())
                        .child("The workspace window is closed"),
                );
            }
        }

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(self.workspaces_section(live, cx))
            .child(self.presets_section(live, cx))
            .child(section("Composition", None, body))
    }

    /// The workspaces section: the saved and shipped bundles as a list, each
    /// a whole look to apply, export, or delete. Saving the current state as
    /// a named workspace, and importing one, ride the header.
    fn workspaces_section(&self, live: bool, cx: &mut Context<Self>) -> Div {
        let settings = Settings::load();
        let entries = crate::workspaces::all(&settings);

        // Save-current-as and import ride the header, so a workspace is one
        // name away and a shared file one pick away.
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(Input::new(&self.workspace_name).small().w(px(150.)))
            .child(small_button(
                "Save Current",
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.save_workspace(window, cx)),
            ))
            .child(small_button(
                "Import",
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.import_workspace(window, cx)),
            ));

        let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
            div().text_xs().text_color(palette::text_muted()).child(
                "A workspace is a whole look - layouts, palette, appearance; \
                 applying one replaces all three",
            ),
        );
        if entries.is_empty() {
            list = list.child(
                div()
                    .text_color(palette::text_muted())
                    .child("No workspaces yet"),
            );
        } else {
            list = list.child(
                div().flex().flex_col().children(
                    entries
                        .into_iter()
                        .map(|entry| self.workspace_row(entry, live, cx)),
                ),
            );
        }
        section("Workspaces", Some(controls.into_any_element()), list)
    }

    /// One workspace's row: its name, a shipped tag when it comes from the
    /// app's assets, apply, and for the user's own, export, overwrite and
    /// delete.
    fn workspace_row(
        &self,
        entry: crate::workspaces::Entry,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = entry.bundle.name.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(name.clone())),
            )
            .when(entry.builtin, |d| d.child(shipped_tag()))
            // Applying replaces the whole look, so it routes through the
            // confirm dialog rather than acting straight off the click.
            .child(small_button("Apply", icons::CHECK, !live, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| {
                    this.pending = Some(Pending::ApplyWorkspace(name.clone()));
                    cx.notify();
                })
            }))
            .when(!entry.builtin, |d| {
                // Export, overwrite and delete are the user's own workspaces
                // only; a shipped one already lives in the app's assets, so
                // there's nothing to save back out. Overwrite routes through
                // the confirm dialog before the replace, matching the presets
                // list and unlike apply and delete which are their own undo.
                d.child(small_button("Export", icons::UPLOAD, false, {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.export_workspace(&name, cx))
                }))
                .child(small_button("Overwrite", icons::REFRESH_CW, !live, {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.pending = Some(Pending::OverwriteWorkspace(name.clone()));
                        cx.notify();
                    })
                }))
                .child(icon_button(icons::TRASH, false, {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.delete_workspace(&name, cx))
                }))
            })
            .into_any_element()
    }

    /// The presets section: the saved and shipped layouts as a list, each
    /// with the roles the mini-player button toggles between and the ways
    /// to apply, delete, or overwrite it. Saving the live layout as a named
    /// preset rides the header.
    fn presets_section(&self, live: bool, cx: &mut Context<Self>) -> Div {
        let settings = Settings::load();
        let presets = crate::layouts::all(&settings);

        // Save-current-as and import ride the header, so a preset is one
        // arrangement plus a name away, or one shared file away.
        let save = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(Input::new(&self.layout_name).small().w(px(150.)))
            .child(small_button(
                "Save Current",
                icons::DOWNLOAD,
                !live,
                cx.listener(|this, _, window, cx| this.save_layout_preset(window, cx)),
            ))
            .child(small_button(
                "Import",
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.import_preset(window, cx)),
            ));

        let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
            div().text_xs().text_color(palette::text_muted()).child(
                "Primary and mini are the two the menubar's mini-player button \
                 swaps between",
            ),
        );
        if presets.is_empty() {
            list = list.child(
                div()
                    .text_color(palette::text_muted())
                    .child("No layouts yet"),
            );
        } else {
            list = list.child(
                div().flex().flex_col().children(
                    presets
                        .into_iter()
                        .map(|preset| self.preset_row(preset, live, cx)),
                ),
            );
        }
        section("Layouts", Some(save.into_any_element()), list)
    }

    /// One preset's row: its name, a shipped tag when it comes from the
    /// app's assets, the primary and mini role badges, and apply plus, for
    /// the user's own, delete.
    fn preset_row(&self, preset: Preset, live: bool, cx: &mut Context<Self>) -> AnyElement {
        let is_primary = self.primary_layout.as_deref() == Some(preset.name.as_str());
        let is_mini = self.mini_layout.as_deref() == Some(preset.name.as_str());
        let name = preset.name.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(preset.name.clone())),
            )
            .child(role_chip("Primary", is_primary, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.set_primary(&name, cx))
            }))
            .child(role_chip("Mini", is_mini, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.set_mini(&name, cx))
            }))
            .child(small_button("Apply", icons::CHECK, !live, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.apply_preset(&name, cx))
            }))
            .child(small_button("Export", icons::UPLOAD, false, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.export_preset(&name, cx))
            }))
            // Overwrite the saved preset with the live layout; the dialog
            // confirms before the replace, unlike apply and delete which are
            // their own undo.
            .child(small_button("Overwrite", icons::REFRESH_CW, !live, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| {
                    this.pending = Some(Pending::OverwritePreset(name.clone()));
                    cx.notify();
                })
            }))
            .child(icon_button(icons::TRASH, false, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.delete_preset(&name, cx))
            }))
            .into_any_element()
    }

    /// Save the workspace's live layout as a named preset, panel configs
    /// and themes along with it. An empty name is ignored; a name that
    /// already exists routes through the confirm dialog rather than a silent
    /// replace. Clears the field on a fresh save.
    fn save_layout_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let name = self.layout_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        if Settings::load().layouts.iter().any(|l| l.name == name) {
            self.pending = Some(Pending::OverwritePreset(name));
            cx.notify();
            return;
        }
        let dump = workspace.read(cx).dock().read(cx).dump(cx);
        let Ok(dump) = serde_json::to_value(dump) else {
            return;
        };
        let size = self.workspace_window_size(cx);
        Settings::update(move |s| s.layouts.push(NamedLayout { name, dump, size }));
        self.layout_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Replace the pending preset's dump and window size with the live ones,
    /// the confirm dialog's yes. Clears the name field on success.
    fn overwrite_preset(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            let dump = workspace.read(cx).dock().read(cx).dump(cx);
            if let Ok(dump) = serde_json::to_value(dump) {
                let size = self.workspace_window_size(cx);
                Settings::update(move |s| {
                    if let Some(existing) = s.layouts.iter_mut().find(|l| l.name == name) {
                        existing.dump = dump;
                        existing.size = size;
                    }
                });
            }
        }
        self.layout_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// The workspace window's content size, for storing with a preset. None
    /// when that window is gone.
    fn workspace_window_size(&self, cx: &mut App) -> Option<LayoutSize> {
        self.workspace_window
            .update(cx, |_, window, _| {
                let s = window.window_bounds().get_bounds().size;
                LayoutSize {
                    width: s.width.into(),
                    height: s.height.into(),
                }
            })
            .ok()
    }

    /// Apply a preset to the workspace's dock, in its own window - the same
    /// path an imported file takes.
    fn apply_preset(&mut self, name: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let name = name.to_string();
        self.workspace_window
            .update(cx, |_, window, cx| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.apply_named_layout(&name, window, cx);
                    });
                }
            })
            .ok();
        cx.notify();
    }

    /// Point the mini-player button's primary role at a preset, or clear it
    /// when the preset already holds the role.
    fn set_primary(&mut self, name: &str, cx: &mut Context<Self>) {
        let clear = self.primary_layout.as_deref() == Some(name);
        self.primary_layout = (!clear).then(|| name.to_string());
        let value = self.primary_layout.clone();
        Settings::update(move |s| s.primary_layout = value);
        self.sync_roles_to_workspace(cx);
        cx.notify();
    }

    /// Point the mini role at a preset, or clear it when the preset already
    /// holds it.
    fn set_mini(&mut self, name: &str, cx: &mut Context<Self>) {
        let clear = self.mini_layout.as_deref() == Some(name);
        self.mini_layout = (!clear).then(|| name.to_string());
        let value = self.mini_layout.clone();
        Settings::update(move |s| s.mini_layout = value);
        self.sync_roles_to_workspace(cx);
        cx.notify();
    }

    /// Delete a user preset, dropping any role it held so the button never
    /// points at a gone name.
    fn delete_preset(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.to_string();
        if self.primary_layout.as_deref() == Some(name.as_str()) {
            self.primary_layout = None;
        }
        if self.mini_layout.as_deref() == Some(name.as_str()) {
            self.mini_layout = None;
        }
        Settings::update(|s| {
            s.layouts.retain(|l| l.name != name);
            if s.primary_layout.as_deref() == Some(name.as_str()) {
                s.primary_layout = None;
            }
            if s.mini_layout.as_deref() == Some(name.as_str()) {
                s.mini_layout = None;
            }
        });
        self.sync_roles_to_workspace(cx);
        cx.notify();
    }

    /// Push the current roles to the workspace so its mini-player button
    /// reflects the edit without waiting on a reload, and repaint it.
    fn sync_roles_to_workspace(&self, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            let primary = self.primary_layout.clone();
            let mini = self.mini_layout.clone();
            workspace.update(cx, |workspace, cx| {
                workspace.set_mini_roles(primary, mini);
                cx.notify();
            });
        }
    }

    /// The confirm dialog, up while a destructive action waits on the user:
    /// an overwrite or a workspace apply, each with its own wording. A scrim
    /// occludes the page under it; the buttons are the only way out, no
    /// click-away, so the action is deliberate.
    fn confirm_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (title, body, confirm): (String, &'static str, &'static str) =
            match self.pending.as_ref()? {
                Pending::OverwritePreset(name) => (
                    format!("Overwrite \"{name}\"?"),
                    "This replaces the saved layout with the current one.",
                    "Overwrite",
                ),
                Pending::OverwriteWorkspace(name) => (
                    format!("Overwrite workspace \"{name}\"?"),
                    "This replaces the saved workspace with the current state.",
                    "Overwrite",
                ),
                Pending::ApplyWorkspace(name) => (
                    format!("Apply \"{name}\"?"),
                    "This replaces your layouts, palette, and appearance with the workspace's.",
                    "Apply",
                ),
            };
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000066))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .w(px(320.))
                        .p(tokens::SPACE_MD)
                        .rounded(tokens::RADIUS)
                        .bg(palette::bg_menu_opaque())
                        .border_1()
                        .border_color(palette::border_light())
                        .shadow_md()
                        .child(div().child(SharedString::from(title)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(body),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap(tokens::SPACE_SM)
                                .child(dialog_button(
                                    "Cancel",
                                    false,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(dialog_button(
                                    confirm,
                                    true,
                                    cx.listener(|this, _, window, cx| {
                                        this.confirm_pending(window, cx)
                                    }),
                                )),
                        ),
                ),
        )
    }

    /// Carry out the pending action, the confirm dialog's yes, and clear it.
    fn confirm_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pending.take() {
            Some(Pending::OverwritePreset(name)) => self.overwrite_preset(name, window, cx),
            Some(Pending::OverwriteWorkspace(name)) => self.overwrite_workspace(name, window, cx),
            Some(Pending::ApplyWorkspace(name)) => self.apply_workspace(&name, window, cx),
            None => {}
        }
    }

    /// One node of the dock into rows. Walks the live stack and tab
    /// entities rather than the dock's `DockItem` tree, which goes stale
    /// once tabs are dragged around; these are what `dump` serializes.
    /// `slot` carries where the node sits among its siblings, so its row
    /// can offer the reorder arrows.
    fn tree_rows(
        &self,
        node: Arc<dyn PanelView>,
        depth: usize,
        slot: TreeSlot,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let view = node.view();
        if let Ok(stack) = view.clone().downcast::<StackPanel>() {
            let (axis, children) = {
                let stack = stack.read(cx);
                (stack.axis(), stack.panels().to_vec())
            };
            rows.push(chrome_row(
                depth,
                match axis {
                    Axis::Horizontal => "Split, side by side",
                    Axis::Vertical => "Split, stacked",
                },
                self.move_controls(&slot, cx),
            ));
            let len = children.len();
            for (ix, child) in children.into_iter().enumerate() {
                let child_slot = TreeSlot::Stack {
                    stack: stack.clone(),
                    ix,
                    len,
                };
                self.tree_rows(child, depth + 1, child_slot, rows, cx);
            }
            return;
        }
        if let Ok(tabs) = view.downcast::<TabPanel>() {
            let children = tabs.read(cx).panels().to_vec();
            // A group of one reads as just its panel; the group only
            // earns its own line once there are tabs to speak of. The
            // solo row inherits the group's slot, so its arrows move the
            // enclosing tab group within the split.
            if let [only] = children.as_slice() {
                self.panel_rows(only.clone(), depth, slot, rows, cx);
                return;
            }
            rows.push(chrome_row(depth, "Tabs", self.move_controls(&slot, cx)));
            let len = children.len();
            for (ix, child) in children.into_iter().enumerate() {
                let child_slot = TreeSlot::Tabs {
                    tabs: tabs.clone(),
                    ix,
                    len,
                };
                self.panel_rows(child, depth + 1, child_slot, rows, cx);
            }
            return;
        }
        self.panel_rows(node, depth, slot, rows, cx);
    }

    /// A panel's row, and under a composite host (group, depth, slide)
    /// its hosted children as indented rows of their own, so the tree
    /// shows what the host holds instead of one opaque line.
    fn panel_rows(
        &self,
        panel: Arc<dyn PanelView>,
        depth: usize,
        slot: TreeSlot,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let children = crate::composite::hosted_children(&panel, cx);
        rows.push(self.panel_row(panel, depth, slot, cx));
        if let Some(children) = children {
            for child in children {
                match child {
                    Some(child) => {
                        rows.push(self.panel_row(child, depth + 1, TreeSlot::Hosted, cx))
                    }
                    None => rows.push(chrome_row(depth + 1, "Empty slot", None)),
                }
            }
        }
    }

    /// A panel's row of the tree: its name (the rename first with the
    /// type in parens), the reorder arrows, the placement-lock toggle,
    /// and the gear opening the same settings window the panel's own
    /// dropdown does. Hosted children skip the arrows and the lock: the
    /// dock never sees them, so neither applies.
    fn panel_row(
        &self,
        panel: Arc<dyn PanelView>,
        depth: usize,
        slot: TreeSlot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let type_name = panel::display_name(panel.panel_name(cx));
        let name: SharedString = match panel.tab_name(cx) {
            Some(custom) => format!("{custom} ({type_name})").into(),
            None => type_name.into(),
        };
        let hosted = matches!(slot, TreeSlot::Hosted);
        let locked = panel.locked(cx);
        let lock_panel = panel.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_MD)
            .pl(indent(depth))
            .group(TREE_ROW_GROUP)
            .child(div().min_w_0().truncate().child(name))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .children(self.move_controls(&slot, cx))
                    .when(!hosted, |d| {
                        let button = icon_button(
                            if locked { icons::LOCK } else { icons::LOCK_OPEN },
                            false,
                            cx.listener(move |_, _, _, cx| {
                                panel_settings::toggle_locked_for_view(&lock_panel, cx);
                                cx.notify();
                            }),
                        );
                        // A closed lock is state worth seeing at rest;
                        // the open one only shows with the row's other
                        // controls.
                        d.child(if locked { button } else { reveal(button) })
                    })
                    .child(reveal(icon_button(icons::SETTINGS, false, move |_, _, cx| {
                        panel_settings::open_for_view(&panel, cx);
                    }))),
            )
            .into_any_element()
    }

    /// The move controls for a movable tree node: the lift-out arrow
    /// pulling it up a layer, then up and down among its siblings, inert
    /// where a direction has nowhere to go. None for the dock root and
    /// hosted children, which have no siblings to move among here.
    fn move_controls(&self, slot: &TreeSlot, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (ix, len) = match slot {
            TreeSlot::Stack { ix, len, .. } | TreeSlot::Tabs { ix, len, .. } => (*ix, *len),
            TreeSlot::Root | TreeSlot::Hosted => return None,
        };
        let lift = self.lift_button(slot, cx);
        let up = self.move_button(slot, icons::ARROW_UP, ix == 0, ix.wrapping_sub(1), cx);
        let down = self.move_button(slot, icons::ARROW_DOWN, ix + 1 >= len, ix + 1, cx);
        Some(
            reveal(div())
                .flex()
                .flex_row()
                .items_center()
                .child(lift)
                .child(up)
                .child(down)
                .into_any_element(),
        )
    }

    /// The lift-out arrow: pull the node one layer up. A tab leaves its
    /// group for one of its own beside it; a split's child (a tab group
    /// or nested split) moves out into the enclosing split. Inert where
    /// there is no layer above - the root split's children stay put.
    fn lift_button(&self, slot: &TreeSlot, cx: &mut Context<Self>) -> Div {
        match slot {
            TreeSlot::Stack { stack, ix, .. } => {
                let dock = self
                    .workspace
                    .upgrade()
                    .map(|workspace| workspace.read(cx).dock().downgrade());
                let inert = dock.is_none() || stack.read(cx).parent().is_none();
                let stack = stack.clone();
                let from = *ix;
                icon_button(
                    icons::ARROW_LEFT,
                    inert,
                    cx.listener(move |this, _, _, cx| {
                        let Some(dock) = dock.clone() else {
                            return;
                        };
                        this.workspace_window
                            .update(cx, |_, window, cx| {
                                stack.update(cx, |stack, cx| {
                                    stack.lift_panel(from, dock, window, cx)
                                });
                            })
                            .ok();
                        cx.notify();
                    }),
                )
            }
            TreeSlot::Tabs { tabs, ix, .. } => {
                let tabs = tabs.clone();
                let from = *ix;
                icon_button(
                    icons::ARROW_LEFT,
                    false,
                    cx.listener(move |this, _, _, cx| {
                        this.workspace_window
                            .update(cx, |_, window, cx| {
                                tabs.update(cx, |tabs, cx| tabs.lift_panel(from, window, cx));
                            })
                            .ok();
                        cx.notify();
                    }),
                )
            }
            TreeSlot::Root | TreeSlot::Hosted => div(),
        }
    }

    /// One reorder arrow: moves the node from its index to `to_ix` in
    /// its parent stack or tab group. The move APIs ignore out-of-range
    /// indices, but the ends render inert anyway so the tree telegraphs
    /// where a row can still go.
    fn move_button(
        &self,
        slot: &TreeSlot,
        icon: &'static str,
        inert: bool,
        to_ix: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        match slot {
            TreeSlot::Stack { stack, ix, .. } => {
                let stack = stack.clone();
                let from = *ix;
                icon_button(
                    icon,
                    inert,
                    cx.listener(move |_, _, _, cx| {
                        stack.update(cx, |stack, cx| stack.move_panel(from, to_ix, cx));
                        cx.notify();
                    }),
                )
            }
            TreeSlot::Tabs { tabs, ix, .. } => {
                let tabs = tabs.clone();
                let from = *ix;
                icon_button(
                    icon,
                    inert,
                    cx.listener(move |_, _, _, cx| {
                        tabs.update(cx, |tabs, cx| tabs.move_panel(from, to_ix, cx));
                        cx.notify();
                    }),
                )
            }
            TreeSlot::Root | TreeSlot::Hosted => div(),
        }
    }

    /// Export a preset to a file: its dump, panel configs and themes
    /// included, so a single layout can leave as a shareable artifact. Works
    /// for shipped presets too, which are dumps like any other.
    fn export_preset(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(preset) = crate::layouts::resolve(&Settings::load(), name) else {
            return;
        };
        let dump = preset.dump;
        let home = dirs::home_dir().unwrap_or_default();
        let file = format!("{name}.json");
        let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            if let Ok(json) = serde_json::to_string_pretty(&dump) {
                std::fs::write(path, json).ok();
            }
        })
        .detach();
    }

    /// Pick a layout file and add it as a new preset, named after the file
    /// and deduped so an import never shadows an existing preset. The file
    /// must parse as a dock dump, the same shape export writes; anything else
    /// is ignored.
    fn import_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let Some(dump) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                .filter(|value| serde_json::from_value::<DockAreaState>(value.clone()).is_ok())
            else {
                return;
            };
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "imported".to_string());
            let name = crate::workspaces::unique_name(&stem, |candidate| {
                crate::layouts::all(&Settings::load())
                    .iter()
                    .any(|p| p.name == candidate)
            });
            Settings::update(move |s| {
                s.layouts.push(NamedLayout {
                    name,
                    dump,
                    size: None,
                })
            });
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Flush the workspace window's live dock to the settings file. Panel
    /// config like the library's column arrangement only reaches disk on the
    /// next layout dump, so without this a workspace save from here would
    /// capture whatever's stale on disk instead of the current look.
    fn flush_workspace_layout(&self, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let _ = self.workspace_window.update(cx, |_, window, cx| {
            if let Some(ws) = ws.upgrade() {
                ws.update(cx, |this, cx| this.persist(window, cx));
            }
        });
    }

    /// Save the current state as a named workspace: layouts, palette, and
    /// appearance in one bundle. An empty name is ignored; a name that already
    /// exists routes through the confirm dialog. Clears the field on a fresh
    /// save.
    fn save_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.workspace_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.flush_workspace_layout(cx);
        if Settings::load().workspaces.iter().any(|w| w.name == name) {
            self.pending = Some(Pending::OverwriteWorkspace(name));
            cx.notify();
            return;
        }
        let bundle = WorkspaceBundle::from_settings(name, &Settings::load());
        Settings::update(move |s| s.workspaces.push(bundle));
        self.workspace_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Replace a saved workspace with the current state, the confirm dialog's
    /// yes. Clears the name field.
    fn overwrite_workspace(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_workspace_layout(cx);
        let bundle = WorkspaceBundle::from_settings(name.clone(), &Settings::load());
        Settings::update(move |s| {
            if let Some(existing) = s.workspaces.iter_mut().find(|w| w.name == name) {
                *existing = bundle;
            }
        });
        self.workspace_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Delete a user workspace. Shipped ones carry no delete.
    fn delete_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.to_string();
        Settings::update(move |s| s.workspaces.retain(|w| w.name != name));
        cx.notify();
    }

    /// Export a workspace bundle to a file, the whole look as one shareable
    /// artifact. Works for shipped bundles too.
    fn export_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(bundle) = crate::workspaces::resolve(&Settings::load(), name) else {
            return;
        };
        let home = dirs::home_dir().unwrap_or_default();
        let file = format!("{name}.json");
        let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            if let Ok(json) = serde_json::to_string_pretty(&bundle) {
                std::fs::write(path, json).ok();
            }
        })
        .detach();
    }

    /// Pick a workspace file and add it to the collection, named after the
    /// file when the bundle carries no name of its own and deduped so an
    /// import never shadows an existing workspace. A bundle from a newer
    /// format, or a file that isn't a bundle, is ignored.
    fn import_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let Some(bundle) = crate::workspaces::read_bundle(&path, &Settings::load()) else {
                return;
            };
            Settings::update(move |s| s.workspaces.push(bundle));
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Apply a workspace: replace the live look wholesale. The persist and
    /// the appearance statics ride the shared path; this window mirrors the
    /// applied look into its own editor state on top, and swaps the dock to
    /// the bundle's primary layout when it names one.
    fn apply_workspace(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bundle) = crate::workspaces::resolve(&Settings::load(), name) else {
            return;
        };
        // Persist the replace and repaint every open window through the live
        // statics, the empty launcher's path too.
        crate::workspaces::apply_look(&bundle, cx);
        // Mirror the applied look into this window's own editor state so the
        // swatches, pickers, and sliders show it. apply_palette re-sets the
        // live palette, which apply_look already did; the repeat is idempotent.
        self.apply_palette(Palette::from_map(&bundle.palette), window, cx);
        let a = &bundle.appearance;
        self.surface_opacity = a.surface_opacity;
        self.backdrop_strength = a.backdrop_strength;
        self.art_theming = a.art_theming;
        self.keep_dark = a.keep_dark;
        self.rating_style = a.rating_style;
        // The mini-player roles, and when the bundle names a primary layout,
        // the dock itself.
        self.primary_layout = bundle.primary_layout.clone();
        self.mini_layout = bundle.mini_layout.clone();
        self.sync_roles_to_workspace(cx);
        if let Some(primary) = bundle.primary_layout.clone() {
            self.apply_preset(&primary, cx);
        }
        cx.notify();
    }

    /// The Scrobbling page: the last.fm account section - the user's own
    /// api credentials, the connect flow, the connection readout - and
    /// the scrobbling knobs under it.
    fn scrobbling_page(&self, cx: &mut Context<Self>) -> Div {
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

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section("Last.fm", None, account))
            .child(section(
                "Scrobbling",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
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
                    ))
                    .child(panel::setting_row(
                        "Scrobble Threshold",
                        Some(
                            "How much of a track has to play before it scrobbles; \
                             the seek strip and waveform can mark it",
                        ),
                        settings_ui::slider(
                            &self.threshold_scrub,
                            config.threshold,
                            |this: &mut Self, fraction, cx| {
                                this.scrobbler
                                    .update(cx, |s, cx| s.set_threshold(fraction, cx));
                                cx.notify();
                            },
                            cx,
                        ),
                    )),
            ))
    }

    /// The lrclib toggle: through the live static, so the lyrics panel's
    /// fetch action appears and hides with it, and into the file.
    fn set_lrclib(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.lrclib = on;
        providers::set_lyrics_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.providers = config);
        cx.notify();
    }

    /// Where a fetched sheet saves: straight into the file, read at
    /// fetch time.
    fn set_lyrics_save(&mut self, save: LyricsSave, cx: &mut Context<Self>) {
        self.providers.lyrics_save = save;
        let config = self.providers.clone();
        Settings::update(move |s| s.providers = config);
        cx.notify();
    }

    /// The MusicBrainz toggle: through the live static, so the metadata
    /// panel's lookup action appears and hides with it, and into the file.
    fn set_musicbrainz(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.musicbrainz = on;
        providers::set_metadata_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.providers = config);
        cx.notify();
    }

    /// The iTunes cover-art toggle: through the live static and into the
    /// file, so the cover editor's search follows it.
    fn set_itunes(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.itunes = on;
        providers::set_itunes_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.providers = config);
        cx.notify();
    }

    /// The Deezer cover-art toggle, iTunes's twin.
    fn set_deezer(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.deezer = on;
        providers::set_deezer_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.providers = config);
        cx.notify();
    }

    /// The artist-lookup toggle: through the live static, so the
    /// biography panel's fetches follow it.
    fn set_artist(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.artist = on;
        providers::set_artist_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.providers = config);
        cx.notify();
    }

    /// The Providers page: the online enrichment services (ADR 14), a
    /// section per domain. Nothing here fetches on its own; the toggles
    /// gate the actions the panels offer.
    fn providers_page(&self, cx: &mut Context<Self>) -> Div {
        div().flex().flex_col().gap(SECTION_GAP).child(section(
            "Lyrics",
            None,
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(
                            "Online lookups run only when a panel action asks for one; \
                             playback and browsing never touch the network",
                        ),
                )
                .child(panel::setting_row(
                    "LRCLIB",
                    Some("Fetch missing lyrics from lrclib.net, synced sheets when it has them"),
                    panel::toggle(self.providers.lrclib, Self::set_lrclib, cx),
                ))
                .child(panel::setting_row(
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
                )),
        ))
        .child(section(
            "Metadata",
            None,
            panel::setting_row(
                "MusicBrainz",
                Some(
                    "Look up tags on musicbrainz.org; the metadata panel's search \
                     shows matches to confirm field by field before writing",
                ),
                panel::toggle(self.providers.musicbrainz, Self::set_musicbrainz, cx),
            ),
        ))
        .child(section(
            "Cover Art",
            None,
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(panel::setting_row(
                    "iTunes",
                    Some("Search iTunes for cover art; the cover editor's search shows matches to pick before setting"),
                    panel::toggle(self.providers.itunes, Self::set_itunes, cx),
                ))
                .child(panel::setting_row(
                    "Deezer",
                    Some("Search Deezer for cover art, up to 1000 pixels"),
                    panel::toggle(self.providers.deezer, Self::set_deezer, cx),
                )),
        ))
        .child(section(
            "Artist",
            None,
            panel::setting_row(
                "Last.fm",
                Some(
                    "Fetch artist biographies, stats, and similar artists for the \
                     biography panel, with a portrait from Deezer; everything is \
                     kept in the data folder and reads offline afterwards",
                ),
                panel::toggle(self.providers.artist, Self::set_artist, cx),
            ),
        ))
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

    fn colors_section(&self, columns: usize, cx: &mut Context<Self>) -> Div {
        let locked = self.art_theming;
        let mut body = div().flex().flex_col().gap(tokens::SPACE_XS);
        if locked {
            body = body.child(div().text_xs().text_color(palette::text_muted()).child(
                "Song theming is on, so the playing track drives these colors \
                 and export saves them; turn it off above to edit them",
            ));
        }
        body = body.child(settings_ui::role_grid(columns, |j| {
            self.color_cell(&ROLES[j], &self.pickers[j], locked)
                .into_any_element()
        }));

        // Import, inverse, and reset lock with the rest of the editor:
        // they change the palette too. Apply Song Theme is the opposite,
        // live only while theming drives the colors it bakes in. Export
        // stays live; unlocked it saves the base palette, locked the
        // derived one the swatches show.
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                "Inverse",
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
        section("Colors", Some(controls.into_any_element()), body)
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

    fn library_page(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.library.read(cx).busy();
        let scanning = busy.is_some();
        let mut body = div().flex().flex_col().gap(tokens::SPACE_SM).child(
            div().text_xs().text_color(palette::text_muted()).child(
                "Folders scanned into the library; removing one drops its \
                 tracks from the catalog and leaves the files alone",
            ),
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
        body = body.child(table);

        // The library's badge and the file under the scan cursor, or the
        // resting status, under the table.
        let note: Option<SharedString> = busy.or_else(|| {
            let status = self.library.read(cx).status();
            (!status.is_empty()).then_some(status)
        });
        body = body.when_some(note, |d, note| {
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
            ));
        section("Folders", Some(controls.into_any_element()), body)
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

    fn storage_page(&self, cx: &mut Context<Self>) -> Div {
        let info = self.storage.unwrap_or_default();
        let music = format!(
            "{} tracks, {} albums, {}",
            info.music.tracks,
            info.music.albums,
            human_size(info.music.bytes)
        );
        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(
                "Library",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        "Music Files",
                        Some("What the scanned folders hold; the files stay where they are"),
                        readout(music),
                    ))
                    .child(panel::setting_row(
                        "Catalog",
                        Some("The track index scans build (library.db)"),
                        readout(human_size(info.catalog)),
                    ))
                    .child(panel::setting_row(
                        "Lyrics",
                        Some("Fetched and edited sheets kept in the app's own store (lyrics/), so library folders stay clean"),
                        readout(human_size(info.lyrics)),
                    )),
            ))
            .child(section(
                "Caches",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
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
                    ))
                    .child(panel::setting_row(
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
                    )),
            ))
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
        .child("Shipped")
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

/// A setting row's value where a control would sit.
fn readout(value: String) -> Div {
    div().text_color(palette::text_muted()).child(value)
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

        let sidebar = sidebar()
            .children(PAGES.iter().map(|&(page, label, icon)| {
                settings_ui::nav_item(
                    label,
                    icon,
                    self.page == page,
                    // Entering Storage measures the files fresh, so the
                    // numbers are current without a per-frame stat.
                    move |this: &mut Self, cx| {
                        this.page = page;
                        if page == Page::Storage {
                            this.refresh_storage(cx);
                        }
                        cx.notify();
                    },
                    cx,
                )
            }))
            // The escape hatches sink to the bottom: the raw file this
            // window edits and the folder it lives in.
            .child(div().flex_1())
            .child(self.sidebar_action("Settings File", icons::FILE_TEXT, settings_path, cx))
            .child(self.sidebar_action("Data Folder", icons::FOLDER, data_dir, cx));

        let page = match self.page {
            Page::Appearance => self.appearance_page(columns, cx),
            Page::Behavior => self.behavior_page(cx),
            Page::Workspace => self.workspace_page(cx),
            Page::Library => self.library_page(cx),
            Page::Providers => self.providers_page(cx),
            Page::Scrobbling => self.scrobbling_page(cx),
            Page::Storage => self.storage_page(cx),
        };

        div()
            .size_full()
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
                    // Always visible, not fading in on scroll: the thumb
                    // is what says more page hangs below the fold. The
                    // absolute wrapper gives the scrollbar its bounds; on
                    // its own it lays out to nothing.
                    .child(div().absolute().inset_0().child(
                        Scrollbar::vertical(&self.scroll).scrollbar_show(ScrollbarShow::Always),
                    )),
            )
            // The overwrite confirm floats over the whole window on its own
            // occluding layer, last so it paints on top of the page.
            .children(self.confirm_overlay(cx))
    }
}
