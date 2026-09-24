//! What a station says it's playing. Radio is the one source where the song
//! changes without the queue moving, so this watches the engine's title
//! revision on the pump clock and announces each turnover once. The
//! turnover is also a stream's only end-of-song signal: without it the
//! scrobbler would file one listen for the whole evening.
//!
//! It also writes back what a station's `icy-` headers say about itself
//! into the row's empty columns and looks for a logo at its homepage, once
//! per station per run and never over an existing value.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use gpui::{Context, Entity, EventEmitter, Subscription};

use rox_library::cue::TrackKey;
use rox_library::rusqlite::Connection;
use rox_library::stations::{self, Heard};
use rox_library::store::{self, TrackMeta};
use rox_playback::{IcyTitle, StationInfo};

use crate::catalog::Library;
use crate::player::Player;
use crate::station_art;
use crate::thumbs::Thumbs;

/// The key is the station's, unchanged: same row, different song.
pub struct TitleChanged {
    pub key: TrackKey,
    pub title: IcyTitle,
}

/// Split out of the entity so the turnover rule tests without a clock.
#[derive(Default)]
struct Live {
    key: Option<TrackKey>,
    title: Option<IcyTitle>,
}

impl Live {
    /// `None` for a different track under the cursor, no title yet, or the
    /// same title again.
    fn advance(&mut self, key: &TrackKey, title: Option<IcyTitle>) -> Option<IcyTitle> {
        // A different entry took over. Its opening title isn't a turnover:
        // the stream only just started.
        if self.key.as_ref() != Some(key) {
            self.key = Some(key.clone());
            self.title = title;
            return None;
        }

        // Stations resend the title several times a minute; a keepalive
        // that scrobbled would double every play.
        let title = title?;
        if self.title.as_ref() == Some(&title) {
            return None;
        }

        self.title = Some(title.clone());
        Some(title)
    }

    /// True when there was something to forget, so the caller notifies.
    fn clear(&mut self) -> bool {
        let held = self.key.is_some() || self.title.is_some();
        self.key = None;
        self.title = None;
        held
    }
}

pub struct Radio {
    live: Live,
    /// Polling the revision atomic is free; taking the title lock every tick
    /// is not.
    seen_rev: u64,
    library: Entity<Library>,
    thumbs: Entity<Thumbs>,
    /// Stream URLs already described this run. One attempt each, or a station
    /// with no favicon is re-fetched on every play.
    described: HashSet<String>,
    _player_changed: Subscription,
}

impl EventEmitter<TitleChanged> for Radio {}

impl Radio {
    pub fn new(
        player: &Entity<Player>,
        library: &Entity<Library>,
        thumbs: &Entity<Thumbs>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _player_changed = cx.observe(player, |this: &mut Self, player, cx| {
            this.tick(&player, cx);
        });

        Radio {
            live: Live::default(),
            seen_rev: 0,
            library: library.clone(),
            thumbs: thumbs.clone(),
            described: HashSet::new(),
            _player_changed,
        }
    }

    /// Shown in place of the row's title, which names the station.
    pub fn live_title(&self) -> Option<IcyTitle> {
        self.live.title.clone()
    }

    /// So a reader can tell a stale title from one about its track.
    pub fn station(&self) -> Option<&TrackKey> {
        self.live.key.as_ref()
    }

    fn tick(&mut self, player: &Entity<Player>, cx: &mut Context<Self>) {
        let player = player.read(cx);

        let Some(now) = player.now_playing() else {
            if self.live.clear() {
                cx.notify();
            }
            return;
        };

        let rev = player.title_rev().unwrap_or(0);
        let switched = self.live.key.as_ref() != Some(&now.key);
        if rev == self.seen_rev && !switched {
            return;
        }
        self.seen_rev = rev;

        // Read both before acting: what follows takes the app mutably.
        let info = player.station_info();
        let title = player.live_title();

        // The description rides the same revision. Mark the station before
        // the library write and the network run.
        if let Some(info) = info {
            self.describe(&now.key, &info, cx);
        }

        if let Some(title) = self.live.advance(&now.key, title) {
            cx.emit(TitleChanged {
                key: now.key,
                title,
            });
            cx.notify();
        }
    }

    /// Stations only: the row write and the art key are scoped by source.
    fn describe(&mut self, key: &TrackKey, info: &StationInfo, cx: &mut Context<Self>) {
        if &*key.source != stations::SOURCE {
            return;
        }

        let url = key.path.to_string_lossy().to_string();
        if !self.described.insert(url.clone()) {
            return;
        }

        self.fill_row(&url, info, cx);
        self.find_logo(&url, info, cx);
    }

    /// Fills only the empty genre, codec, and bitrate columns, and reloads the
    /// projection when something landed.
    fn fill_row(&mut self, url: &str, info: &StationInfo, cx: &mut Context<Self>) {
        let heard = heard_from(info);
        if heard == Heard::default() {
            return;
        }

        let path = self.library.read(cx).db_path();
        let Ok(conn) = store::open(&path) else {
            return;
        };

        match stations::fill_empty(&conn, url, &heard) {
            Ok(true) => self
                .library
                .update(cx, |library, cx| library.reload_projection(cx)),

            Ok(false) => {}

            Err(e) => log::warn!("radio: recording what {url} said failed: {e}"),
        }
    }

    /// Only for a station with no stored picture; a directory add already has
    /// one. Silent either way: `/favicon.ico` is only a guess.
    fn find_logo(&mut self, url: &str, info: &StationInfo, cx: &mut Context<Self>) {
        let Some(favicon) = station_art::favicon_url(&info.homepage) else {
            return;
        };
        let Some(conn) = self.thumbs.read(cx).store_conn() else {
            return;
        };

        let key = url.to_string();
        let thumbs = self.thumbs.clone();

        // The row likely painted before the fetch and cached "no art" as
        // definitive, so tell the texture cache to look again.
        cx.spawn(async move |_, cx| {
            let stored = cx
                .background_executor()
                .spawn(async move {
                    if has_art(&conn, &key) {
                        return None;
                    }

                    station_art::fetch_and_store(&favicon, &key, &conn).then_some(key)
                })
                .await;

            let Some(key) = stored else {
                return;
            };

            thumbs
                .update(cx, |thumbs, cx| {
                    thumbs.forget(Path::new(&key), cx);
                })
                .ok();
        })
        .detach();
    }
}

/// Takes the store lock. Background executor only.
fn has_art(conn: &Mutex<Connection>, key: &str) -> bool {
    rox_library::thumbs::thumbnail(conn, Path::new(key)).is_some()
}

/// The codec goes through the probe's own mapping, so the row records the
/// container the transport decodes as.
fn heard_from(info: &StationInfo) -> Heard {
    Heard {
        genre: info.genre.trim().to_string(),
        codec: rox_playback::http::extension_for(&info.content_type)
            .unwrap_or_default()
            .to_string(),
        bitrate_kbps: info.bitrate_kbps,
    }
}

/// The song's artist and title over the station's row, with the station's
/// name as the album. The row id stays the station's. A station sending one
/// unsplittable field leaves the artist empty, so Last.fm skips it.
pub fn live_tags(station: Option<TrackMeta>, title: &IcyTitle) -> TrackMeta {
    let mut meta = station.unwrap_or(TrackMeta {
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        track_no: 0,
        album_artist: String::new(),
        year: 0,
        genre: String::new(),
        duration_ms: 0,
        codec: String::new(),
        bitrate_kbps: 0,
        sample_rate_hz: 0,
        bit_depth: 0,
        rating: 0,
    });

    meta.album = std::mem::take(&mut meta.title);
    meta.title = title.title.clone();
    meta.artist = title.artist.clone();
    // A length carried over off the row would be a lie the listen rule divides by.
    meta.duration_ms = 0;

    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn station_key() -> TrackKey {
        TrackKey {
            source: rox_library::cue::source_id(rox_library::stations::SOURCE),
            path: PathBuf::from("https://host/jazz"),
            sub: 0,
        }
    }

    fn title(artist: &str, name: &str) -> IcyTitle {
        IcyTitle {
            artist: artist.to_string(),
            title: name.to_string(),
        }
    }

    fn station_meta(name: &str) -> TrackMeta {
        TrackMeta {
            title: name.to_string(),
            artist: String::new(),
            album: String::new(),
            track_no: 0,
            album_artist: String::new(),
            year: 0,
            genre: "Jazz".into(),
            duration_ms: 0,
            codec: String::new(),
            bitrate_kbps: 0,
            sample_rate_hz: 0,
            bit_depth: 0,
            rating: 0,
        }
    }

    #[test]
    fn a_turnover_announces_once() {
        let mut live = Live::default();
        let key = station_key();

        assert_eq!(live.advance(&key, None), None, "the stream just opened");
        assert_eq!(
            live.advance(&key, Some(title("Miles Davis", "So What"))),
            Some(title("Miles Davis", "So What")),
            "the first announcement is the song starting"
        );
        assert_eq!(
            live.advance(&key, Some(title("Bill Evans", "Peace Piece"))),
            Some(title("Bill Evans", "Peace Piece"))
        );
    }

    #[test]
    fn the_same_title_twice_announces_once() {
        let mut live = Live::default();
        let key = station_key();
        let on_air = title("Miles Davis", "So What");

        live.advance(&key, None);
        assert_eq!(
            live.advance(&key, Some(on_air.clone())),
            Some(on_air.clone())
        );
        assert_eq!(live.advance(&key, Some(on_air.clone())), None);
        assert_eq!(live.advance(&key, Some(on_air)), None);
    }

    /// A track change is the queue's event; announcing it here would re-arm a
    /// watch that just began.
    #[test]
    fn a_track_change_is_not_a_turnover() {
        let mut live = Live::default();
        let key = station_key();
        live.advance(&key, Some(title("Miles Davis", "So What")));

        let other = TrackKey::from(PathBuf::from("/music/track.flac"));
        assert_eq!(live.advance(&other, Some(title("A", "B"))), None);
        assert_eq!(live.live_title_for_test(), Some(title("A", "B")));

        assert_eq!(live.advance(&key, None), None);
        assert_eq!(live.live_title_for_test(), None);
    }

    #[test]
    fn a_stopped_player_forgets_the_station() {
        let mut live = Live::default();
        let key = station_key();
        live.advance(&key, Some(title("Miles Davis", "So What")));

        assert!(live.clear(), "there was a station to forget");
        assert!(!live.clear(), "and nothing to forget twice");

        assert_eq!(live.advance(&key, None), None);
        assert_eq!(
            live.advance(&key, Some(title("Miles Davis", "So What"))),
            Some(title("Miles Davis", "So What"))
        );
    }

    #[test]
    fn a_turnover_records_the_new_song_under_the_station() {
        let station = station_meta("Jazz Forever");
        let tags = live_tags(Some(station), &title("Miles Davis", "So What"));

        assert_eq!(tags.title, "So What");
        assert_eq!(tags.artist, "Miles Davis");
        assert_eq!(tags.album, "Jazz Forever", "the station stands in");
        assert_eq!(tags.genre, "Jazz", "the row's other tags carry over");
        assert_eq!(tags.duration_ms, 0, "a stream still has no length");
    }

    #[test]
    fn a_turnover_without_a_row_still_carries_the_song() {
        let tags = live_tags(None, &title("Miles Davis", "So What"));

        assert_eq!(tags.title, "So What");
        assert_eq!(tags.artist, "Miles Davis");
        assert!(tags.album.is_empty());
    }

    fn described(content_type: &str) -> Heard {
        heard_from(&StationInfo {
            genre: " Jazz ".into(),
            bitrate_kbps: 128,
            content_type: content_type.into(),
            ..StationInfo::default()
        })
    }

    /// Through the probe's own mapping, never a second table that could
    /// disagree with it.
    #[test]
    fn the_content_type_names_the_codec() {
        assert_eq!(described("audio/mpeg").codec, "mp3");
        assert_eq!(described("audio/mpeg; charset=UTF-8").codec, "mp3");
        assert_eq!(described("audio/aac").codec, "aac");
        assert_eq!(described("audio/aacp").codec, "aac");
        assert_eq!(described("application/ogg").codec, "ogg");
        assert_eq!(described("audio/ogg").codec, "ogg");

        assert_eq!(described("application/octet-stream").codec, "");
        assert_eq!(described("").codec, "");
    }

    #[test]
    fn a_station_that_says_nothing_has_nothing_to_record() {
        let heard = described("audio/mpeg");
        assert_eq!(heard.genre, "Jazz");
        assert_eq!(heard.bitrate_kbps, 128);

        assert_eq!(heard_from(&StationInfo::default()), Heard::default());
    }

    impl Live {
        fn live_title_for_test(&self) -> Option<IcyTitle> {
            self.title.clone()
        }
    }
}
