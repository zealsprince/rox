//! LRCLIB (lrclib.net): keyless synced and plain lyrics matched by the
//! track's own tags. The search endpoint returns up to twenty candidates
//! by artist and title, each carrying its own tags and both lyric forms,
//! so the picker has a ranked list rather than one guess. Album is left
//! off the query on purpose: it narrows the server's match, and a
//! mistagged album would drop good candidates the confidence score can
//! sort out instead.

use super::{agent, LyricsCandidate, LyricsProvider, TrackQuery};

const API: &str = "https://lrclib.net/api/search";

pub struct Lrclib;

impl LyricsProvider for Lrclib {
    fn name(&self) -> &'static str {
        "lrclib"
    }

    fn search(&self, query: &TrackQuery) -> Result<Vec<LyricsCandidate>, String> {
        let text = agent()
            .get(API)
            .query("artist_name", &query.artist)
            .query("track_name", &query.title)
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())?;
        let results: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let Some(array) = results.as_array() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(array.len());
        for item in array {
            let str_field = |name: &str| {
                item.get(name)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let synced = str_field("syncedLyrics");
            // An instrumental or a metadata-only row carries no text worth
            // saving; skip it rather than offer an empty sheet.
            let Some((text, is_synced)) = synced
                .map(|s| (s, true))
                .or_else(|| str_field("plainLyrics").map(|s| (s, false)))
            else {
                continue;
            };
            out.push(LyricsCandidate {
                provider: self.name(),
                artist: str_field("artistName").unwrap_or_default(),
                title: str_field("trackName").unwrap_or_default(),
                album: str_field("albumName").unwrap_or_default(),
                duration_secs: item.get("duration").and_then(|v| v.as_f64()),
                synced: is_synced,
                text,
                confidence: 0.0,
            });
        }
        Ok(out)
    }
}
