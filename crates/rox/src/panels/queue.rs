//! The play queue panel (ADR 16): the explicit up-next queue, what Play Next
//! and Add to Queue put ahead of the playing track. The album or library you
//! started from plays on as the context and is not listed here, so the queue
//! stays what you hand-picked; a now-playing strip heads the numbered rows
//! so the panel says where the queue picks up from. Rows play now on double
//! click, drop from the right-click menu, and drag to reorder. Its own
//! panel, never a mode of the library.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, svg, uniform_list, App, Context, Div, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, SharedString, Stateful,
    Subscription, UniformListScrollHandle, WeakEntity, Window,
};
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

/// The queue panel's config: just the shared chrome, the panel has no knobs
/// of its own. Kept a struct so a later option has a home and old dumps still
/// load.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
}

/// One resolved queue row: the entry's stable id (what edits address), its
/// track id for the selection and editors, and the tags to draw. A queued
/// file that has left the library resolves to just its file name.
struct Row {
    entry_id: u64,
    track_id: Option<i64>,
    title: String,
    artist: String,
}

/// The value carried through a row drag: the entries being moved, in queue
/// order, and the grabbed row's title for the drag preview. Dragging a row
/// inside a multi-selection carries the whole set; outside it, just that row.
#[derive(Clone)]
struct QueueDrag {
    ids: Vec<u64>,
    title: SharedString,
}

/// The label that floats under the pointer while a queue row is dragged. A
/// multi-row drag shows the grabbed title with a count of the rest.
struct QueueDragPreview {
    title: SharedString,
    extra: usize,
}

impl Render for QueueDragPreview {
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

pub struct QueuePanel {
    state: AppState,
    config: QueueConfig,
    /// The resolved queue rows.
    rows: Vec<Row>,
    /// The playing track's title and artist, for the now-playing strip
    /// heading the list. Follows whatever plays, queued or context, so the
    /// panel always says where the queue picks up from.
    playing: Option<(String, String)>,
    /// The last queue revision the rows were built from; with the playing path,
    /// the cheap change detector so the per-pump observe only re-reads the
    /// queue when an edit lands or a track advances (which shrinks the queue).
    rev: Option<u64>,
    playing_path: Option<PathBuf>,
    /// The selected rows, by index. Shift extends, cmd (ctrl elsewhere)
    /// toggles, Ctrl+A takes the lot, the library's click rules.
    selected: HashSet<usize>,
    /// Where the next shift-click extends from: the last plain or toggle
    /// pick.
    anchor: Option<usize>,
    /// Entries a reorder just moved, kept by id so the next rebuild can
    /// re-select them at their new spots instead of leaving the highlight on
    /// whatever slid into the old indices.
    follow_moved: Vec<u64>,
    /// The row under the last right press, for the context menu.
    menu_row: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _library_changed: Subscription,
}

impl QueuePanel {
    pub fn new(state: AppState, config: QueueConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| this.sync(cx));
        // A retag or rescan changes the tags a row draws; force a rebuild.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.rev = None;
                    this.sync(cx);
                }
            },
        );
        let mut this = QueuePanel {
            state,
            config,
            rows: Vec::new(),
            playing: None,
            rev: None,
            playing_path: None,
            selected: HashSet::new(),
            anchor: None,
            follow_moved: Vec::new(),
            menu_row: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _library_changed,
        };
        this.sync(cx);
        this
    }

    /// Re-read the explicit queue. Bails on the cheap revision and playing-path
    /// compare, so a steady queue costs two reads per tick and nothing else;
    /// rebuilds the rows only when an edit bumps the revision or a track
    /// advance drops a played item off the front.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let rev = self.state.player.read(cx).queue_rev();
        let playing_path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if rev == self.rev && playing_path == self.playing_path {
            return;
        }
        self.rev = rev;
        self.playing_path = playing_path;
        let queued = self.state.player.read(cx).queued();
        let library = self.state.library.read(cx);
        self.playing = self
            .playing_path
            .as_ref()
            .map(|path| match library.meta_for(path) {
                Some(meta) => (meta.title, meta.artist),
                None => (file_label(path), String::new()),
            });
        self.rows = queued
            .iter()
            .map(|entry| {
                let (title, artist) = match library.meta_for(&entry.path) {
                    Some(meta) => (meta.title, meta.artist),
                    None => (file_label(&entry.path), String::new()),
                };
                Row {
                    entry_id: entry.id,
                    track_id: library.id_for(&entry.path),
                    title,
                    artist,
                }
            })
            .collect();
        let len = self.rows.len();
        if !self.follow_moved.is_empty() {
            // A reorder just landed: track the moved entries to their new
            // rows so the selection rides along with the drop.
            let moved = std::mem::take(&mut self.follow_moved);
            self.selected = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| moved.contains(&row.entry_id))
                .map(|(ix, _)| ix)
                .collect();
            self.anchor = self.selected.iter().min().copied();
        }
        self.selected.retain(|&ix| ix < len);
        if self.anchor.is_some_and(|ix| ix >= len) {
            self.anchor = None;
        }
        self.menu_row = None;
        cx.notify();
    }

    /// Put a click on a row: plain selects just it, shift extends from the
    /// anchor, cmd (ctrl elsewhere) toggles - the library's click rules.
    /// Publishes the selection either way.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        if ix >= self.rows.len() {
            return;
        }
        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(ix);
            let (lo, hi) = (anchor.min(ix), anchor.max(ix));
            self.selected = (lo..=hi).collect();
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

    /// Ctrl+A: take the whole queue. Anchors at the top so a follow-up
    /// shift-click narrows back down from there.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (0..self.rows.len()).collect();
        self.anchor = Some(0);
        self.publish_selection(cx);
        cx.notify();
    }

    /// Resolve the selected rows to track ids in queue order and publish
    /// them on the shared selection for the panels that display it.
    fn publish_selection(&self, cx: &mut Context<Self>) {
        let mut ixs: Vec<usize> = self.selected.iter().copied().collect();
        ixs.sort_unstable();
        let ids: Vec<i64> = ixs
            .iter()
            .filter_map(|&ix| self.rows.get(ix).and_then(|row| row.track_id))
            .collect();
        if ids.is_empty() {
            return;
        }
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
    }

    /// A double click plays that entry now. Through the player's
    /// move-then-jump, so the rows above it stay queued instead of falling
    /// behind the cursor as history and vanishing from the panel.
    fn jump(&self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        let id = row.entry_id;
        self.state.player.read(cx).play_queued(id);
    }

    /// Clear Queue: drop every row. Through `remove_ids`, so the panel
    /// empties right away even while paused.
    fn clear(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self.rows.iter().map(|row| row.entry_id).collect();
        self.remove_ids(&ids, cx);
    }

    /// Drop a set of queued entries by id. Drops them from our own rows right
    /// away too, so the change shows even while paused, when the player's pump
    /// is quiet and the sync that would rebuild from the engine does not run;
    /// the next sync reconciles against the engine either way. The lowest
    /// removed spot keeps the mark, so the next item slides up under it and a
    /// run of deletes stays put.
    fn remove_ids(&mut self, ids: &[u64], cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        let player = self.state.player.read(cx);
        for &id in ids {
            player.remove_from_queue(id);
        }
        let landing = self.rows.iter().position(|row| ids.contains(&row.entry_id));
        self.rows.retain(|row| !ids.contains(&row.entry_id));
        self.selected.clear();
        self.anchor = None;
        if let Some(ix) = landing.filter(|&ix| ix < self.rows.len()) {
            self.selected.insert(ix);
            self.anchor = Some(ix);
        }
        self.publish_selection(cx);
        cx.notify();
    }

    /// The entry ids of the current selection, in queue order.
    fn selected_ids(&self) -> Vec<u64> {
        let mut ixs: Vec<usize> = self.selected.iter().copied().collect();
        ixs.sort_unstable();
        ixs.iter()
            .filter_map(|&ix| self.rows.get(ix).map(|row| row.entry_id))
            .collect()
    }

    /// The context menu's remove: the whole selection when the clicked row is
    /// part of it, else just that row. Right-click reselects a row outside the
    /// set first, so by menu time this is the highlighted rows.
    fn remove(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ids = if self.selected.contains(&ix) {
            self.selected_ids()
        } else {
            self.rows
                .get(ix)
                .map(|row| row.entry_id)
                .into_iter()
                .collect()
        };
        self.remove_ids(&ids, cx);
    }

    /// Delete or Backspace drops the selected rows. Ctrl+A takes the whole
    /// queue.
    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if modifiers.secondary() && key == "a" {
            self.select_all(cx);
            return;
        }
        if key == "delete" || key == "backspace" {
            let ids = self.selected_ids();
            self.remove_ids(&ids, cx);
        }
    }

    /// Rows dropped onto position `target`: move them to just before that row,
    /// i.e. right after the nearest queued entry above it that isn't one of the
    /// dragged ones. At the top of the queue there is none, so anchor to the
    /// playing track, which lands the group at the front of the queue rather
    /// than the front of the whole timeline. A multi-row drag chains each entry
    /// after the last, so the group keeps its queue order and lands as one
    /// contiguous run.
    fn reorder(&mut self, dragged: &[u64], target: usize, cx: &mut Context<Self>) {
        if dragged.is_empty() {
            return;
        }
        let above = self.rows[..target.min(self.rows.len())]
            .iter()
            .rev()
            .map(|r| r.entry_id)
            .find(|id| !dragged.contains(id));
        let mut after = match above {
            Some(id) => Some(id),
            None => self.state.player.read(cx).playing_entry(),
        };
        let player = self.state.player.read(cx);
        for &id in dragged {
            player.move_in_queue(id, after);
            after = Some(id);
        }
        // Re-select the group once the reordered queue rebuilds our rows.
        self.follow_moved = dragged.to_vec();
    }

    /// The order column's width, sized to the widest number so a long queue
    /// keeps its titles aligned.
    fn num_width(&self) -> gpui::Pixels {
        px(10. + 7. * self.rows.len().to_string().len() as f32)
    }

    /// The visible slice of the list. Every row is an up-next queue item, so
    /// all reorder by drag and all can be removed.
    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let num_width = self.num_width();
        range
            .filter_map(|ix| {
                let row = self.rows.get(ix)?;
                // Dragging a row inside a multi-selection carries the whole
                // set in queue order; outside it, just this row.
                let ids = if self.selected.len() > 1 && self.selected.contains(&ix) {
                    self.selected_ids()
                } else {
                    vec![row.entry_id]
                };
                let drag = QueueDrag {
                    ids,
                    title: SharedString::from(row.title.clone()),
                };
                let selected = self.selected.contains(&ix);
                Some(
                    div()
                        .id(("queue-row", ix))
                        .w_full()
                        .h(px(ROW_H))
                        .px(tokens::SPACE_SM)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .cursor_pointer()
                        .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
                        .hover(|d| d.bg(palette::bg_control_hover()))
                        .on_drag(drag, |drag, _pos, _window, cx| {
                            cx.new(|_| QueueDragPreview {
                                title: drag.title.clone(),
                                extra: drag.ids.len().saturating_sub(1),
                            })
                        })
                        .drag_over::<QueueDrag>(|style, _, _, _| {
                            style.bg(palette::alpha(palette::accent(), 0x1a))
                        })
                        .on_drop(cx.listener(move |this, drag: &QueueDrag, _, cx| {
                            this.reorder(&drag.ids, ix, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                // Take focus so Delete on the selection reaches
                                // the panel's key handler.
                                window.focus(&this.focus);
                                if event.click_count > 1 {
                                    this.jump(ix, cx);
                                } else if event.modifiers.shift || event.modifiers.secondary() {
                                    // Shift and cmd/ctrl resolve on press.
                                    this.select(ix, event.modifiers, cx);
                                } else if !this.selected.contains(&ix) {
                                    // A plain press on an unselected row picks
                                    // it now, so a drag from here carries it.
                                    this.select(ix, event.modifiers, cx);
                                }
                                // A plain press on an already-selected row keeps
                                // the set so a drag can move it whole; the
                                // collapse to this row waits for the click.
                            }),
                        )
                        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                            // A plain click that never became a drag collapses a
                            // multi-selection down to the row clicked. Modified
                            // and double clicks already resolved on press.
                            let mods = event.modifiers();
                            if event.click_count() == 1
                                && !mods.shift
                                && !mods.secondary()
                                && this.selected.len() > 1
                                && this.selected.contains(&ix)
                            {
                                this.select(ix, Modifiers::default(), cx);
                            }
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.menu_row = Some(ix);
                                // A right click outside the set reselects just
                                // that row, so the menu acts on what is lit.
                                if !this.selected.contains(&ix) {
                                    this.select(ix, Modifiers::default(), cx);
                                }
                            }),
                        )
                        // The order column: where the row sits in the queue,
                        // one-based like a tracklist.
                        .child(
                            div()
                                .w(num_width)
                                .flex_none()
                                .text_color(palette::text_muted())
                                .child(SharedString::from((ix + 1).to_string())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(row.title.clone())),
                        )
                        .when(!row.artist.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(palette::text_secondary())
                                    .child(SharedString::from(row.artist.clone())),
                            )
                        }),
                )
            })
            .collect()
    }
}

impl PanelSettings for QueuePanel {
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

impl EventEmitter<PanelEvent> for QueuePanel {}

impl Focusable for QueuePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for QueuePanel {
    fn panel_name(&self) -> &'static str {
        "queue"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Queue")
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
        let clear = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Clear Queue")
                .icon(Icon::default().path(icons::TRASH))
                .disabled(self.rows.is_empty())
                .on_click(move |_, _, cx| {
                    if let Some(this) = clear.upgrade() {
                        this.update(cx, |this, cx| this.clear(cx));
                    }
                }),
        );
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
                    let dup = cx.new(|cx| QueuePanel::new(state, config, cx));
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

impl Render for QueuePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl QueuePanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)));
        // The now-playing strip heading the list: display only, the marker
        // the numbered rows queue up behind. Wears the library's playing
        // look, the highlight wash with the accent title.
        let root = root.when_some(self.playing.clone(), |root, (title, artist)| {
            root.child(
                div()
                    .flex_none()
                    .w_full()
                    .h(px(ROW_H))
                    .px(tokens::SPACE_SM)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .bg(palette::alpha(palette::highlight(), 0x12))
                    .child(
                        div().w(self.num_width()).flex_none().child(
                            svg()
                                .path(icons::PLAY)
                                .size(px(12.))
                                .text_color(palette::accent()),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(palette::accent())
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
                    }),
            )
        });
        let content = if self.rows.is_empty() {
            div().flex_1().min_h_0().flex().flex_col().child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_faint())
                    .child("Queue is empty"),
            )
        } else {
            let this = cx.entity().downgrade();
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    uniform_list("queue-rows", self.rows.len(), move |range, _, cx| {
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
            // The clicked row plus the selection it acts on. The right press
            // already pulled the row into the set, so this is what is lit.
            let target = {
                let panel = this.read(cx);
                let ix = panel.menu_row.filter(|&ix| panel.rows.get(ix).is_some());
                ix.map(|ix| {
                    let mut ixs: Vec<usize> = panel.selected.iter().copied().collect();
                    ixs.sort_unstable();
                    let track_ids: Vec<i64> = ixs
                        .iter()
                        .filter_map(|&i| panel.rows.get(i).and_then(|row| row.track_id))
                        .collect();
                    (ix, track_ids, panel.selected.len().max(1))
                })
            };
            let Some((ix, track_ids, count)) = target else {
                return this.update(cx, |this, cx| this.dropdown_menu(menu, window, cx));
            };
            let jump_panel = weak.clone();
            let remove_panel = weak.clone();
            let remove_label = if count > 1 {
                format!("Remove {count} from Queue")
            } else {
                "Remove from Queue".to_string()
            };
            let mut menu = menu
                .item(
                    PopupMenuItem::new("Play")
                        .icon(Icon::default().path(icons::PLAY))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = jump_panel.upgrade() {
                                this.update(cx, |this, cx| this.jump(ix, cx));
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new(remove_label)
                        .icon(Icon::default().path(icons::CLOSE))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = remove_panel.upgrade() {
                                this.update(cx, |this, cx| this.remove(ix, cx));
                            }
                        }),
                );
            // The shared edit/reveal actions when the rows are known tracks.
            if !track_ids.is_empty() {
                let state = this.read(cx).state.clone();
                menu = panel::track_actions(
                    menu.separator(),
                    state,
                    track_ids,
                    "Play Now",
                    window,
                    cx,
                    {
                        let panel = weak.clone();
                        move |_, cx| {
                            if let Some(this) = panel.upgrade() {
                                this.update(cx, |this, cx| this.jump(ix, cx));
                            }
                        }
                    },
                );
            }
            this.update(cx, |this, cx| {
                this.dropdown_menu(menu.separator(), window, cx)
            })
        }))
    }
}

/// A queued file's last path component as a fallback label, when the track
/// is not in the library to give a title.
fn file_label(path: &PathBuf) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
