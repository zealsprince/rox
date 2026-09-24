//! The panels that stay in the binary because they call into it: the custom
//! controls through [`crate::keymap`], the rest through
//! [`crate::workspace::Workspace`].

pub mod controls;
pub mod drawer;
pub mod group;
pub mod menu;
pub mod mini;
pub mod overlay;
pub mod queue_widget;
pub mod slide;
pub mod window_controls;
