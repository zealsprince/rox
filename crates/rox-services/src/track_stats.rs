//! The track stats store: the metadata panel's Last.fm rows. What
//! Last.fm counts for a track (listeners, plays, the user's own plays,
//! tags), fetched once and kept as one JSON per track under the artists
//! folder's `tracks` subfolder, so a shown track reads offline from then
//! on and the artist cache's clear takes these with it. Keyed on a
//! stable hash of the folded artist and title plus the asking username,
//! the artist store's naming move; an entry refreshes once it ages past
//! [`TTL_SECS`], and a fetch that fails with a copy on disk serves the
//! copy rather than nothing. Tags fall back from the track to its album
//! to its artist, since Last.fm holds none for many tracks. Blocking,
//! background executor only, like the provider it calls.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use rox_core::settings::artists_dir;
use rox_net::providers::{self, lastfm::TrackStats};

/// How long a cached entry serves before a fetch refreshes it. Counts
/// move faster than a bio, so a week rather than the artist store's
/// month; still long enough that following the playing track costs one
/// call per track per week.
const TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// The cache file's shape: when the fetch happened and what it found.
/// None inside records Last.fm having no such track, so a miss doesn't
/// re-query on every panel open.
#[derive(Serialize, Deserialize)]
struct Entry {
    fetched: u64,
    stats: Option<TrackStats>,
}

fn file_for(artist: &str, title: &str, username: &str) -> PathBuf {
    let key = format!(
        "{}\n{}\n{}",
        providers::normalize(artist),
        providers::normalize(title),
        username.trim().to_lowercase()
    );
    let hash = rox_library::hash::fnv1a(key.as_bytes());
    artists_dir()
        .join("tracks")
        .join(format!("{hash:016x}.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The tag fallback: a track without tags of its own takes its album's,
/// then its artist's, each marked in `tags_scope` so the sheet can say
/// so. The album asked for is the tag's, then Last.fm's own filing.
fn fill_tags(stats: &mut TrackStats, artist: &str, album: &str) {
    if !stats.tags.is_empty() {
        return;
    }
    for candidate in [album.trim(), stats.album.trim()] {
        if candidate.is_empty() {
            continue;
        }
        let tags = providers::lastfm::album_tags(artist, candidate);
        if !tags.is_empty() {
            stats.tags = tags;
            stats.tags_scope = "album".to_string();
            return;
        }
    }
    let tags = providers::lastfm::artist_tags(artist);
    if !tags.is_empty() {
        stats.tags = tags;
        stats.tags_scope = "artist".to_string();
    } else {
        stats.tags_scope = "none".to_string();
    }
}

/// The counts for a track, cache first: a fresh entry is served from
/// disk, a stale or missing one fetches and rewrites it, and with the
/// artist provider off the cache is served at any age. `album` is the
/// tag's, the first place the tag fallback asks when Last.fm files the
/// track under none; `username` adds the user's own counts and keys the
/// entry, so accounts never share one. Ok(None) is a clean miss: Last.fm
/// has no such track, or nothing is cached to serve offline. Blocking,
/// background executor only.
pub fn get(
    artist: &str,
    title: &str,
    album: &str,
    username: &str,
) -> Result<Option<TrackStats>, String> {
    let (artist, title) = (artist.trim(), title.trim());
    if artist.is_empty() || title.is_empty() {
        return Ok(None);
    }
    let file = file_for(artist, title, username);
    let cached: Option<Entry> = fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    // An entry from before the tag fallback existed has empty tags and no
    // scope; it's stale however young, so the fallback gets its one look.
    // A settled miss says "none" in the scope and stays fresh.
    let fresh = cached.as_ref().is_some_and(|entry| {
        now().saturating_sub(entry.fetched) < TTL_SECS
            && entry
                .stats
                .as_ref()
                .is_none_or(|stats| !stats.tags.is_empty() || !stats.tags_scope.is_empty())
    });
    if !providers::artist_online() || fresh {
        return Ok(cached.and_then(|entry| entry.stats));
    }
    let username = Some(username.trim()).filter(|u| !u.is_empty());
    match providers::lastfm::track_info(artist, title, username) {
        Ok(mut stats) => {
            if let Some(stats) = stats.as_mut() {
                fill_tags(stats, artist, album);
            }
            let entry = Entry {
                fetched: now(),
                stats,
            };
            if let Some(dir) = file.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Ok(text) = serde_json::to_string(&entry) {
                let _ = fs::write(&file, text);
            }
            Ok(entry.stats)
        }
        // The network failing with a copy on disk serves the copy; its
        // age beats an empty row.
        Err(e) => match cached.and_then(|entry| entry.stats) {
            Some(stats) => Ok(Some(stats)),
            None => Err(e),
        },
    }
}
