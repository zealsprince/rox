//! The history panel: the listen record as a track list, per ADR 11 and
//! the scope's history surface. Three views over the same events - the
//! newest listens first, tracks by play count, and the library tracks no
//! event has ever named - picked per panel, so a duplicate can watch
//! each. Rows read at panel-open and listen-append cadence off the
//! library's events table, never per frame; clicks select and double
//! clicks queue from the row, the library panel's moves. Its own panel,
//! never a mode of the library.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    div, prelude::*, px, uniform_list, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    MouseButton, MouseDownEvent, SharedString, Subscription, UniformListScrollHandle, WeakEntity,
    Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::listens::TrackPlays;
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::palette::PanelTheme;
use crate::design::{palette, tokens};
use crate::history::HistoryEvent;
use crate::panel::{self, AppState, PanelSettings};
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
            HistoryView::Recent => "recently played",
            HistoryView::Most => "most played",
            HistoryView::Never => "never played",
        }
    }
}

/// The history panel's per-view config: what a saved layout restores,
/// and what the settings window edits. Missing fields take the defaults,
/// so a layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// The rename shown as the tab and title text; None shows the
    /// built-in name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub view: HistoryView,
    /// The panel's palette override.
    #[serde(skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            title: None,
            view: HistoryView::default(),
            theme: PanelTheme::default(),
        }
    }
}

pub struct HistoryPanel {
    state: AppState,
    config: HistoryConfig,
    /// The current view's rows, re-queried when a listen lands or the
    /// catalog changes, cached between.
    rows: Vec<TrackPlays>,
    /// The clicked row, for the selection highlight.
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _history_changed: Subscription,
    _library_changed: Subscription,
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
        let mut this = HistoryPanel {
            state,
            config,
            rows: Vec::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _history_changed,
            _library_changed,
        };
        this.refresh(cx);
        this
    }

    /// Re-read the current view's rows off the events table.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let library = self.state.library.read(cx);
        self.rows = match self.config.view {
            HistoryView::Recent => library.recent_listens(ROWS_CAP),
            HistoryView::Most => library.most_played(ROWS_CAP),
            HistoryView::Never => library.never_played(ROWS_CAP),
        };
        self.selected = None;
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

    /// A double click queues the row and what follows it in the view's
    /// order, the library panel's move. A track deleted since its event
    /// resolves to no path and drops out of the queue quietly.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self.rows[ix..]
            .iter()
            .take(QUEUE_CAP)
            .map(|row| row.track_id)
            .collect();
        let Ok(paths) = self.state.library.read(cx).paths_for(&ids) else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play(paths, cx));
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
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
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
    fn config_menu(&self, mut menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        for view in [HistoryView::Recent, HistoryView::Most, HistoryView::Never] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(match view {
                    HistoryView::Recent => "Recently Played",
                    HistoryView::Most => "Most Played",
                    HistoryView::Never => "Never Played",
                })
                .checked(self.config.view == view)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.set_view(view, cx));
                }),
            );
        }
        menu
    }
}

impl PanelSettings for HistoryPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn custom_title(&self) -> Option<&str> {
        self.config.title.as_deref()
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.title = title;
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
                "view",
                Some("which cut of the listen record the panel shows"),
                panel::choices(
                    &[
                        ("recent", HistoryView::Recent),
                        ("most played", HistoryView::Most),
                        ("never played", HistoryView::Never),
                    ],
                    self.config.view,
                    |this: &mut Self, view, cx| this.set_view(view, cx),
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn theme(&self) -> PanelTheme {
        self.config.theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.config.theme = theme;
        cx.notify();
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
        panel::title_text(self.config.title.as_deref(), "history")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.title.clone().map(SharedString::from)
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The config block: the panel's quick entries and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, cx);
        let menu = menu.separator();
        let menu = panel_settings::rename_item(menu, &cx.entity());
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
        let theme = self.config.theme.clone();
        panel::themed(&theme, || self.body(cx))
    }
}

impl HistoryPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let root = div().size_full().flex().flex_col().bg(palette::bg_root());
        if self.rows.is_empty() {
            return root.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_faint())
                    .child(match self.config.view {
                        HistoryView::Never => "every track has been played",
                        _ => "no listens yet",
                    }),
            );
        }
        let this = cx.entity().downgrade();
        root.child(
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
    }
}

/// A listen's age as a short readout: seconds up through years, one
/// unit, no calendar math.
fn fmt_ago(secs: i64) -> String {
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
