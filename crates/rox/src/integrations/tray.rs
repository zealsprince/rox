//! Quit to tray: the app resident with zero windows, music playing, and a
//! way back in. On Linux that is an SNI icon over D-Bus via ksni, the same
//! zbus stack the media keys ride; on macOS the dock already is the tray
//! and this module only carries the held state for `on_reopen`; Windows has
//! no backend yet, so the close path ignores the setting there.
//!
//! The research entry (docs/0R-research/03-quit-to-tray.md) holds the
//! findings this leans on. The shape matches [`crate::integrations::media_controls`]: the
//! tray's callbacks land on its own service thread and only send commands
//! over an async channel; a drain task on the foreground executor does the
//! work. State flows the other way through [`set_playing`], gated so player
//! notifies don't become D-Bus writes.
//!
//! When the last workspace window closes with the setting on,
//! [`crate::workspace::close_workspace_window`] hands the shared state to
//! [`hold`] instead of quitting. The hold keeps the player and its engine
//! alive, and the tray's Open (or the dock click) adopts it into a fresh
//! window through [`crate::open_workspace_adopting`].

use gpui::{App, Global, Subscription};

use crate::panel::AppState;

/// The tray's app-side state. The hold exists on every platform; the icon
/// handle and its push gate are Linux-only, alive exactly while the
/// setting is on and an SNI host answered.
#[derive(Default)]
struct TrayService {
    hold: Option<Held>,
    #[cfg(target_os = "linux")]
    handle: Option<ksni::blocking::Handle<RoxTray>>,
    /// The (has_track, playing) pair last pushed to the icon, so the
    /// steady stream of player notifies writes only on change.
    #[cfg(target_os = "linux")]
    pushed: Option<(bool, bool)>,
}

impl Global for TrayService {}

/// The shared state stashed by the last window close, keeping the playing
/// player alive while no window holds it.
struct Held {
    state: AppState,
    /// Keeps the menu's Play/Pause label honest while no workspace drives
    /// the publish path - a track running out flips it windowless.
    _observer: Subscription,
}

/// Whether this platform has a way back into a windowless app: the tray
/// icon on Linux, the dock on macOS. The Behavior row hides where this is
/// false, and the close path quits regardless of the setting.
pub(crate) fn supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
}

/// Whether closing the last window can leave the app reachable right now.
/// On Linux that means the icon actually made it onto the bus; a missing
/// SNI host falls back to quitting rather than stranding a headless
/// process.
#[cfg(target_os = "linux")]
pub(crate) fn resident(cx: &mut App) -> bool {
    cx.default_global::<TrayService>().handle.is_some()
}

#[cfg(target_os = "macos")]
pub(crate) fn resident(_cx: &mut App) -> bool {
    true
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn resident(_cx: &mut App) -> bool {
    false
}

/// Stash the closing primary's state and watch its player so the tray
/// label stays current without a window.
pub(crate) fn hold(state: AppState, cx: &mut App) {
    let observer = cx.observe(&state.player, |player, cx| {
        let (has_track, playing) = {
            let player = player.read(cx);
            (player.now_playing().is_some(), player.is_playing())
        };
        set_playing(has_track, playing, cx);
    });
    cx.default_global::<TrayService>().hold = Some(Held {
        state,
        _observer: observer,
    });
}

/// Bring a workspace window back: over the held state when the close
/// stashed one, cold otherwise (quit-to-tray turned on mid-session on
/// macOS, say, where no hold ever formed).
pub(crate) fn reopen(cx: &mut App) {
    let held = cx
        .default_global::<TrayService>()
        .hold
        .take()
        .map(|held| held.state);
    match held {
        Some(state) => crate::open_workspace_adopting(state, cx),
        None => crate::open_workspace(cx),
    }
}

/// What the tray asks of the app. Menu closures run on ksni's service
/// thread, so they only send; the drain on the foreground executor does
/// the work.
#[cfg(target_os = "linux")]
enum TrayCommand {
    Open,
    Toggle,
    Quit,
}

#[cfg(target_os = "linux")]
struct RoxTray {
    has_track: bool,
    playing: bool,
    tx: async_channel::Sender<TrayCommand>,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for RoxTray {
    fn id(&self) -> String {
        crate::APP_ID.into()
    }

    fn title(&self) -> String {
        "rox".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        // The app icon downscaled once: the pixmap travels over the session
        // bus, and the 2048 px source would be 16 MB of it.
        static ICON: std::sync::LazyLock<ksni::Icon> = std::sync::LazyLock::new(|| {
            let img = image::load_from_memory(include_bytes!("../../assets/app/rox.png"))
                .expect("bundled icon decodes")
                .thumbnail(64, 64);
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
        let _ = self.tx.try_send(TrayCommand::Open);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.try_send(TrayCommand::Open);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if self.playing { "Pause" } else { "Play" }.into(),
                enabled: self.has_track,
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.try_send(TrayCommand::Toggle);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.try_send(TrayCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Reconcile the icon with the setting: put it up when quit-to-tray turns
/// on, take it down when it turns off. Called at startup and from both
/// toggles. Failing to reach an SNI host leaves the handle empty and the
/// close path quitting as if the setting were off.
#[cfg(target_os = "linux")]
pub(crate) fn sync(cx: &mut App) {
    use ksni::blocking::TrayMethods as _;
    let on = crate::settings::quit_to_tray();
    let has = cx.default_global::<TrayService>().handle.is_some();
    if on && !has {
        let (tx, events) = async_channel::unbounded();
        let tray = RoxTray {
            has_track: false,
            playing: false,
            tx,
        };
        match tray.spawn() {
            Ok(handle) => {
                let service = cx.default_global::<TrayService>();
                service.handle = Some(handle);
                service.pushed = None;
                cx.spawn(async move |cx| {
                    while let Ok(command) = events.recv().await {
                        let quit = cx.update(|cx| apply(command, cx)).unwrap_or(true);
                        if quit {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(err) => eprintln!("tray: no status notifier host, staying window-bound: {err}"),
        }
    } else if !on && has {
        let service = cx.default_global::<TrayService>();
        service.pushed = None;
        if let Some(handle) = service.handle.take() {
            // Fire and forget: dropping the awaiter lets the service thread
            // wind down on its own, and the closed channel ends the drain.
            let _ = handle.shutdown();
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn sync(_cx: &mut App) {}

/// One tray command against the app, on the foreground executor. Returns
/// true when the app is quitting and the drain should end.
#[cfg(target_os = "linux")]
fn apply(command: TrayCommand, cx: &mut App) -> bool {
    match command {
        TrayCommand::Open => {
            if let Some((window, _)) = crate::workspace::front_workspace(cx) {
                window
                    .update(cx, |_, window, _| window.activate_window())
                    .ok();
            } else {
                reopen(cx);
            }
            false
        }
        TrayCommand::Toggle => {
            // A window's state when one is open, the hold's when resident.
            let state = crate::workspace::front_workspace(cx)
                .map(|(_, state)| state)
                .or_else(|| {
                    cx.default_global::<TrayService>()
                        .hold
                        .as_ref()
                        .map(|held| held.state.clone())
                });
            if let Some(state) = state {
                state.player.update(cx, |player, cx| {
                    player.toggle_pause();
                    cx.notify();
                });
                let (has_track, playing) = {
                    let player = state.player.read(cx);
                    (player.now_playing().is_some(), player.is_playing())
                };
                set_playing(has_track, playing, cx);
            }
            false
        }
        TrayCommand::Quit => {
            // The tray comes down first so the D-Bus name is gone before
            // the loop stops; the prototype timed the whole exit under
            // 200 ms.
            if let Some(handle) = cx.default_global::<TrayService>().handle.take() {
                handle.shutdown().wait();
            }
            cx.quit();
            true
        }
    }
}

/// Push play state to the icon's menu, gated on change. The push blocks
/// until the tray thread acks, which the prototype measured as effectively
/// instant; the menu closures never call back into gpui, so the two
/// threads cannot wait on each other.
#[cfg(target_os = "linux")]
pub(crate) fn set_playing(has_track: bool, playing: bool, cx: &mut App) {
    let service = cx.default_global::<TrayService>();
    let Some(handle) = service.handle.clone() else {
        return;
    };
    if service.pushed == Some((has_track, playing)) {
        return;
    }
    service.pushed = Some((has_track, playing));
    handle.update(|tray| {
        tray.has_track = has_track;
        tray.playing = playing;
    });
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn set_playing(_has_track: bool, _playing: bool, _cx: &mut App) {}
