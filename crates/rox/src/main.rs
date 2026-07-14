//! The rox app shell: the workspace's dock hosts the library panel over the
//! promoted library service, the audio views (spectrum, waveform) fed from
//! the player's PCM tap, and the transport panels (playback controls, seek
//! strip, volume) over the promoted playback engine in the bottom dock.
//! Panels duplicate with their own config and pop out into OS windows over
//! the same shared entities. New Window stays in the menubar so
//! multi-window on Wayland keeps getting exercised.

mod assets;
mod backdrop;
mod palette;
mod panel;
mod panels;
mod peaks;
mod player;
mod selection;
mod settings;
mod source;
mod workspace;

use gpui::{
    point, px, size, App, AppContext, Application, Bounds, SharedString, TitlebarOptions,
    WindowBounds, WindowOptions,
};
use gpui_component::{Root, Theme, ThemeMode};

use assets::Assets;
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
        // The widget baseline follows the app's existing dark palette;
        // themes as shareable token sets come later. Selection follows
        // the accent instead of the stock blue; only tokens widgets
        // actually draw today are fed from the palette.
        Theme::change(ThemeMode::Dark, None, cx);
        let theme = Theme::global_mut(cx);
        theme.table_active = palette::alpha(palette::accent(), 0x26).into();
        theme.table_active_border = palette::accent().into();
        theme.list_active = palette::alpha(palette::accent(), 0x26).into();
        theme.list_active_border = palette::accent().into();
        open_workspace(cx);
        cx.activate(true);
    });
}
