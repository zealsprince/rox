//! Quit to tray: the app resident with zero windows, music playing, and a
//! way back in. On Linux that is an SNI icon over D-Bus via ksni, the same
//! zbus stack the media keys ride; on Windows a notification area icon
//! through tray-icon, which is Shell_NotifyIcon with no GTK anywhere; on
//! macOS the dock already is the tray and this module only carries the held
//! state for `on_reopen`.
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

use gpui::{App, Entity, Global, Subscription};

use crate::integrations::media_controls::MediaSession;
use crate::workspace::Adopted;
use rox_panel_api::panel::AppState;

/// The tray's app-side state. The hold exists on every platform; the icon
/// handle and its push gate exist where there is a real icon to talk to,
/// alive exactly while the setting is on and the platform played along.
#[derive(Default)]
struct TrayService {
    hold: Option<Held>,
    #[cfg(target_os = "linux")]
    handle: Option<ksni::blocking::Handle<RoxTray>>,
    #[cfg(target_os = "windows")]
    icon: Option<WindowsTray>,
    /// The (has_track, playing) pair last pushed to the icon, so the
    /// steady stream of player notifies writes only on change.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pushed: Option<(bool, bool)>,
}

impl Global for TrayService {}

/// The shared state stashed by the last window close, keeping the playing
/// player alive while no window holds it.
struct Held {
    state: AppState,
    /// The OS media service, still registered and still answering the
    /// hardware keys with no window behind it. `None` on Windows, where SMTC
    /// is bound to the window handle and can't outlive it, and wherever the
    /// service never came up in the first place.
    media: Option<Entity<MediaSession>>,
    /// Keeps the menu's Play/Pause label honest while no workspace drives
    /// the publish path - a track running out flips it windowless.
    _observer: Subscription,
}

/// Whether this platform has a way back into a windowless app: the tray
/// icon on Linux and Windows, the dock on macOS. The Application row hides
/// where this is false, and the close path quits regardless of the setting.
pub(crate) fn supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ))
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

/// The same honesty as Linux: true only while the pump thread is up with an
/// icon in the notification area.
#[cfg(target_os = "windows")]
pub(crate) fn resident(cx: &mut App) -> bool {
    cx.default_global::<TrayService>().icon.is_some()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn resident(_cx: &mut App) -> bool {
    false
}

/// Stash the closing primary's state and its media service, and watch the
/// player so the tray label stays current without a window. The service
/// keeps running here, which is what answers the media keys from the tray.
pub(crate) fn hold(state: AppState, media: Option<Entity<MediaSession>>, cx: &mut App) {
    let observer = cx.observe(&state.player, |player, cx| {
        let (has_track, playing) = {
            let player = player.read(cx);
            (player.now_playing().is_some(), player.is_playing())
        };
        set_playing(has_track, playing, cx);
    });
    cx.default_global::<TrayService>().hold = Some(Held {
        state,
        media,
        _observer: observer,
    });
}

/// Bring a workspace window back: over the held state when the close
/// stashed one, cold otherwise (quit-to-tray turned on mid-session on
/// macOS, say, where no hold ever formed).
pub(crate) fn reopen(cx: &mut App) {
    let held = cx.default_global::<TrayService>().hold.take();
    match held {
        Some(held) => crate::open_workspace_adopting(
            Adopted {
                state: held.state,
                media: held.media,
            },
            cx,
        ),
        None => crate::open_workspace(cx),
    }
}

/// What the tray asks of the app. The menu callbacks run on the tray's own
/// thread, so they only send; the drain on the foreground executor does the
/// work.
#[cfg(any(target_os = "linux", target_os = "windows"))]
enum TrayCommand {
    Open,
    Toggle,
    Quit,
}

/// The app icon decoded and thumbnailed once, as (width, height, RGBA). The
/// 2048 px source is 16 MB of pixels, and on Linux every one of them travels
/// over the session bus.
#[cfg(any(target_os = "linux", target_os = "windows"))]
static ICON: std::sync::LazyLock<(u32, u32, Vec<u8>)> = std::sync::LazyLock::new(|| {
    let img = image::load_from_memory(include_bytes!("../../assets/app/rox.png"))
        .expect("bundled icon decodes")
        .thumbnail(64, 64);
    let (width, height) = (img.width(), img.height());
    (width, height, img.into_rgba8().into_vec())
});

/// Hand the tray's channel to the foreground executor. The drain outlives
/// this call and ends when the channel closes or the app quits.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn drain(events: async_channel::Receiver<TrayCommand>, cx: &mut App) {
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

#[cfg(target_os = "linux")]
struct RoxTray {
    has_track: bool,
    playing: bool,
    tx: async_channel::Sender<TrayCommand>,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for RoxTray {
    fn id(&self) -> String {
        rox_core::APP_ID.into()
    }

    fn title(&self) -> String {
        "rox".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        static PIXMAP: std::sync::LazyLock<ksni::Icon> = std::sync::LazyLock::new(|| {
            let (width, height, mut data) = ICON.clone();
            // RGBA to the spec's ARGB32 network byte order.
            for pixel in data.as_chunks_mut::<4>().0 {
                pixel.rotate_right(1);
            }
            ksni::Icon {
                width: width as i32,
                height: height as i32,
                data,
            }
        });
        vec![PIXMAP.clone()]
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
    let on = rox_core::settings::quit_to_tray();
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
                drain(events, cx);
            }
            Err(err) => log::warn!("tray: no status notifier host, staying window-bound: {err}"),
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

/// The Windows icon and its menu, from the app's side. Both are Rc-backed
/// and pinned to the thread that pumps their messages, so this end holds no
/// handle at all: just the thread id to post to, and the join that proves
/// the icon is gone.
#[cfg(target_os = "windows")]
struct WindowsTray {
    thread: u32,
    join: std::thread::JoinHandle<()>,
}

/// Our private thread messages into the pump. WM_APP up is the range
/// Windows reserves for an application's own use.
#[cfg(target_os = "windows")]
const WM_ROX_TRAY_STATE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 1;
#[cfg(target_os = "windows")]
const WM_ROX_TRAY_QUIT: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 2;

#[cfg(target_os = "windows")]
impl WindowsTray {
    fn post(&self, message: u32, wparam: usize) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread,
                message,
                wparam,
                0,
            );
        }
    }

    /// Wait for the thread rather than just asking it to go: the icon drops
    /// there, and returning early would leave a dead one in the notification
    /// area until the shell next swept it.
    fn shutdown(self) {
        self.post(WM_ROX_TRAY_QUIT, 0);
        let _ = self.join.join();
    }
}

/// Put the icon up on its own thread and hand back the way to talk to it.
/// None when the icon or its menu wouldn't build, which leaves the close
/// path quitting as if the setting were off.
#[cfg(target_os = "windows")]
fn spawn_windows_tray(tx: async_channel::Sender<TrayCommand>) -> Option<WindowsTray> {
    let (ready, up) = std::sync::mpsc::channel();
    let join = std::thread::Builder::new()
        .name("rox-tray".into())
        .spawn(move || windows_tray_thread(tx, ready))
        .ok()?;
    match up.recv() {
        Ok(Some(thread)) => Some(WindowsTray { thread, join }),
        _ => {
            let _ = join.join();
            None
        }
    }
}

/// The pump thread. tray-icon needs the icon created on a thread running a
/// win32 message loop, and muda's menu items are Rc-backed, so the menu is
/// built here and only ever touched here; the app pokes it with
/// [`WM_ROX_TRAY_STATE`].
#[cfg(target_os = "windows")]
fn windows_tray_thread(
    tx: async_channel::Sender<TrayCommand>,
    ready: std::sync::mpsc::Sender<Option<u32>>,
) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PeekMessageW, TranslateMessage, MSG, PM_NOREMOVE, WM_USER,
    };

    // PostThreadMessage throws messages away for a thread that has never
    // asked for one, so force the queue into being before the id goes out.
    let mut probe = MSG::default();
    unsafe {
        PeekMessageW(
            &mut probe,
            std::ptr::null_mut(),
            WM_USER,
            WM_USER,
            PM_NOREMOVE,
        )
    };

    let icon = match Icon::from_rgba(ICON.2.clone(), ICON.0, ICON.1) {
        Ok(icon) => icon,
        Err(err) => {
            log::warn!("tray: icon rejected, staying window-bound: {err}");
            let _ = ready.send(None);
            return;
        }
    };

    let open = MenuItem::with_id("open", "Open", true, None);
    let toggle = MenuItem::with_id("toggle", "Play", false, None);
    let quit = MenuItem::with_id("quit", "Quit", true, None);
    let menu = Menu::new();
    if let Err(err) = menu.append_items(&[&open, &toggle, &PredefinedMenuItem::separator(), &quit])
    {
        log::warn!("tray: menu would not build, staying window-bound: {err}");
        let _ = ready.send(None);
        return;
    }

    let built = TrayIconBuilder::new()
        .with_id(rox_core::APP_ID)
        .with_title("rox")
        .with_tooltip("rox")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        // Left click is the way back in, so the menu stays on right click.
        .with_menu_on_left_click(false)
        .build();
    let _tray = match built {
        Ok(tray) => tray,
        Err(err) => {
            log::warn!("tray: no notification area icon, staying window-bound: {err}");
            let _ = ready.send(None);
            return;
        }
    };

    // Both crates fan their events through process-wide channels, so a run
    // that turned the setting off and on again can leave the last icon's
    // clicks sitting there. They mean nothing to this one.
    while TrayIconEvent::receiver().try_recv().is_ok() {}
    while MenuEvent::receiver().try_recv().is_ok() {}

    if ready.send(Some(unsafe { GetCurrentThreadId() })).is_err() {
        return;
    }

    loop {
        let mut msg = MSG::default();
        // Zero is WM_QUIT, negative is an error there is no recovering from.
        if unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } <= 0 {
            break;
        }
        // Thread messages belong to no window, so they never get dispatched.
        if msg.hwnd.is_null() {
            match msg.message {
                WM_ROX_TRAY_STATE => {
                    toggle.set_text(if msg.wParam & 2 != 0 { "Pause" } else { "Play" });
                    toggle.set_enabled(msg.wParam & 1 != 0);
                    continue;
                }
                WM_ROX_TRAY_QUIT => break,
                _ => {}
            }
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Both crates post from the window procedures this just dispatched
        // into, so whatever the click meant is on their channels by now.
        for event in TrayIconEvent::receiver().try_iter() {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = tx.try_send(TrayCommand::Open);
            }
        }
        for event in MenuEvent::receiver().try_iter() {
            let command = match event.id.0.as_str() {
                "open" => TrayCommand::Open,
                "toggle" => TrayCommand::Toggle,
                "quit" => TrayCommand::Quit,
                _ => continue,
            };
            let _ = tx.try_send(command);
        }
    }
}

/// Reconcile the icon with the setting, the Windows half. A thread that
/// cannot get an icon into the notification area leaves the slot empty and
/// the close path quitting as if the setting were off.
#[cfg(target_os = "windows")]
pub(crate) fn sync(cx: &mut App) {
    let on = rox_core::settings::quit_to_tray();
    let has = cx.default_global::<TrayService>().icon.is_some();
    if on && !has {
        let (tx, events) = async_channel::unbounded();
        let Some(icon) = spawn_windows_tray(tx) else {
            return;
        };
        let service = cx.default_global::<TrayService>();
        service.icon = Some(icon);
        service.pushed = None;
        drain(events, cx);
    } else if !on && has {
        let service = cx.default_global::<TrayService>();
        service.pushed = None;
        if let Some(icon) = service.icon.take() {
            icon.shutdown();
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn sync(_cx: &mut App) {}

/// Take the icon down and wait for it to actually be gone, so the platform
/// has released the slot before the event loop stops. The prototype timed
/// the whole exit under 200 ms on the D-Bus side.
#[cfg(target_os = "linux")]
fn shutdown(cx: &mut App) {
    if let Some(handle) = cx.default_global::<TrayService>().handle.take() {
        handle.shutdown().wait();
    }
}

#[cfg(target_os = "windows")]
fn shutdown(cx: &mut App) {
    if let Some(icon) = cx.default_global::<TrayService>().icon.take() {
        icon.shutdown();
    }
}

/// One tray command against the app, on the foreground executor. Returns
/// true when the app is quitting and the drain should end.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn apply(command: TrayCommand, cx: &mut App) -> bool {
    match command {
        TrayCommand::Open => {
            if let Some((window, _)) = rox_panel_api::windows::front_workspace(cx) {
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
            let state = rox_panel_api::windows::front_workspace(cx)
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
            shutdown(cx);
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

/// The same gate on Windows, except the push is a posted thread message the
/// pump picks up on its own time, so this never waits on anything.
#[cfg(target_os = "windows")]
pub(crate) fn set_playing(has_track: bool, playing: bool, cx: &mut App) {
    let service = cx.default_global::<TrayService>();
    if service.pushed == Some((has_track, playing)) {
        return;
    }
    let Some(icon) = service.icon.as_ref() else {
        return;
    };
    icon.post(
        WM_ROX_TRAY_STATE,
        usize::from(has_track) | usize::from(playing) << 1,
    );
    service.pushed = Some((has_track, playing));
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn set_playing(_has_track: bool, _playing: bool, _cx: &mut App) {}
