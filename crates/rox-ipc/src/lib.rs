//! The control socket (ADR 22): rox's one machine interface, newline-delimited
//! JSON-RPC over a Unix domain socket. Everything external goes through this
//! surface, with MPRIS staying the standard desktop shim in front of it.
//!
//! This crate owns the wire: the frame and error types, the listener with the
//! same staging-and-rename bind discipline as the single-instance guard, the
//! per-connection threads that parse frames and hold the version handshake,
//! the [`Events`] registry that pushes id-less frames to connections that
//! subscribed, and a small blocking client for the CLI and tests. What it
//! never owns is an answer: every method past `hello` and `subscribe` crosses
//! to the app as a [`Request`] on an async channel and comes back through the
//! request's responder, so the app side stays the single place that touches
//! the player and the library. Events flow the other way through the same
//! division: the app decides what happened and emits, the crate carries it.
//!
//! Unix speaks std's domain sockets; Windows speaks named pipes through
//! interprocess, behind the same frame discipline and the same generic
//! connection loop, so the two backends can't drift on the protocol.
//!
//! The Windows half of the single-instance guard lives here too, in
//! [`instance`]. It has nothing to do with the protocol, but it's the same
//! transport and the same pipe naming, and this is the crate that can be
//! cross-checked for Windows without dragging the app's native build along.

mod events;
mod protocol;
mod server;

pub mod client;
#[cfg(windows)]
pub mod instance;

pub use events::Events;
pub use protocol::{PROTOCOL_VERSION, RpcError};
pub use server::{Cleanup, Request, Responder, Server};

use std::path::{Path, PathBuf};

/// Where the control socket lives for a data directory. Keyed to the data
/// dir the same way the single-instance guard's socket is, so a `--portable`
/// or `--fresh` run gets its own control surface instead of steering the
/// daily driver. Sockets belong in the runtime dir; the data dir stands in
/// only where there is none.
pub fn socket_path(data_dir: &Path) -> PathBuf {
    let hash = data_dir_hash(data_dir);
    if cfg!(windows) {
        // Named pipes are in their own flat namespace, not the
        // filesystem; this is the canonical spelling of ours, and the
        // backends peel the prefix back off to name the pipe.
        return PathBuf::from(format!(r"\\.\pipe\rox-ipc-{hash:016x}"));
    }
    let dir = dirs::runtime_dir().unwrap_or_else(|| data_dir.to_path_buf());
    dir.join(format!("rox-ipc-{hash:016x}.sock"))
}

/// The key every per-data-dir name hangs off: the control socket here, the
/// single-instance pipe in [`instance`]. It only has to agree with itself
/// across two runs of the same binary, which is well inside what
/// `DefaultHasher` guarantees.
fn data_dir_hash(data_dir: &Path) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data_dir.hash(&mut hasher);
    hasher.finish()
}

/// A socket path as interprocess requires it: the bare pipe name in the
/// named-pipe namespace. Accepts the canonical `\\.\pipe\` spelling from
/// [`socket_path`] and a bare name alike, so a hand-typed `--socket` works
/// either way.
#[cfg(windows)]
pub(crate) fn pipe_name(path: &Path) -> std::io::Result<interprocess::local_socket::Name<'static>> {
    use interprocess::local_socket::{GenericNamespaced, ToNsName as _};

    let text = path.to_string_lossy();
    let bare = text.strip_prefix(r"\\.\pipe\").unwrap_or(&text).to_owned();
    bare.to_ns_name::<GenericNamespaced>()
}

/// Whether a pipe create failed because the name is already served. The
/// listener's first instance carries `FILE_FLAG_FIRST_PIPE_INSTANCE`, and
/// Windows answers a second process asking for the same name with
/// `ERROR_ACCESS_DENIED`, which std reads as a permission problem rather
/// than an address in use. `ERROR_PIPE_BUSY` is what a pipe that someone
/// created with an instance limit answers instead.
#[cfg(windows)]
pub(crate) fn pipe_taken(err: &std::io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_PIPE_BUSY: i32 = 231;

    err.kind() == std::io::ErrorKind::AddrInUse
        || matches!(
            err.raw_os_error(),
            Some(ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY)
        )
}
