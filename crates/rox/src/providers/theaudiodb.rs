//! TheAudioDB (theaudiodb.com): the wide artist art the biography panel
//! wants that last.fm and deezer don't carry - a banner for the header
//! and a fanart background behind the text. One keyless search by name
//! returns both as URLs; the store downloads them beside the deezer
//! portrait. Blocking, background executor only, like the other
//! providers.
//!
//! The key below is TheAudioDB's public test key. It answers the search
//! and image endpoints at a low rate, which is all a per-artist lookup
//! needs; a fork leaning on it harder registers for a supporter key and
//! drops it in here, the last.fm identity's trade-off ([`crate::lastfm::keys`]).

use super::{agent, normalize, string};

/// TheAudioDB's public test key. Enough for the biography panel's
/// one-artist-at-a-time lookups; swap in a supporter key for heavier use.
const API_KEY: &str = "2";

/// The two wide images the panel uses, as URLs; either can be absent when
/// TheAudioDB carries only one for an artist.
#[derive(Clone, Default)]
pub struct ArtistArt {
    /// The 1000x185-ish banner, the header's first choice.
    pub banner: Option<String>,
    /// The 16:9 fanart, the dimmed background behind the text.
    pub fanart: Option<String>,
}

impl ArtistArt {
    pub fn is_empty(&self) -> bool {
        self.banner.is_none() && self.fanart.is_none()
    }
}

/// The wide art for an artist by name: Ok(None) is TheAudioDB not knowing
/// the name, Err the network or the API failing. The result has to fold
/// to the queried name - a name search can hand back a near-miss, and the
/// wrong band's banner is worse than none. The wide thumb stands in for a
/// missing banner or fanart, so an artist with only one still dresses the
/// panel.
pub fn artist_art(name: &str) -> Result<Option<ArtistArt>, String> {
    if name.trim().is_empty() {
        return Ok(None);
    }
    let url = format!("https://www.theaudiodb.com/api/v1/json/{API_KEY}/search.php");
    let text = agent()
        .get(&url)
        .query("s", name.trim())
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    // The endpoint answers an unknown name with `{"artists":null}`, a
    // clean miss rather than an error.
    let Some(artists) = body.get("artists").and_then(|v| v.as_array()) else {
        return Ok(None);
    };
    let folded = normalize(name);
    for artist in artists {
        if normalize(&string(artist.get("strArtist"))) != folded {
            continue;
        }
        let wide = string(artist.get("strArtistWideThumb"));
        let pick = |field: &str| {
            let value = string(artist.get(field));
            if !value.is_empty() {
                Some(value)
            } else if !wide.is_empty() {
                Some(wide.clone())
            } else {
                None
            }
        };
        let art = ArtistArt {
            banner: pick("strArtistBanner"),
            fanart: pick("strArtistFanart"),
        };
        return Ok((!art.is_empty()).then_some(art));
    }
    Ok(None)
}
