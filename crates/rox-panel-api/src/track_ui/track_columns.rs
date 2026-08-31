//! The track columns and album grouping shared by the track-list panels
//! (playlists, queue, history). Each panel keeps its own row type, data
//! source, and interactions; this owns the parts that would otherwise drift
//! across copies: the per-column cell render, the consecutive-run album
//! grouping and its two-line heading rows, and the settings checklist and
//! right-click Columns and Headings menus, wired through small host traits.

use std::path::PathBuf;

use gpui::{
    div, img, prelude::*, px, svg, AnyElement, Context, Div, Entity, MouseButton, ObjectFit,
    Pixels, SharedString, Stateful, Window,
};
use gpui_component::menu::PopupMenu;
use gpui_component::Side;
use rox_core::fmt::fmt_ms;

use crate::group_head::{self, HeadPiece, Headers};
use crate::panel::{self, AppState};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::ui as settings_ui;
use rox_services::thumbs::Thumb;

/// The slider bounds for the row and header-line heights, px at the stock
/// font size, and the stock height itself: what the rows draw at out of
/// the box, and the height whose text is the stock 1 rem (the row and
/// header text scale off their height's ratio to this). The album block is
/// two rows of the stock height, so its cover tile spans a two-row square.
/// The render sites run these through [`palette::scaled_px`] so rows,
/// headings, and tiles grow with the app font, the same way the library
/// table scales its rows. Shared by every panel that offers the row and
/// header appearance knobs.
pub const ROW_HEIGHT_MIN: f32 = 18.;
pub const ROW_HEIGHT_MAX: f32 = 48.;
pub const ROW_HEIGHT_STOCK: f32 = 30.;
pub const HEAD_HEIGHT_MAX: f32 = 72.;

/// The gap and margin sliders' ceilings, same units: the open space over
/// and under a header block, and the cover tile's inset inside the block.
pub const HEAD_GAP_MAX: f32 = 24.;
pub const ART_MARGIN_MAX: f32 = 16.;

/// The row spacing slider's ceiling: extra height grown into each row,
/// which the row fills; the text keeps the size the row height sets.
pub const ROW_SPACING_MAX: f32 = 32.;

/// The header text slider's range and stock value, px at the stock font
/// size. The stock is the 1 rem the lines drew before the knob existed.
pub const HEAD_TEXT_MIN: f32 = 8.;
pub const HEAD_TEXT_MAX: f32 = 32.;
pub const HEAD_TEXT_STOCK: f32 = 16.;

/// How many expanded line slots the config holds and the editors show.
pub const HEAD_LINE_SLOTS: usize = 3;

/// A saved header text size read back clamped to the slider's range;
/// nonsense in a hand-edited dump falls to the stock size.
pub fn fold_head_text(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(
            HEAD_TEXT_MIN,
            settings_ui::ceiling(HEAD_TEXT_MIN, HEAD_TEXT_MAX),
        )
    } else {
        HEAD_TEXT_STOCK
    }
}

/// A saved margin knob read back clamped to the band its input allows,
/// not the strip's own top, so a typed value is kept across a reload;
/// nonsense in a hand-edited dump falls to zero.
pub fn fold_margin(v: f32, max: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0., settings_ui::ceiling(0., max))
    } else {
        0.
    }
}

/// One saved height read back clamped to its slider's band, the ceiling
/// the input allows rather than the strip's own top so a typed value
/// survives a reload. Missing or nonsense falls to the default the caller
/// hands in, which is how a panel folds the row height into the header
/// line's default without repeating the clamp.
pub fn fold_row_height(v: Option<f32>, default: f32, max: f32) -> f32 {
    match v {
        Some(v) if v.is_finite() => {
            v.clamp(ROW_HEIGHT_MIN, settings_ui::ceiling(ROW_HEIGHT_MIN, max))
        }
        _ => default,
    }
}

/// One toggleable column: its config key, its menu and settings label, and
/// whether a fresh panel shows it. A panel's registry fixes the render order.
pub struct Column {
    pub key: &'static str,
    /// Display text, resolved by whoever builds the registry. Owned rather
    /// than `&'static str` so a registry can be rebuilt per locale without
    /// each panel leaking its own copy behind a cache.
    pub label: SharedString,
    pub default_on: bool,
}

/// A registry's default-on keys, in order, for a fresh config.
pub fn default_columns(columns: &[Column]) -> Vec<String> {
    columns
        .iter()
        .filter(|c| c.default_on)
        .map(|c| c.key.to_string())
        .collect()
}

/// The common column values a shared cell draws. A panel fills this per row
/// from its own data and draws any panel-only columns (history's plays and
/// when) itself, falling back to [`cell`] for the shared keys.
pub struct Cell<'a> {
    pub pos: u32,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub year: u16,
    pub genre: &'a str,
    pub duration_ms: u32,
    pub rating: u8,
    pub track_id: i64,
    pub favourite: bool,
    pub playing: bool,
    /// The total play count, for the plays column; 0 hides it.
    pub plays: u32,
    /// The track's cover thumbnail, resolved by the panel (which holds the
    /// context and the path) when the cover column shows; None otherwise.
    pub cover: Option<Thumb>,
}

/// Render one shared column, or None when the key is a panel's own. The text
/// columns flex and truncate; number, year, and duration get fixed slots;
/// rating and favourite hand off to the shared controls, which write through
/// `state`. `row_height` is the caller's own row height at the stock font
/// size, which only the cover square reads; a panel with no height knob
/// hands over [`ROW_HEIGHT_STOCK`]. `compact_plays` swaps the plays column
/// for the library's tick face; a panel without that knob passes false and
/// keeps the plain readout.
pub fn cell(
    key: &str,
    c: &Cell,
    state: &AppState,
    row_height: f32,
    compact_plays: bool,
) -> Option<Div> {
    let text = |value: &str, color: gpui::Rgba| {
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(color)
            .child(SharedString::from(value.to_string()))
    };
    Some(match key {
        "cover" => cover_cell(&c.cover, row_height),
        // The compact face shrinks the count and hangs a faint bar beside
        // it, the library's "1|" playlist tick; the plain face spells the
        // count out. Either way a never-played track reads as absence
        // rather than a zero.
        "plays" if compact_plays => div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(1.))
            .when(c.plays > 0, |d| {
                d.child(div().text_xs().text_color(palette::text_muted()).child(
                    SharedString::from(rox_i18n::format::format_int(c.plays as i64)),
                ))
                .child(div().text_xs().text_color(palette::text_faint()).child("|"))
            }),
        "plays" => div()
            .flex_none()
            .text_color(palette::text_muted())
            .child(SharedString::from(fmt_plays(c.plays))),
        "number" => div()
            .flex_none()
            .w(px(22.))
            .flex()
            .justify_end()
            .text_color(palette::text_muted())
            .child(SharedString::from(c.pos.to_string())),
        "name" => div()
            .flex_1()
            .min_w_0()
            .truncate()
            .when(c.playing, |d| d.text_color(palette::accent()))
            .child(SharedString::from(c.title.to_string())),
        "artist" => text(c.artist, palette::text_secondary()),
        "album" => text(c.album, palette::text_secondary()),
        "genre" => text(c.genre, palette::text_muted()),
        "year" => div()
            .flex_none()
            .text_color(palette::text_muted())
            .child(SharedString::from(if c.year == 0 {
                String::new()
            } else {
                c.year.to_string()
            })),
        // A zero length reads as unknown, not a real 0:00 (the scanner
        // leaves it zero when it can't read a file's tags), so the slot
        // stays blank like the year does, keeping its width for alignment.
        "duration" => {
            div()
                .flex_none()
                .text_color(palette::text_muted())
                .child(SharedString::from(if c.duration_ms == 0 {
                    String::new()
                } else {
                    fmt_ms(c.duration_ms)
                }))
        }
        "rating" => crate::track_ui::track_cells::rating(state.clone(), c.track_id, c.rating),
        "favourite" => {
            crate::track_ui::track_cells::favourite(state.clone(), c.track_id, c.favourite)
        }
        _ => return None,
    })
}

/// A small rounded cover square, the album tile cut to one row. The panel
/// resolves the thumbnail; pending and missing use the quiet placeholder so
/// a cover that arrives later fills without shifting the row. Shared with the library
/// table's cover column, which draws outside [`cell`].
///
/// The mask is the square: `Cover` overruns the element on the art's long
/// side, and gpui paints that overrun rather than cropping it, so a wide
/// sleeve would run out over the title beside it. The box does that
/// masking, not the image, which can only mask against its own overrun
/// bounds.
///
/// `row_height` is the caller's row height at the stock font size, so the
/// square follows a panel's height knob instead of a constant that only
/// happens to match at the stock setting; the 6 px is the breathing room
/// above and below it.
pub fn cover_cell(cover: &Option<Thumb>, row_height: f32) -> Div {
    let side = palette::scaled_px(row_height - 6.);
    let content: AnyElement = match cover {
        Some(Thumb::Ready(image)) => div()
            .size(side)
            .overflow_hidden()
            .child(
                img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .rounded(px(3.)),
            )
            .into_any_element(),
        _ => div()
            .size(side)
            .rounded(px(3.))
            .bg(palette::bg_control())
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(icons::MUSIC)
                    .size(px(12.))
                    .text_color(palette::text_faint()),
            )
            .into_any_element(),
    };
    div().flex_none().flex().items_center().child(content)
}

/// A play count as a short readout, blank when never played.
pub fn fmt_plays(plays: u32) -> String {
    match plays {
        0 => String::new(),
        1 => "1 play".to_string(),
        n => format!("{n} plays"),
    }
}

/// Resolve a track's cover thumbnail for the [`Cell::cover`] slot, or None
/// when the cover column is off or the track has no path. The panel calls
/// this from its row build, where the context and the file path are at hand.
pub fn cover_thumb<P: 'static>(
    state: &AppState,
    path: Option<&std::path::Path>,
    shown: bool,
    cx: &mut Context<P>,
) -> Option<Thumb> {
    let path = shown.then_some(path).flatten()?;
    Some(state.thumbs.update(cx, |thumbs, cx| thumbs.get(path, cx)))
}

/// One album run's heading aggregates, what its two rows draw. Rebuilt each
/// refresh from the run's tracks.
pub struct AlbumGroup {
    pub album: String,
    /// The album artist, or the first track's artist when the album artist
    /// tag is empty, the library's fallback.
    pub artist: String,
    pub year: u16,
    pub genre: String,
    pub quality: String,
    pub tracks: u32,
    pub total_ms: u64,
    pub first_track_id: i64,
    /// Resolved art path, cached on the first paint: outer None not yet
    /// resolved, inner None no art.
    pub art: Option<Option<PathBuf>>,
}

/// One track's grouping inputs, a borrowed view a panel builds per member.
pub struct GroupTrack<'a> {
    pub album: &'a str,
    pub album_artist: &'a str,
    pub artist: &'a str,
    pub year: u16,
    pub genre: &'a str,
    pub codec: &'a str,
    pub bitrate_kbps: u16,
    pub sample_rate_hz: u32,
    pub bit_depth: u8,
    pub duration_ms: u32,
    pub track_id: i64,
}

/// Aggregate a run of same-album tracks into a heading group: the first
/// track names it, the run sums the time and spans the codec, the stream
/// shape, and the bitrate.
pub fn album_group(run: &[GroupTrack]) -> AlbumGroup {
    let first = &run[0];
    let mut codec: Option<&str> = Some(first.codec);
    let (mut bit_depth, mut sample_rate_hz) = (first.bit_depth, first.sample_rate_hz);
    let (mut min_kbps, mut max_kbps, mut total_ms) = (0u16, 0u16, 0u64);
    for t in run {
        if codec != Some(t.codec) {
            codec = None;
        }
        // Depth and rate are all-or-nothing across the run, like the
        // codec: a mixed album has no one shape to name.
        if bit_depth != t.bit_depth {
            bit_depth = 0;
        }
        if sample_rate_hz != t.sample_rate_hz {
            sample_rate_hz = 0;
        }
        if t.bitrate_kbps > 0 {
            min_kbps = if min_kbps == 0 {
                t.bitrate_kbps
            } else {
                min_kbps.min(t.bitrate_kbps)
            };
            max_kbps = max_kbps.max(t.bitrate_kbps);
        }
        total_ms += t.duration_ms as u64;
    }
    let artist = if first.album_artist.is_empty() {
        first.artist
    } else {
        first.album_artist
    };
    AlbumGroup {
        album: first.album.to_string(),
        artist: artist.to_string(),
        year: first.year,
        genre: first.genre.to_string(),
        quality: group_head::quality(
            codec.filter(|c| !c.is_empty()),
            min_kbps,
            max_kbps,
            bit_depth,
            sample_rate_hz,
        ),
        tracks: run.len() as u32,
        total_ms,
        first_track_id: first.track_id,
        art: None,
    }
}

/// The heading look the tree panels drew before any of them had appearance
/// knobs: the cover tile two stock rows tall, square corners hard against
/// the block's left edge, every part shown, text at the stock rem. A panel
/// without its own config hands this to [`album_name_row`] and
/// [`album_meta_row`]; one that grows knobs builds its own [`HeadLook`] the
/// way the library table does, and the shared rows keep drawing the same
/// shape either way.
///
/// [`HeadLook`]: group_head::HeadLook
pub fn stock_head_look() -> group_head::HeadLook {
    group_head::HeadLook {
        tile_side: palette::scaled_px(ROW_HEIGHT_STOCK * 2.),
        show_art: true,
        show_year: true,
        show_details: true,
        line_px: palette::scaled_px(ROW_HEIGHT_STOCK),
        art_side: group_head::ArtSide::Left,
        art_margin: px(0.),
        art_rounding: 0.,
        font_scale: 1.,
    }
}

fn head_of(g: &AlbumGroup) -> group_head::GroupHead {
    group_head::GroupHead {
        name: SharedString::from(g.artist.clone()),
        album: SharedString::from(g.album.clone()),
        year: g.year,
        genre: SharedString::from(g.genre.clone()),
        quality: SharedString::from(g.quality.clone()),
        tracks: g.tracks,
        total_ms: g.total_ms,
        tiled: true,
        thumb: None,
    }
}

/// One half of an album run's cover tile, resolving the run's first track to
/// a path once and caching it on the group, the library's route. The side,
/// corners, inset, and which edge it hangs off all come off the look, so a
/// panel's knobs reach the tile without this knowing about any config.
fn tile<P: 'static>(
    group: &mut AlbumGroup,
    state: &AppState,
    look: &group_head::HeadLook,
    bottom: bool,
    cx: &mut Context<P>,
) -> AnyElement {
    let path = match group.art.clone() {
        Some(path) => path,
        None => {
            // No album tag is the unknown bucket, not a real album: keep the
            // placeholder rather than a loose track's art.
            let path = (!group.album.is_empty())
                .then(|| {
                    state
                        .library
                        .read(cx)
                        .paths_for(&[group.first_track_id])
                        .ok()
                })
                .flatten()
                .and_then(|mut paths| paths.pop());
            group.art = Some(path.clone());
            path
        }
    };
    let thumb = match path {
        Some(path) => state.thumbs.update(cx, |thumbs, cx| thumbs.get(&path, cx)),
        None => Thumb::Missing,
    };
    // The block's rows each paint the whole square; the meta row's copy
    // starts one line higher, which is what makes the two halves line up.
    let lift = if bottom { look.line_px } else { px(0.) };
    group_head::tile(
        thumb,
        look.tile_side,
        look.art_rounding,
        lift,
        look.art_side,
        look.art_margin,
    )
}

/// Where one heading line sits inside the list row that carries it, plus
/// what that line draws.
///
/// The tree panels hang their rows off a `uniform_list`, which lays every
/// row out at one measured height: a heading line can't claim a row taller
/// than a track's the way the library table's per-row height hook lets it.
/// So the line is drawn as a strip inside the row instead: `row_px` is the
/// row the list laid out, `content_top` where the strip starts in it (the
/// gap over the block, or negative on a second line that has to climb back
/// up to meet the first), and `look.line_px` how tall the strip is. What's
/// left over shows the list through, which is what makes the gap knobs
/// read.
pub struct HeadSlot<'a> {
    /// The composed pieces this line draws, left to right.
    pub pieces: &'a [HeadPiece],
    pub look: &'a group_head::HeadLook,
    pub row_px: Pixels,
    pub content_top: Pixels,
    /// Draw the strip on the list background instead of the raised tint,
    /// the library's flush headers.
    pub flush: bool,
}

impl<'a> HeadSlot<'a> {
    /// The slot the tree panels drew before any of them had appearance
    /// knobs: the line filling its whole row, hard against the top, on the
    /// raised tint. Pairs with [`stock_head_look`].
    pub fn stock(pieces: &'a [HeadPiece], look: &'a group_head::HeadLook) -> Self {
        HeadSlot {
            pieces,
            look,
            row_px: palette::scaled_px(ROW_HEIGHT_STOCK),
            content_top: px(0.),
            flush: false,
        }
    }

    /// The strip laid over the row: the tint (skipped when it would be a
    /// second coat of the list's own color, the library's rule) with the
    /// composed line over it, both clipped to the slot's own height so the
    /// gaps stay open.
    fn strip(&self, content: Div) -> Div {
        // Flush means the list's own color, which the panel body has
        // already painted under every row: painting it again lays a second
        // coat, which stops matching the moment surfaces go translucent.
        // So flush paints nothing rather than painting bg_root.
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(self.content_top)
            .h(self.look.line_px)
            .when(!self.flush, |d| d.bg(palette::bg_elevated()))
            .child(content)
    }
}

/// An album run's name line. Expanded opens the two-row cover tile and gives
/// the album artist the line; Compact draws the packed line alone, no tile.
///
/// The look carries the whole appearance, the line height included, and the
/// slot where the line sits in its row, so a panel that has grown knobs and
/// one that hasn't call this the same way; the one without hands over
/// [`stock_head_look`] and [`HeadSlot::stock`].
pub fn album_name_row<P: 'static>(
    ix: usize,
    group: &mut AlbumGroup,
    headers: Headers,
    slot: &HeadSlot,
    state: &AppState,
    cx: &mut Context<P>,
) -> Stateful<Div> {
    let expanded = headers == Headers::Expanded;
    let tile = (expanded && slot.look.show_art).then(|| tile(group, state, slot.look, false, cx));
    let head = head_of(group);
    div()
        .id(("album-head", ix))
        .relative()
        .w_full()
        .h(slot.row_px)
        .child(
            slot.strip(group_head::line_content(
                slot.pieces,
                &head,
                slot.look,
                expanded,
            ))
            .when_some(tile, |d, tile| d.child(tile)),
        )
}

/// The run's meta line: the album, genre, quality, track count, and total
/// time over the tile's bottom half. Only Expanded pushes this row.
pub fn album_meta_row<P: 'static>(
    ix: usize,
    group: &mut AlbumGroup,
    slot: &HeadSlot,
    state: &AppState,
    cx: &mut Context<P>,
) -> Stateful<Div> {
    let tile = slot
        .look
        .show_art
        .then(|| tile(group, state, slot.look, true, cx));
    let head = head_of(group);
    div()
        .id(("album-meta", ix))
        .relative()
        .w_full()
        .h(slot.row_px)
        .child(
            slot.strip(group_head::line_content(
                slot.pieces,
                &head,
                slot.look,
                true,
            ))
            .when_some(tile, |d, tile| d.child(tile)),
        )
}

/// The stock pieces a heading's name row draws in a mode: the packed
/// compact row, or the expanded block's name line. What a panel with no
/// composition config of its own hands [`album_name_row`].
pub fn stock_name_pieces(headers: Headers) -> Vec<HeadPiece> {
    if headers == Headers::Expanded {
        group_head::stock_name_line()
    } else {
        group_head::stock_compact()
    }
}

/// A panel that stores a shown-column set the shared menus edit.
pub trait ColumnHost: 'static + Sized {
    fn column_shown(&self, key: &str) -> bool;
    fn set_column(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>);
}

/// A panel that stores an album heading mode the shared menu edits.
pub trait HeadingHost: 'static + Sized {
    fn headers(&self) -> Headers;
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>);
}

/// The View-page column checklist: a tick per registry column, a click
/// flipping it. The panel's own registry fixes the set and order.
pub fn checklist<P: ColumnHost>(columns: &[Column], panel: &P, cx: &mut Context<P>) -> Div {
    let mut list = div().flex().flex_col().gap(tokens::SPACE_XS);
    for col in columns {
        let key = col.key;
        let on = panel.column_shown(key);
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
                    cx.listener(move |this: &mut P, _, _, cx| {
                        let on = this.column_shown(key);
                        this.set_column(key, !on, cx);
                    }),
                )
                .child(settings_ui::checkbox(on))
                .child(
                    div()
                        .text_color(if on {
                            palette::text()
                        } else {
                            palette::text_muted()
                        })
                        .child(col.label.clone()),
                ),
        );
    }
    list
}

/// The right-click Columns submenu: a live-checked row per registry column,
/// tracking the panel so a flip shows without the menu reopening.
pub fn columns_submenu<P: ColumnHost>(
    columns: Vec<Column>,
    window: &mut Window,
    cx: &mut Context<P>,
) -> Entity<PopupMenu> {
    let panel = cx.entity();
    PopupMenu::build(window, cx, move |mut submenu, _, cx| {
        panel::follow_panel(&panel, cx);
        for col in &columns {
            let key = col.key;
            submenu = submenu.item(panel::check_row(
                col.label.clone(),
                None,
                move |this: &P| this.column_shown(key),
                move |this, cx| {
                    let on = this.column_shown(key);
                    this.set_column(key, !on, cx);
                },
                &panel,
            ));
        }
        submenu
    })
}

/// The right-click Headings submenu: Off, Compact, Expanded, one live check
/// on the active mode, the library's Headers flyout.
pub fn headings_submenu<P: HeadingHost>(
    window: &mut Window,
    cx: &mut Context<P>,
) -> Entity<PopupMenu> {
    let panel = cx.entity();
    PopupMenu::build(window, cx, move |submenu, _, cx| {
        panel::follow_panel(&panel, cx);
        let mut submenu = submenu.check_side(Side::Right);
        for (headers, name) in [
            (Headers::Off, rox_i18n::t!("headers-off")),
            (Headers::Compact, rox_i18n::t!("headers-compact")),
            (Headers::Expanded, rox_i18n::t!("headers-expanded")),
        ] {
            submenu = submenu.item(panel::check_row(
                name,
                None,
                move |this: &P| this.headers() == headers,
                move |this, cx| this.set_headers(headers, cx),
                &panel,
            ));
        }
        submenu
    })
}
