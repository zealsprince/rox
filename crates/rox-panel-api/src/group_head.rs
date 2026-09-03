//! The album heading surface shared by the library table and the tree
//! panels. An album run reads as a block of composed lines: each line is an
//! ordered list of [`HeadPiece`]s, so a layout can put the artist, album,
//! year, and stats wherever it wants them, with a cover tile spanning the
//! block. Each caller resolves a [`GroupHead`] from whatever metadata it
//! holds (the library from its projection, the playlists tree from its
//! member rows) and lays the content over its own row background, so the
//! headings stay one look. The stock lines here are the classic two-line
//! arrangement; the library's config stores its own.

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, rems, svg, AnyElement, Div,
    ObjectFit, Pixels, SharedString,
};
use serde::{Deserialize, Serialize};

use crate::panel::ArrangeSpec;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::motif;
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
/// config stores each line as an ordered piece list; the resolved
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
    /// sibling, for layouts that put the art on a row instead.
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

/// Covers in a genre tile's mosaic when it has that many albums to show,
/// shared by the genre grid's wall and the library's genre headers.
pub const MOSAIC: usize = 4;

/// How a genre tile is drawn, shared by the genre grid and the library's
/// genre-grouped headers. Tinted is the default: the covers still read as
/// "your music" while the genre's own color marks which one at a glance.
/// Mosaic is the plain covers; Gradient and Color are cards in the
/// genre's color (a two-stop lean or a flat fill) decorated with the
/// genre's own geometry.
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

/// The full piece catalog in stock order: what the arrange editors offer,
/// and where a re-shown piece slots back in.
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
    /// The name's sort name, drawn after it as a reading when the switch
    /// is on and the name needs one. Empty for a caller that has none, and
    /// for the groupings (year, genre) whose field has no sort name.
    pub name_reading: SharedString,
    /// The album, shown on the meta line (expanded) or beside the name
    /// (compact). Empty when the grouping is not by album.
    pub album: SharedString,
    /// The album's sort name, the reading beside it. Empty like
    /// [`GroupHead::name_reading`] when there's nothing to read.
    pub album_reading: SharedString,
    /// The year on the name line; 0 hides it.
    pub year: u16,
    pub genre: SharedString,
    /// The codec, stream shape, and bitrate line, from [`quality`].
    pub quality: SharedString,
    pub tracks: u32,
    pub total_ms: u64,
    /// Whether the block shows the cover tile beside its lines, which then
    /// indent past it. The caller decides what has a cover to show: albums
    /// always, the library's artist and genre groupings too, year never.
    pub tiled: bool,
    /// The group's cover, resolved by the caller only when a line includes
    /// the inline art piece; None drops the piece from the line.
    pub thumb: Option<Thumb>,
}

/// The knobs that shape a heading's look, copied from the panel's config.
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
    /// The composed lines' text size as a rem factor, so the text follows
    /// the line height instead of floating small in a tall line. 1 keeps
    /// the stock sizes; the name line's lead multiplies its usual step on
    /// top.
    pub font_scale: f32,
}

/// A sample rate as the kHz a spec sheet writes: 44100 reads "44.1",
/// 48000 reads "48", 22050 reads "22.05". Empty at zero, the value both an
/// unread stream and a mixed group have.
pub fn khz(hz: u32) -> String {
    if hz == 0 {
        return String::new();
    }
    // Hundredths of a kHz, kept integer the whole way: 22050 through an f32
    // divide comes out at 22.04999. Two places is as far as any rate in the wild
    // needs, and the zeros come off after, so nothing grows a decimal it
    // didn't earn.
    let hundredths = (hz + 5) / 10;
    // How many places the rate has earned, still decided on the integers.
    // Only the decimal mark itself is a locale question, and a German
    // spec sheet writes 44,1 kHz, so the join goes through ICU rather
    // than a hardcoded dot.
    let places = match hundredths % 100 {
        0 => 0,
        rest if rest % 10 == 0 => 1,
        _ => 2,
    };
    rox_i18n::format::format_float(f64::from(hundredths) / 100.0, places)
}

/// The stream's shape the way a spec sheet writes it: "16/44.1 kHz" for a
/// lossless file, the rate alone for a lossy one (which has no depth to
/// name once the stream is coefficients), the depth alone if that's all
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
/// any part dropping out when it's mixed across the run or was never
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
/// in order, so the last one's draw is the one that shows, and it's a
/// single unclipped quad. Clipping a slice per row instead leaves hairline
/// seams at the boundaries once a font scale puts the rows on fractional
/// pixels. The same image handle every time decodes once. Pending and
/// missing use the same quiet placeholder, so a cover that arrives later
/// fills the tile without shifting the text beside it. The knob's radius
/// is applied to the cover itself, since gpui content masks stay rectangular.
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

/// The genre grouping's block tile, drawn with the configured [`TileFace`]
/// the genre grid's tiles use: the cover mosaic plain, the covers
/// grayscaled under the genre's color wash, or a card in the genre's
/// color under its geometry motif. The card leaves the name off; the
/// header's own line sets it right beside the tile. Same frame mechanics
/// as [`tile`]; the covers are a two-by-two mosaic once `thumbs` has
/// [`MOSAIC`] of them, the lone first cover below that.
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
        // The gradient leans the genre's own way, the grid's angle rule,
        // so neighbors sharing a hue family still tilt apart.
        (TileFace::Gradient, _) => card(
            linear_gradient(
                ((seed >> 45) % 360) as f32,
                linear_color_stop(color, 0.0),
                linear_color_stop(partner, 1.0),
            ),
            color,
        ),
        // The wash over the grayscaled covers makes the tinted face:
        // identity in color, music underneath.
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
        // A coverless genre on the tinted face borrows the card, the
        // grid's move, so the tile still shows which genre it is.
        (TileFace::Tinted, None) if !name.is_empty() => card(color.into(), color),
        (_, Some(covers)) => covers,
        (_, None) => art_content(Thumb::Missing, rounding, 16., false),
    };
    tile_frame(content, side, lift, art_side, margin)
}

/// Four covers as a two-by-two mosaic filling the tile square, each
/// quadrant rounding only its outer corner per the knob. A quadrant whose
/// cover is still loading (or gone) draws as a quiet elevated square, so
/// covers that arrive later fill in without a shift.
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

/// The tile's placement frame, shared by the single cover and the mosaic:
/// the block-spanning square at its margin on the configured side, lifted
/// past the block rows already painted.
///
/// The square is also the crop. `Cover` scales the art until it fills the
/// tile, so a sleeve that isn't square overruns the tile on its long side,
/// and that overrun paints over the track rows above and below the block.
/// The clip has to sit on this box rather than on the image, the way the
/// mosaic's `cell` already does it.
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

/// A cover's face: the image rounded per the knob, or the quiet music-note
/// placeholder used for both pending and missing, so a cover that arrives
/// later fills in without a layout shift. Shared by the block tile, the inline piece, and
/// the track info panel's art piece.
///
/// `Cover` scales the art until it fills the square, which leaves the odd
/// side hanging outside the element. The crop is the caller's to make, on
/// the sized box this goes into: `overflow_hidden` here is the image
/// masking against its own grown bounds, which crops nothing.
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
        // The text follows the line height through the caller's factor;
        // the pieces inherit it, the lead below steps up from it.
        .text_size(rems(look.font_scale))
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
                                .child(rox_i18n::t!("head-unknown")),
                        );
                    }
                } else {
                    row = row.child(
                        div()
                            .text_color(palette::text_bright())
                            // Expanded, the name is the block's lead and
                            // gives way by truncating; compact keeps it
                            // whole and lets the album truncate instead.
                            // The lead's step (text_lg's 1.125) multiplies
                            // the line-height factor with the rest.
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
                    // The square carries the crop, same as the block tile:
                    // a non-square sleeve's `Cover` overrun would otherwise
                    // paint out over the line's text.
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
        // The decimal mark comes from the locale now, so the assertions
        // pin one rather than reading whatever the OS negotiated to.
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        assert_eq!(khz(44100), "44.1");
        assert_eq!(khz(48000), "48");
        assert_eq!(khz(96000), "96");
        assert_eq!(khz(88200), "88.2");
        assert_eq!(khz(22050), "22.05");
        assert_eq!(khz(0), "");
        // The same rate in a comma locale, which is the whole point of
        // routing the join through ICU.
        rox_i18n::set_locale(Some("de"));
        assert_eq!(khz(44100), "44,1");
        assert_eq!(khz(48000), "48");
        rox_i18n::set_locale(None);
    }

    /// The stream shape pairs the depth with the rate, and drops whichever
    /// half is missing: a lossy file has no depth to name.
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

    /// The group line joins whatever agrees across the run: everything for
    /// a lossless album, the spread when the bitrate varies, and nothing
    /// once the run has nothing in common.
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
        // A mixed run: the codec, depth, and rate all zero out, leaving
        // the bitrate spread on its own.
        assert_eq!(quality(None, 192, 320, 0, 0), "192-320 kbps");
        assert_eq!(quality(None, 0, 0, 0, 0), "");
        // The bitrate alone stays what it always was.
        assert_eq!(quality(Some("wav"), 0, 0, 16, 48000), "wav 16/48 kHz");
    }
}
