//! Quit to tray: the app resident with zero windows, music playing, and a
//! way back in. Linux uses an SNI icon via ksni, Windows a notification area
//! icon via tray-icon, and on macOS the dock is the tray, so this only holds
//! the kept state. Findings in docs/0R-research/03-quit-to-tray.md.
//!
//! The tray's callbacks only send commands over a channel; a drain on the
//! foreground executor does the work. When the last window closes with the
//! setting on, [`hold`] keeps the player alive until a reopen adopts it.

use gpui::{App, Entity, Global, Subscription};

use crate::integrations::media_controls::MediaSession;
use crate::workspace::Adopted;
use rox_panel_api::panel::AppState;

#[derive(Default)]
struct TrayService {
    hold: Option<Held>,
    #[cfg(target_os = "linux")]
    handle: Option<ksni::blocking::Handle<RoxTray>>,
    #[cfg(target_os = "windows")]
    icon: Option<WindowsTray>,
    /// The last pushed (has_track, playing), so writes happen only on change.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pushed: Option<(bool, bool)>,
}

impl Global for TrayService {}

struct Held {
    state: AppState,
    /// The media service, still answering the hardware keys. `None` on Windows,
    /// where SMTC is bound to the window handle and can't outlive it.
    media: Option<Entity<MediaSession>>,
    /// Keeps the Play/Pause label honest with no workspace publishing.
    _observer: Subscription,
}

pub(crate) fn supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ))
}

/// Whether closing the last window leaves the app reachable. With no SNI host
/// the close quits rather than stranding a headless process.
#[cfg(target_os = "linux")]
pub(crate) fn resident(cx: &mut App) -> bool {
    cx.default_global::<TrayService>().handle.is_some()
}

#[cfg(target_os = "macos")]
pub(crate) fn resident(_cx: &mut App) -> bool {
    true
}

#[cfg(target_os = "windows")]
pub(crate) fn resident(cx: &mut App) -> bool {
    cx.default_global::<TrayService>().icon.is_some()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn resident(_cx: &mut App) -> bool {
    false
}

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

/// Adopts the held state, or opens cold when no hold formed.
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

#[cfg(any(target_os = "linux", target_os = "windows"))]
enum TrayCommand {
    Open,
    Toggle,
    Quit,
}

/// Thumbnailed once: the 2048 px source is 16 MB, and on Linux it travels the bus.
#[cfg(any(target_os = "linux", target_os = "windows"))]
static ICON: std::sync::LazyLock<(u32, u32, Vec<u8>)> = std::sync::LazyLock::new(|| {
    let img = image::load_from_memory(include_bytes!("../../assets/app/rox.png"))
        .expect("bundled icon decodes")
        .thumbnail(64, 64);
    let (width, height) = (img.width(), img.height());
    (width, height, img.into_rgba8().into_vec())
});

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
                label: rox_i18n::t!("tray-open").to_string(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.try_send(TrayCommand::Open);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if self.playing {
                    rox_i18n::t!("tray-pause")
                } else {
                    rox_i18n::t!("tray-play")
                }
                .to_string(),
                enabled: self.has_track,
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.try_send(TrayCommand::Toggle);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: rox_i18n::t!("tray-quit").to_string(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.try_send(TrayCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Put the icon up or take it down to match the setting. No SNI host leaves
/// the handle empty, and the close path quits as if the setting were off.
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
            // Dropping the awaiter lets the service thread wind down; the closed
            // channel ends the drain.
            let _ = handle.shutdown();
        }
    }
}

/// The icon and menu are Rc-backed and pinned to their pump thread, so this
/// side holds only the thread id to post to and the join.
#[cfg(target_os = "windows")]
struct WindowsTray {
    thread: u32,
    join: std::thread::JoinHandle<()>,
}

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

    /// Waits: returning early leaves a dead icon until the shell sweeps it.
    fn shutdown(self) {
        self.post(WM_ROX_TRAY_QUIT, 0);
        let _ = self.join.join();
    }
}

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

/// tray-icon needs a thread running a win32 message loop, and muda's menu
/// items are Rc-backed, so the menu lives here; the app pokes it with
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
        DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, TranslateMessage, WM_USER,
    };

    // PostThreadMessage drops messages for a thread with no queue yet, so force
    // one into being before the id goes out.
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

    let open = MenuItem::with_id("open", rox_i18n::t!("tray-open"), true, None);
    let toggle = MenuItem::with_id("toggle", rox_i18n::t!("tray-play"), false, None);
    let quit = MenuItem::with_id("quit", rox_i18n::t!("tray-quit"), true, None);
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

    // Both crates use process-wide channels, so a previous icon's clicks may
    // still be queued. Drain them.
    while TrayIconEvent::receiver().try_recv().is_ok() {}
    while MenuEvent::receiver().try_recv().is_ok() {}

    if ready.send(Some(unsafe { GetCurrentThreadId() })).is_err() {
        return;
    }

    loop {
        let mut msg = MSG::default();
        if unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } <= 0 {
            break;
        }
        if msg.hwnd.is_null() {
            match msg.message {
                WM_ROX_TRAY_STATE => {
                    toggle.set_text(if msg.wParam & 2 != 0 {
                        rox_i18n::t!("tray-pause")
                    } else {
                        rox_i18n::t!("tray-play")
                    });
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

/// Wait for the icon to be gone so the slot is released before the loop stops.
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

/// Returns true when the app is quitting.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn apply(command: TrayCommand, cx: &mut App) -> bool {
    match command {
        TrayCommand::Open => {
            match rox_panel_api::windows::front_workspace(cx) {
                Some((window, _)) => {
                    window
                        .update(cx, |_, window, _| window.activate_window())
                        .ok();
                }
                _ => {
                    reopen(cx);
                }
            }
            false
        }
        TrayCommand::Toggle => {
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

/// Blocks until the tray thread acks. The menu closures never call into gpui,
/// so the two threads can't deadlock.
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

/// A posted thread message, so this never waits.
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
