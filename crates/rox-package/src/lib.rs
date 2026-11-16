//! Zip in and out for rox's package files, with every safety rule an archive
//! has to pass kept here and nowhere else. The workspace, object and plugin
//! formats sit on top and decide what an entry means. This crate knows
//! nothing of workspaces or plugins: it lists, reads, unpacks and packs.
//!
//! An archive is untrusted input, so every refusal below is a trust boundary.
//! Names are checked on the raw bytes the archive stores, sizes are counted
//! on the bytes actually read, and an unpack lands in a fresh folder that is
//! renamed into place only once every entry is out.
//!
//! It also holds [`fresh`], the exclusive create every staged write in rox
//! goes through, since this is the lowest crate those writes share.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

pub mod fresh;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Forward slashes, relative, every component normal.
    pub name: String,
    pub bytes: u64,
    /// Unix permission bits when the archive carries them.
    pub mode: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_entries: usize,
    /// Uncompressed, summed over every entry. `u64::MAX` for a format
    /// that warns instead of refusing.
    pub max_total: u64,
    /// Unpacked bytes over the archive's own size. 0 is unchecked.
    pub max_ratio: u32,
}

/// Entries in archive order. Refuses an unsafe path (absolute, `..`, a
/// backslash, not UTF-8, a component Windows reserves), a symlink entry,
/// a duplicate name, and two names that differ only by case.
pub fn list(archive: &Path, limits: &Limits) -> Result<Vec<Entry>, String> {
    let (mut zip, size) = open(archive)?;
    let entries = check(&mut zip, size, limits)?;

    Ok(entries.into_iter().map(|(_, entry)| entry).collect())
}

/// One entry's bytes. Stops at `max` rather than trusting the stated size.
pub fn read(archive: &Path, name: &str, max: u64) -> Result<Vec<u8>, String> {
    let (mut zip, _) = open(archive)?;
    if zip.has_duplicate_names() {
        return Err("the archive holds a duplicate name".to_string());
    }

    // Looked up on the raw name, since `by_name` falls back to matching a
    // CP437 decode. `by_name_raw` would hand back the compressed bytes.
    let index = zip
        .index_for_name_raw(name.as_bytes())
        .ok_or_else(|| format!("the archive holds no {name}"))?;
    let file = zip.by_index(index).map_err(|e| format!("{name}: {e}"))?;
    if file.is_symlink() {
        return Err(format!("{name} is a symlink"));
    }

    let mut bytes = Vec::new();
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{name}: {e}"))?;
    if bytes.len() as u64 > max {
        return Err(format!("{name} is larger than {max} bytes"));
    }

    Ok(bytes)
}

/// Every entry `keep` accepts, into `dest`. Extracts into `dest` with a
/// `.incoming` suffix and renames, so a failed unpack leaves nothing.
/// With `exec` set, applies `mode & 0o755` on unix. Without it every file
/// is plain data.
pub fn unpack(
    archive: &Path,
    dest: &Path,
    limits: &Limits,
    exec: bool,
    keep: &dyn Fn(&Entry) -> bool,
) -> Result<Vec<Entry>, String> {
    let (mut zip, size) = open(archive)?;
    let entries = check(&mut zip, size, limits)?;

    // The caller decides what replacing an existing folder means.
    if dest.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", dest.display()));
    }

    let incoming = suffixed(dest, ".incoming");

    let result = extract(&mut zip, size, limits, exec, keep, entries, &incoming).and_then(|kept| {
        std::fs::rename(&incoming, dest)
            .map(|()| kept)
            .map_err(|e| format!("{}: {e}", dest.display()))
    });
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&incoming);
    }

    result
}

/// `files` as `(entry name, source path)`. Entries are written sorted by
/// name with a fixed timestamp, so the same input packs to the same bytes.
/// Temp then rename.
pub fn pack(archive: &Path, files: &[(String, PathBuf)]) -> Result<(), String> {
    let mut sorted: Vec<&(String, PathBuf)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    // Never write an archive that `list` would refuse.
    let mut folded = HashSet::new();
    for (name, _) in &sorted {
        safe_name(name)?;
        if !folded.insert(name.to_lowercase()) {
            return Err(format!("{name} differs from another name only by case"));
        }
    }

    let tmp = suffixed(archive, ".tmp");
    let result = write_archive(&tmp, &sorted).and_then(|()| {
        std::fs::rename(&tmp, archive).map_err(|e| format!("{}: {e}", archive.display()))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }

    result
}

fn open(archive: &Path) -> Result<(ZipArchive<BufReader<File>>, u64), String> {
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", archive.display());

    let file = File::open(archive).map_err(|e| fail(&e))?;
    let size = file.metadata().map_err(|e| fail(&e))?.len();
    let zip = ZipArchive::new(BufReader::new(file)).map_err(|e| fail(&e))?;

    Ok((zip, size))
}

/// The walk `list` and `unpack` share, so the two can't disagree on what
/// passes. Each file entry comes back with its index in the archive.
fn check<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    size: u64,
    limits: &Limits,
) -> Result<Vec<(usize, Entry)>, String> {
    // zip keys its entries by name, so a duplicate has already collapsed
    // into one entry by now and the walk below can't see it.
    if zip.has_duplicate_names() {
        return Err("the archive holds a duplicate name".to_string());
    }
    if zip.len() > limits.max_entries {
        return Err(format!(
            "the archive holds {} entries, past its limit of {}",
            zip.len(),
            limits.max_entries
        ));
    }

    let mut entries = Vec::new();
    let mut folded = HashSet::new();
    let mut total: u64 = 0;

    for index in 0..zip.len() {
        let file = zip
            .by_index(index)
            .map_err(|e| format!("entry {index}: {e}"))?;

        // `name()` falls back to CP437 and `enclosed_name()` quietly repairs
        // the very paths refused here, so the raw bytes are what get checked.
        let raw = std::str::from_utf8(file.name_raw())
            .map_err(|_| format!("entry {index} has a name that isn't UTF-8"))?;
        if file.is_dir() {
            safe_name(raw.strip_suffix('/').unwrap_or(raw))?;
            continue;
        }

        let name = safe_name(raw)?;
        if file.is_symlink() {
            return Err(format!("{name} is a symlink"));
        }

        // Windows and a default macOS volume fold these onto one file.
        if !folded.insert(name.to_lowercase()) {
            return Err(format!("{name} differs from another name only by case"));
        }

        total = total.saturating_add(file.size());
        if let Some(reason) = over(total, size, limits) {
            return Err(reason);
        }

        let mode = file.unix_mode().map(|mode| mode & 0o7777);
        entries.push((
            index,
            Entry {
                name,
                bytes: file.size(),
                mode,
            },
        ));
    }

    Ok(entries)
}

fn extract<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    size: u64,
    limits: &Limits,
    exec: bool,
    keep: &dyn Fn(&Entry) -> bool,
    entries: Vec<(usize, Entry)>,
    incoming: &Path,
) -> Result<Vec<Entry>, String> {
    // Replaces a leftover from an unpack that died part way through.
    fresh::dir(incoming).map_err(|e| format!("{}: {e}", incoming.display()))?;

    let budget = budget(size, limits);
    let mut total: u64 = 0;
    let mut kept = Vec::new();

    for (index, entry) in entries {
        if !keep(&entry) {
            continue;
        }

        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", entry.name);
        let out = entry
            .name
            .split('/')
            .fold(incoming.to_path_buf(), |path, part| path.join(part));
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| fail(&e))?;
        }

        let mut sink = File::options()
            .write(true)
            .create_new(true)
            .open(&out)
            .map_err(|e| fail(&e))?;
        let mut file = zip.by_index(index).map_err(|e| fail(&e))?;

        // The stated size is never trusted: read at most one byte past what
        // the limits leave, and count what actually came out.
        let room = (budget - total).saturating_add(1);
        let written =
            std::io::copy(&mut (&mut file).take(room), &mut sink).map_err(|e| fail(&e))?;
        total = total.saturating_add(written);
        if let Some(reason) = over(total, size, limits) {
            return Err(reason);
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            // Only a plugin package carries anything that runs.
            let mode = match entry.mode {
                Some(mode) if exec => mode & 0o755,
                _ => 0o644,
            };
            sink.set_permissions(std::fs::Permissions::from_mode(mode))
                .map_err(|e| fail(&e))?;
        }

        kept.push(Entry {
            bytes: written,
            ..entry
        });
    }

    #[cfg(not(unix))]
    let _ = exec;

    Ok(kept)
}

fn write_archive(tmp: &Path, files: &[&(String, PathBuf)]) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", tmp.display());

    let sink = fresh::file(tmp).map_err(|e| fail(&e))?;
    let mut zip = ZipWriter::new(BufWriter::new(sink));

    for (name, source) in files {
        let mut input = File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;

        // The fixed time is what makes two packs of one folder byte-identical.
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(DateTime::DEFAULT);
        #[cfg(unix)]
        let options = {
            use std::os::unix::fs::PermissionsExt;

            let meta = input
                .metadata()
                .map_err(|e| format!("{}: {e}", source.display()))?;
            options.unix_permissions(meta.permissions().mode())
        };

        zip.start_file(name.as_str(), options)
            .map_err(|e| format!("{name}: {e}"))?;
        std::io::copy(&mut input, &mut zip).map_err(|e| format!("{name}: {e}"))?;
    }

    let mut out = zip.finish().map_err(|e| fail(&e))?;
    out.flush().map_err(|e| fail(&e))?;
    out.get_ref().sync_all().map_err(|e| fail(&e))?;

    Ok(())
}

/// The same rules on every platform, so a package made on Linux opens the
/// same way on Windows.
fn safe_name(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("the archive holds an empty name".to_string());
    }
    if raw.contains('\\') {
        return Err(format!("{raw} holds a backslash"));
    }

    if raw.starts_with('/') {
        return Err(format!("{raw} is an absolute path"));
    }

    // A colon is a drive prefix or an NTFS stream, and joining a drive
    // relative name like `C:x` onto a folder on Windows escapes it.
    if raw.contains(':') {
        return Err(format!("{raw} holds a colon"));
    }
    if raw.contains('\0') {
        return Err(format!("{raw} holds a NUL byte"));
    }

    for part in raw.split('/') {
        match part {
            "" => return Err(format!("{raw} holds an empty component")),
            "." | ".." => return Err(format!("{raw} holds a {part} component")),
            _ => {}
        }

        if reserved(part) {
            return Err(format!("{raw} holds {part}, a name Windows reserves"));
        }
    }

    Ok(raw.to_string())
}

fn reserved(part: &str) -> bool {
    if part.ends_with('.') || part.ends_with(' ') {
        return true;
    }

    let stem = part.split('.').next().unwrap_or(part).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }

    match upper.as_bytes() {
        [b'C', b'O', b'M', digit] | [b'L', b'P', b'T', digit] => (b'1'..=b'9').contains(digit),
        _ => false,
    }
}

fn over(total: u64, size: u64, limits: &Limits) -> Option<String> {
    if total > limits.max_total {
        return Some(format!(
            "the archive unpacks past its limit of {} bytes",
            limits.max_total
        ));
    }
    if limits.max_ratio > 0 && total > size.saturating_mul(u64::from(limits.max_ratio)) {
        return Some(format!(
            "the archive unpacks to more than {} times its own size",
            limits.max_ratio
        ));
    }

    None
}

fn budget(size: u64, limits: &Limits) -> u64 {
    if limits.max_ratio == 0 {
        return limits.max_total;
    }

    limits
        .max_total
        .min(size.saturating_mul(u64::from(limits.max_ratio)))
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("rox-package-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            Scratch(dir)
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }

        fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let path = self.path(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();

            path
        }

        /// Writes `files` under `src/` and packs them as `name`.
        fn pack(&self, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
            let pairs: Vec<(String, PathBuf)> = files
                .iter()
                .map(|(rel, bytes)| (rel.to_string(), self.write(&format!("src/{rel}"), bytes)))
                .collect();
            let archive = self.path(name);
            pack(&archive, &pairs).expect("the files pack");

            archive
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn roomy() -> Limits {
        Limits {
            max_entries: 100,
            max_total: u64::MAX,
            max_ratio: 0,
        }
    }

    fn everything(_: &Entry) -> bool {
        true
    }

    /// `ZipWriter::start_file` stores a name exactly as given, which is
    /// what lets these tests write names `pack` would refuse.
    fn raw_zip(path: &Path, method: CompressionMethod, entries: &[(&str, &[u8])]) {
        let mut zip = ZipWriter::new(File::create(path).unwrap());
        let options = SimpleFileOptions::default().compression_method(method);
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn refused(scratch: &Scratch, name: &str) -> String {
        let archive = scratch.path("raw.zip");
        raw_zip(&archive, CompressionMethod::Stored, &[(name, b"x")]);

        list(&archive, &roomy()).expect_err(name)
    }

    #[test]
    fn a_pack_unpacks_to_the_same_files() {
        let scratch = Scratch::new("roundtrip");
        let files: [(&str, &[u8]); 3] = [
            ("workspace.json", b"{}"),
            ("assets/cover.png", b"png bytes"),
            ("assets/deep/font.ttf", &[0u8, 1, 2, 255]),
        ];
        let archive = scratch.pack("a.zip", &files);

        let dest = scratch.path("out");
        let entries = unpack(&archive, &dest, &roomy(), false, &everything).unwrap();
        assert_eq!(entries.len(), 3);

        for (rel, bytes) in files {
            assert_eq!(std::fs::read(dest.join(rel)).unwrap(), bytes, "{rel}");
        }
        assert!(!suffixed(&dest, ".incoming").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_planted_at_the_pack_temp_name_is_not_followed() {
        let scratch = Scratch::new("packlink");
        let target = scratch.write("target", b"not yours");
        let archive = scratch.path("a.zip");
        std::os::unix::fs::symlink(&target, suffixed(&archive, ".tmp")).unwrap();

        let source = scratch.write("src/a.txt", b"alpha");
        pack(&archive, &[("a.txt".into(), source)]).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"not yours");
        assert_eq!(read(&archive, "a.txt", 64).unwrap(), b"alpha");
        assert!(suffixed(&archive, ".tmp").symlink_metadata().is_err());
    }

    #[test]
    fn a_stale_pack_temp_file_is_replaced() {
        let scratch = Scratch::new("packstale");
        let archive = scratch.path("a.zip");
        scratch.write("a.zip.tmp", &[0xff; 4096]);

        let source = scratch.write("src/a.txt", b"alpha");
        pack(&archive, &[("a.txt".into(), source)]).unwrap();

        assert_eq!(read(&archive, "a.txt", 64).unwrap(), b"alpha");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_planted_at_the_unpack_stage_is_not_followed() {
        let scratch = Scratch::new("unpacklink");
        let archive = scratch.pack("a.zip", &[("workspace.json", b"{}")]);
        let elsewhere = scratch.path("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let dest = scratch.path("out");
        std::os::unix::fs::symlink(&elsewhere, suffixed(&dest, ".incoming")).unwrap();

        unpack(&archive, &dest, &roomy(), false, &everything).unwrap();

        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
        assert_eq!(std::fs::read(dest.join("workspace.json")).unwrap(), b"{}");
        assert!(!dest.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn packing_twice_gives_the_same_bytes() {
        let scratch = Scratch::new("stable");
        let a = scratch.write("src/a.txt", b"alpha");
        let b = scratch.write("src/b/b.txt", b"beta");

        let first = scratch.path("first.zip");
        pack(
            &first,
            &[("a.txt".into(), a.clone()), ("b/b.txt".into(), b.clone())],
        )
        .unwrap();

        // Fresh mtimes and the reverse order still pack to the same bytes.
        std::fs::write(&a, b"alpha").unwrap();
        let second = scratch.path("second.zip");
        pack(&second, &[("b/b.txt".into(), b), ("a.txt".into(), a)]).unwrap();

        assert_eq!(
            std::fs::read(first).unwrap(),
            std::fs::read(second).unwrap()
        );
    }

    #[test]
    fn a_parent_path_is_refused() {
        let scratch = Scratch::new("parent");
        for name in ["../evil.txt", "assets/../../evil.txt", "a/../b.txt"] {
            assert!(refused(&scratch, name).contains(".."), "{name}");
        }
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let scratch = Scratch::new("absolute");
        assert!(refused(&scratch, "/etc/evil").contains("absolute"));
    }

    #[test]
    fn a_colon_is_refused() {
        let scratch = Scratch::new("colon");
        for name in ["C:/evil.txt", "C:evil.txt", "a.txt:stream"] {
            assert!(refused(&scratch, name).contains("colon"), "{name}");
        }
    }

    #[test]
    fn a_backslash_is_refused() {
        let scratch = Scratch::new("backslash");
        for name in ["assets\\a.png", "..\\evil.txt"] {
            assert!(refused(&scratch, name).contains("backslash"), "{name}");
        }
    }

    #[test]
    fn a_symlink_entry_is_refused() {
        let scratch = Scratch::new("symlink");
        let archive = scratch.path("link.zip");

        let mut zip = ZipWriter::new(File::create(&archive).unwrap());
        zip.add_symlink("link", "/etc/passwd", SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();

        assert_eq!(list(&archive, &roomy()).unwrap_err(), "link is a symlink");
    }

    #[test]
    fn a_duplicate_name_is_refused() {
        let scratch = Scratch::new("duplicate");
        let archive = scratch.path("dup.zip");
        raw_zip(
            &archive,
            CompressionMethod::Stored,
            &[("a.txt", b"one"), ("b.txt", b"two")],
        );

        // Same length, so every offset in the local headers and the central
        // directory still holds.
        let bytes = std::fs::read(&archive).unwrap();
        let mut patched = Vec::with_capacity(bytes.len());
        let mut rest = bytes.as_slice();
        while !rest.is_empty() {
            if rest.starts_with(b"b.txt") {
                patched.extend_from_slice(b"a.txt");
                rest = &rest[5..];
            } else {
                patched.push(rest[0]);
                rest = &rest[1..];
            }
        }
        std::fs::write(&archive, patched).unwrap();

        let err = list(&archive, &roomy()).unwrap_err();
        assert!(err.contains("duplicate"), "{err}");

        let dest = scratch.path("out");
        assert!(unpack(&archive, &dest, &roomy(), false, &everything).is_err());
        assert!(read(&archive, "a.txt", 1024).is_err());
    }

    #[test]
    fn read_stops_at_its_limit() {
        let scratch = Scratch::new("read");
        let archive = scratch.pack("r.zip", &[("workspace.json", &[b'x'; 100])]);

        assert_eq!(read(&archive, "workspace.json", 100).unwrap().len(), 100);

        let err = read(&archive, "workspace.json", 99).unwrap_err();
        assert!(err.contains("larger than 99"), "{err}");
    }

    #[test]
    fn unpack_stops_at_the_real_total() {
        let scratch = Scratch::new("understated");
        let archive = scratch.path("lie.zip");
        raw_zip(
            &archive,
            CompressionMethod::Deflated,
            &[("big.bin", &[0u8; 64 * 1024])],
        );

        // State 16 bytes in both the local header and the central directory,
        // so the archive opens and only the data says otherwise.
        let mut bytes = std::fs::read(&archive).unwrap();
        bytes[22..26].copy_from_slice(&16u32.to_le_bytes());
        let central = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&16u32.to_le_bytes());
        std::fs::write(&archive, bytes).unwrap();

        let limits = Limits {
            max_entries: 10,
            max_total: 1024,
            max_ratio: 0,
        };
        assert_eq!(list(&archive, &limits).unwrap()[0].bytes, 16);

        let dest = scratch.path("out");
        assert!(unpack(&archive, &dest, &limits, false, &everything).is_err());
        assert!(!dest.exists());
        assert!(!suffixed(&dest, ".incoming").exists());
    }

    #[test]
    fn a_failed_unpack_leaves_nothing() {
        let scratch = Scratch::new("failed");
        let archive = scratch.path("clash.zip");

        // `a` lands as a file, then `a/b.txt` needs it to be a folder.
        raw_zip(
            &archive,
            CompressionMethod::Stored,
            &[("a", b"file"), ("a/b.txt", b"nested")],
        );

        let dest = scratch.path("out");
        assert!(unpack(&archive, &dest, &roomy(), false, &everything).is_err());
        assert!(!dest.exists());
        assert!(!suffixed(&dest, ".incoming").exists());
    }

    #[test]
    fn unpack_keeps_only_what_the_filter_takes() {
        let scratch = Scratch::new("filter");
        let archive = scratch.pack(
            "f.zip",
            &[
                ("workspace.json", b"{}"),
                ("assets/a.png", b"png"),
                ("stray.exe", b"MZ"),
                ("_rox/sig", b"sig"),
            ],
        );

        let dest = scratch.path("out");
        let keep =
            |entry: &Entry| entry.name == "workspace.json" || entry.name.starts_with("assets/");
        let entries = unpack(&archive, &dest, &roomy(), false, &keep).unwrap();

        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["assets/a.png", "workspace.json"]);
        assert!(dest.join("workspace.json").exists());
        assert!(dest.join("assets/a.png").exists());
        assert!(!dest.join("stray.exe").exists());
        assert!(!dest.join("_rox").exists());
    }

    #[cfg(unix)]
    fn packed_script(scratch: &Scratch) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = scratch.write("src/run.sh", b"#!/bin/sh\n");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let archive = scratch.path("p.zip");
        pack(&archive, &[("run.sh".into(), script)]).unwrap();

        archive
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[cfg(unix)]
    #[test]
    fn the_executable_bit_survives() {
        let scratch = Scratch::new("exec");
        let archive = packed_script(&scratch);

        let dest = scratch.path("out");
        unpack(&archive, &dest, &roomy(), true, &everything).unwrap();
        assert_eq!(mode_of(&dest.join("run.sh")), 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn the_executable_bit_is_dropped_without_exec() {
        let scratch = Scratch::new("noexec");
        let archive = packed_script(&scratch);

        let dest = scratch.path("out");
        unpack(&archive, &dest, &roomy(), false, &everything).unwrap();
        assert_eq!(mode_of(&dest.join("run.sh")), 0o644);
    }

    #[test]
    fn two_names_differing_by_case_are_refused() {
        let scratch = Scratch::new("case");
        let archive = scratch.path("case.zip");
        raw_zip(
            &archive,
            CompressionMethod::Stored,
            &[("assets/Cover.png", b"one"), ("assets/cover.PNG", b"two")],
        );

        let err = list(&archive, &roomy()).unwrap_err();
        assert!(err.contains("only by case"), "{err}");
    }

    #[test]
    fn a_reserved_windows_name_is_refused_everywhere() {
        let scratch = Scratch::new("reserved");
        for name in [
            "CON",
            "assets/nul.png",
            "Com1.txt",
            "lpt9.log",
            "aux/x.json",
            "prn .txt",
            "dot.",
            "space ",
        ] {
            assert!(
                refused(&scratch, name).contains("Windows reserves"),
                "{name}"
            );
        }

        // Near misses stay legal.
        for name in ["console.txt", "com0.txt", "lpt10", "nullable.json"] {
            let archive = scratch.path("ok.zip");
            raw_zip(&archive, CompressionMethod::Stored, &[(name, b"x")]);
            assert!(list(&archive, &roomy()).is_ok(), "{name}");
        }
    }

    #[test]
    fn an_archive_past_its_ratio_is_refused() {
        let scratch = Scratch::new("ratio");
        let archive = scratch.pack("bomb.zip", &[("zeros.bin", &vec![0u8; 4 << 20])]);
        assert!(std::fs::metadata(&archive).unwrap().len() < 16 * 1024);

        let limits = Limits {
            max_entries: 10,
            max_total: u64::MAX,
            max_ratio: 100,
        };
        let err = list(&archive, &limits).unwrap_err();
        assert!(err.contains("100 times"), "{err}");

        let dest = scratch.path("out");
        assert!(unpack(&archive, &dest, &limits, false, &everything).is_err());
        assert!(!dest.exists());
    }
}
