//! The history panel: the listen record as a track list, per ADR 11 and
//! the scope's history surface. Three views over the same events - the
//! newest listens first, tracks by play count, and the library tracks no
//! event has ever named - picked per panel, so a duplicate can watch
//! each. Rows read at panel-open and listen-append cadence off the
//! library's events table, never per frame; clicks select and double
//! clicks queue from the row, the library panel's moves. Its own panel,
//! never a mode of the library.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    div, prelude::*, px, uniform_list, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    MouseButton, MouseDownEvent, SharedString, Stateful, Subscription, UniformListScrollHandle,
    WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::listens::TrackPlays;
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::group_head::Headers;
use crate::history::HistoryEvent;
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::{LibraryEvent, QUEUE_CAP};
use crate::track_cells;
use crate::track_columns::{self, Column, ColumnHost, GroupTrack, HeadingHost};

/// One row's height; the list is a uniform_list, so every row agrees.
const ROW_H: f32 = 30.;

/// How many rows a view reads. The panel is a window into the record,
/// not an export; the events themselves are unbounded.
const ROWS_CAP: usize = 500;

/// Which cut of the events the panel shows.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryView {
    #[default]
    Recent,
    Most,
    Never,
}

impl HistoryView {
    fn label(self) -> &'static str {
        match self {
            HistoryView::Recent => "Recently Played",
            HistoryView::Most => "Most Played",
            HistoryView::Never => "Never Played",
        }
    }
}

/// The track columns, in render order. Plays and Last Played are the record's
/// own, drawn here; the rest are the shared columns [`track_columns::cell`]
/// draws. The view sets the query order, not which columns show.
const COLUMNS: &[Column] = &[
    Column { key: "cover", label: "Cover", default_on: false },
    Column { key: "number", label: "Number", default_on: false },
    Column { key: "name", label: "Name", default_on: true },
    Column { key: "artist", label: "Artist", default_on: true },
    Column { key: "album", label: "Album", default_on: false },
    Column { key: "year", label: "Year", default_on: false },
    Column { key: "genre", label: "Genre", default_on: false },
    Column { key: "duration", label: "Duration", default_on: false },
    Column { key: "plays", label: "Plays", default_on: true },
    Column { key: "lastplayed", label: "Last Played", default_on: true },
    Column { key: "rating", label: "Rating", default_on: true },
    Column { key: "favourite", label: "Favourite", default_on: true },
];

/// A flattened display row: an album heading (Recent view only), or a track
/// at its index into `tracks`.
enum Row {
    Album(u32),
    AlbumMeta(u32),
    Track(u32),
}

/// A history track's grouping inputs, borrowed for the album run aggregate.
fn group_track(t: &TrackPlays) -> GroupTrack<'_> {
    GroupTrack {
        album: &t.album,
        album_artist: &t.album_artist,
        artist: &t.artist,
        year: t.year,
        genre: &t.genre,
        codec: &t.codec,
        bitrate_kbps: t.bitrate_kbps,
        duration_ms: t.duration_ms,
        track_id: t.track_id,
    }
}

/// The history panel's per-view config: what a saved layout restores,
/// and what the settings window edits. Missing fields take the defaults,
/// so a layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub view: HistoryView,
    /// The album heading mode, honoured on the Recent view only - the Most
    /// and Never orders never keep an album's tracks together.
    pub headers: Headers,
    /// The shown column keys; defaults to the registry's default-on set.
    pub columns: Vec<String>,
}

// Hand-written so the columns default to the registry set, both for a new
// panel and a saved layout from before columns existed.
impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            chrome: PanelChrome::default(),
            view: HistoryView::default(),
            headers: Headers::Off,
            columns: track_columns::default_columns(COLUMNS),
        }
    }
}

pub struct HistoryPanel {
    state: AppState,
    config: HistoryConfig,
    /// The current view's tracks in query order, re-read when a listen lands
    /// or the catalog changes, cached between.
    tracks: Vec<TrackPlays>,
    /// The display rows over `tracks`: the tracks flat, or broken by album
    /// headings on the Recent view.
    rows: Vec<Row>,
    /// The album runs the heading rows index, rebuilt with `rows`.
    albums: Vec<track_columns::AlbumGroup>,
    /// The favourited track ids, what each row's heart checks against.
    favourites: std::collections::HashSet<i64>,
    /// The clicked track's index into `tracks`, for the selection highlight.
    selected: Option<usize>,
    /// The playing track's path, the change detector for the highlight;
    /// the player notifies every pump, so the compare keeps sync cheap.
    playing_path: Option<PathBuf>,
    /// The playing track as its library id, the rows' key.
    playing: Option<i64>,
    /// The track under the last right press, for the context menu: the
    /// builder gets no position, so the press records it (the grid keys
    /// off hover for the same reason).
    menu_row: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _history_changed: Subscription,
    _library_changed: Subscription,
    _player_changed: Subscription,
    _thumbs_changed: Subscription,
}

impl HistoryPanel {
    pub fn new(state: AppState, config: HistoryConfig, cx: &mut Context<Self>) -> Self {
        let _history_changed = cx.subscribe(
            &state.history,
            |this: &mut Self, _, _: &HistoryEvent, cx| this.refresh(cx),
        );
        // A landing cover repaints the heading tiles and the cover column.
        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut Self, _, cx| cx.notify());
        // A rescan retags tracks and grows the never-played set; a rating or
        // favourite change moves those columns.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(
                    event,
                    LibraryEvent::Updated | LibraryEvent::Rated | LibraryEvent::PlaylistsChanged
                ) {
                    this.refresh(cx);
                }
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        let mut this = HistoryPanel {
            state,
            config,
            tracks: Vec::new(),
            rows: Vec::new(),
            albums: Vec::new(),
            favourites: std::collections::HashSet::new(),
            selected: None,
            playing_path: None,
            playing: None,
            menu_row: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _history_changed,
            _library_changed,
            _player_changed,
            _thumbs_changed,
        };
        this.refresh(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Follow the player: on a track change, resolve the playing path to
    /// its id (one store lookup), the library panel's move. The highlight
    /// matches rows by that id, so in the recent view every listen of the
    /// playing track carries it.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if path == self.playing_path {
            return;
        }
        self.playing_path = path;
        self.playing = self
            .playing_path
            .as_ref()
            .and_then(|path| self.state.library.read(cx).id_for(path));
        cx.notify();
    }

    /// Re-read the current view's tracks off the events table, then lay out
    /// the display rows.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let library = self.state.library.read(cx);
        self.tracks = match self.config.view {
            HistoryView::Recent => library.recent_listens(0, ROWS_CAP),
            HistoryView::Most => library.most_played(ROWS_CAP),
            HistoryView::Never => library.never_played(ROWS_CAP),
        };
        self.favourites = library.favourite_ids();
        self.selected = None;
        self.menu_row = None;
        self.rebuild_rows();
        cx.notify();
    }

    /// Whether the album headings apply: on, and only in the Recent view,
    /// where the order is at least chronological. Most and Never never keep
    /// an album's tracks consecutive, so the headings would be noise.
    fn grouping(&self) -> bool {
        self.config.view == HistoryView::Recent && self.config.headers != Headers::Off
    }

    /// Lay the display rows over `tracks`: flat, or broken into album runs
    /// with a heading over each. Only the display shape changes, so a headings
    /// or column flip that leaves the tracks alone calls this, not `refresh`.
    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut albums = Vec::new();
        if !self.grouping() {
            rows.extend((0..self.tracks.len() as u32).map(Row::Track));
            self.rows = rows;
            self.albums = albums;
            return;
        }
        let mut i = 0;
        while i < self.tracks.len() {
            let mut j = i + 1;
            while j < self.tracks.len() && self.tracks[j].album == self.tracks[i].album {
                j += 1;
            }
            let group: Vec<GroupTrack> = self.tracks[i..j].iter().map(group_track).collect();
            albums.push(track_columns::album_group(&group));
            let g = (albums.len() - 1) as u32;
            rows.push(Row::Album(g));
            if self.config.headers == Headers::Expanded {
                rows.push(Row::AlbumMeta(g));
            }
            rows.extend((i..j).map(|ti| Row::Track(ti as u32)));
            i = j;
        }
        self.rows = rows;
        self.albums = albums;
    }

    fn set_view(&mut self, view: HistoryView, cx: &mut Context<Self>) {
        if self.config.view == view {
            return;
        }
        self.config.view = view;
        self.refresh(cx);
    }

    /// A single click selects a track: the highlight here, its track id on
    /// the shared selection for the panels that display it. `ti` indexes
    /// `tracks`, not the display rows.
    fn select(&mut self, ti: usize, cx: &mut Context<Self>) {
        let Some(track) = self.tracks.get(ti) else {
            return;
        };
        self.selected = Some(ti);
        let ids = vec![track.track_id];
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
        cx.notify();
    }

    /// A double click queues the track with the surrounding view as its
    /// timeline: earlier tracks seed behind the cursor for Prev, later ones
    /// carry Next, the clicked track plays. Bounded to a window around the
    /// click with a share kept for history. A track deleted since its event
    /// resolves to no path and drops out of the queue quietly.
    fn play_from(&mut self, ti: usize, cx: &mut Context<Self>) {
        let lo = ti
            .saturating_sub(QUEUE_CAP / 2)
            .min(self.tracks.len().saturating_sub(QUEUE_CAP));
        let hi = (lo + QUEUE_CAP).min(self.tracks.len());
        let ids: Vec<i64> = self.tracks[lo..hi].iter().map(|t| t.track_id).collect();
        let Ok(paths) = self.state.library.read(cx).paths_for(&ids) else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        let start = ti - lo;
        self.state
            .player
            .update(cx, |player, cx| player.play_at(paths, start, cx));
    }

    /// The visible slice of the list: album headings (Recent view) and track
    /// rows, drawn through the shared column surface.
    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        range
            .filter_map(|ix| {
                Some(match self.rows.get(ix)? {
                    Row::Album(g) => {
                        let g = *g;
                        let headers = self.config.headers;
                        track_columns::album_name_row(
                            ix,
                            &mut self.albums[g as usize],
                            headers,
                            &self.state,
                            cx,
                        )
                    }
                    Row::AlbumMeta(g) => {
                        let g = *g;
                        track_columns::album_meta_row(ix, &mut self.albums[g as usize], &self.state, cx)
                    }
                    Row::Track(ti) => {
                        let ti = *ti as usize;
                        self.track_row(ix, ti, now, cx)
                    }
                })
            })
            .collect()
    }

    /// One track row: its interactions keyed on the track index, its cells
    /// the shown columns - the shared ones plus the record's own Plays and
    /// Last Played.
    fn track_row(&self, ix: usize, ti: usize, now: i64, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = &self.tracks[ti];
        let playing = self.playing == Some(t.track_id);
        let selected = self.selected == Some(ti);
        let favourite = self.favourites.contains(&t.track_id);
        let mut row = div()
            .id(("history-row", ix))
            // The hover group the rating and favourite cells reveal on.
            .group(track_cells::ROW_GROUP)
            .w_full()
            .h(px(ROW_H))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
            // The playing track wears the highlight role, a faint cut apart
            // from the accent-washed selection, the library's look.
            .when(playing && !selected, |d| {
                d.bg(palette::alpha(palette::highlight(), 0x12))
            })
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.click_count > 1 {
                        this.play_from(ti, cx);
                    } else {
                        this.select(ti, cx);
                    }
                }),
            )
            // The right press records the track and, outside the selection,
            // reselects it, so the menu acts on what is highlighted.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.menu_row = Some(ti);
                    if this.selected != Some(ti) {
                        this.select(ti, cx);
                    }
                }),
            );
        let cover = track_columns::cover_thumb(
            &self.state,
            (!t.path.is_empty()).then(|| std::path::Path::new(&t.path)),
            self.column_shown("cover"),
            cx,
        );
        let cell = track_columns::Cell {
            pos: (ti + 1) as u32,
            title: &t.title,
            artist: &t.artist,
            album: &t.album,
            year: t.year,
            genre: &t.genre,
            duration_ms: t.duration_ms,
            rating: t.rating,
            track_id: t.track_id,
            favourite,
            playing,
            plays: t.plays as u32,
            cover,
        };
        for col in COLUMNS {
            if !self.column_shown(col.key) {
                continue;
            }
            let c = match track_columns::cell(col.key, &cell, &self.state) {
                Some(c) => c,
                // Last Played is the record's own column; the rest, plays
                // included, are shared. Blank when there is nothing to say.
                None => match col.key {
                    "lastplayed" => muted_cell(if t.last_played == 0 {
                        String::new()
                    } else {
                        fmt_ago(now - t.last_played)
                    }),
                    _ => continue,
                },
            };
            row = row.child(c);
        }
        row
    }

    /// The Display section: the view pick, the columns, and - on the Recent
    /// view only, where the order keeps albums together - the headings, the
    /// same knobs the settings window edits.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let panel = cx.entity();
        let view = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            for view in [HistoryView::Recent, HistoryView::Most, HistoryView::Never] {
                submenu = submenu.item(panel::check_row(
                    view.label(),
                    None,
                    move |this: &Self| this.config.view == view,
                    move |this, cx| this.set_view(view, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu
            .label("Display")
            .item(PopupMenuItem::submenu("View", view))
            .item(PopupMenuItem::submenu(
                "Columns",
                track_columns::columns_submenu(COLUMNS, window, cx),
            ));
        if self.config.view == HistoryView::Recent {
            menu.item(PopupMenuItem::submenu(
                "Headings",
                track_columns::headings_submenu(window, cx),
            ))
        } else {
            menu
        }
    }
}

impl ColumnHost for HistoryPanel {
    fn column_shown(&self, key: &str) -> bool {
        self.config.columns.iter().any(|k| k == key)
    }

    fn set_column(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>) {
        let has = self.column_shown(key);
        if on && !has {
            self.config.columns.push(key.to_string());
        } else if !on {
            self.config.columns.retain(|k| k != key);
        }
        cx.notify();
    }
}

impl HeadingHost for HistoryPanel {
    fn headers(&self) -> Headers {
        self.config.headers
    }

    /// Set the heading mode and relay out the rows; the tracks are unchanged,
    /// so no re-query, just a fresh row plan.
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>) {
        if self.config.headers == headers {
            return;
        }
        self.config.headers = headers;
        self.rebuild_rows();
        cx.notify();
    }
}

impl PanelSettings for HistoryPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Content", icons::CLOCK)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let recent = self.config.view == HistoryView::Recent;
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "View",
                Some("Which cut of the listen record the panel shows"),
                panel::choices(
                    &[
                        ("Recent", HistoryView::Recent),
                        ("Most Played", HistoryView::Most),
                        ("Never Played", HistoryView::Never),
                    ],
                    self.config.view,
                    |this: &mut Self, view, cx| this.set_view(view, cx),
                    cx,
                ),
            ))
            .child(panel::setting_block(
                "Columns",
                Some("Which track columns show"),
                None,
                track_columns::checklist(COLUMNS, self, cx),
            ))
            // The album orders only stay together in the Recent view; the
            // headings are off the table on Most and Never.
            .when(recent, |d| {
                d.child(panel::setting_row(
                    "Headings",
                    Some("Break the recent list into album runs; Expanded adds the cover and stats"),
                    panel::choices(
                        &[
                            ("Off", Headers::Off),
                            ("Compact", Headers::Compact),
                            ("Expanded", Headers::Expanded),
                        ],
                        self.config.headers,
                        |this: &mut Self, headers, cx| this.set_headers(headers, cx),
                        cx,
                    ),
                ))
            })
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for HistoryPanel {}

impl Focusable for HistoryPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for HistoryPanel {
    fn panel_name(&self) -> &'static str {
        "history"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "History")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The body serves its own row context menus, so the tab panel's body
    /// right-click stays out; the panel dropdown lives on the tab and
    /// rides along after the track actions.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The config block: the panel's quick entries and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, window, cx);
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the config along, like the metadata's.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| HistoryPanel::new(state, config, cx));
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

impl Render for HistoryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl HistoryPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let root = div().size_full().flex().flex_col().bg(palette::bg_root());
        let content = if self.tracks.is_empty() {
            div().flex_1().min_h_0().flex().flex_col().child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_faint())
                    .child(match self.config.view {
                        HistoryView::Never => "Every track has been played",
                        _ => "No listens yet",
                    }),
            )
        } else {
            let this = cx.entity().downgrade();
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .px(tokens::SPACE_SM)
                        .py(tokens::SPACE_XS)
                        .border_b_1()
                        .border_color(palette::border())
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(self.config.view.label()),
                )
                .child(
                    uniform_list("history-rows", self.rows.len(), move |range, _, cx| {
                        this.upgrade()
                            .map(|this| this.update(cx, |this, cx| this.list_rows(range, cx)))
                            .unwrap_or_default()
                    })
                    .track_scroll(self.scroll.clone())
                    .flex_1()
                    .w_full(),
                )
        };
        // A right press lands here in the capture phase, before any row's
        // bubble handler records itself, so a press off the rows leaves
        // no target and the menu below falls back to the panel's own.
        let content =
            content.capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.menu_row = None;
                }
            }));
        // The row context menu: the track actions every song surface
        // shares, then the panel menu riding along after, so a click
        // over the list never dead-ends at Play.
        let weak = cx.entity().downgrade();
        root.child(content.context_menu(move |menu, window, cx| {
            let Some(this) = weak.upgrade() else {
                return menu;
            };
            let target = {
                let panel = this.read(cx);
                panel
                    .menu_row
                    .and_then(|ti| panel.tracks.get(ti).map(|t| (ti, t.track_id)))
            };
            let Some((ti, id)) = target else {
                return this.update(cx, |this, cx| this.dropdown_menu(menu, window, cx));
            };
            let state = this.read(cx).state.clone();
            let panel = weak.clone();
            // Play queues the track and what follows in the view's order,
            // the double click's move.
            let menu =
                panel::track_actions(menu, state, vec![id], "Play", window, cx, move |_, cx| {
                    if let Some(this) = panel.upgrade() {
                        this.update(cx, |this, cx| this.play_from(ti, cx));
                    }
                });
            this.update(cx, |this, cx| {
                this.dropdown_menu(menu.separator(), window, cx)
            })
        }))
    }
}

/// A trailing muted column: the record's own Plays and Last Played, right
/// of the flexible text columns.
fn muted_cell(text: String) -> Div {
    div()
        .flex_none()
        .text_color(palette::text_muted())
        .child(SharedString::from(text))
}

/// A listen's age as a short readout: seconds up through years, one
/// unit, no calendar math. The stats panel's recents read it too.
pub fn fmt_ago(secs: i64) -> String {
    let secs = secs.max(0);
    let (value, unit) = match secs {
        s if s < 60 => return "just now".into(),
        s if s < 3600 => (s / 60, "m"),
        s if s < 86400 => (s / 3600, "h"),
        s if s < 86400 * 7 => (s / 86400, "d"),
        s if s < 86400 * 365 => (s / (86400 * 7), "w"),
        s => (s / (86400 * 365), "y"),
    };
    format!("{value}{unit} ago")
}
