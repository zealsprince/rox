//! OS integration surfaces: the MPRIS/media-key controls, the system
//! tray for windowless residency, the taskbar button's progress bar, and
//! the filesystem watcher for the library roots.

pub mod discord;
pub mod media_controls;
pub mod taskbar;
pub mod tray;

// The root watcher sits with the library now; the catalog still reaches it
// through the path it always did.
pub use rox_library::watch as library_watch;
