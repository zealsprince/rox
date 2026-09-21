//! The playback engine per the components contract: Symphonia decode on a
//! worker thread, a pre-allocated SPSC ring, a cpal callback that never
//! allocates or locks, gapless decoder swap at track boundaries, and a lossy
//! PCM tap for the visualizer. Grown out of the playback spike, which drove
//! this same engine from stdin in rox-prototype-playback (git history, commit
//! bd22dc1).

pub mod analysis;
pub mod broadcast;
pub mod chain;
pub mod codecs;
pub mod continuation;
pub mod engine;
pub mod eq;
pub mod fingerprint;
pub mod gain;
pub mod http;
pub mod icy;
pub mod latency;
pub mod memory;
pub mod opus;
pub mod output;
pub mod resample;
pub mod shared;
pub mod tape;

// A station's now-playing is a playback type, not an HTTP one: whoever reads
// it off the shared state is showing a track, not parsing a stream. Named here
// so nobody has to reach into the transport's internals for it.
pub use icy::IcyTitle;

// What a station said about itself at the open, for the same reason: the
// surfaces reading it are drawing a station, not parsing a response.
pub use http::StationInfo;

// Where a stream stands. Read by anything that draws a station, so it sits at
// the root beside the other two rather than inside the shared state module.
pub use shared::StreamState;

// How far a live stream is being played behind its own broadcast, for the
// same reason: the transport drawing it is drawing a station, not reading
// engine state.
pub use shared::Shift;

// And the song boundaries inside one, for the same surfaces: where a
// station's songs turned over is the only structure its timeline has.
pub use shared::LiveMark;

// The breaks in one, which the same strips draw beside those boundaries:
// where a reconnect spliced two connections and nothing plays across.
pub use shared::LiveGap;

// And how close to the live edge still counts as standing on it. A caller
// stepping through a buffer needs it for the same reason the tape does:
// under this distance there's no spot for a cursor to move to.
pub use tape::LIVE_EDGE_SNAP_SECS;

// Embedders hold the output stream and the tap consumer, so the types those
// come in need to be nameable without taking on the deps directly.
pub use cpal;
pub use rtrb;
