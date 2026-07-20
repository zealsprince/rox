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
    MouseButton, MouseDownEvent, SharedString, Subscription, UniformListScrollHandle, WeakEntity,
    Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::listens::TrackPlays;
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::history::HistoryEvent;
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::{LibraryEvent, QUEUE_CAP};

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

/// The history panel's per-view config: what a saved layout restores,
/// and what the settings window edits. Missing fields take the defaults,
/// so a layout dumped before a knob existed still loads.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub view: HistoryView,
}

pub struct HistoryPanel {
    state: AppState,
    config: HistoryConfig,
    /// The current view's rows, re-queried when a listen lands or the
    /// catalog changes, cached between.
    rows: Vec<TrackPlays>,
    /// The clicked row, for the selection highlight.
    selected: Option<usize>,
    /// The playing track's path, the change detector for the highlight;
    /// the player notifies every pump, so the compare keeps sync cheap.
    playing_path: Option<PathBuf>,
    /// The playing track as its library id, the rows' key.
    playing: Option<i64>,
    /// The row under the last right press, for the context menu: the
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
}

impl HistoryPanel {
    pub fn new(state: AppState, config: HistoryConfig, cx: &mut Context<Self>) -> Self {
        let _history_changed = cx.subscribe(
            &state.history,
            |this: &mut Self, _, _: &HistoryEvent, cx| this.refresh(cx),
        );
        // A rescan can retag tracks and grow the never-played set.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
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
            rows: Vec::new(),
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

    /// Re-read the current view's rows off the events table.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let library = self.state.library.read(cx);
        self.rows = match self.config.view {
            HistoryView::Recent => library.recent_listens(0, ROWS_CAP),
            HistoryView::Most => library.most_played(ROWS_CAP),
            HistoryView::Never => library.never_played(ROWS_CAP),
        };
        self.selected = None;
        self.menu_row = None;
        cx.notify();
    }

    fn set_view(&mut self, view: HistoryView, cx: &mut Context<Self>) {
        if self.config.view == view {
            return;
        }
        self.config.view = view;
        self.refresh(cx);
    }

    /// A single click selects: the row's highlight here, its track id on
    /// the shared selection for the panels that display it.
    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        self.selected = Some(ix);
        let ids = vec![row.track_id];
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
        cx.notify();
    }

    /// A double click queues the row with the surrounding view as its
    /// timeline: earlier rows seed behind the cursor for Prev, later rows
    /// carry Next, the clicked row plays. Bounded to a window around the
    /// click with a share kept for history. A track deleted since its event
    /// resolves to no path and drops out of the queue quietly.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let lo = ix
            .saturating_sub(QUEUE_CAP / 2)
            .min(self.rows.len().saturating_sub(QUEUE_CAP));
        let hi = (lo + QUEUE_CAP).min(self.rows.len());
        let ids: Vec<i64> = self.rows[lo..hi].iter().map(|row| row.track_id).collect();
        let Ok(paths) = self.state.library.read(cx).paths_for(&ids) else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        let start = ix - lo;
        self.state
            .player
            .update(cx, |player, cx| player.play_at(paths, start, cx));
    }

    /// The visible slice of the list.
    fn list_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<Div> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let view = self.config.view;
        range
            .filter_map(|ix| {
                let row = self.rows.get(ix)?;
                let sub = match (row.artist.is_empty(), row.album.is_empty()) {
                    (false, false) => format!("{} - {}", row.artist, row.album),
                    (false, true) => row.artist.clone(),
                    (true, false) => row.album.clone(),
                    (true, true) => String::new(),
                };
                // The trailing readout: when for recent, how often for
                // most played, nothing for never played.
                let trailing = match view {
                    HistoryView::Recent => Some(fmt_ago(now - row.last_played)),
                    HistoryView::Most => Some(format!("{} plays", row.plays)),
                    HistoryView::Never => None,
                };
                let playing = self.playing == Some(row.track_id);
                Some(
                    div()
                        .w_full()
                        .h(px(ROW_H))
                        .px(tokens::SPACE_SM)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .cursor_pointer()
                        .when(self.selected == Some(ix), |d| {
                            d.bg(palette::alpha(palette::accent(), 0x26))
                        })
                        // The playing track's rows wear the highlight
                        // role, a faint cut apart from the accent-washed
                        // selection, the library's look.
                        .when(playing && self.selected != Some(ix), |d| {
                            d.bg(palette::alpha(palette::highlight(), 0x12))
                        })
                        .hover(|d| d.bg(palette::bg_control_hover()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                if event.click_count > 1 {
                                    this.play_from(ix, cx);
                                } else {
                                    this.select(ix, cx);
                                }
                            }),
                        )
                        // The right press records its row for the context
                        // menu and, outside the selection, reselects just
                        // this row first, so the menu always acts on what
                        // is highlighted - the library's rule.
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.menu_row = Some(ix);
                                if this.selected != Some(ix) {
                                    this.select(ix, cx);
                                }
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when(playing, |d| d.text_color(palette::accent()))
                                .child(SharedString::from(row.title.clone())),
                        )
                        .when(!sub.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(palette::text_secondary())
                                    .child(SharedString::from(sub)),
                            )
                        })
                        .when_some(trailing, |d, trailing| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_color(palette::text_muted())
                                    .child(SharedString::from(trailing)),
                            )
                        }),
                )
            })
            .collect()
    }

    /// The panel's own dropdown entries: the view pick, the same knob
    /// the settings window edits.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            for view in [HistoryView::Recent, HistoryView::Most, HistoryView::Never] {
                submenu = submenu.item(panel::check_row(
                    match view {
                        HistoryView::Recent => "Recently Played",
                        HistoryView::Most => "Most Played",
                        HistoryView::Never => "Never Played",
                    },
                    None,
                    move |this: &Self| this.config.view == view,
                    move |this, cx| this.set_view(view, cx),
                    &panel,
                ));
            }
            submenu
        });
        menu.item(PopupMenuItem::submenu("View", submenu))
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
        let content = if self.rows.is_empty() {
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
                    .and_then(|ix| panel.rows.get(ix).map(|row| (ix, row.track_id)))
            };
            let Some((ix, id)) = target else {
                return this.update(cx, |this, cx| this.dropdown_menu(menu, window, cx));
            };
            let state = this.read(cx).state.clone();
            let panel = weak.clone();
            // Play queues the row and what follows in the view's order,
            // the double click's move.
            let menu =
                panel::track_actions(menu, state, vec![id], "Play", window, cx, move |_, cx| {
                    if let Some(this) = panel.upgrade() {
                        this.update(cx, |this, cx| this.play_from(ix, cx));
                    }
                });
            this.update(cx, |this, cx| {
                this.dropdown_menu(menu.separator(), window, cx)
            })
        }))
    }
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
