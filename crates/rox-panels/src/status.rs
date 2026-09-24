//! The status strip: what the current scope holds, the classic status bar
//! line. A standing selection scopes it; otherwise it covers the catalog.
//! The readouts recompute only when the selection or the catalog moves.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use gpui::{
    AnyElement, AnyView, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    SharedString, Subscription, WeakEntity, Window, div, prelude::*, px,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::{Library, LibraryEvent};
use crate::design::{palette, tokens};
use crate::group_head;
use crate::panel::{self, Align, AppState, PanelChrome, PanelSettings, align_row, justify};
use crate::panel_settings;
use crate::selection::{Selection, SelectionEvent};
use crate::transport::transport_panel;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusItem {
    /// Names the selection where one name covers it: a title, or
    /// "artist - album" for a whole album.
    Count,
    Time,
    /// Keyed on album artist and album, the way the library groups.
    Albums,
    Artists,
    Plays,
    Spacer,
}

/// Stock order: where a menu toggle slots a re-shown readout back in.
const ITEMS: &[panel::ArrangeSpec<StatusItem>] = &[
    panel::ArrangeSpec {
        key: "status-item-count",
        icon: Some(icons::LIST_MUSIC),
        value: StatusItem::Count,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "status-item-time",
        icon: Some(icons::CLOCK),
        value: StatusItem::Time,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "status-item-albums",
        icon: Some(icons::DISC),
        value: StatusItem::Albums,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "status-item-artists",
        icon: Some(icons::MIC),
        value: StatusItem::Artists,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "status-item-plays",
        icon: Some(icons::CHART_PIE),
        value: StatusItem::Plays,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: StatusItem::Spacer,
        repeats: true,
    },
];

fn stock_items() -> Vec<StatusItem> {
    vec![StatusItem::Count, StatusItem::Time]
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "StatusConfigDump")]
pub struct StatusConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub align: Align,
    pub items: Vec<StatusItem>,
}

impl Default for StatusConfig {
    fn default() -> Self {
        StatusConfig {
            chrome: PanelChrome::default(),
            align: Align::default(),
            items: stock_items(),
        }
    }
}

#[derive(Deserialize)]
struct StatusConfigDump {
    #[serde(flatten)]
    chrome: PanelChrome,
    #[serde(default)]
    align: Align,
    #[serde(default = "stock_items")]
    items: Vec<StatusItem>,
}

impl From<StatusConfigDump> for StatusConfig {
    fn from(dump: StatusConfigDump) -> Self {
        StatusConfig {
            chrome: dump.chrome,
            align: dump.align,
            items: panel::dedup(ITEMS, dump.items),
        }
    }
}

struct Totals {
    tracks: usize,
    total_ms: u64,
    albums: usize,
    artists: usize,
    /// Only in the count tooltip, split like the genre grid.
    genres: usize,
    plays: u64,
    /// Stations: no duration or album, so they only count toward the selection.
    live: usize,
    selection: bool,
    selection_label: Option<String>,
}

pub struct StatusPanel {
    state: AppState,
    config: StatusConfig,
    totals: Option<Totals>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The row before a menu toggle hid a readout, so re-showing it restores
    /// its place. Panel state, not config.
    items_stash: Option<Vec<StatusItem>>,
    _selection_changed: Subscription,
    _library_changed: Subscription,
}

impl StatusPanel {
    pub fn new(state: AppState, config: StatusConfig, cx: &mut Context<Self>) -> Self {
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.totals = None;
                cx.notify();
            },
        );
        // A play-count import moves the plays sum.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated | LibraryEvent::PlaysReloaded) {
                    return;
                }
                this.totals = None;
                cx.notify();
            },
        );
        StatusPanel {
            state,
            config,
            totals: None,
            focus: cx.focus_handle(),
            tab_panel: None,
            items_stash: None,
            _selection_changed,
            _library_changed,
        }
    }

    fn config_menu(
        &self,
        menu: PopupMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let mut menu = menu;
        for (name, value) in [
            (rox_i18n::t!("status-item-count"), StatusItem::Count),
            (rox_i18n::t!("status-item-time"), StatusItem::Time),
            (rox_i18n::t!("status-item-albums"), StatusItem::Albums),
            (rox_i18n::t!("status-item-artists"), StatusItem::Artists),
            (rox_i18n::t!("status-item-plays"), StatusItem::Plays),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(name)
                    .checked(self.config.items.contains(&value))
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            this.config.items = panel::toggled_stashed(
                                ITEMS,
                                &this.config.items,
                                &mut this.items_stash,
                                &[value],
                            );
                            cx.notify();
                        });
                    }),
            );
        }
        menu
    }

    /// One pass over the projection, filtered to the selection while one
    /// stands. Albums key on (album artist, album) and artists on album artist,
    /// matching the grids.
    fn compute_totals(&mut self, cx: &App) {
        if self.totals.is_some() {
            return;
        }
        let selected: HashSet<i64> = self
            .state
            .selection
            .read(cx)
            .tracks()
            .iter()
            .copied()
            .collect();
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return;
        };
        let mut tracks = 0usize;
        let mut total_ms = 0u64;
        let mut albums: HashSet<(u32, u32)> = HashSet::new();
        let mut artists: HashSet<u32> = HashSet::new();
        let mut genre_syms: HashSet<u32> = HashSet::new();
        let mut plays = 0u64;
        let mut live = 0usize;
        let mut first_ix: Option<u32> = None;
        for (ix, id) in projection.db_id.iter().enumerate() {
            if projection.is_dead(ix as u32) {
                continue;
            }
            if !selected.is_empty() && !selected.contains(id) {
                continue;
            }
            // A station's plays count, but the row isn't a library track.
            if !projection.is_browsable(ix as u32) {
                live += 1;
                plays += u64::from(projection.plays[ix].load(Ordering::Relaxed));
                continue;
            }
            tracks += 1;
            total_ms += u64::from(projection.duration_ms[ix]);
            albums.insert((projection.album_artist[ix], projection.album[ix]));
            artists.insert(projection.album_artist[ix]);
            genre_syms.insert(projection.genre[ix]);
            plays += u64::from(projection.plays[ix].load(Ordering::Relaxed));
            if first_ix.is_none() {
                first_ix = Some(ix as u32);
            }
        }
        let genres = genre_count(genre_syms, &projection.genres.strings);
        let selection = !selected.is_empty();
        // The album takes the label only when the selection holds all of it.
        let selection_label = first_ix.filter(|_| selection && live == 0).and_then(|ix| {
            let row = projection.resolve(ix);
            if tracks == 1 {
                return (!row.title.is_empty()).then(|| row.title.to_string());
            }
            if albums.len() != 1 {
                return None;
            }
            let i = ix as usize;
            let pair = (projection.album_artist[i], projection.album[i]);
            let album_total = projection
                .album_artist
                .iter()
                .zip(&projection.album)
                .filter(|(aa, a)| (**aa, **a) == pair)
                .count();
            if album_total != tracks || row.album.is_empty() {
                return None;
            }
            // An empty album artist falls back to the track's artist, the header rule.
            let artist = if row.album_artist.is_empty() {
                row.artist
            } else {
                row.album_artist
            };
            Some(if artist.is_empty() {
                row.album.to_string()
            } else {
                format!("{artist} - {}", row.album)
            })
        });
        self.totals = Some(Totals {
            tracks,
            total_ms,
            albums: albums.len(),
            artists: artists.len(),
            genres,
            plays,
            live,
            selection,
            selection_label,
        });
    }

    fn tooltip_rows(&self) -> Vec<(SharedString, SharedString)> {
        self.totals.as_ref().map_or_else(Vec::new, |totals| {
            totals_rows(
                totals.tracks,
                totals.albums,
                totals.artists,
                totals.genres,
                totals.total_ms,
                totals.plays,
            )
        })
    }
}

fn totals_rows(
    tracks: usize,
    albums: usize,
    artists: usize,
    genres: usize,
    total_ms: u64,
    plays: u64,
) -> Vec<(SharedString, SharedString)> {
    [
        (
            rox_i18n::t!("head-piece-tracks"),
            rox_i18n::format::format_int(tracks as i64),
        ),
        (
            rox_i18n::t!("status-item-albums"),
            rox_i18n::format::format_int(albums as i64),
        ),
        (
            rox_i18n::t!("status-item-artists"),
            rox_i18n::format::format_int(artists as i64),
        ),
        (
            rox_i18n::t!("content-total-genres"),
            rox_i18n::format::format_int(genres as i64),
        ),
        (
            rox_i18n::t!("content-total-time"),
            format!(
                "{} ({})",
                group_head::fmt_total(total_ms),
                rox_core::fmt::fmt_span(total_ms / 1000)
            ),
        ),
        (
            rox_i18n::t!("status-item-plays"),
            rox_i18n::format::format_int(plays as i64),
        ),
    ]
    .into_iter()
    .map(|(label, value)| (label, SharedString::from(value)))
    .collect()
}

/// Split like the genre grid, so the counts agree across the app.
fn genre_count(syms: HashSet<u32>, strings: &[String]) -> usize {
    let mut genres: HashSet<&str> = HashSet::new();
    for sym in syms {
        genres.extend(rox_library::genre::split(&strings[sym as usize]));
    }
    genres.remove("");
    genres.len()
}

/// One uncached pass over a scope, the whole catalog when `selected` is
/// empty. For the menubar, which has nowhere to cache.
fn scope_totals(
    library: &Entity<Library>,
    selected: &HashSet<i64>,
    cx: &App,
) -> (usize, usize, u64, Vec<(SharedString, SharedString)>) {
    let Some(projection) = library.read(cx).projection() else {
        return (0, 0, 0, Vec::new());
    };
    let mut tracks = 0usize;
    let mut live = 0usize;
    let mut total_ms = 0u64;
    let mut plays = 0u64;
    let mut albums: HashSet<(u32, u32)> = HashSet::new();
    let mut artists: HashSet<u32> = HashSet::new();
    let mut genre_syms: HashSet<u32> = HashSet::new();
    for (ix, id) in projection.db_id.iter().enumerate() {
        if projection.is_dead(ix as u32) {
            continue;
        }
        if !selected.is_empty() && !selected.contains(id) {
            continue;
        }
        if !projection.is_browsable(ix as u32) {
            live += 1;
            plays += u64::from(projection.plays[ix].load(Ordering::Relaxed));
            continue;
        }
        tracks += 1;
        total_ms += u64::from(projection.duration_ms[ix]);
        plays += u64::from(projection.plays[ix].load(Ordering::Relaxed));
        albums.insert((projection.album_artist[ix], projection.album[ix]));
        artists.insert(projection.album_artist[ix]);
        genre_syms.insert(projection.genre[ix]);
    }
    let rows = totals_rows(
        tracks,
        albums.len(),
        artists.len(),
        genre_count(genre_syms, &projection.genres.strings),
        total_ms,
        plays,
    );
    (tracks, live, total_ms, rows)
}

fn selected_ids(selection: &Entity<Selection>, cx: &App) -> HashSet<i64> {
    selection.read(cx).tracks().iter().copied().collect()
}

/// Computed on open: the menubar has nowhere to cache, and one scan per
/// hover is fine.
pub fn library_tooltip(library: &Entity<Library>, cx: &mut App) -> AnyView {
    let (_, _, _, rows) = scope_totals(library, &HashSet::new(), cx);
    cx.new(|_| TotalsTooltip {
        scope: rox_i18n::t!("panel-title-library"),
        rows,
    })
    .into()
}

pub fn selection_tooltip(
    library: &Entity<Library>,
    selection: &Entity<Selection>,
    cx: &mut App,
) -> AnyView {
    let (_, _, _, rows) = scope_totals(library, &selected_ids(selection, cx), cx);
    cx.new(|_| TotalsTooltip {
        scope: rox_i18n::t!("status-scope-selection"),
        rows,
    })
    .into()
}

/// Stations count toward the number but carry no time, so a station-only
/// pick gets None for the duration.
pub fn selection_summary(
    library: &Entity<Library>,
    selection: &Entity<Selection>,
    cx: &App,
) -> Option<(usize, Option<u64>)> {
    let selected = selected_ids(selection, cx);
    if selected.is_empty() {
        return None;
    }
    let (tracks, live, total_ms, _) = scope_totals(library, &selected, cx);
    let picked = tracks + live;

    (picked > 0).then_some((picked, (tracks > 0).then_some(total_ms)))
}

/// Opaque fill, since it floats over panel content with no backdrop.
struct TotalsTooltip {
    scope: SharedString,
    rows: Vec<(SharedString, SharedString)>,
}

impl Render for TotalsTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .min_w(px(150.))
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border())
            .bg(palette::bg_menu_opaque())
            .shadow_md()
            .text_color(palette::text())
            .text_xs()
            .child(
                div()
                    .text_color(palette::text_muted())
                    .child(self.scope.clone()),
            )
            .children(self.rows.iter().map(|(label, value)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(tokens::SPACE_MD)
                    .text_color(palette::text_secondary())
                    .child(div().min_w_0().truncate().child(label.clone()))
                    .child(div().flex_none().child(value.clone()))
            }))
    }
}

impl PanelSettings for StatusPanel {
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
        &[("Layout", icons::ALIGN_LEFT)]
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
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .child(panel::setting_block(
                rox_i18n::t!("status-readouts"),
                Some(rox_i18n::t!("status-readouts.description")),
                None,
                panel::arrange_editor(
                    "status-items",
                    ITEMS,
                    &self.config.items,
                    |this: &mut Self, items, cx| {
                        this.config.items = items;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }
}

impl Render for StatusPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl StatusPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        self.compute_totals(cx);
        let root = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD);
        // Quiet on an empty catalog; a pick of stations alone still keeps the
        // strip up.
        let Some(totals) = self
            .totals
            .as_ref()
            .filter(|t| t.tracks > 0 || (t.selection && t.live > 0))
        else {
            return root;
        };
        // No clock for a station-only pick, which would read "2 selected / 0:00".
        let live_only = totals.selection && totals.tracks == 0;
        let stat = |text: SharedString| {
            div()
                .min_w_0()
                .truncate()
                .text_color(palette::text_muted())
                .child(text)
                .into_any_element()
        };
        let weak = cx.entity().downgrade();
        let pieces: Vec<AnyElement> = self
            .config
            .items
            .iter()
            .filter(|item| !(live_only && matches!(item, StatusItem::Time)))
            .map(|item| match item {
                StatusItem::Count => {
                    // Titles run long, so this one truncates instead of pinning.
                    let label = match (totals.selection, totals.tracks) {
                        (true, n) => totals.selection_label.clone().unwrap_or_else(|| {
                            let picked = (n + totals.live) as u64;
                            rox_i18n::t!("status-count-selected", count = picked).to_string()
                        }),
                        (false, n) => {
                            rox_i18n::t!("status-count-tracks", count = n as u64).to_string()
                        }
                    };
                    let scope = if totals.selection {
                        rox_i18n::t!("status-scope-selection")
                    } else {
                        rox_i18n::t!("panel-title-library")
                    };
                    let weak = weak.clone();
                    div()
                        .id("status-count")
                        .min_w_0()
                        .truncate()
                        .child(label)
                        .tooltip(move |_window, cx| {
                            let rows = weak
                                .upgrade()
                                .map(|this| this.read(cx).tooltip_rows())
                                .unwrap_or_default();
                            let scope = scope.clone();
                            cx.new(|_| TotalsTooltip { scope, rows }).into()
                        })
                        .into_any_element()
                }
                StatusItem::Time => stat(group_head::fmt_total(totals.total_ms).into()),
                StatusItem::Albums => stat(rox_i18n::t!(
                    "status-count-albums",
                    count = totals.albums as u64
                )),
                StatusItem::Artists => stat(rox_i18n::t!(
                    "status-count-artists",
                    count = totals.artists as u64
                )),
                StatusItem::Plays => stat(rox_i18n::t!("status-count-plays", count = totals.plays)),
                StatusItem::Spacer => div().flex_1().into_any_element(),
            })
            .collect();
        root.children(pieces)
    }
}

// The height floor is one text line, so the strip squeezes to a true
// status bar.
transport_panel!(
    StatusPanel,
    "status",
    rox_i18n::t!("status-title"),
    min_w = |_: &StatusPanel| px(96.),
    min_h = |_: &StatusPanel| palette::scaled_px(16.)
);

#[cfg(test)]
mod tests {
    use super::{StatusConfig, StatusItem};

    #[test]
    fn missing_items_default_to_count_and_time() {
        let config: StatusConfig = serde_json::from_str("{}").unwrap();
        assert!(config.items == vec![StatusItem::Count, StatusItem::Time]);
    }

    #[test]
    fn item_lists_read_ordered_and_deduped() {
        let config: StatusConfig =
            serde_json::from_str(r#"{"items": ["plays", "spacer", "count", "plays"]}"#).unwrap();
        assert!(config.items == vec![StatusItem::Plays, StatusItem::Spacer, StatusItem::Count]);

        let saved = serde_json::to_value(&config).unwrap();
        let back: StatusConfig = serde_json::from_value(saved).unwrap();
        assert!(back.items == config.items);
    }
}
