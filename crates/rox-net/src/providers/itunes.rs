//! iTunes Search (itunes.apple.com): keyless album search. The artwork URL
//! ends `100x100bb.jpg` and serves any size you rewrite that to.

use super::{ArtCandidate, ArtProvider, TrackQuery, agent, net_reason, string};

const API: &str = "https://itunes.apple.com/search";

const THUMB_PX: u32 = 256;
const FULL_PX: u32 = 1000;

pub struct Itunes;

impl ArtProvider for Itunes {
    fn name(&self) -> &'static str {
        "itunes"
    }

    fn search(&self, query: &TrackQuery) -> Result<Vec<ArtCandidate>, String> {
        // Fall back to the title so a single still finds its cover.
        let subject = if query.album.is_empty() {
            &query.title
        } else {
            &query.album
        };
        let term = format!("{} {}", query.artist, subject);
        if term.trim().is_empty() {
            return Ok(Vec::new());
        }
        let text = agent()
            .get(API)
            .query("term", term.trim())
            .query("entity", "album")
            .query("limit", "8")
            .call()
            .map_err(|e| net_reason(&e))?
            .into_string()
            .map_err(|e| e.to_string())?;
        let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let Some(results) = body.get("results").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(results.len());
        for result in results {
            let Some(art100) = result.get("artworkUrl100").and_then(|v| v.as_str()) else {
                continue;
            };
            out.push(ArtCandidate {
                provider: self.name(),
                album: string(result.get("collectionName")),
                artist: string(result.get("artistName")),
                thumb_url: resize(art100, THUMB_PX),
                full_url: resize(art100, FULL_PX),
                width: FULL_PX,
                height: FULL_PX,
            });
        }
        Ok(out)
    }
}

fn resize(url: &str, px: u32) -> String {
    url.replace("100x100bb", &format!("{px}x{px}bb"))
}
