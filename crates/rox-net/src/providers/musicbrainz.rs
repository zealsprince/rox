//! MusicBrainz (musicbrainz.org): keyless recording search for tag
//! candidates, by-id lookups for the fingerprint identify, and the artist
//! search behind the sort-name pass. Both credits carry a Latin sort name,
//! which is where a Japanese-tagged track gets its `ARTISTSORT`.
//!
//! The service allows one request a second and needs a contactable
//! User-Agent (ADR 14); the throttle here is process-wide.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{
    MetadataCandidate, MetadataProvider, TrackQuery, agent, net_reason, normalize_folded, string,
};

const API: &str = "https://musicbrainz.org/ws/2/recording";

const ARTIST_API: &str = "https://musicbrainz.org/ws/2/artist";

/// For the label and ISRCs a recording search leaves out.
const RELEASE_API: &str = "https://musicbrainz.org/ws/2/release";

/// One request a second, sustained, with some room.
const MIN_INTERVAL: Duration = Duration::from_millis(1100);

/// A 503 is MusicBrainz shedding load with the client's quota untouched,
/// and it clears within seconds.
const BUSY_RETRIES: u32 = 3;

/// Used when Retry-After is 0, which it is while shedding.
const BUSY_BACKOFF: Duration = Duration::from_secs(2);

/// MusicBrainz has been seen naming minutes while it sheds; a bulk pass
/// parked that long looks hung, so give up on the name instead.
const BUSY_CEILING: Duration = Duration::from_secs(30);

const CANCEL_SLICE: Duration = Duration::from_millis(100);

/// The bulk pass's stop button. None for a single interactive lookup.
pub type Cancel<'a> = Option<&'a dyn Fn() -> bool>;

/// Told apart because the sort-name pass stops on repeated wire failures
/// but not on a busy server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupError {
    Busy,
    /// Cancelled while waiting out a Retry-After; the name goes back in the pile.
    Cancelled,
    /// Only by-id lookups see this; a search answers 200 with an empty list.
    NotFound,
    Other(String),
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookupError::Busy => write!(f, "service busy after {BUSY_RETRIES} retries"),
            LookupError::Cancelled => f.write_str("cancelled"),
            LookupError::NotFound => f.write_str("no such entry"),
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
        // Quoted phrases, so punctuation in a title doesn't read as query syntax.
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

/// One recording by MBID, scored against `query` like a searched candidate.
/// Ok(None) is what an AcoustID hit on a merged or deleted recording looks
/// like.
pub fn recording_by_id(
    mbid: &str,
    query: &TrackQuery,
) -> Result<Option<MetadataCandidate>, String> {
    let mbid = mbid.trim();
    // The id lands in the URL path, so refuse anything not shaped like an
    // MBID. A malformed id is a miss, not an error.
    if mbid.is_empty() || !mbid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Ok(None);
    }
    let text = match fetch(
        agent()
            .get(&format!("{API}/{mbid}"))
            // The same fields `candidate` reads from a search result.
            .query("inc", "artist-credits+releases+media")
            .query("fmt", "json"),
        None,
    ) {
        Ok(text) => text,
        Err(LookupError::NotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let recording: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(recording_candidate(query, &recording))
}

/// A body with no title isn't a recording: None, same as an unknown id.
fn recording_candidate(
    query: &TrackQuery,
    recording: &serde_json::Value,
) -> Option<MetadataCandidate> {
    if string(recording.get("title")).is_empty() {
        return None;
    }
    let mut candidate = candidate("musicbrainz", query, recording);
    candidate.confidence = super::score_fields(
        query,
        &candidate.title,
        &candidate.artist,
        &candidate.album,
        candidate.duration_secs,
    );
    Some(candidate)
}

/// The recording plus its release best matching the query album, so a track
/// gets that release's numbers rather than a compilation's.
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
            // The search endpoint names this array "track" and the by-id
            // lookup "tracks" (verified against both).
            let track_no = medium
                .and_then(|m| m.get("tracks").or_else(|| m.get("track")))
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

/// The metadata panel's release rows, serialized as the release facts
/// store's cache; missing fields default so old entries still load.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReleaseFacts {
    pub recording_mbid: String,
    pub release_mbid: String,
    /// May differ from the tag's album.
    pub release_title: String,
    /// The recording's earliest release date, "YYYY", "YYYY-MM", or full.
    pub first_release_date: String,
    pub release_date: String,
    /// ISO 3166, "XW" for worldwide.
    pub country: String,
    pub label: String,
    pub catalog_number: String,
    pub barcode: String,
    pub isrc: String,
}

/// Enough to catch a cover version outscoring the original on a partial title.
const FACTS_CANDIDATES: usize = 5;

/// Below this the search matched a different song; a wrong label is worse than none.
const FACTS_MIN_CONFIDENCE: f32 = 0.6;

/// Two throttled calls: the recording search, then the release lookup for
/// label and ISRC.
pub fn release_facts(query: &TrackQuery) -> Result<Option<ReleaseFacts>, String> {
    let mut parts = Vec::new();
    if !query.title.is_empty() {
        parts.push(format!("recording:\"{}\"", escape(&query.title)));
    }
    if !query.artist.is_empty() {
        parts.push(format!("artist:\"{}\"", escape(&query.artist)));
    }
    if parts.is_empty() {
        return Ok(None);
    }
    let text = fetch(
        agent()
            .get(API)
            .query("query", &parts.join(" AND "))
            .query("fmt", "json")
            .query("limit", &FACTS_CANDIDATES.to_string()),
        None,
    )?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let Some(recordings) = body.get("recordings").and_then(|v| v.as_array()) else {
        return Ok(None);
    };
    let best = recordings
        .iter()
        .map(|recording| {
            let candidate = candidate("musicbrainz", query, recording);
            let confidence = super::score_fields(
                query,
                &candidate.title,
                &candidate.artist,
                &candidate.album,
                candidate.duration_secs,
            );
            (confidence, recording)
        })
        .filter(|(confidence, _)| *confidence >= FACTS_MIN_CONFIDENCE)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let Some((_, recording)) = best else {
        return Ok(None);
    };
    let mut facts = ReleaseFacts {
        recording_mbid: string(recording.get("id")),
        first_release_date: string(recording.get("first-release-date")),
        ..ReleaseFacts::default()
    };
    let releases = recording
        .get("releases")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let Some(release) = facts_release(query, releases) else {
        return Ok(Some(facts));
    };
    facts.release_mbid = string(release.get("id"));
    facts.release_title = string(release.get("title"));
    facts.release_date = string(release.get("date"));
    facts.country = string(release.get("country"));
    if facts.release_mbid.is_empty() {
        return Ok(Some(facts));
    }
    // A failed release lookup keeps what the search gave.
    let text = match fetch(
        agent()
            .get(&format!("{RELEASE_API}/{}", facts.release_mbid))
            .query("inc", "labels+recordings+isrcs")
            .query("fmt", "json"),
        None,
    ) {
        Ok(text) => text,
        Err(_) => return Ok(Some(facts)),
    };
    let Ok(release) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(Some(facts));
    };
    read_release(&mut facts, &release);
    Ok(Some(facts))
}

/// The first label with a real catalog number (MusicBrainz writes "[none]"
/// for a known absence), the barcode, and this recording's ISRC.
fn read_release(facts: &mut ReleaseFacts, release: &serde_json::Value) {
    let labels = release
        .get("label-info")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let catalog = |info: &serde_json::Value| {
        let number = string(info.get("catalog-number"));
        if number == "[none]" {
            String::new()
        } else {
            number
        }
    };
    let pick = labels
        .iter()
        .find(|info| !catalog(info).is_empty())
        .or_else(|| labels.first());
    if let Some(info) = pick {
        facts.label = string(info.get("label").and_then(|l| l.get("name")));
        facts.catalog_number = catalog(info);
    }
    facts.barcode = string(release.get("barcode"));
    if facts.release_date.is_empty() {
        facts.release_date = string(release.get("date"));
    }
    if facts.country.is_empty() {
        facts.country = string(release.get("country"));
    }
    let media = release
        .get("media")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for medium in media {
        let tracks = medium
            .get("tracks")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for track in tracks {
            let Some(recording) = track.get("recording") else {
                continue;
            };
            if string(recording.get("id")) != facts.recording_mbid {
                continue;
            }
            facts.isrc = recording
                .get("isrcs")
                .and_then(|v| v.as_array())
                .and_then(|list| list.first())
                .map(|v| string(Some(v)))
                .unwrap_or_default();
            return;
        }
    }
}

/// Official over bootleg or promo, then titled like the tag's album, then
/// earliest: a classic album's label, not a later compilation's.
fn facts_release<'a>(
    query: &TrackQuery,
    releases: &'a [serde_json::Value],
) -> Option<&'a serde_json::Value> {
    releases.iter().max_by(|a, b| {
        let rank = |r: &serde_json::Value| {
            let official = string(r.get("status")).eq_ignore_ascii_case("official");
            let album = if query.album.is_empty() {
                0.0
            } else {
                super::similarity(&query.album, &string(r.get("title")))
            };
            let date = string(r.get("date"));
            let earliness = if date.is_empty() {
                0.0
            } else {
                1.0 - date
                    .split('-')
                    .next()
                    .and_then(|y| y.parse::<f32>().ok())
                    .map_or(1.0, |y| (y / 3000.0).clamp(0.0, 1.0))
            };
            (official, album, earliness)
        };
        let (ao, aa, ae) = rank(a);
        let (bo, ba, be) = rank(b);
        ao.cmp(&bo)
            .then(aa.partial_cmp(&ba).unwrap_or(std::cmp::Ordering::Equal))
            .then(ae.partial_cmp(&be).unwrap_or(std::cmp::Ordering::Equal))
    })
}

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

/// Names joined by their join phrases ("Artist feat. Guest"). AcoustID serves
/// the same credit shape, so both providers share this.
pub(super) fn artist_credit(credit: Option<&serde_json::Value>) -> String {
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

/// The first credited artist's sort name ("Yonezu, Kenshi"): the sort tag
/// names the artist a row files under, not the whole "feat." chain.
fn credit_sort_name(credit: Option<&serde_json::Value>) -> String {
    credit
        .and_then(|v| v.as_array())
        .and_then(|array| array.first())
        .and_then(|part| part.get("artist"))
        .map(|artist| string(artist.get("sort-name")))
        .unwrap_or_default()
}

/// The Latin sort name MusicBrainz files an artist under, or None when
/// nothing is confidently the same artist. The bulk sort-name pass's whole
/// wire surface: it writes rox's own table, never a file, which is what makes
/// a bulk run legitimate under ADR 14.
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
            // Three: the top hit for a common name is often a different act.
            .query("limit", "3"),
        cancel,
    )?;
    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| LookupError::Other(e.to_string()))?;
    Ok(pick_sort_name(name, &body))
}

/// One throttled request, retried on a 503 (load shedding that usually
/// clears in seconds). Anything else fails on the first try.
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
                // Clamped both ways: a long hint would park a bulk pass, and
                // a short one lands back inside the burst that shed us.
                let wait = response
                    .header("retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .filter(|d| !d.is_zero())
                    .unwrap_or(BUSY_BACKOFF)
                    .clamp(BUSY_BACKOFF, BUSY_CEILING);
                wait_out(wait, cancel)?;
            }
            Err(ureq::Error::Status(404, _)) => return Err(LookupError::NotFound),
            Err(e) => return Err(LookupError::Other(net_reason(&e))),
        }
    }
}

fn cancelled(cancel: Cancel<'_>) -> bool {
    cancel.is_some_and(|stop| stop())
}

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

/// The artist whose name, or an alias, matches once folded. Aliases count
/// because MusicBrainz files many Japanese acts under a Latin name with the
/// native spelling as an alias.
///
/// Never fall back to the search score: it's relevance, not identity, and
/// Various Artists scores 100 at the top of a real artist's results. Filing
/// their rows under it can't be undone.
fn pick_sort_name(name: &str, body: &serde_json::Value) -> Option<String> {
    let artists = body.get("artists")?.as_array()?;
    let wanted = normalize_folded(name);
    let picked = artists
        .iter()
        .find(|a| normalize_folded(&string(a.get("name"))) == wanted || has_alias(a, &wanted))?;
    let sort = string(picked.get("sort-name"));
    (!sort.is_empty()).then_some(sort)
}

fn has_alias(artist: &serde_json::Value, wanted: &str) -> bool {
    let Some(aliases) = artist.get("aliases").and_then(|v| v.as_array()) else {
        return false;
    };
    aliases
        .iter()
        .any(|alias| normalize_folded(&string(alias.get("name"))) == wanted)
}

/// Escape the quote and backslash a title can hold, so tag text can't steer
/// the Lucene query.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Blocks until [`MIN_INTERVAL`] has passed: background executor only, never
/// the audio path.
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

    #[test]
    fn facts_prefer_the_official_release_titled_like_the_album() {
        let query = TrackQuery {
            artist: "The Beatles".into(),
            title: "Here Comes the Sun".into(),
            album: "Abbey Road".into(),
            duration_secs: None,
        };
        let releases = vec![
            serde_json::json!({"id": "boot", "title": "The Last Lost Album", "status": "Bootleg", "date": "1969-09"}),
            serde_json::json!({"id": "comp", "title": "1967-1970", "status": "Official", "date": "1973-04-02"}),
            serde_json::json!({"id": "abbey", "title": "Abbey Road", "status": "Official", "date": "1969-09-26"}),
            serde_json::json!({"id": "remaster", "title": "Abbey Road", "status": "Official", "date": "2019-09-27"}),
        ];
        let pick = facts_release(&query, &releases).unwrap();
        assert_eq!(string(pick.get("id")), "abbey");
    }

    #[test]
    fn release_body_fills_label_catalog_barcode_and_isrc() {
        let mut facts = ReleaseFacts {
            recording_mbid: "rec-1".into(),
            ..ReleaseFacts::default()
        };
        let release = serde_json::json!({
            "date": "1969-09-26",
            "country": "GB",
            "barcode": "0094638246817",
            "label-info": [
                {"catalog-number": "[none]", "label": {"name": "Parlophone"}},
                {"catalog-number": "PCS 7088", "label": {"name": "Apple Records"}}
            ],
            "media": [{"tracks": [
                {"number": "6", "recording": {"id": "rec-0", "isrcs": ["GBAYE0601690"]}},
                {"number": "7", "recording": {"id": "rec-1", "isrcs": ["GBAYE0601696"]}}
            ]}]
        });
        read_release(&mut facts, &release);
        assert_eq!(facts.label, "Apple Records");
        assert_eq!(facts.catalog_number, "PCS 7088");
        assert_eq!(facts.barcode, "0094638246817");
        assert_eq!(facts.isrc, "GBAYE0601696");
        assert_eq!(facts.release_date, "1969-09-26");
        assert_eq!(facts.country, "GB");
    }

    /// A trimmed live capture, cut to the keys `candidate` reads.
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

    /// A trimmed live capture; the second entry scores high but isn't the artist.
    const ARTIST_SEARCH: &str = r#"{
        "artists": [
            { "score": 100, "name": "米津玄師", "sort-name": "Yonezu, Kenshi" },
            { "score": 90, "name": "Kenshi Yonezu Tribute", "sort-name": "Tribute, Kenshi Yonezu" }
        ]
    }"#;

    const ARTIST_SECOND: &str = r#"{
        "artists": [
            { "score": 100, "name": "Various Artists", "sort-name": "Various Artists" },
            { "score": 99, "name": "崎山蒼志", "sort-name": "Sakiyama, Soushi" }
        ]
    }"#;

    const ARTIST_NO_MATCH: &str = r#"{
        "artists": [
            { "score": 62, "name": "Someone Else", "sort-name": "Else, Someone" }
        ]
    }"#;

    /// A perfect score on a name nobody asked for.
    const ARTIST_SCORED_STRANGER: &str = r#"{
        "artists": [
            { "score": 100, "name": "Various Artists", "sort-name": "Various Artists" },
            { "score": 88, "name": "Soundtrack", "sort-name": "Soundtrack" }
        ]
    }"#;

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
        assert_eq!(
            sort_name_from(ARTIST_SECOND, "崎山蒼志"),
            Some("Sakiyama, Soushi".to_string())
        );
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
        assert_eq!(
            sort_name_from(
                r#"{ "artists": [{ "score": 71, "name": "Beyonce", "sort-name": "Beyonce" }] }"#,
                "Beyoncé!"
            ),
            Some("Beyonce".to_string())
        );
        assert_eq!(sort_name_from(r#"{ "artists": [] }"#, "Nobody"), None);
        assert_eq!(sort_name_from(r#"{ "count": 0 }"#, "Nobody"), None);
        assert_eq!(
            sort_name_from(r#"{ "artists": [{ "score": 100, "name": "A" }] }"#, "A"),
            None
        );
    }

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
        assert_eq!(sort_name_from(ARTIST_ALIAS, "中島みゆき"), None);
    }

    #[test]
    fn a_cancelled_wait_returns_at_once() {
        let stop = || true;
        let started = Instant::now();
        assert_eq!(
            wait_out(Duration::from_secs(60), Some(&stop)),
            Err(LookupError::Cancelled)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(wait_out(Duration::from_millis(0), None), Ok(()));
    }

    /// A trimmed live capture of aed95205-f79f-4181-b2f7-2c2cb226f5bc with
    /// `inc=artist-credits+releases+media`.
    const BY_ID: &str = r#"{
        "id": "aed95205-f79f-4181-b2f7-2c2cb226f5bc",
        "title": "One More Time",
        "length": 320000,
        "artist-credit": [
            { "name": "Daft Punk", "joinphrase": "", "artist": { "name": "Daft Punk", "sort-name": "Daft Punk" } }
        ],
        "releases": [
            {
                "title": "Ultimate Collection",
                "date": "2001",
                "artist-credit": [
                    { "name": "Daft Punk", "joinphrase": "", "artist": { "name": "Daft Punk", "sort-name": "Daft Punk" } }
                ],
                "media": [
                    {
                        "position": 1,
                        "format": "CD",
                        "track-count": 17,
                        "track-offset": 3,
                        "tracks": [{ "number": "4", "title": "One More Time", "length": 320000 }]
                    }
                ]
            }
        ]
    }"#;

    #[test]
    fn the_by_id_shape_fills_the_release_numbers() {
        let query = TrackQuery {
            artist: "Daft Punk".to_string(),
            title: "One More Time".to_string(),
            album: "Ultimate Collection".to_string(),
            duration_secs: Some(320.0),
        };
        let recording: serde_json::Value = serde_json::from_str(BY_ID).expect("fixture parses");
        let candidate = recording_candidate(&query, &recording).expect("a recording");
        assert_eq!(candidate.title, "One More Time");
        assert_eq!(candidate.artist, "Daft Punk");
        assert_eq!(candidate.artist_sort, "Daft Punk");
        assert_eq!(candidate.album, "Ultimate Collection");
        assert_eq!(candidate.album_artist, "Daft Punk");
        assert_eq!(candidate.year, "2001");
        assert_eq!(candidate.track_no, "4");
        assert_eq!(candidate.disc_no, "1");
        assert_eq!(candidate.duration_secs, Some(320.0));
        assert!(candidate.confidence > 0.9);
    }

    #[test]
    fn a_body_that_is_not_a_recording_is_no_answer() {
        let query = query();
        for body in [r#"{ "error": "Not Found" }"#, r#"{ "title": "" }"#, "{}"] {
            let value: serde_json::Value = serde_json::from_str(body).expect("fixture parses");
            assert!(recording_candidate(&query, &value).is_none());
        }
    }

    #[test]
    fn a_recording_with_no_release_still_parses() {
        let candidate = parse(r#"{ "title": "Lemon" }"#);
        assert!(candidate.artist_sort.is_empty());
        assert!(candidate.album_artist_sort.is_empty());
        assert!(candidate.album.is_empty());
    }
}
