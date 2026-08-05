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
//! flow is Last.fm's desktop dance: fetch a token, authorize it in the
//! browser, trade it for a permanent session key.
//!
//! The same session key carries the favourites mirror: with it armed, a
//! heart in rox becomes a love on Last.fm and taking the heart back
//! unloves it. That half doesn't ride the player at all. It watches the
//! library's favourite set and pushes what moved, through a queue that
//! retries, because a love that quietly failed to send leaves the two
//! sides disagreeing with nothing on screen to say so. The mirror only
//! pushes: nothing here reads Last.fm's loved list back.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, Entity, EventEmitter, SharedString, Subscription};

use rox_library::store::TrackMeta;

use crate::panels::library::{Library, LibraryEvent};
use crate::player::Player;
use crate::settings::{Lastfm, Settings};

pub mod import;
pub mod keys;

/// Whether this build carries its own api identity; without one the
/// settings page asks for the user's pair.
// The pair are consts baked in at compile time, so clippy can const-eval
// this and calls it a constant condition. That's exactly the question
// being asked: which build am I?
#[allow(clippy::const_is_empty)]
pub fn has_builtin_keys() -> bool {
    !keys::API_KEY.is_empty() && !keys::API_SECRET.is_empty()
}

const API_ROOT: &str = "https://ws.audioscrobbler.com/2.0/";

/// Last.fm refuses scrobbles for tracks this short, so the scrobbler
/// doesn't try; the listen signal draws the same line, so history and
/// scrobbling agree on what counts.
const MIN_TRACK_SECS: f64 = 30.0;

/// The fixed listen rule that feeds history, the scrobble standard: a
/// track counts once half of it has sounded. The user's scrobble
/// threshold is a separate knob and doesn't move this line.
const LISTEN_FRACTION: f64 = 0.5;

/// The cap on the listen rule: four minutes of playback counts even when
/// that's less than half a long track, whichever comes first.
const LISTEN_CAP_SECS: f64 = 240.0;

/// A play crossed the listen rule: the one "real listen" signal.
/// History records it always; the scrobble follows its own threshold
/// while armed.
pub struct Listened {
    pub path: PathBuf,
    /// When the play began, unix seconds.
    pub started: u64,
}

/// The wall clock as unix seconds, the scrobble timestamp's unit.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How long a failed love waits before the mirror tries it again, one
/// entry per attempt left. Short enough that a blip clears while the app
/// is still open, spaced enough that a service having a bad afternoon
/// isn't hammered for it. A push that runs the list out is dropped, and
/// its reason stays on the settings page.
const LOVE_BACKOFF: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(120),
];

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

/// A call that didn't land: Last.fm's own error code where the service
/// answered, none where the request never got that far. The message is
/// the part worth showing; the code is what tells a retry from a waste
/// of time.
pub struct ApiError {
    code: Option<i64>,
    message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl ApiError {
    /// Whether the same call could plausibly work later. A transport
    /// failure is the offline case, always worth another go. Of Last.fm's
    /// own codes only the service-side ones qualify: 8 operation failed,
    /// 11 and 16 service down or busy, 29 rate limit. A rejected session
    /// or a track it can't name comes back identical every time, so those
    /// stop where they are rather than burning the backoff.
    fn retryable(&self) -> bool {
        match self.code {
            None => true,
            Some(code) => matches!(code, 8 | 11 | 16 | 29),
        }
    }
}

/// One signed API call, blocking: POST the parameters, parse the JSON,
/// surface the API's own error message when it sends one. Runs on the
/// background executor only.
fn call(
    method: &str,
    secret: &str,
    mut params: BTreeMap<String, String>,
) -> Result<serde_json::Value, ApiError> {
    params.insert("method".into(), method.into());
    let sig = sign(&params, secret);
    params.insert("api_sig".into(), sig);
    params.insert("format".into(), "json".into());
    let pairs: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    // A request that never reached the service, or a body that won't read
    // or parse, gets no code: the next try may well go through, so these
    // land as retryable rather than as a rejection.
    let transport = |message: String| ApiError {
        code: None,
        message,
    };
    // An API error still carries a JSON body worth reading, so a status
    // failure parses like a success. Ride the shared provider agent for its
    // User-Agent and timeout; a bare ureq::post has neither, so a hung endpoint
    // parks the connect flow in Confirming forever.
    let text = match crate::providers::agent().post(API_ROOT).send_form(&pairs) {
        Ok(response) => response
            .into_string()
            .map_err(|e| transport(e.to_string()))?,
        Err(ureq::Error::Status(_, response)) => response
            .into_string()
            .map_err(|e| transport(e.to_string()))?,
        Err(e) => return Err(transport(crate::providers::net_reason(&e))),
    };
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| transport(e.to_string()))?;
    if let Some(code) = value.get("error") {
        let message = value
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error");
        return Err(ApiError {
            code: code.as_i64(),
            message: message.to_string(),
        });
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
    /// The library's tags, or None for a file it doesn't know; Last.fm
    /// needs at least an artist and a title, so untagged tracks watch
    /// silently.
    meta: Option<TrackMeta>,
    duration: Option<f64>,
    /// When audio first moved under this watch, unix seconds: the
    /// scrobble's timestamp. Zero until playback is actually observed.
    started: u64,
    /// Seconds actually listened: position deltas at playback speed.
    /// Seeks jump the clock and don't count.
    played: f64,
    last_pos: f64,
    now_playing_sent: bool,
    /// The listen signal fired for this watch; set on the listen-rule
    /// crossing whether or not scrobbling is armed.
    listened: bool,
    scrobbled: bool,
}

/// A heart waiting to reach Last.fm. The track it belongs to is the
/// queue's key, so this is only which way it went and how the sending has
/// gone so far.
struct Love {
    /// True loves, false unloves: where the heart landed, not how many
    /// times it was clicked getting there.
    on: bool,
    /// Failed sends so far, the index into [`LOVE_BACKOFF`].
    tries: usize,
}

/// The hearts still to send, one entry per track, keyed by the artist and
/// title Last.fm names it by. Keyed rather than a list because the queue's
/// job is to carry where a heart ended up, not the clicking that got it
/// there: flip one twice while the network is down and Last.fm should hear
/// about it once.
#[derive(Default)]
struct LoveQueue(BTreeMap<(String, String), Love>);

impl LoveQueue {
    /// A heart the user just moved. It replaces whatever was waiting for
    /// that track, tries and all: the newest state is the one worth
    /// sending, and it deserves the full run of attempts.
    fn push(&mut self, key: (String, String), on: bool) {
        self.0.insert(key, Love { on, tries: 0 });
    }

    /// Lift the next push out of the queue. Out, not borrowed: a heart
    /// flipped while this one is in flight queues behind it cleanly
    /// instead of racing it.
    fn take(&mut self) -> Option<((String, String), Love)> {
        let key = self.0.keys().next().cloned()?;
        let love = self.0.remove(&key)?;
        Some((key, love))
    }

    /// A failed push back in for another go, under anything the user has
    /// decided since. A retry that overwrote a newer heart would send the
    /// state the user just moved away from.
    fn retry(&mut self, key: (String, String), love: Love) {
        self.0.entry(key).or_insert(love);
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

/// One push about to go out, built while the config is in hand so the
/// drain task can send it without reaching back for anything.
struct LoveSend {
    /// The track as Last.fm names it, artist then title. Also the queue
    /// key this came out of, for putting a retry back.
    key: (String, String),
    love: Love,
    method: &'static str,
    secret: String,
    params: BTreeMap<String, String>,
}

/// The scrobbler entity, one per workspace beside its player. Holds the
/// live Last.fm config (the settings window edits it here and persists
/// through it), so the panels' threshold markers and the scrobble math
/// never read the settings file per frame.
pub struct Scrobbler {
    library: Entity<Library>,
    config: Lastfm,
    phase: AuthPhase,
    watch: Option<Watch>,
    /// The favourite track ids as the mirror last saw them, the diff's
    /// other side. None until there's a set worth trusting: a snapshot
    /// taken before the library loaded would read an empty catalog as
    /// every favourite having just been taken back.
    favourites: Option<HashSet<i64>>,
    loves: LoveQueue,
    /// Whether a drain task is already walking the queue.
    sending: bool,
    /// Why the last push gave up, for the settings page. A love that fails
    /// silently is two sides disagreeing with nothing on screen to say so.
    love_error: Option<SharedString>,
    _player_changed: Subscription,
    _library_changed: Subscription,
}

impl EventEmitter<Listened> for Scrobbler {}

impl Scrobbler {
    pub fn new(player: &Entity<Player>, library: &Entity<Library>, cx: &mut Context<Self>) -> Self {
        // The player's pump notifies every tick while a session runs, so
        // observing it is the scrobbler's whole clock.
        let _player_changed = cx.observe(player, |this: &mut Self, player, cx| {
            this.tick(&player, cx);
        });
        // The mirror rides the library's own events rather than a call
        // site. Every path that moves a heart ends in a playlist change:
        // the favourite panel, the track menu, a drag onto the favourites
        // playlist, delete over a row in it. Diffing the set catches all of
        // them, where hooking the heart's own toggle would catch one.
        let _library_changed = cx.subscribe(
            library,
            |this: &mut Self, _, event: &LibraryEvent, cx| match event {
                LibraryEvent::PlaylistsChanged => this.mirror_favourites(cx),
                // A rescan can rewrite the ids under the snapshot, so the
                // old one means nothing against the new set. Take it again
                // without sending: none of that was anyone unfavouriting.
                LibraryEvent::Updated => this.seed_favourites(cx),
                _ => {}
            },
        );
        Scrobbler {
            library: library.clone(),
            config: Settings::load().accounts.lastfm,
            phase: AuthPhase::Idle,
            watch: None,
            favourites: None,
            loves: LoveQueue::default(),
            sending: false,
            love_error: None,
            _player_changed,
            _library_changed,
        }
    }

    /// The live config, the settings window's and the panels' read.
    pub fn config(&self) -> &Lastfm {
        &self.config
    }

    pub fn phase(&self) -> &AuthPhase {
        &self.phase
    }

    /// How many hearts are still waiting on the network, for the settings
    /// page's readout.
    pub fn loves_pending(&self) -> usize {
        self.loves.len()
    }

    /// Why the last push gave up, if one did. Cleared by the next push
    /// that lands.
    pub fn love_error(&self) -> Option<SharedString> {
        self.love_error.clone()
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

    /// Whether anything could be sent at all: a session in hand and a pair
    /// to sign with. What both switches build on.
    fn connected(&self) -> bool {
        !self.config.session_key.is_empty()
            && !self.api_key().is_empty()
            && !self.api_secret().is_empty()
    }

    /// Whether a played track would actually scrobble: the switch is on
    /// and the account is connected.
    fn armed(&self) -> bool {
        self.config.scrobbling && self.connected()
    }

    /// Whether a heart would actually reach Last.fm. Its own switch beside
    /// the scrobble one: someone who turns scrobbling off for an evening
    /// hasn't asked for their hearts to stop travelling too.
    fn loves_armed(&self) -> bool {
        self.config.love_favourites && self.connected()
    }

    fn persist(&self) {
        let lastfm = self.config.clone();
        Settings::update(move |s| s.accounts.lastfm = lastfm);
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

    /// Arm or disarm the favourites mirror. Arming takes the starting-line
    /// snapshot then and there, so turning it on mirrors from that moment
    /// instead of firing a library's worth of hearts at the account.
    /// Disarming drops the snapshot and anything that hadn't gone yet.
    pub fn set_love_favourites(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.love_favourites = on;
        self.persist();
        if on {
            self.seed_favourites(cx);
        } else {
            self.favourites = None;
            self.loves.clear();
            self.love_error = None;
        }
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
                    call("auth.getToken", &secret, params)
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
                        cx.open_url(&format!(
                            "https://www.Last.fm/api/auth/?api_key={key}&token={token}"
                        ));
                        this.phase = AuthPhase::Waiting(token);
                    }
                    Err(e) => {
                        this.phase = AuthPhase::Failed(format!("getting a token: {e}").into())
                    }
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
                    let value =
                        call("auth.getSession", &secret, params).map_err(|e| e.to_string())?;
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
                        // A connect is where the mirror becomes possible,
                        // so it starts its line here rather than pushing
                        // the favourites that were already on the shelf.
                        this.seed_favourites(cx);
                    }
                    Err(e) => this.phase = AuthPhase::Failed(format!("confirming: {e}").into()),
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
        // Nothing queued can be signed any more, and holding it would only
        // flush it at whatever account connects next.
        self.favourites = None;
        self.loves.clear();
        self.love_error = None;
        self.persist();
        cx.notify();
    }

    /// Take hearts the import just wrote into the snapshot without sending
    /// them. They came from Last.fm in the first place, so pushing them
    /// back would be thousands of calls repeating what it just told us.
    ///
    /// The import calls this in the same update pass as its write, which is
    /// what puts it ahead of the library event the mirror diffs on.
    pub fn absorb_favourites(&mut self, cx: &mut Context<Self>) {
        self.seed_favourites(cx);
    }

    /// Take the favourite snapshot fresh, sending nothing: what arming the
    /// mirror wants, and what a rescan leaves it needing.
    fn seed_favourites(&mut self, cx: &mut Context<Self>) {
        if !self.loves_armed() {
            self.favourites = None;
            return;
        }
        let ids = self.library.read(cx).favourite_ids();
        self.favourites = Some(ids);
    }

    /// A playlist change came through: work out which hearts moved since
    /// the last look and queue them for Last.fm.
    fn mirror_favourites(&mut self, cx: &mut Context<Self>) {
        if !self.loves_armed() {
            // A snapshot kept while disarmed would go stale against every
            // heart clicked meanwhile, and arming takes a fresh one anyway.
            self.favourites = None;
            return;
        }
        let now = self.library.read(cx).favourite_ids();
        let Some(before) = self.favourites.replace(now.clone()) else {
            // First look since arming. The set is the starting line, not a
            // backlog to work through.
            return;
        };
        let loved: Vec<i64> = now.difference(&before).copied().collect();
        let unloved: Vec<i64> = before.difference(&now).copied().collect();
        if loved.is_empty() && unloved.is_empty() {
            return;
        }
        // Names, not ids: Last.fm knows nothing about this library, and a
        // track it can't name never makes it out of here.
        let (loved, unloved) = {
            let library = self.library.read(cx);
            (library.names_for(&loved), library.names_for(&unloved))
        };
        for key in loved {
            self.loves.push(key, true);
        }
        for key in unloved {
            self.loves.push(key, false);
        }
        self.drain_loves(cx);
        cx.notify();
    }

    /// The next push, with the signing pair and session it goes out under.
    fn next_love(&mut self) -> Option<LoveSend> {
        if !self.loves_armed() {
            // Disarmed or disconnected mid-drain: the rest isn't ours to
            // send any more.
            self.loves.clear();
            return None;
        }
        let (key, love) = self.loves.take()?;
        let mut params = BTreeMap::new();
        params.insert("api_key".to_string(), self.api_key().to_string());
        params.insert("sk".to_string(), self.config.session_key.clone());
        params.insert("artist".to_string(), key.0.clone());
        params.insert("track".to_string(), key.1.clone());
        Some(LoveSend {
            method: if love.on {
                "track.love"
            } else {
                "track.unlove"
            },
            secret: self.api_secret().to_string(),
            params,
            key,
            love,
        })
    }

    /// One push came back. A landed call clears the last complaint; a
    /// failure worth another go returns the wait before it; anything else
    /// is dropped with its reason kept where the user can see it.
    fn love_result(
        &mut self,
        send: LoveSend,
        result: Result<serde_json::Value, ApiError>,
        cx: &mut Context<Self>,
    ) -> Option<Duration> {
        let error = match result {
            Ok(_) => {
                self.love_error = None;
                cx.notify();
                return None;
            }
            Err(error) => error,
        };
        let wait = error
            .retryable()
            .then(|| LOVE_BACKOFF.get(send.love.tries).copied())
            .flatten();
        cx.notify();
        match wait {
            Some(wait) => {
                self.loves.retry(
                    send.key,
                    Love {
                        on: send.love.on,
                        tries: send.love.tries + 1,
                    },
                );
                Some(wait)
            }
            None => {
                log::warn!("lastfm: {}: {error}", send.method);
                self.love_error = Some(error.to_string().into());
                None
            }
        }
    }

    /// Walk the queue, one call at a time, until it empties. One drain at
    /// a time: the queue holds a single entry per track, and two walkers
    /// would race over it.
    fn drain_loves(&mut self, cx: &mut Context<Self>) {
        if self.sending || self.loves.is_empty() {
            return;
        }
        self.sending = true;
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some(send)) = this.update(cx, |this, _| this.next_love()) else {
                    break;
                };
                let (method, secret, params) =
                    (send.method, send.secret.clone(), send.params.clone());
                let result = cx
                    .background_executor()
                    .spawn(async move { call(method, &secret, params) })
                    .await;
                match this.update(cx, |this, cx| this.love_result(send, result, cx)) {
                    Ok(Some(wait)) => cx.background_executor().timer(wait).await,
                    Ok(None) => {}
                    // The workspace went away under the drain.
                    Err(_) => break,
                }
            }
            this.update(cx, |this, _| this.sending = false).ok();
        })
        .detach();
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

        // Stamp the start the first time audio is seen moving, not when
        // the watch was created - a launch-restored track sits paused, and
        // Last.fm reads the timestamp as when the track started playing.
        if let Some(watch) = self.watch.as_mut() {
            if watch.started == 0 && playing {
                watch.started = unix_now();
            }
        }

        // Evaluate both rules once against the current watch: the fixed
        // listen rule drives history, the user's threshold drives the
        // scrobble. They accrue off the same clock but cross apart.
        let listens = self.watch.as_ref().is_some_and(Self::qualifies_listen);
        let scrobbles = self
            .watch
            .as_ref()
            .is_some_and(|w| self.qualifies_scrobble(w));

        // The listen signal fires on the listen-rule crossing no matter
        // where scrobbling stands: history records every real listen.
        if let Some(watch) = self.watch.as_mut() {
            if listens && !watch.listened {
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
        if !watch.scrobbled && scrobbles {
            watch.scrobbled = true;
            self.submit("track.scrobble", cx);
        }
    }

    /// The listen rule that feeds history, the scrobble standard: the
    /// track is long enough to count and enough of it has sounded, half
    /// its length or four minutes, whichever comes first.
    fn qualifies_listen(watch: &Watch) -> bool {
        watch
            .duration
            .filter(|d| *d > MIN_TRACK_SECS)
            .is_some_and(|d| watch.played >= (d * LISTEN_FRACTION).min(LISTEN_CAP_SECS))
    }

    /// The scrobble rule: the user's threshold knob against the duration,
    /// deliberately its own line, not the fixed listen rule above.
    fn qualifies_scrobble(&self, watch: &Watch) -> bool {
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
        self.watch = Some(Watch {
            path,
            meta,
            duration,
            // Zero until the tick that first sees audio moving stamps it,
            // so a track restored paused doesn't backdate its scrobble.
            started: 0,
            played: 0.0,
            last_pos: position,
            now_playing_sent: false,
            listened: false,
            scrobbled: false,
        });
    }

    /// Send the watched track to the API, fire and forget: the params the
    /// two track methods share, the timestamp only where the scrobble
    /// wants it. Missing tags skip quietly - Last.fm can't take a track
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
            params.insert(
                "duration".to_string(),
                (duration.round() as u64).to_string(),
            );
        }
        if method == "track.scrobble" {
            params.insert("timestamp".to_string(), watch.started.to_string());
        }
        let secret = self.api_secret().to_string();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = call(method, &secret, params) {
                    log::warn!("lastfm: {method}: {e}");
                }
            })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str) -> (String, String) {
        ("Boards of Canada".to_string(), title.to_string())
    }

    #[test]
    fn a_heart_flipped_twice_sends_once() {
        let mut queue = LoveQueue::default();
        queue.push(track("Roygbiv"), true);
        queue.push(track("Roygbiv"), false);
        assert_eq!(queue.len(), 1, "one track, one pending push");
        let (_, love) = queue.take().unwrap();
        assert!(!love.on, "and it carries where the heart ended up");
        assert!(queue.is_empty());
    }

    #[test]
    fn a_retry_never_lands_on_top_of_a_newer_heart() {
        let mut queue = LoveQueue::default();
        queue.push(track("Olson"), true);
        let (key, love) = queue.take().unwrap();
        // The send is in flight and the user takes the heart back.
        queue.push(track("Olson"), false);
        // Then the flight fails and comes back for another go.
        queue.retry(
            key,
            Love {
                on: love.on,
                tries: love.tries + 1,
            },
        );
        let (_, pending) = queue.take().unwrap();
        assert!(!pending.on, "the newer heart survives the retry");
        assert_eq!(pending.tries, 0, "and keeps its own full run of attempts");
    }

    #[test]
    fn a_failed_push_climbs_the_backoff_then_stops() {
        let mut queue = LoveQueue::default();
        queue.push(track("Dayvan Cowboy"), true);
        let mut waits = Vec::new();
        while let Some((key, love)) = queue.take() {
            let Some(wait) = LOVE_BACKOFF.get(love.tries).copied() else {
                break;
            };
            waits.push(wait);
            queue.retry(
                key,
                Love {
                    on: love.on,
                    tries: love.tries + 1,
                },
            );
        }
        assert_eq!(waits, LOVE_BACKOFF, "every wait, in order, once each");
        assert!(
            queue.is_empty(),
            "and the push is dropped, not retried forever"
        );
    }

    #[test]
    fn only_service_side_failures_are_worth_another_try() {
        let api = |code: i64| ApiError {
            code: Some(code),
            message: "api said no".to_string(),
        };
        // No code at all is the offline case: the request never landed.
        assert!(ApiError {
            code: None,
            message: "no connection".to_string(),
        }
        .retryable());
        assert!(api(11).retryable(), "service offline");
        assert!(api(16).retryable(), "temporarily unavailable");
        assert!(api(29).retryable(), "rate limited");
        assert!(!api(9).retryable(), "invalid session, and it stays invalid");
        assert!(!api(6).retryable(), "a track Last.fm can't name");
    }
}
