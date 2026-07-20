//! The panel catalog: every openable panel as one entry carrying its menu
//! label, icon, dock placement, and constructor. The menubar's Panels
//! menu, the menu panel, the empty window's launcher, and the tab groups'
//! right-click Add Panel submenu all draw from this table, so adding a
//! panel type is one entry here plus its restore builder in
//! `workspace::register_panels`.

use std::sync::Arc;

use gpui::{App, AppContext as _, WeakEntity, Window};
use rox_dock::PanelView;

use crate::assets::icons;
use crate::panel::AppState;
use crate::panels::art::{ArtConfig, ArtPanel};
use crate::panels::biography::{BiographyConfig, BiographyPanel};
use crate::panels::cover::{CoverArtPanel, CoverConfig};
use crate::panels::depth::{DepthConfig, DepthPanel};
use crate::panels::drag_anchor::{DragAnchorConfig, DragAnchorPanel};
use crate::panels::filter::{FilterConfig, FilterPanel};
use crate::panels::grid::{GridConfig, GridPanel};
use crate::panels::group::{GroupConfig, GroupPanel};
use crate::panels::history::{HistoryConfig, HistoryPanel};
use crate::panels::library::{LibraryConfig, LibraryPanel};
use crate::panels::lyrics::{LyricsConfig, LyricsPanel};
use crate::panels::menu::{MenuConfig, MenuPanel};
use crate::panels::metadata::{MetadataConfig, MetadataPanel};
use crate::panels::mini::{MiniToggleConfig, MiniTogglePanel};
use crate::panels::playlists::{PlaylistsConfig, PlaylistsPanel};
use crate::panels::queue::{QueueConfig, QueuePanel};
use crate::panels::queue_widget::{QueueWidgetConfig, QueueWidgetPanel};
use crate::panels::search::{SearchConfig, SearchPanel};
use crate::panels::slide::{SlideConfig, SlidePanel};
use crate::panels::spectrum::{SpectrumConfig, SpectrumPanel};
use crate::panels::transport::{
    SeekConfig, SeekStripPanel, TrackInfoConfig, TrackInfoPanel, TransportConfig, TransportPanel,
    VolumeConfig, VolumePanel,
};
use crate::panels::waveform::{WaveformConfig, WaveformPanel};
use crate::panels::window_controls::{WindowControlsConfig, WindowControlsPanel};
use crate::workspace::Workspace;

/// Where a fresh panel of this kind joins the layout: the center tab
/// group, the transport row along the bottom, or a thin strip across the
/// top (the search bar).
#[derive(Clone, Copy)]
pub(crate) enum PanelPlacement {
    Center,
    Bottom,
    Top,
}

/// One openable panel: what the menus show for it, where it lands, and
/// how to build one with a default config. The workspace handle is for
/// the panels that drive the workspace back (menu, window controls);
/// everything else ignores it.
pub(crate) struct PanelDef {
    pub label: &'static str,
    pub icon: &'static str,
    pub placement: PanelPlacement,
    pub build: fn(&AppState, WeakEntity<Workspace>, &mut Window, &mut App) -> Arc<dyn PanelView>,
}

/// A run of catalog entries under one label, rendered as a flyout: every
/// section is a labeled group (Application, Arrangement, Controls,
/// Catalogue, Details, Visualizers). A group with no label renders its rows
/// flat in place, which nothing uses now.
pub(crate) struct PanelSection {
    /// The group's label and icon; None for the bare top-level run.
    pub group: Option<(&'static str, &'static str)>,
    pub panels: &'static [PanelDef],
}

/// The music collection itself: browse, search, filter, and the play
/// queues. The panels reached most often when getting around the library.
pub(crate) static CATALOGUE: PanelSection = PanelSection {
    group: Some(("Catalogue", icons::DISC)),
    panels: &[
        PanelDef {
            label: "Library",
            icon: icons::LIST_MUSIC,
            placement: PanelPlacement::Center,
            build: |state, _, window, cx| {
                Arc::new(cx.new(|cx| {
                    LibraryPanel::new(state.clone(), LibraryConfig::default(), window, cx)
                }))
            },
        },
        PanelDef {
            label: "Search",
            icon: icons::SEARCH,
            placement: PanelPlacement::Top,
            build: |state, _, window, cx| {
                Arc::new(
                    cx.new(|cx| {
                        SearchPanel::new(state.clone(), SearchConfig::default(), window, cx)
                    }),
                )
            },
        },
        PanelDef {
            label: "Filter",
            icon: icons::FUNNEL,
            placement: PanelPlacement::Center,
            build: |state, _, window, cx| {
                Arc::new(cx.new(|cx| {
                    FilterPanel::new(state.clone(), FilterConfig::default(), window, cx)
                }))
            },
        },
        PanelDef {
            label: "Album Grid",
            icon: icons::LAYOUT_GRID,
            placement: PanelPlacement::Center,
            build: |state, _, window, cx| {
                Arc::new(
                    cx.new(|cx| GridPanel::new(state.clone(), GridConfig::default(), window, cx)),
                )
            },
        },
        PanelDef {
            label: "Album Carousel",
            icon: icons::GALLERY,
            placement: PanelPlacement::Center,
            build: |state, _, window, cx| {
                Arc::new(
                    cx.new(|cx| ArtPanel::new(state.clone(), ArtConfig::default(), window, cx)),
                )
            },
        },
        PanelDef {
            label: "Playlists",
            icon: icons::LIST_MUSIC,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| PlaylistsPanel::new(state.clone(), PlaylistsConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Queue",
            icon: icons::LIST_MUSIC,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| QueuePanel::new(state.clone(), QueueConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "History",
            icon: icons::CLOCK,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| HistoryPanel::new(state.clone(), HistoryConfig::default(), cx)),
                )
            },
        },
    ],
};

/// The inspector views: what's playing or selected, shown from a few
/// angles. Grouped so the Catalogue list stays short.
pub(crate) static DETAILS: PanelSection = PanelSection {
    group: Some(("Details", icons::INFO)),
    panels: &[
        PanelDef {
            label: "Cover Art",
            icon: icons::IMAGE,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| CoverArtPanel::new(state.clone(), CoverConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Metadata",
            icon: icons::FILE_TEXT,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| MetadataPanel::new(state.clone(), MetadataConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Lyrics",
            icon: icons::MIC,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| LyricsPanel::new(state.clone(), LyricsConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Biography",
            icon: icons::USER,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| BiographyPanel::new(state.clone(), BiographyConfig::default(), cx)),
                )
            },
        },
    ],
};

/// The composition hosts: panels that hold other panels inside one dock
/// slot, for the arrangements the dock's splits and tabs can't make.
pub(crate) static ARRANGEMENT: PanelSection = PanelSection {
    group: Some(("Arrangement", icons::LAYOUT_DASHBOARD)),
    panels: &[
        PanelDef {
            label: "Group",
            icon: icons::COLUMNS_2,
            placement: PanelPlacement::Center,
            build: |state, ws, _, cx| {
                Arc::new(cx.new(|cx| {
                    GroupPanel::new(state.clone(), ws, GroupConfig::default(), cx)
                }))
            },
        },
        PanelDef {
            label: "Depth",
            icon: icons::LAYERS,
            placement: PanelPlacement::Center,
            build: |state, ws, _, cx| {
                Arc::new(cx.new(|cx| {
                    DepthPanel::new(state.clone(), ws, DepthConfig::default(), cx)
                }))
            },
        },
        PanelDef {
            label: "Slide",
            icon: icons::GALLERY,
            placement: PanelPlacement::Center,
            build: |state, ws, _, cx| {
                Arc::new(cx.new(|cx| {
                    SlidePanel::new(state.clone(), ws, SlideConfig::default(), cx)
                }))
            },
        },
    ],
};

pub(crate) static APPLICATION: PanelSection = PanelSection {
    group: Some(("Application", icons::APP_WINDOW)),
    panels: &[
        PanelDef {
            label: "Menu",
            icon: icons::MENU,
            placement: PanelPlacement::Bottom,
            build: |state, ws, _, cx| {
                Arc::new(cx.new(|cx| MenuPanel::new(state.clone(), ws, MenuConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Drag Anchor",
            icon: icons::MOVE,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| {
                        DragAnchorPanel::new(state.clone(), DragAnchorConfig::default(), cx)
                    }),
                )
            },
        },
        PanelDef {
            label: "Window Controls",
            icon: icons::APP_WINDOW,
            placement: PanelPlacement::Bottom,
            build: |state, ws, _, cx| {
                Arc::new(cx.new(|cx| {
                    WindowControlsPanel::new(state.clone(), ws, WindowControlsConfig::default(), cx)
                }))
            },
        },
        PanelDef {
            label: "Mini Toggle",
            icon: icons::MINIMIZE,
            placement: PanelPlacement::Bottom,
            build: |state, ws, _, cx| {
                Arc::new(cx.new(|cx| {
                    MiniTogglePanel::new(state.clone(), ws, MiniToggleConfig::default(), cx)
                }))
            },
        },
    ],
};

pub(crate) static CONTROLS: PanelSection = PanelSection {
    group: Some(("Controls", icons::SLIDERS)),
    panels: &[
        PanelDef {
            label: "Track Info",
            icon: icons::INFO,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| TrackInfoPanel::new(state.clone(), TrackInfoConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Playback",
            icon: icons::PLAY,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| TransportPanel::new(state.clone(), TransportConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Seek",
            icon: icons::FAST_FORWARD,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| SeekStripPanel::new(state.clone(), SeekConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Volume",
            icon: icons::VOLUME_2,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| VolumePanel::new(state.clone(), VolumeConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Queue Widget",
            icon: icons::LIST_MUSIC,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| {
                    QueueWidgetPanel::new(state.clone(), QueueWidgetConfig::default(), cx)
                }))
            },
        },
    ],
};

pub(crate) static VISUALIZERS: PanelSection = PanelSection {
    group: Some(("Visualizers", icons::EYE)),
    panels: &[
        PanelDef {
            label: "Spectrum",
            icon: icons::AUDIO_LINES,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| SpectrumPanel::new(state.clone(), SpectrumConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Waveform",
            icon: icons::AUDIO_WAVEFORM,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| WaveformPanel::new(state.clone(), WaveformConfig::default(), cx)),
                )
            },
        },
    ],
};

/// Whether a section holds the composition hosts (group, depth, slide).
/// The composite slot pickers gray these out: a composite can sit in a
/// tab, but not inside another composite's slot, so nesting stays one
/// level deep while the entries stay visible.
pub(crate) fn is_arrangement(section: &PanelSection) -> bool {
    std::ptr::eq(section, &ARRANGEMENT)
}

/// Every section in menu order, the groups laid out alphabetically so the
/// list reads the same in the menubar and the Add Panel flyout.
pub(crate) static CATALOG: &[&PanelSection] = &[
    &APPLICATION,
    &ARRANGEMENT,
    &CONTROLS,
    &CATALOGUE,
    &DETAILS,
    &VISUALIZERS,
];
