//! The playback service entity: one running engine session behind the
//! playback contract (commands in over a channel, state out through shared
//! atomics). The PCM tap is drained by a headless pump task on a timer, not
//! by any render pass, so the audio views' feed keeps flowing no matter
//! which windows are drawing - popped-out panels, a zoomed dock, a
//! minimized main window. The player renders nothing itself; the transport
//! panels are the UI over this state.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use gpui::{Context, Entity, SharedString, Subscription, Task};

use rox_playback::cpal::Stream;
use rox_playback::engine::{self, Cmd, LoopMode};
use rox_playback::output;
use rox_playback::rtrb::Consumer;
use rox_playback::shared::{QueueEntry, QueueSnapshot, Shared};
use rox_viz::AudioFeed;

use crate::settings::Settings;

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
    _stream: Stream,
    device_rate: u32,
    /// The queued paths, kept so the views can resolve the playing track
    /// back to its file.
    queue: Vec<PathBuf>,
}

impl Session {
    fn start(
        queue: Vec<PathBuf>,
        start: usize,
        volume: f32,
        loop_mode: LoopMode,
        shuffle: Option<bool>,
        paused_at: Option<f64>,
        explicit: Vec<bool>,
    ) -> Result<Session, String> {
        let shared = Arc::new(Shared::new(queue.len()));
        // Seed the session with the persisted playback state: volume lands
        // in the shared atomics before the stream opens, the loop and
        // shuffle modes queue on the channel so the engine picks them up
        // first thing.
        shared
            .volume_bits
            .store(volume.to_bits(), Ordering::Relaxed);
        let out = output::open(shared.clone())?;
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
        // The launch restore's seek and pause queue here too, ahead of the
        // decode thread: the engine drains commands before it decodes, so
        // the session comes up already paused at the position and nothing
        // sounds.
        if let Some(secs) = paused_at {
            let _ = tx.send(Cmd::Seek(secs));
            let _ = tx.send(Cmd::TogglePause);
        }
        let engine = engine::Engine::new(
            queue.clone(),
            start,
            shared.clone(),
            out.producer,
            device_rate,
            rx,
            explicit,
        );
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
            queue,
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
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
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
    pub muted: bool,
    pub volume: f32,
    pub error: Option<SharedString>,
}

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
}

impl Player {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Player {
            session: None,
            error: None,
            feed: Arc::new(AudioFeed::new()),
            settings: Settings::load(),
            pump: None,
        }
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

    /// The launch restore for an old settings file that saved only a single
    /// track: load it paused at a position, ready on the seek strip but silent
    /// until asked to play. Files written since carry the whole queue and come
    /// back through [`restore_queue`] instead.
    pub fn restore(&mut self, path: PathBuf, position_secs: f64, cx: &mut Context<Self>) {
        self.start_session(vec![path], 0, Some(position_secs.max(0.0)), Vec::new(), true, cx);
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
        self.start_session(queue, cursor, Some(position_secs.max(0.0)), explicit, true, cx);
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
    pub fn queue_state(&self) -> Option<(Vec<(PathBuf, bool)>, usize, f64)> {
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
        self.insert(after, paths, cx);
    }

    /// Queue tracks at the end of the explicit queue, after anything already
    /// queued but before the context resumes. With nothing loaded this starts
    /// them.
    pub fn enqueue(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let after = self.enqueue_after();
        self.insert(after, paths, cx);
    }

    /// The queue entry index of the playing track, matched by path off the
    /// position clock, so a Play Next lands after what you hear rather than
    /// after a track the decoder has already opened for the gapless boundary.
    fn audible_index(&self, snap: &QueueSnapshot) -> Option<usize> {
        let now = self.now_playing()?;
        snap.entries.iter().position(|e| e.path == now.path)
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
    fn insert(&mut self, after: Option<u64>, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            self.play(paths, cx);
            return;
        };
        session.queue.extend(paths.iter().cloned());
        let _ = session.tx.send(Cmd::Insert {
            after,
            paths,
            explicit: true,
        });
        cx.notify();
    }

    /// Drop a queued entry by its stable id. The playing entry is refused by
    /// the engine, so the UI never has to guard it.
    pub fn remove_from_queue(&self, id: u64) {
        self.send(Cmd::Remove { id });
    }

    /// Drop every up-next explicit entry. The playing track and the context
    /// around it stay; only the hand-picked queue empties.
    pub fn clear_queue(&self) {
        for entry in self.queued() {
            self.remove_from_queue(entry.id);
        }
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
        self.session = None;
        // A fresh context takes the current shuffle mode; a restore preserves
        // the saved order and passes None so the engine leaves it untouched.
        let shuffle = if preserve_order {
            None
        } else {
            Some(self.settings.shuffle)
        };
        match Session::start(
            queue,
            start,
            self.effective_volume(),
            self.settings.loop_mode(),
            shuffle,
            paused_at,
            explicit,
        ) {
            Ok(session) => {
                self.feed.set_sample_rate(session.device_rate);
                let rate = session.device_rate;
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
    /// bars only fires while audio moves, on the play-state edge, or when
    /// the engine finishes a queue edit. That last one matters while
    /// paused: queue commands are fire-and-forget to the engine thread, so
    /// the revision bumps after the notify an enqueue sends, and without a
    /// wake here the queue views would sit one edit behind until the next
    /// poke. A settled pause with a settled queue notifies nobody: the
    /// seek clock is frozen, the visualizers park themselves, and the
    /// whole UI goes quiet.
    fn start_pump(&mut self, cx: &mut Context<Self>) {
        let mut was_playing = self.is_playing();
        let mut seen_rev = self.queue_rev();
        self.pump = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(PUMP_INTERVAL).await;
            let alive = this.update(cx, |this, cx| {
                if this.session.is_none() {
                    return false;
                }
                this.drain_tap();
                let playing = this.is_playing();
                let rev = this.queue_rev();
                if playing || playing != was_playing || rev != seen_rev {
                    cx.notify();
                }
                was_playing = playing;
                seen_rev = rev;
                true
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        }));
    }

    /// Take whatever the tap holds, never wait for more; the samples move
    /// on to the audio views' feed.
    fn drain_tap(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut drained: Vec<f32> = Vec::new();
        while let Ok(s) = session.tap.pop() {
            drained.push(s);
        }
        self.feed.push(&drained);
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
        self.settings.volume
    }

    /// Whether output is muted.
    pub fn muted(&self) -> bool {
        self.settings.muted
    }

    /// What the engine should actually apply: the volume, or silence.
    fn effective_volume(&self) -> f32 {
        if self.settings.muted {
            0.0
        } else {
            self.settings.volume
        }
    }

    /// The persisted loop mode.
    pub fn loop_mode(&self) -> LoopMode {
        self.settings.loop_mode()
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
        self.settings.volume = volume;
        self.settings.muted = false;
        self.send(Cmd::Volume(volume));
        Settings::update(move |s| {
            s.volume = volume;
            s.muted = false;
        });
        cx.notify();
    }

    /// Silence the output without losing the level; unmute restores it.
    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = !self.settings.muted;
        self.settings.muted = muted;
        self.send(Cmd::Volume(self.effective_volume()));
        Settings::update(move |s| s.muted = muted);
        cx.notify();
    }

    /// Whether shuffle is on, the persisted mode.
    pub fn shuffle(&self) -> bool {
        self.settings.shuffle
    }

    /// Flip shuffle and persist the pick. The running session reshuffles in
    /// place; the playing track keeps playing.
    pub fn toggle_shuffle(&mut self) {
        self.set_shuffle(!self.settings.shuffle);
    }

    /// Force shuffle to `on` and persist it, without toggling relative to the
    /// current mode. The library's shuffle actions set this before they queue,
    /// so the transport toggle reflects the mode they chose. A no-op when the
    /// mode already matches.
    pub fn set_shuffle(&mut self, on: bool) {
        if self.settings.shuffle == on {
            return;
        }
        self.settings.shuffle = on;
        self.send(Cmd::SetShuffle(on));
        Settings::update(move |s| s.shuffle = on);
    }

    /// Step off -> all -> one -> off and persist the pick.
    pub fn cycle_loop(&mut self) {
        let mode = match self.settings.loop_mode() {
            LoopMode::Off => LoopMode::All,
            LoopMode::All => LoopMode::One,
            LoopMode::One => LoopMode::Off,
        };
        self.settings.set_loop_mode(mode);
        self.send(Cmd::SetLoop(mode));
        Settings::update(|s| s.set_loop_mode(mode));
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
            muted: self.muted(),
            volume: self.volume(),
            error: self.error(),
        }
    }
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
