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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::NamedLayout;

    fn settings_with_presets() -> Settings {
        let mut s = Settings::default();
        s.layouts.push(NamedLayout {
            name: "Compact".into(),
            dump: serde_json::json!({ "dock": "compact" }),
            size: Some(LayoutSize {
                width: 800.0,
                height: 600.0,
            }),
        });
        s.layouts.push(NamedLayout {
            name: "Wide".into(),
            dump: serde_json::json!({ "dock": "wide" }),
            size: None,
        });
        s
    }

    /// `all` lists every saved preset in save order, dumps and sizes carried
    /// through untouched.
    #[test]
    fn all_lists_presets_in_order() {
        let s = settings_with_presets();
        let presets = all(&s);
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].name, "Compact");
        assert_eq!(presets[1].name, "Wide");
        // The size rides along with the preset that has one.
        assert_eq!(presets[0].size.map(|z| z.width), Some(800.0));
        assert!(presets[1].size.is_none());
    }

    /// `resolve` finds a preset by exact name and hands back its dump and
    /// size; an unknown name resolves to None.
    #[test]
    fn resolve_finds_known_and_misses_unknown() {
        let s = settings_with_presets();
        let hit = resolve(&s, "Compact").expect("Compact resolves");
        assert_eq!(hit.dump, serde_json::json!({ "dock": "compact" }));
        assert_eq!(hit.size.map(|z| z.height), Some(600.0));
        // A preset with no stored size resolves with None, not a default.
        assert!(resolve(&s, "Wide").unwrap().size.is_none());
        // Nothing carries this name.
        assert!(resolve(&s, "Nope").is_none());
    }
}
