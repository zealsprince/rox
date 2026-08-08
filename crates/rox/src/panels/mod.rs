//! The panels that still belong to the binary, plus the door onto the rest.
//!
//! Most panels render out of rox-panels now and are re-exported below, so
//! every `crate::panels::` path in the app still lands. What stays here are
//! the ones that call into [`crate::workspace::Workspace`] for real - the
//! drawer, the group and overlay hosts, the slide and mini frames, the
//! menubar, the window controls, the queue widget - since the workspace is
//! the binary.

pub mod drawer;
pub mod group;
pub mod menu;
pub mod mini;
pub mod overlay;
pub mod queue_widget;
pub mod slide;
pub mod window_controls;

pub use rox_panels::*;
