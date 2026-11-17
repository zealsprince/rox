//! The catalog client (ADR 36): the last good index on disk, the record of
//! what was installed from it, and an install that downloads a package,
//! verifies it and hands it to the import or install a file from disk takes.
//! The wire is [`rox_net::sources::catalog`] and [`rox_net::download`]. This
//! draws nothing.
//!
//! An install applies nothing and switches nothing on. It ends where the
//! same file opened from disk ends: the apply confirm, the Plugins page, or
//! the place an object with nothing to review gets without asking, and
//! nothing the catalog serves can skip a confirm (ADR 35). The
//! index's sha256 proves a file is the one the index meant, fetched from the
//! same origin as the file, so it proves nothing about the origin itself.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, SystemTime};

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

pub(crate) const ORIGIN: &str = "https://catalog.rox.music";

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
    let fetching = cx.background_executor().spawn(async move { refresh_now() });

    cx.spawn(async move |cx| {
        let fetched = fetching.await;
        if fetched.is_ok() {
            cx.update(landed).ok();
        }

        fetched
    })
}

/// [`refresh`]'s work, blocking, for a caller already on the background
/// executor. The chip's count follows the new index.
pub(crate) fn refresh_now() -> Result<Index, String> {
    let text = wire::fetch_index_text(&origin()?)?;
    let index = wire::parse_index(&text)?;

    for reason in &index.dropped {
        log::warn!("catalog: index {reason}");
    }

    // The exact bytes, so a later signature check reads what was served.
    write_atomic(&index_path(), text.as_bytes())?;
    edit_installs(|installs| installs.checked_at = crate::startup::updates::now());
    note_updates();
    prune_previews(&index);

    log::info!("catalog: {} packages", index.packages.len());
    Ok(index)
}

/// After a refresh or an install lands: an open catalog window reloads,
/// and the menubar repaints, since the chip's static is outside gpui's
/// reactivity.
pub(crate) fn landed(cx: &mut App) {
    crate::catalog_window::refreshed(cx);
    cx.refresh_windows();
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }

    let tmp = path.with_extension("json.tmp");
    let written = rox_package::fresh::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
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
        // A read counts as use, so the prune keeps what the window shows.
        let _ = std::fs::File::options()
            .write(true)
            .open(&file)
            .and_then(|kept| kept.set_modified(SystemTime::now()));

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

/// A record's extra previews aren't in the index, so recent use stands in.
const PREVIEW_KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Blocking, for the background executor after a refresh. Only plain files
/// directly in `catalog/previews/` with the cache's own name shape are ever
/// removed: no symlink is followed and nothing is recursed into.
fn prune_previews(index: &Index) {
    let Ok(origin) = origin() else {
        return;
    };
    let named: HashSet<String> = index
        .packages
        .iter()
        .filter_map(|summary| summary.preview.as_deref())
        .filter_map(|path| origin.resolve(path).ok())
        .map(|url| preview_name(&url))
        .collect();

    let dir = settings::catalog_dir().join(PREVIEWS);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };

    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();

        // DirEntry's type and metadata describe a symlink itself, not its target.
        let plain = entry.file_type().is_ok_and(|kind| kind.is_file());
        let modified = entry.metadata().and_then(|meta| meta.modified()).ok();

        if plain && stale_preview(&name, modified, &named, now) {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(e) => log::warn!("catalog: pruning a preview: {e}"),
            }
        }
    }

    if removed > 0 {
        log::info!("catalog: pruned {removed} cached previews");
    }
}

/// Kept: what the index names, and anything else read or fetched within
/// [`PREVIEW_KEEP`]. A name the cache wouldn't make is never ours to remove.
fn stale_preview(
    name: &str,
    modified: Option<SystemTime>,
    named: &HashSet<String>,
    now: SystemTime,
) -> bool {
    let ours = name.len() == 32
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !ours || named.contains(name) {
        return false;
    }

    // A time that won't read or sits in the future counts as fresh.
    modified
        .and_then(|at| now.duration_since(at).ok())
        .is_some_and(|age| age > PREVIEW_KEEP)
}

// ── The record of installs ──

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Installs {
    #[serde(default)]
    checked_at: u64,
    #[serde(default)]
    installed: Vec<Installed>,
    /// Updates whose chip was closed, each by the hash it offered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dismissed: Vec<Offered>,
}

/// One update as the chip offers it. The hash is part of it, so a second
/// update to a package whose first one was dismissed shows again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Offered {
    pub id: String,
    pub sha256: String,
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

/// Nothing is written when nothing changed. A record whose file is gone
/// stays: only rox's own remove forgets one ([`forget`]).
fn edit_installs(edit: impl FnOnce(&mut Installs)) {
    let _held = INSTALLS.lock().unwrap_or_else(PoisonError::into_inner);

    let read = read_installs();
    let mut installs = read.clone();
    edit(&mut installs);

    if installs != read {
        settings::write_json(&installed_path(), &installs, "catalog installs");
    }
}

/// Whether the file a record names is still there. Reads the disk.
pub(crate) fn on_disk(record: &Installed) -> bool {
    settings::data_dir().join(&record.file).is_file()
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

    updates_in(&index, &read_installs().installed, CURRENT)
}

/// Everything on record as installed from the catalog, with its file still
/// there or not. Reads a file, so it's for the background executor.
pub(crate) fn installed() -> Vec<Installed> {
    read_installs().installed
}

/// How many installs are on record, and when the catalog was last fetched,
/// for the launch check to weigh. A record whose file was deleted counts,
/// since its card still offers updates. Reads a file, so it's for the
/// background executor.
pub(crate) fn on_record() -> (usize, u64) {
    let installs = read_installs();

    (installs.installed.len(), installs.checked_at)
}

/// rox's own remove of a stored file, which is the user telling rox, so the
/// record naming it goes. The chip and an open catalog window follow.
pub(crate) fn forget(path: &Path, cx: &mut App) {
    let file = relative(path);

    let forgetting = cx.background_executor().spawn(async move {
        edit_installs(|installs| forget_in(installs, &file));
        note_updates();
    });

    cx.spawn(async move |cx| {
        forgetting.await;
        cx.update(landed).ok();
    })
    .detach();
}

fn forget_in(installs: &mut Installs, file: &str) {
    installs.installed.retain(|record| record.file != file);
}

// ── The updates chip ──

/// The updates the menubar chip announces: pending and not dismissed. A
/// live static because the menubar reads it per frame. The Updates tab
/// asks [`updates`] instead, so a dismissed chip never hides the tab.
static CHIP: RwLock<Vec<Offered>> = RwLock::new(Vec::new());

pub(crate) fn chip_count() -> usize {
    CHIP.read().unwrap_or_else(PoisonError::into_inner).len()
}

/// Works the chip out again from the saved index and the record, and
/// forgets dismissals that no longer match a pending update. Reads two
/// files, so it's for the background executor; the caller repaints.
pub(crate) fn note_updates() {
    let index = load();

    edit_installs(|installs| {
        *CHIP.write().unwrap_or_else(PoisonError::into_inner) =
            chip_in(installs, index.as_ref(), CURRENT);
    });
}

fn chip_in(installs: &mut Installs, index: Option<&Index>, current: &str) -> Vec<Offered> {
    let pending: Vec<Offered> = index
        .map(|index| updates_in(index, &installs.installed, current))
        .unwrap_or_default()
        .into_iter()
        .map(|summary| Offered {
            id: summary.id,
            sha256: summary.latest.sha256,
        })
        .collect();

    installs
        .dismissed
        .retain(|offered| pending.contains(offered));

    pending
        .into_iter()
        .filter(|offered| !installs.dismissed.contains(offered))
        .collect()
}

/// Remembered in `installed.json`. The chip hides at once; the write runs
/// on the background executor.
pub(crate) fn dismiss_chip(cx: &mut App) {
    let closed = std::mem::take(&mut *CHIP.write().unwrap_or_else(PoisonError::into_inner));

    cx.background_executor()
        .spawn(async move {
            edit_installs(|installs| {
                for offered in closed {
                    if !installs.dismissed.contains(&offered) {
                        installs.dismissed.push(offered);
                    }
                }
            });
        })
        .detach();
}

/// A record whose file was deleted is still one: its update stores a fresh
/// copy.
fn updates_in(index: &Index, installed: &[Installed], current: &str) -> Vec<Summary> {
    installed
        .iter()
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
    /// Offered to a canvas whether it's a first install or an update.
    Object(String),
}

/// What an Update does with the file an earlier install of a workspace or
/// an object stored. A first install, a plugin and an update whose recorded
/// file doesn't pass [`replaceable`] all store as Keep Both does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// A copy beside the old file under a deduped name, and the record moves
    /// to it. The old file stays as an ordinary saved one.
    KeepBoth,
    /// The new file takes the recorded file's place under its name.
    Replace,
}

/// Download, verify, then the import or install a file from disk takes,
/// ending in its confirm. One runs at a time. It runs detached, so dropping
/// the answer doesn't stop it; [`cancel`] does, and quitting does.
pub(crate) fn install(id: &str, choice: Choice, cx: &mut App) -> Task<Result<Installed, String>> {
    match start(id, choice, cx) {
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
    choice: Choice,
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
                async move { fetch_and_store(&id, choice, &progress) }
            })
            .await;

        let outcome = cx
            .update(|cx| {
                cx.set_global(Running(None));

                match fetched {
                    Ok((installed, stored)) => {
                        log::info!("catalog: installed {}", installed.id);
                        offer(stored, cx);
                        // An update installed is one less for the chip and
                        // the tab, wherever the install started.
                        landed(cx);
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
fn fetch_and_store(
    id: &str,
    choice: Choice,
    progress: &Progress,
) -> Result<(Installed, Stored), String> {
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
    rox_package::fresh::dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

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

    let earlier = installed()
        .into_iter()
        .find(|record| record.id == summary.id);
    let into = match (choice, &earlier) {
        (Choice::Replace, Some(record)) => match replaceable(record, summary.kind) {
            Ok(path) => Some(path),
            Err(why) => {
                log::warn!(
                    "catalog: {id}: not replacing {:?}, keeping both: {why}",
                    record.file
                );
                None
            }
        },
        _ => None,
    };

    let stored = store(summary, &file, into.as_deref());
    let _ = std::fs::remove_dir_all(&dir);
    let (stored, path) = stored?;

    let installed = Installed {
        id: summary.id.clone(),
        kind: summary.kind,
        sha256: summary.latest.sha256.clone(),
        file: relative(&path),
    };
    record(installed.clone());
    note_updates();

    Ok((installed, stored))
}

/// The disk path's store step for each kind, and where it put the file.
/// `into` is a recorded file that passed [`replaceable`], for a Replace.
fn store(summary: &Summary, file: &Path, into: Option<&Path>) -> Result<(Stored, PathBuf), String> {
    match summary.kind {
        Kind::Workspace => {
            if let Some(into) = into {
                let bundle = crate::workspaces::replace_file(file, into)?;
                log::info!("catalog: replaced {}", into.display());

                return Ok((Stored::Workspace(Box::new(bundle)), into.to_path_buf()));
            }

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
            if let Some(into) = into {
                let name = crate::objects::replace(file, into)?;
                log::info!("catalog: replaced {}", into.display());

                return Ok((Stored::Object(name), into.to_path_buf()));
            }

            let name = crate::objects::install(file)?;
            let path = roxo::objects_dir().join(format!("{name}.{}", roxo::EXT));

            Ok((Stored::Object(name), path))
        }

        Kind::Unknown => Err("that kind of package doesn't install".into()),
    }
}

/// Where the record's file sits as a place a Replace may write, or why it
/// can't be one. `installed.json` is a file on disk anyone can edit, so the
/// path it names is checked like any outside path before rox writes there:
/// one plain file name with the kind's extension, directly in the folder
/// that kind stores in, an existing regular file and not a symlink.
fn replaceable(record: &Installed, kind: Kind) -> Result<PathBuf, String> {
    replaceable_in(&settings::data_dir(), record, kind)
}

fn replaceable_in(data: &Path, record: &Installed, kind: Kind) -> Result<PathBuf, String> {
    let (folder, ext) = match kind {
        Kind::Workspace => ("workspaces", roxw::EXT),
        Kind::Object => ("objects", roxo::EXT),
        Kind::Plugin | Kind::Unknown => return Err("only a workspace or an object asks".into()),
    };
    if record.kind != kind {
        return Err("the record is for another kind of package".into());
    }

    let Some((dir, name)) = record.file.split_once('/') else {
        return Err(format!("the record's file isn't in {folder}/"));
    };
    if dir != folder {
        return Err(format!("the record's file isn't in {folder}/"));
    }

    let ours = settings::assets::flat_name(name)
        && !name.starts_with('.')
        && Path::new(name)
            .extension()
            .is_some_and(|found| found.eq_ignore_ascii_case(ext));
    if !ours {
        return Err("the record's file isn't a plain file name".into());
    }

    let path = data.join(folder).join(name);

    // symlink_metadata describes a link itself, so one is never followed.
    let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("the record's file: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err("the record's file is a symlink".into());
    }
    if !meta.is_file() {
        return Err("the record's file isn't a regular file".into());
    }

    Ok(path)
}

/// Whether an Update of `summary` asks Replace or Keep Both: a workspace or
/// an object with an update pending whose recorded file a Replace could
/// write. Anything else gets the one Update button.
fn asks_in(data: &Path, summary: &Summary, installed: &[Installed]) -> bool {
    installed
        .iter()
        .find(|record| record.id == summary.id)
        .is_some_and(|record| {
            record.sha256 != summary.latest.sha256
                && replaceable_in(data, record, summary.kind).is_ok()
        })
}

/// The ids among `updates` that ask. Reads the record and the disk, so it's
/// for the background executor.
pub(crate) fn asking(updates: &[Summary]) -> HashSet<String> {
    let installed = installed();
    let data = settings::data_dir();

    updates
        .iter()
        .filter(|summary| asks_in(&data, summary, &installed))
        .map(|summary| summary.id.clone())
        .collect()
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
/// confirm, the Plugins page with the new row off, or an object's place. An
/// object's update is offered like its first install, and the copies
/// already placed are the canvas's own items, which stay as they are.
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
                    // Outside this workspace update: a place with nothing to
                    // review edits the canvas at once, which reaches back into
                    // the workspace, and gpui panics on that mid-update.
                    Some(canvas) => window.defer(cx, move |window, cx| {
                        canvas.update(cx, |canvas, cx| {
                            crate::objects::offer(canvas, &name, window, cx)
                        })
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

        assert_eq!(ids(&updates_in(&index, &records, CURRENT)), vec!["moved"]);
    }

    #[test]
    fn a_deleted_file_with_a_newer_hash_is_still_an_update() {
        // The records name files no test creates: nothing here reads the disk.
        let index = index(&[("kept", NEW, "1.0.0"), ("deleted", NEW, "1.0.0")]);
        let records = [installed("kept", OLD), installed("deleted", OLD)];

        assert_eq!(
            ids(&updates_in(&index, &records, CURRENT)),
            vec!["kept", "deleted"]
        );
    }

    #[test]
    fn a_deleted_file_counts_on_the_chip_like_the_updates_tab() {
        let index = index(&[("deleted", NEW, "1.0.0"), ("same", OLD, "1.0.0")]);
        let mut installs = Installs {
            installed: vec![installed("deleted", OLD), installed("same", OLD)],
            ..Installs::default()
        };

        assert_eq!(
            chip_in(&mut installs, Some(&index), CURRENT),
            vec![offered("deleted", NEW)]
        );
        assert_eq!(installs.installed.len(), 2);
    }

    #[test]
    fn rox_removing_a_file_forgets_only_its_record() {
        let mut installs = Installs {
            installed: vec![installed("look", OLD), installed("other", OLD)],
            ..Installs::default()
        };

        forget_in(&mut installs, "workspaces/look.roxw");
        assert_eq!(installs.installed, vec![installed("other", OLD)]);

        // A path no record names forgets nothing.
        forget_in(&mut installs, "workspaces/look.roxw");
        assert_eq!(installs.installed.len(), 1);
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
        assert!(updates_in(&index, &[installed("ahead", OLD)], CURRENT).is_empty());
    }

    fn offered(id: &str, sha256: &str) -> Offered {
        Offered {
            id: id.to_string(),
            sha256: sha256.to_string(),
        }
    }

    const NEWER: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    #[test]
    fn a_dismissed_update_stays_dismissed() {
        let index = index(&[("aurora", NEW, "1.0.0")]);
        let mut installs = Installs {
            installed: vec![installed("aurora", OLD)],
            ..Installs::default()
        };

        assert_eq!(
            chip_in(&mut installs, Some(&index), CURRENT),
            vec![offered("aurora", NEW)]
        );

        // The same update, dismissed, stays gone however often it's asked.
        installs.dismissed.push(offered("aurora", NEW));
        for _ in 0..2 {
            assert!(chip_in(&mut installs, Some(&index), CURRENT).is_empty());
            assert_eq!(installs.dismissed, vec![offered("aurora", NEW)]);
        }

        // The tab's list doesn't read the dismissals.
        assert_eq!(
            ids(&updates_in(&index, &installs.installed, CURRENT)),
            vec!["aurora"]
        );
    }

    #[test]
    fn a_new_update_brings_the_chip_back() {
        let mut installs = Installs {
            installed: vec![installed("aurora", OLD), installed("lamp", OLD)],
            dismissed: vec![offered("aurora", NEW)],
            ..Installs::default()
        };

        // Aurora moves again and lamp moves for the first time: both show,
        // and the stale dismissal is forgotten.
        let moved = index(&[("aurora", NEWER, "1.0.0"), ("lamp", NEW, "1.0.0")]);
        assert_eq!(
            chip_in(&mut installs, Some(&moved), CURRENT),
            vec![offered("aurora", NEWER), offered("lamp", NEW)]
        );
        assert!(installs.dismissed.is_empty());

        // A dismissal for an update that was installed goes too.
        let mut installs = Installs {
            installed: vec![installed("aurora", NEW)],
            dismissed: vec![offered("aurora", NEW)],
            ..Installs::default()
        };
        let current = index(&[("aurora", NEW, "1.0.0")]);
        assert!(chip_in(&mut installs, Some(&current), CURRENT).is_empty());
        assert!(installs.dismissed.is_empty());
    }

    #[test]
    fn a_dismissal_is_kept_in_the_record() {
        let installs = Installs {
            checked_at: 7,
            installed: vec![installed("aurora", OLD)],
            dismissed: vec![offered("aurora", NEW)],
        };

        let text = serde_json::to_string(&installs).unwrap();
        assert_eq!(serde_json::from_str::<Installs>(&text).unwrap(), installs);

        // A record written before dismissals existed still reads.
        let old: Installs = serde_json::from_str(r#"{"checked_at": 7, "installed": []}"#).unwrap();
        assert!(old.dismissed.is_empty());
        assert!(!serde_json::to_string(&old).unwrap().contains("dismissed"));
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
    fn a_preview_is_pruned_only_when_unnamed_and_unused() {
        let now = SystemTime::now();
        let old = Some(now - PREVIEW_KEEP - Duration::from_secs(60));
        let recent = Some(now - Duration::from_secs(60));

        let named_one = preview_name("https://catalog.example/v1/previews/a.webp");
        let unnamed = preview_name("https://catalog.example/v1/previews/gone.webp");
        let named: HashSet<String> = [named_one.clone()].into();

        // The index names it: kept however old.
        assert!(!stale_preview(&named_one, old, &named, now));

        // Unnamed: kept while recently used, removed once it isn't.
        assert!(!stale_preview(&unnamed, recent, &named, now));
        assert!(stale_preview(&unnamed, old, &named, now));

        // A time that doesn't read, or one ahead of the clock, keeps it.
        assert!(!stale_preview(&unnamed, None, &named, now));
        let ahead = Some(now + Duration::from_secs(3600));
        assert!(!stale_preview(&unnamed, ahead, &named, now));

        // A name the cache wouldn't make is never removed.
        for foreign in [
            "notes.txt",
            "index.json",
            &format!("{unnamed}.json.tmp"),
            &unnamed.to_uppercase(),
            &unnamed[..31],
            &format!("{unnamed}0"),
        ] {
            assert!(!stale_preview(foreign, old, &named, now), "{foreign}");
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-catalog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("workspaces")).unwrap();
        std::fs::create_dir_all(dir.join("objects")).unwrap();
        dir
    }

    fn package(id: &str, kind: &str, sha256: &str) -> Summary {
        let packages = serde_json::json!([{
            "id": id,
            "kind": kind,
            "name": id,
            "author": "someone",
            "summary": "One line.",
            "latest": {
                "version": "1",
                "file": format!("files/{id}"),
                "bytes": 10,
                "sha256": sha256,
                "min_rox": "1.0.0",
            },
        }]);
        let text = serde_json::json!({ "version": 1, "packages": packages }).to_string();

        wire::parse_index(&text).unwrap().packages.remove(0)
    }

    fn record(id: &str, kind: Kind, file: &str) -> Installed {
        Installed {
            id: id.to_string(),
            kind,
            sha256: OLD.to_string(),
            file: file.to_string(),
        }
    }

    #[test]
    fn an_update_of_a_workspace_or_an_object_asks_and_a_plugin_doesnt() {
        let data = scratch("asks");
        std::fs::write(data.join("workspaces/Look.roxw"), b"saved").unwrap();
        std::fs::write(data.join("objects/Lamp.roxo"), b"saved").unwrap();

        let installed = [
            record("look", Kind::Workspace, "workspaces/Look.roxw"),
            record("lamp", Kind::Object, "objects/Lamp.roxo"),
            record("hello", Kind::Plugin, "plugins/hello.roxp"),
            record("gone", Kind::Workspace, "workspaces/Gone.roxw"),
        ];

        assert!(asks_in(
            &data,
            &package("look", "workspace", NEW),
            &installed
        ));
        assert!(asks_in(&data, &package("lamp", "object", NEW), &installed));

        // A plugin is always replaced in place, so it never asks.
        assert!(!asks_in(
            &data,
            &package("hello", "plugin", NEW),
            &installed
        ));

        // No update pending, no record, or the file deleted: one button.
        assert!(!asks_in(
            &data,
            &package("look", "workspace", OLD),
            &installed
        ));
        assert!(!asks_in(
            &data,
            &package("new", "workspace", NEW),
            &installed
        ));
        assert!(!asks_in(
            &data,
            &package("gone", "workspace", NEW),
            &installed
        ));

        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_replace_writes_only_a_plain_file_in_its_kinds_folder() {
        let data = scratch("replaceable");
        std::fs::write(data.join("workspaces/Look.roxw"), b"saved").unwrap();
        std::fs::write(data.join("objects/Lamp.roxo"), b"saved").unwrap();
        std::fs::write(data.join("settings.json"), b"{}").unwrap();
        std::fs::create_dir_all(data.join("workspaces/Folder.roxw")).unwrap();

        assert_eq!(
            replaceable_in(
                &data,
                &record("look", Kind::Workspace, "workspaces/Look.roxw"),
                Kind::Workspace
            ),
            Ok(data.join("workspaces/Look.roxw"))
        );
        assert_eq!(
            replaceable_in(
                &data,
                &record("lamp", Kind::Object, "objects/Lamp.roxo"),
                Kind::Object
            ),
            Ok(data.join("objects/Lamp.roxo"))
        );

        let outside = data.join("settings.json").display().to_string();
        let refused = [
            "../settings.json",
            "settings.json",
            outside.as_str(),
            "workspaces/../settings.json",
            "workspaces/sub/Look.roxw",
            "workspaces\\Look.roxw",
            "workspaces/",
            "workspaces/.Look.roxw",
            "workspaces/Look.json",
            "objects/Lamp.roxo",
            "workspaces/Missing.roxw",
            "workspaces/Folder.roxw",
        ];
        for file in refused {
            let record = record("look", Kind::Workspace, file);
            assert!(
                replaceable_in(&data, &record, Kind::Workspace).is_err(),
                "{file}"
            );
        }

        // A record that disagrees with the index on the kind, and a plugin.
        let lamp = record("lamp", Kind::Workspace, "objects/Lamp.roxo");
        assert!(replaceable_in(&data, &lamp, Kind::Object).is_err());
        let hello = record("hello", Kind::Plugin, "plugins/hello.roxp");
        assert!(replaceable_in(&data, &hello, Kind::Plugin).is_err());

        let _ = std::fs::remove_dir_all(&data);
    }

    #[cfg(unix)]
    #[test]
    fn a_replace_never_follows_a_symlink() {
        let data = scratch("symlink");
        std::fs::write(data.join("settings.json"), b"{}").unwrap();
        std::os::unix::fs::symlink(
            data.join("settings.json"),
            data.join("workspaces/Look.roxw"),
        )
        .unwrap();

        let record = record("look", Kind::Workspace, "workspaces/Look.roxw");
        let refused = replaceable_in(&data, &record, Kind::Workspace).unwrap_err();
        assert!(refused.contains("symlink"), "{refused}");

        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_record_lives_under_the_data_dir() {
        let data = Path::new("/data/rox");
        let path = data.join("workspaces").join("Aurora.roxw");
        assert_eq!(relative_in(data, &path), "workspaces/Aurora.roxw");
    }
}
