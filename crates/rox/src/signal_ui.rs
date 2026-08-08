//! The signal-pool and route-binding UI moved to rox-panel-api with the
//! rest of the panel seam; this keeps `crate::signal_ui::` reading the way
//! it always did.

pub use rox_panel_api::signal_ui::*;
