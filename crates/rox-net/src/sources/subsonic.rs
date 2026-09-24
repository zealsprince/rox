//! Subsonic and OpenSubsonic: a catalog on a server the user runs. Every
//! request carries a token `md5(password + salt)` with a fresh salt, the auth
//! every server implements. Responses drift between Subsonic, Navidrome,
//! Airsonic and gonic, so parsing goes through `serde_json::Value`.
//!
//! Streaming asks for `format=raw`: a server transcode would put gapless and
//! ReplayGain to work on something other than the user's file.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{SourcePlaylist, SourceStation, SourceTrack, number, text, text_of};
use crate::providers::{agent, net_reason};

/// The last Subsonic release. `getArtists` and `search3` need 1.8.0, so
/// nothing older works anyway.
const API_VERSION: &str = "1.16.1";

/// Servers show this in their session lists.
const CLIENT: &str = "rox";

/// The OpenSubsonic fields are absent on a plain Subsonic server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    pub version: String,
    /// "Navidrome", "gonic"; empty on plain Subsonic.
    pub server_type: String,
    pub server_version: String,
    pub open_subsonic: bool,
}

/// The password is held in the clear because the token is derived per
/// request. It lives in `accounts.json`, not the shareable settings file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Server {
    /// No trailing `/rest` or slash.
    pub url: String,
    pub user: String,
    pub password: String,
}

impl Server {
    pub fn new(url: &str, user: &str, password: &str) -> Server {
        Server {
            url: url.trim().trim_end_matches('/').to_string(),
            user: user.trim().to_string(),
            password: password.to_string(),
        }
    }

    /// A fresh salt per request, so a captured URL can't replay another call.
    fn auth(&self) -> Vec<(String, String)> {
        let salt = salt();
        let token = token(&self.password, &salt);

        vec![
            ("u".to_string(), self.user.clone()),
            ("t".to_string(), token),
            ("s".to_string(), salt),
            ("v".to_string(), API_VERSION.to_string()),
            ("c".to_string(), CLIENT.to_string()),
            ("f".to_string(), "json".to_string()),
        ]
    }

    /// `.view` because the original Subsonic server only serves the suffixed form.
    fn endpoint(&self, method: &str) -> String {
        format!("{}/rest/{method}.view", self.url)
    }

    fn get(&self, method: &str, params: &[(&str, String)]) -> Result<Value, String> {
        let mut request = agent().get(&self.endpoint(method));

        for (key, value) in self.auth() {
            request = request.query(&key, &value);
        }

        for (key, value) in params {
            request = request.query(key, value);
        }

        let text = request
            .call()
            .map_err(|e| net_reason(&e))?
            .into_string()
            .map_err(|e| e.to_string())?;

        parse_response(&text)
    }

    pub fn ping(&self) -> Result<ServerInfo, String> {
        let body = self.get("ping", &[])?;

        Ok(server_info(&body))
    }

    /// `progress` is called per album.
    pub fn catalog(&self, progress: impl Fn(usize, usize)) -> Result<Vec<SourceTrack>, String> {
        // Only the album call returns songs, so progress counts albums.
        let artists = artist_ids(&self.get("getArtists", &[])?);

        let mut album_ids = Vec::new();
        for artist in &artists {
            let body = self.get("getArtist", &[("id", artist.clone())])?;
            album_ids.extend(album_ids_of(&body));
        }

        let total = album_ids.len();
        let mut tracks = Vec::new();

        for (done, album) in album_ids.iter().enumerate() {
            let body = self.get("getAlbum", &[("id", album.clone())])?;
            tracks.extend(self.songs_of(&body));
            progress(done + 1, total);
        }

        Ok(tracks)
    }

    pub fn playlists(&self) -> Result<Vec<SourcePlaylist>, String> {
        let body = self.get("getPlaylists", &[])?;

        let mut out = Vec::new();
        for (id, name) in playlist_index(&body) {
            let body = self.get("getPlaylist", &[("id", id.clone())])?;

            out.push(SourcePlaylist {
                id,
                name,
                track_ids: playlist_entries(&body),
            });
        }

        Ok(out)
    }

    /// `getInternetRadioStations` (since 1.9.0). These go to the radio source,
    /// not this server's rows.
    pub fn radio_stations(&self) -> Result<Vec<SourceStation>, String> {
        let body = self.get("getInternetRadioStations", &[])?;

        Ok(radio_stations_of(&body))
    }

    /// The spec doesn't promise a song's id works as its art id, so the song is
    /// asked. Empty when it has no cover.
    pub fn cover_id(&self, song_id: &str) -> Result<String, String> {
        let body = self.get("getSong", &[("id", song_id.to_string())])?;

        Ok(song_cover_id(&body))
    }

    /// The server scales, so a list row never pulls a full-resolution scan.
    pub fn cover(&self, art_id: &str, size: u32) -> Result<Vec<u8>, String> {
        let mut request = agent().get(&self.endpoint("getCoverArt"));

        for (key, value) in self.auth() {
            request = request.query(&key, &value);
        }

        let response = request
            .query("id", art_id)
            .query("size", &size.to_string())
            .call()
            .map_err(|e| net_reason(&e))?;

        // A failed request answers 200 with a JSON error body.
        if response.content_type().contains("json") {
            let text = response.into_string().map_err(|e| e.to_string())?;
            parse_response(&text)?;

            return Err("server returned no image".to_string());
        }

        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;

        Ok(bytes)
    }

    /// Empty: Subsonic authorizes in the query string. The registry contract
    /// takes headers for servers behind a proxy.
    pub fn stream_headers(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Everything but token and salt, which go on at resolve time: a stored
    /// token would be a replayable credential in SQLite.
    pub fn stream_url(&self, song_id: &str) -> String {
        format!(
            "{}?id={}&format=raw&v={}&c={}&u={}",
            self.endpoint("stream"),
            urlencode(song_id),
            urlencode(API_VERSION),
            urlencode(CLIENT),
            urlencode(&self.user),
        )
    }

    /// The stored URL with a fresh salt and token, for the resolve step.
    pub fn sign(&self, stream_url: &str) -> String {
        let salt = salt();
        let token = token(&self.password, &salt);

        format!("{stream_url}&t={token}&s={salt}")
    }

    /// "subsonic:" plus a digest of URL and username. Never include the
    /// password: changing it would orphan the whole library.
    pub fn source_id(&self) -> String {
        let digest = format!("{:x}", md5::compute(format!("{}\n{}", self.url, self.user)));

        format!("subsonic:{}", &digest[..16])
    }

    fn songs_of(&self, body: &Value) -> Vec<SourceTrack> {
        let Some(songs) = body.pointer("/album/song").and_then(Value::as_array) else {
            return Vec::new();
        };

        songs
            .iter()
            .filter_map(|song| {
                let id = text(song, "id");
                if id.is_empty() {
                    return None;
                }

                let artist = text(song, "artist");
                let album_artist = {
                    let credited = text(song, "albumArtist");
                    // Same fallback the scanner uses on an untagged file.
                    if credited.is_empty() {
                        artist.clone()
                    } else {
                        credited
                    }
                };

                Some(SourceTrack {
                    stream_url: self.stream_url(&id),
                    title: text(song, "title"),
                    artist,
                    album_artist,
                    album: text(song, "album"),
                    genre: text(song, "genre"),
                    year: number(song, "year") as u16,
                    disc_no: number(song, "discNumber") as u16,
                    track_no: number(song, "track") as u16,
                    duration_ms: (number(song, "duration") as u32).saturating_mul(1000),
                    codec: codec_of(song),
                    bitrate_kbps: number(song, "bitRate") as u16,
                    size: number(song, "size"),
                    cover_id: text(song, "coverArt"),
                    id,
                })
            })
            .collect()
    }
}

pub fn token(password: &str, salt: &str) -> String {
    format!("{:x}", md5::compute(format!("{password}{salt}").as_bytes()))
}

/// The spec wants six or more characters that differ per call, not
/// cryptographic strength. Clock plus counter, no RNG crate.
fn salt() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let digest = format!("{:x}", md5::compute(format!("{nanos}:{seq}").as_bytes()));

    digest[..12].to_string()
}

/// Escapes only what breaks a query string, so ids stay readable in logs.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());

    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }

            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }

    out
}

fn parse_response(text: &str) -> Result<Value, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;

    let Some(body) = root.get("subsonic-response") else {
        return Err("unrecognized response".to_string());
    };

    if text_of(body.get("status")) != "failed" {
        return Ok(body.clone());
    }

    let error = body.get("error");
    let code = error.and_then(|e| e.get("code")).and_then(Value::as_i64);
    let message = text_of(error.and_then(|e| e.get("message")));

    Err(error_reason(code.unwrap_or(0), &message))
}

/// The four credential codes collapse into one line: to the user they're all
/// "the login didn't take".
fn error_reason(code: i64, message: &str) -> String {
    match code {
        40 | 41 | 44 | 50 => "check the username and password".to_string(),

        10 => "the server rejected the request as incomplete".to_string(),

        20 => "the server wants a newer client".to_string(),

        30 => "the server is too old for this client".to_string(),

        42 | 43 => "the server refused this way of signing in".to_string(),

        60 => "the server's trial has expired".to_string(),

        70 => "not found on the server".to_string(),

        _ if !message.is_empty() => message.to_string(),

        _ => "the server returned an error".to_string(),
    }
}

fn server_info(body: &Value) -> ServerInfo {
    ServerInfo {
        version: text(body, "version"),
        server_type: text(body, "type"),
        server_version: text(body, "serverVersion"),
        open_subsonic: body
            .get("openSubsonic")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn artist_ids(body: &Value) -> Vec<String> {
    let Some(indexes) = body.pointer("/artists/index").and_then(Value::as_array) else {
        return Vec::new();
    };

    indexes
        .iter()
        .filter_map(|index| index.get("artist").and_then(Value::as_array))
        .flatten()
        .map(|artist| text(artist, "id"))
        .filter(|id| !id.is_empty())
        .collect()
}

fn album_ids_of(body: &Value) -> Vec<String> {
    let Some(albums) = body.pointer("/artist/album").and_then(Value::as_array) else {
        return Vec::new();
    };

    albums
        .iter()
        .map(|album| text(album, "id"))
        .filter(|id| !id.is_empty())
        .collect()
}

fn playlist_index(body: &Value) -> Vec<(String, String)> {
    let Some(lists) = body
        .pointer("/playlists/playlist")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    lists
        .iter()
        .map(|list| (text(list, "id"), text(list, "name")))
        .filter(|(id, _)| !id.is_empty())
        .collect()
}

fn radio_stations_of(body: &Value) -> Vec<SourceStation> {
    let Some(stations) = body
        .pointer("/internetRadioStations/internetRadioStation")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    stations
        .iter()
        .map(|station| SourceStation {
            id: text(station, "id"),
            name: text(station, "name"),
            stream_url: text(station, "streamUrl"),
            home_page: text(station, "homePageUrl"),
        })
        .filter(|station| !station.stream_url.is_empty())
        .collect()
}

fn playlist_entries(body: &Value) -> Vec<String> {
    let Some(entries) = body.pointer("/playlist/entry").and_then(Value::as_array) else {
        return Vec::new();
    };

    entries
        .iter()
        .map(|entry| text(entry, "id"))
        .filter(|id| !id.is_empty())
        .collect()
}

fn song_cover_id(body: &Value) -> String {
    body.get("song")
        .map(|song| text(song, "coverArt"))
        .unwrap_or_default()
}

/// Servers report a file suffix, a MIME type, or both.
fn codec_of(song: &Value) -> String {
    let suffix = text(song, "suffix");
    if !suffix.is_empty() {
        return suffix.to_lowercase();
    }

    text(song, "contentType")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::new("https://music.example.com/", "andrew", "sesame")
    }

    #[test]
    fn token_matches_the_documented_example() {
        // The OpenSubsonic docs' own example.
        assert_eq!(
            token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
        assert_eq!(token("sesame", "c19b2d").len(), 32);
    }

    #[test]
    fn a_salt_is_long_enough_and_never_repeats() {
        let first = salt();
        let second = salt();

        assert!(first.len() >= 6);
        assert_ne!(first, second);
    }

    #[test]
    fn source_id_is_stable_and_splits_on_the_account() {
        let one = Server::new("https://music.example.com", "andrew", "sesame");
        let same = Server::new("https://music.example.com/", "andrew", "different");
        let other_user = Server::new("https://music.example.com", "guest", "sesame");
        let other_host = Server::new("https://other.example.com", "andrew", "sesame");

        assert_eq!(one.source_id(), same.source_id());

        assert_ne!(one.source_id(), other_user.source_id());
        assert_ne!(one.source_id(), other_host.source_id());

        assert!(one.source_id().starts_with("subsonic:"));
    }

    #[test]
    fn ping_reads_what_the_server_says_about_itself() {
        let body = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1",
               "type":"navidrome","serverVersion":"0.53.3","openSubsonic":true}}"#,
        )
        .expect("ok reply");

        let info = server_info(&body);

        assert_eq!(info.version, "1.16.1");
        assert_eq!(info.server_type, "navidrome");
        assert_eq!(info.server_version, "0.53.3");
        assert!(info.open_subsonic);
    }

    #[test]
    fn a_plain_subsonic_ping_reports_no_extensions() {
        let body =
            parse_response(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#).unwrap();

        let info = server_info(&body);

        assert_eq!(info.server_type, "");
        assert!(!info.open_subsonic);
    }

    #[test]
    fn artists_flatten_out_of_their_index_letters() {
        let body = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","artists":{"index":[
               {"name":"A","artist":[{"id":"ar-1","name":"Aphex Twin","albumCount":9},
                                     {"id":"ar-2","name":"Autechre","albumCount":13}]},
               {"name":"B","artist":[{"id":"ar-3","name":"Boards of Canada","albumCount":4}]}]}}}"#,
        )
        .unwrap();

        assert_eq!(artist_ids(&body), vec!["ar-1", "ar-2", "ar-3"]);
    }

    #[test]
    fn an_empty_library_yields_no_artists() {
        let body =
            parse_response(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#).unwrap();

        assert!(artist_ids(&body).is_empty());
        assert!(album_ids_of(&body).is_empty());
    }

    #[test]
    fn albums_come_off_an_artist_reply() {
        let body = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","artist":{
               "id":"ar-1","name":"Aphex Twin","album":[
               {"id":"al-1","name":"Selected Ambient Works 85-92","songCount":13},
               {"id":"al-2","name":"Drukqs","songCount":30}]}}}"#,
        )
        .unwrap();

        assert_eq!(album_ids_of(&body), vec!["al-1", "al-2"]);
    }

    #[test]
    fn songs_map_out_of_an_album_reply() {
        let body = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","album":{
               "id":"al-1","name":"Drukqs","song":[
               {"id":"sg-1","title":"Jynweythek","album":"Drukqs","artist":"Aphex Twin",
                "albumArtist":"Aphex Twin","track":1,"discNumber":1,"year":2001,
                "genre":"Electronic","coverArt":"al-1","size":7261184,"contentType":"audio/flac",
                "suffix":"flac","duration":109,"bitRate":533}]}}}"#,
        )
        .unwrap();

        let songs = server().songs_of(&body);

        assert_eq!(songs.len(), 1);
        let song = &songs[0];

        assert_eq!(song.id, "sg-1");
        assert_eq!(song.title, "Jynweythek");
        assert_eq!(song.album_artist, "Aphex Twin");
        assert_eq!(song.genre, "Electronic");
        assert_eq!(song.year, 2001);
        assert_eq!(song.disc_no, 1);
        assert_eq!(song.track_no, 1);
        assert_eq!(song.duration_ms, 109_000);
        assert_eq!(song.codec, "flac");
        assert_eq!(song.bitrate_kbps, 533);
        assert_eq!(song.size, 7_261_184);
        assert_eq!(song.cover_id, "al-1");
        assert!(song.stream_url.contains("/rest/stream.view?id=sg-1"));
        assert!(song.stream_url.contains("format=raw"));

        assert!(!song.stream_url.contains("&t="));
        assert!(!song.stream_url.contains("&s="));
    }

    #[test]
    fn a_sparse_song_still_maps() {
        let body = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","album":{"song":[
               {"id":"sg-9","title":"Untitled","album":"Bootleg","artist":"Unknown",
                "size":1024,"contentType":"audio/mpeg","duration":0}]}}}"#,
        )
        .unwrap();

        let songs = server().songs_of(&body);
        let song = &songs[0];

        assert_eq!(song.year, 0);
        assert_eq!(song.disc_no, 0);
        assert_eq!(song.track_no, 0);
        assert_eq!(song.genre, "");
        assert_eq!(song.duration_ms, 0);
        assert_eq!(song.cover_id, "");

        assert_eq!(song.album_artist, "Unknown");
        assert_eq!(song.codec, "mpeg");
    }

    #[test]
    fn a_song_reply_names_its_cover() {
        let body = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","song":{
               "id":"sg-1","title":"Jynweythek","coverArt":"mf-sg-1_65f1a0c2"}}}"#,
        )
        .unwrap();

        assert_eq!(song_cover_id(&body), "mf-sg-1_65f1a0c2");

        let bare = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","song":{"id":"sg-9"}}}"#,
        )
        .unwrap();
        let empty =
            parse_response(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#).unwrap();

        assert_eq!(song_cover_id(&bare), "");
        assert_eq!(song_cover_id(&empty), "");
    }

    #[test]
    fn a_song_with_no_id_is_dropped() {
        let body = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","album":{"song":[
               {"title":"Nameless"},{"id":"sg-2","title":"Real"}]}}}"#,
        )
        .unwrap();

        let songs = server().songs_of(&body);

        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].id, "sg-2");
    }

    #[test]
    fn playlists_parse_index_then_entries() {
        let index = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","playlists":{"playlist":[
               {"id":"pl-1","name":"Late","songCount":2}]}}}"#,
        )
        .unwrap();

        assert_eq!(
            playlist_index(&index),
            vec![("pl-1".to_string(), "Late".to_string())]
        );

        let entries = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","playlist":{
               "id":"pl-1","name":"Late","entry":[{"id":"sg-1"},{"id":"sg-4"}]}}}"#,
        )
        .unwrap();

        assert_eq!(playlist_entries(&entries), vec!["sg-1", "sg-4"]);
    }

    #[test]
    fn the_credential_codes_all_say_the_same_thing() {
        for code in [40, 41, 44, 50] {
            let reply = format!(
                r#"{{"subsonic-response":{{"status":"failed","version":"1.16.1",
                   "error":{{"code":{code},"message":"Wrong username or password."}}}}}}"#
            );

            let err = parse_response(&reply).expect_err("failed reply");

            assert_eq!(err, "check the username and password", "code {code}");
        }
    }

    #[test]
    fn other_codes_keep_their_own_reason() {
        let trial = parse_response(
            r#"{"subsonic-response":{"status":"failed","version":"1.16.1",
               "error":{"code":60,"message":"Trial period is over."}}}"#,
        )
        .expect_err("failed reply");

        assert_eq!(trial, "the server's trial has expired");

        let missing = parse_response(
            r#"{"subsonic-response":{"status":"failed","version":"1.16.1",
               "error":{"code":70,"message":"Song not found."}}}"#,
        )
        .expect_err("failed reply");

        assert_eq!(missing, "not found on the server");
    }

    #[test]
    fn an_unknown_code_falls_back_to_the_servers_message() {
        let err = parse_response(
            r#"{"subsonic-response":{"status":"failed","version":"1.16.1",
               "error":{"code":0,"message":"Something broke."}}}"#,
        )
        .expect_err("failed reply");

        assert_eq!(err, "Something broke.");
    }

    #[test]
    fn a_body_that_isnt_a_subsonic_response_is_an_error() {
        assert!(parse_response(r#"{"hello":"world"}"#).is_err());
        assert!(parse_response("<html>nope</html>").is_err());
    }

    #[test]
    fn signing_appends_a_fresh_token_and_salt() {
        let server = server();
        let url = server.stream_url("sg-1");

        let signed = server.sign(&url);

        assert!(signed.starts_with(&url));
        assert!(signed.contains("&t="));
        assert!(signed.contains("&s="));

        // Never the legacy `p=` password parameter, even though most servers
        // accept it.
        assert!(!signed.contains("p="));
        assert!(!signed.contains("sesame"));

        assert_ne!(signed, server.sign(&url));
    }

    #[test]
    fn a_stream_url_escapes_what_would_break_the_query() {
        let server = Server::new("https://music.example.com", "andrew lake", "sesame");

        let url = server.stream_url("sg 1/2");

        assert!(url.contains("id=sg%201%2F2"));
        assert!(url.contains("u=andrew%20lake"));
    }

    #[test]
    fn stream_headers_are_empty_because_the_url_carries_the_auth() {
        assert!(server().stream_headers().is_empty());
    }

    #[test]
    fn radio_stations_read_off_the_documented_reply() {
        let body = parse_response(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1",
                "internetRadioStations":{"internetRadioStation":[
                    {"id":"1","name":"HBR1.com - Dream Factory",
                     "streamUrl":"http://ubuntu.hbr1.com:19800/ambient.aac",
                     "homePageUrl":"http://www.hbr1.com/"},
                    {"id":"2","name":"Silent","streamUrl":""}
                ]}}}"#,
        )
        .unwrap();

        let stations = radio_stations_of(&body);

        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].id, "1");
        assert_eq!(stations[0].name, "HBR1.com - Dream Factory");
        assert_eq!(
            stations[0].stream_url,
            "http://ubuntu.hbr1.com:19800/ambient.aac"
        );
        assert_eq!(stations[0].home_page, "http://www.hbr1.com/");
    }
}
