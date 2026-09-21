//! The playback service entity: one running engine session behind the
//! playback contract (commands in over a channel, state out through shared
//! atomics). The PCM tap is drained by a headless pump task on a timer, not
//! by any render pass, so the audio views' feed keeps flowing no matter
//! which windows are drawing: popped-out panels, a zoomed dock, a
//! minimized main window. The player renders nothing itself; the transport
//! panels are the UI over this state.

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

// The clock formatters are with the rest of the readouts in rox-core now.
// Callers still get them through the player, where the clock is.
pub use rox_core::fmt::{fmt_time, fmt_time_padded};
// The loop mode is re-exported rather than plainly imported: it's the
// engine's type, but [`Player::loop_mode`] is where anything above the
// services layer meets it, and a crate that can call the method otherwise
// has no way to name what it got back.
pub use rox_playback::engine::LoopMode;

/// Pump cadence, roughly one video frame. The tap ring holds 16,384 samples
/// (about 170 ms at 48 kHz stereo), so a tick has an order of magnitude of
/// headroom before the callback's pushes start getting dropped.
const PUMP_INTERVAL: Duration = Duration::from_millis(16);

/// How many times a second a paused station repaints for its timeshift
/// growing. Fine enough that the bar slides on a short buffer, where a
/// quarter second is several pixels a step.
const PAUSED_SHIFT_STEPS: f64 = 30.0;

/// How long the similarity ordering will wait for a freshly started context
/// to publish its queue, as a number of tries and the gap between them. The
/// decode thread publishes first thing in `run`, so in practice this succeeds
/// on the first or second look; the ceiling is only there so a session that
/// never comes up can't leave a task waiting forever.
const QUEUE_WAIT_TRIES: usize = 40;
const QUEUE_WAIT_STEP: Duration = Duration::from_millis(25);

/// How long a track has to play before a skip counts as a fresh start rather
/// than part of a run. Long enough that skipping an outro you enjoyed doesn't
/// read as rejection, short enough that a track you actually sat through
/// hands the next press a fresh count instead of the pressure the last run
/// built up.
///
/// Settling reorders nothing by itself. The band a run widened to is already
/// in the queue and stays there, so the narrowing only shows in what the next
/// skip draws from. What does re-rank it is the next track ending on its own
/// (see [`reseed_at_boundary`]), and that keeps the band's tracks, it just
/// puts the nearest of them first.
const SKIP_SETTLE: Duration = Duration::from_secs(30);

/// The band a skip draws the next track from, as a count of the nearest
/// entries shuffled among themselves. One skip loosens to a handful, and each
/// one after multiplies, so a few in a row move out of a genre rather than
/// inching down it one track at a time. No skips at all means the strict
/// nearest.
const SKIP_BAND_BASE: usize = 4;
const SKIP_BAND_GROWTH: usize = 4;

/// How many of the nearest tracks a similar draw picks out of. Wide enough
/// that two presses in a row give different tracks, narrow enough that
/// everything in the band still sounds like the seed. The same handful the
/// skip band opens to, for the same reason.
const SIMILAR_BAND: usize = 8;

/// How far down the ranking the draw reads to fill that band. The band is
/// one track per song (see [`song::distinct`]), and a library that holds a
/// song seventeen times puts all seventeen at the front of the ranking, so
/// reading exactly `SIMILAR_BAND` would find one song and call it a
/// neighbourhood.
const SIMILAR_POOL: usize = SIMILAR_BAND * 16;

/// How many tracks apart the similarity ordering keeps two recordings of one
/// song. The queue is the listener's, so nothing is dropped from it: the
/// pile gets spread through the tail instead. Wide enough that a pile never
/// reads as a run, short enough that the tail still sorts by sound.
const SONG_SPACING: usize = 25;

/// A random index below `len`, off the std hasher's per-process random
/// keys; picking a track does not need a rand dependency.
fn random_index(len: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    let hash = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (hash % len as u64) as usize
}

/// The ids the Random button draws from: the view playback started in while
/// it still holds anything, the whole library otherwise. Random is the same
/// question continuation asks (ADR 17), so it reads the same scope: press it
/// inside a playlist and it stays in the playlist.
fn random_pool<'a>(scope: &'a continuation::Scope, library: &'a [i64]) -> &'a [i64] {
    match scope {
        continuation::Scope::View(ids) if !ids.is_empty() => ids,
        _ => library,
    }
}

/// How many tracks the player remembers across session starts, for the
/// draws' no-repeat promise. A queue's worth: enough that pressing Play
/// Similar over and over moves through a neighbourhood instead of bouncing between
/// two tracks, small enough that a big library never runs dry of fresh draws.
const HEARD_CAP: usize = QUEUE_CAP;

/// Fold a session pool into the heard ring, newest last, deduped so a track
/// held again moves to the young end rather than aging out from where it
/// first entered, capped by evicting the oldest.
///
/// This exists because the by-hand draws start sessions, and starting a
/// session replaces the pool their repeat-guard reads: without a memory that
/// persists across the swap, the second press of Play Similar has already forgotten
/// the track the first press left, and the two bounce between each other's
/// bands.
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

/// Where in `pool` a random draw falls: on anything the session hasn't held,
/// and only anywhere at all once it has held the whole pool. The same promise
/// every continuation provider makes (ADR 17), made here because the Random
/// button is the same question asked by hand, and the same wrap at the end of
/// it: an exhausted pool opens back up rather than eating the press.
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

/// One random entry of `pool` with the run it's part of, and where in that
/// run the draw fell. The draw is where playback starts, and the entries
/// around it are what keep it going: Next continues down the rest of the
/// album and the list behind it, Prev goes back. A press picks a spot and
/// keeps playing, the same as double clicking a row.
///
/// The picked spot avoids `seen` (see [`draw_at`]); the run around it
/// doesn't, because the run is context, and an album the draw fell inside
/// should read whole rather than with the heard tracks cut out.
///
/// Bounded like a double click in a big view (`QUEUE_CAP`), with a share of
/// the budget kept behind the draw for history. None when the pool is empty or
/// the id it picked has no file behind it any more.
///
/// Keys rather than paths, so a draw on a cue track takes that track's span
/// instead of the whole image it belongs to.
fn draw_run(
    library: &Library,
    pool: &[i64],
    seen: &HashSet<i64>,
) -> Option<(Vec<TrackKey>, usize)> {
    let at = draw_at(pool, seen)?;
    let drawn = library.keys_for(&[pool[at]]).ok()?.pop()?;
    let (lo, hi) = run_window(at, pool.len());
    let keys = library.keys_for(&pool[lo..hi]).ok()?;
    // Ids whose files have left the library drop out of the resolve, so the
    // cursor is found by the key rather than counted off the pool.
    let start = keys.iter().position(|key| *key == drawn)?;
    Some((keys, start))
}

/// The similar band as bare ids with the session's plays taken out, whole
/// again once every neighbour has been heard. [`draw_at`]'s wrap for the
/// other button mode: the band stays the `SIMILAR_BAND` nearest rather than
/// widening past them, because everything in it has to keep sounding like
/// the seed, and a heard-out neighbourhood repeating is better than a press
/// that wanders somewhere that doesn't.
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

/// The ranking thinned to one track per song, nearest first, cut to
/// `SIMILAR_BAND`: the seed's own song is out, so is anything the session has
/// already heard a version of, and no two entries left are the same song as
/// each other.
///
/// A library that holds one song many times over hands the acoustic ranking
/// a pile of tracks that are each other's nearest neighbours, because by
/// sound they really are the same track. Play Similar reading straight off
/// the top of that would answer with the song it was asked about. The tag
/// identity (see [`song`]) is the only thing that can tell them apart.
///
/// The whole ranking back means nothing survived, which is a neighbourhood
/// of one song: the caller plays it rather than eating the press.
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

/// The song identity of the seed and of every ranked queue entry, keyed by
/// entry id so the ordering can ask about a queue rather than about rows.
///
/// Entries whose row has no usable artist or title come back absent, which
/// reads downstream as "nothing's duplicate": an untagged file keeps its
/// place in the ranking rather than being spaced away from tracks it has no
/// established relationship with.
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

/// The slice of a `len` entry pool a draw at `at` plays inside: at most
/// `QUEUE_CAP` tracks, half the budget behind the draw so Prev has somewhere
/// to go, sliding forward against the end so the window stays full. A pool
/// under the cap comes back whole.
fn run_window(at: usize, len: usize) -> (usize, usize) {
    let lo = at
        .saturating_sub(QUEUE_CAP / 2)
        .min(len.saturating_sub(QUEUE_CAP));
    (lo, (lo + QUEUE_CAP).min(len))
}

/// Whether the queue has run close enough to its end to ask for more
/// (ADR 17): `upcoming` is how many tracks are ahead of the audible one,
/// measured against the floor the trigger insists on keeping.
///
/// Loop is the whole of the suppression rule, and it's here rather than at
/// the call site because it's part of the same decision: loop is the user
/// saying remain here, which narrows the selection range to the list that
/// already exists.
fn queue_running_dry(upcoming: usize, loop_mode: LoopMode) -> bool {
    loop_mode == LoopMode::Off && upcoming <= continuation::FLOOR
}

/// Whether continuation should be extending a session in this state
/// (ADR 17). A paused queue doesn't grow, which keeps the launch
/// restore from growing a queue nobody has pressed play on yet; a queue that
/// played through to its end still reads as playing, which is how an ended
/// session gets woken by the batch appended behind it.
///
/// An armed stop-after is the one thing that pauses on its own, and it means
/// stop, so the queue stays as it is until the listener says otherwise. It
/// stays armed after the stop, so this keeps saying no until they clear
/// it, which is the same stickiness the transport button has.
fn continuation_wanted(playing: bool, stop_after: bool) -> bool {
    playing && !stop_after
}

/// Whether engaging the Similar order should ask for a batch right now rather
/// than wait for the queue to run down to the floor.
///
/// Similar is the radio draw (see [`continuation::provider`]), so turning it
/// on is turning radio on, and a listener who does that with ten browse-order
/// tracks still queued wants the radio ahead of those ten rather than twenty
/// minutes behind them. The ordering alone can't give them that: it sorts what
/// is queued, and what's queued is the wrong music.
///
/// Off outranks it. A queue told to end doesn't start growing because the
/// shuffle order changed under it, which is the same call `provider` makes
/// when it hands Off a None whatever the order says. A paused queue and an
/// armed stop-after say no here for the reasons they say no to the pump.
fn similar_draw_now(
    mode: continuation::Mode,
    similar: bool,
    playing: bool,
    stop_after: bool,
) -> bool {
    similar && mode != continuation::Mode::Off && continuation_wanted(playing, stop_after)
}

/// Whether the pump owes the queue a fresh ranking now the audible track has
/// moved on to pool index `audible`.
///
/// The similarity order ranks the whole tail against one track, so a batch
/// played straight through is walking down an answer to a question about a
/// track several songs back. Re-seeding at the boundary is what keeps the
/// ordering about what's actually playing.
///
/// `skipped` is a skip having already paid for this boundary: `next` re-seeds
/// on where it lands, at a band its run widened, and the pump sees that same
/// landing a tick later. Re-ranking it again here would be a second pass at a
/// band of one, which throws the widening away and undoes the steering.
///
/// No marker yet means nothing is owed: that's a session that has only just
/// come up, and starting one orders its own tail.
fn reseed_at_boundary(similar: bool, skipped: bool, audible: usize, last: Option<usize>) -> bool {
    similar && !skipped && last.is_some_and(|at| at != audible)
}

/// Where a skip's re-ranking stands against the boundary it caused.
///
/// The two race, and the boundary usually wins: the ranking waits for the
/// engine to adopt the track the skip landed on and then scores the library
/// against it, while the boundary check runs off a 16 ms pump. So the claim
/// can't just be "the ranking went out", it has to be made when the skip
/// fires and released when the ranking is done one way or the other. Every
/// give-up inside the ranking (nothing to seed on, nothing the library
/// scored, the mode changed under it) ends without touching the queue, and a
/// claim left standing over one of those means the boundary declined to
/// re-seed for a ranking that never happened, which leaves the tail ordered
/// against a track the listener has already left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SkipReseed {
    /// No skip ranking outstanding, so boundaries re-seed themselves.
    Idle,
    /// A skip's ranking is being computed. It owns the next boundary
    /// whichever way round the two arrive; `passed` records that the boundary
    /// got there first.
    InFlight { passed: bool },
    /// The ranking went out ahead of the boundary. The next boundary spends
    /// it and ranks nothing itself.
    Ready,
}

impl SkipReseed {
    /// A boundary just went past. Returns the new claim and whether the
    /// boundary is already paid for.
    fn spend(self) -> (Self, bool) {
        match self {
            Self::Idle => (Self::Idle, false),
            Self::InFlight { .. } => (Self::InFlight { passed: true }, true),
            Self::Ready => (Self::Idle, true),
        }
    }

    /// The skip's ranking reached the engine. It only has a boundary left to
    /// pay for if one hasn't gone by already.
    fn landed(self) -> Self {
        match self {
            Self::InFlight { passed: false } => Self::Ready,
            _ => Self::Idle,
        }
    }

    /// The skip's ranking gave up. Returns the released claim and whether a
    /// boundary was held off for it, in which case that re-seed is still owed.
    fn abandon(self) -> (Self, bool) {
        (Self::Idle, self == Self::InFlight { passed: true })
    }
}

/// The band `skips` consecutive skips earns.
fn skip_band(skips: u32) -> usize {
    if skips == 0 {
        return 1;
    }
    SKIP_BAND_BASE.saturating_mul(SKIP_BAND_GROWTH.saturating_pow(skips - 1))
}

/// One running engine: decode thread, output stream, and the UI's side of
/// the PCM tap. Dropping it sends Quit and tears the stream down.
struct Session {
    shared: Arc<Shared>,
    tx: mpsc::Sender<Cmd>,
    tap: Consumer<f32>,
    /// The live output stream, held so it keeps playing and dropped to give
    /// the device back. An Option because a device that faulted is handed
    /// back before its replacement is opened: a backend asked for the same
    /// card with the dead handle still on it answers busy, which is the one
    /// answer the recovery can't use.
    stream: Option<Box<dyn output::OutputStream>>,
    device_rate: u32,
    /// What the output layer actually got, as opposed to what was asked
    /// for. The Audio page reads this, and the rate follow compares against
    /// it, so neither is going on the setting's word.
    negotiated: output::Negotiated,
    /// The queued keys, kept so the views can resolve the playing track back
    /// to its file. Keys rather than paths because the engine's pool is
    /// paths: two cue tracks of one image are the same path twice down there,
    /// and this mirror is the only thing left that can tell them apart.
    queue: Vec<TrackKey>,
    /// The ReplayGain tags handed to the engine, in the same pool order as
    /// `queue`. Kept so the status readout can say what the playing file is
    /// actually being levelled by rather than what the setting is set to.
    gains: Vec<gain::ReplayGain>,
    /// Which pool entries are live streams, again in pool order. Off the
    /// locator rather than the key's source, because live is a property of
    /// what the row points at: a station is live, and a source could
    /// perfectly well serve a fixed-length file out of the same source
    /// string. The surfaces that draw a timeline read this to know there
    /// isn't one.
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
        // Seed the session with the persisted playback state: volume is
        // stored in the shared atomics before the stream opens, the loop and
        // shuffle modes queue on the channel so the engine picks them up
        // first thing.
        shared
            .volume_bits
            .store(volume.to_bits(), Ordering::Relaxed);
        let out = output::open(&output, &shared)?;
        let device_rate = out.sample_rate;
        let (tx, rx) = mpsc::channel::<Cmd>();
        let _ = tx.send(Cmd::SetLoop(loop_mode));
        // A fresh context seeds the shuffle mode so the engine scrambles the
        // starting order; a restore passes None and skips it, since the saved
        // order already is the shuffled order and re-sending would reshuffle
        // the not-yet-played tail out from under the saved queue.
        if let Some(on) = shuffle {
            let _ = tx.send(Cmd::SetShuffle(on));
        }
        // An armed stop-after is sent to the fresh session, so queueing a
        // new context does not silently disarm it.
        if stop_after {
            let _ = tx.send(Cmd::SetStopAfter(true));
        }
        // The launch restore's pause and seek. The pause is a store rather
        // than a queued command, because the engine reads this flag before
        // it reads the channel: its first open happens at the top of `run`,
        // and a station opened there would be sitting on a live socket
        // through a pause that hadn't been delivered yet. With the flag
        // already down, that open sees a paused session and parks without
        // connecting. Everything else opens as it always did, silent because
        // the callback is.
        //
        // The seek goes where there's somewhere to land. A stream plays from
        // wherever the broadcast is now, so the saved seconds name a moment
        // that has been and gone, and seeking to them spends a flush to
        // arrive back at the live edge.
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
        // The fade settings are sent ahead of the first decode too, so a
        // session that starts on a skip already has them at its first
        // boundary.
        let _ = tx.send(Cmd::SetCrossfade {
            secs: crossfade.0,
            albums: crossfade.1,
        });
        // And the leveling rule, so the first track opens at the volume the
        // rest of the session will play at rather than jumping once the
        // setting catches up.
        let _ = tx.send(Cmd::SetGainRule(rule));
        // The EQ joins this session's processing chain (ADR 19). Queued
        // here with the rest, so it's in place before the first buffer
        // rather than a few chunks late. It's the only thing this channel
        // ever sends for the chain: the bands are atomics on the shared
        // handle, so every later turn of a knob is a store.
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

/// Which of these locators are live streams, in the order they were handed
/// over. One pass at insert time rather than a lookup per frame: the seek
/// strip and the waveform ask on every pump tick, and the answer can only
/// change when the pool does.
fn live_flags(locators: &[Locator]) -> Vec<bool> {
    locators
        .iter()
        .map(|l| matches!(l, Locator::Remote(remote) if remote.live))
        .collect()
}

/// Whether a restore's saved position is one to seek to, asked of the
/// locator the session comes up on. A live stream has no position: it plays
/// from wherever the broadcast is now, and the seconds written at close name
/// a moment in it that is gone. Everything else takes the seek, a remote file
/// included, since that's a file that happens to arrive over the wire.
fn seeks_on_restore(start: &Locator) -> bool {
    !matches!(start, Locator::Remote(remote) if remote.live)
}

/// Whether this batch takes the queue's place instead of joining it, which
/// is the one thing a station does differently from a track. A stream has no
/// end, so everything queued behind one sits there for as long as you listen,
/// and someone who put a station on has stopped listening to a list anyway.
///
/// Every key has to be a station for the rule to bite. A selection that mixes
/// one in with files is a list, and a list plays the way lists have always
/// played; an empty batch replaces nothing, since there's nothing to play in
/// the queue's place.
fn replaces_queue(keys: &[TrackKey]) -> bool {
    !keys.is_empty() && keys.iter().all(|key| key.origin() == Origin::Radio)
}

/// The library lookup for a batch of keys on their way into the queue, one
/// entry per key in the order they were asked about.
struct QueueMeta {
    groups: Vec<Option<u64>>,
    gains: Vec<gain::ReplayGain>,
    ids: Vec<Option<i64>>,
    /// The slice of the file each key plays, None for a plain file. The
    /// engine takes these beside the paths, which is the whole of how a cue
    /// track differs from the image it's part of once it's in the pool.
    spans: Vec<Option<Span>>,
}

/// Resolve a batch of keys against the library, or return defaults for all of
/// them when there is no database to ask. Split out of the player so the
/// lookup can be tested against a real store without a running session.
///
/// A key the library doesn't hold resolves to ungrouped, untagged, and
/// unidentified, which plays fine: a file dropped in from outside still has
/// samples, it just has nothing to level or splice by.
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
        // The path guard stays: a remote key's path is the source's own id
        // and is always valid UTF-8, but a local one comes off the
        // filesystem and still might not be.
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

/// Where each of these keys plays from, which is what the engine opens. A
/// local key is its own answer. Anything else has to go back to the row,
/// since the stream URL and the live flag are stored and what authorizes
/// the request is not: that comes off the registry the app fills at
/// startup, which both sets the headers and finishes the URL.
///
/// A remote key whose row has gone (a source pruned it mid-queue) answers
/// with an empty URL rather than a path, so the open fails instead of
/// reading some file that happens to sit where the id reads like a path.
fn resolve_locators(
    conn: Option<&rox_library::rusqlite::Connection>,
    keys: &[TrackKey],
) -> Vec<Locator> {
    keys.iter()
        .map(|key| {
            if key.is_local() {
                return Locator::Local(key.path.clone());
            }

            // Through the sub-aware lookup rather than the path one, so a
            // source that splits one reference into subsongs resolves the
            // row that's actually queued.
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

/// A snapshot of the playing track for the audio views: which file and
/// where the position clock is. The tap says whether audio is actually
/// moving, so the views read that from the feed instead.
#[derive(Clone)]
pub struct NowPlaying {
    /// Which file, and which subsong of it. Two cue tracks of one image are
    /// the same path with different subs, so anything naming the track (the
    /// info panel, the scrobbler, the media session) has to read the whole
    /// key rather than just the path.
    pub key: TrackKey,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    /// Pool index of the audible track, off the position clock. The queue
    /// resolver matches entries on this rather than the path, so a file that
    /// appears in the order more than once resolves to the occurrence playing now.
    pub audible_idx: usize,
    /// A stream with no end: a station rather than a file. Everything that
    /// draws a timeline is wrong for one of these, so the seek strip and
    /// the waveform branch on this rather than on a zero duration, which a
    /// file can also have for a moment while it opens.
    pub live: bool,
    /// Where the station's clock stood when its current song started, for
    /// the surfaces that count the song rather than the listen. None off a
    /// station and until the stream announces a title, since an ICY title
    /// is the only song boundary a broadcast has and before the first one
    /// there is nothing to count from. Subtract it from `position_secs`,
    /// or let [`song_clock`] do it.
    ///
    /// Read off the buffer's own title marks wherever they reach: the
    /// listen clock runs forward through a step back through the buffer,
    /// and the song clock has to follow the playhead into whatever song it
    /// landed in rather than starting that song again at zero.
    pub song_start_secs: Option<f64>,
    /// The song under the playhead began while this listen was running, so
    /// `song_start_secs` is a real boundary and not just where the stream
    /// opened. False for the song a mid-song join lands in: a station
    /// announces what is playing the moment you connect, which says what
    /// the song is and nothing about how far into it you are. Anything
    /// timing against the song rather than the listen (a synced lyric
    /// sheet) has to have this before it trusts the clock.
    ///
    /// Also false once the cursor steps back behind the turnover the pump
    /// recorded, since that lands in an earlier song whose start may have
    /// rolled off the back of the tape.
    pub song_from_start: bool,
    /// Which kind of source it came from, for the surfaces that mark one.
    pub origin: Origin,
    /// Where the stream stands, for the entries that are one: opening,
    /// playing, reconnecting through a drop, or gone. None for a local file,
    /// which has none of those states to be in.
    pub stream: Option<StreamState>,
    /// How far behind the broadcast this is playing, and how much of the
    /// broadcast is held. None for anything that isn't a live stream, and
    /// for one whose tape hasn't taken a byte yet.
    ///
    /// The distance is what a pause builds up and what [`Player::seek_live`]
    /// moves through; the window is how far back it can go. Both keep
    /// growing while a pause holds the cursor still, which is why a surface
    /// drawing them repaints through a pause.
    pub shift: Option<Shift>,
}

impl NowPlaying {
    /// The playing file, for the callers that genuinely want a path (cover
    /// lookups, a decode window, a filename fallback) rather than an
    /// identity. None when what's playing isn't a file: a remote track's
    /// path is the source's own id, and handing that out as a path would
    /// have every one of those callers reach for a file that isn't there.
    /// Each decides for itself what to do without one.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.key.is_local().then_some(self.key.path.as_path())
    }
}

/// Where the song under the playhead began, on the same clock
/// `position_secs` runs on, so [`song_clock`] stays the one rule and only
/// its input changes.
///
/// The buffer's answer wins wherever it has one. It knows the byte each
/// title was announced at, so it can say how far into a song the playhead
/// is wherever the playhead has been moved to: a step back into the middle
/// of a song reads as the middle of it, and the listen clock carrying on
/// forward doesn't drag the song clock with it.
///
/// `observed` is what the pump saw as the titles went past, which is the
/// answer for the seconds after a connect, before any title sits behind the
/// playhead for the buffer to measure from.
fn song_start_of(position_secs: f64, shift: Option<Shift>, observed: Option<f64>) -> Option<f64> {
    shift
        .and_then(|shift| shift.song_secs)
        .map(|into| position_secs - into)
        .or(observed)
}

/// What an elapsed clock over a [`NowPlaying`] reads: time into the song
/// once the station has named one, time into the listen before that, and
/// the position itself for everything that isn't a station.
///
/// A start later than the position is a real state rather than a bug. The
/// pump records the start off its own read of the clock, so a seek or a
/// rejoin can leave the position behind it for a tick; flooring at zero
/// keeps that out of the readout.
pub fn song_clock(position_secs: f64, song_start_secs: Option<f64>) -> f64 {
    match song_start_secs {
        Some(start) => (position_secs - start).max(0.0),
        None => position_secs,
    }
}

/// Where a relative seek of `delta` seconds lands in a station's buffer,
/// as a distance behind the live edge. Positive `delta` is forward, which
/// on this timeline means closing the distance.
///
/// The ends are the whole point. Forward stops at the live edge, since the
/// broadcast hasn't sent what's past it, and backward stops at the oldest
/// second the tape still holds. Both hold rather than wrapping or running
/// off, so leaning on an arrow key parks the listener at the end it points
/// at.
///
/// None where the step wouldn't move the cursor anywhere the tape can tell
/// apart, which is what pressing against either end comes to. A live seek
/// costs a rebuilt decoder and a cut in the audio, so sending those would
/// be one hole in the broadcast per press, every one of them landing back
/// where the listener already was. The edge snap is the threshold because
/// it's already the closest the tape will put a cursor to the edge.
fn live_step_target(delta: f64, shift: &Shift) -> Option<f64> {
    let behind = shift.behind_secs;
    let target = (behind - delta).clamp(0.0, shift.window_secs.max(0.0));

    ((target - behind).abs() >= rox_playback::LIVE_EDGE_SNAP_SECS).then_some(target)
}

/// When the audible station last moved to a new song, as the pump saw it.
///
/// The title is here beside the revision because the revision is global:
/// any entry publishing moves it, so an unchanged one proves nothing
/// published while a matching title proves this entry is still on the song
/// it was. The revision is what keeps the common tick off the title lock.
struct SongStart {
    /// The pool entry the song was observed on.
    idx: usize,
    /// The title revision at the observation.
    rev: u64,
    /// The title that was standing then.
    title: IcyTitle,
    /// The station's elapsed at the turnover, the number clocks subtract.
    at_secs: f64,
    /// The turnover was one this listen watched happen, rather than the
    /// title the stream was already carrying when it opened. See
    /// [`NowPlaying::song_from_start`].
    from_start: bool,
}

/// Whether the song under the playhead is one this listen heard begin,
/// which is what [`NowPlaying::song_from_start`] carries and the only
/// question a synced lyric sheet on a station has to answer.
///
/// Three ways to say no. Nothing live is audible, or the record belongs to
/// another entry, so there is no station boundary here at all. The record
/// is the title the stream was already carrying when it opened, which
/// names the song a mid-song join landed in and says nothing about how far
/// into it. And the cursor has been stepped back behind the turnover, into
/// a song whose own start may have rolled off the back of the tape.
fn song_heard_from_start(last: Option<&SongStart>, live: bool, idx: usize, secs: f64) -> bool {
    last.is_some_and(|last| live && last.idx == idx && last.from_start && secs >= last.at_secs)
}

/// Whether a title just read off an entry is a new song rather than the one
/// the record already stands on. A pause rejoin republishes the title it
/// hung up on, and this is the question that keeps the resumed listener's
/// clock where they left it.
fn starts_new_song(last: Option<&SongStart>, idx: usize, title: &IcyTitle) -> bool {
    match last {
        Some(last) => last.idx != idx || &last.title != title,
        None => true,
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Nothing is more waited-on than this: the session going away means
        // something is waiting on the decode thread to end, and a station
        // stuck in its reconnect schedule is the one thing that would make it
        // take seconds.
        self.shared.interrupt();
        let _ = self.tx.send(Cmd::Quit);
    }
}

/// Whether a command is one the listener is standing there waiting for the
/// answer to, which is the question the engine's interrupt flag exists to
/// carry: pause, a move through the queue, a play now, a queue edit they just
/// made, and the session ending.
///
/// It matters for exactly one situation. A station that has dropped leaves
/// the decode thread inside a read, waiting out a reconnect backoff, and
/// nothing queued behind it is read until that returns. Saying so lets the
/// retries be abandoned instead of served in full. Everything else here
/// (volume, the loop mode, a gain rule, a crossfade setting) either lands on
/// the next pass or isn't something anyone watches for, and none of it is
/// worth cutting a station's recovery short for.
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

/// How finely a crossfade's progress is reported. The transport draws the
/// fade as a sweep a couple of dozen pixels wide, so this is the resolution
/// past which nothing on screen would move; it's also what keeps the fade in
/// [`PlayerView`], since a panel then wakes once per step instead of on
/// every pump tick for the whole window.
const FADE_STEPS: u8 = 64;

/// A crossfade in progress, as the transport sees it: how far along, in
/// [`FADE_STEPS`]ths, and which way the skip that started it went.
#[derive(Clone, Copy, PartialEq)]
pub struct FadeView {
    pub step: u8,
    /// The fade came from a Previous. A boundary fade and a Next both read
    /// as forward.
    pub back: bool,
}

impl FadeView {
    /// Progress through the window, 0 to 1.
    pub fn progress(&self) -> f32 {
        self.step as f32 / FADE_STEPS as f32
    }
}

/// Where the A-B command stands. The three-step cycle reads off this: no
/// marks, the first one down, or the section repeating. Both seconds are
/// track-relative, the same clock the seek strip draws.
///
/// The looping case comes from the engine's published snapshot (ADR 16);
/// only the half-marked step lives on the player, because at that point
/// there's no loop for the engine to own yet.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum AbState {
    #[default]
    Off,
    ASet(f64),
    Looping(f64, f64),
}

/// What one press of the A-B command does from where the cycle stands.
#[derive(Clone, Copy, PartialEq, Debug)]
enum AbStep {
    /// Hold A on the player and wait for B; there's no loop to send yet.
    Pending(f64),
    /// Hand the engine a section, or None to end the one it's playing.
    Send(Option<(f64, f64)>),
    /// A double-tap: B landed on top of A. Drop the half-marked step
    /// rather than sending a section the engine would refuse anyway.
    Nothing,
}

/// The three-press cycle as arithmetic, away from the session and the
/// notify around it: mark A, mark B and play the section, clear.
fn ab_step(state: AbState, pos: f64) -> AbStep {
    match state {
        AbState::Looping(..) => AbStep::Send(None),
        AbState::ASet(a) if pos - a >= engine::AB_MIN_SECS => AbStep::Send(Some((a, pos))),
        AbState::ASet(_) => AbStep::Nothing,
        AbState::Off => AbStep::Pending(pos),
    }
}

/// A section named outright, both ends at once, as the socket sets it:
/// the ends put in order so a caller that wrote them backwards still gets
/// the section it meant, and anything shorter than the engine's floor
/// refused here rather than sent to die quietly. Negative or non-finite
/// seconds are refused too, since there's no position they could mean.
fn ab_section(a: f64, b: f64) -> Option<(f64, f64)> {
    if !a.is_finite() || !b.is_finite() || a < 0.0 || b < 0.0 {
        return None;
    }
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    (b - a >= engine::AB_MIN_SECS).then_some((a, b))
}

/// What a pump tick owes the sleep timer.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SleepStep {
    /// No timer set, or the moment it named hasn't come.
    Nothing,
    /// Fire it and arm stop-after on the way through.
    Arm,
    /// Fire it, but leave stop-after where it is: it's already armed, and
    /// the only way to arm it is a toggle, which would turn it off.
    Clear,
}

/// Whether a tick fires the sleep timer, and what it owes stop-after when
/// it does. Pulled out of the pump so the decision can be tested without a
/// session or a context behind it, the same way [`ab_step`] is.
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

/// The player's discrete state: everything the controls and info panels
/// draw that changes on a user action or a track change, never on the bare
/// position tick. The position clock is deliberately left out, so a panel
/// gating on this does not wake for it. See [`observe_view`].
#[derive(Clone, PartialEq)]
pub struct PlayerView {
    /// The playing track, key and all: a bare path can't tell two cue tracks
    /// of one image apart, so a gated observer would miss the boundary
    /// between them and every readout would keep the first one's title.
    pub track: Option<TrackKey>,
    pub duration_secs: Option<f64>,
    pub playing: bool,
    pub active: bool,
    pub ended: bool,
    pub loop_mode: LoopMode,
    pub shuffle: bool,
    /// Which order shuffle is in. Here beside the flag because the transport
    /// button's glyph follows it, and the Playback page can move it from
    /// another window; without it the gated observer sees an unchanged view
    /// and the strip keeps drawing the old order's icon.
    pub shuffle_mode: ShuffleMode,
    /// Which strategy refills a dry queue. Here rather than read straight
    /// off the settings by whoever draws it, so the transport's gated
    /// observer wakes when the mode menu changes it.
    pub continuation: continuation::Mode,
    /// How long a boundary fade runs, zero for off. Here for the same
    /// reason the mode above is: the transport's crossfade button draws
    /// from it, and the Audio page's scrub can move it under the panel.
    pub crossfade_secs: f32,
    pub stop_after: bool,
    /// Where the A-B command has got to: nothing marked, A down and
    /// waiting for B, or a section repeating. Here rather than read off
    /// the shared state by whoever draws it, so the transport's gated
    /// observer wakes on each press.
    pub ab: AbState,
    /// How long the sleep timer has left, in whole seconds, None when no
    /// timer is set. Here rather than read off the player by whoever draws
    /// it so the menu's gated observer wakes when the timer is set or
    /// cancelled; the countdown itself doesn't need a repaint, since the
    /// only thing that reads it is a menu being opened.
    pub sleep_remaining_secs: Option<u64>,
    pub muted: bool,
    pub volume: f32,
    pub error: Option<SharedString>,
    /// The crossfade the ear is in, quantized so a gated observer wakes
    /// once per visible step. None the rest of the time, which is a
    /// comparison that costs nothing on a settled session.
    pub fade: Option<FadeView>,
    /// The station-title revision. A stream moving to the next song changes
    /// nothing else here, so without it a panel gated on this view sits
    /// still through a turnover and shows the new song whenever something
    /// unrelated happens to repaint. Zero with no session.
    pub title_rev: u64,
}

/// What output actually ended up doing, for the Audio page to state instead
/// of echoing the settings back. ADR 19's bit-perfect claim rests on three
/// conditions, and the two this can report are here: which mode is
/// running, and whether the device rate matches the file's.
#[derive(Clone, PartialEq)]
pub struct OutputStatus {
    pub negotiated: Negotiated,
    /// The playing file's own rate. None before a track has opened, which
    /// is also the only accurate answer then.
    pub source_rate: Option<u32>,
    /// What ReplayGain is actually doing to the playing file, in dB. None
    /// when the samples reach the ring untouched: leveling off, or on with
    /// nothing to apply, where an untagged file with no fallback
    /// set ends up. Not a fault when it is set, but it's processing, and
    /// the readout would be claiming the file's own samples without saying
    /// so.
    pub leveling_db: Option<f32>,
}

impl OutputStatus {
    /// The lines under the readout's headline, in the register the surface
    /// asks for. Expanded gives each earned fact a sentence of its own, the
    /// settings page's wording; compact folds them into one comma list, the
    /// headline's own style, so a docked panel stays two lines tall.
    /// `confirm_rate` is the all-clear toggle: whether a rate nothing is
    /// converting still earns a mention, since a conversion is worth stating
    /// either way. Nothing here is derived from the settings: the fallback line
    /// only appears because a backend reported one, and the rate line
    /// compares the device against the file rather than against what was
    /// asked for.
    /// A ReplayGain adjustment the way the format writes it: the sign
    /// always shown, one decimal, and the decimal mark the reader's own.
    /// The sign is spelled out here because a number formatter has no
    /// setting for "always", and a gain without one reads as a level.
    fn signed_db(db: f32) -> String {
        let sign = if db < 0.0 { "-" } else { "+" };
        format!(
            "{sign}{}",
            rox_i18n::format::format_float(f64::from(db.abs()), 1)
        )
    }

    pub fn lines(&self, expanded: bool, confirm_rate: bool) -> Vec<SharedString> {
        let resampling = self
            .source_rate
            .is_some_and(|source| source != self.negotiated.sample_rate);
        let mut lines: Vec<SharedString> = Vec::new();
        // The fallback line is the whole reason a failed claim isn't a
        // mystery: the toggle stays on, and this says why it isn't what
        // you're hearing. Broken enough to keep a line of its own in both
        // registers.
        if let Some(why) = &self.negotiated.fallback {
            lines.push(rox_i18n::t!(
                "output-fell-back-to-shared",
                why = why.to_string()
            ));
        }
        // Leveling multiplies the source on its way to the ring (ADR 19),
        // so it outranks the rate: whatever the rates say, this is the
        // one that decides whether these are the file's own samples. Only
        // when something is actually applied, so an untagged file with the
        // fallback at zero says nothing.
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
    /// Outlives sessions: the audio views hold clones and keep reading
    /// while queues come and go.
    feed: Arc<AudioFeed>,
    /// Persisted playback state; its volume and loop mode are the source of
    /// truth, sessions are seeded from them.
    settings: Settings,
    /// The headless frame driver: drains the tap into the feed on a timer
    /// while a session runs. Replaced (and the old one cancelled) whenever a
    /// new session starts.
    pump: Option<Task<()>>,
    /// Debounce generation for the volume persist; only the last edit in a
    /// burst writes the settings file. See [`Self::persist_volume_soon`].
    persist_gen: u64,
    /// Read connection to the library for the insert-time lookup: the
    /// engine sees bare paths, so the player resolves each path's album
    /// group and ReplayGain here before handing it over. Opened lazily on
    /// the first play; WAL keeps it current alongside the catalog's
    /// connections. None until then, or when the library has no database.
    meta_conn: Option<rox_library::rusqlite::Connection>,
    /// Stop at the end of the playing track, next one cued and paused.
    /// Deliberately not persisted: an armed stop that persisted across a restart
    /// would read as a broken player days later.
    stop_after: bool,
    /// The A-B command's half-marked step: A is down, B isn't, so there's
    /// no loop yet for the engine to hold. Pinned to the track it was
    /// marked on, so a skip away and back doesn't hand the next press a
    /// mark from a different song. Everything past this step lives on the
    /// engine and is read back, never mirrored.
    ab_pending_a: Option<(TrackKey, f64)>,
    /// When the sleep timer arms stop-after, None when it's off. Session
    /// state on purpose, like the stop it arms: a timer that survived a
    /// restart would end a listening session nobody asked it to.
    sleep: Option<Instant>,
    /// Skips in a row under the similarity mode, and when the last one
    /// happened. Together they widen the band the radio draws from: skip
    /// repeatedly and it goes further from the seed each time, so a few
    /// presses move out of a genre. Listening for [`SKIP_SETTLE`] without
    /// skipping counts as settling and the next skip starts from narrow
    /// again. Session-local, like the stop above: yesterday's impatience
    /// should not steer today's radio.
    similar_skips: u32,
    last_skip: Option<Instant>,
    /// The pool index the similarity ordering was last seeded on, and whether
    /// a skip has already paid for the next boundary. Together they are how
    /// the pump tells a track that ended on its own from one the listener
    /// skipped past: the first owes the queue a fresh ranking against what's
    /// playing now, the second was ranked on the way through `next` and must
    /// not be ranked again at a narrower band. See [`reseed_at_boundary`]
    /// and [`SkipReseed`].
    reseeded_at: Option<usize>,
    skip_reseed: SkipReseed,
    /// The rate the next stream asks for, exclusive mode's rate follow
    /// (ADR 19). Holds whatever the last stream negotiated, so a rebuild
    /// comes back up on the rate it went down on instead of dropping to the
    /// device default and following its way back; the pump moves it to the
    /// playing file's rate when the two disagree. None until a stream has
    /// opened; before that the device's own default applies.
    follow_rate: Option<u32>,
    /// Rates the device already rejected. The follow asks once per rate
    /// and then leaves it alone, so a card that can't do 192 kHz doesn't
    /// rebuild the session on every tick of every 192 kHz track. A list
    /// rather than the last one, or a queue alternating two rates the card
    /// lacks would rebuild at every boundary. Cleared when the mode or the
    /// device changes, since the next one may well take them.
    refused_rates: Vec<u32>,
    /// The view playback started in (ADR 17), so a continuation provider can
    /// carry on down the list rather than guess. Whoever starts playback sets
    /// it; a start that names nothing leaves the library at large.
    scope: continuation::Scope,
    /// The library id of every track in the session's pool, in pool order, so
    /// it lines up with `Session::queue`. None for a file the library doesn't
    /// hold. Two jobs at once: the entry the pump is standing on is the
    /// provider's seed, and the whole vec is the recent plays it must not
    /// hand back.
    pool_ids: Vec<Option<i64>>,
    /// What the sessions before this one held, oldest first and capped
    /// (`HEARD_CAP`), folded in whenever a start replaces the pool. The
    /// random and similar draws read it beside the pool, since those draws
    /// are themselves session starts: a guard off the pool alone resets at
    /// exactly the press most likely to repeat.
    heard: VecDeque<i64>,
    /// A continuation query is out. The pump fires on a 16 ms clock and a
    /// provider takes tens of milliseconds, so without this one dry-out would
    /// queue a few dozen of them.
    continuing: bool,
    /// The queue revision the last continuation fired at. The guard above
    /// covers the query; this covers what comes after it. A batch that arrived
    /// moves the revision, so the next tick sees a full queue and stays quiet;
    /// an empty batch doesn't, which stops the pump asking the same
    /// exhausted provider sixty times a second.
    continued_rev: Option<u64>,
    /// The strategy the continuation toggle turns back on. Continuation is a
    /// mode with an off state rather than a switch beside a mode, so the
    /// transport's press has to remember what it turned off. Session-local:
    /// the persisted pick is the mode itself.
    last_continuation: continuation::Mode,
    /// Where the audible station's current song started, kept by the pump.
    /// A station's position counts the whole listen, which is the right
    /// number for the Stations panel and the wrong one for the transport,
    /// so the clock is derived from the two rather than stored twice.
    /// None whenever nothing live is audible. See [`Self::track_song_start`].
    song_start: Option<SongStart>,
}

impl Player {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        // Off is not a strategy to go back to, so a player that starts with
        // continuation off arms the default behind the toggle.
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

    /// What the engine needs per queued key beyond the file itself: the album
    /// group (ADR 17), the ReplayGain tags (ADR 19), and the span a cue track
    /// plays, plus the library id continuation keeps to know what the session
    /// has already held. Four parallel vecs, three of which the queue commands
    /// include. Unknown keys resolve to ungrouped, untagged, unsliced and
    /// unidentified; a missing database means every key does, and playback
    /// continues unlevelled.
    ///
    /// The group falls out of the album tags, so two tracks of one rip resolve
    /// to the same one and the crossfade leaves their gapless splice alone
    /// (ADR 19). Nothing here has to special-case that; the sheet's album is
    /// written to every row the scanner writes.
    fn queue_meta_for(&mut self, keys: &[TrackKey]) -> QueueMeta {
        if self.meta_conn.is_none() {
            let db = rox_core::settings::data_dir().join("library.db");
            self.meta_conn = db.exists().then(|| store::open(&db).ok()).flatten();
        }
        resolve_queue_meta(self.meta_conn.as_ref(), keys)
    }

    /// Where these keys play from, on the same connection the queue
    /// metadata comes off. Always called right after [`queue_meta_for`],
    /// which is what opens the connection.
    fn locators_for(&self, keys: &[TrackKey]) -> Vec<Locator> {
        resolve_locators(self.meta_conn.as_ref(), keys)
    }

    /// The audio feed the audio views read from.
    pub fn feed(&self) -> Arc<AudioFeed> {
        self.feed.clone()
    }

    /// Where playback currently is, resolved off the shared position
    /// clock. None while no session is running or before the first track
    /// opens.
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
        // What the pump saw belongs to the station it was observed on, so a
        // skip to another entry leaves it behind rather than counting the
        // new one from a stranger's boundary.
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

    /// Where the audible station's songs turned over, oldest first, each as
    /// a distance back from the live edge. Empty for a file, and for a
    /// station that hasn't announced anything since the buffer opened.
    ///
    /// The set the buffer holds rather than the station's whole evening:
    /// a song whose start has rolled off the back has no place on a strip
    /// spanning the buffer, so it isn't here either.
    pub fn live_marks(&self) -> Vec<LiveMark> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let Some((track, _)) = session.shared.position(session.device_rate) else {
            return Vec::new();
        };

        // The shift's own index guard, reused: the marks belong to whichever
        // entry is publishing a shift, so a station pre-rolled behind a file
        // doesn't hand its songs to the file's strip.
        match session.shared.shift(track).is_some() {
            true => session.shared.live_marks(),
            false => Vec::new(),
        }
    }

    /// Where the audible station's connection broke and picked up again,
    /// oldest first. Empty for a file and for a station that has held one
    /// connection throughout, which is most of them.
    ///
    /// A strip drawing the buffer draws these as breaks in the bar. They
    /// are the one thing on that strip a click can't get past: the bytes
    /// either side came off two connections and don't decode as one run, so
    /// a seek back over a break stops at it. Showing the wall is the whole
    /// point, since the alternative is a listener finding it by aiming past
    /// it.
    pub fn live_gaps(&self) -> Vec<LiveGap> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let Some((track, _)) = session.shared.position(session.device_rate) else {
            return Vec::new();
        };

        // The shift's index guard again, for the reason the marks use it:
        // a station pre-rolled behind a file has a tape of its own and no
        // business drawing on the file's strip.
        match session.shared.shift(track).is_some() {
            true => session.shared.live_gaps(),
            false => Vec::new(),
        }
    }

    /// Absolute seek within the playing track, for the waveform strip.
    pub fn seek_to(&self, secs: f64) {
        self.send(Cmd::Seek(secs.max(0.0)));
    }

    /// Play the audible station from `behind_secs` back in its own buffer,
    /// zero being the live edge. Anything past what the buffer holds lands
    /// at the oldest thing in it.
    ///
    /// Nothing happens for a file or for a station that isn't the thing
    /// playing: a timeline you can scrub is [`Player::seek_to`]'s business.
    pub fn seek_live(&self, behind_secs: f64) {
        self.send(Cmd::SeekLive(behind_secs.max(0.0)));
    }

    /// Jump to the live edge, which is what the LIVE button does.
    pub fn go_live(&self) {
        self.seek_live(0.0);
    }

    /// Replace whatever is playing with a fresh queue starting at its first
    /// track; the old session quits on drop.
    pub fn play(&mut self, queue: Vec<TrackKey>, cx: &mut Context<Self>) {
        self.start_session(queue, 0, None, Vec::new(), false, cx);
    }

    /// Replace the queue and start at `start`, so the tracks before it stay
    /// behind the cursor as history and Prev goes back into them. What a
    /// double click in a track list uses, seeding the whole list so Next and
    /// Prev continue through the surrounding album instead of dead-ending at
    /// the clicked track.
    pub fn play_at(&mut self, queue: Vec<TrackKey>, start: usize, cx: &mut Context<Self>) {
        self.start_session(queue, start, None, Vec::new(), false, cx);
    }

    /// Replace whatever is playing with a fresh queue whose entries are all
    /// explicit, playing from the first. Unlike [`play`] and [`play_at`],
    /// which seed a context (an album or library run that plays on unlisted),
    /// these entries are the up-next queue, so the queue panel lists them.
    /// Clicking an album in a browser calls this, so the album you played
    /// shows in the queue.
    pub fn play_explicit(&mut self, queue: Vec<TrackKey>, cx: &mut Context<Self>) {
        let explicit = vec![true; queue.len()];
        self.start_session(queue, 0, None, explicit, false, cx);
    }

    /// The launch restore for an old settings file that saved only a single
    /// track: load it paused at a position, ready on the seek strip but silent
    /// until asked to play. Files written since store the whole queue and come
    /// back through [`restore_queue`] instead.
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

    /// The launch restore: bring back the whole play order paused at the
    /// cursor, so Prev and Next move through the saved context and the up-next queue
    /// panel comes back with the explicit entries it held. `explicit` runs
    /// parallel to `queue`; `cursor` is the entry that was playing.
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

    /// The queue's revision, so a panel can skip re-reading the snapshot on
    /// ticks where nothing changed. None while no session runs.
    pub fn queue_rev(&self) -> Option<u64> {
        Some(self.session.as_ref()?.shared.queue_rev())
    }

    /// The station-title revision, the same deal [`queue_rev`](Self::queue_rev)
    /// offers: poll the atomic on the pump's clock and only go take the
    /// title on the ticks where a stream moved to the next song.
    pub fn title_rev(&self) -> Option<u64> {
        Some(self.session.as_ref()?.shared.title_rev())
    }

    /// What the playing stream says is on. Web radio sends its now-playing
    /// in band and nowhere else, so for a station this is the only answer
    /// there is; everything else answers None and its library tags stand.
    pub fn live_title(&self) -> Option<IcyTitle> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;

        session.shared.live_title(track)
    }

    /// What the playing station said about itself when the stream opened:
    /// its own name, genre, bitrate, homepage and content type off the
    /// response headers. None for everything that isn't a station, and for
    /// a station whose server sent none of it.
    ///
    /// The row in the library knows what the user typed and a directory
    /// filled in; this is what the stream itself claims, which is the only
    /// description a typed-in URL ever gets.
    pub fn station_info(&self) -> Option<StationInfo> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;

        session.shared.station_info(track)
    }

    /// Where the playing stream stands: opening, live, reconnecting through a
    /// drop, or given up on. None for a local file, which has none of those
    /// states, and none for a stream nothing has opened yet.
    ///
    /// It moves the title revision, so a surface already polling that for
    /// song changes picks this up on the same tick without a second clock.
    pub fn stream_state(&self) -> Option<StreamState> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;

        session.shared.stream_state(track)
    }

    /// How long the playing station has been on the song it's on, for a
    /// caller that wants the number without the snapshot. None off a
    /// station and until the stream names a song, where the honest answer
    /// is the whole listen and [`NowPlaying::position_secs`] already has it.
    pub fn song_elapsed(&self) -> Option<f64> {
        let now = self.now_playing()?;

        now.song_start_secs
            .map(|start| song_clock(now.position_secs, Some(start)))
    }

    /// The tags every surface that names the playing track draws from: the
    /// library row, with a station's current song laid over it so the song
    /// reads as the title and the station's own name as the album.
    ///
    /// One accessor because there are five of these surfaces (the readout,
    /// the window title, the queue's playing strip, the OS media card and
    /// Discord) and a station announces its songs in band, where none of
    /// them would think to look. Five copies of that lookup would be five
    /// places to forget it.
    pub fn now_meta(&self, library: &Library) -> Option<store::TrackMeta> {
        let key = self.now_playing()?.key;

        self.live_over(library.meta_for_key(&key))
    }

    /// [`now_meta`](Self::now_meta) for a caller that already holds the
    /// row. The track info readout caches its library lookup across frames
    /// and a query per frame is exactly what that cache is there to avoid,
    /// so it keeps the cache and comes here for the overlay.
    ///
    /// Answers the row untouched when nothing live is playing, which is
    /// every local and every server-backed track.
    pub fn live_over(&self, row: Option<store::TrackMeta>) -> Option<store::TrackMeta> {
        let Some(title) = self.live_title() else {
            return row;
        };

        Some(crate::radio::live_tags(row, &title))
    }

    /// The explicit up-next queue: what Play Next and Add to Queue put ahead
    /// of the playing track, apart from the context (the album or library) that
    /// plays on around it. Empty during plain context playback, which
    /// keeps the queue widgets quiet until you actually queue something.
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

    /// How many tracks are in the explicit queue, for the widget badge.
    pub fn queued_count(&self) -> usize {
        self.queued().len()
    }

    /// The key a queue entry names. The engine's pool holds bare locators,
    /// so two cue tracks of one image are indistinguishable down there; this
    /// mirror, indexed by the entry's pool index, tells them apart.
    /// Anything drawing a queue row's title or resolving it back to a library
    /// row has to come through here rather than read `entry.locator`.
    ///
    /// An index the mirror doesn't hold falls back to the entry's own
    /// locator as a plain local file, which is what every entry was before
    /// cue tracks existed. A remote entry that far out of step names its URL
    /// under an empty source: the mirror is the only thing that knew which
    /// source it came from, and an empty one matches no row rather than
    /// claiming to be a file.
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

    /// The key at a pool index, None while no session holds one.
    pub fn key_at(&self, idx: usize) -> Option<TrackKey> {
        self.session.as_ref()?.queue.get(idx).cloned()
    }

    /// The whole play order for the close-time persist: every entry's path
    /// and whether it was explicit, plus the audible cursor and where its
    /// clock is. The cursor comes off the position clock, not the decode
    /// cursor, so it names the track you hear rather than the one already
    /// opened for the gapless boundary. None when no session runs.
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

    /// The whole play order for an external reader (the control socket),
    /// with the handles an edit needs: each entry's stable id, its key, and
    /// whether it was queued explicitly, plus the audible cursor. The same
    /// read as [`queue_state`](Self::queue_state) but keeping the ids, which
    /// the persist has no use for and a remove or move can't do without.
    /// None when no session runs.
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

    /// Queue tracks to play next, at the front of the explicit queue right
    /// after the playing track. With nothing loaded this just starts them.
    pub fn play_next(&mut self, keys: Vec<TrackKey>, cx: &mut Context<Self>) {
        let after = self.playing_after();
        self.insert(after, keys, false, cx);
    }

    /// Play these now without discarding the queue: splice them right after the
    /// playing track and jump to the first, so the rest of the queue plays on
    /// behind them. With nothing loaded this just starts them. The drop's Play
    /// now zone routes here; an OS file open replaces the session instead.
    ///
    /// Radio is the exception at both ends. A station never finishes, so
    /// anything left queued behind one waits forever: putting a station on
    /// empties the queue and the station plays by itself. Leaving a station
    /// for anything else takes the station out too, rather than leaving it
    /// sitting in the timeline for Prev to walk back into. Play Next and Add
    /// to Queue go untouched, since queueing a station is a thing you asked
    /// for.
    pub fn play_now(&mut self, keys: Vec<TrackKey>, cx: &mut Context<Self>) {
        // Nothing to play means nothing changes. `insert` would bail on its
        // own, but the station rules below read as a move off what's on and
        // an empty batch is no such thing.
        if keys.is_empty() {
            return;
        }

        // Asked before the splice: once the jump lands, what's playing is the
        // new track and the station that was on is just another entry. The
        // answer holds whatever replaces it, another station included, since
        // this is about the station you left rather than what you left it for.
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

    /// Queue tracks at the end of the explicit queue, after anything already
    /// queued but before the context resumes. With nothing loaded this starts
    /// them.
    pub fn enqueue(&mut self, keys: Vec<TrackKey>, cx: &mut Context<Self>) {
        let after = self.enqueue_after();
        self.insert(after, keys, false, cx);
    }

    /// The queue entry index of the playing track, matched by pool index off
    /// the position clock, so a Play Next goes in after what you hear rather
    /// than after a track the decoder has already opened for the gapless
    /// boundary. Matching on the pool index rather than the path keeps a file
    /// that appears in the order twice from resolving to the wrong occurrence,
    /// which would otherwise leave the real playing entry inside `queued()`
    /// and never clear.
    fn audible_index(&self, snap: &QueueSnapshot) -> Option<usize> {
        let now = self.now_playing()?;
        snap.entries.iter().position(|e| e.idx == now.audible_idx)
    }

    /// The queue entry index of the newest track the engine has taken on,
    /// which is where a skip went even while the position clock still reads
    /// the track it left.
    ///
    /// The newest segment is the one the engine pushed when it adopted the
    /// track, and under a crossfade it's half a window in the future: the
    /// clock flips at the fade's midpoint so nothing announces a track before
    /// it's audible (ADR 19). Reading the segment itself is how a caller
    /// learns where the queue went without waiting the fade out. None before
    /// any track has been opened, or while the newest one isn't in the order
    /// the snapshot was taken from.
    fn adopted_index(&self, snap: &QueueSnapshot) -> Option<usize> {
        let session = self.session.as_ref()?;
        let adopted = session.shared.segments.lock().unwrap().last()?.track;
        snap.entries.iter().position(|e| e.idx == adopted)
    }

    /// The entry Play Next queues right after: the playing one. Falls back to
    /// the published cursor before audio starts.
    fn playing_after(&self) -> Option<u64> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        match self.audible_index(&snap) {
            Some(i) => snap.entries.get(i).map(|e| e.id),
            None => snap.entries.get(snap.cursor).map(|e| e.id),
        }
    }

    /// The playing entry's id when what you hear is a station, None for
    /// everything else. Both halves of the leave-on-replace question in one
    /// look: whether there's a station to take out, and which entry it is.
    fn playing_station(&self) -> Option<u64> {
        let now = self.now_playing()?;
        if now.origin != Origin::Radio {
            return None;
        }

        self.playing_after()
    }

    /// The entry Add to Queue appends after: the last explicit entry in the
    /// run following the playing track, so it goes at the tail of the queue
    /// and ahead of where the context picks back up. The playing track itself
    /// when the queue is empty.
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

    /// Splice keys into the running session as explicit queue entries,
    /// mirroring the pool growth on our side so `now_playing` can still resolve
    /// a freshly queued track back to its file. With no session, fall back to
    /// starting playback (a context, not a queue).
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

    /// Play `key` now, opened `secs` into it, the play-from-bookmark move:
    /// spliced after the playing track like [`play_now`](Self::play_now),
    /// or started fresh when nothing is loaded. The offset rides the same
    /// command as the insert, so the track's head is never heard; a fresh
    /// session takes the seek ahead of its first decode the way the launch
    /// restore does.
    pub fn play_now_at(&mut self, key: TrackKey, secs: f64, cx: &mut Context<Self>) {
        if self.session.is_none() {
            self.play(vec![key], cx);
            self.seek_to(secs);
            return;
        }
        let after = self.playing_after();
        self.splice(after, vec![key], None, true, true, Some(secs.max(0.0)), cx);
    }

    /// The insert both the hand-queued keys and a delivered continuation batch
    /// go through: resolve the library metadata, mirror the pool growth, and
    /// hand the batch to the engine. `groups` overrides what the library says
    /// about album membership where a caller has an opinion; None per entry,
    /// or None for the whole batch, takes the library's own grouping.
    ///
    /// The keys split into paths and spans right here, at the engine
    /// boundary: below this line a cue track is one more path with a slice
    /// beside it, and nothing in the engine treats it differently.
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
        // Nothing to mirror the growth onto, so bail before anything grows:
        // `pool_ids` runs parallel to the session's pool and a half-applied
        // splice would slide the two apart for the rest of the session.
        if self.session.is_none() {
            return;
        }
        // Library lookup before the session borrow; both want &mut self.
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
        // Resolved before the session borrow, which wants &mut self too.
        let locators = self.locators_for(&keys);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.queue.extend(keys);
        session.gains.extend(meta.gains.iter().copied());
        session.live.extend(live_flags(&locators));
        // Built before it's sent so the interrupt question is asked of the
        // command itself, the same way [`send`](Self::send) asks it. A play
        // now is the one shape of insert someone is waiting on.
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

    /// Drop a queued entry by its stable id. The playing entry is refused by
    /// the engine, so the UI never has to guard it.
    pub fn remove_from_queue(&self, id: u64) {
        self.send(Cmd::Remove { id });
    }

    /// Drop a set of queued entries in one engine pass. One command and one
    /// queue publish for the whole batch, so clearing or multi-deleting a big
    /// queue does not fire an O(n) remove and a UI wake per id.
    pub fn remove_many_from_queue(&self, ids: Vec<u64>) {
        if ids.is_empty() {
            return;
        }
        self.send(Cmd::RemoveMany { ids });
    }

    /// Drop every up-next explicit entry. The playing track and the context
    /// around it stay; only the hand-picked queue empties.
    pub fn clear_queue(&self) {
        let ids: Vec<u64> = self.queued().iter().map(|e| e.id).collect();
        self.remove_many_from_queue(ids);
    }

    /// Drop `id` from the timeline once the engine has moved off it, which is
    /// how a station leaves when something else is played now.
    ///
    /// It can't be one command behind the insert. The engine refuses to
    /// remove the entry you can hear, and it drains everything waiting before
    /// it acts on any of it, so a remove sent right after the jump arrives
    /// while the station is still what's audible and is ignored. Watching the
    /// playing entry change is the only handle there is from up here.
    ///
    /// Bounded by the same patience the queue wait uses. A jump that never
    /// lands means the station is still playing, and then it keeping its
    /// place is the honest outcome rather than something to force.
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

    /// Play a queued entry now without consuming the rest of the queue: the
    /// entry moves to the front of the explicit queue first, then the jump
    /// goes to it. A bare jump would strand everything above the entry
    /// behind the cursor as history, which reads as the queue clearing.
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

    /// Jump to a queued entry by id and play it now.
    pub fn jump_to(&self, id: u64) {
        self.send(Cmd::Jump { id });
    }

    /// The id of the playing entry, so the queue panel can anchor a reorder to
    /// the front of the queue (right after it) rather than the front of the
    /// whole timeline.
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
        // Album groups, ReplayGain and cue spans for the whole context. A
        // restore re-derives all three here too, so none of them needs
        // persisting with the queue.
        let meta = self.queue_meta_for(&queue);
        let (groups, gains, spans) = (meta.groups, meta.gains, meta.spans);
        // A paused start (the launch restore) never renders audio, so the
        // visualizer tap stays empty and the spectrum has nothing to show.
        // Remember what to prime the feed with so a frozen panel gets a real
        // frame at the load position instead of blank bars. A cue track's
        // clock runs from its own zero, so the window to decode is that far
        // into the image rather than that far into the file.
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
        // A fresh context is a fresh session for continuation too: nothing
        // has been played, nothing has been asked for, and whoever started
        // playback names the scope after this returns. A rebuild (a device
        // or rate change) puts all three back, since the music never stopped.
        // The outgoing pool joins the heard ring before the new one replaces
        // it, which lets the draws remember across the swap; on a
        // rebuild the same ids come straight back as the live pool, so the
        // fold is a no-op rather than a wrong answer.
        remember_held(&mut self.heard, self.pool_ids.iter().flatten().copied());
        self.pool_ids = meta.ids;
        self.scope = continuation::Scope::default();
        self.continuing = false;
        self.continued_rev = None;
        // Nothing has been seeded against this pool, and the indices in the
        // marker belong to the pool going out. The first pump tick adopts the
        // new one without asking for a ranking, since the start below orders
        // its own tail when the mode calls for it.
        self.reseeded_at = None;
        self.skip_reseed = SkipReseed::Idle;
        // Whatever was on the air belonged to the pool going out.
        self.song_start = None;
        self.session = None;
        // A fresh context takes the current shuffle mode; a restore preserves
        // the saved order and passes None so the engine leaves it untouched.
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
                // Read here rather than sent as a command: the engine's first
                // open happens before it reads the channel, so a session that
                // starts on a station would otherwise tape a default window
                // for its first entry.
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
                // Ask the next open for the rate this one settled on. A card
                // that took 44.1 keeps being asked for 44.1, so a rebuild
                // for any other reason doesn't drop to the device default
                // and then follow its way back up with a second gap. Only
                // exclusive gets to pick a rate at all: reusing a shared
                // session's mixer rate would make the switch into
                // exclusive open at the mixer rate first, then follow the
                // file's, which is the second gap this exists to avoid.
                self.follow_rate = (session.negotiated.mode == Mode::Exclusive)
                    .then_some(session.negotiated.sample_rate);
                self.session = Some(session);
                self.error = None;
                self.start_pump(cx);
                if let Some((locator, secs)) = prime {
                    self.prime_feed(locator, secs, rate, cx);
                }
                // A fresh context under the similarity mode owes its tail an
                // ordering: the engine seeded it with the plain shuffle flag,
                // which for this mode means pool order. A restore keeps the
                // order it saved and asks for nothing.
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

    /// Drop the running session entirely: playback stops, the position
    /// clock goes away, and the views over it (the seek strip, the
    /// waveform, the cover) fall back to idle. The transport's eject.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.pump = None;
        self.error = None;
        // The loop went with the session; a half-marked A pointing at a
        // track nothing is playing would come back on the next Play.
        self.ab_pending_a = None;
        // Ending playback by hand answers the timer's question. Leaving it
        // running would arm a stop over whatever gets played next.
        self.sleep = None;
        cx.notify();
    }

    /// Run the tap drain on a timer instead of a render pass. The timer
    /// keeps ticking for the whole session so the drain feeds the audio
    /// views and so a resume (which flips on the audio thread) gets noticed,
    /// but the notify that repaints the clock, the meter, and the falling
    /// bars only fires while audio moves, on the play-state edge, when a
    /// paused seek moves the position clock, or when the engine finishes a
    /// queue edit. That last one matters while
    /// paused: queue commands are fire-and-forget to the engine thread, so
    /// the revision bumps after the notify an enqueue sends, and without a
    /// wake here the queue views would stay one edit behind until the next
    /// poke. A settled pause with a settled queue notifies nobody: the
    /// seek clock is frozen, the visualizers park themselves, and the
    /// whole UI goes quiet.
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
                    // The output stream died (device unplugged, backend fault).
                    // Reopen it at the current spot. A swap leaves this pump
                    // running over the same engine; a rebuild starts its own
                    // and a failure clears the session, and both of those end
                    // this one. Two pumps on a session would double-drain the
                    // tap.
                    if this
                        .session
                        .as_ref()
                        .is_some_and(|s| s.shared.device_lost())
                    {
                        return this.reopen_device(cx);
                    }
                    // Exclusive follows the file's rate, which means the same
                    // stop: the rebuild brings its own pump up.
                    if this.follow_source_rate(cx) {
                        return false;
                    }
                    this.drain_tap();
                    // The sleep timer rides the same clock: one compare against
                    // an Instant, on a tick that already runs for every session.
                    this.tick_sleep(cx);
                    // The continuation trigger runs on this same clock (ADR 17).
                    // It reads the queue snapshot the check below already needs
                    // and does nothing at all on the overwhelming majority of
                    // ticks, which is why it can run on a 60 Hz timer.
                    this.continue_if_dry(cx);
                    // Track boundaries ride the same clock, and for the same
                    // reason: this is the only thing watching what's audible
                    // often enough to notice one going past.
                    this.reseed_on_boundary(cx);
                    // A station's songs turn over on the same clock too, and
                    // nothing else is watching the title revision.
                    this.track_song_start();
                    // And an entry the engine gave up on, for the same
                    // reason again: a skip happens inside the session, so
                    // nothing else here would ever notice one went past.
                    this.take_refusal(cx);
                    let playing = this.is_playing();
                    let rev = this.queue_rev();
                    // A seek while paused moves the clock without touching any
                    // of the above: audio stays quiet and the queue keeps its
                    // revision, so the seek strip and the MPRIS position would
                    // show the old spot until the next resume. Compare the
                    // resolved position while paused; playing ticks notify
                    // anyway, so the check skips them and a settled pause still
                    // costs nothing when nothing moved.
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

    /// Watch the audible station for the moment it moves to the next song
    /// and keep where its clock stood when it did. A broadcast announces a
    /// song and nothing else, so an ICY title arriving is the whole
    /// definition of a boundary here, and the transport subtracts the start
    /// to show the song instead of the listen.
    ///
    /// Gated on the title revision, which is what lets this sit on a 16 ms
    /// clock: same entry and same revision means nobody published, and the
    /// title lock goes untaken. The revision is global, so a bump from
    /// another entry still lands here, and the title compare is what sends
    /// it away again. A pause rejoin republishes the song it hung up on,
    /// [`Shared::publish_title`] drops the repeat, and the revision doesn't
    /// move at all: a resumed listener keeps the clock they paused on.
    fn track_song_start(&mut self) {
        let Some(session) = self.session.as_ref() else {
            self.song_start = None;
            return;
        };

        // Nothing live is audible: a file counts from its own zero and has
        // no title coming to move anything.
        let audible = session
            .shared
            .position(session.device_rate)
            .filter(|(track, _)| session.live.get(*track).copied().unwrap_or(false));
        let Some((idx, secs)) = audible else {
            self.song_start = None;
            return;
        };

        // The overwhelming majority of ticks end here.
        let rev = session.shared.title_rev();
        if self
            .song_start
            .as_ref()
            .is_some_and(|start| start.idx == idx && start.rev == rev)
        {
            return;
        }

        // A station that hasn't said anything yet has no song to start from.
        let Some(title) = session.shared.live_title(idx) else {
            self.song_start = None;
            return;
        };

        if starts_new_song(self.song_start.as_ref(), idx, &title) {
            // A record already standing means this station has been on the
            // air with us and just changed song, which is a boundary we
            // watched. An empty record means the entry only now became
            // audible, so its first title is whatever was already playing.
            let from_start = self.song_start.is_some();
            self.song_start = Some(SongStart {
                idx,
                rev,
                title,
                at_secs: secs,
                from_start,
            });
        } else if let Some(start) = self.song_start.as_mut() {
            // Still the same song on a revision some other entry moved. Take
            // the revision so the next tick is the cheap one again.
            start.rev = rev;
        }
    }

    /// The continuation trigger (ADR 17): when the audible cursor comes
    /// within [`continuation::FLOOR`] tracks of the end of the upcoming
    /// portion, ask the active provider for a batch and append it into the
    /// running session.
    ///
    /// Here rather than in the engine, even though the engine gets to the end
    /// first. Its `pos` is the decode cursor and runs up to a ring ahead of
    /// the speakers, and firing there would put the audio thread inside the
    /// library stores, which inverts the one dependency this whole design
    /// keeps clean. The engine stays a decoder stepping through a list, with
    /// no notion of continuation.
    fn continue_if_dry(&mut self, cx: &mut Context<Self>) {
        self.request_continuation(false, cx);
    }

    /// Ask the active provider for a batch and append it. `force` is the draw
    /// the pump would never have made: it skips the dry-out test and the
    /// per-revision guard, which is what engaging the Similar order needs,
    /// since the queue it's about to re-rank is full of tracks that have
    /// nothing to do with what's playing.
    ///
    /// What a forced draw doesn't skip is `continuing`. One query at a time is
    /// the rule for both, or a press landing on the same tick as a dry-out
    /// splices two batches for one gap.
    ///
    /// The revision guard goes the other way. It exists so an empty batch
    /// doesn't have the pump asking the same exhausted provider sixty times a
    /// second, and a forced draw happens once per press, so it stamps the
    /// revision on the way in (a pump tick during the query must not double
    /// it) and clears it again on the way out. Clearing matters: a forced
    /// batch lands on a full queue, so nothing else is going to move the
    /// revision, and leaving the stamp would mean the queue it filled couldn't
    /// ask for more when it really did run down.
    fn request_continuation(&mut self, force: bool, cx: &mut Context<Self>) {
        let mode = self.settings.session.continuation;
        if mode == continuation::Mode::Off || self.continuing {
            return;
        }
        // Only for music that's actually running out.
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
        // The audible cursor, not the decode cursor: that one has run a track
        // ahead for the gapless boundary, and a batch seeded off a track
        // nobody has heard yet is a batch for the wrong taste.
        let audible = session.shared.position(session.device_rate).map(|(t, _)| t);
        if !force && !self.running_dry() {
            return;
        }
        let seed = continuation::Seed {
            track: audible.and_then(|idx| self.pool_ids.get(idx).copied().flatten()),
            scope: self.scope.clone(),
            recent: self.pool_ids.iter().flatten().copied().collect(),
            count: continuation::BATCH,
            // The pick the Similar ordering ranks against, taken on this tick
            // for the same reason the flag below is: a refill scoring one
            // model while the queue is sorted by another is two answers to
            // one question, and on a library described under a single model
            // the wrong name scores nothing at all.
            model: crate::acoustic::acoustic_source().id().to_string(),
        };
        // A queue ordered by sound is refilled by sound: the radio draw
        // belongs to the shuffle order rather than to a continuation mode of
        // its own. Read here rather than inside the provider, because this is
        // the same tick that decides the mode is still current.
        let order = self.queue_order();
        self.continuing = true;
        self.continued_rev = Some(rev);
        let db_path = rox_core::settings::data_dir().join("library.db");
        cx.spawn(async move |this, cx| {
            // Blocking store queries on the background executor, the shape
            // ADR 14 already set for anything that reads a database while
            // music is playing. Its own connection: the player's is for the
            // per-path lookups on this thread.
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
                // The Similar order was engaged while this query was out. The
                // press that engaged it asked for a forced draw and was turned
                // away by `continuing`, and the batch it waited on has just
                // been dropped by the landing for being the wrong order, so
                // nothing is left to bring the radio in before the floor. Ask
                // again on the press's behalf.
                if order != continuation::Order::Similar && this.similar_order() {
                    this.request_continuation(true, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Append a provider's batch into the running session as context entries.
    ///
    /// An append, never a successor session: the gapless boundary (ADR 3)
    /// holds because this is only more entries behind the append-only pool,
    /// and the engine opens the next track for the boundary exactly as it
    /// does mid-album. Starting a second session instead would be a stream
    /// teardown, the same glitch ADR 16 refused for ordinary queue edits.
    fn land_continuation(
        &mut self,
        mode: continuation::Mode,
        order: continuation::Order,
        force: bool,
        picks: Vec<Pick>,
        cx: &mut Context<Self>,
    ) {
        // The mode changed while the query ran, so this answer is for a
        // question nobody is asking any more. A cleared revision says the
        // same thing about the session: a fresh context or a stream rebuild
        // resets it, and a batch picked for the queue that was playing then
        // has no business being appended to this one. `order` goes the same way,
        // since a batch the radio drew is the wrong twenty tracks for a queue
        // that has since gone back to browse order.
        if mode != self.settings.session.continuation
            || order != self.queue_order()
            || self.continued_rev.is_none()
            || self.session.is_none()
        {
            return;
        }
        // The query took long enough to pause in, or to queue an album in, so
        // the trigger's own conditions are asked again here rather than
        // assumed to have held. Twenty context tracks appended behind a queue
        // the listener just filled is the same wrong answer as one appended to
        // a queue they just paused.
        //
        // All but the dry-out, which a forced batch was never waiting on: the
        // queue that asked for this one is full on purpose, and re-asking here
        // would drop every batch the Similar order fires for.
        if !continuation_wanted(self.is_playing(), self.stop_after)
            || (!force && !self.running_dry())
        {
            return;
        }
        if picks.is_empty() {
            // Nothing left to continue with, so playback ends here. This is
            // deliberately not a cue to try another provider: continuation is
            // one taste at a time, not a lookup racing services for the best
            // answer.
            log::info!("continuation: {} had nothing left", mode.label());
            return;
        }
        let mut resolved = self.resolve_picks(&picks);
        if resolved.is_empty() {
            return;
        }
        // Shuffle on means shuffle everywhere, so an arriving batch joins the
        // upcoming permutation rather than staying in provider order at the
        // tail. The two modes fold it in differently because they mean
        // different things.
        //
        // Random shuffles the batch itself and appends it as it is. The obvious
        // alternative, appending and then reshuffling the whole tail, would
        // scramble any explicit queue the listener hand-built every twenty
        // tracks, and a hand-built queue is explicit intent. It's also the
        // same answer: the trigger fires with at most a floor of tracks left,
        // so there is barely a tail to permute against.
        if self.settings.session.shuffle && self.shuffle_mode() == ShuffleMode::Random {
            shuffle_slice(&mut resolved);
        }
        let (keys, groups): (Vec<TrackKey>, Vec<Option<u64>>) = resolved.into_iter().unzip();
        // Context, not queue: what continuation adds is the album or library
        // run playing on around you, so the queue widgets stay quiet about it
        // the way they do for the context that seeded the session. Visible in
        // the timeline and removable all the same, which is the whole answer
        // to "rox is playing things I didn't pick".
        self.splice(None, keys, Some(groups), false, false, None, cx);
        // Similar ranks the whole upcoming portion against the playing track,
        // which is what the mode already does on every skip, so the fold is
        // just asking it again now the batch has arrived. Nothing to pin
        // here: an explicit queue under this mode was always going to be
        // reordered by it.
        if self.similar_order() {
            self.order_tail_by_similarity(1, None, false, cx);
        }
    }

    /// Re-rank the tail against whatever is audible now, once per track
    /// boundary (see [`reseed_at_boundary`]).
    ///
    /// The audible pool index rather than the queue entry: it's the identity
    /// the seed is looked up by anyway, and reading it costs a couple of
    /// atomics where a queue snapshot clones a `PathBuf` per entry, which is
    /// not something to do sixty times a second. Under a crossfade it flips at
    /// the fade's midpoint (ADR 19), which is exactly when the new track
    /// becomes the one to sound like.
    fn reseed_on_boundary(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some((audible, _)) = session.shared.position(session.device_rate) else {
            return;
        };
        let last = self.reseeded_at.replace(audible);
        // The overwhelming majority of ticks stop here, three minutes of them
        // per boundary.
        if last == Some(audible) {
            return;
        }
        // Whatever moved the queue on, the skip's re-seed is spent: it was for
        // this boundary and there won't be another one to spend it at. A
        // ranking still in flight keeps its claim, since it lands on this
        // boundary too, and releases it when it gives up.
        let (claim, skipped) = self.skip_reseed.spend();
        self.skip_reseed = claim;
        if reseed_at_boundary(self.similar_order(), skipped, audible, last) {
            self.order_tail_by_similarity(1, None, false, cx);
        }
    }

    /// Whether the queue has run close enough to its end to need a batch
    /// (ADR 17).
    ///
    /// Counted from the audible cursor, not the decode cursor, which has run
    /// a track ahead for the gapless boundary and would fire a track early.
    /// The published cursor stands in before any frame has played, so a
    /// session that comes up already short (a one-track queue, Play Similar)
    /// fires on its first tick, which is the point.
    ///
    /// Asked twice for every batch, once to fire the query and again when the
    /// answer comes back: a query is a hundred milliseconds, which is plenty
    /// of room to queue an album into, and a batch appended behind one is
    /// twenty tracks nobody asked for.
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

    /// Whether the queue is currently being ordered by what sounds alike:
    /// shuffle on, in the Similar mode, with a library that has the vectors
    /// to do it. What the radio draw runs off rather than a mode of its own.
    fn similar_order(&self) -> bool {
        self.settings.session.shuffle && self.shuffle_mode() == ShuffleMode::Similar
    }

    /// The order the queue is in, for the continuation draw to follow.
    fn queue_order(&self) -> continuation::Order {
        if !self.settings.session.shuffle {
            continuation::Order::Browse
        } else if self.shuffle_mode() == ShuffleMode::Similar {
            continuation::Order::Similar
        } else {
            continuation::Order::Random
        }
    }

    /// Resolve a batch to playable keys, each with the group its pick asked
    /// for. Through the player's own store connection, since path and sub
    /// are both on the row an id names. A pick the library can no longer
    /// resolve drops out with its group beside it, so the two never slide
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

    /// Which strategy refills the queue when it runs dry, Off for the
    /// behavior rox had before continuation existed.
    pub fn continuation_mode(&self) -> continuation::Mode {
        self.settings.session.continuation
    }

    /// Change the strategy and persist it. Takes effect at the next dry-out;
    /// nothing playing is disturbed, and the guard is cleared so a mode
    /// switched while the queue is already short fires on the next tick
    /// instead of waiting for another queue edit.
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

    /// Turn continuation off, or back on in whatever strategy it was last
    /// using. The transport button's plain press.
    pub fn toggle_continuation(&mut self, cx: &mut Context<Self>) {
        let mode = match self.settings.session.continuation {
            continuation::Mode::Off => self.last_continuation,
            _ => continuation::Mode::Off,
        };
        self.set_continuation_mode(mode, cx);
    }

    /// Name the view playback started in, so continuation can carry on down
    /// it rather than guess (ADR 17). Called after the play that seeded the
    /// session, since starting one clears this back to the library. Nothing
    /// on screen reads it, so this wakes nobody.
    pub fn set_scope(&mut self, scope: continuation::Scope) {
        self.scope = scope;
    }

    /// What a by-hand draw must avoid: everything the running session
    /// holds plus the heard ring behind it. Both, because the draws start
    /// sessions, and either half alone forgets the wrong thing: the pool
    /// loses the past the moment a press replaces it, and the ring doesn't
    /// have the present until one does.
    fn draw_seen(&self) -> HashSet<i64> {
        self.pool_ids
            .iter()
            .flatten()
            .copied()
            .chain(self.heard.iter().copied())
            .collect()
    }

    /// Pick a track at random and play on from there, drawn from the
    /// context playback is already in: the view or playlist that started the
    /// session, the library at large when nothing named one. The scope is put
    /// back after the start, so a second press stays in the same list instead
    /// of escaping to the library the way a fresh session would.
    ///
    /// The run around the draw comes with it as playing context, so the press
    /// starts somewhere and then keeps going down that album and that list.
    /// Shuffle scatters what follows the way it does for any other start, and
    /// continuation (ADR 17) still takes over at the end of the run.
    ///
    /// The draw falls outside what the session and the sessions before it
    /// have held ([`Self::draw_seen`]), so pressing it over and over keeps
    /// moving somewhere new until the pool runs out.
    pub fn play_random(&mut self, library: &Entity<Library>, cx: &mut Context<Self>) {
        let scope = self.scope.clone();
        let seen = self.draw_seen();
        let drawn = {
            let library = library.read(cx);
            let all: &[i64] = library
                .projection()
                .map(|p| p.db_id.as_slice())
                .unwrap_or_default();
            // A scope id the library no longer holds resolves to no file, so
            // the draw takes the library at large rather than dropping the
            // press on the floor. A scope that already is the library just
            // gets a second try.
            draw_run(library, random_pool(&scope, all), &seen)
                .or_else(|| draw_run(library, all, &seen))
        };
        let Some((keys, start)) = drawn else { return };
        self.play_at(keys, start, cx);
        // After the play, never before: starting a session clears the scope
        // back to the library at large.
        self.scope = scope;
    }

    /// Play a track that sounds like `seed`, drawn library-wide off the
    /// acoustic vectors, with a track running at a tempo the seed doesn't
    /// share marked down for it (see [`embeddings::ranked`]). The library's
    /// Play Similar and the transport's similar draw both call this.
    ///
    /// Scored against the corpus the store keeps standardized in memory, so
    /// the draw is a dot product per track rather than a read of every vector
    /// in the library. It still runs on the background executor against its
    /// own connection the way the Similar ordering does: the first ask after
    /// the analysis pass writes anything rereads the table, a few hundred
    /// milliseconds on a fifty-thousand-track library, and that has no
    /// business on the UI thread. Pressing this twice on one track, and the
    /// Similar column asking about the track this just drew, come back off the
    /// held map without scoring anything. A library with nothing described, or
    /// a seed the pass hasn't reached, leaves playback alone.
    ///
    /// One track rather than the run around it the random draw takes. What
    /// follows a similar track is continuation's business (ADR 17), and the
    /// tracks filed either side of this one only sound like it by accident.
    /// The scope goes with it: a draw that left the view to find this track
    /// has no business keeping that view.
    pub fn play_similar_to(
        &mut self,
        seed: i64,
        library: &Entity<Library>,
        cx: &mut Context<Self>,
    ) {
        let db_path = rox_core::settings::data_dir().join("library.db");
        // Read here rather than down in the spawn: the pick is a process
        // static, and the query only needs the name it stores vectors under.
        let model = crate::acoustic::acoustic_source().id().to_string();
        // Taken now, before the start this press leads to replaces the pool.
        let seen = self.draw_seen();
        let library = library.clone();
        cx.spawn(async move |this, cx| {
            let drawn = cx
                .background_executor()
                .spawn(async move {
                    let conn = store::open(&db_path).ok()?;
                    let near =
                        embeddings::nearest_ranked(&conn, seed, &model, SIMILAR_POOL).ok()?;
                    // One track per song, so a library holding the seed a
                    // dozen times over doesn't answer "play something like
                    // this" with the same song again. Nothing survived means
                    // the neighbourhood is one song, and then the plain
                    // nearest is the honest answer.
                    let near = one_per_song(&conn, seed, &seen, &near);
                    // One out of the unheard part of the neighbourhood rather
                    // than the single nearest, so pressing twice moves
                    // somewhere instead of asking the same question again,
                    // and never onto a track the session already played while
                    // the band still holds one it hasn't.
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

    /// Play a track that sounds like the one playing. Nothing to draw from
    /// while nothing plays, or while the playing file isn't one the library
    /// holds.
    pub fn play_similar(&mut self, library: &Entity<Library>, cx: &mut Context<Self>) {
        let Some(seed) = self
            .now_playing()
            .and_then(|now| library.read(cx).id_for_key(&now.key))
        else {
            return;
        };
        self.play_similar_to(seed, library, cx);
    }

    /// Rebuild the running session against the current output settings, at
    /// the spot it's playing. Captures the live queue, cursor, and position,
    /// tears the session down, and starts a fresh one, because everything
    /// denominated in the device rate goes with the stream: the sample ring,
    /// the resampler, the consumed clock, the segment list. Resumes playing
    /// if it was playing, since none of the reasons to come through here are
    /// a pause.
    ///
    /// False means there was nothing to rebuild from, no session or a queue
    /// that wouldn't resolve. A rebuild that tried and couldn't open reports
    /// through the session error like any other failed start.
    fn rebuild_session(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        let was_playing = session.shared.playing.load(Ordering::Relaxed);
        // Pull the order, cursor, and position off the old session the same
        // way the close-time persist does, so the rebuilt queue matches what
        // was playing rather than the seed order.
        let Some((entries, cursor, position_secs)) = self.queue_state() else {
            return false;
        };
        let (keys, explicit): (Vec<TrackKey>, Vec<bool>) = entries.into_iter().unzip();
        // A rebuild is the same music on a different stream, so the scope
        // stays: the view play started in is still the view play
        // started in. The start clears it, which is right for a fresh context
        // and wrong for this. The played set needs no such care, since the
        // whole order comes back and it's re-derived from that.
        let scope = self.scope.clone();
        // Restore-shaped start: preserve the saved order, seed the position.
        self.start_session(keys, cursor, Some(position_secs), explicit, true, cx);
        self.scope = scope;
        // A restore comes up paused, so put it back to playing. Only when the
        // start actually produced a session.
        if was_playing && self.session.is_some() {
            self.send(Cmd::TogglePause);
        }
        true
    }

    /// Get back to audio after the device dropped out (unplugged, an ALSA
    /// I/O error, a Bluetooth sink reconnecting, a backend fault). The old
    /// stream is dead either way, so this is the only way back short of the
    /// user restarting.
    ///
    /// Three outcomes, in the order they're worth having. The swap reopens
    /// the output under the running engine, which costs the listener the
    /// gap and nothing else. A rate the swap can't take falls to the session
    /// rebuild, the old behaviour: same music, new engine, and a station
    /// re-dialled. Nothing left to restore surfaces as an error with the
    /// session gone, so the UI stops showing a frozen "playing".
    ///
    /// Returns whether the pump that called this should carry on. Only the
    /// swap keeps it: a rebuild brings its own pump up and a stop leaves
    /// nothing to pump, and two pumps on one session would double-drain
    /// the tap.
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

    /// Reopen the output device and hand the running engine the new ring,
    /// leaving everything upstream of it alone.
    ///
    /// This is what a device fault costs now. The engine keeps decoding, a
    /// station keeps its connection, its tape and the capture being cut out
    /// of it, and the pause clock behind the idle hangup never restarts. The
    /// old path tore the session down for the same event, which on a station
    /// meant re-dialling it, throwing the timeshift buffer away, and filing
    /// whatever the capture had as if the song had ended there.
    ///
    /// False where the swap can't stand in for the rebuild: no session, no
    /// device to open, or a device that came back at another rate. That last
    /// one is a real change rather than a fault to paper over, and everything
    /// in the engine is denominated in the rate it opened at, so it goes down
    /// the same path [`follow_source_rate`](Self::follow_source_rate) takes.
    fn swap_output(&mut self, cx: &mut Context<Self>) -> bool {
        let request = self.output_request();
        let Some(session) = self.session.as_mut() else {
            return false;
        };

        // Down before the open, not after it: a device that faults again
        // while this one is opening should set the flag again and get its
        // own pass, rather than have this pass clear a loss it never saw.
        session.shared.device_lost.store(false, Ordering::Release);
        // And the dead stream goes before its replacement is asked for, so
        // the backend has the device back by the time the open reaches it.
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
        // The engine takes the producer end and re-anchors itself on it. Sent
        // after the fields above so nothing reads a session that is half
        // moved over.
        let _ = session.tx.send(Cmd::SwapOutput(out.producer));
        self.error = None;
        cx.notify();

        true
    }

    /// Exclusive mode follows the file's rate (ADR 19): when the playing
    /// track's rate isn't the rate the device is running, reopen at the
    /// file's. Costs the gap between tracks the ADR budgeted for, and it's
    /// the whole reason a fresh queue can open at the device default and
    /// still end up bit-perfect. A file's rate isn't known until the
    /// decode thread has opened it, so the first one is followed a beat
    /// late rather than guessed at.
    ///
    /// Only fires on a rate the device hasn't already rejected, so a card
    /// that can't match doesn't rebuild the session on every tick. Returns
    /// whether it rebuilt.
    fn follow_source_rate(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        if session.negotiated.mode != Mode::Exclusive {
            return false;
        }
        // A pinned rate means the device stays where it was put, gaps and
        // all, so there's nothing here to follow.
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
        // The card came back with something else, so this rate is one it doesn't
        // have. Remember that instead of asking again next tick.
        if self.negotiated().is_some_and(|n| n.sample_rate != rate) {
            self.refused_rates.push(rate);
        }
        true
    }

    /// The playing file's own sample rate, as the decode thread read it off
    /// the container. None until a track has opened.
    fn source_rate(&self) -> Option<u32> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;
        let tracks = session.shared.tracks.lock().unwrap();
        tracks
            .get(track)
            .and_then(|t| t.as_ref())
            .map(|t| t.sample_rate)
    }

    /// What the running stream negotiated, None while nothing is open.
    fn negotiated(&self) -> Option<&Negotiated> {
        self.session.as_ref().map(|s| &s.negotiated)
    }

    /// What to ask the output layer for: the persisted mode with that mode's
    /// device pick, and the rate to follow. The two device picks are kept
    /// apart in the settings because their ids don't cross.
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
            // A pinned rate is the whole ask; the follow only applies when
            // nothing was pinned, so the two can't fight over the stream.
            rate: output.rate.or(self.follow_rate),
            format: output.format.clone(),
            period_ms: output.period_ms,
        }
    }

    /// What output ended up doing, for the settings page. None while no
    /// stream is open: nothing has negotiated with any device yet.
    pub fn output_status(&self) -> Option<OutputStatus> {
        Some(OutputStatus {
            negotiated: self.negotiated()?.clone(),
            source_rate: self.source_rate(),
            leveling_db: self.leveling_db(),
        })
    }

    /// Raise the device-lost flag by hand: the same fault the output
    /// backend's error callback raises when a device drops out from under a
    /// running stream. The pump picks it up on its next tick and reopens.
    ///
    /// A test surface, and the only one there is for this path (ADR 22).
    /// A real fault needs a card to be unplugged or a sink to reconnect
    /// mid-song, which is not something a script can ask for, so the
    /// recovery would otherwise only ever be exercised by accident on
    /// somebody else's machine. False means no session, so nothing to fault.
    pub fn fault_output(&self) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        session.shared.device_lost.store(true, Ordering::Release);

        true
    }

    /// How far the playing file is being moved by ReplayGain, in dB. Run
    /// through the same rule the engine levels with, off the same tags, so
    /// this says what's happening rather than what's switched on: an
    /// untagged file with the fallback at zero comes out unity, and unity
    /// is nothing to report. None while no track is open.
    fn leveling_db(&self) -> Option<f32> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;
        let rg = session.gains.get(track).copied().unwrap_or_default();
        let factor = self.settings.replay_gain.rule().factor(rg);
        (factor != 1.0).then(|| 20.0 * factor.log10())
    }

    /// The whole EQ cascade's gain at one frequency, for whatever plots the
    /// curve. Evaluated at the device rate where a stream is open, since
    /// that's what the running filters were built against; 48 kHz stands in
    /// while nothing plays so the plot still draws the shape.
    pub fn eq_response_db(&self, hz: f32) -> f32 {
        let rate = self
            .session
            .as_ref()
            .map(|session| session.device_rate)
            .unwrap_or(48_000);
        eq_params().response_db(hz, rate)
    }

    /// How long a crossfade runs, in seconds. Zero is off.
    pub fn crossfade_secs(&self) -> f32 {
        self.settings.crossfade_secs
    }

    /// Whether the fade takes album-contiguous boundaries too.
    pub fn crossfade_albums(&self) -> bool {
        self.settings.crossfade_albums
    }

    /// Set the crossfade length and persist it. The running session takes
    /// it live over the command channel, so the next boundary uses the new
    /// length; nothing rebuilds and nothing playing is interrupted.
    pub fn set_crossfade_secs(&mut self, secs: f32, cx: &mut Context<Self>) {
        // The engine's own clamp, so the persisted number and the audible
        // one can't drift apart.
        let secs = secs.clamp(0.0, engine::CROSSFADE_MAX_SECS);
        if self.settings.crossfade_secs == secs {
            return;
        }
        self.settings.crossfade_secs = secs;
        // A length that isn't off is the one a toggle should come back to, so
        // remember it here rather than in the button: the Audio page's slider
        // and the transport's menu both go through this one place, and the two
        // would otherwise disagree about what "back on" means.
        if secs > 0.0 {
            self.settings.crossfade_restore_secs = secs;
        }
        self.send_crossfade();
        // Dragging the slider calls this per tick, same as the volume, so
        // the file write waits for the drag to settle.
        self.persist_playback_soon(cx);
        cx.notify();
    }

    /// Turn the crossfade off, or back on at the last length it ran at. The
    /// transport button's plain press, `toggle_continuation`'s shape.
    pub fn toggle_crossfade(&mut self, cx: &mut Context<Self>) {
        let secs = if self.settings.crossfade_secs > 0.0 {
            0.0
        } else {
            self.crossfade_restore_secs()
        };
        self.set_crossfade_secs(secs, cx);
    }

    /// The length a switched-off crossfade comes back at. Never zero, so the
    /// toggle can't turn the fade "on" at no length; a settings file with
    /// a zero here (hand-edited, or written before the field existed) reads as
    /// the stock length.
    pub fn crossfade_restore_secs(&self) -> f32 {
        let secs = self.settings.crossfade_restore_secs;
        if secs > 0.0 {
            secs
        } else {
            rox_core::settings::DEFAULT_CROSSFADE_SECS
        }
    }

    /// Fade inside an album as well, or leave a record's own splices alone.
    /// Live on the running session like the length.
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

    /// Set it, and hand it to the station that's on air right now.
    ///
    /// The tape used to be sized at the connect and never again, so moving
    /// this while listening changed nothing you could hear until the next
    /// station. That's a strange answer to give someone who is dragging the
    /// slider precisely because they want more of what they're listening
    /// to, so the engine re-caps the open tape in place: growing raises the
    /// ceiling and the window fills into it, shrinking gives the memory back
    /// on the spot.
    ///
    /// The file write waits for the drag to settle, the same debounce the
    /// volume and the crossfade sit behind. `Settings::update` reads and
    /// rewrites five files, which is nothing once and far too much sixty
    /// times a second.
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

    /// How many bytes a second the playing station is taping, as the tape
    /// measures it. None off a live stream.
    ///
    /// The tape's own answer rather than the `icy-br` header, because it
    /// already prefers what playback measured and falls back to that header
    /// itself. What it's for is turning the buffer setting into the weight
    /// of memory it actually costs on the station being listened to.
    pub fn live_bytes_per_sec(&self) -> Option<f64> {
        let session = self.session.as_ref()?;
        let (track, _) = session.shared.position(session.device_rate)?;

        Some(session.shared.shift(track)?.bytes_per_sec)
    }

    /// How tagged loudness is levelled right now (ADR 19).
    pub fn replay_gain(&self) -> ReplayGainSettings {
        self.settings.replay_gain
    }

    /// Switch which gain the leveling reads, or turn it off. Live on the
    /// running session: the engine relevels every source it holds, so the
    /// change applies to the track playing rather than the one after it.
    pub fn set_replay_gain_mode(&mut self, mode: GainModeSetting, cx: &mut Context<Self>) {
        if self.settings.replay_gain.mode == mode {
            return;
        }
        self.settings.replay_gain.mode = mode;
        self.send_gain_rule();
        Settings::update(move |s| s.replay_gain.mode = mode);
        // The library's Gain column draws whichever gain this reads, so
        // publish the pick to the static it renders from.
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
        // A dragged slider calls this per tick, so the file write waits for
        // the drag to settle, the same as volume and the fade length.
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

    /// Where the measurement pass saves what it measured. The engine never
    /// sees it, but it goes through the player like the other three: the
    /// player holds the live copy of `replay_gain` and flushes the struct
    /// whole, so a value written around it would get clobbered by the next
    /// volume tick.
    pub fn set_replay_gain_save(&mut self, save: ReplayGainSave, cx: &mut Context<Self>) {
        if self.settings.replay_gain.save == save {
            return;
        }
        self.settings.replay_gain.save = save;
        Settings::update(move |s| s.replay_gain.save = save);
        cx.notify();
    }

    /// Whether the measurement pass follows the watcher. Through the player
    /// for the same reason as the destination above: the player flushes
    /// `replay_gain` whole, so a value written around it would be gone by the
    /// next volume tick.
    pub fn set_replay_gain_auto(&mut self, auto: bool, cx: &mut Context<Self>) {
        if self.settings.replay_gain.auto == auto {
            return;
        }
        self.settings.replay_gain.auto = auto;
        Settings::update(move |s| s.replay_gain.auto = auto);
        cx.notify();
    }

    /// Hand the engine the whole rule. One command for all three knobs,
    /// since a factor is only decided by reading them together.
    fn send_gain_rule(&self) {
        self.send(Cmd::SetGainRule(self.settings.replay_gain.rule()));
    }

    /// Hand the engine both fade settings; they arrive together because the
    /// boundary decision reads both.
    fn send_crossfade(&self) {
        self.send(Cmd::SetCrossfade {
            secs: self.settings.crossfade_secs,
            albums: self.settings.crossfade_albums,
        });
    }

    /// The crossfade the ear is in the middle of, if any. Off the output
    /// clock, so it shows while the overlap is audible rather than while
    /// the decode thread is mixing it.
    pub fn crossfade(&self) -> Option<FadeView> {
        let (progress, back) = self.session.as_ref()?.shared.crossfade()?;
        Some(FadeView {
            step: (progress.clamp(0.0, 1.0) * FADE_STEPS as f32) as u8,
            back,
        })
    }

    /// Whether exclusive output is asked for. What's actually running is
    /// [`Self::output_status`]; these two disagree whenever a claim failed.
    pub fn exclusive_output(&self) -> bool {
        self.settings.output.exclusive
    }

    /// The device pick for the mode that's asked for, None for the system
    /// default.
    pub fn output_device(&self) -> Option<&str> {
        let output = &self.settings.output;
        if output.exclusive {
            output.exclusive_device.as_deref()
        } else {
            output.device.as_deref()
        }
    }

    /// Ask for exclusive output, or give the device back. The running
    /// session rebuilds against the other backend right here, so the switch
    /// takes hold without a restart; with nothing playing it takes effect on the
    /// next track.
    pub fn set_exclusive_output(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.settings.output.exclusive == on {
            return;
        }
        self.settings.output.exclusive = on;
        Settings::update(move |s| s.output.exclusive = on);
        // The other backend and the other device are a different set of
        // supported rates, so nothing the old one refused still counts.
        self.refused_rates.clear();
        self.rebuild_session(cx);
        cx.notify();
    }

    /// Pick the device for the mode that's asked for, None for the system
    /// default. Rebuilds the running session onto it.
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

    /// Pin the exclusive device to one rate, or None to go back to following
    /// each file's own. Either way the running session reopens, so the
    /// change is audible now rather than at the next track.
    pub fn set_output_rate(&mut self, rate: Option<u32>, cx: &mut Context<Self>) {
        if self.settings.output.rate == rate {
            return;
        }
        self.settings.output.rate = rate;
        Settings::update(move |s| s.output.rate = rate);
        // Going back to following means asking again for rates the old
        // pinned session never tried, so the refusals no longer apply.
        self.refused_rates.clear();
        self.follow_rate = None;
        self.rebuild_session(cx);
        cx.notify();
    }

    /// Ask the exclusive device for one sample format, or None for the
    /// widest it offers. A card that won't take the pick runs the widest and
    /// reports that, so this can't quietly lie.
    pub fn set_output_format(&mut self, format: Option<String>, cx: &mut Context<Self>) {
        if self.settings.output.format == format {
            return;
        }
        self.settings.output.format = format.clone();
        Settings::update(move |s| s.output.format = format);
        self.rebuild_session(cx);
        cx.notify();
    }

    /// Set the exclusive device's period in milliseconds, or None for the
    /// backend default. The latency knob: shorter periods mean the writer
    /// thread wakes more often and xruns sooner under load.
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

    /// The position clock as a comparable key for the pump's change check:
    /// track index and the seconds' raw bits. One atomic read plus a short
    /// lock on the segment list, a handful of entries.
    fn position_key(&self) -> Option<(usize, u64)> {
        let session = self.session.as_ref()?;
        let (track, secs) = session.shared.position(session.device_rate)?;
        Some((track, secs.to_bits()))
    }

    /// The same for a paused session, with how far behind the broadcast a
    /// paused station has drifted folded in.
    ///
    /// A pause on a station is the one state where nothing moves except the
    /// thing worth watching: the position is frozen, the timeshift grows by
    /// a second a second, and so does the tape while it fills. Quantised to
    /// [`PAUSED_SHIFT_STEPS`] a second, which draws the bar and the playhead
    /// sliding rather than stepping while still repainting at half the
    /// pump's rate.
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

    /// Take whatever the tap holds, never wait for more; the samples move
    /// on to the audio views' feed. Read as chunks straight off the ring's
    /// two slices: this runs 60 times a second for the whole session, so
    /// no per-sample pops and no temporary buffer.
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

    /// Decode one window at the load position off-thread and push it into the
    /// feed, so a spectrum panel frozen on pause shows the track's spectrum on
    /// a paused load instead of blank bars. Skips the push if audio started
    /// flowing in the meantime (a quick resume, or another session), so it
    /// never splices a stale window into a live stream.
    ///
    /// A remote track returns here without decoding anything: the window would
    /// cost a second connection to the server to decorate a paused load, so
    /// the bars stay blank until playback starts feeding the tap.
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

    /// Play/pause, for the bar and the keyboard shortcut alike.
    pub fn toggle_pause(&self) {
        self.send(Cmd::TogglePause);
    }

    /// Skip to the next queued track.
    pub fn next(&mut self, cx: &mut Context<Self>) {
        self.send(Cmd::Next);
        if !self.settings.session.shuffle || self.shuffle_mode() != ShuffleMode::Similar {
            return;
        }
        // A skip that follows a long stretch of listening isn't impatience,
        // it's the start of a fresh run: the listener settled on something
        // and has only now moved on. Anything quicker than that is a run,
        // and each one widens the band.
        let now = Instant::now();
        let settled = self
            .last_skip
            .is_none_or(|at| now.duration_since(at) >= SKIP_SETTLE);
        self.similar_skips = if settled { 1 } else { self.similar_skips + 1 };
        self.last_skip = Some(now);
        // Re-seeded on wherever the skip goes, not on where the mode was
        // engaged: that's what turns skipping into steering rather than
        // drifting outward from a track the listener already left.
        let leaving = self.seed_entry();
        // The pump sees where this lands a tick or two from now and would read
        // it as a track that ended on its own. Claim that boundary before the
        // ranking starts, since the ranking waits for the engine to adopt the
        // new track and the pump gets there first; the ranking hands the claim
        // back if it can't produce an order.
        self.skip_reseed = SkipReseed::InFlight { passed: false };
        self.order_tail_by_similarity(skip_band(self.similar_skips), leaving, true, cx);
    }

    /// The queue entry the ordering currently treats as the seed, for a
    /// caller that needs to wait until it is no longer the one playing.
    fn seed_entry(&self) -> Option<u64> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        let at = self.audible_index(&snap).unwrap_or(snap.cursor);
        snap.entries.get(at).map(|e| e.id)
    }

    /// Skip to the previous queued track.
    pub fn prev(&self) {
        self.send(Cmd::Prev);
    }

    /// Whether audio is moving right now, false while paused or idle.
    pub fn is_playing(&self) -> bool {
        self.session
            .as_ref()
            .map(|s| s.shared.playing.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// Whether a session is running at all, playing or paused. What tells
    /// "opening..." apart from plain idle while the position clock is not
    /// up yet.
    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    /// Whether the queue has played through to its end and stopped.
    pub fn queue_ended(&self) -> bool {
        self.session
            .as_ref()
            .map(|s| s.shared.ended.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// The persisted volume, the engine's clamp range (0 to 2). The level
    /// mute returns to, not what the engine currently applies.
    pub fn volume(&self) -> f32 {
        self.settings.session.volume
    }

    /// Whether output is muted.
    pub fn muted(&self) -> bool {
        self.settings.session.muted
    }

    /// What the engine should actually apply: the volume, or silence.
    fn effective_volume(&self) -> f32 {
        if self.settings.session.muted {
            0.0
        } else {
            self.settings.session.volume
        }
    }

    /// The persisted loop mode.
    pub fn loop_mode(&self) -> LoopMode {
        self.settings.session.loop_mode()
    }

    /// Relative seek, the five seconds either way the arrow keys send.
    ///
    /// Two timelines behind the one step. A file has a position and the
    /// step moves along it. A station has none: its clock counts the
    /// listen, and the only thing a seek can move is the cursor through
    /// the buffer. So on a live entry the step keeps its direction and
    /// changes timeline, forward closing the distance to live and backward
    /// opening it, held between the live edge and the oldest second still
    /// held. Past either end there's nothing to play, either because the
    /// broadcast hasn't sent it or because the tape has dropped it.
    ///
    /// The distance is read off the buffer every time rather than
    /// remembered: a pause keeps taping, so it grows under a listener who
    /// isn't pressing anything.
    pub fn seek_by(&self, delta: f64) {
        let Some(session) = &self.session else {
            return;
        };
        let Some((track, secs)) = session.shared.position(session.device_rate) else {
            return;
        };

        if session.live.get(track).copied().unwrap_or(false) {
            // A station whose tape hasn't taken a byte yet publishes no
            // shift, and there's nothing to step through until it does.
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

    /// Walk the playhead one step, `back` for the other direction. The
    /// fine counterpart to [`Player::seek_by`]'s five seconds, at the size
    /// the settings hold.
    ///
    /// Paused, the step also plays what it landed on, for the preview
    /// length. A 25 ms move is too small to see on a strip and too small to
    /// leave a mark, so without the blip stepping through a track by ear
    /// would mean pressing play after every press. The blip runs through
    /// the pause on the audio thread; the pump notices the clock moving
    /// and repaints the readouts, the same way it does for a paused seek.
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

    /// Set the step size and persist it. Nothing to send: the size is read
    /// at each press, so a change applies to the next one.
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

    /// Set the preview length and persist it, the same way.
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

    /// Set the volume and persist it; dragging the slider calls this.
    /// Setting a level always unmutes: reaching for the slider means
    /// wanting to hear something.
    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        // Same clamp range the engine applies, so the persisted value and
        // the audible one never drift apart.
        let volume = volume.clamp(0.0, 2.0);
        self.settings.session.volume = volume;
        self.settings.session.muted = false;
        self.send(Cmd::Volume(volume));
        self.persist_playback_soon(cx);
        cx.notify();
    }

    /// Persist the scrubbed playback values after the current drag settles.
    /// Every slider tick and wheel notch goes through a setter, and
    /// `Settings::update` reads, parses, and rewrites the files, too much
    /// for a pointer-move rate. The engine and the in-memory copy already
    /// hold the value, so only the file write waits for the last tick. Same
    /// pattern as the settings window's persist_appearance_soon.
    ///
    /// The volume, the fade, the leveling knobs, and the live buffer share
    /// the debounce:
    /// only the file whose contents actually moved gets written, so
    /// covering all of them costs nothing and none can outrun another's
    /// pending write.
    fn persist_playback_soon(&mut self, cx: &mut Context<Self>) {
        self.persist_gen += 1;
        let generation = self.persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            // A later tick bumped the generation past this capture, so only
            // the last edit in a burst writes. Read the values at write
            // time, not capture time, so a mute toggled during the wait
            // persists as is.
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

    /// Whether shuffle is on, the persisted mode.
    pub fn shuffle(&self) -> bool {
        self.settings.session.shuffle
    }

    /// Which order shuffle is actually putting the queue in.
    ///
    /// Similar falls back to Random while nothing has been described: the
    /// mode needs vectors to sort by, and one that can't sort is a mode that
    /// silently does nothing. The pick itself is left alone in the settings
    /// rather than rewritten, so describing the library later brings the
    /// listener's order back without them asking for it twice.
    pub fn shuffle_mode(&self) -> ShuffleMode {
        let mode = self.settings.session.shuffle_mode;
        if mode == ShuffleMode::Similar && !rox_core::settings::similarity_ready() {
            return ShuffleMode::Random;
        }
        mode
    }

    /// Change the order shuffle uses and persist it. Takes effect at once
    /// while shuffle is on, and is just a stored preference while it's off.
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

    /// Flip shuffle and persist the pick. The running session reorders in
    /// place; the playing track keeps playing.
    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.set_shuffle_with(!self.settings.session.shuffle, cx);
    }

    /// Force shuffle to `on` and persist it, without toggling relative to the
    /// current mode. The library's shuffle actions set this before they queue,
    /// so the transport toggle reflects the mode they chose. A no-op when the
    /// mode already matches.
    ///
    /// This is the plain form, which always means the random order. The
    /// library's "Play Shuffled" needs exactly that whatever the transport's
    /// mode says: the user asked to shuffle a set, not to hear things that
    /// sound like each other.
    pub fn set_shuffle(&mut self, on: bool) {
        if self.settings.session.shuffle == on {
            return;
        }
        self.settings.session.shuffle = on;
        self.send(Cmd::SetShuffle(on));
        Settings::update(move |s| s.session.shuffle = on);
    }

    /// Turn shuffle on in a particular mode, whatever it was set to before.
    /// What the library's "Play Similar" asks for: the mode is the point of
    /// the action rather than a preference it should inherit.
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

    /// Turn shuffle on or off in whatever order the current mode names.
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

    /// Put the upcoming queue into the current mode's order.
    fn apply_shuffle_order(&mut self, cx: &mut Context<Self>) {
        match self.shuffle_mode() {
            ShuffleMode::Random => self.send(Cmd::SetShuffle(true)),
            ShuffleMode::Similar => {
                self.order_tail_by_similarity(1, None, false, cx);
                // Ordering sorts what's queued; the draw is what makes it
                // worth sorting. Ten browse-order tracks sorted by sound are
                // still ten tracks the listener didn't turn radio on for, so
                // the batch is asked for now and the landing re-ranks the tail
                // with the fresh picks folded into it.
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

    /// Order what's coming by how much it sounds like the playing track, and
    /// how close it runs to its tempo ([`embeddings::ranked`]).
    ///
    /// Scoring the library is milliseconds off the corpus the store keeps in
    /// memory, and a few hundred on the first ask after the analysis pass
    /// writes anything, so it runs on the background executor against its own
    /// connection rather than in this update. The engine keeps playing the
    /// whole time; the reorder shows up as a queue publish whenever the answer
    /// arrives, which is the same way any other queue edit shows up.
    ///
    /// Anything the library can't score keeps its place behind what it can
    /// (see [`Cmd::OrderTail`]), so an unanalyzed library leaves the queue
    /// exactly as it was rather than scrambling it.
    ///
    /// Nothing resets the tail to pool order first. An earlier
    /// cut sent `SetShuffle(false)` up front to normalize it, which meant a
    /// scan that came back with nothing left the queue sorted into library
    /// order: press Next and you got track one, which looked far more broken
    /// than doing nothing would have. Touching the queue only once, when
    /// there's an answer, makes the failure case invisible.
    ///
    /// `from_skip` marks the ranking a skip fired, which holds a claim on the
    /// boundary that skip caused ([`SkipReseed`]). Every path out of here
    /// settles that claim, so a ranking that gives up hands the boundary back
    /// to the pump instead of silently eating its re-seed.
    fn order_tail_by_similarity(
        &mut self,
        band: usize,
        leaving: Option<u64>,
        from_skip: bool,
        cx: &mut Context<Self>,
    ) {
        let db_path = rox_core::settings::data_dir().join("library.db");
        cx.spawn(async move |this, cx| {
            // The engine publishes its queue from the decode thread, first
            // thing in `run`, so a context that was only just started has
            // nothing to read yet. Waiting rather than giving up is the whole
            // point: engaging the mode and replacing the queue happen
            // together, which is precisely the case that would otherwise find
            // an empty snapshot and quietly do nothing.
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
            // Read on this thread, before the spawn: the pick is a process
            // static, and this only needs the name it stores vectors under.
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
                    // Entries the library has no score for drop out of the
                    // ranking rather than sorting as zero, which would rate
                    // them above everything that genuinely sounds unalike.
                    // The library id rides along, because the song identity
                    // below is a tag question and a queue entry id is not a
                    // row.
                    let mut ranked: Vec<(u64, i64, f32)> = tail
                        .into_iter()
                        .filter_map(|(entry, id)| Some((entry, id, *scores.get(&id)?)))
                        .collect();
                    ranked.sort_by(|a, b| b.2.total_cmp(&a.2));
                    // Spread the version piles before the band picks off the
                    // front. Sorting by sound alone puts every recording of
                    // one song in a run, since by sound they are the same
                    // track, and pressing Next then walks that run: the
                    // studio cut, the live cut, the remaster, the second rip.
                    // Nothing is dropped, because this queue is the
                    // listener's; the copies just move down it.
                    let (playing, songs) = entry_songs(&conn, seed, &ranked);
                    let mut ids: Vec<u64> = ranked.into_iter().map(|(entry, _, _)| entry).collect();
                    song::space(&mut ids, SONG_SPACING, playing.as_deref(), |entry| {
                        songs.get(entry).map(String::as_str)
                    });
                    // The band is the skip pressure: strictly nearest with a
                    // width of one, and a widening handful of the nearest
                    // shuffled among themselves after that. Shuffling the
                    // head rather than picking one keeps the whole ranking
                    // intact behind it, so the queue past the next track
                    // still reads as "closest first".
                    shuffle_head(&mut ids, band);
                    Some(ids)
                })
                .await;
            let Some(ids) = ranked.filter(|ids: &Vec<u64>| !ids.is_empty()) else {
                // Nothing scoreable: an unanalyzed library, or a queue of
                // tracks the pass hasn't reached. The queue keeps the order
                // it had, which is the right answer, but say so rather than
                // leaving the mode looking broken.
                log::info!("shuffle: nothing analyzed to order the queue by");
                if from_skip {
                    this.update(cx, |this, cx| this.release_skip_reseed(cx))
                        .ok();
                }
                return;
            };
            this.update(cx, |this, cx| {
                // Still shuffling in the same mode? A toggle or a mode change
                // while the scan ran means this answer is for a queue nobody
                // asked about any more.
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

    /// Hand a skip's boundary claim back after its ranking came to nothing.
    /// When the boundary went by while the claim was held, that re-seed is
    /// still owed: the pump declined it for an order that never arrived, and
    /// without this the tail stays ranked against a track the listener already
    /// skipped past, for the rest of the queue.
    fn release_skip_reseed(&mut self, cx: &mut Context<Self>) {
        let (claim, owed) = self.skip_reseed.abandon();
        self.skip_reseed = claim;
        if owed && self.similar_order() {
            self.order_tail_by_similarity(1, None, false, cx);
        }
    }

    /// The seed track and the upcoming entries a similarity ordering works
    /// over, or None while the engine has yet to publish a queue or there is
    /// nothing ahead to order.
    ///
    /// Library ids off the pool mirror rather than paths off the snapshot: a
    /// path is not an identity once cue tracks exist, and resolving one back
    /// would score every track of a rip as whichever row sorted first. The
    /// mirror already holds the id the insert looked up, so this costs no
    /// query either. The seed stays optional so a track the library doesn't
    /// hold reads as nothing to score rather than as a queue that has yet to
    /// publish, the thing the caller's retry is waiting on.
    #[allow(clippy::type_complexity)]
    fn similarity_inputs(&self, leaving: Option<u64>) -> Option<(Option<i64>, Vec<(u64, i64)>)> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        // A skip seeds on where it goes, so it reads the track the engine
        // has taken on rather than the one still coming out of the speakers.
        // Under a crossfade those are different for half a window: the clock
        // flips at the midpoint (ADR 19), so on the default four seconds the
        // audible track is the one being left for two whole seconds after the
        // press, and steering that waited for the flip would give up first.
        //
        // The published cursor when nothing is audible yet, which is where a
        // freshly started context is: waiting for the first samples would
        // mean the ordering never ran for the case that needs it most.
        let at = leaving
            .and_then(|_| self.adopted_index(&snap))
            .or_else(|| self.audible_index(&snap))
            .unwrap_or(snap.cursor);
        let entry = snap.entries.get(at)?;
        // The engine takes the skip on from its own thread, so for the tries
        // before it gets there the seed is still the track being left and the
        // answer isn't ready to be computed.
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

    /// Whether stop-after-current is armed.
    pub fn stop_after(&self) -> bool {
        self.stop_after
    }

    /// Arm or clear stop-after-current: armed, the playing track ends the
    /// motion: the engine plays it out, pauses, and cues the next track.
    /// Sticky until cleared, and session-local.
    pub fn toggle_stop_after(&mut self, cx: &mut Context<Self>) {
        self.stop_after = !self.stop_after;
        self.send(Cmd::SetStopAfter(self.stop_after));
        cx.notify();
    }

    /// Where the A-B command stands, resolved the way everything else here
    /// is: the engine's published loop first, and the half-marked step off
    /// the player only when there's no loop and the mark still belongs to
    /// the track playing.
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

    /// The A-B command's three-step cycle: the first press marks A at the
    /// audible position, the second marks B and starts the section
    /// repeating, the third clears it.
    ///
    /// A second press too close to the first drops the mark and starts the
    /// cycle over: the engine refuses a section that short, and leaving A
    /// half-set after a double-tap would put the next press on B without
    /// the listener knowing where A went.
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

    /// Drop the section and the half-marked step with it, wherever the
    /// cycle had got to. The direct way out, for a caller that shouldn't
    /// have to step the cycle to its end to get there.
    pub fn ab_clear(&mut self, cx: &mut Context<Self>) {
        self.ab_pending_a = None;
        self.send(Cmd::SetAbLoop(None));
        cx.notify();
    }

    /// Set a section outright, both ends in track seconds, the way the
    /// control socket does it: no cycle to step, no half-marked state to
    /// leave behind. Refused with a reason when there's nothing playing or
    /// the section isn't one the engine would take, so the caller hears
    /// why instead of watching nothing happen.
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

    /// Arm stop-after once `after` has passed, or clear a timer that's
    /// already running. The track playing when it fires plays out and the
    /// next one cues paused, which is stop-after's behavior and the reason
    /// this arms that rather than inventing a second way to end playback.
    ///
    /// Deliberately unaffected by pause: a paused player with a timer set
    /// still stops later, because that's what was asked for.
    pub fn set_sleep(&mut self, after: Option<Duration>, cx: &mut Context<Self>) {
        self.sleep = after.map(|after| Instant::now() + after);
        cx.notify();
    }

    /// How long the timer has left, None when none is set. Saturating, so
    /// a timer the pump hasn't got to yet reads zero rather than wrapping.
    pub fn sleep_remaining(&self) -> Option<Duration> {
        let now = Instant::now();
        self.sleep
            .map(|ends_at| ends_at.saturating_duration_since(now))
    }

    /// One pump tick's worth of sleep timer. Arms stop-after if it isn't
    /// armed already, through the toggle, so the engine and the transport
    /// button both learn; the guard is what keeps a timer landing on a
    /// stop the listener armed by hand from turning it back off.
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

    /// Put the reason the engine gave up on an entry onto the error line.
    ///
    /// The queue moves on by itself when a track won't open, which is the
    /// right thing to do and also the reason nobody sees why: the entry
    /// stops being the playing one and the next one opens over the top of
    /// it. The line is the one place a failure already gets said out loud,
    /// so a refused stream says it there.
    ///
    /// It clears where everything else on this line clears: the next
    /// session start, and stop. A mid-queue skip is neither, so the reason
    /// stays up for the rest of the queue instead of blinking past on the
    /// frame the next track opens.
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

    /// A snapshot of the discrete state, without the position clock. What
    /// [`observe_view`] diffs to decide whether a tick is worth a repaint.
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

/// The equalizer's live parameters (ADR 19), one set for the whole process.
/// The curve is an app preference rather than something a session owns, so
/// every chain that opens uses this same handle and a band moves under
/// whatever is playing without anyone holding a player. Seeded off the
/// settings file the first time something asks.
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

/// Touched by every EQ setter, so the surfaces drawing the curve wake on a
/// move instead of watching for one. It holds nothing, because there's nothing
/// worth holding: whoever gets woken reads the parameters back. Process-global
/// like they are, which lets a band dragged in the EQ window repaint a
/// widget in some other workspace's transport row.
#[derive(Default)]
pub struct EqChanged;

impl Global for EqChanged {}

/// Tell the curve's watchers something moved. Taking the global mutably is the
/// whole notification: gpui wakes its observers off the borrow.
fn eq_changed(cx: &mut App) {
    let _ = cx.default_global::<EqChanged>();
}

/// Wake `view` whenever the curve moves, wherever it moved from. The EQ's
/// [`observe_view`], minus the diff: the parameters are atomics with no gpui
/// entity behind them, so the setters are the only place a change is known.
pub fn observe_eq<V: 'static>(cx: &mut Context<V>) -> Subscription {
    cx.observe_global::<EqChanged>(|_, cx| cx.notify())
}

/// Whether the equalizer shapes the output.
pub fn eq_enabled() -> bool {
    eq_params().enabled()
}

/// One band's gain in dB, in [`rox_playback::eq::BAND_HZ`] order.
pub fn eq_gain(band: usize) -> f32 {
    eq_params().gain(band)
}

/// Turn the equalizer on or off and persist the pick. The node stays in
/// the chain either way and hands its buffer back untouched while it's
/// off, so this is a store rather than a chain edit; it takes effect as soon as
/// the ring drains past it, up to half a second behind the click.
pub fn set_eq_enabled(on: bool, cx: &mut App) {
    eq_params().set_enabled(on);
    Settings::update(move |s| s.eq.enabled = on);
    eq_changed(cx);
}

/// Move one band, in dB. The decode thread reads the store on its next
/// buffer; the file write waits for the drag to settle.
pub fn set_eq_gain(band: usize, db: f32, cx: &mut App) {
    eq_params().set_gain(band, db);
    persist_eq_soon(cx);
    eq_changed(cx);
}

/// Every band back to 0 dB, which is also the point where the EQ stops
/// touching the samples at all.
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

/// Move a band's center. Same store-then-settle shape the gain has: the
/// atomic gets it to the decode thread now, the file write waits for the
/// drag to stop.
pub fn set_eq_freq(band: usize, hz: f32, cx: &mut App) {
    eq_params().set_freq(band, hz);
    persist_eq_soon(cx);
    eq_changed(cx);
}

/// Widen or narrow a band.
pub fn set_eq_q(band: usize, q: f32, cx: &mut App) {
    eq_params().set_q(band, q);
    persist_eq_soon(cx);
    eq_changed(cx);
}

/// One band back to where it started: its ISO octave, flat, one octave
/// wide. The double-click on a handle, so a band dragged somewhere useless
/// can be put back without hunting for the numbers it had.
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

/// Apply a full graphic EQ curve (10 band gains), resetting band centers and widths
/// to their ISO octave positions, persisting the settings, and notifying observers.
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

/// Put a whole curve in place at once, each band as its center in Hz, its
/// gain in dB and its width. What a saved preset applies through, where
/// [`apply_graphic_eq`] is for the ten fixed octaves a headphone profile
/// arrives as: a curve someone shaped in the window has bands that moved
/// and narrowed, and those numbers have to come back with the gains or the
/// preset isn't the curve they saved.
///
/// A list longer than the engine holds is cut off at the end, and bands it
/// doesn't reach stay where they are.
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

/// Persist the curve once the drag settles, the same shape
/// [`Player::persist_volume_soon`] uses: the atomics already have the
/// value on the audio thread, so only the file write has to wait for the
/// last tick of a slider burst. The generation is global because the
/// parameters are: whoever is dragging, the write they race is the same one.
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

/// Observe the player, but wake the host view only when its discrete state
/// changes, not on every pump tick. The seek strip, waveform, and spectrum
/// need each tick (the clock, the playhead, the bars) and observe the
/// player directly; everything else goes through this so a playing session does
/// not repaint them 60 times a second for a clock they never draw.
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

/// [`observe_view`] for the output state instead: wakes on a stream rebuild
/// and on a track whose rate differs, nothing else. Its own subscription
/// rather than a field on [`PlayerView`], because only the settings window
/// draws this and the comparison costs a lock the transport panels have no
/// reason to pay 60 times a second.
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

    /// A library of tagged rows, ids in insertion order. Enough of the
    /// schema for the song-identity lookups; nothing here scores anything.
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

    /// The whole point of the A-B command is that one key does three
    /// things, so the cycle is what gets tested: mark, mark, clear, and
    /// round again from a clean slate.
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

    /// A section set outright gets its ends put in order and the same
    /// floor the cycle applies, so the socket can't hand the engine a
    /// section the button couldn't have made.
    #[test]
    fn a_section_set_outright_is_ordered_and_floored() {
        assert_eq!(ab_section(12.0, 18.5), Some((12.0, 18.5)));
        assert_eq!(ab_section(18.5, 12.0), Some((12.0, 18.5)));
        assert_eq!(ab_section(12.0, 12.1), None);
        assert_eq!(ab_section(12.0, 12.0), None);
        assert_eq!(ab_section(-1.0, 5.0), None);
        assert_eq!(ab_section(f64::NAN, 5.0), None);
    }

    /// The timer fires at the moment it named and not a tick before, and
    /// an unset one never fires at all. The boundary is the whole test:
    /// a strictly-greater compare would leave a timer that landed exactly
    /// on a tick waiting a further 16 ms, and a set-but-unfired timer that
    /// read as due would arm the stop the instant it was picked.
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

    /// Arming is a toggle, so a timer landing on a stop the listener
    /// already armed by hand has to leave it alone. Firing it anyway would
    /// turn stop-after off at exactly the minute it was supposed to come
    /// on, and the track after this one would keep playing all night.
    #[test]
    fn a_fired_sleep_timer_never_disarms_a_stop_already_set() {
        let now = Instant::now();
        assert_eq!(sleep_step(now, Some(now), false), SleepStep::Arm);
        assert_eq!(sleep_step(now, Some(now), true), SleepStep::Clear);
    }

    /// B on top of A, which is what a double-tap looks like. The section
    /// would be too short to play, so the press drops the mark instead of
    /// sending the engine something it refuses.
    #[test]
    fn a_second_mark_too_close_to_the_first_drops_it() {
        assert_eq!(ab_step(AbState::ASet(12.0), 12.05), AbStep::Nothing);
        // And B behind A, from a seek backwards between the two presses.
        assert_eq!(ab_step(AbState::ASet(12.0), 4.0), AbStep::Nothing);
    }

    /// The Barracuda case for Play Similar: the ranking is the seed's own
    /// song over and over, and the press has to land somewhere else.
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
        // Nearest first, which is what the vectors really do here: every
        // copy scores above the two tracks that aren't the same song.
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

    /// A song the session already heard a version of is out of the band too,
    /// which is what keeps pressing Play Similar from circling back.
    #[test]
    fn a_similar_draw_skips_a_song_the_session_already_heard() {
        let (conn, ids) = tagged(&[
            ("Heart", "Barracuda"),
            ("Heart", "Crazy On You (Live)"),
            ("Heart", "Magic Man"),
        ]);
        let near: Vec<(i64, f32)> = vec![(ids[1], 0.9), (ids[2], 0.8)];
        // The studio "Crazy On You" isn't in this library at all; the
        // session heard it as the live take's identity, which is the point.
        let seen: HashSet<i64> = [ids[1]].into_iter().collect();
        let band = one_per_song(&conn, ids[0], &seen, &near);
        assert_eq!(
            band.iter().map(|&(id, _)| id).collect::<Vec<_>>(),
            vec![ids[2]]
        );
    }

    /// A neighbourhood that is one song and nothing else still answers the
    /// press. The guard passes tracks over, it never eats the button.
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

    /// The ordering side reads identities per queue entry, and the playing
    /// track's own identity comes back with them so the spacing knows what
    /// it must not follow.
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

    /// No skips is the strict nearest; a run widens fast enough that a few
    /// presses reach past a genre rather than inching through it.
    #[test]
    fn the_band_opens_up_with_each_skip_in_a_run() {
        assert_eq!(skip_band(0), 1, "settled radio plays the nearest track");
        assert_eq!(skip_band(1), 4);
        assert_eq!(skip_band(2), 16);
        assert_eq!(skip_band(3), 64);
        // Monotonic, and it saturates rather than overflowing on a listener
        // who holds the skip button down.
        let mut last = 0;
        for skips in 0..64 {
            let band = skip_band(skips);
            assert!(band >= last, "band never narrows mid-run");
            last = band;
        }
    }

    /// The band shuffles the head and leaves the ranking behind it alone, so
    /// what plays next is drawn from the nearest few while the rest of the
    /// queue still reads closest-first.
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

    /// The Random button reads the same scope continuation does, so a press
    /// inside a playlist or a browse view draws from that list rather than
    /// the whole library.
    #[test]
    fn a_random_draw_stays_inside_the_playing_view() {
        let all = vec![1, 2, 3, 4, 5];
        let view = continuation::Scope::View(vec![7, 8].into());
        assert_eq!(random_pool(&view, &all), &[7, 8]);
        // Nothing named a list, so the pool is the library.
        assert_eq!(
            random_pool(&continuation::Scope::Library, &all),
            all.as_slice()
        );
        // A one-track view is still the pool: pressing Random in it plays
        // that track back rather than jumping out to the library.
        let single = continuation::Scope::View(vec![7].into());
        assert_eq!(random_pool(&single, &all), &[7]);
    }

    /// A view that came back empty is no context at all, and drawing from an
    /// empty pool would leave the press doing nothing.
    #[test]
    fn an_empty_view_falls_back_to_the_library() {
        let all = vec![1, 2, 3];
        let empty = continuation::Scope::View(Vec::new().into());
        assert_eq!(random_pool(&empty, &all), all.as_slice());
        // An empty library on top of it has nothing to offer either way.
        assert!(random_pool(&empty, &[]).is_empty());
    }

    /// A random press plays the run around what it drew, so the track it
    /// drew is always inside the window and the entries after it are
    /// there to keep playing into. The window slides rather than shrinking, so a
    /// draw near either end still comes with a full budget of music.
    #[test]
    fn a_random_draw_brings_the_run_around_it() {
        // A pool under the cap comes back whole, wherever the draw fell.
        assert_eq!(run_window(0, 40), (0, 40));
        assert_eq!(run_window(39, 40), (0, 40));

        let len = QUEUE_CAP * 3;
        // Room on both sides: half the budget behind the draw for Prev.
        let (lo, hi) = run_window(len / 2, len);
        assert_eq!((lo, hi), (len / 2 - QUEUE_CAP / 2, len / 2 + QUEUE_CAP / 2));
        // Against the front, the window starts at the top of the pool rather
        // than reaching behind it.
        assert_eq!(run_window(10, len), (0, QUEUE_CAP));
        // Against the back, it slides forward instead of coming up short.
        let (lo, hi) = run_window(len - 1, len);
        assert_eq!((lo, hi), (len - QUEUE_CAP, len));
        // Whichever way it slid, the draw is inside it.
        for at in [0, 1, QUEUE_CAP, len / 2, len - 2, len - 1] {
            let (lo, hi) = run_window(at, len);
            assert!(
                lo <= at && at < hi,
                "the draw at {at} sits outside {lo}..{hi}"
            );
            assert_eq!(hi - lo, QUEUE_CAP, "a full window either side of {at}");
        }
    }

    /// The Random button draws outside what the session has already held,
    /// and only wraps back onto it once the whole pool has been heard: the
    /// providers' no-repeats promise, kept by the press that starts sessions.
    #[test]
    fn a_random_draw_avoids_what_the_session_has_held() {
        let pool = vec![1, 2, 3, 4, 5];
        let seen: HashSet<i64> = [1, 2, 4, 5].into_iter().collect();
        for _ in 0..32 {
            assert_eq!(draw_at(&pool, &seen), Some(2), "3 is the one fresh track");
        }
        // Everything heard: the pool opens back up rather than eating the
        // press, and any index is fair again.
        let all: HashSet<i64> = pool.iter().copied().collect();
        for _ in 0..32 {
            let at = draw_at(&pool, &all).expect("an exhausted pool still draws");
            assert!(at < pool.len());
        }
        // An empty pool is the one case with nothing to play.
        assert_eq!(draw_at(&[], &seen), None);
    }

    /// The similar band takes the session's plays out of the draw and comes
    /// back whole once every neighbour has been heard, without ever widening
    /// past the tracks that actually sound like the seed.
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

    /// The bounce a pool-only guard allows: press Play Similar on track A,
    /// get its neighbour B, press again, and A is back because the new
    /// session's pool never held it. The heard ring keeps A across the
    /// swap, so the second press moves on down the neighbourhood instead.
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

    /// The ring stays deduped and bounded: a track held again moves to the
    /// young end rather than aging out from where it first entered, and past the
    /// cap the oldest fall off first.
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

    /// Every index the draw can produce is inside the pool, which is the one
    /// thing the hasher trick could get wrong.
    #[test]
    fn a_random_index_lands_inside_the_pool() {
        for len in 1..16 {
            for _ in 0..64 {
                assert!(random_index(len) < len);
            }
        }
    }

    /// The continuation trigger's arithmetic: it fires within the floor of
    /// the end of the upcoming portion and stays quiet above it, whichever
    /// end of the order the cursor is at.
    #[test]
    fn the_trigger_fires_inside_the_floor_and_not_above_it() {
        // Nineteen still to come, nothing to do.
        assert!(!queue_running_dry(19, LoopMode::Off));
        // Three to go is one over the floor, two is the floor itself.
        assert!(!queue_running_dry(3, LoopMode::Off));
        assert!(queue_running_dry(2, LoopMode::Off));
        assert!(queue_running_dry(1, LoopMode::Off));
        // Standing on the last entry, which is also where a queue that
        // played out to its end ends up.
        assert!(queue_running_dry(0, LoopMode::Off));
    }

    /// The other half of the trigger's gate: a paused queue doesn't grow,
    /// an ended one still takes a batch because it reads as playing, and an
    /// armed stop-after means stop however the session reads.
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

    /// Turning the Similar order on is turning radio on, so it asks for a
    /// batch there and then instead of leaving the listener with the queue
    /// they had until it runs down to the floor.
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
        // The random order refills from wherever the mode points, at the
        // floor, exactly as it did before.
        assert!(!similar_draw_now(
            continuation::Mode::Continue,
            false,
            true,
            false
        ));
    }

    /// Off still means off, which is the same call `continuation::provider`
    /// makes: a queue told to end must not start growing because the shuffle
    /// order changed under it. Nor may a paused queue or an armed stop-after.
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

    /// A track that ended on its own re-seeds the ordering on what's playing
    /// now, so a batch played through doesn't walk down a ranking made
    /// against a track four songs ago.
    #[test]
    fn a_natural_boundary_reseeds_the_ordering() {
        assert!(reseed_at_boundary(true, false, 7, Some(6)));
        // Same track, same tick, nothing owed. This is the case that runs
        // sixty times a second.
        assert!(!reseed_at_boundary(true, false, 7, Some(7)));
        // Another order is another question; nothing to re-rank.
        assert!(!reseed_at_boundary(false, false, 7, Some(6)));
        // A session that has only just come up orders its own tail, so the
        // first tick adopts the marker rather than asking again.
        assert!(!reseed_at_boundary(true, false, 0, None));
    }

    /// A skip already ranked where it landed, at the band its run widened, so
    /// the boundary that follows leaves it alone: re-ranking at a band of one
    /// would throw the widening away and turn steering back into drifting.
    #[test]
    fn a_skip_pays_for_the_boundary_it_causes() {
        assert!(!reseed_at_boundary(true, true, 7, Some(6)));
        // One boundary each. The next track to end on its own gets the
        // re-seed, since the flag is spent at the first change it sees.
        assert!(reseed_at_boundary(true, false, 8, Some(7)));
    }

    /// The usual order of events on a skip: the pump sees the boundary while
    /// the ranking is still being computed, so the claim covers it there and
    /// the ranking that lands afterwards owes nothing to the boundary after
    /// that one.
    #[test]
    fn a_skips_claim_covers_the_boundary_that_beats_it() {
        let claim = SkipReseed::InFlight { passed: false };
        let (claim, paid) = claim.spend();
        assert!(paid, "the boundary is the skip's, ranked or not yet");
        assert_eq!(claim, SkipReseed::InFlight { passed: true });
        // The ranking arrives after the fact and reorders the tail, which is
        // the answer to that same boundary. Nothing is held over.
        assert_eq!(claim.landed(), SkipReseed::Idle);
        assert!(!SkipReseed::Idle.spend().1, "so the next one re-seeds");
    }

    /// The other way round, where the ranking is quick enough to go out
    /// first: the claim waits for the boundary and is spent there.
    #[test]
    fn a_skips_claim_waits_when_the_ranking_lands_first() {
        let claim = SkipReseed::InFlight { passed: false }.landed();
        assert_eq!(claim, SkipReseed::Ready);
        let (claim, paid) = claim.spend();
        assert!(paid, "already ranked at the band the run earned");
        assert_eq!(claim, SkipReseed::Idle, "and only for the one boundary");
    }

    /// The bug the claim's release exists for: every give-up inside the
    /// ranking returns without touching the queue. A boundary held off for
    /// one of those is owed its re-seed, or the tail stays ranked against the
    /// track the listener skipped away from.
    #[test]
    fn a_ranking_that_gives_up_hands_the_boundary_back() {
        let (claim, owed) = SkipReseed::InFlight { passed: true }.abandon();
        assert!(owed, "the pump declined for an order that never arrived");
        assert_eq!(claim, SkipReseed::Idle);
        // Given up before the boundary, there's nothing to make good: the
        // claim is gone, so the boundary re-seeds itself when it arrives.
        let (claim, owed) = SkipReseed::InFlight { passed: false }.abandon();
        assert!(!owed);
        assert!(!claim.spend().1);
    }

    /// Loop is the user saying remain here, so the trigger never fires while
    /// one is on however short the queue has run.
    #[test]
    fn loop_suppresses_the_trigger_at_any_distance() {
        for mode in [LoopMode::All, LoopMode::One] {
            assert!(!queue_running_dry(0, mode));
            assert!(!queue_running_dry(1, mode));
            assert!(!queue_running_dry(19, mode));
        }
    }

    /// A band of one, the settled radio's band, must not disturb
    /// the ranking at all; nor may a band wider than the queue panic.
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

    /// The pinned crossfade requirement (ADR 19): consecutive cue tracks of
    /// one image have to end up in the same album group, or the fade would run
    /// over a splice that was gapless on the disc.
    ///
    /// It falls out of the library rather than needing a rule here: the group
    /// is hashed from (album artist, album), and the scanner writes the
    /// sheet's album onto every row it cuts. This pins that, so a change to
    /// how groups are derived can't quietly start fading mid-record.
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

        // The spans come back beside the groups, which the engine needs to
        // play a slice rather than the whole image.
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

    /// Nothing to ask means everything defaults, one entry per key, so a
    /// player with no database still queues and plays.
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

    /// A remote row's locator is finished by its own source before the
    /// engine ever sees it: headers set and URL signed, per request.
    ///
    /// This is the shape the Subsonic blocker broke. The signing existed,
    /// the resolve path only asked for headers, and Subsonic authorizes in
    /// the query string, so every remote track went out with a username and
    /// nothing to prove it and every server answered error 40.
    #[test]
    fn a_remote_key_resolves_through_the_source_that_authorizes_it() {
        let mut conn = rox_library::rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();

        let source = "test:resolve-authorizes";
        let mut row = album_row("sg-1", "Album");
        row.remote_url = "https://srv/rest/stream.view?id=sg-1&format=raw".into();
        store::upsert_source_rows(&mut conn, source, &[row]).unwrap();

        // Stands in for the Subsonic server the binary installs at startup:
        // one that rewrites the URL, which is the half a header table alone
        // could never carry.
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

        // A queued row the source has since pruned has no URL to finish, so
        // the open fails on the missing row rather than on a request built
        // out of an empty string.
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

    /// A row on one album, for the group compares above.
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

    /// One track of a cue rip: the same image path, its own subsong number
    /// and span, and the sheet's album on every row.
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

    /// The three readings of an elapsed clock: a station mid-song, a
    /// station that hasn't named one, and the tick where the position
    /// hasn't caught up with the start yet.
    #[test]
    fn the_song_clock_counts_the_song_not_the_listen() {
        assert_eq!(song_clock(930.0, Some(870.0)), 60.0);
        assert_eq!(song_clock(930.0, None), 930.0);
        assert_eq!(song_clock(869.5, Some(870.0)), 0.0);
    }

    /// A step back through the buffer lands in the middle of a song, and
    /// the clock has to read the middle of it. The listen clock keeps
    /// running forward through a seek, so the start it counts from is
    /// whatever the buffer's title marks say, not where the pump happened
    /// to watch the song turn over.
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

        // Forty-five seconds into the song, on a listen that has run for
        // fifteen and a half minutes.
        let start = song_start_of(930.0, shift(Some(45.0)), Some(870.0));
        assert_eq!(song_clock(930.0, start), 45.0);

        // Nothing announced behind the playhead yet: what the pump saw is
        // all there is, and it still answers.
        let start = song_start_of(930.0, shift(None), Some(870.0));
        assert_eq!(song_clock(930.0, start), 60.0);

        // Neither, which is a file or a station that has said nothing.
        assert_eq!(song_start_of(930.0, None, None), None);
    }

    /// The arrow keys on a station. A five second step walks the buffer,
    /// and both ends of it hold: the live edge is as far forward as a
    /// broadcast goes, and the oldest second held is as far back as the
    /// tape does.
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

        // Mid-buffer, where the step is just the step.
        assert_eq!(live_step_target(5.0, &at(300.0)), Some(295.0));
        assert_eq!(live_step_target(-5.0, &at(300.0)), Some(305.0));

        // Forward from closer than the step: live, not past it.
        assert_eq!(live_step_target(5.0, &at(3.0)), Some(0.0));

        // Back from deeper than the tape holds: the oldest second in it.
        assert_eq!(live_step_target(-5.0, &at(597.0)), Some(600.0));

        // Standing on either end, pressing into it. Nothing to send: the
        // cursor is already where the step points.
        assert_eq!(live_step_target(5.0, &at(0.0)), None);
        assert_eq!(live_step_target(-5.0, &at(600.0)), None);

        // And a step too small for the tape to land anywhere different,
        // which is what the fine step keys come to on a broadcast.
        assert_eq!(live_step_target(-0.025, &at(300.0)), None);
    }

    /// What the pump asks on a title it just read. The rejoin case is the
    /// one with teeth: a pause that comes back to the same song must not
    /// restart its clock.
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

        // A different entry is a different stream, so the same song text
        // over there starts a clock of its own.
        assert!(starts_new_song(
            Some(&standing),
            4,
            &title("Boards of Canada", "Roygbiv")
        ));
    }

    /// The rule a station's synced lyrics hang off: only a song we heard
    /// begin can be timed against.
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

        // The title the stream opened carrying: a mid-song join, so the
        // clock says nothing about where in the song we are.
        assert!(!song_heard_from_start(Some(&record(false)), true, 3, 900.0));

        // Stepped back behind the turnover, into the song before it.
        assert!(!song_heard_from_start(Some(&record(true)), true, 3, 800.0));

        // Another entry's boundary, and no station playing at all.
        assert!(!song_heard_from_start(Some(&record(true)), true, 4, 900.0));
        assert!(!song_heard_from_start(Some(&record(true)), false, 3, 900.0));
        assert!(!song_heard_from_start(None, true, 3, 900.0));
    }

    /// A station key as the library files one: the stream URL under the
    /// radio source.
    fn station(url: &str) -> TrackKey {
        TrackKey {
            source: rox_library::cue::source_id(rox_library::stations::SOURCE),
            path: PathBuf::from(url),
            sub: 0,
        }
    }

    /// A remote locator, live or not, since the difference is the whole
    /// question a restore asks about one.
    fn remote(url: &str, live: bool) -> Locator {
        Locator::Remote(rox_library::locator::Remote {
            url: url.into(),
            headers: Vec::new(),
            hint: String::new(),
            live,
        })
    }

    /// What a paused restore does with the position it saved. A station has
    /// nowhere to put it, and a file served off a server is still a file.
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

    /// Which batches take the queue's place. Only a batch that is stations
    /// the whole way through: a station with files around it is a list, and
    /// a list plays the way lists have always played.
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
