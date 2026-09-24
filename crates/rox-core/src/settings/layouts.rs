//! Named dock layouts saved into the live look. The mini-player button
//! toggles between the two picked as primary and mini. Presets belong to
//! their workspace, so they live in `workspace.json` and travel in a bundle.

use serde_json::Value;

use crate::settings::{LayoutSize, Settings};

pub struct Preset {
    pub name: String,
    pub dump: Value,
    pub size: Option<LayoutSize>,
}

pub fn all(settings: &Settings) -> Vec<Preset> {
    settings
        .look
        .bundle
        .layouts
        .iter()
        .map(|saved| Preset {
            name: saved.name.clone(),
            dump: saved.dump.clone(),
            size: saved.size,
        })
        .collect()
}

pub fn resolve(settings: &Settings, name: &str) -> Option<Preset> {
    settings
        .look
        .bundle
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
        s.look.bundle.layouts.push(NamedLayout {
            name: "Compact".into(),
            dump: serde_json::json!({ "dock": "compact" }),
            size: Some(LayoutSize {
                width: 800.0,
                height: 600.0,
            }),
        });
        s.look.bundle.layouts.push(NamedLayout {
            name: "Wide".into(),
            dump: serde_json::json!({ "dock": "wide" }),
            size: None,
        });
        s
    }

    #[test]
    fn all_lists_presets_in_order() {
        let s = settings_with_presets();
        let presets = all(&s);
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].name, "Compact");
        assert_eq!(presets[1].name, "Wide");
        assert_eq!(presets[0].size.map(|z| z.width), Some(800.0));
        assert!(presets[1].size.is_none());
    }

    #[test]
    fn resolve_finds_known_and_misses_unknown() {
        let s = settings_with_presets();
        let hit = resolve(&s, "Compact").expect("Compact resolves");
        assert_eq!(hit.dump, serde_json::json!({ "dock": "compact" }));
        assert_eq!(hit.size.map(|z| z.height), Some(600.0));
        // A preset with no stored size resolves with None, not a default.
        assert!(resolve(&s, "Wide").unwrap().size.is_none());
        assert!(resolve(&s, "Nope").is_none());
    }
}
