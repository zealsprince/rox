//! The playback engine per the components contract: Symphonia decode on a
//! worker thread, a pre-allocated SPSC ring, an output callback that never
//! allocates or locks, gapless decoder swap at track boundaries, and a lossy
//! PCM tap for the visualizer.

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

// Station types surfaces read off the shared state, at the root so nobody
// reaches into the transport or the engine for them.
pub use icy::IcyTitle;

pub use http::StationInfo;

pub use shared::StreamState;

pub use shared::Shift;

pub use shared::LiveMark;

pub use shared::LiveGap;

pub use tape::LIVE_EDGE_SNAP_SECS;

// Embedders hold the output stream and the tap consumer.
pub use cpal;
pub use rtrb;
