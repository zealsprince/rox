//! The concrete panels the workspace hosts, each a view over the shared
//! entities in [`rox_panel_api::panel::AppState`]. The panel framework (ADR
//! 7) lives in rox-panel-api.
//!
//! Nothing here depends on the binary: a panel opens windows through the
//! openers table in [`rox_panel_api::openers`], which the app fills in at
//! startup.

pub mod art;
pub mod artist_grid;
pub mod biography;
pub mod bookmarks;
pub mod cover;
pub mod discs;
pub mod drag_anchor;
pub mod eq_widget;
pub mod filter;
pub mod folder_tree;
pub mod genre_grid;
pub mod grid;
pub mod health_widget;
pub mod history;
pub mod library;
pub mod lyrics;
pub mod metadata;
pub mod milkdrop;
pub mod oscilloscope;
pub mod output;
pub mod particles;
pub mod playlists;
pub mod queue;
pub mod search;
pub mod shader;
pub mod spacer;
pub mod spectrogram;
pub mod spectrum;
pub mod stations;
pub mod stats_widget;
pub mod status;
pub mod theme_toggle;
pub mod transport;
pub mod vu;
pub mod waveform;

mod settings;

pub(crate) use rox_design as design;
pub(crate) use rox_design::assets;
pub(crate) use rox_net::providers;
pub(crate) use rox_panel_api::{
    bookmark_ui, group_head, panel, panel_settings, query, rating_ui, signal_ui, source, track_ui,
};
pub(crate) use rox_playback::continuation;
// Not `history`: that name is the panel here, so the listen recorder stays
// at rox_services::history.
pub(crate) use rox_services::{artists, catalog, peaks, player, selection, thumbs};
