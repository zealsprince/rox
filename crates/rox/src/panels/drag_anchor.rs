//! The drag anchor panel: a grip that moves the OS window it sits in.
//! Layouts without OS decorations (the mini player especially) keep a
//! handle to drag by; the whole strip is the grab surface, not just the
//! icon. The move is the compositor's, so it works wherever
//! `start_window_move` does.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, svg, AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    MouseButton, Pixels, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;

/// The drag anchor panel's per-view config: what a saved layout restores,
/// and what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct DragAnchorConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
}

pub struct DragAnchorPanel {
    state: AppState,
    config: DragAnchorConfig,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
}

impl DragAnchorPanel {
    pub fn new(state: AppState, config: DragAnchorConfig, cx: &mut Context<Self>) -> Self {
        DragAnchorPanel {
            state,
            config,
            focus: cx.focus_handle(),
            tab_panel: None,
        }
    }

    fn body(&self) -> Div {
        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .px(tokens::SPACE_MD)
            .cursor_grab()
            // The whole strip hands the pointer to the compositor's move.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(
                svg()
                    .path(icons::MOVE)
                    .size(px(14.))
                    .text_color(palette::text_faint()),
            )
    }
}

impl PanelSettings for DragAnchorPanel {
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

impl Render for DragAnchorPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body())
    }
}

impl EventEmitter<PanelEvent> for DragAnchorPanel {}

impl Focusable for DragAnchorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for DragAnchorPanel {
    fn panel_name(&self) -> &'static str {
        "drag anchor"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Drag Anchor")
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
        // The grip plus the strip's padding, raised by any user floor.
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
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), _window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate takes the config along, like the transport panels'.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| DragAnchorPanel::new(state, config, cx));
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
