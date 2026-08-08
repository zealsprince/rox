//! OS integration surfaces: the MPRIS/media-key controls, the system tray
//! for windowless residency, the taskbar button's progress bar, and the
//! Discord presence.

pub mod media_controls;
pub mod taskbar;
pub mod tray;

// The presence entity sits with the player and the catalog it watches; the
// app still reaches it through the path it always did.
pub use rox_services::discord_presence as discord;
