//! The panel catalog: every openable panel as one entry with its menu
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
use rox_panels::filter::{FilterConfig, FilterPanel};
use rox_panels::folder_tree::{FolderTreeConfig, FolderTreePanel};
use rox_panels::genre_grid::{GenreGridConfig, GenreGridPanel};
use rox_panels::grid::{GridConfig, GridPanel};
use rox_panels::history::{HistoryConfig, HistoryPanel};
use rox_panels::library::{LibraryConfig, LibraryPanel};
use rox_panels::lyrics::{LyricsConfig, LyricsPanel};
use rox_panels::metadata::{MetadataConfig, MetadataPanel};
use rox_panels::oscilloscope::{OscilloscopeConfig, OscilloscopePanel};
use rox_panels::output::{OutputConfig, OutputPanel};
use rox_panels::particles::{ParticlesConfig, ParticlesPanel};
use rox_panels::playlists::{PlaylistsConfig, PlaylistsPanel};
use rox_panels::queue::{QueueConfig, QueuePanel};
use rox_panels::search::{SearchConfig, SearchPanel};
use rox_panels::shader::{ShaderConfig, ShaderPanel};
use rox_panels::spacer::{SpacerConfig, SpacerPanel};
use rox_panels::spectrogram::{SpectrogramConfig, SpectrogramPanel};
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

/// One openable panel: what the menus show for it, where it goes, and
/// how to build one with a default config. The workspace handle is for
/// the panels that drive the workspace back (menu, window controls);
/// everything else ignores it.
pub(crate) struct PanelDef {
    /// An i18n key, not display text. Resolve through `rox_i18n::t!` or
    /// `rox_i18n::t_static` at the point a menu or picker renders it.
    pub label: &'static str,
    /// The panel's registry name, the string its `panel_name` returns and
    /// `workspace::register_panels` registers its builder under. The label
    /// doesn't derive from it ("art view" shows as Album Carousel), so a
    /// dump-shaped thing (a panel preset) finds its entry through this.
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
    /// The group's label (an i18n key, same as [`PanelDef::label`]) and
    /// icon; None for the bare top-level run.
    pub group: Option<(&'static str, &'static str)>,
    pub panels: &'static [PanelDef],
}

/// The music collection itself: browse, search, filter, and the play
/// queues. The panels used most often when getting around the library.
pub(crate) static CATALOGUE: PanelSection =
    PanelSection {
        group: Some(("panel-catalog-group-catalogue", icons::DISC)),
        panels: &[
            PanelDef {
                label: "panel-title-library",
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
                label: "panel-title-search",
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
                label: "panel-catalog-filter",
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
                label: "panel-catalog-folder-tree",
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
                label: "panel-title-album-grid",
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
                label: "panel-catalog-artist-grid",
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
                label: "panel-catalog-genre-grid",
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
                label: "panel-catalog-album-carousel",
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
                label: "panel-catalog-playlists",
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
                label: "panel-catalog-queue",
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
                label: "panel-catalog-history",
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
    group: Some(("panel-catalog-group-details", icons::INFO)),
    panels: &[
        PanelDef {
            label: "panel-catalog-cover-art",
            name: "cover art",
            icon: icons::IMAGE,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| CoverArtPanel::new(state.clone(), CoverConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "panel-catalog-metadata",
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
            label: "panel-title-lyrics",
            name: "lyrics",
            icon: icons::MIC,
            placement: PanelPlacement::Center,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| LyricsPanel::new(state.clone(), LyricsConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "panel-catalog-biography",
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
            label: "panel-title-output",
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
    group: Some(("panel-catalog-group-arrangement", icons::LAYOUT_DASHBOARD)),
    panels: &[
        PanelDef {
            label: "panel-catalog-drawer",
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
            label: "panel-title-group",
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
            label: "panel-catalog-overlay",
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
            label: "panel-catalog-slide",
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
    group: Some(("panel-catalog-group-application", icons::APP_WINDOW)),
    panels: &[
        PanelDef {
            label: "panel-catalog-menu",
            name: "menu",
            icon: icons::MENU,
            placement: PanelPlacement::Bottom,
            build: |state, ws, _, cx| {
                Arc::new(cx.new(|cx| MenuPanel::new(state.clone(), ws, MenuConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "panel-catalog-drag-anchor",
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
            label: "panel-catalog-spacer",
            name: "spacer",
            icon: icons::SQUARE_DASHED,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| SpacerPanel::new(state.clone(), SpacerConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "panel-catalog-window-controls",
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
            label: "panel-catalog-mini-toggle",
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
    group: Some(("panel-catalog-group-controls", icons::SLIDERS)),
    panels: &[
        PanelDef {
            label: "panel-catalog-track-info",
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
            label: "panel-catalog-status",
            name: "status",
            icon: icons::CHART_PIE,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| StatusPanel::new(state.clone(), StatusConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "panel-title-playback",
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
            label: "panel-catalog-seek",
            name: "seek",
            icon: icons::FAST_FORWARD,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| SeekStripPanel::new(state.clone(), SeekConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "panel-title-volume",
            name: "volume",
            icon: icons::VOLUME_2,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| VolumePanel::new(state.clone(), VolumeConfig::default(), cx)))
            },
        },
        // The rating and favourite panels retired from the catalog once
        // the stars and the heart became transport items and track info
        // pieces; the registry still builds them, so a layout that
        // holds one keeps restoring.
        PanelDef {
            label: "panel-catalog-queue-widget",
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
            label: "panel-catalog-eq-widget",
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
            label: "panel-catalog-stats-widget",
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
            label: "panel-catalog-theme-toggle",
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
    group: Some(("panel-catalog-group-visualizers", icons::EYE)),
    panels: &[
        PanelDef {
            label: "panel-catalog-spectrum",
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
            label: "panel-catalog-spectrogram",
            name: "spectrogram",
            icon: icons::WAVES,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| {
                    SpectrogramPanel::new(state.clone(), SpectrogramConfig::default(), cx)
                }))
            },
        },
        PanelDef {
            label: "panel-catalog-oscilloscope",
            name: "oscilloscope",
            icon: icons::ACTIVITY,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| {
                    OscilloscopePanel::new(state.clone(), OscilloscopeConfig::default(), cx)
                }))
            },
        },
        PanelDef {
            label: "panel-catalog-waveform",
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
            label: "panel-catalog-vu-meter",
            name: "vu meter",
            icon: icons::GAUGE,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| VuPanel::new(state.clone(), VuConfig::default(), cx)))
            },
        },
        PanelDef {
            label: "panel-title-shader",
            name: "shader",
            icon: icons::BLEND,
            placement: PanelPlacement::Bottom,
            build: |state, _, _, cx| {
                Arc::new(cx.new(|cx| ShaderPanel::new(state.clone(), ShaderConfig::default(), cx)))
            },
        },
    ],
};

/// The unfinished work: panels that are real enough to use but not settled
/// enough to ship. Hidden unless the Development page turns experimental
/// features on. A panel graduating moves its entry into the section it
/// belongs in, and nothing else about it changes.
pub(crate) static EXPERIMENTAL: PanelSection = PanelSection {
    group: Some(("panel-catalog-group-experimental", icons::FLASK)),
    panels: &[PanelDef {
        label: "panel-catalog-particles",
        name: "particles",
        icon: icons::STAR,
        placement: PanelPlacement::Bottom,
        build: |state, _, _, cx| {
            Arc::new(
                cx.new(|cx| ParticlesPanel::new(state.clone(), ParticlesConfig::default(), cx)),
            )
        },
    }],
};

/// Whether a section holds the composition hosts (group, overlay, slide).
/// The composite slot pickers gray these out: a composite can go in a
/// tab, but not inside another composite's slot, so nesting stays one
/// level deep while the entries stay visible.
pub(crate) fn is_arrangement(section: &PanelSection) -> bool {
    std::ptr::eq(section, &ARRANGEMENT)
}

/// Whether a section is gated behind the experimental flag.
pub(crate) fn is_experimental(section: &PanelSection) -> bool {
    std::ptr::eq(section, &EXPERIMENTAL)
}

/// The panels whose settings have knobs the shared signal pool can drive,
/// by label, the way the native menu keys its rows. Every menu that lists
/// the catalog marks these with the signal glyph, which the signals window
/// tells people to look for, so what the pool can drive is readable from
/// the menus rather than found by opening panels until a bindable row
/// turns up.
///
/// A panel joins the list by implementing [`rox_panel_api::signal_ui::RouteHost`]
/// and wrapping the rows that should be bindable in
/// [`rox_panel_api::signal_ui::bindable_row`].
const SIGNAL_PANELS: &[&str] = &["particles", "shader"];

pub(crate) fn supports_signals(def: &PanelDef) -> bool {
    // Registry names, not labels. A label is an i18n key whose whole job
    // is to change per language and get reworded by translators, so
    // matching identity on one is a list that silently stops matching.
    // The builder registry and saved presets both key off the name.
    SIGNAL_PANELS.contains(&def.name)
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
/// experimental run while the flag is off. Only discovery is gated: the
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
/// dump rather than a pick: a panel preset identifies itself by name, and
/// needs the icon and placement that name's entry has. Ungated: the
/// experimental flag hides panels from the pickers, it doesn't unmake a
/// preset somebody already saved.
pub(crate) fn def_for(name: &str) -> Option<&'static PanelDef> {
    CATALOG
        .iter()
        .flat_map(|section| section.panels.iter())
        .find(|def| def.name == name)
}

/// The section a registry name is in, for the pickers that gate by section.
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
    /// [`def_for`] finds each one. Names are what a saved dump stores, so a
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

    /// Every label in the catalog is a real message key.
    ///
    /// The field is documented as an i18n key, but nothing held it to
    /// that, and for a while some entries held display text instead.
    /// The two failure modes hide each other: an entry holding text
    /// renders correctly in English, and an entry holding a key renders
    /// as the key itself, so the menu looks half broken and only in a
    /// language nobody on the team reads. The source locale has every
    /// key by definition, so resolving there is the whole check.
    #[test]
    fn every_label_is_a_message_key() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some(rox_i18n::SOURCE_LOCALE));
        for section in CATALOG {
            if let Some((group, _)) = section.group {
                assert!(
                    rox_i18n::try_translate(group).is_some(),
                    "the group label {group} is not a message key"
                );
            }
            for def in section.panels {
                assert!(
                    rox_i18n::try_translate(def.label).is_some(),
                    "{}: the label {} is not a message key",
                    def.name,
                    def.label
                );
            }
        }
        rox_i18n::set_locale(None);
    }

    /// No menu prints a label without resolving it first.
    ///
    /// A label is a message key, so rendering it raw puts
    /// "panel-catalog-filter" on screen where the panel's name belongs.
    /// It reads as ordinary code to whoever writes the call site and
    /// only surfaces when somebody runs the app in a language they can
    /// read. Eight menus draw the catalog and this went wrong in seven
    /// of them across two separate rounds, so the check scans the
    /// crate's own source instead of trusting the next reviewer to
    /// catch the eighth.
    ///
    /// Only `def.label` is covered. The group labels come out of a
    /// destructured tuple and can't be matched on text alone.
    #[test]
    fn no_menu_renders_a_label_raw() {
        fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("the crate has a src directory") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    collect(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }
        // Resolving it, or handing the key onward for someone else to
        // resolve, both count. Printing it doesn't.
        const RESOLVED: [&str; 4] = [
            "t!(def.label",
            "t_static(def.label",
            "try_translate(def.label",
            "label: def.label",
        ];
        let mut paths = Vec::new();
        collect(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut paths,
        );
        paths.sort();
        let mut raw = Vec::new();
        for path in paths {
            let text = std::fs::read_to_string(&path).expect("the source is readable");
            // Tests below assert on labels as data, which isn't a render.
            let code = text.split("#[cfg(test)]").next().unwrap_or(&text);
            for (line_no, line) in code.lines().enumerate() {
                if line.contains("def.label") && !RESOLVED.iter().any(|ok| line.contains(ok)) {
                    raw.push(format!(
                        "{}:{}  {}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        line_no + 1,
                        line.trim()
                    ));
                }
            }
        }
        assert!(
            raw.is_empty(),
            "these draw a label without translating it: {raw:#?}"
        );
    }

    /// Identity never depends on a label. The signal list is the one place
    /// that used to match panels by their display string, which quietly
    /// stops matching the moment a translator rewords it.
    #[test]
    fn the_signal_list_names_registry_names() {
        for name in SIGNAL_PANELS {
            assert!(
                def_for(name).is_some(),
                "{name} is in the signal list but is not a registry name"
            );
        }
    }
}
