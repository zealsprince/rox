//! The main window: an in-window menubar over the dock area. gpui only
//! surfaces `set_menus` in the macOS system bar, so the bar is drawn
//! in-window to behave the same on every platform. The dock, tabs, splits,
//! and resize come from gpui-component per ADR 7; duplicate and pop-out
//! live on the panels themselves. Playback UI is the transport panels in
//! the bottom dock; the PCM tap that feeds the audio views is drained by
//! the player's own pump task, so nothing here has to keep rendering for
//! playback's sake.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, deferred, div, prelude::*, px, svg, AnyElement, AnyWindowHandle, App, Axis, Context,
    DismissEvent, Div, Entity, ExternalPaths, FocusHandle, Focusable as _, FontFeatures, Global,
    KeyBinding, KeyDownEvent, MouseButton, PathPromptOptions, SharedString, Subscription, Task,
    WeakEntity, Window, WindowBounds,
};
use rox_dock::{
    register_panel, DockArea, DockAreaState, DockEvent, DockItem, Panel as _, PanelInfo, PanelView,
    StackPanel, TabPanel, ToggleZoom,
};

use gpui::rgba;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::PopupMenu;
use gpui_component::Icon;

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::panel_catalog::{self as catalog, PanelDef, PanelPlacement, PanelSection};
use crate::composite;
use crate::design::{palette, tokens};
use crate::track_ui::track_drag::PlayDrag;
use crate::history::{History, HistoryEvent};
use crate::lastfm::Scrobbler;
use crate::integrations::media_controls::{MediaCommand, MediaKeys, NowPlayingMeta};
use crate::panel::{self, AppState, TabHosts};
use crate::panels::art::{ArtConfig, ArtPanel};
use crate::panels::biography::BiographyPanel;
use crate::panels::cover::CoverArtPanel;
use crate::panels::depth::DepthPanel;
use crate::panels::drag_anchor::DragAnchorPanel;
use crate::panels::filter::{FilterConfig, FilterPanel};
use crate::panels::folder_tree::FolderTreePanel;
use crate::panels::grid::{GridConfig, GridPanel};
use crate::panels::group::GroupPanel;
use crate::panels::history::HistoryPanel;
use crate::panels::library::{Library, LibraryConfig, LibraryPanel};
use crate::panels::lyrics::{LyricsPanel, StampLine};
use crate::panels::menu::{MenuConfig, MenuPanel};
use crate::panels::metadata::MetadataPanel;
use crate::panels::mini::{MiniToggleConfig, MiniTogglePanel};
use crate::panels::playlists::PlaylistsPanel;
use crate::panels::queue::QueuePanel;
use crate::panels::queue_widget::QueueWidgetPanel;
use crate::panels::search::{SearchConfig, SearchPanel};
use crate::panels::slide::SlidePanel;
use crate::panels::spectrum::SpectrumPanel;
use crate::panels::transport::{
    SeekConfig, SeekStripPanel, TrackInfoConfig, TrackInfoPanel, TransportConfig, TransportPanel,
    VolumeConfig, VolumePanel,
};
use crate::panels::waveform::WaveformPanel;
use crate::panels::window_controls::{WindowControlsConfig, WindowControlsPanel};
use crate::player::{NowPlaying, Player};
use crate::quick_play::QuickPlay;
use crate::selection::Selection;
use crate::settings::{
    self, LastTrack, LayoutEdit, LayoutSize, NamedLayout, QueueState, QueuedTrack, Settings,
    WindowState, WorkspaceBundle,
};
use crate::query::shared_query::SharedQuery;
use crate::thumbs::Thumbs;
use crate::integrations::tray;

mod menubar;

const MENU_BAR_H: f32 = 30.0;

/// The open workspace windows, registered in [`Workspace::new`] and
/// dropped in the close hook. The last one to close takes the app with
/// it; settings, popout, and customize windows are children of a
/// workspace, not reasons to keep a headless process alive. Handles
/// rather than a count so the decorations toggle can reach exactly
/// these windows; the workspace rides along so code that only has a
/// window (the dock's tab-menu hook) can find the workspace behind it.
#[derive(Default)]
struct WorkspaceWindows(Vec<(AnyWindowHandle, WeakEntity<Workspace>)>);

impl Global for WorkspaceWindows {}

/// Renegotiate every workspace window's decorations to the live flag.
/// Only the main windows follow it; child windows (settings, popouts,
/// editors) keep the OS chrome. Called from the Window menu toggle and
/// the settings window's Appearance page.
pub(crate) fn apply_decorations(cx: &mut App) {
    let mode = settings::window_decorations();
    for (handle, _) in cx.default_global::<WorkspaceWindows>().0.clone() {
        handle
            .update(cx, |_, window, _| window.request_decorations(mode))
            .ok();
    }
    // Every window repaints, not just the renegotiated ones: the settings
    // window's Appearance toggle reads the flag live and would show stale
    // otherwise.
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// Tear down a workspace window's app-level state: persist its layout, drop
/// its player's art tint, forget the window, and quit once the last one
/// goes. The OS close runs this from `on_window_should_close`; the Window
/// Controls close button runs it too before removing the window, since a
/// programmatic `remove_window` never fires the OS hook. `workspace` is None
/// only when the entity has already gone.
///
/// With quit-to-tray on and a way back in resident (the tray icon, the dock
/// on macOS), the last close hands the state to [`tray::hold`] instead of
/// quitting: playback keeps going and the tray's Open adopts the same state
/// into a fresh window. The art tint stays too, so the reopened window comes
/// back themed without waiting for a track change.
pub(crate) fn close_workspace_window(
    workspace: Option<Entity<Workspace>>,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = window.window_handle();
    let open = cx.default_global::<WorkspaceWindows>();
    open.0.retain(|(h, _)| *h != handle);
    let last = open.0.is_empty();
    let stay = last && settings::quit_to_tray() && tray::resident(cx) && workspace.is_some();
    let mut had_media = false;
    if let Some(ws) = workspace {
        let player = ws.read(cx).state.player.entity_id();
        let state = stay.then(|| ws.read(cx).state.clone());
        ws.update(cx, |this, cx| {
            this.persist(window, cx);
            // Free the OS media service before a survivor re-registers it; the
            // D-Bus name is per-process, so both can't hold it at once.
            had_media = this.release_media();
        });
        match state {
            Some(state) => tray::hold(state, cx),
            // A shared pop-out re-seeds on its next track change, so a
            // stale entry never lingers.
            None => palette::forget(player, cx),
        }
    }
    // The window that owned the media service just closed with others still
    // open; hand the service to a survivor so the media keys keep working.
    if had_media && !last {
        if let Some((handle, ws)) = cx.default_global::<WorkspaceWindows>().0.first().cloned() {
            let _ = handle.update(cx, |_, window, cx| {
                if let Some(ws) = ws.upgrade() {
                    ws.update(cx, |this, cx| this.install_media(window, cx));
                }
            });
        }
    }
    // Closing the last workspace window quits; without this, a settings or
    // popout window left open keeps the app running with the menubar (and
    // New Window) gone.
    if last && !stay {
        cx.quit();
    }
}

/// The frontmost open workspace window and its shared state: what the tray
/// activates on Open, and whose player its Play/Pause drives. Skips entries
/// whose entity is already gone.
pub(crate) fn front_workspace(cx: &mut App) -> Option<(AnyWindowHandle, AppState)> {
    let open = cx.default_global::<WorkspaceWindows>().0.clone();
    open.iter().find_map(|(handle, ws)| {
        let state = ws.upgrade()?.read(cx).state.clone();
        Some((*handle, state))
    })
}

/// Whether `window` is one of the tracked workspace windows, told apart
/// from settings, popouts, and editors. The Window Controls close button
/// only runs the workspace teardown for these.
pub(crate) fn is_workspace_window(window: &Window, cx: &mut App) -> bool {
    let handle = window.window_handle();
    cx.default_global::<WorkspaceWindows>()
        .0
        .iter()
        .any(|(h, _)| *h == handle)
}

/// The workspace hosting `window`, when it is a workspace window (not a
/// popout, settings, or editor). The queue widget uses it to reach the
/// workspace and open the queue modal there.
pub(crate) fn workspace_for_window(window: &Window, cx: &App) -> Option<WeakEntity<Workspace>> {
    let handle = window.window_handle();
    cx.try_global::<WorkspaceWindows>()?
        .0
        .iter()
        .find(|(h, _)| *h == handle)
        .map(|(_, ws)| ws.clone())
}

/// Append the "Add Panel" flyout to a panel's dropdown as its own section:
/// the whole catalog as a submenu, every group (Application, Arrangement,
/// Controls, Catalogue, Details, Visualizers) as its own nested flyout. A
/// pick
/// opens the panel as a new tab of `tab_panel`, that very group, skipping
/// the placement rules the menubar routes follow. Built as a real submenu
/// the way [`crate::query::shared_query::search_flyout`] builds its Search flyout -
/// a hand-built menu entity behind a submenu item - so it works from every
/// host of the panel menu, the content context menus included. Leads with a
/// divider so it reads as its own band rather than the tail of whatever
/// content section sits above it; the separator is a no-op when Add Panel
/// would be the menu's first item. A popped-out panel (no group) or a window
/// with no workspace behind it gets nothing.
pub(crate) fn add_panel_submenu(
    menu: PopupMenu,
    tab_panel: Option<WeakEntity<TabPanel>>,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let Some(tabs) = tab_panel else {
        return menu;
    };
    let handle = window.window_handle();
    let Some(workspace) = cx
        .default_global::<WorkspaceWindows>()
        .0
        .iter()
        .find(|(h, _)| *h == handle)
        .map(|(_, ws)| ws.clone())
    else {
        return menu;
    };
    let submenu = PopupMenu::build(window, cx, move |mut menu, window, cx| {
        for section in catalog::CATALOG {
            match section.group {
                None => {
                    for def in section.panels {
                        menu = add_panel_item(menu, def, workspace.clone(), tabs.clone());
                    }
                }
                Some((label, icon)) => {
                    let workspace = workspace.clone();
                    let tabs = tabs.clone();
                    menu = menu.submenu_with_icon(
                        Some(Icon::default().path(icon)),
                        label,
                        window,
                        cx,
                        move |mut menu, _, _| {
                            for def in section.panels {
                                menu = add_panel_item(menu, def, workspace.clone(), tabs.clone());
                            }
                            menu
                        },
                    );
                }
            }
        }
        menu
    });
    menu.separator().item(
        gpui_component::menu::PopupMenuItem::submenu("Add Panel", submenu)
            .icon(Icon::default().path(icons::PLUS)),
    )
}

/// One Add Panel row: build the def's panel against the workspace's state
/// and land it as a tab of the clicked group.
fn add_panel_item(
    menu: PopupMenu,
    def: &'static PanelDef,
    workspace: WeakEntity<Workspace>,
    tabs: WeakEntity<TabPanel>,
) -> PopupMenu {
    menu.item(
        gpui_component::menu::PopupMenuItem::new(def.label)
            .icon(Icon::default().path(def.icon))
            .on_click(move |_, window, cx| {
                let (Some(ws), Some(tabs)) = (workspace.upgrade(), tabs.upgrade()) else {
                    return;
                };
                let state = ws.read(cx).state.clone();
                let panel = (def.build)(&state, workspace.clone(), window, cx);
                tabs.update(cx, |tabs, cx| tabs.add_panel(panel, window, cx));
            }),
    )
}

/// Versions the layout dump in settings. Bump on incompatible panel or
/// schema changes; a dump from another version is ignored and the default
/// layout builds instead.
const LAYOUT_VERSION: usize = 1;

/// Layout events fire for every step of a drag or resize, so a save waits
/// out this much quiet first. The close hook catches whatever a pending
/// debounce still holds.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// The transport row's starting height, just enough for the controls plus
/// their status line. The row is a regular split in the one layout tree,
/// so it resizes and collapses like everything else.
const TRANSPORT_ROW_H: f32 = 120.0;

/// The search bar's starting height: the 30px tab strip plus its one input
/// row. It joins as a thin strip, not a tall tile, so it opens near its
/// minimum instead of taking a center panel's share.
const SEARCH_BAR_H: f32 = 52.0;

actions!(
    rox,
    [
        TogglePlayback,
        SeekBackward,
        SeekForward,
        OpenSettings,
        OpenStats,
        OpenQuickPlay,
        FocusSearch,
        Quit
    ]
);

/// Bindings match key contexts along the focus path, so this scope holds
/// anywhere inside a workspace window except while the library search box
/// is focused: there space and arrows keep typing into the query. Bindings
/// win over key listeners, the exclusion is what hands the keys back.
const PLAYBACK_KEY_SCOPE: Option<&str> = Some("Workspace && !SearchInput");

/// App-level key bindings; call once at startup.
pub fn init(cx: &mut App) {
    // Quit binds unscoped so it fires in every window, popped-out panels
    // and the search box included. The macOS system menu is not set, so
    // Cmd+Q only exists through this binding.
    let quit_keys = if cfg!(target_os = "macos") {
        "cmd-q"
    } else {
        "alt-f4"
    };
    // Preferences shortcut follows the platform: Cmd+, on macOS, Ctrl+,
    // elsewhere. Ctrl+I is a second binding on both. These carry modifiers,
    // so they stay unscoped past the search box without stealing typing.
    let settings_keys = if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    };
    // The quick-play modal answers both the palette chord and the find
    // chord; either habit lands in the same search.
    let (quick_play_p, quick_play_f) = if cfg!(target_os = "macos") {
        ("cmd-p", "cmd-f")
    } else {
        ("ctrl-p", "ctrl-f")
    };
    // Stats takes the shifted S so it stays clear of the settings and
    // quick-play chords.
    let stats_keys = if cfg!(target_os = "macos") {
        "cmd-shift-s"
    } else {
        "ctrl-shift-s"
    };
    // Jump to the search box, the browser's address-bar chord. Modified, so
    // it stays out of the way of typing in the box itself.
    let focus_search_keys = if cfg!(target_os = "macos") {
        "cmd-l"
    } else {
        "ctrl-l"
    };
    cx.bind_keys([
        KeyBinding::new("space", TogglePlayback, PLAYBACK_KEY_SCOPE),
        KeyBinding::new("left", SeekBackward, PLAYBACK_KEY_SCOPE),
        KeyBinding::new("right", SeekForward, PLAYBACK_KEY_SCOPE),
        KeyBinding::new(settings_keys, OpenSettings, Some("Workspace")),
        KeyBinding::new("ctrl-i", OpenSettings, Some("Workspace")),
        KeyBinding::new(stats_keys, OpenStats, Some("Workspace")),
        KeyBinding::new(quick_play_p, OpenQuickPlay, Some("Workspace")),
        KeyBinding::new(quick_play_f, OpenQuickPlay, Some("Workspace")),
        KeyBinding::new(focus_search_keys, FocusSearch, Some("Workspace")),
        // Fullscreens the last-clicked panel group over the whole dock
        // area; the same chord or a plain escape backs out. Shift keeps
        // it off the search boxes' bare-escape ladder. This is the dock's
        // own action, so the zoom controls in every panel's menus render
        // the chord next to "Zoom In".
        KeyBinding::new("shift-escape", ToggleZoom, Some("Workspace")),
        // Stamp the current time onto a lyric line, live only inside the
        // lyrics editor (the LyricsEdit context). Shift+Enter is the same
        // chord on every platform and the input leaves it unbound, unlike
        // plain and secondary Enter which type a newline.
        KeyBinding::new("shift-enter", StampLine, Some("LyricsEdit")),
        KeyBinding::new(quit_keys, Quit, None),
    ]);
    // Fallback for windows without a workspace in the focus path (popped-out
    // panels); workspace windows persist their layout first via their own
    // handler, which stops the action before it gets here.
    cx.on_action(|_: &Quit, cx| cx.quit());
}

/// Teach the dock's registry to rebuild every panel type from a layout
/// dump. Registered per workspace so the builders capture that workspace's
/// entities; the restore runs synchronously right after, before another
/// workspace can re-register. The workspace handle is for the panels that
/// drive it back: the window controls' mini toggle and the menu panel.
fn register_panels(state: &AppState, workspace: WeakEntity<Workspace>, cx: &mut App) {
    let s = state.clone();
    register_panel(cx, "library", move |_, _, info, window, cx| {
        let config: LibraryConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| LibraryPanel::new(s.clone(), config, window, cx)))
    });
    // A panel whose config rides the layout dump.
    macro_rules! configured {
        ($name:literal, $panel:ty) => {{
            let s = state.clone();
            register_panel(cx, $name, move |_, _, info, _, cx| {
                let config = panel::config_from_info(info);
                Box::new(cx.new(|cx| <$panel>::new(s.clone(), config, cx)))
            });
        }};
    }
    // The same, but the constructor takes a window: a searching panel spins up
    // its search box's input state at build, like the library's and grid's.
    macro_rules! configured_windowed {
        ($name:literal, $panel:ty) => {{
            let s = state.clone();
            register_panel(cx, $name, move |_, _, info, window, cx| {
                let config = panel::config_from_info(info);
                Box::new(cx.new(|cx| <$panel>::new(s.clone(), config, window, cx)))
            });
        }};
    }
    // Filter carries a window at build so its quick-search box can spin up
    // an input state, like the library panel's search.
    let s = state.clone();
    register_panel(cx, "filter", move |_, _, info, window, cx| {
        let config: FilterConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| FilterPanel::new(s.clone(), config, window, cx)))
    });
    // The folder tree takes a window at build to match its constructor,
    // like the filter's.
    let s = state.clone();
    register_panel(cx, "folder tree", move |_, _, info, window, cx| {
        let config = panel::config_from_info(info);
        Box::new(cx.new(|cx| FolderTreePanel::new(s.clone(), config, window, cx)))
    });
    configured!("seek", SeekStripPanel);
    configured!("track info", TrackInfoPanel);
    configured!("cover art", CoverArtPanel);
    configured!("metadata", MetadataPanel);
    configured!("lyrics", LyricsPanel);
    configured!("biography", BiographyPanel);
    configured_windowed!("history", HistoryPanel);
    configured_windowed!("queue", QueuePanel);
    configured!("queue widget", QueueWidgetPanel);
    configured_windowed!("playlists", PlaylistsPanel);
    // The composition hosts rebuild their children through this same
    // registry, and carry the workspace handle so their slot menus can
    // build replacements from the catalog.
    macro_rules! composite {
        ($name:literal, $panel:ty) => {{
            let s = state.clone();
            let ws = workspace.clone();
            register_panel(
                cx,
                $name,
                move |dock_area, panel_state, info, window, cx| {
                    let config = panel::config_from_info(info);
                    let slots = composite::restore_slots(&dock_area, panel_state, window, cx);
                    Box::new(
                        cx.new(|cx| <$panel>::restore(s.clone(), ws.clone(), config, slots, cx)),
                    )
                },
            );
        }};
    }
    composite!("group", GroupPanel);
    composite!("depth", DepthPanel);
    composite!("slide", SlidePanel);
    // The grid takes the window like the library: its search box builds
    // an input state.
    let s = state.clone();
    register_panel(cx, "album grid", move |_, _, info, window, cx| {
        let config: GridConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| GridPanel::new(s.clone(), config, window, cx)))
    });
    // The art strip shares the grid's search box, so it takes the window
    // the same way.
    let s = state.clone();
    register_panel(cx, "art view", move |_, _, info, window, cx| {
        let config: ArtConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| ArtPanel::new(s.clone(), config, window, cx)))
    });
    // The search panel builds its box's input state, so it takes the window
    // like the library.
    let s = state.clone();
    register_panel(cx, "search", move |_, _, info, window, cx| {
        let config: SearchConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| SearchPanel::new(s.clone(), config, window, cx)))
    });
    configured!("playback", TransportPanel);
    configured!("volume", VolumePanel);
    configured!("spectrum", SpectrumPanel);
    configured!("waveform", WaveformPanel);
    configured!("drag anchor", DragAnchorPanel);
    // These two drive the workspace back, so their builders carry its
    // handle alongside the shared state.
    let s = state.clone();
    let ws = workspace.clone();
    register_panel(cx, "window controls", move |_, _, info, _, cx| {
        let config: WindowControlsConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| WindowControlsPanel::new(s.clone(), ws.clone(), config, cx)))
    });
    let s = state.clone();
    let ws = workspace.clone();
    register_panel(cx, "mini toggle", move |_, _, info, _, cx| {
        let config: MiniToggleConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| MiniTogglePanel::new(s.clone(), ws.clone(), config, cx)))
    });
    let s = state.clone();
    register_panel(cx, "menu", move |_, _, info, _, cx| {
        let config: MenuConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| MenuPanel::new(s.clone(), workspace.clone(), config, cx)))
    });
}

#[derive(Clone, Copy)]
pub(crate) enum MenuAction {
    NewWindow,
    EmptyWindow,
    OpenSettings,
    OpenStats,
    OpenWelcome,
    ToggleMenubar,
    ToggleDecorations,
    /// Pick a workspace file and add it to the collection.
    ImportWorkspace,
    /// Open a catalog panel with its default config, landing where its
    /// placement says. One action for every panel the catalog carries.
    OpenPanel(&'static PanelDef),
    ToggleQuitToTray,
    CloseWindow,
    Quit,
}

#[derive(Clone, Copy)]
pub(crate) struct MenuItem {
    pub(crate) label: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) action: MenuAction,
}

/// A catalog entry as a dropdown row, for the renderers that show panel
/// sections: the def's own label and icon over its open action.
pub(crate) fn panel_menu_item(def: &'static PanelDef) -> MenuItem {
    MenuItem {
        label: def.label,
        icon: def.icon,
        action: MenuAction::OpenPanel(def),
    }
}

/// What picking a preset in a layouts flyout does.
#[derive(Clone, Copy)]
pub(crate) enum LayoutTarget {
    /// Open a fresh window built from the preset.
    NewWindow,
    /// Replace the preset with the current arrangement.
    Overwrite,
    /// Swap the current window into the preset, after a confirm.
    Apply,
}

/// What picking a workspace in a workspaces flyout does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceTarget {
    /// Replace the bundle with the current look. The Save flyout only offers
    /// user bundles, so this never targets a shipped one.
    Overwrite,
    /// Apply the bundle's whole look to this window, after a confirm.
    Apply,
}

/// A dropdown row: an action item, a muted section heading over a divider,
/// a run of panel-catalog entries, or a layout-presets flyout whose items
/// come from the saved and shipped presets at open time.
pub(crate) enum MenuEntry {
    Item(MenuItem),
    Section(&'static str),
    /// A catalog section: a bare one renders as plain rows in place, a
    /// labeled group as a submenu that flies out on hover.
    Panels(&'static PanelSection),
    LayoutsSubmenu {
        label: &'static str,
        icon: &'static str,
        target: LayoutTarget,
        /// Lead the flyout with a "New..." row that opens the save dialog,
        /// the Save Layout submenu's way to a fresh preset.
        with_new: bool,
    },
    /// A workspaces flyout whose items are the saved and shipped workspaces,
    /// read at open time; picking one does the flyout's `target` with that
    /// bundle behind a confirm.
    WorkspacesSubmenu {
        label: &'static str,
        icon: &'static str,
        target: WorkspaceTarget,
        /// Lead the flyout with a "New..." row that opens the save dialog,
        /// the Save Workspace submenu's way to a fresh bundle.
        with_new: bool,
    },
}

pub(crate) struct Menu {
    pub(crate) label: &'static str,
    pub(crate) entries: &'static [MenuEntry],
}

pub(crate) const MENUS: &[Menu] = &[
    Menu {
        label: "Application",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "Settings",
                icon: icons::SETTINGS,
                action: MenuAction::OpenSettings,
            }),
            MenuEntry::Item(MenuItem {
                label: "Stats",
                icon: icons::CHART_PIE,
                action: MenuAction::OpenStats,
            }),
            MenuEntry::Item(MenuItem {
                label: "Welcome",
                icon: icons::INFO,
                action: MenuAction::OpenWelcome,
            }),
            MenuEntry::Section("Session"),
            MenuEntry::Item(MenuItem {
                label: "Exit",
                icon: icons::CLOSE,
                action: MenuAction::Quit,
            }),
        ],
    },
    Menu {
        label: "Window",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "New Window",
                icon: icons::PLUS,
                action: MenuAction::NewWindow,
            }),
            MenuEntry::Item(MenuItem {
                label: "Empty Window",
                icon: icons::SQUARE_DASHED,
                action: MenuAction::EmptyWindow,
            }),
            MenuEntry::Section("Interface"),
            MenuEntry::Item(MenuItem {
                label: "Hide Menubar",
                icon: icons::EYE,
                action: MenuAction::ToggleMenubar,
            }),
            MenuEntry::Item(MenuItem {
                label: "OS Decorations",
                icon: icons::APP_WINDOW,
                action: MenuAction::ToggleDecorations,
            }),
            MenuEntry::Section("Session"),
            MenuEntry::Item(MenuItem {
                // Shown on Windows too, where the close path ignores the
                // flag until a tray backend exists there; the settings row
                // explains the platform story.
                label: "Remain in Tray",
                icon: icons::MINIMIZE,
                action: MenuAction::ToggleQuitToTray,
            }),
            MenuEntry::Item(MenuItem {
                // Closes this window, unlike Application's Exit which quits.
                label: "Close",
                icon: icons::CLOSE,
                action: MenuAction::CloseWindow,
            }),
        ],
    },
    Menu {
        label: "Workspace",
        entries: &[
            MenuEntry::WorkspacesSubmenu {
                label: "Apply Workspace",
                icon: icons::GALLERY,
                target: WorkspaceTarget::Apply,
                with_new: false,
            },
            MenuEntry::WorkspacesSubmenu {
                label: "Save Workspace",
                icon: icons::DOWNLOAD,
                target: WorkspaceTarget::Overwrite,
                with_new: true,
            },
            MenuEntry::Item(MenuItem {
                label: "Import Workspace...",
                icon: icons::UPLOAD,
                action: MenuAction::ImportWorkspace,
            }),
            MenuEntry::Section("Layouts"),
            MenuEntry::LayoutsSubmenu {
                label: "New Window from Layout",
                icon: icons::LAYOUT_DASHBOARD,
                target: LayoutTarget::NewWindow,
                with_new: false,
            },
            MenuEntry::LayoutsSubmenu {
                label: "Save Layout",
                icon: icons::UPLOAD,
                target: LayoutTarget::Overwrite,
                with_new: true,
            },
            MenuEntry::LayoutsSubmenu {
                label: "Apply Layout",
                icon: icons::DOWNLOAD,
                target: LayoutTarget::Apply,
                with_new: false,
            },
        ],
    },
    Menu {
        label: "Panels",
        entries: &[
            MenuEntry::Panels(&catalog::APPLICATION),
            MenuEntry::Panels(&catalog::ARRANGEMENT),
            MenuEntry::Panels(&catalog::CONTROLS),
            MenuEntry::Panels(&catalog::CATALOGUE),
            MenuEntry::Panels(&catalog::DETAILS),
            MenuEntry::Panels(&catalog::VISUALIZERS),
        ],
    },
];

/// The keybinding a dropdown row trails, Zed-style, matching [`init`]'s
/// bindings. Only the primary chord shows; secondaries like Ctrl+I stay
/// off the label.
pub(crate) fn shortcut_for(action: MenuAction) -> Option<&'static str> {
    match action {
        MenuAction::OpenSettings => Some(if cfg!(target_os = "macos") {
            "Cmd-,"
        } else {
            "Ctrl-,"
        }),
        MenuAction::OpenStats => Some(if cfg!(target_os = "macos") {
            "Cmd-Shift-S"
        } else {
            "Ctrl-Shift-S"
        }),
        _ => None,
    }
}

/// A layout action waiting on its dialog, floated over the window: naming a
/// new preset to save, or confirming an apply that replaces the current
/// arrangement.
enum LayoutDialog {
    Save(Entity<InputState>),
    ConfirmOverwrite(String),
    ConfirmApply(String),
    /// Naming a new workspace bundle built from the current look.
    SaveWorkspace(Entity<InputState>),
    /// Replacing a saved workspace of the same name with the current look.
    ConfirmOverwriteWorkspace(String),
    /// Applying a saved or shipped workspace, which replaces the whole look.
    ConfirmApplyWorkspace(String),
}

/// How a workspace window opens, which the menubar's Window entries pick.
pub enum WorkspaceStart {
    /// Launch and plain New Window: restore the saved working layout, and
    /// on launch the last playing track.
    Restore,
    /// A blank dock the user fills from the Panels menu.
    Empty,
    /// Built from a named preset's dump, saved or shipped.
    Preset(String),
}

pub struct Workspace {
    open_menu: Option<usize>,
    /// Which submenu entry of the open dropdown is flown out, by entry
    /// index. Hovering an entry moves it, closing the menu clears it.
    open_submenu: Option<usize>,
    /// A mouse button is held down somewhere in the window. Alt+drag is
    /// the compositor's window move/resize, so an alt-revealed menubar
    /// stays hidden while a button is down: the overlay must not sit in
    /// front of the drag. Tracked in the capture phase so an occluding
    /// child can't hide the press.
    pointer_down: bool,
    state: AppState,
    /// Fallback focus so the key bindings keep a dispatch path under the
    /// Workspace context even before a panel takes focus. The dock focuses
    /// the active panel on activation and takes over from there.
    focus: FocusHandle,
    dock: Entity<DockArea>,
    /// The root of the one layout tree: center tabs over the transport
    /// row, vertically. One tree rather than center-plus-bottom-dock so
    /// closing or moving everything in one region collapses the rest up
    /// into the space.
    stack: Entity<StackPanel>,
    /// The tab panel the layout starts with. Panels-menu panels land here
    /// while it is still showing.
    center_tabs: Entity<TabPanel>,
    /// The transport row's stack: the transport groups at start, and the
    /// row Panels-menu audio panels append to.
    bottom_stack: Entity<StackPanel>,
    /// The debounce for layout saves; replacing it cancels the running
    /// timer, so only a quiet layout dumps.
    save_task: Option<Task<()>>,
    /// The mini-player button's two presets, by name. Cached off the settings
    /// file so the menubar never reads disk per frame; the settings window
    /// pushes changes back through [`Workspace::set_mini_roles`]. The button
    /// hides unless a mini preset is set.
    primary_layout: Option<String>,
    mini_layout: Option<String>,
    /// The named preset this window is on, mirrored to settings on every
    /// apply. Which side of the mini toggle shows falls out of comparing it to
    /// `mini_layout`; a workspace save captures into it. None is an unnamed
    /// arrangement.
    active_layout: Option<String>,
    /// The preset names the empty-window launcher lists, read once when
    /// the launcher shows and dropped when the dock fills again, so the
    /// per-frame render never touches the settings file.
    empty_presets: Option<Vec<String>>,
    /// The workspace names the empty-window launcher lists, cached the same
    /// way as the presets above.
    empty_workspaces: Option<Vec<String>>,
    /// The layout save/apply dialog while it is up; dropped on close.
    layout_dialog: Option<LayoutDialog>,
    /// Submits the save dialog's name field on Enter.
    _layout_input: Option<Subscription>,
    /// The quick-play modal while it is up; dropped on dismiss.
    quick_play: Option<Entity<QuickPlay>>,
    /// Clears `quick_play` and hands focus back when the modal dismisses.
    _quick_play_dismissed: Option<Subscription>,
    /// The queue modal the queue widget opens when no queue panel is docked;
    /// a throwaway queue panel floated over the workspace, dropped on close.
    queue_modal: Option<Entity<QueuePanel>>,
    /// This window's slice of the backdrop: what it painted last, for
    /// retiring the texture on a new bake.
    backdrop: WindowBackdrop,
    /// The playing path the window title currently reflects; None while
    /// the title is the plain app name. Compared each player tick so the
    /// tag lookup and the platform title call only run on a track change.
    titled_track: Option<PathBuf>,
    _layout_changed: Subscription,
    /// The player pump notifies every tick while a session runs; the
    /// title refresh rides it and bails on the path compare.
    _player_changed: Subscription,
    /// The menubar's right side shows the catalog status, so library
    /// updates must repaint the workspace.
    _library_changed: Subscription,
    /// A landed listen bumps its track's play count in the shared
    /// projection, so plays columns move without a reload.
    _history_changed: Subscription,
    /// A new bake must repaint the window that shows it.
    _backdrop_changed: Subscription,
    /// The OS media service, on the primary window only: the D-Bus name is
    /// per-process, so a second window never registers its own. `None` on
    /// every other window and when the platform backend won't come up.
    media: Option<MediaKeys>,
    /// The path the media widget's tags currently reflect, so the library
    /// resolve behind them only runs on a track change, not every frame.
    media_track: Option<PathBuf>,
    /// The await loop pulling media-key presses off souvlaki's thread; dropped
    /// with the window, which ends the loop.
    _media_events: Option<Task<()>>,
}

/// A one-group tabs item plus the TabPanel entity inside it, for wiring the
/// group into a stack.
fn tabs_item(
    panels: Vec<Arc<dyn PanelView>>,
    weak_dock: &WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) -> (DockItem, Entity<TabPanel>) {
    let item = DockItem::tabs(panels, weak_dock, window, cx);
    let view = match &item {
        DockItem::Tabs { view, .. } => view.clone(),
        _ => unreachable!("DockItem::tabs builds a Tabs item"),
    };
    (item, view)
}

/// The split's StackPanel entity, for keeping a handle to a stack a
/// DockItem builder created.
fn split_view(item: &DockItem) -> Entity<StackPanel> {
    match item {
        DockItem::Split { view, .. } => view.clone(),
        _ => unreachable!("split_with_sizes builds a Split item"),
    }
}

/// The starting layout: the library tab group over the transport row.
/// Returns the center item plus the workspace's add targets: the root
/// stack, the center tabs, and the transport row's stack.
fn default_layout(
    state: &AppState,
    weak_dock: &WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) -> (
    DockItem,
    Entity<StackPanel>,
    Entity<TabPanel>,
    Entity<StackPanel>,
) {
    let library_panel =
        cx.new(|cx| LibraryPanel::new(state.clone(), LibraryConfig::default(), window, cx));
    let (tabs, center_tabs) = tabs_item(vec![Arc::new(library_panel)], weak_dock, window, cx);

    // The transport pieces as side-by-side tab groups in one row: the track
    // info readout, the controls, the seek strip, and the volume strip.
    let info = cx.new(|cx| TrackInfoPanel::new(state.clone(), TrackInfoConfig::default(), cx));
    let playback = cx.new(|cx| TransportPanel::new(state.clone(), TransportConfig::default(), cx));
    let seek = cx.new(|cx| SeekStripPanel::new(state.clone(), SeekConfig::default(), cx));
    let volume = cx.new(|cx| VolumePanel::new(state.clone(), VolumeConfig::default(), cx));
    let (info_item, _) = tabs_item(vec![Arc::new(info)], weak_dock, window, cx);
    let (playback_item, _) = tabs_item(vec![Arc::new(playback)], weak_dock, window, cx);
    let (seek_item, _) = tabs_item(vec![Arc::new(seek)], weak_dock, window, cx);
    let (volume_item, _) = tabs_item(vec![Arc::new(volume)], weak_dock, window, cx);
    let transport_row = DockItem::split_with_sizes(
        Axis::Horizontal,
        vec![info_item, playback_item, seek_item, volume_item],
        vec![None, Some(px(420.)), None, Some(px(280.))],
        weak_dock,
        window,
        cx,
    );
    let bottom_stack = split_view(&transport_row);

    // One vertical tree: the center tabs over the transport row, no
    // separate bottom dock. Closing or moving everything out of one region
    // hands its space to the rest instead of leaving a hole, and a parent
    // stack is also what makes tab dragging, splitting, and closing
    // possible at all.
    let center = DockItem::split_with_sizes(
        Axis::Vertical,
        vec![tabs, transport_row],
        vec![None, Some(px(TRANSPORT_ROW_H))],
        weak_dock,
        window,
        cx,
    );
    let stack = split_view(&center);
    (center, stack, center_tabs, bottom_stack)
}

/// A blank starting layout: one empty tab group in the root stack, no
/// transport row. The group renders to nothing while empty, so the window
/// is bare until the Panels menu adds something; the detached transport
/// stack rides along for [`Workspace::add_bottom`] to attach on first use,
/// same as a restored layout with no row.
fn empty_layout(
    weak_dock: &WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) -> (
    DockItem,
    Entity<StackPanel>,
    Entity<TabPanel>,
    Entity<StackPanel>,
) {
    let (tabs, center_tabs) = tabs_item(Vec::new(), weak_dock, window, cx);
    let center = DockItem::split_with_sizes(
        Axis::Vertical,
        vec![tabs],
        vec![None],
        weak_dock,
        window,
        cx,
    );
    let stack = split_view(&center);
    let bottom_stack = cx.new(|cx| StackPanel::new(Axis::Horizontal, window, cx));
    (center, stack, center_tabs, bottom_stack)
}

/// Pull the workspace's add targets back out of a restored layout: the
/// root stack, the first tab group (where add_center prefers to land), and
/// the last horizontal split (the transport row add_bottom appends to).
/// The latter two are heuristics over a tree the user may have rearranged,
/// so they can come up empty.
fn layout_views(
    item: &DockItem,
) -> (
    Entity<StackPanel>,
    Option<Entity<TabPanel>>,
    Option<Entity<StackPanel>>,
) {
    let DockItem::Split { view, items, .. } = item else {
        unreachable!("a restored root stack is a Split item");
    };
    let center_tabs = items.iter().find_map(|child| match child {
        DockItem::Tabs { view, .. } => Some(view.clone()),
        _ => None,
    });
    let bottom = items.iter().rev().find_map(|child| match child {
        DockItem::Split { axis, view, .. } if *axis == Axis::Horizontal => Some(view.clone()),
        _ => None,
    });
    (view.clone(), center_tabs, bottom)
}

/// Drain the OS media service's key presses onto the shared player and keep
/// the widget in step. Shared by the primary window's launch and the hand-off
/// when that window closes with another still open.
fn spawn_media_events(keys: &MediaKeys, cx: &mut Context<Workspace>) -> Task<()> {
    let events = keys.events();
    cx.spawn(async move |this, cx| {
        let _ = this.update(cx, |this, cx| this.publish_media(cx));
        while let Ok(cmd) = events.recv().await {
            let applied = this.update(cx, |this, cx| {
                this.apply_media(cmd, cx);
                this.publish_media(cx);
            });
            if applied.is_err() {
                break;
            }
        }
    })
}

impl Workspace {
    pub fn new(
        start: WorkspaceStart,
        adopt: Option<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A reopen from the tray adopts the state the last close handed to
        // the hold, so the playing player, library, and art carry straight
        // over; every other open builds its own world.
        let adopted = adopt.is_some();
        let state = adopt.unwrap_or_else(|| {
            let player = cx.new(Player::new);
            let library = cx.new(Library::new);
            let scrobbler = cx.new(|cx| Scrobbler::new(&player, &library, cx));
            AppState {
                thumbs: cx.new(|cx| Thumbs::new(&library, cx)),
                history: cx.new(|cx| History::new(&scrobbler, cx)),
                scrobbler,
                library,
                now_art: cx.new(|cx| NowPlayingArt::new(player.clone(), cx)),
                player,
                selection: cx.new(|_| Selection::default()),
                query: cx.new(|_| SharedQuery::default()),
                tab_hosts: cx.new(|_| TabHosts::default()),
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus);

        // The first workspace window is the launch, so it brings back what
        // was playing: the saved id resolves against the library and loads
        // paused at the saved position. A track gone from the library
        // resolves to nothing and the start stays cold. New Window opens
        // idle; its player is its own.
        let settings = Settings::load();
        // The mini-player roles ride in the struct so the menubar never
        // reads the file per frame; captured before the layout field moves
        // out below.
        let primary_layout = settings.primary_layout.clone();
        let mini_layout = settings.mini_layout.clone();
        // Which named preset this window opens on: the persisted one on a
        // restore, the named preset a preset window built from, and nothing
        // for an empty window. A restore that falls back to the default
        // arrangement below still claims the saved name, which the next apply
        // or save corrects.
        let active_layout = match &start {
            WorkspaceStart::Restore => settings.active_layout.clone(),
            WorkspaceStart::Preset(name) => Some(name.clone()),
            WorkspaceStart::Empty => None,
        };
        // The first window to open is the primary: it restores the last track
        // and owns the OS media service. The global is still empty here; this
        // window joins it below, so a later New Window reads false.
        let is_primary = cx.default_global::<WorkspaceWindows>().0.is_empty();
        // An adopted player is already where the user left it, often
        // playing; the launch restore would yank it back to the saved spot.
        if settings.restore_last_track && is_primary && !adopted {
            // Prefer the whole queue: resolve each id back to a path, keeping
            // the explicit flags parallel and realigning the cursor past any
            // entry whose file has left the library. An older file with only
            // last_track falls through to the single-track restore.
            let queue = settings.last_queue.as_ref().and_then(|q| {
                let library = state.library.read(cx);
                let mut paths = Vec::with_capacity(q.entries.len());
                let mut explicit = Vec::with_capacity(q.entries.len());
                let mut cursor = 0;
                for (i, entry) in q.entries.iter().enumerate() {
                    let path = library
                        .paths_for(&[entry.id])
                        .ok()
                        .and_then(|mut paths| paths.pop());
                    if let Some(path) = path {
                        if i <= q.cursor {
                            cursor = paths.len();
                        }
                        paths.push(path);
                        explicit.push(entry.explicit);
                    }
                }
                (!paths.is_empty()).then_some((paths, explicit, cursor, q.position_secs))
            });
            if let Some((paths, explicit, cursor, position_secs)) = queue {
                state.player.update(cx, |player, cx| {
                    player.restore_queue(paths, explicit, cursor, position_secs, cx)
                });
            } else if let Some(last) = settings.last_track {
                let path = state
                    .library
                    .read(cx)
                    .paths_for(&[last.id])
                    .ok()
                    .and_then(|mut paths| paths.pop());
                if let Some(path) = path {
                    state.player.update(cx, |player, cx| {
                        player.restore(path, last.position_secs, cx)
                    });
                }
            }
        }

        // A first launch (no settings file yet) gets the welcome window
        // over this primary workspace. Deferred through a spawn: this
        // constructor runs inside the window's own open, no place to open
        // another.
        if is_primary && settings::first_run() {
            cx.spawn(async move |this, cx| {
                this.update(cx, |this, cx| {
                    crate::startup::welcome_window::open(this.state.clone(), cx);
                })
                .ok();
            })
            .detach();
        }

        let dock = cx.new(|cx| DockArea::new("rox", Some(LAYOUT_VERSION), window, cx));
        let weak_dock = dock.downgrade();

        register_panels(&state, cx.entity().downgrade(), cx);

        // Where the dock starts, by how the window was opened: launch and
        // plain New Window restore the saved working layout, a preset window
        // builds from that named dump, an empty window skips straight to the
        // blank fallback below. A dump it can't trust (wrong version, no
        // stack root) falls through the same as none.
        let source = match &start {
            WorkspaceStart::Restore => settings.layout.clone(),
            WorkspaceStart::Preset(name) => {
                crate::settings::layouts::resolve(&settings, name).map(|preset| preset.dump)
            }
            WorkspaceStart::Empty => None,
        };
        let restored = source
            .and_then(|value| serde_json::from_value::<DockAreaState>(value).ok())
            .filter(|dump| {
                dump.version == Some(LAYOUT_VERSION)
                    && matches!(dump.center.info, PanelInfo::Stack { .. })
            })
            .map(|dump| dump.center.to_item(weak_dock.clone(), window, cx));

        let (center, stack, center_tabs, bottom_stack) = match restored {
            Some(item) => {
                let (stack, tabs, bottom) = layout_views(&item);
                // The preferred add targets may not survive a rearranged
                // layout; fresh detached entities take their place, and the
                // add paths attach them back into the tree on first use.
                let tabs = tabs.unwrap_or_else(|| tabs_item(Vec::new(), &weak_dock, window, cx).1);
                let bottom = bottom
                    .unwrap_or_else(|| cx.new(|cx| StackPanel::new(Axis::Horizontal, window, cx)));
                (item, stack, tabs, bottom)
            }
            // An empty window starts blank; everything else that has no
            // trustworthy dump falls back to the default arrangement.
            None if matches!(start, WorkspaceStart::Empty) => empty_layout(&weak_dock, window, cx),
            None => default_layout(&state, &weak_dock, window, cx),
        };

        // Save the layout when it settles after a change, and once more on
        // close, which also catches window moves and resizes: those emit no
        // dock events.
        let _layout_changed =
            cx.subscribe_in(&dock, window, |this, _, event: &DockEvent, window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    this.save_layout_soon(window, cx);
                }
            });
        // Observe rather than subscribe: scan progress ticks notify the
        // library without emitting Updated, and the badge needs those too.
        // A catalog change can also retag the playing track, so the title
        // re-derives on the next player tick.
        let _library_changed = cx.observe(&state.library, |this, _, cx| {
            this.titled_track = None;
            cx.notify();
        });
        let _player_changed = cx.observe_in(&state.player, window, |this, _, window, cx| {
            this.refresh_title(window, cx);
            this.publish_media(cx);
        });
        let _history_changed = cx.subscribe(&state.history, |this, _, event: &HistoryEvent, cx| {
            let HistoryEvent::Recorded { track_id } = *event;
            this.state
                .library
                .update(cx, |library, cx| library.record_play(track_id, cx));
        });
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let weak_self = cx.entity().downgrade();
        cx.default_global::<WorkspaceWindows>()
            .0
            .push((window.window_handle(), weak_self));
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            close_workspace_window(this.upgrade(), window, cx);
            true
        });

        // Zoom needs nothing from the workspace anymore: adding tab panels
        // to stacks makes the dock area subscribe them, and the zoomed
        // panel covers the whole dock area, which is the whole window under
        // the menu bar.
        dock.update(cx, |dock, cx| {
            dock.set_center(center, window, cx);
            dock.set_toggle_button_visible(false, cx);
            // A middle drag released outside the window pops the panel out
            // into its own OS window, same as the menu's Pop Out.
            let state = state.clone();
            dock.on_middle_drag_out(move |panel, _, _, cx| {
                crate::panel::pop_out_view(panel, state.clone(), cx);
            });
        });

        // The primary window registers the OS media service and drains its
        // key presses on an await loop. A press maps to a transport verb and
        // lands on the shared player; the publish keeps the widget's state in
        // step, both on launch and after each press.
        let media = if is_primary {
            MediaKeys::new(window)
        } else {
            None
        };
        let _media_events = media.as_ref().map(|keys| spawn_media_events(keys, cx));

        Workspace {
            open_menu: None,
            open_submenu: None,
            pointer_down: false,
            state,
            focus,
            dock,
            stack,
            center_tabs,
            bottom_stack,
            save_task: None,
            primary_layout,
            mini_layout,
            active_layout,
            empty_presets: None,
            empty_workspaces: None,
            layout_dialog: None,
            _layout_input: None,
            quick_play: None,
            _quick_play_dismissed: None,
            queue_modal: None,
            backdrop: WindowBackdrop::default(),
            titled_track: None,
            _layout_changed,
            _player_changed,
            _library_changed,
            _history_changed,
            _backdrop_changed,
            media,
            media_track: None,
            _media_events,
        }
    }

    /// The dock area, for the settings window's Layout page: the tree
    /// view walks it and export dumps it.
    pub fn dock(&self) -> &Entity<DockArea> {
        &self.dock
    }

    /// Swap in an imported layout dump: the launch restore's checks and
    /// rebuild, on a live workspace. A dump from another version or with
    /// a non-stack root is refused, same as a stale saved layout.
    pub fn apply_layout(
        &mut self,
        dump: DockAreaState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if dump.version != Some(LAYOUT_VERSION)
            || !matches!(dump.center.info, PanelInfo::Stack { .. })
        {
            return false;
        }
        // Fold the outgoing layout's live dock into its working copy before
        // the swap, so switching away keeps its unsaved tweaks. Synchronous
        // on purpose: the debounced save below would otherwise be the only
        // writer, and it dumps the incoming layout, not this one.
        self.stash_active_edits(window, cx);
        // The registry's builders capture one workspace's entities;
        // re-register so the rebuild lands on this one even after
        // another window registered over it.
        register_panels(&self.state, cx.entity().downgrade(), cx);
        let weak_dock = self.dock.downgrade();
        let item = dump.center.to_item(weak_dock.clone(), window, cx);
        let (stack, tabs, bottom) = layout_views(&item);
        self.stack = stack;
        self.center_tabs = tabs.unwrap_or_else(|| tabs_item(Vec::new(), &weak_dock, window, cx).1);
        self.bottom_stack =
            bottom.unwrap_or_else(|| cx.new(|cx| StackPanel::new(Axis::Horizontal, window, cx)));
        self.dock
            .update(cx, |dock, cx| dock.set_center(item, window, cx));
        self.save_layout_soon(window, cx);
        true
    }

    /// Rebuild the built-in default arrangement on a live workspace, the reset
    /// a workspace with no layout of its own applies. Same swap as
    /// [`apply_layout`], but the center comes from [`default_layout`] instead
    /// of a dump, so there is no registry rebuild to re-register for.
    fn apply_default_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Keep the outgoing layout's tweaks the same as any other swap.
        self.stash_active_edits(window, cx);
        let weak_dock = self.dock.downgrade();
        let (center, stack, center_tabs, bottom_stack) =
            default_layout(&self.state, &weak_dock, window, cx);
        self.stack = stack;
        self.center_tabs = center_tabs;
        self.bottom_stack = bottom_stack;
        self.dock
            .update(cx, |dock, cx| dock.set_center(center, window, cx));
        // The default build has no name of its own.
        self.set_active_layout(None);
        self.save_layout_soon(window, cx);
    }

    /// Record the named preset the window is now on and mirror it to settings,
    /// straight away rather than through the debounced layout save, so a
    /// workspace save right after an apply captures the right layout.
    fn set_active_layout(&mut self, name: Option<String>) {
        self.active_layout = name.clone();
        Settings::update(move |s| {
            // The layout in front of you keeps its live dock in
            // `settings.layout`, not the working-copy store, so clear any
            // stale copy as it becomes active.
            if let Some(name) = &name {
                s.layout_edits.remove(name.as_str());
            }
            s.active_layout = name;
        });
    }

    /// Fold this window's live dock into the active layout's working copy,
    /// the unsaved-tweaks store a later switch reads back. A window on an
    /// unnamed arrangement (the default build, a one-off import) has no name
    /// to key on, so this no-ops; its live dock rides in `settings.layout`
    /// for the launch restore instead.
    fn stash_active_edits(&self, window: &Window, cx: &mut Context<Self>) {
        let Some(name) = self.active_layout.clone() else {
            return;
        };
        let Ok(dump) = serde_json::to_value(self.dock.read(cx).dump(cx)) else {
            return;
        };
        // The current window size rides along, live off the window rather than
        // the debounced `settings.window`, so a resize made just before the
        // switch comes back with the layout.
        let size = Some(window_size(window));
        Settings::update(move |s| {
            s.layout_edits.insert(name, LayoutEdit { dump, size });
        });
    }

    /// Apply a named preset, user or shipped, by name. Returns false when
    /// no preset carries the name or its dump is one [`apply_layout`]
    /// refuses.
    pub fn apply_named_layout(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let settings = Settings::load();
        let Some(preset) = crate::settings::layouts::resolve(&settings, name) else {
            return false;
        };
        // Size to the preset by default; a working copy with its own size
        // overrides that below, so a resize made while editing comes back too.
        let mut size = preset.size;
        // Prefer the layout's working copy, the unsaved tweaks kept from the
        // last time it was in front of you, over the pristine preset. A
        // missing copy, or one an older version can no longer load, falls
        // back to the saved dump.
        let edited = settings.layout_edits.get(name).cloned();
        let mut applied = false;
        if let Some(edit) = &edited {
            if let Ok(dump) = serde_json::from_value::<DockAreaState>(edit.dump.clone()) {
                applied = self.apply_layout(dump, window, cx);
                // The working copy's own size wins when it carries one; a copy
                // from before sizes rode along keeps the preset's.
                if applied && edit.size.is_some() {
                    size = edit.size;
                }
            }
        }
        if !applied {
            let Ok(dump) = serde_json::from_value::<DockAreaState>(preset.dump) else {
                return false;
            };
            applied = self.apply_layout(dump, window, cx);
        }
        if !applied {
            return false;
        }
        self.set_active_layout(Some(name.to_string()));
        // Size the window to whichever source won above (the working copy's
        // size, or the preset's); neither carrying one leaves the window as is.
        if let Some(size) = size {
            resize_clamped(window, size);
        }
        // A programmatic resize only shows on the next drawn frame, and gpui
        // stops pumping frames for a window that is idle and not focused.
        // Applying from the settings window leaves this one in exactly that
        // state, so the resized dock sat stale until the compositor woke it
        // on the next focus, which is why it looked gone until you tabbed
        // back. Wake it and mark it dirty so the new layout draws now.
        window.activate_window();
        window.refresh();
        true
    }

    /// Apply a shipped or saved workspace to this window: the whole look
    /// through the shared path, then this window's dock swaps to the bundle's
    /// primary layout, or resets to the default arrangement when the bundle
    /// carries no layout. The empty launcher's way to start from a vendored
    /// look; a blank window has nothing to replace, so it acts straight off
    /// the click with no confirm. The settings window's apply lands here
    /// too, so both entry points share one flow.
    pub(crate) fn apply_workspace(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bundle) = crate::workspaces::resolve(&Settings::load(), name) else {
            return;
        };
        crate::workspaces::apply_look(&bundle, cx);
        // A whole-look swap drops the previous layout's unsaved edits along
        // with the rest of the old look (apply_look cleared the store); forget
        // the old active name too, so the apply below doesn't stash a stale
        // copy back into the freshly cleared store.
        self.active_layout = None;
        // The mini roles are cached off the file for the menubar; apply_look
        // already persisted them, this just moves the live copy.
        self.primary_layout = bundle.primary_layout.clone();
        self.mini_layout = bundle.mini_layout.clone();
        // The bundle's primary layout fills the window. A bundle without one
        // (or whose named layout no longer resolves) resets to the built-in
        // default arrangement rather than leaving the previous workspace's
        // dock in place, since applying a workspace replaces the look wholesale.
        let applied = bundle
            .primary_layout
            .clone()
            .is_some_and(|primary| self.apply_named_layout(&primary, window, cx));
        if !applied {
            self.apply_default_layout(window, cx);
        }
        cx.notify();
    }

    /// The mini-player button's config, so the settings window can push a
    /// role change back and the menubar picks it up without a file read.
    pub fn set_mini_roles(&mut self, primary: Option<String>, mini: Option<String>) {
        self.primary_layout = primary;
        self.mini_layout = mini;
    }

    /// Whether a mini layout is assigned, the gate every mini toggle
    /// shows behind.
    pub(crate) fn mini_assigned(&self) -> bool {
        self.mini_layout.is_some()
    }

    /// Whether the window is on the mini preset, the side of the toggle that
    /// decides the glyph and which way the next click goes. Falls out of the
    /// active layout rather than a separate flag, so it is always in step with
    /// what is actually showing.
    pub(crate) fn on_mini(&self) -> bool {
        self.mini_layout.is_some() && self.active_layout == self.mini_layout
    }

    /// A layouts-flyout pick, shared with the menu panel: open a preset
    /// window, or stage the overwrite/apply confirm dialog.
    pub(crate) fn run_layout(
        &mut self,
        name: String,
        target: LayoutTarget,
        cx: &mut Context<Self>,
    ) {
        match target {
            LayoutTarget::NewWindow => crate::open_workspace_with(WorkspaceStart::Preset(name), cx),
            LayoutTarget::Overwrite => {
                self.layout_dialog = Some(LayoutDialog::ConfirmOverwrite(name));
                cx.notify();
            }
            LayoutTarget::Apply => {
                self.layout_dialog = Some(LayoutDialog::ConfirmApply(name));
                cx.notify();
            }
        }
    }

    /// A workspaces-flyout pick, shared with the menu panel: stage the
    /// overwrite or apply confirm, since either replaces a whole look
    /// wholesale.
    pub(crate) fn run_workspace(
        &mut self,
        name: String,
        target: WorkspaceTarget,
        cx: &mut Context<Self>,
    ) {
        self.layout_dialog = Some(match target {
            WorkspaceTarget::Overwrite => LayoutDialog::ConfirmOverwriteWorkspace(name),
            WorkspaceTarget::Apply => LayoutDialog::ConfirmApplyWorkspace(name),
        });
        cx.notify();
    }

    /// Open the save-workspace dialog: a focused name field that Enter or the
    /// button commits into a bundle of the current look.
    pub(crate) fn open_save_workspace_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name"));
        self._layout_input =
            Some(
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.commit_save_workspace(window, cx);
                    }
                }),
            );
        window.focus(&input.focus_handle(cx));
        self.layout_dialog = Some(LayoutDialog::SaveWorkspace(input));
        cx.notify();
    }

    /// Save the current look under the dialog's name as a new workspace. An
    /// empty name waits; a name already in use routes through the overwrite
    /// confirm, matching the settings window's Save Current.
    fn commit_save_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(LayoutDialog::SaveWorkspace(input)) = &self.layout_dialog else {
            return;
        };
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        // Flush the live dock first. Panel config like the library's column
        // arrangement only reaches the settings file on the next layout dump,
        // so without this the bundle would capture whatever's stale on disk.
        self.persist(window, cx);
        if Settings::load().workspaces.iter().any(|w| w.name == name) {
            self.layout_dialog = Some(LayoutDialog::ConfirmOverwriteWorkspace(name));
            self._layout_input = None;
            cx.notify();
            return;
        }
        let bundle = WorkspaceBundle::from_settings(name, &Settings::load());
        Settings::update(move |s| s.workspaces.push(bundle));
        self.close_layout_dialog(window, cx);
    }

    /// Replace the pending workspace with the current look, the confirm
    /// dialog's yes. A shipped bundle has no entry to edit, so this saves a
    /// user bundle of the same name, which shadows it everywhere.
    fn overwrite_workspace_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmOverwriteWorkspace(name)) => name.clone(),
            _ => return,
        };
        // Flush the live dock so the overwrite captures current panel config,
        // not the stale disk copy. See commit_save_workspace.
        self.persist(window, cx);
        let bundle = WorkspaceBundle::from_settings(name.clone(), &Settings::load());
        Settings::update(move |s| {
            if let Some(existing) = s.workspaces.iter_mut().find(|w| w.name == name) {
                *existing = bundle;
            } else {
                s.workspaces.push(bundle);
            }
        });
        self.close_layout_dialog(window, cx);
    }

    /// Apply the pending workspace to this window, the confirm dialog's yes.
    fn apply_workspace_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmApplyWorkspace(name)) => name.clone(),
            _ => return,
        };
        self.apply_workspace(&name, window, cx);
        self.close_layout_dialog(window, cx);
    }

    /// Pick a workspace file and add it to the collection, the settings
    /// window's Import path from the menu.
    fn import_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Some(bundle) = crate::workspaces::read_bundle(&path, &Settings::load()) else {
                return;
            };
            Settings::update(move |s| s.workspaces.push(bundle));
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Toggle the mini layout: on the mini preset the click goes back to the
    /// primary, on anything else it goes to the mini preset. The named preset
    /// is the whole story now, so there is no stash to restore and no separate
    /// flag to flip; whichever side we land on becomes the active layout, and
    /// the glyph follows. A missing target (no primary to return to, no mini
    /// to enter) leaves the dock where it is.
    pub(crate) fn toggle_mini(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.on_mini() {
            self.primary_layout.clone()
        } else {
            self.mini_layout.clone()
        };
        let Some(name) = target else {
            return;
        };
        if self.apply_named_layout(&name, window, cx) {
            cx.notify();
        }
    }

    /// Open the save dialog: a focused name field that Enter or the button
    /// commits into a new preset.
    pub(crate) fn open_save_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Layout name"));
        self._layout_input =
            Some(
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.commit_save(window, cx);
                    }
                }),
            );
        window.focus(&input.focus_handle(cx));
        self.layout_dialog = Some(LayoutDialog::Save(input));
        cx.notify();
    }

    /// Save the current arrangement under the dialog's name, a new preset or
    /// an update to one that already carries the name. An empty name waits.
    fn commit_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(LayoutDialog::Save(input)) = &self.layout_dialog else {
            return;
        };
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let Ok(dump) = serde_json::to_value(self.dock.read(cx).dump(cx)) else {
            return;
        };
        let size = Some(window_size(window));
        Settings::update(move |s| {
            // Committing the edits clears the working copy; the saved preset
            // is the state now.
            s.layout_edits.remove(name.as_str());
            if let Some(existing) = s.layouts.iter_mut().find(|l| l.name == name) {
                existing.dump = dump;
                existing.size = size;
            } else {
                s.layouts.push(NamedLayout { name, dump, size });
            }
        });
        self.close_layout_dialog(window, cx);
    }

    /// Replace a preset with the current arrangement and window size. A
    /// shipped preset has no entry to edit, so this saves a user preset of
    /// the same name, which shadows it everywhere presets resolve.
    fn overwrite_layout(&mut self, name: &str, window: &Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        let Ok(dump) = serde_json::to_value(self.dock.read(cx).dump(cx)) else {
            return;
        };
        let size = Some(window_size(window));
        Settings::update(move |s| {
            // Overwriting is a save under the pending name; the working copy
            // it replaces is now the saved preset.
            s.layout_edits.remove(name.as_str());
            if let Some(existing) = s.layouts.iter_mut().find(|l| l.name == name) {
                existing.dump = dump;
                existing.size = size;
            } else {
                s.layouts.push(NamedLayout { name, dump, size });
            }
        });
        cx.notify();
    }

    /// Overwrite the pending preset with the current arrangement, the
    /// confirm dialog's yes.
    fn overwrite_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmOverwrite(name)) => name.clone(),
            _ => return,
        };
        self.overwrite_layout(&name, window, cx);
        self.close_layout_dialog(window, cx);
    }

    /// Apply the pending preset to this window, the confirm dialog's yes.
    fn apply_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmApply(name)) => name.clone(),
            _ => return,
        };
        self.apply_named_layout(&name, window, cx);
        self.close_layout_dialog(window, cx);
    }

    /// Drop the layout dialog and hand focus back to the workspace so the
    /// playback keys keep working.
    fn close_layout_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.layout_dialog = None;
        self._layout_input = None;
        window.focus(&self.focus);
        cx.notify();
    }

    /// Open the quick-play modal, or close it when it is already up. The
    /// modal takes the keyboard through its search input; dismissal hands
    /// focus back to the workspace so the playback keys keep working.
    fn toggle_quick_play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_play.take().is_some() {
            self._quick_play_dismissed = None;
            window.focus(&self.focus);
            cx.notify();
            return;
        }
        let modal = cx.new(|cx| QuickPlay::new(self.state.clone(), window, cx));
        self._quick_play_dismissed =
            Some(
                cx.subscribe_in(&modal, window, |this, _, _: &DismissEvent, window, cx| {
                    this.quick_play = None;
                    this._quick_play_dismissed = None;
                    window.focus(&this.focus);
                    cx.notify();
                }),
            );
        window.focus(&modal.read(cx).focus_handle(cx));
        self.quick_play = Some(modal);
        cx.notify();
    }

    /// Open the queue modal, or close it when it is already up. The queue
    /// widget calls this when no queue panel is docked, so a click always
    /// lands somewhere. A fresh queue panel each open, dropped on close; its
    /// view (columns, headings) rides settings, so it comes back the way it
    /// was left.
    pub(crate) fn toggle_queue_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue_modal.take().is_some() {
            window.focus(&self.focus);
            cx.notify();
            return;
        }
        let modal = cx.new(|cx| QueuePanel::windowed(self.state.clone(), window, cx));
        window.focus(&modal.read(cx).focus_handle(cx));
        self.queue_modal = Some(modal);
        cx.notify();
    }

    /// Drop the queue modal and hand focus back to the workspace, so the
    /// playback keys keep working. The scrim's click-out and the card's
    /// Escape both land here.
    fn close_queue_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.queue_modal = None;
        window.focus(&self.focus);
        cx.notify();
    }

    /// Keep the window title on the playing track: "artist - title - rox"
    /// while something plays, the plain app name otherwise. Untagged files
    /// fall back to their file name, same as the track info readout.
    fn refresh_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if path == self.titled_track {
            return;
        }
        let title = match &path {
            Some(path) => {
                let meta = self.state.library.read(cx).meta_for(path);
                let track = meta.as_ref().map(|m| m.title.clone()).unwrap_or_else(|| {
                    path.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                });
                match meta.map(|m| m.artist).filter(|a| !a.is_empty()) {
                    Some(artist) => format!("{artist} - {track} - rox"),
                    None => format!("{track} - rox"),
                }
            }
            None => "rox".into(),
        };
        window.set_window_title(&title);
        self.titled_track = path;
    }

    /// Route OS-handed files into the shared player. The launch path
    /// (`rox song.flac` and the .desktop actions) lands here after the
    /// window's player exists; paths are already filtered to decodable audio.
    /// Play replaces the restored session so double-clicking a file starts it;
    /// enqueue appends. The player is path-based, so files outside the library
    /// play fine.
    pub fn open_paths(
        &mut self,
        mode: rox_library::open_files::LaunchMode,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        self.state.player.update(cx, |player, cx| match mode {
            rox_library::open_files::LaunchMode::Play => player.play(paths, cx),
            rox_library::open_files::LaunchMode::Enqueue => player.enqueue(paths, cx),
        });
    }

    /// Play files or tracks dropped onto the window body now, filtered to
    /// decodable audio. A drop onto the window reads as "play this", so it
    /// splices in right after the current track and jumps to it, keeping the
    /// rest of the queue behind it. Dropping onto the queue panel adds to the
    /// queue instead; that panel's own handler catches the drop first. An OS
    /// file open (the .desktop default) still replaces the session, that path
    /// runs through open_paths, not here.
    fn play_dropped(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let paths = rox_library::open_files::resolve_audio_paths(paths);
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play_now(paths, cx));
    }

    /// Add dropped files or tracks to the up-next queue, filtered to decodable
    /// audio. The Add to queue drop zone routes here.
    fn queue_dropped(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let paths = rox_library::open_files::resolve_audio_paths(paths);
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.enqueue(paths, cx));
    }

    /// The Play now / Add to queue drop zones, shown only while an audio
    /// payload is dragged: a file from the OS (ExternalPaths) or a track from
    /// the library (PlayDrag). Other drags (panel docking, queue reorder)
    /// leave them hidden. Rendered as the top layer so the drop always lands
    /// here - an occluded window-root target misses it because the panels
    /// block the hit test.
    fn drop_zones_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !cx.active_drag_is::<ExternalPaths>() && !cx.active_drag_is::<PlayDrag>() {
            return None;
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .p(tokens::SPACE_MD)
                .bg(rgba(0x00000055))
                .child(self.drop_zone("Play now", icons::PLAY, true, cx))
                .child(self.drop_zone("Add to queue", icons::LIST_MUSIC, false, cx))
                .into_any_element(),
        )
    }

    /// One drop zone card. `play_now` true plays the drop after the current
    /// track and jumps to it; false appends it to the queue. Both accept a
    /// file from the OS and a track dragged from the library.
    fn drop_zone(
        &self,
        label: &'static str,
        icon: &'static str,
        play_now: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let card = div()
            .flex_1()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .rounded(tokens::RADIUS)
            .border_2()
            .border_color(palette::border_light())
            .bg(palette::bg_menu_opaque())
            .text_color(palette::text_muted())
            .child(Icon::default().path(icon))
            .child(div().text_lg().child(label))
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style
                    .border_color(palette::accent())
                    .bg(palette::bg_control_hover_opaque())
            })
            .drag_over::<PlayDrag>(|style, _, _, _| {
                style
                    .border_color(palette::accent())
                    .bg(palette::bg_control_hover_opaque())
            });
        if play_now {
            card.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.play_dropped(paths.paths().to_vec(), cx);
            }))
            .on_drop(cx.listener(|this, drag: &PlayDrag, _, cx| {
                this.play_dropped(drag.paths.to_vec(), cx);
            }))
        } else {
            card.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.queue_dropped(paths.paths().to_vec(), cx);
            }))
            .on_drop(cx.listener(|this, drag: &PlayDrag, _, cx| {
                this.queue_dropped(drag.paths.to_vec(), cx);
            }))
        }
    }

    /// Apply one media-key press to the shared player. Play and Pause act on
    /// the edge so the OS buttons never flip a state that's already right;
    /// Toggle is the bare play/pause key.
    fn apply_media(&mut self, cmd: MediaCommand, cx: &mut Context<Self>) {
        self.state.player.update(cx, |player, cx| match cmd {
            MediaCommand::Toggle => player.toggle_pause(),
            MediaCommand::Play => {
                if !player.is_playing() {
                    player.toggle_pause();
                }
            }
            MediaCommand::Pause => {
                if player.is_playing() {
                    player.toggle_pause();
                }
            }
            MediaCommand::Next => player.next(),
            MediaCommand::Prev => player.prev(),
            MediaCommand::Stop => player.stop(cx),
            MediaCommand::SeekBy(delta) => player.seek_by(delta),
            MediaCommand::SeekTo(secs) => player.seek_to(secs),
        });
    }

    /// Register the OS media service on this window and start draining its key
    /// presses. The hand-off target when the window that owned the service
    /// closes with this one still open. The D-Bus name is per-process, so the
    /// old owner has to release it first.
    fn install_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let media = MediaKeys::new(window);
        self._media_events = media.as_ref().map(|keys| spawn_media_events(keys, cx));
        self.media = media;
    }

    /// Tear down the OS media service this window owned, freeing the per-process
    /// D-Bus name for a survivor to claim. Returns whether it held one.
    fn release_media(&mut self) -> bool {
        self._media_events = None;
        self.media.take().is_some()
    }

    /// Push the now-playing track and play state out to the media widget. A
    /// no-op off the primary window (no service there); the tag resolve only
    /// runs when the track turns over, the play-state push is gated in
    /// [`MediaKeys`], so this is cheap to call on every player notify.
    fn publish_media(&mut self, cx: &mut Context<Self>) {
        if self.media.is_none() {
            return;
        }
        let now = self.state.player.read(cx).now_playing();
        let playing = self.state.player.read(cx).is_playing();
        let path = now.as_ref().map(|now| now.path.clone());
        if path != self.media_track {
            self.media_track = path.clone();
            let meta = now.as_ref().map(|now| self.now_playing_meta(now, cx));
            if let Some(media) = self.media.as_mut() {
                media.set_track(meta);
            }
            self.publish_cover(path.clone(), cx);
        }
        let position = now
            .as_ref()
            .map(|now| Duration::from_secs_f64(now.position_secs.max(0.0)));
        if let Some(media) = self.media.as_mut() {
            media.set_playing(path.is_some(), playing, position);
        }
        // The tray's Play/Pause label rides the same choke point, gated the
        // same way so player notifies don't turn into D-Bus writes.
        tray::set_playing(path.is_some(), playing, cx);
    }

    /// Resolve the current track's cover off the UI thread and hand it to the
    /// media widget when it lands. `set_track` already cleared the old cover
    /// with the text, so a track with no art (or `None`, nothing playing)
    /// needs no further work. The result is dropped when the track has moved
    /// on by the time the read finishes, so a late cover never lands on the
    /// wrong track.
    fn publish_cover(&mut self, track: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(track) = track else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let resolved = track.clone();
            let cover = cx
                .background_executor()
                .spawn(async move {
                    rox_library::art::cover_art(&resolved).and_then(|(bytes, mime)| {
                        crate::integrations::media_controls::cache_now_playing_art(&resolved, &bytes, &mime)
                    })
                })
                .await;
            this.update(cx, |this, _| {
                if this.media_track.as_deref() != Some(track.as_path()) {
                    return;
                }
                if let Some(media) = this.media.as_mut() {
                    media.set_cover(cover);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Resolve a playing track to the tags the widget shows, off the library.
    /// An unknown file falls back to its filename for the title, empty for
    /// the rest, so the widget never shows a blank card.
    fn now_playing_meta(&self, now: &NowPlaying, cx: &App) -> NowPlayingMeta {
        let tags = self.state.library.read(cx).meta_for(&now.path);
        let title = tags
            .as_ref()
            .map(|m| m.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                now.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        NowPlayingMeta {
            title,
            artist: tags.as_ref().map(|m| m.artist.clone()).unwrap_or_default(),
            album: tags.map(|m| m.album).unwrap_or_default(),
            duration: now
                .duration_secs
                .filter(|d| *d > 0.0)
                .map(Duration::from_secs_f64),
        }
    }

    /// Debounced persist: wait out [`SAVE_DEBOUNCE`] of quiet, then dump.
    /// Replacing the task cancels the previous timer.
    fn save_layout_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            this.update_in(cx, |this, window, cx| this.persist(window, cx))
                .ok();
        }));
    }

    /// Dump the dock layout and the window frame into the settings file.
    /// With several windows open the last writer wins; the file records the
    /// layout most recently touched.
    pub(crate) fn persist(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.save_task = None;
        let layout = serde_json::to_value(self.dock.read(cx).dump(cx)).ok();
        let bounds = window.window_bounds();
        let frame = bounds.get_bounds();
        let window_state = WindowState {
            x: frame.origin.x.into(),
            y: frame.origin.y.into(),
            width: frame.size.width.into(),
            height: frame.size.height.into(),
            maximized: matches!(bounds, WindowBounds::Maximized(_)),
        };
        // The playing track rides along as its library id, for the launch
        // restore. Nothing playing, or a file outside the library, clears
        // it: the next launch starts cold.
        let library = self.state.library.read(cx);
        let last_track = self.state.player.read(cx).now_playing().and_then(|now| {
            let id = library.id_for(&now.path)?;
            Some(LastTrack {
                id,
                position_secs: now.position_secs,
            })
        });
        // The whole queue rides along too, as library ids so it survives path
        // changes, keeping each entry's explicit flag and the audible cursor.
        // A file outside the library drops from the order; the cursor tracks
        // the last kept entry at or before it so it stays on the playing
        // track. Everything gone (or nothing playing) clears it and the
        // single-track fallback above carries the restore.
        let last_queue = self.state.player.read(cx).queue_state().and_then(
            |(entries, cursor, position_secs)| {
                let mut tracks = Vec::with_capacity(entries.len());
                let mut new_cursor = 0;
                for (i, (path, explicit)) in entries.iter().enumerate() {
                    if let Some(id) = library.id_for(path) {
                        if i <= cursor {
                            new_cursor = tracks.len();
                        }
                        tracks.push(QueuedTrack {
                            id,
                            explicit: *explicit,
                        });
                    }
                }
                (!tracks.is_empty()).then_some(QueueState {
                    entries: tracks,
                    cursor: new_cursor,
                    position_secs,
                })
            },
        );
        Settings::update(move |s| {
            s.layout = layout;
            s.window = Some(window_state);
            s.last_track = last_track;
            s.last_queue = last_queue;
        });
    }

    fn add_center(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The dock's own add-to-center always targets the initial tabs item,
        // but drags and closes can empty that tab panel out of the tree.
        // Add to it while it still shows, otherwise to the newest live tab
        // panel, and failing both put the original one back at the top of
        // the root stack, above the transport row.
        let tabs = if self.center_tabs.read(cx).visible(cx) {
            self.center_tabs.clone()
        } else if let Some(tabs) = self.state.tab_hosts.read(cx).last_live(cx) {
            tabs
        } else {
            let tabs_view: Arc<dyn PanelView> = Arc::new(self.center_tabs.clone());
            let weak_dock = self.dock.downgrade();
            self.stack.update(cx, |stack, cx| {
                stack.insert_panel_before(tabs_view, 0, None, weak_dock, window, cx);
            });
            self.center_tabs.clone()
        };
        tabs.update(cx, |tabs, cx| tabs.add_panel(panel, window, cx));
    }

    /// New audio and transport panels join the transport row as their own
    /// tab group at the end - a new group rather than a new tab, so they
    /// sit next to the transport pieces instead of hiding one. The library
    /// stays a center panel: it wants the tall area, and keeping additions
    /// on the center path preserves the recovery route when every center
    /// panel has been closed or popped out.
    fn add_bottom(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak_dock = self.dock.downgrade();
        // The row removes itself from the tree when its last group closes,
        // so put it back at the bottom of the root stack first. A no-op
        // while it is still attached: stacks skip panels they already hold.
        let row: Arc<dyn PanelView> = Arc::new(self.bottom_stack.clone());
        self.stack.update(cx, |stack, cx| {
            stack.add_panel(
                row,
                Some(px(TRANSPORT_ROW_H)),
                weak_dock.clone(),
                window,
                cx,
            );
        });
        let (_, tabs) = tabs_item(vec![panel], &weak_dock, window, cx);
        self.bottom_stack.update(cx, |stack, cx| {
            stack.add_panel(Arc::new(tabs), None, weak_dock, window, cx);
        });
    }

    /// The search bar joins the top of the root stack as its own thin tab
    /// group across the whole window, above the center panels. A search bar
    /// wants to be a strip, not a tall tile, so it goes in sized to just its
    /// input row rather than splitting a center panel's height.
    fn add_top(&mut self, panel: Arc<dyn PanelView>, window: &mut Window, cx: &mut Context<Self>) {
        let weak_dock = self.dock.downgrade();
        let (_, tabs) = tabs_item(vec![panel], &weak_dock, window, cx);
        self.stack.update(cx, |stack, cx| {
            stack.insert_panel_before(
                Arc::new(tabs),
                0,
                Some(px(SEARCH_BAR_H)),
                weak_dock,
                window,
                cx,
            );
        });
    }

    /// Whether the dock shows no panels at all - every stack walked down
    /// to tab groups and all of them empty. The face an Empty Window
    /// opens with, or closing the last panel leaves behind.
    fn dock_is_empty(&self, cx: &App) -> bool {
        fn node_empty(node: &Arc<dyn PanelView>, cx: &App) -> bool {
            let view = node.view();
            if let Ok(stack) = view.clone().downcast::<StackPanel>() {
                let children = stack.read(cx).panels().to_vec();
                return children.iter().all(|child| node_empty(child, cx));
            }
            if let Ok(tabs) = view.downcast::<TabPanel>() {
                return tabs.read(cx).panels().is_empty();
            }
            false
        }
        let root = self.dock.read(cx).items().view();
        node_empty(&root, cx)
    }

    /// The empty dock's launcher, floated mid-window: an empty tab group
    /// renders to nothing, so without this a blank window gives no way in.
    /// The rox mark heads it, then the whole looks - shipped and saved
    /// workspaces and layout presets - lead, with the panel catalog under
    /// a rule below. A layout or workspace applies straight away, no
    /// confirm, because a blank window has nothing to replace. The look
    /// pickers only belong here, in the empty state: applying one replaces
    /// the whole window, so they never show once panels are in.
    fn empty_hint(&mut self, cx: &mut Context<Self>) -> Div {
        // Read once when the launcher shows; the render loop must not
        // touch the settings file per frame.
        let presets = self
            .empty_presets
            .get_or_insert_with(|| {
                crate::settings::layouts::all(&Settings::load())
                    .into_iter()
                    .map(|preset| preset.name)
                    .collect()
            })
            .clone();
        let workspaces = self
            .empty_workspaces
            .get_or_insert_with(|| {
                crate::workspaces::all(&Settings::load())
                    .into_iter()
                    .map(|entry| entry.bundle.name)
                    .collect()
            })
            .clone();
        let has_presets = !presets.is_empty();
        let has_workspaces = !workspaces.is_empty();

        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(tokens::SPACE_MD)
                    .max_w(px(600.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .pb(tokens::SPACE_XS)
                            .child(
                                svg()
                                    .path(icons::LOGO)
                                    .size(px(40.))
                                    .text_color(palette::text()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(px(2.))
                                    .child(
                                        div().text_color(palette::text()).child("An empty window"),
                                    )
                                    .child(
                                        div().text_xs().text_color(palette::text_muted()).child(
                                            "Start from a workspace or layout, or add a panel",
                                        ),
                                    ),
                            ),
                    )
                    // The whole looks lead: from a blank window, dressing
                    // it in a saved arrangement is the bigger move than
                    // adding one panel at a time. They read as headings,
                    // not group labels, because picking one replaces the
                    // whole window.
                    .when(has_workspaces, |d| {
                        d.child(launcher_section(
                            "Workspaces",
                            true,
                            workspaces.into_iter().map(|name| {
                                let apply = name.clone();
                                launcher_tile(
                                    SharedString::from(name),
                                    icons::APP_WINDOW,
                                    cx.listener(move |this, _, window, cx| {
                                        this.apply_workspace(&apply, window, cx);
                                    }),
                                )
                            }),
                        ))
                    })
                    .when(has_presets, |d| {
                        d.child(launcher_section(
                            "Layouts",
                            true,
                            presets.into_iter().map(|name| {
                                let apply = name.clone();
                                launcher_tile(
                                    SharedString::from(name),
                                    icons::LAYOUT_DASHBOARD,
                                    cx.listener(move |this, _, window, cx| {
                                        this.apply_named_layout(&apply, window, cx);
                                    }),
                                )
                            }),
                        ))
                    })
                    // The panel catalog sits below the rule: the piece-by-
                    // piece way in, one titled section per group; the bare
                    // center run reads under a plain "Panels".
                    .when(has_workspaces || has_presets, |d| {
                        d.child(launcher_divider())
                    })
                    .children(catalog::CATALOG.iter().map(|section| {
                        let tiles = section.panels.iter().map(|def| {
                            launcher_tile(
                                def.label,
                                def.icon,
                                cx.listener(move |this, _, window, cx| {
                                    this.run(MenuAction::OpenPanel(def), window, cx);
                                }),
                            )
                        });
                        launcher_section(
                            section.group.map(|(label, _)| label).unwrap_or("Panels"),
                            false,
                            tiles,
                        )
                    })),
            )
    }


    /// The mini-player toggle, at the menubar's left edge before the menus.
    /// Shows whenever a mini layout is assigned; the glyph flips to say
    /// which way the next click goes. Built inline rather than through
    /// [`panel::icon_control`] because the swap needs the window the icon
    /// helper doesn't pass, and it reads like a menu button beside them.
    fn mini_button(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        self.mini_layout.as_ref()?;
        let icon = if self.on_mini() {
            icons::MAXIMIZE
        } else {
            icons::MINIMIZE
        };
        Some(
            div()
                .h_full()
                .px(tokens::SPACE_MD)
                .flex()
                .items_center()
                .cursor_pointer()
                .hover(|d| d.bg(palette::bg_menu_hover()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.toggle_mini(window, cx)),
                )
                .child(
                    svg()
                        .path(icon)
                        .size(px(14.))
                        .text_color(palette::text_muted()),
                ),
        )
    }

    /// The layout save/apply dialog, floated over the window on its own
    /// occluding layer. The save card carries the `SearchInput` key context
    /// so space and arrows type into the name field instead of driving
    /// playback, the search boxes' trick.
    fn layout_dialog_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.layout_dialog.as_ref()?;
        let card = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .w(px(320.))
            .p(tokens::SPACE_MD)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_menu_opaque())
            .border_1()
            .border_color(palette::border_light())
            .shadow_md()
            .occlude();
        let card = match dialog {
            LayoutDialog::Save(input) => card
                .key_context("SearchInput")
                .child(div().child("Save Layout"))
                .child(Input::new(input))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Save",
                            true,
                            cx.listener(|this, _, window, cx| this.commit_save(window, cx)),
                        )),
                ),
            LayoutDialog::ConfirmOverwrite(name) => card
                .child(div().child(SharedString::from(format!("Overwrite \"{name}\"?"))))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("This replaces the saved layout with the current one."),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Overwrite",
                            true,
                            cx.listener(|this, _, window, cx| this.overwrite_confirmed(window, cx)),
                        )),
                ),
            LayoutDialog::ConfirmApply(name) => card
                .child(div().child(SharedString::from(format!("Apply \"{name}\"?"))))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("This replaces this window's current layout."),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Apply",
                            true,
                            cx.listener(|this, _, window, cx| this.apply_confirmed(window, cx)),
                        )),
                ),
            LayoutDialog::SaveWorkspace(input) => card
                .key_context("SearchInput")
                .child(div().child("Save Workspace"))
                .child(Input::new(input))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Save",
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.commit_save_workspace(window, cx)
                            }),
                        )),
                ),
            LayoutDialog::ConfirmOverwriteWorkspace(name) => card
                .child(div().child(SharedString::from(format!("Overwrite \"{name}\"?"))))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("This replaces the saved workspace with the current look."),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Overwrite",
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.overwrite_workspace_confirmed(window, cx)
                            }),
                        )),
                ),
            LayoutDialog::ConfirmApplyWorkspace(name) => card
                .child(div().child(SharedString::from(format!("Apply \"{name}\"?"))))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("This replaces the whole look - layouts, palette, appearance."),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Apply",
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.apply_workspace_confirmed(window, cx)
                            }),
                        )),
                ),
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000066))
                .child(card)
                .into_any_element(),
        )
    }

    /// The queue modal: the queue panel floated over the workspace on a
    /// dimming scrim. The card occludes, so a click on it stays on the queue;
    /// a click on the scrim outside it closes, as does Escape, which bubbles
    /// up from the focused queue panel (its own key handler leaves Escape
    /// alone). Sized fixed so the queue keeps a definite height off the dock.
    fn queue_modal_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let queue = self.queue_modal.clone()?;
        let card = div()
            .w(px(640.))
            .h(px(520.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(tokens::RADIUS)
            .bg(palette::bg_menu_opaque())
            .border_1()
            .border_color(palette::border_light())
            .shadow_md()
            .occlude()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_queue_modal(window, cx);
                }
            }))
            .child(queue);
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000066))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_queue_modal(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }

}

/// Resize the window to a preset's stored size, floored at the window
/// minimum. A bad or zero size in a preset would otherwise collapse the
/// window to nothing on a layout swap or mini toggle.
fn resize_clamped(window: &mut Window, size: LayoutSize) {
    window.resize(gpui::size(
        px(size.width).max(crate::MIN_WINDOW_SIZE.width),
        px(size.height).max(crate::MIN_WINDOW_SIZE.height),
    ));
}

/// The window's content size in logical pixels, for storing with a layout
/// preset. A maximized window reports its restore size, the size the preset
/// makes sense to reopen at.
fn window_size(window: &Window) -> LayoutSize {
    let size = window.window_bounds().get_bounds().size;
    LayoutSize {
        width: size.width.into(),
        height: size.height.into(),
    }
}

/// A muted heading over a divider, grouping the rows below it in a dropdown.
pub(crate) fn menu_section(label: &'static str) -> Div {
    div()
        .mt(tokens::SPACE_XS)
        .pt(tokens::SPACE_XS)
        .px(tokens::SPACE_MD)
        .border_t_1()
        .border_color(palette::border())
        .text_xs()
        .text_color(palette::text_muted())
        .child(label)
}

/// A launcher row: a centered wrap of tiles.
fn tile_row() -> Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(tokens::SPACE_SM)
}

/// A titled launcher block: a centered header over its wrap of tiles.
/// `prominent` reads the header as a heading in full-strength text for the
/// whole-look sections; the panel groups stay muted and small so the tiles
/// carry the weight.
fn launcher_section(
    header: impl Into<SharedString>,
    prominent: bool,
    tiles: impl IntoIterator<Item = Div>,
) -> Div {
    let header = div().child(header.into()).map(|d| {
        if prominent {
            d.text_color(palette::text())
        } else {
            d.text_xs().text_color(palette::text_muted())
        }
    });
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(header)
        .child(tile_row().children(tiles))
}

/// The rule between the panel catalog and the whole-look pickers, a short
/// centered hairline so the two halves read apart.
fn launcher_divider() -> Div {
    div()
        .w(px(220.))
        .h(px(1.))
        .my(tokens::SPACE_XS)
        .bg(palette::border())
}

/// A launcher tile: an icon-and-label chip that opens a panel or applies
/// a layout with one click.
fn launcher_tile(
    label: impl Into<SharedString>,
    icon: &'static str,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .bg(palette::bg_control())
        .hover(|d| d.bg(palette::bg_control_hover()))
        .on_mouse_down(MouseButton::Left, on_click)
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(
            svg()
                .path(icon)
                .size_3p5()
                .text_color(palette::text_muted()),
        )
        .child(label.into())
}

/// A dialog button: the primary one reads as a filled accent control, the
/// rest as plain controls.
fn dialog_button(
    label: &'static str,
    primary: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .map(|d| {
            if primary {
                d.bg(palette::accent())
                    .text_color(palette::text_on_accent())
                    .hover(|d| d.opacity(0.9))
            } else {
                d.bg(palette::bg_control())
                    .hover(|d| d.bg(palette::bg_control_hover()))
            }
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A hidden menubar comes back while alt is held, and stays while a
        // dropdown is open so releasing alt can't strand one barless.
        let menubar_hidden = settings::hide_menubar();
        // Alt reveals the hidden bar, but Alt+drag is the compositor's
        // window move/resize; suppress the reveal while a button is down so
        // the overlay never sits in front of the drag. An open menu keeps
        // it up regardless (that press landed on a menu, not a drag).
        let menubar_revealed =
            self.open_menu.is_some() || (window.modifiers().alt && !self.pointer_down);
        // Every panel in this window renders under its player's art tint,
        // and the window claims the one widget theme while it holds focus.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        // The launcher's preset cache lives only while the launcher shows;
        // dropping it here means the next empty state reads fresh names.
        let dock_empty = self.dock_is_empty(cx);
        if !dock_empty {
            self.empty_presets = None;
            self.empty_workspaces = None;
        }
        panel::window_body(player, || {
            div()
                .flex()
                .flex_col()
                .size_full()
                .track_focus(&self.focus)
                .key_context("Workspace")
                .on_action(cx.listener(|this, _: &TogglePlayback, _, cx| {
                    this.state
                        .player
                        .update(cx, |player, _| player.toggle_pause());
                }))
                .on_action(cx.listener(|this, _: &SeekBackward, _, cx| {
                    this.state
                        .player
                        .update(cx, |player, _| player.seek_by(-5.0));
                }))
                .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                    this.state
                        .player
                        .update(cx, |player, _| player.seek_by(5.0));
                }))
                .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                    crate::settings::window::open(
                        this.state.clone(),
                        cx.entity().downgrade(),
                        window.window_handle(),
                        this.dock.clone(),
                        cx,
                    );
                }))
                .on_action(cx.listener(|this, _: &OpenStats, _, cx| {
                    crate::stats_window::open(this.state.clone(), cx);
                }))
                .on_action(cx.listener(|this, _: &OpenQuickPlay, window, cx| {
                    this.toggle_quick_play(window, cx);
                }))
                .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                    this.dock.update(cx, |dock, cx| {
                        dock.focus_panel_named("search", window, cx);
                    });
                }))
                .on_action(cx.listener(|this, _: &ToggleZoom, window, cx| {
                    this.dock
                        .update(cx, |dock, cx| dock.toggle_zoom_active(window, cx));
                }))
                // Escape backs out of a zoomed panel. A raw listener, not a
                // binding: bindings win over key listeners, and the escape
                // ladders (search boxes, quick-play) live in listeners that
                // stop propagation - a binding here would steal their escape.
                // This runs last in the bubble, so it only sees what they let
                // through.
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key != "escape" || event.keystroke.modifiers.modified() {
                        return;
                    }
                    if this.layout_dialog.is_some() {
                        this.close_layout_dialog(window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    if this.dock.update(cx, |dock, cx| dock.zoom_out(window, cx)) {
                        cx.stop_propagation();
                    }
                }))
                // Quit bypasses the window close hook, so dump the layout and
                // frame here or a pending debounce and any window move since
                // the last save are lost.
                .on_action(cx.listener(|this, _: &Quit, window, cx| {
                    this.persist(window, cx);
                    cx.quit();
                }))
                // Alt reveals a hidden menubar, so modifier flips repaint. Gated
                // on the setting so the common case stays free of repaints.
                .on_modifiers_changed(cx.listener(|_, _, _, cx| {
                    if settings::hide_menubar() {
                        cx.notify();
                    }
                }))
                // Track the mouse button in the capture phase so the
                // alt-revealed bar can duck a window move/resize drag. Capture
                // beats the occluding overlay and any panel that eats the
                // press; only the alt-reveal path cares, so repaint just there.
                .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                    if !this.pointer_down {
                        this.pointer_down = true;
                        if settings::hide_menubar() && this.open_menu.is_none() {
                            cx.notify();
                        }
                    }
                }))
                .capture_any_mouse_up(cx.listener(|this, _, _, cx| {
                    if this.pointer_down {
                        this.pointer_down = false;
                        if settings::hide_menubar() && this.open_menu.is_none() {
                            cx.notify();
                        }
                    }
                }))
                // A compositor-driven Alt+drag can swallow the release, leaving
                // the flag stuck down. Any later move with no button held
                // reconciles it, so the next Alt press reveals the bar again.
                .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if this.pointer_down && event.pressed_button.is_none() {
                        this.pointer_down = false;
                        if settings::hide_menubar() && this.open_menu.is_none() {
                            cx.notify();
                        }
                    }
                }))
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                // The app font cascades from here into the menubar, dock, and
                // every panel; a panel's own font override layers over it in
                // the themed wrapper. None follows the platform default.
                .when_some(settings::app_font(), |d, font| d.font_family(font))
                // The backdrop paints first, under the menubar and dock; how
                // much shows through is the surfaces' call (ADR 10's strength
                // scalar).
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .when(!menubar_hidden, |d| d.child(self.menubar(cx)))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(self.dock.clone())
                        // The hint floats over the dock area only, so it
                        // never covers the menubar or the overlays below.
                        .when(dock_empty, |d| d.child(self.empty_hint(cx))),
                )
                // A hidden bar floats over the dock while revealed, so the
                // layout never shifts under it. After the dock child so it
                // paints on top; occlude keeps its clicks off what it covers.
                .when(menubar_hidden && menubar_revealed, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .occlude()
                            .child(self.menubar(cx)),
                    )
                })
                // The quick-play modal floats over everything on an occluding
                // layer, so a click outside it dismisses without also landing
                // on whatever sits underneath. Not deferred: it is the last
                // child so it already paints on top, and the search box's
                // suggestion popover defers itself - gpui panics on a
                // defer_draw inside a deferred draw.
                .when_some(self.quick_play.clone(), |d, modal| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .occlude()
                            .flex()
                            .flex_col()
                            .items_center()
                            .pt(px(96.))
                            .child(modal),
                    )
                })
                // The layout save/apply dialog floats over everything, same as
                // quick-play and for the same reasons: last child, not deferred.
                .children(self.layout_dialog_overlay(cx))
                // The queue modal floats the same way, last so it paints over
                // the dock.
                .children(self.queue_modal_overlay(cx))
                // The Play now / Add to queue drop zones. Last child so they
                // sit on top of every panel, which also makes them the topmost
                // hitbox: an occluded workspace-root drop target would miss the
                // drop entirely (panels block the hit test).
                .children(self.drop_zones_overlay(cx))
                .into_any_element()
        })
    }
}
