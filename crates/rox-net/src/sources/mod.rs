//! Source clients: servers rox borrows a catalog from rather than scanning
//! disk. A client speaks one server's API and hands back plain data; it never
//! writes a file or knows what a library row looks like. No trait yet: there
//! is one implementation, and a trait invented before the second is a guess.
//!
//! `radio_browser` (the station directory), `stream_probe` (is this URL a
//! stream?) and `autoeq` (headphone EQ profiles) sit here without being
//! source clients.

use serde_json::Value;

pub mod autoeq;
pub mod radio_browser;
pub mod stream_probe;
pub mod subsonic;

/// Strings rather than options: an empty string is what the library stores anyway.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceTrack {
    /// Stored as the row's path.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: u16,
    pub disc_no: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub size: i64,
    /// Credentials in the query minus token and salt, which resolve adds fresh.
    pub stream_url: String,
    pub cover_id: String,
}

/// Servers list stations apart from songs; they land in the radio source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceStation {
    pub id: String,
    pub name: String,
    pub stream_url: String,
    pub home_page: String,
}

/// Server song ids; mapping to rox row ids waits until the tracks are upserted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourcePlaylist {
    pub id: String,
    pub name: String,
    pub track_ids: Vec<String>,
}

pub(crate) fn text(value: &Value, key: &str) -> String {
    text_of(value.get(key))
}

pub(crate) fn text_of(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

/// 0 when missing. A few servers send numbers as strings.
pub(crate) fn number(value: &Value, key: &str) -> i64 {
    let Some(field) = value.get(key) else {
        return 0;
    };

    field
        .as_i64()
        .or_else(|| field.as_f64().map(|n| n as i64))
        .or_else(|| field.as_str().and_then(|s| s.trim().parse().ok()))
        .unwrap_or(0)
}
