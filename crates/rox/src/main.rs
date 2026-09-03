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

mod bake;
mod bake_dialog;
mod composite;
mod console_window;
mod convert;
mod convert_dialog;
mod cover;
mod duplicates;
mod embeddings;
mod eq_window;
mod genre_tagger;
mod health_window;
mod integrations;
mod keymap;
mod lastfm;
mod lyrics;
mod matching;
mod panel_catalog;
mod panel_presets;
mod panel_settings;
mod panels;
mod pass_prompt;
mod playlist_create;
mod quick_play;
mod replaygain_job;
mod romanize_job;
mod search_window;
mod settings;
mod shader_editor;
mod signals_window;
mod smart_playlist;
mod sortnames_job;
mod startup;
mod stats_window;
mod tags;
mod tasks_window;
mod tempo_job;
mod workspace;
mod workspaces;

use gpui::{
    point, px, size, App, AppContext, Application, Bounds, SharedString, TitlebarOptions,
    WindowBounds, WindowOptions,
};
use gpui_component::Root;

use rox_core::settings::{
    layouts, note_first_run, note_os_appearance, os_decorations, resize_border, seed_os_appearance,
    set_acoustic_analysis, set_app_font, set_app_frame, set_design_mode, set_experimental,
    set_fold_case, set_gain_mode, set_hide_menubar, set_language, set_os_decorations,
    set_quit_to_tray, set_rating_dots, set_rating_style, set_resize_border, set_resize_lock,
    set_seams, set_show_readings, set_tempo_analysis, set_theme, set_workspace_migrator,
    window_decorations, Settings, MIN_WINDOW_SIZE,
};
use rox_core::{logging, APP_ID};
use rox_design::assets::Assets;
use rox_design::palette;
use rox_net::providers;
use rox_services::acoustic::set_acoustic_model;
use workspace::Workspace;

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
/// New Window (restore), Empty Window, and New Window from Layout all come
/// through here.
pub fn open_workspace_with(start: workspace::WorkspaceStart, cx: &mut App) {
    open_workspace_window(start, None, None, cx);
}

/// Reopen from the tray or the macOS dock: a window on the saved working
/// layout over the state the last close handed to the hold, so playback
/// continues straight through. The media service comes back with it where it
/// stayed registered through the windowless stretch.
pub fn open_workspace_adopting(adopt: workspace::Adopted, cx: &mut App) {
    open_workspace_window(workspace::WorkspaceStart::Restore, Some(adopt), None, cx);
}

fn open_workspace_window(
    start: workspace::WorkspaceStart,
    adopt: Option<workspace::Adopted>,
    // Audio files handed to us on the command line (`rox song.flac`, or the
    // .desktop actions), with the mode the launch asked for. Play overrides
    // the restore so double-clicking a file starts it; enqueue appends to the
    // up-next queue. None on every other open.
    open: Option<(rox_library::open_files::LaunchMode, Vec<std::path::PathBuf>)>,
    cx: &mut App,
) {
    // Windows open on the saved frame, so a restart, and every New Window,
    // comes back where the last-closed window was.
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
    // A preset window opens at the preset's stored size when it has one,
    // keeping the restored position; a preset without a size opens like any
    // other window.
    if let workspace::WorkspaceStart::Preset(name) = &start {
        if let Some(s) = layouts::resolve(&Settings::load(), name).and_then(|p| p.size) {
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
        window_decorations: Some(window_decorations()),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("rox")),
            // On Windows and macOS the caption is driven by this creation-time
            // flag, not the window_decorations option below (a no-op on both
            // until our gpui patches' request_decorations runs post-open).
            // Opening in the right state avoids a chrome flash on a
            // hidden-decorations window. Linux ignores appears_transparent, so
            // scope the derived value to the two platforms that read it.
            appears_transparent: cfg!(any(target_os = "windows", target_os = "macos"))
                && !os_decorations(),
            ..Default::default()
        }),
        app_id: Some(APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        // The Wayland backend ignores the creation-time titlebar title;
        // only set_window_title reaches the compositor.
        rox_panel_api::windows::set_window_title(window, "rox");
        // `WindowOptions::focus` already asks for this at creation, but
        // some window managers grant the map and deny the raise: the
        // window comes up on top with the keyboard still on whatever had
        // it a moment ago. Only matters for New Window and Empty Window,
        // opened from a window that already holds focus; on launch there's
        // nothing else to steal it from, so this is a no-op there.
        window.activate_window();
        // No WindowOptions field for this one, so the fresh window starts
        // with the platform default and takes the setting here.
        window.set_resize_border(resize_border());
        // System-theme follow comes from the OS appearance events, which
        // only reach us through a window. The window's own cached
        // appearance supplies the settings cache, since the platform's read
        // borrows the Wayland client, which is already borrowed here. The
        // immediate note covers a flip that happened while no window was up
        // (tray residency); the setter dedupes, so repeats cost nothing.
        note_os_appearance(window.appearance(), cx);
        window
            .observe_window_appearance(|window, cx| {
                note_os_appearance(window.appearance(), cx);
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

/// Hand rox-panel-api the windows it can't reach on its own. Panels and the
/// shared helpers are a crate down and can't depend upward, so every call
/// into a window (the tag editor, the stats page, the Add Panel flyout)
/// goes through this table. Installed before anything can open a window.
fn install_openers() {
    rox_panel_api::openers::install(rox_panel_api::openers::Openers {
        tags_editor: tags::editor::open,
        tags_matcher: tags::matcher::open,
        cover_editor: cover::editor::open,
        rename_dialog: tags::rename::open,
        convert_dialog: convert_dialog::open,
        convert_available: convert::available,
        playlist_create: playlist_create::open,
        playlist_rename: playlist_create::open_rename,
        smart_playlist: smart_playlist::open,
        eq_window: eq_window::open,
        stats_window: stats_window::open,
        health_window: health_window::open,
        signals_window: signals_window::open,
        shader_editor: shader_editor::open,
        console_notice: console_window::notice,
        lyrics_watch: watch_lyrics_panel,
        lyrics_edit: lyrics::edit::open,
        lyrics_matcher: lyrics::matcher::open,
        lyrics_saved: lyrics::saved,
        add_panel_submenu: workspace::add_panel_submenu,
        host_settings_item: composite::host_settings_item,
        confirm_close_locked,
    });
}

/// The typed side of the lyrics watch: the panel comes down type-erased,
/// since the registry can't name a concrete panel, and goes back into the
/// watcher list as itself. A panel already dropped never registers.
fn watch_lyrics_panel(panel: gpui::AnyWeakEntity, cx: &mut App) {
    let Some(panel) = panel
        .upgrade()
        .and_then(|panel| panel.downcast::<rox_panels::lyrics::LyricsPanel>().ok())
    else {
        return;
    };
    lyrics::watch(panel.downgrade(), cx);
}

/// The typed side of a pinned panel's Close: find the workspace behind the
/// window and float the confirm there. A window with no workspace behind it
/// has nowhere to put the dialog, and the pin holds as it did before.
fn confirm_close_locked(
    panel: std::sync::Arc<dyn rox_dock::PanelView>,
    tabs: gpui::WeakEntity<rox_dock::TabPanel>,
    window: &mut gpui::Window,
    cx: &mut App,
) {
    let Some(ws) = workspace::workspace_for_window(window, cx).and_then(|ws| ws.upgrade()) else {
        return;
    };
    ws.update(cx, |ws, cx| {
        ws.confirm_close_locked(panel, tabs, window, cx);
    });
}

/// Rein in glibc's malloc before the thread pools exist. Its default of one
/// arena per contending thread (up to 8 x cores) had a dozen arenas each
/// parking a few megabytes of freed-but-retained heap: about 50 MB of idle
/// footprint on the measured library, and the reason applying a workspace
/// looked like a leak - every rebuild's transient allocations spread across
/// arenas that never give pages back. Four arenas keep the parallel paths
/// out of each other's way while bounding that retention, and the megabyte
/// thresholds hand freed panel trees and cover decodes back to the kernel
/// instead of holding them against a rainy day. Measured: idle 234 -> 182 MB,
/// and thirty same-workspace applies flatten at ~230 MB where they used to
/// climb ~8 MB each, forever. Launch-to-ready stays sub-second.
///
/// glibc only; musl, macOS, and Windows allocators don't have these knobs
/// (or the retention pattern).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_allocator() {
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 4);
        libc::mallopt(libc::M_TRIM_THRESHOLD, 1 << 20);
        libc::mallopt(libc::M_MMAP_THRESHOLD, 1 << 20);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn tune_allocator() {}

/// A suspend/resume can take the Vulkan device down with it (#118: NVIDIA on
/// X11), and a lost device can't draw another frame. The renderer detects it
/// (a vendored gpui patch, z3) and hands the response to this handler:
/// re-exec the binary in place. Same pid, same argv, so `--portable` and
/// friends survive, and the launch restore brings the session back from its
/// last persist. The single-instance socket file gets left behind, but the
/// listener closes with the exec and the fresh claim treats an unanswered
/// socket as stale and rebinds.
///
/// The uptime gate keeps a GPU that's dead at boot from turning this into an
/// exec loop: a device lost this early means restarting won't help, so the
/// renderer's exit fallback takes it from there.
#[cfg(unix)]
fn install_gpu_lost_restart() {
    let booted = std::time::Instant::now();
    gpui::set_gpu_device_lost_handler(move || {
        use std::os::unix::process::CommandExt as _;
        if booted.elapsed() < std::time::Duration::from_secs(60) {
            log::error!("the GPU was lost right after launch, not restarting into it again");
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            log::error!("can't locate the running executable, not restarting");
            return;
        };
        log::error!("restarting to get a fresh GPU device");
        log::logger().flush();
        let err = std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .exec();
        // exec only returns on failure; the renderer's exit fallback runs.
        log::error!("restart failed: {err}");
    });
}

#[cfg(not(unix))]
fn install_gpu_lost_restart() {}

fn main() {
    // Allocator knobs go first: mallopt decides arena policy lazily as
    // threads first contend, so this has to beat every thread spawn.
    tune_allocator();
    // The lost-GPU restart goes in before any renderer exists to need it.
    install_gpu_lost_restart();
    // The settings model can't reach up into the workspace files it has to
    // drain on a pre-split launch, so it gets pointed at them first, before
    // anything reads a setting.
    set_workspace_migrator(workspaces::migrate_saved);
    // The windows panels open are up here; the table goes in before any of
    // them can be reached.
    install_openers();
    // Files handed to us on the command line (`rox song.flac`, or the file
    // manager's Open With). Collected before the app boots so a plausible-file
    // filter runs off the real argv, not gpui's.
    let (launch_mode, launch_files) = rox_library::open_files::from_args();
    // One rox per data directory. A rox already running takes this launch
    // (its window comes back out of the tray with our files in hand), and
    // this process is done before it ever opens a compositor connection.
    let Some(instance) = startup::single_instance::claim(launch_mode, &launch_files) else {
        return;
    };
    let app = Application::new().with_assets(Assets);
    // macOS: clicking the dock icon while the app runs with no windows
    // brings a workspace back, the platform's own quit-to-tray. Only the
    // mac backend ever fires this.
    app.on_reopen(|cx| {
        if rox_panel_api::windows::front_workspace(cx).is_none() {
            integrations::tray::reopen(cx);
        }
    });
    app.run(move |cx: &mut App| {
        // The logging backend goes up first, so anything the rest of startup
        // reports is written to the file and the console ring from the first
        // line.
        logging::init();
        // The socket goes live next, so a launch that arrives mid-startup is
        // already queued for the drain rather than bouncing off a closed
        // door and starting its own rox.
        startup::single_instance::serve(instance, cx);
        // Whether this launch found a settings file decides the welcome
        // window later; recorded before anything can write one.
        note_first_run();
        // The shaders inside the shipped workspaces count as agreed to,
        // since they came with the binary. Seeded before any window opens,
        // or a shipped look's first frame paints its panels bare while the
        // gate waits for an approval nobody should have to give.
        workspaces::trust_shipped_shaders();
        // The backdrop layer's shade hook, wired before any window paints
        // a bake so the look's backdrop shader is there from frame one.
        workspace::install_backdrop_shade();
        gpui_component::init(cx);
        rox_panel_kit::ui::init(cx);
        rox_dock::init(cx);
        workspace::init(cx);
        tags::editor::init(cx);
        tags::rename::init(cx);
        tags::repair::init(cx);
        smart_playlist::init(cx);
        playlist_create::init(cx);
        bake_dialog::init(cx);
        convert_dialog::init(cx);
        lyrics::edit::init(cx);
        lyrics::matcher::init(cx);
        shader_editor::init(cx);
        cover::editor::init(cx);
        settings::shader_confirm::init(cx);
        rox_panel_api::panel_settings::init(cx);
        // Last of the inits, and it has to stay last: a rebind rebuilds the
        // whole keymap, and this is where the bindings already registered
        // above get snapshotted so they're preserved through one.
        keymap::init(cx);
        // Startup theme wiring runs through the palette pipeline, the same
        // choke point every later palette change goes through. The setters
        // set the dark baseline and supply the widget theme tokens.
        let settings = Settings::load();
        palette::set_palettes(settings.palette_dark(), settings.palette_light(), cx);
        seed_os_appearance(cx);
        set_theme(settings.theme, cx);
        // Language next to theme: same statics-outside-gpui shape, and
        // it has to happen before the first window title renders.
        set_language(settings.language.as_deref(), cx);
        palette::set_scalars(
            settings.look.bundle.appearance.surface_opacity,
            settings.look.bundle.appearance.backdrop_strength,
            cx,
        );
        palette::set_backdrop_all_windows(settings.look.bundle.appearance.backdrop_all_windows, cx);
        set_app_frame(settings.look.bundle.appearance.frame, cx);
        set_seams(settings.look.bundle.appearance.seams, cx);
        palette::set_keep_theme(settings.look.bundle.appearance.keep_theme, cx);
        palette::set_art_theming(settings.look.bundle.appearance.art_theming, cx);
        set_app_font(settings.look.bundle.appearance.app_font.clone(), cx);
        palette::set_app_font_size(settings.app_font_size, cx);
        set_rating_style(settings.look.bundle.appearance.rating_style, cx);
        set_rating_dots(settings.look.bundle.appearance.rating_dots, cx);
        set_hide_menubar(settings.look.bundle.appearance.hide_menubar, cx);
        set_os_decorations(settings.look.bundle.appearance.os_decorations);
        set_resize_border(settings.look.bundle.appearance.resize_border);
        set_fold_case(settings.fold_case);
        set_show_readings(settings.show_readings, cx);
        rox_library::genre::set_split_compounds(settings.split_genre_compounds);
        set_quit_to_tray(settings.quit_to_tray);
        set_design_mode(settings.design_mode, cx);
        set_resize_lock(settings.resize_lock, cx);
        set_experimental(settings.experimental, cx);
        set_acoustic_analysis(settings.acoustic_analysis, cx);
        set_tempo_analysis(settings.tempo_analysis, cx);
        set_gain_mode(settings.replay_gain.mode, cx);
        set_acoustic_model(&settings.acoustic_model, cx);
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
        // Sweep what a past update left behind: the rename-aside old exe
        // Windows couldn't delete, a stranded stage. Inline rather than
        // spawned, because the check below may start a download whose
        // staging this sweep must not race; it's a handful of removes.
        startup::updater::clean_leftovers();
        // The daily update check, off the UI thread; the toggle and the
        // one-day cache both gate it, so most launches do nothing here.
        // Opted in, a hit rolls straight into the updater's download.
        startup::updates::check_on_launch(cx);
        // Launch files go into the first window; a plain launch (no files)
        // opens on the restored state as before.
        let open = (!launch_files.is_empty()).then_some((launch_mode, launch_files));
        open_workspace_window(workspace::WorkspaceStart::Restore, None, open, cx);
        // The macOS system menu bar, once a workspace exists for its picks to
        // act on. A no-op on every other platform, where the in-window bar
        // is the only menu.
        workspace::native_menu::rebuild(cx);
        cx.activate(true);
    });
}
