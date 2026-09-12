//! The release facts store: the metadata panel's MusicBrainz rows. What
//! MusicBrainz records about a track's release (label, catalog number,
//! country, dates, ISRC), fetched once and kept as one JSON per track
//! under the artists folder's `releases` subfolder, so a shown track
//! reads offline from then on and the artist cache's clear takes these
//! with it. Keyed on a stable hash of the folded artist, title, and
//! album; an entry refreshes once it ages past [`TTL_SECS`], and a fetch
//! that fails with a copy on disk serves the copy rather than nothing.
//! Blocking, background executor only, like the provider it calls, and
//! the provider's one-a-second throttle applies: a lookup is two calls.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use rox_core::settings::artists_dir;
use rox_net::providers::musicbrainz::ReleaseFacts;
use rox_net::providers::{self, TrackQuery};

/// How long a cached entry serves before a fetch refreshes it. Release
/// facts barely move, so the artist store's month.
const TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// The cache file's shape: when the fetch happened and what it found.
/// None inside records MusicBrainz having no such recording, so a miss
/// doesn't re-query on every panel open.
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

/// The release facts for a track, cache first: a fresh entry is served
/// from disk, a stale or missing one fetches and rewrites it, and with
/// the metadata provider off the cache is served at any age. Ok(None) is
/// a clean miss: MusicBrainz has no such recording, or nothing is cached
/// to serve offline. Blocking, background executor only.
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
        // The network failing with a copy on disk serves the copy; its
        // age beats an empty row.
        Err(e) => match cached.and_then(|entry| entry.facts) {
            Some(facts) => Ok(Some(facts)),
            None => Err(e),
        },
    }
}
