//! Criteria typed into a plugin search (ADR 30, amended 2026-10-07):
//! `artist:`, `title:` and `album:` terms, and "quoted phrases". A service's
//! search matches loosely and ranks by its own idea of popularity, so a rare
//! song can sit pages under famous ones that share a word. rox holds every
//! result to the criteria itself, so they work on any plugin, and passes them
//! along for a plugin that can ask its service more precisely.
//!
//! Bare words only steer the service: they go out in the query and filter
//! nothing, so a plain search lists exactly what it did before.

use serde_json::{Map, Value, json};

use rox_library::fold::fold;
use rox_plugins::wire::{Entry, Node, NodeKind, Track};

/// The fields a search can pin, as typed before the colon.
pub const FIELDS: &[&str] = &["artist", "title", "album"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Artist,
    Title,
    Album,
}

impl Field {
    fn named(name: &str) -> Option<Field> {
        match name.to_ascii_lowercase().as_str() {
            "artist" => Some(Field::Artist),
            "title" => Some(Field::Title),
            "album" => Some(Field::Album),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Field::Artist => "artist",
            Field::Title => "title",
            Field::Album => "album",
        }
    }
}

/// One condition every kept row meets. `field: None` is a phrase, which any
/// of the row's names can hold.
#[derive(Clone, Debug, PartialEq)]
struct Term {
    field: Option<Field>,
    typed: String,
    folded: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Criteria {
    /// What the service searches for: every word typed, criteria included,
    /// with the syntax taken off.
    pub text: String,
    terms: Vec<Term>,
}

impl Criteria {
    /// Never fails: an unclosed quote runs to the end, and a word with a
    /// colon that isn't a known field ("Re:Stacks") is just a word.
    pub fn parse(typed: &str) -> Criteria {
        let mut words: Vec<String> = Vec::new();
        let mut terms = Vec::new();
        let mut rest = typed.trim_start();

        while !rest.is_empty() {
            let (token, after) = next_token(rest);
            rest = after.trim_start();

            let (field, value) = match token {
                Token::Phrase(value) => (None, value),

                Token::Word(word) => match word.split_once(':') {
                    Some((name, value)) if !value.is_empty() => match Field::named(name) {
                        Some(field) => (Some(field), value.to_string()),
                        None => {
                            words.push(word.to_string());
                            continue;
                        }
                    },

                    _ => {
                        words.push(word.to_string());
                        continue;
                    }
                },

                Token::FieldPhrase(field, value) => (Some(field), value),
            };

            let value = value.trim().to_string();
            if value.is_empty() {
                continue;
            }

            words.push(value.clone());
            terms.push(Term {
                field,
                folded: fold(&value),
                typed: value,
            });
        }

        Criteria {
            text: words.join(" "),
            terms,
        }
    }

    /// Whether anything typed filters results. A plain search has nothing to
    /// hold rows to and never pages past what the plugin sent.
    pub fn filters(&self) -> bool {
        !self.terms.is_empty()
    }

    /// The `criteria` param: each field's terms and the phrases, as typed.
    /// None for a plain search, so a plugin that never heard of criteria
    /// never sees the key.
    pub fn wire(&self) -> Option<Value> {
        if !self.filters() {
            return None;
        }

        let mut out = Map::new();
        for term in &self.terms {
            let key = term.field.map_or("phrases", Field::key);
            let list = out.entry(key).or_insert_with(|| json!([]));
            if let Value::Array(list) = list {
                list.push(Value::String(term.typed.clone()));
            }
        }

        Some(Value::Object(out))
    }

    /// The rows that meet every term. A heading goes with its rows, so one
    /// whose rows all dropped out goes too.
    pub fn keep(&self, entries: Vec<Entry>) -> Vec<Entry> {
        if !self.filters() {
            return entries;
        }

        let mut kept: Vec<Entry> = Vec::with_capacity(entries.len());
        let mut heading: Option<Entry> = None;

        for entry in entries {
            let meets = match &entry {
                Entry::Section(_) => {
                    heading = Some(entry);
                    continue;
                }

                Entry::Track(track) => self.track_meets(track),
                Entry::Node(node) => self.node_meets(node),
            };

            if meets {
                kept.extend(heading.take());
                kept.push(entry);
            }
        }

        kept
    }

    /// Whether any row in the entries counts, headings aside.
    pub fn any_rows(entries: &[Entry]) -> bool {
        entries
            .iter()
            .any(|entry| !matches!(entry, Entry::Section(_)))
    }

    fn track_meets(&self, track: &Track) -> bool {
        self.terms.iter().all(|term| {
            let names: &[&str] = match term.field {
                Some(Field::Artist) => &[&track.artist, &track.album_artist],
                Some(Field::Title) => &[&track.title],
                Some(Field::Album) => &[&track.album],
                None => &[
                    &track.title,
                    &track.artist,
                    &track.album_artist,
                    &track.album,
                ],
            };
            holds(names, &term.folded)
        })
    }

    /// A node has a title and a subtitle, and its kind says what they name.
    /// A field it can't speak to fails it: an artist node has no song title,
    /// and a playlist names none of the three.
    fn node_meets(&self, node: &Node) -> bool {
        self.terms.iter().all(|term| {
            let names: &[&str] = match (term.field, node.kind) {
                (None, _) => &[&node.title, &node.subtitle],
                (Some(Field::Artist), Some(NodeKind::Artist)) => &[&node.title],
                (Some(Field::Artist), Some(NodeKind::Album)) => &[&node.subtitle],
                (Some(Field::Album), Some(NodeKind::Album)) => &[&node.title],
                _ => &[],
            };
            holds(names, &term.folded)
        })
    }
}

fn holds(names: &[&str], folded: &str) -> bool {
    names.iter().any(|name| fold(name).contains(folded))
}

enum Token<'a> {
    Word(&'a str),
    Phrase(String),
    FieldPhrase(Field, String),
}

/// One token off the front of `rest`, which starts on a non-space.
fn next_token(rest: &str) -> (Token<'_>, &str) {
    if let Some(quoted) = rest.strip_prefix('"') {
        let (phrase, after) = until_quote(quoted);
        return (Token::Phrase(phrase.to_string()), after);
    }

    // `artist:"two words"` keeps its space inside the quotes.
    if let Some((name, quoted)) = rest.split_once(":\"")
        && !name.contains(char::is_whitespace)
        && let Some(field) = Field::named(name)
    {
        let (phrase, after) = until_quote(quoted);
        return (Token::FieldPhrase(field, phrase.to_string()), after);
    }

    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    (Token::Word(&rest[..end]), &rest[end..])
}

/// The text up to the closing quote, and what follows it. No closing quote
/// takes the rest.
fn until_quote(quoted: &str) -> (&str, &str) {
    match quoted.find('"') {
        Some(end) => (&quoted[..end], &quoted[end + 1..]),
        None => (quoted, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str, album: &str) -> Entry {
        Entry::Track(Track {
            key: title.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            ..Default::default()
        })
    }

    fn titles(entries: &[Entry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Track(track) => track.title.as_str(),
                Entry::Node(node) => node.title.as_str(),
                Entry::Section(section) => section.title.as_str(),
            })
            .collect()
    }

    #[test]
    fn a_plain_search_filters_nothing() {
        let criteria = Criteria::parse("  Dubmood   Ma Version ");

        assert_eq!(criteria.text, "Dubmood Ma Version");
        assert!(!criteria.filters());
        assert_eq!(criteria.wire(), None);
    }

    #[test]
    fn fields_and_phrases_parse_and_still_reach_the_service() {
        let criteria = Criteria::parse(r#"artist:"Bright White" title:version "ma ver" dirty"#);

        assert_eq!(criteria.text, "Bright White version ma ver dirty");
        assert_eq!(
            criteria.wire(),
            Some(json!({
                "artist": ["Bright White"],
                "title": ["version"],
                "phrases": ["ma ver"],
            }))
        );
    }

    /// The source browser's search completions are drawn from [`FIELDS`].
    #[test]
    fn every_hinted_field_parses() {
        for field in FIELDS {
            assert!(Criteria::parse(&format!("{field}:x")).filters(), "{field}");
        }
    }

    #[test]
    fn unknown_fields_and_bare_colons_stay_words() {
        let criteria = Criteria::parse("Re:Stacks artist: mood:dub");

        assert_eq!(criteria.text, "Re:Stacks artist: mood:dub");
        assert!(!criteria.filters());
    }

    #[test]
    fn an_unclosed_quote_runs_to_the_end() {
        let criteria = Criteria::parse(r#"title:"ma version"#);

        assert_eq!(criteria.wire(), Some(json!({ "title": ["ma version"] })));
    }

    /// The case this exists for: the song credits its second act in the
    /// title, so an artist filter would miss it and a phrase finds it.
    #[test]
    fn a_phrase_finds_a_name_credited_in_the_title() {
        let entries = vec![
            track("Better Man (Taylor's Version)", "Taylor Swift", "Red"),
            track(
                "Ma Version (Bright White Lightning vs Dubmood)",
                "Bright White Lightning",
                "Dirty Nails",
            ),
        ];

        let kept = Criteria::parse(r#""dubmood" "ma version""#).keep(entries.clone());
        assert_eq!(
            titles(&kept),
            ["Ma Version (Bright White Lightning vs Dubmood)"]
        );

        let kept = Criteria::parse("artist:dubmood").keep(entries);
        assert!(kept.is_empty());
    }

    #[test]
    fn matching_folds_case_and_accents() {
        let entries = vec![track("Halo", "Beyoncé", "I Am... Sasha Fierce")];

        assert_eq!(Criteria::parse("artist:BEYONCE").keep(entries).len(), 1);
    }

    #[test]
    fn nodes_answer_only_for_what_their_kind_names() {
        let node = |title: &str, subtitle: &str, kind| {
            Entry::Node(Node {
                id: title.into(),
                title: title.into(),
                subtitle: subtitle.into(),
                collection: false,
                kind: Some(kind),
                art: String::new(),
                values: Default::default(),
                home: false,
                flags: None,
            })
        };
        let entries = vec![
            node("Dubmood", "", NodeKind::Artist),
            node("Dirty Nails", "Dubmood", NodeKind::Album),
            node("Dubmood Mix", "Someone", NodeKind::Playlist),
        ];

        let kept = Criteria::parse("artist:dubmood").keep(entries.clone());
        assert_eq!(titles(&kept), ["Dubmood", "Dirty Nails"]);

        let kept = Criteria::parse(r#""dubmood""#).keep(entries);
        assert_eq!(titles(&kept), ["Dubmood", "Dirty Nails", "Dubmood Mix"]);
    }

    #[test]
    fn a_heading_goes_with_its_rows() {
        let section = |title: &str| {
            Entry::Section(rox_plugins::wire::Section {
                title: title.into(),
                layout: Default::default(),
            })
        };
        let entries = vec![
            section("Albums"),
            track("Nope", "Other", "Other"),
            section("Tracks"),
            track("Ma Version", "Bright White Lightning", "Dirty Nails"),
        ];

        let kept = Criteria::parse(r#"title:"ma version""#).keep(entries);
        assert_eq!(titles(&kept), ["Tracks", "Ma Version"]);
    }
}
