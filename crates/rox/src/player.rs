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

use gpui::{Context, SharedString, Task};

use rox_playback::cpal::Stream;
use rox_playback::engine::{self, Cmd, LoopMode};
use rox_playback::output;
use rox_playback::rtrb::Consumer;
use rox_playback::shared::Shared;
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
    meter: f32,
}

impl Session {
    fn start(queue: Vec<PathBuf>, volume: f32, loop_mode: LoopMode) -> Result<Session, String> {
        let shared = Arc::new(Shared::new(queue.len()));
        // Seed the session with the persisted playback state: volume lands
        // in the shared atomics before the stream opens, the loop mode
        // queues on the channel so the engine picks it up first thing.
        shared.volume_bits.store(volume.to_bits(), Ordering::Relaxed);
        let out = output::open(shared.clone())?;
        let device_rate = out.sample_rate;
        let (tx, rx) = mpsc::channel::<Cmd>();
        let _ = tx.send(Cmd::SetLoop(loop_mode));
        let engine =
            engine::Engine::new(queue.clone(), shared.clone(), out.producer, device_rate, rx);
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
            meter: 0.0,
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

    /// Replace whatever is playing with a fresh queue; the old session quits
    /// on drop.
    pub fn play(&mut self, queue: Vec<PathBuf>, cx: &mut Context<Self>) {
        if queue.is_empty() {
            return;
        }
        self.session = None;
        match Session::start(queue, self.settings.volume, self.settings.loop_mode()) {
            Ok(session) => {
                self.feed.set_sample_rate(session.device_rate);
                self.session = Some(session);
                self.error = None;
                self.start_pump(cx);
            }
            Err(e) => self.error = Some(format!("audio output: {e}").into()),
        }
        cx.notify();
    }

    /// Run the tap drain on a timer instead of a render pass. The tick also
    /// notifies the player, which is what repaints the bar's clock, meter,
    /// and play state while a session runs.
    fn start_pump(&mut self, cx: &mut Context<Self>) {
        self.pump = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PUMP_INTERVAL).await;
                let alive = this.update(cx, |this, cx| {
                    if this.session.is_none() {
                        return false;
                    }
                    this.drain_tap();
                    cx.notify();
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Take whatever the tap holds, never wait for more. The peak drives
    /// the level meter, the samples move on to the audio views' feed.
    fn drain_tap(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut peak = 0.0f32;
        let mut drained: Vec<f32> = Vec::new();
        while let Ok(s) = session.tap.pop() {
            peak = peak.max(s.abs());
            drained.push(s);
        }
        session.meter = if drained.is_empty() {
            session.meter * 0.85
        } else {
            peak.max(session.meter * 0.85)
        };
        self.feed.push(&drained);
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

    /// The smoothed output level the meter shows, zero while idle.
    pub fn meter(&self) -> f32 {
        self.session.as_ref().map(|s| s.meter).unwrap_or(0.0)
    }

    /// The persisted volume, the engine's clamp range (0 to 2).
    pub fn volume(&self) -> f32 {
        self.settings.volume
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

    /// Step the volume and persist it; the panels and the bar share this.
    pub fn nudge_volume(&mut self, delta: f32) {
        // Same clamp range the engine applies, so the persisted value and
        // the audible one never drift apart.
        self.settings.volume = (self.settings.volume + delta).clamp(0.0, 2.0);
        self.send(Cmd::Volume(self.settings.volume));
        self.settings.save();
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
        self.settings.save();
    }

    /// The transport panel's status line: queue position, track name, and
    /// clock, or "opening..." before the first track resolves. None while
    /// no session is running.
    pub fn status_line(&self) -> Option<SharedString> {
        let session = self.session.as_ref()?;
        let ended = session.shared.ended.load(Ordering::Relaxed);
        Some(match session.shared.position(session.device_rate) {
            Some((track, secs)) => {
                let tracks = session.shared.tracks.lock().unwrap();
                let info = tracks.get(track).and_then(|t| t.as_ref());
                let name = info.map(|i| i.name.as_str()).unwrap_or("?");
                let dur = info
                    .and_then(|i| i.duration_secs)
                    .map(fmt_time)
                    .unwrap_or_else(|| "?".into());
                format!(
                    "[{}/{}] {}  {} / {}{}",
                    track + 1,
                    session.queue.len(),
                    name,
                    fmt_time(secs),
                    dur,
                    if ended { " (queue finished)" } else { "" },
                )
                .into()
            }
            None => "opening...".into(),
        })
    }

    /// The last session-start failure, shown while nothing plays.
    pub fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }
}

fn fmt_time(secs: f64) -> String {
    let m = (secs / 60.0).floor() as u64;
    format!("{m}:{:02}", (secs - (m * 60) as f64).floor() as u64)
}
