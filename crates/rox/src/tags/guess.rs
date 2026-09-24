//! Filename pattern guessing, foobar2000's masstagger idea: a pattern of
//! literal text and %field% placeholders matched against the file stem, with
//! "/" stepping up to the folders above. %skip% swallows a segment.
//! Matching is non-greedy, so the first " - " splits artist from title even
//! when the title contains one. Captures trim and must be non-empty.
//!
//! This module only reads paths. The same pattern runs the other way through
//! [`rox_core::pattern`] for renaming, conversion output and stream capture.

use std::path::{Path, PathBuf};

use rox_core::pattern::{Name, PatternField, Token};
use rox_library::writer::Field;

pub use rox_core::pattern::PLACEHOLDERS;

/// The tag fields as a pattern vocabulary. A wrapper because the trait and
/// [`Field`] both live in other crates; it never leaves this module.
#[derive(Clone, PartialEq)]
enum TagField {
    Tag(Field),
    /// A name the vocabulary parses but a file can't answer (station, source,
    /// format). A pattern carried over from another surface renders without it
    /// rather than being refused.
    Unfilled,
}

impl PatternField for TagField {
    fn from_name(name: Name) -> Option<Self> {
        Some(TagField::Tag(match name {
            Name::Artist => Field::Artist,
            Name::AlbumArtist => Field::AlbumArtist,
            Name::Album => Field::Album,
            Name::Title => Field::Title,
            Name::Track => Field::TrackNo,
            Name::Disc => Field::DiscNo,
            Name::Year | Name::Date => Field::Year,
            Name::Genre => Field::Genre,
            Name::Comment => Field::Comment,
            Name::Station | Name::Source | Name::Format => return Some(TagField::Unfilled),
            Name::Skip => return None,
        }))
    }

    /// No tag field may vanish: a missing album would collapse a folder level.
    /// Also the sanitizer's fallback for a value of pure punctuation.
    fn fallback(&self) -> &'static str {
        let field = match self {
            TagField::Tag(field) => field,
            TagField::Unfilled => return "",
        };

        match field {
            Field::Artist | Field::AlbumArtist => "Unknown Artist",
            Field::Album => "Unknown Album",
            Field::Title => "Untitled",
            Field::TrackNo | Field::DiscNo => "00",
            Field::Year => "Unknown Year",
            Field::Genre => "Unknown Genre",
            // %comment% lands here. Kept total rather than panicking.
            _ => "Unknown",
        }
    }

    fn value(&self, raw: &str) -> String {
        match self {
            TagField::Tag(Field::TrackNo | Field::DiscNo) => padded(raw),
            _ => raw.trim().to_owned(),
        }
    }
}

/// Two digits, so 3 sorts before 12. "3/12" keeps only the number; a
/// non-number like "A1" is left alone.
fn padded(value: &str) -> String {
    let head = value.split('/').next().unwrap_or(value).trim();

    match head.parse::<u32>() {
        Ok(n) => format!("{n:02}"),
        Err(_) => value.trim().to_owned(),
    }
}

pub struct Pattern(rox_core::pattern::Pattern<TagField>);

pub fn parse(text: &str) -> Result<Pattern, String> {
    rox_core::pattern::parse(text).map(Pattern)
}

/// On failure `out` is left as it was.
fn match_tokens(tokens: &[Token<TagField>], text: &str, out: &mut Vec<(Field, String)>) -> bool {
    let Some(token) = tokens.first() else {
        return text.is_empty();
    };
    match token {
        Token::Literal(lit) => match text.strip_prefix(lit.as_str()) {
            Some(rest) => match_tokens(&tokens[1..], rest, out),
            None => false,
        },
        Token::Capture(_) | Token::Skip => {
            // Shortest capture first: the first split the rest of the pattern accepts
            // wins.
            let ends = text
                .char_indices()
                .map(|(i, _)| i)
                .skip(1)
                .chain([text.len()]);
            for end in ends {
                let (taken, rest) = text.split_at(end);
                let trimmed = taken.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let mark = out.len();
                // An unfilled name matches like %skip%.
                if let Token::Capture(TagField::Tag(field)) = token {
                    out.push((field.clone(), trimmed.to_owned()));
                }
                if match_tokens(&tokens[1..], rest, out) {
                    return true;
                }
                out.truncate(mark);
            }
            false
        }
    }
}

impl Pattern {
    pub fn has_folders(&self) -> bool {
        self.0.has_folders()
    }

    pub fn render(&self, values: &[(Field, String)]) -> Result<PathBuf, String> {
        let values: Vec<(TagField, String)> = values
            .iter()
            .map(|(field, value)| (TagField::Tag(field.clone()), value.clone()))
            .collect();

        self.0.render(&values)
    }

    /// A field captured twice keeps the deepest hit, so the filename outranks
    /// the folder. None when any component fails to match.
    pub fn apply(&self, path: &Path) -> Option<Vec<(Field, String)>> {
        let components = self.0.components();
        let mut names: Vec<String> = Vec::with_capacity(components.len());
        let mut at = path;
        for i in 0..components.len() {
            let name = if i == 0 {
                at.file_stem()?.to_str()?.to_owned()
            } else {
                at.file_name()?.to_str()?.to_owned()
            };
            names.push(name);
            at = at.parent()?;
        }
        names.reverse();
        let mut captures = Vec::new();
        for (tokens, name) in components.iter().zip(&names) {
            if !match_tokens(tokens, name, &mut captures) {
                return None;
            }
        }
        let mut deduped: Vec<(Field, String)> = Vec::with_capacity(captures.len());
        for (field, value) in captures.into_iter().rev() {
            if !deduped.iter().any(|(f, _)| *f == field) {
                deduped.push((field, value));
            }
        }
        deduped.reverse();
        Some(deduped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn apply(pattern: &str, path: &str) -> Option<Vec<(Field, String)>> {
        parse(pattern).unwrap().apply(&PathBuf::from(path))
    }

    #[test]
    fn artist_and_title_split_on_first_separator() {
        let got = apply("%artist% - %title%", "/m/Boards - Roygbiv - Live.mp3").unwrap();
        assert_eq!(
            got,
            vec![
                (Field::Artist, "Boards".into()),
                (Field::Title, "Roygbiv - Live".into()),
            ]
        );
    }

    #[test]
    fn folder_component_reads_album() {
        let got = apply(
            "%artist% - %album%/%artist% - %title%",
            "/m/Boards - Geogaddi/Boards - Julie.flac",
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                (Field::Album, "Geogaddi".into()),
                (Field::Artist, "Boards".into()),
                (Field::Title, "Julie".into()),
            ]
        );
    }

    #[test]
    fn leading_track_number_variants() {
        let got = apply("%track%. %title%", "/m/01. Intro.mp3").unwrap();
        assert_eq!(
            got,
            vec![
                (Field::TrackNo, "01".into()),
                (Field::Title, "Intro".into())
            ]
        );
        let got = apply("%track% - %title%", "/m/07 - Outro.mp3").unwrap();
        assert_eq!(
            got,
            vec![
                (Field::TrackNo, "07".into()),
                (Field::Title, "Outro".into())
            ]
        );
    }

    /// A name this vocabulary can't fill still parses, so a capture or Discord
    /// pattern carries over.
    #[test]
    fn a_name_a_file_cannot_answer_carries_over_anyway() {
        let got = apply("%station% - %title%", "/m/Noise FM - Song.mp3").unwrap();
        assert_eq!(got, vec![(Field::Title, "Song".into())]);

        let rendered = parse("%artist% - %format%/%title%")
            .unwrap()
            .render(&[
                (Field::Artist, "Boards".into()),
                (Field::Title, "Julie".into()),
            ])
            .unwrap();
        assert_eq!(rendered, PathBuf::from("Boards/Julie"));
    }

    #[test]
    fn skip_swallows_without_capturing() {
        let got = apply("%skip% - %title%", "/m/03 - Song.mp3").unwrap();
        assert_eq!(got, vec![(Field::Title, "Song".into())]);
    }

    #[test]
    fn deepest_capture_wins_on_duplicate_fields() {
        let got = apply(
            "%artist% - %album%/%artist% - %title%",
            "/m/Various - Comp/Actual Artist - Song.mp3",
        )
        .unwrap();
        assert!(got.contains(&(Field::Artist, "Actual Artist".into())));
        assert!(!got.iter().any(|(_, v)| v == "Various"));
    }

    #[test]
    fn no_match_is_none() {
        assert!(apply("%track%. %title%", "/m/No Number Here.mp3").is_none());
    }

    #[test]
    fn captures_trim_but_never_go_empty() {
        assert!(apply("%artist% - %title%", "/m/ - Title.mp3").is_none());
        let got = apply("%artist%-%title%", "/m/Name - Song.mp3").unwrap();
        assert_eq!(
            got,
            vec![
                (Field::Artist, "Name".into()),
                (Field::Title, "Song".into())
            ]
        );
    }

    fn render(pattern: &str, values: &[(Field, &str)]) -> Result<String, String> {
        let values: Vec<(Field, String)> = values
            .iter()
            .map(|(f, v)| (f.clone(), (*v).to_owned()))
            .collect();

        parse(pattern)
            .unwrap()
            .render(&values)
            .map(|p| p.to_string_lossy().into_owned())
    }

    #[test]
    fn numbers_pad_to_two_digits() {
        assert_eq!(
            render(
                "%disc%-%track% %title%",
                &[
                    (Field::DiscNo, "2"),
                    (Field::TrackNo, "7/12"),
                    (Field::Title, "Outro"),
                ]
            )
            .unwrap(),
            "02-07 Outro"
        );
        assert_eq!(
            render(
                "%track% %title%",
                &[(Field::TrackNo, "123"), (Field::Title, "X")]
            )
            .unwrap(),
            "123 X"
        );
        assert_eq!(
            render(
                "%track% %title%",
                &[(Field::TrackNo, "A1"), (Field::Title, "X")]
            )
            .unwrap(),
            "A1 X"
        );
    }

    #[test]
    fn no_tag_field_vanishes_from_a_name() {
        assert_eq!(
            render(
                "%albumartist%/%album%/%track% %title%",
                &[(Field::Title, "Song")]
            )
            .unwrap(),
            "Unknown Artist/Unknown Album/00 Song"
        );
    }

    #[test]
    fn render_round_trips_through_apply() {
        let pattern = "%albumartist%/%album%/%track% - %title%";
        let values = [
            (Field::AlbumArtist, "Boards of Canada".to_owned()),
            (Field::Album, "Geogaddi".to_owned()),
            (Field::TrackNo, "4".to_owned()),
            (Field::Title, "Julie and Candy".to_owned()),
        ];
        let rendered = parse(pattern).unwrap().render(&values).unwrap();
        assert_eq!(
            rendered,
            PathBuf::from("Boards of Canada/Geogaddi/04 - Julie and Candy")
        );
        let full = PathBuf::from("/music")
            .join(&rendered)
            .with_extension("flac");
        let mut back = parse(pattern).unwrap().apply(&full).unwrap();
        back.sort_by_key(|(f, _)| format!("{f:?}"));
        let mut want: Vec<(Field, String)> = values.to_vec();
        want[2].1 = "04".to_owned();
        want.sort_by_key(|(f, _)| format!("{f:?}"));
        assert_eq!(back, want);
    }
}
