//! The concrete panels the workspace hosts, each a view over the shared
//! entities in [`crate::panel::AppState`]. The panel framework itself, per
//! ADR 7, lives in [`crate::panel`]; this module just gathers the panels.

pub mod cover;
pub mod grid;
pub mod history;
pub mod library;
pub mod metadata;
pub mod spectrum;
pub mod transport;
pub mod waveform;
