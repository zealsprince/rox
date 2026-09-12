//! AcoustID (acoustid.org): what a track is, decided from the sound instead
//! of from what the tags claim. A Chromaprint fingerprint goes up and
//! MusicBrainz recording ids come back, which is the one thing no other
//! provider here can do. A file with an empty title and a misspelled artist
//! gives a text search nothing to match on, and its audio still matches.
//!
//! The lookup takes an application key, registered once at
//! acoustid.org/new-application and baked in from `ACOUSTID_CLIENT_KEY`. It
//! names rox and nothing else, the Last.fm pair's trade; a build without one
//! asks the user for their own on the settings page instead. No user key and
//! no account enter into it, because rox only reads. Submitting fingerprints
//! back to the database is the other half of the service and the half that
//! would need one.
//!
//! AcoustID asks for no more than three requests a second, held process-wide
//! here so callers never have to count them. What this module returns is ids
//! and a score. It writes no file and asks nothing on its own: the identify
//! runs when a panel action asks for it, per ADR 14.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rox_core::settings::Settings;

use super::{agent, net_reason, string};

const API: &str = "https://api.acoustid.org/v2/lookup";

/// AcoustID's published limit is three requests a second, so this is the
/// gap between two of them with a little room over. One identify sends one
/// request and never notices; a pass over a selection would sail past the
/// limit, so the gate lives here rather than being trusted to the caller.
const MIN_INTERVAL: Duration = Duration::from_millis(350);

/// The application key this build was compiled with, empty when it was
/// built without one. `ACOUSTID_CLIENT_KEY` is how the release workflow
/// hands the repository secret to cargo, the shape every baked identity
/// takes.
pub const CLIENT_KEY: &str = match option_env!("ACOUSTID_CLIENT_KEY") {
    Some(key) => key,
    None => "",
};

/// The key a lookup calls with: the settings override when the user entered
/// one, the build's own otherwise, the order the Last.fm reads use. Empty
/// when neither exists, which reads as the identify being unavailable rather
/// than as an error.
pub fn client_key() -> String {
    let key = Settings::load().accounts.providers.acoustid_key;
    if key.is_empty() {
        CLIENT_KEY.to_string()
    } else {
        key
    }
}

/// One recording AcoustID matched the fingerprint to: the MusicBrainz id
/// that turns into a full candidate, how sure the match is, and enough of a
/// name to show while the tags are still being fetched.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// A MusicBrainz recording MBID.
    pub recording_id: String,
    /// AcoustID's own score, 0 to 1. This scores the audio, not the tags,
    /// which is why it stands in for the text scorer's confidence once a
    /// candidate is built.
    pub score: f32,
    pub title: String,
    /// The credited artists joined the way MusicBrainz credits read, since
    /// that is where AcoustID's copy of them comes from.
    pub artist: String,
}

/// One lookup: a fingerprint and the file's whole duration in, the
/// recordings that match out, best first. An empty vec is a clean no-match,
/// which is the ordinary answer for anything the database has never seen.
/// Err is the wire, a missing key, or a message the API sent back.
///
/// Blocking, background executor only.
pub fn lookup(fingerprint: &str, duration_secs: u32) -> Result<Vec<Hit>, String> {
    let key = client_key();
    if key.is_empty() {
        return Err("no AcoustID application key".to_string());
    }
    let fingerprint = fingerprint.trim();
    if fingerprint.is_empty() || duration_secs == 0 {
        // Both are required parameters, so sending this would spend a
        // request to be told what's already known here.
        return Err("the fingerprint is incomplete".to_string());
    }
    let duration = duration_secs.to_string();
    throttle();
    // POST with a form body, not a query string: two minutes of audio
    // encodes to well over a kilobyte of base64, and the service documents
    // the POST as the preferred shape for exactly that reason. The key
    // rides in the body either way, so ureq's Display, which prints the
    // URL, has nothing to leak even before [`net_reason`] catches it.
    let sent = agent().post(API).send_form(&[
        ("client", key.as_str()),
        ("duration", duration.as_str()),
        ("fingerprint", fingerprint),
        // Asking for the recordings rather than bare ids: the title and
        // artist ride along at no extra request and give a picker
        // something to name each hit by before its tags are fetched.
        ("meta", "recordings"),
        ("format", "json"),
    ]);
    // An API error arrives as a 400 carrying the envelope in its body, so
    // a status failure is read like a success and the message pulled out
    // of it, rather than folded down to a bare code.
    let text = match sent {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(net_reason(&e)),
    };
    parse(&text)
}

/// The response body into hits, best first. Its own function so the fixtures
/// below exercise the shapes the service actually sends without a key or a
/// network.
fn parse(text: &str) -> Result<Vec<Hit>, String> {
    let body: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if string(body.get("status")) == "error" {
        let message = body
            .get("error")
            .map(|error| string(error.get("message")))
            .unwrap_or_default();
        return Err(if message.is_empty() {
            "unknown api error".to_string()
        } else {
            message
        });
    }
    let Some(results) = body.get("results").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let mut hits = Vec::new();
    for result in results {
        let score = result.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        // A result is a fingerprint cluster, and a cluster nobody has ever
        // tagged carries no recordings at all. That's a match with nothing
        // to say, which is worth skipping and not worth failing over.
        let Some(recordings) = result.get("recordings").and_then(|v| v.as_array()) else {
            continue;
        };
        for recording in recordings {
            let recording_id = string(recording.get("id"));
            if recording_id.is_empty() {
                continue;
            }
            hits.push(Hit {
                recording_id,
                score,
                title: string(recording.get("title")),
                // The same joiner the MusicBrainz provider runs credits
                // through. AcoustID serves the credit straight out of its
                // MusicBrainz mirror, names and join phrases in credit
                // order, so the two read alike in a compare table.
                artist: super::musicbrainz::artist_credit(recording.get("artists")),
            });
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Two clusters can name the same recording, and each one past the first
    // buys a duplicate row in the picker and a MusicBrainz request behind
    // the one-a-second throttle. The sort is stable, so the copy kept is
    // the one that scored highest.
    let mut seen = HashSet::new();
    hits.retain(|hit| seen.insert(hit.recording_id.clone()));
    Ok(hits)
}

/// Hold the process to AcoustID's rate limit: if the last request was under
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

    /// The documented response shape for `meta=recordings`, with a second
    /// cluster added below the first so the ordering has something to do.
    const LOOKUP: &str = r#"{
        "status": "ok",
        "results": [
            {
                "id": "9ff43b6a-4f16-427c-93c2-92307ca505e0",
                "score": 0.72,
                "recordings": [
                    {
                        "id": "cd2e7c47-16f5-46c6-a37c-a1eb7bf599ff",
                        "title": "Lower Your Eyelids to Die With the Sun",
                        "duration": 639,
                        "artists": [{ "id": "6d7b7cd4", "name": "M83" }]
                    }
                ]
            },
            {
                "id": "1a1a1a1a-0000-0000-0000-000000000000",
                "score": 0.98,
                "recordings": [
                    {
                        "id": "38035858-f990-4fbb-b3b2-f2f8b958eeba",
                        "title": "Teen Angst",
                        "artists": [
                            { "id": "aaa", "name": "M83", "joinphrase": " feat. " },
                            { "id": "bbb", "name": "Morgan Kibby" }
                        ]
                    }
                ]
            }
        ]
    }"#;

    #[test]
    fn hits_come_back_best_first() {
        let hits = parse(LOOKUP).expect("fixture parses");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].recording_id, "38035858-f990-4fbb-b3b2-f2f8b958eeba");
        assert_eq!(hits[0].title, "Teen Angst");
        // The join phrase comes off the credit, so a featured artist reads
        // the way the tag would spell it.
        assert_eq!(hits[0].artist, "M83 feat. Morgan Kibby");
        assert!((hits[0].score - 0.98).abs() < 1e-6);
        assert_eq!(hits[1].artist, "M83");
    }

    /// A fingerprint cluster nobody has tagged yet answers with a score and
    /// no recordings. Nothing to show, and no reason to fail the lookup.
    #[test]
    fn a_result_without_recordings_is_skipped() {
        let hits = parse(
            r#"{
                "status": "ok",
                "results": [
                    { "id": "9ff43b6a", "score": 1.0 },
                    {
                        "id": "1a1a1a1a",
                        "score": 0.4,
                        "recordings": [{ "id": "cd2e7c47", "title": "Known" }]
                    }
                ]
            }"#,
        )
        .expect("fixture parses");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Known");
        // A recording with no id of its own is no use either: the whole
        // point of a hit is the id the MusicBrainz fetch runs on.
        assert!(parse(
            r#"{ "status": "ok", "results": [{ "score": 1.0, "recordings": [{ "title": "x" }] }] }"#
        )
        .expect("fixture parses")
        .is_empty());
    }

    /// The envelope the service really sends on a bad key, captured from a
    /// live call with a made-up one. It arrives with a 400, which is why
    /// the lookup reads the body of a status failure instead of folding it
    /// to a code.
    #[test]
    fn an_error_envelope_carries_the_api_message() {
        assert_eq!(
            parse(r#"{"error": {"code": 4, "message": "invalid API key"}, "status": "error"}"#),
            Err("invalid API key".to_string())
        );
        // An error with nothing said about it still fails, rather than
        // reading as a clean no-match.
        assert_eq!(
            parse(r#"{ "status": "error" }"#),
            Err("unknown api error".to_string())
        );
    }

    #[test]
    fn nothing_matched_is_an_empty_list() {
        assert_eq!(
            parse(r#"{ "status": "ok", "results": [] }"#),
            Ok(Vec::new())
        );
        // No results key at all reads the same way.
        assert_eq!(parse(r#"{ "status": "ok" }"#), Ok(Vec::new()));
        // A body that isn't JSON is the one shape that fails here.
        assert!(parse("<html>").is_err());
    }

    /// The same recording under two clusters is one hit at the better
    /// score, so the picker shows it once and MusicBrainz is asked once.
    #[test]
    fn a_repeated_recording_is_kept_once() {
        let hits = parse(
            r#"{
                "status": "ok",
                "results": [
                    { "score": 0.3, "recordings": [{ "id": "same", "title": "Once" }] },
                    { "score": 0.9, "recordings": [{ "id": "same", "title": "Once" }] }
                ]
            }"#,
        )
        .expect("fixture parses");
        assert_eq!(hits.len(), 1);
        assert!((hits[0].score - 0.9).abs() < 1e-6);
    }
}
