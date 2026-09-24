//! The quick-play modal: Ctrl/Cmd+P or Ctrl/Cmd+F drops a search box over the
//! workspace to jump to a track. Enter or a click queues from the picked track
//! in result order.
//!
//! The Power Search window hosts the same view (see
//! [`set_hosted`](QuickPlay::set_hosted)) as a place to work rather than a
//! jump.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    Action, App, Context, DismissEvent, Div, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, relative, svg,
    uniform_list,
};
use gpui_component::input::{MoveDown, MovePageDown, MovePageUp, MoveUp, SelectAll};
use gpui_component::menu::ContextMenuExt;
use rox_core::QUEUE_CAP;
use rox_core::fmt::fmt_ms;
use rox_library::cue::Origin;
use rox_library::projection::{FilterSet, Projection, QUERY_FIELDS};
use rox_playback::engine::shuffle_slice;

use rox_core::settings::{QuickPlayConfig, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState};
use rox_panel_api::query::search::{SearchBox, SearchEvent};
use rox_panel_api::suggest;
use rox_panel_api::track_ui::track_columns;
use rox_services::catalog::LibraryEvent;
use rox_services::thumbs::Thumb;

/// Row heights for the `uniform_list`, which needs every row to agree.
const ROW_H: f32 = 30.;
const ROW_H_COMFORTABLE: f32 = 40.;

const SUBTITLE_H: f32 = 14.;

const VISIBLE_ROWS: usize = 14;

/// Caps on the heads a search surfaces, so a broad query can't bury the tracks.
const MAX_ARTIST_HITS: usize = 6;
const MAX_ALBUM_HITS: usize = 8;

const PAGE_ROWS: isize = 10;

/// The fields that can be absent; the rest always carry a value.
const ABSENCE_FIELDS: &[&str] = &[
    "year",
    "genre",
    "artist",
    "albumartist",
    "album",
    "title",
    "rating",
    "plays",
];

const SYNTAX_KEYS: &[&str] = &[
    "quick-play-syntax-free",
    "quick-play-syntax-field",
    "quick-play-syntax-numeric",
    "quick-play-syntax-year",
    "quick-play-syntax-absent",
    "quick-play-syntax-exclude",
];

/// A whole artist or album the search surfaces above the track hits. Playing
/// one queues every track it holds.
#[derive(Clone, Copy)]
enum Head {
    Artist {
        album_artist: u32,
        row: u32,
    },
    Album {
        album_artist: u32,
        album: u32,
        row: u32,
    },
}

struct RowInfo {
    ix: usize,
    title: SharedString,
    /// Drawn after the title as a reading. The subtitle gets none: it's a
    /// composed line.
    title_reading: SharedString,
    sub: SharedString,
    trailing: SharedString,
    path: Option<PathBuf>,
    is_head: bool,
    is_station: bool,
}

impl Head {
    fn row(&self) -> u32 {
        match *self {
            Head::Artist { row, .. } | Head::Album { row, .. } => row,
        }
    }

    fn contains(&self, projection: &Projection, row: u32) -> bool {
        let i = row as usize;
        match *self {
            Head::Artist { album_artist, .. } => projection.album_artist[i] == album_artist,
            Head::Album {
                album_artist,
                album,
                ..
            } => projection.album_artist[i] == album_artist && projection.album[i] == album,
        }
    }
}

#[derive(Clone)]
pub struct Seed {
    pub ids: Vec<i64>,
    pub label: SharedString,
}

/// Also the groups the seed's rows cover, so a head tests with a hash lookup.
struct SeedRows {
    mask: Vec<bool>,
    artists: HashSet<u32>,
    albums: HashSet<(u32, u32)>,
}

/// Heads are kept on any of their tracks being in the seed, not their
/// representative row alone, or an album the seed mostly holds would drop out.
fn seed_rows(projection: &Projection, ids: &[i64]) -> SeedRows {
    let mask = projection
        .filter_mask(&FilterSet::with_ids(ids.to_vec()))
        .unwrap_or_else(|| vec![false; projection.len()]);
    let mut artists = HashSet::new();
    let mut albums = HashSet::new();
    for (row, ok) in mask.iter().enumerate() {
        if !ok {
            continue;
        }
        artists.insert(projection.album_artist[row]);
        albums.insert((projection.album_artist[row], projection.album[row]));
    }
    SeedRows {
        mask,
        artists,
        albums,
    }
}

impl SeedRows {
    fn keeps(&self, row: u32) -> bool {
        self.mask.get(row as usize).copied().unwrap_or(false)
    }

    fn keeps_head(&self, head: Head) -> bool {
        match head {
            Head::Artist { album_artist, .. } => self.artists.contains(&album_artist),
            Head::Album {
                album_artist,
                album,
                ..
            } => self.albums.contains(&(album_artist, album)),
        }
    }
}

/// Shift takes the run from the anchor, ctrl or cmd toggles, both stack the run
/// onto what's lit. Returns the new set and anchor.
fn click_selection(
    selected: &HashSet<usize>,
    anchor: Option<usize>,
    ix: usize,
    shift: bool,
    toggle: bool,
) -> (HashSet<usize>, Option<usize>) {
    if shift {
        let start = anchor.unwrap_or(ix);
        let (lo, hi) = (start.min(ix), start.max(ix));
        let range = lo..=hi;
        let set = if toggle {
            selected.iter().copied().chain(range).collect()
        } else {
            range.collect()
        };
        (set, Some(start))
    } else if toggle {
        let mut set = selected.clone();
        if !set.remove(&ix) {
            set.insert(ix);
        }
        (set, Some(ix))
    } else {
        (HashSet::from([ix]), Some(ix))
    }
}

fn nearest_selected(selected: &HashSet<usize>, from: usize) -> Option<usize> {
    selected
        .iter()
        .copied()
        .min_by_key(|&ix| (ix.abs_diff(from), ix))
}

/// The same radio glyph the queue and the track info strip use for a station.
fn station_cell() -> Div {
    let side = palette::scaled_px(track_columns::ROW_HEIGHT_STOCK - 6.);
    div()
        .flex_none()
        .size(side)
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(icons::RADIO)
                .size(px(14.))
                .text_color(palette::text_muted()),
        )
}

pub struct QuickPlay {
    state: AppState,
    search: Entity<SearchBox>,
    query: String,
    heads: Vec<Head>,
    hits: Arc<Vec<u32>>,
    /// Indexes into the combined list, heads first. Always holds the cursor.
    selected: HashSet<usize>,
    cursor: usize,
    anchor: Option<usize>,
    /// Read when the context menu builds a frame later.
    menu_row: Option<usize>,
    /// Filled on a row's first paint, so a scroll doesn't run a store query per
    /// row per frame. Cleared when the hits rebuild.
    cover_paths: HashMap<i64, Option<PathBuf>>,
    scroll: UniformListScrollHandle,
    error: Option<SharedString>,
    config: QuickPlayConfig,
    show_config: bool,
    seed: Option<Seed>,
    seed_rows: Option<SeedRows>,
    show_syntax: bool,
    hosted: bool,
    _input_events: Subscription,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
}

impl EventEmitter<DismissEvent> for QuickPlay {}

impl Focusable for QuickPlay {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.search.read(cx).focus_handle(cx)
    }
}

impl QuickPlay {
    pub fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            SearchBox::new(
                rox_i18n::t!("quick-play-search-placeholder"),
                "",
                window,
                cx,
            )
        });
        let _input_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // A scan swaps the projection, so recompute over the new one.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut QuickPlay, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.attach_suggestions(cx);
                this.refresh(cx);
            },
        );
        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut QuickPlay, _, cx| cx.notify());
        let mut this = QuickPlay {
            state,
            search,
            query: String::new(),
            heads: Vec::new(),
            hits: Arc::new(Vec::new()),
            selected: HashSet::from([0]),
            cursor: 0,
            anchor: Some(0),
            menu_row: None,
            cover_paths: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            error: None,
            config: Settings::load().look.bundle.appearance.quick_play,
            show_config: false,
            seed: None,
            seed_rows: None,
            show_syntax: false,
            hosted: false,
            _input_events,
            _library_changed,
            _thumbs_changed,
        };
        this.attach_suggestions(cx);
        this.refresh(cx);
        this
    }

    /// Turn the view into the Power Search window's content: it fills the
    /// frame, a click selects and a double click or Enter plays, a right click
    /// opens the track actions, and playing doesn't dismiss.
    pub fn set_hosted(&mut self, hosted: bool) {
        self.hosted = hosted;
    }

    fn attach_suggestions(&self, cx: &mut Context<Self>) {
        let provider = suggest::query_provider(&self.state.library, cx);
        self.search
            .update(cx, |search, cx| search.set_completions(provider, cx));
    }

    pub fn set_seed(&mut self, seed: Option<Seed>, cx: &mut Context<Self>) {
        self.seed = seed;
        self.refresh(cx);
    }

    pub fn set_query(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.set_value(text, window, cx));
        self.query = text.to_string();
        self.refresh(cx);
    }

    /// Scaled by the app font, so the list height derived from this stays in
    /// step with the text.
    fn row_h(&self) -> f32 {
        let base = if self.config.comfortable {
            ROW_H_COMFORTABLE
        } else {
            ROW_H
        };
        let base = if self.config.show_subtitle {
            base + SUBTITLE_H
        } else {
            base
        };
        base * palette::font_scale()
    }

    fn menu_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.search
            .update(cx, |search, cx| search.menu_action(action, window, cx))
    }

    fn toggle_config(&mut self, cx: &mut Context<Self>) {
        self.show_config = !self.show_config;
        cx.notify();
    }

    fn edit_config(&mut self, edit: impl FnOnce(&mut QuickPlayConfig), cx: &mut Context<Self>) {
        edit(&mut self.config);
        let config = self.config.clone();
        Settings::update(move |s| s.look.bundle.appearance.quick_play = config);
        cx.notify();
    }

    fn len(&self) -> usize {
        self.heads.len() + self.hits.len()
    }

    /// Browsing with nothing typed is a random draw, so the modal isn't the
    /// same rows every open. A seed lists its own rows in browse order.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (heads, hits, seed_rows) = {
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) => {
                    let seed = self
                        .seed
                        .as_ref()
                        .map(|seed| seed_rows(projection, &seed.ids));
                    if !self.query.is_empty() {
                        let mut heads = Vec::new();
                        for hit in projection
                            .search_artists(&self.query)
                            .into_iter()
                            .take(MAX_ARTIST_HITS)
                        {
                            heads.push(Head::Artist {
                                album_artist: hit.album_artist,
                                row: hit.row,
                            });
                        }
                        for hit in projection
                            .search_albums(&self.query)
                            .into_iter()
                            .take(MAX_ALBUM_HITS)
                        {
                            heads.push(Head::Album {
                                album_artist: hit.album_artist,
                                album: hit.album,
                                row: hit.row,
                            });
                        }
                        // Stations are results here, sorted behind the tracks.
                        let mut hits = projection.search_all(&self.query);
                        if let Some(rows) = &seed {
                            heads.retain(|head| rows.keeps_head(*head));
                            hits.retain(|&row| rows.keeps(row));
                        }
                        (heads, Arc::new(hits), seed)
                    } else if let Some(rows) = &seed {
                        // A seed with nothing typed lists in browse order.
                        let ordered: Vec<u32> = library
                            .order()
                            .iter()
                            .copied()
                            .filter(|&row| rows.keeps(row))
                            .collect();
                        (Vec::new(), Arc::new(ordered), seed)
                    } else {
                        // Shuffled whole rather than sampled, so enter still
                        // queues a full run.
                        let mut rows = library.order().as_ref().clone();
                        shuffle_slice(&mut rows);
                        (Vec::new(), Arc::new(rows), seed)
                    }
                }
                None => (Vec::new(), Arc::new(Vec::new()), None),
            }
        };
        self.heads = heads;
        self.hits = hits;
        self.seed_rows = seed_rows;
        self.cover_paths.clear();
        self.selected = HashSet::from([0]);
        self.cursor = 0;
        self.anchor = Some(0);
        self.menu_row = None;
        self.error = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn on_search_event(
        &mut self,
        search: &Entity<SearchBox>,
        event: &SearchEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => {
                self.query = search.read(cx).query().to_string();
                self.refresh(cx);
            }
            // A ctrl click can empty the selection, and then enter plays
            // nothing.
            SearchEvent::Submitted => {
                if !self.selected.is_empty() {
                    self.play(self.cursor, cx);
                }
            }
            SearchEvent::Dismissed => cx.emit(DismissEvent),
            SearchEvent::FocusChanged => {}
        }
    }

    /// Collapses the selection onto the cursor.
    fn move_selected(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let ix = (self.cursor as isize + delta).clamp(0, len as isize - 1) as usize;
        if ix == self.cursor && self.selected.len() == 1 {
            return;
        }
        self.cursor = ix;
        self.selected = HashSet::from([ix]);
        self.anchor = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let len = self.len();
        if len == 0 {
            return;
        }
        self.selected = (0..len).collect();
        self.anchor = Some(0);
        cx.notify();
    }

    /// Focus goes back to the search box: pick a row, keep typing.
    fn select(
        &mut self,
        ix: usize,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ix >= self.len() {
            return;
        }
        let (selected, anchor) = click_selection(
            &self.selected,
            self.anchor,
            ix,
            modifiers.shift,
            modifiers.secondary(),
        );
        // The cursor must stay lit, or enter would play an unlit row. It steps
        // to the nearest lit row, or stays put when the selection is empty.
        self.cursor = if selected.contains(&ix) {
            ix
        } else {
            nearest_selected(&selected, ix).unwrap_or(self.cursor)
        };
        self.selected = selected;
        self.anchor = anchor;
        let handle = self.search.read(cx).focus_handle(cx);
        window.focus(&handle);
        cx.notify();
    }

    fn play_ids(&self, ix: usize, cx: &App) -> Vec<i64> {
        if ix >= self.len() {
            return Vec::new();
        }
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return Vec::new();
        };
        if ix < self.heads.len() {
            let head = self.heads[ix];
            let seed = self.seed_rows.as_ref();
            library
                .order()
                .iter()
                .copied()
                .filter(|&row| head.contains(projection, row))
                .filter(|&row| seed.is_none_or(|seed| seed.keeps(row)))
                .take(QUEUE_CAP)
                .map(|row| projection.db_id[row as usize])
                .collect()
        } else {
            let track_ix = ix - self.heads.len();
            self.hits[track_ix..]
                .iter()
                .take(QUEUE_CAP)
                .map(|&row| projection.db_id[row as usize])
                .collect()
        }
    }

    fn selection_ids(&self, cx: &App) -> Vec<i64> {
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return Vec::new();
        };
        let seed = self.seed_rows.as_ref();
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for ix in rows {
            if ix < self.heads.len() {
                let head = self.heads[ix];
                ids.extend(
                    library
                        .order()
                        .iter()
                        .copied()
                        .filter(|&row| head.contains(projection, row))
                        .filter(|&row| seed.is_none_or(|seed| seed.keeps(row)))
                        .map(|row| projection.db_id[row as usize]),
                );
            } else if let Some(&row) = self.hits.get(ix - self.heads.len()) {
                ids.push(projection.db_id[row as usize]);
            }
        }
        ids.retain(|id| seen.insert(*id));
        ids.truncate(QUEUE_CAP);
        ids
    }

    /// As an overlay this dismisses too; hosted, the window stays up.
    fn queue_ids(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        let result = self.state.library.read(cx).keys_for(&ids);
        match result {
            Ok(keys) => {
                self.state
                    .player
                    .update(cx, |player, cx| player.play(keys, cx));
                if !self.hosted {
                    cx.emit(DismissEvent);
                }
            }
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
            }
        }
    }

    fn play(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ids = self.play_ids(ix, cx);
        self.queue_ids(ids, cx);
    }

    fn hit_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<Div> {
        let show_cover = self.config.show_cover;
        let hosted = self.hosted;
        let head_count = self.heads.len();
        let rows: Vec<RowInfo> = {
            let QuickPlay {
                state,
                heads,
                hits,
                cover_paths,
                ..
            } = self;
            let library = state.library.read(cx);
            let Some(projection) = library.projection() else {
                return Vec::new();
            };
            let mut cover_path = |row: u32| {
                if !show_cover {
                    return None;
                }
                let id = projection.db_id[row as usize];
                cover_paths
                    .entry(id)
                    .or_insert_with(|| {
                        library
                            .paths_for(&[id])
                            .ok()
                            .and_then(|mut paths| paths.pop())
                    })
                    .clone()
            };
            range
                .filter_map(|ix| {
                    if ix < head_count {
                        let head = *heads.get(ix)?;
                        let (title, reading, sub, tag) = match head {
                            Head::Artist { album_artist, .. } => (
                                projection.album_artists.strings[album_artist as usize].clone(),
                                projection
                                    .album_artists
                                    .sort_name(album_artist as usize)
                                    .to_string(),
                                String::new(),
                                rox_i18n::t!("quick-play-tag-artist"),
                            ),
                            Head::Album {
                                album_artist,
                                album,
                                ..
                            } => (
                                projection.albums.strings[album as usize].clone(),
                                projection.albums.sort_name(album as usize).to_string(),
                                projection.album_artists.strings[album_artist as usize].clone(),
                                rox_i18n::t!("quick-play-tag-album"),
                            ),
                        };
                        return Some(RowInfo {
                            ix,
                            title: SharedString::from(title),
                            title_reading: SharedString::from(reading),
                            sub: SharedString::from(sub),
                            trailing: tag,
                            path: cover_path(head.row()),
                            is_head: true,
                            is_station: false,
                        });
                    }
                    let row = *hits.get(ix - head_count)?;
                    let v = projection.resolve(row);
                    // A station has no artist, album or length, so the subtitle
                    // names it.
                    let is_station = Origin::of(v.source) == Origin::Radio;
                    let sub = match (is_station, v.artist.is_empty(), v.album.is_empty()) {
                        (true, _, _) => rox_i18n::t!("metadata-source-radio").to_string(),
                        (_, false, false) => format!("{} - {}", v.artist, v.album),
                        (_, false, true) => v.artist.to_string(),
                        (_, true, false) => v.album.to_string(),
                        (_, true, true) => String::new(),
                    };
                    // Zero means unknown, not 0:00.
                    let time = if v.duration_ms == 0 {
                        SharedString::default()
                    } else {
                        SharedString::from(fmt_ms(v.duration_ms))
                    };
                    Some(RowInfo {
                        ix,
                        title: SharedString::from(v.title.to_string()),
                        title_reading: SharedString::from(v.title_sort.to_string()),
                        sub: SharedString::from(sub),
                        trailing: time,
                        // No file behind a stream to read a cover off.
                        path: match is_station {
                            true => None,
                            false => cover_path(row),
                        },
                        is_head: false,
                        is_station,
                    })
                })
                .collect()
        };
        let covers: Vec<Option<Thumb>> = rows
            .iter()
            .map(|info| {
                track_columns::cover_thumb(&self.state, info.path.as_deref(), show_cover, cx)
            })
            .collect();
        let row_h = self.row_h();
        let readings = rox_core::settings::show_readings();
        rows.into_iter()
            .zip(covers)
            .map(|(info, cover)| {
                let RowInfo {
                    ix,
                    title,
                    title_reading,
                    sub,
                    trailing,
                    path: _,
                    is_head,
                    is_station,
                } = info;
                div()
                    .w_full()
                    .h(px(row_h))
                    .px(tokens::SPACE_SM)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .cursor_pointer()
                    .when(self.selected.contains(&ix), |d| {
                        d.bg(palette::alpha(palette::accent(), 0x26))
                    })
                    .when(
                        hosted && self.cursor == ix && self.selected.len() > 1,
                        |d| d.bg(palette::alpha(palette::accent(), 0x3d)),
                    )
                    .hover(|d| d.bg(palette::bg_control_hover_opaque()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            if !this.hosted || event.click_count > 1 {
                                this.play(ix, cx);
                            } else {
                                this.select(ix, event.modifiers, window, cx);
                            }
                        }),
                    )
                    .when(hosted, |d| {
                        d.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                this.menu_row = Some(ix);
                                if !this.selected.contains(&ix) {
                                    this.select(ix, Modifiers::default(), window, cx);
                                }
                            }),
                        )
                    })
                    // Shown whether or not covers are, or with covers and
                    // subtitles off a station only differs by its missing
                    // duration.
                    .when(is_station, |d| d.child(station_cell()))
                    .when(show_cover && !is_station, |d| {
                        d.child(track_columns::cover_cell(
                            &cover,
                            track_columns::ROW_HEIGHT_STOCK,
                        ))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .child(div().w_full().truncate().child(panel::named(
                                &title,
                                &title_reading,
                                readings,
                            )))
                            .when(self.config.show_subtitle && !sub.is_empty(), |d| {
                                d.child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_xs()
                                        .text_color(palette::text_secondary())
                                        .child(sub),
                                )
                            }),
                    )
                    .when(
                        is_head || (self.config.show_duration && !trailing.is_empty()),
                        |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_color(palette::text_muted())
                                    .child(trailing),
                            )
                        },
                    )
            })
            .collect()
    }

    fn hint_chip(&self, term: SharedString, cx: &mut Context<Self>) -> Div {
        div()
            .px(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let term = term.clone();
                    move |this, _, window, cx| {
                        this.search
                            .update(cx, |search, cx| search.append_term(&term, window, cx));
                    }
                }),
            )
            .child(term)
    }

    fn hint_row(&self, cx: &mut Context<Self>) -> Div {
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .border_t_1()
            .border_color(palette::border())
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_muted())
            .child(self.syntax_button(cx))
            .children(
                QUERY_FIELDS
                    .iter()
                    .map(|(name, _)| self.hint_chip(SharedString::from(format!("{name}:")), cx)),
            )
            .children(ABSENCE_FIELDS.iter().map(|name| {
                // The trailing space ends the term, so the next word isn't read
                // as its value.
                self.hint_chip(SharedString::from(format!("-{name} ")), cx)
            }))
    }

    fn syntax_button(&self, cx: &mut Context<Self>) -> Div {
        let on = self.show_syntax;
        div()
            .flex_none()
            .p(px(2.))
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .when(on, |d| d.bg(palette::bg_control_active()))
            .when(!on, |d| d.hover(|d| d.bg(palette::bg_control())))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_syntax = !this.show_syntax;
                    cx.notify();
                }),
            )
            .child(svg().path(icons::INFO).size(px(13.)).text_color(if on {
                palette::text()
            } else {
                palette::text_muted()
            }))
    }

    fn syntax_sheet(&self, cx: &mut Context<Self>) -> Div {
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .border_t_1()
            .border_color(palette::border())
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("quick-play-syntax-title"))
            .children(SYNTAX_KEYS.iter().map(|key| {
                let example = rox_i18n::t!(&format!("{key}.example"));
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let example = example.clone();
                            move |this, _, window, cx| {
                                this.search.update(cx, |search, cx| {
                                    search.append_term(&example, window, cx)
                                });
                            }
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(tokens::SPACE_XS)
                            .rounded(tokens::RADIUS)
                            .bg(palette::bg_control())
                            .text_color(palette::text())
                            .child(example),
                    )
                    .child(div().flex_1().min_w_0().child(rox_i18n::t!(*key)))
            }))
    }

    fn seed_chip(&self, label: SharedString, cx: &mut Context<Self>) -> Div {
        div()
            .pt(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .pl(tokens::SPACE_XS)
                    .pr(px(3.))
                    .py(px(1.))
                    .rounded(tokens::RADIUS)
                    .bg(palette::bg_control())
                    .text_xs()
                    .text_color(palette::text())
                    .child(label)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.set_seed(None, cx)),
                            )
                            .child(
                                svg()
                                    .path(icons::CLOSE)
                                    .size(px(10.))
                                    .text_color(palette::text_muted()),
                            ),
                    ),
            )
    }

    fn config_button(&self, cx: &mut Context<Self>) -> Div {
        let on = self.show_config;
        div()
            .flex_none()
            .p(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .when(on, |d| d.bg(palette::bg_control_active()))
            .when(!on, |d| d.hover(|d| d.bg(palette::bg_control())))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.toggle_config(cx)),
            )
            .child(svg().path(icons::SLIDERS).size(px(16.)).text_color(if on {
                palette::text()
            } else {
                palette::text_muted()
            }))
    }

    fn config_panel(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .pt(tokens::SPACE_SM)
            .mt(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .child(panel::setting_row(
                rox_i18n::t!("quick-play-cover"),
                Some(rox_i18n::t!("quick-play-cover.description")),
                panel::toggle(
                    self.config.show_cover,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.show_cover = on, cx);
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("quick-play-subtitle"),
                Some(rox_i18n::t!("quick-play-subtitle.description")),
                panel::toggle(
                    self.config.show_subtitle,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.show_subtitle = on, cx);
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("quick-play-duration"),
                Some(rox_i18n::t!("quick-play-duration.description")),
                panel::toggle(
                    self.config.show_duration,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.show_duration = on, cx);
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("quick-play-comfortable-rows"),
                Some(rox_i18n::t!("quick-play-comfortable-rows.description")),
                panel::toggle(
                    self.config.comfortable,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.comfortable = on, cx);
                    },
                    cx,
                ),
            ))
    }
}

impl Render for QuickPlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let len = self.len();
        // The overlay sizes to its rows up to the cap; hosted, the list fills
        // the frame.
        let list_h: gpui::Length = if self.hosted {
            relative(1.).into()
        } else {
            px(self.row_h() * len.clamp(1, VISIBLE_ROWS) as f32).into()
        };
        let this = cx.entity().downgrade();
        let list = if len == 0 {
            div()
                .h(list_h)
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(if self.query.is_empty() {
                    "The library is empty"
                } else {
                    "No matches"
                })
                .into_any_element()
        } else {
            uniform_list("quick-play-hits", len, move |range, _, cx| {
                this.upgrade()
                    .map(|this| this.update(cx, |this, cx| this.hit_rows(range, cx)))
                    .unwrap_or_default()
            })
            .track_scroll(self.scroll.clone())
            .h(list_h)
            .w_full()
            .into_any_element()
        };
        // The context menu is hosted only: over the workspace it would fight
        // the click-outside dismiss.
        let list = if self.hosted {
            let weak = cx.entity().downgrade();
            div()
                .flex_1()
                .min_h_0()
                .child(list)
                // Capture phase runs before any row records itself, so a press
                // off the rows leaves no stale target.
                .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                    if event.button == MouseButton::Right {
                        this.menu_row = None;
                    }
                }))
                .context_menu(move |menu, window, cx| {
                    let Some(this) = weak.upgrade() else {
                        return menu;
                    };
                    let Some(ix) = ({
                        let view = this.read(cx);
                        view.menu_row.filter(|ix| *ix < view.len())
                    }) else {
                        return menu;
                    };
                    let view = this.read(cx);
                    let ids = view.selection_ids(cx);
                    let rows = view.selected.len();
                    let label = if rows > 1 {
                        rox_i18n::t!("library-play-tracks", count = rows as u64).to_string()
                    } else {
                        match view.heads.get(ix) {
                            Some(Head::Album { .. }) => {
                                rox_i18n::t!("library-play-album").to_string()
                            }
                            Some(Head::Artist { .. }) => {
                                rox_i18n::t!("library-play-group").to_string()
                            }
                            None => rox_i18n::t!("library-play").to_string(),
                        }
                    };
                    let state = view.state.clone();
                    let play = this.downgrade();
                    panel::track_actions(menu, state, ids, label, window, cx, move |_, cx| {
                        let Some(this) = play.upgrade() else {
                            return;
                        };
                        this.update(cx, |this, cx| {
                            if this.selected.len() > 1 {
                                let ids = this.selection_ids(cx);
                                this.queue_ids(ids, cx);
                            } else {
                                this.play(ix, cx);
                            }
                        });
                    })
                })
                .into_any_element()
        } else {
            list
        };
        div()
            .when(self.hosted, |d| d.size_full())
            .when(!self.hosted, |d| d.w(px(560.)))
            .flex()
            .flex_col()
            .bg(palette::bg_menu_opaque())
            // Card chrome and click-outside dismiss are overlay only; a window
            // has its own frame.
            .when(!self.hosted, |d| {
                d.rounded(tokens::RADIUS)
                    .border_1()
                    .border_color(palette::border_light())
                    .shadow_md()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            })
            // Scopes the workspace's playback bindings out while the modal is
            // up.
            .key_context("SearchInput")
            // The single-line input swallows up, down and the page keys, so the
            // list takes them in the capture phase, after the suggestion menu
            // gets its turn.
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if !this.menu_action(Box::new(MoveUp), window, cx) {
                    this.move_selected(-1, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if !this.menu_action(Box::new(MoveDown), window, cx) {
                    this.move_selected(1, cx);
                }
            }))
            .capture_action(
                cx.listener(|this, _: &MovePageUp, _, cx| this.move_selected(-PAGE_ROWS, cx)),
            )
            .capture_action(
                cx.listener(|this, _: &MovePageDown, _, cx| this.move_selected(PAGE_ROWS, cx)),
            )
            // The input binds select-all to its text; hosted, the chord takes
            // every row instead.
            .capture_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if this.hosted {
                    this.select_all(cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(DismissEvent);
                }
            }))
            .child(
                div()
                    .p(tokens::SPACE_SM)
                    .border_b_1()
                    .border_color(palette::border())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.search.update(cx, |search, cx| search.element(cx))),
                            )
                            .child(self.config_button(cx)),
                    )
                    .when_some(
                        self.seed.as_ref().map(|seed| seed.label.clone()),
                        |d, label| d.child(self.seed_chip(label, cx)),
                    )
                    .when(self.show_config, |d| d.child(self.config_panel(cx))),
            )
            .child(list)
            .when(self.show_syntax, |d| d.child(self.syntax_sheet(cx)))
            .child(self.hint_row(cx))
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .px(tokens::SPACE_SM)
                        .py(tokens::SPACE_XS)
                        .border_t_1()
                        .border_color(palette::border())
                        .text_color(palette::text_muted())
                        .child(error),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_library::rusqlite::Connection;
    use rox_library::{TrackRow, store};

    fn track(path: &str, album_artist: &str, album: &str) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
            path: path.into(),
            sub: 0,
            cue: None,
            title: "Song".into(),
            artist: album_artist.into(),
            album_artist: album_artist.into(),
            album: album.into(),
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            genre: String::new(),
            year: 0,
            disc_no: 1,
            track_no: 1,
            duration_ms: 200_000,
            codec: "mp3".into(),
            bitrate_kbps: 320,
            sample_rate_hz: 44100,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    fn projection(rows: &[TrackRow]) -> Projection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    #[test]
    fn a_seed_keeps_its_own_rows_and_the_groups_they_sit_in() {
        let p = projection(&[
            track("/m/a/1.mp3", "In", "First"),
            track("/m/a/2.mp3", "In", "Second"),
            track("/m/b/1.mp3", "Out", "Third"),
            track("/m/b/2.mp3", "Out", "Fourth"),
        ]);
        let row_of = |album: &str| {
            (0..p.len() as u32)
                .find(|&row| p.albums.strings[p.album[row as usize] as usize] == album)
                .expect("row present")
        };
        let (kept, dropped) = (row_of("First"), row_of("Third"));
        let seed = seed_rows(&p, &[p.db_id[kept as usize]]);

        assert!(seed.keeps(kept));
        assert!(!seed.keeps(dropped));
        assert!(!seed.keeps(p.len() as u32 + 10));

        let head = |row: u32| Head::Album {
            album_artist: p.album_artist[row as usize],
            album: p.album[row as usize],
            row,
        };
        assert!(seed.keeps_head(head(kept)));
        assert!(!seed.keeps_head(head(dropped)));
        assert!(seed.keeps_head(Head::Artist {
            album_artist: p.album_artist[kept as usize],
            row: kept,
        }));
        assert!(!seed.keeps_head(Head::Artist {
            album_artist: p.album_artist[dropped as usize],
            row: dropped,
        }));
    }

    #[test]
    fn a_click_takes_a_row_a_toggle_or_a_range() {
        let (set, anchor) = click_selection(&HashSet::new(), None, 3, false, false);
        assert_eq!(set, HashSet::from([3]));
        assert_eq!(anchor, Some(3));

        let (set, anchor) = click_selection(&set, anchor, 5, false, true);
        assert_eq!(set, HashSet::from([3, 5]));
        assert_eq!(anchor, Some(5));
        let (off, _) = click_selection(&set, anchor, 5, false, true);
        assert_eq!(off, HashSet::from([3]));

        let (set, anchor) = click_selection(&set, Some(2), 4, true, false);
        assert_eq!(set, HashSet::from([2, 3, 4]));
        assert_eq!(anchor, Some(2));
        let (set, anchor) = click_selection(&set, anchor, 0, true, false);
        assert_eq!(set, HashSet::from([0, 1, 2]));
        assert_eq!(anchor, Some(2));

        let (set, anchor) = click_selection(&HashSet::new(), None, 7, true, false);
        assert_eq!(set, HashSet::from([7]));
        assert_eq!(anchor, Some(7));

        let (set, anchor) = click_selection(&HashSet::from([0, 1]), Some(5), 7, true, true);
        assert_eq!(set, HashSet::from([0, 1, 5, 6, 7]));
        assert_eq!(anchor, Some(5));
    }

    #[test]
    fn the_cursor_follows_a_toggle_off_to_a_lit_row() {
        let set = HashSet::from([1, 4, 9]);
        assert_eq!(nearest_selected(&set, 5), Some(4));
        assert_eq!(nearest_selected(&set, 7), Some(9));
        assert_eq!(nearest_selected(&HashSet::from([2, 4]), 3), Some(2));
        assert_eq!(nearest_selected(&HashSet::new(), 3), None);
    }

    /// An empty seed narrows to nothing, not to "no seed".
    #[test]
    fn an_empty_seed_keeps_nothing() {
        let p = projection(&[track("/m/a/1.mp3", "In", "First")]);
        let seed = seed_rows(&p, &[]);
        assert!(!seed.keeps(0));
        assert!(seed.artists.is_empty());
        assert!(seed.albums.is_empty());
    }
}
