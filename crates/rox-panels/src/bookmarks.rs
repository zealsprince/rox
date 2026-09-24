//! The bookmarks panel: every mark in the library, listed under the track
//! it sits in, in browse order. Tracks draw with the shared track columns;
//! marks sit indented under them. A mark's right-click menu is the seek
//! strip's chevron menu with a play row ahead of it. Rows read off the
//! library at panel-open and bookmark-edit cadence, never per frame.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Modifiers,
    MouseButton, MouseDownEvent, SharedString, Stateful, Subscription, UniformListScrollHandle,
    WeakEntity, Window, div, prelude::*, px, uniform_list,
};
use gpui_component::Icon;
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use rox_core::fmt::{fmt_ago, fmt_time};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::bookmarks::{Bookmark, BookmarkRow};
use rox_library::cue::{TrackKey, local};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::bookmark_ui;
use crate::catalog::{LibraryEvent, SortNames};
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::track_ui::track_cells;
use crate::track_ui::track_columns::{self, Column, ColumnHost};

/// One height for track and mark rows: the list is a uniform_list.
const ROW_H: f32 = track_columns::ROW_HEIGHT_STOCK;

/// Rebuilt per call so a locale switch relabels.
fn columns() -> Vec<Column> {
    vec![
        Column {
            key: "cover",
            label: rox_i18n::t!("columns-cover"),
            default_on: true,
        },
        Column {
            key: "number",
            label: rox_i18n::t!("columns-number"),
            default_on: false,
        },
        Column {
            key: "name",
            label: rox_i18n::t!("columns-name"),
            default_on: true,
        },
        Column {
            key: "artist",
            label: rox_i18n::t!("head-piece-artist"),
            default_on: true,
        },
        Column {
            key: "album",
            label: rox_i18n::t!("head-piece-album"),
            default_on: true,
        },
        Column {
            key: "year",
            label: rox_i18n::t!("head-piece-year"),
            default_on: false,
        },
        Column {
            key: "genre",
            label: rox_i18n::t!("head-piece-genre"),
            default_on: false,
        },
        Column {
            key: "duration",
            label: rox_i18n::t!("info-item-duration"),
            default_on: true,
        },
        Column {
            key: "rating",
            label: rox_i18n::t!("info-item-rating"),
            default_on: false,
        },
        Column {
            key: "favourite",
            label: rox_i18n::t!("info-item-favourite"),
            default_on: false,
        },
    ]
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BookmarksConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub columns: Vec<String>,
}

impl Default for BookmarksConfig {
    fn default() -> Self {
        BookmarksConfig {
            chrome: PanelChrome::default(),
            columns: track_columns::default_columns(&columns()),
        }
    }
}

struct Group {
    key: TrackKey,
    track: BookmarkRow,
    marks: Vec<Bookmark>,
}

#[derive(Clone, Copy)]
enum Row {
    Track(usize),
    Mark(usize, usize),
}

pub struct BookmarksPanel {
    state: AppState,
    config: BookmarksConfig,
    groups: Vec<Group>,
    rows: Vec<Row>,
    readings: HashMap<i64, SortNames>,
    favourites: HashSet<i64>,
    scroll: UniformListScrollHandle,
    selected: HashSet<usize>,
    anchor: Option<usize>,
    menu_row: Option<Row>,
    /// The playing track, so the observe only repaints on a track change.
    playing: Option<TrackKey>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _player_changed: Subscription,
}

impl BookmarksPanel {
    pub fn new(state: AppState, config: BookmarksConfig, cx: &mut Context<Self>) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| match event {
                LibraryEvent::BookmarksChanged | LibraryEvent::Updated => this.refresh(cx),
                LibraryEvent::Rated | LibraryEvent::PlaylistsChanged => this.refresh(cx),
                _ => {}
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            let playing = this.state.player.read(cx).now_playing().map(|now| now.key);
            if this.playing != playing {
                this.playing = playing;
                cx.notify();
            }
        });
        let mut panel = BookmarksPanel {
            playing: state.player.read(cx).now_playing().map(|now| now.key),
            state,
            config,
            groups: Vec::new(),
            rows: Vec::new(),
            readings: HashMap::new(),
            favourites: HashSet::new(),
            scroll: UniformListScrollHandle::new(),
            selected: HashSet::new(),
            anchor: None,
            menu_row: None,
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _library_changed,
            _player_changed,
        };
        panel.refresh(cx);
        panel
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let library = self.state.library.read(cx);
        let rows = library.all_bookmarks();
        let mut groups: Vec<Group> = Vec::new();
        for row in rows {
            // Bookmarks are dropped on a file, so a mark is always local.
            let key = TrackKey {
                source: local(),
                path: row.path.clone().into(),
                sub: row.sub,
            };
            match groups.last_mut() {
                Some(group) if group.key == key => group.marks.push(row.bookmark.clone()),
                _ => {
                    let mark = row.bookmark.clone();
                    groups.push(Group {
                        key,
                        track: row,
                        marks: vec![mark],
                    })
                }
            }
        }
        self.readings = groups
            .iter()
            .map(|g| {
                (
                    g.track.track_id,
                    library.sort_names_for_id(g.track.track_id),
                )
            })
            .filter(|(_, sort)| {
                !sort.title.is_empty() || !sort.artist.is_empty() || !sort.album.is_empty()
            })
            .collect();
        self.favourites = library.favourite_ids();
        self.rows = groups
            .iter()
            .enumerate()
            .flat_map(|(g, group)| {
                std::iter::once(Row::Track(g))
                    .chain((0..group.marks.len()).map(move |m| Row::Mark(g, m)))
            })
            .collect();
        self.groups = groups;
        // The rows moved under the indices, so nothing selected survives.
        self.selected.clear();
        self.anchor = None;
        self.menu_row = None;
        cx.notify();
    }

    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        if ix >= self.rows.len() {
            return;
        }
        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(ix);
            let (lo, hi) = (anchor.min(ix), anchor.max(ix));
            if modifiers.secondary() {
                self.selected.extend(lo..=hi);
            } else {
                self.selected = (lo..=hi).collect();
            }
            if self.anchor.is_none() {
                self.anchor = Some(ix);
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(ix) {
                self.selected.remove(&ix);
            }
            self.anchor = Some(ix);
        } else {
            self.selected = HashSet::from([ix]);
            self.anchor = Some(ix);
        }
        self.publish_selection(cx);
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        self.anchor = Some(0);
        self.selected = (0..self.rows.len()).collect();
        self.publish_selection(cx);
        cx.notify();
    }

    fn deselect(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        self.anchor = None;
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(Vec::new(), source, cx));
        cx.notify();
    }

    /// A mark stands for the track it sits in.
    fn selected_track_ids(&self) -> Vec<i64> {
        let mut seen = HashSet::new();
        self.rows
            .iter()
            .enumerate()
            .filter(|(ix, _)| self.selected.contains(ix))
            .filter_map(|(_, row)| {
                let g = match row {
                    Row::Track(g) | Row::Mark(g, _) => *g,
                };
                let id = self.groups.get(g)?.track.track_id;
                seen.insert(id).then_some(id)
            })
            .collect()
    }

    fn selected_mark_ids(&self) -> Vec<i64> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(ix, _)| self.selected.contains(ix))
            .filter_map(|(_, row)| match row {
                Row::Mark(g, m) => Some(self.groups.get(*g)?.marks.get(*m)?.id),
                _ => None,
            })
            .collect()
    }

    fn publish_selection(&self, cx: &mut Context<Self>) {
        let ids = self.selected_track_ids();
        if ids.is_empty() {
            return;
        }
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if modifiers.secondary() && key == "a" {
            self.select_all(cx);
        } else if key == "escape" {
            self.deselect(cx);
        } else if key == "delete" || key == "backspace" {
            let ids = self.selected_mark_ids();
            if !ids.is_empty() {
                remove_marks(&self.state, &ids, cx);
            }
        }
    }

    fn right_press(&mut self, ix: usize, row: Row, cx: &mut Context<Self>) {
        self.menu_row = Some(row);
        if !self.selected.contains(&ix) {
            self.select(ix, Modifiers::default(), cx);
        }
    }

    fn play_mark(&self, group: usize, mark: usize, cx: &mut Context<Self>) {
        let Some(group) = self.groups.get(group) else {
            return;
        };
        let Some(mark) = group.marks.get(mark) else {
            return;
        };
        let key = group.key.clone();
        let secs = mark.position_ms as f64 / 1000.0;
        self.state
            .player
            .update(cx, |player, cx| player.play_now_at(key, secs, cx));
    }

    fn play_track(&self, group: usize, cx: &mut Context<Self>) {
        let Some(group) = self.groups.get(group) else {
            return;
        };
        let key = group.key.clone();
        self.state
            .player
            .update(cx, |player, cx| player.play_now(vec![key], cx));
    }

    fn list_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        range
            .filter_map(|ix| self.rows.get(ix).map(|row| (ix, *row)))
            .map(|(ix, row)| match row {
                Row::Track(g) => self.track_row(ix, g, cx).into_any_element(),
                Row::Mark(g, m) => self.mark_row(ix, g, m, now, cx).into_any_element(),
            })
            .collect()
    }

    fn row_base(&self, ix: usize, playing: bool) -> Stateful<Div> {
        let selected = self.selected.contains(&ix);
        div()
            .id(("bookmark-row", ix))
            .group(track_cells::ROW_GROUP)
            .w_full()
            .h(palette::scaled_px(ROW_H))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
            .when(playing && !selected, |d| {
                d.bg(palette::alpha(palette::highlight(), 0x12))
            })
            .hover(|d| d.bg(palette::bg_control_hover()))
    }

    fn track_row(&self, ix: usize, g: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let group = &self.groups[g];
        let t = &group.track;
        let playing = self.playing.as_ref() == Some(&group.key);
        let mut row = self
            .row_base(ix, playing)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    if event.click_count > 1 {
                        this.play_track(g, cx);
                    } else {
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.right_press(ix, Row::Track(g), cx);
                }),
            );
        let cover = track_columns::cover_thumb(
            &self.state,
            Some(std::path::Path::new(&t.path)),
            self.column_shown("cover"),
            cx,
        );
        let sort = self.readings.get(&t.track_id);
        let cell = track_columns::Cell {
            pos: t.track_no as u32,
            title: &t.title,
            artist: &t.artist,
            album: &t.album,
            title_reading: sort.map(|s| s.title.as_str()).unwrap_or(""),
            artist_reading: sort.map(|s| s.artist.as_str()).unwrap_or(""),
            album_reading: sort.map(|s| s.album.as_str()).unwrap_or(""),
            year: t.year,
            genre: &t.genre,
            duration_ms: t.duration_ms,
            rating: t.rating,
            track_id: t.track_id,
            favourite: self.favourites.contains(&t.track_id),
            playing,
            plays: 0,
            cover,
        };
        for col in columns() {
            if !self.column_shown(col.key) {
                continue;
            }
            if let Some(c) = track_columns::cell(col.key, &cell, &self.state, ROW_H, false) {
                row = row.child(c);
            }
        }
        row
    }

    fn mark_row(
        &self,
        ix: usize,
        g: usize,
        m: usize,
        now: i64,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let group = &self.groups[g];
        let mark = &group.marks[m];
        let playing = self.playing.as_ref() == Some(&group.key);
        let color = bookmark_ui::color_of(mark.color.as_deref());
        let label = bookmark_ui::mark_label(&mark.name, mark.position_ms);
        let named = !mark.name.trim().is_empty();
        let time = fmt_time(mark.position_ms as f64 / 1000.0);
        self.row_base(ix, playing)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    if event.click_count > 1 {
                        this.play_mark(g, m, cx);
                    } else {
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.right_press(ix, Row::Mark(g, m), cx);
                }),
            )
            // The indent lines the dot up under the track's name, past the
            // cover square when it shows.
            .child(
                div()
                    .flex_none()
                    .w(px(if self.column_shown("cover") {
                        ROW_H + 8.0
                    } else {
                        tokens::SPACE_MD.into()
                    }))
                    .flex()
                    .justify_end()
                    .child(
                        Icon::default()
                            .path(icons::BOOKMARK)
                            .size(px(12.))
                            .text_color(color),
                    ),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(named, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_sm()
                        .text_color(palette::text_muted())
                        .child(time),
                )
            })
            .child(
                div()
                    .flex_none()
                    .w(px(72.))
                    .text_right()
                    .text_sm()
                    .text_color(palette::text_faint())
                    .child(fmt_ago(now - mark.created)),
            )
    }

    /// Lifted out of the panel so the menu builds without holding it borrowed.
    fn menu_target(&self, row: Row) -> Option<MenuTarget> {
        match row {
            Row::Track(g) => {
                let group = self.groups.get(g)?;
                let mut ids = self.selected_track_ids();
                if ids.is_empty() {
                    ids.push(group.track.track_id);
                }
                Some(MenuTarget::Track {
                    key: group.key.clone(),
                    ids,
                })
            }
            Row::Mark(g, m) => {
                let group = self.groups.get(g)?;
                let mark = group.marks.get(m)?;
                let mut ids = self.selected_mark_ids();
                if ids.is_empty() {
                    ids.push(mark.id);
                }
                Some(MenuTarget::Mark {
                    key: group.key.clone(),
                    secs: mark.position_ms as f64 / 1000.0,
                    id: mark.id,
                    ids,
                })
            }
        }
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let root = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)));
        if self.rows.is_empty() {
            return root.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(tokens::SPACE_MD)
                    .text_sm()
                    .text_center()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("bookmarks-empty")),
            );
        }
        let this = cx.entity().downgrade();
        let list = uniform_list("bookmark-rows", self.rows.len(), move |range, _, cx| {
            this.upgrade()
                .map(|this| this.update(cx, |this, cx| this.list_rows(range, cx)))
                .unwrap_or_default()
        })
        .track_scroll(self.scroll.clone())
        .flex_1()
        .w_full();
        let weak = cx.entity().downgrade();
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .child(list)
                .context_menu(move |menu, window, cx| {
                    let Some(this) = weak.upgrade() else {
                        return menu;
                    };
                    let target = {
                        let panel = this.read(cx);
                        panel
                            .menu_row
                            .and_then(|row| panel.menu_target(row))
                            .map(|target| (target, panel.state.clone()))
                    };
                    match target {
                        Some((target, state)) => row_menu(menu, state, target, window, cx),
                        None => menu,
                    }
                }),
        )
    }
}

enum MenuTarget {
    Track {
        key: TrackKey,
        ids: Vec<i64>,
    },
    Mark {
        key: TrackKey,
        secs: f64,
        id: i64,
        ids: Vec<i64>,
    },
}

fn remove_marks(state: &AppState, ids: &[i64], cx: &mut App) {
    state.library.update(cx, |library, cx| {
        for &id in ids {
            library.remove_bookmark(id, cx);
        }
    });
}

fn row_menu(
    menu: PopupMenu,
    state: AppState,
    target: MenuTarget,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    match target {
        MenuTarget::Track { key, ids } => {
            let player = state.player.clone();
            panel::track_actions(
                menu,
                state,
                ids,
                rox_i18n::t!("library-play"),
                window,
                cx,
                move |_, cx| {
                    player.update(cx, |player, cx| player.play_now(vec![key.clone()], cx));
                },
            )
        }
        MenuTarget::Mark { key, secs, id, ids } => {
            let player = state.player.clone();
            let play_key = key.clone();
            let menu = menu
                .item(
                    PopupMenuItem::new(rox_i18n::t!("bookmark-menu-play"))
                        .icon(Icon::default().path(icons::PLAY))
                        .on_click(move |_, _, cx| {
                            player.update(cx, |player, cx| {
                                player.play_now_at(play_key.clone(), secs, cx)
                            });
                        }),
                )
                .separator();
            if ids.len() < 2 {
                return bookmark_ui::menu_for(menu, state, key, id, window, cx);
            }
            let menu = bookmark_ui::color_submenu(menu, state.clone(), ids.clone(), window, cx);
            let count = ids.len();
            menu.separator().item(
                PopupMenuItem::new(rox_i18n::t!("bookmark-menu-remove-many", count = count))
                    .icon(Icon::default().path(icons::TRASH))
                    .on_click(move |_, _, cx| remove_marks(&state, &ids, cx)),
            )
        }
    }
}

impl ColumnHost for BookmarksPanel {
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

impl PanelSettings for BookmarksPanel {
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
        &[("View", icons::ROWS_3)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_block(
                rox_i18n::t!("library-columns"),
                Some(rox_i18n::t!("panel-columns-description")),
                None,
                track_columns::checklist(&columns(), self, cx),
            ))
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for BookmarksPanel {}

impl Focusable for BookmarksPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for BookmarksPanel {
    fn panel_name(&self) -> &'static str {
        "bookmarks"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("bookmarks-title"),
        )
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

    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

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
        let menu = menu
            .label(rox_i18n::t!("panel-menu-display"))
            .item(PopupMenuItem::submenu(
                rox_i18n::t!("library-columns"),
                track_columns::columns_submenu(columns(), window, cx),
            ))
            .separator();
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        panel_settings::settings_item(menu, &cx.entity(), cx)
    }
}

impl Render for BookmarksPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}
