//! Matching a song named by artist and title, as an online service names it,
//! to the rows that hold it: exact after folding, then with feature credits
//! dropped, and a bracketed qualifier gets a last look. Anything ambiguous
//! matches nothing, for a human to settle.

use std::collections::{HashMap, HashSet};

use rox_net::providers::normalize;

struct Entry {
    title: String,
    /// `title` with feature credits dropped first.
    plain: String,
    /// `title` with bracketed qualifiers dropped first.
    bare: String,
    id: i64,
}

/// Normalized artist to every track filed under it.
pub struct Index(HashMap<String, Vec<Entry>>);

impl Index {
    pub fn build(rows: Vec<(i64, String, String)>) -> Index {
        let mut index: HashMap<String, Vec<Entry>> = HashMap::new();
        for (id, artist, title) in rows {
            let artist = normalize(&artist);
            if artist.is_empty() {
                continue;
            }
            let folded = normalize(&title);
            if folded.is_empty() {
                continue;
            }
            index.entry(artist).or_default().push(Entry {
                plain: plain(&title),
                bare: bare(&title),
                title: folded,
                id,
            });
        }
        Index(index)
    }

    /// Every library track the name could mean, empty when unsure. All of them:
    /// a heart goes on every copy, while a play goes to [`pick_one`].
    ///
    /// A feature credit names who sings, not which take, so titles that
    /// differ only by one are the same song. The bracket-stripped last look
    /// only settles on a single title, so a studio and a live take that
    /// differ by nothing else never match.
    pub fn resolve(&self, artist: &str, title: &str) -> Vec<i64> {
        let Some(entries) = self.0.get(&normalize(artist)) else {
            return Vec::new();
        };

        let want = normalize(title);
        let exact: Vec<&Entry> = entries.iter().filter(|entry| entry.title == want).collect();
        if !exact.is_empty() {
            return ids(&exact);
        }

        let want = plain(title);
        let credited: Vec<&Entry> = entries.iter().filter(|entry| entry.plain == want).collect();
        if !want.is_empty() && !credited.is_empty() {
            return ids(&credited);
        }

        let want = bare(title);
        if want.is_empty() {
            return Vec::new();
        }
        let near: Vec<&Entry> = entries.iter().filter(|entry| entry.bare == want).collect();
        let Some(first) = near.first() else {
            return Vec::new();
        };
        if near.iter().any(|entry| entry.title != first.title) {
            return Vec::new();
        }
        ids(&near)
    }
}

/// In index order, each once: a track filed under both its artist and its
/// album artist can land in one artist's list twice.
fn ids(entries: &[&Entry]) -> Vec<i64> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .map(|entry| entry.id)
        .filter(|id| seen.insert(*id))
        .collect()
}

pub fn bare(title: &str) -> String {
    normalize(&strip_brackets(title))
}

pub fn plain(title: &str) -> String {
    normalize(&strip_credits(title))
}

/// Drops a bracketed group that's only a credit ("(feat. Yizzy)", "[with
/// Allison Ponthier]") and a "feat." tail. A group that says more ("(KOAN
/// Sound remix - feat Geoffroy)") stays, since it names a different take,
/// though a credit inside it is still cut.
pub fn strip_credits(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(open) = rest.find(['(', '[', '{']) {
        out.push_str(&rest[..open]);

        let group = &rest[open..];
        let (inner, end) = match closing(group) {
            Some(close) => (&group[1..close], close + 1),
            None => (&group[1..], group.len()),
        };
        if !is_credit(inner) {
            out.push_str(&group[..end]);
        }
        rest = &group[end..];
    }
    out.push_str(rest);

    // ASCII lowering keeps byte offsets, so the cut lands on the same spot.
    let lower = out.to_ascii_lowercase();
    let tail = [" feat. ", " feat ", " ft. ", " ft ", " featuring "]
        .iter()
        .filter_map(|marker| lower.find(marker))
        .min();
    if let Some(at) = tail {
        out.truncate(at);
    }

    out
}

/// The byte offset of the bracket closing the group `s` opens, nesting
/// counted. None when it never closes.
fn closing(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

/// "feat. X", "ft X", "featuring X" or "with X", and nothing else first.
fn is_credit(inner: &str) -> bool {
    let mut words = inner.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };

    let first = first.trim_end_matches('.').to_lowercase();
    matches!(first.as_str(), "feat" | "ft" | "featuring" | "with") && words.next().is_some()
}

/// An unclosed group takes the rest of the line with it.
pub fn strip_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for ch in s.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Whether two artist and title pairs name one song, by [`Index::resolve`]'s
/// rules.
pub fn same_song(artist: &str, title: &str, other_artist: &str, other_title: &str) -> bool {
    let other = Index::build(vec![(0, other_artist.to_string(), other_title.to_string())]);
    !other.resolve(artist, title).is_empty()
}

/// Among copies of one song, the one with the most plays here wins, since
/// that's the copy being listened to. Ties take the first.
pub fn pick_one(found: &[i64], plays: &HashMap<i64, u32>) -> Option<i64> {
    let (&first, rest) = found.split_first()?;

    let mut best = (first, plays.get(&first).copied().unwrap_or(0));
    for &id in rest {
        let count = plays.get(&id).copied().unwrap_or(0);
        if count > best.1 {
            best = (id, count);
        }
    }
    Some(best.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Index {
        Index::build(vec![
            (1, "Boards of Canada".into(), "Roygbiv".into()),
            (2, "Boards of Canada".into(), "Olson".into()),
            (3, "Boards of Canada".into(), "Roygbiv".into()),
            (
                4,
                "Radiohead".into(),
                "Everything in Its Right Place".into(),
            ),
            (5, "Air".into(), "La Femme d'Argent (Live)".into()),
        ])
    }

    #[test]
    fn a_loved_track_hearts_every_copy_the_library_holds() {
        assert_eq!(
            library().resolve("Boards of Canada", "Roygbiv"),
            [1, 3],
            "one song, both rows"
        );
    }

    #[test]
    fn folding_beats_punctuation_and_case() {
        assert_eq!(
            library().resolve("radiohead", "Everything In Its Right Place"),
            [4]
        );
        assert_eq!(library().resolve("BOARDS OF CANADA", "olson!"), [2]);
    }

    #[test]
    fn a_bracketed_qualifier_gets_a_second_look_from_either_side() {
        assert_eq!(
            library().resolve("Boards of Canada", "Olson (2013 Remaster)"),
            [2]
        );
        assert_eq!(library().resolve("Air", "La Femme d'Argent"), [5]);
    }

    #[test]
    fn two_titles_that_differ_only_by_their_qualifier_are_left_alone() {
        let index = Index::build(vec![
            (1, "Air".into(), "Sexy Boy".into()),
            (2, "Air".into(), "Sexy Boy (Live)".into()),
        ]);
        assert_eq!(index.resolve("Air", "Sexy Boy"), [1]);
        assert!(index.resolve("Air", "Sexy Boy (Remastered)").is_empty());
    }

    #[test]
    fn a_feature_credit_is_the_same_song_and_a_remix_is_not() {
        let index = Index::build(vec![
            (1, "Apashe".into(), "Distance (feat. Geoffroy)".into()),
            (
                2,
                "Apashe".into(),
                "Distance (Volac remix - feat Geoffroy)".into(),
            ),
            (
                3,
                "Apashe".into(),
                "Distance (KOAN Sound remix - feat Geoffroy)".into(),
            ),
            (4, "Lord Huron".into(), "I Lied".into()),
            (5, "Apashe".into(), "Work feat. Vo Williams".into()),
        ]);

        assert_eq!(index.resolve("Apashe", "Distance"), [1]);
        assert_eq!(index.resolve("Apashe", "Distance [ft. Geoffroy]"), [1]);
        assert_eq!(index.resolve("Apashe", "Work"), [5]);
        assert_eq!(
            index.resolve("Lord Huron", "I Lied (with Allison Ponthier)"),
            [4]
        );
        assert_eq!(index.resolve("Apashe", "Distance (Volac Remix)"), [2]);
    }

    #[test]
    fn credits_go_and_everything_else_stays() {
        assert_eq!(strip_credits("Dead (feat. Yizzy)"), "Dead ");
        assert_eq!(strip_credits("I Lied (with August Ponthier)"), "I Lied ");
        assert_eq!(strip_credits("Sexy Boy (Live)"), "Sexy Boy (Live)");
        assert_eq!(strip_credits("Dancing with Myself"), "Dancing with Myself");
        assert_eq!(
            strip_credits("Mercy (With)"),
            "Mercy (With)",
            "a credit names someone"
        );
        assert_eq!(
            strip_credits("Work (feat. Vo"),
            "Work ",
            "an unclosed credit"
        );
    }

    #[test]
    fn a_track_filed_twice_under_one_artist_comes_back_once() {
        let index = Index::build(vec![
            (1, "APASHE".into(), "Work".into()),
            (1, "Apashe".into(), "Work".into()),
        ]);
        assert_eq!(index.resolve("Apashe", "Work"), [1]);
    }

    #[test]
    fn an_unknown_name_matches_nothing() {
        assert!(library().resolve("Aphex Twin", "Xtal").is_empty());
        assert!(
            library()
                .resolve("Boards of Canada", "Dayvan Cowboy")
                .is_empty()
        );
    }

    #[test]
    fn one_song_reads_the_same_through_case_and_qualifiers() {
        assert!(same_song(
            "KATSEYE",
            "HOOTIE FRUTTI",
            "KATSEYE",
            "Hootie Frutti"
        ));
        assert!(same_song("Lisa", "SaWaDiKa", "LISA", "SaWaDiKa"));
        assert!(same_song(
            "Air",
            "La Femme d'Argent",
            "Air",
            "La Femme d'Argent (Live)"
        ));
        assert!(!same_song("Noisia", "Dustup", "Noisia", "Into Dust"));
        assert!(!same_song("Polyphia", "Reverie", "Muse", "Reverie"));
    }

    #[test]
    fn the_copy_already_played_wins_over_its_duplicates() {
        let mut plays = HashMap::new();
        plays.insert(2, 10);
        assert_eq!(pick_one(&[1, 2, 3], &plays), Some(2));
        assert_eq!(pick_one(&[1, 2, 3], &HashMap::new()), Some(1));
        assert_eq!(pick_one(&[], &plays), None);
    }
}
