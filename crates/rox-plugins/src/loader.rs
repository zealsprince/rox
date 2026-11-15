//! The plugins folder (ADR 30): one subfolder per plugin, named after its id,
//! or one `.roxp` per plugin (ADR 32), unpacked into the cache and loaded
//! from there. A scan reads each manifest, hashes each folder and looks for
//! the programs it lists. It runs nothing. A plugin that doesn't load still
//! comes back, with its reason, so the Plugins page can say what's wrong
//! with it.
//!
//! The watch follows the folder so a plugin dropped in or edited shows up
//! without a restart. Events are hints to scan again, never a diff.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use notify_debouncer_full::notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::hash;
use crate::manifest::{self, Manifest};
use crate::package;

const DEBOUNCE: Duration = Duration::from_millis(500);

/// One scan unpacks and sweeps the cache at a time, so two can't trample
/// each other's copies.
static UNPACKING: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Folder,
    /// The `.roxp` the plugin was unpacked from.
    Package(PathBuf),
}

/// One plugin under the plugins folder, as the last scan found it.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    /// The folder's name, or the id a package's manifest gives. The manifest
    /// it loads from has to match it.
    pub id: String,
    /// The folder, or the unpacked copy. A package that never unpacked
    /// points at its archive.
    pub dir: PathBuf,
    pub origin: Origin,
    /// None when `plugin.json` is missing or doesn't parse.
    pub manifest: Option<Manifest>,
    /// `plugin.json` as written, which an approval stores for the next diff.
    pub document: serde_json::Value,
    /// Empty when the folder couldn't be hashed.
    pub hash: String,
    /// Each program the manifest lists, and whether it's on the plugin's
    /// search path.
    pub programs: Vec<(String, bool)>,
    /// Why this folder can't run, if it can't.
    pub error: Option<String>,
    /// The source icon's SVG, checked by `manifest::icon_for`.
    pub icon: Option<Vec<u8>>,
    /// Each action's icon by action id, checked the same way.
    pub action_icons: Vec<(String, Vec<u8>)>,
}

impl Loaded {
    /// Whether the host may start it, approval aside.
    pub fn runs(&self) -> bool {
        self.manifest.is_some() && self.error.is_none()
    }

    fn failed(id: String, dir: PathBuf, origin: Origin, error: String) -> Loaded {
        Loaded {
            id,
            dir,
            origin,
            manifest: None,
            document: serde_json::Value::Null,
            hash: String::new(),
            programs: Vec::new(),
            error: Some(error),
            icon: None,
            action_icons: Vec::new(),
        }
    }
}

/// Every folder, then every `*.roxp`. A folder shadows a package with
/// its id. A second package with an id already taken fails to load and
/// says which file holds it. Sorted by id, with the plugin that holds an id
/// ahead of any row refused for it. A missing `dir` scans to nothing; stray
/// files beside the plugins are skipped.
pub fn scan(dir: &Path, cache: &Path) -> Vec<Loaded> {
    let mut folders = Vec::new();
    let mut archives = Vec::new();
    let mut refused = Vec::new();

    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let is_package = path.extension().is_some_and(|ext| ext == package::EXT);

        // `file_type` doesn't follow links, so a linked folder shows as one.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() && is_package {
            let stem = stem_of(&path);
            let origin = Origin::Package(path.clone());
            refused.push(Loaded::failed(
                stem,
                path,
                origin,
                "the package is a symlink".into(),
            ));
        } else if kind.is_symlink() {
            let error = "the folder is a symlink".into();
            folders.push(Loaded::failed(name, path, Origin::Folder, error));
        } else if kind.is_dir() {
            folders.push(load(name, path, Origin::Folder));
        } else if kind.is_file() && is_package {
            archives.push(path);
        }
    }

    // "Earlier" has to mean the same thing on every scan.
    archives.sort();

    let mut claimed: Vec<Loaded> = Vec::new();
    let mut kept: Vec<(String, String)> = Vec::new();
    let unpacking = UNPACKING.lock().unwrap_or_else(|e| e.into_inner());

    for archive in archives {
        let origin = Origin::Package(archive.clone());
        let peek = match package::peek(&archive) {
            Ok(peek) => peek,
            Err(e) => {
                refused.push(Loaded::failed(stem_of(&archive), archive, origin, e));
                continue;
            }
        };

        let id = peek.manifest.id;
        if let Some(holder) = folders.iter().chain(&claimed).find(|l| l.id == id) {
            let error = format!("{} already holds plugin {id}", holder_name(holder));
            refused.push(Loaded::failed(id, archive, origin, error));
            continue;
        }

        kept.push((id.clone(), peek.hash[..16].to_string()));
        claimed.push(match package::unpack(&archive, cache) {
            Ok(copy) => load(id, copy, origin),
            Err(e) => Loaded::failed(id, archive, origin, e),
        });
    }

    sweep(cache, &kept);
    drop(unpacking);

    // A stable sort, so the holder of an id stays ahead of what it refused.
    let mut found = folders;
    found.extend(claimed);
    found.extend(refused);
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

fn stem_of(archive: &Path) -> String {
    archive
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn holder_name(holder: &Loaded) -> String {
    match &holder.origin {
        Origin::Folder => format!("the folder {}", holder.id),
        Origin::Package(archive) => archive
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// Drops every copy in the cache but the ones `kept` names as `(id, first 16
/// of hash)`. A copy that won't go, held open on Windows or by a plugin
/// still running from it, waits for the next scan.
fn sweep(cache: &Path, kept: &[(String, String)]) {
    let discard = |path: &Path| {
        if let Err(e) = package::clear(path) {
            log::debug!("plugins: leaving {} for the next scan: {e}", path.display());
        }
    };

    for home in std::fs::read_dir(cache).into_iter().flatten().flatten() {
        let name = home.file_name();
        let wanted: Vec<&str> = kept
            .iter()
            .filter(|(id, _)| name == id.as_str())
            .map(|(_, short)| short.as_str())
            .collect();

        let is_dir = home.file_type().is_ok_and(|kind| kind.is_dir());
        if wanted.is_empty() || !is_dir {
            discard(&home.path());
            continue;
        }

        for copy in std::fs::read_dir(home.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let keep = copy.file_type().is_ok_and(|kind| kind.is_dir())
                && wanted.iter().any(|short| copy.file_name() == *short);
            if !keep {
                discard(&copy.path());
            }
        }
    }
}

pub(crate) fn load(id: String, dir: PathBuf, origin: Origin) -> Loaded {
    let manifest = match manifest::load(&dir) {
        Ok(manifest) => manifest,
        Err(e) => return Loaded::failed(id, dir, origin, e),
    };

    // A second parse of a file just read whole, kept apart from the typed
    // one so the stored copy has every key the author wrote.
    let document = std::fs::read_to_string(dir.join(manifest::FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);

    let programs = manifest
        .programs
        .iter()
        .map(|program| (program.clone(), manifest::program_found(program, &dir)))
        .collect();

    let (hash, hash_error) = match hash::folder_hash(&dir) {
        Ok(hash) => (hash, None),
        Err(e) => (String::new(), Some(e)),
    };

    let icon = manifest::icon_for(&manifest, &dir);
    let action_icons = manifest::action_icons_for(&manifest, &dir);

    // The source id is fixed by the manifest, so it has to match the id the
    // plugin lists under: a folder's name, or for a package's copy, which is
    // named for its hash, the id the archive's manifest gave.
    let error = match manifest.id == id {
        false => Some(format!(
            "the folder is named {id} but holds plugin {}",
            manifest.id
        )),
        true => reserved_error(&dir)
            .or(hash_error)
            .or_else(|| manifest::entry_for(&manifest, &dir).err())
            .or_else(|| icon.as_ref().err().cloned())
            .or_else(|| action_icons.as_ref().err().cloned()),
    };

    Loaded {
        id,
        dir,
        origin,
        manifest: Some(manifest),
        document,
        hash,
        programs,
        error,
        icon: icon.ok().flatten(),
        action_icons: action_icons.unwrap_or_default(),
    }
}

/// `_rox/` would sit outside the trust hash an archive gives, yet plugin code
/// could still read it, so a folder holding one never loads.
fn reserved_error(dir: &Path) -> Option<String> {
    let holds = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(hash::RESERVED))
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
        });

    holds.then(|| {
        format!(
            "the folder holds {}/, a name kept for signatures",
            hash::RESERVED
        )
    })
}

/// Follows the plugins folder, and its parent until the folder exists: rox
/// never creates it on its own. Dropping this stops the watch.
pub struct Watch {
    dir: PathBuf,
    debouncer: Mutex<Debouncer<RecommendedWatcher, RecommendedCache>>,
    /// Whether the folder itself is watched.
    armed: Mutex<bool>,
}

impl Watch {
    /// `changed` runs on the watcher's thread after each settled burst. Call
    /// [`Watch::arm`] from it, or after anything that creates the folder.
    pub fn new(dir: &Path, changed: impl Fn() + Send + 'static) -> Result<Watch, String> {
        let parent = dir
            .parent()
            .ok_or_else(|| format!("{} has no parent", dir.display()))?
            .to_path_buf();
        // FSEvents reports paths with symlinks resolved, inotify as watched,
        // so an event can name the folder either way. The folder may not
        // exist yet, so its parent is what resolves.
        let resolved = parent
            .canonicalize()
            .ok()
            .zip(dir.file_name())
            .map(|(parent, name)| parent.join(name));
        let own: Vec<PathBuf> = std::iter::once(dir.to_path_buf()).chain(resolved).collect();

        let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            let Ok(batch) = result else {
                return;
            };

            // A scan opens every file to hash it, which inotify reports as an
            // access; answering those would scan forever.
            let ours = batch.iter().any(|event| {
                !matches!(event.kind, EventKind::Access(_))
                    && event
                        .paths
                        .iter()
                        .any(|path| own.iter().any(|own| path.starts_with(own)))
            });
            if ours {
                changed();
            }
        })
        .map_err(|e| e.to_string())?;

        debouncer
            .watch(&parent, RecursiveMode::NonRecursive)
            .map_err(|e| format!("{}: {e}", parent.display()))?;

        let watch = Watch {
            dir: dir.to_path_buf(),
            debouncer: Mutex::new(debouncer),
            armed: Mutex::new(false),
        };
        watch.arm();

        Ok(watch)
    }

    /// Watches the folder itself once it exists, and notes when it's gone so
    /// a new one is picked up.
    pub fn arm(&self) {
        let (Ok(mut armed), Ok(mut debouncer)) = (self.armed.lock(), self.debouncer.lock()) else {
            return;
        };

        let exists = self.dir.is_dir();
        if exists && !*armed {
            match debouncer.watch(&self.dir, RecursiveMode::Recursive) {
                Ok(()) => *armed = true,
                Err(e) => log::warn!("plugins: not watching {}: {e}", self.dir.display()),
            }
        } else if !exists && *armed {
            let _ = debouncer.unwatch(&self.dir);
            *armed = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir()
                .join(format!("rox-plugins-loader-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            Scratch(dir)
        }

        /// Outside the scanned folder, as the real cache is.
        fn cache(&self) -> PathBuf {
            self.0.with_extension("cache")
        }

        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();

            path
        }

        /// A plugin whose entry is a native build for whatever runs the test,
        /// so nothing has to be on PATH.
        fn plugin(&self, id: &str) {
            let manifest = format!(
                r#"{{
                    "id": "{id}",
                    "name": "Tones",
                    "version": "0.1.0",
                    "api": 1,
                    "entry": {{ "native": {{ "{}": "bin/tones" }} }},
                    "programs": ["rox-no-such-program"]
                }}"#,
                manifest::platform()
            );
            self.write(&format!("{id}/plugin.json"), &manifest);
            self.write(&format!("{id}/bin/tones"), "binary");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            let _ = std::fs::remove_dir_all(self.cache());
        }
    }

    #[test]
    fn a_good_folder_loads_with_its_hash_and_programs() {
        let scratch = Scratch::new("good");
        scratch.plugin("tones");

        let found = scan(&scratch.0, &scratch.cache());
        assert_eq!(found.len(), 1);

        let tones = &found[0];
        assert!(tones.runs(), "{:?}", tones.error);
        assert_eq!(tones.id, "tones");
        assert_eq!(
            tones.hash,
            hash::folder_hash(&scratch.0.join("tones")).unwrap()
        );
        assert_eq!(tones.document["name"], "Tones");
        assert_eq!(tones.programs, vec![("rox-no-such-program".into(), false)]);
    }

    #[test]
    fn a_source_icon_loads_only_from_a_plain_svg_in_the_folder() {
        let scratch = Scratch::new("icon");
        let with_icon = |id: &str, icon: &str| {
            let manifest = format!(
                r#"{{
                    "id": "{id}",
                    "name": "Tones",
                    "version": "0.1.0",
                    "api": 1,
                    "entry": {{ "native": {{ "{}": "bin/tones" }} }},
                    "capabilities": {{ "source": {{ "label": "Tones", "icon": "{icon}" }} }}
                }}"#,
                manifest::platform()
            );
            scratch.write(&format!("{id}/plugin.json"), &manifest);
            scratch.write(&format!("{id}/bin/tones"), "binary");
        };

        with_icon("good", "icon.svg");
        scratch.write("good/icon.svg", "<svg xmlns='http://www.w3.org/2000/svg'/>");
        with_icon("missing", "icon.svg");
        with_icon("escapes", "../good/icon.svg");
        with_icon("png", "icon.png");
        scratch.write("png/icon.png", "png");
        with_icon("peeks", "icon.svg");
        scratch.write("peeks/icon.svg", "<svg><IMAGE href='/etc/passwd'/></svg>");

        let found = scan(&scratch.0, &scratch.cache());
        let by_id = |id: &str| found.iter().find(|p| p.id == id).unwrap();

        assert!(by_id("good").runs(), "{:?}", by_id("good").error);
        assert!(
            by_id("good")
                .icon
                .as_deref()
                .is_some_and(|b| b.starts_with(b"<svg"))
        );

        for (id, words) in [
            ("missing", "is missing"),
            ("escapes", "leaves the plugin folder"),
            ("png", "isn't an .svg"),
            ("peeks", "embeds an image"),
        ] {
            let plugin = by_id(id);
            let error = plugin.error.as_deref().unwrap_or_default();
            assert!(error.contains(words), "{id}: {error}");
            assert!(plugin.icon.is_none(), "{id}");
        }
    }

    #[test]
    fn an_action_icon_loads_like_the_source_icon() {
        let scratch = Scratch::new("action-icon");
        let with_icon = |id: &str, icon: &str| {
            let manifest = format!(
                r#"{{
                    "id": "{id}",
                    "name": "Tones",
                    "version": "0.1.0",
                    "api": 1,
                    "entry": {{ "native": {{ "{}": "bin/tones" }} }},
                    "capabilities": {{ "source": {{ "label": "Tones", "actions": [
                        {{ "id": "save", "label": "Save", "on": ["track"], "icon": "{icon}" }},
                        {{ "id": "plain", "label": "Plain", "on": ["track"] }}
                    ] }} }}
                }}"#,
                manifest::platform()
            );
            scratch.write(&format!("{id}/plugin.json"), &manifest);
            scratch.write(&format!("{id}/bin/tones"), "binary");
        };

        with_icon("good", "save.svg");
        scratch.write("good/save.svg", "<svg xmlns='http://www.w3.org/2000/svg'/>");
        with_icon("escapes", "../good/save.svg");
        with_icon("peeks", "save.svg");
        scratch.write("peeks/save.svg", "<svg><feImage href='/etc/passwd'/></svg>");

        let found = scan(&scratch.0, &scratch.cache());
        let by_id = |id: &str| found.iter().find(|p| p.id == id).unwrap();

        let good = by_id("good");
        assert!(good.runs(), "{:?}", good.error);
        assert_eq!(good.action_icons.len(), 1, "only the action that names one");
        assert_eq!(good.action_icons[0].0, "save");

        for (id, words) in [
            ("escapes", "leaves the plugin folder"),
            ("peeks", "embeds an image"),
        ] {
            let error = by_id(id).error.as_deref().unwrap_or_default();
            assert!(
                error.contains(words) && error.contains("save"),
                "{id}: {error}"
            );
        }
    }

    #[test]
    fn a_program_the_plugin_ships_counts_as_found() {
        let scratch = Scratch::new("shipped");
        scratch.plugin("tones");
        scratch.write("tones/bin/rox-no-such-program", "binary");

        let found = scan(&scratch.0, &scratch.cache());
        assert_eq!(
            found[0].programs,
            vec![("rox-no-such-program".into(), true)]
        );
    }

    #[test]
    fn a_bad_manifest_still_shows_with_its_reason() {
        let scratch = Scratch::new("bad");
        scratch.write("broken/plugin.json", r#"{ "id": "broken", "tools": [] }"#);
        scratch.write("empty/readme.txt", "no manifest here");

        let found = scan(&scratch.0, &scratch.cache());
        let ids: Vec<&str> = found.iter().map(|loaded| loaded.id.as_str()).collect();
        assert_eq!(ids, ["broken", "empty"]);

        for loaded in &found {
            assert!(!loaded.runs());
            assert!(loaded.manifest.is_none());
            assert!(loaded.error.is_some());
        }
    }

    #[test]
    fn a_folder_named_for_another_id_is_refused() {
        let scratch = Scratch::new("renamed");
        scratch.plugin("tones");
        std::fs::rename(scratch.0.join("tones"), scratch.0.join("other")).unwrap();

        let found = scan(&scratch.0, &scratch.cache());
        let error = found[0].error.as_deref().unwrap_or_default();
        assert!(error.contains("holds plugin tones"), "{error}");
    }

    #[test]
    fn a_folder_with_a_reserved_directory_fails_to_load() {
        let scratch = Scratch::new("reserved");
        scratch.plugin("tones");
        scratch.plugin("upper");
        scratch.write("tones/_rox/signature", "sig");
        scratch.write("upper/_ROX/signature", "sig");

        for plugin in scan(&scratch.0, &scratch.cache()) {
            let error = plugin.error.as_deref().unwrap_or_default();
            assert!(error.contains("_rox/"), "{}: {error}", plugin.id);
        }
    }

    #[test]
    fn stray_files_are_skipped() {
        let scratch = Scratch::new("stray");
        scratch.plugin("tones");
        scratch.write("notes.txt", "not a plugin");
        scratch.write(".DS_Store", "");

        let ids: Vec<String> = scan(&scratch.0, &scratch.cache())
            .into_iter()
            .map(|l| l.id)
            .collect();
        assert_eq!(ids, ["tones"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_refused_not_followed() {
        let scratch = Scratch::new("symlink");
        scratch.plugin("tones");
        std::os::unix::fs::symlink(scratch.0.join("tones"), scratch.0.join("linked")).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", scratch.0.join("tones/hosts")).unwrap();

        let found = scan(&scratch.0, &scratch.cache());
        let linked = found.iter().find(|l| l.id == "linked").unwrap();
        assert!(linked.manifest.is_none());
        assert_eq!(linked.error.as_deref(), Some("the folder is a symlink"));

        let tones = found.iter().find(|l| l.id == "tones").unwrap();
        assert!(!tones.runs(), "a link inside the folder refuses it too");
        assert!(tones.hash.is_empty());
    }

    #[test]
    fn a_changed_byte_changes_the_hash() {
        let scratch = Scratch::new("changed");
        scratch.plugin("tones");
        let before = scan(&scratch.0, &scratch.cache())[0].hash.clone();

        scratch.write("tones/bin/tones", "binarY");
        let after = scan(&scratch.0, &scratch.cache())[0].hash.clone();

        assert!(!before.is_empty());
        assert_ne!(before, after);
    }

    /// The data folder reached through a link, as a portable install or
    /// macOS's `/tmp` can be: the folder appearing later still wakes it.
    #[cfg(unix)]
    #[test]
    fn the_watch_sees_a_folder_made_under_a_linked_parent() {
        let scratch = Scratch::new("watch");
        std::fs::create_dir_all(scratch.0.join("real")).unwrap();
        std::os::unix::fs::symlink(scratch.0.join("real"), scratch.0.join("link")).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let dir = scratch.0.join("link/plugins");
        let watch = Watch::new(&dir, move || {
            let _ = tx.send(());
        })
        .unwrap();

        scratch.plugin("link/plugins/tones");
        let woke = rx.recv_timeout(Duration::from_secs(10));
        assert!(woke.is_ok(), "no event for a folder made under the link");

        drop(watch);
    }

    #[test]
    fn a_missing_folder_scans_to_nothing() {
        let scratch = Scratch::new("missing");
        assert!(scan(&scratch.0.join("plugins"), &scratch.cache()).is_empty());
    }
}
