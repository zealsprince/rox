//! The main window: an in-window menubar over the dock area. GPUI only
//! surfaces `set_menus` in the macOS system bar, so the bar is drawn
//! in-window to behave the same on every platform. The dock, tabs, splits,
//! and resize come from gpui-component per ADR 7; duplicate and pop-out
//! live on the panels themselves. Playback UI is the transport panels in
//! the bottom dock; the PCM tap that feeds the audio views is drained by
//! the player's own pump task, so nothing here has to keep rendering for
//! playback's sake.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, deferred, div, prelude::*, px, rgb, App, Axis, Context, Entity, FocusHandle,
    KeyBinding, MouseButton, Subscription, Task, WeakEntity, Window, WindowBounds,
};
use rox_dock::{
    register_panel, DockArea, DockAreaState, DockEvent, DockItem, Panel as _, PanelInfo,
    PanelView, StackPanel, TabPanel,
};

use crate::library::{Library, LibraryConfig, LibraryPanel};
use crate::panel::{self, AppState, TabHosts};
use crate::player::Player;
use crate::settings::{Settings, WindowState};
use crate::spectrum::SpectrumPanel;
use crate::transport::{
    SeekConfig, SeekStripPanel, TrackInfoConfig, TrackInfoPanel, TransportConfig, TransportPanel,
    VolumeConfig, VolumePanel,
};
use crate::waveform::WaveformPanel;

const MENU_BAR_H: f32 = 30.0;

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

actions!(rox, [TogglePlayback, SeekBackward, SeekForward]);

/// Bindings match key contexts along the focus path, so this scope holds
/// anywhere inside a workspace window except while the library search box
/// is focused: there space and arrows keep typing into the query. Bindings
/// win over key listeners, the exclusion is what hands the keys back.
const PLAYBACK_KEY_SCOPE: Option<&str> = Some("Workspace && !SearchInput");

/// App-level key bindings; call once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", TogglePlayback, PLAYBACK_KEY_SCOPE),
        KeyBinding::new("left", SeekBackward, PLAYBACK_KEY_SCOPE),
        KeyBinding::new("right", SeekForward, PLAYBACK_KEY_SCOPE),
    ]);
}

/// Teach the dock's registry to rebuild every panel type from a layout
/// dump. Registered per workspace so the builders capture that workspace's
/// entities; the restore runs synchronously right after, before another
/// workspace can re-register.
fn register_panels(state: &AppState, cx: &mut App) {
    let s = state.clone();
    register_panel(cx, "library", move |_, _, info, _, cx| {
        let config: LibraryConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| LibraryPanel::new(s.clone(), config.query, cx)))
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
    configured!("playback", TransportPanel);
    configured!("volume", VolumePanel);
    macro_rules! stateless {
        ($name:literal, $panel:ty) => {{
            let s = state.clone();
            register_panel(cx, $name, move |_, _, _, _, cx| {
                Box::new(cx.new(|cx| <$panel>::new(s.clone(), cx)))
            });
        }};
    }
    stateless!("spectrum", SpectrumPanel);
    stateless!("waveform", WaveformPanel);
}

#[derive(Clone, Copy)]
enum MenuAction {
    NewWindow,
    OpenFolder,
    OpenLibrary,
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

struct Menu {
    label: &'static str,
    items: &'static [MenuItem],
}

const MENUS: &[Menu] = &[
    Menu {
        label: "Window",
        items: &[MenuItem {
            label: "New Window",
            action: MenuAction::NewWindow,
        }],
    },
    Menu {
        label: "Library",
        items: &[MenuItem {
            label: "Open Folder...",
            action: MenuAction::OpenFolder,
        }],
    },
    Menu {
        label: "View",
        items: &[
            MenuItem {
                label: "Library",
                action: MenuAction::OpenLibrary,
            },
            MenuItem {
                label: "Spectrum",
                action: MenuAction::OpenSpectrum,
            },
            MenuItem {
                label: "Waveform",
                action: MenuAction::OpenWaveform,
            },
            MenuItem {
                label: "Track Info",
                action: MenuAction::OpenTrackInfo,
            },
            MenuItem {
                label: "Playback",
                action: MenuAction::OpenPlayback,
            },
            MenuItem {
                label: "Volume",
                action: MenuAction::OpenVolume,
            },
            MenuItem {
                label: "Seek",
                action: MenuAction::OpenSeek,
            },
        ],
    },
];

pub struct Workspace {
    open_menu: Option<usize>,
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
    /// The tab panel the layout starts with. View-menu panels land here
    /// while it is still showing.
    center_tabs: Entity<TabPanel>,
    /// The transport row's stack: the transport groups at start, and the
    /// row View-menu audio panels append to.
    bottom_stack: Entity<StackPanel>,
    /// The debounce for layout saves; replacing it cancels the running
    /// timer, so only a quiet layout dumps.
    save_task: Option<Task<()>>,
    _layout_changed: Subscription,
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
    let library_panel = cx.new(|cx| LibraryPanel::new(state.clone(), String::new(), cx));
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
        let state = AppState {
            library: cx.new(Library::new),
            player: cx.new(Player::new),
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
                let tabs =
                    tabs.unwrap_or_else(|| tabs_item(Vec::new(), &weak_dock, window, cx).1);
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
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.persist(window, cx));
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
            state,
            focus,
            dock,
            stack,
            center_tabs,
            bottom_stack,
            save_task: None,
            _layout_changed,
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
            MenuAction::OpenFolder => self
                .state
                .library
                .update(cx, |library, cx| library.browse(cx)),
            MenuAction::OpenLibrary => {
                let panel = cx.new(|cx| LibraryPanel::new(self.state.clone(), String::new(), cx));
                self.add_center(Arc::new(panel), window, cx);
            }
            MenuAction::OpenSpectrum => {
                let panel = cx.new(|cx| SpectrumPanel::new(self.state.clone(), cx));
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
                let panel = cx.new(|cx| SeekStripPanel::new(
                    self.state.clone(),
                    SeekConfig::default(),
                    cx,
                ));
                self.add_bottom(Arc::new(panel), window, cx);
            }
        }
    }

    fn menu_button(&self, index: usize, menu: &'static Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open_menu == Some(index);
        div()
            .relative()
            .h_full()
            .px_3()
            .flex()
            .items_center()
            .cursor_pointer()
            .when(open, |d| d.bg(rgb(0x333333)))
            .hover(|d| d.bg(rgb(0x2f2f2f)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_menu = if this.open_menu == Some(index) {
                        None
                    } else {
                        Some(index)
                    };
                    cx.notify();
                }),
            )
            // Clicking anywhere outside this button closes its menu; a click
            // that lands on a dropdown item still runs the item's handler.
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.open_menu = None;
                    cx.notify();
                }))
            })
            .child(menu.label)
            .when(open, |d| d.child(deferred(self.dropdown(menu, cx))))
    }

    fn dropdown(&self, menu: &'static Menu, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .left_0()
            .top(px(MENU_BAR_H))
            .min_w(px(180.))
            .flex()
            .flex_col()
            .py_1()
            .bg(rgb(0x262626))
            .border_1()
            .border_color(rgb(0x3a3a3a))
            .shadow_md()
            .occlude()
            .children(menu.items.iter().map(|item| {
                let action = item.action;
                div()
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(0x3a3a3a)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.open_menu = None;
                            cx.notify();
                            this.run(action, window, cx);
                        }),
                    )
                    .child(item.label)
            }))
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .track_focus(&self.focus)
            .key_context("Workspace")
            .on_action(cx.listener(|this, _: &TogglePlayback, _, cx| {
                this.state.player.update(cx, |player, _| player.toggle_pause());
            }))
            .on_action(cx.listener(|this, _: &SeekBackward, _, cx| {
                this.state.player.update(cx, |player, _| player.seek_by(-5.0));
            }))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                this.state.player.update(cx, |player, _| player.seek_by(5.0));
            }))
            .bg(rgb(0x1c1c1c))
            .text_color(rgb(0xe0e0e0))
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(px(MENU_BAR_H))
                    .flex_none()
                    .bg(rgb(0x242424))
                    .border_b_1()
                    .border_color(rgb(0x333333))
                    .children(
                        MENUS
                            .iter()
                            .enumerate()
                            .map(|(i, menu)| self.menu_button(i, menu, cx)),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.dock.clone()))
    }
}
