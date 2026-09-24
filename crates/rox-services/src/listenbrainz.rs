//! ListenBrainz submission. It owns no clock: it rides the scrobbler's
//! [`Started`] and [`Crossed`], so the one shared threshold decides when a
//! play counts. Failed listens wait in a bounded backlog that rides out with
//! the next send. The credential is a user token; "connected" means a token
//! the service answered a name for.

use gpui::{Context, Entity, Subscription};

use rox_core::settings::Settings;
use rox_net::listenbrainz::{self, Listen};

use crate::lastfm::{Crossed, Scrobbler, Started};

/// Past this the oldest failed listens go; the newest are the ones missed.
const BACKLOG_CAP: usize = 100;

#[derive(Clone, PartialEq)]
pub enum Status {
    Off,
    Unverified,
    Connected(String),
    /// Every submission fails until the token is replaced, so it's shown.
    Invalid,
    Failed(String),
}

pub struct ListenBrainz {
    config: rox_core::settings::ListenBrainz,
    backlog: Vec<Listen>,
    /// So a second Connect click doesn't race the first.
    validating: bool,
    /// So two listens close together don't send the same backlog twice.
    sending: bool,
    status: Status,
    _started: Subscription,
    _crossed: Subscription,
}

impl ListenBrainz {
    pub fn new(scrobbler: &Entity<Scrobbler>, cx: &mut Context<Self>) -> Self {
        let _started = cx.subscribe(scrobbler, |this: &mut Self, _, event: &Started, cx| {
            this.now_playing(event, cx);
        });
        let _crossed = cx.subscribe(scrobbler, |this: &mut Self, _, event: &Crossed, cx| {
            this.crossed(event, cx);
        });
        let config = Settings::load().accounts.listenbrainz;
        let status = if config.token.is_empty() {
            Status::Off
        } else {
            match &config.username {
                Some(name) => Status::Connected(name.clone()),
                None => Status::Unverified,
            }
        };
        let mut this = ListenBrainz {
            config,
            backlog: Vec::new(),
            validating: false,
            sending: false,
            status,
            _started,
            _crossed,
        };
        // A token with no name was stored by a check that never came back.
        if this.status == Status::Unverified {
            this.validate(cx);
        }
        this
    }

    pub fn config(&self) -> &rox_core::settings::ListenBrainz {
        &self.config
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn pending(&self) -> usize {
        self.backlog.len()
    }

    /// Validation is for the readout, not a gate: an unnamed token still sends.
    pub fn connected(&self) -> bool {
        !self.config.token.is_empty()
    }

    fn persist(&self) {
        let config = self.config.clone();
        Settings::update(move |s| s.accounts.listenbrainz = config);
    }

    /// Saved before the check, so a check that never returns costs a retry
    /// rather than a re-paste.
    pub fn set_token(&mut self, token: String, cx: &mut Context<Self>) {
        self.config.token = token.trim().to_string();
        self.config.username = None;
        self.persist();
        if self.config.token.is_empty() {
            self.status = Status::Off;
            cx.notify();
            return;
        }
        self.validate(cx);
    }

    /// Nothing is revoked over there; a token is only revoked on the site.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.config.token.clear();
        self.config.username = None;
        self.persist();
        self.backlog.clear();
        self.status = Status::Off;
        cx.notify();
    }

    fn validate(&mut self, cx: &mut Context<Self>) {
        if self.validating || self.config.token.is_empty() {
            return;
        }
        self.validating = true;
        self.status = Status::Unverified;
        cx.notify();
        let token = self.config.token.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { listenbrainz::validate_token(&token) })
                .await;
            this.update(cx, |this, cx| {
                this.validating = false;
                match result {
                    Ok(Some(name)) => {
                        this.config.username = Some(name.clone());
                        this.status = Status::Connected(name);
                        this.persist();
                    }
                    Ok(None) => {
                        this.config.username = None;
                        this.status = Status::Invalid;
                        this.persist();
                    }
                    Err(e) => {
                        log::warn!("listenbrainz: validate: {e}");
                        this.status = Status::Failed(e.message);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Never queued or retried: a late playing-now is worse than none.
    fn now_playing(&mut self, event: &Started, cx: &mut Context<Self>) {
        if !self.connected() || event.artist.is_empty() || event.title.is_empty() {
            return;
        }
        let listen = Listen::new(
            event.artist.clone(),
            event.title.clone(),
            event.album.clone(),
            event.duration_secs,
            None,
        );
        let token = self.config.token.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { listenbrainz::submit(&token, "playing_now", &[listen]) })
                .await;
            if let Err(e) = result {
                let rejected = e.token_rejected();
                log::warn!("listenbrainz: playing now: {e}");
                if rejected {
                    this.update(cx, |this, cx| this.token_rejected(cx)).ok();
                }
            }
        })
        .detach();
    }

    fn crossed(&mut self, event: &Crossed, cx: &mut Context<Self>) {
        if !self.connected() || event.artist.is_empty() || event.title.is_empty() {
            return;
        }
        enqueue(
            &mut self.backlog,
            Listen::new(
                event.artist.clone(),
                event.title.clone(),
                event.album.clone(),
                event.duration_secs,
                Some(event.started),
            ),
        );
        self.flush(cx);
    }

    /// `single` for one listen, `import` when a dropped network left more.
    fn flush(&mut self, cx: &mut Context<Self>) {
        if self.sending || self.backlog.is_empty() || !self.connected() {
            return;
        }
        self.sending = true;
        let batch = self.backlog.clone();
        let count = batch.len();
        let listen_type = if count == 1 { "single" } else { "import" };
        let token = self.config.token.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { listenbrainz::submit(&token, listen_type, &batch) })
                .await;
            this.update(cx, |this, cx| {
                this.sending = false;
                match result {
                    Ok(()) => {
                        // Drain what landed, not the whole list: a listen
                        // queued while this was in flight is still owed.
                        landed(&mut this.backlog, count);
                        if !matches!(this.status, Status::Connected(_)) {
                            this.validate(cx);
                        }
                        if !this.backlog.is_empty() {
                            this.flush(cx);
                        }
                    }
                    Err(e) if e.token_rejected() => {
                        log::warn!("listenbrainz: submit: {e}");
                        this.token_rejected(cx);
                    }
                    Err(e) => {
                        log::warn!("listenbrainz: submit: {e}");
                        // A payload the service will never take goes
                        // rather than blocking everything behind it.
                        if !e.retryable() {
                            landed(&mut this.backlog, count);
                        }
                        this.status = Status::Failed(e.message);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The backlog stays, so a replacement token sends it.
    fn token_rejected(&mut self, cx: &mut Context<Self>) {
        self.config.username = None;
        self.persist();
        self.status = Status::Invalid;
        cx.notify();
    }
}

fn enqueue(backlog: &mut Vec<Listen>, listen: Listen) {
    backlog.push(listen);
    if backlog.len() > BACKLOG_CAP {
        backlog.drain(..backlog.len() - BACKLOG_CAP);
    }
}

fn landed(backlog: &mut Vec<Listen>, count: usize) {
    backlog.drain(..count.min(backlog.len()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listen(title: &str) -> Listen {
        Listen::new(
            "Boards of Canada".into(),
            title.into(),
            "Geogaddi".into(),
            Some(300.0),
            Some(1_700_000_000),
        )
    }

    fn titles(backlog: &[Listen]) -> Vec<&str> {
        backlog
            .iter()
            .map(|l| l.track_metadata.track_name.as_str())
            .collect()
    }

    #[test]
    fn a_failed_send_keeps_its_listens_in_order() {
        let mut backlog = Vec::new();
        enqueue(&mut backlog, listen("Dawn Chorus"));
        enqueue(&mut backlog, listen("Julie and Candy"));
        assert_eq!(titles(&backlog), vec!["Dawn Chorus", "Julie and Candy"]);
    }

    #[test]
    fn a_success_clears_what_it_sent_and_nothing_else() {
        let mut backlog = Vec::new();
        enqueue(&mut backlog, listen("Dawn Chorus"));
        enqueue(&mut backlog, listen("Julie and Candy"));
        let sent = backlog.len();
        enqueue(&mut backlog, listen("Alpha and Omega"));
        landed(&mut backlog, sent);
        assert_eq!(
            titles(&backlog),
            vec!["Alpha and Omega"],
            "the one that arrived mid-flight is still owed"
        );
        let sent = backlog.len();
        landed(&mut backlog, sent);
        assert!(backlog.is_empty());
    }

    #[test]
    fn the_cap_drops_the_oldest() {
        let mut backlog = Vec::new();
        for n in 0..BACKLOG_CAP + 5 {
            enqueue(&mut backlog, listen(&format!("track {n}")));
        }
        assert_eq!(backlog.len(), BACKLOG_CAP);
        assert_eq!(
            titles(&backlog).first().copied(),
            Some("track 5"),
            "the five oldest went, the newest stayed"
        );
        let newest = format!("track {}", BACKLOG_CAP + 4);
        assert_eq!(titles(&backlog).last().copied(), Some(newest.as_str()));
    }
}
