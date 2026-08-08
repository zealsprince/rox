//! The settings window itself moved to rox-panel-api, where it's generic
//! over [`PanelSettings`] and knows no concrete panel. What stays here is
//! the part that can't: the downcast table naming every settings-capable
//! panel type, and the two type-erased routes in (the layout tree's gear
//! and lock) that walk it.

use std::sync::Arc;

use gpui::App;
use rox_dock::PanelView;
use rox_panel_api::panel::PanelSettings;

pub use rox_panel_api::panel_settings::*;

use crate::panels::art::ArtPanel;
use crate::panels::artist_grid::ArtistGridPanel;
use crate::panels::biography::BiographyPanel;
use crate::panels::cover::CoverArtPanel;
use crate::panels::drag_anchor::DragAnchorPanel;
use crate::panels::drawer::DrawerPanel;
use crate::panels::eq_widget::EqWidgetPanel;
use crate::panels::favourite::FavouritePanel;
use crate::panels::filter::FilterPanel;
use crate::panels::folder_tree::FolderTreePanel;
use crate::panels::genre_grid::GenreGridPanel;
use crate::panels::grid::GridPanel;
use crate::panels::group::GroupPanel;
use crate::panels::history::HistoryPanel;
use crate::panels::library::LibraryPanel;
use crate::panels::lyrics::LyricsPanel;
use crate::panels::menu::MenuPanel;
use crate::panels::metadata::MetadataPanel;
use crate::panels::mini::MiniTogglePanel;
use crate::panels::output::OutputPanel;
use crate::panels::overlay::OverlayPanel;
use crate::panels::particles::ParticlesPanel;
use crate::panels::playlists::PlaylistsPanel;
use crate::panels::queue::QueuePanel;
use crate::panels::queue_widget::QueueWidgetPanel;
use crate::panels::rating::RatingPanel;
use crate::panels::search::SearchPanel;
use crate::panels::shader::ShaderPanel;
use crate::panels::slide::SlidePanel;
use crate::panels::spacer::SpacerPanel;
use crate::panels::spectrum::SpectrumPanel;
use crate::panels::stats_widget::StatsWidgetPanel;
use crate::panels::status::StatusPanel;
use crate::panels::theme_toggle::ThemeTogglePanel;
use crate::panels::transport::{SeekStripPanel, TrackInfoPanel, TransportPanel, VolumePanel};
use crate::panels::vu::VuPanel;
use crate::panels::waveform::WaveformPanel;
use crate::panels::window_controls::WindowControlsPanel;

/// Dispatch a type-erased panel view to its concrete settings-capable
/// type: try each downcast until one lands and run the body with the
/// typed entity bound. The type list mirrors the workspace's panel
/// registry; a type missing here just no-ops on the type-erased routes
/// (the layout tree's gear and lock).
macro_rules! with_settings_panel {
    ($view:expr, |$panel:ident| $body:expr) => {
        with_settings_panel!(
            @try $view, $panel, $body,
            LibraryPanel,
            SearchPanel,
            FilterPanel,
            GridPanel,
            ArtistGridPanel,
            GenreGridPanel,
            ArtPanel,
            PlaylistsPanel,
            QueuePanel,
            HistoryPanel,
            CoverArtPanel,
            MetadataPanel,
            LyricsPanel,
            BiographyPanel,
            TrackInfoPanel,
            TransportPanel,
            SeekStripPanel,
            VolumePanel,
            QueueWidgetPanel,
            EqWidgetPanel,
            StatsWidgetPanel,
            OutputPanel,
            RatingPanel,
            FavouritePanel,
            SpectrumPanel,
            WaveformPanel,
            ParticlesPanel,
            ShaderPanel,
            StatusPanel,
            MenuPanel,
            DragAnchorPanel,
            WindowControlsPanel,
            GroupPanel,
            OverlayPanel,
            DrawerPanel,
            SlidePanel,
            FolderTreePanel,
            MiniTogglePanel,
            ThemeTogglePanel,
            SpacerPanel,
            VuPanel,
        )
    };
    (@try $view:expr, $panel:ident, $body:expr, $($ty:ty),+ $(,)?) => {
        $(
            if let Ok($panel) = $view.view().downcast::<$ty>() {
                $body;
                return;
            }
        )+
    };
}

/// Open the settings window for a type-erased panel, the settings
/// window's layout tree route in.
pub fn open_for_view(panel: &Arc<dyn PanelView>, cx: &mut App) {
    with_settings_panel!(panel, |panel| open(panel, cx));
}

/// Flip a type-erased panel's placement lock, the layout tree's lock
/// toggle. The dock reads the flag through `Panel::locked` on its next
/// paint, so the flip settles on its own.
pub fn toggle_locked_for_view(panel: &Arc<dyn PanelView>, cx: &mut App) {
    with_settings_panel!(panel, |panel| panel.update(cx, |panel, cx| {
        let on = !panel.chrome().locked;
        panel.set_locked(on, cx);
    }));
}
