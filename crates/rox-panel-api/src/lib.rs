//! The seam panels compile against: the shared state they render over, the
//! frame config, the chrome helpers, the settings window behind the gear,
//! and the surfaces more than one panel draws (track rows, group heads, the
//! query, the signal routes editor).
//!
//! Nothing in here knows a concrete panel type or calls up into the binary
//! directly. Windows defined in the app go through [`openers`], a table of
//! function pointers the binary installs at startup.

pub mod actions;
pub mod bookmark_ui;
pub mod buttons;
pub mod charts;
pub mod cue_ui;
pub mod fallback_chrome;
pub mod group_head;
pub mod openers;
pub mod panel;
pub mod panel_settings;
pub mod position_bound;
pub mod preset_browser;
pub mod query;
pub mod rating_ui;
pub mod signal_ui;
pub mod source;
pub mod suggest;
pub mod track_ui;
pub mod windows;
