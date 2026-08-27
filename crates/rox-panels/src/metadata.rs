//! The metadata panel: the current track's tags laid out as a sheet, with
//! title and artist up top, then the labeled fields the library has
//! (album, genre, year, duration, codec, bitrate). What it describes is
//! per-view config through [`MetadataSource`]: the playing track, the
//! selected one, or the library as a whole, where the sheet zooms out to
//! the catalog's counts, so a duplicate can watch each. The background
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
    div, img, prelude::*, px, AnyElement, App, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, Image, ImageFormat, KeyDownEvent, ObjectFit, SharedString, Stateful, Subscription,
    WeakEntity, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side, Sizable};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::projection::FilterField;
use rox_library::writer::{self, Change, Field};
use serde::{Deserialize, Serialize};

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
/// `track_columns::checklist`/`columns_submenu` want a `'static` slice, so
/// this rebuilds and leaks once per active locale rather than on every
/// call, mirroring `rox_i18n::t_static`'s own per-locale cache.
fn fields() -> Vec<Column> {
    vec![
        Column {
            key: "album",
            label: rox_i18n::t!("head-piece-album"),
            default_on: true,
        },
        Column {
            key: "album_artist",
            label: rox_i18n::t!("filter-field-album-artist"),
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
    ]
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
}

/// The editable fields in sheet order, each with its input row's label:
/// the tags the panel shows plus the comment, which is only stored in the
/// file. Duration, codec, and bitrate stay display-only, they describe
/// the stream. A plain function rather than a `const`: `t_static` isn't
/// const-evaluable, and nothing outside this file needs the slice itself
/// to be `'static`, so it just rebuilds (cheaply, `t_static` caches the
/// strings) on each call.
fn edit_fields() -> Vec<(Field, gpui::SharedString)> {
    vec![
        (Field::Title, rox_i18n::t!("info-item-title")),
        (Field::Artist, rox_i18n::t!("head-piece-artist")),
        (Field::Album, rox_i18n::t!("head-piece-album")),
        (
            Field::AlbumArtist,
            rox_i18n::t!("filter-field-album-artist"),
        ),
        (Field::DiscNo, rox_i18n::t!("metadata-field-disc")),
        (Field::TrackNo, rox_i18n::t!("metadata-field-track")),
        (Field::Genre, rox_i18n::t!("head-piece-genre")),
        (Field::Year, rox_i18n::t!("head-piece-year")),
        (Field::Comment, rox_i18n::t!("metadata-field-comment")),
    ]
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
    /// A commit is in flight; the buttons hold still until it finishes.
    saving: bool,
    _input_events: Vec<Subscription>,
}

pub struct MetadataPanel {
    state: AppState,
    config: MetadataConfig,
    /// The in-progress edit while the sheet shows its edit face.
    edit: Option<EditState>,
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
            details: None,
            totals: None,
            art: panel::TrackedImage::default(),
            resolved: ResolvedTrack::default(),
            focus: cx.focus_handle(),
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
                let row = projection.db_id.iter().position(|&db_id| db_id == id)?;
                let v = projection.resolve(row as u32);
                Some(Details {
                    title: v.title.to_owned(),
                    artist: v.artist.to_owned(),
                    album_artist: v.album_artist.to_owned(),
                    album: v.album.to_owned(),
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
            for ix in 0..projection.db_id.len() {
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
                tracks: projection.db_id.len(),
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
        let submenu = track_columns::columns_submenu(fields(), window, cx);
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
        let mut changes = Vec::new();
        for ((field, _), input) in edit_fields().iter().zip(&edit.inputs) {
            let value = input.read(cx).value().to_string();
            let original = baseline
                .iter()
                .find(|(f, _)| f == field)
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            if value == original {
                continue;
            }
            changes.push(Change {
                field: field.clone(),
                value: (!value.is_empty()).then_some(value),
            });
        }
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
                track_columns::checklist(&fields(), self, cx),
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
    search: Option<Search>,
    query: Option<&Entity<SharedQuery>>,
    next_id: &mut usize,
) -> Div {
    let (Some(search), Some(query)) = (search, query) else {
        return div().min_w_0().truncate().child(value.to_string());
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
                .child(term),
        );
    }
    row
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
        panel::themed(&chrome, || self.body(cx))
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
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .when(headerless && show_toggle, |d| d.child(self.toolbar(cx)))
            .child(self.sheet_body(cx).flex_1().min_h_0())
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

        // The shown fields in registry order, each skipped when its value
        // is empty: absence reads cleaner than a labeled blank. The key
        // comes along for [`query_field`], which decides whether the value
        // is clickable.
        let mut fields: Vec<(gpui::SharedString, String, Option<Search>)> = Vec::new();
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
                    "plays" => (d.plays > 0).then(|| track_columns::fmt_plays(d.plays)),
                    "rating" => (d.rating > 0).then(|| crate::rating_ui::fmt(d.rating).to_string()),
                    _ => None,
                }),
            };
            if let Some(value) = value {
                fields.push((col.label.clone(), value, query_field(col.key)));
            }
        }
        let artist = details
            .as_ref()
            .map(|d| d.artist.clone())
            .filter(|a| !a.is_empty());
        // The title only searches when the library has the track; for one
        // it doesn't the line is the file name, which no tag holds.
        let title_field = details.as_ref().and_then(|_| query_field("title"));

        // The table face: title and artist fold in as rows, the list goes
        // where the vertical knob puts it, and it scrolls when the panel
        // runs short.
        if self.config.display == MetadataDisplay::Table {
            let mut rows: Vec<(gpui::SharedString, String, Option<Search>)> = Vec::new();
            rows.push((rox_i18n::t!("info-item-title"), title, title_field));
            if let Some(artist) = artist {
                rows.push((
                    rox_i18n::t!("head-piece-artist"),
                    artist,
                    query_field("artist"),
                ));
            }
            rows.extend(fields);
            let stripes = self.config.stripes;
            let borders = self.config.row_borders;
            let rows: Vec<Div> = rows
                .into_iter()
                .enumerate()
                .map(|(ix, (label, value, field))| {
                    let cell = value_cell(&value, field, query.as_ref(), &mut hit_id);
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

        // The sheet: title over artist, the fields below, placed by the
        // two alignment knobs. The cells build up front so each one gets
        // its turn at the shared hit-id counter.
        let title_cell = value_cell(&title, title_field, query.as_ref(), &mut hit_id)
            .text_lg()
            .text_color(palette::text_bright())
            .max_w_full();
        let artist_cell = artist.map(|artist| {
            value_cell(&artist, query_field("artist"), query.as_ref(), &mut hit_id)
                .text_color(palette::text_muted())
                .max_w_full()
        });
        let rows: Vec<Div> = fields
            .into_iter()
            .map(|(label, value, search)| {
                field(
                    label,
                    value_cell(&value, search, query.as_ref(), &mut hit_id),
                )
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
                    let cell = value_cell(&value, None, None, &mut hit_id);
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
            None,
            None,
            &mut hit_id,
        )
        .text_lg()
        .text_color(palette::text_bright())
        .max_w_full();
        let rows: Vec<Div> = fields
            .into_iter()
            .map(|(label, value)| field(label, value_cell(&value, None, None, &mut hit_id)))
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
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{MetadataConfig, MetadataSource};

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
}
