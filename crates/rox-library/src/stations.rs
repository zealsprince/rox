//! Web radio stations as ordinary track rows under `source = 'radio'`, stream
//! URL in both `path` and `remote_url`, `remote_live` set. That gets the
//! queue, playlists, history and search for free; a side table would have
//! meant a second play path.
//!
//! No duration, gapless boundary or ReplayGain. Codec and bitrate come off
//! the response headers, so they stay empty until [`fill_empty`] writes
//! them. The URL is the identity.

use std::collections::HashSet;

use rusqlite::Connection;

use crate::TrackRow;
use crate::locator::{Locator, Remote};
use crate::playlist_file::Format;
use crate::store;

/// One source for every station: there's no account to tell them apart.
pub const SOURCE: &str = "radio";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Station {
    pub url: String,
    pub name: String,
    pub genre: String,
}

/// The columns [`fill_empty`] writes once something has connected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    pub genre: String,
    pub codec: String,
    pub bitrate_kbps: u32,
}

/// Add or update stations. A nameless one takes the URL's last segment, via
/// [`Locator::label`].
pub fn put(conn: &mut Connection, stations: &[Station]) -> rusqlite::Result<()> {
    let rows: Vec<TrackRow> = stations.iter().map(row_for).collect();

    store::upsert_source_rows(conn, SOURCE, &rows)
}

/// Through [`store::prune_source`], which is scoped to the source, so a URL
/// that reads like a local path can't reach a local row.
pub fn remove(conn: &mut Connection, url: &str) -> rusqlite::Result<()> {
    let keep: HashSet<String> = all(conn)?
        .into_iter()
        .map(|station| station.url)
        .filter(|held| held != url)
        .collect();

    store::prune_source(conn, SOURCE, &keep)?;
    Ok(())
}

/// By name, URL breaking ties.
pub fn all(conn: &Connection) -> rusqlite::Result<Vec<Station>> {
    let mut stmt = conn.prepare(
        "SELECT path, title, genre FROM tracks
          WHERE source = ?1 ORDER BY title, path",
    )?;

    let rows = stmt.query_map([SOURCE], |r| {
        Ok(Station {
            url: r.get(0)?,
            name: r.get(1)?,
            genre: r.get(2)?,
        })
    })?;

    rows.collect()
}

/// [`all`] plus what's known about each stream.
pub fn detailed(conn: &Connection) -> rusqlite::Result<Vec<(Station, Heard)>> {
    let mut stmt = conn.prepare(
        "SELECT path, title, genre, codec, bitrate FROM tracks
          WHERE source = ?1 ORDER BY title, path",
    )?;

    let rows = stmt.query_map([SOURCE], |r| {
        Ok((
            Station {
                url: r.get(0)?,
                name: r.get(1)?,
                genre: r.get(2)?,
            },
            Heard {
                genre: r.get(2)?,
                codec: r.get(3)?,
                bitrate_kbps: r.get(4)?,
            },
        ))
    })?;

    rows.collect()
}

/// Fill only the empty columns: a typed name or chosen genre must survive a
/// station announcing "Various" on every connect. True when something landed.
pub fn fill_empty(conn: &Connection, url: &str, heard: &Heard) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE tracks
            SET genre = CASE WHEN genre = '' THEN ?3 ELSE genre END,
                codec = CASE WHEN codec = '' THEN ?4 ELSE codec END,
                bitrate = CASE WHEN bitrate = 0 THEN ?5 ELSE bitrate END
          WHERE source = ?1 AND path = ?2
            AND ((genre = '' AND ?3 <> '')
              OR (codec = '' AND ?4 <> '')
              OR (bitrate = 0 AND ?5 <> 0))",
        rusqlite::params![SOURCE, url, heard.genre, heard.codec, heard.bitrate_kbps],
    )?;

    Ok(changed > 0)
}

/// One reason per thing a person pastes, so the add box can say what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Stations open over HTTP and nothing else.
    Scheme,

    /// HLS: a manifest of segments, while the transport reads one byte stream.
    Hls,

    /// A list of stations; the answer is [`import`].
    Playlist,
}

/// About the string alone; whether the far end serves audio is `rox-net`'s
/// question. The suffix is read off the path, never the query.
pub fn refusal(url: &str) -> Option<Refusal> {
    let lower = url.trim().to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Some(Refusal::Scheme);
    }

    let path = lower.split(['?', '#']).next().unwrap_or(&lower);

    if path.ends_with(".m3u8") {
        return Some(Refusal::Hls);
    }

    if path.ends_with(".m3u") || path.ends_with(".pls") {
        return Some(Refusal::Playlist);
    }

    None
}

/// Stations from a `.pls` or `.m3u`. Its own walk, since the shared readers
/// drop the names. Anything [`refusal`] rejects is dropped, so a playlist of
/// local files imports as nothing.
pub fn import(text: &str) -> Vec<Station> {
    // A leading BOM would cling to the first line.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    match Format::sniff(text) {
        Format::Pls => from_pls(text),

        Format::M3u => from_m3u(text),

        // XSPF titles live in the XML; take the shared reader's URLs.
        Format::Xspf => crate::xspf::parse(text)
            .iter()
            .filter_map(|url| station(url, ""))
            .collect(),
    }
}

/// A title only counts for the next entry, so a dropped local path never
/// lends its name onward.
fn from_m3u(text: &str) -> Vec<Station> {
    let mut out = Vec::new();
    let mut pending = String::new();

    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            pending = rest
                .split_once(',')
                .map(|(_, name)| name.trim().to_string())
                .unwrap_or_default();
            continue;
        }

        if line.starts_with('#') {
            continue;
        }

        out.extend(station(line, &pending));
        pending.clear();
    }

    out
}

/// In entry-number order, which the format says to trust over line order.
fn from_pls(text: &str) -> Vec<Station> {
    let mut urls: Vec<(u32, String)> = Vec::new();
    let mut names: Vec<(u32, String)> = Vec::new();

    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };

        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }

        // Winamp-era files mix File1, file1 and FILE1.
        if let Some(index) = numbered(key, "file") {
            urls.push((index, value.to_string()));
        } else if let Some(index) = numbered(key, "title") {
            names.push((index, value.to_string()));
        }
    }

    urls.sort_by_key(|(index, _)| *index);

    urls.iter()
        .filter_map(|(index, url)| {
            let name = names
                .iter()
                .find(|(at, _)| at == index)
                .map(|(_, name)| name.as_str())
                .unwrap_or("");

            station(url, name)
        })
        .collect()
}

fn numbered(key: &str, prefix: &str) -> Option<u32> {
    let head = key.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(prefix) {
        return None;
    }

    key[prefix.len()..].trim().parse().ok()
}

/// Drops the refusal reason: a forty-line file can't explain each, and the
/// import notice reports the count.
fn station(url: &str, name: &str) -> Option<Station> {
    if refusal(url).is_some() {
        return None;
    }

    Some(Station {
        url: url.to_string(),
        name: name.trim().to_string(),
        genre: String::new(),
    })
}

/// An empty album keeps stations out of the distinct-album rollups.
fn row_for(station: &Station) -> TrackRow {
    let title = if station.name.trim().is_empty() {
        label_of(&station.url)
    } else {
        station.name.trim().to_string()
    };

    TrackRow {
        path: station.url.clone(),
        sub: 0,
        cue: None,
        remote_url: station.url.clone(),
        remote_live: true,
        title,
        artist: String::new(),
        album_artist: String::new(),
        album: String::new(),
        title_sort: String::new(),
        artist_sort: String::new(),
        album_artist_sort: String::new(),
        album_sort: String::new(),
        genre: station.genre.clone(),
        year: 0,
        disc_no: 0,
        track_no: 0,
        duration_ms: 0,
        codec: String::new(),
        bitrate_kbps: 0,
        sample_rate_hz: 0,
        bit_depth: 0,
        rating: 0,
        replay_gain: Default::default(),
        bpm: None,
        size: 0,
        mtime: 0,
    }
}

fn label_of(url: &str) -> String {
    Locator::Remote(Remote {
        url: url.to_string(),
        headers: Vec::new(),
        hint: String::new(),
        live: true,
    })
    .label()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        conn
    }

    fn station_named(url: &str, name: &str) -> Station {
        Station {
            url: url.to_string(),
            name: name.to_string(),
            genre: String::new(),
        }
    }

    #[test]
    fn the_same_url_twice_is_one_row_with_the_newer_name() {
        let mut conn = db();

        put(&mut conn, &[station_named("https://host/jazz", "Jazz")]).unwrap();
        put(
            &mut conn,
            &[station_named("https://host/jazz", "Jazz Forever")],
        )
        .unwrap();

        let held = all(&conn).unwrap();
        assert_eq!(held.len(), 1, "one row per URL");
        assert_eq!(held[0].name, "Jazz Forever");
    }

    #[test]
    fn remove_takes_only_its_own_row() {
        let mut conn = db();

        put(
            &mut conn,
            &[
                station_named("https://host/jazz", "Jazz"),
                station_named("https://host/soul", "Soul"),
            ],
        )
        .unwrap();

        remove(&mut conn, "https://host/jazz").unwrap();

        let held = all(&conn).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].url, "https://host/soul");
    }

    #[test]
    fn a_station_and_a_local_file_can_share_a_path_string() {
        let mut conn = db();

        let shared = "https://host/jazz";
        store::insert_batch(
            &mut conn,
            &[TrackRow {
                title: "A local file that happens to be named this".into(),
                ..row_for(&station_named(shared, "Jazz"))
            }],
        )
        .unwrap();

        put(&mut conn, &[station_named(shared, "Jazz")]).unwrap();

        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tracks WHERE path = ?1",
                [shared],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 2, "one row per source");

        let held = all(&conn).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].name, "Jazz");
    }

    #[test]
    fn a_station_row_is_live_and_lengthless() {
        let mut conn = db();

        put(&mut conn, &[station_named("https://host/jazz", "Jazz")]).unwrap();

        let id = store::id_for_path(&conn, SOURCE, "https://host/jazz")
            .unwrap()
            .unwrap();
        let locator = store::locators_for(&conn, &[id]).unwrap().pop().unwrap();

        assert_eq!(
            locator,
            Locator::Remote(Remote {
                url: "https://host/jazz".into(),
                headers: Vec::new(),
                hint: String::new(),
                live: true,
            })
        );

        let duration: i64 = conn
            .query_row("SELECT duration_ms FROM tracks WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(duration, 0);
    }

    /// Pins where stations land in the rollups: the folder counts are local
    /// only, the whole-library counts include them. Scoping those is store.rs's
    /// call.
    #[test]
    fn a_station_in_the_library_rollups() {
        let mut conn = db();

        store::insert_batch(
            &mut conn,
            &[TrackRow {
                path: "/music/one.flac".into(),
                album: "An Album".into(),
                size: 1_000,
                ..row_for(&station_named("https://host/jazz", "Jazz"))
            }],
        )
        .unwrap();
        put(&mut conn, &[station_named("https://host/jazz", "Jazz")]).unwrap();

        let stats = store::stats(&conn).unwrap();
        assert_eq!(stats.dirs, 1, "a station has no folder to watch");
        assert_eq!(stats.albums, 1, "and no album to count");
        assert_eq!(stats.bytes, 1_000, "and no bytes on disk");
        assert_eq!(stats.tracks, 2, "but it does count as a track");

        assert_eq!(store::replaygain_breakdown(&conn).unwrap().missing, 2);
        assert_eq!(store::bpm_breakdown(&conn).unwrap().missing, 2);
    }

    #[test]
    fn an_unnamed_station_falls_back_to_the_url() {
        let mut conn = db();

        put(&mut conn, &[station_named("https://host/radio/jazz", "")]).unwrap();

        assert_eq!(all(&conn).unwrap()[0].name, "jazz");
    }

    #[test]
    fn import_reads_a_pls() {
        let text = "[playlist]\n\
                    NumberOfEntries=2\n\
                    File1=https://host/jazz\n\
                    Title1=Jazz Forever\n\
                    Length1=-1\n\
                    File2=https://host/soul\n\
                    Title2=Soul Kitchen\n\
                    Length2=-1\n\
                    Version=2\n";

        assert_eq!(
            import(text),
            vec![
                station_named("https://host/jazz", "Jazz Forever"),
                station_named("https://host/soul", "Soul Kitchen"),
            ]
        );
    }

    #[test]
    fn import_reads_an_m3u() {
        let text = "#EXTM3U\n\
                    #EXTINF:-1,Jazz Forever\n\
                    https://host/jazz\n\
                    #EXTINF:-1,Soul Kitchen\n\
                    https://host/soul\n";

        assert_eq!(
            import(text),
            vec![
                station_named("https://host/jazz", "Jazz Forever"),
                station_named("https://host/soul", "Soul Kitchen"),
            ]
        );
    }

    #[test]
    fn import_rejects_everything_that_is_not_a_stream() {
        let mixed = "#EXTM3U\n\
                     #EXTINF:210,A Local Song\n\
                     /music/artist/song.flac\n\
                     #EXTINF:-1,Jazz Forever\n\
                     https://host/jazz\n\
                     #EXTINF:-1,Old Windows Stream\n\
                     mms://host/legacy\n";

        assert_eq!(
            import(mixed),
            vec![station_named("https://host/jazz", "Jazz Forever")]
        );

        let all_local = "/music/one.flac\n/music/two.flac\n";
        assert!(import(all_local).is_empty());

        let pls_local = "[playlist]\nFile1=C:\\Music\\one.mp3\nTitle1=One\n";
        assert!(import(pls_local).is_empty());
    }

    /// Plenty of mounts carry a listener token in the query.
    #[test]
    fn an_ordinary_stream_url_is_not_refused() {
        assert_eq!(refusal("https://host/jazz"), None);
        assert_eq!(refusal("http://host:8000/live"), None);
        assert_eq!(refusal("https://host/live?session=.m3u"), None);
    }

    #[test]
    fn a_url_that_is_not_http_is_refused_for_its_scheme() {
        assert_eq!(refusal("mms://host/legacy"), Some(Refusal::Scheme));
        assert_eq!(refusal("/music/one.flac"), Some(Refusal::Scheme));
        assert_eq!(refusal("file:///music/one.flac"), Some(Refusal::Scheme));
    }

    #[test]
    fn an_m3u8_is_refused_as_hls() {
        assert_eq!(refusal("https://host/live.m3u8"), Some(Refusal::Hls));
        assert_eq!(refusal("HTTPS://HOST/LIVE.M3U8"), Some(Refusal::Hls));
        assert_eq!(refusal("https://host/live.m3u8?t=9"), Some(Refusal::Hls));
    }

    #[test]
    fn a_playlist_url_is_refused_as_a_playlist() {
        assert_eq!(
            refusal("https://host/stations.m3u"),
            Some(Refusal::Playlist)
        );
        assert_eq!(
            refusal("https://host/stations.pls"),
            Some(Refusal::Playlist)
        );
        assert_eq!(
            refusal("https://host/stations.PLS?v=2#top"),
            Some(Refusal::Playlist)
        );
    }

    #[test]
    fn import_drops_playlists_and_hls_the_way_the_add_box_does() {
        let nested = "#EXTM3U\n\
                      #EXTINF:-1,Somebody's Station List\n\
                      https://host/stations.pls\n\
                      #EXTINF:-1,A Segmented One\n\
                      https://host/live.m3u8\n\
                      #EXTINF:-1,Jazz Forever\n\
                      https://host/jazz\n";

        assert_eq!(
            import(nested),
            vec![station_named("https://host/jazz", "Jazz Forever")]
        );
    }

    #[test]
    fn imported_stations_land_as_rows() {
        let mut conn = db();

        let stations = import("#EXTM3U\n#EXTINF:-1,Jazz Forever\nhttps://host/jazz\n");
        put(&mut conn, &stations).unwrap();

        assert_eq!(
            all(&conn).unwrap(),
            vec![station_named("https://host/jazz", "Jazz Forever")]
        );
    }

    #[test]
    fn the_stream_fills_the_columns_the_row_left_empty() {
        let mut conn = db();
        put(
            &mut conn,
            &[station_named("https://host/jazz", "Jazz Forever")],
        )
        .unwrap();

        let heard = Heard {
            genre: "Jazz".into(),
            codec: "mp3".into(),
            bitrate_kbps: 128,
        };
        assert!(fill_empty(&conn, "https://host/jazz", &heard).unwrap());

        let (station, filled) = detailed(&conn).unwrap().pop().expect("the one station");
        assert_eq!(station.genre, "Jazz");
        assert_eq!(filled, heard);

        // Nothing left to fill, so no projection rebuild.
        assert!(!fill_empty(&conn, "https://host/jazz", &heard).unwrap());
    }

    #[test]
    fn what_the_row_already_says_survives_the_stream() {
        let mut conn = db();
        put(
            &mut conn,
            &[Station {
                url: "https://host/jazz".into(),
                name: "Jazz Forever".into(),
                genre: "Bebop".into(),
            }],
        )
        .unwrap();

        let heard = Heard {
            genre: "Various".into(),
            codec: "mp3".into(),
            bitrate_kbps: 128,
        };
        assert!(fill_empty(&conn, "https://host/jazz", &heard).unwrap());

        let (station, filled) = detailed(&conn).unwrap().pop().expect("the one station");
        assert_eq!(station.genre, "Bebop", "the typed genre stands");
        assert_eq!(filled.codec, "mp3", "and the empty columns still fill");
        assert_eq!(filled.bitrate_kbps, 128);
    }

    #[test]
    fn a_station_that_describes_nothing_writes_nothing() {
        let mut conn = db();
        put(
            &mut conn,
            &[station_named("https://host/jazz", "Jazz Forever")],
        )
        .unwrap();

        assert!(!fill_empty(&conn, "https://host/jazz", &Heard::default()).unwrap());
        assert!(
            !fill_empty(
                &conn,
                "https://host/other",
                &Heard {
                    genre: "Jazz".into(),
                    codec: "mp3".into(),
                    bitrate_kbps: 128,
                }
            )
            .unwrap(),
            "and a URL no row holds is nobody's station"
        );
    }
}
