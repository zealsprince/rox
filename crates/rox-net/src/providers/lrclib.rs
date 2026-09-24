//! LRCLIB (lrclib.net): keyless synced and plain lyrics by artist and
//! title. Album stays off the query: a mistagged one would drop good
//! candidates the confidence score can rank instead.

use super::{LyricsCandidate, LyricsProvider, TrackQuery, agent, net_reason};

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
            .map_err(|e| net_reason(&e))?
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
            // Skip instrumental and metadata-only rows.
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
