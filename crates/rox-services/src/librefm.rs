//! Libre.fm scrobbling. It speaks Last.fm's protocol at its own host, with
//! no api identity to file sessions under. Like ListenBrainz it owns no
//! clock: it rides [`Started`] and [`Crossed`], so the shared threshold
//! decides. Sends are fire and forget; a refused session (error 9) is
//! dropped and shown rather than retried.

use std::collections::BTreeMap;

use gpui::{Context, Entity, Subscription};

use rox_core::settings::Settings;
use rox_net::lastfm::AuthPhase;
use rox_net::librefm::{self, API_KEY, AUTH_URL};

use crate::lastfm::{Crossed, Scrobbler, Started};

pub struct LibreFm {
    config: rox_core::settings::LibreFm,
    phase: AuthPhase,
    _started: Subscription,
    _crossed: Subscription,
}

impl LibreFm {
    pub fn new(scrobbler: &Entity<Scrobbler>, cx: &mut Context<Self>) -> Self {
        let _started = cx.subscribe(scrobbler, |this: &mut Self, _, event: &Started, cx| {
            this.now_playing(event, cx);
        });
        let _crossed = cx.subscribe(scrobbler, |this: &mut Self, _, event: &Crossed, cx| {
            this.crossed(event, cx);
        });
        LibreFm {
            config: Settings::load().accounts.librefm,
            phase: AuthPhase::Idle,
            _started,
            _crossed,
        }
    }

    pub fn config(&self) -> &rox_core::settings::LibreFm {
        &self.config
    }

    pub fn phase(&self) -> &AuthPhase {
        &self.phase
    }

    pub fn username(&self) -> &str {
        &self.config.username
    }

    /// The scrobble switch isn't asked here: nothing rides the scrobbler's
    /// events while it's off.
    pub fn connected(&self) -> bool {
        !self.config.session_key.is_empty()
    }

    fn persist(&self) {
        let config = self.config.clone();
        Settings::update(move |s| s.accounts.librefm = config);
    }

    pub fn begin_auth(&mut self, cx: &mut Context<Self>) {
        self.phase = AuthPhase::Requesting;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut params = BTreeMap::new();
                    params.insert("api_key".to_string(), API_KEY.to_string());
                    librefm::call("auth.getToken", params)
                        .map_err(|e| e.to_string())?
                        .get("token")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                        .ok_or_else(|| "no token in the response".to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(token) => {
                        cx.open_url(&format!("{AUTH_URL}?api_key={API_KEY}&token={token}"));
                        this.phase = AuthPhase::Waiting(token);
                    }
                    Err(e) => this.phase = AuthPhase::Failed(format!("getting a token: {e}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn finish_auth(&mut self, cx: &mut Context<Self>) {
        let AuthPhase::Waiting(token) = &self.phase else {
            return;
        };
        let token = token.clone();
        self.phase = AuthPhase::Confirming;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut params = BTreeMap::new();
                    params.insert("api_key".to_string(), API_KEY.to_string());
                    params.insert("token".to_string(), token);
                    let value =
                        librefm::call("auth.getSession", params).map_err(|e| e.to_string())?;
                    let session = value
                        .get("session")
                        .ok_or_else(|| "no session in the response".to_string())?;
                    let read = |field: &str| {
                        session
                            .get(field)
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .ok_or_else(|| format!("no session {field} in the response"))
                    };
                    Ok::<_, String>((read("key")?, read("name")?))
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((session_key, username)) => {
                        this.config.session_key = session_key;
                        this.config.username = username;
                        this.phase = AuthPhase::Idle;
                        this.persist();
                    }
                    Err(e) => this.phase = AuthPhase::Failed(format!("confirming: {e}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.drop_session(AuthPhase::Idle, cx);
    }

    fn session_rejected(&mut self, cx: &mut Context<Self>) {
        log::warn!("librefm: the session was rejected, reconnecting is the fix");
        self.drop_session(AuthPhase::Rejected, cx);
    }

    fn drop_session(&mut self, phase: AuthPhase, cx: &mut Context<Self>) {
        self.config.session_key.clear();
        self.config.username.clear();
        self.phase = phase;
        self.persist();
        cx.notify();
    }

    fn now_playing(&mut self, event: &Started, cx: &mut Context<Self>) {
        if !self.connected() {
            return;
        }
        let Some(params) = track_params(
            &self.config.session_key,
            &event.artist,
            &event.title,
            &event.album,
            event.duration_secs,
            None,
        ) else {
            return;
        };
        self.submit("track.updateNowPlaying", params, cx);
    }

    fn crossed(&mut self, event: &Crossed, cx: &mut Context<Self>) {
        if !self.connected() {
            return;
        }
        let Some(params) = track_params(
            &self.config.session_key,
            &event.artist,
            &event.title,
            &event.album,
            event.duration_secs,
            Some(event.started),
        ) else {
            return;
        };
        self.submit("track.scrobble", params, cx);
    }

    /// Nothing is retried. A refused session drops the connection where the
    /// user can see it, since every later call would fail the same way.
    fn submit(
        &self,
        method: &'static str,
        params: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { librefm::call(method, params) })
                .await;
            match result {
                Ok(_) => {}
                Err(e) if e.session_rejected() => {
                    log::warn!("librefm: {method}: {e}");
                    this.update(cx, |this, cx| this.session_rejected(cx)).ok();
                }
                Err(e) => log::warn!("librefm: {method}: {e}"),
            }
        })
        .detach();
    }
}

/// None for a track with no artist or title, which the service can't take.
fn track_params(
    session_key: &str,
    artist: &str,
    title: &str,
    album: &str,
    duration_secs: Option<f64>,
    timestamp: Option<u64>,
) -> Option<BTreeMap<String, String>> {
    if artist.is_empty() || title.is_empty() {
        return None;
    }
    let mut params = BTreeMap::new();
    params.insert("api_key".to_string(), API_KEY.to_string());
    params.insert("sk".to_string(), session_key.to_string());
    params.insert("artist".to_string(), artist.to_string());
    params.insert("track".to_string(), title.to_string());
    if !album.is_empty() {
        params.insert("album".to_string(), album.to_string());
    }
    if let Some(duration) = duration_secs.filter(|d| *d > 0.0) {
        params.insert(
            "duration".to_string(),
            (duration.round() as u64).to_string(),
        );
    }
    if let Some(timestamp) = timestamp {
        params.insert("timestamp".to_string(), timestamp.to_string());
    }
    Some(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scrobble_carries_its_timestamp_and_a_now_playing_does_not() {
        let now = track_params("sk", "Boards of Canada", "Roygbiv", "", Some(151.4), None)
            .expect("tagged");
        assert_eq!(now.get("timestamp"), None);
        assert_eq!(now.get("duration").map(String::as_str), Some("151"));
        assert!(!now.contains_key("album"), "a blank album stays out");

        let scrobble = track_params(
            "sk",
            "Boards of Canada",
            "Roygbiv",
            "Music Has the Right to Children",
            Some(151.4),
            Some(1_700_000_000),
        )
        .expect("tagged");
        assert_eq!(
            scrobble.get("timestamp").map(String::as_str),
            Some("1700000000")
        );
        assert_eq!(
            scrobble.get("album").map(String::as_str),
            Some("Music Has the Right to Children")
        );
        assert_eq!(scrobble.get("sk").map(String::as_str), Some("sk"));
    }

    #[test]
    fn a_track_without_tags_sends_nothing() {
        assert!(track_params("sk", "", "Roygbiv", "", None, None).is_none());
        assert!(track_params("sk", "Boards of Canada", "", "", None, None).is_none());
    }
}
