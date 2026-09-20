//! Subsonic and OpenSubsonic: a catalog on a server the user runs, read
//! over a documented HTTP API. Every request carries the same six
//! parameters and a token that's `md5(password + salt)` with a fresh salt
//! each time, so a URL someone captures off the wire doesn't replay against
//! a different call. That's the auth scheme every server implements, which
//! is why it's the one built first; the `apiKey` parameter newer servers
//! also take is one more branch here whenever a server asks for it.
//!
//! Responses come back wrapped in `subsonic-response`, and the shape inside
//! drifts between Subsonic, Navidrome, Airsonic and gonic, so parsing goes
//! through `serde_json::Value` rather than typed structs. That's the
//! convention the crate's Cargo.toml already names: typed where the shape
//! is stable, untyped where the services wander.
//!
//! Streaming asks for `format=raw`. A server-side transcode would hand the
//! engine re-encoded audio, and gapless and ReplayGain would then be acting
//! on something other than the file the user has. Bandwidth is a real want
//! and a separate decision.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{SourcePlaylist, SourceStation, SourceTrack, number, text, text_of};
use crate::providers::{agent, net_reason};

/// The protocol version rox claims. 1.16.1 is the last Subsonic release's
/// version and covers everything used here; `getArtists` and `search3` have
/// been in since 1.8.0, so nothing older than that will talk to us anyway.
const API_VERSION: &str = "1.16.1";

/// The client name every request identifies itself by. Servers show it in
/// their session lists, so it wants to read as the app, not as a library.
const CLIENT: &str = "rox";

/// What the server said about itself when we pinged it. The three
/// OpenSubsonic fields are absent on a plain Subsonic server, which is the
/// documented way to tell the two apart before asking for extensions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    /// The protocol version the server answers at.
    pub version: String,
    /// The server's own name ("Navidrome", "gonic"), empty on a plain
    /// Subsonic server.
    pub server_type: String,
    pub server_version: String,
    /// Whether the server reports OpenSubsonic support.
    pub open_subsonic: bool,
}

/// One server and the account rox reaches it with. The password is held in
/// the clear because the token is derived per request and the server has no
/// other way to accept us; it lives in `accounts.json`, not in the settings
/// file people hand around.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Server {
    /// Base URL with scheme, no trailing `/rest`. Trailing slashes are
    /// trimmed, so what someone pastes out of a browser works.
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

    /// Salt and token for one request, plus the four parameters every call
    /// carries. A fresh salt per request, so a captured URL isn't
    /// replayable against a different one.
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

    /// The endpoint URL for one method. `.view` rather than the bare name
    /// because the original Subsonic server only serves the suffixed form,
    /// and everything newer accepts both.
    fn endpoint(&self, method: &str) -> String {
        format!("{}/rest/{method}.view", self.url)
    }

    /// One GET, parsed down to the body of `subsonic-response`. Errors come
    /// back already folded to something worth showing.
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

    /// Connectivity and credentials in one call. Ok carries the server's
    /// type and version when it reports them, so the settings row can show
    /// what it connected to.
    pub fn ping(&self) -> Result<ServerInfo, String> {
        let body = self.get("ping", &[])?;

        Ok(server_info(&body))
    }

    /// The whole catalog, artists then albums then songs. `progress` is
    /// called per album so a sync of a large library isn't a silent wait.
    pub fn catalog(&self, progress: impl Fn(usize, usize)) -> Result<Vec<SourceTrack>, String> {
        // Two walks down: the index of artists, then each artist's albums.
        // Only the album call returns songs, so the album list is what the
        // progress count is measured against.
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

    /// The server's own internet radio list (`getInternetRadioStations`,
    /// in the protocol since 1.9.0). A station is a name and a stream URL;
    /// there's no catalog behind it, so the sync hands these to the radio
    /// source rather than to this server's rows.
    pub fn radio_stations(&self) -> Result<Vec<SourceStation>, String> {
        let body = self.get("getInternetRadioStations", &[])?;

        Ok(radio_stations_of(&body))
    }

    /// Cover art bytes for an art id, at a requested size. The server
    /// scales, so asking for what the thumbnail cache wants avoids pulling
    /// a full-resolution scan down for a list row.
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

        // Art comes back as image bytes, but a server that failed answers
        // 200 with a JSON error body, so the content type decides which.
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

    /// Headers for a stream request. Empty today: Subsonic authorizes in
    /// the query string, so the URL carries it. Present because the
    /// registry contract takes headers and a server behind a reverse proxy
    /// may need them later.
    pub fn stream_headers(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// The stream URL for one song id, with everything but the token and
    /// the salt already on it. Those two go on fresh at resolve time, since
    /// a token stored on a row would be a replayable credential sitting in
    /// SQLite.
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

    /// Finish a stored stream URL for playback: the same URL with a fresh
    /// salt and token appended. What the resolve step calls.
    pub fn sign(&self, stream_url: &str) -> String {
        let salt = salt();
        let token = token(&self.password, &salt);

        format!("{stream_url}&t={token}&s={salt}")
    }

    /// The source string every row of this server is keyed under:
    /// "subsonic:" plus a stable digest of the base URL and username, so
    /// two accounts on one server, or one account on two servers, never
    /// collide. The password is deliberately not in the digest, or changing
    /// it would orphan the whole library.
    pub fn source_id(&self) -> String {
        let digest = format!("{:x}", md5::compute(format!("{}\n{}", self.url, self.user)));

        format!("subsonic:{}", &digest[..16])
    }

    /// Every song on one `getAlbum` reply, mapped to the plain shape the
    /// sync consumes. A method rather than a free function because the
    /// stream URL needs the server's base URL and account.
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
                    // Same fallback the scanner uses on an untagged file,
                    // so an album groups the same whichever side it came
                    // from.
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
                    // The API reports whole seconds; the library holds
                    // milliseconds, so an unknown duration stays 0 either
                    // way.
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

/// The token for one request: `md5(password + salt)` as 32 lowercase hex
/// characters, both sides UTF-8. The docs' own example is the test.
pub fn token(password: &str, salt: &str) -> String {
    format!("{:x}", md5::compute(format!("{password}{salt}").as_bytes()))
}

/// A fresh salt for one request. The spec asks for at least six characters
/// and what matters is that it differs per call, not that it's
/// cryptographically strong, since it only has to stop one signed URL from
/// standing in for another. The clock plus a per-process counter gives that
/// without pulling an RNG crate into rox-net for it.
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

/// Percent-encode a query value. Only the characters that would break a
/// query string get escaped, which keeps a song id readable in a log line.
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

/// Unwrap `subsonic-response`, turning a `failed` status into the reason
/// its code means. Every call goes through here, so an expired trial or a
/// rejected password reads the same wherever it surfaces.
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

/// A Subsonic error code folded to a reason worth showing. The four
/// credential codes collapse into one line because they're the same
/// situation from the typist's side: the login didn't take. Everything else
/// keeps the server's own message when it sent one, since those are
/// specific enough to act on.
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

/// What a `ping` reply says about the server. The three OpenSubsonic fields
/// are simply missing on a plain Subsonic server.
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

/// Every artist id on a `getArtists` reply. The index letters are a display
/// concern, so they're flattened away here.
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

/// Every album id on a `getArtist` reply.
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

/// Id and name for every playlist on a `getPlaylists` reply.
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

/// The stations on a `getInternetRadioStations` reply. One with no stream
/// URL is skipped: nothing could play it.
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

/// The song ids on a `getPlaylist` reply, in the order the server holds
/// them, which is the order the playlist is in.
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

/// The container name for a song, which servers report as either a file
/// suffix or a MIME type and sometimes both.
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
        // Straight out of the OpenSubsonic docs: password "sesame" salted
        // with "c19b2d". If this drifts, no server will take us.
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

        // A trailing slash and a changed password are the same library.
        assert_eq!(one.source_id(), same.source_id());

        // Two accounts on one server, and one account on two servers, are
        // not.
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

        // The token and the salt are added at resolve time, never stored.
        assert!(!song.stream_url.contains("&t="));
        assert!(!song.stream_url.contains("&s="));
    }

    #[test]
    fn a_sparse_song_still_maps() {
        // What real servers send constantly: no year, no disc, no genre, no
        // credited album artist, no cover.
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

        // Album artist falls back to the track artist, and the codec comes
        // off the MIME type when there's no suffix.
        assert_eq!(song.album_artist, "Unknown");
        assert_eq!(song.codec, "mpeg");
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

        // The token is what goes on the wire, never the password. The legacy
        // `p=` parameter would work against most servers, which is exactly
        // why it wants pinning shut.
        assert!(!signed.contains("p="));
        assert!(!signed.contains("sesame"));

        // Two signings of one URL differ, which is the point of the salt.
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

    /// The docs' own `getInternetRadioStations` example, plus one entry
    /// with no stream, which nothing could play and so isn't returned.
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
