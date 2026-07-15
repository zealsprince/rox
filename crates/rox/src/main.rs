//! The rox app shell: the workspace's dock hosts the library panel over the
//! promoted library service, the audio views (spectrum, waveform) fed from
//! the player's PCM tap, and the transport panels (playback controls, seek
//! strip, volume) over the promoted playback engine in the bottom dock.
//! Panels duplicate with their own config and pop out into OS windows over
//! the same shared entities. New Window stays in the menubar so
//! multi-window on Wayland keeps getting exercised.

mod assets;
mod backdrop;
mod design;
mod panel;
mod panels;
mod peaks;
mod player;
mod selection;
mod settings;
mod settings_window;
mod source;
mod workspace;

use gpui::{
    point, px, size, App, AppContext, Application, Bounds, SharedString, TitlebarOptions,
    WindowBounds, WindowOptions,
};
use gpui_component::Root;

use assets::Assets;
use design::palette;
use settings::Settings;
use workspace::Workspace;

pub fn open_workspace(cx: &mut App) {
    // Windows open on the saved frame, so a restart, and every New Window,
    // comes back where the last-closed window sat.
    let window_bounds = match Settings::load().window {
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
    let options = WindowOptions {
        window_bounds: Some(window_bounds),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("rox")),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        // The Wayland backend ignores the creation-time titlebar title;
        // only set_window_title reaches the compositor.
        window.set_window_title("rox");
        let workspace = cx.new(|cx| Workspace::new(window, cx));
        // gpui-component windows layer sheets, dialogs, and dock drag
        // overlays through a Root at the top of the window.
        cx.new(|cx| Root::new(workspace, window, cx))
    })
    .expect("failed to open the main window");
}

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        gpui_component::init(cx);
        rox_dock::init(cx);
        workspace::init(cx);
        // Startup theme wiring runs through the palette pipeline - the
        // same choke point every later palette change goes through. The
        // setters set the dark baseline and feed the widget theme tokens.
        let settings = Settings::load();
        palette::set(settings.palette(), cx);
        palette::set_scalars(settings.surface_opacity, settings.backdrop_strength, cx);
        palette::set_art_theming(settings.art_theming, cx);
        open_workspace(cx);
        cx.activate(true);
    });
}
