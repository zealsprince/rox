//! The seam panels compile against. Everything a panel needs that isn't a
//! widget: the shared state it renders over, the frame config it carries,
//! the chrome helpers its menus are built from, the settings window behind
//! its gear, and the shared surfaces (track rows, group heads, the query,
//! the signal routes editor) that more than one panel draws.
//!
//! The rule that keeps this crate a seam: nothing in here knows a concrete
//! panel type, and nothing in here reaches up into the binary directly. The
//! windows a panel opens (the tag editor, the stats page, the signals
//! window) live up in the app, so the calls go through [`openers`], a table
//! of function pointers the binary installs once at startup.

pub mod actions;
pub mod charts;
pub mod group_head;
pub mod openers;
pub mod panel;
pub mod panel_settings;
pub mod query;
pub mod rating_ui;
pub mod signal_ui;
pub mod source;
pub mod suggest;
pub mod track_ui;
pub mod windows;
