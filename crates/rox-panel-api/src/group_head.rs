//! The album heading surface shared by the library table and the tree
//! panels. An album run reads as a block of composed lines, each an ordered
//! list of [`HeadPiece`]s, with a cover tile spanning the block. Each caller
//! resolves a [`GroupHead`] from the metadata it holds and lays the content
//! over its own row background, so the headings stay one look.

use gpui::{
    AnyElement, Div, ObjectFit, Pixels, SharedString, div, img, linear_color_stop, linear_gradient,
    prelude::*, px, rems, svg,
};
use serde::{Deserialize, Serialize};

use crate::panel::ArrangeSpec;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::motif;
use rox_services::thumbs::Thumb;

/// Compact spends one row on the name line; Expanded adds a meta line and
/// the two-row cover tile.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Headers {
    Off,
    Compact,
    #[default]
    Expanded,
}

/// One piece of a heading line, the arrange editor's unit. A piece whose
/// field is empty drops out of the line.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadPiece {
    /// The album artist, or whatever the grouping keys on.
    Artist,
    Album,
    Year,
    Genre,
    /// The codec, stream shape, and bitrate readout.
    Quality,
    Tracks,
    Time,
    /// A flexible gap that splits a line into a left and a right side.
    Spacer,
    /// A spacer that draws a hairline in the border color across its gap.
    Divider,
    /// An inline cover square, one line tall.
    Art,
}

/// The composed lines indent past the tile on the same side.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtSide {
    #[default]
    Left,
    Right,
}

/// A genre tile draws a mosaic once it has this many covers.
pub const MOSAIC: usize = 4;

/// Tinted is the default: the covers still read as your music while the
/// genre's color marks which one. Gradient and Color are cards in the
/// genre's color under its geometry motif.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileFace {
    Mosaic,
    #[default]
    Tinted,
    Gradient,
    Color,
}

impl TileFace {
    pub fn label(self) -> gpui::SharedString {
        match self {
            TileFace::Mosaic => rox_i18n::t!("tile-face-mosaic"),
            TileFace::Tinted => rox_i18n::t!("tile-face-tinted"),
            TileFace::Gradient => rox_i18n::t!("tile-face-gradient"),
            TileFace::Color => rox_i18n::t!("tile-face-color"),
        }
    }

    /// The card faces paint no covers, so they skip the thumbnail cache.
    pub fn is_card(self) -> bool {
        matches!(self, TileFace::Gradient | TileFace::Color)
    }
}

/// Stock order, which is where a re-shown piece slots back in.
pub const PIECES: &[ArrangeSpec<HeadPiece>] = &[
    ArrangeSpec {
        key: "head-piece-artist",
        icon: Some(icons::MIC),
        value: HeadPiece::Artist,
        repeats: false,
    },
    ArrangeSpec {
        key: "head-piece-album",
        icon: Some(icons::DISC),
        value: HeadPiece::Album,
        repeats: false,
    },
    ArrangeSpec {
        key: "head-piece-year",
        icon: Some(icons::CALENDAR),
        value: HeadPiece::Year,
        repeats: false,
    },
    ArrangeSpec {
        key: "head-piece-genre",
        icon: Some(icons::TAG),
        value: HeadPiece::Genre,
        repeats: false,
    },
    ArrangeSpec {
        key: "head-piece-quality",
        icon: Some(icons::AUDIO_WAVEFORM),
        value: HeadPiece::Quality,
        repeats: false,
    },
    ArrangeSpec {
        key: "head-piece-tracks",
        icon: Some(icons::LIST_MUSIC),
        value: HeadPiece::Tracks,
        repeats: false,
    },
    ArrangeSpec {
        key: "head-piece-time",
        icon: Some(icons::CLOCK),
        value: HeadPiece::Time,
        repeats: false,
    },
    ArrangeSpec {
        key: "head-piece-spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: HeadPiece::Spacer,
        repeats: true,
    },
    ArrangeSpec {
        key: "head-piece-divider",
        icon: Some(icons::MINUS),
        value: HeadPiece::Divider,
        repeats: true,
    },
    ArrangeSpec {
        key: "head-piece-art",
        icon: Some(icons::IMAGE),
        value: HeadPiece::Art,
        repeats: false,
    },
];

pub fn stock_compact() -> Vec<HeadPiece> {
    vec![
        HeadPiece::Artist,
        HeadPiece::Album,
        HeadPiece::Spacer,
        HeadPiece::Year,
    ]
}

pub fn stock_name_line() -> Vec<HeadPiece> {
    vec![HeadPiece::Artist, HeadPiece::Spacer, HeadPiece::Year]
}

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

/// Strings are display text as-is; an empty `name` draws "Unknown".
#[derive(Default)]
pub struct GroupHead {
    /// The album artist, or the field a non-album grouping keys on.
    pub name: SharedString,
    /// Drawn as a reading when the switch is on. Empty when there's none,
    /// including the year and genre groupings.
    pub name_reading: SharedString,
    /// Empty when the grouping is not by album.
    pub album: SharedString,
    pub album_reading: SharedString,
    /// 0 hides it.
    pub year: u16,
    pub genre: SharedString,
    pub quality: SharedString,
    pub tracks: u32,
    pub total_ms: u64,
    /// The caller decides what has a cover: albums always, the library's
    /// artist and genre groupings too, year never.
    pub tiled: bool,
    /// Resolved only when a line includes the inline art piece. None drops
    /// the piece.
    pub thumb: Option<Thumb>,
}

pub struct HeadLook {
    /// Two rows tall; the content indents past it.
    pub tile_side: Pixels,
    pub show_art: bool,
    pub show_year: bool,
    pub show_details: bool,
    /// What the inline art piece squares to.
    pub line_px: Pixels,
    pub art_side: ArtSide,
    /// The tile's inset from the block edges, part of the indent.
    pub art_margin: Pixels,
    pub art_rounding: f32,
    /// Text size as a rem factor, so text follows the line height. The name
    /// line's lead multiplies its own step on top.
    pub font_scale: f32,
}

/// 44100 reads "44.1", 22050 reads "22.05". Empty at zero, which is both an
/// unread stream and a mixed group.
pub fn khz(hz: u32) -> String {
    if hz == 0 {
        return String::new();
    }
    // Integer hundredths: 22050 through an f32 divide comes out at 22.04999.
    let hundredths = (hz + 5) / 10;
    // Only the decimal mark is a locale question (German writes 44,1 kHz),
    // so the join goes through ICU.
    let places = match hundredths % 100 {
        0 => 0,
        rest if rest % 10 == 0 => 1,
        _ => 2,
    };
    rox_i18n::format::format_float(f64::from(hundredths) / 100.0, places)
}

/// "16/44.1 kHz" for lossless, the rate alone for lossy, the depth alone if
/// that's all there is. Zero on either side means unread or mixed.
pub fn stream_format(bit_depth: u8, sample_rate_hz: u32) -> String {
    match (bit_depth, khz(sample_rate_hz)) {
        (0, rate) if rate.is_empty() => String::new(),
        (0, rate) => format!("{rate} kHz"),
        (bits, rate) if rate.is_empty() => format!("{bits} bit"),
        (bits, rate) => format!("{bits}/{rate} kHz"),
    }
}

/// Never empty, so a block always spans at least one row.
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

/// "flac 16/44.1 kHz 1006 kbps" when everything agrees, a kbps range when
/// tracks spread, and any part that's mixed or unread dropped.
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

/// One row's share of a heading block's cover tile. The rows have no
/// spanning cell, so every row paints the whole block-tall square at the same
/// spot, `lift` above itself, and the last row's draw shows. Clipping a slice
/// per row instead leaves hairline seams once a font scale puts rows on
/// fractional pixels. The radius goes on the cover itself, since gpui content
/// masks stay rectangular.
pub fn tile(
    thumb: Thumb,
    side: Pixels,
    rounding: f32,
    lift: Pixels,
    art_side: ArtSide,
    margin: Pixels,
) -> AnyElement {
    tile_frame(
        art_content(thumb, rounding, 16., false),
        side,
        lift,
        art_side,
        margin,
    )
}

/// The genre grouping's block tile, drawn with the grid's [`TileFace`]: a
/// two-by-two mosaic once `thumbs` has [`MOSAIC`] covers, the lone first
/// cover below that. The card leaves the name off for the header line.
#[allow(clippy::too_many_arguments)]
pub fn genre_tile(
    face: TileFace,
    thumbs: &[Thumb],
    name: &str,
    side: Pixels,
    rounding: f32,
    lift: Pixels,
    art_side: ArtSide,
    margin: Pixels,
) -> AnyElement {
    let (color, partner) = palette::genre_color_pair(name);
    let seed = palette::genre_seed(name);
    let card = |background: gpui::Background, base: gpui::Rgba| -> AnyElement {
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .rounded(px(rounding))
            .bg(background)
            .child(motif(seed, palette::text_on(base)))
            .into_any_element()
    };
    let grayed = face == TileFace::Tinted;
    let covers: Option<AnyElement> = if face.is_card() || thumbs.is_empty() {
        None
    } else if thumbs.len() >= MOSAIC {
        Some(mosaic_content(thumbs, rounding, grayed))
    } else {
        Some(art_content(thumbs[0].clone(), rounding, 16., grayed))
    };
    let content: AnyElement = match (face, covers) {
        (TileFace::Color, _) => card(color.into(), color),
        // The grid's angle rule, so neighbors in a hue family still tilt apart.
        (TileFace::Gradient, _) => card(
            linear_gradient(
                ((seed >> 45) % 360) as f32,
                linear_color_stop(color, 0.0),
                linear_color_stop(partner, 1.0),
            ),
            color,
        ),
        (TileFace::Tinted, Some(covers)) => div()
            .size_full()
            .relative()
            .child(covers)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(rounding))
                    .bg(palette::alpha(color, 0x73)),
            )
            .into_any_element(),
        // A coverless genre borrows the card so the tile still names it.
        (TileFace::Tinted, None) if !name.is_empty() => card(color.into(), color),
        (_, Some(covers)) => covers,
        (_, None) => art_content(Thumb::Missing, rounding, 16., false),
    };
    tile_frame(content, side, lift, art_side, margin)
}

/// Each quadrant rounds only its outer corner.
fn mosaic_content(thumbs: &[Thumb], rounding: f32, grayed: bool) -> AnyElement {
    let quarter = |thumb: &Thumb, corner: usize| -> AnyElement {
        match thumb {
            Thumb::Ready(image) => {
                let image = img(image.clone())
                    .size_full()
                    .overflow_hidden()
                    .object_fit(ObjectFit::Cover)
                    .grayscale(grayed);
                match corner {
                    0 => image.rounded_tl(px(rounding)),
                    1 => image.rounded_tr(px(rounding)),
                    2 => image.rounded_bl(px(rounding)),
                    _ => image.rounded_br(px(rounding)),
                }
                .into_any_element()
            }
            _ => div()
                .size_full()
                .bg(palette::bg_elevated())
                .into_any_element(),
        }
    };
    let half = |a: AnyElement, b: AnyElement| {
        let cell = |content: AnyElement| div().flex_1().min_w_0().overflow_hidden().child(content);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .child(cell(a))
            .child(cell(b))
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(half(quarter(&thumbs[0], 0), quarter(&thumbs[1], 1)))
        .child(half(quarter(&thumbs[2], 2), quarter(&thumbs[3], 3)))
        .into_any_element()
}

/// The tile's placement frame. The square is also the crop: `Cover` overruns
/// a non-square sleeve on its long side and that overrun paints over the rows
/// around the block, so the clip sits on this box, not on the image.
fn tile_frame(
    content: AnyElement,
    side: Pixels,
    lift: Pixels,
    art_side: ArtSide,
    margin: Pixels,
) -> AnyElement {
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
                .overflow_hidden()
                .child(content),
        )
        .into_any_element()
}

/// A cover's face, or the music-note placeholder for both pending and
/// missing so a late cover fills in without a layout shift. The crop is the
/// caller's to make on the sized box: `overflow_hidden` here masks against
/// the image's own grown bounds and crops nothing.
pub fn art_content(thumb: Thumb, rounding: f32, icon_px: f32, grayed: bool) -> AnyElement {
    match thumb {
        Thumb::Ready(image) => img(image)
            .size_full()
            .overflow_hidden()
            .object_fit(ObjectFit::Cover)
            .grayscale(grayed)
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

/// One heading line composed from its pieces. A run of adjacent stat pieces
/// joins into one muted span. An empty field's piece drops out, except the
/// name, which reads "Unknown" unless a shown album already names the line.
pub fn line_content(
    pieces: &[HeadPiece],
    head: &GroupHead,
    look: &HeadLook,
    expanded: bool,
) -> Div {
    let has_tile = expanded && head.tiled && look.show_art;
    let indent = look.art_margin + look.tile_side + tokens::SPACE_SM;
    let album_here = !head.album.is_empty() && pieces.contains(&HeadPiece::Album);
    let readings = rox_core::settings::show_readings();
    let mut row = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .px(tokens::SPACE_SM)
        .text_size(rems(look.font_scale))
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
                                .child(rox_i18n::t!("head-unknown")),
                        );
                    }
                } else {
                    row = row.child(
                        div()
                            .text_color(palette::text_bright())
                            // Expanded, the name leads and truncates at
                            // text_lg's 1.125 step; compact keeps it whole
                            // and lets the album truncate.
                            .map(|d| {
                                if expanded {
                                    d.min_w_0()
                                        .truncate()
                                        .text_size(rems(1.125 * look.font_scale))
                                } else {
                                    d.flex_none()
                                }
                            })
                            .child(crate::panel::named(
                                &head.name,
                                &head.name_reading,
                                readings,
                            )),
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
                            .child(crate::panel::named(
                                &head.album,
                                &head.album_reading,
                                readings,
                            )),
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
                    // The square carries the crop, same as the block tile.
                    row = row.child(
                        div()
                            .flex_none()
                            .w(side)
                            .h(side)
                            .overflow_hidden()
                            .child(art_content(thumb.clone(), look.art_rounding, 12., false)),
                    );
                }
            }
        }
    }
    flush_stats(row, &mut stats)
}

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

pub fn meta_content(head: &GroupHead, look: &HeadLook) -> Div {
    let mut pieces = stock_meta_line();
    if !look.show_details {
        pieces.retain(|p| !matches!(p, HeadPiece::Genre | HeadPiece::Quality));
    }
    line_content(&pieces, head, look, true)
}

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

    #[test]
    fn a_rate_reads_as_khz() {
        // Pin the locale, since the decimal mark comes from it.
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        assert_eq!(khz(44100), "44.1");
        assert_eq!(khz(48000), "48");
        assert_eq!(khz(96000), "96");
        assert_eq!(khz(88200), "88.2");
        assert_eq!(khz(22050), "22.05");
        assert_eq!(khz(0), "");
        rox_i18n::set_locale(Some("de"));
        assert_eq!(khz(44100), "44,1");
        assert_eq!(khz(48000), "48");
        rox_i18n::set_locale(None);
    }

    #[test]
    fn the_stream_shape_drops_what_it_lacks() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        assert_eq!(stream_format(16, 44100), "16/44.1 kHz");
        assert_eq!(stream_format(24, 96000), "24/96 kHz");
        assert_eq!(stream_format(0, 44100), "44.1 kHz");
        assert_eq!(stream_format(16, 0), "16 bit");
        assert_eq!(stream_format(0, 0), "");
    }

    #[test]
    fn the_group_line_joins_what_agrees() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        assert_eq!(
            quality(Some("flac"), 1006, 1006, 16, 44100),
            "flac 16/44.1 kHz 1006 kbps"
        );
        assert_eq!(
            quality(Some("mp3"), 192, 320, 0, 44100),
            "mp3 44.1 kHz 192-320 kbps"
        );
        // A mixed run: codec, depth, and rate zero out.
        assert_eq!(quality(None, 192, 320, 0, 0), "192-320 kbps");
        assert_eq!(quality(None, 0, 0, 0, 0), "");
        assert_eq!(quality(Some("wav"), 0, 0, 16, 48000), "wav 16/48 kHz");
    }
}
