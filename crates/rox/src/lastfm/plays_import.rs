//! The play-counts import: Last.fm's user top tracks pulled back into the
//! library as listen history, populating the tracklist's plays column, smart
//! playlists, and history.
//!
//! Runs as a dynamic task like the loved-tracks import ([`super::import`]),
//! stepping through pages of an account's top tracks. It is started from
//! Settings -> Last.fm, reports live progress in the tasks window, and can
//! be stopped at any page boundary.
//!
//! The import is idempotent: running it multiple times will not duplicate
//! plays. It only backfills missing listens up to each track's Last.fm playcount,
//! anchored before existing local listens so that real local listening timestamps
//! are preserved.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, Entity, Global, SharedString};

use rox_library::store;
use rox_net::providers::{agent, net_reason};
use rox_services::catalog::Library;

use super::import::{Index, api_key, username};

const API: &str = "https://ws.audioscrobbler.com/2.0/";

/// Tracks asked for per request.
const PAGE: usize = 200;

/// Pause between requests to stay polite to Last.fm rate limits.
const PAGE_PAUSE: Duration = Duration::from_millis(250);

/// Maximum pages to fetch before stopping.
const MAX_PAGES: usize = 500;

/// Live progress of a play count import, written by the worker and polled by the UI.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    unmatched: AtomicUsize,
    current: Mutex<String>,
    cancel: AtomicBool,
    pace: rox_core::pace::Pace,
}

impl Progress {
    /// Tracks fetched so far.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Total tracks the account holds. Zero until the first page returns.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Tracks with no home in this library.
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

/// What a play-count import accomplished.
#[derive(Clone, Copy, Default)]
pub struct Summary {
    /// Top tracks the account holds and this run read.
    pub fetched: usize,
    /// Of those, how many named at least one track in this library.
    pub matched: usize,
    /// Total play records backfilled into the library's history.
    pub updated: usize,
    /// Tracks with no unambiguous home here.
    pub unmatched: usize,
    /// Whether it was stopped rather than reaching the end.
    pub stopped: bool,
}

impl Summary {
    /// The one-line report, matching the loved-tracks report cadence.
    pub fn line(&self) -> String {
        let head = if self.stopped {
            rox_i18n::t!("lastfm-import-plays-stopped", count = self.fetched as u64).to_string()
        } else {
            rox_i18n::t!("lastfm-import-plays-read", count = self.fetched as u64).to_string()
        };
        let mut line = head;
        line.push_str(&rox_i18n::t!(
            "lastfm-import-plays-matched",
            count = self.matched as u64
        ));
        line.push_str(&rox_i18n::t!(
            "lastfm-import-plays-updated",
            count = self.updated as u64
        ));
        line
    }
}

#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

#[derive(Default)]
struct Last(Option<Result<Summary, SharedString>>);

impl Global for Last {}

/// The running import's progress, or None while nothing is importing.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// How the last import went: its summary, or why it failed.
pub fn last(cx: &App) -> Option<Result<Summary, SharedString>> {
    cx.try_global::<Last>().and_then(|l| l.0.clone())
}

/// Drop the last run's report.
pub fn dismiss(cx: &mut App) {
    cx.set_global(Last(None));
}

/// Ask the running import to stop at the next page.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Why a play-count import cannot run right now.
pub fn blocked_reason(cx: &App) -> Option<&'static str> {
    if progress(cx).is_some() || super::import::progress(cx).is_some() {
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

/// Start importing play counts from Last.fm in the background.
pub fn start(library: Entity<Library>, cx: &mut App) {
    if blocked_reason(cx).is_some() {
        return;
    }
    let user = username();
    let key = api_key();
    let db_path = library.read(cx).db_path();
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(Last(None));
    crate::tasks_window::repaint_while_running(cx);
    crate::tasks_window::open(cx);
    cx.spawn(async move |cx| {
        let outcome = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&user, &key, &db_path, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            let result = match outcome {
                Ok(summary) => {
                    library.update(cx, |library, cx| {
                        library.reload_plays(cx);
                    });
                    Ok(summary)
                }
                Err(e) => {
                    log::warn!("lastfm: importing play counts: {e}");
                    Err(SharedString::from(e))
                }
            };
            cx.set_global(Last(Some(result)));
        })
        .ok();
    })
    .detach();
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TopTrack {
    artist: String,
    title: String,
    playcount: u32,
}

struct Pages {
    count: usize,
    total: usize,
}

fn run(
    user: &str,
    key: &str,
    db_path: &std::path::Path,
    progress: &Progress,
) -> Result<Summary, String> {
    let mut tracks: Vec<TopTrack> = Vec::new();
    let mut page = 1;
    progress.pace.begin();
    loop {
        let (entries, pages) = fetch_page(key, user, page)?;
        if let Some(last) = entries.last() {
            progress.say(format!(
                "{} - {} ({} plays)",
                last.artist, last.title, last.playcount
            ));
        }
        let count_on_page = entries.len();
        tracks.extend(entries);
        progress.done.store(tracks.len(), Ordering::Relaxed);
        if page == 1 {
            progress.total.store(pages.total, Ordering::Relaxed);
        }
        if page >= pages.count.min(MAX_PAGES) || count_on_page == 0 || !progress.keep_going() {
            break;
        }
        page += 1;
        std::thread::sleep(PAGE_PAUSE);
    }

    progress.say(rox_i18n::t!("lastfm-import-matching"));
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    let index = Index::build(store::name_index(&conn).map_err(|e| e.to_string())?);
    let mut targets: Vec<(i64, u32)> = Vec::new();
    let mut matched = 0usize;
    let mut unmatched = 0usize;

    for item in &tracks {
        let found = index.resolve(&item.artist, &item.title);
        if found.is_empty() {
            unmatched += 1;
            log::debug!("lastfm: no match for {} - {}", item.artist, item.title);
            continue;
        }
        matched += 1;
        for id in found {
            targets.push((id, item.playcount));
        }
    }
    progress.unmatched.store(unmatched, Ordering::Relaxed);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let updated = rox_library::listens::backfill_plays_batch(&mut conn, &targets, now)
        .map_err(|e| e.to_string())?;

    Ok(Summary {
        fetched: tracks.len(),
        matched,
        updated,
        unmatched,
        stopped: progress.stopping(),
    })
}

fn fetch_page(key: &str, user: &str, page: usize) -> Result<(Vec<TopTrack>, Pages), String> {
    let request = agent()
        .get(API)
        .query("method", "user.gettoptracks")
        .query("user", user)
        .query("api_key", key)
        .query("period", "overall")
        .query("limit", &PAGE.to_string())
        .query("page", &page.to_string())
        .query("format", "json");

    let text = match request.call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(net_reason(&e)),
    };
    parse_page(&text)
}

fn parse_page(text: &str) -> Result<(Vec<TopTrack>, Pages), String> {
    let body: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if body.get("error").is_some() {
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error");
        return Err(message.to_string());
    }
    let Some(toptracks) = body.get("toptracks") else {
        return Err("no toptracks in the response".into());
    };
    let attr = toptracks.get("@attr");
    let number = |field: &str| -> usize {
        attr.and_then(|a| a.get(field))
            .map(|v| match v {
                serde_json::Value::String(s) => s.parse().unwrap_or(0),
                other => other.as_u64().unwrap_or(0) as usize,
            })
            .unwrap_or(0)
    };
    let entries: Vec<&serde_json::Value> = match toptracks.get("track") {
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
            let artist = match row.get("artist") {
                Some(serde_json::Value::Object(map)) => map
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim(),
                Some(serde_json::Value::String(s)) => s.trim(),
                _ => "",
            };
            let playcount = match row.get("playcount") {
                Some(serde_json::Value::String(s)) => s.parse::<u32>().unwrap_or(0),
                Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0) as u32,
                _ => 0,
            };
            (!title.is_empty() && !artist.is_empty() && playcount > 0).then(|| TopTrack {
                artist: artist.to_string(),
                title: title.to_string(),
                playcount,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_tracks_page() {
        let json = r#"{
            "toptracks": {
                "track": [
                    {
                        "name": "Aria Math",
                        "playcount": "164",
                        "artist": {
                            "name": "C418"
                        }
                    },
                    {
                        "name": "Moog City 2",
                        "playcount": "139",
                        "artist": {
                            "name": "C418"
                        }
                    }
                ],
                "@attr": {
                    "user": "cuervow",
                    "totalPages": "5",
                    "page": "1",
                    "total": "980",
                    "perPage": "200"
                }
            }
        }"#;

        let (tracks, pages) = parse_page(json).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].artist, "C418");
        assert_eq!(tracks[0].title, "Aria Math");
        assert_eq!(tracks[0].playcount, 164);
        assert_eq!(tracks[1].artist, "C418");
        assert_eq!(tracks[1].title, "Moog City 2");
        assert_eq!(tracks[1].playcount, 139);
        assert_eq!(pages.count, 5);
        assert_eq!(pages.total, 980);
    }

    #[test]
    fn parses_single_top_track() {
        let json = r#"{
            "toptracks": {
                "track": {
                    "name": "Alone",
                    "playcount": 1000,
                    "artist": {
                        "name": "Heart"
                    }
                },
                "@attr": {
                    "totalPages": "1",
                    "total": "1"
                }
            }
        }"#;

        let (tracks, pages) = parse_page(json).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Alone");
        assert_eq!(tracks[0].playcount, 1000);
        assert_eq!(pages.total, 1);
    }
}
