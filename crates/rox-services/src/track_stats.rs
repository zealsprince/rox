//! The metadata panel's Last.fm track counts, cached as one JSON per track
//! and username under the artists folder's `tracks/`. Tags fall back from the
//! track to its album to its artist, since Last.fm holds none for many
//! tracks. A failed fetch serves the stale copy. Blocking.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use rox_core::settings::artists_dir;
use rox_net::providers::{self, lastfm::TrackStats};

/// Counts move faster than a bio, so a week rather than the artist store's month.
const TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// `stats: None` records a miss, so it doesn't re-query on every open.
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

/// `tags_scope` records where the tags came from so the sheet can say so.
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

/// `album` is the tag's, the first place the tag fallback asks. `username`
/// adds the user's own counts and keys the entry. Blocking.
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
    // An entry with empty tags and no scope predates the fallback: stale
    // however young. A settled miss says "none" and stays fresh.
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
        Err(e) => match cached.and_then(|entry| entry.stats) {
            Some(stats) => Ok(Some(stats)),
            None => Err(e),
        },
    }
}
