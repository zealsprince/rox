//! The main window: an in-window menubar over the dock area. GPUI only
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
    actions, deferred, div, prelude::*, px, svg, App, Axis, Context, Div, Entity, FocusHandle,
    FontFeatures, Global, KeyBinding, MouseButton, Subscription, Task, WeakEntity, Window,
    WindowBounds,
};
use rox_dock::{
    register_panel, DockArea, DockAreaState, DockEvent, DockItem, Panel as _, PanelInfo, PanelView,
    StackPanel, TabPanel,
};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, TabHosts};
use crate::panels::cover::{CoverArtPanel, CoverConfig};
use crate::panels::library::{Library, LibraryConfig, LibraryPanel};
use crate::panels::spectrum::{SpectrumConfig, SpectrumPanel};
use crate::panels::transport::{
    SeekConfig, SeekStripPanel, TrackInfoConfig, TrackInfoPanel, TransportConfig, TransportPanel,
    VolumeConfig, VolumePanel,
};
use crate::panels::waveform::WaveformPanel;
use crate::player::Player;
use crate::selection::Selection;
use crate::settings::{Settings, WindowState};

const MENU_BAR_H: f32 = 30.0;

/// How many workspace windows are open: counted up in [`Workspace::new`],
/// down in the close hook. The last one to close takes the app with it;
/// settings, popout, and customize windows are children of a workspace,
/// not reasons to keep a headless process alive.
#[derive(Default)]
struct WorkspaceWindows(usize);

impl Global for WorkspaceWindows {}

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

actions!(
    rox,
    [TogglePlayback, SeekBackward, SeekForward, OpenSettings, Quit]
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
    cx.bind_keys([
        KeyBinding::new("space", TogglePlayback, PLAYBACK_KEY_SCOPE),
        KeyBinding::new("left", SeekBackward, PLAYBACK_KEY_SCOPE),
        KeyBinding::new("right", SeekForward, PLAYBACK_KEY_SCOPE),
        KeyBinding::new(settings_keys, OpenSettings, Some("Workspace")),
        KeyBinding::new("ctrl-i", OpenSettings, Some("Workspace")),
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
/// workspace can re-register.
fn register_panels(state: &AppState, cx: &mut App) {
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
    configured!("seek", SeekStripPanel);
    configured!("track info", TrackInfoPanel);
    configured!("cover art", CoverArtPanel);
    configured!("playback", TransportPanel);
    configured!("volume", VolumePanel);
    configured!("spectrum", SpectrumPanel);
    macro_rules! stateless {
        ($name:literal, $panel:ty) => {{
            let s = state.clone();
            register_panel(cx, $name, move |_, _, _, _, cx| {
                Box::new(cx.new(|cx| <$panel>::new(s.clone(), cx)))
            });
        }};
    }
    stateless!("waveform", WaveformPanel);
}

#[derive(Clone, Copy)]
enum MenuAction {
    NewWindow,
    OpenSettings,
    OpenLibrary,
    OpenCoverArt,
    OpenSpectrum,
    OpenWaveform,
    OpenTrackInfo,
    OpenPlayback,
    OpenVolume,
    OpenSeek,
}

struct MenuItem {
    label: &'static str,
    action: MenuAction,
}

/// A dropdown row: either an action item or a submenu that flies out to
/// the side on hover.
enum MenuEntry {
    Item(MenuItem),
    Submenu {
        label: &'static str,
        items: &'static [MenuItem],
    },
}

struct Menu {
    label: &'static str,
    entries: &'static [MenuEntry],
}

const MENUS: &[Menu] = &[
    Menu {
        label: "Window",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "New Window",
                action: MenuAction::NewWindow,
            }),
            MenuEntry::Item(MenuItem {
                label: "Settings...",
                action: MenuAction::OpenSettings,
            }),
        ],
    },
    Menu {
        label: "Panels",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "Library",
                action: MenuAction::OpenLibrary,
            }),
            MenuEntry::Item(MenuItem {
                label: "Cover Art",
                action: MenuAction::OpenCoverArt,
            }),
            MenuEntry::Submenu {
                label: "Controls",
                items: &[
                    MenuItem {
                        label: "Track Info",
                        action: MenuAction::OpenTrackInfo,
                    },
                    MenuItem {
                        label: "Playback",
                        action: MenuAction::OpenPlayback,
                    },
                    MenuItem {
                        label: "Seek",
                        action: MenuAction::OpenSeek,
                    },
                    MenuItem {
                        label: "Volume",
                        action: MenuAction::OpenVolume,
                    },
                ],
            },
            MenuEntry::Submenu {
                label: "Visualizers",
                items: &[
                    MenuItem {
                        label: "Spectrum",
                        action: MenuAction::OpenSpectrum,
                    },
                    MenuItem {
                        label: "Waveform",
                        action: MenuAction::OpenWaveform,
                    },
                ],
            },
        ],
    },
];

pub struct Workspace {
    open_menu: Option<usize>,
    /// Which submenu entry of the open dropdown is flown out, by entry
    /// index. Hovering an entry moves it, closing the menu clears it.
    open_submenu: Option<usize>,
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
    /// A new bake must repaint the window that shows it.
    _backdrop_changed: Subscription,
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

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let player = cx.new(Player::new);
        let state = AppState {
            library: cx.new(Library::new),
            now_art: cx.new(|cx| NowPlayingArt::new(player.clone(), cx)),
            player,
            selection: cx.new(|_| Selection::default()),
            tab_hosts: cx.new(|_| TabHosts::default()),
        };
        let focus = cx.focus_handle();
        window.focus(&focus);

        let dock = cx.new(|cx| DockArea::new("rox", Some(LAYOUT_VERSION), window, cx));
        let weak_dock = dock.downgrade();

        register_panels(&state, cx);

        // The saved layout when there is one it can trust: current version,
        // and a stack at the root, which a dumped root stack always is.
        // Anything else builds the default layout instead.
        let restored = Settings::load()
            .layout
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
        });
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        cx.default_global::<WorkspaceWindows>().0 += 1;
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.persist(window, cx));
            }
            // Closing the last workspace window quits; without this, a
            // settings or popout window left open keeps the app running
            // with the menubar (and New Window) gone.
            let open = cx.default_global::<WorkspaceWindows>();
            open.0 = open.0.saturating_sub(1);
            if open.0 == 0 {
                cx.quit();
            }
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

        Workspace {
            open_menu: None,
            open_submenu: None,
            state,
            focus,
            dock,
            stack,
            center_tabs,
            bottom_stack,
            save_task: None,
            backdrop: WindowBackdrop::default(),
            titled_track: None,
            _layout_changed,
            _player_changed,
            _library_changed,
            _backdrop_changed,
        }
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
    fn persist(&mut self, window: &Window, cx: &mut Context<Self>) {
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
        Settings::update(move |s| {
            s.layout = layout;
            s.window = Some(window_state);
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

    fn run(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            MenuAction::NewWindow => crate::open_workspace(cx),
            MenuAction::OpenSettings => {
                crate::settings_window::open(self.state.clone(), cx)
            }
            MenuAction::OpenLibrary => {
                let panel = cx.new(|cx| {
                    LibraryPanel::new(self.state.clone(), LibraryConfig::default(), window, cx)
                });
                self.add_center(Arc::new(panel), window, cx);
            }
            MenuAction::OpenCoverArt => {
                let panel =
                    cx.new(|cx| CoverArtPanel::new(self.state.clone(), CoverConfig::default(), cx));
                self.add_center(Arc::new(panel), window, cx);
            }
            MenuAction::OpenSpectrum => {
                let panel = cx.new(|cx| {
                    SpectrumPanel::new(self.state.clone(), SpectrumConfig::default(), cx)
                });
                self.add_bottom(Arc::new(panel), window, cx);
            }
            MenuAction::OpenWaveform => {
                let panel = cx.new(|cx| WaveformPanel::new(self.state.clone(), cx));
                self.add_bottom(Arc::new(panel), window, cx);
            }
            MenuAction::OpenTrackInfo => {
                let panel = cx.new(|cx| {
                    TrackInfoPanel::new(self.state.clone(), TrackInfoConfig::default(), cx)
                });
                self.add_bottom(Arc::new(panel), window, cx);
            }
            MenuAction::OpenPlayback => {
                let panel = cx.new(|cx| {
                    TransportPanel::new(self.state.clone(), TransportConfig::default(), cx)
                });
                self.add_bottom(Arc::new(panel), window, cx);
            }
            MenuAction::OpenVolume => {
                let panel =
                    cx.new(|cx| VolumePanel::new(self.state.clone(), VolumeConfig::default(), cx));
                self.add_bottom(Arc::new(panel), window, cx);
            }
            MenuAction::OpenSeek => {
                let panel =
                    cx.new(|cx| SeekStripPanel::new(self.state.clone(), SeekConfig::default(), cx));
                self.add_bottom(Arc::new(panel), window, cx);
            }
        }
    }

    fn menu_button(
        &self,
        index: usize,
        menu: &'static Menu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_menu == Some(index);
        div()
            .relative()
            .h_full()
            .px(tokens::SPACE_MD)
            .flex()
            .items_center()
            .cursor_pointer()
            .when(open, |d| d.bg(palette::bg_control_active()))
            .hover(|d| d.bg(palette::bg_menu_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_menu = if this.open_menu == Some(index) {
                        None
                    } else {
                        Some(index)
                    };
                    this.open_submenu = None;
                    cx.notify();
                }),
            )
            // Clicking anywhere outside this button closes its menu; a click
            // that lands on a dropdown item still runs the item's handler.
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.open_menu = None;
                    this.open_submenu = None;
                    cx.notify();
                }))
            })
            .child(menu.label)
            .when(open, |d| d.child(deferred(self.dropdown(menu, cx))))
    }

    /// The menubar's right side: the catalog status line, a badge while a
    /// scan or load runs, a rescan button once a folder is known, and an
    /// abort button while a scan runs.
    fn library_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (busy, status, can_rescan, scanning) = {
            let library = self.state.library.read(cx);
            (
                library.busy(),
                library.status(),
                library.can_rescan(),
                library.scanning(),
            )
        };
        let idle = busy.is_none();
        // Status text leftmost so its width changes grow into the empty
        // middle of the bar; the badge and buttons keep their spot at the
        // right edge.
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .when(!status.is_empty(), |d| {
                d.child(
                    div()
                        .max_w(px(480.))
                        .truncate()
                        .text_color(palette::text_muted())
                        // While scanning the status is the full path of the
                        // file under the cursor: smaller text.
                        .when(scanning, |d| d.text_xs())
                        .child(status),
                )
            })
            .when_some(busy, |d, label| {
                // Tabular digits, so the count ticking up never changes
                // the badge width within a digit count.
                let mut badge = div()
                    .px(tokens::SPACE_SM)
                    .py(px(2.))
                    .rounded_full()
                    .bg(palette::accent())
                    .text_xs()
                    .text_color(palette::text_on_accent());
                badge
                    .text_style()
                    .get_or_insert_with(Default::default)
                    .font_features = Some(FontFeatures(Arc::new(vec![("tnum".into(), 1)])));
                d.child(badge.child(label))
            })
            .when(can_rescan && idle, |d| {
                d.child(panel::icon_control_sized(
                    icons::REFRESH_CW,
                    px(12.),
                    palette::text_muted(),
                    |this: &mut Workspace, cx| {
                        this.state
                            .library
                            .update(cx, |library, cx| library.rescan(cx));
                    },
                    cx,
                ))
            })
            .when(scanning, |d| {
                d.child(panel::icon_control_sized(
                    icons::CLOSE,
                    px(12.),
                    palette::text_muted(),
                    |this: &mut Workspace, cx| {
                        this.state
                            .library
                            .update(cx, |library, cx| library.abort_scan(cx));
                    },
                    cx,
                ))
            })
    }

    fn dropdown(&self, menu: &'static Menu, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .left_0()
            .top(px(MENU_BAR_H))
            .min_w(px(180.))
            .flex()
            .flex_col()
            .py(tokens::SPACE_XS)
            .bg(palette::bg_menu_opaque())
            .border_1()
            .border_color(palette::border_light())
            .shadow_md()
            .occlude()
            .children(menu.entries.iter().enumerate().map(|(i, entry)| {
                match entry {
                    MenuEntry::Item(item) => self
                        .action_item(item, cx)
                        .id(("menu-entry", i))
                        // Sliding onto a plain item retracts a flyout a
                        // sibling submenu left open.
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if *hovered && this.open_submenu.is_some() {
                                this.open_submenu = None;
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                    MenuEntry::Submenu { label, items } => {
                        self.submenu_row(i, label, items, cx).into_any_element()
                    }
                }
            }))
    }

    /// A dropdown row that runs an action and closes the menu. The caller
    /// chains its hover behavior, which differs between the top level and a
    /// flyout.
    fn action_item(&self, item: &'static MenuItem, cx: &mut Context<Self>) -> Div {
        let action = item.action;
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.open_menu = None;
                    this.open_submenu = None;
                    cx.notify();
                    this.run(action, window, cx);
                }),
            )
            .child(item.label)
    }

    /// A dropdown row that flies its items out to the side while hovered.
    /// The flyout stays open until another entry is hovered or the menu
    /// closes, so the pointer can cross the gap without losing it.
    fn submenu_row(
        &self,
        index: usize,
        label: &'static str,
        items: &'static [MenuItem],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_submenu == Some(index);
        div()
            .id(("menu-entry", index))
            .relative()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .when(open, |d| d.bg(palette::bg_control_hover_opaque()))
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_submenu != Some(index) {
                    this.open_submenu = Some(index);
                    cx.notify();
                }
            }))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .child(label)
            .child(
                svg()
                    .path(icons::CHEVRON_RIGHT)
                    .size_3()
                    .text_color(palette::text_muted()),
            )
            .when(open, |d| {
                d.child(
                    // Top offset backs out the parent's padding and the
                    // dropdown border so the first item lines up with the
                    // parent row.
                    div()
                        .absolute()
                        .left_full()
                        .top(px(-5.))
                        .min_w(px(160.))
                        .flex()
                        .flex_col()
                        .py(tokens::SPACE_XS)
                        .bg(palette::bg_menu_opaque())
                        .border_1()
                        .border_color(palette::border_light())
                        .shadow_md()
                        .occlude()
                        .children(items.iter().map(|item| self.action_item(item, cx))),
                )
            })
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                crate::settings_window::open(this.state.clone(), cx);
            }))
            // Quit bypasses the window close hook, so dump the layout and
            // frame here or a pending debounce and any window move since
            // the last save are lost.
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.persist(window, cx);
                cx.quit();
            }))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // The backdrop paints first, under the menubar and dock; how
            // much shows through is the surfaces' call (ADR 10's strength
            // scalar).
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(px(MENU_BAR_H))
                    .flex_none()
                    .bg(palette::bg_menubar())
                    .border_b_1()
                    .border_color(palette::border())
                    .children(
                        MENUS
                            .iter()
                            .enumerate()
                            .map(|(i, menu)| self.menu_button(i, menu, cx)),
                    )
                    .child(div().flex_1())
                    .child(self.library_status(cx)),
            )
            .child(div().flex_1().min_h_0().child(self.dock.clone()))
    }
}
