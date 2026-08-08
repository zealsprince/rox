//! The album heading surface shared by the library table and the tree
//! panels. An album run reads as a block of composed lines: each line is an
//! ordered list of [`HeadPiece`]s, so a layout can put the artist, album,
//! year, and stats wherever it wants them, with a cover tile spanning the
//! block. Each caller resolves a [`GroupHead`] from whatever metadata it
//! holds (the library from its projection, the playlists tree from its
//! member rows) and lays the content over its own row background, so the
//! headings stay one look. The stock lines here are the classic two-line
//! arrangement; the library's config carries its own.

use gpui::{div, img, prelude::*, px, svg, AnyElement, Div, ObjectFit, Pixels, SharedString};
use serde::{Deserialize, Serialize};

use crate::panel::ArrangeSpec;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_services::thumbs::Thumb;

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

/// One piece of a heading line, the arrange editor's unit. A panel's
/// config carries each line as an ordered piece list; the resolved
/// [`GroupHead`] supplies the text, and a piece whose field is empty just
/// drops out of the line.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadPiece {
    /// The group's name field: the album artist, or whatever the grouping
    /// keys on.
    Artist,
    Album,
    Year,
    Genre,
    /// The codec, stream shape, and bitrate readout, from [`quality`].
    Quality,
    /// The track count.
    Tracks,
    /// The total running time.
    Time,
    /// A flexible gap that splits a line into a left and a right side.
    Spacer,
    /// A spacer that draws a hairline in the border color across its gap.
    Divider,
    /// An inline cover square, one line tall; the block tile's small
    /// sibling, for layouts that want the art on a row instead.
    Art,
}

/// Which side of the header block the cover tile sits on; the composed
/// lines indent past it on the same side.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtSide {
    #[default]
    Left,
    Right,
}

/// The full piece catalog in stock order: what the arrange editors offer,
/// and where a re-shown piece slots back in.
pub const PIECES: &[ArrangeSpec<HeadPiece>] = &[
    ArrangeSpec {
        label: "Artist",
        icon: Some(icons::MIC),
        value: HeadPiece::Artist,
    },
    ArrangeSpec {
        label: "Album",
        icon: Some(icons::DISC),
        value: HeadPiece::Album,
    },
    ArrangeSpec {
        label: "Year",
        icon: Some(icons::CALENDAR),
        value: HeadPiece::Year,
    },
    ArrangeSpec {
        label: "Genre",
        icon: Some(icons::TAG),
        value: HeadPiece::Genre,
    },
    ArrangeSpec {
        label: "Quality",
        icon: Some(icons::AUDIO_WAVEFORM),
        value: HeadPiece::Quality,
    },
    ArrangeSpec {
        label: "Tracks",
        icon: Some(icons::LIST_MUSIC),
        value: HeadPiece::Tracks,
    },
    ArrangeSpec {
        label: "Time",
        icon: Some(icons::CLOCK),
        value: HeadPiece::Time,
    },
    ArrangeSpec {
        label: "Spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: HeadPiece::Spacer,
    },
    ArrangeSpec {
        label: "Divider",
        icon: Some(icons::MINUS),
        value: HeadPiece::Divider,
    },
    ArrangeSpec {
        label: "Art",
        icon: Some(icons::IMAGE),
        value: HeadPiece::Art,
    },
];

/// The compact header's stock row: the name and album packed left, the
/// year opposite.
pub fn stock_compact() -> Vec<HeadPiece> {
    vec![
        HeadPiece::Artist,
        HeadPiece::Album,
        HeadPiece::Spacer,
        HeadPiece::Year,
    ]
}

/// The expanded block's stock name line.
pub fn stock_name_line() -> Vec<HeadPiece> {
    vec![HeadPiece::Artist, HeadPiece::Spacer, HeadPiece::Year]
}

/// The expanded block's stock meta line.
pub fn stock_meta_line() -> Vec<HeadPiece> {
    vec![
        HeadPiece::Album,
        HeadPiece::Spacer,
        HeadPiece::Genre,
        HeadPiece::Quality,
        HeadPiece::Tracks,
        HeadPiece::Time,
    ]
}

/// One album run's heading, resolved by the caller. The strings are the
/// display text as-is; an empty `name` draws "Unknown".
#[derive(Default)]
pub struct GroupHead {
    /// The album artist, or the field a non-album grouping keys on.
    pub name: SharedString,
    /// The album, shown on the meta line (expanded) or beside the name
    /// (compact). Empty when the grouping is not by album.
    pub album: SharedString,
    /// The year on the name line; 0 hides it.
    pub year: u16,
    pub genre: SharedString,
    /// The codec, stream shape, and bitrate line, from [`quality`].
    pub quality: SharedString,
    pub tracks: u32,
    pub total_ms: u64,
    /// Whether this is an album grouping: the cover tile, the album text,
    /// and the trailing year are album presentation, off for the rest.
    pub by_album: bool,
    /// The group's cover, resolved by the caller only when a line carries
    /// the inline art piece; None drops the piece from the line.
    pub thumb: Option<Thumb>,
}

/// The knobs that shape a heading's look, mirrored from the panel's config.
pub struct HeadLook {
    /// The cover tile's side, two rows tall, so the content indents past it.
    pub tile_side: Pixels,
    pub show_art: bool,
    pub show_year: bool,
    pub show_details: bool,
    /// One composed line's height, what the inline art piece squares to.
    pub line_px: Pixels,
    /// Which side the block tile sits on, and how far the lines indent.
    pub art_side: ArtSide,
    /// The tile's inset from the block edges; part of the indent.
    pub art_margin: Pixels,
    /// The cover corners' radius, shared by the tile and the inline piece.
    pub art_rounding: f32,
}

/// A sample rate as the kHz a spec sheet writes: 44100 reads "44.1",
/// 48000 reads "48", 22050 reads "22.05". Empty at zero, which is what an
/// unread stream and a mixed group both carry.
pub fn khz(hz: u32) -> String {
    if hz == 0 {
        return String::new();
    }
    // Hundredths of a kHz, kept integer the whole way: 22050 through an f32
    // divide lands at 22.04999. Two places is as far as any rate in the wild
    // needs, and the zeros come off after, so nothing grows a decimal it
    // didn't earn.
    let hundredths = (hz + 5) / 10;
    let whole = hundredths / 100;
    match hundredths % 100 {
        0 => whole.to_string(),
        rest if rest % 10 == 0 => format!("{whole}.{}", rest / 10),
        rest => format!("{whole}.{rest:02}"),
    }
}

/// The stream's shape the way a spec sheet writes it: "16/44.1 kHz" for a
/// lossless file, the rate alone for a lossy one (which has no depth to
/// name once the stream is coefficients), the depth alone if that is all
/// there is. Zero on either side means unread or mixed across the group,
/// and drops out.
pub fn stream_format(bit_depth: u8, sample_rate_hz: u32) -> String {
    match (bit_depth, khz(sample_rate_hz)) {
        (0, rate) if rate.is_empty() => String::new(),
        (0, rate) => format!("{rate} kHz"),
        (bits, rate) if rate.is_empty() => format!("{bits} bit"),
        (bits, rate) => format!("{bits}/{rate} kHz"),
    }
}

/// The composed lines a header mode renders: the compact row alone, or
/// the expanded slots with the empty ones dropped. Never empty, so a
/// block always spans at least one row.
pub fn effective_head_lines(
    headers: Headers,
    compact: &[HeadPiece],
    lines: &[Vec<HeadPiece>],
) -> Vec<Vec<HeadPiece>> {
    let lines: Vec<Vec<HeadPiece>> = match headers {
        Headers::Expanded => lines.iter().filter(|l| !l.is_empty()).cloned().collect(),
        _ => vec![compact.to_vec()],
    };
    if lines.is_empty() {
        vec![Vec::new()]
    } else {
        lines
    }
}

/// A group's codec, stream shape, and bitrate stat: "flac 16/44.1 kHz 1006
/// kbps" when everything agrees, the kbps a range when tracks spread, and
/// any part dropping out when it is mixed across the run or was never
/// read. Empty when nothing agrees.
pub fn quality(
    codec: Option<&str>,
    min_kbps: u16,
    max_kbps: u16,
    bit_depth: u8,
    sample_rate_hz: u32,
) -> String {
    let kbps = match (min_kbps, max_kbps) {
        (0, _) => String::new(),
        (min, max) if min == max => format!("{min} kbps"),
        (min, max) => format!("{min}-{max} kbps"),
    };
    let format = stream_format(bit_depth, sample_rate_hz);
    [codec.unwrap_or(""), format.as_str(), kbps.as_str()]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// One row's share of a heading block's cover tile. The block draws as
/// fixed-height rows with no spanning cell, so every row paints the whole
/// block-tall square at the same spot: `lift` is how far above this row
/// the square starts, the line index times the row height. The rows paint
/// in order, so the last one's draw is what shows - one unclipped quad,
/// which is the point. Clipping a slice per row instead leaves hairline
/// seams at the boundaries once a font scale puts the rows on fractional
/// pixels. The same image handle every time decodes once. Pending and
/// missing wear the same quiet placeholder, so a landing cover fills the
/// tile without shifting the text beside it. The knob's radius rides the
/// cover itself, since gpui content masks stay rectangular.
pub fn tile(
    thumb: Thumb,
    side: Pixels,
    rounding: f32,
    lift: Pixels,
    art_side: ArtSide,
    margin: Pixels,
) -> AnyElement {
    let content = art_content(thumb, rounding, 16.);
    div()
        .absolute()
        .top_0()
        .map(|d| match art_side {
            ArtSide::Left => d.left(margin),
            ArtSide::Right => d.right(margin),
        })
        .w(side)
        .child(
            div()
                .absolute()
                .left_0()
                .w(side)
                .h(side)
                .top(margin - lift)
                .child(content),
        )
        .into_any_element()
}

/// A cover's face: the image rounded per the knob, or the quiet music-note
/// placeholder both pending and missing wear, so a landing cover fills in
/// without a layout shift. Shared by the block tile and the inline piece.
///
/// `Cover` scales the art until it fills the square, which leaves the odd
/// side hanging outside the element. gpui hands `paint_image` those larger
/// bounds and masks nothing on its own, so a sleeve that isn't square
/// spills over the track rows above and below the block. The crop is ours
/// to make: `overflow_hidden` puts the mask back at the square's edge.
fn art_content(thumb: Thumb, rounding: f32, icon_px: f32) -> AnyElement {
    match thumb {
        Thumb::Ready(image) => img(image)
            .size_full()
            .overflow_hidden()
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
                    .size(px(icon_px))
                    .text_color(palette::text_faint()),
            )
            .into_any_element(),
    }
}

/// Append a buffered run of stat pieces as one muted span, joined with the
/// classic " | " separators, and clear the buffer.
fn flush_stats(row: Div, stats: &mut Vec<String>) -> Div {
    if stats.is_empty() {
        return row;
    }
    let text = stats.join(" | ");
    stats.clear();
    row.child(
        div()
            .flex_none()
            .text_color(palette::text_muted())
            .child(SharedString::from(text)),
    )
}

/// One heading line composed from its pieces: the absolute-filled row a
/// caller lays over its own background and the cover tile's slice. The
/// name and album shrink and truncate, the year pins, a spacer pushes the
/// sides apart, and a run of adjacent stat pieces (genre, quality, tracks,
/// time) joins into one muted span so the readout keeps its separators.
/// An empty field's piece drops out, except the name, which reads
/// "Unknown" unless a shown album already names the line.
pub fn line_content(
    pieces: &[HeadPiece],
    head: &GroupHead,
    look: &HeadLook,
    expanded: bool,
) -> Div {
    let has_tile = expanded && head.by_album && look.show_art;
    let indent = look.art_margin + look.tile_side + tokens::SPACE_SM;
    let album_here = !head.album.is_empty() && pieces.contains(&HeadPiece::Album);
    let mut row = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .px(tokens::SPACE_SM)
        // Clear of the cover tile, which spans the block on its side.
        .when(has_tile, |d| match look.art_side {
            ArtSide::Left => d.pl(indent),
            ArtSide::Right => d.pr(indent),
        })
        .overflow_hidden();
    let mut stats: Vec<String> = Vec::new();
    for piece in pieces {
        match piece {
            HeadPiece::Artist => {
                row = flush_stats(row, &mut stats);
                if head.name.is_empty() {
                    if !album_here {
                        row = row.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(palette::text_muted())
                                .child("Unknown"),
                        );
                    }
                } else {
                    row = row.child(
                        div()
                            .text_color(palette::text_bright())
                            // Expanded, the name is the block's lead and
                            // gives way by truncating; compact keeps it
                            // whole and lets the album truncate instead.
                            .map(|d| {
                                if expanded {
                                    d.min_w_0().truncate().text_lg()
                                } else {
                                    d.flex_none()
                                }
                            })
                            .child(head.name.clone()),
                    );
                }
            }
            HeadPiece::Album => {
                row = flush_stats(row, &mut stats);
                if !head.album.is_empty() {
                    row = row.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(palette::text_secondary())
                            .child(head.album.clone()),
                    );
                }
            }
            HeadPiece::Year => {
                row = flush_stats(row, &mut stats);
                if head.year != 0 {
                    row = row.child(
                        div()
                            .flex_none()
                            .text_color(if expanded {
                                palette::text_secondary()
                            } else {
                                palette::text_muted()
                            })
                            .child(SharedString::from(head.year.to_string())),
                    );
                }
            }
            HeadPiece::Genre => {
                if !head.genre.is_empty() {
                    stats.push(head.genre.to_string());
                }
            }
            HeadPiece::Quality => {
                if !head.quality.is_empty() {
                    stats.push(head.quality.to_string());
                }
            }
            HeadPiece::Tracks => {
                stats.push(if head.tracks == 1 {
                    "1 track".to_string()
                } else {
                    format!("{} tracks", head.tracks)
                });
            }
            HeadPiece::Time => {
                stats.push(fmt_total(head.total_ms));
            }
            HeadPiece::Spacer => {
                row = flush_stats(row, &mut stats);
                row = row.child(div().flex_1());
            }
            HeadPiece::Divider => {
                row = flush_stats(row, &mut stats);
                row = row.child(div().flex_1().h(px(1.)).bg(palette::border()));
            }
            HeadPiece::Art => {
                row = flush_stats(row, &mut stats);
                if let Some(thumb) = &head.thumb {
                    let side = look.line_px - tokens::SPACE_XS * 2.;
                    row = row.child(div().flex_none().w(side).h(side).child(art_content(
                        thumb.clone(),
                        look.art_rounding,
                        12.,
                    )));
                }
            }
        }
    }
    flush_stats(row, &mut stats)
}

/// The heading's name line in the stock arrangement: expanded gives the
/// name the line with the year opposite; compact packs the album in too.
pub fn name_content(head: &GroupHead, look: &HeadLook, expanded: bool) -> Div {
    let mut pieces = if expanded {
        stock_name_line()
    } else {
        stock_compact()
    };
    if !look.show_year {
        pieces.retain(|p| *p != HeadPiece::Year);
    }
    line_content(&pieces, head, look, expanded)
}

/// The expanded block's stock meta line: the album, then the group's stats
/// on the right. `show_details` drops the genre and quality while the
/// track count and total time stay.
pub fn meta_content(head: &GroupHead, look: &HeadLook) -> Div {
    let mut pieces = stock_meta_line();
    if !look.show_details {
        pieces.retain(|p| !matches!(p, HeadPiece::Genre | HeadPiece::Quality));
    }
    line_content(&pieces, head, look, true)
}

/// A group's total time: minutes and seconds, growing an hours place once
/// it earns one.
pub fn fmt_total(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::{khz, quality, stream_format};

    /// A rate reads as the kHz a spec sheet writes, the decimals only when
    /// there are some to show, and the second one only when the first
    /// wouldn't be the rate.
    #[test]
    fn a_rate_reads_as_khz() {
        assert_eq!(khz(44100), "44.1");
        assert_eq!(khz(48000), "48");
        assert_eq!(khz(96000), "96");
        assert_eq!(khz(88200), "88.2");
        assert_eq!(khz(22050), "22.05");
        assert_eq!(khz(0), "");
    }

    /// The stream shape pairs the depth with the rate, and drops whichever
    /// half is missing: a lossy file has no depth to name.
    #[test]
    fn the_stream_shape_drops_what_it_lacks() {
        assert_eq!(stream_format(16, 44100), "16/44.1 kHz");
        assert_eq!(stream_format(24, 96000), "24/96 kHz");
        assert_eq!(stream_format(0, 44100), "44.1 kHz");
        assert_eq!(stream_format(16, 0), "16 bit");
        assert_eq!(stream_format(0, 0), "");
    }

    /// The group line joins whatever agrees across the run: everything for
    /// a lossless album, the spread when the bitrate varies, and nothing
    /// once the run has nothing in common.
    #[test]
    fn the_group_line_joins_what_agrees() {
        assert_eq!(
            quality(Some("flac"), 1006, 1006, 16, 44100),
            "flac 16/44.1 kHz 1006 kbps"
        );
        assert_eq!(
            quality(Some("mp3"), 192, 320, 0, 44100),
            "mp3 44.1 kHz 192-320 kbps"
        );
        // A mixed run: the codec, depth, and rate all zero out, leaving
        // the bitrate spread on its own.
        assert_eq!(quality(None, 192, 320, 0, 0), "192-320 kbps");
        assert_eq!(quality(None, 0, 0, 0, 0), "");
        // The bitrate alone stays what it always was.
        assert_eq!(quality(Some("wav"), 0, 0, 16, 48000), "wav 16/48 kHz");
    }
}
