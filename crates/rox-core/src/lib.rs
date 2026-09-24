//! The app's data floor: the settings file and the small services under the
//! rest of rox. Nothing here draws, and nothing depends on the app above it.

pub mod acoustic;
pub mod continuation;
pub mod fmt;
pub mod install;
pub mod logging;
pub mod pace;
pub mod pattern;
pub mod settings;

/// The Wayland/X11 app id, set on every window. Without a shared id, Wayland
/// ignores xdg-activation from one window to raise another.
pub const APP_ID: &str = "rox";

/// How many tracks any play-from-a-list queues behind the clicked row.
pub const QUEUE_CAP: usize = 1000;

/// How many tracks a shuffled play samples from across the whole view. Sampled
/// rather than sliced, or a big library only ever shuffles its first artists.
pub const SHUFFLE_SEED: usize = 100;
