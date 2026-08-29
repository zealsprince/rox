//! Song identity: what two recordings of the same song share, for the draws
//! that pick tracks by sound rather than by name.
//!
//! A library that holds a song seventeen times (a studio cut, three live
//! takes, a BBC session, and a pile of near-identical rips) hands the
//! acoustic ranking seventeen tracks that all score as each other's nearest
//! neighbour. Left alone, radio walks the pile: every version of "Barracuda"
//! in a row, which is the one thing a listener never wants from a station.
//! The key here is what lets a draw notice it's about to do that.
//!
//! This is deliberately looser than [`crate::duplicates`], which asks whether
//! two files are the same recording and can offer to trash one. Nothing here
//! deletes anything, so it can afford to fold a live take onto the studio cut
//! and be wrong once in a while: the cost of a false match is one track
//! passed over in a radio draw.

use std::collections::{HashMap, HashSet, VecDeque};

use rusqlite::Connection;

/// The words that mark a bracketed tail as a version rather than part of the
/// title. "Barracuda (Live In Japan)" is Barracuda; "Barracuda (Part 2)"
/// isn't necessarily, so "part" isn't in here.
///
/// Matched against whole tokens of the tail, so "livewire" doesn't read as
/// live. Both halves of the pairs that drift ("remaster", "remastered") are
/// listed rather than matched by prefix, which would catch "mixtape" on
/// "mix".
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

/// The separators that mark the end of the primary artist. A featuring
/// credit doesn't make a new song, and neither does the "& Friends" a live
/// record files its band under.
///
/// Bare "and" isn't here: it's part of too many band names to cut on, and
/// the ampersand covers the case that matters.
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

/// The identity `artist` and `title` name, or None when either tag is empty:
/// a row with nothing to be identified by is never the same song as anything.
///
/// Folding is deliberately aggressive on punctuation and spacing, because the
/// same song ripped twice really does come back as "Don't Stop" and "Dont
/// Stop", and a guard that missed that would miss the case it exists for.
pub fn key(artist: &str, title: &str) -> Option<String> {
    let artist = fold_artist(artist);
    let title = fold_title(title);
    (!artist.is_empty() && !title.is_empty()).then(|| format!("{artist}\u{1}{title}"))
}

/// The primary artist, folded. Everything from the first credit separator on
/// is dropped, so "Heart & Friends" and "Heart" are one artist.
fn fold_artist(artist: &str) -> String {
    let lower = artist.to_lowercase();
    let cut = ARTIST_SPLITS
        .iter()
        .filter_map(|sep| lower.find(sep))
        .min()
        .unwrap_or(lower.len());
    squash(&lower[..cut])
}

/// The title with its version tails cut off, folded. "Barracuda (Live)
/// (2010 Remaster)" unwinds to Barracuda, and a trailing " - " tail goes the
/// same way for the stores that write "Barracuda - Live" instead.
fn fold_title(title: &str) -> String {
    let mut work = title.trim().to_lowercase();
    while let Some(rest) = strip_tail(&work) {
        // A title that is nothing but its annotation names no song, so it
        // gets no identity rather than an identity of "live" that every
        // such row in the library would share.
        if rest.is_empty() {
            return String::new();
        }
        work = rest.to_string();
    }
    squash(&work)
}

/// The title without its trailing annotation, or None when what's there is
/// part of the name.
///
/// The bracketed groups come off together rather than one at a time, and
/// one of them naming a version takes the lot: a real library writes
/// "Barracuda (Live from BBC Radio Concert) (Previously unreleased)", where
/// peeling one group at a time stops on the note that isn't a version and
/// never reaches the one that is.
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
    // The dash form, and only with the spaces around it: a title like
    // "Re-Recorded" has no tail, it just has a hyphen in it.
    let at = title.rfind(" - ")?;
    is_version(&title[at + 3..]).then(|| title[..at].trim_end())
}

/// The bracketed group at the end of `title` as (what came before it, what's
/// inside it), or None when the title doesn't end in one.
fn trailing_group(title: &str) -> Option<(&str, &str)> {
    for (open, close) in [('(', ')'), ('[', ']')] {
        if let Some(rest) = title.strip_suffix(close) {
            if let Some(at) = rest.rfind(open) {
                return Some((&rest[..at], &rest[at + open.len_utf8()..]));
            }
        }
    }
    None
}

/// Whether a tail names a version of a song rather than more of its title.
fn is_version(tail: &str) -> bool {
    tail.split(|c: char| !c.is_alphanumeric())
        .any(|word| VERSION_WORDS.contains(&word))
}

/// Letters and digits, everything else dropped. Spacing, apostrophes, and
/// the hyphens that differ between two rips of one disc all come out.
fn squash(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The identities `ids` resolve to, skipping rows the library no longer holds
/// and rows with nothing to identify them by.
///
/// One indexed lookup per id rather than a scan, the same shape
/// [`crate::store::names_for`] uses: the callers ask about a batch of
/// candidates, not the library.
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

/// The identities a set of already-played ids covers: what a draw shouldn't
/// come back with while it still has anything else to offer.
pub fn keys_of(keys: &HashMap<i64, String>, ids: impl IntoIterator<Item = i64>) -> HashSet<String> {
    ids.into_iter()
        .filter_map(|id| keys.get(&id).cloned())
        .collect()
}

/// Walk `candidates` in rank order and keep the ones that bring a song the
/// draw hasn't got yet: nothing in `blocked`, and one per identity. Rows with
/// no identity are always kept, since an untagged track is nothing's
/// duplicate.
///
/// Returns an empty vec when everything is blocked, which is the caller's
/// signal to fall back rather than a signal to stop: a neighbourhood that
/// holds one song and nothing else should still play.
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

/// Reorder `items` so no two entries sharing a song identity land within
/// `gap` of each other, keeping the order they came in otherwise.
///
/// For the callers that must not drop anything: a queue the listener can see
/// is theirs, so the version pile gets spread through it rather than cut out
/// of it. `playing` seeds the window with the identity already sounding, so
/// the reorder never puts another take of it next.
///
/// Greedy and stable: each step takes the first entry the window allows, and
/// falls back to the first entry outright when the window allows none, so a
/// tail that's all one song comes out in its original order rather than not
/// at all.
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

    /// The case this module exists for: every Heart "Barracuda" in the
    /// library is one song, whatever the rip called it.
    #[test]
    fn every_version_of_one_song_folds_together() {
        let studio = k("Heart", "Barracuda");
        for (artist, title) in [
            ("Heart", "Barracuda (Live)"),
            ("Heart", "Barracuda (Live In Japan)"),
            ("Heart", "Barracuda (Live from BBC Radio Concert)"),
            // A note that isn't a version, behind one that is: real tags do
            // this, and peeling one group at a time would stop on the first.
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

    /// A different band's song of the same name is a different song, which is
    /// the whole reason the artist is in the key.
    #[test]
    fn the_same_title_by_someone_else_is_a_different_song() {
        assert_ne!(k("Heart", "Barracuda"), k("Noisestorm", "Barracuda"));
        assert_ne!(k("Heart", "Barracuda"), k("Vantage", "Barracuda"));
    }

    /// Punctuation and spacing drift between rips of one disc, so none of it
    /// is part of the identity.
    #[test]
    fn punctuation_drift_folds_away() {
        assert_eq!(k("Heart", "Don't Stop"), k("Heart", "Dont  Stop"));
        assert_eq!(k("Heart", "Barra-cuda"), k("Heart", "Barra cuda"));
    }

    /// A bracketed tail that isn't a version stays part of the title, or
    /// every numbered movement in a library would read as one song.
    #[test]
    fn a_tail_that_names_no_version_stays() {
        assert_ne!(k("A", "Intro (Part 1)"), k("A", "Intro (Part 2)"));
        assert_ne!(k("A", "Intro (Part 1)"), k("A", "Intro"));
        // The hyphen inside a word is not the dash form's separator.
        assert_ne!(k("A", "Re-Recorded"), k("A", "Re"));
    }

    /// Nothing to identify a row by is no identity at all, rather than an
    /// empty one every untagged file would share.
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

    /// The selection guard: one track per song, and nothing the session has
    /// already heard a version of.
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

    /// A candidate with no identity is nothing's duplicate, so it always
    /// gets through.
    #[test]
    fn distinct_never_drops_what_it_cannot_identify() {
        let keys = keymap(&[(1, "Barracuda"), (2, "Barracuda (Live)")]);
        assert_eq!(
            distinct(&[1, 2, 7, 8], &keys, &HashSet::new(), 9),
            vec![1, 7, 8]
        );
    }

    /// Everything blocked comes back empty rather than falling back here:
    /// the caller decides what a heard-out neighbourhood does.
    #[test]
    fn distinct_reports_a_neighbourhood_it_used_up() {
        let keys = keymap(&[(1, "Barracuda"), (2, "Barracuda (Live)")]);
        let blocked = keys_of(&keys, [1]);
        assert!(distinct(&[1, 2], &keys, &blocked, 9).is_empty());
    }

    /// The ordering guard drops nothing and spreads the pile instead.
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

    /// The track already sounding counts as the first thing in the window,
    /// which is the jump the listener actually complained about.
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

    /// A tail with nothing else in it plays in the order it had: spacing
    /// never costs a track its place in the queue.
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
