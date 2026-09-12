//! TheAudioDB (theaudiodb.com): the wide artist art the biography panel
//! needs and Last.fm and deezer don't provide, plus the facts about the
//! act that Last.fm's wiki text buries: a banner and up to four fanarts
//! for the header, the country code, and the years active. One keyless
//! search by name returns all of it; the store downloads the images
//! beside the deezer portrait and keeps the record in the cache file.
//! Blocking, background executor only, like the other providers.
//!
//! The key below is TheAudioDB's public test key. It works on the search
//! and image endpoints at a low rate limit, which is all a per-artist lookup
//! needs; a fork leaning on it harder registers for a supporter key and
//! drops it in here, the Last.fm identity's trade-off ([`crate::lastfm::keys`]).

use serde::{Deserialize, Serialize};

use super::{agent, net_reason, normalize, string};

/// TheAudioDB's public test key. Enough for the biography panel's
/// one-artist-at-a-time lookups; swap in a supporter key for heavier use.
const API_KEY: &str = "2";

/// What TheAudioDB has on an artist: the image URLs the panel shows and
/// the facts the header line reads. Any field can be absent; an artist
/// the service knows by name alone still has a record, so the store can
/// tell "asked and got nothing" from "never asked". Serialized into the
/// artist store's cache file; missing fields default, so an old entry
/// still loads after the shape drifts.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistProfile {
    /// The 1000x185-ish banner, the header's first choice. Falls back to
    /// the wide thumb when the service has no banner.
    pub banner: Option<String>,
    /// The 16:9 fanart, the dimmed background behind the text. The first
    /// real fanart, or the wide thumb when there is none.
    pub fanart: Option<String>,
    /// Every real fanart the service holds, distinct, in its numbering;
    /// the header's cycle. The first one is also [`Self::fanart`] when
    /// that didn't fall back to the wide thumb.
    pub fanarts: Vec<String>,
    /// The ISO 3166 alpha-2 country code, uppercase, empty when unknown.
    pub country: String,
    /// The year the act formed or the person was born.
    pub formed: Option<u16>,
    /// The year the act ended, when the service records one.
    pub ended: Option<u16>,
    /// Whether the service marks the act as disbanded, for one that ended
    /// without a year on file.
    pub disbanded: bool,
}

impl ArtistProfile {
    /// Whether there is any image to download.
    pub fn has_images(&self) -> bool {
        self.banner.is_some() || self.fanart.is_some() || !self.fanarts.is_empty()
    }
}

/// The record for an artist by name: Ok(None) is TheAudioDB having no
/// such name, Err the network or the API failing. The result has to fold
/// to the queried name: a name search can hand back a near-miss, and the
/// wrong band's banner is worse than none. The wide thumb stands in for a
/// missing banner or fanart, so an artist with only one still fills both
/// slots.
pub fn artist_profile(name: &str) -> Result<Option<ArtistProfile>, String> {
    if name.trim().is_empty() {
        return Ok(None);
    }
    let url = format!("https://www.theaudiodb.com/api/v1/json/{API_KEY}/search.php");
    let text = agent()
        .get(&url)
        .query("s", name.trim())
        .call()
        .map_err(|e| net_reason(&e))?
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
        return Ok(Some(parse(artist)));
    }
    Ok(None)
}

/// One search result into a profile.
fn parse(artist: &serde_json::Value) -> ArtistProfile {
    let wide = string(artist.get("strArtistWideThumb"));
    let mut fanarts = Vec::new();
    for field in [
        "strArtistFanart",
        "strArtistFanart2",
        "strArtistFanart3",
        "strArtistFanart4",
    ] {
        let value = string(artist.get(field));
        if !value.is_empty() && !fanarts.contains(&value) {
            fanarts.push(value);
        }
    }
    let or_wide = |value: String| {
        if !value.is_empty() {
            Some(value)
        } else if !wide.is_empty() {
            Some(wide.clone())
        } else {
            None
        }
    };
    // The years arrive as strings ("2013"), or null; anything that isn't
    // a plain year reads as unknown.
    let year = |field: &str| string(artist.get(field)).parse::<u16>().ok();
    ArtistProfile {
        banner: or_wide(string(artist.get("strArtistBanner"))),
        fanart: or_wide(fanarts.first().cloned().unwrap_or_default()),
        country: string(artist.get("strCountryCode")).trim().to_uppercase(),
        formed: year("intFormedYear").or_else(|| year("intBornYear")),
        ended: year("intDiedYear"),
        disbanded: string(artist.get("strDisbanded")).eq_ignore_ascii_case("yes"),
        fanarts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_thumb_fills_missing_slots_and_years_parse() {
        let artist = serde_json::json!({
            "strArtist": "Band",
            "strArtistWideThumb": "https://x/wide",
            "strArtistFanart": "https://x/f1",
            "strArtistFanart2": "https://x/f2",
            "strArtistFanart3": "https://x/f2",
            "strCountryCode": "jp",
            "intFormedYear": "2013",
            "intDiedYear": null,
            "strDisbanded": null
        });
        let profile = parse(&artist);
        assert_eq!(profile.banner.as_deref(), Some("https://x/wide"));
        assert_eq!(profile.fanart.as_deref(), Some("https://x/f1"));
        assert_eq!(profile.fanarts, vec!["https://x/f1", "https://x/f2"]);
        assert_eq!(profile.country, "JP");
        assert_eq!(profile.formed, Some(2013));
        assert_eq!(profile.ended, None);
        assert!(!profile.disbanded);
    }

    #[test]
    fn a_bare_record_has_no_images() {
        let profile = parse(&serde_json::json!({ "strArtist": "Band", "strDisbanded": "Yes" }));
        assert!(!profile.has_images());
        assert!(profile.disbanded);
        assert!(profile.fanart.is_none());
    }
}
