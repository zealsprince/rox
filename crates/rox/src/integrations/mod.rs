//! OS integration surfaces: the MPRIS/media-key controls, the system
//! tray for windowless residency, and the filesystem watcher for the
//! library roots.

pub mod library_watch;
pub mod media_controls;
pub mod tray;
