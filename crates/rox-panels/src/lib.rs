//! The concrete panels the workspace hosts, each a view over the shared
//! entities in [`rox_panel_api::panel::AppState`]. The panel framework
//! itself, per ADR 7, lives in rox-panel-api; this crate is just the
//! panels.
//!
//! Nothing in here knows the binary. Where a panel opens a window, it goes
//! through the openers table in [`rox_panel_api::openers`], which the app
//! fills in at startup.

pub mod art;
pub mod artist_grid;
pub mod biography;
pub mod cover;
pub mod discs;
pub mod drag_anchor;
pub mod eq_widget;
pub mod favourite;
pub mod filter;
pub mod folder_tree;
pub mod genre_grid;
pub mod grid;
pub mod history;
pub mod library;
pub mod lyrics;
pub mod metadata;
pub mod output;
pub mod particles;
pub mod playlists;
pub mod queue;
pub mod rating;
pub mod search;
pub mod shader;
pub mod spacer;
pub mod spectrum;
pub mod stats_widget;
pub mod status;
pub mod theme_toggle;
pub mod transport;
pub mod vu;
pub mod waveform;

mod settings;

// The panels reach their neighbours at the same module paths they always
// did, so a file that moved down here reads unchanged. Everything below is
// a name this crate borrows from a crate under it.
pub(crate) use rox_design as design;
pub(crate) use rox_design::assets;
pub(crate) use rox_net::providers;
pub(crate) use rox_panel_api::{
    group_head, panel, panel_settings, query, rating_ui, signal_ui, source, track_ui,
};
pub(crate) use rox_playback::continuation;
// The listen history is the one name that clashes: `history` here is the
// panel, so the recorder behind it gets read at its own crate path.
pub(crate) use rox_services::{artists, catalog, peaks, player, selection, thumbs};
