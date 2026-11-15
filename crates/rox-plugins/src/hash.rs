//! The folder hash, a plugin's identity for trust (ADR 30). SHA-256 over
//! every file under the folder, each fed as `<relative path>\0<length as u64
//! LE><bytes>` in byte order of the relative paths, `/` as the separator on
//! every OS so a folder hashes the same wherever it's copied.
//!
//! A symlink refuses the plugin instead of being followed: following one
//! would let the approved hash stand for bytes outside the folder.
//!
//! A folder and the `.roxp` packed from it hash the same, so packing never
//! asks for approval again (ADR 32).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Written by the OS, never executed. `__pycache__` isn't here: Python runs a
/// planted `.pyc`, so it counts.
const SKIPPED: [&str; 3] = [".DS_Store", "Thumbs.db", "desktop.ini"];

/// Kept for signatures in every rox archive. Matched without case, since a
/// case-folding volume would open `_ROX/` as the same folder.
pub(crate) const RESERVED: &str = "_rox";

pub fn folder_hash(dir: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    walk(dir, "", &mut files)?;

    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    let mut hasher = Sha256::new();
    for (rel, path) in &files {
        let bytes = std::fs::read(path).map_err(|e| format!("{rel}: {e}"))?;
        feed(&mut hasher, rel, &bytes);
    }

    Ok(hex(hasher))
}

/// The digest `folder_hash` gives the same files unpacked: same framing,
/// same order, same skipped names.
pub fn archive_hash(archive: &Path) -> Result<String, String> {
    let mut entries: Vec<rox_package::Entry> = rox_package::list(archive, &crate::package::LIMITS)?
        .into_iter()
        .filter(|entry| hashed(&entry.name))
        .collect();

    entries.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));

    let mut hasher = Sha256::new();
    for entry in &entries {
        let bytes = rox_package::read(archive, &entry.name, entry.bytes)?;
        feed(&mut hasher, &entry.name, &bytes);
    }

    Ok(hex(hasher))
}

/// Whether an archive entry is part of the plugin: what an unpack extracts
/// and what `archive_hash` feeds. Anything this drops never reaches the
/// folder plugin code runs from.
pub(crate) fn hashed(name: &str) -> bool {
    let last = name.rsplit('/').next().unwrap_or(name);

    !SKIPPED.contains(&last) && !reserved(name)
}

/// A name under `_rox/`.
fn reserved(name: &str) -> bool {
    name.split_once('/')
        .is_some_and(|(first, _)| first.eq_ignore_ascii_case(RESERVED))
}

fn feed(hasher: &mut Sha256, rel: &str, bytes: &[u8]) {
    hasher.update(rel.as_bytes());
    hasher.update([0u8]);
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn hex(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Every file `folder_hash` feeds, as `(relative path, path)`, unsorted.
pub(crate) fn walk(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();

        // A name that isn't UTF-8 can't hash the same on every OS.
        let name = name
            .to_str()
            .ok_or_else(|| format!("{prefix}{}: not a UTF-8 name", name.to_string_lossy()))?;
        let rel = format!("{prefix}{name}");

        let kind = entry.file_type().map_err(|e| format!("{rel}: {e}"))?;
        if kind.is_symlink() {
            return Err(format!("{rel} is a symlink"));
        }

        if kind.is_dir() {
            walk(&entry.path(), &format!("{rel}/"), out)?;
            continue;
        }

        if SKIPPED.contains(&name) {
            continue;
        }

        out.push((rel, entry.path()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("rox-plugins-hash-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            let scratch = Scratch(dir);
            scratch.write("plugin.json", "{}");
            scratch.write("lib/tones.py", "print('hi')");

            scratch
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn hash(&self) -> String {
            folder_hash(&self.0).expect("the folder hashes")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_hash_is_stable_across_runs() {
        let scratch = Scratch::new("stable");

        let first = scratch.hash();
        assert_eq!(first.len(), 64);
        assert_eq!(scratch.hash(), first);
    }

    #[test]
    fn the_hash_matches_the_contract_layout() {
        let scratch = Scratch::new("layout");

        // Byte order: "lib/tones.py" sorts before "plugin.json".
        let mut hasher = Sha256::new();
        for (rel, text) in [("lib/tones.py", "print('hi')"), ("plugin.json", "{}")] {
            hasher.update(rel.as_bytes());
            hasher.update([0u8]);
            hasher.update((text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        let expected: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        assert_eq!(scratch.hash(), expected);
    }

    #[test]
    fn a_changed_byte_changes_the_hash() {
        let scratch = Scratch::new("byte");
        let before = scratch.hash();

        scratch.write("lib/tones.py", "print('ho')");
        assert_ne!(scratch.hash(), before);
    }

    #[test]
    fn an_added_file_changes_the_hash() {
        let scratch = Scratch::new("added");
        let before = scratch.hash();

        scratch.write("lib/__pycache__/tones.cpython-312.pyc", "planted");
        assert_ne!(scratch.hash(), before, "a planted .pyc counts");
    }

    #[test]
    fn a_renamed_file_changes_the_hash() {
        let scratch = Scratch::new("renamed");
        let before = scratch.hash();

        std::fs::rename(
            scratch.0.join("lib/tones.py"),
            scratch.0.join("lib/tunes.py"),
        )
        .unwrap();
        assert_ne!(scratch.hash(), before);
    }

    #[test]
    fn os_litter_leaves_the_hash_alone() {
        let scratch = Scratch::new("litter");
        let before = scratch.hash();

        scratch.write(".DS_Store", "finder");
        scratch.write("lib/Thumbs.db", "explorer");
        scratch.write("desktop.ini", "explorer");
        assert_eq!(scratch.hash(), before);
    }

    #[test]
    fn an_archive_skips_what_a_folder_skips() {
        let scratch = Scratch::new("archive");
        let before = scratch.hash();

        // Litter and a signature slot beside the plugin's own files. The
        // sources sit outside the folder, which hashes only what it holds.
        let mut files = Vec::new();
        for (name, text) in [
            ("plugin.json", "{}"),
            ("lib/tones.py", "print('hi')"),
            (".DS_Store", "finder"),
            ("lib/Thumbs.db", "explorer"),
            ("_rox/signature", "sig"),
            ("_ROX/signature.old", "sig"),
        ] {
            let source = scratch.0.with_extension(format!("src-{}", files.len()));
            std::fs::write(&source, text).unwrap();
            files.push((name.to_string(), source));
        }

        let archive = scratch.0.with_extension("roxp");
        rox_package::pack(&archive, &files).unwrap();
        let packed = archive_hash(&archive);

        for (_, source) in &files {
            let _ = std::fs::remove_file(source);
        }
        let _ = std::fs::remove_file(&archive);

        assert_eq!(packed.unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_refuses_the_folder() {
        let scratch = Scratch::new("symlink");
        std::os::unix::fs::symlink("/etc/hosts", scratch.0.join("lib/hosts")).unwrap();

        let err = folder_hash(&scratch.0).unwrap_err();
        assert_eq!(err, "lib/hosts is a symlink");
    }
}
