//! Text items (ADR 33): one line of now-playing text, written as a
//! `rox_core::pattern` line with the presence card's fields. The pattern
//! parses when the item changes and never while painting. Text is always
//! flat: its center rides the projection and its size follows the depth,
//! but it takes no tilt.

use std::borrow::Cow;

use gpui::{
    AnyElement, App, ContentMask, Font, Hsla, Rgba, SharedString, TextRun, canvas, point,
    prelude::*, px,
};
use rox_canvas::{Align, CanvasItem, CanvasState, Content};
use rox_core::pattern::{self, Name, Pattern, PatternField};
use rox_design::palette;
use rox_panel_api::panel::AppState;
use rox_services::discord_presence::{PresenceSample, format_quality};

use crate::canvas::fades::Mix;
use crate::canvas::{self, Body};

/// The now-playing fields a text item fills. Every placeholder name parses,
/// so a pattern written for the presence card works here too.
#[derive(Clone, PartialEq)]
pub(crate) enum CanvasField {
    Artist,
    AlbumArtist,
    Album,
    Title,
    Track,
    Year,
    Genre,
    Station,
    Source,
    Format,
    /// A name the canvas has no value for. It renders nothing.
    Unfilled,
}

impl PatternField for CanvasField {
    fn from_name(name: Name) -> Option<Self> {
        Some(match name {
            Name::Artist => CanvasField::Artist,
            Name::AlbumArtist => CanvasField::AlbumArtist,
            Name::Album => CanvasField::Album,
            Name::Title => CanvasField::Title,
            Name::Track => CanvasField::Track,
            Name::Year | Name::Date => CanvasField::Year,
            Name::Genre => CanvasField::Genre,
            Name::Station => CanvasField::Station,
            Name::Source => CanvasField::Source,
            Name::Format => CanvasField::Format,
            Name::Comment | Name::Disc => CanvasField::Unfilled,
            Name::Skip => return None,
        })
    }

    fn fallback(&self) -> &'static str {
        ""
    }
}

/// The names the pattern box's tip lists: the ones that fill.
pub(crate) const PLACEHOLDERS: &[&str] = &[
    "%artist%",
    "%albumartist%",
    "%album%",
    "%title%",
    "%track%",
    "%year%",
    "%genre%",
    "%station%",
    "%source%",
    "%format%",
];

/// What a new text item reads.
pub(crate) const NEW_TEXT: &str = "%title%";

pub(crate) type Values = Vec<(CanvasField, String)>;

/// A text item's line, parsed once per edit.
pub(crate) struct Line {
    raw: String,
    parsed: Result<Pattern<CanvasField>, String>,
}

impl Line {
    pub(crate) fn new(text: &str) -> Line {
        Line {
            raw: text.to_string(),
            parsed: pattern::parse_line(text),
        }
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.parsed.as_ref().err().map(String::as_str)
    }

    /// A line that doesn't parse paints as typed, so a stray `%` shows
    /// where it is instead of blanking the item.
    pub(crate) fn render(&self, values: &[(CanvasField, String)]) -> String {
        match &self.parsed {
            Ok(pattern) => pattern.render_line(values),
            Err(_) => self.raw.clone(),
        }
    }
}

/// The values for the playing track. Nothing playing gives none, and every
/// hole closes.
pub(crate) fn values(sample: Option<&PresenceSample>) -> Values {
    let Some(sample) = sample else {
        return Vec::new();
    };

    vec![
        (CanvasField::Artist, sample.artist.clone()),
        (CanvasField::AlbumArtist, sample.album_artist.clone()),
        (CanvasField::Album, sample.album.clone()),
        (CanvasField::Title, sample.title.clone()),
        (CanvasField::Track, sample.track.clone()),
        (CanvasField::Year, sample.year.clone()),
        (CanvasField::Genre, sample.genre.clone()),
        (CanvasField::Station, sample.station.clone()),
        (CanvasField::Source, sample.source.clone()),
        (
            CanvasField::Format,
            format_quality(&sample.codec, sample.bitrate_kbps),
        ),
    ]
}

/// Reads the library's store, so it runs once per track or tag change and
/// the result is held, never once per frame.
pub(crate) fn now_playing(state: &AppState, cx: &App) -> Values {
    let sample = PresenceSample::playing(state.player.read(cx), state.library.read(cx));
    values(sample.as_ref())
}

pub(crate) fn holds_text(canvas: &CanvasState) -> bool {
    let mut found = false;
    canvas.walk(&mut |item, _| found |= matches!(item.content, Content::Text { .. }));
    found
}

/// The line built for `id`, groups descended.
pub(crate) fn line_of<'a, G>(
    items: &'a [CanvasItem],
    bodies: &'a [Body<G>],
    id: &str,
) -> Option<&'a Line> {
    for (item, body) in items.iter().zip(bodies) {
        match (&item.content, body) {
            (_, Body::Text(line)) if item.id == id => return Some(line),
            (Content::Group { items }, Body::Group(bodies)) => {
                if let Some(line) = line_of(items, bodies, id) {
                    return Some(line);
                }
            }
            _ => {}
        }
    }

    None
}

/// A palette role by name, the way a Custom Controls case reads its color,
/// or a hex color. Anything else is the text color.
pub(crate) fn color(name: &str) -> Rgba {
    if let Some(role) = palette::ROLES.iter().find(|role| role.name == name) {
        return (role.get)(&palette::resolved());
    }

    palette::parse_hex(name).unwrap_or_else(palette::text)
}

/// The colors a case's ease and the looks blend this frame, each part named
/// as [`color`] reads it, None for the item's own `own`.
pub(crate) fn mixed(colors: &Mix, own: &str) -> Rgba {
    let mut out = Rgba {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 0.,
    };
    let mut total = 0.;

    for (name, weight) in colors.parts() {
        let part = color(name.unwrap_or(own));
        out.r += part.r * weight;
        out.g += part.g * weight;
        out.b += part.b * weight;
        out.a += part.a * weight;
        total += weight;
    }

    if total <= 0. {
        return color(own);
    }

    Rgba {
        r: out.r / total,
        g: out.g / total,
        b: out.b / total,
        a: out.a / total,
    }
}

/// How one text item paints this frame.
pub(crate) struct Paint {
    pub text: SharedString,
    /// None takes the window's font.
    pub family: Option<String>,
    pub size: f32,
    pub color: Rgba,
    pub opacity: f32,
    pub align: Align,
}

/// The line shaped and painted raw inside the item's box, clipped to it.
pub(crate) fn element(paint: Paint, corners: &[[f32; 2]; 4]) -> AnyElement {
    let (origin, extent) = canvas::bbox(corners);

    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            if paint.text.is_empty() || paint.size <= 0. {
                return;
            }

            let base = window.text_style().font();
            let font = match &paint.family {
                Some(family) => Font {
                    family: SharedString::from(family.clone()),
                    ..base
                },
                None => base,
            };

            let mut color: Hsla = paint.color.into();
            color.a *= paint.opacity.clamp(0., 1.);

            // A pattern's tag value or a hand-edited line can carry a newline.
            let text: SharedString = match rox_panel_kit::one_line(&paint.text) {
                Cow::Borrowed(_) => paint.text.clone(),
                Cow::Owned(text) => text.into(),
            };

            let run = TextRun {
                len: text.len(),
                font,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let font_size = px(paint.size);
            let line = window
                .text_system()
                .shape_line(text, font_size, &[run], None);

            let width = f32::from(bounds.size.width);
            let spare = width - f32::from(line.width);
            let x = match paint.align {
                Align::Left => 0.,
                Align::Center => spare / 2.,
                Align::Right => spare,
            };
            let line_height = font_size * 1.25;
            let y = (f32::from(bounds.size.height) - f32::from(line_height)) / 2.;
            let at = point(bounds.origin.x + px(x), bounds.origin.y + px(y));

            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let _ = line.paint(at, line_height, window, cx);
            });
        },
    )
    .absolute()
    .left(px(origin[0]))
    .top(px(origin[1]))
    .w(px(extent[0]))
    .h(px(extent[1]))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(artist: &str, title: &str) -> Values {
        vec![
            (CanvasField::Artist, artist.to_string()),
            (CanvasField::Title, title.to_string()),
        ]
    }

    #[test]
    fn an_empty_field_closes_its_hole() {
        let line = Line::new("%artist% - %title%");

        assert_eq!(
            line.render(&playing("Boards", "Roygbiv")),
            "Boards - Roygbiv"
        );
        assert_eq!(line.render(&playing("", "Roygbiv")), "Roygbiv");
        assert_eq!(line.render(&playing("Boards", "")), "Boards");
    }

    #[test]
    fn a_bad_pattern_keeps_its_text() {
        let unclosed = Line::new("%artist - %title%");
        assert!(unclosed.error().is_some());
        assert_eq!(
            unclosed.render(&playing("Boards", "Roygbiv")),
            "%artist - %title%"
        );

        let unknown = Line::new("%tittle%");
        assert!(unknown.error().is_some());
        assert_eq!(unknown.render(&[]), "%tittle%");

        assert!(Line::new("%title%").error().is_none());
    }

    #[test]
    fn nothing_playing_renders_the_literals() {
        let none = values(None);
        assert!(none.is_empty());

        assert_eq!(Line::new("Now playing").render(&none), "Now playing");
        assert_eq!(Line::new("%artist% - %title%").render(&none), "");
    }

    #[test]
    fn a_color_reads_a_role_or_hex() {
        let accent = palette::ROLES.iter().find(|role| role.name == "accent");
        assert_eq!(
            Some(color("accent")),
            accent.map(|role| (role.get)(&palette::resolved()))
        );
        assert_eq!(color("#ff0000"), palette::parse_hex("#ff0000").unwrap());
        assert_eq!(color("not a color"), palette::text());
    }
}
