//! The panel catalog: every openable panel as one entry carrying its menu
//! label, icon, dock placement, and constructor. The menubar's Panels
//! menu, the menu panel, the empty window's launcher, and the tab groups'
//! right-click Add Panel submenu all draw from this table, so adding a
//! panel type is one entry here plus its restore builder in
//! `workspace::register_panels`.

use std::sync::Arc;

use gpui::{App, AppContext as _, WeakEntity, Window};
use rox_dock::PanelView;

use crate::panels::drawer::{DrawerConfig, DrawerPanel};
use crate::panels::group::{GroupConfig, GroupPanel};
use crate::panels::menu::{MenuConfig, MenuPanel};
use crate::panels::mini::{MiniToggleConfig, MiniTogglePanel};
use crate::panels::overlay::{OverlayConfig, OverlayPanel};
use crate::panels::queue_widget::{QueueWidgetConfig, QueueWidgetPanel};
use crate::panels::slide::{SlideConfig, SlidePanel};
use crate::panels::window_controls::{WindowControlsConfig, WindowControlsPanel};
use crate::workspace::Workspace;
use rox_design::assets::icons;
use rox_panel_api::panel::AppState;
use rox_panels::art::{ArtConfig, ArtPanel};
use rox_panels::artist_grid::{ArtistGridConfig, ArtistGridPanel};
use rox_panels::biography::{BiographyConfig, BiographyPanel};
use rox_panels::cover::{CoverArtPanel, CoverConfig};
use rox_panels::drag_anchor::{DragAnchorConfig, DragAnchorPanel};
use rox_panels::eq_widget::{EqWidgetConfig, EqWidgetPanel};
use rox_panels::favourite::{FavouriteConfig, FavouritePanel};
use rox_panels::filter::{FilterConfig, FilterPanel};
use rox_panels::folder_tree::{FolderTreeConfig, FolderTreePanel};
use rox_panels::genre_grid::{GenreGridConfig, GenreGridPanel};
use rox_panels::grid::{GridConfig, GridPanel};
use rox_panels::history::{HistoryConfig, HistoryPanel};
use rox_panels::library::{LibraryConfig, LibraryPanel};
use rox_panels::lyrics::{LyricsConfig, LyricsPanel};
use rox_panels::metadata::{MetadataConfig, MetadataPanel};
use rox_panels::output::{OutputConfig, OutputPanel};
use rox_panels::particles::{ParticlesConfig, ParticlesPanel};
use rox_panels::playlists::{PlaylistsConfig, PlaylistsPanel};
use rox_panels::queue::{QueueConfig, QueuePanel};
use rox_panels::rating::{RatingConfig, RatingPanel};
use rox_panels::search::{SearchConfig, SearchPanel};
use rox_panels::shader::{ShaderConfig, ShaderPanel};
use rox_panels::spacer::{SpacerConfig, SpacerPanel};
use rox_panels::spectrum::{SpectrumConfig, SpectrumPanel};
use rox_panels::stats_widget::{StatsWidgetConfig, StatsWidgetPanel};
use rox_panels::status::{StatusConfig, StatusPanel};
use rox_panels::theme_toggle::{ThemeToggleConfig, ThemeTogglePanel};
use rox_panels::transport::{
    SeekConfig, SeekStripPanel, TrackInfoConfig, TrackInfoPanel, TransportConfig, TransportPanel,
    VolumeConfig, VolumePanel,
};
use rox_panels::vu::{VuConfig, VuPanel};
use rox_panels::waveform::{WaveformConfig, WaveformPanel};

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
    /// The panel's registry name, the string its `panel_name` returns and
    /// `workspace::register_panels` registers its builder under. The label
    /// doesn't derive from it ("art view" shows as Album Carousel), so a
    /// dump-shaped thing - a panel preset - finds its entry through this.
    pub name: &'static str,
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
pub(crate) static CATALOGUE: PanelSection =
    PanelSection {
        group: Some(("Catalogue", icons::DISC)),
        panels: &[
            PanelDef {
                label: "Library",
                name: "library",
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
                name: "search",
                icon: icons::SEARCH,
                placement: PanelPlacement::Top,
                build: |state, _, window, cx| {
                    Arc::new(cx.new(|cx| {
                        SearchPanel::new(state.clone(), SearchConfig::default(), window, cx)
                    }))
                },
            },
            PanelDef {
                label: "Filter",
                name: "filter",
                icon: icons::FUNNEL,
                placement: PanelPlacement::Center,
                build: |state, _, window, cx| {
                    Arc::new(cx.new(|cx| {
                        FilterPanel::new(state.clone(), FilterConfig::default(), window, cx)
                    }))
                },
            },
            PanelDef {
                label: "Folder Tree",
                name: "folder tree",
                icon: icons::FOLDER,
                placement: PanelPlacement::Center,
                build: |state, _, window, cx| {
                    Arc::new(cx.new(|cx| {
                        FolderTreePanel::new(state.clone(), FolderTreeConfig::default(), window, cx)
                    }))
                },
            },
            PanelDef {
                label: "Album Grid",
                name: "album grid",
                icon: icons::LAYOUT_GRID,
                placement: PanelPlacement::Center,
                build: |state, _, window, cx| {
                    Arc::new(
                        cx.new(|cx| {
                            GridPanel::new(state.clone(), GridConfig::default(), window, cx)
                        }),
                    )
                },
            },
            PanelDef {
                label: "Artist Grid",
                name: "artist grid",
                icon: icons::USER,
                placement: PanelPlacement::Center,
                build: |state, _, window, cx| {
                    Arc::new(cx.new(|cx| {
                        ArtistGridPanel::new(state.clone(), ArtistGridConfig::default(), window, cx)
                    }))
                },
            },
            PanelDef {
                label: "Genre Grid",
                name: "genre grid",
                icon: icons::TAG,
                placement: PanelPlacement::Center,
                build: |state, _, window, cx| {
                    Arc::new(cx.new(|cx| {
                        GenreGridPanel::new(state.clone(), GenreGridConfig::default(), window, cx)
                    }))
                },
            },
            PanelDef {
                label: "Album Carousel",
                name: "art view",
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
                name: "playlists",
                icon: icons::LIST_MUSIC,
                placement: PanelPlacement::Center,
                build: |state, _, window, cx| {
                    Arc::new(cx.new(|cx| {
                        PlaylistsPanel::new(state.clone(), PlaylistsConfig::default(), window, cx)
                    }))
                },
            },
            PanelDef {
                label: "Queue",
                name: "queue",
                icon: icons::LIST_MUSIC,
                placement: PanelPlacement::Center,
                build: |state, _, window, cx| {
                    Arc::new(cx.new(|cx| {
                        QueuePanel::new(state.clone(), QueueConfig::default(), window, cx)
                    }))
                },
            },
            PanelDef {
                label: "History",
                name: "history",
                icon: icons::CLOCK,
                placement: PanelPlacement::Center,
                build: |state, _, window, cx| {
                    Arc::new(cx.new(|cx| {
                        HistoryPanel::new(state.clone(), HistoryConfig::default(), window, cx)
                    }))
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
            name: "cover art",
            icon: icons::IMAGE,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| CoverArtPanel::new(state.clone(), CoverConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Metadata",
            name: "metadata",
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
            name: "lyrics",
            icon: icons::MIC,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| LyricsPanel::new(state.clone(), LyricsConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Biography",
            name: "biography",
            icon: icons::USER,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| BiographyPanel::new(state.clone(), BiographyConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Output",
            name: "output",
            icon: icons::VOLUME_2,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| OutputPanel::new(state.clone(), OutputConfig::default(), cx)))
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
            label: "Drawer",
            name: "drawer",
            icon: icons::PANEL_BOTTOM,
            placement: PanelPlacement::Center,
            build: |state, ws, _, cx| {
                Arc::new(
                    cx.new(|cx| DrawerPanel::new(state.clone(), ws, DrawerConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Group",
            name: "group",
            icon: icons::COLUMNS_2,
            placement: PanelPlacement::Center,
            build: |state, ws, _, cx| {
                Arc::new(
                    cx.new(|cx| GroupPanel::new(state.clone(), ws, GroupConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Overlay",
            name: "overlay",
            icon: icons::LAYERS,
            placement: PanelPlacement::Center,
            build: |state, ws, _, cx| {
                Arc::new(
                    cx.new(|cx| OverlayPanel::new(state.clone(), ws, OverlayConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Slide",
            name: "slide",
            icon: icons::GALLERY,
            placement: PanelPlacement::Center,
            build: |state, ws, _, cx| {
                Arc::new(
                    cx.new(|cx| SlidePanel::new(state.clone(), ws, SlideConfig::default(), cx)),
                )
            },
        },
    ],
};

pub(crate) static APPLICATION: PanelSection = PanelSection {
    group: Some(("Application", icons::APP_WINDOW)),
    panels: &[
        PanelDef {
            label: "Menu",
            name: "menu",
            icon: icons::MENU,
            placement: PanelPlacement::Bottom,
            build: |state, ws, _, cx| {
                Arc::new(cx.new(|cx| MenuPanel::new(state.clone(), ws, MenuConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Drag Anchor",
            name: "drag anchor",
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
            label: "Spacer",
            name: "spacer",
            icon: icons::SQUARE_DASHED,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| SpacerPanel::new(state.clone(), SpacerConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Window Controls",
            name: "window controls",
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
            name: "mini toggle",
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
            name: "track info",
            icon: icons::INFO,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| TrackInfoPanel::new(state.clone(), TrackInfoConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Status",
            name: "status",
            icon: icons::CHART_PIE,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| StatusPanel::new(state.clone(), StatusConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Playback",
            name: "playback",
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
            name: "seek",
            icon: icons::FAST_FORWARD,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| SeekStripPanel::new(state.clone(), SeekConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Volume",
            name: "volume",
            icon: icons::VOLUME_2,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| VolumePanel::new(state.clone(), VolumeConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Rating",
            name: "rating",
            icon: icons::STAR,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| RatingPanel::new(state.clone(), RatingConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "Favourite",
            name: "favourite",
            icon: icons::HEART,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| FavouritePanel::new(state.clone(), FavouriteConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Queue Widget",
            name: "queue widget",
            icon: icons::LIST_MUSIC,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| {
                    QueueWidgetPanel::new(state.clone(), QueueWidgetConfig::default(), cx)
                }))
            },
        },
        PanelDef {
            label: "EQ Widget",
            name: "eq widget",
            icon: icons::AUDIO_LINES,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| EqWidgetPanel::new(state.clone(), EqWidgetConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Stats Widget",
            name: "stats widget",
            icon: icons::CHART_PIE,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| {
                    StatsWidgetPanel::new(state.clone(), StatsWidgetConfig::default(), cx)
                }))
            },
        },
        PanelDef {
            label: "Theme Toggle",
            name: "theme toggle",
            icon: icons::CONTRAST,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| {
                    ThemeTogglePanel::new(state.clone(), ThemeToggleConfig::default(), cx)
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
            name: "spectrum",
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
            name: "waveform",
            icon: icons::AUDIO_WAVEFORM,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| WaveformPanel::new(state.clone(), WaveformConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "VU Meter",
            name: "vu meter",
            icon: icons::GAUGE,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| VuPanel::new(state.clone(), VuConfig::default(), cx)))
            },
        },
    ],
};

/// The unfinished work: panels that are real enough to use but not settled
/// enough to ship. Hidden unless the Development page turns experimental
/// features on. A panel graduating moves its entry into the section it
/// belongs in, and nothing else about it changes.
pub(crate) static EXPERIMENTAL: PanelSection = PanelSection {
    group: Some(("Experimental", icons::FLASK)),
    panels: &[
        PanelDef {
            label: "Particles",
            name: "particles",
            icon: icons::STAR,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(
                    cx.new(|cx| ParticlesPanel::new(state.clone(), ParticlesConfig::default(), cx)),
                )
            },
        },
        PanelDef {
            label: "Shader",
            name: "shader",
            icon: icons::BLEND,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| ShaderPanel::new(state.clone(), ShaderConfig::default(), cx)))
            },
        },
    ],
};

/// Whether a section holds the composition hosts (group, overlay, slide).
/// The composite slot pickers gray these out: a composite can sit in a
/// tab, but not inside another composite's slot, so nesting stays one
/// level deep while the entries stay visible.
pub(crate) fn is_arrangement(section: &PanelSection) -> bool {
    std::ptr::eq(section, &ARRANGEMENT)
}

/// Whether a section is gated behind the experimental flag.
pub(crate) fn is_experimental(section: &PanelSection) -> bool {
    std::ptr::eq(section, &EXPERIMENTAL)
}

/// The panels whose settings carry knobs the shared signal pool can drive,
/// by label, the way the native menu keys its rows. Every menu that lists
/// the catalog marks these with the signal glyph, which is what the signals
/// window tells people to look for, so what the pool can reach is readable
/// from the menus rather than found by opening panels until a bindable row
/// turns up.
///
/// A panel joins the list by implementing [`rox_panel_api::signal_ui::RouteHost`]
/// and wrapping the rows it wants bindable in
/// [`rox_panel_api::signal_ui::bindable_row`].
const SIGNAL_PANELS: &[&str] = &["Particles", "Shader"];

pub(crate) fn supports_signals(def: &PanelDef) -> bool {
    SIGNAL_PANELS.contains(&def.label)
}

/// Every section in menu order, the groups laid out alphabetically so the
/// list reads the same in the menubar and the Add Panel flyout, with the
/// experimental run last. Read it through [`sections`] rather than
/// directly, so the gated entries stay out of the menus.
static CATALOG: &[&PanelSection] = &[
    &APPLICATION,
    &ARRANGEMENT,
    &CONTROLS,
    &CATALOGUE,
    &DETAILS,
    &VISUALIZERS,
    &EXPERIMENTAL,
];

/// The sections a panel picker should offer: the whole catalog, minus the
/// experimental run while the flag is off. Only discovery is gated - the
/// restore builders in `workspace::register_panels` stay registered either
/// way, so a layout holding an experimental panel keeps it after the flag
/// goes back off.
pub(crate) fn sections() -> impl Iterator<Item = &'static &'static PanelSection> {
    let experimental = rox_core::settings::experimental();
    CATALOG
        .iter()
        .filter(move |section| experimental || !is_experimental(section))
}

/// The catalog entry for a registry name, for the surfaces that start from a
/// dump rather than a pick: a panel preset knows what it is by name, and
/// needs the icon and placement that name's entry carries. Ungated on
/// purpose - the experimental flag hides panels from the pickers, it doesn't
/// unmake a preset somebody already saved.
pub(crate) fn def_for(name: &str) -> Option<&'static PanelDef> {
    CATALOG
        .iter()
        .flat_map(|section| section.panels.iter())
        .find(|def| def.name == name)
}

/// The section a registry name sits in, for the pickers that gate by section.
/// The composite slot menus gray out the arrangement panels, and a preset of
/// one has to gray out with them.
pub(crate) fn section_for(name: &str) -> Option<&'static PanelSection> {
    CATALOG
        .iter()
        .copied()
        .find(|section| section.panels.iter().any(|def| def.name == name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry's registry name is a distinct lowercase string, and
    /// [`def_for`] finds each one. Names are what a saved dump carries, so a
    /// duplicate or a stray capital costs a preset its panel.
    #[test]
    fn names_are_unique_and_resolvable() {
        let mut seen = std::collections::HashSet::new();
        for section in CATALOG {
            for def in section.panels {
                assert_eq!(
                    def.name,
                    def.name.to_lowercase(),
                    "{} has a capital in its registry name",
                    def.label
                );
                assert!(seen.insert(def.name), "two entries claim {}", def.name);
                let found = def_for(def.name).expect("its own name resolves");
                assert_eq!(found.label, def.label);
                assert!(section_for(def.name).is_some());
            }
        }
        assert!(def_for("no such panel").is_none());
    }
}
