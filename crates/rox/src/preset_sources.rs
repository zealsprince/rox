//! Every preset one panel kind can take, gathered from the four tiers in the
//! order a picker lists them: built in, global, this workspace, then the
//! other saved workspaces. rox-core stores the two writable tiers; the
//! built-ins live here because they're configs typed by the panel crates,
//! and the other workspaces because only this crate reads their bundles.

use rox_core::settings::panel_presets::{self, PresetEntry, PresetScope};
use rox_core::settings::{PanelPreset, Settings};

/// The openers entry behind the settings window's Presets row.
pub(crate) fn for_kind(kind: &str) -> Vec<PresetEntry> {
    let shipped = builtins(kind).into_iter().map(|preset| PresetEntry {
        scope: PresetScope::Builtin,
        preset,
    });
    let saved = panel_presets::listed(&Settings::load())
        .into_iter()
        .filter(|entry| entry.preset.panel_name() == Some(kind));

    shipped
        .chain(saved)
        .chain(crate::workspaces::presets_elsewhere(kind))
        .collect()
}

/// One arm per panel kind that ships presets. Their names are i18n keys.
fn builtins(kind: &str) -> Vec<PanelPreset> {
    match kind {
        "lyrics" => rox_panels::lyrics::builtin_presets()
            .into_iter()
            .map(|(key, config)| preset_of("lyrics", key, &config))
            .collect(),
        _ => Vec::new(),
    }
}

/// A dump of one configured panel, the shape the restore path builds.
pub(crate) fn preset_of(
    kind: &str,
    name_key: &'static str,
    config: &impl serde::Serialize,
) -> PanelPreset {
    let config = serde_json::to_value(config).unwrap_or_else(|e| {
        log::warn!("preset sources: {name_key} did not serialize: {e}");
        serde_json::Value::Null
    });

    PanelPreset {
        name: name_key.into(),
        panel: serde_json::json!({
            "panel_name": kind,
            "children": [],
            "info": { "panel": config },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize)]
    struct Config {
        size: u32,
        wrap: bool,
    }

    #[test]
    fn preset_of_wraps_the_config_as_a_panel_dump() {
        let preset = preset_of(
            "lyrics",
            "lyrics-preset-quiet",
            &Config {
                size: 14,
                wrap: true,
            },
        );

        assert_eq!(preset.name, "lyrics-preset-quiet");
        assert_eq!(preset.panel_name(), Some("lyrics"));
        assert_eq!(preset.panel["children"], serde_json::json!([]));
        assert_eq!(
            preset.panel["info"]["panel"],
            serde_json::json!({ "size": 14, "wrap": true })
        );
    }
}
