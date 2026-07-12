//! The rox app shell: the workspace's dock hosts the library panel over the
//! promoted library service, the audio views (spectrum, waveform) fed from
//! the player's PCM tap, and the transport panels (playback controls, seek
//! strip, volume) over the promoted playback engine in the bottom dock.
//! Panels duplicate with their own config and pop out into OS windows over
//! the same shared entities. New Window stays in the menubar so
//! multi-window on Wayland keeps getting exercised.

mod library;
mod panel;
mod player;
mod settings;
mod spectrum;
mod transport;
mod waveform;
mod workspace;

use gpui::{
    px, size, App, AppContext, Application, Bounds, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions,
};
use gpui_component::{Root, Theme, ThemeMode};
use gpui_component_assets::Assets;

use workspace::Workspace;

pub fn open_workspace(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1100.), px(700.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
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
        workspace::init(cx);
        // The widget baseline follows the app's existing dark palette;
        // themes as shareable token sets come later.
        Theme::change(ThemeMode::Dark, None, cx);
        open_workspace(cx);
        cx.activate(true);
    });
}
