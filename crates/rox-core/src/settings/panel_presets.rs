//! Named single panels the user saved into the live look. Each is one panel's
//! dump under a name, the leaf a layout stores per panel: adding one back
//! builds the panel already configured. Presets belong to the workspace they
//! were saved in, so they're stored in `workspace.json` with the rest of the
//! look and travel inside a shared bundle, next to the shader pool a saved
//! panel can name.

use serde_json::Value;

use crate::settings::{PanelPreset, Settings};

/// Every saved preset for a menu or the settings list, in save order.
pub fn all(settings: &Settings) -> Vec<PanelPreset> {
    settings.look.bundle.panel_presets.clone()
}

/// Resolve a preset name to its panel dump. None when no preset has that
/// name.
pub fn resolve(settings: &Settings, name: &str) -> Option<PanelPreset> {
    settings
        .look
        .bundle
        .panel_presets
        .iter()
        .find(|preset| preset.name == name)
        .cloned()
}

/// Save `panel` under `name`, replacing the preset already using that name.
/// Saving over a preset is how you update one: the dialog says so, and
/// there's nothing else a second save of the same name could mean.
pub fn save(name: String, panel: Value) {
    Settings::update(move |s| put(&mut s.look.bundle.panel_presets, name, panel));
}

/// The save itself, over a borrowed list: replace by name, else append. Split
/// out from the settings write so the replace rule is testable without a file
/// on disk.
fn put(presets: &mut Vec<PanelPreset>, name: String, panel: Value) {
    match presets.iter_mut().find(|preset| preset.name == name) {
        Some(preset) => preset.panel = panel,
        None => presets.push(PanelPreset { name, panel }),
    }
}

/// Drop the preset named `name`. A name no preset has is a no-op.
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

    /// `all` lists every saved preset in save order, dumps passed through
    /// untouched.
    #[test]
    fn all_lists_presets_in_order() {
        let s = settings_with_presets();
        let presets = all(&s);
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].name, "Big Cover");
        assert_eq!(presets[1].name, "Scope");
    }

    /// `resolve` finds a preset by exact name and hands back its dump; an
    /// unknown name resolves to None.
    #[test]
    fn resolve_finds_known_and_misses_unknown() {
        let s = settings_with_presets();
        let hit = resolve(&s, "Scope").expect("Scope resolves");
        assert_eq!(hit.panel, serde_json::json!({ "panel_name": "spectrum" }));
        assert!(resolve(&s, "Nope").is_none());
    }

    /// Saving under a name already in the list replaces that preset in place
    /// rather than growing a second entry with the same name; a fresh name
    /// is appended at the end.
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

    /// The panel kind reads off the dump without deserializing it, and a blob
    /// that isn't a panel state says so rather than guessing.
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
