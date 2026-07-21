//! The group panel: two panels sharing one dock slot as a resizable
//! split, so a pair can ride a single tab. The dock's own splits can't
//! live inside a tab, so the group hosts its children itself through
//! [`crate::composite`]; the divider drag is the group's own, not the
//! dock's resize machinery.

use gpui::{
    canvas, div, prelude::*, px, relative, App, Axis, Context, Div, EventEmitter, FocusHandle,
    Focusable, SharedString, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::composite::{self, DividerState, Slot};
use crate::design::palette;
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::workspace::Workspace;

/// The divider's hit strip, wide enough to grab without reading as a gap.
const DIVIDER_W: f32 = 5.0;

/// How far toward either edge the divider can go; keeps both slots
/// grabbable.
const RATIO_MIN: f32 = 0.1;
const RATIO_MAX: f32 = 0.9;

fn default_ratio() -> f32 {
    0.5
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Stacked (top over bottom) instead of side by side.
    pub stacked: bool,
    /// The first slot's share of the split, [`RATIO_MIN`]..[`RATIO_MAX`].
    #[serde(default = "default_ratio")]
    pub ratio: f32,
}

impl Default for GroupConfig {
    fn default() -> Self {
        Self {
            chrome: PanelChrome::default(),
            stacked: false,
            ratio: default_ratio(),
        }
    }
}

pub struct GroupPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: GroupConfig,
    slots: [Slot; 2],
    divider: DividerState,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
}

impl GroupPanel {
    pub fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: GroupConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::restore(state, workspace, config, Vec::new(), cx)
    }

    /// Build with already-restored children, the layout-dump route in.
    /// The dump always carries two slots; a hand-edited or older one is
    /// padded out.
    pub fn restore(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: GroupConfig,
        mut slots: Vec<Slot>,
        cx: &mut Context<Self>,
    ) -> Self {
        slots.resize_with(2, || None);
        let mut fixed: [Slot; 2] = [None, None];
        for (slot, restored) in fixed.iter_mut().zip(slots) {
            *slot = restored;
        }
        GroupPanel {
            state,
            workspace,
            config,
            slots: fixed,
            divider: DividerState::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
        }
    }

    /// The hosted slots in split order, for the settings window's layout
    /// tree.
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    fn axis(&self) -> Axis {
        if self.config.stacked {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    fn set_slot(&mut self, ix: usize, slot: Slot, cx: &mut Context<Self>) {
        self.slots[ix] = slot;
        cx.notify();
    }

    /// One side of the split: the child's view, or the empty add
    /// affordance, under the floating slot controls.
    fn cell(&self, ix: usize, cx: &mut Context<Self>) -> Div {
        let content = match &self.slots[ix] {
            Some(child) => div().size_full().child(child.view()),
            None => {
                let weak = cx.entity().downgrade();
                composite::empty_slot(
                    if ix == 0 { "group-add-0" } else { "group-add-1" },
                    self.state.clone(),
                    self.workspace.clone(),
                    move |panel, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.set_slot(ix, Some(panel), cx));
                        }
                    },
                )
            }
        };
        let controls = self.slots[ix].clone().map(|child| {
            composite::corner_controls().child(composite::slot_button(
                ("group-slot", ix),
                child,
                self.state.clone(),
                self.workspace.clone(),
                move |this: &mut Self, panel, cx| this.set_slot(ix, Some(panel), cx),
                move |this: &mut Self, cx| this.set_slot(ix, None, cx),
                |menu, _| menu,
                cx,
            ))
        });
        div()
            .relative()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(content)
            .children(controls)
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let axis = self.axis();
        let ratio = self.config.ratio.clamp(RATIO_MIN, RATIO_MAX);
        let divider = self.divider.clone();
        let weak = cx.entity().downgrade();

        let first = self.cell(0, cx).map(|d| match axis {
            Axis::Horizontal => d.h_full().w(relative(ratio)),
            Axis::Vertical => d.w_full().h(relative(ratio)),
        });
        let second = self.cell(1, cx).flex_1();

        let divider_line = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match axis {
                Axis::Horizontal => d.w(px(DIVIDER_W)).h_full().cursor_col_resize(),
                Axis::Vertical => d.h(px(DIVIDER_W)).w_full().cursor_row_resize(),
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.divider.begin();
                    cx.notify();
                }),
            )
            .child(div().bg(palette::border()).map(|d| match axis {
                Axis::Horizontal => d.w(px(1.)).h_full(),
                Axis::Vertical => d.h(px(1.)).w_full(),
            }));

        let parent = composite::parent_button("Group", cx);
        div()
            .size_full()
            .relative()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            // The drag layer: records where the slots span painted and,
            // while a drag is live, keeps window-level handlers moving the
            // ratio. No hitbox of its own, so it never eats the slots'
            // clicks.
            .child(
                canvas(
                    {
                        let divider = self.divider.clone();
                        move |bounds, _, _| divider.set_bounds(bounds)
                    },
                    move |_, _, window, _| {
                        composite::divider_on_paint(&divider, axis, window, move |fraction, cx| {
                            if let Some(this) = weak.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.config.ratio = fraction.clamp(RATIO_MIN, RATIO_MAX);
                                    cx.notify();
                                });
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .size_full()
                    .flex()
                    .map(|d| match axis {
                        Axis::Horizontal => d.flex_row(),
                        Axis::Vertical => d.flex_col(),
                    })
                    .child(first)
                    .child(divider_line)
                    .child(second),
            )
            .child(composite::parent_controls().child(parent))
    }
}

impl PanelSettings for GroupPanel {
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
}

impl EventEmitter<PanelEvent> for GroupPanel {}

impl Focusable for GroupPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for GroupPanel {
    fn panel_name(&self) -> &'static str {
        "group"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Group")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        for slot in self.slots.iter().flatten() {
            slot.set_active(active, window, cx);
        }
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state.children = composite::dump_slots(&self.slots, cx);
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
        // The toggle names the arrangement a click lands on, not the
        // current one.
        let (flip_label, flip_icon) = if self.config.stacked {
            ("Split Side by Side", icons::MOVE_HORIZONTAL)
        } else {
            ("Split Stacked", icons::MOVE_VERTICAL)
        };
        let flip = cx.entity().downgrade();
        let swap = cx.entity().downgrade();
        let menu = menu
            .item(
                PopupMenuItem::new(flip_label)
                    .icon(Icon::default().path(flip_icon))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = flip.upgrade() {
                            this.update(cx, |this, cx| {
                                this.config.stacked = !this.config.stacked;
                                cx.notify();
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("Swap Panels")
                    .icon(Icon::default().path(icons::REFRESH_CW))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = swap.upgrade() {
                            this.update(cx, |this, cx| {
                                this.slots.swap(0, 1);
                                cx.notify();
                            });
                        }
                    }),
            );
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for GroupPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}
