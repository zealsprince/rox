//! The continuation mode the settings file stores. It's defined with the
//! strategies it names, in [`rox_playback::continuation`], since those need
//! the library and the player to do any of it; the settings model just
//! holds the pick.

pub use rox_playback::continuation::Mode;
