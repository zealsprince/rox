//! The library: a shared catalog entity over the promoted library service,
//! and the dockable panel that browses it. The catalog owns the app's library
//! database and only ever hands out the in-memory projection, per the library
//! service boundary. Panels are views over the shared catalog with their own
//! search config, so a duplicated panel filters independently. Clicking a
//! track queues it straight on the shared player.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, relative, rgb, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, PathPromptOptions, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, PanelInfo, PanelState, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::projection::Projection;
use rox_library::rusqlite::Connection;
use rox_library::scanner::{self, ScanSummary};
use rox_library::store;

use crate::panel::{self, AppState};

/// Play from a clicked row: at most this many tracks are queued behind it.
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
    /// Set while a scan or projection load runs in the background.
    busy: Option<SharedString>,
    status: SharedString,
}

impl EventEmitter<LibraryEvent> for Library {}

impl Library {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let db_path = crate::settings::data_dir().join("library.db");
        let (conn, status) = match store::open(&db_path)
            .and_then(|conn| store::init_schema(&conn).map(|_| conn))
        {
            Ok(conn) => (Some(conn), SharedString::default()),
            Err(e) => (None, SharedString::from(format!("library db: {e}"))),
        };

        let mut this = Library {
            db_path,
            conn,
            projection: None,
            order: Arc::new(Vec::new()),
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

    /// What the toolbar shows: the running operation, or the last status.
    pub fn line(&self) -> SharedString {
        self.busy.clone().unwrap_or_else(|| self.status.clone())
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
}

/// One browse view over the shared catalog: its own search query and row
/// order, duplicable and poppable like any panel.
pub struct LibraryPanel {
    state: AppState,
    /// Rows currently displayed: the canonical order, or search hits.
    view: Arc<Vec<u32>>,
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
}

impl LibraryPanel {
    pub fn new(state: AppState, query: String, cx: &mut Context<Self>) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut LibraryPanel, _, _: &LibraryEvent, cx| {
                this.error = None;
                this.refresh_view(cx);
                cx.notify();
                this.refresh_title_bar(cx);
            },
        );
        let mut this = LibraryPanel {
            state,
            view: Arc::new(Vec::new()),
            query,
            focus: cx.focus_handle(),
            search_focus: cx.focus_handle(),
            error: None,
            tab_panel: None,
            _tabs_changed: None,
            _library_changed,
        };
        this.refresh_view(cx);
        this
    }

    fn refresh_view(&mut self, cx: &App) {
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            self.view = Arc::new(Vec::new());
            return;
        };
        self.view = if self.query.is_empty() {
            library.order()
        } else {
            Arc::new(projection.search(&self.query))
        };
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        self.state
            .library
            .update(cx, |library, cx| library.browse(cx));
    }

    /// While docked, the panel's controls live in the tab panel's title bar,
    /// which only repaints when the tab panel itself is notified. Call this
    /// after any change the title bar shows: query, focus, status, error.
    fn refresh_title_bar(&self, cx: &mut App) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|tabs| tabs.upgrade()) {
            tabs.update(cx, |_, cx| cx.notify());
        }
    }

    /// Queue the clicked row and what follows it in the current view order.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let result = {
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return;
            };
            let end = (ix + QUEUE_CAP).min(self.view.len());
            let ids: Vec<i64> = self.view[ix..end]
                .iter()
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
                let Some(text) = &keystroke.key_char else { return };
                self.query.push_str(text);
            }
        }
        self.refresh_view(cx);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    fn open_folder_button(&self, cx: &mut Context<Self>) -> Div {
        div()
            .px_2()
            .h(px(22.))
            .flex()
            .items_center()
            .flex_none()
            .rounded_md()
            .bg(rgb(0x2a2a2a))
            .hover(|d| d.bg(rgb(0x3a3a3a)))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.browse(cx)),
            )
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
            .bg(rgb(0x141414))
            .border_1()
            .border_color(if focused { rgb(0x4a6a55) } else { rgb(0x333333) })
            .when(self.query.is_empty(), |d| d.text_color(rgb(0x808080)))
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

    fn status_line(&self, cx: &Context<Self>) -> SharedString {
        self.error
            .clone()
            .unwrap_or_else(|| self.state.library.read(cx).line())
    }

    /// The popped-out window has no title bar to host the controls, so it
    /// keeps them as a toolbar row above the list.
    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line = self.status_line(cx);
        div()
            .flex_none()
            .h(px(36.))
            .px_2()
            .gap_2()
            .flex()
            .flex_row()
            .items_center()
            .bg(rgb(0x1f1f1f))
            .border_b_1()
            .border_color(rgb(0x333333))
            .child(self.open_folder_button(cx))
            .child(self.search_box(window, cx).flex_1())
            .child(div().flex_none().text_color(rgb(0x808080)).child(line))
    }

    fn track_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::uniform_list(
            "tracks",
            self.view.len(),
            cx.processor(|this, range: Range<usize>, _, cx| {
                let Some(projection) = this.state.library.read(cx).projection().cloned() else {
                    return Vec::new();
                };
                let view = this.view.clone();
                range
                    .map(|ix| {
                        let v = projection.resolve(view[ix]);
                        div()
                            .id(ix)
                            .flex_none()
                            .h(px(28.))
                            .px_2()
                            .gap_2()
                            .flex()
                            .flex_row()
                            .items_center()
                            .when(ix % 2 == 1, |d| d.bg(rgb(0x202020)))
                            .hover(|d| d.bg(rgb(0x2e2e2e)))
                            .cursor_pointer()
                            // Focus moves to the panel: queueing a track is
                            // done with the search box, space should pause.
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.focus);
                                this.play_from(ix, cx)
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(SharedString::from(v.title.to_string())),
                            )
                            .child(
                                div()
                                    .w(relative(0.22))
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(0xa0a0a0))
                                    .child(SharedString::from(v.artist.to_string())),
                            )
                            .child(
                                div()
                                    .w(relative(0.22))
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(0xa0a0a0))
                                    .child(SharedString::from(v.album.to_string())),
                            )
                            .child(
                                div()
                                    .w(px(44.))
                                    .flex_none()
                                    .text_color(rgb(0x808080))
                                    .child(fmt_ms(v.duration_ms)),
                            )
                    })
                    .collect()
            }),
        )
        .h_full()
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.browse(cx)),
            )
            .child(div().text_lg().child("open a music folder"))
            .child(
                div()
                    .text_color(rgb(0x808080))
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
        let line = self.status_line(cx);
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .gap_2()
                .child(self.open_folder_button(cx))
                .child(self.search_box(window, cx).w(px(180.)))
                .child(
                    div()
                        .max_w(px(240.))
                        .truncate()
                        .text_color(rgb(0x808080))
                        .child(line),
                ),
        )
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn dump(&self, _cx: &App) -> PanelState {
        let config = LibraryConfig {
            query: self.query.clone(),
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
        let menu = menu.item(PopupMenuItem::new("Duplicate").on_click(move |_, window, cx| {
            let Some(this) = weak.upgrade() else { return };
            let (state, query, tabs) = {
                let panel = this.read(cx);
                (
                    panel.state.clone(),
                    panel.query.clone(),
                    panel.tab_panel.clone(),
                )
            };
            let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                return;
            };
            let dup = cx.new(|cx| LibraryPanel::new(state, query, cx));
            tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
        }));
        panel::popout_item(menu, &cx.entity(), self.tab_panel.clone(), self.state.clone())
    }
}

impl Render for LibraryPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.state.library.read(cx).busy.is_some();
        let empty = self.view.is_empty();
        let body = if empty && !busy && self.query.is_empty() {
            self.empty_state(cx).into_any_element()
        } else {
            self.track_list(cx).into_any_element()
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
            .bg(rgb(0x181818))
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
