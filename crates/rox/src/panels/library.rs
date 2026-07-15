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
use std::time::Duration;

use gpui::{
    div, prelude::*, px, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, PathPromptOptions, SharedString, Stateful, Subscription, WeakEntity, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState};
use gpui_component::{Sizable, Size};
use rox_dock::{Panel, PanelEvent, PanelInfo, PanelState, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::projection::{Projection, SortKey};
use rox_library::rusqlite::Connection;
use rox_library::scanner::{self, ScanSummary};
use rox_library::store;

use crate::design::{palette, tokens};
use crate::panel::{self, AppState};

/// Play from a double-clicked row: at most this many tracks are queued
/// behind it.
const QUEUE_CAP: usize = 1000;

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
    /// The canonical browse order: artist, album, track number.
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

    /// Each folder with how many tracks it holds, on the UI-side
    /// connection. The list never nests, so no track counts twice.
    pub fn root_counts(&self) -> Vec<(PathBuf, u64)> {
        self.scan_roots
            .iter()
            .map(|root| {
                let count = self
                    .conn
                    .as_ref()
                    .and_then(|conn| store::count_under(conn, root).ok())
                    .unwrap_or(0);
                (root.clone(), count)
            })
            .collect()
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
    ColumnDef { key: "album", label: "album", default_width: 220., right: false, default_on: true, sort: SortKey::Album },
    ColumnDef { key: "genre", label: "genre", default_width: 140., right: false, default_on: false, sort: SortKey::Genre },
    ColumnDef { key: "year", label: "year", default_width: 56., right: true, default_on: false, sort: SortKey::Year },
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
    /// The shown columns in display order, each with its width. Empty
    /// restores the registry default set. Named apart from the old
    /// index-keyed `columns` field so pre-registry layouts drop their
    /// widths quietly instead of failing the whole config.
    #[serde(default)]
    pub column_layout: Vec<ColumnSpec>,
    /// The sorted column's registry key. None browses the canonical
    /// artist, album, track order.
    #[serde(default)]
    pub sort_key: Option<String>,
    #[serde(default)]
    pub sort_desc: bool,
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

/// The table delegate: the column set and the rows one panel displays.
/// Lives inside the panel's `TableState`; the panel swaps `view` when the
/// query or the catalog changes.
struct TrackTable {
    state: AppState,
    /// The owning panel, for dispatching context menu actions back to it.
    panel: WeakEntity<LibraryPanel>,
    /// Rows currently displayed: the canonical order, or search hits,
    /// re-sorted when a column sort is active.
    view: Arc<Vec<u32>>,
    /// Selected rows as indices into `view`. Cleared when the view swaps,
    /// since the indices point elsewhere afterwards.
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
}

impl TrackTable {
    /// The next row whose leading text starts with the typed prefix, from
    /// the cursor on, wrapping. The leading text follows the active sort:
    /// its column when it has text, artist for the canonical order and
    /// for sorts without one (duration). ASCII-insensitive, like search.
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
            let v = projection.resolve(self.view[ix]);
            let text = match field {
                Some("title") => v.title,
                Some("album") => v.album,
                _ => v.artist,
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
                .position(|&row| projection.db_id[row as usize] == id)
        });
        self.playing_row = row;
    }

    /// The rows this panel shows: the canonical order or search hits, put
    /// through the active column sort when one is set.
    fn compute_view(&self, query: &str, cx: &App) -> Arc<Vec<u32>> {
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return Arc::new(Vec::new());
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
            Some((key, desc)) => Arc::new(projection.sort_view(&base, key, desc)),
            None => base,
        }
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
            .filter_map(|&ix| self.view.get(ix))
            .map(|&row| projection.db_id[row as usize])
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
        self.view = self.compute_view(&query, cx);
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
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
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
    /// the menu always acts on what is highlighted.
    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if !self.selected.contains(&row_ix) {
            self.selected = HashSet::from([row_ix]);
            self.anchor = Some(row_ix);
            self.publish_selection(cx);
            cx.notify();
        }
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        let panel = self.panel.clone();
        if rows.len() > 1 {
            menu.item(
                PopupMenuItem::new(format!("Play {} Tracks", rows.len())).on_click(
                    move |_, _, cx| {
                        if let Some(panel) = panel.upgrade() {
                            panel.update(cx, |panel, cx| panel.play_rows(rows.clone(), cx));
                        }
                    },
                ),
            )
        } else {
            menu.item(PopupMenuItem::new("Play").on_click(move |_, _, cx| {
                if let Some(panel) = panel.upgrade() {
                    panel.update(cx, |panel, cx| panel.play_from(row_ix, cx));
                }
            }))
        }
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(&row) = self.view.get(row_ix) else {
            return div().into_any_element();
        };
        let Some(projection) = self.state.library.read(cx).projection().cloned() else {
            return div().into_any_element();
        };
        let v = projection.resolve(row);
        let cell = div().truncate();
        match self.columns[col_ix].key.as_ref() {
            "track" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.track_no)),
            "title" => cell
                .when(self.playing_row == Some(row_ix), |d| {
                    d.text_color(palette::accent())
                })
                .child(SharedString::from(v.title.to_string())),
            "artist" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.artist.to_string())),
            "album" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.album.to_string())),
            "genre" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.genre.to_string())),
            "year" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.year)),
            "duration" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(fmt_ms(v.duration_ms))),
            _ => cell,
        }
        .into_any_element()
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
    /// The query editor; `query` mirrors its value via change events.
    search_input: Entity<InputState>,
    /// A panel-local error (a failed play), shown until the catalog updates.
    error: Option<SharedString>,
    /// The playing track's path, the change detector: the player notifies
    /// every pump tick, so everything up to this compare stays cheap.
    playing_path: Option<PathBuf>,
    /// The type-ahead buffer and when it last grew; a pause starts over.
    type_ahead: String,
    type_ahead_at: Option<std::time::Instant>,
    /// The track list's row height, applied on the table each render.
    density: Density,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Watches the hosting tab panel: whether this panel is solo decides
    /// where the toolbar renders, so membership changes must re-render.
    _tabs_changed: Option<Subscription>,
    _library_changed: Subscription,
    _table_events: Subscription,
    _search_events: Subscription,
    _player_changed: Subscription,
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
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            columns: track_columns(&config.column_layout, &sort),
            sort,
            playing_id: None,
            playing_row: None,
        };
        // Widths and order persist by column key, so a drag survives a
        // layout save; the delegate mirrors the widget's reorder.
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .col_movable(true)
                .col_selectable(false)
        });
        let _table_events = cx.subscribe_in(&table, window, Self::on_table_event);
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("search")
                .default_value(config.query.clone())
        });
        let _search_events = cx.subscribe_in(&search_input, window, Self::on_search_event);
        let _player_changed = cx.observe(&state.player, |this: &mut LibraryPanel, _, cx| {
            this.sync_playing(cx)
        });
        let mut this = LibraryPanel {
            state,
            table,
            query: config.query,
            focus: cx.focus_handle(),
            search_input,
            error: None,
            playing_path: None,
            type_ahead: String::new(),
            type_ahead_at: None,
            density: config.density,
            tab_panel: None,
            _tabs_changed: None,
            _library_changed,
            _table_events,
            _search_events,
            _player_changed,
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
    }

    /// Browse from the keyboard while the panel itself is focused: arrows
    /// move a cursor, shift extends from the click path's anchor, enter
    /// plays, and plain typing jumps to the next match in the leading
    /// column. With the search box focused these stay out of the way: in
    /// the solo and popped-out layouts its toolbar sits inside the panel
    /// root, so its keystrokes bubble through here.
    fn on_panel_key(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        if self
            .search_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
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
            "home" => self.set_cursor(0, shift, cx),
            "end" => {
                let len = self.table.read(cx).delegate().view.len();
                if len > 0 {
                    self.set_cursor(len - 1, shift, cx);
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
            if ix >= delegate.view.len() {
                return;
            }
            delegate.cursor = Some(ix);
            if extend {
                let anchor = delegate.anchor.unwrap_or(ix);
                let (lo, hi) = (anchor.min(ix), anchor.max(ix));
                delegate.selected = (lo..=hi).collect();
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
    /// the step heads toward.
    fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let target = {
            let delegate = self.table.read(cx).delegate();
            let len = delegate.view.len();
            if len == 0 {
                return;
            }
            match delegate.cursor {
                None if delta >= 0 => 0,
                None => len - 1,
                Some(cursor) => (cursor as isize + delta).clamp(0, len as isize - 1) as usize,
            }
        };
        self.set_cursor(target, extend, cx);
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
            let view = table.delegate().compute_view(&query, cx);
            let delegate = table.delegate_mut();
            delegate.view = view;
            delegate.selected.clear();
            delegate.anchor = None;
            delegate.cursor = None;
            delegate.locate_playing(cx);
            table.clear_selection(cx);
            cx.notify();
        });
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
                let modifiers = window.modifiers();
                self.table.update(cx, |table, cx| {
                    let delegate = table.delegate_mut();
                    if modifiers.shift {
                        let anchor = delegate.anchor.unwrap_or(ix);
                        let (lo, hi) = (anchor.min(ix), anchor.max(ix));
                        delegate.selected = (lo..=hi).collect();
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
            // to select.
            TableEvent::DoubleClickedRow(ix) => {
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
            column_layout: self.column_specs(cx),
            sort_key: sort.as_ref().map(|(key, _)| key.to_string()),
            sort_desc: sort.is_some_and(|(_, desc)| desc),
        }
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
    /// view order.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let end = {
            let view = &self.table.read(cx).delegate().view;
            (ix + QUEUE_CAP).min(view.len())
        };
        self.play_rows((ix..end).collect(), cx);
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
                .filter_map(|ix| view.get(ix))
                .map(|&row| projection.db_id[row as usize])
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
        input: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.query = input.read(cx).value().to_string();
                self.refresh_view(cx);
                cx.notify();
                self.refresh_title_bar(cx);
            }
            // The input's focus ring renders in the title bar while the
            // panel shares a group, and that row only repaints when the
            // tab panel is notified.
            InputEvent::Focus | InputEvent::Blur => {
                cx.notify();
                self.refresh_title_bar(cx);
            }
            _ => {}
        }
    }

    fn search_box(&self, _window: &Window, cx: &mut Context<Self>) -> Div {
        div()
            // Scopes the workspace's playback key bindings out while the
            // input is focused, so space and arrows type instead.
            .key_context("SearchInput")
            // First escape clears the query, a second one leaves the box,
            // which hands the playback keys back to the workspace. The
            // widget propagates escape when it has nothing of its own
            // (IME, context menu) to close, so it lands here.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                if this.query.is_empty() {
                    window.focus(&this.focus);
                    cx.notify();
                    this.refresh_title_bar(cx);
                } else {
                    this.search_input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
            }))
            .child(Input::new(&self.search_input).small().w_full())
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
        // View section: what this view shows and how, all flat so a click
        // dismisses the menu and the next open rebuilds with the change
        // reflected. Nested submenus keep their own state and can't close
        // the root from inside, so a toggled check reads stale until the
        // whole menu is reopened; flat items sidestep that.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Jump to Playing").on_click(move |_, _, cx| {
                if let Some(this) = weak.upgrade() {
                    this.update(cx, |this, cx| this.jump_to_playing(cx));
                }
            }),
        );

        let mut menu = menu.separator().label("Columns");
        let shown = self.shown_columns(cx);
        for def in COLUMNS {
            let key = def.key;
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(def.label)
                    .checked(shown.contains(key))
                    .on_click(move |_, _, cx| {
                        if let Some(panel) = weak.upgrade() {
                            panel.update(cx, |panel, cx| panel.toggle_column(key, cx));
                        }
                    }),
            );
        }
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Reset Columns").on_click(move |_, _, cx| {
                if let Some(panel) = weak.upgrade() {
                    panel.update(cx, |panel, cx| panel.reset_columns(cx));
                }
            }),
        );

        let mut menu = menu.separator().label("Density");
        for (density, name) in [
            (Density::Compact, "Compact"),
            (Density::Comfortable, "Comfortable"),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(name)
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
        // player. Hand-rolled rather than `panel::duplicate_item` because
        // the copy takes the query along.
        let weak = cx.entity().downgrade();
        let menu = menu.separator().item(
            PopupMenuItem::new("Duplicate Panel").on_click(move |_, window, cx| {
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
                let mut root_total = 0;
                let s = scanner::scan(&mut conn, &root, |scanned, total, path| {
                    root_total = total;
                    progress.scanned.store(done + scanned, Ordering::Relaxed);
                    progress.total.store(done + total, Ordering::Relaxed);
                    *progress.current.lock().unwrap() = path.to_string_lossy().into_owned();
                    !progress.cancel.load(Ordering::Relaxed)
                })?;
                done += root_total;
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
    let order = projection.sort_artist_album_track();
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

fn fmt_ms(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
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
