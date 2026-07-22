//! iTunes Search (itunes.apple.com): keyless album search whose artwork
//! URL rewrites to any size. The search returns a 100px thumbnail URL
//! ending `100x100bb.jpg`; swapping the dimensions in that name serves the
//! preview and a large image off the same source, so one result yields
//! both without a second lookup.

use super::{agent, string, ArtCandidate, ArtProvider, TrackQuery};

const API: &str = "https://itunes.apple.com/search";

/// The sizes we rewrite the artwork URL to: a crisp grid preview and a
/// large image for the embed. iTunes serves whatever is asked.
const THUMB_PX: u32 = 256;
const FULL_PX: u32 = 1000;

pub struct Itunes;

impl ArtProvider for Itunes {
    fn name(&self) -> &'static str {
        "itunes"
    }

    fn search(&self, query: &TrackQuery) -> Result<Vec<ArtCandidate>, String> {
        // Album search wants the album name; fall back to the title when a
        // track carries no album, so a single still finds its cover.
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
            .map_err(|e| e.to_string())?
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
                thumb_url: resize(art100, THUMB_PX),
                full_url: resize(art100, FULL_PX),
                width: FULL_PX,
                height: FULL_PX,
            });
        }
        Ok(out)
    }
}

/// Rewrite the `100x100bb.jpg` tail of an artwork URL to a square of
/// `px`. Leaves anything that does not carry that marker alone, so an
/// unexpected URL shape degrades to itself rather than a broken link.
fn resize(url: &str, px: u32) -> String {
    url.replace("100x100bb", &format!("{px}x{px}bb"))
}
