//! Audio analysis behind the app's audio views. The app drains the engine's
//! PCM tap into an [`AudioFeed`], which hands out a shared spectrum per window
//! size; [`signal`] turns that spectrum into modulation sources panels bind
//! to. This crate is the DSP and the serde types; rendering lives in the app.

pub mod analysis;
pub mod curve;
pub mod feed;
pub mod signal;

pub use feed::AudioFeed;
