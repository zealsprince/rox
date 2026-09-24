//! What a lyrics provider is asked for a track, and where a found sheet is
//! saved. A path doesn't name either: a Subsonic song has no file, and a
//! station's row names the station while the song only exists in what the
//! stream announces. [`LyricsTarget`] carries both halves.

use std::path::Path;

use gpui::{App, Entity};

use rox_core::settings::{LyricsSave, Settings, lyrics_dir};
use rox_library::cue::{Origin, TrackKey};
use rox_library::lyrics::{self, Source, Subject};
use rox_net::providers::TrackQuery;
use rox_playback::IcyTitle;

use crate::catalog::Library;

/// Built together because for a station both come from the announced song;
/// off the library row alone they'd name the station.
#[derive(Clone, Debug, PartialEq)]
pub struct LyricsTarget {
    pub subject: Subject,
    pub query: TrackQuery,
}

impl LyricsTarget {
    pub fn file(&self) -> Option<&Path> {
        self.subject.file()
    }

    pub fn label(&self) -> String {
        if self.query.artist.is_empty() {
            self.query.title.clone()
        } else {
            format!("{} - {}", self.query.title, self.query.artist)
        }
    }
}

/// Duration comes off the projection, so the score doesn't depend on the
/// track being the one playing.
pub fn query_for(library: &Entity<Library>, key: &TrackKey, cx: &App) -> TrackQuery {
    let catalog = library.read(cx);
    let resolved = catalog.resolve_key(key);
    let duration_ms = resolved
        .as_ref()
        .and_then(|(id, _)| duration_ms_for(library, *id, cx))
        .unwrap_or(0);
    let meta = resolved.map(|(_, meta)| meta);
    let (artist, title, album) = meta
        .map(|m| (m.artist, m.title, m.album))
        .unwrap_or_default();
    TrackQuery {
        artist,
        title,
        album,
        duration_secs: (duration_ms > 0).then(|| duration_ms as f64 / 1000.0),
    }
}

/// Pass `live` only when this track is the playing station. None for a
/// station that hasn't named a song yet.
pub fn target_for(
    library: &Entity<Library>,
    key: &TrackKey,
    live: Option<&IcyTitle>,
    cx: &App,
) -> Option<LyricsTarget> {
    let mut query = query_for(library, key, cx);

    if let Some(live) = live {
        query.artist = live.artist.clone();
        query.title = live.title.clone();
        // The station's name isn't the song's album, and a stream has no
        // duration.
        query.album = String::new();
        query.duration_secs = None;
    }

    Some(LyricsTarget {
        subject: subject_for(key, live)?,
        query,
    })
}

/// Cheap enough for a playback tick, without the catalog lookup.
pub fn subject_for(key: &TrackKey, live: Option<&IcyTitle>) -> Option<Subject> {
    if key.is_local() {
        return Some(Subject::File(key.path.clone()));
    }

    if matches!(key.origin(), Origin::Radio) {
        let live = live?;

        return Subject::song(&live.artist, &live.title);
    }

    Some(Subject::remote(&key.to_fragment()))
}

pub fn playing_subject(player: &crate::player::Player) -> Option<Subject> {
    let now = player.now_playing()?;
    let live = now.live.then(|| player.live_title()).flatten();

    subject_for(&now.key, live.as_ref())
}

/// Per the Providers page's tag/sidecar/store choice. A subject with no file
/// always goes to the store.
pub fn save_target(subject: &Subject) -> Source {
    let Some(path) = subject.file() else {
        return Source::Store(lyrics::store_file(&lyrics_dir(), subject));
    };

    match Settings::load().accounts.providers.lyrics_save {
        LyricsSave::Tag => Source::Tag,
        LyricsSave::Sidecar => Source::Sidecar(lyrics::default_sidecar(path)),
        LyricsSave::Store => Source::Store(lyrics::store_file(&lyrics_dir(), subject)),
    }
}

fn duration_ms_for(library: &Entity<Library>, id: i64, cx: &App) -> Option<u32> {
    let catalog = library.read(cx);
    let projection = catalog.projection()?;
    let row = (0..projection.len() as u32)
        .find(|&row| projection.db_id[row as usize] == id && !projection.is_dead(row))?;
    Some(projection.resolve(row).duration_ms)
}
