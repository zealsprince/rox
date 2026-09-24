//! Listening history per ADR 11: each [`Listened`] from the scrobbler becomes
//! an append-only event row. Listening to the scrobbler rather than the
//! player keeps one listen rule. Appends run on the background executor over
//! their own connection.

use std::path::PathBuf;

use gpui::{Context, Entity, EventEmitter, Subscription};

use rox_library::{listens, store};

use crate::lastfm::{Listened, Scrobbler};

pub enum HistoryEvent {
    Recorded { track_id: i64 },
}

pub struct History {
    db_path: PathBuf,
    _listened: Subscription,
}

impl EventEmitter<HistoryEvent> for History {}

impl History {
    pub fn new(scrobbler: &Entity<Scrobbler>, cx: &mut Context<Self>) -> Self {
        let _listened = cx.subscribe(scrobbler, |this: &mut Self, _, event: &Listened, cx| {
            // Use the event's row id, never re-resolve by path: a cue rip's
            // path resolves to whichever track sorts first.
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
