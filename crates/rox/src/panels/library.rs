//! The library: a shared catalog entity over the promoted library service,
//! and the dockable panel that browses it. The catalog owns the app's library
//! database and only ever hands out the in-memory projection, per the library
//! service boundary. Panels are views over the shared catalog with their own
//! search config, so a duplicated panel filters independently. Double
//! clicking a track queues it straight on the shared player; single clicks
//! select, and the selection publishes app-wide for panels that display it.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, PathPromptOptions, SharedString, Stateful, Subscription, WeakEntity,
    Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::table::{Column, Table, TableDelegate, TableEvent, TableState};
use rox_dock::{Panel, PanelEvent, PanelInfo, PanelState, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::projection::Projection;
use rox_library::rusqlite::Connection;
use rox_library::scanner::{self, ScanSummary};
use rox_library::store;

use crate::palette;
use crate::panel::{self, AppState};

/// Play from a double-clicked row: at most this many tracks are queued
/// behind it.
const QUEUE_CAP: usize = 1000;

/// The catalog changed: a scan finished or the projection reloaded. Panels
/// subscribe and refresh their views.
pub enum LibraryEvent {
    Updated,
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
    /// The folder scans read from, remembered for rescans and persisted
    /// in settings. None until a folder has been opened.
    scan_root: Option<PathBuf>,
    /// Set while a scan or projection load runs in the background.
    busy: Option<SharedString>,
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

        // A library indexed before the root was persisted still has one in
        // its paths: the deepest directory shared by every track. Session
        // only; the next Open Folder persists what the user actually picked.
        let scan_root = crate::settings::Settings::load().library_root.or_else(|| {
            conn.as_ref()
                .and_then(|conn| store::common_root(conn).ok().flatten())
        });

        let mut this = Library {
            db_path,
            conn,
            projection: None,
            order: Arc::new(Vec::new()),
            scan_root,
            busy: None,
            status,
        };
        if this.conn.is_some() {
            this.reload(None, cx);
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
    pub fn status(&self) -> SharedString {
        self.status.clone()
    }

    /// Whether a rescan has a folder to scan.
    pub fn can_rescan(&self) -> bool {
        self.scan_root.is_some()
    }

    /// Scan the remembered folder again; a no-op until one has been opened
    /// or while a scan is already running.
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        if let Some(root) = self.scan_root.clone() {
            self.reload(Some(root), cx);
        }
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

    /// Load the projection off the UI thread, optionally scanning `root`
    /// first. The finished projection and its canonical sort swap in whole.
    fn reload(&mut self, scan_root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.busy = Some(if scan_root.is_some() {
            "scanning...".into()
        } else {
            "loading library...".into()
        });
        if let Some(root) = &scan_root {
            self.scan_root = Some(root.clone());
            let root = root.clone();
            crate::settings::Settings::update(move |s| s.library_root = Some(root));
        }
        let db_path = self.db_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load(&db_path, scan_root) })
                .await;
            this.update(cx, |this, cx| {
                this.busy = None;
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
                    this.update(cx, |this, cx| this.reload(Some(root), cx)).ok();
                }
            }
        })
        .detach();
    }
}

/// The panel's per-view config: what a saved layout restores, and the
/// schema a future per-panel settings menu edits. One struct serves both,
/// so new knobs land here.
#[derive(Default, Serialize, Deserialize)]
pub struct LibraryConfig {
    #[serde(default)]
    pub query: String,
    /// Column widths in px, in table column order. Missing entries fall
    /// back to the defaults, so an empty vec means the default layout.
    #[serde(default)]
    pub columns: Vec<f32>,
}

/// The column set, with saved widths overriding the defaults per index.
fn track_columns(widths: &[f32]) -> Vec<Column> {
    [
        ("title", "title", 420.),
        ("artist", "artist", 220.),
        ("album", "album", 220.),
        ("duration", "time", 64.),
    ]
    .iter()
    .enumerate()
    .map(|(ix, (key, name, default))| {
        let width = widths.get(ix).copied().unwrap_or(*default);
        let column = Column::new(*key, *name).width(px(width));
        if *key == "duration" {
            column.text_right()
        } else {
            column
        }
    })
    .collect()
}

/// The table delegate: the column set and the rows one panel displays.
/// Lives inside the panel's `TableState`; the panel swaps `view` when the
/// query or the catalog changes.
struct TrackTable {
    state: AppState,
    /// The owning panel, for dispatching context menu actions back to it.
    panel: WeakEntity<LibraryPanel>,
    /// Rows currently displayed: the canonical order, or search hits.
    view: Arc<Vec<u32>>,
    /// Selected rows as indices into `view`. Cleared when the view swaps,
    /// since the indices point elsewhere afterwards.
    selected: HashSet<usize>,
    /// Where the next shift-click extends from: the last plain or
    /// toggle-clicked row.
    anchor: Option<usize>,
    columns: Vec<Column>,
}

impl TrackTable {
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

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        // The same wash the widget theme paints its own focus row with, so
        // multi-selected rows read as one set.
        div()
            .id(("row", row_ix))
            .cursor_pointer()
            .when(self.selected.contains(&row_ix), |d| {
                d.bg(palette::alpha(palette::accent(), 0x26))
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
            "title" => cell.child(SharedString::from(v.title.to_string())),
            "artist" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.artist.to_string())),
            "album" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.album.to_string())),
            "duration" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(fmt_ms(v.duration_ms))),
            _ => cell,
        }
        .into_any_element()
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
    /// apart from the search focus so activating the tab does not put every
    /// keystroke in the query, and so the playback key bindings (scoped out
    /// of SearchInput) stay live.
    focus: FocusHandle,
    search_focus: FocusHandle,
    /// A panel-local error (a failed play), shown until the catalog updates.
    error: Option<SharedString>,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Watches the hosting tab panel: whether this panel is solo decides
    /// where the toolbar renders, so membership changes must re-render.
    _tabs_changed: Option<Subscription>,
    _library_changed: Subscription,
    _table_events: Subscription,
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
        let delegate = TrackTable {
            state: state.clone(),
            panel: cx.weak_entity(),
            view: Arc::new(Vec::new()),
            selected: HashSet::new(),
            anchor: None,
            columns: track_columns(&config.columns),
        };
        // Sorting waits on the projection growing sortable views; column
        // moves would need the width persistence to track order too.
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .sortable(false)
                .col_movable(false)
                .col_selectable(false)
        });
        let _table_events = cx.subscribe_in(&table, window, Self::on_table_event);
        let mut this = LibraryPanel {
            state,
            table,
            query: config.query,
            focus: cx.focus_handle(),
            search_focus: cx.focus_handle(),
            error: None,
            tab_panel: None,
            _tabs_changed: None,
            _library_changed,
            _table_events,
        };
        this.refresh_view(cx);
        this
    }

    fn refresh_view(&mut self, cx: &mut Context<Self>) {
        let view = {
            let library = self.state.library.read(cx);
            match library.projection() {
                None => Arc::new(Vec::new()),
                Some(projection) => {
                    if self.query.is_empty() {
                        library.order()
                    } else {
                        Arc::new(projection.search(&self.query))
                    }
                }
            }
        };
        self.table.update(cx, |table, cx| {
            // Selection indices point into the old view; drop them along
            // with the widget's own focus row. The shared selection keeps
            // the last explicit pick, a view refresh is not one.
            let delegate = table.delegate_mut();
            delegate.view = view;
            delegate.selected.clear();
            delegate.anchor = None;
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

    /// Current column widths, for the layout dump and for duplicates.
    fn column_widths(&self, cx: &App) -> Vec<f32> {
        self.table
            .read(cx)
            .delegate()
            .columns
            .iter()
            .map(|column| f32::from(column.width))
            .collect()
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

    fn on_search_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "backspace" => {
                self.query.pop();
            }
            // First escape clears the query, a second one leaves the search
            // box, which hands the playback keys back to the workspace.
            "escape" => {
                if self.query.is_empty() {
                    window.focus(&self.focus);
                } else {
                    self.query.clear();
                }
            }
            _ => {
                if keystroke.modifiers.control
                    || keystroke.modifiers.platform
                    || keystroke.modifiers.alt
                {
                    return;
                }
                let Some(text) = &keystroke.key_char else {
                    return;
                };
                self.query.push_str(text);
            }
        }
        self.refresh_view(cx);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    fn open_folder_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("open-folder")
            .px_2()
            .h(px(22.))
            .flex()
            .items_center()
            .flex_none()
            .rounded_md()
            .bg(palette::bg_control())
            .hover(|d| d.bg(palette::bg_control_hover()))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.browse(cx)))
            .child("open folder")
    }

    fn search_box(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let focused = self.search_focus.is_focused(window);
        let search_text: SharedString = if self.query.is_empty() {
            "search".into()
        } else {
            self.query.clone().into()
        };
        div()
            .px_2()
            .h(px(22.))
            .flex()
            .items_center()
            .rounded_md()
            .bg(palette::bg_input())
            .border_1()
            .border_color(if focused {
                palette::focus_ring()
            } else {
                palette::border()
            })
            .when(self.query.is_empty(), |d| {
                d.text_color(palette::text_muted())
            })
            .track_focus(&self.search_focus)
            // Scopes the workspace's playback key bindings out while the
            // box is focused, so space and arrows type instead.
            .key_context("SearchInput")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.search_focus);
                    cx.notify();
                    this.refresh_title_bar(cx);
                }),
            )
            .on_key_down(cx.listener(|this, event, window, cx| {
                this.on_search_key(event, window, cx);
            }))
            .child(search_text)
    }

    /// The popped-out window has no title bar to host the controls, so it
    /// keeps them as a toolbar row above the list. The catalog status lives
    /// in the workspace menubar; only a panel-local error shows here.
    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(36.))
            .px_2()
            .gap_2()
            .flex()
            .flex_row()
            .items_center()
            .bg(palette::bg_toolbar())
            .border_b_1()
            .border_color(palette::border())
            .child(self.open_folder_button(cx))
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
        Table::new(&self.table).stripe(true).bordered(false)
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("library-empty")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
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
                .gap_2()
                .child(self.open_folder_button(cx))
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
        let config = LibraryConfig {
            query: self.query.clone(),
            columns: self.column_widths(cx),
        };
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // Duplicate: a second view with its own copy of the config, over the
        // same catalog and player. Hand-rolled rather than through
        // `panel::duplicate_item` because the copy takes the query along.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate").on_click(move |_, window, cx| {
                let Some(this) = weak.upgrade() else { return };
                let (state, config, tabs) = {
                    let panel = this.read(cx);
                    let config = LibraryConfig {
                        query: panel.query.clone(),
                        columns: panel.column_widths(cx),
                    };
                    (panel.state.clone(), config, panel.tab_panel.clone())
                };
                let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                    return;
                };
                let dup = cx.new(|cx| LibraryPanel::new(state, config, window, cx));
                tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
            }),
        );
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
            .when(headerless, |d| d.child(self.toolbar(window, cx)))
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn load(
    db_path: &std::path::Path,
    scan_root: Option<PathBuf>,
) -> Result<(Projection, Vec<u32>, Option<ScanSummary>), rox_library::rusqlite::Error> {
    let summary = match scan_root {
        Some(root) => {
            let mut conn = store::open(db_path)?;
            store::init_schema(&conn)?;
            Some(scanner::scan(&mut conn, &root)?)
        }
        None => None,
    };
    let shards = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let projection = Projection::load_parallel(db_path, shards)?;
    let order = projection.sort_artist_album_track();
    Ok((projection, order, summary))
}

fn status_line(total: usize, summary: Option<&ScanSummary>) -> String {
    match summary {
        Some(s) => format!(
            "{} tracks ({} indexed, {} unchanged, {} untagged)",
            total, s.indexed, s.unchanged, s.untagged
        ),
        None => format!("{total} tracks"),
    }
}

fn fmt_ms(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}
