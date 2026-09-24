//! The play-history import: an account's Last.fm listening pulled into the
//! library as listens.
//!
//! `user.getRecentTracks` gives every scrobble with its second and is read
//! first. `user.getTopTracks` gives undated totals and fills what the history
//! missed, as an even ladder down the account's lifetime marked
//! [`rox_library::listens::ORIGIN_ESTIMATE`] rather than passed off as
//! history.
//!
//! Idempotent: a scrobble is its track plus its second, a rerun only asks for
//! what arrived since, and the count half only fills gaps it can still see.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, Entity, Global, SharedString};

use rox_core::settings::Settings;
use rox_library::listens::{self, Ladder};
use rox_library::store;
use rox_net::lastfm::user::{self, Scrobble};
use rox_net::providers::{agent, net_reason};
use rox_services::catalog::Library;

use super::import::{Index, api_key, username};

/// The API's own ceiling for both calls.
const PAGE: usize = 200;

const PAGE_PAUSE: Duration = Duration::from_millis(250);

const MAX_PAGES: usize = 500;

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
    /// Zero on a run stopped inside the history, which never reached the counts.
    pub fetched: usize,
    pub scrobbles: usize,
    pub matched: usize,
    pub updated: usize,
    /// Rows carrying Last.fm's own second; the rest were placed to make a count add up.
    pub dated: usize,
    pub unmatched: usize,
    pub stopped: bool,
}

impl Summary {
    pub fn line(&self) -> String {
        let mut line = if self.stopped && self.fetched == 0 {
            rox_i18n::t!(
                "lastfm-import-plays-stopped-history",
                count = self.scrobbles as u64
            )
            .to_string()
        } else if self.stopped {
            rox_i18n::t!("lastfm-import-plays-stopped", count = self.fetched as u64).to_string()
        } else {
            rox_i18n::t!("lastfm-import-plays-read", count = self.fetched as u64).to_string()
        };
        if self.fetched > 0 {
            line.push_str(&rox_i18n::t!(
                "lastfm-import-plays-matched",
                count = self.matched as u64
            ));
        }
        line.push_str(&rox_i18n::t!(
            "lastfm-import-plays-updated",
            count = self.updated as u64
        ));
        if self.dated > 0 {
            line.push_str(&rox_i18n::t!(
                "lastfm-import-plays-dated",
                count = self.dated as u64
            ));
        }
        line
    }
}

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

pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

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
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    // Per account: the listens table records that a row came from Last.fm, not
    // who scrobbled it, so one shared bound would hide a second account's history.
    let since = Settings::load().accounts.lastfm.imported_through(user);

    progress.pace.begin();
    progress.say(rox_i18n::t!("lastfm-import-history"));
    // An unreadable history isn't fatal: the counts below still carry it.
    let history = match fetch_history(key, user, since, progress) {
        Ok(history) => history,
        Err(e) => {
            log::warn!("lastfm: reading scrobble history: {e}");
            Vec::new()
        }
    };
    // Stopped mid-history: the counts would invent rows for scrobbles the next
    // run imports for real, so write what was read and stop there.
    let cut_short = progress.stopping();

    let tracks = if cut_short {
        Vec::new()
    } else {
        progress.say(rox_i18n::t!("lastfm-import-plays-counts"));
        progress.done.store(0, Ordering::Relaxed);
        progress.total.store(0, Ordering::Relaxed);
        match fetch_counts(key, user, progress) {
            Ok(tracks) => tracks,
            // Only fatal with nothing else to write; don't throw away a history in hand.
            Err(e) if history.is_empty() => return Err(e),
            Err(e) => {
                log::warn!("lastfm: reading play counts: {e}");
                Vec::new()
            }
        }
    };

    progress.say(rox_i18n::t!("lastfm-import-matching"));
    let index = Index::build(store::name_index(&conn).map_err(|e| e.to_string())?);
    // Read before writing: ties between copies of a song go to the one already
    // being played here.
    let current_counts = listens::counts(&conn).unwrap_or_default();
    let mut resolved: HashMap<(String, String), Option<i64>> = HashMap::new();

    // Names repeat thousands of times, so the matcher answers once per name.
    let mut plays: Vec<(i64, i64)> = Vec::new();
    for scrobble in &history {
        let Some(track_id) = target_for(
            &index,
            &current_counts,
            &mut resolved,
            &scrobble.artist,
            &scrobble.title,
        ) else {
            continue;
        };
        // No date is the now-playing row; the count half covers it next run.
        if let Some(played_at) = scrobble.played_at {
            plays.push((track_id, played_at));
        }
    }

    let mut targets: Vec<(i64, u32)> = Vec::new();
    let mut matched = 0usize;
    let mut unmatched = 0usize;
    for item in &tracks {
        let Some(track_id) = target_for(
            &index,
            &current_counts,
            &mut resolved,
            &item.artist,
            &item.title,
        ) else {
            unmatched += 1;
            log::debug!("lastfm: no match for {} - {}", item.artist, item.title);
            continue;
        };
        matched += 1;
        targets.push((track_id, item.playcount));
    }
    progress.unmatched.store(unmatched, Ordering::Relaxed);

    // Real plays first, so the ladder only fills what they leave missing.
    progress.say(rox_i18n::t!("lastfm-import-plays-writing"));
    progress.done.store(0, Ordering::Relaxed);
    progress.total.store(plays.len(), Ordering::Relaxed);
    let dated = listens::import_scrobbles(&mut conn, &plays, |done, total| {
        report(progress, done, total)
    })
    .map_err(|e| e.to_string())?;
    // The next run's bound: the whole history seen, not just rows that landed,
    // since unmatched scrobbles won't match next time either. Only after the
    // writes succeed, so a failed write leaves no bound over lost scrobbles.
    if let Some(through) = history.iter().filter_map(|s| s.played_at).max() {
        let user = user.to_string();
        Settings::update(move |s| s.accounts.lastfm.note_import(&user, through));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let ladder = Ladder {
        now,
        since: if targets.is_empty() {
            None
        } else {
            user::registered_at(key, user)
                .map_err(|e| log::warn!("lastfm: reading the account's registration: {e}"))
                .ok()
                .flatten()
        },
    };
    progress.done.store(0, Ordering::Relaxed);
    progress.total.store(targets.len(), Ordering::Relaxed);
    let estimated = listens::backfill_plays_batch(&mut conn, &targets, ladder, |done, total| {
        report(progress, done, total)
    })
    .map_err(|e| e.to_string())?;

    Ok(Summary {
        fetched: tracks.len(),
        scrobbles: history.len(),
        matched,
        updated: dated + estimated,
        dated,
        unmatched,
        stopped: progress.stopping(),
    })
}

/// Publishes every fifty rows. Always true: Stop ends the paging, but the
/// plays already read still get written.
fn report(progress: &Progress, done: usize, total: usize) -> bool {
    if done.is_multiple_of(50) || done == total {
        progress.done.store(done, Ordering::Relaxed);
        progress.total.store(total, Ordering::Relaxed);
    }
    true
}

/// Memoized: both halves ask about the same names.
fn target_for(
    index: &Index,
    current_counts: &HashMap<i64, u32>,
    resolved: &mut HashMap<(String, String), Option<i64>>,
    artist: &str,
    title: &str,
) -> Option<i64> {
    let key = (artist.to_string(), title.to_string());
    if let Some(found) = resolved.get(&key) {
        return *found;
    }
    let found = pick_target_track(&index.resolve(artist, title), current_counts);
    resolved.insert(key, found);
    found
}

/// Read oldest page first, so a stopped run's rows sit contiguously above
/// its starting bound and the next bound never steps over a hole. A scrobble
/// arriving mid-run can shift pages by one; the second-based identity and the
/// next run absorb that.
fn fetch_history(
    key: &str,
    user: &str,
    since: Option<i64>,
    progress: &Progress,
) -> Result<Vec<Scrobble>, String> {
    let shape = user::recent_tracks(key, user, 1, since, PAGE)?;
    progress.total.store(shape.total, Ordering::Relaxed);
    let pages = shape.pages.min(MAX_PAGES);
    if pages <= 1 {
        progress
            .done
            .store(shape.scrobbles.len(), Ordering::Relaxed);
        return Ok(shape.scrobbles);
    }

    let mut history: Vec<Scrobble> = Vec::new();
    for page in (1..=pages).rev() {
        if !progress.keep_going() {
            break;
        }
        std::thread::sleep(PAGE_PAUSE);
        let read = user::recent_tracks(key, user, page, since, PAGE)?;
        if let Some(first) = read.scrobbles.first() {
            progress.say(format!("{} - {}", first.artist, first.title));
        }
        history.extend(read.scrobbles);
        progress.done.store(history.len(), Ordering::Relaxed);
    }
    Ok(history)
}

fn fetch_counts(key: &str, user: &str, progress: &Progress) -> Result<Vec<TopTrack>, String> {
    let mut tracks: Vec<TopTrack> = Vec::new();
    let mut page = 1;
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
    Ok(tracks)
}

/// Among copies of one song, the one with local plays wins; ties take the first.
fn pick_target_track(found: &[i64], current_counts: &HashMap<i64, u32>) -> Option<i64> {
    if found.is_empty() {
        return None;
    }
    if found.len() == 1 {
        return Some(found[0]);
    }
    let mut best_id = found[0];
    let mut max_plays = current_counts.get(&best_id).copied().unwrap_or(0);
    for &id in &found[1..] {
        let plays = current_counts.get(&id).copied().unwrap_or(0);
        if plays > max_plays {
            best_id = id;
            max_plays = plays;
        }
    }
    Some(best_id)
}

fn fetch_page(key: &str, user: &str, page: usize) -> Result<(Vec<TopTrack>, Pages), String> {
    let request = agent()
        .get(&rox_net::lastfm::api_root())
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

    #[test]
    fn picks_track_with_existing_local_plays_over_duplicates() {
        let mut counts = std::collections::HashMap::new();
        counts.insert(2, 10);
        assert_eq!(pick_target_track(&[1, 2, 3], &counts), Some(2));

        let empty = std::collections::HashMap::new();
        assert_eq!(pick_target_track(&[1, 2, 3], &empty), Some(1));

        assert_eq!(pick_target_track(&[], &counts), None);
    }
}
