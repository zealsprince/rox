//! Icon packs: a folder of SVGs that overrides the app's built-in icons.
//! Each pack is a subfolder under [`packs_dir`] holding flat SVG files named
//! like the built-in icons (play.svg, heart.svg); a file present there wins
//! over our own embedded icon and the bundled widget set, a missing one
//! falls through. The active pack rides in [`crate::settings::Settings`] by
//! name and the resolver in [`crate::assets`] reads its folder.
//!
//! Switching packs takes effect on the next launch: gpui's sprite atlas keys
//! on the icon's path and only reads its bytes on a cache miss, so icons
//! already on screen keep their tiles. Startup wires the chosen pack in
//! before any window opens, so a launch always draws the picked set.

use std::path::PathBuf;

use crate::assets::{self, icons};
use crate::settings::data_dir;

/// The folder holding every icon pack, one subfolder per pack, beside the
/// library database. Not created here: the first pack makes it, so an unused
/// packs feature never leaves an empty folder behind.
pub fn packs_dir() -> PathBuf {
    data_dir().join("icons")
}

/// Every pack the user has, by name (its folder name), sorted for a stable
/// order in the settings window. A missing packs folder just reads as none.
pub fn all() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(packs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// The folder for a pack by name, if it exists on disk.
pub fn resolve_dir(name: &str) -> Option<PathBuf> {
    let dir = packs_dir().join(name);
    dir.is_dir().then_some(dir)
}

/// Point the asset resolver at a pack by name, or the built-in set for None
/// or a name whose folder is gone. Startup and the picker both call this.
pub fn activate(name: Option<&str>) {
    let dir = name.and_then(resolve_dir);
    assets::set_active_pack(dir);
}

/// A pack name safe to use as a folder name: trimmed, path separators and
/// other troublesome characters dropped, and never one of the dotted names.
/// Empty after cleaning falls back to "pack".
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        "pack".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Create a new pack folder seeded with the current icons: every icon the
/// app draws written out as an editable SVG, so an author starts from the
/// real set. A name that collides gets a numbered suffix. Returns the
/// created name for the caller to select, or an error string to surface.
pub fn create(name: &str) -> Result<String, String> {
    let base = sanitize(name);
    let name = crate::workspaces::unique_name(&base, |candidate| {
        packs_dir().join(candidate).exists()
    });
    let dir = packs_dir().join(&name);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for path in icons::CATALOG {
        // The catalog is all icons/<file>.svg; the pack is flat, so write
        // each under its bare file name.
        let Some(file) = path.strip_prefix("icons/") else {
            continue;
        };
        if let Some(bytes) = assets::builtin_bytes(path) {
            std::fs::write(dir.join(file), bytes).map_err(|e| e.to_string())?;
        }
    }
    Ok(name)
}

/// Delete a pack folder and everything in it. A missing folder is a no-op.
pub fn delete(name: &str) {
    if let Some(dir) = resolve_dir(name) {
        let _ = std::fs::remove_dir_all(dir);
    }
}
