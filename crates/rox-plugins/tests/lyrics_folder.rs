//! The lyrics-folder example (`examples/plugins/lyrics-folder`) against the
//! real host: a plugin that only provides, answering `provider.lyrics` from a
//! scratch folder of `.lrc` files. Skipped with a note when no interpreter
//! resolves, like the tones tests.

use std::path::{Path, PathBuf};

use rox_plugins::{Host, HostConfig, hash, manifest, wire};
use serde_json::{Value, json};

const SHEET: &str = "[ar:Some Artist]\n[ti:Some Song]\n\
[00:12.00]<00:12.00>Bring <00:12.40>me <00:12.80>home\n\
[00:15.00]<00:15.00>One <00:15.50>more\n";

/// No tags, so the plugin goes by the file's name.
const PLAIN: &str = "First line\nSecond line\n";

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins")
}

fn plugin_dir() -> PathBuf {
    examples().join("lyrics-folder")
}

/// The test's own folder under the temp dir: the sheets the plugin reads and
/// its data dir. Gone when the test ends, pass or fail.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn host(tag: &str) -> Option<(Host, Scratch)> {
    let dir = plugin_dir();
    let manifest = manifest::load(&dir).expect("the example's manifest loads");

    if let Err(e) = manifest::entry_for(&manifest, &dir) {
        eprintln!("skipping the lyrics-folder example tests: {e}");
        return None;
    }

    let scratch = Scratch(std::env::temp_dir().join(format!(
        "rox-plugins-lyrics-folder-{tag}-{}",
        std::process::id()
    )));
    let sheets = scratch.0.join("sheets");
    std::fs::create_dir_all(&sheets).unwrap();
    std::fs::write(sheets.join("Some Artist - Some Song.lrc"), SHEET).unwrap();
    std::fs::write(sheets.join("Other Artist - Other Song.lrc"), PLAIN).unwrap();

    let host = Host::new(HostConfig::new(
        dir,
        manifest,
        json!({ "folder": sheets }),
        scratch.0.join("data"),
    ));

    Some((host, scratch))
}

fn call(host: &Host, method: &'static str, params: Value) -> Value {
    host.call(method, params, host.timeouts().listing)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn ask(host: &Host, artist: &str, title: &str) -> wire::LyricsCandidates {
    let answer = call(
        host,
        "provider.lyrics",
        json!({"artist": artist, "title": title, "album": "", "duration_secs": null}),
    );

    wire::decode(answer).expect("a checked answer")
}

#[test]
fn the_folder_loads_as_a_provider_only_plugin() {
    let manifest = manifest::load(&plugin_dir()).unwrap();
    assert_eq!(manifest.capabilities.source, None);
    assert!(manifest.capabilities.providers.lyrics);
    assert!(!manifest.capabilities.providers.metadata);
    assert!(!manifest.capabilities.providers.artist);

    let Some((host, _scratch)) = host("load") else {
        return;
    };
    let before = hash::folder_hash(&plugin_dir()).unwrap();

    host.ensure().expect("hello answers");
    ask(&host, "Some Artist", "Some Song");
    host.stop("test over");

    let after = hash::folder_hash(&plugin_dir()).unwrap();
    assert_eq!(
        before, after,
        "running it wrote nothing into its own folder"
    );
}

#[test]
fn a_tagged_file_answers_with_a_hint() {
    let Some((host, _scratch)) = host("tagged") else {
        return;
    };

    // Case and punctuation fold away, the way rox compares names.
    let found = ask(&host, "some artist", "Some Song!");
    host.stop("test over");

    assert_eq!(found.candidates.len(), 1);
    let hit = &found.candidates[0];
    assert_eq!(
        (hit.artist.as_str(), hit.title.as_str()),
        ("Some Artist", "Some Song")
    );
    assert!(hit.synced);
    assert_eq!(hit.confidence, Some(1.0));
    assert!(hit.text.contains("<00:12.40>"), "word tags come through");
}

#[test]
fn a_named_file_answers_without_a_hint() {
    let Some((host, _scratch)) = host("named") else {
        return;
    };

    let found = ask(&host, "Other Artist", "Other Song");
    host.stop("test over");

    assert_eq!(found.candidates.len(), 1);
    let hit = &found.candidates[0];
    assert!(!hit.synced);
    assert_eq!(hit.confidence, None);
    assert_eq!(hit.text, PLAIN);
}

#[test]
fn no_match_answers_an_empty_list() {
    let Some((host, _scratch)) = host("miss") else {
        return;
    };

    let other = ask(&host, "Some Artist", "Another Song");
    let blank = ask(&host, "", "");
    host.stop("test over");

    assert!(other.candidates.is_empty());
    assert!(blank.candidates.is_empty());
}
