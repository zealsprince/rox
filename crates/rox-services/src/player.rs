//! The playback service entity: one running engine session behind the
//! playback contract (commands in over a channel, state out through shared
//! atomics). The PCM tap is drained by a headless pump task on a timer, not
//! by any render pass, so the audio views' feed keeps flowing no matter
//! which windows are drawing - popped-out panels, a zoomed dock, a
//! minimized main window. The player renders nothing itself; the transport
//! panels are the UI over this state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, Global, SharedString, Subscription, Task};

use rox_core::settings::{
    GainModeSetting, ReplayGainSave, ReplayGainSettings, Settings, ShuffleMode,
};
use rox_library::embeddings;
use rox_library::store;
use rox_playback::continuation::{self, Pick};
use rox_playback::engine::{self, shuffle_head, shuffle_slice, Cmd, LoopMode, StartQueue};
use rox_playback::eq::{Eq, EqParams};
use rox_playback::gain;
use rox_playback::output::{self, Mode, Negotiated, Request};
use rox_playback::rtrb::Consumer;
use rox_playback::shared::{QueueEntry, QueueSnapshot, Shared};
use rox_viz::AudioFeed;

use crate::catalog::Library;

// The clock formatters live with the rest of the readouts in rox-core now.
// Callers still reach them through the player, where the clock is.
pub use rox_core::fmt::{fmt_time, fmt_time_padded};

/// Pump cadence, roughly one video frame. The tap ring holds 16,384 samples
/// (about 170 ms at 48 kHz stereo), so a tick has an order of magnitude of
/// headroom before the callback's pushes start getting dropped.
const PUMP_INTERVAL: Duration = Duration::from_millis(16);

/// How long the similarity ordering will wait for a freshly started context
/// to publish its queue, as a number of tries and the gap between them. The
/// decode thread publishes first thing in `run`, so in practice this lands on
/// the first or second look; the ceiling is only there so a session that
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
/// skip draws from; there is no pass that walks it back while you listen.
const SKIP_SETTLE: Duration = Duration::from_secs(30);

/// The band a skip draws the next track from, as a count of the nearest
/// entries shuffled among themselves. One skip loosens to a handful, and each
/// one after multiplies, so a few in a row walks out of a genre rather than
/// inching down it one track at a time. No skips at all means the strict
/// nearest.
const SKIP_BAND_BASE: usize = 4;
const SKIP_BAND_GROWTH: usize = 4;

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

/// One random entry of `pool` resolved to a playable path. None when the
/// pool is empty or the id it landed on has no file behind it any more.
fn draw_one(library: &Library, pool: &[i64]) -> Option<Vec<PathBuf>> {
    if pool.is_empty() {
        return None;
    }
    let id = pool[random_index(pool.len())];
    library.paths_for(&[id]).ok().filter(|p| !p.is_empty())
}

/// Whether the queue has run close enough to its end to ask for more
/// (ADR 17): `upcoming` tracks sit ahead of the audible one, against the
/// floor the trigger insists on keeping.
///
/// Loop is the whole of the suppression rule, and it's here rather than at
/// the call site because it's part of the same decision: loop is the user
/// saying remain here, which narrows the selection range to the list that
/// already exists.
fn queue_running_dry(upcoming: usize, loop_mode: LoopMode) -> bool {
    loop_mode == LoopMode::Off && upcoming <= continuation::FLOOR
}

/// Whether a session in this state is one continuation should be feeding
/// (ADR 17). A paused queue refuses to grow, which is what keeps the launch
/// restore from growing a queue nobody has pressed play on yet; a queue that
/// played through to its end still reads as playing, which is how an ended
/// session gets woken by the batch that lands behind it.
///
/// An armed stop-after is the one thing that pauses on its own, and it means
/// stop, so the queue stays as it is until the listener says otherwise. It
/// stays armed after the stop lands, so this keeps refusing until they clear
/// it, which is the same stickiness the transport button has.
fn continuation_wanted(playing: bool, stop_after: bool) -> bool {
    playing && !stop_after
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
    _stream: Box<dyn output::OutputStream>,
    device_rate: u32,
    /// What the output layer actually got, as opposed to what was asked
    /// for. The Audio page reads this, and the rate follow compares against
    /// it, so neither is going on the setting's word.
    negotiated: output::Negotiated,
    /// The queued paths, kept so the views can resolve the playing track
    /// back to its file.
    queue: Vec<PathBuf>,
    /// The ReplayGain tags handed to the engine, in the same pool order as
    /// `queue`. Kept so the status readout can say what the playing file is
    /// actually being levelled by rather than what the setting is set to.
    gains: Vec<gain::ReplayGain>,
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    fn start(
        queue: StartQueue,
        volume: f32,
        loop_mode: LoopMode,
        shuffle: Option<bool>,
        stop_after: bool,
        paused_at: Option<f64>,
        crossfade: (f32, bool),
        rule: gain::GainRule,
        output: output::Request,
    ) -> Result<Session, String> {
        let shared = Arc::new(Shared::new(queue.paths.len()));
        // Seed the session with the persisted playback state: volume lands
        // in the shared atomics before the stream opens, the loop and
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
        // An armed stop-after carries into the fresh session, so queueing a
        // new context does not silently disarm it.
        if stop_after {
            let _ = tx.send(Cmd::SetStopAfter(true));
        }
        // The launch restore's seek and pause queue here too, ahead of the
        // decode thread: the engine drains commands before it decodes, so
        // the session comes up already paused at the position and nothing
        // sounds.
        if let Some(secs) = paused_at {
            let _ = tx.send(Cmd::Seek(secs));
            let _ = tx.send(Cmd::TogglePause);
        }
        // The fade settings ride ahead of the first decode too, so a
        // session that starts on a skip already knows what to do at its
        // first boundary.
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
        // ever carries for the chain: the bands are atomics on the shared
        // handle, so every later turn of a knob is a store.
        let _ = tx.send(Cmd::ChainPush(Box::new(Eq::new(eq_params().clone()))));
        let paths = queue.paths.clone();
        let gains = queue.gains.clone();
        let engine = engine::Engine::new(queue, shared.clone(), out.producer, device_rate, rx);
        std::thread::Builder::new()
            .name("decode".into())
            .spawn(move || engine.run())
            .map_err(|e| format!("spawn decode thread: {e}"))?;
        Ok(Session {
            shared,
            tx,
            tap: out.tap,
            _stream: out.stream,
            device_rate,
            negotiated: out.negotiated,
            queue: paths,
            gains,
        })
    }
}

/// The library lookup for a batch of paths on their way into the queue, one
/// entry per path in the order they were asked about.
struct QueueMeta {
    groups: Vec<Option<u64>>,
    gains: Vec<gain::ReplayGain>,
    ids: Vec<Option<i64>>,
}

/// A snapshot of the playing track for the audio views: which file and
/// where the position clock sits. Whether audio is actually moving is what
/// the tap says, so the views read that from the feed instead.
#[derive(Clone)]
pub struct NowPlaying {
    pub path: PathBuf,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    /// Pool index of the audible track, off the position clock. The queue
    /// resolver matches entries on this rather than the path, so a file that
    /// sits in the order more than once lands on the occurrence playing now.
    pub audible_idx: usize,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
    }
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

/// The player's discrete state: everything the controls and info panels
/// draw that changes on a user action or a track change, never on the bare
/// position tick. The position clock is deliberately left out, so a panel
/// gating on this does not wake for it. See [`observe_view`].
#[derive(Clone, PartialEq)]
pub struct PlayerView {
    pub track: Option<PathBuf>,
    pub duration_secs: Option<f64>,
    pub playing: bool,
    pub active: bool,
    pub ended: bool,
    pub loop_mode: LoopMode,
    pub shuffle: bool,
    /// Which order shuffle is in. Here beside the flag because the transport
    /// button's glyph follows it, and the Behavior page can move it from
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
    pub muted: bool,
    pub volume: f32,
    pub error: Option<SharedString>,
    /// The crossfade the ear is in, quantized so a gated observer wakes
    /// once per visible step. None the rest of the time, which is a
    /// comparison that costs nothing on a settled session.
    pub fade: Option<FadeView>,
}

/// What output actually ended up doing, for the Audio page to state instead
/// of echoing the settings back. ADR 19's bit-perfect claim rests on three
/// conditions, and the two this can speak to are here: which mode is
/// running, and whether the device rate matches the file's.
#[derive(Clone, PartialEq)]
pub struct OutputStatus {
    pub negotiated: Negotiated,
    /// The playing file's own rate. None before a track has opened, which
    /// is also the only honest answer then.
    pub source_rate: Option<u32>,
    /// What ReplayGain is actually doing to the playing file, in dB. None
    /// when the samples reach the ring untouched: leveling off, or on with
    /// nothing to apply, which is what an untagged file with no fallback
    /// set comes to. Not a fault when it is set, but it's processing, and
    /// the readout would be claiming the file's own samples without saying
    /// so.
    pub leveling_db: Option<f32>,
}

/// A queue snapshot for the close-time persist: every entry's path and
/// explicit flag, the audible cursor, and the position clock in seconds.
pub type QueueStatePersist = (Vec<(PathBuf, bool)>, usize, f64);

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
    /// Deliberately not persisted: an armed stop that survived a restart
    /// would read as a broken player days later.
    stop_after: bool,
    /// Skips in a row under the similarity mode, and when the last one
    /// landed. Together they widen the band the radio draws from: skip
    /// repeatedly and it reaches further from the seed each time, so a few
    /// presses walk out of a genre. Listening for [`SKIP_SETTLE`] without
    /// skipping counts as settling and the next skip starts from narrow
    /// again. Session-local, like the stop above: yesterday's impatience
    /// should not steer today's radio.
    similar_skips: u32,
    last_skip: Option<Instant>,
    /// The rate the next stream asks for, exclusive mode's rate follow
    /// (ADR 19). Holds whatever the last stream negotiated, so a rebuild
    /// comes back up on the rate it went down on instead of dropping to the
    /// device default and following its way back; the pump moves it to the
    /// playing file's rate when the two disagree. None until a stream has
    /// opened, which is when the device's own default answers.
    follow_rate: Option<u32>,
    /// Rates the device already turned down. The follow asks once per rate
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
    /// A continuation query is out. The pump fires on a 16 ms clock and a
    /// provider takes tens of milliseconds, so without this one dry-out would
    /// queue a few dozen of them.
    continuing: bool,
    /// The queue revision the last continuation fired at. The guard above
    /// covers the query; this covers what comes after it. A batch that landed
    /// moves the revision, so the next tick sees a full queue and stays quiet;
    /// an empty batch doesn't, and this is what stops the pump asking the same
    /// exhausted provider sixty times a second.
    continued_rev: Option<u64>,
    /// The strategy the continuation toggle turns back on. Continuation is a
    /// mode with an off state rather than a switch beside a mode, so the
    /// transport's press has to remember what it turned off. Session-local:
    /// the persisted pick is the mode itself.
    last_continuation: continuation::Mode,
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
            similar_skips: 0,
            last_skip: None,
            follow_rate: None,
            refused_rates: Vec::new(),
            scope: continuation::Scope::default(),
            pool_ids: Vec::new(),
            continuing: false,
            continued_rev: None,
            last_continuation,
        }
    }

    /// What the engine needs per queued path beyond the path itself: the
    /// album group (ADR 17) and the ReplayGain tags (ADR 19), plus the
    /// library id continuation keeps to know what the session has already
    /// held. Three parallel vecs, two of which the queue commands carry.
    /// Unknown paths resolve to ungrouped, untagged, and unidentified; a
    /// missing database means every path does, and playback carries on
    /// unlevelled.
    fn queue_meta_for(&mut self, paths: &[PathBuf]) -> QueueMeta {
        if self.meta_conn.is_none() {
            let db = rox_core::settings::data_dir().join("library.db");
            self.meta_conn = db.exists().then(|| store::open(&db).ok()).flatten();
        }
        let Some(conn) = self.meta_conn.as_ref() else {
            return QueueMeta {
                groups: vec![None; paths.len()],
                gains: vec![Default::default(); paths.len()],
                ids: vec![None; paths.len()],
            };
        };
        let mut meta = QueueMeta {
            groups: Vec::with_capacity(paths.len()),
            gains: Vec::with_capacity(paths.len()),
            ids: Vec::with_capacity(paths.len()),
        };
        for path in paths {
            let row = path
                .to_str()
                .and_then(|s| store::queue_meta_for_path(conn, s).ok())
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
        }
        meta
    }

    /// The audio feed the audio views read from.
    pub fn feed(&self) -> Arc<AudioFeed> {
        self.feed.clone()
    }

    /// Where playback currently sits, resolved off the shared position
    /// clock. None while no session is running or before the first track
    /// opens.
    pub fn now_playing(&self) -> Option<NowPlaying> {
        let session = self.session.as_ref()?;
        let (track, secs) = session.shared.position(session.device_rate)?;
        let path = session.queue.get(track)?.clone();
        let duration_secs = {
            let tracks = session.shared.tracks.lock().unwrap();
            tracks
                .get(track)
                .and_then(|t| t.as_ref())
                .and_then(|t| t.duration_secs)
        };
        Some(NowPlaying {
            path,
            position_secs: secs,
            duration_secs,
            audible_idx: track,
        })
    }

    /// Absolute seek within the playing track, for the waveform strip.
    pub fn seek_to(&self, secs: f64) {
        self.send(Cmd::Seek(secs.max(0.0)));
    }

    /// Replace whatever is playing with a fresh queue starting at its first
    /// track; the old session quits on drop.
    pub fn play(&mut self, queue: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.start_session(queue, 0, None, Vec::new(), false, cx);
    }

    /// Replace the queue and start at `start`, so the tracks before it sit
    /// behind the cursor as history and Prev walks back into them. What a
    /// double click in a track list uses, seeding the whole list so Next and
    /// Prev carry through the surrounding album instead of dead-ending at the
    /// clicked track.
    pub fn play_at(&mut self, queue: Vec<PathBuf>, start: usize, cx: &mut Context<Self>) {
        self.start_session(queue, start, None, Vec::new(), false, cx);
    }

    /// Replace whatever is playing with a fresh queue whose entries are all
    /// explicit, playing from the first. Unlike [`play`] and [`play_at`],
    /// which seed a context (an album or library run that plays on unlisted),
    /// these entries are the up-next queue, so the queue panel lists them.
    /// Clicking an album in a browser lands here, so the album you played
    /// shows in the queue.
    pub fn play_explicit(&mut self, queue: Vec<PathBuf>, cx: &mut Context<Self>) {
        let explicit = vec![true; queue.len()];
        self.start_session(queue, 0, None, explicit, false, cx);
    }

    /// The launch restore for an old settings file that saved only a single
    /// track: load it paused at a position, ready on the seek strip but silent
    /// until asked to play. Files written since carry the whole queue and come
    /// back through [`restore_queue`] instead.
    pub fn restore(&mut self, path: PathBuf, position_secs: f64, cx: &mut Context<Self>) {
        self.start_session(
            vec![path],
            0,
            Some(position_secs.max(0.0)),
            Vec::new(),
            true,
            cx,
        );
    }

    /// The launch restore: bring back the whole play order paused at the
    /// cursor, so Prev and Next walk the saved context and the up-next queue
    /// panel comes back with the explicit entries it held. `explicit` runs
    /// parallel to `queue`; `cursor` is the entry that was playing.
    pub fn restore_queue(
        &mut self,
        queue: Vec<PathBuf>,
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

    /// The explicit up-next queue: what Play Next and Add to Queue put ahead
    /// of the playing track, apart from the context (the album or library) that
    /// plays on around it. Empty during plain context playback, which is what
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

    /// How many tracks sit in the explicit queue, for the widget badge.
    pub fn queued_count(&self) -> usize {
        self.queued().len()
    }

    /// The whole play order for the close-time persist: every entry's path
    /// and whether it was explicit, plus the audible cursor and where its
    /// clock sits. The cursor rides off the position clock, not the decode
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
            .map(|e| (e.path.clone(), e.explicit))
            .collect();
        Some((entries, cursor, position_secs))
    }

    /// Queue tracks to play next, at the front of the explicit queue right
    /// after the playing track. With nothing loaded this just starts them.
    pub fn play_next(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let after = self.playing_after();
        self.insert(after, paths, false, cx);
    }

    /// Play these now without discarding the queue: splice them right after the
    /// playing track and jump to the first, so the rest of the queue plays on
    /// behind them. With nothing loaded this just starts them. The drop's Play
    /// now zone routes here; an OS file open replaces the session instead.
    pub fn play_now(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let after = self.playing_after();
        self.insert(after, paths, true, cx);
    }

    /// Queue tracks at the end of the explicit queue, after anything already
    /// queued but before the context resumes. With nothing loaded this starts
    /// them.
    pub fn enqueue(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let after = self.enqueue_after();
        self.insert(after, paths, false, cx);
    }

    /// The queue entry index of the playing track, matched by pool index off
    /// the position clock, so a Play Next lands after what you hear rather than
    /// after a track the decoder has already opened for the gapless boundary.
    /// Matching on the pool index rather than the path keeps a file that sits
    /// in the order twice from resolving to the wrong occurrence, which would
    /// otherwise leave the real playing entry inside `queued()` and refuse to
    /// clear.
    fn audible_index(&self, snap: &QueueSnapshot) -> Option<usize> {
        let now = self.now_playing()?;
        snap.entries.iter().position(|e| e.idx == now.audible_idx)
    }

    /// The queue entry index of the newest track the engine has taken on,
    /// which is where a skip landed even while the position clock still reads
    /// the track it left.
    ///
    /// The newest segment is the one the engine pushed when it adopted the
    /// track, and under a crossfade it sits half a window in the future: the
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

    /// The entry Add to Queue appends after: the last explicit entry in the
    /// run following the playing track, so it lands at the tail of the queue
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

    /// Splice paths into the running session as explicit queue entries,
    /// mirroring the pool growth on our side so `now_playing` can still resolve
    /// a freshly queued track back to its file. With no session, fall back to
    /// starting playback (a context, not a queue).
    fn insert(
        &mut self,
        after: Option<u64>,
        paths: Vec<PathBuf>,
        and_play: bool,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        if self.session.is_none() {
            self.play(paths, cx);
            return;
        }
        self.splice(after, paths, None, true, and_play, cx);
    }

    /// The insert both the hand-queued paths and a landed continuation batch
    /// go through: resolve the library metadata, mirror the pool growth, and
    /// hand the batch to the engine. `groups` overrides what the library says
    /// about album membership where a caller has an opinion; None per entry,
    /// or None for the whole batch, takes the library's own grouping.
    #[allow(clippy::too_many_arguments)]
    fn splice(
        &mut self,
        after: Option<u64>,
        paths: Vec<PathBuf>,
        groups: Option<Vec<Option<u64>>>,
        explicit: bool,
        and_play: bool,
        cx: &mut Context<Self>,
    ) {
        // Nothing to mirror the growth onto, so bail before anything grows:
        // `pool_ids` runs parallel to the session's pool and a half-applied
        // splice would slide the two apart for the rest of the session.
        if self.session.is_none() {
            return;
        }
        // Library lookup before the session borrow; both want &mut self.
        let meta = self.queue_meta_for(&paths);
        let groups = match groups {
            Some(picked) => picked
                .into_iter()
                .zip(meta.groups)
                .map(|(picked, library)| picked.or(library))
                .collect(),
            None => meta.groups,
        };
        self.pool_ids.extend(meta.ids);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.queue.extend(paths.iter().cloned());
        session.gains.extend(meta.gains.iter().copied());
        let _ = session.tx.send(Cmd::Insert {
            after,
            paths,
            groups,
            gains: meta.gains,
            explicit,
            and_play,
        });
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

    /// Play a queued entry now without consuming the rest of the queue: the
    /// entry moves to the front of the explicit queue first, then the jump
    /// lands on it. A bare jump would strand everything above the entry
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
        queue: Vec<PathBuf>,
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
        // A paused start (the launch restore) never renders audio, so the
        // visualizer tap stays empty and the spectrum has nothing to show.
        // Remember what to prime the feed with so a frozen panel gets a real
        // frame at the load position instead of blank bars.
        let prime = paused_at.map(|secs| (queue[start].clone(), secs.max(0.0)));
        // Album groups and ReplayGain for the whole context. A restore
        // re-derives both here too, so neither needs persisting with the
        // queue.
        let meta = self.queue_meta_for(&queue);
        let (groups, gains) = (meta.groups, meta.gains);
        // A fresh context is a fresh session for continuation too: nothing
        // has been played, nothing has been asked for, and whoever started
        // playback names the scope after this returns. A rebuild (a device
        // or rate change) puts all three back, since the music never stopped.
        self.pool_ids = meta.ids;
        self.scope = continuation::Scope::default();
        self.continuing = false;
        self.continued_rev = None;
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
                paths: queue,
                start,
                explicit,
                groups,
                gains,
            },
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
                // Ask the next open for the rate this one landed on. A card
                // that took 44.1 keeps being asked for 44.1, so a rebuild
                // for any other reason doesn't drop to the device default
                // and then follow its way back up with a second gap. Only
                // exclusive gets to pick a rate at all: carrying a shared
                // session's mixer rate forward would make the switch into
                // exclusive open at the mixer rate first, then follow the
                // file's, which is the second gap this exists to avoid.
                self.follow_rate = (session.negotiated.mode == Mode::Exclusive)
                    .then_some(session.negotiated.sample_rate);
                self.session = Some(session);
                self.error = None;
                self.start_pump(cx);
                if let Some((path, secs)) = prime {
                    self.prime_feed(path, secs, rate, cx);
                }
                // A fresh context under the similarity mode owes its tail an
                // ordering: the engine seeded it with the plain shuffle flag,
                // which for this mode means pool order. A restore keeps the
                // order it saved and asks for nothing.
                if !preserve_order
                    && self.settings.session.shuffle
                    && self.shuffle_mode() == ShuffleMode::Similar
                {
                    self.order_tail_by_similarity(1, None, cx);
                }
            }
            Err(e) => self.error = Some(format!("audio output: {e}").into()),
        }
        cx.notify();
    }

    /// Drop the running session entirely: playback stops, the position
    /// clock goes away, and the views over it - the seek strip, the
    /// waveform, the cover - fall back to idle. The transport's eject.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.pump = None;
        self.error = None;
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
    /// wake here the queue views would sit one edit behind until the next
    /// poke. A settled pause with a settled queue notifies nobody: the
    /// seek clock is frozen, the visualizers park themselves, and the
    /// whole UI goes quiet.
    fn start_pump(&mut self, cx: &mut Context<Self>) {
        let mut was_playing = self.is_playing();
        let mut seen_rev = self.queue_rev();
        let mut seen_pos = self.position_key();
        self.pump = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(PUMP_INTERVAL).await;
            let alive = this.update(cx, |this, cx| {
                if this.session.is_none() {
                    return false;
                }
                // The output stream died (device unplugged, backend fault).
                // Rebuild it at the current spot and stop this pump: the
                // rebuild starts its own, and running two would double-drain
                // the tap. If the rebuild couldn't get a device it clears the
                // session, so either way this pump is done.
                if this
                    .session
                    .as_ref()
                    .is_some_and(|s| s.shared.device_lost())
                {
                    this.reopen_device(cx);
                    return false;
                }
                // Exclusive follows the file's rate, which means the same
                // stop: the rebuild brings its own pump up.
                if this.follow_source_rate(cx) {
                    return false;
                }
                this.drain_tap();
                // The continuation trigger rides this same clock (ADR 17).
                // It reads the queue snapshot the check below already wants
                // and does nothing at all on the overwhelming majority of
                // ticks, which is why it can live on a 60 Hz timer.
                this.continue_if_dry(cx);
                let playing = this.is_playing();
                let rev = this.queue_rev();
                // A seek while paused moves the clock without touching any
                // of the above: audio stays quiet and the queue keeps its
                // revision, so the seek strip and the MPRIS position would
                // show the old spot until the next resume. Compare the
                // resolved position while paused; playing ticks notify
                // anyway, so the check skips them and a settled pause still
                // costs nothing when nothing moved.
                let pos = if playing { None } else { this.position_key() };
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
        }));
    }

    /// The continuation trigger (ADR 17): when the audible cursor comes
    /// within [`continuation::FLOOR`] tracks of the end of the upcoming
    /// portion, ask the active provider for a batch and append it into the
    /// running session.
    ///
    /// Here rather than in the engine, even though the engine reaches the end
    /// first. Its `pos` is the decode cursor and runs up to a ring ahead of
    /// the speakers, and firing there would put the audio thread inside the
    /// library stores, which inverts the one dependency this whole design
    /// keeps clean. The engine stays a decoder walking a list and never
    /// learns continuation exists.
    fn continue_if_dry(&mut self, cx: &mut Context<Self>) {
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
        if self.continued_rev == Some(rev) {
            return;
        }
        // The audible cursor, not the decode cursor: that one has run a track
        // ahead for the gapless boundary, and a batch seeded off a track
        // nobody has heard yet is a batch for the wrong taste.
        let audible = session.shared.position(session.device_rate).map(|(t, _)| t);
        if !self.running_dry() {
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
        let similar = self.similar_order();
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
                    let provider = continuation::provider(mode, similar)?;
                    let conn = store::open(&db_path).ok()?;
                    Some(provider.next(&conn, &seed))
                })
                .await
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                this.continuing = false;
                this.land_continuation(mode, similar, picks, cx);
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
        similar: bool,
        picks: Vec<Pick>,
        cx: &mut Context<Self>,
    ) {
        // The mode changed while the query ran, so this answer is for a
        // question nobody is asking any more. A cleared revision says the
        // same thing about the session: a fresh context or a stream rebuild
        // resets it, and a batch picked for the queue that was playing then
        // has no business landing in this one. `similar` goes the same way,
        // since a batch the radio drew is the wrong twenty tracks for a queue
        // that has since gone back to browse order.
        if mode != self.settings.session.continuation
            || similar != self.similar_order()
            || self.continued_rev.is_none()
            || self.session.is_none()
        {
            return;
        }
        // The query took long enough to pause in, or to queue an album in, so
        // the trigger's own conditions are asked again here rather than
        // assumed to have held. Twenty context tracks landing behind a queue
        // the listener just filled is the same wrong answer as one landing on
        // a queue they just paused.
        if !continuation_wanted(self.is_playing(), self.stop_after) || !self.running_dry() {
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
        // Shuffle on means shuffle everywhere, so a landed batch joins the
        // upcoming permutation rather than sitting in provider order at the
        // tail. The two modes fold it in differently because they mean
        // different things.
        //
        // Random shuffles the batch itself and lands it as it is. The obvious
        // alternative, appending and then reshuffling the whole tail, would
        // scramble any explicit queue the listener hand-built every twenty
        // tracks, and a hand-built queue is explicit intent. It's also the
        // same answer: the trigger fires with at most a floor of tracks left,
        // so there is barely a tail to permute against.
        if self.settings.session.shuffle && self.shuffle_mode() == ShuffleMode::Random {
            shuffle_slice(&mut resolved);
        }
        let (paths, groups): (Vec<PathBuf>, Vec<Option<u64>>) = resolved.into_iter().unzip();
        // Context, not queue: what continuation adds is the album or library
        // run playing on around you, so the queue widgets stay quiet about it
        // the way they do for the context that seeded the session. Visible in
        // the timeline and removable all the same, which is the whole answer
        // to "rox is playing things I didn't pick".
        self.splice(None, paths, Some(groups), false, false, cx);
        // Similar ranks the whole upcoming portion against the playing track,
        // which is what the mode already does on every skip, so the fold is
        // just asking it again now the batch has arrived. Nothing to pin
        // here: an explicit queue under this mode was always going to be
        // reordered by it.
        if self.similar_order() {
            self.order_tail_by_similarity(1, None, cx);
        }
    }

    /// Whether the queue has run close enough to its end to want a batch
    /// (ADR 17).
    ///
    /// Counted from the audible cursor, not the decode cursor, which has run
    /// a track ahead for the gapless boundary and would fire a track early.
    /// The published cursor stands in before any frame has played, so a
    /// session that comes up already short (a one-track queue, the random
    /// button, Start Radio) fires on its first tick, which is the point.
    ///
    /// Asked twice for every batch, once to fire the query and again when the
    /// answer lands: a query is a hundred milliseconds, which is plenty of
    /// room to queue an album into, and a batch that lands behind one is
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
    /// to do it. What the radio draw rides on rather than a mode of its own.
    fn similar_order(&self) -> bool {
        self.settings.session.shuffle && self.shuffle_mode() == ShuffleMode::Similar
    }

    /// Resolve a batch to playable paths, each with the group its pick asked
    /// for. One lookup per id rather than one for the batch, because
    /// `paths_for` drops ids it can't resolve and the groups would slide out
    /// from under the paths they belong to.
    fn resolve_picks(&mut self, picks: &[Pick]) -> Vec<(PathBuf, Option<u64>)> {
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
                let path = store::paths_for(conn, &[pick.id]).ok()?.pop()?;
                Some((PathBuf::from(path), pick.group))
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

    /// Play one track at random as a fresh one-track queue, drawn from the
    /// context playback is already in: the view or playlist that started the
    /// session, the library at large when nothing named one. The scope is put
    /// back after the start, so a second press stays in the same list instead
    /// of escaping to the library the way a fresh session would.
    pub fn play_random(&mut self, library: &Entity<Library>, cx: &mut Context<Self>) {
        let scope = self.scope.clone();
        let paths = {
            let library = library.read(cx);
            let all: &[i64] = library
                .projection()
                .map(|p| p.db_id.as_slice())
                .unwrap_or_default();
            // A scope id the library no longer holds resolves to no file, so
            // the draw takes the library at large rather than dropping the
            // press on the floor. A scope that already is the library just
            // gets a second try.
            draw_one(library, random_pool(&scope, all)).or_else(|| draw_one(library, all))
        };
        let Some(paths) = paths else { return };
        self.play(paths, cx);
        // After the play, never before: starting a session clears the scope
        // back to the library at large.
        self.scope = scope;
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
        let (paths, explicit): (Vec<PathBuf>, Vec<bool>) = entries.into_iter().unzip();
        // A rebuild is the same music on a different stream, so the scope
        // carries over: the view play started in is still the view play
        // started in. The start clears it, which is right for a fresh context
        // and wrong for this. The played set needs no such care, since the
        // whole order comes back and it's re-derived from that.
        let scope = self.scope.clone();
        // Restore-shaped start: preserve the saved order, seed the position.
        self.start_session(paths, cursor, Some(position_secs), explicit, true, cx);
        self.scope = scope;
        // A restore comes up paused, so put it back to playing. Only when the
        // start actually produced a session.
        if was_playing && self.session.is_some() {
            self.send(Cmd::TogglePause);
        }
        true
    }

    /// Rebuild the output after the device dropped out. The old stream is
    /// already dead, so this is the only way back to audio short of the user
    /// restarting; start_session opens against the current default device,
    /// which is the reconnected (or newly default) one. Nothing left to
    /// restore surfaces as an error with the session gone, so the UI stops
    /// showing a frozen "playing".
    fn reopen_device(&mut self, cx: &mut Context<Self>) {
        if self.session.is_none() || self.rebuild_session(cx) {
            return;
        }
        self.stop(cx);
        self.error = Some("audio output: device lost".into());
        cx.notify();
    }

    /// Exclusive mode follows the file's rate (ADR 19): when the playing
    /// track's rate isn't the rate the device is running, reopen at the
    /// file's. Costs the gap between tracks the ADR budgeted for, and it's
    /// the whole reason a fresh queue can open at the device default and
    /// still end up bit-perfect. Nothing knows a file's rate until the
    /// decode thread has opened it, so the first one is followed a beat
    /// late rather than guessed at.
    ///
    /// Only fires on a rate the device hasn't already turned down, so a card
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
        // The card landed somewhere else, so this rate is one it doesn't
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
            // A pinned rate is the whole ask; the follow only speaks when
            // nothing was pinned, so the two can't fight over the stream.
            rate: output.rate.or(self.follow_rate),
            format: output.format.clone(),
            period_ms: output.period_ms,
        }
    }

    /// What output ended up doing, for the settings page. None while no
    /// stream is open, which is the honest answer: nothing has negotiated
    /// with any device yet.
    pub fn output_status(&self) -> Option<OutputStatus> {
        Some(OutputStatus {
            negotiated: self.negotiated()?.clone(),
            source_rate: self.source_rate(),
            leveling_db: self.leveling_db(),
        })
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
        // and the transport's menu both land in this one place, and the two
        // would otherwise disagree about what "back on" means.
        if secs > 0.0 {
            self.settings.crossfade_restore_secs = secs;
        }
        self.send_crossfade();
        // Dragging the slider lands here per tick, same as the volume, so
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
    /// toggle can't turn the fade "on" at no length; a settings file carrying
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

    /// How tagged loudness is levelled right now (ADR 19).
    pub fn replay_gain(&self) -> ReplayGainSettings {
        self.settings.replay_gain
    }

    /// Switch which gain the leveling reads, or turn it off. Live on the
    /// running session: the engine relevels every source it holds, so the
    /// change lands on the track playing rather than the one after it.
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
        // A dragged slider lands here per tick, so the file write waits for
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
    /// lands without a restart; with nothing playing it takes effect on the
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

    /// Take whatever the tap holds, never wait for more; the samples move
    /// on to the audio views' feed. Read as chunks straight off the ring's
    /// two slices - this runs 60 times a second for the whole session, so
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
    fn prime_feed(&self, path: PathBuf, secs: f64, rate: u32, cx: &mut Context<Self>) {
        let feed = self.feed.clone();
        let before = feed.written();
        cx.spawn(async move |this, cx| {
            let window = cx
                .background_executor()
                .spawn(async move {
                    engine::decode_window(&path, secs, rate, rox_viz::analysis::MAX_FFT_SIZE)
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
        // Re-seeded on wherever the skip lands, not on where the mode was
        // engaged: that's what turns skipping into steering rather than
        // drifting outward from a track the listener already left.
        let leaving = self.seed_entry();
        self.order_tail_by_similarity(skip_band(self.similar_skips), leaving, cx);
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

    /// Relative seek within the playing track.
    pub fn seek_by(&self, delta: f64) {
        if let Some(session) = &self.session {
            if let Some((_, secs)) = session.shared.position(session.device_rate) {
                let _ = session.tx.send(Cmd::Seek((secs + delta).max(0.0)));
            }
        }
    }

    /// Set the volume and persist it; dragging the slider lands here.
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
    /// Every slider tick and wheel notch lands in a setter, and
    /// `Settings::update` reads, parses, and rewrites the files - too much
    /// for a pointer-move rate. The engine and the in-memory copy already
    /// hold the value, so only the file write waits for the last tick. Same
    /// pattern as the settings window's persist_appearance_soon.
    ///
    /// The volume, the fade, and the leveling knobs share the debounce:
    /// only the file whose contents actually moved gets written, so
    /// covering all of them costs nothing and none can outrun another's
    /// pending write.
    fn persist_playback_soon(&mut self, cx: &mut Context<Self>) {
        self.persist_gen += 1;
        let gen = self.persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            // A later tick bumped the gen past this capture, so only the last
            // edit in a burst writes. Read the values at write time, not
            // capture time, so a mute toggled during the wait persists as is.
            let Ok((latest, volume, muted, crossfade, restore, replay_gain)) =
                this.update(cx, |this, _| {
                    (
                        this.persist_gen,
                        this.settings.session.volume,
                        this.settings.session.muted,
                        this.settings.crossfade_secs,
                        this.settings.crossfade_restore_secs,
                        this.settings.replay_gain,
                    )
                })
            else {
                return;
            };
            if latest == gen {
                Settings::update(move |s| {
                    s.session.volume = volume;
                    s.session.muted = muted;
                    s.crossfade_secs = crossfade;
                    s.crossfade_restore_secs = restore;
                    s.replay_gain = replay_gain;
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
    /// library's "Play Shuffled" wants exactly that whatever the transport's
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
            ShuffleMode::Similar => self.order_tail_by_similarity(1, None, cx),
        }
    }

    /// Order what's coming by how much it sounds like the playing track.
    ///
    /// The scan over the library's vectors is tens of milliseconds, so it
    /// runs on the background executor against its own connection rather than
    /// in this update. The engine keeps playing the whole time; the reorder
    /// lands as a queue publish whenever the answer arrives, which is the
    /// same way any other queue edit shows up.
    ///
    /// Anything the library can't score keeps its place behind what it can
    /// (see [`Cmd::OrderTail`]), so an unanalyzed library leaves the queue
    /// exactly as it was rather than scrambling it.
    ///
    /// Nothing resets the tail to pool order first, deliberately. An earlier
    /// cut sent `SetShuffle(false)` up front to normalize it, which meant a
    /// scan that came back with nothing left the queue sorted into library
    /// order: press Next and you got track one, which looked far more broken
    /// than doing nothing would have. Touching the queue only once, when
    /// there's an answer, makes the failure case invisible.
    fn order_tail_by_similarity(
        &mut self,
        band: usize,
        leaving: Option<u64>,
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
            let Some((seed_path, tail)) = inputs else {
                return;
            };
            // Read on this thread, before the spawn: the pick is a process
            // static, and this only needs the name it stores vectors under.
            let model = crate::acoustic::acoustic_source().id().to_string();
            let ranked = cx
                .background_executor()
                .spawn(async move {
                    let conn = store::open(&db_path).ok()?;
                    let seed = store::id_for_path(&conn, seed_path.to_str()?).ok()??;
                    let scores: HashMap<i64, f32> = embeddings::scores(&conn, seed, &model)
                        .ok()?
                        .into_iter()
                        .collect();
                    // Entries the library has no score for drop out of the
                    // ranking rather than sorting as zero, which would rate
                    // them above everything that genuinely sounds unalike.
                    let mut ranked: Vec<(u64, f32)> = tail
                        .into_iter()
                        .filter_map(|(entry, path)| {
                            let id = store::id_for_path(&conn, path.to_str()?).ok()??;
                            Some((entry, *scores.get(&id)?))
                        })
                        .collect();
                    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
                    let mut ids: Vec<u64> = ranked.into_iter().map(|(entry, _)| entry).collect();
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
                return;
            };
            this.update(cx, |this, cx| {
                // Still shuffling in the same mode? A toggle or a mode change
                // while the scan ran means this answer is for a queue nobody
                // asked about any more.
                if this.settings.session.shuffle && this.shuffle_mode() == ShuffleMode::Similar {
                    this.send(Cmd::OrderTail(ids));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The seed track and the upcoming entries a similarity ordering works
    /// over, or None while the engine has yet to publish a queue or there is
    /// nothing ahead to order.
    fn similarity_inputs(&self, leaving: Option<u64>) -> Option<(PathBuf, Vec<(u64, PathBuf)>)> {
        let session = self.session.as_ref()?;
        let snap = session.shared.queue_snapshot();
        // A skip seeds on where it lands, so it reads the track the engine
        // has taken on rather than the one still coming out of the speakers.
        // Under a crossfade those are different for half a window: the clock
        // flips at the midpoint (ADR 19), so on the default four seconds the
        // audible track is the one being left for two whole seconds after the
        // press, and steering that waited for the flip would give up first.
        //
        // The published cursor when nothing is audible yet, which is where a
        // freshly started context sits: waiting for the first samples would
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
        let seed = entry.path.clone();
        let tail: Vec<(u64, PathBuf)> = snap
            .entries
            .get(at + 1..)?
            .iter()
            .map(|e| (e.id, e.path.clone()))
            .collect();
        (!tail.is_empty()).then_some((seed, tail))
    }

    /// Whether stop-after-current is armed.
    pub fn stop_after(&self) -> bool {
        self.stop_after
    }

    /// Arm or clear stop-after-current: armed, the playing track ends the
    /// motion - the engine plays it out, pauses, and cues the next track.
    /// Sticky until cleared, and session-local by design.
    pub fn toggle_stop_after(&mut self, cx: &mut Context<Self>) {
        self.stop_after = !self.stop_after;
        self.send(Cmd::SetStopAfter(self.stop_after));
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

    /// The last session-start failure, shown while nothing plays.
    pub fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }

    /// A snapshot of the discrete state, without the position clock. What
    /// [`observe_view`] diffs to decide whether a tick is worth a repaint.
    pub fn view(&self) -> PlayerView {
        let now = self.now_playing();
        PlayerView {
            track: now.as_ref().map(|now| now.path.clone()),
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
            muted: self.muted(),
            volume: self.volume(),
            error: self.error(),
            fade: self.crossfade(),
        }
    }
}

/// The equalizer's live parameters (ADR 19), one set for the whole process.
/// The curve is an app preference rather than something a session owns, so
/// every chain that opens rides this same handle and a band moves under
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
/// like they are, which is what lets a band dragged in the EQ window repaint a
/// widget sitting in some other workspace's transport row.
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
/// off, so this is a store rather than a chain edit; it lands as soon as
/// the ring drains past it, up to half a second behind the click.
pub fn set_eq_enabled(on: bool, cx: &mut App) {
    eq_params().set_enabled(on);
    Settings::update(move |s| s.eq.enabled = on);
    eq_changed(cx);
}

/// Move one band, in dB. The store is what the decode thread reads on its
/// next buffer; the file write waits for the drag to settle.
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
/// atomic carries it to the decode thread now, the file write waits for the
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

/// Persist the curve once the drag settles, the same shape
/// [`Player::persist_volume_soon`] uses: the atomics already carry the
/// value to the audio thread, so only the file write has to wait for the
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
/// want each tick (the clock, the playhead, the bars) and observe the
/// player directly; everything else rides this so a playing session does
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
    /// end of the order the cursor sits at.
    #[test]
    fn the_trigger_fires_inside_the_floor_and_not_above_it() {
        // Nineteen still to come, nothing to do.
        assert!(!queue_running_dry(19, LoopMode::Off));
        // Three to go is one over the floor, two is the floor itself.
        assert!(!queue_running_dry(3, LoopMode::Off));
        assert!(queue_running_dry(2, LoopMode::Off));
        assert!(queue_running_dry(1, LoopMode::Off));
        // Standing on the last entry, which is also where a queue that
        // played out to its end sits.
        assert!(queue_running_dry(0, LoopMode::Off));
    }

    /// The other half of the trigger's gate: a paused queue refuses to grow,
    /// an ended one still wants a batch because it reads as playing, and an
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

    /// A band of one, which is what a settled radio uses, must not disturb
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
}
