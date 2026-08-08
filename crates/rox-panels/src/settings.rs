//! The settings the panels read, at the path they always read it. The model
//! and the accessors live in rox-core, the settings-window chrome the panel
//! settings pages build on lives in rox-panel-kit, and the resolved acoustic
//! source comes off rox-services.

pub(crate) use rox_core::settings::*;
pub(crate) use rox_panel_kit::ui;
pub(crate) use rox_services::acoustic::acoustic_source;
