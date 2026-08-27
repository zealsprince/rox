//! Taskbar progress: the OS launcher button showing how far the background
//! jobs have got. A scan is minutes and the acoustic pass is an afternoon,
//! and neither is worth going and finding a window for when the button is
//! already on screen. Nothing here reads a view, so it works with the tasks
//! window shut.
//!
//! Two backends with nothing in common but the sampler. Windows paints the
//! bar into the taskbar button through ITaskbarList3, which needs the
//! window's own handle and the COM apartment gpui's platform init already
//! set up, so those calls stay on the foreground executor. Linux has no
//! equivalent; what the desktops settled on is Unity's LauncherEntry
//! signal, a session-bus broadcast keyed by desktop file, with no window in
//! it at all. macOS has the dock tile and isn't wired here.
//!
//! [`watch`] is the whole surface. Each job start calls it, either directly
//! or through [`follow`] for the scans the catalog announces, one sampler
//! spins at [`TICK`] while anything runs and stops itself once the last one
//! ends. Writes are gated on the whole percent moving, the same shape as
//! [`super::tray::set_playing`] against player notifies: a scan counts ten
//! times a second and the button draws a hundred steps.

use std::time::Duration;

use gpui::{App, Entity, Global};

use rox_services::catalog::{Library, LibraryJob};

/// How often the sampler reads the running jobs. The tasks window's own
/// tick, for the same reason: a bar this coarse has nothing to say more
/// often, and the sample iterates over every job.
const TICK: Duration = Duration::from_millis(500);

/// The button's state as this side knows it, plus whatever the platform
/// needs to write it with.
#[derive(Default)]
struct Taskbar {
    /// Whether the sampler is up, so the second job to start doesn't spawn
    /// one of its own.
    watching: bool,
    /// The whole percent last written out, `None` for nothing running,
    /// which is also where this starts, so an idle app never writes at all.
    pushed: Option<u8>,
    /// The connection task's end of the wire, stood up on the first write.
    #[cfg(target_os = "linux")]
    unity: Option<async_channel::Sender<Push>>,
    /// The taskbar COM object, created on the first write and held for the
    /// session. `Some(None)` means it wouldn't create and we stopped asking.
    #[cfg(target_os = "windows")]
    list: Option<Option<windows::Win32::UI::Shell::ITaskbarList3>>,
}

impl Global for Taskbar {}

/// Start the sampler if it isn't already up. Called wherever a job starts;
/// the loop ends on its own once the last one stops, after the write that
/// clears the button.
pub(crate) fn watch(cx: &mut App) {
    if cx.default_global::<Taskbar>().watching {
        return;
    }
    cx.default_global::<Taskbar>().watching = true;
    cx.spawn(async move |cx| {
        // A tick in rather than right now: this is called from inside the
        // job's own start, where the counts it would read are still being
        // put together.
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

/// Follow a library's scans. A scan is the one job that never touches the
/// tasks window's ticker, so the sampler is started off the catalog's own
/// event instead.
///
/// The launch catch-up scan starts inside `Library::new`, before anything
/// can be subscribed to it, so a library that's already scanning gets the
/// sampler here rather than waiting for the next one.
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

/// Read what's running and write the button if the picture moved. Returns
/// whether anything is still going, which keeps the sampler alive.
///
/// Runs off the sampler's own update, so nothing is mid-update and the
/// Windows arm can take a window out of its slot for the handle.
fn sync(cx: &mut App) -> bool {
    let percent = crate::tasks_window::aggregate(cx).map(|(done, total)| match total {
        // Still working out what there is to do. Zero rather than nothing,
        // the button says busy, it just can't say how far yet.
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

/// What the connection task is told to do. The clear includes an ack because
/// the one at quit has to go out before the process does.
#[cfg(target_os = "linux")]
enum Push {
    Set(Option<u8>),
    Clear(async_channel::Sender<()>),
}

/// Where the signal goes out from. Consumers match on the interface and the
/// member rather than the path, so this only has to be stable and ours; the
/// convention is the app's own id under the launcher entry tree.
#[cfg(target_os = "linux")]
const PATH: &str = "/com/canonical/unity/launcherentry/rox";

/// Which launcher entry the update is about, matched against the installed
/// desktop file.
#[cfg(target_os = "linux")]
const APP_URI: &str = "application://rox.desktop";

/// Hand the write to the connection task, standing one up on the first
/// call. Nothing waits here: the send is into an unbounded channel and the
/// emit happens on the background executor, the same discipline the tray
/// and the media keys keep.
#[cfg(target_os = "linux")]
fn publish(percent: Option<u8>, cx: &mut App) {
    let tx = match cx.default_global::<Taskbar>().unity.clone() {
        Some(tx) => tx,
        None => {
            let (tx, rx) = async_channel::unbounded();
            cx.background_executor().spawn(serve(rx)).detach();
            cx.default_global::<Taskbar>().unity = Some(tx.clone());
            // The launcher remembers the last thing it was told, so quitting
            // with a bar up would leave one showing on a closed app. Only
            // worth arming once there's something to take back down.
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

/// Own the bus connection and write every update in order. Ends when the
/// app drops the sending half.
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
        // The quit path is waiting on this; a bus that never came up still
        // has to let it go.
        if let Some(ack) = ack {
            let _ = ack.send(()).await;
        }
    }
}

/// One LauncherEntry update out on the session bus. Plasma, Unity, and
/// GNOME's Dash to Dock draw these; stock GNOME ignores them, so there the
/// button stays as it was.
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
    // A broadcast, so there's no name to own and nobody to be missing: it
    // goes out whether a launcher is listening or not.
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

/// Write the bar into the taskbar button. The button has to exist for this
/// to work, which it does by the time a job can have been started from a
/// window. Known limitation: an Explorer restart rebuilds every button and
/// broadcasts TaskbarButtonCreated, which nothing here listens for, so the
/// bar comes back on the next percent rather than immediately.
#[cfg(target_os = "windows")]
fn publish(percent: Option<u8>, cx: &mut App) {
    use windows::Win32::UI::Shell::{TBPF_NOPROGRESS, TBPF_NORMAL};

    // The bar belongs to a window, so it goes on whichever workspace is in
    // front, the same one the tasks window reads its scan off. Asked for
    // before the COM object so a windowless run never creates one.
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

/// The taskbar COM object, created on the thread that owns the windows and
/// the apartment gpui already initialized. `None` when it wouldn't come up,
/// which leaves the app running without a bar rather than retrying forever.
#[cfg(target_os = "windows")]
fn create_list() -> Option<windows::Win32::UI::Shell::ITaskbarList3> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};

    unsafe {
        let list: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_ALL)
            .inspect_err(|err| log::warn!("taskbar: no taskbar list, no progress bar: {err}"))
            .ok()?;
        // ITaskbarList needs this before anything else on the interface.
        list.HrInit().ok()?;
        Some(list)
    }
}

/// The Win32 handle of the window the bar is drawn on, pulled off the gpui
/// window the same way the media keys pull theirs.
#[cfg(target_os = "windows")]
fn window_hwnd(window: &gpui::Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    // gpui's inherent Window::window_handle() returns AnyWindowHandle and
    // shadows the trait, so reach for the raw handle through the trait.
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(windows::Win32::Foundation::HWND(
            handle.hwnd.get() as *mut std::ffi::c_void
        )),
        _ => None,
    }
}

/// macOS keeps this sort of thing on the dock tile, which is its own
/// surface and its own decision. Nothing is wired there, so the sampler
/// runs and the write goes nowhere.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn publish(_percent: Option<u8>, _cx: &mut App) {}
