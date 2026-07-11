//! A GPUI front end over the playback prototype engine: drop audio files or
//! browse for them, then drive the same decode thread the CLI spike uses
//! with buttons instead of stdin.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};

use gpui::{
    div, prelude::*, px, relative, rgb, size, App, Bounds, Context, ExternalPaths, MouseButton,
    PathPromptOptions, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use rox_prototype_playback::cpal::Stream;
use rox_prototype_playback::engine::{self, Cmd};
use rox_prototype_playback::output;
use rox_prototype_playback::rtrb::Consumer;
use rox_prototype_playback::shared::Shared;

pub fn open_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(640.), px(320.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("rox prototype: playback")),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(options, |_, cx| cx.new(|_| PlaybackProto::new()))
        .expect("failed to open the playback window");
}

/// One running engine: decode thread, output stream, and the UI's side of
/// the PCM tap. Dropping it sends Quit and tears the stream down.
struct Session {
    shared: Arc<Shared>,
    tx: mpsc::Sender<Cmd>,
    tap: Consumer<f32>,
    _stream: Stream,
    device_rate: u32,
    queue_len: usize,
    meter: f32,
}

impl Session {
    fn start(queue: Vec<PathBuf>) -> Result<Session, String> {
        let shared = Arc::new(Shared::new(queue.len()));
        let out = output::open(shared.clone())?;
        let device_rate = out.sample_rate;
        let queue_len = queue.len();
        let (tx, rx) = mpsc::channel::<Cmd>();
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
            queue_len,
            meter: 0.0,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
    }
}

pub struct PlaybackProto {
    session: Option<Session>,
    error: Option<SharedString>,
}

impl PlaybackProto {
    pub fn new() -> Self {
        PlaybackProto {
            session: None,
            error: None,
        }
    }

    fn load(&mut self, queue: Vec<PathBuf>, cx: &mut Context<Self>) {
        if queue.is_empty() {
            return;
        }
        // Replaces any running session; the old one quits on drop.
        self.session = None;
        match Session::start(queue) {
            Ok(session) => {
                self.session = Some(session);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("audio output: {e}").into()),
        }
        cx.notify();
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |this, cx| this.load(paths, cx)).ok();
            }
        })
        .detach();
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
        on_click: impl Fn(&mut PlaybackProto, &mut Context<PlaybackProto>) + 'static,
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

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.browse(cx)),
            )
            .child(div().text_lg().child("drop audio files here"))
            .child(
                div()
                    .text_color(rgb(0x808080))
                    .child("or click to browse (flac, mp3, wav)"),
            )
    }

    fn session_state(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Repaint continuously while a session runs: the clock and the tap
        // meter update every frame, like the visualizer consumer will.
        window.request_animation_frame();

        let session = self.session.as_mut().expect("session state without session");

        // Drain the tap like a visualizer would: take what's there, never
        // wait for more.
        let mut peak = 0.0f32;
        let mut drained = false;
        while let Ok(s) = session.tap.pop() {
            peak = peak.max(s.abs());
            drained = true;
        }
        session.meter = if drained {
            peak.max(session.meter * 0.85)
        } else {
            session.meter * 0.85
        };

        let playing = session.shared.playing.load(Ordering::Relaxed);
        let ended = session.shared.ended.load(Ordering::Relaxed);
        let volume = (session.shared.volume() * 100.0).round() as u32;
        let meter = session.meter.min(1.0);

        let status = match session.shared.position(session.device_rate) {
            Some((track, secs)) => {
                let tracks = session.shared.tracks.lock().unwrap();
                let info = tracks.get(track).and_then(|t| t.as_ref());
                let name = info.map(|i| i.name.as_str()).unwrap_or("?");
                let dur = info
                    .and_then(|i| i.duration_secs)
                    .map(fmt_time)
                    .unwrap_or_else(|| "?".into());
                format!(
                    "[{}/{}] {} - {} / {}{}",
                    track + 1,
                    session.queue_len,
                    name,
                    fmt_time(secs),
                    dur,
                    if ended { " (queue finished)" } else { "" },
                )
            }
            None => "opening...".to_string(),
        };

        div()
            .flex_1()
            .flex()
            .flex_col()
            .justify_center()
            .gap_3()
            .child(div().child(status))
            .child(
                div()
                    .h(px(6.))
                    .w_full()
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(self.control("prev", |this, _| this.send(Cmd::Prev), cx))
                    .child(self.control(
                        if playing { "pause" } else { "play" },
                        |this, _| this.send(Cmd::TogglePause),
                        cx,
                    ))
                    .child(self.control("next", |this, _| this.send(Cmd::Next), cx))
                    .child(self.control("-10s", |this, _| this.seek_by(-10.0), cx))
                    .child(self.control("+10s", |this, _| this.seek_by(10.0), cx))
                    .child(self.control("vol -", |this, _| this.nudge_volume(-0.1), cx))
                    .child(self.control(format!("{volume}%"), |_, _| {}, cx))
                    .child(self.control("vol +", |this, _| this.nudge_volume(0.1), cx)),
            )
            .child(
                div()
                    .text_color(rgb(0x808080))
                    .child("drop files to replace the queue"),
            )
    }
}

impl Render for PlaybackProto {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.session.is_some() {
            self.session_state(window, cx).into_any_element()
        } else {
            self.empty_state(cx).into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .p_4()
            .bg(rgb(0x141414))
            .text_color(rgb(0xe0e0e0))
            .text_sm()
            .can_drop(|drag, _, _| drag.downcast_ref::<ExternalPaths>().is_some())
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.load(paths.paths().to_vec(), cx);
            }))
            .children(
                self.error
                    .clone()
                    .map(|e| div().text_color(rgb(0xff8080)).child(e)),
            )
            .child(body)
    }
}

fn fmt_time(secs: f64) -> String {
    let m = (secs / 60.0).floor() as u64;
    format!("{m}:{:04.1}", secs - (m * 60) as f64)
}
