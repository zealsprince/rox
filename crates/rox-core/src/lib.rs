//! The app's data floor: the settings file and the small services that sit
//! under everything else in rox. Nothing here draws anything, and nothing
//! here reaches back up into the app, so the UI crate can rebuild without
//! taking the settings model with it.

pub mod acoustic;
pub mod continuation;
pub mod logging;
pub mod pace;
pub mod settings;
