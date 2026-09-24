//! The track columns and album grouping shared by the track-list panels
//! (playlists, queue, history): the per-column cell render, the
//! consecutive-run album grouping and its heading rows, and the Columns and
//! Headings menus. Each panel keeps its own row type, data, and interactions.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, Div, Entity, MouseButton, ObjectFit, Pixels, SharedString, Stateful,
    Window, div, img, prelude::*, px, svg,
};
use gpui_component::Side;
use gpui_component::menu::PopupMenu;
use rox_core::fmt::fmt_ms;

use crate::group_head::{self, HeadPiece, Headers};
use crate::panel::{self, AppState};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::ui as settings_ui;
use rox_services::thumbs::Thumb;

/// Row and header-line height bounds, px at the stock font size. The stock
/// row height carries the stock 1 rem text, and row and header text scale off
/// their height's ratio to it. Render sites run these through
/// [`palette::scaled_px`] so everything grows with the app font.
pub const ROW_HEIGHT_MIN: f32 = 18.;
pub const ROW_HEIGHT_MAX: f32 = 48.;
pub const ROW_HEIGHT_STOCK: f32 = 30.;
pub const HEAD_HEIGHT_MAX: f32 = 72.;

/// Ceilings for the gap over and under a header block and the cover tile's
/// inset inside it.
pub const HEAD_GAP_MAX: f32 = 24.;
pub const ART_MARGIN_MAX: f32 = 16.;

/// Extra height grown into each row. The text keeps the size the row height
/// sets.
pub const ROW_SPACING_MAX: f32 = 32.;

pub const HEAD_TEXT_MIN: f32 = 8.;
pub const HEAD_TEXT_MAX: f32 = 32.;
pub const HEAD_TEXT_STOCK: f32 = 16.;

pub const HEAD_LINE_SLOTS: usize = 3;

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

/// Clamped to the input's band, not the strip's own top, so a typed value
/// survives a reload.
pub fn fold_margin(v: f32, max: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0., settings_ui::ceiling(0., max))
    } else {
        0.
    }
}

/// Clamped like [`fold_margin`]. Missing or nonsense falls to `default`,
/// which lets a panel fold the row height into the header line's default.
pub fn fold_row_height(v: Option<f32>, default: f32, max: f32) -> f32 {
    match v {
        Some(v) if v.is_finite() => {
            v.clamp(ROW_HEIGHT_MIN, settings_ui::ceiling(ROW_HEIGHT_MIN, max))
        }
        _ => default,
    }
}

/// A panel's registry fixes the render order.
pub struct Column {
    pub key: &'static str,
    /// Owned so a registry can be rebuilt per locale without each panel
    /// leaking a copy.
    pub label: SharedString,
    pub default_on: bool,
}

pub fn default_columns(columns: &[Column]) -> Vec<String> {
    columns
        .iter()
        .filter(|c| c.default_on)
        .map(|c| c.key.to_string())
        .collect()
}

/// A panel draws its own columns (history's plays and when) and falls back
/// to [`cell`] for the shared keys.
pub struct Cell<'a> {
    pub pos: u32,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    /// Sort names, drawn as a reading when the switch is on. Empty where the
    /// projection holds no row.
    pub title_reading: &'a str,
    pub artist_reading: &'a str,
    pub album_reading: &'a str,
    pub year: u16,
    pub genre: &'a str,
    pub duration_ms: u32,
    pub rating: u8,
    pub track_id: i64,
    pub favourite: bool,
    pub playing: bool,
    /// 0 leaves the plays cell blank.
    pub plays: u32,
    /// Resolved by the panel when the cover column shows.
    pub cover: Option<Thumb>,
}

/// Render one shared column, or None when the key is a panel's own.
/// `row_height` is the row height at the stock font size, which only the
/// cover reads. `compact_plays` swaps in the library's tick face for plays.
pub fn cell(
    key: &str,
    c: &Cell,
    state: &AppState,
    row_height: f32,
    compact_plays: bool,
) -> Option<Div> {
    let readings = rox_core::settings::show_readings();
    let text = |value: &str, reading: &str, color: gpui::Rgba| {
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(color)
            .child(crate::panel::named(value, reading, readings))
    };
    let numeric = |width: f32, value: String| numeric_cell(width, palette::text_muted(), value);
    Some(match key {
        "cover" => cover_cell(&c.cover, row_height),
        // The compact face is the library's "1|" playlist tick. A never-played
        // track reads as absence rather than a zero.
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
        "plays" => numeric(PLAYS_WIDTH, fmt_plays(c.plays)),
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
            .child(crate::panel::named(c.title, c.title_reading, readings)),
        "artist" => text(c.artist, c.artist_reading, palette::text_secondary()),
        "album" => text(c.album, c.album_reading, palette::text_secondary()),
        // Genres are interned without sort names, so there's never a reading.
        "genre" => text(c.genre, "", palette::text_muted()),
        "year" => numeric(
            YEAR_WIDTH,
            if c.year == 0 {
                String::new()
            } else {
                c.year.to_string()
            },
        ),
        // The scanner leaves zero when it can't read the tags, so zero is
        // unknown, not 0:00.
        "duration" => numeric(
            DURATION_WIDTH,
            if c.duration_ms == 0 {
                String::new()
            } else {
                fmt_ms(c.duration_ms)
            },
        ),
        "rating" => crate::track_ui::track_cells::rating(state.clone(), c.track_id, c.rating),
        "favourite" => {
            crate::track_ui::track_cells::favourite(state.clone(), c.track_id, c.favourite)
        }
        _ => return None,
    })
}

/// A small rounded cover square, also drawn by the library table's cover
/// column. `Cover` overruns the element on the art's long side and gpui paints
/// that overrun rather than cropping it, so the box masks, not the image.
/// `row_height` is at the stock font size; 6 px is the room above and below.
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

/// Readout widths, px at the stock font size, matching the library table's
/// defaults. A content-sized cell would vary per row ("8m ago" against
/// "21m ago") and drag every flexing column after it.
pub const PLAYS_WIDTH: f32 = 56.;
pub const YEAR_WIDTH: f32 = 56.;
pub const DURATION_WIDTH: f32 = 64.;
pub const LAST_PLAYED_WIDTH: f32 = 84.;

/// Right-aligned at a fixed width so the digits and the text columns before
/// it line up. Overflow clips.
pub fn numeric_cell(width: f32, color: gpui::Rgba, value: String) -> Div {
    div()
        .flex_none()
        .w(palette::scaled_px(width))
        .flex()
        .justify_end()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_color(color)
        .child(SharedString::from(value))
}

pub fn fmt_plays(plays: u32) -> String {
    match plays {
        0 => String::new(),
        1 => "1 play".to_string(),
        n => format!("{n} plays"),
    }
}

/// None when the cover column is off or the track has no path.
pub fn cover_thumb<P: 'static>(
    state: &AppState,
    path: Option<&std::path::Path>,
    shown: bool,
    cx: &mut Context<P>,
) -> Option<Thumb> {
    let path = shown.then_some(path).flatten()?;
    Some(state.thumbs.update(cx, |thumbs, cx| thumbs.get(path, cx)))
}

pub struct AlbumGroup {
    pub album: String,
    /// Falls back to the first track's artist, like the library.
    pub artist: String,
    pub year: u16,
    pub genre: String,
    pub quality: String,
    pub tracks: u32,
    pub total_ms: u64,
    pub first_track_id: i64,
    /// Cached on first paint: outer None not yet resolved, inner None no art.
    pub art: Option<Option<PathBuf>>,
}

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

pub fn album_group(run: &[GroupTrack]) -> AlbumGroup {
    let first = &run[0];
    let mut codec: Option<&str> = Some(first.codec);
    let (mut bit_depth, mut sample_rate_hz) = (first.bit_depth, first.sample_rate_hz);
    let (mut min_kbps, mut max_kbps, mut total_ms) = (0u16, 0u16, 0u64);
    for t in run {
        if codec != Some(t.codec) {
            codec = None;
        }
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

/// The stock heading look, for a panel without its own config. The library
/// table builds its own [`HeadLook`].
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
        // No readings: these runs group off store tags, which carry no sort
        // names.
        name_reading: SharedString::default(),
        album: SharedString::from(g.album.clone()),
        album_reading: SharedString::default(),
        year: g.year,
        genre: SharedString::from(g.genre.clone()),
        quality: SharedString::from(g.quality.clone()),
        tracks: g.tracks,
        total_ms: g.total_ms,
        tiled: true,
        thumb: None,
    }
}

/// Resolves the run's first track to a path once and caches it on the group.
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
            // No album tag is the unknown bucket: keep the placeholder rather
            // than a loose track's art.
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
    // Each row paints the whole square; the meta row's copy starts one line
    // higher so the two halves line up.
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

/// Where one heading line sits inside its list row. `uniform_list` lays every
/// row out at one height, so a heading can't claim a taller row the way the
/// library table can. The line is a strip inside the row instead: `row_px`
/// is the laid-out row, `content_top` where the strip starts (negative on a
/// second line climbing back to meet the first), and `look.line_px` its
/// height.
pub struct HeadSlot<'a> {
    pub pieces: &'a [HeadPiece],
    pub look: &'a group_head::HeadLook,
    pub row_px: Pixels,
    pub content_top: Pixels,
    /// The list background instead of the raised tint, the library's flush
    /// headers.
    pub flush: bool,
}

impl<'a> HeadSlot<'a> {
    /// The line filling its whole row on the raised tint. Pairs with
    /// [`stock_head_look`].
    pub fn stock(pieces: &'a [HeadPiece], look: &'a group_head::HeadLook) -> Self {
        HeadSlot {
            pieces,
            look,
            row_px: palette::scaled_px(ROW_HEIGHT_STOCK),
            content_top: px(0.),
            flush: false,
        }
    }

    fn strip(&self, content: Div) -> Div {
        // Flush paints nothing: the body already painted the list color, and
        // a second coat stops matching once surfaces go translucent.
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

/// Expanded opens the two-row cover tile; Compact draws the packed line
/// alone. A panel without knobs hands over [`stock_head_look`] and
/// [`HeadSlot::stock`].
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

/// The meta line over the tile's bottom half. Only Expanded pushes this row.
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

pub fn stock_name_pieces(headers: Headers) -> Vec<HeadPiece> {
    if headers == Headers::Expanded {
        group_head::stock_name_line()
    } else {
        group_head::stock_compact()
    }
}

pub trait ColumnHost: 'static + Sized {
    fn column_shown(&self, key: &str) -> bool;
    fn set_column(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>);
}

pub trait HeadingHost: 'static + Sized {
    fn headers(&self) -> Headers;
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>);
}

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

/// Follows the panel so a flip shows without the menu reopening.
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
