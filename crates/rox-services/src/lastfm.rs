//! Last.fm scrobbling: the scrobbler entity that watches the player and
//! sends the signed calls rox-net makes. The scrobbler rides the
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
//! Which session that lands under follows from the identity signing for
//! it, per ADR 26: Last.fm binds a session to its api key, rox ships a
//! different one per channel, and they all read the same file. So the
//! scrobbler reads the session filed under the key it signs with, and
//! treats a refusal (error 9) as that session being gone rather than as
//! one more failed call, which is the only way a dead connection reaches
//! the screen instead of the log.
//!
//! The same session key carries the favourites mirror: with it armed, a
//! heart in rox becomes a love on Last.fm and taking the heart back
//! unloves it. That half doesn't ride the player at all. It watches the
//! library's favourite set and pushes what moved, through a queue that
//! retries, because a love that quietly failed to send leaves the two
//! sides disagreeing with nothing on screen to say so. The mirror only
//! pushes: nothing here reads Last.fm's loved list back.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, Entity, EventEmitter, SharedString, Subscription};

use rox_library::cue::TrackKey;
use rox_library::store::TrackMeta;

use rox_core::settings::{Lastfm, LastfmSession, Settings};

use crate::catalog::{Library, LibraryEvent};
use crate::player::Player;

// The signing, the call that sends it, and the identity it signs with all
// live in rox-net now; the scrobbler reaches them through the same paths it
// always did.
pub use rox_net::lastfm::{call, has_builtin_keys, keys, ApiError, AuthPhase};

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
    /// Which track played: path and subsong both, since a path alone can't
    /// name one track of a cue rip.
    pub key: TrackKey,
    /// The library row the watch resolved to, None for a file the library
    /// doesn't hold. Carried rather than looked up again by the recorder,
    /// which would only have the path to ask with.
    pub track_id: Option<i64>,
    /// The tag snapshot the event row keeps, off that same lookup.
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
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

/// The playing track under watch: its identity, tags, and how much of it
/// has actually sounded so far.
struct Watch {
    key: TrackKey,
    /// The library row behind the key, None for a file it doesn't know.
    /// Resolved once with the tags below, since both come out of the same
    /// (path, sub) lookup.
    id: Option<i64>,
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
    /// Where the scrobble rule crossed, 0 to 1: stamped once so the
    /// marker stays put after the fact instead of trailing later seeks.
    scrobble_at: Option<f32>,
}

impl Watch {
    /// Project where the scrobble crossing lands, 0 to 1: the current
    /// position plus the listening still owed against the threshold.
    /// Seeked past the end it returns None - this play can't reach the
    /// threshold anymore. A crossed threshold pins the line where it
    /// happened; seeks after the fact have nothing left to move.
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
    /// Only audio that actually sounds counts toward the threshold, so
    /// the line rides the watch: seeks shift where the crossing lands.
    pub fn marker(&self) -> Option<f32> {
        if !self.armed() {
            return None;
        }
        let threshold = self.config.threshold;
        match &self.watch {
            Some(watch) => watch.marker(threshold),
            None => Some(threshold),
        }
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

    /// The session this build signs with, None where it holds none for
    /// its own api key. Sessions are filed by the key that minted them,
    /// so which one this is follows from the identity above.
    fn session(&self) -> Option<&LastfmSession> {
        self.config.session(self.api_key())
    }

    /// The session key the signed calls carry, empty where this build
    /// holds none. The armed switches gate every caller, so an empty one
    /// never actually reaches the wire.
    fn session_key(&self) -> String {
        self.session().map(|s| s.key.clone()).unwrap_or_default()
    }

    /// The connected account's name, for the settings readout.
    pub fn username(&self) -> &str {
        self.config.username(self.api_key())
    }

    /// Whether a session exists under some other api key: a build that
    /// connected before this one, on an install that signs differently.
    pub fn connected_elsewhere(&self) -> bool {
        self.config.connected_elsewhere(self.api_key())
    }

    /// Whether anything could be sent at all: a session in hand and a pair
    /// to sign with. What both switches build on.
    pub fn connected(&self) -> bool {
        self.session().is_some() && !self.api_secret().is_empty()
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

    /// Persist once the edit burst settles, the store-then-settle shape the
    /// EQ curve uses: the config field already carries the value, so only
    /// the file write waits out the drag. A settings write reloads and
    /// reserializes every shard, and per scrub tick that stutters the app.
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
        // Settled, not straight through: this rides a slider scrub, the one
        // scrobbler write that can fire per mouse move.
        self.persist_soon(cx);
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
                    Err(e) => this.phase = AuthPhase::Failed(format!("getting a token: {e}")),
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
                        let api_key = this.api_key().to_string();
                        this.config.connect(&api_key, session_key, username);
                        this.phase = AuthPhase::Idle;
                        this.persist();
                        // A connect is where the mirror becomes possible,
                        // so it starts its line here rather than pushing
                        // the favourites that were already on the shelf.
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

    /// Drop the session locally. Last.fm keeps its side until the user
    /// revokes rox there; a fresh connect just lands a new session. Only
    /// this build's session goes: another install signing with a
    /// different api key keeps the one it authorized itself.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.drop_session(AuthPhase::Idle, cx);
    }

    /// Last.fm refused the session, so it's worthless to this build: the
    /// user revoked rox on the site, or the session was minted under
    /// another install's api key. Same teardown as a disconnect, minus
    /// the user having asked for it, so the phase carries why.
    fn session_rejected(&mut self, cx: &mut Context<Self>) {
        log::warn!("lastfm: the session was rejected, reconnecting is the fix");
        self.drop_session(AuthPhase::Rejected, cx);
    }

    /// Let go of whatever session this build was holding and settle every
    /// piece of state that only made sense while it was good.
    fn drop_session(&mut self, phase: AuthPhase, cx: &mut Context<Self>) {
        let api_key = self.api_key().to_string();
        self.config.clear_session(&api_key);
        self.phase = phase;
        // Nothing queued can be signed any more, and holding it would only
        // flush it at whatever account connects next.
        self.favourites = None;
        self.loves.clear();
        self.love_error = None;
        self.persist();
        cx.notify();
    }

    /// One call came back clean. The only thing riding on that beyond the
    /// call itself is the unattributed session: a landed call is the
    /// proof of who minted it, so this is where it gets claimed.
    fn call_landed(&mut self, cx: &mut Context<Self>) {
        let api_key = self.api_key().to_string();
        if self.config.attribute(&api_key) {
            self.persist();
            cx.notify();
        }
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
                self.call_landed(cx);
                cx.notify();
                return None;
            }
            Err(error) => error,
        };
        // A refused session isn't this heart's problem: every call fails
        // the same way until the account reconnects, so the queue stops
        // here rather than spending its backoff on a certainty.
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
            .map(|watch| watch.key != now.key)
            .unwrap_or(true);
        if changed {
            self.begin_watch(now.key.clone(), now.duration_secs, now.position_secs, cx);
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
                self.begin_watch(now.key.clone(), now.duration_secs, now.position_secs, cx);
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
                    key: watch.key.clone(),
                    track_id: watch.id,
                    title: watch
                        .meta
                        .as_ref()
                        .map(|m| m.title.clone())
                        .unwrap_or_default(),
                    artist: watch
                        .meta
                        .as_ref()
                        .map(|m| m.artist.clone())
                        .unwrap_or_default(),
                    album: watch
                        .meta
                        .as_ref()
                        .map(|m| m.album.clone())
                        .unwrap_or_default(),
                    genre: watch
                        .meta
                        .as_ref()
                        .map(|m| m.genre.clone())
                        .unwrap_or_default(),
                    started: watch.started,
                });
            }
            // Pin the marker at the crossing, armed or not: where the
            // threshold fell is a fact of the play, not of the account.
            if scrobbles && watch.scrobble_at.is_none() {
                watch.scrobble_at = watch
                    .duration
                    .filter(|d| *d > 0.0)
                    .map(|d| (watch.last_pos / d).clamp(0.0, 1.0) as f32);
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
        key: TrackKey,
        duration: Option<f64>,
        position: f64,
        cx: &mut Context<Self>,
    ) {
        let resolved = self.library.read(cx).resolve_key(&key);
        let (id, meta) = match resolved {
            Some((id, meta)) => (Some(id), Some(meta)),
            None => (None, None),
        };
        self.watch = Some(Watch {
            key,
            id,
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
            scrobble_at: None,
        });
    }

    /// Send the watched track to the API: the params the two track
    /// methods share, the timestamp only where the scrobble wants it.
    /// Missing tags skip quietly - Last.fm can't take a track without an
    /// artist and a title.
    ///
    /// The result comes back rather than being dropped where it lands.
    /// Nothing here retries, and a track that failed to send is gone
    /// either way, but a refused session is the app's to notice: without
    /// this the connection reads as fine on screen while every scrobble
    /// falls into the log.
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
            scrobbled: false,
            scrobble_at: None,
        }
    }

    #[test]
    fn the_marker_sits_at_the_threshold_on_a_straight_play() {
        // Played and position agree: nobody seeked, the line is the knob.
        assert_eq!(watch(200.0, 50.0, 50.0).marker(0.5), Some(0.5));
    }

    #[test]
    fn a_seek_forward_pushes_the_crossing_out() {
        // 50s sounded, then a jump to 120s: 50s still owed, landing at 170s.
        assert_eq!(watch(200.0, 50.0, 120.0).marker(0.5), Some(0.85));
    }

    #[test]
    fn a_seek_back_pulls_the_crossing_in() {
        // 80s sounded, rewound to 30s: 20s owed, the line lands at 50s.
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
        // A seek after the scrobble moves nothing.
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
}
