//! `.roxp` packages against the examples rox publishes: a package hashes like
//! the folder it came from, unpacks into the cache under that hash, and runs
//! from the copy. The tests that pack through `package::pack` or start a
//! plugin are skipped with a note when no interpreter resolves, like the
//! tones tests.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rox_plugins::loader::{self, Origin};
use rox_plugins::{Host, HostConfig, Options, Stream, hash, manifest, package};
use serde_json::{Value, json};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins")
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("rox-plugins-package-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("plugins")).unwrap();

        Scratch(dir)
    }

    fn plugins(&self) -> PathBuf {
        self.0.join("plugins")
    }

    fn cache(&self) -> PathBuf {
        self.0.join("plugin-cache")
    }

    /// The example's files under the names they'd have in its package, plus
    /// `extra` written beside it, packed straight through rox-package so no
    /// interpreter has to resolve.
    fn pack(&self, example: &str, archive: &str, extra: &[(&str, &str)]) -> PathBuf {
        let mut files = files_of(&examples().join(example), "");
        for (name, text) in extra {
            let source = self.0.join(format!("extra-{}", files.len()));
            std::fs::write(&source, text).unwrap();
            files.push((name.to_string(), source));
        }

        let archive = self.plugins().join(archive);
        rox_package::pack(&archive, &files).expect("the files pack");

        archive
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn files_of(dir: &Path, prefix: &str) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let rel = format!("{prefix}{}", entry.file_name().to_str().unwrap());
        match entry.file_type().unwrap().is_dir() {
            true => files.extend(files_of(&entry.path(), &format!("{rel}/"))),
            false => files.push((rel, entry.path())),
        }
    }

    files
}

fn runnable(dir: &Path) -> bool {
    let manifest = manifest::load(dir).expect("the example's manifest loads");
    match manifest::entry_for(&manifest, dir) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("skipping {}: {e}", dir.display());
            false
        }
    }
}

#[test]
fn a_packed_example_hashes_like_its_folder() {
    let scratch = Scratch::new("examples");

    for example in ["tones", "internet-archive", "lyrics-folder"] {
        let folder = examples().join(example);
        if !runnable(&folder) {
            continue;
        }

        let archive = scratch.plugins().join(format!("{example}.roxp"));
        package::pack(&folder, &archive).unwrap_or_else(|e| panic!("{example}: {e}"));

        let folder_hash = hash::folder_hash(&folder).unwrap();
        assert_eq!(
            hash::archive_hash(&archive).unwrap(),
            folder_hash,
            "{example}"
        );
        assert_eq!(
            package::peek(&archive).unwrap().hash,
            folder_hash,
            "{example}"
        );
    }
}

#[test]
fn peek_reads_the_manifest_without_unpacking() {
    let scratch = Scratch::new("peek");
    let archive = scratch.pack("tones", "tones.roxp", &[]);

    let peek = package::peek(&archive).unwrap();
    assert_eq!(peek.manifest.id, "tones");
    assert_eq!(peek.document["id"], "tones");
    assert_eq!(
        peek.hash,
        hash::folder_hash(&examples().join("tones")).unwrap()
    );

    assert!(!scratch.cache().exists());
    let beside: Vec<_> = std::fs::read_dir(scratch.plugins()).unwrap().collect();
    assert_eq!(beside.len(), 1, "only the archive itself");
}

#[test]
fn a_wrapped_folder_says_where_the_manifest_belongs() {
    let scratch = Scratch::new("wrapped");
    let files: Vec<(String, PathBuf)> = files_of(&examples().join("tones"), "tones/");
    let archive = scratch.plugins().join("tones.roxp");
    rox_package::pack(&archive, &files).unwrap();

    let err = package::peek(&archive).unwrap_err();
    assert!(
        err.contains("top of the package") && err.contains("tones/"),
        "{err}"
    );
}

#[test]
fn unpack_reuses_a_copy_that_still_matches() {
    let scratch = Scratch::new("reuse");
    let archive = scratch.pack("tones", "tones.roxp", &[]);

    let copy = package::unpack(&archive, &scratch.cache()).unwrap();
    let peek = package::peek(&archive).unwrap();
    assert_eq!(copy, scratch.cache().join("tones").join(&peek.hash[..16]));

    // A modified time isn't part of the hash, so it marks the files as the
    // ones the first unpack wrote.
    let marked = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let file = std::fs::File::options()
        .write(true)
        .open(copy.join("tones.py"))
        .unwrap();
    file.set_modified(marked).unwrap();
    drop(file);

    assert_eq!(package::unpack(&archive, &scratch.cache()).unwrap(), copy);
    let modified = std::fs::metadata(copy.join("tones.py"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(modified, marked);
}

#[test]
fn unpack_replaces_a_copy_someone_edited() {
    let scratch = Scratch::new("edited");
    let archive = scratch.pack("tones", "tones.roxp", &[]);
    let copy = package::unpack(&archive, &scratch.cache()).unwrap();

    std::fs::write(copy.join("tones.py"), "print('planted')").unwrap();
    std::fs::write(copy.join("extra.py"), "print('planted')").unwrap();

    assert_eq!(package::unpack(&archive, &scratch.cache()).unwrap(), copy);
    assert_eq!(
        std::fs::read(copy.join("tones.py")).unwrap(),
        std::fs::read(examples().join("tones/tones.py")).unwrap()
    );
    assert!(!copy.join("extra.py").exists());
    assert_eq!(
        hash::folder_hash(&copy).unwrap(),
        package::peek(&archive).unwrap().hash
    );
}

#[test]
fn reserved_entries_are_never_unpacked() {
    let scratch = Scratch::new("reserved");
    let archive = scratch.pack(
        "tones",
        "tones.roxp",
        &[("_rox/signature", "sig"), ("_ROX/other", "sig")],
    );

    let copy = package::unpack(&archive, &scratch.cache()).unwrap();
    let names: Vec<String> = std::fs::read_dir(&copy)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|name| name.eq_ignore_ascii_case("_rox")),
        "{names:?}"
    );

    let folder_hash = hash::folder_hash(&examples().join("tones")).unwrap();
    assert_eq!(hash::folder_hash(&copy).unwrap(), folder_hash);
    assert_eq!(package::peek(&archive).unwrap().hash, folder_hash);
}

#[test]
fn a_packed_tones_runs_from_its_unpacked_copy() {
    let folder = examples().join("tones");
    if !runnable(&folder) {
        return;
    }

    let scratch = Scratch::new("runs");
    package::pack(&folder, &scratch.plugins().join("tones.roxp")).unwrap();

    let found = loader::scan(&scratch.plugins(), &scratch.cache());
    assert_eq!(found.len(), 1);
    let tones = &found[0];
    assert!(tones.runs(), "{:?}", tones.error);
    assert_eq!(
        tones.origin,
        Origin::Package(scratch.plugins().join("tones.roxp"))
    );
    assert!(tones.dir.starts_with(scratch.cache().join("tones")));
    assert_eq!(tones.hash, hash::folder_hash(&folder).unwrap());

    let host = Host::new(HostConfig::new(
        tones.dir.clone(),
        tones.manifest.clone().unwrap(),
        json!({}),
        scratch.0.join("plugin-data"),
    ));
    host.ensure().expect("hello answers");

    let listed: Value = host
        .call(
            "source.browse",
            json!({"node": "chords", "cursor": null}),
            host.timeouts().sync_first,
        )
        .unwrap();
    let key = listed["entries"][0]["track"]["key"].as_str().unwrap();
    let stream = Stream::open(&host, key, Options::default()).unwrap();
    assert!(!stream.read_at(0, 4096).unwrap().is_empty());

    drop(stream);
    host.stop("test over");

    assert_eq!(
        hash::folder_hash(&tones.dir).unwrap(),
        tones.hash,
        "running it wrote nothing into its copy"
    );
}

#[test]
fn the_scan_lists_a_package_and_a_folder_shadows_it() {
    let scratch = Scratch::new("shadow");
    scratch.pack("tones", "tones.roxp", &[]);

    let found = loader::scan(&scratch.plugins(), &scratch.cache());
    assert_eq!(found.len(), 1);
    assert!(matches!(found[0].origin, Origin::Package(_)));
    assert!(found[0].manifest.is_some(), "{:?}", found[0].error);
    assert!(scratch.cache().join("tones").is_dir());

    // An author's working copy beside the release they installed.
    for (rel, source) in files_of(&examples().join("tones"), "") {
        let target = scratch.plugins().join("tones").join(rel);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(source, target).unwrap();
    }

    let found = loader::scan(&scratch.plugins(), &scratch.cache());
    let origins: Vec<(&str, bool)> = found
        .iter()
        .map(|l| (l.id.as_str(), l.origin == Origin::Folder))
        .collect();
    assert_eq!(origins, [("tones", true), ("tones", false)]);

    let shadowed = found[1].error.as_deref().unwrap_or_default();
    assert!(shadowed.contains("the folder tones"), "{shadowed}");
    assert!(!scratch.cache().join("tones").exists(), "its copy is swept");
}

#[test]
fn two_packages_with_one_id_fail_the_second() {
    let scratch = Scratch::new("twice");
    scratch.pack("tones", "a.roxp", &[]);
    scratch.pack("tones", "b.roxp", &[("notes.txt", "a second release")]);

    let found = loader::scan(&scratch.plugins(), &scratch.cache());
    assert_eq!(found.len(), 2);

    assert_eq!(
        found[0].origin,
        Origin::Package(scratch.plugins().join("a.roxp"))
    );
    assert!(found[0].manifest.is_some(), "{:?}", found[0].error);

    assert_eq!(
        found[1].origin,
        Origin::Package(scratch.plugins().join("b.roxp"))
    );
    let error = found[1].error.as_deref().unwrap_or_default();
    assert!(
        error.contains("a.roxp already holds plugin tones"),
        "{error}"
    );

    let copies: Vec<_> = std::fs::read_dir(scratch.cache().join("tones"))
        .unwrap()
        .collect();
    assert_eq!(copies.len(), 1, "only the first one unpacks");
}
