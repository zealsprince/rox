//! AcoustID (acoustid.org): identify a track from its Chromaprint
//! fingerprint, the one lookup that doesn't trust the tags. The application
//! key is baked in from `ACOUSTID_CLIENT_KEY` or entered by the user; rox
//! only reads, so no user key is involved. Runs only on a panel action, per
//! ADR 14.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rox_core::settings::Settings;

use super::{agent, net_reason, string};

const API: &str = "https://api.acoustid.org/v2/lookup";

/// AcoustID's published limit is three requests a second. Enforced here
/// rather than trusted to callers, since a pass over a selection would blow it.
const MIN_INTERVAL: Duration = Duration::from_millis(350);

pub const CLIENT_KEY: &str = match option_env!("ACOUSTID_CLIENT_KEY") {
    Some(key) => key,
    None => "",
};

/// The settings override, else the build's key. Empty means the identify is
/// unavailable, not an error.
pub fn client_key() -> String {
    let key = Settings::load().accounts.providers.acoustid_key;
    if key.is_empty() {
        CLIENT_KEY.to_string()
    } else {
        key
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub recording_id: String,
    /// 0 to 1, scoring the audio, so it replaces the text scorer's confidence.
    pub score: f32,
    pub title: String,
    pub artist: String,
}

/// Hits best first; an empty vec is the ordinary no-match.
pub fn lookup(fingerprint: &str, duration_secs: u32) -> Result<Vec<Hit>, String> {
    let key = client_key();
    if key.is_empty() {
        return Err("no AcoustID application key".to_string());
    }
    let fingerprint = fingerprint.trim();
    if fingerprint.is_empty() || duration_secs == 0 {
        return Err("the fingerprint is incomplete".to_string());
    }
    let duration = duration_secs.to_string();
    throttle();
    // POST, not a query string: two minutes of audio is over a kilobyte of
    // base64, and it keeps the key out of the URL.
    let sent = agent().post(API).send_form(&[
        ("client", key.as_str()),
        ("duration", duration.as_str()),
        ("fingerprint", fingerprint),
        // Recordings carry title and artist, so a picker can name each hit early.
        ("meta", "recordings"),
        ("format", "json"),
    ]);
    // An API error is a 400 with the message in its body, so read the body.
    let text = match sent {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(net_reason(&e)),
    };
    parse(&text)
}

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
        // A cluster nobody has tagged carries no recordings: skip it.
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
                // Same joiner as MusicBrainz, so the two read alike in a compare.
                artist: super::musicbrainz::artist_credit(recording.get("artists")),
            });
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Two clusters can name one recording. The sort is stable, so the copy
    // kept is the highest-scoring one.
    let mut seen = HashSet::new();
    hits.retain(|hit| seen.insert(hit.recording_id.clone()));
    Ok(hits)
}

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
        assert_eq!(hits[0].artist, "M83 feat. Morgan Kibby");
        assert!((hits[0].score - 0.98).abs() < 1e-6);
        assert_eq!(hits[1].artist, "M83");
    }

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
        assert!(parse(
            r#"{ "status": "ok", "results": [{ "score": 1.0, "recordings": [{ "title": "x" }] }] }"#
        )
        .expect("fixture parses")
        .is_empty());
    }

    /// The real envelope for a bad key, captured live. It arrives with a 400.
    #[test]
    fn an_error_envelope_carries_the_api_message() {
        assert_eq!(
            parse(r#"{"error": {"code": 4, "message": "invalid API key"}, "status": "error"}"#),
            Err("invalid API key".to_string())
        );
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
        assert_eq!(parse(r#"{ "status": "ok" }"#), Ok(Vec::new()));
        assert!(parse("<html>").is_err());
    }

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
