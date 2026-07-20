//! The playlists panel (ADR 16): a tree of playlists, each expanding to its
//! tracks. A track plays the playlist from that point on double click, drops
//! from the right-click menu, and drags to another playlist to move there or
//! within its own to reorder. Playlists rename and delete from their own
//! right-click, and New Playlist lives in the panel menu. Its own panel, never
//! a mode of the library.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, svg, uniform_list, App, Context, Div, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, PathPromptOptions,
    SharedString, Stateful, Subscription, UniformListScrollHandle, WeakEntity, Window,
};
use gpui_component::button::Button;
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;

/// One row's height; the list is a uniform_list, so every row agrees.
const ROW_H: f32 = 30.;

/// The playlists panel's config: just the shared chrome, and which playlists
/// are expanded so a saved layout restores the open ones.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistsConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub expanded: Vec<i64>,
}

/// A flattened tree row: a playlist header, or one of its tracks.
enum Row {
    Head {
        id: i64,
        name: String,
        count: u64,
        expanded: bool,
        /// The one default playlist behind the heart column: shown with a
        /// heart, shielded from rename and delete.
        favourite: bool,
    },
    Track {
        playlist_id: i64,
        member_id: i64,
        track_id: i64,
        title: String,
        artist: String,
    },
}

/// A dragged set of members, in view order, and the grabbed row's title for
/// the preview. Dragging a row inside a multi-selection carries the whole set;
/// outside it, just that row. Where they land is the drop target's call, so no
/// source playlist rides along.
#[derive(Clone)]
struct TrackDrag {
    members: Vec<i64>,
    title: SharedString,
}

/// The label that floats under the pointer while tracks are dragged. A
/// multi-row drag shows the grabbed title with a count of the rest.
struct TrackDragPreview {
    title: SharedString,
    extra: usize,
}

impl Render for TrackDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.extra > 0 {
            SharedString::from(format!("{} +{}", self.title, self.extra))
        } else {
            self.title.clone()
        };
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .text_color(palette::text())
            .child(label)
    }
}

pub struct PlaylistsPanel {
    state: AppState,
    config: PlaylistsConfig,
    rows: Vec<Row>,
    /// The expanded playlist ids, mirrored into the config on every change.
    expanded: HashSet<i64>,
    /// The playing track's library id, for the row highlight.
    playing: Option<i64>,
    /// The selected members, by row id. Keyed on the member id, not the row
    /// index, so a rescan, an expand, or a reorder rebuilds the tree without
    /// dropping the highlight. Shift extends, cmd (ctrl elsewhere) toggles,
    /// Ctrl+A takes the lot, the library's click rules.
    selected: HashSet<i64>,
    /// Where the next shift-click extends from: the last plain or toggle pick,
    /// held as a member id so it survives a rebuild too.
    anchor: Option<i64>,
    menu_row: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _player_changed: Subscription,
}

impl PlaylistsPanel {
    pub fn new(state: AppState, config: PlaylistsConfig, cx: &mut Context<Self>) -> Self {
        let expanded: HashSet<i64> = config.expanded.iter().copied().collect();
        // Playlist edits and rescans both change what the tree shows.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(
                    event,
                    LibraryEvent::PlaylistsChanged | LibraryEvent::Updated
                ) {
                    this.refresh(cx);
                }
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        let mut this = PlaylistsPanel {
            state,
            config,
            rows: Vec::new(),
            expanded,
            playing: None,
            selected: HashSet::new(),
            anchor: None,
            menu_row: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _library_changed,
            _player_changed,
        };
        this.refresh(cx);
        this.sync_playing(cx);
        this
    }

    /// Rebuild the flattened tree from the catalog: a header per playlist, its
    /// tracks under it when expanded.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let library = self.state.library.read(cx);
        let mut rows = Vec::new();
        for playlist in library.playlists() {
            let expanded = self.expanded.contains(&playlist.id);
            rows.push(Row::Head {
                id: playlist.id,
                name: playlist.name,
                count: playlist.tracks,
                expanded,
                favourite: playlist.favourite,
            });
            if expanded {
                for track in library.playlist_tracks(playlist.id) {
                    rows.push(Row::Track {
                        playlist_id: playlist.id,
                        member_id: track.member_id,
                        track_id: track.track_id,
                        title: track.title,
                        artist: track.artist,
                    });
                }
            }
        }
        self.rows = rows;
        // Keep only members that still exist; a removed track drops out of the
        // selection, a moved one stays lit at its new spot.
        let live: HashSet<i64> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track { member_id, .. } => Some(*member_id),
                _ => None,
            })
            .collect();
        self.selected.retain(|member| live.contains(member));
        if self.anchor.is_some_and(|a| !live.contains(&a)) {
            self.anchor = None;
        }
        self.menu_row = None;
        cx.notify();
    }

    /// Follow the player: resolve the playing path to its track id, so every
    /// row of that track across playlists carries the highlight.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let playing = self
            .state
            .player
            .read(cx)
            .now_playing()
            .and_then(|now| self.state.library.read(cx).id_for(&now.path));
        if playing != self.playing {
            self.playing = playing;
            cx.notify();
        }
    }

    /// Expand or collapse a playlist, mirroring the set into the config so a
    /// layout dump keeps it.
    fn toggle(&mut self, id: i64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.config.expanded = self.expanded.iter().copied().collect();
        self.refresh(cx);
    }

    /// Start the playlist playing, from `start_track` when given (a double
    /// click on a row), from the top otherwise (the header's Play).
    fn play(&self, playlist_id: i64, start_track: Option<i64>, cx: &mut Context<Self>) {
        let (paths, start) = {
            let library = self.state.library.read(cx);
            let ids = library.playlist_ids(playlist_id);
            let start = start_track
                .and_then(|t| ids.iter().position(|&x| x == t))
                .unwrap_or(0);
            (library.paths_for(&ids).unwrap_or_default(), start)
        };
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play_at(paths, start, cx));
    }

    /// Write a playlist to an M3U8 file the user picks, named after it by
    /// default. Only playable members land in it; a deleted track has no file
    /// to point at.
    fn export(&self, playlist_id: i64, name: String, cx: &mut Context<Self>) {
        let rows = self
            .state
            .library
            .read(cx)
            .playlist_export_rows(playlist_id);
        if rows.is_empty() {
            return;
        }
        let text = crate::m3u::to_m3u8(&rows);
        let home = dirs::home_dir().unwrap_or_default();
        let file = format!("{name}.m3u8");
        let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));
        cx.spawn(async move |_, _| {
            if let Ok(Ok(Some(path))) = rx.await {
                std::fs::write(path, text).ok();
            }
        })
        .detach();
    }

    /// Pick an M3U file and load it as a new playlist named after the file.
    /// Entries resolve to catalog tracks, relative paths against the file's
    /// folder; paths the library never scanned are skipped.
    fn import(&self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                return;
            };
            let entries = crate::m3u::parse(&text);
            if entries.is_empty() {
                return;
            }
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Imported".into());
            let base = path
                .parent()
                .map(|dir| dir.to_path_buf())
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                this.state.library.update(cx, |library, cx| {
                    library.import_playlist(&name, &base, &entries, cx);
                });
            })
            .ok();
        })
        .detach();
    }

    /// The member id at a row, if it is a track row.
    fn member_at(&self, ix: usize) -> Option<i64> {
        match self.rows.get(ix) {
            Some(Row::Track { member_id, .. }) => Some(*member_id),
            _ => None,
        }
    }

    /// The row index of a member, if it is still on screen.
    fn index_of(&self, member: i64) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Track { member_id, .. } if *member_id == member))
    }

    /// The selected members in view order, so a drag or remove keeps the order
    /// you see rather than a set's arbitrary one.
    fn selected_members(&self) -> Vec<i64> {
        self.rows
            .iter()
            .filter_map(|row| match row {
                Row::Track { member_id, .. } if self.selected.contains(member_id) => {
                    Some(*member_id)
                }
                _ => None,
            })
            .collect()
    }

    /// Put a click on a track row: plain selects just it, shift extends from
    /// the anchor over the tracks between, cmd (ctrl elsewhere) toggles - the
    /// library's click rules. Publishes the selection either way.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(member) = self.member_at(ix) else {
            return;
        };
        if modifiers.shift {
            let anchor_ix = self.anchor.and_then(|a| self.index_of(a)).unwrap_or(ix);
            let (lo, hi) = (anchor_ix.min(ix), anchor_ix.max(ix));
            // Only track rows in the span, so a header caught between two
            // playlists is skipped rather than selected.
            self.selected = self.rows[lo..=hi]
                .iter()
                .filter_map(|row| match row {
                    Row::Track { member_id, .. } => Some(*member_id),
                    _ => None,
                })
                .collect();
            if self.anchor.is_none() {
                self.anchor = Some(member);
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(member) {
                self.selected.remove(&member);
            }
            self.anchor = Some(member);
        } else {
            self.selected = HashSet::from([member]);
            self.anchor = Some(member);
        }
        self.publish_selection(cx);
        cx.notify();
    }

    /// Ctrl+A: take every track across every open playlist. Anchors at the
    /// first so a follow-up shift-click narrows from the top.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let members = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track { member_id, .. } => Some(*member_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        if members.is_empty() {
            return;
        }
        self.anchor = members.first().copied();
        self.selected = members.into_iter().collect();
        self.publish_selection(cx);
        cx.notify();
    }

    /// Resolve the selected members to track ids in view order and publish them
    /// on the shared selection for the panels that display it.
    fn publish_selection(&self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track {
                    member_id,
                    track_id,
                    ..
                } if self.selected.contains(member_id) => Some(*track_id),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
    }

    /// Drop the given members. The library edit rebuilds the tree, and the
    /// refresh prunes them out of the selection.
    fn remove_members(&mut self, members: Vec<i64>, cx: &mut Context<Self>) {
        if members.is_empty() {
            return;
        }
        self.state.library.update(cx, |library, cx| {
            library.remove_playlist_members(&members, cx);
        });
    }

    /// Delete or Backspace drops the selected members. Ctrl+A takes every
    /// visible track.
    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if modifiers.secondary() && key == "a" {
            self.select_all(cx);
            return;
        }
        if key == "delete" || key == "backspace" {
            let members = self.selected_members();
            self.remove_members(members, cx);
        }
    }

    /// A dragged set dropped onto a row: onto a header, or a track, it lands as
    /// one block before the target (or at the end of a header's playlist),
    /// pulling in members from other playlists on the way. Dropping onto one of
    /// the dragged rows does nothing.
    fn drop_on(&mut self, drag: &TrackDrag, target: usize, cx: &mut Context<Self>) {
        let (playlist_id, before) = match self.rows.get(target) {
            Some(Row::Head { id, .. }) => (*id, None),
            Some(Row::Track {
                playlist_id,
                member_id,
                ..
            }) => (*playlist_id, Some(*member_id)),
            None => return,
        };
        if before.is_some_and(|b| drag.members.contains(&b)) {
            return;
        }
        let members = drag.members.clone();
        self.state.library.update(cx, |library, cx| {
            library.place_playlist_members(playlist_id, &members, before, cx);
        });
    }

    /// The visible slice of the tree.
    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        range
            .filter_map(|ix| {
                Some(match self.rows.get(ix)? {
                    Row::Head {
                        name,
                        count,
                        expanded,
                        favourite,
                        ..
                    } => self.head_row(ix, name.clone(), *count, *expanded, *favourite, cx),
                    Row::Track {
                        playlist_id,
                        member_id,
                        track_id,
                        title,
                        artist,
                    } => {
                        let selected = self.selected.contains(member_id);
                        self.track_row(
                            ix,
                            *playlist_id,
                            *member_id,
                            *track_id,
                            title.clone(),
                            artist.clone(),
                            selected,
                            cx,
                        )
                    }
                })
            })
            .collect()
    }

    fn head_row(
        &self,
        ix: usize,
        name: String,
        count: u64,
        expanded: bool,
        favourite: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let chevron = if expanded {
            icons::CHEVRON_DOWN
        } else {
            icons::CHEVRON_RIGHT
        };
        div()
            .id(("playlist-head", ix))
            .w_full()
            .h(px(ROW_H))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover()))
            // A header is a drop target: tracks dropped on it move there.
            .drag_over::<TrackDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _, cx| {
                this.drop_on(drag, ix, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    if let Some(Row::Head { id, .. }) = this.rows.get(ix) {
                        this.toggle(*id, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.menu_row = Some(ix);
                    cx.notify();
                }),
            )
            .child(
                svg()
                    .path(chevron)
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::text_muted()),
            )
            // The favourites playlist wears a heart so it reads as the default
            // one, not just another list named Favourites.
            .when(favourite, |d| {
                d.child(
                    svg()
                        .path(icons::HEART_FILLED)
                        .size(px(13.))
                        .flex_none()
                        .text_color(palette::accent()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(name)),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(count.to_string())),
            )
            // Export this playlist to an M3U8 file. Its own mouse-down stops
            // the press from reaching the row, so a click here never toggles
            // the header open.
            .child(
                div()
                    .id(("playlist-export", ix))
                    .flex_none()
                    .p(px(3.))
                    .rounded(tokens::RADIUS)
                    .cursor_pointer()
                    .hover(|d| d.bg(palette::bg_control()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if let Some(Row::Head { id, name, .. }) = this.rows.get(ix) {
                                let (id, name) = (*id, name.clone());
                                this.export(id, name, cx);
                            }
                        }),
                    )
                    .child(
                        svg()
                            .path(icons::UPLOAD)
                            .size(px(14.))
                            .flex_none()
                            .text_color(palette::text_muted()),
                    ),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn track_row(
        &self,
        ix: usize,
        playlist_id: i64,
        member_id: i64,
        track_id: i64,
        title: String,
        artist: String,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let playing = self.playing == Some(track_id);
        // Dragging a row inside a multi-selection carries the whole set in view
        // order; outside it, just this row.
        let members = if self.selected.len() > 1 && self.selected.contains(&member_id) {
            self.selected_members()
        } else {
            vec![member_id]
        };
        let drag = TrackDrag {
            members,
            title: SharedString::from(title.clone()),
        };
        div()
            .id(("playlist-track", ix))
            .w_full()
            .h(px(ROW_H))
            // Indented under its header, past the chevron column.
            .pl(px(28.))
            .pr(tokens::SPACE_SM)
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
            .on_drag(drag, |drag, _pos, _window, cx| {
                cx.new(|_| TrackDragPreview {
                    title: drag.title.clone(),
                    extra: drag.members.len().saturating_sub(1),
                })
            })
            .drag_over::<TrackDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _, cx| {
                this.drop_on(drag, ix, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    // Take focus so Delete on the selection reaches the panel's
                    // key handler.
                    window.focus(&this.focus);
                    if event.click_count > 1 {
                        this.play(playlist_id, Some(track_id), cx);
                    } else if event.modifiers.shift || event.modifiers.secondary() {
                        // Shift and cmd/ctrl resolve on press.
                        this.select(ix, event.modifiers, cx);
                    } else if !this.selected.contains(&member_id) {
                        // A plain press on an unselected row picks it now, so a
                        // drag from here carries it. A press on an already-lit
                        // row keeps the set for a whole-group drag; the collapse
                        // to this one row waits for the click.
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                // A plain click that never became a drag collapses a
                // multi-selection down to the row clicked. Modified and double
                // clicks already resolved on press.
                let mods = event.modifiers();
                if event.click_count() == 1
                    && !mods.shift
                    && !mods.secondary()
                    && this.selected.len() > 1
                    && this.selected.contains(&member_id)
                {
                    this.select(ix, Modifiers::default(), cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.menu_row = Some(ix);
                    // A right click outside the set reselects just that row, so
                    // the menu acts on what is lit.
                    if !this.selected.contains(&member_id) {
                        this.select(ix, Modifiers::default(), cx);
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(playing, |d| d.text_color(palette::accent()))
                    .child(SharedString::from(title)),
            )
            .when(!artist.is_empty(), |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(palette::text_secondary())
                        .child(SharedString::from(artist)),
                )
            })
    }

    /// The panel menu's New Playlist entry, shared by the dropdown and the
    /// empty state.
    fn new_playlist_item(&self, menu: PopupMenu) -> PopupMenu {
        let state = self.state.clone();
        menu.item(
            PopupMenuItem::new("New Playlist...")
                .icon(Icon::default().path(icons::PLUS))
                .on_click(move |_, _, cx| {
                    crate::playlist_create::open(state.clone(), Vec::new(), cx);
                }),
        )
    }
}

impl PanelSettings for PlaylistsPanel {
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
}

impl EventEmitter<PanelEvent> for PlaylistsPanel {}

impl Focusable for PlaylistsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for PlaylistsPanel {
    fn panel_name(&self) -> &'static str {
        "playlists"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Playlists")
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = self.new_playlist_item(menu);
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), _window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
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
                    let dup = cx.new(|cx| PlaylistsPanel::new(state, config, cx));
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

    /// The tab bar's own Import button, beside the panel menu. Import is a
    /// panel-level action, unlike per-playlist export, so it lives here where
    /// it reads clearly instead of buried in the dropdown.
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![Button::new("import-playlist")
            .icon(Icon::default().path(icons::DOWNLOAD))
            .tooltip("Import Playlist")
            .on_click(cx.listener(|this, _, window, cx| {
                this.import(window, cx)
            }))])
    }
}

impl Render for PlaylistsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl PlaylistsPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)));
        let content = if self.rows.is_empty() {
            div().flex_1().min_h_0().flex().flex_col().child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_faint())
                    .child("No playlists yet, add tracks or use New Playlist"),
            )
        } else {
            let this = cx.entity().downgrade();
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    uniform_list("playlist-rows", self.rows.len(), move |range, _, cx| {
                        this.upgrade()
                            .map(|this| this.update(cx, |this, cx| this.list_rows(range, cx)))
                            .unwrap_or_default()
                    })
                    .track_scroll(self.scroll.clone())
                    .size_full(),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Scrollbar::vertical(&self.scroll)),
                )
        };
        let content =
            content.capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.menu_row = None;
                }
            }));
        let weak = cx.entity().downgrade();
        root.child(content.context_menu(move |menu, window, cx| {
            let Some(this) = weak.upgrade() else {
                return menu;
            };
            this.update(cx, |this, cx| this.row_menu(menu, window, cx))
        }))
    }

    /// The right-click menu for the row under the last press: track actions
    /// for a track, play/rename/delete for a header, the panel menu when the
    /// press missed the rows.
    fn row_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let Some(ix) = self.menu_row else {
            return self.dropdown_menu(menu, window, cx);
        };
        let weak = cx.entity().downgrade();
        match self.rows.get(ix) {
            Some(Row::Track {
                playlist_id,
                member_id,
                track_id,
                ..
            }) => {
                let (playlist_id, member_id, track_id) = (*playlist_id, *member_id, *track_id);
                let play_panel = weak.clone();
                let menu = panel::track_actions(
                    menu,
                    self.state.clone(),
                    vec![track_id],
                    "Play",
                    window,
                    cx,
                    move |_, cx| {
                        if let Some(this) = play_panel.upgrade() {
                            this.update(cx, |this, cx| this.play(playlist_id, Some(track_id), cx));
                        }
                    },
                );
                let remove_panel = weak.clone();
                // The right press already pulled the row into the selection, so
                // Remove drops the whole lit set: one row or many.
                let remove_label = if self.selected.contains(&member_id) && self.selected.len() > 1
                {
                    format!("Remove {} from Playlist", self.selected.len())
                } else {
                    "Remove from Playlist".to_string()
                };
                let menu = menu.item(
                    PopupMenuItem::new(remove_label)
                        .icon(Icon::default().path(icons::CLOSE))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = remove_panel.upgrade() {
                                this.update(cx, |this, cx| {
                                    let members = if this.selected.contains(&member_id) {
                                        this.selected_members()
                                    } else {
                                        vec![member_id]
                                    };
                                    this.remove_members(members, cx);
                                });
                            }
                        }),
                );
                self.dropdown_menu(menu.separator(), window, cx)
            }
            Some(Row::Head {
                id,
                name,
                favourite,
                ..
            }) => {
                let (id, name, favourite) = (*id, name.clone(), *favourite);
                let play_panel = weak.clone();
                let menu = menu.item(
                    PopupMenuItem::new("Play")
                        .icon(Icon::default().path(icons::PLAY))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = play_panel.upgrade() {
                                this.update(cx, |this, cx| this.play(id, None, cx));
                            }
                        }),
                );
                // The favourites playlist is the one default: no rename, no
                // delete, so the heart column and menu always have their home.
                let rename_state = self.state.clone();
                let menu = menu.when(!favourite, |menu| {
                    menu.item(
                        PopupMenuItem::new("Rename...")
                            .icon(Icon::default().path(icons::PENCIL))
                            .on_click(move |_, _, cx| {
                                crate::playlist_create::open_rename(
                                    rename_state.clone(),
                                    id,
                                    name.clone(),
                                    cx,
                                );
                            }),
                    )
                });
                let delete_panel = weak.clone();
                let menu = menu.when(!favourite, |menu| {
                    menu.item(
                        PopupMenuItem::new("Delete Playlist")
                            .icon(Icon::default().path(icons::TRASH))
                            .on_click(move |_, _, cx| {
                                if let Some(this) = delete_panel.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.state.library.update(cx, |library, cx| {
                                            library.delete_playlist(id, cx);
                                        });
                                    });
                                }
                            }),
                    )
                });
                self.dropdown_menu(menu.separator(), window, cx)
            }
            None => self.dropdown_menu(menu, window, cx),
        }
    }
}
