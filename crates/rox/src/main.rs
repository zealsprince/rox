//! The rox app shell: the workspace's dock hosts the library panel over the
//! promoted library service, the audio views (spectrum, waveform) fed from
//! the player's PCM tap, and the transport panels (playback controls, seek
//! strip, volume) over the promoted playback engine in the bottom dock.
//! Panels duplicate with their own config and pop out into OS windows over
//! the same shared entities. New Window stays in the menubar so
//! multi-window on Wayland keeps getting exercised.

mod artists;
mod assets;
mod backdrop;
mod catalog;
mod charts;
mod composite;
mod cover_editor;
mod cover_match;
mod design;
mod history;
mod icon_packs;
mod lastfm;
mod layouts;
mod lyrics_edit;
mod lyrics_match;
mod m3u;
mod media_controls;
mod panel;
mod panel_settings;
mod panels;
mod peaks;
mod player;
mod playlist_create;
mod providers;
mod quick_play;
mod rating_ui;
mod search;
mod selection;
mod settings;
mod settings_ui;
mod settings_window;
mod shared_query;
mod source;
mod stats_window;
mod suggest;
mod tag_editor;
mod tag_match;
mod thumbs;
mod tray;
mod welcome_window;
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

pub fn open_workspace(cx: &mut App) {
    open_workspace_with(workspace::WorkspaceStart::Restore, cx);
}

/// Open a workspace window with a chosen starting layout: the Window menu's
/// New Window (restore), Empty Window, and New Window from Layout all land
/// here.
pub fn open_workspace_with(start: workspace::WorkspaceStart, cx: &mut App) {
    open_workspace_window(start, None, cx);
}

/// Reopen from the tray or the macOS dock: a window on the saved working
/// layout over the state the last close handed to the hold, so playback
/// carries straight over.
pub fn open_workspace_adopting(state: panel::AppState, cx: &mut App) {
    open_workspace_window(workspace::WorkspaceStart::Restore, Some(state), cx);
}

fn open_workspace_window(
    start: workspace::WorkspaceStart,
    adopt: Option<panel::AppState>,
    cx: &mut App,
) {
    // Windows open on the saved frame, so a restart, and every New Window,
    // comes back where the last-closed window sat.
    let mut window_bounds = match Settings::load().window {
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
        None => WindowBounds::Windowed(Bounds::centered(None, size(px(1100.), px(700.)), cx)),
    };
    // A preset window opens at the preset's stored size when it carries one,
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
    let options = WindowOptions {
        window_bounds: Some(window_bounds),
        window_min_size: Some(MIN_WINDOW_SIZE),
        window_decorations: Some(settings::window_decorations()),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("rox")),
            ..Default::default()
        }),
        app_id: Some(APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        // The Wayland backend ignores the creation-time titlebar title;
        // only set_window_title reaches the compositor.
        window.set_window_title("rox");
        let workspace = cx.new(|cx| Workspace::new(start, adopt, window, cx));
        // gpui-component windows layer sheets, dialogs, and dock drag
        // overlays through a Root at the top of the window.
        cx.new(|cx| Root::new(workspace, window, cx))
    })
    .expect("failed to open the main window");
}

fn main() {
    let app = Application::new().with_assets(Assets);
    // macOS: clicking the dock icon while the app runs with no windows
    // brings a workspace back - the platform's own quit-to-tray. Only the
    // mac backend ever fires this.
    app.on_reopen(|cx| {
        if workspace::front_workspace(cx).is_none() {
            tray::reopen(cx);
        }
    });
    app.run(|cx: &mut App| {
        // Whether this launch found a settings file decides the welcome
        // window later; recorded before anything can write one.
        settings::note_first_run();
        gpui_component::init(cx);
        rox_dock::init(cx);
        workspace::init(cx);
        tag_editor::init(cx);
        // Startup theme wiring runs through the palette pipeline - the
        // same choke point every later palette change goes through. The
        // setters set the dark baseline and feed the widget theme tokens.
        let settings = Settings::load();
        palette::set(settings.palette(), cx);
        palette::set_scalars(settings.surface_opacity, settings.backdrop_strength, cx);
        palette::set_art_theming(settings.art_theming, cx);
        settings::set_app_font(settings.app_font.clone(), cx);
        settings::set_rating_style(settings.rating_style, cx);
        settings::set_hide_menubar(settings.hide_menubar, cx);
        settings::set_os_decorations(settings.os_decorations);
        settings::set_quit_to_tray(settings.quit_to_tray);
        tray::sync(cx);
        // Point the icon resolver at the chosen pack before any window
        // opens, so the first frame already draws it.
        icon_packs::activate(settings.icon_pack.as_deref());
        providers::set_lyrics_online(settings.providers.lrclib);
        providers::set_metadata_online(settings.providers.musicbrainz);
        providers::set_itunes_online(settings.providers.itunes);
        providers::set_deezer_online(settings.providers.deezer);
        providers::set_artist_online(settings.providers.artist);
        open_workspace(cx);
        cx.activate(true);
    });
}
