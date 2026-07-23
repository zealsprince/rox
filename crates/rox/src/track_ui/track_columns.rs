//! The track columns and album grouping shared by the track-list panels
//! (playlists, queue, history). Each panel keeps its own row type, data
//! source, and interactions; this owns the parts that would otherwise drift
//! across copies - the per-column cell render, the consecutive-run album
//! grouping and its two-line heading rows, and the settings checklist and
//! right-click Columns and Headings menus, wired through small host traits.

use std::path::PathBuf;

use gpui::{
    div, img, prelude::*, px, svg, AnyElement, Context, Div, Entity, MouseButton, ObjectFit,
    SharedString, Stateful, Window,
};
use gpui_component::menu::PopupMenu;
use gpui_component::Side;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::group_head::{self, Headers};
use crate::panel::{self, AppState};
use crate::panels::library::fmt_ms;
use crate::settings::ui as settings_ui;
use crate::thumbs::Thumb;

/// Every track row is this tall at the stock font size; the album block is
/// two of them, so the cover tile spans a two-row square. Matches each
/// panel's own row height. The call sites run it through
/// [`palette::scaled_px`] so headings and tiles grow with the app font, the
/// same way the library table scales its rows.
const ROW_H: f32 = 30.;

/// One toggleable column: its config key, its menu and settings label, and
/// whether a fresh panel shows it. A panel's registry fixes the render order.
pub struct Column {
    pub key: &'static str,
    pub label: &'static str,
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
/// columns flex and truncate; number, year, and duration sit in fixed slots;
/// rating and favourite hand off to the shared controls, which write through
/// `state`.
pub fn cell(key: &str, c: &Cell, state: &AppState) -> Option<Div> {
    let text = |value: &str, color: gpui::Rgba| {
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(color)
            .child(SharedString::from(value.to_string()))
    };
    Some(match key {
        "cover" => cover_cell(&c.cover),
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
/// resolves the thumbnail; pending and missing wear the quiet placeholder so
/// a landing cover fills without shifting the row. Shared with the library
/// table's cover column, which draws outside [`cell`].
pub fn cover_cell(cover: &Option<Thumb>) -> Div {
    let side = palette::scaled_px(ROW_H - 6.);
    let content: AnyElement = match cover {
        Some(Thumb::Ready(image)) => img(image.clone())
            .size(side)
            .object_fit(ObjectFit::Cover)
            .rounded(px(3.))
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
/// this from its row build, where the context and the file path live.
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
    pub duration_ms: u32,
    pub track_id: i64,
}

/// Aggregate a run of same-album tracks into a heading group: the first
/// track names it, the run sums the time and spans the codec and bitrate.
pub fn album_group(run: &[GroupTrack]) -> AlbumGroup {
    let first = &run[0];
    let mut codec: Option<&str> = Some(first.codec);
    let (mut min_kbps, mut max_kbps, mut total_ms) = (0u16, 0u16, 0u64);
    for t in run {
        if codec != Some(t.codec) {
            codec = None;
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
        quality: group_head::quality(codec.filter(|c| !c.is_empty()), min_kbps, max_kbps),
        tracks: run.len() as u32,
        total_ms,
        first_track_id: first.track_id,
        art: None,
    }
}

/// The heading look for the tree panels: the cover tile two rows tall, every
/// part shown.
fn look() -> group_head::HeadLook {
    group_head::HeadLook {
        tile_side: palette::scaled_px(ROW_H * 2.),
        show_art: true,
        show_year: true,
        show_details: true,
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
        by_album: true,
    }
}

/// One half of an album run's cover tile, resolving the run's first track to
/// a path once and caching it on the group, the library's route.
fn tile<P: 'static>(
    group: &mut AlbumGroup,
    state: &AppState,
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
    group_head::tile(thumb, palette::scaled_px(ROW_H * 2.), 0., bottom)
}

/// An album run's name line. Expanded opens the two-row cover tile and gives
/// the album artist the line; Compact draws the packed line alone, no tile.
pub fn album_name_row<P: 'static>(
    ix: usize,
    group: &mut AlbumGroup,
    headers: Headers,
    state: &AppState,
    cx: &mut Context<P>,
) -> Stateful<Div> {
    let expanded = headers == Headers::Expanded;
    let tile = expanded.then(|| tile(group, state, false, cx));
    let head = head_of(group);
    div()
        .id(("album-head", ix))
        .relative()
        .w_full()
        .h(palette::scaled_px(ROW_H))
        .bg(palette::bg_elevated())
        .when_some(tile, |d, tile| d.child(tile))
        .child(group_head::name_content(&head, &look(), expanded))
}

/// The run's meta line: the album, genre, quality, track count, and total
/// time over the tile's bottom half. Only Expanded pushes this row.
pub fn album_meta_row<P: 'static>(
    ix: usize,
    group: &mut AlbumGroup,
    state: &AppState,
    cx: &mut Context<P>,
) -> Stateful<Div> {
    let tile = tile(group, state, true, cx);
    let head = head_of(group);
    div()
        .id(("album-meta", ix))
        .relative()
        .w_full()
        .h(palette::scaled_px(ROW_H))
        .bg(palette::bg_elevated())
        .child(tile)
        .child(group_head::meta_content(&head, &look()))
}

/// A panel that stores a shown-column set the shared menus edit.
pub trait ColumnHost: 'static + Sized {
    fn column_shown(&self, key: &str) -> bool;
    fn set_column(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>);
}

/// A panel that carries an album heading mode the shared menu edits.
pub trait HeadingHost: 'static + Sized {
    fn headers(&self) -> Headers;
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>);
}

/// The View-page column checklist: a tick per registry column, a click
/// flipping it. The panel's own registry fixes the set and order.
pub fn checklist<P: ColumnHost>(columns: &'static [Column], panel: &P, cx: &mut Context<P>) -> Div {
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
                        .child(col.label),
                ),
        );
    }
    list
}

/// The right-click Columns submenu: a live-checked row per registry column,
/// tracking the panel so a flip shows without the menu reopening.
pub fn columns_submenu<P: ColumnHost>(
    columns: &'static [Column],
    window: &mut Window,
    cx: &mut Context<P>,
) -> Entity<PopupMenu> {
    let panel = cx.entity();
    PopupMenu::build(window, cx, move |mut submenu, _, cx| {
        panel::follow_panel(&panel, cx);
        for col in columns {
            let key = col.key;
            submenu = submenu.item(panel::check_row(
                col.label,
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
            (Headers::Off, "Off"),
            (Headers::Compact, "Compact"),
            (Headers::Expanded, "Expanded"),
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
