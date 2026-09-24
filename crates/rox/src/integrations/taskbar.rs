//! Taskbar progress: the OS launcher button showing how far the background
//! jobs have got, with or without the tasks window open.
//!
//! Windows paints into the taskbar button through ITaskbarList3, which needs
//! the window handle and gpui's COM apartment, so it stays on the foreground
//! executor. Linux uses Unity's LauncherEntry signal, a session-bus broadcast
//! keyed by desktop file. macOS isn't wired.
//!
//! One sampler runs while any job does; writes are gated on the whole
//! percent moving.

use std::time::Duration;

use gpui::{App, Entity, Global};

use rox_services::catalog::{Library, LibraryJob};

/// The tasks window's own tick.
const TICK: Duration = Duration::from_millis(500);

#[derive(Default)]
struct Taskbar {
    watching: bool,
    /// `None` for nothing running, so an idle app never writes.
    pushed: Option<u8>,
    #[cfg(target_os = "linux")]
    unity: Option<async_channel::Sender<Push>>,
    /// `Some(None)`: creation failed and we stopped asking.
    #[cfg(target_os = "windows")]
    list: Option<Option<windows::Win32::UI::Shell::ITaskbarList3>>,
}

impl Global for Taskbar {}

/// Start the sampler if it isn't up. It ends itself after clearing the button.
pub(crate) fn watch(cx: &mut App) {
    if cx.default_global::<Taskbar>().watching {
        return;
    }
    cx.default_global::<Taskbar>().watching = true;
    cx.spawn(async move |cx| {
        // A tick in: the caller is mid-start and its counts aren't built yet.
        loop {
            cx.background_executor().timer(TICK).await;
            if !matches!(cx.update(sync), Ok(true)) {
                break;
            }
        }
        cx.update(|cx| cx.default_global::<Taskbar>().watching = false)
            .ok();
    })
    .detach();
}

/// Scans never touch the tasks window's ticker, so the catalog's event starts
/// the sampler. The launch scan starts inside `Library::new`, before anything
/// can subscribe, hence the check up front.
pub(crate) fn follow(library: &Entity<Library>, cx: &mut App) {
    if library.read(cx).scanning() {
        watch(cx);
    }
    App::subscribe(cx, library, |_, event, cx| {
        if matches!(event, LibraryJob::ScanStarted) {
            watch(cx);
        }
    })
    .detach();
}

/// Returns whether anything is still running. Runs off the sampler's own
/// update, so the Windows arm can take a window out of its slot.
fn sync(cx: &mut App) -> bool {
    let percent = crate::tasks_window::aggregate(cx).map(|(done, total)| match total {
        // Busy but no total yet: zero rather than nothing.
        0 => 0,
        total => (done * 100 / total).min(100) as u8,
    });
    let state = cx.default_global::<Taskbar>();
    if state.pushed == percent {
        return percent.is_some();
    }
    state.pushed = percent;
    publish(percent, cx);
    percent.is_some()
}

/// The clear carries an ack: the one at quit has to land before the process exits.
#[cfg(target_os = "linux")]
enum Push {
    Set(Option<u8>),
    Clear(async_channel::Sender<()>),
}

/// Consumers match the interface and member, not the path; it only has to be stable.
#[cfg(target_os = "linux")]
const PATH: &str = "/com/canonical/unity/launcherentry/rox";

#[cfg(target_os = "linux")]
const APP_URI: &str = "application://rox.desktop";

#[cfg(target_os = "linux")]
fn publish(percent: Option<u8>, cx: &mut App) {
    let tx = match cx.default_global::<Taskbar>().unity.clone() {
        Some(tx) => tx,
        None => {
            let (tx, rx) = async_channel::unbounded();
            cx.background_executor().spawn(serve(rx)).detach();
            cx.default_global::<Taskbar>().unity = Some(tx.clone());
            // The launcher keeps the last value, so clear the bar on quit.
            cx.on_app_quit(|cx| {
                let tx = cx.default_global::<Taskbar>().unity.clone();
                async move {
                    let Some(tx) = tx else {
                        return;
                    };
                    let (done, landed) = async_channel::bounded(1);
                    if tx.send(Push::Clear(done)).await.is_ok() {
                        let _ = landed.recv().await;
                    }
                }
            })
            .detach();
            tx
        }
    };
    let _ = tx.try_send(Push::Set(percent));
}

#[cfg(target_os = "linux")]
async fn serve(rx: async_channel::Receiver<Push>) {
    let conn = zbus::Connection::session().await;
    if let Err(err) = &conn {
        log::warn!("taskbar: no session bus, no launcher progress: {err}");
    }
    while let Ok(push) = rx.recv().await {
        let (percent, ack) = match push {
            Push::Set(percent) => (percent, None),
            Push::Clear(ack) => (None, Some(ack)),
        };
        if let Ok(conn) = &conn {
            emit(conn, percent).await;
        }
        // The quit path waits on this, even when the bus never came up.
        if let Some(ack) = ack {
            let _ = ack.send(()).await;
        }
    }
}

/// Plasma, Unity, and Dash to Dock draw these; stock GNOME ignores them.
#[cfg(target_os = "linux")]
async fn emit(conn: &zbus::Connection, percent: Option<u8>) {
    use zbus::zvariant::Value;
    let props = std::collections::HashMap::from([
        (
            "progress",
            Value::from(f64::from(percent.unwrap_or(0)) / 100.),
        ),
        ("progress-visible", Value::from(percent.is_some())),
    ]);
    let sent = conn
        .emit_signal(
            None::<&str>,
            PATH,
            "com.canonical.Unity.LauncherEntry",
            "Update",
            &(APP_URI, props),
        )
        .await;
    if let Err(err) = sent {
        log::debug!("taskbar: launcher update went nowhere: {err}");
    }
}

/// Known limitation: an Explorer restart broadcasts TaskbarButtonCreated,
/// which nothing listens for, so the bar returns on the next percent.
#[cfg(target_os = "windows")]
fn publish(percent: Option<u8>, cx: &mut App) {
    use windows::Win32::UI::Shell::{TBPF_NOPROGRESS, TBPF_NORMAL};

    // The front workspace's button. Checked before the COM object so a
    // windowless run never creates one.
    let Some((handle, _)) = rox_panel_api::windows::front_workspace(cx) else {
        return;
    };
    let hwnd = handle
        .update(cx, |_, window, _| window_hwnd(window))
        .ok()
        .flatten();
    let Some(hwnd) = hwnd else {
        return;
    };
    let Some(list) = cx
        .default_global::<Taskbar>()
        .list
        .get_or_insert_with(create_list)
        .clone()
    else {
        return;
    };
    unsafe {
        match percent {
            Some(percent) => {
                let _ = list.SetProgressState(hwnd, TBPF_NORMAL);
                let _ = list.SetProgressValue(hwnd, u64::from(percent), 100);
            }
            None => {
                let _ = list.SetProgressState(hwnd, TBPF_NOPROGRESS);
            }
        }
    }
}

/// `None` leaves the app without a bar rather than retrying forever.
#[cfg(target_os = "windows")]
fn create_list() -> Option<windows::Win32::UI::Shell::ITaskbarList3> {
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};

    unsafe {
        let list: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_ALL)
            .inspect_err(|err| log::warn!("taskbar: no taskbar list, no progress bar: {err}"))
            .ok()?;
        // ITaskbarList needs this before any other call.
        list.HrInit().ok()?;
        Some(list)
    }
}

#[cfg(target_os = "windows")]
fn window_hwnd(window: &gpui::Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    // gpui's inherent window_handle() shadows the trait method, so call it
    // through the trait.
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(windows::Win32::Foundation::HWND(
            handle.hwnd.get() as *mut std::ffi::c_void
        )),
        _ => None,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn publish(_percent: Option<u8>, _cx: &mut App) {}
