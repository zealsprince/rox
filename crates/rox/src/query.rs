//! The search box and the shared query moved to rox-panel-api; this keeps
//! `crate::query::` pointing at them.

pub use rox_panel_api::query::*;
