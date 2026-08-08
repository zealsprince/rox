//! Listening history recording per ADR 11: the scrobbler's listen
//! signal lands as an append-only event row in the library database.
//! The recorder rides the scrobbler's [`Listened`] event rather than
//! watching the player itself, so it inherits the fixed listen rule
//! (half the track or four minutes, minimum length, seeks and pauses
//! don't count) without re-deriving it from the position clock. Appends
//! run on the background executor over their own connection, like the
//! scans, so recording never touches the audio path or holds up a
//! frame; history views and the stats window subscribe for the refresh.

use std::path::PathBuf;

use gpui::{Context, Entity, EventEmitter, Subscription};

use rox_library::{listens, store};

use crate::lastfm::{Listened, Scrobbler};

/// A listen landed on disk; history views re-query, and the library
/// bumps the track's cached play count in place.
pub enum HistoryEvent {
    Recorded { track_id: i64 },
}

/// The recorder entity, one per workspace beside its scrobbler.
pub struct History {
    db_path: PathBuf,
    _listened: Subscription,
}

impl EventEmitter<HistoryEvent> for History {}

impl History {
    pub fn new(scrobbler: &Entity<Scrobbler>, cx: &mut Context<Self>) -> Self {
        let _listened = cx.subscribe(scrobbler, |this: &mut Self, _, event: &Listened, cx| {
            // The event already carries the row it resolved to and the tags
            // to snapshot, so nothing here has to ask the database who
            // played. That matters for a cue rip: the path would answer with
            // whichever track of the disc sorts first, every single time.
            let Some(track_id) = event.track_id else {
                return;
            };
            let listen = listens::Listen {
                track_id,
                played_at: event.started as i64,
                title: event.title.clone(),
                artist: event.artist.clone(),
                album: event.album.clone(),
                genre: event.genre.clone(),
                path: event.key.to_fragment(),
            };
            this.record(listen, cx);
        });
        History {
            db_path: rox_core::settings::data_dir().join("library.db"),
            _listened,
        }
    }

    /// Append one listen off the UI thread. A file outside the library never
    /// gets here - events key to track identity, and the scrobbler drops the
    /// id for one it couldn't resolve. Failures log and never touch playback,
    /// like the scrobbler's own submissions.
    fn record(&self, listen: listens::Listen, cx: &mut Context<Self>) {
        let db_path = self.db_path.clone();
        cx.spawn(async move |this, cx| {
            let recorded = cx
                .background_executor()
                .spawn(async move {
                    let conn = store::open(&db_path).map_err(|e| e.to_string())?;
                    listens::append(&conn, &listen).map_err(|e| e.to_string())?;
                    Ok::<i64, String>(listen.track_id)
                })
                .await;
            this.update(cx, |_, cx| match recorded {
                Ok(track_id) => cx.emit(HistoryEvent::Recorded { track_id }),
                Err(e) => log::warn!("history: {e}"),
            })
            .ok();
        })
        .detach();
    }
}
