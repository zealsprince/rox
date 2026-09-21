//! The play-history import: what Last.fm knows about an account's
//! listening, pulled back into the library as listens, populating the
//! tracklist's plays column, smart playlists, history, and the stats
//! window's charts.
//!
//! Two calls, because neither one answers the whole question.
//! `user.getRecentTracks` hands over every scrobble with the second it
//! happened at, which is the only place a real date comes from, and it is
//! read first. `user.getTopTracks` hands over totals with no dates at
//! all, and it is read second to catch what the history missed: an
//! account whose scrobbles predate what Last.fm will page back through,
//! or plays imported into Last.fm itself from somewhere else. The
//! difference between the two is placed as an even ladder down the span
//! the account has existed for, marked as invented
//! ([`rox_library::listens::ORIGIN_ESTIMATE`]) rather than dressed up as
//! history.
//!
//! Runs as a dynamic task like the loved-tracks import ([`super::import`]),
//! stepping through pages. It is started from Settings -> Last.fm, reports
//! live progress in the tasks window, and can be stopped at any page or
//! any batch of writes.
//!
//! The import is idempotent. A scrobble is identified by its track and
//! its second, so a row already sitting there is the same play and a
//! re-run only takes what arrived since; the count half only ever fills a
//! gap it can still see. Re-running duplicates nothing.

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

/// Tracks asked for per request, the API's own ceiling for both calls.
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

/// What a play-history import accomplished.
#[derive(Clone, Copy, Default)]
pub struct Summary {
    /// Top tracks the account holds and this run read. Zero on a run
    /// stopped inside the history, which never reached the counts.
    pub fetched: usize,
    /// Scrobbles this run read out of the account's history. Lower than
    /// the whole history on a re-run, which only asks for what arrived
    /// after the last one.
    pub scrobbles: usize,
    /// Of the top tracks, how many named at least one track in this library.
    pub matched: usize,
    /// Total play records written into the library's history.
    pub updated: usize,
    /// Of those, how many carry the second Last.fm says they happened at.
    /// The rest were placed to make a count add up.
    pub dated: usize,
    /// Tracks with no unambiguous home here.
    pub unmatched: usize,
    /// Whether it was stopped rather than reaching the end.
    pub stopped: bool,
}

impl Summary {
    /// The one-line report, matching the loved-tracks report cadence.
    pub fn line(&self) -> String {
        // A run stopped inside the history never reached the counts, so
        // it reports what it did read: scrobbles rather than tracks.
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
        // Nothing read means nothing to have matched, and a bare ", matched
        // 0" beside it is noise.
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
        // Only when there are any: a run that fell back to counts alone
        // shouldn't announce a zero it can do nothing about.
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
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    // Where the last run on this account got to. Asking for what arrived
    // after it is the difference between a re-import costing one page and
    // costing a decade of them. Per account, because the listens table
    // records that a row came from Last.fm and not who scrobbled it: one
    // bound across all of them meant a second account's whole history
    // read as "nothing new".
    let since = Settings::load().accounts.lastfm.imported_through(user);

    progress.pace.begin();
    progress.say(rox_i18n::t!("lastfm-import-history"));
    // A history this can't read isn't the end of the import: the counts
    // below still carry it, which is the same path an account older than
    // Last.fm's paging falls back to.
    let history = match fetch_history(key, user, since, progress) {
        Ok(history) => history,
        Err(e) => {
            log::warn!("lastfm: reading scrobble history: {e}");
            Vec::new()
        }
    };
    // Stopped partway through the history: it holds the older end of the
    // range and not the newer, so the counts would have this invent rows
    // for scrobbles the next run then imports for real. A stopped run
    // writes what it read and asks for nothing else.
    let cut_short = progress.stopping();

    let tracks = if cut_short {
        Vec::new()
    } else {
        progress.say(rox_i18n::t!("lastfm-import-plays-counts"));
        progress.done.store(0, Ordering::Relaxed);
        progress.total.store(0, Ordering::Relaxed);
        match fetch_counts(key, user, progress) {
            Ok(tracks) => tracks,
            // Only fatal when this run has nothing else to write. A key or
            // a name the service refuses fails both calls and reports
            // here; losing a history already in hand to a second failure
            // would just make the user fetch it again.
            Err(e) if history.is_empty() => return Err(e),
            Err(e) => {
                log::warn!("lastfm: reading play counts: {e}");
                Vec::new()
            }
        }
    };

    progress.say(rox_i18n::t!("lastfm-import-matching"));
    let index = Index::build(store::name_index(&conn).map_err(|e| e.to_string())?);
    // Read before anything is written: afterwards every matched track has
    // plays, and the tie-break between two copies of a song is which one
    // was already being played here.
    let current_counts = listens::counts(&conn).unwrap_or_default();
    let mut resolved: HashMap<(String, String), Option<i64>> = HashMap::new();

    // The dated half. An account scrobbles the same few hundred songs
    // thousands of times, so the matcher answers once per name and the
    // rest is a map lookup.
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
        // No date is the now-playing row: a play that hasn't finished has
        // no second to file it under, and the count half covers it on the
        // next run.
        if let Some(played_at) = scrobble.played_at {
            plays.push((track_id, played_at));
        }
    }

    // The counted half, which is everything the history couldn't date.
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

    // Real plays first, so the ladder below only ever fills what they
    // leave missing.
    progress.say(rox_i18n::t!("lastfm-import-plays-writing"));
    progress.done.store(0, Ordering::Relaxed);
    progress.total.store(plays.len(), Ordering::Relaxed);
    let dated = listens::import_scrobbles(&mut conn, &plays, |done, total| {
        report(progress, done, total)
    })
    .map_err(|e| e.to_string())?;
    // How far this account has been read now, which is where the next run
    // starts. The whole history this run saw, not just the rows that
    // landed: a scrobble of a track this library doesn't hold has nowhere
    // to go and won't on the next run either, and the count half below is
    // what covers it. Only once the writing is done, so a run that failed
    // to write doesn't leave a bound standing over scrobbles that never
    // made it in.
    if let Some(through) = history.iter().filter_map(|s| s.played_at).max() {
        let user = user.to_string();
        Settings::update(move |s| s.accounts.lastfm.note_import(&user, through));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // How far back the invented rows may reach. Asked for only when there
    // is something to place, and a profile that won't answer just leaves
    // the ladder on its default span.
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

/// The write callbacks' shared body: publish the count every so often.
/// Fifty rows between stores keeps a hundred thousand inserts from
/// spending their time on atomics.
///
/// Always true, so the writing runs to the end even after Stop. Stop ends
/// the paging; the plays already read are no less real for the rest going
/// unread, which is the promise the loved import makes too. The writes are
/// one local transaction and take seconds, not the minutes the fetching
/// does.
fn report(progress: &Progress, done: usize, total: usize) -> bool {
    if done.is_multiple_of(50) || done == total {
        progress.done.store(done, Ordering::Relaxed);
        progress.total.store(total, Ordering::Relaxed);
    }
    true
}

/// The one track a Last.fm name means here, remembered by name. Both
/// halves of the import ask about the same names, and the answer costs a
/// fold and a scan of everything the artist has.
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

/// The account's scrobble history, as far back as `since` or as far back
/// as Last.fm will page.
///
/// Read oldest page first, against the order the service hands them out.
/// That's what makes stopping safe: the rows a partial run wrote run
/// contiguously up from the bound it started at, so the next run's bound
/// still has everything below it. Newest first would leave the newest
/// scrobbles in the table with a hole underneath, and the bound would step
/// straight over what was missed.
///
/// One request is spent learning how many pages there are, since that
/// count is what the walk runs backwards from. A scrobble arriving mid-run
/// shifts every page by one, so a run can read the same play twice or step
/// over one. Neither matters much: a play already in the table is
/// recognized by its second and skipped, and one that slipped past the
/// paging is picked up by the next run.
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

/// The account's per-track totals, the half with no dates on it.
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

/// When multiple local tracks match the Last.fm title/artist (e.g. remastered
/// duplicate, compilation album), select the track that already has local plays
/// recorded, or fall back to the first row if none have plays or on ties.
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
        // Track 2 has 10 plays, track 1 and 3 have 0. Track 2 should be selected.
        assert_eq!(pick_target_track(&[1, 2, 3], &counts), Some(2));

        // If tied or none have plays, picks the first track in the slice.
        let empty = std::collections::HashMap::new();
        assert_eq!(pick_target_track(&[1, 2, 3], &empty), Some(1));

        // Empty list returns None.
        assert_eq!(pick_target_track(&[], &counts), None);
    }
}
