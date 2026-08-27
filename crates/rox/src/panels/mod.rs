//! The panels that still belong to the binary.
//!
//! Most panels render out of [`rox_panels`] now. The ones that stay here
//! call into [`crate::workspace::Workspace`] for real, and the workspace is
//! the binary: the drawer, the group and overlay hosts, the slide and mini
//! frames, the menubar, the window controls, and the queue widget.

pub mod drawer;
pub mod group;
pub mod menu;
pub mod mini;
pub mod overlay;
pub mod queue_widget;
pub mod slide;
pub mod window_controls;
