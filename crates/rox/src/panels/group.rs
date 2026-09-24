//! The group panel: several panels sharing one dock slot as a resizable split.
//! The dock's own splits can't go inside a tab, so the group hosts its children
//! through [`crate::composite`] and drags its own dividers.

use gpui::{
    App, Axis, Context, Div, EventEmitter, FocusHandle, Focusable, SharedString, WeakEntity,
    Window, canvas, div, prelude::*, px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::composite::{self, DividerState, Slot};
use crate::workspace::Workspace;
use rox_design::assets::icons;
use rox_design::palette;
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;

const DIVIDER_W: f32 = 5.0;

/// Closest a seam gets to its neighbor or the edge, so every slot stays
/// grabbable.
const SHARE_MIN: f32 = 0.05;

fn default_ratio() -> f32 {
    0.5
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub stacked: bool,
    /// The first seam, from when a group was a fixed pair. Kept in step with
    /// `dividers[0]` so older builds still read the split.
    #[serde(default = "default_ratio")]
    pub ratio: f32,
    #[serde(default)]
    pub dividers: Vec<f32>,
}

impl Default for GroupConfig {
    fn default() -> Self {
        Self {
            chrome: PanelChrome::default(),
            stacked: false,
            ratio: default_ratio(),
            dividers: Vec::new(),
        }
    }
}

/// The stored seams when they fit the slot count, the pair-era ratio, or an
/// even spread. Ascending is enforced, so a hand-edited dump can't fold the
/// split over itself.
fn normalized_dividers(config: &GroupConfig, count: usize) -> Vec<f32> {
    let seams = count.saturating_sub(1);
    let mut dividers = config.dividers.clone();
    if dividers.is_empty() && seams == 1 {
        dividers = vec![config.ratio];
    }
    if dividers.len() != seams {
        return (1..count).map(|ix| ix as f32 / count as f32).collect();
    }
    let mut prev = 0.0;
    for seam in dividers.iter_mut() {
        *seam = seam.clamp(prev, 1.0);
        prev = *seam;
    }
    dividers
}

pub struct GroupPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: GroupConfig,
    slots: Vec<Slot>,
    dividers: Vec<DividerState>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    introduced: bool,
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

    /// Floored at a pair; seams that don't match the slot count re-derive.
    pub fn restore(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        mut config: GroupConfig,
        mut slots: Vec<Slot>,
        cx: &mut Context<Self>,
    ) -> Self {
        if slots.len() < 2 {
            slots.resize_with(2, || None);
        }
        config.dividers = normalized_dividers(&config, slots.len());
        let dividers = (1..slots.len()).map(|_| DividerState::default()).collect();
        GroupPanel {
            state,
            workspace,
            config,
            slots,
            dividers,
            focus: cx.focus_handle(),
            tab_panel: None,
            introduced: false,
        }
    }

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
        self.introduced = false;
        cx.notify();
    }

    fn sync_ratio(&mut self) {
        if let Some(first) = self.config.dividers.first() {
            self.config.ratio = *first;
        }
    }

    fn add_slot(&mut self, cx: &mut Context<Self>) {
        let last = self.config.dividers.last().copied().unwrap_or(0.0);
        self.config.dividers.push((last + 1.0) / 2.0);
        self.slots.push(None);
        self.dividers.push(DividerState::default());
        cx.notify();
    }

    /// Only an empty slot leaves, and a pair is the floor.
    fn remove_slot(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.slots.len() <= 2 || ix >= self.slots.len() || self.slots[ix].is_some() {
            return;
        }
        self.slots.remove(ix);
        let seam = ix.min(self.config.dividers.len() - 1);
        self.config.dividers.remove(seam);
        self.dividers.pop();
        self.sync_ratio();
        cx.notify();
    }

    fn shift(&mut self, ix: usize, forward: bool, cx: &mut Context<Self>) {
        let other = if forward { ix + 1 } else { ix.wrapping_sub(1) };
        if ix >= self.slots.len() || other >= self.slots.len() {
            return;
        }
        self.slots.swap(ix, other);
        cx.notify();
    }

    /// Held off its neighbors so no slot pinches shut.
    fn drag_seam(&mut self, ix: usize, fraction: f32, cx: &mut Context<Self>) {
        let seams = self.config.dividers.len();
        if ix >= seams {
            return;
        }
        let lo = if ix == 0 {
            0.0
        } else {
            self.config.dividers[ix - 1]
        } + SHARE_MIN;
        let hi = if ix + 1 == seams {
            1.0
        } else {
            self.config.dividers[ix + 1]
        } - SHARE_MIN;
        self.config.dividers[ix] = fraction.clamp(lo, hi.max(lo));
        self.sync_ratio();
        cx.notify();
    }

    fn cell(&self, ix: usize, cx: &mut Context<Self>) -> Div {
        let content = match &self.slots[ix] {
            // The group opted out of the dock's body menu, so the slot serves
            // the right-click itself.
            Some(child) => composite::menu_routed_slot(child, &self.tab_panel, cx),
            None => {
                let weak = cx.entity().downgrade();
                composite::empty_slot(
                    ("group-add", ix),
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
        // Finished layouts hide the builder's buttons; the Workspace page's
        // tree still swaps slots.
        let controls = if self.config.chrome.controls_hidden() {
            None
        } else {
            match self.slots[ix].clone() {
                Some(child) => {
                    let count = self.slots.len();
                    let stacked = self.config.stacked;
                    Some(composite::corner_controls().child(composite::slot_button(
                        ("group-slot", ix),
                        child,
                        self.state.clone(),
                        self.workspace.clone(),
                        move |this: &mut Self, panel, cx| this.set_slot(ix, Some(panel), cx),
                        move |this: &mut Self, cx| this.set_slot(ix, None, cx),
                        move |menu, weak| {
                            if count <= 2 {
                                return menu;
                            }
                            let (back_label, back_icon) = if stacked {
                                (rox_i18n::t!("group-panel-move-up"), icons::ARROW_UP)
                            } else {
                                (rox_i18n::t!("composite-move-left"), icons::CHEVRON_LEFT)
                            };
                            let (fwd_label, fwd_icon) = if stacked {
                                (rox_i18n::t!("group-panel-move-down"), icons::ARROW_DOWN)
                            } else {
                                (rox_i18n::t!("composite-move-right"), icons::CHEVRON_RIGHT)
                            };
                            let back = weak.clone();
                            let forward = weak;
                            menu.item(
                                PopupMenuItem::new(back_label)
                                    .icon(Icon::default().path(back_icon))
                                    .disabled(ix == 0)
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = back.upgrade() {
                                            this.update(cx, |this, cx| this.shift(ix, false, cx));
                                        }
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(fwd_label)
                                    .icon(Icon::default().path(fwd_icon))
                                    .disabled(ix + 1 >= count)
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = forward.upgrade() {
                                            this.update(cx, |this, cx| this.shift(ix, true, cx));
                                        }
                                    }),
                            )
                            .separator()
                        },
                        cx,
                    )))
                }
                _ => {
                    if self.slots.len() > 2 {
                        let weak = cx.entity().downgrade();
                        Some(
                            composite::corner_controls().child(
                                Button::new(("group-drop", ix))
                                    .icon(Icon::default().path(icons::CLOSE))
                                    .small()
                                    .ghost()
                                    .tooltip(rox_i18n::t!("group-panel-remove-slot"))
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = weak.upgrade() {
                                            this.update(cx, |this, cx| this.remove_slot(ix, cx));
                                        }
                                    }),
                            ),
                        )
                    } else {
                        None
                    }
                }
            }
        };
        let cell = div()
            .relative()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(content)
            .children(controls);
        // A capped child keeps its size; the seams spend the rest on the other
        // slots.
        composite::clamp_to_panel(cell, &self.slots[ix], cx)
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let group_title = rox_i18n::t!("group-panel-title");
        composite::report_hosted(
            self.slots.iter().flatten(),
            self.config.chrome.title.as_deref().unwrap_or(&group_title),
            cx,
        );

        let axis = self.axis();
        let seams = self.config.dividers.clone();
        let weak = cx.entity().downgrade();

        // Shares are flex bases, not hard spans, so a slot held to a size gives
        // unused space back to its neighbors. A hosted child's size cap applies
        // along the split.
        let share = |cell: Div, basis: f32, cap: Option<gpui::Size<gpui::Pixels>>| {
            cell.map(|d| match axis {
                Axis::Horizontal => d.h_full(),
                Axis::Vertical => d.w_full(),
            })
            .when_some(cap, |d, cap| {
                let along = match axis {
                    Axis::Horizontal => cap.width,
                    Axis::Vertical => cap.height,
                };
                if along < gpui::Pixels::MAX {
                    match axis {
                        Axis::Horizontal => d.max_w(along),
                        Axis::Vertical => d.max_h(along),
                    }
                } else {
                    d
                }
            })
            .flex_basis(relative(basis))
            .flex_grow()
            .flex_shrink()
        };

        // Seams draw at the panel's frame border width, widest side for an
        // uneven border, 1px when borderless.
        let split = self
            .config
            .chrome
            .theme
            .border_sides(rox_core::settings::app_frame().border)
            .max()
            .clamp(1.0, DIVIDER_W);
        let live = !rox_dock::resize_locked();

        let count = self.slots.len();
        let mut row = div()
            .size_full()
            .flex()
            .justify_center()
            .map(|d| match axis {
                Axis::Horizontal => d.flex_row(),
                Axis::Vertical => d.flex_col(),
            });
        for ix in 0..count {
            let start = if ix == 0 { 0.0 } else { seams[ix - 1] };
            let end = if ix + 1 == count { 1.0 } else { seams[ix] };
            let cap = self.slots[ix].as_ref().map(|child| child.max_size(cx));
            row = row.child(share(self.cell(ix, cx), (end - start).max(0.0), cap));
            if ix + 1 < count {
                let line = div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| match axis {
                        Axis::Horizontal => d.w(px(DIVIDER_W)).h_full(),
                        Axis::Vertical => d.h(px(DIVIDER_W)).w_full(),
                    })
                    .when(live, |d| {
                        d.map(|d| match axis {
                            Axis::Horizontal => d.cursor_col_resize(),
                            Axis::Vertical => d.cursor_row_resize(),
                        })
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if let Some(seam) = this.dividers.get(ix) {
                                    seam.begin();
                                }
                                cx.notify();
                            }),
                        )
                    })
                    // Follows the app's seams toggle, like the dock's handles.
                    .child(
                        div()
                            .when(rox_core::settings::seams(), |d| d.bg(palette::border()))
                            .map(|d| match axis {
                                Axis::Horizontal => d.w(px(split)).h_full(),
                                Axis::Vertical => d.h(px(split)).w_full(),
                            }),
                    );
                row = row.child(line);
            }
        }

        let parent = (!self.config.chrome.controls_hidden()).then(|| {
            composite::parent_controls().child(composite::parent_button(
                rox_i18n::t!("group-panel-title"),
                cx,
            ))
        });
        div()
            .size_full()
            .relative()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            .child(
                canvas(
                    {
                        let states = self.dividers.clone();
                        move |bounds, _, _| {
                            for seam in &states {
                                seam.set_bounds(bounds);
                            }
                        }
                    },
                    {
                        let states = self.dividers.clone();
                        move |_, _, window, _| {
                            for (ix, seam) in states.iter().enumerate() {
                                let weak = weak.clone();
                                composite::divider_on_paint(
                                    seam,
                                    axis,
                                    window,
                                    move |fraction, cx| {
                                        if let Some(this) = weak.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.drag_seam(ix, fraction, cx)
                                            });
                                        }
                                    },
                                );
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(row)
            .children(parent)
    }
}

impl PanelSettings for GroupPanel {
    fn composite(&self) -> bool {
        true
    }

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

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        composite::open_slot_settings(&self.slots, window, cx);
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("group-panel-title"),
        )
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
        rox_panel_api::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        rox_panel_api::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
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
        self.introduced = false;
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_panel = None;
        self.introduced = false;
        for child in self.slots.iter().flatten() {
            child.on_removed(window, cx);
        }
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let (flip_label, flip_icon) = if self.config.stacked {
            (
                rox_i18n::t!("group-panel-split-side-by-side"),
                icons::MOVE_HORIZONTAL,
            )
        } else {
            (
                rox_i18n::t!("group-panel-split-stacked"),
                icons::MOVE_VERTICAL,
            )
        };
        let flip = cx.entity().downgrade();
        let add = cx.entity().downgrade();
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
                PopupMenuItem::new(rox_i18n::t!("group-panel-add-slot"))
                    .icon(Icon::default().path(icons::PLUS))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = add.upgrade() {
                            this.update(cx, |this, cx| this.add_slot(cx));
                        }
                    }),
            );
        let menu = if self.slots.len() == 2 {
            let swap = cx.entity().downgrade();
            menu.item(
                PopupMenuItem::new(rox_i18n::t!("group-panel-swap-panels"))
                    .icon(Icon::default().path(icons::REFRESH_CW))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = swap.upgrade() {
                            this.update(cx, |this, cx| {
                                this.slots.swap(0, 1);
                                cx.notify();
                            });
                        }
                    }),
            )
        } else {
            menu
        };
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for GroupPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        composite::introduce_slots(
            self.slots.iter().flatten(),
            &self.tab_panel,
            &mut self.introduced,
            window,
            cx,
        );
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}
