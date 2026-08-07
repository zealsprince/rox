//! The few acoustic-analysis constants the settings file is written in terms
//! of. The extractors themselves, the model catalog, and the download that
//! installs one live in `rox-acoustic`, which depends on this crate and so
//! reads them from here rather than the other way around.

/// What produced the vectors, and the name they're stored under. Change the
/// built-in extractor's features and this changes with them: the old vectors
/// stay readable under the old name, and nothing compares across the two.
pub const MODEL: &str = "dsp-timbre-1";

/// PANNs CNN10's name, the model-based extractor's catalog id. The built-in
/// one's is [`MODEL`], which stays what it is: it names the vectors already
/// in people's libraries.
pub const PANNS_CNN10: &str = "panns-cnn10";

/// The default for [`crate::settings::Settings::acoustic_workers`]: enough
/// to make a dent, few enough that the machine stays usable while a library
/// analyzes in the background. The setting exists because that trade is the
/// user's to make: on a machine with cores to spare, more workers is the
/// difference between a pass measured in days and one measured in hours.
pub const DEFAULT_WORKERS: usize = 4;
