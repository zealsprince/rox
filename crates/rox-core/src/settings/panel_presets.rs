//! Named single panels: one panel's dump, rebuilt already configured. The
//! workspace tier lives in the live bundle because a panel can name a shader
//! from that workspace's pool. The global tier lives in `presets.json` and
//! follows the user across workspaces, so its shaders are inlined at save
//! and never name a pool. Built-in and other-workspace presets are assembled
//! up in rox; this module only stores the two writable tiers.

use serde_json::{Map, Value};

use crate::settings::{NamedShader, PanelPreset, SHADER_PANEL, Settings, shader_pool};

/// Where a listed preset came from. Built-in and other-workspace entries are
/// read-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetScope {
    /// Shipped in code. Its `name` is an i18n key, not display text.
    Builtin,
    /// From `presets.json`.
    Global,
    /// From the live workspace's bundle.
    Workspace,
    /// A saved workspace that isn't the live one, by its name.
    Other(String),
}

#[derive(Clone, Debug)]
pub struct PresetEntry {
    pub scope: PresetScope,
    pub preset: PanelPreset,
}

/// Where a save or delete goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveScope {
    Workspace,
    Global,
}

/// The workspace tier.
pub fn all(settings: &Settings) -> Vec<PanelPreset> {
    settings.look.bundle.panel_presets.clone()
}

pub fn global(settings: &Settings) -> Vec<PanelPreset> {
    settings.presets.panel_presets.clone()
}

/// Global first, then workspace: the order every flat menu lists them in.
pub fn listed(settings: &Settings) -> Vec<PresetEntry> {
    let entries_of = |scope: PresetScope, presets: &[PanelPreset]| {
        presets
            .iter()
            .map(|preset| PresetEntry {
                scope: scope.clone(),
                preset: preset.clone(),
            })
            .collect::<Vec<_>>()
    };

    let mut entries = entries_of(PresetScope::Global, &settings.presets.panel_presets);
    entries.extend(entries_of(
        PresetScope::Workspace,
        &settings.look.bundle.panel_presets,
    ));
    entries
}

/// In the workspace tier.
pub fn resolve(settings: &Settings, name: &str) -> Option<PanelPreset> {
    resolve_in(settings, SaveScope::Workspace, name)
}

pub fn resolve_in(settings: &Settings, scope: SaveScope, name: &str) -> Option<PanelPreset> {
    tier(settings, scope)
        .iter()
        .find(|preset| preset.name == name)
        .cloned()
}

/// Save `panel` under `name` in the workspace tier, replacing any preset
/// with that name.
pub fn save(name: String, panel: Value) {
    save_to(SaveScope::Workspace, name, panel);
}

/// Save `panel` under `name` in `scope`, replacing any preset with that name
/// there. Global saves go through [`detach`] against the live pool first.
pub fn save_to(scope: SaveScope, name: String, mut panel: Value) {
    if scope == SaveScope::Global {
        detach_dump(&mut panel, &shader_pool());
    }

    Settings::update(move |s| put(tier_mut(s, scope), name, panel));
}

fn tier(settings: &Settings, scope: SaveScope) -> &Vec<PanelPreset> {
    match scope {
        SaveScope::Workspace => &settings.look.bundle.panel_presets,
        SaveScope::Global => &settings.presets.panel_presets,
    }
}

fn tier_mut(settings: &mut Settings, scope: SaveScope) -> &mut Vec<PanelPreset> {
    match scope {
        SaveScope::Workspace => &mut settings.look.bundle.panel_presets,
        SaveScope::Global => &mut settings.presets.panel_presets,
    }
}

fn put(presets: &mut Vec<PanelPreset>, name: String, panel: Value) {
    match presets.iter_mut().find(|preset| preset.name == name) {
        Some(preset) => preset.panel = panel,
        None => presets.push(PanelPreset { name, panel }),
    }
}

/// In the workspace tier.
pub fn remove(name: &str) {
    remove_from(SaveScope::Workspace, name);
}

pub fn remove_from(scope: SaveScope, name: &str) {
    let name = name.to_string();
    Settings::update(move |s| {
        tier_mut(s, scope).retain(|p| p.name != name);
    });
}

/// A copy whose shader references no longer need `pool`: every `name` the
/// pool holds becomes that entry's `source`, and a name it doesn't hold
/// empties the shader rather than letting stale inline text run under it.
pub fn detach(preset: &PanelPreset, pool: &[NamedShader]) -> PanelPreset {
    let mut detached = preset.clone();
    detach_dump(&mut detached.panel, pool);
    detached
}

/// Walks the same two shapes as the dump walks in `settings.rs`.
fn detach_dump(value: &mut Value, pool: &[NamedShader]) {
    match value {
        Value::Object(map) => {
            // Any panel's surface shader, flattened onto its config.
            if let Some(Value::Object(shader)) = map.get_mut("shader") {
                inline_named(shader, pool);
            }

            // The Shader panel, whose config is the shader.
            if map.get("panel_name").and_then(Value::as_str) == Some(SHADER_PANEL)
                && let Some(Value::Object(config)) =
                    map.get_mut("info").and_then(|info| info.get_mut("panel"))
            {
                inline_named(config, pool);
            }

            for child in map.values_mut() {
                detach_dump(child, pool);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| detach_dump(item, pool)),
        _ => {}
    }
}

fn inline_named(shader: &mut Map<String, Value>, pool: &[NamedShader]) {
    let Some(name) = shader.get("name").and_then(Value::as_str) else {
        return;
    };

    // First match, the same entry `shader_pool_get` runs.
    let source = pool
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.source.clone())
        .unwrap_or_default();

    shader.remove("name");
    // The bookmark points at pre-name inline text. Left on an unnamed
    // shader, the surface would watch it and pull that text back in.
    shader.remove("path");
    shader.insert("source".into(), Value::String(source));
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
    fn listed_puts_global_before_workspace() {
        let mut s = settings_with_presets();
        put(
            &mut s.presets.panel_presets,
            "Everywhere".into(),
            serde_json::json!({ "panel_name": "lyrics" }),
        );

        let entries = listed(&s);
        let order: Vec<_> = entries
            .iter()
            .map(|entry| (entry.scope.clone(), entry.preset.name.as_str()))
            .collect();
        assert_eq!(
            order,
            [
                (PresetScope::Global, "Everywhere"),
                (PresetScope::Workspace, "Big Cover"),
                (PresetScope::Workspace, "Scope"),
            ]
        );

        assert!(resolve_in(&s, SaveScope::Global, "Everywhere").is_some());
        assert!(resolve_in(&s, SaveScope::Global, "Scope").is_none());
        assert!(resolve(&s, "Everywhere").is_none());
    }

    fn grain_pool() -> Vec<NamedShader> {
        vec![NamedShader {
            name: "Grain".into(),
            source: "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4(uv, 0.0, 1.0); }".into(),
            ..Default::default()
        }]
    }

    fn preset(panel: Value) -> PanelPreset {
        PanelPreset {
            name: "Test".into(),
            panel,
        }
    }

    #[test]
    fn detach_inlines_a_pool_name_and_drops_the_bookmark() {
        let pool = grain_pool();
        let original = preset(serde_json::json!({
            "panel_name": "cover art",
            "info": { "panel": {
                "shader": { "enabled": true, "name": "Grain", "path": "/x.wgsl" },
            } },
        }));

        let detached = detach(&original, &pool);
        let shader = &detached.panel["info"]["panel"]["shader"];
        assert_eq!(shader["source"], pool[0].source.as_str());
        assert!(shader.get("name").is_none());
        assert!(shader.get("path").is_none());
        assert_eq!(shader["enabled"], true);
        assert_eq!(
            original.panel["info"]["panel"]["shader"]["name"], "Grain",
            "the input is left alone"
        );
    }

    #[test]
    fn detach_empties_a_name_the_pool_lacks() {
        let detached = detach(
            &preset(serde_json::json!({
                "panel_name": "cover art",
                "info": { "panel": {
                    "shader": { "name": "Gone", "source": "stale text", "path": "/x.wgsl" },
                } },
            })),
            &grain_pool(),
        );

        let shader = &detached.panel["info"]["panel"]["shader"];
        assert_eq!(shader["source"], "");
        assert!(shader.get("name").is_none());
        assert!(shader.get("path").is_none());
    }

    #[test]
    fn detach_reaches_a_composites_children_and_the_shader_panel() {
        let pool = grain_pool();
        let detached = detach(
            &preset(serde_json::json!({
                "panel_name": "stack",
                "children": [
                    {
                        "panel_name": "shader",
                        "info": { "panel": { "name": "Grain" } },
                    },
                    {
                        "panel_name": "spectrum",
                        "info": { "panel": { "shader": { "name": "Gone" } } },
                    },
                    {
                        "panel_name": "waveform",
                        "info": { "panel": { "shader": { "source": "own text", "path": "/y.wgsl" } } },
                    },
                ],
            })),
            &pool,
        );

        let children = &detached.panel["children"];
        assert_eq!(
            children[0]["info"]["panel"]["source"],
            pool[0].source.as_str()
        );
        assert!(children[0]["info"]["panel"].get("name").is_none());
        assert_eq!(children[1]["info"]["panel"]["shader"]["source"], "");

        let unnamed = &children[2]["info"]["panel"]["shader"];
        assert_eq!(unnamed["source"], "own text");
        assert_eq!(
            unnamed["path"], "/y.wgsl",
            "an unnamed shader keeps its bookmark"
        );
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
