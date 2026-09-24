//! The playback service entity: one running engine session behind the
//! playback contract (commands in over a channel, state out through shared
//! atomics). A headless pump task drains the PCM tap on a timer, not a
//! render pass, so the audio views' feed keeps flowing whichever windows are
//! drawing. The player renders nothing itself; the transport panels are the
//! UI over this state.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, Global, SharedString, Subscription, Task};

use rox_core::QUEUE_CAP;
use rox_core::settings::{
    GainModeSetting, ReplayGainSave, ReplayGainSettings, Settings, ShuffleMode,
    clamp_live_buffer_secs,
};
use rox_library::cue::{Origin, Span, TrackKey};
use rox_library::embeddings;
use rox_library::locator::Locator;
use rox_library::song;
use rox_library::store;
use rox_playback::IcyTitle;
use rox_playback::LiveGap;
use rox_playback::LiveMark;
use rox_playback::Shift;
use rox_playback::StationInfo;
use rox_playback::StreamState;
use rox_playback::continuation::{self, Pick};
use rox_playback::engine::{self, Cmd, StartQueue, shuffle_head, shuffle_slice};
use rox_playback::eq::{Eq, EqParams};
use rox_playback::gain;
use rox_playback::output::{self, Mode, Negotiated, Request};
use rox_playback::rtrb::Consumer;
use rox_playback::shared::{QueueEntry, QueueSnapshot, Shared};
use rox_viz::AudioFeed;

use crate::catalog::Library;
use crate::sources_registry;

pub use rox_core::fmt::{fmt_time, fmt_time_padded};
// Re-exported so crates calling [`Player::loop_mode`] can name the type.
pub use rox_playback::engine::LoopMode;

/// Pump cadence, about one frame. The tap ring holds 16,384 samples (about
/// 170 ms at 48 kHz stereo), so a tick has plenty of headroom before the
/// callback starts dropping pushes.
const PUMP_INTERVAL: Duration = Duration::from_millis(16);

/// How many times a second a paused station repaints as its timeshift grows.
const PAUSED_SHIFT_STEPS: f64 = 30.0;

/// How long the similarity ordering waits for a fresh context to publish its
/// queue. The decode thread publishes first thing, so this is only a ceiling.
const QUEUE_WAIT_TRIES: usize = 40;
const QUEUE_WAIT_STEP: Duration = Duration::from_millis(25);

/// How long a track has to play before a skip starts a fresh run instead of
/// widening the last one.
const SKIP_SETTLE: Duration = Duration::from_secs(30);

/// The skip band: the first skip draws from the nearest handful, each one
/// after multiplies it, so a few in a row leave the genre.
const SKIP_BAND_BASE: usize = 4;
const SKIP_BAND_GROWTH: usize = 4;

/// How many of the nearest tracks a similar draw picks from: wide enough that
/// two presses differ, narrow enough that it all sounds like the seed.
const SIMILAR_BAND: usize = 8;

/// How far down the ranking the draw reads to fill the band. The band is one
/// track per song, and a song held seventeen times fills the top of it.
const SIMILAR_POOL: usize = SIMILAR_BAND * 16;

/// How many tracks apart the similarity ordering spreads recordings of one
/// song. Nothing is dropped from the listener's queue.
const SONG_SPACING: usize = 25;

/// Uses std's per-process random hasher keys to avoid a rand dependency.
fn random_index(len: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    let hash = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (hash % len as u64) as usize
}

/// Random reads the same scope continuation does (ADR 17): the view playback
/// started in while it holds anything, the whole library otherwise.
fn random_pool<'a>(scope: &'a continuation::Scope, library: &'a [i64]) -> &'a [i64] {
    match scope {
        continuation::Scope::View(ids) if !ids.is_empty() => ids,
        _ => library,
    }
}

/// How many tracks the draws' no-repeat memory keeps across session starts.
const HEARD_CAP: usize = QUEUE_CAP;

/// Fold a session pool into the heard ring, newest last, deduped, oldest
/// evicted. Starting a session replaces the pool the repeat guard reads, so
/// without this two Play Similar presses bounce between each other's bands.
fn remember_held(heard: &mut VecDeque<i64>, pool: impl Iterator<Item = i64>) {
    for id in pool {
        if let Some(at) = heard.iter().position(|&held| held == id) {
            heard.remove(at);
        }
        heard.push_back(id);
    }
    while heard.len() > HEARD_CAP {
        heard.pop_front();
    }
}

/// A random index into `pool` the session hasn't held, or anywhere once it
/// has held the whole pool (the ADR 17 no-repeat promise).
fn draw_at(pool: &[i64], seen: &HashSet<i64>) -> Option<usize> {
    if pool.is_empty() {
        return None;
    }
    let fresh: Vec<usize> = (0..pool.len())
        .filter(|&at| !seen.contains(&pool[at]))
        .collect();
    Some(match fresh.is_empty() {
        true => random_index(pool.len()),
        false => fresh[random_index(fresh.len())],
    })
}

/// A random entry of `pool` with the run around it, like double clicking a
/// row. Only the drawn spot avoids `seen`: an album the draw fell inside
/// plays whole.
fn draw_run(
    library: &Library,
    pool: &[i64],
    seen: &HashSet<i64>,
) -> Option<(Vec<TrackKey>, usize)> {
    let at = draw_at(pool, seen)?;
    let drawn = library.keys_for(&[pool[at]]).ok()?.pop()?;
    let (lo, hi) = run_window(at, pool.len());
    let keys = library.keys_for(&pool[lo..hi]).ok()?;
    // Missing files drop out of the resolve, so find the cursor by key.
    let start = keys.iter().position(|key| *key == drawn)?;
    Some((keys, start))
}

/// The similar band minus the session's plays, whole again once all were
/// heard. Don't widen past the band: a repeating neighbourhood beats a press
/// that wanders off the seed's sound.
fn fresh_band(near: &[(i64, f32)], seen: &HashSet<i64>) -> Vec<i64> {
    let fresh: Vec<i64> = near
        .iter()
        .map(|&(id, _)| id)
        .filter(|id| !seen.contains(id))
        .collect();
    if fresh.is_empty() {
        return near.iter().map(|&(id, _)| id).collect();
    }
    fresh
}

/// The ranking thinned to one track per song, cut to `SIMILAR_BAND`, without
/// the seed's song or anything already heard. Copies of a song are each
/// other's nearest neighbours by sound, so only the tag identity separates
/// them. Returns the whole ranking when nothing survives.
fn one_per_song(
    conn: &rox_library::rusqlite::Connection,
    seed: i64,
    seen: &HashSet<i64>,
    near: &[(i64, f32)],
) -> Vec<(i64, f32)> {
    let candidates: Vec<i64> = near.iter().map(|&(id, _)| id).collect();
    let mut lookup = candidates.clone();
    lookup.push(seed);
    lookup.extend(seen.iter().copied());
    let Ok(keys) = song::keys_for(conn, &lookup) else {
        return near.to_vec();
    };
    let blocked = song::keys_of(&keys, seen.iter().copied().chain([seed]));
    let kept: HashSet<i64> = song::distinct(&candidates, &keys, &blocked, SIMILAR_BAND)
        .into_iter()
        .collect();
    if kept.is_empty() {
        return near.to_vec();
    }
    near.iter()
        .copied()
        .filter(|(id, _)| kept.contains(id))
        .collect()
}

/// The song identity of the seed and of each ranked entry, keyed by entry id.
/// Untagged entries are absent, so they're never spaced out as duplicates.
fn entry_songs(
    conn: &rox_library::rusqlite::Connection,
    seed: i64,
    ranked: &[(u64, i64, f32)],
) -> (Option<String>, HashMap<u64, String>) {
    let mut lookup: Vec<i64> = ranked.iter().map(|&(_, id, _)| id).collect();
    lookup.push(seed);
    let Ok(keys) = song::keys_for(conn, &lookup) else {
        return (None, HashMap::new());
    };
    let songs = ranked
        .iter()
        .filter_map(|&(entry, id, _)| Some((entry, keys.get(&id)?.clone())))
        .collect();
    (keys.get(&seed).cloned(), songs)
}

/// At most `QUEUE_CAP` tracks around `at`, half behind it so Prev has
/// somewhere to go.
fn run_window(at: usize, len: usize) -> (usize, usize) {
    let lo = at
        .saturating_sub(QUEUE_CAP / 2)
        .min(len.saturating_sub(QUEUE_CAP));
    (lo, (lo + QUEUE_CAP).min(len))
}

/// Whether the queue is close enough to its end to ask for more (ADR 17).
/// Loop suppresses it: the listener asked to stay in this list.
fn queue_running_dry(upcoming: usize, loop_mode: LoopMode) -> bool {
    loop_mode == LoopMode::Off && upcoming <= continuation::FLOOR
}

/// Whether continuation should extend the session (ADR 17). A paused queue
/// doesn't grow, so the launch restore stays put. An ended queue still reads
/// as playing, which is how the appended batch wakes it. Stop-after stays
/// armed after the stop, so this says no until it's cleared.
fn continuation_wanted(playing: bool, stop_after: bool) -> bool {
    playing && !stop_after
}

/// Whether engaging Similar should draw a radio batch now, ahead of the
/// browse-order tracks still queued, instead of waiting for the floor. Off
/// still wins, as it does in [`continuation::provider`].
fn similar_draw_now(
    mode: continuation::Mode,
    similar: bool,
    playing: bool,
    stop_after: bool,
) -> bool {
    similar && mode != continuation::Mode::Off && continuation_wanted(playing, stop_after)
}

/// Whether the pump owes the queue a fresh ranking now the audible track has
/// moved to `audible`. `skipped` means a skip already re-seeded this boundary
/// at its widened band, and ranking again at a band of one would undo that.
/// No marker yet is a fresh session, which orders its own tail.
fn reseed_at_boundary(similar: bool, skipped: bool, audible: usize, last: Option<usize>) -> bool {
    similar && !skipped && last.is_some_and(|at| at != audible)
}

/// A skip's re-ranking claim on the boundary it causes.
///
/// The two race and the boundary usually wins: the ranking waits for the
/// engine to adopt the new track, while the boundary check runs off a 16 ms
/// pump. So the claim is made when the skip fires and released when the
/// ranking finishes either way. A claim left standing over a ranking that
/// gave up leaves the tail ordered against a track the listener already left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SkipReseed {
    /// No skip ranking outstanding.
    Idle,
    /// `passed` records that the boundary arrived before the ranking.
    InFlight { passed: bool },
    /// The ranking went out first; the next boundary spends it.
    Ready,
}

impl SkipReseed {
    /// Returns the new claim and whether this boundary is already paid for.
    fn spend(self) -> (Self, bool) {
        match self {
            Self::Idle => (Self::Idle, false),
            Self::InFlight { .. } => (Self::InFlight { passed: true }, true),
            Self::Ready => (Self::Idle, true),
        }
    }

    fn landed(self) -> Self {
        match self {
            Self::InFlight { passed: false } => Self::Ready,
            _ => Self::Idle,
        }
    }

    /// Returns whether a boundary was held off for the abandoned ranking, in
    /// which case that re-seed is still owed.
    fn abandon(self) -> (Self, bool) {
        (Self::Idle, self == Self::InFlight { passed: true })
    }
}

fn skip_band(skips: u32) -> usize {
    if skips == 0 {
        return 1;
    }
    SKIP_BAND_BASE.saturating_mul(SKIP_BAND_GROWTH.saturating_pow(skips - 1))
}

/// One running engine. Dropping it sends Quit and tears the stream down.
struct Session {
    shared: Arc<Shared>,
    tx: mpsc::Sender<Cmd>,
    tap: Consumer<f32>,
    /// An Option because a faulted device is released before its replacement
    /// opens: a backend asked for the same card while the dead handle holds
    /// it answers busy.
    stream: Option<Box<dyn output::OutputStream>>,
    device_rate: u32,
    /// What the output layer actually got, as opposed to what was asked for.
    negotiated: output::Negotiated,
    /// Keys, because two cue tracks of one image are the same path twice in
    /// the engine's pool.
    queue: Vec<TrackKey>,
    /// The ReplayGain tags in pool order, so the status readout shows what the
    /// playing file is levelled by rather than the setting.
    gains: Vec<gain::ReplayGain>,
    /// Off the locator rather than the key's source: one source can serve both
    /// stations and fixed-length files.
    live: Vec<bool>,
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    fn start(
        queue: StartQueue,
        keys: Vec<TrackKey>,
        volume: f32,
        loop_mode: LoopMode,
        shuffle: Option<bool>,
        stop_after: bool,
        paused_at: Option<f64>,
        crossfade: (f32, bool),
        rule: gain::GainRule,
        output: output::Request,
    ) -> Result<Session, String> {
        let shared = Arc::new(Shared::new(queue.locators.len()));
        shared
            .volume_bits
            .store(volume.to_bits(), Ordering::Relaxed);
        let out = output::open(&output, &shared)?;
        let device_rate = out.sample_rate;
        let (tx, rx) = mpsc::channel::<Cmd>();
        let _ = tx.send(Cmd::SetLoop(loop_mode));
        // A restore passes None: its saved order is already shuffled, and
        // re-sending would reshuffle the unplayed tail.
        if let Some(on) = shuffle {
            let _ = tx.send(Cmd::SetShuffle(on));
        }
        if stop_after {
            let _ = tx.send(Cmd::SetStopAfter(true));
        }
        // The restore's pause is a store, not a command: the engine reads this
        // flag before the channel, and a station opened at the top of `run`
        // would otherwise connect through a pause not yet delivered.
        if let Some(secs) = paused_at {
            shared.playing.store(false, Ordering::Relaxed);

            if queue
                .locators
                .get(queue.start)
                .is_some_and(seeks_on_restore)
            {
                let _ = tx.send(Cmd::Seek(secs));
            }
        }
        // Fade and gain settings go ahead of the first decode, so the first
        // track and boundary already have them.
        let _ = tx.send(Cmd::SetCrossfade {
            secs: crossfade.0,
            albums: crossfade.1,
        });
        let _ = tx.send(Cmd::SetGainRule(rule));
        // The EQ joins the chain (ADR 19) before the first buffer. It's the
        // only chain command sent: later knob turns are atomic stores.
        let _ = tx.send(Cmd::ChainPush(Box::new(Eq::new(eq_params().clone()))));
        let gains = queue.gains.clone();
        let live = live_flags(&queue.locators);
        let engine = engine::Engine::new(queue, shared.clone(), out.producer, device_rate, rx);
        std::thread::Builder::new()
            .name("decode".into())
            .spawn(move || engine.run())
            .map_err(|e| format!("spawn decode thread: {e}"))?;
        Ok(Session {
            shared,
            tx,
            tap: out.tap,
            stream: Some(out.stream),
            device_rate,
            negotiated: out.negotiated,
            queue: keys,
            gains,
            live,
        })
    }
}

/// Computed once at insert time, since the seek strip and waveform ask every
/// pump tick.
fn live_flags(locators: &[Locator]) -> Vec<bool> {
    locators
        .iter()
        .map(|l| matches!(l, Locator::Remote(remote) if remote.live))
        .collect()
}

/// A live stream has no position to restore: its saved seconds name a moment
/// that's gone. Remote files still seek.
fn seeks_on_restore(start: &Locator) -> bool {
    !matches!(start, Locator::Remote(remote) if remote.live)
}

/// An all-station batch replaces the queue instead of joining it: a stream
/// never ends, so nothing behind it would play. A mixed batch is a list.
fn replaces_queue(keys: &[TrackKey]) -> bool {
    !keys.is_empty() && keys.iter().all(|key| key.origin() == Origin::Radio)
}

struct QueueMeta {
    groups: Vec<Option<u64>>,
    gains: Vec<gain::ReplayGain>,
    ids: Vec<Option<i64>>,
    /// The slice of the file each key plays, None for a plain file.
    spans: Vec<Option<Span>>,
}

/// Resolve keys against the library, or defaults with no database. A key the
/// library doesn't hold still plays, with nothing to level or splice by.
fn resolve_queue_meta(
    conn: Option<&rox_library::rusqlite::Connection>,
    keys: &[TrackKey],
) -> QueueMeta {
    let mut meta = QueueMeta {
        groups: Vec::with_capacity(keys.len()),
        gains: Vec::with_capacity(keys.len()),
        ids: Vec::with_capacity(keys.len()),
        spans: Vec::with_capacity(keys.len()),
    };
    for key in keys {
        // A local path comes off the filesystem and may not be UTF-8.
        let row = conn
            .zip(key.path.to_str())
            .and_then(|(conn, path)| {
                store::queue_meta_for_key(conn, &key.source, path, key.sub).ok()
            })
            .unwrap_or_default();
        let rg = row.replay_gain;
        meta.groups.push(row.group);
        meta.gains.push(gain::ReplayGain {
            track_db: rg.track_db,
            track_peak: rg.track_peak,
            album_db: rg.album_db,
            album_peak: rg.album_peak,
        });
        meta.ids.push(row.id);
        meta.spans.push(row.span);
    }
    meta
}

/// Where each key plays from. A remote key reads its URL and live flag off
/// the row, and the sources registry adds the auth. A pruned row answers with
/// an empty URL so the open fails instead of reading whatever file sits at
/// the id's path.
fn resolve_locators(
    conn: Option<&rox_library::rusqlite::Connection>,
    keys: &[TrackKey],
) -> Vec<Locator> {
    keys.iter()
        .map(|key| {
            if key.is_local() {
                return Locator::Local(key.path.clone());
            }

            // Sub-aware, so a source that splits one reference into subsongs
            // resolves the queued row.
            let row = conn.zip(key.path.to_str()).and_then(|(conn, path)| {
                let id = store::queue_meta_for_key(conn, &key.source, path, key.sub)
                    .ok()?
                    .id?;
                store::locators_for(conn, &[id]).ok()?.pop()
            });

            let mut remote = match row {
                Some(Locator::Remote(remote)) => remote,

                _ => rox_library::locator::Remote {
                    url: String::new(),
                    headers: Vec::new(),
                    hint: String::new(),
                    live: false,
                },
            };

            sources_registry::authorize(&key.source, &mut remote);
            Locator::Remote(remote)
        })
        .collect()
}

/// A snapshot of the playing track for the audio views. Whether audio is
/// actually moving comes from the feed's tap.
#[derive(Clone)]
pub struct NowPlaying {
    /// Two cue tracks of one image share a path, so anything naming the track
    /// reads the whole key.
    pub key: TrackKey,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    /// The queue resolver matches on this rather than the path, so a file
    /// queued twice resolves to the occurrence playing now.
    pub audible_idx: usize,
    /// A station. Timeline surfaces branch on this rather than a zero
    /// duration, which a file also has while it opens.
    pub live: bool,
    /// The station clock at the start of the current song, None until the
    /// stream announces a title. Read off the buffer's title marks, so it
    /// follows the playhead back through the buffer. See [`song_clock`].
    pub song_start_secs: Option<f64>,
    /// This listen heard the current song begin, so `song_start_secs` is a
    /// real boundary. False for a mid-song join and once the cursor steps
    /// back behind the recorded turnover. Synced lyrics must check this.
    pub song_from_start: bool,
    pub origin: Origin,
    /// None for a local file.
    pub stream: Option<StreamState>,
    /// How far behind the broadcast this plays and how much is held, None off
    /// a live stream. Both grow while paused, so surfaces drawing them
    /// repaint through a pause.
    pub shift: Option<Shift>,
}

impl NowPlaying {
    /// None for a remote track: its path is the source's id, not a file.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.key.is_local().then_some(self.key.path.as_path())
    }
}

/// Where the song under the playhead began, on the `position_secs` clock. The
/// buffer's title marks win where they reach; `observed` covers the seconds
/// after a connect, before any title is behind the playhead.
fn song_start_of(position_secs: f64, shift: Option<Shift>, observed: Option<f64>) -> Option<f64> {
    shift
        .and_then(|shift| shift.song_secs)
        .map(|into| position_secs - into)
        .or(observed)
}

/// Time into the song once a station names one, otherwise the position.
/// Floors at zero: a seek or rejoin can leave the position behind the
/// recorded start for a tick.
pub fn song_clock(position_secs: f64, song_start_secs: Option<f64>) -> f64 {
    match song_start_secs {
        Some(start) => (position_secs - start).max(0.0),
        None => position_secs,
    }
}

/// Where a relative seek of `delta` lands, as seconds behind the live edge,
/// clamped between the edge and the oldest held second. None when the step
/// moves less than the edge snap: every live seek costs a rebuilt decoder
/// and an audible cut.
fn live_step_target(delta: f64, shift: &Shift) -> Option<f64> {
    let behind = shift.behind_secs;
    let target = (behind - delta).clamp(0.0, shift.window_secs.max(0.0));

    ((target - behind).abs() >= rox_playback::LIVE_EDGE_SNAP_SECS).then_some(target)
}

/// When the audible station last moved to a new song, as the pump saw it.
///
/// The revision is global, so an unchanged one proves nothing published,
/// while a matching title proves this entry is still on the same song. The
/// revision keeps the common tick off the title lock.
struct SongStart {
    idx: usize,
    rev: u64,
    title: IcyTitle,
    at_secs: f64,
    /// False for the title the stream already carried when it opened.
    from_start: bool,
}

/// No when nothing live is audible, the record is another entry's or the
/// title carried at open, or the cursor stepped back behind the turnover.
fn song_heard_from_start(last: Option<&SongStart>, live: bool, idx: usize, secs: f64) -> bool {
    last.is_some_and(|last| live && last.idx == idx && last.from_start && secs >= last.at_secs)
}

/// A pause rejoin republishes the title it hung up on, and this keeps the
/// resumed clock where it was.
fn starts_new_song(last: Option<&SongStart>, idx: usize, title: &IcyTitle) -> bool {
    match last {
        Some(last) => last.idx != idx || &last.title != title,
        None => true,
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A station stuck in its reconnect schedule would hold the decode
        // thread for seconds.
        self.shared.interrupt();
        let _ = self.tx.send(Cmd::Quit);
    }
}

/// Commands the listener is waiting on. They trip the engine's interrupt, so
/// a dropped station abandons its reconnect backoff instead of blocking the
/// channel. Settings like volume or crossfade aren't worth cutting a
/// station's recovery short for.
fn answers_now(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::TogglePause
            | Cmd::SeekLive(_)
            | Cmd::Next
            | Cmd::Prev
            | Cmd::Jump { .. }
            | Cmd::RemoveMany { .. }
            | Cmd::Quit
            | Cmd::Insert { and_play: true, .. }
    )
}

/// How finely crossfade progress is reported. Past this nothing on screen
/// moves, and a gated panel wakes once per step instead of every tick.
const FADE_STEPS: u8 = 64;

/// A crossfade in progress, in [`FADE_STEPS`]ths.
#[derive(Clone, Copy, PartialEq)]
pub struct FadeView {
    pub step: u8,
    /// A Previous started it. A boundary fade reads as forward.
    pub back: bool,
}

impl FadeView {
    pub fn progress(&self) -> f32 {
        self.step as f32 / FADE_STEPS as f32
    }
}

/// Where the A-B cycle stands, in track-relative seconds. Only the
/// half-marked step lives on the player; the loop comes from the engine's
/// snapshot (ADR 16).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum AbState {
    #[default]
    Off,
    ASet(f64),
    Looping(f64, f64),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum AbStep {
    Pending(f64),
    /// A section, or None to end the one playing.
    Send(Option<(f64, f64)>),
    /// B landed on top of A: drop the step rather than send a section the
    /// engine would refuse.
    Nothing,
}

fn ab_step(state: AbState, pos: f64) -> AbStep {
    match state {
        AbState::Looping(..) => AbStep::Send(None),
        AbState::ASet(a) if pos - a >= engine::AB_MIN_SECS => AbStep::Send(Some((a, pos))),
        AbState::ASet(_) => AbStep::Nothing,
        AbState::Off => AbStep::Pending(pos),
    }
}

/// A section named outright, as the socket sets it: ends put in order,
/// anything under the engine's floor or not a real position refused.
fn ab_section(a: f64, b: f64) -> Option<(f64, f64)> {
    if !a.is_finite() || !b.is_finite() || a < 0.0 || b < 0.0 {
        return None;
    }
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    (b - a >= engine::AB_MIN_SECS).then_some((a, b))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum SleepStep {
    Nothing,
    Arm,
    /// Fire without arming: stop-after is already armed, and arming is a
    /// toggle that would turn it off.
    Clear,
}

fn sleep_step(now: Instant, sleep: Option<Instant>, stop_after: bool) -> SleepStep {
    match sleep {
        Some(ends_at) if now >= ends_at => {
            if stop_after {
                SleepStep::Clear
            } else {
                SleepStep::Arm
            }
        }
        _ => SleepStep::Nothing,
    }
}

/// The player's discrete state for the controls and info panels. Leaves out
/// the position clock so a gated panel doesn't wake every tick; anything a
/// panel draws from the player belongs here so the gate sees it change. See
/// [`observe_view`].
#[derive(Clone, PartialEq)]
pub struct PlayerView {
    /// The whole key: a bare path can't tell two cue tracks of one image
    /// apart, so gated observers would miss the boundary.
    pub track: Option<TrackKey>,
    pub duration_secs: Option<f64>,
    pub playing: bool,
    pub active: bool,
    pub ended: bool,
    pub loop_mode: LoopMode,
    pub shuffle: bool,
    pub shuffle_mode: ShuffleMode,
    pub continuation: continuation::Mode,
    /// Zero for off.
    pub crossfade_secs: f32,
    pub stop_after: bool,
    pub ab: AbState,
    /// Whole seconds left. The countdown doesn't repaint anything: only an
    /// opening menu reads it.
    pub sleep_remaining_secs: Option<u64>,
    pub muted: bool,
    pub volume: f32,
    pub error: Option<SharedString>,
    pub fade: Option<FadeView>,
    /// Wakes a gated panel on a station's song turnover, which changes
    /// nothing else here. Zero with no session.
    pub title_rev: u64,
}

/// What output actually ended up doing, for the Audio page. Reports two of
/// ADR 19's three bit-perfect conditions: the running mode, and whether the
/// device rate matches the file's.
#[derive(Clone, PartialEq)]
pub struct OutputStatus {
    pub negotiated: Negotiated,
    /// None before a track has opened.
    pub source_rate: Option<u32>,
    /// The ReplayGain applied to the playing file in dB, None when the
    /// samples reach the ring untouched.
    pub leveling_db: Option<f32>,
}

impl OutputStatus {
    /// The sign is spelled out because a number formatter has no "always"
    /// setting, and a gain without one reads as a level.
    fn signed_db(db: f32) -> String {
        let sign = if db < 0.0 { "-" } else { "+" };
        format!(
            "{sign}{}",
            rox_i18n::format::format_float(f64::from(db.abs()), 1)
        )
    }

    /// The lines under the readout's headline. Expanded gives each fact a
    /// sentence; compact folds them into one comma list so a docked panel
    /// stays two lines tall. `confirm_rate` says whether a matching rate still
    /// gets a line. Nothing here reads the settings, only what was negotiated.
    pub fn lines(&self, expanded: bool, confirm_rate: bool) -> Vec<SharedString> {
        let resampling = self
            .source_rate
            .is_some_and(|source| source != self.negotiated.sample_rate);
        let mut lines: Vec<SharedString> = Vec::new();
        // The toggle stays on after a fallback, so both registers say why.
        if let Some(why) = &self.negotiated.fallback {
            lines.push(rox_i18n::t!(
                "output-fell-back-to-shared",
                why = why.to_string()
            ));
        }
        // Leveling changes the samples (ADR 19), so it outranks the rate.
        if expanded {
            if let Some(db) = self.leveling_db {
                lines.push(rox_i18n::t!(
                    "output-replaygain-levelling",
                    db = Self::signed_db(db)
                ));
            }
            if let Some(source) = self.source_rate {
                if resampling {
                    lines.push(rox_i18n::t!("output-rate-resampled", rate = source));
                } else if confirm_rate {
                    lines.push(rox_i18n::t!("output-rate-native", rate = source));
                }
            }
        } else {
            let mut bits: Vec<SharedString> = Vec::new();
            if let Some(db) = self.leveling_db {
                bits.push(rox_i18n::t!(
                    "output-replaygain-short",
                    db = Self::signed_db(db)
                ));
            }
            if let Some(source) = self.source_rate {
                if resampling {
                    bits.push(rox_i18n::t!("output-rate-resampled-short", rate = source));
                } else if confirm_rate {
                    bits.push(rox_i18n::t!("output-rate-native-short", rate = source));
                }
            }
            if !bits.is_empty() {
                lines.push(
                    bits.iter()
                        .map(SharedString::as_ref)
                        .collect::<Vec<_>>()
                        .join(", ")
                        .into(),
                );
            }
        }
        lines
    }
}

/// A queue snapshot for the close-time persist: every entry's key and
/// explicit flag, the audible cursor, and the position clock in seconds.
pub type QueueStatePersist = (Vec<(TrackKey, bool)>, usize, f64);

/// A queue snapshot for an external editor: every entry's stable id, key,
/// and explicit flag, plus the audible cursor.
pub type PlayOrder = (Vec<(u64, TrackKey, bool)>, usize);

pub struct Player {
    session: Option<Session>,
    error: Option<SharedString>,
    /// Outlives sessions: the audio views hold clones.
    feed: Arc<AudioFeed>,
    /// The source of truth sessions are seeded from.
    settings: Settings,
    /// Drains the tap into the feed while a session runs. Replaced on each
    /// new session.
    pump: Option<Task<()>>,
    /// Debounce generation for the volume persist.
    persist_gen: u64,
    /// Read connection for the insert-time lookup, opened on first play. WAL
    /// keeps it current. None when the library has no database.
    meta_conn: Option<rox_library::rusqlite::Connection>,
    /// Never persisted: an armed stop surviving a restart reads as a broken
    /// player.
    stop_after: bool,
    /// A-B's half-marked step, pinned to its track so a skip away and back
    /// doesn't reuse another song's mark. Past this step the loop lives on
    /// the engine: read it back, never mirror it.
    ab_pending_a: Option<(TrackKey, f64)>,
    /// Session-only like stop-after: a timer surviving a restart would end a
    /// session nobody asked it to.
    sleep: Option<Instant>,
    /// Consecutive skips under Similar, which widen the skip band. Listening
    /// for [`SKIP_SETTLE`] resets it. Session-local.
    similar_skips: u32,
    last_skip: Option<Instant>,
    /// The pool index the similarity ordering was last seeded on. See
    /// [`reseed_at_boundary`] and [`SkipReseed`].
    reseeded_at: Option<usize>,
    skip_reseed: SkipReseed,
    /// Exclusive mode's rate follow (ADR 19). Holds the last negotiated rate,
    /// so a rebuild comes back up on it instead of the device default.
    follow_rate: Option<u32>,
    /// Rates the device already rejected, asked once each so an unsupported
    /// rate doesn't rebuild every tick. A list, or a queue alternating two
    /// unsupported rates would rebuild at every boundary. Cleared on mode or
    /// device change.
    refused_rates: Vec<u32>,
    /// The view playback started in (ADR 17).
    scope: continuation::Scope,
    /// Library ids in pool order, lined up with `Session::queue`. The audible
    /// entry seeds continuation, and the whole vec is what it must not hand
    /// back.
    pool_ids: Vec<Option<i64>>,
    /// Earlier sessions' pools, capped at `HEARD_CAP`. The random and similar
    /// draws start sessions, so a guard off the pool alone resets on the
    /// press most likely to repeat.
    heard: VecDeque<i64>,
    /// A continuation query is out. The pump ticks every 16 ms and a provider
    /// takes tens of ms, so without this one dry-out fires dozens.
    continuing: bool,
    /// An empty batch doesn't move the queue revision, so this stops the pump
    /// re-asking an exhausted provider every tick.
    continued_rev: Option<u64>,
    /// What the continuation toggle turns back on, since Off is a mode.
    last_continuation: continuation::Mode,
    /// Where the audible station's current song started, None when nothing
    /// live is audible. The position counts the whole listen.
    song_start: Option<SongStart>,
}

impl Player {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        // Off isn't a strategy to toggle back to, so arm the default.
        let last_continuation = match settings.session.continuation {
            continuation::Mode::Off => continuation::Mode::default(),
            mode => mode,
        };
        Player {
            session: None,
            error: None,
            feed: Arc::new(AudioFeed::new()),
            settings,
            pump: None,
            persist_gen: 0,
            meta_conn: None,
            stop_after: false,
            ab_pending_a: None,
            sleep: None,
            similar_skips: 0,
            last_skip: None,
            reseeded_at: None,
            skip_reseed: SkipReseed::Idle,
            follow_rate: None,
            refused_rates: Vec::new(),
            scope: continuation::Scope::default(),
            pool_ids: Vec::new(),
            heard: VecDeque::new(),
            continuing: false,
            continued_rev: None,
            last_continuation,
            song_start: None,
        }
    }

    /// Per-key album group (ADR 17), ReplayGain (ADR 19), cue span and
    /// library id. The group comes from the album tags, so tracks of one rip
    /// share it and the crossfade leaves their gapless splice alone.
    fn queue_meta_for(&mut self, keys: &[TrackKey]) -> QueueMeta {
        if self.meta_conn.is_none() {
            let db = rox_core::settings::data_dir().join("library.db");
            self.meta_conn = db.exists().then(|| store::open(&db).ok()).flatten();
        }
        resolve_queue_meta(self.meta_conn.as_ref(), keys)
    }

    /// Call after [`queue_meta_for`](Self::queue_meta_for), which opens the
    /// connection.
    fn locators_for(&self, keys: &[TrackKey]) -> Vec<Locator> {
        resolve_locators(self.meta_conn.as_ref(), keys)
    }

    pub fn feed(&self) -> Arc<AudioFeed> {
        self.feed.clone()
    }

    /// None with no session or before the first track opens.
    pub fn now_playing(&self) -> Option<NowPlaying> {
        let session = self.session.as_ref()?;
        let (track, secs) = session.shared.position(session.device_rate)?;
        let key = session.queue.get(track)?.clone();
        let duration_secs = {
            let tracks = session.shared.tracks.lock().unwrap();
            tracks
                .get(track)
                .and_then(|t| t.as_ref())
                .and_then(|t| t.duration_secs)
        };
        let origin = key.origin();
        let live = session.live.get(track).copied().unwrap_or(false);
        let shift = live.then(|| session.shared.shift(track)).flatten();
        // The pump's observation belongs to the entry it was seen on.
        let observed = self
            .song_start
            .as_ref()
            .filter(|start| live && start.idx == track)
            .map(|start| start.at_secs);
        let song_start_secs = song_start_of(secs, shift, observed);
        let song_from_start = song_heard_from_start(self.song_start.as_ref(), live, track, secs);

        Some(NowPlaying {
            key,
            position_secs: secs,
            duration_secs,
            audible_idx: track,
            live,
            song_start_secs,
            song_from_start,
            origin,
            stream: session.shared.stream_state(track),
            shift,
        })
    }

    /// Where the audible station's songs turned over within the buffer, as
    /// distances back from the live edge, oldest first.
    pub fn live_marks(&self) -> Vec<LiveMark> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let Some((track, _)) = session.shared.position(session.device_rate) else {
            return Vec::new();
        };

        // Guard on the shift's index, so a station pre-rolled behind a file
        // doesn't hand its marks to the file's strip.
        match session.shared.shift(track).is_some() {
            true => session.shared.live_marks(),
            false => Vec::new(),
        }
    }

    /// Where the audible station's connection broke and resumed, oldest
    /// first. A seek back stops at a break, since the bytes either side don't
    /// decode as one run, so strips draw these as walls.
    pub fn live_gaps(&self) -> Vec<LiveGap> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let Some((track, _)) = session.shared.position(session.device_rate) else {
            return Vec::new();
        };

        // Same guard as `live_marks`.
        match session.shared.shift(track).is_some() {
            true => session.shared.live_gaps(),
            false => Vec::new(),
        }
    }

    pub fn seek_to(&self, secs: f64) {
        self.send(Cmd::Seek(secs.max(0.0)));
    }

    /// Play the audible station from `behind_secs` back in its buffer, zero
    /// being the live edge, clamped to the oldest held second. No-op for a
    /// file.
    pub fn seek_live(&self, behind_secs: f64) {
        self.send(Cmd::SeekLive(behind_secs.max(0.0)));
    }

    pub fn go_live(&self) {
        self.seek_live(0.0);
    }

    /// Replace the playing context, starting at its first track. See
    /// [`play_at`](Self::play_at).
    pub fn play(&mut self, queue: Vec<TrackKey>, cx: &mut Context<Self>) {
        self.play_at(queue, 0, cx);
    }

    /// Replace the playing context and start at `start`, keeping the tracks
    /// before it as history for Prev. The explicit queue survives the swap
    /// and resumes right after the starting track (ADR 16).
    pub fn play_at(&mut self, queue: Vec<TrackKey>, start: usize, cx: &mut Context<Self>) {
        if queue.is_empty() {
            return;
        }

        let start = start.min(queue.len() - 1);
        let queued: Vec<TrackKey> = self.queued().iter().map(|e| self.key_for(e)).collect();
        self.start_session(queue, start, None, Vec::new(), false, cx);

        // Spliced after the start rather than seeded, or the fresh context's
        // shuffle would scatter it. The shuffle is queued ahead of this insert,
        // and a fresh session numbers entries by position, so the start is
        // entry `start`.
        if !queued.is_empty() && self.session.is_some() {
            self.splice(Some(start as u64), queued, None, true, false, None, cx);
        }
    }

    /// Launch restore for old settings files that saved a single track,
    /// loaded paused. Newer files go through
    /// [`restore_queue`](Self::restore_queue).
    pub fn restore(&mut self, key: TrackKey, position_secs: f64, cx: &mut Context<Self>) {
        self.start_session(
            vec![key],
            0,
            Some(position_secs.max(0.0)),
            Vec::new(),
            true,
            cx,
        );
    }

    /// The launch restore: the whole play order, paused at `cursor`.
    /// `explicit` runs parallel to `queue`.
    pub fn restore_queue(
        &mut self,
        queue: Vec<TrackKey>,
        explicit: Vec<bool>,
        cursor: usize,
        position_secs: f64,
        cx: &mut Context<Self>,
    ) {
        self.start_session(
            queue,
            cursor,
            Some(position_secs.max(0.0)),
            explicit,
            true,
            cx,
        );
    }

    /// Lets a panel skip re-reading the snapshot when nothing changed.
    pub fn queue_rev(&self) -> Option<u64> {
        Some(self.session.as_ref()?.shared.queue_rev())
    }

    /// Poll this each tick and only read the title when it moves.
    pub fn title_rev(&self) -> Option<u64> {
        Some(self.session.as_ref()?.shared.title_rev())
    }

    /// What the playing station says is on, None for anything else.
    pub fn live_title(&self) -> Option<IcyTitle> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;

        session.shared.live_title(track)
    }

    /// The playing station's own description off its response headers, None
    /// off a station. The only description a typed-in URL gets.
    pub fn station_info(&self) -> Option<StationInfo> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;

        session.shared.station_info(track)
    }

    /// None for a local file or an unopened stream. Changes move the title
    /// revision, so title pollers see them on the same tick.
    pub fn stream_state(&self) -> Option<StreamState> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;

        session.shared.stream_state(track)
    }

    /// Time into the station's current song, None until it names one.
    pub fn song_elapsed(&self) -> Option<f64> {
        let now = self.now_playing()?;

        now.song_start_secs
            .map(|start| song_clock(now.position_secs, Some(start)))
    }

    /// The playing track's tags, with a station's current song laid over the
    /// library row. Every surface naming the playing track reads this, since
    /// a station announces its songs in band.
    pub fn now_meta(&self, library: &Library) -> Option<store::TrackMeta> {
        let key = self.now_playing()?.key;

        self.live_over(library.meta_for_key(&key))
    }

    /// [`now_meta`](Self::now_meta) for a caller already holding the row,
    /// like the track info readout's cache.
    pub fn live_over(&self, row: Option<store::TrackMeta>) -> Option<store::TrackMeta> {
        let Some(title) = self.live_title() else {
            return row;
        };

        Some(crate::radio::live_tags(row, &title))
    }

    /// The explicit up-next queue ahead of the playing track, apart from the
    /// context around it. Empty during plain context playback.
    pub fn queued(&self) -> Vec<QueueEntry> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let snap = session.shared.queue_snapshot();
        let start = self.audible_index(&snap).map(|i| i + 1).unwrap_or(0);
        snap.entries
            .get(start..)
            .unwrap_or(&[])
            .iter()
            .filter(|e| e.explicit)
            .cloned()
            .collect()
    }

    pub fn queued_count(&self) -> usize {
        self.queued().len()
    }

    /// Two cue tracks of one image share a locator, so anything naming or
    /// resolving a queue row comes through here, never `entry.locator`. An
    /// entry the mirror lacks falls back to its locator, a remote one under
    /// an empty source that matches no row.
    pub fn key_for(&self, entry: &QueueEntry) -> TrackKey {
        self.key_at(entry.idx)
            .unwrap_or_else(|| match &entry.locator {
                Locator::Local(path) => TrackKey::from(path.clone()),

                Locator::Remote(remote) => TrackKey {
                    source: rox_library::cue::source_id(""),
                    path: PathBuf::from(&remote.url),
                    sub: 0,
                },
            })
    }

    pub fn key_at(&self, idx: usize) -> Option<TrackKey> {
        self.session.as_ref()?.queue.get(idx).cloned()
    }

    /// The play order for the close-time persist. The cursor comes off the
    /// position clock, so it names the track you hear rather than one opened
    /// for the gapless boundary.
    pub fn queue_state(&self) -> Option<QueueStatePersist> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        if snap.entries.is_empty() {
            return None;
        }
        let cursor = self.audible_index(&snap).unwrap_or(snap.cursor);
        let position_secs = self.now_playing().map(|n| n.position_secs).unwrap_or(0.0);
        let entries = snap
            .entries
            .iter()
            .map(|e| (self.key_for(e), e.explicit))
            .collect();
        Some((entries, cursor, position_secs))
    }

    /// The play order for the control socket, with the stable ids an edit
    /// needs.
    pub fn play_order(&self) -> Option<PlayOrder> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        if snap.entries.is_empty() {
            return None;
        }
        let cursor = self.audible_index(&snap).unwrap_or(snap.cursor);
        let entries = snap
            .entries
            .iter()
            .map(|e| (e.id, self.key_for(e), e.explicit))
            .collect();
        Some((entries, cursor))
    }

    /// Queue at the front of the explicit queue. With nothing loaded this
    /// starts them.
    pub fn play_next(&mut self, keys: Vec<TrackKey>, cx: &mut Context<Self>) {
        let after = self.playing_after();
        self.insert(after, keys, false, cx);
    }

    /// Splice after the playing track and jump to the first, keeping the
    /// queue behind them. A batch of stations clears the queue, and leaving a
    /// station takes it out. Play Next and Add to Queue leave stations alone.
    pub fn play_now(&mut self, keys: Vec<TrackKey>, cx: &mut Context<Self>) {
        // `insert` bails on its own, but the station rules below would not.
        if keys.is_empty() {
            return;
        }

        // Asked before the splice: after the jump, the station you left is
        // just another entry.
        let leaving = self.playing_station();

        if replaces_queue(&keys) {
            self.clear_queue();
        }

        let after = self.playing_after();
        self.insert(after, keys, true, cx);

        if let Some(id) = leaving {
            self.drop_when_left(id, cx);
        }
    }

    /// Append after anything already queued, before the context resumes.
    pub fn enqueue(&mut self, keys: Vec<TrackKey>, cx: &mut Context<Self>) {
        let after = self.enqueue_after();
        self.insert(after, keys, false, cx);
    }

    /// Matched by pool index off the position clock, so Play Next goes after
    /// what you hear rather than a track opened for the gapless boundary, and
    /// a file queued twice resolves to the right occurrence.
    fn audible_index(&self, snap: &QueueSnapshot) -> Option<usize> {
        let now = self.now_playing()?;
        snap.entries.iter().position(|e| e.idx == now.audible_idx)
    }

    /// The newest track the engine adopted, which is where a skip went even
    /// while the clock still reads the old track. Under a crossfade the clock
    /// flips at the midpoint (ADR 19), so this avoids waiting the fade out.
    fn adopted_index(&self, snap: &QueueSnapshot) -> Option<usize> {
        let session = self.session.as_ref()?;
        let adopted = session.shared.segments.lock().unwrap().last()?.track;
        snap.entries.iter().position(|e| e.idx == adopted)
    }

    /// Falls back to the published cursor before audio starts.
    fn playing_after(&self) -> Option<u64> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        match self.audible_index(&snap) {
            Some(i) => snap.entries.get(i).map(|e| e.id),
            None => snap.entries.get(snap.cursor).map(|e| e.id),
        }
    }

    fn playing_station(&self) -> Option<u64> {
        let now = self.now_playing()?;
        if now.origin != Origin::Radio {
            return None;
        }

        self.playing_after()
    }

    /// The last explicit entry after the playing track, or the playing track.
    fn enqueue_after(&self) -> Option<u64> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        let cursor = self.audible_index(&snap).unwrap_or(snap.cursor);
        let mut after = snap.entries.get(cursor).map(|e| e.id)?;
        let mut i = cursor + 1;
        while let Some(entry) = snap.entries.get(i) {
            if !entry.explicit {
                break;
            }
            after = entry.id;
            i += 1;
        }
        Some(after)
    }

    /// Splice keys in as explicit entries, or start playback with no session.
    fn insert(
        &mut self,
        after: Option<u64>,
        keys: Vec<TrackKey>,
        and_play: bool,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty() {
            return;
        }
        if self.session.is_none() {
            self.play(keys, cx);
            return;
        }
        self.splice(after, keys, None, true, and_play, None, cx);
    }

    /// Play `key` now from `secs` in, for bookmarks. The offset rides the
    /// insert command, so the track's head is never heard.
    pub fn play_now_at(&mut self, key: TrackKey, secs: f64, cx: &mut Context<Self>) {
        if self.session.is_none() {
            self.play(vec![key], cx);
            self.seek_to(secs);
            return;
        }
        let after = self.playing_after();
        self.splice(after, vec![key], None, true, true, Some(secs.max(0.0)), cx);
    }

    /// The insert hand-queued keys and continuation batches share. `groups`
    /// overrides the library's album grouping where it's Some.
    #[allow(clippy::too_many_arguments)]
    fn splice(
        &mut self,
        after: Option<u64>,
        keys: Vec<TrackKey>,
        groups: Option<Vec<Option<u64>>>,
        explicit: bool,
        and_play: bool,
        start_secs: Option<f64>,
        cx: &mut Context<Self>,
    ) {
        // Bail before anything grows: `pool_ids` runs parallel to the pool,
        // and a half-applied splice would slide them apart for good.
        if self.session.is_none() {
            return;
        }
        let meta = self.queue_meta_for(&keys);
        let groups = match groups {
            Some(picked) => picked
                .into_iter()
                .zip(meta.groups)
                .map(|(picked, library)| picked.or(library))
                .collect(),
            None => meta.groups,
        };
        self.pool_ids.extend(meta.ids);
        let locators = self.locators_for(&keys);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.queue.extend(keys);
        session.gains.extend(meta.gains.iter().copied());
        session.live.extend(live_flags(&locators));
        // A play now is the one insert someone is waiting on.
        let cmd = Cmd::Insert {
            after,
            locators,
            groups,
            gains: meta.gains,
            spans: meta.spans,
            explicit,
            and_play,
            start_secs,
        };
        if answers_now(&cmd) {
            session.shared.interrupt();
        }

        let _ = session.tx.send(cmd);
        cx.notify();
    }

    /// The engine refuses the playing entry, so callers needn't guard it.
    pub fn remove_from_queue(&self, id: u64) {
        self.send(Cmd::Remove { id });
    }

    /// One command and one queue publish for the whole batch.
    pub fn remove_many_from_queue(&self, ids: Vec<u64>) {
        if ids.is_empty() {
            return;
        }
        self.send(Cmd::RemoveMany { ids });
    }

    /// Drop every up-next explicit entry. The playing track and context stay.
    pub fn clear_queue(&self) {
        let ids: Vec<u64> = self.queued().iter().map(|e| e.id).collect();
        self.remove_many_from_queue(ids);
    }

    /// Drop `id` once the engine has moved off it. It can't be a command
    /// behind the insert: the engine drains its channel before acting, so a
    /// remove sent after the jump arrives while the station is still audible,
    /// and the engine refuses to remove the audible entry. If the jump never
    /// lands, the station keeps its place.
    fn drop_when_left(&self, id: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            for _ in 0..QUEUE_WAIT_TRIES {
                cx.background_executor().timer(QUEUE_WAIT_STEP).await;

                let Ok(playing) = this.update(cx, |this, _| this.playing_entry()) else {
                    return;
                };
                if playing == Some(id) {
                    continue;
                }

                this.update(cx, |this, _| this.remove_from_queue(id)).ok();
                return;
            }
        })
        .detach();
    }

    /// Moves the entry to the front of the queue before jumping: a bare jump
    /// would strand everything above it behind the cursor as history.
    pub fn play_queued(&self, id: u64) {
        if let Some(after) = self.playing_entry().filter(|&playing| playing != id) {
            self.move_in_queue(id, Some(after));
        }
        self.jump_to(id);
    }

    /// Move a queued entry to just after `after`, or to the front when None.
    pub fn move_in_queue(&self, id: u64, after: Option<u64>) {
        self.send(Cmd::Move { id, after });
    }

    pub fn jump_to(&self, id: u64) {
        self.send(Cmd::Jump { id });
    }

    pub fn playing_entry(&self) -> Option<u64> {
        self.playing_after()
    }

    fn start_session(
        &mut self,
        queue: Vec<TrackKey>,
        start: usize,
        paused_at: Option<f64>,
        explicit: Vec<bool>,
        preserve_order: bool,
        cx: &mut Context<Self>,
    ) {
        if queue.is_empty() {
            return;
        }
        let start = start.min(queue.len() - 1);
        // A restore re-derives groups, gains and spans here, so none of them
        // are persisted.
        let meta = self.queue_meta_for(&queue);
        let (groups, gains, spans) = (meta.groups, meta.gains, meta.spans);
        // A paused start renders nothing, so prime the feed with a frame at
        // the load position. A cue track's clock runs from its span's start.
        let locators = self.locators_for(&queue);
        let prime = paused_at.map(|secs| {
            let offset = spans
                .get(start)
                .copied()
                .flatten()
                .map(|span| span.start_ms as f64 / 1000.0)
                .unwrap_or(0.0);
            (locators[start].clone(), offset + secs.max(0.0))
        });
        // The outgoing pool joins the heard ring before it's replaced. On a
        // rebuild the same ids come straight back, so the fold is a no-op.
        remember_held(&mut self.heard, self.pool_ids.iter().flatten().copied());
        self.pool_ids = meta.ids;
        self.scope = continuation::Scope::default();
        self.continuing = false;
        self.continued_rev = None;
        // The marker's indices belong to the outgoing pool. The start below
        // orders its own tail, so the first tick adopts it without ranking.
        self.reseeded_at = None;
        self.skip_reseed = SkipReseed::Idle;
        self.song_start = None;
        self.session = None;
        let shuffle = if preserve_order {
            None
        } else {
            Some(self.settings.session.shuffle)
        };
        match Session::start(
            StartQueue {
                locators,
                start,
                explicit,
                groups,
                gains,
                spans,
                // Passed in, not sent: the engine's first open happens before
                // it reads the channel.
                live_buffer_secs: clamp_live_buffer_secs(self.settings.live_buffer_secs),
            },
            queue,
            self.effective_volume(),
            self.settings.session.loop_mode(),
            shuffle,
            self.stop_after,
            paused_at,
            (self.settings.crossfade_secs, self.settings.crossfade_albums),
            self.settings.replay_gain.rule(),
            self.output_request(),
        ) {
            Ok(session) => {
                self.feed.set_sample_rate(session.device_rate);
                let rate = session.device_rate;
                // Ask the next open for the rate this one settled on, so a
                // rebuild doesn't drop to the device default and follow back
                // with a second gap. Exclusive only: a shared mixer rate would
                // make the switch into exclusive open at the wrong rate first.
                self.follow_rate = (session.negotiated.mode == Mode::Exclusive)
                    .then_some(session.negotiated.sample_rate);
                self.session = Some(session);
                self.error = None;
                self.start_pump(cx);
                if let Some((locator, secs)) = prime {
                    self.prime_feed(locator, secs, rate, cx);
                }
                // Under Similar the engine's shuffle flag leaves pool order,
                // so a fresh context orders its tail here.
                if !preserve_order
                    && self.settings.session.shuffle
                    && self.shuffle_mode() == ShuffleMode::Similar
                {
                    self.order_tail_by_similarity(1, None, false, cx);
                }
            }
            Err(e) => self.error = Some(format!("audio output: {e}").into()),
        }
        cx.notify();
    }

    /// The transport's eject: drop the session so every view goes idle.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.pump = None;
        self.error = None;
        self.ab_pending_a = None;
        // Left running, the timer would arm a stop over the next play.
        self.sleep = None;
        cx.notify();
    }

    /// Drain the tap on a timer instead of a render pass. The timer ticks all
    /// session so the audio views keep feeding and a resume (flipped on the
    /// audio thread) is noticed. It only notifies while audio moves, on a
    /// play-state edge, on a paused seek, or on a new queue revision. Queue
    /// commands are fire-and-forget, so the revision bumps after the
    /// enqueue's own notify, and without that wake a paused queue view lags
    /// one edit. A settled pause notifies nobody.
    fn start_pump(&mut self, cx: &mut Context<Self>) {
        let mut was_playing = self.is_playing();
        let mut seen_rev = self.queue_rev();
        let mut seen_pos = self.paused_key();
        self.pump = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PUMP_INTERVAL).await;
                let alive = this.update(cx, |this, cx| {
                    if this.session.is_none() {
                        return false;
                    }
                    // The output stream died. A swap keeps this pump over the
                    // same engine; a rebuild or failure ends it. Two pumps on
                    // one session would double-drain the tap.
                    if this
                        .session
                        .as_ref()
                        .is_some_and(|s| s.shared.device_lost())
                    {
                        return this.reopen_device(cx);
                    }
                    // A rate follow rebuilds, which brings its own pump.
                    if this.follow_source_rate(cx) {
                        return false;
                    }
                    this.drain_tap();
                    // Lets the signal hub see a song change without holding
                    // the player.
                    this.feed.set_track(this.playing_entry());
                    this.tick_sleep(cx);
                    // Continuation (ADR 17). A no-op on nearly every tick.
                    this.continue_if_dry(cx);
                    // The pump is the only thing watching the audible track
                    // often enough to see boundaries, song turnovers and
                    // refused entries go past.
                    this.reseed_on_boundary(cx);
                    this.track_song_start();
                    this.take_refusal(cx);
                    let playing = this.is_playing();
                    let rev = this.queue_rev();
                    // A paused seek moves the clock without moving audio or the
                    // revision, so compare the resolved position while paused.
                    let pos = if playing { None } else { this.paused_key() };
                    if playing || playing != was_playing || rev != seen_rev || pos != seen_pos {
                        cx.notify();
                    }
                    was_playing = playing;
                    seen_rev = rev;
                    seen_pos = pos;
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Record the station clock when the audible station's ICY title turns
    /// over, so the transport can show the song rather than the listen.
    ///
    /// Gated on the title revision so the common tick skips the title lock.
    /// The revision is global, so the title compare filters out another
    /// entry's bump. [`Shared::publish_title`] drops a pause rejoin's repeated
    /// title, so a resumed listener keeps their clock.
    fn track_song_start(&mut self) {
        let Some(session) = self.session.as_ref() else {
            self.song_start = None;
            return;
        };

        let audible = session
            .shared
            .position(session.device_rate)
            .filter(|(track, _)| session.live.get(*track).copied().unwrap_or(false));
        let Some((idx, secs)) = audible else {
            self.song_start = None;
            return;
        };

        let rev = session.shared.title_rev();
        if self
            .song_start
            .as_ref()
            .is_some_and(|start| start.idx == idx && start.rev == rev)
        {
            return;
        }

        let Some(title) = session.shared.live_title(idx) else {
            self.song_start = None;
            return;
        };

        if starts_new_song(self.song_start.as_ref(), idx, &title) {
            // A standing record means we watched this change. An empty one
            // means the entry just became audible, mid-song.
            let from_start = self.song_start.is_some();
            self.song_start = Some(SongStart {
                idx,
                rev,
                title,
                at_secs: secs,
                from_start,
            });
        } else if let Some(start) = self.song_start.as_mut() {
            // Another entry moved the revision. Take it so the next tick is
            // cheap again.
            start.rev = rev;
        }
    }

    /// The continuation trigger (ADR 17). Keep it out of the engine: its
    /// decode cursor runs a ring ahead of the speakers, and firing there
    /// would put the audio thread inside the library stores.
    fn continue_if_dry(&mut self, cx: &mut Context<Self>) {
        self.request_continuation(false, cx);
    }

    /// Ask the active provider for a batch and append it. `force` skips the
    /// dry-out test and revision guard, for engaging Similar. It still waits
    /// on `continuing`, or a press on a dry-out tick splices two batches.
    ///
    /// A forced draw stamps the revision on the way in and clears it on the
    /// way out: its batch lands on a full queue, so a leftover stamp would
    /// stop the queue asking for more when it really runs down.
    fn request_continuation(&mut self, force: bool, cx: &mut Context<Self>) {
        let mode = self.settings.session.continuation;
        if mode == continuation::Mode::Off || self.continuing {
            return;
        }
        if !continuation_wanted(self.is_playing(), self.stop_after) {
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let rev = session.shared.queue_rev();
        if !force && self.continued_rev == Some(rev) {
            return;
        }
        // The audible cursor, not the decode cursor, which has run ahead for
        // the gapless boundary.
        let audible = session.shared.position(session.device_rate).map(|(t, _)| t);
        if !force && !self.running_dry() {
            return;
        }
        let seed = continuation::Seed {
            track: audible.and_then(|idx| self.pool_ids.get(idx).copied().flatten()),
            scope: self.scope.clone(),
            recent: self.pool_ids.iter().flatten().copied().collect(),
            count: continuation::BATCH,
            // Taken on this tick so the refill scores the model the queue is
            // sorted by.
            model: crate::acoustic::acoustic_source().id().to_string(),
        };
        // The radio draw follows the shuffle order. Read on the same tick
        // that checked the mode.
        let order = self.queue_order();
        self.continuing = true;
        self.continued_rev = Some(rev);
        let db_path = rox_core::settings::data_dir().join("library.db");
        cx.spawn(async move |this, cx| {
            // Blocking store queries go on the background executor (ADR 14),
            // on their own connection.
            let picks = cx
                .background_executor()
                .spawn(async move {
                    let provider = continuation::provider(mode, order)?;
                    let conn = store::open(&db_path).ok()?;
                    Some(provider.next(&conn, &seed))
                })
                .await
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                this.continuing = false;
                this.land_continuation(mode, order, force, picks, cx);
                if force {
                    this.continued_rev = None;
                }
                // Similar was engaged while this query was out: its forced draw
                // was turned away by `continuing`, and the landing just dropped
                // this batch as the wrong order. Ask again for it.
                if order != continuation::Order::Similar && this.similar_order() {
                    this.request_continuation(true, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Append a provider's batch as context entries. Never start a successor
    /// session instead: the gapless boundary (ADR 3) holds because these are
    /// more entries in the append-only pool, and a new session tears the
    /// stream down (ADR 16).
    fn land_continuation(
        &mut self,
        mode: continuation::Mode,
        order: continuation::Order,
        force: bool,
        picks: Vec<Pick>,
        cx: &mut Context<Self>,
    ) {
        // Drop a stale answer: the mode, order, or session changed while the
        // query ran.
        if mode != self.settings.session.continuation
            || order != self.queue_order()
            || self.continued_rev.is_none()
            || self.session.is_none()
        {
            return;
        }
        // Re-check the trigger: the query took long enough to pause or queue
        // an album in. A forced batch skips the dry-out, its queue is full on
        // purpose.
        if !continuation_wanted(self.is_playing(), self.stop_after)
            || (!force && !self.running_dry())
        {
            return;
        }
        if picks.is_empty() {
            // Playback ends here. Continuation never falls back to another
            // provider.
            log::info!("continuation: {} had nothing left", mode.label());
            return;
        }
        let mut resolved = self.resolve_picks(&picks);
        if resolved.is_empty() {
            return;
        }
        // Shuffle the batch itself, never the whole tail: that would scramble
        // a hand-built queue every batch.
        if self.settings.session.shuffle && self.shuffle_mode() == ShuffleMode::Random {
            shuffle_slice(&mut resolved);
        }
        let (keys, groups): (Vec<TrackKey>, Vec<Option<u64>>) = resolved.into_iter().unzip();
        // Context, not queue: the queue widgets stay quiet about it, but it's
        // in the timeline and removable.
        self.splice(None, keys, Some(groups), false, false, None, cx);
        if self.similar_order() {
            self.order_tail_by_similarity(1, None, false, cx);
        }
    }

    /// Re-rank the tail against what's audible, once per boundary. Reads the
    /// audible pool index (a few atomics) rather than a queue snapshot, which
    /// clones a path per entry every tick.
    fn reseed_on_boundary(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some((audible, _)) = session.shared.position(session.device_rate) else {
            return;
        };
        let last = self.reseeded_at.replace(audible);
        if last == Some(audible) {
            return;
        }
        // The skip's re-seed is spent here either way. A ranking still in
        // flight keeps its claim and releases it when it gives up.
        let (claim, skipped) = self.skip_reseed.spend();
        self.skip_reseed = claim;
        if reseed_at_boundary(self.similar_order(), skipped, audible, last) {
            self.order_tail_by_similarity(1, None, false, cx);
        }
    }

    /// Counted from the audible cursor (ADR 17). The published cursor stands
    /// in before the first frame, so a session that starts short fires on its
    /// first tick.
    fn running_dry(&self) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        let audible = session.shared.position(session.device_rate).map(|(t, _)| t);
        let Some((_, upcoming)) = session.shared.upcoming_from(audible) else {
            return false;
        };
        queue_running_dry(upcoming, self.settings.session.loop_mode())
    }

    fn similar_order(&self) -> bool {
        self.settings.session.shuffle && self.shuffle_mode() == ShuffleMode::Similar
    }

    fn queue_order(&self) -> continuation::Order {
        if !self.settings.session.shuffle {
            continuation::Order::Browse
        } else if self.shuffle_mode() == ShuffleMode::Similar {
            continuation::Order::Similar
        } else {
            continuation::Order::Random
        }
    }

    /// An unresolvable pick drops out with its group, so the two never slide
    /// apart.
    fn resolve_picks(&mut self, picks: &[Pick]) -> Vec<(TrackKey, Option<u64>)> {
        if self.meta_conn.is_none() {
            let db = rox_core::settings::data_dir().join("library.db");
            self.meta_conn = db.exists().then(|| store::open(&db).ok()).flatten();
        }
        let Some(conn) = self.meta_conn.as_ref() else {
            return Vec::new();
        };
        picks
            .iter()
            .filter_map(|pick| {
                let key = store::key_for_id(conn, pick.id).ok().flatten()?;
                Some((key, pick.group))
            })
            .collect()
    }

    pub fn continuation_mode(&self) -> continuation::Mode {
        self.settings.session.continuation
    }

    /// Takes effect at the next dry-out. Clears the guard so a switch on an
    /// already short queue fires on the next tick.
    pub fn set_continuation_mode(&mut self, mode: continuation::Mode, cx: &mut Context<Self>) {
        if self.settings.session.continuation == mode {
            return;
        }
        self.settings.session.continuation = mode;
        if mode != continuation::Mode::Off {
            self.last_continuation = mode;
        }
        self.continued_rev = None;
        Settings::update(move |s| s.session.continuation = mode);
        cx.notify();
    }

    /// Off, or back on in the last strategy used.
    pub fn toggle_continuation(&mut self, cx: &mut Context<Self>) {
        let mode = match self.settings.session.continuation {
            continuation::Mode::Off => self.last_continuation,
            _ => continuation::Mode::Off,
        };
        self.set_continuation_mode(mode, cx);
    }

    /// Call after the play that seeded the session, since starting one
    /// clears the scope (ADR 17).
    pub fn set_scope(&mut self, scope: continuation::Scope) {
        self.scope = scope;
    }

    /// The pool plus the heard ring: the pool forgets the past when a press
    /// replaces it, and the ring lacks the present until one does.
    fn draw_seen(&self) -> HashSet<i64> {
        self.pool_ids
            .iter()
            .flatten()
            .copied()
            .chain(self.heard.iter().copied())
            .collect()
    }

    /// Play a random unheard track from the current scope, with the run
    /// around it as context. The scope is put back after the start so a
    /// second press stays in the same list.
    pub fn play_random(&mut self, library: &Entity<Library>, cx: &mut Context<Self>) {
        let scope = self.scope.clone();
        let seen = self.draw_seen();
        let drawn = {
            let library = library.read(cx);
            let all: &[i64] = library
                .projection()
                .map(|p| p.db_id.as_slice())
                .unwrap_or_default();
            // A stale scope falls back to the whole library.
            draw_run(library, random_pool(&scope, all), &seen)
                .or_else(|| draw_run(library, all, &seen))
        };
        let Some((keys, start)) = drawn else { return };
        self.play_at(keys, start, cx);
        // After the play, never before: starting a session clears the scope.
        self.scope = scope;
    }

    /// Play one track that sounds like `seed`, drawn library-wide off the
    /// acoustic vectors (see [`embeddings::ranked`]). Runs on the background
    /// executor: the first ask after the analysis pass writes rereads the
    /// table, a few hundred milliseconds on a fifty-thousand-track library.
    /// One track and no scope: what follows is continuation's business
    /// (ADR 17).
    pub fn play_similar_to(
        &mut self,
        seed: i64,
        library: &Entity<Library>,
        cx: &mut Context<Self>,
    ) {
        let db_path = rox_core::settings::data_dir().join("library.db");
        let model = crate::acoustic::acoustic_source().id().to_string();
        // Taken before the start this press leads to replaces the pool.
        let seen = self.draw_seen();
        let library = library.clone();
        cx.spawn(async move |this, cx| {
            let drawn = cx
                .background_executor()
                .spawn(async move {
                    let conn = store::open(&db_path).ok()?;
                    let near =
                        embeddings::nearest_ranked(&conn, seed, &model, SIMILAR_POOL).ok()?;
                    let near = one_per_song(&conn, seed, &seen, &near);
                    let band = fresh_band(&near, &seen);
                    if band.is_empty() {
                        return None;
                    }
                    Some(band[random_index(band.len())])
                })
                .await;
            let Some(id) = drawn else {
                log::info!("play similar: nothing analyzed to draw from");
                return;
            };
            this.update(cx, |this, cx| {
                let Ok(keys) = library.read(cx).keys_for(&[id]) else {
                    return;
                };
                this.play(keys, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Does nothing when the playing file isn't in the library.
    pub fn play_similar(&mut self, library: &Entity<Library>, cx: &mut Context<Self>) {
        let Some(seed) = self
            .now_playing()
            .and_then(|now| library.read(cx).id_for_key(&now.key))
        else {
            return;
        };
        self.play_similar_to(seed, library, cx);
    }

    /// Rebuild the session against the current output settings, at the spot
    /// it's playing: everything denominated in the device rate goes with the
    /// stream. False when there's nothing to rebuild from.
    fn rebuild_session(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        let was_playing = session.shared.playing.load(Ordering::Relaxed);
        let Some((entries, cursor, position_secs)) = self.queue_state() else {
            return false;
        };
        let (keys, explicit): (Vec<TrackKey>, Vec<bool>) = entries.into_iter().unzip();
        // Same music on a new stream, so keep the scope the start clears.
        let scope = self.scope.clone();
        self.start_session(keys, cursor, Some(position_secs), explicit, true, cx);
        self.scope = scope;
        // A restore comes up paused.
        if was_playing && self.session.is_some() {
            self.send(Cmd::TogglePause);
        }
        true
    }

    /// Get back to audio after the device dropped out: swap the output under
    /// the running engine, fall back to a rebuild, or stop with an error so
    /// the UI doesn't show a frozen "playing". Returns whether the calling
    /// pump carries on. Only the swap keeps it, since two pumps on one
    /// session would double-drain the tap.
    fn reopen_device(&mut self, cx: &mut Context<Self>) -> bool {
        if self.session.is_none() {
            return false;
        }
        if self.swap_output(cx) {
            return true;
        }
        if self.rebuild_session(cx) {
            return false;
        }

        self.stop(cx);
        self.error = Some("audio output: device lost".into());
        cx.notify();

        false
    }

    /// Reopen the output and hand the running engine the new ring. Costs only
    /// the gap: a station keeps its connection, tape and capture, where a
    /// rebuild would re-dial it and drop the timeshift buffer. False with no
    /// session, no device, or a device back at another rate, which needs the
    /// rebuild.
    fn swap_output(&mut self, cx: &mut Context<Self>) -> bool {
        let request = self.output_request();
        let Some(session) = self.session.as_mut() else {
            return false;
        };

        // Clear the flag before the open, so a fault during the open gets its
        // own pass instead of being cleared unseen.
        session.shared.device_lost.store(false, Ordering::Release);
        // Release the dead stream first so the backend has the device back.
        drop(session.stream.take());

        let out = match output::open(&request, &session.shared) {
            Ok(out) => out,
            Err(e) => {
                log::warn!("audio output: reopen after device loss failed: {e}");

                return false;
            }
        };
        if out.sample_rate != session.device_rate {
            log::info!(
                "audio output came back at {} Hz, was {}; rebuilding the session",
                out.sample_rate,
                session.device_rate
            );

            return false;
        }

        log::info!(
            "audio output reopened on {}, keeping the engine",
            out.negotiated.device
        );
        session.negotiated = out.negotiated;
        session.tap = out.tap;
        session.stream = Some(out.stream);
        // Sent after the fields above so nothing reads a half-moved session.
        let _ = session.tx.send(Cmd::SwapOutput(out.producer));
        self.error = None;
        cx.notify();

        true
    }

    /// Exclusive mode follows the file's rate (ADR 19). A file's rate is only
    /// known once it's open, so the first one is followed a beat late. Skips
    /// rates the device already refused. Returns whether it rebuilt.
    fn follow_source_rate(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        if session.negotiated.mode != Mode::Exclusive {
            return false;
        }
        if self.settings.output.rate.is_some() {
            return false;
        }
        let Some(rate) = self.source_rate() else {
            return false;
        };
        if rate == session.device_rate || self.refused_rates.contains(&rate) {
            return false;
        }
        self.follow_rate = Some(rate);
        if !self.rebuild_session(cx) {
            return false;
        }
        // The card came back at another rate, so it lacks this one.
        if self.negotiated().is_some_and(|n| n.sample_rate != rate) {
            self.refused_rates.push(rate);
        }
        true
    }

    fn source_rate(&self) -> Option<u32> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;
        let tracks = session.shared.tracks.lock().unwrap();
        tracks
            .get(track)
            .and_then(|t| t.as_ref())
            .map(|t| t.sample_rate)
    }

    fn negotiated(&self) -> Option<&Negotiated> {
        self.session.as_ref().map(|s| &s.negotiated)
    }

    /// The two modes' device picks are stored apart because their ids don't
    /// cross.
    fn output_request(&self) -> Request {
        let output = &self.settings.output;
        let exclusive = output.exclusive;
        Request {
            mode: if exclusive {
                Mode::Exclusive
            } else {
                Mode::Shared
            },
            device: if exclusive {
                output.exclusive_device.clone()
            } else {
                output.device.clone()
            },
            // A pinned rate wins, so the follow can't fight it.
            rate: output.rate.or(self.follow_rate),
            format: output.format.clone(),
            period_ms: output.period_ms,
        }
    }

    /// None while no stream is open.
    pub fn output_status(&self) -> Option<OutputStatus> {
        Some(OutputStatus {
            negotiated: self.negotiated()?.clone(),
            source_rate: self.source_rate(),
            leveling_db: self.leveling_db(),
        })
    }

    /// Raise the device-lost flag as the backend's error callback would. The
    /// only test surface for the recovery path (ADR 22). False with no
    /// session.
    pub fn fault_output(&self) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        session.shared.device_lost.store(true, Ordering::Release);

        true
    }

    /// The engine's own rule over the same tags, so this reports what's
    /// applied rather than what's switched on. Unity is None.
    fn leveling_db(&self) -> Option<f32> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;
        let rg = session.gains.get(track).copied().unwrap_or_default();
        let factor = self.settings.replay_gain.rule().factor(rg);
        (factor != 1.0).then(|| 20.0 * factor.log10())
    }

    /// The EQ cascade's gain at `hz`, at the device rate the filters run at,
    /// or 48 kHz while nothing plays.
    pub fn eq_response_db(&self, hz: f32) -> f32 {
        let rate = self
            .session
            .as_ref()
            .map(|session| session.device_rate)
            .unwrap_or(48_000);
        eq_params().response_db(hz, rate)
    }

    /// Zero is off.
    pub fn crossfade_secs(&self) -> f32 {
        self.settings.crossfade_secs
    }

    /// Whether the fade takes album-contiguous boundaries too.
    pub fn crossfade_albums(&self) -> bool {
        self.settings.crossfade_albums
    }

    /// Live on the running session: the next boundary uses it.
    pub fn set_crossfade_secs(&mut self, secs: f32, cx: &mut Context<Self>) {
        // The engine's own clamp, so the persisted and audible values agree.
        let secs = secs.clamp(0.0, engine::CROSSFADE_MAX_SECS);
        if self.settings.crossfade_secs == secs {
            return;
        }
        self.settings.crossfade_secs = secs;
        // Remembered here, where the slider and the transport menu both pass,
        // so they agree on what "back on" means.
        if secs > 0.0 {
            self.settings.crossfade_restore_secs = secs;
        }
        self.send_crossfade();
        // Debounced: dragging the slider calls this per tick.
        self.persist_playback_soon(cx);
        cx.notify();
    }

    /// Off, or back on at the last length.
    pub fn toggle_crossfade(&mut self, cx: &mut Context<Self>) {
        let secs = if self.settings.crossfade_secs > 0.0 {
            0.0
        } else {
            self.crossfade_restore_secs()
        };
        self.set_crossfade_secs(secs, cx);
    }

    /// Never zero, so the toggle can't turn the fade on at no length.
    pub fn crossfade_restore_secs(&self) -> f32 {
        let secs = self.settings.crossfade_restore_secs;
        if secs > 0.0 {
            secs
        } else {
            rox_core::settings::DEFAULT_CROSSFADE_SECS
        }
    }

    /// Fade inside an album as well, or leave a record's own splices alone.
    pub fn set_crossfade_albums(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.settings.crossfade_albums == on {
            return;
        }
        self.settings.crossfade_albums = on;
        self.send_crossfade();
        Settings::update(move |s| s.crossfade_albums = on);
        cx.notify();
    }

    /// How much of a live stream is kept behind the playhead, in seconds.
    pub fn live_buffer_secs(&self) -> u32 {
        clamp_live_buffer_secs(self.settings.live_buffer_secs)
    }

    /// The engine re-caps the open tape in place. The write is debounced:
    /// `Settings::update` rewrites five files, far too much per drag tick.
    pub fn set_live_buffer_secs(&mut self, secs: u32, cx: &mut Context<Self>) {
        let secs = clamp_live_buffer_secs(secs);
        if self.settings.live_buffer_secs == secs {
            return;
        }
        self.settings.live_buffer_secs = secs;
        self.send(Cmd::SetLiveBuffer(secs));
        crate::capture::follow_live_buffer(secs);
        self.persist_playback_soon(cx);
        cx.notify();
    }

    /// Bytes a second the playing station tapes, as the tape measures it, for
    /// showing the buffer setting's memory cost. None off a live stream.
    pub fn live_bytes_per_sec(&self) -> Option<f64> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;

        Some(session.shared.shift(track)?.bytes_per_sec)
    }

    pub fn replay_gain(&self) -> ReplayGainSettings {
        self.settings.replay_gain
    }

    /// Live: the engine relevels every source it holds, the playing one too.
    pub fn set_replay_gain_mode(&mut self, mode: GainModeSetting, cx: &mut Context<Self>) {
        if self.settings.replay_gain.mode == mode {
            return;
        }
        self.settings.replay_gain.mode = mode;
        self.send_gain_rule();
        Settings::update(move |s| s.replay_gain.mode = mode);
        // The library's Gain column renders from this static.
        rox_core::settings::set_gain_mode(mode, cx);
        cx.notify();
    }

    /// The offset on every tagged gain, in dB.
    pub fn set_replay_gain_preamp(&mut self, db: f32, cx: &mut Context<Self>) {
        if self.settings.replay_gain.preamp_db == db {
            return;
        }
        self.settings.replay_gain.preamp_db = db;
        self.send_gain_rule();
        self.persist_playback_soon(cx);
        cx.notify();
    }

    /// What an untagged file plays at, in dB.
    pub fn set_replay_gain_fallback(&mut self, db: f32, cx: &mut Context<Self>) {
        if self.settings.replay_gain.fallback_db == db {
            return;
        }
        self.settings.replay_gain.fallback_db = db;
        self.send_gain_rule();
        self.persist_playback_soon(cx);
        cx.notify();
    }

    /// Where the measurement pass saves. Goes through the player though the
    /// engine never sees it: the player flushes `replay_gain` whole, so a
    /// value written around it gets clobbered.
    pub fn set_replay_gain_save(&mut self, save: ReplayGainSave, cx: &mut Context<Self>) {
        if self.settings.replay_gain.save == save {
            return;
        }
        self.settings.replay_gain.save = save;
        Settings::update(move |s| s.replay_gain.save = save);
        cx.notify();
    }

    /// Whether the measurement pass follows the watcher. Through the player
    /// for the same reason as the save destination.
    pub fn set_replay_gain_auto(&mut self, auto: bool, cx: &mut Context<Self>) {
        if self.settings.replay_gain.auto == auto {
            return;
        }
        self.settings.replay_gain.auto = auto;
        Settings::update(move |s| s.replay_gain.auto = auto);
        cx.notify();
    }

    fn send_gain_rule(&self) {
        self.send(Cmd::SetGainRule(self.settings.replay_gain.rule()));
    }

    fn send_crossfade(&self) {
        self.send(Cmd::SetCrossfade {
            secs: self.settings.crossfade_secs,
            albums: self.settings.crossfade_albums,
        });
    }

    /// Off the output clock, so it shows while the overlap is audible.
    pub fn crossfade(&self) -> Option<FadeView> {
        let (progress, back) = self.session.as_ref()?.shared.crossfade()?;
        Some(FadeView {
            step: (progress.clamp(0.0, 1.0) * FADE_STEPS as f32) as u8,
            back,
        })
    }

    /// What's asked for. [`Self::output_status`] says what's running.
    pub fn exclusive_output(&self) -> bool {
        self.settings.output.exclusive
    }

    /// None for the system default.
    pub fn output_device(&self) -> Option<&str> {
        let output = &self.settings.output;
        if output.exclusive {
            output.exclusive_device.as_deref()
        } else {
            output.device.as_deref()
        }
    }

    /// Rebuilds the running session onto the other backend at once.
    pub fn set_exclusive_output(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.settings.output.exclusive == on {
            return;
        }
        self.settings.output.exclusive = on;
        Settings::update(move |s| s.output.exclusive = on);
        // Another backend means another set of supported rates.
        self.refused_rates.clear();
        self.rebuild_session(cx);
        cx.notify();
    }

    /// None for the system default. Rebuilds the running session onto it.
    pub fn set_output_device(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        if self.output_device() == device.as_deref() {
            return;
        }
        let exclusive = self.settings.output.exclusive;
        if exclusive {
            self.settings.output.exclusive_device = device.clone();
        } else {
            self.settings.output.device = device.clone();
        }
        Settings::update(move |s| {
            if exclusive {
                s.output.exclusive_device = device;
            } else {
                s.output.device = device;
            }
        });
        self.refused_rates.clear();
        self.rebuild_session(cx);
        cx.notify();
    }

    /// Pin the exclusive rate, or None to follow each file. Reopens now.
    pub fn set_output_rate(&mut self, rate: Option<u32>, cx: &mut Context<Self>) {
        if self.settings.output.rate == rate {
            return;
        }
        self.settings.output.rate = rate;
        Settings::update(move |s| s.output.rate = rate);
        self.refused_rates.clear();
        self.follow_rate = None;
        self.rebuild_session(cx);
        cx.notify();
    }

    /// None for the widest format. A card that refuses the pick runs the
    /// widest and reports that.
    pub fn set_output_format(&mut self, format: Option<String>, cx: &mut Context<Self>) {
        if self.settings.output.format == format {
            return;
        }
        self.settings.output.format = format.clone();
        Settings::update(move |s| s.output.format = format);
        self.rebuild_session(cx);
        cx.notify();
    }

    /// None for the backend default. Shorter periods wake the writer thread
    /// more often and xrun sooner under load.
    pub fn set_output_period(&mut self, ms: Option<f64>, cx: &mut Context<Self>) {
        if self.settings.output.period_ms == ms {
            return;
        }
        self.settings.output.period_ms = ms;
        Settings::update(move |s| s.output.period_ms = ms);
        self.rebuild_session(cx);
        cx.notify();
    }

    /// The pinned exclusive rate, None while output follows the file.
    pub fn output_rate(&self) -> Option<u32> {
        self.settings.output.rate
    }

    /// The pinned exclusive format, None while output takes the widest.
    pub fn output_format(&self) -> Option<&str> {
        self.settings.output.format.as_deref()
    }

    /// The pinned exclusive period in milliseconds, None on the default.
    pub fn output_period(&self) -> Option<f64> {
        self.settings.output.period_ms
    }

    /// A comparable key for the pump's change check.
    fn position_key(&self) -> Option<(usize, u64)> {
        let session = self.session.as_ref()?;
        let (track, secs) = session.shared.position(session.device_rate)?;
        Some((track, secs.to_bits()))
    }

    /// The same, with a paused station's growing timeshift folded in,
    /// quantised to [`PAUSED_SHIFT_STEPS`] a second.
    fn paused_key(&self) -> Option<(usize, u64, u64)> {
        let session = self.session.as_ref()?;
        let (track, secs) = self.position_key()?;
        let moved = session
            .shared
            .shift(track)
            .map(|shift| ((shift.behind_secs + shift.window_secs) * PAUSED_SHIFT_STEPS) as u64)
            .unwrap_or(0);

        Some((track, secs, moved))
    }

    /// Reads chunks straight off the ring's two slices: this runs every tick,
    /// so no per-sample pops and no temporary buffer.
    fn drain_tap(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let n = session.tap.slots();
        let Ok(chunk) = session.tap.read_chunk(n) else {
            return;
        };
        let (a, b) = chunk.as_slices();
        self.feed.push(a);
        self.feed.push(b);
        chunk.commit_all();
    }

    /// Decode one window at the load position so a paused load's spectrum
    /// isn't blank. Skipped if audio started meanwhile, so a stale window
    /// never lands in a live feed. Remote tracks skip it rather than open a
    /// second server connection.
    fn prime_feed(&self, locator: Locator, secs: f64, rate: u32, cx: &mut Context<Self>) {
        if locator.path().is_none() {
            return;
        }

        let feed = self.feed.clone();
        let before = feed.written();
        cx.spawn(async move |this, cx| {
            let window = cx
                .background_executor()
                .spawn(async move {
                    engine::decode_window(&locator, secs, rate, rox_viz::analysis::MAX_FFT_SIZE)
                })
                .await;
            let Ok(samples) = window else { return };
            this.update(cx, |this, cx| {
                if feed.written() != before || this.is_playing() {
                    return;
                }
                feed.push(&samples);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn send(&self, cmd: Cmd) {
        if let Some(session) = &self.session {
            if answers_now(&cmd) {
                session.shared.interrupt();
            }

            let _ = session.tx.send(cmd);
        }
    }

    pub fn toggle_pause(&self) {
        self.send(Cmd::TogglePause);
    }

    pub fn next(&mut self, cx: &mut Context<Self>) {
        self.send(Cmd::Next);
        if !self.settings.session.shuffle || self.shuffle_mode() != ShuffleMode::Similar {
            return;
        }
        // A skip after `SKIP_SETTLE` of listening starts a fresh run. Quicker
        // skips widen the band.
        let now = Instant::now();
        let settled = self
            .last_skip
            .is_none_or(|at| now.duration_since(at) >= SKIP_SETTLE);
        self.similar_skips = if settled { 1 } else { self.similar_skips + 1 };
        self.last_skip = Some(now);
        // Re-seed on where the skip lands, which is what makes skipping steer.
        let leaving = self.seed_entry();
        // Claim the boundary before ranking: the pump sees the landing before
        // the ranking's wait on the engine ends, and would re-seed it at a band
        // of one. The ranking hands the claim back if it can't order.
        self.skip_reseed = SkipReseed::InFlight { passed: false };
        self.order_tail_by_similarity(skip_band(self.similar_skips), leaving, true, cx);
    }

    /// For a caller that waits until the seed is no longer playing.
    fn seed_entry(&self) -> Option<u64> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        let at = self.audible_index(&snap).unwrap_or(snap.cursor);
        snap.entries.get(at).map(|e| e.id)
    }

    pub fn prev(&self) {
        self.send(Cmd::Prev);
    }

    pub fn is_playing(&self) -> bool {
        self.session
            .as_ref()
            .map(|s| s.shared.playing.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// Playing or paused. Tells "opening..." apart from idle before the clock
    /// is up.
    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    pub fn queue_ended(&self) -> bool {
        self.session
            .as_ref()
            .map(|s| s.shared.ended.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// The persisted level (0 to 2) that mute returns to, not what the engine
    /// currently applies.
    pub fn volume(&self) -> f32 {
        self.settings.session.volume
    }

    pub fn muted(&self) -> bool {
        self.settings.session.muted
    }

    fn effective_volume(&self) -> f32 {
        if self.settings.session.muted {
            0.0
        } else {
            self.settings.session.volume
        }
    }

    pub fn loop_mode(&self) -> LoopMode {
        self.settings.session.loop_mode()
    }

    /// Relative seek. On a station it moves the cursor through the buffer
    /// instead. The distance is read fresh each time, since a pause keeps
    /// taping.
    pub fn seek_by(&self, delta: f64) {
        let Some(session) = &self.session else {
            return;
        };
        let Some((track, secs)) = session.shared.position(session.device_rate) else {
            return;
        };

        if session.live.get(track).copied().unwrap_or(false) {
            // No shift until the tape takes its first byte.
            if let Some(target) = session
                .shared
                .shift(track)
                .and_then(|shift| live_step_target(delta, &shift))
            {
                self.seek_live(target);
            }
            return;
        }

        let _ = session.tx.send(Cmd::Seek((secs + delta).max(0.0)));
    }

    /// One fine step. Paused, it also plays a preview blip of where it
    /// landed, since a 25 ms move is too small to see.
    pub fn step_by(&self, back: bool) {
        let step = self.step_ms() as f64 / 1000.0;
        self.seek_by(if back { -step } else { step });
        if !self.is_playing() {
            self.send(Cmd::Audition(self.step_preview_ms() as f64 / 1000.0));
        }
    }

    /// The step size in milliseconds, what the settings row scrubs.
    pub fn step_ms(&self) -> f32 {
        Self::in_range(
            self.settings.step_ms,
            rox_core::settings::STEP_MS_MIN,
            rox_core::settings::STEP_MS_MAX,
            rox_core::settings::DEFAULT_STEP_MS,
        )
    }

    /// How long a paused step plays for, in milliseconds.
    pub fn step_preview_ms(&self) -> f32 {
        Self::in_range(
            self.settings.step_preview_ms,
            rox_core::settings::STEP_PREVIEW_MS_MIN,
            rox_core::settings::STEP_PREVIEW_MS_MAX,
            rox_core::settings::DEFAULT_STEP_PREVIEW_MS,
        )
    }

    /// A stored knob read back inside its range, or the stock value where
    /// the file holds something that isn't a number.
    fn in_range(value: f32, min: f32, max: f32, stock: f32) -> f32 {
        if value.is_finite() {
            value.clamp(min, max)
        } else {
            stock
        }
    }

    /// Nothing to send: the size is read at each press.
    pub fn set_step_ms(&mut self, ms: f32, cx: &mut Context<Self>) {
        let ms = ms.clamp(
            rox_core::settings::STEP_MS_MIN,
            rox_core::settings::STEP_MS_MAX,
        );
        if self.settings.step_ms == ms {
            return;
        }
        self.settings.step_ms = ms;
        self.persist_playback_soon(cx);
        cx.notify();
    }

    pub fn set_step_preview_ms(&mut self, ms: f32, cx: &mut Context<Self>) {
        let ms = ms.clamp(
            rox_core::settings::STEP_PREVIEW_MS_MIN,
            rox_core::settings::STEP_PREVIEW_MS_MAX,
        );
        if self.settings.step_preview_ms == ms {
            return;
        }
        self.settings.step_preview_ms = ms;
        self.persist_playback_soon(cx);
        cx.notify();
    }

    /// Setting a level always unmutes.
    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        // The engine's clamp range, so persisted and audible values agree.
        let volume = volume.clamp(0.0, 2.0);
        self.settings.session.volume = volume;
        self.settings.session.muted = false;
        self.send(Cmd::Volume(volume));
        self.persist_playback_soon(cx);
        cx.notify();
    }

    /// Persist the scrubbed values once the drag settles: `Settings::update`
    /// rewrites the files, too much per pointer move. They share one debounce
    /// so none can outrun another's pending write.
    fn persist_playback_soon(&mut self, cx: &mut Context<Self>) {
        self.persist_gen += 1;
        let generation = self.persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            // Read at write time, so a mute toggled during the wait persists.
            let Ok((latest, volume, muted, crossfade, restore, step, replay_gain, live_buffer)) =
                this.update(cx, |this, _| {
                    (
                        this.persist_gen,
                        this.settings.session.volume,
                        this.settings.session.muted,
                        this.settings.crossfade_secs,
                        this.settings.crossfade_restore_secs,
                        (this.settings.step_ms, this.settings.step_preview_ms),
                        this.settings.replay_gain,
                        this.settings.live_buffer_secs,
                    )
                })
            else {
                return;
            };
            if latest == generation {
                Settings::update(move |s| {
                    s.session.volume = volume;
                    s.session.muted = muted;
                    s.crossfade_secs = crossfade;
                    s.crossfade_restore_secs = restore;
                    (s.step_ms, s.step_preview_ms) = step;
                    s.replay_gain = replay_gain;
                    s.live_buffer_secs = live_buffer;
                });
            }
        })
        .detach();
    }

    /// Silence the output without losing the level; unmute restores it.
    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = !self.settings.session.muted;
        self.settings.session.muted = muted;
        self.send(Cmd::Volume(self.effective_volume()));
        Settings::update(move |s| s.session.muted = muted);
        cx.notify();
    }

    pub fn shuffle(&self) -> bool {
        self.settings.session.shuffle
    }

    /// Similar falls back to Random until the library is described. The
    /// setting keeps Similar, so describing the library brings it back.
    pub fn shuffle_mode(&self) -> ShuffleMode {
        let mode = self.settings.session.shuffle_mode;
        if mode == ShuffleMode::Similar && !rox_core::settings::similarity_ready() {
            return ShuffleMode::Random;
        }
        mode
    }

    /// Applies at once while shuffle is on.
    pub fn set_shuffle_mode(&mut self, mode: ShuffleMode, cx: &mut Context<Self>) {
        if self.settings.session.shuffle_mode == mode {
            return;
        }
        self.settings.session.shuffle_mode = mode;
        Settings::update(move |s| s.session.shuffle_mode = mode);
        if self.settings.session.shuffle {
            self.apply_shuffle_order(cx);
        }
        cx.notify();
    }

    /// The running session reorders in place.
    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.set_shuffle_with(!self.settings.session.shuffle, cx);
    }

    /// Force shuffle to `on` in the random order, for the library's shuffle
    /// actions: "Play Shuffled" means random whatever the transport's mode.
    pub fn set_shuffle(&mut self, on: bool) {
        if self.settings.session.shuffle == on {
            return;
        }
        self.settings.session.shuffle = on;
        self.send(Cmd::SetShuffle(on));
        Settings::update(move |s| s.session.shuffle = on);
    }

    /// Shuffle on in `mode` whatever it was before, as "Play Similar" asks.
    pub fn shuffle_in_mode(&mut self, mode: ShuffleMode, cx: &mut Context<Self>) {
        self.settings.session.shuffle_mode = mode;
        self.settings.session.shuffle = true;
        Settings::update(move |s| {
            s.session.shuffle_mode = mode;
            s.session.shuffle = true;
        });
        self.apply_shuffle_order(cx);
        cx.notify();
    }

    fn set_shuffle_with(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.settings.session.shuffle == on {
            return;
        }
        self.settings.session.shuffle = on;
        Settings::update(move |s| s.session.shuffle = on);
        if on {
            self.apply_shuffle_order(cx);
        } else {
            // Off is off for every mode: the engine restores pool order.
            self.send(Cmd::SetShuffle(false));
        }
        cx.notify();
    }

    fn apply_shuffle_order(&mut self, cx: &mut Context<Self>) {
        match self.shuffle_mode() {
            ShuffleMode::Random => self.send(Cmd::SetShuffle(true)),
            ShuffleMode::Similar => {
                self.order_tail_by_similarity(1, None, false, cx);
                // Sorting what's queued isn't radio: draw a batch now, and the
                // landing re-ranks it in.
                if similar_draw_now(
                    self.settings.session.continuation,
                    self.similar_order(),
                    self.is_playing(),
                    self.stop_after,
                ) {
                    self.request_continuation(true, cx);
                }
            }
        }
    }

    /// Order what's coming by how much it sounds like the playing track and
    /// how close its tempo is ([`embeddings::ranked`]), on the background
    /// executor. Unscored entries keep their place behind scored ones (see
    /// [`Cmd::OrderTail`]).
    ///
    /// Never reset the tail to pool order first: a scan that then finds
    /// nothing would leave the queue in library order.
    ///
    /// `from_skip` holds a [`SkipReseed`] claim. Every path out settles it.
    fn order_tail_by_similarity(
        &mut self,
        band: usize,
        leaving: Option<u64>,
        from_skip: bool,
        cx: &mut Context<Self>,
    ) {
        let db_path = rox_core::settings::data_dir().join("library.db");
        cx.spawn(async move |this, cx| {
            // A just-started context hasn't published its queue yet. Wait for
            // it: engaging the mode and replacing the queue happen together.
            let mut inputs = None;
            for attempt in 0..QUEUE_WAIT_TRIES {
                if attempt > 0 {
                    cx.background_executor().timer(QUEUE_WAIT_STEP).await;
                }
                inputs = this
                    .update(cx, |this, _| this.similarity_inputs(leaving))
                    .ok()
                    .flatten();
                if inputs.is_some() {
                    break;
                }
            }
            let Some((seed, tail)) = inputs else {
                if from_skip {
                    this.update(cx, |this, cx| this.release_skip_reseed(cx))
                        .ok();
                }
                return;
            };
            let model = crate::acoustic::acoustic_source().id().to_string();
            let ranked = cx
                .background_executor()
                .spawn(async move {
                    let conn = store::open(&db_path).ok()?;
                    let seed = seed?;
                    let scores: HashMap<i64, f32> = embeddings::ranked(&conn, seed, &model)
                        .ok()?
                        .into_iter()
                        .collect();
                    // Unscored entries drop out rather than sorting as zero,
                    // which would rank them above genuinely unalike tracks.
                    let mut ranked: Vec<(u64, i64, f32)> = tail
                        .into_iter()
                        .filter_map(|(entry, id)| Some((entry, id, *scores.get(&id)?)))
                        .collect();
                    ranked.sort_by(|a, b| b.2.total_cmp(&a.2));
                    // Spread recordings of one song, which sound identical and
                    // would otherwise run together. Nothing is dropped.
                    let (playing, songs) = entry_songs(&conn, seed, &ranked);
                    let mut ids: Vec<u64> = ranked.into_iter().map(|(entry, _, _)| entry).collect();
                    song::space(&mut ids, SONG_SPACING, playing.as_deref(), |entry| {
                        songs.get(entry).map(String::as_str)
                    });
                    // Shuffle only the head, so the ranking behind it stays
                    // closest first.
                    shuffle_head(&mut ids, band);
                    Some(ids)
                })
                .await;
            let Some(ids) = ranked.filter(|ids: &Vec<u64>| !ids.is_empty()) else {
                log::info!("shuffle: nothing analyzed to order the queue by");
                if from_skip {
                    this.update(cx, |this, cx| this.release_skip_reseed(cx))
                        .ok();
                }
                return;
            };
            this.update(cx, |this, cx| {
                // Drop the answer if shuffle or its mode changed meanwhile.
                if this.settings.session.shuffle && this.shuffle_mode() == ShuffleMode::Similar {
                    if from_skip {
                        this.skip_reseed = this.skip_reseed.landed();
                    }
                    this.send(Cmd::OrderTail(ids));
                    cx.notify();
                } else if from_skip {
                    this.release_skip_reseed(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Hand a skip's claim back after its ranking came to nothing. If the
    /// boundary passed meanwhile, its re-seed is still owed, or the tail
    /// stays ranked against a track already skipped.
    fn release_skip_reseed(&mut self, cx: &mut Context<Self>) {
        let (claim, owed) = self.skip_reseed.abandon();
        self.skip_reseed = claim;
        if owed && self.similar_order() {
            self.order_tail_by_similarity(1, None, false, cx);
        }
    }

    /// The seed and the upcoming entries to order, None until the engine
    /// publishes a queue or with nothing ahead. Library ids off the pool
    /// mirror, since a path isn't an identity once cue tracks exist. The seed
    /// is optional so an unheld track isn't mistaken for an unpublished queue.
    #[allow(clippy::type_complexity)]
    fn similarity_inputs(&self, leaving: Option<u64>) -> Option<(Option<i64>, Vec<(u64, i64)>)> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        // A skip seeds on the track the engine adopted, not the audible one:
        // under a crossfade the clock flips at the midpoint (ADR 19), and
        // waiting for it would outlast the retries. Before anything is
        // audible, the published cursor.
        let at = leaving
            .and_then(|_| self.adopted_index(&snap))
            .or_else(|| self.audible_index(&snap))
            .unwrap_or(snap.cursor);
        let entry = snap.entries.get(at)?;
        // The engine takes the skip on its own thread. Until it does, the
        // seed is still the track being left.
        if leaving == Some(entry.id) {
            return None;
        }
        let seed = self.pool_ids.get(entry.idx).copied().flatten();
        let tail: Vec<(u64, i64)> = snap
            .entries
            .get(at + 1..)?
            .iter()
            .filter_map(|e| Some((e.id, self.pool_ids.get(e.idx).copied().flatten()?)))
            .collect();
        (!tail.is_empty()).then_some((seed, tail))
    }

    pub fn stop_after(&self) -> bool {
        self.stop_after
    }

    /// Armed, the engine plays the current track out, pauses, and cues the
    /// next. Sticky until cleared.
    pub fn toggle_stop_after(&mut self, cx: &mut Context<Self>) {
        self.stop_after = !self.stop_after;
        self.send(Cmd::SetStopAfter(self.stop_after));
        cx.notify();
    }

    /// The engine's loop first, then the half-marked step if it still
    /// belongs to the playing track.
    pub fn ab_state(&self) -> AbState {
        if let Some((a, b)) = self.session.as_ref().and_then(|s| s.shared.ab()) {
            return AbState::Looping(a, b);
        }
        let Some((key, a)) = self.ab_pending_a.as_ref() else {
            return AbState::Off;
        };
        match self.now_playing() {
            Some(now) if now.key == *key => AbState::ASet(*a),
            _ => AbState::Off,
        }
    }

    /// Mark A, mark B and loop, clear. A second press too close to the first
    /// drops the mark, since the engine refuses a section that short.
    pub fn ab_mark(&mut self, cx: &mut Context<Self>) {
        let Some(now) = self.now_playing() else {
            return;
        };
        match ab_step(self.ab_state(), now.position_secs) {
            AbStep::Pending(a) => self.ab_pending_a = Some((now.key, a)),
            AbStep::Send(marks) => {
                self.ab_pending_a = None;
                self.send(Cmd::SetAbLoop(marks));
            }
            AbStep::Nothing => self.ab_pending_a = None,
        }
        cx.notify();
    }

    /// Drop the section and any half-marked step.
    pub fn ab_clear(&mut self, cx: &mut Context<Self>) {
        self.ab_pending_a = None;
        self.send(Cmd::SetAbLoop(None));
        cx.notify();
    }

    /// Set a section outright in track seconds, as the control socket does.
    pub fn ab_set(&mut self, a: f64, b: f64, cx: &mut Context<Self>) -> Result<(), String> {
        if self.now_playing().is_none() {
            return Err("nothing is playing".into());
        }
        let Some(marks) = ab_section(a, b) else {
            return Err(format!(
                "a section needs two positions in seconds at least {} apart",
                engine::AB_MIN_SECS
            ));
        };
        self.ab_pending_a = None;
        self.send(Cmd::SetAbLoop(Some(marks)));
        cx.notify();
        Ok(())
    }

    /// Arm stop-after once `after` has passed, or clear with None. Pause
    /// doesn't hold the timer.
    pub fn set_sleep(&mut self, after: Option<Duration>, cx: &mut Context<Self>) {
        self.sleep = after.map(|after| Instant::now() + after);
        cx.notify();
    }

    /// Saturating, so a timer the pump hasn't reached reads zero.
    pub fn sleep_remaining(&self) -> Option<Duration> {
        let now = Instant::now();
        self.sleep
            .map(|ends_at| ends_at.saturating_duration_since(now))
    }

    /// Arms through the toggle so the engine and the button both learn.
    fn tick_sleep(&mut self, cx: &mut Context<Self>) {
        match sleep_step(Instant::now(), self.sleep, self.stop_after) {
            SleepStep::Nothing => return,
            SleepStep::Arm => self.toggle_stop_after(cx),
            SleepStep::Clear => {}
        }
        self.sleep = None;
        cx.notify();
    }

    /// Step off -> all -> one -> off and persist the pick.
    pub fn cycle_loop(&mut self) {
        let mode = match self.settings.session.loop_mode() {
            LoopMode::Off => LoopMode::All,
            LoopMode::All => LoopMode::One,
            LoopMode::One => LoopMode::Off,
        };
        self.settings.session.set_loop_mode(mode);
        self.send(Cmd::SetLoop(mode));
        Settings::update(|s| s.session.set_loop_mode(mode));
    }

    /// Show why the engine gave up on an entry, since the queue moves on
    /// silently. It stays up until the next session start or stop.
    fn take_refusal(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_ref() else {
            return;
        };

        let Some(reason) = session.shared.take_refusal() else {
            return;
        };

        self.error = Some(rox_i18n::t!("player-stream-refused", reason = reason));
        cx.notify();
    }

    /// The last session-start failure, shown while nothing plays.
    pub fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }

    /// What [`observe_view`] diffs to decide whether a tick is worth a
    /// repaint.
    pub fn view(&self) -> PlayerView {
        let now = self.now_playing();
        PlayerView {
            track: now.as_ref().map(|now| now.key.clone()),
            duration_secs: now.and_then(|now| now.duration_secs),
            playing: self.is_playing(),
            active: self.is_active(),
            ended: self.queue_ended(),
            loop_mode: self.loop_mode(),
            shuffle: self.shuffle(),
            shuffle_mode: self.shuffle_mode(),
            continuation: self.continuation_mode(),
            crossfade_secs: self.crossfade_secs(),
            stop_after: self.stop_after(),
            ab: self.ab_state(),
            sleep_remaining_secs: self.sleep_remaining().map(|left| left.as_secs()),
            muted: self.muted(),
            volume: self.volume(),
            error: self.error(),
            fade: self.crossfade(),
            title_rev: self.title_rev().unwrap_or(0),
        }
    }
}

/// The EQ's live parameters (ADR 19), one set per process, so a band moves
/// under whatever plays without holding a player.
fn eq_params() -> &'static Arc<EqParams> {
    static EQ: OnceLock<Arc<EqParams>> = OnceLock::new();
    EQ.get_or_init(|| {
        let saved = Settings::load().eq;
        Arc::new(EqParams::new(
            saved.enabled,
            &saved.gains,
            &saved.freqs,
            &saved.qs,
        ))
    })
}

/// Touched by every EQ setter so curve surfaces wake on a move. Holds
/// nothing: observers read the parameters back.
#[derive(Default)]
pub struct EqChanged;

impl Global for EqChanged {}

/// Taking the global mutably is the notification: gpui wakes observers off
/// the borrow.
fn eq_changed(cx: &mut App) {
    let _ = cx.default_global::<EqChanged>();
}

/// Wake `view` whenever the curve moves. The parameters are atomics with no
/// gpui entity, so this watches the setters' global.
pub fn observe_eq<V: 'static>(cx: &mut Context<V>) -> Subscription {
    cx.observe_global::<EqChanged>(|_, cx| cx.notify())
}

pub fn eq_enabled() -> bool {
    eq_params().enabled()
}

/// One band's gain in dB, in [`rox_playback::eq::BAND_HZ`] order.
pub fn eq_gain(band: usize) -> f32 {
    eq_params().gain(band)
}

/// The node stays in the chain and passes through while off, so this is a
/// store. It lands as the ring drains, up to half a second after the click.
pub fn set_eq_enabled(on: bool, cx: &mut App) {
    eq_params().set_enabled(on);
    Settings::update(move |s| s.eq.enabled = on);
    eq_changed(cx);
}

pub fn set_eq_gain(band: usize, db: f32, cx: &mut App) {
    eq_params().set_gain(band, db);
    persist_eq_soon(cx);
    eq_changed(cx);
}

/// Every band to 0 dB, where the EQ stops touching the samples.
pub fn flatten_eq(cx: &mut App) {
    eq_params().flatten();
    let gains = eq_params().gains();
    Settings::update(move |s| s.eq.gains = gains);
    eq_changed(cx);
}

/// A band's center in Hz, and its width.
pub fn eq_freq(band: usize) -> f32 {
    eq_params().freq(band)
}

pub fn eq_q(band: usize) -> f32 {
    eq_params().q(band)
}

pub fn set_eq_freq(band: usize, hz: f32, cx: &mut App) {
    eq_params().set_freq(band, hz);
    persist_eq_soon(cx);
    eq_changed(cx);
}

pub fn set_eq_q(band: usize, q: f32, cx: &mut App) {
    eq_params().set_q(band, q);
    persist_eq_soon(cx);
    eq_changed(cx);
}

/// One band back to its ISO octave, flat, one octave wide.
pub fn reset_eq_band(band: usize, cx: &mut App) {
    let params = eq_params();
    params.set_freq(band, rox_playback::eq::BAND_HZ[band]);
    params.set_gain(band, 0.0);
    params.set_q(band, rox_playback::eq::Q_DEFAULT);
    let (gains, freqs, qs) = (params.gains(), params.freqs(), params.qs());
    Settings::update(move |s| {
        s.eq.gains = gains;
        s.eq.freqs = freqs;
        s.eq.qs = qs;
    });
    eq_changed(cx);
}

/// Every band back to its ISO octave at one octave wide, gains untouched.
pub fn reset_eq_shape(cx: &mut App) {
    eq_params().reset_shape();
    let (freqs, qs) = (eq_params().freqs(), eq_params().qs());
    Settings::update(move |s| {
        s.eq.freqs = freqs;
        s.eq.qs = qs;
    });
    eq_changed(cx);
}

/// Apply a graphic curve, resetting centers and widths to their ISO octaves.
pub fn apply_graphic_eq(gains: &[f32; rox_playback::eq::BANDS], cx: &mut App) {
    let params = eq_params();
    params.apply_graphic_curve(gains);
    let (gains_vec, freqs, qs) = (params.gains(), params.freqs(), params.qs());
    Settings::update(move |s| {
        s.eq.gains = gains_vec;
        s.eq.freqs = freqs;
        s.eq.qs = qs;
    });
    eq_changed(cx);
}

/// Put a whole curve in place as (Hz, dB, Q) per band, for saved presets
/// whose bands moved and narrowed. Extra bands are cut off, and bands the
/// list doesn't reach stay put.
pub fn apply_eq_bands(bands: &[(f32, f32, f32)], cx: &mut App) {
    let params = eq_params();
    for (band, &(hz, db, q)) in bands.iter().take(rox_playback::eq::BANDS).enumerate() {
        params.set_freq(band, hz);
        params.set_gain(band, db);
        params.set_q(band, q);
    }

    let (gains, freqs, qs) = (params.gains(), params.freqs(), params.qs());
    Settings::update(move |s| {
        s.eq.gains = gains;
        s.eq.freqs = freqs;
        s.eq.qs = qs;
    });
    eq_changed(cx);
}

/// Debounced like [`Player::persist_playback_soon`]. The generation is
/// global because the parameters are.
fn persist_eq_soon(cx: &mut App) {
    static GEN: AtomicU64 = AtomicU64::new(0);
    let mine = GEN.fetch_add(1, Ordering::Relaxed) + 1;
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(200))
            .await;
        if GEN.load(Ordering::Relaxed) != mine {
            return;
        }
        let params = eq_params();
        let (gains, freqs, qs) = (params.gains(), params.freqs(), params.qs());
        Settings::update(move |s| {
            s.eq.gains = gains;
            s.eq.freqs = freqs;
            s.eq.qs = qs;
        });
    })
    .detach();
}

/// Observe the player, waking only when its discrete state changes. Views
/// that draw the clock observe the player directly.
pub fn observe_view<V: 'static>(player: &Entity<Player>, cx: &mut Context<V>) -> Subscription {
    let mut last = player.read(cx).view();
    cx.observe(player, move |_, player, cx| {
        let now = player.read(cx).view();
        if now != last {
            last = now;
            cx.notify();
        }
    })
}

/// [`observe_view`] for the output state. Its own subscription because only
/// the settings window draws it and the compare costs a lock.
pub fn observe_output<V: 'static>(player: &Entity<Player>, cx: &mut Context<V>) -> Subscription {
    let mut last = player.read(cx).output_status();
    cx.observe(player, move |_, player, cx| {
        let now = player.read(cx).output_status();
        if now != last {
            last = now;
            cx.notify();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use rox_library::rusqlite::{Connection, params};

    /// Tagged rows for the song-identity lookups, ids in insertion order.
    fn tagged(rows: &[(&str, &str)]) -> (Connection, Vec<i64>) {
        let conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let mut ids = Vec::new();
        for (i, (artist, title)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO tracks (path, title, artist, album, album_artist, genre, year,
                    track_no, disc_no, duration_ms, size, mtime)
                 VALUES (?1, ?2, ?3, 'al', ?3, 'g', 0, 1, 1, 200000, 0, 0)",
                params![format!("/m/{i:03}.flac"), title, artist],
            )
            .unwrap();
            ids.push(conn.last_insert_rowid());
        }
        (conn, ids)
    }

    #[test]
    fn the_ab_cycle_marks_then_loops_then_clears() {
        assert_eq!(ab_step(AbState::Off, 12.0), AbStep::Pending(12.0));
        assert_eq!(
            ab_step(AbState::ASet(12.0), 18.5),
            AbStep::Send(Some((12.0, 18.5)))
        );
        assert_eq!(
            ab_step(AbState::Looping(12.0, 18.5), 14.0),
            AbStep::Send(None)
        );
        assert_eq!(ab_step(AbState::Off, 3.0), AbStep::Pending(3.0));
    }

    #[test]
    fn a_section_set_outright_is_ordered_and_floored() {
        assert_eq!(ab_section(12.0, 18.5), Some((12.0, 18.5)));
        assert_eq!(ab_section(18.5, 12.0), Some((12.0, 18.5)));
        assert_eq!(ab_section(12.0, 12.1), None);
        assert_eq!(ab_section(12.0, 12.0), None);
        assert_eq!(ab_section(-1.0, 5.0), None);
        assert_eq!(ab_section(f64::NAN, 5.0), None);
    }

    /// Pins `>=`: a timer landing exactly on a tick fires on it.
    #[test]
    fn the_sleep_timer_fires_at_its_moment_and_not_before() {
        let now = Instant::now();
        let ends_at = now + Duration::from_secs(60);
        assert_eq!(sleep_step(now, Some(ends_at), false), SleepStep::Nothing);
        assert_eq!(
            sleep_step(ends_at - Duration::from_millis(1), Some(ends_at), false),
            SleepStep::Nothing
        );
        assert_eq!(sleep_step(ends_at, Some(ends_at), false), SleepStep::Arm);
        assert_eq!(
            sleep_step(ends_at + Duration::from_secs(5), Some(ends_at), false),
            SleepStep::Arm
        );
        assert_eq!(sleep_step(now, None, false), SleepStep::Nothing);
        assert_eq!(sleep_step(now, None, true), SleepStep::Nothing);
    }

    /// Arming is a toggle, so firing on a hand-armed stop would turn it off.
    #[test]
    fn a_fired_sleep_timer_never_disarms_a_stop_already_set() {
        let now = Instant::now();
        assert_eq!(sleep_step(now, Some(now), false), SleepStep::Arm);
        assert_eq!(sleep_step(now, Some(now), true), SleepStep::Clear);
    }

    #[test]
    fn a_second_mark_too_close_to_the_first_drops_it() {
        assert_eq!(ab_step(AbState::ASet(12.0), 12.05), AbStep::Nothing);
        // And B behind A, from a seek backwards between the two presses.
        assert_eq!(ab_step(AbState::ASet(12.0), 4.0), AbStep::Nothing);
    }

    /// The Barracuda case: the ranking is the seed's own song over and over.
    #[test]
    fn a_similar_draw_leaves_the_seed_s_own_song_behind() {
        let (conn, ids) = tagged(&[
            ("Heart", "Barracuda"),
            ("Heart", "Barracuda (Live)"),
            ("Heart", "Barracuda (Live In Japan)"),
            ("Heart", "Barracuda - Live"),
            ("Heart", "Barracuda [2010 Remaster]"),
            ("Heart", "Crazy On You"),
            ("Heart", "Magic Man"),
        ]);
        // Every copy scores above the two tracks that aren't the same song.
        let near: Vec<(i64, f32)> = ids
            .iter()
            .skip(1)
            .enumerate()
            .map(|(at, &id)| (id, 1.0 - at as f32 / 100.0))
            .collect();
        let band = one_per_song(&conn, ids[0], &HashSet::new(), &near);
        assert_eq!(
            band.iter().map(|&(id, _)| id).collect::<Vec<_>>(),
            vec![ids[5], ids[6]],
            "the draw stayed on the seed's own song"
        );
    }

    #[test]
    fn a_similar_draw_skips_a_song_the_session_already_heard() {
        let (conn, ids) = tagged(&[
            ("Heart", "Barracuda"),
            ("Heart", "Crazy On You (Live)"),
            ("Heart", "Magic Man"),
        ]);
        let near: Vec<(i64, f32)> = vec![(ids[1], 0.9), (ids[2], 0.8)];
        // The session heard the live take; the studio cut isn't in the library.
        let seen: HashSet<i64> = [ids[1]].into_iter().collect();
        let band = one_per_song(&conn, ids[0], &seen, &near);
        assert_eq!(
            band.iter().map(|&(id, _)| id).collect::<Vec<_>>(),
            vec![ids[2]]
        );
    }

    #[test]
    fn a_similar_draw_on_nothing_but_copies_still_plays() {
        let (conn, ids) = tagged(&[
            ("Heart", "Barracuda"),
            ("Heart", "Barracuda (Live)"),
            ("Heart", "Barracuda - Live"),
        ]);
        let near: Vec<(i64, f32)> = vec![(ids[1], 0.99), (ids[2], 0.98)];
        assert_eq!(one_per_song(&conn, ids[0], &HashSet::new(), &near).len(), 2);
    }

    #[test]
    fn the_ordering_reads_a_song_identity_per_queue_entry() {
        let (conn, ids) = tagged(&[
            ("Heart", "Barracuda"),
            ("Heart", "Barracuda (Live)"),
            ("", ""),
        ]);
        let ranked = vec![(10u64, ids[1], 0.9), (11u64, ids[2], 0.8)];
        let (playing, songs) = entry_songs(&conn, ids[0], &ranked);
        let seed_key = playing.expect("a tagged seed has an identity");
        assert_eq!(songs.get(&10), Some(&seed_key), "the live take is the song");
        assert!(!songs.contains_key(&11), "an untagged row has no identity");
    }

    #[test]
    fn the_band_opens_up_with_each_skip_in_a_run() {
        assert_eq!(skip_band(0), 1, "settled radio plays the nearest track");
        assert_eq!(skip_band(1), 4);
        assert_eq!(skip_band(2), 16);
        assert_eq!(skip_band(3), 64);
        // Monotonic, and saturating rather than overflowing.
        let mut last = 0;
        for skips in 0..64 {
            let band = skip_band(skips);
            assert!(band >= last, "band never narrows mid-run");
            last = band;
        }
    }

    #[test]
    fn shuffling_the_head_leaves_the_ranking_behind_it() {
        let ranked: Vec<u64> = (0..20).collect();
        let mut ids = ranked.clone();
        shuffle_head(&mut ids, 5);
        assert_eq!(
            &ids[5..],
            &ranked[5..],
            "the tail past the band is untouched"
        );
        let mut head = ids[..5].to_vec();
        head.sort();
        assert_eq!(head, ranked[..5], "the band holds the same entries");
    }

    #[test]
    fn a_random_draw_stays_inside_the_playing_view() {
        let all = vec![1, 2, 3, 4, 5];
        let view = continuation::Scope::View(vec![7, 8].into());
        assert_eq!(random_pool(&view, &all), &[7, 8]);
        assert_eq!(
            random_pool(&continuation::Scope::Library, &all),
            all.as_slice()
        );
        // A one-track view is still the pool.
        let single = continuation::Scope::View(vec![7].into());
        assert_eq!(random_pool(&single, &all), &[7]);
    }

    #[test]
    fn an_empty_view_falls_back_to_the_library() {
        let all = vec![1, 2, 3];
        let empty = continuation::Scope::View(Vec::new().into());
        assert_eq!(random_pool(&empty, &all), all.as_slice());
        assert!(random_pool(&empty, &[]).is_empty());
    }

    #[test]
    fn a_random_draw_brings_the_run_around_it() {
        // A pool under the cap comes back whole, wherever the draw fell.
        assert_eq!(run_window(0, 40), (0, 40));
        assert_eq!(run_window(39, 40), (0, 40));

        let len = QUEUE_CAP * 3;
        // Room on both sides: half the budget behind the draw for Prev.
        let (lo, hi) = run_window(len / 2, len);
        assert_eq!((lo, hi), (len / 2 - QUEUE_CAP / 2, len / 2 + QUEUE_CAP / 2));
        assert_eq!(run_window(10, len), (0, QUEUE_CAP));
        // Against the back, it slides forward instead of coming up short.
        let (lo, hi) = run_window(len - 1, len);
        assert_eq!((lo, hi), (len - QUEUE_CAP, len));
        for at in [0, 1, QUEUE_CAP, len / 2, len - 2, len - 1] {
            let (lo, hi) = run_window(at, len);
            assert!(
                lo <= at && at < hi,
                "the draw at {at} sits outside {lo}..{hi}"
            );
            assert_eq!(hi - lo, QUEUE_CAP, "a full window either side of {at}");
        }
    }

    #[test]
    fn a_random_draw_avoids_what_the_session_has_held() {
        let pool = vec![1, 2, 3, 4, 5];
        let seen: HashSet<i64> = [1, 2, 4, 5].into_iter().collect();
        for _ in 0..32 {
            assert_eq!(draw_at(&pool, &seen), Some(2), "3 is the one fresh track");
        }
        // Everything heard: the pool opens back up.
        let all: HashSet<i64> = pool.iter().copied().collect();
        for _ in 0..32 {
            let at = draw_at(&pool, &all).expect("an exhausted pool still draws");
            assert!(at < pool.len());
        }
        assert_eq!(draw_at(&[], &seen), None);
    }

    #[test]
    fn the_similar_band_skips_heard_neighbours_until_it_runs_out() {
        let near = vec![(10, 0.9), (11, 0.8), (12, 0.7)];
        let seen: HashSet<i64> = [10, 12].into_iter().collect();
        assert_eq!(fresh_band(&near, &seen), vec![11]);
        // The whole neighbourhood heard: it repeats rather than wandering.
        let all: HashSet<i64> = [10, 11, 12].into_iter().collect();
        assert_eq!(fresh_band(&near, &all), vec![10, 11, 12]);
        // Nothing analyzed near the seed stays nothing to draw from.
        assert!(fresh_band(&[], &seen).is_empty());
    }

    /// Pins the bounce a pool-only guard allowed: A, then B, then A again,
    /// because the new session's pool never held A.
    #[test]
    fn the_heard_ring_stops_the_bounce_between_two_neighbours() {
        let mut heard = VecDeque::new();
        // Session one held A (id 1). The press replaces it with B (id 2).
        remember_held(&mut heard, [1].into_iter());
        let pool = [Some(2)];
        let seen: HashSet<i64> = pool
            .iter()
            .flatten()
            .copied()
            .chain(heard.iter().copied())
            .collect();
        // B's band holds A nearest, exactly the shape that bounced.
        let band = vec![(1, 0.95), (3, 0.9)];
        assert_eq!(fresh_band(&band, &seen), vec![3], "the press walks on");
    }

    #[test]
    fn the_heard_ring_dedupes_and_evicts_oldest_first() {
        let mut heard = VecDeque::new();
        remember_held(&mut heard, [1, 2, 3].into_iter());
        remember_held(&mut heard, [2].into_iter());
        assert_eq!(heard.iter().copied().collect::<Vec<_>>(), vec![1, 3, 2]);
        remember_held(&mut heard, (0..HEARD_CAP as i64).map(|i| 100 + i));
        assert_eq!(heard.len(), HEARD_CAP, "the ring never grows past the cap");
        assert!(!heard.contains(&1), "the oldest aged out first");
        assert_eq!(heard.back(), Some(&(100 + HEARD_CAP as i64 - 1)));
    }

    #[test]
    fn a_random_index_lands_inside_the_pool() {
        for len in 1..16 {
            for _ in 0..64 {
                assert!(random_index(len) < len);
            }
        }
    }

    #[test]
    fn the_trigger_fires_inside_the_floor_and_not_above_it() {
        assert!(!queue_running_dry(19, LoopMode::Off));
        // Three to go is one over the floor, two is the floor itself.
        assert!(!queue_running_dry(3, LoopMode::Off));
        assert!(queue_running_dry(2, LoopMode::Off));
        assert!(queue_running_dry(1, LoopMode::Off));
        // The last entry, which is also where an ended queue sits.
        assert!(queue_running_dry(0, LoopMode::Off));
    }

    #[test]
    fn a_pause_or_an_armed_stop_keeps_the_queue_from_growing() {
        assert!(continuation_wanted(true, false));
        assert!(!continuation_wanted(false, false), "a paused queue");
        assert!(
            !continuation_wanted(true, true),
            "stop-after is armed, so the queue stays as it is"
        );
        assert!(!continuation_wanted(false, true));
    }

    #[test]
    fn engaging_the_similar_order_draws_without_waiting_for_the_floor() {
        assert!(similar_draw_now(
            continuation::Mode::Continue,
            true,
            true,
            false
        ));
        assert!(similar_draw_now(
            continuation::Mode::Weighted,
            true,
            true,
            false
        ));
        // The random order refills at the floor.
        assert!(!similar_draw_now(
            continuation::Mode::Continue,
            false,
            true,
            false
        ));
    }

    #[test]
    fn the_similar_draw_respects_off_and_the_transport() {
        assert!(!similar_draw_now(
            continuation::Mode::Off,
            true,
            true,
            false
        ));
        assert!(
            continuation::provider(continuation::Mode::Off, continuation::Order::Similar).is_none()
        );
        assert!(!similar_draw_now(
            continuation::Mode::Continue,
            true,
            false,
            false
        ));
        assert!(!similar_draw_now(
            continuation::Mode::Continue,
            true,
            true,
            true
        ));
    }

    #[test]
    fn a_natural_boundary_reseeds_the_ordering() {
        assert!(reseed_at_boundary(true, false, 7, Some(6)));
        // Same track, nothing owed: the case that runs every tick.
        assert!(!reseed_at_boundary(true, false, 7, Some(7)));
        assert!(!reseed_at_boundary(false, false, 7, Some(6)));
        // A fresh session orders its own tail.
        assert!(!reseed_at_boundary(true, false, 0, None));
    }

    #[test]
    fn a_skip_pays_for_the_boundary_it_causes() {
        assert!(!reseed_at_boundary(true, true, 7, Some(6)));
        // One boundary each: the flag is spent at the first change.
        assert!(reseed_at_boundary(true, false, 8, Some(7)));
    }

    /// The usual order: the pump sees the boundary before the ranking lands.
    #[test]
    fn a_skips_claim_covers_the_boundary_that_beats_it() {
        let claim = SkipReseed::InFlight { passed: false };
        let (claim, paid) = claim.spend();
        assert!(paid, "the boundary is the skip's, ranked or not yet");
        assert_eq!(claim, SkipReseed::InFlight { passed: true });
        // The late ranking answers that same boundary. Nothing is held over.
        assert_eq!(claim.landed(), SkipReseed::Idle);
        assert!(!SkipReseed::Idle.spend().1, "so the next one re-seeds");
    }

    #[test]
    fn a_skips_claim_waits_when_the_ranking_lands_first() {
        let claim = SkipReseed::InFlight { passed: false }.landed();
        assert_eq!(claim, SkipReseed::Ready);
        let (claim, paid) = claim.spend();
        assert!(paid, "already ranked at the band the run earned");
        assert_eq!(claim, SkipReseed::Idle, "and only for the one boundary");
    }

    /// Pins the bug the release exists for: a boundary held off for a ranking
    /// that gave up is still owed its re-seed.
    #[test]
    fn a_ranking_that_gives_up_hands_the_boundary_back() {
        let (claim, owed) = SkipReseed::InFlight { passed: true }.abandon();
        assert!(owed, "the pump declined for an order that never arrived");
        assert_eq!(claim, SkipReseed::Idle);
        // Given up before the boundary: it re-seeds itself when it arrives.
        let (claim, owed) = SkipReseed::InFlight { passed: false }.abandon();
        assert!(!owed);
        assert!(!claim.spend().1);
    }

    #[test]
    fn loop_suppresses_the_trigger_at_any_distance() {
        for mode in [LoopMode::All, LoopMode::One] {
            assert!(!queue_running_dry(0, mode));
            assert!(!queue_running_dry(1, mode));
            assert!(!queue_running_dry(19, mode));
        }
    }

    #[test]
    fn a_band_of_one_or_wider_than_the_queue_is_safe() {
        let ranked: Vec<u64> = (0..5).collect();
        let mut ids = ranked.clone();
        shuffle_head(&mut ids, 1);
        assert_eq!(ids, ranked);
        shuffle_head(&mut ids, 0);
        assert_eq!(ids, ranked);
        let mut wide = ranked.clone();
        shuffle_head(&mut wide, 999);
        wide.sort();
        assert_eq!(wide, ranked);
        let mut empty: Vec<u64> = Vec::new();
        shuffle_head(&mut empty, 4);
        assert!(empty.is_empty());
    }

    /// Pins ADR 19's gapless rule: cue tracks of one image share an album
    /// group, or the crossfade would fade mid-record. The group comes from
    /// (album artist, album), which the scanner writes on every cut row.
    #[test]
    fn cue_tracks_of_one_image_share_an_album_group() {
        let mut conn = rox_library::rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let image = "/m/Album/disc.flac";
        store::insert_batch(
            &mut conn,
            &[
                cue_row(image, 1, 0, Some(180_000)),
                cue_row(image, 2, 180_000, Some(400_000)),
                cue_row(image, 3, 400_000, None),
                other_album_row("/m/Other/loose.flac"),
            ],
        )
        .unwrap();

        let keys = [
            TrackKey {
                source: rox_library::cue::local(),
                path: PathBuf::from(image),
                sub: 1,
            },
            TrackKey {
                source: rox_library::cue::local(),
                path: PathBuf::from(image),
                sub: 2,
            },
            TrackKey {
                source: rox_library::cue::local(),
                path: PathBuf::from(image),
                sub: 3,
            },
            TrackKey::from(PathBuf::from("/m/Other/loose.flac")),
        ];
        let meta = resolve_queue_meta(Some(&conn), &keys);

        assert!(meta.groups[0].is_some(), "a cue track is grouped at all");
        assert_eq!(
            meta.groups[0], meta.groups[1],
            "tracks 1 and 2 of one rip are the same album"
        );
        assert_eq!(meta.groups[1], meta.groups[2]);
        assert_ne!(
            meta.groups[2], meta.groups[3],
            "a track off another record is not"
        );

        assert_eq!(
            meta.spans[0],
            Some(Span {
                start_ms: 0,
                end_ms: Some(180_000)
            })
        );
        assert_eq!(
            meta.spans[2],
            Some(Span {
                start_ms: 400_000,
                end_ms: None
            }),
            "the last track of an image runs to the file's own end"
        );
        assert_eq!(meta.spans[3], None, "a plain file plays whole");
        assert_eq!(meta.ids.len(), 4);
        assert!(meta.ids.iter().all(|id| id.is_some()));
        assert_ne!(meta.ids[0], meta.ids[1], "each cue track is its own row");
    }

    #[test]
    fn a_missing_database_defaults_every_key() {
        let keys = [
            TrackKey::from(PathBuf::from("/m/a.flac")),
            TrackKey {
                source: rox_library::cue::local(),
                path: PathBuf::from("/m/disc.flac"),
                sub: 4,
            },
        ];
        let meta = resolve_queue_meta(None, &keys);
        assert_eq!(meta.groups, vec![None, None]);
        assert_eq!(meta.ids, vec![None, None]);
        assert_eq!(meta.spans, vec![None, None]);
        assert_eq!(meta.gains.len(), 2);
    }

    /// Subsonic authorizes in the query string, so a resolve that only set
    /// headers had every server answer error 40.
    #[test]
    fn a_remote_key_resolves_through_the_source_that_authorizes_it() {
        let mut conn = rox_library::rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();

        let source = "test:resolve-authorizes";
        let mut row = album_row("sg-1", "Album");
        row.remote_url = "https://srv/rest/stream.view?id=sg-1&format=raw".into();
        store::upsert_source_rows(&mut conn, source, &[row]).unwrap();

        // Stands in for the Subsonic authorizer, which rewrites the URL.
        crate::sources_registry::install(
            source,
            Box::new(|remote| {
                remote.headers.push(("X-Test".into(), "1".into()));
                remote.url = format!("{}&t=tok&s=salt", remote.url);
            }),
        );

        let key = TrackKey {
            source: rox_library::cue::source_id(source),
            path: PathBuf::from("sg-1"),
            sub: 0,
        };
        let locators = resolve_locators(Some(&conn), std::slice::from_ref(&key));

        // A pruned row resolves to an empty URL, so the open fails on it.
        let pruned = TrackKey {
            path: PathBuf::from("gone"),
            ..key
        };
        let missing = resolve_locators(Some(&conn), &[pruned]);

        crate::sources_registry::forget(source);

        match &locators[0] {
            Locator::Remote(remote) => {
                assert_eq!(
                    remote.url,
                    "https://srv/rest/stream.view?id=sg-1&format=raw&t=tok&s=salt"
                );
                assert_eq!(
                    remote.headers,
                    vec![("X-Test".to_string(), "1".to_string())]
                );
            }
            other => panic!("a remote row resolved to {other:?}"),
        }

        match &missing[0] {
            Locator::Remote(remote) => assert!(remote.url.is_empty()),
            other => panic!("a pruned row resolved to {other:?}"),
        }
    }

    fn album_row(path: &str, album: &str) -> rox_library::TrackRow {
        rox_library::TrackRow {
            remote_url: String::new(),
            remote_live: false,
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            path: path.to_string(),
            sub: 0,
            cue: None,
            title: "Song".into(),
            artist: "X".into(),
            album_artist: "X".into(),
            album: album.to_string(),
            genre: String::new(),
            year: 0,
            disc_no: 0,
            track_no: 0,
            duration_ms: 180_000,
            codec: "flac".into(),
            bitrate_kbps: 0,
            sample_rate_hz: 0,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    fn other_album_row(path: &str) -> rox_library::TrackRow {
        album_row(path, "Other")
    }

    fn cue_row(path: &str, sub: u16, start_ms: u32, end_ms: Option<u32>) -> rox_library::TrackRow {
        rox_library::TrackRow {
            remote_url: String::new(),
            remote_live: false,
            sub,
            track_no: sub,
            cue: Some(rox_library::CueSlice {
                cue_path: "/m/Album/disc.cue".into(),
                span: Span { start_ms, end_ms },
            }),
            ..album_row(path, "Album")
        }
    }

    fn title(artist: &str, song: &str) -> IcyTitle {
        IcyTitle {
            artist: artist.into(),
            title: song.into(),
        }
    }

    #[test]
    fn the_song_clock_counts_the_song_not_the_listen() {
        assert_eq!(song_clock(930.0, Some(870.0)), 60.0);
        assert_eq!(song_clock(930.0, None), 930.0);
        assert_eq!(song_clock(869.5, Some(870.0)), 0.0);
    }

    /// A seek into a song reads its start off the buffer's title marks, not
    /// where the pump watched the turnover.
    #[test]
    fn a_seek_into_a_song_reads_the_song_off_the_buffer() {
        let shift = |song_secs| {
            Some(Shift {
                behind_secs: 300.0,
                window_secs: 600.0,
                cap_secs: 600.0,
                bytes_per_sec: 16_000.0,
                song_secs,
                song_len_secs: None,
            })
        };

        let start = song_start_of(930.0, shift(Some(45.0)), Some(870.0));
        assert_eq!(song_clock(930.0, start), 45.0);

        // Nothing announced behind the playhead: the pump's observation answers.
        let start = song_start_of(930.0, shift(None), Some(870.0));
        assert_eq!(song_clock(930.0, start), 60.0);

        // Neither, which is a file or a station that has said nothing.
        assert_eq!(song_start_of(930.0, None, None), None);
    }

    #[test]
    fn a_step_through_a_station_stays_inside_its_buffer() {
        let at = |behind_secs| Shift {
            behind_secs,
            window_secs: 600.0,
            cap_secs: 600.0,
            bytes_per_sec: 16_000.0,
            song_secs: None,
            song_len_secs: None,
        };

        assert_eq!(live_step_target(5.0, &at(300.0)), Some(295.0));
        assert_eq!(live_step_target(-5.0, &at(300.0)), Some(305.0));

        // Forward from closer than the step: live, not past it.
        assert_eq!(live_step_target(5.0, &at(3.0)), Some(0.0));

        // Back from deeper than the tape holds: the oldest second in it.
        assert_eq!(live_step_target(-5.0, &at(597.0)), Some(600.0));

        // Pressing into either end sends nothing.
        assert_eq!(live_step_target(5.0, &at(0.0)), None);
        assert_eq!(live_step_target(-5.0, &at(600.0)), None);

        // A fine step too small for the tape to land anywhere different.
        assert_eq!(live_step_target(-0.025, &at(300.0)), None);
    }

    /// A pause rejoin on the same song must not restart its clock.
    #[test]
    fn a_republished_title_does_not_restart_the_song() {
        let standing = SongStart {
            idx: 3,
            rev: 7,
            title: title("Boards of Canada", "Roygbiv"),
            at_secs: 870.0,
            from_start: true,
        };

        assert!(starts_new_song(None, 3, &standing.title));
        assert!(!starts_new_song(
            Some(&standing),
            3,
            &title("Boards of Canada", "Roygbiv")
        ));
        assert!(starts_new_song(
            Some(&standing),
            3,
            &title("Boards of Canada", "Olson")
        ));

        // Another entry is another stream.
        assert!(starts_new_song(
            Some(&standing),
            4,
            &title("Boards of Canada", "Roygbiv")
        ));
    }

    #[test]
    fn only_a_watched_turnover_counts_as_a_song_start() {
        let record = |from_start| SongStart {
            idx: 3,
            rev: 7,
            title: title("Boards of Canada", "Roygbiv"),
            at_secs: 870.0,
            from_start,
        };

        // The turnover we watched, with the playhead still in that song.
        assert!(song_heard_from_start(Some(&record(true)), true, 3, 900.0));

        // A mid-song join.
        assert!(!song_heard_from_start(Some(&record(false)), true, 3, 900.0));

        // Stepped back behind the turnover, into the song before it.
        assert!(!song_heard_from_start(Some(&record(true)), true, 3, 800.0));

        // Another entry's boundary, and no station playing at all.
        assert!(!song_heard_from_start(Some(&record(true)), true, 4, 900.0));
        assert!(!song_heard_from_start(Some(&record(true)), false, 3, 900.0));
        assert!(!song_heard_from_start(None, true, 3, 900.0));
    }

    fn station(url: &str) -> TrackKey {
        TrackKey {
            source: rox_library::cue::source_id(rox_library::stations::SOURCE),
            path: PathBuf::from(url),
            sub: 0,
        }
    }

    fn remote(url: &str, live: bool) -> Locator {
        Locator::Remote(rox_library::locator::Remote {
            url: url.into(),
            headers: Vec::new(),
            hint: String::new(),
            live,
        })
    }

    #[test]
    fn a_restore_seeks_to_everything_but_a_stream() {
        assert!(seeks_on_restore(&Locator::Local(PathBuf::from(
            "/m/a.flac"
        ))));
        assert!(seeks_on_restore(&remote(
            "https://srv/rest/stream?id=1",
            false
        )));
        assert!(!seeks_on_restore(&remote(
            "https://stream.example/live",
            true
        )));
    }

    #[test]
    fn only_an_all_station_batch_replaces_the_queue() {
        let live = station("https://stream.example/live");
        let jazz = station("https://stream.example/jazz");
        let file = TrackKey::from(PathBuf::from("/m/a.flac"));
        let served = TrackKey {
            source: rox_library::cue::source_id("subsonic:home"),
            path: PathBuf::from("tr-1042"),
            sub: 0,
        };

        assert!(replaces_queue(std::slice::from_ref(&live)));
        assert!(replaces_queue(&[live.clone(), jazz]));

        // Nothing to play in the queue's place, so nothing is replaced.
        assert!(!replaces_queue(&[]));

        assert!(!replaces_queue(std::slice::from_ref(&file)));
        assert!(!replaces_queue(&[served]));
        assert!(!replaces_queue(&[live.clone(), file.clone()]));
        assert!(!replaces_queue(&[file, live]));
    }
}
