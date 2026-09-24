//! The loved-tracks import: Last.fm's loved list pulled into the library as
//! hearts. A dynamic task, shown in the tasks window only while running or
//! just finished.
//!
//! It only adds. An unresolved name is counted and left alone, and nothing
//! ever takes a heart back, which also makes a rerun free. Matching follows
//! [`rox_library::playlists::reattach`]: exact after folding, a bracketed
//! qualifier gets a second look, and anything ambiguous is left for a human.

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

/// The API caps this itself; paging reads the page count the response gives.
const PAGE: usize = 200;

/// Last.fm asks callers to stay under a handful of requests a second.
const PAGE_PAUSE: Duration = Duration::from_millis(250);

/// Reaching this means the pagination is lying, not that the list is that long.
const MAX_PAGES: usize = 500;

#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    unmatched: AtomicUsize,
    /// A track name while fetching, a phase name otherwise.
    current: Mutex<String>,
    cancel: AtomicBool,
    pace: rox_core::pace::Pace,
}

impl Progress {
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Zero until the first page returns.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

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

#[derive(Clone, Copy, Default)]
pub struct Summary {
    pub fetched: usize,
    /// Loved tracks that named at least one library track.
    pub matched: usize,
    /// Hearts this run turned on; lower than `matched` on a rerun.
    pub added: usize,
    pub unmatched: usize,
    pub stopped: bool,
}

impl Summary {
    pub fn line(&self) -> String {
        let head = if self.stopped {
            rox_i18n::t!("lastfm-import-stopped", count = self.fetched as u64).to_string()
        } else {
            rox_i18n::t!("lastfm-import-read", count = self.fetched as u64).to_string()
        };
        let mut line = head;
        line.push_str(&rox_i18n::t!(
            "lastfm-import-matched",
            count = self.matched as u64
        ));
        line.push_str(&rox_i18n::t!(
            "lastfm-import-added",
            count = self.added as u64
        ));
        line
    }
}

/// App-global so it outlives the settings window that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

#[derive(Default)]
struct Last(Option<Result<Summary, SharedString>>);

impl Global for Last {}

pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

pub fn last(cx: &App) -> Option<Result<Summary, SharedString>> {
    cx.try_global::<Last>().and_then(|l| l.0.clone())
}

pub fn dismiss(cx: &mut App) {
    cx.set_global(Last(None));
}

/// Stops at the next page. What already matched is still applied.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn blocked_reason(cx: &App) -> Option<&'static str> {
    if progress(cx).is_some() || super::plays_import::progress(cx).is_some() {
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

/// Sessions are filed by the api key that minted them, so this is the account
/// under the key the reads sign with, not whatever connected last.
pub(crate) fn username() -> String {
    Settings::load()
        .accounts
        .lastfm
        .username(&api_key())
        .to_string()
}

/// The scrobbler's fallback order: the settings override, then the build's key.
pub(crate) fn api_key() -> String {
    let key = Settings::load().accounts.lastfm.api_key;
    if key.is_empty() {
        rox_net::lastfm::keys::API_KEY.to_string()
    } else {
        key
    }
}

pub fn start(library: Entity<Library>, scrobbler: Entity<Scrobbler>, cx: &mut App) {
    if blocked_reason(cx).is_some() {
        return;
    }
    let user = username();
    let key = api_key();
    let db_path = library.read(cx).db_path();
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(Last(None));
    // Nothing observes an app-global pass on its own.
    crate::tasks_window::repaint_while_running(cx);
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

struct Found {
    ids: Vec<i64>,
    fetched: usize,
    matched: usize,
    unmatched: usize,
}

fn apply(
    found: Found,
    progress: &Progress,
    library: &Entity<Library>,
    scrobbler: &Entity<Scrobbler>,
    cx: &mut App,
) -> Summary {
    // Counted before the write, after which they all read as favourites.
    let already = library.read(cx).favourite_ids();
    let added = found.ids.iter().filter(|id| !already.contains(id)).count();
    library.update(cx, |library, cx| {
        library.set_favourites(&found.ids, true, cx);
    });
    // These came from Last.fm: the mirror absorbs them rather than pushing them
    // back. Same update pass, so it lands before the library event reaches the
    // mirror's diff.
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
            progress.total.store(pages.total, Ordering::Relaxed);
        }
        if page >= pages.count.min(MAX_PAGES) || !progress.keep_going() {
            break;
        }
        page += 1;
        std::thread::sleep(PAGE_PAUSE);
    }

    progress.say(rox_i18n::t!("lastfm-import-matching"));
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let index = Index::build(store::name_index(&conn).map_err(|e| e.to_string())?);
    let mut ids: Vec<i64> = Vec::new();
    let mut matched = 0usize;
    let mut unmatched = 0usize;
    for track in &loved {
        let found = index.resolve(&track.artist, &track.title);
        if found.is_empty() {
            unmatched += 1;
            // Misses can number in the hundreds, so they go to the log.
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

/// No album: the loved list doesn't include one.
struct Loved {
    artist: String,
    title: String,
}

struct Pages {
    count: usize,
    total: usize,
}

/// Unsigned: a public read of a named account needs an api key, no session.
fn fetch_page(key: &str, user: &str, page: usize) -> Result<(Vec<Loved>, Pages), String> {
    let request = agent()
        .get(API)
        .query("method", "user.getlovedtracks")
        .query("user", user)
        .query("api_key", key)
        .query("limit", &PAGE.to_string())
        .query("page", &page.to_string())
        .query("format", "json");
    // An API error still has a JSON body, so a status failure parses too.
    let text = match request.call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(net_reason(&e)),
    };
    parse_page(&text)
}

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
                // Strings today, but take numbers too.
                serde_json::Value::String(s) => s.parse().unwrap_or(0),
                other => other.as_u64().unwrap_or(0) as usize,
            })
            .unwrap_or(0)
    };
    // Last.fm returns a lone item as an object rather than a one-element array.
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

struct Entry {
    title: String,
    /// `title` with bracketed qualifiers dropped first.
    bare: String,
    id: i64,
}

/// Normalized artist to every track filed under it. Shared by both imports.
pub(crate) struct Index(HashMap<String, Vec<Entry>>);

impl Index {
    pub(crate) fn build(rows: Vec<(i64, String, String)>) -> Index {
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

    /// Every library track the name could mean, empty when unsure. All of them:
    /// a heart goes on every copy, while [`super::plays_import`] picks one.
    ///
    /// The bracket-stripped second look only settles on a single title, so a
    /// studio and a live take that differ by nothing else never match.
    pub(crate) fn resolve(&self, artist: &str, title: &str) -> Vec<i64> {
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

pub(crate) fn bare(title: &str) -> String {
    normalize(&strip_brackets(title))
}

/// An unclosed group takes the rest of the line with it.
pub(crate) fn strip_brackets(s: &str) -> String {
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
    fn an_unknown_name_matches_nothing() {
        assert!(library().resolve("Aphex Twin", "Xtal").is_empty());
        assert!(
            library()
                .resolve("Boards of Canada", "Dayvan Cowboy")
                .is_empty()
        );
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
