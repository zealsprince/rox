//! OS integration surfaces: media keys, the control socket and its debug
//! drive, the broadcast sink, the tray, taskbar progress, and window
//! placement.

pub mod broadcast;
pub mod drive;
pub mod ipc;
mod ipc_plugins;
#[cfg(target_os = "linux")]
mod kwin;
pub mod media_controls;
pub mod placement;
pub mod taskbar;
pub mod tray;
