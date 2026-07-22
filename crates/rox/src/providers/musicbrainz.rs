//! MusicBrainz (musicbrainz.org): keyless release metadata, matched by a
//! recording search over the track's artist and title. Each recording
//! carries the tags a tagger fills - title, artist, and, through its best
//! matching release, album, album artist, year, track, and disc - so the
//! compare has real candidates to set from.
//!
//! The service caps clients at one request a second and refuses anything
//! without a contactable User-Agent (ADR 14: the shared agent carries
//! it). The throttle lives here so callers never see it, the rate limit
//! held process-wide against the next request.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{agent, string, MetadataCandidate, MetadataProvider, TrackQuery};

const API: &str = "https://musicbrainz.org/ws/2/recording";

/// MusicBrainz's rate limit: one request a second, sustained. A single
/// lookup never hits it, but a batch would, so the gate is here rather
/// than trusted to the caller.
const MIN_INTERVAL: Duration = Duration::from_millis(1100);

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
        throttle();
        let text = agent()
            .get(API)
            .query("query", &lucene)
            .query("fmt", "json")
            .query("limit", "10")
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())?;
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
    let duration_secs = recording
        .get("length")
        .and_then(|v| v.as_f64())
        .map(|ms| ms / 1000.0);

    let release = recording
        .get("releases")
        .and_then(|v| v.as_array())
        .and_then(|releases| best_release(query, releases));

    let (album, album_artist, year, track_no, disc_no) = match release {
        Some(release) => {
            let album = string(release.get("title"));
            let album_artist = artist_credit(release.get("artist-credit"));
            let year = string(release.get("date"))
                .split('-')
                .next()
                .unwrap_or("")
                .to_string();
            // The disc and track come off the media block the recording
            // sits on: the disc is the medium's position, the track its
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
            (album, album_artist, year, track_no, disc_no)
        }
        None => (
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
        year,
        track_no,
        disc_no,
        duration_secs,
        confidence: 0.0,
    }
}

/// The release whose title best matches the query album, so the candidate
/// carries the numbers for the album the track claims. Falls back to the
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
/// carries.
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
