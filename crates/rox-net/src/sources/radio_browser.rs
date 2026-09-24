//! radio-browser.info, the community station directory. Its maintainers ask
//! for a real User-Agent and that clients not hammer the `all.` round-robin
//! name, so the first search picks one mirror for the session. Broken and HLS
//! stations are dropped: the transport reads one byte stream.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{number, text};
use crate::providers::{agent, net_reason};

const ANY_MIRROR: &str = "all.api.radio-browser.info";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    /// Past any `.pls`; the directory resolves those.
    pub url: String,
    pub homepage: String,
    pub favicon: String,
    pub tags: String,
    pub country: String,
    pub codec: String,
    /// Zero when unknown.
    pub bitrate_kbps: u16,
    pub votes: u64,
}

/// Most voted first.
pub fn search(text: &str, limit: usize) -> Result<Vec<Found>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let body = agent()
        .get(&format!("https://{}/json/stations/search", mirror()))
        .query("name", text)
        .query("limit", &limit.to_string())
        .query("hidebroken", "true")
        .query("order", "votes")
        .query("reverse", "true")
        .call()
        .map_err(|e| net_reason(&e))?
        .into_string()
        .map_err(|e| e.to_string())?;

    parse(&body)
}

/// Picked once per session, so paging doesn't hit three different caches.
fn mirror() -> &'static str {
    static MIRROR: OnceLock<String> = OnceLock::new();

    MIRROR.get_or_init(|| {
        let listed = agent()
            .get(&format!("https://{ANY_MIRROR}/json/servers"))
            .call()
            .ok()
            .and_then(|response| response.into_string().ok())
            .map(|body| mirrors(&body))
            .unwrap_or_default();

        pick(&listed).unwrap_or_else(|| ANY_MIRROR.to_string())
    })
}

/// Deduplicated: a dual-stack mirror is listed once per address.
fn mirrors(body: &str) -> Vec<String> {
    let Ok(Value::Array(servers)) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };

    let mut names: Vec<String> = servers
        .iter()
        .map(|server| text(server, "name"))
        .filter(|name| !name.is_empty())
        .collect();

    names.sort();
    names.dedup();
    names
}

/// Spread by the clock, to avoid an RNG dependency.
fn pick(names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);

    names.get(nanos % names.len()).cloned()
}

fn parse(body: &str) -> Result<Vec<Found>, String> {
    let Value::Array(stations) = serde_json::from_str::<Value>(body).map_err(|e| e.to_string())?
    else {
        return Err("unrecognized reply".to_string());
    };

    Ok(stations.iter().filter_map(found).collect())
}

fn found(station: &Value) -> Option<Found> {
    if number(station, "hls") != 0 || number(station, "lastcheckok") == 0 {
        return None;
    }

    // Fall back to the plain URL when the directory hasn't resolved it.
    let mut url = text(station, "url_resolved");
    if url.is_empty() {
        url = text(station, "url");
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }

    let name = text(station, "name");
    if name.is_empty() {
        return None;
    }

    Some(Found {
        name,
        url,
        homepage: text(station, "homepage"),
        favicon: text(station, "favicon"),
        tags: text(station, "tags"),
        country: text(station, "countrycode").to_uppercase(),
        codec: text(station, "codec"),
        bitrate_kbps: number(station, "bitrate").clamp(0, u16::MAX as i64) as u16,
        votes: number(station, "votes").max(0) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"[
        {"name":"Adroit Jazz Underground","url":"https://icecast.walmradio.com:8443/jazz",
         "url_resolved":"https://icecast.walmradio.com:8443/jazz","homepage":"https://walmradio.com/",
         "favicon":"https://icecast.walmradio.com:8443/jazz.jpg","tags":"bebop,cool jazz",
         "countrycode":"us","codec":"MP3","bitrate":320,"hls":0,"lastcheckok":1,"votes":181857},
        {"name":"Segmented","url":"https://example.com/live.m3u8","url_resolved":"https://example.com/live.m3u8",
         "countrycode":"DE","codec":"AAC","bitrate":128,"hls":1,"lastcheckok":1,"votes":5},
        {"name":"Plain","url":"http://example.org/stream","url_resolved":"",
         "countrycode":"","codec":"","bitrate":"96","hls":0,"lastcheckok":1,"votes":"2"}
    ]"#;

    #[test]
    fn a_reply_keeps_the_playable_stations() {
        let found = parse(REPLY).unwrap();

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "Adroit Jazz Underground");
        assert_eq!(found[0].url, "https://icecast.walmradio.com:8443/jazz");
        assert_eq!(found[0].country, "US");
        assert_eq!(found[0].codec, "MP3");
        assert_eq!(found[0].bitrate_kbps, 320);
        assert_eq!(found[0].votes, 181857);
        assert_eq!(found[0].tags, "bebop,cool jazz");
    }

    #[test]
    fn an_unresolved_url_falls_back_to_the_plain_one() {
        let found = parse(REPLY).unwrap();

        assert_eq!(found[1].url, "http://example.org/stream");
        assert_eq!(found[1].bitrate_kbps, 96);
        assert_eq!(found[1].votes, 2);
        assert_eq!(found[1].country, "");
    }

    #[test]
    fn broken_and_non_http_stations_are_dropped() {
        let found = parse(
            r#"[{"name":"Down","url":"http://a/x","url_resolved":"http://a/x","hls":0,"lastcheckok":0},
                {"name":"Odd","url":"rtsp://a/x","url_resolved":"rtsp://a/x","hls":0,"lastcheckok":1},
                {"name":"","url":"http://a/x","url_resolved":"http://a/x","hls":0,"lastcheckok":1}]"#,
        )
        .unwrap();

        assert!(found.is_empty());
    }

    #[test]
    fn a_reply_that_is_not_a_list_is_an_error() {
        assert!(parse(r#"{"error":"nope"}"#).is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn mirrors_dedupe_the_dual_stack_entries() {
        let names = mirrors(
            r#"[{"ip":"91.98.4.78","name":"de1.api.radio-browser.info"},
                {"ip":"2a01:4f8::1","name":"de1.api.radio-browser.info"},
                {"ip":"1.2.3.4","name":"fi1.api.radio-browser.info"}]"#,
        );

        assert_eq!(
            names,
            vec!["de1.api.radio-browser.info", "fi1.api.radio-browser.info"]
        );
        assert!(pick(&names).is_some());
        assert!(pick(&[]).is_none());
    }
}
