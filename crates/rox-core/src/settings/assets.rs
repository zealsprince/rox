//! The live workspace's files: `<data>/assets/`, one flat file per entry in
//! the bundle's asset table. The table is cached the way the shader pool is,
//! so a render reads it without touching the disk, and every write goes
//! through [`Settings::update`] after the cache.
//!
//! Nothing here draws or decodes. Assets are data and never gate (ADR 23):
//! approval is over code.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, RwLock};

use sha2::{Digest, Sha256};

use crate::settings::{Asset, AssetKind, Settings, data_dir};

/// The most a name is slid to (`frame-99.png`) before an import gives up.
const VARIANTS: u32 = 99;

/// Leaves room for `-99` inside the 64 characters an id may hold.
const STEM_MAX: usize = 60;

static TABLE: LazyLock<RwLock<Vec<Asset>>> =
    LazyLock::new(|| RwLock::new(Settings::load().look.bundle.assets));

/// Bumped on every table change and on a file move the watch sees, so a
/// cached image checks staleness with one load.
static REV: AtomicU64 = AtomicU64::new(0);

/// Not created here: a look without assets never makes the folder.
pub fn dir() -> PathBuf {
    data_dir().join("assets")
}

pub fn kind_for(file: &str) -> Option<AssetKind> {
    let (_, ext) = file.rsplit_once('.')?;

    match ext.to_ascii_lowercase().as_str() {
        // What the image crate decodes with its default features, so no
        // image a 1.x shader carried is dropped on the way to a file.
        "png" | "apng" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tga" | "tif" | "tiff"
        | "ico" | "qoi" | "dds" | "hdr" | "exr" | "pnm" | "pbm" | "pgm" | "ppm" | "pam" | "ff" => {
            Some(AssetKind::Image)
        }
        "ttf" | "otf" => Some(AssetKind::Font),
        "mp4" | "webm" => Some(AssetKind::Video),
        "glb" => Some(AssetKind::Model),
        _ => None,
    }
}

/// Cloned out so a render never holds the lock and blocks an import.
pub fn all() -> Vec<Asset> {
    TABLE.read().unwrap().clone()
}

pub fn get(id: &str) -> Option<Asset> {
    TABLE
        .read()
        .unwrap()
        .iter()
        .find(|asset| asset.id == id)
        .cloned()
}

pub fn path(asset: &Asset) -> Option<PathBuf> {
    path_in(&dir(), asset)
}

pub fn read(id: &str) -> Result<Vec<u8>, String> {
    let asset = get(id).ok_or_else(|| format!("no asset {id}"))?;
    let path = path(&asset).ok_or_else(|| format!("{id} names no flat file"))?;

    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Copy a file in, hash it, add it to the live table. The same bytes
/// already in the table return that entry.
pub fn import(src: &Path) -> Result<Asset, String> {
    let file = src
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no usable file name", src.display()))?;
    let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;

    import_bytes(file, &bytes)
}

pub fn import_bytes(file: &str, bytes: &[u8]) -> Result<Asset, String> {
    let mut table = all();
    let asset = import_in(&dir(), &table, file, bytes)?;

    if !table.contains(&asset) {
        table.push(asset.clone());
        commit(table);
    }

    Ok(asset)
}

/// An import copied into the staging folder and hashed, waiting on
/// [`commit_staged`]. Dropping it deletes the copy.
pub struct Staged {
    file: String,
    tmp: PathBuf,
    sha256: String,
    bytes: u64,
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.tmp);
    }
}

/// An import's slow half, for a background thread. It streams `src` into a
/// staging folder beside the store while hashing it, so a video is never
/// held in memory, and it touches neither the table nor the live folder, so
/// an apply or a sweep can run meanwhile. Past `max_bytes` it refuses.
pub fn stage(src: &Path, max_bytes: u64) -> Result<Staged, String> {
    stage_in(&data_dir().join("assets.import"), src, max_bytes)
}

/// An import's quick half: a rename and the table write. Call it on the UI
/// thread, where apply swaps the table, so the two never interleave. The
/// same bytes already in the table return that entry.
pub fn commit_staged(staged: Staged) -> Result<Asset, String> {
    let mut table = all();
    let asset = commit_staged_in(&dir(), &table, &staged)?;

    if !table.contains(&asset) {
        table.push(asset.clone());
        commit(table);
    }

    Ok(asset)
}

pub fn remove(id: &str) -> bool {
    let mut table = all();
    let Some(index) = table.iter().position(|asset| asset.id == id) else {
        return false;
    };

    let asset = table.remove(index);
    commit(table);

    if let Some(path) = path(&asset)
        && let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("assets: removing {}: {e}", path.display());
    }

    true
}

/// Apply's half: `staged` holds the incoming workspace's files. The live
/// dir is swapped for it whole, then the table is set.
pub fn swap_in(staged: &Path, assets: Vec<Asset>) -> Result<(), String> {
    swap_in_at(&dir(), staged, assets, commit)
}

/// Bumps on any table change and when the watch sees a file move.
pub fn rev() -> u64 {
    REV.load(Ordering::Relaxed)
}

pub fn note_changed() {
    REV.fetch_add(1, Ordering::Relaxed);
}

/// A file edited in place stops matching its table entry, and a save checks
/// every file against the table. This rehashes the named files and takes
/// what's on disk, so the edit is what gets saved. A file that's gone keeps
/// its entry. Returns whether an entry changed.
pub fn refresh(files: &[String]) -> bool {
    let mut table = all();
    if !refresh_in(&dir(), &mut table, files) {
        return false;
    }

    commit(table);

    true
}

/// Delete files the table doesn't name.
pub fn sweep() {
    sweep_in(&dir(), &all());
}

/// Cache only, for tests and for an apply that already wrote the bundle.
pub fn note(assets: Vec<Asset>) {
    *TABLE.write().unwrap() = assets;
    note_changed();
}

fn commit(assets: Vec<Asset>) {
    note(assets.clone());
    Settings::update(move |s| {
        s.look.bundle.assets = assets;
    });
}

/// Whether `file` is one plain name. A bundle's JSON is untrusted, and this
/// is what keeps a workspace from naming a path outside the asset folder.
pub fn flat_name(file: &str) -> bool {
    !file.is_empty() && file != "." && file != ".." && !file.contains(['/', '\\', ':', '\0'])
}

pub(crate) fn path_in(dir: &Path, asset: &Asset) -> Option<PathBuf> {
    flat_name(&asset.file).then(|| dir.join(&asset.file))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes).as_slice())
}

/// Size and hash of a file on disk, streamed so a video isn't read whole.
pub fn sha256_file(path: &Path) -> Result<(u64, String), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());

    let mut file = std::fs::File::open(path).map_err(fail)?;
    let mut hasher = Sha256::new();
    let bytes = std::io::copy(&mut file, &mut hasher).map_err(fail)?;

    Ok((bytes, hex(hasher.finalize().as_slice())))
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A name as the store keeps it: lowercase ASCII, so two names never differ
/// only by case, and never one Windows reserves, so it packs on every
/// platform. None for an extension no kind claims.
pub(crate) fn store_name(file: &str) -> Option<String> {
    kind_for(file)?;
    let (stem, ext) = file.rsplit_once('.')?;

    let mut stem: String = stem
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '.' | '_' | '-') => c,
            _ => '-',
        })
        .collect();

    // An id has to start on a letter or digit, and a trailing dot is one of
    // the names Windows refuses.
    stem = stem
        .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
        .trim_end_matches(['.', '-'])
        .chars()
        .take(STEM_MAX)
        .collect();
    stem = stem.trim_end_matches(['.', '-']).to_string();
    if stem.is_empty() {
        stem = "asset".to_string();
    }

    if reserved(stem.split('.').next().unwrap_or(&stem)) {
        stem.push_str("-file");
    }

    Some(format!("{stem}.{}", ext.to_ascii_lowercase()))
}

/// The device names Windows keeps whatever the extension, the same set
/// rox-package refuses.
fn reserved(head: &str) -> bool {
    match head.to_ascii_uppercase().as_bytes() {
        b"CON" | b"PRN" | b"AUX" | b"NUL" => true,
        [b'C', b'O', b'M', digit] | [b'L', b'P', b'T', digit] => (b'1'..=b'9').contains(digit),
        _ => false,
    }
}

/// The first of `frame.png`, `frame-2.png` and on that `taken` passes over,
/// with its id. `name` is already a [`store_name`].
pub(crate) fn free_name(
    name: &str,
    taken: impl Fn(&str, &str) -> bool,
) -> Option<(String, String)> {
    let (stem, ext) = name.rsplit_once('.')?;

    (1..=VARIANTS).find_map(|variant| {
        let id = if variant == 1 {
            stem.to_string()
        } else {
            format!("{stem}-{variant}")
        };
        let file = format!("{id}.{ext}");

        (!taken(&id, &file)).then_some((id, file))
    })
}

/// Through a temp name in the same folder, so a crash never leaves half a
/// file under a name the table holds.
pub(crate) fn write_file_in(dir: &Path, file: &str, bytes: &[u8]) -> Result<(), String> {
    if !flat_name(file) {
        return Err(format!("{file} isn't a plain file name"));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{file}.{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let path = dir.join(file);

    let result = std::fs::File::create(&tmp)
        .and_then(|mut out| {
            out.write_all(bytes)?;
            out.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("{}: {e}", path.display()));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }

    result
}

/// [`import_bytes`] against a given folder and table, leaving both the
/// cache and the settings alone.
fn import_in(dir: &Path, table: &[Asset], file: &str, bytes: &[u8]) -> Result<Asset, String> {
    let sha256 = sha256_hex(bytes);
    if let Some(existing) = table.iter().find(|asset| asset.sha256 == sha256) {
        return Ok(existing.clone());
    }

    let (id, file, kind) = claim_in(dir, table, file)?;

    write_file_in(dir, &file, bytes)?;

    Ok(Asset {
        id,
        file,
        kind,
        sha256,
        bytes: bytes.len() as u64,
    })
}

/// A free id and file name for `file` against `dir` and `table`, with its kind.
fn claim_in(
    dir: &Path,
    table: &[Asset],
    file: &str,
) -> Result<(String, String, AssetKind), String> {
    let name = store_name(file).ok_or_else(|| format!("{file} isn't a kind of file rox reads"))?;
    let kind = kind_for(&name).unwrap_or_default();

    // A file on disk the table doesn't name could be another import part
    // way through, so it's taken too.
    let taken = |id: &str, candidate: &str| {
        table
            .iter()
            .any(|asset| asset.id == id || asset.file.eq_ignore_ascii_case(candidate))
            || dir.join(candidate).symlink_metadata().is_ok()
    };
    let (id, claimed) = free_name(&name, taken)
        .ok_or_else(|| format!("every name for {file} up to {VARIANTS} is taken"))?;

    Ok((id, claimed, kind))
}

/// Refuses past `max_bytes` on what was actually read, whatever the file's
/// size said when it was opened.
fn stage_in(staging: &Path, src: &Path, max_bytes: u64) -> Result<Staged, String> {
    let fail = |e: std::io::Error| format!("{}: {e}", src.display());

    let file = src
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no usable file name", src.display()))?;
    if store_name(file).is_none() {
        return Err(format!("{file} isn't a kind of file rox reads"));
    }

    let mut input = std::fs::File::open(src).map_err(fail)?;
    if input.metadata().map_err(fail)?.len() > max_bytes {
        return Err(format!("{file} is over {max_bytes} bytes"));
    }

    std::fs::create_dir_all(staging).map_err(|e| format!("{}: {e}", staging.display()))?;

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut staged = Staged {
        file: file.to_string(),
        tmp: staging.join(format!(
            "{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )),
        sha256: String::new(),
        bytes: 0,
    };

    // From here a failure drops `staged`, which deletes the partial copy.
    let mut out = std::fs::File::create(&staged.tmp).map_err(fail)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 256 * 1024];
    loop {
        let read = input.read(&mut buf).map_err(fail)?;
        if read == 0 {
            break;
        }

        staged.bytes += read as u64;
        if staged.bytes > max_bytes {
            return Err(format!("{file} is over {max_bytes} bytes"));
        }

        hasher.update(&buf[..read]);
        out.write_all(&buf[..read]).map_err(fail)?;
    }
    out.sync_all().map_err(fail)?;

    staged.sha256 = hex(hasher.finalize().as_slice());

    Ok(staged)
}

fn commit_staged_in(dir: &Path, table: &[Asset], staged: &Staged) -> Result<Asset, String> {
    if let Some(existing) = table.iter().find(|asset| asset.sha256 == staged.sha256) {
        return Ok(existing.clone());
    }

    let (id, file, kind) = claim_in(dir, table, &staged.file)?;

    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(&file);
    std::fs::rename(&staged.tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;

    Ok(Asset {
        id,
        file,
        kind,
        sha256: staged.sha256.clone(),
        bytes: staged.bytes,
    })
}

/// `set` runs between the second rename and the delete, so the folder and
/// the table change together.
fn swap_in_at(
    live: &Path,
    staged: &Path,
    assets: Vec<Asset>,
    set: impl FnOnce(Vec<Asset>),
) -> Result<(), String> {
    if !staged.is_dir() {
        return Err(format!("{} isn't a folder", staged.display()));
    }

    let mut aside = live.as_os_str().to_owned();
    aside.push(".old");
    let aside = PathBuf::from(aside);
    let _ = std::fs::remove_dir_all(&aside);

    let had_live = match std::fs::rename(live, &aside) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(format!("{}: {e}", live.display())),
    };

    if let Err(e) = std::fs::rename(staged, live) {
        if had_live && let Err(back) = std::fs::rename(&aside, live) {
            log::warn!("assets: putting {} back: {back}", live.display());
        }
        return Err(format!("{}: {e}", live.display()));
    }

    set(assets);

    if had_live && let Err(e) = std::fs::remove_dir_all(&aside) {
        log::warn!("assets: removing {}: {e}", aside.display());
    }

    Ok(())
}

fn refresh_in(dir: &Path, table: &mut [Asset], files: &[String]) -> bool {
    let mut changed = false;

    for asset in table.iter_mut().filter(|asset| files.contains(&asset.file)) {
        let Some(path) = path_in(dir, asset) else {
            continue;
        };
        let Ok((bytes, sha256)) = sha256_file(&path) else {
            continue;
        };

        if bytes != asset.bytes || sha256 != asset.sha256 {
            asset.bytes = bytes;
            asset.sha256 = sha256;
            changed = true;
        }
    }

    changed
}

fn sweep_in(dir: &Path, table: &[Asset]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| table.iter().any(|asset| asset.file == name))
        {
            continue;
        }

        // `file_type` doesn't follow a link, so a symlink goes as a file and
        // its target stays.
        let path = entry.path();
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        if let Err(e) = removed {
            log::warn!("assets: sweeping {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-assets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn asset(file: &str) -> Asset {
        Asset {
            id: file.split('.').next().unwrap().to_string(),
            file: file.to_string(),
            ..Asset::default()
        }
    }

    #[test]
    fn importing_the_same_bytes_twice_gives_one_entry() {
        let dir = scratch("twice");
        let mut table = Vec::new();

        let first = import_in(&dir, &table, "frame.png", b"one frame").unwrap();
        table.push(first.clone());
        let second = import_in(&dir, &table, "other.png", b"one frame").unwrap();

        assert_eq!(first, second);
        assert_eq!(first.id, "frame");
        assert_eq!(first.sha256, sha256_hex(b"one frame"));
        assert_eq!(first.bytes, 9);
        assert_eq!(std::fs::read(dir.join("frame.png")).unwrap(), b"one frame");
        assert!(!dir.join("other.png").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_taken_name_gets_the_next_one() {
        let dir = scratch("taken");
        let mut table = Vec::new();

        let first = import_in(&dir, &table, "Frame.PNG", b"first").unwrap();
        table.push(first.clone());
        let second = import_in(&dir, &table, "frame.png", b"second").unwrap();
        table.push(second.clone());

        assert_eq!(
            (first.id.as_str(), first.file.as_str()),
            ("frame", "frame.png")
        );
        assert_eq!(
            (second.id.as_str(), second.file.as_str()),
            ("frame-2", "frame-2.png")
        );
        assert_eq!(std::fs::read(dir.join("frame-2.png")).unwrap(), b"second");

        // A file the table doesn't name yet still holds its name.
        std::fs::write(dir.join("frame-3.png"), b"in flight").unwrap();
        let third = import_in(&dir, &table, "frame.png", b"third").unwrap();
        assert_eq!(third.file, "frame-4.png");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_staged_import_lands_once_and_leaves_no_copy() {
        let root = scratch("staged");
        let (dir, staging) = (root.join("assets"), root.join("assets.import"));
        let src = root.join("Plate.PNG");
        std::fs::write(&src, b"a plate").unwrap();
        let mut table = Vec::new();

        let staged = stage_in(&staging, &src, u64::MAX).unwrap();
        let first = commit_staged_in(&dir, &table, &staged).unwrap();
        drop(staged);
        table.push(first.clone());

        assert_eq!(
            (first.id.as_str(), first.file.as_str()),
            ("plate", "plate.png")
        );
        assert_eq!(first.kind, AssetKind::Image);
        assert_eq!(first.sha256, sha256_hex(b"a plate"));
        assert_eq!(first.bytes, 7);
        assert_eq!(std::fs::read(dir.join("plate.png")).unwrap(), b"a plate");

        // The same bytes again return the entry, and the second copy goes.
        let staged = stage_in(&staging, &src, u64::MAX).unwrap();
        let second = commit_staged_in(&dir, &table, &staged).unwrap();
        drop(staged);

        assert_eq!(first, second);
        assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn staging_refuses_past_its_cap_and_an_unknown_kind() {
        let root = scratch("staged-cap");
        let staging = root.join("assets.import");
        let font = root.join("big.ttf");
        std::fs::write(&font, [0u8; 64]).unwrap();
        let script = root.join("run.sh");
        std::fs::write(&script, b"#!/bin/sh").unwrap();

        assert!(stage_in(&staging, &font, 63).is_err());
        assert!(stage_in(&staging, &font, 64).is_ok());
        assert!(stage_in(&staging, &script, u64::MAX).is_err());
        assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        let dir = scratch("kind");

        assert!(import_in(&dir, &[], "run.sh", b"#!/bin/sh").is_err());
        assert_eq!(kind_for("Song.OTF"), Some(AssetKind::Font));
        assert_eq!(kind_for("clip.webm"), Some(AssetKind::Video));
        assert_eq!(kind_for("ship.glb"), Some(AssetKind::Model));
        assert_eq!(kind_for("png"), None);
        assert_eq!(kind_for("Old Plate.TGA"), Some(AssetKind::Image));
        assert_eq!(kind_for("plate.bmp"), Some(AssetKind::Image));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_names_pack_on_every_platform() {
        assert_eq!(store_name("My Frame.PNG").as_deref(), Some("my-frame.png"));
        assert_eq!(store_name("..hidden.png").as_deref(), Some("hidden.png"));
        assert_eq!(store_name("con.png").as_deref(), Some("con-file.png"));
        assert_eq!(store_name("Com3.ttf").as_deref(), Some("com3-file.ttf"));
        assert_eq!(store_name("!!!.gif").as_deref(), Some("asset.gif"));

        let long = format!("{}.png", "a".repeat(200));
        let name = store_name(&long).unwrap();
        assert_eq!(name.len(), STEM_MAX + 4);
    }

    #[test]
    fn a_file_name_with_a_separator_has_no_path() {
        let dir = Path::new("/data/assets");

        for file in [
            "../secrets.png",
            "nested/a.png",
            "..\\evil.png",
            "C:evil.png",
            "..",
            ".",
            "",
        ] {
            assert_eq!(path_in(dir, &asset(file)), None, "{file:?}");
        }
        assert_eq!(
            path_in(dir, &asset("frame.png")),
            Some(dir.join("frame.png"))
        );
    }

    #[test]
    fn swap_in_replaces_the_folder_and_the_table() {
        let root = scratch("swap");
        let live = root.join("assets");
        let staged = root.join("staged");

        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("old.png"), b"old").unwrap();
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("new.png"), b"new").unwrap();

        let mut set = None;
        swap_in_at(&live, &staged, vec![asset("new.png")], |table| {
            set = Some(table)
        })
        .unwrap();

        assert_eq!(set, Some(vec![asset("new.png")]));
        assert_eq!(std::fs::read(live.join("new.png")).unwrap(), b"new");
        assert!(!live.join("old.png").exists());
        assert!(!staged.exists());
        assert!(!root.join("assets.old").exists());

        // A missing stage leaves the live folder as it was.
        assert!(swap_in_at(&live, &staged, Vec::new(), |_| panic!("set")).is_err());
        assert!(live.join("new.png").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_edited_file_takes_its_new_hash() {
        let dir = scratch("refresh");
        let mut table = vec![
            import_in(&dir, &[], "plate.png", b"one").unwrap(),
            import_in(&dir, &[], "frame.png", b"frame").unwrap(),
        ];
        let frame = table[1].clone();
        let named = ["plate.png".to_string(), "gone.png".to_string()];

        assert!(!refresh_in(&dir, &mut table, &named), "nothing moved yet");

        std::fs::write(dir.join("plate.png"), b"one and two").unwrap();
        std::fs::write(dir.join("frame.png"), b"not asked about").unwrap();
        assert!(refresh_in(&dir, &mut table, &named));

        assert_eq!(table[0].bytes, 11);
        assert_eq!(table[0].sha256, sha256_hex(b"one and two"));
        assert_eq!(table[1], frame, "only the named files are rehashed");

        // A file that's gone keeps the entry it had.
        std::fs::remove_file(dir.join("plate.png")).unwrap();
        let before = table.clone();
        assert!(!refresh_in(&dir, &mut table, &named));
        assert_eq!(table, before);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sweep_removes_only_unnamed_files() {
        let dir = scratch("sweep");
        std::fs::write(dir.join("kept.png"), b"kept").unwrap();
        std::fs::write(dir.join("stray.png"), b"stray").unwrap();
        std::fs::write(dir.join(".half.png.tmp"), b"half").unwrap();
        std::fs::create_dir_all(dir.join("folder")).unwrap();

        sweep_in(&dir, &[asset("kept.png")]);

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["kept.png"]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
