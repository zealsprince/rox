//! The rox app shell: the workspace's dock hosts the library panel over the
//! promoted library service, the audio views (spectrum, waveform) fed from
//! the player's PCM tap, and the transport panels (playback controls, seek
//! strip, volume) over the promoted playback engine in the bottom dock.
//! Panels duplicate with their own config and pop out into OS windows over
//! the same shared entities. New Window stays in the menubar so
//! multi-window on Wayland keeps getting exercised.

// On Windows a console-subsystem binary pops a terminal window next to the app.
// Build release as a GUI (windows) subsystem so it doesn't; keep the console in
// debug builds so stdout/stderr logging stays visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod artists;
mod assets;
mod backdrop;
mod catalog;
mod charts;
mod composite;
mod console_window;
mod cover;
mod design;
mod discord;
mod duplicates;
mod eq_window;
mod group_head;
mod history;
mod integrations;
mod lastfm;
mod logging;
mod lyrics;
mod matching;
mod panel;
mod panel_catalog;
mod panel_settings;
mod panels;
mod peaks;
mod player;
mod playlist_create;
mod providers;
mod query;
mod quick_play;
mod rating_ui;
mod replaygain_job;
mod selection;
mod settings;
mod source;
mod startup;
mod stats_window;
mod tags;
mod thumbs;
mod track_ui;
mod workspace;
mod workspaces;

use gpui::{
    point, px, size, App, AppContext, Application, Bounds, SharedString, TitlebarOptions,
    WindowBounds, WindowOptions,
};
use gpui_component::Root;

use assets::Assets;
use design::palette;
use settings::Settings;
use workspace::Workspace;

/// The Wayland/X11 app id, set on every window we open. Windows share it so
/// the compositor groups them as one app and, on Wayland, will consider an
/// xdg-activation request from one window to raise another (bringing an
/// already-open settings or customize window to the front). Without it the
/// backend's activate is a no-op.
pub const APP_ID: &str = "rox";

/// The floor under every rox window. Applying a layout or toggling the
/// mini-player resizes the window to a preset's stored size, and a bad or
/// zero size there used to collapse the window to nothing, so you had to go
/// fish it back out with the window manager. This is the OS-level minimum and
/// the clamp the programmatic resizes run through, small enough for a compact
/// mini-player but never zero.
pub const MIN_WINDOW_SIZE: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(240.),
    height: px(140.),
};

/// The frame size pinned on the command line: `rox --window-size 1440x900`
/// opens at exactly that and layout swaps leave it alone for the session. A
/// dev flag for shooting the workspace previews at one consistent size; the
/// window is a Wayland client, so nothing outside the process can size it.
pub(crate) fn window_size_override() -> Option<gpui::Size<gpui::Pixels>> {
    static SIZE: std::sync::OnceLock<Option<(f32, f32)>> = std::sync::OnceLock::new();
    SIZE.get_or_init(|| {
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--window-size" {
                let value = args.next()?;
                let (w, h) = value.split_once('x')?;
                return Some((w.parse().ok()?, h.parse().ok()?));
            }
        }
        None
    })
    .map(|(w, h)| {
        size(
            px(w).max(MIN_WINDOW_SIZE.width),
            px(h).max(MIN_WINDOW_SIZE.height),
        )
    })
}

pub fn open_workspace(cx: &mut App) {
    open_workspace_with(workspace::WorkspaceStart::Restore, cx);
}

/// Open a workspace window with a chosen starting layout: the Window menu's
/// New Window (restore), Empty Window, and New Window from Layout all land
/// here.
pub fn open_workspace_with(start: workspace::WorkspaceStart, cx: &mut App) {
    open_workspace_window(start, None, None, cx);
}

/// Reopen from the tray or the macOS dock: a window on the saved working
/// layout over the state the last close handed to the hold, so playback
/// carries straight over.
pub fn open_workspace_adopting(state: panel::AppState, cx: &mut App) {
    open_workspace_window(workspace::WorkspaceStart::Restore, Some(state), None, cx);
}

fn open_workspace_window(
    start: workspace::WorkspaceStart,
    adopt: Option<panel::AppState>,
    // Audio files handed to us on the command line (`rox song.flac`, or the
    // .desktop actions), with the mode the launch asked for. Play overrides
    // the restore so double-clicking a file starts it; enqueue appends to the
    // up-next queue. None on every other open.
    open: Option<(rox_library::open_files::LaunchMode, Vec<std::path::PathBuf>)>,
    cx: &mut App,
) {
    // Windows open on the saved frame, so a restart, and every New Window,
    // comes back where the last-closed window sat.
    let mut window_bounds = match Settings::load().windows.main {
        Some(w) => {
            let bounds = Bounds {
                origin: point(px(w.x), px(w.y)),
                size: size(px(w.width), px(w.height)),
            };
            if w.maximized {
                WindowBounds::Maximized(bounds)
            } else {
                WindowBounds::Windowed(bounds)
            }
        }
        // A hair larger than the welcome window (1160x660) it opens under on
        // a first run, so that window nests inside it like a child. Sized to
        // still fit a 1366x768 laptop with margin to spare.
        None => WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(720.)), cx)),
    };
    // A preset window opens at the preset's stored size when it carries one,
    // keeping the restored position; a preset without a size opens like any
    // other window.
    if let workspace::WorkspaceStart::Preset(name) = &start {
        if let Some(s) = settings::layouts::resolve(&Settings::load(), name).and_then(|p| p.size) {
            window_bounds = WindowBounds::Windowed(Bounds {
                origin: window_bounds.get_bounds().origin,
                size: size(
                    px(s.width).max(MIN_WINDOW_SIZE.width),
                    px(s.height).max(MIN_WINDOW_SIZE.height),
                ),
            });
        }
    }
    // The pinned dev size beats both the saved frame and a preset's size, so
    // every window this session comes up at exactly what the flag asked for.
    if let Some(s) = window_size_override() {
        window_bounds = WindowBounds::Windowed(Bounds {
            origin: window_bounds.get_bounds().origin,
            size: s,
        });
    }
    let options = WindowOptions {
        window_bounds: Some(window_bounds),
        window_min_size: Some(MIN_WINDOW_SIZE),
        window_decorations: Some(settings::window_decorations()),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("rox")),
            // On Windows and macOS the caption is driven by this creation-time
            // flag, not the window_decorations option below (a no-op on both
            // until our gpui patches' request_decorations runs post-open).
            // Opening in the right state avoids a chrome flash on a
            // hidden-decorations window. Linux ignores appears_transparent, so
            // scope the derived value to the two platforms that read it.
            appears_transparent: cfg!(any(target_os = "windows", target_os = "macos"))
                && !settings::os_decorations(),
            ..Default::default()
        }),
        app_id: Some(APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        // The Wayland backend ignores the creation-time titlebar title;
        // only set_window_title reaches the compositor.
        window.set_window_title("rox");
        // System-theme follow rides the OS appearance events, which only
        // reach us through a window. The window's own cached appearance
        // feeds the settings cache - the platform's read borrows the
        // Wayland client, which is already borrowed here. The immediate
        // note covers a flip that happened while no window was up (tray
        // residency); the setter dedupes, so repeats cost nothing.
        settings::note_os_appearance(window.appearance(), cx);
        window
            .observe_window_appearance(|window, cx| {
                settings::note_os_appearance(window.appearance(), cx);
            })
            .detach();
        let workspace = cx.new(|cx| Workspace::new(start, adopt, window, cx));
        // Command-line files route into the fresh window's player. The player
        // is path-based, so this works for files outside the library.
        if let Some((mode, paths)) = open {
            workspace.update(cx, |ws, cx| ws.open_paths(mode, paths, cx));
        }
        // gpui-component windows layer sheets, dialogs, and dock drag
        // overlays through a Root at the top of the window.
        cx.new(|cx| Root::new(workspace, window, cx))
    })
    .expect("failed to open the main window");
}

fn main() {
    // Files handed to us on the command line (`rox song.flac`, or the file
    // manager's Open With). Collected before the app boots so a plausible-file
    // filter runs off the real argv, not gpui's.
    let (launch_mode, launch_files) = rox_library::open_files::from_args();
    // One rox per data directory. A rox already running takes this launch -
    // its window comes back out of the tray with our files in hand - and this
    // process is done before it ever opens a compositor connection.
    let Some(instance) = startup::single_instance::claim(launch_mode, &launch_files) else {
        return;
    };
    let app = Application::new().with_assets(Assets);
    // macOS: clicking the dock icon while the app runs with no windows
    // brings a workspace back - the platform's own quit-to-tray. Only the
    // mac backend ever fires this.
    app.on_reopen(|cx| {
        if workspace::front_workspace(cx).is_none() {
            integrations::tray::reopen(cx);
        }
    });
    app.run(move |cx: &mut App| {
        // The logging backend goes up first, so anything the rest of startup
        // reports lands in the file and the console ring from the first line.
        logging::init();
        // The socket goes live next, so a launch that lands mid-startup is
        // already queued for the drain rather than bouncing off a closed
        // door and starting its own rox.
        startup::single_instance::serve(instance, cx);
        // Whether this launch found a settings file decides the welcome
        // window later; recorded before anything can write one.
        settings::note_first_run();
        gpui_component::init(cx);
        rox_dock::init(cx);
        workspace::init(cx);
        tags::editor::init(cx);
        // Startup theme wiring runs through the palette pipeline - the
        // same choke point every later palette change goes through. The
        // setters set the dark baseline and feed the widget theme tokens.
        let settings = Settings::load();
        palette::set_palettes(settings.palette_dark(), settings.palette_light(), cx);
        settings::seed_os_appearance(cx);
        settings::set_theme(settings.theme, cx);
        palette::set_scalars(
            settings.look.bundle.appearance.surface_opacity,
            settings.look.bundle.appearance.backdrop_strength,
            cx,
        );
        settings::set_app_frame(settings.look.bundle.appearance.frame, cx);
        settings::set_seams(settings.look.bundle.appearance.seams, cx);
        palette::set_keep_theme(settings.look.bundle.appearance.keep_theme, cx);
        palette::set_art_theming(settings.look.bundle.appearance.art_theming, cx);
        settings::set_app_font(settings.look.bundle.appearance.app_font.clone(), cx);
        palette::set_app_font_size(settings.app_font_size, cx);
        settings::set_rating_style(settings.look.bundle.appearance.rating_style, cx);
        settings::set_rating_dots(settings.look.bundle.appearance.rating_dots, cx);
        settings::set_hide_menubar(settings.look.bundle.appearance.hide_menubar, cx);
        settings::set_os_decorations(settings.look.bundle.appearance.os_decorations);
        settings::set_fold_case(settings.fold_case);
        rox_library::genre::set_split_compounds(settings.split_genre_compounds);
        settings::set_quit_to_tray(settings.quit_to_tray);
        settings::set_experimental(settings.experimental, cx);
        integrations::tray::sync(cx);
        // Point the icon resolver at the chosen pack before any window
        // opens, so the first frame already draws it.
        startup::icon_packs::activate(settings.icon_pack.as_deref());
        providers::set_lyrics_online(settings.accounts.providers.lrclib);
        providers::set_metadata_online(settings.accounts.providers.musicbrainz);
        providers::set_itunes_online(settings.accounts.providers.itunes);
        providers::set_deezer_online(settings.accounts.providers.deezer);
        providers::set_lastfm_art_online(settings.accounts.providers.lastfm_art);
        providers::set_artist_online(settings.accounts.providers.artist);
        // The daily update check, off the UI thread; the toggle and the
        // one-day cache both gate it, so most launches do nothing here.
        startup::updates::check_on_launch(cx);
        // Launch files ride into the first window; a plain launch (no files)
        // opens on the restored state as before.
        let open = (!launch_files.is_empty()).then_some((launch_mode, launch_files));
        open_workspace_window(workspace::WorkspaceStart::Restore, None, open, cx);
        cx.activate(true);
    });
}
