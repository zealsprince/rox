//! The metadata panel's MusicBrainz release facts, cached as one JSON per
//! track under the artists folder's `releases/`, so the artist cache's clear
//! takes them too. A failed fetch serves the stale copy. Blocking, and the
//! provider's one-a-second throttle applies: a lookup is two calls.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use rox_core::settings::artists_dir;
use rox_net::providers::musicbrainz::ReleaseFacts;
use rox_net::providers::{self, TrackQuery};

const TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// `facts: None` records a miss, so it doesn't re-query on every open.
#[derive(Serialize, Deserialize)]
struct Entry {
    fetched: u64,
    facts: Option<ReleaseFacts>,
}

fn file_for(query: &TrackQuery) -> PathBuf {
    let key = format!(
        "{}\n{}\n{}",
        providers::normalize(&query.artist),
        providers::normalize(&query.title),
        providers::normalize(&query.album)
    );
    let hash = rox_library::hash::fnv1a(key.as_bytes());
    artists_dir()
        .join("releases")
        .join(format!("{hash:016x}.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// With the metadata provider off, the cache serves at any age. Blocking.
pub fn get(query: &TrackQuery) -> Result<Option<ReleaseFacts>, String> {
    if query.artist.trim().is_empty() || query.title.trim().is_empty() {
        return Ok(None);
    }
    let file = file_for(query);
    let cached: Option<Entry> = fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let fresh = cached
        .as_ref()
        .is_some_and(|entry| now().saturating_sub(entry.fetched) < TTL_SECS);
    if !providers::metadata_online() || fresh {
        return Ok(cached.and_then(|entry| entry.facts));
    }
    match providers::musicbrainz::release_facts(query) {
        Ok(facts) => {
            let entry = Entry {
                fetched: now(),
                facts,
            };
            if let Some(dir) = file.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Ok(text) = serde_json::to_string(&entry) {
                let _ = fs::write(&file, text);
            }
            Ok(entry.facts)
        }
        Err(e) => match cached.and_then(|entry| entry.facts) {
            Some(facts) => Ok(Some(facts)),
            None => Err(e),
        },
    }
}
