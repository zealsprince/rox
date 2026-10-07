//! The window controls panel: minimize, maximize and close for the OS window
//! hosting it, for layouts with the OS decorations off. Icons or macOS traffic
//! lights; a popped-out copy controls its own window. The buttons, the mini
//! toggle and the pin are one arrangeable strip.

use gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, Pixels, Rgba, SharedString, Stateful, Subscription, WeakEntity, Window, div,
    prelude::*, px, svg,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_core::settings::ChromeStyle;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_panel_kit::{icon_controls, traffic_lights};
use serde::{Deserialize, Serialize};

use crate::integrations::placement;
use crate::workspace::Workspace;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_kit::{Align, align_row, justify};

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlItem {
    /// Only in the workspace's own window, where the placement API reaches.
    Pin,
    /// Shows once a mini layout is assigned.
    Mini,
    Minimize,
    Maximize,
    Close,
    Spacer,
}

impl ControlItem {
    fn window_button(self) -> bool {
        matches!(
            self,
            ControlItem::Minimize | ControlItem::Maximize | ControlItem::Close
        )
    }
}

/// Stock order: where a menu toggle slots a re-shown item back in.
const ITEMS: &[panel::ArrangeSpec<ControlItem>] = &[
    panel::ArrangeSpec {
        key: "window-controls-pin",
        icon: Some(icons::PIN),
        value: ControlItem::Pin,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "window-controls-mini-toggle",
        icon: Some(icons::MINIMIZE),
        value: ControlItem::Mini,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "window-controls-minimize",
        icon: Some(icons::MINUS),
        value: ControlItem::Minimize,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "window-controls-maximize",
        icon: Some(icons::STOP),
        value: ControlItem::Maximize,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "window-controls-close",
        icon: Some(icons::WINDOW_CLOSE),
        value: ControlItem::Close,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: ControlItem::Spacer,
        repeats: true,
    },
];

/// Each style's platform order for the three window buttons, which is also
/// the order `icon_controls` and `traffic_lights` hand them back in.
fn platform_order(style: ChromeStyle) -> [ControlItem; 3] {
    match style {
        ChromeStyle::Icons => [
            ControlItem::Minimize,
            ControlItem::Maximize,
            ControlItem::Close,
        ],
        ChromeStyle::Traffic => [
            ControlItem::Close,
            ControlItem::Minimize,
            ControlItem::Maximize,
        ],
    }
}

/// Carry the window buttons over to the new style's platform order, unless
/// they've been rearranged by hand.
fn restyled(items: &[ControlItem], from: ChromeStyle, to: ChromeStyle) -> Vec<ControlItem> {
    let shown: Vec<ControlItem> = items
        .iter()
        .copied()
        .filter(|item| item.window_button())
        .collect();
    let stock = |style| -> Vec<ControlItem> {
        platform_order(style)
            .into_iter()
            .filter(|item| shown.contains(item))
            .collect()
    };
    if shown != stock(from) {
        return items.to_vec();
    }

    let mut next = stock(to).into_iter();
    items
        .iter()
        .map(|&item| match item.window_button() {
            true => next.next().unwrap_or(item),
            false => item,
        })
        .collect()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "WindowControlsDump")]
pub struct WindowControlsConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub style: ChromeStyle,
    pub items: Vec<ControlItem>,
    /// The pin shows only while the window is on the mini layout.
    pub pin_mini_only: bool,
    pub align: Align,
}

impl Default for WindowControlsConfig {
    fn default() -> Self {
        WindowControlsConfig {
            chrome: PanelChrome::default(),
            style: ChromeStyle::default(),
            items: platform_order(ChromeStyle::default()).to_vec(),
            pin_mini_only: false,
            align: Align::default(),
        }
    }
}

/// The retired pin picker. Legacy-only.
#[derive(Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PinButton {
    #[default]
    Hide,
    Show,
    Mini,
}

/// Reads the ordered list, or the `mini` switch and `pin` picker that led
/// the row before it.
#[derive(Deserialize)]
struct WindowControlsDump {
    #[serde(flatten)]
    chrome: PanelChrome,
    #[serde(default)]
    style: ChromeStyle,
    #[serde(default)]
    items: Option<Vec<ControlItem>>,
    #[serde(default)]
    pin_mini_only: bool,
    #[serde(default)]
    align: Align,
    #[serde(default)]
    mini: bool,
    #[serde(default)]
    pin: PinButton,
}

impl From<WindowControlsDump> for WindowControlsConfig {
    fn from(dump: WindowControlsDump) -> Self {
        let (items, pin_mini_only) = match dump.items {
            Some(items) => (panel::dedup(ITEMS, items), dump.pin_mini_only),
            None => {
                // The order the row rendered: pin, mini, then the buttons.
                let mut items = Vec::new();
                if dump.pin != PinButton::Hide {
                    items.push(ControlItem::Pin);
                }
                if dump.mini {
                    items.push(ControlItem::Mini);
                }
                items.extend(platform_order(dump.style));
                (items, dump.pin == PinButton::Mini)
            }
        };
        WindowControlsConfig {
            chrome: dump.chrome,
            style: dump.style,
            items,
            pin_mini_only,
            align: dump.align,
        }
    }
}

pub struct WindowControlsPanel {
    state: AppState,
    config: WindowControlsConfig,
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The arrangement before a menu toggle hid an item, so showing it again
    /// puts it back where it was.
    items_stash: Option<Vec<ControlItem>>,
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
            items_stash: None,
            _workspace_changed,
        }
    }

    fn set_style(&mut self, style: ChromeStyle, cx: &mut Context<Self>) {
        self.config.items = restyled(&self.config.items, self.config.style, style);
        self.config.style = style;
        cx.notify();
    }

    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let mut menu = menu;
        for spec in ITEMS
            .iter()
            .filter(|spec| spec.value != ControlItem::Spacer)
        {
            let value = spec.value;
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(rox_i18n::t!(spec.key))
                    .checked(self.config.items.contains(&value))
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            this.config.items = panel::toggled_stashed(
                                ITEMS,
                                &this.config.items,
                                &mut this.items_stash,
                                &[value],
                            );
                            cx.notify();
                        });
                    }),
            );
        }

        let weak = cx.entity().downgrade();
        menu.separator().item(
            PopupMenuItem::new(rox_i18n::t!("window-controls-traffic-lights"))
                .checked(self.config.style == ChromeStyle::Traffic)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        let style = match this.config.style {
                            ChromeStyle::Icons => ChromeStyle::Traffic,
                            ChromeStyle::Traffic => ChromeStyle::Icons,
                        };
                        this.set_style(style, cx);
                    });
                }),
        )
    }

    fn mini_button(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let ws = self.workspace.upgrade()?;
        if !ws.read(cx).mini_assigned() {
            return None;
        }
        let (icon, tip) = if ws.read(cx).on_mini() {
            (icons::MAXIMIZE, rox_i18n::t!("mini-tip-back"))
        } else {
            (icons::MINIMIZE, rox_i18n::t!("mini-tip-shrink"))
        };
        Some(self.toggle_button(
            "mini-toggle",
            icon,
            palette::text_muted(),
            tip,
            |ws, window, cx| ws.toggle_mini(window, cx),
            cx,
        ))
    }

    /// Only in the workspace's own window: a popped-out copy would pin the
    /// wrong one.
    fn pin_button(&self, window: &Window, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let ws = self.workspace.upgrade()?;
        let shown = !self.config.pin_mini_only || ws.read(cx).on_mini();
        if !shown || !placement::available(cx) || !crate::workspace::is_workspace_window(window, cx)
        {
            return None;
        }

        let (color, tip) = if ws.read(cx).pinned(window, cx) {
            (
                palette::accent(),
                rox_i18n::t!("window-controls-pin-tip-off"),
            )
        } else {
            (
                palette::text_muted(),
                rox_i18n::t!("window-controls-pin-tip-on"),
            )
        };
        Some(self.toggle_button(
            "pin-toggle",
            icons::PIN,
            color,
            tip,
            |ws, window, cx| ws.toggle_pin(window, cx),
            cx,
        ))
    }

    fn toggle_button(
        &self,
        key: &'static str,
        icon: &'static str,
        color: Rgba,
        tip: SharedString,
        action: fn(&mut Workspace, &mut Window, &mut Context<Workspace>),
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        panel::Tip::keyed(key, tip).apply(
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
                    cx.listener(move |this, _, window, cx| {
                        // Deferred: the mini toggle dumps the dock, which
                        // reads this panel, and a read inside its own update
                        // panics.
                        let ws = this.workspace.clone();
                        window.defer(cx, move |window, cx| {
                            let Some(ws) = ws.upgrade() else { return };
                            ws.update(cx, |ws, cx| action(ws, window, cx));
                        });
                    }),
                )
                .child(svg().path(icon).size(px(14.)).text_color(color)),
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

        // Built in the style's platform order, then taken out one at a time
        // wherever the arrangement puts each.
        let style = self.config.style;
        let (gap, buttons) = match style {
            ChromeStyle::Icons => (tokens::SPACE_XS, icon_controls(window, cx.listener(close))),
            ChromeStyle::Traffic => (tokens::SPACE_SM, traffic_lights(window, cx.listener(close))),
        };
        let mut buttons = buttons.map(Some);

        let items = self.config.items.clone();
        let children: Vec<AnyElement> = items
            .into_iter()
            .filter_map(|item| match item {
                ControlItem::Pin => self
                    .pin_button(window, cx)
                    .map(IntoElement::into_any_element),

                ControlItem::Mini => self.mini_button(cx).map(IntoElement::into_any_element),

                ControlItem::Minimize | ControlItem::Maximize | ControlItem::Close => {
                    let slot = platform_order(style).iter().position(|o| *o == item)?;
                    buttons[slot].take().map(IntoElement::into_any_element)
                }

                ControlItem::Spacer => Some(div().flex_1().into_any_element()),
            })
            .collect();

        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .px(tokens::SPACE_MD)
            .gap(gap)
            .children(children)
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
                    |this: &mut Self, style, cx| this.set_style(style, cx),
                    cx,
                ),
            ))
            .child(panel::setting_block(
                rox_i18n::t!("window-controls-pieces"),
                Some(rox_i18n::t!("window-controls-pieces.description")),
                None,
                panel::arrange_editor(
                    "window-controls-items",
                    ITEMS,
                    &self.config.items,
                    |this: &mut Self, items, cx| {
                        this.config.items = items;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.items.contains(&ControlItem::Pin), |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("window-controls-pin-mini-only"),
                    Some(rox_i18n::t!("window-controls-pin-mini-only.description")),
                    panel::toggle(
                        self.config.pin_mini_only,
                        |this: &mut Self, on, cx| {
                            this.config.pin_mini_only = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
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

    /// 24px buttons 4px apart inside the padding: 40 for one, 96 for three.
    fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
        let buttons = self
            .config
            .items
            .iter()
            .filter(|item| **item != ControlItem::Spacer)
            .count()
            .max(1);
        rox_panel_api::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                px(12. + 28. * buttons as f32),
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
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

#[cfg(test)]
mod tests {
    use super::{ControlItem, WindowControlsConfig, restyled};
    use rox_core::settings::ChromeStyle;

    use ControlItem::*;

    #[test]
    fn legacy_switches_fold_into_the_item_list() {
        let config: WindowControlsConfig =
            serde_json::from_str(r#"{"style": "icons", "mini": true, "pin": "mini"}"#).unwrap();
        assert!(config.items == vec![Pin, Mini, Minimize, Maximize, Close]);
        assert!(config.pin_mini_only);

        let config: WindowControlsConfig = serde_json::from_str(r#"{"style": "traffic"}"#).unwrap();
        assert!(config.items == vec![Close, Minimize, Maximize]);
        assert!(!config.pin_mini_only);
    }

    #[test]
    fn a_saved_list_wins_over_the_legacy_switches() {
        let config: WindowControlsConfig =
            serde_json::from_str(r#"{"items": ["close", "spacer", "mini"], "mini": false}"#)
                .unwrap();
        assert!(config.items == vec![Close, Spacer, Mini]);
    }

    #[test]
    fn a_new_save_reads_back_unchanged() {
        let config = WindowControlsConfig {
            items: vec![Mini, Spacer, Close],
            pin_mini_only: true,
            ..WindowControlsConfig::default()
        };
        let back: WindowControlsConfig =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert!(back.items == config.items);
        assert!(back.pin_mini_only);
    }

    #[test]
    fn a_style_switch_carries_stock_order_and_leaves_a_hand_order() {
        let items = [Pin, Minimize, Maximize, Close];
        assert!(
            restyled(&items, ChromeStyle::Icons, ChromeStyle::Traffic)
                == vec![Pin, Close, Minimize, Maximize]
        );

        let hand = [Close, Maximize, Minimize];
        assert!(restyled(&hand, ChromeStyle::Icons, ChromeStyle::Traffic) == hand.to_vec());

        // A hidden button doesn't make the rest a hand order.
        let partial = [Minimize, Spacer, Close];
        assert!(
            restyled(&partial, ChromeStyle::Icons, ChromeStyle::Traffic)
                == vec![Close, Spacer, Minimize]
        );
    }
}
