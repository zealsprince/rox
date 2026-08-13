//! The status strip: one quiet line with what the current scope holds,
//! the classic status bar readout. A standing selection scopes it to the
//! picked tracks; otherwise the whole catalog answers. The readouts are
//! an ordered items list like the transport strips', recomputed only when
//! the selection or the catalog moves, so the strip costs nothing per
//! frame.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use gpui::{
    div, prelude::*, px, AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    Pixels, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::group_head;
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::selection::SelectionEvent;
use crate::transport::transport_panel;

/// One readout of the status strip, the arrange editor's unit. The
/// config's list carries the shown ones in display order.
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
    /// strip holds as many as the layout wants.
    Spacer,
}

/// The strip's full catalog in stock order: what the arrange editor
/// offers, and where a menu toggle slots a re-shown readout back in.
const ITEMS: &[panel::ArrangeSpec<StatusItem>] = &[
    panel::ArrangeSpec {
        label: "Count",
        icon: Some(icons::LIST_MUSIC),
        value: StatusItem::Count,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Time",
        icon: Some(icons::CLOCK),
        value: StatusItem::Time,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Albums",
        icon: Some(icons::DISC),
        value: StatusItem::Albums,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Artists",
        icon: Some(icons::MIC),
        value: StatusItem::Artists,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Plays",
        icon: Some(icons::CHART_PIE),
        value: StatusItem::Plays,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Spacer",
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
/// distinct album and artist counts, play total, and whether a selection
/// scoped it.
struct Totals {
    tracks: usize,
    total_ms: u64,
    albums: usize,
    artists: usize,
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
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
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
            _selection_changed,
            _library_changed,
        }
    }

    /// The panel's own dropdown entries: quick show/hide per readout. A
    /// re-shown one slots back at its stock position; the settings
    /// window's arrange editor is where the order changes.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let mut menu = menu;
        for (name, value) in [
            ("Count", StatusItem::Count),
            ("Time", StatusItem::Time),
            ("Albums", StatusItem::Albums),
            ("Artists", StatusItem::Artists),
            ("Plays", StatusItem::Plays),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(name)
                    .checked(self.config.items.contains(&value))
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            this.config.items = panel::toggled(ITEMS, &this.config.items, value);
                            cx.notify();
                        });
                    }),
            );
        }
        menu
    }

    /// The readouts, computed on a miss: one pass over the projection,
    /// filtered to the selection while one stands. A selected id the
    /// catalog no longer knows just drops out of the sums. Albums key on
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
        let mut plays = 0u64;
        let mut first_ix: Option<u32> = None;
        for (ix, id) in projection.db_id.iter().enumerate() {
            if !selected.is_empty() && !selected.contains(id) {
                continue;
            }
            tracks += 1;
            total_ms += u64::from(projection.duration_ms[ix]);
            albums.insert((projection.album_artist[ix], projection.album[ix]));
            artists.insert(projection.album_artist[ix]);
            plays += u64::from(projection.plays[ix].load(Ordering::Relaxed));
            if first_ix.is_none() {
                first_ix = Some(ix as u32);
            }
        }
        let selection = !selected.is_empty();
        // Name the selection where one name covers it: any picked row
        // answers for the whole set once the counts say it's one track or
        // one album. The album only claims the label when the selection
        // holds all of it - a partial pick reads "N selected" instead of
        // wearing the full album's name.
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
            // The artist rides along, the header rows' fallback rule:
            // an empty album artist borrows the first track's artist.
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
            plays,
            selection,
            selection_label,
        });
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
                "Readouts",
                Some(
                    "Drag along the bar to reorder; drag between the rows, \
                     or use a chip's x and plus, to hide and show",
                ),
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
        // The count leads in the text color; every other readout sits
        // muted behind it, the classic status bar weighting.
        let stat = |text: String| {
            div()
                .min_w_0()
                .truncate()
                .text_color(palette::text_muted())
                .child(text)
                .into_any_element()
        };
        let noun = |n: usize, one: &str, many: &str| {
            if n == 1 {
                format!("1 {one}")
            } else {
                format!("{n} {many}")
            }
        };
        // The strip renders the config's list as-is: each shown readout
        // in its place, whatever order the arrange editor left them in.
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
                        (true, n) => totals
                            .selection_label
                            .clone()
                            .unwrap_or_else(|| format!("{n} selected")),
                        (false, n) => noun(n, "track", "tracks"),
                    };
                    div().min_w_0().truncate().child(label).into_any_element()
                }
                StatusItem::Time => stat(group_head::fmt_total(totals.total_ms)),
                StatusItem::Albums => stat(noun(totals.albums, "album", "albums")),
                StatusItem::Artists => stat(noun(totals.artists, "artist", "artists")),
                StatusItem::Plays => stat(noun(totals.plays as usize, "play", "plays")),
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
    "Status",
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

    /// A layout that carries the list uses it as-is, duplicates dropped,
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
