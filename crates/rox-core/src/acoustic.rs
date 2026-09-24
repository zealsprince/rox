//! The acoustic-analysis constants the settings file is written in terms of.
//! The extractors and the model catalog are in `rox-acoustic`, which depends
//! on this crate.

/// The built-in extractor's vector name. Change its features and bump this, so
/// old and new vectors never get compared.
pub const MODEL: &str = "dsp-timbre-1";

pub const PANNS_CNN10: &str = "panns-cnn10";

/// Default worker count for the analysis passes: few enough that the machine
/// stays usable.
pub const DEFAULT_WORKERS: usize = 4;
