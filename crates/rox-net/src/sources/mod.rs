//! Source clients: the servers rox borrows a catalog from rather than
//! scanning one off disk. A source client speaks one server's API, blocks
//! like everything else in this crate, and hands back plain data. It never
//! writes a file, never touches SQLite and never knows what a library row
//! looks like; the service layer above does that mapping, the same way the
//! enrichment providers hand back candidates and leave the writing to the
//! writer.
//!
//! There's no per-domain trait split here the way `providers` has one trait
//! per enrichment domain. That split earned itself by having five services
//! answer the same question. Sources have one implementation so far, and a
//! trait invented ahead of its second implementor is a guess about what the
//! second one will need. When it lands, the shape these types already have
//! is the trait.
//!
//! Two modules here sit beside the source clients without being ones, and
//! both are about radio. `radio_browser` is a directory: it answers "which
//! stations exist", and what it finds only becomes a row when someone adds
//! it. `stream_probe` answers the narrower question a typed URL raises,
//! which is whether the thing on the other end is a stream at all.

use serde_json::Value;

pub mod autoeq;
pub mod radio_browser;
pub mod stream_probe;
pub mod subsonic;

/// One track as a source describes it, before anything maps it onto a
/// library row. Strings rather than options throughout: a server that
/// doesn't know a track's genre just leaves the field out, and an empty
/// string is what the library stores for that anyway.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceTrack {
    /// The server's own id for the song, stored as the row's path.
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
    /// Bytes on the server, for the row's size column.
    pub size: i64,
    /// The stream URL, credentials already in the query string minus the
    /// token and salt, which the resolve step adds fresh.
    pub stream_url: String,
    /// The server's cover art id, empty when it has none.
    pub cover_id: String,
}

/// An internet radio station as a server lists it. Servers keep these as a
/// side list, not as songs, so one lands in the radio source rather than
/// under the server's own rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceStation {
    pub id: String,
    pub name: String,
    pub stream_url: String,
    pub home_page: String,
}

/// A playlist as the server holds it: a name and the song ids in it. The
/// ids are the server's own, so turning them into rox row ids happens after
/// the tracks are upserted and never before.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourcePlaylist {
    pub id: String,
    pub name: String,
    pub track_ids: Vec<String>,
}

/// A string field, trimmed, empty when it's missing or isn't a string.
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

/// A numeric field, 0 when it's missing. Servers send these as numbers, but
/// a few send them as strings, so both are read.
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
