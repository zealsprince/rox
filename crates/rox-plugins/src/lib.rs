//! The plugin host (ADR 30): a plugin is a folder with a `plugin.json`, or a
//! `.roxp` package of one (ADR 32), run as a subprocess speaking JSON-RPC
//! over its stdin and stdout. This crate loads the manifest, hashes and
//! unpacks packages, spawns and supervises the process, and reads its
//! streams. It knows nothing of the library, the player or the
//! UI; `rox-services` turns a host into an opener and a sync.
//!
//! A plugin runs with the user's permissions. What the host guarantees is
//! narrower: it only runs what the user switched on, it never trusts a byte
//! the plugin sends, and nothing it starts outlives it.

pub mod env;
pub mod hash;
pub mod host;
pub mod loader;
pub mod manifest;
pub mod package;
pub mod process;
pub mod search;
pub mod stream;
pub mod wire;

pub use host::{Host, HostConfig, Status, Timeouts};
pub use loader::{Loaded, Origin};
pub use manifest::Manifest;
pub use stream::{Options, Stats, Stream};
