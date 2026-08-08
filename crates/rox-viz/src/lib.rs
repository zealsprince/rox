//! Audio analysis behind the app's audio views. The app drains the playback
//! engine's PCM tap into an [`AudioFeed`]; the spectrum panel reads the
//! newest window back out, runs one FFT per frame through
//! [`analysis::Analyzer`], and pools the magnitudes into bars. The
//! [`signal`] module turns the same spectrum into modulation sources a
//! panel can bind its parameters to. Rendering lives with the panels in
//! the app crate; this crate is the DSP, plus serde so the binding configs
//! panels persist can live here too.

pub mod analysis;
pub mod curve;
pub mod feed;
pub mod signal;

pub use feed::AudioFeed;
