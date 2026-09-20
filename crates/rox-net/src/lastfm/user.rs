//! The public reads against a named account: the scrobble history behind
//! the play-count import, and the registration date the import's fallback
//! needs to know how far back an account's listening can possibly reach.
//!
//! Neither call is signed. They read a public profile, so they carry an
//! api key and nothing else, the same terms [`crate::providers::lastfm`]
//! reads an artist on. They ride the shared provider agent for its
//! User-Agent and its timeout, and a ureq error folds through
//! [`crate::providers::net_reason`] so the api key in the URL never
//! reaches a log line.
//!
//! `user.getRecentTracks` is the only Last.fm method that dates a play.
//! The top-tracks and track-info calls report how often an account played
//! something and never when, which is why an import built on counts alone
//! has to invent its timestamps. This one hands over every scrobble with
//! the unix second it happened at, newest first, 200 to a page.
//!
//! Two rows come back without a date: the track playing right now, marked
//! `nowplaying`, and the very occasional entry Last.fm has no time for.
//! They parse like any other and carry `played_at: None`, for the caller
//! to fall back on rather than for this module to guess at.

use std::collections::BTreeMap;

use super::api_root;
use crate::providers::{agent, net_reason};

/// One scrobble as Last.fm recorded it. No album: the import matches on
/// artist and title the way the loved list does, and a scrobble's album
/// field is whatever the submitting client happened to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scrobble {
    pub artist: String,
    pub title: String,
    /// When the play began, unix seconds. None for the now-playing row.
    pub played_at: Option<i64>,
}

/// One page of history and what it said about the rest of it.
#[derive(Debug, Default)]
pub struct RecentPage {
    pub scrobbles: Vec<Scrobble>,
    /// Pages behind this one at the requested page size, at least 1.
    pub pages: usize,
    /// Scrobbles in the whole range the request asked for.
    pub total: usize,
}

/// The most scrobbles one request may ask for. The API's own ceiling;
/// asking for more just gets this many back.
pub const MAX_LIMIT: usize = 200;

/// One page of an account's scrobble history, newest first.
///
/// `from` bounds the range at the bottom: with it the service only
/// answers with scrobbles after that second, which is how a re-import
/// pulls what arrived since the last one instead of the whole decade
/// again. The page count comes back scoped to the same bound, so paging
/// reads it from the response rather than from an earlier unbounded run.
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

/// When the account was registered, unix seconds, or None when the
/// profile doesn't say. The floor for anything that has to place invented
/// history somewhere: nobody scrobbled before they had an account.
pub fn registered_at(key: &str, user: &str) -> Result<Option<i64>, String> {
    let mut query: BTreeMap<&str, String> = BTreeMap::new();
    query.insert("method", "user.getinfo".into());
    query.insert("user", user.into());
    query.insert("api_key", key.into());
    query.insert("format", "json".into());

    parse_info(&get(&query)?)
}

/// The shared half of both calls: send the query, hand back the body.
/// An API error still answers with JSON worth reading, so a status
/// failure reads out like a success and the parse finds the message.
fn get(query: &BTreeMap<&str, String>) -> Result<String, String> {
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

/// A recent-tracks body to scrobbles and the shape of the history behind
/// them. Split from the request so the parsing is testable without a
/// network.
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
    // One scrobble comes back as a bare object where a list would be an
    // array, the shape every Last.fm collection takes at length one.
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

/// One row of the list. Neither name is optional: an entry missing one
/// can't be matched against anything, so it never enters the count.
fn scrobble(row: &serde_json::Value) -> Option<Scrobble> {
    let title = string(row.get("name"))?;
    // Plain calls send the artist as an element with its mbid on it, so
    // the name lands under "#text"; `extended=1` sends an object with a
    // "name" instead. Read either, since which one arrives is a request
    // flag away and this has to survive both.
    let artist = row
        .get("artist")
        .and_then(|artist| match artist {
            serde_json::Value::String(name) => Some(name.clone()),
            _ => string(artist.get("#text")).or_else(|| string(artist.get("name"))),
        })
        .filter(|artist| !artist.is_empty())?;

    // No date is the now-playing row, and it's a real entry with an
    // unknowable time rather than a broken one.
    let played_at = row
        .get("date")
        .and_then(|date| date.get("uts"))
        .map(number_of)
        .map(|uts| uts as i64)
        .filter(|uts| *uts > 0);

    Some(Scrobble {
        artist,
        title,
        played_at,
    })
}

/// When the profile says the account was registered. The unix second
/// lives on the attribute; the element's own text is the human date in
/// one version of the API and the same number in another, so only the
/// attribute is read.
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

/// The API's own error message where the body carries one.
fn api_error(body: &serde_json::Value) -> Option<String> {
    body.get("error").map(|_| {
        body.get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error")
            .to_string()
    })
}

/// A count or a timestamp as Last.fm sends it. The numbers come back as
/// strings, but a service that starts sending them as numbers shouldn't
/// reset someone's import to zero.
fn number_of(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(0),
        other => other.as_u64().unwrap_or(0) as usize,
    }
}

/// A trimmed string field, or None when it's missing or empty.
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
            {"artist":{"name":"Air","url":"x"},"name":"Sexy Boy","date":{"uts":1600000000}},
            "@attr":{"total":"1"}}}"#;
        let page = parse_recent(body).unwrap();
        assert_eq!(page.scrobbles.len(), 1);
        assert_eq!(page.scrobbles[0].artist, "Air");
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
