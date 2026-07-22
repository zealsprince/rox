//! The shared library catalog: the `Library` entity over the promoted
//! library service. It owns the on-disk database and hands out only the
//! in-memory projection, and it drives scanning, watching, and the derived
//! playlist mutations. UI-free by design.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, EventEmitter, PathPromptOptions, SharedString, Task};

use rox_library::listens;
use rox_library::playlists;
use rox_library::projection::Projection;
use rox_library::rusqlite::{self, Connection};
use rox_library::scanner::{self, ScanSummary};
use rox_library::store;
use rox_library::writer;

use crate::integrations::library_watch::{LibraryWatcher, WatchBatch};

/// The catalog changed: a scan finished or the projection reloaded. Panels
/// subscribe and refresh their views.
pub enum LibraryEvent {
    Updated,
    /// One rating moved, in place through the shared projection. Its own
    /// variant so the panels that rebuild on Updated - the grid's tiles,
    /// the history reads, the stats recounts - sit a star click out.
    Rated,
    /// One listen landed, its count bumped in place through the shared
    /// projection, same deal as Rated: cells repaint, nothing rebuilds.
    Played,
    /// A playlist was created, renamed, deleted, or had its tracks change.
    /// The playlist panel and the add-to-playlist menu re-read on it.
    PlaylistsChanged,
}

/// Wall clock in unix seconds, for the playlist created/updated stamps.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How often the UI samples a running scan's progress.
const SCAN_POLL: Duration = Duration::from_millis(100);

/// Interim projection swaps while a scan runs, so panels fill in live
/// instead of waiting for the end. An empty library polls fast until the
/// first batch lands - the first scan should paint tracks right away. A
/// populated one takes its first swap after [`SCAN_REFRESH_FIRST`], then
/// settles to [`SCAN_REFRESH_STEADY`].
const SCAN_REFRESH_EMPTY: Duration = Duration::from_secs(1);
const SCAN_REFRESH_FIRST: Duration = Duration::from_secs(15);
const SCAN_REFRESH_STEADY: Duration = Duration::from_secs(30);

/// How stale the last scan must be before launch spends a full catch-up walk
/// on edits made while the app was closed. Under this, a restart trusts the
/// stored projection and the live watch to stay current, so a quick relaunch
/// never re-walks the library. One day: long enough that ordinary restarts
/// skip it, short enough that a day-old offline edit still gets swept in.
const CATCH_UP_STALE: i64 = 24 * 60 * 60;

/// Live progress of a background scan: the scan thread writes it per file,
/// the UI polls it at [`SCAN_POLL`] cadence. Zero total means the folder
/// walk has not finished yet.
#[derive(Default)]
struct ScanProgress {
    scanned: AtomicUsize,
    total: AtomicUsize,
    /// Full path of the file the scan last touched.
    current: Mutex<String>,
    /// Raised by [`Library::abort_scan`]; the scan stops at the next file.
    cancel: AtomicBool,
}

/// What a background refresh does before the projection reloads.
enum Refresh {
    /// Just reload the projection.
    Load,
    /// Scan these folders first.
    Scan(Vec<PathBuf>),
    /// Drop this folder's rows first.
    Remove(PathBuf),
    /// Re-read exactly these files first, the tag editor's write-back:
    /// their rows converge to what is on disk now, duration and codec
    /// included, not just the columns the edit named.
    Reindex(Vec<PathBuf>),
    /// Drop exactly these files' rows first, the duplicates window's
    /// delete: the files are already gone (trashed), so each row goes by
    /// the same scoped delete the watcher uses, no disk walk.
    Prune(Vec<PathBuf>),
    /// Sync a batch of watched paths, the filesystem watcher's per-change
    /// path. A correlated rename moves the row and keeps its id; a path still
    /// on disk is re-read and upserted; one that is gone has its subtree pruned
    /// by a scoped delete. The work is proportional to what changed, never the
    /// library size - no folder walk. The roots ride along so a prune or rename
    /// stays strictly inside them and never wipes a root that only momentarily
    /// reads gone.
    Watch {
        paths: Vec<PathBuf>,
        renames: Vec<(PathBuf, PathBuf)>,
        roots: Vec<PathBuf>,
    },
}

/// One watch batch's rollup, folded into the status line the same way a
/// scan's [`ScanSummary`] is. Terse on purpose: it shares the menubar.
#[derive(Default)]
struct WatchSummary {
    /// Files re-read and upserted this batch.
    updated: usize,
    /// Rows dropped because their paths are gone from disk.
    removed: usize,
    /// Rows moved by a correlated rename, id kept.
    renamed: usize,
}

/// The shared catalog entity. Owns the database and the projection; every
/// library panel reads it, none of them own it.
pub struct Library {
    db_path: PathBuf,
    /// UI-side connection for id -> path lookups; scans and projection loads
    /// open their own connections on the background executor.
    conn: Option<Connection>,
    projection: Option<Arc<Projection>>,
    /// The canonical browse order: album artist, album, disc, track number.
    order: Arc<Vec<u32>>,
    /// The folders scans read from, in the order they were added,
    /// persisted in settings. Empty until a folder has been opened.
    scan_roots: Vec<PathBuf>,
    /// Set while a scan or projection load runs in the background.
    busy: Option<SharedString>,
    /// The running scan's progress, while one runs; the handle abort
    /// reaches through.
    scan: Option<Arc<ScanProgress>>,
    /// Rating clicks waiting on their tag write, newest value per track,
    /// and whether the one-at-a-time drain is running.
    pending_ratings: HashMap<i64, u8>,
    rating_write_running: bool,
    status: SharedString,
    /// Whether watching is meant to be on, mirroring the setting. Kept apart
    /// from `watcher`, which is None both when off and when there are no roots
    /// to watch yet, so adding the first folder knows to arm.
    watch_on: bool,
    /// The live filesystem watcher over the roots, while watching is on. None
    /// when off or when the platform watcher would not start.
    watcher: Option<LibraryWatcher>,
    /// The loop pulling change batches off the watcher; dropped to stop it.
    watch_task: Option<Task<()>>,
    /// Paths a watch batch reported, waiting on their sync. The set dedups a
    /// burst's repeats; it drains into one `Refresh::Watch` once no other
    /// refresh is running, so changes fold into a single re-read-and-swap.
    pending: HashSet<PathBuf>,
    /// Correlated renames waiting on the same sync, carried apart from
    /// `pending` because a pair keeps the row's id and a plain path does not.
    pending_renames: Vec<(PathBuf, PathBuf)>,
    /// Paths the app itself just wrote, with when it wrote them. A watch
    /// batch filters against this so the app's own tag, rating, and cover
    /// writes do not bounce back as a redundant reindex of a file it just
    /// touched.
    self_writes: HashMap<PathBuf, std::time::Instant>,
}

impl EventEmitter<LibraryEvent> for Library {}

impl Library {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let db_path = crate::settings::data_dir().join("library.db");
        let (conn, status) =
            match store::open(&db_path).and_then(|conn| store::init_schema(&conn).map(|_| conn)) {
                Ok(conn) => (Some(conn), SharedString::default()),
                Err(e) => (None, SharedString::from(format!("library db: {e}"))),
            };
        // The favourites playlist is the one default: make it up front so the
        // heart column and the Favourites menu always have somewhere to write,
        // and it shows in the playlists panel from a cold start.
        if let Some(conn) = &conn {
            let _ = playlists::ensure_favourites(conn, now_secs());
        }

        // The same never-nests rule add_root keeps, applied to the loaded
        // list: hand-edited files and lists from before the guard flatten
        // here, a nested folder falling to the one that covers it.
        let loaded = crate::settings::Settings::load().library_roots;
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
            crate::settings::Settings::update(move |s| s.library_roots = roots);
        }

        // A library indexed before roots were persisted still has one in
        // its paths: the deepest directory shared by every track. Session
        // only; the next Open Folder persists the whole list.
        if scan_roots.is_empty() {
            if let Some(root) = conn
                .as_ref()
                .and_then(|conn| store::common_root(conn).ok().flatten())
            {
                scan_roots.push(root);
            }
        }

        let mut this = Library {
            db_path,
            conn,
            projection: None,
            order: Arc::new(Vec::new()),
            scan_roots,
            busy: None,
            scan: None,
            pending_ratings: HashMap::new(),
            rating_write_running: false,
            status,
            watch_on: crate::settings::Settings::load().watch_library,
            watcher: None,
            watch_task: None,
            pending: HashSet::new(),
            pending_renames: Vec::new(),
            self_writes: HashMap::new(),
        };
        // Watching only sees changes made while the app runs, so edits made
        // while it was closed still need one catch-up pass. When watch is on
        // with roots and the last scan has gone stale, open on a scan instead
        // of a plain projection load: the scan reloads the projection all the
        // same, and the interim-projection machinery keeps it non-blocking, so
        // the "no manual rescan" promise holds across restarts. A recent scan
        // skips it, so a quick restart does not walk the whole library again;
        // off, or with no roots, the plain load stands.
        let last_scan = crate::settings::Settings::load().last_scan;
        let stale = now_secs().saturating_sub(last_scan) > CATCH_UP_STALE;
        let catch_up = this.watch_on && !this.scan_roots.is_empty() && stale;
        if this.conn.is_some() {
            if catch_up {
                this.reload(Refresh::Scan(this.scan_roots.clone()), cx);
            } else {
                this.reload(Refresh::Load, cx);
            }
        }
        // Arm the watcher if the setting keeps it on, so live changes from
        // here on fold in over the catch-up above.
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

    /// The running background operation's label, for the menubar's badge.
    pub fn busy(&self) -> Option<SharedString> {
        self.busy.clone()
    }

    /// The last status line: the track count, scan counts, or an error.
    /// While a scan runs, the file it is currently on.
    pub fn status(&self) -> SharedString {
        self.status.clone()
    }

    /// Whether a rescan has folders to scan.
    pub fn can_rescan(&self) -> bool {
        !self.scan_roots.is_empty()
    }

    /// Whether a scan is running right now, for the menubar's abort button.
    /// False for other background work: only scans can be aborted.
    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// Stop the running scan at the next file. What it already indexed
    /// stays in the library; the projection reload still follows, so the
    /// partial result shows up. A no-op when no scan is running.
    pub fn abort_scan(&mut self, cx: &mut Context<Self>) {
        let Some(scan) = &self.scan else {
            return;
        };
        scan.cancel.store(true, Ordering::Relaxed);
        self.busy = Some("stopping...".into());
        cx.notify();
    }

    /// Scan every remembered folder again; a no-op until one has been
    /// opened or while a scan is already running.
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.scan_roots.is_empty() {
            return;
        }
        self.reload(Refresh::Scan(self.scan_roots.clone()), cx);
    }

    /// Each folder with its rollup - tracks, albums, bytes on disk - on
    /// the UI-side connection. The list never nests, so nothing counts
    /// twice.
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

    /// The remembered folders, the roots a maintenance pass walks: the tag
    /// repair window scans the whole library by walking exactly these.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.scan_roots.clone()
    }

    /// The whole library's rollup, for the storage page.
    pub fn stats(&self) -> store::Stats {
        self.conn
            .as_ref()
            .and_then(|conn| store::stats(conn).ok())
            .unwrap_or_default()
    }

    /// Add a folder and scan it. The list never nests, so counts never
    /// overlap and removals never reach into another folder's tracks: one
    /// already covered by a listed folder is not added, just rescanned,
    /// and one that covers listed folders absorbs them. A no-op while a
    /// scan is running.
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

    /// Drop a folder: out of the list, its tracks out of the database. The
    /// files themselves are untouched. A no-op while a scan is running.
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

    /// Write the folder list through the settings file.
    fn persist_roots(&self) {
        let roots = self.scan_roots.clone();
        crate::settings::Settings::update(move |s| s.library_roots = roots);
    }

    /// Whether the library is watching its roots right now.
    pub fn watching(&self) -> bool {
        self.watcher.is_some()
    }

    /// How many roots the live watch actually covers versus how many it was
    /// asked to, so a partial watch (a missing folder, an unplugged drive)
    /// can be surfaced. None when watching is off. The settings UI hookup to
    /// show a partial or failed watch is a follow-up; this only exposes the
    /// data.
    pub fn watch_coverage(&self) -> Option<(usize, usize)> {
        self.watcher.as_ref().map(|w| w.coverage())
    }

    /// Note paths the app is about to write itself, so the next watch batch
    /// can drop them instead of reindexing a file the app just touched. Called
    /// from every point that initiates a file write - a tag or cover commit, a
    /// rating write - with the target path.
    pub fn note_self_write<I>(&mut self, paths: I)
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let now = std::time::Instant::now();
        for path in paths {
            self.self_writes.insert(path, now);
        }
    }

    /// Turn filesystem watching on or off and remember the choice. On arms a
    /// watcher over the current roots and starts folding live changes in; off
    /// drops it and any pending work, so updates stop until it is turned back
    /// on or the next manual rescan.
    pub fn set_watch(&mut self, on: bool, cx: &mut Context<Self>) {
        self.watch_on = on;
        crate::settings::Settings::update(move |s| s.watch_library = on);
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

    /// Bring the watcher up over the current roots and start draining its
    /// change batches. A no-op when there are no roots to watch; replaces any
    /// live watcher, so it doubles as the re-arm after the folder list moves.
    ///
    /// Arming a recursive watch walks the whole tree adding one OS watch per
    /// directory, slow enough on a big library to stall the window, so the
    /// build runs off the UI thread and the handle lands on the entity once
    /// it is ready. Dropping the prior task cancels an in-flight build, so a
    /// quick re-arm never leaves two watchers running.
    fn arm_watch(&mut self, cx: &mut Context<Self>) {
        self.watcher = None;
        self.watch_task = None;
        if self.scan_roots.is_empty() {
            return;
        }
        let roots = self.scan_roots.clone();
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            let Some(watcher) = cx
                .background_executor()
                .spawn(async move { LibraryWatcher::new(&roots) })
                .await
            else {
                return;
            };
            let events = watcher.events();
            // Store the handle only if watching is still wanted; a toggle-off
            // or a newer re-arm that raced this build wins.
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

    /// Re-point the watcher at the current roots after the folder list moves,
    /// but only while watching is on, so an add or remove never turns it back
    /// on behind the setting.
    fn rearm_watch(&mut self, cx: &mut Context<Self>) {
        if self.watch_on {
            self.arm_watch(cx);
        }
    }

    /// Take in a watch batch and kick the sync. Cheap on the UI thread: the
    /// self-write filter and a couple of buffer inserts, no disk touched; the
    /// sort into renames, re-reads, and prunes happens off-thread in the sync
    /// itself.
    fn note_changes(&mut self, batch: WatchBatch, cx: &mut Context<Self>) {
        // Drop the app's own writes so a tag, rating, or cover commit does not
        // bounce back as a redundant reindex. The window sits a few seconds
        // past the 1s debounce, comfortably long enough to cover the write ->
        // flush -> deliver round trip; expired entries clear each pass so the
        // map never grows. A missed suppression only costs one reindex, so the
        // window errs short rather than eat a real user edit.
        let now = std::time::Instant::now();
        let window = std::time::Duration::from_secs(5);
        self.self_writes.retain(|_, at| now.duration_since(*at) < window);
        let fresh = self
            .self_writes
            .iter()
            .filter(|(_, at)| now.duration_since(**at) < window)
            .map(|(p, _)| p.clone())
            .collect::<HashSet<_>>();
        self.pending
            .extend(batch.paths.into_iter().filter(|p| !fresh.contains(p)));
        self.pending_renames.extend(batch.renames);
        self.pump_watch(cx);
    }

    /// Drain the pending paths and renames into one `Refresh::Watch`, once no
    /// other refresh holds the badge. Re-run after every reload finishes, which
    /// is how changes that arrived mid-refresh get picked up.
    fn pump_watch(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() || (self.pending.is_empty() && self.pending_renames.is_empty()) {
            return;
        }
        let paths: Vec<PathBuf> = self.pending.drain().collect();
        let renames = std::mem::take(&mut self.pending_renames);
        let roots = self.scan_roots.clone();
        self.reload(Refresh::Watch { paths, renames, roots }, cx);
    }

    /// Resolve database ids to playable paths on the UI-side connection.
    pub fn paths_for(&self, ids: &[i64]) -> Result<Vec<PathBuf>, String> {
        let Some(conn) = &self.conn else {
            return Ok(Vec::new());
        };
        store::paths_for(conn, ids)
            .map(|paths| paths.into_iter().map(Into::into).collect())
            .map_err(|e| e.to_string())
    }

    /// Resolve a playing file back to its tags on the UI-side connection,
    /// for the track info panel. None when the path is not in the library.
    pub fn meta_for(&self, path: &std::path::Path) -> Option<store::TrackMeta> {
        let conn = self.conn.as_ref()?;
        store::meta_for_path(conn, path.to_str()?).ok().flatten()
    }

    /// Resolve a playing file to its track id on the UI-side connection,
    /// for marking its row. None when the path is not in the library.
    pub fn id_for(&self, path: &std::path::Path) -> Option<i64> {
        let conn = self.conn.as_ref()?;
        store::id_for_path(conn, path.to_str()?).ok().flatten()
    }

    /// Resolve a path to its track id and tags together in one query, for
    /// callers (the queue) that want both and would otherwise pay `id_for`
    /// plus `meta_for` separately. None when the path is not in the library.
    pub fn resolve_path(&self, path: &std::path::Path) -> Option<(i64, store::TrackMeta)> {
        let conn = self.conn.as_ref()?;
        store::meta_row_for_path(conn, path.to_str()?).ok().flatten()
    }

    /// The history views' reads, on the UI-side connection: SQL over the
    /// indexed events table at panel-open and listen-append cadence, per
    /// ADR 11, never per keystroke or frame.
    pub fn recent_listens(&self, since: i64, limit: usize) -> Vec<listens::TrackPlays> {
        self.listen_query(|conn| listens::recent(conn, since, limit))
    }

    pub fn most_played(&self, limit: usize) -> Vec<listens::TrackPlays> {
        self.listen_query(|conn| listens::most_played(conn, limit))
    }

    pub fn never_played(&self, limit: usize) -> Vec<listens::TrackPlays> {
        self.listen_query(|conn| listens::never_played(conn, limit))
    }

    /// Play counts grouped under one tag over a trailing range, the
    /// stats panel's rollups; `since` 0 counts every event.
    pub fn listen_rollup(
        &self,
        by: listens::Rollup,
        since: i64,
        limit: usize,
    ) -> Vec<listens::NamePlays> {
        self.listen_query(|conn| listens::rollup(conn, by, since, limit))
    }

    /// How many listens landed at or after `since` (unix seconds).
    pub fn listens_since(&self, since: i64) -> u64 {
        self.conn
            .as_ref()
            .and_then(|conn| listens::count_since(conn, since).ok())
            .unwrap_or_default()
    }

    /// When the first listen landed; None before any has.
    pub fn first_listen(&self) -> Option<i64> {
        self.conn
            .as_ref()
            .and_then(|conn| listens::earliest(conn).ok())
            .flatten()
    }

    /// Listens bucketed over time, the stats chart's bars.
    pub fn listen_histogram(&self, since: i64, bucket: i64, now: i64) -> Vec<u64> {
        self.listen_query(|conn| listens::histogram(conn, since, bucket, now))
    }

    /// Resolve a rollup name to its library tracks in browse order, so
    /// a stats row can queue what it counts.
    pub fn ids_for_rollup(&self, by: listens::Rollup, name: &str, limit: usize) -> Vec<i64> {
        self.listen_query(|conn| listens::ids_for_name(conn, by, name, limit))
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

    /// Every playlist with its track count, for the sidebar and the
    /// add-to-playlist menu.
    pub fn playlists(&self) -> Vec<playlists::Playlist> {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::list(conn).ok())
            .unwrap_or_default()
    }

    /// One playlist's tracks in order, for the playlist panel.
    pub fn playlist_tracks(&self, id: i64) -> Vec<playlists::PlaylistTrack> {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::tracks(conn, id).ok())
            .unwrap_or_default()
    }

    /// One playlist's playable track ids in order, what the panel hands the
    /// player to start the whole list.
    pub fn playlist_ids(&self, id: i64) -> Vec<i64> {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::ids(conn, id).ok())
            .unwrap_or_default()
    }

    /// One playlist's playable members resolved for an M3U export, in order.
    pub fn playlist_export_rows(&self, id: i64) -> Vec<playlists::ExportTrack> {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::export_rows(conn, id).ok())
            .unwrap_or_default()
    }

    /// The favourited track ids, what the library's heart column checks each
    /// row against.
    pub fn favourite_ids(&self) -> HashSet<i64> {
        self.conn
            .as_ref()
            .and_then(|conn| playlists::favourite_track_ids(conn).ok())
            .map(|ids| ids.into_iter().collect())
            .unwrap_or_default()
    }

    /// Turn favourite on or off for a set of tracks at once, the heart click
    /// and the Favourites menu. One event for the whole batch.
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

    /// Create an empty playlist and return its id.
    pub fn create_playlist(&mut self, name: &str, cx: &mut Context<Self>) -> Option<i64> {
        let conn = self.conn.as_ref()?;
        let id = playlists::create(conn, name, now_secs()).ok()?;
        cx.emit(LibraryEvent::PlaylistsChanged);
        Some(id)
    }

    /// Rename a playlist.
    pub fn rename_playlist(&mut self, id: i64, name: &str, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else { return };
        if playlists::rename(conn, id, name, now_secs()).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    /// Delete a playlist and its member rows.
    pub fn delete_playlist(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        if playlists::delete(conn, id).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    /// Append tracks to a playlist, keeping duplicates.
    pub fn add_to_playlist(&mut self, id: i64, track_ids: &[i64], cx: &mut Context<Self>) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        if playlists::add(conn, id, track_ids, now_secs()).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    /// Drop a drag of members into `playlist_id` before `before` (or at the
    /// end when None): the one call behind every playlist drag, single or
    /// multi, reorder or cross-playlist move.
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

    /// Drop several members at once, a multi-select remove or Delete keypress.
    pub fn remove_playlist_members(&mut self, member_ids: &[i64], cx: &mut Context<Self>) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        if playlists::remove_members(conn, member_ids, now_secs()).is_ok() {
            cx.emit(LibraryEvent::PlaylistsChanged);
        }
    }

    /// Build a playlist from imported M3U entries: resolve each path to a
    /// catalog track, relative paths against `base_dir` (the file's folder),
    /// and add the hits in order. Entries the library never scanned fall away,
    /// there is no file behind them to play. Returns the new playlist's id.
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
            .filter_map(|entry| {
                let path = Path::new(entry);
                let full = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    base_dir.join(path)
                };
                store::id_for_path(conn, full.to_str()?).ok().flatten()
            })
            .collect();
        let id = playlists::create(conn, name, now_secs()).ok()?;
        if !ids.is_empty() {
            playlists::add(conn, id, &ids, now_secs()).ok()?;
        }
        cx.emit(LibraryEvent::PlaylistsChanged);
        Some(id)
    }

    /// A committed tag edit into the catalog: the named columns land first
    /// on the UI connection, so a busy library that drops the reload still
    /// shows the edit, then the file is re-read whole so the row converges
    /// to what the writer put on disk. The optimistic patch alone left
    /// duration, codec, and the like on their stale scan values; the
    /// reindex behind it carries those too. The file was already written
    /// and verified by the caller.
    pub fn apply_edit(&mut self, path: &Path, changes: &[writer::Change], cx: &mut Context<Self>) {
        // The caller already wrote the file; note it so the watch batch it
        // triggers does not bounce back as a redundant reindex.
        self.note_self_write([path.to_path_buf()]);
        if let Some((id, conn)) = self.id_for(path).zip(self.conn.as_ref()) {
            if let Err(e) = store::apply_changes(conn, id, changes) {
                self.status = format!("library: {e}").into();
                cx.notify();
            }
        }
        self.reload(Refresh::Reindex(vec![path.to_path_buf()]), cx);
    }

    /// A batch of committed edits into the catalog, the tag editor's save:
    /// every named column lands first on the UI connection, then one
    /// reindex re-reads the whole batch off disk so duration, codec, and
    /// every other scanner-derived field converge with the edit, not just
    /// the columns the form named. A file the writer fixed or a filename
    /// the user finally tagged both read back true here.
    pub fn apply_edits(&mut self, edits: &[writer::Edit], cx: &mut Context<Self>) {
        for edit in edits {
            let Some(id) = self.id_for(&edit.path) else {
                continue;
            };
            let Some(conn) = &self.conn else { return };
            if let Err(e) = store::apply_changes(conn, id, &edit.changes) {
                self.status = format!("library: {e}").into();
                cx.notify();
            }
        }
        let paths: Vec<PathBuf> = edits.iter().map(|edit| edit.path.clone()).collect();
        // The caller wrote these files; note them so the watch batch they
        // trigger does not bounce back as a redundant reindex.
        self.note_self_write(paths.iter().cloned());
        self.reload(Refresh::Reindex(paths), cx);
    }

    /// Drop these files' rows from the catalog, the duplicates window's
    /// write-back after trashing them. The paths are deliberately not noted
    /// as self-writes: if the library is busy and this reload is dropped,
    /// the watcher still sees the deletions and prunes the same rows, so the
    /// catalog converges either way; when both run, the second prune is a
    /// no-op.
    pub fn remove_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        self.reload(Refresh::Prune(paths), cx);
    }

    /// A rating click into the catalog: onto the track's database row, and
    /// into the shared projection in place - its ratings are atomics exactly
    /// so this never pays the reload a tag edit does, and it works mid-scan
    /// where a reload would be dropped. The row/id pair guards against a
    /// projection swapped between paint and click; when they disagree the
    /// database row still lands and the next reload shows it. The file's
    /// tags follow through the write queue below.
    /// Rate a track by id, resolving its projection row for the in-place
    /// atomic update. The library table already holds the row and calls
    /// [`set_rating`](Self::set_rating) directly; surfaces that only know
    /// the id (the playlists tree) come through here. A track not in the
    /// projection still lands on disk and shows on the next reload.
    pub fn rate(&mut self, id: i64, rating: u8, cx: &mut Context<Self>) {
        let row = self
            .projection
            .as_ref()
            .and_then(|p| p.db_id.iter().position(|&other| other == id))
            .map(|r| r as u32)
            .unwrap_or(u32::MAX);
        self.set_rating(row, id, rating, cx);
    }

    pub fn set_rating(&mut self, row: u32, id: i64, rating: u8, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else { return };
        if let Err(e) = store::set_rating(conn, id, rating) {
            self.status = format!("library: {e}").into();
            cx.notify();
            return;
        }
        if let Some(projection) = &self.projection {
            if projection.db_id.get(row as usize) == Some(&id) {
                projection.rating[row as usize].store(rating, Ordering::Relaxed);
            }
        }
        self.queue_rating_write(id, rating, cx);
        cx.emit(LibraryEvent::Rated);
        cx.notify();
    }

    /// A landed listen into the shared projection in place: plays are
    /// atomics like the ratings, so the count moves without the reload
    /// a catalog change pays. The event row is already on disk; this
    /// only refreshes the cached column, per ADR 11 the events stay
    /// the source.
    pub fn record_play(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(projection) = &self.projection else {
            return;
        };
        if let Some(row) = projection.db_id.iter().position(|&other| other == id) {
            projection.plays[row].fetch_add(1, Ordering::Relaxed);
            cx.emit(LibraryEvent::Played);
        }
    }

    /// The total play count for each of `ids`, off the in-memory projection,
    /// in one pass. A track not in the catalog (a deleted playlist member) is
    /// absent from the map. What the queue and playlists plays column reads.
    pub fn plays_for(&self, ids: &[i64]) -> HashMap<i64, u32> {
        let Some(projection) = &self.projection else {
            return HashMap::new();
        };
        let wanted: HashSet<i64> = ids.iter().copied().collect();
        projection
            .db_id
            .iter()
            .enumerate()
            .filter(|(_, id)| wanted.contains(id))
            .map(|(row, &id)| (id, projection.plays[row].load(Ordering::Relaxed)))
            .collect()
    }

    /// Queue one track's rating for its tag write. The map holds the
    /// newest value per track and one drain runs at a time, so rapid
    /// clicks collapse to the last value instead of racing the writer's
    /// clone-and-rename on the same file.
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
                let Ok(Some(path)) = this.update(cx, |this, _| {
                    let path = this.paths_for(&[id]).ok().and_then(|mut paths| paths.pop());
                    // Note the write before it lands so the watch batch it
                    // triggers is suppressed, not reindexed.
                    if let Some(path) = &path {
                        this.note_self_write([path.clone()]);
                    }
                    path
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
                        writer::commit(&path, &[change]).map_err(|e| (path, e))
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

    /// Run a refresh off the UI thread: its own step first, then the
    /// projection reload. The finished projection and its canonical sort
    /// swap in whole. One refresh at a time: while one runs, another is
    /// dropped here, so two never race on the database or the badge.
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
            self.refresh_during_scan(cx);
        }
        // A watch sync that errors loses its drained batch unless it is put
        // back, so keep a copy to re-queue on failure. Only the watch path
        // owns pending work; the others carry nothing to retry.
        let retry = match &refresh {
            Refresh::Watch { paths, renames, .. } => Some((paths.clone(), renames.clone())),
            _ => None,
        };
        // A full scan is the only refresh that reconciles the whole library
        // with disk, so it stamps the catch-up clock; the incremental watch
        // syncs and projection loads leave it be.
        let was_scan = matches!(refresh, Refresh::Scan(_));
        let db_path = self.db_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load(&db_path, refresh, &progress) })
                .await;
            this.update(cx, |this, cx| {
                this.busy = None;
                this.scan = None;
                let ok = result.is_ok();
                match result {
                    Ok((projection, order, summary, watch)) => {
                        this.status =
                            status_line(projection.len(), summary.as_ref(), watch.as_ref()).into();
                        this.projection = Some(Arc::new(projection));
                        this.order = Arc::new(order);
                        // A finished scan reconciled with disk; stamp now so
                        // the next launch's catch-up only fires once it ages. An
                        // aborted walk never finished, so leave last_scan alone
                        // and let the next launch catch up.
                        let aborted = summary.as_ref().is_some_and(|s| s.aborted);
                        if was_scan && !aborted {
                            let now = now_secs();
                            crate::settings::Settings::update(move |s| s.last_scan = now);
                        }
                    }
                    Err(e) => {
                        this.status = format!("library: {e}").into();
                        // Put the drained watch batch back so it is not lost;
                        // the errored sync left its paths unapplied and they
                        // would otherwise wait on a manual rescan.
                        if let Some((paths, renames)) = retry {
                            this.pending.extend(paths);
                            this.pending_renames.extend(renames);
                        }
                    }
                }
                cx.emit(LibraryEvent::Updated);
                cx.notify();
                // On success, drain anything that arrived mid-refresh now that
                // the badge is free. On error, hold: the batch just went back
                // into pending, and re-pumping it here would busy-loop against
                // whatever is failing. The next watch event or a manual rescan
                // picks it back up.
                if ok {
                    this.pump_watch(cx);
                }
            })
            .ok();
        })
        .detach();
        cx.emit(LibraryEvent::Updated);
        cx.notify();
    }

    /// Mirror a running scan into the busy badge and status line: the count
    /// so far and the file under the cursor. Stops itself once the reload
    /// clears `busy`; only observers repaint, panels see no event.
    fn poll_scan(&self, progress: Arc<ScanProgress>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SCAN_POLL).await;
                let live = this.update(cx, |this, cx| {
                    if this.busy.is_none() {
                        return false;
                    }
                    let total = progress.total.load(Ordering::Relaxed);
                    // A pending stop owns the badge; counting on would
                    // contradict it.
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

    /// Swap interim projections in while a scan runs, so panels fill in
    /// live. The scanner commits per batch and the store is WAL, so a
    /// reader sees whatever has landed. Each swap is the same whole
    /// replace the final reload does, minus the status line and badge,
    /// which the scan still owns. Stops itself when the scan ends; the
    /// final reload swaps the authoritative result.
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
                let db_path = db_path.clone();
                let loaded = cx
                    .background_executor()
                    .spawn(async move { load_projection(&db_path) })
                    .await;
                let Ok((projection, order)) = loaded else {
                    continue;
                };
                // Nothing indexed yet: keep the fast poll until the
                // first batch lands.
                if projection.is_empty() && delay == SCAN_REFRESH_EMPTY {
                    continue;
                }
                delay = if delay == SCAN_REFRESH_EMPTY {
                    SCAN_REFRESH_FIRST
                } else {
                    SCAN_REFRESH_STEADY
                };
                let live = this.update(cx, |this, cx| {
                    // The scan finished while this projection loaded; the
                    // final reload's swap is newer, keep it.
                    if this.scan.is_none() {
                        return false;
                    }
                    this.projection = Some(Arc::new(projection));
                    this.order = Arc::new(order);
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

    /// Prompt for a folder and add it to the library.
    pub fn browse(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await {
                if let Some(root) = paths.pop() {
                    this.update(cx, |this, cx| this.add_root(root, cx)).ok();
                }
            }
        })
        .detach();
    }
}

#[allow(clippy::type_complexity)]
fn load(
    db_path: &std::path::Path,
    refresh: Refresh,
    progress: &ScanProgress,
) -> Result<
    (Projection, Vec<u32>, Option<ScanSummary>, Option<WatchSummary>),
    rox_library::rusqlite::Error,
> {
    let mut watch = None;
    let summary = match refresh {
        Refresh::Load => None,
        Refresh::Scan(roots) => {
            let mut conn = store::open(db_path)?;
            store::init_schema(&conn)?;
            // One summary and one running count across the folders; later
            // folders grow the total as their walks finish.
            let mut summary = ScanSummary::default();
            let mut done = 0;
            for root in roots {
                // The scanner ticks this closure per file from its worker
                // threads, so the captured state is atomics, not &mut.
                let root_total = AtomicUsize::new(0);
                let s = scanner::scan(&mut conn, &root, |scanned, total, path| {
                    root_total.store(total, Ordering::Relaxed);
                    progress.scanned.store(done + scanned, Ordering::Relaxed);
                    progress.total.store(done + total, Ordering::Relaxed);
                    *progress.current.lock().unwrap() = path.to_string_lossy().into_owned();
                    !progress.cancel.load(Ordering::Relaxed)
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
            scanner::reindex(&mut conn, &paths)?;
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
        Refresh::Watch { paths, renames, roots } => {
            let mut conn = store::open(db_path)?;
            store::init_schema(&conn)?;
            watch = Some(watch_sync(&mut conn, paths, renames, &roots)?);
            None
        }
    };
    let (projection, order) = load_projection(db_path)?;
    Ok((projection, order, summary, watch))
}

/// Sort one watch batch into renames, re-reads, and prunes and apply all
/// three, the cheap half of watching: the work scales with what changed, not
/// the library. Renames go first: moving the row keeps its id (and with it the
/// `added` stamp, the db-only rating, and the joins), and the order is safe -
/// after the move the `to` path exists, so the existence pass below re-reads
/// it and reconciles mtime, and the `from` path is gone, so its prune finds
/// nothing. A path still on disk is re-read and upserted through the reindex
/// path (only audio files become rows, so cover art and stray files are
/// skipped); one that is gone has its subtree pruned by a scoped delete, no
/// folder walk. A rename or prune only touches endpoints strictly inside a
/// root, so a root that momentarily reads gone - a rename in flight, a remount
/// - never wipes the library. Returns the batch's rollup. Blocking; run it off
/// the UI thread.
fn watch_sync(
    conn: &mut Connection,
    paths: Vec<PathBuf>,
    renames: Vec<(PathBuf, PathBuf)>,
    roots: &[PathBuf],
) -> Result<WatchSummary, rox_library::rusqlite::Error> {
    let under_root = |path: &Path| roots.iter().any(|root| path.starts_with(root) && path != *root);
    let mut summary = WatchSummary::default();
    // Only a rename with both endpoints strictly inside a root moves a row;
    // anything reaching a root boundary or out of the tree falls through to
    // the existence routing, which handles it as a plain create or delete.
    for (from, to) in renames {
        if under_root(&from) && under_root(&to) {
            summary.renamed += store::rename_within(conn, &from, &to)?;
        }
    }
    let mut changed = Vec::new();
    let mut removed = Vec::new();
    for path in paths {
        if path.exists() {
            if path.is_dir() {
                // A directory moved into a root lands as one dir-path event.
                // A dir is not is_audio, so without walking it the tracks
                // inside never get indexed. Its counterpart, a dir moved out,
                // is a non-existent path that remove_subtree already prunes.
                if under_root(&path) {
                    changed.extend(scanner::audio_files(&path));
                }
            } else if scanner::is_audio(&path) {
                changed.push(path);
            }
        } else if under_root(&path) {
            removed.push(path);
        }
    }
    if !changed.is_empty() {
        summary.updated += scanner::reindex(conn, &changed)?;
    }
    for path in &removed {
        summary.removed += store::remove_subtree(conn, path)?;
    }
    Ok(summary)
}

/// Load the projection and its canonical order, sharded across cores.
/// Blocking; run it off the UI thread.
fn load_projection(
    db_path: &std::path::Path,
) -> Result<(Projection, Vec<u32>), rox_library::rusqlite::Error> {
    let shards = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let projection = Projection::load_parallel(db_path, shards)?;
    let order = projection.sort_canonical();
    Ok((projection, order))
}

fn status_line(
    total: usize,
    summary: Option<&ScanSummary>,
    watch: Option<&WatchSummary>,
) -> String {
    // Zero counts say nothing, keep them out so the line stays short
    // enough for the menubar.
    let mut parts = Vec::new();
    if let Some(s) = summary {
        if s.indexed > 0 {
            parts.push(format!("{} indexed", s.indexed));
        }
        if s.unchanged > 0 {
            parts.push(format!("{} unchanged", s.unchanged));
        }
        if s.untagged > 0 {
            parts.push(format!("{} untagged", s.untagged));
        }
        if s.removed > 0 {
            parts.push(format!("{} removed", s.removed));
        }
        if s.aborted {
            parts.push("stopped early".into());
        }
    }
    // A watch sync speaks its own counts, terse in the same voice as a scan.
    if let Some(w) = watch {
        if w.updated > 0 {
            parts.push(format!("{} updated", w.updated));
        }
        if w.removed > 0 {
            parts.push(format!("{} removed", w.removed));
        }
        if w.renamed > 0 {
            parts.push(format!("{} renamed", w.renamed));
        }
    }
    if parts.is_empty() {
        return format!("{total} tracks");
    }
    format!("{} tracks ({})", total, parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::watch_sync;
    use rox_library::store;
    use std::path::PathBuf;

    /// The watcher's per-change sync: a new file on disk lands as a row, a
    /// deleted one drops out, a deleted folder takes its subtree, and a path
    /// equal to a root is never pruned even when it reads gone.
    #[test]
    fn watch_sync_reindexes_present_and_prunes_gone() {
        let dir = std::env::temp_dir().join("rox-watch-sync");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Album")).unwrap();
        let roots = vec![dir.clone()];

        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();

        // Two real files on disk, indexed through the sync's re-read path.
        // Dummy bytes index under the filename, enough to make a row.
        let one = dir.join("Album/1.mp3");
        let two = dir.join("Album/2.mp3");
        std::fs::write(&one, b"not audio").unwrap();
        std::fs::write(&two, b"not audio").unwrap();
        // A cover write rides the same batch and must not become a row.
        std::fs::write(dir.join("Album/cover.jpg"), b"jpeg").unwrap();
        watch_sync(
            &mut conn,
            vec![one.clone(), two.clone(), dir.join("Album/cover.jpg")],
            Vec::new(),
            &roots,
        )
        .unwrap();
        assert_eq!(store::count(&conn).unwrap(), 2);

        // A correlated rename moves the row and keeps its id, so the moved
        // file is not a fresh insert. The renamed-then-present path re-reads
        // clean since the file is on disk under its new name.
        let one_id = store::id_for_path(&conn, one.to_str().unwrap()).unwrap().unwrap();
        let renamed = dir.join("Album/renamed.mp3");
        std::fs::rename(&one, &renamed).unwrap();
        let s = watch_sync(
            &mut conn,
            vec![one.clone(), renamed.clone()],
            vec![(one.clone(), renamed.clone())],
            &roots,
        )
        .unwrap();
        assert_eq!(s.renamed, 1);
        assert_eq!(
            store::id_for_path(&conn, renamed.to_str().unwrap()).unwrap(),
            Some(one_id),
            "a correlated rename keeps the id"
        );
        assert!(store::id_for_path(&conn, one.to_str().unwrap()).unwrap().is_none());
        assert_eq!(store::count(&conn).unwrap(), 2);

        // Delete one file on disk, then sync its path: only its row goes.
        std::fs::remove_file(&two).unwrap();
        watch_sync(&mut conn, vec![two.clone()], Vec::new(), &roots).unwrap();
        assert_eq!(store::count(&conn).unwrap(), 1);
        assert!(store::id_for_path(&conn, renamed.to_str().unwrap())
            .unwrap()
            .is_some());

        // Delete the whole Album folder, sync its path: the subtree prunes
        // with no walk.
        std::fs::remove_dir_all(dir.join("Album")).unwrap();
        watch_sync(&mut conn, vec![dir.join("Album")], Vec::new(), &roots).unwrap();
        assert_eq!(store::count(&conn).unwrap(), 0);

        // Re-seed a row, then hand the sync the root path itself as if it
        // vanished: the guard refuses to prune a root, so the row survives.
        std::fs::create_dir_all(dir.join("Album")).unwrap();
        std::fs::write(&one, b"not audio").unwrap();
        watch_sync(&mut conn, vec![one.clone()], Vec::new(), &roots).unwrap();
        assert_eq!(store::count(&conn).unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
        watch_sync(&mut conn, vec![dir.clone()], Vec::new(), &roots).unwrap();
        assert_eq!(
            store::count(&conn).unwrap(),
            1,
            "a root that reads gone is never pruned"
        );
    }
}
