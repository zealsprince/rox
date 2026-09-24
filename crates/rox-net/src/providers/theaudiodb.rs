//! TheAudioDB (theaudiodb.com): the wide artist art the biography panel
//! needs (banner, fanarts) plus country and years active, from one keyless
//! search by name.

use serde::{Deserialize, Serialize};

use super::{agent, net_reason, normalize, string};

/// The public test key: low rate limit, enough for one-artist lookups.
const API_KEY: &str = "2";

/// Serialized into the artist store's cache; missing fields default so old
/// entries still load.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistProfile {
    /// Falls back to the wide thumb.
    pub banner: Option<String>,
    /// The first real fanart, or the wide thumb.
    pub fanart: Option<String>,
    pub fanarts: Vec<String>,
    /// ISO 3166 alpha-2, uppercase, empty when unknown.
    pub country: String,
    /// The year the act formed or the person was born.
    pub formed: Option<u16>,
    pub ended: Option<u16>,
    /// For an act that ended without a year on file.
    pub disbanded: bool,
}

impl ArtistProfile {
    pub fn has_images(&self) -> bool {
        self.banner.is_some() || self.fanart.is_some() || !self.fanarts.is_empty()
    }
}

/// The result has to match the name once folded: a near-miss is the wrong
/// band's banner.
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
