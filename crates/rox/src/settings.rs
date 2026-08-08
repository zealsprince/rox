//! The settings model itself lives in [`rox_core::settings`] and is
//! re-exported whole below, so every `crate::settings::` path in the app
//! reads the same as it always did. What stays here is the settings window
//! and its chrome.

pub mod shader_confirm;
pub mod ui;
pub mod window;

pub(crate) use rox_core::settings::*;

// The live model pick sits with the services that read it: the catalog's
// coverage count and the player's similarity draws both ask on paths that
// have left the binary. The settings pages still reach it from here.
pub(crate) use rox_services::acoustic::{acoustic_ml_source, acoustic_source, set_acoustic_model};
