//! The library: a shared catalog entity over the promoted library service,
//! and the dockable panel that browses it. The catalog owns the app's library
//! database and only ever hands out the in-memory projection, per the library
//! service boundary. Panels are views over the shared catalog with their own
//! search config, so a duplicated panel filters independently. Double
//! clicking a track queues it straight on the shared player; single clicks
//! select, and the selection publishes app-wide for panels that display it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    div, img, prelude::*, px, svg, AnyElement, App, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, ObjectFit, PathPromptOptions,
    ScrollStrategy, SharedString, Stateful, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState};
use gpui_component::{Icon, Side, Sizable, Size};
use rox_dock::{Panel, PanelEvent, PanelInfo, PanelState, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::projection::{Projection, SortKey};
use rox_library::rusqlite::Connection;
use rox_library::scanner::{self, ScanSummary};
use rox_library::store;
use rox_viz::analysis::{log_bands, Analyzer, FFT_SIZE};
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::palette::PanelTheme;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState};
use crate::panel_settings;
use crate::search::{SearchBox, SearchEvent};
use crate::thumbs::Thumb;

/// Play from a double-clicked row: at most this many tracks are queued
/// behind it. The quick-play modal caps its queue the same way.
pub(crate) const QUEUE_CAP: usize = 1000;

/// How far page up and page down step the keyboard cursor.
const PAGE_ROWS: isize = 25;

/// Typing pauses longer than this restart the type-ahead buffer.
const TYPE_AHEAD: Duration = Duration::from_millis(1000);

/// The catalog changed: a scan finished or the projection reloaded. Panels
/// subscribe and refresh their views.
pub enum LibraryEvent {
    Updated,
}

/// How often the UI samples a running scan's progress.
const SCAN_POLL: Duration = Duration::from_millis(100);

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
}

/// The shared catalog entity. Owns the database and the projection; every
/// library panel reads it, none of them own it.
pub struct Library {
    db_path: PathBuf,
    /// UI-side connection for id -> path lookups; scans and projection loads
    /// open their own connections on the background executor.
    conn: Option<Connection>,
    projection: Option<Arc<Projection>>,
    /// The canonical browse order: album artist, album, track number.
    order: Arc<Vec<u32>>,
    /// The folders scans read from, in the order they were added,
    /// persisted in settings. Empty until a folder has been opened.
    scan_roots: Vec<PathBuf>,
    /// Set while a scan or projection load runs in the background.
    busy: Option<SharedString>,
    /// The running scan's progress, while one runs; the handle abort
    /// reaches through.
    scan: Option<Arc<ScanProgress>>,
    status: SharedString,
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
            status,
        };
        if this.conn.is_some() {
            this.reload(Refresh::Load, cx);
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
        self.reload(Refresh::Remove(root.to_path_buf()), cx);
    }

    /// Write the folder list through the settings file.
    fn persist_roots(&self) {
        let roots = self.scan_roots.clone();
        crate::settings::Settings::update(move |s| s.library_roots = roots);
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
        });
        let progress = Arc::new(ScanProgress::default());
        if matches!(refresh, Refresh::Scan(_)) {
            self.scan = Some(progress.clone());
            self.poll_scan(progress.clone(), cx);
        }
        let db_path = self.db_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load(&db_path, refresh, &progress) })
                .await;
            this.update(cx, |this, cx| {
                this.busy = None;
                this.scan = None;
                match result {
                    Ok((projection, order, summary)) => {
                        this.status = status_line(projection.len(), summary.as_ref()).into();
                        this.projection = Some(Arc::new(projection));
                        this.order = Arc::new(order);
                    }
                    Err(e) => this.status = format!("library: {e}").into(),
                }
                cx.emit(LibraryEvent::Updated);
                cx.notify();
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

/// One column the library can show: its stable key, header label, default
/// width, and whether it renders right-aligned. The registry order is the
/// default display order; the default visible set is marked per entry.
struct ColumnDef {
    key: &'static str,
    label: &'static str,
    default_width: f32,
    right: bool,
    /// Shown when a panel has no saved column layout.
    default_on: bool,
    sort: SortKey,
}

/// Every column the library knows how to draw. Adding a column is one line
/// here plus its arm in [`TrackTable::render_td`].
const COLUMNS: &[ColumnDef] = &[
    ColumnDef { key: "track", label: "#", default_width: 44., right: true, default_on: false, sort: SortKey::TrackNo },
    ColumnDef { key: "title", label: "title", default_width: 420., right: false, default_on: true, sort: SortKey::Title },
    ColumnDef { key: "artist", label: "artist", default_width: 220., right: false, default_on: true, sort: SortKey::Artist },
    ColumnDef { key: "album_artist", label: "album artist", default_width: 220., right: false, default_on: false, sort: SortKey::AlbumArtist },
    ColumnDef { key: "album", label: "album", default_width: 220., right: false, default_on: true, sort: SortKey::Album },
    ColumnDef { key: "genre", label: "genre", default_width: 140., right: false, default_on: false, sort: SortKey::Genre },
    ColumnDef { key: "year", label: "year", default_width: 56., right: true, default_on: false, sort: SortKey::Year },
    ColumnDef { key: "codec", label: "codec", default_width: 64., right: false, default_on: false, sort: SortKey::Codec },
    ColumnDef { key: "bitrate", label: "kbps", default_width: 64., right: true, default_on: false, sort: SortKey::Bitrate },
    ColumnDef { key: "duration", label: "time", default_width: 64., right: true, default_on: true, sort: SortKey::Duration },
];

/// The registry entry for a key.
fn column_def(key: &str) -> Option<&'static ColumnDef> {
    COLUMNS.iter().find(|c| c.key == key)
}

/// One shown column: its registry key and current width. The order of the
/// vec is the display order, so this carries visibility, order, and width
/// together. An empty layout means the registry's default set.
#[derive(Clone, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub key: String,
    pub width: f32,
}

/// The registry's default visible columns, in registry order.
fn default_layout() -> Vec<ColumnSpec> {
    COLUMNS
        .iter()
        .filter(|c| c.default_on)
        .map(|c| ColumnSpec {
            key: c.key.to_string(),
            width: c.default_width,
        })
        .collect()
}

/// The row height for the track list. Compact packs a large library
/// tight, comfortable gives each row room; both persist per panel.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Compact,
    Comfortable,
}

/// How the canonical browse order breaks into album groups (keyed by
/// album artist and album). Compact spends one row per break, expanded
/// two: the album artist and year, then the album with the group's track
/// count and total time. Searching or sorting by a column always renders
/// flat, whatever this says.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Headers {
    Off,
    Compact,
    #[default]
    Expanded,
}

impl Density {
    fn size(self) -> Size {
        match self {
            Density::Compact => Size::Small,
            Density::Comfortable => Size::Large,
        }
    }
}

/// The panel's per-view config: what a saved layout restores, and the
/// schema a future per-panel settings menu edits. One struct serves both,
/// so new knobs land here.
#[derive(Default, Serialize, Deserialize)]
pub struct LibraryConfig {
    #[serde(default)]
    pub query: String,
    /// The track list's row height.
    #[serde(default)]
    pub density: Density,
    /// How the canonical order shows its group breaks.
    #[serde(default)]
    pub headers: Headers,
    /// The shown columns in display order, each with its width. Empty
    /// restores the registry default set. Named apart from the old
    /// index-keyed `columns` field so pre-registry layouts drop their
    /// widths quietly instead of failing the whole config.
    #[serde(default)]
    pub column_layout: Vec<ColumnSpec>,
    /// The sorted column's registry key. None browses the canonical
    /// album artist, album, track order.
    #[serde(default)]
    pub sort_key: Option<String>,
    #[serde(default)]
    pub sort_desc: bool,
    /// The view row at the top of the viewport, so a relaunch reopens the
    /// list where it was left. An index, not pixels: it survives a density
    /// change, and drifts at most a group's headers if the catalog shifts.
    #[serde(default)]
    pub scroll_row: usize,
    /// Scroll to the playing row when the track changes.
    #[serde(default)]
    pub follow_playing: bool,
    /// Glide there instead of jumping.
    #[serde(default)]
    pub smooth_follow: bool,
    /// The panel's palette override.
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
}

/// Build the table columns from a saved layout (or the default set),
/// marking the active sort's direction on its column. Unknown keys in a
/// hand-edited layout are skipped.
fn track_columns(layout: &[ColumnSpec], sort: &Option<(SharedString, bool)>) -> Vec<Column> {
    let specs = if layout.is_empty() {
        default_layout()
    } else {
        layout.to_vec()
    };
    specs
        .iter()
        .filter_map(|spec| {
            let def = column_def(&spec.key)?;
            let state = match sort {
                Some((k, desc)) if k.as_ref() == def.key => {
                    if *desc {
                        ColumnSort::Descending
                    } else {
                        ColumnSort::Ascending
                    }
                }
                _ => ColumnSort::Default,
            };
            let column = Column::new(def.key, def.label).width(px(spec.width)).sort(state);
            Some(if def.right { column.text_right() } else { column })
        })
        .collect()
}

/// Map a column key to the projection's sort key.
fn sort_key(key: &str) -> Option<SortKey> {
    column_def(key).map(|def| def.sort)
}

/// One display row of the track list: a track from the projection, or a
/// line of the group header opening the artist/album run that follows it.
/// Headers are presentation of the canonical order only - search hits and
/// column sorts render flat - and they live in the same index space as
/// tracks, so the virtualized table scrolls them like any row. The table
/// draws every row one fixed height, so the expanded header block is two
/// rows, each drawing its own line.
#[derive(Clone, Copy, PartialEq)]
enum Row {
    Track(u32),
    /// The group's name line, indexing [`TrackTable::groups`].
    Header(u32),
    /// The group's album and stats line under an expanded header.
    Meta(u32),
}

/// One artist/album group of the current view: what its header rows draw.
/// The name, year, and genre resolve through the first track.
struct Group {
    first: u32,
    tracks: u32,
    total_ms: u64,
    /// The group's codec symbol while every track agrees; None once two
    /// differ, and the meta line drops it.
    codec: Option<u32>,
    /// The bitrate spread over tracks that carry one, in kbps; both 0 when
    /// none does.
    min_kbps: u16,
    max_kbps: u16,
    /// The first track's path, what the header tile's thumbnail loads by.
    /// Resolved through the store once, on the group's first paint; the
    /// inner None is a track the store no longer knows.
    art: Option<Option<PathBuf>>,
}

/// The canonical order with a header block opening every album run: one
/// row compact, name and stats rows expanded. Groups break on the album
/// artist, not the track artist, so a compilation stays one run with its
/// per-track artists inside. Breaks compare interned symbols and the
/// stats are two integer sums, so the walk stays cheap and runs once per
/// view swap, never while scrolling.
fn group_rows(order: &[u32], projection: &Projection, expanded: bool) -> (Vec<Row>, Vec<Group>) {
    let mut rows = Vec::with_capacity(order.len() + order.len() / 8);
    let mut groups: Vec<Group> = Vec::new();
    let mut last = None;
    for &row in order {
        let key = (
            projection.album_artist[row as usize],
            projection.album[row as usize],
        );
        if last != Some(key) {
            let g = groups.len() as u32;
            groups.push(Group {
                first: row,
                tracks: 0,
                total_ms: 0,
                codec: Some(projection.codec[row as usize]),
                min_kbps: 0,
                max_kbps: 0,
                art: None,
            });
            rows.push(Row::Header(g));
            if expanded {
                rows.push(Row::Meta(g));
            }
            last = Some(key);
        }
        let group = groups.last_mut().unwrap();
        group.tracks += 1;
        group.total_ms += projection.duration_ms[row as usize] as u64;
        if group.codec != Some(projection.codec[row as usize]) {
            group.codec = None;
        }
        let kbps = projection.bitrate_kbps[row as usize];
        if kbps > 0 {
            group.min_kbps = if group.min_kbps == 0 {
                kbps
            } else {
                group.min_kbps.min(kbps)
            };
            group.max_kbps = group.max_kbps.max(kbps);
        }
        rows.push(Row::Track(row));
    }
    (rows, groups)
}

/// A group's codec and bitrate stat: "mp3 320 kbps" when everything
/// agrees, the kbps a range when tracks spread, either half alone when
/// the other is mixed or missing, empty when both are.
fn group_quality(group: &Group, projection: &Projection) -> String {
    let codec = group
        .codec
        .map(|sym| projection.codecs.strings[sym as usize].as_str())
        .unwrap_or("");
    let kbps = match (group.min_kbps, group.max_kbps) {
        (0, _) => String::new(),
        (min, max) if min == max => format!("{min} kbps"),
        (min, max) => format!("{min}-{max} kbps"),
    };
    match (codec.is_empty(), kbps.is_empty()) {
        (false, false) => format!("{codec} {kbps}"),
        (false, true) => codec.to_string(),
        _ => kbps,
    }
}

/// The playing mark's band count and analysis range: low, mid, high over
/// roughly the musical spectrum, log-split like the spectrum panel's bars.
const PULSE_BARS: usize = 3;
const PULSE_LO_HZ: f32 = 60.0;
const PULSE_HI_HZ: f32 = 10_000.0;

/// The dB window and per-second smoothing the levels ease through, the
/// spectrum panel's numbers: bands jump up fast and fall slowly, so beats
/// read as beats instead of flicker.
const PULSE_FLOOR_DB: f32 = -66.0;
const PULSE_MAX_DB: f32 = -12.0;
const PULSE_ATTACK: f32 = 40.0;
const PULSE_RELEASE: f32 = 10.0;

/// How long the feed may sit still before it reads as stopped audio rather
/// than the gap between pump ticks, the spectrum panel's number: between
/// ticks the bars hold their targets instead of dipping toward silence.
const PULSE_SILENT_AFTER: f32 = 0.15;

/// The mark's window size: the analyzer default, bass bins enough for
/// three wide bands while reacting twice as fast as the largest window.
const PULSE_FFT: usize = FFT_SIZE;

/// The playing mark's audio tap: the spectrum panel's analysis at glyph
/// scale. Three log bands over the player's PCM feed become the three bar
/// levels; once the feed sits still past [`PULSE_SILENT_AFTER`] (paused,
/// stopped) they fall to silence. Built lazily and boxed, so a table that
/// never shows the playing row doesn't carry the FFT buffers (they run
/// ~70KB).
struct Pulse {
    analyzer: Analyzer,
    mono: [f32; PULSE_FFT],
    last_written: u64,
    last_tick: Option<Instant>,
    sample_rate: u32,
    bands: Vec<(usize, usize)>,
    /// What each bar eases toward: refreshed per analysis, held between
    /// them, zeroed once the feed reads as stopped.
    targets: [f32; PULSE_BARS],
    /// When the feed last carried new audio.
    last_fresh: Option<Instant>,
    levels: [f32; PULSE_BARS],
}

impl Pulse {
    fn new() -> Self {
        Pulse {
            analyzer: Analyzer::new(PULSE_FFT),
            mono: [0.0; PULSE_FFT],
            last_written: 0,
            last_tick: None,
            sample_rate: 0,
            bands: Vec::new(),
            targets: [0.0; PULSE_BARS],
            last_fresh: None,
            levels: [0.0; PULSE_BARS],
        }
    }

    /// One tick, the spectrum's step at three bands: pull the newest
    /// window off the feed when it moved and refresh each band's target,
    /// hold the targets between pump ticks, let the levels fall once the
    /// feed reads as stopped.
    fn step(&mut self, feed: &AudioFeed) -> [f32; PULSE_BARS] {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32().min(0.1))
            .unwrap_or(1.0 / 60.0);
        self.last_tick = Some(now);

        let rate = feed.sample_rate();
        if rate != self.sample_rate {
            self.sample_rate = rate;
            self.bands = log_bands(PULSE_BARS, PULSE_LO_HZ, PULSE_HI_HZ, rate, PULSE_FFT / 2);
        }
        let written = feed.written();
        let fresh = written != self.last_written && feed.latest_mono(&mut self.mono) == PULSE_FFT;
        self.last_written = written;
        let mags = fresh.then(|| self.analyzer.magnitudes(&self.mono));
        if fresh {
            self.last_fresh = Some(now);
        }
        let stopped = self
            .last_fresh
            .is_none_or(|t| (now - t).as_secs_f32() > PULSE_SILENT_AFTER);

        for (i, level) in self.levels.iter_mut().enumerate() {
            if let Some(mags) = mags {
                let (lo, hi) = self.bands[i];
                let mut peak = 0.0f32;
                for &m in &mags[lo..hi] {
                    peak = peak.max(m);
                }
                let db = 20.0 * (peak + 1e-9).log10();
                self.targets[i] =
                    ((db - PULSE_FLOOR_DB) / (PULSE_MAX_DB - PULSE_FLOOR_DB)).clamp(0.0, 1.0);
            } else if stopped {
                self.targets[i] = 0.0;
            }
            let target = self.targets[i];
            let rate = if target > *level {
                PULSE_ATTACK
            } else {
                PULSE_RELEASE
            };
            *level += (target - *level) * (rate * dt).min(1.0);
        }
        self.levels
    }
}

/// The playing mark: three bars riding the playing audio, low, mid, and
/// high bands left to right. The floor keeps visible stubs through quiet
/// passages and while paused, where the levels settle to rest. Each bar
/// hangs absolutely off the box floor: flex end-alignment drifted inside
/// the table cell, moving the bases with the levels.
fn playing_bars(levels: [f32; PULSE_BARS]) -> Div {
    const SPAN: f32 = 10.;
    const BAR_W: f32 = 2.;
    const GAP: f32 = 1.;
    let mut bars = div()
        .relative()
        .flex_none()
        .w(px(PULSE_BARS as f32 * (BAR_W + GAP) - GAP))
        .h(px(SPAN));
    for (ix, level) in levels.into_iter().enumerate() {
        bars = bars.child(
            div()
                .absolute()
                .bottom_0()
                .left(px(ix as f32 * (BAR_W + GAP)))
                .w(px(BAR_W))
                .h(px(SPAN * (0.18 + 0.82 * level)))
                .rounded(px(1.))
                .bg(palette::accent()),
        );
    }
    bars
}

/// The table delegate: the column set and the rows one panel displays.
/// Lives inside the panel's `TableState`; the panel swaps `view` when the
/// query or the catalog changes.
struct TrackTable {
    state: AppState,
    /// The owning panel, for dispatching context menu actions back to it.
    panel: WeakEntity<LibraryPanel>,
    /// Rows currently displayed: the canonical order broken by group
    /// headers, or flat search hits, re-sorted when a column sort is
    /// active.
    view: Arc<Vec<Row>>,
    /// The current view's groups, what header rows index; empty when the
    /// view renders flat. Swapped together with `view`, always.
    groups: Vec<Group>,
    /// How the canonical order breaks into groups. Lives on the delegate
    /// because the view computation reads it; the panel reads it back for
    /// the layout dump.
    headers: Headers,
    /// The panel's row height, mirrored here because the header tile is
    /// sized in rows and the widget's size lives outside the delegate.
    density: Density,
    /// Selected rows as indices into `view`, track rows only - headers
    /// take no selection. Cleared when the view swaps, since the indices
    /// point elsewhere afterwards.
    selected: HashSet<usize>,
    /// Where the next shift-click extends from: the last plain or
    /// toggle-clicked row.
    anchor: Option<usize>,
    /// The keyboard cursor: where arrows move from and enter plays from.
    /// Follows clicks, so keys and mouse hand off mid-browse.
    cursor: Option<usize>,
    columns: Vec<Column>,
    /// The active sort: a column key and whether it descends. None is the
    /// canonical order. Lives on the delegate because the header click
    /// lands here; the panel reads it back for the layout dump.
    sort: Option<(SharedString, bool)>,
    /// The playing track's id, resolved once per track change by the
    /// panel, and its row in the current view when the view holds it.
    playing_id: Option<i64>,
    playing_row: Option<usize>,
    /// The playing mark's band meter, born the first time the playing row
    /// renders under a live session.
    pulse: Option<Box<Pulse>>,
}

impl TrackTable {
    /// The track a view row holds; None for a header row.
    fn track_at(&self, ix: usize) -> Option<u32> {
        match self.view.get(ix) {
            Some(&Row::Track(row)) => Some(row),
            _ => None,
        }
    }

    /// The nearest track row from `ix` heading `forward`, bouncing off the
    /// ends; None only when the view holds no tracks. Cursor moves route
    /// through this, so the cursor never lands on a header.
    fn snap_to_track(&self, ix: usize, forward: bool) -> Option<usize> {
        let len = self.view.len();
        if len == 0 {
            return None;
        }
        let ix = ix.min(len - 1);
        let ahead = || (ix..len).find(|&i| self.track_at(i).is_some());
        let behind = || (0..=ix).rev().find(|&i| self.track_at(i).is_some());
        if forward {
            ahead().or_else(behind)
        } else {
            behind().or_else(ahead)
        }
    }

    /// The edge length of an expanded header's cover tile: the full
    /// two-row block, so the art squares off exactly against the text.
    fn tile_side(&self) -> gpui::Pixels {
        self.density.size().table_row_height() * 2.
    }

    /// One half of an expanded header's cover tile. The table draws every
    /// row one fixed height with no spanning cell, so each of the block's
    /// two rows clips its own half of a two-row-tall square: the name row
    /// the top (`bottom` false), the meta row the bottom. Same image
    /// handle both times, so gpui decodes it once. Pending and missing
    /// wear the same quiet placeholder, so a landing cover fills the tile
    /// without shifting the text beside it.
    fn group_tile(
        &mut self,
        g: u32,
        bottom: bool,
        cx: &mut Context<TableState<Self>>,
    ) -> AnyElement {
        let path = match self
            .groups
            .get(g as usize)
            .and_then(|group| group.art.clone())
        {
            Some(path) => path,
            None => {
                let id = {
                    let library = self.state.library.read(cx);
                    self.groups.get(g as usize).and_then(|group| {
                        library
                            .projection()
                            .map(|projection| projection.db_id[group.first as usize])
                    })
                };
                let path = id
                    .and_then(|id| self.state.library.read(cx).paths_for(&[id]).ok())
                    .and_then(|mut paths| paths.pop());
                if let Some(group) = self.groups.get_mut(g as usize) {
                    group.art = Some(path.clone());
                }
                path
            }
        };
        let thumb = match path {
            Some(path) => self
                .state
                .thumbs
                .update(cx, |thumbs, cx| thumbs.get(&path, cx)),
            None => Thumb::Missing,
        };
        let side = self.tile_side();
        let content: AnyElement = match thumb {
            Thumb::Ready(image) => img(image)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(icons::MUSIC)
                        .size(px(16.))
                        .text_color(palette::text_faint()),
                )
                .into_any_element(),
        };
        div()
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(side)
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .w(side)
                    .h(side)
                    .map(|d| if bottom { d.bottom_0() } else { d.top_0() })
                    .child(content),
            )
            .into_any_element()
    }

    /// The group's name line: compact packs the album artist, album, and
    /// year into its one row; expanded gives the album artist the line,
    /// larger, the year on the right, hands the album to the meta line
    /// under it, and opens the two-row cover tile the meta line closes.
    fn render_group_header(
        &mut self,
        row_ix: usize,
        g: u32,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let expanded = self.headers == Headers::Expanded;
        let tile = expanded.then(|| self.group_tile(g, false, cx));
        let indent = self.tile_side() + tokens::SPACE_SM;
        let (artist, album, year) = match (
            self.groups.get(g as usize),
            self.state.library.read(cx).projection(),
        ) {
            (Some(group), Some(projection)) => {
                let v = projection.resolve(group.first);
                // Rows migrated from before the album artist column carry
                // an empty one until a rescan re-reads their tags; the
                // first track's artist stands in rather than "unknown".
                let name = if v.album_artist.is_empty() {
                    v.artist
                } else {
                    v.album_artist
                };
                (name.to_string(), v.album.to_string(), v.year)
            }
            _ => Default::default(),
        };
        let unknown = artist.is_empty() && (expanded || album.is_empty());
        let artist = (!artist.is_empty()).then(|| SharedString::from(artist));
        let album = (!expanded && !album.is_empty()).then(|| SharedString::from(album));
        div()
            .id(("row", row_ix))
            .bg(palette::bg_elevated())
            // The expanded block reads as one: no border between its name
            // and meta lines. The width stays, so rows keep their height.
            .when(expanded, |d| d.border_color(gpui::transparent_black()))
            .when_some(tile, |d, tile| d.child(tile))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .px(tokens::SPACE_SM)
                    // Clear of the cover tile, which spans the block.
                    .when(expanded, |d| d.pl(indent))
                    .overflow_hidden()
                    .when(unknown, |d| {
                        d.child(
                            div()
                                .flex_1()
                                .text_color(palette::text_muted())
                                .child("unknown"),
                        )
                    })
                    .when_some(artist, |d, artist| {
                        d.child(
                            div()
                                .truncate()
                                .text_color(palette::text_bright())
                                .map(|d| {
                                    if expanded {
                                        d.flex_1().text_lg()
                                    } else {
                                        d.flex_none()
                                    }
                                })
                                .child(artist),
                        )
                    })
                    .when_some(album, |d, album| {
                        d.child(
                            div()
                                .truncate()
                                .text_color(palette::text_secondary())
                                .child(album),
                        )
                    })
                    .when(year != 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_color(if expanded {
                                    palette::text_secondary()
                                } else {
                                    palette::text_muted()
                                })
                                .child(fmt_num(year)),
                        )
                    }),
            )
    }

    /// The expanded header's second line: the album, then the group's
    /// genre, codec and bitrate, track count, and total time on the
    /// right, beside the cover tile's bottom half.
    fn render_group_meta(
        &mut self,
        row_ix: usize,
        g: u32,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let tile = self.group_tile(g, true, cx);
        let indent = self.tile_side() + tokens::SPACE_SM;
        let (album, genre, quality, tracks, total_ms) = match (
            self.groups.get(g as usize),
            self.state.library.read(cx).projection(),
        ) {
            (Some(group), Some(projection)) => {
                let v = projection.resolve(group.first);
                (
                    v.album.to_string(),
                    v.genre.to_string(),
                    group_quality(group, projection),
                    group.tracks,
                    group.total_ms,
                )
            }
            _ => Default::default(),
        };
        let mut stats = Vec::new();
        if !genre.is_empty() {
            stats.push(genre);
        }
        if !quality.is_empty() {
            stats.push(quality);
        }
        stats.push(if tracks == 1 {
            "1 track".to_string()
        } else {
            format!("{tracks} tracks")
        });
        stats.push(fmt_total(total_ms));
        div()
            .id(("row", row_ix))
            .bg(palette::bg_elevated())
            .child(tile)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .px(tokens::SPACE_SM)
                    // Clear of the cover tile, which spans the block.
                    .pl(indent)
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_color(palette::text_secondary())
                            .child(SharedString::from(album)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(palette::text_muted())
                            .child(SharedString::from(stats.join(" | "))),
                    ),
            )
    }

    /// The next row whose leading text starts with the typed prefix, from
    /// the cursor on, wrapping. The leading text follows the active sort:
    /// its column when it has text, the album artist for the canonical
    /// order (what the grouping runs on), the track artist for sorts
    /// without text of their own (duration). ASCII-insensitive, like
    /// search.
    fn find_prefix(&self, prefix: &str, include_current: bool, cx: &App) -> Option<usize> {
        let library = self.state.library.read(cx);
        let projection = library.projection()?;
        let len = self.view.len();
        if len == 0 {
            return None;
        }
        let field = self.sort.as_ref().map(|(key, _)| key.as_ref());
        let start = match self.cursor {
            Some(cursor) if include_current => cursor,
            Some(cursor) => cursor + 1,
            None => 0,
        };
        (0..len).map(|i| (start + i) % len).find(|&ix| {
            let Some(row) = self.track_at(ix) else {
                return false;
            };
            let v = projection.resolve(row);
            let text = match field {
                Some("title") => v.title,
                Some("album") => v.album,
                Some("album_artist") | None => v.album_artist,
                Some("codec") => v.codec,
                Some(_) => v.artist,
            };
            text.get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        })
    }

    /// Re-locate the playing track in the current view: one scan per view
    /// swap or track change, never per frame.
    fn locate_playing(&mut self, cx: &App) {
        let row = self.playing_id.and_then(|id| {
            let library = self.state.library.read(cx);
            let projection = library.projection()?;
            self.view
                .iter()
                .position(|&row| matches!(row, Row::Track(r) if projection.db_id[r as usize] == id))
        });
        self.playing_row = row;
    }

    /// The rows this panel shows: the canonical order or search hits, put
    /// through the active column sort when one is set. Only the canonical
    /// order gets grouping headers; a query or a column sort breaks the
    /// artist/album runs the headers name, so those views render flat.
    fn compute_view(&self, query: &str, cx: &App) -> (Arc<Vec<Row>>, Vec<Group>) {
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return (Arc::new(Vec::new()), Vec::new());
        };
        let base = if query.is_empty() {
            library.order()
        } else {
            Arc::new(projection.search(query))
        };
        let active = self
            .sort
            .as_ref()
            .and_then(|(key, desc)| sort_key(key).map(|key| (key, *desc)));
        match active {
            Some((key, desc)) => (
                Arc::new(
                    projection
                        .sort_view(&base, key, desc)
                        .into_iter()
                        .map(Row::Track)
                        .collect(),
                ),
                Vec::new(),
            ),
            None if query.is_empty() && self.headers != Headers::Off => {
                let (rows, groups) =
                    group_rows(&base, projection, self.headers == Headers::Expanded);
                (Arc::new(rows), groups)
            }
            None => (
                Arc::new(base.iter().copied().map(Row::Track).collect()),
                Vec::new(),
            ),
        }
    }

    /// Append the owning panel's dropdown items to a row context menu.
    /// Called while the table entity is mid-update, so the panel's
    /// `dropdown_menu` must not read the table entity at build time (its
    /// click handlers may, they run after the update ends).
    fn panel_menu(&self, menu: PopupMenu, window: &mut Window, cx: &mut App) -> PopupMenu {
        let Some(panel) = self.panel.upgrade() else {
            return menu;
        };
        panel.update(cx, |panel, cx| panel.dropdown_menu(menu, window, cx))
    }

    /// Resolve the selected rows to db ids in view order and publish them
    /// on the shared selection.
    fn publish_selection(&self, cx: &mut App) {
        let Some(projection) = self.state.library.read(cx).projection().cloned() else {
            return;
        };
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        let ids: Vec<i64> = rows
            .iter()
            .filter_map(|&ix| self.track_at(ix))
            .map(|row| projection.db_id[row as usize])
            .collect();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
    }
}

impl TableDelegate for TrackTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.view.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    /// The header cell: the stock label plus a right-click menu that
    /// toggles the shown columns in place, the customize window's chips
    /// without the trip there. The table's own right-click menu stays a
    /// row affair; over the header it builds empty and never shows, so
    /// the two menus don't stack.
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let shown: HashSet<String> = self.columns.iter().map(|c| c.key.to_string()).collect();
        let panel = self.panel.clone();
        div()
            .size_full()
            .child(self.column(col_ix, cx).name.clone())
            .context_menu(move |mut menu, _, _| {
                for def in COLUMNS {
                    let key = def.key;
                    let panel = panel.clone();
                    menu = menu.item(
                        PopupMenuItem::new(def.label)
                            .checked(shown.contains(key))
                            .on_click(move |_, _, cx| {
                                if let Some(panel) = panel.upgrade() {
                                    panel.update(cx, |panel, cx| panel.toggle_column(key, cx));
                                }
                            }),
                    );
                }
                menu
            })
    }

    /// The header sort hook. The widget has already advanced the clicked
    /// column's cycle (canonical -> descending -> ascending) in its own
    /// column state; mirror it into the delegate's columns and swap the
    /// view here, because the table entity is mid-update and the panel's
    /// refresh path would re-enter it. The panel reads the sort back for
    /// persistence via `dump`.
    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        for (ix, column) in self.columns.iter_mut().enumerate() {
            column.sort = Some(if ix == col_ix { sort } else { ColumnSort::Default });
        }
        self.sort = match sort {
            ColumnSort::Ascending => Some((self.columns[col_ix].key.clone(), false)),
            ColumnSort::Descending => Some((self.columns[col_ix].key.clone(), true)),
            ColumnSort::Default => None,
        };
        let query = self
            .panel
            .upgrade()
            .map(|panel| panel.read(cx).query.clone())
            .unwrap_or_default();
        let (view, groups) = self.compute_view(&query, cx);
        self.view = view;
        self.groups = groups;
        // The old indices point elsewhere in the new order, same as any
        // view swap. The widget's own focus row does too, but it can only
        // be cleared once the table's update ends.
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.locate_playing(cx);
        let table = cx.entity();
        cx.defer(move |cx| {
            table.update(cx, |table, cx| table.clear_selection(cx));
        });
        cx.notify();
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        // A group header line is one full-width strip over emptied cells,
        // since the table has no row-spanning cell. It hangs off the row
        // itself, outside the horizontally scrolled cell region, so the
        // title stays put when wide column sets scroll sideways.
        match self.view.get(row_ix).copied() {
            Some(Row::Header(g)) => return self.render_group_header(row_ix, g, cx),
            Some(Row::Meta(g)) => return self.render_group_meta(row_ix, g, cx),
            _ => {}
        }
        // The same wash the widget theme paints its own focus row with, so
        // multi-selected rows read as one set. The playing row wears a
        // fainter cut of it, so a selection still reads over the mark.
        let selected = self.selected.contains(&row_ix);
        div()
            .id(("row", row_ix))
            .cursor_pointer()
            .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
            .when(self.playing_row == Some(row_ix) && !selected, |d| {
                d.bg(palette::alpha(palette::accent(), 0x12))
            })
    }

    /// The row context menu. A right click inside the selection acts on the
    /// whole set; outside it, the click reselects just that row first, so
    /// the menu always acts on what is highlighted. The panel's own menu
    /// rides along after the track actions: the panel body hands its
    /// right-click to the table (`content_context_menu`), so this menu is
    /// the only one a click over the list opens, and it must not dead-end
    /// at Play. Headers get the panel menu alone; acting by group waits
    /// until headers are more than presentation.
    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if self.track_at(row_ix).is_none() {
            return self.panel_menu(menu, window, cx);
        }
        if !self.selected.contains(&row_ix) {
            self.selected = HashSet::from([row_ix]);
            self.anchor = Some(row_ix);
            self.publish_selection(cx);
            cx.notify();
        }
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        let panel = self.panel.clone();
        let menu = if rows.len() > 1 {
            menu.item(
                PopupMenuItem::new(format!("Play {} Tracks", rows.len()))
                    .icon(Icon::default().path(icons::PLAY))
                    .on_click(move |_, _, cx| {
                        if let Some(panel) = panel.upgrade() {
                            panel.update(cx, |panel, cx| panel.play_rows(rows.clone(), cx));
                        }
                    }),
            )
        } else {
            menu.item(
                PopupMenuItem::new("Play")
                    .icon(Icon::default().path(icons::PLAY))
                    .on_click(move |_, _, cx| {
                        if let Some(panel) = panel.upgrade() {
                            panel.update(cx, |panel, cx| panel.play_from(row_ix, cx));
                        }
                    }),
            )
        };
        self.panel_menu(menu.separator(), window, cx)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Header rows draw in render_tr; their cells stay empty.
        let Some(row) = self.track_at(row_ix) else {
            return div().into_any_element();
        };
        let Some(projection) = self.state.library.read(cx).projection().cloned() else {
            return div().into_any_element();
        };
        let v = projection.resolve(row);
        let playing = self.playing_row == Some(row_ix);
        let cell = div().truncate();
        let cell = match self.columns[col_ix].key.as_ref() {
            "track" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.track_no)),
            "title" => cell
                .when(playing, |d| d.text_color(palette::accent()))
                .child(SharedString::from(v.title.to_string())),
            "artist" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.artist.to_string())),
            "album_artist" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.album_artist.to_string())),
            "album" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.album.to_string())),
            "genre" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.genre.to_string())),
            "year" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.year)),
            "codec" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(v.codec.to_string())),
            "bitrate" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.bitrate_kbps)),
            "duration" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(fmt_ms(v.duration_ms))),
            _ => cell,
        };
        // The playing mark rides the right edge of the leading cell,
        // whichever column that is, so it survives any column layout and
        // the text keeps its place. Repainting by frame while a session
        // runs follows the transport's poll: it is what steps the meter,
        // and it never stops on pause, because pause flips on the audio
        // thread and notifies nobody - the feed just stops moving and the
        // bars fall to rest.
        if playing && col_ix == 0 {
            let (active, feed) = {
                let player = self.state.player.read(cx);
                (player.is_active(), player.feed())
            };
            let levels = if active {
                let table = cx.entity_id();
                window.on_next_frame(move |_, cx| cx.notify(table));
                self.pulse.get_or_insert_with(|| Box::new(Pulse::new())).step(&feed)
            } else {
                [0.0; PULSE_BARS]
            };
            return div()
                .flex()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(cell.flex_1().min_w_0())
                .child(playing_bars(levels))
                .into_any_element();
        }
        cell.into_any_element()
    }

    /// Keep the delegate's columns in the widget's order: the table calls
    /// this before it reorders its own col_groups the same way, so cell
    /// rendering (indexed by the visual column) stays aligned. The layout
    /// dump reads the new order back off `columns`.
    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        if col_ix >= self.columns.len() || to_ix >= self.columns.len() {
            return;
        }
        let column = self.columns.remove(col_ix);
        self.columns.insert(to_ix, column);
    }

    /// No rows and a non-empty query means no hits; keep the body quiet
    /// like the old flat list did. The no-library case never reaches here,
    /// the panel renders its own empty state instead of the table.
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
}

/// One browse view over the shared catalog: its own search query and row
/// order, duplicable and poppable like any panel.
pub struct LibraryPanel {
    state: AppState,
    /// The table over the current view; the delegate holds the rows.
    table: Entity<TableState<TrackTable>>,
    query: String,
    /// The panel's own focus, what the dock focuses on tab activation. Kept
    /// apart from the search input's focus so activating the tab does not
    /// put every keystroke in the query, and so the playback key bindings
    /// (scoped out of SearchInput) stay live.
    focus: FocusHandle,
    /// The query editor, the shared search box; `query` mirrors its value
    /// via change events.
    search: Entity<SearchBox>,
    /// A panel-local error (a failed play), shown until the catalog updates.
    error: Option<SharedString>,
    /// The playing track's path, the change detector: the player notifies
    /// every pump tick, so everything up to this compare stays cheap.
    playing_path: Option<PathBuf>,
    /// The type-ahead buffer and when it last grew; a pause starts over.
    type_ahead: String,
    type_ahead_at: Option<std::time::Instant>,
    /// The saved scroll row waiting for rows to restore against. The
    /// catalog loads after the panel builds, so the first non-empty view
    /// consumes this; None once applied.
    restore_scroll: Option<usize>,
    /// Scroll to the playing row when the track changes, and whether to
    /// glide there instead of jumping.
    follow_playing: bool,
    smooth_follow: bool,
    /// The view row the follow glide is headed to; stepped every frame in
    /// `body` and cleared on arrival.
    glide_to: Option<usize>,
    /// The last glide tick, its dt.
    glide_tick: Instant,
    /// The track list's row height, applied on the table each render.
    density: Density,
    /// The panel's palette override, live for the render and carried by
    /// the config dump like every other view knob.
    theme: PanelTheme,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Watches the hosting tab panel: whether this panel is solo decides
    /// where the toolbar renders, so membership changes must re-render.
    _tabs_changed: Option<Subscription>,
    _library_changed: Subscription,
    _table_events: Subscription,
    _search_events: Subscription,
    _player_changed: Subscription,
    _thumbs_changed: Subscription,
}

impl LibraryPanel {
    pub fn new(
        state: AppState,
        config: LibraryConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut LibraryPanel, _, _: &LibraryEvent, cx| {
                this.error = None;
                this.refresh_view(cx);
                // The catalog loads after a restored track starts, so the
                // launch's follow waits for this first rebuild; rescans
                // re-land on the playing row the same way.
                if this.follow_playing {
                    this.follow_playing(cx);
                }
                cx.notify();
                this.refresh_title_bar(cx);
            },
        );
        let sort = config
            .sort_key
            .map(|key| (SharedString::from(key), config.sort_desc));
        let delegate = TrackTable {
            state: state.clone(),
            panel: cx.weak_entity(),
            view: Arc::new(Vec::new()),
            groups: Vec::new(),
            headers: config.headers,
            density: config.density,
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            columns: track_columns(&config.column_layout, &sort),
            sort,
            playing_id: None,
            playing_row: None,
            pulse: None,
        };
        // Widths and order persist by column key, so a drag survives a
        // layout save; the delegate mirrors the widget's reorder.
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .col_movable(true)
                .col_selectable(false)
        });
        let _table_events = cx.subscribe_in(&table, window, Self::on_table_event);
        let search = cx.new(|cx| SearchBox::new("search", &config.query, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        let _player_changed = cx.observe(&state.player, |this: &mut LibraryPanel, _, cx| {
            this.sync_playing(cx)
        });
        // A thumbnail landing repaints the rows; the panel itself has
        // nothing to recompute.
        let _thumbs_changed = cx.observe(&state.thumbs, |this: &mut LibraryPanel, _, cx| {
            this.table.update(cx, |_, cx| cx.notify());
        });
        let mut this = LibraryPanel {
            state,
            table,
            query: config.query,
            focus: cx.focus_handle(),
            search,
            error: None,
            playing_path: None,
            type_ahead: String::new(),
            type_ahead_at: None,
            restore_scroll: (config.scroll_row > 0).then_some(config.scroll_row),
            follow_playing: config.follow_playing,
            smooth_follow: config.smooth_follow,
            glide_to: None,
            glide_tick: Instant::now(),
            density: config.density,
            theme: config.theme,
            tab_panel: None,
            _tabs_changed: None,
            _library_changed,
            _table_events,
            _search_events,
            _player_changed,
            _thumbs_changed,
        };
        this.refresh_view(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Follow the player: on a track change, resolve the playing path to
    /// its id (one store lookup) and re-locate its row in the view.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if path == self.playing_path {
            return;
        }
        self.playing_path = path;
        let id = self
            .playing_path
            .as_ref()
            .and_then(|path| self.state.library.read(cx).id_for(path));
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.playing_id = id;
            delegate.locate_playing(cx);
            cx.notify();
        });
        if self.follow_playing {
            self.follow_playing(cx);
        }
    }

    /// Scroll the playing row into view: a glide when smooth is on, the
    /// jump otherwise.
    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        if self.smooth_follow {
            if let Some(row) = self.table.read(cx).delegate().playing_row {
                self.glide_to = Some(row);
                cx.notify();
            }
        } else {
            self.jump_to_playing(cx);
        }
    }

    /// Browse from the keyboard while the panel itself is focused: arrows
    /// move a cursor, shift extends from the click path's anchor, enter
    /// plays, and plain typing jumps to the next match in the leading
    /// column. With the search box focused these stay out of the way: in
    /// the solo and popped-out layouts its toolbar sits inside the panel
    /// root, so its keystrokes bubble through here.
    fn on_panel_key(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        if self.search.read(cx).is_focused(window, cx) {
            return;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        let shift = keystroke.modifiers.shift;
        match keystroke.key.as_str() {
            "up" => self.move_cursor(-1, shift, cx),
            "down" => self.move_cursor(1, shift, cx),
            "pageup" => self.move_cursor(-PAGE_ROWS, shift, cx),
            "pagedown" => self.move_cursor(PAGE_ROWS, shift, cx),
            // The edges snap inward past a leading header.
            "home" => {
                if let Some(ix) = self.table.read(cx).delegate().snap_to_track(0, true) {
                    self.set_cursor(ix, shift, cx);
                }
            }
            "end" => {
                let target = {
                    let delegate = self.table.read(cx).delegate();
                    delegate.snap_to_track(delegate.view.len().saturating_sub(1), false)
                };
                if let Some(ix) = target {
                    self.set_cursor(ix, shift, cx);
                }
            }
            "enter" => self.play_selection(cx),
            _ => {
                let Some(text) = &keystroke.key_char else {
                    return;
                };
                // Space stays the workspace's play/pause; it never starts
                // a jump, only continues one mid-phrase.
                if self.type_ahead.is_empty() && text == " " {
                    return;
                }
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// Grow or restart the type-ahead buffer and jump to its next match.
    /// A grown buffer re-tests the current row first, so refining a match
    /// stays put instead of skipping ahead.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let grown = self
            .type_ahead_at
            .is_some_and(|at| now.duration_since(at) < TYPE_AHEAD);
        if grown {
            self.type_ahead.push_str(&text);
        } else {
            self.type_ahead = text;
        }
        self.type_ahead_at = Some(now);
        let target = {
            let delegate = self.table.read(cx).delegate();
            delegate.find_prefix(&self.type_ahead, grown, cx)
        };
        if let Some(ix) = target {
            self.set_cursor(ix, false, cx);
        }
    }

    /// Put the cursor on a view row: plain selects just it, extend grows
    /// the selection from the anchor. Either way it publishes and scrolls
    /// into view.
    fn set_cursor(&mut self, ix: usize, extend: bool, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if delegate.track_at(ix).is_none() {
                return;
            }
            delegate.cursor = Some(ix);
            if extend {
                let anchor = delegate.anchor.unwrap_or(ix);
                let (lo, hi) = (anchor.min(ix), anchor.max(ix));
                // A range spanning a group break selects its tracks only.
                let range = (lo..=hi)
                    .filter(|&i| delegate.track_at(i).is_some())
                    .collect();
                delegate.selected = range;
            } else {
                delegate.selected = HashSet::from([ix]);
                delegate.anchor = Some(ix);
            }
            table.delegate().publish_selection(cx);
            table.scroll_to_row(ix, cx);
            cx.notify();
        });
    }

    /// Step the cursor; the first press with no cursor lands on the edge
    /// the step heads toward. A step landing on a header overshoots it the
    /// way it was heading, bouncing back at the ends.
    fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let target = {
            let delegate = self.table.read(cx).delegate();
            let len = delegate.view.len();
            if len == 0 {
                return;
            }
            let raw = match delegate.cursor {
                None if delta >= 0 => 0,
                None => len - 1,
                Some(cursor) => (cursor as isize + delta).clamp(0, len as isize - 1) as usize,
            };
            delegate.snap_to_track(raw, delta >= 0)
        };
        if let Some(target) = target {
            self.set_cursor(target, extend, cx);
        }
    }

    /// Enter: a multi-selection plays exactly itself, a lone cursor plays
    /// from its row in view order like a double click.
    fn play_selection(&mut self, cx: &mut Context<Self>) {
        let (mut rows, cursor) = {
            let delegate = self.table.read(cx).delegate();
            let rows: Vec<usize> = delegate.selected.iter().copied().collect();
            (rows, delegate.cursor)
        };
        rows.sort_unstable();
        if rows.len() > 1 {
            self.play_rows(rows, cx);
        } else if let Some(ix) = cursor.or_else(|| rows.first().copied()) {
            self.play_from(ix, cx);
        }
    }

    /// Scroll the playing track's row into view, when the view holds it.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            if let Some(row) = table.delegate().playing_row {
                table.scroll_to_row(row, cx);
            }
        });
    }

    fn refresh_view(&mut self, cx: &mut Context<Self>) {
        let query = self.query.clone();
        self.table.update(cx, |table, cx| {
            // Selection indices point into the old view; drop them along
            // with the widget's own focus row. The shared selection keeps
            // the last explicit pick, a view refresh is not one.
            let (view, groups) = table.delegate().compute_view(&query, cx);
            let delegate = table.delegate_mut();
            delegate.view = view;
            delegate.groups = groups;
            delegate.selected.clear();
            delegate.anchor = None;
            delegate.cursor = None;
            delegate.locate_playing(cx);
            table.clear_selection(cx);
            cx.notify();
        });
        // The saved scroll restores against the first view with rows; a
        // strict deferred scroll on the handle, so it lands on the paint
        // that shows them, even if the panel sits in a background tab
        // until then. Earlier refreshes (the empty initial load) keep it
        // pending.
        if let Some(row) = self.restore_scroll {
            if !self.table.read(cx).delegate().view.is_empty() {
                self.restore_scroll = None;
                self.table
                    .read(cx)
                    .vertical_scroll_handle
                    .scroll_to_item_strict(row, ScrollStrategy::Top);
            }
        }
    }

    fn on_table_event(
        &mut self,
        _: &Entity<TableState<TrackTable>>,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // A click selects; focus moves back to the panel so the
            // playback keys stay with the workspace, not the table. Shift
            // extends from the anchor, cmd (ctrl elsewhere) toggles, and a
            // plain click starts over. The widget also fires this for a
            // double click's first clicks, which land as a plain select.
            TableEvent::SelectRow(ix) => {
                window.focus(&self.focus);
                let ix = *ix;
                // A header row takes no selection; drop the widget's own
                // focus row so the click leaves no mark on it.
                if self.table.read(cx).delegate().track_at(ix).is_none() {
                    self.table.update(cx, |table, cx| table.clear_selection(cx));
                    return;
                }
                let modifiers = window.modifiers();
                self.table.update(cx, |table, cx| {
                    let delegate = table.delegate_mut();
                    if modifiers.shift {
                        let anchor = delegate.anchor.unwrap_or(ix);
                        let (lo, hi) = (anchor.min(ix), anchor.max(ix));
                        // Tracks only across a group break, like the
                        // keyboard's shift-extend.
                        let range = (lo..=hi)
                            .filter(|&i| delegate.track_at(i).is_some())
                            .collect();
                        delegate.selected = range;
                    } else if modifiers.secondary() {
                        if !delegate.selected.insert(ix) {
                            delegate.selected.remove(&ix);
                            // The widget put its focus row here on the way
                            // in; a toggle-off must clear that too.
                            table.clear_selection(cx);
                        }
                        table.delegate_mut().anchor = Some(ix);
                    } else {
                        delegate.selected = HashSet::from([ix]);
                        delegate.anchor = Some(ix);
                    }
                    table.delegate_mut().cursor = Some(ix);
                    table.delegate().publish_selection(cx);
                    cx.notify();
                });
            }
            // The double click is what plays, leaving single clicks free
            // to select. Headers play nothing; acting by group waits
            // until headers are more than presentation.
            TableEvent::DoubleClickedRow(ix)
                if self.table.read(cx).delegate().track_at(*ix).is_some() =>
            {
                self.play_from(*ix, cx);
            }
            // Written back into the delegate's columns: refresh() re-reads
            // them, and the layout dump persists them.
            TableEvent::ColumnWidthsChanged(widths) => {
                let widths = widths.clone();
                self.table.update(cx, |table, _| {
                    let columns = &mut table.delegate_mut().columns;
                    for (column, width) in columns.iter_mut().zip(widths) {
                        column.width = width;
                    }
                });
            }
            _ => {}
        }
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        self.state
            .library
            .update(cx, |library, cx| library.browse(cx));
    }

    /// The shown columns in display order, each with its live width, for
    /// the layout dump and for duplicates.
    fn column_specs(&self, cx: &App) -> Vec<ColumnSpec> {
        self.table
            .read(cx)
            .delegate()
            .columns
            .iter()
            .map(|column| ColumnSpec {
                key: column.key.to_string(),
                width: f32::from(column.width),
            })
            .collect()
    }

    /// The panel's live config, for the layout dump and for duplicates.
    fn config(&self, cx: &App) -> LibraryConfig {
        let sort = self.table.read(cx).delegate().sort.clone();
        LibraryConfig {
            query: self.query.clone(),
            density: self.density,
            headers: self.table.read(cx).delegate().headers,
            column_layout: self.column_specs(cx),
            sort_key: sort.as_ref().map(|(key, _)| key.to_string()),
            sort_desc: sort.is_some_and(|(_, desc)| desc),
            scroll_row: self.scroll_row(cx),
            follow_playing: self.follow_playing,
            smooth_follow: self.smooth_follow,
            theme: self.theme.clone(),
        }
    }

    /// The view row at the top of the viewport, read off the table's
    /// scroll handle. The uniform list never reports child bounds to its
    /// base handle, so the row comes from the pixel offset over the row
    /// height - the density's, the same fixed height every row renders
    /// at (the handle's own `last_item_size.item` is the viewport, not a
    /// row). A restore still pending (the panel never painted) reports
    /// its target, so an unshown panel round-trips its position instead
    /// of dropping to zero.
    fn scroll_row(&self, cx: &App) -> usize {
        if let Some(row) = self.restore_scroll {
            return row;
        }
        let table = self.table.read(cx);
        let handle = table.vertical_scroll_handle.0.borrow();
        if let Some(deferred) = &handle.deferred_scroll_to_item {
            return deferred.item_index;
        }
        let row_height = table.delegate().density.size().table_row_height();
        if row_height <= px(0.) {
            return 0;
        }
        (-handle.base_handle.offset().y / row_height)
            .floor()
            .max(0.) as usize
    }

    /// Show or hide a registry column, keeping the rest in place. A shown
    /// column appends at the end in its default width; hiding drops it.
    /// The table re-reads the delegate's columns and the view stays put.
    fn toggle_column(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(def) = column_def(key) else { return };
        self.table.update(cx, |table, cx| {
            let columns = &mut table.delegate_mut().columns;
            if let Some(ix) = columns.iter().position(|c| c.key.as_ref() == key) {
                // Never let the last column go: an empty table has no
                // header to bring one back from.
                if columns.len() > 1 {
                    columns.remove(ix);
                }
            } else {
                let column = Column::new(def.key, def.label).width(px(def.default_width));
                columns.push(if def.right { column.text_right() } else { column });
            }
            table.refresh(cx);
        });
        self.refresh_title_bar(cx);
    }

    /// The keys of the currently shown columns, for the menu's checks.
    fn shown_columns(&self, cx: &App) -> HashSet<String> {
        self.table
            .read(cx)
            .delegate()
            .columns
            .iter()
            .map(|c| c.key.to_string())
            .collect()
    }

    /// The customize window's column picker: one chip per registry column,
    /// filled while shown, plus a reset. Multi-select, so it wraps chips
    /// instead of using the exclusive segmented control.
    fn column_chips(&self, cx: &mut Context<Self>) -> Div {
        let shown = self.shown_columns(cx);
        let mut chips = div().flex().flex_row().flex_wrap().gap(tokens::SPACE_XS);
        for def in COLUMNS {
            let key = def.key;
            let on = shown.contains(key);
            chips = chips.child(
                div()
                    .px(tokens::SPACE_SM)
                    .py(tokens::SPACE_XS)
                    .rounded(tokens::RADIUS)
                    .bg(if on {
                        palette::accent()
                    } else {
                        palette::bg_control()
                    })
                    .text_color(if on {
                        palette::text_on_accent()
                    } else {
                        palette::text()
                    })
                    .when(!on, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.toggle_column(key, cx)),
                    )
                    .child(def.label),
            );
        }
        chips.child(
            div()
                .px(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .rounded(tokens::RADIUS)
                .bg(palette::bg_control())
                .text_color(palette::text_muted())
                .hover(|d| d.bg(palette::bg_control_hover()))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.reset_columns(cx)),
                )
                .child("reset"),
        )
    }

    /// Restore the registry's default visible set and order.
    fn reset_columns(&mut self, cx: &mut Context<Self>) {
        let sort = self.table.read(cx).delegate().sort.clone();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().columns = track_columns(&[], &sort);
            table.refresh(cx);
        });
        self.refresh_title_bar(cx);
    }

    /// While docked, the panel's controls live in the tab panel's title bar,
    /// which only repaints when the tab panel itself is notified. Call this
    /// after any change the title bar shows: query, focus, status, error.
    fn refresh_title_bar(&self, cx: &mut App) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|tabs| tabs.upgrade()) {
            tabs.update(cx, |_, cx| cx.notify());
        }
    }

    /// Queue the double-clicked row and what follows it in the current
    /// view order. Headers pass under the cap, so it counts tracks.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows: Vec<usize> = {
            let delegate = self.table.read(cx).delegate();
            (ix..delegate.view.len())
                .filter(|&i| delegate.track_at(i).is_some())
                .take(QUEUE_CAP)
                .collect()
        };
        self.play_rows(rows, cx);
    }

    /// Resolve view rows to paths and queue them on the shared player.
    fn play_rows(&mut self, rows: Vec<usize>, cx: &mut Context<Self>) {
        let result = {
            let view = self.table.read(cx).delegate().view.clone();
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return;
            };
            let ids: Vec<i64> = rows
                .into_iter()
                .filter_map(|ix| match view.get(ix) {
                    Some(&Row::Track(row)) => Some(projection.db_id[row as usize]),
                    _ => None,
                })
                .collect();
            library.paths_for(&ids)
        };
        match result {
            Ok(paths) => self
                .state
                .player
                .update(cx, |player, cx| player.play(paths, cx)),
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
                self.refresh_title_bar(cx);
            }
        }
    }

    fn on_search_event(
        &mut self,
        search: &Entity<SearchBox>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => {
                self.query = search.read(cx).query().to_string();
                self.refresh_view(cx);
                cx.notify();
                self.refresh_title_bar(cx);
            }
            // The input's focus ring renders in the title bar while the
            // panel shares a group, and that row only repaints when the
            // tab panel is notified.
            SearchEvent::FocusChanged => {
                cx.notify();
                self.refresh_title_bar(cx);
            }
            // Escape on an empty query leaves the box, which hands the
            // playback keys back to the workspace.
            SearchEvent::Dismissed => {
                window.focus(&self.focus);
                cx.notify();
                self.refresh_title_bar(cx);
            }
            SearchEvent::Submitted => {}
        }
    }

    fn search_box(&self, _window: &Window, cx: &mut Context<Self>) -> Div {
        self.search.update(cx, |search, cx| search.element(cx))
    }

    /// The popped-out window has no title bar to host the controls, so it
    /// keeps them as a toolbar row above the list. The catalog status lives
    /// in the workspace menubar; only a panel-local error shows here.
    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(36.))
            .px(tokens::SPACE_SM)
            .gap(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .bg(palette::bg_toolbar())
            .border_b_1()
            .border_color(palette::border())
            .child(self.search_box(window, cx).flex_1())
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .flex_none()
                        .text_color(palette::text_muted())
                        .child(error),
                )
            })
    }

    fn track_list(&self) -> impl IntoElement {
        Table::new(&self.table)
            .stripe(true)
            .bordered(false)
            .with_size(self.density.size())
    }

    /// Set the row height and re-render; persisted on the next layout dump.
    fn set_density(&mut self, density: Density, cx: &mut Context<Self>) {
        if self.density == density {
            return;
        }
        self.density = density;
        // The delegate mirrors it for the header tile's row math.
        self.table
            .update(cx, |table, _| table.delegate_mut().density = density);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    /// Set the header style and rebuild the view; persisted on the next
    /// layout dump.
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>) {
        if self.table.read(cx).delegate().headers == headers {
            return;
        }
        self.table
            .update(cx, |table, _| table.delegate_mut().headers = headers);
        self.refresh_view(cx);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("library-empty")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.browse(cx)))
            .child(div().text_lg().child("open a music folder"))
            .child(
                div()
                    .text_color(palette::text_muted())
                    .child("it gets scanned into the library (flac, mp3, wav)"),
            )
    }
}

impl panel::PanelSettings for LibraryPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn pages(&self) -> &'static [&'static str] {
        &["View", "Behavior"]
    }

    fn page(
        &mut self,
        page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if page == "Behavior" {
            return div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(panel::setting_row(
                    "follow playing",
                    Some("scroll to the playing row whenever the track changes"),
                    panel::toggle(
                        self.follow_playing,
                        |this: &mut Self, on, cx| {
                            this.follow_playing = on;
                            // Catch up right away instead of waiting for
                            // the next track change.
                            if on {
                                this.follow_playing(cx);
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .when(self.follow_playing, |d| {
                    d.child(panel::setting_row(
                        "smooth scrolling",
                        Some("glide to the row instead of jumping"),
                        panel::toggle(
                            self.smooth_follow,
                            |this: &mut Self, on, cx| {
                                this.smooth_follow = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                })
                .into_any_element();
        }
        let headers = self.table.read(cx).delegate().headers;
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_block(
                "columns",
                Some("which columns show; drag the headers in the panel to reorder and size them"),
                self.column_chips(cx),
            ))
            .child(panel::setting_row(
                "headers",
                Some("album breaks over the canonical order; searching or sorting renders flat"),
                panel::choices(
                    &[
                        ("off", Headers::Off),
                        ("compact", Headers::Compact),
                        ("expanded", Headers::Expanded),
                    ],
                    headers,
                    |this: &mut Self, headers, cx| this.set_headers(headers, cx),
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "density",
                Some("the track list's row height"),
                panel::choices(
                    &[
                        ("compact", Density::Compact),
                        ("comfortable", Density::Comfortable),
                    ],
                    self.density,
                    |this: &mut Self, density, cx| this.set_density(density, cx),
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn theme(&self) -> PanelTheme {
        self.theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for LibraryPanel {}

impl Focusable for LibraryPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for LibraryPanel {
    fn panel_name(&self) -> &'static str {
        "library"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("library")
    }

    /// The panel's controls share the title bar row instead of stacking a
    /// second toolbar row under it. Kept compact: the title row is 30px.
    fn title_suffix(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .gap(tokens::SPACE_SM)
                .child(self.search_box(window, cx).w(px(180.)))
                .when_some(self.error.clone(), |d, error| {
                    d.child(
                        div()
                            .max_w(px(240.))
                            .truncate()
                            .text_color(palette::text_muted())
                            .child(error),
                    )
                }),
        )
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The table serves row context menus over the whole body, so the tab
    /// panel's body right-click stays out; the panel dropdown lives on the
    /// tab and the toolbar.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn dump(&self, cx: &App) -> PanelState {
        let config = self.config(cx);
        let mut state = PanelState::new(self);
        state.info =
            PanelInfo::panel(serde_json::to_value(config).unwrap_or(serde_json::Value::Null));
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self._tabs_changed = tab_panel
            .upgrade()
            .map(|tabs| cx.observe(&tabs, |_, _, cx| cx.notify()));
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
        self._tabs_changed = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // View section: quick settings only, all flat so a click dismisses
        // the menu and the next open rebuilds with the change reflected.
        // The bigger knobs - columns, headers, the query - live in the
        // customize window, where they get real controls instead of a
        // wall of check items.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Jump to Playing")
                .icon(Icon::default().path(icons::DISC))
                .on_click(move |_, _, cx| {
                    if let Some(this) = weak.upgrade() {
                        this.update(cx, |this, cx| this.jump_to_playing(cx));
                    }
                }),
        );

        // The density options carry their own icon, so the check that marks
        // the active one moves to the right edge instead of taking the left
        // icon slot; a left check would drop the icon on the picked row.
        let mut menu = menu.separator().label("Density").check_side(Side::Right);
        for (density, name, icon) in [
            (Density::Compact, "Compact", icons::ROWS_3),
            (Density::Comfortable, "Comfortable", icons::ROWS_2),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(name)
                    .icon(Icon::default().path(icon))
                    .checked(self.density == density)
                    .on_click(move |_, _, cx| {
                        if let Some(panel) = weak.upgrade() {
                            panel.update(cx, |panel, cx| panel.set_density(density, cx));
                        }
                    }),
            );
        }

        // Panel section: operations on the panel itself, not its contents.
        // Duplicate copies this view's config, over the same catalog and
        // player. Hand-rolled because the copy takes the query along.
        let menu = menu.separator();
        let menu = panel_settings::settings_item(menu, &cx.entity());
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (panel.state.clone(), panel.config(cx), panel.tab_panel.clone())
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| LibraryPanel::new(state, config, window, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
        let _ = window;
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for LibraryPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        panel::themed(&theme, || self.body(window, cx).into_any_element())
    }
}

impl LibraryPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The follow glide eases toward the playing row, stepped here in
        // render (the cover panel's fade idiom), one frame at a time until
        // it lands.
        let dt = self.glide_tick.elapsed().as_secs_f32().min(0.05);
        self.glide_tick = Instant::now();
        if let Some(row) = self.glide_to {
            let (handle, count) = {
                let table = self.table.read(cx);
                (
                    table.vertical_scroll_handle.clone(),
                    table.delegate().view.len(),
                )
            };
            match panel::glide_target(&handle, row, count) {
                Some(target) if !panel::glide_step(&handle, target, dt) => self.glide_to = None,
                // Not laid out yet, or still moving: keep going.
                _ => window.request_animation_frame(),
            }
        }

        let busy = self.state.library.read(cx).busy.is_some();
        let empty = self.table.read(cx).delegate().view.is_empty();
        let body = if empty && !busy && self.query.is_empty() {
            self.empty_state(cx).into_any_element()
        } else {
            self.track_list().into_any_element()
        };
        // The controls live in the tab bar via title_suffix while the panel
        // shares a group; solo or popped out there is no header at all, so
        // the toolbar renders in the body instead.
        let headerless = self
            .tab_panel
            .as_ref()
            .and_then(|tabs| tabs.upgrade())
            .map_or(true, |tabs| tabs.read(cx).panels_count() < 2);
        // The root must size itself: the dock's tab panel lays the panel view
        // out as a root element (cached, absolute), where flex_1 has no flex
        // parent to grow in and the height would collapse to the content.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_panel())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, window, cx| this.on_panel_key(event, window, cx)))
            .when(headerless, |d| d.child(self.toolbar(window, cx)))
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn load(
    db_path: &std::path::Path,
    refresh: Refresh,
    progress: &ScanProgress,
) -> Result<(Projection, Vec<u32>, Option<ScanSummary>), rox_library::rusqlite::Error> {
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
    };
    let shards = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let projection = Projection::load_parallel(db_path, shards)?;
    let order = projection.sort_canonical();
    Ok((projection, order, summary))
}

fn status_line(total: usize, summary: Option<&ScanSummary>) -> String {
    let Some(s) = summary else {
        return format!("{total} tracks");
    };
    // Zero counts say nothing, keep them out so the line stays short
    // enough for the menubar.
    let mut parts = Vec::new();
    if s.indexed > 0 {
        parts.push(format!("{} indexed", s.indexed));
    }
    if s.unchanged > 0 {
        parts.push(format!("{} unchanged", s.unchanged));
    }
    if s.untagged > 0 {
        parts.push(format!("{} untagged", s.untagged));
    }
    if s.aborted {
        parts.push("stopped early".into());
    }
    if parts.is_empty() {
        return format!("{total} tracks");
    }
    format!("{} tracks ({})", total, parts.join(", "))
}

pub(crate) fn fmt_ms(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// A group's total time: like [`fmt_ms`], growing an hours place once it
/// earns one.
fn fmt_total(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// A track number or year cell: blank when zero, since the scanner stores
/// a missing tag as 0 and a bare 0 reads as data.
fn fmt_num(n: u16) -> SharedString {
    if n == 0 {
        SharedString::default()
    } else {
        n.to_string().into()
    }
}
