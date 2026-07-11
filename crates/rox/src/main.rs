//! The rox app shell. First light proved GPUI renders on this machine; the
//! shell now hosts the research prototypes behind a menubar so plain
//! `cargo run` reaches everything without --package. New Window stays in the
//! menubar so multi-window on Wayland keeps getting exercised.

mod playback;
mod workspace;

use gpui::{
    px, size, App, AppContext, Application, Bounds, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions,
};

use workspace::Workspace;

pub fn open_workspace(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(960.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("rox")),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(options, |_, cx| cx.new(|_| Workspace::new()))
        .expect("failed to open the main window");
}

fn main() {
    Application::new().run(|cx: &mut App| {
        open_workspace(cx);
        cx.activate(true);
    });
}
