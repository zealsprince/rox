//! `.roxw`, the workspace archive: `workspace.json` holding the bundle at
//! version 2, plus one flat `assets/<file>` per entry in its asset table.
//! Nothing else in an archive is read, and the zip rules themselves live in
//! rox-package.
//!
//! Also the move from version 1, whose bundles carried shader images inline
//! as base64. [`upgrade`] drains those into asset table entries and bytes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rox_package::{Entry, Limits};

use crate::settings::{Asset, WORKSPACE_VERSION, WorkspaceBundle, assets, safe_file_stem};

pub const EXT: &str = "roxw";
pub const MANIFEST: &str = "workspace.json";

/// No hard cap on the total: a large look warns in the confirm and still
/// imports. The ratio is what refuses a bomb.
pub const LIMITS: Limits = Limits {
    max_entries: 10_000,
    max_total: u64::MAX,
    max_ratio: 100,
};

/// Past this many unpacked bytes the import confirm shows the size.
pub const WARN_BYTES: u64 = 256 * 1024 * 1024;

const MANIFEST_MAX: u64 = 16 * 1024 * 1024;

const ASSETS: &str = "assets/";

/// Asset table entries with the bytes drained out of a version 1 bundle.
pub type Drained = Vec<(Asset, Vec<u8>)>;

/// The bundle alone, without extracting. Refuses a newer version.
pub fn read_bundle(archive: &Path) -> Result<WorkspaceBundle, String> {
    rox_package::list(archive, &LIMITS)?;

    let text = rox_package::read(archive, MANIFEST, MANIFEST_MAX)?;
    let mut bundle: WorkspaceBundle =
        serde_json::from_slice(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    if bundle.version > WORKSPACE_VERSION {
        return Err(format!(
            "the workspace is version {}, newer than this build reads ({WORKSPACE_VERSION})",
            bundle.version
        ));
    }

    // An archive never held version 1's inline images, so one carrying them
    // was made by something other than rox.
    if carries_data(&bundle) {
        return Err(format!("{MANIFEST} carries inline shader images"));
    }
    check_table(&bundle.assets)?;

    upgrade(&mut bundle);

    Ok(bundle)
}

/// One asset's bytes, checked against the table's size and hash.
pub fn read_asset(archive: &Path, bundle: &WorkspaceBundle, id: &str) -> Result<Vec<u8>, String> {
    let asset = bundle
        .assets
        .iter()
        .find(|asset| asset.id == id)
        .ok_or_else(|| format!("the workspace has no asset {id}"))?;
    if !assets::flat_name(&asset.file) {
        return Err(format!("{} isn't a plain file name", asset.file));
    }

    let bytes = rox_package::read(archive, &format!("{ASSETS}{}", asset.file), asset.bytes)?;
    if bytes.len() as u64 != asset.bytes || assets::sha256_hex(&bytes) != asset.sha256 {
        return Err(format!("{} doesn't match its hash", asset.file));
    }

    Ok(bytes)
}

/// The table's files into `into`. Only names the table holds, each stopped
/// at its stated size and checked against its hash.
pub fn extract_assets(archive: &Path, bundle: &WorkspaceBundle, into: &Path) -> Result<(), String> {
    check_table(&bundle.assets)?;
    if into.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", into.display()));
    }

    let named: HashMap<String, &Asset> = bundle
        .assets
        .iter()
        .map(|asset| (format!("{ASSETS}{}", asset.file), asset))
        .collect();

    // rox-package counts every entry against the limits, the ones the
    // filter drops included, so the total is the table's bytes plus what
    // the rest of the archive states. A named entry stating more than the
    // table is refused before anything is written.
    let listed = rox_package::list(archive, &LIMITS)?;
    let mut total: u64 = bundle.assets.iter().map(|asset| asset.bytes).sum();
    for entry in &listed {
        match named.get(&entry.name) {
            Some(asset) if entry.bytes != asset.bytes => {
                return Err(format!("{} doesn't match its stated size", entry.name));
            }
            Some(_) => {}
            None => total = total.saturating_add(entry.bytes),
        }
    }
    let limits = Limits {
        max_total: total,
        ..LIMITS
    };

    let unpacked = suffixed(into, ".unpack");
    let _ = std::fs::remove_dir_all(&unpacked);

    let keep = |entry: &Entry| named.contains_key(&entry.name);
    let result = rox_package::unpack(archive, &unpacked, &limits, false, &keep)
        .and_then(|_| verify(&unpacked.join("assets"), &bundle.assets))
        .and_then(|()| place(&unpacked.join("assets"), into));
    let _ = std::fs::remove_dir_all(&unpacked);

    result
}

/// Pack `bundle` with its files read from `assets_from`.
pub fn write(archive: &Path, bundle: &WorkspaceBundle, assets_from: &Path) -> Result<(), String> {
    check_table(&bundle.assets)?;

    // Version 1's images are never written, so one still inline would be
    // lost on the way out.
    if carries_data(bundle) {
        return Err("the workspace still carries inline shader images".to_string());
    }

    let mut files = Vec::with_capacity(bundle.assets.len() + 1);
    for asset in &bundle.assets {
        let path = assets_from.join(&asset.file);
        let (bytes, sha256) = assets::sha256_file(&path)?;
        if bytes != asset.bytes || sha256 != asset.sha256 {
            return Err(format!("{} doesn't match its hash", asset.file));
        }

        files.push((format!("{ASSETS}{}", asset.file), path));
    }

    let mut bundle = bundle.clone();
    bundle.version = WORKSPACE_VERSION;

    // Stamped during serialization, not through a Value, so the manifest
    // keeps its field order with `$schema` first.
    #[derive(serde::Serialize)]
    struct Stamped<'a> {
        #[serde(rename = "$schema")]
        schema: &'a str,
        #[serde(flatten)]
        bundle: &'a WorkspaceBundle,
    }
    let text = serde_json::to_string_pretty(&Stamped {
        schema: &schema_url(),
        bundle: &bundle,
    })
    .map_err(|e| format!("{MANIFEST}: {e}"))?;

    let manifest = suffixed(archive, ".manifest");
    std::fs::write(&manifest, text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    files.push((MANIFEST.to_string(), manifest.clone()));

    let result = rox_package::pack(archive, &files);
    let _ = std::fs::remove_file(&manifest);

    result
}

/// The schema at the release that wrote the file, so an unpacked manifest
/// validates against the shape it was written in.
pub fn schema_url() -> String {
    format!(
        "https://raw.githubusercontent.com/zealsprince/rox/v{}/crates/rox/assets/workspace.schema.json",
        env!("CARGO_PKG_VERSION")
    )
}

/// Version 1 to 2 in memory: every `ShaderAsset.data` becomes an asset and
/// its bytes, and the bundle's version is set. A version 2 bundle returns
/// nothing.
///
/// Keyed on `data` rather than the version, since a pre-split look is
/// stamped with the current version while it still holds inline images.
pub fn upgrade(bundle: &mut WorkspaceBundle) -> Drained {
    let mut drained = Drained::new();
    let table = &mut bundle.assets;

    for shader in &mut bundle.shaders {
        let stem = safe_file_stem(&shader.name, "shader");

        shader.assets.retain_mut(|carried| {
            if carried.data.is_empty() {
                return true;
            }

            // Lenient like the bundle's lists: a broken image costs only
            // itself.
            let bytes = match carried.decode() {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::warn!(
                        "workspace: dropping {}'s {}: {e}",
                        shader.name,
                        carried.file
                    );
                    return false;
                }
            };
            carried.data.clear();

            let sha256 = assets::sha256_hex(&bytes);
            if let Some(shared) = table.iter().find(|asset| asset.sha256 == sha256) {
                carried.asset = shared.id.clone();
                return true;
            }

            let Some(name) = assets::store_name(&format!("{stem}-{}", carried.file)) else {
                log::warn!(
                    "workspace: dropping {}'s {}, a kind of file rox doesn't read",
                    shader.name,
                    carried.file
                );
                return false;
            };
            let taken = |id: &str, file: &str| {
                table
                    .iter()
                    .any(|asset| asset.id == id || asset.file.eq_ignore_ascii_case(file))
            };
            let Some((id, file)) = assets::free_name(&name, taken) else {
                log::warn!(
                    "workspace: dropping {}'s {}, no free name",
                    shader.name,
                    carried.file
                );
                return false;
            };

            let asset = Asset {
                id: id.clone(),
                kind: assets::kind_for(&file).unwrap_or_default(),
                file,
                sha256,
                bytes: bytes.len() as u64,
            };
            carried.asset = id;
            table.push(asset.clone());
            drained.push((asset, bytes));

            true
        });
    }

    bundle.version = WORKSPACE_VERSION;

    drained
}

/// A version 1 `.json` export, upgraded: the bundle and its drained bytes.
pub fn read_v1(json: &Path) -> Result<(WorkspaceBundle, Drained), String> {
    let text = std::fs::read_to_string(json).map_err(|e| format!("{}: {e}", json.display()))?;
    let mut bundle: WorkspaceBundle =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", json.display()))?;
    if bundle.version > 1 {
        return Err(format!(
            "{} is version {}, and only a version 1 export is read as JSON",
            json.display(),
            bundle.version
        ));
    }

    let drained = upgrade(&mut bundle);

    Ok((bundle, drained))
}

fn carries_data(bundle: &WorkspaceBundle) -> bool {
    bundle
        .shaders
        .iter()
        .flat_map(|shader| &shader.assets)
        .any(|carried| !carried.data.is_empty())
}

/// The table is the extraction bound, so it has to be sane before anything
/// is read against it: flat names of a kind rox knows, no repeats.
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

fn verify(dir: &Path, table: &[Asset]) -> Result<(), String> {
    for asset in table {
        let path = dir.join(&asset.file);
        if !path.exists() {
            return Err(format!("the archive is missing {ASSETS}{}", asset.file));
        }

        let (bytes, sha256) = assets::sha256_file(&path)?;
        if bytes != asset.bytes || sha256 != asset.sha256 {
            return Err(format!("{} doesn't match its hash", asset.file));
        }
    }

    Ok(())
}

/// An empty table still leaves a folder, so a swap always has one to take.
fn place(unpacked: &Path, into: &Path) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", into.display());

    if unpacked.is_dir() {
        std::fs::rename(unpacked, into).map_err(fail)
    } else {
        std::fs::create_dir_all(into).map_err(fail)
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;

    use super::*;
    use crate::settings::{NamedShader, ShaderAsset};

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-roxw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn inline(file: &str, bytes: &[u8]) -> ShaderAsset {
        ShaderAsset {
            file: file.to_string(),
            asset: String::new(),
            data: BASE64.encode(bytes),
        }
    }

    fn shader(name: &str, assets: Vec<ShaderAsset>) -> NamedShader {
        NamedShader {
            name: name.to_string(),
            source: "// fs".to_string(),
            path: None,
            assets,
        }
    }

    /// A bundle naming `frame.png`, with the file written under `dir`.
    fn carrying(dir: &Path, bytes: &[u8]) -> WorkspaceBundle {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("frame.png"), bytes).unwrap();

        WorkspaceBundle {
            name: "Framed".to_string(),
            assets: vec![Asset {
                id: "frame".to_string(),
                file: "frame.png".to_string(),
                kind: crate::settings::AssetKind::Image,
                sha256: assets::sha256_hex(bytes),
                bytes: bytes.len() as u64,
            }],
            ..WorkspaceBundle::default()
        }
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

    #[test]
    fn a_written_archive_reads_back() {
        let root = scratch("roundtrip");
        let bundle = carrying(&root.join("live"), b"frame bytes");
        let archive = root.join("framed.roxw");

        write(&archive, &bundle, &root.join("live")).unwrap();

        // The schema leads the manifest, pinned to this release.
        let manifest = rox_package::read(&archive, MANIFEST, MANIFEST_MAX).unwrap();
        let manifest = String::from_utf8(manifest).unwrap();
        let first = manifest
            .lines()
            .nth(1)
            .unwrap_or_default()
            .trim()
            .to_string();
        assert_eq!(first, format!("\"$schema\": \"{}\",", schema_url()));
        assert!(schema_url().contains(&format!("/v{}/", env!("CARGO_PKG_VERSION"))));

        let back = read_bundle(&archive).unwrap();
        assert_eq!(back.name, "Framed");
        assert_eq!(back.version, WORKSPACE_VERSION);
        assert_eq!(back.assets, bundle.assets);
        assert_eq!(
            read_asset(&archive, &back, "frame").unwrap(),
            b"frame bytes"
        );

        let into = root.join("staged");
        extract_assets(&archive, &back, &into).unwrap();
        assert_eq!(
            std::fs::read(into.join("frame.png")).unwrap(),
            b"frame bytes"
        );
        assert!(!suffixed(&into, ".unpack").exists());
        assert!(!suffixed(&archive, ".manifest").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_refuses_a_file_that_doesnt_match_its_hash() {
        let root = scratch("write-hash");
        let bundle = carrying(&root.join("live"), b"frame bytes");
        std::fs::write(root.join("live/frame.png"), b"other bytes").unwrap();

        let archive = root.join("framed.roxw");
        assert!(write(&archive, &bundle, &root.join("live")).is_err());
        assert!(!archive.exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_refuses_a_file_that_doesnt_match_its_hash() {
        let root = scratch("extract-hash");
        let bundle = carrying(&root.join("live"), b"frame bytes");
        let manifest = serde_json::to_vec(&bundle).unwrap();

        // Same length, other bytes, so only the hash catches it.
        let archive = root.join("forged.roxw");
        raw_zip(
            &archive,
            &[(MANIFEST, &manifest), ("assets/frame.png", b"FRAME BYTES")],
        );

        let back = read_bundle(&archive).unwrap();
        assert!(read_asset(&archive, &back, "frame").is_err());

        let into = root.join("staged");
        let err = extract_assets(&archive, &back, &into).unwrap_err();
        assert!(err.contains("hash"), "{err}");
        assert!(!into.exists());
        assert!(!suffixed(&into, ".unpack").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_refuses_a_missing_file() {
        let root = scratch("extract-missing");
        let bundle = carrying(&root.join("live"), b"frame bytes");
        let manifest = serde_json::to_vec(&bundle).unwrap();

        let archive = root.join("short.roxw");
        raw_zip(&archive, &[(MANIFEST, &manifest)]);

        let err = extract_assets(&archive, &bundle, &root.join("staged")).unwrap_err();
        assert!(err.contains("missing"), "{err}");
        assert!(!root.join("staged").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_skips_entries_the_table_doesnt_name() {
        let root = scratch("extract-skip");
        let bundle = carrying(&root.join("live"), b"frame bytes");
        let manifest = serde_json::to_vec(&bundle).unwrap();

        let archive = root.join("extra.roxw");
        raw_zip(
            &archive,
            &[
                (MANIFEST, &manifest),
                ("assets/frame.png", b"frame bytes"),
                ("assets/stray.png", b"stray"),
                ("run.sh", b"#!/bin/sh"),
                ("_rox/sig", b"sig"),
            ],
        );

        let into = root.join("staged");
        extract_assets(&archive, &bundle, &into).unwrap();

        let mut names: Vec<String> = std::fs::read_dir(&into)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["frame.png"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_table_naming_a_path_is_refused() {
        let root = scratch("table-path");
        let mut bundle = carrying(&root.join("live"), b"frame bytes");
        bundle.assets[0].file = "../frame.png".to_string();
        let manifest = serde_json::to_vec(&bundle).unwrap();

        let archive = root.join("escape.roxw");
        raw_zip(&archive, &[(MANIFEST, &manifest)]);

        assert!(read_bundle(&archive).is_err());
        assert!(extract_assets(&archive, &bundle, &root.join("staged")).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_newer_archive_is_refused() {
        let root = scratch("newer");
        let bundle = WorkspaceBundle {
            version: WORKSPACE_VERSION + 1,
            ..WorkspaceBundle::default()
        };
        let manifest = serde_json::to_vec(&bundle).unwrap();

        let archive = root.join("future.roxw");
        raw_zip(&archive, &[(MANIFEST, &manifest)]);

        let err = read_bundle(&archive).err().unwrap();
        assert!(err.contains("newer"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_archive_carrying_inline_images_is_refused() {
        let root = scratch("inline");
        let json = serde_json::json!({
            "version": 1,
            "shaders": [{
                "name": "Serpent",
                "source": "// fs",
                "assets": [{ "file": "plate.png", "data": "AAEC" }],
            }],
        });

        let archive = root.join("inline.roxw");
        raw_zip(&archive, &[(MANIFEST, json.to_string().as_bytes())]);
        assert!(read_bundle(&archive).is_err());

        // Version 1 without images is just an older stamp.
        let plain = serde_json::json!({ "version": 1, "name": "Plain" });
        raw_zip(&archive, &[(MANIFEST, plain.to_string().as_bytes())]);
        assert_eq!(read_bundle(&archive).unwrap().version, WORKSPACE_VERSION);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn upgrade_drains_every_shader_asset() {
        let mut bundle = WorkspaceBundle {
            version: 1,
            shaders: vec![
                shader("Serpent", vec![inline("plate.png", b"plate")]),
                shader(
                    "Grain",
                    vec![
                        inline("noise.png", b"noise"),
                        inline("Dither.PNG", b"dither"),
                    ],
                ),
            ],
            ..WorkspaceBundle::default()
        };

        let drained = upgrade(&mut bundle);

        assert_eq!(bundle.version, WORKSPACE_VERSION);
        assert_eq!(drained.len(), 3);
        assert_eq!(bundle.assets.len(), 3);

        let files: Vec<&str> = bundle.assets.iter().map(|a| a.file.as_str()).collect();
        assert_eq!(
            files,
            ["serpent-plate.png", "grain-noise.png", "grain-dither.png"]
        );

        for shader in &bundle.shaders {
            for carried in &shader.assets {
                assert!(carried.data.is_empty());
                let asset = bundle
                    .assets
                    .iter()
                    .find(|a| a.id == carried.asset)
                    .unwrap();
                let (_, bytes) = drained.iter().find(|(a, _)| a.id == asset.id).unwrap();
                assert_eq!(asset.sha256, assets::sha256_hex(bytes));
            }
        }
        assert_eq!(bundle.shaders[1].assets[1].file, "Dither.PNG");

        let json = serde_json::to_string(&bundle).unwrap();
        assert!(!json.contains("\"data\""), "{json}");
    }

    #[test]
    fn upgrade_drops_a_broken_asset_and_keeps_the_rest() {
        let mut broken = inline("dither.png", b"");
        broken.data = "not base64!".to_string();
        let mut bundle = WorkspaceBundle {
            version: 1,
            shaders: vec![shader(
                "Serpent",
                vec![inline("plate.png", b"plate"), broken],
            )],
            ..WorkspaceBundle::default()
        };

        let drained = upgrade(&mut bundle);

        assert_eq!(drained.len(), 1);
        assert_eq!(bundle.shaders[0].assets.len(), 1);
        assert_eq!(bundle.shaders[0].assets[0].file, "plate.png");
    }

    #[test]
    fn upgrade_shares_one_asset_between_equal_bytes() {
        let mut bundle = WorkspaceBundle {
            version: 1,
            shaders: vec![
                shader("Serpent", vec![inline("plate.png", b"same")]),
                shader("Grain", vec![inline("other.png", b"same")]),
            ],
            ..WorkspaceBundle::default()
        };

        let drained = upgrade(&mut bundle);

        assert_eq!(drained.len(), 1);
        assert_eq!(bundle.assets.len(), 1);
        assert_eq!(bundle.shaders[0].assets[0].asset, "serpent-plate");
        assert_eq!(bundle.shaders[1].assets[0].asset, "serpent-plate");
    }

    #[test]
    fn upgrading_a_v2_bundle_changes_nothing() {
        let root = scratch("v2");
        let mut bundle = carrying(&root.join("live"), b"frame bytes");
        bundle.shaders = vec![shader(
            "Framed",
            vec![ShaderAsset {
                file: "frame.png".to_string(),
                asset: "frame".to_string(),
                data: String::new(),
            }],
        )];
        let before = serde_json::to_string(&bundle).unwrap();

        assert!(upgrade(&mut bundle).is_empty());
        assert_eq!(serde_json::to_string(&bundle).unwrap(), before);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_v1_upgrades_and_refuses_a_newer_file() {
        let root = scratch("v1");
        let path = root.join("Old.json");
        let json = serde_json::json!({
            "version": 1,
            "name": "Old",
            "shaders": [{
                "name": "Serpent",
                "source": "// fs",
                "assets": [{ "file": "plate.png", "data": BASE64.encode(b"plate") }],
            }],
        });
        std::fs::write(&path, json.to_string()).unwrap();

        let (bundle, drained) = read_v1(&path).unwrap();
        assert_eq!(bundle.version, WORKSPACE_VERSION);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].1, b"plate");

        std::fs::write(&path, r#"{"version": 2}"#).unwrap();
        assert!(read_v1(&path).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
