//! Research prototype for ADR 8: can a curl-noise flow field driven by
//! spectrum bands hit the reference look inside a sane frame budget with
//! CPU-side rendering, and does it draw better as GPUI polylines or as a
//! per-frame image blit?
//!
//! Runs standalone via the crate's binary, or embedded in the rox app via
//! [`open_window`]. Run with --release either way, the sim and the
//! rasterizer are an order of magnitude off in debug.

pub mod noise;
pub mod sim;
mod view;

pub use view::VizProto;

use std::sync::Arc;

use gpui::{
    px, size, App, AppContext, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions,
};

/// Spawn the sim thread and open the prototype window. The sim thread exits
/// when the window's view drops.
pub fn open_window(cx: &mut App) {
    let shared = Arc::new(sim::Shared::new());
    sim::spawn(shared.clone());

    let bounds = Bounds::centered(None, size(px(1280.), px(720.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("rox prototype: viz")),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(options, |_, cx| cx.new(|_| VizProto::new(shared)))
        .expect("failed to open the viz window");
}
