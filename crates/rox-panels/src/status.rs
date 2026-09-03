//! The status strip: one quiet line with what the current scope holds,
//! the classic status bar readout. A standing selection scopes it to the
//! picked tracks; otherwise the numbers cover the whole catalog. The readouts are
//! an ordered items list like the transport strips', recomputed only when
//! the selection or the catalog moves, so the strip costs nothing per
//! frame.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use gpui::{
    div, prelude::*, px, AnyElement, AnyView, App, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, Pixels, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::{Library, LibraryEvent};
use crate::design::{palette, tokens};
use crate::group_head;
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::selection::{Selection, SelectionEvent};
use crate::transport::transport_panel;

/// One readout of the status strip, the arrange editor's unit. The
/// config's list holds the shown ones in display order.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusItem {
    /// The track count. A selection scopes it and names itself where one
    /// name covers the set: the title for a single track, "artist -
    /// album" for a whole album, the plain "N selected" past that.
    Count,
    /// The scope's total running time.
    Time,
    /// How many distinct albums the scope spans, keyed the way the
    /// library groups them: album artist and album together.
    Albums,
    /// How many distinct album artists the scope spans.
    Artists,
    /// The scope's summed play count.
    Plays,
    /// A flexible gap that pushes the readouts around it apart; the
    /// strip holds as many as the layout needs.
    Spacer,
}

/// The strip's full catalog in stock order: what the arrange editor
/// offers, and where a menu toggle slots a re-shown readout back in.
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

/// The count and total the strip shipped with; albums, artists, and plays
/// are opt-in.
fn stock_items() -> Vec<StatusItem> {
    vec![StatusItem::Count, StatusItem::Time]
}

/// The status strip's per-view config: what a saved layout restores, and
/// what the settings window edits. Deserialization routes through
/// [`StatusConfigDump`] only to dedup a hand-edited list; there are no
/// legacy piece fields to fold.
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "StatusConfigDump")]
pub struct StatusConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub align: Align,
    /// The shown readouts in display order; one not listed is hidden.
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

/// The dump shape [`StatusConfig`] deserializes through.
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

/// One computed readout set: the scope's track count, summed time,
/// distinct album, artist, and genre counts, play total, and whether a
/// selection scoped it.
struct Totals {
    tracks: usize,
    total_ms: u64,
    albums: usize,
    artists: usize,
    /// Distinct genres, split the way the genre grid splits compound
    /// tags. Not a readout of its own; the count tooltip includes it so
    /// the hover matches the metadata panel's library sheet.
    genres: usize,
    plays: u64,
    selection: bool,
    /// What a standing selection resolves to by name: the track's title
    /// when one row is picked, "artist - album" when the selection holds
    /// one album whole. None past that (or on blank tags, or a partial
    /// album), where the plain "N selected" reads better.
    selection_label: Option<String>,
}

pub struct StatusPanel {
    state: AppState,
    config: StatusConfig,
    /// The computed readout, rebuilt when the selection or the catalog
    /// moves; renders in between just redraw the cached one.
    totals: Option<Totals>,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The row as it stood when a menu toggle last hid a readout, so
    /// showing it again puts it back where it was rather than at its
    /// catalog rank. The undo for one toggle, not a layout anybody saves,
    /// so it's stored on the panel and not the config.
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
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
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

    /// The panel's own dropdown entries: quick show/hide per readout. A
    /// re-shown one goes back where it was; the order changes in the
    /// settings window's arrange editor.
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

    /// The readouts, computed on a miss: one pass over the projection,
    /// filtered to the selection while one stands. A selected id the
    /// catalog no longer has just drops out of the sums. Albums key on
    /// the (album artist, album) pair the library groups by, so two
    /// artists' "Greatest Hits" count apart; artists are the distinct
    /// album artists, matching the artist grid.
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
        let mut first_ix: Option<u32> = None;
        for (ix, id) in projection.db_id.iter().enumerate() {
            if projection.is_dead(ix as u32) {
                continue;
            }
            if !selected.is_empty() && !selected.contains(id) {
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
        // Name the selection where one name covers it: any picked row
        // stands in for the whole set once the counts say it's one track
        // or one album. The album only takes the label when the selection
        // holds all of it: a partial pick reads "N selected" instead of
        // showing the full album's name.
        let selection_label = first_ix.filter(|_| selection).and_then(|ix| {
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
            // The artist is included too, the header rows' fallback rule:
            // an empty album artist falls back to the first track's artist.
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
            selection,
            selection_label,
        });
    }

    /// The count tooltip's rows off the cached totals: everything the
    /// metadata panel's library sheet lists, whether or not the strip
    /// shows the readout.
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

/// The row set both hover cards share: each readout's label with its
/// formatted value, the metadata panel's library sheet as a list.
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

/// The distinct genres behind a set of syms, split the way the genre
/// grid splits compound tags, so the counts agree across the app.
fn genre_count(syms: HashSet<u32>, strings: &[String]) -> usize {
    let mut genres: HashSet<&str> = HashSet::new();
    for sym in syms {
        genres.extend(rox_library::genre::split(&strings[sym as usize]));
    }
    genres.remove("");
    genres.len()
}

/// One pass over the projection for a scope, filtered to the given ids
/// while the set holds any and covering the whole catalog when it's
/// empty. Hands back the track count and the summed time alongside the
/// hover card's rows, since the menubar's status line reads those two off
/// the same walk. The panel keeps its own richer scan; this one is what
/// the surfaces with nowhere to cache share.
fn scope_totals(
    library: &Entity<Library>,
    selected: &HashSet<i64>,
    cx: &App,
) -> (usize, u64, Vec<(SharedString, SharedString)>) {
    let Some(projection) = library.read(cx).projection() else {
        return (0, 0, Vec::new());
    };
    let mut tracks = 0usize;
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
    (tracks, total_ms, rows)
}

/// The selected ids as a set, for scoping a scan.
fn selected_ids(selection: &Entity<Selection>, cx: &App) -> HashSet<i64> {
    selection.read(cx).tracks().iter().copied().collect()
}

/// The whole catalog's totals as a hover card, computed on open. The
/// menubar's track count uses this one: no panel stands behind it, so
/// there's nowhere to cache and one projection scan per hover is fine.
pub fn library_tooltip(library: &Entity<Library>, cx: &mut App) -> AnyView {
    let (_, _, rows) = scope_totals(library, &HashSet::new(), cx);
    cx.new(|_| TotalsTooltip {
        scope: rox_i18n::t!("panel-title-library"),
        rows,
    })
    .into()
}

/// The standing selection's totals as a hover card, the counterpart to
/// [`library_tooltip`] for the menubar's line once a pick scopes it.
pub fn selection_tooltip(
    library: &Entity<Library>,
    selection: &Entity<Selection>,
    cx: &mut App,
) -> AnyView {
    let (_, _, rows) = scope_totals(library, &selected_ids(selection, cx), cx);
    cx.new(|_| TotalsTooltip {
        scope: rox_i18n::t!("status-scope-selection"),
        rows,
    })
    .into()
}

/// The selection's track count and summed time, or None while nothing is
/// picked or the catalog has none of what is. The menubar's status line
/// runs this when the selection or the catalog moves and shows the cached
/// pair in between, the way the strip caches its own readouts.
pub fn selection_summary(
    library: &Entity<Library>,
    selection: &Entity<Selection>,
    cx: &App,
) -> Option<(usize, u64)> {
    let selected = selected_ids(selection, cx);
    if selected.is_empty() {
        return None;
    }
    let (tracks, total_ms, _) = scope_totals(library, &selected, cx);
    (tracks > 0).then_some((tracks, total_ms))
}

/// The count's hover card: the scope's full readout set, the stats
/// widget's tooltip shape. Opaque fill like the popup menus, since it
/// floats over panel content with no backdrop behind it.
struct TotalsTooltip {
    /// "Library", or "Selection" while one scopes the numbers.
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
        // An empty catalog with nothing picked stays quiet, like the
        // track info panel at idle.
        let Some(totals) = self.totals.as_ref().filter(|t| t.tracks > 0) else {
            return root;
        };
        // The count leads in the text color; every other readout is
        // muted behind it, the classic status bar weighting.
        let stat = |text: SharedString| {
            div()
                .min_w_0()
                .truncate()
                .text_color(palette::text_muted())
                .child(text)
                .into_any_element()
        };
        // The strip renders the config's list as-is: each shown readout
        // in its place, whatever order the arrange editor left them in.
        let weak = cx.entity().downgrade();
        let pieces: Vec<AnyElement> = self
            .config
            .items
            .iter()
            .map(|item| match item {
                StatusItem::Count => {
                    // A selection that resolves to one name shows the name:
                    // the track's title alone, the album's for a one-album
                    // set. Past that the plain count takes over. Titles run
                    // long, so this one truncates instead of pinning.
                    let label = match (totals.selection, totals.tracks) {
                        (true, n) => totals.selection_label.clone().unwrap_or_else(|| {
                            rox_i18n::t!("status-count-selected", count = n as u64).to_string()
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
                    // The hover shows the whole readout set, so the count
                    // covers the readouts the strip hides.
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

// Wide enough for the count and a long total side by side; the height
// floor is one line of text, so the strip squeezes to a true status bar
// instead of holding the stock panel minimum.
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

    /// A layout with no items field decodes to the stock pair, so strips
    /// saved before the readouts became a list look unchanged.
    #[test]
    fn missing_items_default_to_count_and_time() {
        let config: StatusConfig = serde_json::from_str("{}").unwrap();
        assert!(config.items == vec![StatusItem::Count, StatusItem::Time]);
    }

    /// A layout with the list uses it as-is, duplicates dropped,
    /// and round-trips through a save.
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
