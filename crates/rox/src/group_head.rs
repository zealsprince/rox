//! The album heading surface shared by the library table and the playlists
//! tree. One album run reads as a two-line block: a name line with the album
//! artist, year, and a cover tile, and a meta line with the album, genre,
//! quality, track count, and total time under it. Each caller resolves a
//! [`GroupHead`] from whatever metadata it holds (the library from its
//! projection, the playlists tree from its member rows) and lays the content
//! over its own row background, so the two headings stay one look.

use gpui::{div, img, prelude::*, px, svg, AnyElement, Div, ObjectFit, Pixels, SharedString};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::thumbs::Thumb;

/// How a group's header shows, shared by the library table and the playlists
/// tree. Compact spends one row on the group's name line; Expanded adds a
/// meta line under it and the two-row cover tile beside them. Off hides the
/// headers, leaving a flat list.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Headers {
    Off,
    Compact,
    #[default]
    Expanded,
}

/// One album run's heading, resolved by the caller. The strings are the
/// display text as-is; an empty `name` draws "Unknown".
pub struct GroupHead {
    /// The album artist, or the field a non-album grouping keys on.
    pub name: SharedString,
    /// The album, shown on the meta line (expanded) or beside the name
    /// (compact). Empty when the grouping is not by album.
    pub album: SharedString,
    /// The year on the name line; 0 hides it.
    pub year: u16,
    pub genre: SharedString,
    /// The codec and bitrate line, from [`quality`].
    pub quality: SharedString,
    pub tracks: u32,
    pub total_ms: u64,
    /// Whether this is an album grouping: the cover tile, the album text,
    /// and the trailing year are album presentation, off for the rest.
    pub by_album: bool,
}

/// The knobs that shape a heading's look, mirrored from the panel's config.
pub struct HeadLook {
    /// The cover tile's side, two rows tall, so the content indents past it.
    pub tile_side: Pixels,
    pub show_art: bool,
    pub show_year: bool,
    pub show_details: bool,
}

/// A group's codec and bitrate stat: "mp3 320 kbps" when everything agrees,
/// the kbps a range when tracks spread, either half alone when the other is
/// mixed or missing, empty when both are.
pub fn quality(codec: Option<&str>, min_kbps: u16, max_kbps: u16) -> String {
    let codec = codec.unwrap_or("");
    let kbps = match (min_kbps, max_kbps) {
        (0, _) => String::new(),
        (min, max) if min == max => format!("{min} kbps"),
        (min, max) => format!("{min}-{max} kbps"),
    };
    match (codec.is_empty(), kbps.is_empty()) {
        (false, false) => format!("{codec} {kbps}"),
        (false, true) => codec.to_string(),
        _ => kbps,
    }
}

/// One half of an expanded header's cover tile. The block draws as two
/// fixed-height rows with no spanning cell, so each row clips its own half
/// of a two-row-tall square: the name row the top (`bottom` false), the meta
/// row the bottom. The same image handle both times decodes once. Pending
/// and missing wear the same quiet placeholder, so a landing cover fills the
/// tile without shifting the text beside it. The knob's radius rides the
/// cover itself, since gpui content masks stay rectangular.
pub fn tile(thumb: Thumb, side: Pixels, rounding: f32, bottom: bool) -> AnyElement {
    let content: AnyElement = match thumb {
        Thumb::Ready(image) => img(image)
            .size_full()
            .object_fit(ObjectFit::Cover)
            .rounded(px(rounding))
            .into_any_element(),
        _ => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(icons::MUSIC)
                    .size(px(16.))
                    .text_color(palette::text_faint()),
            )
            .into_any_element(),
    };
    div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(side)
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .left_0()
                .w(side)
                .h(side)
                .map(|d| if bottom { d.bottom_0() } else { d.top_0() })
                .child(content),
        )
        .into_any_element()
}

/// The heading's name line: the absolute-filled row a caller lays over its
/// own background and (for album groupings) the top half of the cover tile.
/// Expanded gives the name the line, larger, the year on the right, and
/// hands the album to the meta line; compact packs the album and year
/// alongside the name.
pub fn name_content(head: &GroupHead, look: &HeadLook, expanded: bool) -> Div {
    let has_tile = expanded && head.by_album && look.show_art;
    let indent = look.tile_side + tokens::SPACE_SM;
    let unknown = head.name.is_empty() && (expanded || head.album.is_empty());
    let name = (!head.name.is_empty()).then(|| head.name.clone());
    let album = (!expanded && !head.album.is_empty()).then(|| head.album.clone());
    div()
        .absolute()
        .inset_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .px(tokens::SPACE_SM)
        // Clear of the cover tile, which spans the block.
        .when(has_tile, |d| d.pl(indent))
        .overflow_hidden()
        .when(unknown, |d| {
            d.child(
                div()
                    .flex_1()
                    .text_color(palette::text_muted())
                    .child("Unknown"),
            )
        })
        .when_some(name, |d, name| {
            d.child(
                div()
                    .truncate()
                    .text_color(palette::text_bright())
                    .map(|d| {
                        if expanded {
                            d.flex_1().text_lg()
                        } else {
                            d.flex_none()
                        }
                    })
                    .child(name),
            )
        })
        .when_some(album, |d, album| {
            d.child(
                div()
                    .truncate()
                    .text_color(palette::text_secondary())
                    .child(album),
            )
        })
        .when(head.year != 0 && look.show_year, |d| {
            d.child(
                div()
                    .flex_none()
                    .text_color(if expanded {
                        palette::text_secondary()
                    } else {
                        palette::text_muted()
                    })
                    .child(SharedString::from(head.year.to_string())),
            )
        })
}

/// The expanded header's second line: the album, then its genre, quality,
/// track count, and total time on the right, over the tile's bottom half.
/// A non-album grouping keeps the count and time, with an empty album
/// spacer, since the album, genre, and quality describe one album.
pub fn meta_content(head: &GroupHead, look: &HeadLook) -> Div {
    let has_tile = head.by_album && look.show_art;
    let indent = look.tile_side + tokens::SPACE_SM;
    let mut stats = Vec::new();
    if look.show_details {
        if !head.genre.is_empty() {
            stats.push(head.genre.to_string());
        }
        if !head.quality.is_empty() {
            stats.push(head.quality.to_string());
        }
    }
    stats.push(if head.tracks == 1 {
        "1 track".to_string()
    } else {
        format!("{} tracks", head.tracks)
    });
    stats.push(fmt_total(head.total_ms));
    div()
        .absolute()
        .inset_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .px(tokens::SPACE_SM)
        // Clear of the cover tile, which spans the block.
        .when(has_tile, |d| d.pl(indent))
        .overflow_hidden()
        .child(
            div()
                .flex_1()
                .truncate()
                .text_color(palette::text_secondary())
                .child(head.album.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_color(palette::text_muted())
                .child(SharedString::from(stats.join(" | "))),
        )
}

/// A group's total time: minutes and seconds, growing an hours place once
/// it earns one.
fn fmt_total(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}
