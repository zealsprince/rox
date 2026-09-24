//! Last.fm scrobbling. The scrobbler rides the player's pump, accumulates
//! how much of the track has actually sounded (seeks don't count), and
//! emits two separate signals: [`Crossed`] at the user's threshold, which
//! every scrobble destination sends on, and [`Listened`] at the fixed listen
//! rule history records. The threshold never moves the listen rule.
//!
//! Sessions are filed under the api key that minted them (ADR 26), since
//! each release channel signs with its own key. A refusal (error 9) drops
//! the session on screen rather than failing quietly in the log.
//!
//! The favourites mirror pushes hearts as loves through a retrying queue,
//! diffing the library's favourite set. It never reads Last.fm's loved list
//! back.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, Entity, EventEmitter, SharedString, Subscription};

use rox_library::cue::TrackKey;
use rox_library::store::TrackMeta;
use rox_playback::IcyTitle;

use rox_core::settings::{Lastfm, LastfmSession, Settings, clamp_threshold};

use crate::catalog::{Library, LibraryEvent};
use crate::player::Player;
use crate::radio::{Radio, TitleChanged, live_tags};

pub use rox_net::lastfm::{ApiError, AuthPhase, call, has_builtin_keys, keys};

/// Last.fm rejects scrobbles this short; history uses the same floor.
const MIN_TRACK_SECS: f64 = 30.0;

/// A track counts once half of it has sounded. Not the user's threshold.
const LISTEN_FRACTION: f64 = 0.5;

/// Four minutes counts even when that's less than half a long track.
const LISTEN_CAP_SECS: f64 = 240.0;

/// The fixed listen rule crossed: the one "real listen" signal history
/// records, whatever the threshold or accounts.
pub struct Listened {
    /// Path and subsong: a path alone can't name one track of a cue rip.
    pub key: TrackKey,
    /// Carried in the event: the recorder only has a path to look up with,
    /// which is wrong for a cue rip.
    pub track_id: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub started: u64,
    /// None for a stream that never reported a duration.
    pub duration_secs: Option<f64>,
}

/// The user's threshold crossed: the one signal every scrobble destination
/// sends on. Fires whether or not any account is connected, but never while
/// the shared switch is off.
pub struct Crossed {
    pub key: TrackKey,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub started: u64,
    pub duration_secs: Option<f64>,
}

/// A new track under watch, before any threshold or account gate. Only
/// tracks the library holds tags for are announced.
pub struct Started {
    pub key: TrackKey,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: Option<f64>,
}

fn started_event(
    key: &TrackKey,
    meta: Option<&TrackMeta>,
    duration: Option<f64>,
) -> Option<Started> {
    let meta = meta?;
    Some(Started {
        key: key.clone(),
        title: meta.title.clone(),
        artist: meta.artist.clone(),
        album: meta.album.clone(),
        duration_secs: duration,
    })
}

/// Filed by both the threshold crossing and a station's turnover.
fn listened_event(watch: &Watch) -> Listened {
    Listened {
        key: watch.key.clone(),
        track_id: watch.id,
        title: watch.tag(|m| &m.title),
        artist: watch.tag(|m| &m.artist),
        album: watch.tag(|m| &m.album),
        genre: watch.tag(|m| &m.genre),
        started: watch.started,
        duration_secs: watch.duration,
    }
}

fn crossed_event(watch: &Watch) -> Crossed {
    Crossed {
        key: watch.key.clone(),
        title: watch.tag(|m| &m.title),
        artist: watch.tag(|m| &m.artist),
        album: watch.tag(|m| &m.album),
        started: watch.started,
        duration_secs: watch.duration,
    }
}

/// About an hour of radio. The live buffer can be set longer, and a song
/// stepped back to past the oldest remembered one files again.
const FILED_SONGS: usize = 16;

/// Only a stream's watch closes here: a file crosses the ordinary rules on
/// its own clock, and closing it here too would file it twice.
fn closes_on_turnover(watch: &Watch) -> bool {
    watch.duration.is_none() && watch.played >= MIN_TRACK_SECS
}

/// Stepping back through the buffer re-crosses title marks, and each
/// crossing is a turnover. Without this the songs around the rewind get a
/// second listen apiece. Remembered by title, not timed against the buffer.
fn already_filed(filed: &VecDeque<(TrackKey, IcyTitle)>, key: &TrackKey, title: &IcyTitle) -> bool {
    filed
        .iter()
        .any(|(station, song)| station == key && song == title)
}

fn remember_filed(filed: &mut VecDeque<(TrackKey, IcyTitle)>, song: (TrackKey, IcyTitle)) {
    if already_filed(filed, &song.0, &song.1) {
        return;
    }

    if filed.len() >= FILED_SONGS {
        filed.pop_front();
    }

    filed.push_back(song);
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One entry per retry. A push that runs out is dropped, with its reason
/// kept on the settings page.
const LOVE_BACKOFF: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(120),
];

struct Watch {
    key: TrackKey,
    id: Option<i64>,
    /// Last.fm needs an artist and a title, so untagged tracks watch silently.
    meta: Option<TrackMeta>,
    duration: Option<f64>,
    /// Zero until playback is observed, so a restored paused track doesn't
    /// backdate its scrobble.
    started: u64,
    /// Position deltas at playback speed; seeks don't count.
    played: f64,
    last_pos: f64,
    now_playing_sent: bool,
    /// Set on the listen-rule crossing whether or not scrobbling is armed.
    listened: bool,
    crossed: bool,
    scrobbled: bool,
    /// Kept so closing the watch can say which song was filed.
    live_title: Option<IcyTitle>,
    /// Stamped once, so the marker stays put instead of trailing later seeks.
    scrobble_at: Option<f32>,
}

impl Watch {
    fn tag(&self, pick: fn(&TrackMeta) -> &String) -> String {
        self.meta.as_ref().map(pick).cloned().unwrap_or_default()
    }

    /// The current position plus the listening still owed. None once a seek
    /// puts the threshold out of reach.
    fn marker(&self, threshold: f32) -> Option<f32> {
        if let Some(at) = self.scrobble_at {
            return Some(at);
        }
        let Some(duration) = self.duration.filter(|d| *d > 0.0) else {
            return Some(threshold);
        };
        let owed = (duration * threshold as f64 - self.played).max(0.0);
        let at = ((self.last_pos + owed) / duration) as f32;
        (at <= 1.0).then_some(at)
    }
}

struct Love {
    /// Where the heart ended up, not how many times it was clicked.
    on: bool,
    tries: usize,
}

/// Keyed per track, so a heart flipped twice while offline reaches Last.fm
/// once.
#[derive(Default)]
struct LoveQueue(BTreeMap<(String, String), Love>);

impl LoveQueue {
    /// Replaces whatever was waiting for that track, tries and all.
    fn push(&mut self, key: (String, String), on: bool) {
        self.0.insert(key, Love { on, tries: 0 });
    }

    /// Taken out, not borrowed, so a heart flipped mid-flight queues behind
    /// it cleanly.
    fn take(&mut self) -> Option<((String, String), Love)> {
        let key = self.0.keys().next().cloned()?;
        let love = self.0.remove(&key)?;
        Some((key, love))
    }

    /// Never over a newer heart, or it would send the state the user just
    /// left.
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

struct LoveSend {
    key: (String, String),
    love: Love,
    method: &'static str,
    secret: String,
    params: BTreeMap<String, String>,
}

/// Holds the live config and shared threshold, so the panels' markers never
/// read the settings file per frame.
pub struct Scrobbler {
    library: Entity<Library>,
    config: Lastfm,
    scrobbling: bool,
    threshold: f32,
    phase: AuthPhase,
    watch: Option<Watch>,
    /// Station songs already filed, newest last. See [`already_filed`].
    filed: VecDeque<(TrackKey, IcyTitle)>,
    /// None until there's a set worth trusting: a snapshot before the library
    /// loaded would read as every favourite just taken back.
    favourites: Option<HashSet<i64>>,
    loves: LoveQueue,
    sending: bool,
    love_error: Option<SharedString>,
    /// Shared with the app: two live-title services over one player would
    /// each announce every song.
    radio: Entity<Radio>,
    _player_changed: Subscription,
    _library_changed: Subscription,
    _radio_changed: Subscription,
}

impl EventEmitter<Listened> for Scrobbler {}
impl EventEmitter<Started> for Scrobbler {}
impl EventEmitter<Crossed> for Scrobbler {}

impl Scrobbler {
    pub fn new(
        player: &Entity<Player>,
        library: &Entity<Library>,
        radio: &Entity<Radio>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _player_changed = cx.observe(player, |this: &mut Self, player, cx| {
            this.tick(&player, cx);
        });
        // Every path that moves a heart ends in a playlist change, so diffing
        // the set catches all of them.
        let _library_changed = cx.subscribe(
            library,
            |this: &mut Self, _, event: &LibraryEvent, cx| match event {
                LibraryEvent::PlaylistsChanged => this.mirror_favourites(cx),
                // A rescan can rewrite the ids, so reseed without sending.
                LibraryEvent::Updated => this.seed_favourites(cx),
                _ => {}
            },
        );
        // A stream's only end-of-song signal.
        let _radio_changed = cx.subscribe(radio, |this: &mut Self, _, event: &TitleChanged, cx| {
            this.on_turnover(&event.key, &event.title, cx);
        });

        let settings = Settings::load();
        Scrobbler {
            library: library.clone(),
            config: settings.accounts.lastfm,
            scrobbling: settings.scrobbling,
            threshold: settings.scrobble_threshold,
            phase: AuthPhase::Idle,
            watch: None,
            filed: VecDeque::new(),
            favourites: None,
            loves: LoveQueue::default(),
            sending: false,
            love_error: None,
            radio: radio.clone(),
            _player_changed,
            _library_changed,
            _radio_changed,
        }
    }

    pub fn radio(&self) -> &Entity<Radio> {
        &self.radio
    }

    pub fn config(&self) -> &Lastfm {
        &self.config
    }

    pub fn phase(&self) -> &AuthPhase {
        &self.phase
    }

    pub fn scrobbling(&self) -> bool {
        self.scrobbling
    }

    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    pub fn loves_pending(&self) -> usize {
        self.loves.len()
    }

    pub fn love_error(&self) -> Option<SharedString> {
        self.love_error.clone()
    }

    /// None once the play has seeked past any chance of crossing. Whether a
    /// line shows at all is `AppState::scrobble_marker`'s call.
    pub fn marker(&self) -> Option<f32> {
        match &self.watch {
            Some(watch) => watch.marker(self.threshold),
            None => Some(self.threshold),
        }
    }

    /// The settings override when one was entered, the build's own otherwise.
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

    /// Sessions are filed by the api key that minted them.
    fn session(&self) -> Option<&LastfmSession> {
        self.config.session(self.api_key())
    }

    fn session_key(&self) -> String {
        self.session().map(|s| s.key.clone()).unwrap_or_default()
    }

    pub fn username(&self) -> &str {
        self.config.username(self.api_key())
    }

    pub fn connected_elsewhere(&self) -> bool {
        self.config.connected_elsewhere(self.api_key())
    }

    pub fn connected(&self) -> bool {
        self.session().is_some() && !self.api_secret().is_empty()
    }

    pub fn armed(&self) -> bool {
        self.scrobbling && self.connected()
    }

    /// Its own switch: turning scrobbling off for an evening doesn't stop
    /// the hearts.
    fn loves_armed(&self) -> bool {
        self.config.love_favourites && self.connected()
    }

    fn persist(&self) {
        let lastfm = self.config.clone();
        let scrobbling = self.scrobbling;
        let threshold = self.threshold;
        Settings::update(move |s| {
            s.accounts.lastfm = lastfm;
            s.scrobbling = scrobbling;
            s.scrobble_threshold = threshold;
        });
    }

    /// A settings write reserializes every shard; per scrub tick that stutters.
    fn persist_soon(&self, cx: &mut Context<Self>) {
        static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mine = GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            if GEN.load(std::sync::atomic::Ordering::Relaxed) != mine {
                return;
            }
            this.update(cx, |this, _| this.persist()).ok();
        })
        .detach();
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
        self.scrobbling = on;
        self.persist();
        cx.notify();
    }

    /// Arming snapshots the set, so it mirrors from now on rather than firing
    /// a library's worth of hearts.
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
        self.threshold = clamp_threshold(threshold);
        // Settled: a slider scrub fires this per mouse move.
        self.persist_soon(cx);
        cx.notify();
    }

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
                        let api_key = this.api_key().to_string();
                        this.config.connect(&api_key, session_key, username);
                        this.phase = AuthPhase::Idle;
                        this.persist();
                        // Start the mirror's line here, not pushing the
                        // favourites already on the shelf.
                        this.seed_favourites(cx);
                    }
                    Err(e) => this.phase = AuthPhase::Failed(format!("confirming: {e}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Only this build's session goes; Last.fm keeps its side until revoked.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.drop_session(AuthPhase::Idle, cx);
    }

    /// Revoked on the site, or minted under another install's key.
    fn session_rejected(&mut self, cx: &mut Context<Self>) {
        log::warn!("lastfm: the session was rejected, reconnecting is the fix");
        self.drop_session(AuthPhase::Rejected, cx);
    }

    fn drop_session(&mut self, phase: AuthPhase, cx: &mut Context<Self>) {
        let api_key = self.api_key().to_string();
        self.config.clear_session(&api_key);
        self.phase = phase;
        // Queued hearts would otherwise flush at whatever account connects next.
        self.favourites = None;
        self.loves.clear();
        self.love_error = None;
        self.persist();
        cx.notify();
    }

    /// A successful call proves who minted the unattributed session.
    fn call_landed(&mut self, cx: &mut Context<Self>) {
        let api_key = self.api_key().to_string();
        if self.config.attribute(&api_key) {
            self.persist();
            cx.notify();
        }
    }

    /// Hearts the import just wrote came from Last.fm, so they join the
    /// snapshot unsent. Called in the same update as the write, ahead of the
    /// library event.
    pub fn absorb_favourites(&mut self, cx: &mut Context<Self>) {
        self.seed_favourites(cx);
    }

    fn seed_favourites(&mut self, cx: &mut Context<Self>) {
        if !self.loves_armed() {
            self.favourites = None;
            return;
        }
        let ids = self.library.read(cx).favourite_ids();
        self.favourites = Some(ids);
    }

    fn mirror_favourites(&mut self, cx: &mut Context<Self>) {
        if !self.loves_armed() {
            self.favourites = None;
            return;
        }
        let now = self.library.read(cx).favourite_ids();
        let Some(before) = self.favourites.replace(now.clone()) else {
            // First look since arming: the starting line, not a backlog.
            return;
        };
        let loved: Vec<i64> = now.difference(&before).copied().collect();
        let unloved: Vec<i64> = before.difference(&now).copied().collect();
        if loved.is_empty() && unloved.is_empty() {
            return;
        }
        // Names, not ids: library ids mean nothing to Last.fm.
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

    fn next_love(&mut self) -> Option<LoveSend> {
        if !self.loves_armed() {
            self.loves.clear();
            return None;
        }
        let (key, love) = self.loves.take()?;
        let mut params = BTreeMap::new();
        params.insert("api_key".to_string(), self.api_key().to_string());
        params.insert("sk".to_string(), self.session_key());
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

    fn love_result(
        &mut self,
        send: LoveSend,
        result: Result<serde_json::Value, ApiError>,
        cx: &mut Context<Self>,
    ) -> Option<Duration> {
        let error = match result {
            Ok(_) => {
                self.love_error = None;
                self.call_landed(cx);
                cx.notify();
                return None;
            }
            Err(error) => error,
        };
        // A refused session fails every call, so stop rather than back off.
        if error.session_rejected() {
            self.session_rejected(cx);
            return None;
        }
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

    /// One drain at a time, or two walkers race over the queue.
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
                    Err(_) => break,
                }
            }
            this.update(cx, |this, _| this.sending = false).ok();
        })
        .detach();
    }

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
            .map(|watch| watch.key != now.key)
            .unwrap_or(true);
        if changed {
            self.begin_watch(
                now.key.clone(),
                now.duration_secs,
                now.position_secs,
                None,
                cx,
            );
        } else {
            let watch = self.watch.as_mut().expect("watch exists when unchanged");
            if now.duration_secs.is_some() {
                watch.duration = now.duration_secs;
            }
            let delta = now.position_secs - watch.last_pos;
            if counts_as_listening(playing, delta) {
                watch.played += delta;
            } else if delta < -5.0 && watch.listened && now.position_secs < 5.0 {
                // Back to the top after a counted listen is a fresh play.
                self.begin_watch(
                    now.key.clone(),
                    now.duration_secs,
                    now.position_secs,
                    None,
                    cx,
                );
                return;
            }
            watch.last_pos = now.position_secs;
        }

        // Stamp the start when audio first moves: a restored track starts
        // paused, and Last.fm reads this as when it started playing.
        if let Some(watch) = self.watch.as_mut()
            && watch.started == 0
            && playing
        {
            watch.started = unix_now();
        }

        let listens = self.watch.as_ref().is_some_and(Self::qualifies_listen);
        let scrobbles = self
            .watch
            .as_ref()
            .is_some_and(|w| self.qualifies_scrobble(w));

        let scrobbling = self.scrobbling;
        if let Some(watch) = self.watch.as_mut() {
            if listens && !watch.listened {
                watch.listened = true;
                let event = listened_event(watch);
                cx.emit(event);
            }
            // Pin the marker at the crossing, armed or not.
            if scrobbles && watch.scrobble_at.is_none() {
                watch.scrobble_at = watch
                    .duration
                    .filter(|d| *d > 0.0)
                    .map(|d| (watch.last_pos / d).clamp(0.0, 1.0) as f32);
            }
            // The shared switch is the one gate every destination shares,
            // kept here so none of them has to ask.
            if scrobbles && !watch.crossed {
                watch.crossed = true;
                if scrobbling {
                    let event = crossed_event(watch);
                    cx.emit(event);
                }
            }
        }

        if !self.armed() {
            return;
        }

        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        // Waits for audio to move, so a restored paused track announces nothing.
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

    fn qualifies_listen(watch: &Watch) -> bool {
        watch
            .duration
            .filter(|d| *d > MIN_TRACK_SECS)
            .is_some_and(|d| watch.played >= (d * LISTEN_FRACTION).min(LISTEN_CAP_SECS))
    }

    /// Deliberately its own line, not the fixed listen rule.
    fn qualifies_scrobble(&self, watch: &Watch) -> bool {
        watch
            .duration
            .filter(|d| *d > MIN_TRACK_SECS)
            .is_some_and(|d| watch.played >= d * self.threshold as f64)
    }

    /// The listened clock starts empty wherever the position is. `live` is a
    /// station's in-band title on a turnover; the row stays the station's.
    fn begin_watch(
        &mut self,
        key: TrackKey,
        duration: Option<f64>,
        position: f64,
        live: Option<&IcyTitle>,
        cx: &mut Context<Self>,
    ) {
        let refiled = live.is_some_and(|title| already_filed(&self.filed, &key, title));
        let resolved = self.library.read(cx).resolve_key(&key);
        let (id, meta) = match resolved {
            Some((id, meta)) => (Some(id), Some(meta)),
            None => (None, None),
        };

        let meta = match live {
            Some(title) => Some(live_tags(meta, title)),

            None => meta,
        };
        // Emitted here so a loop back to the top announces like a track
        // change. Before any account gate; only the shared switch stands
        // ahead of it.
        if self.scrobbling
            && let Some(event) = started_event(&key, meta.as_ref(), duration)
        {
            cx.emit(event);
        }
        self.watch = Some(Watch {
            key,
            id,
            meta,
            duration,
            started: 0,
            played: 0.0,
            last_pos: position,
            now_playing_sent: false,
            // A song already filed once is armed as filed, so a rewind
            // doesn't file it twice.
            listened: refiled,
            crossed: refiled,
            scrobbled: refiled,
            scrobble_at: None,
            live_title: live.cloned(),
        });
    }

    /// Guarded on the key, so a turnover published a tick after a skip away
    /// doesn't land on whatever plays now.
    fn on_turnover(&mut self, key: &TrackKey, title: &IcyTitle, cx: &mut Context<Self>) {
        let Some(watch) = self.watch.as_ref() else {
            return;
        };
        if &watch.key != key {
            return;
        }

        let position = watch.last_pos;
        self.close_stream_watch(cx);
        self.begin_watch(key.clone(), None, position, Some(title), cx);
    }

    /// A stream's watch never crosses the threshold rules, which divide by a
    /// duration it doesn't have, so the turnover files it.
    fn close_stream_watch(&mut self, cx: &mut Context<Self>) {
        let scrobbling = self.scrobbling;
        let armed = self.armed();

        let Some(watch) = self.watch.as_mut() else {
            return;
        };

        if !closes_on_turnover(watch) {
            return;
        }

        let listen = (!watch.listened).then(|| {
            watch.listened = true;
            listened_event(watch)
        });
        let crossed = (!watch.crossed && scrobbling).then(|| {
            watch.crossed = true;
            crossed_event(watch)
        });
        let scrobble = armed && !watch.scrobbled;
        if scrobble {
            watch.scrobbled = true;
        }

        let filed = listen.is_some() || crossed.is_some() || scrobble;
        let song = watch
            .live_title
            .clone()
            .map(|title| (watch.key.clone(), title));

        if let Some(listen) = listen {
            cx.emit(listen);
        }
        if let Some(crossed) = crossed {
            cx.emit(crossed);
        }
        if scrobble {
            self.submit("track.scrobble", cx);
        }

        if let Some(song) = song.filter(|_| filed) {
            remember_filed(&mut self.filed, song);
        }
    }

    /// Nothing retries, but a rejected session is surfaced: otherwise the
    /// connection reads as fine while every scrobble fails in the log.
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
        params.insert("sk".to_string(), self.session_key());
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
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { call(method, &secret, params) })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(_) => this.call_landed(cx),
                Err(e) if e.session_rejected() => this.session_rejected(cx),
                Err(e) => log::warn!("lastfm: {method}: {e}"),
            })
            .ok();
        })
        .detach();
    }
}

/// Anything bigger than a tick is a seek, and anything while paused doesn't
/// count however small: otherwise the step keys could walk a track to its
/// scrobble line without anyone listening.
fn counts_as_listening(playing: bool, delta: f64) -> bool {
    playing && delta > 0.0 && delta <= 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str) -> (String, String) {
        ("Boards of Canada".to_string(), title.to_string())
    }

    fn watch(duration: f64, played: f64, pos: f64) -> Watch {
        Watch {
            key: TrackKey::from(std::path::PathBuf::from("/music/track.flac")),
            id: None,
            meta: None,
            duration: Some(duration),
            started: 0,
            played,
            last_pos: pos,
            now_playing_sent: false,
            listened: false,
            crossed: false,
            scrobbled: false,
            scrobble_at: None,
            live_title: None,
        }
    }

    #[test]
    fn a_song_filed_once_is_not_filed_again_on_the_way_back_through() {
        let station = TrackKey::from(std::path::PathBuf::from("http://example.invalid/live"));
        let other = TrackKey::from(std::path::PathBuf::from("http://example.invalid/other"));
        let song = |name: &str| IcyTitle {
            artist: "Boards of Canada".into(),
            title: name.into(),
        };

        let mut filed = VecDeque::new();
        remember_filed(&mut filed, (station.clone(), song("Roygbiv")));

        assert!(already_filed(&filed, &station, &song("Roygbiv")));
        assert!(!already_filed(&filed, &station, &song("Olson")));
        assert!(
            !already_filed(&filed, &other, &song("Roygbiv")),
            "another station's song is its own"
        );

        remember_filed(&mut filed, (station.clone(), song("Roygbiv")));
        assert_eq!(filed.len(), 1);

        for n in 0..FILED_SONGS {
            remember_filed(&mut filed, (station.clone(), song(&format!("track {n}"))));
        }
        assert_eq!(filed.len(), FILED_SONGS);
        assert!(!already_filed(&filed, &station, &song("Roygbiv")));
    }

    #[test]
    fn a_tick_of_playback_counts_and_a_seek_doesnt() {
        assert!(counts_as_listening(true, 0.016));
        assert!(counts_as_listening(true, 1.0));
        assert!(!counts_as_listening(true, 5.0), "a seek forward");
        assert!(!counts_as_listening(true, -0.5), "a seek back");
        assert!(!counts_as_listening(true, 0.0), "nothing moved");
    }

    #[test]
    fn a_paused_step_and_its_blip_dont_count() {
        assert!(!counts_as_listening(false, 0.025));
        assert!(!counts_as_listening(false, 0.1));
    }

    #[test]
    fn the_marker_sits_at_the_threshold_on_a_straight_play() {
        assert_eq!(watch(200.0, 50.0, 50.0).marker(0.5), Some(0.5));
    }

    #[test]
    fn a_seek_forward_pushes_the_crossing_out() {
        // 50s sounded, then a jump to 120s: 50s still owed, crossing at 170s.
        assert_eq!(watch(200.0, 50.0, 120.0).marker(0.5), Some(0.85));
    }

    #[test]
    fn a_seek_back_pulls_the_crossing_in() {
        // 80s sounded, rewound to 30s: 20s owed, the line falls at 50s.
        assert_eq!(watch(200.0, 80.0, 30.0).marker(0.5), Some(0.25));
    }

    #[test]
    fn seeked_past_reach_the_marker_disappears() {
        // 10s sounded, jumped to 150s: 90s owed with 50s left in the track.
        assert_eq!(watch(200.0, 10.0, 150.0).marker(0.5), None);
    }

    #[test]
    fn a_crossed_threshold_pins_the_line() {
        let mut w = watch(200.0, 100.0, 100.0);
        w.scrobble_at = Some(0.5);
        w.last_pos = 180.0;
        assert_eq!(w.marker(0.5), Some(0.5));
    }

    #[test]
    fn no_duration_falls_back_to_the_knob() {
        let mut w = watch(200.0, 0.0, 0.0);
        w.duration = None;
        assert_eq!(w.marker(0.5), Some(0.5));
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
        queue.push(track("Olson"), false);
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
    fn a_watch_only_announces_a_start_when_it_has_tags() {
        let key = TrackKey::from(std::path::PathBuf::from("/music/track.flac"));
        let meta = TrackMeta {
            title: "Roygbiv".into(),
            artist: "Boards of Canada".into(),
            album: "Music Has the Right to Children".into(),
            track_no: 8,
            album_artist: "Boards of Canada".into(),
            year: 1998,
            genre: "Electronic".into(),
            duration_ms: 151_000,
            codec: "flac".into(),
            bitrate_kbps: 900,
            sample_rate_hz: 44_100,
            bit_depth: 16,
            rating: 0,
        };
        let event = started_event(&key, Some(&meta), Some(151.0)).expect("tags in hand");
        assert_eq!(event.artist, "Boards of Canada");
        assert_eq!(event.title, "Roygbiv");
        assert_eq!(event.duration_secs, Some(151.0));
        assert!(started_event(&key, None, Some(151.0)).is_none());
    }

    fn station_row(name: &str) -> TrackMeta {
        TrackMeta {
            title: name.into(),
            artist: String::new(),
            album: String::new(),
            track_no: 0,
            album_artist: String::new(),
            year: 0,
            genre: "Jazz".into(),
            duration_ms: 0,
            codec: String::new(),
            bitrate_kbps: 0,
            sample_rate_hz: 0,
            bit_depth: 0,
            rating: 0,
        }
    }

    fn stream_watch(played: f64) -> Watch {
        let mut watch = watch(0.0, played, played);
        watch.duration = None;
        watch
    }

    #[test]
    fn a_turnover_files_a_stream_that_played_long_enough() {
        assert!(closes_on_turnover(&stream_watch(60.0)));

        assert!(!closes_on_turnover(&stream_watch(10.0)));

        assert!(!closes_on_turnover(&watch(200.0, 60.0, 60.0)));
    }

    #[test]
    fn a_turnover_files_the_song_against_the_stations_row() {
        let mut watch = stream_watch(60.0);
        watch.id = Some(42);
        watch.meta = Some(live_tags(
            Some(station_row("Jazz Forever")),
            &IcyTitle {
                artist: "Miles Davis".into(),
                title: "So What".into(),
            },
        ));

        let listen = listened_event(&watch);
        assert_eq!(listen.track_id, Some(42), "the station's row played");
        assert_eq!(listen.title, "So What");
        assert_eq!(listen.artist, "Miles Davis");
        assert_eq!(listen.album, "Jazz Forever", "the station stands in");
        assert_eq!(listen.genre, "Jazz", "off the row, not the stream");
        assert_eq!(listen.duration_secs, None, "a stream still has no length");

        let crossed = crossed_event(&watch);
        assert_eq!(crossed.title, "So What");
        assert_eq!(crossed.artist, "Miles Davis");
    }
}
