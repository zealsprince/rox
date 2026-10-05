//! Unsigned public reads against a named account: the scrobble history
//! behind the play-count import, and the registration date that floors its
//! invented timestamps. [`get`] is the one retrying read both Last.fm imports
//! go through. Errors go through [`crate::providers::net_reason`] so the api
//! key in the URL never reaches a log.
//!
//! `user.getRecentTracks` is the only Last.fm method that dates a play.
//! The now-playing row, and the odd entry Last.fm has no time for, come
//! back with `played_at: None` for the caller to handle.

use std::collections::BTreeMap;
use std::time::Duration;

use super::api_root;
use crate::providers::{agent, net_reason};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scrobble {
    pub artist: String,
    pub title: String,
    /// Whatever the submitting client sent, empty when it sent none. Display
    /// only: the import matches on artist and title.
    pub album: String,
    /// Unix seconds. None for the now-playing row.
    pub played_at: Option<i64>,
}

#[derive(Debug, Default)]
pub struct RecentPage {
    pub scrobbles: Vec<Scrobble>,
    /// At least 1.
    pub pages: usize,
    pub total: usize,
}

/// The API's own page-size ceiling.
pub const MAX_LIMIT: usize = 200;

/// One page of scrobble history, newest first. `from` bounds the range at
/// the bottom so a re-import only pulls what's new; the page count comes back
/// scoped to that bound.
pub fn recent_tracks(
    key: &str,
    user: &str,
    page: usize,
    from: Option<i64>,
    limit: usize,
) -> Result<RecentPage, String> {
    let mut query: BTreeMap<&str, String> = BTreeMap::new();
    query.insert("method", "user.getrecenttracks".into());
    query.insert("user", user.into());
    query.insert("api_key", key.into());
    query.insert("limit", limit.clamp(1, MAX_LIMIT).to_string());
    query.insert("page", page.max(1).to_string());
    query.insert("format", "json".into());
    if let Some(from) = from {
        query.insert("from", from.max(0).to_string());
    }

    parse_recent(&get(&query)?)
}

/// When the account was registered, unix seconds: the floor for invented
/// history.
pub fn registered_at(key: &str, user: &str) -> Result<Option<i64>, String> {
    let mut query: BTreeMap<&str, String> = BTreeMap::new();
    query.insert("method", "user.getinfo".into());
    query.insert("user", user.into());
    query.insert("api_key", key.into());
    query.insert("format", "json".into());

    parse_info(&get(&query)?)
}

/// Error codes Last.fm means as "try again": operation failed (8), service
/// offline (11), temporarily unavailable (16) and rate limited (29).
const TRANSIENT: [usize; 4] = [8, 11, 16, 29];

/// Between attempts at one request. A history import walks hundreds of pages
/// and a failed page ends the run before its bound is saved, so a code 8 a
/// retry would clear costs the whole import.
const RETRY_WAITS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(4),
    Duration::from_secs(15),
];

/// One unsigned read of the API, retried while Last.fm answers a transient
/// error. A status failure still carries a JSON error body, so it reads like
/// a success and the caller's parse reports it. Blocking, and up to 20 s
/// longer than the request when Last.fm keeps failing.
pub fn get(query: &BTreeMap<&str, String>) -> Result<String, String> {
    let mut waits = RETRY_WAITS.iter();

    loop {
        let body = get_once(query)?;
        match (transient(&body), waits.next()) {
            (true, Some(&wait)) => std::thread::sleep(wait),
            _ => return Ok(body),
        }
    }
}

fn get_once(query: &BTreeMap<&str, String>) -> Result<String, String> {
    let mut request = agent().get(&api_root());
    for (name, value) in query {
        request = request.query(name, value);
    }

    match request.call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(_, response)) => response.into_string().map_err(|e| e.to_string()),
        Err(e) => Err(net_reason(&e)),
    }
}

fn transient(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|body| body.get("error").map(number_of))
        .is_some_and(|code| TRANSIENT.contains(&code))
}

fn parse_recent(text: &str) -> Result<RecentPage, String> {
    let body: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if let Some(message) = api_error(&body) {
        return Err(message);
    }
    let Some(recent) = body.get("recenttracks") else {
        return Err("no recent tracks in the response".into());
    };

    let attr = recent.get("@attr");
    let number =
        |field: &str| -> usize { attr.and_then(|a| a.get(field)).map(number_of).unwrap_or(0) };
    // A single scrobble arrives as a bare object instead of an array.
    let rows: Vec<&serde_json::Value> = match recent.get("track") {
        Some(serde_json::Value::Array(rows)) => rows.iter().collect(),
        Some(one @ serde_json::Value::Object(_)) => vec![one],
        _ => Vec::new(),
    };

    let scrobbles = rows.into_iter().filter_map(scrobble).collect();

    Ok(RecentPage {
        scrobbles,
        pages: number("totalPages").max(1),
        total: number("total"),
    })
}

/// An entry missing either name can't be matched, so it's dropped.
fn scrobble(row: &serde_json::Value) -> Option<Scrobble> {
    let title = string(row.get("name"))?;
    // Plain calls put the name under "#text", `extended=1` under "name".
    let artist = row
        .get("artist")
        .and_then(|artist| match artist {
            serde_json::Value::String(name) => Some(name.clone()),
            _ => string(artist.get("#text")).or_else(|| string(artist.get("name"))),
        })
        .filter(|artist| !artist.is_empty())?;
    // The album takes the artist's two shapes.
    let album = row
        .get("album")
        .and_then(|album| match album {
            serde_json::Value::String(name) => Some(name.trim().to_string()),
            _ => string(album.get("#text")).or_else(|| string(album.get("name"))),
        })
        .unwrap_or_default();

    let played_at = row
        .get("date")
        .and_then(|date| date.get("uts"))
        .map(number_of)
        .map(|uts| uts as i64)
        .filter(|uts| *uts > 0);

    Some(Scrobble {
        artist,
        title,
        album,
        played_at,
    })
}

/// The unix second lives on the attribute; the element's text is a human
/// date in some API versions.
fn parse_info(text: &str) -> Result<Option<i64>, String> {
    let body: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if let Some(message) = api_error(&body) {
        return Err(message);
    }

    Ok(body
        .get("user")
        .and_then(|user| user.get("registered"))
        .and_then(|registered| match registered {
            serde_json::Value::Object(_) => registered.get("unixtime").map(number_of),
            other => Some(number_of(other)),
        })
        .map(|seconds| seconds as i64)
        .filter(|seconds| *seconds > 0))
}

fn api_error(body: &serde_json::Value) -> Option<String> {
    body.get("error").map(|_| {
        body.get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error")
            .to_string()
    })
}

/// Last.fm sends numbers as strings; accept either.
fn number_of(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(0),
        other => other.as_u64().unwrap_or(0) as usize,
    }
}

fn string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r##"{"recenttracks":{"track":[
        {"artist":{"mbid":"aaa","#text":"Boards of Canada"},
         "name":"Roygbiv","album":{"#text":"Music Has the Right to Children"},
         "@attr":{"nowplaying":"true"}},
        {"artist":{"mbid":"aaa","#text":"Boards of Canada"},
         "name":"Olson","date":{"uts":"1712345678","#text":"5 Apr 2024, 19:34"}},
        {"artist":{"mbid":"","#text":"Aphex Twin"},
         "name":"Xtal","date":{"uts":"1700000000","#text":"14 Nov 2023, 22:13"}},
        {"artist":{"mbid":"","#text":""},"name":"Nameless","date":{"uts":"1699999999"}}
    ],"@attr":{"user":"someone","page":"1","perPage":"200","totalPages":"7","total":"1364"}}}"##;

    #[test]
    fn a_page_parses_its_scrobbles_and_its_shape() {
        let page = parse_recent(PAGE).unwrap();
        assert_eq!(page.scrobbles.len(), 3, "the artistless row never counts");
        assert_eq!(page.pages, 7);
        assert_eq!(page.total, 1364);
        assert_eq!(page.scrobbles[1].artist, "Boards of Canada");
        assert_eq!(page.scrobbles[1].title, "Olson");
        assert_eq!(page.scrobbles[1].played_at, Some(1_712_345_678));
        assert_eq!(page.scrobbles[2].played_at, Some(1_700_000_000));
    }

    #[test]
    fn the_album_comes_along_and_reads_empty_when_absent() {
        let page = parse_recent(PAGE).unwrap();
        assert_eq!(page.scrobbles[0].album, "Music Has the Right to Children");
        assert_eq!(page.scrobbles[1].album, "", "no album on the row");
    }

    #[test]
    fn only_a_try_again_error_retries() {
        let failed = r#"{"error":8,"message":"Operation failed - Most likely the backend service failed. Please try again."}"#;
        assert!(transient(failed));
        assert!(transient(
            r#"{"error":"29","message":"Rate limit exceeded"}"#
        ));

        assert!(!transient(r#"{"error":6,"message":"User not found"}"#));
        assert!(!transient(PAGE), "a page is no error");
        assert!(!transient("<html>bad gateway</html>"));
    }

    #[test]
    fn the_now_playing_row_arrives_without_a_time() {
        let page = parse_recent(PAGE).unwrap();
        assert_eq!(page.scrobbles[0].title, "Roygbiv");
        assert_eq!(
            page.scrobbles[0].played_at, None,
            "nothing to date a play that hasn't finished"
        );
    }

    #[test]
    fn the_extended_form_names_its_artist_differently() {
        let body = r#"{"recenttracks":{"track":
            {"artist":{"name":"Air","url":"x"},"name":"Sexy Boy",
             "album":{"name":"Moon Safari"},"date":{"uts":1600000000}},
            "@attr":{"total":"1"}}}"#;
        let page = parse_recent(body).unwrap();
        assert_eq!(page.scrobbles.len(), 1);
        assert_eq!(page.scrobbles[0].artist, "Air");
        assert_eq!(page.scrobbles[0].album, "Moon Safari");
        assert_eq!(
            page.scrobbles[0].played_at,
            Some(1_600_000_000),
            "a number reads the same as the string"
        );
        assert_eq!(page.pages, 1, "no page count still means one page");
    }

    #[test]
    fn an_empty_history_is_a_clean_zero() {
        let page = parse_recent(r#"{"recenttracks":{"@attr":{"total":"0"}}}"#).unwrap();
        assert!(page.scrobbles.is_empty());
        assert_eq!(page.total, 0);
    }

    #[test]
    fn an_api_error_carries_its_message_out() {
        let body = r#"{"error":6,"message":"User not found"}"#;
        assert_eq!(parse_recent(body).err(), Some("User not found".to_string()));
        assert_eq!(parse_info(body).err(), Some("User not found".to_string()));
    }

    #[test]
    fn a_profile_gives_up_its_registration_second() {
        let body = r##"{"user":{"name":"RJ","playcount":"54189",
            "registered":{"unixtime":"1037793040","#text":1037793040}}}"##;
        assert_eq!(parse_info(body).unwrap(), Some(1_037_793_040));
    }

    #[test]
    fn a_profile_without_one_says_so() {
        assert_eq!(parse_info(r#"{"user":{"name":"RJ"}}"#).unwrap(), None);
        assert_eq!(
            parse_info(r#"{"user":{"registered":{"unixtime":"0"}}}"#).unwrap(),
            None,
            "the epoch is nobody's registration date"
        );
    }
}
