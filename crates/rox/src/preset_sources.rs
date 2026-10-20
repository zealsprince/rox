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
    use rox_panels::*;

    fn wrap<C: serde::Serialize>(kind: &str, presets: Vec<(&'static str, C)>) -> Vec<PanelPreset> {
        presets
            .into_iter()
            .map(|(key, config)| preset_of(kind, key, &config))
            .collect()
    }

    match kind {
        "lyrics" => wrap(kind, lyrics::builtin_presets()),

        "spectrum" => wrap(kind, spectrum::builtin_presets()),
        "spectrogram" => wrap(kind, spectrogram::builtin_presets()),
        "oscilloscope" => wrap(kind, oscilloscope::builtin_presets()),
        "waveform" => wrap(kind, waveform::builtin_presets()),
        "vu meter" => wrap(kind, vu::builtin_presets()),
        "particles" => wrap(kind, particles::builtin_presets()),
        "milkdrop" => wrap(kind, milkdrop::builtin_presets()),

        "album grid" => wrap(kind, grid::builtin_presets()),
        "artist grid" => wrap(kind, artist_grid::builtin_presets()),
        "genre grid" => wrap(kind, genre_grid::builtin_presets()),

        "art view" => wrap(kind, art::builtin_presets()),
        "cover art" => wrap(kind, cover::builtin_presets()),
        "metadata" => wrap(kind, metadata::builtin_presets()),
        "biography" => wrap(kind, biography::builtin_presets()),

        "library" => wrap(kind, library::builtin_presets()),
        "playlists" => wrap(kind, playlists::builtin_presets()),
        "folder tree" => wrap(kind, folder_tree::builtin_presets()),
        "history" => wrap(kind, history::builtin_presets()),
        "queue" => wrap(kind, queue::builtin_presets()),

        "track info" => wrap(kind, transport::track_info_presets()),
        "seek" => wrap(kind, transport::seek_presets()),
        "playback" => wrap(kind, transport::playback_presets()),
        "volume" => wrap(kind, transport::volume_presets()),

        _ => Vec::new(),
    }
}

/// Every kind with built-ins, for the tests that walk them all.
#[cfg(test)]
const SHIPPED_KINDS: &[&str] = &[
    "lyrics",
    "spectrum",
    "spectrogram",
    "oscilloscope",
    "waveform",
    "vu meter",
    "particles",
    "milkdrop",
    "album grid",
    "artist grid",
    "genre grid",
    "art view",
    "cover art",
    "metadata",
    "biography",
    "library",
    "playlists",
    "folder tree",
    "history",
    "queue",
    "track info",
    "seek",
    "playback",
    "volume",
];

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

    /// A built-in's name is shown through `t!`, so a key the ftl lacks would
    /// surface raw in the picker.
    #[test]
    fn every_builtin_name_is_a_message_key() {
        rox_i18n::pin_thread_locale(rox_i18n::SOURCE_LOCALE);
        for kind in SHIPPED_KINDS {
            let presets = builtins(kind);
            assert!(!presets.is_empty(), "{kind} ships no presets");

            for preset in presets {
                assert_eq!(preset.panel_name(), Some(*kind));
                assert!(
                    rox_i18n::try_translate(&preset.name).is_some(),
                    "{kind}: {} is not a message key",
                    preset.name
                );
            }
        }
    }

    /// The catalog is the registry; a kind the match names but the catalog
    /// doesn't would never be asked for.
    #[test]
    fn shipped_kinds_are_catalog_names() {
        for kind in SHIPPED_KINDS {
            assert!(
                crate::panel_catalog::def_for(kind).is_some(),
                "{kind} is not a catalog name"
            );
        }
    }

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
