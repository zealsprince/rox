//! The app's data floor: the settings file and the small services that sit
//! under everything else in rox. Nothing here draws anything, and nothing
//! here reaches back up into the app, so the UI crate can rebuild without
//! taking the settings model with it.

pub mod acoustic;
pub mod continuation;
pub mod fmt;
pub mod logging;
pub mod pace;
pub mod settings;

/// The Wayland/X11 app id, set on every window we open. Windows share it so
/// the compositor groups them as one app and, on Wayland, will consider an
/// xdg-activation request from one window to raise another (bringing an
/// already-open settings or customize window to the front). Without it the
/// backend's activate is a no-op.
pub const APP_ID: &str = "rox";

/// Play from a double-clicked row: at most this many tracks are queued
/// behind it. Every surface that plays out of a list caps the same way,
/// the quick-play modal and the stats window included.
pub const QUEUE_CAP: usize = 1000;
