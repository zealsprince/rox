//! Names built from a pattern: literal text with `%field%` placeholders,
//! where "/" starts a folder level. The renamer, conversion naming, stream
//! capture, and the Discord card all render through it, sharing the grammar,
//! the escaping, and [`Name`]. Each surface brings its own [`PatternField`];
//! a name it can't fill still parses and renders nothing, so a pattern works
//! in every box. Matching values back out of a path stays with the tag
//! guesser.
//!
//! [`parse`] and [`Pattern::render`] make file names: `%skip%` is an error,
//! values go through [`safe_file_stem`], and an empty segment is refused.
//! [`parse_line`] and [`Pattern::render_line`] make one line of prose.

use std::path::PathBuf;

use crate::settings::safe_file_stem;

pub enum Token<F> {
    Literal(String),
    Capture(F),
    Skip,
}

pub struct Pattern<F> {
    components: Vec<Vec<Token<F>>>,
}

/// Every placeholder name, app-wide. Implementors match on this so the
/// compiler asks about every name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Name {
    Artist,
    AlbumArtist,
    Album,
    Title,
    Track,
    Disc,
    Year,
    /// The day a capture landed; the renamer reads it as the release year.
    Date,
    Genre,
    Comment,
    Station,
    Source,
    Format,
    /// Matching-only: swallows a piece and renders nothing.
    Skip,
}

/// In the order the placeholder tip lists them. `%skip%` is left out: it's
/// matching-only.
pub const PLACEHOLDERS: &[&str] = &[
    "%artist%",
    "%albumartist%",
    "%album%",
    "%title%",
    "%track%",
    "%disc%",
    "%year%",
    "%date%",
    "%genre%",
    "%comment%",
    "%station%",
    "%source%",
    "%format%",
];

/// An unknown name is a parse error, not a literal: a typoed `%tittle%`
/// passing as text would be hard to spot in a preview.
fn name(text: &str) -> Result<Name, String> {
    Ok(match text {
        "artist" => Name::Artist,
        "albumartist" | "album artist" => Name::AlbumArtist,
        "album" => Name::Album,
        "title" => Name::Title,
        "track" | "tracknumber" => Name::Track,
        "disc" | "discnumber" => Name::Disc,
        "year" => Name::Year,
        "date" => Name::Date,
        "genre" => Name::Genre,
        "comment" => Name::Comment,
        "station" => Name::Station,
        "source" => Name::Source,
        "format" => Name::Format,
        "skip" | "dummy" | "ignore" => Name::Skip,
        other => {
            return Err(
                rox_i18n::t!("tags-guess-unknown-placeholder", name = other.to_owned()).to_string(),
            );
        }
    })
}

pub trait PatternField: Clone + PartialEq + Sized {
    /// None for a name it swallows. Every surface answers for every [`Name`];
    /// one with no value takes a field whose fallback is empty.
    fn from_name(name: Name) -> Option<Self>;

    /// An empty fallback lets the field vanish, taking a separator with it.
    /// Anything else must name something, since an empty segment is refused.
    fn fallback(&self) -> &'static str;

    /// The hook numbers pad through; most fields only need the trim.
    fn value(&self, raw: &str) -> String {
        raw.trim().to_owned()
    }
}

pub fn parse<F: PatternField>(text: &str) -> Result<Pattern<F>, String> {
    let mut components = Vec::new();

    for part in text.split('/') {
        components.push(tokenize(part)?);
    }

    let captures = components
        .iter()
        .flatten()
        .any(|t| matches!(t, Token::Capture(_)));
    if !captures {
        return Err(rox_i18n::t!("tags-guess-no-placeholders").to_string());
    }

    Ok(Pattern { components })
}

/// "/" is literal here and a pattern with no placeholder renders itself,
/// both of which [`parse`] refuses for a file name.
pub fn parse_line<F: PatternField>(text: &str) -> Result<Pattern<F>, String> {
    Ok(Pattern {
        components: vec![tokenize(text)?],
    })
}

fn tokenize<F: PatternField>(part: &str) -> Result<Vec<Token<F>>, String> {
    let mut tokens: Vec<Token<F>> = Vec::new();
    let mut rest = part;

    while let Some(start) = rest.find('%') {
        if !rest[..start].is_empty() {
            tokens.push(Token::Literal(rest[..start].to_owned()));
        }

        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            return Err(rox_i18n::t!("tags-guess-unclosed").to_string());
        };

        match F::from_name(name(&after[..end])?) {
            Some(field) => tokens.push(Token::Capture(field)),
            None => tokens.push(Token::Skip),
        }

        rest = &after[end + 1..];
    }

    if !rest.is_empty() {
        tokens.push(Token::Literal(rest.to_owned()));
    }

    Ok(tokens)
}

/// Trailing spaces and dots are eaten by Windows and hide the file elsewhere.
fn trim_segment(segment: &str) -> &str {
    segment.trim().trim_matches('.').trim()
}

enum Piece {
    Literal(String),
    Value(String),
}

impl Piece {
    fn text(&self) -> &str {
        match self {
            Piece::Literal(text) | Piece::Value(text) => text,
        }
    }
}

/// Each empty value takes the literal after it, or before it at the end of
/// the segment, so "%artist% - %title%" with no artist reads "Title".
fn collapse(pieces: &mut Vec<Piece>) {
    let mut i = 0;

    while i < pieces.len() {
        if !matches!(&pieces[i], Piece::Value(value) if value.is_empty()) {
            i += 1;
            continue;
        }

        if matches!(pieces.get(i + 1), Some(Piece::Literal(_))) {
            pieces.remove(i + 1);
        } else if i > 0 && matches!(pieces.get(i - 1), Some(Piece::Literal(_))) {
            i -= 1;
            pieces.remove(i);
        }

        pieces.remove(i);
    }
}

impl<F: PatternField> Pattern<F> {
    pub fn components(&self) -> &[Vec<Token<F>>] {
        &self.components
    }

    pub fn has_folders(&self) -> bool {
        self.components.len() > 1
    }

    /// Missing values emit the field's fallback. The caller appends the
    /// extension.
    pub fn render(&self, values: &[(F, String)]) -> Result<PathBuf, String> {
        let mut path = PathBuf::new();

        for tokens in &self.components {
            let mut pieces: Vec<Piece> = Vec::with_capacity(tokens.len());

            for token in tokens {
                match token {
                    Token::Literal(lit) => pieces.push(Piece::Literal(lit.clone())),

                    Token::Skip => {
                        return Err(rox_i18n::t!("tags-guess-skip-renders-nothing").to_string());
                    }

                    Token::Capture(field) => {
                        let raw = values
                            .iter()
                            .find(|(f, _)| f == field)
                            .map(|(_, v)| v.as_str())
                            .unwrap_or_default();
                        let value = field.value(raw);

                        pieces.push(Piece::Value(safe_file_stem(&value, field.fallback())));
                    }
                }
            }

            collapse(&mut pieces);

            let segment: String = pieces.iter().map(Piece::text).collect();
            let trimmed = trim_segment(&segment);
            if trimmed.is_empty() {
                return Err(rox_i18n::t!("tags-guess-empty-segment").to_string());
            }

            path.push(trimmed);
        }

        Ok(path)
    }

    /// The same holes-close rule as [`render`](Self::render), none of the
    /// file-name rules. An empty line comes back empty, not as an error, and
    /// `%skip%` drops out.
    pub fn render_line(&self, values: &[(F, String)]) -> String {
        let mut components = Vec::with_capacity(self.components.len());

        for tokens in &self.components {
            let mut pieces: Vec<Piece> = Vec::with_capacity(tokens.len());

            for token in tokens {
                match token {
                    Token::Literal(lit) => pieces.push(Piece::Literal(lit.clone())),

                    Token::Skip => {}

                    Token::Capture(field) => {
                        let raw = values
                            .iter()
                            .find(|(f, _)| f == field)
                            .map(|(_, v)| v.as_str())
                            .unwrap_or_default();
                        let value = field.value(raw);
                        let value = if value.is_empty() {
                            field.fallback().to_owned()
                        } else {
                            value
                        };

                        pieces.push(Piece::Value(value));
                    }
                }
            }

            collapse(&mut pieces);

            components.push(pieces.iter().map(Piece::text).collect::<String>());
        }

        components.join("/").trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two fields that always render and one allowed to vanish, which every
    /// other name lands in.
    #[derive(Clone, PartialEq)]
    enum Word {
        Artist,
        Title,
        Maybe,
    }

    impl PatternField for Word {
        fn from_name(name: super::Name) -> Option<Self> {
            match name {
                super::Name::Artist => Some(Word::Artist),
                super::Name::Title => Some(Word::Title),
                super::Name::Skip => None,
                _ => Some(Word::Maybe),
            }
        }

        fn fallback(&self) -> &'static str {
            match self {
                Word::Artist => "Unknown Artist",
                Word::Title => "Untitled",
                Word::Maybe => "",
            }
        }
    }

    fn render(pattern: &str, values: &[(Word, &str)]) -> Result<String, String> {
        let values: Vec<(Word, String)> = values
            .iter()
            .map(|(f, v)| (f.clone(), (*v).to_owned()))
            .collect();

        parse::<Word>(pattern)
            .unwrap()
            .render(&values)
            .map(|p| p.to_string_lossy().into_owned())
    }

    fn render_line(pattern: &str, values: &[(Word, &str)]) -> String {
        let values: Vec<(Word, String)> = values
            .iter()
            .map(|(f, v)| (f.clone(), (*v).to_owned()))
            .collect();

        parse_line::<Word>(pattern).unwrap().render_line(&values)
    }

    #[test]
    fn a_line_keeps_what_a_file_name_cannot() {
        assert_eq!(
            render_line(
                "%artist% / %title%",
                &[(Word::Artist, "AC/DC"), (Word::Title, "T.N.T.")]
            ),
            "AC/DC / T.N.T."
        );
        assert_eq!(render_line("Listening", &[]), "Listening");
    }

    #[test]
    fn an_empty_value_takes_its_separator_along() {
        assert_eq!(
            render_line(
                "%comment% - %title%",
                &[(Word::Maybe, ""), (Word::Title, "Xtal")]
            ),
            "Xtal"
        );
        assert_eq!(render_line("%comment%", &[(Word::Maybe, "")]), "");
    }

    #[test]
    fn values_cannot_escape_their_segment() {
        assert_eq!(
            render(
                "%artist%/%title%",
                &[(Word::Artist, "AC/DC"), (Word::Title, "Who Made Who?")]
            )
            .unwrap(),
            "AC DC/Who Made Who"
        );
        assert_eq!(
            render(
                "%artist% - %title%",
                &[(Word::Artist, "../etc"), (Word::Title, "x:y|z")]
            )
            .unwrap(),
            "etc - x y z"
        );
    }

    #[test]
    fn empty_values_fall_back_instead_of_vanishing() {
        assert_eq!(
            render("%artist%/%title%", &[(Word::Title, "Song")]).unwrap(),
            "Unknown Artist/Song"
        );
        assert_eq!(
            render(
                "%artist% - %title%",
                &[(Word::Artist, "///"), (Word::Title, "Song")]
            )
            .unwrap(),
            "Unknown Artist - Song"
        );
    }

    #[test]
    fn segments_never_end_in_space_or_dot() {
        assert_eq!(
            render(
                "%artist% - /%title%.",
                &[(Word::Artist, "Name"), (Word::Title, "Song")]
            )
            .unwrap(),
            "Name -/Song"
        );
    }

    #[test]
    fn render_rejects_skip_and_empty_segments() {
        assert!(
            parse::<Word>("%skip% - %title%")
                .unwrap()
                .render(&[(Word::Title, "Song".into())])
                .is_err()
        );
        assert!(
            parse::<Word>("%artist%//%title%")
                .unwrap()
                .render(&[(Word::Artist, "A".into()), (Word::Title, "B".into())])
                .is_err()
        );
    }

    #[test]
    fn parse_rejects_bad_patterns() {
        assert!(parse::<Word>("%tittle%").is_err());
        assert!(parse::<Word>("%artist").is_err());
        assert!(parse::<Word>("plain text").is_err());
        assert!(parse::<Word>("%skip%").is_err());
    }

    #[test]
    fn a_field_allowed_to_vanish_takes_its_separator() {
        assert_eq!(
            render("%comment% - %title%", &[(Word::Title, "Song")]).unwrap(),
            "Song"
        );
        assert_eq!(
            render("%title% - %comment%", &[(Word::Title, "Song")]).unwrap(),
            "Song"
        );
        assert_eq!(
            render(
                "%comment% - %title%",
                &[(Word::Maybe, "Band"), (Word::Title, "Song")]
            )
            .unwrap(),
            "Band - Song"
        );

        assert!(render("%comment%/%title%", &[(Word::Title, "Song")]).is_err());
    }
}
