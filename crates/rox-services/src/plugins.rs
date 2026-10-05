//! The plugin host's services face: browsing, searching, syncing and picking
//! through a `plugin:<id>` source, the covers its rows show, and the stream
//! opener the engine plays them through (ADR 30). Plugins run as subprocesses
//! behind `rox-plugins`; every call into one runs on the background executor,
//! never the UI thread.
//!
//! The plugins folder is scanned and watched, and [`apply`] keeps one host
//! per plugin that's switched on, loaded, and approved for the exact folder
//! hash it has now. A folder that changed since its approval is switched off
//! until the user switches it on again, which is the approving act. Developer
//! mode, which the user turns on per plugin for one session, approves such a
//! change instead when the manifest declares nothing new. A record whose
//! folder is gone is Missing: nothing starts and nothing is swept.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use gpui::{App, Entity, Global, Task};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use rox_core::settings::{self, PluginRecord, Settings, ShuffleMode, SyncedCollection};
use rox_library::cue::{PLUGIN_PREFIX, TrackKey};
use rox_library::locator::{Locator, PluginStream};
use rox_library::members::{self, PluginTrack};
use rox_library::rusqlite::Connection;
use rox_library::store;
use rox_playback::continuation::{self, Pick};
use rox_playback::plugin::{Opened, ReadAt};
use rox_plugins::manifest::SourceCap;
use rox_plugins::{Host, HostConfig, Loaded, Options, Status, Stream, loader, wire};

use crate::catalog::Library;
use crate::player::Player;

const NO_HOST: &str = "no plugin host";

/// A pre-opened stream nobody takes in this long is closed.
const PREOPEN_KEEP: Duration = Duration::from_secs(60);

/// How far ahead of the audible track streams are opened.
const PREOPEN_AHEAD: usize = 2;

/// How long a track has to hold before the ones after it are opened.
const PREOPEN_SETTLE: Duration = Duration::from_secs(2);

/// How far ahead of the crossfade the next track opens. Cold opens measured
/// up to 3.8 s, and with the longest fade it still lands well inside
/// [`PREOPEN_KEEP`].
const PREOPEN_LEAD: Duration = Duration::from_secs(20);

/// A sync past this many pages is a plugin looping on its own cursor.
const MAX_SYNC_PAGES: usize = 2000;

/// How long an open or a cover waits for the first [`apply`] before refusing.
const FIRST_APPLY_WAIT: Duration = Duration::from_secs(10);

/// One page of a browse or search. A `None` cursor is the last page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub entries: Vec<Entry>,
    pub cursor: Option<String>,
    pub notice: Option<Notice>,
    /// Other ways the plugin can list this place, and which one this is.
    pub views: Vec<View>,
    pub view: Option<String>,
    /// Columns the plugin fills beside the tags, and each entry's values,
    /// lined up with `entries`. Shorter than `entries` when rows have none.
    pub fields: Vec<Field>,
    pub values: Vec<Values>,
    /// Where each track's album and artists open, by track key. Keyed rather
    /// than lined up, so it needs no care when rows sort or drop out.
    pub go_to: HashMap<String, GoTo>,
    /// Each row's flags, by track key or node id, for the rows whose plugin
    /// knows them. A row missing here can take every action.
    pub flags: HashMap<String, Vec<String>>,
}

/// The nodes Go to opens for one track. Serialized, it's what the library
/// keeps beside a plugin row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoTo {
    pub album: Option<Target>,
    pub artists: Vec<Target>,
}

/// A node Go to opens, with what a place needs to show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub title: String,
    pub collection: bool,
    pub kind: Option<NodeKind>,
}

impl From<wire::Node> for Target {
    fn from(node: wire::Node) -> Self {
        Target {
            id: node.id,
            title: node.title,
            collection: node.collection,
            kind: node.kind,
        }
    }
}

/// A column the plugin's service knows, like a popularity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
}

pub use rox_plugins::wire::{FieldKind, MAX_FIELDS};

#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Number(f64),
    Text(String),
}

/// One entry's values by field id.
pub type Values = Vec<(String, FieldValue)>;

fn values_of(wire: wire::Values) -> Values {
    wire.into_iter()
        .filter_map(|(id, value)| match value {
            Value::Number(n) => n.as_f64().map(|n| (id, FieldValue::Number(n))),
            Value::String(text) => Some((id, FieldValue::Text(text))),
            _ => None,
        })
        .collect()
}

/// A filter or an order the plugin's service applies to a place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub id: String,
    pub label: String,
}

/// What the plugin wants said over a page. `setup` means it needs a setting
/// from the user first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub setup: bool,
    pub link: Option<NoticeLink>,
}

/// A web page a notice offers to open. The host has checked it's http or https.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeLink {
    pub url: String,
    pub label: String,
}

/// Why a plugin source has nothing to answer with, in terms the user can act
/// on from the Plugins page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The Plugins switch is off.
    PluginsOff,
    /// Its own switch is off.
    SwitchedOff,
    /// Its folder changed since the last approval.
    Changed,
    /// Its folder is gone from the plugins folder.
    Missing,
    /// Its folder is there but doesn't load.
    Failed,
    /// It crashed too often and was stopped.
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A folder-like node; `collection` marks one that can be synced.
    Node {
        id: String,
        title: String,
        subtitle: String,
        collection: bool,
        kind: Option<NodeKind>,
        /// A key the plugin answers a cover for. Empty for none.
        art: String,
        /// The service's home, whose page can follow the roots.
        home: bool,
    },
    Track(PluginTrack),
    /// A heading over the entries after it. `tiles` shows them as a shelf of
    /// covers rather than rows.
    Section {
        title: String,
        tiles: bool,
    },
}

pub use rox_plugins::wire::NodeKind;

/// A plugin with a host, and what the host was built from: a new hash or new
/// config means a new host.
pub(crate) struct Running {
    pub(crate) host: Host,
    pub(crate) label: String,
    hash: String,
    config: Value,
}

/// Source id to its host. [`apply`] is the only writer.
static HOSTS: LazyLock<RwLock<HashMap<String, Arc<Running>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The plugins folder as the last scan found it. The first read scans, which
/// is the projection's first load at startup, so its rows hide right away.
static FOLDERS: LazyLock<RwLock<Vec<Loaded>>> =
    LazyLock::new(|| RwLock::new(loader::scan(&settings::plugins_dir())));

/// Run at the end of every [`apply`]. See [`after_apply`].
static AFTER_APPLY: OnceLock<fn(&mut App)> = OnceLock::new();

/// The live ids [`apply`] last reloaded the projection for.
static LIVE: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Moves whenever the folders or the records might have, so the Plugins
/// page re-reads on a change rather than every frame.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Opens once the first [`apply`] has built the hosts, plugins on or off. A
/// launch restore opens its start entry before that.
static FIRST_APPLY: Gate = Gate::new();

static STARTED: AtomicBool = AtomicBool::new(false);

/// A latch that stays open once opened.
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    const fn new() -> Self {
        Gate {
            open: Mutex::new(false),
            opened: Condvar::new(),
        }
    }

    fn open(&self) {
        if let Ok(mut open) = self.open.lock() {
            *open = true;
        }
        self.opened.notify_all();
    }

    fn is_open(&self) -> bool {
        self.open.lock().is_ok_and(|open| *open)
    }

    /// Blocks for at most `limit`. Never on the UI thread: that's where the
    /// first apply runs, so it would wait out the whole limit.
    fn wait(&self, limit: Duration) -> bool {
        let Ok(open) = self.open.lock() else {
            return false;
        };

        self.opened
            .wait_timeout_while(open, limit, |open| !*open)
            .is_ok_and(|(open, _)| *open)
    }
}

/// What `start` hands the rest of the module: the library to reload, and the
/// watch, which stops when this drops.
struct Wiring {
    library: Entity<Library>,
    watch: Option<Arc<loader::Watch>>,
}

impl Global for Wiring {}

pub(crate) fn running(source: &str) -> Option<Arc<Running>> {
    HOSTS.read().ok()?.get(source).cloned()
}

pub(crate) fn host_for(source: &str) -> Result<Host, String> {
    running(source)
        .map(|running| running.host.clone())
        .ok_or_else(|| NO_HOST.to_string())
}

fn record_id(source: &str) -> Option<&str> {
    source.strip_prefix(PLUGIN_PREFIX)
}

fn source_of(id: &str) -> String {
    format!("{PLUGIN_PREFIX}{id}")
}

/// Every folder the last scan found, loadable or not, sorted by id.
pub fn loaded() -> Vec<Loaded> {
    FOLDERS
        .read()
        .map(|folders| folders.clone())
        .unwrap_or_default()
}

/// Whether the plugin's folder is there with a manifest that parses. A
/// record without one is Missing, and its rows hide.
pub fn present(id: &str) -> bool {
    FOLDERS.read().is_ok_and(|folders| {
        folders
            .iter()
            .any(|folder| folder.id == id && folder.manifest.is_some())
    })
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

fn bump() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// The running host's status, None while the plugin has no host.
pub fn status(id: &str) -> Option<Status> {
    running(&source_of(id)).map(|running| running.host.status())
}

/// Installs the stream opener, follows `player` to open upcoming plugin
/// tracks early, expires old picks, and starts every plugin that's switched
/// on and approved.
/// Once per app; a later window's call is a no-op.
pub fn start(library: Entity<Library>, player: Entity<Player>, cx: &mut App) {
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }

    expire_picks(&library, cx);
    cx.set_global(Wiring {
        library,
        watch: None,
    });
    crate::openers::install(Arc::new(open));
    follow(player, cx);

    // The first scan hashes every plugin folder, which stays off the UI
    // thread. Not on the pool either: cover fetches there wait for the apply
    // after it, and on a fixed-size pool they could hold every thread.
    let (scanned, scan) = async_channel::bounded::<()>(1);
    let spawned = std::thread::Builder::new()
        .name("plugin-scan".into())
        .spawn(move || {
            drop(loaded());
            let _ = scanned.try_send(());
        });
    if let Err(e) = spawned {
        log::warn!("plugins: scanning on the UI thread, no scan thread: {e}");
    }

    cx.spawn(async move |cx| {
        let _ = scan.recv().await;
        cx.update(apply).ok();
    })
    .detach();

    cx.on_app_quit(|cx| {
        let hosts: Vec<Host> = HOSTS
            .read()
            .map(|table| table.values().map(|running| running.host.clone()).collect())
            .unwrap_or_default();

        cx.background_executor().spawn(async move {
            for host in hosts {
                host.hang_up_now("rox quit");
            }
        })
    })
    .detach();
}

/// Plugins run only with the Plugins switch on.
pub fn allowed() -> bool {
    settings::plugins_enabled()
}

/// Played-but-never-added tracks nobody came back to go (ADR 29). At launch
/// only, so a queue in use is never touched; the saved queue and the last
/// track are kept, since they restore by row id.
fn expire_picks(library: &Entity<Library>, cx: &mut App) {
    let db_path = library.read(cx).db_path();
    let expired = cx.background_executor().spawn(async move {
        let session = Settings::load().session;
        let keep: HashSet<i64> = session
            .last_queue
            .iter()
            .flat_map(|queue| queue.entries.iter().map(|entry| entry.id))
            .chain(session.last_track.map(|track| track.id))
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let expired = store::open(&db_path).and_then(|mut conn| {
            members::expire_picks(&mut conn, now - members::PICK_KEEP_SECS, &keep)
        });
        match expired {
            Ok(count) => count,
            Err(e) => {
                log::warn!("plugins: expiring old picks failed: {e}");
                0
            }
        }
    });

    let library = library.clone();
    cx.spawn(async move |cx| {
        let count = expired.await;
        if count > 0 {
            log::info!("plugins: {count} played but never added tracks expired");
            library
                .update(cx, |library, cx| library.reload_projection(cx))
                .ok();
        }
    })
    .detach();
}

/// Brings the hosts in line with the records, the folders and the approvals,
/// then reloads the projection if that changed which rows browse. Run after
/// anything that moves one of them.
///
/// Plugins run only with the Plugins switch on. A switched-on record whose
/// folder isn't the approved one is switched off here, whatever the gate,
/// unless Developer mode approves it first.
pub fn apply(cx: &mut App) {
    let Some(library) = cx.try_global::<Wiring>().map(|w| w.library.clone()) else {
        return;
    };
    let on = allowed();
    if on {
        watch(cx);
    }

    let folders = loaded();
    let folder = |id: &str| folders.iter().find(|folder| folder.id == id);

    let mut records = Settings::load().accounts.plugins;
    if redevelop(&records, &folders) {
        records = Settings::load().accounts.plugins;
    }

    // Changed on disk. A Missing record keeps its switch: deleting the old
    // folder is how many people update a plugin. So does a Developer mode
    // folder that can't load, since it can't run and a save mid-edit is how
    // that happens.
    let changed: Vec<String> = records
        .iter()
        .filter(|record| record.enabled)
        .filter(|record| {
            folder(&record.id).is_some_and(|folder| {
                !settings::plugin_approved(&record.id, &folder.hash)
                    && !(developing(&record.id) && !folder.runs())
            })
        })
        .map(|record| record.id.clone())
        .collect();

    if !changed.is_empty() {
        log::info!("plugins: switched off, changed since approval: {changed:?}");
        let off = changed.clone();
        Settings::update(move |s| {
            for record in &mut s.accounts.plugins {
                if off.contains(&record.id) {
                    record.enabled = false;
                }
            }
        });
    }

    let wanted: HashMap<String, (Loaded, PluginRecord)> = records
        .into_iter()
        .filter(|_| on)
        .filter(|record| record.enabled && !changed.contains(&record.id))
        .filter_map(|record| {
            let folder = folder(&record.id)?;
            let declares = folder
                .manifest
                .as_ref()
                .is_some_and(|m| m.capabilities.source.is_some());

            (folder.runs() && declares).then(|| (source_of(&record.id), (folder.clone(), record)))
        })
        .collect();

    let (stopping, fresh) = reconcile(wanted);

    if !stopping.is_empty() {
        cx.background_executor()
            .spawn(async move {
                for host in stopping {
                    host.stop("switched off");
                }
            })
            .detach();
    }

    // A fresh host syncs what it has switched on, which also starts it.
    for source in fresh {
        let sync = sync_now(library.clone(), &source, cx);
        cx.spawn(async move |_| {
            if let Err(e) = sync.await {
                log::warn!("{source}: the first sync failed: {e}");
            }
        })
        .detach();
    }

    if live_moved() || !changed.is_empty() {
        library.update(cx, |library, cx| library.reload_projection(cx));
    }

    FIRST_APPLY.open();
    bump();

    if let Some(hook) = AFTER_APPLY.get() {
        hook(cx);
    }
}

/// The app's hook for state that follows which plugins run, since apply
/// also runs off the folder watch where no caller is around to follow up.
/// Set once; a second call is ignored.
pub fn after_apply(hook: fn(&mut App)) {
    let _ = AFTER_APPLY.set(hook);
}

/// Swaps the host table to `wanted`, keeping a host whose folder and config
/// haven't moved. Answers the hosts to stop and the sources that got one.
fn reconcile(wanted: HashMap<String, (Loaded, PluginRecord)>) -> (Vec<Host>, Vec<String>) {
    let Ok(mut table) = HOSTS.write() else {
        return (Vec::new(), Vec::new());
    };

    let mut stopping = Vec::new();
    table.retain(|source, running| {
        let keep = wanted.get(source).is_some_and(|(folder, record)| {
            running.hash == folder.hash && running.config == record.config
        });
        if !keep {
            stopping.push(running.host.clone());
        }

        keep
    });

    let mut fresh = Vec::new();
    for (source, (folder, record)) in wanted {
        if table.contains_key(&source) {
            continue;
        }

        let Some(manifest) = folder.manifest.clone() else {
            continue;
        };
        let label = match record.label.is_empty() {
            true => manifest
                .capabilities
                .source
                .as_ref()
                .map(|cap| cap.label.clone())
                .unwrap_or_default(),
            false => record.label.clone(),
        };

        log::info!("{source}: loaded, folder hash {}", folder.hash);
        let mut config = HostConfig::new(
            folder.dir.clone(),
            manifest,
            record.config.clone(),
            settings::plugin_data_dir(&record.id),
        );
        // Taken at start: a language switch reaches a running plugin the next
        // time it starts, rather than cutting off what it's playing.
        config.locale = rox_i18n::locale().to_string();
        table.insert(
            source.clone(),
            Arc::new(Running {
                host: Host::new(config),
                label,
                hash: folder.hash.clone(),
                config: record.config.clone(),
            }),
        );
        fresh.push(source);
    }

    (stopping, fresh)
}

/// Whether the ids whose rows browse moved since the last call. The first
/// call only records them: the projection's first load read the same.
fn live_moved() -> bool {
    let live = crate::sources::live_ids(&Settings::load().accounts);
    let Ok(mut last) = LIVE.lock() else {
        return false;
    };

    let moved = last.as_ref().is_some_and(|last| *last != live);
    *last = Some(live);

    moved
}

/// Scans the plugins folder off the UI thread, then applies whatever moved.
/// It probes interpreters afresh, so only the page's Rescan and a Program
/// Folders edit call it, never the folder watch.
pub fn rescan(cx: &mut App) {
    let dir = settings::plugins_dir();

    cx.spawn(async move |cx| {
        let found = cx
            .background_executor()
            .spawn(async move {
                rox_plugins::manifest::forget_probes();
                loader::scan(&dir)
            })
            .await;

        let moved = match FOLDERS.write() {
            Ok(mut folders) if *folders != found => {
                *folders = found;
                true
            }
            _ => false,
        };

        if moved {
            cx.update(|cx| {
                apply(cx);
                cx.refresh_windows();
            })
            .ok();
        }
    })
    .detach();
}

/// Starts following the plugins folder, once. A folder that appears later is
/// picked up through its parent.
fn watch(cx: &mut App) {
    let Some(wiring) = cx.try_global::<Wiring>() else {
        return;
    };
    if wiring.watch.is_some() {
        return;
    }

    let (tx, events) = async_channel::unbounded::<()>();
    let watch = match loader::Watch::new(&settings::plugins_dir(), move || {
        let _ = tx.try_send(());
    }) {
        Ok(watch) => Arc::new(watch),
        Err(e) => {
            log::warn!("plugins: not watching the plugins folder: {e}");
            return;
        }
    };

    cx.global_mut::<Wiring>().watch = Some(Arc::clone(&watch));

    cx.spawn(async move |cx| {
        while events.recv().await.is_ok() {
            watch.arm();
            if cx.update(rescan).is_err() {
                break;
            }
        }
    })
    .detach();
}

/// Switching on is the approving act: the folder's hash goes in the
/// machine's approvals, and the record keeps the manifest for the next
/// diff. Creates the record the first time.
pub fn approve(folder: &Loaded, cx: &mut App) {
    record_approval(folder);
    apply(cx);
}

/// The approval's writes, without the [`apply`] that follows them.
fn record_approval(folder: &Loaded) {
    let Some(manifest) = folder.manifest.as_ref() else {
        return;
    };

    settings::approve_plugin(&folder.id, &folder.hash);

    let id = folder.id.clone();
    let hash = folder.hash.clone();
    let document = folder.document.clone();
    let label = manifest
        .capabilities
        .source
        .as_ref()
        .map(|cap| cap.label.clone())
        .unwrap_or_default();
    Settings::update(move |s| {
        let records = &mut s.accounts.plugins;
        let at = match records.iter().position(|record| record.id == id) {
            Some(at) => at,
            None => {
                records.push(PluginRecord {
                    id: id.clone(),
                    ..Default::default()
                });
                records.len() - 1
            }
        };

        let record = &mut records[at];
        // Scrobbling newly declared starts on: the card just showed it. A
        // user who turned it off under the same declaration keeps it off.
        if declares_scrobble_in(&document) && !declares_scrobble_in(&record.approved_manifest) {
            record.scrobble = true;
        }
        record.enabled = true;
        record.label = label;
        record.hash = hash;
        record.approved_manifest = document;
    });
}

/// Developer mode's half of [`apply`]: approves every switched-on folder in
/// it that changed, loads, and declares nothing its last approval didn't.
/// Answers whether it approved any.
fn redevelop(records: &[PluginRecord], folders: &[Loaded]) -> bool {
    let mut approved = false;
    for record in records.iter().filter(|r| r.enabled && developing(&r.id)) {
        let Some(folder) = folders.iter().find(|f| f.id == record.id) else {
            continue;
        };
        if !folder.runs() || settings::plugin_approved(&record.id, &folder.hash) {
            continue;
        }

        // A new capability, program, entry or scrobble ask goes to the card.
        if !changes(&record.approved_manifest, &folder.document).is_empty() {
            continue;
        }

        log::info!(
            "plugin:{}: developer mode approved {}",
            record.id,
            folder.hash
        );
        record_approval(folder);
        approved = true;
    }

    approved
}

/// Plugins in Developer mode. Never saved, so it ends with the session.
static DEVELOPING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn developing(id: &str) -> bool {
    DEVELOPING
        .lock()
        .is_ok_and(|developing| developing.contains(id))
}

/// Developer mode approves the plugin's folder on its own whenever it
/// changes, for the rest of the session, so a save restarts the plugin
/// instead of switching it off. Switching the plugin off ends it.
pub fn set_developing(id: &str, on: bool) {
    if let Ok(mut developing) = DEVELOPING.lock() {
        match on {
            true => developing.insert(id.to_string()),
            false => developing.remove(id),
        };
    }

    bump();
}

fn declares_scrobble_in(document: &Value) -> bool {
    document["capabilities"]["source"]["scrobble"]
        .as_bool()
        .unwrap_or(false)
}

/// Switches a plugin that has a record. Switching one on here doesn't
/// approve anything: a folder that isn't the approved one goes straight back
/// off in [`apply`].
pub fn set_enabled(id: &str, on: bool, cx: &mut App) {
    if !on {
        set_developing(id, false);
    }

    edit(id, move |record| record.enabled = on);
    apply(cx);
}

/// The record's half of the scrobble gate; the manifest's is
/// [`declares_scrobble`].
pub fn set_scrobble(id: &str, on: bool) {
    edit(id, move |record| record.scrobble = on);
}

pub fn set_lyrics(id: &str, on: bool) {
    edit(id, move |record| record.lyrics = on);
}

/// One config value. The host picks it up when [`apply`] next runs, which
/// restarts it with the new config.
pub fn set_config(id: &str, key: &str, value: Value) {
    let key = key.to_string();
    edit(id, move |record| {
        if !record.config.is_object() {
            record.config = json!({});
        }
        if let Some(config) = record.config.as_object_mut() {
            config.insert(key, value);
        }
    });
}

fn edit(id: &str, change: impl FnOnce(&mut PluginRecord) + Send + 'static) {
    let id = id.to_string();
    Settings::update(move |s| {
        if let Some(record) = s.accounts.plugins.iter_mut().find(|r| r.id == id) {
            change(record);
        }
    });
}

/// Stops the plugin, drops its rows with their membership, forgets its
/// record and its approval, and leaves its folder where it is. Answers the
/// rows removed.
pub fn remove(id: &str, cx: &mut App) -> Task<Result<usize, String>> {
    let Some(library) = cx.try_global::<Wiring>().map(|w| w.library.clone()) else {
        return Task::ready(Err(NO_HOST.to_string()));
    };

    let source = source_of(id);
    let stopping = HOSTS
        .write()
        .ok()
        .and_then(|mut table| table.remove(&source));

    settings::revoke_plugin(id);
    set_developing(id, false);
    let gone = id.to_string();
    Settings::update(move |s| s.accounts.plugins.retain(|record| record.id != gone));
    // The write below reloads the projection, so apply needn't.
    live_moved();
    bump();

    write(library, cx, move |db_path| {
        // Stopped before the delete, so a sync in flight can't land rows
        // behind it.
        if let Some(running) = stopping {
            running.host.stop("removed");
        }

        let mut conn = store::open(&db_path).map_err(|e| e.to_string())?;
        members::remove_source(&mut conn, &source).map_err(|e| e.to_string())
    })
}

/// What changed in a manifest since it was approved, for the enable card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    CapabilityAdded(String),
    CapabilityRemoved(String),
    ProgramAdded(String),
    /// The scrobble declaration, on or off.
    Scrobble(bool),
    /// The entry, which is how the plugin runs.
    Entry,
    /// An action by its label: new, or declared differently.
    ActionAdded(String),
    ActionChanged(String),
}

/// Compares the manifests as written, so a capability this build doesn't
/// know yet still shows.
pub fn changes(approved: &Value, now: &Value) -> Vec<Change> {
    let keys = |doc: &Value| -> Vec<String> {
        doc["capabilities"]
            .as_object()
            .map(|caps| caps.keys().cloned().collect())
            .unwrap_or_default()
    };
    let programs = |doc: &Value| -> Vec<String> {
        doc["programs"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|p| p.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    let (was, is) = (keys(approved), keys(now));
    let mut found: Vec<Change> = is
        .iter()
        .filter(|key| !was.contains(key))
        .map(|key| Change::CapabilityAdded(key.clone()))
        .collect();
    found.extend(
        was.iter()
            .filter(|key| !is.contains(key))
            .map(|key| Change::CapabilityRemoved(key.clone())),
    );

    // Inside a source that was already approved, a feature switched on asks
    // for something new too, like links the user can open, or one declared
    // as an object, like favourites. A new source is already the capability
    // line above.
    let features = |doc: &Value| -> Vec<String> {
        doc["capabilities"]["source"]
            .as_object()
            .map(|source| {
                source
                    .iter()
                    .filter(|(key, value)| {
                        *key != "scrobble" && (value.as_bool() == Some(true) || value.is_object())
                    })
                    .map(|(key, _)| key.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    if approved["capabilities"]["source"].is_object() {
        let had = features(approved);
        found.extend(
            features(now)
                .into_iter()
                .filter(|feature| !had.contains(feature))
                .map(Change::CapabilityAdded),
        );
    }

    let actions = |doc: &Value| -> Vec<(String, Value)> {
        doc["capabilities"]["source"]["actions"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|action| {
                        (
                            action["id"].as_str().unwrap_or("").to_string(),
                            action.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let had = actions(approved);
    for (id, action) in actions(now) {
        let label = action["label"].as_str().unwrap_or(&id).to_string();

        match had.iter().find(|(old, _)| *old == id) {
            None => found.push(Change::ActionAdded(label)),
            Some((_, old)) if *old != action => found.push(Change::ActionChanged(label)),
            Some(_) => {}
        }
    }

    let before = programs(approved);
    found.extend(
        programs(now)
            .into_iter()
            .filter(|program| !before.contains(program))
            .map(Change::ProgramAdded),
    );

    let scrobbles = declares_scrobble_in(now);
    if scrobbles != declares_scrobble_in(approved) {
        found.push(Change::Scrobble(scrobbles));
    }

    if approved["entry"] != now["entry"] {
        found.push(Change::Entry);
    }

    found
}

/// What the engine reads a plugin track through. Reading a track to its end
/// can change what its row is, like a copy the plugin kept as it played, so
/// rox asks for the row's flags then, and once more after it closes.
struct Reader {
    stream: Option<Stream>,
    host: Host,
    source: String,
    key: String,
    asked: AtomicBool,
}

/// How long after a close rox asks again, for a copy the plugin finishes
/// after the stream is gone.
const FLAGS_AFTER_CLOSE: Duration = Duration::from_secs(10);

impl Reader {
    fn new(stream: Stream, host: Host, source: &str, key: &str) -> Reader {
        Reader {
            stream: Some(stream),
            host,
            source: source.to_string(),
            key: key.to_string(),
            asked: AtomicBool::new(false),
        }
    }

    fn ask_flags(&self, after: Duration) {
        if !asks_flags(&self.host) {
            return;
        }

        let (host, source, key) = (self.host.clone(), self.source.clone(), self.key.clone());
        let spawned = std::thread::Builder::new()
            .name("plugin-row-flags".into())
            .spawn(move || {
                std::thread::sleep(after);

                let params = json!({ "items": [key] });
                let answer = host
                    .call("source.flags", params, host.timeouts().listing)
                    .and_then(wire::decode::<wire::FlagsAnswer>);
                match answer {
                    Ok(answer) => crate::plugin_actions::report(&source, answer.flags),
                    Err(e) => log::debug!("{source}: flags for {key}: {e}"),
                }
            });

        if let Err(e) = spawned {
            log::warn!(
                "{}: couldn't ask for {}'s flags: {e}",
                self.source,
                self.key
            );
        }
    }
}

impl ReadAt for Reader {
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let Some(stream) = &self.stream else {
            return Ok(Vec::new());
        };
        let data = stream.read_at(offset, len)?;

        let end = offset + data.len() as u64;
        let whole = stream.length.is_some_and(|length| end >= length);
        if whole && !self.asked.swap(true, Ordering::Relaxed) {
            self.ask_flags(Duration::ZERO);
        }

        Ok(data)
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // The close goes out first, so the plugin knows the track ended.
        drop(self.stream.take());
        self.ask_flags(FLAGS_AFTER_CLOSE);
    }
}

/// Whether the plugin gates any action on a flag, the only reason to ask.
fn asks_flags(host: &Host) -> bool {
    host.manifest()
        .capabilities
        .source
        .as_ref()
        .is_some_and(|cap| cap.actions.iter().any(|action| !action.when.is_empty()))
}

/// How the engine's opens were answered this run, for the prototype's
/// measurements.
static OPENED_WARM: AtomicU64 = AtomicU64::new(0);
static OPENED_JOINED: AtomicU64 = AtomicU64::new(0);
static OPENED_COLD: AtomicU64 = AtomicU64::new(0);

/// For the blocking paths that can run before the first [`apply`]. Answers
/// at once when [`start`] never ran, since then nothing opens the gate.
pub(crate) fn await_first_apply(source: &str) {
    if FIRST_APPLY.is_open() || !STARTED.load(Ordering::Relaxed) {
        return;
    }

    let began = Instant::now();
    let ready = FIRST_APPLY.wait(FIRST_APPLY_WAIT);
    log::info!(
        "{source}: waited {:.0} ms for the first apply{}",
        began.elapsed().as_secs_f64() * 1000.0,
        if ready { "" } else { ", and gave up" }
    );
}

/// Where a part of a plugin track starts, for the seek strip's marks.
#[derive(Clone, Debug, PartialEq)]
pub struct Chapter {
    pub start_secs: f64,
    pub title: String,
}

/// The chapters of the last few tracks opened, newest last. Only the
/// playing track's are ever asked for, so a handful covers a skip back.
static CHAPTERS: Mutex<Vec<(SourceKey, Arc<[Chapter]>)>> = Mutex::new(Vec::new());
type SourceKey = (String, String);
const CHAPTERS_KEPT: usize = 8;

/// The wire refuses only the shape, so a list out of order or with blank
/// titles still plays: it's sorted, and what can't be drawn drops here.
fn tidy_chapters(sent: &[wire::Chapter]) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = sent
        .iter()
        .filter(|chapter| !chapter.title.trim().is_empty())
        .map(|chapter| Chapter {
            start_secs: chapter.start_ms as f64 / 1000.0,
            title: chapter.title.trim().to_string(),
        })
        .collect();

    chapters.sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
    chapters.dedup_by(|later, earlier| later.start_secs == earlier.start_secs);
    chapters
}

fn note_chapters(source: &str, key: &str, sent: &[wire::Chapter]) {
    let id = (source.to_string(), key.to_string());
    let mut kept = CHAPTERS.lock().unwrap_or_else(|e| e.into_inner());
    kept.retain(|(at, _)| *at != id);

    let chapters = tidy_chapters(sent);
    if chapters.is_empty() {
        return;
    }

    kept.push((id, chapters.into()));
    if kept.len() > CHAPTERS_KEPT {
        kept.remove(0);
    }
}

/// What the last open of this track said its chapters are. Empty before
/// it opens, and for a plugin that sends none.
pub fn chapters(source: &str, key: &str) -> Arc<[Chapter]> {
    CHAPTERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|((s, k), _)| s == source && k == key)
        .map(|(_, chapters)| chapters.clone())
        .unwrap_or_else(|| Arc::from([]))
}

/// What the engine calls, on its decode thread, for every plugin entry.
fn open(stream: &PluginStream) -> Result<Opened, String> {
    await_first_apply(&stream.source);

    let host = host_for(&stream.source)?;
    let began = Instant::now();

    let (opened, how) = match take_preopened(&stream.source, &stream.key, host.timeouts().open) {
        Some((opened, joined)) => {
            let counter = if joined { &OPENED_JOINED } else { &OPENED_WARM };
            counter.fetch_add(1, Ordering::Relaxed);
            (
                opened,
                if joined {
                    "joined a pre-open"
                } else {
                    "pre-opened"
                },
            )
        }
        None => {
            OPENED_COLD.fetch_add(1, Ordering::Relaxed);
            (
                Stream::open(&host, &stream.key, Options::default())?,
                "cold",
            )
        }
    };

    log::info!(
        "{}: open {} {how} in {:.0} ms (pre-opened {}, joined {}, cold {} this run)",
        stream.source,
        stream.key,
        began.elapsed().as_secs_f64() * 1000.0,
        OPENED_WARM.load(Ordering::Relaxed),
        OPENED_JOINED.load(Ordering::Relaxed),
        OPENED_COLD.load(Ordering::Relaxed),
    );

    if !stream.live {
        note_chapters(&stream.source, &stream.key, &opened.chapters);
    }

    Ok(Opened {
        hint: opened.hint.clone(),
        length: opened.length,
        seekable: opened.seekable,
        buffer_whole: opened.buffer_whole,
        duration_ms: opened.duration_ms,
        reader: Box::new(Reader::new(
            opened,
            host.clone(),
            &stream.source,
            &stream.key,
        )),
    })
}

/// A pre-open's result, filled once, waited on by an engine open that
/// arrives while it's still under way.
type Slot = Arc<(Mutex<Option<Result<Stream, String>>>, Condvar)>;

struct Preopen {
    since: Instant,
    slot: Slot,
}

static PREOPENED: LazyLock<Mutex<HashMap<(String, String), Preopen>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The stream, and whether it was still opening when asked for.
fn take_preopened(source: &str, key: &str, wait: Duration) -> Option<(Stream, bool)> {
    let pending = PREOPENED
        .lock()
        .ok()?
        .remove(&(source.to_string(), key.to_string()))?;

    let (lock, ready) = &*pending.slot;
    let guard = lock.lock().ok()?;
    let joined = guard.is_none();
    let (mut guard, _) = ready
        .wait_timeout_while(guard, wait, |result| result.is_none())
        .ok()?;

    match guard.take()? {
        // Opened on a process that has since exited: open it fresh.
        Ok(stream) if stream.alive() => Some((stream, joined)),
        Ok(_) => None,
        Err(e) => {
            log::info!("{source}: the pre-open of {key} failed: {e}");
            None
        }
    }
}

fn preopen(stream: PluginStream, cx: &App) {
    let Ok(host) = host_for(&stream.source) else {
        return;
    };

    let slot: Slot = Arc::new((Mutex::new(None), Condvar::new()));
    {
        let Ok(mut table) = PREOPENED.lock() else {
            return;
        };
        let id = (stream.source.clone(), stream.key.clone());
        if table.contains_key(&id) {
            return;
        }
        table.insert(
            id,
            Preopen {
                since: Instant::now(),
                slot: Arc::clone(&slot),
            },
        );
    }

    cx.background_executor()
        .spawn(async move {
            let began = Instant::now();
            let result = Stream::open(&host, &stream.key, Options::default());
            log::debug!(
                "{}: pre-opened {} in {:.0} ms",
                stream.source,
                stream.key,
                began.elapsed().as_secs_f64() * 1000.0
            );

            let (lock, ready) = &*slot;
            if let Ok(mut filled) = lock.lock() {
                *filled = Some(result);
            }
            ready.notify_all();
        })
        .detach();
}

/// Drops pre-opens nobody took. One still opening goes when it finishes and
/// its last handle drops.
fn sweep() {
    if let Ok(mut table) = PREOPENED.lock() {
        table.retain(|_, preopen| preopen.since.elapsed() < PREOPEN_KEEP);
    }
}

/// Open the next plugin tracks before the engine asks for them, once the
/// audible track has held for [`PREOPEN_SETTLE`] and is within
/// [`PREOPEN_LEAD`] of its crossfade. Skipping through faster than that opens
/// nothing: every open is the plugin's work and a request to its service.
/// Opening at the start of a long track would see the sweep close the stream
/// before the boundary, and the engine open it cold mid-fade. Live streams
/// aren't opened early either, since that would start a broadcast nobody
/// hears yet.
fn follow(player: Entity<Player>, cx: &mut App) {
    let mut audible: Option<usize> = None;
    let mut since = Instant::now();
    let mut opened = false;

    // The pump notifies every tick while playing, so this sees the window open.
    cx.observe(&player, move |player, cx| {
        let player = player.read(cx);
        let now = player.now_playing();

        let idx = now.as_ref().map(|now| now.audible_idx);
        if idx != audible {
            audible = idx;
            since = Instant::now();
            opened = false;
            sweep();
        }

        let Some(now) = now else {
            return;
        };

        let left = now
            .duration_secs
            .map(|total| (total - now.position_secs).max(0.0));

        // A seek back out of the window opens again when it comes round.
        if !preopen_due(since.elapsed(), left, player.crossfade_secs()) {
            opened = false;
            return;
        }

        if opened {
            return;
        }
        opened = true;

        for stream in worth_preopening(player.upcoming_locators(PREOPEN_AHEAD), left) {
            preopen(stream, cx);
        }
    })
    .detach();

    // A paused player notifies nothing, so the sweep has its own clock.
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(PREOPEN_KEEP / 2).await;
            sweep();
        }
    })
    .detach();
}

/// Held long enough, and near enough the end. A track of unknown length
/// has no end to count back from, so it opens what's next once it holds.
fn preopen_due(held: Duration, left: Option<f64>, fade_secs: f32) -> bool {
    if held < PREOPEN_SETTLE {
        return false;
    }

    let lead = fade_secs.max(0.0) as f64 + PREOPEN_LEAD.as_secs_f64();
    left.is_none_or(|left| left <= lead)
}

/// The next entry, and past it only what starts before the sweep would
/// close it. A long track ahead leaves the one behind it to its own window.
fn worth_preopening(upcoming: Vec<Locator>, left: Option<f64>) -> Vec<PluginStream> {
    let keep = PREOPEN_KEEP.as_secs_f64();
    let mut starts_in = left;
    let mut streams = Vec::new();

    for (i, locator) in upcoming.into_iter().enumerate() {
        if i > 0 && starts_in.is_none_or(|secs| secs >= keep) {
            break;
        }

        // A file or a remote row has no length here to count past.
        let Locator::Plugin(stream) = locator else {
            break;
        };

        starts_in = starts_in
            .zip(stream.duration_ms)
            .map(|(secs, ms)| secs + ms as f64 / 1000.0);

        if !stream.live {
            streams.push(stream);
        }
    }

    streams
}

fn track(mut wire: wire::Track) -> PluginTrack {
    let go_to = wire
        .go_to
        .take()
        .map(|nodes| go_to_json(&go_to_of(*nodes)))
        .unwrap_or_default();

    PluginTrack {
        key: wire.key,
        title: wire.title,
        artist: wire.artist,
        album_artist: wire.album_artist,
        album: wire.album,
        genre: wire.genre,
        year: wire.year,
        disc_no: wire.disc_no,
        track_no: wire.track_no,
        duration_ms: wire.duration_ms,
        codec: wire.codec,
        bitrate_kbps: wire.bitrate_kbps,
        live: wire.live,
        go_to,
    }
}

fn page(wire: wire::Page) -> Page {
    let mut go_to = HashMap::new();
    let mut flags = HashMap::new();

    let (entries, values) = wire
        .entries
        .into_iter()
        .map(|entry| match entry {
            wire::Entry::Node(mut node) => {
                if let Some(known) = node.flags.take() {
                    flags.insert(node.id.clone(), known);
                }

                (
                    Entry::Node {
                        id: node.id,
                        title: node.title,
                        subtitle: node.subtitle,
                        collection: node.collection,
                        kind: node.kind,
                        art: node.art,
                        home: node.home,
                    },
                    values_of(node.values),
                )
            }
            wire::Entry::Track(mut t) => {
                let values = values_of(std::mem::take(&mut t.values));
                if let Some(known) = t.flags.take() {
                    flags.insert(t.key.clone(), known);
                }
                if let Some(nodes) = &t.go_to {
                    go_to.insert(t.key.clone(), go_to_of((**nodes).clone()));
                }

                (Entry::Track(track(t)), values)
            }
            wire::Entry::Section(section) => (
                Entry::Section {
                    title: section.title,
                    tiles: section.layout == wire::Layout::Tiles,
                },
                Values::new(),
            ),
        })
        .unzip();

    Page {
        entries,
        cursor: wire.cursor,
        notice: wire.notice.map(|notice| Notice {
            text: notice.text,
            setup: notice.kind == wire::NoticeKind::Setup,
            link: notice.link.map(|link| NoticeLink {
                url: link.url,
                label: link.label,
            }),
        }),
        views: wire
            .views
            .into_iter()
            .map(|view| View {
                id: view.id,
                label: view.label,
            })
            .collect(),
        view: wire.view,
        fields: wire
            .fields
            .into_iter()
            .map(|field| Field {
                id: field.id,
                label: field.label,
                kind: field.kind,
            })
            .collect(),
        values,
        go_to,
        flags,
    }
}

fn go_to_json(go_to: &GoTo) -> String {
    serde_json::to_string(go_to).unwrap_or_default()
}

/// Stores Go to a listing showed for rows already in the library, so rows
/// that became rows before Go to was kept get it too. Blocking.
pub fn keep_go_to(db_path: &Path, source: &str, listed: &HashMap<String, GoTo>) {
    let found: Vec<(String, String)> = listed
        .iter()
        .map(|(key, go_to)| (key.clone(), go_to_json(go_to)))
        .collect();
    if found.is_empty() {
        return;
    }

    let wrote =
        store::open(db_path).and_then(|mut conn| members::keep_go_to(&mut conn, source, &found));
    if let Err(e) = wrote {
        log::warn!("{source}: keeping Go to from a listing: {e}");
    }
}

/// The Go to the library keeps for one plugin row, or None.
pub fn stored_go_to(db_path: &Path, source: &str, key: &str) -> Option<GoTo> {
    let conn = store::open(db_path).ok()?;
    let mut stored = members::go_to(&conn, source, &[key.to_string()]).ok()?;

    go_to_from(&stored.remove(key)?)
}

/// Go to as the library keeps it, or None for what doesn't parse.
pub fn go_to_from(stored: &str) -> Option<GoTo> {
    serde_json::from_str(stored).ok()
}

fn go_to_of(wire: wire::GoTo) -> GoTo {
    GoTo {
        album: wire.album.map(Target::from),
        artists: wire.artists.into_iter().map(Target::from).collect(),
    }
}

fn listing(
    source: &str,
    method: &'static str,
    params: Value,
    cx: &App,
) -> Task<Result<Page, String>> {
    let source = source.to_string();

    cx.background_executor().spawn(async move {
        // A panel restored at launch lists before the first apply has built
        // the hosts, and would otherwise read that as no host at all.
        await_first_apply(&source);
        let host = host_for(&source)?;

        let timeout = host.timeouts().listing;
        let listed = host
            .call(method, params, timeout)
            .and_then(wire::decode::<wire::Page>)
            .map(page)?;

        crate::plugin_actions::note(&source, &listed.flags);
        Ok(listed)
    })
}

/// `node: None` asks for the roots. `view` is one the place's first page
/// offered, or None for the plugin's default.
pub fn browse(
    source: &str,
    node: Option<String>,
    view: Option<String>,
    cursor: Option<String>,
    cx: &App,
) -> Task<Result<Page, String>> {
    let mut params = json!({ "node": node, "cursor": cursor });
    with_view(&mut params, view);

    listing(source, "source.browse", params, cx)
}

pub fn search(
    source: &str,
    query: String,
    view: Option<String>,
    cursor: Option<String>,
    cx: &App,
) -> Task<Result<Page, String>> {
    let mut params = json!({ "query": query, "cursor": cursor });
    with_view(&mut params, view);

    listing(source, "source.search", params, cx)
}

/// Only sent once the plugin offered views, so a plugin that never heard of
/// them never sees the key.
fn with_view(params: &mut Value, view: Option<String>) {
    if let (Some(view), Some(params)) = (view, params.as_object_mut()) {
        params.insert("view".into(), Value::String(view));
    }
}

/// Whether a running plugin's manifest offers a radio.
pub fn has_radio(source: &str) -> bool {
    offers(source, |cap| cap.radio)
}

/// Whether any running plugin offers a radio, which is enough for Similar
/// shuffle to have something to draw from without acoustic analysis.
pub fn any_radio() -> bool {
    if !allowed() {
        return false;
    }

    HOSTS.read().is_ok_and(|table| {
        table.values().any(|running| {
            running
                .host
                .manifest()
                .capabilities
                .source
                .as_ref()
                .is_some_and(|cap| cap.radio)
        })
    })
}

/// Whether a running plugin's manifest offers links to its tracks and nodes.
pub fn has_links(source: &str) -> bool {
    offers(source, |cap| cap.links)
}

fn offers(source: &str, what: impl Fn(&SourceCap) -> bool) -> bool {
    running(source).is_some_and(|running| {
        running
            .host
            .manifest()
            .capabilities
            .source
            .as_ref()
            .is_some_and(what)
    })
}

/// The web page of a track's key or a node's id, asked when the user picks
/// Open in Browser or Copy Link. None when the plugin has no page for it.
pub fn link(source: &str, item: String, cx: &App) -> Task<Result<Option<String>, String>> {
    let source = source.to_string();

    cx.background_executor().spawn(async move {
        await_first_apply(&source);
        let host = host_for(&source)?;

        let answer = host.call(
            "source.link",
            json!({ "item": item }),
            host.timeouts().listing,
        )?;
        if answer.is_null() {
            return Ok(None);
        }

        wire::decode::<wire::Link>(answer).map(|link| Some(link.url))
    })
}

/// The label of a running plugin whose lyrics the user switched on, or
/// None. Reads the settings file, so ask once per track, not per frame.
pub fn lyrics_from(source: &str) -> Option<String> {
    let id = source.strip_prefix(PLUGIN_PREFIX)?;
    let on = Settings::load()
        .accounts
        .plugins
        .iter()
        .any(|record| record.id == id && record.lyrics);
    if !on {
        return None;
    }

    let running = running(source)?;
    let cap = running.host.manifest().capabilities.source.as_ref()?;

    cap.lyrics.then(|| cap.label.clone())
}

/// A track's sheet from its plugin: the text, and whether it's LRC. None
/// when the plugin has none. Blocking.
pub fn lyrics(source: &str, key: &str) -> Result<Option<(String, bool)>, String> {
    await_first_apply(source);
    let host = host_for(source)?;

    let answer = host.call(
        "source.lyrics",
        json!({ "key": key }),
        host.timeouts().listing,
    )?;
    if answer.is_null() {
        return Ok(None);
    }

    wire::decode::<wire::LyricsAnswer>(answer).map(|sheet| Some((sheet.text, sheet.synced)))
}

/// One batch of a station, and where the next starts. Blocking.
fn radio_page(
    source: &str,
    seed: &str,
    cursor: Option<String>,
) -> Result<(Vec<PluginTrack>, Option<String>), String> {
    await_first_apply(source);
    let host = host_for(source)?;

    let params = json!({ "seed": seed, "cursor": cursor, "count": continuation::BATCH });
    let page = host
        .call("source.radio", params, host.timeouts().listing)
        .and_then(wire::decode::<wire::RadioPage>)?;

    Ok((page.tracks.into_iter().map(track).collect(), page.cursor))
}

/// A plugin's station, drawn from while the context it started plays (ADR
/// 17). Its tracks become picks as they're drawn: played, never kept.
struct Station {
    source: String,
    db_path: PathBuf,
    /// The seed the station plays from, and where its next batch starts. An
    /// empty seed is a station that follows a play: it seeds from the last of
    /// the source's tracks heard when the queue first runs low.
    at: Mutex<(String, Option<String>)>,
    /// The track Play Similar started from, which never plays as part of it.
    skip: Option<String>,
}

/// The saved form of a station: which plugin, and where it had got to.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedStation {
    source: String,
    seed: String,
    cursor: Option<String>,
    #[serde(default)]
    skip: Option<String>,
}

fn station_db(cx: &App) -> Option<PathBuf> {
    Some(cx.try_global::<Wiring>()?.library.read(cx).db_path())
}

/// What a play from a plugin's panel continues with when the queue runs low:
/// the plugin's radio, where it has one, rather than the local library.
pub fn follow_on(source: &str, cx: &App) -> Option<continuation::Scope> {
    if !has_radio(source) {
        return None;
    }

    let station = Station {
        source: source.to_string(),
        db_path: station_db(cx)?,
        at: Mutex::new((String::new(), None)),
        skip: None,
    };

    Some(continuation::Scope::Provided(Arc::new(station)))
}

/// A station saved with the queue, brought back at launch. The plugin may
/// not be running yet; its first draw waits for the hosts like any call.
pub fn station_from(saved: &str, cx: &App) -> Option<continuation::Scope> {
    let saved: SavedStation = serde_json::from_str(saved).ok()?;
    if !saved.source.starts_with(PLUGIN_PREFIX) {
        return None;
    }

    let station = Station {
        source: saved.source,
        db_path: station_db(cx)?,
        at: Mutex::new((saved.seed, saved.cursor)),
        skip: saved.skip,
    };

    Some(continuation::Scope::Provided(Arc::new(station)))
}

/// A station that only hands back what already played gets this many more
/// asks before the queue ends.
const STATION_TRIES: usize = 3;

impl continuation::Provider for Station {
    fn saved(&self) -> Option<String> {
        let (seed, cursor) = self.at.lock().ok()?.clone();
        let saved = SavedStation {
            source: self.source.clone(),
            seed,
            cursor,
            skip: self.skip.clone(),
        };

        serde_json::to_string(&saved).ok()
    }

    fn next(&self, conn: &Connection, seed: &continuation::Seed) -> Vec<Pick> {
        let seen: HashSet<i64> = seed.recent.iter().copied().collect();

        for _ in 0..STATION_TRIES {
            let Ok(mut at) = self.at.lock() else {
                return Vec::new();
            };
            let (station, cursor) = at.clone();

            // A station that follows a play seeds now, from where it got to.
            let station = match station.is_empty() {
                true => match self.last_heard(conn, seed) {
                    Some(heard) => heard,
                    None => return Vec::new(),
                },
                false => station,
            };

            let (tracks, next) = match radio_page(&self.source, &station, cursor) {
                Ok(page) => page,
                Err(e) => {
                    log::warn!("{}: radio from {station}: {e}", self.source);
                    return Vec::new();
                }
            };

            // A station that ran out carries on from the last of its tracks
            // heard, the way a service's own radio drifts.
            *at = match next {
                Some(next) => (station, Some(next)),
                None => (self.last_heard(conn, seed).unwrap_or(station), None),
            };
            drop(at);

            let picks = self.pick(&tracks, &seen, seed.count);
            if !picks.is_empty() {
                return picks;
            }
        }

        Vec::new()
    }
}

impl Station {
    fn last_heard(&self, conn: &Connection, seed: &continuation::Seed) -> Option<String> {
        seed.recent.iter().rev().find_map(|&id| {
            let key = store::key_for_id(conn, id).ok().flatten()?;
            (key.source.as_ref() == self.source).then(|| key.path.to_string_lossy().into_owned())
        })
    }

    /// Writes the tracks as picks and answers their ids, less what played
    /// and the track Play Similar started from.
    fn pick(&self, tracks: &[PluginTrack], seen: &HashSet<i64>, count: usize) -> Vec<Pick> {
        let tracks: Vec<PluginTrack> = tracks
            .iter()
            .filter(|track| self.skip.as_ref() != Some(&track.key))
            .cloned()
            .collect();

        let write = || -> Result<Vec<i64>, String> {
            let mut conn = store::open(&self.db_path).map_err(|e| e.to_string())?;
            let keys =
                members::pick(&mut conn, &self.source, &tracks).map_err(|e| e.to_string())?;

            Ok(keys
                .iter()
                .filter_map(|key| {
                    let path = key.path.to_string_lossy();
                    store::id_for_path(&conn, &self.source, &path)
                        .ok()
                        .flatten()
                })
                .collect())
        };

        let ids = match write() {
            Ok(ids) => ids,
            Err(e) => {
                log::warn!("{}: radio rows: {e}", self.source);
                return Vec::new();
            }
        };

        let mut kept = HashSet::new();
        ids.into_iter()
            .filter(|id| !seen.contains(id) && kept.insert(*id))
            .take(count)
            .map(|id| Pick { id, group: None })
            .collect()
    }
}

/// Most of a node's tracks rox reads to play it: an album or a playlist,
/// not a whole catalogue.
const NODE_TRACKS: usize = 1000;

/// Pages a node gets to reach [`NODE_TRACKS`], so a plugin paging empty
/// pages can't keep a play waiting.
const NODE_PAGES: usize = 50;

/// Every track a node lists, across its pages, in the plugin's default view.
/// Blocking.
fn list_node(source: &str, node: &str) -> Result<Vec<PluginTrack>, String> {
    await_first_apply(source);
    let host = host_for(source)?;
    let timeout = host.timeouts().listing;

    let mut tracks = Vec::new();
    let mut cursor: Option<String> = None;

    for _ in 0..NODE_PAGES {
        let params = json!({ "node": node, "cursor": cursor });
        let page = host
            .call("source.browse", params, timeout)
            .and_then(wire::decode::<wire::Page>)?;

        tracks.extend(page.entries.into_iter().filter_map(|entry| match entry {
            wire::Entry::Track(t) => Some(track(t)),
            _ => None,
        }));

        cursor = page.cursor;
        if cursor.is_none() || tracks.len() >= NODE_TRACKS {
            break;
        }
    }

    tracks.truncate(NODE_TRACKS);
    Ok(tracks)
}

/// A node's tracks, for playing or queueing it whole.
pub fn node_tracks(source: &str, node: &str, cx: &App) -> Task<Result<Vec<PluginTrack>, String>> {
    let (source, node) = (source.to_string(), node.to_string());
    cx.background_executor()
        .spawn(async move { list_node(&source, &node) })
}

/// What a radio starts from, which plays before the station does.
pub enum RadioSeed {
    Track(PluginTrack),
    /// A node's id: its own tracks play first, like an album before its
    /// artist's radio.
    Node(String),
}

/// Play Similar: a node's own tracks now, then the station's first batch,
/// and the rest as continuation draws it under Similar shuffle, which this
/// turns on. A track seeds the station without playing itself.
pub fn play_similar(
    library: Entity<Library>,
    player: Entity<Player>,
    source: &str,
    seed: RadioSeed,
    cx: &mut App,
) -> Task<Result<(), String>> {
    let source = source.to_string();
    let db_path = library.read(cx).db_path();

    cx.spawn(async move |cx| {
        let asked = source.clone();
        let (seed, lead, skip, (batch, cursor)) = cx
            .background_executor()
            .spawn(async move {
                let (seed, lead, skip) = match seed {
                    RadioSeed::Track(track) => (track.key.clone(), Vec::new(), Some(track.key)),
                    RadioSeed::Node(node) => {
                        let lead = list_node(&asked, &node)?;
                        (node, lead, None)
                    }
                };

                let first = radio_page(&asked, &seed, None)?;
                Ok::<_, String>((seed, lead, skip, first))
            })
            .await?;

        // The station often opens on its seed, which already led or is the
        // track asked to be skipped.
        let mut tracks = lead;
        let mut listed: HashSet<String> = tracks.iter().map(|t| t.key.clone()).collect();
        listed.extend(skip.clone());
        tracks.extend(batch.into_iter().filter(|t| listed.insert(t.key.clone())));

        if tracks.is_empty() {
            return Err("the radio came back empty".to_string());
        }

        let picked = cx
            .update(|cx| pick(library, &source, tracks, cx))
            .map_err(|e| e.to_string())?;
        let keys = picked.await?;

        let station = Station {
            source,
            db_path,
            at: Mutex::new((seed, cursor)),
            skip,
        };

        player
            .update(cx, |player, cx| {
                player.play_at(keys, 0, cx);
                // After the play: starting a session clears the scope.
                player.set_scope(continuation::Scope::Provided(Arc::new(station)));
                player.shuffle_in_mode(ShuffleMode::Similar, cx);
            })
            .map_err(|e| e.to_string())
    })
}

/// Play Similar from the playing track, when it's a plugin's and the plugin
/// has a radio. None otherwise, so the library's own Similar can answer.
pub fn play_similar_to_playing(
    library: Entity<Library>,
    player: Entity<Player>,
    cx: &mut App,
) -> Option<Task<Result<(), String>>> {
    let key = player.read(cx).now_playing()?.key;
    play_similar_to_key(library, player, &key, cx)
}

/// Play Similar from a library row, when it's a plugin's and the plugin has
/// a radio. None otherwise.
pub fn play_similar_to_key(
    library: Entity<Library>,
    player: Entity<Player>,
    key: &TrackKey,
    cx: &mut App,
) -> Option<Task<Result<(), String>>> {
    if key.origin() != rox_library::cue::Origin::Plugin {
        return None;
    }

    let source = key.source.to_string();
    if !has_radio(&source) {
        return None;
    }

    // The row as the plugin gave it, so the pick that leads rewrites nothing.
    let conn = store::open(&library.read(cx).db_path()).ok()?;
    let item = key.path.to_string_lossy();
    let track = members::track(&conn, &source, &item).ok().flatten()?;

    Some(play_similar(
        library,
        player,
        &source,
        RadioSeed::Track(track),
        cx,
    ))
}

/// Rows written outside a panel's own write, like a radio's, reach the
/// views on the next projection.
pub fn reload_library(cx: &mut App) {
    let Some(wiring) = cx.try_global::<Wiring>() else {
        return;
    };

    let library = wiring.library.clone();
    library.update(cx, |library, cx| library.reload_projection(cx));
}

/// The asset path of an action's icon, when its plugin ships one.
pub fn action_icon(source: &str, action: &str) -> Option<gpui::SharedString> {
    let id = source.strip_prefix(PLUGIN_PREFIX)?;
    let folders = FOLDERS.read().ok()?;
    let folder = folders
        .iter()
        .find(|folder| folder.id == id && folder.runs())?;
    let (_, bytes) = folder
        .action_icons
        .iter()
        .find(|(name, _)| name == action)?;

    // Keyed apart from the source icon and every other action's.
    Some(rox_design::assets::plugin_icon(
        &format!("{id}.action.{action}"),
        &folder.hash,
        bytes,
    ))
}

/// The asset path of a loaded plugin's icon, when it ships one.
pub fn icon(source: &str) -> Option<gpui::SharedString> {
    let id = source.strip_prefix(PLUGIN_PREFIX)?;
    let folders = FOLDERS.read().ok()?;
    let folder = folders
        .iter()
        .find(|folder| folder.id == id && folder.runs())?;
    let bytes = folder.icon.as_deref()?;

    Some(rox_design::assets::plugin_icon(id, &folder.hash, bytes))
}

/// Asks the plugin what each of its library rows is right now, so their
/// menus offer only the actions that apply (ADR 30, amended 2026-10-02).
/// Only a plugin with a `when` on some action is asked. One that doesn't
/// answer `source.flags` leaves its rows unknown, which offers every action.
fn refresh_flags(host: &Host, db_path: &Path, source: &str) {
    if !asks_flags(host) {
        return;
    }

    let keys = match store::open(db_path).and_then(|conn| members::keys(&conn, source)) {
        Ok(keys) => keys,
        Err(e) => {
            log::warn!("{source}: reading rows for their flags: {e}");
            return;
        }
    };

    for chunk in keys.chunks(wire::MAX_ENTRIES) {
        let params = json!({ "items": chunk });
        let answer = host
            .call("source.flags", params, host.timeouts().listing)
            .and_then(wire::decode::<wire::FlagsAnswer>);

        match answer {
            Ok(answer) => crate::plugin_actions::note(source, &answer.flags),
            Err(e) => {
                log::info!("{source}: no flags for its library rows: {e}");
                return;
            }
        }
    }
}

/// Every page of one collection. None when the plugin says nothing changed
/// since `token`; the stored token then stands, whatever the answer carried.
fn fetch_collection(
    host: &Host,
    collection: &str,
    token: &str,
) -> Result<Option<(Vec<PluginTrack>, String)>, String> {
    let timeouts = host.timeouts();
    let mut tracks = Vec::new();
    let mut cursor: Option<String> = None;

    for page in 0..MAX_SYNC_PAGES {
        let first = page == 0;
        let timeout = match first {
            true => timeouts.sync_first,
            false => timeouts.listing,
        };

        // Only the first page carries the stored token; the rest carry the cursor.
        let sent = (first && !token.is_empty()).then_some(token);
        let params = json!({ "collection": collection, "token": sent, "cursor": cursor });
        let answer: wire::SyncPage = wire::decode(host.call("source.sync", params, timeout)?)?;

        if first && answer.unchanged {
            return Ok(None);
        }

        tracks.extend(answer.tracks.into_iter().map(track));

        match answer.cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(Some((tracks, answer.token.unwrap_or_default()))),
        }
    }

    Err(format!(
        "{collection} is still paging after {MAX_SYNC_PAGES} pages"
    ))
}

/// Sync one collection into the library: the whole membership in one write,
/// only after the last page. Answers the rows written, None when unchanged.
fn sync_collection(
    host: &Host,
    db_path: &Path,
    source: &str,
    synced: &SyncedCollection,
) -> Result<Option<usize>, String> {
    let began = Instant::now();

    let resync = store::open(db_path)
        .and_then(|conn| members::needs_resync(&conn, source, &synced.id))
        .unwrap_or(false);
    let token = match resync {
        true => "",
        false => synced.token.as_str(),
    };

    let Some((tracks, token)) = fetch_collection(host, &synced.id, token)? else {
        log::info!("{source}: {} unchanged", synced.id);
        return Ok(None);
    };

    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    members::set_collection(&mut conn, source, &synced.id, &tracks).map_err(|e| e.to_string())?;

    log::info!(
        "{source}: synced {} ({} tracks) in {:.1} s",
        synced.id,
        tracks.len(),
        began.elapsed().as_secs_f64()
    );

    let (id, collection) = (
        record_id(source).unwrap_or_default().to_string(),
        synced.id.clone(),
    );
    Settings::update(move |s| {
        let found = s
            .accounts
            .plugins
            .iter_mut()
            .find(|record| record.id == id)
            .and_then(|record| record.synced.iter_mut().find(|c| c.id == collection));
        if let Some(stored) = found {
            stored.token = token;
        }
    });
    bump();

    Ok(Some(tracks.len()))
}

fn synced_of(source: &str) -> Vec<SyncedCollection> {
    let Some(id) = record_id(source) else {
        return Vec::new();
    };

    Settings::load()
        .accounts
        .plugins
        .into_iter()
        .find(|record| record.id == id)
        .map(|record| record.synced)
        .unwrap_or_default()
}

/// Runs `work` on the background executor with the library's database path,
/// then reloads the projection whatever it answered.
fn write<T: Send + 'static>(
    library: Entity<Library>,
    cx: &mut App,
    work: impl FnOnce(PathBuf) -> Result<T, String> + Send + 'static,
) -> Task<Result<T, String>> {
    write_if(library, cx, move |db_path| (work(db_path), true))
}

/// [`write`] for work that can find nothing to change, which answers whether
/// it wrote. A reload swaps the projection under every panel, so one that
/// changed nothing shows as a flicker.
fn write_if<T: Send + 'static>(
    library: Entity<Library>,
    cx: &mut App,
    work: impl FnOnce(PathBuf) -> (Result<T, String>, bool) + Send + 'static,
) -> Task<Result<T, String>> {
    let db_path = library.read(cx).db_path();

    cx.spawn(async move |cx| {
        let (result, wrote) = cx
            .background_executor()
            .spawn(async move { work(db_path) })
            .await;

        if wrote {
            library
                .update(cx, |library, cx| library.reload_projection(cx))
                .ok();
        }

        result
    })
}

/// Turns a collection's sync on or off. Answers the rows it now holds.
/// `collection` carries the node as listed; its token is ignored.
pub fn set_synced(
    library: Entity<Library>,
    source: &str,
    collection: SyncedCollection,
    on: bool,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    let Some(id) = record_id(source).map(str::to_string) else {
        return Task::ready(Err(NO_HOST.to_string()));
    };

    // Only a sync needs the plugin. Stopping is a settings change and a
    // delete, so it works with the plugin stopped or crashed.
    let host = match on {
        true => match host_for(source) {
            Ok(host) => Some(host),
            Err(e) => return Task::ready(Err(e)),
        },
        false => None,
    };

    let collection = SyncedCollection {
        token: String::new(),
        ..collection
    };
    let stored = collection.clone();
    Settings::update(move |s| {
        let Some(record) = s.accounts.plugins.iter_mut().find(|r| r.id == id) else {
            return;
        };

        record.synced.retain(|c| c.id != stored.id);
        if on {
            record.synced.push(stored);
        }
    });
    bump();

    let source = source.to_string();
    write(library, cx, move |db_path| match host {
        Some(host) => {
            let synced = sync_collection(&host, &db_path, &source, &collection);
            refresh_flags(&host, &db_path, &source);
            synced.map(|rows| rows.unwrap_or(0))
        }

        None => {
            let mut conn = store::open(&db_path).map_err(|e| e.to_string())?;
            members::drop_collection(&mut conn, &source, &collection.id)
                .map(|_| 0)
                .map_err(|e| e.to_string())
        }
    })
}

/// Brings kept collections' names, lines, kinds and art up to date with how
/// a listing just showed them, so the library draws them the same way with
/// the plugin stopped. Every call writes settings, so `seen` holds only the
/// collections whose look changed.
pub fn restyle_synced(source: &str, seen: Vec<SyncedCollection>) {
    let Some(id) = record_id(source).map(str::to_string) else {
        return;
    };

    Settings::update(move |s| {
        let Some(record) = s.accounts.plugins.iter_mut().find(|r| r.id == id) else {
            return;
        };

        for kept in record.synced.iter_mut() {
            if let Some(look) = seen.iter().find(|look| look.id == kept.id) {
                kept.title = look.title.clone();
                kept.subtitle = look.subtitle.clone();
                kept.kind = look.kind.clone();
                kept.art = look.art.clone();
            }
        }
    });
}

/// Syncs every collection the source has switched on. Answers the rows
/// written.
pub fn sync_now(
    library: Entity<Library>,
    source: &str,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    let host = match host_for(source) {
        Ok(host) => host,
        Err(e) => return Task::ready(Err(e)),
    };

    let source = source.to_string();
    let collections = synced_of(&source);

    // Every launch runs this once per plugin, mostly against unchanged
    // collections, so only a sync that wrote reloads.
    write_if(library, cx, move |db_path| {
        // Starting the plugin is part of the first sync, and the one
        // failure worth stopping on.
        if let Err(e) = host.ensure() {
            return (Err(e), false);
        }

        let (mut written, mut wrote) = (0, false);
        for collection in &collections {
            match sync_collection(&host, &db_path, &source, collection) {
                Ok(Some(rows)) => (written, wrote) = (written + rows, true),
                Ok(None) => {}
                Err(e) => log::warn!("{source}: syncing {} failed: {e}", collection.id),
            }
        }

        refresh_flags(&host, &db_path, &source);
        (Ok(written), wrote)
    })
}

/// Adds single tracks to the library and answers their keys, for a caller
/// that wants to queue them straight away.
pub fn pick(
    library: Entity<Library>,
    source: &str,
    tracks: Vec<PluginTrack>,
    cx: &mut App,
) -> Task<Result<Vec<TrackKey>, String>> {
    if running(source).is_none() {
        return Task::ready(Err(NO_HOST.to_string()));
    }

    let source = source.to_string();
    write(library, cx, move |db_path| {
        let mut conn = store::open(&db_path).map_err(|e| e.to_string())?;
        members::pick(&mut conn, &source, &tracks).map_err(|e| e.to_string())
    })
}

/// Add to Library: the tracks show in the library from now on, rather
/// than only playing (ADR 29).
pub fn save(
    library: Entity<Library>,
    source: &str,
    tracks: Vec<PluginTrack>,
    cx: &mut App,
) -> Task<Result<Vec<TrackKey>, String>> {
    let source = source.to_string();
    write(library, cx, move |db_path| {
        let mut conn = store::open(&db_path).map_err(|e| e.to_string())?;
        members::save(&mut conn, &source, &tracks).map_err(|e| e.to_string())
    })
}

/// Remove from Library: tracks added one at a time go back to being only
/// played, or go altogether if nothing else holds them.
pub fn unsave(
    library: Entity<Library>,
    source: &str,
    paths: Vec<String>,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    let source = source.to_string();
    write(library, cx, move |db_path| {
        let mut conn = store::open(&db_path).map_err(|e| e.to_string())?;
        members::unsave(&mut conn, &source, &paths).map_err(|e| e.to_string())
    })
}

/// Blocking; called from `fetch_cover`'s thread. A miss there is held for
/// minutes, so one asked before the first apply waits for it.
pub fn cover(source: &str, key: &str) -> Option<Vec<u8>> {
    await_first_apply(source);

    let host = host_for(source).ok()?;
    let answer = host
        .call("source.cover", json!({ "key": key }), host.timeouts().cover)
        .inspect_err(|e| log::debug!("{source}: cover for {key}: {e}"))
        .ok()?;

    if answer.is_null() {
        return None;
    }

    let cover: wire::Cover = wire::decode(answer).ok()?;
    cover.bytes().ok().filter(|bytes| !bytes.is_empty())
}

/// The manifest's half of the scrobble gate: false for a plugin that isn't
/// loaded, so nothing can scrobble on a record alone.
pub fn declares_scrobble(source: &str) -> bool {
    running(source).is_some_and(|running| {
        running
            .host
            .manifest()
            .capabilities
            .source
            .as_ref()
            .is_some_and(|cap| cap.scrobble)
    })
}

/// A running plugin as the Add Panel pickers list it.
#[derive(Clone, Debug)]
pub struct RunningPlugin {
    pub id: String,
    /// The manifest's display name.
    pub name: String,
    /// Its declared panels, past the External Sources panel every one gets.
    pub panels: Vec<rox_plugins::manifest::DeclaredPanel>,
}

/// Every running plugin, by display name. Running is switched on and
/// approved, so nothing lists for code nobody agreed to. Empty while
/// plugins are off.
pub fn running_plugins() -> Vec<RunningPlugin> {
    if !allowed() {
        return Vec::new();
    }
    let Ok(table) = HOSTS.read() else {
        return Vec::new();
    };

    let mut plugins: Vec<RunningPlugin> = table
        .iter()
        .filter_map(|(source, running)| {
            let manifest = running.host.manifest();
            Some(RunningPlugin {
                id: record_id(source)?.to_string(),
                name: manifest.name.clone(),
                panels: manifest.capabilities.panels.clone(),
            })
        })
        .collect();
    plugins.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));

    plugins
}

/// Whether `source` has a host that isn't stopped. Cheap enough to ask
/// while drawing.
pub fn answers(source: &str) -> bool {
    allowed()
        && running(source)
            .is_some_and(|running| !matches!(running.host.status(), Status::Stopped(_)))
}

/// Why `source` can't answer, or None when it has a host that's running.
/// Reads the settings file, so it's for after a call failed, not per frame.
pub fn unavailable(source: &str) -> Option<Unavailable> {
    if !allowed() {
        return Some(Unavailable::PluginsOff);
    }

    if let Some(running) = running(source) {
        return match running.host.status() {
            Status::Stopped(_) => Some(Unavailable::Stopped),
            _ => None,
        };
    }

    let id = record_id(source)?;
    let folders = loaded();
    let Some(folder) = folders.iter().find(|folder| folder.id == id) else {
        return Some(Unavailable::Missing);
    };
    if !folder.runs() {
        return Some(Unavailable::Failed);
    }

    let record = Settings::load()
        .accounts
        .plugins
        .into_iter()
        .find(|record| record.id == id);
    let approved_before = record.as_ref().is_some_and(|r| !r.hash.is_empty());
    if approved_before && !settings::plugin_approved(id, &folder.hash) {
        return Some(Unavailable::Changed);
    }

    Some(Unavailable::SwitchedOff)
}

/// Whether the plugin has a host right now: switched on, approved, loaded.
pub fn is_running(id: &str) -> bool {
    running(&source_of(id)).is_some()
}

/// `(source id, label)` for every plugin that's loaded and not stopped.
pub fn live_sources() -> Vec<(String, String)> {
    let Ok(table) = HOSTS.read() else {
        return Vec::new();
    };

    let mut live: Vec<(String, String)> = table
        .iter()
        .filter(|(_, running)| !matches!(running.host.status(), Status::Stopped(_)))
        .map(|(source, running)| (source.clone(), running.label.clone()))
        .collect();
    live.sort();

    live
}

/// `(source id, label)` for every running, not-stopped plugin with a source
/// capability, sorted like `live_sources`. The History panel lists these to
/// play an Unknown row from. Every source answers `source.search`.
pub fn searchable_sources() -> Vec<(String, String)> {
    if !allowed() {
        return Vec::new();
    }

    let mut sources = live_sources();
    // A panel-only plugin has a host too, with nothing to search.
    sources.retain(|(source, _)| offers(source, |_| true));

    sources
}

/// Searches `source` for the song and answers the key of the result that's
/// the same song, picked into the library (as a played row, not a saved one)
/// so it can be played at once. Ok(None) when nothing on the first page is
/// that song. Err on a plugin failure.
pub fn find_track(
    library: Entity<Library>,
    source: &str,
    artist: String,
    title: String,
    cx: &mut App,
) -> Task<Result<Option<TrackKey>, String>> {
    let source = source.to_string();
    let searched = search(&source, format!("{artist} {title}"), None, None, cx);

    cx.spawn(async move |cx| {
        let page = searched.await?;
        let mut tracks: Vec<PluginTrack> = page
            .entries
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Track(track) => Some(track),
                _ => None,
            })
            .collect();

        let Some(at) = same_song(&tracks, &artist, &title) else {
            return Ok(None);
        };

        let track = tracks.swap_remove(at);
        let picked = cx
            .update(|cx| pick(library, &source, vec![track], cx))
            .map_err(|e| e.to_string())?;

        Ok(picked.await?.into_iter().next())
    })
}

/// The position of the first result that's the song named, by the same rules
/// the Last.fm import matches with. No fallback to the top result: a cover or
/// a karaoke take played as the user's song is worse than nothing found.
fn same_song(tracks: &[PluginTrack], artist: &str, title: &str) -> Option<usize> {
    // The album artist is a second name, the way local tracks are filed: a
    // plugin can send the lead there when its artist is the whole credit list.
    let rows = tracks
        .iter()
        .enumerate()
        .flat_map(|(at, track)| {
            let album_artist = (!track.album_artist.is_empty()
                && track.album_artist != track.artist)
                .then(|| (at as i64, track.album_artist.clone(), track.title.clone()));
            std::iter::once((at as i64, track.artist.clone(), track.title.clone()))
                .chain(album_artist)
        })
        .collect();

    // The service ranked its results, so the earliest match wins a tie.
    let found = crate::names::Index::build(rows).resolve(artist, title);
    found.into_iter().min().map(|at| at as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(start_ms: u64, title: &str) -> wire::Chapter {
        wire::Chapter {
            start_ms,
            title: title.into(),
        }
    }

    fn plugin_row(key: &str, duration_ms: Option<u32>) -> Locator {
        Locator::Plugin(PluginStream {
            source: "plugin:example".into(),
            key: key.into(),
            live: false,
            duration_ms,
        })
    }

    fn keys(streams: &[PluginStream]) -> Vec<&str> {
        streams.iter().map(|s| s.key.as_str()).collect()
    }

    #[test]
    fn a_long_track_opens_whats_next_only_near_its_crossfade() {
        let held = Duration::from_secs(30);

        assert!(!preopen_due(held, Some(200.0), 6.0));
        assert!(preopen_due(held, Some(25.0), 6.0));
        assert!(
            !preopen_due(held, Some(25.0), 0.0),
            "no fade, a shorter lead"
        );
        assert!(
            !preopen_due(Duration::from_secs(1), Some(5.0), 6.0),
            "skipping through opens nothing"
        );
        assert!(preopen_due(held, None, 6.0), "no length, opens once held");
    }

    #[test]
    fn the_second_entry_opens_only_when_it_plays_before_the_sweep() {
        let short_next = vec![
            plugin_row("a", Some(20_000)),
            plugin_row("b", Some(200_000)),
        ];
        assert_eq!(keys(&worth_preopening(short_next, Some(15.0))), ["a", "b"]);

        let long_next = vec![
            plugin_row("a", Some(200_000)),
            plugin_row("b", Some(200_000)),
        ];
        assert_eq!(keys(&worth_preopening(long_next, Some(15.0))), ["a"]);

        let unknown_next = vec![plugin_row("a", None), plugin_row("b", None)];
        assert_eq!(keys(&worth_preopening(unknown_next, Some(15.0))), ["a"]);
    }

    #[test]
    fn chapters_come_sorted_with_the_blank_and_doubled_dropped() {
        let tidy = tidy_chapters(&[
            sent(271_000, " Story "),
            sent(0, "Intro"),
            sent(90_000, "  "),
            sent(271_000, "Again"),
        ]);

        assert_eq!(
            tidy,
            vec![
                Chapter {
                    start_secs: 0.0,
                    title: "Intro".into()
                },
                Chapter {
                    start_secs: 271.0,
                    title: "Story".into()
                },
            ]
        );
    }

    #[test]
    fn a_reopen_with_no_chapters_forgets_the_old_ones() {
        let (source, key) = ("plugin:test-chapters", "reopened");
        note_chapters(source, key, &[sent(0, "Intro")]);
        assert_eq!(chapters(source, key).len(), 1);

        note_chapters(source, key, &[]);
        assert!(chapters(source, key).is_empty());
        assert!(chapters(source, "never-opened").is_empty());
    }

    #[test]
    fn a_station_saves_where_it_got_to() {
        let station = Station {
            source: "plugin:example".into(),
            db_path: PathBuf::from("/nowhere/library.db"),
            at: Mutex::new(("t1".into(), Some("40".into()))),
            skip: Some("t1".into()),
        };

        let saved = continuation::Provider::saved(&station).expect("a station saves");
        let back: SavedStation = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            (
                back.source.as_str(),
                back.seed.as_str(),
                back.cursor.as_deref()
            ),
            ("plugin:example", "t1", Some("40"))
        );
        assert_eq!(back.skip.as_deref(), Some("t1"), "the seed stays skipped");

        let following = Station {
            at: Mutex::new((String::new(), None)),
            ..station
        };
        let saved = continuation::Provider::saved(&following).unwrap();
        let back: SavedStation = serde_json::from_str(&saved).unwrap();
        assert!(
            back.seed.is_empty(),
            "a follow-on station still seeds itself after a restart"
        );
    }

    #[test]
    fn the_gate_wakes_a_waiter_when_it_opens() {
        let gate = Arc::new(Gate::new());
        let opener = Arc::clone(&gate);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            opener.open();
        });

        assert!(gate.wait(Duration::from_secs(10)));
        assert!(gate.is_open());
        thread.join().unwrap();
    }

    #[test]
    fn a_gate_nobody_opens_gives_up_at_its_limit() {
        let gate = Gate::new();
        let began = Instant::now();

        assert!(!gate.wait(Duration::from_millis(50)));
        assert!(began.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn a_plugin_that_isnt_loaded_declares_nothing() {
        assert!(!declares_scrobble("plugin:never-loaded"));
        assert!(!declares_scrobble("subsonic:abc"));
    }

    fn manifest(caps: Value, programs: Value, entry: &str) -> Value {
        json!({
            "id": "tones",
            "entry": { "script": { "path": entry, "interpreter": "python3" } },
            "capabilities": caps,
            "programs": programs,
        })
    }

    #[test]
    fn an_unchanged_manifest_has_no_changes() {
        let doc = manifest(json!({ "source": { "label": "T" } }), json!(["a"]), "t.py");
        assert!(changes(&doc, &doc).is_empty());
    }

    #[test]
    fn the_diff_names_what_an_approval_would_grant() {
        let before = manifest(
            json!({ "source": { "label": "T" } }),
            json!(["a", "b"]),
            "t.py",
        );
        let after = manifest(
            json!({ "source": { "label": "T", "scrobble": true }, "panels": [] }),
            json!(["b", "c"]),
            "u.py",
        );

        assert_eq!(
            changes(&before, &after),
            vec![
                Change::CapabilityAdded("panels".into()),
                Change::ProgramAdded("c".into()),
                Change::Scrobble(true),
                Change::Entry,
            ]
        );

        let dropped = manifest(json!({}), json!(["b"]), "t.py");
        assert_eq!(
            changes(&before, &dropped),
            vec![Change::CapabilityRemoved("source".into())]
        );
    }

    #[test]
    fn features_and_actions_inside_a_source_show_too() {
        let export = json!({ "id": "export", "label": "Export", "on": ["track"] });
        let before = manifest(
            json!({ "source": { "label": "T", "radio": true, "actions": [export] } }),
            json!([]),
            "t.py",
        );

        let changed = json!({ "id": "export", "label": "Export", "on": ["track", "node"] });
        let download = json!({ "id": "download", "label": "Download", "on": ["track"] });
        let after = manifest(
            json!({ "source": { "label": "T", "radio": true, "links": true,
                                "actions": [changed, download] } }),
            json!([]),
            "t.py",
        );

        assert_eq!(
            changes(&before, &after),
            vec![
                Change::CapabilityAdded("links".into()),
                Change::ActionChanged("Export".into()),
                Change::ActionAdded("Download".into()),
            ]
        );
    }

    #[test]
    fn declaring_favourites_shows_as_a_new_capability() {
        let add = json!({ "id": "add", "label": "Add", "on": ["track"] });
        let drop = json!({ "id": "drop", "label": "Drop", "on": ["track"] });
        let before = manifest(
            json!({ "source": { "label": "T", "actions": [add.clone(), drop.clone()] } }),
            json!([]),
            "t.py",
        );
        let after = manifest(
            json!({ "source": { "label": "T", "actions": [add, drop],
                                "favourites": { "add": "add", "remove": "drop" } } }),
            json!([]),
            "t.py",
        );

        assert_eq!(
            changes(&before, &after),
            vec![Change::CapabilityAdded("favourites".into())]
        );
    }

    #[test]
    fn a_first_approval_diffs_against_nothing() {
        let doc = manifest(json!({ "source": { "label": "T" } }), json!([]), "t.py");
        assert_eq!(
            changes(&Value::Null, &doc),
            vec![Change::CapabilityAdded("source".into()), Change::Entry]
        );
    }

    fn results(found: &[(&str, &str)]) -> Vec<PluginTrack> {
        found
            .iter()
            .map(|&(artist, title)| PluginTrack {
                key: format!("{artist}/{title}"),
                artist: artist.into(),
                title: title.into(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn the_same_song_wins_over_a_result_ranked_above_it() {
        let found = results(&[
            ("Air", "Sexy Boy (Karaoke Version)"),
            ("Air", "Kelly Watch the Stars"),
            ("Air", "Sexy Boy"),
            ("Air", "Sexy Boy"),
        ]);
        assert_eq!(same_song(&found, "air", "Sexy Boy"), Some(2));
    }

    #[test]
    fn a_bracketed_qualifier_on_either_side_is_still_the_song() {
        let found = results(&[("Boards of Canada", "Olson (2013 Remaster)")]);
        assert_eq!(same_song(&found, "Boards of Canada", "Olson"), Some(0));

        let found = results(&[("Boards of Canada", "Olson")]);
        assert_eq!(
            same_song(&found, "Boards of Canada", "Olson (Remastered)"),
            Some(0)
        );
    }

    #[test]
    fn a_cover_by_another_artist_is_not_the_song() {
        let found = results(&[("Some Covers Band", "Roygbiv")]);
        assert_eq!(same_song(&found, "Boards of Canada", "Roygbiv"), None);
    }

    #[test]
    fn two_takes_that_differ_only_by_qualifier_settle_nothing() {
        let found = results(&[("Air", "Sexy Boy (Live)"), ("Air", "Sexy Boy (Demo)")]);
        assert_eq!(same_song(&found, "Air", "Sexy Boy"), None);
    }

    #[test]
    fn a_credit_list_or_an_album_artist_finds_the_lead() {
        let found = results(&[("Lemaitre, Sofiloud", "Trip Sitter")]);
        assert_eq!(same_song(&found, "Lemaitre", "Trip Sitter"), Some(0));

        let mut found = results(&[("Sofiloud & Lemaitre", "Trip Sitter")]);
        found[0].album_artist = "Lemaitre".into();
        assert_eq!(same_song(&found, "Lemaitre", "Trip Sitter"), Some(0));
    }

    #[test]
    fn no_results_find_nothing() {
        assert_eq!(same_song(&[], "Air", "Sexy Boy"), None);
    }
}
