//! The loved-tracks import: Last.fm's loved list pulled back into the
//! library as hearts, the other direction from the mirror in
//! [`crate::lastfm`].
//!
//! It runs as a task rather than a button that blocks: fetching walks
//! pages of an account's whole loved history, and an account with a
//! decade behind it has thousands. The task is dynamic, unlike the scan
//! and the two analysis passes, so it only appears in the tasks window
//! while it's running or has just finished. It's started from Settings,
//! it takes seconds rather than an afternoon, and a permanent row for it
//! would be a row that spends its life saying nothing.
//!
//! The import only adds. A heart it can't find a home for is counted and
//! left alone, and nothing here ever takes a heart back: what's on the
//! shelf locally is the user's, and a name this can't resolve is far more
//! likely a tagging difference than a track anyone unloved. That also
//! keeps the run idempotent, so a second import is free and a first one
//! can't destroy anything.
//!
//! Matching is exact or it doesn't happen, per the rule
//! [`rox_library::playlists::reattach`] already draws for playlist
//! members: names fold to comparable words, a bracketed qualifier gets a
//! second look, and anything that could name two different local titles
//! is left for a human rather than guessed at.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{App, Entity, Global, SharedString};

use rox_library::store;

use rox_core::settings::Settings;
use rox_net::providers::{agent, net_reason, normalize};
use rox_services::catalog::Library;
use rox_services::lastfm::Scrobbler;

const API: &str = "https://ws.audioscrobbler.com/2.0/";

/// Loved tracks asked for per request. The API caps this on its own side;
/// asking for more than it allows just gets a shorter page, and the walk
/// reads the page count it answers with rather than assuming this one.
const PAGE: usize = 200;

/// A breath between pages. Last.fm asks callers to stay under a handful of
/// requests a second, and a loved list is a few pages: this costs the
/// import nothing worth measuring and keeps it a good guest.
const PAGE_PAUSE: Duration = Duration::from_millis(250);

/// Where the walk gives up whatever the API keeps answering. At [`PAGE`] a
/// page this is a loved list longer than anyone has, so reaching it means
/// the pagination is lying rather than that the account is enormous.
const MAX_PAGES: usize = 500;

/// Live progress of an import, written by the worker and polled by the UI.
/// Total is what the API says the account has loved, zero until the first
/// page answers.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Loved tracks the library had no unambiguous home for, so the readout
    /// can own up to what it left behind.
    unmatched: AtomicUsize,
    /// What the import is on: a track name while fetching, a phase while
    /// it's doing something the names don't describe.
    current: Mutex<String>,
    cancel: AtomicBool,
    pace: rox_core::pace::Pace,
}

impl Progress {
    /// Loved tracks fetched so far.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Loved tracks the account holds. Zero until the first page lands.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Loved tracks with no home in this library.
    pub fn unmatched(&self) -> usize {
        self.unmatched.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }

    fn say(&self, what: impl Into<String>) {
        *self.current.lock().unwrap() = what.into();
    }
}

/// What an import left behind, for the rows and readouts that report on it
/// after the fact.
#[derive(Clone, Copy, Default)]
pub struct Summary {
    /// Loved tracks the account holds and this run read.
    pub fetched: usize,
    /// Of those, how many named at least one track in this library.
    pub matched: usize,
    /// Tracks whose heart this run actually turned on. Lower than matched
    /// on a second run, where most of them were already favourites.
    pub added: usize,
    /// Loved tracks with no unambiguous home here.
    pub unmatched: usize,
    /// Whether it was stopped rather than reaching the end.
    pub stopped: bool,
}

impl Summary {
    /// The one-line report, the same sentence wherever it's shown.
    pub fn line(&self) -> String {
        let head = if self.stopped {
            format!("Stopped after {} loved tracks", self.fetched)
        } else {
            format!("Read {} loved tracks", self.fetched)
        };
        format!(
            "{head}, matched {}, added {} to favourites",
            self.matched, self.added
        )
    }
}

/// The running import, or nothing. App-global like the other passes: it
/// outlives the settings window that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The last run's result, kept for the readouts until the app closes.
#[derive(Default)]
struct Last(Option<Result<Summary, SharedString>>);

impl Global for Last {}

/// The running import's progress, or None while nothing is importing.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// How the last import went: its summary, or why it failed. None until one
/// has run in this session, and None again once it's been dismissed.
pub fn last(cx: &App) -> Option<Result<Summary, SharedString>> {
    cx.try_global::<Last>().and_then(|l| l.0.clone())
}

/// Drop the last run's report, the X on its row. What it did stands; this
/// is only the reading of it being finished with.
pub fn dismiss(cx: &mut App) {
    cx.set_global(Last(None));
}

/// Ask the running import to stop at the next page. What it already
/// matched still lands: the hearts it found are no less right for the rest
/// going unread.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Why an import can't run right now, or None when it can. The account has
/// to be connected: the loved list is read per user, and there's no user
/// without one.
pub fn blocked_reason(cx: &App) -> Option<&'static str> {
    if progress(cx).is_some() {
        return Some("An import is already running");
    }
    if api_key().is_empty() {
        return Some("This build has no api key to ask with");
    }
    if username().is_empty() {
        return Some("Connect a Last.fm account first");
    }
    None
}

/// Whose loved tracks to read. Sessions are filed by the api key that
/// minted them, so this is the account connected under the identity the
/// read calls sign with, not whatever connected last.
fn username() -> String {
    Settings::load()
        .accounts
        .lastfm
        .username(&api_key())
        .to_string()
}

/// The key the read calls with, the scrobbler's fallback order: the
/// settings override where the user entered one, the build's identity
/// otherwise.
fn api_key() -> String {
    let key = Settings::load().accounts.lastfm.api_key;
    if key.is_empty() {
        rox_net::lastfm::keys::API_KEY.to_string()
    } else {
        key
    }
}

/// Pull the account's loved tracks and heart every one this library can
/// name. A no-op while an import is already running, or while nothing is
/// connected to read from.
pub fn start(library: Entity<Library>, scrobbler: Entity<Scrobbler>, cx: &mut App) {
    if blocked_reason(cx).is_some() {
        return;
    }
    let user = username();
    let key = api_key();
    let db_path = library.read(cx).db_path();
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    // A fresh run's report replaces the last one rather than sitting under
    // it, so the row never shows an old count beside a live bar.
    cx.set_global(Last(None));
    // Nothing observes an app-global pass on its own; this is what keeps
    // the tasks window and the menubar chip ticking while it runs.
    crate::tasks_window::repaint_while_running(cx);
    // The import outlives whichever window started it, so hand over
    // something that does too, the same as starting a pass does: the tasks
    // window carries the count and the stop button.
    crate::tasks_window::open(cx);
    cx.spawn(async move |cx| {
        let found = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&user, &key, &db_path, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            let outcome = match found {
                Ok(found) => Ok(apply(found, &progress, &library, &scrobbler, cx)),
                Err(e) => {
                    log::warn!("lastfm: importing loved tracks: {e}");
                    Err(SharedString::from(e))
                }
            };
            cx.set_global(Last(Some(outcome)));
        })
        .ok();
    })
    .detach();
}

/// What the fetch and match came back with: the tracks to heart, and how
/// much of the loved list they came out of.
struct Found {
    ids: Vec<i64>,
    fetched: usize,
    matched: usize,
    unmatched: usize,
}

/// Turn the matched tracks into hearts, on the UI side where the library
/// entity lives. Returns what to report.
fn apply(
    found: Found,
    progress: &Progress,
    library: &Entity<Library>,
    scrobbler: &Entity<Scrobbler>,
    cx: &mut App,
) -> Summary {
    // Counted before the write, since afterwards they all read as
    // favourites and there's no telling which ones this run turned on.
    let already = library.read(cx).favourite_ids();
    let added = found.ids.iter().filter(|id| !already.contains(id)).count();
    library.update(cx, |library, cx| {
        library.set_favourites(&found.ids, true, cx);
    });
    // These hearts came FROM Last.fm, so the mirror absorbs them instead of
    // pushing them straight back as thousands of loves it already knows.
    // Same update pass as the write, so it lands before the library's event
    // reaches the mirror's diff.
    scrobbler.update(cx, |scrobbler, cx| {
        scrobbler.absorb_favourites(cx);
    });
    Summary {
        fetched: found.fetched,
        matched: found.matched,
        added,
        unmatched: found.unmatched,
        stopped: progress.stopping(),
    }
}

/// The whole blocking half: walk the loved list, fold the library, and
/// match one against the other. Background executor only.
fn run(
    user: &str,
    key: &str,
    db_path: &std::path::Path,
    progress: &Progress,
) -> Result<Found, String> {
    let mut loved: Vec<Loved> = Vec::new();
    let mut page = 1;
    progress.pace.begin();
    loop {
        let (entries, pages) = fetch_page(key, user, page)?;
        if let Some(last) = entries.last() {
            progress.say(format!("{} - {}", last.artist, last.title));
        }
        loved.extend(entries);
        progress.done.store(loved.len(), Ordering::Relaxed);
        if page == 1 {
            // The total only means anything once the first page has
            // answered with it; before that the row shows a bar with
            // nothing behind it, which is the honest picture.
            progress.total.store(pages.total, Ordering::Relaxed);
        }
        if page >= pages.count.min(MAX_PAGES) || !progress.keep_going() {
            break;
        }
        page += 1;
        std::thread::sleep(PAGE_PAUSE);
    }

    progress.say("Matching against the library");
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let index = Index::build(store::name_index(&conn).map_err(|e| e.to_string())?);
    let mut ids: Vec<i64> = Vec::new();
    let mut matched = 0usize;
    let mut unmatched = 0usize;
    for track in &loved {
        let found = index.resolve(&track.artist, &track.title);
        if found.is_empty() {
            unmatched += 1;
            // The names that missed are the whole story when a run matches
            // less than someone expected, and there can be hundreds, so
            // they go to the log rather than to a window nobody asked for.
            log::debug!("lastfm: no match for {} - {}", track.artist, track.title);
            continue;
        }
        matched += 1;
        ids.extend(found);
    }
    progress.unmatched.store(unmatched, Ordering::Relaxed);
    ids.sort_unstable();
    ids.dedup();
    Ok(Found {
        ids,
        fetched: loved.len(),
        matched,
        unmatched,
    })
}

/// One loved track as Last.fm names it. No album: the loved list doesn't
/// carry one, which is why matching leans on the artist and the title
/// alone.
struct Loved {
    artist: String,
    title: String,
}

/// What a page said about the shape of the list behind it.
struct Pages {
    count: usize,
    total: usize,
}

/// One page of the loved list. Unsigned, like the artist lookup: this is a
/// public read of a named account, so it needs an api key and no session.
fn fetch_page(key: &str, user: &str, page: usize) -> Result<(Vec<Loved>, Pages), String> {
    let request = agent()
        .get(API)
        .query("method", "user.getlovedtracks")
        .query("user", user)
        .query("api_key", key)
        .query("limit", &PAGE.to_string())
        .query("page", &page.to_string())
        .query("format", "json");
    // An API error still carries a JSON body worth reading, so a status
    // failure parses like a success, the scrobbler's move.
    let text = match request.call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(net_reason(&e)),
    };
    parse_page(&text)
}

/// A page's JSON to tracks and the list's shape. Split from the request so
/// the parsing is testable without a network.
fn parse_page(text: &str) -> Result<(Vec<Loved>, Pages), String> {
    let body: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if body.get("error").is_some() {
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error");
        return Err(message.to_string());
    }
    let Some(loved) = body.get("lovedtracks") else {
        return Err("no loved tracks in the response".into());
    };
    let attr = loved.get("@attr");
    let number = |field: &str| -> usize {
        attr.and_then(|a| a.get(field))
            .map(|v| match v {
                // The counts come back as strings, but a service that
                // starts sending numbers shouldn't reset someone's import
                // to zero.
                serde_json::Value::String(s) => s.parse().unwrap_or(0),
                other => other.as_u64().unwrap_or(0) as usize,
            })
            .unwrap_or(0)
    };
    // A single loved track comes back as one object where a list would be
    // an array, the shape every Last.fm collection takes at length one.
    let entries: Vec<&serde_json::Value> = match loved.get("track") {
        Some(serde_json::Value::Array(rows)) => rows.iter().collect(),
        Some(one @ serde_json::Value::Object(_)) => vec![one],
        _ => Vec::new(),
    };
    let tracks = entries
        .into_iter()
        .filter_map(|row| {
            let title = row
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let artist = row
                .get("artist")
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            // Neither half is optional: a track missing one can't be
            // matched against anything, so it never enters the count.
            (!title.is_empty() && !artist.is_empty()).then(|| Loved {
                artist: artist.to_string(),
                title: title.to_string(),
            })
        })
        .collect();
    Ok((
        tracks,
        Pages {
            count: number("totalPages").max(1),
            total: number("total"),
        },
    ))
}

/// One library track as the matcher sees it: its title folded two ways, and
/// the row it belongs to.
struct Entry {
    /// The title folded to comparable words.
    title: String,
    /// The same with bracketed qualifiers dropped first, so "(Remastered
    /// 2013)" stops being the reason a match misses.
    bare: String,
    id: i64,
}

/// The library folded to what a loved track can be looked up by: normalized
/// artist to every track filed under it.
struct Index(HashMap<String, Vec<Entry>>);

impl Index {
    fn build(rows: Vec<(i64, String, String)>) -> Index {
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
                bare: bare(&title),
                title: folded,
                id,
            });
        }
        Index(index)
    }

    /// Every library track a loved entry names, empty when there's nothing
    /// this can be sure of.
    ///
    /// All of them, deliberately: the same recording sitting on an album, a
    /// compilation, and a single is three rows and one song, and a heart
    /// belongs on the song. The second look drops bracketed qualifiers from
    /// both sides, but only settles when what's left names a single title -
    /// a studio take and a live one that differ by nothing else are exactly
    /// the guess this shouldn't make.
    fn resolve(&self, artist: &str, title: &str) -> Vec<i64> {
        let Some(entries) = self.0.get(&normalize(artist)) else {
            return Vec::new();
        };
        let want = normalize(title);
        let exact: Vec<i64> = entries
            .iter()
            .filter(|entry| entry.title == want)
            .map(|entry| entry.id)
            .collect();
        if !exact.is_empty() {
            return exact;
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
        near.iter().map(|entry| entry.id).collect()
    }
}

/// A title with its bracketed tails dropped, then folded: what "Roygbiv"
/// and "Roygbiv (Remastered 2013)" have in common, which is the difference
/// a local tag and Last.fm most often disagree by.
fn bare(title: &str) -> String {
    normalize(&strip_brackets(title))
}

/// Drop every parenthesized or bracketed group, nesting and all. An
/// unclosed group takes the rest of the line with it: a title that opens a
/// bracket and never shuts it has nothing trustworthy after it.
fn strip_brackets(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Index {
        Index::build(vec![
            (1, "Boards of Canada".into(), "Roygbiv".into()),
            (2, "Boards of Canada".into(), "Olson".into()),
            // The same song again off a compilation: one recording, two rows.
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
        // Last.fm carries the remaster, the library doesn't.
        assert_eq!(
            library().resolve("Boards of Canada", "Olson (2013 Remaster)"),
            [2]
        );
        // And the other way around: the library's copy is the live one.
        assert_eq!(library().resolve("Air", "La Femme d'Argent"), [5]);
    }

    #[test]
    fn two_titles_that_differ_only_by_their_qualifier_are_left_alone() {
        let index = Index::build(vec![
            (1, "Air".into(), "Sexy Boy".into()),
            (2, "Air".into(), "Sexy Boy (Live)".into()),
        ]);
        // The exact title still resolves: it names one of them outright.
        assert_eq!(index.resolve("Air", "Sexy Boy"), [1]);
        // This one could mean either, so it means neither.
        assert!(index.resolve("Air", "Sexy Boy (Remastered)").is_empty());
    }

    #[test]
    fn an_unknown_name_matches_nothing() {
        assert!(library().resolve("Aphex Twin", "Xtal").is_empty());
        assert!(library()
            .resolve("Boards of Canada", "Dayvan Cowboy")
            .is_empty());
    }

    #[test]
    fn a_page_parses_its_tracks_and_its_shape() {
        let body = r#"{"lovedtracks":{"track":[
            {"name":"Roygbiv","artist":{"name":"Boards of Canada"}},
            {"name":"Olson","artist":{"name":"Boards of Canada"}},
            {"name":"","artist":{"name":"Nameless"}}
        ],"@attr":{"user":"someone","totalPages":"3","total":"512"}}}"#;
        let (tracks, pages) = parse_page(body).unwrap();
        assert_eq!(tracks.len(), 2, "the untitled row never enters the count");
        assert_eq!(tracks[0].artist, "Boards of Canada");
        assert_eq!(tracks[0].title, "Roygbiv");
        assert_eq!(pages.count, 3);
        assert_eq!(pages.total, 512);
    }

    #[test]
    fn one_loved_track_arrives_as_an_object_not_a_list() {
        let body = r#"{"lovedtracks":{"track":
            {"name":"Olson","artist":{"name":"Boards of Canada"}},
            "@attr":{"total":"1"}}}"#;
        let (tracks, pages) = parse_page(body).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(pages.count, 1, "no page count still means one page");
        assert_eq!(pages.total, 1);
    }

    #[test]
    fn an_empty_loved_list_is_a_clean_zero() {
        let body = r#"{"lovedtracks":{"@attr":{"total":"0"}}}"#;
        let (tracks, pages) = parse_page(body).unwrap();
        assert!(tracks.is_empty());
        assert_eq!(pages.total, 0);
    }

    #[test]
    fn an_api_error_carries_its_message_out() {
        let body = r#"{"error":6,"message":"User not found"}"#;
        assert_eq!(parse_page(body).err(), Some("User not found".to_string()));
    }
}
