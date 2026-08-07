//! The settings-window chrome moved to rox-panel-kit with the rest of the
//! widget layer. This keeps crate::settings::ui pointing at it, so the
//! pages that build on it path through here the way they always have.

pub use rox_panel_kit::ui::*;
