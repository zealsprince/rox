//! Launch-time surfaces: the update check and the updater it can roll
//! into, the first-run welcome window, the about window, icon pack
//! activation, and the guard that keeps a second launch from becoming a
//! second rox.

pub mod about_window;
pub mod icon_packs;
pub mod single_instance;
pub mod updater;
pub mod updates;
pub mod welcome_window;
