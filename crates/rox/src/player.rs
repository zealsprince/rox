//! The player bar, a fixed row at the bottom of the workspace: one running
//! engine session behind the playback contract (commands in over a channel,
//! state out through shared atomics). The PCM tap gets drained here every
//! frame; the peak drives the level meter and the samples move on to the
//! audio views' shared feed. That per-frame drain is why the bar lives
//! outside the dock as exactly one view: it must keep rendering no matter
//! what the panels do.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};

use gpui::{div, prelude::*, px, relative, rgb, Context, MouseButton, SharedString, Window};

use rox_playback::cpal::Stream;
use rox_playback::engine::{self, Cmd};
use rox_playback::output;
use rox_playback::rtrb::Consumer;
use rox_playback::shared::Shared;
use rox_viz::AudioFeed;

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
    fn start(queue: Vec<PathBuf>) -> Result<Session, String> {
        let shared = Arc::new(Shared::new(queue.len()));
        let out = output::open(shared.clone())?;
        let device_rate = out.sample_rate;
        let (tx, rx) = mpsc::channel::<Cmd>();
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
}

impl Player {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Player {
            session: None,
            error: None,
            feed: Arc::new(AudioFeed::new()),
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
        match Session::start(queue) {
            Ok(session) => {
                self.feed.set_sample_rate(session.device_rate);
                self.session = Some(session);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("audio output: {e}").into()),
        }
        cx.notify();
    }

    fn send(&self, cmd: Cmd) {
        if let Some(session) = &self.session {
            let _ = session.tx.send(cmd);
        }
    }

    fn seek_by(&self, delta: f64) {
        if let Some(session) = &self.session {
            if let Some((_, secs)) = session.shared.position(session.device_rate) {
                let _ = session.tx.send(Cmd::Seek((secs + delta).max(0.0)));
            }
        }
    }

    fn nudge_volume(&self, delta: f32) {
        if let Some(session) = &self.session {
            let _ = session
                .tx
                .send(Cmd::Volume(session.shared.volume() + delta));
        }
    }

    fn control(
        &self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Player, &mut Context<Player>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x2a2a2a))
            .hover(|d| d.bg(rgb(0x3a3a3a)))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| on_click(this, cx)),
            )
            .child(label.into())
    }

    fn session_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Repaint continuously while a session runs: the clock and the tap
        // meter update every frame.
        window.request_animation_frame();

        let session = self.session.as_mut().expect("session bar without session");

        // Drain the tap: take what's there, never wait for more. The peak
        // drives the meter, the samples go on to the audio views' feed.
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

        let playing = session.shared.playing.load(Ordering::Relaxed);
        let ended = session.shared.ended.load(Ordering::Relaxed);
        let volume = (session.shared.volume() * 100.0).round() as u32;
        let meter = session.meter.min(1.0);

        let status: SharedString = match session.shared.position(session.device_rate) {
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
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .size_full()
            .child(self.control("prev", |this, _| this.send(Cmd::Prev), cx))
            .child(self.control(
                if playing { "pause" } else { "play" },
                |this, _| this.send(Cmd::TogglePause),
                cx,
            ))
            .child(self.control("next", |this, _| this.send(Cmd::Next), cx))
            .child(self.control("-10s", |this, _| this.seek_by(-10.0), cx))
            .child(self.control("+10s", |this, _| this.seek_by(10.0), cx))
            .child(div().flex_1().min_w_0().truncate().child(status))
            .child(
                div()
                    .w(px(60.))
                    .h(px(6.))
                    .flex_none()
                    .rounded_sm()
                    .bg(rgb(0x2a2a2a))
                    .child(
                        div()
                            .h_full()
                            .rounded_sm()
                            .bg(rgb(0x3dff9c))
                            .w(relative(meter)),
                    ),
            )
            .child(self.control("vol -", |this, _| this.nudge_volume(-0.1), cx))
            .child(div().w(px(40.)).flex_none().text_center().child(format!("{volume}%")))
            .child(self.control("vol +", |this, _| this.nudge_volume(0.1), cx))
    }
}

impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.session.is_some() {
            self.session_bar(window, cx).into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .text_color(rgb(0x808080))
                .child(
                    self.error
                        .clone()
                        .unwrap_or_else(|| "nothing playing".into()),
                )
                .into_any_element()
        };

        // Fills whatever height the workspace's bar slot gives it.
        div()
            .size_full()
            .px_3()
            .flex()
            .items_center()
            .bg(rgb(0x1f1f1f))
            .border_t_1()
            .border_color(rgb(0x333333))
            .child(body)
    }
}

fn fmt_time(secs: f64) -> String {
    let m = (secs / 60.0).floor() as u64;
    format!("{m}:{:02}", (secs - (m * 60) as f64).floor() as u64)
}
