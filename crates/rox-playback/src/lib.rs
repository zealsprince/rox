//! The playback engine per the components contract: Symphonia decode on a
//! worker thread, a pre-allocated SPSC ring, a cpal callback that never
//! allocates or locks, gapless decoder swap at track boundaries, and a lossy
//! PCM tap for the visualizer. Grown out of the playback spike, which still
//! drives this same engine from stdin in rox-prototype-playback.

pub mod engine;
pub mod output;
pub mod resample;
pub mod shared;

// Embedders hold the output stream and the tap consumer, so the types those
// come in need to be nameable without taking on the deps directly.
pub use cpal;
pub use rtrb;
