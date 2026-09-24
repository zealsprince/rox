//! Song identity: what two recordings of the same song share, so a radio
//! draw doesn't walk every version of "Barracuda" in a row.
//!
//! Deliberately looser than [`crate::duplicates`]: nothing here deletes, so
//! folding a live take onto the studio cut costs at most one skipped track.

use std::collections::{HashMap, HashSet, VecDeque};

use rusqlite::Connection;

/// Words that mark a bracketed tail as a version, matched as whole tokens.
/// "part" is out: "(Part 2)" may be a different piece.
const VERSION_WORDS: &[&str] = &[
    "acoustic",
    "alternate",
    "anniversary",
    "bonus",
    "clean",
    "cover",
    "deluxe",
    "demo",
    "demos",
    "edit",
    "edits",
    "explicit",
    "extended",
    "feat",
    "featuring",
    "ft",
    "instrumental",
    "karaoke",
    "live",
    "mix",
    "mixes",
    "mono",
    "radio",
    "recorded",
    "reissue",
    "remaster",
    "remastered",
    "remix",
    "remixed",
    "session",
    "sessions",
    "single",
    "stereo",
    "take",
    "unplugged",
    "version",
    "versions",
];

/// Where the primary artist ends. Bare "and" is out: too many band names.
const ARTIST_SPLITS: &[&str] = &[
    " feat.",
    " feat ",
    " featuring ",
    " ft.",
    " ft ",
    " with ",
    " vs.",
    " vs ",
    " & ",
    ",",
];

/// None when either tag is empty. Folding is aggressive on punctuation
/// because rips really do disagree ("Don't Stop" vs "Dont Stop").
pub fn key(artist: &str, title: &str) -> Option<String> {
    let artist = fold_artist(artist);
    let title = fold_title(title);
    (!artist.is_empty() && !title.is_empty()).then(|| format!("{artist}\u{1}{title}"))
}

fn fold_artist(artist: &str) -> String {
    let lower = artist.to_lowercase();
    let cut = ARTIST_SPLITS
        .iter()
        .filter_map(|sep| lower.find(sep))
        .min()
        .unwrap_or(lower.len());
    squash(&lower[..cut])
}

fn fold_title(title: &str) -> String {
    let mut work = title.trim().to_lowercase();
    while let Some(rest) = strip_tail(&work) {
        // An annotation-only title gets no identity, not a shared one.
        if rest.is_empty() {
            return String::new();
        }
        work = rest.to_string();
    }
    squash(&work)
}

/// All trailing groups come off together if any names a version: "(Live
/// from BBC) (Previously unreleased)" hides the version behind the note.
fn strip_tail(title: &str) -> Option<&str> {
    let mut head = title.trim_end();
    let mut version = false;
    let mut stripped = false;
    while let Some((rest, inner)) = trailing_group(head) {
        version |= is_version(inner);
        head = rest.trim_end();
        stripped = true;
    }
    if stripped && version {
        return Some(head);
    }
    // Only with the spaces: "Re-Recorded" has no tail.
    let at = title.rfind(" - ")?;
    is_version(&title[at + 3..]).then(|| title[..at].trim_end())
}

fn trailing_group(title: &str) -> Option<(&str, &str)> {
    for (open, close) in [('(', ')'), ('[', ']')] {
        if let Some(rest) = title.strip_suffix(close)
            && let Some(at) = rest.rfind(open)
        {
            return Some((&rest[..at], &rest[at + open.len_utf8()..]));
        }
    }
    None
}

fn is_version(tail: &str) -> bool {
    tail.split(|c: char| !c.is_alphanumeric())
        .any(|word| VERSION_WORDS.contains(&word))
}

fn squash(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect()
}

pub fn keys_for(conn: &Connection, ids: &[i64]) -> rusqlite::Result<HashMap<i64, String>> {
    let mut stmt = conn.prepare_cached("SELECT artist, title FROM tracks WHERE id = ?1")?;
    let mut out = HashMap::with_capacity(ids.len());
    for &id in ids {
        let Ok((artist, title)) = stmt.query_row([id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) else {
            continue;
        };
        if let Some(key) = key(&artist, &title) {
            out.insert(id, key);
        }
    }
    Ok(out)
}

pub fn keys_of(keys: &HashMap<i64, String>, ids: impl IntoIterator<Item = i64>) -> HashSet<String> {
    ids.into_iter()
        .filter_map(|id| keys.get(&id).cloned())
        .collect()
}

/// Keep candidates that bring a new song: not in `blocked`, one per
/// identity, unidentified rows always kept. Empty means everything was
/// blocked, and the caller falls back.
pub fn distinct(
    candidates: &[i64],
    keys: &HashMap<i64, String>,
    blocked: &HashSet<String>,
    want: usize,
) -> Vec<i64> {
    let mut taken: HashSet<&str> = HashSet::new();
    let mut out = Vec::with_capacity(want.min(candidates.len()));
    for &id in candidates {
        match keys.get(&id) {
            Some(key) if blocked.contains(key) => continue,
            Some(key) if !taken.insert(key.as_str()) => continue,
            _ => out.push(id),
        }
        if out.len() >= want {
            break;
        }
    }
    out
}

/// Reorder so no two entries sharing an identity land within `gap`, without
/// dropping any. `playing` seeds the window. Greedy and stable: an all-one-song
/// tail keeps its order.
pub fn space<'a, T>(
    items: &mut Vec<T>,
    gap: usize,
    playing: Option<&str>,
    key: impl Fn(&T) -> Option<&'a str>,
) {
    if gap == 0 || items.len() < 2 {
        return;
    }
    let mut window: VecDeque<String> = playing.map(|k| k.to_string()).into_iter().collect();
    let mut src: Vec<T> = std::mem::take(items);
    let mut out = Vec::with_capacity(src.len());
    while !src.is_empty() {
        let at = src
            .iter()
            .position(|item| match key(item) {
                Some(k) => !window.iter().any(|held| held == k),
                None => true,
            })
            .unwrap_or(0);
        let item = src.remove(at);
        if let Some(k) = key(&item) {
            window.push_back(k.to_string());
            while window.len() > gap {
                window.pop_front();
            }
        }
        out.push(item);
    }
    *items = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(artist: &str, title: &str) -> String {
        key(artist, title).expect("a tagged row has an identity")
    }

    #[test]
    fn every_version_of_one_song_folds_together() {
        let studio = k("Heart", "Barracuda");
        for (artist, title) in [
            ("Heart", "Barracuda (Live)"),
            ("Heart", "Barracuda (Live In Japan)"),
            ("Heart", "Barracuda (Live from BBC Radio Concert)"),
            (
                "Heart",
                "Barracuda (Live from BBC Radio Concert) (Previously unreleased)",
            ),
            ("Heart", "Barracuda - Live"),
            ("Heart", "Barracuda [2010 Remaster]"),
            ("Heart", "Barracuda (Live) (Remastered)"),
            ("HEART", "barracuda"),
            ("Heart & Friends", "Barracuda"),
            ("Heart feat. Ann Wilson", "Barracuda"),
        ] {
            assert_eq!(k(artist, title), studio, "{artist} - {title}");
        }
    }

    #[test]
    fn the_same_title_by_someone_else_is_a_different_song() {
        assert_ne!(k("Heart", "Barracuda"), k("Noisestorm", "Barracuda"));
        assert_ne!(k("Heart", "Barracuda"), k("Vantage", "Barracuda"));
    }

    #[test]
    fn punctuation_drift_folds_away() {
        assert_eq!(k("Heart", "Don't Stop"), k("Heart", "Dont  Stop"));
        assert_eq!(k("Heart", "Barra-cuda"), k("Heart", "Barra cuda"));
    }

    #[test]
    fn a_tail_that_names_no_version_stays() {
        assert_ne!(k("A", "Intro (Part 1)"), k("A", "Intro (Part 2)"));
        assert_ne!(k("A", "Intro (Part 1)"), k("A", "Intro"));
        assert_ne!(k("A", "Re-Recorded"), k("A", "Re"));
    }

    #[test]
    fn an_untagged_row_has_no_identity() {
        assert!(key("", "Barracuda").is_none());
        assert!(key("Heart", "").is_none());
        assert!(key("Heart", "(Live)").is_none());
    }

    fn keymap(pairs: &[(i64, &str)]) -> HashMap<i64, String> {
        pairs
            .iter()
            .map(|&(id, title)| (id, k("Heart", title)))
            .collect()
    }

    #[test]
    fn distinct_keeps_one_of_each_song() {
        let keys = keymap(&[
            (1, "Barracuda"),
            (2, "Barracuda (Live)"),
            (3, "Crazy On You"),
            (4, "Barracuda [Remaster]"),
            (5, "Magic Man"),
        ]);
        let blocked = keys_of(&keys, [3]);
        assert_eq!(distinct(&[1, 2, 3, 4, 5], &keys, &blocked, 9), vec![1, 5]);
    }

    #[test]
    fn distinct_never_drops_what_it_cannot_identify() {
        let keys = keymap(&[(1, "Barracuda"), (2, "Barracuda (Live)")]);
        assert_eq!(
            distinct(&[1, 2, 7, 8], &keys, &HashSet::new(), 9),
            vec![1, 7, 8]
        );
    }

    #[test]
    fn distinct_reports_a_neighbourhood_it_used_up() {
        let keys = keymap(&[(1, "Barracuda"), (2, "Barracuda (Live)")]);
        let blocked = keys_of(&keys, [1]);
        assert!(distinct(&[1, 2], &keys, &blocked, 9).is_empty());
    }

    #[test]
    fn space_spreads_a_pile_without_losing_it() {
        let keys = keymap(&[
            (1, "Barracuda"),
            (2, "Barracuda (Live)"),
            (3, "Barracuda [Remaster]"),
            (4, "Crazy On You"),
            (5, "Magic Man"),
        ]);
        let mut ids = vec![1, 2, 3, 4, 5];
        space(&mut ids, 2, None, |id| keys.get(id).map(String::as_str));
        assert_eq!(ids, vec![1, 4, 5, 2, 3], "the queue kept every entry");
    }

    #[test]
    fn space_never_follows_the_playing_track_with_another_take_of_it() {
        let keys = keymap(&[(1, "Barracuda (Live)"), (2, "Crazy On You")]);
        let playing = k("Heart", "Barracuda");
        let mut ids = vec![1, 2];
        space(&mut ids, 4, Some(&playing), |id| {
            keys.get(id).map(String::as_str)
        });
        assert_eq!(ids, vec![2, 1]);
    }

    #[test]
    fn space_leaves_a_tail_of_one_song_alone() {
        let keys = keymap(&[
            (1, "Barracuda"),
            (2, "Barracuda (Live)"),
            (3, "Barracuda - Live"),
        ]);
        let mut ids = vec![1, 2, 3];
        space(&mut ids, 8, None, |id| keys.get(id).map(String::as_str));
        assert_eq!(ids, vec![1, 2, 3]);
    }
}
