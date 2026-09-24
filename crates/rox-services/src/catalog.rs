//! The shared library catalog: the `Library` entity over [`rox_library`].
//! It owns the database, hands out the in-memory projection, and drives
//! scanning, watching, and playlist mutations. UI-free.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, Context, Entity, EventEmitter, PathPromptOptions, SharedString, Task};

use rox_library::bookmarks::{self, Bookmark, BookmarkRow};
use rox_library::cue::TrackKey;
use rox_library::embeddings;
use rox_library::exclude::Exclusions;
use rox_library::listens;
use rox_library::locator::Locator;
use rox_library::playlists;
use rox_library::projection::{self, Builder, Patch, Projection, RowView};
use rox_library::rusqlite::{self, Connection};
use rox_library::scanner::{self, ScanSummary};
use rox_library::store;
use rox_library::watch::{LibraryWatcher, WatchBatch};
use rox_library::writer;

use crate::sources_registry;

pub enum LibraryEvent {
    Updated,
    /// One rating moved in place, so panels that rebuild on Updated can
    /// ignore a star click.
    Rated,
    /// One play count bumped in place: cells repaint, nothing rebuilds.
    Played,
    /// Counts moved for an unnamed set of tracks (a Last.fm backfill).
    /// Updated would reset the selection, cursor, and scroll.
    PlaysReloaded,
    PlaylistsChanged,
    BookmarksChanged,
}

/// Background job milestones. Separate from [`LibraryEvent`] so panels that
/// rebuild on catalog changes don't wake for these.
pub enum LibraryJob {
    /// The app hangs the taskbar sampler off this.
    ScanStarted,
    /// A scan over every folder ran to the end. A one-folder or stopped scan
    /// doesn't count.
    ScanFinished,
    /// The app follows this with the acoustic pass when the switch is on.
    WatchSettled,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Off the projection row: its rating and play count are the live atomics,
/// not whatever the last write put on disk.
fn meta_from_row(row: &RowView<'_>) -> store::TrackMeta {
    store::TrackMeta {
        title: row.title.to_string(),
        artist: row.artist.to_string(),
        album: row.album.to_string(),
        track_no: row.track_no,
        album_artist: row.album_artist.to_string(),
        year: row.year,
        genre: row.genre.to_string(),
        duration_ms: row.duration_ms,
        codec: row.codec.to_string(),
        bitrate_kbps: row.bitrate_kbps,
        sample_rate_hz: row.sample_rate_hz,
        bit_depth: row.bit_depth,
        rating: row.rating,
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct SortNames {
    pub title: String,
    pub artist: String,
    pub album: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCopy {
    pub track_id: i64,
    pub path: PathBuf,
}

/// The other half of [`Library::playlist_export_rows`]. A `path#N` entry is
/// a cue track unless the library holds a file by that literal name
/// ([`TrackKey::from_fragment`]). Relative names resolve against the sheet's
/// folder.
fn resolve_m3u_entry(conn: &Connection, base_dir: &Path, entry: &str) -> Option<i64> {
    let resolve = |name: &str| {
        let path = Path::new(name);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        }
    };
    let exists = |name: &str| {
        resolve(name)
            .to_str()
            .and_then(|full| {
                store::id_for_path(conn, rox_library::cue::LOCAL, full)
                    .ok()
                    .flatten()
            })
            .is_some()
    };
    let key = TrackKey::from_fragment(entry, exists);

    // Only a local entry joins onto the sheet's folder.
    let path = if key.is_local() {
        resolve(key.path.to_str()?).to_string_lossy().into_owned()
    } else {
        key.path.to_str()?.to_string()
    };

    store::queue_meta_for_key(conn, &key.source, &path, key.sub)
        .ok()?
        .id
}

const SCAN_POLL: Duration = Duration::from_millis(100);

/// Interim projection swaps while a scan runs. An empty library polls fast
/// so the first scan paints tracks right away.
const SCAN_REFRESH_EMPTY: Duration = Duration::from_secs(1);
const SCAN_REFRESH_FIRST: Duration = Duration::from_secs(15);
const SCAN_REFRESH_STEADY: Duration = Duration::from_secs(30);

/// Past this tombstoned fraction a sync rebuilds instead of patching:
/// patches append, and the arenas only grow between rebuilds.
const COMPACT_DEAD_FRACTION: f64 = 0.10;

/// A patch reads rows back one lookup at a time, cheap for a watch batch's
/// handful but not for a renamed root holding the whole library.
const PATCH_MAX_ROWS: usize = 5_000;

/// A restart within this of the last scan trusts the stored projection and
/// the watch, so a quick relaunch never re-walks the library.
const CATCH_UP_STALE: i64 = 24 * 60 * 60;

/// Only Linux prices watches per directory: inotify's per-user
/// `max_user_watches` budget is shared with every watching app, so the
/// library claims at most half. The half also covers intermediate folders
/// that [`store::Stats::dirs`] doesn't count. None elsewhere.
pub fn watch_limit_dirs() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    static LIMIT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    Some(*LIMIT.get_or_init(|| {
        std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            // The kernel's long-standing default when /proc is unreadable;
            // 5.11+ scales the real value with memory.
            .unwrap_or(65_536)
            / 2
    }))
}

/// Zero total means the folder walk hasn't finished.
#[derive(Default)]
struct ScanProgress {
    scanned: AtomicUsize,
    total: AtomicUsize,
    current: Mutex<String>,
    cancel: AtomicBool,
    /// Started on the first file, so the directory walk before it doesn't
    /// price the whole scan.
    pace: rox_core::pace::Pace,
    /// Atomic: the tick runs on the scanner's worker threads.
    timing: AtomicBool,
}

impl ScanProgress {
    fn tick(&self, scanned: usize, total: usize, path: &std::path::Path) -> bool {
        if !self.timing.swap(true, Ordering::Relaxed) {
            self.pace.begin();
        }
        self.scanned.store(scanned, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        *self.current.lock().unwrap() = path.to_string_lossy().into_owned();
        !self.cancel.load(Ordering::Relaxed)
    }
}

pub struct ScanStatus {
    pub done: usize,
    pub total: usize,
    pub current: String,
    pub eta: Option<f64>,
    pub stopping: bool,
}

enum Refresh {
    Load,
    Scan(Vec<PathBuf>),
    Remove(PathBuf),
    /// The tag editor's write-back: rows converge to what's on disk, not just
    /// the columns the edit named.
    Reindex(Vec<PathBuf>),
    /// The duplicates window's delete: the files are already trashed.
    Prune(Vec<PathBuf>),
    /// The watcher's per-change sync, proportional to what changed. The roots
    /// are passed so a prune or rename stays strictly inside them and never
    /// wipes a root that momentarily reads gone.
    Watch {
        paths: Vec<PathBuf>,
        renames: Vec<(PathBuf, PathBuf)>,
        roots: Vec<PathBuf>,
    },
}

#[derive(Default)]
struct WatchSummary {
    updated: usize,
    removed: usize,
    renamed: usize,
}

pub struct Library {
    db_path: PathBuf,
    /// UI-side lookups only; background work opens its own connections.
    conn: Option<Connection>,
    projection: Option<Arc<Projection>>,
    /// Bumped on every whole swap. A rebuild renumbers every row, so anything
    /// holding row indices across an event stamps them with this and checks
    /// before reading. A patch appends and tombstones, so it leaves this alone.
    projection_gen: u64,
    order: Arc<Vec<u32>>,
    /// Rebuilt on every projection swap.
    row_by_id: HashMap<i64, u32>,
    scan_roots: Vec<PathBuf>,
    /// Swapped whole on change, so a running scan keeps the list it started
    /// with.
    exclude: Arc<Exclusions>,
    busy: Option<SharedString>,
    /// Skips the next interim tick while one load is in flight: a big
    /// library's load outlasts the cadence, and racing loads only waste cores.
    interim_loading: bool,
    /// A load asked for while the library was busy, owed once it frees up.
    /// Dropping it would leave, say, a finished server sync out of every list.
    load_owed: bool,
    scan: Option<Arc<ScanProgress>>,
    /// Newest value per track; one drain writes them at a time.
    pending_ratings: HashMap<i64, u8>,
    rating_write_running: bool,
    status: SharedString,
    /// Apart from `watcher`, which is also None with no roots yet, so adding
    /// the first folder arms it.
    watch_on: bool,
    watcher: Option<LibraryWatcher>,
    watch_task: Option<Task<()>>,
    /// Deduped burst of paths, drained into one `Refresh::Watch` once no other
    /// refresh runs.
    pending: HashSet<PathBuf>,
    /// Kept apart: a rename pair keeps the row's id.
    pending_renames: Vec<(PathBuf, PathBuf)>,
    /// The app's own writes, filtered out of watch batches so they don't
    /// bounce back as a reindex.
    self_writes: HashMap<PathBuf, std::time::Instant>,
    /// The same for the app's own renames: the row already moved with the file.
    self_renames: HashMap<(PathBuf, PathBuf), std::time::Instant>,
}

impl EventEmitter<LibraryEvent> for Library {}
impl EventEmitter<LibraryJob> for Library {}

impl Library {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let db_path = rox_core::settings::data_dir().join("library.db");
        let (conn, status) =
            match store::open(&db_path).and_then(|conn| store::init_schema(&conn).map(|_| conn)) {
                Ok(conn) => (Some(conn), SharedString::default()),
                Err(e) => (None, SharedString::from(format!("library db: {e}"))),
            };
        // Favourites is the one default playlist, so the heart always has
        // somewhere to write.
        if let Some(conn) = &conn {
            let _ = playlists::ensure_favourites(conn, now_secs());
            // Older builds could leave double rows in favourites.
            let _ = playlists::dedupe_favourites(conn, now_secs());
            // Before the first load, so merged genres match from the first paint.
            if let Ok(aliases) = rox_library::genre_meta::aliases(conn) {
                rox_library::genre::set_aliases(aliases);
            }
        }

        // Apply add_root's never-nests rule to the loaded list too.
        let loaded = rox_core::settings::Settings::load().library_roots;
        let before = loaded.len();
        let mut scan_roots: Vec<PathBuf> = Vec::with_capacity(before);
        for root in loaded {
            if scan_roots.iter().any(|r| root.starts_with(r)) {
                continue;
            }
            scan_roots.retain(|r| !r.starts_with(&root));
            scan_roots.push(root);
        }
        if scan_roots.len() != before {
            let roots = scan_roots.clone();
            rox_core::settings::Settings::update(move |s| s.library_roots = roots);
        }

        // A library indexed before roots were persisted: use the deepest
        // shared directory for this session.
        if scan_roots.is_empty()
            && let Some(root) = conn
                .as_ref()
                .and_then(|conn| store::common_root(conn).ok().flatten())
        {
            scan_roots.push(root);
        }

        let mut this = Library {
            db_path,
            conn,
            projection: None,
            projection_gen: 0,
            order: Arc::new(Vec::new()),
            row_by_id: HashMap::new(),
            scan_roots,
            exclude: Arc::new(Exclusions::new(
                &rox_core::settings::Settings::load().library_exclude,
            )),
            busy: None,
            interim_loading: false,
            load_owed: false,
            scan: None,
            pending_ratings: HashMap::new(),
            rating_write_running: false,
            status,
            watch_on: rox_core::settings::Settings::load().watch_library,
            watcher: None,
            watch_task: None,
            pending: HashSet::new(),
            pending_renames: Vec::new(),
            self_writes: HashMap::new(),
            self_renames: HashMap::new(),
        };
        // The watch only sees changes while the app runs, so a stale last
        // scan opens on a catch-up scan instead of a plain load.
        let last_scan = rox_core::settings::Settings::load().session.last_scan;
        let stale = now_secs().saturating_sub(last_scan) > CATCH_UP_STALE;
        let catch_up = this.watch_on && !this.scan_roots.is_empty() && stale;
        if this.conn.is_some() {
            if catch_up {
                this.reload(Refresh::Scan(this.scan_roots.clone()), cx);
            } else {
                this.reload(Refresh::Load, cx);
            }
        }
        if this.watch_on {
            this.arm_watch(cx);
        }
        this
    }

    pub fn projection(&self) -> Option<&Arc<Projection>> {
        self.projection.as_ref()
    }

    pub fn order(&self) -> Arc<Vec<u32>> {
        self.order.clone()
    }

    /// Anything caching row indices checks this before use: a change means the
    /// rows were renumbered.
    pub fn projection_gen(&self) -> u64 {
        self.projection_gen
    }

    /// The only place `projection` and `order` change wholesale, so the index
    /// and [`Library::projection_gen`] move here and nowhere else. The index
    /// arrives prebuilt by [`load_projection`]: a million hash inserts don't
    /// belong on the UI thread.
    fn swap_projection(
        &mut self,
        projection: Projection,
        order: Vec<u32>,
        row_by_id: HashMap<i64, u32>,
    ) {
        self.row_by_id = row_by_id;
        self.projection = Some(Arc::new(projection));
        self.order = Arc::new(order);
        self.projection_gen = self.projection_gen.wrapping_add(1);
    }

    /// The tombstone-and-append path. False means the caller owes a full
    /// reload: someone else holds a clone of the projection (a tag editor
    /// does while open), or it refused the patch at the arena ceiling.
    fn apply_patch(
        &mut self,
        shard: Builder,
        gone: &[i64],
        plays: &HashMap<i64, u32>,
        spans: &HashMap<i64, rox_library::cue::Span>,
    ) -> bool {
        let Some(shared) = self.projection.take() else {
            return false;
        };
        let mut projection = match Arc::try_unwrap(shared) {
            Ok(projection) => projection,
            Err(shared) => {
                // Logged with a count because the answer should be nobody:
                // every holder costs the incremental sync.
                log::debug!(
                    "catalog: {} other holders of the projection, rebuilding instead of patching",
                    Arc::strong_count(&shared) - 1
                );
                self.projection = Some(shared);
                return false;
            }
        };
        let Some(mut patch) = projection.apply_upserts(shard, &self.row_by_id, plays, spans) else {
            log::warn!("catalog: the projection refused a patch, rebuilding instead");
            self.projection = Some(Arc::new(projection));
            return false;
        };
        // One patch for both halves, so the order is walked once.
        let removed = projection.remove_ids(gone, &self.row_by_id);
        patch.dropped.extend(removed.dropped);
        patch.gone = removed.gone;
        self.install_patch(projection, patch);
        true
    }

    /// Leaves [`Library::projection_gen`] alone: a patch renumbers nothing.
    fn install_patch(&mut self, projection: Projection, patch: Patch) {
        if !patch.is_empty() {
            // A patch that moved a value the projection already knew (a new
            // sort name) reordered rows it never saw, so it needs a full sort.
            self.order = Arc::new(if patch.reordered {
                projection.sort_canonical()
            } else {
                projection.patch_order(&self.order, &patch)
            });
            for &row in &patch.added {
                self.row_by_id.insert(projection.db_id[row as usize], row);
            }
            for id in &patch.gone {
                self.row_by_id.remove(id);
            }
        }
        self.projection = Some(Arc::new(projection));
    }

    pub fn busy(&self) -> Option<SharedString> {
        self.busy.clone()
    }

    pub fn status(&self) -> SharedString {
        self.status.clone()
    }

    pub fn can_rescan(&self) -> bool {
        !self.scan_roots.is_empty()
    }

    /// Only scans can be aborted.
    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    pub fn scan_status(&self) -> Option<ScanStatus> {
        let scan = self.scan.as_ref()?;
        let done = scan.scanned.load(Ordering::Relaxed);
        let total = scan.total.load(Ordering::Relaxed);
        Some(ScanStatus {
            done,
            total,
            current: scan.current.lock().unwrap().clone(),
            eta: scan.pace.eta_secs(done, total),
            stopping: scan.cancel.load(Ordering::Relaxed),
        })
    }

    /// What it already indexed stays; the reload still follows.
    pub fn abort_scan(&mut self, cx: &mut Context<Self>) {
        let Some(scan) = &self.scan else {
            return;
        };
        scan.cancel.store(true, Ordering::Relaxed);
        self.busy = Some("stopping...".into());
        cx.notify();
    }

    /// While another job holds the library, the load is owed rather than
    /// dropped: the database may have moved after the running load read it.
    pub fn reload_projection(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            self.load_owed = true;
            return;
        }

        self.load_owed = false;
        self.reload(Refresh::Load, cx);
    }

    fn settle_owed_load(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.load_owed) {
            self.reload(Refresh::Load, cx);
        }
    }

    /// Merges are an opinion in the genre_meta table; the files keep their tags.
    pub fn merge_genres(&mut self, sources: &[String], target: &str, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else {
            return;
        };
        for source in sources {
            if let Err(e) = rox_library::genre_meta::set_alias(conn, source, target) {
                self.status = format!("genre merge: {e}").into();
            }
        }
        self.refresh_genre_aliases(cx);
    }

    pub fn unmerge_genre(&mut self, target: &str, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else {
            return;
        };
        if let Err(e) = rox_library::genre_meta::clear_aliases_into(conn, target) {
            self.status = format!("genre unmerge: {e}").into();
        }
        self.refresh_genre_aliases(cx);
    }

    pub fn genre_aliases_into(&self, target: &str) -> Vec<String> {
        self.conn
            .as_ref()
            .and_then(|conn| rox_library::genre_meta::aliases_into(conn, target).ok())
            .unwrap_or_default()
    }

    fn refresh_genre_aliases(&mut self, cx: &mut Context<Self>) {
        if let Some(conn) = &self.conn
            && let Ok(aliases) = rox_library::genre_meta::aliases(conn)
        {
            rox_library::genre::set_aliases(aliases);
        }
        self.reload_projection(cx);
    }

    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.scan_roots.is_empty() {
            return;
        }
        self.reload(Refresh::Scan(self.scan_roots.clone()), cx);
    }

    pub fn root_stats(&self) -> Vec<(PathBuf, store::Stats)> {
        self.scan_roots
            .iter()
            .map(|root| {
                let stats = self
                    .conn
                    .as_ref()
                    .and_then(|conn| store::stats_under(conn, root).ok())
                    .unwrap_or_default();
                (root.clone(), stats)
            })
            .collect()
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        self.scan_roots.clone()
    }

    pub fn exclusions(&self) -> Arc<Exclusions> {
        self.exclude.clone()
    }

    /// Nothing is rescanned: the watcher picks the list up on its next batch,
    /// and existing rows answer to it on the next Rescan.
    pub fn set_exclusions(&mut self, patterns: Vec<String>) {
        self.exclude = Arc::new(Exclusions::new(&patterns));
        rox_core::settings::Settings::update(move |s| s.library_exclude = patterns);
    }

    pub fn stats(&self) -> store::Stats {
        self.conn
            .as_ref()
            .and_then(|conn| store::stats(conn).ok())
            .unwrap_or_default()
    }

    pub fn replaygain_breakdown(&self) -> store::GainCoverage {
        self.conn
            .as_ref()
            .and_then(|conn| store::replaygain_breakdown(conn).ok())
            .unwrap_or_default()
    }

    pub fn bpm_breakdown(&self) -> store::BpmCoverage {
        self.conn
            .as_ref()
            .and_then(|conn| store::bpm_breakdown(conn).ok())
            .unwrap_or_default()
    }

    /// The switch only permits vectors; until a pass has run there's nothing
    /// to rank by.
    pub fn analyzed(&self, model: &str) -> bool {
        self.conn
            .as_ref()
            .and_then(|conn| embeddings::any(conn, model).ok())
            .unwrap_or(false)
    }

    pub fn acoustic_coverage(&self, model: &str) -> embeddings::Coverage {
        self.conn
            .as_ref()
            .and_then(|conn| embeddings::coverage(conn, model).ok())
            .unwrap_or_default()
    }

    /// On the UI connection: a tenth of a second on a big library, on a page
    /// somebody just opened.
    pub fn storage_breakdown(&self) -> store::Storage {
        self.conn
            .as_ref()
            .and_then(|conn| store::storage_breakdown(conn).ok())
            .unwrap_or_default()
    }

    /// Includes models other than this build's, whose rows a renamed extractor
    /// leaves behind.
    pub fn embedding_models(&self) -> Vec<embeddings::ModelRows> {
        self.conn
            .as_ref()
            .and_then(|conn| embeddings::models(conn).ok())
            .unwrap_or_default()
    }

    /// The VACUUM rewrites the whole file, so this runs on its own connection
    /// and holds the busy badge like a rescan. It can't gate the analysis
    /// pass, which opens the library by path: the caller must refuse while a
    /// pass runs. No reload needed; the projection holds no vectors.
    pub fn clear_embeddings(&mut self, model: &str, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("clearing vectors...".into());
        let db_path = self.db_path.clone();
        let model = model.to_owned();
        cx.spawn(async move |this, cx| {
            let dropped = cx
                .background_executor()
                .spawn(async move {
                    let conn = store::open(&db_path)?;
                    embeddings::clear(&conn, &model)
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = None;
                this.settle_owed_load(cx);
                this.status = match dropped {
                    Ok(n) => format!("cleared {n} vectors").into(),
                    Err(e) => format!("library: {e}").into(),
                };
                let described = this.analyzed(crate::acoustic::acoustic_source().id());
                rox_core::settings::set_acoustic_described(described, cx);
                cx.emit(LibraryEvent::Updated);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Forget every tempo rox measured, keeping tagged ones, so an improved
    /// estimator gets to re-measure. Unlike [`Library::clear_embeddings`] it
    /// reloads the projection, which holds the BPM column. The caller must
    /// refuse while the tempo pass runs.
    pub fn clear_measured_bpm(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("clearing tempos...".into());
        let db_path = self.db_path.clone();
        cx.spawn(async move |this, cx| {
            let cleared = cx
                .background_executor()
                .spawn(async move {
                    let conn = store::open(&db_path)?;
                    store::clear_measured_bpm(&conn)
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = None;
                this.status = match cleared {
                    Ok(n) => format!("forgot {n} measured tempos").into(),
                    Err(e) => format!("library: {e}").into(),
                };
                this.reload_projection(cx);
                cx.emit(LibraryEvent::Updated);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The one path that removes a listen event, behind the caller's confirm.
    /// The Last.fm import bounds go too, or the next import would fetch
    /// nothing past a history that no longer exists.
    pub fn clear_listens(&mut self, what: listens::Clear, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("clearing listens...".into());
        let db_path = self.db_path.clone();
        cx.spawn(async move |this, cx| {
            let cleared = cx
                .background_executor()
                .spawn(async move {
                    let conn = store::open(&db_path)?;
                    listens::clear(&conn, what)
                })
                .await;
            rox_core::settings::Settings::update(|s| s.accounts.lastfm.forget_imports());
            this.update(cx, |this, cx| {
                this.busy = None;
                this.settle_owed_load(cx);
                this.status = match cleared {
                    Ok(n) => format!("cleared {n} listens").into(),
                    Err(e) => format!("library: {e}").into(),
                };
                this.reload_plays(cx);
                cx.emit(LibraryEvent::Updated);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn db_path(&self) -> PathBuf {
        self.db_path.clone()
    }

    /// No reload needed: the projection holds no gain columns this changes.
    pub fn note_gain_written(&mut self, cx: &mut Context<Self>) {
        cx.emit(LibraryEvent::Updated);
        cx.notify();
    }

    /// Note the writes, then reindex, like [`Library::apply_edits`] minus the
    /// optimistic column patch.
    pub fn reindex_written(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            self.note_gain_written(cx);
            return;
        }
        self.note_self_write(paths.iter().cloned());
        self.reload(Refresh::Reindex(paths), cx);
    }

    /// The list never nests: a covered folder is rescanned, not added, and one
    /// that covers listed folders absorbs them. A no-op while a scan runs.
    pub fn add_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        if !self.scan_roots.iter().any(|r| root.starts_with(r)) {
            self.scan_roots.retain(|r| !r.starts_with(&root));
            self.scan_roots.push(root.clone());
            self.persist_roots();
            self.rearm_watch(cx);
        }
        self.reload(Refresh::Scan(vec![root]), cx);
    }

    /// The files are untouched. A no-op while a scan runs.
    pub fn remove_root(&mut self, root: &Path, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(ix) = self.scan_roots.iter().position(|r| r == root) else {
            return;
        };
        self.scan_roots.remove(ix);
        self.persist_roots();
        self.rearm_watch(cx);
        self.reload(Refresh::Remove(root.to_path_buf()), cx);
    }

    fn persist_roots(&self) {
        let roots = self.scan_roots.clone();
        rox_core::settings::Settings::update(move |s| s.library_roots = roots);
    }

    pub fn watching(&self) -> bool {
        self.watcher.is_some()
    }

    /// Past [`watch_limit_dirs`] the watcher doesn't arm and the toggle grays
    /// out.
    pub fn watch_limited(&self) -> bool {
        watch_limit_dirs().is_some_and(|limit| self.stats().dirs > limit)
    }

    /// Covered roots versus asked-for. Nothing in the settings UI reads this
    /// yet.
    pub fn watch_coverage(&self) -> Option<(usize, usize)> {
        self.watcher.as_ref().map(|w| w.coverage())
    }

    /// Call before every file write the app initiates, so the watch batch it
    /// triggers is dropped.
    pub fn note_self_write<I>(&mut self, paths: I)
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let now = std::time::Instant::now();
        for path in paths {
            self.self_writes.insert(path, now);
        }
    }

    /// The echo would be harmless (the second `rename_within` finds nothing),
    /// but renames arrive by the hundred and each echo reloads the whole
    /// projection. Both endpoints go into `self_writes` too, for when the
    /// watcher can't correlate the pair.
    pub fn note_self_rename<I>(&mut self, moves: I)
    where
        I: IntoIterator<Item = (PathBuf, PathBuf)>,
    {
        let now = std::time::Instant::now();
        for (from, to) in moves {
            self.self_writes.insert(from.clone(), now);
            self.self_writes.insert(to.clone(), now);
            self.self_renames.insert((from, to), now);
        }
    }

    pub fn set_watch(&mut self, on: bool, cx: &mut Context<Self>) {
        self.watch_on = on;
        rox_core::settings::Settings::update(move |s| s.watch_library = on);
        if on {
            self.arm_watch(cx);
        } else {
            self.watcher = None;
            self.watch_task = None;
            self.pending.clear();
            self.pending_renames.clear();
        }
        cx.notify();
    }

    /// Replaces any live watcher. Arming walks the whole tree adding one OS
    /// watch per directory, so the build runs off the UI thread; dropping the
    /// prior task cancels an in-flight build.
    fn arm_watch(&mut self, cx: &mut Context<Self>) {
        self.watcher = None;
        self.watch_task = None;
        if self.scan_roots.is_empty() {
            return;
        }
        let roots = self.scan_roots.clone();
        let db_path = self.db_path.clone();
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            // The ceiling check is a full table scan, so it rides the same
            // background hop. A library past the ceiling never arms; the
            // preference stays, so dropping back under lets it watch again.
            let Some(watcher) = cx
                .background_executor()
                .spawn(async move {
                    if let Some(limit) = watch_limit_dirs() {
                        let dirs = store::open(&db_path)
                            .and_then(|conn| store::stats(&conn))
                            .map(|stats| stats.dirs)
                            .unwrap_or(0);
                        if dirs > limit {
                            return None;
                        }
                    }
                    LibraryWatcher::new(&roots)
                })
                .await
            else {
                return;
            };
            let events = watcher.events();
            // A toggle-off or a newer re-arm that raced this build wins.
            let stored = this.update(cx, |this, _| {
                if !this.watch_on {
                    return false;
                }
                this.watcher = Some(watcher);
                true
            });
            if !matches!(stored, Ok(true)) {
                return;
            }
            while let Ok(batch) = events.recv().await {
                if this
                    .update(cx, |this, cx| this.note_changes(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// Only while watching is on, so a folder change never re-enables it.
    fn rearm_watch(&mut self, cx: &mut Context<Self>) {
        if self.watch_on {
            self.arm_watch(cx);
        }
    }

    /// Cheap: no disk touched; sorting the batch happens in the sync.
    fn note_changes(&mut self, batch: WatchBatch, cx: &mut Context<Self>) {
        // A few seconds past the 1s debounce covers the write, flush, and
        // deliver round trip. A missed suppression only costs one reindex, so
        // the window errs short rather than eat a real edit.
        let now = std::time::Instant::now();
        let window = std::time::Duration::from_secs(5);
        self.self_writes
            .retain(|_, at| now.duration_since(*at) < window);
        self.self_renames
            .retain(|_, at| now.duration_since(*at) < window);
        self.pending.extend(
            batch
                .paths
                .into_iter()
                .filter(|p| !self.self_writes.contains_key(p)),
        );
        self.pending_renames
            .extend(batch.renames.into_iter().filter(|pair| {
                !self
                    .self_renames
                    .contains_key(&(pair.0.clone(), pair.1.clone()))
            }));
        self.pump_watch(cx);
    }

    /// Re-run after every reload, which picks up changes that arrived
    /// mid-refresh.
    fn pump_watch(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() || (self.pending.is_empty() && self.pending_renames.is_empty()) {
            return;
        }
        let paths: Vec<PathBuf> = self.pending.drain().collect();
        let renames = std::mem::take(&mut self.pending_renames);
        let roots = self.scan_roots.clone();
        self.reload(
            Refresh::Watch {
                paths,
                renames,
                roots,
            },
            cx,
        );
    }

    pub fn paths_for(&self, ids: &[i64]) -> Result<Vec<PathBuf>, String> {
        let Some(conn) = &self.conn else {
            return Ok(Vec::new());
        };
        store::paths_for(conn, ids)
            .map(|paths| paths.into_iter().map(Into::into).collect())
            .map_err(|e| e.to_string())
    }

    /// What the Last.fm mirror names a track by. Untaggable ids drop out.
    pub fn names_for(&self, ids: &[i64]) -> Vec<(String, String)> {
        self.conn
            .as_ref()
            .and_then(|conn| store::names_for(conn, ids).ok())
            .unwrap_or_default()
    }

    /// The local file behind each named song, for surfaces holding a song's
    /// names with no file (every radio listen). One projection walk for the
    /// whole list.
    pub fn local_copies(&self, names: &[(&str, &str)]) -> Vec<Option<LocalCopy>> {
        let Some(projection) = self.projection() else {
            return vec![None; names.len()];
        };

        let rows = projection.find_locals(names);
        let ids: Vec<i64> = rows
            .iter()
            .flatten()
            .map(|&row| projection.db_id[row as usize])
            .collect();
        let paths = self
            .conn
            .as_ref()
            .and_then(|conn| store::paths_by_id(conn, &ids).ok())
            .unwrap_or_default();

        rows.into_iter()
            .map(|row| {
                let track_id = projection.db_id[row? as usize];
                Some(LocalCopy {
                    track_id,
                    path: paths.get(&track_id)?.into(),
                })
            })
            .collect()
    }

    /// Anything that plays what it resolved uses this, not
    /// [`paths_for`](Self::paths_for): a cue track's key points at its own span.
    pub fn keys_for(&self, ids: &[i64]) -> Result<Vec<TrackKey>, String> {
        let Some(conn) = &self.conn else {
            return Ok(Vec::new());
        };
        let mut keys = Vec::with_capacity(ids.len());
        for &id in ids {
            // One query per id: a dropped id would misalign a batch.
            let row = store::key_for_id(conn, id).map_err(|e| e.to_string())?;
            if let Some(row) = row {
                keys.push(TrackKey {
                    source: row.source,
                    path: row.path,
                    sub: self.sub_for_id(id),
                });
            }
        }
        Ok(keys)
    }

    /// A remote locator is finished here off the registry, so the engine
    /// never asks a source anything.
    pub fn locators_for(&self, ids: &[i64]) -> Result<Vec<Locator>, String> {
        let Some(conn) = &self.conn else {
            return Ok(Vec::new());
        };

        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            // One id at a time, for [`keys_for`](Self::keys_for)'s reason.
            let Some(mut locator) = store::locators_for(conn, &[id])
                .map_err(|e| e.to_string())?
                .pop()
            else {
                continue;
            };

            if let Locator::Remote(remote) = &mut locator {
                let source = store::key_for_id(conn, id)
                    .ok()
                    .flatten()
                    .map(|key| key.source)
                    .unwrap_or_else(rox_library::cue::local);

                sources_registry::authorize(&source, remote);
            }

            out.push(locator);
        }

        Ok(out)
    }

    /// Zero when the projection has no row, the safe answer mid-scan.
    pub fn sub_for_id(&self, id: i64) -> u16 {
        let (Some(projection), Some(&row)) = (&self.projection, self.row_by_id.get(&id)) else {
            return 0;
        };
        projection.sub.get(row as usize).copied().unwrap_or(0)
    }

    /// What file actions (editors, rename, convert, reveal) take: a server
    /// song or a station has no file. An id with no projection row yet stays
    /// in.
    pub fn local_ids(&self, ids: &[i64]) -> Vec<i64> {
        let Some(projection) = &self.projection else {
            return ids.to_vec();
        };

        ids.iter()
            .copied()
            .filter(|id| {
                let Some(&row) = self.row_by_id.get(id) else {
                    return true;
                };

                projection.source.get(row as usize).is_none_or(|&sym| {
                    projection.sources.strings[sym as usize] == rox_library::cue::LOCAL
                })
            })
            .collect()
    }

    /// Off the projection, so the readings are the interned ones. All empty
    /// for an id with no row.
    pub fn sort_names_for_id(&self, id: i64) -> SortNames {
        let (Some(projection), Some(&row)) = (&self.projection, self.row_by_id.get(&id)) else {
            return SortNames::default();
        };
        if projection.db_id.get(row as usize) != Some(&id) {
            return SortNames::default();
        }
        let v = projection.resolve(row);
        SortNames {
            title: v.title_sort.to_string(),
            artist: v.artist_sort.to_string(),
            album: v.album_sort.to_string(),
        }
    }

    pub fn meta_for_key(&self, key: &TrackKey) -> Option<store::TrackMeta> {
        self.resolve_key(key).map(|(_, meta)| meta)
    }

    pub fn id_for_key(&self, key: &TrackKey) -> Option<i64> {
        let conn = self.conn.as_ref()?;
        store::queue_meta_for_key(conn, &key.source, key.path.to_str()?, key.sub)
            .ok()?
            .id
    }

    /// Keyed on (source, path, sub): a path-only lookup returns whichever cue
    /// track sorts first. Tags come off the projection row, falling back to
    /// the store when there's none.
    pub fn resolve_key(&self, key: &TrackKey) -> Option<(i64, store::TrackMeta)> {
        let conn = self.conn.as_ref()?;
        let path = key.path.to_str()?;
        let id = store::queue_meta_for_key(conn, &key.source, path, key.sub)
            .ok()?
            .id;
        if let Some(id) = id
            && let (Some(projection), Some(&row)) = (&self.projection, self.row_by_id.get(&id))
        {
            // Guard against a projection swapped between paint and lookup.
            if projection.db_id.get(row as usize) == Some(&id) {
                return Some((id, meta_from_row(&projection.resolve(row))));
            }
        }
        // Only a sub 0 key resolves from the store, which can't tell cue
        // tracks apart.
        (key.sub == 0)
            .then(|| {
                store::meta_row_for_path(conn, &key.source, path)
                    .ok()
                    .flatten()
            })
            .flatten()
    }

    /// Panel-open and listen-append cadence only, per ADR 11.
    pub fn recent_listens(&self, since: i64, until: i64, limit: usize) -> Vec<listens::TrackPlays> {
        self.listen_query(|conn| listens::recent(conn, since, until, limit))
    }

    pub fn listen_summary(&self, id: i64, since: i64) -> Option<listens::TrackSummary> {
        self.conn
            .as_ref()
            .and_then(|conn| listens::track_summary(conn, id, since).ok().flatten())
    }

    pub fn last_played(&self) -> HashMap<i64, i64> {
        self.conn
            .as_ref()
            .and_then(|conn| listens::last_played(conn).ok())
            .unwrap_or_default()
    }

    pub fn most_played(&self, limit: usize) -> Vec<listens::TrackPlays> {
        self.listen_query(|conn| listens::most_played(conn, limit))
    }

    pub fn never_played(
        &self,
        order: listens::NeverOrder,
        descending: bool,
        limit: usize,
    ) -> Vec<listens::TrackPlays> {
        self.listen_query(|conn| listens::never_played(conn, order, descending, limit))
    }

    /// `since` 0 counts every event.
    pub fn listen_rollup(
        &self,
        by: listens::Rollup,
        since: i64,
        until: i64,
        limit: usize,
    ) -> Vec<listens::NamePlays> {
        let fold = rox_core::settings::fold_case();
        self.listen_query(|conn| listens::rollup(conn, by, since, until, limit, fold))
    }

    pub fn listens_since(&self, since: i64) -> u64 {
        self.listens_between(since, i64::MAX)
    }

    pub fn listens_between(&self, since: i64, until: i64) -> u64 {
        self.conn
            .as_ref()
            .and_then(|conn| listens::count_between(conn, since, until).ok())
            .unwrap_or_default()
    }

    pub fn listens_tally(&self) -> listens::Tally {
        self.conn
            .as_ref()
            .and_then(|conn| listens::tally(conn).ok())
            .unwrap_or_default()
    }

    pub fn first_listen(&self) -> Option<i64> {
        self.conn
            .as_ref()
            .and_then(|conn| listens::earliest(conn).ok())
            .flatten()
    }

    pub fn listen_histogram(&self, since: i64, bucket: i64, end: i64, until: i64) -> Vec<u64> {
        self.listen_query(|conn| listens::histogram(conn, since, bucket, end, until))
    }

    pub fn ids_for_rollup(&self, by: listens::Rollup, name: &str, limit: usize) -> Vec<i64> {
        let fold = rox_core::settings::fold_case();
        self.listen_query(|conn| listens::ids_for_name(conn, by, name, limit, fold))
    }

    fn listen_query<T>(
        &self,
        query: impl FnOnce(&Connection) -> rusqlite::Result<Vec<T>>,
    ) -> Vec<T> {
        self.conn
            .as_ref()
            .and_then(|conn| query(conn).ok())
            .unwrap_or_default()
    }

    pub fn playlists(&self) -> Vec<playlists::Playlist> {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::list(conn).ok())
            .unwrap_or_default()
    }

    pub fn playlist_tracks(&self, id: i64) -> Vec<playlists::PlaylistTrack> {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::tracks(conn, id).ok())
            .unwrap_or_default()
    }

    /// A smart playlist materializes here, so play, export, and continuation
    /// all take one route.
    pub fn playlist_ids(&self, id: i64) -> Vec<i64> {
        if let Some(def) = self.playlist_definition(id) {
            return self.smart_ids(&def);
        }
        self.conn
            .as_ref()
            .and_then(|conn| playlists::ids(conn, id).ok())
            .unwrap_or_default()
    }

    /// Paths are [`TrackKey`] fragments, so a cue rip exports as `disc.flac#N`
    /// lines. Without a projection this falls back to the store's path-only
    /// rows, wrong only for cue tracks.
    pub fn playlist_export_rows(&self, id: i64) -> Vec<playlists::ExportTrack> {
        let Some(conn) = &self.conn else {
            return Vec::new();
        };
        let Some(projection) = &self.projection else {
            // Without a projection, only static members can be written.
            return playlists::export_rows(conn, id).unwrap_or_default();
        };
        // An export of a smart playlist writes what it holds right now,
        // the same materialization the panel is showing. The definition
        // itself doesn't travel; an M3U has nowhere to put it.
        let ids = self.playlist_ids(id);
        ids.iter()
            .filter_map(|&track_id| {
                let &row = self.row_by_id.get(&track_id)?;
                if projection.db_id.get(row as usize) != Some(&track_id) {
                    return None;
                }
                let view = projection.resolve(row);
                let path = store::paths_for(conn, &[track_id]).ok()?.pop()?;
                let key = TrackKey {
                    source: rox_library::cue::source_id(view.source),
                    path: PathBuf::from(path),
                    sub: view.sub,
                };
                Some(playlists::ExportTrack {
                    path: key.to_fragment(),
                    title: view.title.to_string(),
                    artist: view.artist.to_string(),
                    // Nearest second, the resolution #EXTINF uses.
                    duration_secs: (view.duration_ms as i64 + 500) / 1000,
                })
            })
            .collect()
    }

    /// One playlist's saved query, None for a static playlist.
    pub fn playlist_definition(&self, id: i64) -> Option<playlists::SmartDef> {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::definition(conn, id).ok())
            .flatten()
    }

    /// Evaluate a smart playlist against the loaded projection: the track
    /// ids its query, filter, and sort name, capped by its limit. Nothing
    /// to evaluate against before the first projection loads, so that
    /// reads as an empty list.
    pub fn smart_ids(&self, def: &playlists::SmartDef) -> Vec<i64> {
        match &self.projection {
            Some(projection) => def.ids(projection, self.order.clone()),
            None => Vec::new(),
        }
    }

    /// A smart definition's projection rows, [`Library::smart_ids`] one
    /// step earlier. The editor's preview reads these: it resolves the
    /// handful of rows on screen straight off the projection, so a query
    /// that takes the whole library costs one pass and nothing per row.
    pub fn smart_rows(&self, def: &playlists::SmartDef) -> Vec<u32> {
        match &self.projection {
            Some(projection) => def.rows(projection, self.order.clone()),
            None => Vec::new(),
        }
    }

    /// A smart playlist's tracks as the display rows the panel draws,
    /// resolved off the projection rather than member rows. Only the path
    /// comes from the store, for the cover cell.
    pub fn smart_tracks(&self, def: &playlists::SmartDef) -> Vec<playlists::PlaylistTrack> {
        let Some(projection) = &self.projection else {
            return Vec::new();
        };
        let ids = self.smart_ids(def);
        // One read for the whole list rather than one per row: a smart
        // playlist can be the size of the library, and this runs on every
        // panel refresh.
        let paths = self
            .conn
            .as_ref()
            .and_then(|conn| store::paths_by_id(conn, &ids).ok())
            .unwrap_or_default();
        ids.iter()
            .filter_map(|&track_id| {
                let &row = self.row_by_id.get(&track_id)?;
                if projection.db_id.get(row as usize) != Some(&track_id) {
                    return None;
                }
                let view = projection.resolve(row);
                let path = paths.get(&track_id).cloned().unwrap_or_default();
                Some(playlists::PlaylistTrack {
                    // A smart playlist has no member rows, so there is no
                    // rowid to address one by. The panel keys its own
                    // selection off the pair instead; nothing that edits
                    // members ever touches these rows.
                    member_id: 0,
                    track_id,
                    title: view.title.to_string(),
                    artist: view.artist.to_string(),
                    album: view.album.to_string(),
                    album_artist: view.album_artist.to_string(),
                    year: view.year,
                    genre: view.genre.to_string(),
                    duration_ms: view.duration_ms,
                    codec: view.codec.to_string(),
                    bitrate_kbps: view.bitrate_kbps,
                    sample_rate_hz: view.sample_rate_hz,
                    bit_depth: view.bit_depth,
                    rating: view.rating,
                    path,
                    source: view.source.to_string(),
                })
            })
            .collect()
    }

    pub fn favourite_ids(&self) -> HashSet<i64> {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::favourite_track_ids(conn).ok())
            .map(|ids| ids.into_iter().collect())
            .unwrap_or_default()
    }

    pub fn is_favourite(&self, track_id: i64) -> bool {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::is_favourite(conn, track_id).ok())
            .unwrap_or(false)
    }

    /// One event for the whole batch.
    pub fn set_favourites(&mut self, track_ids: &[i64], on: bool, cx: &mut Context<Self>) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let now = now_secs();
        let mut changed = false;
        for &id in track_ids {
            if playlists::set_favourite(conn, id, on, now).is_ok() {
                changed = true;
            }
        }
        if changed {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    /// Empty for a key the library doesn't hold: a bookmark needs a row.
    pub fn bookmarks_for(&self, key: &TrackKey) -> Vec<Bookmark> {
        let Some(conn) = &self.conn else {
            return Vec::new();
        };
        let Some(id) = self.id_for_key(key) else {
            return Vec::new();
        };
        bookmarks::for_track(conn, id).unwrap_or_default()
    }

    pub fn all_bookmarks(&self) -> Vec<BookmarkRow> {
        let Some(conn) = &self.conn else {
            return Vec::new();
        };
        bookmarks::all(conn).unwrap_or_default()
    }

    pub fn bookmark(&self, id: i64) -> Option<Bookmark> {
        let conn = self.conn.as_ref()?;
        bookmarks::get(conn, id).ok().flatten()
    }

    /// `color` is `#rrggbb`; None follows the theme accent.
    pub fn add_bookmark(
        &mut self,
        key: &TrackKey,
        position_ms: u32,
        name: &str,
        color: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Option<i64> {
        let track_id = self.id_for_key(key)?;
        let conn = self.conn.as_ref()?;
        let id =
            bookmarks::add(conn, track_id, &key.to_fragment(), position_ms, name, color).ok()?;
        cx.emit(LibraryEvent::BookmarksChanged);
        Some(id)
    }

    pub fn rename_bookmark(&mut self, id: i64, name: &str, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else { return };
        if bookmarks::rename(conn, id, name).is_ok() {
            cx.emit(LibraryEvent::BookmarksChanged);
        }
    }

    pub fn set_bookmark_color(&mut self, id: i64, color: Option<&str>, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else { return };
        if bookmarks::set_color(conn, id, color).is_ok() {
            cx.emit(LibraryEvent::BookmarksChanged);
        }
    }

    pub fn move_bookmark(&mut self, id: i64, position_ms: u32, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else { return };
        if bookmarks::set_position(conn, id, position_ms).is_ok() {
            cx.emit(LibraryEvent::BookmarksChanged);
        }
    }

    pub fn bookmark_count_for(&self, track_ids: &[i64]) -> u64 {
        let Some(conn) = &self.conn else { return 0 };
        bookmarks::count_for_tracks(conn, track_ids).unwrap_or(0)
    }

    pub fn remove_track_bookmarks(&mut self, track_ids: &[i64], cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else { return };
        if matches!(bookmarks::remove_for_tracks(conn, track_ids), Ok(n) if n > 0) {
            cx.emit(LibraryEvent::BookmarksChanged);
        }
    }

    pub fn remove_bookmark(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else { return };
        if bookmarks::remove(conn, id).is_ok() {
            cx.emit(LibraryEvent::BookmarksChanged);
        }
    }

    pub fn create_playlist(&mut self, name: &str, cx: &mut Context<Self>) -> Option<i64> {
        let conn = self.conn.as_ref()?;
        let id = playlists::create(conn, name, now_secs()).ok()?;
        cx.emit(LibraryEvent::PlaylistsChanged);
        Some(id)
    }

    pub fn create_smart_playlist(
        &mut self,
        name: &str,
        def: &playlists::SmartDef,
        cx: &mut Context<Self>,
    ) -> Option<i64> {
        let conn = self.conn.as_ref()?;
        let id = playlists::create_smart(conn, name, def, now_secs()).ok()?;
        cx.emit(LibraryEvent::PlaylistsChanged);
        Some(id)
    }

    pub fn set_playlist_definition(
        &mut self,
        id: i64,
        def: &playlists::SmartDef,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = &self.conn else { return };
        if playlists::set_definition(conn, id, def, now_secs()).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    pub fn rename_playlist(&mut self, id: i64, name: &str, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else { return };
        if playlists::rename(conn, id, name, now_secs()).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    pub fn delete_playlist(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        if playlists::delete(conn, id).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    pub fn add_to_playlist(&mut self, id: i64, track_ids: &[i64], cx: &mut Context<Self>) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        if playlists::add(conn, id, track_ids, now_secs()).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    /// `before` is a member id, or None for the end.
    pub fn add_to_playlist_at(
        &mut self,
        id: i64,
        track_ids: &[i64],
        before: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };

        let Ok(members) = playlists::add(conn, id, track_ids, now_secs()) else {
            return;
        };
        if members.is_empty() {
            return;
        }

        if before.is_some() {
            let _ = playlists::place_members(conn, id, &members, before, now_secs());
        }

        cx.emit(LibraryEvent::PlaylistsChanged);
    }

    /// The one call behind every playlist drag: single or multi, reorder or
    /// cross-playlist move.
    pub fn place_playlist_members(
        &mut self,
        playlist_id: i64,
        members: &[i64],
        before: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        if playlists::place_members(conn, playlist_id, members, before, now_secs()).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    pub fn remove_playlist_members(&mut self, member_ids: &[i64], cx: &mut Context<Self>) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        if playlists::remove_members(conn, member_ids, now_secs()).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    /// Relative paths resolve against `base_dir`; entries the library never
    /// scanned fall away.
    pub fn import_playlist(
        &mut self,
        name: &str,
        base_dir: &Path,
        entries: &[String],
        cx: &mut Context<Self>,
    ) -> Option<i64> {
        let conn = self.conn.as_mut()?;
        let ids: Vec<i64> = entries
            .iter()
            .filter_map(|entry| resolve_m3u_entry(conn, base_dir, entry))
            .collect();
        let id = playlists::create(conn, name, now_secs()).ok()?;
        if !ids.is_empty() {
            playlists::add(conn, id, &ids, now_secs()).ok()?;
        }
        cx.emit(LibraryEvent::PlaylistsChanged);
        Some(id)
    }

    /// The named columns are written first, so the edit shows even if a busy
    /// library drops the reload; then the file is reindexed so duration and
    /// codec converge too.
    pub fn apply_edit(
        &mut self,
        key: &TrackKey,
        changes: &[writer::Change],
        cx: &mut Context<Self>,
    ) {
        self.note_self_write([key.path.clone()]);
        if let Some((id, conn)) = self.id_for_key(key).zip(self.conn.as_ref())
            && let Err(e) = store::apply_changes(conn, id, changes)
        {
            self.status = format!("library: {e}").into();
            cx.notify();
        }
        self.reload(Refresh::Reindex(vec![key.path.clone()]), cx);
    }

    /// The tag editor's save. `subs` runs parallel to `edits` (padding with
    /// 0): an edit names a file, and one cue image is many tracks.
    pub fn apply_edits(&mut self, edits: &[writer::Edit], subs: &[u16], cx: &mut Context<Self>) {
        for (i, edit) in edits.iter().enumerate() {
            let key = TrackKey {
                source: rox_library::cue::local(),
                path: edit.path.clone(),
                sub: subs.get(i).copied().unwrap_or(0),
            };
            let Some(id) = self.id_for_key(&key) else {
                continue;
            };
            let Some(conn) = &self.conn else { return };
            if let Err(e) = store::apply_changes(conn, id, &edit.changes) {
                self.status = format!("library: {e}").into();
                cx.notify();
            }
        }
        let paths: Vec<PathBuf> = edits.iter().map(|edit| edit.path.clone()).collect();
        self.note_self_write(paths.iter().cloned());
        self.reload(Refresh::Reindex(paths), cx);
    }

    /// Deliberately not noted as self-writes: if this reload is dropped, the
    /// watcher still prunes the same rows.
    pub fn remove_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        self.reload(Refresh::Prune(paths), cx);
    }

    /// Ids stay put through [`store::rename_within`], so ratings and joins stay
    /// attached. The reattach passes refresh the path snapshots playlist
    /// members and listens keep for when a row is pruned; the watch path never
    /// runs them.
    pub fn rename_files(&mut self, moves: Vec<(PathBuf, PathBuf)>, cx: &mut Context<Self>) {
        if moves.is_empty() {
            return;
        }
        let mut failure = None;
        if let Some(conn) = self.conn.as_mut() {
            for (from, to) in &moves {
                if let Err(e) = store::rename_within(conn, from, to) {
                    failure = Some(format!("library: {e}"));
                    break;
                }
            }
            if failure.is_none()
                && let Err(e) = playlists::reattach(conn).and_then(|_| listens::reattach(conn))
            {
                failure = Some(format!("library: {e}"));
            }
        }
        if let Some(e) = failure {
            self.status = e.into();
            cx.notify();
        }
        let paths: Vec<PathBuf> = moves.into_iter().map(|(_, to)| to).collect();
        self.reload(Refresh::Reindex(paths), cx);
    }

    /// Written to the row and into the projection's rating atomics in place,
    /// so it needs no reload and works mid-scan. The `db_id` guard catches a
    /// projection swapped between paint and click. The file's tags follow
    /// through the write queue.
    pub fn rate(&mut self, id: i64, rating: u8, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else { return };
        if let Err(e) = store::set_rating(conn, id, rating) {
            self.status = format!("library: {e}").into();
            cx.notify();
            return;
        }
        if let (Some(projection), Some(&row)) = (&self.projection, self.row_by_id.get(&id))
            && projection.db_id.get(row as usize) == Some(&id)
        {
            projection.rating[row as usize].store(rating, Ordering::Relaxed);
        }
        self.queue_rating_write(id, rating, cx);
        cx.emit(LibraryEvent::Rated);
        cx.notify();
    }

    /// The event row is already on disk (ADR 11); this only bumps the cached
    /// column.
    pub fn record_play(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(projection) = &self.projection else {
            return;
        };
        // Through the id map: after a patch the id also sits on a tombstoned
        // row.
        if let Some(&row) = self.row_by_id.get(&id) {
            projection.plays[row as usize].fetch_add(1, Ordering::Relaxed);
            cx.emit(LibraryEvent::Played);
        }
    }

    /// After an external import (a Last.fm backfill). Raises
    /// [`LibraryEvent::PlaysReloaded`], since the counts moved for tracks
    /// nobody named.
    pub fn reload_plays(&mut self, cx: &mut Context<Self>) {
        let (Some(projection), Some(conn)) = (&self.projection, &self.conn) else {
            return;
        };
        let Ok(counts) = rox_library::listens::counts(conn) else {
            return;
        };
        for (id, &row) in &self.row_by_id {
            let count = counts.get(id).copied().unwrap_or(0);
            projection.plays[row as usize].store(count, Ordering::Relaxed);
        }
        cx.emit(LibraryEvent::PlaysReloaded);
    }

    pub fn plays_for(&self, ids: &[i64]) -> HashMap<i64, u32> {
        let Some(projection) = &self.projection else {
            return HashMap::new();
        };
        ids.iter()
            .filter_map(|&id| {
                let row = *self.row_by_id.get(&id)? as usize;
                Some((id, projection.plays[row].load(Ordering::Relaxed)))
            })
            .collect()
    }

    /// Panels caching ratings re-read here on `Rated` instead of rebuilding.
    pub fn ratings_for(&self, ids: &[i64]) -> HashMap<i64, u8> {
        let Some(projection) = &self.projection else {
            return HashMap::new();
        };
        ids.iter()
            .filter_map(|&id| {
                let row = *self.row_by_id.get(&id)? as usize;
                Some((id, projection.rating[row].load(Ordering::Relaxed)))
            })
            .collect()
    }

    /// Newest value per track, one drain at a time, so rapid clicks don't race
    /// the writer's clone-and-rename on one file.
    fn queue_rating_write(&mut self, id: i64, rating: u8, cx: &mut Context<Self>) {
        self.pending_ratings.insert(id, rating);
        if self.rating_write_running {
            return;
        }
        self.rating_write_running = true;
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |this, _| {
                    let id = this.pending_ratings.keys().next().copied();
                    id.map(|id| (id, this.pending_ratings.remove(&id).unwrap()))
                });
                let Ok(Some((id, rating))) = next else { break };
                let Ok(Some(key)) = this.update(cx, |this, _| {
                    let key = this.keys_for(&[id]).ok().and_then(|mut keys| keys.pop());
                    if let Some(key) = &key {
                        this.note_self_write([key.path.clone()]);
                    }
                    key
                }) else {
                    continue;
                };
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let change = writer::Change {
                            field: writer::Field::Rating,
                            value: (rating > 0).then(|| rox_library::rating::display(rating)),
                        };
                        // Through the key: stamping a cue image's file would
                        // rate every track of the disc.
                        writer::commit_key(&key.path, key.sub, &[change], &[])
                            .map_err(|e| (key.path, e))
                    })
                    .await;
                if let Err((path, e)) = result {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    this.update(cx, |this, cx| {
                        this.status = format!("rating: {name}: {e}").into();
                        cx.notify();
                    })
                    .ok();
                }
            }
            this.update(cx, |this, _| this.rating_write_running = false)
                .ok();
        })
        .detach();
    }

    /// One refresh at a time: while one runs, another is dropped here, so two
    /// never race on the database or the badge.
    fn reload(&mut self, refresh: Refresh, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(match &refresh {
            Refresh::Load => "loading library...".into(),
            Refresh::Scan(_) => "scanning...".into(),
            Refresh::Remove(_) => "removing...".into(),
            Refresh::Reindex(_) => "refreshing...".into(),
            Refresh::Prune(_) => "removing...".into(),
            Refresh::Watch { .. } => "syncing...".into(),
        });
        let progress = Arc::new(ScanProgress::default());
        if matches!(refresh, Refresh::Scan(_)) {
            self.scan = Some(progress.clone());
            self.poll_scan(progress.clone(), cx);
            // The scan is the one job the tasks window's ticker doesn't see.
            cx.emit(LibraryJob::ScanStarted);
            self.refresh_during_scan(cx);
        }
        // A failed watch sync re-queues its drained batch.
        let retry = match &refresh {
            Refresh::Watch { paths, renames, .. } => Some((paths.clone(), renames.clone())),
            _ => None,
        };
        // Only a full scan stamps the catch-up clock.
        let was_scan = matches!(refresh, Refresh::Scan(_));
        let full_scan = matches!(&refresh, Refresh::Scan(roots) if *roots == self.scan_roots);
        let was_watch = matches!(refresh, Refresh::Watch { .. });
        // Only a sync may patch; past the dead-weight ceiling it rebuilds,
        // which is where compaction happens.
        let patch = matches!(refresh, Refresh::Watch { .. } | Refresh::Reindex(_))
            .then_some(self.projection.as_ref())
            .flatten()
            .filter(|p| !p.is_empty() && p.dead_fraction() < COMPACT_DEAD_FRACTION)
            .map(|p| p.fold);
        let db_path = self.db_path.clone();
        let exclude = self.exclude.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load(&db_path, refresh, &exclude, &progress, patch) })
                .await;
            this.update(cx, |this, cx| {
                this.busy = None;
                this.scan = None;
                let ok = result.is_ok();
                // A refused patch leaves the library where it was, so the
                // reload is owed.
                let mut owed = false;
                match result {
                    Ok((loaded, summary, watch)) => {
                        match loaded {
                            Loaded::Full {
                                projection,
                                order,
                                row_by_id,
                            } => {
                                this.status = status_line(
                                    projection.browse_len(),
                                    summary.as_ref(),
                                    watch.as_ref(),
                                )
                                .into();
                                this.swap_projection(*projection, order, row_by_id);
                            }
                            Loaded::Patch {
                                shard,
                                gone,
                                plays,
                                spans,
                            } => {
                                owed = !this.apply_patch(*shard, &gone, &plays, &spans);
                                let total = this.projection.as_ref().map_or(0, |p| p.browse_len());
                                this.status =
                                    status_line(total, summary.as_ref(), watch.as_ref()).into();
                            }
                        }
                        // An aborted walk never finished, so it doesn't stamp
                        // last_scan.
                        let aborted = summary.as_ref().is_some_and(|s| s.aborted);
                        if was_scan && !aborted {
                            let now = now_secs();
                            rox_core::settings::Settings::update(move |s| {
                                s.session.last_scan = now
                            });
                        }
                        if full_scan && !aborted {
                            cx.emit(LibraryJob::ScanFinished);
                        }
                    }
                    Err(e) => {
                        this.status = format!("library: {e}").into();
                        if let Some((paths, renames)) = retry {
                            this.pending.extend(paths);
                            this.pending_renames.extend(renames);
                        }
                    }
                }
                // A refresh is when "anything to rank by sound" can change.
                let described = this.analyzed(crate::acoustic::acoustic_source().id());
                rox_core::settings::set_acoustic_described(described, cx);
                cx.emit(LibraryEvent::Updated);
                cx.notify();
                // An owed rebuild goes first. On error, hold the batch rather
                // than re-pump it into a busy loop against whatever is failing.
                if owed {
                    this.reload(Refresh::Load, cx);
                }
                this.settle_owed_load(cx);
                if ok {
                    this.pump_watch(cx);
                }
                // Only after a watch sync, once the pump has nothing left, so a
                // burst of arrivals is analyzed once. A full scan is an import,
                // where a library's worth of decoding should be asked for. The
                // pass is idempotent and no-ops while off or already running.
                if ok && was_watch && this.busy.is_none() {
                    cx.emit(LibraryJob::WatchSettled);
                }
            })
            .ok();
        })
        .detach();
        cx.emit(LibraryEvent::Updated);
        cx.notify();
    }

    /// Stops itself once the reload clears `busy`; panels see no event.
    fn poll_scan(&self, progress: Arc<ScanProgress>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SCAN_POLL).await;
                let live = this.update(cx, |this, cx| {
                    if this.busy.is_none() {
                        return false;
                    }
                    let total = progress.total.load(Ordering::Relaxed);
                    // A pending stop owns the badge.
                    if total > 0 && !progress.cancel.load(Ordering::Relaxed) {
                        let scanned = progress.scanned.load(Ordering::Relaxed);
                        this.busy = Some(format!("scanning {scanned}/{total}").into());
                        this.status = progress.current.lock().unwrap().clone().into();
                        cx.notify();
                    }
                    true
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The store is WAL, so a reader sees each committed batch. The final
    /// reload swaps the authoritative result.
    fn refresh_during_scan(&self, cx: &mut Context<Self>) {
        let db_path = self.db_path.clone();
        let mut delay = if self.projection.as_ref().is_none_or(|p| p.is_empty()) {
            SCAN_REFRESH_EMPTY
        } else {
            SCAN_REFRESH_FIRST
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(delay).await;
                if !matches!(this.read_with(cx, |this, _| this.scan.is_some()), Ok(true)) {
                    break;
                }
                let claimed = this.update(cx, |this, _| {
                    if this.interim_loading {
                        return false;
                    }
                    this.interim_loading = true;
                    true
                });
                match claimed {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => break,
                }
                let db_path = db_path.clone();
                let loaded = cx
                    .background_executor()
                    .spawn(async move { load_projection(&db_path) })
                    .await;
                let Ok((projection, order, row_by_id)) = loaded else {
                    this.update(cx, |this, _| this.interim_loading = false).ok();
                    continue;
                };
                if projection.is_empty() && delay == SCAN_REFRESH_EMPTY {
                    this.update(cx, |this, _| this.interim_loading = false).ok();
                    continue;
                }
                delay = if delay == SCAN_REFRESH_EMPTY {
                    SCAN_REFRESH_FIRST
                } else {
                    SCAN_REFRESH_STEADY
                };
                let live = this.update(cx, |this, cx| {
                    this.interim_loading = false;
                    // The final reload's swap is newer; keep it.
                    if this.scan.is_none() {
                        return false;
                    }
                    this.swap_projection(projection, order, row_by_id);
                    cx.emit(LibraryEvent::Updated);
                    cx.notify();
                    true
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Patch or full load is decided before the refresh runs (see
/// [`Library::reload`]): only the UI thread knows whether the projection can
/// be patched.
enum Loaded {
    Full {
        /// Boxed, or every field added to `Projection` trips clippy's
        /// large-variant lint here.
        projection: Box<Projection>,
        order: Vec<u32>,
        row_by_id: HashMap<i64, u32>,
    },
    Patch {
        /// Boxed for the same reason.
        shard: Box<Builder>,
        gone: Vec<i64>,
        /// The two columns a row can't read off its own `tracks` row.
        plays: HashMap<i64, u32>,
        spans: HashMap<i64, rox_library::cue::Span>,
    },
}

/// Ids, not paths: only the database can translate, while the connection is
/// open.
#[derive(Default)]
struct Touched {
    /// An id whose row is gone on re-read joins `removed`, which is how a cue
    /// track a re-cut sheet dropped leaves the projection.
    changed: Vec<i64>,
    removed: Vec<i64>,
}

#[allow(clippy::type_complexity)]
fn load(
    db_path: &std::path::Path,
    refresh: Refresh,
    exclude: &Exclusions,
    progress: &ScanProgress,
    patch: Option<bool>,
) -> Result<(Loaded, Option<ScanSummary>, Option<WatchSummary>), rox_library::rusqlite::Error> {
    let mut watch = None;
    let mut touched = Touched::default();
    let summary = match refresh {
        Refresh::Load => None,
        Refresh::Scan(roots) => {
            let mut conn = store::open(db_path)?;
            store::init_schema(&conn)?;
            let mut summary = ScanSummary::default();
            let mut done = 0;
            for root in roots {
                // Ticked from worker threads, so atomics, not &mut.
                let root_total = AtomicUsize::new(0);
                let s = scanner::scan(&mut conn, &root, exclude, |scanned, total, path| {
                    root_total.store(total, Ordering::Relaxed);
                    progress.tick(done + scanned, done + total, path)
                })?;
                done += root_total.load(Ordering::Relaxed);
                summary.indexed += s.indexed;
                summary.unchanged += s.unchanged;
                summary.untagged += s.untagged;
                summary.removed += s.removed;
                if s.aborted {
                    summary.aborted = true;
                    break;
                }
            }
            Some(summary)
        }
        Refresh::Remove(root) => {
            let conn = store::open(db_path)?;
            store::init_schema(&conn)?;
            store::remove_under(&conn, &root)?;
            None
        }
        Refresh::Reindex(paths) => {
            let mut conn = store::open(db_path)?;
            store::init_schema(&conn)?;
            // Ids before and after: a row the reindex dropped only shows in
            // the difference.
            touched.changed = store::ids_for_paths(&conn, &paths)?;
            touched.changed.extend(cue_neighbours(&conn, &paths)?);
            scanner::reindex(&mut conn, &paths)?;
            touched.changed.extend(store::ids_for_paths(&conn, &paths)?);
            touched.changed.extend(cue_neighbours(&conn, &paths)?);
            None
        }
        Refresh::Prune(paths) => {
            let conn = store::open(db_path)?;
            store::init_schema(&conn)?;
            for path in &paths {
                store::remove_subtree(&conn, path)?;
            }
            None
        }
        Refresh::Watch {
            paths,
            renames,
            roots,
        } => {
            let mut conn = store::open(db_path)?;
            store::init_schema(&conn)?;
            watch = Some(watch_sync(
                &mut conn,
                paths,
                renames,
                &roots,
                exclude,
                &mut touched,
            )?);
            None
        }
    };
    if let Some(fold) = patch {
        let conn = store::open(db_path)?;
        let removed: HashSet<i64> = touched.removed.iter().copied().collect();
        let mut changed: Vec<i64> = touched
            .changed
            .iter()
            .copied()
            .filter(|id| !removed.contains(id))
            .collect();
        changed.sort_unstable();
        changed.dedup();
        if changed.len() + removed.len() <= PATCH_MAX_ROWS {
            // The same shard builder the full load fills, so both read a row
            // alike.
            let shard = projection::shard_for_ids(&conn, &changed, fold)?;
            let present: HashSet<i64> = shard.ids().iter().copied().collect();
            let mut gone = touched.removed;
            gone.extend(changed.into_iter().filter(|id| !present.contains(id)));
            let plays = store::plays_for_ids(&conn, shard.ids())?;
            let spans = store::cue_spans_for_ids(&conn, shard.ids())?;
            return Ok((
                Loaded::Patch {
                    shard: Box::new(shard),
                    gone,
                    plays,
                    spans,
                },
                summary,
                watch,
            ));
        }
    }
    let (projection, order, row_by_id) = load_projection(db_path)?;
    Ok((
        Loaded::Full {
            projection: Box::new(projection),
            order,
            row_by_id,
        },
        summary,
        watch,
    ))
}

/// Every row in a directory where one of these paths is a cue sheet. A new
/// sheet retires the image's plain sub-0 row, whose id is in neither the
/// before nor the after list, so ask the directory instead: coarse but
/// cheap.
fn cue_neighbours(conn: &Connection, paths: &[PathBuf]) -> Result<Vec<i64>, rusqlite::Error> {
    let mut dirs: Vec<&Path> = paths
        .iter()
        .filter(|path| scanner::is_cue(path))
        .filter_map(|path| path.parent())
        .collect();
    dirs.sort_unstable();
    dirs.dedup();
    let mut out = Vec::new();
    for dir in dirs {
        out.extend(store::ids_under(conn, dir)?);
    }
    Ok(out)
}

/// A vanished sheet only cut images in its own directory, so no recursive
/// walk.
fn siblings(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| scanner::is_audio(path))
        .collect()
}

/// Renames first: moving the row keeps its id, the `to` path then re-reads,
/// and the `from` prune finds nothing. Present paths reindex, gone ones
/// prune their subtree. Renames and prunes only touch endpoints strictly
/// inside a root, so a root that momentarily reads gone never wipes the
/// library. Excluded paths prune whether or not they're on disk. Blocking.
fn watch_sync(
    conn: &mut Connection,
    mut paths: Vec<PathBuf>,
    renames: Vec<(PathBuf, PathBuf)>,
    roots: &[PathBuf],
    exclude: &Exclusions,
    touched: &mut Touched,
) -> Result<WatchSummary, rox_library::rusqlite::Error> {
    // The list never nests, so at most one root.
    let root_of = |path: &Path| {
        roots
            .iter()
            .find(|root| path.starts_with(root) && path != root.as_path())
    };
    let under_root = |path: &Path| root_of(path).is_some();
    // An event names the file, not the excluded folder above it.
    let excluded = |path: &Path| root_of(path).is_some_and(|root| exclude.covers(root, path));

    let mut summary = WatchSummary::default();
    // A rename crossing a root boundary falls through to a plain create and
    // delete.
    for (from, to) in renames {
        if under_root(&from) && under_root(&to) {
            // A rename across a pattern (Album to Album.bak under `*.bak`)
            // is rows leaving or arriving, not a move.
            if excluded(&from) || excluded(&to) {
                paths.push(from);
                paths.push(to);
                continue;
            }

            // Read the ids before the move; they're changes, not removals.
            touched.changed.extend(store::ids_under(conn, &from)?);
            summary.renamed += store::rename_within(conn, &from, &to)?;
        }
    }
    let mut changed = Vec::new();
    let mut removed = Vec::new();
    for path in paths {
        if excluded(&path) {
            removed.push(path);
            continue;
        }

        if path.exists() {
            if path.is_dir() {
                // A dir moved in arrives as one event, so walk it, skipping
                // junk folders like .Trashes as the full scan does.
                if let Some(root) = root_of(&path)
                    && !scanner::is_junk_dir(&path)
                {
                    changed.extend(scanner::audio_files_in(root, &path, exclude));
                }
            } else if scanner::is_relevant(&path) {
                // Sheets too: a .cue edit re-cuts the image beside it.
                changed.push(path);
            }
        } else if under_root(&path) {
            if scanner::is_cue(&path) {
                // A vanished sheet has no row to prune: reindex its images so
                // they go back to one plain row each.
                changed.extend(siblings(&path).into_iter().filter(|p| !excluded(p)));
            } else {
                removed.push(path);
            }
        }
    }
    if !changed.is_empty() {
        touched
            .changed
            .extend(store::ids_for_paths(conn, &changed)?);
        touched.changed.extend(cue_neighbours(conn, &changed)?);
        summary.updated += scanner::reindex(conn, &changed)?;
        touched
            .changed
            .extend(store::ids_for_paths(conn, &changed)?);
        touched.changed.extend(cue_neighbours(conn, &changed)?);
    }
    for path in &removed {
        // Before the delete: afterwards there is nothing left to ask.
        touched.removed.extend(store::ids_under(conn, path)?);
        summary.removed += store::remove_subtree(conn, path)?;
    }
    Ok(summary)
}

/// Builds the id index here, off the UI thread (see
/// [`Library::swap_projection`]). Blocking.
#[allow(clippy::type_complexity)]
fn load_projection(
    db_path: &std::path::Path,
) -> Result<(Projection, Vec<u32>, HashMap<i64, u32>), rox_library::rusqlite::Error> {
    let shards = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let mut projection =
        Projection::load_parallel(db_path, shards, rox_core::settings::fold_case())?;
    // Before the order, which is built off the browse mask this rewrites.
    crate::sources::publish_labels();
    projection.hide_sources(crate::sources::hidden_sources());
    let order = projection.sort_canonical();
    let row_by_id = projection
        .db_id
        .iter()
        .enumerate()
        .map(|(row, &id)| (id, row as u32))
        .collect();
    Ok((projection, order, row_by_id))
}

fn status_line(
    total: usize,
    summary: Option<&ScanSummary>,
    watch: Option<&WatchSummary>,
) -> String {
    // Zero counts stay out, so the line fits the menubar.
    let mut parts = Vec::new();
    let count = |n: usize| rox_i18n::format::format_int(n as i64);
    if let Some(s) = summary {
        if s.indexed > 0 {
            parts.push(format!("{} indexed", count(s.indexed)));
        }
        if s.unchanged > 0 {
            parts.push(format!("{} unchanged", count(s.unchanged)));
        }
        if s.untagged > 0 {
            parts.push(format!("{} untagged", count(s.untagged)));
        }
        if s.removed > 0 {
            parts.push(format!("{} removed", count(s.removed)));
        }
        if s.aborted {
            parts.push("stopped early".into());
        }
    }
    if let Some(w) = watch {
        if w.updated > 0 {
            parts.push(format!("{} updated", count(w.updated)));
        }
        if w.removed > 0 {
            parts.push(format!("{} removed", count(w.removed)));
        }
        if w.renamed > 0 {
            parts.push(format!("{} renamed", count(w.renamed)));
        }
    }
    let tracks = rox_i18n::t!("status-count-tracks", count = total as u64);
    if parts.is_empty() {
        return tracks.to_string();
    }
    format!("{tracks} ({})", parts.join(", "))
}

pub fn browse(library: &Entity<Library>, cx: &mut App) {
    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    let library = library.clone();
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(mut paths))) = rx.await
            && let Some(root) = paths.pop()
        {
            library
                .update(cx, |library, cx| library.add_root(root, cx))
                .ok();
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::{Touched, resolve_m3u_entry, watch_sync};
    use rox_library::exclude::Exclusions;
    use rox_library::store;

    #[test]
    fn watch_sync_reindexes_present_and_prunes_gone() {
        let dir = std::env::temp_dir().join("rox-watch-sync");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Album")).unwrap();
        let roots = vec![dir.clone()];

        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();

        // Dummy bytes index under the filename, enough to make a row.
        let one = dir.join("Album/1.mp3");
        let two = dir.join("Album/2.mp3");
        std::fs::write(&one, b"not audio").unwrap();
        std::fs::write(&two, b"not audio").unwrap();
        std::fs::write(dir.join("Album/cover.jpg"), b"jpeg").unwrap();
        let mut touched = Touched::default();
        watch_sync(
            &mut conn,
            vec![one.clone(), two.clone(), dir.join("Album/cover.jpg")],
            Vec::new(),
            &roots,
            &Exclusions::default(),
            &mut touched,
        )
        .unwrap();
        assert_eq!(store::count(&conn).unwrap(), 2);
        touched.changed.sort_unstable();
        touched.changed.dedup();
        assert_eq!(touched.changed.len(), 2);
        assert!(touched.removed.is_empty());

        let one_id = store::id_for_path(&conn, rox_library::cue::LOCAL, one.to_str().unwrap())
            .unwrap()
            .unwrap();
        let renamed = dir.join("Album/renamed.mp3");
        std::fs::rename(&one, &renamed).unwrap();
        let mut touched = Touched::default();
        let s = watch_sync(
            &mut conn,
            vec![one.clone(), renamed.clone()],
            vec![(one.clone(), renamed.clone())],
            &roots,
            &Exclusions::default(),
            &mut touched,
        )
        .unwrap();
        assert!(touched.changed.contains(&one_id));
        assert!(touched.removed.is_empty());
        assert_eq!(s.renamed, 1);
        assert_eq!(
            store::id_for_path(&conn, rox_library::cue::LOCAL, renamed.to_str().unwrap()).unwrap(),
            Some(one_id),
            "a correlated rename keeps the id"
        );
        assert!(
            store::id_for_path(&conn, rox_library::cue::LOCAL, one.to_str().unwrap())
                .unwrap()
                .is_none()
        );
        assert_eq!(store::count(&conn).unwrap(), 2);

        std::fs::remove_file(&two).unwrap();
        let mut touched = Touched::default();
        watch_sync(
            &mut conn,
            vec![two.clone()],
            Vec::new(),
            &roots,
            &Exclusions::default(),
            &mut touched,
        )
        .unwrap();
        assert_eq!(store::count(&conn).unwrap(), 1);
        assert_eq!(touched.removed.len(), 1);
        assert!(
            store::id_for_path(&conn, rox_library::cue::LOCAL, renamed.to_str().unwrap())
                .unwrap()
                .is_some()
        );

        std::fs::remove_dir_all(dir.join("Album")).unwrap();
        watch_sync(
            &mut conn,
            vec![dir.join("Album")],
            Vec::new(),
            &roots,
            &Exclusions::default(),
            &mut Touched::default(),
        )
        .unwrap();
        assert_eq!(store::count(&conn).unwrap(), 0);

        // Hand the sync the root itself as if it vanished: it must not prune.
        std::fs::create_dir_all(dir.join("Album")).unwrap();
        std::fs::write(&one, b"not audio").unwrap();
        watch_sync(
            &mut conn,
            vec![one.clone()],
            Vec::new(),
            &roots,
            &Exclusions::default(),
            &mut Touched::default(),
        )
        .unwrap();
        assert_eq!(store::count(&conn).unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
        watch_sync(
            &mut conn,
            vec![dir.clone()],
            Vec::new(),
            &roots,
            &Exclusions::default(),
            &mut Touched::default(),
        )
        .unwrap();
        assert_eq!(
            store::count(&conn).unwrap(),
            1,
            "a root that reads gone is never pruned"
        );
    }

    /// The batches carry the rename alone, so the rename handling has to do
    /// it by itself.
    #[test]
    fn watch_sync_renames_across_an_exclusion() {
        let dir = std::env::temp_dir().join(format!("rox-watch-exclude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Album")).unwrap();
        let roots = vec![dir.clone()];
        let exclude = Exclusions::new(&["*.bak".to_string()]);

        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();

        let album = dir.join("Album");
        let backup = dir.join("Album.bak");
        for name in ["1.mp3", "2.mp3"] {
            std::fs::write(album.join(name), b"not audio").unwrap();
        }
        watch_sync(
            &mut conn,
            vec![album.clone()],
            Vec::new(),
            &roots,
            &exclude,
            &mut Touched::default(),
        )
        .unwrap();
        assert_eq!(store::count(&conn).unwrap(), 2);

        std::fs::rename(&album, &backup).unwrap();
        let mut touched = Touched::default();
        let s = watch_sync(
            &mut conn,
            Vec::new(),
            vec![(album.clone(), backup.clone())],
            &roots,
            &exclude,
            &mut touched,
        )
        .unwrap();
        assert_eq!(s.renamed, 0, "a rename into a pattern moves nothing");
        assert_eq!(s.removed, 2);
        assert_eq!(touched.removed.len(), 2);
        assert_eq!(store::count(&conn).unwrap(), 0);

        std::fs::write(backup.join("3.mp3"), b"not audio").unwrap();
        watch_sync(
            &mut conn,
            vec![backup.join("3.mp3")],
            Vec::new(),
            &roots,
            &exclude,
            &mut Touched::default(),
        )
        .unwrap();
        assert_eq!(store::count(&conn).unwrap(), 0);

        std::fs::rename(&backup, &album).unwrap();
        let s = watch_sync(
            &mut conn,
            Vec::new(),
            vec![(backup.clone(), album.clone())],
            &roots,
            &exclude,
            &mut Touched::default(),
        )
        .unwrap();
        assert_eq!(s.renamed, 0);
        assert_eq!(s.updated, 3);
        assert_eq!(store::count(&conn).unwrap(), 3);
        assert!(
            store::id_for_path(
                &conn,
                rox_library::cue::LOCAL,
                album.join("3.mp3").to_str().unwrap()
            )
            .unwrap()
            .is_some()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn m3u_fragments_round_trip_a_cue_rip() {
        use rox_library::cue::TrackKey;
        use rox_library::m3u;
        use rox_library::playlists::ExportTrack;
        use std::path::{Path, PathBuf};

        let mut conn = rox_library::rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let image = "/m/Album/disc.flac";
        store::insert_batch(
            &mut conn,
            &[
                cue_row(image, 1, 0, Some(180_000)),
                cue_row(image, 2, 180_000, Some(400_000)),
                cue_row(image, 3, 400_000, None),
                plain_row("/m/Album/loose.mp3"),
                // A file whose name really ends in `#2`.
                plain_row("/m/Album/track#2"),
            ],
        )
        .unwrap();

        let keys = [
            TrackKey {
                source: rox_library::cue::local(),
                path: PathBuf::from(image),
                sub: 1,
            },
            TrackKey {
                source: rox_library::cue::local(),
                path: PathBuf::from(image),
                sub: 3,
            },
            TrackKey::from(PathBuf::from("/m/Album/loose.mp3")),
            TrackKey::from(PathBuf::from("/m/Album/track#2")),
        ];
        let rows: Vec<ExportTrack> = keys
            .iter()
            .map(|key| ExportTrack {
                path: key.to_fragment(),
                title: "Song".into(),
                artist: "X".into(),
                duration_secs: 180,
            })
            .collect();
        let document = m3u::to_m3u8(&rows);
        assert!(document.contains("/m/Album/disc.flac#1"));
        assert!(document.contains("/m/Album/disc.flac#3"));

        let read: Vec<i64> = m3u::parse(&document)
            .iter()
            .filter_map(|entry| resolve_m3u_entry(&conn, Path::new("/m/Album"), entry))
            .collect();
        let want: Vec<i64> = keys
            .iter()
            .map(|key| {
                store::queue_meta_for_key(&conn, &key.source, key.path.to_str().unwrap(), key.sub)
                    .unwrap()
                    .id
                    .expect("every fixture key is in the library")
            })
            .collect();
        assert_eq!(read, want, "each line comes back as the row it was");
        assert_ne!(read[0], read[1]);

        assert_eq!(
            resolve_m3u_entry(&conn, Path::new("/m/Album"), "disc.flac#2"),
            store::queue_meta_for_key(&conn, rox_library::cue::LOCAL, image, 2)
                .unwrap()
                .id,
        );
        assert_eq!(
            resolve_m3u_entry(&conn, Path::new("/m/Album"), "/m/gone.flac"),
            None
        );
    }

    /// XSPF locations are URIs: the subsong rides in a real fragment and the
    /// path is percent-encoded.
    #[test]
    fn xspf_fragments_round_trip_a_cue_rip() {
        use rox_library::cue::TrackKey;
        use rox_library::playlists::ExportTrack;
        use rox_library::xspf;
        use std::path::{Path, PathBuf};

        let mut conn = rox_library::rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let image = "/m/Album/disc one.flac";
        store::insert_batch(
            &mut conn,
            &[
                cue_row(image, 1, 0, Some(180_000)),
                cue_row(image, 2, 180_000, Some(400_000)),
                plain_row("/m/Album/loose.mp3"),
            ],
        )
        .unwrap();

        let keys = [
            TrackKey {
                source: rox_library::cue::local(),
                path: PathBuf::from(image),
                sub: 2,
            },
            TrackKey {
                source: rox_library::cue::local(),
                path: PathBuf::from(image),
                sub: 1,
            },
            TrackKey::from(PathBuf::from("/m/Album/loose.mp3")),
        ];
        let rows: Vec<ExportTrack> = keys
            .iter()
            .map(|key| ExportTrack {
                path: key.to_fragment(),
                title: "Song".into(),
                artist: "X".into(),
                duration_secs: 180,
            })
            .collect();
        let document = xspf::to_xspf(&rows);
        assert!(
            document.contains("<location>file:///m/Album/disc%20one.flac#2</location>"),
            "{document}"
        );

        let read: Vec<i64> = xspf::parse(&document)
            .iter()
            .filter_map(|entry| resolve_m3u_entry(&conn, Path::new("/m/Album"), entry))
            .collect();
        let want: Vec<i64> = keys
            .iter()
            .map(|key| {
                store::queue_meta_for_key(&conn, &key.source, key.path.to_str().unwrap(), key.sub)
                    .unwrap()
                    .id
                    .expect("every fixture key is in the library")
            })
            .collect();
        assert_eq!(read, want, "each location comes back as the row it was");
        assert_ne!(read[0], read[1]);
    }

    fn plain_row(path: &str) -> rox_library::TrackRow {
        rox_library::TrackRow {
            remote_url: String::new(),
            remote_live: false,
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            path: path.to_string(),
            sub: 0,
            cue: None,
            title: "Song".into(),
            artist: "X".into(),
            album_artist: "X".into(),
            album: "Album".into(),
            genre: String::new(),
            year: 0,
            disc_no: 0,
            track_no: 0,
            duration_ms: 180_000,
            codec: "flac".into(),
            bitrate_kbps: 0,
            sample_rate_hz: 0,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    fn cue_row(path: &str, sub: u16, start_ms: u32, end_ms: Option<u32>) -> rox_library::TrackRow {
        rox_library::TrackRow {
            remote_url: String::new(),
            remote_live: false,
            sub,
            track_no: sub,
            cue: Some(rox_library::CueSlice {
                cue_path: "/m/Album/disc.cue".into(),
                span: rox_library::cue::Span { start_ms, end_ms },
            }),
            ..plain_row(path)
        }
    }
}
