//! A plugin shipped as a `.roxp` (ADR 32): a zip with `plugin.json` at its
//! root and everything else the plugin's folder holds. A subprocess needs a
//! real directory, so a package runs from a copy unpacked into rox's cache
//! under its hash. The copy is rox's own and is rehashed at every scan, so
//! the copy that runs is the copy that was approved.
//!
//! Every archive rule (names, links, counts, sizes) lives in rox-package.
//! This module only decides what an entry means: which entries are the
//! plugin, and which are kept for signatures and never extracted (ADR 35).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use rox_package::Limits;

use crate::hash;
use crate::loader::{self, Origin};
use crate::manifest::{self, Manifest};

pub const EXT: &str = "roxp";

/// A package has a hard total, so it needs no ratio check.
pub const LIMITS: Limits = Limits {
    max_entries: 20_000,
    max_total: 1 << 30,
    max_ratio: 0,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Peek {
    pub manifest: Manifest,
    /// The manifest as written, for the approval diff.
    pub document: serde_json::Value,
    pub hash: String,
}

/// An archive's length and modified time.
type Stamp = (u64, Option<SystemTime>);

/// The last peek per archive, keyed on its stamp, so a rescan rehashes only
/// the packages that moved. Nothing approved rests on this: the copy that
/// runs is rehashed on every scan.
static PEEKS: LazyLock<Mutex<HashMap<PathBuf, (Stamp, Peek)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Manifest and hash without unpacking.
pub fn peek(archive: &Path) -> Result<Peek, String> {
    let meta = std::fs::metadata(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let stamp = (meta.len(), meta.modified().ok());

    let remembered = PEEKS
        .lock()
        .ok()
        .and_then(|peeks| peeks.get(archive).cloned());
    if let Some((seen, peek)) = remembered
        && seen == stamp
    {
        return Ok(peek);
    }

    let peek = read_peek(archive)?;
    if let Ok(mut peeks) = PEEKS.lock() {
        peeks.insert(archive.to_path_buf(), (stamp, peek.clone()));
    }

    Ok(peek)
}

fn read_peek(archive: &Path) -> Result<Peek, String> {
    let entries = rox_package::list(archive, &LIMITS)?;
    if !entries.iter().any(|entry| entry.name == manifest::FILE) {
        return Err(no_manifest(&entries));
    }

    let hash = hash::archive_hash(archive)?;

    let bytes = rox_package::read(archive, manifest::FILE, manifest::MAX_BYTES)?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} isn't UTF-8", manifest::FILE))?;
    let manifest = manifest::parse(&text)?;
    let document = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);

    Ok(Peek {
        manifest,
        document,
        hash,
    })
}

/// Names the folder when the whole plugin sits one level down, the way
/// zipping a folder from a file manager packs it.
fn no_manifest(entries: &[rox_package::Entry]) -> String {
    let wrappers: Vec<&str> = entries
        .iter()
        .filter_map(|entry| entry.name.strip_suffix(manifest::FILE)?.strip_suffix('/'))
        .filter(|folder| !folder.contains('/'))
        .collect();

    match wrappers.as_slice() {
        [folder] => format!(
            "{} has to sit at the top of the package, not inside {folder}/",
            manifest::FILE
        ),
        _ => format!("the package holds no {}", manifest::FILE),
    }
}

/// Into `cache/<id>/<first 16 of hash>/`. Reused as is when that folder
/// already hashes to the archive's hash, replaced when it doesn't.
pub fn unpack(archive: &Path, cache: &Path) -> Result<PathBuf, String> {
    let peek = peek(archive)?;

    // The id passed `valid_id`, so it's one plain component inside the cache.
    let home = cache.join(&peek.manifest.id);
    let target = home.join(&peek.hash[..16]);

    // A link here would run the plugin from wherever it points.
    if home.symlink_metadata().is_ok_and(|meta| !meta.is_dir()) {
        clear(&home).map_err(|e| format!("{}: {e}", home.display()))?;
    }

    let real_dir = target.symlink_metadata().is_ok_and(|meta| meta.is_dir());
    if real_dir && hash::folder_hash(&target).is_ok_and(|now| now == peek.hash) {
        return Ok(target);
    }

    clear(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    std::fs::create_dir_all(&home).map_err(|e| format!("{}: {e}", home.display()))?;
    rox_package::unpack(archive, &target, &LIMITS, true, &|entry| {
        hash::hashed(&entry.name)
    })?;

    // The guard: what landed on disk is what the archive's hash stands for.
    let unpacked = hash::folder_hash(&target);
    if unpacked.as_ref().is_ok_and(|now| *now == peek.hash) {
        return Ok(target);
    }

    let _ = clear(&target);
    Err(match unpacked {
        Err(e) => e,
        Ok(_) => "the unpacked copy doesn't match its package".to_string(),
    })
}

/// A folder that loads cleanly, packed.
pub fn pack(folder: &Path, archive: &Path) -> Result<(), String> {
    let id = folder
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no usable name", folder.display()))?;

    let loaded = loader::load(id.to_string(), folder.to_path_buf(), Origin::Folder);
    if !loaded.runs() {
        return Err(loaded.error.unwrap_or_else(|| format!("{id} doesn't load")));
    }

    // Written into the folder, the package would change the hash it was
    // packed under and switch the author's own plugin off.
    if inside(archive, folder) {
        return Err(format!("the package can't go inside {id}/"));
    }

    let mut files = Vec::new();
    hash::walk(folder, "", &mut files)?;
    rox_package::pack(archive, &files)?;

    // A file saved mid-pack would ship bytes nobody approved under this hash.
    let packed = hash::archive_hash(archive);
    if packed.as_ref().is_ok_and(|hash| *hash == loaded.hash) {
        return Ok(());
    }

    let _ = std::fs::remove_file(archive);
    Err(match packed {
        Err(e) => e,
        Ok(_) => format!("{id} changed while it was packed"),
    })
}

fn inside(archive: &Path, folder: &Path) -> bool {
    let (Some(parent), Some(name)) = (archive.parent(), archive.file_name()) else {
        return false;
    };
    let parent = match parent.as_os_str().is_empty() {
        true => Path::new("."),
        false => parent,
    };

    match (parent.canonicalize(), folder.canonicalize()) {
        (Ok(parent), Ok(folder)) => parent.join(name).starts_with(folder),
        _ => false,
    }
}

/// Removes whatever sits at `path`, a link as the link itself.
pub(crate) fn clear(path: &Path) -> std::io::Result<()> {
    match path.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        // A directory link on Windows only goes with `remove_dir`.
        Ok(_) => std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path)),
    }
}
