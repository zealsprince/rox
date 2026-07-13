//! The main window: an in-window menubar over the dock area. GPUI only
//! surfaces `set_menus` in the macOS system bar, so the bar is drawn
//! in-window to behave the same on every platform. The dock, tabs, splits,
//! and resize come from gpui-component per ADR 7; duplicate and pop-out
//! live on the panels themselves. Playback UI is the transport panels in
//! the bottom dock; the PCM tap that feeds the audio views is drained by
//! the player's own pump task, so nothing here has to keep rendering for
//! playback's sake.

use std::sync::Arc;

use gpui::{
    actions, deferred, div, prelude::*, px, rgb, App, Axis, Context, Entity, FocusHandle,
    KeyBinding, MouseButton, WeakEntity, Window,
};
use rox_dock::{DockArea, DockItem, Panel as _, PanelView, StackPanel, TabPanel};

use crate::library::{Library, LibraryPanel};
use crate::panel::{AppState, TabHosts};
use crate::player::Player;
use crate::spectrum::SpectrumPanel;
use crate::transport::{SeekStripPanel, TransportPanel, VolumePanel};
use crate::waveform::WaveformPanel;

const MENU_BAR_H: f32 = 30.0;

/// The bottom dock's starting height. Docks clamp to a 100px minimum, so
/// this is just enough for the transport row plus its status line.
const BOTTOM_DOCK_H: f32 = 120.0;

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

#[derive(Clone, Copy)]
enum MenuAction {
    NewWindow,
    OpenFolder,
    OpenLibrary,
    OpenSpectrum,
    OpenWaveform,
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
    /// The stack the center tabs sit in; the parent that makes tab dragging
    /// and splitting possible at all.
    stack: Entity<StackPanel>,
    /// The tab panel the layout starts with. View-menu panels land here
    /// while it is still showing.
    center_tabs: Entity<TabPanel>,
    /// The bottom dock's stack: the transport groups at start, and the row
    /// View-menu audio panels append to.
    bottom_stack: Entity<StackPanel>,
}

/// A one-group tabs item plus the TabPanel entity inside it, for wiring the
/// group into a hand-built stack.
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

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState {
            library: cx.new(Library::new),
            player: cx.new(Player::new),
            tab_hosts: cx.new(|_| TabHosts::default()),
        };
        let focus = cx.focus_handle();
        window.focus(&focus);

        let dock = cx.new(|cx| DockArea::new("rox", None, window, cx));
        let weak_dock = dock.downgrade();

        let library_panel = cx.new(|cx| LibraryPanel::new(state.clone(), String::new(), cx));
        let (tabs, center_tabs) = tabs_item(vec![Arc::new(library_panel)], &weak_dock, window, cx);

        // Tab dragging, dropping, and splitting only work when the tab panel
        // has a parent StackPanel: without one it counts itself locked (which
        // also kills closing, middle-click included). Wrap the tabs in a
        // one-item split, built by hand because 0.5.1's
        // DockItem::split_with_sizes adds every panel to its stack twice.
        let stack = cx.new(|cx| {
            let mut stack = StackPanel::new(Axis::Horizontal, window, cx);
            stack.add_panel(
                Arc::new(center_tabs.clone()),
                None,
                weak_dock.clone(),
                window,
                cx,
            );
            stack
        });
        let center = DockItem::Split {
            axis: Axis::Horizontal,
            size: None,
            items: vec![tabs],
            sizes: vec![None],
            view: stack.clone(),
        };

        // The bottom dock replaces the old fixed player bar: the transport
        // pieces as three side-by-side tab groups. Hand-built the same way
        // as the center, and for the same lock reason.
        let playback = cx.new(|cx| TransportPanel::new(state.clone(), cx));
        let seek = cx.new(|cx| SeekStripPanel::new(state.clone(), cx));
        let volume = cx.new(|cx| VolumePanel::new(state.clone(), cx));
        let (playback_item, playback_tabs) =
            tabs_item(vec![Arc::new(playback)], &weak_dock, window, cx);
        let (seek_item, seek_tabs) = tabs_item(vec![Arc::new(seek)], &weak_dock, window, cx);
        let (volume_item, volume_tabs) = tabs_item(vec![Arc::new(volume)], &weak_dock, window, cx);
        let sizes = [Some(px(420.)), None, Some(px(280.))];
        let bottom_stack = cx.new(|cx| {
            let mut stack = StackPanel::new(Axis::Horizontal, window, cx);
            for (tabs, size) in [playback_tabs, seek_tabs, volume_tabs]
                .into_iter()
                .zip(sizes)
            {
                stack.add_panel(Arc::new(tabs), size, weak_dock.clone(), window, cx);
            }
            stack
        });
        let bottom = DockItem::Split {
            axis: Axis::Horizontal,
            size: None,
            items: vec![playback_item, seek_item, volume_item],
            sizes: sizes.to_vec(),
            view: bottom_stack.clone(),
        };

        // Zoom needs nothing from the workspace anymore: adding tab panels
        // to stacks makes the dock area subscribe them, and the zoomed
        // panel covers the whole dock area, which is the whole window under
        // the menu bar.
        dock.update(cx, |dock, cx| {
            dock.set_center(center, window, cx);
            dock.set_bottom_dock(bottom, Some(px(BOTTOM_DOCK_H)), true, window, cx);
            dock.set_toggle_button_visible(false, cx);
        });

        Workspace {
            open_menu: None,
            state,
            focus,
            dock,
            stack,
            center_tabs,
            bottom_stack,
        }
    }

    fn add_center(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The dock's own add-to-center always targets the initial tabs item,
        // but drags can empty that tab panel out of the tree. Add to it
        // while it still shows, otherwise to the newest live tab panel, and
        // failing both (everything popped out or closed) put the original
        // one back on the stack first.
        let tabs = if self.center_tabs.read(cx).visible(cx) {
            self.center_tabs.clone()
        } else if let Some(tabs) = self.state.tab_hosts.read(cx).last_live(cx) {
            tabs
        } else {
            let tabs_view: Arc<dyn PanelView> = Arc::new(self.center_tabs.clone());
            let weak_dock = self.dock.downgrade();
            self.stack.update(cx, |stack, cx| {
                stack.add_panel(tabs_view, None, weak_dock, window, cx);
            });
            self.center_tabs.clone()
        };
        tabs.update(cx, |tabs, cx| tabs.add_panel(panel, window, cx));
    }

    /// New audio and transport panels join the bottom dock as their own tab
    /// group at the end of the row - a new group rather than a new tab, so
    /// they sit next to the transport pieces instead of hiding one. The
    /// library stays a center panel: it wants the tall area, and keeping
    /// additions on the center path preserves the recovery route when every
    /// center panel has been closed or popped out.
    fn add_bottom(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak_dock = self.dock.downgrade();
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
            MenuAction::OpenPlayback => {
                let panel = cx.new(|cx| TransportPanel::new(self.state.clone(), cx));
                self.add_bottom(Arc::new(panel), window, cx);
            }
            MenuAction::OpenVolume => {
                let panel = cx.new(|cx| VolumePanel::new(self.state.clone(), cx));
                self.add_bottom(Arc::new(panel), window, cx);
            }
            MenuAction::OpenSeek => {
                let panel = cx.new(|cx| SeekStripPanel::new(self.state.clone(), cx));
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
