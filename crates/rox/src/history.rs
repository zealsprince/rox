//! Listening history recording per ADR 11: the scrobbler's listen
//! signal lands as an append-only event row in the library database.
//! The recorder shares the scrobbler's qualification (played-time
//! threshold, minimum length, seeks don't count) by riding its
//! [`Listened`] event rather than watching the player itself. Appends
//! run on the background executor over their own connection, like the
//! scans, so recording never touches the audio path or holds up a
//! frame; history views and the stats window subscribe for the refresh.

use std::path::PathBuf;

use gpui::{Context, Entity, EventEmitter, Subscription};

use rox_library::{listens, store};

use crate::lastfm::{Listened, Scrobbler};

/// A listen landed on disk; history views re-query.
pub enum HistoryEvent {
    Recorded,
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
            this.record(event.path.clone(), event.started, cx);
        });
        History {
            db_path: crate::settings::data_dir().join("library.db"),
            _listened,
        }
    }

    /// Append one listen off the UI thread: resolve the track and its
    /// tag snapshot, insert the event row. A file outside the library
    /// records nothing - events key to track identity. Failures log and
    /// never touch playback, like the scrobbler's own submissions.
    fn record(&self, path: PathBuf, started: u64, cx: &mut Context<Self>) {
        let db_path = self.db_path.clone();
        cx.spawn(async move |this, cx| {
            let recorded = cx
                .background_executor()
                .spawn(async move {
                    let conn = store::open(&db_path).map_err(|e| e.to_string())?;
                    let Some(path) = path.to_str() else {
                        return Ok(false);
                    };
                    let Some(listen) = listens::listen_for_path(&conn, path, started as i64)
                        .map_err(|e| e.to_string())?
                    else {
                        return Ok(false);
                    };
                    listens::append(&conn, &listen).map_err(|e| e.to_string())?;
                    Ok::<bool, String>(true)
                })
                .await;
            this.update(cx, |_, cx| match recorded {
                Ok(true) => cx.emit(HistoryEvent::Recorded),
                Ok(false) => {}
                Err(e) => eprintln!("history: {e}"),
            })
            .ok();
        })
        .detach();
    }
}
