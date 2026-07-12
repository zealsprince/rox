//! Audio analysis behind the app's audio views. The app drains the playback
//! engine's PCM tap into an [`AudioFeed`]; the spectrum panel reads the
//! newest window back out, runs one FFT per frame through
//! [`analysis::Analyzer`], and pools the magnitudes into bars. Rendering
//! lives with the panels in the app crate; this crate is just the DSP and
//! stays dependency-free.

pub mod analysis;
pub mod feed;

pub use feed::AudioFeed;
