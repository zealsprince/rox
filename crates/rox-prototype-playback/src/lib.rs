//! Playback spike for the implementation doc 01-playback: Symphonia decode on
//! a worker thread, a pre-allocated SPSC ring, a cpal callback that never
//! allocates or locks, gapless decoder swap at track boundaries, and a lossy
//! PCM tap standing in for the visualizer.
//!
//! The crate's binary drives it from stdin; the rox app embeds the same
//! engine behind a GPUI window.

pub mod engine;
pub mod output;
pub mod resample;
pub mod shared;

// Embedders hold the output stream and the tap consumer, so the types those
// come in need to be nameable without taking on the deps directly.
pub use cpal;
pub use rtrb;
