//! The panel layer moved to rox-panel-api, the seam the panels compile
//! against. This keeps every `crate::panel::` path in the app pointing at
//! it, the widget-layer re-exports included, so nothing that reads through
//! here had to change.

pub use rox_panel_api::panel::*;
