//! The concrete panels the workspace hosts, each a view over the shared
//! entities in [`crate::panel::AppState`]. The panel framework itself, per
//! ADR 7, lives in [`crate::panel`]; this module just gathers the panels.

pub mod art;
pub mod artist_grid;
pub mod biography;
pub mod cover;
pub mod drag_anchor;
pub mod drawer;
pub mod favourite;
pub mod filter;
pub mod folder_tree;
pub mod grid;
pub mod group;
pub mod history;
pub mod library;
pub mod lyrics;
pub mod menu;
pub mod metadata;
pub mod mini;
pub mod overlay;
pub mod particles;
pub mod playlists;
pub mod queue;
pub mod queue_widget;
pub mod rating;
pub mod search;
pub mod slide;
pub mod spacer;
pub mod spectrum;
pub mod theme_toggle;
pub mod transport;
pub mod vu;
pub mod waveform;
pub mod window_controls;
