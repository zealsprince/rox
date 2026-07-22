//! The dockable library panel that browses the shared catalog entity (which
//! lives in `crate::catalog`). The catalog owns the app's library database and
//! only ever hands out the in-memory projection, per the library service
//! boundary. Panels are views over the shared catalog with their own
//! search config, so a duplicated panel filters independently. Double
//! clicking a track queues it straight on the shared player; single clicks
//! select, and the selection publishes app-wide for panels that display it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    div, prelude::*, px, AnyElement, App, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, MouseButton,
    ScrollStrategy, ScrollWheelEvent, SharedString, Stateful, Subscription, WeakEntity,
    Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState};
use gpui_component::{Icon, Side, Sizable};
use rox_dock::{Panel, PanelEvent, PanelInfo, PanelState, TabPanel};

use rox_library::projection::Projection;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::group_head::{self, Headers};
use crate::panel::{self, AppState, PanelChrome, ResumeIdle, ScrubState};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::settings::ui as settings_ui;
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::thumbs::Thumb;
use crate::track_ui::track_cells;
use crate::track_ui::track_drag::{PlayDrag, PlayDragPreview};

/// Play from a double-clicked row: at most this many tracks are queued
/// behind it. The quick-play modal caps its queue the same way.
pub(crate) const QUEUE_CAP: usize = 1000;

/// The header tiles' rounding knob ceiling, the panel frame sliders'
/// scale.
const ART_ROUNDING_MAX: f32 = 24.;

/// How far page up and page down step the keyboard cursor.
const PAGE_ROWS: isize = 25;

pub(crate) use crate::catalog::{Library, LibraryEvent};


mod columns;

pub(crate) use columns::LibraryConfig;
use columns::*;

/// One display row of the track list: a track from the projection, or a
/// line of the group header opening the artist/album run that follows it.
/// Headers are presentation of the canonical order only - search hits and
/// column sorts render flat - and they live in the same index space as
/// tracks, so the virtualized table scrolls them like any row. The table
/// draws every row one fixed height, so the expanded header block is two
/// rows, each drawing its own line.
#[derive(Clone, Copy, PartialEq)]
enum Row {
    Track(u32),
    /// The group's name line, indexing [`TrackTable::groups`].
    Header(u32),
    /// The group's album and stats line under an expanded header.
    Meta(u32),
    /// The divider opening one disc's run inside a multi-disc group.
    Disc(u16),
}

/// One group of the current view: what its header rows draw. The name,
/// year, and genre resolve through the first track.
struct Group {
    first: u32,
    tracks: u32,
    total_ms: u64,
    /// The group's codec symbol while every track agrees; None once two
    /// differ, and the meta line drops it.
    codec: Option<u32>,
    /// The bitrate spread over tracks that carry one, in kbps; both 0 when
    /// none does.
    min_kbps: u16,
    max_kbps: u16,
    /// The first track's path, what the header tile's thumbnail loads by.
    /// Resolved through the store once, on the group's first paint; the
    /// inner None is a track the store no longer knows.
    art: Option<Option<PathBuf>>,
}

/// The given order with a header block opening every group run: one row
/// compact, name and stats rows expanded. The order must keep each
/// group's rows contiguous (the caller re-sorts for genre and year).
/// Album groups break on the album artist, not the track artist, so a
/// compilation stays one run with its per-track artists inside, and a
/// group spanning discs gets a divider row opening each numbered disc's
/// run; untagged tracks (disc 0) sit under the header undivided. Breaks
/// compare interned symbols (years their raw value) and the stats are
/// two integer sums, so the walk stays cheap and runs once per view
/// swap, never while scrolling.
fn group_rows(
    order: &[u32],
    projection: &Projection,
    expanded: bool,
    group_by: GroupBy,
) -> (Vec<Row>, Vec<Group>) {
    let mut rows = Vec::with_capacity(order.len() + order.len() / 8);
    let mut groups: Vec<Group> = Vec::new();
    let key = |row: u32| -> u64 {
        let i = row as usize;
        match group_by {
            GroupBy::Album => {
                (projection.album_artist[i] as u64) << 32 | projection.album[i] as u64
            }
            GroupBy::Artist => projection.album_artist[i] as u64,
            GroupBy::Genre => projection.genre[i] as u64,
            GroupBy::Year => projection.year[i] as u64,
        }
    };
    let mut i = 0;
    while i < order.len() {
        // One album run: the canonical order keeps a group contiguous, so
        // its extent is known before any of its rows are pushed, which is
        // what lets the first disc get its divider too.
        let mut j = i + 1;
        while j < order.len() && key(order[j]) == key(order[i]) {
            j += 1;
        }
        let run = &order[i..j];
        i = j;

        let g = groups.len() as u32;
        groups.push(Group {
            first: run[0],
            tracks: 0,
            total_ms: 0,
            codec: Some(projection.codec[run[0] as usize]),
            min_kbps: 0,
            max_kbps: 0,
            art: None,
        });
        rows.push(Row::Header(g));
        if expanded {
            rows.push(Row::Meta(g));
        }
        let disc = |row: u32| projection.disc_no[row as usize];
        let multi_disc =
            group_by == GroupBy::Album && run.iter().any(|&row| disc(row) != disc(run[0]));
        let mut last_disc = None;
        for &row in run {
            if multi_disc && disc(row) > 0 && last_disc != Some(disc(row)) {
                rows.push(Row::Disc(disc(row)));
                last_disc = Some(disc(row));
            }
            let group = groups.last_mut().unwrap();
            group.tracks += 1;
            group.total_ms += projection.duration_ms[row as usize] as u64;
            if group.codec != Some(projection.codec[row as usize]) {
                group.codec = None;
            }
            let kbps = projection.bitrate_kbps[row as usize];
            if kbps > 0 {
                group.min_kbps = if group.min_kbps == 0 {
                    kbps
                } else {
                    group.min_kbps.min(kbps)
                };
                group.max_kbps = group.max_kbps.max(kbps);
            }
            rows.push(Row::Track(row));
        }
    }
    (rows, groups)
}

/// A group's codec and bitrate stat, resolving the interned codec symbol
/// before handing off to the shared [`group_head::quality`].
fn group_quality(group: &Group, projection: &Projection) -> String {
    let codec = group
        .codec
        .map(|sym| projection.codecs.strings[sym as usize].as_str());
    group_head::quality(codec, group.min_kbps, group.max_kbps)
}

/// The table delegate: the column set and the rows one panel displays.
/// Lives inside the panel's `TableState`; the panel swaps `view` when the
/// query or the catalog changes.
struct TrackTable {
    state: AppState,
    /// The owning panel, for dispatching context menu actions back to it.
    panel: WeakEntity<LibraryPanel>,
    /// Rows currently displayed: the canonical order broken by group
    /// headers, or flat search hits, re-sorted when a column sort is
    /// active.
    view: Arc<Vec<Row>>,
    /// The current view's groups, what header rows index; empty when the
    /// view renders flat. Swapped together with `view`, always.
    groups: Vec<Group>,
    /// How the canonical order breaks into groups, and on what field.
    /// Mirrored from the panel like the density: the view computation
    /// and the header render read them here, the knobs live on the
    /// panel.
    headers: Headers,
    group_by: GroupBy,
    /// The panel's row height, mirrored here because the header tile is
    /// sized in rows and the widget's size lives outside the delegate.
    density: Density,
    /// The header tiles' corner radius, mirrored from the panel like the
    /// density: the tile renders here, the knob lives on the panel.
    art_rounding: f32,
    /// The header rows' look knobs, mirrored from the panel the same
    /// way: the cover tile, the year, and the meta line's details.
    header_art: bool,
    header_year: bool,
    header_details: bool,
    /// Selected rows as indices into `view`, track rows only - headers
    /// take no selection. Cleared when the view swaps, since the indices
    /// point elsewhere afterwards.
    selected: HashSet<usize>,
    /// Where the next shift-click extends from: the last plain or
    /// toggle-clicked row.
    anchor: Option<usize>,
    /// The keyboard cursor: where arrows move from and enter plays from.
    /// Follows clicks, so keys and mouse hand off mid-browse.
    cursor: Option<usize>,
    columns: Vec<Column>,
    /// The active sort: a column key and whether it descends. None is the
    /// canonical order. Lives on the delegate because the header click
    /// lands here; the panel reads it back for the layout dump.
    sort: Option<(SharedString, bool)>,
    /// The playing track's id, resolved once per track change by the
    /// panel, and its row in the current view when the view holds it.
    playing_id: Option<i64>,
    playing_row: Option<usize>,
    /// The favourited track ids, what the heart column checks each row
    /// against. Refreshed off the library on a playlist change, so a toggle
    /// anywhere lights the same track here without a full view rebuild.
    favourites: HashSet<i64>,
    /// Resolved file paths for the cover column, cached per track id on the
    /// cell's first paint so the thumbnail lookup does not re-query the
    /// catalog every frame. Paths are stable per id; cleared on reload.
    cover_paths: HashMap<i64, Option<PathBuf>>,
    /// Resolved file paths for the drag payload, cached per track id. A row's
    /// `on_drag` value is built eagerly every frame, so the id-to-path query
    /// caches here or a scrolled list would hit the catalog per row per frame.
    /// Same lifetime as `cover_paths`; cleared on reload.
    drag_paths: HashMap<i64, Option<PathBuf>>,
    /// Bumped on every selection change. Keys the drag-set cache below so it
    /// rebuilds only when the selection actually moves, not per frame. A view
    /// swap always clears the selection, so this catches those too.
    sel_gen: u64,
    /// The wall clock the "added" column dates against, refreshed at most every
    /// half minute instead of a `SystemTime::now` per shown cell per frame;
    /// relative-time granularity is coarse enough that the small lag is unseen.
    added_now: i64,
    added_now_at: Instant,
    /// The multi-selection drag paths, in view order, built once per selection
    /// change and shared behind an Arc. A grab inside the selection hands every
    /// visible selected row this same Arc instead of rebuilding the whole set
    /// per row per frame.
    drag_set: Option<(u64, Arc<[PathBuf]>)>,
}

impl TrackTable {
    /// The current unix time the "added" column dates against, refreshed at
    /// most twice a minute so a wall of shown cells shares one read instead of
    /// each calling `SystemTime::now`.
    fn added_now(&mut self) -> i64 {
        if self.added_now_at.elapsed() >= Duration::from_secs(30) {
            self.added_now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(self.added_now);
            self.added_now_at = Instant::now();
        }
        self.added_now
    }

    /// The track a view row holds; None for a header row.
    fn track_at(&self, ix: usize) -> Option<u32> {
        match self.view.get(ix) {
            Some(&Row::Track(row)) => Some(row),
            _ => None,
        }
    }

    /// The drag payload for a grab on row `ix`. A grab inside a multi
    /// selection carries the whole set in view order; outside it, just that
    /// row - queue.rs's rule. Resolves to paths through the same `paths_for`
    /// the play actions use, so a drop enqueues exactly what those queue, ids
    /// aligned per path for the drop target that wants them. The value is
    /// built eagerly every frame, so paths come from `drag_paths`, filled per
    /// id on the first grab that needs it rather than a query per row per frame.
    fn drag_payload(&mut self, ix: usize, cx: &App) -> Option<PlayDrag> {
        let projection = self.state.library.read(cx).projection().cloned()?;
        let title = self
            .track_at(ix)
            .map(|row| projection.resolve(row).title.to_string())
            .unwrap_or_default();
        // A grab inside a multi-selection carries the whole set in view order,
        // built once per selection change and shared behind an Arc so it costs
        // a refcount bump per row, not a rebuild. Outside it, just this row.
        let paths: Arc<[PathBuf]> = if self.selected.len() > 1 && self.selected.contains(&ix) {
            if self.drag_set.as_ref().map(|(gen, _)| *gen) != Some(self.sel_gen) {
                let mut rows: Vec<usize> = self.selected.iter().copied().collect();
                rows.sort_unstable();
                let set: Arc<[PathBuf]> = self.resolve_drag_paths(&rows, &projection, cx).into();
                self.drag_set = Some((self.sel_gen, set));
            }
            self.drag_set.as_ref().map(|(_, set)| set.clone())?
        } else {
            self.resolve_drag_paths(&[ix], &projection, cx).into()
        };
        if paths.is_empty() {
            return None;
        }
        Some(PlayDrag {
            paths,
            title: title.into(),
        })
    }

    /// Resolve view rows to their files in row order, through the same per-id
    /// path cache the cover column fills, so a drag never re-queries the
    /// catalog once a track's path is known.
    fn resolve_drag_paths(&mut self, rows: &[usize], projection: &Projection, cx: &App) -> Vec<PathBuf> {
        let ids: Vec<i64> = rows
            .iter()
            .filter_map(|&i| self.track_at(i))
            .map(|row| projection.db_id[row as usize])
            .collect();
        let mut paths = Vec::with_capacity(ids.len());
        for id in ids {
            let path = match self.drag_paths.get(&id) {
                Some(path) => path.clone(),
                None => {
                    let path = self
                        .state
                        .library
                        .read(cx)
                        .paths_for(&[id])
                        .ok()
                        .and_then(|mut paths| paths.pop());
                    self.drag_paths.insert(id, path.clone());
                    path
                }
            };
            if let Some(path) = path {
                paths.push(path);
            }
        }
        paths
    }

    /// The nearest track row from `ix` heading `forward`, bouncing off the
    /// ends; None only when the view holds no tracks. Cursor moves route
    /// through this, so the cursor never lands on a header.
    fn snap_to_track(&self, ix: usize, forward: bool) -> Option<usize> {
        let len = self.view.len();
        if len == 0 {
            return None;
        }
        let ix = ix.min(len - 1);
        let ahead = || (ix..len).find(|&i| self.track_at(i).is_some());
        let behind = || (0..=ix).rev().find(|&i| self.track_at(i).is_some());
        if forward {
            ahead().or_else(behind)
        } else {
            behind().or_else(ahead)
        }
    }

    /// The track rows under the group header line at `ix`, in view
    /// order; None when the row is no header. Meta counts as the header
    /// it sits under, disc dividers don't open a group of their own.
    fn group_track_rows(&self, ix: usize) -> Option<Vec<usize>> {
        match self.view.get(ix) {
            Some(Row::Header(_)) | Some(Row::Meta(_)) => {}
            _ => return None,
        }
        let rows = (ix + 1..self.view.len())
            .take_while(|&i| !matches!(self.view.get(i), Some(Row::Header(_))))
            .filter(|&i| self.track_at(i).is_some())
            .collect();
        Some(rows)
    }

    /// The edge length of an expanded header's cover tile: the full
    /// two-row block, so the art squares off exactly against the text.
    fn tile_side(&self) -> gpui::Pixels {
        self.density.size().table_row_height() * 2.
    }

    /// The heading look knobs packaged for the shared surface, mirrored
    /// off the delegate the same way the tile side is.
    fn head_look(&self) -> group_head::HeadLook {
        group_head::HeadLook {
            tile_side: self.tile_side(),
            show_art: self.header_art,
            show_year: self.header_year,
            show_details: self.header_details,
        }
    }

    /// One half of an expanded header's cover tile. The table draws every
    /// row one fixed height with no spanning cell, so each of the block's
    /// two rows clips its own half of a two-row-tall square: the name row
    /// the top (`bottom` false), the meta row the bottom. Same image
    /// handle both times, so gpui decodes it once. Pending and missing
    /// wear the same quiet placeholder, so a landing cover fills the tile
    /// without shifting the text beside it.
    fn group_tile(
        &mut self,
        g: u32,
        bottom: bool,
        cx: &mut Context<TableState<Self>>,
    ) -> AnyElement {
        let path = match self
            .groups
            .get(g as usize)
            .and_then(|group| group.art.clone())
        {
            Some(path) => path,
            None => {
                let id = {
                    let library = self.state.library.read(cx);
                    self.groups.get(g as usize).and_then(|group| {
                        library.projection().and_then(|projection| {
                            // No album tag means this is the unknown bucket,
                            // not a real album: keep the placeholder instead
                            // of whichever loose track's art lands first.
                            (!projection.resolve(group.first).album.is_empty())
                                .then(|| projection.db_id[group.first as usize])
                        })
                    })
                };
                let path = id
                    .and_then(|id| self.state.library.read(cx).paths_for(&[id]).ok())
                    .and_then(|mut paths| paths.pop());
                if let Some(group) = self.groups.get_mut(g as usize) {
                    group.art = Some(path.clone());
                }
                path
            }
        };
        let thumb = match path {
            Some(path) => self
                .state
                .thumbs
                .update(cx, |thumbs, cx| thumbs.get(&path, cx)),
            None => Thumb::Missing,
        };
        group_head::tile(thumb, self.tile_side(), self.art_rounding, bottom)
    }

    /// The group's name line. Grouped by album, compact packs the album
    /// artist, album, and year into its one row; expanded gives the
    /// album artist the line, larger, the year on the right, hands the
    /// album to the meta line under it, and opens the two-row cover tile
    /// the meta line closes. The other groupings name their one field -
    /// the tile and the trailing year are album presentation, so those
    /// stay off.
    fn render_group_header(
        &mut self,
        row_ix: usize,
        g: u32,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let expanded = self.headers == Headers::Expanded;
        let by_album = self.group_by == GroupBy::Album;
        let has_tile = expanded && by_album && self.header_art;
        let tile = has_tile.then(|| self.group_tile(g, false, cx));
        let (name, album, year) = match (
            self.groups.get(g as usize),
            self.state.library.read(cx).projection(),
        ) {
            (Some(group), Some(projection)) => {
                let v = projection.resolve(group.first);
                match self.group_by {
                    GroupBy::Album | GroupBy::Artist => {
                        // Rows migrated from before the album artist
                        // column carry an empty one until a rescan
                        // re-reads their tags; the first track's artist
                        // stands in rather than "unknown".
                        let name = if v.album_artist.is_empty() {
                            v.artist
                        } else {
                            v.album_artist
                        };
                        if by_album {
                            (name.to_string(), v.album.to_string(), v.year)
                        } else {
                            (name.to_string(), String::new(), 0)
                        }
                    }
                    GroupBy::Genre => (v.genre.to_string(), String::new(), 0),
                    GroupBy::Year => (
                        if v.year == 0 {
                            String::new()
                        } else {
                            v.year.to_string()
                        },
                        String::new(),
                        0,
                    ),
                }
            }
            _ => Default::default(),
        };
        let head = group_head::GroupHead {
            name: SharedString::from(name),
            album: SharedString::from(album),
            year,
            genre: SharedString::default(),
            quality: SharedString::default(),
            tracks: 0,
            total_ms: 0,
            by_album,
        };
        let look = self.head_look();
        div()
            .id(("row", row_ix))
            .bg(palette::bg_elevated())
            // A click selects the album, a double click plays it, so the
            // strip wears the same pointer a track row does.
            .cursor_pointer()
            // The expanded block reads as one: no border between its name
            // and meta lines. The width stays, so rows keep their height.
            .when(expanded, |d| d.border_color(gpui::transparent_black()))
            .when_some(tile, |d, tile| d.child(tile))
            .child(group_head::name_content(&head, &look, expanded))
    }

    /// The expanded header's second line: the album, then the group's
    /// genre, codec and bitrate, track count, and total time on the
    /// right, beside the cover tile's bottom half. The other groupings
    /// keep the count and time; the album, genre, and quality describe
    /// one album, not a mixed run, and the tile goes with them.
    fn render_group_meta(
        &mut self,
        row_ix: usize,
        g: u32,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let by_album = self.group_by == GroupBy::Album;
        let has_tile = by_album && self.header_art;
        let tile = has_tile.then(|| self.group_tile(g, true, cx));
        let (album, genre, quality, tracks, total_ms) = match (
            self.groups.get(g as usize),
            self.state.library.read(cx).projection(),
        ) {
            (Some(group), Some(projection)) if by_album => {
                let v = projection.resolve(group.first);
                (
                    v.album.to_string(),
                    v.genre.to_string(),
                    group_quality(group, projection),
                    group.tracks,
                    group.total_ms,
                )
            }
            (Some(group), Some(_)) => (
                String::new(),
                String::new(),
                String::new(),
                group.tracks,
                group.total_ms,
            ),
            _ => Default::default(),
        };
        let head = group_head::GroupHead {
            name: SharedString::default(),
            album: SharedString::from(album),
            year: 0,
            genre: SharedString::from(genre),
            quality: SharedString::from(quality),
            tracks,
            total_ms,
            by_album,
        };
        let look = self.head_look();
        div()
            .id(("row", row_ix))
            .bg(palette::bg_elevated())
            // Part of the same clickable album block as the name line.
            .cursor_pointer()
            .when_some(tile, |d, tile| d.child(tile))
            .child(group_head::meta_content(&head, &look))
    }

    /// The slim strip opening one disc's run inside a multi-disc group,
    /// a full-width line like the header rows so it stays put when wide
    /// column sets scroll sideways.
    fn render_disc_row(&mut self, row_ix: usize, disc: u16) -> Stateful<Div> {
        div().id(("row", row_ix)).child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_row()
                .items_center()
                .px(tokens::SPACE_SM)
                .text_color(palette::text_muted())
                .child(SharedString::from(format!("Disc {disc}"))),
        )
    }

    /// The next row whose leading text starts with the typed prefix, from
    /// the cursor on, wrapping. The leading text follows the active sort:
    /// its column when it has text, the album artist for the canonical
    /// order (what the grouping runs on), the track artist for sorts
    /// without text of their own (duration). ASCII-insensitive, like
    /// search.
    fn find_prefix(&self, prefix: &str, include_current: bool, cx: &App) -> Option<usize> {
        let library = self.state.library.read(cx);
        let projection = library.projection()?;
        let len = self.view.len();
        if len == 0 {
            return None;
        }
        let field = self.sort.as_ref().map(|(key, _)| key.as_ref());
        let start = match self.cursor {
            Some(cursor) if include_current => cursor,
            Some(cursor) => cursor + 1,
            None => 0,
        };
        (0..len).map(|i| (start + i) % len).find(|&ix| {
            let Some(row) = self.track_at(ix) else {
                return false;
            };
            let v = projection.resolve(row);
            let text = match field {
                Some("title") => v.title,
                Some("album") => v.album,
                Some("album_artist") | None => v.album_artist,
                Some("codec") => v.codec,
                Some(_) => v.artist,
            };
            text.get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        })
    }

    /// Re-locate the playing track in the current view: one scan per view
    /// swap or track change, never per frame.
    fn locate_playing(&mut self, cx: &App) {
        let row = self.playing_id.and_then(|id| {
            let library = self.state.library.read(cx);
            let projection = library.projection()?;
            self.view
                .iter()
                .position(|&row| matches!(row, Row::Track(r) if projection.db_id[r as usize] == id))
        });
        self.playing_row = row;
    }

    /// The rows this panel shows: the canonical order or search hits,
    /// narrowed by the shared structured filter, put through the active
    /// column sort when one is set. Only the canonical order gets grouping
    /// headers; a query or a column sort breaks the artist/album runs the
    /// headers name, so those views render flat.
    fn compute_view(
        &self,
        query: &str,
        filter: &rox_library::projection::FilterSet,
        cx: &App,
    ) -> (Arc<Vec<Row>>, Vec<Group>) {
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return (Arc::new(Vec::new()), Vec::new());
        };
        let base = if query.is_empty() {
            library.order()
        } else {
            Arc::new(projection.search(query))
        };
        let base = match projection.filter_mask(filter) {
            Some(mask) => Arc::new(
                base.iter()
                    .copied()
                    .filter(|&row| mask[row as usize])
                    .collect(),
            ),
            None => base,
        };
        let active = self
            .sort
            .as_ref()
            .and_then(|(key, desc)| sort_key(key).map(|key| (key, *desc)));
        match active {
            Some((key, desc)) => (
                Arc::new(
                    projection
                        .sort_view(&base, key, desc)
                        .into_iter()
                        .map(Row::Track)
                        .collect(),
                ),
                Vec::new(),
            ),
            None if query.is_empty() && self.headers != Headers::Off => {
                // Genre and year runs aren't contiguous in the canonical
                // order; re-sort by the group field, canonical inside.
                let base = match self.group_by.sort() {
                    Some(key) => Arc::new(projection.sort_view(&base, key, false)),
                    None => base,
                };
                let (rows, groups) = group_rows(
                    &base,
                    projection,
                    self.headers == Headers::Expanded,
                    self.group_by,
                );
                (Arc::new(rows), groups)
            }
            None => (
                Arc::new(base.iter().copied().map(Row::Track).collect()),
                Vec::new(),
            ),
        }
    }

    /// Append the owning panel's dropdown items to a row context menu.
    /// Called while the table entity is mid-update, so the panel's
    /// `dropdown_menu` must not read the table entity at build time (its
    /// click handlers may, they run after the update ends).
    fn panel_menu(&self, menu: PopupMenu, window: &mut Window, cx: &mut App) -> PopupMenu {
        let Some(panel) = self.panel.upgrade() else {
            return menu;
        };
        panel.update(cx, |panel, cx| panel.dropdown_menu(menu, window, cx))
    }

    /// Resolve the selected rows to db ids in view order and publish them
    /// on the shared selection.
    fn publish_selection(&self, cx: &mut App) {
        let Some(projection) = self.state.library.read(cx).projection().cloned() else {
            return;
        };
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        let ids: Vec<i64> = rows
            .iter()
            .filter_map(|&ix| self.track_at(ix))
            .map(|row| projection.db_id[row as usize])
            .collect();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
    }
}

impl TableDelegate for TrackTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.view.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    /// The header cell: the stock label plus a right-click menu that
    /// toggles the shown columns in place, the customize window's chips
    /// without the trip there. The table's own right-click menu stays a
    /// row affair; over the header it builds empty and never shows, so
    /// the two menus don't stack.
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let shown: HashSet<String> = self.columns.iter().map(|c| c.key.to_string()).collect();
        let panel = self.panel.clone();
        div()
            .size_full()
            .child(self.column(col_ix, cx).name.clone())
            .context_menu(move |mut menu, _, _| {
                for def in COLUMNS {
                    let key = def.key;
                    let panel = panel.clone();
                    menu = menu.item(
                        PopupMenuItem::new(def.label)
                            .checked(shown.contains(key))
                            .on_click(move |_, _, cx| {
                                if let Some(panel) = panel.upgrade() {
                                    panel.update(cx, |panel, cx| panel.toggle_column(key, cx));
                                }
                            }),
                    );
                }
                menu
            })
    }

    /// The header sort hook. The widget has already advanced the clicked
    /// column's cycle (canonical -> descending -> ascending) in its own
    /// column state; mirror it into the delegate's columns and swap the
    /// view here, because the table entity is mid-update and the panel's
    /// refresh path would re-enter it. The panel reads the sort back for
    /// persistence via `dump`.
    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        for (ix, column) in self.columns.iter_mut().enumerate() {
            column.sort = Some(if ix == col_ix {
                sort
            } else {
                ColumnSort::Default
            });
        }
        self.sort = match sort {
            ColumnSort::Ascending => Some((self.columns[col_ix].key.clone(), false)),
            ColumnSort::Descending => Some((self.columns[col_ix].key.clone(), true)),
            ColumnSort::Default => None,
        };
        let (query, filter) = self
            .panel
            .upgrade()
            .map(|panel| {
                let panel = panel.read(cx);
                (panel.effective_query(cx), panel.effective_filter(cx))
            })
            .unwrap_or_default();
        let (view, groups) = self.compute_view(&query, &filter, cx);
        self.view = view;
        self.groups = groups;
        // The old indices point elsewhere in the new order, same as any
        // view swap. The widget's own focus row does too, but it can only
        // be cleared once the table's update ends.
        self.selected.clear();
        self.sel_gen += 1;
        self.anchor = None;
        self.cursor = None;
        self.locate_playing(cx);
        let table = cx.entity();
        cx.defer(move |cx| {
            table.update(cx, |table, cx| table.clear_selection(cx));
        });
        cx.notify();
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        // A group header line is one full-width strip over emptied cells,
        // since the table has no row-spanning cell. It hangs off the row
        // itself, outside the horizontally scrolled cell region, so the
        // title stays put when wide column sets scroll sideways.
        match self.view.get(row_ix).copied() {
            Some(Row::Header(g)) => return self.render_group_header(row_ix, g, cx),
            Some(Row::Meta(g)) => return self.render_group_meta(row_ix, g, cx),
            Some(Row::Disc(disc)) => return self.render_disc_row(row_ix, disc),
            _ => {}
        }
        // The same wash the widget theme paints its own focus row with, so
        // multi-selected rows read as one set. The playing row wears the
        // highlight role instead, a faint cut of it, so it stays apart
        // from the accent-washed selection.
        let selected = self.selected.contains(&row_ix);
        // The row is a drag source: dragging carries the grabbed row, or the
        // whole set when the grab lands inside a multi-selection, onto a drop
        // target that queues it. Resolved here so the payload rides the frame.
        let drag = self.drag_payload(row_ix, cx);
        div()
            // Group bounds resolve innermost-first, so one shared name
            // still scopes each cell's group_hover to its own row: the
            // rating cell fades its unrated stars in on row hover.
            .group(track_cells::ROW_GROUP)
            .id(("row", row_ix))
            .cursor_pointer()
            .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
            .when(self.playing_row == Some(row_ix) && !selected, |d| {
                d.bg(palette::alpha(palette::highlight(), 0x12))
            })
            .when_some(drag, |d, drag| {
                d.on_drag(drag, |drag, _pos, _window, cx| {
                    cx.new(|_| PlayDragPreview {
                        title: drag.title.clone(),
                        extra: drag.len().saturating_sub(1),
                    })
                })
            })
    }

    /// The row context menu. A right click inside the selection acts on the
    /// whole set; outside it, the click reselects just that row first, so
    /// the menu always acts on what is highlighted. A group header stands
    /// for its album: the click selects the whole group, and the play item
    /// reads Play Album. The panel's own menu rides along after the track
    /// actions: the panel body hands its right-click to the table
    /// (`content_context_menu`), so this menu is the only one a click over
    /// the list opens, and it must not dead-end at Play. Disc dividers get
    /// the panel menu alone.
    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let album = self.group_track_rows(row_ix);
        if self.track_at(row_ix).is_none() && album.is_none() {
            return self.panel_menu(menu, window, cx);
        }
        if let Some(rows) = &album {
            self.selected = rows.iter().copied().collect();
            self.sel_gen += 1;
            self.anchor = rows.first().copied();
            self.cursor = rows.first().copied();
            self.publish_selection(cx);
            cx.notify();
        } else if !self.selected.contains(&row_ix) {
            self.selected = HashSet::from([row_ix]);
            self.sel_gen += 1;
            self.anchor = Some(row_ix);
            self.publish_selection(cx);
            cx.notify();
        }
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        // The selection as db ids, resolved now so the editor gets this
        // set even if another panel publishes over the shared selection
        // before the click lands.
        let ids: Vec<i64> = self
            .state
            .library
            .read(cx)
            .projection()
            .map(|projection| {
                rows.iter()
                    .filter_map(|&ix| self.track_at(ix))
                    .map(|row| projection.db_id[row as usize])
                    .collect()
            })
            .unwrap_or_default();
        let panel = self.panel.clone();
        let label = if album.is_some() {
            if self.group_by == GroupBy::Album {
                "Play Album".to_string()
            } else {
                "Play Group".to_string()
            }
        } else if rows.len() > 1 {
            format!("Play {} Tracks", rows.len())
        } else {
            "Play".to_string()
        };
        // A single row plays from it through the view, the double click's
        // move; a set or a group queues exactly the highlighted rows.
        let single_row = album.is_none() && rows.len() <= 1;
        let from_row = single_row.then_some(row_ix);
        let play_panel = panel.clone();
        let play_rows = rows.clone();
        let menu = panel::track_actions(
            menu,
            self.state.clone(),
            ids,
            label,
            window,
            cx,
            move |_, cx| {
                let Some(panel) = play_panel.upgrade() else { return };
                panel.update(cx, |panel, cx| match from_row {
                    Some(ix) => panel.play_from(ix, cx),
                    None => panel.play_rows(play_rows.clone(), cx),
                });
            },
        );
        // Filter the panel's search down to the clicked row's album or artist,
        // the cheap faceted browse. Only for a single clicked track row: a group
        // header stands for a whole album already, and a multi-row set has no
        // one album or artist to pin. An empty field skips its entry, nothing
        // to filter by.
        let menu = if album.is_none() && rows.len() == 1 {
            let (jump_album, jump_artist) = self
                .state
                .library
                .read(cx)
                .projection()
                .and_then(|projection| {
                    self.track_at(row_ix).map(|row| {
                        let v = projection.resolve(row);
                        (v.album.to_string(), v.artist.to_string())
                    })
                })
                .unwrap_or_default();
            let mut menu = menu;
            if !jump_album.is_empty() || !jump_artist.is_empty() {
                menu = menu.separator();
            }
            if !jump_album.is_empty() {
                let album_panel = panel.clone();
                menu = menu.item(
                    PopupMenuItem::new("Filter by Album")
                        .icon(Icon::default().path(icons::DISC))
                        .on_click(move |_, _, cx| {
                            let Some(panel) = album_panel.upgrade() else {
                                return;
                            };
                            let album = jump_album.clone();
                            panel.update(cx, |panel, cx| panel.jump_to_query("album", &album, cx));
                        }),
                );
            }
            if !jump_artist.is_empty() {
                let artist_panel = panel.clone();
                menu = menu.item(
                    PopupMenuItem::new("Filter by Artist")
                        .icon(Icon::default().path(icons::MIC))
                        .on_click(move |_, _, cx| {
                            let Some(panel) = artist_panel.upgrade() else {
                                return;
                            };
                            let artist = jump_artist.clone();
                            panel
                                .update(cx, |panel, cx| panel.jump_to_query("artist", &artist, cx));
                        }),
                );
            }
            menu
        } else {
            menu
        };
        self.panel_menu(menu.separator(), window, cx)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Header rows draw in render_tr; their cells stay empty.
        let Some(row) = self.track_at(row_ix) else {
            return div().into_any_element();
        };
        let Some(projection) = self.state.library.read(cx).projection().cloned() else {
            return div().into_any_element();
        };
        let v = projection.resolve(row);
        let playing = self.playing_row == Some(row_ix);
        let cell = div().truncate();
        // Copied out so the cover arm can borrow the delegate mutably (its
        // path cache) without the match still holding `self.columns`.
        let key = self.columns[col_ix].key.clone();
        if key.as_ref() == "cover" {
            let id = projection.db_id[row as usize];
            let path = match self.cover_paths.get(&id) {
                Some(path) => path.clone(),
                None => {
                    let path = self
                        .state
                        .library
                        .read(cx)
                        .paths_for(&[id])
                        .ok()
                        .and_then(|mut paths| paths.pop());
                    self.cover_paths.insert(id, path.clone());
                    path
                }
            };
            let thumb = crate::track_ui::track_columns::cover_thumb(&self.state, path.as_deref(), true, cx);
            return crate::track_ui::track_columns::cover_cell(&thumb).into_any_element();
        }
        let cell = match key.as_ref() {
            "track" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.track_no)),
            "title" => cell
                .when(playing, |d| d.text_color(palette::accent()))
                .child(SharedString::from(v.title.to_string())),
            "artist" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.artist.to_string())),
            "album_artist" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.album_artist.to_string())),
            "album" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.album.to_string())),
            "genre" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.genre.to_string())),
            "year" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.year)),
            "codec" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(v.codec.to_string())),
            "bitrate" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.bitrate_kbps)),
            "duration" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(fmt_ms(v.duration_ms))),
            "rating" => track_cells::rating(
                self.state.clone(),
                projection.db_id[row as usize],
                v.rating,
            ),
            "favourite" => {
                let id = projection.db_id[row as usize];
                track_cells::favourite(self.state.clone(), id, self.favourites.contains(&id))
            }
            // Blank at zero like the track and year cells: never played
            // reads cleaner as absence than as a column of zeros.
            "plays" => cell
                .text_color(palette::text_muted())
                .child(if v.plays == 0 {
                    SharedString::default()
                } else {
                    SharedString::from(v.plays.to_string())
                }),
            // How long ago the track was scanned in, blank when unknown
            // (a library indexed before the timestamp existed).
            "added" => cell
                .text_color(palette::text_muted())
                .child(if v.added <= 0 {
                    SharedString::default()
                } else {
                    SharedString::from(super::history::fmt_ago(self.added_now() - v.added))
                }),
            _ => cell,
        };
        cell.into_any_element()
    }

    /// Keep the delegate's columns in the widget's order: the table calls
    /// this before it reorders its own col_groups the same way, so cell
    /// rendering (indexed by the visual column) stays aligned. The layout
    /// dump reads the new order back off `columns`.
    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        if col_ix >= self.columns.len() || to_ix >= self.columns.len() {
            return;
        }
        let column = self.columns.remove(col_ix);
        self.columns.insert(to_ix, column);
    }

    /// No rows and a non-empty query means no hits; keep the body quiet
    /// like the old flat list did. The no-library case never reaches here,
    /// the panel renders its own empty state instead of the table.
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
}

/// One browse view over the shared catalog: its own search query and row
/// order, duplicable and poppable like any panel.
pub struct LibraryPanel {
    state: AppState,
    /// The table over the current view; the delegate holds the rows.
    table: Entity<TableState<TrackTable>>,
    query: String,
    /// The panel's own focus, what the dock focuses on tab activation. Kept
    /// apart from the search input's focus so activating the tab does not
    /// put every keystroke in the query, and so the playback key bindings
    /// (scoped out of SearchInput) stay live.
    focus: FocusHandle,
    /// The query editor, the shared search box; `query` mirrors its value
    /// via change events.
    search: Entity<SearchBox>,
    /// Show the search box; while hidden the query keeps its text but
    /// stops applying.
    show_search: bool,
    /// Filter by the panel's own `query` or follow the shared app-wide one.
    /// While global the box mirrors and writes the shared query; `query`
    /// keeps the panel's own text, dormant, for the switch back to local.
    query_source: QuerySource,
    /// A pending box reset: the active source's text needs to land in the
    /// box, but that needs a window, so the next render (which has one)
    /// applies it. Set on a source toggle or a shared-query change.
    resync_box: bool,
    /// A panel-local error (a failed play), shown until the catalog updates.
    error: Option<SharedString>,
    /// The playing track's path, the change detector: the player notifies
    /// every pump tick, so everything up to this compare stays cheap.
    playing_path: Option<PathBuf>,
    /// The type-ahead buffer and when it last grew; a pause starts over.
    type_ahead: String,
    type_ahead_at: Option<std::time::Instant>,
    /// The saved scroll row waiting for rows to restore against. The
    /// catalog loads after the panel builds, so the first non-empty view
    /// consumes this; None once applied.
    restore_scroll: Option<usize>,
    /// Scroll to the playing row when the track changes, and whether to
    /// glide there instead of jumping.
    follow_playing: bool,
    smooth_follow: bool,
    /// Scroll back to the playing row on its own once the list has sat
    /// untouched a spell.
    resume_playing: bool,
    /// The idle-resume clock: stamped on every scroll or press, it wakes
    /// the list back to the playing row when `resume_playing` is on and the
    /// user has stepped away.
    resume_idle: ResumeIdle,
    /// The view row the follow glide is headed to; stepped every frame in
    /// `body` and cleared on arrival.
    glide_to: Option<usize>,
    /// The last glide tick, its dt.
    glide_tick: Instant,
    /// The track list's row height, applied on the table each render.
    density: Density,
    /// The header style and what the headers group on. The delegate
    /// mirrors both for the view computation; they live here too so the
    /// dropdown's checkmarks build without reading the table entity
    /// (the row context menu builds mid-table-update).
    headers: Headers,
    group_by: GroupBy,
    /// The keys of the currently shown columns, mirrored off the delegate
    /// whenever the set changes so the Columns dropdown builds its checks
    /// without reading the table entity (the row context menu builds
    /// mid-table-update). Order and width live on the delegate; only the
    /// shown set matters here.
    columns_shown: HashSet<String>,
    /// The header tiles' corner radius; the delegate mirrors it for the
    /// tile render, the config dump carries it.
    art_rounding: f32,
    /// The art rounding slider's scrub strip, for the settings window.
    art_scrub: ScrubState,
    /// The header rows' look knobs; the delegate mirrors them for the
    /// header renders, the config dump carries them.
    header_art: bool,
    header_year: bool,
    header_details: bool,
    /// The rename, theme override, and placement locks shared by every
    /// panel, live for the render and carried by the config dump like
    /// every other view knob.
    chrome: PanelChrome,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Watches the hosting tab panel: whether this panel is solo decides
    /// where the toolbar renders, so membership changes must re-render.
    _tabs_changed: Option<Subscription>,
    _library_changed: Subscription,
    _table_events: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _player_changed: Subscription,
    _thumbs_changed: Subscription,
}

impl LibraryPanel {
    pub fn new(
        state: AppState,
        config: LibraryConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut LibraryPanel, _, event: &LibraryEvent, cx| {
                // A rating click or a landed listen only needs the cells
                // repainted: the value sits in the shared projection
                // already, and re-sorting a rating-sorted view here would
                // yank the row out from under the cursor mid-click. The
                // order catches up on the next refresh.
                if matches!(event, LibraryEvent::Rated | LibraryEvent::Played) {
                    this.table.update(cx, |_, cx| cx.notify());
                    return;
                }
                // A playlist edit does not touch the catalog view, only the
                // favourite highlights: reload the set and repaint, no rebuild.
                if matches!(event, LibraryEvent::PlaylistsChanged) {
                    this.reload_favourites(cx);
                    return;
                }
                this.error = None;
                this.refresh_view(cx);
                // The catalog loads after a restored track starts, so the
                // launch's follow waits for this first rebuild; rescans
                // re-land on the playing row the same way.
                if this.follow_playing {
                    this.follow_playing(cx);
                }
                cx.notify();
                this.refresh_title_bar(cx);
            },
        );
        let sort = config
            .sort_key
            .map(|key| (SharedString::from(key), config.sort_desc));
        let delegate = TrackTable {
            state: state.clone(),
            panel: cx.weak_entity(),
            view: Arc::new(Vec::new()),
            groups: Vec::new(),
            headers: config.headers,
            group_by: config.group_by,
            density: config.density,
            art_rounding: config.art_rounding,
            header_art: config.header_art,
            header_year: config.header_year,
            header_details: config.header_details,
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            columns: track_columns(&config.column_layout, &sort),
            sort,
            playing_id: None,
            playing_row: None,
            favourites: state.library.read(cx).favourite_ids(),
            cover_paths: HashMap::new(),
            drag_paths: HashMap::new(),
            sel_gen: 0,
            added_now: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            added_now_at: Instant::now(),
            drag_set: None,
        };
        // Widths and order persist by column key, so a drag survives a
        // layout save; the delegate mirrors the widget's reorder.
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .col_movable(true)
                .col_selectable(false)
        });
        let _table_events = cx.subscribe_in(&table, window, Self::on_table_event);
        // A panel restored as global opens showing the shared query; a local
        // one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local => config.query.clone(),
        };
        let search = cx.new(|cx| SearchBox::new("Search", &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Follow the shared query while global: re-filter and reset the box
        // to it on the next render. The reset needs a window, so it rides the
        // resync flag rather than happening here.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut LibraryPanel, _, _: &SharedQueryEvent, cx| {
                this.on_shared_query_changed(cx);
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut LibraryPanel, _, cx| {
            this.sync_playing(cx)
        });
        // A thumbnail landing repaints the rows; the panel itself has
        // nothing to recompute.
        let _thumbs_changed = cx.observe(&state.thumbs, |this: &mut LibraryPanel, _, cx| {
            this.table.update(cx, |_, cx| cx.notify());
        });
        let mut this = LibraryPanel {
            state,
            table,
            query: config.query,
            focus: cx.focus_handle(),
            search,
            show_search: config.search,
            query_source: config.query_source,
            resync_box: false,
            error: None,
            playing_path: None,
            type_ahead: String::new(),
            type_ahead_at: None,
            restore_scroll: (config.scroll_row > 0).then_some(config.scroll_row),
            follow_playing: config.follow_playing,
            smooth_follow: config.smooth_follow,
            resume_playing: config.resume_playing,
            resume_idle: ResumeIdle::default(),
            glide_to: None,
            glide_tick: Instant::now(),
            density: config.density,
            headers: config.headers,
            group_by: config.group_by,
            columns_shown: HashSet::new(),
            art_rounding: config.art_rounding,
            art_scrub: ScrubState::default(),
            header_art: config.header_art,
            header_year: config.header_year,
            header_details: config.header_details,
            chrome: config.chrome,
            tab_panel: None,
            _tabs_changed: None,
            _library_changed,
            _table_events,
            _search_events,
            _query_changed,
            _player_changed,
            _thumbs_changed,
        };
        this.refresh_view(cx);
        this.columns_shown = this.shown_columns(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Follow the player: on a track change, resolve the playing path to
    /// its id (one store lookup) and re-locate its row in the view.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if path == self.playing_path {
            return;
        }
        self.playing_path = path;
        let id = self
            .playing_path
            .as_ref()
            .and_then(|path| self.state.library.read(cx).id_for(path));
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.playing_id = id;
            delegate.locate_playing(cx);
            cx.notify();
        });
        if self.follow_playing {
            self.follow_playing(cx);
        }
    }

    /// Scroll the playing row into view: a glide when smooth is on, the
    /// jump otherwise. Scroll only - the automatic follow never touches
    /// the selection, that's the menu jump's move.
    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        if self.smooth_follow {
            if let Some(row) = self.table.read(cx).delegate().playing_row {
                self.glide_to = Some(row);
                cx.notify();
            }
        } else {
            self.table.update(cx, |table, cx| {
                if let Some(row) = table.delegate().playing_row {
                    table.scroll_to_row(row, cx);
                }
            });
        }
    }

    /// A scroll, drag, or press: restart the idle clock and arm a wake, so
    /// the list scrolls back to the playing row once the user steps away. A
    /// no-op unless the resume behavior is on, so an off panel spends
    /// nothing per gesture.
    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    /// The idle wake's landing: scroll back to the playing row, so long as
    /// the resume is still on. The clock only fires this once the list has
    /// sat untouched a full window, a gesture in between having pushed it
    /// out, so no extra idle check is needed here.
    fn resume_to_playing(&mut self, cx: &mut Context<Self>) {
        if self.resume_playing {
            self.follow_playing(cx);
        }
    }

    /// Browse from the keyboard while the panel itself is focused: arrows
    /// move a cursor, shift extends from the click path's anchor, enter
    /// plays, and plain typing jumps to the next match in the leading
    /// column. With the search box focused these stay out of the way: in
    /// the solo and popped-out layouts its toolbar sits inside the panel
    /// root, so its keystrokes bubble through here.
    fn on_panel_key(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        if self.search.read(cx).is_focused(window, cx) {
            return;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        // Arrow and type-ahead navigation is browsing too, so it restarts
        // the idle clock the same as a scroll or a click.
        self.touch_resume(cx);
        let shift = keystroke.modifiers.shift;
        match keystroke.key.as_str() {
            "up" => self.move_cursor(-1, shift, cx),
            "down" => self.move_cursor(1, shift, cx),
            "pageup" => self.move_cursor(-PAGE_ROWS, shift, cx),
            "pagedown" => self.move_cursor(PAGE_ROWS, shift, cx),
            // The edges snap inward past a leading header.
            "home" => {
                if let Some(ix) = self.table.read(cx).delegate().snap_to_track(0, true) {
                    self.set_cursor(ix, shift, cx);
                }
            }
            "end" => {
                let target = {
                    let delegate = self.table.read(cx).delegate();
                    delegate.snap_to_track(delegate.view.len().saturating_sub(1), false)
                };
                if let Some(ix) = target {
                    self.set_cursor(ix, shift, cx);
                }
            }
            "enter" => self.play_selection(cx),
            _ => {
                let Some(text) = &keystroke.key_char else {
                    return;
                };
                // Space stays the workspace's play/pause; it never starts
                // a jump, only continues one mid-phrase.
                if self.type_ahead.is_empty() && text == " " {
                    return;
                }
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// Grow or restart the type-ahead buffer and jump to its next match.
    /// A grown buffer re-tests the current row first, so refining a match
    /// stays put instead of skipping ahead.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let grown = panel::type_ahead_grow(&mut self.type_ahead, &mut self.type_ahead_at, text);
        let target = {
            let delegate = self.table.read(cx).delegate();
            delegate.find_prefix(&self.type_ahead, grown, cx)
        };
        if let Some(ix) = target {
            self.set_cursor(ix, false, cx);
        }
    }

    /// Put the cursor on a view row: plain selects just it, extend grows
    /// the selection from the anchor. Either way it publishes and scrolls
    /// into view.
    fn set_cursor(&mut self, ix: usize, extend: bool, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if delegate.track_at(ix).is_none() {
                return;
            }
            delegate.cursor = Some(ix);
            if extend {
                let anchor = delegate.anchor.unwrap_or(ix);
                let (lo, hi) = (anchor.min(ix), anchor.max(ix));
                // A range spanning a group break selects its tracks only.
                let range = (lo..=hi)
                    .filter(|&i| delegate.track_at(i).is_some())
                    .collect();
                delegate.selected = range;
                if delegate.anchor.is_none() {
                    delegate.anchor = Some(anchor);
                }
            } else {
                delegate.selected = HashSet::from([ix]);
                delegate.anchor = Some(ix);
            }
            delegate.sel_gen += 1;
            table.delegate().publish_selection(cx);
            table.scroll_to_row(ix, cx);
            cx.notify();
        });
    }

    /// Step the cursor; the first press with no cursor lands on the edge
    /// the step heads toward. A step landing on a header overshoots it the
    /// way it was heading, bouncing back at the ends.
    fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let target = {
            let delegate = self.table.read(cx).delegate();
            let len = delegate.view.len();
            if len == 0 {
                return;
            }
            let raw = match delegate.cursor {
                None if delta >= 0 => 0,
                None => len - 1,
                Some(cursor) => (cursor as isize + delta).clamp(0, len as isize - 1) as usize,
            };
            delegate.snap_to_track(raw, delta >= 0)
        };
        if let Some(target) = target {
            self.set_cursor(target, extend, cx);
        }
    }

    /// Enter: a multi-selection plays exactly itself, a lone cursor plays
    /// from its row in view order like a double click.
    fn play_selection(&mut self, cx: &mut Context<Self>) {
        let (mut rows, cursor) = {
            let delegate = self.table.read(cx).delegate();
            let rows: Vec<usize> = delegate.selected.iter().copied().collect();
            (rows, delegate.cursor)
        };
        rows.sort_unstable();
        if rows.len() > 1 {
            self.play_rows(rows, cx);
        } else if let Some(ix) = cursor.or_else(|| rows.first().copied()) {
            self.play_from(ix, cx);
        }
    }

    /// The menu's jump: put the cursor on the playing row, which selects
    /// it, publishes, and scrolls it into view in one move.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let row = self.table.read(cx).delegate().playing_row;
        if let Some(row) = row {
            self.set_cursor(row, false, cx);
        }
    }

    /// The menu's follow toggle: flip the follow state and catch up right
    /// away when turning it on, the same move as the settings switch.
    fn toggle_follow_playing(&mut self, cx: &mut Context<Self>) {
        self.follow_playing = !self.follow_playing;
        if self.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// Re-read the favourited set into the table and repaint the hearts. Runs
    /// on a playlist change, far cheaper than a view rebuild since the rows
    /// themselves do not move.
    fn reload_favourites(&mut self, cx: &mut Context<Self>) {
        let favourites = self.state.library.read(cx).favourite_ids();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().favourites = favourites;
            cx.notify();
        });
    }

    fn refresh_view(&mut self, cx: &mut Context<Self>) {
        let query = self.effective_query(cx);
        let filter = self.effective_filter(cx);
        self.table.update(cx, |table, cx| {
            // Selection indices point into the old view; drop them along
            // with the widget's own focus row. The shared selection keeps
            // the last explicit pick, a view refresh is not one.
            let (view, groups) = table.delegate().compute_view(&query, &filter, cx);
            let delegate = table.delegate_mut();
            delegate.view = view;
            delegate.groups = groups;
            delegate.selected.clear();
            delegate.sel_gen += 1;
            delegate.anchor = None;
            delegate.cursor = None;
            delegate.locate_playing(cx);
            table.clear_selection(cx);
            cx.notify();
        });
        // The saved scroll restores against the first view with rows; a
        // strict deferred scroll on the handle, so it lands on the paint
        // that shows them, even if the panel sits in a background tab
        // until then. Earlier refreshes (the empty initial load) keep it
        // pending.
        if let Some(row) = self.restore_scroll {
            if !self.table.read(cx).delegate().view.is_empty() {
                self.restore_scroll = None;
                self.table
                    .read(cx)
                    .vertical_scroll_handle
                    .scroll_to_item_strict(row, ScrollStrategy::Top);
            }
        }
    }

    fn on_table_event(
        &mut self,
        _: &Entity<TableState<TrackTable>>,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // A click selects; focus moves back to the panel so the
            // playback keys stay with the workspace, not the table. Shift
            // extends from the anchor, cmd (ctrl elsewhere) toggles, and a
            // plain click starts over. The widget also fires this for a
            // double click's first clicks, which land as a plain select.
            TableEvent::SelectRow(ix) => {
                window.focus(&self.focus);
                let ix = *ix;
                // A click on a group header selects its album whole. The
                // widget's own focus row drops either way, so the header
                // strip itself takes no mark; disc dividers just clear.
                if self.table.read(cx).delegate().track_at(ix).is_none() {
                    self.table.update(cx, |table, cx| {
                        table.clear_selection(cx);
                        let Some(rows) = table.delegate().group_track_rows(ix) else {
                            return;
                        };
                        let delegate = table.delegate_mut();
                        delegate.anchor = rows.first().copied();
                        delegate.cursor = rows.first().copied();
                        delegate.selected = rows.into_iter().collect();
                        delegate.sel_gen += 1;
                        table.delegate().publish_selection(cx);
                        cx.notify();
                    });
                    return;
                }
                let modifiers = window.modifiers();
                self.table.update(cx, |table, cx| {
                    let delegate = table.delegate_mut();
                    if modifiers.shift {
                        let anchor = delegate.anchor.unwrap_or(ix);
                        let (lo, hi) = (anchor.min(ix), anchor.max(ix));
                        // Tracks only across a group break, like the
                        // keyboard's shift-extend.
                        let range: Vec<usize> = (lo..=hi)
                            .filter(|&i| delegate.track_at(i).is_some())
                            .collect();
                        // Ctrl+Shift stacks the range onto the selection so
                        // you can skip a run and grab a second block; plain
                        // shift replaces.
                        if modifiers.secondary() {
                            delegate.selected.extend(range);
                        } else {
                            delegate.selected = range.into_iter().collect();
                        }
                        if delegate.anchor.is_none() {
                            delegate.anchor = Some(anchor);
                        }
                    } else if modifiers.secondary() {
                        if !delegate.selected.insert(ix) {
                            delegate.selected.remove(&ix);
                            // The widget put its focus row here on the way
                            // in; a toggle-off must clear that too.
                            table.clear_selection(cx);
                        }
                        table.delegate_mut().anchor = Some(ix);
                    } else {
                        delegate.selected = HashSet::from([ix]);
                        delegate.anchor = Some(ix);
                    }
                    table.delegate_mut().cursor = Some(ix);
                    table.delegate_mut().sel_gen += 1;
                    table.delegate().publish_selection(cx);
                    cx.notify();
                });
            }
            // The double click is what plays, leaving single clicks free
            // to select. A track plays from itself through the view; a
            // group header plays its album whole, the Play Album the
            // right click also carries. A disc divider plays nothing, so
            // its rows come back empty.
            TableEvent::DoubleClickedRow(ix) => {
                let ix = *ix;
                let (is_track, album) = {
                    let delegate = self.table.read(cx).delegate();
                    (
                        delegate.track_at(ix).is_some(),
                        delegate.group_track_rows(ix).unwrap_or_default(),
                    )
                };
                if is_track {
                    self.play_from(ix, cx);
                } else if !album.is_empty() {
                    self.play_rows(album, cx);
                }
            }
            // Written back into the delegate's columns: refresh() re-reads
            // them, and the save request persists them.
            TableEvent::ColumnWidthsChanged(widths) => {
                let widths = widths.clone();
                self.table.update(cx, |table, _| {
                    let columns = &mut table.delegate_mut().columns;
                    for (column, width) in columns.iter_mut().zip(widths) {
                        column.width = width;
                    }
                });
                self.request_layout_save(cx);
            }
            // The widget already reordered the delegate's columns; just get
            // the new order onto disk.
            TableEvent::MoveColumn(..) => self.request_layout_save(cx),
            _ => {}
        }
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        self.state
            .library
            .update(cx, |library, cx| library.browse(cx));
    }

    /// The shown columns in display order, each with its live width, for
    /// the layout dump and for duplicates.
    fn column_specs(&self, cx: &App) -> Vec<ColumnSpec> {
        self.table
            .read(cx)
            .delegate()
            .columns
            .iter()
            .map(|column| ColumnSpec {
                key: column.key.to_string(),
                width: f32::from(column.width),
            })
            .collect()
    }

    /// The panel's live config, for the layout dump and for duplicates.
    fn config(&self, cx: &App) -> LibraryConfig {
        let sort = self.table.read(cx).delegate().sort.clone();
        LibraryConfig {
            chrome: self.chrome.clone(),
            query: self.query.clone(),
            search: self.show_search,
            query_source: self.query_source,
            density: self.density,
            headers: self.headers,
            group_by: self.group_by,
            column_layout: self.column_specs(cx),
            sort_key: sort.as_ref().map(|(key, _)| key.to_string()),
            sort_desc: sort.is_some_and(|(_, desc)| desc),
            scroll_row: self.scroll_row(cx),
            follow_playing: self.follow_playing,
            resume_playing: self.resume_playing,
            smooth_follow: self.smooth_follow,
            art_rounding: self.art_rounding,
            header_art: self.header_art,
            header_year: self.header_year,
            header_details: self.header_details,
        }
    }

    /// The view row at the top of the viewport, read off the table's
    /// scroll handle. The uniform list never reports child bounds to its
    /// base handle, so the row comes from the pixel offset over the row
    /// height - the density's, the same fixed height every row renders
    /// at (the handle's own `last_item_size.item` is the viewport, not a
    /// row). A restore still pending (the panel never painted) reports
    /// its target, so an unshown panel round-trips its position instead
    /// of dropping to zero.
    fn scroll_row(&self, cx: &App) -> usize {
        if let Some(row) = self.restore_scroll {
            return row;
        }
        let table = self.table.read(cx);
        let handle = table.vertical_scroll_handle.0.borrow();
        if let Some(deferred) = &handle.deferred_scroll_to_item {
            return deferred.item_index;
        }
        let row_height = table.delegate().density.size().table_row_height();
        if row_height <= px(0.) {
            return 0;
        }
        (-handle.base_handle.offset().y / row_height)
            .floor()
            .max(0.) as usize
    }

    /// Show or hide a registry column, keeping the rest in place. A shown
    /// column appends at the end in its default width; hiding drops it.
    /// The table re-reads the delegate's columns and the view stays put.
    fn toggle_column(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(def) = column_def(key) else { return };
        let mut sort_cleared = false;
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if let Some(ix) = delegate.columns.iter().position(|c| c.key.as_ref() == key) {
                // Never let the last column go: an empty table has no
                // header to bring one back from.
                if delegate.columns.len() > 1 {
                    delegate.columns.remove(ix);
                    // A hidden sort column leaves no header to clear the
                    // sort; drop back to the canonical order instead.
                    if delegate.sort.as_ref().is_some_and(|(k, _)| k.as_ref() == key) {
                        delegate.sort = None;
                        sort_cleared = true;
                    }
                }
            } else {
                let column = Column::new(def.key, def.label)
                    .width(px(def.default_width))
                    .sort(ColumnSort::Default);
                delegate.columns.push(if def.right {
                    column.text_right()
                } else {
                    column
                });
            }
            table.refresh(cx);
        });
        if sort_cleared {
            self.refresh_view(cx);
        }
        self.columns_shown = self.shown_columns(cx);
        self.refresh_title_bar(cx);
        self.request_layout_save(cx);
    }

    /// The keys of the currently shown columns, for the settings checklist.
    /// The dropdown reads the `columns_shown` mirror instead, so it never
    /// touches the table while the row context menu builds mid-update.
    fn shown_columns(&self, cx: &App) -> HashSet<String> {
        self.table
            .read(cx)
            .delegate()
            .columns
            .iter()
            .map(|c| c.key.to_string())
            .collect()
    }

    /// The customize window's column picker: one checkable row per registry
    /// column, ticked while shown. Multi-select, so it stacks a checklist
    /// instead of the exclusive segmented control; the reset rides the
    /// block's header.
    fn column_checklist(&self, cx: &mut Context<Self>) -> Div {
        let shown = self.shown_columns(cx);
        let mut list = div().flex().flex_col().gap(tokens::SPACE_XS);
        for def in COLUMNS {
            let key = def.key;
            let on = shown.contains(key);
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .py(px(1.))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.toggle_column(key, cx)),
                    )
                    .child(settings_ui::checkbox(on))
                    .child(
                        div()
                            .text_color(if on {
                                palette::text()
                            } else {
                                palette::text_muted()
                            })
                            .child(def.label),
                    ),
            );
        }
        list
    }

    /// Restore the registry's default visible set and order.
    fn reset_columns(&mut self, cx: &mut Context<Self>) {
        let sort = self.table.read(cx).delegate().sort.clone();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().columns = track_columns(&[], &sort);
            table.refresh(cx);
        });
        self.columns_shown = self.shown_columns(cx);
        self.refresh_title_bar(cx);
        self.request_layout_save(cx);
    }

    /// Nudge the dock to persist the layout after a column change it never
    /// sees on its own - a resize, reorder, or toggle. The panel's own events
    /// don't reach the dock, but its host tab panel's do, so bounce a
    /// LayoutChanged through it and the workspace's debounced save picks the
    /// new columns up. Without this the columns only reach disk on a clean
    /// close or the next unrelated dock change, so a relaunch can lose them.
    fn request_layout_save(&self, cx: &mut Context<Self>) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|w| w.upgrade()) {
            tabs.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
        }
    }

    /// While docked, the panel's controls live in the tab panel's title bar,
    /// which only repaints when the tab panel itself is notified. Call this
    /// after any change the title bar shows: query, focus, status, error.
    fn refresh_title_bar(&self, cx: &mut App) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|tabs| tabs.upgrade()) {
            tabs.update(cx, |_, cx| cx.notify());
        }
    }

    /// Queue the double-clicked track as the start of a natural progression
    /// through the view: the tracks before it seed behind the cursor so Prev
    /// walks back, the ones after carry Next on through the library, and the
    /// clicked track plays. This is the playing context, not the queue, so it
    /// never shows in the queue panel; the window is bounded so a huge view
    /// doesn't materialize whole, with a share of the budget kept for history.
    /// Headers pass under the cap, so it counts tracks.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        // With shuffle on, draw from the whole view, not just the rows after
        // the clicked one; the engine pins the head, so the clicked track
        // still plays first while everything else shuffles in behind it.
        if self.state.player.read(cx).shuffle() {
            self.play_shuffled_from(ix, cx);
            return;
        }
        let (rows, start) = {
            let delegate = self.table.read(cx).delegate();
            let tracks: Vec<usize> = (0..delegate.view.len())
                .filter(|&i| delegate.track_at(i).is_some())
                .collect();
            let Some(clicked) = tracks.iter().position(|&i| i == ix) else {
                return;
            };
            // Keep up to half the budget behind the click for history, then
            // slide the window forward to fill it against the view's end.
            let mut lo = clicked.saturating_sub(QUEUE_CAP / 2);
            lo = lo.min(tracks.len().saturating_sub(QUEUE_CAP));
            let hi = (lo + QUEUE_CAP).min(tracks.len());
            (tracks[lo..hi].to_vec(), clicked - lo)
        };
        self.play_rows_at(rows, start, cx);
    }

    /// Resolve view rows to paths and queue them starting at the first,
    /// the explicit-selection play.
    fn play_rows(&mut self, rows: Vec<usize>, cx: &mut Context<Self>) {
        self.play_rows_at(rows, 0, cx);
    }

    /// Resolve view rows to paths and queue them on the shared player with
    /// the cursor at `start`.
    fn play_rows_at(&mut self, rows: Vec<usize>, start: usize, cx: &mut Context<Self>) {
        let result = {
            let view = self.table.read(cx).delegate().view.clone();
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return;
            };
            let ids: Vec<i64> = rows
                .into_iter()
                .filter_map(|ix| match view.get(ix) {
                    Some(&Row::Track(row)) => Some(projection.db_id[row as usize]),
                    _ => None,
                })
                .collect();
            library.paths_for(&ids)
        };
        match result {
            Ok(paths) => self
                .state
                .player
                .update(cx, |player, cx| player.play_at(paths, start, cx)),
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
                self.refresh_title_bar(cx);
            }
        }
    }

    /// Turn shuffle on, then queue `rows` from the front. The engine pins the
    /// head when shuffle engages, so the first row plays first and the rest
    /// draw in a random order. Rows past the cap drop.
    fn play_shuffled(&mut self, mut rows: Vec<usize>, cx: &mut Context<Self>) {
        rows.truncate(QUEUE_CAP);
        self.state
            .player
            .update(cx, |player, _| player.set_shuffle(true));
        self.play_rows_at(rows, 0, cx);
    }

    /// Play the whole view shuffled with `ix` first: the clicked row heads the
    /// queue so the pinned head plays before the shuffled rest. "Play Shuffled"
    /// on a single row and a shuffle-on double click both land here.
    fn play_shuffled_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows = {
            let delegate = self.table.read(cx).delegate();
            let mut tracks: Vec<usize> = (0..delegate.view.len())
                .filter(|&i| delegate.track_at(i).is_some())
                .collect();
            if let Some(pos) = tracks.iter().position(|&i| i == ix) {
                let clicked = tracks.remove(pos);
                tracks.insert(0, clicked);
            }
            tracks
        };
        self.play_shuffled(rows, cx);
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
            // The input's focus ring renders in the title bar while the
            // panel shares a group, and that row only repaints when the
            // tab panel is notified.
            SearchEvent::FocusChanged => {
                cx.notify();
                self.refresh_title_bar(cx);
            }
            // Escape on an empty query leaves the box, which hands the
            // playback keys back to the workspace.
            SearchEvent::Dismissed => {
                window.focus(&self.focus);
                cx.notify();
                self.refresh_title_bar(cx);
            }
            SearchEvent::Submitted => {}
        }
    }

    fn search_box(&self, _window: &Window, cx: &mut Context<Self>) -> Div {
        self.search.update(cx, |search, cx| search.element(cx))
    }

    /// The popped-out window has no title bar to host the controls, so it
    /// keeps them as a toolbar row above the list. The catalog status lives
    /// in the workspace menubar; only a panel-local error shows here.
    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(36.))
            .px(tokens::SPACE_SM)
            .gap(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .bg(palette::bg_toolbar())
            .border_b_1()
            .border_color(palette::border())
            .when(self.show_search, |d| {
                d.child(self.search_box(window, cx).flex_1())
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .flex_none()
                        .text_color(palette::text_muted())
                        .child(error),
                )
            })
    }

    fn track_list(&self) -> impl IntoElement {
        Table::new(&self.table)
            .stripe(true)
            .bordered(false)
            .with_size(self.density.size())
    }

    /// Set the row height and re-render; persisted on the next layout dump.
    fn set_density(&mut self, density: Density, cx: &mut Context<Self>) {
        if self.density == density {
            return;
        }
        self.density = density;
        // The delegate mirrors it for the header tile's row math.
        self.table
            .update(cx, |table, _| table.delegate_mut().density = density);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    /// Set the header style and rebuild the view; persisted on the next
    /// layout dump.
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>) {
        if self.headers == headers {
            return;
        }
        self.headers = headers;
        self.table
            .update(cx, |table, _| table.delegate_mut().headers = headers);
        self.refresh_view(cx);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    /// Set what the headers group on and rebuild the view; persisted on
    /// the next layout dump like the header style.
    fn set_group_by(&mut self, group_by: GroupBy, cx: &mut Context<Self>) {
        if self.group_by == group_by {
            return;
        }
        self.group_by = group_by;
        self.table
            .update(cx, |table, _| table.delegate_mut().group_by = group_by);
        self.refresh_view(cx);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("library-empty")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.browse(cx)))
            .child(div().text_lg().child("Open a music folder"))
            .child(
                div()
                    .text_color(palette::text_muted())
                    .child("It gets scanned into the library (flac, mp3, wav)"),
            )
    }
}

impl panel::PanelSettings for LibraryPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("View", icons::ROWS_3)]
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
                    self.show_search,
                    |this: &mut Self, on, cx| {
                        this.show_search = on;
                        // The box keeps its text; the view snaps to the
                        // full catalog while hidden.
                        this.refresh_view(cx);
                        cx.notify();
                        this.refresh_title_bar(cx);
                    },
                    self.query_source,
                    |this: &mut Self, source, cx| this.pick_query_source(source, cx),
                    cx,
                ))
                .child(panel::tracking_section(
                    self.follow_playing,
                    "Scroll to the playing row whenever the track changes",
                    |this: &mut Self, on, cx| {
                        this.follow_playing = on;
                        // Catch up right away instead of waiting for
                        // the next track change.
                        if on {
                            this.follow_playing(cx);
                        }
                        cx.notify();
                    },
                    self.resume_playing,
                    "Scroll back to the playing row after you stop browsing",
                    |this: &mut Self, on, cx| {
                        this.resume_playing = on;
                        cx.notify();
                    },
                    self.smooth_follow,
                    "Glide to the row instead of jumping",
                    |this: &mut Self, on, cx| {
                        this.smooth_follow = on;
                        cx.notify();
                    },
                    cx,
                ))
                .into_any_element(),
        )
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let headers = self.headers;
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_block(
                "Columns",
                Some("Which columns show; drag the headers in the panel to reorder and size them"),
                Some(
                    settings_ui::small_button(
                        "Reset",
                        icons::REFRESH_CW,
                        false,
                        cx.listener(|this, _, _, cx| this.reset_columns(cx)),
                    )
                    .into_any_element(),
                ),
                self.column_checklist(cx),
            ))
            .child(panel::setting_row(
                "Headers",
                Some("Group breaks over the canonical order; searching or sorting renders flat"),
                panel::choices(
                    &[
                        ("Off", Headers::Off),
                        ("Compact", Headers::Compact),
                        ("Expanded", Headers::Expanded),
                    ],
                    headers,
                    |this: &mut Self, headers, cx| this.set_headers(headers, cx),
                    cx,
                ),
            ))
            .when(headers != Headers::Off, |d| {
                d.child(panel::setting_row(
                    "Group By",
                    Some("What the headers break on; genre and year re-sort the list"),
                    panel::choices(
                        &[
                            ("Album", GroupBy::Album),
                            ("Artist", GroupBy::Artist),
                            ("Genre", GroupBy::Genre),
                            ("Year", GroupBy::Year),
                        ],
                        self.group_by,
                        |this: &mut Self, group_by, cx| this.set_group_by(group_by, cx),
                        cx,
                    ),
                ))
            })
            .child(panel::setting_row(
                "Density",
                Some("The track list's row height"),
                panel::choices(
                    &[
                        ("Compact", Density::Compact),
                        ("Comfortable", Density::Comfortable),
                    ],
                    self.density,
                    |this: &mut Self, density, cx| this.set_density(density, cx),
                    cx,
                ),
            ))
            .into_any_element()
    }

    /// The library's own appearance rows on the shared page: what the
    /// group heading rows show and how their covers round, look knobs
    /// that live on the config because they shape the rows, not the
    /// panel frame.
    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rounding = self.art_rounding;
        let fraction = (rounding / ART_ROUNDING_MAX).clamp(0., 1.);
        let headers = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "Cover Art",
                Some("The expanded album headers' cover tile"),
                panel::toggle(
                    self.header_art,
                    |this: &mut Self, on, cx| {
                        this.header_art = on;
                        this.table
                            .update(cx, |table, _| table.delegate_mut().header_art = on);
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Year",
                Some("The year on the heading rows"),
                panel::toggle(
                    self.header_year,
                    |this: &mut Self, on, cx| {
                        this.header_year = on;
                        this.table
                            .update(cx, |table, _| table.delegate_mut().header_year = on);
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Details",
                Some("The genre and quality on the expanded meta line; the track count and total time stay"),
                panel::toggle(
                    self.header_details,
                    |this: &mut Self, on, cx| {
                        this.header_details = on;
                        this.table
                            .update(cx, |table, _| table.delegate_mut().header_details = on);
                        cx.notify();
                    },
                    cx,
                ),
            ));
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(settings_ui::section("Headers", None, headers))
                .child(settings_ui::section(
                    "Covers",
                    None,
                    panel::setting_row(
                        "Art Rounding",
                        Some("Round the album headers' cover corners"),
                        settings_ui::slider_labeled(
                            &self.art_scrub,
                            fraction,
                            format!("{rounding:.0} px"),
                            |this: &mut Self, fraction, cx| {
                                let value = (fraction * ART_ROUNDING_MAX).round();
                                this.art_rounding = value;
                                // The delegate mirrors it for the tile render,
                                // the density's route.
                                this.table.update(cx, |table, _| {
                                    table.delegate_mut().art_rounding = value
                                });
                                cx.notify();
                            },
                            cx,
                        ),
                    ),
                ))
                .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for LibraryPanel {}

impl Focusable for LibraryPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl QueryFilter for LibraryPanel {
    fn shared_query(&self) -> &Entity<crate::query::shared_query::SharedQuery> {
        &self.state.query
    }
    fn query_box(&self) -> &Entity<SearchBox> {
        &self.search
    }
    fn query_source(&self) -> QuerySource {
        self.query_source
    }
    fn set_query_source_value(&mut self, source: QuerySource) {
        self.query_source = source;
    }
    fn local_query(&self) -> String {
        self.query.clone()
    }
    fn set_local_query(&mut self, query: String) {
        self.query = query;
    }
    fn query_box_shown(&self) -> bool {
        self.show_search
    }
    fn set_query_box_shown(&mut self, shown: bool) {
        self.show_search = shown;
    }
    fn rebuild_query_view(&mut self, cx: &mut Context<Self>) {
        self.refresh_view(cx);
    }
    fn set_query_resync(&mut self, pending: bool) {
        self.resync_box = pending;
    }
    fn after_query_change(&mut self, cx: &mut Context<Self>) {
        self.refresh_title_bar(cx);
    }
}

impl Panel for LibraryPanel {
    fn panel_name(&self) -> &'static str {
        "library"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.chrome.title.as_deref(), "Library")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.chrome.title.clone().map(SharedString::from)
    }

    /// The panel's controls share the title bar row instead of stacking a
    /// second toolbar row under it. Kept compact: the title row is 30px.
    fn title_suffix(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.show_search && self.error.is_none() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .gap(tokens::SPACE_SM)
                .when(self.show_search, |d| {
                    d.child(self.search_box(window, cx).w(px(180.)))
                })
                .when_some(self.error.clone(), |d, error| {
                    d.child(
                        div()
                            .max_w(px(240.))
                            .truncate()
                            .text_color(palette::text_muted())
                            .child(error),
                    )
                }),
        )
    }

    fn locked(&self, _cx: &App) -> bool {
        self.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The table serves row context menus over the whole body, so the tab
    /// panel's body right-click stays out; the panel dropdown lives on the
    /// tab and the toolbar.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.chrome, self.min_size(cx))
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn dump(&self, cx: &App) -> PanelState {
        let config = self.config(cx);
        let mut state = PanelState::new(self);
        state.info =
            PanelInfo::panel(serde_json::to_value(config).unwrap_or(serde_json::Value::Null));
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self._tabs_changed = tab_panel
            .upgrade()
            .map(|tabs| cx.observe(&tabs, |_, _, cx| cx.notify()));
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
        self._tabs_changed = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // Jump and Follow ride at the top; the view knobs group under a
        // Display flyout below so the menu stays short. Every entry
        // dismisses the menu on click, and the next open rebuilds with the
        // change reflected. The customize window still holds the same
        // knobs with real controls.
        let weak = cx.entity().downgrade();
        let weak_f = cx.entity().downgrade();
        let follow = self.follow_playing;
        // Checks on the right so these two keep their icons; the default
        // left side would swap the check in for the icon.
        let menu = menu
            .check_side(Side::Right)
            .item(
                PopupMenuItem::new("Jump to Playing")
                    .icon(Icon::default().path(icons::DISC))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.jump_to_playing(cx));
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("Follow Playing")
                    .icon(Icon::default().path(icons::LOCATE))
                    .checked(follow)
                    .on_click(move |_, _, cx| {
                        if let Some(this) = weak_f.upgrade() {
                            this.update(cx, |this, cx| this.toggle_follow_playing(cx));
                        }
                    }),
            );

        // Display section: the view knobs, one flyout per setting so the
        // menu stays short. The flyouts build eagerly off the panel's
        // mirrors, never the table: this menu also builds inside the row
        // context menu, mid-table-update.
        let menu = menu.separator().label("Display");

        // Columns: the same toggles as the header dropdown and the settings
        // checklist, one row per registry column ticked while shown, read off
        // the panel's mirror.
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            for def in COLUMNS {
                let key = def.key;
                submenu = submenu.item(panel::check_row(
                    def.label,
                    None,
                    move |this: &Self| this.columns_shown.contains(key),
                    move |this, cx| this.toggle_column(key, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu("Columns", submenu));

        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            // The density options carry their own icon, so the tick that
            // marks the active one moves to the right edge instead of
            // taking the left icon slot; a left check would drop the icon
            // on the picked row.
            let mut submenu = submenu.check_side(Side::Right);
            for (density, name, icon) in [
                (Density::Compact, "Compact", icons::ROWS_3),
                (Density::Comfortable, "Comfortable", icons::ROWS_2),
            ] {
                submenu = submenu.item(panel::check_row(
                    name,
                    Some(icon),
                    move |this: &Self| this.density == density,
                    move |this, cx| this.set_density(density, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu("Density", submenu));

        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            let mut submenu = submenu.check_side(Side::Right);
            for (headers, name) in [
                (Headers::Off, "Off"),
                (Headers::Compact, "Compact"),
                (Headers::Expanded, "Expanded"),
            ] {
                submenu = submenu.item(panel::check_row(
                    name,
                    None,
                    move |this: &Self| this.headers == headers,
                    move |this, cx| this.set_headers(headers, cx),
                    &panel,
                ));
            }
            submenu
        });
        let mut menu = menu.item(PopupMenuItem::submenu("Headers", submenu));

        if self.headers != Headers::Off {
            let panel = cx.entity();
            let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
                panel::follow_panel(&panel, cx);
                let mut submenu = submenu.check_side(Side::Right);
                for (group_by, name) in [
                    (GroupBy::Album, "Album"),
                    (GroupBy::Artist, "Artist"),
                    (GroupBy::Genre, "Genre"),
                    (GroupBy::Year, "Year"),
                ] {
                    submenu = submenu.item(panel::check_row(
                        name,
                        None,
                        move |this: &Self| this.group_by == group_by,
                        move |this, cx| this.set_group_by(group_by, cx),
                        &panel,
                    ));
                }
                submenu
            });
            menu = menu.item(PopupMenuItem::submenu("Group By", submenu));
        }

        // Follow the shared search query, or filter by this panel's own box.
        let menu = crate::query::shared_query::search_flyout(
            menu,
            |this: &Self| this.query_source,
            |this: &Self| this.show_search,
            &cx.entity(),
            |this: &mut Self, source, cx| this.pick_query_source(source, cx),
            |this: &mut Self, on, cx| {
                this.show_search = on;
                // The box keeps its text; the view snaps to the full catalog
                // while hidden.
                this.refresh_view(cx);
                cx.notify();
                this.refresh_title_bar(cx);
            },
            window,
            cx,
        );

        // Panel section: operations on the panel itself, not its contents.
        // Duplicate copies this view's config, the query included, over the
        // same catalog and player.
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        let menu = panel::duplicate_item(menu, &cx.entity(), self.tab_panel.clone(), |this, window, cx| {
            let (state, config) = {
                let panel = this.read(cx);
                (panel.state.clone(), panel.config(cx))
            };
            LibraryPanel::new(state, config, window, cx)
        });
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for LibraryPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl LibraryPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // A pending box reset (a source toggle or a shared-query change)
        // lands here, where a window exists to set the input's text.
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        // The follow glide eases toward the playing row, stepped here in
        // render (the cover panel's fade idiom), one frame at a time until
        // it lands.
        let dt = self.glide_tick.elapsed().as_secs_f32().min(0.05);
        self.glide_tick = Instant::now();
        if let Some(row) = self.glide_to {
            let (handle, count) = {
                let table = self.table.read(cx);
                (
                    table.vertical_scroll_handle.clone(),
                    table.delegate().view.len(),
                )
            };
            match panel::glide_target(&handle, row, count) {
                Some(target) if !panel::glide_step(&handle, target, dt) => self.glide_to = None,
                // Not laid out yet, or still moving: keep going.
                _ => window.request_animation_frame(),
            }
        }

        let busy = self.state.library.read(cx).busy().is_some();
        // The "open a folder" call-to-action means the catalog itself holds no
        // tracks, so it keys off the loaded projection, never the view. Off the
        // view it would flash during the initial load - the projection has not
        // landed, the view is transiently empty - and it would wrongly show
        // when a search or filter hides every row. `is_some_and` keeps it off
        // until the projection loads: while None, the empty view stands.
        let catalog_empty = self
            .state
            .library
            .read(cx)
            .projection()
            .is_some_and(|p| p.is_empty());
        let body = if catalog_empty && !busy {
            self.empty_state(cx).into_any_element()
        } else {
            self.track_list().into_any_element()
        };
        // The controls live in the tab bar via title_suffix while the panel
        // shares a group; solo or popped out there is no header at all, so
        // the toolbar renders in the body instead.
        let headerless = self
            .tab_panel
            .as_ref()
            .and_then(|tabs| tabs.upgrade())
            .is_none_or(|tabs| tabs.read(cx).panels_count() < 2);
        // The root must size itself: the dock's tab panel lays the panel view
        // out as a root element (cached, absolute), where flex_1 has no flex
        // parent to grow in and the height would collapse to the content.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_panel())
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, event, window, cx| this.on_panel_key(event, window, cx)),
            )
            // Any scroll or press over the list counts as browsing; the
            // stamps only restart the idle clock, leaving the scroll and the
            // click to the table underneath, so nothing acts twice.
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                this.touch_resume(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.touch_resume(cx)),
            )
            .when(
                headerless && (self.show_search || self.error.is_some()),
                |d| d.child(self.toolbar(window, cx)),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

pub(crate) fn fmt_ms(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// A track number or year cell: blank when zero, since the scanner stores
/// a missing tag as 0 and a bare 0 reads as data.
fn fmt_num(n: u16) -> SharedString {
    if n == 0 {
        SharedString::default()
    } else {
        n.to_string().into()
    }
}
