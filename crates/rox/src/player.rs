//! The playback service entity: one running engine session behind the
//! playback contract (commands in over a channel, state out through shared
//! atomics). The PCM tap is drained by a headless pump task on a timer, not
//! by any render pass, so the audio views' feed keeps flowing no matter
//! which windows are drawing - popped-out panels, a zoomed dock, a
//! minimized main window. The player renders nothing itself; the transport
//! panels are the UI over this state.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;

use gpui::{App, Context, Entity, Global, SharedString, Subscription, Task};

use rox_library::store;
use rox_playback::engine::{self, Cmd, LoopMode, StartQueue};
use rox_playback::eq::{Eq, EqParams};
use rox_playback::gain;
use rox_playback::output::{self, Mode, Negotiated, Request};
use rox_playback::rtrb::Consumer;
use rox_playback::shared::{QueueEntry, QueueSnapshot, Shared};
use rox_viz::AudioFeed;

use crate::settings::{GainModeSetting, ReplayGainSave, ReplayGainSettings, Settings};

/// Pump cadence, roughly one video frame. The tap ring holds 16,384 samples
/// (about 170 ms at 48 kHz stereo), so a tick has an order of magnitude of
/// headroom before the callback's pushes start getting dropped.
const PUMP_INTERVAL: Duration = Duration::from_millis(16);

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
}

impl Player {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Player {
            session: None,
            error: None,
            feed: Arc::new(AudioFeed::new()),
            settings: Settings::load(),
            pump: None,
            persist_gen: 0,
            meta_conn: None,
            stop_after: false,
            follow_rate: None,
            refused_rates: Vec::new(),
        }
    }

    /// What the engine needs per queued path beyond the path itself: the
    /// album group (ADR 17) and the ReplayGain tags (ADR 19), split into
    /// the two parallel vecs the queue commands carry. Unknown paths
    /// resolve to ungrouped and untagged; a missing database means every
    /// path does, and playback carries on unlevelled.
    fn queue_meta_for(&mut self, paths: &[PathBuf]) -> (Vec<Option<u64>>, Vec<gain::ReplayGain>) {
        if self.meta_conn.is_none() {
            let db = crate::settings::data_dir().join("library.db");
            self.meta_conn = db.exists().then(|| store::open(&db).ok()).flatten();
        }
        let Some(conn) = self.meta_conn.as_ref() else {
            return (
                vec![None; paths.len()],
                vec![Default::default(); paths.len()],
            );
        };
        paths
            .iter()
            .map(|p| {
                let meta = p
                    .to_str()
                    .and_then(|s| store::queue_meta_for_path(conn, s).ok())
                    .unwrap_or_default();
                let rg = meta.replay_gain;
                (
                    meta.group,
                    gain::ReplayGain {
                        track_db: rg.track_db,
                        track_peak: rg.track_peak,
                        album_db: rg.album_db,
                        album_peak: rg.album_peak,
                    },
                )
            })
            .unzip()
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
        // Library lookup before the session borrow; both want &mut self.
        let (groups, gains) = self.queue_meta_for(&paths);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.queue.extend(paths.iter().cloned());
        session.gains.extend(gains.iter().copied());
        let _ = session.tx.send(Cmd::Insert {
            after,
            paths,
            groups,
            gains,
            explicit: true,
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
        let (groups, gains) = self.queue_meta_for(&queue);
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
        // Restore-shaped start: preserve the saved order, seed the position.
        self.start_session(paths, cursor, Some(position_secs), explicit, true, cx);
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
        self.send_crossfade();
        // Dragging the slider lands here per tick, same as the volume, so
        // the file write waits for the drag to settle.
        self.persist_playback_soon(cx);
        cx.notify();
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
    pub fn next(&self) {
        self.send(Cmd::Next);
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
            let Ok((latest, volume, muted, crossfade, replay_gain)) = this.update(cx, |this, _| {
                (
                    this.persist_gen,
                    this.settings.session.volume,
                    this.settings.session.muted,
                    this.settings.crossfade_secs,
                    this.settings.replay_gain,
                )
            }) else {
                return;
            };
            if latest == gen {
                Settings::update(move |s| {
                    s.session.volume = volume;
                    s.session.muted = muted;
                    s.crossfade_secs = crossfade;
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

    /// Flip shuffle and persist the pick. The running session reshuffles in
    /// place; the playing track keeps playing.
    pub fn toggle_shuffle(&mut self) {
        self.set_shuffle(!self.settings.session.shuffle);
    }

    /// Force shuffle to `on` and persist it, without toggling relative to the
    /// current mode. The library's shuffle actions set this before they queue,
    /// so the transport toggle reflects the mode they chose. A no-op when the
    /// mode already matches.
    pub fn set_shuffle(&mut self, on: bool) {
        if self.settings.session.shuffle == on {
            return;
        }
        self.settings.session.shuffle = on;
        self.send(Cmd::SetShuffle(on));
        Settings::update(move |s| s.session.shuffle = on);
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

/// The playback clock format the panels share: minutes and seconds.
pub fn fmt_time(secs: f64) -> String {
    fmt_time_padded(secs, 1)
}

/// `fmt_time` with the minutes zero-padded to `digits`, for clocks that
/// tick every frame and need to hold one width for a whole track.
pub fn fmt_time_padded(secs: f64, digits: usize) -> String {
    let m = (secs / 60.0).floor() as u64;
    format!(
        "{m:0digits$}:{:02}",
        (secs - (m * 60) as f64).floor() as u64
    )
}
