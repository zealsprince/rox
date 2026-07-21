//! The mini toggle panel: a single button that swaps the workspace between
//! its mini and primary layouts, the menubar toggle and the window
//! controls' lead button as a panel of its own. The glyph follows the
//! workspace, minimize while on the primary layout, maximize while on the
//! mini one. With no mini layout assigned it sits faint and inert, the
//! same gate every mini toggle shows behind.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, svg, AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    MouseButton, Pixels, Subscription, WeakEntity, Window,
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

/// The mini toggle panel's per-view config: what a saved layout restores,
/// and what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MiniToggleConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
}

pub struct MiniTogglePanel {
    state: AppState,
    config: MiniToggleConfig,
    /// The workspace this panel toggles; gone in a popped-out window whose
    /// workspace has closed, where the button just sits inert.
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The glyph follows the workspace's mini state.
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
        // Assigned and live: the interactive toggle. Otherwise a faint,
        // inert glyph so the panel isn't blank, the same gate the menubar
        // and window controls toggles show behind.
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

        let button = div()
            .size(px(24.))
            .rounded(tokens::RADIUS)
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg().path(icon).size(px(14.)).text_color(if assigned {
                    palette::text_muted()
                } else {
                    palette::text_faint()
                }),
            )
            .when(assigned, |d| {
                d.cursor_pointer()
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
            });

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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Mini Toggle")
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
        // The button plus the strip's padding, raised by any user floor.
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(px(40.), rox_dock::resizable::PANEL_MIN_SIZE),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
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
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), _window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate takes the config along, like the window controls panel's.
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
                    let dup = cx.new(|cx| MiniTogglePanel::new(state, workspace, config, cx));
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
