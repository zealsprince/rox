//! Deezer (api.deezer.com): keyless album search that hands back cover
//! URLs at fixed sizes. `cover_big` is the 500px preview, `cover_xl` the
//! 1000px image a save embeds.

use super::{agent, string, ArtCandidate, ArtProvider, TrackQuery};

const API: &str = "https://api.deezer.com/search/album";

/// The pixel size Deezer's `cover_xl` serves; used for the caption and
/// the quality sort.
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
            // The 500px cover is the preview; fall back to the xl when a
            // result omits it.
            let thumb = {
                let big = string(album.get("cover_big"));
                if big.is_empty() {
                    full.clone()
                } else {
                    big
                }
            };
            out.push(ArtCandidate {
                provider: self.name(),
                album: string(album.get("title")),
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

/// Search Deezer for an artist's portrait: the xl picture URL of the
/// best name match, None when nothing matches. The name has to match
/// once folded - a search for an unknown act returns lookalikes, and a
/// wrong face is worse than none.
pub fn artist_picture(name: &str) -> Result<Option<String>, String> {
    if name.trim().is_empty() {
        return Ok(None);
    }
    let text = agent()
        .get(ARTIST_API)
        .query("q", name.trim())
        .query("limit", "8")
        .call()
        .map_err(|e| e.to_string())?
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
        // An artist without a photo still gets a URL, pointing at the
        // placeholder star; its empty id reads as a doubled slash.
        if full.is_empty() || full.contains("/artist//") {
            continue;
        }
        return Ok(Some(full));
    }
    Ok(None)
}
