//! The metadata panel: the current track's tags laid out as a sheet, with
//! title and artist up top, then the labeled fields the library has
//! (album, genre, year, duration, codec, bitrate, and a sort name beside
//! each name that carries one). What it describes is per-view config
//! through [`MetadataSource`]: the playing track, the selected one, or
//! the library as a whole, where the sheet zooms out to the catalog's
//! counts, so a duplicate can watch each. The background
//! can show the track's cover art, cropped to fill and dimmed under a
//! scrim so the fields keep reading; art comes off the file on a
//! background thread like the cover panel's and is retired the same way
//! when the track moves on.
//!
//! The sheet has an edit face, the pencil in the title row: the tag
//! fields become inputs over a baseline read off the file itself, and a
//! save commits only the fields that moved against it, through the
//! writer's atomic layer. A successful commit is written to the catalog too,
//! so the library shows the edit without a rescan.
//!
//! The tag values click through to the app-wide search, so the sheet
//! doubles as a way into the rest of the library. Artist, album artist,
//! album, genre, and year go through the shared filter, the filter
//! panel's path: the pick shows as a chip beside the search box, whatever
//! is typed there keeps narrowing alongside it, and a second click drops
//! it again. The title has no filter column, so it goes into the query text
//! as a `title:"value"` term instead, appended and removed the same way.
//! A genre list splits, so a click takes the one value it hit. The
//! hit areas only show while a search panel is up somewhere to display
//! what a click writes.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use gpui::{
    div, img, prelude::*, px, AnyElement, App, ClipboardItem, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, Image, ImageFormat, KeyDownEvent, MouseButton, MouseDownEvent,
    ObjectFit, SharedString, Stateful, Subscription, WeakEntity, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side, Sizable};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::projection::FilterField;
use rox_library::writer::{self, Change, Field};
use rox_romanize::{Japanese, Reading};
use serde::{Deserialize, Serialize};
use std::rc::Rc;

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{
    self, align_row, justify, justify_v, valign_row, Align, AppState, PanelChrome, PanelSettings,
    VAlign,
};
use crate::panel_settings;
use crate::player::fmt_time;
use crate::providers;
use crate::query::shared_query::{self, SharedQuery};
use crate::selection::SelectionEvent;
use crate::settings::ui as settings_ui;
use crate::source::{ResolvedTrack, TrackSource};
use crate::track_ui::track_columns::{self, Column};

/// The metadata panel's per-view config: what a saved layout restores, and
/// what the settings window edits. Missing fields take the defaults, so a
/// layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetadataConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub source: MetadataSource,
    pub align: Align,
    /// Where the content goes down the panel when there's height to
    /// spare. The sheet has always centered, so that stays the default;
    /// the table face follows the knob too, and pins to the top with it.
    pub valign: VAlign,
    /// The track's cover art behind the fields, dimmed under a scrim.
    pub cover: bool,
    /// How the fields lay out; see [`MetadataDisplay`].
    pub display: MetadataDisplay,
    /// Tint every other row of the table face; the sheet never stripes.
    pub stripes: bool,
    /// Draw the hairline under each table row. Off by default: the table
    /// face has always drawn bare, the stripes alone carrying the rhythm.
    pub row_borders: bool,
    /// The shown field keys out of [`fields`]; the registry's default-on
    /// set for a fresh panel. Title and artist head the sheet and are not
    /// listed.
    pub fields: Vec<String>,
}

impl Default for MetadataConfig {
    fn default() -> Self {
        MetadataConfig {
            chrome: PanelChrome::default(),
            source: MetadataSource::default(),
            align: Align::default(),
            valign: VAlign::default(),
            cover: true,
            display: MetadataDisplay::default(),
            stripes: true,
            row_borders: false,
            fields: track_columns::default_columns(&fields()),
        }
    }
}

/// How the fields lay out: the title-led sheet, or a flat label and
/// value table from the top, the classic file-info pane. The table
/// folds the title and artist in as rows of their own.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetadataDisplay {
    #[default]
    Sheet,
    Table,
}

/// What the sheet describes: the playing track, the selected one, or the
/// library as a whole, the same fields idea zoomed out to the catalog.
/// The track sides spell the same as [`TrackSource`], so a layout saved
/// before the library scope existed reads unchanged.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetadataSource {
    #[default]
    Playing,
    Selected,
    Library,
}

impl MetadataSource {
    /// The track-scoped side, None for the library scope.
    fn track(self) -> Option<TrackSource> {
        match self {
            MetadataSource::Playing => Some(TrackSource::Playing),
            MetadataSource::Selected => Some(TrackSource::Selected),
            MetadataSource::Library => None,
        }
    }
}

/// The sheet's toggleable fields in display order, the library-column
/// registry shape so the shared checklist and Fields submenu drive them.
/// The file facts are off by default; the tag sheet is the stock face.
///
/// The four sort names lead, mirroring the title-over-artist head the
/// sheet puts above this list, and each other one sits under the field
/// it sorts. They're on by default and cost a Latin-only library
/// nothing, since a row whose value is empty is skipped like any other.
///
/// `track_columns::checklist`/`columns_submenu` want a `'static` slice, so
/// this rebuilds and leaks once per active locale rather than on every
/// call, mirroring `rox_i18n::t_static`'s own per-locale cache.
fn fields() -> Vec<Column> {
    vec![
        Column {
            key: "title_sort",
            label: rox_i18n::t!("metadata-field-title-sort"),
            default_on: true,
        },
        Column {
            key: "artist_sort",
            label: rox_i18n::t!("metadata-field-artist-sort"),
            default_on: true,
        },
        Column {
            key: "album",
            label: rox_i18n::t!("head-piece-album"),
            default_on: true,
        },
        Column {
            key: "album_sort",
            label: rox_i18n::t!("metadata-field-album-sort"),
            default_on: true,
        },
        Column {
            key: "album_artist",
            label: rox_i18n::t!("filter-field-album-artist"),
            default_on: true,
        },
        Column {
            key: "album_artist_sort",
            label: rox_i18n::t!("metadata-field-album-artist-sort"),
            default_on: true,
        },
        Column {
            key: "disc",
            label: rox_i18n::t!("metadata-field-disc"),
            default_on: true,
        },
        Column {
            key: "track",
            label: rox_i18n::t!("metadata-field-track"),
            default_on: true,
        },
        Column {
            key: "genre",
            label: rox_i18n::t!("head-piece-genre"),
            default_on: true,
        },
        Column {
            key: "year",
            label: rox_i18n::t!("head-piece-year"),
            default_on: true,
        },
        Column {
            key: "duration",
            label: rox_i18n::t!("info-item-duration"),
            default_on: true,
        },
        Column {
            key: "bpm",
            label: rox_i18n::t!("columns-bpm"),
            default_on: true,
        },
        Column {
            key: "codec",
            label: rox_i18n::t!("metadata-field-codec"),
            default_on: true,
        },
        Column {
            key: "bitrate",
            label: rox_i18n::t!("metadata-field-bitrate"),
            default_on: true,
        },
        Column {
            key: "sample_rate",
            label: rox_i18n::t!("metadata-field-sample-rate"),
            default_on: false,
        },
        Column {
            key: "bit_depth",
            label: rox_i18n::t!("metadata-field-bit-depth"),
            default_on: false,
        },
        Column {
            key: "gain_track",
            label: rox_i18n::t!("metadata-field-gain-track"),
            default_on: false,
        },
        Column {
            key: "gain_album",
            label: rox_i18n::t!("metadata-field-gain-album"),
            default_on: false,
        },
        Column {
            key: "file",
            label: rox_i18n::t!("metadata-field-file"),
            default_on: false,
        },
        Column {
            key: "plays",
            label: rox_i18n::t!("status-item-plays"),
            default_on: false,
        },
        Column {
            key: "rating",
            label: rox_i18n::t!("info-item-rating"),
            default_on: false,
        },
        Column {
            key: "added",
            label: rox_i18n::t!("columns-scanned"),
            default_on: false,
        },
    ]
}

/// The fields a picker should offer: the registry minus BPM while tempo
/// analysis is off, the way the library's column picker hides it. Only
/// discovery is gated: a layout already holding the field keeps showing
/// whatever tempo the tags brought in.
fn offered() -> Vec<Column> {
    let tempo = crate::settings::tempo_analysis();
    fields()
        .into_iter()
        .filter(|col| tempo || col.key != "bpm")
        .collect()
}

/// A ReplayGain figure with its sign forced, "+1.25 dB", so a positive
/// gain reads as one rather than as a bare number. The locale formatter
/// has no sign flag, so it's glued on by hand.
fn fmt_gain(db: f32) -> String {
    let sign = if db.is_sign_negative() { "-" } else { "+" };
    let magnitude = rox_i18n::format::format_float(f64::from(db.abs()), 2);
    format!("{sign}{magnitude} dB")
}

/// What a click on a value does to the shared query: pin the exact value
/// on the structured filter, the filter panel's own path, or add the
/// `title:"value"` term to the text for the title, which the filter keeps
/// no column for. Either way it adds to what's already narrowed
/// instead of replacing it, and clicking the same value again takes it
/// back off.
#[derive(Clone, Copy)]
enum Search {
    Pick(FilterField),
    Term(&'static str),
}

/// How a shown value searches when it's clicked, keyed by [`fields`] plus
/// the sheet's two head rows. The rest of the sheet describes the file
/// rather than tagging it, and neither the filter nor the query syntax
/// covers those, so duration, codec, bitrate, plays, rating, and the file
/// name stay inert text.
fn query_field(key: &str) -> Option<Search> {
    match key {
        "title" => Some(Search::Term("title")),
        "artist" => Some(Search::Pick(FilterField::Artist)),
        "album_artist" => Some(Search::Pick(FilterField::AlbumArtist)),
        "album" => Some(Search::Pick(FilterField::Album)),
        "genre" => Some(Search::Pick(FilterField::Genre)),
        "year" => Some(Search::Pick(FilterField::Year)),
        _ => None,
    }
}

/// The library scope's readouts: the catalog boiled down to the counts a
/// collection sheet lists. Cached and rebuilt when the catalog or the
/// listen record moves, never per frame.
struct LibraryTotals {
    tracks: usize,
    albums: usize,
    artists: usize,
    genres: usize,
    total_ms: u64,
    plays: u64,
}

/// The shown track's full projection row, owned so it outlives the borrow
/// of the library.
#[derive(Clone)]
struct Details {
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    /// The sort names, empty when the file carries none. Off the
    /// projection like the rest: the interned ones ride their symbol
    /// tables, the sort title is per row.
    title_sort: String,
    artist_sort: String,
    album_artist_sort: String,
    album_sort: String,
    genre: String,
    year: u16,
    disc_no: u16,
    track_no: u16,
    duration_ms: u32,
    codec: String,
    bitrate_kbps: u16,
    sample_rate_hz: u32,
    bit_depth: u8,
    plays: u32,
    rating: u8,
    /// Beats a minute, None where nothing has filled a tempo.
    bpm: Option<f32>,
    /// Whether that tempo is rox's own estimate rather than a tag.
    bpm_measured: bool,
    /// The file's ReplayGain figures in dB, None where it carries none.
    track_gain_db: Option<f32>,
    album_gain_db: Option<f32>,
    /// When the scanner took the track in, as unix seconds, 0 when the
    /// library predates the timestamp.
    added: i64,
}

/// The editable fields in sheet order, each with its input row's label:
/// the tags the panel shows plus the comment, which is only stored in
/// the file. Each sort name sits under the field it sorts, so a
/// romanization is typed next to the name it stands in for. Duration,
/// codec, and bitrate stay display-only, they describe the stream. A
/// plain function rather than a `const`: `t_static` isn't
/// const-evaluable, and nothing outside this file needs the slice itself
/// to be `'static`, so it just rebuilds (cheaply, `t_static` caches the
/// strings) on each call.
fn edit_fields() -> Vec<(Field, gpui::SharedString)> {
    vec![
        (Field::Title, rox_i18n::t!("info-item-title")),
        (Field::TitleSort, rox_i18n::t!("metadata-field-title-sort")),
        (Field::Artist, rox_i18n::t!("head-piece-artist")),
        (
            Field::ArtistSort,
            rox_i18n::t!("metadata-field-artist-sort"),
        ),
        (Field::Album, rox_i18n::t!("head-piece-album")),
        (Field::AlbumSort, rox_i18n::t!("metadata-field-album-sort")),
        (
            Field::AlbumArtist,
            rox_i18n::t!("filter-field-album-artist"),
        ),
        (
            Field::AlbumArtistSort,
            rox_i18n::t!("metadata-field-album-artist-sort"),
        ),
        (Field::DiscNo, rox_i18n::t!("metadata-field-disc")),
        (Field::TrackNo, rox_i18n::t!("metadata-field-track")),
        (Field::Genre, rox_i18n::t!("head-piece-genre")),
        (Field::Year, rox_i18n::t!("head-piece-year")),
        (Field::Comment, rox_i18n::t!("metadata-field-comment")),
    ]
}

/// What Romanize does to one sort input, decided per field by
/// [`sort_fill`].
#[derive(PartialEq, Debug)]
enum Fill {
    /// Leave the input alone: the user typed something into it, or the
    /// name it sorts already files where a person would look for it.
    Leave,
    /// Put this in the input. Either the sort name the library already
    /// holds, or a fresh reading of the name.
    Value(String),
    /// The name is kanji and no dictionary is installed, so there's no
    /// honest answer. The sheet says so under the rows.
    NeedsDictionary,
}

/// What Romanize should put in one sort input, given the name it sorts
/// (`base`), what's typed in the input now, and the sort name the
/// library already holds for that name (`stored`).
///
/// Three rules, in order. A typed value is never touched: the whole
/// point of the sheet is that a person can overrule any of this, and a
/// button that overwrites what they wrote is a button they stop
/// pressing. A name that already reads in Latin letters gets nothing at
/// all, whatever the library holds for it, because this button says
/// Romanize and "Beatles, The" is not a romanization. What's left is a
/// name that needs one, and there the sort name the library already
/// holds wins over a fresh reading: it's MusicBrainz's or the user's
/// answer where the reading is IPADIC's guess.
fn sort_fill(
    base: &str,
    current: &str,
    stored: &str,
    reading: Reading,
    ja: Option<&Japanese>,
) -> Fill {
    if !current.trim().is_empty() {
        return Fill::Leave;
    }
    let read = rox_romanize::romanize_as(base, ja, reading);
    // Either there's a reading, or there would be one with the
    // dictionary installed. Anything else is a name this can't improve:
    // Latin already, or a script the crate doesn't read.
    let readable = read.is_some() || rox_romanize::needs_dictionary(base, reading);
    if !readable {
        return Fill::Leave;
    }
    if !stored.trim().is_empty() {
        return Fill::Value(stored.to_string());
    }
    match read {
        Some(read) => Fill::Value(read),
        // The one refusal worth telling the user about. A dictionary
        // that's loaded and still can't read the name is a different
        // problem, and pointing at the download wouldn't fix it.
        None if ja.is_none() => Fill::NeedsDictionary,
        None => Fill::Leave,
    }
}

/// The four sort fields Romanize fills, each with the field it sorts and
/// the sort name the projection holds for that value, empty where it
/// holds none.
///
/// The stored side is what the fill pass and the romanize pass wrote into
/// the library's own tables, which is exactly the answer this button is
/// for: the sheet shows it, and Save is what gets it into the file.
fn sort_targets(details: Option<&Details>) -> [(Field, Field, String); 4] {
    let stored =
        |pick: fn(&Details) -> &str| details.map(|d| pick(d).to_string()).unwrap_or_default();
    [
        (Field::TitleSort, Field::Title, stored(|d| &d.title_sort)),
        (Field::ArtistSort, Field::Artist, stored(|d| &d.artist_sort)),
        (Field::AlbumSort, Field::Album, stored(|d| &d.album_sort)),
        (
            Field::AlbumArtistSort,
            Field::AlbumArtist,
            stored(|d| &d.album_artist_sort),
        ),
    ]
}

/// The changes a save writes: one per [`edit_fields`] entry whose input
/// drifted from the baseline the writer read off the file, and nothing
/// for the rest, so an untouched tag never rewrites. A field the file
/// never carried reads as empty, which is what keeps a blank input
/// quiet; emptying one it does carry drops the tag.
fn diff_baseline(values: &[String], baseline: &[(Field, String)]) -> Vec<Change> {
    edit_fields()
        .iter()
        .zip(values)
        .filter_map(|((field, _), value)| {
            let original = baseline
                .iter()
                .find(|(f, _)| f == field)
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            if value == original {
                return None;
            }
            Some(Change {
                field: field.clone(),
                value: (!value.is_empty()).then(|| value.clone()),
            })
        })
        .collect()
}

/// One in-progress edit: the pinned track, the baseline read off its
/// file, and one input per entry of [`edit_fields`]. Lives only while
/// edit mode is on.
struct EditState {
    key: TrackKey,
    /// The named fields as the writer read them, what save diffs
    /// against; None until the read finishes (or never, on a file the
    /// writer can't parse), and save stays inert without it.
    baseline: Option<Vec<(Field, String)>>,
    inputs: Vec<Entity<InputState>>,
    /// A failed read or commit, shown inline over the buttons.
    error: Option<SharedString>,
    /// What Romanize couldn't answer, a muted line under the rows. Set
    /// when a name needed the Japanese dictionary and none is installed.
    note: Option<SharedString>,
    /// Run Romanize as soon as the baseline read lands. The menu row
    /// opens the sheet and fills it in one click, and the read is what
    /// puts the file's own sort names in the way of the fill.
    romanize_on_open: bool,
    /// A commit is in flight; the buttons hold still until it finishes.
    saving: bool,
    _input_events: Vec<Subscription>,
}

pub struct MetadataPanel {
    state: AppState,
    config: MetadataConfig,
    /// The in-progress edit while the sheet shows its edit face.
    edit: Option<EditState>,
    /// The row a right press last landed on, as its label and the value
    /// it shows, which is what the context menu's Copy entry writes. A
    /// press anywhere but a row clears it, so the menu falls back to the
    /// panel's own entries.
    menu_field: Option<(SharedString, String)>,
    /// The shown path's row, or None inside for a file the library does
    /// not know. Cached because the pump notifies every frame and the row
    /// lookup scans the projection; cleared when the catalog changes.
    details: Option<(TrackKey, Option<Details>)>,
    /// The library scope's cached counts; cleared when the catalog or the
    /// listen record moves.
    totals: Option<LibraryTotals>,
    /// The loaded background art keyed by the track it belongs to, with the
    /// pending marker, generation guard, and swap/drop retires the shared
    /// loader provides.
    art: panel::TrackedImage,
    /// The cached source resolve, so the pump's per-frame notifies never
    /// turn into selection lookups.
    resolved: ResolvedTrack,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
    /// Retires the shown background art when the panel is dropped (closed or
    /// its pop-out window shut), so nothing stays pinned in gpui's
    /// never-evicting asset cache.
    _retire_on_drop: Subscription,
}

impl MetadataPanel {
    pub fn new(state: AppState, config: MetadataConfig, cx: &mut Context<Self>) -> Self {
        // The tags and details turn over with the track, not as it plays,
        // so the gated observe skips the pump's per-tick repaints.
        let _player_changed = crate::player::observe_view(&state.player, cx);
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
        // A rescan can rewrite tags, art files, and id -> path mappings;
        // drop the resolve and the row so they re-read, and send the cover
        // background back through the file behind the one it's showing.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                // A rating click or a new listen moves two of the sheet's
                // fields, and the listen moves the library scope's play
                // total too; re-resolve those, nothing else changed.
                if matches!(event, LibraryEvent::Rated | LibraryEvent::Played) {
                    this.details = None;
                    this.totals = None;
                    cx.notify();
                    return;
                }
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.resolved.invalidate();
                this.details = None;
                this.totals = None;
                this.art.refresh();
                cx.notify();
            },
        );
        let _retire_on_drop = cx.on_release(|this, cx| this.art.invalidate(cx));
        MetadataPanel {
            state,
            config,
            edit: None,
            menu_field: None,
            details: None,
            totals: None,
            art: panel::TrackedImage::default(),
            resolved: ResolvedTrack::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
            _retire_on_drop,
        }
    }

    /// The track the panel describes, through the source's track side;
    /// the library scope names no track, which folds the pencil and the
    /// online lookup away there.
    fn resolved_track(&mut self, cx: &App) -> Option<TrackKey> {
        let source = self.config.source.track()?;
        self.resolved.get(source, &self.state, cx)
    }

    /// The shown track's row, from the cache or one projection scan on a
    /// miss. None for a track the library does not know or while the
    /// projection is still loading.
    fn details_for(&mut self, key: &TrackKey, cx: &App) -> Option<&Details> {
        if self.details.as_ref().map(|(k, _)| k) != Some(key) {
            let library = self.state.library.read(cx);
            let details = library.id_for_key(key).and_then(|id| {
                let projection = library.projection()?;
                // The live row for the id: an update tombstones the old
                // row and appends the new one, so both can carry the id
                // and the dead one comes first.
                let row = (0..projection.len() as u32).find(|&row| {
                    projection.db_id[row as usize] == id && !projection.is_dead(row)
                })?;
                let v = projection.resolve(row);
                Some(Details {
                    title: v.title.to_owned(),
                    artist: v.artist.to_owned(),
                    album_artist: v.album_artist.to_owned(),
                    album: v.album.to_owned(),
                    title_sort: v.title_sort.to_owned(),
                    artist_sort: v.artist_sort.to_owned(),
                    album_artist_sort: v.album_artist_sort.to_owned(),
                    album_sort: v.album_sort.to_owned(),
                    genre: v.genre.to_owned(),
                    year: v.year,
                    disc_no: v.disc_no,
                    track_no: v.track_no,
                    duration_ms: v.duration_ms,
                    codec: v.codec.to_owned(),
                    bitrate_kbps: v.bitrate_kbps,
                    sample_rate_hz: v.sample_rate_hz,
                    bit_depth: v.bit_depth,
                    plays: v.plays,
                    rating: v.rating,
                    bpm: v.bpm,
                    bpm_measured: matches!(v.bpm_source, rox_library::tempo::Source::Measured),
                    track_gain_db: v.track_gain_db,
                    album_gain_db: v.album_gain_db,
                    added: v.added,
                })
            });
            self.details = Some((key.clone(), details));
        }
        self.details
            .as_ref()
            .and_then(|(_, details)| details.as_ref())
    }

    /// The library scope's counts, from the cache or one projection scan
    /// on a miss: the whole catalog's tracks, albums, artists, genres,
    /// running time, and play total. Albums key on the (album artist,
    /// album) pair the library groups by; artists are the distinct album
    /// artists, matching the artist grid; genres split compound tags the
    /// way the genre grid does, so the counts agree across the app.
    fn library_totals(&mut self, cx: &App) -> Option<&LibraryTotals> {
        if self.totals.is_none() {
            let library = self.state.library.read(cx);
            let projection = library.projection()?;
            let mut total_ms = 0u64;
            let mut plays = 0u64;
            let mut albums: HashSet<(u32, u32)> = HashSet::new();
            let mut artists: HashSet<u32> = HashSet::new();
            let mut genre_syms: HashSet<u32> = HashSet::new();
            for ix in 0..projection.len() {
                if projection.is_dead(ix as u32) {
                    continue;
                }
                total_ms += u64::from(projection.duration_ms[ix]);
                plays += u64::from(projection.plays[ix].load(Ordering::Relaxed));
                albums.insert((projection.album_artist[ix], projection.album[ix]));
                artists.insert(projection.album_artist[ix]);
                genre_syms.insert(projection.genre[ix]);
            }
            // The distinct syms first, then the strings split once each:
            // a compound tag names every genre in it, the grid's read.
            let mut genres: HashSet<String> = HashSet::new();
            for sym in genre_syms {
                for genre in rox_library::genre::split(&projection.genres.strings[sym as usize]) {
                    genres.insert(genre.to_string());
                }
            }
            genres.remove("");
            self.totals = Some(LibraryTotals {
                tracks: projection.live_len(),
                albums: albums.len(),
                artists: artists.len(),
                genres: genres.len(),
                total_ms,
                plays,
            });
        }
        self.totals.as_ref()
    }

    /// Make sure the background art for `path` is cached or on its way:
    /// read the file off the UI thread through the shared loader, which
    /// swaps the result in and retires the previous decode.
    fn ensure_art(&mut self, path: &Path, cx: &mut Context<Self>) {
        let read = path.to_path_buf();
        self.art.ensure(
            path,
            |this: &mut Self| &mut this.art,
            move || {
                rox_library::art::cover_art(&read).and_then(|(bytes, mime)| {
                    let format = ImageFormat::from_mime_type(&mime)?;
                    Some(Arc::new(Image::from_bytes(format, bytes)))
                })
            },
            cx,
        );
    }

    /// The panel's own dropdown entries: the source pick and the cover
    /// background toggle, the same knobs the customize window edits. The
    /// source flyout is the panel's own rather than the shared track
    /// pair, because this sheet can also describe the library itself.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            // The flyout follows the panel so the picked row's tick swaps
            // live instead of going stale until the menu is reopened.
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for (label, icon, source) in [
                (
                    rox_i18n::t!("source-follow-playing"),
                    icons::PLAY,
                    MetadataSource::Playing,
                ),
                (
                    rox_i18n::t!("source-follow-selection"),
                    icons::LIST_MUSIC,
                    MetadataSource::Selected,
                ),
                (
                    rox_i18n::t!("panel-title-library"),
                    icons::DATABASE,
                    MetadataSource::Library,
                ),
            ] {
                submenu = submenu.item(panel::check_row(
                    label,
                    Some(icon),
                    move |this: &Self| this.config.source == source,
                    move |this, cx| {
                        this.config.source = source;
                        cx.notify();
                    },
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("metadata-source"),
            submenu,
        ));
        let submenu = track_columns::columns_submenu(offered(), window, cx);
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("metadata-fields"),
            submenu,
        ));
        let weak = cx.entity().downgrade();
        menu.separator().item(
            PopupMenuItem::new(rox_i18n::t!("metadata-cover-background"))
                .checked(self.config.cover)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.cover = !this.config.cover;
                        cx.notify();
                    });
                }),
        )
    }

    /// The title-row pencil: into edit mode, or back out of it.
    fn toggle_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            self.close_edit(cx);
        } else {
            self.start_edit(window, cx);
        }
    }

    /// Open edit mode on the shown track: one input per field, filled
    /// once the writer's read finishes off the UI thread. The path pins
    /// here, so a Playing source that moves on mid-edit doesn't steal
    /// the form.
    fn start_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            return;
        }
        let Some(key) = self.resolved_track(cx) else {
            return;
        };
        // IPADIC is forty megabytes of mapped tables and the first caller
        // in the session pays for the open. Romanize lives one click away
        // from here, so the open happens off the UI thread while the sheet
        // is being filled in. The load is cached and idempotent, so a
        // second sheet costs a lock.
        cx.background_executor()
            .spawn(async {
                rox_romanize::japanese();
            })
            .detach();
        let inputs: Vec<Entity<InputState>> = edit_fields()
            .iter()
            .map(|_| cx.new(|cx| InputState::new(window, cx)))
            .collect();
        // Enter in any input saves; Escape is handled by the sheet's wrapper.
        let _input_events = inputs
            .iter()
            .map(|input| {
                cx.subscribe(input, |this: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save_edit(cx);
                    }
                })
            })
            .collect();
        window.focus(&inputs[0].read(cx).focus_handle(cx));
        self.edit = Some(EditState {
            key: key.clone(),
            baseline: None,
            inputs,
            error: None,
            note: None,
            romanize_on_open: false,
            saving: false,
            _input_events,
        });
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn({
                    let path = key.path.clone();
                    async move { writer::read(&path) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                let Some(edit) = &mut this.edit else { return };
                if edit.key != key {
                    return;
                }
                match read {
                    Ok(fields) => {
                        for ((field, _), input) in edit_fields().iter().zip(&edit.inputs) {
                            // Multi-value tags show their first item, the
                            // same one the writer's verify reads back.
                            let value = fields
                                .iter()
                                .find(|(f, _)| f == field)
                                .map(|(_, v)| v.clone())
                                .unwrap_or_default();
                            input.update(cx, |input, cx| input.set_value(value, window, cx));
                        }
                        edit.baseline = Some(fields);
                    }
                    Err(e) => edit.error = Some(e.into()),
                }
                // The menu's row opens the sheet and fills it; the fill
                // waits for this read, which would otherwise land on top
                // of it with the file's own (empty) sort names.
                if this.edit.as_ref().is_some_and(|edit| edit.romanize_on_open) {
                    this.fill_romanize(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Drop the edit face without writing anything.
    fn close_edit(&mut self, cx: &mut Context<Self>) {
        self.edit = None;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    /// Commit the fields that moved against the baseline, through the
    /// writer's atomic layer off the UI thread. Nothing moved closes the
    /// form; a failed commit keeps it open with the error inline, the
    /// file untouched. Success hands the changes to the catalog, so the
    /// projection follows without a rescan.
    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = &mut self.edit else { return };
        // No baseline means nothing safe to diff against: the read is
        // still running, or the file defeated it.
        let (Some(baseline), false) = (&edit.baseline, edit.saving) else {
            return;
        };
        let values: Vec<String> = edit
            .inputs
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect();
        let changes = diff_baseline(&values, baseline);
        if changes.is_empty() {
            self.close_edit(cx);
            return;
        }
        edit.saving = true;
        edit.error = None;
        let key = edit.key.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let key = key.clone();
                    let changes = changes.clone();
                    // Through the key: a cue track's edit stays in the
                    // library, since the image on disk belongs to the whole
                    // disc and writing a title there would title all of it.
                    async move { writer::commit_key(&key.path, key.sub, &changes, &[]) }
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if this.edit.as_ref().is_some_and(|edit| edit.key == key) {
                            this.edit = None;
                            panel::refresh_tab_panel(&this.tab_panel, cx);
                        }
                        this.state
                            .library
                            .update(cx, |library, cx| library.apply_edit(&key, &changes, cx));
                    }
                    Err(e) => {
                        if let Some(edit) = &mut this.edit {
                            if edit.key == key {
                                edit.saving = false;
                                edit.error = Some(e.into());
                            }
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Romanize, from the panel's own menu: open the sheet if it's closed,
/// then fill it. Nothing here writes the file; the sort names land in
/// the inputs and Save is what commits them, which is the confirmed
/// write step every enrichment path in the app goes through.
impl MetadataPanel {
    fn romanize_sort_names(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &mut self.edit {
            // Already open, and its baseline read is either done or
            // about to land on top of the fill; flag it either way and
            // fill now if the read is already in.
            Some(edit) => {
                if edit.baseline.is_none() {
                    edit.romanize_on_open = true;
                    return;
                }
                self.fill_romanize(window, cx);
            }
            None => {
                self.start_edit(window, cx);
                if let Some(edit) = &mut self.edit {
                    edit.romanize_on_open = true;
                }
            }
        }
    }

    /// Fill the empty sort inputs with a Latin reading of the name each
    /// one sorts, leaving every typed value alone. [`sort_fill`] makes
    /// the call per field; this is where the values it needs come from.
    ///
    /// The reading hint comes off the whole row rather than the one
    /// field: bare kanji is the same characters in Japanese and Chinese,
    /// and kana anywhere in the track's names is the one signal that says
    /// which. That's the same read the library pass makes.
    fn fill_romanize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.edit.as_ref().map(|edit| edit.key.clone()) else {
            return;
        };
        let details = self.details_for(&key, cx).cloned();
        let fields = edit_fields();
        let Some(edit) = &self.edit else { return };
        let values: Vec<String> = edit
            .inputs
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect();
        let index = |field: &Field| fields.iter().position(|(f, _)| f == field);
        // The name a sort field sorts, as the sheet has it now: what's
        // typed, or what the library holds when the file carried no tag.
        let name = |field: &Field, from_details: fn(&Details) -> &str| -> String {
            let typed = index(field)
                .map(|ix| values[ix].trim().to_string())
                .unwrap_or_default();
            if !typed.is_empty() {
                return typed;
            }
            details
                .as_ref()
                .map(|d| from_details(d).to_string())
                .unwrap_or_default()
        };
        let title = name(&Field::Title, |d| &d.title);
        let artist = name(&Field::Artist, |d| &d.artist);
        let album = name(&Field::Album, |d| &d.album);
        let album_artist = name(&Field::AlbumArtist, |d| &d.album_artist);
        let reading = if [&title, &artist, &album, &album_artist]
            .iter()
            .any(|text| rox_romanize::has_kana(text))
        {
            Reading::Japanese
        } else {
            Reading::Auto
        };
        // What each sort input would take, gathered here and decided off
        // the UI thread: the reading walks IPADIC, and the first call in a
        // session opens it, which is dictionary-sized rather than
        // click-sized.
        let mut plan: Vec<(usize, String, String, String)> = Vec::new();
        for (sort, base, stored) in sort_targets(details.as_ref()) {
            let Some(sort_ix) = index(&sort) else {
                continue;
            };
            let base_value = match base {
                Field::Title => &title,
                Field::Artist => &artist,
                Field::Album => &album,
                _ => &album_artist,
            };
            plan.push((sort_ix, base_value.clone(), values[sort_ix].clone(), stored));
        }
        // The flag is spent the moment the reading starts, so the baseline
        // landing behind it doesn't queue a second pass over the same row.
        if let Some(edit) = &mut self.edit {
            edit.romanize_on_open = false;
        }
        cx.spawn_in(window, async move |this, cx| {
            let filled = cx
                .background_executor()
                .spawn(async move {
                    let ja = rox_romanize::japanese();
                    plan.into_iter()
                        .map(|(sort_ix, base, current, stored)| {
                            (sort_ix, sort_fill(&base, &current, &stored, reading, ja))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                let Some(edit) = &this.edit else { return };
                // The sheet moved to another track while the dictionary
                // opened, so this reading is about a row nobody is editing.
                if edit.key != key {
                    return;
                }
                let mut needs_dictionary = false;
                let mut writes: Vec<(Entity<InputState>, String)> = Vec::new();
                for (sort_ix, fill) in filled {
                    let Some(input) = edit.inputs.get(sort_ix) else {
                        continue;
                    };
                    match fill {
                        Fill::Leave => {}
                        // Typed while the reading ran, and a typed value is
                        // never overwritten.
                        Fill::Value(_) if !input.read(cx).value().trim().is_empty() => {}
                        Fill::Value(value) => writes.push((input.clone(), value)),
                        Fill::NeedsDictionary => needs_dictionary = true,
                    }
                }
                for (input, value) in writes {
                    input.update(cx, |input, cx| input.set_value(value, window, cx));
                }
                if let Some(edit) = &mut this.edit {
                    edit.note = needs_dictionary
                        .then(|| rox_i18n::t!("metadata-romanize-needs-dictionary"));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The shared column machinery drives the sheet's field set: the
/// settings checklist and the right-click Fields submenu both edit
/// through here. Turning a field on rebuilds the list in registry order,
/// so the sheet never shuffles with toggle order.
impl track_columns::ColumnHost for MetadataPanel {
    fn column_shown(&self, key: &str) -> bool {
        self.config.fields.iter().any(|k| k == key)
    }

    fn set_column(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>) {
        if on {
            let shown: Vec<&str> = self.config.fields.iter().map(String::as_str).collect();
            self.config.fields = fields()
                .iter()
                .filter(|c| c.key == key || shown.contains(&c.key))
                .map(|c| c.key.to_string())
                .collect();
        } else {
            self.config.fields.retain(|k| k != key);
        }
        cx.notify();
    }
}

impl PanelSettings for MetadataPanel {
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
        &[("Content", icons::FILE_TEXT)]
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
                rox_i18n::t!("metadata-source"),
                Some(rox_i18n::t!("metadata-source.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("source-playing"), MetadataSource::Playing),
                        (rox_i18n::t!("source-selected"), MetadataSource::Selected),
                        (rox_i18n::t!("panel-title-library"), MetadataSource::Library),
                    ],
                    self.config.source,
                    |this: &mut Self, source, cx| {
                        this.config.source = source;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(valign_row(
                self.config.valign,
                |this: &mut Self, valign, cx| {
                    this.config.valign = valign;
                    cx.notify();
                },
                cx,
            ))
            .child(panel::setting_row(
                rox_i18n::t!("metadata-display"),
                Some(rox_i18n::t!("metadata-display.description")),
                panel::choices_shared(
                    &[
                        (
                            rox_i18n::t!("metadata-display-sheet"),
                            MetadataDisplay::Sheet,
                        ),
                        (
                            rox_i18n::t!("metadata-display-table"),
                            MetadataDisplay::Table,
                        ),
                    ],
                    self.config.display,
                    |this: &mut Self, display, cx| {
                        this.config.display = display;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            // The horizontal knob only places the sheet; the table always
            // runs full width, so the row is hidden while that face shows.
            .when(self.config.display == MetadataDisplay::Sheet, |d| {
                d.child(align_row(
                    self.config.align,
                    |this: &mut Self, align, cx| {
                        this.config.align = align;
                        cx.notify();
                    },
                    cx,
                ))
            })
            .child(panel::setting_row(
                rox_i18n::t!("metadata-cover-background"),
                Some(rox_i18n::t!("metadata-cover-background.description")),
                panel::toggle(
                    self.config.cover,
                    |this: &mut Self, on, cx| {
                        this.config.cover = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_block(
                rox_i18n::t!("metadata-fields"),
                Some(rox_i18n::t!("metadata-fields.description")),
                None,
                track_columns::checklist(&offered(), self, cx),
            ))
            .into_any_element()
    }

    /// The table face's row look on the shared Appearance page, the
    /// library's Rows section for this panel. The sheet has no rows, so
    /// the section only shows with the table.
    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.config.display != MetadataDisplay::Table {
            return None;
        }
        Some(
            settings_ui::section(
                rox_i18n::t!("library-section-rows"),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        rox_i18n::t!("library-stripes"),
                        Some(rox_i18n::t!("metadata-stripes-description")),
                        panel::toggle(
                            self.config.stripes,
                            |this: &mut Self, on, cx| {
                                this.config.stripes = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(panel::setting_row(
                        rox_i18n::t!("library-row-borders"),
                        Some(rox_i18n::t!("metadata-row-borders-description")),
                        panel::toggle(
                            self.config.row_borders,
                            |this: &mut Self, on, cx| {
                                this.config.row_borders = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    )),
            )
            .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for MetadataPanel {}

impl Focusable for MetadataPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for MetadataPanel {
    fn panel_name(&self) -> &'static str {
        "metadata"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-metadata"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    /// The edit toggle shares the title bar row, the library's move.
    /// Hidden while the panel shows no track; lit while an edit is open.
    fn title_suffix(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let editing = self.edit.is_some();
        if !editing && self.resolved_track(cx).is_none() {
            return None;
        }
        let weak = cx.entity().downgrade();
        Some(
            settings_ui::icon_button(icons::PENCIL, false, move |_, window, cx| {
                let Some(this) = weak.upgrade() else { return };
                this.update(cx, |this, cx| this.toggle_edit(window, cx));
            })
            .when(editing, |d| d.bg(palette::bg_control_active())),
        )
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The sheet serves its own right click, so the tab panel's body
    /// menu stays out of the way: a press over a row offers to copy that
    /// row's value, with the panel's own entries after it.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    /// The layout dump stores the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The config block: the panel's quick entries and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, window, cx);
        // The online lookup, gated with the provider toggle so the menu
        // never offers a search that can't run. Opens the compare window;
        // the write waits for a confirmed, field-by-field pick.
        let menu = match (providers::metadata_online(), self.resolved_track(cx)) {
            (true, Some(key)) => {
                let library = self.state.library.clone();
                let now_art = self.state.now_art.clone();
                menu.separator().item(
                    PopupMenuItem::new(rox_i18n::t!("metadata-find-online"))
                        .icon(Icon::default().path(icons::DOWNLOAD))
                        .on_click(move |_, _, cx| {
                            rox_panel_api::openers::tags_matcher(
                                library.clone(),
                                now_art.clone(),
                                key.clone(),
                                cx,
                            );
                        }),
                )
            }
            _ => menu,
        };
        // Reading the names into Latin letters, the library pass's work
        // on the one track the sheet has pinned. It opens the edit face
        // and fills the empty sort inputs; the file is only touched when
        // Save is pressed, so this is a proposal like every other
        // enrichment path. No track, nothing to read.
        let menu = match self.resolved_track(cx) {
            Some(_) => {
                let weak = cx.entity().downgrade();
                // The online lookup above draws the separator when it's
                // there; with the provider off this row is the first of
                // the group and draws its own.
                let menu = if providers::metadata_online() {
                    menu
                } else {
                    menu.separator()
                };
                menu.item(
                    PopupMenuItem::new(rox_i18n::t!("metadata-romanize-sort-names"))
                        .icon(Icon::default().path(icons::GLOBE))
                        .on_click(move |_, window, cx| {
                            let Some(this) = weak.upgrade() else { return };
                            this.update(cx, |this, cx| this.romanize_sort_names(window, cx));
                        }),
                )
            }
            None => menu,
        };
        // Copy takes the track the panel showed when the menu opened; the
        // tags resolve at click time so a fresh tag write copies through.
        let menu = match self.resolved_track(cx) {
            Some(key) => {
                let library = self.state.library.clone();
                panel::copy_submenu(
                    menu,
                    window,
                    cx,
                    Rc::new(move |cx: &App| {
                        vec![panel::CopyText::from_key(&key, library.read(cx))]
                    }),
                )
            }
            None => menu,
        };
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, _window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config.clone())
                };
                MetadataPanel::new(state, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

/// The value side of a row: plain truncating text, or the same text as
/// hit areas that narrow the app-wide search. `query` holds the shared
/// query only while a search panel is up to show what a click writes;
/// without one every follower would narrow with nothing on screen saying
/// why, so the values render inert. `search` is [`query_field`]'s verdict,
/// and `next_id` counts up through the sheet so each hit area gets an
/// element id of its own.
///
/// The genre column is a "; " list, so it splits: a click picks the value
/// it hit rather than filtering on the whole list at once.
fn value_cell(
    value: &str,
    reading: &str,
    search: Option<Search>,
    query: Option<&Entity<SharedQuery>>,
    next_id: &mut usize,
) -> Div {
    let readings = crate::settings::show_readings();
    let (Some(search), Some(query)) = (search, query) else {
        return div()
            .min_w_0()
            .truncate()
            .child(panel::named(value, reading, readings));
    };
    let terms: Vec<String> = match search {
        Search::Pick(FilterField::Genre) => rox_library::genre::split(value)
            .map(str::to_string)
            .collect(),
        _ => vec![value.to_string()],
    };
    let mut row = div().flex().flex_row().min_w_0().overflow_hidden();
    for (ix, term) in terms.into_iter().enumerate() {
        if ix > 0 {
            row = row.child(
                div()
                    .flex_none()
                    .text_color(palette::text_muted())
                    .child("; "),
            );
        }
        let id = *next_id;
        *next_id += 1;
        let query = query.clone();
        let value = term.clone();
        row = row.child(
            div()
                .id(("metadata-value", id))
                .min_w_0()
                .truncate()
                .cursor_pointer()
                .hover(|d| d.text_color(palette::accent()))
                .on_click(move |_, _, cx| match search {
                    Search::Pick(field) => shared_query::toggle_pick(&query, field, &value, cx),
                    Search::Term(field) => shared_query::toggle_term(&query, field, &value, cx),
                })
                .child(panel::named(&term, reading, readings)),
        );
    }
    row
}

/// Arm one row for the copy menu: a right press over it records the
/// label and the value it shows, which the panel's context menu turns
/// into a "Copy Title" entry. The row records itself on the press and
/// the menu reads it back, the shape the history and queue panels use
/// for their track rows.
fn copy_target(
    row: Div,
    label: SharedString,
    value: String,
    panel: &WeakEntity<MetadataPanel>,
) -> Div {
    let panel = panel.clone();
    row.on_mouse_down(MouseButton::Right, move |_: &MouseDownEvent, _, cx| {
        let Some(panel) = panel.upgrade() else { return };
        panel.update(cx, |panel, _| {
            panel.menu_field = Some((label.clone(), value.clone()));
        });
    })
}

/// One labeled field of the sheet: the tag's name dimmed in a fixed
/// column, its value truncating beside it.
fn field(label: impl Into<SharedString>, value: Div) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(tokens::SPACE_SM)
        .child(
            div()
                .w(px(84.))
                .flex_none()
                .text_color(palette::text_muted())
                .child(label.into()),
        )
        .child(value)
}

/// One row of the table face: the label column, the value beside it,
/// faint striping and a bottom hairline as the knobs ask, both in the
/// library rows' colors. The stripe is translucent so the cover
/// background keeps showing through.
fn table_row(
    ix: usize,
    label: impl Into<SharedString>,
    value: Div,
    stripes: bool,
    borders: bool,
) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .px(tokens::SPACE_MD)
        .py(px(3.))
        .when(stripes && ix % 2 == 1, |d| {
            d.bg(palette::alpha(palette::bg_elevated(), 0x80))
        })
        .when(borders, |d| d.border_b_1().border_color(palette::border()))
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .text_color(palette::text_muted())
                .child(label.into()),
        )
        .child(value)
}

/// The scrolling frame every face is drawn in: as tall as its content,
/// capped at the panel. Short content leaves slack the body's column hands
/// to the vertical knob; tall content fills the panel and scrolls from the
/// top. The placement can't go inside the scroll box, since a percentage
/// height resolves to nothing in there and the column collapses onto its
/// content, which is why the sheet always stayed at the top no matter the
/// knob.
fn scroll_frame(id: &'static str, align: Align, content: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .max_h_full()
        .flex_none()
        .overflow_y_scroll()
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .map(|d| match align {
                    Align::Left => d.items_start(),
                    Align::Center => d.items_center(),
                    Align::Right => d.items_end(),
                })
                .child(content),
        )
}

impl Render for MetadataPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        // The panel is a focus stop: a click puts the keyboard here and
        // tab walks to it, which is also what puts its tab group on the
        // focus path for the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(cx).track_focus(&focus))
    }
}

impl MetadataPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        // The edit toggle goes in the tab bar via title_suffix while the
        // panel shares a group; solo or popped out there's no header at
        // all, so it renders as a toolbar in the body instead, the
        // library's move.
        let headerless = self
            .tab_panel
            .as_ref()
            .and_then(|tabs| tabs.upgrade())
            .is_none_or(|tabs| tabs.read(cx).panels_count() < 2);
        // Same show rule as the suffix: hidden while the panel shows no
        // track, unless an edit is already open. The chrome's finished-
        // furniture flag drops it too, for a slot in a shipped layout.
        // Deliberately the panel's own flag rather than `controls_hidden`:
        // this one edits tags, not the layout, so design mode leaves it be.
        let show_toggle = !self.config.chrome.hide_controls
            && (self.edit.is_some() || self.resolved_track(cx).is_some());
        // A right press arrives here in the capture phase, before any
        // row's own handler records itself, so a press off the rows
        // leaves no target and the menu below falls back to the panel's
        // entries alone. The history panel's shape.
        let sheet = self
            .sheet_body(cx)
            .flex_1()
            .min_h_0()
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.menu_field = None;
                }
            }));
        let weak = cx.entity().downgrade();
        let sheet = sheet.context_menu(move |menu, window, cx| {
            let Some(this) = weak.upgrade() else {
                return menu;
            };
            // Copy first, then the panel's normal entries after a
            // separator, so a right click over the sheet never loses what
            // the tab menu offers.
            let field = this.read(cx).menu_field.clone();
            let menu = match field {
                Some((label, value)) if !value.is_empty() => menu
                    .item(
                        PopupMenuItem::new(rox_i18n::t!(
                            "metadata-copy-field",
                            field = label.to_string()
                        ))
                        .icon(Icon::default().path(icons::COPY))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                        }),
                    )
                    .separator(),
                _ => menu,
            };
            this.update(cx, |this, cx| this.dropdown_menu(menu, window, cx))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .when(headerless && show_toggle, |d| d.child(self.toolbar(cx)))
            .child(sheet)
    }

    /// Solo or popped out there is no title bar to host the edit toggle,
    /// so it renders as a toolbar row above the sheet instead, the
    /// library's move.
    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let editing = self.edit.is_some();
        let weak = cx.entity().downgrade();
        div()
            .flex_none()
            .h(px(36.))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .bg(palette::bg_toolbar())
            .border_b_1()
            .border_color(palette::border())
            .child(
                settings_ui::icon_button(icons::PENCIL, false, move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.toggle_edit(window, cx));
                })
                .when(editing, |d| d.bg(palette::bg_control_active())),
            )
    }

    /// The sheet under the toolbar: the display face, or the edit face
    /// while an edit is open.
    fn sheet_body(&mut self, cx: &mut Context<Self>) -> Div {
        let align = self.config.align;
        // The faces are normal-flow children of this column, so the
        // vertical knob places them the way flexbox places any child that
        // leaves slack. The background art layers are absolute inside it,
        // out of the flow.
        let root = justify_v(div().relative().flex().flex_col(), self.config.valign);

        // The library scope: the catalog's own sheet, no track to
        // resolve, no cover, nothing to edit. An open edit still shows
        // its form, so a source flip mid-edit doesn't eat the typing.
        if self.config.source == MetadataSource::Library && self.edit.is_none() {
            return self.library_sheet(root, cx);
        }

        // An open edit pins its track; the source only drives the sheet
        // while nothing is being edited.
        let Some(key) = self
            .edit
            .as_ref()
            .map(|edit| edit.key.clone())
            .or_else(|| self.resolved_track(cx))
        else {
            // The source points at no track: a quiet line in place of the
            // sheet.
            return root.child(
                justify(div().w_full().flex_none().flex(), align)
                    .p(tokens::SPACE_MD)
                    .child(
                        div()
                            .text_color(palette::text_faint())
                            .child(rox_i18n::t!("content-no-track")),
                    ),
            );
        };

        // The background layer: the track's art cropped to fill, a scrim
        // over it so the fields keep reading over busy covers. Until the
        // load finishes the plain background stands in; no fade, the sheet's
        // text swaps in the same frame anyway.
        // Art hangs off the file, which cue tracks of one image share, so
        // the cache stays keyed on the path.
        let path = key.path.clone();
        if self.config.cover {
            self.ensure_art(&path, cx);
        }
        let backdrop = self.config.cover.then(|| self.art.get(&path)).flatten();
        let root = root.when_some(backdrop, |root, image| {
            root.child(
                div().absolute().inset_0().child(
                    img(image)
                        .overflow_hidden()
                        .object_fit(ObjectFit::Cover)
                        .size_full(),
                ),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(palette::alpha(palette::bg_root(), 0xB8)),
            )
        });

        if self.edit.is_some() {
            return root.child(scroll_frame("metadata-edit", align, self.edit_sheet(cx)));
        }

        // An untagged file still shows something: its file name for the
        // title, no fields.
        let details = self.details_for(&key, cx).cloned();
        let title = details
            .as_ref()
            .map(|d| d.title.clone())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string())
            });

        // A click on a taggable value narrows the app-wide search, but only
        // while a search panel is up somewhere to show the pick it writes,
        // since the chips appear beside that box. With none in the tree the
        // followers would narrow with nothing on screen saying why, or how
        // to undo it, so the values stay inert text instead.
        let query = self
            .state
            .query
            .read(cx)
            .has_box()
            .then(|| self.state.query.clone());
        // Counts up through the rendered values so each hit area gets its own
        // element id.
        let mut hit_id = 0usize;
        // Every row arms itself for the copy menu on a right press.
        let weak = cx.entity().downgrade();

        // The shown fields in registry order, each skipped when its value
        // is empty: absence reads cleaner than a labeled blank. The key
        // comes along for [`query_field`], which decides whether the value
        // is clickable.
        let mut fields: Vec<(gpui::SharedString, String, String, Option<Search>)> = Vec::new();
        for col in self::fields() {
            if !self.config.fields.iter().any(|k| k == col.key) {
                continue;
            }
            let value = match col.key {
                // The file name comes off the path, so it shows even for
                // a track the library doesn't know.
                "file" => path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
                key => details.as_ref().and_then(|d| match key {
                    "album" => (!d.album.is_empty()).then(|| d.album.clone()),
                    // Absent on everything but a romanized library, and
                    // an absent value skips its row, so a Latin-only
                    // sheet looks exactly as it did.
                    "title_sort" => (!d.title_sort.is_empty()).then(|| d.title_sort.clone()),
                    "artist_sort" => (!d.artist_sort.is_empty()).then(|| d.artist_sort.clone()),
                    "album_sort" => (!d.album_sort.is_empty()).then(|| d.album_sort.clone()),
                    "album_artist_sort" => (!d.album_artist_sort.is_empty()
                        && d.album_artist_sort != d.artist_sort)
                        .then(|| d.album_artist_sort.clone()),
                    "album_artist" => (!d.album_artist.is_empty() && d.album_artist != d.artist)
                        .then(|| d.album_artist.clone()),
                    "disc" => (d.disc_no > 0).then(|| d.disc_no.to_string()),
                    "track" => (d.track_no > 0).then(|| format!("{:02}", d.track_no)),
                    "genre" => (!d.genre.is_empty()).then(|| d.genre.clone()),
                    "year" => (d.year > 0).then(|| d.year.to_string()),
                    "duration" => {
                        (d.duration_ms > 0).then(|| fmt_time(d.duration_ms as f64 / 1000.0))
                    }
                    "codec" => (!d.codec.is_empty()).then(|| d.codec.clone()),
                    "bitrate" => (d.bitrate_kbps > 0).then(|| {
                        format!(
                            "{} kbps",
                            rox_i18n::format::format_int(i64::from(d.bitrate_kbps))
                        )
                    }),
                    "sample_rate" => (d.sample_rate_hz > 0)
                        .then(|| format!("{} kHz", crate::group_head::khz(d.sample_rate_hz))),
                    "bit_depth" => (d.bit_depth > 0).then(|| format!("{} bit", d.bit_depth)),
                    // Whole beats, like the library column: the fraction
                    // comes from the estimator, and an estimate says so.
                    "bpm" => d.bpm.map(|bpm| {
                        let beats = rox_i18n::format::format_int(bpm.round() as i64);
                        if d.bpm_measured {
                            rox_i18n::t!("metadata-field-bpm-measured", bpm = beats).to_string()
                        } else {
                            beats
                        }
                    }),
                    "gain_track" => d.track_gain_db.map(fmt_gain),
                    "gain_album" => d.album_gain_db.map(fmt_gain),
                    "added" => (d.added > 0).then(|| rox_core::fmt::fmt_date(d.added)),
                    "plays" => (d.plays > 0).then(|| track_columns::fmt_plays(d.plays)),
                    "rating" => (d.rating > 0).then(|| crate::rating_ui::fmt(d.rating).to_string()),
                    _ => None,
                }),
            };
            if let Some(value) = value {
                // A field row's value is the tag itself, sort rows
                // included, so none of them takes a reading.
                fields.push((
                    col.label.clone(),
                    value,
                    String::new(),
                    query_field(col.key),
                ));
            }
        }
        let artist = details
            .as_ref()
            .map(|d| d.artist.clone())
            .filter(|a| !a.is_empty());
        // The head's two readings. A file the library doesn't know has no
        // details and so no sort names, which is also the case where the
        // title above is a file name rather than a tag.
        let title_reading = details
            .as_ref()
            .map(|d| d.title_sort.clone())
            .unwrap_or_default();
        let artist_reading = details
            .as_ref()
            .map(|d| d.artist_sort.clone())
            .unwrap_or_default();
        // The title only searches when the library has the track; for one
        // it doesn't the line is the file name, which no tag holds.
        let title_field = details.as_ref().and_then(|_| query_field("title"));

        // The table face: title and artist fold in as rows, the list goes
        // where the vertical knob puts it, and it scrolls when the panel
        // runs short.
        if self.config.display == MetadataDisplay::Table {
            let mut rows: Vec<(gpui::SharedString, String, String, Option<Search>)> = Vec::new();
            rows.push((
                rox_i18n::t!("info-item-title"),
                title,
                title_reading,
                title_field,
            ));
            if let Some(artist) = artist {
                rows.push((
                    rox_i18n::t!("head-piece-artist"),
                    artist,
                    artist_reading,
                    query_field("artist"),
                ));
            }
            rows.extend(fields);
            let stripes = self.config.stripes;
            let borders = self.config.row_borders;
            let rows: Vec<Div> = rows
                .into_iter()
                .enumerate()
                .map(|(ix, (label, value, reading, field))| {
                    let cell = value_cell(&value, &reading, field, query.as_ref(), &mut hit_id);
                    let row = table_row(ix, label.clone(), cell, stripes, borders);
                    copy_target(row, label, value, &weak)
                })
                .collect();
            return root.child(
                div()
                    .id("metadata-table")
                    .w_full()
                    .max_h_full()
                    .flex_none()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .py(tokens::SPACE_XS)
                            .children(rows),
                    ),
            );
        }

        // The sheet: title over artist, the fields below, placed by the
        // two alignment knobs. The cells build up front so each one gets
        // its turn at the shared hit-id counter.
        let title_cell = value_cell(
            &title,
            &title_reading,
            title_field,
            query.as_ref(),
            &mut hit_id,
        )
        .text_lg()
        .text_color(palette::text_bright())
        .max_w_full();
        let title_cell = copy_target(
            title_cell,
            rox_i18n::t!("info-item-title"),
            title.clone(),
            &weak,
        );
        let artist_cell = artist.map(|artist| {
            let cell = value_cell(
                &artist,
                &artist_reading,
                query_field("artist"),
                query.as_ref(),
                &mut hit_id,
            )
            .text_color(palette::text_muted())
            .max_w_full();
            copy_target(cell, rox_i18n::t!("head-piece-artist"), artist, &weak)
        });
        let rows: Vec<Div> = fields
            .into_iter()
            .map(|(label, value, reading, search)| {
                let row = field(
                    label.clone(),
                    value_cell(&value, &reading, search, query.as_ref(), &mut hit_id),
                );
                copy_target(row, label, value, &weak)
            })
            .collect();
        let sheet = div()
            .max_w_full()
            .min_w_0()
            .p(tokens::SPACE_MD)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(title_cell)
            .children(artist_cell)
            .when(!rows.is_empty(), |d| {
                d.child(
                    div()
                        .mt(tokens::SPACE_MD)
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(rows),
                )
            });

        root.child(scroll_frame("metadata-sheet", align, sheet))
    }

    /// The library scope's face: the catalog's counts through the same
    /// two layouts the track fields use, "Library" standing where the
    /// title does. Nothing here names a track, so there's no cover
    /// backdrop and the values stay inert text.
    fn library_sheet(&mut self, root: Div, cx: &mut Context<Self>) -> Div {
        let align = self.config.align;
        let display = self.config.display;
        let stripes = self.config.stripes;
        let borders = self.config.row_borders;
        let Some(totals) = self.library_totals(cx) else {
            // The projection is still loading: the quiet line the track
            // scopes show while they point at nothing.
            return root.child(
                justify(div().w_full().flex_none().flex(), align)
                    .p(tokens::SPACE_MD)
                    .child(
                        div()
                            .text_color(palette::text_faint())
                            .child(rox_i18n::t!("metadata-no-library")),
                    ),
            );
        };
        let fields: Vec<(gpui::SharedString, String)> = vec![
            (
                rox_i18n::t!("head-piece-tracks"),
                rox_i18n::format::format_int(totals.tracks as i64),
            ),
            (
                rox_i18n::t!("status-item-albums"),
                rox_i18n::format::format_int(totals.albums as i64),
            ),
            (
                rox_i18n::t!("status-item-artists"),
                rox_i18n::format::format_int(totals.artists as i64),
            ),
            (
                rox_i18n::t!("content-total-genres"),
                rox_i18n::format::format_int(totals.genres as i64),
            ),
            (
                rox_i18n::t!("content-total-time"),
                format!(
                    "{} ({})",
                    crate::group_head::fmt_total(totals.total_ms),
                    rox_core::fmt::fmt_span(totals.total_ms / 1000)
                ),
            ),
            (
                rox_i18n::t!("status-item-plays"),
                rox_i18n::format::format_int(totals.plays as i64),
            ),
        ];
        let mut hit_id = 0usize;
        if display == MetadataDisplay::Table {
            let rows: Vec<Div> = fields
                .into_iter()
                .enumerate()
                .map(|(ix, (label, value))| {
                    let cell = value_cell(&value, "", None, None, &mut hit_id);
                    table_row(ix, label, cell, stripes, borders)
                })
                .collect();
            return root.child(
                div()
                    .id("metadata-table")
                    .w_full()
                    .max_h_full()
                    .flex_none()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .py(tokens::SPACE_XS)
                            .children(rows),
                    ),
            );
        }
        let title_cell = value_cell(
            &rox_i18n::t!("panel-title-library"),
            "",
            None,
            None,
            &mut hit_id,
        )
        .text_lg()
        .text_color(palette::text_bright())
        .max_w_full();
        let rows: Vec<Div> = fields
            .into_iter()
            .map(|(label, value)| field(label, value_cell(&value, "", None, None, &mut hit_id)))
            .collect();
        let sheet = div()
            .max_w_full()
            .min_w_0()
            .p(tokens::SPACE_MD)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(title_cell)
            .child(
                div()
                    .mt(tokens::SPACE_MD)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(rows),
            );
        root.child(scroll_frame("metadata-sheet", align, sheet))
    }

    /// The sheet's edit face: one input per editable field, the save and
    /// cancel row under them, and whatever error the last read or commit
    /// left. Enter saves through the inputs' own event; Escape cancels
    /// here, where the widget propagates it.
    fn edit_sheet(&self, cx: &mut Context<Self>) -> Div {
        let Some(edit) = &self.edit else {
            return div();
        };
        let edit_fields = edit_fields();
        let rows = edit_fields
            .iter()
            .zip(&edit.inputs)
            .map(|((_, label), input)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(
                        div()
                            .w(px(84.))
                            .flex_none()
                            .text_color(palette::text_muted())
                            .child(label.clone()),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(input).small()))
            });
        div()
            // Scopes the workspace's playback key bindings out while an
            // input is focused, so space and arrows type instead.
            .key_context("SearchInput")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                cx.stop_propagation();
                this.close_edit(cx);
            }))
            .w_full()
            .max_w(px(420.))
            .p(tokens::SPACE_MD)
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(rows)
            // Romanize's one refusal: a kanji name with no dictionary
            // installed. Muted, under the rows, and it says where the
            // download is.
            .when_some(edit.note.clone(), |d, note| {
                d.child(
                    div()
                        .mt(tokens::SPACE_XS)
                        .text_color(palette::text_muted())
                        .child(note),
                )
            })
            .when_some(edit.error.clone(), |d, error| {
                d.child(
                    div()
                        .mt(tokens::SPACE_XS)
                        .text_color(palette::text_muted())
                        .child(error),
                )
            })
            .child(
                div()
                    .mt(tokens::SPACE_XS)
                    .flex()
                    .flex_row()
                    .gap(tokens::SPACE_SM)
                    .child(settings_ui::small_button(
                        rox_i18n::t!("metadata-edit-save"),
                        icons::CHECK,
                        edit.saving || edit.baseline.is_none(),
                        cx.listener(|this, _, _, cx| this.save_edit(cx)),
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("bake-cancel"),
                        icons::CLOSE,
                        edit.saving,
                        cx.listener(|this, _, _, cx| this.close_edit(cx)),
                    ))
                    // Fills the empty sort inputs and stops there: what
                    // it wrote is a proposal until Save takes it to the
                    // file, and a typed sort name survives it.
                    .child(settings_ui::small_button(
                        rox_i18n::t!("metadata-romanize"),
                        icons::GLOBE,
                        // Inert until the baseline read lands, like Save:
                        // the read fills every input, so a fill before it
                        // would be overwritten a moment later.
                        edit.saving || edit.baseline.is_none(),
                        cx.listener(|this, _, window, cx| this.fill_romanize(window, cx)),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        diff_baseline, edit_fields, fields, sort_fill, sort_targets, Fill, MetadataConfig,
        MetadataSource,
    };
    use rox_library::writer::Field;
    use rox_romanize::Reading;

    /// One input value per [`edit_fields`] entry, seeded from a baseline
    /// so nothing reads as drifted until the caller moves a field.
    fn inputs(baseline: &[(Field, String)]) -> Vec<String> {
        edit_fields()
            .iter()
            .map(|(field, _)| {
                baseline
                    .iter()
                    .find(|(f, _)| f == field)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// Typing a romanization into one sort field writes that field and
    /// nothing else, and leaving the form alone writes nothing at all.
    /// This is the property the baseline diff exists for, and the one
    /// that breaks if `writer::field_of` is missing a reverse arm: a
    /// sort field with no baseline entry would read as always-dirty.
    #[test]
    fn only_the_moved_sort_field_writes() {
        let baseline = vec![
            (Field::Artist, "米津玄師".to_string()),
            (Field::ArtistSort, "Yonezu, Kenshi".to_string()),
        ];

        assert!(diff_baseline(&inputs(&baseline), &baseline).is_empty());

        let ix = edit_fields()
            .iter()
            .position(|(field, _)| *field == Field::AlbumArtistSort)
            .expect("the album artist sort field is in the form");
        let mut values = inputs(&baseline);
        values[ix] = "Yonezu, Kenshi".to_string();
        let changes = diff_baseline(&values, &baseline);
        assert!(changes.len() == 1);
        assert!(changes[0].field == Field::AlbumArtistSort);
        assert!(changes[0].value.as_deref() == Some("Yonezu, Kenshi"));

        // Emptying a sort field the file carries drops the tag.
        let mut values = inputs(&baseline);
        let ix = edit_fields()
            .iter()
            .position(|(field, _)| *field == Field::ArtistSort)
            .unwrap();
        values[ix].clear();
        let changes = diff_baseline(&values, &baseline);
        assert!(changes.len() == 1);
        assert!(changes[0].field == Field::ArtistSort);
        assert!(changes[0].value.is_none());
    }

    /// Every sort field the form edits has a read-only row to show it
    /// when the panel isn't editing, so a value a user types comes back
    /// as a labeled row instead of vanishing until the next edit.
    #[test]
    fn sort_names_show_outside_the_edit_form() {
        for key in [
            "title_sort",
            "artist_sort",
            "album_artist_sort",
            "album_sort",
        ] {
            assert!(fields().iter().any(|col| col.key == key), "{key}");
        }
        for field in [
            Field::TitleSort,
            Field::ArtistSort,
            Field::AlbumArtistSort,
            Field::AlbumSort,
        ] {
            assert!(edit_fields().iter().any(|(f, _)| *f == field));
        }
    }

    /// Every projection field the library table offers has a sheet row
    /// too, so the two faces never disagree about what a track carries.
    #[test]
    fn projection_extras_have_fields() {
        for key in ["bpm", "gain_track", "gain_album", "added"] {
            assert!(fields().iter().any(|col| col.key == key), "{key}");
        }
    }

    /// A layout saved before the library scope existed spells its source
    /// the shared track pair's way, and still reads; no source at all is
    /// the stock follow-playing.
    #[test]
    fn track_sources_read_unchanged() {
        let config: MetadataConfig = serde_json::from_str(r#"{"source": "selected"}"#).unwrap();
        assert!(config.source == MetadataSource::Selected);

        let config: MetadataConfig = serde_json::from_str("{}").unwrap();
        assert!(config.source == MetadataSource::Playing);

        let config: MetadataConfig = serde_json::from_str(r#"{"source": "library"}"#).unwrap();
        assert!(config.source == MetadataSource::Library);
    }

    /// Romanize's decision, per field. The dictionary is never installed
    /// on a CI runner, so these are the cases that answer without one:
    /// kana reads, kanji doesn't, and neither one gets to touch a value
    /// somebody typed.
    #[test]
    fn romanize_fills_only_the_empty_sort_inputs() {
        // An empty input with a non-Latin name gets its reading.
        assert_eq!(
            sort_fill("レモン", "", "", Reading::Auto, None),
            Fill::Value("Remon".to_string())
        );
        // A typed sort name is never overwritten, whatever else is on
        // offer.
        assert_eq!(
            sort_fill("レモン", "Lemon", "", Reading::Auto, None),
            Fill::Leave
        );
        assert_eq!(
            sort_fill("レモン", "Lemon", "remon", Reading::Auto, None),
            Fill::Leave
        );
        // A Latin name has no reading to add, so the row stays empty
        // rather than filling with a copy of itself, and the sort name
        // the library holds for it stays out of the file too: this
        // button romanizes, it doesn't fill sort names in general.
        assert_eq!(sort_fill("Lemon", "", "", Reading::Auto, None), Fill::Leave);
        assert_eq!(
            sort_fill("The Beatles", "", "Beatles, The", Reading::Auto, None),
            Fill::Leave
        );
        // The sort name the library already holds wins over a fresh
        // reading: it's MusicBrainz's or the user's answer, where the
        // reading is IPADIC's guess.
        assert_eq!(
            sort_fill("米津玄師", "", "Yonezu, Kenshi", Reading::Japanese, None),
            Fill::Value("Yonezu, Kenshi".to_string())
        );
        // Kanji with nothing stored and no dictionary: the sheet says so
        // instead of writing a Chinese reading of Japanese text.
        assert_eq!(
            sort_fill("米津玄師", "", "", Reading::Japanese, None),
            Fill::NeedsDictionary
        );
    }

    /// Every sort field the button fills is a field the form actually
    /// has, and each one is paired with the name it sorts.
    #[test]
    fn romanize_covers_the_four_sort_fields() {
        let targets = sort_targets(None);
        for (sort, base, stored) in &targets {
            assert!(edit_fields().iter().any(|(f, _)| f == sort), "{sort:?}");
            assert!(edit_fields().iter().any(|(f, _)| f == base), "{base:?}");
            // No row, nothing stored: the fill falls back to reading the
            // name itself.
            assert!(stored.is_empty());
        }
        for field in [
            Field::TitleSort,
            Field::ArtistSort,
            Field::AlbumSort,
            Field::AlbumArtistSort,
        ] {
            assert!(targets.iter().any(|(sort, _, _)| *sort == field));
        }
    }

    /// The row context menu names the field it copies, so a right click
    /// on Title offers Copy Title rather than a bare Copy.
    #[test]
    fn the_copy_entry_carries_the_rows_label() {
        let label = rox_i18n::t!("info-item-title");
        let entry = rox_i18n::t!("metadata-copy-field", field = label.to_string());
        assert!(
            entry.contains(label.as_ref()),
            "{entry} should name {label}"
        );
        assert!(entry.as_ref() != label.as_ref());
    }
}
