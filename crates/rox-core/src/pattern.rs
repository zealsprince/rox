//! Names built from a pattern: values in, a relative path out. A pattern
//! is literal text with `%field%` placeholders in it ("%artist% -
//! %title%"), and a "/" starts a folder level, so
//! "%albumartist%/%album%/%track% - %title%" names two folders and the
//! file inside them. foobar2000's masstagger idea, and the half of it
//! that writes rather than reads.
//!
//! It lives down here because parts of rox that can't see each other
//! render names the same way: the tag renamer and the conversion output
//! naming up in the app, the stream capture down in the services, and
//! Discord's presence card beside it. What they share is the grammar,
//! the escaping, the rule that a segment can't come out empty, and
//! [`Name`], the one set of placeholder names every box takes. What each
//! surface can fill is its own business: a caller brings a
//! [`PatternField`] and the engine never learns what a tag is.
//!
//! A name a surface has no value for still parses there and renders as
//! nothing, so a pattern written in one box works in the next one. That
//! is what makes the vocabulary worth sharing: without it every box
//! would have to print its own list, and typing %station% into the wrong
//! one would read as a mistake rather than as a blank.
//!
//! What isn't here either is matching, reading values back out of a path
//! a pattern describes. That only makes sense over files that already
//! exist and it stays with the tag guesser, on top of the same parse.
//!
//! A path is stricter than a line, so the two directions are separate
//! calls. [`parse`] and [`Pattern::render`] make file names: `%skip%` is
//! an error, values go through [`safe_file_stem`] so a slash in an
//! artist name can't open a folder, and an empty segment is refused
//! rather than quietly dropping a folder level. [`parse_line`] and
//! [`Pattern::render_line`] make one line of prose for a card or a
//! status, where a slash is a slash and an empty line is a line left
//! off.

use std::path::PathBuf;

use crate::settings::safe_file_stem;

/// One piece of a pattern component: text that must appear verbatim, a
/// field, or a swallowed segment.
pub enum Token<F> {
    Literal(String),
    Capture(F),
    Skip,
}

/// A parsed pattern: one token list per path component, deepest last.
pub struct Pattern<F> {
    components: Vec<Vec<Token<F>>>,
}

/// Every placeholder name a pattern may use, app-wide. One vocabulary
/// rather than one per box: a pattern learned in the rename dialog parses
/// in the capture row and in Discord's card, so nobody has to find out
/// which box speaks which dialect.
///
/// What differs per surface is what it can fill. A name a surface has no
/// value for renders nothing and takes its separator with it, which is
/// the [`PatternField`] implementation's call, made against this enum so
/// the compiler asks about every name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Name {
    Artist,
    AlbumArtist,
    Album,
    Title,
    Track,
    Disc,
    Year,
    /// A date rather than a year: the day a capture landed, where a
    /// surface has one. The rename dialog reads it as the release year,
    /// which is the only date a tagged file carries.
    Date,
    Genre,
    Comment,
    /// The station a stream came from, which doubles as its album.
    Station,
    /// The kind of source behind what's playing, "Radio" or "Subsonic".
    Source,
    /// The audio format, as much of it as the surface knows: "FLAC
    /// Lossless" off a tagged file, the container off a stream.
    Format,
    /// `%skip%`: swallow a piece while matching, and render nothing.
    /// Matching-only, so the surfaces that render don't list it.
    Skip,
}

/// The names as a pattern spells them, in the order the placeholder tip
/// lists them. %skip% is left out: it swallows text while matching and
/// renders nothing, so the surfaces that read a pattern forwards have no
/// use for it, and the one that matches says so in its own note.
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

/// The name a placeholder spells, with the aliases folded in. An unknown
/// name is a parse error, not a literal: a typoed `%tittle%` silently
/// matching as text would be far harder to spot in a preview.
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

/// What a pattern is allowed to name, and what each name does on the way
/// out. One implementation per set of values: the tag fields up in the
/// tag guesser, the capture's own set down in the services, and Discord's
/// card beside it.
pub trait PatternField: Clone + PartialEq + Sized {
    /// The field this surface fills a name from, None for a name it
    /// swallows rather than fills. Every surface answers for every
    /// [`Name`], because a pattern that parses in one box has to parse in
    /// all of them; a name with no value behind it takes a field whose
    /// [`fallback`](PatternField::fallback) is empty and renders nothing.
    fn from_name(name: Name) -> Option<Self>;

    /// What this renders as when the values carry nothing for it.
    ///
    /// An empty fallback is a field that's allowed to vanish: it takes
    /// one of the literals it sat between with it, so a pattern written
    /// for a full set of values doesn't leave its separators hanging.
    /// Everything else answers with a name, because a segment that
    /// renders to nothing is refused outright.
    fn fallback(&self) -> &'static str;

    /// The raw value on its way into a segment, before sanitizing. The
    /// hook numbers pad through; most fields only need the trim.
    fn value(&self, raw: &str) -> String {
        raw.trim().to_owned()
    }
}

/// Parse `text` into components, or say what is wrong with it: an unknown
/// placeholder, an unclosed %, or nothing to render at all.
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

/// Parse `text` as one line of prose rather than a path: "/" is literal
/// text, and a line with no placeholder in it at all is a pattern that
/// renders itself. Both are wrong for a file name, which is why [`parse`]
/// refuses them, and both are ordinary in a line someone reads.
///
/// Renders through [`Pattern::render_line`]. The grammar and the
/// placeholder names are the same ones the renamer uses, so a pattern
/// learned there reads the same here.
pub fn parse_line<F: PatternField>(text: &str) -> Result<Pattern<F>, String> {
    Ok(Pattern {
        components: vec![tokenize(text)?],
    })
}

/// One component's tokens: literal runs and the `%field%` placeholders
/// between them. Shared by both parses, since what differs between a path
/// and a line is the rules around the grammar rather than the grammar.
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

/// Clean up an assembled segment's edges: the literal text around a value
/// can leave a trailing space or dot that Windows silently eats and that
/// hides the file everywhere else.
fn trim_segment(segment: &str) -> &str {
    segment.trim().trim_matches('.').trim()
}

/// One rendered piece of a segment, kept apart until the holes are closed
/// because a literal beside an empty value is a separator with nothing
/// left to separate.
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

/// Close the holes an empty value leaves. Each one takes the literal that
/// followed it, or the one in front of it when it ends the segment, so
/// "%artist% - %title%" with no artist reads as the title rather than as
/// " - Title".
///
/// Only a field whose fallback is empty can ever get here, so a pattern
/// over tags never sees this: a missing album renders "Unknown Album" and
/// the separators around it still have both their sides.
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
    /// The parsed components, deepest last, for the matcher that runs the
    /// same parse the other way.
    pub fn components(&self) -> &[Vec<Token<F>>] {
        &self.components
    }

    /// Whether the pattern has a folder part at all: `%track% - %title%`
    /// is a file name alone, `%album%/%title%` names a folder above it.
    pub fn has_folders(&self) -> bool {
        self.components.len() > 1
    }

    /// Render `values` into a relative path, one component per "/" in the
    /// pattern and the deepest one the file name. Literals emit verbatim,
    /// values emit through [`safe_file_stem`], and a missing or empty one
    /// emits the field's fallback instead of nothing. %skip% is an error
    /// here: it exists to swallow text while matching, and there's
    /// nothing to swallow while emitting. The extension belongs to the
    /// source, so the caller appends it; the path that comes back has
    /// none.
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

    /// Render `values` into one line of text: the same holes-close rule as
    /// [`render`], none of the file-name rules. A value keeps its own
    /// punctuation because nothing here becomes a path, "/" stays where
    /// the pattern put it, and a line that comes out empty comes back
    /// empty rather than as an error, since a line with nothing in it is
    /// a line its surface leaves off.
    ///
    /// %skip% drops out for the same reason it's an error in [`render`]:
    /// it swallows text while matching and has nothing to swallow while
    /// emitting. A line is prose, so the pattern still renders without it
    /// instead of refusing.
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

    /// A vocabulary for the engine's own tests. The tag fields live two
    /// crates up now, and what's being tested here is the grammar rather
    /// than any particular set of names: two fields that always render,
    /// and one that's allowed to vanish, which every name this
    /// vocabulary doesn't spell out lands in.
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
                // Everything else is a name this vocabulary carries no
                // value for, the case every real surface has some of.
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

    /// A line keeps what a file name can't: the slash the pattern wrote,
    /// the punctuation inside a value, and a literal-only line with no
    /// placeholder to render.
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

    /// The holes-close rule carries over: a value that's allowed to
    /// vanish takes its separator with it rather than leaving the line
    /// opening on a dash.
    #[test]
    fn an_empty_value_takes_its_separator_along() {
        assert_eq!(
            render_line(
                "%comment% - %title%",
                &[(Word::Maybe, ""), (Word::Title, "Xtal")]
            ),
            "Xtal"
        );
        // Nothing to render at all is an empty line, not an error: the
        // surface leaves the line off.
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
        // A value that sanitizes down to nothing takes the same road.
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

    /// A field with an empty fallback takes a separator with it. No tag
    /// field is allowed to vanish, so this is the capture's rule being
    /// exercised where the mechanism lives.
    #[test]
    fn a_field_allowed_to_vanish_takes_its_separator() {
        // The separator after the hole goes, wherever the hole sits.
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

        // Nothing left at all is still a refusal.
        assert!(render("%comment%/%title%", &[(Word::Title, "Song")]).is_err());
    }
}
