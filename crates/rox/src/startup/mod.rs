//! Launch-time surfaces: the update check and the updater it can roll
//! into, the first-run welcome window, the about window, the AppImage's
//! menu entry, and the guard that keeps a second launch from becoming a
//! second rox.

pub mod about_window;
pub mod desktop_integration;
// The registry half is Windows-only; the key list is tested everywhere.
#[cfg(any(windows, test))]
pub mod file_types;
pub mod single_instance;
pub mod updater;
pub mod updates;
pub mod welcome_window;
