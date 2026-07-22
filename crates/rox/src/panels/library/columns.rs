//! The library's column model: the column registry, the per-panel column
//! and grouping configuration, and how a panel's layout maps into the table
//! widget's columns.

use gpui::{px, SharedString};
use gpui_component::table::{Column, ColumnSort};
use gpui_component::Size;
use rox_library::projection::SortKey;
use serde::{Deserialize, Serialize};

use crate::group_head::Headers;
use crate::panel::PanelChrome;
use crate::query::shared_query::QuerySource;

/// One column the library can show: its stable key, header label, default
/// width, and whether it renders right-aligned. The registry order is the
/// default display order; the default visible set is marked per entry.
pub(crate) struct ColumnDef {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    pub(crate) default_width: f32,
    pub(crate) right: bool,
    /// Shown when a panel has no saved column layout.
    pub(crate) default_on: bool,
    pub(crate) sort: SortKey,
}

/// Every column the library knows how to draw. Adding a column is one line
/// here plus its arm in [`TrackTable::render_td`].
pub(crate) const COLUMNS: &[ColumnDef] = &[
    ColumnDef {
        // The cover thumbnail. Not sortable (art is not a projection field),
        // so `sort` here is never read; `sort_key` returns None for it.
        key: "cover",
        label: "Cover",
        default_width: 36.,
        right: false,
        default_on: false,
        sort: SortKey::TrackNo,
    },
    ColumnDef {
        key: "track",
        label: "#",
        default_width: 44.,
        right: true,
        default_on: true,
        sort: SortKey::TrackNo,
    },
    ColumnDef {
        key: "title",
        label: "Title",
        default_width: 420.,
        right: false,
        default_on: true,
        sort: SortKey::Title,
    },
    ColumnDef {
        key: "artist",
        label: "Artist",
        default_width: 220.,
        right: false,
        default_on: true,
        sort: SortKey::Artist,
    },
    ColumnDef {
        key: "album_artist",
        label: "Album Artist",
        default_width: 220.,
        right: false,
        default_on: false,
        sort: SortKey::AlbumArtist,
    },
    ColumnDef {
        key: "album",
        label: "Album",
        default_width: 220.,
        right: false,
        default_on: true,
        sort: SortKey::Album,
    },
    ColumnDef {
        key: "genre",
        label: "Genre",
        default_width: 140.,
        right: false,
        default_on: false,
        sort: SortKey::Genre,
    },
    ColumnDef {
        key: "year",
        label: "Year",
        default_width: 56.,
        right: true,
        default_on: false,
        sort: SortKey::Year,
    },
    ColumnDef {
        key: "codec",
        label: "Codec",
        default_width: 64.,
        right: false,
        default_on: false,
        sort: SortKey::Codec,
    },
    ColumnDef {
        key: "bitrate",
        label: "Kbps",
        default_width: 64.,
        right: true,
        default_on: true,
        sort: SortKey::Bitrate,
    },
    ColumnDef {
        key: "duration",
        label: "Time",
        default_width: 64.,
        right: true,
        default_on: true,
        sort: SortKey::Duration,
    },
    ColumnDef {
        key: "rating",
        label: "Rating",
        default_width: 110.,
        right: false,
        default_on: true,
        sort: SortKey::Rating,
    },
    ColumnDef {
        // The heart toggle. Not sortable (favourites live in a playlist, not
        // the projection the sort runs over), so `sort` here is never read;
        // `sort_key` returns None for it.
        key: "favourite",
        label: "Fav",
        default_width: 44.,
        right: false,
        default_on: false,
        sort: SortKey::Rating,
    },
    ColumnDef {
        key: "plays",
        label: "Plays",
        default_width: 56.,
        right: true,
        default_on: false,
        sort: SortKey::Plays,
    },
    ColumnDef {
        key: "added",
        label: "Scanned",
        default_width: 84.,
        right: true,
        default_on: false,
        sort: SortKey::Added,
    },
];

/// The registry entry for a key.
pub(crate) fn column_def(key: &str) -> Option<&'static ColumnDef> {
    COLUMNS.iter().find(|c| c.key == key)
}

/// One shown column: its registry key and current width. The order of the
/// vec is the display order, so this carries visibility, order, and width
/// together. An empty layout means the registry's default set.
#[derive(Clone, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub key: String,
    pub width: f32,
}

/// The registry's default visible columns, in registry order.
fn default_layout() -> Vec<ColumnSpec> {
    COLUMNS
        .iter()
        .filter(|c| c.default_on)
        .map(|c| ColumnSpec {
            key: c.key.to_string(),
            width: c.default_width,
        })
        .collect()
}

/// The row height for the track list. Compact packs a large library
/// tight, comfortable gives each row room; both persist per panel.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Compact,
    Comfortable,
}


/// What the group headers break on. Album keys the album artist and
/// album together over the canonical order as-is; the rest key one
/// field, and genre and year re-sort the list by that field first
/// (canonical inside each group), since the canonical order doesn't
/// keep their runs contiguous.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    #[default]
    Album,
    /// The album artist, the canonical order's leading key.
    Artist,
    Genre,
    Year,
}

impl GroupBy {
    /// The re-sort a grouping needs before its runs are contiguous;
    /// None keeps the canonical order (album and artist already are).
    pub(crate) fn sort(self) -> Option<SortKey> {
        match self {
            GroupBy::Album | GroupBy::Artist => None,
            GroupBy::Genre => Some(SortKey::Genre),
            GroupBy::Year => Some(SortKey::Year),
        }
    }
}

impl Density {
    pub(crate) fn size(self) -> Size {
        match self {
            Density::Compact => Size::Small,
            Density::Comfortable => Size::Large,
        }
    }
}

/// The panel's per-view config: what a saved layout restores, and the
/// schema a future per-panel settings menu edits. One struct serves both,
/// so new knobs land here.
#[derive(Serialize, Deserialize)]
pub struct LibraryConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub query: String,
    /// Show the search box; the query only applies while it shows. Off by
    /// default; the tab's own filter is opt-in, not always on.
    #[serde(default)]
    pub search: bool,
    /// Whether this panel filters by its own query or follows the shared
    /// app-wide one. Shared by default; switch a duplicated panel to its own
    /// query for an independent filter.
    #[serde(default)]
    pub query_source: QuerySource,
    /// The track list's row height.
    #[serde(default)]
    pub density: Density,
    /// How the canonical order shows its group breaks.
    #[serde(default)]
    pub headers: Headers,
    /// What the headers group on while they show.
    #[serde(default)]
    pub group_by: GroupBy,
    /// The shown columns in display order, each with its width. Empty
    /// restores the registry default set. Named apart from the old
    /// index-keyed `columns` field so pre-registry layouts drop their
    /// widths quietly instead of failing the whole config.
    #[serde(default)]
    pub column_layout: Vec<ColumnSpec>,
    /// The sorted column's registry key. None browses the canonical
    /// album artist, album, track order.
    #[serde(default)]
    pub sort_key: Option<String>,
    #[serde(default)]
    pub sort_desc: bool,
    /// The view row at the top of the viewport, so a relaunch reopens the
    /// list where it was left. An index, not pixels: it survives a density
    /// change, and drifts at most a group's headers if the catalog shifts.
    #[serde(default)]
    pub scroll_row: usize,
    /// Scroll to the playing row when the track changes.
    #[serde(default)]
    pub follow_playing: bool,
    /// After the list sits untouched for a spell, scroll back to the
    /// playing row on its own. Off by default; a browse surface only chases
    /// the player once you ask it to.
    #[serde(default)]
    pub resume_playing: bool,
    /// Glide there instead of jumping.
    #[serde(default)]
    pub smooth_follow: bool,
    /// The group headers' cover tile corner radius, in px.
    #[serde(default)]
    pub art_rounding: f32,
    /// Show the expanded album headers' cover tile.
    #[serde(default = "default_true")]
    pub header_art: bool,
    /// Show the year on the group headers' name line.
    #[serde(default = "default_true")]
    pub header_year: bool,
    /// Show the genre and quality on the expanded headers' meta line;
    /// the track count and total time always stay.
    #[serde(default = "default_true")]
    pub header_details: bool,
}

fn default_true() -> bool {
    true
}

// Hand-written over derived for the one default-true knob.
impl Default for LibraryConfig {
    fn default() -> Self {
        LibraryConfig {
            chrome: PanelChrome::default(),
            query: String::new(),
            search: false,
            query_source: QuerySource::default(),
            density: Density::default(),
            headers: Headers::default(),
            group_by: GroupBy::default(),
            column_layout: Vec::new(),
            sort_key: None,
            sort_desc: false,
            scroll_row: 0,
            follow_playing: false,
            resume_playing: false,
            smooth_follow: false,
            art_rounding: 0.,
            header_art: true,
            header_year: true,
            header_details: true,
        }
    }
}

/// Build the table columns from a saved layout (or the default set),
/// marking the active sort's direction on its column. Unknown keys in a
/// hand-edited layout are skipped.
pub(crate) fn track_columns(layout: &[ColumnSpec], sort: &Option<(SharedString, bool)>) -> Vec<Column> {
    let specs = if layout.is_empty() {
        default_layout()
    } else {
        layout.to_vec()
    };
    specs
        .iter()
        .filter_map(|spec| {
            let def = column_def(&spec.key)?;
            let state = match sort {
                Some((k, desc)) if k.as_ref() == def.key => {
                    if *desc {
                        ColumnSort::Descending
                    } else {
                        ColumnSort::Ascending
                    }
                }
                _ => ColumnSort::Default,
            };
            let column = Column::new(def.key, def.label).width(px(spec.width));
            // The favourite column toggles, it does not sort; leaving its sort
            // unset keeps the header from cycling a sort that goes nowhere.
            let column = if sort_key(def.key).is_some() {
                column.sort(state)
            } else {
                column
            };
            Some(if def.right {
                column.text_right()
            } else {
                column
            })
        })
        .collect()
}

/// Map a column key to the projection's sort key. The favourite column has
/// none - it toggles rather than sorts - so its header never triggers a sort.
pub(crate) fn sort_key(key: &str) -> Option<SortKey> {
    if key == "favourite" || key == "cover" {
        return None;
    }
    column_def(key).map(|def| def.sort)
}
