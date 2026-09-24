//! The rox app shell: boots settings, theme, and integrations in order,
//! then opens workspace windows.

// Release builds use the GUI subsystem so Windows opens no console next to the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autoeq_window;
mod backdrop_visual;
mod bake;
mod bake_dialog;
mod bookmark_dialog;
mod cjk_fonts;
mod composite;
mod console_window;
mod convert;
mod convert_dialog;
mod cover;
mod duplicates;
mod embeddings;
mod eq_presets;
mod eq_window;
mod genre_tagger;
mod goto_dialog;
mod health_window;
mod integrations;
mod keymap;
mod lastfm;
mod lyrics;
mod matching;
mod milkdrop_picker;
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
mod station_directory;
mod stats_window;
mod tags;
mod tasks_window;
mod tempo_job;
mod workspace;
mod workspaces;

use gpui::{
    App, AppContext, Application, Bounds, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions, point, px, size,
};
use gpui_component::Root;

use rox_core::settings::{
    MIN_WINDOW_SIZE, Settings, layouts, note_first_run, note_os_appearance, os_decorations,
    resize_border, seed_os_appearance, set_acoustic_analysis, set_app_font, set_app_frame,
    set_bare_child_windows, set_child_titlebar, set_chrome_side, set_chrome_style, set_design_mode,
    set_experimental, set_fold_case, set_gain_mode, set_hide_menubar, set_language,
    set_menubar_buttons, set_os_decorations, set_quit_to_tray, set_rating_dots, set_rating_style,
    set_resize_border, set_resize_lock, set_seams, set_show_readings, set_tempo_analysis,
    set_theme, set_workspace_migrator, window_decorations,
};
use rox_core::{APP_ID, logging};
use rox_design::assets::Assets;
use rox_design::palette;
use rox_net::providers;
use rox_services::acoustic::set_acoustic_model;
use workspace::Workspace;

/// `rox --window-size 1440x900`: a dev flag pinning every window's size for
/// the session, for shooting previews. Wayland gives nothing outside the
/// process a way to size it.
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

pub fn open_workspace_with(start: workspace::WorkspaceStart, cx: &mut App) {
    open_workspace_window(start, None, None, cx);
}

/// Reopen from the tray or dock over the held state, so playback continues.
pub fn open_workspace_adopting(adopt: workspace::Adopted, cx: &mut App) {
    open_workspace_window(workspace::WorkspaceStart::Restore, Some(adopt), None, cx);
}

fn open_workspace_window(
    start: workspace::WorkspaceStart,
    adopt: Option<workspace::Adopted>,
    // Command-line files and their launch mode. None on every other open.
    open: Option<(rox_library::open_files::LaunchMode, Vec<std::path::PathBuf>)>,
    cx: &mut App,
) {
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
        // A hair larger than the first-run welcome window (1240x660), and still
        // fits a 1366x768 laptop.
        None => WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(720.)), cx)),
    };
    if let workspace::WorkspaceStart::Preset(name) = &start
        && let Some(s) = layouts::resolve(&Settings::load(), name).and_then(|p| p.size)
    {
        window_bounds = WindowBounds::Windowed(Bounds {
            origin: window_bounds.get_bounds().origin,
            size: size(
                px(s.width).max(MIN_WINDOW_SIZE.width),
                px(s.height).max(MIN_WINDOW_SIZE.height),
            ),
        });
    }
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
            // Windows and macOS read the caption from this creation-time flag, not
            // window_decorations (a no-op there until request_decorations runs), so
            // opening right avoids a chrome flash. Linux ignores it.
            appears_transparent: cfg!(any(target_os = "windows", target_os = "macos"))
                && !os_decorations(),
            ..Default::default()
        }),
        app_id: Some(APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        // Wayland ignores the creation-time title.
        rox_panel_api::windows::set_window_title(window, "rox");
        // Some window managers grant the map and deny the raise, leaving focus on
        // the window that opened this one.
        window.activate_window();
        window.set_resize_border(resize_border());
        // OS appearance only reaches us through a window. The platform's own read
        // borrows the Wayland client, which is already borrowed here, so use the
        // window's cached value. The immediate note covers a flip while windowless.
        note_os_appearance(window.appearance(), cx);
        window
            .observe_window_appearance(|window, cx| {
                note_os_appearance(window.appearance(), cx);
            })
            .detach();
        let workspace = cx.new(|cx| Workspace::new(start, adopt, window, cx));
        // The player is path-based, so this works for files outside the library.
        if let Some((mode, paths)) = open {
            workspace.update(cx, |ws, cx| ws.open_paths(mode, paths, cx));
        }
        cx.new(|cx| Root::new(workspace, window, cx))
    })
    .expect("failed to open the main window");
}

/// Panels are a crate down and can't depend upward, so every window they open
/// goes through this table. Installed before any window can open.
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
        bookmark_new: bookmark_dialog::open_new,
        bookmark_edit: bookmark_dialog::open_edit,
        smart_playlist: smart_playlist::open,
        eq_window: eq_window::open,
        stats_window: stats_window::open,
        health_window: health_window::open,
        station_directory: station_directory::open,
        signals_window: signals_window::open,
        shader_editor: shader_editor::open,
        milkdrop_picker: milkdrop_picker::open,
        console_notice: console_window::notice,
        lyrics_watch: watch_lyrics_panel,
        lyrics_edit: lyrics::edit::open,
        lyrics_matcher: lyrics::matcher::open,
        lyrics_saved: lyrics::saved,
        lyrics_preview: lyrics::preview,
        add_panel_submenu: workspace::add_panel_submenu,
        host_settings_item: composite::host_settings_item,
        confirm_close_locked,
    });
}

/// The registry can't name a concrete panel, so it arrives type-erased.
fn watch_lyrics_panel(panel: gpui::AnyWeakEntity, cx: &mut App) {
    let Some(panel) = panel
        .upgrade()
        .and_then(|panel| panel.downcast::<rox_panels::lyrics::LyricsPanel>().ok())
    else {
        return;
    };
    lyrics::watch(panel.downgrade(), cx);
}

/// A window with no workspace behind it has nowhere for the dialog; the pin holds.
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

/// Rein in glibc's malloc before the thread pools exist. The default arena
/// per contending thread left a dozen arenas holding freed heap, about 50 MB
/// idle, and made workspace applies look like a leak. Four arenas and 1 MB
/// trim/mmap thresholds measured idle 234 -> 182 MB, and thirty applies
/// flatten at ~230 MB instead of climbing ~8 MB each. glibc only.
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

/// A suspend/resume can lose the Vulkan device (#118: NVIDIA on X11). The
/// renderer detects it (vendored gpui patch z3) and this re-execs in place:
/// same pid and argv, and the launch restore brings the session back. The
/// stale single-instance socket is treated as dead and rebound.
///
/// The uptime gate stops a GPU that's dead at boot from becoming an exec loop.
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
        log::error!("restart failed: {err}");
    });
}

#[cfg(not(unix))]
fn install_gpu_lost_restart() {}

fn main() {
    // First: mallopt decides arena policy as threads first contend.
    tune_allocator();
    install_gpu_lost_restart();
    // Before anything reads a setting: the settings model can't reach up into
    // the workspace files a pre-split launch has to drain.
    set_workspace_migrator(workspaces::migrate_saved);
    install_openers();
    // Before the app boots, so the file filter reads the real argv, not gpui's.
    let (launch_mode, launch_files) = rox_library::open_files::from_args();
    // One rox per data directory: a running one takes this launch, and this
    // process exits before opening a compositor connection.
    let Some(instance) = startup::single_instance::claim(launch_mode, &launch_files) else {
        return;
    };
    let app = Application::new().with_assets(Assets);
    // macOS only: a dock click with no windows brings a workspace back.
    app.on_reopen(|cx| {
        if rox_panel_api::windows::front_workspace(cx).is_none() {
            integrations::tray::reopen(cx);
        }
    });
    app.run(move |cx: &mut App| {
        logging::init();
        // Early, so a launch arriving mid-startup queues instead of starting its own rox.
        startup::single_instance::serve(instance, cx);
        // Recorded before anything can write a settings file.
        note_first_run();
        // Before any window opens, or a shipped look's first frame paints bare while
        // its shaders wait for approval.
        workspaces::trust_shipped_shaders();
        workspace::install_backdrop_shade();
        // The backdrop's engine lives in a never-dropped static, so without this its
        // thread keeps issuing GL calls while glibc runs Mesa's exit handlers. Stop
        // now and wait in the future, so the teardowns overlap with the panels'.
        cx.on_app_quit(|_| {
            let engine = backdrop_visual::take_engine();
            let at = std::time::Instant::now();
            if let Some(engine) = engine.as_ref() {
                engine.stop();
            }
            async move {
                let Some(engine) = engine else {
                    return;
                };
                if engine.wait() {
                    log::info!(
                        "backdrop visual: engine stood down {} us after the hang-up",
                        at.elapsed().as_micros()
                    );
                } else {
                    log::warn!("backdrop visual: engine still up at quit");
                }
            }
        })
        .detach();
        gpui_component::init(cx);
        rox_panel_kit::ui::init(cx);
        rox_dock::init(cx);
        workspace::init(cx);
        // After the widget library's init, whose bindings it snapshots as the bottom layer.
        keymap::init(cx);
        let settings = Settings::load();
        palette::set_palettes(settings.palette_dark(), settings.palette_light(), cx);
        seed_os_appearance(cx);
        set_theme(settings.theme, cx);
        // Before the first window title renders.
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
        set_menubar_buttons(settings.look.bundle.appearance.menubar_buttons, cx);
        set_os_decorations(settings.look.bundle.appearance.os_decorations);
        set_bare_child_windows(settings.look.bundle.appearance.bare_child_windows);
        set_child_titlebar(settings.look.bundle.appearance.child_titlebar);
        set_chrome_style(settings.look.bundle.appearance.chrome_style);
        set_chrome_side(settings.look.bundle.appearance.chrome_side);
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
        providers::set_lyrics_online(settings.accounts.providers.lrclib);
        providers::set_metadata_online(settings.accounts.providers.musicbrainz);
        providers::set_acoustid_online(settings.accounts.providers.acoustid);
        providers::set_itunes_online(settings.accounts.providers.itunes);
        providers::set_deezer_online(settings.accounts.providers.deezer);
        providers::set_lastfm_art_online(settings.accounts.providers.lastfm_art);
        providers::set_artist_online(settings.accounts.providers.artist);
        rox_services::sources::install_registry();
        // Repoint the menu entry of an AppImage that moved.
        startup::desktop_integration::heal();
        // Inline, not spawned: the update check below may start a download whose
        // staging this sweep must not race.
        startup::updater::clean_leftovers();
        startup::updates::check_on_launch(cx);
        let open = (!launch_files.is_empty()).then_some((launch_mode, launch_files));
        open_workspace_window(workspace::WorkspaceStart::Restore, None, open, cx);
        // The macOS menu bar needs a workspace to act on. A no-op elsewhere.
        workspace::native_menu::rebuild(cx);
        cx.activate(true);
    });
}
