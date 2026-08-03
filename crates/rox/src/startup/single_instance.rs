//! One rox per data directory.
//!
//! Launching rox while a rox is already running - a click on the pinned
//! launcher, a file manager's Open With, a second `rox` in a terminal - used
//! to start a whole second process, with its own window, its own tray icon,
//! and its own claim on the media keys. With quit-to-tray on that reads as
//! the tray opening a duplicate window: the resident instance is still there
//! playing, and a stranger shows up next to it.
//!
//! So the first instance binds a Unix socket keyed to its data directory, and
//! every later launch connects, hands over what it was asked to open, and
//! exits. The running rox raises its window (or comes back out of the tray)
//! and takes the files. `rox --new-instance` skips the guard when a second
//! process is what you actually want.
//!
//! Windows has no backend here yet, so a second launch there starts its own
//! process the way it always did.

use std::path::PathBuf;

use gpui::App;
use rox_library::open_files::LaunchMode;
use serde::{Deserialize, Serialize};

/// What a second launch hands over. The files are already filtered to audio
/// rox can decode and made absolute: the running instance has its own working
/// directory and can't resolve a relative path the way the caller meant it.
/// The mode travels as a bool because [`LaunchMode`] isn't a serde type and
/// there are only the two.
#[derive(Serialize, Deserialize)]
struct Launch {
    enqueue: bool,
    files: Vec<PathBuf>,
}

/// The listening end of the guard, carried from [`claim`] (before the app
/// boots) to [`serve`] (once there's a `cx` to drain onto). Empty when this
/// run has no guard: `--new-instance`, a platform without a backend, or a
/// bind that didn't take.
pub struct Server {
    #[cfg(unix)]
    listener: Option<std::os::unix::net::UnixListener>,
    /// The inode the socket path pointed at when we bound it, so quit can
    /// tell our socket from one a racing launch put there since.
    #[cfg(unix)]
    inode: Option<u64>,
}

/// Whether this process is the rox for its data directory. `Some` means run
/// the app and hand the server to [`serve`]; `None` means a running rox took
/// this launch and there is nothing left for this process to do.
#[cfg(unix)]
pub fn claim(mode: LaunchMode, files: &[PathBuf]) -> Option<Server> {
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::os::unix::net::{UnixListener, UnixStream};

    // No guard this run: no socket to bind, and nothing for quit to remove.
    let unguarded = || {
        Some(Server {
            listener: None,
            inode: None,
        })
    };
    if std::env::args().any(|arg| arg == "--new-instance") {
        return unguarded();
    }
    let path = socket_path();
    let launch = Launch {
        enqueue: mode == LaunchMode::Enqueue,
        files: files
            .iter()
            .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
            .collect(),
    };
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let payload = serde_json::to_vec(&launch).unwrap_or_default();
        // The write is the whole handoff; closing our end is the EOF the
        // other side reads on.
        if stream.write_all(&payload).is_ok() && stream.flush().is_ok() {
            return None;
        }
    }
    // Nobody answered. Either no rox is running or one died without taking
    // its socket file with it - a live listener would have accepted the
    // connect above, so what's left is safe to replace.
    //
    // Bind under our own name and rename it into place rather than unlinking
    // the path first: two cold launches in the same instant can't then delete
    // each other's freshly bound socket, the path just ends up pointing at
    // whichever of them renamed last.
    let staging = path.with_extension(format!("{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&staging);
    let Ok(listener) = UnixListener::bind(&staging) else {
        // A runtime dir we can't write. Not worth refusing to start over:
        // run without the guard.
        return unguarded();
    };
    // The runtime dir is user-private already; the data dir standing in for
    // it isn't guaranteed to be, and this socket carries the paths of files
    // we're about to play.
    let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600));
    if std::fs::rename(&staging, &path).is_err() {
        let _ = std::fs::remove_file(&staging);
        return unguarded();
    }
    Some(Server {
        listener: Some(listener),
        inode: std::fs::metadata(&path).ok().map(|meta| meta.ino()),
    })
}

#[cfg(not(unix))]
pub fn claim(_mode: LaunchMode, _files: &[PathBuf]) -> Option<Server> {
    Some(Server {})
}

/// Take over the socket: an accept thread parses each handoff and a drain on
/// the foreground executor applies it, the same marshalling the tray and the
/// media keys use to get off their own threads.
#[cfg(unix)]
pub fn serve(server: Server, cx: &mut App) {
    use std::io::Read as _;
    use std::os::unix::fs::MetadataExt as _;

    let Some(listener) = server.listener else {
        return;
    };
    let inode = server.inode;
    let (tx, launches) = async_channel::unbounded();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // A peer that connects and then says nothing can't hold the
            // thread; the handoff is one write and a close.
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let mut payload = Vec::new();
            if stream.read_to_end(&mut payload).is_err() {
                continue;
            }
            match serde_json::from_slice::<Launch>(&payload) {
                Ok(launch) => {
                    if tx.send_blocking(launch).is_err() {
                        break;
                    }
                }
                Err(err) => log::warn!("single instance: unreadable handoff: {err}"),
            }
        }
    });
    cx.spawn(async move |cx| {
        while let Ok(launch) = launches.recv().await {
            if cx.update(|cx| adopt(launch, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    cx.on_app_quit(move |_| {
        let path = socket_path();
        async move {
            // Only clear the socket if it's still the one we bound. A launch
            // that raced us at startup may have renamed its own over the
            // path, and taking that with us would leave a running rox nobody
            // can reach, with every later launch starting another process.
            let live = std::fs::metadata(&path).ok().map(|meta| meta.ino());
            if inode.is_some() && live == inode {
                let _ = std::fs::remove_file(path);
            }
        }
    })
    .detach();
}

#[cfg(not(unix))]
pub fn serve(_server: Server, _cx: &mut App) {}

/// A second launch, applied to this one. The window comes back first - out of
/// the tray when residency swallowed it, raised when it's only buried - then
/// the files ride into whatever is now front.
#[cfg(unix)]
fn adopt(launch: Launch, cx: &mut App) {
    let mode = if launch.enqueue {
        LaunchMode::Enqueue
    } else {
        LaunchMode::Play
    };
    // Filtered again on this side: what arrives is only as trustworthy as the
    // socket, and re-running the resolve costs nothing.
    let paths = rox_library::open_files::resolve_audio_paths(launch.files);
    match crate::workspace::front_workspace(cx) {
        // Best effort on Wayland: raising takes an activation token the
        // compositor can refuse, and the launcher's token died with the
        // process that handed us the files.
        Some((window, _)) => {
            window
                .update(cx, |_, window, _| window.activate_window())
                .ok();
        }
        None => crate::integrations::tray::reopen(cx),
    }
    if let Some((_, state)) = crate::workspace::front_workspace(cx) {
        crate::workspace::play_launch_paths(&state, mode, paths, cx);
    }
}

/// Where the instance listens. Keyed to the data directory, so a `--portable`
/// or `--fresh` run is its own instance instead of talking to the daily
/// driver's. The hash only has to agree with itself across two runs of the
/// same binary, which is well inside what `DefaultHasher` promises. Sockets
/// belong in the runtime dir and the path has a length limit, so that comes
/// first and the data dir only stands in where there is no runtime dir.
#[cfg(unix)]
fn socket_path() -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    crate::settings::data_dir().hash(&mut hasher);
    let dir = dirs::runtime_dir().unwrap_or_else(crate::settings::data_dir);
    dir.join(format!("rox-{:016x}.sock", hasher.finish()))
}
