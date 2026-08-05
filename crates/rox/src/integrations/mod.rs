//! OS integration surfaces: the MPRIS/media-key controls, the system
//! tray for windowless residency, the taskbar button's progress bar, and
//! the filesystem watcher for the library roots.

pub mod discord;
pub mod library_watch;
pub mod media_controls;
pub mod taskbar;
pub mod tray;
