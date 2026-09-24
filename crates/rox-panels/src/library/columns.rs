//! The library's column model: the column registry, the per-panel column
//! and grouping configuration, and how a panel's layout maps into the table
//! widget's columns.

use std::collections::HashMap;

use gpui::{SharedString, px};
use gpui_component::table::{Column, ColumnSort};
use rox_library::projection::SortKey;
use rox_panel_kit::config::default_true;
use serde::{Deserialize, Serialize};

use crate::group_head::{self, ArtSide, HeadPiece, Headers, TileFace};
use crate::panel::{PanelChrome, dedup};
use crate::query::shared_query::QuerySource;
use crate::settings::GainModeSetting;

/// One column the library can show. Registry order is the default display
/// order.
pub struct ColumnDef {
    pub key: &'static str,
    pub label: &'static str,
    pub default_width: f32,
    pub right: bool,
    /// Shown when a panel has no saved column layout.
    pub default_on: bool,
    pub sort: SortKey,
}

/// Every column the library knows how to draw. Adding one is an entry here
/// plus its arm in `TrackTable::render_td`. A function because `t_static`
/// isn't const; it leaks once per active locale.
pub fn columns() -> &'static [ColumnDef] {
    static CACHE: std::sync::Mutex<Option<(&'static str, &'static [ColumnDef])>> =
        std::sync::Mutex::new(None);
    let locale = rox_i18n::locale();
    let mut cache = CACHE.lock().unwrap();
    if let Some((cached_locale, cached_columns)) = *cache
        && cached_locale == locale
    {
        return cached_columns;
    }
    let built: Vec<ColumnDef> = vec![
        ColumnDef {
            // Not sortable, so `sort` is never read.
            key: "cover",
            label: rox_i18n::t_static("columns-cover"),
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
            label: rox_i18n::t_static("info-item-title"),
            default_width: 420.,
            right: false,
            default_on: true,
            sort: SortKey::Title,
        },
        ColumnDef {
            // A sort key of its own: the base key falls back to the display
            // name, which would file the few tagged rows among the untagged
            // ones. The sort keys park untagged rows at the bottom either way.
            // The other three sort columns work the same.
            key: "title_sort",
            label: rox_i18n::t_static("columns-title-sort"),
            default_width: 420.,
            right: false,
            default_on: false,
            sort: SortKey::TitleSort,
        },
        ColumnDef {
            key: "artist",
            label: rox_i18n::t_static("head-piece-artist"),
            default_width: 220.,
            right: false,
            default_on: true,
            sort: SortKey::Artist,
        },
        ColumnDef {
            key: "artist_sort",
            label: rox_i18n::t_static("columns-artist-sort"),
            default_width: 220.,
            right: false,
            default_on: false,
            sort: SortKey::ArtistSort,
        },
        ColumnDef {
            key: "album_artist",
            label: rox_i18n::t_static("filter-field-album-artist"),
            default_width: 220.,
            right: false,
            default_on: false,
            sort: SortKey::AlbumArtist,
        },
        ColumnDef {
            key: "album_artist_sort",
            label: rox_i18n::t_static("columns-album-artist-sort"),
            default_width: 220.,
            right: false,
            default_on: false,
            sort: SortKey::AlbumArtistSort,
        },
        ColumnDef {
            key: "album",
            label: rox_i18n::t_static("head-piece-album"),
            default_width: 220.,
            right: false,
            default_on: true,
            sort: SortKey::Album,
        },
        ColumnDef {
            key: "album_sort",
            label: rox_i18n::t_static("columns-album-sort"),
            default_width: 220.,
            right: false,
            default_on: false,
            sort: SortKey::AlbumSort,
        },
        ColumnDef {
            key: "genre",
            label: rox_i18n::t_static("head-piece-genre"),
            default_width: 140.,
            right: false,
            default_on: false,
            sort: SortKey::Genre,
        },
        ColumnDef {
            key: "year",
            label: rox_i18n::t_static("head-piece-year"),
            default_width: 56.,
            right: true,
            default_on: false,
            sort: SortKey::Year,
        },
        ColumnDef {
            key: "codec",
            label: rox_i18n::t_static("columns-codec"),
            default_width: 64.,
            right: false,
            default_on: false,
            sort: SortKey::Codec,
        },
        ColumnDef {
            // "Local" for a file, a server by its given name or host.
            key: "source",
            label: rox_i18n::t_static("columns-source"),
            default_width: 140.,
            right: false,
            default_on: false,
            sort: SortKey::Source,
        },
        ColumnDef {
            key: "bitrate",
            label: rox_i18n::t_static("columns-kbps"),
            default_width: 64.,
            right: true,
            default_on: true,
            sort: SortKey::Bitrate,
        },
        ColumnDef {
            // The unit sits in the label so the cell stays a bare number.
            key: "sample_rate",
            label: rox_i18n::t_static("columns-khz"),
            default_width: 64.,
            right: true,
            default_on: false,
            sort: SortKey::SampleRate,
        },
        ColumnDef {
            key: "bit_depth",
            label: rox_i18n::t_static("columns-bits"),
            default_width: 48.,
            right: true,
            default_on: false,
            sort: SortKey::BitDepth,
        },
        ColumnDef {
            // Follows the Audio page's leveling mode, so `sort` is the Track
            // reading and [`sort_key`] swaps in Album's.
            key: "gain",
            label: rox_i18n::t_static("columns-gain"),
            default_width: 64.,
            right: true,
            default_on: false,
            sort: SortKey::TrackGain,
        },
        ColumnDef {
            key: "duration",
            label: rox_i18n::t_static("head-piece-time"),
            default_width: 64.,
            right: true,
            default_on: true,
            sort: SortKey::Duration,
        },
        ColumnDef {
            // Only offered while tempo analysis is on, per [`offered`].
            key: "bpm",
            label: rox_i18n::t_static("columns-bpm"),
            default_width: 56.,
            right: true,
            default_on: false,
            sort: SortKey::Bpm,
        },
        ColumnDef {
            key: "rating",
            label: rox_i18n::t_static("info-item-rating"),
            default_width: 110.,
            right: false,
            default_on: true,
            sort: SortKey::Rating,
        },
        ColumnDef {
            // Not sortable (favourites live in a playlist, not the
            // projection), so `sort` is never read.
            key: "favourite",
            label: rox_i18n::t_static("columns-fav"),
            default_width: 44.,
            right: false,
            default_on: false,
            sort: SortKey::Rating,
        },
        ColumnDef {
            key: "plays",
            label: rox_i18n::t_static("status-item-plays"),
            default_width: 56.,
            right: true,
            default_on: false,
            sort: SortKey::Plays,
        },
        ColumnDef {
            key: "added",
            label: rox_i18n::t_static("columns-scanned"),
            default_width: 84.,
            right: true,
            default_on: false,
            sort: SortKey::Added,
        },
        ColumnDef {
            // Not a projection field: the view build orders it on the
            // delegate's score map, so `sort` is never read. Only offered
            // while acoustic analysis is on, per [`offered`].
            key: "similar",
            label: rox_i18n::t_static("columns-similar"),
            default_width: 64.,
            right: true,
            default_on: false,
            sort: SortKey::Title,
        },
    ];
    let leaked: &'static [ColumnDef] = Box::leak(built.into_boxed_slice());
    *cache = Some((locale, leaked));
    leaked
}

/// Only discovery is gated: a saved layout already holding the column keeps
/// drawing it.
pub fn offered() -> impl Iterator<Item = &'static ColumnDef> {
    let acoustic = crate::settings::acoustic_analysis();
    let tempo = crate::settings::tempo_analysis();
    columns()
        .iter()
        .filter(move |def| acoustic || def.key != "similar")
        .filter(move |def| tempo || def.key != "bpm")
}

pub fn column_def(key: &str) -> Option<&'static ColumnDef> {
    columns().iter().find(|c| c.key == key)
}

/// Reword built columns in the active language, in place. Order, widths,
/// and the active sort mean the same in every language, so a rebuild would
/// throw away a layout to fix a label.
pub fn reword(columns: &mut [Column], labels: &HashMap<String, String>) {
    for column in columns {
        if let Some(label) = labels.get(column.key.as_ref()) {
            column.name = label.clone().into();
        } else if let Some(def) = column_def(&column.key) {
            column.name = def.label.into();
        }
    }
}

/// An empty string is a real entry: it's how a header is asked to draw
/// blank.
pub fn label_overrides(layout: &[ColumnSpec]) -> HashMap<String, String> {
    layout
        .iter()
        .filter_map(|spec| Some((spec.key.clone(), spec.label.clone()?)))
        .collect()
}

/// The vec's order is the display order. An empty layout means the
/// registry's default set.
#[derive(Clone, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub key: String,
    pub width: f32,
    /// `Some("")` is a blank header, so it has to round-trip apart from None,
    /// the registry label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn default_layout() -> Vec<ColumnSpec> {
    columns()
        .iter()
        .filter(|c| c.default_on)
        .map(|c| ColumnSpec {
            key: c.key.to_string(),
            width: c.default_width,
            label: None,
        })
        .collect()
}

/// Kept only so layouts saved before the height sliders decode;
/// [`fold_row_heights`] maps it onto pixels.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Compact,
    Comfortable,
}

/// Album keys the album artist and album together. Genre and year re-sort
/// by that field first, since the canonical order doesn't keep their runs
/// contiguous.
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
    pub fn sort(self) -> Option<SortKey> {
        match self {
            GroupBy::Album | GroupBy::Artist => None,
            GroupBy::Genre => Some(SortKey::Genre),
            GroupBy::Year => Some(SortKey::Year),
        }
    }

    /// Search starts in projection row order, so album and artist need their
    /// leading canonical key put back; `sort_view` supplies the rest as its
    /// tie-break.
    pub fn search_sort(self) -> SortKey {
        match self {
            GroupBy::Album | GroupBy::Artist => SortKey::AlbumArtist,
            GroupBy::Genre => SortKey::Genre,
            GroupBy::Year => SortKey::Year,
        }
    }
}

/// Shared with the playlists panel, which has the same knobs.
use crate::track_ui::track_columns::fold_row_height;
pub use crate::track_ui::track_columns::{
    ART_MARGIN_MAX, HEAD_GAP_MAX, HEAD_HEIGHT_MAX, HEAD_LINE_SLOTS, HEAD_TEXT_MAX, HEAD_TEXT_MIN,
    HEAD_TEXT_STOCK, ROW_HEIGHT_MAX, ROW_HEIGHT_MIN, ROW_HEIGHT_STOCK, ROW_SPACING_MAX,
    fold_head_text, fold_margin,
};

fn default_head_text() -> f32 {
    HEAD_TEXT_STOCK
}

/// Compact and Comfortable map to the 30 and 40 px the old table sizes drew.
/// The header line defaults to the row height.
pub fn fold_row_heights(config: &LibraryConfig) -> (f32, f32) {
    let stock = match config.density {
        Some(Density::Comfortable) => 40.,
        _ => ROW_HEIGHT_STOCK,
    };
    let row = fold_row_height(config.row_height, stock, ROW_HEIGHT_MAX);
    let head = fold_row_height(config.head_height, row, HEAD_HEIGHT_MAX);
    (row, head)
}

/// The saved layout and the settings window share this struct, so new knobs
/// go here.
#[derive(Serialize, Deserialize)]
pub struct LibraryConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub query: String,
    /// The query only applies while the box shows.
    #[serde(default)]
    pub search: bool,
    #[serde(default)]
    pub query_source: QuerySource,
    /// Px at the stock font size; the app font scale and the panel override
    /// multiply it at render.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_height: Option<f32>,
    /// Independent of the rows: a header block spans however many table rows
    /// its lines need.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_height: Option<f32>,
    /// Extra height grown into each row, without growing the text.
    #[serde(default)]
    pub row_spacing: f32,
    /// Free of the line height, so the cover art (which spans the lines)
    /// grows without dragging the text along.
    #[serde(default = "default_head_text")]
    pub head_text: f32,
    /// Folds into the heights and never writes back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<Density>,
    #[serde(default)]
    pub headers: Headers,
    #[serde(default)]
    pub group_by: GroupBy,
    /// Keep the group headers while a search is active; off lists results
    /// flat.
    #[serde(default = "default_true")]
    pub group_search_results: bool,
    /// Named apart from the old index-keyed `columns` field so pre-registry
    /// layouts drop their widths quietly instead of failing the config.
    #[serde(default)]
    pub column_layout: Vec<ColumnSpec>,
    /// None browses the canonical album artist, album, track order.
    #[serde(default)]
    pub sort_key: Option<String>,
    #[serde(default)]
    pub sort_desc: bool,
    /// The view row at the top of the viewport. An index, not pixels, so it
    /// survives a row height change.
    #[serde(default)]
    pub scroll_row: usize,
    #[serde(default)]
    pub follow_playing: bool,
    /// Scroll back to the playing row once the list goes untouched for a
    /// spell.
    #[serde(default)]
    pub resume_playing: bool,
    #[serde(default)]
    pub smooth_follow: bool,
    #[serde(default)]
    pub art_rounding: f32,
    #[serde(default)]
    pub art_side: ArtSide,
    /// The tile shrinks to keep the square.
    #[serde(default)]
    pub art_margin: f32,
    /// Open space over each header block, where the list shows through.
    #[serde(default)]
    pub header_gap_above: f32,
    /// The same under the block.
    #[serde(default)]
    pub header_gap_below: f32,
    /// Show the expanded headers' cover tile.
    #[serde(default = "default_true")]
    pub header_art: bool,
    /// Round the artist grouping's tiles to the full circle the artist wall
    /// uses.
    #[serde(default = "default_true")]
    pub portrait_circle: bool,
    #[serde(default)]
    pub genre_face: TileFace,
    /// Draw headers on the list background instead of the Elevated tint. A
    /// role, not a color, so song theming moves them with the list.
    #[serde(default)]
    pub header_flush: bool,
    /// Empty falls back to the stock packing.
    #[serde(default)]
    pub header_compact: Vec<HeadPiece>,
    /// A rendered block drops its empty lines. Empty falls back to the stock
    /// name and meta pair.
    #[serde(default)]
    pub header_lines: Vec<Vec<HeadPiece>>,
    /// Pre-composition layouts' year toggle, folded into the stock lines.
    /// Never written back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_year: Option<bool>,
    /// The genre-and-quality toggle from the same era, folded the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_details: Option<bool>,
    /// A small count with a faint dash beside it, the classic playlist tick.
    #[serde(default)]
    pub compact_plays: bool,
    #[serde(default = "default_true")]
    pub stripes: bool,
    #[serde(default = "default_true")]
    pub row_borders: bool,
    /// Sorting and resizing happen in the header row, so hiding it freezes
    /// the layout.
    #[serde(default = "default_true")]
    pub column_headers: bool,
    /// Off by default: it takes the column reorder drag over to Alt and hides
    /// the sort icon.
    #[serde(default)]
    pub sort_on_click: bool,
}

// Hand-written because several knobs default on.
impl Default for LibraryConfig {
    fn default() -> Self {
        LibraryConfig {
            chrome: PanelChrome::default(),
            query: String::new(),
            search: false,
            query_source: QuerySource::default(),
            row_height: None,
            head_height: None,
            row_spacing: 0.,
            head_text: HEAD_TEXT_STOCK,
            density: None,
            headers: Headers::default(),
            group_by: GroupBy::default(),
            group_search_results: true,
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
            portrait_circle: true,
            genre_face: TileFace::default(),
            header_flush: false,
            header_compact: Vec::new(),
            header_lines: Vec::new(),
            header_year: None,
            header_details: None,
            compact_plays: false,
            stripes: true,
            row_borders: true,
            column_headers: true,
            sort_on_click: false,
        }
    }
}

/// A layout saved before composition folds its year and details toggles
/// into the stock lines. Hand-edited lists come back deduped and capped.
pub fn fold_head_lines(config: &LibraryConfig) -> (Vec<HeadPiece>, Vec<Vec<HeadPiece>>) {
    let year = config.header_year.unwrap_or(true);
    let details = config.header_details.unwrap_or(true);
    let compact = if config.header_compact.is_empty() {
        let mut row = group_head::stock_compact();
        if !year {
            row.retain(|p| *p != HeadPiece::Year);
        }
        row
    } else {
        dedup(group_head::PIECES, config.header_compact.clone())
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
            .map(|line| dedup(group_head::PIECES, line.clone()))
            .collect()
    };
    lines.resize(HEAD_LINE_SLOTS, Vec::new());
    (compact, lines)
}

/// Unknown keys in a hand-edited layout are skipped. `labels` is
/// [`label_overrides`] on restore and the panel's live map on a rebuild.
pub fn track_columns(
    layout: &[ColumnSpec],
    sort: &Option<(SharedString, bool)>,
    labels: &HashMap<String, String>,
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
            let label: SharedString = match labels.get(def.key) {
                Some(label) => label.clone().into(),
                None => def.label.into(),
            };
            let column = Column::new(def.key, label).width(px(spec.width));
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

/// Show-only columns stay stateless: the table hands the delegate's columns
/// back to its column groups on refresh, and a cover header with a sort
/// state would sort the list by nothing.
pub fn mirror_sort(columns: &mut [Column], col_ix: usize, sort: ColumnSort) {
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

/// Wider than [`sort_key`]: Similar sorts on the delegate's score map, and
/// the table ignores header clicks on a column with no sort state.
pub fn sortable(key: &str) -> bool {
    key == "similar" || sort_key(key).is_some()
}

pub fn sort_key(key: &str) -> Option<SortKey> {
    if key == "favourite" || key == "cover" || key == "similar" {
        return None;
    }
    // Gain sorts by whichever figure the leveling mode reads, or the order
    // and the numbers disagree. Off falls to the track gain.
    if key == "gain" && crate::settings::gain_mode() == GainModeSetting::Album {
        return Some(SortKey::AlbumGain);
    }
    column_def(key).map(|def| def.sort)
}

#[cfg(test)]
mod tests {
    use super::{
        ColumnSort, ColumnSpec, HEAD_HEIGHT_MAX, HEAD_LINE_SLOTS, HashMap, HeadPiece,
        LibraryConfig, ROW_HEIGHT_MIN, SharedString, fold_head_lines, label_overrides, mirror_sort,
        reword, track_columns,
    };
    use crate::panel::letter_initial;
    use crate::settings::ui as settings_ui;
    use rox_library::projection::SymTable;

    /// Labels are stored on the built column while each header's menu reads
    /// the registry every frame, so without the reword the two disagree.
    #[test]
    fn a_language_switch_rewords_headers_without_moving_them() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        let layout: Vec<ColumnSpec> = ["title", "artist", "year"]
            .iter()
            .map(|key| ColumnSpec {
                key: key.to_string(),
                width: 64.,
                label: None,
            })
            .collect();
        rox_i18n::set_locale(Some("en-CA"));
        let sort = Some((SharedString::from("title"), true));
        let mut columns = track_columns(&layout, &sort, &HashMap::new());
        let english: Vec<String> = columns.iter().map(|c| c.name.to_string()).collect();
        let order: Vec<String> = columns.iter().map(|c| c.key.to_string()).collect();
        let sorts: Vec<Option<ColumnSort>> = columns.iter().map(|c| c.sort).collect();

        rox_i18n::set_locale(Some("ja"));
        reword(&mut columns, &HashMap::new());
        let japanese: Vec<String> = columns.iter().map(|c| c.name.to_string()).collect();
        assert_ne!(english, japanese, "the headers should follow the language");
        assert_eq!(
            order,
            columns
                .iter()
                .map(|c| c.key.to_string())
                .collect::<Vec<_>>(),
            "rewording must not reorder the columns"
        );
        assert_eq!(
            sorts,
            columns.iter().map(|c| c.sort).collect::<Vec<_>>(),
            "rewording must not disturb the active sort"
        );
        assert!(
            columns.iter().all(|c| c.width == gpui::px(64.)),
            "rewording must not resize the columns"
        );

        rox_i18n::set_locale(Some("en-CA"));
        reword(&mut columns, &HashMap::new());
        assert_eq!(
            english,
            columns
                .iter()
                .map(|c| c.name.to_string())
                .collect::<Vec<_>>()
        );
        rox_i18n::set_locale(None);
    }

    /// Dropping `Some("")` to None would put the registry's label back on a
    /// header asked to draw nothing.
    #[test]
    fn a_renamed_header_survives_a_language_switch_and_a_save() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        let layout = vec![
            ColumnSpec {
                key: "title".to_string(),
                width: 64.,
                label: Some("Song".to_string()),
            },
            ColumnSpec {
                key: "artist".to_string(),
                width: 64.,
                label: Some(String::new()),
            },
            ColumnSpec {
                key: "year".to_string(),
                width: 64.,
                label: None,
            },
        ];
        let labels = label_overrides(&layout);
        let mut columns = track_columns(&layout, &None, &labels);
        let names = |columns: &[super::Column]| -> Vec<String> {
            columns.iter().map(|c| c.name.to_string()).collect()
        };
        assert_eq!(names(&columns)[0], "Song");
        assert_eq!(names(&columns)[1], "", "a blank header draws blank");
        let year = names(&columns)[2].clone();

        rox_i18n::set_locale(Some("ja"));
        reword(&mut columns, &labels);
        assert_eq!(names(&columns)[0], "Song", "a typed name has no language");
        assert_eq!(names(&columns)[1], "");
        assert_ne!(names(&columns)[2], year, "the rest still follow the switch");
        rox_i18n::set_locale(None);

        // Blank included, or a relaunch quietly undoes the rename.
        let saved = serde_json::to_string(&layout).unwrap();
        let back: Vec<ColumnSpec> = serde_json::from_str(&saved).unwrap();
        assert_eq!(back[0].label.as_deref(), Some("Song"));
        assert_eq!(back[1].label.as_deref(), Some(""));
        assert_eq!(back[2].label, None);

        // And a layout saved before the rename decodes with none of them.
        let old: Vec<ColumnSpec> =
            serde_json::from_str(r#"[{"key": "title", "width": 64.0}]"#).unwrap();
        assert_eq!(old[0].label, None);
        assert!(label_overrides(&old).is_empty());
    }

    /// A state written onto cover or favourite comes back through the table's
    /// column groups as a header that sorts by nothing. Similar keeps
    /// cycling, which is why the gate is `sortable`, not `sort_key`.
    #[test]
    fn a_sort_leaves_the_show_only_columns_alone() {
        let layout: Vec<ColumnSpec> = ["title", "cover", "favourite", "similar"]
            .iter()
            .map(|key| ColumnSpec {
                key: key.to_string(),
                width: 64.,
                label: None,
            })
            .collect();
        let mut columns = track_columns(&layout, &None, &HashMap::new());
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

        let title = ix(&columns, "title");
        mirror_sort(&mut columns, title, ColumnSort::Ascending);
        assert!(of(&columns, "title") == Some(ColumnSort::Ascending));
        assert!(of(&columns, "similar") == Some(ColumnSort::Default));
        assert!(of(&columns, "cover").is_none());
        assert!(of(&columns, "favourite").is_none());
    }

    #[test]
    fn a_restored_similar_column_sorts() {
        let layout: Vec<ColumnSpec> = ["title", "cover", "favourite", "similar"]
            .iter()
            .map(|key| ColumnSpec {
                key: key.to_string(),
                width: 64.,
                label: None,
            })
            .collect();
        let sort = Some((SharedString::from("similar"), true));
        let columns = track_columns(&layout, &sort, &HashMap::new());
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

    #[test]
    fn grouped_search_config_defaults_on_and_round_trips_off() {
        let old: LibraryConfig = serde_json::from_str("{}").unwrap();
        assert!(old.group_search_results);

        let off: LibraryConfig =
            serde_json::from_str(r#"{"group_search_results": false}"#).unwrap();
        assert!(!off.group_search_results);
        let saved = serde_json::to_value(&off).unwrap();
        let restored: LibraryConfig = serde_json::from_value(saved).unwrap();
        assert!(!restored.group_search_results);
    }

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

    #[test]
    fn legacy_density_folds_into_heights() {
        let config: LibraryConfig = serde_json::from_str(r#"{"density": "comfortable"}"#).unwrap();
        assert!(super::fold_row_heights(&config) == (40., 40.));

        let config: LibraryConfig = serde_json::from_str("{}").unwrap();
        assert!(super::fold_row_heights(&config) == (30., 30.));
    }

    /// Clamped to the band the readout's input allows, not the strip's own
    /// top, so a typed height persists across the reload.
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

    #[test]
    fn the_sort_columns_are_offered_and_sortable() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        let offered: Vec<&str> = super::offered().map(|def| def.key).collect();
        for (base, sort) in [
            ("title", "title_sort"),
            ("artist", "artist_sort"),
            ("album_artist", "album_artist_sort"),
            ("album", "album_sort"),
        ] {
            let base_ix = offered
                .iter()
                .position(|key| *key == base)
                .unwrap_or_else(|| panic!("{base} should be offered"));
            let sort_ix = offered
                .iter()
                .position(|key| *key == sort)
                .unwrap_or_else(|| panic!("{sort} should be offered"));
            assert!(
                sort_ix == base_ix + 1,
                "{sort} should follow {base} in the picker"
            );
            assert!(
                super::sortable(sort),
                "{sort} should offer a sort of its own"
            );
            assert!(
                super::sort_key(sort) != super::sort_key(base),
                "{sort} should order on its own key, not {base}'s, which falls \
                 back to the display name and interleaves the untagged rows"
            );
            assert!(
                !super::column_def(sort).expect("registered").default_on,
                "{sort} is a diagnostic column, off by default"
            );
        }
        rox_i18n::set_locale(None);
    }

    /// A CJK name with a Latin sort tag sorts under its sort name, so a rail
    /// reading the display name would print it after Z.
    #[test]
    fn a_rail_off_the_sort_key_stays_monotonic() {
        let table = SymTable {
            strings: vec![
                "Adele".to_string(),
                "米津玄師".to_string(),
                "ZZ Top".to_string(),
            ],
            lower: vec![
                "adele".to_string(),
                "米津玄師".to_string(),
                "zz top".to_string(),
            ],
            sort: vec![String::new(), "Yonezu, Kenshi".to_string(), String::new()],
            sort_lower: vec![String::new(), "yonezu, kenshi".to_string(), String::new()],
        };
        let mut order: Vec<usize> = (0..table.strings.len()).collect();
        order.sort_by(|&a, &b| table.sort_key(a).cmp(table.sort_key(b)));

        let mut letters: Vec<String> = Vec::new();
        for &sym in &order {
            let letter = letter_initial(table.sort_key(sym));
            if letters.last().map(String::as_str) != Some(letter.as_str()) {
                letters.push(letter);
            }
        }
        assert!(
            letters == vec!["A".to_string(), "Y".to_string(), "Z".to_string()],
            "the rail should read {letters:?} as A, Y, Z"
        );
        let mut sorted = letters.clone();
        sorted.sort();
        assert!(sorted == letters, "the rail's letters have to climb");
    }
}
