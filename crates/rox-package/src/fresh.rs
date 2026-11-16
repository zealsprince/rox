//! Temp and staging paths that are created new, never written through. rox
//! stages a write at a predictable name beside the file it replaces, and a
//! plain create follows whatever already stands at that name: a symlink
//! planted there would carry the write to its target. Every staged write
//! goes through here instead.
//!
//! This is a trust boundary for anyone who can write to the folder. Exploiting
//! it needs that access already, so it's hardening rather than a wall, and the
//! names stay predictable so cleanup stays simple.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// A new file at `path`, open for writing. A leftover at that name is
/// removed first: `remove_file` takes a symlink itself, never its target.
/// `create_new` then refuses anything that took its place in between, so
/// an entry still standing there is an error rather than a write through it.
pub fn file(path: &Path) -> io::Result<File> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    OpenOptions::new().write(true).create_new(true).open(path)
}

/// [`file`], with `bytes` written into it.
pub fn write(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    file(path)?.write_all(bytes.as_ref())
}

/// `from` copied into a [`file`] at `to`, permissions included, the way
/// `std::fs::copy` would. Returns the bytes copied.
pub fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    let mut input = File::open(from)?;
    let permissions = input.metadata()?.permissions();

    let mut out = file(to)?;
    let copied = io::copy(&mut input, &mut out)?;
    out.set_permissions(permissions)?;

    Ok(copied)
}

/// A new, empty folder at `path`, its parents made as needed. A leftover is
/// removed first without following a link: a folder with everything in it,
/// anything else as a file. `create_dir` then refuses a name that's taken
/// again, where `create_dir_all` would accept a link to a folder and the
/// writes inside would land in its target.
pub fn dir(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path)?,
        Ok(_) => std::fs::remove_file(path)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::create_dir(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("rox-fresh-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            Scratch(dir)
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_new_name_gets_the_bytes() {
        let scratch = Scratch::new("new");
        let tmp = scratch.path("a.tmp");

        write(&tmp, b"fresh").unwrap();
        assert_eq!(std::fs::read(&tmp).unwrap(), b"fresh");
    }

    #[test]
    fn a_stale_file_at_the_name_is_replaced() {
        let scratch = Scratch::new("stale");
        let tmp = scratch.path("a.tmp");
        std::fs::write(&tmp, b"left over from a crash, and longer").unwrap();

        write(&tmp, b"fresh").unwrap();
        assert_eq!(std::fs::read(&tmp).unwrap(), b"fresh");
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_symlink_is_replaced_not_followed() {
        let scratch = Scratch::new("link");
        let target = scratch.path("target");
        std::fs::write(&target, b"not yours").unwrap();
        let tmp = scratch.path("a.tmp");
        std::os::unix::fs::symlink(&target, &tmp).unwrap();

        write(&tmp, b"fresh").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"not yours");
        assert!(!tmp.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&tmp).unwrap(), b"fresh");
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_through_a_planted_symlink_leaves_its_target_alone() {
        let scratch = Scratch::new("copy");
        let source = scratch.path("source");
        std::fs::write(&source, b"copied").unwrap();
        let target = scratch.path("target");
        std::fs::write(&target, b"not yours").unwrap();
        let tmp = scratch.path("a.tmp");
        std::os::unix::fs::symlink(&target, &tmp).unwrap();

        assert_eq!(copy(&source, &tmp).unwrap(), 6);
        assert_eq!(std::fs::read(&target).unwrap(), b"not yours");
        assert_eq!(std::fs::read(&tmp).unwrap(), b"copied");
    }

    #[test]
    fn a_folder_at_a_file_name_is_refused() {
        let scratch = Scratch::new("folder");
        let tmp = scratch.path("a.tmp");
        std::fs::create_dir(&tmp).unwrap();

        assert!(file(&tmp).is_err());
        assert!(tmp.is_dir(), "and it's left where it was");
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_folder_link_is_replaced_by_a_real_folder() {
        let scratch = Scratch::new("dirlink");
        let target = scratch.path("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), b"not yours").unwrap();
        let staging = scratch.path("a.files");
        std::os::unix::fs::symlink(&target, &staging).unwrap();

        dir(&staging).unwrap();
        let meta = staging.symlink_metadata().unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink());
        assert_eq!(std::fs::read(target.join("keep")).unwrap(), b"not yours");
        assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);
    }

    #[test]
    fn a_stale_folder_is_emptied_and_a_stale_file_replaced() {
        let scratch = Scratch::new("staledir");
        let staging = scratch.path("a.files");
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("old"), b"old").unwrap();

        dir(&staging).unwrap();
        assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);

        let plain = scratch.path("b.files");
        std::fs::write(&plain, b"a file where a folder goes").unwrap();
        dir(&plain).unwrap();
        assert!(plain.is_dir());
    }
}
