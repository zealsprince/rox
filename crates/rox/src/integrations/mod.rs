//! OS integration surfaces: the MPRIS/media-key controls, the control
//! socket, the system tray for windowless residency, the taskbar button's
//! progress bar, and the Discord presence.

pub mod ipc;
pub mod media_controls;
pub mod taskbar;
pub mod tray;
