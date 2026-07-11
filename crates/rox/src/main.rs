//! The rox app shell: the library panel over the promoted library service,
//! the player bar over the promoted playback engine, and a menubar that still
//! reaches the remaining research prototypes. New Window stays in the menubar
//! so multi-window on Wayland keeps getting exercised.

mod library;
mod player;
mod workspace;

use gpui::{
    px, size, App, AppContext, Application, Bounds, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions,
};

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
    cx.open_window(options, |_, cx| cx.new(Workspace::new))
        .expect("failed to open the main window");
}

fn main() {
    Application::new().run(|cx: &mut App| {
        open_workspace(cx);
        cx.activate(true);
    });
}
