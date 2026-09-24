//! The window controls panel: minimize, maximize and close for the OS window
//! hosting it, for layouts with the OS decorations off. Icons or macOS traffic
//! lights; a popped-out copy controls its own window.

use gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, Pixels, Stateful, Subscription, WeakEntity, Window, div, prelude::*, px, svg,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_core::settings::ChromeStyle;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_panel_kit::{icon_controls, traffic_lights};
use serde::{Deserialize, Serialize};

use crate::workspace::Workspace;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_kit::{Align, align_row, justify};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct WindowControlsConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub style: ChromeStyle,
    #[serde(default)]
    pub mini: bool,
    #[serde(default)]
    pub align: Align,
}

pub struct WindowControlsPanel {
    state: AppState,
    config: WindowControlsConfig,
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
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

    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("window-controls-traffic-lights"))
                .checked(self.config.style == ChromeStyle::Traffic)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.style = match this.config.style {
                            ChromeStyle::Icons => ChromeStyle::Traffic,
                            ChromeStyle::Traffic => ChromeStyle::Icons,
                        };
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new(rox_i18n::t!("window-controls-mini-toggle"))
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

    fn mini_button(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if !self.config.mini {
            return None;
        }
        let ws = self.workspace.upgrade()?;
        if !ws.read(cx).mini_assigned() {
            return None;
        }
        let (icon, tip) = if ws.read(cx).on_mini() {
            (icons::MAXIMIZE, rox_i18n::t!("mini-tip-back"))
        } else {
            (icons::MINIMIZE, rox_i18n::t!("mini-tip-shrink"))
        };
        Some(
            panel::Tip::keyed("mini-toggle", tip).apply(
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
                            // Deferred: the toggle dumps the dock, which reads this
                            // panel, and a read inside its own update panics.
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
            ),
        )
    }

    fn body(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        // A workspace window runs the OS close button's teardown, so closing
        // the last one quits. A popped-out copy just closes.
        let close =
            |this: &mut Self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| {
                // Deferred: the teardown dumps the layout, which reads this
                // panel, and a read inside its own update panics.
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
                ChromeStyle::Icons => d
                    .gap(tokens::SPACE_XS)
                    .children(icon_controls(window, cx.listener(close))),
                ChromeStyle::Traffic => d
                    .gap(tokens::SPACE_SM)
                    .children(traffic_lights(window, cx.listener(close))),
            })
    }
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
                rox_i18n::t!("window-controls-style"),
                Some(rox_i18n::t!("window-controls-style.description")),
                panel::choices_shared(
                    &[
                        (
                            rox_i18n::t!("window-controls-style-icons"),
                            ChromeStyle::Icons,
                        ),
                        (
                            rox_i18n::t!("window-controls-traffic-lights"),
                            ChromeStyle::Traffic,
                        ),
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
                rox_i18n::t!("window-controls-mini-toggle"),
                Some(rox_i18n::t!("window-controls-mini-toggle.description")),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
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

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("window-controls-title"),
        )
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
        rox_panel_api::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(px(96.), rox_dock::resizable::PANEL_MIN_SIZE),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
        rox_panel_api::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = self.config_menu(menu, cx);
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, _window, cx| {
                let (state, workspace, config) = {
                    let panel = this.read(cx);
                    (
                        panel.state.clone(),
                        panel.workspace.clone(),
                        panel.config.clone(),
                    )
                };
                WindowControlsPanel::new(state, workspace, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}
