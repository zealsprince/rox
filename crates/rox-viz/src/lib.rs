//! Audio analysis behind the app's audio views. The app drains the playback
//! engine's PCM tap into an [`AudioFeed`]; the spectrum panel asks it for the
//! newest window's spectrum ([`AudioFeed::magnitudes`], one
//! [`analysis::Analyzer`] per window size shared by every view) and pools the
//! magnitudes into bars. The
//! [`signal`] module turns the same spectrum into modulation sources a
//! panel can bind its parameters to. Rendering is with the panels in
//! the app crate; this crate is the DSP, plus serde so the binding configs
//! panels persist can be defined here too.

pub mod analysis;
pub mod curve;
pub mod feed;
pub mod signal;

pub use feed::AudioFeed;
