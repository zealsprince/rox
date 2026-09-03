//! The settings window itself moved to rox-panel-api, where it's generic
//! over [`PanelSettings`] and references no concrete panel type. This
//! module holds the part that can't be generic: the downcast table naming
//! every settings-capable panel type, and the two type-erased routes in
//! (the layout tree's gear and lock) that use it.

use std::sync::Arc;

use gpui::App;
use rox_dock::PanelView;
use rox_panel_api::panel::PanelSettings;

use rox_panel_api::panel_settings::open;

use crate::panels::drawer::DrawerPanel;
use crate::panels::group::GroupPanel;
use crate::panels::menu::MenuPanel;
use crate::panels::mini::MiniTogglePanel;
use crate::panels::overlay::OverlayPanel;
use crate::panels::queue_widget::QueueWidgetPanel;
use crate::panels::slide::SlidePanel;
use crate::panels::window_controls::WindowControlsPanel;
use rox_panels::art::ArtPanel;
use rox_panels::artist_grid::ArtistGridPanel;
use rox_panels::biography::BiographyPanel;
use rox_panels::cover::CoverArtPanel;
use rox_panels::drag_anchor::DragAnchorPanel;
use rox_panels::eq_widget::EqWidgetPanel;
use rox_panels::filter::FilterPanel;
use rox_panels::folder_tree::FolderTreePanel;
use rox_panels::genre_grid::GenreGridPanel;
use rox_panels::grid::GridPanel;
use rox_panels::health_widget::HealthWidgetPanel;
use rox_panels::history::HistoryPanel;
use rox_panels::library::LibraryPanel;
use rox_panels::lyrics::LyricsPanel;
use rox_panels::metadata::MetadataPanel;
use rox_panels::oscilloscope::OscilloscopePanel;
use rox_panels::output::OutputPanel;
use rox_panels::particles::ParticlesPanel;
use rox_panels::playlists::PlaylistsPanel;
use rox_panels::queue::QueuePanel;
use rox_panels::search::SearchPanel;
use rox_panels::shader::ShaderPanel;
use rox_panels::spacer::SpacerPanel;
use rox_panels::spectrogram::SpectrogramPanel;
use rox_panels::spectrum::SpectrumPanel;
use rox_panels::stats_widget::StatsWidgetPanel;
use rox_panels::status::StatusPanel;
use rox_panels::theme_toggle::ThemeTogglePanel;
use rox_panels::transport::{SeekStripPanel, TrackInfoPanel, TransportPanel, VolumePanel};
use rox_panels::vu::VuPanel;
use rox_panels::waveform::WaveformPanel;

/// Dispatch a type-erased panel view to its concrete settings-capable
/// type: try each downcast until one succeeds and run the body with the
/// typed entity bound. The type list matches the workspace's panel
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
            HealthWidgetPanel,
            OutputPanel,
            SpectrumPanel,
            SpectrogramPanel,
            OscilloscopePanel,
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
