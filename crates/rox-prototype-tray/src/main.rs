//! Research prototype for quit to tray
//! (docs/0R-research/03-quit-to-tray.md): a minimal GPUI app with one
//! window, a ksni tray on its own thread, and a channel into the foreground
//! executor. It exists to answer the writeup's open questions on the Wayland
//! daily driver:
//!
//! - Do tray clicks and menu-state updates round-trip without deadlock?
//! - Can a windowless app reopen a window, and does it get focus?
//! - Does the windowless process idle clean while "playback" keeps going?
//! - Does quit from the tray shut the D-Bus thread down without hanging?
//!
//! Playback is simulated: a ticker thread advances a position counter while
//! the playing flag is set, standing in for the audio engine surviving with
//! zero windows. Run with `cargo run -p rox-prototype-tray`.
//!
//! Stock gpui 0.2.2 stops the Linux event loop when the last window closes
//! (see the writeup's findings), so on an unpatched build the process exits
//! on window close. The windowless answers were gathered with the two-line
//! upstream fix applied to the 0.2.2 source via a local `[patch.crates-io]`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};

/// What the tray asks of the app. Menu closures run on ksni's service
/// thread, so they only send; the foreground drain loop does the work.
enum TrayEvent {
    Open,
    Toggle,
    Quit,
}

/// The simulated player: a position that advances once a second while
/// playing. Shared between the ticker thread, the window view, and the
/// drain loop, standing in for the playback engine's shared state.
struct Shared {
    playing: AtomicBool,
    position: AtomicU64,
}

/// The tray-side state. `playing` is a mirror of the app's flag, pushed
/// back through `Handle::update` so the menu label tracks app state; the
/// tray never owns the truth.
struct ProtoTray {
    playing: bool,
    tx: async_channel::Sender<TrayEvent>,
}

impl ksni::Tray for ProtoTray {
    fn id(&self) -> String {
        "rox-prototype-tray".into()
    }

    fn title(&self) -> String {
        "rox tray prototype".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        // The app icon, downscaled once: the pixmap travels over D-Bus, so
        // shipping the 2048px source would be 16 MB of session bus traffic.
        static ICON: LazyLock<ksni::Icon> = LazyLock::new(|| {
            let img = image::load_from_memory(include_bytes!("../../rox/assets/app/rox.png"))
                .expect("bundled icon decodes");
            let img = img.thumbnail(64, 64);
            let (width, height) = (img.width() as i32, img.height() as i32);
            let mut data = img.into_rgba8().into_vec();
            // RGBA to the spec's ARGB32 network byte order.
            for pixel in data.chunks_exact_mut(4) {
                pixel.rotate_right(1);
            }
            ksni::Icon {
                width,
                height,
                data,
            }
        });
        vec![ICON.clone()]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.try_send(TrayEvent::Open);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.try_send(TrayEvent::Open);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if self.playing { "Pause" } else { "Play" }.into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.try_send(TrayEvent::Toggle);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.try_send(TrayEvent::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// The one window: shows the simulated playback so it's visible that the
/// ticker survives close and reopen. A one-second notify loop keeps the
/// numbers moving; it dies with the view, so a closed window costs nothing.
struct ProtoView {
    shared: Arc<Shared>,
}

impl ProtoView {
    fn new(shared: Arc<Shared>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(1000))
                .await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();
        ProtoView { shared }
    }
}

impl Render for ProtoView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let playing = self.shared.playing.load(Ordering::Relaxed);
        let position = self.shared.position.load(Ordering::Relaxed);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(rgb(0x1a1a1f))
            .text_color(rgb(0xd8d8e0))
            .child(format!(
                "{} at {}:{:02}",
                if playing { "Playing" } else { "Paused" },
                position / 60,
                position % 60
            ))
            .child("Close this window: the process stays, the tray holds the way back.")
            .child("Tray: click reopens, menu has Open / Play-Pause / Quit.")
    }
}

fn open_window(shared: Arc<Shared>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(520.), px(200.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("rox prototype: tray")),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(options, |_, cx| {
        cx.new(|cx| ProtoView::new(shared, cx))
    })
    .expect("failed to open the prototype window");
}

fn main() {
    let shared = Arc::new(Shared {
        playing: AtomicBool::new(true),
        position: AtomicU64::new(0),
    });

    // The stand-in for the audio engine: advances while playing, for the
    // process's whole life, windows or none.
    let ticker = shared.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        if ticker.playing.load(Ordering::Relaxed) {
            ticker.position.fetch_add(1, Ordering::Relaxed);
        }
    });

    Application::new().run(move |cx: &mut App| {
        let (tx, events) = async_channel::unbounded();
        let handle = ProtoTray { playing: true, tx }
            .spawn()
            .expect("tray spawns; needs an SNI host on the session bus");

        // Log every close so the terminal shows the process outliving its
        // windows.
        cx.on_window_closed(|cx| {
            eprintln!("window closed, {} left, still running", cx.windows().len());
        })
        .detach();

        open_window(shared.clone(), cx);

        // The drain loop: tray events land here on the foreground executor.
        // The `handle.update` inside blocks this thread until the service
        // thread acks; that round trip is one of the questions this
        // prototype exists to answer, so it stays the straightforward call.
        let drain = shared.clone();
        cx.spawn(async move |cx| {
            while let Ok(event) = events.recv().await {
                let done = cx.update(|cx| match event {
                    TrayEvent::Open => {
                        if let Some(window) = cx.windows().first() {
                            eprintln!("open: activating the existing window");
                            let _ = window.update(cx, |_, window, _| window.activate_window());
                        } else {
                            eprintln!("open: no window, opening one");
                            open_window(drain.clone(), cx);
                        }
                        false
                    }
                    TrayEvent::Toggle => {
                        let playing = !drain.playing.load(Ordering::Relaxed);
                        drain.playing.store(playing, Ordering::Relaxed);
                        // Push the app's state back to the tray so the menu
                        // label flips. Blocking, see above.
                        handle.update(|tray| tray.playing = playing);
                        eprintln!("toggle: now {}", if playing { "playing" } else { "paused" });
                        false
                    }
                    TrayEvent::Quit => {
                        eprintln!("quit: shutting the tray down");
                        handle.shutdown().wait();
                        eprintln!("quit: tray down, quitting the app");
                        cx.quit();
                        true
                    }
                });
                if done.unwrap_or(true) {
                    break;
                }
            }
        })
        .detach();

        cx.activate(true);
    });
}
