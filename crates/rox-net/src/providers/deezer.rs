//! Deezer (api.deezer.com): keyless album search with fixed-size cover URLs.

use super::{ArtCandidate, ArtProvider, TrackQuery, agent, net_reason, string};

const API: &str = "https://api.deezer.com/search/album";

const XL_PX: u32 = 1000;

pub struct Deezer;

impl ArtProvider for Deezer {
    fn name(&self) -> &'static str {
        "deezer"
    }

    fn search(&self, query: &TrackQuery) -> Result<Vec<ArtCandidate>, String> {
        let subject = if query.album.is_empty() {
            &query.title
        } else {
            &query.album
        };
        let q = format!("{} {}", query.artist, subject);
        if q.trim().is_empty() {
            return Ok(Vec::new());
        }
        let text = agent()
            .get(API)
            .query("q", q.trim())
            .query("limit", "8")
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())?;
        let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let Some(data) = body.get("data").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(data.len());
        for album in data {
            let full = string(album.get("cover_xl"));
            if full.is_empty() {
                continue;
            }
            let thumb = {
                let big = string(album.get("cover_big"));
                if big.is_empty() { full.clone() } else { big }
            };
            out.push(ArtCandidate {
                provider: self.name(),
                album: string(album.get("title")),
                artist: string(album.get("artist").and_then(|a| a.get("name"))),
                thumb_url: thumb,
                full_url: full,
                width: XL_PX,
                height: XL_PX,
            });
        }
        Ok(out)
    }
}

const ARTIST_API: &str = "https://api.deezer.com/search/artist";

/// The xl portrait of an exact (folded) name match. Unknown acts return
/// lookalikes, and a wrong face is worse than none.
pub fn artist_picture(name: &str) -> Result<Option<String>, String> {
    if name.trim().is_empty() {
        return Ok(None);
    }
    let text = agent()
        .get(ARTIST_API)
        .query("q", name.trim())
        .query("limit", "8")
        .call()
        .map_err(|e| net_reason(&e))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let Some(data) = body.get("data").and_then(|v| v.as_array()) else {
        return Ok(None);
    };
    let folded = super::normalize(name);
    for artist in data {
        if super::normalize(&string(artist.get("name"))) != folded {
            continue;
        }
        let full = {
            let xl = string(artist.get("picture_xl"));
            if xl.is_empty() {
                string(artist.get("picture_big"))
            } else {
                xl
            }
        };
        // A photoless artist still gets a URL, to the placeholder star; its
        // empty id shows as a doubled slash.
        if full.is_empty() || full.contains("/artist//") {
            continue;
        }
        return Ok(Some(full));
    }
    Ok(None)
}
