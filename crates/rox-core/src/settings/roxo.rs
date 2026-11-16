//! `.roxo`, the object archive: `object.json` holding one canvas item and
//! what it uses, plus one flat `assets/<file>` per entry in its asset table.
//! Nothing else in an archive is read, and nothing is ever unpacked: an
//! object's files are read one at a time, straight into the asset store,
//! when it's placed. The zip rules themselves live in rox-package.
//!
//! The item is kept as JSON, since rox-core doesn't know canvases. The walks
//! over it below match the dump walks in `settings.rs` key for key.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use rox_package::Limits;
use rox_viz::signal::{Route, Signal};
use serde::{Deserialize, Serialize};

use crate::settings::{
    Asset, NamedShader, SHADER_PANEL, WorkspaceMeta, assets, collect_owners, data_dir, lenient,
    scrub_dump_paths,
};

pub const EXT: &str = "roxo";
pub const MANIFEST: &str = "object.json";
pub const OBJECT_VERSION: u32 = 1;

/// No hard cap on the total: a heavy object warns in the confirm and still
/// installs. The ratio is what refuses a bomb.
pub const LIMITS: Limits = Limits {
    max_entries: 2_000,
    max_total: u64::MAX,
    max_ratio: 100,
};

/// Past this many unpacked bytes the place confirm shows the size.
pub const WARN_BYTES: u64 = 64 * 1024 * 1024;

/// One item tree, grids' dock dumps included, runs to kilobytes. The cap
/// stops a hostile manifest before it's parsed.
pub const MANIFEST_MAX: u64 = 4 * 1024 * 1024;

const ASSETS: &str = "assets/";

/// One item lifted out of a canvas, with everything it needs to work in
/// another. Reads leniently field by field, so one broken entry costs only
/// itself.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectBundle {
    pub version: u32,
    pub name: String,
    /// The same card a workspace carries.
    #[serde(
        skip_serializing_if = "WorkspaceMeta::is_empty",
        deserialize_with = "lenient::or_default"
    )]
    pub meta: WorkspaceMeta,
    /// A `rox_canvas::CanvasItem`. Ids inside are local to the object.
    pub item: serde_json::Value,
    /// Routes whose targets name the item or one of its children.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub routes: Vec<Route>,
    /// The pool signals those routes read, and the ones the item's own
    /// routes read.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub signals: Vec<Signal>,
    /// The pool shaders the item's shader content names.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub shaders: Vec<NamedShader>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub assets: Vec<Asset>,
    /// Plugin ids, from the panels inside any grid it holds.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub requires: Vec<String>,
}

impl Default for ObjectBundle {
    fn default() -> Self {
        ObjectBundle {
            version: OBJECT_VERSION,
            name: String::new(),
            meta: WorkspaceMeta::default(),
            item: serde_json::Value::Null,
            routes: Vec::new(),
            signals: Vec::new(),
            shaders: Vec::new(),
            assets: Vec::new(),
            requires: Vec::new(),
        }
    }
}

impl ObjectBundle {
    /// What its files unpack to, summed over the table.
    pub fn unpacked(&self) -> u64 {
        self.assets.iter().map(|asset| asset.bytes).sum()
    }
}

/// Installed objects, one `.roxo` each. Not created here.
pub fn objects_dir() -> PathBuf {
    data_dir().join("objects")
}

/// The bundle alone, without extracting. Refuses a newer version, and an
/// archive whose table names a file it doesn't hold at the stated size.
pub fn read_bundle(archive: &Path) -> Result<ObjectBundle, String> {
    // Every name, count and stated size passes rox-package's rules first.
    let listed = rox_package::list(archive, &LIMITS)?;

    let text = rox_package::read(archive, MANIFEST, MANIFEST_MAX)?;
    let bundle: ObjectBundle =
        serde_json::from_slice(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    if bundle.version > OBJECT_VERSION {
        return Err(format!(
            "a newer rox made this object (version {}, this build reads {OBJECT_VERSION})",
            bundle.version
        ));
    }

    if !bundle.item.is_object() {
        return Err(format!("{MANIFEST} holds no item"));
    }
    // An object never carried version 1's inline images.
    if bundle
        .shaders
        .iter()
        .flat_map(|shader| &shader.assets)
        .any(|carried| !carried.data.is_empty())
    {
        return Err(format!("{MANIFEST} carries inline shader images"));
    }
    check_table(&bundle.assets)?;

    let stated: HashMap<&str, u64> = listed
        .iter()
        .map(|entry| (entry.name.as_str(), entry.bytes))
        .collect();
    for asset in &bundle.assets {
        let name = format!("{ASSETS}{}", asset.file);
        match stated.get(name.as_str()) {
            Some(&bytes) if bytes == asset.bytes => {}
            Some(_) => return Err(format!("{name} doesn't match its stated size")),
            None => return Err(format!("the archive is missing {name}")),
        }
    }

    Ok(bundle)
}

/// One asset's bytes, stopped at the table's size and checked against its
/// hash. Only a flat name of a kind rox knows is ever looked up, and only
/// under `assets/`.
pub fn read_asset(archive: &Path, asset: &Asset) -> Result<Vec<u8>, String> {
    check_table(std::slice::from_ref(asset))?;

    let bytes = rox_package::read(archive, &format!("{ASSETS}{}", asset.file), asset.bytes)?;
    if bytes.len() as u64 != asset.bytes || assets::sha256_hex(&bytes) != asset.sha256 {
        return Err(format!("{} doesn't match its hash", asset.file));
    }

    Ok(bytes)
}

/// Pack `bundle` with each table entry's file from `files`.
pub fn write(
    archive: &Path,
    bundle: &ObjectBundle,
    files: &[(Asset, PathBuf)],
) -> Result<(), String> {
    check_table(&bundle.assets)?;

    let mut packed = Vec::with_capacity(bundle.assets.len() + 1);
    for asset in &bundle.assets {
        let (_, path) = files
            .iter()
            .find(|(held, _)| held.id == asset.id)
            .ok_or_else(|| format!("no file for {}", asset.file))?;

        let (bytes, sha256) = assets::sha256_file(path)?;
        if bytes != asset.bytes || sha256 != asset.sha256 {
            return Err(format!("{} doesn't match its hash", asset.file));
        }

        packed.push((format!("{ASSETS}{}", asset.file), path.clone()));
    }

    let mut bundle = bundle.clone();
    bundle.version = OBJECT_VERSION;
    let text = serde_json::to_string_pretty(&bundle).map_err(|e| format!("{MANIFEST}: {e}"))?;

    let manifest = suffixed(archive, ".manifest");
    rox_package::fresh::write(&manifest, text)
        .map_err(|e| format!("{}: {e}", manifest.display()))?;
    packed.push((MANIFEST.to_string(), manifest.clone()));

    let result = rox_package::pack(archive, &packed);
    let _ = std::fs::remove_file(&manifest);

    result
}

/// The plugin ids an item's panels came from.
pub fn requires(item: &serde_json::Value) -> Vec<String> {
    let mut ids = BTreeSet::new();
    collect_owners(item, &mut ids);
    ids.into_iter().collect()
}

/// Takes the shader file bookmarks out of an item, the way an export does
/// for a workspace's layouts.
pub fn scrub(item: &mut serde_json::Value) {
    scrub_dump_paths(item);
}

/// Every pool shader an item picks by name: a surface shader's `name`, and
/// the Shader panel's config `name` inside a grid.
pub fn shader_names(item: &serde_json::Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    visit_shader_names(item, &mut |name| {
        names.insert(name.clone());
    });
    names
}

/// Points each pick in `renamed` at its new pool name.
pub fn rename_shaders(item: &mut serde_json::Value, renamed: &HashMap<String, String>) {
    visit_shader_names_mut(item, &mut |name| {
        if let Some(to) = renamed.get(name.as_str()) {
            *name = to.clone();
        }
    });
}

/// Every signal a route inside the item reads: a surface shader's slot
/// routes, and any panel's in a grid. The canvas's own routes aren't in here.
pub fn route_signals(item: &serde_json::Value) -> BTreeSet<u64> {
    let mut ids = BTreeSet::new();
    visit_routes(item, &mut |route| {
        if let Some(id) = route.get("signal").and_then(|id| id.as_u64()) {
            ids.insert(id);
        }
    });
    ids
}

/// Re-aims each route inside the item at the pool's id for its signal.
pub fn remap_route_signals(item: &mut serde_json::Value, ids: &HashMap<u64, u64>) {
    visit_routes_mut(item, &mut |route| {
        let Some(id) = route.get("signal").and_then(|id| id.as_u64()) else {
            return;
        };
        if let Some(&to) = ids.get(&id) {
            route.insert("signal".into(), to.into());
        }
    });
}

fn visit_shader_names(value: &serde_json::Value, f: &mut dyn FnMut(&String)) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(name)) =
                map.get("shader").and_then(|shader| shader.get("name"))
            {
                f(name);
            }
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL)
                && let Some(serde_json::Value::String(name)) = map
                    .get("info")
                    .and_then(|info| info.get("panel"))
                    .and_then(|config| config.get("name"))
            {
                f(name);
            }
            map.values().for_each(|child| visit_shader_names(child, f));
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| visit_shader_names(v, f)),
        _ => {}
    }
}

fn visit_shader_names_mut(value: &mut serde_json::Value, f: &mut dyn FnMut(&mut String)) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(name)) = map
                .get_mut("shader")
                .and_then(|shader| shader.get_mut("name"))
            {
                f(name);
            }
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL)
                && let Some(serde_json::Value::String(name)) = map
                    .get_mut("info")
                    .and_then(|info| info.get_mut("panel"))
                    .and_then(|config| config.get_mut("name"))
            {
                f(name);
            }
            map.values_mut()
                .for_each(|child| visit_shader_names_mut(child, f));
        }
        serde_json::Value::Array(items) => {
            items.iter_mut().for_each(|v| visit_shader_names_mut(v, f))
        }
        _ => {}
    }
}

/// Every `Route`'s JSON: an entry of a `routes` list that names a signal.
fn visit_routes(
    value: &serde_json::Value,
    f: &mut dyn FnMut(&serde_json::Map<String, serde_json::Value>),
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(routes)) = map.get("routes") {
                routes
                    .iter()
                    .filter_map(|route| route.as_object())
                    .filter(|route| route.contains_key("signal"))
                    .for_each(&mut *f);
            }
            map.values().for_each(|child| visit_routes(child, f));
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| visit_routes(v, f)),
        _ => {}
    }
}

fn visit_routes_mut(
    value: &mut serde_json::Value,
    f: &mut dyn FnMut(&mut serde_json::Map<String, serde_json::Value>),
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(routes)) = map.get_mut("routes") {
                routes
                    .iter_mut()
                    .filter_map(|route| route.as_object_mut())
                    .filter(|route| route.contains_key("signal"))
                    .for_each(&mut *f);
            }
            map.values_mut()
                .for_each(|child| visit_routes_mut(child, f));
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|v| visit_routes_mut(v, f)),
        _ => {}
    }
}

/// The table is the only thing that says which entries get read, so it has
/// to be sane first: flat names of a kind rox knows, no repeats.
fn check_table(table: &[Asset]) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut files = HashSet::new();

    for asset in table {
        if !assets::flat_name(&asset.file) {
            return Err(format!("{} isn't a plain file name", asset.file));
        }
        if assets::kind_for(&asset.file).is_none() {
            return Err(format!("{} isn't a kind of file rox reads", asset.file));
        }
        if !ids.insert(asset.id.as_str()) {
            return Err(format!("the asset id {} is used twice", asset.id));
        }
        if !files.insert(asset.file.to_lowercase()) {
            return Err(format!("{} is named twice", asset.file));
        }
    }

    Ok(())
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AssetKind;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-roxo-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn raw_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let src = path.with_extension("src");
        let files: Vec<(String, PathBuf)> = entries
            .iter()
            .enumerate()
            .map(|(n, (name, bytes))| {
                let file = src.join(n.to_string());
                std::fs::create_dir_all(&src).unwrap();
                std::fs::write(&file, bytes).unwrap();
                (name.to_string(), file)
            })
            .collect();
        rox_package::pack(path, &files).unwrap();
    }

    /// A bundle naming `frame.png`, with the file written under `dir`.
    fn carrying(dir: &Path, bytes: &[u8]) -> (ObjectBundle, Vec<(Asset, PathBuf)>) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("frame.png");
        std::fs::write(&path, bytes).unwrap();

        let asset = Asset {
            id: "frame".to_string(),
            file: "frame.png".to_string(),
            kind: AssetKind::Image,
            sha256: assets::sha256_hex(bytes),
            bytes: bytes.len() as u64,
        };
        let bundle = ObjectBundle {
            name: "Framed".to_string(),
            item: serde_json::json!({
                "id": "frame",
                "content": { "kind": "image", "asset": "frame" },
            }),
            assets: vec![asset.clone()],
            ..ObjectBundle::default()
        };

        (bundle, vec![(asset, path)])
    }

    #[test]
    fn an_object_round_trips() {
        let root = scratch("roundtrip");
        let (mut bundle, files) = carrying(&root.join("live"), b"frame bytes");
        bundle.meta.author = "Someone".to_string();
        bundle.routes = vec![Route {
            signal: 3,
            target: "frame.y".to_string(),
            ..Route::default()
        }];
        bundle.signals = vec![Signal {
            id: 3,
            name: "Kick".to_string(),
            ..Signal::default()
        }];
        let archive = root.join("framed.roxo");

        write(&archive, &bundle, &files).unwrap();
        assert!(!suffixed(&archive, ".manifest").exists());

        let back = read_bundle(&archive).unwrap();
        assert_eq!(back.name, "Framed");
        assert_eq!(back.version, OBJECT_VERSION);
        assert_eq!(back.meta.author, "Someone");
        assert_eq!(back.item, bundle.item);
        assert_eq!(back.assets, bundle.assets);
        assert_eq!(back.routes[0].target, "frame.y");
        assert_eq!(back.signals[0].name, "Kick");
        assert_eq!(
            read_asset(&archive, &back.assets[0]).unwrap(),
            b"frame bytes"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_the_manifest_and_assets_are_read() {
        let root = scratch("only");
        let (bundle, _) = carrying(&root.join("live"), b"frame bytes");
        let manifest = serde_json::to_vec(&bundle).unwrap();

        let archive = root.join("extra.roxo");
        raw_zip(
            &archive,
            &[
                (MANIFEST, &manifest),
                ("assets/frame.png", b"frame bytes"),
                ("frame.png", b"FRAME BYTES"),
                ("run.sh", b"#!/bin/sh"),
                ("_rox/sig", b"sig"),
            ],
        );

        let back = read_bundle(&archive).unwrap();
        assert_eq!(
            read_asset(&archive, &back.assets[0]).unwrap(),
            b"frame bytes"
        );

        // A table naming a path, or a kind rox doesn't read, is never looked
        // up: the table is what decides which entries get read.
        for file in ["../frame.png", "assets/frame.png", "run.sh", "/frame.png"] {
            let mut sneaky = back.assets[0].clone();
            sneaky.file = file.to_string();
            assert!(read_asset(&archive, &sneaky).is_err(), "{file}");

            let mut forged = bundle.clone();
            forged.assets[0].file = file.to_string();
            let manifest = serde_json::to_vec(&forged).unwrap();
            raw_zip(
                &root.join("forged.roxo"),
                &[(MANIFEST, &manifest), ("run.sh", b"#!/bin/sh")],
            );
            assert!(read_bundle(&root.join("forged.roxo")).is_err(), "{file}");
        }

        // An asset kept beside the manifest instead of under `assets/` is
        // refused as missing.
        let archive = root.join("loose.roxo");
        raw_zip(
            &archive,
            &[(MANIFEST, &manifest), ("frame.png", b"frame bytes")],
        );
        let err = read_bundle(&archive).err().unwrap();
        assert!(err.contains("missing"), "{err}");

        // Same length, other bytes, so only the hash catches it.
        let archive = root.join("swapped.roxo");
        raw_zip(
            &archive,
            &[(MANIFEST, &manifest), ("assets/frame.png", b"FRAME BYTES")],
        );
        let back = read_bundle(&archive).unwrap();
        assert!(read_asset(&archive, &back.assets[0]).is_err());

        // A manifest past the cap is refused before it's parsed.
        let archive = root.join("huge.roxo");
        let huge = vec![b' '; MANIFEST_MAX as usize + 1];
        raw_zip(&archive, &[(MANIFEST, &huge)]);
        assert!(read_bundle(&archive).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_newer_object_is_refused() {
        let root = scratch("newer");
        let bundle = ObjectBundle {
            version: OBJECT_VERSION + 1,
            item: serde_json::json!({ "id": "x" }),
            ..ObjectBundle::default()
        };
        let manifest = serde_json::to_vec(&bundle).unwrap();

        let archive = root.join("future.roxo");
        raw_zip(&archive, &[(MANIFEST, &manifest)]);

        let err = read_bundle(&archive).err().unwrap();
        assert!(err.contains("newer rox"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_broken_field_costs_only_itself() {
        let root = scratch("lenient");
        let json = serde_json::json!({
            "version": 1,
            "name": "Loose",
            "meta": 7,
            "item": { "id": "x" },
            "signals": [{ "id": 1, "source": { "kind": "level" } }, { "id": "two" }],
            "requires": ["one", 2],
        });

        let archive = root.join("loose.roxo");
        raw_zip(&archive, &[(MANIFEST, json.to_string().as_bytes())]);

        let back = read_bundle(&archive).unwrap();
        assert_eq!(back.name, "Loose");
        assert!(back.meta.is_empty());
        assert_eq!(back.signals.len(), 1);
        assert_eq!(back.requires, ["one"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_walks_find_and_rewrite_shader_picks_and_route_signals() {
        let mut item = serde_json::json!({
            "id": "group",
            "content": { "kind": "group", "items": [
                { "id": "glow", "content": { "kind": "shader", "shader": {
                    "name": "Glow",
                    "path": "/home/me/glow.wgsl",
                    "routes": [{ "signal": 4, "target": "0" }],
                } } },
                { "id": "grid", "content": { "kind": "grid", "dock": {
                    "panel_name": "shader",
                    "info": { "panel": { "name": "Plasma", "path": "/x.wgsl" } },
                } } },
            ] },
        });

        let names: Vec<String> = shader_names(&item).into_iter().collect();
        assert_eq!(names, ["Glow", "Plasma"]);
        assert_eq!(route_signals(&item).into_iter().collect::<Vec<_>>(), [4]);

        rename_shaders(
            &mut item,
            &HashMap::from([("Glow".to_string(), "Glow (2)".to_string())]),
        );
        remap_route_signals(&mut item, &HashMap::from([(4, 9)]));
        scrub(&mut item);

        let shader = &item["content"]["items"][0]["content"]["shader"];
        assert_eq!(shader["name"], "Glow (2)");
        assert_eq!(shader["routes"][0]["signal"], 9);
        assert!(shader.get("path").is_none());

        let config = &item["content"]["items"][1]["content"]["dock"]["info"]["panel"];
        assert_eq!(config["name"], "Plasma");
        assert!(config.get("path").is_none());
    }
}
