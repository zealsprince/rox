//! OS integration surfaces: media keys, the control socket and its debug
//! drive and frame timer, the broadcast sink, the tray, taskbar progress,
//! and window placement.

pub mod broadcast;
pub mod drive;
pub(crate) mod frames;
pub mod ipc;
#[cfg(debug_assertions)]
mod ipc_catalog;
mod ipc_documents;
mod ipc_plugins;
#[cfg(all(debug_assertions, target_os = "linux"))]
mod ipc_renderer;
mod ipc_settings;
mod ipc_track;
#[cfg(target_os = "linux")]
mod kwin;
pub mod media_controls;
pub mod placement;
pub mod taskbar;
pub mod tray;
