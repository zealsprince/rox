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
    deferred, div, prelude::*, px, rgb, Context, Entity, MouseButton, Subscription, Window,
};
use gpui_component::dock::{DockArea, DockItem, DockPlacement, PanelEvent, PanelView};

use crate::library::{Library, LibraryPanel};
use crate::panel::AppState;
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
    /// A panel is zoomed: the player bar collapses so the dock takes the
    /// whole area under the menu bar.
    zoomed: bool,
    _zoom_relay: Subscription,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState {
            library: cx.new(Library::new),
            player: cx.new(Player::new),
        };

        let dock = cx.new(|cx| DockArea::new("rox", None, window, cx));
        let weak_dock = dock.downgrade();

        let library_panel = cx.new(|cx| LibraryPanel::new(state.clone(), String::new(), cx));
        let center: Vec<Arc<dyn PanelView>> = vec![Arc::new(library_panel)];
        let center = DockItem::tabs(center, &weak_dock, window, cx);

        // gpui-component only wires zoom for tab panels inside splits, docks,
        // and tiles; a bare tabs item at the center emits its zoom events
        // into the void. Handle them here: the center already spans the dock
        // area, so zoom means giving it the player bar's row too.
        let center_tabs = match &center {
            DockItem::Tabs { view, .. } => view.clone(),
            _ => unreachable!("the center is built as tabs right above"),
        };
        let _zoom_relay = cx.subscribe_in(
            &center_tabs,
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
        );

        dock.update(cx, |dock, cx| {
            dock.set_center(center, window, cx);
            dock.set_toggle_button_visible(false, cx);
        });

        Workspace {
            open_menu: None,
            state,
            dock,
            zoomed: false,
            _zoom_relay,
        }
    }

    fn add_center(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock.update(cx, |dock, cx| {
            dock.add_panel(panel, DockPlacement::Center, None, window, cx)
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
