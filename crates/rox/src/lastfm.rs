//! Last.fm scrobbling: the signed audioscrobbler API calls and the
//! scrobbler entity that watches the player. The scrobbler rides the
//! player's pump ticks, accumulates how much of the playing track has
//! actually sounded (seeks don't count), sends the now-playing update
//! when a track starts, and scrobbles once the listened time crosses the
//! configured threshold of the duration. All HTTP runs blocking on the
//! background executor, like the decoders and the database do their
//! work; failures log and never touch playback. The API key and secret
//! come from the build's own identity ([`keys`]), with the settings
//! file's pair as the override for builds that ship none. The connect
//! flow is last.fm's desktop dance: fetch a token, authorize it in the
//! browser, trade it for a permanent session key.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Context, Entity, EventEmitter, SharedString, Subscription};

use rox_library::store::TrackMeta;

use crate::panels::library::Library;
use crate::player::Player;
use crate::settings::{Lastfm, Settings};

pub mod keys;

/// Whether this build carries its own api identity; without one the
/// settings page asks for the user's pair.
pub fn has_builtin_keys() -> bool {
    !keys::API_KEY.is_empty() && !keys::API_SECRET.is_empty()
}

const API_ROOT: &str = "https://ws.audioscrobbler.com/2.0/";

/// Last.fm refuses scrobbles for tracks this short, so the scrobbler
/// doesn't try; the listen signal draws the same line, so history and
/// scrobbling agree on what counts.
const MIN_TRACK_SECS: f64 = 30.0;

/// A play crossed the listen threshold: the one "real listen" signal.
/// History records it always; the scrobble follows only while armed.
pub struct Listened {
    pub path: PathBuf,
    /// When the play began, unix seconds.
    pub started: u64,
}

/// The api_sig the API requires on every signed call: the parameters
/// sorted by name, concatenated as name-value, the secret appended, md5
/// hex over the lot. `format` stays out of the signature per the docs.
fn sign(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut base = String::new();
    for (name, value) in params {
        base.push_str(name);
        base.push_str(value);
    }
    base.push_str(secret);
    format!("{:x}", md5::compute(base.as_bytes()))
}

/// One signed API call, blocking: POST the parameters, parse the JSON,
/// surface the API's own error message when it sends one. Runs on the
/// background executor only.
fn call(
    method: &str,
    secret: &str,
    mut params: BTreeMap<String, String>,
) -> Result<serde_json::Value, String> {
    params.insert("method".into(), method.into());
    let sig = sign(&params, secret);
    params.insert("api_sig".into(), sig);
    params.insert("format".into(), "json".into());
    let pairs: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    // An API error still carries a JSON body worth reading, so a status
    // failure parses like a success.
    let text = match ureq::post(API_ROOT).send_form(&pairs) {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if value.get("error").is_some() {
        let message = value
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error");
        return Err(message.to_string());
    }
    Ok(value)
}

/// Where the connect flow stands, for the settings window's readout.
/// Connected is not a phase: a filled session key in the config is.
#[derive(Clone, PartialEq)]
pub enum AuthPhase {
    Idle,
    /// auth.getToken is in flight.
    Requesting,
    /// The browser has the authorize page; the token waits for the user
    /// to come back and finish.
    Waiting(String),
    /// auth.getSession is in flight.
    Confirming,
    Failed(SharedString),
}

/// The playing track under watch: its identity, tags, and how much of it
/// has actually sounded so far.
struct Watch {
    path: PathBuf,
    /// The library's tags, or None for a file it doesn't know; last.fm
    /// needs at least an artist and a title, so untagged tracks watch
    /// silently.
    meta: Option<TrackMeta>,
    duration: Option<f64>,
    /// When the watch began, unix seconds: the scrobble's timestamp.
    started: u64,
    /// Seconds actually listened: position deltas at playback speed.
    /// Seeks jump the clock and don't count.
    played: f64,
    last_pos: f64,
    now_playing_sent: bool,
    /// The listen signal fired for this watch; set on the threshold
    /// crossing whether or not scrobbling is armed.
    listened: bool,
    scrobbled: bool,
}

/// The scrobbler entity, one per workspace beside its player. Holds the
/// live last.fm config (the settings window edits it here and persists
/// through it), so the panels' threshold markers and the scrobble math
/// never read the settings file per frame.
pub struct Scrobbler {
    library: Entity<Library>,
    config: Lastfm,
    phase: AuthPhase,
    watch: Option<Watch>,
    _player_changed: Subscription,
}

impl EventEmitter<Listened> for Scrobbler {}

impl Scrobbler {
    pub fn new(
        player: &Entity<Player>,
        library: &Entity<Library>,
        cx: &mut Context<Self>,
    ) -> Self {
        // The player's pump notifies every tick while a session runs, so
        // observing it is the scrobbler's whole clock.
        let _player_changed = cx.observe(player, |this: &mut Self, player, cx| {
            this.tick(&player, cx);
        });
        Scrobbler {
            library: library.clone(),
            config: Settings::load().lastfm,
            phase: AuthPhase::Idle,
            watch: None,
            _player_changed,
        }
    }

    /// The live config, the settings window's and the panels' read.
    pub fn config(&self) -> &Lastfm {
        &self.config
    }

    pub fn phase(&self) -> &AuthPhase {
        &self.phase
    }

    /// Where the threshold marker sits, 0 to 1 - or None while scrobbling
    /// couldn't happen anyway, so the panels never draw a line that lies.
    pub fn marker(&self) -> Option<f32> {
        self.armed().then_some(self.config.threshold)
    }

    /// The signing pair the calls use: the settings override when the
    /// user entered one, the build's own identity otherwise.
    fn api_key(&self) -> &str {
        if self.config.api_key.is_empty() {
            keys::API_KEY
        } else {
            &self.config.api_key
        }
    }

    fn api_secret(&self) -> &str {
        if self.config.api_secret.is_empty() {
            keys::API_SECRET
        } else {
            &self.config.api_secret
        }
    }

    /// Whether a played track would actually scrobble: the switch is on
    /// and the account is connected.
    fn armed(&self) -> bool {
        self.config.scrobbling
            && !self.config.session_key.is_empty()
            && !self.api_key().is_empty()
            && !self.api_secret().is_empty()
    }

    fn persist(&self) {
        let lastfm = self.config.clone();
        Settings::update(move |s| s.lastfm = lastfm);
    }

    pub fn set_api_key(&mut self, key: String, cx: &mut Context<Self>) {
        self.config.api_key = key;
        self.persist();
        cx.notify();
    }

    pub fn set_api_secret(&mut self, secret: String, cx: &mut Context<Self>) {
        self.config.api_secret = secret;
        self.persist();
        cx.notify();
    }

    pub fn set_scrobbling(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.scrobbling = on;
        self.persist();
        cx.notify();
    }

    pub fn set_threshold(&mut self, threshold: f32, cx: &mut Context<Self>) {
        // The same band the settings loader enforces; the slider's low end
        // stops short of a threshold that scrobbles on the first note.
        self.config.threshold = threshold.clamp(0.1, 1.0);
        self.persist();
        cx.notify();
    }

    /// Start the connect flow: fetch a request token and hand the
    /// authorize page to the browser. The token then waits in
    /// [`AuthPhase::Waiting`] for [`Self::finish_auth`].
    pub fn begin_auth(&mut self, cx: &mut Context<Self>) {
        if self.api_key().is_empty() || self.api_secret().is_empty() {
            self.phase = AuthPhase::Failed("enter an api key and secret first".into());
            cx.notify();
            return;
        }
        self.phase = AuthPhase::Requesting;
        cx.notify();
        let key = self.api_key().to_string();
        let secret = self.api_secret().to_string();
        cx.spawn(async move |this, cx| {
            let request_key = key.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut params = BTreeMap::new();
                    params.insert("api_key".to_string(), request_key);
                    call("auth.getToken", &secret, params)?
                        .get("token")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                        .ok_or_else(|| "no token in the response".to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(token) => {
                        cx.open_url(&format!(
                            "https://www.last.fm/api/auth/?api_key={key}&token={token}"
                        ));
                        this.phase = AuthPhase::Waiting(token);
                    }
                    Err(e) => this.phase = AuthPhase::Failed(format!("getting a token: {e}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Trade the authorized token for the permanent session key, the
    /// flow's last step once the browser side is done.
    pub fn finish_auth(&mut self, cx: &mut Context<Self>) {
        let AuthPhase::Waiting(token) = &self.phase else {
            return;
        };
        let token = token.clone();
        self.phase = AuthPhase::Confirming;
        cx.notify();
        let key = self.api_key().to_string();
        let secret = self.api_secret().to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut params = BTreeMap::new();
                    params.insert("api_key".to_string(), key);
                    params.insert("token".to_string(), token);
                    let value = call("auth.getSession", &secret, params)?;
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
                    Err(e) => {
                        this.phase = AuthPhase::Failed(format!("confirming: {e}").into())
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Drop the session locally. Last.fm keeps its side until the user
    /// revokes rox there; a fresh connect just lands a new session.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.config.session_key.clear();
        self.config.username.clear();
        self.phase = AuthPhase::Idle;
        self.persist();
        cx.notify();
    }

    /// One pump tick: keep the watch on the playing track, grow the
    /// listened clock, and fire the submissions their moments call for.
    fn tick(&mut self, player: &Entity<Player>, cx: &mut Context<Self>) {
        let player = player.read(cx);
        let Some(now) = player.now_playing() else {
            self.watch = None;
            return;
        };
        let playing = player.is_playing();

        let changed = self
            .watch
            .as_ref()
            .map(|watch| watch.path != now.path)
            .unwrap_or(true);
        if changed {
            self.begin_watch(now.path.clone(), now.duration_secs, now.position_secs, cx);
        } else {
            let watch = self.watch.as_mut().expect("watch exists when unchanged");
            if now.duration_secs.is_some() {
                watch.duration = now.duration_secs;
            }
            let delta = now.position_secs - watch.last_pos;
            if delta > 0.0 && delta <= 1.0 {
                // A tick's worth of playback; anything bigger is a seek
                // and doesn't count as listening.
                watch.played += delta;
            } else if delta < -5.0 && watch.listened && now.position_secs < 5.0 {
                // Back to the top after a counted listen - a loop restart
                // or a deliberate replay - counts as a fresh play.
                self.begin_watch(now.path.clone(), now.duration_secs, now.position_secs, cx);
                return;
            }
            watch.last_pos = now.position_secs;
        }

        // The listen signal fires on the threshold crossing no matter
        // where scrobbling stands: history records every real listen,
        // the scrobble below reuses the same crossing while armed.
        let qualifies = self.watch.as_ref().is_some_and(|w| self.qualifies(w));
        if let Some(watch) = self.watch.as_mut() {
            if qualifies && !watch.listened {
                watch.listened = true;
                cx.emit(Listened {
                    path: watch.path.clone(),
                    started: watch.started,
                });
            }
        }

        if !self.armed() {
            return;
        }

        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        // The now-playing update waits for audio to actually move, so a
        // restored track sitting paused announces nothing.
        if !watch.now_playing_sent && playing {
            watch.now_playing_sent = true;
            self.submit("track.updateNowPlaying", cx);
            return;
        }
        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        if !watch.scrobbled && qualifies {
            watch.scrobbled = true;
            self.submit("track.scrobble", cx);
        }
    }

    /// The one "real listen" rule, shared by the scrobble and the
    /// history recorder's [`Listened`] signal: the track is long enough
    /// to count and enough of it has actually sounded.
    fn qualifies(&self, watch: &Watch) -> bool {
        watch
            .duration
            .filter(|d| *d > MIN_TRACK_SECS)
            .is_some_and(|d| watch.played >= d * self.config.threshold as f64)
    }

    /// Point the watch at a track that just came up. The listened clock
    /// starts empty no matter where the position sits, so a track opened
    /// mid-way still has to play its share.
    fn begin_watch(
        &mut self,
        path: PathBuf,
        duration: Option<f64>,
        position: f64,
        cx: &mut Context<Self>,
    ) {
        let meta = self.library.read(cx).meta_for(&path);
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.watch = Some(Watch {
            path,
            meta,
            duration,
            started,
            played: 0.0,
            last_pos: position,
            now_playing_sent: false,
            listened: false,
            scrobbled: false,
        });
    }

    /// Send the watched track to the API, fire and forget: the params the
    /// two track methods share, the timestamp only where the scrobble
    /// wants it. Missing tags skip quietly - last.fm can't take a track
    /// without an artist and a title.
    fn submit(&self, method: &'static str, cx: &mut Context<Self>) {
        let Some(watch) = &self.watch else {
            return;
        };
        let Some(meta) = &watch.meta else {
            return;
        };
        if meta.artist.is_empty() || meta.title.is_empty() {
            return;
        }
        let mut params = BTreeMap::new();
        params.insert("api_key".to_string(), self.api_key().to_string());
        params.insert("sk".to_string(), self.config.session_key.clone());
        params.insert("artist".to_string(), meta.artist.clone());
        params.insert("track".to_string(), meta.title.clone());
        if !meta.album.is_empty() {
            params.insert("album".to_string(), meta.album.clone());
        }
        if let Some(duration) = watch.duration {
            params.insert("duration".to_string(), (duration.round() as u64).to_string());
        }
        if method == "track.scrobble" {
            params.insert("timestamp".to_string(), watch.started.to_string());
        }
        let secret = self.api_secret().to_string();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = call(method, &secret, params) {
                    eprintln!("lastfm: {method}: {e}");
                }
            })
            .detach();
    }
}
