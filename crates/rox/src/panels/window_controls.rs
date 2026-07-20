//! The window controls panel: minimize, maximize, and close for whatever
//! OS window hosts it, the stand-in buttons for layouts that turn the OS
//! decorations off. Two styles: flat icons in the app's palette, or the
//! macOS traffic lights. The buttons drive the window they render in, so
//! a popped-out copy controls its own window.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, svg, AnyElement, App, Context, Div, EventEmitter, FocusHandle,
    Focusable, MouseButton, MouseDownEvent, Pixels, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::workspace::Workspace;

/// How the three buttons draw: flat icons like the rest of the chrome, or
/// the macOS traffic lights.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlStyle {
    #[default]
    Icons,
    Traffic,
}

/// The window controls panel's per-view config: what a saved layout
/// restores, and what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct WindowControlsConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub style: ControlStyle,
    /// Lead the row with the mini-layout toggle, the menubar button's
    /// twin. Only shows once a mini layout is assigned.
    #[serde(default)]
    pub mini: bool,
    #[serde(default)]
    pub align: Align,
}

/// The macOS traffic light colors, close to minimize to zoom.
const TRAFFIC_CLOSE: u32 = 0xff5f57;
const TRAFFIC_MIN: u32 = 0xfebc2e;
const TRAFFIC_ZOOM: u32 = 0x28c840;

pub struct WindowControlsPanel {
    state: AppState,
    config: WindowControlsConfig,
    /// The workspace this panel drives its mini toggle through; gone in
    /// a popped-out window whose workspace has closed, where the toggle
    /// just hides.
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The mini toggle's glyph follows the workspace's state.
    _workspace_changed: Option<Subscription>,
}

impl WindowControlsPanel {
    pub fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: WindowControlsConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let _workspace_changed = workspace
            .upgrade()
            .map(|ws| cx.observe(&ws, |_, _, cx| cx.notify()));
        WindowControlsPanel {
            state,
            config,
            workspace,
            focus: cx.focus_handle(),
            tab_panel: None,
            _workspace_changed,
        }
    }

    /// The panel's own dropdown entries: the quick style flip and the
    /// mini toggle, the same knobs the customize window edits.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Traffic Lights")
                .checked(self.config.style == ControlStyle::Traffic)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.style = match this.config.style {
                            ControlStyle::Icons => ControlStyle::Traffic,
                            ControlStyle::Traffic => ControlStyle::Icons,
                        };
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new("Mini Toggle")
                .checked(self.config.mini)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.mini = !this.config.mini;
                        cx.notify();
                    });
                }),
        )
    }

    /// The mini-layout toggle, the menubar button's twin: swaps the
    /// workspace between its mini and primary layouts. None while turned
    /// off, no mini layout is assigned, or the workspace is gone.
    fn mini_button(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.config.mini {
            return None;
        }
        let ws = self.workspace.upgrade()?;
        if !ws.read(cx).mini_assigned() {
            return None;
        }
        let icon = if ws.read(cx).on_mini() {
            icons::MAXIMIZE
        } else {
            icons::MINIMIZE
        };
        Some(
            div()
                .size(px(24.))
                .rounded(tokens::RADIUS)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|d| d.bg(palette::bg_control_hover()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        // Deferred out of this panel's update: the toggle
                        // stashes a dock dump, and dumping reads every
                        // panel, this one included - a read inside its own
                        // update panics.
                        let ws = this.workspace.clone();
                        window.defer(cx, move |window, cx| {
                            let Some(ws) = ws.upgrade() else { return };
                            ws.update(cx, |ws, cx| ws.toggle_mini(window, cx));
                        });
                    }),
                )
                .child(
                    svg()
                        .path(icon)
                        .size(px(14.))
                        .text_color(palette::text_muted()),
                ),
        )
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        // Close the window this panel sits in. A workspace window runs the
        // same teardown the OS close button does, so shutting the last one
        // quits and takes the settings and popout windows with it; a
        // popped-out copy of this panel isn't a workspace window, so it just
        // closes.
        let close =
            |this: &mut Self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| {
                // Deferred out of this panel's update: the workspace teardown
                // persists the layout, and dumping reads every panel, this one
                // included - a read inside its own update panics.
                let ws = this.workspace.clone();
                window.defer(cx, move |window, cx| {
                    if crate::workspace::is_workspace_window(window, cx) {
                        crate::workspace::close_workspace_window(ws.upgrade(), window, cx);
                    }
                    window.remove_window();
                });
            };
        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .px(tokens::SPACE_MD)
            .children(self.mini_button(cx))
            .map(|d| match self.config.style {
                // Windows order: minimize, maximize, close.
                ControlStyle::Icons => d
                    .gap(tokens::SPACE_XS)
                    .child(icon_button(icons::MINUS, |_, w, _| w.minimize_window()))
                    .child(icon_button(icons::STOP, |_, w, _| w.zoom_window()))
                    .child(icon_button(icons::CLOSE, cx.listener(close))),
                // macOS order: close, minimize, zoom.
                ControlStyle::Traffic => d
                    .gap(tokens::SPACE_SM)
                    .child(traffic_light(TRAFFIC_CLOSE, cx.listener(close)))
                    .child(traffic_light(TRAFFIC_MIN, |_, w, _| w.minimize_window()))
                    .child(traffic_light(TRAFFIC_ZOOM, |_, w, _| w.zoom_window())),
            })
    }
}

/// One flat button: an icon that runs its click handler.
fn icon_button(
    icon: &'static str,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .size(px(24.))
        .rounded(tokens::RADIUS)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|d| d.bg(palette::bg_control_hover()))
        .on_mouse_down(MouseButton::Left, handler)
        .child(
            svg()
                .path(icon)
                .size(px(14.))
                .text_color(palette::text_muted()),
        )
}

/// One traffic light: a colored circle that runs its click handler. No
/// hover glyphs, the color carries the meaning like macOS without focus.
fn traffic_light(
    color: u32,
    handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .size(px(12.))
        .rounded_full()
        .bg(rgb(color))
        .cursor_pointer()
        .hover(|d| d.opacity(0.8))
        .on_mouse_down(MouseButton::Left, handler)
}

impl PanelSettings for WindowControlsPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Layout", icons::ALIGN_LEFT)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "Style",
                Some("Flat icons, or the macOS traffic lights"),
                panel::choices(
                    &[
                        ("Icons", ControlStyle::Icons),
                        ("Traffic Lights", ControlStyle::Traffic),
                    ],
                    self.config.style,
                    |this: &mut Self, style, cx| {
                        this.config.style = style;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Mini Toggle",
                Some("Lead with the mini-layout toggle; shows once a mini layout is assigned"),
                panel::toggle(
                    self.config.mini,
                    |this: &mut Self, mini, cx| {
                        this.config.mini = mini;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .into_any_element()
    }
}

impl Render for WindowControlsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl EventEmitter<PanelEvent> for WindowControlsPanel {}

impl Focusable for WindowControlsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for WindowControlsPanel {
    fn panel_name(&self) -> &'static str {
        "window controls"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Window Controls")
    }

    fn tab_name(&self, _cx: &App) -> Option<gpui::SharedString> {
        self.config
            .chrome
            .title
            .clone()
            .map(gpui::SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
        // Three buttons plus the strip's padding.
        gpui::size(px(96.), rox_dock::resizable::PANEL_MIN_SIZE)
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The config block: the panel's quick entry and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, cx);
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), _window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate takes the config along, like the transport panels'.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, workspace, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.workspace.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| WindowControlsPanel::new(state, workspace, config, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}
