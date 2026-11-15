//! One rox per data directory.
//!
//! Launching rox while a rox is already running (a click on the pinned
//! launcher, a file manager's Open With, a second `rox` in a terminal) used
//! to start a whole second process, with its own window, its own tray icon,
//! and its own claim on the media keys. With quit-to-tray on that reads as
//! the tray opening a duplicate window: the resident instance is still there
//! playing, and a stranger shows up next to it.
//!
//! So the first instance binds a Unix socket keyed to its data directory, and
//! every later launch connects, hands over what it was asked to open, and
//! exits. The running rox raises its window (or comes back out of the tray)
//! and takes the files. `rox --new-instance` skips the guard when you
//! actually want a second process.
//!
//! Windows gets the same handoff over a named pipe, through the guard half
//! of `rox_ipc::instance`. The pipe needs none of the socket's bind
//! discipline: it isn't a file, so a crash leaves nothing stale behind, and
//! the first process to create it owns it outright, so two launches in the
//! same instant can't both come up as the owner. Everything past the
//! transport (the payload, the adopt) is shared with Unix.

use std::path::{Path, PathBuf};

use gpui::App;
use rox_library::open_files::{Document, LaunchArgs, LaunchMode};
use serde::{Deserialize, Serialize};

/// What a second launch hands over. The files are already filtered to audio
/// rox can decode and made absolute: the running instance has its own working
/// directory and can't resolve a relative path the way the caller meant it.
/// The mode is sent as a bool because [`LaunchMode`] isn't a serde type and
/// there are only the two.
#[derive(Serialize, Deserialize)]
struct Launch {
    enqueue: bool,
    files: Vec<PathBuf>,
    /// Document paths made absolute, links as written. Defaulted so a 1.x
    /// launch, which never sends it, still reads.
    #[serde(default)]
    documents: Vec<String>,
}

impl Launch {
    fn new(args: &LaunchArgs) -> Launch {
        let documents = args
            .documents
            .iter()
            .filter_map(|document| match document {
                // A link is never a path: `absolute` on Windows would join it
                // onto the working directory.
                Document::Link(link) => Some(link.clone()),
                Document::Workspace(path) | Document::Plugin(path) | Document::Object(path) => {
                    absolute(path).to_str().map(str::to_owned)
                }
            })
            .collect();

        Launch {
            enqueue: args.mode == LaunchMode::Enqueue,
            files: args.files.iter().map(|p| absolute(p)).collect(),
            documents,
        }
    }
}

/// A launch path made absolute for the running instance. Unix resolves it
/// through the filesystem. Windows only joins it onto the working
/// directory, because canonicalize there hands back the `\\?\` verbatim
/// form, which would reach the queue as a different path from the one the
/// library holds for the same file.
#[cfg(unix)]
fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(windows)]
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The listening end of the guard, passed from [`claim`] (before the app
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
    #[cfg(windows)]
    listener: Option<rox_ipc::instance::Listener>,
    /// Why this run has no guard when it wanted one. [`claim`] runs before
    /// the logger does, so the reason waits here for [`serve`] to report.
    #[cfg(windows)]
    unguarded: Option<String>,
}

/// Whether this process is the rox for its data directory. `Some` means run
/// the app and hand the server to [`serve`]; `None` means a running rox took
/// this launch and there's nothing left for this process to do.
#[cfg(unix)]
pub fn claim(args: &LaunchArgs) -> Option<Server> {
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
    if rox_core::settings::launch_flag("--new-instance") {
        return unguarded();
    }
    let path = socket_path();
    let launch = Launch::new(args);
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let payload = serde_json::to_vec(&launch).unwrap_or_default();
        // The write is the whole handoff; closing our end is the EOF the
        // other side reads on.
        if stream.write_all(&payload).is_ok() && stream.flush().is_ok() {
            return None;
        }
    }
    // Nobody answered. Either no rox is running or one died without taking
    // its socket file with it. A live listener would have accepted the
    // connect above, so what's left is safe to replace.
    //
    // Bind under our own name and rename it into place rather than unlinking
    // the path first: two cold launches in the same instant can't then delete
    // each other's freshly bound socket, the path just ends up pointing at
    // whichever of them renamed last.
    rox_ipc::ensure_socket_dir(&path);
    let staging = path.with_extension(format!("{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&staging);
    let Ok(listener) = UnixListener::bind(&staging) else {
        // A runtime dir we can't write. Not worth refusing to start over:
        // run without the guard.
        return unguarded();
    };
    // The runtime dir is user-private already; the data dir standing in for
    // it isn't guaranteed to be, and this socket sends the paths of files
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

#[cfg(windows)]
pub fn claim(args: &LaunchArgs) -> Option<Server> {
    use rox_ipc::instance::Claim;

    let unguarded = |reason: Option<String>| {
        Some(Server {
            listener: None,
            unguarded: reason,
        })
    };
    if rox_core::settings::launch_flag("--new-instance") {
        return unguarded(None);
    }

    let payload = serde_json::to_vec(&Launch::new(args)).unwrap_or_default();
    match rox_ipc::instance::claim(&rox_core::settings::data_dir(), &payload) {
        Claim::Owner(listener) => Some(Server {
            listener: Some(listener),
            unguarded: None,
        }),
        Claim::HandedOff => None,
        // Same call as a Unix bind that didn't take: a second process is
        // better than refusing to start.
        Claim::Unguarded(reason) => unguarded(Some(reason)),
    }
}

#[cfg(not(any(unix, windows)))]
pub fn claim(_args: &LaunchArgs) -> Option<Server> {
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
            // A peer that connects and then sends nothing can't hold the
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

/// The pipe's twin of the Unix serve. The guard's own threads hand over raw
/// bytes, so parsing moves onto the drain, and there's no quit hook: the
/// pipe closes with the process and leaves nothing to clean.
#[cfg(windows)]
pub fn serve(server: Server, cx: &mut App) {
    if let Some(reason) = server.unguarded {
        log::warn!("single instance: running without the guard: {reason}");
    }
    let Some(listener) = server.listener else {
        return;
    };

    let handoffs = listener.spawn();
    cx.spawn(async move |cx| {
        while let Ok(payload) = handoffs.recv().await {
            let launch = match serde_json::from_slice::<Launch>(&payload) {
                Ok(launch) => launch,
                Err(err) => {
                    log::warn!("single instance: unreadable handoff: {err}");
                    continue;
                }
            };
            if cx.update(|cx| adopt(launch, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(not(any(unix, windows)))]
pub fn serve(_server: Server, _cx: &mut App) {}

/// A second launch, applied to this one. Split and filtered again on this
/// side: what arrives is only as trustworthy as the socket, and re-running
/// the resolve costs nothing.
#[cfg(any(unix, windows))]
fn adopt(launch: Launch, cx: &mut App) {
    let mode = if launch.enqueue {
        LaunchMode::Enqueue
    } else {
        LaunchMode::Play
    };
    let (documents, _) = rox_library::open_files::split_documents(launch.documents);
    let paths = rox_library::open_files::resolve_audio_paths(launch.files);

    receive(mode, paths, documents, cx);
}

/// Files and documents from outside this process, opened in it. The window
/// comes back first, out of the tray when residency swallowed it or raised
/// when it's only buried, then the documents go to their routes and the
/// files to whatever workspace is now front.
#[cfg(any(unix, windows))]
pub(crate) fn receive(
    mode: LaunchMode,
    paths: Vec<PathBuf>,
    documents: Vec<Document>,
    cx: &mut App,
) {
    raise(cx);
    crate::documents::open(documents, cx);
    if let Some((_, state)) = rox_panel_api::windows::front_workspace(cx) {
        crate::workspace::play_launch_paths(&state, mode, paths, cx);
    }
}

#[cfg(any(unix, windows))]
fn raise(cx: &mut App) {
    match rox_panel_api::windows::front_workspace(cx) {
        // Best effort on Wayland: raising takes an activation token the
        // compositor can reject, and the launcher's token died with the
        // process that handed us the files.
        Some((window, _)) => {
            window
                .update(cx, |_, window, _| window.activate_window())
                .ok();
        }
        None => crate::integrations::tray::reopen(cx),
    }
}

/// Where the instance listens. Keyed to the data directory, so a `--portable`
/// or `--fresh` run is its own instance instead of talking to the daily
/// driver's. The hash only has to agree with itself across two runs of the
/// same binary, which is well inside what `DefaultHasher` guarantees. The
/// folder is picked the same way as the control socket's.
#[cfg(unix)]
fn socket_path() -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};

    let data_dir = rox_core::settings::data_dir();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data_dir.hash(&mut hasher);

    rox_ipc::socket_in(&data_dir, &format!("rox-{:016x}.sock", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_without_documents_still_reads() {
        let payload = br#"{"enqueue": true, "files": ["/m/one.flac"]}"#;

        let launch: Launch = serde_json::from_slice(payload).unwrap();

        assert!(launch.enqueue);
        assert_eq!(launch.files, [PathBuf::from("/m/one.flac")]);
        assert!(launch.documents.is_empty());
    }

    #[test]
    fn a_link_crosses_as_written() {
        let args = LaunchArgs {
            mode: LaunchMode::Play,
            files: Vec::new(),
            documents: vec![Document::Link("rox://install/x".into())],
        };

        assert_eq!(Launch::new(&args).documents, ["rox://install/x"]);
    }
}
