//! Listening history per ADR 11: each [`Listened`] from the scrobbler becomes
//! an append-only event row. Listening to the scrobbler rather than the
//! player keeps one listen rule. Appends run on the background executor over
//! their own connection.

use std::path::PathBuf;

use gpui::{Context, Entity, EventEmitter, Subscription};

use rox_library::{listens, store};

use crate::lastfm::{Listened, Scrobbler};

pub enum HistoryEvent {
    /// `claimed` when the listen's plugin track took over a hearted Unknown
    /// row ([`crate::unknown::claim`]), so its listens and heart moved too.
    Recorded { track_id: i64, claimed: bool },
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
                    let mut conn = store::open(&db_path).map_err(|e| e.to_string())?;
                    listens::append(&conn, &listen).map_err(|e| e.to_string())?;

                    // The listen is in either way; a failed claim waits for the
                    // next play or load.
                    let claimed =
                        crate::unknown::claim(&mut conn, listen.track_id).unwrap_or_else(|e| {
                            log::warn!("history: moving unknown hearts onto a plugin track: {e}");
                            false
                        });
                    Ok::<_, String>((listen.track_id, claimed))
                })
                .await;
            this.update(cx, |_, cx| match recorded {
                Ok((track_id, claimed)) => cx.emit(HistoryEvent::Recorded { track_id, claimed }),
                Err(e) => log::warn!("history: {e}"),
            })
            .ok();
        })
        .detach();
    }
}
