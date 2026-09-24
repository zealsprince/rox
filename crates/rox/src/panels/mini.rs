//! The mini toggle panel: one button that swaps the workspace between its mini
//! and primary layouts. Faint and inert with no mini layout assigned.

use gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, MouseButton, Pixels,
    Subscription, WeakEntity, Window, div, prelude::*, px, svg,
};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::workspace::Workspace;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_kit::{Align, align_row, justify};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MiniToggleConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
}

pub struct MiniTogglePanel {
    state: AppState,
    config: MiniToggleConfig,
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _workspace_changed: Option<Subscription>,
}

impl MiniTogglePanel {
    pub fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: MiniToggleConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let _workspace_changed = workspace
            .upgrade()
            .map(|ws| cx.observe(&ws, |_, _, cx| cx.notify()));
        MiniTogglePanel {
            state,
            config,
            workspace,
            focus: cx.focus_handle(),
            tab_panel: None,
            _workspace_changed,
        }
    }

    fn body(&self, cx: &mut Context<Self>) -> Div {
        let assigned = self
            .workspace
            .upgrade()
            .is_some_and(|ws| ws.read(cx).mini_assigned());
        let on_mini = self
            .workspace
            .upgrade()
            .is_some_and(|ws| ws.read(cx).on_mini());
        let icon = if on_mini {
            icons::MAXIMIZE
        } else {
            icons::MINIMIZE
        };
        let tip = match (assigned, on_mini) {
            (false, _) => rox_i18n::t!("mini-tip-none"),
            (true, true) => rox_i18n::t!("mini-tip-back"),
            (true, false) => rox_i18n::t!("mini-tip-shrink"),
        };

        let button = panel::Tip::keyed("mini-toggle", tip).apply(
            div()
                .size(px(24.))
                .rounded(tokens::RADIUS)
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon).size(px(14.)).text_color(if assigned {
                    palette::text_muted()
                } else {
                    palette::text_faint()
                }))
                .when(assigned, |d| {
                    d.cursor_pointer()
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
                }),
        );

        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .px(tokens::SPACE_MD)
            .child(button)
    }
}

impl PanelSettings for MiniTogglePanel {
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

impl Render for MiniTogglePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl EventEmitter<PanelEvent> for MiniTogglePanel {}

impl Focusable for MiniTogglePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for MiniTogglePanel {
    fn panel_name(&self) -> &'static str {
        "mini toggle"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("mini-title"),
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
            gpui::size(px(40.), rox_dock::resizable::PANEL_MIN_SIZE),
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
                MiniTogglePanel::new(state, workspace, config, cx)
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
