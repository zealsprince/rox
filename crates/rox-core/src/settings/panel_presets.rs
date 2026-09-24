//! Named single panels saved into the live look: one panel's dump, rebuilt
//! already configured. They live in the workspace bundle because a panel can
//! name a shader from that workspace's pool.

use serde_json::Value;

use crate::settings::{PanelPreset, Settings};

pub fn all(settings: &Settings) -> Vec<PanelPreset> {
    settings.look.bundle.panel_presets.clone()
}

pub fn resolve(settings: &Settings, name: &str) -> Option<PanelPreset> {
    settings
        .look
        .bundle
        .panel_presets
        .iter()
        .find(|preset| preset.name == name)
        .cloned()
}

/// Save `panel` under `name`, replacing any preset with that name.
pub fn save(name: String, panel: Value) {
    Settings::update(move |s| put(&mut s.look.bundle.panel_presets, name, panel));
}

fn put(presets: &mut Vec<PanelPreset>, name: String, panel: Value) {
    match presets.iter_mut().find(|preset| preset.name == name) {
        Some(preset) => preset.panel = panel,
        None => presets.push(PanelPreset { name, panel }),
    }
}

pub fn remove(name: &str) {
    let name = name.to_string();
    Settings::update(move |s| {
        s.look.bundle.panel_presets.retain(|p| p.name != name);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with_presets() -> Settings {
        let mut s = Settings::default();
        s.look.bundle.panel_presets.push(PanelPreset {
            name: "Big Cover".into(),
            panel: serde_json::json!({
                "panel_name": "cover art",
                "children": [],
                "info": { "panel": { "size": 400 } },
            }),
        });
        s.look.bundle.panel_presets.push(PanelPreset {
            name: "Scope".into(),
            panel: serde_json::json!({ "panel_name": "spectrum" }),
        });
        s
    }

    #[test]
    fn all_lists_presets_in_order() {
        let s = settings_with_presets();
        let presets = all(&s);
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].name, "Big Cover");
        assert_eq!(presets[1].name, "Scope");
    }

    #[test]
    fn resolve_finds_known_and_misses_unknown() {
        let s = settings_with_presets();
        let hit = resolve(&s, "Scope").expect("Scope resolves");
        assert_eq!(hit.panel, serde_json::json!({ "panel_name": "spectrum" }));
        assert!(resolve(&s, "Nope").is_none());
    }

    #[test]
    fn put_replaces_by_name() {
        let mut presets = settings_with_presets().look.bundle.panel_presets;
        put(
            &mut presets,
            "Scope".into(),
            serde_json::json!({ "panel_name": "waveform" }),
        );
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[1].panel_name(), Some("waveform"));
        put(
            &mut presets,
            "Lyrics".into(),
            serde_json::json!({ "panel_name": "lyrics" }),
        );
        assert_eq!(presets.len(), 3);
        assert_eq!(presets[2].name, "Lyrics");
    }

    #[test]
    fn panel_name_reads_the_dump() {
        let s = settings_with_presets();
        assert_eq!(
            s.look.bundle.panel_presets[0].panel_name(),
            Some("cover art")
        );
        let junk = PanelPreset {
            name: "Junk".into(),
            panel: serde_json::json!("not a panel"),
        };
        assert!(junk.panel_name().is_none());
    }
}
