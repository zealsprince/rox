//! Named dock layouts the user saved into settings. Each is a full dock dump
//! under a name; the settings window lists them, and the mini-player button
//! toggles between the two a user picks as primary and mini. Shareable presets
//! live one level up, bundled into workspaces.

use serde_json::Value;

use crate::settings::{LayoutSize, Settings};

/// A layout preset for the settings list: its name, the dock dump to apply,
/// and an optional window size to restore with it.
pub struct Preset {
    pub name: String,
    pub dump: Value,
    pub size: Option<LayoutSize>,
}

/// Every saved preset for the settings list, in save order.
pub fn all(settings: &Settings) -> Vec<Preset> {
    settings
        .layouts
        .iter()
        .map(|saved| Preset {
            name: saved.name.clone(),
            dump: saved.dump.clone(),
            size: saved.size,
        })
        .collect()
}

/// Resolve a preset name to its dump and size. None when nothing carries that
/// name.
pub fn resolve(settings: &Settings, name: &str) -> Option<Preset> {
    settings
        .layouts
        .iter()
        .find(|l| l.name == name)
        .map(|saved| Preset {
            name: saved.name.clone(),
            dump: saved.dump.clone(),
            size: saved.size,
        })
}
