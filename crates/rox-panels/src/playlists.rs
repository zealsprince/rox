//! The playlists panel (ADR 16): a tree of playlists, each expanding to its
//! tracks. Its own panel, never a mode of the library.
//!
//! A smart playlist is the same tree row over a saved query. It
//! materializes on every refresh, so there's no cache to invalidate, and it
//! refuses member edits out loud rather than dropping them.
//!
//! The cost: rating and play edits patch rows in place and rebuild nothing,
//! so a smart playlist keyed on either can show a stale row until the next
//! refresh. Accepted; re-materializing every open smart list per star click
//! is worse. A bulk play-count import does refresh.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Modifiers,
    MouseButton, MouseDownEvent, PathPromptOptions, Pixels, ScrollStrategy, ScrollWheelEvent,
    SharedString, Stateful, Subscription, UniformListScrollHandle, WeakEntity, Window, div,
    prelude::*, px, rems, svg, uniform_list,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{ContextMenuExt, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Sizable};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::continuation;
use crate::design::{palette, tokens};
use crate::group_head::{self, ArtSide, HeadPiece, Headers};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, ResumeIdle, ScrubState};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::selection::SelectionEvent;
use crate::settings::ui as settings_ui;
use crate::track_ui::track_cells;
use crate::track_ui::track_columns::{
    self, ART_MARGIN_MAX, Column, ColumnHost, GroupTrack, HEAD_GAP_MAX, HEAD_HEIGHT_MAX,
    HEAD_TEXT_MAX, HEAD_TEXT_MIN, HEAD_TEXT_STOCK, HeadSlot, HeadingHost, ROW_HEIGHT_MAX,
    ROW_HEIGHT_MIN, ROW_HEIGHT_STOCK, ROW_SPACING_MAX,
};
use crate::track_ui::track_drag::PlayDrag;
use rox_library::playlist_file::Format;
use rox_library::playlists::{PlaylistKind, PlaylistTrack};
use rox_library::projection::{FilterSet, Filterable, Term, parse_query};
use rox_panel_kit::config::default_true;

const ART_ROUNDING_MAX: f32 = 24.;

/// Serde's default, so an older layout opens at the size its headings
/// already drew.
fn default_head_text() -> f32 {
    HEAD_TEXT_STOCK
}

/// Denser than the library's [`ROW_HEIGHT_STOCK`]: a playlist is browsed
/// in short bursts, not read top to bottom. [`row_font_scale`] still
/// measures against the shared stock.
const ROW_HEIGHT_DEFAULT: f32 = 24.;

/// Render order; the config picks which show.
fn columns() -> Vec<Column> {
    vec![
        Column {
            key: "cover",
            label: rox_i18n::t!("columns-cover"),
            default_on: false,
        },
        Column {
            key: "number",
            label: rox_i18n::t!("columns-number"),
            default_on: true,
        },
        Column {
            key: "name",
            label: rox_i18n::t!("columns-name"),
            default_on: true,
        },
        Column {
            key: "artist",
            label: rox_i18n::t!("head-piece-artist"),
            default_on: true,
        },
        Column {
            key: "album",
            label: rox_i18n::t!("head-piece-album"),
            default_on: false,
        },
        Column {
            key: "year",
            label: rox_i18n::t!("head-piece-year"),
            default_on: false,
        },
        Column {
            key: "genre",
            label: rox_i18n::t!("head-piece-genre"),
            default_on: false,
        },
        Column {
            key: "duration",
            label: rox_i18n::t!("info-item-duration"),
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
            default_on: true,
        },
        Column {
            key: "favourite",
            label: rox_i18n::t!("info-item-favourite"),
            default_on: true,
        },
    ]
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistsConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub expanded: Vec<i64>,
    /// The library's album grouping brought to the tree.
    pub headers: Headers,
    /// In no particular order; render order is the registry's.
    pub columns: Vec<String>,
    #[serde(default)]
    pub search: bool,
    #[serde(default)]
    pub query_source: QuerySource,
    /// Kept while following the shared query, for the switch back.
    #[serde(default)]
    pub query: String,
    /// Px at the stock font size; the app and panel font scales multiply it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_height: Option<f32>,
    /// Can only shrink a line inside the row the list gives it; see
    /// [`PlaylistsPanel::line_px`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_height: Option<f32>,
    /// Grown into each row, which the row fills.
    #[serde(default)]
    pub row_spacing: f32,
    /// Free of the line height, so the cover tile grows without the text.
    #[serde(default = "default_head_text")]
    pub head_text: f32,
    #[serde(default)]
    pub art_rounding: f32,
    #[serde(default)]
    pub art_side: ArtSide,
    /// Px at the stock font size; the tile shrinks to stay square.
    #[serde(default)]
    pub art_margin: f32,
    /// The list shows through, so a block reads apart from the run above.
    #[serde(default)]
    pub header_gap_above: f32,
    #[serde(default)]
    pub header_gap_below: f32,
    #[serde(default = "default_true")]
    pub header_art: bool,
    /// A role, not a color, so song theming moves the headings with the list.
    #[serde(default)]
    pub header_flush: bool,
    /// Empty falls back to the stock packing.
    #[serde(default)]
    pub header_compact: Vec<HeadPiece>,
    /// The name row and the meta row under it; the tree's heading has exactly
    /// these two (see [`Row`]). Empty falls back to the stock line.
    #[serde(default)]
    pub header_name_line: Vec<HeadPiece>,
    #[serde(default)]
    pub header_meta_line: Vec<HeadPiece>,
    /// A small count with a faint dash, the classic playlist tick.
    #[serde(default)]
    pub compact_plays: bool,
    #[serde(default = "default_true")]
    pub stripes: bool,
    #[serde(default = "default_true")]
    pub row_borders: bool,
    #[serde(default)]
    pub follow_playing: bool,
    /// After the tree goes untouched for a spell.
    #[serde(default)]
    pub resume_playing: bool,
    #[serde(default)]
    pub smooth_follow: bool,
    /// An index, not pixels, so it survives a height change.
    #[serde(default)]
    pub scroll_row: usize,
}

// Hand-written so the columns default to the registry set and the
// headings to off.
impl Default for PlaylistsConfig {
    fn default() -> Self {
        PlaylistsConfig {
            chrome: PanelChrome::default(),
            expanded: Vec::new(),
            headers: Headers::Off,
            columns: track_columns::default_columns(&columns()),
            search: false,
            query_source: QuerySource::default(),
            query: String::new(),
            row_height: None,
            head_height: None,
            row_spacing: 0.,
            head_text: HEAD_TEXT_STOCK,
            art_rounding: 0.,
            art_side: ArtSide::default(),
            art_margin: 0.,
            header_gap_above: 0.,
            header_gap_below: 0.,
            header_art: true,
            header_flush: false,
            header_compact: Vec::new(),
            header_name_line: Vec::new(),
            header_meta_line: Vec::new(),
            compact_plays: false,
            stripes: true,
            row_borders: true,
            follow_playing: false,
            resume_playing: false,
            smooth_follow: false,
            scroll_row: 0,
        }
    }
}

/// An empty list means never edited and reads as the stock arrangement.
/// Saved lists come back deduped against the registry.
fn fold_head_lines(config: &PlaylistsConfig) -> (Vec<HeadPiece>, Vec<HeadPiece>, Vec<HeadPiece>) {
    let fold = |saved: &[HeadPiece], stock: fn() -> Vec<HeadPiece>| {
        if saved.is_empty() {
            stock()
        } else {
            panel::dedup(group_head::PIECES, saved.to_vec())
        }
    };
    (
        fold(&config.header_compact, group_head::stock_compact),
        fold(&config.header_name_line, group_head::stock_name_line),
        fold(&config.header_meta_line, group_head::stock_meta_line),
    )
}

enum Row {
    Head {
        id: i64,
        name: String,
        count: u64,
        expanded: bool,
        /// The default playlist behind the heart column, shielded from rename and
        /// delete.
        favourite: bool,
        /// A saved query: takes no member edits and offers Edit Query.
        smart: bool,
    },
    /// Indexes [`PlaylistsPanel::albums`]; one block per run of tracks sharing
    /// an album.
    Album(u32),
    AlbumMeta(u32),
    Track(TrackRow),
}

/// The favourite is looked up live off the panel's set.
struct TrackRow {
    playlist_id: i64,
    /// A static row's member rowid, or [`smart_key`]'s negative stand-in for
    /// a smart row.
    member_id: i64,
    track_id: i64,
    /// So edits that need a member row refuse instead of silently doing
    /// nothing.
    smart: bool,
    /// 1-based play order, unbroken through the album headings.
    pos: u32,
    title: String,
    artist: String,
    album: String,
    /// Empty for a member the library no longer holds.
    title_reading: String,
    artist_reading: String,
    album_reading: String,
    year: u16,
    genre: String,
    duration_ms: u32,
    rating: u8,
    plays: u32,
    path: String,
}

/// A smart row has no member rowid, so a hash of the playlist and track
/// ids stands in, stable across refreshes. Forced negative so it can't
/// collide with a real member id: `member < 0` means "from a query".
fn smart_key(playlist_id: i64, track_id: i64) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in playlist_id
        .to_le_bytes()
        .iter()
        .chain(track_id.to_le_bytes().iter())
    {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Shift into the positive half before negating, and set the low bit so
    // the key is never zero.
    -(((hash >> 1) as i64) | 1)
}

impl TrackRow {
    fn new(
        playlist_id: i64,
        pos: u32,
        t: &PlaylistTrack,
        plays: u32,
        sort: rox_services::catalog::SortNames,
        smart: bool,
    ) -> TrackRow {
        TrackRow {
            playlist_id,
            member_id: if smart {
                smart_key(playlist_id, t.track_id)
            } else {
                t.member_id
            },
            track_id: t.track_id,
            smart,
            pos,
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            title_reading: sort.title,
            artist_reading: sort.artist,
            album_reading: sort.album,
            year: t.year,
            genre: t.genre.clone(),
            duration_ms: t.duration_ms,
            rating: t.rating,
            plays,
            path: t.path.clone(),
        }
    }
}

fn group_track(t: &PlaylistTrack) -> GroupTrack<'_> {
    GroupTrack {
        album: &t.album,
        album_artist: &t.album_artist,
        artist: &t.artist,
        year: t.year,
        genre: &t.genre,
        codec: &t.codec,
        bitrate_kbps: t.bitrate_kbps,
        sample_rate_hz: t.sample_rate_hz,
        bit_depth: t.bit_depth,
        duration_ms: t.duration_ms,
        track_id: t.track_id,
    }
}

/// No source playlist: where the rows land is the drop target's call.
#[derive(Clone)]
struct TrackDrag {
    members: Arc<[i64]>,
    title: SharedString,
}

struct TrackDragPreview {
    title: SharedString,
    extra: usize,
}

impl Render for TrackDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.extra > 0 {
            SharedString::from(format!(
                "{} +{}",
                self.title,
                rox_i18n::format::format_int(self.extra as i64)
            ))
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

pub struct PlaylistsPanel {
    state: AppState,
    config: PlaylistsConfig,
    search: Entity<SearchBox>,
    /// Applied on the next render, where a window exists to set the input.
    resync_box: bool,
    /// Runtime only: a restore re-pins from whatever is picked then.
    selection_ids: Vec<i64>,
    /// Snapshotted on query change; a searching tree surfaces matches from
    /// every list.
    applied_query: String,
    applied_filter: FilterSet,
    rows: Vec<Row>,
    /// Empty when the headings are off.
    albums: Vec<track_columns::AlbumGroup>,
    expanded: HashSet<i64>,
    /// Reloaded every refresh, since a favourite toggle emits the same event
    /// as a playlist edit.
    favourites: HashSet<i64>,
    playing: Option<i64>,
    /// By member id, so a rescan, expand, or reorder keeps the highlight.
    selected: HashSet<i64>,
    /// Bumped on a selection or row-order change, keying the drag-set cache
    /// so every visible selected row shares one Arc.
    drag_gen: u64,
    drag_set: Option<(u64, Arc<[i64]>)>,
    /// A member id, so it survives a rebuild.
    anchor: Option<i64>,
    menu_row: Option<usize>,
    /// What a smart playlist refused and why. Cleared on the next refresh.
    refusal: Option<SharedString>,
    /// Px at the stock font size; together they make the list's stride.
    row_height: f32,
    row_spacing: f32,
    head_height: f32,
    head_text: f32,
    art_rounding: f32,
    art_side: ArtSide,
    art_margin: f32,
    header_gap_above: f32,
    header_gap_below: f32,
    header_art: bool,
    header_flush: bool,
    header_compact: Vec<HeadPiece>,
    header_name_line: Vec<HeadPiece>,
    header_meta_line: Vec<HeadPiece>,
    compact_plays: bool,
    stripes: bool,
    row_borders: bool,
    row_scrub: ScrubState,
    row_spacing_scrub: ScrubState,
    head_scrub: ScrubState,
    head_text_scrub: ScrubState,
    art_scrub: ScrubState,
    art_margin_scrub: ScrubState,
    header_gap_above_scrub: ScrubState,
    header_gap_below_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    follow_playing: bool,
    smooth_follow: bool,
    /// So a refresh that leaves the playing track in place doesn't scroll
    /// again.
    followed_row: Option<usize>,
    /// The resume switch and the idle clock that fires it.
    resume_playing: bool,
    resume_idle: ResumeIdle,
    /// Stepped each frame in [`PlaylistsPanel::body`], cleared on arrival.
    glide_to: Option<usize>,
    glide_tick: Instant,
    /// The catalog loads after the panel builds, so the first non-empty tree
    /// consumes this.
    restore_scroll: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _player_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
}

impl PlaylistsPanel {
    pub fn new(
        state: AppState,
        config: PlaylistsConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let expanded: HashSet<i64> = config.expanded.iter().copied().collect();
        // A rating click patches in place instead of reloading the expanded
        // lists. A play-count import is a reload: it moves smart-list membership.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Rated) {
                    this.patch_ratings(cx);
                    return;
                }
                if matches!(
                    event,
                    LibraryEvent::PlaylistsChanged
                        | LibraryEvent::Updated
                        | LibraryEvent::PlaysReloaded
                ) {
                    this.refresh(cx);
                }
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut Self, _, cx| cx.notify());
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| this.on_shared_query_changed(cx),
        );
        let selection_ids = state.selection.read(cx).tracks().to_vec();
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                this.on_selection_changed(event.source, cx);
            },
        );
        // Clamped to the bands the inputs allow, so a hand-edited dump can't
        // hand the render a nonsense height.
        let row_height =
            track_columns::fold_row_height(config.row_height, ROW_HEIGHT_DEFAULT, ROW_HEIGHT_MAX);
        let head_height =
            track_columns::fold_row_height(config.head_height, row_height, HEAD_HEIGHT_MAX);
        let (header_compact, header_name_line, header_meta_line) = fold_head_lines(&config);
        let mut this = PlaylistsPanel {
            state,
            search,
            resync_box: false,
            selection_ids,
            applied_query: String::new(),
            applied_filter: FilterSet::default(),
            rows: Vec::new(),
            albums: Vec::new(),
            expanded,
            favourites: HashSet::new(),
            playing: None,
            selected: HashSet::new(),
            drag_gen: 0,
            drag_set: None,
            anchor: None,
            menu_row: None,
            refusal: None,
            row_height,
            row_spacing: track_columns::fold_margin(config.row_spacing, ROW_SPACING_MAX),
            head_height,
            head_text: track_columns::fold_head_text(config.head_text),
            art_rounding: config.art_rounding,
            art_side: config.art_side,
            art_margin: track_columns::fold_margin(config.art_margin, ART_MARGIN_MAX),
            header_gap_above: track_columns::fold_margin(config.header_gap_above, HEAD_GAP_MAX),
            header_gap_below: track_columns::fold_margin(config.header_gap_below, HEAD_GAP_MAX),
            header_art: config.header_art,
            header_flush: config.header_flush,
            header_compact,
            header_name_line,
            header_meta_line,
            compact_plays: config.compact_plays,
            stripes: config.stripes,
            row_borders: config.row_borders,
            row_scrub: ScrubState::default(),
            row_spacing_scrub: ScrubState::default(),
            head_scrub: ScrubState::default(),
            head_text_scrub: ScrubState::default(),
            art_scrub: ScrubState::default(),
            art_margin_scrub: ScrubState::default(),
            header_gap_above_scrub: ScrubState::default(),
            header_gap_below_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            follow_playing: config.follow_playing,
            smooth_follow: config.smooth_follow,
            followed_row: None,
            resume_playing: config.resume_playing,
            resume_idle: ResumeIdle::default(),
            glide_to: None,
            glide_tick: Instant::now(),
            restore_scroll: (config.scroll_row > 0).then_some(config.scroll_row),
            config,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _library_changed,
            _player_changed,
            _thumbs_changed,
            _search_events,
            _query_changed,
            _selection_changed,
        };
        this.refresh(cx);
        this.sync_playing(cx);
        this
    }

    /// While a query is active every list opens and only matches show; a list
    /// with no match drops out.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_query(cx);
        let terms = parse_query(&self.applied_query);
        let searching = !self.applied_query.is_empty() || !self.applied_filter.is_empty();
        let library = self.state.library.read(cx);
        let favourites = library.favourite_ids();
        let mut rows = Vec::new();
        let mut albums = Vec::new();
        for playlist in library.playlists() {
            let expanded = self.expanded.contains(&playlist.id);
            // A searching tree loads every list; otherwise only the expanded ones.
            let show_tracks = expanded || searching;
            let smart = playlist.kind == PlaylistKind::Smart;
            // A smart playlist materializes even collapsed, since its header count is
            // the result's length. Collapsed, that's the ids alone.
            let def = smart
                .then(|| library.playlist_definition(playlist.id))
                .flatten();
            let (count, all) = match (&def, show_tracks) {
                (Some(def), true) => {
                    let rows = library.smart_tracks(def);
                    (rows.len() as u64, rows)
                }
                (Some(def), false) => (library.smart_ids(def).len() as u64, Vec::new()),
                // A smart playlist whose definition won't load holds nothing.
                (None, _) if smart => (0, Vec::new()),
                (None, true) => (playlist.tracks, library.playlist_tracks(playlist.id)),
                (None, false) => (playlist.tracks, Vec::new()),
            };
            // Positions stay the playlist's, not the filtered run's.
            let visible: Vec<usize> = if searching {
                (0..all.len())
                    .filter(|&i| self.track_visible(&terms, &all[i]))
                    .collect()
            } else {
                (0..all.len()).collect()
            };
            if searching && visible.is_empty() {
                continue;
            }
            rows.push(Row::Head {
                id: playlist.id,
                name: playlist.name,
                count,
                expanded: show_tracks,
                favourite: playlist.favourite,
                smart,
            });
            if !show_tracks {
                continue;
            }
            let ids: Vec<i64> = visible.iter().map(|&i| all[i].track_id).collect();
            let plays = library.plays_for(&ids);
            let plays_of = |t: &PlaylistTrack| plays.get(&t.track_id).copied().unwrap_or(0);
            // Readings looked up at rebuild time, not per paint.
            let sort_of = |t: &PlaylistTrack| library.sort_names_for_id(t.track_id);
            if self.config.headers == Headers::Off {
                for &i in &visible {
                    rows.push(Row::Track(TrackRow::new(
                        playlist.id,
                        (i + 1) as u32,
                        &all[i],
                        plays_of(&all[i]),
                        sort_of(&all[i]),
                        smart,
                    )));
                }
                continue;
            }
            // A heading opens each run of shown tracks sharing an album, in play
            // order with no re-sort. Empty albums merge into one Unknown run.
            let mut k = 0;
            while k < visible.len() {
                let mut m = k + 1;
                let head = &all[visible[k]];
                while m < visible.len()
                    && all[visible[m]].album == head.album
                    && all[visible[m]].album_artist == head.album_artist
                {
                    m += 1;
                }
                let group: Vec<GroupTrack> = visible[k..m]
                    .iter()
                    .map(|&i| group_track(&all[i]))
                    .collect();
                albums.push(track_columns::album_group(&group));
                let g = (albums.len() - 1) as u32;
                rows.push(Row::Album(g));
                if self.config.headers == Headers::Expanded {
                    rows.push(Row::AlbumMeta(g));
                }
                for &i in &visible[k..m] {
                    rows.push(Row::Track(TrackRow::new(
                        playlist.id,
                        (i + 1) as u32,
                        &all[i],
                        plays_of(&all[i]),
                        sort_of(&all[i]),
                        smart,
                    )));
                }
                k = m;
            }
        }
        self.rows = rows;
        self.drag_gen += 1;
        self.albums = albums;
        self.favourites = favourites;
        // Keep only members that still exist.
        let live: HashSet<i64> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) => Some(t.member_id),
                _ => None,
            })
            .collect();
        self.selected.retain(|member| live.contains(member));
        if self.anchor.is_some_and(|a| !live.contains(&a)) {
            self.anchor = None;
        }
        self.menu_row = None;
        self.refusal = None;
        // Restore against the first tree with rows; strict so it lands even in
        // a background tab.
        if let Some(row) = self.restore_scroll
            && !self.rows.is_empty()
        {
            self.restore_scroll = None;
            self.scroll.scroll_to_item_strict(row, ScrollStrategy::Top);
        }
        // Only re-scroll when the playing row moved, or a rating edit elsewhere
        // would yank the tree.
        if self.follow_playing && self.playing_row() != self.followed_row {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    fn config(&self) -> PlaylistsConfig {
        PlaylistsConfig {
            row_height: Some(self.row_height),
            head_height: Some(self.head_height),
            row_spacing: self.row_spacing,
            head_text: self.head_text,
            art_rounding: self.art_rounding,
            art_side: self.art_side,
            art_margin: self.art_margin,
            header_gap_above: self.header_gap_above,
            header_gap_below: self.header_gap_below,
            header_art: self.header_art,
            header_flush: self.header_flush,
            header_compact: self.header_compact.clone(),
            header_name_line: self.header_name_line.clone(),
            header_meta_line: self.header_meta_line.clone(),
            compact_plays: self.compact_plays,
            stripes: self.stripes,
            row_borders: self.row_borders,
            follow_playing: self.follow_playing,
            resume_playing: self.resume_playing,
            smooth_follow: self.smooth_follow,
            scroll_row: self.scroll_row(),
            ..self.config.clone()
        }
    }

    /// Nothing happened, and the line says which nothing it was.
    fn refuse(&mut self, why: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.refusal = Some(why.into());
        cx.notify();
    }

    /// Off the tree rather than the catalog: the rows came from the same read.
    fn is_smart(&self, playlist_id: i64) -> bool {
        self.rows
            .iter()
            .any(|row| matches!(row, Row::Head { id, smart, .. } if *id == playlist_id && *smart))
    }

    /// A track can sit in more than one open list, so every row holding its id
    /// is patched.
    fn patch_ratings(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) => Some(t.track_id),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        let ratings = self.state.library.read(cx).ratings_for(&ids);
        for row in &mut self.rows {
            if let Row::Track(t) = row
                && let Some(&r) = ratings.get(&t.track_id)
            {
                t.rating = r;
            }
        }
        cx.notify();
    }

    fn refresh_query(&mut self, cx: &Context<Self>) {
        self.applied_query = self.effective_query(cx);
        self.applied_filter = self.effective_filter(cx);
    }

    fn track_visible(&self, terms: &[Term], t: &PlaylistTrack) -> bool {
        t.passes(terms, &self.applied_filter, crate::settings::fold_case())
    }

    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let playing = self
            .state
            .player
            .read(cx)
            .now_playing()
            .and_then(|now| self.state.library.read(cx).id_for_key(&now.key));
        if playing != self.playing {
            self.playing = playing;
            if self.follow_playing {
                self.follow_playing(cx);
            }
            cx.notify();
        }
    }

    /// The topmost copy, when a track sits in more than one open playlist.
    fn playing_row(&self) -> Option<usize> {
        let id = self.playing?;
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Track(t) if t.track_id == id))
    }

    /// Scroll only, never the selection: chasing the player shouldn't change
    /// what Delete would drop.
    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        self.followed_row = self.playing_row();
        let Some(row) = self.followed_row else {
            return;
        };
        if self.smooth_follow {
            self.glide_to = Some(row);
        } else {
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// A no-op unless the resume is on.
    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    /// The clock only fires after a full untouched window, so no extra idle
    /// check.
    fn resume_to_playing(&mut self, cx: &mut Context<Self>) {
        if self.resume_playing {
            self.follow_playing(cx);
        }
    }

    /// A pending restore reports its target, so a panel that never painted
    /// keeps its position.
    fn scroll_row(&self) -> usize {
        if let Some(row) = self.restore_scroll {
            return row;
        }
        let offset = -self.scroll.0.borrow().base_handle.offset().y;
        if offset <= px(0.) {
            return 0;
        }
        // A dump runs outside render, where the thread-local font scale isn't
        // set; read this panel's override off its theme.
        let panel_scale = self
            .config
            .chrome
            .theme
            .font_scale
            .map(|s| s.clamp(palette::PANEL_FONT_SCALE_MIN, palette::PANEL_FONT_SCALE_MAX))
            .unwrap_or(1.0);
        let stride = (self.row_height + self.row_spacing) * palette::font_scale() * panel_scale;
        if stride <= 0. {
            return 0;
        }
        (f32::from(offset) / stride) as usize
    }

    fn row_px(&self) -> Pixels {
        palette::scaled_px(self.row_height + self.row_spacing)
    }

    /// Floored so a dense tree stays legible, the library table's rule.
    fn row_font_scale(&self) -> f32 {
        (self.row_height / ROW_HEIGHT_STOCK).clamp(0.8, 1.8)
    }

    /// Fixed by [`Row`]'s two heading variants.
    fn head_lines(&self) -> f32 {
        if self.config.headers == Headers::Expanded {
            2.
        } else {
            1.
        }
    }

    fn gap_above_px(&self) -> Pixels {
        palette::scaled_px(self.header_gap_above)
    }

    fn gap_below_px(&self) -> Pixels {
        palette::scaled_px(self.header_gap_below)
    }

    /// A `uniform_list` gives every row one height, so a heading line can't
    /// get a row of its own size. This knob only shrinks the line inside the
    /// block's rows, and the gaps come off the same room.
    fn line_px(&self) -> Pixels {
        let lines = self.head_lines();
        let room = f32::from(self.row_px()) * lines
            - f32::from(self.gap_above_px())
            - f32::from(self.gap_below_px());
        px(f32::from(palette::scaled_px(self.head_height))
            .min(room / lines)
            .max(0.))
    }

    fn tile_side(&self) -> Pixels {
        let side = f32::from(self.line_px()) * self.head_lines()
            - f32::from(palette::scaled_px(self.art_margin)) * 2.;
        px(side.max(0.))
    }

    /// The year and details switches stay on: the composed lines already hold
    /// those choices.
    fn head_look(&self) -> group_head::HeadLook {
        group_head::HeadLook {
            tile_side: self.tile_side(),
            show_art: self.header_art,
            show_year: true,
            show_details: true,
            line_px: self.line_px(),
            art_side: self.art_side,
            art_margin: palette::scaled_px(self.art_margin),
            art_rounding: self.art_rounding,
            font_scale: self.head_text / HEAD_TEXT_STOCK,
        }
    }

    fn on_search_event(
        &mut self,
        _search: &Entity<SearchBox>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => self.on_query_box_changed(cx),
            SearchEvent::FocusChanged => {
                cx.notify();
                panel::refresh_tab_panel(&self.tab_panel, cx);
            }
            SearchEvent::Dismissed => {
                window.focus(&self.focus);
                cx.notify();
                panel::refresh_tab_panel(&self.tab_panel, cx);
            }
            SearchEvent::Submitted => {}
        }
    }

    fn set_search(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.search = on;
        self.rebuild_query_view(cx);
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }

    /// Emit LayoutChanged through the host tab panel: the panel's own events
    /// never reach the dock, and a plain repaint doesn't arm the debounced
    /// save. Without this an edit only lands on a clean close.
    fn request_layout_save(&self, cx: &mut Context<Self>) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|w| w.upgrade()) {
            tabs.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
        }
    }

    fn toggle(&mut self, id: i64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.config.expanded = self.expanded.iter().copied().collect();
        self.request_layout_save(cx);
        self.refresh(cx);
    }

    fn play(&self, playlist_id: i64, start_track: Option<i64>, cx: &mut Context<Self>) {
        let (keys, start, ids) = {
            let library = self.state.library.read(cx);
            let ids = library.playlist_ids(playlist_id);
            let start = start_track
                .and_then(|t| ids.iter().position(|&x| x == t))
                .unwrap_or(0);
            (library.keys_for(&ids).unwrap_or_default(), start, ids)
        };
        if keys.is_empty() {
            return;
        }
        self.state.player.update(cx, |player, cx| {
            player.play_at(keys, start, cx);
            // After the play, never before: starting a session clears the scope.
            // Continuation then follows the playlist's order (ADR 17).
            player.set_scope(continuation::Scope::View(ids.into()));
        });
    }

    /// GPUI's save prompt has no filter list, so a typed playlist extension
    /// wins over the picked format. Only playable members go in.
    fn export(&self, playlist_id: i64, name: String, format: Format, cx: &mut Context<Self>) {
        let rows = self
            .state
            .library
            .read(cx)
            .playlist_export_rows(playlist_id);
        if rows.is_empty() {
            return;
        }
        let home = dirs::home_dir().unwrap_or_default();
        let file = format!("{name}.{}", format.extension());
        let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));
        cx.spawn(async move |_, _| {
            if let Ok(Ok(Some(path))) = rx.await {
                let format = Format::from_path(&path).unwrap_or(format);
                let text = rox_library::playlist_file::write(format, &rows);
                std::fs::write(path, text).ok();
            }
        })
        .detach();
    }

    /// The format comes off the content, not the extension. Relative paths
    /// resolve against the file's folder; unscanned paths are skipped.
    fn import(&self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                return;
            };
            let entries = rox_library::playlist_file::parse(&text);
            if entries.is_empty() {
                return;
            }
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| rox_i18n::t!("playlists-imported-fallback").to_string());
            let base = path
                .parent()
                .map(|dir| dir.to_path_buf())
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                this.state.library.update(cx, |library, cx| {
                    library.import_playlist(&name, &base, &entries, cx);
                });
            })
            .ok();
        })
        .detach();
    }

    fn member_at(&self, ix: usize) -> Option<i64> {
        match self.rows.get(ix) {
            Some(Row::Track(t)) => Some(t.member_id),
            _ => None,
        }
    }

    fn index_of(&self, member: i64) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Track(t) if t.member_id == member))
    }

    fn selected_members(&self) -> Vec<i64> {
        self.rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) if self.selected.contains(&t.member_id) => Some(t.member_id),
                _ => None,
            })
            .collect()
    }

    fn drag_members(&mut self) -> Arc<[i64]> {
        if self.drag_set.as_ref().map(|(generation, _)| *generation) != Some(self.drag_gen) {
            let members: Arc<[i64]> = self.selected_members().into();
            self.drag_set = Some((self.drag_gen, members));
        }
        self.drag_set
            .as_ref()
            .map(|(_, members)| members.clone())
            .unwrap_or_else(|| Arc::from([]))
    }

    /// Publishes the selection either way.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(member) = self.member_at(ix) else {
            return;
        };
        if modifiers.shift {
            let anchor_ix = self.anchor.and_then(|a| self.index_of(a)).unwrap_or(ix);
            let (lo, hi) = (anchor_ix.min(ix), anchor_ix.max(ix));
            // Only track rows, so a header in the span is skipped.
            let range: Vec<_> = self.rows[lo..=hi]
                .iter()
                .filter_map(|row| match row {
                    Row::Track(t) => Some(t.member_id),
                    _ => None,
                })
                .collect();
            // Ctrl+Shift stacks the range; plain shift replaces.
            if modifiers.secondary() {
                self.selected.extend(range);
            } else {
                self.selected = range.into_iter().collect();
            }
            if self.anchor.is_none() {
                self.anchor = Some(member);
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(member) {
                self.selected.remove(&member);
            }
            self.anchor = Some(member);
        } else {
            self.selected = HashSet::from([member]);
            self.anchor = Some(member);
        }
        self.drag_gen += 1;
        self.publish_selection(cx);
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let members = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) => Some(t.member_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        if members.is_empty() {
            return;
        }
        self.anchor = members.first().copied();
        self.selected = members.into_iter().collect();
        self.drag_gen += 1;
        self.publish_selection(cx);
        cx.notify();
    }

    fn publish_selection(&self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) if self.selected.contains(&t.member_id) => Some(t.track_id),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }

    fn remove_members(&mut self, members: Vec<i64>, cx: &mut Context<Self>) {
        if members.is_empty() {
            return;
        }
        // Smart rows hold a synthetic key; drop what's real and name what wasn't.
        let (members, smart): (Vec<i64>, Vec<i64>) =
            members.into_iter().partition(|&member| member > 0);
        if !smart.is_empty() {
            self.refuse(rox_i18n::t!("playlists-refuse-edit-query"), cx);
        }
        if members.is_empty() {
            return;
        }
        self.state.library.update(cx, |library, cx| {
            library.remove_playlist_members(&members, cx);
        });
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        // Keying the tree counts as browsing too.
        self.touch_resume(cx);
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if modifiers.secondary() && key == "a" {
            self.select_all(cx);
            return;
        }
        if key == "escape" {
            self.deselect(cx);
            return;
        }
        if key == "delete" || key == "backspace" {
            let members = self.selected_members();
            self.remove_members(members, cx);
        }
    }

    /// The local publish skips empty sets, so the clear goes to the selection
    /// directly.
    fn deselect(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        self.anchor = None;
        self.drag_gen += 1;
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(Vec::new(), source, cx));
        cx.notify();
    }

    /// A header means the end of its list, a track the slot before itself.
    /// An album heading is no slot. Shared by both drop paths.
    fn drop_target(&self, target: usize) -> Option<(i64, Option<i64>)> {
        match self.rows.get(target) {
            Some(Row::Head { id, .. }) => Some((*id, None)),
            Some(Row::Track(t)) => Some((t.playlist_id, Some(t.member_id))),
            Some(Row::Album(_) | Row::AlbumMeta(_)) | None => None,
        }
    }

    /// Goes in as one block before the target. Dropping onto a dragged row
    /// does nothing.
    fn drop_on(&mut self, drag: &TrackDrag, target: usize, cx: &mut Context<Self>) {
        let Some((playlist_id, before)) = self.drop_target(target) else {
            return;
        };

        if before.is_some_and(|b| drag.members.contains(&b)) {
            return;
        }
        // A smart playlist is its query's answer; refuse rather than swallow.
        if self.is_smart(playlist_id) {
            self.refuse(rox_i18n::t!("playlists-refuse-smart-source"), cx);
            return;
        }
        // A row dragged out of a smart list has no member to move.
        if drag.members.iter().any(|&member| member < 0) {
            self.refuse(rox_i18n::t!("playlists-refuse-drag-out"), cx);
            return;
        }
        let members = drag.members.clone();
        self.state.library.update(cx, |library, cx| {
            library.place_playlist_members(playlist_id, &members, before, cx);
        });
    }

    /// The drag carries library ids beside its keys; only an id-less source
    /// resolves keys, and a key with no row drops out.
    fn drop_tracks(&mut self, drag: &PlayDrag, target: usize, cx: &mut Context<Self>) {
        let Some((playlist_id, before)) = self.drop_target(target) else {
            return;
        };

        // The same refusal a member drag gets.
        if self.is_smart(playlist_id) {
            self.refuse(rox_i18n::t!("playlists-refuse-smart-source"), cx);
            return;
        }

        let ids: Vec<i64> = if drag.ids.is_empty() {
            let library = self.state.library.read(cx);
            drag.keys
                .iter()
                .filter_map(|key| library.id_for_key(key))
                .collect()
        } else {
            drag.ids.to_vec()
        };

        if ids.is_empty() {
            return;
        }

        self.state.library.update(cx, |library, cx| {
            library.add_to_playlist_at(playlist_id, &ids, before, cx);
        });
    }

    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        // Resolved once per frame and cached until the selection or rows move.
        let multi_drag = (self.selected.len() > 1).then(|| self.drag_members());
        range
            .filter_map(|ix| {
                Some(match self.rows.get(ix)? {
                    Row::Head {
                        name,
                        count,
                        expanded,
                        favourite,
                        smart,
                        ..
                    } => self.head_row(ix, name.clone(), *count, *expanded, *favourite, *smart, cx),
                    Row::Album(g) => {
                        let g = *g;
                        self.album_row(ix, g, cx)
                    }
                    Row::AlbumMeta(g) => {
                        let g = *g;
                        self.album_meta_row(ix, g, cx)
                    }
                    Row::Track(t) => {
                        let selected = self.selected.contains(&t.member_id);
                        self.track_row(ix, t, selected, multi_drag.as_ref(), cx)
                    }
                })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn head_row(
        &self,
        ix: usize,
        name: String,
        count: u64,
        expanded: bool,
        favourite: bool,
        smart: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let chevron = if expanded {
            icons::CHEVRON_DOWN
        } else {
            icons::CHEVRON_RIGHT
        };
        div()
            .id(("playlist-head", ix))
            .w_full()
            .h(self.row_px())
            .text_size(rems(self.row_font_scale()))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .when(self.row_borders, |d| {
                d.border_b_1().border_color(palette::border())
            })
            .hover(|d| d.bg(palette::bg_control_hover()))
            .drag_over::<TrackDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .drag_over::<PlayDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _, cx| {
                this.drop_on(drag, ix, cx);
            }))
            // New members from any other panel. gpui dispatches on_drop by payload
            // type, so this sits beside the TrackDrag move.
            .on_drop(cx.listener(move |this, drag: &PlayDrag, _, cx| {
                this.drop_tracks(drag, ix, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    if let Some(Row::Head { id, .. }) = this.rows.get(ix) {
                        this.toggle(*id, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.menu_row = Some(ix);
                    cx.notify();
                }),
            )
            .child(
                svg()
                    .path(chevron)
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::text_muted()),
            )
            // The heart marks the default favourites list.
            .when(favourite, |d| {
                d.child(
                    svg()
                        .path(icons::HEART_FILLED)
                        .size(px(13.))
                        .flex_none()
                        .text_color(palette::accent()),
                )
            })
            // The funnel marks a list you can't drag into.
            .when(smart, |d| {
                d.child(
                    svg()
                        .path(icons::FUNNEL)
                        .size(px(13.))
                        .flex_none()
                        .text_color(palette::text_muted()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(name)),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(rox_i18n::format::format_int(
                        count as i64,
                    ))),
            )
            // The popover trigger lets the press bubble, so swallow it or the header
            // toggles under the menu.
            .child(
                div()
                    .flex_none()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new(("playlist-export", ix))
                            .ghost()
                            .xsmall()
                            .icon(Icon::default().path(icons::UPLOAD))
                            .tooltip(rox_i18n::t!("playlists-export-tooltip"))
                            .dropdown_menu({
                                let weak = cx.entity().downgrade();
                                move |mut menu, _, _| {
                                    for format in Format::ALL {
                                        let weak = weak.clone();
                                        menu =
                                            menu.item(PopupMenuItem::new(format.label()).on_click(
                                                move |_, _, cx| {
                                                    let Some(this) = weak.upgrade() else { return };
                                                    this.update(cx, |this, cx| {
                                                        if let Some(Row::Head {
                                                            id, name, ..
                                                        }) = this.rows.get(ix)
                                                        {
                                                            let (id, name) = (*id, name.clone());
                                                            this.export(id, name, format, cx);
                                                        }
                                                    });
                                                },
                                            ));
                                    }
                                    menu
                                }
                            }),
                    ),
            )
    }

    fn album_row(&mut self, ix: usize, g: u32, cx: &mut Context<Self>) -> Stateful<Div> {
        let headers = self.config.headers;
        let expanded = headers == Headers::Expanded;
        let look = self.head_look();
        let pieces = if expanded {
            self.header_name_line.clone()
        } else {
            self.header_compact.clone()
        };
        let slot = HeadSlot {
            pieces: &pieces,
            look: &look,
            row_px: self.row_px(),
            // Content starts under the top gap; the list shows through above.
            content_top: self.gap_above_px(),
            flush: self.header_flush,
        };
        // Compact, this row carries the hairline; expanded, the meta line does.
        let border = self.row_borders && !expanded;
        track_columns::album_name_row(
            ix,
            &mut self.albums[g as usize],
            headers,
            &slot,
            &self.state,
            cx,
        )
        .when(border, |d| d.border_b_1().border_color(palette::border()))
    }

    /// Climbs back up to meet the name line, which only drew its own line
    /// height inside a possibly taller row.
    fn album_meta_row(&mut self, ix: usize, g: u32, cx: &mut Context<Self>) -> Stateful<Div> {
        let look = self.head_look();
        let pieces = self.header_meta_line.clone();
        let row_px = self.row_px();
        let slot = HeadSlot {
            pieces: &pieces,
            look: &look,
            row_px,
            content_top: self.gap_above_px() + look.line_px - row_px,
            flush: self.header_flush,
        };
        let border = self.row_borders;
        track_columns::album_meta_row(ix, &mut self.albums[g as usize], &slot, &self.state, cx)
            .when(border, |d| d.border_b_1().border_color(palette::border()))
    }

    fn track_row(
        &self,
        ix: usize,
        t: &TrackRow,
        selected: bool,
        multi_drag: Option<&Arc<[i64]>>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (playlist_id, member_id, track_id) = (t.playlist_id, t.member_id, t.track_id);
        let playing = self.playing == Some(track_id);
        let favourite = self.favourites.contains(&track_id);
        // The shared Arc from `list_rows` when inside the selection.
        let members: Arc<[i64]> = match multi_drag {
            Some(set) if selected => set.clone(),
            _ => Arc::from([member_id]),
        };
        let drag = TrackDrag {
            members,
            title: SharedString::from(t.title.clone()),
        };
        let mut row = div()
            .id(("playlist-track", ix))
            .group(track_cells::ROW_GROUP)
            .w_full()
            .h(self.row_px())
            // The cells inherit this, so the text follows the row height.
            .text_size(rems(self.row_font_scale()))
            .pl(px(28.))
            .pr(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .when(self.row_borders, |d| {
                d.border_b_1().border_color(palette::border())
            })
            // Under the selection and playing washes. Keyed on the tree index, so
            // the banding runs unbroken through the headings.
            .when(self.stripes && !ix.is_multiple_of(2), |d| {
                d.bg(palette::alpha(palette::bg_elevated(), 0x80))
            })
            .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
            .when(playing && !selected, |d| {
                d.bg(palette::alpha(palette::highlight(), 0x12))
            })
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_drag(drag, |drag, _pos, _window, cx| {
                cx.new(|_| TrackDragPreview {
                    title: drag.title.clone(),
                    extra: drag.members.len().saturating_sub(1),
                })
            })
            .drag_over::<TrackDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .drag_over::<PlayDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _, cx| {
                this.drop_on(drag, ix, cx);
            }))
            .on_drop(cx.listener(move |this, drag: &PlayDrag, _, cx| {
                this.drop_tracks(drag, ix, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    if event.click_count > 1 {
                        this.play(playlist_id, Some(track_id), cx);
                    } else if event.modifiers.shift || event.modifiers.secondary() {
                        this.select(ix, event.modifiers, cx);
                    } else if !this.selected.contains(&member_id) {
                        // A press on an unselected row picks it now so a drag takes it; on a lit
                        // row the collapse waits for the click.
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                // A click that never became a drag collapses the selection to this row.
                let mods = event.modifiers();
                if event.click_count() == 1
                    && !mods.shift
                    && !mods.secondary()
                    && this.selected.len() > 1
                    && this.selected.contains(&member_id)
                {
                    this.select(ix, Modifiers::default(), cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.menu_row = Some(ix);
                    // A right click outside the set reselects just that row.
                    if !this.selected.contains(&member_id) {
                        this.select(ix, Modifiers::default(), cx);
                    }
                    cx.notify();
                }),
            );
        // The shared surface draws every playlist column.
        let cover = track_columns::cover_thumb(
            &self.state,
            (!t.path.is_empty()).then(|| std::path::Path::new(&t.path)),
            self.column_shown("cover"),
            cx,
        );
        let cell = track_columns::Cell {
            pos: t.pos,
            title: &t.title,
            artist: &t.artist,
            album: &t.album,
            title_reading: &t.title_reading,
            artist_reading: &t.artist_reading,
            album_reading: &t.album_reading,
            year: t.year,
            genre: &t.genre,
            duration_ms: t.duration_ms,
            rating: t.rating,
            track_id,
            favourite,
            playing,
            plays: t.plays,
            cover,
        };
        for col in columns() {
            if !self.column_shown(col.key) {
                continue;
            }
            if let Some(c) = track_columns::cell(
                col.key,
                &cell,
                &self.state,
                self.row_height,
                self.compact_plays,
            ) {
                row = row.child(c);
            }
        }
        row
    }

    /// No rebuild: a look knob never moves the rows.
    fn restyle(&mut self, cx: &mut Context<Self>) {
        self.request_layout_save(cx);
        cx.notify();
    }

    fn rows_section(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (row_height, row_spacing) = (self.row_height, self.row_spacing);
        settings_ui::section(
            rox_i18n::t!("library-section-rows"),
            None,
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(panel::setting_row(
                    rox_i18n::t!("library-row-height"),
                    Some(rox_i18n::t!("library-row-height.description")),
                    settings_ui::scalar(
                        &self.row_scrub,
                        &self.value_edit,
                        row_height,
                        settings_ui::span(ROW_HEIGHT_MIN, ROW_HEIGHT_MAX, " px"),
                        |this: &mut Self, value, cx| {
                            this.row_height = value;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-row-spacing"),
                    Some(rox_i18n::t!("library-row-spacing.description")),
                    settings_ui::scalar(
                        &self.row_spacing_scrub,
                        &self.value_edit,
                        row_spacing,
                        settings_ui::span(0., ROW_SPACING_MAX, " px"),
                        |this: &mut Self, value, cx| {
                            this.row_spacing = value;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-stripes"),
                    Some(rox_i18n::t!("library-stripes.description")),
                    panel::toggle(
                        self.stripes,
                        |this: &mut Self, on, cx| {
                            this.stripes = on;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-row-borders"),
                    Some(rox_i18n::t!("library-row-borders.description")),
                    panel::toggle(
                        self.row_borders,
                        |this: &mut Self, on, cx| {
                            this.row_borders = on;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-compact-plays"),
                    Some(rox_i18n::t!("library-compact-plays.description")),
                    panel::toggle(
                        self.compact_plays,
                        |this: &mut Self, on, cx| {
                            this.compact_plays = on;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                )),
        )
        .into_any_element()
    }

    /// One well per line rather than the library's add-a-line rows: the
    /// tree's heading is [`Row::Album`] and [`Row::AlbumMeta`] and nothing else.
    fn headings_section(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let expanded = self.config.headers == Headers::Expanded;
        let (head_height, head_text) = (self.head_height, self.head_text);
        let (gap_above, gap_below) = (self.header_gap_above, self.header_gap_below);
        settings_ui::section(
            rox_i18n::t!("library-headers"),
            None,
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(panel::setting_row(
                    rox_i18n::t!("library-line-height"),
                    Some(rox_i18n::t!("playlists-line-height-description")),
                    settings_ui::scalar(
                        &self.head_scrub,
                        &self.value_edit,
                        head_height,
                        settings_ui::span(ROW_HEIGHT_MIN, HEAD_HEIGHT_MAX, " px"),
                        |this: &mut Self, value, cx| {
                            this.head_height = value;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-text-size"),
                    Some(rox_i18n::t!("library-text-size.description")),
                    settings_ui::scalar(
                        &self.head_text_scrub,
                        &self.value_edit,
                        head_text,
                        settings_ui::span(HEAD_TEXT_MIN, HEAD_TEXT_MAX, " px"),
                        |this: &mut Self, value, cx| {
                            this.head_text = value;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-flush-background"),
                    Some(rox_i18n::t!("library-flush-background.description")),
                    panel::toggle(
                        self.header_flush,
                        |this: &mut Self, on, cx| {
                            this.header_flush = on;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-gap-above"),
                    Some(rox_i18n::t!("library-gap-above.description")),
                    settings_ui::scalar(
                        &self.header_gap_above_scrub,
                        &self.value_edit,
                        gap_above,
                        settings_ui::span(0., HEAD_GAP_MAX, " px"),
                        |this: &mut Self, value, cx| {
                            this.header_gap_above = value;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-gap-below"),
                    Some(rox_i18n::t!("library-gap-below.description")),
                    settings_ui::scalar(
                        &self.header_gap_below_scrub,
                        &self.value_edit,
                        gap_below,
                        settings_ui::span(0., HEAD_GAP_MAX, " px"),
                        |this: &mut Self, value, cx| {
                            this.header_gap_below = value;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                // Only the wells the active mode paints.
                .when(!expanded, |d| {
                    d.child(panel::setting_block(
                        rox_i18n::t!("library-header-row"),
                        Some(rox_i18n::t!("library-header-row.description")),
                        None,
                        panel::arrange_editor(
                            "playlists-head-compact",
                            group_head::PIECES,
                            &self.header_compact,
                            |this: &mut Self, items, cx| {
                                this.header_compact = items;
                                this.restyle(cx);
                            },
                            cx,
                        ),
                    ))
                })
                .when(expanded, |d| {
                    d.child(panel::setting_block(
                        rox_i18n::t!("playlists-name-line"),
                        Some(rox_i18n::t!("playlists-name-line-description")),
                        None,
                        panel::arrange_editor(
                            "playlists-head-name",
                            group_head::PIECES,
                            &self.header_name_line,
                            |this: &mut Self, items, cx| {
                                this.header_name_line = items;
                                this.restyle(cx);
                            },
                            cx,
                        ),
                    ))
                    .child(panel::setting_block(
                        rox_i18n::t!("playlists-meta-line"),
                        Some(rox_i18n::t!("playlists-meta-line-description")),
                        None,
                        panel::arrange_editor(
                            "playlists-head-meta",
                            group_head::PIECES,
                            &self.header_meta_line,
                            |this: &mut Self, items, cx| {
                                this.header_meta_line = items;
                                this.restyle(cx);
                            },
                            cx,
                        ),
                    ))
                }),
        )
        .into_any_element()
    }

    fn art_section(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (rounding, margin) = (self.art_rounding, self.art_margin);
        settings_ui::section(
            rox_i18n::t!("head-piece-art"),
            None,
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(panel::setting_row(
                    rox_i18n::t!("head-piece-art"),
                    Some(rox_i18n::t!("playlists-art-description")),
                    panel::toggle(
                        self.header_art,
                        |this: &mut Self, on, cx| {
                            this.header_art = on;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-art-rounding"),
                    Some(rox_i18n::t!("library-art-rounding.description")),
                    settings_ui::scalar(
                        &self.art_scrub,
                        &self.value_edit,
                        rounding,
                        settings_ui::span(0., ART_ROUNDING_MAX, " px"),
                        |this: &mut Self, value, cx| {
                            this.art_rounding = value;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-art-position"),
                    Some(rox_i18n::t!("library-art-position.description")),
                    panel::choices_shared(
                        &[
                            (rox_i18n::t!("side-left"), ArtSide::Left),
                            (rox_i18n::t!("side-right"), ArtSide::Right),
                        ],
                        self.art_side,
                        |this: &mut Self, side, cx| {
                            this.art_side = side;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-art-margin"),
                    Some(rox_i18n::t!("library-art-margin.description")),
                    settings_ui::scalar(
                        &self.art_margin_scrub,
                        &self.value_edit,
                        margin,
                        settings_ui::span(0., ART_MARGIN_MAX, " px"),
                        |this: &mut Self, value, cx| {
                            this.art_margin = value;
                            this.restyle(cx);
                        },
                        cx,
                    ),
                )),
        )
        .into_any_element()
    }

    fn new_playlist_item(&self, menu: PopupMenu) -> PopupMenu {
        let state = self.state.clone();
        let smart_state = self.state.clone();
        menu.item(
            PopupMenuItem::new(rox_i18n::t!("playlists-new"))
                .icon(Icon::default().path(icons::PLUS))
                .on_click(move |_, _, cx| {
                    rox_panel_api::openers::playlist_create(state.clone(), Vec::new(), cx);
                }),
        )
        .item(
            PopupMenuItem::new(rox_i18n::t!("playlists-new-smart"))
                .icon(Icon::default().path(icons::FUNNEL))
                .on_click(move |_, _, cx| {
                    rox_panel_api::openers::smart_playlist(smart_state.clone(), None, cx);
                }),
        )
    }
}

impl ColumnHost for PlaylistsPanel {
    fn column_shown(&self, key: &str) -> bool {
        self.config.columns.iter().any(|k| k == key)
    }

    fn set_column(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>) {
        let has = self.column_shown(key);
        if on && !has {
            self.config.columns.push(key.to_string());
        } else if !on {
            self.config.columns.retain(|k| k != key);
        }
        self.request_layout_save(cx);
        cx.notify();
    }
}

impl HeadingHost for PlaylistsPanel {
    fn headers(&self) -> Headers {
        self.config.headers
    }

    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>) {
        if self.config.headers == headers {
            return;
        }
        self.config.headers = headers;
        self.request_layout_save(cx);
        self.refresh(cx);
    }
}

impl QueryFilter for PlaylistsPanel {
    fn shared_query(&self) -> &Entity<crate::query::shared_query::SharedQuery> {
        &self.state.query
    }
    fn query_box(&self) -> &Entity<SearchBox> {
        &self.search
    }
    fn query_source(&self) -> QuerySource {
        self.config.query_source
    }
    fn set_query_source_value(&mut self, source: QuerySource) {
        self.config.query_source = source;
    }
    fn local_query(&self) -> String {
        self.config.query.clone()
    }
    fn set_local_query(&mut self, query: String) {
        self.config.query = query;
    }
    fn query_box_shown(&self) -> bool {
        self.config.search
    }
    fn set_query_box_shown(&mut self, shown: bool) {
        self.config.search = shown;
    }
    fn rebuild_query_view(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
    }
    fn set_query_resync(&mut self, pending: bool) {
        self.resync_box = pending;
    }
    fn selection(&self) -> &Entity<crate::selection::Selection> {
        &self.state.selection
    }
    fn selection_ids(&self) -> &[i64] {
        &self.selection_ids
    }
    fn set_selection_ids(&mut self, ids: Vec<i64>) {
        self.selection_ids = ids;
    }
    fn after_query_change(&mut self, cx: &mut Context<Self>) {
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }
}

impl PanelSettings for PlaylistsPanel {
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
        &[("View", icons::ROWS_3)]
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
            .child(panel::setting_block(
                rox_i18n::t!("library-columns"),
                Some(rox_i18n::t!("playlists-columns")),
                None,
                track_columns::checklist(&columns(), self, cx),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("panel-headings"),
                Some(rox_i18n::t!("playlists-headings")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("headers-off"), Headers::Off),
                        (rox_i18n::t!("headers-compact"), Headers::Compact),
                        (rox_i18n::t!("headers-expanded"), Headers::Expanded),
                    ],
                    self.config.headers,
                    |this: &mut Self, headers, cx| this.set_headers(headers, cx),
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn behavior(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(crate::query::shared_query::search_section(
                    self.config.search,
                    |this: &mut Self, on, cx| this.set_search(on, cx),
                    self.config.query_source,
                    |this: &mut Self, source, cx| this.pick_query_source(source, cx),
                    cx,
                ))
                .child(panel::tracking_section(
                    self.follow_playing,
                    rox_i18n::t!("library-follow-description"),
                    |this: &mut Self, on, cx| {
                        this.follow_playing = on;
                        // Catch up right away.
                        if on {
                            this.follow_playing(cx);
                        }
                        this.restyle(cx);
                    },
                    self.resume_playing,
                    rox_i18n::t!("library-resume-description"),
                    |this: &mut Self, on, cx| {
                        this.resume_playing = on;
                        this.restyle(cx);
                    },
                    self.smooth_follow,
                    rox_i18n::t!("library-smooth-description"),
                    |this: &mut Self, on, cx| {
                        this.smooth_follow = on;
                        this.restyle(cx);
                    },
                    cx,
                ))
                .into_any_element(),
        )
    }

    /// On the config because they shape the content, not the frame.
    fn appearance(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let headings = self.config.headers != Headers::Off;
        let rows = self.rows_section(cx);
        // The heading look only matters while headings show.
        let heading_rows = headings.then(|| self.headings_section(cx));
        let art = self.art_section(cx);
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(rows)
                .children(heading_rows)
                .child(art)
                .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for PlaylistsPanel {}

impl Focusable for PlaylistsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for PlaylistsPanel {
    fn panel_name(&self) -> &'static str {
        "playlists"
    }

    rox_panel_api::opens_settings!();

    /// A drop that misses every row is a no-op: empty space names no playlist.
    fn accepts_drop(&self, cx: &App) -> bool {
        cx.active_drag_is::<PlayDrag>()
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("playlists-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn title_suffix(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.config.search {
            return None;
        }
        Some(
            self.search
                .update(cx, |search, cx| search.element(cx))
                .w(px(180.)),
        )
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
            serde_json::to_value(self.config()).unwrap_or(serde_json::Value::Null),
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
        let menu = self.new_playlist_item(menu);

        // Also reachable here, since the tab bar isn't drawn in every placement.
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("playlists-import"))
                .icon(Icon::default().path(icons::DOWNLOAD))
                .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
        );

        let menu = menu.separator().label(rox_i18n::t!("panel-menu-display"));
        let columns_menu = track_columns::columns_submenu(columns(), window, cx);
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("library-columns"),
            columns_menu,
        ));
        let headings = track_columns::headings_submenu(window, cx);
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("panel-headings"),
            headings,
        ));
        let menu = crate::query::shared_query::search_flyout(
            menu,
            |this: &Self| this.config.query_source,
            |this: &Self| this.config.search,
            &cx.entity(),
            |this: &mut Self, source, cx| this.pick_query_source(source, cx),
            |this: &mut Self, on, cx| this.set_search(on, cx),
            window,
            cx,
        );

        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config())
                };
                PlaylistsPanel::new(state, config, window, cx)
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

    /// Import is panel-level, unlike per-playlist export.
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("import-playlist")
                .icon(Icon::default().path(icons::DOWNLOAD))
                .tooltip(rox_i18n::t!("playlists-import-tooltip"))
                .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
        ])
    }
}

impl Render for PlaylistsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl PlaylistsPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        // The follow glide, stepped one frame at a time. Every row is one stride
        // tall, so the target is the index times the stride.
        let dt = self.glide_tick.elapsed().as_secs_f32().min(0.05);
        self.glide_tick = Instant::now();
        if let Some(row) = self.glide_to {
            let handle = self.scroll.0.borrow().base_handle.clone();
            let stride = self.row_px();
            let target =
                panel::glide_target_at(&handle, gpui::Axis::Vertical, stride * row as f32, stride);
            match target {
                // A rebuild can strand the target past the end; drop the glide.
                _ if row >= self.rows.len() => self.glide_to = None,
                Some(target)
                    if !panel::glide_step_axis(&handle, gpui::Axis::Vertical, target, dt) =>
                {
                    self.glide_to = None
                }
                _ => window.request_animation_frame(),
            }
        }
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)))
            // Only restarts the idle clock; the rows underneath handle the event.
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                this.touch_resume(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.touch_resume(cx)),
            );
        let searching = !self.applied_query.is_empty() || !self.applied_filter.is_empty();
        let content = if self.rows.is_empty() {
            let message = if searching {
                rox_i18n::t!("picker-no-matches")
            } else {
                rox_i18n::t!("playlists-empty")
            };
            div().flex_1().min_h_0().flex().flex_col().child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(tokens::SPACE_SM)
                    .p(tokens::SPACE_MD)
                    .text_center()
                    .text_color(palette::text_faint())
                    .child(message)
                    // The tab bar isn't on screen in every placement.
                    .when(!searching, |empty| {
                        empty.child(crate::settings::ui::small_button(
                            rox_i18n::t!("playlists-import"),
                            icons::DOWNLOAD,
                            false,
                            cx.listener(|this, _, window, cx| this.import(window, cx)),
                        ))
                    }),
            )
        } else {
            let this = cx.entity().downgrade();
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    uniform_list("playlist-rows", self.rows.len(), move |range, _, cx| {
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
            this.update(cx, |this, cx| this.row_menu(menu, window, cx))
        }))
        .children(self.refusal.clone().map(|why| {
            div()
                .flex_none()
                .px(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .border_t_1()
                .border_color(palette::border())
                .text_xs()
                .text_color(palette::text_muted())
                .child(why)
        }))
    }

    fn row_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let Some(ix) = self.menu_row else {
            return self.dropdown_menu(menu, window, cx);
        };
        let weak = cx.entity().downgrade();
        match self.rows.get(ix) {
            Some(Row::Track(t)) => {
                let (playlist_id, member_id, track_id) = (t.playlist_id, t.member_id, t.track_id);
                let smart = t.smart;
                let play_panel = weak.clone();
                let menu = panel::track_actions(
                    menu,
                    self.state.clone(),
                    vec![track_id],
                    rox_i18n::t!("library-play"),
                    window,
                    cx,
                    move |_, cx| {
                        if let Some(this) = play_panel.upgrade() {
                            this.update(cx, |this, cx| this.play(playlist_id, Some(track_id), cx));
                        }
                    },
                );
                let remove_panel = weak.clone();
                // The right press already pulled the row into the selection.
                let remove_count = if self.selected.contains(&member_id) && self.selected.len() > 1
                {
                    self.selected.len()
                } else {
                    1
                };
                let remove_label =
                    rox_i18n::t!("playlists-remove", count = remove_count as u64).to_string();
                let menu = menu.item(
                    PopupMenuItem::new(remove_label)
                        .icon(Icon::default().path(icons::CLOSE))
                        // A smart row is here because the query says so; there's no member to
                        // remove.
                        .disabled(smart)
                        .on_click(move |_, _, cx| {
                            if let Some(this) = remove_panel.upgrade() {
                                this.update(cx, |this, cx| {
                                    let members = if this.selected.contains(&member_id) {
                                        this.selected_members()
                                    } else {
                                        vec![member_id]
                                    };
                                    this.remove_members(members, cx);
                                });
                            }
                        }),
                );
                self.dropdown_menu(menu.separator(), window, cx)
            }
            Some(Row::Head {
                id,
                name,
                favourite,
                smart,
                ..
            }) => {
                let (id, name, favourite, smart) = (*id, name.clone(), *favourite, *smart);
                let play_panel = weak.clone();
                let menu = menu.item(
                    PopupMenuItem::new(rox_i18n::t!("library-play"))
                        .icon(Icon::default().path(icons::PLAY))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = play_panel.upgrade() {
                                this.update(cx, |this, cx| this.play(id, None, cx));
                            }
                        }),
                );
                // Editing the query is the only way to change a smart list.
                let query_state = self.state.clone();
                let menu = menu.when(smart, |menu| {
                    menu.item(
                        PopupMenuItem::new(rox_i18n::t!("playlists-edit-query"))
                            .icon(Icon::default().path(icons::FUNNEL))
                            .on_click(move |_, _, cx| {
                                rox_panel_api::openers::smart_playlist(
                                    query_state.clone(),
                                    Some(id),
                                    cx,
                                );
                            }),
                    )
                });
                // The favourites playlist is the one default: no rename, no delete.
                let rename_state = self.state.clone();
                let menu = menu.when(!favourite, |menu| {
                    menu.item(
                        PopupMenuItem::new(rox_i18n::t!("playlists-rename"))
                            .icon(Icon::default().path(icons::PENCIL))
                            .on_click(move |_, _, cx| {
                                rox_panel_api::openers::playlist_rename(
                                    rename_state.clone(),
                                    id,
                                    name.clone(),
                                    cx,
                                );
                            }),
                    )
                });
                let delete_panel = weak.clone();
                let menu = menu.when(!favourite, |menu| {
                    menu.item(
                        PopupMenuItem::new(rox_i18n::t!("playlists-delete"))
                            .icon(Icon::default().path(icons::TRASH))
                            .on_click(move |_, _, cx| {
                                if let Some(this) = delete_panel.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.state.library.update(cx, |library, cx| {
                                            library.delete_playlist(id, cx);
                                        });
                                    });
                                }
                            }),
                    )
                });
                self.dropdown_menu(menu.separator(), window, cx)
            }
            // Headings never set a menu row, but keep the match total.
            Some(Row::Album(_) | Row::AlbumMeta(_)) | None => self.dropdown_menu(menu, window, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PlaylistsConfig, fold_head_lines};
    use crate::group_head::{self, HeadPiece};

    /// A saved line is deduped: a hand-edited dump naming a piece twice
    /// would draw it twice.
    #[test]
    fn empty_lines_fall_back_to_stock_and_saved_ones_dedupe() {
        let config = PlaylistsConfig::default();
        let (compact, name, meta) = fold_head_lines(&config);
        assert!(compact == group_head::stock_compact());
        assert!(name == group_head::stock_name_line());
        assert!(meta == group_head::stock_meta_line());

        let config: PlaylistsConfig = serde_json::from_str(
            r#"{"header_name_line": ["artist", "spacer", "year", "artist"],
                "header_meta_line": ["album"]}"#,
        )
        .unwrap();
        let (compact, name, meta) = fold_head_lines(&config);
        assert!(compact == group_head::stock_compact());
        assert!(name == vec![HeadPiece::Artist, HeadPiece::Spacer, HeadPiece::Year]);
        assert!(meta == vec![HeadPiece::Album]);
    }

    #[test]
    fn composed_lines_round_trip() {
        let config: PlaylistsConfig =
            serde_json::from_str(r#"{"header_compact": ["album", "spacer", "time"]}"#).unwrap();
        let saved = serde_json::to_value(&config).unwrap();
        let back: PlaylistsConfig = serde_json::from_value(saved).unwrap();
        assert!(back.header_compact == config.header_compact);
        assert!(fold_head_lines(&back) == fold_head_lines(&config));
    }
}
