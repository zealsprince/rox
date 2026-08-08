//! The two catalog-shaped pieces the lyrics matcher and the lyrics panel
//! both need: what a provider gets asked for a track, and where a found
//! sheet lands. Neither renders anything, so both sit down here where the
//! panel and the matcher window can each reach them.

use std::path::Path;

use gpui::{App, Entity};

use rox_core::settings::{lyrics_dir, LyricsSave, Settings};
use rox_library::cue::TrackKey;
use rox_library::lyrics::{self, Source};
use rox_net::providers::TrackQuery;

use crate::catalog::Library;

/// The provider query for a track: its tags off the catalog, and its
/// duration off the projection so the score doesn't depend on the track
/// being the one playing.
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

/// Where a saved sheet lands, per the Providers page's tag/sidecar/store
/// choice. Shared by the matcher's Apply and the panel's auto-search so
/// both honor the one destination setting.
pub fn save_target(path: &Path) -> Source {
    match Settings::load().accounts.providers.lyrics_save {
        LyricsSave::Tag => Source::Tag,
        LyricsSave::Sidecar => Source::Sidecar(lyrics::default_sidecar(path)),
        LyricsSave::Store => Source::Store(lyrics::store_file(&lyrics_dir(), path)),
    }
}

/// The track's duration in ms off the projection, resolved from its id.
fn duration_ms_for(library: &Entity<Library>, id: i64, cx: &App) -> Option<u32> {
    let catalog = library.read(cx);
    let projection = catalog.projection()?;
    let row = projection.db_id.iter().position(|&db| db == id)?;
    Some(projection.resolve(row as u32).duration_ms)
}
