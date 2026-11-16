//! A canvas's saved shape and where its items land on screen (ADR 33). The
//! types are what a canvas panel stores in its config. The math places each
//! item through its parents' frames and a camera, orders the items back to
//! front, and maps a pointer drag back into an item's parent space.
//!
//! Drawing stays out, and so do panels, assets' bytes and signals. Those
//! belong to the app, which hands this crate a viewport, the pointer and
//! each item's live route offsets, and paints what comes back.

mod mat;
mod project;
mod state;

pub use mat::Mat4;
pub use project::{
    Frame, Live, Projected, Viewport, child_frame, inside, paint_order, project, unproject_delta,
};
pub use state::{
    Align, Anchor, CANVAS_VERSION, Camera, CanvasItem, CanvasState, Content, Cost, Fit, ImageFit,
    Length, MAX_DEPTH, Place, Stage, Sway,
};
