//! The library's column model: the column registry, the per-panel column
//! and grouping configuration, and how a panel's layout maps into the table
//! widget's columns.

use gpui::{px, SharedString};
use gpui_component::table::{Column, ColumnSort};
use rox_library::projection::SortKey;
use serde::{Deserialize, Serialize};

use crate::group_head::{self, ArtSide, HeadPiece, Headers};
use crate::panel::{dedup, PanelChrome};
use crate::query::shared_query::QuerySource;
use crate::settings::ui as settings_ui;

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
        // The sample rate as kHz, the label carrying the unit so the cell
        // stays a bare number beside the bitrate.
        key: "sample_rate",
        label: "kHz",
        default_width: 64.,
        right: true,
        default_on: false,
        sort: SortKey::SampleRate,
    },
    ColumnDef {
        key: "bit_depth",
        label: "Bits",
        default_width: 48.,
        right: true,
        default_on: false,
        sort: SortKey::BitDepth,
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
    ColumnDef {
        // How much each track resembles the one playing, off the acoustic
        // vectors. Not a projection field, so `sort_key` returns None and
        // `compute_view` orders it on the delegate's own score map; the
        // `sort` here is never read. Only offered while acoustic analysis
        // is switched on, per [`offered`].
        key: "similar",
        label: "Similar",
        default_width: 64.,
        right: true,
        default_on: false,
        sort: SortKey::Title,
    },
];

/// The columns a picker should offer: the registry, minus the ones whose
/// feature is switched off. Only discovery is gated, the same way the panel
/// catalog gates its experimental run: a saved layout already holding the
/// column keeps drawing it, and the cells simply read empty without vectors
/// to score against.
pub(crate) fn offered() -> impl Iterator<Item = &'static ColumnDef> {
    let acoustic = crate::settings::acoustic_analysis();
    COLUMNS
        .iter()
        .filter(move |def| acoustic || def.key != "similar")
}

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

/// The old two-way row height choice, kept only so layouts saved before
/// the height sliders still decode; [`fold_row_heights`] maps it onto the
/// pixel knobs.
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

/// The slider bounds for the row and header-line heights, px at the stock
/// font size.
pub(crate) const ROW_HEIGHT_MIN: f32 = 18.;
pub(crate) const ROW_HEIGHT_MAX: f32 = 48.;
pub(crate) const HEAD_HEIGHT_MAX: f32 = 72.;

/// The gap and margin sliders' ceilings, same units: the open space over
/// and under a header block, and the cover tile's inset inside the block.
pub(crate) const HEAD_GAP_MAX: f32 = 24.;
pub(crate) const ART_MARGIN_MAX: f32 = 16.;

/// A saved margin knob read back clamped to the band its input reaches,
/// not the strip's own top, so a typed value survives a reload; nonsense
/// in a hand-edited dump falls to zero.
pub(crate) fn fold_margin(v: f32, max: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0., settings_ui::ceiling(0., max))
    } else {
        0.
    }
}

/// The saved row and header-line heights, the legacy density folded in: a
/// layout from before the sliders carries Compact or Comfortable, which
/// map to the 30 and 40 px the Small and Large table sizes drew. Missing
/// or nonsense values fall back to the stock shape, the header line
/// defaulting to the row height.
pub(crate) fn fold_row_heights(config: &LibraryConfig) -> (f32, f32) {
    let stock = match config.density {
        Some(Density::Comfortable) => 40.,
        _ => 30.,
    };
    let clamp = |v: Option<f32>, default: f32, max: f32| match v {
        Some(v) if v.is_finite() => {
            v.clamp(ROW_HEIGHT_MIN, settings_ui::ceiling(ROW_HEIGHT_MIN, max))
        }
        _ => default,
    };
    let row = clamp(config.row_height, stock, ROW_HEIGHT_MAX);
    let head = clamp(config.head_height, row, HEAD_HEIGHT_MAX);
    (row, head)
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
    /// The track rows' height, px at the stock font size; the app font
    /// scale and the panel override multiply it at render. Replaces the
    /// old two-way density.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_height: Option<f32>,
    /// One header line's height, same scaling, independent of the rows: a
    /// header block spans however many table rows its lines need.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_height: Option<f32>,
    /// Pre-slider layouts' density choice; folds into the heights and
    /// never writes back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<Density>,
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
    /// Which side of the header block the cover tile sits on.
    #[serde(default)]
    pub art_side: ArtSide,
    /// The cover tile's inset from the block edges, px at the stock font
    /// size; the tile shrinks to keep the square.
    #[serde(default)]
    pub art_margin: f32,
    /// Open space over each header block, same units; the list shows
    /// through, so a block reads apart from the run above it.
    #[serde(default)]
    pub header_gap_above: f32,
    /// The same under the block, before its own tracks.
    #[serde(default)]
    pub header_gap_below: f32,
    /// Show the expanded album headers' cover tile.
    #[serde(default = "default_true")]
    pub header_art: bool,
    /// Sit the header rows on the list background instead of the raised
    /// Elevated tint. A role, not a color, so song theming moves the
    /// headers together with the list.
    #[serde(default)]
    pub header_flush: bool,
    /// The compact header's composed row, left to right; empty falls back
    /// to the stock packing (folding the legacy year toggle below).
    #[serde(default)]
    pub header_compact: Vec<HeadPiece>,
    /// The expanded block's composed lines, top to bottom; a rendered
    /// block drops its empty lines. Empty falls back to the stock name and
    /// meta pair (folding the legacy toggles below).
    #[serde(default)]
    pub header_lines: Vec<Vec<HeadPiece>>,
    /// Pre-composition layouts' year toggle; folds into the stock lines
    /// when no composed lines are saved. Never written back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_year: Option<bool>,
    /// The genre-and-quality toggle from the same era, folded the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_details: Option<bool>,
    /// Draw the plays column as a small count with a faint dash beside
    /// it, the classic playlist tick, instead of the plain number.
    #[serde(default)]
    pub compact_plays: bool,
    /// Tint every other track row so a long list scans.
    #[serde(default = "default_true")]
    pub stripes: bool,
    /// Draw the hairline under each track row.
    #[serde(default = "default_true")]
    pub row_borders: bool,
    /// Draw the column header row over the list; sorting and column
    /// resizing live there, so hiding it freezes the current layout.
    #[serde(default = "default_true")]
    pub column_headers: bool,
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
            row_height: None,
            head_height: None,
            density: None,
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
            art_side: ArtSide::default(),
            art_margin: 0.,
            header_gap_above: 0.,
            header_gap_below: 0.,
            header_art: true,
            header_flush: false,
            header_compact: Vec::new(),
            header_lines: Vec::new(),
            header_year: None,
            header_details: None,
            compact_plays: false,
            stripes: true,
            row_borders: true,
            column_headers: true,
        }
    }
}

/// How many expanded line slots the config carries and the editors show.
pub(crate) const HEAD_LINE_SLOTS: usize = 3;

/// The saved header composition folded to the editors' shape: the compact
/// row, and exactly [`HEAD_LINE_SLOTS`] expanded lines. A layout saved
/// before composition carries the year and details toggles instead; those
/// fold into the stock lines, so old headers look unchanged. Hand-edited
/// lists come back deduped and capped.
pub(crate) fn fold_head_lines(config: &LibraryConfig) -> (Vec<HeadPiece>, Vec<Vec<HeadPiece>>) {
    let year = config.header_year.unwrap_or(true);
    let details = config.header_details.unwrap_or(true);
    let compact = if config.header_compact.is_empty() {
        let mut row = group_head::stock_compact();
        if !year {
            row.retain(|p| *p != HeadPiece::Year);
        }
        row
    } else {
        dedup(config.header_compact.clone())
    };
    let mut lines: Vec<Vec<HeadPiece>> = if config.header_lines.is_empty() {
        let mut name = group_head::stock_name_line();
        if !year {
            name.retain(|p| *p != HeadPiece::Year);
        }
        let mut meta = group_head::stock_meta_line();
        if !details {
            meta.retain(|p| !matches!(p, HeadPiece::Genre | HeadPiece::Quality));
        }
        vec![name, meta]
    } else {
        config
            .header_lines
            .iter()
            .take(HEAD_LINE_SLOTS)
            .map(|line| dedup(line.clone()))
            .collect()
    };
    lines.resize(HEAD_LINE_SLOTS, Vec::new());
    (compact, lines)
}

/// Build the table columns from a saved layout (or the default set),
/// marking the active sort's direction on its column. Unknown keys in a
/// hand-edited layout are skipped.
pub(crate) fn track_columns(
    layout: &[ColumnSpec],
    sort: &Option<(SharedString, bool)>,
) -> Vec<Column> {
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
            // The cover and favourite columns show rather than sort; leaving
            // their sort unset keeps the header from cycling a sort that goes
            // nowhere.
            let column = if sortable(def.key) {
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

/// Mirror a header's advanced sort cycle onto a built column set: the
/// clicked column takes the new state, the rest fall back to canonical.
/// The columns that show rather than sort stay stateless throughout, since
/// the table hands the delegate's columns back to its own column groups on
/// the next refresh and a cover header carrying a state is one a click can
/// sort the list by nothing with.
pub(crate) fn mirror_sort(columns: &mut [Column], col_ix: usize, sort: ColumnSort) {
    for (ix, column) in columns.iter_mut().enumerate() {
        if !sortable(column.key.as_ref()) {
            continue;
        }
        column.sort = Some(if ix == col_ix {
            sort
        } else {
            ColumnSort::Default
        });
    }
}

/// Whether a column's header offers a sort at all. Wider than [`sort_key`]:
/// Similar sorts without a projection key behind it, on the delegate's own
/// score map, and the table widget ignores a header click on a column that
/// carries no sort state, so this is what decides whether the column gets
/// one built.
pub(crate) fn sortable(key: &str) -> bool {
    key == "similar" || sort_key(key).is_some()
}

/// Map a column key to the projection's sort key. Three columns have none:
/// the cover and favourite ones show rather than sort, and Similar sorts on
/// scores the delegate holds rather than a projection field.
pub(crate) fn sort_key(key: &str) -> Option<SortKey> {
    if key == "favourite" || key == "cover" || key == "similar" {
        return None;
    }
    column_def(key).map(|def| def.sort)
}

#[cfg(test)]
mod tests {
    use super::{
        fold_head_lines, mirror_sort, settings_ui, track_columns, ColumnSort, ColumnSpec,
        HeadPiece, LibraryConfig, SharedString, HEAD_HEIGHT_MAX, HEAD_LINE_SLOTS, ROW_HEIGHT_MIN,
    };

    /// Sorting leaves the show-only columns alone. The table reads the
    /// delegate's columns back into its own column groups on a refresh, so
    /// a state written onto cover or favourite here comes back as a header
    /// that cycles, sorts by nothing, and saves the dead key into the
    /// layout. Similar has to keep cycling through the same pass, which is
    /// why the gate is [`sortable`] and not [`sort_key`].
    #[test]
    fn a_sort_leaves_the_show_only_columns_alone() {
        let layout: Vec<ColumnSpec> = ["title", "cover", "favourite", "similar"]
            .iter()
            .map(|key| ColumnSpec {
                key: key.to_string(),
                width: 64.,
            })
            .collect();
        let mut columns = track_columns(&layout, &None);
        let ix = |columns: &[super::Column], key: &str| {
            columns
                .iter()
                .position(|c| c.key.as_ref() == key)
                .unwrap_or_else(|| panic!("{key} should be built"))
        };
        let of = |columns: &[super::Column], key: &str| {
            columns
                .iter()
                .find(|c| c.key.as_ref() == key)
                .unwrap_or_else(|| panic!("{key} should be built"))
                .sort
        };

        let similar = ix(&columns, "similar");
        mirror_sort(&mut columns, similar, ColumnSort::Descending);
        assert!(of(&columns, "similar") == Some(ColumnSort::Descending));
        assert!(of(&columns, "title") == Some(ColumnSort::Default));
        assert!(
            of(&columns, "cover").is_none(),
            "the cover column shows, it sorts on nothing"
        );
        assert!(of(&columns, "favourite").is_none(), "and the heart toggles");

        // The next click moves the cycle to another column, and the two
        // stateless ones stay that way.
        let title = ix(&columns, "title");
        mirror_sort(&mut columns, title, ColumnSort::Ascending);
        assert!(of(&columns, "title") == Some(ColumnSort::Ascending));
        assert!(of(&columns, "similar") == Some(ColumnSort::Default));
        assert!(of(&columns, "cover").is_none());
        assert!(of(&columns, "favourite").is_none());
    }

    /// A restored layout has to build the Similar column sortable. Its
    /// ordering runs off the delegate's score map rather than a projection
    /// key, so the projection's answer can't be what decides: a column with
    /// no sort state on it is one the table widget ignores header clicks
    /// for, and the arrow never draws.
    #[test]
    fn a_restored_similar_column_sorts() {
        let layout: Vec<ColumnSpec> = ["title", "cover", "favourite", "similar"]
            .iter()
            .map(|key| ColumnSpec {
                key: key.to_string(),
                width: 64.,
            })
            .collect();
        let sort = Some((SharedString::from("similar"), true));
        let columns = track_columns(&layout, &sort);
        let of = |key: &str| {
            columns
                .iter()
                .find(|c| c.key.as_ref() == key)
                .unwrap_or_else(|| panic!("{key} should be built"))
                .sort
        };
        assert!(of("similar") == Some(ColumnSort::Descending));
        assert!(of("title") == Some(ColumnSort::Default));
        assert!(
            of("cover").is_none(),
            "the cover column shows, it sorts on nothing"
        );
        assert!(of("favourite").is_none(), "and the heart toggles");
    }

    /// A layout saved before composition folds its year and details
    /// toggles into the stock lines, so old headers look unchanged.
    #[test]
    fn legacy_toggles_fold_into_stock_lines() {
        let config: LibraryConfig =
            serde_json::from_str(r#"{"header_year": false, "header_details": false}"#).unwrap();
        let (compact, lines) = fold_head_lines(&config);
        assert!(compact == vec![HeadPiece::Artist, HeadPiece::Album, HeadPiece::Spacer]);
        assert!(lines.len() == HEAD_LINE_SLOTS);
        assert!(lines[0] == vec![HeadPiece::Artist, HeadPiece::Spacer]);
        assert!(
            lines[1]
                == vec![
                    HeadPiece::Album,
                    HeadPiece::Spacer,
                    HeadPiece::Tracks,
                    HeadPiece::Time,
                ]
        );
        assert!(lines[2].is_empty());
    }

    /// A layout that carries composed lines uses them as-is, deduped and
    /// padded to the editors' slots, and its save drops the legacy toggles.
    #[test]
    fn composed_lines_read_ordered_and_round_trip() {
        let config: LibraryConfig = serde_json::from_str(
            r#"{"header_lines": [["artist"], ["album", "spacer", "year", "album"]]}"#,
        )
        .unwrap();
        let (_, lines) = fold_head_lines(&config);
        assert!(lines.len() == HEAD_LINE_SLOTS);
        assert!(lines[0] == vec![HeadPiece::Artist]);
        assert!(lines[1] == vec![HeadPiece::Album, HeadPiece::Spacer, HeadPiece::Year]);
        assert!(lines[2].is_empty());

        let saved = serde_json::to_value(&config).unwrap();
        assert!(saved.get("header_year").is_none());
        assert!(saved.get("header_details").is_none());
        let back: LibraryConfig = serde_json::from_value(saved).unwrap();
        assert!(back.header_lines == config.header_lines);
    }

    /// A layout from before the height sliders carries a density choice;
    /// it folds onto the pixel heights the old sizes drew, the header
    /// line following the rows.
    #[test]
    fn legacy_density_folds_into_heights() {
        let config: LibraryConfig = serde_json::from_str(r#"{"density": "comfortable"}"#).unwrap();
        assert!(super::fold_row_heights(&config) == (40., 40.));

        let config: LibraryConfig = serde_json::from_str("{}").unwrap();
        assert!(super::fold_row_heights(&config) == (30., 30.));
    }

    /// Saved heights read back clamped to the band the readout's input
    /// reaches, not the strip's own top, so a typed height survives the
    /// reload. Nonsense falls to the stock shape, and a save drops the
    /// legacy density.
    #[test]
    fn heights_clamp_and_round_trip() {
        let config: LibraryConfig =
            serde_json::from_str(r#"{"row_height": 4.0, "head_height": 5000.0}"#).unwrap();
        let ceiling = settings_ui::ceiling(ROW_HEIGHT_MIN, HEAD_HEIGHT_MAX);
        assert!(super::fold_row_heights(&config) == (18., ceiling));

        // A height typed past the strip's top comes back whole.
        let config: LibraryConfig =
            serde_json::from_str(r#"{"row_height": 96.0, "head_height": 120.0}"#).unwrap();
        assert!(super::fold_row_heights(&config) == (96., 120.));

        let config: LibraryConfig =
            serde_json::from_str(r#"{"row_height": 24.0, "density": "comfortable"}"#).unwrap();
        let (row, head) = super::fold_row_heights(&config);
        assert!((row, head) == (24., 24.));

        let saved = serde_json::to_value(&config).unwrap();
        assert!(saved.get("density").is_some());
        let mut round = config;
        round.row_height = Some(row);
        round.head_height = Some(head);
        round.density = None;
        let saved = serde_json::to_value(&round).unwrap();
        assert!(saved.get("density").is_none());
        assert!(saved.get("row_height").is_some());
    }
}
