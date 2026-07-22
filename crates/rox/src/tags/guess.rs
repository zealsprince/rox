//! Filename pattern guessing: tag values pulled out of a path by a
//! format string, foobar2000's masstagger idea. A pattern mixes literal
//! text with %field% placeholders - "%artist% - %title%" - and matches
//! against the file stem; a "/" in the pattern walks up the path, so
//! "%artist% - %album%/%track%. %title%" reads the folder name too.
//! %skip% swallows a segment without keeping it. Matching is non-greedy:
//! a capture takes the shortest text that lets the rest of the pattern
//! land, so the first " - " splits artist from title even when the title
//! carries one. Captures trim their edges and must be non-empty. The
//! editor previews every track through [`Pattern::apply`] before
//! anything is written, so a bad pattern costs nothing.

use std::path::Path;

use rox_library::writer::Field;

/// One piece of a pattern component: text that must appear verbatim, a
/// field capture, or a swallowed segment.
enum Token {
    Literal(String),
    Capture(Field),
    Skip,
}

/// A parsed pattern: one token list per path component, deepest last.
pub struct Pattern {
    components: Vec<Vec<Token>>,
}

/// The placeholder names a pattern may use, the help line's source of
/// truth. Aliases share a row.
pub const PLACEHOLDERS: &[&str] = &[
    "%artist%",
    "%albumartist%",
    "%album%",
    "%title%",
    "%track%",
    "%disc%",
    "%year%",
    "%genre%",
    "%comment%",
    "%skip%",
];

/// The field behind a placeholder name, or None for the skip marker.
/// Unknown names are a parse error, not a literal: a typoed %tittle%
/// silently matching as text would be far harder to spot in the preview.
fn field_for(name: &str) -> Result<Option<Field>, String> {
    Ok(Some(match name {
        "artist" => Field::Artist,
        "albumartist" | "album artist" => Field::AlbumArtist,
        "album" => Field::Album,
        "title" => Field::Title,
        "track" | "tracknumber" => Field::TrackNo,
        "disc" | "discnumber" => Field::DiscNo,
        "year" | "date" => Field::Year,
        "genre" => Field::Genre,
        "comment" => Field::Comment,
        "skip" | "dummy" | "ignore" => return Ok(None),
        other => return Err(format!("unknown placeholder %{other}%")),
    }))
}

/// Parse `pattern` into components, or say what is wrong with it: an
/// unknown placeholder, an unclosed %, or nothing to capture at all.
pub fn parse(pattern: &str) -> Result<Pattern, String> {
    let mut components = Vec::new();
    for part in pattern.split('/') {
        let mut tokens: Vec<Token> = Vec::new();
        let mut rest = part;
        while let Some(start) = rest.find('%') {
            if !rest[..start].is_empty() {
                tokens.push(Token::Literal(rest[..start].to_owned()));
            }
            let after = &rest[start + 1..];
            let Some(end) = after.find('%') else {
                return Err("unclosed %".into());
            };
            match field_for(&after[..end])? {
                Some(field) => tokens.push(Token::Capture(field)),
                None => tokens.push(Token::Skip),
            }
            rest = &after[end + 1..];
        }
        if !rest.is_empty() {
            tokens.push(Token::Literal(rest.to_owned()));
        }
        components.push(tokens);
    }
    let captures = components
        .iter()
        .flatten()
        .any(|t| matches!(t, Token::Capture(_)));
    if !captures {
        return Err("no placeholders".into());
    }
    Ok(Pattern { components })
}

/// Match `tokens` against `text` from the front, non-greedy, collecting
/// captures into `out`. On failure `out` is left as it was.
fn match_tokens(tokens: &[Token], text: &str, out: &mut Vec<(Field, String)>) -> bool {
    let Some(token) = tokens.first() else {
        return text.is_empty();
    };
    match token {
        Token::Literal(lit) => match text.strip_prefix(lit.as_str()) {
            Some(rest) => match_tokens(&tokens[1..], rest, out),
            None => false,
        },
        Token::Capture(_) | Token::Skip => {
            // Shortest capture first: every char boundary is a candidate
            // split, and the first one the rest of the pattern accepts
            // wins. A trailing capture takes the whole remainder in one
            // step since only the empty tail can close the list.
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
                if let Token::Capture(field) = token {
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
    /// Run the pattern over `path`: the last component against the file
    /// stem, earlier ones against the folders above it. A field captured
    /// twice keeps the deepest hit - the filename outranks the folder
    /// when both name the artist. None when any component fails to
    /// match, including a pattern deeper than the path itself.
    pub fn apply(&self, path: &Path) -> Option<Vec<(Field, String)>> {
        let mut names: Vec<String> = Vec::with_capacity(self.components.len());
        let mut at = path;
        for i in 0..self.components.len() {
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
        for (tokens, name) in self.components.iter().zip(&names) {
            if !match_tokens(tokens, name, &mut captures) {
                return None;
            }
        }
        // Deepest capture of a field wins; matching walked shallow to
        // deep, so keep each field's last entry.
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
            vec![(Field::TrackNo, "01".into()), (Field::Title, "Intro".into())]
        );
        let got = apply("%track% - %title%", "/m/07 - Outro.mp3").unwrap();
        assert_eq!(
            got,
            vec![(Field::TrackNo, "07".into()), (Field::Title, "Outro".into())]
        );
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
            vec![(Field::Artist, "Name".into()), (Field::Title, "Song".into())]
        );
    }

    #[test]
    fn parse_rejects_bad_patterns() {
        assert!(parse("%tittle%").is_err());
        assert!(parse("%artist").is_err());
        assert!(parse("plain text").is_err());
        assert!(parse("%skip%").is_err());
    }
}
