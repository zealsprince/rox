//! MusicBrainz (musicbrainz.org): keyless release metadata, matched by a
//! recording search over the track's artist and title. Each recording
//! has the tags a tagger fills (title, artist, and, through its best
//! matching release, album, album artist, year, track, and disc), so the
//! compare has real candidates to set from. Both credits also carry a
//! Latin sort name, which is where a Japanese-tagged track gets its
//! `ARTISTSORT` from; there is no title or album sort in the model, so
//! those two stay hand-typed in the editor.
//!
//! The service caps clients at one request a second and rejects anything
//! without a contactable User-Agent (ADR 14: the shared agent sends
//! it). The throttle is here so callers never see it, the rate limit
//! held process-wide against the next request.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{
    agent, net_reason, normalize_folded, string, MetadataCandidate, MetadataProvider, TrackQuery,
};

const API: &str = "https://musicbrainz.org/ws/2/recording";

/// The artist search, the other half of the same web service. Asked by
/// name alone, which is all the sort-name pass has: it's working from
/// library values rather than from a track.
const ARTIST_API: &str = "https://musicbrainz.org/ws/2/artist";

/// MusicBrainz's rate limit: one request a second, sustained. A single
/// lookup never hits it, but a batch would, so the gate is here rather
/// than trusted to the caller.
const MIN_INTERVAL: Duration = Duration::from_millis(1100);

/// How many times a request is re-sent after the server says it's busy.
/// A 503 from MusicBrainz is load shedding on its side, sent with the
/// client's quota untouched, and it comes and goes within seconds, so one
/// or two more tries recover nearly all of them.
const BUSY_RETRIES: u32 = 3;

/// The pause before a retry when the server names no Retry-After of its
/// own (it sends 0 while shedding). Long enough to land outside the burst
/// that shed us, short enough that a batch barely notices.
const BUSY_BACKOFF: Duration = Duration::from_secs(2);

/// The longest a single retry will wait, whatever Retry-After says.
/// MusicBrainz has been seen naming minutes while it sheds, and a bulk
/// pass that parks for that long looks hung. Past this the run is better
/// off giving up on the name and asking again next time.
const BUSY_CEILING: Duration = Duration::from_secs(30);

/// How long a wait sleeps before it looks at the cancel flag again. Short
/// enough that a stop click lands as one, long enough that the check
/// costs nothing.
const CANCEL_SLICE: Duration = Duration::from_millis(100);

/// A predicate the caller hands in to interrupt a wait, which is what the
/// bulk pass's stop button is. None means nothing can cancel, the right
/// answer for a single interactive lookup that's over in a second.
pub type Cancel<'a> = Option<&'a dyn Fn() -> bool>;

/// Why a lookup came back empty-handed, told apart because the sort-name
/// pass counts them differently: a wire failure repeated enough times
/// means the network is gone and the run should stop, while a busy
/// server is MusicBrainz's problem and says nothing about the next name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupError {
    /// The server shed the request with a 503 every time it was tried.
    Busy,
    /// The caller's cancel predicate went true while a retry waited out
    /// the server's Retry-After. Nothing went wrong; the run is stopping
    /// and this name goes back in the pile.
    Cancelled,
    /// Anything else, already folded through [`net_reason`].
    Other(String),
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookupError::Busy => write!(f, "service busy after {BUSY_RETRIES} retries"),
            LookupError::Cancelled => f.write_str("cancelled"),
            LookupError::Other(reason) => f.write_str(reason),
        }
    }
}

impl From<LookupError> for String {
    fn from(e: LookupError) -> String {
        e.to_string()
    }
}

pub struct MusicBrainz;

impl MetadataProvider for MusicBrainz {
    fn name(&self) -> &'static str {
        "musicbrainz"
    }

    fn search(&self, query: &TrackQuery) -> Result<Vec<MetadataCandidate>, String> {
        // The Lucene query the search endpoint takes: the title and artist
        // as quoted phrases, so punctuation in a title does not read as
        // query syntax. Either field alone still searches, so a hand-edited
        // query with just a title works; both empty is a clean no-match.
        let mut parts = Vec::new();
        if !query.title.is_empty() {
            parts.push(format!("recording:\"{}\"", escape(&query.title)));
        }
        if !query.artist.is_empty() {
            parts.push(format!("artist:\"{}\"", escape(&query.artist)));
        }
        if parts.is_empty() {
            return Ok(Vec::new());
        }
        let lucene = parts.join(" AND ");
        let text = fetch(
            agent()
                .get(API)
                .query("query", &lucene)
                .query("fmt", "json")
                .query("limit", "10"),
            None,
        )?;
        let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let Some(recordings) = body.get("recordings").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(recordings.len());
        for recording in recordings {
            out.push(candidate(self.name(), query, recording));
        }
        Ok(out)
    }
}

/// One recording into a candidate: its title and artist, plus the release
/// among its releases that best matches the query album, so a track
/// tagged with a specific album surfaces that release's numbers rather
/// than a random compilation's.
fn candidate(
    provider: &'static str,
    query: &TrackQuery,
    recording: &serde_json::Value,
) -> MetadataCandidate {
    let title = string(recording.get("title"));
    let artist = artist_credit(recording.get("artist-credit"));
    let artist_sort = credit_sort_name(recording.get("artist-credit"));
    let duration_secs = recording
        .get("length")
        .and_then(|v| v.as_f64())
        .map(|ms| ms / 1000.0);

    let release = recording
        .get("releases")
        .and_then(|v| v.as_array())
        .and_then(|releases| best_release(query, releases));

    let (album, album_artist, album_artist_sort, year, track_no, disc_no) = match release {
        Some(release) => {
            let album = string(release.get("title"));
            let album_artist = artist_credit(release.get("artist-credit"));
            let album_artist_sort = credit_sort_name(release.get("artist-credit"));
            let year = string(release.get("date"))
                .split('-')
                .next()
                .unwrap_or("")
                .to_string();
            // The disc and track come off the media block the recording
            // appears in: the disc is the medium's position, the track its
            // number in that medium.
            let medium = release
                .get("media")
                .and_then(|v| v.as_array())
                .and_then(|media| media.first());
            let disc_no = medium
                .and_then(|m| m.get("position"))
                .and_then(|v| v.as_u64())
                .filter(|&n| n > 0)
                .map(|n| n.to_string())
                .unwrap_or_default();
            let track_no = medium
                .and_then(|m| m.get("track"))
                .and_then(|v| v.as_array())
                .and_then(|tracks| tracks.first())
                .map(|t| string(t.get("number")))
                .unwrap_or_default();
            (
                album,
                album_artist,
                album_artist_sort,
                year,
                track_no,
                disc_no,
            )
        }
        None => (
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ),
    };

    MetadataCandidate {
        provider,
        title,
        artist,
        album,
        album_artist,
        artist_sort,
        album_artist_sort,
        year,
        track_no,
        disc_no,
        duration_secs,
        confidence: 0.0,
    }
}

/// The release whose title best matches the query album, so the candidate
/// has the numbers for the album the track claims. Falls back to the
/// first release when the query has no album to match on.
fn best_release<'a>(
    query: &TrackQuery,
    releases: &'a [serde_json::Value],
) -> Option<&'a serde_json::Value> {
    if query.album.is_empty() {
        return releases.first();
    }
    releases.iter().max_by(|a, b| {
        let score =
            |r: &serde_json::Value| super::similarity(&query.album, &string(r.get("title")));
        score(a)
            .partial_cmp(&score(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// An artist-credit array folded to one display string, joining each name
/// with its own join phrase ("Artist feat. Guest"), the shape a tag
/// stores.
fn artist_credit(credit: Option<&serde_json::Value>) -> String {
    let Some(array) = credit.and_then(|v| v.as_array()) else {
        return String::new();
    };
    let mut out = String::new();
    for part in array {
        out.push_str(&string(part.get("name")));
        out.push_str(
            part.get("joinphrase")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
    }
    out.trim().to_string()
}

/// The Latin sort name of the first credited artist, the inverted form
/// ("Yonezu, Kenshi") that `ARTISTSORT` wants. It rides along in the
/// response rox already asks for, no `inc=` and no second request. Only
/// the first credit: the sort tag names the artist the row files under,
/// not the whole "feat." chain. Empty when the credit or the sort name is
/// missing, which is normal and leaves the compare row with nothing to
/// apply.
fn credit_sort_name(credit: Option<&serde_json::Value>) -> String {
    credit
        .and_then(|v| v.as_array())
        .and_then(|array| array.first())
        .and_then(|part| part.get("artist"))
        .map(|artist| string(artist.get("sort-name")))
        .unwrap_or_default()
}

/// Look one artist name up and return the Latin sort name MusicBrainz
/// files them under, or None when nothing there is confidently the same
/// artist.
///
/// This is the bulk pass's whole wire surface. It's a name in and a name
/// out rather than a `MetadataProvider` call, because the pass is working
/// through library values with no track behind them: there's no title to
/// search a recording by, no candidate list to rank, and nothing for the
/// confirmed picker ADR 14 asks for to show. What it writes is a row in
/// rox's own table, never a file, which is what makes a bulk run of it
/// legitimate at all.
///
/// Ok(None) is the ordinary answer for an artist MusicBrainz doesn't
/// know, and the caller stores nothing for it, so the next run asks
/// again. An Err is the wire failing, which is worth telling apart.
///
/// `cancel` is polled while a busy-server retry waits, so a bulk pass can
/// be stopped without sitting through the server's Retry-After first.
pub fn artist_sort_name(name: &str, cancel: Cancel<'_>) -> Result<Option<String>, LookupError> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let text = fetch(
        agent()
            .get(ARTIST_API)
            .query("query", &format!("artist:\"{}\"", escape(name)))
            .query("fmt", "json")
            // Three, not one: the top hit for a common name is often a
            // different act with the same spelling, and `pick_sort_name`
            // wants a couple of rows to find the exact name among.
            .query("limit", "3"),
        cancel,
    )?;
    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| LookupError::Other(e.to_string()))?;
    Ok(pick_sort_name(name, &body))
}

/// Send one request under the throttle and hand back its body, re-sending
/// it when the server says it's busy.
///
/// MusicBrainz answers a 503 with "the web server is currently busy" from
/// a global shedding zone, with the client's own quota untouched and a
/// Retry-After of 0, and a request a couple of seconds later usually goes
/// through. Retrying here keeps every caller from having to know that.
/// Any other status or a transport failure is handed back on the first
/// try, since repeating those wouldn't change the answer.
fn fetch(request: ureq::Request, cancel: Cancel<'_>) -> Result<String, LookupError> {
    let mut attempt = 0;
    loop {
        if cancelled(cancel) {
            return Err(LookupError::Cancelled);
        }
        throttle();
        match request.clone().call() {
            Ok(response) => {
                return response
                    .into_string()
                    .map_err(|e| LookupError::Other(e.to_string()));
            }
            Err(ureq::Error::Status(503, response)) => {
                if attempt >= BUSY_RETRIES {
                    return Err(LookupError::Busy);
                }
                attempt += 1;
                // Clamped both ways: the header is a hint from a server
                // under load, not an instruction worth handing a bulk
                // pass's whole afternoon to, and a one-second hint would
                // land us back inside the burst that shed us.
                let wait = response
                    .header("retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .filter(|d| !d.is_zero())
                    .unwrap_or(BUSY_BACKOFF)
                    .clamp(BUSY_BACKOFF, BUSY_CEILING);
                wait_out(wait, cancel)?;
            }
            Err(e) => return Err(LookupError::Other(net_reason(&e))),
        }
    }
}

/// Whether the caller wants out. No predicate is no cancel.
fn cancelled(cancel: Cancel<'_>) -> bool {
    cancel.is_some_and(|stop| stop())
}

/// Sleep `total`, in slices, giving up as soon as the caller cancels. A
/// plain `sleep` here would hold a stopped pass for the length of whatever
/// the server asked for, which is the whole reason the wait is sliced.
fn wait_out(total: Duration, cancel: Cancel<'_>) -> Result<(), LookupError> {
    let deadline = Instant::now() + total;
    loop {
        if cancelled(cancel) {
            return Err(LookupError::Cancelled);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(CANCEL_SLICE));
    }
}

/// Which of the returned artists is the one that was asked for, and what
/// it files under.
///
/// The name has to match, compared through [`normalize_folded`] so casing,
/// punctuation and accents don't decide it, against the artist's own name
/// or any alias the response carries. An alias counts because MusicBrainz
/// files plenty of Japanese acts under a Latin primary name with the
/// native spelling as an alias, which is the exact pair the library has.
///
/// Nothing else counts. The score used to be a fallback, on the theory
/// that MusicBrainz's own 100 means "this is the name you typed", and it
/// doesn't: the search scores relevance, and a compilation credit like
/// Various Artists takes 100 at the top of a result set for a real
/// artist. Filing every one of that artist's rows under Various Artists
/// is worse than the empty column it replaced, and there's no recovering
/// from it without knowing which rows the pass wrote. No name match is no
/// answer, and the next run asks again. An empty sort name on the winner
/// is a miss too.
fn pick_sort_name(name: &str, body: &serde_json::Value) -> Option<String> {
    let artists = body.get("artists")?.as_array()?;
    let wanted = normalize_folded(name);
    let picked = artists
        .iter()
        .find(|a| normalize_folded(&string(a.get("name"))) == wanted || has_alias(a, &wanted))?;
    let sort = string(picked.get("sort-name"));
    (!sort.is_empty()).then_some(sort)
}

/// Whether one of the artist's aliases is the name that was asked for,
/// folded the same way. Absent aliases are the common case (the search
/// only carries them for artists that have them) and read as no match.
fn has_alias(artist: &serde_json::Value, wanted: &str) -> bool {
    let Some(aliases) = artist.get("aliases").and_then(|v| v.as_array()) else {
        return false;
    };
    aliases
        .iter()
        .any(|alias| normalize_folded(&string(alias.get("name"))) == wanted)
}

/// Escape the Lucene specials that would otherwise steer the query, the
/// quote and backslash a title can hold.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Hold the process to one request a second: if the last one was under
/// the interval ago, sleep the remainder. Blocking, background executor
/// only, never the audio path.
fn throttle() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(prev) = *last {
        let elapsed = prev.elapsed();
        if elapsed < MIN_INTERVAL {
            std::thread::sleep(MIN_INTERVAL - elapsed);
        }
    }
    *last = Some(Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed capture of the real response for the query rox builds,
    /// cut to the keys `candidate` reads. The sort names ride on the
    /// recording's credit and on the release's, which is what lets one
    /// request fill both fields.
    const RECORDING: &str = r#"{
        "title": "Lemon",
        "length": 255000,
        "artist-credit": [
            { "name": "米津玄師", "artist": { "name": "米津玄師", "sort-name": "Yonezu, Kenshi" } }
        ],
        "releases": [
            {
                "title": "Lemon",
                "date": "2018-03-14",
                "artist-credit": [
                    { "name": "米津玄師", "artist": { "name": "米津玄師", "sort-name": "Yonezu, Kenshi" } }
                ],
                "media": [{ "position": 1, "track": [{ "number": "1" }] }]
            }
        ]
    }"#;

    /// The same shape with both `sort-name` keys gone, which is how plenty
    /// of real MusicBrainz entries come back.
    const NO_SORT: &str = r#"{
        "title": "Lemon",
        "artist-credit": [{ "name": "米津玄師", "artist": { "name": "米津玄師" } }],
        "releases": [
            {
                "title": "Lemon",
                "artist-credit": [{ "name": "米津玄師", "artist": { "name": "米津玄師" } }]
            }
        ]
    }"#;

    fn query() -> TrackQuery {
        TrackQuery {
            artist: "米津玄師".to_string(),
            title: "Lemon".to_string(),
            album: "Lemon".to_string(),
            duration_secs: None,
        }
    }

    fn parse(json: &str) -> MetadataCandidate {
        let recording: serde_json::Value = serde_json::from_str(json).expect("fixture parses");
        candidate("musicbrainz", &query(), &recording)
    }

    #[test]
    fn sort_names_come_off_both_credits() {
        let candidate = parse(RECORDING);
        assert_eq!(candidate.artist, "米津玄師");
        assert_eq!(candidate.artist_sort, "Yonezu, Kenshi");
        assert_eq!(candidate.album_artist_sort, "Yonezu, Kenshi");
    }

    #[test]
    fn missing_sort_names_come_back_empty() {
        let candidate = parse(NO_SORT);
        assert_eq!(candidate.album_artist, "米津玄師");
        assert!(candidate.artist_sort.is_empty());
        assert!(candidate.album_artist_sort.is_empty());
    }

    /// A trimmed capture of the artist search for 米津玄師, cut to the
    /// keys `pick_sort_name` reads. The second entry is the shape that
    /// makes the exact-name check worth having: a high-scoring hit that
    /// isn't the artist asked for.
    const ARTIST_SEARCH: &str = r#"{
        "artists": [
            { "score": 100, "name": "米津玄師", "sort-name": "Yonezu, Kenshi" },
            { "score": 90, "name": "Kenshi Yonezu Tribute", "sort-name": "Tribute, Kenshi Yonezu" }
        ]
    }"#;

    /// The artist named second, which is how MusicBrainz orders a search
    /// where a compilation credit outscores the person.
    const ARTIST_SECOND: &str = r#"{
        "artists": [
            { "score": 100, "name": "Various Artists", "sort-name": "Various Artists" },
            { "score": 99, "name": "崎山蒼志", "sort-name": "Sakiyama, Soushi" }
        ]
    }"#;

    /// Nothing there is the artist asked for, so the pass writes nothing
    /// and tries again next run.
    const ARTIST_NO_MATCH: &str = r#"{
        "artists": [
            { "score": 62, "name": "Someone Else", "sort-name": "Else, Someone" }
        ]
    }"#;

    /// The shape that made the score fallback dangerous: MusicBrainz hands
    /// back a perfect score for a name nobody asked for, and nothing else
    /// in the set matches either.
    const ARTIST_SCORED_STRANGER: &str = r#"{
        "artists": [
            { "score": 100, "name": "Various Artists", "sort-name": "Various Artists" },
            { "score": 88, "name": "Soundtrack", "sort-name": "Soundtrack" }
        ]
    }"#;

    /// A Latin primary name with the native spelling filed as an alias,
    /// which is how MusicBrainz carries a good part of its Japanese
    /// catalogue.
    const ARTIST_ALIAS: &str = r#"{
        "artists": [
            {
                "score": 97,
                "name": "Sheena Ringo",
                "sort-name": "Ringo, Sheena",
                "aliases": [{ "name": "椎名林檎" }]
            }
        ]
    }"#;

    fn sort_name_from(json: &str, name: &str) -> Option<String> {
        let body: serde_json::Value = serde_json::from_str(json).expect("fixture parses");
        pick_sort_name(name, &body)
    }

    #[test]
    fn the_artist_search_gives_a_latin_sort_name() {
        assert_eq!(
            sort_name_from(ARTIST_SEARCH, "米津玄師"),
            Some("Yonezu, Kenshi".to_string())
        );
        // Position doesn't decide it; the name does.
        assert_eq!(
            sort_name_from(ARTIST_SECOND, "崎山蒼志"),
            Some("Sakiyama, Soushi".to_string())
        );
        // Casing and punctuation are normalized off both sides, so the
        // exact-name check lands without a perfect score behind it.
        assert_eq!(
            sort_name_from(
                r#"{ "artists": [{ "score": 71, "name": "AC/DC", "sort-name": "AC/DC" }] }"#,
                "ac dc"
            ),
            Some("AC/DC".to_string())
        );
    }

    #[test]
    fn a_search_with_nothing_matching_comes_back_empty() {
        assert_eq!(sort_name_from(ARTIST_NO_MATCH, "米津玄師"), None);
        // The accents come off both sides, so a tag spelling and a
        // MusicBrainz spelling of the same name meet in the middle. No
        // perfect score behind it, and it doesn't need one.
        assert_eq!(
            sort_name_from(
                r#"{ "artists": [{ "score": 71, "name": "Beyonce", "sort-name": "Beyonce" }] }"#,
                "Beyoncé!"
            ),
            Some("Beyonce".to_string())
        );
        // No artists at all, and an artist carrying no sort name.
        assert_eq!(sort_name_from(r#"{ "artists": [] }"#, "Nobody"), None);
        assert_eq!(sort_name_from(r#"{ "count": 0 }"#, "Nobody"), None);
        assert_eq!(
            sort_name_from(r#"{ "artists": [{ "score": 100, "name": "A" }] }"#, "A"),
            None
        );
    }

    /// The regression the score fallback was: a perfect score on somebody
    /// else's name buys nothing, because the score is relevance and not
    /// identity.
    #[test]
    fn a_perfect_score_on_another_name_is_not_an_answer() {
        assert_eq!(sort_name_from(ARTIST_SCORED_STRANGER, "崎山蒼志"), None);
    }

    #[test]
    fn an_alias_counts_as_the_name() {
        assert_eq!(
            sort_name_from(ARTIST_ALIAS, "椎名林檎"),
            Some("Ringo, Sheena".to_string())
        );
        // An artist whose aliases are all somebody else's is still a miss.
        assert_eq!(sort_name_from(ARTIST_ALIAS, "中島みゆき"), None);
    }

    /// A cancel that's already up ends the wait immediately rather than
    /// sitting out the server's Retry-After. The whole point of slicing
    /// the sleep.
    #[test]
    fn a_cancelled_wait_returns_at_once() {
        let stop = || true;
        let started = Instant::now();
        assert_eq!(
            wait_out(Duration::from_secs(60), Some(&stop)),
            Err(LookupError::Cancelled)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        // No predicate is no cancel, and a zero wait is over before it
        // starts either way.
        assert_eq!(wait_out(Duration::from_millis(0), None), Ok(()));
    }

    #[test]
    fn a_recording_with_no_release_still_parses() {
        let candidate = parse(r#"{ "title": "Lemon" }"#);
        assert!(candidate.artist_sort.is_empty());
        assert!(candidate.album_artist_sort.is_empty());
        assert!(candidate.album.is_empty());
    }
}
