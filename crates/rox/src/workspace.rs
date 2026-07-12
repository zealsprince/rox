//! The main window: an in-window menubar over the dock area, with the player
//! bar as a fixed row under it. GPUI only surfaces `set_menus` in the macOS
//! system bar, so the bar is drawn in-window to behave the same on every
//! platform. The dock, tabs, splits, and resize come from gpui-component per
//! ADR 7; duplicate and pop-out live on the panels themselves. The player
//! stays outside the dock: its render pass drains the PCM tap that feeds
//! every audio view, so it has to keep rendering even while a panel is
//! zoomed, and a dock would stack a title row over it and clamp it to the
//! dock's 100px minimum height.

use std::sync::Arc;

use gpui::{
    deferred, div, prelude::*, px, rgb, Axis, Context, Entity, MouseButton, Subscription, Window,
};
use gpui_component::dock::{
    DockArea, DockItem, Panel as _, PanelEvent, PanelView, StackPanel, TabPanel,
};

use crate::library::{Library, LibraryPanel};
use crate::panel::{AppState, TabHostAdded, TabHosts};
use crate::player::Player;
use crate::spectrum::SpectrumPanel;
use crate::waveform::WaveformPanel;

const MENU_BAR_H: f32 = 30.0;
const PLAYER_BAR_H: f32 = 46.0;

#[derive(Clone, Copy)]
enum MenuAction {
    NewWindow,
    OpenFolder,
    OpenLibrary,
    OpenSpectrum,
    OpenWaveform,
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
        ],
    },
];

pub struct Workspace {
    open_menu: Option<usize>,
    state: AppState,
    dock: Entity<DockArea>,
    /// The stack the center tabs sit in; the parent that makes tab dragging
    /// and splitting possible at all.
    stack: Entity<StackPanel>,
    /// The tab panel the layout starts with. View-menu panels land here
    /// while it is still showing.
    center_tabs: Entity<TabPanel>,
    /// A panel is zoomed: the player bar collapses so the dock takes the
    /// whole area under the menu bar.
    zoomed: bool,
    /// One zoom relay per tab panel hosting our panels; drag-splits add
    /// more through the tab host watch.
    zoom_relays: Vec<Subscription>,
    _tab_hosts_watch: Subscription,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState {
            library: cx.new(Library::new),
            player: cx.new(Player::new),
            tab_hosts: cx.new(|_| TabHosts::default()),
        };

        let dock = cx.new(|cx| DockArea::new("rox", None, window, cx));
        let weak_dock = dock.downgrade();

        let library_panel = cx.new(|cx| LibraryPanel::new(state.clone(), String::new(), cx));
        let panels: Vec<Arc<dyn PanelView>> = vec![Arc::new(library_panel)];
        let tabs = DockItem::tabs(panels, &weak_dock, window, cx);
        let center_tabs = match &tabs {
            DockItem::Tabs { view, .. } => view.clone(),
            _ => unreachable!("the center is built as tabs right above"),
        };

        // Tab dragging, dropping, and splitting only work when the tab panel
        // has a parent StackPanel: without one it counts itself locked. Wrap
        // the tabs in a one-item split, built by hand because 0.5.1's
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

        // Adding a tab panel to a stack makes the dock area subscribe it, so
        // the dock now renders zoom itself: the zoomed panel covers the dock
        // area. The workspace listens too, only to hand the player bar's row
        // to the zoomed panel. Tab panels the dock creates later by
        // drag-splitting announce themselves through the tab host registry;
        // the watch below wires the same relay for those.
        let zoom_relays = vec![Self::zoom_relay(&center_tabs, window, cx)];
        let _tab_hosts_watch = cx.subscribe_in(
            &state.tab_hosts,
            window,
            |this: &mut Workspace, _, event: &TabHostAdded, window, cx| {
                if let Some(tabs) = event.0.upgrade() {
                    let relay = Self::zoom_relay(&tabs, window, cx);
                    this.zoom_relays.push(relay);
                }
            },
        );

        dock.update(cx, |dock, cx| {
            dock.set_center(center, window, cx);
            dock.set_toggle_button_visible(false, cx);
        });

        Workspace {
            open_menu: None,
            state,
            dock,
            stack,
            center_tabs,
            zoomed: false,
            zoom_relays,
            _tab_hosts_watch,
        }
    }

    fn zoom_relay(
        tabs: &Entity<TabPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(
            tabs,
            window,
            |this: &mut Workspace, _, event: &PanelEvent, _, cx| match event {
                PanelEvent::ZoomIn => {
                    this.zoomed = true;
                    cx.notify();
                }
                PanelEvent::ZoomOut => {
                    this.zoomed = false;
                    cx.notify();
                }
                PanelEvent::LayoutChanged => {}
            },
        )
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
                self.add_center(Arc::new(panel), window, cx);
            }
            MenuAction::OpenWaveform => {
                let panel = cx.new(|cx| WaveformPanel::new(self.state.clone(), cx));
                self.add_center(Arc::new(panel), window, cx);
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
            .child(
                // Zoomed, the bar collapses to zero height instead of
                // leaving the tree: the player's render pass must keep
                // running to drain the PCM tap for the audio views.
                div()
                    .flex_none()
                    .h(px(if self.zoomed { 0. } else { PLAYER_BAR_H }))
                    .overflow_hidden()
                    .child(self.state.player.clone()),
            )
    }
}
