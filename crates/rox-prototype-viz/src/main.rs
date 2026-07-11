//! Standalone runner. The same window opens from the rox menubar; this
//! binary keeps `cargo run -p rox-prototype-viz` working on its own.

use gpui::{App, Application};

fn main() {
    Application::new().run(|cx: &mut App| {
        rox_prototype_viz::open_window(cx);
        cx.activate(true);
    });
}
