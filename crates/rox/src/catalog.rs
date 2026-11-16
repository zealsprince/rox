//! The catalog client (ADR 36): the last good index on disk, the record of
//! what was installed from it, and an install that downloads a package,
//! verifies it and hands it to the import or install a file from disk takes.
//! The wire is [`rox_net::sources::catalog`] and [`rox_net::download`]. This
//! draws nothing.
//!
//! An install applies nothing, places nothing and switches nothing on. It
//! ends in the confirm or the Plugins page the same file opened from disk
//! ends in, and nothing the catalog serves can skip one (ADR 35). The
//! index's sha256 proves a file is the one the index meant, fetched from the
//! same origin as the file, so it proves nothing about the origin itself.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use gpui::{App, Global, Task};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use rox_core::settings::{self, WorkspaceBundle, roxo, roxw};
use rox_net::download::{self, Progress};
use rox_net::sources::catalog::{self as wire, Index, Kind, Origin, Record, Summary};
use rox_panel_api::panel;
use rox_panel_api::toast::Toast;
use rox_plugins::package;
use rox_services::plugins as host;

use crate::startup::updates::CURRENT;

/// Not chosen yet. `.invalid` never resolves, so a build carrying this
/// reaches no one.
pub(crate) const ORIGIN: &str = "https://catalog.invalid";

const INDEX_FILE: &str = "index.json";
const INSTALLED_FILE: &str = "installed.json";
const INCOMING: &str = "incoming";
const PREVIEWS: &str = "previews";

/// The compiled-in origin. Only a debug build reads an override, for a
/// local test server; a release build has no other origin to reach.
fn origin() -> Result<Origin, String> {
    #[cfg(debug_assertions)]
    if let Ok(text) = std::env::var("ROX_CATALOG_ORIGIN") {
        return Origin::new(&text);
    }

    Origin::new(ORIGIN)
}

fn index_path() -> PathBuf {
    settings::catalog_dir().join(INDEX_FILE)
}

fn installed_path() -> PathBuf {
    settings::catalog_dir().join(INSTALLED_FILE)
}

// ── The index ──

/// The last index a refresh fetched, read under the same cap as the wire.
pub(crate) fn load() -> Option<Index> {
    let text = read_capped(&index_path(), wire::INDEX_MAX)?;
    wire::parse_index(&text).ok()
}

fn read_capped(path: &Path, cap: u64) -> Option<String> {
    String::from_utf8(read_bytes_capped(path, cap)?).ok()
}

fn read_bytes_capped(path: &Path, cap: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .ok()?;

    (bytes.len() as u64 <= cap).then_some(bytes)
}

/// Fetch, parse, then replace the cached copy. A failed fetch or an index
/// that doesn't read leaves the cache as it was.
pub(crate) fn refresh(cx: &mut App) -> Task<Result<Index, String>> {
    cx.background_executor().spawn(async move { refresh_now() })
}

fn refresh_now() -> Result<Index, String> {
    let text = wire::fetch_index_text(&origin()?)?;
    let index = wire::parse_index(&text)?;

    for reason in &index.dropped {
        log::warn!("catalog: index {reason}");
    }

    // The exact bytes, so a later signature check reads what was served.
    write_atomic(&index_path(), text.as_bytes())?;
    edit_installs(|installs| installs.checked_at = crate::startup::updates::now());

    log::info!("catalog: {} packages", index.packages.len());
    Ok(index)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }

    let tmp = path.with_extension("json.tmp");
    let written = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }

    written.map_err(|e| format!("{}: {e}", path.display()))
}

// ── Records and previews ──

/// A package's record, fetched on the background executor.
pub(crate) fn fetch_record(id: &str, cx: &App) -> Task<Result<Record, String>> {
    let id = id.to_string();

    cx.background_executor().spawn(async move {
        let record = wire::fetch_record(&origin()?, &id)?;

        // Only reached once the wire has checked the id is a slug.
        for reason in &record.dropped {
            log::warn!("catalog: record {id}: {reason}");
        }

        Ok(record)
    })
}

/// A preview's bytes, from `catalog/previews/` or else fetched and kept
/// there. Blocking. The bytes are only ever what the server called an image
/// under the cap, still undecoded: decoding is the caller's, under its own
/// pixel cap.
pub(crate) fn preview(path: &str) -> Result<Vec<u8>, String> {
    let url = origin()?.resolve(path)?;
    let file = settings::catalog_dir()
        .join(PREVIEWS)
        .join(preview_name(&url));

    if let Some(bytes) = read_bytes_capped(&file, wire::PREVIEW_MAX) {
        return Ok(bytes);
    }

    let bytes = wire::fetch_preview(&origin()?, path)?;
    if let Err(e) = write_atomic(&file, &bytes) {
        log::warn!("catalog: keeping a preview: {e}");
    }

    Ok(bytes)
}

/// A hash of the URL, so nothing the index says ever names a file on disk.
fn preview_name(url: &str) -> String {
    Sha256::digest(url.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ── The record of installs ──

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Installs {
    #[serde(default)]
    checked_at: u64,
    #[serde(default)]
    installed: Vec<Installed>,
}

/// One file installed from the catalog, by the sha256 it was verified
/// against. `file` is where the import stored it, under the data dir.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Installed {
    pub id: String,
    pub kind: Kind,
    pub sha256: String,
    pub file: String,
}

/// Serializes every read-modify-write of `installed.json`: a refresh and an
/// install both write it from the background.
static INSTALLS: Mutex<()> = Mutex::new(());

fn read_installs() -> Installs {
    read_capped(&installed_path(), wire::INDEX_MAX)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Records whose file is gone are dropped on every write.
fn edit_installs(edit: impl FnOnce(&mut Installs)) {
    let _held = INSTALLS.lock().unwrap_or_else(PoisonError::into_inner);

    let mut installs = read_installs();
    edit(&mut installs);
    installs
        .installed
        .retain(|record| still_there(&record.file));

    settings::write_json(&installed_path(), &installs, "catalog installs");
}

fn still_there(file: &str) -> bool {
    settings::data_dir().join(file).is_file()
}

fn record(installed: Installed) {
    edit_installs(move |installs| {
        installs
            .installed
            .retain(|record| record.id != installed.id);
        installs.installed.push(installed);
    });
}

/// Installed packages whose index entry's `latest.sha256` has moved, among
/// those this build can run. No version is compared. It reads two files, so
/// it's for the background executor.
pub(crate) fn updates() -> Vec<Summary> {
    let Some(index) = load() else {
        return Vec::new();
    };

    updates_in(&index, &read_installs().installed, still_there, CURRENT)
}

/// What came from the catalog and is still on disk. Reads a file, so it's
/// for the background executor.
pub(crate) fn installed() -> Vec<Installed> {
    let mut installed = read_installs().installed;
    installed.retain(|record| still_there(&record.file));

    installed
}

fn updates_in(
    index: &Index,
    installed: &[Installed],
    exists: impl Fn(&str) -> bool,
    current: &str,
) -> Vec<Summary> {
    installed
        .iter()
        .filter(|record| exists(&record.file))
        .filter_map(|record| {
            let summary = index.get(&record.id)?;
            let moved = summary.latest.sha256 != record.sha256;

            (moved && runs_here(&summary.latest.min_rox, current).is_ok()).then(|| summary.clone())
        })
        .collect()
}

/// `min_rox` against the running build. One that doesn't parse is refused,
/// and its text isn't echoed.
pub(crate) fn runs_here(min_rox: &str, current: &str) -> Result<(), String> {
    let need = Version::parse(min_rox)
        .map_err(|_| "this package's minimum rox version doesn't read".to_string())?;
    let have = Version::parse(current)
        .map_err(|_| format!("this build's version {current} doesn't read"))?;

    if need > have {
        return Err(format!(
            "this package needs rox {need} or newer, and this is {have}"
        ));
    }

    Ok(())
}

// ── Install ──

/// The install in flight, app-global so it outlives whatever started it.
#[derive(Default)]
struct Running(Option<(String, Arc<Progress>)>);

impl Global for Running {}

/// The id and the progress of the install in flight.
pub(crate) fn running(cx: &App) -> Option<(String, Arc<Progress>)> {
    cx.try_global::<Running>()
        .and_then(|running| running.0.clone())
}

/// The part file goes with it. An install already past its download runs
/// on to its confirm.
pub(crate) fn cancel(cx: &mut App) {
    if let Some((_, progress)) = running(cx) {
        progress.cancel();
    }
}

/// What the store step kept, for the offer that follows on the main thread.
enum Stored {
    Workspace(Box<WorkspaceBundle>),
    Plugin(String),
    Object(String),
}

/// Download, verify, then the import or install a file from disk takes,
/// ending in its confirm. One runs at a time. It runs detached, so dropping
/// the answer doesn't stop it; [`cancel`] does, and quitting does.
pub(crate) fn install(id: &str, cx: &mut App) -> Task<Result<Installed, String>> {
    match start(id, cx) {
        Ok(answer) => cx.background_executor().spawn(async move {
            answer
                .recv()
                .await
                .unwrap_or_else(|_| Err("the install was dropped".into()))
        }),
        Err(e) => Task::ready(Err(e)),
    }
}

fn start(
    id: &str,
    cx: &mut App,
) -> Result<async_channel::Receiver<Result<Installed, String>>, String> {
    // The id may come from a link or a script: checked before it's looked
    // up, logged or put in a path.
    if !wire::is_slug(id) {
        return Err("that isn't a catalog id".into());
    }

    if let Some((other, _)) = running(cx) {
        return Err(format!("{other} is still installing"));
    }

    let progress = Arc::new(Progress::new());
    cx.set_global(Running(Some((id.to_string(), progress.clone()))));

    // Cancelled on quit so the download drops its part file. A quit that
    // lands mid-chunk leaves it to the next install's sweep.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel();
            async {}
        }
    })
    .detach();

    let (tx, rx) = async_channel::bounded(1);

    let id = id.to_string();
    cx.spawn(async move |cx| {
        let fetched = cx
            .background_executor()
            .spawn({
                let id = id.clone();
                async move { fetch_and_store(&id, &progress) }
            })
            .await;

        let outcome = cx
            .update(|cx| {
                cx.set_global(Running(None));

                match fetched {
                    Ok((installed, stored)) => {
                        log::info!("catalog: installed {}", installed.id);
                        offer(stored, cx);
                        Ok(installed)
                    }
                    Err(e) => {
                        log::warn!("catalog: installing {id}: {e}");
                        Err(e)
                    }
                }
            })
            .unwrap_or_else(|_| Err("rox closed during the install".into()));

        let _ = tx.send(outcome).await;
    })
    .detach();

    Ok(rx)
}

/// Each download gets a fresh name: a plugin peek is remembered by path,
/// length and time, and two packages could share all three.
static NEXT: AtomicU64 = AtomicU64::new(0);

fn incoming_dir() -> PathBuf {
    settings::catalog_dir().join(INCOMING)
}

/// Off the main thread: the lookup, the download, then the same store step
/// the socket verbs run for a file from disk, then the record. The
/// downloaded file is deleted whatever the store step answers.
fn fetch_and_store(id: &str, progress: &Progress) -> Result<(Installed, Stored), String> {
    let index = load().ok_or("there's no catalog index yet")?;
    let summary = index
        .get(id)
        .ok_or_else(|| format!("the catalog has no package {id}"))?;
    runs_here(&summary.latest.min_rox, CURRENT)?;

    let url = origin()?.resolve(&summary.latest.file)?;

    let ext = match summary.kind {
        Kind::Workspace => roxw::EXT,
        Kind::Plugin => package::EXT,
        Kind::Object => roxo::EXT,
        Kind::Unknown => return Err("that kind of package doesn't install".into()),
    };

    // Only one install runs, so anything here is a leftover.
    let dir = incoming_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    // The name is rox's own. Nothing from the index names a file on disk.
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let file = dir.join(format!("download-{n}.{ext}"));

    download::to_file(
        &url,
        &file,
        summary.latest.bytes,
        &summary.latest.sha256,
        progress,
    )?;

    let stored = store(summary, &file);
    let _ = std::fs::remove_file(&file);
    let (stored, path) = stored?;

    let installed = Installed {
        id: summary.id.clone(),
        kind: summary.kind,
        sha256: summary.latest.sha256.clone(),
        file: relative(&path),
    };
    record(installed.clone());

    Ok((installed, stored))
}

/// The disk path's store step for each kind, and where it put the file.
fn store(summary: &Summary, file: &Path) -> Result<(Stored, PathBuf), String> {
    match summary.kind {
        Kind::Workspace => {
            let bundle = crate::workspaces::import_file(file)?;
            let path = crate::workspaces::path_for(&bundle.name);

            Ok((Stored::Workspace(Box::new(bundle)), path))
        }

        Kind::Plugin => {
            // The catalog's id for a plugin is its manifest's. One that
            // differs would record an install under the wrong name.
            let peek = package::peek(file)?;
            if peek.manifest.id != summary.id {
                return Err(format!(
                    "the package holds another plugin than {}",
                    summary.id
                ));
            }

            let id = host::install_file(file)?;
            let path = settings::plugins_dir().join(format!("{id}.{}", package::EXT));

            Ok((Stored::Plugin(peek.manifest.name), path))
        }

        Kind::Object => {
            let name = crate::objects::install(file)?;
            let path = roxo::objects_dir().join(format!("{name}.{}", roxo::EXT));

            Ok((Stored::Object(name), path))
        }

        Kind::Unknown => Err("that kind of package doesn't install".into()),
    }
}

/// Under the data dir, with forward slashes, so a portable data dir that
/// moves keeps its records.
fn relative(path: &Path) -> String {
    relative_in(&settings::data_dir(), path)
}

fn relative_in(data: &Path, path: &Path) -> String {
    let inside = path.strip_prefix(data).unwrap_or(path);

    inside
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// What a file opened from disk shows next, in the front window: the apply
/// confirm, the Plugins page with the new row off, or the place confirm.
fn offer(stored: Stored, cx: &mut App) {
    match stored {
        Stored::Workspace(bundle) => {
            crate::workspace::with_front_workspace(cx, move |ws, _, cx| {
                ws.offer_imported(&bundle, cx);
            });
        }

        Stored::Plugin(name) => {
            host::rescan(cx);
            crate::workspace::with_front_workspace(cx, move |_, window, cx| {
                rox_panel_api::panel_settings::open_app_page("settings-page-plugins", window, cx);
                Toast::new(
                    panel::Tone::Good,
                    rox_i18n::t!("settings-plugins-installed", name = name),
                )
                .show(window, cx);
            });
        }

        Stored::Object(name) => {
            crate::workspace::with_front_workspace(cx, move |ws, window, cx| {
                match crate::objects::canvas_for(ws, cx) {
                    Some(canvas) => canvas.update(cx, |canvas, cx| {
                        crate::objects::offer(canvas, &name, window, cx)
                    }),
                    None => Toast::new(
                        panel::Tone::Info,
                        rox_i18n::t!("canvas-object-needs-canvas", name = name.as_str()),
                    )
                    .show(window, cx),
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const NEW: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    fn index(entries: &[(&str, &str, &str)]) -> Index {
        let packages: Vec<serde_json::Value> = entries
            .iter()
            .map(|(id, sha256, min_rox)| {
                serde_json::json!({
                    "id": id,
                    "kind": "workspace",
                    "name": id,
                    "author": "someone",
                    "summary": "One line.",
                    "latest": {
                        "version": "1",
                        "file": format!("files/{id}.roxw"),
                        "bytes": 10,
                        "sha256": sha256,
                        "min_rox": min_rox,
                    },
                })
            })
            .collect();

        wire::parse_index(&serde_json::json!({ "version": 1, "packages": packages }).to_string())
            .unwrap()
    }

    fn installed(id: &str, sha256: &str) -> Installed {
        Installed {
            id: id.to_string(),
            kind: Kind::Workspace,
            sha256: sha256.to_string(),
            file: format!("workspaces/{id}.roxw"),
        }
    }

    fn ids(summaries: &[Summary]) -> Vec<&str> {
        summaries
            .iter()
            .map(|summary| summary.id.as_str())
            .collect()
    }

    /// One major past this build, so the test never pins a version.
    fn newer_than_this_build() -> String {
        let have = Version::parse(CURRENT).unwrap();
        format!("{}.0.0", have.major + 1)
    }

    #[test]
    fn an_update_is_a_changed_hash() {
        let index = index(&[
            ("same", OLD, "1.0.0"),
            ("moved", NEW, "1.0.0"),
            ("unlisted-here", NEW, "1.0.0"),
        ]);
        let records = [
            installed("same", OLD),
            installed("moved", OLD),
            installed("gone-from-index", OLD),
        ];

        assert_eq!(
            ids(&updates_in(&index, &records, |_| true, CURRENT)),
            vec!["moved"]
        );
    }

    #[test]
    fn a_missing_file_drops_its_record() {
        let index = index(&[("kept", NEW, "1.0.0"), ("deleted", NEW, "1.0.0")]);
        let records = [installed("kept", OLD), installed("deleted", OLD)];

        let exists = |file: &str| file != "workspaces/deleted.roxw";
        assert_eq!(
            ids(&updates_in(&index, &records, exists, CURRENT)),
            vec!["kept"]
        );
    }

    #[test]
    fn a_newer_min_rox_is_refused_with_a_reason() {
        let newer = newer_than_this_build();

        let e = runs_here(&newer, CURRENT).unwrap_err();
        assert!(e.contains(&newer) && e.contains(CURRENT), "{e}");

        assert!(runs_here(CURRENT, CURRENT).is_ok());
        assert!(runs_here("1.0.0", CURRENT).is_ok());

        // A minimum that doesn't read is refused, and not echoed.
        let e = runs_here("2.0\nforged log line", CURRENT).unwrap_err();
        assert!(!e.contains("forged"), "{e}");

        // An update this build can't install isn't offered.
        let index = index(&[("ahead", NEW, &newer)]);
        assert!(updates_in(&index, &[installed("ahead", OLD)], |_| true, CURRENT).is_empty());
    }

    #[test]
    fn a_preview_is_kept_under_a_name_rox_makes() {
        let name = preview_name("https://catalog.example/v1/previews/../../a b.png");

        assert_eq!(name.len(), 32);
        assert!(name.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(
            name,
            preview_name("https://catalog.example/v1/previews/b.png")
        );
    }

    #[test]
    fn a_record_lives_under_the_data_dir() {
        let data = Path::new("/data/rox");
        let path = data.join("workspaces").join("Aurora.roxw");
        assert_eq!(relative_in(data, &path), "workspaces/Aurora.roxw");
    }
}
